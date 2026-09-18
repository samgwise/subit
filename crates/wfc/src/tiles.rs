//! Tile prototypes: the concrete tile set the generator collapses onto, plus
//! the adjacency predicate over them.

use crate::socket::{Direction, Socket};

/// Gameplay class of a tile; drives reachability now, and later collider
/// spawning and audio triggers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TileClass {
    Floor,
    Wall,
    Terminal,
}

impl TileClass {
    /// Whether the player can traverse tiles of this class.
    pub fn walkable(self) -> bool {
        !matches!(self, TileClass::Wall)
    }
}

/// A tile prototype with a socket on each edge (indexed by `Direction`,
/// north/east/south/west — declaration order) plus its gameplay class.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TilePrototype {
    pub sockets: [Socket; 4],
    pub class: TileClass,
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

/// A prototype and its relative selection weight.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WeightedPrototype {
    pub prototype: TilePrototype,
    pub weight: f32,
}

/// Build the generator's prototype set: 16 floor variants (one per 4-bit
/// edge mask), one wall tile, one terminal tile.
///
/// `wall_share` and `terminal_share` are the expected fractions of the map
/// those classes should occupy; they must be non-negative and sum below 1.
/// The floor variants take the remainder, weighted by open-edge count so
/// open arenas arise more readily than hairpin corridors.
pub fn prototype_set(wall_share: f32, terminal_share: f32) -> Vec<WeightedPrototype> {
    assert!(
        wall_share >= 0.0 && terminal_share >= 0.0 && wall_share + terminal_share < 1.0,
        "wall and terminal shares must be non-negative and sum below 1"
    );
    // Floor weights stay in their native units: 1 + open-edge count over all
    // 16 masks sums to 48. Wall and terminal weights are scaled so their
    // requested shares come out of the weighted pick in expectation.
    let floor_weight_total: f32 = (0u8..16).map(|mask| 1.0 + mask.count_ones() as f32).sum();
    let scale = floor_weight_total / (1.0 - wall_share - terminal_share);

    let mut set = Vec::with_capacity(18);
    for mask in 0u8..16 {
        set.push(WeightedPrototype {
            prototype: TilePrototype {
                sockets: [
                    masked_edge(mask, 0),
                    masked_edge(mask, 1),
                    masked_edge(mask, 2),
                    masked_edge(mask, 3),
                ],
                class: TileClass::Floor,
            },
            weight: 1.0 + mask.count_ones() as f32,
        });
    }
    set.push(WeightedPrototype {
        prototype: TilePrototype {
            sockets: [Socket::Wall; 4],
            class: TileClass::Wall,
        },
        weight: wall_share * scale,
    });
    set.push(WeightedPrototype {
        prototype: TilePrototype {
            sockets: [Socket::Terminal; 4],
            class: TileClass::Terminal,
        },
        weight: terminal_share * scale,
    });
    set
}

/// `Path` where the mask bit (north/east/south/west order) is set, `Stop`
/// elsewhere.
fn masked_edge(mask: u8, edge: usize) -> Socket {
    if mask & (1 << edge) != 0 {
        Socket::Path
    } else {
        Socket::Stop
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prototype_set_has_all_floor_masks_plus_wall_and_terminal() {
        let set = prototype_set(0.4, 0.02);
        assert_eq!(set.len(), 18);
        assert_eq!(
            set.iter()
                .filter(|p| p.prototype.class == TileClass::Floor)
                .count(),
            16
        );
        assert_eq!(
            set.iter()
                .filter(|p| p.prototype.class == TileClass::Wall)
                .count(),
            1
        );
        assert_eq!(
            set.iter()
                .filter(|p| p.prototype.class == TileClass::Terminal)
                .count(),
            1
        );
    }

    #[test]
    fn floor_sockets_match_their_edge_mask() {
        let set = prototype_set(0.4, 0.02);
        for (index, weighted) in set.iter().enumerate().take(16) {
            for (edge, direction) in Direction::ALL.iter().enumerate() {
                let expected = if index & (1 << edge) != 0 {
                    Socket::Path
                } else {
                    Socket::Stop
                };
                assert_eq!(
                    weighted.prototype.socket(*direction),
                    expected,
                    "floor mask {index} edge {edge}"
                );
            }
        }
    }

    #[test]
    fn weights_realise_the_requested_shares() {
        let set = prototype_set(0.4, 0.02);
        let total: f32 = set.iter().map(|p| p.weight).sum();
        let wall: f32 = set
            .iter()
            .filter(|p| p.prototype.class == TileClass::Wall)
            .map(|p| p.weight)
            .sum();
        let terminal: f32 = set
            .iter()
            .filter(|p| p.prototype.class == TileClass::Terminal)
            .map(|p| p.weight)
            .sum();
        assert!((wall / total - 0.4).abs() < 1e-4);
        assert!((terminal / total - 0.02).abs() < 1e-4);
        assert!(set.iter().all(|p| p.weight > 0.0));
    }

    #[test]
    fn floors_meet_walls_through_stop_edges() {
        let set = prototype_set(0.4, 0.02);
        let wall = &set
            .iter()
            .find(|p| p.prototype.class == TileClass::Wall)
            .unwrap()
            .prototype;
        // Mask 0b1101: Stop on the east edge only, Path elsewhere.
        let floor_with_stop_east = &set[13].prototype;
        assert_eq!(floor_with_stop_east.socket(Direction::East), Socket::Stop);

        // The wall sits east of that floor: its west edge (Wall) faces the
        // floor's east edge (Stop).
        assert!(tiles_are_compatible(
            wall,
            floor_with_stop_east,
            Direction::East
        ));
        // A fully open floor cannot sit east of it: its west edge (Path)
        // would face the floor's Stop edge.
        let fully_open = &set[15].prototype;
        assert!(!tiles_are_compatible(
            fully_open,
            floor_with_stop_east,
            Direction::East
        ));
    }

    #[test]
    fn terminals_only_sit_amid_open_floor() {
        let set = prototype_set(0.4, 0.02);
        let terminal = &set
            .iter()
            .find(|p| p.prototype.class == TileClass::Terminal)
            .unwrap()
            .prototype;
        let fully_open = &set[15].prototype;
        let wall = &set
            .iter()
            .find(|p| p.prototype.class == TileClass::Wall)
            .unwrap()
            .prototype;
        assert!(tiles_are_compatible(terminal, fully_open, Direction::East));
        assert!(tiles_are_compatible(fully_open, terminal, Direction::East));
        assert!(!tiles_are_compatible(terminal, wall, Direction::East));
        assert!(!tiles_are_compatible(wall, terminal, Direction::East));
    }
}
