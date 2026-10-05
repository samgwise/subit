//! The form controller: the piece's tonal geography, above the composer.
//!
//! The composer plans phrases; the form decides where those phrases live.
//! Home is the ground state — a cycle of a few phrases (rolled from four,
//! six or eight) whose expiry rolls for a change: a **sequence** (a tone
//! up or down for one phrase — a colour, not a decision, home next
//! phrase) or an **episode** (a departure to a related key — ♭III, iv or
//! v — for a few phrases, each rolling a return chance, capped so an
//! episode can't outstay the form). The composer composes within the key
//! the form reports; the conductor keeps the clock; neither fights.

use rand::RngExt;
use rand::SeedableRng;
use rand::rngs::SmallRng;
use serde::Serialize;

use crate::music::KeyState;

/// The phrase counts a home stretch may run before the form rolls for a
/// change.
const HOME_CYCLES: [u32; 3] = [4, 6, 8];
/// The chance (0–1) that an expired cycle actually changes the music —
/// home has gravity; some cycles pass quietly.
const CHANGE_CHANCE: f32 = 0.6;
/// The chance that a change is a sequence (a tone away, one phrase)
/// rather than an episode.
const SEQUENCE_CHANCE: f32 = 0.6;
/// The chance an episode's phrase heads home instead of continuing.
const EPISODE_RETURN_CHANCE: f32 = 0.4;
/// The fewest and most phrases an episode may run.
const EPISODE_MIN_PHRASES: u32 = 2;
const EPISODE_MAX_PHRASES: u32 = 4;
/// An episode's destination, in tonic offsets from home: ♭III (the
/// relative major), iv (the subdominant minor), v (the modal dominant).
const EPISODE_OFFSETS: [i32; 3] = [3, 5, 7];

/// Which way a sequence steps.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    Up,
    Down,
}

/// The piece's tonal geography, ticked once per phrase.
#[derive(Debug, Clone)]
pub enum Form {
    /// The ground state, counting down its cycle (rolled from 4, 6 or 8).
    Home { phrases_left: u32, cycle: u32 },
    /// A tone away for a single phrase, then home.
    Sequence {
        direction: Direction,
        phrases_left: u32,
    },
    /// A related key (a tonic offset) for a bounded handful of phrases.
    Episode {
        offset: i32,
        phrases_left: u32,
    },
}

/// The composer's view of the form — rides into every plan request.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct FormSummary {
    /// "home", "sequence" or "episode".
    pub state: &'static str,
    /// The key the composer composes in (the offset applied to home).
    pub key: String,
    pub phrases_left: u32,
}

impl Default for FormSummary {
    fn default() -> Self {
        Self {
            state: "home",
            key: KeyState::default().name().to_string(),
            phrases_left: 4,
        }
    }
}

impl Form {
    /// A fresh form: home, with a rolled cycle.
    pub fn new(seed: u64) -> Self {
        let mut rng = SmallRng::seed_from_u64(seed);
        Self::home(&mut rng)
    }

    fn home(rng: &mut SmallRng) -> Self {
        let cycle = HOME_CYCLES[rng.random_range(0..HOME_CYCLES.len())];
        Self::Home {
            phrases_left: cycle,
            cycle,
        }
    }

    /// The tonic's offset from home while this state holds.
    pub fn key_offset(&self) -> i32 {
        match self {
            Form::Home { .. } => 0,
            Form::Sequence { direction, .. } => match direction {
                Direction::Up => 2,
                Direction::Down => -2,
            },
            Form::Episode { offset, .. } => *offset,
        }
    }

    /// Advance one phrase and roll the chances. Returns whether the form
    /// moved (a transition the daemon logs); an episode simply continuing
    /// doesn't count.
    pub fn tick(&mut self, rng: &mut SmallRng) -> bool {
        match self {
            Form::Home { phrases_left, .. } => {
                *phrases_left -= 1;
                if *phrases_left > 0 {
                    return false;
                }
                // The cycle expired: home has gravity — some cycles pass
                // quietly, and a quiet expiry re-rolls the next cycle.
                if rng.random::<f32>() >= CHANGE_CHANCE {
                    *self = Self::home(rng);
                    return false;
                }
                if rng.random::<f32>() < SEQUENCE_CHANCE {
                    let direction =
                        if rng.random::<bool>() { Direction::Up } else { Direction::Down };
                    *self = Form::Sequence {
                        direction,
                        phrases_left: 1,
                    };
                } else {
                    let offset = EPISODE_OFFSETS[rng.random_range(0..EPISODE_OFFSETS.len())];
                    let span = rng.random_range(EPISODE_MIN_PHRASES..=EPISODE_MAX_PHRASES);
                    *self = Form::Episode {
                        offset,
                        phrases_left: span,
                    };
                }
                true
            }
            Form::Sequence { .. } => {
                // A sequence is a colour, not a decision: home next phrase.
                *self = Self::home(rng);
                true
            }
            Form::Episode { phrases_left, .. } => {
                if rng.random::<f32>() < EPISODE_RETURN_CHANCE || *phrases_left <= 1 {
                    *self = Self::home(rng);
                    true
                } else {
                    *phrases_left -= 1;
                    false
                }
            }
        }
    }

    /// The composer's view of the form.
    pub fn summary(&self) -> FormSummary {
        let (state, phrases_left) = match self {
            Form::Home { phrases_left, .. } => ("home", *phrases_left),
            Form::Sequence { phrases_left, .. } => ("sequence", *phrases_left),
            Form::Episode { phrases_left, .. } => ("episode", *phrases_left),
        };
        FormSummary {
            state,
            key: KeyState {
                tonic: self.key_offset(),
            }
            .name()
            .to_string(),
            phrases_left,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Tick a fresh form through `phrases` phrases, collecting the states
    /// it passed through.
    fn walk(seed: u64, phrases: u32) -> Vec<Form> {
        let mut rng = SmallRng::seed_from_u64(seed);
        let mut form = Form::new(seed);
        let mut walked = vec![form.clone()];
        for _ in 0..phrases {
            form.tick(&mut rng);
            walked.push(form.clone());
        }
        walked
    }

    #[test]
    fn a_sequence_always_returns_home_after_one_phrase() {
        for seed in 0..2_000u64 {
            for pair in walk(seed, 60).windows(2) {
                if let Form::Sequence { .. } = pair[0] {
                    assert!(
                        matches!(pair[1], Form::Home { .. }),
                        "a sequence's next phrase is home"
                    );
                }
            }
        }
    }

    #[test]
    fn episodes_run_a_bounded_handful_of_phrases() {
        for seed in 0..2_000u64 {
            let mut rng = SmallRng::seed_from_u64(seed);
            let mut form = Form::new(seed);
            let mut run = 0;
            for _ in 0..60 {
                let before = form.clone();
                form.tick(&mut rng);
                match (&before, &form) {
                    (Form::Episode { offset, .. }, Form::Episode { offset: after, .. }) => {
                        assert_eq!(offset, after, "an episode continues in its key");
                        run += 1;
                        assert!(run < EPISODE_MAX_PHRASES, "the cap forces the return");
                    }
                    (Form::Episode { offset, .. }, Form::Home { .. }) => {
                        // An early return is by design — the return chance
                        // may fire on any phrase; only the cap is law.
                        assert!(
                            EPISODE_OFFSETS.contains(offset),
                            "the episode lived in a related key"
                        );
                        run = 0;
                    }
                    _ => run = 0,
                }
            }
        }
    }

    #[test]
    fn the_offset_tracks_the_state() {
        let mut rng = SmallRng::seed_from_u64(7);
        let mut form = Form::new(7);
        for _ in 0..200 {
            let offset = form.key_offset();
            match &form {
                Form::Home { .. } => assert_eq!(offset, 0),
                Form::Sequence { direction, .. } => assert_eq!(
                    offset,
                    match direction {
                        Direction::Up => 2,
                        Direction::Down => -2,
                    }
                ),
                Form::Episode { offset: episode, .. } => {
                    assert_eq!(offset, *episode);
                    assert!(EPISODE_OFFSETS.contains(episode));
                }
            }
            form.tick(&mut rng);
        }
    }

    #[test]
    fn the_summary_names_the_key_and_state() {
        let sequence = Form::Sequence {
            direction: Direction::Up,
            phrases_left: 1,
        };
        let summary = sequence.summary();
        assert_eq!(summary.state, "sequence");
        assert_eq!(summary.key, "E minor"); // a tone above home
        assert_eq!(summary.phrases_left, 1);

        let episode = Form::Episode {
            offset: 3,
            phrases_left: 3,
        };
        let summary = episode.summary();
        assert_eq!(summary.state, "episode");
        assert_eq!(summary.key, "F minor"); // ♭III of home
    }
}
