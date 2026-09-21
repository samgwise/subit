//! subit game client: boots the generated world, moves the player with
//! physics-driven movement, fights the enemy swarm, and publishes game
//! events to the Ensemble hub.

mod bridge;
mod combat;
mod drops;
mod enemies;
mod hud;
mod progression;
mod projectiles;
mod skills;
mod world;

use avian2d::prelude::LinearVelocity;
use bevy::prelude::*;
use skills::GameState;

fn main() {
    App::new()
        .add_plugins((
            DefaultPlugins.set(ImagePlugin::default_nearest()),
            bevy_ecs_tilemap::prelude::TilemapPlugin,
            avian2d::PhysicsPlugins::default(),
            bridge::EnsembleBridgePlugin,
            world::WorldMapPlugin,
            enemies::EnemyPlugin,
            combat::CombatPlugin,
            projectiles::ProjectilePlugin,
            progression::ProgressionPlugin,
            drops::DropsPlugin,
            skills::SkillsPlugin,
            hud::HudPlugin,
        ))
        .add_systems(Startup, setup)
        .add_systems(Update, camera_follow)
        // The player sim pauses while the skills menu is open.
        .add_systems(Update, player_movement.run_if(in_state(GameState::Playing)))
        .run();
}

#[derive(Component)]
pub struct Player;

/// The player sprite's resting tint (also the flash/blink reset colour).
pub const PLAYER_COLOUR: Color = Color::srgb(0.9, 0.9, 0.95);

/// Camera zoom: ortho scale 0.5 renders tiles at 2× their pixel size —
/// roughly a dozen tiles across the window.
const CAMERA_ZOOM: f32 = 0.5;

fn setup(mut commands: Commands) {
    commands.spawn((
        Camera2d,
        Projection::Orthographic(OrthographicProjection {
            scale: CAMERA_ZOOM,
            ..OrthographicProjection::default_2d()
        }),
    ));
}

/// WASD movement direction, normalised; zero when idle.
pub(crate) fn move_direction(input: &ButtonInput<KeyCode>) -> Vec2 {
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

/// Physics-driven WASD movement: write the desired velocity and let the
/// solver resolve wall collisions. An active dash overrides input.
fn player_movement(
    input: Res<ButtonInput<KeyCode>>,
    dash: Res<skills::DashState>,
    mut player: Single<&mut LinearVelocity, With<Player>>,
) {
    if !dash.active.is_finished() {
        player.0 = dash.dir * skills::DASH_SPEED;
        return;
    }
    player.0 = move_direction(&input) * world::PLAYER_SPEED;
}

fn camera_follow(
    player: Single<&Transform, With<Player>>,
    mut camera: Single<&mut Transform, (With<Camera2d>, Without<Player>)>,
) {
    camera.translation = player.translation.with_z(camera.translation.z);
}
