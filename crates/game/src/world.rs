//! World generation and instantiation: run the WFC generator at startup and
//! materialise the result as a rendered tilemap with colliders, shaded for
//! legibility (wall edges lit where they face floor, corridors darker than
//! open arenas, exit marked with a beacon).

use std::collections::HashSet;

use avian2d::prelude::{
    Collider, CollidingEntities, CollisionLayers, Gravity, LinearVelocity, LockedAxes, Position,
    RigidBody, Rotation, SleepingDisabled,
};
use bevy::asset::RenderAssetUsages;
use bevy::image::Image;
use bevy::prelude::*;
use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat};
use bevy_ecs_tilemap::prelude::*;
use rand::RngExt;
use rand::SeedableRng;
use rand::rngs::SmallRng;
use wfc::{
    FlowField, GeneratedMap, GeneratorConfig, Socket, TileClass, WeightedPrototype, generate,
    prototype_set,
};

use crate::corruption::{CorruptedTilemapMaterial, CorruptedZones};
use crate::neon_material::NeonTilemapMaterial;

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

/// The generated map data plus the prototype set its tile indices refer to,
/// as an ECS resource for systems that need the raw tile data (enemy
/// placement, combat sight, wayfinding).
#[derive(Resource)]
pub struct WorldMapRes {
    pub map: GeneratedMap,
    pub prototypes: Vec<WeightedPrototype>,
    /// Walkable fraction at generation time — the tint reference for floor
    /// tiles spawned later (blasted holes never know their original
    /// corridor depth, so they shade from the depth's stored integrity).
    pub integrity: f32,
}

/// World-space position the player respawns at.
#[derive(Resource, Debug, Clone, Copy)]
pub struct SpawnPoint(pub Vec2);

/// The BFS flow field toward the player's tile: enemies descend it when
/// they lack line of sight, routing around bends instead of hugging
/// corners. Rebuilt when the player changes tile or the map regenerates.
#[derive(Resource)]
pub struct PlayerFlow {
    pub field: FlowField,
    pub target: (u32, u32),
}

/// Current depth layer; increments each time the player reaches the exit.
#[derive(Resource, Debug, Clone, Copy, Default)]
pub struct Depth(pub u32);

/// Marker for the pulsing glow pillar marking the exit.
#[derive(Component)]
struct ExitBeacon;

/// Collision layer bits (avian `CollisionLayers::from_bits` memberships and
/// filters). Allegiance filtering keeps enemy shots off enemies and reflected
/// shots off the player.
pub const LAYER_WALL: u32 = 1 << 0;
pub const LAYER_PLAYER: u32 = 1 << 1;
pub const LAYER_ENEMY: u32 = 1 << 2;
pub const LAYER_ENEMY_SHOT: u32 = 1 << 3;
pub const LAYER_PLAYER_SHOT: u32 = 1 << 4;
pub const LAYER_GRENADE: u32 = 1 << 5;
pub const LAYER_CRACKED_WALL: u32 = 1 << 6;

/// Marker for the compound static wall body, so collision events can
/// distinguish wall bounces from gameplay hits.
#[derive(Component)]
pub struct WallBody;

/// Marker for the second compound static body covering just the cracked
/// (destructible) wall tiles — blasting one rebuilds only this small
/// compound, never the solid-wall one.
#[derive(Component)]
pub struct CrackedWallBody;

/// Atlas layout: 1 grid floor, 16 wall autotile masks, 1 terminal, 16
/// cracked wall autotile masks. Public so the shader preview example can
/// lay out every variant.
pub const FLOOR_ATLAS_INDEX: u32 = 0;
pub const WALL_ATLAS_BASE: u32 = 1;
pub const TERMINAL_ATLAS_INDEX: u32 = 17;
pub const CRACKED_WALL_ATLAS_BASE: u32 = 18;
pub const ATLAS_TILES: u32 = 34;

/// Share of the *eligible* wall tiles marked cracked — destructible by the
/// grenade blast, phaseable by the dash. Eligibility is thinness (see
/// [`CrackedWalls::build`]), so the real share of all walls runs lower.
const CRACKED_SHARE: f32 = 0.18;
/// Salt keeping the cracked pass uncorrelated with the generator's own use
/// of the seed.
const CRACKED_SALT: u64 = 0x0C7A_C4ED;

/// The cracked (destructible) wall tiles of the current depth: a
/// deterministic per-seed pass over the generated grid, game-side only —
/// the solver never knows which walls can come down.
#[derive(Resource, Debug, Default)]
pub struct CrackedWalls(pub HashSet<(u32, u32)>);

impl CrackedWalls {
    /// Mark the cracked walls of a generated map: about [`CRACKED_SHARE`]
    /// of the *eligible* wall tiles, rolled with a seeded RNG so the same
    /// seed and depth always crack the same walls. Eligibility is thinness
    /// — walkable on one full axis (N+S or E+W) — so every crack reads as
    /// a passable door along its thin side; a crack embedded in a thick
    /// wall could stop a phasing dash short and re-harden around the
    /// player.
    pub fn build(map: &WorldMapRes, seed: u64) -> Self {
        let grid = &map.map.grid;
        let (width, height) = (grid.width(), grid.height());
        let class_of = |x: u32, y: u32| {
            map.prototypes[grid.get(x, y).expect("fully collapsed") as usize]
                .prototype
                .class
        };
        let mut rng = SmallRng::seed_from_u64(seed ^ CRACKED_SALT);
        let mut tiles = HashSet::new();
        for y in 0..height {
            for x in 0..width {
                // The sealed border stays sealed — no pockets off the map.
                let interior = x > 0 && y > 0 && x + 1 < width && y + 1 < height;
                if !interior || class_of(x, y) != TileClass::Wall {
                    continue;
                }
                let thin_ns = class_of(x, y + 1).walkable() && class_of(x, y - 1).walkable();
                let thin_ew = class_of(x + 1, y).walkable() && class_of(x - 1, y).walkable();
                if (thin_ns || thin_ew) && rng.random::<f32>() < CRACKED_SHARE {
                    tiles.insert((x, y));
                }
            }
        }
        Self(tiles)
    }
}

pub struct WorldMapPlugin;

impl Plugin for WorldMapPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<MapConfig>()
            .init_resource::<Depth>()
            // The tilemap renders through the neon material; the custom
            // material pipeline ships with bevy_ecs_tilemap.
            .add_plugins(MaterialTilemapPlugin::<NeonTilemapMaterial>::default())
            // Top-down view: the physics default pulls everything down at
            // 9.81 units/s^2, so zero it out.
            .insert_resource(Gravity::ZERO)
            .add_systems(Startup, startup_world)
            .add_systems(
                Update,
                (
                    pulse_exit_beacon,
                    update_flow,
                    descend.run_if(in_state(crate::skills::GameState::Playing)),
                )
                    .run_if(in_state(crate::skills::GameState::Playing)),
            );
    }
}

/// Startup: build the first depth and spawn the player and its swarm.
pub(crate) fn startup_world(
    mut commands: Commands,
    mut images: ResMut<Assets<Image>>,
    mut map_materials: ResMut<Assets<NeonTilemapMaterial>>,
    mut corruption_materials: ResMut<Assets<CorruptedTilemapMaterial>>,
    config: Res<MapConfig>,
    bridge: Res<crate::bridge::BridgeTx>,
) {
    let (spawn, generated) = build_world(
        &mut commands,
        &mut images,
        &mut map_materials,
        &mut corruption_materials,
        &config,
        &bridge,
        0,
    );
    spawn_player(&mut commands, spawn, config.tile_size);
    crate::enemies::spawn_swarm(&mut commands, &generated, &config, 0);
}

/// Spawn the player body at `position`.
fn spawn_player(commands: &mut Commands, position: Vec2, tile_size: f32) {
    let player_size = tile_size * 0.6;
    commands.spawn((
        crate::Player,
        Sprite::from_color(crate::PLAYER_COLOUR, Vec2::splat(player_size)),
        Transform::from_xyz(position.x, position.y, 1.0),
        RigidBody::Dynamic,
        Collider::rectangle(player_size, player_size),
        LockedAxes::ROTATION_LOCKED,
        SleepingDisabled,
        CollidingEntities::default(),
        // Cracked walls block like walls — until the dash drops the layer
        // to phase through them (see combat::dash_trigger).
        player_layers(false),
    ));
}

/// The player's collision filter: cracked walls block — unless `phasing`,
/// which is the dash ghosting through them (restored the frame it ends).
pub(crate) fn player_layers(phasing: bool) -> CollisionLayers {
    let mut filter = LAYER_WALL | LAYER_ENEMY | LAYER_ENEMY_SHOT | LAYER_CRACKED_WALL;
    if phasing {
        filter &= !LAYER_CRACKED_WALL;
    }
    CollisionLayers::from_bits(LAYER_PLAYER, filter)
}

/// Swap the player's collision layers — `CollisionLayers` is immutable:
/// remove + insert (the same pattern the shield reflection uses).
pub(crate) fn swap_player_layers(commands: &mut Commands, player: Entity, phasing: bool) {
    if let Ok(mut entity_commands) = commands.get_entity(player) {
        entity_commands.remove::<CollisionLayers>();
        entity_commands.insert(player_layers(phasing));
    }
}

/// Reaching the exit descends to the next depth: the old world is cleared
/// and a fresh map generates from a depth-derived seed. Progression (HP,
/// XP, level, points, unlocks) carries forward; the combo and
/// invulnerability reset so the new depth never ambushes mid-blink.
#[allow(clippy::too_many_arguments, clippy::type_complexity)]
fn descend(
    mut commands: Commands,
    player: Single<(&mut Position, &mut LinearVelocity), With<crate::Player>>,
    map: Res<WorldMapRes>,
    config: Res<MapConfig>,
    mut depth: ResMut<Depth>,
    mut vitals: ResMut<crate::combat::PlayerVitals>,
    mut combo: ResMut<crate::combat::ComboState>,
    mut images: ResMut<Assets<Image>>,
    mut map_materials: ResMut<Assets<NeonTilemapMaterial>>,
    mut corruption_materials: ResMut<Assets<CorruptedTilemapMaterial>>,
    bridge: Res<crate::bridge::BridgeTx>,
    // One bundled param keeps the system within Bevy's 16-param limit.
    world_entities: (
        Query<Entity, With<TilePos>>,
        Query<Entity, With<TilemapSize>>,
        Query<Entity, With<WallBody>>,
        Query<Entity, With<CrackedWallBody>>,
        Query<Entity, With<ExitBeacon>>,
        Query<Entity, With<crate::enemies::Enemy>>,
        Query<Entity, With<crate::projectiles::Projectile>>,
        Query<Entity, With<crate::projectiles::Grenade>>,
        Query<Entity, With<crate::drops::Pickup>>,
    ),
) {
    let (tiles, tilemaps, walls, cracked_walls, beacons, enemies, projectiles, grenades, pickups) =
        world_entities;
    let (mut position, mut velocity) = player.into_inner();
    let (width, height) = (map.map.grid.width(), map.map.grid.height());
    let exit_pos = tile_world_pos((width, height), map.map.exit, config.tile_size);
    if position.0.distance(exit_pos) > TILE_SIZE / 2.0 {
        return;
    }

    depth.0 += 1;
    for entity in tiles
        .iter()
        .chain(tilemaps.iter())
        .chain(walls.iter())
        .chain(cracked_walls.iter())
        .chain(beacons.iter())
        .chain(enemies.iter())
        .chain(projectiles.iter())
        .chain(grenades.iter())
        .chain(pickups.iter())
    {
        if let Ok(mut entity_commands) = commands.get_entity(entity) {
            entity_commands.despawn();
        }
    }

    let (spawn, generated) = build_world(
        &mut commands,
        &mut images,
        &mut map_materials,
        &mut corruption_materials,
        &config,
        &bridge,
        depth.0,
    );
    crate::enemies::spawn_swarm(&mut commands, &generated, &config, depth.0);
    position.0 = spawn;
    velocity.0 = Vec2::ZERO;
    vitals.reset_invuln();
    *combo = crate::combat::ComboState::default();
    bridge.send(crate::bridge::GameAudioEvent::Descent);
    tracing::info!(depth = depth.0, "descended to the next depth");
}

/// Build a depth's world: run the WFC generator and materialise the map as
/// a rendered tilemap with colliders. Returns the new spawn point and the
/// generated map — the player body and swarm are the caller's concern
/// (spawned fresh on startup, repositioned/re-spawned on descent).
fn build_world(
    commands: &mut Commands,
    images: &mut Assets<Image>,
    map_materials: &mut Assets<NeonTilemapMaterial>,
    corruption_materials: &mut Assets<CorruptedTilemapMaterial>,
    config: &MapConfig,
    bridge: &crate::bridge::BridgeTx,
    depth: u32,
) -> (Vec2, GeneratedMap) {
    // Depth-derived seed: deterministic per (base seed, depth).
    let mut generator = config.generator.clone();
    generator.seed = generator.seed.wrapping_add(depth as u64);
    let generated = generate(&generator).expect("world generation failed");
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

    // The corrupted zones: clustered terminals seed glitching patches. The
    // zone set drives gameplay (drain, stealth); the mask image drives the
    // base shader's artefact pass; the overlay layer draws the corruption
    // itself.
    let world_res = WorldMapRes {
        map: generated.clone(),
        prototypes: prototype_set(
            config.generator.wall_weight,
            config.generator.terminal_weight,
        ),
        integrity,
    };
    // The cracked (destructible) walls: marked after generation, game-side
    // — the solver never knew. Drives the cracked tile rendering, the
    // second collider body and the grenade destruction pass.
    let cracked_tiles = CrackedWalls::build(&world_res, generator.seed);
    let zones = CorruptedZones::build(&world_res);
    let zone_mask: Handle<Image> = images.add(build_zone_mask(&zones, width, height));

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
                    TileClass::Wall => {
                        let mask = floor_facing_mask(
                            &generated.grid,
                            &prototypes,
                            (width, height),
                            (x, y),
                        );
                        if cracked_tiles.0.contains(&(x, y)) {
                            cracked_wall_atlas_index(mask)
                        } else {
                            wall_atlas_index(mask)
                        }
                    }
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
    // The tilemap renders through the neon material so the shader can
    // animate the baked edge strips — this depth's material carries this
    // depth's zone mask.
    commands
        .entity(tilemap_entity)
        .insert(MaterialTilemapBundle::<NeonTilemapMaterial> {
            grid_size: tile_size.into(),
            size: map_size_wide(width, height),
            storage: tile_storage,
            texture: TilemapTexture::Single(atlas),
            tile_size,
            anchor: TilemapAnchor::Center,
            material: MaterialTilemapHandle::from(
                map_materials.add(NeonTilemapMaterial::new(zone_mask)),
            ),
            ..Default::default()
        });

    // The corruption overlay: a second, sparse tilemap holding a tile on
    // every corrupted cell — its shader draws the static and data rain just
    // above the base tiles.
    let corruption_atlas: Handle<Image> =
        images.add(build_corruption_atlas(config.tile_size, generator.seed));
    let corruption_entity = commands.spawn_empty().id();
    let mut corruption_storage = TileStorage::empty(map_size_wide(width, height));
    for &(x, y) in &zones.0 {
        let pos = TilePos { x, y };
        let tile_entity = commands
            .spawn(TileBundle {
                position: pos,
                tilemap_id: TilemapId(corruption_entity),
                ..Default::default()
            })
            .id();
        corruption_storage.set(&pos, tile_entity);
    }
    commands
        .entity(corruption_entity)
        .insert(MaterialTilemapBundle::<CorruptedTilemapMaterial> {
            grid_size: tile_size.into(),
            size: map_size_wide(width, height),
            storage: corruption_storage,
            texture: TilemapTexture::Single(corruption_atlas),
            tile_size,
            anchor: TilemapAnchor::Center,
            transform: Transform::from_xyz(0.0, 0.0, 0.05),
            material: MaterialTilemapHandle::from(
                corruption_materials.add(CorruptedTilemapMaterial::default()),
            ),
            ..Default::default()
        });

    // One static compound body covering every solid wall tile: tiles have
    // no Transform of their own, so colliders live on a dedicated entity.
    // Cracked tiles are excluded — they get their own body below, so
    // destruction rebuilds only the small compound.
    let wall_rectangles: Vec<_> = (0..height)
        .flat_map(|y| (0..width).map(move |x| (x, y)))
        .filter(|&(x, y)| {
            class_of(generated.grid.get(x, y).expect("fully collapsed")) == TileClass::Wall
                && !cracked_tiles.0.contains(&(x, y))
        })
        .map(|(x, y)| {
            (
                Position::from(tile_world_pos((width, height), (x, y), config.tile_size)),
                Rotation::default(),
                Collider::rectangle(config.tile_size, config.tile_size),
            )
        })
        .collect();
    commands.spawn((
        WallBody,
        RigidBody::Static,
        Collider::compound(wall_rectangles),
        CollisionLayers::from_bits(LAYER_WALL, u32::MAX),
    ));
    // Parry rejects empty compounds, so the body only spawns when the
    // depth actually cracked some walls.
    if !cracked_tiles.0.is_empty() {
        commands.spawn((
            CrackedWallBody,
            RigidBody::Static,
            Collider::compound(cracked_compound_rectangles(
                &cracked_tiles,
                (width, height),
                config.tile_size,
            )),
            CollisionLayers::from_bits(LAYER_CRACKED_WALL, u32::MAX),
        ));
    }

    let spawn_pos = tile_world_pos((width, height), generated.spawn, config.tile_size);

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

    // Resources other systems build on: the raw map data (plus the prototype
    // set its tile indices refer to), the cracked walls, the corrupted
    // zones, the respawn point, and the first flow field — seeded toward
    // the spawn tile until the player moves.
    let initial_flow = FlowField::build(&generated.grid, &world_res.prototypes, generated.spawn);
    commands.insert_resource(world_res);
    commands.insert_resource(cracked_tiles);
    commands.insert_resource(zones);
    commands.insert_resource(SpawnPoint(spawn_pos));
    commands.insert_resource(PlayerFlow {
        field: initial_flow,
        target: generated.spawn,
    });
    bridge.send(crate::bridge::GameAudioEvent::WorldTelemetry { integrity });
    tracing::info!(integrity, depth, "world integrity published");
    (spawn_pos, generated)
}

/// Rebuild the flow field when the player changes tile or the map
/// regenerates on descent — a 48x48 BFS per change is trivial, and enemy
/// reads are O(1).
fn update_flow(
    player: Single<&Position, With<crate::Player>>,
    map: Res<WorldMapRes>,
    config: Res<MapConfig>,
    mut flow: ResMut<PlayerFlow>,
) {
    let (width, height) = (map.map.grid.width(), map.map.grid.height());
    let units = tile_units((width, height), config.tile_size, player.0);
    let tile = (
        (units.x.floor() as i32).clamp(0, width as i32 - 1) as u32,
        (units.y.floor() as i32).clamp(0, height as i32 - 1) as u32,
    );
    if map.is_changed() || tile != flow.target {
        flow.field = FlowField::build(&map.map.grid, &map.prototypes, tile);
        flow.target = tile;
    }
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

/// Atlas column for a cracked wall autotile mask.
fn cracked_wall_atlas_index(floor_facing_mask: u8) -> u32 {
    CRACKED_WALL_ATLAS_BASE + floor_facing_mask as u32
}

/// Bit set where a wall tile's face touches walkable space (N=1, E=2,
/// S=4, W=8 in grid space) — the autotile mask. Shared by generation
/// (baking the atlas variants) and the destruction pass (holes and their
/// neighbours re-derive from the mutated grid).
fn floor_facing_mask(
    grid: &wfc::Grid,
    prototypes: &[WeightedPrototype],
    size: (u32, u32),
    tile: (u32, u32),
) -> u8 {
    let (width, height) = size;
    let (x, y) = tile;
    let walkable = |nx: u32, ny: u32| {
        prototypes[grid.get(nx, ny).expect("fully collapsed") as usize]
            .prototype
            .class
            != TileClass::Wall
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

/// The corrupted-zone mask image: one pixel per cell, red channel high when
/// the cell sits in corruption — the neon shader's glitch pass reads it.
fn build_zone_mask(zones: &CorruptedZones, width: u32, height: u32) -> Image {
    let mut data = Vec::with_capacity((width * height * 4) as usize);
    for y in 0..height {
        for x in 0..width {
            let on = if zones.0.contains(&(x, y)) { 255u8 } else { 0 };
            data.extend_from_slice(&[on, 0, 0, 255]);
        }
    }
    Image::new(
        Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
        TextureDimension::D2,
        data,
        TextureFormat::Rgba8UnormSrgb,
        RenderAssetUsages::MAIN_WORLD | RenderAssetUsages::RENDER_WORLD,
    )
}

/// A tiny procedural noise atlas for the corruption overlay: green-tinted
/// random static the overlay shader animates. One tile, deterministic per
/// seed.
fn build_corruption_atlas(tile_size: f32, seed: u64) -> Image {
    let tile_px = tile_size as u32;
    let mut rng = SmallRng::seed_from_u64(seed);
    let mut data = Vec::with_capacity((tile_px * tile_px * 4) as usize);
    for _ in 0..tile_px * tile_px {
        let green = rng.random_range(60..200);
        data.extend_from_slice(&[
            rng.random_range(0..40),
            green,
            rng.random_range(40..120),
            255,
        ]);
    }
    Image::new(
        Extent3d {
            width: tile_px,
            height: tile_px,
            depth_or_array_layers: 1,
        },
        TextureDimension::D2,
        data,
        TextureFormat::Rgba8UnormSrgb,
        RenderAssetUsages::MAIN_WORLD | RenderAssetUsages::RENDER_WORLD,
    )
}

/// Continuous tile-space position of a world position — the exact inverse of
/// [`tile_world_pos`]. Tile (x, y) centres on (x + 0.5, y + 0.5).
pub fn tile_units(map_size: (u32, u32), tile_size: f32, world_pos: Vec2) -> Vec2 {
    Vec2::new(
        world_pos.x / tile_size + map_size.0 as f32 / 2.0,
        world_pos.y / tile_size + map_size.1 as f32 / 2.0,
    )
}

/// `TilemapSize` for a width/height pair.
fn map_size_wide(width: u32, height: u32) -> TilemapSize {
    TilemapSize {
        x: width,
        y: height,
    }
}

/// The cracked wall tiles inside a blast radius (euclidean from the
/// detonation point to the tile centre), in row-major order — the
/// shockwave does not route around walls.
fn cracked_tiles_in_radius(
    cracked: &CrackedWalls,
    map_size: (u32, u32),
    tile_size: f32,
    detonation: Vec2,
    blast_radius: f32,
) -> Vec<(u32, u32)> {
    let mut tiles: Vec<_> = cracked
        .0
        .iter()
        .copied()
        .filter(|&(x, y)| {
            tile_world_pos(map_size, (x, y), tile_size).distance(detonation) <= blast_radius
        })
        .collect();
    tiles.sort();
    tiles
}

/// The compound rectangles for the current cracked set: one per tile in
/// deterministic row-major order — a rebuild must never depend on the
/// hash set's iteration order.
fn cracked_compound_rectangles(
    cracked: &CrackedWalls,
    map_size: (u32, u32),
    tile_size: f32,
) -> Vec<(Position, Rotation, Collider)> {
    let mut tiles: Vec<_> = cracked.0.iter().copied().collect();
    tiles.sort();
    tiles
        .iter()
        .map(|&(x, y)| {
            (
                Position::from(tile_world_pos(map_size, (x, y), tile_size)),
                Rotation::default(),
                Collider::rectangle(tile_size, tile_size),
            )
        })
        .collect()
}

/// Grenade-blast destruction: every cracked wall within `blast_radius` of
/// `detonation` becomes open floor — the grid cell flips to a walkable tile
/// (the flow field and sight checks route through the hole), the tile
/// entity retextures to the standard floor, the surviving wall neighbours'
/// autotile masks recompute (they now face the hole), and the cracked
/// compound rebuilds without the destroyed rectangles. Returns the
/// destroyed tiles in row-major order. Destroying the last cracked wall
/// removes the body's collider entirely — parry rejects empty compounds.
#[allow(clippy::too_many_arguments)]
pub(crate) fn destroy_cracked_walls(
    commands: &mut Commands,
    map: &mut WorldMapRes,
    cracked: &mut CrackedWalls,
    storage: &mut TileStorage,
    body: Entity,
    collider: &mut Collider,
    detonation: Vec2,
    blast_radius: f32,
    tile_size: f32,
) -> Vec<(u32, u32)> {
    let (width, height) = (map.map.grid.width(), map.map.grid.height());
    let size = (width, height);
    let destroyed = cracked_tiles_in_radius(cracked, size, tile_size, detonation, blast_radius);
    if destroyed.is_empty() {
        return destroyed;
    }
    for &(x, y) in &destroyed {
        // Flip the cell to a walkable floor prototype; the exact variant
        // (how many faces it leaves on walkable space) settles in the
        // retexture pass once every hole in the blast exists.
        let provisional = floor_facing_mask(&map.map.grid, &map.prototypes, size, (x, y)) as u32;
        map.map.grid.set(x, y, provisional);
        cracked.0.remove(&(x, y));
        // The tile entity becomes the floor it now is; its tint lands in
        // the retexture pass too.
        let pos = TilePos { x, y };
        if let Some(entity) = storage.checked_get(&pos)
            && let Ok(mut entity_commands) = commands.get_entity(entity)
        {
            entity_commands.insert(TileTextureIndex(FLOOR_ATLAS_INDEX));
        }
    }
    retexture_after_destruction(commands, map, storage, &destroyed);
    if cracked.0.is_empty() {
        // The last cracked wall came down: strip the (now shapeless) body.
        if let Ok(mut entity_commands) = commands.get_entity(body) {
            entity_commands.remove::<Collider>();
        }
    } else {
        *collider = Collider::compound(cracked_compound_rectangles(cracked, size, tile_size));
    }
    destroyed
}

/// Recompute the tiles affected by a destruction: the holes themselves
/// (their floor variant depends on the fully mutated grid, and their tint
/// comes from the map's stored integrity) and every surviving wall
/// neighbour (its autotile mask now faces the hole).
fn retexture_after_destruction(
    commands: &mut Commands,
    map: &mut WorldMapRes,
    storage: &mut TileStorage,
    destroyed: &[(u32, u32)],
) {
    let (width, height) = (map.map.grid.width(), map.map.grid.height());
    let size = (width, height);
    // Affected cells: the holes plus their 8-way neighbours, deduped and
    // in deterministic order.
    let mut affected: Vec<(u32, u32)> = destroyed
        .iter()
        .flat_map(|&(x, y)| {
            let x = x as i32;
            let y = y as i32;
            (-1i32..=1).flat_map(move |dx| (-1i32..=1).map(move |dy| (x + dx, y + dy)))
        })
        .filter(|&(x, y)| x >= 0 && y >= 0 && x < width as i32 && y < height as i32)
        .map(|(x, y)| (x as u32, y as u32))
        .collect();
    affected.sort();
    affected.dedup();

    for (x, y) in affected {
        let pos = TilePos { x, y };
        let Some(entity) = storage.checked_get(&pos) else {
            continue;
        };
        let Ok(mut entity_commands) = commands.get_entity(entity) else {
            continue;
        };
        let class = map.prototypes[map.map.grid.get(x, y).expect("fully collapsed") as usize]
            .prototype
            .class;
        if class == TileClass::Wall {
            // A surviving wall (solid or cracked) now faces the hole: its
            // lit strips re-derive from the recomputed mask.
            let mask = floor_facing_mask(&map.map.grid, &map.prototypes, size, (x, y));
            entity_commands.insert(TileTextureIndex(wall_atlas_index(mask)));
        } else if destroyed.contains(&(x, y)) {
            // A hole: settle its floor variant on the final grid and tint
            // it like any other floor of this depth (the variant index is
            // the N/E/S/W mask; the tint counts its open edges).
            let open = floor_facing_mask(&map.map.grid, &map.prototypes, size, (x, y)) as u32;
            map.map.grid.set(x, y, open);
            entity_commands.insert(TileColor(destabilise(
                floor_tint(open.count_ones()),
                map.integrity,
            )));
        }
    }
}

/// RGBA of one atlas pixel: flat base colours with a 1-px grid on the floor
/// and lit strips on wall faces whose autotile mask bit is set; cracked
/// wall variants cut fracture lines through strips and body alike.
fn atlas_pixel(column: u32, px: u32, py: u32, tile_px: u32) -> [u8; 4] {
    const FLOOR: [u8; 4] = [26, 26, 46, 255];
    const GRID_LINE: [u8; 4] = [40, 40, 70, 255];
    const WALL: [u8; 4] = [58, 74, 94, 255];
    const WALL_EDGE: [u8; 4] = [120, 220, 255, 255];
    const TERMINAL: [u8; 4] = [57, 255, 20, 255];
    const CRACK: [u8; 4] = [24, 28, 38, 255];
    if column == FLOOR_ATLAS_INDEX {
        let border = px == 0 || py == 0 || px == tile_px - 1 || py == tile_px - 1;
        return if border { GRID_LINE } else { FLOOR };
    }
    if column == TERMINAL_ATLAS_INDEX {
        return TERMINAL;
    }
    // Wall autotile (solid or cracked): bright strip on faces whose mask
    // bit is set. N is the +y face, drawn as the first pixel rows of the
    // tile texture.
    let (mask, cracked) = if column >= CRACKED_WALL_ATLAS_BASE {
        ((column - CRACKED_WALL_ATLAS_BASE) as u8, true)
    } else {
        ((column - WALL_ATLAS_BASE) as u8, false)
    };
    let lit = (mask & 1 != 0 && py < 2)
        || (mask & 2 != 0 && px >= tile_px - 2)
        || (mask & 4 != 0 && py >= tile_px - 2)
        || (mask & 8 != 0 && px < 2);
    if cracked && crack_pixel(px, py, tile_px) {
        CRACK
    } else if lit {
        WALL_EDGE
    } else {
        WALL
    }
}

/// Whether an atlas pixel falls on a crack line: a diagonal fracture with
/// a branch forking toward the bottom-left, cutting through lit strips and
/// wall body alike. Normalised to the tile so the look is size-independent.
fn crack_pixel(px: u32, py: u32, tile_px: u32) -> bool {
    let x = px as f32 / tile_px as f32;
    let y = py as f32 / tile_px as f32;
    // Main fracture, top-left to bottom-right.
    let main = (y - (0.2 + 0.6 * x)).abs() < 0.07;
    // Branch: from the fracture's midpoint toward the bottom-left corner.
    let branch = x < 0.5 && (y - (0.5 + 0.6 * (0.5 - x))).abs() < 0.05;
    main || branch
}

/// Build the atlas image: one column per tile variant. Image data is
/// row-major across the FULL atlas width — row 0 of every column first, then
/// the next row — so the loops must walk rows, then x.
pub fn build_atlas(tile_size: f32) -> Image {
    let tile_px = tile_size as u32;
    let mut data = Vec::with_capacity((tile_px * ATLAS_TILES * tile_px * 4) as usize);
    for py in 0..tile_px {
        for x in 0..tile_px * ATLAS_TILES {
            let column = x / tile_px;
            let px = x % tile_px;
            data.extend_from_slice(&atlas_pixel(column, px, py, tile_px));
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

    /// A generated map (fixed seed) plus its config for the cracked-wall
    /// tests.
    fn test_map(seed: u64) -> (WorldMapRes, MapConfig) {
        let config = MapConfig {
            generator: GeneratorConfig {
                width: 24,
                height: 24,
                seed,
                ..Default::default()
            },
            tile_size: TILE_SIZE,
        };
        let generated = generate(&config.generator).expect("generation succeeds");
        let map = WorldMapRes {
            map: generated,
            prototypes: prototype_set(
                config.generator.wall_weight,
                config.generator.terminal_weight,
            ),
            integrity: 1.0,
        };
        (map, config)
    }

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
    fn tile_units_inverts_tile_world_pos() {
        let size = (48u32, 48u32);
        for tile in [(0u32, 0u32), (47, 0), (23, 24), (10, 33), (47, 47)] {
            let world = tile_world_pos(size, tile, TILE_SIZE);
            let back = tile_units(size, TILE_SIZE, world);
            assert!((back.x - (tile.0 as f32 + 0.5)).abs() < 1e-4);
            assert!((back.y - (tile.1 as f32 + 0.5)).abs() < 1e-4);
        }
        // The map centre sits between tiles 23 and 24.
        let centre = tile_units(size, TILE_SIZE, Vec2::ZERO);
        assert!((centre.x - 24.0).abs() < 1e-6);
        assert!((centre.y - 24.0).abs() < 1e-6);
    }

    #[test]
    fn wall_masks_map_to_consecutive_atlas_columns() {
        assert_eq!(wall_atlas_index(0), WALL_ATLAS_BASE);
        assert_eq!(wall_atlas_index(15), WALL_ATLAS_BASE + 15);
        assert_eq!(cracked_wall_atlas_index(0), CRACKED_WALL_ATLAS_BASE);
        assert_eq!(cracked_wall_atlas_index(15), CRACKED_WALL_ATLAS_BASE + 15);
        // Distinct masks land in distinct columns.
        for base in [wall_atlas_index, cracked_wall_atlas_index] {
            let columns: Vec<_> = (0..=15u8).map(base).collect();
            let mut sorted = columns.clone();
            sorted.sort();
            assert_eq!(columns, sorted);
        }
    }

    #[test]
    fn floor_facing_mask_bits_map_to_directions() {
        let prototypes = prototype_set(0.25, 0.02);
        let floor = prototypes
            .iter()
            .position(|p| p.prototype.class.walkable())
            .expect("the set has walkable tiles") as u32;
        let wall = prototypes
            .iter()
            .position(|p| !p.prototype.class.walkable())
            .expect("the set has a wall") as u32;
        let mut grid = wfc::Grid::new(3, 3);
        for y in 0..3 {
            for x in 0..3 {
                grid.set(x, y, floor);
            }
        }
        grid.set(1, 1, wall);
        // The wall faces floor on all four sides...
        assert_eq!(floor_facing_mask(&grid, &prototypes, (3, 3), (1, 1)), 15);
        // ...and walls out its east neighbour.
        grid.set(2, 1, wall);
        assert_eq!(floor_facing_mask(&grid, &prototypes, (3, 3), (1, 1)), 13);
    }

    #[test]
    fn cracked_marking_is_deterministic_thin_and_interior_only() {
        let (map, config) = test_map(11);
        let cracked = CrackedWalls::build(&map, config.generator.seed);
        let again = CrackedWalls::build(&map, config.generator.seed);
        assert_eq!(cracked.0, again.0, "same seed, same cracks");
        let (width, height) = (map.map.grid.width(), map.map.grid.height());
        let class_of = |t: (u32, u32)| {
            map.prototypes[map.map.grid.get(t.0, t.1).expect("fully collapsed") as usize]
                .prototype
                .class
        };
        // Eligibility is thinness: walkable on one full axis, interior.
        let eligible = |t: (u32, u32)| -> bool {
            let (x, y) = t;
            let interior = x > 0 && y > 0 && x + 1 < width && y + 1 < height;
            interior
                && class_of(t) == TileClass::Wall
                && ((class_of((x, y + 1)).walkable() && class_of((x, y - 1)).walkable())
                    || (class_of((x + 1, y)).walkable() && class_of((x - 1, y)).walkable()))
        };
        for &(x, y) in &cracked.0 {
            assert!(
                eligible((x, y)),
                "cracks only mark thin interior walls: {x},{y}"
            );
        }
        // The share of the eligible pool lands near the target —
        // deterministic per seed, so the band never drifts between runs.
        let pool = (0..height)
            .flat_map(|y| (0..width).map(move |x| (x, y)))
            .filter(|&t| eligible(t))
            .count();
        assert!(pool > 0, "the generated map has thin interior walls");
        assert!(!cracked.0.is_empty(), "some thin walls crack");
        let share = cracked.0.len() as f32 / pool as f32;
        assert!(
            share > 0.05 && share < 0.35,
            "cracked share {share} out of band"
        );
    }

    #[test]
    fn cracked_tiles_in_radius_selects_by_tile_centre_distance() {
        let cracked = CrackedWalls(HashSet::from([(2, 2), (2, 3), (6, 6)]));
        let size = (10u32, 10u32);
        let centre = tile_world_pos(size, (2, 2), TILE_SIZE);
        // A 2.5-tile blast reaches the seed tile and its neighbour —
        // euclidean, tile centres — but never the far one.
        let hit = cracked_tiles_in_radius(&cracked, size, TILE_SIZE, centre, TILE_SIZE * 2.5);
        assert_eq!(hit, vec![(2, 2), (2, 3)]);
        // A pinprick blast still takes the tile it sits on.
        let hit = cracked_tiles_in_radius(&cracked, size, TILE_SIZE, centre, 1.0);
        assert_eq!(hit, vec![(2, 2)]);
    }

    #[test]
    fn cracked_columns_draw_wall_look_with_fracture_lines() {
        let t = 32u32;
        // Plain body pixels match the solid wall...
        assert_eq!(
            atlas_pixel(cracked_wall_atlas_index(0), 4, 4, t),
            [58, 74, 94, 255]
        );
        // ...the crack line cuts dark through the body...
        assert_eq!(
            atlas_pixel(cracked_wall_atlas_index(0), 16, 16, t),
            [24, 28, 38, 255]
        );
        // ...and through lit strips on the cracked variants only — the
        // same pixel on a solid mask-2 wall stays lit.
        assert_eq!(
            atlas_pixel(wall_atlas_index(2), 31, 25, t),
            [120, 220, 255, 255]
        );
        assert_eq!(
            atlas_pixel(cracked_wall_atlas_index(2), 31, 25, t),
            [24, 28, 38, 255]
        );
    }

    #[test]
    fn destruction_opens_the_grid_retextures_and_rebuilds() {
        // 5x5 of floor with an interior cracked wall column at x=2, y=1..3;
        // the grenade takes out its middle tile.
        let prototypes = prototype_set(0.25, 0.02);
        let floor = prototypes
            .iter()
            .position(|p| p.prototype.class.walkable())
            .expect("the set has walkable tiles") as u32;
        let wall = prototypes
            .iter()
            .position(|p| !p.prototype.class.walkable())
            .expect("the set has a wall") as u32;
        let mut grid = wfc::Grid::new(5, 5);
        for y in 0..5 {
            for x in 0..5 {
                grid.set(x, y, floor);
            }
        }
        for y in 1..4 {
            grid.set(2, y, wall);
        }
        let size = (5u32, 5u32);
        let mut map = WorldMapRes {
            map: GeneratedMap {
                grid,
                spawn: (0, 2),
                exit: (4, 2),
            },
            prototypes: prototypes.clone(),
            integrity: 1.0,
        };
        // Around the column is a 6-step walk; through the hole, 2.
        let before = FlowField::build(&map.map.grid, &map.prototypes, (3, 2));
        assert_eq!(
            before.steps((1.5, 2.5)),
            Some(6),
            "the column blocks west-east"
        );

        let mut cracked = CrackedWalls(HashSet::from([(2, 2)]));
        let mut world = World::new();
        let tilemap_entity = world.spawn_empty().id();
        let mut storage = TileStorage::empty(map_size_wide(5, 5));
        for y in 0..5 {
            for x in 0..5 {
                let pos = TilePos { x, y };
                let texture = if (x, y) == (2, 2) {
                    cracked_wall_atlas_index(0)
                } else if map.map.grid.get(x, y) == Some(wall) {
                    wall_atlas_index(floor_facing_mask(
                        &map.map.grid,
                        &map.prototypes,
                        size,
                        (x, y),
                    ))
                } else {
                    FLOOR_ATLAS_INDEX
                };
                let entity = world
                    .spawn(TileBundle {
                        position: pos,
                        texture_index: TileTextureIndex(texture),
                        tilemap_id: TilemapId(tilemap_entity),
                        ..Default::default()
                    })
                    .id();
                storage.set(&pos, entity);
            }
        }
        let body = world
            .spawn(Collider::compound(cracked_compound_rectangles(
                &cracked, size, TILE_SIZE,
            )))
            .id();
        let centre = tile_world_pos(size, (2, 2), TILE_SIZE);
        // A local collider for the rebuild branch — this test destroys the
        // only cracked wall, so the empty path (collider removed via
        // commands) is the one that runs.
        let mut collider =
            Collider::compound(cracked_compound_rectangles(&cracked, size, TILE_SIZE));
        {
            let mut commands = world.commands();
            let destroyed = destroy_cracked_walls(
                &mut commands,
                &mut map,
                &mut cracked,
                &mut storage,
                body,
                &mut collider,
                centre,
                TILE_SIZE * 2.5,
                TILE_SIZE,
            );
            assert_eq!(destroyed, vec![(2, 2)]);
        }
        world.flush();
        // The last cracked wall came down: the shapeless body lost its
        // collider (parry rejects empty compounds).
        assert!(world.entity(body).get::<Collider>().is_none());

        // The grid cell flipped to a walkable floor variant — the N/E/S/W
        // mask with only east and west open (2 + 8).
        let index = map.map.grid.get(2, 2).expect("fully collapsed");
        assert!(map.prototypes[index as usize].prototype.class.walkable());
        assert_eq!(index, 10);
        // The cracked set and the rebuilt compound lost the rectangle.
        assert!(cracked.0.is_empty());
        assert!(cracked_compound_rectangles(&cracked, size, TILE_SIZE).is_empty());
        // The hole's tile retextured to the floor with the standard tint.
        let hole = storage
            .get(&TilePos { x: 2, y: 2 })
            .expect("the hole keeps its tile entity");
        assert_eq!(
            world
                .entity(hole)
                .get::<TileTextureIndex>()
                .expect("texture")
                .0,
            FLOOR_ATLAS_INDEX
        );
        assert_eq!(
            world.entity(hole).get::<TileColor>().expect("colour").0,
            destabilise(floor_tint(2), 1.0)
        );
        // The surviving wall south of the hole re-derived its mask: it now
        // faces the hole on its north face (15, up from 14).
        let neighbour = storage
            .get(&TilePos { x: 2, y: 1 })
            .expect("the wall keeps its tile entity");
        assert_eq!(
            world
                .entity(neighbour)
                .get::<TileTextureIndex>()
                .expect("texture")
                .0,
            wall_atlas_index(15)
        );
        // The flow field now routes through the hole.
        let flow = FlowField::build(&map.map.grid, &map.prototypes, (3, 2));
        assert_eq!(flow.steps((1.5, 2.5)), Some(2), "the hole routes east");
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
    fn atlas_data_is_row_major() {
        // Regression: the atlas was once written column-major (whole tile
        // blocks), scrambling every rendered tile into a grey smear.
        let tile_px = 8u32;
        let image = build_atlas(tile_px as f32);
        let data = image
            .data
            .as_deref()
            .expect("the atlas keeps its data on the main world");
        let width = (tile_px * ATLAS_TILES) as usize;
        for py in [0u32, 3, 7] {
            for x in 0..width as u32 {
                let expected = atlas_pixel(x / tile_px, x % tile_px, py, tile_px);
                let i = ((py as usize) * width + x as usize) * 4;
                assert_eq!(&data[i..i + 4], &expected[..]);
            }
        }
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
