//! subit game client: boots the generated world, moves the player with
//! physics-driven movement, and publishes game events to the Ensemble hub.

mod bridge;
mod world;

use avian2d::prelude::LinearVelocity;
use bevy::prelude::*;

fn main() {
    App::new()
        .add_plugins((
            DefaultPlugins.set(ImagePlugin::default_nearest()),
            bevy_ecs_tilemap::prelude::TilemapPlugin,
            avian2d::PhysicsPlugins::default(),
            bridge::EnsembleBridgePlugin,
            world::WorldMapPlugin,
        ))
        .add_systems(Startup, setup)
        .add_systems(
            Update,
            (player_movement, camera_follow, bridge::send_test_pulse),
        )
        .run();
}

#[derive(Component)]
pub struct Player;

fn setup(mut commands: Commands) {
    commands.spawn(Camera2d);
}

/// Physics-driven WASD movement: write the desired velocity and let the
/// solver resolve wall collisions.
fn player_movement(
    input: Res<ButtonInput<KeyCode>>,
    mut player: Single<&mut LinearVelocity, With<Player>>,
) {
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
    player.0 = direction * world::PLAYER_SPEED;
}

fn camera_follow(
    player: Single<&Transform, With<Player>>,
    mut camera: Single<&mut Transform, (With<Camera2d>, Without<Player>)>,
) {
    camera.translation = player.translation.with_z(camera.translation.z);
}
