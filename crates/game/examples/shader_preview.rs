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
use bevy::color::LinearRgba;
use bevy::math::primitives::Circle;
use bevy::mesh::Mesh2d;
use bevy::post_process::bloom::Bloom;
use bevy::prelude::*;
use bevy::sprite_render::{Material2dPlugin, MeshMaterial2d};
use bevy_ecs_tilemap::prelude::*;
use game::ASSETS_PATH;
use game::neon_material::NeonTilemapMaterial;
use game::player::move_direction;
use game::shield_fx::{DomeData, ShieldDomeMaterial};
use game::world::{ATLAS_TILES, TILE_SIZE, WALL_ATLAS_BASE, build_atlas};

/// Marker for the dome whose hit flash is cycled for inspection.
#[derive(Component)]
struct FlashingDome;

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
            Material2dPlugin::<ShieldDomeMaterial>::default(),
        ))
        .add_systems(Startup, setup)
        .add_systems(Update, (pan_camera, flash_dome))
        .run();
}

fn setup(
    mut commands: Commands,
    mut images: ResMut<Assets<Image>>,
    mut map_materials: ResMut<Assets<NeonTilemapMaterial>>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut dome_materials: ResMut<Assets<ShieldDomeMaterial>>,
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
    // Mark the right half of the strip corrupted so the glitch artefacts
    // have somewhere to live.
    let mut mask_data = Vec::with_capacity((ATLAS_TILES * 2 * 4) as usize);
    for _y in 0..2u32 {
        for x in 0..ATLAS_TILES {
            let on = if x >= ATLAS_TILES / 2 { 255u8 } else { 0 };
            mask_data.extend_from_slice(&[on, 0, 0, 255]);
        }
    }
    let zone_mask = images.add(Image::new(
        bevy::render::render_resource::Extent3d {
            width: ATLAS_TILES,
            height: 2,
            depth_or_array_layers: 1,
        },
        bevy::render::render_resource::TextureDimension::D2,
        mask_data,
        bevy::render::render_resource::TextureFormat::Rgba8UnormSrgb,
        bevy::asset::RenderAssetUsages::MAIN_WORLD | bevy::asset::RenderAssetUsages::RENDER_WORLD,
    ));
    let material =
        MaterialTilemapHandle::from(map_materials.add(NeonTilemapMaterial::new(zone_mask)));
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

    spawn_dome_row(&mut commands, &mut meshes, &mut dome_materials);

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

/// Lay out a row of shield domes below the tile strip: four plate
/// fractions (cyan), the barrier's violet tint, and one whose hit flash is
/// cycled for inspection.
fn spawn_dome_row(
    commands: &mut Commands,
    meshes: &mut Assets<Mesh>,
    dome_materials: &mut Assets<ShieldDomeMaterial>,
) {
    let radius = 16.0;
    let cyan = LinearRgba::new(0.13, 0.79, 1.0, 1.0);
    let violet = LinearRgba::new(0.45, 0.13, 1.0, 1.0);
    let dome = |fraction: f32, tint: LinearRgba| {
        ShieldDomeMaterial::new(DomeData {
            plate_fraction: fraction,
            hit_age: 999.0,
            tint_r: tint.red,
            tint_g: tint.green,
            tint_b: tint.blue,
            tint_a: tint.alpha,
            _pad0: 0.0,
            _pad1: 0.0,
        })
    };
    for (i, (fraction, tint)) in [
        (1.0, cyan),
        (0.75, cyan),
        (0.5, cyan),
        (0.25, cyan),
        (1.0, violet),
    ]
    .into_iter()
    .enumerate()
    {
        commands.spawn((
            Mesh2d(meshes.add(Circle::new(radius))),
            MeshMaterial2d(dome_materials.add(dome(fraction, tint))),
            Transform::from_xyz(-150.0 + 60.0 * i as f32, -72.0, 0.2),
        ));
    }
    // The flashing dome starts mid-flash; the update system cycles it.
    commands.spawn((
        FlashingDome,
        Mesh2d(meshes.add(Circle::new(radius))),
        MeshMaterial2d(dome_materials.add(dome(1.0, cyan))),
        Transform::from_xyz(150.0, -72.0, 0.2),
    ));
}

/// Cycle the flashing dome's hit age so the flash curve plays on loop.
fn flash_dome(
    time: Res<Time>,
    mut dome_materials: ResMut<Assets<ShieldDomeMaterial>>,
    domes: Query<&MeshMaterial2d<ShieldDomeMaterial>, With<FlashingDome>>,
) {
    for handle in &domes {
        if let Some(mut material) = dome_materials.get_mut(handle.id()) {
            material.set_hit_age((time.elapsed_secs() * 2.0) % 0.5);
        }
    }
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
