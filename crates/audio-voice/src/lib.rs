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

/// MIDI controller the player-speed telemetry drives (mod wheel; synths
/// commonly route it to filter cutoff).
pub const MOD_WHEEL_CC: u8 = 1;

/// Game events the audio voice responds to (GDD section 5)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GameEvent {
    /// Primary attack trigger — quantised synth stab.
    AttackPrimary,
    /// Enemy kill / AoE mob sweep — melodic run based on hit count, with the
    /// combo chain lifting the performance intensity.
    MobSweep { kill_count: u32, combo: u32 },
    /// A thrower enemy lobbed a projectile — low quiet blip.
    ProjectileThrow,
    /// The shield reflected a projectile — bright stab.
    ShieldReflect,
    /// The player levelled up — rising chime.
    LevelUp,
    /// The player dashed — short high whoosh.
    Dash,
    /// A grenade detonated — deep boom.
    GrenadeBlast,
    /// The player fired a nova burst — deeper, wider boom.
    Nova,
    /// The player engaged the cloak — soft descending breath.
    Cloak,
    /// The player descended to the next depth — deep transition tone.
    Descent,
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

/// Performance mapping: choose a note for a game event. Milestone 5
/// replaces this with scale-aware harmonic selection.
pub fn perform_event(event: GameEvent) -> NoteEvent {
    match event {
        GameEvent::AttackPrimary => NoteEvent {
            channel: 0,
            note: 60,
            velocity: 100,
            duration_secs: 0.25,
        },
        GameEvent::MobSweep { kill_count, combo } => {
            // Longer combos hit harder: velocity climbs with the chain.
            let velocity = (100 + combo.saturating_sub(1) * 5).min(127) as u8;
            NoteEvent {
                channel: 0,
                note: 72 + kill_count.clamp(0, 12) as u8,
                velocity,
                duration_secs: 0.5,
            }
        }
        GameEvent::ProjectileThrow => NoteEvent {
            channel: 0,
            note: 45,
            velocity: 60,
            duration_secs: 0.1,
        },
        GameEvent::ShieldReflect => NoteEvent {
            channel: 0,
            note: 79,
            velocity: 110,
            duration_secs: 0.15,
        },
        GameEvent::LevelUp => NoteEvent {
            channel: 0,
            note: 84,
            velocity: 90,
            duration_secs: 0.3,
        },
        GameEvent::Dash => NoteEvent {
            channel: 0,
            note: 91,
            velocity: 70,
            duration_secs: 0.08,
        },
        GameEvent::GrenadeBlast => NoteEvent {
            channel: 0,
            note: 36,
            velocity: 127,
            duration_secs: 0.4,
        },
        GameEvent::Nova => NoteEvent {
            channel: 0,
            note: 38,
            velocity: 127,
            duration_secs: 0.45,
        },
        GameEvent::Cloak => NoteEvent {
            channel: 0,
            note: 67,
            velocity: 75,
            duration_secs: 0.3,
        },
        GameEvent::Descent => NoteEvent {
            channel: 0,
            note: 43,
            velocity: 100,
            duration_secs: 0.4,
        },
    }
}

/// Map player speed onto the 7-bit CC range: stationary is 0, full speed
/// (the game's `PLAYER_SPEED`) is 127. Out-of-range input clamps.
pub fn speed_to_cc(speed: f32, max_speed: f32) -> u8 {
    if max_speed <= 0.0 {
        return 0;
    }
    (speed / max_speed * 127.0).round().clamp(0.0, 127.0) as u8
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
        let low = perform_event(GameEvent::MobSweep {
            kill_count: 1,
            combo: 1,
        });
        let high = perform_event(GameEvent::MobSweep {
            kill_count: 10,
            combo: 1,
        });
        assert!(high.note > low.note);
    }

    #[test]
    fn combo_chains_lift_velocity() {
        let lone = perform_event(GameEvent::MobSweep {
            kill_count: 3,
            combo: 1,
        });
        let chained = perform_event(GameEvent::MobSweep {
            kill_count: 3,
            combo: 4,
        });
        assert_eq!(lone.note, chained.note);
        assert!(chained.velocity > lone.velocity);
        // Velocity never clips past the MIDI ceiling.
        let huge = perform_event(GameEvent::MobSweep {
            kill_count: 3,
            combo: 100,
        });
        assert_eq!(huge.velocity, 127);
    }

    #[test]
    fn throws_blip_low_and_reflects_stab_bright() {
        let throw = perform_event(GameEvent::ProjectileThrow);
        let reflect = perform_event(GameEvent::ShieldReflect);
        assert!(reflect.note > throw.note);
        assert!(reflect.velocity > throw.velocity);
    }

    #[test]
    fn speed_maps_onto_the_full_cc_range() {
        assert_eq!(speed_to_cc(0.0, 240.0), 0);
        assert_eq!(speed_to_cc(240.0, 240.0), 127);
        assert_eq!(speed_to_cc(120.0, 240.0), 64);
        // Beyond full speed and garbage configs clamp safely.
        assert_eq!(speed_to_cc(480.0, 240.0), 127);
        assert_eq!(speed_to_cc(10.0, 0.0), 0);
        assert_eq!(speed_to_cc(-5.0, 240.0), 0);
    }
}
