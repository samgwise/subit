//! The composer: a slow planning tier over the performing engine.
//!
//! A local ollama model reads the score's recent past (the slot history)
//! and the game's present (the dramatic tallies) and lays out the next
//! phrase — chord slots from the vocabulary plus optional contour control
//! points. The engine performs in real time regardless: the conductor
//! keeps the clock, and a slow, wrong or absent model changes nothing —
//! rejected plans and transport failures keep the current phrase, and a
//! depleted one falls back to the deterministic cycle.

use std::time::Duration;

use serde::Serialize;
use serde_json::{json, Value};
use tokio::sync::{mpsc, watch};

use crate::curve::{default_curve, Curve};
use crate::form::FormSummary;
use crate::harmony::{validate, HarmonyPlan, VOCABULARY};

/// The slot history the context carries — the recent past the composer
/// reads (a phrase and a bit).
pub const HISTORY_LEN: usize = 16;

/// The slot count at which the composer starts planning the next phrase —
/// the margin that hides the model's latency behind the still-live plan.
pub const REPLAN_MARGIN_SLOTS: usize = 2;

/// Seconds before a composer request gives up. The plan it replaces is
/// still playing, so a timeout costs nothing but silence from the model —
/// but the margin has to cover a 12B model's constrained generation (a
/// plan takes 10–15 s here) and the first request's cold model load, which
/// ollama continues despite the cancelled call. The next request then
/// finds the model resident and lands inside the window.
pub const REQUEST_TIMEOUT_SECS: u64 = 30;

/// How far the aggro peak may climb above the phrase's average before the
/// jump counts as a sharp shift worth replanning around.
const AGGRO_JUMP_LOCKS: f32 = 3.0;

/// The system prompt: the grammar and the game's dramatic roles. The user
/// message carries the compact JSON context; the reply is the plan JSON.
pub const SYSTEM_PROMPT: &str = "\
You are the composer for subit, a generative game score in a minor \
mixture (a borrowed colour tints its diminished seventh). A \
deterministic conductor owns intensity: aggro locks drive the harmonic \
rhythm, the patterns and the bass pulse. You choose direction — the next \
phrase's chords and the register contour they play in. The form \
controller owns the key — home, a sequence a tone away, or an episode \
in a related key — so compose within the key the context reports, and \
let its phrase count colour how much ground you cover.

Reply with only a JSON plan: `slots` picks 4-8 chords from the numbered \
vocabulary — roman numerals relative to the key the context reports — \
and each slot may set `bass` to a chord-tone index (0 root, 1 third, \
2 fifth, 3 seventh) to invert mid-phrase for stepwise bass motion; the \
phrase edges always stay on the root. Optionally `curve_upper` and \
`curve_lower` give 2-8 control points each — voicing bounds in \
semitones from the tonic, drawn as gentle arcs — and `intent` names the \
dramatic aim in a few words.

The grammar: voice-lead the roots (stay within a few semitones of the \
sounding chord and of each other), never more than two of the same chord \
in a row, and close the phrase on a tonic or a dominant. Save the dim7 \
for a dominant's approach — E major prepares it. Calm scenes live on \
tonic and subdominant space; combat earns dominants and colour; a \
corrupted zone asks for the darker colour.

The run has an arc, and it is yours to score: the player's \
objective_progress (0 fresh from the spawn or just died back, 1 at the \
goal) steers direction — the far outbounds live on tonic and \
subdominant colour, the approach turns toward dominants and the \
borrowed F and C, and a death resets the run: sink to the minor colours \
and rebuild from home. The conductor will cut or hold your chords as \
the game demands — plan the harmony, not the clock.";

/// The response schema the model is constrained to — deliberately flat:
/// fixed keys, integer chord picks, shallow arrays. Grammar-constrained
/// sampling misbehaves on nested or alternative schemas. The chord
/// maximum tracks the vocabulary (a test keeps them in step).
///
/// Every number is bounded: the grammar bounds JSON shape, not generation,
/// and an unbounded `number` lets the model wander digits for thousands of
/// tokens (`suggested_beats: 1600000000000000` was the tell). Only proven
/// constructs appear — integer `minimum`/`maximum` and array
/// `minItems`/`maxItems` — because ollama 0.35's grammar loops on the rest
/// (float bounds, `maxLength`): the sampler rolls back for thousands of
/// tokens before escaping. The advisory `suggested_beats` stays out of the
/// schema entirely — the conductor owns the clock — and everything the
/// grammar can't say (array lengths, the intent's size, the curve's
/// fineness) remains the validator's job.
pub const PLAN_SCHEMA: &str = r#"{
    "type": "object",
    "properties": {
        "slots": {
            "type": "array",
            "items": {
                "type": "object",
                "properties": {
                    "chord": { "type": "integer", "minimum": 0, "maximum": 16 },
                    "bass": { "type": "integer", "minimum": 0, "maximum": 3 }
                },
                "required": ["chord"]
            },
            "minItems": 4,
            "maxItems": 16
        },
        "curve_upper": { "type": "array", "items": { "type": "integer", "minimum": -48, "maximum": 48 } },
        "curve_lower": { "type": "array", "items": { "type": "integer", "minimum": -48, "maximum": 48 } },
        "intent": { "type": "string" }
    },
    "required": ["slots"]
}"#;

/// A performed chord slot's record — the composer's memory of the recent
/// past, one entry per rendered slot.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct SlotRecord {
    /// The chord's vocabulary index.
    pub chord: u32,
    /// How long it actually sounded.
    pub duration_secs: f32,
    /// The conductor cut this chord short (an aggro spike).
    pub cut: bool,
    /// The rhythm held its rung through this slot (a fight easing).
    pub held: bool,
}

/// Dramatic tallies since the previous plan — what happened while the last
/// phrase played. The compact history the composer reads.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct EventTallies {
    pub mob_sweeps: u32,
    pub kills: u32,
    pub level_ups: u32,
    pub corruption_enters: u32,
    pub corruption_exits: u32,
    pub descents: u32,
    /// Deaths since the last plan — the run's resets.
    pub deaths: u32,
    /// Mean aggro locks across the phrase.
    pub aggro_average: f32,
    pub aggro_peak: u32,
}

/// The tallies' accumulator: events and aggro samples collected since the
/// last plan request, drained into an [`EventTallies`] snapshot.
#[derive(Debug, Default)]
pub struct Tally {
    mob_sweeps: u32,
    kills: u32,
    level_ups: u32,
    corruption_enters: u32,
    corruption_exits: u32,
    descents: u32,
    deaths: u32,
    aggro_sum: f32,
    aggro_samples: u32,
    aggro_peak: u32,
}

impl Tally {
    /// Record a game event — the dramatic moments the composer reads.
    pub fn record_event(&mut self, event: &crate::GameEvent) {
        match event {
            crate::GameEvent::MobSweep { kill_count, .. } => {
                self.mob_sweeps += 1;
                self.kills += kill_count;
            }
            crate::GameEvent::LevelUp => self.level_ups += 1,
            crate::GameEvent::CorruptionEnter => self.corruption_enters += 1,
            crate::GameEvent::CorruptionExit => self.corruption_exits += 1,
            crate::GameEvent::Descent => self.descents += 1,
            crate::GameEvent::Death => self.deaths += 1,
            _ => {}
        }
    }

    /// Record an aggro sample — the intensity trace since the last plan.
    pub fn record_aggro(&mut self, locks: u32) {
        self.aggro_sum += locks as f32;
        self.aggro_samples += 1;
        self.aggro_peak = self.aggro_peak.max(locks);
    }

    /// The snapshot since the last drain — and reset.
    pub fn drain(&mut self) -> EventTallies {
        let tallies = EventTallies {
            mob_sweeps: self.mob_sweeps,
            kills: self.kills,
            level_ups: self.level_ups,
            corruption_enters: self.corruption_enters,
            corruption_exits: self.corruption_exits,
            descents: self.descents,
            deaths: self.deaths,
            aggro_average: if self.aggro_samples > 0 {
                self.aggro_sum / self.aggro_samples as f32
            } else {
                0.0
            },
            aggro_peak: self.aggro_peak,
        };
        *self = Self::default();
        tallies
    }
}

/// Why the composer wants a fresh plan.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReplanReason {
    /// The live plan is down to the replan margin.
    Depleted,
    /// The corruption boundary moved.
    Corruption,
    LevelUp,
    Descent,
    /// Aggro spiked well above the phrase's average.
    AggroJump,
    /// The player died — the run reset to the spawn.
    Death,
}

/// Whether the composer should request a plan now, and why. Pure — the
/// daemon calls it at slot boundaries with the drained tallies. Sharp
/// shifts fire even without a live plan (they re-engage the composer
/// after a fallback); depletion only means something when one is live.
pub fn should_replan(slots_remaining: Option<usize>, tallies: &EventTallies) -> Option<ReplanReason> {
    if slots_remaining.is_some_and(|remaining| remaining <= REPLAN_MARGIN_SLOTS) {
        return Some(ReplanReason::Depleted);
    }
    if tallies.deaths > 0 {
        // The run reset — the most dramatic shift there is.
        return Some(ReplanReason::Death);
    }
    if tallies.corruption_enters > 0 || tallies.corruption_exits > 0 {
        return Some(ReplanReason::Corruption);
    }
    if tallies.level_ups > 0 {
        return Some(ReplanReason::LevelUp);
    }
    if tallies.descents > 0 {
        return Some(ReplanReason::Descent);
    }
    if tallies.aggro_peak as f32 > tallies.aggro_average + AGGRO_JUMP_LOCKS {
        return Some(ReplanReason::AggroJump);
    }
    None
}

/// The context snapshot the composer composes from.
#[derive(Debug, Clone, Default)]
pub struct ScoreContext {
    pub history: Vec<SlotRecord>,
    pub tallies: EventTallies,
    /// The vocabulary index sounding now (None before the first slot).
    pub sounding_chord: Option<usize>,
    /// The player stands in degraded data — the harmony mutates there.
    pub degraded: bool,
    /// The player's slow-field progress toward the exit (0 at the spawn
    /// or just died back, 1 at the goal) — the run's dramatic arc.
    pub progress: f32,
    /// The form controller's state — the key and its state of mind.
    pub form: FormSummary,
}

/// Build the compact JSON context the user message carries: the vocabulary
/// menu, the recent history, and the tallies.
pub fn build_context(ctx: &ScoreContext) -> Value {
    json!({
        "sounding_chord": ctx.sounding_chord,
        "degraded": ctx.degraded,
        "objective_progress": ctx.progress,
        "form": ctx.form,
        "vocabulary": VOCABULARY
            .iter()
            .enumerate()
            .map(|(index, entry)| {
                json!({
                    "index": index,
                    "name": entry.name,
                    "function": entry.function.name(),
                })
            })
            .collect::<Vec<_>>(),
        "history": ctx
            .history
            .iter()
            .map(|slot| {
                json!({
                    "chord": slot.chord,
                    "duration_secs": slot.duration_secs,
                    "cut": slot.cut,
                    "held": slot.held,
                })
            })
            .collect::<Vec<_>>(),
        "tallies": ctx.tallies,
    })
}

/// Parse and validate a plan reply — serde into the schema, then the
/// grammar's gate. A rejection keeps the current plan; the daemon falls
/// back to the cycle when the live plan depletes.
pub fn parse_plan(text: &str, prev_chord: Option<usize>) -> Result<HarmonyPlan, String> {
    let plan: HarmonyPlan = serde_json::from_str(text.trim())
        .map_err(|err| format!("not the plan schema: {err}"))?;
    validate(&plan, prev_chord)
        .map_err(|reason| format!("off the grammar: {reason}"))?;
    Ok(plan)
}

/// The contour a plan performs with: its own control points — stitched
/// from the previous phrase's tail so consecutive curves join — or the
/// neutral default for any side it left empty.
pub fn plan_curve(plan: &HarmonyPlan, previous: Option<&Curve>) -> Curve {
    let default = default_curve();
    let side = |points: &[f64], fallback: Vec<f64>| {
        if points.len() < 2 {
            fallback
        } else {
            points.to_vec()
        }
    };
    let curve = Curve {
        upper: side(&plan.curve_upper, default.upper),
        lower: side(&plan.curve_lower, default.lower),
    };
    let supplied = plan.curve_upper.len() >= 2 || plan.curve_lower.len() >= 2;
    match (supplied, previous) {
        (true, Some(tail)) => curve.extend_from(tail),
        _ => curve,
    }
}

/// The composer's request failures — everything except a rejected plan
/// (those are grammar, logged separately and never published).
#[derive(Debug)]
pub enum ComposerError {
    /// The transport failed (connection refused, timeout).
    Http(reqwest::Error),
    /// The model service returned an error status.
    Status(u16),
    /// The reply carried no content.
    Empty,
}

impl std::fmt::Display for ComposerError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ComposerError::Http(err) => write!(f, "transport: {err}"),
            ComposerError::Status(status) => write!(f, "status {status}"),
            ComposerError::Empty => write!(f, "empty reply"),
        }
    }
}

impl std::error::Error for ComposerError {}

/// The planner transport — a testable seam over the model client.
pub trait LlmClient {
    fn plan(
        &self,
        context: &Value,
    ) -> impl std::future::Future<Output = Result<String, ComposerError>> + Send;
}

/// The ollama client: plans over the local chat API, constrained to the
/// plan schema.
pub struct OllamaClient {
    http: reqwest::Client,
    endpoint: String,
    model: String,
}

impl OllamaClient {
    /// Build from the OLLAMA_* environment: `OLLAMA_URL` (default
    /// http://127.0.0.1:11434) and `OLLAMA_MODEL` (default qwen3:0.6b —
    /// the verified sweet spot: a plan in a couple of seconds. Heavy
    /// generalists roll back under ollama 0.35's grammar and take minutes).
    pub fn from_env() -> Self {
        let endpoint =
            std::env::var("OLLAMA_URL").unwrap_or_else(|_| "http://127.0.0.1:11434".into());
        let model = std::env::var("OLLAMA_MODEL").unwrap_or_else(|_| "qwen3:0.6b".into());
        Self::new(endpoint, model)
    }

    pub fn new(endpoint: String, model: String) -> Self {
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(REQUEST_TIMEOUT_SECS))
            .build()
            .expect("the reqwest client builds");
        Self { http, endpoint, model }
    }
}

impl LlmClient for OllamaClient {
    async fn plan(&self, context: &Value) -> Result<String, ComposerError> {
        let schema: Value =
            serde_json::from_str(PLAN_SCHEMA).expect("the plan schema is valid JSON");
        let body = json!({
            "model": self.model,
            "stream": false,
            "format": schema,
            "keep_alive": "10m",
            "options": { "temperature": 0.8 },
            "messages": [
                { "role": "system", "content": SYSTEM_PROMPT },
                { "role": "user", "content": context.to_string() },
            ],
        });
        let response = self
            .http
            .post(format!("{}/api/chat", self.endpoint))
            .json(&body)
            .send()
            .await
            .map_err(ComposerError::Http)?;
        let response = response.error_for_status().map_err(|err| match err.status() {
            Some(status) => ComposerError::Status(status.as_u16()),
            None => ComposerError::Http(err),
        })?;
        let reply: Value = response.json().await.map_err(ComposerError::Http)?;
        reply["message"]["content"]
            .as_str()
            .map(str::to_owned)
            .ok_or(ComposerError::Empty)
    }
}

/// Whether the composer is enabled: `OLLAMA_DISABLED` unset or not "1".
pub fn enabled_from_env() -> bool {
    std::env::var("OLLAMA_DISABLED").is_ok_and(|disabled| disabled != "1")
}

/// A request for the next phrase — the context snapshot to compose from.
#[derive(Debug, Clone)]
pub struct PlanRequest {
    /// The compact JSON context (see `build_context`).
    pub context: Value,
    /// The sounding chord's vocabulary index — the validator's boundary
    /// check. None when the plan opens the piece.
    pub prev_chord: Option<usize>,
    /// The previous phrase's curve, for tail stitching.
    pub previous_curve: Option<Curve>,
}

/// The accepted plan the daemon watches for: the validated plan plus its
/// stitched contour.
#[derive(Debug, Clone)]
pub struct PlanOutcome {
    pub plan: HarmonyPlan,
    pub curve: Curve,
}

/// Collapse queued requests to the newest — an older context is stale the
/// moment a newer arrives.
fn collapse_to_newest(
    mut request: PlanRequest,
    requests: &mut mpsc::Receiver<PlanRequest>,
) -> PlanRequest {
    while let Ok(newer) = requests.try_recv() {
        request = newer;
    }
    request
}

/// Spawn the composer task: consume plan requests, ask the client, parse +
/// validate, and publish the latest accepted plan to the watch channel.
/// Requests queue in a bounded channel (a full channel means one is
/// queued behind the in-flight request — the daemon skips); queued
/// requests collapse to the newest. A failed or rejected plan publishes
/// nothing — the live plan plays on.
pub fn spawn<L: LlmClient + Send + Sync + 'static>(
    client: L,
    mut requests: mpsc::Receiver<PlanRequest>,
    plans: watch::Sender<Option<PlanOutcome>>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        while let Some(request) = requests.recv().await {
            let request = collapse_to_newest(request, &mut requests);
            match client.plan(&request.context).await {
                Ok(text) => match parse_plan(&text, request.prev_chord) {
                    Ok(plan) => {
                        let curve = plan_curve(&plan, request.previous_curve.as_ref());
                        let _ = plans.send(Some(PlanOutcome { plan, curve }));
                    }
                    Err(reason) => tracing::warn!("composer plan rejected: {reason}"),
                },
                Err(err) => {
                    tracing::warn!("the composer request failed ({err}); the current plan plays on")
                }
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::GameEvent;

    fn plan_from(chords: &[u32]) -> HarmonyPlan {
        HarmonyPlan {
            slots: chords
                .iter()
                .map(|&chord| crate::harmony::PlanSlot {
                    chord,
                    bass: None,
                    suggested_beats: 0.0,
                })
                .collect(),
            curve_upper: vec![],
            curve_lower: vec![],
            intent: String::new(),
        }
    }

    #[test]
    fn the_context_shows_the_menu_history_and_tallies() {
        let ctx = ScoreContext {
            history: vec![SlotRecord {
                chord: 3,
                duration_secs: 2.0,
                cut: true,
                held: false,
            }],
            tallies: EventTallies {
                mob_sweeps: 2,
                kills: 5,
                aggro_average: 3.5,
                aggro_peak: 6,
                ..EventTallies::default()
            },
            sounding_chord: Some(3),
            degraded: true,
            progress: 0.75, // exactly representable in f32
            form: FormSummary::default(),
        };
        let context = build_context(&ctx);
        assert_eq!(context["sounding_chord"], 3);
        assert_eq!(context["degraded"], true);
        assert_eq!(context["objective_progress"], 0.75);
        assert_eq!(context["form"]["state"], "home");
        assert_eq!(context["form"]["key"], "D minor");
        // The menu is complete: every chord named and labelled.
        assert_eq!(context["vocabulary"].as_array().unwrap().len(), VOCABULARY.len());
        assert_eq!(context["vocabulary"][0]["function"], "tonic");
        assert_eq!(context["history"][0]["chord"], 3);
        assert_eq!(context["history"][0]["cut"], true);
        assert_eq!(context["tallies"]["kills"], 5);
        assert_eq!(context["tallies"]["aggro_peak"], 6);
    }

    #[test]
    fn parse_plan_round_trips_a_well_formed_reply() {
        let text = r#"{
            "slots": [{"chord": 9}, {"chord": 10}, {"chord": 0}, {"chord": 0}],
            "curve_upper": [30.0, 36.0],
            "curve_lower": [8.0, 10.0],
            "intent": "the fight builds"
        }"#;
        let plan = parse_plan(text, Some(1)).expect("the plan parses");
        assert_eq!(plan.slots.len(), 4);
        assert_eq!(plan.slots[2].chord, 0);
        assert_eq!(plan.curve_upper, vec![30.0, 36.0]);
        assert_eq!(plan.intent, "the fight builds");
    }

    #[test]
    fn parse_plan_rejects_malformed_and_off_grammar_replies() {
        assert!(parse_plan("not json at all", None).is_err());
        // Off the menu.
        let text = r#"{"slots": [{"chord": 99}, {"chord": 0}, {"chord": 0}, {"chord": 0}]}"#;
        assert!(parse_plan(text, None).is_err());
        // Well-formed but leapy: Dm straight to the dim7.
        let text = r#"{"slots": [{"chord": 0}, {"chord": 7}, {"chord": 0}, {"chord": 0}]}"#;
        assert!(parse_plan(text, None).is_err());
    }

    #[test]
    fn the_plan_curve_defaults_without_points_and_stitches_with_them() {
        // No points: the neutral default, untouched.
        let plan = plan_from(&[0, 1, 0, 1]);
        assert_eq!(plan_curve(&plan, None), default_curve());
        // Points without a tail: as supplied.
        let mut curved = plan_from(&[0, 1, 0, 1]);
        curved.curve_upper = vec![30.0, 34.0];
        curved.curve_lower = vec![8.0, 12.0];
        let expected = Curve { upper: vec![30.0, 34.0], lower: vec![8.0, 12.0] };
        assert_eq!(plan_curve(&curved, None), expected);
        // Points with a tail: stitched from the previous curve.
        let tail = Curve { upper: vec![40.0, 42.0], lower: vec![12.0, 14.0] };
        let stitched = plan_curve(&curved, Some(&tail));
        assert_eq!(stitched.upper.first(), Some(&42.0));
        assert_eq!(stitched.lower.first(), Some(&14.0));
        // A side left empty falls back to the default for that side.
        let mut upper_only = plan_from(&[0, 1, 0, 1]);
        upper_only.curve_upper = vec![30.0, 34.0];
        let curve = plan_curve(&upper_only, None);
        assert_eq!(curve.lower, default_curve().lower);
    }

    #[test]
    fn the_replan_triggers_fire_on_the_margin_and_sharp_shifts() {
        // Depletion: at or under the margin, only when a plan is live.
        assert_eq!(
            should_replan(Some(REPLAN_MARGIN_SLOTS), &EventTallies::default()),
            Some(ReplanReason::Depleted)
        );
        assert_eq!(should_replan(Some(5), &EventTallies::default()), None);
        assert_eq!(should_replan(None, &EventTallies::default()), None);

        // Sharp shifts.
        let corruption = EventTallies { corruption_enters: 1, ..EventTallies::default() };
        assert_eq!(should_replan(Some(5), &corruption), Some(ReplanReason::Corruption));
        let exits = EventTallies { corruption_exits: 1, ..EventTallies::default() };
        assert_eq!(should_replan(Some(5), &exits), Some(ReplanReason::Corruption));
        let level = EventTallies { level_ups: 1, ..EventTallies::default() };
        assert_eq!(should_replan(Some(5), &level), Some(ReplanReason::LevelUp));
        let descent = EventTallies { descents: 1, ..EventTallies::default() };
        assert_eq!(should_replan(Some(5), &descent), Some(ReplanReason::Descent));
        let death = EventTallies { deaths: 1, ..EventTallies::default() };
        assert_eq!(should_replan(Some(5), &death), Some(ReplanReason::Death));

        // An aggro jump: the peak racing three locks past the average.
        let jump = EventTallies { aggro_average: 1.0, aggro_peak: 5, ..EventTallies::default() };
        assert_eq!(should_replan(Some(5), &jump), Some(ReplanReason::AggroJump));
        let steady = EventTallies { aggro_average: 4.0, aggro_peak: 5, ..EventTallies::default() };
        assert_eq!(should_replan(Some(5), &steady), None);
    }

    #[test]
    fn tallies_accumulate_and_drain() {
        let mut tally = Tally::default();
        tally.record_event(&GameEvent::MobSweep { kill_count: 3, combo: 1 });
        tally.record_event(&GameEvent::MobSweep { kill_count: 2, combo: 2 });
        tally.record_event(&GameEvent::LevelUp);
        tally.record_event(&GameEvent::Descent);
        tally.record_event(&GameEvent::CorruptionEnter);
        tally.record_event(&GameEvent::CorruptionExit);
        tally.record_event(&GameEvent::Death);
        tally.record_event(&GameEvent::Dash); // untallied — the score reads it live
        tally.record_aggro(2);
        tally.record_aggro(6);

        let drained = tally.drain();
        assert_eq!(drained.mob_sweeps, 2);
        assert_eq!(drained.kills, 5);
        assert_eq!(drained.level_ups, 1);
        assert_eq!(drained.descents, 1);
        assert_eq!(drained.corruption_enters, 1);
        assert_eq!(drained.corruption_exits, 1);
        assert_eq!(drained.deaths, 1);
        assert!((drained.aggro_average - 4.0).abs() < 1e-6);
        assert_eq!(drained.aggro_peak, 6);

        // The drain resets: the next phrase's tallies start clean.
        assert_eq!(tally.drain(), EventTallies::default());
    }

    /// A scripted client: serves the queued replies front to back and
    /// records the contexts it was asked about.
    struct MockClient {
        replies: std::sync::Mutex<Vec<Result<String, ComposerError>>>,
        received: std::sync::Mutex<Vec<Value>>,
    }

    impl LlmClient for MockClient {
        async fn plan(&self, context: &Value) -> Result<String, ComposerError> {
            self.received.lock().unwrap().push(context.clone());
            self.replies.lock().unwrap().remove(0)
        }
    }

    fn request_with(marker: &str) -> PlanRequest {
        PlanRequest {
            context: json!({ "marker": marker }),
            prev_chord: None,
            previous_curve: None,
        }
    }

    /// A grammatical plan built around one chord — the marker the test
    /// reads back out of the published outcome. Every variant closes on a
    /// tonic or a dominant, as the grammar demands.
    fn plan_around(chord: u32) -> String {
        let slots: [u32; 4] = match chord {
            9 => [9, 10, 0, 1],   // G → A → Dm → D (tonic close)
            10 => [10, 0, 1, 10], // A → Dm → D → A (dominant close)
            _ => [0, 9, 10, 1],   // Dm → G → A → D (tonic close)
        };
        json!({
            "slots": slots.iter().map(|&c| json!({ "chord": c })).collect::<Vec<_>>()
        })
        .to_string()
    }

    #[tokio::test]
    async fn the_composer_task_publishes_the_latest_accepted_plan() {
        let client = MockClient {
            replies: std::sync::Mutex::new(vec![
                Ok(plan_around(9)),
                Ok(plan_around(10)),
            ]),
            received: std::sync::Mutex::new(vec![]),
        };
        let (tx, rx) = mpsc::channel(8);
        let (plan_tx, mut plan_rx) = watch::channel(None);
        let task = spawn(client, rx, plan_tx);

        tx.send(request_with("first")).await.unwrap();
        plan_rx.changed().await.unwrap();
        let outcome = plan_rx.borrow_and_update().clone().expect("the first plan is live");
        assert_eq!(outcome.plan.slots[0].chord, 9);

        // The next request replaces it — the latest accepted plan wins.
        tx.send(request_with("second")).await.unwrap();
        plan_rx.changed().await.unwrap();
        let outcome = plan_rx.borrow_and_update().clone().expect("the second plan is live");
        assert_eq!(outcome.plan.slots[0].chord, 10);

        // Dropping the request side ends the task — recv() sees the close.
        drop(tx);
        task.await.unwrap();
    }

    #[tokio::test]
    async fn a_failed_or_rejected_plan_keeps_the_previous_outcome() {
        let good = r#"{"slots": [{"chord": 0}, {"chord": 1}, {"chord": 0}, {"chord": 1}]}"#;
        let client = MockClient {
            replies: std::sync::Mutex::new(vec![
                Ok(good.into()),
                Err(ComposerError::Status(500)),
                Ok("not json".into()),
            ]),
            received: std::sync::Mutex::new(vec![]),
        };
        let (tx, rx) = mpsc::channel(8);
        let (plan_tx, mut plan_rx) = watch::channel(None);
        let task = spawn(client, rx, plan_tx);

        // The first plan lands.
        tx.send(request_with("good")).await.unwrap();
        plan_rx.changed().await.unwrap();
        let outcome = plan_rx.borrow_and_update().clone().expect("the good plan is live");
        assert_eq!(outcome.plan.slots[0].chord, 0);

        // A transport failure and a malformed reply publish nothing — the
        // good plan plays on. (The two requests may collapse to one; either
        // way nothing is published.)
        tx.send(request_with("failed")).await.unwrap();
        tx.send(request_with("malformed")).await.unwrap();
        tokio::time::sleep(Duration::from_millis(20)).await;
        assert!(plan_rx.borrow_and_update().is_some());
        assert_eq!(plan_rx.borrow().as_ref().unwrap().plan.slots[0].chord, 0);

        // Dropping the request side ends the task — recv() sees the close.
        drop(tx);
        task.await.unwrap();
    }

    #[test]
    fn queued_requests_collapse_to_the_newest() {
        let (tx, mut rx) = mpsc::channel(8);
        tx.try_send(request_with("first")).unwrap();
        tx.try_send(request_with("second")).unwrap();
        tx.try_send(request_with("third")).unwrap();
        let collapsed = collapse_to_newest(request_with("pulled"), &mut rx);
        assert_eq!(collapsed.context["marker"], "third");
        // The queue is drained behind the collapse.
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn the_schema_and_vocabulary_agree() {
        // The schema's chord maximum is the menu's last index — kept in
        // step by construction here.
        let schema: Value = serde_json::from_str(PLAN_SCHEMA).unwrap();
        assert_eq!(
            schema["properties"]["slots"]["items"]["properties"]["chord"]["maximum"],
            VOCABULARY.len() as u32 - 1
        );
        assert_eq!(VOCABULARY.len(), 17);
    }
}
