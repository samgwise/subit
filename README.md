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
  shield reflection, dash and grenade abilities, destructible cracked walls
  (the dash phases through them, the grenade blasts them open — and wakes
  the neighbourhood), XP/level progression with a pause-and-spend skills
  menu, and the Ensemble bridge publishing combat events and telemetry to
  the hub.
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
- Optional: [ollama](https://ollama.com) running locally for the LLM composer —
  an unreachable ollama (or `OLLAMA_DISABLED=1`) just leaves the deterministic
  score playing.

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

## Tuning the neon shader

The tilemap's neon glow is a WGSL shader with hot reload. Preview every
atlas variant in isolation with the game's bloom (WASD pans):

```sh
cargo run -p game --example shader_preview
```

Edit `assets/shaders/neon_tilemap.wgsl` while the preview (or the game) is
running — saves apply live in about a second; a broken shader logs an error
and keeps the last good look.

## Controls

| Input | Controller | Action |
| --- | --- | --- |
| WASD | Left stick | Move |
| Left mouse | RT (or A) | Cleave toward the cursor / aim stick (kills in a 60° arc, wall-occluded; the reach skill widens and extends it) |
| Right mouse | LT | Raise the shield — blocks contact, reflects projectiles in the ring |
| Space | LB (or B) | Dash (once unlocked) — short burst with i-frames; phases silently through cracked walls |
| E | Y | Nova (once unlocked) — 360° burst that damages and shoves the swarm |
| C | L3 | Cloak (once unlocked) — sneak past enemies; attacking breaks it (the drone firing doesn't) |
| G | RB (or X) | Grenade (once unlocked) — wall-bouncing lob; the blast destroys cracked walls and provokes the neighbourhood (and downs a nearby drone) |
| Walk onto the magenta exit | Walk onto the magenta exit | Descend to the next depth (the mob grows; progression carries over) |
| Tab | Start | Pause and open the skills menu (rows stay mouse-driven) |

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
`/midi/play` notes and `/midi/cc` mod-wheel output. The voice performs the
generative score continuously — give REAPER seven synths on channels 1–7
(bass, distant pad, bell, airy, pattern A, pattern B, combat fx); the mod
wheel rides channel 1. On Windows, REAPER needs a
virtual MIDI loopback (e.g. loopMIDI) to receive notes from the Ensemble MIDI
bridge.

The score's composer (an optional LLM planning tier) reads its model from
ollama — `OLLAMA_URL` (default `http://127.0.0.1:11434`), `OLLAMA_MODEL`
(default `qwen3:0.6b` — small instruction models are the sweet spot: a
validated plan in a couple of seconds), and `OLLAMA_DISABLED=1` to skip
planning entirely. The composer's plans and fallbacks are logged; the
deterministic cycle plays whenever the model is slow, wrong or absent.
