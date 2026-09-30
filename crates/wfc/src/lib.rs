//! Pure-Rust Wave Function Collapse (WFC) map generation for the subit
//! prototype.
//!
//! Engine-free by design: generation runs on plain Rust data so it can
//! execute off the ECS thread (GDD phase 1) and be unit tested in isolation.
//! [`generate`] performs the abstract WFC collapse, then a flood-fill
//! reachability pass that guarantees a spawn-to-exit route (GDD phase 2);
//! Bevy instantiation of the result happens later, in Milestone 3.

mod reachability;
mod socket;
mod solver;
mod tiles;

pub use reachability::{FlowField, line_of_sight, walkable_distances};
pub use socket::{Direction, Socket};
pub use tiles::{TileClass, TilePrototype, WeightedPrototype, prototype_set, tiles_are_compatible};

use rand::SeedableRng;
use rand::rngs::SmallRng;

/// A collapsed grid of tile indices into the prototype set ([`None`] whilst a
/// cell is still in superposition).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Grid {
    width: u32,
    height: u32,
    cells: Vec<Option<u32>>,
}

impl Grid {
    pub fn new(width: u32, height: u32) -> Self {
        Self {
            width,
            height,
            cells: vec![None; (width * height) as usize],
        }
    }

    pub fn width(&self) -> u32 {
        self.width
    }

    pub fn height(&self) -> u32 {
        self.height
    }

    pub fn get(&self, x: u32, y: u32) -> Option<u32> {
        self.cells[self.index(x, y)]
    }

    pub fn set(&mut self, x: u32, y: u32, tile: u32) {
        let i = self.index(x, y);
        self.cells[i] = Some(tile);
    }

    fn index(&self, x: u32, y: u32) -> usize {
        (y * self.width + x) as usize
    }
}

/// Configuration for [`generate`].
///
/// `wall_weight` and `terminal_weight` are probability shares: the expected
/// fraction of the map those classes occupy, with the floor variants taking
/// the remainder weighted by open-edge count.
#[derive(Debug, Clone)]
pub struct GeneratorConfig {
    pub width: u32,
    pub height: u32,
    pub seed: u64,
    pub wall_weight: f32,
    pub terminal_weight: f32,
    /// Minimum fraction of interior cells the largest walkable region must
    /// cover before the map is accepted.
    pub min_walkable_ratio: f32,
    /// How many collapse attempts may fail before giving up.
    pub max_restarts: u32,
}

impl Default for GeneratorConfig {
    fn default() -> Self {
        Self {
            width: 48,
            height: 48,
            seed: 0,
            // Measured ceiling: shares at or above ~0.3 let wall adjacency
            // escalate (each wall neighbour eliminates open floor variants),
            // fragmenting the walkable space below the ratio threshold.
            // Robust at the default 48x48; lower to ~0.15 for 64x64+ grids.
            wall_weight: 0.25,
            terminal_weight: 0.02,
            min_walkable_ratio: 0.3,
            max_restarts: 100,
        }
    }
}

/// Why generation failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GenerationError {
    /// Dimensions too small to hold a sealed border plus an interior.
    GridTooSmall,
    /// No valid map was found within the restart budget.
    ExhaustedRestarts { attempts: u32 },
}

/// A fully collapsed map plus its chosen spawn and exit cells.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GeneratedMap {
    pub grid: Grid,
    pub spawn: (u32, u32),
    pub exit: (u32, u32),
}

/// Mix a base seed and attempt index into an unrelated RNG seed. Plain
/// addition would make neighbouring config seeds cycle overlapping derived
/// seeds, so once attempts start failing, different seeds collapse to the
/// same first-viable map.
fn attempt_seed(base: u64, attempt: u32) -> u64 {
    // SplitMix64 finaliser over a uniquely-combined input.
    let mut z = base ^ (attempt as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// Generate a map per the GDD pipeline: abstract WFC collapse (phase 1),
/// then the flood-fill reachability pass with spawn/exit selection (phase
/// 2). Contradictions and undersized walkable regions restart the collapse
/// with a derived seed until a valid map is produced or the restart budget
/// is spent.
pub fn generate(config: &GeneratorConfig) -> Result<GeneratedMap, GenerationError> {
    if config.width < 5 || config.height < 5 {
        return Err(GenerationError::GridTooSmall);
    }
    let prototypes = tiles::prototype_set(config.wall_weight, config.terminal_weight);
    for attempt in 0..config.max_restarts {
        let mut rng = SmallRng::seed_from_u64(attempt_seed(config.seed, attempt));
        let Some(grid) = solver::solve_once(config.width, config.height, &prototypes, &mut rng)
        else {
            continue; // contradiction — restart with the next derived seed
        };
        let Some(region) = reachability::largest_walkable_region(&grid, &prototypes) else {
            continue; // nothing walkable at all
        };
        let interior_cells = ((config.width - 2) * (config.height - 2)) as f32;
        if region.len() as f32 / interior_cells < config.min_walkable_ratio {
            continue; // too cramped for play — restart
        }
        let (spawn, exit) =
            reachability::choose_spawn_and_exit(&grid, &prototypes, &region, &mut rng);
        return Ok(GeneratedMap { grid, spawn, exit });
    }
    Err(GenerationError::ExhaustedRestarts {
        attempts: config.max_restarts,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn different_seeds_generate_differently() {
        let first = generate(&GeneratorConfig {
            width: 24,
            height: 24,
            seed: 1,
            ..Default::default()
        })
        .unwrap();
        let second = generate(&GeneratorConfig {
            width: 24,
            height: 24,
            seed: 2,
            ..Default::default()
        })
        .unwrap();
        assert_ne!(
            first.grid, second.grid,
            "distinct seeds must generate distinct maps"
        );
    }

    #[test]
    fn grid_tracks_dimensions_and_cells() {
        let mut grid = Grid::new(3, 2);
        assert_eq!((grid.width(), grid.height()), (3, 2));
        assert_eq!(grid.get(0, 0), None);
        grid.set(2, 1, 7);
        assert_eq!(grid.get(2, 1), Some(7));
        assert_eq!(grid.get(1, 1), None);
    }
}
