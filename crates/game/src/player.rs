//! The player body: the neon camera pass, WASD movement, and camera follow.

use crate::skills;
use avian2d::prelude::LinearVelocity;
use bevy::post_process::bloom::Bloom;
use bevy::prelude::*;

#[derive(Component)]
pub struct Player;

/// The player sprite's resting tint (also the flash/blink reset colour).
pub const PLAYER_COLOUR: Color = Color::srgb(0.9, 0.9, 0.95);

/// Camera zoom: ortho scale 0.5 renders tiles at 2× their pixel size —
/// roughly a dozen tiles across the window.
const CAMERA_ZOOM: f32 = 0.5;

pub(crate) fn setup(mut commands: Commands) {
    commands.spawn((
        Camera2d,
        Projection::Orthographic(OrthographicProjection {
            scale: CAMERA_ZOOM,
            ..OrthographicProjection::default_2d()
        }),
        // Neon pass: bright pixels (lit wall edges, beacon, terminals, the
        // cleave flash, shots) bleed glow; the dark tiles stay dark. The
        // OLD_SCHOOL preset is additive with a ~0.6 threshold — the stylised
        // look; NATURAL would haze the whole scene. The preset's intensity
        // (0.05) is too shy for Tron; 0.15 keeps the glow pronounced
        // without washing out the dark tiles.
        Bloom {
            intensity: 0.15,
            ..Bloom::OLD_SCHOOL
        },
    ));
}

/// Keyboard movement direction (the keyboard half of the shared move
/// intent), normalised; zero when idle.
pub fn move_direction(input: &ButtonInput<KeyCode>) -> Vec2 {
    let mut direction = Vec2::ZERO;
    if input.pressed(KeyCode::KeyW) {
        direction.y += 1.0;
    }
    if input.pressed(KeyCode::KeyS) {
        direction.y -= 1.0;
    }
    if input.pressed(KeyCode::KeyA) {
        direction.x -= 1.0;
    }
    if input.pressed(KeyCode::KeyD) {
        direction.x += 1.0;
    }
    if direction != Vec2::ZERO {
        direction = direction.normalize();
    }
    direction
}

/// Physics-driven movement: write the desired velocity (the shared move
/// intent — keyboard and left stick merged, see input) and let the solver
/// resolve wall collisions. An active dash overrides input.
pub(crate) fn player_movement(
    intent: Res<crate::input::MoveIntent>,
    dash: Res<skills::DashState>,
    mut player: Single<&mut LinearVelocity, With<Player>>,
) {
    if !dash.active.is_finished() {
        player.0 = dash.dir * skills::DASH_SPEED;
        return;
    }
    player.0 = intent.0 * crate::world::PLAYER_SPEED;
}

pub(crate) fn camera_follow(
    player: Single<&Transform, With<Player>>,
    mut camera: Single<&mut Transform, (With<Camera2d>, Without<Player>)>,
) {
    camera.translation = player.translation.with_z(camera.translation.z);
}
