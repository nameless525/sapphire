# M3 handoff notes

What M3 inherits from M2, for whoever starts M3. It's a map, as [m2-handoff.md](m2-handoff.md) was: each item gets a line or two and a pointer to where the detail lives, the spec ([engine-spec.md](engine-spec.md)) by section, the plan ([m2-plan.md](m2-plan.md)) by package ID, or the code.

Written at M2's close (Z3, 2026-11-02), on the review line's top, 7a28c828, with Z1, WA-3 and five reviews under it. Counts are as of then, and the exit tests' numbers come from 410cf24b ([m2-exit-run.md](m2-exit-run.md), "Rehearsal"). **M3 has no go-ahead:** spec §10 makes M2 the go/no-go gate, and [m2-go-no-go.md](m2-go-no-go.md) is what the maintainer decides from. If the answer is the off-ramp, "What M3 builds on" below is the list of Rust systems that stay.

## Where M2 stands

- **Built, with nothing judged by a person** (spec §10, "M2 status"). Every package but the feel reviews (Z2) has landed.
- **Reviewed by agents who hadn't built it:** 31 bugs fixed ([m2-go-no-go.md](m2-go-no-go.md), "What the review found"). What they left open is below, under the device checks and the deferred list.
- **Exit test 1 (feel):** nobody has played the fight. The window's fight loop has run on an emulator and a simulator, and, scripted, in an outside reviewer's native Mac window. Nobody has played it on a desktop. [m2-exit-run.md](m2-exit-run.md) has the sheet for the three reviews.
- **Exit test 2 (iteration):** holds on the desktop, an emulator and a simulator. A phone is owed.
- **Exit test 3 (determinism):** CI enforces it on every pull request, on three OSes.
- **Exit test 4 (budget):** needs a phone. Z1's numbers on an Apple M5 are spec §3.5.

## Device checks still owed

Nothing here has been run, for want of the device. [m2-exit-run.md](m2-exit-run.md) has M2's procedures and [m1-exit-run.md](m1-exit-run.md) has M1's.

### M2's

- **A desktop's window, by hand:** the mouse at the look scale of −0.008, no jump after capture or resume, the cursor's warp and return, focus around Cmd-Tab, minimize and clicking away, a trackpad's fingers and the HUD's zones, the veil, and Windows raw input and X11 and Wayland relative mode ([m2-exit-run.md](m2-exit-run.md), "The pointer and focus"). A live resize and a zero-size window were read, not run; fresh runs passed from 1×1 to 3456×2234 with the window's format.
- **A gamepad on a desktop:** the window's first run on a desktop, `PadRumble` and `stop_rumble` on a real pad, `SDL_RumbleGamepad` on any pad, and whether Zenless Zone Zero's layout, written from memory in [combat_bindings.ron](../crates/sapphire_controls/combat_bindings.ron), is right.
- **Sound, on any speaker:** nobody has heard a cue. Which sound suits which hit, the `step` and `skid` sounds, and levels (plan E3).
- **An audio device with no `f32`:** the stream takes the default config's format and converts the mix ([output.rs](../crates/sapphire_audio/src/output.rs)), held only by a fake device. A raw ALSA `hw:` device on Linux is the likely route, and a Windows mix format of integers another: that it opens, and that it sounds right.
- **A phone, touch:** the layout and sizes under thumbs, the stick's feel, a real thumb's drift against the tap slop's 10 points, press-to-swing latency at 30 fps, the view's drag rate, the speaker, real calls and alarms, and the OS's edge swipes ([m2-exit-run.md](m2-exit-run.md), "What only a phone can show"). iOS has had one mouse touch, an outside reviewer's, through Simulator's window: `simctl` can't inject one.
- **A phone, exit test 4:** Mobile-Low held at 30 fps, thermally throttled, after 15 minutes of combat, on an iPhone 12 and a Snapdragon 865-class phone, with every row of spec §3.5 read there.
- **A phone, exit test 2:** a saved move on the next attempt in under 5 s over live link.
- **A phone, the rest of §3.5's list:** parallel recording's stalls on a phone's core mix, memory on a 4 GB phone, the composite at native output on a tile GPU, the audio callback at real-time priority, a first launch's cold pipeline caches.
- **Audio on a phone:** latency, AAudio's fast path and MMAP, audio focus, real calls and Siri, route changes such as headphones, and iOS's `Ambient` session with the player's music on and the silent switch (spec §5.15). A real media services reset and a real refused activation too: [audio_session.rs](../crates/sapphire_platform/src/audio_session.rs) answers both, proved on a simulator with the wipe and the refusals forced.
- **The timeline editor's window has never opened** on any OS: drag feel, 8-point handles, tick snapping, layout, 60 Hz playback, real keyboards, Close and Quit ([development-iteration.md](development-iteration.md), "The timeline editor"). It has no cursor icons or clipboard, because `sapphire_platform` has neither and [window.rs](../tools/sapphire_editor/src/window.rs) drops egui's `platform_output`, so Cmd+V won't paste into the address box. Two of the review's fixes there, a lost surface and pending textures at exit, were made by reading, and so was the way window.rs uses [recovery.rs](../tools/sapphire_editor/src/recovery.rs): with no device it tries again on a growing wait, tested with no window, and closing then saves rather than asks a question nobody could see. egui may log "Detected a linear framebuffer", probably only a warning.
- **`xcrun devicectl device copy from`,** to fetch a recording from an iPhone, is untried.
- **End-to-end latency:** spec §4.2 asks for it in M2. Nothing has measured a press to a change on glass. Pillar 1's "3 ticks" is measured in the simulation (plan G1, O3).
- **A crash nobody cleared:** an emulator died once on its own hang watchdog during a relaunch under load, and six relaunches didn't repeat it (plan I3). The Windows stack fix (`.cargo/config.toml`'s 8 MiB main thread, `sapphire_jobs`' 4 MiB workers) rests on what the dumps showed, not on a proof: three crashes in two CI rounds before it (`lookdev-strike-springs-mark`, `effects-impact-frame`, `lookdev-clip-idle`) and none since.

### M1's, still owed

[m2-handoff.md](m2-handoff.md), "Device checks still owed", has each with its pointer. None was settled in M2:

- Exit test 1 on an iPhone and an Android phone, and test 2's Mobile budgets on phones, throttled.
- iOS frame pacing's ProMotion check (F7) and Android's 90 and 120 Hz check (F8).
- On-tile MSAA on Mali, Adreno and Apple GPUs (R7, Q5), and post on an A14-class phone (P2, P3).
- Whether the performance hunt's 9–13% holds on phones (#114), and a phone's cold start.
- A phone driver's compile time for a pushed shader.
- Q14 (Android pre-rotation), Q9 (device floors), tier selection beyond R6's rules.
- Windows and Linux hardware, Q3 (the Steam Deck), Q4 (Windows' default backend). CI runs those OSes on software rasterizers only.
- Android crash capture (spec §2, **Verify**). Crash upload stays deferred by decision (spec §10, "M0 status").

## What M2 settled from its handoff

These items in [m2-handoff.md](m2-handoff.md) are closed, and don't carry:

- **Q7 and Q8:** our own animation runtime, and `kira` (plan §3).
- **Clips, skeletons as assets, spring bones, constraints** (N1, N2, N5).
- **The combat slice itself:** combat core, party, camera director, one enemy's brain, audio, touch controls (G1–G7, G5, E1, I1–I3).
- **Hit-stop and the finish freeze:** a finish freeze freezes every actor and leaves the world's clock running, so it needs no clock of its own (G3).
- **Fixed-step spawning:** pools sized at creation (D1).
- **Debug-draw API, hitbox and intent views, replay recorder** (T1). The entity inspector stays open.
- **Combat replays on every pull request** (D2).
- **Android Back:** decided in I1. It opens the pause, and leaves from it.
- **`alpha` means two things (WA-3):** fixed. Every clock a frame reads interpolates as transforms do (spec §4.5), and 110 of the 142 Metal goldens moved for it.
- **A second character:** G7's party is recolors of the one character, so Seed-san wasn't needed. A real second character is still M3's call.

## What M3 builds on

### Crates and layering

31 crates in `crates/`, the sandbox app and five tools, each placed in [xtask/layers.toml](../xtask/layers.toml), which `cargo xtask check-layers` enforces.

- **M2's new placements:**
  - L2: `sapphire_moves` (moves' cooked data), `sapphire_colliders` (collision's plain data).
  - L3: `sapphire_anim`, `sapphire_physics`, `sapphire_audio`, `sapphire_vfx`, `sapphire_ui`.
  - L4: `sapphire_gameplay`, `sapphire_camera`.
  - L5: `sapphire_controls`, `sapphire_cues`.
  - Dev only: `sapphire_watch`. Tools: `sapphire_editor`, `sapphire_replay`.
- **`[simulation]`** in the same file lists the simulation's crates and what may never be in their trees (SDL, wgpu, render, world, anim, input, platform). That's why a replay builds fast and runs with no GPU. A new simulation crate joins the list and holds the same `clippy.toml`, byte for byte.
- **`[cooker]`** keeps collision libraries, the renderer and the world out of `sapphire_pipeline`. #74's lesson still holds: if the cooker links WGSL or edited documents, the first cook after an edit goes from under a second to over 20 s.
- **Rule R6 is enforced live:** a fixed step that allocates fails its replay, naming the tick (gate 4).

### Data path

- **New cooked kinds:** skeleton (`.sskel`), clip (`.sclip`), move set (`.smoves`), animation graph (`.sgraph`), collider set (`.scol`), sound and cue. Formats live in `sapphire_assets`, `sapphire_moves` and `sapphire_colliders`, each with a version. Spec §5.10 and §5.27.
- **New sources:** clips from glTF, FBX, VRMA and `.keypose.ron` (N1), with retargeting by humanoid map and the sidecar settings `adjust`, `gain` and `follow` (O2). `.moves.ron`, `.animgraph.ron`, `.colliders.ron`, `.cue.ron`, `.ogg`.
- **Bake once, read tables:** a move's root motion and hit paths are sampled at cook time into per-tick tables, one set of bytes on every architecture (N1 part 3; plan §3). FBX is refused for tables, because `ufbx` evaluates through a C libm.
- **A second pack:** `audio.spack` holds sound and is mounted only by what plays it, the first real group pack (E1).

### The combat stack

From the bottom up. Spec §5.13–§5.18 and §5.22 are the reference, and each crate's `//!` docs say what it owns.

- **Simulation** (`sapphire_gameplay`, D1): `Simulation`, stable `ActorId`s, seeded RNG, pools, a state checksum, the order `CombatSet` chains.
- **Moves** (G2): intents with buffers, cancels at exact sub-ticks, frame-data tracks, root motion and warps, cues.
- **Controller** (G1) on **physics** (`parry3d` directly, D3): kinematic capsules, no dynamics, no EPA.
- **Hits** (D4) and **resolve** (G3): swept volumes, hit groups, damage, poise, armour, reactions, hitstop, finish freeze.
- **Dodges and counters** (G4), **party** (G7), **brain** (G6).
- **Replays** (D2): `.sreplay`, per-architecture checksums, the headless runner.
- **Presentation** (no edge into the simulation): `sapphire_anim` (clips, graphs with inertialization, IK, spring bones), `sapphire_camera` (the director), `sapphire_cues` (one cue to effect, sound, shake, screen effect and rumble), `sapphire_vfx`, `sapphire_audio`, `sapphire_ui` (HUD and touch as quads), `sapphire_controls` (bindings to `TickActions`).
- **The sandbox holds what a game would:** `apps/sapphire_sandbox/src/combat.rs`, `arena.rs`, `enemy.rs`, `blade.rs`, `hud.rs`, `cues.rs`, `party_looks.rs`. There's no `game_app` crate yet. Moving these out is M3's first structural job, since M3's exit test is "a boss authored with no Rust changes".

### Tools

- **Live tuning** (T2): moves, camera and cue documents reach a running fight with no restart. Protocol 4.
- **The timeline editor** (T3): `tools/sapphire_editor`, unopened.
- **Combat views** (T1): `--debug-draw volumes|colliders|combat`, `--debug-overlay=combat`, the overlay's recorder.
- **The fight bench** (Z1): `--bench <frames> --replay`, spec §3.5, `--bench-trace`.
- **Contact sheets** (O2): `--contact-sheet <clip> <png>`, half a second a clip with no window.
- **Motion by target** (O3): `tools/motion/`, plain Python.
- **What still needs a rebuild:** [m2-exit-run.md](m2-exit-run.md), "What changes what", lists it by name. The largest gap is the controller's numbers.

### How to add a move

To the player, on a clip of its own:

1. **Pose it:** a function in [tools/motion/moves.py](../tools/motion/moves.py), added to its `MOVES`, and an ID of its own in `frames.py`'s `IDS`. `--write` writes `<name>.keypose.ron` and its sidecar, naming `rig`. Read a sheet with `sheet.py`, by that ID until `PLAYER_DOCUMENTS` in [lookdev.rs](../apps/sapphire_sandbox/src/lookdev.rs) gives it a name.
2. **Frame it:** a row in [player.moves.ron](../content/combat/player/player.moves.ron), last in `moves`, so no other move's place in the set changes. Startup, active and recovery must add up to the clip's length. Name its intent, cancels, hit volumes on path `blade`, warps and cues. `frames.py` has the same numbers for a first draft; once tuned, the file owns them. Add it to `MOVES` in [player_moves.rs](../tools/sapphire_pipeline/tests/player_moves.rs), or `each_move_plays_the_clip_of_its_name` fails.
3. **Give it a row in the graph:** [character.animgraph.ron](../content/lookdev/character/character.animgraph.ron), `clips` and `combat.moves`. `every_move_a_fighter_has_is_in_its_graph` fails without it.
4. **Give any new cue a row:** [default_cues.ron](../crates/sapphire_cues/default_cues.ron). `every_cue_the_recordings_publish_has_a_row` fails without it.
5. **Cook and rewrite the simulation's copy:** `cargo xtask content`, `SAPPHIRE_BAKE_MOVES=1 cargo test -p sapphire_pipeline --test player_moves the_simulations_copy`, `cargo xtask replay --update`.
6. **Commit,** then fill in x86-64's checksums from CI's artifact ([development-iteration.md](development-iteration.md), "Filling in x86-64's checksums"). So it costs two CI rounds.

A new `Fighter` or actor field goes in several places, and tests hold them together: `every_field_of_a_fighter_is_in_the_checksum` for one, and spec §5.22 has the rest.

### How to add an enemy

Follow [content/combat/enemy/](../content/combat/enemy/) and [enemy.rs](../apps/sapphire_sandbox/src/enemy.rs):

1. **A mesh with its own skeleton,** in the anime character family. The warrior is generated (`sapphire_pipeline gen-combat`, and a new generated file goes in xtask's `COMBAT_SOURCES`) because the pack's chibi figure couldn't read as a threat under any material (O4).
2. **Clips, through N1's retarget:** add the mesh's ID as a `retarget:` target in the pack's sidecars. Sidecars only, no code.
3. **`<name>.moves.ron`:** `intents` with no buttons, `fighter`, `brain`, `moves`. The warrior's hit volumes are hand-keyed and reach further than its clips' hands, so they want retuning against the pose.
4. **`<name>.animgraph.ron`:** a row for every move and every reaction.
5. **Code, today:** a row in `COOKED_MOVES` (`apps/sapphire_sandbox/src/tuning.rs`), a `SCENARIOS` row in [scenario.rs](../crates/sapphire_gameplay/src/replay/scenario.rs) that loads it and spawns it, and a change to `enemy.rs`, which spawns the one warrior. There's no data-driven encounter.

### How to add a stage

The arena is the one example ([arena.rs](../apps/sapphire_sandbox/src/arena.rs), spec §5.22):

1. **The look:** an assembly (`.assembly.ron`), a day and light rigs. The town stays one mesh until zones.
2. **Colliders:** `<stage>.colliders.ron` with a sidecar. A `block_` prefix is a character blocker and `camera_` a camera blocker, held by a test. Moving a collider means updating `ARENA_WORLD_DIGEST`, `cargo xtask replay --update`, then `cargo xtask content`.
3. **Spawn points:** `<stage>.spawns.ron`, compiled in with `include_str!`, so a replay can't run from another build's points.
4. **A scenario row,** and goldens through `--arena-shot follow` so a camera edit moves none.
5. **Code, today:** everything above is compiled in for one stage. `sapphire_gameplay::arena` holds the two documents, each arena scenario's setup reads them there, and the sandbox draws `lookdev::TOWN` on `lookdev::day()` and loads its `arena::COLLIDERS`. A second stage needs a second of each.

Zones replace most of this: a stage can't yet be chosen by data.

### Rules the reviews made

Each has a test holding it, and each binds what M3 adds.

- **Every struct in a move document denies unknown fields** ([document.rs](../crates/sapphire_moves/src/document.rs)), so `cancles:` is refused by name in a save, a push and a cook. A new struct in the format needs `deny_unknown_fields` too.
- **`MAX_MOVE_REACH`** (1,000 m, [set.rs](../crates/sapphire_moves/src/set.rs), line 32): a root key or knockback past it is an issue naming the move. A finite 1e30 m used to overflow the controller.
- **`MIN_SHAPE_EXTENT`** (1 mm, the same file, line 36): a document's shape under what the physics world takes is an issue. A test holds the two limits together.
- **A defeat takes the stick** for good, as a running move does ([movement.rs](../crates/sapphire_gameplay/src/movement.rs), line 61). Anything that walks a fallen character needs its own road.
- **`LiveMoves::start` sets up on the edit** ([tuning.rs](../crates/sapphire_gameplay/src/replay/tuning.rs), line 70): a setup sees edited sets as revision 0, so a brain's first wait and the RNG stream match a build with the edit in its document. A setup can refuse an edit, as a party does a set with no swap, and may not assert what a saved document needn't have.
- **Clocks (WA-3, spec §4.5):** a new clock a frame reads goes `alpha` of the way along its own last step, as transforms do. The section lists what's deliberately on another clock, and `scene::clock_tests` holds the rule through recorded fights.
- **A fight starting over ends what the last one set going** (sparks, screen effects, shakes, rumble, sound, damage numbers), when the fight's clock goes back. A held button stays hidden until it lets go.
- **A live cue table can't name a sound nobody loaded** (`CueTable::check_sounds`, spec §5.29), and a cook that started before a save leaves the save playing where the app has the file.
- **`PointerState`** ([app.rs](../apps/sapphire_sandbox/src/app.rs), line 572) decides who has a desktop's pointer, and `focus_lost` pauses only a played fight, so scripts and replays keep their checksums.

## Everything deferred

### Named by the plan as not in M2

XPBD cloth, ragdolls, Luau, navigation, decals, GPU particles, music, lip-sync, a second enemy type, zones and saves (plan §3). Spec §10 places most of them in M3.

### What M2 left on the way

- **Compile-time iteration:** spec §7.2's `sapphire_dylib` and `subsecond` hot-patching were never built. `cargo xtask run --incremental` makes a code edit's build 1.4–5.5 s, but the app restarts and the fight starts over. Move the numbers below into data, or build hot-patching, before people tune there at length.
- **Numbers that are code:**
  - `SpringTuning::ACTION` ([springs.rs](../crates/sapphire_anim/src/springs.rs)), `IkControls` ([ik.rs](../apps/sapphire_sandbox/src/ik.rs)), `TargetingWeights` ([targeting.rs](../crates/sapphire_gameplay/src/targeting.rs)).
  - The HUD's and touch controls' layout ([hud.rs](../crates/sapphire_ui/src/hud.rs), [touch.rs](../crates/sapphire_ui/src/touch.rs)).
  - `combat_bindings.ron` and `default_effects.ron` are data, compiled in, with no reload route. `VfxWorld::set_library` is the hook; nothing calls it for a fight. `VfxWorld` has no `set_budget`.
- **Sound isn't hot-reloaded:** the mixer holds what it decoded at startup, and a live table naming a sound not loaded then is refused. A stream rebuild keeps its state at Running, so cues queue and play in a small burst. A NaN emitter or listener position would poison a bus: no route to one was found, and it isn't hardened.
- **G1:** no per-move `Ground | Flight` field (every move is a `strike`), and no teleport API. Capsules block but don't push. Spec §5.22.
- **D3:** statics are fixed once built, a box can't `move_and_collide`, a document's meshes are inline, and there are no dynamic bodies or triggers. A `parry3d` upgrade re-pins `tests/checksum.rs` and reruns `tests/allocations.rs`.
- **D4:** a target crossing a swing inside a tick is judged by its end pose. A step of several ticks blends a root in a straight line (at 5×, 7.5 cm short across the end of a 7.5 m/s motion). G3's `rearm` and `blocked_by` are authored only in the test moves so far.
- **G3:** no factions, so everyone hits everyone. A defeated actor's capsule stays as an obstacle. The ability system above resolve waits for a fight that needs it (spec §5.22).
- **G6:** one enemy, steering directly with no navigation. Its target is a fixed foe, with no director of several enemies.
- **G7:** no HUD slot for party members (`Simulation::party_ready_in` is what one would read), no touch button for swap, and recolors made at load. A decision from the review: once someone saves away `swap_in`, a running party fight goes on, and the next fight won't start until it's restored, as a build with that document would do.
- **G5:** sequencer takeovers are a pose slot with nothing driving it. Nothing in a step reads a soft lock's choice: a move warps to the target its scenario gave it, and `choose_target` only tells the director who to frame, by `TargetingWeights::TOUCH` while a thumb is playing ([arena.rs](../apps/sapphire_sandbox/src/arena.rs), `set_played_on_touch`). A second enemy makes the choice matter, and if a step comes to read it, its weights have to come from the recording, not from the device.
- **N3:** `Jump_Start` and the side dodges cook and aren't played. The arena's two fighters animate in turn on one thread, with no list and no allocation, and a crowd would want `evaluate_all`'s jobs with a retained list ([scene.rs](../apps/sapphire_sandbox/src/scene.rs), `animate_fighters`). `--clip`'s path still allocates a `Vec` a frame. Breathing adds on top of O2's `idle`, which may already breathe: nobody has judged whether that doubles.
- **N4:** no get-up clip, so a knockdown rises by a decay alone. Hands on a weapon are tested, and the graph's grip hasn't been looked at.
- **N5:** a clip's root-motion loop teleports with no view cut, which may leave a one-frame velocity streak (unverified). Limits beat colliders, so an arm can pass through a strand by up to 7.2 cm. Springs have no floor, so hair passes through paving. `AnimStats` isn't in the overlay.
- **O2's verdicts, owed to a person:** walk plain and 0.72 m/s at 1× against the controller's 2 m/s; strafes hold the body turned 60°; side dodges stiff; `Jump_Idle` holds its arms out; both idles and deaths written without an animator; the run over-splits its legs.
- **O3's reads, owed to a person:** finisher and skill's coil strongest, dodge weakest, the dash attack's hit needs a trail, `light_2` has weak anticipation. The sleeve opens at the armpit, shorts push through the shirt's back past about 12° of hips pitch, and a black patch shows at the shirt's hem.
- **O4:** the warrior's blade is a mesh placed by hand at `handslot.r`, with no attachment system. Both capsules are 1.6 × 0.3 m, written out in each document's `movement`, so its head and horns stand clear of what stops it until someone tunes that.
- **Movement as data (T2):** each fighter's `movement` is a save away (`sapphire_moves::ControllerProfile`, `MOVEMENT_LIMITS`). What's left: the timeline editor has no rows for it, since it's per fighter and the rows are per move, so it's edited in the text; `IkControls`' test stages in [ik.rs](../apps/sapphire_sandbox/src/ik.rs) repeat the default capsule; and a character meeting a curb's edge in the air is thrown up along it, which `MAX_THROW_REACH` keeps clear of the arena's 6 m walls by limiting the numbers rather than changing the throw, since that would change today's fights. A level with lower walls wants that fixed in the controller.
- **O5 and the look:** looking toward +Z from the square shows only lawn and sky. The kit tree at (13, 5) has a canopy 1.4 m up. The chin shadow doesn't follow a turned head. Interior lines read as dashes at low resolution.
- **Camera framing is counted now, not judged (G5's framing pass, spec §5.22):** `arena::framing` holds whole fights to its bounds, and on tours that push a fight round the square's edge and clutter it took the player's time out of frame from about 12% of frames to 4%, but what's left is real. Up against a stall with the enemy across it every view is poor, and the camera holds a poor one rather than swing (up to 3 s), and enemy-out is the number that rose, from 3.4% to 4.7%. The kit tree's canopy at (13, 5) makes a corridor with the east row where the eye passes through the player's chest for up to 3 s; moving or raising the tree moves the look-dev goldens. A reader who hadn't built it found six bugs in this pass, all fixed, and left one thing: with every view poor it tries again about every 5 s (`clearing.rs`'s `HELD_FOR`, a feel call). Whether any of it feels right is the first thing a person should say. The plain follow camera can sit inside a stall late in a fight (only goldens use it, and fixing it moves them).
- **The sword** stands straight up from a fallen hand.
- **Balance:** mashing `light` still wins in 13–15 s with 220–280 of 300 health left (G6). Armour on the swipe is the next lever. Light hits can cut a swipe's wind-up, by design so far.
- **The HUD:** damage numbers keep floating in a pause, on the real clock, as documented.
- **Phone UX:** an iPhone has no Back, so no pause or leave by touch. Nothing auto-pauses on return from the background. A pad's stick up and a finger's drag up go opposite ways. The launcher icon opens the default scene, not the fight.
- **Z1's recommendations:**
  - Decide parallel recording against one encoder on a phone (`PARALLEL_MIN_PASSES`, [executor.rs](../crates/sapphire_render/src/graph/executor.rs)).
  - Give §3.3's post row a screen-effects share.
  - Per-cook texture size in the cooker, because `Body_00`, `Body_00_nml` and `Tops_01` stay over §3.4's 1K a part on the mobile cook.
  - Bring the character toward §3.4's 4 opaque materials (it has 11), or exempt it.
  - Add footprint to the windowed `frame stats` (Z3 added live heap, not footprint).
- **CPU rows move together** by ×2.6–3 between runs on identical code, and nobody has shown why (spec §3.5). `sim_dropped_ms` read 33–199 on rebuilds under load. Paused captures differ on 0.6% of pixels, also unexplained.
- **Smaller code notes:**
  - [scenario.rs](../crates/sapphire_gameplay/src/replay/scenario.rs)'s `arena_fight` compiles documents at every start: a fight's restart takes 4.3–6.5 ms and about 6,000 allocations (§3.5). A game would hold its tables.
  - `gpu-tests-warp.yml` doesn't run the app's replay tests.

### Carried from M2's handoff, unchanged

[m2-handoff.md](m2-handoff.md) has the detail.

- **Toolchain:** wgpu 30, because 31 isn't out (reassess at this boundary). SDL 3.4.18, pinned exactly. WESL and `encase` were never used.
- **Reflection probes** were to come in early M2 and didn't (spec §5.12, W5's note). SSR for the characters' reflection add.
- **Screen effects M2 didn't author:** cut-ins, which may need the character mask carried through TAA; halftone; C7's screen pattern and SDF reveal.
- **Renderer:** PSO warm-up (M4) growing from `shader_builds`, LOD selection and GPU culling, upscalers, local-light shadows, characters shadowing each other, the performance hunt's deferred costs.
- **The look gaps X1 found:** depth of field, light shafts, water.
- **Assets:** group packs beyond `audio.spack`, memory-mapped packs, mip streaming, a shared cache, a cook-on-save watch mode.
- **Live link:** a pairing token (spec §5.29, "Security"), the tier table and bindings as hot-reloaded documents.
- **Store and signing (M4), crash upload, wind and weather per zone, the entity inspector, nightly cooks and device runs in CI, gate 5 (budgets on a nightly device run).**
- **The M1 reviews' small limits,** none of which M2 touched: RS2-4 (a fade under a nearer surface), PN-5 (panic reports aren't capped within a run), the assets, importers, renderer, post and tools items in [m2-handoff.md](m2-handoff.md), "Known limits and risks".

## Known limits and risks

- **Feel is unknown.** All of M2 could be well built and still not fun. This is the largest risk, and [m2-go-no-go.md](m2-go-no-go.md) is about it.
- **More bugs are likely.** The review found 31 past every gate, a later read of the camera's framing pass found six more, an outside review found 48 more after that, reading those 48 fixes found five, and nobody can say how many are left. Its fuzz (51,678 swaps and the rest, in [m2-go-no-go.md](m2-go-no-go.md)) ran on arm64 only. The two windows nobody has opened are the likeliest place.
- **No phone has run anything,** after three milestones. Every mobile number is an M5 at the Mobile tiers' settings, the iOS Simulator or the Android emulator. §3's budgets are still proposals.
- **Motion without an animator** (plan §6). The player's moves are solved from poses set by target, the enemy's and locomotion's are retargeted chibi clips, and nobody has said whether they're good enough to judge the fight by. The fallback is a commissioned animator or a bought pack, and a pack that bars redistribution can't go in a public repo.
- **Tools and scenes are single-example:** one character, one enemy, one stage, one set of moves. What's data today has one consumer, so "authored with no Rust changes" is untested.
- **The town doesn't scale as built:** one mesh, drawn whole wherever it shows (M2's handoff has this too).
- **A moved checksum costs two CI rounds,** because no machine here runs x86-64 ([development-iteration.md](development-iteration.md)).
- **GitHub's macOS runners were out** from about 12:30 to about 23:20 UTC on 2026-11-02. Landings in that window were verified on Metal, tests, replays and iOS builds by a local stand-in, not by CI's macOS jobs. Landings after it ran CI's own again.
- **Software rasterizers aren't GPUs,** and the character shader is full: all eight permutation bits and all 16 sampled textures are taken (M2's handoff).
