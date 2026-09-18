//! Enemy swarm: spawn the mob on tiles the player can actually reach, then
//! steer each enemy straight at the player and let the physics solver handle
//! wall and enemy-to-enemy collisions.

use avian2d::prelude::{
    Collider, LinearVelocity, LockedAxes, Position, RigidBody, SleepingDisabled,
};
use bevy::prelude::*;
use rand::RngExt;
use rand::SeedableRng;
use rand::rngs::SmallRng;
use wfc::{prototype_set, walkable_distances};

use crate::world::{MapConfig, WorldMapRes, tile_world_pos};

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

#[derive(Component)]
pub struct Enemy;

pub struct EnemyPlugin;

impl Plugin for EnemyPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Startup, spawn_enemies.after(crate::world::generate_world))
            .add_systems(Update, enemy_seek);
    }
}

/// Spawn the swarm on far, BFS-reachable walkable tiles of the generated map.
fn spawn_enemies(mut commands: Commands, map: Res<WorldMapRes>, config: Res<MapConfig>) {
    let generated = &map.0;
    let (width, height) = (generated.grid.width(), generated.grid.height());
    let prototypes = prototype_set(
        config.generator.wall_weight,
        config.generator.terminal_weight,
    );
    let distances = walkable_distances(&generated.grid, &prototypes, generated.spawn);
    let candidates = far_reachable_cells(&distances, width, MIN_SPAWN_DISTANCE);

    // Deterministic per-seed placement: same seed, same mob layout. The salt
    // keeps enemy placement uncorrelated with the generator's own use of the
    // seed.
    let mut rng = SmallRng::seed_from_u64(config.generator.seed ^ 0x5EBEE_51A7D);
    let mut pool = candidates;
    let mut picked = Vec::with_capacity(ENEMY_COUNT);
    while picked.len() < ENEMY_COUNT && !pool.is_empty() {
        let i = rng.random_range(0..pool.len());
        picked.push(pool.swap_remove(i));
    }
    if picked.len() < ENEMY_COUNT {
        tracing::warn!(
            "only {}/{} enemy spawn sites reachable at {}+ tiles from spawn",
            picked.len(),
            ENEMY_COUNT,
            MIN_SPAWN_DISTANCE
        );
    }

    let size = config.tile_size * ENEMY_SIZE_TILES;
    for &cell in &picked {
        let pos = tile_world_pos((width, height), cell, config.tile_size);
        commands.spawn((
            Enemy,
            Sprite::from_color(Color::srgb(0.95, 0.2, 0.2), Vec2::splat(size)),
            Transform::from_xyz(pos.x, pos.y, 1.0),
            RigidBody::Dynamic,
            Collider::rectangle(size, size),
            LockedAxes::ROTATION_LOCKED,
            SleepingDisabled,
        ));
    }
    tracing::info!("spawned {} enemies", picked.len());
}

/// Steer every enemy toward the player; collision resolution does the rest.
fn enemy_seek(
    player: Single<&Position, With<crate::Player>>,
    mut enemies: Query<(&Position, &mut LinearVelocity), With<Enemy>>,
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
