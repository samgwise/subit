This is a fantastic combination. Combining a **Cyberpunk Synthwave aesthetic**, **2D Grid top-down movement**, **Wave Function Collapse (WFC) generation**, **Fast Mob-Sweeping combat**, and **Full Interactive Audio** creates a tight, highly feedback-driven game loop.

Because player actions actively trigger quantized musical events, combat *becomes* part of the generative performance.

Here is the draft for your foundational Game Design Document (GDD) and system architecture.

---

# Prototype Game Design Document (GDD)

## 1. High-Level Vision

* **Title / Working Name:** *Signal Breach* (or *Grid Runner*)
* **Genre:** Top-Down 2D Cyberpunk ARPG / Horde Sweeper
* **Core Loop:** Enter a procedurally collapsing cyber-grid -> Sweep density-packed rogue programs with fast-paced AoE combat -> Harvest data nodes -> Reach the gateway to the next depth layer.
* **Audio Core Concept:** The game is a live musical instrument. Player movement, weapon triggers, enemy deaths, and tile states drive the harmonic generation engine via the Ensemble protocol.

---

## 2. Technical Stack Architecture

```text
+-----------------------------------------------------------------------------------+
|                                  BEVY ARPG ENGINE                                 |
|                                                                                   |
|  [ WFC Grid Generator ] --> [ bevy_ecs_tilemap ] --> [ avian2d Physics World ]    |
|                                                                                   |
|  [ Combat / Event Systems ] ------------------------------------+                 |
+-----------------------------------------------------------------|-----------------+
                                                                  |
                                                     Ensemble IPC / Protocol
                                                                  |
                                                                  v
+-----------------------------------------------------------------------------------+
|                             GENERATIVE AUDIO DAEMON                               |
|                                                                                   |
|  [ Time-Sync Engine ] --> [ Quantizer & Harmonic Engine ] --> [ MIDI Output Engine]|
+-----------------------------------------------------------------|-----------------+
                                                                  |
                                                            Virtual MIDI
                                                                  |
                                                                  v
                                                     +--------------------------+
                                                     |  REAPER DAW (Synth Racks)|
                                                     +--------------------------+

```

* **Game Engine:** Bevy (Rust)
* **Physics:** `avian2d` (Handles player movement, project collisions, and mob hitboxes)
* **Tilemap Rendering:** `bevy_ecs_tilemap`
* **ProcGen:** Custom Wave Function Collapse (WFC) engine outputting abstract grid arrays.
* **Audio Protocol:** `ensemble` for IPC messaging, timing synchronization, and quantized event streaming.
* **Sound Design Host:** REAPER driving soft-synths/samplers via virtual MIDI.

---

## 3. Level Generation (WFC Topology)

### WFC Constraint Architecture

The map consists of a 2D tile grid. WFC uses local adjacency constraints to guarantee path connectivity while creating dense, neon-lit industrial corridor layouts.

* **Tile Sockets & Metadata:**
* `Open Ground`: High traversal speed, high enemy spawn capacity.
* `Wall / Conduit`: Blocking physics, high visual light emitter density.
* `Terminal / Node`: Interactive objective points that trigger audio mode shifts.


* **Pipeline Sequence:**
1. **Phase 1 (Abstract WFC Execution):** Run the WFC collapse in pure Rust off the ECS thread to resolve the matrix array.
2. **Phase 2 (Graph Validation):** Run a flood-fill/A* check to ensure spawn-to-exit reachability.
3. **Phase 3 (ECS Instantiation):** Populate `bevy_ecs_tilemap` and generate `avian2d` static colliders for wall sockets.



---

## 4. Gameplay & Combat Mechanics

* **Player Control:** WASD 8-directional movement with mouse-aim directional shooting/cleaving.
* **Abilities:**
* *Primary Attack (Pulse Blade / Arc Shot):* High-frequency AoE cleave. Fast cooldown.
* *Secondary Attack (Emp Burst):* Radial knockback that stuns mobs and disrupts grid tiles.
* *Dash (Phase Shift):* Short-range invulnerability pass-through.


* **Enemy Density:** High-volume "swarms" with simple flocking/steering behavior aiming to overwhelm the player.

---

## 5. Interactive Generative Audio Model (Ensemble Integration)

Player actions fire timing-aware events across the Ensemble protocol bridge to drive rhythmic and melodic patterns in sync with the master clock.

| Game Action | Ensemble Message / Telemetry Payload | Audio Daemon Output |
| --- | --- | --- |
| **Beat Pulse (Internal Clock)** | Ensemble Timing Sync Protocol | Drives global tempo, quantizes all incoming game actions to nearest 1/16th note. |
| **Player Movement Velocity** | `/telemetry/player { speed: f32 }` | Modulates synth filter cutoff / arpeggiator rate in real-time. |
| **Primary Attack Trigger** | `/event/action { type: "attack_primary" }` | Sends quantized synth-stab MIDI note on the current scale degree. |
| **Enemy Kill / AoE Mob Sweep** | `/event/combat { kill_count: u32, combo: u32 }` | Triggers melodic pentatonic runs or percussion fills based on hit count. |
| **WFC Biome Collapse Level** | `/telemetry/world { integrity: f32 }` | Shifts the active harmonic scale mode (e.g., Dorian to Phrygian) as grid destabilizes. |

---

## 6. Development Milestones

1. **Milestone 1 (Audio Infrastructure):** Set up the `ensemble` protocol bridge between a basic Rust test binary and REAPER. Verify note quantization and timing sync over virtual MIDI.
2. **Milestone 2 (ProcGen Core):** Implement the standalone WFC grid solver in pure Rust with 4 basic tile socket types. Verify guaranteed pathing.
3. **Milestone 3 (Bevy Integration):** Hook the WFC output into `bevy_ecs_tilemap` and set up `avian2d` collision shapes for wall tiles.
4. **Milestone 4 (Combat & Telemetry):** Implement WASD player movement, basic swarm AI, and wire combat events into the Ensemble publisher.