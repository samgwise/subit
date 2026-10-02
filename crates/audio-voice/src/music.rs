//! The generative score: a coherent layered harmony conducted by aggro locks.
//!
//! Three scalevec layers stack into the pitch resolution chain — the harmony
//! cycle spells chord slots as degrees, the scale layer (a D major/minor
//! mixture with the borrowed G# colour) resolves degrees to semitones, and
//! the chromatic ladder hands those semitones to MIDI as note numbers. The
//! bass line is locked to the cycle in lockstep — one note per chord slot,
//! sounding that slot's designated bass — so every pairing is intentional,
//! and later harmonic disintegration can be a deliberate transformation of
//! any single layer while the others hold.
//!
//! The live aggro-lock count (streamed from the game) conducts three knobs:
//! pattern gating (A joins at one lock, B at three), note density
//! (subdivision dropout thins as it calms), and harmonic rhythm (one chord
//! per bar at rest, up to four in full combat). Intensity is smoothed
//! compressor-style — fast attack, slow release — so the boundaries never
//! strobe.

use scalevec::{Scale, ScaleVec, Stack};

use crate::NoteEvent;

/// Bar length at the stub tempo: four beats at 120 BPM.
pub const BAR_SECS: f64 = 2.0;

/// 16th-note steps in a bar.
pub const STEPS_PER_BAR: usize = 16;

/// MIDI channels — one synth each in REAPER, with the one-shot combat fx on
/// their own channel (see `FX_CHANNEL` in the crate root).
pub const CHANNEL_BASS: u8 = 0;
pub const CHANNEL_CHORDS: u8 = 1;
pub const CHANNEL_PATTERN_A: u8 = 2;
pub const CHANNEL_PATTERN_B: u8 = 3;

/// Fast attack time constant (seconds) for the intensity smoother.
const ATTACK_TAU: f32 = 0.5;
/// Slow release time constant (seconds) — combat fades out over bars.
const RELEASE_TAU: f32 = 8.0;
/// Lock count mapped to full intensity (deep fights sit past this).
const FULL_INTENSITY_LOCKS: f32 = 6.0;
/// The pad registers chord tones above this (D3); the patterns sit an octave
/// (A) or two (B) above it.
const PAD_BASE: f64 = 50.0;
const PATTERN_A_OCTAVE: f64 = 12.0;
const PATTERN_B_OCTAVE: f64 = 24.0;

/// Chord quality: the intervals (semitones from the root) its voicing stacks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Quality {
    Minor,
    Major,
    /// C# minor carrying the 4–3 suspension colour: the suspended fourth sits
    /// beside the third until a voice resolves it.
    MinorSus43,
    Diminished7,
}

impl Quality {
    fn intervals(self) -> &'static [f64] {
        match self {
            Quality::Minor => &[0.0, 3.0, 7.0],
            Quality::Major => &[0.0, 4.0, 7.0],
            Quality::MinorSus43 => &[0.0, 3.0, 4.0, 7.0],
            Quality::Diminished7 => &[0.0, 3.0, 6.0, 9.0],
        }
    }
}

/// The scale layer: the D-based major/minor mixture the harmony lives in, in
/// semitones from the tonic — both thirds (F for Dm, F# for the major
/// colours), the natural seventh, and the borrowed G# the dim7 needs. Ten
/// degrees plus the octave boundary.
fn scale_layer() -> ScaleVec {
    ScaleVec::new(
        vec![0.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 9.0, 10.0, 11.0, 12.0],
        None,
    )
}

/// The chromatic layer: the full semitone ladder the MIDI grid speaks, with
/// octave wrapping every twelve steps.
fn chromatic_layer() -> ScaleVec {
    ScaleVec::new((0..=12).map(f64::from).collect(), None)
}

/// The pitch resolution stack: harmony degrees → scale semitones → chromatic
/// semitones. On integers the chromatic pass is the identity, but it is
/// where octave folding lands anything fractional, and it keeps the three
/// layers explicit in the chain.
fn pitch_stack() -> Stack {
    Stack::new(vec![Box::new(scale_layer()), Box::new(chromatic_layer())])
}

/// The harmony layer: the nine chord slots as (scale degree of the root,
/// quality). E major sits penultimate to prepare G#dim7 — the shared G# and
/// B make the slide into the diminished smooth, and the dim7 resolves home
/// to Dm. The bass follows this cycle in lockstep (see `bass_cycle`).
const HARMONY_CYCLE: [(f64, Quality); 9] = [
    (0.0, Quality::Minor),       // Dm
    (7.0, Quality::Minor),       // Bm
    (0.0, Quality::Major),       // D
    (7.0, Quality::Major),       // B
    (3.0, Quality::Major),       // F#
    (9.0, Quality::MinorSus43),  // C#m4-3
    (7.0, Quality::Major),       // B/D (the bass sounds the D)
    (1.0, Quality::Major),       // E — prepares the dim7
    (5.0, Quality::Diminished7), // G#dim7 — resolves home to Dm
];

/// The slot count both cycles share — the harmony's ground truth the bass is
/// locked to (and the seam later disintegration effects deform).
const CYCLE_LEN: usize = HARMONY_CYCLE.len();

/// The bass line: one absolute-MIDI note per chord slot, locked to the
/// harmony cycle — D2 B1 D2 B1 F#2 C#2 D2 E2 G#2, with the B/D slot sounding
/// its D. A plain sequence (ScaleVec's step() maps monotonic ladders, not
/// ordered cycles), wrapped by the same slot arithmetic as the harmony.
const BASS_CYCLE: [u8; 9] = [38, 35, 38, 35, 42, 37, 38, 40, 44];

/// The slot's bass note — the bass cycle walks in lockstep with the harmony.
fn bass_note(slot: u32) -> u8 {
    BASS_CYCLE[(slot as usize) % CYCLE_LEN]
}

/// The slot's pad voicing: the root degree walks the harmony cycle, the
/// pitch stack turns the degree into semitones from the tonic, and the
/// quality stacks intervals above it — each tone folded into the pad's
/// octave by its pitch class.
fn chord_voicing(slot: u32) -> Vec<u8> {
    let (degree, quality) = HARMONY_CYCLE[(slot as usize) % CYCLE_LEN];
    let root_semitone = pitch_stack().step(degree);
    let root_pc = root_semitone.rem_euclid(12.0);
    quality
        .intervals()
        .iter()
        .map(|&interval| (PAD_BASE + root_pc + interval) as u8)
        .collect()
}

/// Pattern A's figure: voicing indices walked per 16th step, up the chord.
const FIGURE_A: [usize; 4] = [0, 1, 2, 3];
/// Pattern B's figure: a sparser contour leaning back down the chord.
const FIGURE_B: [usize; 4] = [2, 1, 0, 1];

/// A pattern note for bar step `step`: the figure walks the slot's voicing,
/// `octave` above the pad.
fn pattern_note(voicing: &[u8], figure: &[usize; 4], step: usize, octave: f64) -> u8 {
    let tone = voicing[figure[step % figure.len()] % voicing.len()];
    (tone as f64 + octave) as u8
}

/// Compressor-style smoothing: fast attack, slow release, so a single aggro
/// flicker never strobes the arrangement. Non-positive `dt` changes nothing.
pub fn smooth_value(current: f32, target: f32, attack_tau: f32, release_tau: f32, dt: f32) -> f32 {
    if dt <= 0.0 {
        return current;
    }
    let tau = if target > current { attack_tau } else { release_tau };
    current + (target - current) * (1.0 - (-dt / tau).exp())
}

/// The raw lock count mapped onto 0–1 intensity (six locks is full flight).
pub fn intensity_target(locks: u32) -> f32 {
    (locks as f32 / FULL_INTENSITY_LOCKS).min(1.0)
}

/// Chord slots per bar at this smoothed intensity: one at rest, then two,
/// then four in full combat.
pub fn harmonic_rhythm(smoothed: f32) -> u32 {
    if smoothed < 1.0 / 3.0 {
        1
    } else if smoothed < 2.0 / 3.0 {
        2
    } else {
        4
    }
}

/// The density dropout: which 16th steps of the bar sound at this smoothed
/// intensity — quarter-note pulses when quiet, everything at full combat.
pub fn density_mask(smoothed: f32) -> [bool; STEPS_PER_BAR] {
    let keep = if smoothed < 0.25 {
        2
    } else if smoothed < 0.55 {
        4
    } else if smoothed < 0.85 {
        8
    } else {
        STEPS_PER_BAR
    };
    let mut mask = [false; STEPS_PER_BAR];
    for step in (0..STEPS_PER_BAR).step_by(STEPS_PER_BAR / keep) {
        mask[step] = true;
    }
    mask
}

/// The first bar boundary strictly after `now` — bar starts quantise to the
/// hub clock so scheduled notes never land in the past.
pub fn next_bar_boundary(now: f64, bar_secs: f64) -> f64 {
    ((now / bar_secs).floor() + 1.0) * bar_secs
}

/// Everything the conductor knows: the raw locks, the smoothed intensity
/// they drive, and where the cycles stand.
#[derive(Debug, Clone, PartialEq)]
pub struct MusicState {
    /// Live aggro locks streamed from the game (0 at rest).
    pub aggro_locks: u32,
    /// Compressor-smoothed 0–1 intensity.
    pub intensity: f32,
    /// Chord slots elapsed since the score started — the shared phase of
    /// both cycles.
    pub slot: u32,
    /// Bars elapsed (bookkeeping for tests and logs).
    pub bars: u32,
}

impl Default for MusicState {
    fn default() -> Self {
        Self {
            aggro_locks: 0,
            intensity: 0.0,
            slot: 0,
            bars: 0,
        }
    }
}

/// Renders whole bars deterministically from the state.
pub struct Engine;

impl Engine {
    /// The notes of the next bar: bass and pad in lockstep per chord slot,
    /// patterns gated by raw locks and thinned by density. Returns
    /// (offset within the bar, note) pairs — same state, same notes.
    pub fn bar_notes(state: &mut MusicState, bar_secs: f64) -> Vec<(f64, NoteEvent)> {
        // The conductor reads the room: retarget intensity from the raw
        // locks and tick the smoother across the bar.
        let target = intensity_target(state.aggro_locks);
        state.intensity =
            smooth_value(state.intensity, target, ATTACK_TAU, RELEASE_TAU, bar_secs as f32);

        let slots = harmonic_rhythm(state.intensity);
        let slot_secs = bar_secs / slots as f64;
        let step_secs = bar_secs / STEPS_PER_BAR as f64;
        let mask = density_mask(state.intensity);
        let mut notes = Vec::new();

        for s in 0..slots {
            let slot = state.slot + s;
            let at = s as f64 * slot_secs;

            // Bass, locked to the harmony.
            notes.push((
                at,
                NoteEvent {
                    channel: CHANNEL_BASS,
                    note: bass_note(slot),
                    velocity: 88,
                    duration_secs: slot_secs * 0.8,
                },
            ));

            // Pad voicing.
            for &tone in &chord_voicing(slot) {
                notes.push((
                    at,
                    NoteEvent {
                        channel: CHANNEL_CHORDS,
                        note: tone,
                        velocity: 60,
                        duration_secs: slot_secs * 0.92,
                    },
                ));
            }
        }

        // Patterns ride the whole bar's 16th grid, walking whichever chord
        // owns each step. A joins at one lock; B at three, on even steps
        // only — an eighth-note shadow under A's sixteenths.
        if state.aggro_locks >= 1 {
            for (step, &on) in mask.iter().enumerate() {
                if !on {
                    continue;
                }
                let at = step as f64 * step_secs;
                let step_slot =
                    state.slot + (step * slots as usize / STEPS_PER_BAR) as u32;
                let voicing = chord_voicing(step_slot);
                notes.push((
                    at,
                    NoteEvent {
                        channel: CHANNEL_PATTERN_A,
                        note: pattern_note(&voicing, &FIGURE_A, step, PATTERN_A_OCTAVE),
                        velocity: 72,
                        duration_secs: 0.11,
                    },
                ));
                if state.aggro_locks >= 3 && step % 2 == 0 {
                    notes.push((
                        at,
                        NoteEvent {
                            channel: CHANNEL_PATTERN_B,
                            note: pattern_note(&voicing, &FIGURE_B, step, PATTERN_B_OCTAVE),
                            velocity: 78,
                            duration_secs: 0.11,
                        },
                    ));
                }
            }
        }

        // Advance the shared phase by however many slots the bar held.
        state.slot += slots;
        state.bars += 1;
        notes
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// ScaleVec arithmetic is exact on integer degrees; a whisker of slack
    /// keeps the assertions honest regardless.
    fn assert_semitone(actual: f64, expected: f64) {
        assert!(
            (actual - expected).abs() < 1e-9,
            "expected {expected}, got {actual}"
        );
    }

    #[test]
    fn scale_layer_resolves_the_mixture_degrees() {
        let scale = scale_layer();
        assert_semitone(scale.step(0.0), 0.0); // D
        assert_semitone(scale.step(1.0), 2.0); // E
        assert_semitone(scale.step(3.0), 4.0); // F#
        assert_semitone(scale.step(5.0), 6.0); // the borrowed G#
        assert_semitone(scale.step(7.0), 9.0); // B
        assert_semitone(scale.step(9.0), 11.0); // C#
        assert_semitone(scale.step(10.0), 12.0); // the octave boundary
    }

    #[test]
    fn cycles_wrap_exactly() {
        for slot in 0..(CYCLE_LEN as u32 * 3) {
            let next = slot + CYCLE_LEN as u32;
            assert_eq!(bass_note(next), bass_note(slot));
            assert_eq!(chord_voicing(next), chord_voicing(slot));
        }
    }

    #[test]
    fn bass_is_locked_to_the_harmony() {
        // Every slot sounds its designated bass: the D under B/D, the E
        // under the E major prepare, the G# under the dim7.
        assert_eq!(bass_note(6), 38); // B/D sounds its D
        assert_eq!(bass_note(7), 40); // E
        assert_eq!(bass_note(8), 44); // G#
        assert_eq!(bass_note(0), 38); // home
    }

    #[test]
    fn e_major_prepares_the_dim7_which_resolves_home() {
        assert_eq!(chord_voicing(7), vec![52, 56, 59]); // E3 G#3 B3
        assert_eq!(chord_voicing(8), vec![56, 59, 62, 65]); // G#3 B3 D4 F4
        assert_eq!(chord_voicing(0), vec![50, 53, 57]); // Dm, home again
    }

    #[test]
    fn smoothing_attacks_fast_and_releases_slowly() {
        let up = smooth_value(0.0, 1.0, 0.5, 8.0, 0.5);
        let down = smooth_value(1.0, 0.0, 0.5, 8.0, 0.5);
        assert!(up > 0.6, "half a second of attack closes most of the gap");
        assert!(down > 0.9, "half a second of release barely falls");
        assert_eq!(smooth_value(0.5, 0.5, 0.5, 8.0, 1.0), 0.5);
        assert_eq!(smooth_value(0.5, 1.0, 0.5, 8.0, 0.0), 0.5);
    }

    #[test]
    fn harmonic_rhythm_and_density_climb_with_intensity() {
        assert_eq!(harmonic_rhythm(0.0), 1);
        assert_eq!(harmonic_rhythm(0.4), 2);
        assert_eq!(harmonic_rhythm(0.9), 4);
        let calm = density_mask(0.1);
        assert_eq!(calm.iter().filter(|&&on| on).count(), 2);
        let mid = density_mask(0.4);
        assert_eq!(mid.iter().filter(|&&on| on).count(), 4);
        let hot = density_mask(1.0);
        assert!(hot.iter().all(|&on| on));
    }

    #[test]
    fn intensity_saturates_at_six_locks() {
        assert_eq!(intensity_target(0), 0.0);
        assert!((intensity_target(3) - 0.5).abs() < 1e-6);
        assert_eq!(intensity_target(6), 1.0);
        assert_eq!(intensity_target(12), 1.0);
    }

    #[test]
    fn a_calm_bar_is_bass_and_pad_only() {
        let mut state = MusicState::default();
        let notes = Engine::bar_notes(&mut state, BAR_SECS);
        assert_eq!(notes.len(), 1 + 3); // one bass note, one three-tone pad
        assert!(notes.iter().all(|(_, n)| n.channel <= CHANNEL_CHORDS));
        assert_eq!(state.slot, 1);
        assert_eq!(state.bars, 1);
    }

    #[test]
    fn combat_bars_gate_the_patterns_by_lock_count() {
        let mut quiet = MusicState {
            aggro_locks: 1,
            ..MusicState::default()
        };
        let notes = Engine::bar_notes(&mut quiet, BAR_SECS);
        let a_notes = notes
            .iter()
            .filter(|(_, n)| n.channel == CHANNEL_PATTERN_A)
            .count();
        let b_notes = notes
            .iter()
            .filter(|(_, n)| n.channel == CHANNEL_PATTERN_B)
            .count();
        assert!(a_notes > 0);
        assert_eq!(b_notes, 0); // B waits for the third lock

        // Steady combat (six locks, smoother converged): A covers all 16
        // steps and B shadows it on the eighths.
        let mut combat = MusicState {
            aggro_locks: 6,
            ..MusicState::default()
        };
        let notes = (0..20)
            .map(|_| Engine::bar_notes(&mut combat, BAR_SECS))
            .last()
            .expect("at least one bar");
        let a_notes = notes
            .iter()
            .filter(|(_, n)| n.channel == CHANNEL_PATTERN_A)
            .count();
        let b_notes = notes
            .iter()
            .filter(|(_, n)| n.channel == CHANNEL_PATTERN_B)
            .count();
        assert_eq!(a_notes, STEPS_PER_BAR); // full 16ths
        assert_eq!(b_notes, STEPS_PER_BAR / 2); // eighth-note shadow
    }

    #[test]
    fn bars_are_deterministic() {
        let mut first = MusicState {
            aggro_locks: 4,
            ..MusicState::default()
        };
        let mut second = MusicState {
            aggro_locks: 4,
            ..MusicState::default()
        };
        assert_eq!(
            Engine::bar_notes(&mut first, BAR_SECS),
            Engine::bar_notes(&mut second, BAR_SECS)
        );
    }

    #[test]
    fn bar_boundaries_quantise_forward() {
        assert_eq!(next_bar_boundary(0.3, 2.0), 2.0);
        assert_eq!(next_bar_boundary(2.0, 2.0), 4.0); // strictly after
        assert_eq!(next_bar_boundary(4.1, 2.0), 6.0);
    }
}
