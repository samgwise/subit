//! A stand-in for the game's Ensemble bridge: a scripted aggro ramp, a mob
//! sweep and a corruption boundary — enough drama to trip the composer's
//! replan triggers without launching the game.
//!
//! Run against a live hub with the audio-voice daemon attached and watch
//! the daemon's log for "asked the composer" and "the composer's plan is
//! live":
//!
//! ```sh
//! cargo run -p audio-voice --example aggro_driver
//! ```

use std::collections::BTreeMap;
use std::time::Duration;

use ensemble_client::Hub;
use ensemble_core::protocol::*;

/// Publish a game-shaped event: the same addresses and payload maps the
/// game's bridge uses (see `crates/game/src/bridge.rs`).
async fn publish(hub: &Hub, address: &str, signal: SignalType, fields: Vec<(&str, Value)>) {
    let mut map = BTreeMap::new();
    for (key, value) in fields {
        map.insert(key.into(), value);
    }
    let msg = action(address, signal, 0.0, Value::Map(map));
    if let Err(err) = hub.send_action(msg).await {
        tracing::error!(address, "failed to publish: {err:?}");
    }
}

fn text(value: &str) -> Value {
    Value::String(value.into())
}

fn number(value: i64) -> Value {
    Value::Integer(value)
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt::init();

    let hub = loop {
        match Hub::connect_with_discovery("subit-fake-game").await {
            Ok(hub) => break hub,
            Err(err) => {
                tracing::warn!("Ensemble hub unreachable ({err:?}); retrying in 2s");
                tokio::time::sleep(Duration::from_secs(2)).await;
            }
        }
    };
    tracing::info!(voice_id = hub.voice_id, "connected to Ensemble hub");

    // A fight kicks off: the aggro locks climb one at a time — the
    // conductor's ramps, and aggro-jump fodder for the composer.
    for locks in 1..=6 {
        publish(
            &hub,
            "/subit/game/telemetry/aggro",
            SignalType::Stream,
            vec![("type", text("aggro_locks")), ("locks", number(locks))],
        )
        .await;
        tracing::info!(locks, "published aggro");
        tokio::time::sleep(Duration::from_millis(700)).await;
    }

    // A cleave swing lands kills; the combo chains.
    publish(
        &hub,
        "/subit/game/event/combat",
        SignalType::Event,
        vec![
            ("type", text("mob_sweep")),
            ("kill_count", number(3)),
            ("combo", number(2)),
        ],
    )
    .await;
    tracing::info!("published a mob sweep");

    // The player wades into corrupted data — a sharp-shift trigger.
    publish(
        &hub,
        "/subit/game/event/action",
        SignalType::Event,
        vec![("type", text("corruption_enter"))],
    )
    .await;
    tracing::info!("the corruption boundary moved");

    tokio::time::sleep(Duration::from_secs(5)).await;
    publish(
        &hub,
        "/subit/game/event/action",
        SignalType::Event,
        vec![("type", text("corruption_exit"))],
    )
    .await;
    tracing::info!("the corruption boundary restored");
    // The hub drops a publisher's in-flight actions when the socket closes
    // — grace the final publish before exiting.
    tokio::time::sleep(Duration::from_secs(2)).await;
    tracing::info!("the scripted drama is done — the score plays on");
}
