//! Connectivity analysis over collapsed grids: flood-fill the walkable space
//! and choose spawn/exit cells (GDD phase 2).

use std::collections::VecDeque;

use rand::RngExt;
use rand::rngs::SmallRng;

use crate::Grid;
use crate::socket::Direction;
use crate::tiles::WeightedPrototype;

/// Find the largest connected region of walkable tiles. Returns `None` when
/// the grid has no walkable cells (or an uncollapsed cell).
pub(crate) fn largest_walkable_region(
    grid: &Grid,
    prototypes: &[WeightedPrototype],
) -> Option<Vec<(u32, u32)>> {
    let width = grid.width();
    let height = grid.height();
    let mut visited = vec![false; (width * height) as usize];
    let mut largest: Vec<(u32, u32)> = Vec::new();

    for y in 0..height {
        for x in 0..width {
            let start = (y * width + x) as usize;
            if visited[start] {
                continue;
            }
            let tile = grid.get(x, y)?;
            if !prototypes[tile as usize].prototype.class.walkable() {
                visited[start] = true;
                continue;
            }

            // Flood-fill this region, marking cells visited as they are seen
            // so nothing is enqueued twice.
            let mut region = Vec::new();
            let mut queue = VecDeque::new();
            visited[start] = true;
            queue.push_back((x, y));
            while let Some((cx, cy)) = queue.pop_front() {
                region.push((cx, cy));
                for direction in Direction::ALL {
                    let (dx, dy) = direction.delta();
                    let nx = cx as i32 + dx;
                    let ny = cy as i32 + dy;
                    if nx < 0 || ny < 0 || nx >= width as i32 || ny >= height as i32 {
                        continue;
                    }
                    let (nx, ny) = (nx as u32, ny as u32);
                    let neighbour = (ny * width + nx) as usize;
                    if visited[neighbour] {
                        continue;
                    }
                    visited[neighbour] = true;
                    let tile = grid.get(nx, ny)?;
                    if prototypes[tile as usize].prototype.class.walkable() {
                        queue.push_back((nx, ny));
                    }
                }
            }
            if region.len() > largest.len() {
                largest = region;
            }
        }
    }
    if largest.is_empty() {
        None
    } else {
        Some(largest)
    }
}

/// Choose the spawn cell at random within `region`, then the exit as the
/// region cell farthest from spawn by BFS distance — a connected, reasonably
/// long route is guaranteed by construction.
pub(crate) fn choose_spawn_and_exit(
    grid: &Grid,
    prototypes: &[WeightedPrototype],
    region: &[(u32, u32)],
    rng: &mut SmallRng,
) -> ((u32, u32), (u32, u32)) {
    let spawn = region[rng.random_range(0..region.len())];

    // BFS distances from the spawn across walkable cells.
    let width = grid.width();
    let height = grid.height();
    let mut distance: Vec<Option<u32>> = vec![None; (width * height) as usize];
    let mut queue = VecDeque::new();
    distance[(spawn.1 * width + spawn.0) as usize] = Some(0);
    queue.push_back(spawn);
    while let Some((cx, cy)) = queue.pop_front() {
        let current = distance[(cy * width + cx) as usize].expect("dequeued cells have distances");
        for direction in Direction::ALL {
            let (dx, dy) = direction.delta();
            let nx = cx as i32 + dx;
            let ny = cy as i32 + dy;
            if nx < 0 || ny < 0 || nx >= width as i32 || ny >= height as i32 {
                continue;
            }
            let (nx, ny) = (nx as u32, ny as u32);
            let neighbour = (ny * width + nx) as usize;
            if distance[neighbour].is_some() {
                continue;
            }
            let tile = grid.get(nx, ny).expect("grid is fully collapsed");
            if !prototypes[tile as usize].prototype.class.walkable() {
                continue;
            }
            distance[neighbour] = Some(current + 1);
            queue.push_back((nx, ny));
        }
    }

    let mut exit = spawn;
    let mut best = 0u32;
    for &(cx, cy) in region {
        let d = distance[(cy * width + cx) as usize].unwrap_or(0);
        if d > best {
            best = d;
            exit = (cx, cy);
        }
    }
    (spawn, exit)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::socket::Socket;
    use crate::tiles::{TileClass, prototype_set};
    use rand::SeedableRng;

    /// Prototype set plus the indices of the all-path floor and wall tiles.
    fn test_setup() -> (Vec<WeightedPrototype>, usize, usize) {
        let prototypes = prototype_set(0.4, 0.02);
        let wall = prototypes
            .iter()
            .position(|p| p.prototype.class == TileClass::Wall)
            .unwrap();
        let open_floor = prototypes
            .iter()
            .position(|p| {
                p.prototype.class == TileClass::Floor && p.prototype.sockets == [Socket::Path; 4]
            })
            .unwrap();
        (prototypes, wall, open_floor)
    }

    #[test]
    fn largest_region_is_one_side_of_a_wall_split() {
        let (prototypes, wall, floor) = test_setup();
        let mut grid = Grid::new(5, 5);
        for y in 0..5 {
            for x in 0..5 {
                grid.set(x, y, floor as u32);
            }
        }
        // Sealed border of walls, then a wall column down the middle of the
        // interior: the walkable space splits into two disconnected 3-cell
        // halves.
        for xy in 0..5 {
            grid.set(xy, 0, wall as u32);
            grid.set(xy, 4, wall as u32);
            grid.set(0, xy, wall as u32);
            grid.set(4, xy, wall as u32);
        }
        for y in 1..4 {
            grid.set(2, y, wall as u32);
        }
        let region = largest_walkable_region(&grid, &prototypes).expect("walkable region exists");
        assert_eq!(region.len(), 3);
    }

    #[test]
    fn spawn_and_exit_differ_inside_a_connected_region() {
        let (prototypes, _wall, floor) = test_setup();
        let mut grid = Grid::new(7, 7);
        for y in 0..7 {
            for x in 0..7 {
                grid.set(x, y, floor as u32);
            }
        }
        let region = largest_walkable_region(&grid, &prototypes).expect("walkable region exists");
        let mut rng = SmallRng::seed_from_u64(7);
        let (spawn, exit) = choose_spawn_and_exit(&grid, &prototypes, &region, &mut rng);
        // The farthest cell of a 25-cell region is never the spawn itself.
        assert_ne!(spawn, exit);
        assert!(region.contains(&spawn));
        assert!(region.contains(&exit));
    }
}
