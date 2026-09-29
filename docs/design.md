# Signal Breach — as-built design

The living design reference for the prototype as it is actually built. The
founding vision stays frozen in `initial-vision.md`; when the implementation
and the vision diverge, this document wins and the vision stays untouched.
Shipped milestone history lives in `milestones.md`.

Numbers are gameplay-tuning values. They also exist as constants in code (each
is named once there); this document mirrors them so design discussions can
happen without reading Rust.

## Core loop

Enter a procedurally generated cyber-grid → sweep rogue programs with fast
mouse-aimed AoE combat → collect the XP and HP they drop → level up and spend
skill points in the Tab menu → reach the magenta exit and descend to the next
depth, where the mob grows and the run continues with all progression intact.

## World and procedural generation

- The map is a 48×48 tile grid (32 px tiles) collapsed by a pure-Rust Wave
  Function Collapse solver in the `wfc` crate, engine-free and deterministic:
  same config seed, same map.
- Sockets are edge connectors decoupled from tile classes, with complementary
  matching: `Path` (open floor, accepts Path/Terminal), `Stop` (floor edge that
  ends, accepts Wall only), `Wall` (accepts Wall/Stop), `Terminal` (accepts
  Terminal/Path).
- Tile classes: `Floor` (16 edge-mask variants, weights favouring open
  arenas), `Wall`, `Terminal`. Configured shares: walls 25% of the map,
  terminals 2%. Contradictions and undersized walkable regions restart the
  collapse with a derived seed (up to 100 attempts).
- Reachability is guaranteed: a BFS flood fill keeps only the largest walkable
  region (≥30% of interior cells), picks the spawn at random within it and the
  exit as the cell farthest from the spawn. `wfc::walkable_distances` exposes
  the same BFS for enemy placement.
- Depth descent: walking onto the exit tile (within half a tile) clears the
  world — tiles, colliders, enemies, projectiles, grenades, pickups — and
  regenerates from a depth-derived seed (base seed + depth, still fully
  deterministic). Progression (HP, XP, level, points, unlocks) carries
  forward; the combo and invulnerability reset. Depth scales the mob: +10
  enemies per layer (cap 80), and throwers tighten from every 4th spawn to
  every 3rd from depth 2.
- Rendering is a code-generated 18-tile atlas (row-major image data): a
  grid-lined floor, 16 wall autotile variants with lit edges where walls face
  floor, and a neon-green terminal. Floor brightness scales with open-edge
  count (corridors read darker than arenas) and drifts cooler as world
  integrity drops — the visual twin of the audio mode shift. The exit tile is
  magenta, marked by a pulsing beacon pillar; an off-screen HUD arrow points
  to it when it leaves the viewport.
- The camera zooms 2× (ortho scale 0.5): tiles render ~64 px and roughly a
  dozen tiles span the window — pathing and projectile avoidance read at a
  glance, with the beacon and arrow covering what zoom leaves off-screen.
- Neon bloom: an HDR camera post-process (Bevy's built-in `Bloom`, additive
  composite, ~0.6 luminance threshold) makes the bright pixels bleed glow —
  lit wall edges, the beacon pillar, terminals, the cleave flash, shots and
  pickups — while the dark floor and wall bodies stay matte.
- Neon tilemap shader: the tilemap renders through a custom `MaterialTilemap`
  (WGSL at `assets/shaders/neon_tilemap.wgsl`) that pumps the baked lit
  strips and terminals with a sine pulse driven by GPU time — no per-frame
  CPU work. Wall-edge strips brighten toward white-cyan (strength 0.45) at
  3 rad/s with a phase that drifts across each tile so the shimmer travels
  along the edge; terminals pulse gently (0.2); floors and wall bodies pass
  through untouched. Strip pixels are detected by luminance (> 0.35 — strips
  bake at ~0.7 linear, wall bodies ~0.07). Tunables are WGSL constants and
  hot-reload: edit the file and the running game (or the shader preview
  example) updates within a second; a broken shader logs an error and keeps
  the last good look.

## The player

- Dynamic physics body, 0.6-tile square collider, rotation locked. WASD moves
  at 240 u/s; walls resolve collisions. The camera follows.
- Base 100 HP, raised 25 per vitality point (the upgrade heals the same
  amount it raises the ceiling). Damage sources: enemy contact (30), enemy
  projectile (20). Damage opens a 0.5 s invulnerability window — red flash
  for 0.15 s, then a blink.
- Barrier (unlock: 3 points): a regenerating 10-plate pool (5 HP per plate)
  that soaks damage before health — the player's version of the enemy
  shields. Regrows one plate per 2.5 s without damage; shown as a violet
  HUD bar and a violet energy dome (slightly wider than the reflect dome)
  while owned with plates remaining.
- Death: respawn at the map spawn with full HP; the combo resets.

## Combat

- **Cleave (left mouse):** a 60° cone (the reach skill widens it by 5° per
  point), 3-tile radius (+0.25 tiles per reach point, cap 4), aimed from the
  player to the cursor, 0.25 s cooldown. Hits only enemies with a clear grid
  line of sight (exact Amanatides–Woo traversal; diagonal corner peeks
  between walls are blocked conservatively). Base damage 100 (+25 per skill
  point), shared with reflected projectiles. All kills in one swing batch
  into a single mob-sweep event.
- **Combo:** kills within 2 s of the previous kill chain (+1 each); the chain
  resets to 1 on the next kill after the window lapses. The chain lifts the
  mob-sweep performance intensity.
- **Shield (right mouse):** 0.6 s active, 3 s cooldown (−0.25 s per skill
  point, floor 1 s). While up: contact damage is blocked and any enemy
  projectile inside the ring (1.2-tile radius — the cyan dome drawn around
  the player is the real catch zone) is caught and reflected along the
  cursor with a fresh bounce budget. Reflected shots turn cyan, become
  player weapons, and can never hit the player again (physical via
  collision layers). With the deflect volley unlock (5 points) each catch
  also fires two extra auto-aimed shots at the nearest enemies (a ±30° fan
  when the swarm is elsewhere).
- **Grenade (G, unlock: 3 points):** 5 s cooldown. A fused lob along the
  cursor at 400 u/s that bounces off walls — two bounces survive, the third
  wall contact detonates it where it hits — or the 0.5 s fuse sets it off
  mid-flight. Blast: 200 damage (+50 per damage point, cap 6) in a 2.5-tile
  radius; kills batch into the combo.
- **Dash (Space, unlock: 2 points):** 2 s cooldown (−0.25 s per point, cap
  4, floor 1 s), 0.15 s at 600 u/s along the current WASD direction
  (falling back to the cursor). The dash grants i-frames by opening the
  invulnerability window.
- **Nova (E, unlock: 4 points):** 6 s cooldown. A 360° burst — 150 damage
  in a 2.5-tile radius that shoves every survivor outward (a decaying kick
  layered over their steering); routes through shields like every hit.
  An expanding ring shows the reach.
- Every hit routes through plate pools first: enemy shields soak their
  plates before an enemy's health, the barrier before player HP — one shared
  absorption helper for cleaves, reflected shots, projectiles and blasts.

## Enemies

- 40 enemies per map (more with depth), spawned on tiles that are
  BFS-reachable and at least 8 steps from the player spawn; layout is
  deterministic per seed. Every 4th spawn is a thrower (every 3rd from depth
  2) and every 8th from depth 1 is a tank; the roles shuffle so the specials
  scatter through the mob instead of clustering.
- **Chaser:** red, 100 HP (one base cleave), 120 u/s, steers straight at the
  player; wall and enemy-to-enemy collisions come from physics.
- **Tank:** red-brown, 400 HP, 60 u/s, a chunky 0.9-tile body — an unhurried
  wall of muscle that soaks the cleaves that delete lesser enemies.
- **Thrower:** amber, 200 HP (two base cleaves), 70 u/s, holds 6 tiles away
  and lobs
  a bouncing projectile at the player's current position every 2 s (no
  leading, no line-of-sight check — blind lobs around corners are a feature).
  Projectiles fly at 240 u/s (12 px sprite), bounce off walls up to 3 times
  then fail, and despawn on an unshielded player hit. They pass through other
  enemies, and a reflected one can never re-hit the player (collision
  layers).
- **Shields:** each spawn rolls for a regenerating shield — chance 5% + 5%
  per depth, capped at 40%. Eight plates of 5 HP: each plate soaks 5 damage
  (a lighter hit still cracks the plate whole), regrowing one plate per
  2.5 s without damage. Shielded enemies wear a hex-faceted energy dome
  (a custom WGSL material, `assets/shaders/shield_dome.wgsl`) whose rim is
  divided into eight arcs — one per plate: cracked plates go dark and
  regrow visibly, and a hit flashes the dome. Hidden while the pool is
  empty.
- Enemies darken with damage — sprite brightness scales with their HP
  fraction (floor 45%) — so remaining hits read at a glance.

## Drops and progression

- Kills always drop an XP shard (10 chaser, 25 thrower) and 15% of kills add
  an HP cross (25 HP, clamped to max). Pickups (10–12 px, alpha-pulsing) drift
  to the player inside a 2-tile magnet radius and apply within half a tile.
- `xp_for_level(level) = 40 + 30 × level` — linear, the single place the curve
  shape lives. A full 40-enemy clear (~550 XP) is about five levels.
- Each level-up grants 1 skill point. Tab pauses the world (all simulation
  systems gate on the game state) and opens the skills menu.
- Skills: cleave damage +25 (1 point), cleave reach +0.25 tiles with arc
  +5° (1 point, cap 4), shield cooldown −0.25 s (1 point, cap 4, floor 1 s),
  shield duration +0.15 s (1 point, cap 4), max health +25 (1 point), dash
  cooldown −0.25 s (1 point, cap 4, floor 1 s, needs dash), grenade damage
  +50 (1 point, cap 6, needs grenade), dash unlock (2 points), grenade
  unlock (3 points), barrier unlock (3 points), nova unlock (4 points),
  deflect volley unlock (5 points). Purchases apply immediately; rows grey
  out when unaffordable, maxed or owned, and show their level and cost.

## Ensemble audio

Chain: game bridge → Ensemble hub → audio-voice daemon → MIDI bridge →
REAPER. The voice quantises discrete events to 1/16ths at 120 BPM (stub tempo
until the hub clock protocol lands); telemetry bypasses quantisation.

| Game moment | Address / signal | Voice output |
| --- | --- | --- |
| Primary attack | `/subit/game/event/action` `attack_primary` (event) | Quantised stab — note 60, vel 100, 0.25 s |
| Mob sweep (kills per swing/blast, combo) | `/subit/game/event/combat` `mob_sweep` (event) | Melodic run — note 72 + kill count (max +12), velocity 100 + 5 per combo step, 0.5 s |
| Projectile thrown | `/subit/game/event/action` `projectile_throw` (event) | Low blip — note 45, vel 60, 0.1 s |
| Shield reflect | `/subit/game/event/action` `shield_reflect` (event) | Bright stab — note 79, vel 110, 0.15 s |
| Level up | `/subit/game/event/action` `level_up` (event) | Chime — note 84, vel 90, 0.3 s |
| Dash | `/subit/game/event/action` `dash` (event) | Whoosh — note 91, vel 70, 0.08 s |
| Grenade blast | `/subit/game/event/action` `grenade_blast` (event) | Boom — note 36, vel 127, 0.4 s |
| Depth descent | `/subit/game/event/action` `descent` (event) | Transition tone — note 43, vel 100, 0.4 s |
| Player speed (≈10 Hz) | `/subit/game/telemetry/player` `player_speed` (stream) | Mod-wheel CC1, full scale at 240 u/s |
| World integrity (per map) | `/subit/game/telemetry/world` `world_integrity` (param) | Logged only — harmonic mode shift pending |

## Tuning reference

| Constant | Value |
| --- | --- |
| Player speed / base max HP / vitality per point | 240 u/s / 100 / +25 |
| Camera zoom | 2× (ortho scale 0.5, ~64 px tiles) |
| Bloom | OLD_SCHOOL preset, intensity 0.15 (additive, ~0.6 threshold) |
| Neon shader: pulse speed / strip strength / terminal strength / edge threshold | 3 rad/s / 0.45 / 0.2 / luminance 0.35 |
|| Cleave: radius / cone / cooldown / base damage / damage per point | 3–4 tiles / 60–80° / 0.25 s / 100 / +25 |
|| Shield duration: per point / cap | +0.15 s / 4 (0.6 → 1.2 s) |
|| Dash cooldown reduction: per point / floor | −0.25 s / 1.0 s |
|| Grenade damage: per point / cap | +50 / 6 |
|| Nova: damage / radius / cooldown / unlock cost | 150 / 2.5 tiles / 6 s / 4 pts |
|| Deflect volley: extra shots / unlock cost | 2 / 5 pts |
| Combo window | 2 s |
| Shield: active / cooldown / ring | 0.6 s / 3 s (−0.25/pt, floor 1 s) / 1.2 tiles |
| Dash: speed / duration / cooldown / unlock cost | 600 u/s / 0.15 s / 2 s / 2 pts |
| Grenade: speed / fuse / cooldown / blast / damage / bounces / unlock cost | 400 u/s / 0.5 s / 5 s / 2.5 tiles / 200 / 2 / 3 pts |
| Enemy: count / chaser HP / thrower HP / tank HP / min spawn distance | 40 + 10×depth (cap 80) / 100 / 200 / 400 / 8 BFS steps |
| Chaser / thrower / tank speed, throw range, throw cooldown | 120 / 70 / 60 / 6 tiles / 2 s |
| Thrower / tank frequency | every 4th spawn (every 3rd from depth 2) / every 8th from depth 1 |
| Enemy shield: plates / plate HP / regen / spawn chance | 8 / 5 / one plate per 2.5 s / 5% + 5%×depth (cap 40%) |
| Shield dome: hex spin / hit flash decay / barrier radius | 0.2 rad/s / ~0.3 s (decay 8) / 1.35 tiles |
| Player barrier: plates / regen / unlock cost | 10 / one plate per 2.5 s / 3 pts |
| Vitality: max HP per point | +25 (heals the same) |
| Projectile: speed / bounces / lifetime / player damage | 240 u/s / 3 / 16 s / 20 |
| Drops: XP chaser/thrower, heal chance/amount | 10 / 25, 15% / 25 HP |
| XP curve / magnet / collect radius | 40 + 30×level / 2 tiles / 0.5 tiles |
| Map: size / wall share / terminal share / min walkable | 48×48 / 25% / 2% / 30% |

## Pending and known gaps

- World integrity does not shift the harmonic mode in the audio voice yet.
- Throwers never check line of sight before lobbing.
- No persistence or rebinding UI.
