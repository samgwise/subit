//! Projectiles: thrown by thrower enemies, bouncing off walls. A shielded
//! player catches any enemy projectile that enters the shield ring and
//! reflects it along the cursor direction with a fresh bounce budget,
//! turning it back into a weapon; an unshielded player takes the hit
//! instead.
//!
//! Allegiance is physical: enemy shots and player shots live on separate
//! collision layers, so a reflected shot can never hit the player again and
//! never friendly-fires into the swarm.

use avian2d::prelude::{
    Collider, CollisionEventsEnabled, CollisionLayers, CollisionStart, Friction, LinearVelocity,
    LockedAxes, Position, Restitution, RigidBody, SleepingDisabled,
};
use bevy::prelude::*;

use crate::bridge::{BridgeTx, GameAudioEvent};
use crate::combat::{ComboState, PlayerShield, PlayerVitals, SHIELD_RADIUS};
use crate::world::{LAYER_ENEMY, LAYER_ENEMY_SHOT, LAYER_PLAYER, LAYER_PLAYER_SHOT, LAYER_WALL};

/// Projectile travel speed in world units per second (~4 px/frame, so no
/// CCD is needed).
pub const PROJECTILE_SPEED: f32 = 240.0;

/// Wall bounces a fresh projectile can survive; the contact after the last
/// one fails it.
pub const MAX_BOUNCES: u32 = 3;

/// Safety net so reflected pinballs can never live forever.
const LIFETIME_SECS: f32 = 16.0;

/// Projectile collider radius.
const RADIUS: f32 = 5.0;

/// Damage an (unshielded) projectile hit deals.
const PROJECTILE_DAMAGE: i32 = 20;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProjectileAllegiance {
    Enemy,
    Player,
}

#[derive(Component, Debug)]
pub struct Projectile {
    pub bounces: u32,
    pub allegiance: ProjectileAllegiance,
}

/// Per-projectile expiry timer.
#[derive(Component, Debug)]
struct Lifetime(Timer);

pub struct ProjectilePlugin;

impl Plugin for ProjectilePlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(
            Update,
            (projectile_impacts, shield_reflection, projectile_lifetime),
        );
    }
}

/// The collision layer set for an allegiance.
fn layers_for(allegiance: ProjectileAllegiance) -> CollisionLayers {
    match allegiance {
        ProjectileAllegiance::Enemy => {
            CollisionLayers::from_bits(LAYER_ENEMY_SHOT, LAYER_WALL | LAYER_PLAYER)
        }
        ProjectileAllegiance::Player => {
            CollisionLayers::from_bits(LAYER_PLAYER_SHOT, LAYER_WALL | LAYER_ENEMY)
        }
    }
}

/// Sprite tint per allegiance: enemy shots burn orange, reflected shots
/// turn cyan (matching the cleave colour).
fn colour_for(allegiance: ProjectileAllegiance) -> Color {
    match allegiance {
        ProjectileAllegiance::Enemy => Color::srgb(1.0, 0.6, 0.2),
        ProjectileAllegiance::Player => Color::srgb(0.4, 0.9, 1.0),
    }
}

/// Spawn a projectile at `origin` moving with `velocity`.
pub fn spawn_projectile(
    commands: &mut Commands,
    origin: Vec2,
    velocity: Vec2,
    allegiance: ProjectileAllegiance,
) {
    commands.spawn((
        Projectile {
            bounces: MAX_BOUNCES,
            allegiance,
        },
        Sprite::from_color(colour_for(allegiance), Vec2::splat(RADIUS * 2.0)),
        Transform::from_xyz(origin.x, origin.y, 2.0),
        RigidBody::Dynamic,
        Collider::circle(RADIUS),
        LinearVelocity(velocity),
        Restitution::new(1.0),
        Friction::ZERO,
        LockedAxes::ROTATION_LOCKED,
        SleepingDisabled,
        CollisionEventsEnabled,
        layers_for(allegiance),
        Lifetime(Timer::from_seconds(LIFETIME_SECS, TimerMode::Once)),
    ));
}

/// Remaining bounces after a wall contact: `None` means it fails and is
/// despawned (the contact after the last budgeted bounce).
fn bounce_result(bounces: u32) -> Option<u32> {
    bounces.checked_sub(1)
}

/// Velocity of a reflected projectile: along the player's aim when there is
/// one, otherwise straight back the way it came.
fn reflected_velocity(incoming: Vec2, aim: Vec2) -> Vec2 {
    if aim != Vec2::ZERO {
        aim.normalize_or_zero() * PROJECTILE_SPEED
    } else if incoming != Vec2::ZERO {
        -incoming.normalize_or_zero() * PROJECTILE_SPEED
    } else {
        Vec2::ZERO
    }
}

/// Handle every projectile collision this frame: wall bounces, player hits
/// and reflected kills. Shield reflection itself is proximity-based (see
/// `shield_reflection`) — a shot touching a shielded player simply doesn't
/// hurt.
#[allow(clippy::too_many_arguments)]
fn projectile_impacts(
    mut commands: Commands,
    mut collisions: MessageReader<CollisionStart>,
    mut projectiles: Query<(
        Entity,
        &Position,
        &mut Projectile,
        &mut LinearVelocity,
        &mut Sprite,
        &mut Lifetime,
    )>,
    walls: Query<(), With<crate::world::WallBody>>,
    player: Single<(Entity, &Position), With<crate::Player>>,
    enemies: Query<Entity, With<crate::enemies::Enemy>>,
    mut vitals: ResMut<PlayerVitals>,
    shield: Res<PlayerShield>,
    mut combo: ResMut<ComboState>,
    bridge: Res<BridgeTx>,
    time: Res<Time>,
) {
    let (player_entity, _) = *player;
    let mut despawned: Vec<Entity> = Vec::new();

    for event in collisions.read() {
        // Identify which side of the pair (if any) is our projectile.
        let (projectile_entity, other) = if projectiles.contains(event.collider1) {
            (event.collider1, event.collider2)
        } else if projectiles.contains(event.collider2) {
            (event.collider2, event.collider1)
        } else {
            continue;
        };
        if despawned.contains(&projectile_entity) {
            continue;
        }

        if walls.contains(other) {
            // Wall bounce: count down, fail after the budget is spent.
            let Ok((_, _, mut projectile, ..)) = projectiles.get_mut(projectile_entity) else {
                continue;
            };
            match bounce_result(projectile.bounces) {
                Some(remaining) => projectile.bounces = remaining,
                None => {
                    if let Ok(mut entity_commands) = commands.get_entity(projectile_entity) {
                        entity_commands.despawn();
                        despawned.push(projectile_entity);
                    }
                }
            }
        } else if other == player_entity {
            // Shield up: the projectile passes through untouched — the
            // proximity reflect (ring radius) re-aims it before it can land.
            // Unshielded: it lands, if the invulnerability window allows.
            if shield.is_active() {
                continue;
            }
            if vitals.damage(PROJECTILE_DAMAGE) {
                tracing::info!(hp = vitals.hp, "player hit by projectile");
                if let Ok(mut entity_commands) = commands.get_entity(projectile_entity) {
                    entity_commands.despawn();
                    despawned.push(projectile_entity);
                }
            }
            // Blocked by the invulnerability window: let it fly.
        } else if enemies.contains(other) {
            // Only player-allegiance shots can reach an enemy (layers), but
            // stay defensive about it.
            let is_player_shot = projectiles
                .get(projectile_entity)
                .is_ok_and(|(_, _, p, ..)| p.allegiance == ProjectileAllegiance::Player);
            if !is_player_shot {
                continue;
            }
            if let Ok(mut entity_commands) = commands.get_entity(other) {
                entity_commands.despawn();
            }
            if let Ok(mut entity_commands) = commands.get_entity(projectile_entity) {
                entity_commands.despawn();
                despawned.push(projectile_entity);
            }
            combo.register_kill(time.elapsed());
            bridge.send(GameAudioEvent::MobSweep {
                kill_count: 1,
                combo: combo.count,
            });
            tracing::info!(combo = combo.count, "projectile killed an enemy");
        }
    }
}

/// True while the projectile is inside the shield's catch radius (the drawn
/// ring). Boundary counts as caught.
fn within_shield_range(player_pos: Vec2, projectile_pos: Vec2) -> bool {
    player_pos.distance(projectile_pos) <= SHIELD_RADIUS
}

/// While the shield is up, catch every enemy projectile inside the ring and
/// reflect it along the cursor with a fresh bounce budget — the drawn ring
/// is the actual catch zone, not a decoration.
#[allow(clippy::too_many_arguments)]
fn shield_reflection(
    mut commands: Commands,
    mut projectiles: Query<(
        Entity,
        &Position,
        &mut Projectile,
        &mut LinearVelocity,
        &mut Sprite,
        &mut Lifetime,
    )>,
    player: Single<&Position, With<crate::Player>>,
    shield: Res<PlayerShield>,
    bridge: Res<BridgeTx>,
    window: Single<&Window>,
    camera: Single<(&Camera, &GlobalTransform), With<Camera2d>>,
) {
    if !shield.is_active() {
        return;
    }
    let player_pos = player.0;
    for (entity, pos, mut projectile, mut velocity, mut sprite, mut lifetime) in &mut projectiles {
        if projectile.allegiance != ProjectileAllegiance::Enemy
            || !within_shield_range(player_pos, pos.0)
        {
            continue;
        }
        apply_reflection(
            &mut commands,
            entity,
            &mut projectile,
            &mut velocity,
            &mut sprite,
            &mut lifetime,
            player_pos,
            *window,
            camera.0,
            camera.1,
        );
        bridge.send(GameAudioEvent::ShieldReflect);
        tracing::info!("shield reflected a projectile");
    }
}

/// Reflect a projectile off the shield: re-aim along the cursor, restore the
/// bounce budget, refresh the lifetime, and flip allegiance.
#[allow(clippy::too_many_arguments)]
fn apply_reflection(
    commands: &mut Commands,
    entity: Entity,
    projectile: &mut Projectile,
    velocity: &mut LinearVelocity,
    sprite: &mut Sprite,
    lifetime: &mut Lifetime,
    player_pos: Vec2,
    window: &Window,
    camera: &Camera,
    camera_transform: &GlobalTransform,
) {
    let aim = window
        .cursor_position()
        .and_then(|cursor| camera.viewport_to_world_2d(camera_transform, cursor).ok())
        .map(|cursor_world| cursor_world - player_pos)
        .unwrap_or(Vec2::ZERO);

    velocity.0 = reflected_velocity(velocity.0, aim);
    projectile.bounces = MAX_BOUNCES;
    projectile.allegiance = ProjectileAllegiance::Player;
    sprite.color = colour_for(ProjectileAllegiance::Player);
    lifetime.0.reset();
    // CollisionLayers is immutable: swap it via remove + insert.
    if let Ok(mut entity_commands) = commands.get_entity(entity) {
        entity_commands.remove::<CollisionLayers>();
        entity_commands.insert(layers_for(ProjectileAllegiance::Player));
    }
}

/// Expire projectiles whose lifetime ran out.
fn projectile_lifetime(
    mut commands: Commands,
    time: Res<Time>,
    mut projectiles: Query<(Entity, &mut Lifetime)>,
) {
    for (entity, mut lifetime) in &mut projectiles {
        lifetime.0.tick(time.delta());
        if lifetime.0.is_finished()
            && let Ok(mut entity_commands) = commands.get_entity(entity)
        {
            entity_commands.despawn();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wall_bounces_count_down_then_fail() {
        assert_eq!(bounce_result(MAX_BOUNCES), Some(MAX_BOUNCES - 1));
        assert_eq!(bounce_result(2), Some(1));
        assert_eq!(bounce_result(1), Some(0));
        // The contact after the last budgeted bounce fails the projectile.
        assert_eq!(bounce_result(0), None);
    }

    #[test]
    fn shield_ring_boundary_counts_as_caught() {
        let player = Vec2::ZERO;
        // Just inside, exactly on, and just outside the ring.
        let inside = Vec2::new(SHIELD_RADIUS * 0.99, 0.0);
        let on_edge = Vec2::new(SHIELD_RADIUS, 0.0);
        let outside = Vec2::new(SHIELD_RADIUS * 1.01, 0.0);
        assert!(within_shield_range(player, inside));
        assert!(within_shield_range(player, on_edge));
        assert!(!within_shield_range(player, outside));
    }

    #[test]
    fn reflection_prefers_the_aim_and_falls_back_to_reversal() {
        let incoming = Vec2::new(PROJECTILE_SPEED, 0.0);
        // With a cursor aim: straight along the aim at full speed.
        let reflected = reflected_velocity(incoming, Vec2::new(0.0, 5.0));
        assert!((reflected - Vec2::new(0.0, PROJECTILE_SPEED)).length() < 1e-4);
        // No cursor: bounce straight back.
        let back = reflected_velocity(incoming, Vec2::ZERO);
        assert!((back - Vec2::new(-PROJECTILE_SPEED, 0.0)).length() < 1e-4);
        // Degenerate everything: no crash, no motion.
        assert_eq!(reflected_velocity(Vec2::ZERO, Vec2::ZERO), Vec2::ZERO);
    }
}
