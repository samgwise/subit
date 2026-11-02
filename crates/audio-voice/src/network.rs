//! The chord network: the vocabulary as a voice-leading graph, after
//! ScaleVec::Chord::Graph and Tymoczko's voice-leading geometries.
//!
//! Each vocabulary chord is a node — its pitch-class set relative to
//! the tonic (key-invariant: the graph transposes with the form for
//! free) — and two chords are near when they share tones. The metric
//! [`difference`] is the coarse Tymoczko voice-leading distance: the
//! larger chord's size minus the tones it shares (the shared tones are
//! precisely the voices that need not move).
//!
//! [`realise_plan`] walks the network to realise a composer's plan: the
//! phrase's dynamic arc sets a movement target for each transition — a
//! settled phrase holds voices (maximal common tones), a climax moves
//! them — and the middle slots are re-picked to follow it. The plan's
//! first and last slots are anchors (the composer's cadence), any model
//! slot already inside its target stays, and a dead end degrades to the
//! model's own pick — the walk never fails and the validator still
//! gates the result.

use rand::rngs::SmallRng;
use rand::seq::SliceRandom;
use rand::RngExt;
use scalevec::Scale;

use crate::curve::Curve;
use crate::harmony::{
    voice_leap_is_fine, Function, HarmonyPlan, PlanSlot, VOCABULARY,
};
use crate::music::{curve_norm, pitch_stack};

/// A chord's pitch classes, relative to the tonic — the node's identity
/// in the voice-leading geometry (transposition-invariant).
pub(crate) fn chord_pitch_classes(index: usize) -> Vec<u8> {
    let entry = &VOCABULARY[index];
    let root = pitch_stack().step(entry.degree).rem_euclid(12.0);
    entry
        .quality
        .intervals()
        .iter()
        .map(|&interval| ((root + interval).rem_euclid(12.0)).round() as u8)
        .collect()
}

/// The chords' common-tone distance: the larger set's size minus the
/// tones it shares with the other — the shared tones are precisely the
/// voices that need not move. Zero means one set contains the other.
pub fn difference(a: &[u8], b: &[u8]) -> usize {
    let (larger, smaller) = if a.len() >= b.len() { (a, b) } else { (b, a) };
    let shared = smaller.iter().filter(|tone| larger.contains(tone)).count();
    larger.len() - shared
}

/// Every vocabulary chord sharing at least one pitch class with
/// `index` — the graph's neighbours (implicit: no adjacency structure
/// at this size).
pub(crate) fn neighbours(index: usize) -> impl Iterator<Item = usize> {
    let classes = chord_pitch_classes(index);
    (0..VOCABULARY.len()).filter(move |&other| {
        other != index
            && chord_pitch_classes(other)
                .iter()
                .any(|tone| classes.contains(tone))
    })
}

/// The movement targets the dynamic arc sets for each transition into
/// the phrase's slots — the arc's shape read as common-tone retention.
/// A third or less holds the voices (target 0 — in practice this keeps
/// the model's own stepwise craft, since no two distinct vocabulary
/// chords share every tone), a third to two thirds allows one moving
/// tone, past that two.
fn movement_targets(dynamic: &Curve, slots: usize) -> Vec<usize> {
    (0..slots)
        .map(|slot| match curve_norm(dynamic, slot as f64 / slots.max(1) as f64) {
            value if value <= 1.0 / 3.0 => 0,
            value if value <= 2.0 / 3.0 => 1,
            _ => 2,
        })
        .collect()
}

/// Whether a transition into `candidate` stays inside the grammar's
/// step rules: the voice-leap span, and no more than two of the same
/// chord in a row (the run counted into `prev`, `prev == next` adds one).
fn step_is_grammatical(prev: usize, prev_run: usize, candidate: usize) -> bool {
    voice_leap_is_fine(prev, candidate)
        && if prev == candidate { prev_run + 1 } else { 1 } <= 2
}

/// The candidates for a transition, best first: the model's own pick
/// when it fits the target, then the exact-target neighbours, then the
/// within-target ones — each shuffled so the walks vary.
fn candidates_for(
    plan: &HarmonyPlan,
    slot: usize,
    prev: usize,
    prev_run: usize,
    target: usize,
    rng: &mut SmallRng,
) -> Vec<PlanSlot> {
    let model = &plan.slots[slot];
    let model_fits =
        difference(&chord_pitch_classes(prev), &chord_pitch_classes(model.chord as usize)) <= target;
    let mut options: Vec<usize> = neighbours(prev)
        .filter(|&candidate| {
            difference(&chord_pitch_classes(prev), &chord_pitch_classes(candidate)) <= target
        })
        .collect();
    options.sort_by_key(|&candidate| {
        difference(&chord_pitch_classes(prev), &chord_pitch_classes(candidate))
    });
    options.shuffle(rng);

    let mut candidates = Vec::new();
    // The model's craft wins when it already moves at the curve's pace.
    if model_fits {
        candidates.push(*model);
    }
    let exact = options
        .iter()
        .copied()
        .filter(|&candidate| {
            step_is_grammatical(prev, prev_run, candidate)
                && difference(&chord_pitch_classes(prev), &chord_pitch_classes(candidate)) == target
        });
    let close = options
        .iter()
        .copied()
        .filter(|&candidate| step_is_grammatical(prev, prev_run, candidate));
    for candidate in exact.chain(close) {
        candidates.push(PlanSlot { chord: candidate as u32, suggested_beats: 0.0 });
        if candidates.len() >= 4 {
            break; // a handful is plenty — the walk backtracks above us
        }
    }
    candidates
}

/// Realise the composer's plan through the network: the first and last
/// slots are the composer's anchors, and each middle transition follows
/// the dynamic arc's movement target — the model's own slot kept when
/// it already fits, else a neighbour walked to fit (exact target
/// preferred). A phrase whose walk fails the grammar after a few tries
/// degrades to the model's plan untouched — the walk never fails.
pub fn realise_plan(plan: &HarmonyPlan, dynamic: &Curve, rng: &mut SmallRng) -> HarmonyPlan {
    if plan.slots.len() <= 2 {
        return plan.clone(); // anchors only — nothing to re-pick
    }
    let targets = movement_targets(dynamic, plan.slots.len());
    for _ in 0..3 {
        let mut realised = vec![plan.slots[0]];
        for (slot, &target) in targets.iter().enumerate().skip(1).take(plan.slots.len() - 2) {
            let previous = *realised.last().unwrap();
            let prev_index = previous.chord as usize;
            let prev_run = if realised.len() >= 2
                && realised[realised.len() - 2].chord == previous.chord
            {
                1
            } else {
                0
            };
            let candidates = candidates_for(plan, slot, prev_index, prev_run, target, rng);
            match candidates.first() {
                Some(candidate) => realised.push(*candidate),
                None => {
                    // A dead end: the model's own pick stands.
                    realised.push(plan.slots[slot]);
                }
            }
        }
        realised.push(plan.slots[plan.slots.len() - 1]);

        let outcome = HarmonyPlan {
            slots: realised,
            curve_upper: plan.curve_upper.clone(),
            curve_lower: plan.curve_lower.clone(),
            curve_dynamic: plan.curve_dynamic.clone(),
            intent: plan.intent.clone(),
        };
        // The grammar still gates the realisation — a walk that broke a
        // rule (the last-middle into the anchor, say) retries, then
        // degrades to the model's plan.
        if validate_walk(&outcome) {
            return outcome;
        }
    }
    plan.clone()
}

/// The realised plan must pass the plan grammar; the curve and intent
/// came from the already-validated plan, so only the slots change here.
fn validate_walk(plan: &HarmonyPlan) -> bool {
    crate::harmony::validate(plan, None).is_ok()
}

/// Walk the network for the fallback's next turn: from `current` to a
/// rolled cadence (a tonic or a dominant), the dynamic arc supplying
/// the movement profile — the same machinery the composer's plans use,
/// seeded by the form's RNG so the same seed walks the same turn.
/// Deterministic, grammatical, and anchored where it started.
pub fn walk_turn(current: usize, dynamic: &Curve, rng: &mut SmallRng) -> HarmonyPlan {
    // The cadence: a closable entry — tonic or dominant.
    let closable: Vec<usize> = (0..VOCABULARY.len())
        .filter(|&index| {
            matches!(
                VOCABULARY[index].function,
                Function::Tonic | Function::Dominant
            )
        })
        .collect();
    let cadence = closable[rng.random_range(0..closable.len())];
    // The skeleton: the current chord as the start anchor, the cadence
    // as the end, the middle filled with a loose grammatical ramble —
    // realise_plan re-picks the middle to the movement profile anyway.
    let length = rng.random_range(5..=9);
    let mut slots = vec![current];
    for _ in 1..length - 1 {
        let previous = *slots.last().unwrap();
        let step_options: Vec<usize> = neighbours(previous)
            .filter(|&candidate| {
                voice_leap_is_fine(previous, candidate)
                    && candidate != *slots.last().unwrap()
            })
            .collect();
        if step_options.is_empty() {
            break;
        }
        slots.push(step_options[rng.random_range(0..step_options.len())]);
    }
    slots.push(cadence);
    let skeleton = HarmonyPlan {
        slots: slots
            .iter()
            .map(|&chord| PlanSlot { chord: chord as u32, suggested_beats: 0.0 })
            .collect(),
        curve_upper: vec![],
        curve_lower: vec![],
        curve_dynamic: vec![],
        intent: String::new(),
    };
    realise_plan(&skeleton, dynamic, rng)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::curve::DynamicShape;
    use crate::harmony::validate;
    use crate::music::default_phrase_curve;
    use rand::SeedableRng;
    use rand::RngExt;

    fn plan_from(chords: &[u32]) -> HarmonyPlan {
        HarmonyPlan {
            slots: chords
                .iter()
                .map(|&chord| PlanSlot { chord, suggested_beats: 0.0 })
                .collect(),
            curve_upper: vec![],
            curve_lower: vec![],
            curve_dynamic: vec![],
            intent: String::new(),
        }
    }

    #[test]
    fn the_difference_counts_the_moving_voices() {
        // Identical sets keep every voice.
        assert_eq!(difference(&[0, 3, 7], &[0, 3, 7]), 0);
        // A subset: only the foreign tones move.
        assert_eq!(difference(&[0, 3, 7], &[0, 3]), 1);
        // Disjoint triads: all three voices move.
        assert_eq!(difference(&[0, 3, 7], &[1, 5, 8]), 3);
        // Triad against tetrad: the larger set's size sets the scale.
        assert_eq!(difference(&[0, 3, 7], &[0, 3, 6, 9]), 2);
    }

    #[test]
    fn the_vocabulary_neighbours_share_tones() {
        // The tonic minor and its dominant share a tone (the V's root is
        // the i's fifth); the tonic and the natural seventh share none.
        let tonic = chord_pitch_classes(0);
        assert!(neighbours(0).any(|other| other == 10)); // V shares the fifth
        assert!(!chord_pitch_classes(14).iter().any(|tone| tonic.contains(tone)));
        assert!(!neighbours(0).any(|other| other == 14));
    }

    #[test]
    fn the_movement_targets_climb_with_the_arc() {
        let climb = DynamicShape::Climb.curve();
        let targets = movement_targets(&climb, 6);
        assert_eq!(targets[0], 0);
        assert!(targets.windows(2).all(|pair| pair[1] >= pair[0]));
        assert_eq!(*targets.last().unwrap(), 2);
        // A high arc — the shape never settles — never drops to 0: the
        // harmony keeps moving through the phrase.
        let high = Curve { upper: vec![1.0, 0.9, 1.0], lower: vec![1.0, 0.9, 1.0] };
        assert!(movement_targets(&high, 5).iter().all(|&target| target >= 1));
    }

    #[test]
    fn the_walk_keeps_the_anchors_and_the_calm_craft() {
        // A settled arc (every middle target 1): each transition either
        // keeps no more than one moving tone or keeps the model's own
        // pick — the walk narrows, never overrides.
        let plan = plan_from(&[0, 9, 10, 1]);
        let settled = Curve { upper: vec![0.0, 0.0], lower: vec![0.0, 0.0] };
        let mut rng = SmallRng::seed_from_u64(7);
        let realised = realise_plan(&plan, &settled, &mut rng);
        assert_eq!(realised.slots.first().unwrap().chord, 0);
        assert_eq!(realised.slots.last().unwrap().chord, 1);
        assert!(validate(&realised, None).is_ok());
        let targets = movement_targets(&settled, 4);
        for (slot, pair) in realised.slots.windows(2).enumerate() {
            let moved = difference(
                &chord_pitch_classes(pair[0].chord as usize),
                &chord_pitch_classes(pair[1].chord as usize),
            );
            let kept = pair[1].chord == plan.slots[slot + 1].chord;
            assert!(
                moved <= targets[slot + 1] || kept,
                "transition {slot} moved {moved} past its target {}",
                targets[slot + 1]
            );
        }
    }

    #[test]
    fn anchors_only_plans_pass_through() {
        // Nothing to re-pick: the plan is returned as it arrived.
        let plan = plan_from(&[0, 1]);
        let mut rng = SmallRng::seed_from_u64(3);
        let realised = realise_plan(&plan, &DynamicShape::Climb.curve(), &mut rng);
        assert_eq!(
            realised.slots.iter().map(|slot| slot.chord).collect::<Vec<_>>(),
            vec![0, 1]
        );
    }

    #[test]
    fn a_climbing_arc_repicks_toward_the_climax() {
        // The climb's tail demands movement: the transitions into the
        // climax must keep no more common tones than their target —
        // unless the walk fell back to the model's pick.
        let plan = plan_from(&[0, 0, 1, 0]);
        let climb = DynamicShape::Climb.curve();
        let mut rng = SmallRng::seed_from_u64(11);
        let realised = realise_plan(&plan, &climb, &mut rng);

        assert_eq!(realised.slots.first().unwrap().chord, 0);
        assert_eq!(realised.slots.last().unwrap().chord, 0);
        assert!(validate(&realised, None).is_ok(), "the walk stays grammatical");
        let targets = movement_targets(&climb, 4);
        for (slot, pair) in realised.slots.windows(2).enumerate() {
            let moved = difference(
                &chord_pitch_classes(pair[0].chord as usize),
                &chord_pitch_classes(pair[1].chord as usize),
            );
            let kept = pair[1].chord == plan.slots[slot + 1].chord;
            assert!(
                moved <= targets[slot + 1] || kept,
                "transition {slot} moved {moved} past its target {}",
                targets[slot + 1]
            );
        }
    }

    #[test]
    fn the_fallback_walk_varies_and_cadences() {
        // Same seed, same walk; every walk starts where it was asked,
        // closes on a tonic or a dominant, and stays grammatical.
        for seed in 0..12u64 {
            let dynamic = DynamicShape::Swell.curve();
            let first = walk_turn(0, &dynamic, &mut SmallRng::seed_from_u64(seed));
            let again = walk_turn(0, &dynamic, &mut SmallRng::seed_from_u64(seed));
            assert_eq!(
                first.slots.iter().map(|slot| slot.chord).collect::<Vec<_>>(),
                again.slots.iter().map(|slot| slot.chord).collect::<Vec<_>>(),
                "seed {seed} walked differently twice"
            );
            assert_eq!(first.slots.first().unwrap().chord, 0);
            let closer = &VOCABULARY[first.slots.last().unwrap().chord as usize];
            assert!(
                matches!(closer.function, Function::Tonic | Function::Dominant),
                "seed {seed} cadenced on a colour"
            );
            assert!(validate(&first, None).is_ok(), "seed {seed} broke the grammar");
        }
    }

    #[test]
    fn every_realisation_stays_grammatical_and_anchored() {
        // Across shapes, starts and seeds: anchors preserved, grammar held.
        // (Every prefix closes on a tonic or a dominant — the inputs are
        // grammatical plans.)
        for seed in 0..24u64 {
            let mut rng = SmallRng::seed_from_u64(seed);
            let shape = [
                DynamicShape::Swell,
                DynamicShape::Dip,
                DynamicShape::Climb,
                DynamicShape::Fade,
            ][seed as usize % 4];
            let plan = plan_from(&[0, 1, 10, 1, 0][..2 + seed as usize % 4]);
            let realised = realise_plan(&plan, &shape.curve(), &mut rng);
            assert!(validate(&realised, None).is_ok(), "seed {seed} broke the grammar");
            assert_eq!(realised.slots.first().unwrap().chord, plan.slots[0].chord);
            assert_eq!(
                realised.slots.last().unwrap().chord,
                plan.slots.last().unwrap().chord
            );
        }
    }
}
