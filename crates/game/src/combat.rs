//! Combat: mouse-aimed cleave attack, contact damage, and the player health
//! loop (invulnerability window, respawn at the map spawn).
//!
//! The geometry and combo logic are pure functions so they can be tested
//! without spinning up the ECS.

use std::time::Duration;

use avian2d::prelude::{CollidingEntities, LinearVelocity, Position};
use bevy::prelude::*;

use crate::bridge::{BridgeTx, GameAudioEvent};
use crate::enemies::Enemy;
use crate::world::{SpawnPoint, TILE_SIZE};

/// Player hit points at full health.
const MAX_HP: i32 = 100;

/// Damage dealt by one enemy contact.
const CONTACT_DAMAGE: i32 = 30;

/// Contact damage is applied at most once per window, so brushing a swarm
/// cannot delete the player in a single frame.
const INVULN_SECS: f32 = 0.5;

/// Seconds between cleave swings.
const ATTACK_COOLDOWN_SECS: f32 = 0.25;

/// Cleave reach in world units (three tiles).
const CLEAVE_RADIUS: f32 = TILE_SIZE * 3.0;

/// Half the cleave arc: a 90-degree cone around the aim direction.
const CLEAVE_HALF_ANGLE: f32 = std::f32::consts::FRAC_PI_4;

/// Kills further apart than this window do not count as the same combo.
const COMBO_WINDOW_SECS: f64 = 2.0;

/// How long the cleave flash stays on screen.
const ATTACK_FX_SECS: f32 = 0.1;

/// Player health state plus the contact-damage invulnerability window.
#[derive(Resource, Debug)]
pub struct PlayerVitals {
    pub hp: i32,
    invuln: Timer,
}

/// Cooldown gating consecutive cleave swings.
#[derive(Resource, Debug)]
struct CleaveCooldown(Timer);

/// Consecutive-kill combo state: kills within the combo window extend the
/// chain, anything longer resets it on the next swing.
#[derive(Resource, Debug, Default)]
pub struct ComboState {
    pub count: u32,
    last_kill: Option<Duration>,
}

/// Transient cleave visual: a ring flash around the swing origin.
#[derive(Resource, Debug, Default)]
struct CleaveFx(Option<CleaveFxActive>);

#[derive(Debug)]
struct CleaveFxActive {
    origin: Vec2,
    timer: Timer,
}

pub struct CombatPlugin;

impl Plugin for CombatPlugin {
    fn build(&self, app: &mut App) {
        app.insert_resource(PlayerVitals {
            hp: MAX_HP,
            invuln: Timer::from_seconds(INVULN_SECS, TimerMode::Once),
        })
        .insert_resource(CleaveCooldown(Timer::from_seconds(
            ATTACK_COOLDOWN_SECS,
            TimerMode::Once,
        )))
        .init_resource::<ComboState>()
        .init_resource::<CleaveFx>()
        .add_systems(
            Update,
            (
                player_attack,
                cleave_fx,
                contact_damage,
                player_speed_telemetry,
            ),
        );
    }
}

/// True when `target` is both within `radius` of `origin` and inside the
/// cone of half-angle `half_angle` around the `aim` direction. A zero-length
/// aim degenerates to a radial hit (everything within radius counts).
fn in_cleave_arc(origin: Vec2, aim: Vec2, target: Vec2, radius: f32, half_angle: f32) -> bool {
    let to_target = target - origin;
    if to_target.length_squared() > radius * radius {
        return false;
    }
    let aim = aim.normalize_or_zero();
    if aim == Vec2::ZERO {
        return true;
    }
    aim.dot(to_target / to_target.length()) >= half_angle.cos()
}

/// The combo count after a kill: consecutive kills inside the window stack,
/// anything after the window lapses starts a fresh chain.
fn advance_combo(current: u32, since_last_kill: Option<Duration>, window: Duration) -> u32 {
    match since_last_kill {
        Some(dt) if dt <= window => current + 1,
        _ => 1,
    }
}

/// Left-click cleave: aim from the player to the cursor, kill every enemy in
/// the arc, batch the kills into one mob-sweep event.
// Bevy systems routinely carry more than clippy's default parameter budget.
#[allow(clippy::too_many_arguments)]
fn player_attack(
    mut commands: Commands,
    mut cooldown: ResMut<CleaveCooldown>,
    mut combo: ResMut<ComboState>,
    mut fx: ResMut<CleaveFx>,
    bridge: Res<BridgeTx>,
    time: Res<Time>,
    mouse: Res<ButtonInput<MouseButton>>,
    window: Single<&Window>,
    camera: Single<(&Camera, &GlobalTransform), With<Camera2d>>,
    player: Single<&Position, With<crate::Player>>,
    enemies: Query<(Entity, &Position), With<Enemy>>,
) {
    cooldown.0.tick(time.delta());
    if !mouse.just_pressed(MouseButton::Left) || !cooldown.0.is_finished() {
        return;
    }

    // Aim: player -> cursor, both in world space. The camera transform must
    // be propagated before this runs, which Update ordering guarantees for a
    // camera that only moves in Update.
    if let Some(cursor) = window.cursor_position()
        && let Ok(cursor_world) = camera.0.viewport_to_world_2d(camera.1, cursor)
    {
        let origin = player.0;
        let aim = cursor_world - origin;

        cooldown.0.reset();
        bridge.send(GameAudioEvent::AttackPrimary);
        *fx = CleaveFx(Some(CleaveFxActive {
            origin,
            timer: Timer::from_seconds(ATTACK_FX_SECS, TimerMode::Once),
        }));

        let killed: Vec<Entity> = enemies
            .iter()
            .filter(|(_, pos)| in_cleave_arc(origin, aim, pos.0, CLEAVE_RADIUS, CLEAVE_HALF_ANGLE))
            .map(|(entity, _)| entity)
            .collect();
        for entity in &killed {
            if let Ok(mut entity_commands) = commands.get_entity(*entity) {
                entity_commands.despawn();
            }
        }

        if !killed.is_empty() {
            combo.count = advance_combo(
                combo.count,
                combo.last_kill,
                Duration::from_secs_f64(COMBO_WINDOW_SECS),
            );
            combo.last_kill = Some(time.elapsed());
            bridge.send(GameAudioEvent::MobSweep {
                kill_count: killed.len() as u32,
                combo: combo.count,
            });
            tracing::info!(kill_count = killed.len(), combo = combo.count, "mob swept");
        }
    }
}

/// Draw the cleave flash while it is active.
fn cleave_fx(mut fx: ResMut<CleaveFx>, time: Res<Time>, mut gizmos: Gizmos) {
    let Some(active) = fx.0.as_mut() else {
        return;
    };
    active.timer.tick(time.delta());
    if active.timer.is_finished() {
        *fx = CleaveFx(None);
    } else {
        gizmos.circle_2d(
            active.origin,
            CLEAVE_RADIUS,
            Color::srgba(0.4, 0.9, 1.0, 0.8),
        );
    }
}

/// Enemies touching the player hurt it, subject to the invulnerability
/// window; death respawns the player at the map spawn with full health.
fn contact_damage(
    time: Res<Time>,
    mut vitals: ResMut<PlayerVitals>,
    mut combo: ResMut<ComboState>,
    spawn_point: Res<SpawnPoint>,
    player: Single<(&CollidingEntities, &mut Position, &mut LinearVelocity), With<crate::Player>>,
    enemies: Query<Entity, With<Enemy>>,
) {
    vitals.invuln.tick(time.delta());
    let (colliding, mut position, mut velocity) = player.into_inner();

    let touching_enemy = colliding.iter().any(|entity| enemies.contains(*entity));
    if !touching_enemy || !vitals.invuln.is_finished() {
        return;
    }

    vitals.invuln.reset();
    vitals.hp -= CONTACT_DAMAGE;
    tracing::info!(hp = vitals.hp, "player hit by enemy contact");

    if vitals.hp <= 0 {
        // Respawn: back to the spawn tile, still and whole, combo lost.
        vitals.hp = MAX_HP;
        vitals.invuln.reset();
        position.0 = spawn_point.0;
        velocity.0 = Vec2::ZERO;
        *combo = ComboState::default();
        tracing::info!("player died; respawned at spawn point");
    }
}

/// Stream player speed to the audio chain (throttled; the bridge's Stream
/// signal type tolerates drops under congestion).
fn player_speed_telemetry(
    bridge: Res<BridgeTx>,
    time: Res<Time>,
    mut last_send: Local<Option<f32>>,
    player: Single<&LinearVelocity, With<crate::Player>>,
) {
    // ~10 Hz is plenty for filter modulation and keeps the channel cheap.
    let elapsed = time.elapsed_secs();
    let last = last_send.unwrap_or(f32::INFINITY);
    if elapsed - last < 0.1 {
        return;
    }
    *last_send = Some(elapsed);
    bridge.send(GameAudioEvent::PlayerTelemetry {
        speed: player.0.length(),
        max_speed: crate::world::PLAYER_SPEED,
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cleave_arc_hits_ahead_and_not_behind() {
        let origin = Vec2::ZERO;
        let aim = Vec2::X;
        // Dead ahead, well inside radius and arc.
        assert!(in_cleave_arc(
            origin,
            aim,
            Vec2::new(50.0, 0.0),
            96.0,
            CLEAVE_HALF_ANGLE
        ));
        // Inside radius but behind the aim line.
        assert!(!in_cleave_arc(
            origin,
            aim,
            Vec2::new(-50.0, 0.0),
            96.0,
            CLEAVE_HALF_ANGLE
        ));
        // Inside radius, outside the 90-degree cone (edge case: perpendicular
        // is exactly on the boundary for a 90-degree cone, so go wider).
        assert!(!in_cleave_arc(
            origin,
            aim,
            Vec2::new(10.0, 90.0),
            96.0,
            CLEAVE_HALF_ANGLE
        ));
        // Outside radius but inside the arc direction.
        assert!(!in_cleave_arc(
            origin,
            aim,
            Vec2::new(200.0, 0.0),
            96.0,
            CLEAVE_HALF_ANGLE
        ));
    }

    #[test]
    fn cleave_arc_boundary_is_inclusive() {
        let origin = Vec2::ZERO;
        let aim = Vec2::X;
        // Exactly on the cone edge: dot == cos(half_angle) counts as a hit.
        let edge = Vec2::new(CLEAVE_HALF_ANGLE.cos(), CLEAVE_HALF_ANGLE.sin());
        assert!(in_cleave_arc(
            origin,
            aim,
            edge,
            CLEAVE_RADIUS,
            CLEAVE_HALF_ANGLE
        ));
        // Exactly on the radius edge counts as a hit.
        assert!(in_cleave_arc(
            origin,
            aim,
            Vec2::new(CLEAVE_RADIUS, 0.0),
            CLEAVE_RADIUS,
            CLEAVE_HALF_ANGLE
        ));
    }

    #[test]
    fn zero_aim_degenerates_to_a_radial_hit() {
        assert!(in_cleave_arc(
            Vec2::ZERO,
            Vec2::ZERO,
            Vec2::new(30.0, 30.0),
            96.0,
            CLEAVE_HALF_ANGLE
        ));
        assert!(!in_cleave_arc(
            Vec2::ZERO,
            Vec2::ZERO,
            Vec2::new(300.0, 300.0),
            96.0,
            CLEAVE_HALF_ANGLE
        ));
    }

    #[test]
    fn combo_stacks_inside_the_window_and_resets_outside() {
        let window = Duration::from_secs_f64(COMBO_WINDOW_SECS);
        assert_eq!(advance_combo(0, None, window), 1);
        assert_eq!(
            advance_combo(1, Some(Duration::from_secs_f64(1.9)), window),
            2
        );
        assert_eq!(
            advance_combo(5, Some(Duration::from_secs_f64(1.0)), window),
            6
        );
        // Just past the window: fresh chain.
        assert_eq!(
            advance_combo(5, Some(Duration::from_secs_f64(2.1)), window),
            1
        );
    }
}
