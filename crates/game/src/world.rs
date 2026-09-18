//! World generation and instantiation: run the WFC generator at startup and
//! materialise the result as a rendered tilemap with colliders.

use avian2d::prelude::{
    Collider, Gravity, LockedAxes, Position, RigidBody, Rotation, SleepingDisabled,
};
use bevy::asset::RenderAssetUsages;
use bevy::image::Image;
use bevy::prelude::*;
use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat};
use bevy_ecs_tilemap::prelude::*;
use wfc::{GeneratorConfig, TileClass, generate, prototype_set};

/// Size of a single tile in world units (pixels).
pub const TILE_SIZE: f32 = 32.0;

/// Player movement speed in world units per second.
pub const PLAYER_SPEED: f32 = 240.0;

/// World generation parameters.
#[derive(Resource, Debug, Clone)]
pub struct MapConfig {
    pub generator: GeneratorConfig,
    pub tile_size: f32,
}

impl Default for MapConfig {
    fn default() -> Self {
        Self {
            generator: GeneratorConfig::default(),
            tile_size: TILE_SIZE,
        }
    }
}

pub struct WorldMapPlugin;

impl Plugin for WorldMapPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<MapConfig>()
            // Top-down view: the physics default pulls everything down at
            // 9.81 units/s^2, so zero it out.
            .insert_resource(Gravity::ZERO)
            .add_systems(Startup, generate_world);
    }
}

/// Run the WFC generator and materialise the map as a rendered tilemap with
/// colliders and a player.
fn generate_world(
    mut commands: Commands,
    mut images: ResMut<Assets<Image>>,
    config: Res<MapConfig>,
) {
    let generated = generate(&config.generator).expect("world generation failed");
    let (width, height) = (generated.grid.width(), generated.grid.height());
    let map_size = TilemapSize {
        x: width,
        y: height,
    };
    let tile_size = TilemapTileSize {
        x: config.tile_size,
        y: config.tile_size,
    };

    // Procedural atlas: one flat-colour tile per class. Built in code so the
    // prototype needs no art assets.
    let atlas: Handle<Image> = images.add(build_atlas(config.tile_size));

    // Class lookup built from the same generator config the map came from.
    let prototypes = prototype_set(
        config.generator.wall_weight,
        config.generator.terminal_weight,
    );
    let class_of = |tile_index: u32| prototypes[tile_index as usize].prototype.class;

    let tilemap_entity = commands.spawn_empty().id();
    let mut tile_storage = TileStorage::empty(map_size);

    for y in 0..height {
        for x in 0..width {
            let tile_index = generated.grid.get(x, y).expect("fully collapsed");
            let tile_pos = TilePos { x, y };
            let mut tile = TileBundle {
                position: tile_pos,
                texture_index: TileTextureIndex(atlas_index(class_of(tile_index))),
                tilemap_id: TilemapId(tilemap_entity),
                ..Default::default()
            };
            if (x, y) == generated.exit {
                // Bright tint so the goal stands out on the map.
                tile.color = TileColor(Color::srgb(0.3, 1.0, 0.3));
            }
            let tile_entity = commands.spawn(tile).id();
            tile_storage.set(&tile_pos, tile_entity);
        }
    }

    commands.entity(tilemap_entity).insert(TilemapBundle {
        grid_size: tile_size.into(),
        size: map_size,
        storage: tile_storage,
        texture: TilemapTexture::Single(atlas),
        tile_size,
        anchor: TilemapAnchor::Center,
        ..Default::default()
    });

    // One static compound body covering every wall tile: tiles have no
    // Transform of their own, so colliders live on a dedicated entity.
    let wall_rectangles: Vec<_> = (0..height)
        .flat_map(|y| (0..width).map(move |x| (x, y)))
        .filter(|&(x, y)| {
            class_of(generated.grid.get(x, y).expect("fully collapsed")) == TileClass::Wall
        })
        .map(|(x, y)| {
            (
                Position::from(tile_world_pos((width, height), (x, y), config.tile_size)),
                Rotation::default(),
                Collider::rectangle(config.tile_size, config.tile_size),
            )
        })
        .collect();
    commands.spawn((RigidBody::Static, Collider::compound(wall_rectangles)));

    // Player at the generated spawn point, above the tiles.
    let spawn_pos = tile_world_pos((width, height), generated.spawn, config.tile_size);
    let player_size = config.tile_size * 0.6;
    commands.spawn((
        crate::Player,
        Sprite::from_color(Color::srgb(0.9, 0.9, 0.95), Vec2::splat(player_size)),
        Transform::from_xyz(spawn_pos.x, spawn_pos.y, 1.0),
        RigidBody::Dynamic,
        Collider::rectangle(player_size, player_size),
        LockedAxes::ROTATION_LOCKED,
        SleepingDisabled,
    ));
}

/// World-space centre of a tile for an anchor-centred map whose tilemap
/// entity sits at the origin.
pub fn tile_world_pos(map_size: (u32, u32), tile: (u32, u32), tile_size: f32) -> Vec2 {
    Vec2::new(
        (tile.0 as f32 - map_size.0 as f32 / 2.0 + 0.5) * tile_size,
        (tile.1 as f32 - map_size.1 as f32 / 2.0 + 0.5) * tile_size,
    )
}

/// Atlas column for a tile class.
fn atlas_index(class: TileClass) -> u32 {
    match class {
        TileClass::Floor => 0,
        TileClass::Wall => 1,
        TileClass::Terminal => 2,
    }
}

/// Build a 3x1 atlas image with one flat colour per tile class.
fn build_atlas(tile_size: f32) -> Image {
    const CLASS_COLOURS: [[u8; 4]; 3] = [
        [26, 26, 46, 255],  // floor: near-black navy
        [58, 74, 94, 255],  // wall: grey-cyan
        [57, 255, 20, 255], // terminal: neon green
    ];
    let tile_px = tile_size as u32;
    let mut data = Vec::with_capacity((tile_px * 3 * tile_px * 4) as usize);
    for colour in &CLASS_COLOURS {
        for _ in 0..tile_px * tile_px {
            data.extend_from_slice(colour);
        }
    }
    Image::new(
        Extent3d {
            width: tile_px * 3,
            height: tile_px,
            depth_or_array_layers: 1,
        },
        TextureDimension::D2,
        data,
        TextureFormat::Rgba8UnormSrgb,
        RenderAssetUsages::MAIN_WORLD | RenderAssetUsages::RENDER_WORLD,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tile_world_pos_centres_the_grid_on_the_origin() {
        // 2x2 map of 10px tiles: tile centres at ±5.
        assert_eq!(tile_world_pos((2, 2), (0, 0), 10.0), Vec2::new(-5.0, -5.0));
        assert_eq!(tile_world_pos((2, 2), (1, 1), 10.0), Vec2::new(5.0, 5.0));
        // 3x3: the centre tile sits on the origin, corner tile centres at
        // ±10 (one tile width from the centre tile).
        assert_eq!(tile_world_pos((3, 3), (1, 1), 10.0), Vec2::ZERO);
        assert_eq!(tile_world_pos((3, 3), (0, 2), 10.0), Vec2::new(-10.0, 10.0));
    }

    #[test]
    fn atlas_indices_follow_the_class_order() {
        assert_eq!(atlas_index(TileClass::Floor), 0);
        assert_eq!(atlas_index(TileClass::Wall), 1);
        assert_eq!(atlas_index(TileClass::Terminal), 2);
    }
}
