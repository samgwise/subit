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

// Atlas columns: 0 floor, 1-16 wall autotiles, 17 terminal, 18-33 cracked
// wall autotiles (they pulse like the solid walls).
const WALL_TILE_FIRST: i32 = 1;
const WALL_TILE_LAST: i32 = 16;
const TERMINAL_TILE: i32 = 17;
const CRACKED_TILE_FIRST: i32 = 18;
const CRACKED_TILE_LAST: i32 = 33;

// Corruption: the zone mask (one pixel per map cell, red > 0.5 = corrupted)
// is read at the fragment's tile position; the artefacts share the burst
// clock with the corruption overlay so the whole zone glitches in unison.
@group(3) @binding(0) var zone_mask: texture_2d<f32>;

// Burst schedule: a ~0.3 s glitch burst every ~3 s, hash-scheduled.
const BURST_INTERVAL: f32 = 3.0;
const BURST_LENGTH: f32 = 0.3;
// Blocks across a tile for the displacement/dropout hashing, and the
// re-shuffle cadence during a burst.
const GLITCH_BLOCKS: f32 = 8.0;
const RESHUFFLE_SECS: f32 = 0.05;
// Displacement reach (fraction of a tile) and the dropout share.
const DISPLACE_AMOUNT: f32 = 0.25;
const DROPOUT_SHARE: f32 = 0.15;

fn hash(x: f32) -> f32 {
    return fract(sin(x) * 43758.5453);
}

// 1 while a burst is running (the same schedule every zone tile — unison).
fn burst(now: f32) -> f32 {
    let slot = floor(now / BURST_INTERVAL);
    return step(0.7, hash(slot * 91.7));
}

// The corruption artefacts applied to a tile-local uv: block displacement,
// scanline tear, dropout, colour rot. Returns the sample uv (possibly
// displaced), whether the block survives (false = dropout), and the colour
// rotation mode.
struct Glitch {
    uv: vec2<f32>,
    alive: f32,
    rotate: f32,
}

fn glitch_uv(tile_uv: vec2<f32>, now: f32, strength: f32) -> Glitch {
    let block = floor(tile_uv * GLITCH_BLOCKS);
    let step_n = floor(now / RESHUFFLE_SECS);
    let seed = block.x + block.y * GLITCH_BLOCKS;
    let roll = hash(hash(seed) + step_n * 57.13);
    let shift = (roll - 0.5) * DISPLACE_AMOUNT * strength;
    let tear = (hash(floor(tile_uv.y * 32.0) + step_n * 3.1) - 0.5) * 0.12 * strength;
    let uv = clamp(
        tile_uv + vec2<f32>(shift + tear, shift * 0.6),
        vec2<f32>(0.001),
        vec2<f32>(0.999),
    );
    let alive = step(DROPOUT_SHARE, hash(hash(seed * 1.7) + step_n * 11.9));
    let rotate = step(0.9, hash(seed * 2.9 + step_n));
    return Glitch(uv, alive, rotate);
}

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
    // Corruption: is this tile inside a zone, and is a burst running?
    let corrupted = textureLoad(zone_mask, vec2<i32>(in.storage_position), 0).r;
    let glitch = corrupted * burst(globals.time);
    var sample_uv = in.uv.xy;
    var alive = 1.0;
    var rotate = 0.0;
    if (glitch > 0.0) {
        let artefact = glitch_uv(in.uv.xy, globals.time, glitch);
        sample_uv = artefact.uv;
        alive = artefact.alive;
        rotate = artefact.rotate;
    }
    let sampled = textureSample(sprite_texture, sprite_sampler, sample_uv, in.tile_id);
    var base = sampled * in.color;
    // Colour rot: occasionally hand the pixel's channels a quarter turn.
    if (rotate > 0.0) {
        base = vec4<f32>(sampled.g, sampled.b, sampled.r, sampled.a) * in.color;
    }
    if (base.a < 0.001 || alive < 0.5) {
        discard;
    }

    // Neon pass: pump the baked lit strips and the terminal; everything
    // else (floors, wall bodies) stays exactly as baked.
    var glow = 0.0;
    if ((in.tile_id >= WALL_TILE_FIRST && in.tile_id <= WALL_TILE_LAST)
        || (in.tile_id >= CRACKED_TILE_FIRST && in.tile_id <= CRACKED_TILE_LAST)) {
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
