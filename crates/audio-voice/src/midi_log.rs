//! The MIDI log: what the voice actually scheduled, as plain data — the
//! piano-roll's source. Modelled on the bridge's own key semantics: a
//! play OPENS a span, an early note-off CLOSES it, a cancel re-arms its
//! end (the bridge drops the pending stop and arms a fresh one). At most
//! one span per (channel, note) is open at a time — the conductor never
//! re-plays a held tone.

use serde::Serialize;

use ensemble_core::protocol::{get_string, get_value, Value, WireMessage};

use crate::music::{
    CHANNEL_AIRY, CHANNEL_BASS, CHANNEL_BELL, CHANNEL_DISTANT, CHANNEL_PATTERN_A,
    CHANNEL_PATTERN_B,
};
use crate::FX_CHANNEL;

/// How many note spans and CC points the log keeps at most.
const RING_LEN: usize = 256;

/// How far back (in seconds) the log keeps material.
pub const LOG_WINDOW_SECS: f64 = 30.0;

/// One scheduled note: a span on the piano-roll.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct NoteSpan {
    pub channel: u8,
    pub note: u8,
    pub velocity: u8,
    /// The hub time the note-on is scheduled for.
    pub start: f64,
    /// The hub time the note's stop is currently scheduled for — moved
    /// out by tie re-arms (cancels), pulled in by early note-offs.
    pub end: f64,
    /// Whether the key is still down (no stop seen yet).
    pub open: bool,
}

/// One controller point on a CC lane.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct CcPoint {
    pub when: f64,
    pub channel: u8,
    pub controller: u8,
    pub value: u8,
}

/// The channels' names in the score — the roll's legend and tooltips.
/// None for channels outside the score's map.
pub fn channel_name(channel: u8) -> Option<&'static str> {
    match channel {
        CHANNEL_BASS => Some("bass"),
        CHANNEL_DISTANT => Some("distant"),
        CHANNEL_BELL => Some("bell"),
        CHANNEL_AIRY => Some("airy"),
        CHANNEL_PATTERN_A => Some("pattern A"),
        CHANNEL_PATTERN_B => Some("pattern B"),
        FX_CHANNEL => Some("fx"),
        _ => None,
    }
}

/// An integer read from a tuple element.
fn as_int(value: &Value) -> Option<i64> {
    if let Value::Integer(integer) = value {
        Some(*integer)
    } else {
        None
    }
}

/// A float read from a tuple element.
fn as_float(value: &Value) -> Option<f64> {
    if let Value::Float(float) = value {
        Some(float.value())
    } else {
        None
    }
}

/// The log of scheduled MIDI: bounded rings the roll renders.
#[derive(Debug, Default, Clone)]
pub struct MidiLog {
    spans: Vec<NoteSpan>,
    ccs: Vec<CcPoint>,
}

impl MidiLog {
    /// Record a scheduled note-on: a new span, open until a stop or a
    /// re-arm says otherwise.
    pub fn record_play(&mut self, channel: u8, note: u8, velocity: u8, when: f64, duration_secs: f64) {
        self.spans.push(NoteSpan {
            channel,
            note,
            velocity,
            start: when,
            end: when + duration_secs,
            open: true,
        });
        self.cap_spans();
    }

    /// Record an early note-off: the open span for the key closes at
    /// `when` (the bridge stops the sounding note immediately).
    pub fn record_note_off(&mut self, channel: u8, note: u8, when: f64) {
        if let Some(span) = self.open_span_mut(channel, note) {
            span.end = when;
            span.open = false;
        }
    }

    /// Record a tie re-arm: the open span's stop moves out to
    /// `when + window` — the re-armed stop's time.
    pub fn record_cancel(&mut self, channel: u8, note: u8, when: f64, window_secs: f64) {
        if let Some(span) = self.open_span_mut(channel, note) {
            span.end = when + window_secs;
        }
    }

    /// Record a controller point on its lane.
    pub fn record_cc(&mut self, when: f64, channel: u8, controller: u8, value: u8) {
        self.ccs.push(CcPoint {
            when,
            channel,
            controller,
            value,
        });
        if self.ccs.len() > RING_LEN {
            self.ccs.remove(0);
        }
    }

    /// The one capture point: parse a built action and record it.
    /// Anything unrecognised — other addresses, malformed tuples — is
    /// ignored without panicking (the builders' own tests pin the
    /// shapes; this tolerates the rest).
    pub fn record_action(&mut self, msg: &WireMessage) {
        let Value::Map(map) = &msg.payload else {
            return;
        };
        let Some(address) = get_string(map, "address") else {
            return;
        };
        let Value::Tuple(values) = get_value(map, "payload").unwrap_or(Value::Null) else {
            return;
        };
        let element = |index: usize| values.get(index);
        match address.as_str() {
            "/midi/play" => {
                if let (Some(channel), Some(note), Some(velocity), Some(duration)) = (
                    element(0).and_then(as_int),
                    element(1).and_then(as_int),
                    element(2).and_then(as_int),
                    element(3).and_then(as_float),
                ) {
                    self.record_play(
                        channel as u8,
                        note as u8,
                        velocity as u8,
                        msg_timestamp_or_zero(msg),
                        duration,
                    );
                }
            }
            "/midi/note-off" => {
                if let (Some(channel), Some(note)) =
                    (element(0).and_then(as_int), element(1).and_then(as_int))
                {
                    self.record_note_off(channel as u8, note as u8, msg_timestamp_or_zero(msg));
                }
            }
            "/midi/cancel" => {
                if let (Some(channel), Some(note), Some(window)) = (
                    element(0).and_then(as_int),
                    element(1).and_then(as_int),
                    element(2).and_then(as_float),
                ) {
                    self.record_cancel(
                        channel as u8,
                        note as u8,
                        msg_timestamp_or_zero(msg),
                        window,
                    );
                }
            }
            "/midi/cc" => {
                if let (Some(channel), Some(controller), Some(value)) = (
                    element(0).and_then(as_int),
                    element(1).and_then(as_int),
                    element(2).and_then(as_int),
                ) {
                    self.record_cc(
                        msg_timestamp_or_zero(msg),
                        channel as u8,
                        controller as u8,
                        value as u8,
                    );
                }
            }
            _ => {}
        }
    }

    /// Drop stale material: spans that ended before the window's horizon
    /// and CC points older than it — and hard caps in case a burst
    /// outruns the window's arithmetic.
    pub fn trim(&mut self, now: f64) {
        let horizon = now - LOG_WINDOW_SECS;
        self.spans.retain(|span| span.open || span.end > horizon);
        if self.spans.len() > RING_LEN {
            self.spans.drain(..self.spans.len() - RING_LEN);
        }
        self.ccs.retain(|point| point.when > horizon);
        if self.ccs.len() > RING_LEN {
            self.ccs.drain(..self.ccs.len() - RING_LEN);
        }
    }

    /// The note spans to render.
    pub fn spans(&self) -> &[NoteSpan] {
        &self.spans
    }

    /// The controller points to render.
    pub fn ccs(&self) -> &[CcPoint] {
        &self.ccs
    }

    /// The newest open span for a key — the one a stop or re-arm means.
    fn open_span_mut(&mut self, channel: u8, note: u8) -> Option<&mut NoteSpan> {
        self.spans
            .iter_mut()
            .rev()
            .find(|span| span.open && span.channel == channel && span.note == note)
    }

    fn cap_spans(&mut self) {
        if self.spans.len() > RING_LEN {
            self.spans.remove(0);
        }
    }
}

/// The action's hub timestamp — the protocol's action builder stamps
/// every message with the schedule time, which is exactly the span's
/// clock. A zero stands in when the shape surprises us (harmless for
/// the roll).
fn msg_timestamp_or_zero(msg: &WireMessage) -> f64 {
    if let Value::Map(map) = &msg.payload {
        get_value(map, "timestamp")
            .and_then(|value| as_float(&value))
            .unwrap_or(0.0)
    } else {
        0.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ensemble_core::protocol::{action, FloatValue, SignalType};

    /// Build a wire message the way the daemon's builders do.
    fn wire(address: &str, when: f64, values: Value) -> WireMessage {
        action(address, SignalType::Event, when, values)
    }

    fn tuple(values: Vec<Value>) -> Value {
        Value::Tuple(values)
    }

    #[test]
    fn a_play_opens_a_span_ending_at_its_duration() {
        let mut log = MidiLog::default();
        log.record_action(&wire(
            "/midi/play",
            4.0,
            tuple(vec![
                Value::Integer(0),
                Value::Integer(38),
                Value::Integer(88),
                Value::Float(FloatValue::new(1.5)),
            ]),
        ));
        let spans = log.spans();
        assert_eq!(spans.len(), 1);
        assert!(spans[0].open);
        assert_eq!((spans[0].channel, spans[0].note, spans[0].velocity), (0, 38, 88));
        assert!((spans[0].start - 4.0).abs() < 1e-9);
        assert!((spans[0].end - 5.5).abs() < 1e-9);
    }

    #[test]
    fn an_early_note_off_closes_the_open_span() {
        let mut log = MidiLog::default();
        log.record_action(&wire(
            "/midi/play",
            4.0,
            tuple(vec![
                Value::Integer(1),
                Value::Integer(62),
                Value::Integer(60),
                Value::Float(FloatValue::new(6.0)),
            ]),
        ));
        log.record_action(&wire(
            "/midi/note-off",
            5.0,
            tuple(vec![Value::Integer(1), Value::Integer(62)]),
        ));
        let spans = log.spans();
        assert_eq!(spans.len(), 1);
        assert!(!spans[0].open, "the tie is broken");
        assert!((spans[0].end - 5.0).abs() < 1e-9, "closed at the off");
    }

    #[test]
    fn a_cancel_rearms_the_ties_end() {
        let mut log = MidiLog::default();
        log.record_action(&wire(
            "/midi/play",
            4.0,
            tuple(vec![
                Value::Integer(3),
                Value::Integer(69),
                Value::Integer(50),
                Value::Float(FloatValue::new(6.0)),
            ]),
        ));
        log.record_action(&wire(
            "/midi/cancel",
            5.0,
            tuple(vec![
                Value::Integer(3),
                Value::Integer(69),
                Value::Float(FloatValue::new(3.0)),
            ]),
        ));
        let spans = log.spans();
        assert_eq!(spans.len(), 1);
        assert!(spans[0].open, "the tie lives on");
        assert!((spans[0].end - 8.0).abs() < 1e-9, "re-armed to when + window");
    }

    #[test]
    fn a_replay_records_a_second_span() {
        let mut log = MidiLog::default();
        for _ in 0..2 {
            log.record_action(&wire(
                "/midi/play",
                0.0,
                tuple(vec![
                    Value::Integer(0),
                    Value::Integer(38),
                    Value::Integer(88),
                    Value::Float(FloatValue::new(0.3)),
                ]),
            ));
        }
        assert_eq!(log.spans().len(), 2);
    }

    #[test]
    fn the_rings_trim_to_the_window_and_the_cap() {
        let mut log = MidiLog::default();
        // Closed spans a second apart; the trim at t = 40 keeps the last
        // window's worth.
        for k in 0..40 {
            log.record_play(0, 38, 88, k as f64, 600.0);
            log.record_note_off(0, 38, k as f64 + 0.5);
        }
        log.trim(40.0);
        assert!(
            log.spans().iter().all(|span| span.end > 40.0 - LOG_WINDOW_SECS),
            "stale spans fall out of the window"
        );

        // Open ties survive the window — a held key stays visible — but
        // a burst past the cap still loses its oldest.
        for k in 0..(RING_LEN + 32) {
            log.record_play(1, 45, 88, k as f64, 600.0);
        }
        log.trim((RING_LEN + 32) as f64);
        assert_eq!(log.spans().len(), RING_LEN);
    }

    #[test]
    fn unknown_addresses_and_malformed_payloads_are_ignored() {
        let mut log = MidiLog::default();
        // Not one of the four addresses.
        log.record_action(&wire(
            "/midi/in/note-on",
            0.0,
            tuple(vec![Value::Integer(0), Value::Integer(60)]),
        ));
        // A missing payload, a non-tuple payload, a wrong-typed field.
        log.record_action(&wire("/midi/play", 0.0, Value::Null));
        log.record_action(&wire("/midi/play", 0.0, Value::Integer(3)));
        log.record_action(&wire(
            "/midi/play",
            0.0,
            tuple(vec![Value::String("bogus".into())]),
        ));
        // A non-map message entirely.
        let mut stray = wire("/midi/play", 0.0, Value::Null);
        stray.payload = Value::Null;
        log.record_action(&stray);
        assert!(log.spans().is_empty());
        assert!(log.ccs().is_empty());
    }

    #[test]
    fn cc_points_land_on_their_lane() {
        let mut log = MidiLog::default();
        log.record_action(&wire(
            "/midi/cc",
            2.0,
            tuple(vec![Value::Integer(0), Value::Integer(1), Value::Integer(96)]),
        ));
        let points = log.ccs();
        assert_eq!(points.len(), 1);
        assert_eq!((points[0].channel, points[0].controller, points[0].value), (0, 1, 96));
    }

    #[test]
    fn the_channels_are_named_for_the_legend() {
        assert_eq!(channel_name(CHANNEL_BASS), Some("bass"));
        assert_eq!(channel_name(CHANNEL_DISTANT), Some("distant"));
        assert_eq!(channel_name(CHANNEL_BELL), Some("bell"));
        assert_eq!(channel_name(CHANNEL_AIRY), Some("airy"));
        assert_eq!(channel_name(CHANNEL_PATTERN_A), Some("pattern A"));
        assert_eq!(channel_name(CHANNEL_PATTERN_B), Some("pattern B"));
        assert_eq!(channel_name(FX_CHANNEL), Some("fx"));
        assert_eq!(channel_name(15), None);
    }
}
