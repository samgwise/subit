//! Death dissolve: corpses and broken walls go out in the corruption's own
//! glitch language — block dropout, scanline tear, channel rot — with a
//! bright corruption-green eating edge that the camera's bloom turns into
//! glow. The corpse shows its own colour while it comes apart.
//!
//! The material follows the shield-dome pattern (`shield_fx.rs`): a
//! per-instance `Material2d` over a rectangle mesh, all animation state in
//! a handful of f32 uniforms, and the shader itself
//! (`assets/shaders/dissolve.wgsl`) hot-reloads like the rest.

use bevy::asset::Asset;
use bevy::math::primitives::Rectangle;
use bevy::mesh::Mesh2d;
use bevy::prelude::*;
use bevy::reflect::TypePath;
use bevy::render::render_resource::{AsBindGroup, ShaderType};
use bevy::shader::ShaderRef;
use bevy::sprite_render::{AlphaMode2d, Material2d, Material2dPlugin, MeshMaterial2d};

/// Seconds a corpse takes to dissolve.
pub const DISSOLVE_SECS: f32 = 0.5;

/// The corpse tint for destroyed cracked walls: the wall atlas body grey
/// (58, 74, 94) — the tile was a wall, so its ghost is too.
pub const WALL_CORPSE_TINT: Color = Color::srgb(0.227, 0.290, 0.369);

/// GPU state for one dissolving corpse. All f32 so the WGSL struct needs
/// no alignment padding tricks.
#[derive(ShaderType, Clone, Copy, Debug)]
pub struct DissolveData {
    /// Death progress 0..1 — the eating front.
    pub progress: f32,
    /// Per-corpse noise seed, so same-looking enemies dissolve differently.
    pub seed: f32,
    pub tint_r: f32,
    pub tint_g: f32,
    pub tint_b: f32,
    pub tint_a: f32,
    pub _pad0: f32,
    pub _pad1: f32,
}

/// The dissolve material: one material instance per corpse, so the
/// uniforms above are effectively per-entity state.
#[derive(AsBindGroup, TypePath, Debug, Clone, Asset)]
pub struct DissolveMaterial {
    #[uniform(0)]
    data: DissolveData,
}

impl DissolveMaterial {
    /// A dissolve material from its GPU state.
    pub fn new(data: DissolveData) -> Self {
        Self { data }
    }

    /// Move the eating front along.
    pub fn set_progress(&mut self, progress: f32) {
        self.data.progress = progress.clamp(0.0, 1.0);
    }
}

impl Material2d for DissolveMaterial {
    fn fragment_shader() -> ShaderRef {
        "shaders/dissolve.wgsl".into()
    }

    fn alpha_mode(&self) -> AlphaMode2d {
        AlphaMode2d::Blend
    }
}

/// A corpse waiting to dissolve: a detached record entity (no transform,
/// no mesh) spawned by the kill funnel or the wall destruction pass, and
/// consumed by the fx system, which turns it into a dissolving quad.
#[derive(Component, Debug, Clone, Copy)]
pub struct CorpseFx {
    /// World-space centre of the corpse quad.
    pub position: Vec2,
    /// The corpse quad's full width and height (world units).
    pub size: Vec2,
    /// The body colour — the enemy's own, or the wall grey.
    pub tint: Color,
    /// Per-corpse noise seed.
    pub seed: f32,
}

/// A live dissolve overlay: the quad is on stage, the timer drives the
/// eating front.
#[derive(Component)]
struct Dissolving {
    timer: Timer,
}

/// The GPU state for a record: the front at zero, the record's tint and
/// seed carried through.
fn dissolve_data(record: &CorpseFx) -> DissolveData {
    let c = record.tint.to_srgba();
    DissolveData {
        progress: 0.0,
        seed: record.seed,
        tint_r: c.red,
        tint_g: c.green,
        tint_b: c.blue,
        tint_a: c.alpha,
        _pad0: 0.0,
        _pad1: 0.0,
    }
}

/// Death progress from elapsed dissolve time.
fn dissolve_progress(elapsed_secs: f32) -> f32 {
    (elapsed_secs / DISSOLVE_SECS).clamp(0.0, 1.0)
}

pub struct DissolvePlugin;

impl Plugin for DissolvePlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(Material2dPlugin::<DissolveMaterial>::default())
            .add_systems(
                Update,
                (spawn_corpse_overlays, tick_dissolves)
                    .run_if(in_state(crate::skills::GameState::Playing)),
            );
    }
}

/// Turn every newly queued record into a dissolving quad just above the
/// enemy layer (z 1.0) and below the drones (1.5).
fn spawn_corpse_overlays(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<DissolveMaterial>>,
    records: Query<(Entity, &CorpseFx)>,
) {
    for (record_entity, record) in &records {
        commands.spawn((
            Dissolving {
                timer: Timer::from_seconds(DISSOLVE_SECS, TimerMode::Once),
            },
            Mesh2d(meshes.add(Rectangle::new(record.size.x, record.size.y))),
            MeshMaterial2d(materials.add(DissolveMaterial::new(dissolve_data(record)))),
            Transform::from_xyz(record.position.x, record.position.y, 1.2),
        ));
        if let Ok(mut entity_commands) = commands.get_entity(record_entity) {
            entity_commands.despawn();
        }
    }
}

/// Push each corpse's eating front along and despawn it once the progress
/// has eaten everything.
fn tick_dissolves(
    mut commands: Commands,
    time: Res<Time>,
    mut materials: ResMut<Assets<DissolveMaterial>>,
    mut corpses: Query<(Entity, &mut Dissolving, &MeshMaterial2d<DissolveMaterial>)>,
) {
    for (entity, mut dissolving, material) in &mut corpses {
        dissolving.timer.tick(time.delta());
        if let Some(mut material) = materials.get_mut(&material.0) {
            material.set_progress(dissolve_progress(dissolving.timer.elapsed_secs()));
        }
        if dissolving.timer.is_finished() {
            commands.entity(entity).despawn();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::world::TILE_SIZE;

    #[test]
    fn the_record_maps_onto_the_material_state() {
        let record = CorpseFx {
            position: Vec2::new(10.0, -4.0),
            size: Vec2::splat(TILE_SIZE),
            tint: Color::srgba(0.95, 0.2, 0.2, 1.0),
            seed: 42.0,
        };
        let data = dissolve_data(&record);
        assert_eq!(data.progress, 0.0, "the front starts unspoiled");
        assert_eq!(data.seed, 42.0);
        let c = record.tint.to_srgba();
        assert_eq!((data.tint_r, data.tint_g, data.tint_b, data.tint_a),
            (c.red, c.green, c.blue, c.alpha));
    }

    #[test]
    fn progress_climbs_with_elapsed_time_and_clamps() {
        assert_eq!(dissolve_progress(0.0), 0.0);
        assert!((dissolve_progress(DISSOLVE_SECS * 0.5) - 0.5).abs() < 1e-5);
        assert_eq!(dissolve_progress(DISSOLVE_SECS), 1.0);
        // Overruns (a slow frame after the finish tick) stay clamped.
        assert_eq!(dissolve_progress(DISSOLVE_SECS * 2.0), 1.0);
    }
}
