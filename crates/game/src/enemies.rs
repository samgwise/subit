//! Enemy swarm: spawn the mob on tiles the player can actually reach, then
//! steer each enemy straight at the player and let the physics solver handle
//! wall and enemy-to-enemy collisions.

use std::time::Duration;

use avian2d::prelude::{
    Collider, CollisionLayers, LinearVelocity, LockedAxes, Position, RigidBody, SleepingDisabled,
};
use bevy::prelude::*;
use rand::RngExt;
use rand::SeedableRng;
use rand::rngs::SmallRng;
use wfc::{GeneratedMap, prototype_set, walkable_distances};

use crate::drops::spawn_drops;
use crate::world::{MapConfig, TILE_SIZE, tile_world_pos};

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

/// Initial throw stagger for the thrower at spawn index `index`: a strictly
/// positive slice of the cooldown so first throws don't fire in lockstep.
/// Never zero — `Timer::reset` restores the timer's own duration, so a
/// zero-length stagger would leave that thrower firing every frame forever.
fn throw_stagger_secs(index: usize) -> f32 {
    THROW_COOLDOWN_SECS * ((index % 4) as f32 + 1.0) / 4.0
}

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
/// Tank hit points; a serious soak even before a shield.
const TANK_HP: i32 = 400;

/// Chaser tint.
const CHASER_COLOUR: Color = Color::srgb(0.95, 0.2, 0.2);
/// Thrower tint — amber, distinct from the chaser at a glance.
const THROWER_COLOUR: Color = Color::srgb(1.0, 0.62, 0.15);
/// Tank tint — red-brown, distinct from chaser red and thrower amber.
const TANK_COLOUR: Color = Color::srgb(0.62, 0.26, 0.18);

/// Tank chase speed — half a chaser, an unhurried wall of muscle.
const TANK_SPEED: f32 = 60.0;
/// Tank collider footprint as a fraction of a tile.
const TANK_SIZE_TILES: f32 = 0.9;
/// Every Nth spawn is a tank, from depth 1 onward.
const TANK_EVERY: usize = 8;

/// Hit points soaked by one shield plate.
pub const PLATE_HP: i32 = 5;
/// Seconds a shield must go without damage before it regrows a plate.
pub const PLATE_REGEN_SECS: f32 = 2.5;
/// Plates on a freshly spawned enemy shield.
pub const ENEMY_SHIELD_PLATES: u32 = 8;

/// Sprite brightness for an enemy at `hp/max`: healthy is full, and damage
/// darkens the enemy so its state reads at a glance.
fn damage_tint(base: Color, hp: i32, max: i32) -> Color {
    let fraction = if max > 0 {
        hp.clamp(0, max) as f32 / max as f32
    } else {
        1.0
    };
    let factor = 0.45 + 0.55 * fraction;
    let c = base.to_srgba();
    Color::srgba(c.red * factor, c.green * factor, c.blue * factor, c.alpha)
}

/// Enemy hit points.
#[derive(Component, Debug)]
pub struct Health {
    pub hp: i32,
    pub max: i32,
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

/// Slow, tough bruiser: soaks cleaves that would delete lesser enemies.
#[derive(Component)]
pub struct Tank;

/// Per-entity chase speed in world units per second (chasers and tanks;
/// throwers' stand-off logic carries its own speed).
#[derive(Component)]
struct Speed(f32);

/// An enemy's undamaged tint, kept so the damage darkening re-derives from
/// the same colour every frame.
#[derive(Component)]
struct BaseColour(Color);

/// What an enemy does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Role {
    Chaser,
    Thrower,
    Tank,
}

/// Roles for a swarm: throwers and tanks take fixed quotas of the slots and
/// the list is shuffled so they scatter through the mob instead of
/// clustering at the end. Deterministic per (seed, depth) via the swarm RNG.
fn roles_for(count: usize, depth: u32, rng: &mut SmallRng) -> Vec<Role> {
    let throwers = count / thrower_every_for(depth);
    let tanks = if depth >= 1 { count / TANK_EVERY } else { 0 };
    let mut roles = vec![Role::Chaser; count];
    for role in roles.iter_mut().take(throwers) {
        *role = Role::Thrower;
    }
    for role in roles.iter_mut().skip(throwers).take(tanks) {
        *role = Role::Tank;
    }
    // Fisher–Yates with the same RNG primitive the site picker uses.
    for i in (1..roles.len()).rev() {
        roles.swap(i, rng.random_range(0..=i));
    }
    roles
}

/// Regenerating energy shield: a pool of plates that soak damage before
/// health, regrowing one plate at a time after a quiet spell.
#[derive(Component, Debug)]
pub struct Shield {
    pub plates: u32,
    pub regen: Timer,
}

/// Chance an enemy spawns shielded, rising with depth and capping.
fn shielded_chance(depth: u32) -> f32 {
    (0.05 + 0.05 * depth as f32).min(0.4)
}

/// Roll whether this spawn gets a shield.
fn shielded_roll(rng: &mut SmallRng, depth: u32) -> bool {
    rng.random::<f32>() < shielded_chance(depth)
}

/// Route `damage` through a plate pool: each plate soaks [`PLATE_HP`], and a
/// hit that cracks a plate consumes it whole. Returns the damage that gets
/// through to whatever sits behind the shield (and restarts the regen clock
/// whenever a plate is spent).
pub fn absorb_damage(plates: &mut u32, regen: &mut Timer, damage: i32) -> i32 {
    if damage <= 0 || *plates == 0 {
        return damage.max(0);
    }
    // Manual ceil for the positive damage this branch guarantees.
    let needed = ((damage + PLATE_HP - 1) / PLATE_HP) as u32;
    let spent = needed.min(*plates);
    *plates -= spent;
    regen.reset();
    // A hit lighter than a plate cracks it without leaking anything.
    (damage - spent as i32 * PLATE_HP).max(0)
}

/// Regrow one plate per [`PLATE_REGEN_SECS`] of uninterrupted quiet, up to
/// `max_plates`.
pub fn regrow_plate(plates: &mut u32, regen: &mut Timer, max_plates: u32, delta: Duration) {
    if *plates >= max_plates {
        return;
    }
    regen.tick(delta);
    if regen.is_finished() {
        *plates += 1;
        regen.reset();
    }
}

/// Per-thrower throw cooldown.
#[derive(Component, Debug)]
struct ThrowTimer(Timer);

/// A short-lived velocity kick layered on top of seek steering (nova
/// blasts); decays exponentially so steering takes back over.
#[derive(Component, Debug)]
pub struct Knockback {
    pub velocity: Vec2,
}

/// How fast the knockback kick decays (per-second exponential factor).
const KNOCKBACK_DECAY: f32 = 8.0;

/// Take the current knockback kick for this frame's velocity and decay it
/// in place. No component, no kick.
fn decay_knockback(knockback: Option<&mut Knockback>, delta_secs: f32) -> Vec2 {
    match knockback {
        Some(kb) => {
            let kick = kb.velocity;
            kb.velocity *= (-KNOCKBACK_DECAY * delta_secs).exp();
            kick
        }
        None => Vec2::ZERO,
    }
}

pub struct EnemyPlugin;

impl Plugin for EnemyPlugin {
    fn build(&self, app: &mut App) {
        // Spawning is driven by the world lifecycle (startup + descent), so
        // there is no Startup system here.
        app.add_systems(
            Update,
            (enemy_seek, thrower_seek, thrower_attack, shield_regen)
                .run_if(in_state(crate::skills::GameState::Playing)),
        );
    }
}

/// Spawn the swarm on far, BFS-reachable walkable tiles of a freshly built
/// map. Called by the world lifecycle for the first depth and on descent.
pub fn spawn_swarm(
    commands: &mut Commands,
    generated: &GeneratedMap,
    config: &MapConfig,
    depth: u32,
) {
    let (width, height) = (generated.grid.width(), generated.grid.height());
    let prototypes = prototype_set(
        config.generator.wall_weight,
        config.generator.terminal_weight,
    );
    let distances = walkable_distances(&generated.grid, &prototypes, generated.spawn);
    let candidates = far_reachable_cells(&distances, width, MIN_SPAWN_DISTANCE);
    let enemy_count = enemy_count_for(depth);

    // Deterministic per-seed placement: same seed and depth, same mob
    // layout. The salt keeps enemy placement uncorrelated with the
    // generator's own use of the seed, and the depth mixes in so deep maps
    // don't reuse earlier placement patterns.
    let swarm_seed = config.generator.seed
        ^ 0x5EBEE_51A7D
        ^ ((depth as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15));
    let mut rng = SmallRng::seed_from_u64(swarm_seed);
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

    let roles = roles_for(picked.len(), depth, &mut rng);
    for (i, (&cell, &role)) in picked.iter().zip(roles.iter()).enumerate() {
        let (hp, colour, size_tiles) = match role {
            Role::Chaser => (CHASER_HP, CHASER_COLOUR, ENEMY_SIZE_TILES),
            Role::Thrower => (THROWER_HP, THROWER_COLOUR, ENEMY_SIZE_TILES),
            Role::Tank => (TANK_HP, TANK_COLOUR, TANK_SIZE_TILES),
        };
        let pos = tile_world_pos((width, height), cell, config.tile_size);
        let size = config.tile_size * size_tiles;
        let mut enemy = commands.spawn((
            Enemy,
            Health { hp, max: hp },
            BaseColour(colour),
            Sprite::from_color(colour, Vec2::splat(size)),
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
        match role {
            Role::Chaser => {
                enemy.insert(Speed(ENEMY_SPEED));
            }
            Role::Tank => {
                enemy.insert(Tank).insert(Speed(TANK_SPEED));
            }
            Role::Thrower => {
                enemy
                    .insert(Thrower)
                    // Stagger the first throws so the mob does not fire in
                    // lockstep; deterministic per seed.
                    .insert(ThrowTimer(Timer::from_seconds(
                        throw_stagger_secs(i),
                        TimerMode::Once,
                    )));
            }
        }
        if shielded_roll(&mut rng, depth) {
            enemy.insert(Shield {
                plates: ENEMY_SHIELD_PLATES,
                regen: Timer::from_seconds(PLATE_REGEN_SECS, TimerMode::Once),
            });
        }
    }
    tracing::info!("spawned {} enemies", picked.len());
}

/// Steer chasers and tanks toward the player and show their damage state.
#[allow(clippy::type_complexity)] // Bevy query tuples read worse split up.
fn enemy_seek(
    time: Res<Time>,
    player: Single<&Position, With<crate::Player>>,
    mut enemies: Query<
        (
            &Position,
            &mut LinearVelocity,
            &Health,
            &Speed,
            &BaseColour,
            &mut Sprite,
            Option<&mut Knockback>,
        ),
        (With<Enemy>, Without<Thrower>),
    >,
) {
    let player_pos = player.0;
    for (pos, mut velocity, health, speed, base, mut sprite, mut knockback) in &mut enemies {
        let to_player = player_pos - pos.0;
        let seek = if to_player != Vec2::ZERO {
            to_player.normalize() * speed.0
        } else {
            Vec2::ZERO
        };
        let kick = decay_knockback(knockback.as_deref_mut(), time.delta_secs());
        velocity.0 = seek + kick;
        sprite.color = damage_tint(base.0, health.hp, health.max);
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

/// Throwers keep their distance instead of swarming, tinted by damage.
#[allow(clippy::type_complexity)]
fn thrower_seek(
    time: Res<Time>,
    player: Single<&Position, With<crate::Player>>,
    mut throwers: Query<
        (
            &Position,
            &mut LinearVelocity,
            &Health,
            &BaseColour,
            &mut Sprite,
            Option<&mut Knockback>,
        ),
        With<Thrower>,
    >,
) {
    let player_pos = player.0;
    for (pos, mut velocity, health, base, mut sprite, mut knockback) in &mut throwers {
        let seek = thrower_velocity(player_pos - pos.0);
        let kick = decay_knockback(knockback.as_deref_mut(), time.delta_secs());
        velocity.0 = seek + kick;
        sprite.color = damage_tint(base.0, health.hp, health.max);
    }
}

/// Regrow shield plates after a quiet spell.
fn shield_regen(time: Res<Time>, mut shields: Query<&mut Shield>) {
    for shield in &mut shields {
        let Shield { plates, regen } = shield.into_inner();
        regrow_plate(plates, regen, ENEMY_SHIELD_PLATES, time.delta());
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
    fn throw_stagger_never_zeros_the_cooldown() {
        for i in 0..64 {
            let secs = throw_stagger_secs(i);
            assert!(secs > 0.0, "index {i}: a zero stagger machineguns");
            assert!(secs <= THROW_COOLDOWN_SECS);
        }
        // Four distinct phases cycle rather than bunching at one value.
        let mut phases: Vec<f32> = (0..4).map(throw_stagger_secs).collect();
        phases.sort_by(|a, b| a.partial_cmp(b).unwrap());
        for pair in phases.windows(2) {
            assert_ne!(pair[0], pair[1]);
        }
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
    fn roles_fill_quotas_deterministically_and_add_tanks_with_depth() {
        let mut rng = SmallRng::seed_from_u64(7);
        let roles = roles_for(40, 0, &mut rng);
        assert_eq!(roles.len(), 40);
        assert_eq!(
            roles.iter().filter(|r| **r == Role::Thrower).count(),
            40 / 4
        );
        assert_eq!(roles.iter().filter(|r| **r == Role::Tank).count(), 0);
        // Same seed, same shuffle.
        let mut rng = SmallRng::seed_from_u64(7);
        assert_eq!(roles, roles_for(40, 0, &mut rng));
        // From depth 1 the swarm carries tanks too.
        let mut rng = SmallRng::seed_from_u64(7);
        let deep = roles_for(40, 1, &mut rng);
        assert_eq!(deep.iter().filter(|r| **r == Role::Tank).count(), 40 / 8);
    }

    #[test]
    fn shielded_chance_rises_with_depth_and_caps() {
        assert!((shielded_chance(0) - 0.05).abs() < 1e-6);
        assert!(shielded_chance(3) > shielded_chance(1));
        assert!((shielded_chance(100) - 0.4).abs() < 1e-6);
    }

    #[test]
    fn shields_absorb_in_plates_and_leak_the_rest() {
        let mut regen = Timer::from_seconds(PLATE_REGEN_SECS, TimerMode::Once);
        let mut plates = 8u32;
        // A light hit still costs a whole plate.
        assert_eq!(absorb_damage(&mut plates, &mut regen, 3), 0);
        assert_eq!(plates, 7);
        // A big hit soaks every plate and leaks the remainder.
        assert_eq!(
            absorb_damage(&mut plates, &mut regen, 100),
            100 - 7 * PLATE_HP
        );
        assert_eq!(plates, 0);
        // An empty shield passes everything through.
        assert_eq!(absorb_damage(&mut plates, &mut regen, 20), 20);
        assert_eq!(absorb_damage(&mut plates, &mut regen, 0), 0);
    }

    #[test]
    fn knockback_kicks_then_decays() {
        let mut kb = Some(Knockback {
            velocity: Vec2::new(500.0, 0.0),
        });
        let kick = decay_knockback(kb.as_mut(), 1.0 / 60.0);
        // The frame's velocity uses the undecayed kick...
        assert!((kick.x - 500.0).abs() < 1e-4);
        // ...and the stored kick decays so seek takes back over.
        assert!(kb.as_ref().unwrap().velocity.x < 500.0);
        // No component, no kick.
        assert_eq!(decay_knockback(None, 1.0 / 60.0), Vec2::ZERO);
    }

    #[test]
    fn spending_a_plate_restarts_the_regen_clock() {
        let mut plates = 2u32;
        let mut regen = Timer::from_seconds(PLATE_REGEN_SECS, TimerMode::Once);
        regen.tick(Duration::from_secs_f64(PLATE_REGEN_SECS as f64));
        absorb_damage(&mut plates, &mut regen, 5);
        assert_eq!(plates, 1);
        assert!(!regen.is_finished());
    }

    #[test]
    fn shields_regrow_one_plate_per_quiet_interval() {
        let mut plates = 0u32;
        let mut regen = Timer::from_seconds(PLATE_REGEN_SECS, TimerMode::Once);
        let delta = Duration::from_secs_f64(1.0);
        // Not enough quiet time yet.
        for _ in 0..2 {
            regrow_plate(&mut plates, &mut regen, 3, delta);
        }
        assert_eq!(plates, 0);
        // The third second regrows one plate and restarts the clock.
        regrow_plate(&mut plates, &mut regen, 3, delta);
        assert_eq!(plates, 1);
        // Full shields stop regrowing.
        for _ in 0..10 {
            regrow_plate(&mut plates, &mut regen, 3, delta);
        }
        assert_eq!(plates, 3);
    }

    #[test]
    fn spawned_swarm_carries_roles_speeds_and_shields() {
        let config = crate::world::MapConfig {
            generator: wfc::GeneratorConfig {
                width: 24,
                height: 24,
                seed: 99,
                ..Default::default()
            },
            tile_size: TILE_SIZE,
        };
        let generated = wfc::generate(&config.generator).expect("generation succeeds");
        let mut world = World::new();
        {
            let mut commands = world.commands();
            spawn_swarm(&mut commands, &generated, &config, 1);
        }
        world.flush();

        // Throwers carry no Speed (their stand-off logic is separate), so
        // query it optionally and account for every role.
        let mut enemies = world.query::<(&Enemy, Option<&Speed>, Option<&Tank>)>();
        let mut total = 0;
        let mut tanks = 0;
        let mut speeded = 0;
        for (_, speed, tank) in enemies.iter(&world) {
            total += 1;
            if tank.is_some() {
                tanks += 1;
                assert_eq!(speed.expect("tanks carry speed").0, TANK_SPEED);
            } else if let Some(speed) = speed {
                speeded += 1;
                assert_eq!(speed.0, ENEMY_SPEED);
            }
        }
        assert_eq!(total, enemy_count_for(1));
        // The tank quota is a slice of the actual spawn count; the chasers
        // and tanks carry Speed, throwers are the remaining quota.
        assert_eq!(tanks, total / TANK_EVERY);
        assert_eq!(total - tanks - speeded, total / thrower_every_for(1));

        // Every shield that rolled in starts with a full plate stack.
        let mut shields = world.query::<&Shield>();
        for shield in shields.iter(&world) {
            assert_eq!(shield.plates, ENEMY_SHIELD_PLATES);
        }
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
