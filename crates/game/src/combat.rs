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
use crate::enemies::{Enemy, Health, Shield, Thrower, absorb_damage, kill_enemy, regrow_plate};
use crate::skills::{CloakState, DashState, cleave_damage, cleave_half_angle, cleave_radius};
use crate::world::{MapConfig, SpawnPoint, TILE_SIZE, WorldMapRes, tile_units};
use wfc::line_of_sight;

/// Player hit points at full health, before vitality upgrades.
pub(crate) const BASE_MAX_HP: i32 = 100;

/// Damage dealt by one enemy contact.
const CONTACT_DAMAGE: i32 = 30;

/// Contact damage is applied at most once per window, so brushing a swarm
/// cannot delete the player in a single frame.
const INVULN_SECS: f32 = 0.5;

/// Seconds between cleave swings.
const ATTACK_COOLDOWN_SECS: f32 = 0.25;

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

/// Radius of the shield ring — the full reflect reach while active.
pub(crate) const SHIELD_RADIUS: f32 = TILE_SIZE * 1.2;

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
    pub max_hp: i32,
    invuln: Timer,
}

impl PlayerVitals {
    /// Open the invulnerability window now (dash i-frames, respawn and
    /// depth-descent grace).
    pub fn reset_invuln(&mut self) {
        self.invuln.reset();
    }
}

/// Plates on the unlockable barrier once purchased.
pub(crate) const BARRIER_PLATES: u32 = 10;

/// Unlockable regenerating barrier: a plate pool that soaks damage before
/// health — the player's version of the enemy shields. The pool stays
/// empty (inert) until the skill is purchased.
#[derive(Resource, Debug)]
pub struct Barrier {
    pub plates: u32,
    pub regen: Timer,
}

/// Route a player hit through the barrier then health: the barrier soaks
/// what it can, the rest lands on HP. The invulnerability window gates the
/// whole hit either way. Returns whether the hit landed.
pub fn damage_player(vitals: &mut PlayerVitals, barrier: &mut Barrier, amount: i32) -> bool {
    if !vitals.invuln.is_finished() {
        return false;
    }
    vitals.invuln.reset();
    let Barrier { plates, regen } = barrier;
    let leaked = absorb_damage(plates, regen, amount);
    vitals.hp -= leaked;
    true
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

    /// Raise the shield — it stays up until dropped or its duration runs
    /// out.
    pub fn raise(&mut self) {
        self.active.reset();
    }

    /// Drop the shield: the down state begins, and with it the cooldown —
    /// dropping early (a manual toggle-off or the duration expiring) starts
    /// the re-raise wait immediately. The active timer is finished with a
    /// tick (its `finished` flag only updates on ticks, never on
    /// `set_elapsed`).
    pub fn lower(&mut self) {
        self.active.tick(self.active.duration());
        self.cooldown.reset();
    }

    /// Re-derive the cooldown timer after a menu purchase.
    pub fn set_cooldown_secs(&mut self, secs: f32) {
        self.cooldown
            .set_duration(Duration::from_secs_f64(secs as f64));
    }

    /// Re-derive the active duration after a menu purchase.
    pub fn set_active_secs(&mut self, secs: f32) {
        self.active
            .set_duration(Duration::from_secs_f64(secs as f64));
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

/// Damage the nova burst deals within its radius.
const NOVA_DAMAGE: i32 = 150;
/// Nova reach in world units (2.5 tiles).
const NOVA_RADIUS: f32 = TILE_SIZE * 2.5;
/// Seconds between novas.
const NOVA_COOLDOWN_SECS: f32 = 6.0;
/// Knockback speed kicked into caught enemies, decaying in `enemies.rs`.
const NOVA_KNOCKBACK: f32 = 500.0;
/// How long the expanding nova ring stays on screen.
const NOVA_FX_SECS: f32 = 0.3;

/// Nova ability cooldown.
#[derive(Resource, Debug)]
struct NovaState(Timer);

/// Transient nova visual: an expanding ring from the blast centre.
#[derive(Resource, Debug, Default)]
struct NovaFx(Option<(Vec2, Timer)>);

pub struct CombatPlugin;

impl Plugin for CombatPlugin {
    fn build(&self, app: &mut App) {
        // Timers that gate availability start expired: no grace period on
        // spawn (the first hit lands, the shield starts down, the first
        // click works).
        app.insert_resource(PlayerVitals {
            hp: BASE_MAX_HP,
            max_hp: BASE_MAX_HP,
            invuln: expired(INVULN_SECS),
        })
        .insert_resource(CleaveCooldown(expired(ATTACK_COOLDOWN_SECS)))
        .insert_resource(PlayerShield {
            active: expired(SHIELD_ACTIVE_SECS),
            cooldown: expired(SHIELD_COOLDOWN_SECS),
        })
        .insert_resource(Barrier {
            plates: 0,
            regen: expired(crate::enemies::PLATE_REGEN_SECS),
        })
        .insert_resource(NovaState(expired(NOVA_COOLDOWN_SECS)))
        .init_resource::<ComboState>()
        .init_resource::<CleaveFx>()
        .init_resource::<NovaFx>()
        .add_systems(
            Update,
            (
                player_attack,
                cleave_fx,
                update_shield,
                nova_trigger,
                nova_fx,
                cloak_trigger,
                dash_trigger,
                player_vitals_fx,
                cloak_fx.after(player_vitals_fx),
                barrier_regen,
                contact_damage,
                player_speed_telemetry,
            )
                .run_if(in_state(crate::skills::GameState::Playing)),
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
#[allow(clippy::too_many_arguments, clippy::type_complexity)]
fn player_attack(
    mut commands: Commands,
    mut cooldown: ResMut<CleaveCooldown>,
    mut combo: ResMut<ComboState>,
    mut fx: ResMut<CleaveFx>,
    mut cloak: ResMut<CloakState>,
    bridge: Res<BridgeTx>,
    time: Res<Time>,
    mouse: Res<ButtonInput<MouseButton>>,
    window: Single<&Window>,
    camera: Single<(&Camera, &GlobalTransform), With<Camera2d>>,
    player: Single<&Position, With<crate::Player>>,
    mut enemies: Query<
        (
            Entity,
            &Position,
            Option<&Thrower>,
            &mut Health,
            Option<&mut Shield>,
        ),
        With<Enemy>,
    >,
    map: Res<WorldMapRes>,
    config: Res<MapConfig>,
    levels: Res<crate::skills::SkillLevels>,
    mut rng: ResMut<crate::drops::DropRng>,
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
        cloak.end(); // swinging gives the player away
        bridge.send(GameAudioEvent::AttackPrimary);
        *fx = CleaveFx(Some(CleaveFxActive {
            origin,
            aim,
            timer: Timer::from_seconds(ATTACK_FX_SECS, TimerMode::Once),
        }));

        let (width, height) = (map.map.grid.width(), map.map.grid.height());
        let origin_units = tile_units((width, height), config.tile_size, origin);
        let origin_units = (origin_units.x, origin_units.y);
        let damage = cleave_damage(&levels);
        let radius = cleave_radius(&levels);
        let half_angle = cleave_half_angle(&levels);
        let mut killed = 0usize;
        for (entity, pos, thrower, mut health, mut shield) in &mut enemies {
            if !in_cleave_arc(origin, aim, pos.0, radius, half_angle)
                || !line_of_sight(&map.map.grid, &map.prototypes, origin_units, {
                    let units = tile_units((width, height), config.tile_size, pos.0);
                    (units.x, units.y)
                })
            {
                continue;
            }
            // Shields soak their plates first; the leak lands on health.
            let to_health = match shield.as_mut() {
                Some(shield) => {
                    let Shield { plates, regen } = &mut **shield;
                    absorb_damage(plates, regen, damage)
                }
                None => damage,
            };
            health.hp -= to_health;
            if health.hp <= 0 {
                killed += 1;
                kill_enemy(&mut commands, entity, pos.0, thrower.is_some(), &mut rng.0);
            }
        }

        if killed > 0 {
            combo.register_kill(time.elapsed());
            bridge.send(GameAudioEvent::MobSweep {
                kill_count: killed as u32,
                combo: combo.count,
            });
            tracing::info!(kill_count = killed, combo = combo.count, "mob swept");
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
fn cleave_fx(
    mut fx: ResMut<CleaveFx>,
    levels: Res<crate::skills::SkillLevels>,
    time: Res<Time>,
    mut gizmos: Gizmos,
) {
    let Some(active) = fx.0.as_mut() else {
        return;
    };
    active.timer.tick(time.delta());
    if active.timer.is_finished() {
        *fx = CleaveFx(None);
        return;
    }

    let radius = cleave_radius(&levels);
    let half_angle = cleave_half_angle(&levels);
    let colour = Color::srgba(0.4, 0.9, 1.0, 0.8);
    let aim_angle = active.aim.y.atan2(active.aim.x);
    if active.aim == Vec2::ZERO {
        // Degenerate aim: the hit test treats it as radial, so flash radial.
        gizmos.circle_2d(active.origin, radius, colour);
        return;
    }
    let isometry = Isometry2d::new(
        active.origin,
        Rot2::radians(cone_rotation(active.aim, half_angle)),
    );
    gizmos.arc_2d(isometry, half_angle * 2.0, radius, colour);
    for edge_angle in [aim_angle - half_angle, aim_angle + half_angle] {
        gizmos.line_2d(
            active.origin,
            active.origin + Vec2::from_angle(edge_angle) * radius,
            colour,
        );
    }
}

/// Trigger the dash on Space: unlocked, off cooldown, with a direction from
/// the current WASD input falling back to the cursor. The dash doubles as
/// i-frames — it resets the invulnerability window, so the blink shows it.
#[allow(clippy::too_many_arguments)]
fn dash_trigger(
    input: Res<ButtonInput<KeyCode>>,
    unlocks: Res<crate::skills::AbilityUnlocks>,
    mut dash: ResMut<DashState>,
    mut vitals: ResMut<PlayerVitals>,
    bridge: Res<BridgeTx>,
    time: Res<Time>,
    window: Single<&Window>,
    camera: Single<(&Camera, &GlobalTransform), With<Camera2d>>,
    player: Single<&Position, With<crate::Player>>,
) {
    dash.active.tick(time.delta());
    dash.cooldown.tick(time.delta());
    if !input.just_pressed(KeyCode::Space) || !unlocks.dash || !dash.cooldown.is_finished() {
        return;
    }

    // Direction: current WASD input, falling back to the cursor direction.
    let mut dir = crate::move_direction(&input);
    if dir == Vec2::ZERO
        && let Some(cursor) = window.cursor_position()
        && let Ok(cursor_world) = camera.0.viewport_to_world_2d(camera.1, cursor)
    {
        dir = (cursor_world - player.0).normalize_or_zero();
    }
    if dir == Vec2::ZERO {
        return; // standing still with no cursor — nothing to dash along
    }

    dash.dir = dir;
    dash.active.reset();
    dash.cooldown.reset();
    vitals.invuln.reset();
    bridge.send(GameAudioEvent::Dash);
    tracing::info!(dir = ?dash.dir, "dash");
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

/// Right-mouse toggles the shield: click up (it stays up for at most the
/// duration skill's window), click again to drop it early — the cooldown
/// starts the moment it's down. While it is up the reflect dome (spawned
/// and toggled by shield_fx) shows the reach.
fn update_shield(
    time: Res<Time>,
    mouse: Res<ButtonInput<MouseButton>>,
    mut shield: ResMut<PlayerShield>,
) {
    if shield.is_active() {
        shield.active.tick(time.delta());
        // A second click drops it early; the duration ends it regardless —
        // either way the cooldown begins now.
        if shield.active.is_finished() || mouse.just_pressed(MouseButton::Right) {
            shield.lower();
        }
    } else {
        shield.cooldown.tick(time.delta());
        if mouse.just_pressed(MouseButton::Right) && shield.cooldown.is_finished() {
            shield.raise();
        }
    }
}

/// C toggles the cloak: engage while off cooldown (enemies then evaluate
/// their aggro with the sneak range — see the aggro gate — and the
/// player's sprite dims), press again to drop it early. The duration ends
/// it regardless, and the cooldown starts the moment it's down. Attacking
/// breaks it too.
fn cloak_trigger(
    input: Res<ButtonInput<KeyCode>>,
    unlocks: Res<crate::skills::AbilityUnlocks>,
    mut cloak: ResMut<CloakState>,
    time: Res<Time>,
    bridge: Res<BridgeTx>,
) {
    if cloak.is_active() {
        cloak.active.tick(time.delta());
        if cloak.active.is_finished() || input.just_pressed(KeyCode::KeyC) {
            cloak.end();
        }
        return;
    }
    cloak.cooldown.tick(time.delta());
    if !input.just_pressed(KeyCode::KeyC) || !unlocks.cloak || !cloak.cooldown.is_finished() {
        return;
    }
    cloak.engage();
    bridge.send(GameAudioEvent::Cloak);
    tracing::info!("cloak engaged");
}

/// Dim the player's sprite while cloaked — a slow shimmer over the resting
/// tint. Runs after the vitals blink so the cloak look wins while active.
fn cloak_fx(
    cloak: Res<CloakState>,
    time: Res<Time>,
    mut player: Single<&mut Sprite, With<crate::Player>>,
) {
    if cloak.is_active() {
        let alpha = 0.3 + 0.08 * (time.elapsed_secs() * 6.0).sin();
        let mut colour = crate::PLAYER_COLOUR;
        colour.set_alpha(alpha);
        player.color = colour;
    }
}

/// Fire a nova on E: unlock-gated, off cooldown. A 360-degree burst —
/// damage every enemy in the radius (through their shields), shove each
/// survivor outward, expand a ring, batch the kills into one sweep.
#[allow(clippy::too_many_arguments, clippy::type_complexity)]
fn nova_trigger(
    mut commands: Commands,
    input: Res<ButtonInput<KeyCode>>,
    unlocks: Res<crate::skills::AbilityUnlocks>,
    mut nova: ResMut<NovaState>,
    mut cloak: ResMut<CloakState>,
    time: Res<Time>,
    player: Single<&Position, With<crate::Player>>,
    mut enemies: Query<
        (
            Entity,
            &Position,
            Option<&Thrower>,
            &mut Health,
            Option<&mut Shield>,
        ),
        With<Enemy>,
    >,
    mut combo: ResMut<ComboState>,
    mut fx: ResMut<NovaFx>,
    bridge: Res<BridgeTx>,
    mut rng: ResMut<crate::drops::DropRng>,
) {
    nova.0.tick(time.delta());
    if !input.just_pressed(KeyCode::KeyE) || !unlocks.nova || !nova.0.is_finished() {
        return;
    }
    nova.0.reset();
    cloak.end(); // bursting gives the player away
    bridge.send(GameAudioEvent::Nova);
    *fx = NovaFx(Some((
        player.0,
        Timer::from_seconds(NOVA_FX_SECS, TimerMode::Once),
    )));

    let origin = player.0;
    let mut killed = 0usize;
    for (entity, pos, thrower, mut health, mut shield) in &mut enemies {
        let distance = pos.0.distance(origin);
        if distance > NOVA_RADIUS {
            continue;
        }
        // Shove every caught enemy outward — the knockback decays in the
        // seek systems, so steering takes back over.
        let direction = (pos.0 - origin).normalize_or_zero();
        if direction != Vec2::ZERO
            && let Ok(mut entity_commands) = commands.get_entity(entity)
        {
            entity_commands.insert(crate::enemies::Knockback {
                velocity: direction * NOVA_KNOCKBACK,
            });
        }
        // Shields soak their plates first; the leak lands on health.
        let to_health = match shield.as_mut() {
            Some(shield) => {
                let Shield { plates, regen } = &mut **shield;
                absorb_damage(plates, regen, NOVA_DAMAGE)
            }
            None => NOVA_DAMAGE,
        };
        health.hp -= to_health;
        if health.hp <= 0 {
            killed += 1;
            kill_enemy(&mut commands, entity, pos.0, thrower.is_some(), &mut rng.0);
        }
    }
    if killed > 0 {
        combo.register_kill(time.elapsed());
        bridge.send(GameAudioEvent::MobSweep {
            kill_count: killed as u32,
            combo: combo.count,
        });
    }
    tracing::info!(killed, "nova burst");
}

/// Draw the nova ring while it is active — an expanding circle out to the
/// blast radius.
fn nova_fx(mut fx: ResMut<NovaFx>, time: Res<Time>, mut gizmos: Gizmos) {
    let Some((centre, timer)) = fx.0.as_mut() else {
        return;
    };
    timer.tick(time.delta());
    if timer.is_finished() {
        *fx = NovaFx(None);
        return;
    }
    let progress = timer.elapsed_secs() / NOVA_FX_SECS;
    let radius = NOVA_RADIUS * progress;
    let alpha = 0.8 * (1.0 - progress);
    gizmos.circle_2d(*centre, radius, Color::srgba(0.4, 0.9, 1.0, alpha));
}

/// Regrow barrier plates while the unlock is owned.
fn barrier_regen(
    time: Res<Time>,
    unlocks: Res<crate::skills::AbilityUnlocks>,
    barrier: ResMut<Barrier>,
) {
    if !unlocks.barrier {
        return;
    }
    let Barrier { plates, regen } = barrier.into_inner();
    regrow_plate(plates, regen, BARRIER_PLATES, time.delta());
}

/// Enemies touching the player hurt it, subject to the shield, the barrier
/// and the invulnerability window; death respawns the player at the map
/// spawn with full health.
#[allow(clippy::too_many_arguments)]
fn contact_damage(
    time: Res<Time>,
    mut vitals: ResMut<PlayerVitals>,
    mut barrier: ResMut<Barrier>,
    shield: Res<PlayerShield>,
    mut combo: ResMut<ComboState>,
    spawn_point: Res<SpawnPoint>,
    player: Single<(&CollidingEntities, &mut Position, &mut LinearVelocity), With<crate::Player>>,
    enemies: Query<Entity, With<Enemy>>,
) {
    vitals.invuln.tick(time.delta());
    let (colliding, mut position, mut velocity) = player.into_inner();

    let touching_enemy = colliding.iter().any(|entity| enemies.contains(*entity));
    if !touching_enemy
        || shield.is_active()
        || !damage_player(&mut vitals, &mut barrier, CONTACT_DAMAGE)
    {
        return;
    }
    tracing::info!(hp = vitals.hp, "player hit by enemy contact");

    if vitals.hp <= 0 {
        // Respawn: back to the spawn tile, still and whole, combo lost.
        vitals.hp = vitals.max_hp;
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

    /// A fixed cone for the arc tests — a 90-degree cone at 96 units.
    const TEST_HALF_ANGLE: f32 = 45.0_f32.to_radians();
    const TEST_RADIUS: f32 = 96.0;

    #[test]
    fn cleave_arc_hits_ahead_and_not_behind() {
        let origin = Vec2::ZERO;
        let aim = Vec2::X;
        // Dead ahead, well inside radius and arc.
        assert!(in_cleave_arc(
            origin,
            aim,
            Vec2::new(50.0, 0.0),
            TEST_RADIUS,
            TEST_HALF_ANGLE
        ));
        // Inside radius but behind the aim line.
        assert!(!in_cleave_arc(
            origin,
            aim,
            Vec2::new(-50.0, 0.0),
            TEST_RADIUS,
            TEST_HALF_ANGLE
        ));
        // Inside radius but near-perpendicular to the aim: well outside the
        // cone.
        assert!(!in_cleave_arc(
            origin,
            aim,
            Vec2::new(10.0, 90.0),
            TEST_RADIUS,
            TEST_HALF_ANGLE
        ));
        // Outside radius but inside the arc direction.
        assert!(!in_cleave_arc(
            origin,
            aim,
            Vec2::new(200.0, 0.0),
            TEST_RADIUS,
            TEST_HALF_ANGLE
        ));
    }

    #[test]
    fn cleave_arc_boundary_is_inclusive() {
        let origin = Vec2::ZERO;
        let aim = Vec2::X;
        // Exactly on the cone edge: dot == cos(half_angle) counts as a hit.
        let edge = Vec2::new(TEST_HALF_ANGLE.cos(), TEST_HALF_ANGLE.sin());
        assert!(in_cleave_arc(
            origin,
            aim,
            edge,
            TEST_RADIUS,
            TEST_HALF_ANGLE
        ));
        // Exactly on the radius edge counts as a hit.
        assert!(in_cleave_arc(
            origin,
            aim,
            Vec2::new(TEST_RADIUS, 0.0),
            TEST_RADIUS,
            TEST_HALF_ANGLE
        ));
    }

    #[test]
    fn zero_aim_degenerates_to_a_radial_hit() {
        assert!(in_cleave_arc(
            Vec2::ZERO,
            Vec2::ZERO,
            Vec2::new(30.0, 30.0),
            TEST_RADIUS,
            TEST_HALF_ANGLE
        ));
        assert!(!in_cleave_arc(
            Vec2::ZERO,
            Vec2::ZERO,
            Vec2::new(300.0, 300.0),
            TEST_RADIUS,
            TEST_HALF_ANGLE
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
            hp: BASE_MAX_HP,
            max_hp: BASE_MAX_HP,
            invuln: Timer::from_seconds(INVULN_SECS, TimerMode::Once),
        };
        let mut barrier = Barrier {
            plates: 0,
            regen: expired(crate::enemies::PLATE_REGEN_SECS),
        };
        // The window starts unexpired: the first hit is blocked...
        assert!(!damage_player(&mut vitals, &mut barrier, CONTACT_DAMAGE));
        // ...until it runs out.
        vitals
            .invuln
            .tick(Duration::from_secs_f64(INVULN_SECS as f64 + 0.01));
        assert!(damage_player(&mut vitals, &mut barrier, CONTACT_DAMAGE));
        assert_eq!(vitals.hp, BASE_MAX_HP - CONTACT_DAMAGE);
        // A second hit inside the new window is blocked too.
        assert!(!damage_player(&mut vitals, &mut barrier, CONTACT_DAMAGE));
    }

    #[test]
    fn the_barrier_soaks_hits_before_health() {
        let mut vitals = PlayerVitals {
            hp: BASE_MAX_HP,
            max_hp: BASE_MAX_HP,
            invuln: expired(INVULN_SECS),
        };
        let mut barrier = Barrier {
            plates: 10,
            regen: expired(crate::enemies::PLATE_REGEN_SECS),
        };
        // A light hit never reaches health...
        assert!(damage_player(&mut vitals, &mut barrier, 20));
        assert_eq!(vitals.hp, BASE_MAX_HP);
        assert_eq!(barrier.plates, 10 - 4);
        // The first hit opened the invulnerability window; let it lapse.
        vitals
            .invuln
            .tick(Duration::from_secs_f64(INVULN_SECS as f64 + 0.01));
        // ...and a heavy one leaks the remainder.
        assert!(damage_player(&mut vitals, &mut barrier, 100));
        assert_eq!(
            vitals.hp,
            BASE_MAX_HP - (100 - 6 * crate::enemies::PLATE_HP)
        );
        assert_eq!(barrier.plates, 0);
    }

    #[test]
    fn the_invulnerability_window_gates_barrier_hits_too() {
        let mut vitals = PlayerVitals {
            hp: BASE_MAX_HP,
            max_hp: BASE_MAX_HP,
            invuln: Timer::from_seconds(INVULN_SECS, TimerMode::Once),
        };
        let mut barrier = Barrier {
            plates: 10,
            regen: expired(crate::enemies::PLATE_REGEN_SECS),
        };
        assert!(!damage_player(&mut vitals, &mut barrier, 20));
        assert_eq!(vitals.hp, BASE_MAX_HP);
        // The untouched barrier keeps every plate.
        assert_eq!(barrier.plates, 10);
    }

    #[test]
    fn an_empty_barrier_passes_damage_straight_through() {
        let mut vitals = PlayerVitals {
            hp: BASE_MAX_HP,
            max_hp: BASE_MAX_HP,
            invuln: expired(INVULN_SECS),
        };
        let mut barrier = Barrier {
            plates: 0,
            regen: expired(crate::enemies::PLATE_REGEN_SECS),
        };
        assert!(damage_player(&mut vitals, &mut barrier, CONTACT_DAMAGE));
        assert_eq!(vitals.hp, BASE_MAX_HP - CONTACT_DAMAGE);
    }

    #[test]
    fn shield_toggles_up_and_the_cooldown_starts_at_the_drop() {
        let mut shield = PlayerShield {
            active: expired(SHIELD_ACTIVE_SECS),
            cooldown: expired(SHIELD_COOLDOWN_SECS),
        };
        // Fresh state: shield down, cooldown ready.
        assert!(!shield.is_active());
        assert!(shield.cooldown.is_finished());
        // Raise: up, and stays up while the duration runs.
        shield.raise();
        assert!(shield.is_active());
        shield
            .active
            .tick(Duration::from_secs_f64(SHIELD_ACTIVE_SECS as f64 * 0.5));
        assert!(shield.is_active());
        // The duration ends it; the cooldown begins at the drop.
        shield
            .active
            .tick(Duration::from_secs_f64(SHIELD_ACTIVE_SECS as f64 * 0.6));
        shield.lower();
        assert!(!shield.is_active());
        assert!(!shield.cooldown.is_finished());
        // Ready again once the cooldown runs out.
        shield
            .cooldown
            .tick(Duration::from_secs_f64(SHIELD_COOLDOWN_SECS as f64 + 0.01));
        assert!(shield.cooldown.is_finished());
        shield.raise();
        assert!(shield.is_active());
        // An early toggle-off drops it and still starts the wait.
        shield.lower();
        assert!(!shield.is_active());
        assert!(!shield.cooldown.is_finished());
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
        let half = TEST_HALF_ANGLE;
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
