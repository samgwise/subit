//! The counsellor: a fast decision tier over ollama's decision models —
//! Tev1 via the `/v1/systemone` endpoint (the Jev API). Where the
//! composer plans phrases, the counsellor makes picks: tie-breaks,
//! shape rolls, walk steps — and answers the form's boolean
//! questions. Every decision fails closed — a transport
//! error, a timeout, or an answer that doesn't beat the roll it
//! replaces defers to the deterministic roll, and the score never
//! stalls on the model.

use std::time::Duration;

use rand::rngs::SmallRng;
use rand::SeedableRng;
use serde_json::{json, Value};
use tokio::sync::watch;

use crate::form::FormDecisions;
use crate::harmony::{HarmonyPlan, VOCABULARY};
use crate::music::BassStrategy;

/// Seconds before a decision gives up; the fallback roll plays on.
/// Every decision is asynchronous and fail-closed, so the budget is
/// generous — the multi-question calls (the form's, the ranked walk's)
/// brush a full second on a warm model.
pub const DECISION_TIMEOUT_SECS: u64 = 2;

/// How far a pick's probability must clear the uniform roll it
/// replaces. The endpoint's own confidence shrinks as the option
/// count grows — a clear 20-way pick scores about 0.25 — so
/// acceptance is judged against the roll itself: 0.29 over a 0.05
/// roll is signal, a 58/42 two-option lean is a coin flip and defers.
pub const PROBABILITY_MARGIN: f64 = 0.1;

/// Seconds the warm-up's load may take — a cold 4b model loads in one
/// lump of tens of seconds.
pub const WARMUP_TIMEOUT_SECS: u64 = 120;

/// A choice option — the key the answer names and the words the model
/// reads.
#[derive(Debug, Clone)]
pub struct ChoiceOption {
    pub key: String,
    pub description: String,
}

/// A boolean question — the endpoint's noul type. The answer names
/// its P(true) directly.
#[derive(Debug, Clone, Copy)]
pub struct NoulQuestion {
    pub name: &'static str,
    pub instructions: &'static str,
}

/// The form's boolean questions — asked only when the next tick
/// actually faces the roll, in one call.
pub const CHANGE_QUESTION: NoulQuestion = NoulQuestion {
    name: "change",
    instructions: "Should the form change key for the next phrase of a generative game score?",
};
pub const SEQUENCE_QUESTION: NoulQuestion = NoulQuestion {
    name: "sequence",
    instructions: "If the form changes, should it be a brief one-phrase sequence rather than \
        a longer episode?",
};
pub const RETURN_QUESTION: NoulQuestion = NoulQuestion {
    name: "return_home",
    instructions: "Should the episode end and the form return home now?",
};

/// One ranked transition's distribution: the criteria keys mapped to
/// their probabilities. A missing or malformed answer is no map at all
/// (fail closed) — the probabilities ORDER the walk's candidates;
/// they never accept.
#[derive(Debug, Clone, Default)]
pub struct ProbabilityMap(serde_json::Map<String, Value>);

impl ProbabilityMap {
    /// Read a reply answer's probabilities object; None when the
    /// answer carries no distribution.
    pub(crate) fn from_value(answer: &Value) -> Option<Self> {
        Some(Self(answer.get("probabilities")?.as_object()?.clone()))
    }

    /// The probability the model gave this key (None when unscored or
    /// not a number).
    pub fn probability(&self, key: &str) -> Option<f64> {
        self.0.get(key)?.as_f64()
    }

    /// The distribution's favourite — the key and its probability.
    pub fn favourite(&self) -> Option<(&str, f64)> {
        self.0
            .iter()
            .filter_map(|(key, probability)| Some((key.as_str(), probability.as_f64()?)))
            .max_by(|a, b| a.1.total_cmp(&b.1))
    }
}

/// One ranked transition's question — a named choice over the
/// transition's candidates, answered with their distribution.
#[derive(Debug, Clone)]
pub struct RankedQuestion {
    pub name: String,
    pub instructions: String,
    pub options: Vec<ChoiceOption>,
}

/// One decision: the state to judge and a choice question over it.
#[derive(Debug, Clone)]
pub struct Decision {
    /// The state to judge — a compact JSON (the model's context window
    /// is about 2 000 tokens; keep it well inside).
    pub state: Value,
    pub instructions: String,
    pub options: Vec<ChoiceOption>,
}

/// The outcome: the picked option's key — None when the pick defers
/// (it failed to beat the roll) to the deterministic roll.
#[derive(Debug, Clone, PartialEq)]
pub struct DecisionOutcome {
    pub choice: Option<String>,
    pub confidence: f32,
}

/// The decision transport's failures.
#[derive(Debug)]
pub enum DecisionError {
    /// The transport failed (the server down, the timeout).
    Http(reqwest::Error),
    /// The server returned an error status.
    Status(u16),
    /// The reply's shape surprised us.
    Invalid(String),
}

impl std::fmt::Display for DecisionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DecisionError::Http(err) => write!(f, "transport: {err}"),
            DecisionError::Status(status) => write!(f, "status {status}"),
            DecisionError::Invalid(reason) => write!(f, "invalid reply: {reason}"),
        }
    }
}

impl std::error::Error for DecisionError {}

/// The decision transport — a testable seam (the composer's
/// LlmClient pattern).
pub trait DecisionClient {
    fn decide(
        &self,
        decision: &Decision,
    ) -> impl std::future::Future<Output = Result<DecisionOutcome, DecisionError>> + Send;

    /// The boolean questions, asked in ONE call and answered by name —
    /// each `Some` only when the opinion clears a coin by
    /// [`PROBABILITY_MARGIN`] (inside the band the loaded roll plays).
    fn decide_noul(
        &self,
        state: &Value,
        questions: &[NoulQuestion],
    ) -> impl std::future::Future<Output = Result<Vec<Option<bool>>, DecisionError>> + Send;

    /// The ranked questions: ONE call, each answered with its
    /// candidates' probability distribution (None where the answer is
    /// missing or malformed — fail closed). The probabilities ORDER
    /// the walk's candidates; they never accept.
    fn decide_ranked(
        &self,
        state: &Value,
        questions: &[RankedQuestion],
    ) -> impl std::future::Future<Output = Result<Vec<Option<ProbabilityMap>>, DecisionError>>
           + Send;

    /// Bring the decision model resident — a no-op for in-memory
    /// transports. Fired at spawn and again after a transport failure,
    /// because a cold load (tens of seconds) dwarfs the decision
    /// timeout: without it every decision times out while the weights
    /// stream in.
    fn warm_up(&self) -> impl std::future::Future<Output = ()> + Send {
        std::future::ready(())
    }
}

/// The ollama system-one client: POST {url}/v1/systemone with the
/// model, the state and the named choice question.
pub struct SystemOneClient {
    http: reqwest::Client,
    endpoint: String,
    model: String,
}

impl SystemOneClient {
    /// Build from the env: `OLLAMA_URL` (the composer's own base URL)
    /// and `TEV1_MODEL` (default tev1:4b — the 0.8b sibling is the
    /// documented low-memory option).
    pub fn from_env() -> Self {
        let endpoint =
            std::env::var("OLLAMA_URL").unwrap_or_else(|_| "http://127.0.0.1:11434".into());
        let model = std::env::var("TEV1_MODEL").unwrap_or_else(|_| "tev1:4b".into());
        Self::new(endpoint, model)
    }

    pub fn new(endpoint: String, model: String) -> Self {
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(DECISION_TIMEOUT_SECS))
            .build()
            .expect("the reqwest client builds");
        Self { http, endpoint, model }
    }
}

impl DecisionClient for SystemOneClient {
    async fn decide(&self, decision: &Decision) -> Result<DecisionOutcome, DecisionError> {
        let criteria: Value = decision
            .options
            .iter()
            .map(|option| (option.key.clone(), json!(option.description)))
            .collect();
        let body = json!({
            "model": self.model,
            "state": decision.state,
            "questions": {
                "pick": {
                    "type": "choice",
                    "instructions": decision.instructions,
                    "criteria": criteria,
                }
            },
            "keep_alive": "10m",
        });
        let reply = self.post_systemone(body).await?;
        Self::parse_reply(&reply, decision)
    }

    async fn decide_noul(
        &self,
        state: &Value,
        questions: &[NoulQuestion],
    ) -> Result<Vec<Option<bool>>, DecisionError> {
        let entries: Value = questions
            .iter()
            .map(|question| {
                (
                    question.name.to_string(),
                    json!({ "type": "noul", "instructions": question.instructions }),
                )
            })
            .collect();
        let body = json!({
            "model": self.model,
            "state": state,
            "questions": entries,
            "keep_alive": "10m",
        });
        let reply = self.post_systemone(body).await?;
        // The raw answers ride the log — the model's own P(true) per
        // question, taken or deferred — the cheapest way to hear what
        // it thinks between the form's moves.
        tracing::info!(
            answers = %reply
                .get("answers")
                .map(|answers| answers.to_string())
                .unwrap_or_default(),
            "the counsellor's form opinions"
        );
        Ok(questions
            .iter()
            .map(|question| Self::parse_noul(&reply, question.name))
            .collect())
    }

    async fn decide_ranked(
        &self,
        state: &Value,
        questions: &[RankedQuestion],
    ) -> Result<Vec<Option<ProbabilityMap>>, DecisionError> {
        let entries: Value = questions
            .iter()
            .map(|question| {
                let criteria: Value = question
                    .options
                    .iter()
                    .map(|option| (option.key.clone(), json!(option.description)))
                    .collect();
                (
                    question.name.clone(),
                    json!({
                        "type": "choice",
                        "instructions": question.instructions,
                        "criteria": criteria,
                    }),
                )
            })
            .collect();
        let body = json!({
            "model": self.model,
            "state": state,
            "questions": entries,
            "keep_alive": "10m",
        });
        let reply = self.post_systemone(body).await?;
        let maps: Vec<Option<ProbabilityMap>> = questions
            .iter()
            .map(|question| {
                reply
                    .get("answers")
                    .and_then(|answers| answers.get(&question.name))
                    .and_then(ProbabilityMap::from_value)
            })
            .collect();
        // The favourites ride the log — the model's per-transition
        // read of the walk.
        for (question, map) in questions.iter().zip(maps.iter()) {
            if let Some(favourite) = map.as_ref().and_then(ProbabilityMap::favourite) {
                    tracing::info!(
                    transition = %question.name,
                    pick = favourite.0,
                    probability = favourite.1,
                    "the counsellor's ranked pick"
                );
            }
        }
        Ok(maps)
    }

    fn warm_up(&self) -> impl std::future::Future<Output = ()> + Send {
        // The cold load is one lump of many seconds — far past the
        // decision timeout — so the warm-up posts a cheap two-option
        // question with a generous timeout (the load, not the answer,
        // is the point) and the usual keep_alive, leaving the model
        // resident for the run's turns. A failed warm-up is harmless:
        // every decision fails closed and the deterministic roll
        // plays until one succeeds.
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(WARMUP_TIMEOUT_SECS))
            .build()
            .expect("the warm-up client builds");
        let endpoint = self.endpoint.clone();
        let model = self.model.clone();
        async move {
            let started = std::time::Instant::now();
            let body = json!({
                "model": model,
                "state": {},
                "questions": {
                    "warm": {
                        "type": "choice",
                        "instructions": "Is this a warm-up?",
                        "criteria": { "yes": "true", "no": "false" },
                    }
                },
                "keep_alive": "10m",
            });
            match http
                .post(format!("{endpoint}/v1/systemone"))
                .json(&body)
                .send()
                .await
            {
                Ok(response) if response.status().is_success() => tracing::info!(
                    loaded_in_secs = started.elapsed().as_secs_f32(),
                    "the counsellor's model is warm"
                ),
                Ok(response) => tracing::warn!(
                    status = %response.status(),
                    "the counsellor's warm-up failed; the deterministic roll plays on"
                ),
                Err(err) => tracing::warn!(
                    "the counsellor's warm-up failed ({err}); the deterministic roll plays on"
                ),
            }
        }
    }
}

impl SystemOneClient {
    /// POST a built body to the system-one endpoint and return the
    /// reply's JSON.
    async fn post_systemone(&self, body: Value) -> Result<Value, DecisionError> {
        let response = self
            .http
            .post(format!("{}/v1/systemone", self.endpoint))
            .json(&body)
            .send()
            .await
            .map_err(DecisionError::Http)?;
        let response = response
            .error_for_status()
            .map_err(|err| match err.status() {
                Some(status) => DecisionError::Status(status.as_u16()),
                None => DecisionError::Http(err),
            })?;
        response.json().await.map_err(DecisionError::Http)
    }

    /// A noul answer names its P(true) directly (no confidence, no
    /// probabilities map) — `Some` only when the opinion clears a coin
    /// by [`PROBABILITY_MARGIN`]; a malformed or missing answer defers.
    fn parse_noul(reply: &Value, name: &str) -> Option<bool> {
        let probability = reply.get("answers")?.get(name)?.get("noul")?.as_f64()?;
        if (probability - 0.5).abs() >= PROBABILITY_MARGIN {
            Some(probability > 0.5)
        } else {
            None
        }
    }

    /// Map the reply's answer back to an option key — deferring (a None
    /// choice) when the pick fails to beat the roll it replaces, or
    /// the shape surprises us.
    fn parse_reply(reply: &Value, decision: &Decision) -> Result<DecisionOutcome, DecisionError> {
        let answer = reply
            .get("answers")
            .and_then(|answers| answers.get("pick"))
            .ok_or_else(|| DecisionError::Invalid("no pick answer".into()))?;
        let confidence = answer
            .get("confidence")
            .and_then(Value::as_f64)
            .unwrap_or(0.0) as f32;
        let choice = answer
            .get("choice")
            .and_then(Value::as_str)
            .map(str::to_owned);
        let choice = match choice {
            Some(key) if decision.options.iter().any(|option| option.key == key) => Some(key),
            Some(key) => {
                return Err(DecisionError::Invalid(format!("off the options: {key}")));
            }
            None => None,
        };
        // The endpoint's own confidence is a margin-style number that
        // shrinks as the option count grows (a clear 20-way pick
        // scores about 0.25), so acceptance reads the picked option's
        // probability against the uniform roll it replaces: it must
        // beat the roll by [`PROBABILITY_MARGIN`].
        let roll = 1.0 / decision.options.len().max(1) as f64;
        let choice = choice.filter(|key| {
            answer
                .get("probabilities")
                .and_then(|probabilities| probabilities.get(key.as_str()))
                .and_then(Value::as_f64)
                .unwrap_or(0.0)
                >= roll + PROBABILITY_MARGIN
        });
        Ok(DecisionOutcome { choice, confidence })
    }
}

/// A counselled turn: the walked plan, the shape names the counsellor
/// picked (for the log and the monitor), and the bass character per
/// slot (None = the hug).
#[derive(Debug, Clone)]
pub struct CounselledTurn {
    pub plan: HarmonyPlan,
    pub contour_shape: &'static str,
    pub dynamic_shape: &'static str,
    pub bass: Vec<Option<BassStrategy>>,
}

/// Spawn the counsellor task: consume requests — a turn's counselled
/// walk (re-rolling the tiles to the picked shapes, re-walking with
/// the model's picks, publishing on the turns' watch) or a form tick's
/// opinions (one noul call, published with its generation on the
/// form's watch). A failed or deferred decision publishes nothing —
/// the deterministic roll plays on. The model warms at spawn (and
/// again after any transport failure) — a cold load dwarfs the
/// decision timeout. Returns the request sender and the views.
pub fn spawn<C: DecisionClient + Send + Sync + 'static>(
    client: C,
) -> (tokio::sync::mpsc::Sender<CounselRequest>, CounselView) {
    let (request_tx, mut request_rx) = tokio::sync::mpsc::channel::<CounselRequest>(4);
    let (turn_tx, turn_rx) = watch::channel(None);
    let (form_tx, form_rx) = watch::channel(None);
    tokio::spawn(async move {
        client.warm_up().await;
        while let Some(request) = request_rx.recv().await {
            match request {
                CounselRequest::Turn(turn) => {
                    // The tile pick: one call, two questions — the
                    // contour shape and the dynamic shape to follow.
                    let state = json!({
                        "form": turn.form_summary,
                        "world_integrity": turn.integrity,
                    });
                    let decision = Decision {
                        state,
                        instructions: "Pick the shapes for the next phrase of a \
                            generative game score, given its dramatic state."
                            .into(),
                        options: crate::curve::shape_pair_options(),
                    };
                    let outcome = client.decide(&decision).await;
                    match outcome {
                        Ok(outcome) => {
                            if let Some(key) = &outcome.choice {
                                tracing::info!(
                                    pick = %key,
                                    confidence = outcome.confidence,
                                    "the counsellor picked the shapes"
                                );
                            }
                            let picked = outcome
                                .choice
                                .as_deref()
                                .and_then(crate::curve::shape_pair_from_key);
                            let (contour, dynamic) =
                                picked.unwrap_or((turn.contour, turn.dynamic));
                            let dynamic_curve = dynamic.curve();
                            let mut rng = SmallRng::seed_from_u64(turn.seed);
                            let walked = crate::network::counselled_walk(
                                &client,
                                turn.current,
                                &dynamic_curve,
                                turn.integrity,
                                &turn.form_summary,
                                &mut rng,
                            );
                            if let Some(plan) = walked.await {
                                // The bass's character: a second ranked
                                // call, a question per slot of the walked
                                // plan (the strategies as criteria, keyed
                                // by name).
                                let questions: Vec<RankedQuestion> = plan
                                    .slots
                                    .iter()
                                    .enumerate()
                                    .map(|(slot, chord)| RankedQuestion {
                                        name: format!("b{slot}"),
                                        instructions: format!(
                                            "The bass line for this phrase's {} — which character \
                                             should it take?",
                                            VOCABULARY[chord.chord as usize].name
                                        ),
                                        options: BassStrategy::ALL
                                            .iter()
                                            .map(|strategy| ChoiceOption {
                                                key: strategy.name().into(),
                                                description: strategy.description().into(),
                                            })
                                            .collect(),
                                    })
                                    .collect();
                                let state = json!({
                                    "form": turn.form_summary,
                                    "world_integrity": turn.integrity,
                                    "contour": turn.contour.name(),
                                    "dynamic": turn.dynamic.name(),
                                    "walk": plan
                                        .slots
                                        .iter()
                                        .map(|slot| VOCABULARY[slot.chord as usize].name)
                                        .collect::<Vec<_>>(),
                                });
                                let bass = match client.decide_ranked(&state, &questions).await {
                                    Ok(maps) => maps
                                        .iter()
                                        .map(|map| {
                                            map.as_ref()
                                                .and_then(ProbabilityMap::favourite)
                                                .and_then(|(key, _)| match key {
                                                    "hug" => Some(BassStrategy::Hug),
                                                    "anchor" => Some(BassStrategy::Anchor),
                                                    "walk" => Some(BassStrategy::Walk),
                                                    _ => None,
                                                })
                                        })
                                        .collect(),
                                    Err(err) => {
                                        tracing::warn!(
                                            "the counsellor's bass pick failed ({err}); the hug plays on"
                                        );
                                        Vec::new()
                                    }
                                };
                                let _ = turn_tx.send(Some(CounselledTurn {
                                    plan,
                                    contour_shape: contour.name(),
                                    dynamic_shape: dynamic.name(),
                                    bass,
                                }));
                            }
                        }
                        Err(err) => {
                            tracing::warn!("the counsellor's decision failed ({err}); the deterministic roll plays on");
                            // A cold model times out every decision — the
                            // load re-fires so the next turn gets decisions.
                            client.warm_up().await;
                        }
                    }
                }
                CounselRequest::Form(form) => {
                    match client.decide_noul(&form.state, form.questions).await {
                        Ok(opinions) => {
                            // The answers map back by question name — the
                            // request may have carried any subset.
                            let mut decisions = FormDecisions::default();
                            for (question, opinion) in form.questions.iter().zip(opinions) {
                                match question.name {
                                    "change" => decisions.change = opinion,
                                    "sequence" => decisions.sequence = opinion,
                                    "return_home" => decisions.return_home = opinion,
                                    _ => {}
                                }
                            }
                            let _ = form_tx.send(Some((form.generation, decisions)));
                        }
                        Err(err) => tracing::warn!(
                            "the counsellor's form opinions failed ({err}); the loaded rolls play on"
                        ),
                    }
                }
            }
        }
    });
    (request_tx, CounselView { rx: turn_rx, form_rx })
}

/// A request to the counsellor task: a turn's counselled walk or the
/// form's next-tick opinions.
#[derive(Debug)]
pub enum CounselRequest {
    Turn(TurnRequest),
    Form(FormRequest),
}

/// A turn request: everything the counselled walk needs.
#[derive(Debug)]
pub struct TurnRequest {
    pub current: usize,
    pub contour: crate::curve::ContourShape,
    pub dynamic: crate::curve::DynamicShape,
    pub integrity: f32,
    pub form_summary: Value,
    /// A seed for the walk's remaining shuffles — deterministic per turn.
    pub seed: u64,
}

/// A form request: the opinions asked for the next phrase tick — only
/// the questions that tick will face. The generation guards the
/// application — a late reply never steers a later tick.
#[derive(Debug)]
pub struct FormRequest {
    pub generation: u64,
    pub state: Value,
    pub questions: &'static [NoulQuestion],
}

/// The conductor's view of the counsellor's outputs.
#[derive(Clone)]
pub struct CounselView {
    rx: watch::Receiver<Option<CounselledTurn>>,
    form_rx: watch::Receiver<Option<(u64, FormDecisions)>>,
}

impl CounselView {
    /// The latest counselled turn (None before the first).
    pub fn latest(&self) -> Option<CounselledTurn> {
        self.rx.borrow().clone()
    }

    /// Wait for the next counselled turn to land.
    pub async fn changed(&mut self) -> Result<(), tokio::sync::watch::error::RecvError> {
        self.rx.changed().await
    }

    /// The latest form opinions with their generation (None before the
    /// first Form request lands).
    pub fn form_decisions(&self) -> Option<(u64, FormDecisions)> {
        *self.form_rx.borrow()
    }

    /// Wait for the next form opinions to land.
    pub async fn form_changed(
        &mut self,
    ) -> Result<(), tokio::sync::watch::error::RecvError> {
        self.form_rx.changed().await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A scripted client serving the queued outcomes.
    struct MockClient {
        replies: std::sync::Mutex<Vec<Result<DecisionOutcome, DecisionError>>>,
        noul_replies: std::sync::Mutex<Vec<Result<Vec<Option<bool>>, DecisionError>>>,
        ranked_replies: std::sync::Mutex<Vec<Result<Vec<Option<ProbabilityMap>>, DecisionError>>>,
    }

    impl DecisionClient for MockClient {
        async fn decide(&self, _decision: &Decision) -> Result<DecisionOutcome, DecisionError> {
            self.replies.lock().unwrap().remove(0)
        }

        async fn decide_noul(
            &self,
            _state: &Value,
            _questions: &[NoulQuestion],
        ) -> Result<Vec<Option<bool>>, DecisionError> {
            self.noul_replies.lock().unwrap().remove(0)
        }

        async fn decide_ranked(
            &self,
            _state: &Value,
            _questions: &[RankedQuestion],
        ) -> Result<Vec<Option<ProbabilityMap>>, DecisionError> {
            self.ranked_replies.lock().unwrap().remove(0)
        }
    }

    fn decision() -> Decision {
        Decision {
            state: json!({ "mood": "settled" }),
            instructions: "Pick the shape.".into(),
            options: vec![
                ChoiceOption { key: "arc".into(), description: "A wide arch.".into() },
                ChoiceOption { key: "valley".into(), description: "A dip.".into() },
            ],
        }
    }

    #[test]
    fn a_clear_pick_names_its_option() {
        let reply = json!({
            "answers": { "pick": { "choice": "valley", "confidence": 0.9,
                "probabilities": { "arc": 0.25, "valley": 0.75 } } }
        });
        let outcome = SystemOneClient::parse_reply(&reply, &decision()).unwrap();
        assert_eq!(outcome.choice.as_deref(), Some("valley"));
        assert!((outcome.confidence - 0.9).abs() < 1e-6);
    }

    #[test]
    fn a_weak_pick_defers() {
        // The 58/42 lean the real probe produced: under the roll plus
        // margin bar (0.6 for two options) — a coin flip defers.
        let reply = json!({
            "answers": { "pick": { "choice": "arc", "confidence": 0.16,
                "probabilities": { "arc": 0.58, "valley": 0.42 } } }
        });
        let outcome = SystemOneClient::parse_reply(&reply, &decision()).unwrap();
        assert_eq!(outcome.choice, None, "under the bar: defer");
    }

    #[test]
    fn a_twenty_way_pick_clears_its_much_lower_bar() {
        // The real tile pick's shape: 0.292 over a 0.05 roll — clear
        // signal at a confidence (0.255) the old floor would refuse.
        let options: Vec<ChoiceOption> = (0..20)
            .map(|index| ChoiceOption { key: index.to_string(), description: "a shape".into() })
            .collect();
        let decision = Decision { state: json!({}), instructions: "Pick.".into(), options };
        let mut probabilities = serde_json::Map::new();
        for index in 0..20 {
            probabilities
                .insert(index.to_string(), json!(if index == 3 { 0.292 } else { 0.0354 }));
        }
        let reply = json!({
            "answers": { "pick": { "choice": "3", "confidence": 0.255,
                "probabilities": probabilities } }
        });
        let outcome = SystemOneClient::parse_reply(&reply, &decision).unwrap();
        assert_eq!(outcome.choice.as_deref(), Some("3"));
    }

    #[test]
    fn a_reply_without_probabilities_defers() {
        let reply = json!({
            "answers": { "pick": { "choice": "valley", "confidence": 0.9 } }
        });
        let outcome = SystemOneClient::parse_reply(&reply, &decision()).unwrap();
        assert_eq!(outcome.choice, None, "no probability: fail closed");
    }

    #[test]
    fn a_probability_map_reads_the_distribution() {
        let answer = json!({ "choice": "b", "probabilities": { "a": 0.2, "b": 0.7 } });
        let map = ProbabilityMap::from_value(&answer).unwrap();
        assert_eq!(map.probability("b"), Some(0.7));
        assert_eq!(map.probability("unscored"), None);
        assert_eq!(map.favourite(), Some(("b", 0.7)));
    }

    #[test]
    fn a_mapless_or_malformed_answer_has_no_distribution() {
        // No probabilities at all…
        assert!(
            ProbabilityMap::from_value(&json!({ "choice": "b" })).is_none(),
            "mapless: fail closed"
        );
        // …and a probabilities that isn't an object.
        assert!(
            ProbabilityMap::from_value(&json!({ "probabilities": "strong" })).is_none(),
            "not an object: fail closed"
        );
        // A bad ENTRY, though, only unscores that key — the rest of
        // the distribution survives.
        let map = ProbabilityMap::from_value(&json!({ "probabilities": { "a": "strong" } }));
        assert_eq!(map.and_then(|map| map.probability("a")), None);
    }

    #[tokio::test]
    async fn a_turn_request_walks_ranked_and_lands() {
        let (tx, mut view) = spawn(MockClient {
            replies: std::sync::Mutex::new(vec![Ok(DecisionOutcome {
                choice: Some("arc+climb".into()),
                confidence: 0.9,
            })]),
            noul_replies: std::sync::Mutex::new(Vec::new()),
            // Enough empty maps for the walk's retries and the bass's
            // pick — every answer empty, the deterministic order plays.
            ranked_replies: std::sync::Mutex::new(
                (0..6).map(|_| Ok(Vec::new())).collect(),
            ),
        });
        tx.send(CounselRequest::Turn(TurnRequest {
            current: 0,
            contour: crate::curve::ContourShape::Arc,
            dynamic: crate::curve::DynamicShape::Climb,
            integrity: 1.0,
            form_summary: json!({ "state": "home" }),
            seed: 7,
        }))
        .await
        .expect("the request queues");
        view.changed().await.expect("the turn lands");
        let turn = view.latest().expect("the counselled turn is live");
        assert_eq!(turn.contour_shape, "arc");
        assert_eq!(turn.dynamic_shape, "climb");
        // The empty maps defer everything — no bass picks.
        assert!(turn.bass.is_empty(), "no answers: no picks");
        // The walk stays grammatical — the validator's own rules.
        assert!(crate::harmony::validate(&turn.plan, None).is_ok());
    }

    /// A client whose bass questions (b*) come back anchored and whose
    /// walk questions (t*) come back empty.
    struct AnchorBassClient;

    impl DecisionClient for AnchorBassClient {
        async fn decide(&self, _decision: &Decision) -> Result<DecisionOutcome, DecisionError> {
            Ok(DecisionOutcome { choice: Some("arc+climb".into()), confidence: 0.9 })
        }

        async fn decide_noul(
            &self,
            _state: &Value,
            questions: &[NoulQuestion],
        ) -> Result<Vec<Option<bool>>, DecisionError> {
            Ok(questions.iter().map(|_| None).collect())
        }

        async fn decide_ranked(
            &self,
            _state: &Value,
            questions: &[RankedQuestion],
        ) -> Result<Vec<Option<ProbabilityMap>>, DecisionError> {
            Ok(questions
                .iter()
                .map(|question| {
                    if question.name.starts_with('b') {
                        ProbabilityMap::from_value(&json!({
                            "probabilities": { "anchor": 1.0, "hug": 0.0, "walk": 0.0 }
                        }))
                    } else {
                        None
                    }
                })
                .collect())
        }
    }

    #[tokio::test]
    async fn the_turns_bass_picks_land_by_name() {
        let (tx, mut view) = spawn(AnchorBassClient);
        tx.send(CounselRequest::Turn(TurnRequest {
            current: 0,
            contour: crate::curve::ContourShape::Arc,
            dynamic: crate::curve::DynamicShape::Climb,
            integrity: 1.0,
            form_summary: json!({ "state": "home" }),
            seed: 7,
        }))
        .await
        .expect("the request queues");
        view.changed().await.expect("the turn lands");
        let turn = view.latest().expect("the counselled turn is live");
        // A pick per slot, the anchor's favourite taken by name.
        assert_eq!(turn.bass.len(), turn.plan.slots.len());
        assert!(
            turn.bass.iter().all(|pick| *pick == Some(BassStrategy::Anchor)),
            "the anchor picked: {:?}",
            turn.bass
        );
        assert!(crate::harmony::validate(&turn.plan, None).is_ok());
    }

    #[test]
    fn a_clear_noul_opinion_is_taken() {
        let reply = json!({ "answers": { "change": { "noul": 0.72 } } });
        assert_eq!(SystemOneClient::parse_noul(&reply, "change"), Some(true));
        let reply = json!({ "answers": { "change": { "noul": 0.28 } } });
        assert_eq!(SystemOneClient::parse_noul(&reply, "change"), Some(false));
    }

    #[test]
    fn a_fenced_noul_opinion_defers() {
        // The real probe's opinions: 0.496 and 0.525 — inside the
        // coin-plus-margin band, the loaded roll plays.
        for probability in [0.496_f64, 0.525, 0.55, 0.45] {
            let reply = json!({ "answers": { "change": { "noul": probability } } });
            assert_eq!(
                SystemOneClient::parse_noul(&reply, "change"),
                None,
                "inside the band: defer ({probability})"
            );
        }
    }

    #[test]
    fn a_shapeless_noul_reply_defers() {
        assert_eq!(SystemOneClient::parse_noul(&json!({}), "change"), None);
        assert_eq!(
            SystemOneClient::parse_noul(&json!({ "answers": {} }), "change"),
            None
        );
        assert_eq!(
            SystemOneClient::parse_noul(
                &json!({ "answers": { "change": { "noul": "maybe" } } }),
                "change"
            ),
            None
        );
    }

    #[tokio::test]
    async fn a_form_request_lands_its_opinions_with_their_generation() {
        let (tx, mut view) = spawn(MockClient {
            replies: std::sync::Mutex::new(Vec::new()),
            noul_replies: std::sync::Mutex::new(vec![Ok(vec![
                Some(true),
                None,
                Some(false),
            ])]),
            ranked_replies: std::sync::Mutex::new(Vec::new()),
        });
        tx.send(CounselRequest::Form(FormRequest {
            generation: 5,
            state: json!({ "form": "home" }),
            questions: &[CHANGE_QUESTION, SEQUENCE_QUESTION, RETURN_QUESTION],
        }))
        .await
        .expect("the request queues");
        view.form_changed().await.expect("the opinions land");
        let (generation, decisions) = view.form_decisions().expect("the opinions land");
        assert_eq!(generation, 5);
        assert_eq!(decisions.change, Some(true));
        assert_eq!(decisions.sequence, None);
        assert_eq!(decisions.return_home, Some(false));
    }

    #[tokio::test]
    async fn a_form_request_maps_a_subset_by_name() {
        // Only the return question asked, first in the list — the
        // mapping lands on return_home, not the first field.
        let (tx, mut view) = spawn(MockClient {
            replies: std::sync::Mutex::new(Vec::new()),
            noul_replies: std::sync::Mutex::new(vec![Ok(vec![Some(true)])]),
            ranked_replies: std::sync::Mutex::new(Vec::new()),
        });
        tx.send(CounselRequest::Form(FormRequest {
            generation: 2,
            state: json!({ "form": "episode" }),
            questions: &[RETURN_QUESTION],
        }))
        .await
        .expect("the request queues");
        view.form_changed().await.expect("the opinions land");
        let (generation, decisions) = view.form_decisions().expect("the opinions land");
        assert_eq!(generation, 2);
        assert_eq!(decisions.change, None);
        assert_eq!(decisions.sequence, None);
        assert_eq!(decisions.return_home, Some(true));
    }

    #[test]
    fn an_off_the_options_reply_is_an_error() {
        let reply = json!({
            "answers": { "pick": { "choice": "sonata", "confidence": 0.99 } }
        });
        assert!(matches!(
            SystemOneClient::parse_reply(&reply, &decision()),
            Err(DecisionError::Invalid(_))
        ));
    }

    #[test]
    fn a_shapeless_reply_is_an_error() {
        assert!(matches!(
            SystemOneClient::parse_reply(&json!({}), &decision()),
            Err(DecisionError::Invalid(_))
        ));
        let prose = json!({ "answers": { "pick": { "choice": null, "confidence": 0.5 } } });
        assert!(SystemOneClient::parse_reply(&prose, &decision()).is_ok());
    }

}
