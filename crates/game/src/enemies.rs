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
use wfc::{FlowField, GeneratedMap, prototype_set, walkable_distances};

use crate::corruption::{CorruptedZones, zone_notice};
use crate::drops::spawn_drops;
use crate::world::{MapConfig, TILE_SIZE, WorldMapRes, tile_units, tile_world_pos};

/// How many enemies to seed the map with.
const ENEMY_COUNT: usize = 40;

/// Enemy chase speed in world units per second (half the player's speed so
/// kiting and escapes stay viable).
const ENEMY_SPEED: f32 = 120.0;

/// Enemies only notice the player within this many path steps (BFS cell
/// steps around the walls, not euclidean distance) — an enemy behind a wall
/// stays calm no matter how close it stands.
const AGGRO_RANGE_STEPS: u32 = 8;
/// Alerted enemies stand down only when the player escapes this many steps
/// farther — hysteresis so nobody flickers in and out of aggro at the
/// boundary.
const DEAGGRO_EXTRA_STEPS: u32 = 2;
/// Enemies spawn at least this many BFS steps from the player spawn —
/// beyond the stand-down range, so the opening frame cannot aggro anyone
/// and the hysteresis band starts fully calm. Derived from the aggro
/// constants, so retuning either can never re-create a boundary overlap.
const MIN_SPAWN_DISTANCE: u32 = AGGRO_RANGE_STEPS + DEAGGRO_EXTRA_STEPS;
/// Un-alerted enemies mill slowly around where they spawned.
const WANDER_SPEED: f32 = 30.0;
/// How far a wandering enemy may stray from its home before it turns back.
const WANDER_LEASH_TILES: f32 = 1.5;
/// Seconds between wander direction re-rolls (±0.5 s).
const WANDER_REPICK_SECS: f32 = 1.5;

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

/// Marker: this enemy has noticed the player and pursues.
#[derive(Component)]
pub struct Aggro;

/// Seconds a provoked enemy stays on the warpath after the blast.
pub const PROVOKED_SECS: f32 = 10.0;

/// Blast-sound radius: enemies within this euclidean distance of the
/// detonation are provoked — sound passes through walls.
pub const PROVOKE_RADIUS: f32 = TILE_SIZE * 10.0;

/// Provoked by a loud blast: forced pursuit for the duration, no stand-down
/// while it lasts — the normal hysteresis resumes when it lapses. Inserted
/// by the grenade detonation; a fresh blast re-inserts (refreshing) it.
#[derive(Component, Debug)]
pub struct Provoked(pub Timer);

/// Whether an enemy at `position` is within earshot of a blast at `centre`
/// — euclidean, because sound does not route around walls.
pub fn within_earshot(centre: Vec2, position: Vec2) -> bool {
    position.distance(centre) <= PROVOKE_RADIUS
}

/// Tick a provoked timer: `true` while the forced hunt lasts, `false` the
/// frame it lapses (the caller removes the component and falls back to the
/// normal aggro rules).
fn provoked_tick(timer: &mut Timer, delta: Duration) -> bool {
    timer.tick(delta);
    !timer.is_finished()
}

/// Home position and wander state for an un-alerted enemy's milling.
#[derive(Component, Debug)]
struct Wander {
    home: Vec2,
    dir: Vec2,
    repick: Timer,
}

/// Whether an enemy's aggro should flip at `steps` path steps from the
/// player (the BFS distance around the walls): `Some(true)` to alert,
/// `Some(false)` to stand down, `None` to hold — the band between the
/// notice range and the stand-down range is hysteresis. No path at all
/// means stand down: the player is somewhere the enemy can never reach.
/// `notice` is the range that counts — the cloak shrinks it (the sneak).
fn aggro_flip(aggroed: bool, steps: Option<u32>, notice: u32) -> Option<bool> {
    match (aggroed, steps) {
        (false, Some(steps)) if steps <= notice => Some(true),
        (true, Some(steps)) if steps > notice + DEAGGRO_EXTRA_STEPS => Some(false),
        (true, None) => Some(false),
        _ => None,
    }
}

/// The wandering velocity: drift in the current wander direction, unless
/// the leash pulls — beyond the stray radius from home, head home instead.
fn wander_velocity(home: Vec2, position: Vec2, dir: Vec2) -> Vec2 {
    if position.distance(home) > WANDER_LEASH_TILES * TILE_SIZE {
        (home - position).normalize_or_zero() * WANDER_SPEED
    } else {
        dir * WANDER_SPEED
    }
}

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
            (
                aggro_gate,
                wander_seek,
                enemy_seek,
                thrower_seek,
                thrower_attack,
                shield_regen,
            )
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
        let pos = tile_world_pos((width, height), cell, config.tile_size);
        spawn_enemy(commands, pos, role, i, depth, &mut rng, config.tile_size);
    }
    tracing::info!("spawned {} enemies", picked.len());
}

/// Spawn one enemy at `position` with its role's stats, milling around home
/// there, the depth's shield roll and (for throwers) the staggered first
/// throw of spawn index `index`.
fn spawn_enemy(
    commands: &mut Commands,
    position: Vec2,
    role: Role,
    index: usize,
    depth: u32,
    rng: &mut SmallRng,
    tile_size: f32,
) {
    let (hp, colour, size_tiles) = match role {
        Role::Chaser => (CHASER_HP, CHASER_COLOUR, ENEMY_SIZE_TILES),
        Role::Thrower => (THROWER_HP, THROWER_COLOUR, ENEMY_SIZE_TILES),
        Role::Tank => (TANK_HP, TANK_COLOUR, TANK_SIZE_TILES),
    };
    let size = tile_size * size_tiles;
    let mut enemy = commands.spawn((
        Enemy,
        Health { hp, max: hp },
        BaseColour(colour),
        Sprite::from_color(colour, Vec2::splat(size)),
        Transform::from_xyz(position.x, position.y, 1.0),
        // Un-alerted until the player comes close; the wander state
        // mills them around home while they wait.
        Wander {
            home: position,
            dir: Vec2::from_angle(rng.random_range(0.0..core::f32::consts::TAU)),
            repick: Timer::from_seconds(
                WANDER_REPICK_SECS + rng.random_range(-0.5..0.5),
                TimerMode::Once,
            ),
        },
        RigidBody::Dynamic,
        // Circles, not boxes: round agents glide around tile corners
        // instead of snagging on their vertices.
        Collider::circle(size * 0.5),
        LockedAxes::ROTATION_LOCKED,
        SleepingDisabled,
        CollisionLayers::from_bits(
            crate::world::LAYER_ENEMY,
            crate::world::LAYER_WALL
                | crate::world::LAYER_CRACKED_WALL
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
                    throw_stagger_secs(index),
                    TimerMode::Once,
                )));
        }
    }
    if shielded_roll(rng, depth) {
        enemy.insert(Shield {
            plates: ENEMY_SHIELD_PLATES,
            regen: Timer::from_seconds(PLATE_REGEN_SECS, TimerMode::Once),
        });
    }
}

/// Salt keeping the vault mob's rolls uncorrelated with the main swarm's.
const VAULT_MOB_SALT: u64 = 0x6A7_BA1E;

/// Spawn the vault mob inside the vault's interior: `vault_mob_for(depth)`
/// enemies on distinct interior cells. They stay calm behind the closed
/// door — the cells are BFS-unreachable from the player until it opens —
/// and mill around their cells waiting for the fight. Called by the world
/// build, so startup and descent get one alike.
pub fn spawn_vault_mob(
    commands: &mut Commands,
    vault: &crate::vault::Vault,
    map_size: (u32, u32),
    config: &MapConfig,
    depth: u32,
) {
    // Deterministic per (seed, depth), like the main swarm's placement.
    let seed = config.generator.seed
        ^ VAULT_MOB_SALT
        ^ ((depth as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15));
    let mut rng = SmallRng::seed_from_u64(seed);

    let count = crate::vault::vault_mob_for(depth).min(vault.interior.len());
    let mut pool = vault.interior.clone();
    let mut picked = Vec::with_capacity(count);
    while picked.len() < count && !pool.is_empty() {
        let i = rng.random_range(0..pool.len());
        picked.push(pool.swap_remove(i));
    }
    let roles = roles_for(picked.len(), depth, &mut rng);
    for (i, (&cell, &role)) in picked.iter().zip(roles.iter()).enumerate() {
        let pos = tile_world_pos(map_size, cell, config.tile_size);
        spawn_enemy(commands, pos, role, i, depth, &mut rng, config.tile_size);
    }
    tracing::info!("spawned {count} vault enemies");
}

/// Alert enemies that come within aggro range of the player — by path steps
/// around the walls, so enemies behind a nearby wall stay dormant — and
/// stand down the ones it escapes. Standing down re-homes the wanderer
/// where it lost the player, so it mills there instead of marching back to
/// its spawn.
#[allow(clippy::type_complexity)]
#[allow(clippy::too_many_arguments)]
fn aggro_gate(
    mut commands: Commands,
    time: Res<Time>,
    player: Single<&Position, With<crate::Player>>,
    map: Res<crate::world::WorldMapRes>,
    config: Res<MapConfig>,
    flow: Res<crate::world::PlayerFlow>,
    zones: Res<CorruptedZones>,
    cloak: Res<crate::skills::CloakState>,
    levels: Res<crate::skills::SkillLevels>,
    mut enemies: Query<
        (
            Entity,
            &Position,
            Has<Aggro>,
            &mut Wander,
            Option<&mut Provoked>,
        ),
        With<Enemy>,
    >,
) {
    let (width, height) = (map.map.grid.width(), map.map.grid.height());
    // A cloaked player is only noticed at the sneak range; standing in a
    // corrupted zone sheds two more steps off whatever range applies — the
    // two stack, floored at one by the zone helper.
    let player_units = tile_units((width, height), config.tile_size, player.0);
    let base_notice = if cloak.is_active() {
        crate::skills::cloak_sneak_steps(&levels)
    } else {
        AGGRO_RANGE_STEPS
    };
    let notice = zone_notice(
        base_notice,
        zones
            .0
            .contains(&(player_units.x.floor() as u32, player_units.y.floor() as u32)),
    );
    for (entity, pos, aggroed, mut wander, mut provoked) in &mut enemies {
        // A provoked enemy hunts regardless of the path distance — the
        // blast told it exactly where to look — standing down only when
        // the timer lapses back into the normal rules.
        let forced = match provoked.as_deref_mut() {
            Some(timer) => provoked_tick(&mut timer.0, time.delta()),
            None => false,
        };
        if forced {
            if !aggroed && let Ok(mut entity_commands) = commands.get_entity(entity) {
                entity_commands.insert(Aggro);
            }
            continue;
        }
        if provoked.is_some()
            && let Ok(mut entity_commands) = commands.get_entity(entity)
        {
            // The timer just lapsed: back to the normal hysteresis.
            entity_commands.remove::<Provoked>();
        }
        let units = tile_units((width, height), config.tile_size, pos.0);
        match aggro_flip(aggroed, flow.field.steps((units.x, units.y)), notice) {
            Some(true) => {
                if let Ok(mut entity_commands) = commands.get_entity(entity) {
                    entity_commands.insert(Aggro);
                }
            }
            Some(false) => {
                wander.home = pos.0;
                if let Ok(mut entity_commands) = commands.get_entity(entity) {
                    entity_commands.remove::<Aggro>();
                }
            }
            None => {}
        }
    }
}

/// Un-alerted enemies mill around their home position instead of pursuing.
#[allow(clippy::type_complexity)]
fn wander_seek(
    time: Res<Time>,
    mut rng: ResMut<crate::drops::DropRng>,
    mut enemies: Query<
        (&Position, &mut LinearVelocity, &mut Wander),
        (With<Enemy>, Without<Aggro>),
    >,
) {
    for (pos, mut velocity, mut wander) in &mut enemies {
        wander.repick.tick(time.delta());
        if wander.repick.is_finished() {
            wander.dir = Vec2::from_angle(rng.0.random_range(0.0..core::f32::consts::TAU));
            wander.repick.set_duration(Duration::from_secs_f64(
                (WANDER_REPICK_SECS + rng.0.random_range(-0.5..0.5)) as f64,
            ));
            wander.repick.reset();
        }
        velocity.0 = wander_velocity(wander.home, pos.0, wander.dir);
    }
}

/// Steer chasers and tanks toward the player and show their damage state.
#[allow(clippy::type_complexity)] // Bevy query tuples read worse split up.
#[allow(clippy::too_many_arguments)]
fn enemy_seek(
    time: Res<Time>,
    player: Single<&Position, With<crate::Player>>,
    map: Res<crate::world::WorldMapRes>,
    config: Res<MapConfig>,
    flow: Res<crate::world::PlayerFlow>,
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
        (With<Enemy>, Without<Thrower>, With<Aggro>),
    >,
) {
    let player_pos = player.0;
    for (pos, mut velocity, health, speed, base, mut sprite, mut knockback) in &mut enemies {
        let to_player = player_pos - pos.0;
        let seek = seek_velocity(
            &map,
            config.tile_size,
            pos.0,
            to_player,
            speed.0,
            &flow.field,
        );
        let kick = decay_knockback(knockback.as_deref_mut(), time.delta_secs());
        velocity.0 = seek + kick;
        sprite.color = damage_tint(base.0, health.hp, health.max);
    }
}

/// Throwers keep their distance instead of swarming, tinted by damage.
/// Out of throw range they steer like anything else (flow field when
/// blocked); in range they hold position.
#[allow(clippy::type_complexity)]
fn thrower_seek(
    time: Res<Time>,
    player: Single<&Position, With<crate::Player>>,
    map: Res<crate::world::WorldMapRes>,
    config: Res<MapConfig>,
    flow: Res<crate::world::PlayerFlow>,
    mut throwers: Query<
        (
            &Position,
            &mut LinearVelocity,
            &Health,
            &BaseColour,
            &mut Sprite,
            Option<&mut Knockback>,
        ),
        (With<Thrower>, With<Aggro>),
    >,
) {
    let player_pos = player.0;
    for (pos, mut velocity, health, base, mut sprite, mut knockback) in &mut throwers {
        let to_player = player_pos - pos.0;
        let seek = if to_player.length() <= THROW_RANGE {
            Vec2::ZERO // in range: hold position
        } else {
            seek_velocity(
                &map,
                config.tile_size,
                pos.0,
                to_player,
                THROWER_SPEED,
                &flow.field,
            )
        };
        let kick = decay_knockback(knockback.as_deref_mut(), time.delta_secs());
        velocity.0 = seek + kick;
        sprite.color = damage_tint(base.0, health.hp, health.max);
    }
}

/// Steering for one pursuer: straight at the player when the sight line is
/// clear (smooth pursuit, no grid quantisation), otherwise a flow-field
/// step that routes around bends instead of pressing into the nearest
/// corner.
fn seek_velocity(
    map: &WorldMapRes,
    tile_size: f32,
    position: Vec2,
    to_player: Vec2,
    speed: f32,
    flow: &FlowField,
) -> Vec2 {
    if to_player == Vec2::ZERO {
        return Vec2::ZERO;
    }
    let (width, height) = (map.map.grid.width(), map.map.grid.height());
    let units = tile_units((width, height), tile_size, position);
    let player_units = tile_units((width, height), tile_size, position + to_player);
    let clear = wfc::line_of_sight(
        &map.map.grid,
        &map.prototypes,
        (units.x, units.y),
        (player_units.x, player_units.y),
    );
    if clear {
        to_player.normalize() * speed
    } else {
        flow.direction((units.x, units.y))
            .map_or(Vec2::ZERO, |(dx, dy)| Vec2::new(dx, dy) * speed)
    }
}

/// Regrow shield plates after a quiet spell.
fn shield_regen(time: Res<Time>, mut shields: Query<&mut Shield>) {
    for shield in &mut shields {
        let Shield { plates, regen } = shield.into_inner();
        regrow_plate(plates, regen, ENEMY_SHIELD_PLATES, time.delta());
    }
}

/// Lob a bouncing projectile at the player's current position on cooldown —
/// alerted throwers only; an un-alerted mob never opens fire.
#[allow(clippy::type_complexity)]
fn thrower_attack(
    mut commands: Commands,
    time: Res<Time>,
    player: Single<&Position, With<crate::Player>>,
    mut throwers: Query<(&Position, &mut ThrowTimer), (With<Thrower>, With<Aggro>)>,
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
            crate::projectiles::PROJECTILE_DAMAGE,
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
    fn the_spawn_floor_sits_outside_aggro_reach() {
        // The relation between the two tuning constants is a compile-time
        // invariant: the build refuses a spawn floor inside aggro reach.
        const _: () = assert!(MIN_SPAWN_DISTANCE > AGGRO_RANGE_STEPS);
        // An un-alerted enemy on the minimum spawn distance can never flip
        // aggro while the player stands at the spawn.
        assert_eq!(
            aggro_flip(false, Some(MIN_SPAWN_DISTANCE), AGGRO_RANGE_STEPS),
            None
        );
    }

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

    /// A 5x5 map (all floor) as a `WorldMapRes`, plus a flow field toward
    /// tile (3, 2). `wall_tile` optionally walls one cell.
    fn test_map(wall_tile: Option<(u32, u32)>) -> (WorldMapRes, FlowField) {
        let prototypes = prototype_set(0.25, 0.02);
        let floor = prototypes
            .iter()
            .position(|p| p.prototype.class.walkable())
            .expect("the set has walkable tiles") as u32;
        let mut grid = wfc::Grid::new(5, 5);
        for y in 0..5 {
            for x in 0..5 {
                grid.set(x, y, floor);
            }
        }
        if let Some((x, y)) = wall_tile {
            let wall = prototypes
                .iter()
                .position(|p| !p.prototype.class.walkable())
                .expect("the set has a wall") as u32;
            grid.set(x, y, wall);
        }
        let map = GeneratedMap {
            grid: grid.clone(),
            spawn: (2, 2),
            exit: (2, 2),
        };
        let flow = FlowField::build(&grid, &prototypes, (3, 2));
        (
            WorldMapRes {
                map,
                prototypes,
                integrity: 1.0,
            },
            flow,
        )
    }

    #[test]
    fn seek_drives_straight_when_sight_is_clear() {
        let (map, flow) = test_map(None);
        // Enemy on tile (2,2) (world origin-ish), player one tile east:
        // clear sight, straight pursuit at full speed.
        let position = Vec2::new(0.0, 0.0);
        let player = Vec2::new(TILE_SIZE, 0.0);
        let velocity = seek_velocity(&map, TILE_SIZE, position, player - position, 120.0, &flow);
        assert!((velocity.x - 120.0).abs() < 1e-4);
        assert!(velocity.y.abs() < 1e-4);
        // No separation: no seek at all.
        let velocity = seek_velocity(&map, TILE_SIZE, position, Vec2::ZERO, 120.0, &flow);
        assert_eq!(velocity, Vec2::ZERO);
    }

    #[test]
    fn seek_follows_the_flow_field_when_blocked() {
        // A wall at (2,2) blocks the straight shot from (0,2) to (3,2).
        let (map, flow) = test_map(Some((2, 2)));
        // Enemy on tile (0,2): world x = (0.5 - 2.5) * 32 = -64.
        let position = Vec2::new(-64.0, 0.0);
        let player = Vec2::new(TILE_SIZE, 0.0);
        let velocity = seek_velocity(&map, TILE_SIZE, position, player - position, 120.0, &flow);
        // The flow field routes up-right through the open corner above the
        // wall — a diagonal descent.
        assert!((velocity.x - 120.0 / core::f32::consts::SQRT_2).abs() < 1e-4);
        assert!((velocity.y + 120.0 / core::f32::consts::SQRT_2).abs() < 1e-4);
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
    fn aggro_flips_on_approach_and_escape_with_hysteresis() {
        let notice = AGGRO_RANGE_STEPS;
        // Un-alerted enemies wake within the notice range...
        assert_eq!(aggro_flip(false, Some(notice), notice), Some(true));
        // ...but hold their state in the hysteresis band: an un-alerted
        // enemy between the ranges stays calm, an alerted one stays angry.
        let between = Some(notice + 1);
        assert_eq!(aggro_flip(false, between, notice), None);
        assert_eq!(aggro_flip(true, between, notice), None);
        // Alerted enemies stand down only past the stand-down range.
        assert_eq!(
            aggro_flip(true, Some(notice + DEAGGRO_EXTRA_STEPS + 1), notice),
            Some(false)
        );
        // Boundary: the aggro edge is inclusive, the stand-down edge is not
        // — exactly at the escape range the pursuit holds.
        assert_eq!(
            aggro_flip(true, Some(notice + DEAGGRO_EXTRA_STEPS), notice),
            None
        );
        // No path to the player at all: stand down rather than chase
        // forever.
        assert_eq!(aggro_flip(true, None, notice), Some(false));
        assert_eq!(aggro_flip(false, None, notice), None);
    }

    #[test]
    fn the_cloak_shrinks_the_notice_range() {
        // Uncloaked: the standard range. Cloaked: the sneak range — an
        // enemy 4 steps away stays dormant while cloaked (the standard
        // range would have woken it), and an alerted one stands down there.
        assert_eq!(aggro_flip(false, Some(4), AGGRO_RANGE_STEPS), Some(true));
        let sneak = crate::skills::cloak_sneak_steps(&crate::skills::SkillLevels {
            cloak: 0,
            ..Default::default()
        });
        assert_eq!(sneak, 6);
        // A 6-step notice still wakes a 5-step enemy...
        assert_eq!(aggro_flip(false, Some(5), sneak), Some(true));
        // ...but a 7-step one stays dormant (hysteresis band).
        assert_eq!(aggro_flip(false, Some(7), sneak), None);
        assert_eq!(aggro_flip(true, Some(4), sneak), None);
        // Maxed sneak: two steps.
        let sneak = crate::skills::cloak_sneak_steps(&crate::skills::SkillLevels {
            cloak: crate::skills::CLOAK_SNEAK_CAP,
            ..Default::default()
        });
        assert_eq!(sneak, 2);
        assert_eq!(aggro_flip(false, Some(2), sneak), Some(true));
        assert_eq!(aggro_flip(false, Some(3), sneak), None);
    }

    #[test]
    fn wander_drifts_until_the_leash_pulls_home() {
        let home = Vec2::ZERO;
        let dir = Vec2::X;
        // Inside the leash: drift along the wander direction.
        let velocity = wander_velocity(home, Vec2::new(10.0, 0.0), dir);
        assert!((velocity.x - WANDER_SPEED).abs() < 1e-5);
        // Beyond the leash: head home regardless of the wander direction.
        let far = Vec2::new(WANDER_LEASH_TILES * TILE_SIZE + 10.0, 0.0);
        let velocity = wander_velocity(home, far, dir);
        assert!(velocity.x < 0.0, "the leash turns the enemy back home");
        assert!((velocity.length() - WANDER_SPEED).abs() < 1e-5);
    }

    #[test]
    fn provoked_holds_for_the_duration_then_lapses() {
        let mut timer = Timer::from_seconds(PROVOKED_SECS, TimerMode::Once);
        // Holds through the duration...
        assert!(provoked_tick(
            &mut timer,
            Duration::from_secs_f64(PROVOKED_SECS as f64 * 0.9)
        ));
        assert!(provoked_tick(
            &mut timer,
            Duration::from_secs_f64(PROVOKED_SECS as f64 * 0.09)
        ));
        // ...and lapses the frame the timer runs out.
        assert!(!provoked_tick(
            &mut timer,
            Duration::from_secs_f64(PROVOKED_SECS as f64 * 0.01)
        ));
    }

    #[test]
    fn earshot_is_euclidean_sound_ignores_walls() {
        let centre = Vec2::ZERO;
        // Boundary counts as within earshot.
        assert!(within_earshot(centre, Vec2::new(PROVOKE_RADIUS, 0.0)));
        assert!(within_earshot(
            centre,
            Vec2::new(PROVOKE_RADIUS * 0.99, 0.0)
        ));
        assert!(!within_earshot(
            centre,
            Vec2::new(PROVOKE_RADIUS * 1.01, 0.0)
        ));
        // The radius is euclidean, not Chebyshev: the box corner sits
        // farther out than the circle, even though its axes are in range.
        assert!(!within_earshot(
            centre,
            Vec2::new(PROVOKE_RADIUS, PROVOKE_RADIUS)
        ));
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
