// Corruption-style death dissolve for the subit corpses.
//
// Runs as the fragment stage of a custom Material2d on a rectangle mesh
// (DissolveMaterial). The corpse comes apart in the corruption's own
// glitch language: hash-blocked dropout eats it from all over, scanlines
// tear out in full rows, blocks jitter their brightness and occasionally
// rotate the body colour's channels, and the eating edge glows
// corruption-green - unclamped, so the camera's bloom spreads it into
// a brief neon rim.
//
// Tunables are the constants below; edits hot-reload while the game (or
// `cargo run -p game --example shader_preview`) is running.

#import bevy_sprite::mesh2d_vertex_output::VertexOutput
#import bevy_sprite::mesh2d_view_bindings::{view, globals}

struct DissolveData {
    progress: f32,  // death progress 0..1 - the eating front
    seed: f32,      // per-corpse noise seed
    tint_r: f32,
    tint_g: f32,
    tint_b: f32,
    tint_a: f32,
    _pad0: f32,
    _pad1: f32,
};
@group(2) @binding(0) var<uniform> corpse: DissolveData;

// Glitch texture: blocks across the sprite, the tear reshuffle cadence,
// the scanline row count, and the glowing rim's width (in noise space).
const BLOCKS: f32 = 9.0;
const RESHUFFLE_SECS: f32 = 0.06;
const SCANLINES: f32 = 24.0;
const EDGE_BAND: f32 = 0.16;
// Corruption green for the eating edge (unclamped green feeds the bloom).
const EDGE_TINT: vec3<f32> = vec3<f32>(0.25, 1.5, 0.5);

fn hash(x: f32) -> f32 {
    return fract(sin(x) * 43758.5453);
}

@fragment
fn fragment(mesh: VertexOutput) -> @location(0) vec4<f32> {
    let uv = mesh.uv;
    let block = floor(uv * BLOCKS);
    let block_seed = block.x + block.y * BLOCKS + corpse.seed * 17.0;
    let step_n = floor(globals.time / RESHUFFLE_SECS);

    // The eating front: a mixed chunky/fine roll against the progress.
    // Chunky blocks go first in a scattered order (the corruption
    // dropout), the fine grain keeps the front granular.
    let chunky = hash(hash(block_seed) + step_n * 57.13);
    let fine = hash(dot(uv, vec2<f32>(12.9898, 78.233)) + corpse.seed);
    let noise = clamp(chunky * 0.75 + fine * 0.25, 0.0, 1.0);
    if (noise < corpse.progress) {
        discard;
    }

    // Scanline tear: whole rows drop out on a fast reshuffle cadence.
    let row = floor(uv.y * SCANLINES);
    if (hash(row + step_n * 3.1 + corpse.seed) < 0.06) {
        discard;
    }

    // The surviving body: the corpse's own colour with per-block
    // brightness jitter, and a quarter-turn channel rot now and then.
    var tint = vec3<f32>(corpse.tint_r, corpse.tint_g, corpse.tint_b);
    let jitter = 0.85 + 0.3 * hash(block_seed * 1.7 + step_n * 11.9);
    tint = tint * jitter;
    let rotate = step(0.9, hash(block_seed * 2.9 + step_n + corpse.seed));
    if (rotate > 0.0) {
        tint = vec3<f32>(tint.g, tint.b, tint.r);
    }

    // The glowing rim: everything between the front and one band ahead
    // burns corruption-green, unclamped so the bloom spreads it.
    let edge = step(corpse.progress, noise) * step(noise, corpse.progress + EDGE_BAND);
    let flicker = 0.7 + 0.5 * hash(block_seed + step_n * 7.7);
    let rim = EDGE_TINT * edge * flicker;

    return vec4<f32>(tint + rim, corpse.tint_a);
}
