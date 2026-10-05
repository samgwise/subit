//! A composer probe: asks the real ollama client for a plan with a
//! live-shaped context, printing the timing and — on failure — the full
//! error source chain the daemon's log only summarises:
//!
//! ```sh
//! cargo run -p audio-voice --example ollama_probe
//! ```

use std::time::Instant;

use audio_voice::composer::{
    build_context, EventTallies, LlmClient, OllamaClient, ScoreContext, PLAN_SCHEMA, SYSTEM_PROMPT,
};

/// The exact request body the client sends, dumped for replay experiments
/// (the probe prints it here so shell-based replays can use the same wire
/// shape).
fn dump_body(context: &serde_json::Value) {
    let schema: serde_json::Value =
        serde_json::from_str(PLAN_SCHEMA).expect("the plan schema is valid JSON");
    let body = serde_json::json!({
        "model": std::env::var("OLLAMA_MODEL").unwrap_or_default(),
        "stream": false,
        "format": schema,
        "keep_alive": "10m",
        "options": { "temperature": 0.8 },
        "messages": [
            { "role": "system", "content": SYSTEM_PROMPT },
            { "role": "user", "content": context.to_string() },
        ],
    });
    std::fs::write(
        "ollama_probe.body.json",
        serde_json::to_string_pretty(&body).expect("the body serialises"),
    )
    .expect("the body file writes");
}

fn cause_chain(err: &dyn std::error::Error) -> String {
    let mut chain = vec![err.to_string()];
    let mut source = err.source();
    while let Some(cause) = source {
        chain.push(cause.to_string());
        source = cause.source();
    }
    chain.join(" ← ")
}

#[tokio::main]
async fn main() {
    let client = OllamaClient::from_env();
    let context = build_context(&ScoreContext {
        tallies: EventTallies {
            aggro_average: 2.0,
            aggro_peak: 6,
            ..EventTallies::default()
        },
        ..ScoreContext::default()
    });
    dump_body(&context);
    let start = Instant::now();
    match client.plan(&context).await {
        Ok(text) => println!("{:.1}s — the plan landed:\n{text}", start.elapsed().as_secs_f64()),
        Err(err) => println!(
            "{:.1}s — the request failed: {}",
            start.elapsed().as_secs_f64(),
            cause_chain(&err)
        ),
    }
}
