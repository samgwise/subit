//! Edge sockets and directions: how tiles connect across a shared edge.

/// The four cardinal directions, used to name a tile's edges.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Direction {
    North,
    East,
    South,
    West,
}

impl Direction {
    /// Every direction, in declaration order (matches `TilePrototype`'s
    /// socket array indices).
    pub const ALL: [Direction; 4] = [
        Direction::North,
        Direction::East,
        Direction::South,
        Direction::West,
    ];

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

    /// Unit step to the neighbouring cell in this direction (y grows
    /// southwards).
    pub fn delta(self) -> (i32, i32) {
        match self {
            Direction::North => (0, -1),
            Direction::East => (1, 0),
            Direction::South => (0, 1),
            Direction::West => (-1, 0),
        }
    }
}

/// Edge sockets describe how tiles connect across a shared edge — the GDD's
/// four basic tile socket types, matched by a complementary relation rather
/// than plain equality (see [`Socket::accepts`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Socket {
    /// Open floor continuing into the neighbour.
    Path,
    /// A floor edge that ends here (faces a wall).
    Stop,
    /// A wall edge.
    Wall,
    /// An interactive terminal node edge.
    Terminal,
}

impl Socket {
    /// Symmetric compatibility relation for two facing edge sockets:
    ///
    /// - `Path` accepts `Path` — open floor continues
    /// - `Path` accepts `Terminal` (and vice versa) — terminals sit on open ground
    /// - `Stop` accepts `Wall` (and vice versa) — a floor edge ending against a wall
    /// - `Wall` accepts `Wall` — wall meeting wall
    ///
    /// Everything else is rejected: two floors cannot meet through a `Stop`
    /// seam, and a wall can never face an open `Path` edge.
    pub fn accepts(self, other: Socket) -> bool {
        matches!(
            (self, other),
            (Socket::Path, Socket::Path)
                | (Socket::Path, Socket::Terminal)
                | (Socket::Terminal, Socket::Path)
                | (Socket::Terminal, Socket::Terminal)
                | (Socket::Stop, Socket::Wall)
                | (Socket::Wall, Socket::Stop)
                | (Socket::Wall, Socket::Wall)
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Full compatibility matrix, indexed `[self][other]` in `Socket`
    /// declaration order.
    fn accepts_matrix() -> [[bool; 4]; 4] {
        //                Path   Stop   Wall   Terminal
        [
            /* Path     */ [true, false, false, true],
            /* Stop     */ [false, false, true, false],
            /* Wall     */ [false, true, true, false],
            /* Terminal */ [true, false, false, true],
        ]
    }

    fn socket(index: usize) -> Socket {
        [Socket::Path, Socket::Stop, Socket::Wall, Socket::Terminal][index]
    }

    #[test]
    fn compatibility_follows_the_relation_matrix() {
        for (a, row) in accepts_matrix().iter().enumerate() {
            for (b, &expected) in row.iter().enumerate() {
                assert_eq!(
                    socket(a).accepts(socket(b)),
                    expected,
                    "unexpected relation between sockets {a} and {b}"
                );
            }
        }
    }

    #[test]
    fn compatibility_is_symmetric() {
        for a in 0..4 {
            for b in 0..4 {
                assert_eq!(
                    socket(a).accepts(socket(b)),
                    socket(b).accepts(socket(a)),
                    "relation must be symmetric between sockets {a} and {b}"
                );
            }
        }
    }

    #[test]
    fn opposite_directions_pair_up() {
        for direction in Direction::ALL {
            assert_eq!(direction.opposite().opposite(), direction);
        }
    }

    #[test]
    fn deltas_cancel_out_across_opposites() {
        for direction in Direction::ALL {
            let (ax, ay) = direction.delta();
            let (bx, by) = direction.opposite().delta();
            assert_eq!(ax + bx, 0);
            assert_eq!(ay + by, 0);
        }
    }
}
