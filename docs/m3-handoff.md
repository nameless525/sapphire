# M3 handoff notes

What M3 inherits from M2, for whoever starts M3. It's a map, as [m2-handoff.md](m2-handoff.md) was: each item gets a line or two and a pointer to where the detail lives, the spec ([engine-spec.md](engine-spec.md)) by section, the plan ([m2-plan.md](m2-plan.md)) by package ID, or the code.

Written at M2's close (Z3, 2026-11-02), on the review line's top, f8fa8f3f, with Z1, WA-3 and five reviews under it. Counts are as of then, and the exit tests' numbers come from 410cf24b ([m2-exit-run.md](m2-exit-run.md), "Rehearsal"). **M3 has no go-ahead:** spec §10 makes M2 the go/no-go gate, and [m2-go-no-go.md](m2-go-no-go.md) is what the maintainer decides from. If the answer is the off-ramp, "What M3 builds on" below is the list of Rust systems that stay.

## Where M2 stands

- **Built, with nothing judged by a person** (spec §10, "M2 status"). Every package but the feel reviews (Z2) has landed or is landing.
- **Reviewed by agents who hadn't built it:** 31 bugs fixed ([m2-go-no-go.md](m2-go-no-go.md), "What the review found"). What they left open is below, under the device checks and the deferred list.
- **Exit test 1 (feel):** nobody has played the fight. The window's fight loop has run on an emulator and a simulator, and never on a desktop. [m2-exit-run.md](m2-exit-run.md) has the sheet for the three reviews.
- **Exit test 2 (iteration):** holds on the desktop, an emulator and a simulator. A phone is owed.
- **Exit test 3 (determinism):** CI enforces it on every pull request, on three OSes.
- **Exit test 4 (budget):** needs a phone. Z1's numbers on an Apple M5 are spec §3.5.

## Device checks still owed

Nothing here has been run, for want of the device. [m2-exit-run.md](m2-exit-run.md) has M2's procedures and [m1-exit-run.md](m1-exit-run.md) has M1's.

### M2's

- **A desktop's window, by hand:** the mouse at the look scale of −0.008, no jump after capture or resume, the cursor's warp and return, focus around Cmd-Tab, minimize and clicking away, a trackpad's fingers and the HUD's zones, the veil, and Windows raw input and X11 and Wayland relative mode ([m2-exit-run.md](m2-exit-run.md), "The pointer and focus"). A live resize and a zero-size window were read, not run; fresh runs passed from 1×1 to 3456×2234 with the window's format.
- **A gamepad on a desktop:** the window's first run on a desktop, `PadRumble` and `stop_rumble` on a real pad, `SDL_RumbleGamepad` on any pad, and whether Zenless Zone Zero's layout, written from memory in [combat_bindings.ron](../crates/sapphire_controls/combat_bindings.ron), is right.
- **Sound, on any speaker:** nobody has heard a cue. Which sound suits which hit, the `step` and `skid` sounds, and levels (plan E3).
- **A phone, touch:** the layout and sizes under thumbs, the stick's feel, press-to-swing latency at 30 fps, the view's drag rate, the speaker, real calls and alarms, and the OS's edge swipes ([m2-exit-run.md](m2-exit-run.md), "What only a phone can show"). iOS has never had a touch: `simctl` can't inject one.
- **A phone, the pause's tap (plausible, not made to fail):** `GestureSettings::tap_slop` is 10 window points ([bindings.rs](../crates/sapphire_input/src/bindings.rs), line 144), and Android's are pixels, about 3.3 dp on a 3× phone, so a tap may read as a drag and leave Back as the only exit. Scale gesture positions by the display's scale.
- **A phone, exit test 4:** Mobile-Low held at 30 fps, thermally throttled, after 15 minutes of combat, on an iPhone 12 and a Snapdragon 865-class phone, with every row of spec §3.5 read there.
- **A phone, exit test 2:** a saved move on the next attempt in under 5 s over live link.
- **A phone, the rest of §3.5's list:** parallel recording's stalls on a phone's core mix, memory on a 4 GB phone, the composite at native output on a tile GPU, the audio callback at real-time priority, a first launch's cold pipeline caches.
- **Audio on a phone:** latency, AAudio's fast path and MMAP, audio focus, real calls and Siri, route changes such as headphones, and iOS's `Ambient` session with the player's music on and the silent switch (spec §5.15). Three plausible gaps in [audio_session.rs](../crates/sapphire_platform/src/audio_session.rs), none made to fail: nothing re-applies `Ambient` after a media services reset, since `prepare` (line 53) runs once; an interruption's `Ended` whose `activate()` fails leaves `INTERRUPTED` true with no retry until the next background trip (`observe_interruptions`, line 92); Android's `prepare` is behind a `Once` (line 132), so a second call after a failed first returns Ok and cpal then panics, which nothing reaches today.
- **The timeline editor's window has never opened** on any OS: drag feel, 8-point handles, tick snapping, layout, 60 Hz playback, real keyboards, Close and Quit ([development-iteration.md](development-iteration.md), "The timeline editor"). It has no cursor icons or clipboard, because `sapphire_platform` has neither and [window.rs](../tools/sapphire_editor/src/window.rs) drops egui's `platform_output`, so Cmd+V won't paste into the address box. Two of the review's fixes there, a lost surface and pending textures at exit, were made by reading. A surface that stays lost now rebuilds the GPU stack on each pass, with no pacing, and egui may log "Detected a linear framebuffer", probably only a warning.
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

1. **Pose it:** a function in [tools/motion/moves.py](../tools/motion/moves.py), and read a sheet with `sheet.py`. `--write` writes `<name>.keypose.ron`. A new key-pose document needs its sidecar by hand, naming `rig`.
2. **Frame it:** a row in [player.moves.ron](../content/combat/player/player.moves.ron), last in `moves`, so no other move's place in the set changes. Startup, active and recovery must add up to the clip's length. Name its intent, cancels, hit volumes on path `blade`, warps and cues. `frames.py` has the same numbers for a first draft; once tuned, the file owns them.
3. **Give it a row in the graph:** [character.animgraph.ron](../content/lookdev/character/character.animgraph.ron), `clips` and `combat.moves`. `every_move_a_fighter_has_is_in_its_graph` fails without it.
4. **Give any new cue a row:** [default_cues.ron](../crates/sapphire_cues/default_cues.ron). `every_cue_the_recordings_publish_has_a_row` fails without it.
5. **Cook and rewrite the simulation's copy:** `cargo xtask content`, `SAPPHIRE_BAKE_MOVES=1 cargo test -p sapphire_pipeline --test player_moves the_simulations_copy`, `cargo xtask replay --update`.
6. **Commit,** then fill in x86-64's checksums from CI's artifact ([development-iteration.md](development-iteration.md), "Filling in x86-64's checksums"). So it costs two CI rounds.

A new `Fighter` or actor field goes in several places, and tests hold them together: `every_field_of_a_fighter_is_in_the_checksum` for one, and spec §5.22 has the rest.

### How to add an enemy

Follow [content/combat/enemy/](../content/combat/enemy/) and [enemy.rs](../apps/sapphire_sandbox/src/enemy.rs):

1. **A mesh with its own skeleton,** in the anime character family. The warrior is generated (`sapphire_pipeline gen-combat`) because the pack's chibi figure couldn't read as a threat under any material (O4).
2. **Clips, through N1's retarget:** add the mesh's ID as a `retarget:` target in the pack's sidecars. Sidecars only, no code.
3. **`<name>.moves.ron`:** `intents` with no buttons, `fighter`, `brain`, `moves`. The warrior's hit volumes are hand-keyed and reach further than its clips' hands, so they want retuning against the pose.
4. **`<name>.animgraph.ron`:** a row for every move and every reaction.
5. **Code, today:** a row in `COOKED_MOVES` (`apps/sapphire_sandbox/src/tuning.rs`), a `SCENARIOS` row in [scenario.rs](../crates/sapphire_gameplay/src/replay/scenario.rs) that loads it and spawns it, and a change to `enemy.rs`, which spawns the one warrior. There's no data-driven encounter.

### How to add a stage

The arena is the one example ([arena.rs](../apps/sapphire_sandbox/src/arena.rs), spec §5.22):

1. **The look:** an assembly (`.assembly.ron`), a day and light rigs. The town stays one mesh until zones.
2. **Colliders:** `<stage>.colliders.ron` with a sidecar. A `block_` prefix is a character blocker and `camera_` a camera blocker, held by a test. Moving a collider means updating `ARENA_WORLD_DIGEST`, `cargo xtask replay --update`, then `cargo xtask content`.
3. **Spawn points:** `<stage>.spawns.ron`, compiled in with `include_str!`, so a replay can't run from another build's points.
4. **A scenario row** naming it, and goldens through `--arena-shot follow` so a camera edit moves none.

Zones replace most of this: a stage can't yet be chosen by data.

### Rules the reviews made

Each has a test holding it, and each binds what M3 adds.

- **Every struct in a move document denies unknown fields** ([document.rs](../crates/sapphire_moves/src/document.rs)), so `cancles:` is refused by name in a save, a push and a cook. A new struct in the format needs `deny_unknown_fields` too.
- **`MAX_MOVE_REACH`** (1,000 m, [set.rs](../crates/sapphire_moves/src/set.rs), line 32): a root key or knockback past it is an issue naming the move. A finite 1e30 m used to overflow the controller.
- **`MIN_SHAPE_EXTENT`** (1 mm, the same file, line 36): a document's shape under what the physics world takes is an issue. A test holds the two limits together.
- **A defeat takes the stick** for good, as a running move does ([movement.rs](../crates/sapphire_gameplay/src/movement.rs), line 61). Anything that walks a fallen character needs its own road.
- **`LiveMoves::start` sets up on the edit** ([tuning.rs](../crates/sapphire_gameplay/src/replay/tuning.rs), line 70): a setup sees edited sets as revision 0, so a brain's first wait and the RNG stream match a build with the edit in its document. A setup can refuse an edit, as a party does a set with no swap, and may not assert what a saved document needn't have.
- **Clocks (WA-3, spec §4.5):** a new clock a frame reads goes `alpha` of the way along its own last step, as transforms do. The section lists what's deliberately on another clock, and `scene::clock_tests` holds the rule through recorded fights.
- **A fight starting over ends what the last one set going** (sparks, screen effects, shakes, rumble, damage numbers), when the fight's clock goes back. A held button stays hidden until it lets go.
- **A live cue table can't name a sound nobody loaded** (`CueTable::check_sounds`, spec §5.29), and a cook that started before a save leaves the save playing where the app has the file.
- **`PointerState`** ([app.rs](../apps/sapphire_sandbox/src/app.rs), line 572) decides who has a desktop's pointer, and `focus_lost` pauses only a played fight, so scripts and replays keep their checksums.

## Everything deferred

### Named by the plan as not in M2

XPBD cloth, ragdolls, Luau, navigation, decals, GPU particles, music, lip-sync, a second enemy type, zones and saves (plan §3). Spec §10 places most of them in M3.

### What M2 left on the way

- **Compile-time iteration:** spec §7.2's `sapphire_dylib` and `subsecond` hot-patching were never built. `cargo xtask run --incremental` makes a code edit's build 1.4–5.5 s, but the app restarts and the fight starts over. Move the numbers below into data, or build hot-patching, before people tune there at length.
- **Numbers that are code:**
  - `ControllerProfile::default()` ([profile.rs](../crates/sapphire_gameplay/src/controller/profile.rs)) through [arena.rs](../crates/sapphire_gameplay/src/arena.rs)'s `profile()`: no document names it, though its struct is ready for one.
  - `SpringTuning::ACTION` ([springs.rs](../crates/sapphire_anim/src/springs.rs)), `IkControls` ([ik.rs](../apps/sapphire_sandbox/src/ik.rs)), `TargetingWeights` ([targeting.rs](../crates/sapphire_gameplay/src/targeting.rs)).
  - The HUD's and touch controls' layout ([hud.rs](../crates/sapphire_ui/src/hud.rs), [touch.rs](../crates/sapphire_ui/src/touch.rs)).
  - `combat_bindings.ron` and `default_effects.ron` are data, compiled in, with no reload route. `VfxWorld::set_library` is the hook; nothing calls it for a fight. `VfxWorld` has no `set_budget`.
- **The enemy's graph has no document route:** `warrior.animgraph.ron` comes through a cook (under 2 s, measured in the exit run's rehearsal), and a push is refused by name. The character's takes a save and a push in a real fight. A live copy like `Content`'s `live_graph` for `enemy_graph`, plus a `Scene` route, is about 40 lines, and a test stage can now stand two fighters in the arena.
- **Sound isn't hot-reloaded:** the mixer holds what it decoded at startup, and a live table naming a sound not loaded then is refused. A sound already playing rings out across a restart, because the mixer can't stop a voice (longest in `content/` is 1.94 s); a voice stop on `AudioBackend` ([backend.rs](../crates/sapphire_audio/src/backend.rs), line 131) would do it. A stream rebuild keeps its state at Running, so cues queue and play in a small burst. A NaN emitter or listener position would poison a bus: no route to one was found, and it isn't hardened.
- **Input, marginal:** an analog trigger held through focus loss reads its first jitter on return as a new press. `read_touch` ([controls.rs](../crates/sapphire_controls/src/controls.rs), line 269) pushes after the pump's events, so a thumb press and Back in one frame apply after the pause.
- **G1:** no per-move `Ground | Flight` field (every move is a `strike`), and no teleport API. Capsules block but don't push. Spec §5.22. A jump pressed in a freeze is lost, because the raw button's edge is read once ([mod.rs](../crates/sapphire_gameplay/src/controller/mod.rs), line 460) and `step_controller` returns early on a frozen clock ([step.rs](../crates/sapphire_gameplay/src/controller/step.rs), line 57). It's rare: a frozen character that isn't in a move. When jump matters, make it an intent; `ControllerDrive::jump` is the seam.
- **D3:** statics are fixed once built, a box can't `move_and_collide`, a document's meshes are inline, and there are no dynamic bodies or triggers. A `parry3d` upgrade re-pins `tests/checksum.rs` and reruns `tests/allocations.rs`.
- **D4:** a target crossing a swing inside a tick is judged by its end pose. A step of several ticks blends a root in a straight line (at 5×, 7.5 cm short across the end of a 7.5 m/s motion). G3's `rearm` and `blocked_by` are authored only in the test moves so far.
- **G3:** no factions, so everyone hits everyone. A defeated actor's capsule stays as an obstacle. The ability system above resolve waits for a fight that needs it (spec §5.22).
- **G6:** one enemy, steering directly with no navigation. Its target is a fixed foe, with no director of several enemies.
- **G7:** no HUD slot for party members (`Simulation::party_ready_in` is what one would read), no touch button for swap, and recolors made at load. A decision from the review: once someone saves away `swap_in`, a running party fight goes on, and the next fight won't start until it's restored, as a build with that document would do.
- **G5:** sequencer takeovers are a pose slot with nothing driving it. Soft lock's `TargetingWeights::TOUCH` exists and nothing uses it: [arena.rs](../apps/sapphire_sandbox/src/arena.rs) always passes `SOFT_LOCK`, which makes no difference with one enemy.
- **N3:** `Jump_Start` and the side dodges cook and aren't played. The arena's two fighters animate in turn on one thread, with no list and no allocation, and a crowd would want `evaluate_all`'s jobs with a retained list ([scene.rs](../apps/sapphire_sandbox/src/scene.rs), `animate_fighters`). `--clip`'s path still allocates a `Vec` a frame. Breathing adds on top of O2's `idle`, which may already breathe: nobody has judged whether that doubles. Two dev-only gaps from the review, in [scene.rs](../apps/sapphire_sandbox/src/scene.rs): a new enemy skeleton or joint count has no path back, because `replace_graph` rebuilds only the player's driver, so the enemy holds a stale pose until relaunch (`driver.fits`, line 1384; nothing panics). And `arena.graph_input` (line 1379) spends `cut` and `landed` before that check skips the fighter.
- **N4:** no get-up clip, so a knockdown rises by a decay alone. Hands on a weapon are tested, and the graph's grip hasn't been looked at.
- **N5:** a clip's root-motion loop teleports with no view cut, which may leave a one-frame velocity streak (unverified). Limits beat colliders, so an arm can pass through a strand by up to 7.2 cm. Springs have no floor, so hair passes through paving. `AnimStats` isn't in the overlay.
- **O2's verdicts, owed to a person:** walk plain and 0.72 m/s at 1× against the controller's 2 m/s; strafes hold the body turned 60°; side dodges stiff; `Jump_Idle` holds its arms out; both idles and deaths written without an animator; the run over-splits its legs.
- **O3's reads, owed to a person:** finisher and skill's coil strongest, dodge weakest, the dash attack's hit needs a trail, `light_2` has weak anticipation. The sleeve opens at the armpit, shorts push through the shirt's back past about 12° of hips pitch, and a black patch shows at the shirt's hem.
- **O4:** the warrior's blade is a mesh placed by hand at `handslot.r`, with no attachment system. Both capsules are the 1.6 × 0.3 m default, so its head and horns stand clear of what stops it.
- **O5 and the look:** looking toward +Z from the square shows only lawn and sky. The kit tree at (13, 5) has a canopy 1.4 m up. The chin shadow doesn't follow a turned head. Interior lines read as dashes at low resolution.
- **Camera framing nobody has judged:** the player out of sight behind a tree after a 3 s push, the enemy filling the frame beside a stall, the enemy between camera and player, a view through the pavilion. The plain follow camera can sit inside a stall late in a fight (only goldens use it).
- **The sword** stands straight up from a fallen hand.
- **Balance:** mashing `light` still wins in 13–15 s with 220–280 of 300 health left (G6). Armour on the swipe is the next lever. Light hits can cut a swipe's wind-up, by design so far.
- **The HUD:** damage numbers keep floating in a pause, on the real clock, as documented. `Scene::show_damage` ([scene.rs](../apps/sapphire_sandbox/src/scene.rs), line 2272) has no tick guard, so a recording stopped at `MAX_TICKS` (4 hours) would show its last step's hits again if that tick landed one.
- **Phone UX:** an iPhone has no Back, so no pause or leave by touch. Nothing auto-pauses on return from the background. A pad's stick up and a finger's drag up go opposite ways. The launcher icon opens the default scene, not the fight.
- **Z1's recommendations:**
  - Decide parallel recording against one encoder on a phone (`PARALLEL_MIN_PASSES`, [executor.rs](../crates/sapphire_render/src/graph/executor.rs)).
  - Give §3.3's post row a screen-effects share.
  - Per-cook texture size in the cooker, because `Body_00`, `Body_00_nml` and `Tops_01` stay over §3.4's 1K a part on the mobile cook.
  - Bring the character toward §3.4's 4 opaque materials (it has 11), or exempt it.
  - Add footprint to the windowed `frame stats` (Z3 added live heap, not footprint).
- **CPU rows move together** by ×2.6–3 between runs on identical code, and nobody has shown why (spec §3.5). `sim_dropped_ms` read 33–199 on rebuilds under load. Paused captures differ on 0.6% of pixels, also unexplained.
- **Smaller code notes:**
  - `apps/sapphire_sandbox/src/scene.rs`: `Scene`'s extract runs two `resource_scope`s a frame (lines 2090 and 2139), which R6 doesn't reach, outside a step.
  - [scenario.rs](../crates/sapphire_gameplay/src/replay/scenario.rs)'s `arena_fight` compiles documents at every start: a fight's restart takes 4.3–6.5 ms and about 6,000 allocations (§3.5). A game would hold its tables.
  - `gpu-tests-warp.yml` doesn't run the app's replay tests.
  - `__pycache__` isn't ignored for `tools/motion`; its tests are written to run with `python3 -B`.
- **`scenes/costs.rs`** wants refreshing from three CI runs on the tree with WA-3 and the review in it (the orchestrator has that).

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
- **More bugs are likely.** The review found 31 past every gate, and nobody can say how many are left. Its fuzz (51,678 swaps and the rest, in [m2-go-no-go.md](m2-go-no-go.md)) ran on arm64 only. The two windows nobody has opened are the likeliest place.
- **No phone has run anything,** after three milestones. Every mobile number is an M5 at the Mobile tiers' settings, the iOS Simulator or the Android emulator. §3's budgets are still proposals.
- **Motion without an animator** (plan §6). The player's moves are solved from poses set by target, the enemy's and locomotion's are retargeted chibi clips, and nobody has said whether they're good enough to judge the fight by. The fallback is a commissioned animator or a bought pack, and a pack that bars redistribution can't go in a public repo.
- **Tools and scenes are single-example:** one character, one enemy, one stage, one set of moves. What's data today has one consumer, so "authored with no Rust changes" is untested.
- **The town doesn't scale as built:** one mesh, drawn whole wherever it shows (M2's handoff has this too).
- **A moved checksum costs two CI rounds,** because no machine here runs x86-64 ([development-iteration.md](development-iteration.md)).
- **GitHub's macOS runners were out** from about 12:30 UTC on 2026-11-02. Every landing since, this one included, was verified on Metal, tests, replays and iOS builds by a local stand-in, not by CI's macOS jobs. Those jobs should run again when runners return.
- **Software rasterizers aren't GPUs,** and the character shader is full: all eight permutation bits and all 16 sampled textures are taken (M2's handoff).
