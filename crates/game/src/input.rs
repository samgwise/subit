//! Input: twin-stick controller support layered beside the mouse and
//! keyboard. Two shared resources — [`MoveIntent`] (movement) and [`Aim`]
//! (a world-space aim direction) — are written every frame from whichever
//! device is live, and the combat systems read them instead of unprojecting
//! the cursor inline. Button edges are read per-system straight off the
//! pad's own digital `ButtonInput` (`Gamepad::digital`), OR-ed with the
//! existing key and mouse checks.
//!
//! The writers run in `PreUpdate`, so every `Update` consumer reads values
//! from the current frame without any explicit ordering coupling.

use std::time::Duration;

use avian2d::prelude::Position;
use bevy::input::gamepad::{Gamepad, GamepadAxis, GamepadButton, GamepadInput};
use bevy::prelude::*;

/// Which device last produced aim input.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
enum AimSource {
    #[default]
    None,
    Mouse,
    Pad,
}

/// While the pad owns the aim and the stick is centred, the aim drifts
/// toward the direction of travel at this angular rate (radians per
/// second) — the character naturally faces where they're heading, so
/// re-engaging the trigger starts near where you expect.
const AIM_DRIFT_RAD_PER_SEC: f32 = 8.0;

/// World-space aim direction, updated every frame from the live device.
/// Zero until some device claims it (a fresh run with no mouse movement and
/// a centred stick degenerates to radial cleaves until one does).
#[derive(Resource, Debug, Default)]
pub struct Aim {
    pub dir: Vec2,
    source: AimSource,
}

impl Aim {
    /// Whether the pad currently owns the aim — a stick player has no
    /// cursor on screen and needs an aim indicator of their own.
    pub fn is_pad(&self) -> bool {
        self.source == AimSource::Pad
    }
}

/// Movement direction this frame: keyboard and left stick merged.
#[derive(Resource, Debug, Default)]
pub struct MoveIntent(pub Vec2);

/// Stick magnitude treated as neutral.
const STICK_DEADZONE: f32 = 0.15;

pub struct InputPlugin;

impl Plugin for InputPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<Aim>()
            .init_resource::<MoveIntent>()
            // Chained: the aim drift reads this frame's travel direction.
            .add_systems(PreUpdate, (update_move_intent, update_aim).chain());
    }
}

/// Radial deadzone for a stick pair: below the deadzone the stick reads as
/// neutral, above it the magnitude is rescaled so the edge of the zone (not
/// of the physical stick's travel) is the true zero point — no snap from
/// neutral as it crosses.
fn radial_deadzone(raw: Vec2, deadzone: f32) -> Vec2 {
    let length = raw.length();
    if length <= deadzone {
        return Vec2::ZERO;
    }
    let rescaled = ((length - deadzone) / (1.0 - deadzone)).min(1.0);
    raw / length * rescaled
}

/// Merge the keyboard direction and the left stick: magnitudes add (a
/// half-tilted stick walks), clamped to unit length.
fn merge_move(keyboard: Vec2, stick: Vec2) -> Vec2 {
    let merged = keyboard + stick;
    if merged.length_squared() > 1.0 {
        merged.normalize()
    } else {
        merged
    }
}

/// Arbitrate the aim: a displaced right stick owns the aim outright (and
/// holds its last direction once recentred); a live or just-moved mouse
/// takes it back; otherwise the previous aim stands.
fn pick_aim(
    prev: Vec2,
    source: AimSource,
    mouse_moved: bool,
    mouse_dir: Vec2,
    stick: Vec2,
) -> (Vec2, AimSource) {
    if stick != Vec2::ZERO {
        return (stick.normalize_or_zero(), AimSource::Pad);
    }
    // A mouse that physically moved always claims the aim back; a live one
    // keeps tracking while still, because the cursor's world point drifts
    // as the player walks under it.
    if mouse_dir != Vec2::ZERO && (mouse_moved || source == AimSource::Mouse) {
        return (mouse_dir.normalize_or_zero(), AimSource::Mouse);
    }
    (prev, source)
}

/// One frame of aim drift: with the stick centred and the player
/// travelling, the aim swings toward the travel direction at a fixed
/// angular rate and settles there. No travel (or no aim) leaves it alone.
fn drift_aim(aim: Vec2, travel: Vec2, delta: Duration) -> Vec2 {
    if travel == Vec2::ZERO || aim == Vec2::ZERO {
        return aim;
    }
    let step = AIM_DRIFT_RAD_PER_SEC * delta.as_secs_f32();
    let aim_angle = aim.y.atan2(aim.x);
    // The shortest signed rotation toward the travel direction.
    let toward = (travel.y.atan2(travel.x) - aim_angle).rem_euclid(core::f32::consts::TAU);
    let toward = if toward > core::f32::consts::PI {
        toward - core::f32::consts::TAU
    } else {
        toward
    };
    Vec2::from_angle(aim_angle + toward.clamp(-step, step))
}

/// Poll the first connected pad's sticks, deadzoned: (left, right).
/// All-zero when no pad is connected.
fn stick_pair(pads: &Query<&Gamepad>) -> (Vec2, Vec2) {
    let Some(pad) = pads.iter().next() else {
        return (Vec2::ZERO, Vec2::ZERO);
    };
    let axis = |a: GamepadAxis| pad.get(GamepadInput::Axis(a)).unwrap_or(0.0);
    let left = Vec2::new(axis(GamepadAxis::LeftStickX), axis(GamepadAxis::LeftStickY));
    let right = Vec2::new(
        axis(GamepadAxis::RightStickX),
        axis(GamepadAxis::RightStickY),
    );
    (
        radial_deadzone(left, STICK_DEADZONE),
        radial_deadzone(right, STICK_DEADZONE),
    )
}

/// Whether any connected pad just pressed any of `buttons` (digital edges).
pub fn just_pressed(pads: &Query<&Gamepad>, buttons: &[GamepadButton]) -> bool {
    pads.iter()
        .any(|pad| buttons.iter().any(|b| pad.digital().just_pressed(*b)))
}

/// Write `MoveIntent` from WASD plus the left stick.
fn update_move_intent(
    input: Res<ButtonInput<KeyCode>>,
    pads: Query<&Gamepad>,
    mut intent: ResMut<MoveIntent>,
) {
    let (left, _) = stick_pair(&pads);
    intent.0 = merge_move(crate::move_direction(&input), left);
}

/// Write `Aim` from the mouse cursor and the right stick.
#[allow(clippy::type_complexity, clippy::too_many_arguments)]
fn update_aim(
    pads: Query<&Gamepad>,
    intent: Res<MoveIntent>,
    time: Res<Time>,
    player: Single<&Position, With<crate::Player>>,
    window: Single<&Window>,
    camera: Single<(&Camera, &GlobalTransform), With<Camera2d>>,
    mut aim: ResMut<Aim>,
    mut last_cursor: Local<Option<Vec2>>,
) {
    let (_, right) = stick_pair(&pads);
    let cursor = window.cursor_position();
    // The mouse produced input when its cursor moved (a still cursor gives
    // the stick room to own the aim).
    let mouse_moved = match (*last_cursor, cursor) {
        (Some(prev), Some(cur)) => prev != cur,
        (None, Some(_)) => true,
        _ => false,
    };
    *last_cursor = cursor;
    let mouse_dir = cursor
        .and_then(|cursor| camera.0.viewport_to_world_2d(camera.1, cursor).ok())
        .map_or(Vec2::ZERO, |world| world - player.0);
    let (mut dir, source) = pick_aim(aim.dir, aim.source, mouse_moved, mouse_dir, right);
    // Controller mode only: with the stick centred and the player moving,
    // the aim sweeps toward the direction of travel.
    if source == AimSource::Pad && right == Vec2::ZERO {
        dir = drift_aim(dir, intent.0, time.delta());
    }
    aim.dir = dir;
    aim.source = source;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_deadzone_rescales_from_its_edge() {
        let d = STICK_DEADZONE;
        // Inside the zone: neutral, direction irrelevant.
        assert_eq!(radial_deadzone(Vec2::new(d * 0.9, 0.0), d), Vec2::ZERO);
        assert_eq!(radial_deadzone(Vec2::new(0.1, -0.1), d), Vec2::ZERO);
        // At the zone's edge: the true zero point.
        let edge = radial_deadzone(Vec2::new(d + 0.01, 0.0), d);
        assert!(edge.x > 0.0 && edge.x < 0.1);
        // Full deflection: unit length, direction kept.
        let full = radial_deadzone(Vec2::new(0.0, -1.0), d);
        assert!((full.length() - 1.0).abs() < 1e-5);
        assert!(full.y < 0.0);
    }

    #[test]
    fn movement_adds_and_clamps() {
        // Pure keyboard: normalised, as `move_direction` always produced.
        assert!((merge_move(Vec2::Y, Vec2::ZERO).length() - 1.0).abs() < 1e-5);
        // A half-tilted stick walks.
        let half = merge_move(Vec2::ZERO, Vec2::new(0.5, 0.0));
        assert!((half.x - 0.5).abs() < 1e-5);
        // Keyboard plus stick beyond unit: clamped back to one.
        let clamped = merge_move(Vec2::Y, Vec2::new(0.0, 0.75));
        assert!((clamped.length() - 1.0).abs() < 1e-5);
        // Both idle: still.
        assert_eq!(merge_move(Vec2::ZERO, Vec2::ZERO), Vec2::ZERO);
    }

    #[test]
    fn the_stick_owns_the_aim_until_it_recentres() {
        let stick = Vec2::new(0.8, -0.6);
        // Stick displaced: it wins, direction normalised.
        let (dir, source) = pick_aim(Vec2::ZERO, AimSource::None, false, Vec2::X, stick);
        assert_eq!(source, AimSource::Pad);
        assert!((dir.length() - 1.0).abs() < 1e-5);
        // Stick recentres, mouse untouched: the last pad aim HOLDS (no
        // snap back).
        let (dir, source) = pick_aim(dir, AimSource::Pad, false, Vec2::X, Vec2::ZERO);
        assert_eq!(source, AimSource::Pad);
        assert!((dir.length() - 1.0).abs() < 1e-5);
        // A moved mouse claims the aim back.
        let (dir, source) = pick_aim(dir, AimSource::Pad, true, Vec2::Y, Vec2::ZERO);
        assert_eq!(source, AimSource::Mouse);
        assert_eq!(dir, Vec2::Y);
        // An unmoved mouse does not steal it.
        let (_, source) = pick_aim(Vec2::Y, AimSource::Pad, false, Vec2::X, Vec2::ZERO);
        assert_eq!(source, AimSource::Pad);
    }

    #[test]
    fn the_aim_drifts_toward_travel_when_the_stick_rests() {
        let delta = Duration::from_secs_f64(0.1); // 0.8 rad per step
        // Aim east, heading north: the aim sweeps toward north...
        let drifted = drift_aim(Vec2::X, Vec2::Y, delta);
        assert!(drifted.x > 0.0 && drifted.y > 0.0, "swept toward travel");
        assert!((drifted.length() - 1.0).abs() < 1e-5);
        // ...and settles ON the travel direction given enough time.
        let settled = drift_aim(Vec2::X, Vec2::Y, Duration::from_secs_f64(1.0));
        assert!((settled - Vec2::Y).length() < 1e-4);
        // No travel: the aim holds.
        assert_eq!(drift_aim(Vec2::X, Vec2::ZERO, delta), Vec2::X);
        // No aim yet: nothing to drift.
        assert_eq!(drift_aim(Vec2::ZERO, Vec2::Y, delta), Vec2::ZERO);
        // The long way around is never taken: one step from north-west to
        // east sweeps clockwise through north, not the other way.
        let nw = Vec2::new(-1.0, 1.0).normalize();
        let swept = drift_aim(nw, Vec2::new(1.0, -1.0), Duration::from_secs_f64(0.01));
        assert!(swept.y < nw.y, "shortest arc, not the long way");
    }

    #[test]
    fn a_fresh_run_waits_for_the_first_device() {
        // No movement, no cursor yet: aim stays zero.
        let (dir, _) = pick_aim(Vec2::ZERO, AimSource::None, false, Vec2::ZERO, Vec2::ZERO);
        assert_eq!(dir, Vec2::ZERO);
        // The first mouse move claims it.
        let (dir, source) =
            pick_aim(Vec2::ZERO, AimSource::None, true, Vec2::new(2.0, 0.0), Vec2::ZERO);
        assert_eq!(source, AimSource::Mouse);
        assert_eq!(dir, Vec2::X);
    }
}
