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

/// Whether the sight line between two points is unobstructed by non-walkable
/// cells. Both points are in continuous tile-space coordinates, where tile
/// (x, y) centres on (x + 0.5, y + 0.5).
///
/// Walks the exact sequence of cells the segment passes through
/// (Amanatides–Woo grid traversal); only cells strictly between the
/// endpoints can block — you stand on your tile and hit the enemy on
/// theirs. Out-of-bounds cells block. A segment crossing a cell corner
/// exactly is blocked when either orthogonal neighbour around that corner
/// is unwalkable — sight can't squeeze through the crack between two
/// diagonal walls.
pub fn line_of_sight(
    grid: &Grid,
    prototypes: &[WeightedPrototype],
    from: (f32, f32),
    to: (f32, f32),
) -> bool {
    let width = grid.width() as i32;
    let height = grid.height() as i32;
    let walkable = |x: i32, y: i32| -> bool {
        if x < 0 || y < 0 || x >= width || y >= height {
            return false;
        }
        let tile = grid.get(x as u32, y as u32).expect("fully collapsed");
        prototypes[tile as usize].prototype.class.walkable()
    };

    let (mut x, mut y) = (from.0.floor() as i32, from.1.floor() as i32);
    let end = (to.0.floor() as i32, to.1.floor() as i32);
    let delta = (to.0 - from.0, to.1 - from.1);
    let step_x = delta.0.partial_sign();
    let step_y = delta.1.partial_sign();

    // Parametric distance t along the segment to the next cell boundary in
    // each axis, and the t-spacing between boundaries. Flat axes never
    // advance (t_max stays infinite).
    let next_boundary_x = if step_x == 1 {
        (x + 1) as f32
    } else {
        x as f32
    };
    let next_boundary_y = if step_y == 1 {
        (y + 1) as f32
    } else {
        y as f32
    };
    let mut t_max_x = if step_x == 0 {
        f32::INFINITY
    } else {
        (next_boundary_x - from.0) / delta.0
    };
    let mut t_max_y = if step_y == 0 {
        f32::INFINITY
    } else {
        (next_boundary_y - from.1) / delta.1
    };
    let t_delta_x = if step_x == 0 {
        f32::INFINITY
    } else {
        1.0 / delta.0.abs()
    };
    let t_delta_y = if step_y == 0 {
        f32::INFINITY
    } else {
        1.0 / delta.1.abs()
    };

    // Hard cap: a segment can never cross more cells than the full grid
    // twice over; the cap only guards against float pathology.
    for _ in 0..=(width + height) * 2 {
        if (x, y) == end {
            return true;
        }
        if t_max_x < t_max_y {
            x += step_x;
            t_max_x += t_delta_x;
        } else if t_max_y < t_max_x {
            y += step_y;
            t_max_y += t_delta_y;
        } else {
            // Corner crossing: conservative sight — the segment passes
            // through the shared corner, and we refuse to let sight squeeze
            // between two diagonal walls.
            if !walkable(x + step_x, y) || !walkable(x, y + step_y) {
                return false;
            }
            x += step_x;
            y += step_y;
            t_max_x += t_delta_x;
            t_max_y += t_delta_y;
        }
        if (x, y) == end {
            return true;
        }
        if !walkable(x, y) {
            return false;
        }
    }
    // Cap exhausted (should not happen): fall back to the end cell's own
    // walkability.
    walkable(end.0, end.1)
}

/// Sign for grid stepping: +1, -1, or 0 for (near enough) zero.
trait PartialSign {
    fn partial_sign(self) -> i32;
}

impl PartialSign for f32 {
    fn partial_sign(self) -> i32 {
        if self > 0.0 {
            1
        } else if self < 0.0 {
            -1
        } else {
            0
        }
    }
}

/// BFS distances from `from` across the walkable cells of a collapsed grid.
///
/// Row-major over `grid.width()`; `None` means the cell is unreachable (a
/// wall, outside the connected region, or `from` itself is not walkable).
/// Consumers use this for reachability guarantees over the finished map —
/// e.g. spawning enemies the player can actually reach.
pub fn walkable_distances(
    grid: &Grid,
    prototypes: &[WeightedPrototype],
    from: (u32, u32),
) -> Vec<Option<u32>> {
    let width = grid.width();
    let height = grid.height();
    let mut distance: Vec<Option<u32>> = vec![None; (width * height) as usize];
    let mut queue = VecDeque::new();
    distance[(from.1 * width + from.0) as usize] = Some(0);
    queue.push_back(from);
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
    distance
}

/// A BFS flow field toward a target cell: every walkable cell knows the
/// neighbouring cell (of 8) with the lowest BFS distance from the target,
/// so agents can descend the gradient to reach it. Built over a collapsed
/// grid; rebuilt whenever the target moves (in the game: when the player
/// changes tile).
pub struct FlowField {
    width: u32,
    height: u32,
    distance: Vec<Option<u32>>,
}

impl FlowField {
    /// Build a flow field across `grid`'s walkable cells toward `target`.
    /// The distance pass is exactly [`walkable_distances`] from the target —
    /// no separate search.
    pub fn build(grid: &Grid, prototypes: &[WeightedPrototype], target: (u32, u32)) -> Self {
        Self {
            width: grid.width(),
            height: grid.height(),
            distance: walkable_distances(grid, prototypes, target),
        }
    }

    /// The agent's BFS distance to the target, in cell steps, at the cell
    /// containing `position` (continuous tile-space coordinates, matching
    /// [`line_of_sight`]). `None` when off-grid, inside a wall, or unreachable
    /// from the target — there is no path.
    pub fn steps(&self, position: (f32, f32)) -> Option<u32> {
        let width = self.width as i32;
        let height = self.height as i32;
        let (x, y) = (position.0.floor() as i32, position.1.floor() as i32);
        if x < 0 || y < 0 || x >= width || y >= height {
            return None;
        }
        self.distance[(y * width + x) as usize]
    }

    /// Steering direction for an agent at `position` (continuous tile-space
    /// coordinates — tile (x, y) centres on (x + 0.5, y + 0.5), matching
    /// [`line_of_sight`]): a unit vector aimed at the centre of the walkable
    /// neighbour with the lowest BFS distance — from the agent's actual
    /// position, so the steering line always ends in the middle of the next
    /// walkable cell and never dead-lifts into a wall corner vertex (the
    /// wedge an agent can't slide out of). `None` when there is nothing to
    /// flow to — off-grid, inside a wall, unreachable from the target, or
    /// arrived at the target itself.
    pub fn direction(&self, position: (f32, f32)) -> Option<(f32, f32)> {
        let width = self.width as i32;
        let height = self.height as i32;
        let (x, y) = (position.0.floor() as i32, position.1.floor() as i32);
        if x < 0 || y < 0 || x >= width || y >= height {
            return None;
        }
        if self.distance[(y * width + x) as usize]? == 0 {
            return None; // arrived at the target
        }
        // Descend the gradient over all 8 neighbours — diagonal steps keep
        // movement out of axis-only zigzags — refusing diagonals that
        // squeeze through a cracked corner (mirrors the `line_of_sight`
        // corner rule).
        let mut best: Option<(u32, i32, i32)> = None;
        for dy in -1..=1 {
            for dx in -1..=1 {
                if dx == 0 && dy == 0 {
                    continue;
                }
                let (nx, ny) = (x + dx, y + dy);
                if nx < 0 || ny < 0 || nx >= width || ny >= height {
                    continue;
                }
                let Some(distance) = self.distance[(ny * width + nx) as usize] else {
                    continue;
                };
                if dx != 0
                    && dy != 0
                    && (self.distance[(y * width + nx) as usize].is_none()
                        || self.distance[((y + dy) * width + x) as usize].is_none())
                {
                    continue; // the diagonal cuts between two walls
                }
                if best.is_none_or(|(best_distance, _, _)| distance < best_distance) {
                    best = Some((distance, dx, dy));
                }
            }
        }
        let (_, dx, dy) = best?;
        // Aim at the chosen neighbour's centre from where the agent actually
        // is. Both hops are clip-free: an orthogonal neighbour's union with
        // this cell is a convex 2x1 block, and a legal diagonal's 2x2 block
        // is convex too — the straight line to its centre stays inside.
        let aim = (
            (x + dx) as f32 + 0.5 - position.0,
            (y + dy) as f32 + 0.5 - position.1,
        );
        let length = (aim.0 * aim.0 + aim.1 * aim.1).sqrt();
        Some((aim.0 / length, aim.1 / length))
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

    let distance = walkable_distances(grid, prototypes, spawn);

    let mut exit = spawn;
    let mut best = 0u32;
    for &(cx, cy) in region {
        let d = distance[width_height_index(grid.width, cx, cy)].unwrap_or(0);
        if d > best {
            best = d;
            exit = (cx, cy);
        }
    }
    (spawn, exit)
}

/// Row-major vector index of a cell.
fn width_height_index(width: u32, x: u32, y: u32) -> usize {
    (y * width + x) as usize
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

    #[test]
    fn sight_passes_through_open_cells_and_blocks_at_walls() {
        let (prototypes, wall, floor) = test_setup();
        let mut grid = Grid::new(5, 5);
        for y in 0..5 {
            for x in 0..5 {
                grid.set(x, y, floor as u32);
            }
        }
        let centre = |x: u32, y: u32| (x as f32 + 0.5, y as f32 + 0.5);

        // Open adjacent and same-tile sight lines.
        assert!(line_of_sight(
            &grid,
            &prototypes,
            centre(1, 1),
            centre(2, 1)
        ));
        assert!(line_of_sight(
            &grid,
            &prototypes,
            centre(1, 1),
            centre(1, 1)
        ));
        assert!(line_of_sight(
            &grid,
            &prototypes,
            centre(1, 1),
            centre(3, 3)
        ));

        // A wall directly between blocks...
        grid.set(2, 1, wall as u32);
        assert!(!line_of_sight(
            &grid,
            &prototypes,
            centre(1, 1),
            centre(3, 1)
        ));
        // A centre-to-centre diagonal that clips the wall tile's corner is
        // blocked too (conservative corner rule).
        assert!(!line_of_sight(
            &grid,
            &prototypes,
            centre(1, 1),
            centre(2, 2)
        ));
        // Walkable cells beside the wall stay sighted.
        assert!(line_of_sight(
            &grid,
            &prototypes,
            centre(1, 1),
            centre(1, 2)
        ));
        assert!(line_of_sight(
            &grid,
            &prototypes,
            centre(1, 2),
            centre(2, 2)
        ));
        // The blocked cell's own centre: endpoints never block.
        assert!(line_of_sight(
            &grid,
            &prototypes,
            centre(2, 1),
            centre(2, 1)
        ));
    }

    /// A 7x5 room with a sealed border and a wall column down x=3 that
    /// leaves its gap at the top row (y=1): the only route between the left
    /// and right halves climbs through that gap.
    fn split_room() -> (Grid, Vec<WeightedPrototype>) {
        let (prototypes, wall, floor) = test_setup();
        let mut grid = Grid::new(7, 5);
        for y in 0..5 {
            for x in 0..7 {
                grid.set(x, y, floor as u32);
            }
        }
        for xy in 0..7 {
            grid.set(xy, 0, wall as u32);
            grid.set(xy, 4, wall as u32);
        }
        for y in 0..5 {
            grid.set(0, y, wall as u32);
            grid.set(6, y, wall as u32);
        }
        for y in 2..4 {
            grid.set(3, y, wall as u32);
        }
        (grid, prototypes)
    }

    #[test]
    fn flow_routes_around_a_wall_through_the_gap() {
        let (grid, prototypes) = split_room();
        let centre = |x: u32, y: u32| (x as f32 + 0.5, y as f32 + 0.5);
        let field = FlowField::build(&grid, &prototypes, (5, 3));

        // Bottom-left of the split: the descent climbs up-right toward the
        // gap — a diagonal step through an open corner.
        let (dx, dy) = field.direction(centre(1, 3)).expect("reachable cell flows");
        assert!(
            dx > 0.7 && dy < -0.7,
            "routed diagonally up-right: ({dx}, {dy})"
        );
        // Mid-corridor above the wall: straight through the gap.
        let (dx, dy) = field.direction(centre(3, 1)).expect("the gap cell flows");
        assert!(
            dx > 0.7 && dy.abs() < 0.3,
            "flows right along the gap: ({dx}, {dy})"
        );
        // Fractional positions aim at the same waypoint — the next cell's
        // centre — from wherever they sit in their cell.
        let (dx, dy) = field.direction((1.9, 3.9)).expect("reachable cell flows");
        let cross = dx * (2.5 - 3.9) - dy * (2.5 - 1.9);
        assert!(cross.abs() < 1e-4, "aimed at the waypoint: ({dx}, {dy})");
        // The target itself has nothing left to flow to.
        assert_eq!(field.direction(centre(5, 3)), None);
        // Walls never flow.
        assert_eq!(field.direction(centre(3, 2)), None);
    }

    #[test]
    fn flow_aims_at_the_waypoint_from_anywhere_in_the_cell() {
        // Regression: an agent hugging the corner of its cell used to keep
        // the cell's fixed direction and dead-lift into the wall corner
        // vertex ahead — a wedge it could not slide out of. The steering now
        // aims at the next cell's centre, bending away from the corner.
        let (grid, prototypes) = split_room();
        let field = FlowField::build(&grid, &prototypes, (5, 3));
        let (dx, dy) = field.direction((1.95, 3.05)).expect("reachable cell flows");
        let cross = dx * (2.5 - 3.05) - dy * (2.5 - 1.95);
        assert!(cross.abs() < 1e-4, "aimed at the waypoint: ({dx}, {dy})");
    }

    #[test]
    fn flow_descends_diagonally_in_the_open() {
        let (prototypes, _wall, floor) = test_setup();
        let mut grid = Grid::new(5, 5);
        for y in 0..5 {
            for x in 0..5 {
                grid.set(x, y, floor as u32);
            }
        }
        let field = FlowField::build(&grid, &prototypes, (2, 2));
        // From the top-left of an open room the fastest descent is the
        // diagonal step straight at the target.
        let (dx, dy) = field.direction((1.5, 1.5)).expect("open room flows");
        assert!((dx - core::f32::consts::FRAC_1_SQRT_2).abs() < 1e-5);
        assert!((dy - core::f32::consts::FRAC_1_SQRT_2).abs() < 1e-5);
    }

    #[test]
    fn flow_refuses_diagonal_descent_between_walls() {
        let (prototypes, wall, floor) = test_setup();
        let mut grid = Grid::new(7, 5);
        for y in 0..5 {
            for x in 0..7 {
                grid.set(x, y, floor as u32);
            }
        }
        // Seal the border, then wall the two orthogonal neighbours of the
        // target (3,3): the diagonal from (2,2) to the target squeezes
        // between those walls and must not be offered — the descent has to
        // take the long way round through (2,1).
        for xy in 0..7 {
            grid.set(xy, 0, wall as u32);
            grid.set(xy, 4, wall as u32);
        }
        for y in 0..5 {
            grid.set(0, y, wall as u32);
            grid.set(6, y, wall as u32);
        }
        grid.set(3, 2, wall as u32);
        grid.set(2, 3, wall as u32);
        let field = FlowField::build(&grid, &prototypes, (3, 3));
        let (dx, dy) = field
            .direction((2.5, 2.5))
            .expect("the cell still connects around");
        // The only descent is the orthogonal step up through (2,1).
        assert!(
            dx.abs() < 0.3 && dy < -0.7,
            "stepped around, not through: ({dx}, {dy})"
        );
    }

    #[test]
    fn flow_gives_nothing_for_unreachable_cells() {
        let (prototypes, wall, floor) = test_setup();
        let mut grid = Grid::new(7, 5);
        for y in 0..5 {
            for x in 0..7 {
                grid.set(x, y, floor as u32);
            }
        }
        // A full wall column: the left half is unreachable from the right.
        for y in 0..5 {
            grid.set(3, y, wall as u32);
        }
        let field = FlowField::build(&grid, &prototypes, (5, 2));
        assert_eq!(field.direction((1.5, 2.5)), None);
        // Off-grid is None too.
        assert_eq!(field.direction((-1.0, 2.5)), None);
        assert_eq!(field.direction((7.5, 2.5)), None);
    }

    #[test]
    fn diagonal_sight_that_clips_a_wall_corner_blocks() {
        let (prototypes, wall, floor) = test_setup();
        let mut grid = Grid::new(5, 5);
        for y in 0..5 {
            for x in 0..5 {
                grid.set(x, y, floor as u32);
            }
        }
        let centre = |x: u32, y: u32| (x as f32 + 0.5, y as f32 + 0.5);

        // Diagonal across an open corner passes.
        assert!(line_of_sight(
            &grid,
            &prototypes,
            centre(0, 0),
            centre(1, 1)
        ));
        // Put walls on BOTH orthogonal neighbours of the corner: the
        // diagonal must pass through the wall corner cell itself.
        grid.set(1, 0, wall as u32);
        grid.set(0, 1, wall as u32);
        assert!(!line_of_sight(
            &grid,
            &prototypes,
            centre(0, 0),
            centre(1, 1)
        ));
    }

    #[test]
    fn sight_leaves_the_grid_blocked() {
        let (prototypes, _wall, floor) = test_setup();
        let mut grid = Grid::new(4, 4);
        for y in 0..4 {
            for x in 0..4 {
                grid.set(x, y, floor as u32);
            }
        }
        // A segment running off the edge and back is obstructed.
        assert!(!line_of_sight(&grid, &prototypes, (2.5, 2.5), (-1.5, 2.5)));
    }

    #[test]
    fn walkable_distances_measures_from_the_origin_cell() {
        let (prototypes, wall, floor) = test_setup();
        let mut grid = Grid::new(5, 5);
        for y in 0..5 {
            for x in 0..5 {
                grid.set(x, y, floor as u32);
            }
        }
        // Sealed border plus a wall column down the middle splits the space.
        for xy in 0..5 {
            grid.set(xy, 0, wall as u32);
            grid.set(xy, 4, wall as u32);
            grid.set(0, xy, wall as u32);
            grid.set(4, xy, wall as u32);
        }
        for y in 1..4 {
            grid.set(2, y, wall as u32);
        }

        let distances = walkable_distances(&grid, &prototypes, (1, 1));
        let at = |x: u32, y: u32| distances[(y * 5 + x) as usize];
        // Origin is zero, same-side cells are reachable, the far side is not.
        assert_eq!(at(1, 1), Some(0));
        assert_eq!(at(1, 2), Some(1));
        assert_eq!(at(1, 3), Some(2));
        assert_eq!(at(3, 3), None);

        // Distances measure from the origin even when it is a wall cell —
        // the flood simply walks out of it.
        let from_wall = walkable_distances(&grid, &prototypes, (2, 1));
        let from = |x: u32, y: u32| from_wall[(y * 5 + x) as usize];
        assert_eq!(from(2, 1), Some(0));
        assert_eq!(from(1, 1), Some(1));
        assert_eq!(from(3, 3), Some(3));
    }
}
