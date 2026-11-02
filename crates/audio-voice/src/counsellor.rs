//! The counsellor: a fast decision tier over ollama's decision models —
//! Tev1 via the `/v1/systemone` endpoint (the Jev API). Where the
//! composer plans phrases, the counsellor makes picks: tie-breaks,
//! shape rolls, walk steps. Every decision fails closed — a transport
//! error, a timeout, or a low-confidence answer defers to the
//! deterministic roll, and the score never stalls on the model.

use std::time::Duration;

use rand::rngs::SmallRng;
use rand::SeedableRng;
use serde_json::{json, Value};
use tokio::sync::watch;

use crate::harmony::HarmonyPlan;

/// Seconds before a decision gives up; the fallback roll plays on.
pub const DECISION_TIMEOUT_SECS: u64 = 1;

/// The confidence under which a pick defers to the deterministic roll.
pub const CONFIDENCE_FLOOR: f32 = 0.5;

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
/// (low confidence) to the deterministic roll.
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
}

impl SystemOneClient {
    /// Map the reply's answer back to an option key — deferring (a None
    /// choice) when the confidence runs under the floor or the shape
    /// surprises us.
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
        let choice = if confidence >= CONFIDENCE_FLOOR { choice } else { None };
        Ok(DecisionOutcome { choice, confidence })
    }
}

/// Whether the counsellor participates: `TEV1_DISABLED` unset or not "1"
/// (the mirror of OLLAMA_DISABLED).
pub fn enabled_from_env() -> bool {
    std::env::var("TEV1_DISABLED").is_ok_and(|disabled| disabled != "1")
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
/// on. Returns the request sender and the counselled turns' view.
pub fn spawn<C: DecisionClient + Send + Sync + 'static>(
    client: C,
) -> (tokio::sync::mpsc::Sender<CounselRequest>, CounselView) {
    let (request_tx, mut request_rx) = tokio::sync::mpsc::channel::<CounselRequest>(4);
    let (turn_tx, turn_rx) = watch::channel(None);
    tokio::spawn(async move {
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
                    tracing::warn!("the counsellor's decision failed ({err}); the deterministic roll plays on")
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
    fn a_confident_pick_names_its_option() {
        let client = SystemOneClient::new("http://127.0.0.1:1".into(), "tev1:4b".into());
        let reply = json!({
            "answers": { "pick": { "choice": "valley", "confidence": 0.9 } }
        });
        let outcome = SystemOneClient::parse_reply(&reply, &decision()).unwrap();
        assert_eq!(outcome.choice.as_deref(), Some("valley"));
        assert!((outcome.confidence - 0.9).abs() < 1e-6);
    }

    #[test]
    fn a_low_confidence_pick_defers() {
        let reply = json!({
            "answers": { "pick": { "choice": "valley", "confidence": 0.2 } }
        });
        let outcome = SystemOneClient::parse_reply(&reply, &decision()).unwrap();
        assert_eq!(outcome.choice, None, "under the floor: defer");
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
