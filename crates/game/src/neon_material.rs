//! The neon tilemap material: a custom `MaterialTilemap` whose fragment
//! shader animates the baked wall edge strips and terminals on top of the
//! existing bloom, and glitches the geometry of corrupted-zone tiles (via
//! the zone mask). Shader source lives in
//! `assets/shaders/neon_tilemap.wgsl` and hot-reloads while the game (or
//! the shader preview example) runs.

use bevy::asset::Asset;
use bevy::image::Image;
use bevy::prelude::*;
use bevy::reflect::TypePath;
use bevy::render::render_resource::AsBindGroup;
use bevy::shader::ShaderRef;
use bevy_ecs_tilemap::prelude::{MaterialTilemap, MaterialTilemapHandle};

/// Animated neon glow for the world tilemap — pulses the lit wall strips
/// and terminals; floors and wall bodies pass through untouched except in
/// corrupted zones, where the shader artefacts the sampling. Tunables are
/// constants in the WGSL, editable live thanks to hot reload.
#[derive(AsBindGroup, TypePath, Debug, Clone, Default, Asset)]
pub struct NeonTilemapMaterial {
    /// Corrupted-zone mask: one pixel per map cell, red channel > 0.5 marks
    /// a corrupted cell. Read with textureLoad at the fragment's
    /// storage_position.
    #[texture(0)]
    pub zone_mask: Handle<Image>,
}

impl MaterialTilemap for NeonTilemapMaterial {
    fn fragment_shader() -> ShaderRef {
        "shaders/neon_tilemap.wgsl".into()
    }
}

impl NeonTilemapMaterial {
    /// A neon material over a zone mask — rebuilt each depth (the mask
    /// changes with the map).
    pub fn new(zone_mask: Handle<Image>) -> Self {
        Self { zone_mask }
    }
}

/// Shared handle to the one neon material asset every depth renders with.
#[derive(Resource)]
pub struct NeonTilemapHandle(pub MaterialTilemapHandle<NeonTilemapMaterial>);
