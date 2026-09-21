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
