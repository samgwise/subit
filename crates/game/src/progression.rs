//! Progression: XP from drops levels the player up and grants skill points
//! for the skills menu. The curve shape lives in one function so pacing can
//! be retuned in one place.

use bevy::prelude::*;

use crate::bridge::{BridgeTx, GameAudioEvent};

/// XP needed to go from `level` to `level + 1`.
pub fn xp_for_level(level: u32) -> u32 {
    XP_BASE + XP_GROWTH * level
}

/// XP for the first level-up.
const XP_BASE: u32 = 40;
/// Linear growth per level.
const XP_GROWTH: u32 = 30;

/// The player's experience: current level and progress toward the next.
#[derive(Resource, Debug)]
pub struct Experience {
    pub xp: u32,
    pub level: u32,
}

impl Default for Experience {
    fn default() -> Self {
        Self { xp: 0, level: 1 }
    }
}

/// Unspent points for the skills menu; one per level-up.
#[derive(Resource, Debug, Default)]
pub struct SkillPoints(pub u32);

/// Apply an XP gain: returns (remaining xp, new level, points gained),
/// cascading through as many level-ups as the amount covers.
fn apply_gain(xp: u32, level: u32, amount: u32) -> (u32, u32, u32) {
    let mut xp = xp + amount;
    let mut level = level;
    let mut points = 0;
    while xp >= xp_for_level(level) {
        xp -= xp_for_level(level);
        level += 1;
        points += 1;
    }
    (xp, level, points)
}

pub struct ProgressionPlugin;

impl Plugin for ProgressionPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<Experience>()
            .init_resource::<SkillPoints>()
            .add_systems(
                Update,
                gain_level_ups.run_if(in_state(crate::skills::GameState::Playing)),
            );
    }
}

/// Convert banked XP into level-ups and skill points.
fn gain_level_ups(
    mut experience: ResMut<Experience>,
    mut points: ResMut<SkillPoints>,
    bridge: Res<BridgeTx>,
) {
    let (xp, level, gained) = apply_gain(experience.xp, experience.level, 0);
    if gained == 0 {
        return;
    }
    experience.xp = xp;
    experience.level = level;
    points.0 += gained;
    tracing::info!(level, points = points.0, "level up");
    for _ in 0..gained {
        bridge.send(GameAudioEvent::LevelUp);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_curve_is_monotone_and_linear() {
        for level in 1..30 {
            assert!(xp_for_level(level + 1) > xp_for_level(level));
        }
        assert_eq!(xp_for_level(1), XP_BASE + XP_GROWTH);
        assert_eq!(xp_for_level(3), XP_BASE + 3 * XP_GROWTH);
    }

    #[test]
    fn gains_cascade_through_multiple_level_ups() {
        // 200 XP at level 1: 70 for L2 (130 left), 100 for L3 (30 left).
        let (xp, level, points) = apply_gain(0, 1, 200);
        assert_eq!((xp, level, points), (30, 3, 2));
    }

    #[test]
    fn exactly_reaching_the_boundary_earns_the_level() {
        let (xp, level, points) = apply_gain(0, 1, xp_for_level(1));
        assert_eq!((xp, level, points), (0, 2, 1));
        // A sliver under the boundary earns nothing.
        let (xp, level, points) = apply_gain(0, 1, xp_for_level(1) - 1);
        assert_eq!((xp, level, points), (xp_for_level(1) - 1, 1, 0));
        // Zero is a no-op.
        let (xp, level, points) = apply_gain(10, 4, 0);
        assert_eq!((xp, level, points), (10, 4, 0));
    }
}
