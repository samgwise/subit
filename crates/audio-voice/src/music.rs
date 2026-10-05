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
//! in quavers, cycling 3+2, 3+3+2 and 3+2+2. The bass pulses in quavers —
//! six and then rest in the relaxed regimes, continuous through the short
//! ones. A recalculated rhythm never
//! stretches a sounding chord — the change lands at the new period's
//! boundary when that falls inside the slot (a cut, note-offs silencing the
//! chord there), or at the end of the note when it falls past. Descents
//! hold their rung for a few seconds before dropping down — a fight ends,
//! the drive doesn't collapse on the spot. Patterns A and B gate on raw
//! locks (A at one, B at three) with density dropout thinning as it calms,
//! all off a compressor-smoothed intensity (fast attack, slow release) so
//! the boundaries never strobe. While the player stands in degraded data,
//! every chromatic mapping mutates — a noise-like rotation scatters the
//! harmony around the cycle, deterministically wrong.

use scalevec::{Scale, ScaleVec, Stack};

use crate::curve::{default_curve, Curve};
use crate::harmony::{PlanSlot, Quality, VOCABULARY};
use crate::NoteEvent;

/// Stub tempo the score performs at (the hub tempo protocol stays deferred).
pub const STUB_BPM: f64 = 120.0;

/// Seconds per crotchet at the stub tempo.
pub const CROTCHET_SECS: f64 = 60.0 / STUB_BPM;
/// Seconds per quaver.
pub const QUAVER_SECS: f64 = CROTCHET_SECS / 2.0;
/// Seconds per 16th.
pub const STEP_SECS: f64 = CROTCHET_SECS / 4.0;

/// MIDI channels — one synth each in REAPER: the score's bass, the three
/// chord voices (distant, bell, airy), the two patterns, and the one-shot
/// combat fx on their own channel (see `FX_CHANNEL` in the crate root).
pub const CHANNEL_BASS: u8 = 0;
pub const CHANNEL_DISTANT: u8 = 1;
pub const CHANNEL_BELL: u8 = 2;
pub const CHANNEL_AIRY: u8 = 3;
pub const CHANNEL_PATTERN_A: u8 = 4;
pub const CHANNEL_PATTERN_B: u8 = 5;

/// Smoothed intensity past which the harmonic rhythm goes additive.
const ADDITIVE_THRESHOLD: f32 = 0.95;
/// Seconds the harmonic rhythm holds its rung before dropping down.
const RHYTHM_HOLD_SECS: f32 = 3.0;
/// Quaver pulses the bass states in the relaxed regimes before resting.
const BASS_PULSES: usize = 6;
/// The bass pulse's gate as a fraction of a quaver. The note-off must have
/// room to fire before the next pulse: the bridge drops a re-play while the
/// key is still down (and the bumped-away note-off would stick the note).
const BASS_PULSE_GATE: f64 = 0.6;
/// The degraded zone's rotation spread: a mutated mapping lands within
/// ±`DEGRADATION_SEMITONES` of its clean pitch class — enough to mangle the
/// intervals, never enough to lose the gesture.
const DEGRADATION_SEMITONES: i64 = 3;
/// Fast attack time constant (seconds) for the intensity smoother.
const ATTACK_TAU: f32 = 0.5;
/// Slow release time constant (seconds) — combat fades out over bars.
const RELEASE_TAU: f32 = 8.0;
/// Lock count mapped to full intensity (deep fights sit past this).
const FULL_INTENSITY_LOCKS: f32 = 6.0;
/// The chord voices register their tones above this (D3); the patterns sit
/// an octave (A) or two (B) above it.
const PAD_BASE: f64 = 50.0;
const PATTERN_A_OCTAVE: f64 = 12.0;
const PATTERN_B_OCTAVE: f64 = 24.0;
/// The three chord voices' register bands (semitones from the tonic): the
/// distant pad hugs the floor, the airy spreads the top, and the bell
/// sits above them all.
const DISTANT_BAND: (f64, f64) = (0.0, 14.0);
const AIRY_BAND: (f64, f64) = (14.0, 26.0);
const BELL_BAND: (f64, f64) = (26.0, 38.0);
/// The voices' rhythmic offsets: the distant floor lands on the slot, the
/// airy enters a quaver in, and the bell plinks three 16ths in — off the
/// quaver grid, pointillistic.
const DISTANT_OFFSET: f64 = 0.0;
const AIRY_OFFSET: f64 = QUAVER_SECS;
const BELL_OFFSET: f64 = STEP_SECS * 3.0;
/// The bell's gate — a pluck, not a sustain.
const BELL_SECS: f64 = 0.3;
/// The idle bell's rhythmic patterns, in 16th-step offsets — four shapes
/// it walks through when the score sits in the sparse state (the single
/// plink is the first; the others scatter pairs and a three-plink
/// scatter).
const BELL_PATTERNS: [&[usize]; 4] = [&[3], &[0, 6], &[2, 7, 12], &[1, 9]];
/// The sustained voices' scheduled length, in slots — generously long so
/// the daemon's common-tone legato can hold them across changes; the
/// note-offs trim what doesn't persist.
const SUSTAIN_SLOTS: f64 = 3.0;
/// The objective's progress past which the arrival lift adds the airy
/// voice to even a sparse slot — the goal's glow, audible from afar.
const ARRIVAL_LIFT_PROGRESS: f32 = 0.85;

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
pub(crate) fn pitch_stack() -> Stack {
    Stack::new(vec![Box::new(scale_layer()), Box::new(chromatic_layer())])
}

/// The harmony layer: the nine chord slots as (scale degree of the root,
/// quality), in roman numerals relative to the tonic. E major sits
/// penultimate to prepare the dim7 — the shared tones make the slide into
/// the diminished smooth, and the dim7 resolves home to i. The bass
/// follows this cycle in lockstep (see `BASS_CYCLE`).
const HARMONY_CYCLE: [(f64, Quality); 9] = [
    (0.0, Quality::Minor),       // i
    (7.0, Quality::Minor),       // v
    (0.0, Quality::Major),       // I
    (7.0, Quality::Major),       // VI
    (3.0, Quality::Major),       // III
    (9.0, Quality::MinorSus43),  // #vii (4-3)
    (7.0, Quality::Major),       // VI, first inversion (the bass sounds the D)
    (1.0, Quality::Major),       // II — prepares the dim7
    (5.0, Quality::Diminished7), // #v°7 — resolves home to i
];

/// The slot count both cycles share — the harmony's ground truth the bass is
/// locked to (and the seam later disintegration effects deform).
const CYCLE_LEN: usize = HARMONY_CYCLE.len();

/// The bass line: one (degree, octave) per chord slot, locked to the
/// harmony cycle — the roots with the VI slot's inversion (the bass sounds
/// the tonic under it) and the line's written contour (the v under, the
/// #vii below). The degrees resolve through the pitch stack like every
/// other voice — transposed by the key, degraded with it.
const BASS_CYCLE: [(f64, i32); 9] = [
    (0.0, 0),
    (7.0, -1),
    (0.0, 0),
    (7.0, -1),
    (3.0, 0),
    (9.0, -1),
    (0.0, 0),
    (1.0, 0),
    (5.0, 0),
];

/// The tonic's bass note (D2) — every bass note sits relative to it: the
/// key's offset, the degree's semitones, and the line's own octave.
const TONIC_BASS: u8 = 38;

/// The bass note for a written line's (degree, octave) under the key: the
/// stack resolves the transposed degree, the degradation rotates the pitch
/// class like every voice, and the line's octave keeps its contour.
fn bass_for_line(degree: f64, octave: i32, tonic: i32, degraded: bool) -> u8 {
    let semitone = pitch_stack().step(degree) + tonic as f64;
    (TONIC_BASS as i32 + 12 * octave + chromatic_map(semitone, degraded).rem_euclid(12.0) as i32)
        as u8
}

/// The bass note for a planned chord, honouring an optional inversion: the
/// named chord tone's pitch class (0 root, 1 third, 2 fifth, 3 seventh)
/// resolves through the stack under the key. The plan's bass sits in the
/// home octave of the register.
fn bass_for_tone(
    degree: f64,
    quality: Quality,
    tone: Option<u32>,
    tonic: i32,
    degraded: bool,
) -> u8 {
    let intervals = quality.intervals();
    let tone = tone.map_or(0, |tone| (tone as usize).min(intervals.len() - 1));
    let semitone = pitch_stack().step(degree) + tonic as f64 + intervals[tone];
    TONIC_BASS + chromatic_map(semitone, degraded).rem_euclid(12.0) as u8
}

/// The deterministic cycle's slots as vocabulary indices — the history and
/// the composer's context speak the same numbered menu the plans do.
const CYCLE_VOCAB: [usize; CYCLE_LEN] = [0, 2, 1, 3, 4, 5, 3, 6, 7];

/// Whether the deterministic cycle just completed a full turn at this slot
/// count — the fallback path's phrase boundary for the form (plans count
/// their own phrases; the cycle's turn is its phrase).
pub fn cycle_turn_complete(slot: u32) -> bool {
    slot > 0 && (slot as usize).is_multiple_of(CYCLE_LEN)
}

/// The noise-like rotation: a splitmix64 finaliser over the pitch class —
/// cheap, deterministic, and uncorrelated between neighbouring pitches.
fn noise_hash(pc: i64) -> i64 {
    let mut z = (pc as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    (z ^ (z >> 31)) as i64
}

/// The chromatic layer's mapping for a chord tone: the semitone lands in
/// the pad's register as-is — and when the player stands in degraded data,
/// its pitch class rotates by the noise-like mutation first (the octave
/// part carries through, so the voicing keeps its shape). Every mapping
/// operation mutates, so a voicing comes out wrong the same way every time
/// while different pitches scatter differently.
fn chromatic_map(semitone: f64, degraded: bool) -> f64 {
    if !degraded {
        return semitone;
    }
    let pc = semitone.rem_euclid(12.0);
    let octave = semitone - pc;
    let rotation = noise_hash(pc as i64).rem_euclid(DEGRADATION_SEMITONES * 2 + 1) as f64
        - DEGRADATION_SEMITONES as f64;
    octave + (pc + rotation).rem_euclid(12.0)
}

/// Where each chord slot's harmony comes from: the deterministic cycle, or
/// a composer's plan consumed slot by slot. Either way the conductor keeps
/// the clock — the rhythm ladder, cuts and holds are untouched by the
/// source.
#[derive(Debug, Clone, PartialEq)]
pub enum HarmonySource {
    /// The nine-chord cycle — the deterministic ground truth.
    Cycle,
    /// A composer's plan: vocabulary chord slots (bass inversions
    /// included), the phrase's contour, and the next slot to perform. A
    /// depleted plan hands back to the cycle.
    Plan {
        slots: Vec<PlanSlot>,
        /// The phrase's voicing contour, in semitones from the tonic.
        curve: Curve,
        cursor: usize,
    },
}

impl HarmonySource {
    /// Chord slots left in the plan (None for the cycle — it never
    /// depletes). The replan margin reads this.
    pub fn slots_remaining(&self) -> Option<usize> {
        match self {
            HarmonySource::Cycle => None,
            HarmonySource::Plan { slots, cursor, .. } => {
                Some(slots.len().saturating_sub(*cursor))
            }
        }
    }

    /// The vocabulary index of the slot about to perform — the history and
    /// the composer's context speak the same menu the plans do.
    pub fn current_vocab(&self, slot: u32) -> usize {
        match self {
            HarmonySource::Cycle => CYCLE_VOCAB[(slot as usize) % CYCLE_LEN],
            HarmonySource::Plan { slots, cursor, .. } => slots
                .get(*cursor)
                .map(|slot| slot.chord as usize)
                .unwrap_or_else(|| CYCLE_VOCAB[(slot as usize) % CYCLE_LEN]),
        }
    }
}

/// The key the harmony lives in: a semitone offset from home (D). The
/// form controller moves it (sequences, episodes); every voice resolves
/// relative to it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct KeyState {
    pub tonic: i32,
}

impl KeyState {
    /// The key's name for the composer's prompt — the tonic's spelling,
    /// minor-preferred (the harmony is a minor mixture).
    pub fn name(self) -> &'static str {
        const NAMES: [&str; 12] = [
            "D minor",
            "E♭ minor",
            "E minor",
            "F minor",
            "F♯ minor",
            "G minor",
            "G♯ minor",
            "A minor",
            "B♭ minor",
            "B minor",
            "C minor",
            "C♯ minor",
        ];
        NAMES[self.tonic.rem_euclid(12) as usize]
    }
}

/// One slot's resolved chord: the root's scale degree, its quality, and the
/// bass note that sounds beneath it.
#[derive(Debug, Clone, Copy, PartialEq)]
struct SlotChord {
    degree: f64,
    quality: Quality,
    bass: u8,
}

/// The cycle's chord for a slot — the harmony and its locked bass.
fn cycle_chord(slot: u32, tonic: i32, degraded: bool) -> SlotChord {
    let (degree, quality) = HARMONY_CYCLE[(slot as usize) % CYCLE_LEN];
    let (bass_degree, octave) = BASS_CYCLE[(slot as usize) % CYCLE_LEN];
    SlotChord {
        degree,
        quality,
        bass: bass_for_line(bass_degree, octave, tonic, degraded),
    }
}

/// Resolve the slot's chord from the harmony source: the plan's next slot
/// when live (advancing its cursor), else the deterministic cycle. A
/// depleted plan hands back to the cycle first.
fn resolve_slot(state: &mut MusicState) -> SlotChord {
    let depleted = match &state.harmony {
        HarmonySource::Plan { slots, cursor, .. } => *cursor >= slots.len(),
        HarmonySource::Cycle => false,
    };
    if depleted {
        state.harmony = HarmonySource::Cycle;
    }
    // A live plan performs its next slot — advancing the cursor — with the
    // bass on the root unless the slot names an inversion (a mid-phrase
    // chord tone); the key transposes and the degradation reaches it like
    // every voice.
    if let HarmonySource::Plan { slots, cursor, .. } = &mut state.harmony {
        let planned = &slots[*cursor];
        let entry = &VOCABULARY[planned.chord as usize];
        let chord = SlotChord {
            degree: entry.degree,
            quality: entry.quality,
            bass: bass_for_tone(
                entry.degree,
                entry.quality,
                planned.bass,
                state.key.tonic,
                state.degraded,
            ),
        };
        *cursor += 1;
        return chord;
    }
    cycle_chord(state.slot, state.key.tonic, state.degraded)
}

/// The pad voicing for a resolved chord: the pitch stack turns the root
/// degree (plus the key's offset) into semitones, and the quality stacks
/// intervals above it — each tone mapped through the chromatic layer into
/// the pad's octave (mutated when degraded).
fn voicing_for(degree: f64, quality: Quality, tonic: i32, degraded: bool) -> Vec<u8> {
    let root_semitone = pitch_stack().step(degree) + tonic as f64;
    let root_pc = root_semitone.rem_euclid(12.0);
    quality
        .intervals()
        .iter()
        .map(|&interval| (PAD_BASE + chromatic_map(root_pc + interval, degraded)) as u8)
        .collect()
}

/// Revoice a semitone-from-tonic into the arrangement space: a tone
/// already inside the band stays put, one outside shifts by the smallest
/// octave count that lands it within. The inversion falls out of the
/// placement — whichever tone lands lowest, sounds lowest.
fn revoice(semitone: f64, low: f64, high: f64) -> f64 {
    let (low, high) = if low <= high { (low, high) } else { (high, low) };
    if (low..=high).contains(&semitone) {
        return semitone;
    }
    let octaves = if semitone < low {
        ((low - semitone) / 12.0).ceil() // the smallest lift that enters
    } else {
        -((semitone - high) / 12.0).ceil() // the smallest drop that enters
    };
    semitone + 12.0 * octaves
}

/// Which chord voices sound this slot — the arrangement's decision.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Arrangement {
    pub distant: bool,
    pub bell: bool,
    pub airy: bool,
}

/// The arrangement for a slot: the drive layers the voices up — one at
/// rest, two past a third of full flight (the floor plus the airy), three
/// in combat. When sparse, the soloist rotates (two slots each) so calm
/// exploration never sits still; the bell only joins the rotation once
/// some ground is made (progress past 0.4 — the far outbounds alternate
/// distant and airy), and the arrival lift (progress past 0.85) adds the
/// airy to even a sparse slot: the goal's glow, audible from afar.
pub fn arrangement(intensity: f32, progress: f32, slot: u32) -> Arrangement {
    if intensity >= 0.66 {
        return Arrangement { distant: true, bell: true, airy: true };
    }
    if intensity >= 0.33 {
        return Arrangement { distant: true, bell: false, airy: true };
    }
    let mut sparse = match (slot / 2) % if progress >= 0.4 { 3 } else { 2 } {
        0 => Arrangement { distant: true, bell: false, airy: false },
        1 => Arrangement { distant: false, bell: false, airy: true },
        _ => Arrangement { distant: false, bell: true, airy: false },
    };
    if progress >= ARRIVAL_LIFT_PROGRESS {
        sparse.airy = true;
    }
    sparse
}

/// A set of sounding tones: (channel, note) pairs.
pub type ToneSet = Vec<(u8, u8)>;

/// The common-tone split between the previous slot's sustained voices and
/// the next's: tones still sounding are HELD (not re-played — the bridge
/// drops a re-play while a key is down), tones that don't persist get
/// their note-offs at the boundary. The bass pulses and the patterns stay
/// percussive — the daemon only feeds the sustained voices through here.
pub fn sustain_split(previous: &ToneSet, next: &ToneSet) -> (ToneSet, ToneSet) {
    let held = next
        .iter()
        .filter(|tone| previous.contains(tone))
        .copied()
        .collect();
    let off = previous
        .iter()
        .filter(|tone| !next.contains(tone))
        .copied()
        .collect();
    (held, off)
}

/// Bend a pattern note into the contour — "the arp exists below the
/// curve": the note clamps inside the bounds at its phrase parameter. The
/// bounds are semitones from the tonic; notes are absolute MIDI.
fn contour_clamp(note: u8, curve: &Curve, t: f64) -> u8 {
    let (low, high) = curve.contour(t);
    let semitone = f64::from(note) - PAD_BASE;
    (semitone.clamp(low, high) + PAD_BASE).round() as u8
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
    /// The player stands in a degraded data zone — the harmony rotates.
    pub degraded: bool,
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
    /// Where each slot's chord comes from — the cycle, or the composer's
    /// live plan.
    pub harmony: HarmonySource,
    /// The key the harmony lives in — the form controller's tonic offset.
    pub key: KeyState,
    /// The player's slow-field progress toward the exit (0 spawn, 1
    /// goal) — the arrangement's geography.
    pub objective: f32,
}

impl Default for MusicState {
    fn default() -> Self {
        Self {
            aggro_locks: 0,
            degraded: false,
            intensity: 0.0,
            slot: 0,
            rhythm_intensity: 0.0,
            rhythm_hold: 0.0,
            additive_pos: 0,
            pattern_step: 0,
            last_slot_secs: 4.0 * CROTCHET_SECS,
            harmony: HarmonySource::Cycle,
            key: KeyState::default(),
            objective: 0.0,
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

        // The slot's chord: the plan's next slot when the composer is
        // live, else the deterministic cycle.
        let slot_chord = resolve_slot(state);

        // The phrase's contour: a plan supplies the curve; the cycle plays
        // the neutral default, whose bounds never clip today's material.
        // `t` walks the slot's own span within the phrase — the patterns'
        // fence.
        let (curve, phrase_t) = match &state.harmony {
            HarmonySource::Plan { curve, slots, cursor } => {
                let phrase_len = slots.len().max(1) as f64;
                let slot_index = cursor.saturating_sub(1) as f64;
                (
                    curve.clone(),
                    Box::new(move |within: f64| {
                        ((slot_index + within) / phrase_len).clamp(0.0, 1.0)
                    }) as Box<dyn Fn(f64) -> f64>,
                )
            }
            HarmonySource::Cycle => (
                default_curve(),
                Box::new(|_: f64| 0.5) as Box<dyn Fn(f64) -> f64>,
            ),
        };

        let mut notes = Vec::new();

        // Bass, locked to the harmony, pulsing in quavers: six and then
        // rest until the next change in the relaxed regimes (3+ beats per
        // chord); continuous through the short ones, so the busier harmonic
        // rhythms carry the bass drive with them.
        let slot_quavers = (duration_secs / QUAVER_SECS).round() as usize;
        let pulses = if !is_additive(state.rhythm_intensity)
            && harmonic_period_beats(state.rhythm_intensity) >= 3.0
        {
            BASS_PULSES.min(slot_quavers)
        } else {
            slot_quavers
        };
        for k in 0..pulses {
            notes.push((
                k as f64 * QUAVER_SECS,
                NoteEvent {
                    channel: CHANNEL_BASS,
                    note: slot_chord.bass,
                    velocity: 88,
                    duration_secs: QUAVER_SECS * BASS_PULSE_GATE,
                },
            ));
        }

        // The three voices: the arrangement decides who sounds this slot —
        // sparse and swapping at low drive (the objective colours the
        // choice), layered up when the fight needs it. Each voice is a thin
        // subset of the chord revoiced into its own register band, and they
        // enter rhythmically offset rather than in a block. The sustained
        // voices schedule generously — the daemon holds common tones
        // across the change and trims the rest with note-offs.
        let arrangement = arrangement(state.intensity, state.objective, state.slot);
        let voicing: Vec<u8> = voicing_for(
            slot_chord.degree,
            slot_chord.quality,
            state.key.tonic,
            state.degraded,
        );
        if arrangement.distant {
            for &tone in &voicing {
                let placed = revoice(f64::from(tone) - PAD_BASE, DISTANT_BAND.0, DISTANT_BAND.1);
                notes.push((
                    DISTANT_OFFSET,
                    NoteEvent {
                        channel: CHANNEL_DISTANT,
                        note: (PAD_BASE + placed) as u8,
                        velocity: 55,
                        duration_secs: duration_secs * SUSTAIN_SLOTS,
                    },
                ));
            }
        }
        if arrangement.airy {
            // Layered, the airy carries the upper structure only; as the
            // sparse soloist it carries the whole shape.
            let top = if arrangement.distant {
                voicing.len().saturating_sub(2)
            } else {
                0
            };
            for &tone in &voicing[top..] {
                let placed = revoice(f64::from(tone) - PAD_BASE, AIRY_BAND.0, AIRY_BAND.1);
                notes.push((
                    AIRY_OFFSET,
                    NoteEvent {
                        channel: CHANNEL_AIRY,
                        note: (PAD_BASE + placed) as u8,
                        velocity: 45,
                        duration_secs: duration_secs * SUSTAIN_SLOTS,
                    },
                ));
            }
        }
        if arrangement.bell {
            if state.intensity < 0.33 {
                // The idle bell: a colour instrument — its plinks draw
                // from the chord's third, fifth, added sixth and natural
                // seventh (transposed and degraded like every tone), over
                // one of four rhythmic patterns, a new colour each plink.
                let root_pc =
                    (pitch_stack().step(slot_chord.degree) + state.key.tonic as f64)
                        .rem_euclid(12.0);
                let intervals = slot_chord.quality.intervals();
                let pool = [
                    root_pc + intervals[1], // the third
                    root_pc + intervals[2], // the fifth
                    root_pc + 9.0,          // the added sixth
                    root_pc + 10.0,         // the natural seventh
                ];
                let pattern = BELL_PATTERNS[(state.slot as usize / 2) % BELL_PATTERNS.len()];
                for (plink, &step) in pattern.iter().enumerate() {
                    let colour = chromatic_map(
                        pool[(plink + state.slot as usize) % pool.len()],
                        state.degraded,
                    );
                    let placed = revoice(colour, BELL_BAND.0, BELL_BAND.1);
                    notes.push((
                        step as f64 * STEP_SECS,
                        NoteEvent {
                            channel: CHANNEL_BELL,
                            note: (PAD_BASE + placed) as u8,
                            velocity: 70,
                            duration_secs: BELL_SECS,
                        },
                    ));
                }
            } else {
                // In combat the bell returns to the single topmost plink:
                // stability where the drive is.
                let topmost = voicing.last().expect("the chord has tones");
                let placed = revoice(f64::from(*topmost) - PAD_BASE, BELL_BAND.0, BELL_BAND.1);
                notes.push((
                    BELL_OFFSET,
                    NoteEvent {
                        channel: CHANNEL_BELL,
                        note: (PAD_BASE + placed) as u8,
                        velocity: 70,
                        duration_secs: BELL_SECS,
                    },
                ));
            }
        }

        // Patterns ride the global 16th grid across the slot's span —
        // odd-length additive slots land mid-figure and the figure carries.
        // A joins at one lock; B at three, on even steps only — an
        // eighth-note shadow under A's sixteenths.
        if state.aggro_locks >= 1 {
            let mask = density_mask(state.intensity);
            let steps = (duration_secs / STEP_SECS).round() as usize;
            for k in 0..steps {
                let global = state.pattern_step + k;
                if !mask[global % mask.len()] {
                    continue;
                }
                let at = k as f64 * STEP_SECS;
                // The patterns exist below the curve: each 16th's note
                // clamps into the contour at its phrase parameter.
                let t = phrase_t(k as f64 / steps.max(1) as f64);
                notes.push((
                    at,
                    NoteEvent {
                        channel: CHANNEL_PATTERN_A,
                        note: contour_clamp(
                            pattern_note(&voicing, &FIGURE_A, global, PATTERN_A_OCTAVE),
                            &curve,
                            t,
                        ),
                        velocity: 72,
                        duration_secs: 0.11,
                    },
                ));
                if state.aggro_locks >= 3 && global.is_multiple_of(2) {
                    notes.push((
                        at,
                        NoteEvent {
                            channel: CHANNEL_PATTERN_B,
                            note: contour_clamp(
                                pattern_note(&voicing, &FIGURE_B, global, PATTERN_B_OCTAVE),
                                &curve,
                                t,
                            ),
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

    /// Plan slots from chord indices, roots in the bass.
    fn plan_slots(chords: &[u32]) -> Vec<PlanSlot> {
        chords
            .iter()
            .map(|&chord| PlanSlot { chord, bass: None, suggested_beats: 0.0 })
            .collect()
    }

    /// The cycle's voicing for a slot — what the deterministic path plays.
    fn cycle_voicing(slot: u32, degraded: bool) -> Vec<u8> {
        let chord = cycle_chord(slot, 0, degraded);
        voicing_for(chord.degree, chord.quality, 0, degraded)
    }

    /// The bass notes a rendered slot sounded, in order.
    fn bass_notes(notes: &[(f64, NoteEvent)]) -> Vec<u8> {
        notes
            .iter()
            .filter(|(_, n)| n.channel == CHANNEL_BASS)
            .map(|(_, n)| n.note)
            .collect()
    }

    /// The single bass note a slot sounded — the pulses all strike it.
    fn slot_bass(notes: &[(f64, NoteEvent)]) -> u8 {
        let bass = bass_notes(notes);
        assert!(!bass.is_empty(), "the slot sounded a bass");
        assert!(
            bass.iter().all(|&note| note == bass[0]),
            "the slot's bass pulses agree: {bass:?}"
        );
        bass[0]
    }

    #[test]
    fn cycles_wrap_exactly() {
        for slot in 0..(CYCLE_LEN as u32 * 3) {
            let next = slot + CYCLE_LEN as u32;
            let chord = cycle_chord(slot, 0, false);
            let wrapped = cycle_chord(next, 0, false);
            assert_eq!(chord.bass, wrapped.bass);
            assert_eq!(
                voicing_for(chord.degree, chord.quality, 0, false),
                voicing_for(wrapped.degree, wrapped.quality, 0, false)
            );
        }
    }

    #[test]
    fn revoice_leaves_insiders_and_lifts_outsiders() {
        // A tone already in the space stays put; one below lifts an
        // octave in; one high folds down.
        assert_eq!(revoice(7.0, 0.0, 48.0), 7.0);
        assert_eq!(revoice(0.0, 12.0, 24.0), 12.0);
        assert_eq!(revoice(23.0, 0.0, 12.0), 11.0);
    }

    #[test]
    fn the_arrangement_layers_voices_with_the_drive() {
        // At rest: one voice. Mid-drive: the floor plus the airy. Full
        // flight: everything.
        assert_eq!(
            arrangement(0.0, 0.0, 0),
            Arrangement { distant: true, bell: false, airy: false }
        );
        assert_eq!(
            arrangement(0.5, 0.0, 0),
            Arrangement { distant: true, bell: false, airy: true }
        );
        assert_eq!(
            arrangement(1.0, 0.0, 0),
            Arrangement { distant: true, bell: true, airy: true }
        );
    }

    #[test]
    fn the_sparse_soloist_rotates_and_the_objective_colours_it() {
        // Far out (low progress): distant and airy alternate — the bell
        // waits for some ground made.
        assert_eq!(
            arrangement(0.0, 0.0, 0),
            Arrangement { distant: true, bell: false, airy: false }
        );
        assert_eq!(
            arrangement(0.0, 0.0, 2),
            Arrangement { distant: false, bell: false, airy: true }
        );
        assert_eq!(
            arrangement(0.0, 0.0, 4),
            Arrangement { distant: true, bell: false, airy: false }
        );
        // Past 0.4 progress the bell joins the rotation.
        assert_eq!(
            arrangement(0.0, 0.5, 4),
            Arrangement { distant: false, bell: true, airy: false }
        );
    }

    #[test]
    fn the_arrival_lift_adds_the_airy() {
        // Approaching the goal: even a sparse slot carries the airy — the
        // goal's glow, audible from afar.
        let lift = arrangement(0.0, 0.9, 0);
        assert!(lift.distant && lift.airy && !lift.bell);
    }

    #[test]
    fn the_voices_take_thin_subsets_in_their_own_bands() {
        // Full flight: all three sound — the distant holds the full shape
        // low, the airy the top two, the bell the topmost tone alone; the
        // offsets stagger their entries.
        let mut state = MusicState {
            aggro_locks: 6,
            intensity: 1.0,
            rhythm_intensity: 1.0,
            ..MusicState::default()
        };
        let (notes, _) = Engine::slot_notes(&mut state, 0.0);
        let distant: Vec<u8> = notes
            .iter()
            .filter(|(_, n)| n.channel == CHANNEL_DISTANT)
            .map(|(_, n)| n.note)
            .collect();
        let bell: Vec<&(f64, NoteEvent)> = notes
            .iter()
            .filter(|(_, n)| n.channel == CHANNEL_BELL)
            .collect();
        let airy: Vec<u8> = notes
            .iter()
            .filter(|(_, n)| n.channel == CHANNEL_AIRY)
            .map(|(_, n)| n.note)
            .collect();
        // Distant: the full shape in its band.
        assert_eq!(distant.len(), 3);
        for tone in &distant {
            let from_tonic = f64::from(*tone) - PAD_BASE;
            assert!((DISTANT_BAND.0..=DISTANT_BAND.1).contains(&from_tonic));
        }
        // Airy: the top two, in the airy band.
        assert_eq!(airy.len(), 2);
        for tone in &airy {
            let from_tonic = f64::from(*tone) - PAD_BASE;
            assert!((AIRY_BAND.0..=AIRY_BAND.1).contains(&from_tonic));
        }
        // Bell: a single tone, high and short, off the quaver grid.
        assert_eq!(bell.len(), 1);
        let (at, note) = bell[0];
        assert_secs(*at, BELL_OFFSET);
        assert_secs(note.duration_secs, BELL_SECS);
        let from_tonic = f64::from(note.note) - PAD_BASE;
        assert!((BELL_BAND.0..=BELL_BAND.1).contains(&from_tonic));
    }

    #[test]
    fn the_idle_bell_plays_the_colour_pool_over_patterns() {
        // Sparse, some ground made: slot 4 is the bell's rotation slot —
        // pattern index 2 (the three-plink scatter), the plinks drawing
        // from the chord's third, fifth, added sixth and natural seventh.
        let mut state = MusicState {
            objective: 0.5,
            ..MusicState::default()
        };
        state.slot = 4;
        let (notes, _) = Engine::slot_notes(&mut state, 0.0);
        let bell: Vec<(f64, NoteEvent)> = notes
            .iter()
            .filter(|(_, n)| n.channel == CHANNEL_BELL)
            .map(|(at, n)| (*at, n.clone()))
            .collect();
        // Pattern 2 = three plinks at 16th steps 2, 7 and 12; the tones
        // rotate through the colour pool. The cycle's slot 4 is the III
        // (F# major): its pool is the major third, the fifth, the added
        // sixth and the natural seventh — pcs 8, 11, 1 and 2 from the
        // tonic.
        assert_eq!(bell.len(), 3);
        for (k, (at, note)) in bell.iter().enumerate() {
            assert_secs(*at, BELL_PATTERNS[2][k] as f64 * STEP_SECS);
            let from_tonic = f64::from(note.note) - PAD_BASE;
            assert!(
                [8.0, 11.0, 1.0, 2.0].contains(&(from_tonic.rem_euclid(12.0))),
                "the plink is a pool colour: {}",
                note.note
            );
            assert_secs(note.duration_secs, BELL_SECS);
        }

        // The next bell slot walks to a different pattern (a pair).
        state.slot = 10;
        let (notes, _) = Engine::slot_notes(&mut state, 0.0);
        let bell_count = notes
            .iter()
            .filter(|(_, n)| n.channel == CHANNEL_BELL)
            .count();
        assert_eq!(bell_count, 2);
    }

    #[test]
    fn common_tones_held_and_strays_trimmed() {
        let previous = vec![
            (CHANNEL_DISTANT, 50),
            (CHANNEL_DISTANT, 57),
            (CHANNEL_AIRY, 69),
        ];
        let next = vec![
            (CHANNEL_DISTANT, 50),
            (CHANNEL_DISTANT, 57),
            (CHANNEL_AIRY, 67),
        ];
        let (held, off) = sustain_split(&previous, &next);
        // The common tones hold (no re-play); the stray gets trimmed.
        assert_eq!(held, vec![(CHANNEL_DISTANT, 50), (CHANNEL_DISTANT, 57)]);
        assert_eq!(off, vec![(CHANNEL_AIRY, 69)]);
    }

    #[test]
    fn transposition_moves_every_voice_by_the_offset() {
        // The same slot a tone higher: the voicing and the bass differ by
        // exactly two semitones — the whole dictionary travels with the
        // key.
        let home = cycle_chord(4, 0, false);
        let up = cycle_chord(4, 2, false);
        let home_voicing = voicing_for(home.degree, home.quality, 0, false);
        let up_voicing = voicing_for(up.degree, up.quality, 2, false);
        for (home_tone, up_tone) in home_voicing.iter().zip(up_voicing.iter()) {
            assert_eq!(up_tone - home_tone, 2);
        }
        assert_eq!(up.bass - home.bass, 2);
    }

    #[test]
    fn degradation_reaches_the_bass_and_survives_transposition() {
        // The bass degrades with the stack now: deterministic, within the
        // spread, and the rotation composes with a transposed key.
        let clean = cycle_chord(0, 0, false);
        let degraded = cycle_chord(0, 0, true);
        assert_ne!(degraded.bass, clean.bass, "the corrupted bass is heard");
        assert_eq!(cycle_chord(0, 0, true).bass, degraded.bass, "deterministic");
        let shift = (degraded.bass as i64 - clean.bass as i64).rem_euclid(12);
        let shift = shift.min(12 - shift);
        assert!(shift <= DEGRADATION_SEMITONES, "the bass moved {shift} semitones");

        // A transposed key degrades on its own pitch classes.
        let up_clean = cycle_chord(0, 2, false);
        let up_degraded = cycle_chord(0, 2, true);
        assert_ne!(up_degraded.bass, up_clean.bass);
        assert_ne!(up_degraded.bass, degraded.bass, "the key moves the rotation");
    }

    #[test]
    fn bass_is_locked_to_the_harmony() {
        // Every slot sounds its designated bass: the D under B/D, the E
        // under the E major prepare, the G# under the dim7.
        assert_eq!(cycle_chord(6, 0, false).bass, 38); // B/D sounds its D
        assert_eq!(cycle_chord(7, 0, false).bass, 40); // E
        assert_eq!(cycle_chord(8, 0, false).bass, 44); // G#
        assert_eq!(cycle_chord(0, 0, false).bass, 38); // home
    }

    #[test]
    fn e_major_prepares_the_dim7_which_resolves_home() {
        assert_eq!(cycle_voicing(7, false), vec![52, 56, 59]); // E3 G#3 B3
        assert_eq!(cycle_voicing(8, false), vec![56, 59, 62, 65]); // G#3 B3 D4 F4
        assert_eq!(cycle_voicing(0, false), vec![50, 53, 57]); // Dm, home again
    }

    #[test]
    fn a_plan_drives_the_slots_until_it_depletes() {
        // G → A → Dm from the vocabulary: the plan's bass sounds the chord
        // roots (the tonic bass plus each root's semitones), and the
        // depleted plan hands back to the cycle.
        let mut state = MusicState {
            harmony: HarmonySource::Plan {
                slots: plan_slots(&[9, 10, 0]),
                curve: default_curve(),
                cursor: 0,
            },
            ..MusicState::default()
        };
        let (notes, _) = Engine::slot_notes(&mut state, 0.0);
        assert_eq!(slot_bass(&notes), TONIC_BASS + 5); // G
        assert_eq!(state.harmony.slots_remaining(), Some(2));

        let (notes, _) = Engine::slot_notes(&mut state, 0.0);
        assert_eq!(slot_bass(&notes), TONIC_BASS + 7); // A
        assert_eq!(state.harmony.slots_remaining(), Some(1));

        let (notes, _) = Engine::slot_notes(&mut state, 0.0);
        assert_eq!(slot_bass(&notes), TONIC_BASS); // Dm, home
        // Depleted but not yet reverted — the remaining count of zero
        // keeps the daemon's replan trigger firing until a late plan
        // finally lands.
        assert_eq!(state.harmony.slots_remaining(), Some(0));

        // The next slot is the deterministic cycle's again.
        let (notes, _) = Engine::slot_notes(&mut state, 0.0);
        assert_eq!(state.harmony, HarmonySource::Cycle);
        assert_eq!(slot_bass(&notes), 35); // the cycle's B under B
        assert_eq!(state.slot, 4);
    }

    #[test]
    fn pattern_notes_bend_into_the_plan_s_contour() {
        // A plan curve pinched low: the patterns clamp inside it instead of
        // soaring above — the arp exists below the curve.
        let mut pinched = MusicState {
            aggro_locks: 6,
            intensity: 1.0,
            rhythm_intensity: 1.0,
            harmony: HarmonySource::Plan {
                slots: plan_slots(&[0, 0, 1, 0, 0, 1, 0, 0]),
                curve: Curve {
                    upper: vec![14.0, 14.0, 14.0, 14.0],
                    lower: vec![0.0, 0.0, 0.0, 0.0],
                },
                cursor: 0,
            },
            ..MusicState::default()
        };
        let (notes, _) = Engine::slot_notes(&mut pinched, 0.0);
        let highest = notes
            .iter()
            .filter(|(_, n)| n.channel == CHANNEL_PATTERN_A)
            .map(|(_, n)| n.note)
            .max()
            .expect("full flight plays pattern A");
        // The upper bound sits 14 semitones above the tonic: nothing climbs
        // past the pad base plus 14 (a whisker of rounding slack).
        assert!(f64::from(highest) <= PAD_BASE + 14.0 + 1.0, "clamped: {highest}");

        // The unpinched cycle would have soared — the clamp bit.
        let mut free = MusicState {
            aggro_locks: 6,
            intensity: 1.0,
            rhythm_intensity: 1.0,
            ..MusicState::default()
        };
        let (notes, _) = Engine::slot_notes(&mut free, 0.0);
        let unclamped = notes
            .iter()
            .filter(|(_, n)| n.channel == CHANNEL_PATTERN_A)
            .map(|(_, n)| n.note)
            .max()
            .unwrap();
        assert!(f64::from(unclamped) > PAD_BASE + 15.0);
    }

    #[test]
    fn degradation_rotates_the_harmony_deterministically() {
        // Every mapping mutates, so the degraded voicing differs from the
        // clean one — but the same way every time.
        let clean = cycle_voicing(0, false);
        let degraded = cycle_voicing(0, true);
        assert_ne!(degraded, clean, "the corruption is heard");
        assert_eq!(degraded, cycle_voicing(0, true), "deterministic");

        // Each tone lands within the rotation spread of its clean pitch
        // class (mod the octave), still inside the pad's register.
        for (dirty, &clean_tone) in degraded.iter().zip(clean.iter()) {
            let shift = (*dirty as i64 - clean_tone as i64).rem_euclid(12);
            let shift = shift.min(12 - shift); // fold to the signed distance
            assert!(
                shift <= DEGRADATION_SEMITONES,
                "tone moved {shift} semitones"
            );
            // The octave part carries through: the tone stays in the pad's
            // two-octave band.
            assert!(*dirty >= PAD_BASE as u8 && *dirty < PAD_BASE as u8 + 24);
        }

        // The mutation is per mapping operation: a different chord's
        // degradation scatters differently, and the patterns inherit it by
        // walking the rotated voicing.
        assert_ne!(cycle_voicing(7, true), cycle_voicing(7, false));
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
    fn a_calm_slot_is_bass_pulses_and_one_soloist_over_eight_crotchets() {
        let mut state = MusicState::default();
        let (notes, end) = Engine::slot_notes(&mut state, 0.0);
        // Six bass pulses, then the sparse soloist's full shape (the
        // distant pad, first in the rotation).
        assert_eq!(notes.len(), BASS_PULSES + 3);
        assert!(notes.iter().all(|(_, n)| n.channel <= CHANNEL_AIRY));
        assert_secs(end, 8.0 * CROTCHET_SECS);
        assert_eq!(state.slot, 1);
    }

    #[test]
    fn the_bass_pulses_six_then_rests_or_drives_continuously() {
        // Relaxed regime (eight crotchets): six quaver pulses on the slot's
        // designated bass, evenly spaced, gated short of a quaver, then
        // nothing until the next change.
        let mut calm = MusicState::default();
        let (notes, end) = Engine::slot_notes(&mut calm, 0.0);
        let bass: Vec<(f64, u8)> = notes
            .iter()
            .filter(|(_, n)| n.channel == CHANNEL_BASS)
            .map(|(at, n)| (*at, n.note))
            .collect();
        assert_eq!(bass.len(), BASS_PULSES);
        for (k, (at, note)) in bass.iter().enumerate() {
            assert_secs(*at, k as f64 * QUAVER_SECS);
            assert_eq!(*note, 38); // the D under Dm
        }
        for (_, n) in notes
            .iter()
            .filter(|(_, n)| n.channel == CHANNEL_BASS)
        {
            assert_secs(n.duration_secs, QUAVER_SECS * BASS_PULSE_GATE);
        }
        // The last pulse ends well before the change: the rest is heard.
        assert!(bass.last().unwrap().0 + QUAVER_SECS * BASS_PULSE_GATE < end);

        // Short regime (two crotchets): continuous quaver pulses — four
        // across the slot.
        let mut driving = MusicState {
            aggro_locks: 4,
            intensity: 0.8,
            rhythm_intensity: 0.8,
            ..MusicState::default()
        };
        let (notes, _) = Engine::slot_notes(&mut driving, 0.0);
        assert_eq!(
            notes
                .iter()
                .filter(|(_, n)| n.channel == CHANNEL_BASS)
                .count(),
            4
        );

        // Additive: each slot's quavers pulse one for one — three in a
        // 3-quaver slot, two in a 2-quaver slot.
        let mut additive = MusicState {
            aggro_locks: 6,
            intensity: 1.0,
            rhythm_intensity: 1.0,
            ..MusicState::default()
        };
        let (notes, _) = Engine::slot_notes(&mut additive, 0.0);
        assert_eq!(
            notes
                .iter()
                .filter(|(_, n)| n.channel == CHANNEL_BASS)
                .count(),
            3
        );
        // The next slot in the additive cycle is 2 quavers: two pulses.
        let (notes, _) = Engine::slot_notes(&mut additive, 0.0);
        assert_eq!(
            notes
                .iter()
                .filter(|(_, n)| n.channel == CHANNEL_BASS)
                .count(),
            2
        );
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
