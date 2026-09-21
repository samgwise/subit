//! Enemy drops: fallen enemies leave XP shards (always) and sometimes an HP
//! cross. Pickups are plain sprites — no physics — that drift toward the
//! player inside a magnet radius and apply themselves on pickup.

use avian2d::prelude::Position;
use bevy::prelude::*;
use rand::RngExt;
use rand::SeedableRng;
use rand::rngs::SmallRng;

use crate::combat::MAX_HP;
use crate::progression::Experience;
use crate::world::TILE_SIZE;

/// XP from a chaser kill.
const XP_CHASER: u32 = 10;
/// XP from a thrower kill (tougher, worth more).
const XP_THROWER: u32 = 25;
/// Chance a kill also drops an HP cross.
const HEAL_CHANCE: f64 = 0.15;
/// HP restored by a cross.
const HEAL_AMOUNT: i32 = 25;

/// Drift toward the player inside this radius.
const MAGNET_RADIUS: f32 = TILE_SIZE * 2.0;
/// Pickup applies inside this radius.
const COLLECT_RADIUS: f32 = TILE_SIZE * 0.5;
/// Magnet drift speed in world units per second.
const PULL_SPEED: f32 = 220.0;

/// What a pickup does when collected.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PickupKind {
    Xp(u32),
    Heal(i32),
}

#[derive(Component, Debug)]
pub struct Pickup(pub PickupKind);

/// Shared RNG for drop rolls; seeded once so runs stay reproducible.
#[derive(Resource)]
pub struct DropRng(pub SmallRng);

/// The drops left by one dead enemy.
fn drop_kinds(rng: &mut SmallRng, thrower: bool) -> Vec<PickupKind> {
    let mut kinds = vec![PickupKind::Xp(if thrower { XP_THROWER } else { XP_CHASER })];
    if rng.random::<f64>() < HEAL_CHANCE {
        kinds.push(PickupKind::Heal(HEAL_AMOUNT));
    }
    kinds
}

pub struct DropsPlugin;

impl Plugin for DropsPlugin {
    fn build(&self, app: &mut App) {
        // Seeded deterministically (salted away from the other RNG uses) so
        // a whole run replays from the map seed.
        app.insert_resource(DropRng(SmallRng::seed_from_u64(0x600D_5EED)))
            .add_systems(
                Update,
                (pickup_drift, pickup_collect).run_if(in_state(crate::skills::GameState::Playing)),
            );
    }
}

/// Spawn the drops for a dead enemy around its death position.
pub fn spawn_drops(commands: &mut Commands, position: Vec2, thrower: bool, rng: &mut SmallRng) {
    for kind in drop_kinds(rng, thrower) {
        // Scatter the pickups slightly so multi-drops don't overlap.
        let angle = rng.random::<f32>() * std::f32::consts::TAU;
        let offset = Vec2::from_angle(angle) * rng.random_range(0.0..8.0);
        let (colour, size) = match kind {
            PickupKind::Xp(_) => (Color::srgba(0.4, 0.9, 1.0, 0.95), 10.0),
            PickupKind::Heal(_) => (Color::srgba(0.3, 1.0, 0.45, 0.95), 12.0),
        };
        commands.spawn((
            Pickup(kind),
            Sprite::from_color(colour, Vec2::splat(size)),
            Transform::from_xyz(position.x + offset.x, position.y + offset.y, 0.5),
        ));
    }
}

/// Drift pickups toward the player inside the magnet radius, pulsing
/// gently so they pop against the tiles.
fn pickup_drift(
    player: Single<&Position, With<crate::Player>>,
    time: Res<Time>,
    mut pickups: Query<(&mut Transform, &mut Sprite, &Pickup)>,
) {
    let player_pos = player.0;
    let elapsed = time.elapsed_secs();
    for (mut transform, mut sprite, _) in &mut pickups {
        let to_player = player_pos - transform.translation.xy();
        if to_player.length() <= MAGNET_RADIUS && to_player != Vec2::ZERO {
            transform.translation +=
                to_player.normalize().extend(0.0) * PULL_SPEED * time.delta_secs();
        }
        // Phase varies per pickup so they don't blink in unison.
        let phase = transform.translation.x * 0.07 + transform.translation.y * 0.05;
        sprite
            .color
            .set_alpha(0.6 + 0.35 * (elapsed * 4.0 + phase).sin());
    }
}

/// Apply and despawn pickups the player touches.
fn pickup_collect(
    mut commands: Commands,
    player: Single<&Position, With<crate::Player>>,
    mut vitals: ResMut<crate::combat::PlayerVitals>,
    mut experience: ResMut<Experience>,
    pickups: Query<(Entity, &Transform, &Pickup)>,
) {
    let player_pos = player.0;
    for (entity, transform, pickup) in &pickups {
        if player_pos.distance(transform.translation.xy()) > COLLECT_RADIUS {
            continue;
        }
        match pickup.0 {
            PickupKind::Xp(amount) => experience.xp += amount,
            PickupKind::Heal(amount) => {
                vitals.hp = (vitals.hp + amount).min(MAX_HP);
                tracing::info!(hp = vitals.hp, "healed by drop");
            }
        }
        commands.entity(entity).despawn();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_kill_drops_xp_and_sometimes_heals() {
        let mut rng = SmallRng::seed_from_u64(11);
        for thrower in [false, true] {
            let kinds = drop_kinds(&mut rng, thrower);
            assert!(!kinds.is_empty());
            // First drop is always the XP shard with the right value.
            assert_eq!(
                kinds[0],
                PickupKind::Xp(if thrower { XP_THROWER } else { XP_CHASER })
            );
            // Any extra drops are heals.
            for extra in &kinds[1..] {
                assert!(matches!(extra, PickupKind::Heal(HEAL_AMOUNT)));
            }
        }
    }

    #[test]
    fn drop_kinds_are_deterministic_for_a_seed() {
        let mut a = SmallRng::seed_from_u64(42);
        let mut b = SmallRng::seed_from_u64(42);
        for _ in 0..20 {
            assert_eq!(drop_kinds(&mut a, false), drop_kinds(&mut b, false));
        }
    }
}
