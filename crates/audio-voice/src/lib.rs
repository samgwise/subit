//! Generative audio voice for subit.
//!
//! The musical logic here is deliberately hub-free and async-free so it can
//! be unit tested in isolation and, if the prototype matures, woven directly
//! into the game process rather than run as a separate daemon. The thin
//! binary target (`src/main.rs`) provides the standalone daemon role from the
//! GDD.

/// Musical subdivisions the voice quantises incoming events to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Subdivision {
    Quarter,
    Eighth,
    Sixteenth,
}

impl Subdivision {
    /// Number of subdivisions per beat.
    fn per_beat(self) -> f64 {
        match self {
            Subdivision::Quarter => 1.0,
            Subdivision::Eighth => 2.0,
            Subdivision::Sixteenth => 4.0,
        }
    }
}

/// Quantises hub-clock timestamps onto subdivision boundaries at a fixed
/// tempo. The GDD quantises player actions to the nearest 1/16th note;
/// scheduling on the *next* boundary avoids ever scheduling a note in the
/// past.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Quantiser {
    /// Seconds per subdivision.
    step_secs: f64,
}

impl Quantiser {
    /// Build a quantiser for `bpm` beats per minute and the given subdivision.
    ///
    /// Panics if `bpm` is not positive.
    pub fn new(bpm: f64, subdivision: Subdivision) -> Self {
        assert!(bpm > 0.0, "bpm must be positive");
        Self {
            step_secs: 60.0 / bpm / subdivision.per_beat(),
        }
    }

    /// The first subdivision boundary strictly after `now` on the hub clock.
    pub fn next_boundary(&self, now: f64) -> f64 {
        ((now / self.step_secs).floor() + 1.0) * self.step_secs
    }
}

/// Game events the audio voice responds to (GDD section 5).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GameEvent {
    /// Primary attack trigger — quantised synth stab.
    AttackPrimary,
    /// Enemy kill / AoE mob sweep — melodic run based on hit count.
    MobSweep { kill_count: u32 },
    /// Placeholder event used to verify the plumbing end to end.
    TestPulse,
}

/// A MIDI-ready note event.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct NoteEvent {
    pub channel: u8,
    pub note: u8,
    pub velocity: u8,
    /// Note length in seconds.
    pub duration_secs: f64,
}

/// Stub performance mapping: choose a note for a game event. Milestone 4
/// replaces this with scale-aware harmonic selection.
pub fn perform_event(event: GameEvent) -> NoteEvent {
    match event {
        GameEvent::AttackPrimary | GameEvent::TestPulse => NoteEvent {
            channel: 0,
            note: 60,
            velocity: 100,
            duration_secs: 0.25,
        },
        GameEvent::MobSweep { kill_count } => NoteEvent {
            channel: 0,
            note: 72 + kill_count.clamp(0, 12) as u8,
            velocity: 100,
            duration_secs: 0.5,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sixteenths_at_120bpm_are_an_eighth_of_a_second() {
        let quantiser = Quantiser::new(120.0, Subdivision::Sixteenth);
        assert!((quantiser.next_boundary(0.0) - 0.125).abs() < 1e-9);
        assert!((quantiser.next_boundary(0.3) - 0.375).abs() < 1e-9);
    }

    #[test]
    fn events_exactly_on_a_boundary_move_to_the_next_step() {
        let quantiser = Quantiser::new(120.0, Subdivision::Quarter);
        assert!((quantiser.next_boundary(0.5) - 1.0).abs() < 1e-9);
    }

    #[test]
    fn quarters_at_60bpm_are_one_second() {
        let quantiser = Quantiser::new(60.0, Subdivision::Quarter);
        assert!((quantiser.next_boundary(2.2) - 3.0).abs() < 1e-9);
    }

    #[test]
    fn sweep_notes_climb_with_kill_count() {
        let low = perform_event(GameEvent::MobSweep { kill_count: 1 });
        let high = perform_event(GameEvent::MobSweep { kill_count: 10 });
        assert!(high.note > low.note);
    }
}
