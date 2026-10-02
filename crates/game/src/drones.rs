//! The transmitter drone: a player-side companion summoned by the
//! transmitter skill. It hovers at a standoff ring around the player and
//! fires chip-damage shots at the nearest enemy. The link is fragile in
//! exactly one way — corrupted data zones jam the transmitter signal, and a
//! grenade blast fries the drone outright. Either way it goes offline and
//! reboots on a clock that only runs while the link is clean.

use std::time::Duration;

use avian2d::prelude::Position;
use bevy::prelude::*;

use crate::bridge::{BridgeTx, GameAudioEvent};
use crate::corruption::CorruptedZones;
use crate::projectiles::{ProjectileAllegiance, spawn_projectile, PROJECTILE_SPEED};
use crate::skills::{AbilityUnlocks, GameState, SkillLevels, fleet_count};
use crate::world::{MapConfig, PlayerFlow, TILE_SIZE, WorldMapRes, tile_units};
use wfc::line_of_sight;

/// Damage per drone shot — a chip, not a cleave: three shots finish a
/// chaser, so the drone weakens and finishes rather than replacing the
/// player's aim.
pub const DRONE_DAMAGE: i32 = 34;

/// Seconds between drone shots.
const DRONE_FIRE_SECS: f32 = 2.0;

/// The drone engages enemies within this distance of itself (its shots
/// travel and bounce — blind lobs are the thrower's trick and it has no
/// line-of-sight check either).
const DRONE_RANGE: f32 = TILE_SIZE * 6.0;

/// Drone travel speed in world units per second: brisk enough to keep up
/// when the player strolls, never quite matching a sprint.
const DRONE_SPEED: f32 = 180.0;

/// The drone hovers this far from the player and holds station inside it.
const DRONE_STANDOFF: f32 = TILE_SIZE * 2.0;

/// Seconds of clean link a downed drone needs before it rejoins.
pub const DRONE_REBOOT_SECS: f32 = 8.0;

/// Drone sprite size.
const DRONE_SPRITE: f32 = 12.0;

/// Drone tint while online — pale cyan, bloom-friendly.
const DRONE_COLOUR: Color = Color::srgb(0.6, 0.95, 1.0);

/// Drone tint while jammed or rebooting — grounded and grey.
const DRONE_OFFLINE_COLOUR: Color = Color::srgb(0.35, 0.38, 0.42);

/// A companion drone.
#[derive(Component)]
pub struct Drone;

/// Per-drone fire cadence.
#[derive(Component, Debug)]
struct FireTimer(Timer);

/// Marker: the transmitter link is jammed by corruption (the player's end
/// of the link stands in a zone — the drone's own position is irrelevant).
/// Inserted/removed by the jam gate; while present the drone neither
/// moves, fires, nor reboots.
#[derive(Component, Debug)]
pub struct Jammed;

/// Marker with the reboot clock: the drone is offline and counting clean
/// link time. The clock only ticks while the link is unjammed, so a drone
/// downed (or unjammed) beside corruption waits it out.
#[derive(Component, Debug)]
pub struct Downed(pub Timer);

pub struct DronePlugin;

impl Plugin for DronePlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(
            Update,
            (fleet_maintenance, jam_gate, drone_fire, drone_seek)
                .run_if(in_state(GameState::Playing)),
        );
    }
}

/// Spawn the shortfall of the fleet at the player (spread on a small ring
/// so a full fleet doesn't stack), or despawn extras. Runs every frame;
/// buying the transmitter or a fleet point, and descending (drones are
/// cleared with the world) are both healed by the next tick.
#[allow(clippy::type_complexity)]
fn fleet_maintenance(
    mut commands: Commands,
    unlocks: Res<AbilityUnlocks>,
    levels: Res<SkillLevels>,
    // The physics position, not the transform: on the descent frame the
    // transform still lags the teleport, and a drone spawned at the stale
    // exit can land inside a wall of the fresh map.
    player: Single<&Position, With<crate::Player>>,
    drones: Query<Entity, With<Drone>>,
) {
    let existing: Vec<Entity> = drones.iter().collect();
    let desired = if unlocks.transmitter {
        fleet_count(&levels)
    } else {
        0
    };
    if existing.len() >= desired {
        for &entity in existing[desired.min(existing.len())..].iter() {
            if let Ok(mut entity_commands) = commands.get_entity(entity) {
                entity_commands.despawn();
            }
        }
        return;
    }
    let centre = player.0;
    for i in existing.len()..desired {
        // A deterministic four-point ring so a fleet reads as a formation.
        let offset = match i % 4 {
            0 => Vec2::ZERO,
            1 => Vec2::new(TILE_SIZE, TILE_SIZE * 0.5),
            2 => Vec2::new(-TILE_SIZE, TILE_SIZE * 0.5),
            _ => Vec2::new(0.0, -TILE_SIZE),
        };
        let pos = centre + offset;
        commands.spawn((
            Drone,
            Sprite::from_color(DRONE_COLOUR, Vec2::splat(DRONE_SPRITE)),
            Transform::from_xyz(pos.x, pos.y, 1.5),
            FireTimer(Timer::from_seconds(DRONE_FIRE_SECS, TimerMode::Once)),
        ));
        tracing::info!("transmitter deployed a drone");
    }
}

/// Advance a drone's reboot clock one frame: `Some(true)` when the reboot
/// finishes this frame (back online), `Some(false)` while still waiting,
/// `None` when the link is jammed — no clean time accrues inside corruption.
fn tick_reboot(jammed: bool, timer: &mut Timer, delta: Duration) -> Option<bool> {
    if jammed {
        return None;
    }
    timer.tick(delta);
    Some(timer.is_finished())
}

/// Keep every drone's link state current: jam the fleet while the player
/// stands in corruption, start the reboot when the player leaves, and
/// count the reboot down to a return. The link rides on the player's
/// transmitter alone — a drone hovering over corruption flies on fine,
/// and can never strand itself somewhere it cannot leave while offline.
#[allow(clippy::type_complexity, clippy::too_many_arguments)]
fn jam_gate(
    mut commands: Commands,
    time: Res<Time>,
    zones: Res<CorruptedZones>,
    map: Res<WorldMapRes>,
    config: Res<MapConfig>,
    // The physics position: current even on the descent frame (the
    // transform lags the teleport until the physics sync runs).
    player: Single<&Position, With<crate::Player>>,
    bridge: Res<BridgeTx>,
    // Drones only — without the filter this matched every entity with a
    // transform, jamming the whole world in corruption and "rebooting" it
    // on the way out.
    mut drones: Query<(Entity, Has<Jammed>, Option<&mut Downed>), With<Drone>>,
) {
    let map_size = (map.map.grid.width(), map.map.grid.height());
    let player_in_zone = zones.contains_world(map_size, config.tile_size, player.0);
    for (entity, jammed, mut downed) in &mut drones {
        match (jammed, player_in_zone) {
            // The link just died: dark and silent until it clears.
            (false, true) => {
                if let Ok(mut entity_commands) = commands.get_entity(entity) {
                    entity_commands.insert(Jammed);
                }
            }
            // The link just cleared: the reboot starts now, not before.
            (true, false) => {
                if let Ok(mut entity_commands) = commands.get_entity(entity) {
                    entity_commands.remove::<Jammed>();
                    if downed.is_none() {
                        entity_commands.insert(Downed(Timer::from_seconds(
                            DRONE_REBOOT_SECS,
                            TimerMode::Once,
                        )));
                    }
                }
            }
            _ => {}
        }
        if let Some(clock) = downed.as_deref_mut()
            && tick_reboot(player_in_zone, &mut clock.0, time.delta()) == Some(true)
        {
            if let Ok(mut entity_commands) = commands.get_entity(entity) {
                entity_commands.remove::<Downed>();
            }
            bridge.send(GameAudioEvent::DroneOnline);
            tracing::info!("drone rebooted and rejoined");
        }
    }
}

/// Down every online drone inside the blast radius — the detonation fries
/// the little helper. Jammed or already-downed drones keep their state (a
/// fresh blast does not reset a running reboot).
#[allow(clippy::type_complexity)]
pub fn down_drones_in_radius(
    commands: &mut Commands,
    drones: &Query<(Entity, &Transform, Has<Jammed>, Has<Downed>), With<Drone>>,
    centre: Vec2,
    radius: f32,
    bridge: &BridgeTx,
) {
    for (entity, transform, jammed, downed) in drones {
        if jammed
            || downed
            || transform.translation.xy().distance(centre) > radius
        {
            continue;
        }
        if let Ok(mut entity_commands) = commands.get_entity(entity) {
            entity_commands.insert(Downed(Timer::from_seconds(
                DRONE_REBOOT_SECS,
                TimerMode::Once,
            )));
        }
        bridge.send(GameAudioEvent::DroneDown);
        tracing::info!("blast downed a drone");
    }
}

/// A drone stranded where the flow field cannot read — embedded in a wall
/// or sealed in a pocket by a world change — recalls to the player once it
/// is outside the standoff ring. The fleet can be delayed, never lost.
fn needs_recall(has_path: bool, distance: f32) -> bool {
    !has_path && distance > DRONE_STANDOFF
}

/// The drone's speed at `distance` from its standoff ring around the
/// player: full speed a tile out, easing to a hold at the ring.
fn approach_speed(distance: f32) -> f32 {
    if distance <= DRONE_STANDOFF {
        return 0.0;
    }
    let overshoot = distance - DRONE_STANDOFF;
    let ease = (overshoot / TILE_SIZE).min(1.0);
    DRONE_SPEED * ease
}

/// Fire a chip shot at the nearest enemy in range — online drones only;
/// a jammed or downed drone is dead in the air.
#[allow(clippy::type_complexity)]
fn drone_fire(
    mut commands: Commands,
    time: Res<Time>,
    mut drones: Query<(&Transform, &mut FireTimer), (With<Drone>, Without<Jammed>, Without<Downed>)>,
    enemies: Query<&Position, With<crate::enemies::Enemy>>,
    bridge: Res<BridgeTx>,
) {
    let enemy_positions: Vec<Vec2> = enemies.iter().map(|pos| pos.0).collect();
    for (transform, mut timer) in &mut drones {
        timer.0.tick(time.delta());
        if !timer.0.is_finished() {
            continue;
        }
        let origin = transform.translation.xy();
        let Some(target) = nearest_enemy(origin, &enemy_positions) else {
            continue;
        };
        timer.0.reset();
        let dir = (target - origin).normalize_or_zero();
        spawn_projectile(
            &mut commands,
            origin + dir * DRONE_SPRITE,
            dir * PROJECTILE_SPEED,
            ProjectileAllegiance::Player,
            DRONE_DAMAGE,
        );
        bridge.send(GameAudioEvent::DroneShot);
    }
}

/// The position of the enemy nearest to `origin` within the drone's engage
/// range, if any.
fn nearest_enemy(origin: Vec2, enemy_positions: &[Vec2]) -> Option<Vec2> {
    enemy_positions
        .iter()
        .filter(|pos| origin.distance(**pos) <= DRONE_RANGE)
        .copied()
        .min_by(|a, b| {
            origin
                .distance(*a)
                .total_cmp(&origin.distance(*b))
        })
}

/// Hover at the standoff ring around the player: straight pursuit on a
/// clear sight line, flow-field descent around walls. Offline drones hang
/// where they stopped until the reboot returns them.
#[allow(clippy::type_complexity, clippy::too_many_arguments)]
fn drone_seek(
    time: Res<Time>,
    player: Single<&Position, With<crate::Player>>,
    map: Res<WorldMapRes>,
    config: Res<MapConfig>,
    flow: Res<PlayerFlow>,
    mut drones: Query<
        (&mut Transform, &mut Sprite, Has<Jammed>, Has<Downed>),
        (With<Drone>, Without<crate::Player>),
    >,
) {
    let (width, height) = (map.map.grid.width(), map.map.grid.height());
    let player_pos = player.0;
    let delta = time.delta_secs();
    for (mut transform, mut sprite, jammed, downed) in &mut drones {
        let pos = transform.translation.xy();
        let offline = jammed || downed;
        // The tint tracks the link state: pale and pulsing online, grey
        // while jammed or rebooting.
        sprite.color = if offline {
            DRONE_OFFLINE_COLOUR
        } else {
            pulse_colour(time.elapsed_secs())
        };
        if offline {
            continue;
        }
        let units = tile_units((width, height), config.tile_size, pos);
        let player_units = tile_units((width, height), config.tile_size, player_pos);
        let has_path = flow.field.steps((units.x, units.y)).is_some();
        if needs_recall(has_path, pos.distance(player_pos)) {
            // Stranded where no path reads (the descent race, or a world
            // change): beam back to the player instead of hanging forever.
            transform.translation = player_pos.extend(1.5);
            continue;
        }
        let dir = if line_of_sight(
            &map.map.grid,
            &map.prototypes,
            (units.x, units.y),
            (player_units.x, player_units.y),
        ) {
            (player_pos - pos).normalize_or_zero()
        } else {
            flow
                .field
                .direction((units.x, units.y))
                .map_or(Vec2::ZERO, |(dx, dy)| Vec2::new(dx, dy))
        };
        let step = dir * (approach_speed(pos.distance(player_pos)) * delta);
        transform.translation += step.extend(0.0);
    }
}

/// The online drone tint: a soft sine pulse on the pale cyan.
fn pulse_colour(elapsed: f32) -> Color {
    let pulse = 0.75 + 0.25 * (elapsed * 3.0).sin();
    Color::srgb(
        DRONE_COLOUR.to_srgba().red * pulse,
        DRONE_COLOUR.to_srgba().green * pulse,
        DRONE_COLOUR.to_srgba().blue * pulse,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_reboot_holds_inside_corruption() {
        let mut timer = Timer::from_seconds(DRONE_REBOOT_SECS, TimerMode::Once);
        let delta = Duration::from_secs_f64(1.0);
        // Jammed: no clean time accrues.
        for _ in 0..12 {
            assert_eq!(tick_reboot(true, &mut timer, delta), None);
        }
        // The moment the link clears, the full clock still runs.
        let mut finished = false;
        for _ in 0..DRONE_REBOOT_SECS as i32 {
            finished = tick_reboot(false, &mut timer, delta) == Some(true);
        }
        assert!(finished, "ten clean seconds reboot the drone");
    }

    #[test]
    fn stranded_drones_recall_once_they_are_outside_the_ring() {
        // No path and far away: the wall has it — recall.
        assert!(needs_recall(false, DRONE_STANDOFF + 1.0));
        // A path (even a long one): it flies back on its own.
        assert!(!needs_recall(true, DRONE_STANDOFF + 100.0));
        // No path but already at the player's side: nothing to recall.
        assert!(!needs_recall(false, DRONE_STANDOFF));
    }

    #[test]
    fn the_drone_holds_at_the_standoff_ring() {
        assert_eq!(approach_speed(DRONE_STANDOFF), 0.0);
        assert_eq!(approach_speed(DRONE_STANDOFF - 1.0), 0.0);
        // Full speed a tile past the ring, easing in between.
        assert_eq!(approach_speed(DRONE_STANDOFF + TILE_SIZE), DRONE_SPEED);
        let mid = approach_speed(DRONE_STANDOFF + TILE_SIZE * 0.5);
        assert!(mid > 0.0 && mid < DRONE_SPEED);
    }

    #[test]
    fn the_drone_engages_only_the_nearest_enemy_in_range() {
        let origin = Vec2::ZERO;
        let nearer = Vec2::new(DRONE_RANGE * 0.9, 0.0);
        let farther = Vec2::new(DRONE_RANGE - 1.0, 0.0);
        let out_of_range = Vec2::new(DRONE_RANGE + TILE_SIZE, 0.0);
        assert_eq!(
            nearest_enemy(origin, &[out_of_range, farther, nearer]),
            Some(nearer)
        );
        assert_eq!(nearest_enemy(origin, &[out_of_range]), None);
        assert_eq!(nearest_enemy(origin, &[]), None);
    }
}
