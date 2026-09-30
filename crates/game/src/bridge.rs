//! Ensemble bridge: publishes game events to the Ensemble hub from a
//! background thread so the Bevy render loop never blocks on I/O.
//!
//! Addressing and signal semantics per the GDD event table: discrete combat
//! moments are `Event` actions, per-frame player telemetry is high-rate
//! `Stream` data (dropped rather than queued under congestion), and the
//! world-integrity value is a `Param` — stateful, replayed to late joiners.

use std::collections::BTreeMap;
use std::time::Duration;

use bevy::prelude::*;
use ensemble_client::Hub;
use ensemble_core::protocol::*;
use tokio::sync::mpsc;

/// Game events the bridge knows how to publish (GDD section 5).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum GameAudioEvent {
    /// Primary attack trigger.
    AttackPrimary,
    /// Batched result of one cleave swing.
    MobSweep { kill_count: u32, combo: u32 },
    /// A thrower enemy lobbed a projectile.
    ProjectileThrow,
    /// The shield reflected a projectile.
    ShieldReflect,
    /// The player levelled up.
    LevelUp,
    /// The player dashed.
    Dash,
    /// A grenade detonated.
    GrenadeBlast,
    /// The player fired a nova burst.
    Nova,
    /// The player engaged the cloak.
    Cloak,
    /// The player crossed a corrupted zone boundary.
    Corruption { entered: bool },
    /// The player descended to the next depth.
    Descent,
    /// Streaming player speed for audio modulation.
    PlayerTelemetry { speed: f32, max_speed: f32 },
    /// World-integrity ratio of the generated map (walkable fraction).
    WorldTelemetry { integrity: f32 },
}

impl GameAudioEvent {
    /// The hub address the event is published on.
    fn address(&self) -> &'static str {
        match self {
            GameAudioEvent::AttackPrimary
            | GameAudioEvent::ProjectileThrow
            | GameAudioEvent::ShieldReflect
            | GameAudioEvent::LevelUp
            | GameAudioEvent::Dash
            | GameAudioEvent::GrenadeBlast
            | GameAudioEvent::Nova
            | GameAudioEvent::Cloak
            | GameAudioEvent::Corruption { .. }
            | GameAudioEvent::Descent => "/subit/game/event/action",
            GameAudioEvent::MobSweep { .. } => "/subit/game/event/combat",
            GameAudioEvent::PlayerTelemetry { .. } => "/subit/game/telemetry/player",
            GameAudioEvent::WorldTelemetry { .. } => "/subit/game/telemetry/world",
        }
    }

    /// The hub signal semantics for the event's address.
    fn signal_type(&self) -> SignalType {
        match self {
            GameAudioEvent::AttackPrimary
            | GameAudioEvent::ProjectileThrow
            | GameAudioEvent::ShieldReflect
            | GameAudioEvent::LevelUp
            | GameAudioEvent::Dash
            | GameAudioEvent::GrenadeBlast
            | GameAudioEvent::Nova
            | GameAudioEvent::Cloak
            | GameAudioEvent::Corruption { .. }
            | GameAudioEvent::Descent
            | GameAudioEvent::MobSweep { .. } => SignalType::Event,
            GameAudioEvent::PlayerTelemetry { .. } => SignalType::Stream,
            GameAudioEvent::WorldTelemetry { .. } => SignalType::Param,
        }
    }

    /// The event payload as a protocol value map.
    fn payload(&self) -> Value {
        let mut fields = BTreeMap::new();
        match self {
            GameAudioEvent::AttackPrimary => {
                fields.insert("type".into(), Value::String("attack_primary".into()));
            }
            GameAudioEvent::MobSweep { kill_count, combo } => {
                fields.insert("type".into(), Value::String("mob_sweep".into()));
                fields.insert("kill_count".into(), Value::Integer(*kill_count as i64));
                fields.insert("combo".into(), Value::Integer(*combo as i64));
            }
            GameAudioEvent::ProjectileThrow => {
                fields.insert("type".into(), Value::String("projectile_throw".into()));
            }
            GameAudioEvent::ShieldReflect => {
                fields.insert("type".into(), Value::String("shield_reflect".into()));
            }
            GameAudioEvent::LevelUp => {
                fields.insert("type".into(), Value::String("level_up".into()));
            }
            GameAudioEvent::Dash => {
                fields.insert("type".into(), Value::String("dash".into()));
            }
            GameAudioEvent::GrenadeBlast => {
                fields.insert("type".into(), Value::String("grenade_blast".into()));
            }
            GameAudioEvent::Nova => {
                fields.insert("type".into(), Value::String("nova".into()));
            }
            GameAudioEvent::Cloak => {
                fields.insert("type".into(), Value::String("cloak".into()));
            }
            GameAudioEvent::Corruption { entered } => {
                fields.insert(
                    "type".into(),
                    Value::String(if *entered {
                        "corruption_enter".into()
                    } else {
                        "corruption_exit".into()
                    }),
                );
            }
            GameAudioEvent::Descent => {
                fields.insert("type".into(), Value::String("descent".into()));
            }
            GameAudioEvent::PlayerTelemetry { speed, max_speed } => {
                fields.insert("type".into(), Value::String("player_speed".into()));
                fields.insert("speed".into(), Value::Float(FloatValue::new(*speed as f64)));
                fields.insert(
                    "max_speed".into(),
                    Value::Float(FloatValue::new(*max_speed as f64)),
                );
            }
            GameAudioEvent::WorldTelemetry { integrity } => {
                fields.insert("type".into(), Value::String("world_integrity".into()));
                fields.insert(
                    "integrity".into(),
                    Value::Float(FloatValue::new(*integrity as f64)),
                );
            }
        }
        Value::Map(fields)
    }
}

/// Sender half of the bridge channel.
#[derive(Resource)]
pub struct BridgeTx(mpsc::Sender<GameAudioEvent>);

impl BridgeTx {
    /// Queue an event for publication; drops it (with a warning) when the
    /// channel is full or not yet connected.
    pub fn send(&self, event: GameAudioEvent) {
        if let Err(err) = self.0.try_send(event) {
            tracing::warn!("dropped {:?}: {err:?}", event.address());
        }
    }
}

pub struct EnsembleBridgePlugin;

impl Plugin for EnsembleBridgePlugin {
    fn build(&self, app: &mut App) {
        // Bounded channel: if the hub connection stalls, events are dropped
        // rather than growing the queue without limit.
        let (tx, rx) = mpsc::channel(64);
        app.insert_resource(BridgeTx(tx));
        std::thread::Builder::new()
            .name("ensemble-bridge".into())
            .spawn(move || {
                let runtime = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .expect("failed to build tokio runtime for the Ensemble bridge");
                runtime.block_on(bridge_task(rx));
            })
            .expect("failed to spawn the Ensemble bridge thread");
    }
}

async fn bridge_task(mut rx: mpsc::Receiver<GameAudioEvent>) {
    let hub = loop {
        match Hub::connect_with_discovery("subit-game").await {
            Ok(hub) => break hub,
            Err(err) => {
                tracing::warn!("Ensemble hub unreachable ({err:?}); retrying in 2s");
                tokio::time::sleep(Duration::from_secs(2)).await;
            }
        }
    };
    tracing::info!(voice_id = hub.voice_id, "connected to Ensemble hub");

    while let Some(event) = rx.recv().await {
        let msg = action(event.address(), event.signal_type(), 0.0, event.payload());
        if let Err(err) = hub.send_action(msg).await {
            tracing::error!("failed to publish game event: {err:?}");
        }
    }
}
