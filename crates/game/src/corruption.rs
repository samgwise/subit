//! Corrupted data zones: terminal clusters seed glitching patches that bleed
//! shields (everyone's — the lure tactic) and hide the player (−2 notice
//! steps, stacking with the cloak). Pure geometry lives in pure helpers;
//! the systems tick drains and publish the sonic edge triggers.

use std::collections::HashSet;

use avian2d::prelude::Position;
use bevy::asset::Asset;
use bevy::prelude::*;
use bevy::reflect::TypePath;
use bevy::render::render_resource::AsBindGroup;
use bevy::shader::ShaderRef;
use bevy_ecs_tilemap::prelude::{MaterialTilemap, MaterialTilemapPlugin};

use crate::bridge::{BridgeTx, GameAudioEvent};
use crate::combat::Barrier;
use crate::enemies::Shield;
use crate::world::{MapConfig, WorldMapRes, tile_units};

/// A corrupted patch covers the seeding cluster plus every tile within this
/// Chebyshev distance of it.
const ZONE_DILATION: i32 = 1;

/// Seconds between shield-plate drains for entities standing in corruption.
const DRAIN_INTERVAL_SECS: f32 = 1.0;

/// How many notice steps the player sheds inside a corrupted zone.
pub const ZONE_STEALTH_STEPS: u32 = 2;

/// The corrupted tiles of the current map: the tile set behind both the
/// gameplay (drain, stealth) and the render mask.
#[derive(Resource, Debug, Default)]
pub struct CorruptedZones(pub HashSet<(u32, u32)>);

impl CorruptedZones {
    /// Build the zones for a map. Every terminal tile with at least one
    /// terminal neighbour (8-way) seeds a patch — the cluster plus its
    /// dilation — so the green patches the map already grows become the
    /// corruption; lone terminals stay ordinary.
    pub fn build(map: &WorldMapRes) -> Self {
        let grid = &map.map.grid;
        let (width, height) = (grid.width(), grid.height());
        let is_terminal = |x: i32, y: i32| -> bool {
            x >= 0
                && y >= 0
                && x < width as i32
                && y < height as i32
                && grid.get(x as u32, y as u32).is_some_and(|tile| {
                    map.prototypes[tile as usize].prototype.class == wfc::TileClass::Terminal
                })
        };

        let mut tiles = HashSet::new();
        for y in 0..height as i32 {
            for x in 0..width as i32 {
                if !is_terminal(x, y) {
                    continue;
                }
                let clustered = NEIGHBOURS
                    .iter()
                    .any(|(dx, dy)| is_terminal(x + dx, y + dy));
                if !clustered {
                    continue;
                }
                for dy in -ZONE_DILATION..=ZONE_DILATION {
                    for dx in -ZONE_DILATION..=ZONE_DILATION {
                        if is_terminal(x + dx, y + dy) || is_walkable_floor(map, x + dx, y + dy) {
                            tiles.insert(((x + dx) as u32, (y + dy) as u32));
                        }
                    }
                }
            }
        }
        Self(tiles)
    }

    /// Whether a world-space position sits inside a corrupted zone.
    pub fn contains_world(&self, map_size: (u32, u32), tile_size: f32, position: Vec2) -> bool {
        let units = tile_units(map_size, tile_size, position);
        self.0
            .contains(&(units.x.floor() as u32, units.y.floor() as u32))
    }
}

/// The 8-way neighbour offsets for cluster detection.
const NEIGHBOURS: [(i32, i32); 8] = [
    (-1, -1),
    (0, -1),
    (1, -1),
    (-1, 0),
    (1, 0),
    (-1, 1),
    (0, 1),
    (1, 1),
];

/// Walkable non-terminal cell (floor) — dilation covers walkable tiles but
/// never walls (walls keep their hard edges).
fn is_walkable_floor(map: &WorldMapRes, x: i32, y: i32) -> bool {
    let (width, height) = (map.map.grid.width() as i32, map.map.grid.height() as i32);
    x >= 0
        && y >= 0
        && x < width
        && y < height
        && map.map.grid.get(x as u32, y as u32).is_some_and(|tile| {
            map.prototypes[tile as usize].prototype.class.walkable()
                && map.prototypes[tile as usize].prototype.class != wfc::TileClass::Terminal
        })
}

/// The aggro notice range for a player at `player_in_zone`: the zone's
/// stealth bonus subtracts from the base, floored at one — walking through
/// corruption can bend detection but never fully hide the player.
pub fn zone_notice(base: u32, player_in_zone: bool) -> u32 {
    if player_in_zone {
        base.saturating_sub(ZONE_STEALTH_STEPS).max(1)
    } else {
        base
    }
}

/// Marks an enemy standing in corruption: its shield bleeds a plate per
/// drain interval.
#[derive(Component, Debug)]
struct Draining(Timer);

/// The corruption overlay's material: a transparent glitch layer over the
/// zone tiles — static, data rain and the shared burst clock. Shader at
/// `assets/shaders/corrupted_tilemap.wgsl`.
#[derive(AsBindGroup, TypePath, Debug, Clone, Default, Asset)]
pub struct CorruptedTilemapMaterial {}

impl MaterialTilemap for CorruptedTilemapMaterial {
    fn fragment_shader() -> ShaderRef {
        "shaders/corrupted_tilemap.wgsl".into()
    }
}

pub struct CorruptionPlugin;

impl Plugin for CorruptionPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(MaterialTilemapPlugin::<CorruptedTilemapMaterial>::default())
            .add_systems(
                Update,
                (drain_shields, corruption_audio)
                    .run_if(in_state(crate::skills::GameState::Playing)),
            );
    }
}

/// Bleed every shield standing in corruption — the player's barrier (a
/// resource, timed by a local clock that exists while in-zone) and enemy
/// shields (a per-enemy `Draining` timer inserted on entry, removed on
/// exit).
#[allow(clippy::type_complexity)]
#[allow(clippy::too_many_arguments)]
fn drain_shields(
    mut commands: Commands,
    time: Res<Time>,
    zones: Res<CorruptedZones>,
    map: Res<WorldMapRes>,
    config: Res<MapConfig>,
    player: Single<&Position, With<crate::Player>>,
    mut barrier: ResMut<Barrier>,
    mut player_drain: Local<Option<Timer>>,
    mut enemies: Query<(Entity, &Position, &mut Shield, Option<&mut Draining>)>,
) {
    let map_size = (map.map.grid.width(), map.map.grid.height());

    // The player's barrier bleeds while standing in corruption.
    if zones.contains_world(map_size, config.tile_size, player.0) {
        let timer = player_drain
            .get_or_insert_with(|| Timer::from_seconds(DRAIN_INTERVAL_SECS, TimerMode::Once));
        timer.tick(time.delta());
        if timer.is_finished() {
            barrier.plates = barrier.plates.saturating_sub(1);
            timer.reset();
            tracing::info!(
                plates = barrier.plates,
                "corruption drained a barrier plate"
            );
        }
    } else {
        *player_drain = None;
    }

    // Enemy shields bleed the same way, via the per-enemy timer.
    for (entity, pos, shield, mut draining) in &mut enemies {
        let in_zone = zones.contains_world(map_size, config.tile_size, pos.0);
        match (in_zone, draining.as_deref_mut()) {
            (true, None) => {
                if let Ok(mut entity_commands) = commands.get_entity(entity) {
                    entity_commands.insert(Draining(Timer::from_seconds(
                        DRAIN_INTERVAL_SECS,
                        TimerMode::Once,
                    )));
                }
            }
            (true, Some(draining)) => {
                draining.0.tick(time.delta());
                if draining.0.is_finished() {
                    let Shield { plates, regen: _ } = shield.into_inner();
                    let before = *plates;
                    *plates = plates.saturating_sub(1);
                    draining.0.reset();
                    if *plates != before {
                        tracing::info!(plates = *plates, "corruption drained an enemy shield");
                    }
                }
            }
            (false, Some(_)) => {
                if let Ok(mut entity_commands) = commands.get_entity(entity) {
                    entity_commands.remove::<Draining>();
                }
            }
            (false, None) => {}
        }
    }
}

/// The player crossing a zone boundary is a sonic trigger: a dissonant
/// glitch stab entering, a resolve blip leaving.
fn corruption_audio(
    player: Single<&Position, With<crate::Player>>,
    zones: Res<CorruptedZones>,
    map: Res<WorldMapRes>,
    config: Res<MapConfig>,
    mut inside: Local<bool>,
    bridge: Res<BridgeTx>,
) {
    let map_size = (map.map.grid.width(), map.map.grid.height());
    let now_in = zones.contains_world(map_size, config.tile_size, player.0);
    if now_in != *inside {
        bridge.send(GameAudioEvent::Corruption { entered: now_in });
        *inside = now_in;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::world::{MapConfig, TILE_SIZE};
    use wfc::{GeneratorConfig, prototype_set};

    /// A map built from a fixed seed, plus its config.
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
        let generated = wfc::generate(&config.generator).expect("generation succeeds");
        let world = WorldMapRes {
            map: generated,
            prototypes: prototype_set(
                config.generator.wall_weight,
                config.generator.terminal_weight,
            ),
        };
        (world, config)
    }

    /// Force a terminal at a cell (overwriting whatever was there).
    fn force_terminal(map: &mut WorldMapRes, x: u32, y: u32) {
        let terminal = map
            .prototypes
            .iter()
            .position(|p| p.prototype.class == wfc::TileClass::Terminal)
            .expect("the set has a terminal") as u32;
        map.map.grid.set(x, y, terminal);
    }

    #[test]
    fn lone_terminals_never_seed_zones() {
        let (mut map, _config) = test_map(7);
        force_terminal(&mut map, 5, 5);
        // One terminal, all floor neighbours: no cluster, no zone.
        let zones = CorruptedZones::build(&map);
        assert!(zones.0.is_empty());
    }

    #[test]
    fn clustered_terminals_seed_dilated_zones() {
        let (mut map, _config) = test_map(7);
        // Two adjacent terminals seed a patch.
        force_terminal(&mut map, 5, 5);
        force_terminal(&mut map, 6, 5);
        let zones = CorruptedZones::build(&map);
        // The seeds...
        assert!(zones.0.contains(&(5, 5)));
        assert!(zones.0.contains(&(6, 5)));
        // ...their dilated ring (including the cell between them)...
        assert!(zones.0.contains(&(5, 4)));
        assert!(zones.0.contains(&(6, 4)));
        assert!(zones.0.contains(&(5, 6)));
        // ...but nothing two steps away.
        assert!(!zones.0.contains(&(5, 7)));
        assert!(!zones.0.contains(&(3, 5)));
    }

    #[test]
    fn zone_dilation_never_covers_walls() {
        let (mut map, _config) = test_map(7);
        force_terminal(&mut map, 5, 5);
        force_terminal(&mut map, 6, 5);
        // Wall the cell above the cluster; dilation skips it.
        let wall = map
            .prototypes
            .iter()
            .position(|p| p.prototype.class == wfc::TileClass::Wall)
            .expect("the set has a wall") as u32;
        map.map.grid.set(5, 4, wall);
        map.map.grid.set(6, 4, wall);
        let zones = CorruptedZones::build(&map);
        assert!(!zones.0.contains(&(5, 4)));
        assert!(!zones.0.contains(&(6, 4)));
        assert!(zones.0.contains(&(5, 6)));
    }

    #[test]
    fn zone_stealth_shaves_two_steps_and_floors_at_one() {
        assert_eq!(zone_notice(8, false), 8);
        assert_eq!(zone_notice(8, true), 6);
        // Stacks with the cloak's sneak range, floored at one.
        assert_eq!(zone_notice(3, true), 1);
        assert_eq!(zone_notice(2, true), 1);
        assert_eq!(zone_notice(1, true), 1);
    }

    #[test]
    fn contains_world_maps_positions_to_tiles() {
        let (mut map, config) = test_map(7);
        force_terminal(&mut map, 5, 5);
        force_terminal(&mut map, 6, 5);
        let zones = CorruptedZones::build(&map);
        let (width, height) = (map.map.grid.width(), map.map.grid.height());
        let tile_size = config.tile_size;
        // The seeded tile's centre is in-zone...
        let centre = crate::world::tile_world_pos((width, height), (5, 5), tile_size);
        assert!(zones.contains_world((width, height), tile_size, centre));
        // ...and a cell two steps away is not.
        let outside = crate::world::tile_world_pos((width, height), (5, 8), tile_size);
        assert!(!zones.contains_world((width, height), tile_size, outside));
    }
}
