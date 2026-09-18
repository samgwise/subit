# subit

Prototype workspace for *Signal Breach* (see `initial-vision.md`): a top-down 2D
cyberpunk ARPG with Wave Function Collapse level generation and Ensemble-driven
generative audio.

## Workspace

- `crates/game` — Bevy client: window, WASD placeholder player, tilemap/physics
  wiring, and the Ensemble bridge that publishes game events to the hub.
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

## Running the audio chain (Milestone 1 verification)

```sh
# Terminal 1 — from ../ensemble: the hub
cargo run --bin ensemble-hub-tui
# Terminal 2 — from ../ensemble: the MIDI bridge
cargo run --bin ensemble-bridge-midi
# Terminal 3 — the audio voice
cargo run -p audio-voice
# Terminal 4 — the game (press Space to fire a test event)
cargo run -p game
```

The hub's TUI action monitor shows the published events and the scheduled
`/midi/play` actions. On Windows, REAPER needs a virtual MIDI loopback
(e.g. loopMIDI) to receive notes from the Ensemble MIDI bridge.
