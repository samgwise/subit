//! The monitor channel: the conductor publishes score snapshots, the
//! optional window (the `ui` feature) reads the latest one each frame.
//! Publishing overwrites the watch slot, so an absent window costs
//! nothing and the async loop never blocks.

use std::sync::Arc;

use tokio::sync::watch;

use crate::snapshot::UiSnapshot;

#[cfg(feature = "ui")]
pub mod app;
#[cfg(feature = "ui")]
pub mod roll;

/// The conductor's publisher.
#[derive(Clone)]
pub struct Monitor {
    tx: watch::Sender<Option<Arc<UiSnapshot>>>,
}

/// The window's view of the published snapshots.
#[derive(Clone)]
pub struct MonitorView {
    rx: watch::Receiver<Option<Arc<UiSnapshot>>>,
}

/// Wire the conductor and the window together.
pub fn channel() -> (Monitor, MonitorView) {
    let (tx, rx) = watch::channel(None);
    (Monitor { tx }, MonitorView { rx })
}

impl Monitor {
    /// Publish a fresh snapshot — the newest wins, nothing queues.
    pub fn publish(&self, snapshot: UiSnapshot) {
        self.tx.send_replace(Some(Arc::new(snapshot)));
    }
}

impl MonitorView {
    /// The latest published snapshot (None before the first publish).
    /// A synchronous borrow, so the UI thread needs no runtime.
    pub fn latest(&self) -> Option<Arc<UiSnapshot>> {
        self.rx.borrow().clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::form::FormSummary;
    use crate::midi_log::MidiLog;
    use crate::music::MusicState;
    use crate::snapshot::{ComposerInputs, SnapshotInputs};

    fn snapshot_with_locks(locks: u32) -> UiSnapshot {
        let state = MusicState { aggro_locks: locks, ..MusicState::default() };
        let mut midi_log = MidiLog::default();
        UiSnapshot::capture(SnapshotInputs {
            hub_time: 0.0,
            slot_start: 0.0,
            slot_end: 2.0,
            state: &state,
            form: &FormSummary::default(),
            intent: None,
            history: &[],
            events: &[],
            midi_log: &mut midi_log,
            composer: ComposerInputs {
                enabled: false,
                last_replan: None,
                last_tallies: None,
            },
        })
    }

    #[test]
    fn the_latest_snapshot_wins() {
        let (monitor, view) = channel();
        assert!(view.latest().is_none());

        monitor.publish(snapshot_with_locks(2));
        assert_eq!(view.latest().unwrap().drive.aggro_locks, 2);

        monitor.publish(snapshot_with_locks(5));
        assert_eq!(view.latest().unwrap().drive.aggro_locks, 5);
    }
}
