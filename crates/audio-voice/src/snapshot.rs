//! The monitor's view of the conductor: plain snapshots of the score's
//! state, published by the daemon and rendered by the optional window (the
//! `ui` feature). Deliberately UI-free and hub-free — the same data could
//! later ride the hub to a separate visualiser voice.

use serde::Serialize;

use crate::composer::{EventTallies, ReplanReason, SlotRecord};
use crate::curve::{effective_curve, span_factor, Curve};
use crate::form::FormSummary;
use crate::harmony::VOCABULARY;
use crate::music::{curve_norm, HarmonySource, MusicState, CYCLE_LEN};

/// How many performed events the daemon's log ring keeps.
pub const EVENT_LOG_LEN: usize = 32;

/// How many points the builder samples each curve at — smooth enough for
/// the plots, cheap enough to publish every loop iteration.
const CURVE_SAMPLES: usize = 64;

/// One performed game event in the daemon's recent-past log.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct EventLogEntry {
    /// The hub clock when the event arrived.
    pub when: f64,
    pub event: crate::GameEvent,
}

/// Where the harmony currently comes from — the badge the UI shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum HarmonyKind {
    /// The deterministic nine-chord cycle.
    Cycle,
    /// A composer's plan, consumed slot by slot.
    Plan,
}

/// The clock view: where the piece stands on the hub timeline.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct ClockView {
    pub hub_time: f64,
    pub slot_start: f64,
    pub slot_end: f64,
    /// Chord slots elapsed — the harmonic cycle's phase.
    pub slot: u32,
}

/// The drive view: what the game is doing to the music right now.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct DriveView {
    pub aggro_locks: u32,
    /// Compressor-smoothed 0–1 intensity.
    pub intensity: f32,
    /// The intensity the harmonic rhythm plays at (holds its rung).
    pub rhythm_intensity: f32,
    /// Seconds left on the rhythm's hold (0 = not holding).
    pub rhythm_hold: f32,
    /// The player stands in degraded data — the harmony mutates there.
    pub degraded: bool,
    /// The player's slow-field progress toward the exit (0–1).
    pub objective: f32,
    /// The form sits in an episode — the dynamic sits one mark quieter.
    pub episode: bool,
}

/// One planned chord slot, named for the UI.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct PlannedSlotView {
    pub chord: u32,
    pub name: &'static str,
    pub function: &'static str,
    /// The slot's optional bass tone (an inversion — 0 root, 1 third,
    /// 2 fifth, 3 seventh).
    pub bass: Option<u32>,
}

/// The chord sounding right now, named for the UI.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ChordView {
    pub chord: u32,
    pub name: &'static str,
    pub function: &'static str,
}

/// The harmony view: what chord plays, why, and what's coming.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct HarmonyView {
    pub source: HarmonyKind,
    /// The plan's next slot to perform (None on the cycle).
    pub cursor: Option<usize>,
    /// Slots left in the plan (None on the cycle — it never depletes).
    pub slots_remaining: Option<usize>,
    /// The live plan's dramatic intent (shown when the source is a plan).
    pub intent: Option<String>,
    /// The planned slots, in order (empty on the cycle).
    pub planned: Vec<PlannedSlotView>,
    /// On the cycle: the turn's position and length — the fallback's
    /// phrase phase.
    pub turn_position: Option<(usize, usize)>,
    pub current: ChordView,
    /// The key the harmony lives in.
    pub key: String,
    /// The form controller's state — the tonal geography.
    pub form: FormSummary,
}

/// The curves view: the phrase's voicing contour and dynamic arc, both as
/// raw control points (for dots) and sampled (for lines).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct CurveViews {
    /// The upper voicing bound's control points, semitones from the tonic.
    pub upper_control: Vec<f64>,
    /// The lower voicing bound's control points.
    pub lower_control: Vec<f64>,
    /// The contour sampled across the phrase: (t, lower, upper) with the
    /// bounds sorted at each sample.
    pub contour: Vec<(f64, f64, f64)>,
    /// The dynamic arc's shape sampled across the phrase: (t, 0–1) — read
    /// relative to itself, like the engine reads it.
    pub dynamic_shape: Vec<(f64, f64)>,
    /// The phrase parameter of the slot sounding now — the plots' marker.
    pub phrase_t: f64,
    /// The conductor's span factor the bounds obey (1.0 at full flight,
    /// `SPAN_REST` at rest).
    pub span: f32,
}

/// The composer view: the planning tier's trace since the last publish.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ComposerView {
    /// Whether the composer tier is enabled (ollama reachable config).
    pub enabled: bool,
    /// Why the last plan was asked for.
    pub last_replan: Option<ReplanReason>,
    /// The tallies that accompanied the last replan decision.
    pub last_tallies: Option<EventTallies>,
}

/// A whole-score snapshot: everything the window renders, in one plain
/// value the conductor publishes and the UI borrows.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct UiSnapshot {
    pub clock: ClockView,
    pub drive: DriveView,
    pub harmony: HarmonyView,
    pub curves: CurveViews,
    pub composer: ComposerView,
    /// The recent chord slots, in performance order.
    pub history: Vec<SlotRecord>,
    /// The recent performed game events, in arrival order.
    pub events: Vec<EventLogEntry>,
}

/// The composer's inputs to a snapshot — copied out of the daemon's locals.
#[derive(Debug, Clone)]
pub struct ComposerInputs {
    pub enabled: bool,
    pub last_replan: Option<ReplanReason>,
    pub last_tallies: Option<EventTallies>,
}

/// The pieces the builder reads that live outside the [`MusicState`].
pub struct SnapshotInputs<'a> {
    pub hub_time: f64,
    pub slot_start: f64,
    pub slot_end: f64,
    pub state: &'a MusicState,
    pub form: &'a FormSummary,
    /// The live plan's intent (None on the cycle or before a plan lands).
    pub intent: Option<String>,
    pub history: &'a [SlotRecord],
    pub events: &'a [EventLogEntry],
    pub composer: ComposerInputs,
}

/// The curve pair (voicing contour and dynamic arc) a harmony source
/// plays: a plan's own curves, or the cycle's current tiles.
fn source_curves(state: &MusicState) -> (Curve, Curve) {
    match &state.harmony {
        HarmonySource::Plan { curve, dynamic, .. } => (curve.clone(), dynamic.clone()),
        HarmonySource::Cycle => (state.cycle_curve.clone(), state.cycle_dynamic.clone()),
    }
}

/// The phrase parameter of the slot sounding now — the same t the pads'
/// expression reads (the slot just rendered sits at cursor − 1).
fn phrase_t(state: &MusicState) -> f64 {
    match &state.harmony {
        HarmonySource::Plan { slots, cursor, .. } => {
            (cursor.saturating_sub(1) as f64) / slots.len().max(1) as f64
        }
        HarmonySource::Cycle => {
            ((state.slot as usize % CYCLE_LEN) as f64) / CYCLE_LEN as f64
        }
    }
}

impl UiSnapshot {
    /// Capture the conductor's state — pure, so the tests can drive it
    /// without a hub or a UI.
    pub fn capture(inputs: SnapshotInputs<'_>) -> Self {
        let state = inputs.state;

        let current_index = state.harmony.current_vocab(state.slot);
        let current_entry = &VOCABULARY[current_index];
        let current = ChordView {
            chord: current_index as u32,
            name: current_entry.name,
            function: current_entry.function.name(),
        };

        let (source, cursor, slots_remaining, planned, turn_position) = match &state.harmony {
            HarmonySource::Cycle => (
                HarmonyKind::Cycle,
                None,
                None,
                Vec::new(),
                Some((state.slot as usize % CYCLE_LEN, CYCLE_LEN)),
            ),
            HarmonySource::Plan { slots, cursor, .. } => {
                let planned = slots
                    .iter()
                    .map(|slot| {
                        let entry = &VOCABULARY[slot.chord as usize];
                        PlannedSlotView {
                            chord: slot.chord,
                            name: entry.name,
                            function: entry.function.name(),
                            bass: slot.bass,
                        }
                    })
                    .collect();
                (
                    HarmonyKind::Plan,
                    Some(*cursor),
                    Some(slots.len().saturating_sub(*cursor)),
                    planned,
                    None,
                )
            }
        };

        // The plots show what the voices obey: the spanned bounds.
        let (raw_curve, dynamic) = source_curves(state);
        let curve = effective_curve(&raw_curve, state.intensity);
        let contour = (0..=CURVE_SAMPLES)
            .map(|step| {
                let t = step as f64 / CURVE_SAMPLES as f64;
                let (lower, upper) = curve.contour(t);
                (t, lower, upper)
            })
            .collect();
        let dynamic_shape = (0..=CURVE_SAMPLES)
            .map(|step| {
                let t = step as f64 / CURVE_SAMPLES as f64;
                (t, curve_norm(&dynamic, t))
            })
            .collect();

        Self {
            clock: ClockView {
                hub_time: inputs.hub_time,
                slot_start: inputs.slot_start,
                slot_end: inputs.slot_end,
                slot: state.slot,
            },
            drive: DriveView {
                aggro_locks: state.aggro_locks,
                intensity: state.intensity,
                rhythm_intensity: state.rhythm_intensity,
                rhythm_hold: state.rhythm_hold,
                degraded: state.degraded,
                objective: state.objective,
                episode: state.episode,
            },
            harmony: HarmonyView {
                source,
                cursor,
                slots_remaining,
                intent: inputs.intent,
                planned,
                turn_position,
                current,
                key: state.key.name().to_string(),
                form: inputs.form.clone(),
            },
            curves: CurveViews {
                upper_control: curve.upper.clone(),
                lower_control: curve.lower.clone(),
                contour,
                dynamic_shape,
                phrase_t: phrase_t(state),
                span: span_factor(state.intensity),
            },
            composer: ComposerView {
                enabled: inputs.composer.enabled,
                last_replan: inputs.composer.last_replan,
                last_tallies: inputs.composer.last_tallies,
            },
            history: inputs.history.to_vec(),
            events: inputs.events.to_vec(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::composer::SlotRecord;
    use crate::curve::{default_curve, SPAN_REST};
    use crate::harmony::PlanSlot;
    use crate::music::{default_phrase_curve, KeyState};

    /// A default state with a chosen slot count.
    fn state_at(slot: u32) -> MusicState {
        let mut state = MusicState::default();
        state.slot = slot;
        state
    }

    fn inputs<'a>(state: &'a MusicState, form: &'a FormSummary) -> SnapshotInputs<'a> {
        SnapshotInputs {
            hub_time: 10.0,
            slot_start: 8.0,
            slot_end: 12.0,
            state,
            form,
            intent: None,
            history: &[],
            events: &[],
            composer: ComposerInputs {
                enabled: false,
                last_replan: None,
                last_tallies: None,
            },
        }
    }

    #[test]
    fn the_cycle_derives_its_turn_position_and_phrase_t() {
        let state = state_at(4);
        let form = FormSummary::default();
        let snapshot = UiSnapshot::capture(inputs(&state, &form));
        assert_eq!(snapshot.harmony.source, HarmonyKind::Cycle);
        assert_eq!(snapshot.harmony.turn_position, Some((4, CYCLE_LEN)));
        assert!((snapshot.curves.phrase_t - 4.0 / CYCLE_LEN as f64).abs() < 1e-9);
        // The cycle's chord names come from the same menu the plans use.
        assert_eq!(snapshot.harmony.current.chord, 4);
        assert_eq!(snapshot.harmony.current.name, VOCABULARY[4].name);
    }

    #[test]
    fn the_plan_derives_its_phrase_t_from_the_cursor() {
        let mut state = MusicState::default();
        state.slot = 100;
        state.key = KeyState { tonic: 2 };
        state.harmony = HarmonySource::Plan {
            slots: vec![
                PlanSlot { chord: 9, bass: None, suggested_beats: 0.0 },
                PlanSlot { chord: 10, bass: None, suggested_beats: 0.0 },
                PlanSlot { chord: 0, bass: None, suggested_beats: 0.0 },
                PlanSlot { chord: 1, bass: None, suggested_beats: 0.0 },
            ],
            curve: default_curve(),
            dynamic: default_phrase_curve(),
            cursor: 3,
        };
        let form = FormSummary::default();
        let snapshot = UiSnapshot::capture(inputs(&state, &form));

        assert_eq!(snapshot.harmony.source, HarmonyKind::Plan);
        assert_eq!(snapshot.harmony.cursor, Some(3));
        assert_eq!(snapshot.harmony.slots_remaining, Some(1));
        assert_eq!(snapshot.harmony.planned.len(), 4);
        assert_eq!(snapshot.harmony.planned[0].name, VOCABULARY[9].name);
        // The sounding slot is the one just performed (cursor − 1 = 2).
        assert!((snapshot.curves.phrase_t - 0.5).abs() < 1e-9);
        assert_eq!(snapshot.harmony.key, "E minor");
    }

    #[test]
    fn the_contour_samples_stay_sorted_and_the_shape_in_range() {
        let mut state = state_at(0);
        // Full flight: the effective curve is the drawn shape, untouched.
        state.intensity = 1.0;
        state.harmony = HarmonySource::Plan {
            slots: vec![
                PlanSlot { chord: 0, bass: None, suggested_beats: 0.0 },
                PlanSlot { chord: 9, bass: None, suggested_beats: 0.0 },
                PlanSlot { chord: 10, bass: None, suggested_beats: 0.0 },
                PlanSlot { chord: 1, bass: None, suggested_beats: 0.0 },
            ],
            curve: Curve {
                upper: vec![48.0, 20.0, 50.0],
                lower: vec![10.0, 14.0, 8.0],
            },
            dynamic: default_phrase_curve(),
            cursor: 1,
        };
        let form = FormSummary::default();
        let snapshot = UiSnapshot::capture(inputs(&state, &form));

        assert_eq!(snapshot.curves.contour.len(), CURVE_SAMPLES + 1);
        for (t, lower, upper) in &snapshot.curves.contour {
            assert!(lower <= upper, "bounds out of order at t = {t}");
        }
        // The samples start and end at the phrase's edges.
        assert_eq!(snapshot.curves.contour[0].0, 0.0);
        assert_eq!(snapshot.curves.contour.last().unwrap().0, 1.0);
        for (_, shape) in &snapshot.curves.dynamic_shape {
            assert!((0.0..=1.0).contains(shape));
        }
        // The control points ride along for the dots.
        assert_eq!(snapshot.curves.upper_control, vec![48.0, 20.0, 50.0]);
        assert!((snapshot.curves.span - 1.0).abs() < 1e-6);
    }

    #[test]
    fn the_snapshot_shows_the_spanned_bounds_at_rest() {
        let state = state_at(0);
        let form = FormSummary::default();
        let snapshot = UiSnapshot::capture(inputs(&state, &form));

        assert!((snapshot.curves.span - SPAN_REST).abs() < 1e-6);
        // The effective bounds fold toward the centre: the floor rises
        // above the raw curve's, the ceiling sits below it.
        for (t, lower, upper) in &snapshot.curves.contour {
            let (raw_low, raw_high) = default_curve().contour(*t);
            assert!(lower > &raw_low, "the folded floor rose at t = {t}");
            assert!(upper < &raw_high, "the folded ceiling fell at t = {t}");
        }
    }

    #[test]
    fn the_history_and_events_ride_along() {
        let mut state = state_at(3);
        state.aggro_locks = 4;
        state.degraded = true;
        let history = [SlotRecord {
            chord: 7,
            duration_secs: 1.5,
            cut: true,
            held: false,
        }];
        let events = [EventLogEntry {
            when: 9.25,
            event: crate::GameEvent::LevelUp,
        }];
        let form = FormSummary::default();
        let mut snapshot_inputs = inputs(&state, &form);
        snapshot_inputs.history = &history;
        snapshot_inputs.events = &events;
        let snapshot = UiSnapshot::capture(snapshot_inputs);

        assert_eq!(snapshot.history.len(), 1);
        assert!(snapshot.history[0].cut);
        assert_eq!(snapshot.events[0].event, crate::GameEvent::LevelUp);
        assert!(snapshot.drive.degraded);
        assert_eq!(snapshot.drive.aggro_locks, 4);
        assert_eq!(snapshot.clock.slot, 3);
    }
}
