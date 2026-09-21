// Animated neon glow for the subit tilemap.
//
// Runs as the fragment stage of a bevy_ecs_tilemap custom material
// (NeonTilemapMaterial). Tiles keep their baked look — floors, grid lines,
// wall bodies — while the lit edge strips on wall autotiles and the
// terminal green pump on a travelling sine wave. The camera's bloom then
// spreads the boosted pixels into glow.
//
// Tunables are the constants below; edits hot-reload while the game (or
// `cargo run -p game --example shader_preview`) is running.

#import bevy_ecs_tilemap::vertex_output::MeshVertexOutput
#import bevy_sprite::mesh2d_view_bindings::{view, globals}

// Atlas columns: 0 floor, 1-16 wall autotiles, 17 terminal.
const WALL_TILE_FIRST: i32 = 1;
const WALL_TILE_LAST: i32 = 16;
const TERMINAL_TILE: i32 = 17;

// Pulse speed (radians/second), extra brightness on glowing pixels (0..1),
// and the luminance separating baked edge strips (linear ~0.7) from wall
// bodies (~0.07).
const PULSE_SPEED: f32 = 3.0;
const PULSE_STRENGTH: f32 = 0.45;
const EDGE_LUMINANCE: f32 = 0.35;
// Gentler pump for the terminal green.
const TERMINAL_PULSE: f32 = 0.2;

@group(2) @binding(0) var sprite_texture: texture_2d_array<f32>;
@group(2) @binding(1) var sprite_sampler: sampler;

@fragment
fn fragment(in: MeshVertexOutput) -> @location(0) vec4<f32> {
    let base = textureSample(sprite_texture, sprite_sampler, in.uv.xy, in.tile_id) * in.color;
    if (base.a < 0.001) {
        discard;
    }

    // Neon pass: pump the baked lit strips and the terminal; everything
    // else (floors, wall bodies) stays exactly as baked.
    var glow = 0.0;
    if (in.tile_id >= WALL_TILE_FIRST && in.tile_id <= WALL_TILE_LAST) {
        let luminance = dot(base.rgb, vec3<f32>(0.2126, 0.7152, 0.0722));
        if (luminance > EDGE_LUMINANCE) {
            // Phase drifts across the tile so the shimmer travels along
            // the edge rather than blinking in step.
            let phase = (in.uv.z + in.uv.w) * 1.5 * 6.2831853;
            let pulse = 0.5 + 0.5 * sin(globals.time * PULSE_SPEED + phase);
            glow = PULSE_STRENGTH * pulse;
        }
    } else if (in.tile_id == TERMINAL_TILE) {
        let pulse = 0.5 + 0.5 * sin(globals.time * PULSE_SPEED);
        glow = TERMINAL_PULSE * pulse;
    }

    // Boost toward white-cyan; unclamped so bloom picks up the peaks.
    let neon = vec3<f32>(0.6, 0.9, 1.0) * glow;
    return vec4<f32>(base.rgb + neon, base.a);
}
