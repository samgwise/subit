//! The harmonic grammar: the vocabulary, the plan the composer emits, and
//! the validator that keeps every plan inside the grammar.
//!
//! The vocabulary is a numbered menu of chords — the deterministic cycle's
//! nine plus submarine's function palette (supertonic, subdominant,
//! dominant, dominant seventh) — each labelled with its harmonic function,
//! which is what the cadence rules and the composer's prompt speak. A
//! [`HarmonyPlan`] is a phrase: a sequence of vocabulary picks plus optional
//! contour control points. [`validate`] is the gate: anything outside the
//! grammar falls back to the deterministic cycle, logged.

use serde::Deserialize;
use scalevec::Scale;

/// Chord quality: the intervals (semitones from the root) its voicing stacks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Quality {
    Minor,
    Major,
    /// C# minor carrying the 4–3 suspension colour: the suspended fourth sits
    /// beside the third until a voice resolves it.
    MinorSus43,
    Diminished7,
    Dominant7,
}

impl Quality {
    pub fn intervals(self) -> &'static [f64] {
        match self {
            Quality::Minor => &[0.0, 3.0, 7.0],
            Quality::Major => &[0.0, 4.0, 7.0],
            Quality::MinorSus43 => &[0.0, 3.0, 4.0, 7.0],
            Quality::Diminished7 => &[0.0, 3.0, 6.0, 9.0],
            Quality::Dominant7 => &[0.0, 4.0, 7.0, 10.0],
        }
    }

    /// The quality's index in the composer's menu — the plan's JSON speaks
    /// integers, not names.
    pub fn index(self) -> u32 {
        match self {
            Quality::Minor => 0,
            Quality::Major => 1,
            Quality::MinorSus43 => 2,
            Quality::Diminished7 => 3,
            Quality::Dominant7 => 4,
        }
    }
}

/// A chord's harmonic function — the grammar's grammar: cadences resolve
/// through these, and the composer's prompt reasons in these terms.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Function {
    Tonic,
    Subdominant,
    Dominant,
    Colour,
}

impl Function {
    /// The function's name — the vocabulary the composer's prompt speaks.
    pub fn name(self) -> &'static str {
        match self {
            Function::Tonic => "tonic",
            Function::Subdominant => "subdominant",
            Function::Dominant => "dominant",
            Function::Colour => "colour",
        }
    }
}

/// One chord in the vocabulary: a scale-layer degree, its quality, and the
/// harmonic function it serves.
pub struct VocabEntry {
    pub degree: f64,
    pub quality: Quality,
    pub function: Function,
    /// The name the prompt shows the composer.
    pub name: &'static str,
}

/// The chord menu, in menu order (the plan's JSON picks by index). The
/// first nine are the deterministic cycle; the rest are submarine's
/// function palette over the D mixture.
pub const VOCABULARY: &[VocabEntry] = &[
    VocabEntry { degree: 0.0, quality: Quality::Minor, function: Function::Tonic, name: "Dm (tonic minor)" },
    VocabEntry { degree: 0.0, quality: Quality::Major, function: Function::Tonic, name: "D (tonic)" },
    VocabEntry { degree: 7.0, quality: Quality::Minor, function: Function::Colour, name: "Bm (submediant)" },
    VocabEntry { degree: 7.0, quality: Quality::Major, function: Function::Colour, name: "B (borrowed colour)" },
    VocabEntry { degree: 3.0, quality: Quality::Major, function: Function::Dominant, name: "F# (V of Bm)" },
    VocabEntry { degree: 9.0, quality: Quality::MinorSus43, function: Function::Colour, name: "C#m4-3 (suspension)" },
    VocabEntry { degree: 1.0, quality: Quality::Major, function: Function::Dominant, name: "E (V of V, prepares the dim7)" },
    VocabEntry { degree: 5.0, quality: Quality::Diminished7, function: Function::Dominant, name: "G#dim7 (resolves home)" },
    VocabEntry { degree: 1.0, quality: Quality::Minor, function: Function::Subdominant, name: "Em (supertonic)" },
    VocabEntry { degree: 4.0, quality: Quality::Major, function: Function::Subdominant, name: "G (subdominant)" },
    VocabEntry { degree: 6.0, quality: Quality::Major, function: Function::Dominant, name: "A (dominant)" },
    VocabEntry { degree: 6.0, quality: Quality::Dominant7, function: Function::Dominant, name: "A7 (dominant seventh)" },
    VocabEntry { degree: 5.0, quality: Quality::Minor, function: Function::Colour, name: "Gm (minor colour)" },
    // The objective's palette: colours for the run's dramatic arc — hope
    // far out, mixo motion on the approach, soft tension, and the tonic's
    // own dominant seventh (the second closable dominant).
    VocabEntry { degree: 2.0, quality: Quality::Major, function: Function::Colour, name: "F (the borrowed third)" },
    VocabEntry { degree: 8.0, quality: Quality::Major, function: Function::Colour, name: "C (the natural seventh)" },
    VocabEntry { degree: 6.0, quality: Quality::Minor, function: Function::Colour, name: "Am (the modal v)" },
    VocabEntry { degree: 0.0, quality: Quality::Dominant7, function: Function::Dominant, name: "D7 (the pull to the subdominant)" },
];

/// One planned chord: a vocabulary index and an advisory duration. The
/// conductor owns the clock (the M25 regime ladder, cuts, holds) — the
/// suggestion is logged and ignored by the engine in v1, kept in the schema
/// for a later hand-off.
#[derive(Debug, Clone, Copy, PartialEq, Deserialize)]
pub struct PlanSlot {
    pub chord: u32,
    #[serde(default)]
    pub suggested_beats: f32,
}

/// The composer's plan for the next phrase: chord slots, optional contour
/// control points, and the intent it acted on. Deliberately flat — it
/// doubles as the ollama response schema, and grammar-constrained sampling
/// misbehaves on nested or alternative schemas.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct HarmonyPlan {
    pub slots: Vec<PlanSlot>,
    #[serde(default)]
    pub curve_upper: Vec<f64>,
    #[serde(default)]
    pub curve_lower: Vec<f64>,
    #[serde(default)]
    pub intent: String,
}

/// The most chords a plan may hold — one long phrase, never a movement.
pub const PLAN_MAX_SLOTS: usize = 16;

/// The furthest two consecutive roots may sit, in semitones — leaps beyond
/// this read as a teleport, not a voice-lead.
const MAX_VOICE_LEAP: f64 = 5.0;

/// Validate a plan against the grammar. `prev_chord` is the sounding
/// vocabulary index (for the boundary voice-leap check); `None` when the
/// plan opens the piece. Rejected plans fall back to the cycle.
pub fn validate(plan: &HarmonyPlan, prev_chord: Option<usize>) -> Result<(), String> {
    if plan.slots.is_empty() {
        return Err("no slots".into());
    }
    if plan.slots.len() > PLAN_MAX_SLOTS {
        return Err(format!("too many slots ({})", plan.slots.len()));
    }
    for slot in &plan.slots {
        if slot.chord as usize >= VOCABULARY.len() {
            return Err(format!("chord {} is off the menu", slot.chord));
        }
    }

    // Voice-leading: consecutive roots stay close (including across the
    // boundary from the sounding chord). The distance folds around the
    // octave — a fifth down is the same hand position as a fourth up. The
    // dim7 is exempt as a source: it resolves by stepwise tones, not root
    // motion (the cycle's own cadence is a tritone apart).
    let leaps_well = |a: usize, b: usize| {
        use crate::music::pitch_stack;
        if VOCABULARY[a].quality == Quality::Diminished7 {
            return true;
        }
        let semitone =
            |i: usize| pitch_stack().step(VOCABULARY[i].degree).rem_euclid(12.0);
        let distance = (semitone(b) - semitone(a)).rem_euclid(12.0);
        distance.min(12.0 - distance) <= MAX_VOICE_LEAP
    };
    for pair in plan.slots.windows(2) {
        let (a, b) = (pair[0].chord as usize, pair[1].chord as usize);
        if !leaps_well(a, b) {
            return Err(format!("voice leap from {a} to {b} exceeds the span"));
        }
    }
    if let Some(prev) = prev_chord
        && !leaps_well(prev, plan.slots[0].chord as usize)
    {
        return Err("boundary voice leap exceeds the span".into());
    }

    // Restraint: the same chord at most twice in a row.
    let mut run = 1;
    for pair in plan.slots.windows(2) {
        run = if pair[0].chord == pair[1].chord { run + 1 } else { 1 };
        if run > 2 {
            return Err("more than two identical chords in a row".into());
        }
    }

    // Cadence: a phrase can't close on colour — it resolves home or hands
    // over to a dominant the next phrase answers.
    let closer = &VOCABULARY[plan.slots.last().unwrap().chord as usize].function;
    if !matches!(closer, Function::Tonic | Function::Dominant) {
        return Err("plan closes on a colour chord".into());
    }

    // Contour sanity: a few control points each, within a couple of octaves
    // of the register the instruments live in.
    for (label, points) in [("upper", &plan.curve_upper), ("lower", &plan.curve_lower)] {
        if points.is_empty() {
            continue;
        }
        if points.len() < 2 || points.len() > 8 {
            return Err(format!("curve {label} needs 2-8 control points"));
        }
        if points.iter().any(|v| !v.is_finite() || v.abs() > 48.0) {
            return Err(format!("curve {label} leaves the instrument's range"));
        }
    }

    if plan.intent.chars().count() > 200 {
        return Err("intent too long for the log".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plan_from(chords: &[u32]) -> HarmonyPlan {
        HarmonyPlan {
            slots: chords
                .iter()
                .map(|&chord| PlanSlot { chord, suggested_beats: 0.0 })
                .collect(),
            curve_upper: vec![],
            curve_lower: vec![],
            intent: String::new(),
        }
    }

    #[test]
    fn the_vocabulary_is_function_labelled() {
        assert_eq!(VOCABULARY[0].function, Function::Tonic);
        assert_eq!(VOCABULARY[6].name, "E (V of V, prepares the dim7)");
        assert_eq!(VOCABULARY[9].function, Function::Subdominant);
    }

    #[test]
    fn accepts_a_well_formed_plan() {
        // G (subdominant) → A (dominant) → Dm (resolve home).
        assert!(validate(&plan_from(&[9, 10, 0]), Some(1)).is_ok());
    }

    #[test]
    fn rejects_chords_off_the_menu() {
        let mut plan = plan_from(&[0, 1]);
        plan.slots[1].chord = VOCABULARY.len() as u32;
        assert!(validate(&plan, None).is_err());
    }

    #[test]
    fn rejects_empty_and_oversized_plans() {
        assert!(validate(&plan_from(&[]), None).is_err());
        let mut plan = plan_from(&[0, 1, 2, 0, 1, 2, 0, 1, 2, 0, 1, 2, 0, 1, 2, 0, 1]);
        assert!(validate(&plan, None).is_err());
        plan.slots.truncate(PLAN_MAX_SLOTS);
        assert!(validate(&plan, None).is_ok());
    }

    #[test]
    fn rejects_voice_leaps_beyond_the_span() {
        // Dm straight to the dim7: a tritone root leap — the cycle sends
        // E first to prepare it.
        assert!(validate(&plan_from(&[0, 7]), None).is_err());
        // G → A → Dm: a tone, then a fifth (a fourth folded) — fine.
        assert!(validate(&plan_from(&[9, 10, 0]), None).is_ok());
    }

    #[test]
    fn the_dim7_s_resolution_is_exempt_from_the_root_leap() {
        // The cycle's own cadence: the dim7 resolves home to Dm — the
        // voices move by step even though the roots sit a tritone apart.
        assert!(validate(&plan_from(&[7, 0, 1, 0]), Some(6)).is_ok());
        // Opening ON the dim7 straight after home is still the wild leap.
        assert!(validate(&plan_from(&[0, 7, 0, 1]), None).is_err());
    }

    #[test]
    fn rejects_more_than_two_repeats() {
        assert!(validate(&plan_from(&[0, 0, 0]), None).is_err());
        assert!(validate(&plan_from(&[0, 0, 1]), None).is_ok());
    }

    #[test]
    fn rejects_closing_on_colour() {
        assert!(validate(&plan_from(&[1, 2]), None).is_err());
        assert!(validate(&plan_from(&[2, 1]), None).is_ok()); // dominant close
        assert!(validate(&plan_from(&[1, 0]), None).is_ok()); // tonic close
    }

    #[test]
    fn rejects_curve_points_out_of_bounds() {
        let mut plan = plan_from(&[0, 1, 0]);
        plan.curve_upper = vec![0.0, 100.0];
        plan.curve_lower = vec![-12.0, -5.0];
        assert!(validate(&plan, None).is_err());
        plan.curve_upper = vec![0.0, 40.0, 30.0];
        assert!(validate(&plan, None).is_ok());
        // One point is not a curve.
        plan.curve_lower = vec![12.0];
        assert!(validate(&plan, None).is_err());
    }
}
