//! The locked vault: one sealed room per depth sitting just off the main
//! path, holding a small mob and a guaranteed heal. Its door only answers
//! to the transmitter — the unlock is the key, which is why it is cheap and
//! fields no drones of its own (the fleet skill adds those).
//!
//! Placement claims a natural island of the generated map when one
//! qualifies (big enough, sealable by a single thin wall), otherwise carves
//! a sealed room into the walls beside the main path. Everything here is
//! deterministic per (seed, depth), and nothing ever fails hard: a depth
//! that cannot host a vault simply runs without one.

use std::collections::HashSet;

use rand::RngExt;
use rand::SeedableRng;
use rand::rngs::SmallRng;
use wfc::{GeneratedMap, Grid, TileClass, WeightedPrototype, walkable_regions};

/// Salt keeping vault placement uncorrelated with the other seeded passes.
const VAULT_SALT: u64 = 0x1A17_5A1E;

/// Smallest island that can host a vault fight (the carved rooms match it —
/// see the room geometry below).
pub const VAULT_MIN_CELLS: usize = 12;

/// The vault mob starts at this size.
const VAULT_MOB_BASE: usize = 3;
/// Depth per extra vault enemy.
const VAULT_MOB_PER_DEPTHS: u32 = 3;
/// The vault mob never outgrows this.
const VAULT_MOB_CAP: usize = 5;

/// Mob size waiting inside the vault at `depth`.
pub fn vault_mob_for(depth: u32) -> usize {
    (VAULT_MOB_BASE + (depth / VAULT_MOB_PER_DEPTHS) as usize).min(VAULT_MOB_CAP)
}

/// The vault of the current depth.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Vault {
    /// The door cell — a wall in the grid while closed, thin between the
    /// main region and the vault interior.
    pub door: (u32, u32),
    /// The vault's walkable interior (the claimed island or the carved
    /// room's floor). The mob and the heal live here.
    pub interior: Vec<(u32, u32)>,
    /// The door connects east-west (its walkable neighbours sit east and
    /// west of it); false when it connects north-south.
    pub east_west: bool,
}

impl Vault {
    /// Place this depth's vault, mutating `map` when a room must be carved.
    /// Deterministic per (seed, depth). Returns `None` when no vault could
    /// be placed — the depth runs vault-less rather than failing.
    pub fn build(
        map: &mut GeneratedMap,
        prototypes: &[WeightedPrototype],
        seed: u64,
        depth: u32,
    ) -> Option<Self> {
        let mut rng = SmallRng::seed_from_u64(
            seed ^ VAULT_SALT ^ (depth as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15),
        );
        let regions = walkable_regions(&map.grid, prototypes);
        let main_index = regions
            .iter()
            .position(|region| region.contains(&map.spawn))?;
        let main = &regions[main_index];

        // Islands first: the largest one that qualifies wins (regions are
        // sorted largest first, so this is also the best-scoring).
        for (index, island) in regions.iter().enumerate() {
            if index == main_index || island.len() < VAULT_MIN_CELLS {
                continue;
            }
            if let Some((door, east_west)) =
                door_site(island, main, &map.grid, prototypes, &mut rng)
            {
                return Some(Self {
                    door,
                    interior: island.clone(),
                    east_west,
                });
            }
        }

        carve_vault(map, prototypes, main, &mut rng)
    }

    /// Every wall cell sealing the vault from outside — the door plus all
    /// of the interior's wall neighbours. Cracked-wall marking skips these
    /// so grenades and dashes can never open a second way in and bypass
    /// the transmitter gate.
    pub fn sealed_walls(&self, grid: &Grid, prototypes: &[WeightedPrototype]) -> Vec<(u32, u32)> {
        let (width, height) = (grid.width(), grid.height());
        let class_of = |x: u32, y: u32| {
            prototypes[grid.get(x, y).expect("fully collapsed") as usize]
                .prototype
                .class
        };
        let mut sealed = vec![self.door];
        for &(x, y) in &self.interior {
            for (dx, dy) in [(1i32, 0), (-1, 0), (0, 1), (0, -1)] {
                let nx = x as i32 + dx;
                let ny = y as i32 + dy;
                if nx < 0 || ny < 0 || nx >= width as i32 || ny >= height as i32 {
                    continue;
                }
                let (nx, ny) = (nx as u32, ny as u32);
                if class_of(nx, ny) == TileClass::Wall {
                    sealed.push((nx, ny));
                }
            }
        }
        sealed.sort();
        sealed.dedup();
        sealed
    }
}

/// A door site between `island` and `main`: an interior wall cell with the
/// island on one side and the main region exactly on the other. Picked at
/// random from the candidates (deterministic via `rng`).
fn door_site(
    island: &[(u32, u32)],
    main: &[(u32, u32)],
    grid: &Grid,
    prototypes: &[WeightedPrototype],
    rng: &mut SmallRng,
) -> Option<((u32, u32), bool)> {
    let (width, height) = (grid.width(), grid.height());
    let class_of = |x: u32, y: u32| {
        prototypes[grid.get(x, y).expect("fully collapsed") as usize]
            .prototype
            .class
    };
    let main_set: HashSet<(u32, u32)> = main.iter().copied().collect();
    let mut candidates = Vec::new();
    for &(x, y) in island {
        for (dx, dy) in [(1i32, 0), (-1, 0), (0, 1), (0, -1)] {
            let (wx, wy) = (x as i32 + dx, y as i32 + dy);
            let (fx, fy) = (x as i32 + 2 * dx, y as i32 + 2 * dy);
            // The door wall must be interior — the sealed border never
            // opens — and the far side must be the main region.
            if wx < 1 || wy < 1 || wx >= width as i32 - 1 || wy >= height as i32 - 1 {
                continue;
            }
            if fx < 0 || fy < 0 || fx >= width as i32 || fy >= height as i32 {
                continue;
            }
            if class_of(wx as u32, wy as u32) != TileClass::Wall {
                continue;
            }
            if main_set.contains(&(fx as u32, fy as u32)) {
                candidates.push(((wx as u32, wy as u32), dx != 0));
            }
        }
    }
    if candidates.is_empty() {
        return None;
    }
    let (door, east_west) = candidates.swap_remove(rng.random_range(0..candidates.len()));
    Some((door, east_west))
}

/// The carved room: a 5×3 walkable interior inside a 7×5 wall ring (the
/// interior covers the same ground as a minimal qualifying island).
const ROOM_INTERIOR_WIDTH: i32 = 5;
const ROOM_INTERIOR_HEIGHT: i32 = 3;

/// Carve a sealed room beside the main path: find a seeded door candidate
/// (an interior wall cell adjacent to the main region), lay the room out in
/// the direction away from the main region, and overwrite the footprint —
/// walls for the ring, floor for the interior. The footprint never touches
/// the main region, the spawn or the exit, so the only way in is the door.
/// Returns `None` when no candidate leaves room for the footprint.
fn carve_vault(
    map: &mut GeneratedMap,
    prototypes: &[WeightedPrototype],
    main: &[(u32, u32)],
    rng: &mut SmallRng,
) -> Option<Vault> {
    let (width, height) = (map.grid.width(), map.grid.height());
    let main_set: HashSet<(u32, u32)> = main.iter().copied().collect();
    let class_of = |x: u32, y: u32| {
        prototypes[map.grid.get(x, y).expect("fully collapsed") as usize]
            .prototype
            .class
    };

    // Door candidates: interior wall cells with a main-region neighbour.
    // Seeded-shuffled so the pick is spread but deterministic.
    let mut candidates: Vec<(u32, u32)> = (1..width - 1)
        .flat_map(|x| (1..height - 1).map(move |y| (x, y)))
        .filter(|&(x, y)| {
            class_of(x, y) == TileClass::Wall
                && [(1i32, 0), (-1, 0), (0, 1), (0, -1)].iter().any(|&(dx, dy)| {
                    let (nx, ny) = (x as i32 + dx, y as i32 + dy);
                    nx >= 0
                        && ny >= 0
                        && nx < width as i32
                        && ny < height as i32
                        && main_set.contains(&(nx as u32, ny as u32))
                })
        })
        .collect();
    for i in (1..candidates.len()).rev() {
        candidates.swap(i, rng.random_range(0..=i));
    }

    for door in candidates {
        // Lay the room out away from the main-region side: the main-side
        // neighbour points at the main region, so the room direction is its
        // opposite (the interior's near edge centres just past the door,
        // and the ring wraps the interior — the door is the ring's near
        // centre).
        let Some(t) = [(1i32, 0), (-1, 0), (0, 1), (0, -1)]
            .iter()
            .filter(|&&(dx, dy)| {
                let (nx, ny) = (door.0 as i32 + dx, door.1 as i32 + dy);
                nx >= 0
                    && ny >= 0
                    && nx < width as i32
                    && ny < height as i32
                    && main_set.contains(&(nx as u32, ny as u32))
            })
            .map(|&(dx, dy)| (-dx, -dy))
            .next()
        else {
            continue;
        };

        let Some(layout) = room_layout(door, t, width, height, &main_set) else {
            continue;
        };
        let wall_index = prototypes
            .iter()
            .position(|weighted| weighted.prototype.class == TileClass::Wall)
            .expect("prototype set must contain a wall tile") as u32;
        for &(x, y) in &layout.ring {
            map.grid.set(x, y, wall_index);
        }
        // The interior floors re-derive their variant from the finished
        // ring (the mask is the prototype index, matching how the
        // destruction pass settles holes).
        for &(x, y) in &layout.interior {
            let mask = floor_facing_mask(&map.grid, prototypes, (width, height), (x, y)) as u32;
            map.grid.set(x, y, mask);
        }
        return Some(Vault {
            door,
            interior: layout.interior,
            east_west: t.0 != 0,
        });
    }
    None
}

/// The carved room's geometry: the walkable interior and its wall ring.
#[derive(Debug)]
struct RoomLayout {
    interior: Vec<(u32, u32)>,
    ring: Vec<(u32, u32)>,
}

/// Lay out the room for a door and room direction `t`: the interior spans
/// [`ROOM_INTERIOR_WIDTH`] across and [`ROOM_INTERIOR_HEIGHT`] deep starting
/// just past the door. `None` when the footprint leaves the map's interior
/// or would overwrite main-region floor.
fn room_layout(
    door: (u32, u32),
    t: (i32, i32),
    width: u32,
    height: u32,
    main_set: &HashSet<(u32, u32)>,
) -> Option<RoomLayout> {
    // Across = perpendicular to the room direction.
    let across = (-t.1, t.0);
    let mut interior = Vec::new();
    for depth in 1..=ROOM_INTERIOR_HEIGHT {
        for offset in -(ROOM_INTERIOR_WIDTH) / 2..=(ROOM_INTERIOR_WIDTH) / 2 {
            let x = door.0 as i32 + t.0 * depth + across.0 * offset;
            let y = door.1 as i32 + t.1 * depth + across.1 * offset;
            if x < 1 || y < 1 || x >= width as i32 - 1 || y >= height as i32 - 1 {
                return None;
            }
            if main_set.contains(&(x as u32, y as u32)) {
                return None;
            }
            interior.push((x as u32, y as u32));
        }
    }
    // The ring: every cell adjacent (8-way) to the interior that is not
    // interior itself — including the door cell on the near edge.
    let mut ring = Vec::new();
    for &(x, y) in &interior {
        for dx in -1i32..=1 {
            for dy in -1i32..=1 {
                let nx = x as i32 + dx;
                let ny = y as i32 + dy;
                if nx < 1 || ny < 1 || nx >= width as i32 - 1 || ny >= height as i32 - 1 {
                    return None;
                }
                let cell = (nx as u32, ny as u32);
                if !interior.contains(&cell) && !ring.contains(&cell) && !main_set.contains(&cell)
                {
                    ring.push(cell);
                }
            }
        }
    }
    // The door cell is the ring's near-edge centre by construction and
    // stays a wall — a belt-and-braces guard for the layout maths.
    if !ring.contains(&door) {
        return None;
    }
    Some(RoomLayout { interior, ring })
}

/// Bit set where a wall tile's face touches walkable space (N=1, E=2, S=4,
/// W=8 in grid space) — the floor variant mask. Kept in sync with
/// `world::floor_facing_mask` (which owns the rendering masks); duplicated
/// here because vault placement runs before the world materialises.
fn floor_facing_mask(
    grid: &Grid,
    prototypes: &[WeightedPrototype],
    size: (u32, u32),
    tile: (u32, u32),
) -> u8 {
    let (width, height) = size;
    let (x, y) = tile;
    let walkable = |nx: u32, ny: u32| {
        prototypes[grid.get(nx, ny).expect("fully collapsed") as usize]
            .prototype
            .class
            != TileClass::Wall
    };
    let mut mask = 0u8;
    if y + 1 < height && walkable(x, y + 1) {
        mask |= 1;
    }
    if x + 1 < width && walkable(x + 1, y) {
        mask |= 2;
    }
    if y > 0 && walkable(x, y - 1) {
        mask |= 4;
    }
    if x > 0 && walkable(x - 1, y) {
        mask |= 8;
    }
    mask
}

#[cfg(test)]
mod tests {
    use super::*;
    use wfc::prototype_set;

    /// An all-floor grid sealed by a wall border, plus the prototype set.
    fn floor_grid(width: u32, height: u32) -> (GeneratedMap, Vec<WeightedPrototype>) {
        let prototypes = prototype_set(0.25, 0.02);
        let floor = prototypes
            .iter()
            .position(|p| p.prototype.class.walkable())
            .expect("the set has walkable tiles") as u32;
        let wall = prototypes
            .iter()
            .position(|p| !p.prototype.class.walkable())
            .expect("the set has a wall") as u32;
        let mut grid = Grid::new(width, height);
        for y in 0..height {
            for x in 0..width {
                grid.set(x, y, if x == 0 || y == 0 || x + 1 == width || y + 1 == height {
                    wall
                } else {
                    floor
                });
            }
        }
        let map = GeneratedMap {
            grid,
            spawn: (1, 1),
            exit: (width - 2, height - 2),
        };
        (map, prototypes)
    }

    /// Seal a full wall column down `x` (splitting the interior in two).
    fn wall_column(map: &mut GeneratedMap, prototypes: &[WeightedPrototype], x: u32) {
        let wall = prototypes
            .iter()
            .position(|p| !p.prototype.class.walkable())
            .expect("the set has a wall") as u32;
        let height = map.grid.height();
        for y in 0..height {
            map.grid.set(x, y, wall);
        }
    }

    #[test]
    fn a_qualifying_island_is_claimed_with_a_thin_door() {
        let (mut map, prototypes) = floor_grid(13, 9);
        wall_column(&mut map, &prototypes, 6);
        // Spawn in the left half; the right half is a 35-cell island.
        map.spawn = (1, 1);

        let vault = Vault::build(&mut map, &prototypes, 7, 0).expect("the island qualifies");
        assert_eq!(vault.door.0, 6, "the door sits in the splitting wall");
        assert!(vault.east_west, "the door connects east-west");
        assert_eq!(vault.interior.len(), 35, "the island is the interior");
        assert!(!vault.interior.contains(&vault.door));
        // Claiming an island leaves the grid untouched: the door is still
        // a wall.
        assert_eq!(
            map.grid.get(vault.door.0, vault.door.1).unwrap(),
            map.grid.get(0, 0).unwrap(), // the border is the same wall tile
        );
    }

    #[test]
    fn small_islands_are_never_claimed() {
        let (mut map, prototypes) = floor_grid(13, 9);
        // Two wall columns with the pocket between them filled except for a
        // 4-cell gap — far below the vault minimum.
        wall_column(&mut map, &prototypes, 6);
        wall_column(&mut map, &prototypes, 9);
        let wall = prototypes
            .iter()
            .position(|p| !p.prototype.class.walkable())
            .expect("the set has a wall") as u32;
        for y in 1..8 {
            for x in 7..9 {
                if !(4..=5).contains(&y) {
                    map.grid.set(x, y, wall);
                }
            }
        }
        map.spawn = (1, 1);

        // The pocket never becomes a vault; if the carve fallback finds a
        // room elsewhere, it is a full-size one.
        let vault = Vault::build(&mut map, &prototypes, 7, 0);
        assert!(
            vault
                .iter()
                .all(|v| v.interior.len() >= VAULT_MIN_CELLS && !v.interior.contains(&v.door))
        );
    }

    #[test]
    fn carving_punches_a_sealed_room_into_the_walls() {
        let (mut map, prototypes) = floor_grid(15, 11);
        // A solid 7x5 wall block: the only place a room fits is a 7x5
        // footprint inside it, door on the top row.
        let wall = prototypes
            .iter()
            .position(|p| !p.prototype.class.walkable())
            .expect("the set has a wall") as u32;
        for y in 3..8 {
            for x in 7..14 {
                map.grid.set(x, y, wall);
            }
        }
        map.spawn = (1, 1);

        let vault = Vault::build(&mut map, &prototypes, 7, 0).expect("the block hosts a room");
        // Two door sites fit — a room may grow south off the top row or
        // north off the bottom row; either way the interior is the middle.
        assert!(
            matches!(vault.door, (10, 3) | (10, 7)),
            "the only fitting door column: {:?}",
            vault.door
        );
        assert!(!vault.east_west, "the room lies above or below the door");
        assert_eq!(vault.interior.len(), 15);
        // Every interior cell is now walkable and every ring cell a wall.
        let class_of = |cell: (u32, u32)| {
            prototypes[map.grid.get(cell.0, cell.1).unwrap() as usize]
                .prototype
                .class
        };
        for &cell in &vault.interior {
            assert!(class_of(cell).walkable(), "{cell:?} must be floor");
        }
        for &cell in &vault.sealed_walls(&map.grid, &prototypes) {
            assert_eq!(class_of(cell), wfc::TileClass::Wall);
        }
        // The interior never leaked into the main region.
        for &cell in &vault.interior {
            assert_ne!(cell, map.spawn);
        }
    }

    #[test]
    fn placement_is_deterministic_per_seed_and_depth() {
        let (mut first, prototypes) = floor_grid(15, 11);
        let wall = prototypes
            .iter()
            .position(|p| !p.prototype.class.walkable())
            .expect("the set has a wall") as u32;
        for y in 3..8 {
            for x in 7..14 {
                first.grid.set(x, y, wall);
            }
        }
        first.spawn = (1, 1);
        let mut second = first.clone();

        let a = Vault::build(&mut first, &prototypes, 11, 2).expect("vault placed");
        let b = Vault::build(&mut second, &prototypes, 11, 2).expect("vault placed");
        assert_eq!(a, b);
        // A different seed may land elsewhere; it must still be a vault.
        let (mut third, prototypes) = {
            let (mut map, prototypes) = floor_grid(15, 11);
            let wall = prototypes
                .iter()
                .position(|p| !p.prototype.class.walkable())
                .expect("the set has a wall") as u32;
            for y in 3..8 {
                for x in 7..14 {
                    map.grid.set(x, y, wall);
                }
            }
            map.spawn = (1, 1);
            (map, prototypes)
        };
        let _ = Vault::build(&mut third, &prototypes, 12, 2);
    }

    #[test]
    fn no_vault_without_space() {
        // A small all-floor map: no islands, no walls to carve into.
        let (mut map, prototypes) = floor_grid(9, 9);
        assert_eq!(Vault::build(&mut map, &prototypes, 3, 0), None);
    }

    #[test]
    fn sealed_walls_cover_the_whole_ring() {
        let (mut map, prototypes) = floor_grid(13, 9);
        wall_column(&mut map, &prototypes, 6);
        map.spawn = (1, 1);
        let vault = Vault::build(&mut map, &prototypes, 7, 0).expect("the island qualifies");

        let sealed = vault.sealed_walls(&map.grid, &prototypes);
        // The full splitting column x=6 (border rows excluded — the border
        // cells never neighbour the interior).
        for y in 1..8 {
            assert!(sealed.contains(&(6, y)), "the seal covers (6, {y})");
        }
        assert_eq!(sealed.first(), Some(&(6, 1)));
    }

    #[test]
    fn the_vault_mob_grows_with_depth_and_caps() {
        assert_eq!(vault_mob_for(0), 3);
        assert_eq!(vault_mob_for(3), 4);
        assert_eq!(vault_mob_for(9), 5);
        assert_eq!(vault_mob_for(50), 5);
    }
}
