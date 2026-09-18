//! Pure-Rust Wave Function Collapse (WFC) solver for the subit prototype.
//!
//! This crate is deliberately engine-free (no Bevy dependencies) so the
//! collapse can run off the ECS thread and be unit tested in isolation,
//! per the GDD pipeline (phase 1: abstract WFC execution).

/// Edge sockets describe how tiles connect to their neighbours. The GDD's
/// three tile classes each expose a distinct socket so adjacency rules can
/// guarantee sensible corridor layouts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Socket {
    /// High traversal speed, high enemy spawn capacity.
    OpenGround,
    /// Blocking physics, high visual light emitter density.
    WallConduit,
    /// Interactive objective points that trigger audio mode shifts.
    TerminalNode,
}

impl Socket {
    /// Socket compatibility is symmetrical: a socket only accepts a matching
    /// socket on the neighbouring edge.
    pub fn accepts(self, other: Socket) -> bool {
        self == other
    }
}

/// The four cardinal directions, used to name a tile's edges.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Direction {
    North,
    East,
    South,
    West,
}

impl Direction {
    /// The opposing direction, i.e. the edge on the neighbouring tile that
    /// touches this one.
    pub fn opposite(self) -> Direction {
        match self {
            Direction::North => Direction::South,
            Direction::East => Direction::West,
            Direction::South => Direction::North,
            Direction::West => Direction::East,
        }
    }
}

/// A tile prototype with a socket on each edge, indexed by `Direction`
/// (north, east, south, west — declaration order).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TilePrototype {
    pub sockets: [Socket; 4],
}

impl TilePrototype {
    /// Look up the socket on the given edge.
    pub fn socket(&self, direction: Direction) -> Socket {
        self.sockets[direction as usize]
    }
}

/// Check whether tile `a` may be placed in `direction` of tile `b`. The edges
/// that touch are `a`'s opposing socket and `b`'s socket on `direction`.
pub fn tiles_are_compatible(a: &TilePrototype, b: &TilePrototype, direction: Direction) -> bool {
    a.socket(direction.opposite()).accepts(b.socket(direction))
}

/// A collapsed grid of tile indices (`None` whilst a cell is still in
/// superposition).
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

/// Behaviour for a WFC solver: collapse a `width x height` region to indices
/// into `tiles`, honouring the adjacency constraints the socket model implies.
pub trait Solver {
    fn collapse(&self, width: u32, height: u32, tiles: &[TilePrototype]) -> Grid;
}

/// Placeholder solver used until Milestone 2 lands the real algorithm: it
/// fills every cell with the first tile prototype so downstream wiring has
/// something deterministic to consume.
pub struct TrivialSolver;

impl Solver for TrivialSolver {
    fn collapse(&self, width: u32, height: u32, tiles: &[TilePrototype]) -> Grid {
        let mut grid = Grid::new(width, height);
        if !tiles.is_empty() {
            for y in 0..height {
                for x in 0..width {
                    grid.set(x, y, 0);
                }
            }
        }
        grid
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tile(north: Socket, east: Socket, south: Socket, west: Socket) -> TilePrototype {
        TilePrototype {
            sockets: [north, east, south, west],
        }
    }

    #[test]
    fn sockets_only_accept_matching_sockets() {
        assert!(Socket::OpenGround.accepts(Socket::OpenGround));
        assert!(Socket::WallConduit.accepts(Socket::WallConduit));
        assert!(!Socket::OpenGround.accepts(Socket::WallConduit));
        assert!(!Socket::WallConduit.accepts(Socket::OpenGround));
    }

    #[test]
    fn opposite_directions_pair_up() {
        assert_eq!(Direction::North.opposite(), Direction::South);
        assert_eq!(Direction::East.opposite(), Direction::West);
        assert_eq!(Direction::South.opposite(), Direction::North);
        assert_eq!(Direction::West.opposite(), Direction::East);
    }

    #[test]
    fn clashing_edges_are_incompatible() {
        let open = tile(
            Socket::OpenGround,
            Socket::OpenGround,
            Socket::OpenGround,
            Socket::OpenGround,
        );
        let wall = tile(
            Socket::WallConduit,
            Socket::WallConduit,
            Socket::WallConduit,
            Socket::WallConduit,
        );
        assert!(!tiles_are_compatible(&open, &wall, Direction::East));
        assert!(tiles_are_compatible(&wall, &wall, Direction::East));

        // Only the touching edges matter: an open tile cannot sit east of a
        // tile whose eastern edge is a wall socket, even though both are
        // otherwise open.
        let open_with_wall_east = tile(
            Socket::OpenGround,
            Socket::WallConduit,
            Socket::OpenGround,
            Socket::OpenGround,
        );
        assert!(!tiles_are_compatible(
            &open,
            &open_with_wall_east,
            Direction::East
        ));
    }

    #[test]
    fn trivial_solver_fills_grid_with_first_tile() {
        let open = tile(
            Socket::OpenGround,
            Socket::OpenGround,
            Socket::OpenGround,
            Socket::OpenGround,
        );
        let tiles = vec![open, open];
        let grid = TrivialSolver.collapse(3, 2, &tiles);
        assert_eq!((grid.width(), grid.height()), (3, 2));
        for y in 0..grid.height() {
            for x in 0..grid.width() {
                assert_eq!(grid.get(x, y), Some(0));
            }
        }
    }

    #[test]
    fn trivial_solver_without_tiles_leaves_grid_uncollapsed() {
        let grid = TrivialSolver.collapse(2, 2, &[]);
        assert_eq!(grid.get(0, 0), None);
        assert_eq!(grid.get(1, 1), None);
    }
}
