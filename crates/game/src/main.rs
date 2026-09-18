//! subit game client: a minimal Bevy app with a movable placeholder player
//! and the Ensemble bridge that publishes game events to the hub.

mod bridge;

use bevy::prelude::*;
use bevy_ecs_tilemap::prelude::TilemapPlugin;

fn main() {
    App::new()
        .add_plugins((
            DefaultPlugins,
            TilemapPlugin,
            avian2d::PhysicsPlugins::default(),
            bridge::EnsembleBridgePlugin,
        ))
        .add_systems(Startup, setup)
        .add_systems(Update, (player_movement, bridge::send_test_pulse))
        .run();
}

#[derive(Component)]
struct Player;

fn setup(mut commands: Commands) {
    commands.spawn(Camera2d);
    // Placeholder player: a plain white square until proper art lands.
    commands.spawn((
        Sprite::default(),
        Transform::from_scale(Vec3::splat(50.0)),
        Player,
    ));
}

fn player_movement(
    time: Res<Time>,
    input: Res<ButtonInput<KeyCode>>,
    mut player: Single<&mut Transform, With<Player>>,
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
        player.translation += direction.normalize().extend(0.0) * 300.0 * time.delta_secs();
    }
}
