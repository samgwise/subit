# subit

Prototype workspace for *Signal Breach* (see `initial-vision.md`): a top-down 2D
cyberpunk ARPG with Wave Function Collapse level generation and Ensemble-driven
generative audio.

## Workspace

- `crates/game` — Bevy client: WFC-generated tilemap with physics, WASD player,
  enemy swarm, mouse-aimed cleave combat, and the Ensemble bridge that
  publishes combat events and telemetry to the hub.
- `crates/wfc` — pure-Rust Wave Function Collapse solver, deliberately
  engine-free so it can run off the ECS thread and be tested in isolation.
- `crates/audio-voice` — generative audio voice as a library plus a thin
  standalone binary. The library (quantiser, event→note mapping) can later be
  woven directly into the game process; the binary provides the standalone
  audio daemon role from the GDD.

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

## Running the audio chain

```sh
# Terminal 1 — from ../ensemble: the hub
cargo run --bin ensemble-hub-tui
# Terminal 2 — from ../ensemble: the MIDI bridge
cargo run --bin ensemble-bridge-midi
# Terminal 3 — the audio voice
cargo run -p audio-voice
# Terminal 4 — the game
#   WASD to move, left-click to cleave toward the cursor, enemies chase you
cargo run -p game
```

The hub's TUI action monitor shows the published events (`/subit/game/event/*`
combat moments, `/subit/game/telemetry/*` streams) and the scheduled
`/midi/play` notes and `/midi/cc` mod-wheel output. On Windows, REAPER needs a
virtual MIDI loopback (e.g. loopMIDI) to receive notes from the Ensemble MIDI
bridge.
