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
//! The live aggro-lock count (streamed from the game) conducts the harmonic
//! rhythm and the patterns. Chord slots start at eight crotchets and shed
//! two beats at a time down to two; at full flight the rhythm goes additive
//! in quavers, cycling 3+2, 3+3+2 and 3+2+2. A recalculated rhythm never
//! stretches a sounding chord — the change lands at the new period's
//! boundary when that falls inside the slot (a cut, note-offs silencing the
//! chord there), or at the end of the note when it falls past. Descents
//! hold their rung for a few seconds before dropping down — a fight ends,
//! the drive doesn't collapse on the spot. Patterns A and B gate on raw
//! locks (A at one, B at three) with density dropout thinning as it calms,
//! all off a compressor-smoothed intensity (fast attack, slow release) so
//! the boundaries never strobe.

use scalevec::{Scale, ScaleVec, Stack};

use crate::NoteEvent;

/// Stub tempo the score performs at (the hub tempo protocol stays deferred).
pub const STUB_BPM: f64 = 120.0;

/// Seconds per crotchet at the stub tempo.
pub const CROTCHET_SECS: f64 = 60.0 / STUB_BPM;
/// Seconds per quaver.
pub const QUAVER_SECS: f64 = CROTCHET_SECS / 2.0;
/// Seconds per 16th.
pub const STEP_SECS: f64 = CROTCHET_SECS / 4.0;

/// MIDI channels — one synth each in REAPER, with the one-shot combat fx on
/// their own channel (see `FX_CHANNEL` in the crate root).
pub const CHANNEL_BASS: u8 = 0;
pub const CHANNEL_CHORDS: u8 = 1;
pub const CHANNEL_PATTERN_A: u8 = 2;
pub const CHANNEL_PATTERN_B: u8 = 3;

/// Smoothed intensity past which the harmonic rhythm goes additive.
const ADDITIVE_THRESHOLD: f32 = 0.95;
/// Seconds the harmonic rhythm holds its rung before dropping down.
const RHYTHM_HOLD_SECS: f32 = 3.0;
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
/// to Dm. The bass follows this cycle in lockstep (see `BASS_CYCLE`).
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

/// A pattern note for global 16th step `step`: the figure walks the slot's
/// voicing, `octave` above the pad.
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

/// Whether this smoothed intensity sits in the additive regime.
pub fn is_additive(intensity: f32) -> bool {
    intensity >= ADDITIVE_THRESHOLD
}

/// The harmonic period in crotchets at this smoothed intensity: eight at
/// rest, shedding two beats at a time down to two, before the additive
/// regime takes over (see `is_additive`).
pub fn harmonic_period_beats(intensity: f32) -> f32 {
    if intensity < 0.25 {
        8.0
    } else if intensity < 0.5 {
        6.0
    } else if intensity < 0.75 {
        4.0
    } else {
        2.0
    }
}

/// The additive harmonic cycle at full flight: chord durations in quavers,
/// flattening 3+2, 3+3+2 and 3+2+2. Changes land on every group boundary.
const ADDITIVE_CYCLE_QUAVERS: [u32; 8] = [3, 2, 3, 3, 2, 3, 2, 2];

/// The period the CURRENT slot would take under this smoothed intensity —
/// the mid-slot recalculation's new period. In the additive regime the
/// first entry (3 quavers) stands in; against a shorter additive chord the
/// min in `change_point` makes that a no-op.
pub fn current_slot_period_secs(intensity: f32) -> f64 {
    if is_additive(intensity) {
        ADDITIVE_CYCLE_QUAVERS[0] as f64 * QUAVER_SECS
    } else {
        harmonic_period_beats(intensity) as f64 * CROTCHET_SECS
    }
}

/// The density dropout: which 16th steps of the cycle sound at this smoothed
/// intensity — quarter-note pulses when quiet, everything at full combat.
/// Indexed by the global step modulo the cycle length, so the rhythm keeps
/// flowing across slots of any length.
pub fn density_mask(smoothed: f32) -> [bool; 16] {
    let keep = if smoothed < 0.25 {
        2
    } else if smoothed < 0.55 {
        4
    } else if smoothed < 0.85 {
        8
    } else {
        16
    };
    let mut mask = [false; 16];
    for step in (0..16).step_by(16 / keep) {
        mask[step] = true;
    }
    mask
}

/// The first grid boundary strictly after `now`.
pub fn next_grid_boundary(now: f64, grid_secs: f64) -> f64 {
    ((now / grid_secs).floor() + 1.0) * grid_secs
}

/// Where a mid-slot recalculation moves the change point: the new period's
/// boundary when it lands inside the slot (a cut), otherwise the end of the
/// note. An overdue boundary — the recalculation arrived after the chord
/// should already have changed — snaps to the next grid point first.
pub fn change_point(
    slot_start: f64,
    scheduled_end: f64,
    new_period_secs: f64,
    grid_secs: f64,
    now: f64,
) -> f64 {
    let intended = slot_start + new_period_secs;
    let intended = if intended <= now {
        next_grid_boundary(now, grid_secs)
    } else {
        intended
    };
    intended.min(scheduled_end)
}

/// Everything the conductor knows: the raw locks, the smoothed intensity
/// they drive, and where the cycles stand.
#[derive(Debug, Clone, PartialEq)]
pub struct MusicState {
    /// Live aggro locks streamed from the game (0 at rest).
    pub aggro_locks: u32,
    /// Compressor-smoothed 0–1 intensity.
    pub intensity: f32,
    /// Chord slots elapsed — the harmonic cycle's phase.
    pub slot: u32,
    /// The intensity the harmonic rhythm plays at — it follows the smoothed
    /// intensity up instantly, but holds its rung before dropping down.
    pub rhythm_intensity: f32,
    /// Seconds left on the rhythm's hold (0 = not holding).
    pub rhythm_hold: f32,
    /// Position in the additive duration cycle (advances only in the
    /// additive regime).
    pub additive_pos: usize,
    /// Global 16th counter — the patterns' continuous figure and density
    /// phase across slots of any length.
    pub pattern_step: usize,
    /// The previous slot's duration — the smoother's tick between slots.
    pub last_slot_secs: f64,
}

impl Default for MusicState {
    fn default() -> Self {
        Self {
            aggro_locks: 0,
            intensity: 0.0,
            slot: 0,
            rhythm_intensity: 0.0,
            rhythm_hold: 0.0,
            additive_pos: 0,
            pattern_step: 0,
            last_slot_secs: 4.0 * CROTCHET_SECS,
        }
    }
}

/// Track the harmonic rhythm's intensity toward the smoothed one: follow up
/// instantly (a fight kicking off cuts the chords straight away), but on the
/// first drop request hold the current rung for [`RHYTHM_HOLD_SECS`] before
/// following down — the end of a fight eases rather than collapses.
fn follow_rhythm(state: &mut MusicState, dt_secs: f32) {
    if dt_secs <= 0.0 {
        return;
    }
    if state.intensity >= state.rhythm_intensity {
        // Attack — and a recovering fight cancels any hold in progress.
        state.rhythm_intensity = state.intensity;
        state.rhythm_hold = 0.0;
    } else if state.rhythm_hold > 0.0 {
        // Holding the rung while the drop request persists.
        state.rhythm_hold -= dt_secs;
        if state.rhythm_hold <= 0.0 {
            state.rhythm_intensity = state.intensity;
            state.rhythm_hold = 0.0;
        }
    } else {
        // First drop request: hold the rung, then follow down.
        state.rhythm_hold = RHYTHM_HOLD_SECS;
    }
}

/// Tick the smoother toward the raw lock count across `dt` seconds — the
/// mid-slot recalculation path (slots normally tick it as they render).
pub fn retune(state: &mut MusicState, locks: u32, dt_secs: f32) {
    state.aggro_locks = locks;
    let target = intensity_target(locks);
    state.intensity = smooth_value(state.intensity, target, ATTACK_TAU, RELEASE_TAU, dt_secs);
    follow_rhythm(state, dt_secs);
}

/// Renders chord slots deterministically from the state.
pub struct Engine;

impl Engine {
    /// The chord slot starting at `slot_start` (a 16th-grid hub time): the
    /// bass and pad in lockstep — one chord, one bass note — and the
    /// patterns on the global 16th grid across the span. Returns
    /// (offset within the slot, note) pairs and the slot's end (the next
    /// change point). Same state, same notes.
    pub fn slot_notes(state: &mut MusicState, slot_start: f64) -> (Vec<(f64, NoteEvent)>, f64) {
        // The conductor reads the room: retarget intensity from the raw
        // locks and tick the smoother across the previous slot's span.
        let target = intensity_target(state.aggro_locks);
        state.intensity = smooth_value(
            state.intensity,
            target,
            ATTACK_TAU,
            RELEASE_TAU,
            state.last_slot_secs as f32,
        );
        follow_rhythm(state, state.last_slot_secs as f32);

        // The slot's duration: a crotchet-band period, or the next entry of
        // the additive cycle at full flight — the rhythm's intensity, which
        // holds its rung on the way down, not the raw smoothed value.
        let duration_secs = if is_additive(state.rhythm_intensity) {
            let quavers =
                ADDITIVE_CYCLE_QUAVERS[state.additive_pos % ADDITIVE_CYCLE_QUAVERS.len()];
            state.additive_pos += 1;
            quavers as f64 * QUAVER_SECS
        } else {
            harmonic_period_beats(state.rhythm_intensity) as f64 * CROTCHET_SECS
        };
        state.last_slot_secs = duration_secs;
        let slot_end = slot_start + duration_secs;

        let mut notes = Vec::new();

        // Bass, locked to the harmony.
        notes.push((
            0.0,
            NoteEvent {
                channel: CHANNEL_BASS,
                note: bass_note(state.slot),
                velocity: 88,
                duration_secs: duration_secs * 0.8,
            },
        ));

        // Pad voicing.
        for &tone in &chord_voicing(state.slot) {
            notes.push((
                0.0,
                NoteEvent {
                    channel: CHANNEL_CHORDS,
                    note: tone,
                    velocity: 60,
                    duration_secs: duration_secs * 0.92,
                },
            ));
        }

        // Patterns ride the global 16th grid across the slot's span —
        // odd-length additive slots land mid-figure and the figure carries.
        // A joins at one lock; B at three, on even steps only — an
        // eighth-note shadow under A's sixteenths.
        if state.aggro_locks >= 1 {
            let mask = density_mask(state.intensity);
            let voicing = chord_voicing(state.slot);
            let steps = (duration_secs / STEP_SECS).round() as usize;
            for k in 0..steps {
                let global = state.pattern_step + k;
                if !mask[global % mask.len()] {
                    continue;
                }
                let at = k as f64 * STEP_SECS;
                notes.push((
                    at,
                    NoteEvent {
                        channel: CHANNEL_PATTERN_A,
                        note: pattern_note(&voicing, &FIGURE_A, global, PATTERN_A_OCTAVE),
                        velocity: 72,
                        duration_secs: 0.11,
                    },
                ));
                if state.aggro_locks >= 3 && global.is_multiple_of(2) {
                    notes.push((
                        at,
                        NoteEvent {
                            channel: CHANNEL_PATTERN_B,
                            note: pattern_note(&voicing, &FIGURE_B, global, PATTERN_B_OCTAVE),
                            velocity: 78,
                            duration_secs: 0.11,
                        },
                    ));
                }
            }
            state.pattern_step += steps;
        }

        // Advance the harmonic cycle — one chord per slot.
        state.slot += 1;
        (notes, slot_end)
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

    fn assert_secs(actual: f64, expected: f64) {
        assert!(
            (actual - expected).abs() < 1e-9,
            "expected {expected}s, got {actual}s"
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
    fn intensity_saturates_at_six_locks() {
        assert_eq!(intensity_target(0), 0.0);
        assert!((intensity_target(3) - 0.5).abs() < 1e-6);
        assert_eq!(intensity_target(6), 1.0);
        assert_eq!(intensity_target(12), 1.0);
    }

    #[test]
    fn harmonic_period_sheds_two_beats_at_a_time() {
        assert_eq!(harmonic_period_beats(0.0), 8.0);
        assert_eq!(harmonic_period_beats(0.3), 6.0);
        assert_eq!(harmonic_period_beats(0.55), 4.0);
        assert_eq!(harmonic_period_beats(0.8), 2.0);
        assert!(!is_additive(0.8));
        assert!(is_additive(1.0));
    }

    #[test]
    fn a_calm_slot_is_bass_and_pad_over_eight_crotchets() {
        let mut state = MusicState::default();
        let (notes, end) = Engine::slot_notes(&mut state, 0.0);
        assert_eq!(notes.len(), 1 + 3); // one bass note, one three-tone pad
        assert!(notes.iter().all(|(_, n)| n.channel <= CHANNEL_CHORDS));
        assert_secs(end, 8.0 * CROTCHET_SECS);
        assert_eq!(state.slot, 1);
    }

    #[test]
    fn full_flight_plays_the_additive_cycle() {
        let mut state = MusicState {
            aggro_locks: 6,
            intensity: 1.0,
            ..MusicState::default()
        };
        // 3+2, 3+3+2, 3+2+2 quavers, then the cycle wraps around again.
        let expected = [3, 2, 3, 3, 2, 3, 2, 2, 3];
        let mut cursor = 0.0;
        for &quavers in &expected {
            let (_, end) = Engine::slot_notes(&mut state, cursor);
            assert_secs(end - cursor, quavers as f64 * QUAVER_SECS);
            cursor = end;
        }
    }

    #[test]
    fn combat_slots_gate_the_patterns_by_lock_count() {
        // One lock: pattern A joins (thin — the smoother just started),
        // B waits for the third lock.
        let mut quiet = MusicState {
            aggro_locks: 1,
            ..MusicState::default()
        };
        let (notes, _) = Engine::slot_notes(&mut quiet, 0.0);
        let a_notes = notes
            .iter()
            .filter(|(_, n)| n.channel == CHANNEL_PATTERN_A)
            .count();
        let b_notes = notes
            .iter()
            .filter(|(_, n)| n.channel == CHANNEL_PATTERN_B)
            .count();
        assert!(a_notes > 0);
        assert_eq!(b_notes, 0);

        // Steady combat: a 3-quaver additive slot spans six 16ths; A covers
        // them all and B shadows on the eighths.
        let mut combat = MusicState {
            aggro_locks: 6,
            intensity: 1.0,
            ..MusicState::default()
        };
        let mut cursor = 0.0;
        let notes = (0..3)
            .map(|_| {
                let (notes, end) = Engine::slot_notes(&mut combat, cursor);
                cursor = end;
                notes
            })
            .last()
            .expect("at least one slot");
        let a_notes = notes
            .iter()
            .filter(|(_, n)| n.channel == CHANNEL_PATTERN_A)
            .count();
        let b_notes = notes
            .iter()
            .filter(|(_, n)| n.channel == CHANNEL_PATTERN_B)
            .count();
        assert_eq!(a_notes, 6); // full 16ths across the 3-quaver slot
        assert_eq!(b_notes, 3); // eighth-note shadow
    }

    #[test]
    fn slots_are_deterministic() {
        let mut first = MusicState {
            aggro_locks: 6,
            intensity: 1.0,
            ..MusicState::default()
        };
        let mut second = MusicState {
            aggro_locks: 6,
            intensity: 1.0,
            ..MusicState::default()
        };
        assert_eq!(
            Engine::slot_notes(&mut first, 2.0),
            Engine::slot_notes(&mut second, 2.0)
        );
    }

    #[test]
    fn mid_slot_recalc_cuts_short_or_rides_to_the_end_of_the_note() {
        // A boundary inside the slot cuts the chord short.
        assert_secs(change_point(0.0, 4.0, 1.0, 0.5, 0.4), 1.0);
        // An overdue boundary snaps to the next grid point first.
        assert_secs(change_point(0.0, 4.0, 1.0, 0.5, 1.2), 1.5);
        // A boundary past the end never stretches the note: it changes at
        // the end of the note.
        assert_secs(change_point(0.0, 4.0, 6.0, 0.5, 2.0), 4.0);
    }

    #[test]
    fn retune_ticks_the_smoother_toward_the_locks() {
        let mut state = MusicState::default();
        retune(&mut state, 6, 0.5);
        assert!(state.intensity > 0.6, "attack closes most of the gap");
        retune(&mut state, 0, 0.5);
        assert!(state.intensity > 0.55, "release barely falls");
        assert_eq!(state.aggro_locks, 0);
    }

    #[test]
    fn the_rhythm_holds_its_rung_before_dropping_down() {
        let mut state = MusicState {
            aggro_locks: 6,
            intensity: 1.0,
            rhythm_intensity: 1.0,
            ..MusicState::default()
        };
        assert!(is_additive(state.rhythm_intensity));

        // The fight breaks: the smoothed intensity glides down, but the
        // rhythm holds the additive rung for the hold duration.
        retune(&mut state, 0, 1.0);
        assert!(is_additive(state.rhythm_intensity), "holding the rung");

        // Slots tick by (short additive chords); the hold lapses within
        // them and the rhythm follows the intensity down and out.
        let mut cursor = 0.0;
        for _ in 0..12 {
            let (_, end) = Engine::slot_notes(&mut state, cursor);
            cursor = end;
        }
        assert!(
            !is_additive(state.rhythm_intensity),
            "the rhythm dropped down after the hold"
        );
    }

    #[test]
    fn a_recovering_fight_cancels_the_hold_and_follows_back_up() {
        let mut state = MusicState::default();
        retune(&mut state, 6, 0.5);
        assert!((state.rhythm_intensity - state.intensity).abs() < 1e-6);
        // A dip starts a hold; a recovery cancels it and follows up.
        state.intensity = 0.3;
        follow_rhythm(&mut state, 0.1);
        assert!(state.rhythm_hold > 0.0, "drop request starts the hold");
        state.intensity = 0.9;
        follow_rhythm(&mut state, 0.1);
        assert_eq!(state.rhythm_hold, 0.0);
        assert!((state.rhythm_intensity - 0.9).abs() < 1e-6);
    }

    #[test]
    fn grid_boundaries_quantise_forward() {
        assert_secs(next_grid_boundary(0.3, 0.5), 0.5);
        assert_secs(next_grid_boundary(0.5, 0.5), 1.0); // strictly after
        assert_secs(next_grid_boundary(4.1, 0.25), 4.25);
    }
}
