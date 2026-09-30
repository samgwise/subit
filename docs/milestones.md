# Milestones

Shipped history, newest last. The as-built mechanics live in `design.md`;
this file is the changelog of what each milestone added.

## Milestone 1 — Audio infrastructure

Multi-crate workspace (`game`, `wfc`, `audio-voice`) on Bevy 0.19 with an
Ensemble bridge: hub connection from a background tokio thread, a bounded
event channel into the render loop, and a quantised stub voice performing
game events as MIDI notes.

## Milestone 2 — WFC procgen core

The real solver: sockets redefined as complementary edge connectors
(Path/Stop/Wall/Terminal), 16 edge-masked floor variants plus wall and
terminal classes, entropy-driven collapse with restart-on-contradiction and
SplitMix64-derived attempt seeds, and BFS reachability guaranteeing a
spawn-to-exit route. ASCII map example for eyeballing.

## Milestone 3 — Bevy integration

The generated grid materialised as a tilemap with a procedural flat-colour
atlas, one compound static collider over all wall tiles, and a physics-driven
player with camera follow.

## Milestone 4 — Combat and telemetry

Enemy swarm spawned on BFS-reachable tiles with seek AI, mouse-aimed cleave
combat with combo tracking, contact damage with an invulnerability window and
respawn. Bridge event surface overhauled to the GDD table (per-type
addresses, event/stream/param semantics); audio-voice maps player speed to
mod-wheel CC.

## Milestone 5 — World legibility pass

Truth-in-feedback (cone-shaped cleave flash, magenta exit with a pulsing
beacon, HP bar, invulnerability blink, hit flash), an 18-tile procedural
atlas (grid-lined floor, wall autotiling with lit edges, corridor-versus-arena
shading), an off-screen exit arrow, and an integrity-paired floor tint.

## Milestone 6 — Thrower enemy, bouncing projectile and shield reflect

First enemy-diversity pass: slower throwers holding at range with lobbed
projectiles (three wall bounces then fail), and a right-mouse shield that
blocks contact damage and reflects projectiles along the cursor with a fresh
bounce budget — physically flipping allegiance via collision layers. Also
fixed combo chains comparing against raw timestamps instead of kill spacing.

## Milestone 7 — Progression, drops and the skills menu

Enemy health and a shared death path; XP shards and HP-cross drops with
magnet pickup; a linear XP curve granting skill points; a Tab-paused skills
menu (cleave damage, shield cooldown, dash and grenade unlocks); Space dash
with i-frames; and a wall-bouncing fused grenade with a radial blast.

## Milestone 8 — Depth descent

Reaching the magenta exit descends to the next depth: the world clears and
regenerates from a depth-derived seed while all progression (HP, XP, level,
points, unlocks) carries forward. Depth scales the mob (+10 enemies per
layer, capped; throwers tighten to every 3rd spawn from depth 2) and a
descent tone plays through the audio chain.

## Milestone 9 — Rendering fidelity and readability

Fixed the atlas writing its data column-major — every tile rendered a
transposed grey smear, making floors and walls indistinguishable — with a
row-major rewrite and a regression test. Added a 2× camera zoom, larger
projectile/pickup sprites with an alpha pulse, amber throwers distinct from
red chasers, and an enemy damage tint darkening with HP fraction.

## Milestone 10 — Shields, tanks and health upgrades

Second enemy-diversity pass plus defensive depth for the player. Enemies can
spawn with regenerating shields — eight plates of 5 HP, one plate regrown per
2.5 s of quiet, depth-scaled spawn chance, cyan ring while plates remain —
that soak cleaves, reflected shots and grenade blasts through one shared
absorption helper. Slow red-brown tanks (400 HP, 0.9-tile bodies) join the
mob from depth 1 via shuffled role quotas. Two new skills: the barrier unlock
(a player-side plate pool with its own violet HUD bar, soaking damage before
health) and repeatable max health +25 that heals as it raises the ceiling.

## Milestone 11 — Animated neon tilemap shader

The tilemap renders through a custom bevy_ecs_tilemap material whose WGSL
fragment shader pumps the baked lit wall strips and terminals with a
gpu-time sine pulse — a shimmer that travels along each edge while floors
and wall bodies stay matte — composing with the camera bloom into a
breathing Tron glow. Shader iteration is fast: a `shader_preview` example
renders every atlas variant in isolation with the real material and bloom,
and Bevy's file watcher hot-reloads the WGSL live (about a second from save
to screen; broken shaders log an error and keep the last good look).
Supporting refactor: the game crate split into a library plus a thin binary
so examples (and future integration tests) reuse the real modules, with
asset paths anchored at the workspace root.

## Milestone 12 — Shield energy domes

The gizmo rings around shields became shader-drawn energy domes: a custom
2D material on a circle mesh renders a translucent hex-faceted bubble whose
rim is divided into eight arcs — one per shield plate — so cracked plates
show as dark segments and regrowth relights them; a hit flashes the dome
off the regen clock. The player's reflect ring wears the same cyan dome,
and the barrier gains a violet one (a little wider, hidden until owned and
charged). All animation runs on the GPU from globals time; the CPU side
pushes uniforms only when dome state actually changes. The shader previews
in the same isolated window as the tilemap neon and hot-reloads live.

## Milestone 13 — Skill expansion

Every combat tool gained upgrade paths, and the menu learned to show
levels: rows render their level out of a cap and their current cost, and
grey out when maxed, owned or gated behind their ability (the dash and
grenade cooldown skills need the ability first). The cleave starts at a
narrower 60° cone; its new reach skill extends it — +0.25 tiles and +5° of
arc per point (capped at 4). The shield gained a duration skill (+0.15 s,
cap 4) beside the existing cooldown skill, the dash a cooldown skill
(−0.25 s, cap 4, floor 1 s), and the grenade a damage skill (+50, cap 6).
Two expensive unlocks joined the panel: nova (4 points) — a 360° burst
that damages through shields and shoves survivors aside, with an expanding
ring and its own audio event — and deflect volley (5 points) — every caught
projectile is reflected alongside two extra auto-aimed shots at the
nearest enemies, fanning around the reflection when the swarm is
elsewhere.

## Milestone 14 — Enemy pathing

Corner-stuck enemies fixed two ways. Colliders went from boxes to circles
(same footprints), so agents glide around tile vertices instead of
snagging on them; and a BFS flow field (`wfc::FlowField`, built over the
existing reachability BFS) steers blocked enemies around bends — direct
pursuit when the sight line is clear, field descent when it isn't. The
field targets the player's tile and rebuilds only on tile changes or depth
descent; enemy reads are O(1), and knockback composes additively with the
new steering.

## Milestone 15 — Aggro and wandering

The fully competent swarm stopped converging from the opening second:
enemies notice the player within 8 path steps (BFS distance around the
walls — enemies behind a wall stay dormant no matter how close they
stand) and stand down once the player escapes 10 — a hysteresis band so
nobody flickers at the boundary — while un-alerted enemies mill slowly
around home on a short leash, never pursuing or opening fire. Depths now
open calm and the mob engages as the player reaches it, and standing down
re-homes the wanderer where it lost the player.

## Milestone 17 — Corrupted data zones

The green terminal patches became corrupted data zones. Clustered
terminals seed patches (the cluster dilated a tile, walls excluded)
whose tiles glitch — a hash-scheduled burst every few seconds shuffles,
drops and tears the base tiles' pixels via a zone-mask texture read in
the neon shader, while a sparse overlay tilemap flickers green static
and data rain above them, everything on one shared burst clock. Every
shield standing in a zone bleeds a plate per second (enemy shields and
the player's barrier — luring a tank through the corruption strips it),
the player sheds two aggro notice steps inside a zone (stacking with the
cloak), and crossing a zone edge fires corruption sonic triggers through
the bridge.

## Milestone 16 — Cloak

A stealth ability in the reflector's shape: the cloak unlock (5 points,
key C) shrinks the enemy notice range to six path steps while it is up,
the sneak skill tightens it by a step per point (cap 4 — two at max) and
the duration skill adds a second per point (cap 4, from three seconds);
the cooldown is a flat ten. The cloak works both ways — un-alerted
enemies don't notice you until you're inside the sneak range, and alerted
ones stand down once you escape it, re-homing where they lost you.
Attacking (cleave, nova, grenade) breaks the cloak; dashing doesn't. The
player's sprite dims to a slow shimmer while cloaked, and a soft cloak
event plays through the audio chain.
