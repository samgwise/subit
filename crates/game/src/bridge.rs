//! Ensemble bridge: publishes game events to the Ensemble hub from a
//! background thread so the Bevy render loop never blocks on I/O.

use std::collections::BTreeMap;
use std::time::Duration;

use bevy::prelude::*;
use ensemble_client::Hub;
use ensemble_core::protocol::*;
use tokio::sync::mpsc;

/// Game events the bridge knows how to publish (GDD section 5).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GameAudioEvent {
    /// Placeholder event fired on Space to verify the full chain.
    TestPulse,
}

/// Sender half of the bridge channel.
#[derive(Resource)]
pub struct BridgeTx(mpsc::Sender<GameAudioEvent>);

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
        let mut fields = BTreeMap::new();
        match event {
            GameAudioEvent::TestPulse => {
                fields.insert("type".into(), Value::String("test_pulse".into()));
            }
        }
        let msg = action(
            "/subit/game/event",
            SignalType::Event,
            0.0,
            Value::Map(fields),
        );
        if let Err(err) = hub.send_action(msg).await {
            tracing::error!("failed to publish game event: {err:?}");
        }
    }
}

/// Fire a test event on Space so the audio chain can be verified.
pub fn send_test_pulse(input: Res<ButtonInput<KeyCode>>, bridge: Res<BridgeTx>) {
    if input.just_pressed(KeyCode::Space)
        && let Err(err) = bridge.0.try_send(GameAudioEvent::TestPulse)
    {
        tracing::warn!("dropped TestPulse: {err:?}");
    }
}
