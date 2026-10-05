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
  collapse with a derived seed (up to 100 attempts). Exhausting the budget
  is a real outcome — about a quarter of default-config seeds reject ~98%
  of their attempts on region size (solver contradictions never happen):
  the game escalates to an uncorrelated seed family for up to four rounds,
  halving and then dropping the walkable floor only as a last resort, so a
  map always comes out and the same (seed, depth) stays deterministic.
  Every escalation is logged as a warning — a tuning signal, not an error.
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
- Rendering is a code-generated 34-tile atlas (row-major image data): a
  grid-lined floor, 16 wall autotile variants with lit edges where walls face
  floor, 16 cracked wall variants (the same lit edges plus baked crack
  lines — they pulse like walls), and a neon-green terminal. Floor brightness scales with open-edge
  count (corridors read darker than arenas) and drifts cooler as world
  integrity drops — the visual twin of the audio mode shift. The exit tile is
  magenta, marked by a pulsing beacon pillar; an off-screen HUD arrow points
  to it when it leaves the viewport.
- The camera zooms 2× (ortho scale 0.5): tiles render ~64 px and roughly a
  dozen tiles span the window — pathing and projectile avoidance read at a
  glance, with the beacon and arrow covering what zoom leaves off-screen.
- Corrupted data zones: terminal CLUSTERS (terminals with a terminal
  neighbour) seed corrupted patches — the cluster dilated by one tile,
  walls excluded. The zones glitch: a hash-scheduled ~0.3 s burst every
  ~3 s shuffles, drops and tears the base tiles' pixels (a zone-mask
  texture read in the neon shader), and a sparse overlay tilemap flickers
  green static and data rain above them — one shared burst clock, all
  GPU-time driven. Every shield inside a zone bleeds a plate per second
  (enemy shields and the player's barrier alike — lure the tank through
  the corruption to strip it), and the player's aggro notice range sheds
  2 steps inside a zone (stacking with the cloak, floored at 1). Crossing
  a zone edge is a sonic trigger.
- Cracked walls: about 18% of the *eligible* wall tiles are cracked — a
  deterministic per-seed pass after generation (the solver never knows; the
  sealed border never cracks). Eligibility is thinness: the tile must be
  walkable on one full axis (N+S or E+W), so every crack reads as a
  passable door along its thin side and a phasing dash can never end
  embedded in a thick wall. They render with the cracked variants and
  block like walls, and they come down two ways. The dash phases through
  them silently: the player's collision filter drops the cracked layer
  mid-dash and restores the frame it ends — nothing is destroyed and no
  shortcut is left behind. The phase only arms when the dash rests on
  walkable ground (the dash line is marched quarter-tile by quarter-tile:
  cracks phase, solid walls stop the travel); a crack that backs onto more
  wall leaves the dash bonking like any wall instead of wedging the
  player. The grenade blast destroys cracks loudly: the tile becomes open
  floor (tinted from the depth's stored integrity), the surviving walls'
  autotile masks recompute around the hole, the cracked compound rebuilds
  (vanishing when the last one goes), and the flow field rebuilds through
  the new route — and the downed walls dissolve as wall-grey corpses in
  the same corruption language the enemies get. The blast is loud too —
  every enemy within 10 tiles (euclidean, so sound passes through walls)
  is provoked: a forced ~10 s hunt that ignores the aggro hysteresis
  until it lapses.
- Locked vaults: every depth seals one room off the main path — a natural
  island when one qualifies (at least 12 cells with a single thin wall
  facing the main region), otherwise a carved 5×3 room inside a 7×5 wall
  ring punched beside the main path (placement is deterministic per
  (seed, depth), and a depth that fits neither runs vault-less rather
  than failing). The door is a wall in the grid — nothing paths, sees or
  routes through — drawn as a pulsing amber slab on its own small body.
  Inside waits a mixed mob (3 enemies, +1 every 3 depths, cap 5 — same
  role mix and shield rolls as the depth's swarm, calm because the room
  is unreachable) and a guaranteed HP cross at the room's centre. The
  door and the room's whole seal are excluded from cracked-wall marking,
  so grenades and dashes can never open a second way in; the vault is
  the transmitter's to open.
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
- Cloak (unlock: 5 points, C): toggling C engages it for up to its
  duration (3 s, +1 s per point, cap 4) — the enemy notice range shrinks
  to 6 path steps while it is up; the sneak skill tightens it by a step
  per point (cap 4, two at max). C again drops it early. The cloak doubles
  as an escape: alerted enemies stand down once you're beyond the sneak
  range, and re-homing mills them where they lost you. The player's sprite
  dims to a slow shimmer while cloaked; attacking (cleave, nova, grenade)
  ends it too — dashing doesn't. The 10 s cooldown starts when the cloak
  drops.
- Death: respawn at the map spawn with full HP; the combo resets.

## Input and controls

- Twin-stick beside the mouse: WASD or the left stick moves (magnitudes
  merge — a half-tilted stick walks), the right stick aims where the cursor
  would. Whichever device last produced input owns the aim — a displaced
  stick holds its last direction once recentred, a moved mouse claims it
  back — and every consumer reads one shared `Aim` direction.
- Sticks run through a radial deadzone (rescaled from the zone's edge, so
  neutral never snaps); bevy's per-axis settings sit underneath. While the
  pad owns the aim and the stick rests, the aim drifts toward the direction
  of travel (8 rad/s, settling on it) — the character faces where they're
  walking, so re-engaging the trigger starts near where you expect.
- Mapping: RT cleave, LT shield, LB dash, RB grenade, Y nova, L3 cloak,
  Start the skills menu — the combat verbs sit on triggers and bumpers
  because the right thumb lives on the aim stick; the face buttons stay as
  aliases (A cleave, B dash, X grenade, Y nova) for when it isn't. Each
  ability's trigger is the pad edge OR'd with its key/mouse counterpart.
  The menu's rows stay mouse-driven (a known gap, shared with keyboard
  navigation).
- Aim indicator: while the pad owns the aim, a thin cyan line runs from
  the player out to the cleave's current reach with a small end cap — the
  stick player's cursor, and a range hint in one. The mouse's cursor makes
  it invisible to mouse play.
- Standard-gamepad only — no Deck-specific code: the Steam Deck (native or
  through Proton's Steam Input) and any XInput pad land in the same input
  stack.

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
- **Shield (right mouse):** toggles up for at most 0.6 s (the duration
  skill adds +0.15 s per point), a second click drops it early, and the
  3 s cooldown (−0.25 s per skill point, floor 1 s) starts the moment it's
  down. While up: contact damage is blocked and any enemy
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
  radius; kills batch into the combo. Cracked walls in the blast radius are
  destroyed (they become open floor — see World) and every enemy within
  10 tiles is provoked into a forced hunt (see Enemies).
- **Dash (Space, unlock: 2 points):** 2 s cooldown (−0.25 s per point, cap
  4, floor 1 s), 0.15 s at 600 u/s along the current WASD direction
  (falling back to the cursor). The dash grants i-frames by opening the
  invulnerability window, and it phases silently through cracked walls —
  the collision filter drops the cracked layer mid-dash, restoring the
  frame it ends. The phase arms only when the dash would rest on walkable
  ground; a crack that backs onto more wall bonks like any wall.
- **Nova (E, unlock: 4 points):** 6 s cooldown. A 360° burst — 150 damage
  in a 2.5-tile radius that shoves every survivor outward (a decaying kick
  layered over their steering); routes through shields like every hit.
  An expanding ring shows the reach.
- **Transmitter drone (unlock: 2 points):** the vault key first, a weapon
  second. Owning it opens locked vaults — walking within 1.5 tiles of a
  closed door broadcasts the code and the door slides open (an
  unlock-and-slide chime through the audio chain). The transmitter fields
  no drones of its own; the fleet skill adds one drone per point (cap 4,
  one point each), each hovering at a 2-tile standoff ring and firing a
  34-damage chip shot at the nearest enemy within 6 tiles every 2 s — no
  line-of-sight check (bouncing lobs read fine coming from a machine).
  Drones follow the player: straight when the sight line is clear,
  otherwise down the player's flow field, gliding at 180 u/s with no
  collider — nothing touches them in flight. The link is fragile in
  exactly two ways. Corruption jams it: while the player stands in a
  corrupted zone the drones hang dark and offline — the drones' own
  position is irrelevant (they hover over corruption freely, so they can
  never strand themselves in a zone they cannot leave offline) — and the
  8 s reboot clock only starts when the player leaves the zone. A grenade
  blast fries an online drone caught in the radius — the same 8 s reboot,
  held if the jam keeps it down. Shots from drones count as the player's:
  a surviving target aggros on the spot, though the drone firing never
  breaks the cloak. Kills feed the usual drops and combo.
- Every hit routes through plate pools first: enemy shields soak their
  plates before an enemy's health, the barrier before player HP — one shared
  absorption helper for cleaves, reflected shots, projectiles and blasts.

## Enemies

- **Steering:** enemies pursue with a hybrid seek — straight at the player
  when the sight line is clear (smooth pursuit, no grid quantisation),
  otherwise along a BFS flow field toward the player's tile that routes
  around bends instead of pressing into the nearest corner. The field
  (`wfc::FlowField`, built over the existing reachability BFS) rebuilds only
  when the player changes tile or the map regenerates; enemy reads are O(1).
  Throwers keep their hold-at-range behaviour and only flow-field while
  approaching. Colliders are circles (same footprints as the old boxes), so
  agents glide around tile vertices instead of snagging on them.
- **Aggro:** enemies only notice the player within 8 path steps (BFS
  distance around the walls — an enemy behind a wall stays calm no matter
  how close it stands), standing down once the player escapes 10 — a
  hysteresis band keeps the boundary calm. Inside a corrupted zone the
  notice range sheds 2 steps (stacking with the cloak, floored at 1).
  Un-alerted enemies mill slowly around home on a leash (they never open
  fire or pursue), re-homing where they lost the player, so every depth
  opens calm and the mob engages as you reach it. A grenade blast provokes
  every enemy within 10 tiles — euclidean, through walls: forced pursuit
  for ~10 s with no stand-down while it lasts; the normal hysteresis
  resumes when it lapses.
- 40 enemies per map (more with depth), spawned on tiles that are
  BFS-reachable and at least 10 steps from the player spawn — beyond the
  stand-down range, so no depth opens aggroed; layout is
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
- **Death:** corpses don't vanish — they dissolve in the corruption's
  own glitch language: hash-blocked dropout eats the sprite, scanlines
  tear out whole rows, blocks jitter brightness and occasionally rotate
  the colour channels, and the eating edge glows corruption-green (the
  bloom spreads it). The corpse keeps its own colour — the damage tint
  it died in — and takes ~0.5 s to go, dissolving behind the drops it
  scatters (a custom `Material2d` over a quad per corpse,
  `assets/shaders/dissolve.wgsl`; broken cracked walls dissolve the
  same way in wall grey).

## Drops and progression

- Kills always drop an XP shard (10 chaser, 25 thrower) and 15% of kills add
  an HP cross (25 HP, clamped to max). Pickups (10–12 px, alpha-pulsing) drift
  to the player inside a 2-tile magnet radius and apply within half a tile.
  The vault holds a guaranteed cross at its centre — the room's treasure,
  claimed with the mob's own drops.
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
  deflect volley unlock (5 points), cloak unlock (5 points) with sneak
  −1 step (cap 4, two at max) and duration +1 s (cap 4) upgrades,
  transmitter unlock (2 points — the vault key), drone fleet +1 drone per
  point (cap 4, needs transmitter; the unlock fields none).
  Purchases apply immediately; rows grey out when unaffordable, maxed or
  owned, and show their level and cost.

## Ensemble audio

Chain: game bridge → Ensemble hub → audio-voice daemon → MIDI bridge →
REAPER. The voice quantises discrete events to 1/16ths at 120 BPM (stub tempo
until the hub clock protocol lands); telemetry bypasses quantisation. A
generative score runs continuously underneath the one-shot stabs (below).

| Game moment | Address / signal | Voice output |
| --- | --- | --- |
| Primary attack | `/subit/game/event/action` `attack_primary` (event) | Quantised stab — note 60, vel 100, 0.25 s |
| Mob sweep (kills per swing/blast, combo) | `/subit/game/event/combat` `mob_sweep` (event) | Melodic run — note 72 + kill count (max +12), velocity 100 + 5 per combo step, 0.5 s |
| Projectile thrown | `/subit/game/event/action` `projectile_throw` (event) | Low blip — note 45, vel 60, 0.1 s |
| Shield reflect | `/subit/game/event/action` `shield_reflect` (event) | Bright stab — note 79, vel 110, 0.15 s |
| Level up | `/subit/game/event/action` `level_up` (event) | Chime — note 84, vel 90, 0.3 s |
| Dash | `/subit/game/event/action` `dash` (event) | Whoosh — note 91, vel 70, 0.08 s |
| Grenade blast | `/subit/game/event/action` `grenade_blast` (event) | Boom — note 36, vel 127, 0.4 s |
| Nova burst | `/subit/game/event/action` `nova` (event) | Deeper boom — note 38, vel 127, 0.45 s |
| Cloak engage | `/subit/game/event/action` `cloak` (event) | Soft descending breath — note 67, vel 75, 0.3 s |
| Corruption enter | `/subit/game/event/action` `corruption_enter` (event) | Dissonant glitch stab — note 46, vel 95, 0.3 s |
| Corruption exit | `/subit/game/event/action` `corruption_exit` (event) | Resolve blip — note 52, vel 60, 0.15 s |
| Depth descent | `/subit/game/event/action` `descent` (event) | Transition tone — note 43, vel 100, 0.4 s |
| Transmitter drone fires | `/subit/game/event/action` `drone_shot` (event) | Small blip — note 64, vel 55, 0.1 s |
| Transmitter drone goes offline | `/subit/game/event/action` `drone_down` (event) | Glitchy fall — note 40, vel 100, 0.35 s |
| Transmitter drone rejoins | `/subit/game/event/action` `drone_online` (event) | Rising blip — note 76, vel 85, 0.2 s |
| Vault door opens | `/subit/game/event/action` `vault_open` (event) | Unlock-and-slide chime — note 70, vel 90, 0.3 s |
| Player speed (≈10 Hz) | `/subit/game/telemetry/player` `player_speed` (stream) | Mod-wheel CC1, full scale at 240 u/s |
| Aggro locks (≈10 Hz, on change) | `/subit/game/telemetry/aggro` `aggro_locks` (stream) | Conducts the generative score (below) |
| World integrity (per map) | `/subit/game/telemetry/world` `world_integrity` (param) | Logged only — harmonic mode shift pending |

### The generative score

Underneath the one-shots, the voice performs a continuous score built on a
chromatic ↔ scale ↔ harmony layer stack (`scalevec`): the harmony cycle
spells nine chord slots as degrees of a D major/minor mixture collection
(with the borrowed G# the dim7 needs), the scale layer resolves degrees to
semitones, and the chromatic ladder hands them to MIDI as note numbers.
The cycle — Dm, Bm, D, B, F#, C#m4-3, B/D, E, G#dim7 — puts E major
penultimate to prepare the dim7 (the shared G# and B make the slide smooth),
and the dim7 resolves home to Dm. The bass is locked to the cycle in
lockstep, sounding its slot's designated bass (the D under B/D included) as
quaver pulses — six of them and then rest until the next change in the
relaxed regimes (3+ beats per chord), continuous quavers through the short
ones, so the busier harmonic rhythms carry the bass drive with them — every
pairing is intentional, and later harmonic disintegration can be a
deliberate transformation of a single layer while the others hold.

The live aggro-lock count conducts three knobs off a compressor-smoothed
intensity (fast attack, slow release, so boundaries never strobe): pattern
gating (pattern A joins at 1 lock, pattern B at 3), note density (dropout
thins from full 16ths at max intensity to quarter-note pulses at rest), and
harmonic rhythm — one chord per eight crotchets at rest, shedding two beats
at a time down to two, then going additive in quavers at full flight
(3+2, 3+3+2, 3+2+2, cycling). Descents hold their rung for 3 s before
dropping down — a fight ends, the drive eases rather than collapses (and a
recovering fight cancels the hold and follows straight back up). The rhythm
recalculates mid-chord: a new period landing inside the sounding chord cuts
it short there (a note-off silences the bass and pad at the boundary), one
landing past the note's end leaves it to ring and changes at the end of the
note.

Harmonic degradation: while the player stands in a corrupted data zone,
every chromatic mapping mutates — each chord tone's pitch class rotates by
a noise-like hash (deterministic, ±3 semitones) before it lands, so the
harmony comes out wrong the same way every time while different pitches
scatter differently — the corruption's glitch language applied to the
pitch resolution chain. The bass stays clean (it never passes through the
chromatic layer — the corrupted upper structure over an anchored bass is
the instability), and the patterns inherit the rotation by walking the
mutated voicings. Crossing back out restores the harmony.

### The composer — an LLM-planned harmony

The deterministic cycle is the ground truth the score falls back to; the
voice also carries a composer — a slow planning tier over the performing
engine. A local ollama model reads the score's recent past (the last 16
performed slots: chord, duration, whether the conductor cut or held it)
and the game's present (tallies since the last plan: mob sweeps, kills,
level-ups, corruption boundaries, descents, and the aggro average and
peak) and lays out the next phrase — 4–8 chord slots picked from a
thirteen-entry vocabulary (the cycle's nine plus submarine's function
palette: supertonic, subdominant, dominant, dominant seventh), each
labelled with its harmonic function, plus optional Bézier control points
for the phrase's voicing contour.

The plan travels as flat JSON (it doubles as the ollama response schema —
grammar-constrained sampling misbehaves on nested shapes) and must pass
the grammar in code before it's performed: roots stay within five
semitones of each other (folded around the octave — the dim7's resolution
excepted, whose voices rather than its root do the stepping), no more
than two of the same chord in a row, and the phrase closes on a tonic or
a dominant.
Rejected plans and transport failures keep the current phrase playing; a
depleted plan hands back to the deterministic cycle. The model never owns
the clock — the conductor keeps the rhythm ladder, cuts and holds (the
schema asks for no durations at all); it owns direction only.

The composer replans when the live plan drains to two slots, or on sharp
shifts: a corruption boundary, a level-up, a descent, or the aggro peak
racing three locks past the phrase's average. One request is in flight at
a time (queued requests collapse to the newest), requests time out after
30 s (a safety valve — the verified model plans in a couple of seconds),
and `OLLAMA_DISABLED=1` hard-disables the tier — the score never stalls
on the model.

Contour: a plan may draw the phrase's register as paired upper/lower
Bézier control points (semitones from the tonic). The patterns' notes
clamp into the bounds at their phrase position — the arp exists below
the curve — and consecutive curves stitch from the previous tail control
point so phrases join without a seam. Plans without curves get the
neutral default, whose bounds sit outside today's material. Env:
`OLLAMA_URL` (default `http://127.0.0.1:11434`), `OLLAMA_MODEL`
(default `qwen3:0.6b` — verified: a validated plan in a couple of
seconds; heavier generalists trip ollama 0.35's grammar into
minute-long sampler rollbacks), `OLLAMA_DISABLED`.

Channel map (one synth each in REAPER): 1 bass, 2 chords, 3 pattern A,
4 pattern B, 5 one-shot combat fx. The mod wheel (player speed) stays on
channel 1 — the bass synth's filter tracks motion.

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
||| Deflect volley: extra shots / unlock cost | 2 / 5 pts |
||| Transmitter drone: damage / cadence / range / speed / standoff / reboot | 34 / 2 s / 6 tiles / 180 u/s / 2 tiles / 8 s |
||| Transmitter unlock / fleet | 2 pts / +1 drone per point (cap 4, none at unlock) |
||| Vault: min island / carved interior / mob / door open radius | 12 cells / 5×3 / 3 + 1 per 3 depths (cap 5) / 1.5 tiles |
| Death dissolve | ~0.5 s, corruption-green eating edge |
| Score: chord period / pattern gates / rhythm hold / smoothing | 8 → 6 → 4 → 2 crotchets, then 3+2 / 3+3+2 / 3+2+2 quavers (additive past 0.95 intensity) / A ≥1 lock, B ≥3 / 3 s before dropping down / τ 0.5 s attack, 8 s release |
| Bass: pulse rate / relaxed figure / gate | quavers / 6 then rest (periods of 3+ beats), continuous below / 0.6 × quaver |
| Degraded zone: chromatic rotation | ±3 semitones per mapping, hash of the pitch class |
| Composer: phrase / replan margin / request timeout | 4–8 slots (13-chord vocabulary) / 2 slots / 30 s |
|| Aggro: notice range / stand-down range / wander speed / leash | 8 steps / 10 / 30 u/s / 1.5 tiles |
| Cloak: unlock / sneak per point (floor) / duration per point (cap) / cooldown | 5 pts / −1 step (2) / +1 s (cap 4) / 10 s |
| Corruption: burst interval / burst length / drain / stealth | ~3 s / ~0.3 s / 1 plate per s / −2 steps |
| Combo window | 2 s |
| Shield: active / cooldown / ring | 0.6 s / 3 s (−0.25/pt, floor 1 s) / 1.2 tiles |
| Dash: speed / duration / cooldown / unlock cost | 600 u/s / 0.15 s / 2 s / 2 pts |
| Grenade: speed / fuse / cooldown / blast / damage / bounces / unlock cost | 400 u/s / 0.5 s / 5 s / 2.5 tiles / 200 / 2 / 3 pts |
|| Enemy: count / chaser HP / thrower HP / tank HP / min spawn distance | 40 + 10×depth (cap 80) / 100 / 200 / 400 / 10 BFS steps |
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
