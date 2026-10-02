//! Island statistics: how often do generated maps contain walkable regions
//! disconnected from the main region (islands the player can see but never
//! reach)? Usage: `cargo run -p wfc --example island_stats [seeds]`.

use wfc::{GeneratorConfig, prototype_set, walkable_regions};

fn main() {
    let seeds: u32 = std::env::args()
        .nth(1)
        .and_then(|n| n.parse().ok())
        .unwrap_or(200);

    let mut maps_with_islands = 0u32;
    let mut total_islands = 0u32;
    let mut largest_island_cells = 0u32;
    let mut total_secondary_cells = 0u32;
    let mut failed_seeds = 0u32;

    for seed in 0..seeds {
        let config = GeneratorConfig {
            seed: seed as u64,
            ..Default::default()
        };
        // Some seeds exhaust the restart budget; skip them (counted apart).
        let Ok(generated) = wfc::generate(&config) else {
            failed_seeds += 1;
            continue;
        };
        let regions = wfc_regions(&generated.grid, &config);
        let secondary: Vec<u32> = regions
            .iter()
            .map(|r| r.len() as u32)
            .filter(|len| *len > 0)
            .skip(1) // region 0 is the largest (sorted below)
            .collect();
        if !secondary.is_empty() {
            maps_with_islands += 1;
            total_islands += secondary.len() as u32;
            total_secondary_cells += secondary.iter().sum::<u32>();
            largest_island_cells = largest_island_cells.max(*secondary.iter().max().unwrap());
        }
    }

    let generated_maps = seeds - failed_seeds;
    println!(
        "{generated_maps}/{seeds} seeds generated (skipped {failed_seeds} exhausted): \
         {maps_with_islands} maps with islands ({:.1}%), {total_islands} islands total, \
         mean {:.1} cells/island, largest island {largest_island_cells} cells",
        100.0 * maps_with_islands as f32 / generated_maps.max(1) as f32,
        total_secondary_cells as f32 / total_islands.max(1) as f32,
    );
}

/// All 4-connected walkable regions, largest first — the library pass
/// over this depth's prototype set.
fn wfc_regions(grid: &wfc::Grid, config: &GeneratorConfig) -> Vec<Vec<(u32, u32)>> {
    walkable_regions(grid, &prototype_set(config.wall_weight, config.terminal_weight))
}
