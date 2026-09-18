//! Standalone audio voice daemon: subscribes to subit game events on the hub
//! and performs them as quantised `/midi/play` actions for the MIDI bridge.

use std::collections::BTreeMap;
use std::time::Duration;

use audio_voice::{GameEvent, NoteEvent, Quantiser, Subdivision};
use ensemble_client::Hub;
use ensemble_core::protocol::*;

/// Subscription pattern covering everything the game publishes.
const GAME_EVENTS_PATTERN: &str = "/subit/game/*";

/// Tempo the stub voice performs at; replaced by a proper musical clock once
/// the hub tempo protocol is wired up in Milestone 4.
const STUB_BPM: f64 = 120.0;

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt::init();

    let mut hub = connect_with_retry().await;
    tracing::info!(voice_id = hub.voice_id, "connected to Ensemble hub");

    hub.subscribe(GAME_EVENTS_PATTERN)
        .await
        .expect("hub rejected our event subscription");

    let quantiser = Quantiser::new(STUB_BPM, Subdivision::Sixteenth);

    loop {
        // Hub-reported errors (e.g. rejected subscriptions) are non-fatal by
        // protocol design; log and carry on.
        while let Some(err) = hub.try_recv_error() {
            tracing::warn!(code = %err.code, "hub error: {}", err.message);
        }

        let Some(msg) = hub.recv_action().await else {
            tracing::error!("hub connection closed");
            break;
        };

        let Value::Map(routed) = &msg.payload else {
            continue;
        };
        let Some(address) = get_string(routed, "address") else {
            continue;
        };
        let Some(event) = parse_event(routed) else {
            tracing::warn!(address = %address, "unrecognised event payload");
            continue;
        };

        let note = audio_voice::perform_event(event);
        let when = quantiser.next_boundary(hub.now().await);
        match hub.send_action(midi_play(&note, when)).await {
            Ok(()) => tracing::info!(
                address = %address,
                when,
                note = note.note as i64,
                "performed event"
            ),
            Err(err) => tracing::error!(address = %address, "failed to schedule note: {err:?}"),
        }
    }
}

async fn connect_with_retry() -> Hub {
    loop {
        match Hub::connect_with_discovery("subit-audio-voice").await {
            Ok(hub) => return hub,
            Err(err) => {
                tracing::warn!("Ensemble hub unreachable ({err:?}); retrying in 2s");
                tokio::time::sleep(Duration::from_secs(2)).await;
            }
        }
    }
}

/// Recover the game event from a routed action's inner payload.
fn parse_event(routed: &BTreeMap<String, Value>) -> Option<GameEvent> {
    let Value::Map(payload) = routed.get("payload")? else {
        return None;
    };
    match get_string(payload, "type").as_deref()? {
        "attack_primary" => Some(GameEvent::AttackPrimary),
        "mob_sweep" => Some(GameEvent::MobSweep {
            kill_count: get_integer(payload, "kill_count").unwrap_or(0).max(0) as u32,
        }),
        "test_pulse" => Some(GameEvent::TestPulse),
        _ => None,
    }
}

/// Build a scheduled `/midi/play` action for the MIDI bridge.
fn midi_play(note: &NoteEvent, when: f64) -> WireMessage {
    action(
        "/midi/play",
        SignalType::Event,
        when,
        Value::Tuple(vec![
            Value::Integer(note.channel as i64),
            Value::Integer(note.note as i64),
            Value::Integer(note.velocity as i64),
            Value::Float(FloatValue::new(note.duration_secs)),
        ]),
    )
}
