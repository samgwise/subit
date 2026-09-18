//! Wave Function Collapse core: entropy-guided collapse with worklist
//! propagation and restart-on-contradiction.

use rand::RngExt;
use rand::rngs::SmallRng;

use crate::Grid;
use crate::socket::Direction;
use crate::tiles::{TileClass, WeightedPrototype, tiles_are_compatible};

#[derive(Debug, Clone, Copy)]
struct Cell {
    x: u32,
    y: u32,
}

/// Option sets as u32 bitmasks over the prototype indices, one per cell.
struct Board {
    width: u32,
    height: u32,
    /// Bit `i` set in a cell's mask means prototype `i` is still possible
    /// there; a mask of zero is a contradiction.
    options: Vec<u32>,
}

impl Board {
    fn new(width: u32, height: u32, prototype_count: usize) -> Self {
        assert!(
            prototype_count < 32,
            "bitmask option sets cap at 32 prototypes"
        );
        let full = (1u32 << prototype_count) - 1;
        Self {
            width,
            height,
            options: vec![full; (width * height) as usize],
        }
    }

    fn index(&self, x: u32, y: u32) -> usize {
        (y * self.width + x) as usize
    }

    fn options_at(&self, x: u32, y: u32) -> u32 {
        self.options[self.index(x, y)]
    }

    fn set_options(&mut self, x: u32, y: u32, mask: u32) {
        let i = self.index(x, y);
        self.options[i] = mask;
    }

    fn collapse(&mut self, x: u32, y: u32, prototype_index: usize) {
        self.set_options(x, y, 1 << prototype_index);
    }

    /// The sole remaining option of a collapsed cell.
    fn only_option(&self, x: u32, y: u32) -> u32 {
        self.options_at(x, y).trailing_zeros()
    }
}

/// Prototype indices still possible under a bitmask.
fn iter_options(mask: u32) -> impl Iterator<Item = usize> {
    (0..32).filter(move |&i| mask & (1u32 << i) != 0)
}

enum Selection {
    /// The chosen cell to collapse next.
    Collapsed(Cell),
    /// Every cell is decided.
    Complete,
    /// Some cell has no remaining options.
    Contradiction,
}

/// Pick the uncollapsed cell with the fewest remaining options, breaking
/// ties randomly.
fn select_cell(board: &Board, rng: &mut SmallRng) -> Selection {
    let mut min_count = u32::MAX;
    let mut candidates: Vec<Cell> = Vec::new();
    for y in 0..board.height {
        for x in 0..board.width {
            let count = board.options_at(x, y).count_ones();
            match count {
                0 => return Selection::Contradiction,
                1 => continue, // already collapsed
                _ if count < min_count => {
                    min_count = count;
                    candidates.clear();
                    candidates.push(Cell { x, y });
                }
                _ if count == min_count => candidates.push(Cell { x, y }),
                _ => {}
            }
        }
    }
    if candidates.is_empty() {
        return Selection::Complete;
    }
    let cell = candidates[rng.random_range(0..candidates.len())];
    Selection::Collapsed(cell)
}

/// Weighted-random choice among a cell's remaining prototypes.
fn weighted_pick(
    board: &Board,
    cell: Cell,
    prototypes: &[WeightedPrototype],
    rng: &mut SmallRng,
) -> usize {
    let mask = board.options_at(cell.x, cell.y);
    let total: f32 = iter_options(mask).map(|i| prototypes[i].weight).sum();
    let mut pick = rng.random_range(0.0..total);
    for i in iter_options(mask) {
        pick -= prototypes[i].weight;
        if pick <= 0.0 {
            return i;
        }
    }
    // Float rounding can leave `pick` marginally above zero; fall back to
    // the last remaining option.
    iter_options(mask).last().expect("cell has options")
}

/// Filter neighbouring option sets until stable. Returns `false` on
/// contradiction (some cell left with no options).
fn propagate(board: &mut Board, prototypes: &[WeightedPrototype], mut worklist: Vec<Cell>) -> bool {
    while let Some(cell) = worklist.pop() {
        let cell_mask = board.options_at(cell.x, cell.y);
        for direction in Direction::ALL {
            let (dx, dy) = direction.delta();
            let nx = cell.x as i32 + dx;
            let ny = cell.y as i32 + dy;
            if nx < 0 || ny < 0 || nx >= board.width as i32 || ny >= board.height as i32 {
                continue;
            }
            let (nx, ny) = (nx as u32, ny as u32);
            let neighbour_mask = board.options_at(nx, ny);
            let mut filtered = 0u32;
            for p in iter_options(neighbour_mask) {
                // The neighbour sits in `direction` of the cell, so the cell
                // sits in `direction.opposite()` of the neighbour: the pair is
                // legal when the cell still has some option that fits.
                let neighbour_prototype = &prototypes[p].prototype;
                let supported = iter_options(cell_mask).any(|q| {
                    tiles_are_compatible(
                        &prototypes[q].prototype,
                        neighbour_prototype,
                        direction.opposite(),
                    )
                });
                if supported {
                    filtered |= 1 << p;
                }
            }
            if filtered != neighbour_mask {
                if filtered == 0 {
                    return false;
                }
                board.set_options(nx, ny, filtered);
                worklist.push(Cell { x: nx, y: ny });
            }
        }
    }
    true
}

/// Attempt one full collapse of a `width x height` board. Returns `None` on
/// contradiction; the caller restarts with a fresh derived seed.
pub(crate) fn solve_once(
    width: u32,
    height: u32,
    prototypes: &[WeightedPrototype],
    rng: &mut SmallRng,
) -> Option<Grid> {
    let wall_index = prototypes
        .iter()
        .position(|weighted| weighted.prototype.class == TileClass::Wall)
        .expect("prototype set must contain a wall tile");
    let mut board = Board::new(width, height, prototypes.len());

    // Seal the map: pre-collapse every border cell to the wall tile.
    let mut frontier = Vec::new();
    for x in 0..width {
        for y in [0, height - 1] {
            board.collapse(x, y, wall_index);
            frontier.push(Cell { x, y });
        }
    }
    for y in 1..height - 1 {
        for x in [0, width - 1] {
            board.collapse(x, y, wall_index);
            frontier.push(Cell { x, y });
        }
    }
    if !propagate(&mut board, prototypes, frontier) {
        return None;
    }

    loop {
        match select_cell(&board, rng) {
            Selection::Complete => break,
            Selection::Contradiction => return None,
            Selection::Collapsed(cell) => {
                let prototype_index = weighted_pick(&board, cell, prototypes, rng);
                board.collapse(cell.x, cell.y, prototype_index);
                if !propagate(&mut board, prototypes, vec![cell]) {
                    return None;
                }
            }
        }
    }

    let mut grid = Grid::new(width, height);
    for y in 0..height {
        for x in 0..width {
            grid.set(x, y, board.only_option(x, y));
        }
    }
    Some(grid)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rand::SeedableRng;

    #[test]
    fn seeds_produce_distinct_streams() {
        let sample = |seed: u64| {
            let mut rng = SmallRng::seed_from_u64(seed);
            (0..8)
                .map(|_| rng.random_range(0.0..100.0_f32))
                .collect::<Vec<f32>>()
        };
        assert_ne!(
            sample(1),
            sample(2),
            "distinct seeds must give distinct streams"
        );
    }

    #[test]
    fn the_same_seed_gives_the_same_stream() {
        let sample = |seed: u64| {
            let mut rng = SmallRng::seed_from_u64(seed);
            (0..8)
                .map(|_| rng.random_range(0.0..100.0_f32))
                .collect::<Vec<f32>>()
        };
        assert_eq!(sample(7), sample(7));
    }

    #[test]
    fn different_seeds_collapse_differently() {
        let prototypes = crate::tiles::prototype_set(0.15, 0.02);
        let first = solve_once(24, 24, &prototypes, &mut SmallRng::seed_from_u64(1)).unwrap();
        let second = solve_once(24, 24, &prototypes, &mut SmallRng::seed_from_u64(2)).unwrap();
        assert_ne!(
            first, second,
            "distinct seeds must collapse to distinct maps"
        );
    }
}
