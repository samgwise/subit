//! Enemy swarm: spawn the mob on tiles the player can actually reach, then
//! steer each enemy straight at the player and let the physics solver handle
//! wall and enemy-to-enemy collisions.

use avian2d::prelude::{
    Collider, CollisionLayers, LinearVelocity, LockedAxes, Position, RigidBody, SleepingDisabled,
};
use bevy::prelude::*;
use rand::RngExt;
use rand::SeedableRng;
use rand::rngs::SmallRng;
use wfc::walkable_distances;

use crate::drops::spawn_drops;
use crate::world::{MapConfig, TILE_SIZE, WorldMapRes, tile_world_pos};

/// How many enemies to seed the map with.
const ENEMY_COUNT: usize = 40;

/// Enemy chase speed in world units per second (half the player's speed so
/// kiting and escapes stay viable).
const ENEMY_SPEED: f32 = 120.0;

/// Enemies spawn at least this many BFS steps from the player spawn, so the
/// opening seconds are never an instant ambush.
const MIN_SPAWN_DISTANCE: u32 = 8;

/// Enemy collider footprint as a fraction of a tile.
const ENEMY_SIZE_TILES: f32 = 0.6;

/// Every Nth spawned enemy is a thrower.
const THROWER_EVERY: usize = 4;

/// Thrower chase speed (noticeably slower than the chasers).
const THROWER_SPEED: f32 = 70.0;

/// Throwers hold this distance from the player and lob from there.
const THROW_RANGE: f32 = TILE_SIZE * 6.0;

/// Seconds between throws.
const THROW_COOLDOWN_SECS: f32 = 2.0;

/// The mob never grows past this, however deep the run goes.
const ENEMY_COUNT_CAP: usize = 80;

/// Enemy count for a depth: +10 per layer, capped.
fn enemy_count_for(depth: u32) -> usize {
    (ENEMY_COUNT + 10 * depth as usize).min(ENEMY_COUNT_CAP)
}

/// Thrower frequency for a depth: every 4th spawn, tightening to every 3rd
/// from depth 2 down.
fn thrower_every_for(depth: u32) -> usize {
    if depth >= 2 { 3 } else { THROWER_EVERY }
}

/// Chaser hit points; the base cleave one-shots them.
const CHASER_HP: i32 = 100;
/// Thrower hit points; two base cleaves.
const THROWER_HP: i32 = 200;

/// Enemy hit points.
#[derive(Component, Debug)]
pub struct Health {
    pub hp: i32,
}

/// Kill an enemy: scatter its drops and despawn it. Combo and event batching
/// is the caller's concern (a cleave batches a whole swing).
pub fn kill_enemy(
    commands: &mut Commands,
    entity: Entity,
    position: Vec2,
    thrower: bool,
    rng: &mut SmallRng,
) {
    spawn_drops(commands, position, thrower, rng);
    if let Ok(mut entity_commands) = commands.get_entity(entity) {
        entity_commands.despawn();
    }
}

#[derive(Component)]
pub struct Enemy;

/// Ranged enemy: slower, holds at a distance and throws bouncing
/// projectiles.
#[derive(Component)]
pub struct Thrower;

/// Per-thrower throw cooldown.
#[derive(Component, Debug)]
struct ThrowTimer(Timer);

pub struct EnemyPlugin;

impl Plugin for EnemyPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Startup, spawn_enemies.after(crate::world::startup_world))
            .add_systems(
                Update,
                (enemy_seek, thrower_seek, thrower_attack)
                    .run_if(in_state(crate::skills::GameState::Playing)),
            );
    }
}

/// Spawn the swarm on far, BFS-reachable walkable tiles of the generated map.
fn spawn_enemies(
    mut commands: Commands,
    map: Res<WorldMapRes>,
    config: Res<MapConfig>,
    depth: Res<crate::world::Depth>,
) {
    let generated = &map.map;
    let (width, height) = (generated.grid.width(), generated.grid.height());
    let distances = walkable_distances(&generated.grid, &map.prototypes, generated.spawn);
    let candidates = far_reachable_cells(&distances, width, MIN_SPAWN_DISTANCE);
    let enemy_count = enemy_count_for(depth.0);
    let thrower_every = thrower_every_for(depth.0);

    // Deterministic per-seed placement: same seed, same mob layout. The salt
    // keeps enemy placement uncorrelated with the generator's own use of the
    // seed.
    let mut rng = SmallRng::seed_from_u64(config.generator.seed ^ 0x5EBEE_51A7D);
    let mut pool = candidates;
    let mut picked = Vec::with_capacity(enemy_count);
    while picked.len() < enemy_count && !pool.is_empty() {
        let i = rng.random_range(0..pool.len());
        picked.push(pool.swap_remove(i));
    }
    if picked.len() < enemy_count {
        tracing::warn!(
            "only {}/{} enemy spawn sites reachable at {}+ tiles from spawn",
            picked.len(),
            enemy_count,
            MIN_SPAWN_DISTANCE
        );
    }

    let size = config.tile_size * ENEMY_SIZE_TILES;
    for (i, &cell) in picked.iter().enumerate() {
        let pos = tile_world_pos((width, height), cell, config.tile_size);
        let thrower = i % thrower_every == thrower_every - 1;
        let mut enemy = commands.spawn((
            Enemy,
            Health {
                hp: if thrower { THROWER_HP } else { CHASER_HP },
            },
            Sprite::from_color(Color::srgb(0.95, 0.2, 0.2), Vec2::splat(size)),
            Transform::from_xyz(pos.x, pos.y, 1.0),
            RigidBody::Dynamic,
            Collider::rectangle(size, size),
            LockedAxes::ROTATION_LOCKED,
            SleepingDisabled,
            CollisionLayers::from_bits(
                crate::world::LAYER_ENEMY,
                crate::world::LAYER_WALL
                    | crate::world::LAYER_ENEMY
                    | crate::world::LAYER_PLAYER
                    | crate::world::LAYER_PLAYER_SHOT,
            ),
        ));
        if thrower {
            enemy
                .insert(Thrower)
                // Stagger the first throws so the mob does not fire in
                // lockstep; deterministic per seed.
                .insert(ThrowTimer(Timer::from_seconds(
                    THROW_COOLDOWN_SECS * ((i % thrower_every) as f32 / thrower_every as f32),
                    TimerMode::Once,
                )));
        }
    }
    tracing::info!("spawned {} enemies", picked.len());
}

/// Steer chasers toward the player; collision resolution does the rest.
#[allow(clippy::type_complexity)] // Bevy query tuples read worse split up.
fn enemy_seek(
    player: Single<&Position, With<crate::Player>>,
    mut enemies: Query<(&Position, &mut LinearVelocity), (With<Enemy>, Without<Thrower>)>,
) {
    let player_pos = player.0;
    for (pos, mut velocity) in &mut enemies {
        let to_player = player_pos - pos.0;
        velocity.0 = if to_player != Vec2::ZERO {
            to_player.normalize() * ENEMY_SPEED
        } else {
            Vec2::ZERO
        };
    }
}

/// Velocity for a thrower given the vector to the player: chase while
/// farther than throw range, then hold position.
fn thrower_velocity(to_player: Vec2) -> Vec2 {
    if to_player.length() > THROW_RANGE {
        to_player.normalize_or_zero() * THROWER_SPEED
    } else {
        Vec2::ZERO
    }
}

/// Throwers keep their distance instead of swarming.
fn thrower_seek(
    player: Single<&Position, With<crate::Player>>,
    mut throwers: Query<(&Position, &mut LinearVelocity), With<Thrower>>,
) {
    let player_pos = player.0;
    for (pos, mut velocity) in &mut throwers {
        velocity.0 = thrower_velocity(player_pos - pos.0);
    }
}

/// Lob a bouncing projectile at the player's current position on cooldown.
fn thrower_attack(
    mut commands: Commands,
    time: Res<Time>,
    player: Single<&Position, With<crate::Player>>,
    mut throwers: Query<(&Position, &mut ThrowTimer), With<Thrower>>,
    bridge: Res<crate::bridge::BridgeTx>,
) {
    let player_pos = player.0;
    for (pos, mut timer) in &mut throwers {
        timer.0.tick(time.delta());
        if !timer.0.is_finished() {
            continue;
        }
        let to_player = player_pos - pos.0;
        if to_player == Vec2::ZERO || to_player.length() > THROW_RANGE {
            continue;
        }
        timer.0.reset();
        crate::projectiles::spawn_projectile(
            &mut commands,
            pos.0,
            to_player.normalize_or_zero() * crate::projectiles::PROJECTILE_SPEED,
            crate::projectiles::ProjectileAllegiance::Enemy,
        );
        bridge.send(crate::bridge::GameAudioEvent::ProjectileThrow);
    }
}

/// Cells whose BFS distance from the spawn is at least `min_distance`, in
/// row-major order. Guarantees enemies start on tiles connected to the
/// player's region.
fn far_reachable_cells(
    distances: &[Option<u32>],
    width: u32,
    min_distance: u32,
) -> Vec<(u32, u32)> {
    distances
        .iter()
        .enumerate()
        .filter_map(|(i, d)| {
            if d.is_some_and(|dist| dist >= min_distance) {
                Some(((i as u32) % width, (i as u32) / width))
            } else {
                None
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn far_cells_exclude_the_spawn_and_walls() {
        // 3x3 grid, all cells reachable; spawn at the centre (distance 0),
        // corners at distance 2, edges at distance 1.
        let distances = vec![
            Some(2),
            Some(1),
            Some(2), //
            Some(1),
            Some(0),
            Some(1), //
            Some(2),
            Some(1),
            Some(2),
        ];
        let cells = far_reachable_cells(&distances, 3, 2);
        assert_eq!(cells, vec![(0, 0), (2, 0), (0, 2), (2, 2)]);
        // Minimum distance of 1 keeps everything but the spawn itself.
        let cells = far_reachable_cells(&distances, 3, 1);
        assert_eq!(cells.len(), 8);
        assert!(!cells.contains(&(1, 1)));
    }

    #[test]
    fn depth_scales_the_swarm() {
        assert_eq!(enemy_count_for(0), 40);
        assert_eq!(enemy_count_for(2), 60);
        assert_eq!(enemy_count_for(10), 80); // capped
        assert_eq!(thrower_every_for(0), 4);
        assert_eq!(thrower_every_for(2), 3);
        assert_eq!(thrower_every_for(9), 3);
    }

    #[test]
    fn throwers_chase_until_range_then_hold() {
        let far = Vec2::new(THROW_RANGE * 2.0, 0.0);
        let velocity = thrower_velocity(far);
        assert!(velocity.x > 0.0);
        assert!((velocity.length() - THROWER_SPEED).abs() < 1e-5);
        // Inside throw range: hold.
        assert_eq!(
            thrower_velocity(Vec2::new(THROW_RANGE * 0.5, 0.0)),
            Vec2::ZERO
        );
        // Boundary is exclusive: exactly at range means hold.
        assert_eq!(thrower_velocity(Vec2::new(THROW_RANGE, 0.0)), Vec2::ZERO);
    }

    #[test]
    fn unreachable_cells_never_become_spawn_sites() {
        let distances = vec![
            Some(9),
            None,
            Some(1),
            None,
            Some(0),
            None,
            Some(3),
            None,
            None,
        ];
        let cells = far_reachable_cells(&distances, 3, 2);
        assert_eq!(cells, vec![(0, 0), (0, 2)]);
    }
}
