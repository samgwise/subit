// Shield energy dome for the subit shields.
//
// Runs as the fragment stage of a custom Material2d on a Circle mesh
// (ShieldDomeMaterial). The dome reads as a translucent hex-faceted energy
// bubble: alpha rises toward the rim, cell borders shimmer while slowly
// spinning, and the rim is divided into 8 angular arcs — one per shield
// plate — that go dark as plates crack and relight as they regrow. A plate
// hit flashes the dome, decaying over a fraction of a second.
//
// Tunables are the constants below; edits hot-reload while the game (or
// `cargo run -p game --example shader_preview`) is running.

#import bevy_sprite::mesh2d_vertex_output::VertexOutput
#import bevy_sprite::mesh2d_view_bindings::{view, globals}

struct DomeData {
    plate_fraction: f32,  // lit plates over the maximum, 0..1
    hit_age: f32,         // seconds since the last plate crack
    tint_r: f32,
    tint_g: f32,
    tint_b: f32,
    tint_a: f32,
    _pad0: f32,
    _pad1: f32,
};
@group(2) @binding(0) var<uniform> dome: DomeData;

const PI: f32 = 3.14159265;
// Angular arcs drawn on the rim — one per shield plate.
const PLATES: f32 = 8.0;
// Hex facet density across the dome and the lattice spin rate (rad/s).
const HEX_SCALE: f32 = 5.0;
const SPIN_SPEED: f32 = 0.2;
// Hit flash decay (higher = snappier).
const FLASH_DECAY: f32 = 8.0;

// Distance to the edge of the hex cell containing p, for a pointy-top
// lattice of unit-ish cells (0 at cell centre, 0.5 at a corner).
fn hex_dist(p: vec2<f32>) -> f32 {
    let q = abs(p);
    return max(q.x * 0.8660254 + q.y * 0.5, q.y);
}

// Distance field over the hex lattice containing p, always positive-mod.
fn hex_edge(p: vec2<f32>) -> f32 {
    let r = vec2<f32>(1.0, 1.7320508);
    let h = r * 0.5;
    let a = ((p % r) + r) % r - h;
    let b = (((p + h) % r) + r) % r - h;
    let gv = select(b, a, dot(a, a) < dot(b, b));
    return hex_dist(gv);
}

fn rot(p: vec2<f32>, angle: f32) -> vec2<f32> {
    let c = cos(angle);
    let s = sin(angle);
    return vec2<f32>(c * p.x - s * p.y, s * p.x + c * p.y);
}

@fragment
fn fragment(mesh: VertexOutput) -> @location(0) vec4<f32> {
    // Unit-disc coordinates from the circle mesh's centred UVs.
    let p = mesh.uv * 2.0 - 1.0;
    let r = length(p);
    let rim = smoothstep(0.45, 1.0, r);

    // Hex facets: energy cells slowly spinning over the dome.
    let hex = hex_edge(rot(p, globals.time * SPIN_SPEED) * HEX_SCALE);
    let facet_line = smoothstep(0.3, 0.5, hex);
    let facets = 0.08 + 0.30 * facet_line;

    // Plate arcs: the rim band is divided into PLATES angular segments;
    // arc i is lit iff i < plates. Cracked plates dim to a faint husk.
    let theta = atan2(p.y, p.x);
    let arc = floor((theta + PI) / (2.0 * PI) * PLATES);
    let plate_lit = step(arc + 0.5, dome.plate_fraction * PLATES);
    let arc_zone = smoothstep(0.62, 0.8, r);
    let plate_mask = 1.0 - arc_zone * (1.0 - plate_lit) * 0.75;

    // Hit flash: bright pulse decaying after a plate cracks (a regrow
    // restarts the clock too — a small shimmer as the segment relights).
    let flash = exp(-dome.hit_age * FLASH_DECAY) * (0.5 + 0.5 * rim);

    let tint = vec3<f32>(dome.tint_r, dome.tint_g, dome.tint_b);
    let energy = facets * plate_mask + rim * 0.5 * plate_mask + flash;
    let colour = tint * energy + vec3<f32>(1.0) * flash * rim * 0.35;
    let alpha = clamp(0.35 * rim + facets * plate_mask + flash, 0.0, 0.9);
    return vec4<f32>(colour, alpha * dome.tint_a);
}
