//! subit game library: boots the generated world, moves the player with
//! physics-driven movement, fights the enemy swarm, and publishes game
//! events to the Ensemble hub.
//!
//! Organised as a library (with the binary a thin wrapper) so examples such
//! as the shader preview — and future integration tests — can reuse the
//! real modules instead of duplicating them.

pub mod bridge;
pub mod combat;
pub mod corruption;
pub mod drops;
pub mod enemies;
pub mod hud;
pub mod neon_material;
pub mod player;
pub mod progression;
pub mod projectiles;
pub mod shield_fx;
pub mod skills;
pub mod world;

use bevy::asset::AssetPlugin;
use bevy::prelude::*;
use skills::GameState;

/// Absolute assets directory, resolved at compile time — Bevy roots asset
/// sources at the executable's directory, which for target/ binaries is
/// nowhere near the workspace assets without this override.
pub const ASSETS_PATH: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets");

// The crate-root surface other modules (and examples) reach for via
// `crate::` paths.
pub use player::{PLAYER_COLOUR, Player, move_direction};

/// Assemble and run the game client.
pub fn run() {
    App::new()
        .add_plugins((
            DefaultPlugins
                .set(AssetPlugin {
                    file_path: ASSETS_PATH.into(),
                    ..default()
                })
                .set(ImagePlugin::default_nearest()),
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
            shield_fx::ShieldFxPlugin,
            corruption::CorruptionPlugin,
        ))
        .add_systems(Startup, player::setup)
        .add_systems(Update, player::camera_follow)
        // The player sim pauses while the skills menu is open.
        .add_systems(
            Update,
            player::player_movement.run_if(in_state(GameState::Playing)),
        )
        .run();
}
