//! Shield energy domes: a custom 2D material replacing the gizmo rings.
//! Each dome is a translucent hex-faceted bubble whose rim shows the plate
//! pool as angular arcs — cracked plates go dark, regrowth relights them —
//! with a brightness flash decaying after every hit. All animation runs on
//! the GPU from globals time; the CPU side only pushes state on change.
//!
//! The shader lives in `assets/shaders/shield_dome.wgsl` and hot-reloads
//! while the game (or the shader preview) runs.

use bevy::asset::Asset;
use bevy::color::LinearRgba;
use bevy::math::primitives::Circle;
use bevy::mesh::Mesh2d;
use bevy::prelude::*;
use bevy::render::render_resource::{AsBindGroup, ShaderType};
use bevy::shader::ShaderRef;
use bevy::sprite_render::{AlphaMode2d, Material2d, Material2dPlugin, MeshMaterial2d};

use crate::combat::SHIELD_RADIUS;
use crate::enemies::{ENEMY_SHIELD_PLATES, Shield};
use crate::world::TILE_SIZE;

/// Dome radius for the barrier — slightly wider than the reflect dome
/// ([`SHIELD_RADIUS`]) so the two never coincide when both are up.
const BARRIER_RADIUS: f32 = TILE_SIZE * 1.35;

/// Cyan — the shared shield language of domes and reflected shots.
const REFLECT_TINT: LinearRgba = LinearRgba::new(0.13, 0.79, 1.0, 1.0);
/// Violet — the barrier's HUD bar colour, carried onto its dome.
const BARRIER_TINT: LinearRgba = LinearRgba::new(0.45, 0.13, 1.0, 1.0);

/// Per-dome state pushed to the GPU. All f32 so the WGSL struct needs no
/// alignment padding tricks.
#[derive(ShaderType, Clone, Copy, Debug)]
pub struct DomeData {
    /// Lit plates over the maximum, 0..1 (drives the plate arcs).
    pub plate_fraction: f32,
    /// Seconds since the last plate crack — drives the hit flash.
    pub hit_age: f32,
    pub tint_r: f32,
    pub tint_g: f32,
    pub tint_b: f32,
    pub tint_a: f32,
    pub _pad0: f32,
    pub _pad1: f32,
}

/// The shield dome material: one material instance per dome entity, so the
/// uniforms above are effectively per-entity state.
#[derive(AsBindGroup, TypePath, Debug, Clone, Asset)]
pub struct ShieldDomeMaterial {
    #[uniform(0)]
    dome: DomeData,
}

impl ShieldDomeMaterial {
    /// A dome material from its GPU state — also the preview example's
    /// entry point for laying out inspection domes.
    pub fn new(dome: DomeData) -> Self {
        Self { dome }
    }

    /// Overwrite the hit age — how the preview example loops the flash
    /// curve for inspection.
    pub fn set_hit_age(&mut self, secs: f32) {
        self.dome.hit_age = secs;
    }
}

impl Material2d for ShieldDomeMaterial {
    fn fragment_shader() -> ShaderRef {
        "shaders/shield_dome.wgsl".into()
    }

    fn alpha_mode(&self) -> AlphaMode2d {
        AlphaMode2d::Blend
    }
}

/// Dome owned by a shielded enemy — a child entity that follows it around.
#[derive(Component)]
struct EnemyDome;

/// Dome for the player's reflect shield, visible only while it is up.
#[derive(Component)]
struct ReflectDome;

/// Dome for the barrier, visible while owned with plates remaining.
#[derive(Component)]
struct BarrierDome;

/// GPU state for a dome, derived from a plate pool: the lit fraction, and
/// the age of the last hit — but only while the pool is below maximum. At
/// full strength the regen timer no longer ticks, so its elapsed time would
/// freeze near zero and the dome would flash forever; a large age reads as
/// "quiet" instead.
fn dome_data(plates: u32, max_plates: u32, regen_elapsed: f32, tint: LinearRgba) -> DomeData {
    let plate_fraction = (plates as f32 / max_plates.max(1) as f32).min(1.0);
    let hit_age = if plates >= max_plates {
        999.0
    } else {
        regen_elapsed
    };
    DomeData {
        plate_fraction,
        hit_age,
        tint_r: tint.red,
        tint_g: tint.green,
        tint_b: tint.blue,
        tint_a: tint.alpha,
        _pad0: 0.0,
        _pad1: 0.0,
    }
}

pub struct ShieldFxPlugin;

impl Plugin for ShieldFxPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(Material2dPlugin::<ShieldDomeMaterial>::default())
            .add_systems(
                Update,
                (
                    spawn_domes,
                    spawn_player_domes,
                    sync_enemy_domes,
                    sync_barrier_dome,
                    toggle_reflect_dome,
                    toggle_barrier_dome,
                )
                    .run_if(in_state(crate::skills::GameState::Playing)),
            );
    }
}

/// Spawn a dome child for every newly shielded enemy. Children despawn with
/// their parent, so descent cleanup carries them away for free.
fn spawn_domes(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<ShieldDomeMaterial>>,
    shields: Query<(Entity, &Sprite), Added<Shield>>,
) {
    for (entity, sprite) in &shields {
        let radius = sprite.custom_size.unwrap_or(Vec2::splat(TILE_SIZE)).x * 0.5 + 4.0;
        commands.spawn((
            EnemyDome,
            Mesh2d(meshes.add(Circle::new(radius))),
            MeshMaterial2d(materials.add(ShieldDomeMaterial::new(dome_data(
                ENEMY_SHIELD_PLATES,
                ENEMY_SHIELD_PLATES,
                0.0,
                REFLECT_TINT,
            )))),
            // Slightly above the enemy sprite so the dome renders over it.
            Transform::from_xyz(0.0, 0.0, 0.1),
            ChildOf(entity),
        ));
    }
}

/// Spawn the player's reflect and barrier domes once, hidden until owned.
fn spawn_player_domes(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<ShieldDomeMaterial>>,
    players: Query<Entity, Added<crate::Player>>,
) {
    for entity in &players {
        // The two domes differ in marker type, so they spawn separately:
        // the reflect dome's uniforms are static (no plate pool), while the
        // barrier dome's arcs follow the barrier pool.
        commands.spawn((
            ReflectDome,
            Mesh2d(meshes.add(Circle::new(SHIELD_RADIUS))),
            MeshMaterial2d(materials.add(ShieldDomeMaterial::new(dome_data(
                1,
                1,
                0.0,
                REFLECT_TINT,
            )))),
            Transform::from_xyz(0.0, 0.0, 0.12),
            Visibility::Hidden,
            ChildOf(entity),
        ));
        commands.spawn((
            BarrierDome,
            Mesh2d(meshes.add(Circle::new(BARRIER_RADIUS))),
            MeshMaterial2d(materials.add(ShieldDomeMaterial::new(dome_data(
                1,
                1,
                0.0,
                BARRIER_TINT,
            )))),
            Transform::from_xyz(0.0, 0.0, 0.08),
            Visibility::Hidden,
            ChildOf(entity),
        ));
    }
}

/// Push enemy shield state into dome uniforms — only where it moved, so
/// quiet domes cost no asset traffic.
fn sync_enemy_domes(
    mut materials: ResMut<Assets<ShieldDomeMaterial>>,
    domes: Query<(&ChildOf, &MeshMaterial2d<ShieldDomeMaterial>), With<EnemyDome>>,
    shields: Query<&Shield>,
) {
    for (parent, handle) in &domes {
        let Ok(shield) = shields.get(parent.parent()) else {
            continue;
        };
        sync_dome(
            &mut materials,
            handle,
            dome_data(
                shield.plates,
                ENEMY_SHIELD_PLATES,
                shield.regen.elapsed_secs(),
                REFLECT_TINT,
            ),
        );
    }
}

/// Push barrier state into its dome uniform.
fn sync_barrier_dome(
    barrier: Res<crate::combat::Barrier>,
    mut materials: ResMut<Assets<ShieldDomeMaterial>>,
    dome: Query<&MeshMaterial2d<ShieldDomeMaterial>, With<BarrierDome>>,
) {
    for handle in &dome {
        sync_dome(
            &mut materials,
            handle,
            dome_data(
                barrier.plates,
                crate::combat::BARRIER_PLATES,
                barrier.regen.elapsed_secs(),
                BARRIER_TINT,
            ),
        );
    }
}

/// Write `wanted` into the material behind `handle` when it differs beyond
/// frame noise. The flash decays every frame while active, so this is the
/// per-frame path; idle domes never mutate.
fn sync_dome(
    materials: &mut Assets<ShieldDomeMaterial>,
    handle: &MeshMaterial2d<ShieldDomeMaterial>,
    wanted: DomeData,
) {
    let Some(mut material) = materials.get_mut(handle.id()) else {
        return;
    };
    let dome = &material.dome;
    let changed = (dome.plate_fraction - wanted.plate_fraction).abs() > 1e-4
        || (dome.hit_age - wanted.hit_age).abs() > 1.0 / 120.0;
    if changed {
        material.dome = wanted;
    }
}

/// Show the reflect dome exactly while the shield is up. The dome is
/// spawned by a command from another system, so it may not exist yet on
/// early frames — an absent dome simply has nothing to show.
fn toggle_reflect_dome(
    shield: Res<crate::combat::PlayerShield>,
    dome: Option<Single<&mut Visibility, With<ReflectDome>>>,
) {
    let Some(mut dome) = dome else {
        return;
    };
    let wanted = shield_visibility(shield.is_active());
    if **dome != wanted {
        **dome = wanted;
    }
}

/// Show the barrier dome while the unlock is owned and plates remain.
fn toggle_barrier_dome(
    unlocks: Res<crate::skills::AbilityUnlocks>,
    barrier: Res<crate::combat::Barrier>,
    dome: Option<Single<&mut Visibility, With<BarrierDome>>>,
) {
    let Some(mut dome) = dome else {
        return;
    };
    let wanted = shield_visibility(unlocks.barrier && barrier.plates > 0);
    if **dome != wanted {
        **dome = wanted;
    }
}

fn shield_visibility(shown: bool) -> Visibility {
    if shown {
        Visibility::Inherited
    } else {
        Visibility::Hidden
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TINT: LinearRgba = LinearRgba::WHITE;

    #[test]
    fn full_plates_never_flash() {
        // The regen timer stops ticking at full strength; its elapsed time
        // would freeze at the reset value and flash forever. Full pools
        // report a large age instead.
        let data = dome_data(ENEMY_SHIELD_PLATES, ENEMY_SHIELD_PLATES, 0.0, TINT);
        assert_eq!(data.plate_fraction, 1.0);
        assert_eq!(data.hit_age, 999.0);
    }

    #[test]
    fn damaged_shields_report_their_pool_and_flash_age() {
        let data = dome_data(6, 8, 0.1, TINT);
        assert!((data.plate_fraction - 0.75).abs() < 1e-6);
        assert!((data.hit_age - 0.1).abs() < 1e-6);
        // An old hit is quiet again.
        let quiet = dome_data(6, 8, 2.4, TINT);
        assert!((quiet.hit_age - 2.4).abs() < 1e-6);
    }

    #[test]
    fn plate_fraction_clamps_to_full() {
        let data = dome_data(12, 8, 0.0, TINT);
        assert_eq!(data.plate_fraction, 1.0);
    }

    #[test]
    fn empty_pool_reads_dark_without_flash() {
        // An empty pool still takes hits (they leak through), so the age is
        // the regen clock; the fraction is what dims the dome.
        let data = dome_data(0, 8, 1.0, TINT);
        assert_eq!(data.plate_fraction, 0.0);
        assert!((data.hit_age - 1.0).abs() < 1e-6);
    }

    #[test]
    fn shield_visibility_toggles() {
        assert_eq!(shield_visibility(true), Visibility::Inherited);
        assert_eq!(shield_visibility(false), Visibility::Hidden);
    }
}
