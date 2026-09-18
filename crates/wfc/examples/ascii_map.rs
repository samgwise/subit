//! Print an ASCII rendering of a generated map:
//! `.` floor, `#` wall, `T` terminal, plus `S` spawn and `E` exit.
//!
//! ```sh
//! cargo run -p wfc --example ascii_map [seed] [width] [height] [wall_share] [min_walkable_ratio_percent]
//! ```

use wfc::{GeneratorConfig, TileClass, generate, prototype_set};

fn main() {
    let args = std::env::args().skip(1);
    let parsed: Vec<u64> = args.filter_map(|arg| arg.parse().ok()).collect();
    let map_arg = |index: usize, default: u64| parsed.get(index).copied().unwrap_or(default);
    let config = GeneratorConfig {
        seed: map_arg(0, 0),
        width: map_arg(1, 48) as u32,
        height: map_arg(2, 48) as u32,
        wall_weight: map_arg(3, 25) as f32 / 100.0,
        min_walkable_ratio: map_arg(4, 30) as f32 / 100.0,
        ..Default::default()
    };
    let map = generate(&config).expect("map generation failed");
    let prototypes = prototype_set(config.wall_weight, config.terminal_weight);

    for y in 0..config.height {
        let mut line = String::with_capacity(config.width as usize);
        for x in 0..config.width {
            let tile = map.grid.get(x, y).expect("fully collapsed");
            let glyph = match prototypes[tile as usize].prototype.class {
                TileClass::Floor => '.',
                TileClass::Wall => '#',
                TileClass::Terminal => 'T',
            };
            if (x, y) == map.spawn {
                line.push('S');
            } else if (x, y) == map.exit {
                line.push('E');
            } else {
                line.push(glyph);
            }
        }
        println!("{line}");
    }
    println!(
        "seed: {}  spawn: {:?}  exit: {:?}",
        config.seed, map.spawn, map.exit
    );
}
