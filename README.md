# subit

Prototype workspace for *Signal Breach* (see `initial-vision.md`): a top-down 2D
cyberpunk ARPG with Wave Function Collapse level generation and Ensemble-driven
generative audio.

- `docs/design.md` — the as-built design: mechanics, tuning values, and the
  audio event table. The living reference for future development.
- `docs/milestones.md` — what each milestone shipped.
- `initial-vision.md` — the founding vision, kept frozen as a record.

## Workspace

- `crates/game` — Bevy client: WFC-generated tilemap with physics, WASD player
  with HP and respawn, enemy swarm (chasers and throwers), mouse-aimed cleave,
  shield reflection, dash and grenade abilities, XP/level progression with a
  pause-and-spend skills menu, and the Ensemble bridge publishing combat
  events and telemetry to the hub.
- `crates/wfc` — pure-Rust Wave Function Collapse solver with grid
  reachability (BFS distances, line of sight), deliberately engine-free so it
  can run off the ECS thread and be tested in isolation.
- `crates/audio-voice` — generative audio voice as a library plus a thin
  standalone binary. The library (quantiser, event→note mapping, speed→CC)
  can later be woven directly into the game process; the binary provides the
  standalone audio daemon role from the GDD.

## Prerequisites

- Rust stable (built and tested with 1.97)
- The `ensemble` repository checked out as a sibling directory (`../ensemble`):
  the workspace's path dependencies and the hub/MIDI-bridge binaries live there.

## Building and testing

```sh
cargo build
cargo test --workspace
cargo clippy --workspace -- -D warnings
```

## Eyeballing generated maps

Print an ASCII rendering of a generated map for a given seed:

```sh
cargo run -p wfc --example ascii_map 42
```

## Controls

| Input | Action |
| --- | --- |
| WASD | Move |
| Left mouse | Cleave toward the cursor (kills in a 90° arc, wall-occluded) |
| Right mouse | Raise the shield — blocks contact, reflects projectiles in the ring |
| Space | Dash (once unlocked) — short burst with i-frames |
| G | Grenade (once unlocked) — wall-bouncing lob with a radial blast |
| Walk onto the magenta exit | Descend to the next depth (the mob grows; progression carries over) |
| Tab | Pause and open the skills menu |

## Running the audio chain

```sh
# Terminal 1 — from ../ensemble: the hub
cargo run --bin ensemble-hub-tui
# Terminal 2 — from ../ensemble: the MIDI bridge
cargo run --bin ensemble-bridge-midi
# Terminal 3 — the audio voice
cargo run -p audio-voice
# Terminal 4 — the game
cargo run -p game
```

The hub's TUI action monitor shows the published events (`/subit/game/event/*`
combat moments, `/subit/game/telemetry/*` streams) and the scheduled
`/midi/play` notes and `/midi/cc` mod-wheel output. On Windows, REAPER needs a
virtual MIDI loopback (e.g. loopMIDI) to receive notes from the Ensemble MIDI
bridge.
