//! Cross-seed invariants for the whole generation pipeline.

use std::collections::VecDeque;

use wfc::{
    Direction, GeneratedMap, GenerationError, GeneratorConfig, Grid, TileClass, WeightedPrototype,
    generate, prototype_set, tiles_are_compatible,
};

fn class_at(prototypes: &[WeightedPrototype], grid: &Grid, x: u32, y: u32) -> TileClass {
    prototypes[grid.get(x, y).expect("fully collapsed") as usize]
        .prototype
        .class
}

/// BFS from spawn across walkable tiles; true when the exit is reached.
fn exit_is_reachable(
    prototypes: &[WeightedPrototype],
    map: &GeneratedMap,
    config: &GeneratorConfig,
) -> bool {
    let width = config.width;
    let height = config.height;
    let mut visited = vec![false; (width * height) as usize];
    let mut queue = VecDeque::new();
    visited[(map.spawn.1 * width + map.spawn.0) as usize] = true;
    queue.push_back(map.spawn);
    while let Some((cx, cy)) = queue.pop_front() {
        if (cx, cy) == map.exit {
            return true;
        }
        for direction in Direction::ALL {
            let (dx, dy) = direction.delta();
            let nx = cx as i32 + dx;
            let ny = cy as i32 + dy;
            if nx < 0 || ny < 0 || nx >= width as i32 || ny >= height as i32 {
                continue;
            }
            let (nx, ny) = (nx as u32, ny as u32);
            let index = (ny * width + nx) as usize;
            if visited[index] {
                continue;
            }
            visited[index] = true;
            let tile = map.grid.get(nx, ny).expect("fully collapsed");
            if prototypes[tile as usize].prototype.class.walkable() {
                queue.push_back((nx, ny));
            }
        }
    }
    false
}

#[test]
fn generates_valid_maps_across_many_seeds() {
    for seed in 0..20u64 {
        let config = GeneratorConfig {
            width: 32,
            height: 32,
            seed,
            ..Default::default()
        };
        let map = generate(&config).expect("generation should succeed for typical seeds");
        let prototypes = prototype_set(config.wall_weight, config.terminal_weight);

        // Every cell collapsed.
        for y in 0..config.height {
            for x in 0..config.width {
                assert!(map.grid.get(x, y).is_some(), "cell ({x}, {y}) uncollapsed");
            }
        }

        // Sealed border of walls.
        for x in 0..config.width {
            assert_eq!(class_at(&prototypes, &map.grid, x, 0), TileClass::Wall);
            assert_eq!(
                class_at(&prototypes, &map.grid, x, config.height - 1),
                TileClass::Wall
            );
        }
        for y in 0..config.height {
            assert_eq!(class_at(&prototypes, &map.grid, 0, y), TileClass::Wall);
            assert_eq!(
                class_at(&prototypes, &map.grid, config.width - 1, y),
                TileClass::Wall
            );
        }

        // Every adjacency honours the socket relation.
        for y in 0..config.height {
            for x in 0..config.width {
                for direction in Direction::ALL {
                    let (dx, dy) = direction.delta();
                    let nx = x as i32 + dx;
                    let ny = y as i32 + dy;
                    if nx < 0 || ny < 0 || nx >= config.width as i32 || ny >= config.height as i32 {
                        continue;
                    }
                    let cell = &prototypes[map.grid.get(x, y).unwrap() as usize].prototype;
                    let neighbour =
                        &prototypes[map.grid.get(nx as u32, ny as u32).unwrap() as usize].prototype;
                    // The neighbour sits in `direction` of this cell.
                    assert!(
                        tiles_are_compatible(neighbour, cell, direction),
                        "invalid adjacency at ({x}, {y}) towards {direction:?}"
                    );
                }
            }
        }

        // Spawn and exit are walkable and connected.
        assert!(class_at(&prototypes, &map.grid, map.spawn.0, map.spawn.1).walkable());
        assert!(class_at(&prototypes, &map.grid, map.exit.0, map.exit.1).walkable());
        assert!(
            exit_is_reachable(&prototypes, &map, &config),
            "exit unreachable from spawn for seed {seed}"
        );
    }
}

#[test]
fn same_seed_produces_identical_maps() {
    let config = GeneratorConfig {
        width: 24,
        height: 24,
        seed: 42,
        ..Default::default()
    };
    let first = generate(&config).expect("first generation");
    let second = generate(&config).expect("second generation");
    assert_eq!(first.grid, second.grid);
    assert_eq!(first.spawn, second.spawn);
    assert_eq!(first.exit, second.exit);
}

#[test]
fn grids_below_five_cells_a_side_are_rejected() {
    let config = GeneratorConfig {
        width: 4,
        height: 10,
        ..Default::default()
    };
    assert_eq!(generate(&config), Err(GenerationError::GridTooSmall));
    let config = GeneratorConfig {
        width: 10,
        height: 2,
        ..Default::default()
    };
    assert_eq!(generate(&config), Err(GenerationError::GridTooSmall));
}
