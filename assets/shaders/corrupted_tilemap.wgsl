// Corruption overlay for the corrupted data zones.
//
// Runs as the fragment stage of a bevy_ecs_tilemap custom material
// (CorruptedTilemapMaterial) on a sparse tilemap holding tiles only in
// corrupted cells. Draws sickly green static and falling data-stream
// columns over the base tiles, with the same burst clock as the base
// shader's artefact pass — the whole zone glitches in unison.
//
// Tunables are the constants below; edits hot-reload while the game (or
// `cargo run -p game --example shader_preview`) is running.

#import bevy_ecs_tilemap::vertex_output::MeshVertexOutput
#import bevy_sprite::mesh2d_view_bindings::{view, globals}

// The overlay's own noise texture (the tilemap pipeline binds one; the
// pattern is procedural and barely uses it).
@group(2) @binding(0) var noise_texture: texture_2d_array<f32>;
@group(2) @binding(1) var noise_sampler: sampler;

// Burst schedule — identical constants to the base shader's artefact pass,
// so overlay and base glitch in unison.
const BURST_INTERVAL: f32 = 3.0;
const BURST_LENGTH: f32 = 0.3;
// Calm and burst alpha ceilings for the overlay.
const CALM_ALPHA: f32 = 0.16;
const BURST_ALPHA: f32 = 0.42;
// Data-rain shaping: columns per tile, fall speed (columns/second) and the
// streak duty cycle.
const RAIN_COLUMNS: f32 = 6.0;
const RAIN_SPEED: f32 = 3.0;
const RAIN_DUTY: f32 = 0.18;

fn hash(x: f32) -> f32 {
    return fract(sin(x) * 43758.5453);
}

// 1 while a burst is running (the same schedule every zone tile — unison).
fn burst(now: f32) -> f32 {
    let slot = floor(now / BURST_INTERVAL);
    return step(0.7, hash(slot * 91.7));
}

@fragment
fn fragment(in: MeshVertexOutput) -> @location(0) vec4<f32> {
    let p = in.uv.zw; // tile-local uv

    // Static: hashed green noise, re-rolling on a fine time step.
    let step_n = floor(globals.time * 12.0);
    let cell = floor(p * 16.0);
    let static_noise = hash(cell.x * 7.13 + cell.y * 3.71 + step_n * 17.7);

    // Data rain: bright streaks falling down hashed columns.
    let column = floor(p.x * RAIN_COLUMNS);
    let column_phase = hash(column * 12.9);
    let fall = fract(p.y * 2.0 + globals.time * RAIN_SPEED + column_phase);
    let streak = step(1.0 - RAIN_DUTY, fall);

    // The shared burst: everything brightens and the static coarsens.
    let now_bursting = burst(globals.time);
    let energy = 0.35 + 0.3 * static_noise + 0.5 * streak;
    let alpha = mix(CALM_ALPHA, BURST_ALPHA, now_bursting) * (0.5 + 0.5 * energy);

    // Sickly green, brightest at the streaks.
    let green = vec3<f32>(0.15, 1.0, 0.35) * energy + vec3<f32>(0.6, 1.0, 0.5) * streak * 0.4;
    return vec4<f32>(green, clamp(alpha, 0.0, 0.9));
}
