//! Shader preview: renders every atlas variant with the real neon tilemap
//! material and the game's bloom settings — the fast way to tune
//! `assets/shaders/neon_tilemap.wgsl` without booting the full game.
//! Edits to the shader hot-reload while this window is open.
//!
//! ```sh
//! cargo run -p game --example shader_preview
//! ```
//!
//! Top row: the full atlas catalogue (floor, wall autotiles 1–16, terminal).
//! Bottom row: fully-lit wall tiles butted together to eyeball seams.

use bevy::asset::AssetPlugin;
use bevy::post_process::bloom::Bloom;
use bevy::prelude::*;
use bevy_ecs_tilemap::prelude::*;
use game::ASSETS_PATH;
use game::neon_material::NeonTilemapMaterial;
use game::player::move_direction;
use game::world::{ATLAS_TILES, TILE_SIZE, WALL_ATLAS_BASE, build_atlas};

/// Camera zoom: 0.25 scale = 4× pixel size, matching the game's chunky look.
const PREVIEW_ZOOM: f32 = 0.25;

fn main() {
    App::new()
        .add_plugins((
            DefaultPlugins
                .set(AssetPlugin {
                    file_path: ASSETS_PATH.into(),
                    ..default()
                })
                .set(WindowPlugin {
                    primary_window: Some(Window {
                        title: "subit shader preview".into(),
                        ..default()
                    }),
                    ..default()
                })
                .set(ImagePlugin::default_nearest()),
            TilemapPlugin,
            MaterialTilemapPlugin::<NeonTilemapMaterial>::default(),
        ))
        .add_systems(Startup, setup)
        .add_systems(Update, pan_camera)
        .run();
}

fn setup(
    mut commands: Commands,
    mut images: ResMut<Assets<Image>>,
    mut materials: ResMut<Assets<NeonTilemapMaterial>>,
) {
    commands.spawn((
        Camera2d,
        Projection::Orthographic(OrthographicProjection {
            scale: PREVIEW_ZOOM,
            ..OrthographicProjection::default_2d()
        }),
        // Same neon pass as the game so the preview predicts the real look.
        Bloom {
            intensity: 0.15,
            ..Bloom::OLD_SCHOOL
        },
    ));

    let atlas = images.add(build_atlas(TILE_SIZE));
    let material = MaterialTilemapHandle::from(materials.add(NeonTilemapMaterial::default()));
    let map_size = TilemapSize {
        x: ATLAS_TILES,
        y: 2,
    };
    let tilemap_entity = commands.spawn_empty().id();
    let mut storage = TileStorage::empty(map_size);
    for x in 0..ATLAS_TILES {
        // Row 0: the full catalogue — column i rendered with texture i.
        spawn_tile(
            &mut commands,
            tilemap_entity,
            &mut storage,
            TilePos { x, y: 0 },
            x,
        );
        // Row 1: fully-lit walls (mask 15) butted together to eyeball seams.
        spawn_tile(
            &mut commands,
            tilemap_entity,
            &mut storage,
            TilePos { x, y: 1 },
            WALL_ATLAS_BASE + 15,
        );
    }

    commands
        .entity(tilemap_entity)
        .insert(MaterialTilemapBundle::<NeonTilemapMaterial> {
            grid_size: TilemapGridSize {
                x: TILE_SIZE,
                y: TILE_SIZE,
            },
            size: map_size,
            storage,
            texture: TilemapTexture::Single(atlas),
            tile_size: TilemapTileSize {
                x: TILE_SIZE,
                y: TILE_SIZE,
            },
            anchor: TilemapAnchor::Center,
            material,
            ..default()
        });
}

/// Spawn one tile at `position` with atlas column `texture_index`, owned by
/// the shared tilemap entity.
fn spawn_tile(
    commands: &mut Commands,
    tilemap_entity: Entity,
    storage: &mut TileStorage,
    position: TilePos,
    texture_index: u32,
) {
    let entity = commands
        .spawn(TileBundle {
            position,
            texture_index: TileTextureIndex(texture_index),
            tilemap_id: TilemapId(tilemap_entity),
            ..default()
        })
        .id();
    storage.set(&position, entity);
}

/// WASD pans the camera (zoomed in, the strip runs off-screen).
fn pan_camera(
    input: Res<ButtonInput<KeyCode>>,
    time: Res<Time>,
    mut camera: Single<&mut Transform, With<Camera2d>>,
) {
    let dir = move_direction(&input);
    camera.translation += (dir * 400.0 * time.delta_secs()).extend(0.0);
}
