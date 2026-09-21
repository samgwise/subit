//! The neon tilemap material: a custom `MaterialTilemap` whose fragment
//! shader animates the baked wall edge strips and terminals on top of the
//! existing bloom. Shader source lives in `assets/shaders/neon_tilemap.wgsl`
//! and hot-reloads while the game (or the shader preview example) runs.

use bevy::asset::Asset;
use bevy::prelude::*;
use bevy::reflect::TypePath;
use bevy::render::render_resource::AsBindGroup;
use bevy::shader::ShaderRef;
use bevy_ecs_tilemap::prelude::{MaterialTilemap, MaterialTilemapHandle};

/// Animated neon glow for the world tilemap — pulses the lit wall strips
/// and terminals; floors and wall bodies pass through untouched. Tunables
/// are constants in the WGSL, editable live thanks to hot reload.
#[derive(AsBindGroup, TypePath, Debug, Clone, Default, Asset)]
pub struct NeonTilemapMaterial {}

impl MaterialTilemap for NeonTilemapMaterial {
    fn fragment_shader() -> ShaderRef {
        "shaders/neon_tilemap.wgsl".into()
    }
}

/// Shared handle to the one neon material asset every depth renders with.
#[derive(Resource)]
pub struct NeonTilemapHandle(pub MaterialTilemapHandle<NeonTilemapMaterial>);

impl FromWorld for NeonTilemapHandle {
    fn from_world(world: &mut World) -> Self {
        let mut materials = world.resource_mut::<bevy::asset::Assets<NeonTilemapMaterial>>();
        Self(MaterialTilemapHandle::from(
            materials.add(NeonTilemapMaterial::default()),
        ))
    }
}
