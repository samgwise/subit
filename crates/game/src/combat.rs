//! Combat: mouse-aimed cleave attack, contact damage, and the player health
//! loop (invulnerability window, respawn at the map spawn).
//!
//! The geometry and combo logic are pure functions so they can be tested
//! without spinning up the ECS.

use std::time::Duration;

use avian2d::prelude::{CollidingEntities, LinearVelocity, Position};
use bevy::math::{Isometry2d, Rot2};
use bevy::prelude::*;

use crate::bridge::{BridgeTx, GameAudioEvent};
use crate::enemies::Enemy;
use crate::world::{MapConfig, SpawnPoint, TILE_SIZE, WorldMapRes, tile_units};
use wfc::line_of_sight;

/// Player hit points at full health.
pub(crate) const MAX_HP: i32 = 100;

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

/// How long the player sprite tints red immediately after taking a hit.
const HIT_FLASH_SECS: f32 = 0.15;

/// How long the shield stays up after activation.
const SHIELD_ACTIVE_SECS: f32 = 0.6;

/// Cooldown before the shield can be raised again.
const SHIELD_COOLDOWN_SECS: f32 = 3.0;

/// Radius of the shield ring (and its projectile-reflection reach).
const SHIELD_RADIUS: f32 = TILE_SIZE * 1.2;

/// A once-off timer that starts already expired (no grace period).
fn expired(secs: f32) -> Timer {
    let mut timer = Timer::from_seconds(secs, TimerMode::Once);
    timer.tick(Duration::from_secs_f64(secs as f64 + 1.0));
    timer
}

/// Player health state plus the contact-damage invulnerability window.
#[derive(Resource, Debug)]
pub struct PlayerVitals {
    pub hp: i32,
    invuln: Timer,
}

impl PlayerVitals {
    /// Apply damage, respecting the invulnerability window. Returns whether
    /// the hit landed (false while invulnerable).
    pub fn damage(&mut self, amount: i32) -> bool {
        if !self.invuln.is_finished() {
            return false;
        }
        self.invuln.reset();
        self.hp -= amount;
        true
    }
}

/// Shield state: a short reflection window on activation, then a cooldown
/// before it can be raised again.
#[derive(Resource, Debug)]
pub struct PlayerShield {
    pub active: Timer,
    cooldown: Timer,
}

impl PlayerShield {
    /// True while the shield is up (blocking damage, reflecting shots).
    pub fn is_active(&self) -> bool {
        !self.active.is_finished()
    }
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

impl ComboState {
    /// Register a kill at `now`, extending or starting the chain.
    pub fn register_kill(&mut self, now: Duration) {
        // Compare against the time SINCE the last kill, not the raw
        // timestamp — the timestamp would only chain kills inside the first
        // window's worth of game time.
        let since_last_kill = self.last_kill.map(|last| now.saturating_sub(last));
        self.count = advance_combo(
            self.count,
            since_last_kill,
            Duration::from_secs_f64(COMBO_WINDOW_SECS),
        );
        self.last_kill = Some(now);
    }
}

/// Transient cleave visual: a cone flash matching the actual hit area.
#[derive(Resource, Debug, Default)]
struct CleaveFx(Option<CleaveFxActive>);

#[derive(Debug)]
struct CleaveFxActive {
    origin: Vec2,
    aim: Vec2,
    timer: Timer,
}

pub struct CombatPlugin;

impl Plugin for CombatPlugin {
    fn build(&self, app: &mut App) {
        // Timers that gate availability start expired: no grace period on
        // spawn (the first hit lands, the shield starts down, the first
        // click works).
        app.insert_resource(PlayerVitals {
            hp: MAX_HP,
            invuln: expired(INVULN_SECS),
        })
        .insert_resource(CleaveCooldown(expired(ATTACK_COOLDOWN_SECS)))
        .insert_resource(PlayerShield {
            active: expired(SHIELD_ACTIVE_SECS),
            cooldown: expired(SHIELD_COOLDOWN_SECS),
        })
        .init_resource::<ComboState>()
        .init_resource::<CleaveFx>()
        .add_systems(
            Update,
            (
                player_attack,
                cleave_fx,
                update_shield,
                player_vitals_fx,
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
    map: Res<WorldMapRes>,
    config: Res<MapConfig>,
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
            aim,
            timer: Timer::from_seconds(ATTACK_FX_SECS, TimerMode::Once),
        }));

        let (width, height) = (map.map.grid.width(), map.map.grid.height());
        let origin_units = tile_units((width, height), config.tile_size, origin);
        let origin_units = (origin_units.x, origin_units.y);
        let killed: Vec<Entity> = enemies
            .iter()
            .filter(|(_, pos)| {
                in_cleave_arc(origin, aim, pos.0, CLEAVE_RADIUS, CLEAVE_HALF_ANGLE)
                    && line_of_sight(&map.map.grid, &map.prototypes, origin_units, {
                        let units = tile_units((width, height), config.tile_size, pos.0);
                        (units.x, units.y)
                    })
            })
            .map(|(entity, _)| entity)
            .collect();
        for entity in &killed {
            if let Ok(mut entity_commands) = commands.get_entity(*entity) {
                entity_commands.despawn();
            }
        }

        if !killed.is_empty() {
            combo.register_kill(time.elapsed());
            bridge.send(GameAudioEvent::MobSweep {
                kill_count: killed.len() as u32,
                combo: combo.count,
            });
            tracing::info!(kill_count = killed.len(), combo = combo.count, "mob swept");
        }
    }
}

/// Rotation that centres a `GizmoBuffer::arc_2d` arc on `aim`. Gizmo arcs
/// sweep counter-clockwise starting from `Vec2::Y`, so the arc covers
/// [aim − half, aim + half] when rotated by aim − π/2 − half.
fn cone_rotation(aim: Vec2, half_angle: f32) -> f32 {
    aim.y.atan2(aim.x) - core::f32::consts::FRAC_PI_2 - half_angle
}

/// Draw the cleave flash while it is active — the wedge outline of the
/// actual hit area, not a circle.
fn cleave_fx(mut fx: ResMut<CleaveFx>, time: Res<Time>, mut gizmos: Gizmos) {
    let Some(active) = fx.0.as_mut() else {
        return;
    };
    active.timer.tick(time.delta());
    if active.timer.is_finished() {
        *fx = CleaveFx(None);
        return;
    }

    let colour = Color::srgba(0.4, 0.9, 1.0, 0.8);
    let aim_angle = active.aim.y.atan2(active.aim.x);
    if active.aim == Vec2::ZERO {
        // Degenerate aim: the hit test treats it as radial, so flash radial.
        gizmos.circle_2d(active.origin, CLEAVE_RADIUS, colour);
        return;
    }
    let isometry = Isometry2d::new(
        active.origin,
        Rot2::radians(cone_rotation(active.aim, CLEAVE_HALF_ANGLE)),
    );
    gizmos.arc_2d(isometry, CLEAVE_HALF_ANGLE * 2.0, CLEAVE_RADIUS, colour);
    for edge_angle in [aim_angle - CLEAVE_HALF_ANGLE, aim_angle + CLEAVE_HALF_ANGLE] {
        gizmos.line_2d(
            active.origin,
            active.origin + Vec2::from_angle(edge_angle) * CLEAVE_RADIUS,
            colour,
        );
    }
}

/// Make the player's state legible: red flash on the frame of a hit, then a
/// dim blink for the rest of the invulnerability window.
fn player_vitals_fx(
    mut player: Single<&mut Sprite, With<crate::Player>>,
    vitals: Res<PlayerVitals>,
    time: Res<Time>,
) {
    if vitals.invuln.is_finished() {
        player.color = crate::PLAYER_COLOUR;
    } else if INVULN_SECS - vitals.invuln.elapsed_secs() < HIT_FLASH_SECS {
        player.color = Color::srgba(1.0, 0.25, 0.25, 0.9);
    } else {
        let lit = ((time.elapsed_secs() * 12.0) as i64) % 2 == 0;
        player.color = Color::srgba(0.9, 0.9, 0.95, if lit { 0.9 } else { 0.35 });
    }
}

/// Raise the shield on right-mouse while off cooldown, and draw its ring
/// while it is up.
fn update_shield(
    time: Res<Time>,
    mouse: Res<ButtonInput<MouseButton>>,
    mut shield: ResMut<PlayerShield>,
    player: Single<&Position, With<crate::Player>>,
    mut gizmos: Gizmos,
) {
    shield.active.tick(time.delta());
    shield.cooldown.tick(time.delta());
    if mouse.just_pressed(MouseButton::Right) && shield.cooldown.is_finished() {
        shield.active.reset();
        shield.cooldown.reset();
    }
    if shield.is_active() {
        gizmos.circle_2d(player.0, SHIELD_RADIUS, Color::srgba(0.4, 0.9, 1.0, 0.5));
    }
}

/// Enemies touching the player hurt it, subject to the shield and the
/// invulnerability window; death respawns the player at the map spawn with
/// full health.
fn contact_damage(
    time: Res<Time>,
    mut vitals: ResMut<PlayerVitals>,
    shield: Res<PlayerShield>,
    mut combo: ResMut<ComboState>,
    spawn_point: Res<SpawnPoint>,
    player: Single<(&CollidingEntities, &mut Position, &mut LinearVelocity), With<crate::Player>>,
    enemies: Query<Entity, With<Enemy>>,
) {
    vitals.invuln.tick(time.delta());
    let (colliding, mut position, mut velocity) = player.into_inner();

    let touching_enemy = colliding.iter().any(|entity| enemies.contains(*entity));
    if !touching_enemy || shield.is_active() || !vitals.damage(CONTACT_DAMAGE) {
        return;
    }
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

    #[test]
    fn damage_respects_the_invulnerability_window() {
        let mut vitals = PlayerVitals {
            hp: MAX_HP,
            invuln: Timer::from_seconds(INVULN_SECS, TimerMode::Once),
        };
        // The window starts unexpired: the first hit is blocked...
        assert!(!vitals.damage(CONTACT_DAMAGE));
        // ...until it runs out.
        vitals
            .invuln
            .tick(Duration::from_secs_f64(INVULN_SECS as f64 + 0.01));
        assert!(vitals.damage(CONTACT_DAMAGE));
        assert_eq!(vitals.hp, MAX_HP - CONTACT_DAMAGE);
        // A second hit inside the new window is blocked too.
        assert!(!vitals.damage(CONTACT_DAMAGE));
    }

    #[test]
    fn shield_blocks_and_then_recovers() {
        let mut shield = PlayerShield {
            active: expired(SHIELD_ACTIVE_SECS),
            cooldown: expired(SHIELD_COOLDOWN_SECS),
        };
        // Fresh state: shield down, cooldown ready.
        assert!(!shield.is_active());
        assert!(shield.cooldown.is_finished());
        // Raise: active, and stays active through the window.
        shield.active.reset();
        shield.cooldown.reset();
        assert!(shield.is_active());
        shield
            .active
            .tick(Duration::from_secs_f64(SHIELD_ACTIVE_SECS as f64 + 0.01));
        assert!(!shield.is_active());
        // Cooldown still running: not ready again.
        shield
            .cooldown
            .tick(Duration::from_secs_f64(SHIELD_COOLDOWN_SECS as f64 + 0.01));
        assert!(shield.cooldown.is_finished());
    }

    #[test]
    fn register_kill_extends_and_restarts_the_chain() {
        let mut combo = ComboState::default();
        combo.register_kill(Duration::ZERO);
        assert_eq!(combo.count, 1);
        combo.register_kill(Duration::from_secs_f64(1.0));
        assert_eq!(combo.count, 2);
        // Past the window: fresh chain.
        combo.register_kill(Duration::from_secs_f64(5.0));
        assert_eq!(combo.count, 1);
    }

    #[test]
    fn cone_rotation_centres_the_arc_on_the_aim() {
        let half = CLEAVE_HALF_ANGLE;
        // Arc angles covered by an arc rotated by `cone_rotation`: the arc
        // sweeps CCW from Vec2::Y (π/2) plus the rotation.
        let span = |rotation: f32| {
            (core::f32::consts::FRAC_PI_2 + rotation).rem_euclid(core::f32::consts::TAU)
        };
        // Aiming +X: the span must start at −half (i.e. TAU − half).
        assert!(
            (span(cone_rotation(Vec2::X, half)) - (core::f32::consts::TAU - half)).abs() < 1e-6
        );
        // Aiming +Y: the span starts at π/2 − half.
        assert!(
            (span(cone_rotation(Vec2::Y, half)) - (core::f32::consts::FRAC_PI_2 - half)).abs()
                < 1e-6
        );
        // Aiming −X: the span starts at π − half.
        assert!(
            (span(cone_rotation(Vec2::NEG_X, half)) - (core::f32::consts::PI - half)).abs() < 1e-6
        );
    }
}
