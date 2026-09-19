//! World generation and instantiation: run the WFC generator at startup and
//! materialise the result as a rendered tilemap with colliders, shaded for
//! legibility (wall edges lit where they face floor, corridors darker than
//! open arenas, exit marked with a beacon).

use avian2d::prelude::{
    Collider, CollidingEntities, Gravity, LockedAxes, Position, RigidBody, Rotation,
    SleepingDisabled,
};
use bevy::asset::RenderAssetUsages;
use bevy::image::Image;
use bevy::prelude::*;
use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat};
use bevy_ecs_tilemap::prelude::*;
use wfc::{GeneratedMap, GeneratorConfig, Socket, TileClass, generate, prototype_set};

/// Size of a single tile in world units (pixels).
pub const TILE_SIZE: f32 = 32.0;

/// Player movement speed in world units per second.
pub const PLAYER_SPEED: f32 = 240.0;

/// Exit-tile tint: magenta, distinct from the neon-green terminals.
const EXIT_COLOUR: Color = Color::srgb(1.0, 0.3, 1.0);

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

/// The generated map data (grid, spawn, exit) as an ECS resource, for
/// systems that need the raw tile data (enemy placement, wayfinding).
#[derive(Resource)]
pub struct WorldMapRes(pub GeneratedMap);

/// World-space position the player respawns at.
#[derive(Resource, Debug, Clone, Copy)]
pub struct SpawnPoint(pub Vec2);

/// Marker for the pulsing glow pillar marking the exit.
#[derive(Component)]
struct ExitBeacon;

/// Atlas layout: 1 grid floor, 16 wall autotile masks, 1 terminal.
const FLOOR_ATLAS_INDEX: u32 = 0;
const WALL_ATLAS_BASE: u32 = 1;
const TERMINAL_ATLAS_INDEX: u32 = 17;
const ATLAS_TILES: u32 = 18;

pub struct WorldMapPlugin;

impl Plugin for WorldMapPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<MapConfig>()
            // Top-down view: the physics default pulls everything down at
            // 9.81 units/s^2, so zero it out.
            .insert_resource(Gravity::ZERO)
            .add_systems(Startup, generate_world)
            .add_systems(Update, pulse_exit_beacon);
    }
}

/// Run the WFC generator and materialise the map as a rendered tilemap with
/// colliders and a player.
pub(crate) fn generate_world(
    mut commands: Commands,
    mut images: ResMut<Assets<Image>>,
    config: Res<MapConfig>,
    bridge: Res<crate::bridge::BridgeTx>,
) {
    let generated = generate(&config.generator).expect("world generation failed");
    let (width, height) = (generated.grid.width(), generated.grid.height());

    // Class lookup built from the same generator config the map came from.
    let prototypes = prototype_set(
        config.generator.wall_weight,
        config.generator.terminal_weight,
    );
    let class_of = |tile_index: u32| prototypes[tile_index as usize].prototype.class;
    let open_edges_of = |tile_index: u32| -> u32 {
        prototypes[tile_index as usize]
            .prototype
            .sockets
            .iter()
            .filter(|s| **s == Socket::Path)
            .count() as u32
    };
    // Bit set where a wall tile's face touches walkable space (N=1, E=2,
    // S=4, W=8 in grid space).
    let floor_facing_mask = |x: u32, y: u32| -> u8 {
        let walkable = |nx: u32, ny: u32| {
            class_of(generated.grid.get(nx, ny).expect("fully collapsed")) != TileClass::Wall
        };
        let mut mask = 0u8;
        if y + 1 < height && walkable(x, y + 1) {
            mask |= 1;
        }
        if x + 1 < width && walkable(x + 1, y) {
            mask |= 2;
        }
        if y > 0 && walkable(x, y - 1) {
            mask |= 4;
        }
        if x > 0 && walkable(x - 1, y) {
            mask |= 8;
        }
        mask
    };

    // Walkable fraction of the grid — the world-integrity value that pairs
    // with the audio's harmonic mode shift.
    let walkable_count = (0..height)
        .flat_map(|y| (0..width).map(move |x| (x, y)))
        .filter(|&(x, y)| {
            class_of(generated.grid.get(x, y).expect("fully collapsed")) != TileClass::Wall
        })
        .count();
    let integrity = walkable_count as f32 / (width * height) as f32;

    // Procedural atlas: one autotiling wall set, grid floor and terminal.
    // Built in code so the prototype needs no art assets.
    let atlas: Handle<Image> = images.add(build_atlas(config.tile_size));

    let tilemap_entity = commands.spawn_empty().id();
    let mut tile_storage = TileStorage::empty(map_size_wide(width, height));

    for y in 0..height {
        for x in 0..width {
            let tile_index = generated.grid.get(x, y).expect("fully collapsed");
            let class = class_of(tile_index);
            let tile_pos = TilePos { x, y };
            let mut tile = TileBundle {
                position: tile_pos,
                texture_index: TileTextureIndex(match class {
                    TileClass::Floor => FLOOR_ATLAS_INDEX,
                    TileClass::Wall => wall_atlas_index(floor_facing_mask(x, y)),
                    TileClass::Terminal => TERMINAL_ATLAS_INDEX,
                }),
                tilemap_id: TilemapId(tilemap_entity),
                color: match class {
                    // Corridors read darker than open arenas; the whole floor
                    // drifts cooler as integrity drops.
                    TileClass::Floor => TileColor(destabilise(
                        floor_tint(open_edges_of(tile_index)),
                        integrity,
                    )),
                    _ => TileColor::default(),
                },
                ..Default::default()
            };
            if (x, y) == generated.exit {
                // Magenta so the goal never reads as a terminal.
                tile.color = TileColor(EXIT_COLOUR);
            }
            let tile_entity = commands.spawn(tile).id();
            tile_storage.set(&tile_pos, tile_entity);
        }
    }

    let tile_size = TilemapTileSize {
        x: config.tile_size,
        y: config.tile_size,
    };
    commands.entity(tilemap_entity).insert(TilemapBundle {
        grid_size: tile_size.into(),
        size: map_size_wide(width, height),
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
    // CollidingEntities is populated by avian and read by the contact-damage
    // system; it must be added manually.
    let spawn_pos = tile_world_pos((width, height), generated.spawn, config.tile_size);
    let player_size = config.tile_size * 0.6;
    commands.spawn((
        crate::Player,
        Sprite::from_color(crate::PLAYER_COLOUR, Vec2::splat(player_size)),
        Transform::from_xyz(spawn_pos.x, spawn_pos.y, 1.0),
        RigidBody::Dynamic,
        Collider::rectangle(player_size, player_size),
        LockedAxes::ROTATION_LOCKED,
        SleepingDisabled,
        CollidingEntities::default(),
    ));

    // Wayfinding: a translucent glow pillar over the exit, visible from
    // across the map.
    let exit_pos = tile_world_pos((width, height), generated.exit, config.tile_size);
    commands.spawn((
        ExitBeacon,
        Sprite::from_color(
            Color::srgba(1.0, 0.3, 1.0, 0.3),
            Vec2::new(config.tile_size * 0.5, config.tile_size * 4.0),
        ),
        Transform::from_xyz(exit_pos.x, exit_pos.y + config.tile_size * 1.5, 1.5),
    ));

    // Resources other systems build on: the raw map data and respawn point.
    commands.insert_resource(WorldMapRes(generated.clone()));
    commands.insert_resource(SpawnPoint(spawn_pos));
    bridge.send(crate::bridge::GameAudioEvent::WorldTelemetry { integrity });
    tracing::info!(integrity, "world integrity published");
}

/// Gentle sine pulse on the exit beacon's alpha.
fn pulse_exit_beacon(time: Res<Time>, mut beacon: Single<&mut Sprite, With<ExitBeacon>>) {
    let alpha = 0.2 + 0.15 * (time.elapsed_secs() * 3.0).sin();
    beacon.color.set_alpha(alpha);
}

/// World-space centre of a tile for an anchor-centred map whose tilemap
/// entity sits at the origin.
pub fn tile_world_pos(map_size: (u32, u32), tile: (u32, u32), tile_size: f32) -> Vec2 {
    Vec2::new(
        (tile.0 as f32 - map_size.0 as f32 / 2.0 + 0.5) * tile_size,
        (tile.1 as f32 - map_size.1 as f32 / 2.0 + 0.5) * tile_size,
    )
}

/// Atlas column for a wall autotile mask.
fn wall_atlas_index(floor_facing_mask: u8) -> u32 {
    WALL_ATLAS_BASE + floor_facing_mask as u32
}

/// Floor shading: corridors (few open edges) read darker, arenas lighter.
fn floor_tint(open_edges: u32) -> Color {
    let shade = 0.75 + 0.25 * (open_edges.min(4) as f32 / 4.0);
    Color::srgb(shade, shade, shade)
}

/// Shift a floor tint toward a cool destabilised hue as integrity drops
/// (at most halfway, so the shift stays subtle).
fn destabilise(base: Color, integrity: f32) -> Color {
    let t = (1.0 - integrity.clamp(0.0, 1.0)) * 0.5;
    let c = base.to_srgba();
    Color::srgba(
        c.red * (1.0 - t) + 0.75 * t,
        c.green * (1.0 - t) + 0.85 * t,
        c.blue * (1.0 - t) + 1.0 * t,
        c.alpha,
    )
}

/// `TilemapSize` for a width/height pair.
fn map_size_wide(width: u32, height: u32) -> TilemapSize {
    TilemapSize {
        x: width,
        y: height,
    }
}

/// RGBA of one atlas pixel: flat base colours with a 1-px grid on the floor
/// and lit strips on wall faces whose autotile mask bit is set.
fn atlas_pixel(column: u32, px: u32, py: u32, tile_px: u32) -> [u8; 4] {
    const FLOOR: [u8; 4] = [26, 26, 46, 255];
    const GRID_LINE: [u8; 4] = [40, 40, 70, 255];
    const WALL: [u8; 4] = [58, 74, 94, 255];
    const WALL_EDGE: [u8; 4] = [120, 220, 255, 255];
    const TERMINAL: [u8; 4] = [57, 255, 20, 255];
    if column == FLOOR_ATLAS_INDEX {
        let border = px == 0 || py == 0 || px == tile_px - 1 || py == tile_px - 1;
        return if border { GRID_LINE } else { FLOOR };
    }
    if column == TERMINAL_ATLAS_INDEX {
        return TERMINAL;
    }
    // Wall autotile: bright strip on faces whose mask bit is set. N is the
    // +y face, drawn as the first pixel rows of the tile texture.
    let mask = (column - WALL_ATLAS_BASE) as u8;
    let lit = (mask & 1 != 0 && py < 2)
        || (mask & 2 != 0 && px >= tile_px - 2)
        || (mask & 4 != 0 && py >= tile_px - 2)
        || (mask & 8 != 0 && px < 2);
    if lit { WALL_EDGE } else { WALL }
}

/// Build the atlas image: one column per tile variant.
fn build_atlas(tile_size: f32) -> Image {
    let tile_px = tile_size as u32;
    let mut data = Vec::with_capacity((tile_px * ATLAS_TILES * tile_px * 4) as usize);
    for column in 0..ATLAS_TILES {
        for py in 0..tile_px {
            for px in 0..tile_px {
                data.extend_from_slice(&atlas_pixel(column, px, py, tile_px));
            }
        }
    }
    Image::new(
        Extent3d {
            width: tile_px * ATLAS_TILES,
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
    fn wall_masks_map_to_consecutive_atlas_columns() {
        assert_eq!(wall_atlas_index(0), WALL_ATLAS_BASE);
        assert_eq!(wall_atlas_index(15), WALL_ATLAS_BASE + 15);
        // Distinct masks land in distinct columns.
        let columns: Vec<_> = (0..=15u8).map(wall_atlas_index).collect();
        let mut sorted = columns.clone();
        sorted.sort();
        assert_eq!(columns, sorted);
    }

    #[test]
    fn floor_tint_brightens_with_open_edges() {
        let corridor = floor_tint(0).to_srgba();
        let mid = floor_tint(2).to_srgba();
        let arena = floor_tint(4).to_srgba();
        assert!(arena.red > mid.red && mid.red > corridor.red);
        assert!((corridor.red - 0.75).abs() < 1e-6);
        assert!((arena.red - 1.0).abs() < 1e-6);
        // Overshoot clamps.
        assert!((floor_tint(9).to_srgba().red - 1.0).abs() < 1e-6);
    }

    #[test]
    fn destabilise_leaves_full_integrity_alone_and_shifts_low() {
        let base = floor_tint(4);
        assert_eq!(destabilise(base, 1.0), base);
        let shifted = destabilise(base, 0.0).to_srgba();
        let base = base.to_srgba();
        // Red falls — and falls harder than blue, which is already at its
        // ceiling on the brightest floor — a relatively cooler shift.
        assert!(shifted.red < base.red);
        assert!((base.red - shifted.red) > (base.blue - shifted.blue));
        assert!(shifted.red >= 0.75 * 0.5);
    }

    #[test]
    fn atlas_pixel_draws_grid_edges_and_lit_wall_faces() {
        let t = 8u32; // small tile for test maths
        // Floor: border is the grid line, centre is floor.
        assert_eq!(atlas_pixel(FLOOR_ATLAS_INDEX, 0, 3, t), [40, 40, 70, 255]);
        assert_eq!(atlas_pixel(FLOOR_ATLAS_INDEX, 3, 3, t), [26, 26, 46, 255]);
        // Wall with no floor-facing faces never lights up.
        assert_eq!(atlas_pixel(wall_atlas_index(0), 0, 0, t), [58, 74, 94, 255]);
        // Mask 1 (N face) lights the top rows only.
        assert_eq!(
            atlas_pixel(wall_atlas_index(1), 4, 0, t),
            [120, 220, 255, 255]
        );
        assert_eq!(
            atlas_pixel(wall_atlas_index(1), 4, t - 3, t),
            [58, 74, 94, 255]
        );
        // Mask 15 lights every edge pixel.
        for (px, py) in [
            (0u32, 0u32),
            (t - 1, 0),
            (0, t - 1),
            (t - 1, t - 1),
            (4, 0),
            (4, t - 1),
        ] {
            assert_eq!(
                atlas_pixel(wall_atlas_index(15), px, py, t),
                [120, 220, 255, 255]
            );
        }
        // Terminal stays neon green.
        assert_eq!(
            atlas_pixel(TERMINAL_ATLAS_INDEX, 3, 3, t),
            [57, 255, 20, 255]
        );
    }
}
