# Cinematic mode (marketing shot sequence)

Everything behind `MapType::Cinematic` and the admin `/cinematic` command: a
deterministic stage world plus a server-orchestrated, camera-scripted shot
sequence for recording the website hero video and the Steam page trailer.
Invariant-wise this is ordinary gameplay: the server simulates, clients render
replicated state, and nothing pauses; the camera is the only thing that goes
off-script. See [CLAUDE.md](../CLAUDE.md) for the invariants this leans on.

## The recording workflow

1. Create a world with map type "Cinematic Stage" (create-world dialog).
   Every such world is byte-identical: pinned seed + pinned Small dims, so
   terrain, biomes, ruins, procedural scatter, and the authored stage repeat
   exactly on every fresh world. For multi-angle recording, a dedicated host
   can generate the same stage with `ashwend server --cinematic --world
   <path>` and several clients can film one take together.
2. Enter the world as the host (admin), set up OBS, and type `/cinematic`
   (or `/cinematic play`) in chat whenever ready. `/cinematic stop` aborts.
3. The init phase cleans the world (dropped items, loot bags, projectiles,
   all non-ruin-cache deployables), rebuilds the stage (base compound, lit
   furnace, torches, props), spawns the dummy actors, warps the admin to the
   stage anchor, and freezes the day/night clock.
4. Each shot then runs: countdown slate (5 s, camera parked on the opening
   frame) -> the shot plays (camera flies the authored path, HUD hidden,
   nameplates and damage numbers kept) -> intermission hold (12 s on the
   final frame). Cut in post around the slates.
5. After the last shot the sequence stops itself: actors despawn, the clock
   thaws, controls return.

## Where the pieces live

| Concern | Where |
| --- | --- |
| Shared stage layout (seed, clear zones, authored nodes, base grid, props, actor roster) | `src/cinematic/layout.rs` |
| Shot list, per-shot time-of-day, countdown/intermission timings, camera keyframes | `src/cinematic/script.rs` |
| Camera path sampling (Catmull-Rom over `(t, eye, look)` keys) | `src/cinematic/camera.rs` |
| Worldgen injection (stage zones suppress scatter + regrow; authored spawns appended, trees always alive) | `ChunkManager::new_for_world_with_stage` (`src/server/chunk_manager.rs`), wired by map type in `src/server/lifecycle.rs` |
| Orchestrator: `/cinematic` command, init/cleanup, phase machine, actor choreography, scripted kill, meteor cue | `src/server/cinematic.rs` (state on `GameServer.cinematic`, ticked from `src/server/tick.rs` beside the meteor tick) |
| Wire cues | `ServerMessage::Cinematic(CinematicCue)` (`src/protocol/messages.rs`), reliable channel |
| Client overlay state + input gating | `MenuState.cinematic` (`src/app/state/menu.rs`), gated in `src/app/systems/input/gating.rs - no_blocking_modal` |
| Detached camera + phase clock | `src/app/systems/camera/cinematic.rs` (follow writer stands down in `follow.rs`) |
| Slate UI (countdown / preparing / intermission chip) | `src/app/ui/cinematic.rs`, drawn from `in_game.rs`; HUD/chat force-hidden there while active |

## Dummy actors

Actors are synthetic `ServerClient`s (`synthetic: true`): no transport, no
private-state consumer, excluded from persistence (`world_save` skips them),
removed on stop. The orchestrator writes their controller pose directly
(nonzero velocity while walking is what drives the remote walk cycle), bumps
`swing_seq`/`swing_model` on work cadences (peers edge-detect the swing), and
keeps `last_seen_tick` fresh so the stale sweep never fires. They replicate
through the ordinary player mirror, so rigs, held items, armor, swings, and
nameplates come free; they appear in the player roster, which is deliberate
(multiplayer signal on camera).

Roles: two gatherers (hero-pine chopper, two-node miner), a hammer-swinging
builder at the sticks extension, two arena fighters (the scripted kill lands
10 s into the Skirmish shot through the real `kill_player` chain, loot bag
and corpse included), and a waypoint wanderer crossing the middle distance.

## Determinism notes

- The stage sits inside the ruin scatter's centre exclusion ring (~71 m on
  Small dims), so ruins can never intersect it regardless of the seed math.
- Stage clear zones ride the same footprint gate as ruins, applied to both
  initial generation and regrow (re-appended after `ChunkManager::from_save`
  on reload, see `add_placement_exclusions`).
- Authored stage nodes spawn with `world_seed: None` so the dead-snag noise
  roll never turns a composed tree into a snag.
- The meteor scheduler is suspended during playback (no random shower can
  wander into a take); the Starfall shot forces a single strike at the
  authored impact point, and the scheduler re-rolls on stop.

## Editing the sequence

Shots are data: edit `SHOTS` in `src/cinematic/script.rs` (camera keys are
world-space `(t, eye, look)` rows; the last key's `t` is the shot length) and
the stage tables in `layout.rs`. The wire only carries shot indices, so pure
timing/framing edits need no protocol change. Keep the script sanity test
(`script_is_sane`) and the layout tests green; they pin the cross-references
(meteor/skirmish indices, key ordering, zone coverage).
