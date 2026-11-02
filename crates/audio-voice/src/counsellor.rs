//! The counsellor: a fast decision tier over ollama's decision models —
//! Tev1 via the `/v1/systemone` endpoint (the Jev API). Where the
//! composer plans phrases, the counsellor makes picks: tie-breaks,
//! shape rolls, walk steps. Every decision fails closed — a transport
//! error, a timeout, or an answer that doesn't beat the roll it
//! replaces defers to the deterministic roll, and the score never
//! stalls on the model.

use std::time::Duration;

use rand::rngs::SmallRng;
use rand::SeedableRng;
use serde_json::{json, Value};
use tokio::sync::watch;

use crate::harmony::HarmonyPlan;

/// Seconds before a decision gives up; the fallback roll plays on.
pub const DECISION_TIMEOUT_SECS: u64 = 1;

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
        let reply: Value = response.json().await.map_err(DecisionError::Http)?;
        Self::parse_reply(&reply, decision)
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

/// A counselled turn: the walked plan plus the shape names the
/// counsellor picked (for the log and the monitor).
#[derive(Debug, Clone)]
pub struct CounselledTurn {
    pub plan: HarmonyPlan,
    pub contour_shape: &'static str,
    pub dynamic_shape: &'static str,
}

/// Spawn the counsellor task: consume turn requests, re-roll the tiles
/// to the picked shapes and re-walk the turn with the model's picks,
/// publishing the counselled turn on its watch channel. A failed or
/// deferred decision publishes nothing — the deterministic walk plays
/// on. The model warms at spawn (and again after any transport
/// failure) — a cold load dwarfs the decision timeout. Returns the
/// request sender and the counselled turns' view.
pub fn spawn<C: DecisionClient + Send + Sync + 'static>(
    client: C,
) -> (tokio::sync::mpsc::Sender<CounselRequest>, CounselView) {
    let (request_tx, mut request_rx) = tokio::sync::mpsc::channel::<CounselRequest>(4);
    let (turn_tx, turn_rx) = watch::channel(None);
    tokio::spawn(async move {
        client.warm_up().await;
        while let Some(request) = request_rx.recv().await {
            // The tile pick: one call, two questions — the contour
            // shape and the dynamic shape the walk should follow.
            let state = json!({
                "form": request.form_summary,
                "world_integrity": request.integrity,
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
                        picked.unwrap_or((request.contour, request.dynamic));
                    let dynamic_curve = dynamic.curve();
                    let mut rng = SmallRng::seed_from_u64(request.seed);
                    let walked = crate::network::counselled_walk(
                        &client,
                        request.current,
                        &dynamic_curve,
                        request.integrity,
                        &mut rng,
                    );
                    if let Some(plan) = walked.await {
                        let _ = turn_tx.send(Some(CounselledTurn {
                            plan,
                            contour_shape: contour.name(),
                            dynamic_shape: dynamic.name(),
                        }));
                    }
                }
                Err(err) => {
                    tracing::warn!("the counsellor's decision failed ({err}); the deterministic roll plays on");
                    // A cold model times out every decision — the load
                    // re-fires so the next turn gets decisions again.
                    client.warm_up().await;
                }
            }
        }
    });
    (request_tx, CounselView { rx: turn_rx })
}

/// A turn request: everything the counselled walk needs.
#[derive(Debug)]
pub struct CounselRequest {
    pub current: usize,
    pub contour: crate::curve::ContourShape,
    pub dynamic: crate::curve::DynamicShape,
    pub integrity: f32,
    pub form_summary: Value,
    /// A seed for the walk's remaining shuffles — deterministic per turn.
    pub seed: u64,
}

/// The conductor's view of the counselled turns.
#[derive(Clone)]
pub struct CounselView {
    rx: watch::Receiver<Option<CounselledTurn>>,
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
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A scripted client serving the queued outcomes.
    struct MockClient {
        replies: std::sync::Mutex<Vec<Result<DecisionOutcome, DecisionError>>>,
    }

    impl DecisionClient for MockClient {
        async fn decide(&self, _decision: &Decision) -> Result<DecisionOutcome, DecisionError> {
            self.replies.lock().unwrap().remove(0)
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
        let client = SystemOneClient::new("http://127.0.0.1:1".into(), "tev1:4b".into());
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
