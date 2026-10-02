//! Standalone audio voice daemon: subscribes to subit game events on the hub
//! and performs them as quantised `/midi/play` actions for the MIDI bridge.
//! Player-speed telemetry bypasses quantisation — it forwards immediately as
//! `/midi/cc` so the synth filter tracks motion in real time.

use std::collections::BTreeMap;
use std::time::Duration;

use audio_voice::{
    GameEvent, MOD_WHEEL_CC, NoteEvent, Quantiser, Subdivision, perform_event, speed_to_cc,
};
use ensemble_client::Hub;
use ensemble_core::protocol::*;

/// Subscription pattern covering everything the game publishes: combat events
/// under `/subit/game/event/*` and telemetry under `/subit/game/telemetry/*`.
const GAME_EVENTS_PATTERN: &str = "/subit/game/**";

/// Speed the game moves the player at; the CC mapping's full-scale value
/// (matches `world::PLAYER_SPEED` in the game crate).
const DEFAULT_MAX_SPEED: f32 = 240.0;

/// Tempo the stub voice performs at; replaced by a proper musical clock once
/// the hub tempo protocol is wired up in Milestone 5.
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
        let Value::Map(payload) = get_value(routed, "payload").unwrap_or(Value::Null) else {
            tracing::warn!(address = %address, "unrecognised event payload");
            continue;
        };

        match address.as_str() {
            "/subit/game/telemetry/player" => {
                let speed = get_float(&payload, "speed").unwrap_or(0.0) as f32;
                let max_speed = get_float(&payload, "max_speed")
                    .map(|m| m as f32)
                    .unwrap_or(DEFAULT_MAX_SPEED);
                let cc = midi_cc(0, MOD_WHEEL_CC, speed_to_cc(speed, max_speed));
                if let Err(err) = hub.send_action(cc).await {
                    tracing::error!("failed to forward speed CC: {err:?}");
                } else {
                    tracing::debug!(speed, "forwarded speed as mod wheel CC");
                }
            }
            "/subit/game/telemetry/world" => {
                tracing::info!(
                    integrity = get_float(&payload, "integrity").unwrap_or(0.0),
                    "world integrity reported"
                );
            }
            _ => {
                let Some(event) = parse_event(&payload) else {
                    tracing::warn!(address = %address, "unrecognised event payload");
                    continue;
                };

                let note = perform_event(event);
                let when = quantiser.next_boundary(hub.now().await);
                match hub.send_action(midi_play(&note, when)).await {
                    Ok(()) => tracing::info!(
                        address = %address,
                        when,
                        note = note.note as i64,
                        "performed event"
                    ),
                    Err(err) => {
                        tracing::error!(address = %address, "failed to schedule note: {err:?}")
                    }
                }
            }
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
fn parse_event(payload: &BTreeMap<String, Value>) -> Option<GameEvent> {
    match get_string(payload, "type").as_deref()? {
        "attack_primary" => Some(GameEvent::AttackPrimary),
        "mob_sweep" => Some(GameEvent::MobSweep {
            kill_count: get_integer(payload, "kill_count").unwrap_or(0).max(0) as u32,
            combo: get_integer(payload, "combo").unwrap_or(1).max(1) as u32,
        }),
        "projectile_throw" => Some(GameEvent::ProjectileThrow),
        "shield_reflect" => Some(GameEvent::ShieldReflect),
        "level_up" => Some(GameEvent::LevelUp),
        "dash" => Some(GameEvent::Dash),
        "grenade_blast" => Some(GameEvent::GrenadeBlast),
        "nova" => Some(GameEvent::Nova),
        "cloak" => Some(GameEvent::Cloak),
        "corruption_enter" => Some(GameEvent::CorruptionEnter),
        "corruption_exit" => Some(GameEvent::CorruptionExit),
        "descent" => Some(GameEvent::Descent),
        "drone_shot" => Some(GameEvent::DroneShot),
        "drone_down" => Some(GameEvent::DroneDown),
        "drone_online" => Some(GameEvent::DroneOnline),
        "vault_open" => Some(GameEvent::VaultOpen),
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

/// Build an immediate `/midi/cc` action for the MIDI bridge.
fn midi_cc(channel: u8, controller: u8, value: u8) -> WireMessage {
    action(
        "/midi/cc",
        SignalType::Event,
        0.0,
        Value::Tuple(vec![
            Value::Integer(channel as i64),
            Value::Integer(controller as i64),
            Value::Integer(value as i64),
        ]),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_mob_sweep_with_combo() {
        let mut payload = BTreeMap::new();
        payload.insert("type".into(), Value::String("mob_sweep".into()));
        payload.insert("kill_count".into(), Value::Integer(3));
        payload.insert("combo".into(), Value::Integer(5));
        let event = parse_event(&payload).expect("mob_sweep parses");
        assert_eq!(
            event,
            GameEvent::MobSweep {
                kill_count: 3,
                combo: 5
            }
        );
    }

    #[test]
    fn mob_sweep_defaults_combo_to_one() {
        let mut payload = BTreeMap::new();
        payload.insert("type".into(), Value::String("mob_sweep".into()));
        payload.insert("kill_count".into(), Value::Integer(2));
        let event = parse_event(&payload).expect("mob_sweep parses");
        assert_eq!(
            event,
            GameEvent::MobSweep {
                kill_count: 2,
                combo: 1
            }
        );
    }

    #[test]
    fn cc_actions_carry_a_three_byte_tuple() {
        let msg = midi_cc(0, MOD_WHEEL_CC, 96);
        let Value::Map(map) = &msg.payload else {
            panic!("expected map payload");
        };
        assert_eq!(get_string(map, "address"), Some("/midi/cc".into()));
        let Value::Tuple(values) = get_value(map, "payload").unwrap() else {
            panic!("expected tuple payload");
        };
        assert_eq!(
            values,
            vec![
                Value::Integer(0),
                Value::Integer(MOD_WHEEL_CC as i64),
                Value::Integer(96),
            ]
        );
    }
}
