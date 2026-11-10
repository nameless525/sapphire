# M4 handoff notes

What M4 inherits from M3, for whoever starts M4. Like [m3-handoff.md](m3-handoff.md), it's a map. Each item gets a line or two and a pointer to where the detail lives: the spec ([engine-spec.md](engine-spec.md)) by section, the plan ([m3-plan.md](m3-plan.md)) by package ID, the runbook ([m3-exit-run.md](m3-exit-run.md)), or the code.

**Written on 2026-11-10** on top of all of M3's packages (Z4, in three parts). The final exit run's head, its numbers and its date are Z2's, in [m3-exit-run.md](m3-exit-run.md).

**The plan M4 builds from is [m4-plan.md](m4-plan.md).** Its §3 says which of the deferred items below it takes and which wait.

**Nobody has played anything.** The maintainer started M3 on 2026-11-03 without having played M2's fight, so review 3 from [m2-exit-run.md](m2-exit-run.md) was never run. Nobody has played M3's slice either. Whether any of it is fun is unknown, and that is still the largest risk in the project.

## Where M3 stands

M3 built a small game, `game_app`. It has a title with a new game and a continue, and the Lantern Ward, with fights in three rooms and a boss on the terrace. It has the court's gate cutscene and a guide's conversation. It has checkpoints and a save, a pause, settings and a character screen, in English and Japanese, on a pad, a keyboard and touch. Its flow is a document, `content/game.flow.ron`, and almost everything else is under `content/` too. The exceptions are the "Rust steps" in "How to add".

| Exit test ([m3-plan.md](m3-plan.md) §1) | Where it stands |
|---|---|
| 1. A zone with three encounters, walked to its boss | Replays of the ward's walk and its fights hold on all three desktop OSes, with no allocation in a step. The game's flow (G1) plays the ward with every encounter live, and the terrace's clearing wins. The whole walk through the flow, from a new game to the Knell Colossus's defeat with the gate watched and the guide answered, is `ward-whole-walk` (G1c): `game_flow`'s `the_whole_walk_replays_to_the_win` in every OS's tests, and `game_app`'s `wins_the_ward` on the stage in CI's App tests. `plays_the_ward` holds a continue at the court, the gate skipped, the guide's choice, back to the title, and a continue that replays neither. |
| 2. A boss with no Rust changes | Holds. `crates/sapphire_gameplay/tests/bosses_are_content.rs` finds the Knell Colossus by its being a boss. Its name is in no `.rs` file, its files and script are reached from the zone's document, and the commit that put it in the zone touched only `content/`, `tests/replays/` and `tests/golden/`. CI's Linux leg reads the whole history for that last clause. A second boss was added by the recipe below with no Rust. |
| 3. One cutscene | The court encounter's start plays the gate through a flow beat, aligned to the court's checkpoint, and the guide's conversation follows. A watch and a skip reach one checksum (`ward-court-watched`, `ward-court-skipped`). The HUD draws the subtitles in the chosen language, held by goldens at three aspects in both (`game-ward-gate-*`, M3 Q4). |
| 4. HUD and menus in two languages | The menus are walked by pad, keyboard and touch at three aspects in both languages, with goldens. A real run has a title with continue and new game, a pause with a way back to the title, and a HUD with a cutscene's skip and banners (G1). The goldens `game-ward-title` and `game-ward-checkpoint` draw the ward's title and a continue from its court. The Japanese is machine-quality, and a reader is owed. |
| 5. Five platforms | Done, pending devices, as M1 and M2 closed. Linux and Windows run on CI's software rasterizers, and macOS on Metal. The iOS Simulator played a scripted fight to this Mac's checksum. The Android emulator plays committed replays offscreen (Z2b): `zone-trip`, `ward-court-watched` and `arena-fight-won` reached their checksums with no allocation in a step ([m3-exit-run.md](m3-exit-run.md)). No phone has run anything. |
| 6. The editor | Proved by tests with no window. The window is owed to a person. |
| 7. The inventory | 0 first-use pipeline builds after load on Desktop-Low and Mobile-Low, in every app lane and bench, `wins_the_ward`'s whole walk through the flow among them. |
| 8. Budgets and durability | Saves survive a kill at every step of a write, and old saves migrate. The game's own save types have their version 1 bytes frozen (Z3c). On the zone bench (Z1, spec §3.6), §3.2's script and streaming rows are inside budget on every tier. Memory holds flat across the walk: the asset server at 59.6 MB on desktop and 47.7 MB on mobile, and the process at 404–775 MB by tier. A trip's wipe is gone 69 frames after its exit on Desktop-Low (425–497 ms), and 37 on Mobile-Low. Z1's fixes read the GPU rows on Metal again, took the allocation out of a trip's step, and scaled the springs' clearance pass by tier, so secondary motion's p50 is inside its rows on Desktop-Low and Mobile-Low; its p99 is still a solve frame's, and Desktop-Low's mean is over (see "Known to the run"). |

**The reviews (Z3a–Z3f)** gave six areas a fresh read after their packages landed, the way M2's did:

- the renderer and UI, and the editor and live link;
- the simulation, scripts and saves, and content, assets and streaming;
- the platform, input, audio, animation and strings, and the game's flow, the game and the sandbox.

Together they landed 33 commits of fixes and new holds, and G1 landed a 34th for Z3c's finding about a zone a trip leaves. "Rules the reviews made" has what binds now. What each review saw but left is under "Everything deferred".

## Device checks still owed

Nothing here has been run, for want of a device or a person. [m3-exit-run.md](m3-exit-run.md) has M3's procedures, and [m2-exit-run.md](m2-exit-run.md) and [m1-exit-run.md](m1-exit-run.md) have the older ones.

**M3's:**

- **Review 3, and the same for M3's slice.** Thirty minutes with a gamepad on [m2-exit-run.md](m2-exit-run.md)'s sheet. Then the Lancer and the boss, three enemies in a room with tokens, the boss's second wind, the ultimate and its meter, and the ward from the title to the boss.
- **Motion nobody has judged:** the Lancer's and the Knell Colossus's key-posed and retargeted clips (C2 part 1), the gate cutscene's poses (Q3) and the ultimate's chop (Q3b).
- **A reader of Japanese** for every string in `content/loc/ja/` (C3, and the bosses' names from Z3e). Both files' resource comments say they're machine-quality.
- **Anything heard.** Nobody has heard A1's music or voice lines, which are placeholders `tools/sapphire_soundgen` synthesizes, or any of M2's cues. On the emulator, audio says once that the device refused the low-latency path, which an emulator always does (Z2c), and nobody has listened.
- **A window on this Mac, or any desktop,** since agents may not open one here. The game's first run in a window. The editor (`cargo xtask editor`): docking by hand, a real drag on the gizmo, keyboard focus, and a lost device (E3, E4, E5). A cutscene's blend, whose camera numbers are bounded but which nobody has watched.
- **Exit test 5's devices:** a phone of each OS, and a desktop GPU other than this Mac's. Windows and Linux have drawn only on CI's software rasterizers.
- **A phone.** Only sizes were measured on the Android and iOS builds. Nothing of Luau, `parley` text, ICU plurals, zones and streaming, or the 3D inset has run on one. Neither have saves on `SDL_GetPrefPath`'s real paths, a phone's flash or a power cut, nor a phone driver's cold compile of the packs' pipelines, which blocks a load for 6–7 s on an M5 (K1). Mobile-Low's numbers come from this Mac at Mobile-Low's settings.
- **Touch on iOS.** `simctl` can't touch, so iOS touch is unproved. The Android emulator took real taps (S2).
- **x86-64 Android** links Luau and has never run it (L1).
- **Windows' save retry** (Z3e): a save waits about 310 ms for a file an indexer or scanner holds (`RENAME_WAITS` in `crates/sapphire_platform/src/durable.rs`). Its test runs only on Windows, so its first proof is CI's Windows lane.

**M2's, carried** ([m3-handoff.md](m3-handoff.md), "Device checks still owed", has each with its pointer; M3 settled none): a desktop window's pointer and focus, by hand; a gamepad on a desktop, rumble and the pad layout; sound on any speaker, and an audio device with no `f32` format; a phone's touch, exit test 4's budgets (Mobile-Low held at 30 fps, throttled, after 15 minutes), exit test 2 over live link, the rest of §3.5's list and audio sessions; the timeline editor's window, now a panel in `sapphire_editor`'s shell; `xcrun devicectl device copy from`; end-to-end latency on glass; the emulator crash nobody cleared, and the Windows stack fix that rests on dumps.

**M1's, carried** ([m2-handoff.md](m2-handoff.md), "Device checks still owed", has each; none was settled in M2 or M3): exit tests on an iPhone and an Android phone, and a phone's cold start; ProMotion and 90/120 Hz pacing (F7, F8); on-tile MSAA (R7, Q5), and post on an A14-class phone; the performance hunt's numbers on phones, and a phone driver's shader compile time; Q14 (Android pre-rotation), Q9 (device floors) and tier selection; Windows and Linux hardware, Q3 (the Steam Deck) and Q4 (Windows' default backend); Android crash capture.

## What M3 settled from its handoff

These items in [m3-handoff.md](m3-handoff.md) are closed and don't carry:

- **Not in M2, now built:** Luau (L0–L3), navigation (P0–P3), music (A1), a second enemy type and a boss (B3, C2), zones (W1–W5), saves (V1) and a game's flow (G1). XPBD cloth, ragdolls, decals, GPU particles and lip-sync stay out (spec §10).
- **One stage, compiled in:** zones are documents (W1), found by name with `--zone <name>`. Scenarios are documents (B2), compiled in by a build script with no Rust edit, and they list the timelines, dialogues and shelved zones a run loads (G1).
- **The town as one mesh:** cut into cells that stream (W2), with memory given back (W3).
- **G6 and G3, one enemy, no director, everyone hits everyone:** several enemies with factions and soft-lock choice, and a fallen capsule in no group (B1). Encounters with waves and tokens (B2). Paths, separation and patrols (P2, P3).
- **G1's missing teleport and the curb's throw, G5's empty takeover slot, and O4's hand-placed blade:** `Simulation::teleport` and checkpoints (B4), with the throw fixed in the controller. Timelines drive the camera's takeover (Q1). A kind's look holds a mesh at a joint, or nothing when the weapon is skinned in (B3, B3b).
- **Numbers that were code:** springs, IK controls, targeting's weights and the HUD's layout are documents. Bindings and effects reload through live tuning (S3).
- **Single-example tools and scenes:** three kinds of enemy plus the captain, six zones, and recipes walked on new ones.
- **PSO warm-up's first half:** a pack lists its pipelines and a load builds them, with a first-use count that fails a bench (K1). The blocking screen and a driver cache are M4's. **Group packs beyond `audio.spack`:** one pack per zone, plus `fonts`, `loc` and `ui`.

## What M4 builds on

### Crates and layering

There are 46 crates in `crates/`, four in `game/`, the sandbox and the tools. Each is placed in [xtask/layers.toml](../xtask/layers.toml), which `cargo xtask check-layers` enforces. Each crate's `//!` doc says what it owns, and [docs/adr/](adr/) has the reasons. M3's new crates:

- **L2, plain data:** `sapphire_zone`, `sapphire_navmesh`, `sapphire_timeline`, `sapphire_dialogue`, `sapphire_strings`, `sapphire_loc`, and `sapphire_bytecode`, a Luau compile and scan the cooker can link.
- **L3:** `sapphire_text` (`parley`, `swash`, the glyph atlas), and `sapphire_body_pose`, the CPU pose evaluator that presentation and the simulation's bodies share.
- **Simulation and above:** `sapphire_nav`, `sapphire_script` (Luau), `sapphire_sequencer`, `sapphire_save` (L4), and `sapphire_stage` (L5). The stage is the one road from a simulation and cooked content to a `FramePacket`, shared by the sandbox, the game and the editor's viewport. What only the sandbox's sample scenes build is behind its `samples` feature, which a game leaves off (Z5d).
- **L6, the game:** `game_combat` (a fight's app side), `game_ui` (HUD, touch, menus and the dialogue panel), `game_flow` (the title-to-victory machine and the save, spec §5.30) and `game_app`.
- **Tools and dev:** `sapphire_document` (the editor's change records), `sapphire_soundgen`, and `sapphire_synthetic_zone` (dev only).

**`[simulation]`** lists twelve crates, whose trees may hold no SDL, wgpu, renderer, world, anim, input or platform code. A new simulation crate joins the list and holds the same `clippy.toml`. **`[cooker]`** keeps the renderer and world out of `sapphire_pipeline`, which is why `sapphire_bytecode` exists.

**Kinds and versions follow the landing order.** `AssetKind` numbers are contiguous, up to `VoiceLines` 25. `MoveSet` is version 9, `Zone` 5, and `Progress` and `FlowDocument` 1. The replay format is 4, written only for a run with a zone start (G1). `CHECKSUM_LAYOUT` is 9 (S3b). A new kind or version takes the next number at its final rebase. A pack encoding's cache key holds the kind's number, so a renumbered kind re-encodes on its own (Z3d).

### Data path

- **New cooked kinds** (spec §5.10): zone (`.szone`) and its cells' meshes, navmesh (`.snav`), encounter (`.sencounter`), timeline, Luau script (`.sscript`), dialogue (`.sdialogue`), font (`.sfont`) and font chains (`.sfontc`), locale (`.sloc`), UI screen (`.sui`), kind of enemy (`.senemy`), `Score` and `VoiceLines`.
- **Packs:** one per zone, `zone_<name>.spack`, holding what only that zone reaches (W1), plus `fonts`, `loc` and `ui`. A pack's table lists each submesh's pipeline key (K1) and the names of clips and move sets (S3).
- **What the simulation reads is pinned:** a zone's digest and its encounters' tables, each script's bytes, each timeline's and dialogue's simulation half, and each kind's mind. All go into `tables_hash`, so a replay can't run on other data. Zones load whole before the first step, and only presentation streams (plan §3, "The simulation streams nothing").
- **Compiled in,** so an edit needs a rebuild: scenario documents, by `crates/sapphire_gameplay/build.rs`, which also embeds every `.enemy.ron` under `content/combat/` with the moves and script it names, and every `.timeline.ron` beside a moves document, a cinematic its moves may name (Z5a); and the cue table, `crates/sapphire_cues/default_cues.ron`.

### The combat stack

From the bottom up, on M2's ([m3-handoff.md](m3-handoff.md)). Spec §5.19 and §5.21–§5.24 are the reference.

- **Factions, soft lock and fallen capsules** (B1). **Encounters** (B2): triggers fire events, waves come by `Start`, `Remaining(n)` or `After(ticks)`, and tokens gate who swings.
- **Kinds of enemy** (B3): a kind's mind (brains by name, phases in thousandths of health) is the simulation's half, and its look is the stage's. A boss has a bar, named phases and a name in each language.
- **Navigation** (P1–P3): a flat graph per zone, A* on a preallocated pool, a repath budget per tick, line of sight, separation and patrols.
- **Scripts** (L1–L3, C2): one Luau VM a fight, with event hooks only, where each kind's script hears only its own fighters. Commands go into a fixed queue applied at one point in the step, beside a hashed board of 64 numbers. Hot reload happens between steps.
- **Timelines** (Q1, Q3, Q3b): a cutscene holds the simulation at a step's edge, and a skip is recorded. A move may name a cinematic, and costs from a fighter's meter (Q3c).
- **Dialogue and flags** (Q2, Q2b): 256 `i32` flags, conditions asked of the VM between steps, and a choice's effect recorded.
- **Checkpoints and a run's start** (B4, G1): a save holds a `ZoneStart`, a replay of a continued run holds it too (format 4), and `Simulation::start_at` restores it. A zone a trip leaves goes back on its shelf with a running encounter started over (`SimZone::left`).

### The game's flow

`game_flow` (G1, with Z3f's fixes; spec §5.30) holds where the game is, from the title through a run's beats to a defeat or a win:

- **Beats** fire once a run, each `when` an encounter starts or clears, a timeline ends or a checkpoint is reached in the flow's zone. Each `then` plays a timeline, opens a dialogue or wins, `unless` a flag is set.
- **Runs:** each is a recording in memory, kept as a file only if asked. The flow acts only between steps, through recorded commands, so a run replays with no flow.
- **Saves:** `Progress` version 1 holds the flow's zone, its `ZoneStart` and the story's flags, written off the frame whenever the run moves on in that zone.
- **On a stage:** `GameShow` stands the fight, its HUD and the menus. `game_flow::capture` draws it with no window, for `game_app --capture` and the sandbox's `--game title|new-game|continue [--progress <save.ron>]`. G1 decided two things for the maintainer: the guide's dialogue names no trigger (the flow opens it after the gate), and voice in a conversation is M4's.

### World and streaming

- **`sapphire_world::streaming`** (W2) activates cells by distance within a budget of N entities or M µs a frame. A hold (`Streamer::hold`) freezes it while an encounter runs, and takes only once the zone has caught up, so a fight that starts at once still gets its ground.
- **Memory comes back** (W3): `AssetServer::unmount`, `Renderer::free_mesh` and `free_material` after the frames that read them, and generational handles.
- **Trips between zones** (W4): a zone's `exits`, a wipe that hides the load, and a prefetch from a hub. The simulation enters the next zone between steps, at a recorded tick, with its triggers, encounters and checkpoints.
- **Encounter volumes** (W5): spares of each kind stand before a trigger fires, so a wave's arrival builds nothing.
- **The pipeline inventory** (K1): `PipelineKey` per material slot, `Renderer::prewarm`, and `PipelineBuilds` counting every build. A bench fails on a first use.

### Tools

- **The editor** (E1–E5): `tools/sapphire_editor` is a docking shell over `sapphire_document`. Every edit is a change record that splices the fewest bytes, and undo returns the same bytes. It has the moves timeline, the real renderer in a viewport, play-in-editor from a snapshot, and zone, sequencer and screen panes. Nobody has opened its window.
- **Live tuning and live link** (protocol 6) carry moves, camera, cues, the S3 documents, screens, and a `.luau` reloaded between steps (`link push`, `link watch`). A tool that stops reading stalls only its own connection (Z3b).
- **Benches and lanes:**
  - `--bench` for a fight or a zone walk (`--arena --replay tests/replays/walk-the-ward.sreplay`, with `--stream <entities>:<radius>`), logging memory and each trip's time (spec §3.6);
  - the sandbox's `--ignored` replay lane;
  - `game_app`'s lanes `reaches_the_fight`, `zone_trip`, `opens_the_guide`, `plays_the_ward` and `wins_the_ward`, each `cargo test -p game_app --features dev --test <name> -- --ignored`;
  - the whole walk's recorder, `cargo test -p game_flow --test ward_walk -- --ignored`, with `SAPPHIRE_WALK_OUT=<file>` to write the replay again when the fight changes under it.
- **Look-dev for a new figure:** `--look-dev enemy --model <mesh>[:<clip>[@<tick>]]`. **Generators:** `sapphire_pipeline gen-figures --specs tools/figures --out content` for figures from specs, `tools/motion/` for key poses in plain Python, and `tools/sapphire_soundgen` for placeholder sound.

## How to add

M3's first pillar is that a new enemy, encounter, stage, boss, cutscene or screen is documents ([m3-plan.md](m3-plan.md) §2). The recipes below say where that holds. **A step that still edits Rust is marked "Rust step", and each is listed under "Everything deferred" as a bug against the pillar.**

Every recipe ends the same way: `cargo xtask content` to cook, `cargo xtask replay` for the replays, the sandbox's replay lane for the app, and a commit. Then come x86-64's checksums from CI's artifact for any replay that's new or moved ([development-iteration.md](development-iteration.md), "Filling in x86-64's checksums"), which costs two CI rounds.

**What's left of Rust after Z5e:** a recipe's only Rust steps are a golden's row in `scenes.rs`, a replay's new scripted input in `script.rs`, a zone a scenario shelves in `BAKED_ZONES`, a placeholder sound, and a screen showing a value no model holds. The first three are test lists and a build-time table, which a commit of documents alone can do without: a golden or a standalone replay is a check someone chooses to add, and the shelf is for replays of a trip, which the game's own trips don't need. No test pins the shipped Lantern Ward's content any more (M3 Z5e): the tests that held its counts and lists read them from its documents, so a checkpoint, an encounter's fighter, a cutscene or a conversation added to the ward or its run passes them with only `content/` changed. Such a change still moves the ward's replays, and a fight that plays differently needs `ward-whole-walk` recorded again by its recorder (see "Tools"), which is data.

**Walked for real.** The enemy-kind and boss recipes were followed end to end on a scratch branch on 2026-11-09, with no Rust edit:

- a new kind on the Lancer's figure with moves and two brains of its own, in the ward's hub;
- a new boss on the Knell Colossus's figure at four fifths, with three phases and a script of its own, in the proving ground.

Both cooked and recorded replays. Both played beside the renderer to their checksums with no first-use build, and the captures showed each kind drawn and fighting. A one-off test read the replays: the kind changed brain at its phase, the boss's script changed its brain and forced its move on the crossing step, and the boss's document changed its brain at its next phase. `bosses_are_content.rs` held the new boss to exit test 2. Then the branch was deleted. What the walk found is in the recipes, or fixed (see "Fixed in M3").

### A move

For the player, on a clip of its own ([m3-handoff.md](m3-handoff.md) has M2's version):

1. **Pose it.** Add a function in [tools/motion/moves.py](../tools/motion/moves.py) to its `MOVES`, and give it an ID in `frames.py`'s `IDS`. `--write` writes `<name>.keypose.ron` and its sidecar. `--write-numbers` puts `frames.py`'s draft frame numbers into the moves document. Read a sheet with `sheet.py`.
2. **Frame it.** Add a row to [player.moves.ron](../content/combat/player/player.moves.ron), last in `moves`, with `name`; `startup`, `active` and `recovery`, adding up to the clip's length; `motion`; `hitboxes` on path `blade`; `cancels`, `warps` and `cues`; and optionally `invulnerable`.

   A row has no intent. A press reaches a move through the top-level `intents` and `neutral`, or through another move's `cancels`. M3 added two optional fields: `cost`, against the fighter's `energy` (Q3c), and `cinematic`, naming a timeline (Q3b): `<name>.timeline.ron` beside the moves document, on the `Steps` clock. Every fight on the set loads it by that name (Z5a), and a name with no timeline there fails the scenario, naming the move.
3. **Its tests find it** (Z5e): [player_moves.rs](../tools/sapphire_pipeline/tests/player_moves.rs) reads the moves with a clip of their own from `player.moves.ron` (a `clip`, and `<name>.keypose.ron` beside the document) and the attacks from those with `hitboxes`. `each_move_plays_the_clip_of_its_name` holds every row: a move plays the key-pose document of its name, an assist the one its `source` names, and only a reaction has no clip. The clip's checks (feet, limits, root motion, the blade fastest on the hit) then hold the new move with no edit.
4. **Give it a row in the graph:** [character.animgraph.ron](../content/lookdev/character/character.animgraph.ron)'s `clips` and `combat.moves`. Without it, `every_move_a_fighter_has_is_in_its_graph` fails.
5. **Give any new cue a row** in [default_cues.ron](../crates/sapphire_cues/default_cues.ron). Without it, `every_cue_the_recordings_publish_has_a_row` fails.
6. **Cook and rewrite the simulation's copy,** `crates/sapphire_gameplay/baked/player.smoves`:
   ```bash
   cargo xtask content
   SAPPHIRE_BAKE_MOVES=1 cargo test -p sapphire_pipeline --test player_moves the_simulations_copy
   cargo xtask replay --update
   ```

### A kind of enemy, with its brains

Walked. The worked examples are [content/combat/lancer/](../content/combat/lancer/) and [warrior.enemy.ron](../content/combat/enemy/warrior.enemy.ron).

1. **A figure, if it's a new body.** Write `tools/figures/<name>.figure.ron`: KayKit's joint names at the figure's own places, up to six painted regions, and the shapes along the bones, with the weapon weighted to `handslot.r`. Then:
   ```bash
   cargo run -p sapphire_pipeline --profile tools -- gen-figures --specs tools/figures --out content
   ```
   This writes `content/combat/<name>/`'s model and maps. The material, ramp, ILM and sidecars are hand-written after the Lancer's. Stand it with `--look-dev enemy --model <mesh>`. The walk reused the Lancer's figure and clips, so it started at step 3.
2. **Clips.** Retargeted: add the mesh's ID as a `retarget:` target in the KayKit packs' sidecars (`content/third_party/kaykit_character_animations/**/*.glb.meta`); the `.glb` files themselves stay byte-identical. Key-posed: a module per figure in `tools/motion/` (as `lancer.py` does through `figure_pose.py`) writes `.keypose.ron` documents with `--write`.
3. **Moves:** `<name>.moves.ron` with a `MoveSet` sidecar, as [lancer.moves.ron](../content/combat/lancer/lancer.moves.ron) is. It holds `intents` with no buttons, `fighter` (health, poise, capsule), `moves`, and the five reactions under the warrior's names, which a hit forces. Two kinds may share a moves document, and each plays a copy.
4. **Graph:** `<name>.animgraph.ron`, with a row for every move and reaction. Kinds that name the same mesh and graph share them.
5. **The kind:** `<name>.enemy.ron` with an `Enemy` sidecar ([lancer.enemy.ron](../content/combat/lancer/lancer.enemy.ron)). It names `moves`, by ID; `brains`, by name, each with spacing, pace, patience, rest, `attacks` pressing intents by name, and `rise`; the starting `brain`; `phases`, in thousandths of health, where a phase may name a `brain` to change to; and `look`: `mesh`, `graph`, `held` (`None` when the weapon is skinned in), `scale`, `height`, `eyes_below_crown` and `framed_radius`. The walk's kind had a second brain at its 500 phase, and changed to it on the hit that crossed it.
6. **Into a zone:** see "An encounter in a zone". A spawner's `spawns` names the kind by its file's stem.

### A boss, with phases and a script

Walked. The worked example is [content/combat/knell_colossus/](../content/combat/knell_colossus/), and B3's test boss is [captain.enemy.ron](../content/combat/enemy/captain.enemy.ron).

1. **A kind of enemy, as above,** with three more things in its `.enemy.ron`:
   - `boss: Some((name: "boss-<name>"))`, its bar's name key, which the HUD draws on the bar. Add `boss-<name>` to `content/loc/{en,ja}/game_ui.ftl`, and any new Han to `content/fonts/ui.charset.ron`. `cargo test -p game_ui --test strings` fails a boss whose name either language lacks (Z3e).
   - `phases` with names, `(at: 500, name: Some("tolling"))`. The hit that crosses a phase says its name as the boss's cue and changes the brain the phase names. The HUD's boss bar marks each phase. From there the music plays the score's phase of that name, if it has one (see "Music by encounter phase").
   - `script: "<id>"`, naming a `.luau` by its sidecar's ID. The sidecar's importer is `Script(())`.
2. **The script.** It returns `names` and its hooks: `on_phase(actor, phase, health)`, `on_move`, `on_hit`, `on_defeat` and `on_beat` are in [sapphire.d.luau](../crates/sapphire_script/sapphire.d.luau). **Phases count from 1, and so do places in `names`;** the walk's first try checked `phase == 0` and never fired. Commands such as `sapphire.set_brain(actor, place)` and `sapphire.force_move(actor, place)` take a place in `names`. A script is pure decisions, with no strings at run time (plan §3, "Luau's job"), and the scan refuses string concatenation and a bare `for` over a table.
3. **Into a zone, content only.** Add a spawner whose `spawns` is the kind, an encounter that calls it, and the scenario updates the next recipe names. The commit that does this must touch only `content/`, `tests/replays/` and `tests/golden/`, or `bosses_are_content.rs` fails. The boss's name must be in no `.rs` file, test messages included, and its script must be reached from the zone.
4. **Kinds with scripts can share a zone.** Each kind's script hears only its own fighters: their phases, moves and defeats, the hits they land or take, and every beat. A fight's scripts share one VM, with its memory limit and each step's budget. They also share one board of 64 numbers, as they share flags. So a script that keeps numbers on the board should use slots that no other script in its zones uses.
5. **Hot reload:** in a `dev` build, `link push <file>.luau` reloads the script into the running fight between steps, and the board is kept.

### An encounter in a zone

Walked, as part of both recipes above.

1. **The encounter:** `<name>.encounter.ron` beside the zone, with an `Encounter` sidecar ([market.encounter.ron](../content/zones/lantern_ward/market.encounter.ron)). It holds `starts_on`, a trigger's event; `faction`; `tokens`, how many may swing at once, and `token_rest`; and `waves`, each with `when` (`Start`, `Remaining(n)` or `After(ticks)`), a `gap` between spawns, and `spawners` by name.
2. **In the zone:** spawners for its fighters (`spawns` is the kind's stem); a trigger (`Box` or `Sphere`) whose `event` starts it; and its ID in `encounters`. A ground-level box must reach about 0.25 m under the paving, or feet never enter it. The cook refuses a spawner off the navmesh, and an encounter naming a spawner or trigger the zone lacks.
3. **Every scenario on that zone** (`content/scenarios/*.scenario.ron` whose `zone` is it) must list the new encounter in `encounters`, and in `cleared` if it shouldn't run there. A scenario that doesn't fails to set up.

   Its kinds needn't be listed anywhere else. The zone's spawners say what it spawns, so a scenario adds each kind that its encounters' spawners name after the kinds it lists, with that kind's moves and script. Cleared encounters count too. The kind is the `<spawns>.enemy.ron` under `content/combat/`, which the build compiles in. A kind's name is its file's, so two `.enemy.ron` files with one name fail the build. A name with no kind fails the scenario's setup, naming the spawner.

   A scenario's `max_actors` must hold its cast and every fighter its encounters bring, cleared ones aside, since a fallen fighter keeps its place: a game's flow refuses a scenario that's too small (`Game::new`, G1c), where the last arrivals would come to nobody and their encounter clear with no fight. `room_for_everyone_counts_the_cast_and_every_fighter_to_come` holds that count to what the zone plans for every scenario, and `ward_flow` holds the flow's run to its pool, so a fighter more in the ward means raising `ward-run`'s `max_actors`, a document's number.
4. **A scenario of its own:** a `cast` member with `moves: "player"` standing at a player spawner inside the trigger, as [ward-terrace.scenario.ron](../content/scenarios/ward-terrace.scenario.ron) does. Then record a replay:
   ```bash
   cargo xtask replay --record <name> --scenario <scenario> --script fight-back --ticks <n>
   ```
   The script is any row of `SCRIPTS`, whose `fight-back` serves any encounter. **Rust step,** only for an input none of them gives: a new scripted input is a row in `crates/sapphire_gameplay/src/replay/script.rs`.
5. **Every replay on the zone moves,** because the zone's digest is in `tables_hash`. Re-stamp them with `cargo xtask replay --update`. The walk moved six. x86-64's lines are owed from CI. On the Lantern Ward, a fight that plays differently also needs `ward-whole-walk` recorded again by its recorder (see "Tools"), which is data. No test counts the ward's kinds or fighters (Z5e): `a_zones_encounters_bring_the_kinds_they_spawn` reads them from every zone's spawners.
6. **A golden** of the fight in its zone, if you want one. **Rust step:** a golden scene is a row in `tools/sapphire_testkit/src/scenes.rs`, with the replay's `--arena --replay` capture and a tick. It's a test list, so a fight lands without it: exit test 2 allows the PNG in the boss's commit, and the row goes in a commit of its own, as the ward's did.

### A zone, with checkpoints and exits

The worked examples are [content/zones/lantern_ward/](../content/zones/lantern_ward/), [proving_hub/](../content/zones/proving_hub/) and [proving_ground/](../content/zones/proving_ground/). Spec §5.21 is the reference.

1. **The zone:** `content/zones/<name>/<name>.zone.ron` with a `Zone` sidecar. The cook finds it by its suffix and packs `zone_<name>.spack`. The `ZoneDocument` denies unknown fields. It has `grid: (cell_size)`; `placements` of prefabs by GUID (`.assembly.ron`) and `instances` of meshes; `lights`; `probes` (a `.probes.ron` by ID); `day` and `rigs`, as relative paths; `colliders` (by ID); `navigation: Some((min, max, seeds))`, where `seeds` is optional; and `spawners`, `triggers`, `checkpoints`, `encounters`, `splines` and `exits`.
2. **Colliders:** `<name>.colliders.ron` with a `Colliders` sidecar. This is what bodies walk on and what the navmesh bakes from, since placements bring no colliders. A blocker needs its `groups` (`CharacterBlocker`, `CameraBlocker`); a `block_` or `camera_` prefix is only a convention, which a test holds for the arena alone.
3. **Checkpoints:** `(name, position, yaw_degrees, volume)`, reached outside a fight. A save names the latest. The cook refuses one off the navmesh.
4. **Exits:** `(name, to: "<zone id>", arrive: "<spawner there>", prefetch)`, with a trigger whose `event` is the exit's name. The cook reads the other zone to check `arrive`. A hub's `prefetch` brings the next zone in ahead of time.
5. **Look at it:** `--zone <name>[:<spawner>]` on the sandbox, with `--capture`. In the editor, `cargo xtask content --cook editor` first, then the zone pane moves placements.
6. **A scenario and a walk:** a scenario document on the zone, as above, with the zones a trip reaches in its `shelf`. **Rust step,** for a replay of a trip only: a shelved zone must be a row of `BAKED_ZONES` in `crates/sapphire_gameplay/src/replay/scenario.rs`, a build-time table, since its navmesh takes a bake, and a replay test that names a zone's entry uses that file's constants. A trip in the game itself is data. A new zone is otherwise documents, held by the cook's checks. On the Lantern Ward, `zone_ward.rs` reads its checkpoints from the document (Z5e): each is a walk from the start, and no two share a name.

### A dialogue, with story flags

The worked example is [ward_guide.dialogue.ron](../content/dialogue/ward_guide.dialogue.ron). Spec §5.23 is the reference.

1. **Flags:** add the names to [content/flags.ron](../content/flags.ron), at most 256. A flag is an `i32` that reads 0 until set. The cook refuses a flag that isn't listed.
2. **The dialogue:** `content/dialogue/<name>.dialogue.ron` with a `Dialogue` sidecar. It holds `start`, optionally `opens_on` (an event name), and `nodes`. Each node has `say: (speaker, line, vo)`, `set` flag writes, `next`, up to 4 `choices` (each with `line`, `to`, `when`, `set`) and `branch` arms with `otherwise`. A condition is a small language over flags: `flag.x >= 1 and not (flag.y == 2)`.
3. **Strings:** every key starts `dialogue-`. Add them to `content/loc/en/dialogue.ftl` and `content/loc/ja/dialogue.ftl`, and new Han to `content/fonts/ui.charset.ron`.
4. **Opening it:** the run's scenario lists it in `dialogues`, with `flags` naming the registry, as `ward-run` does. Then either a beat in [game.flow.ron](../content/game.flow.ron) opens it (`then: Dialogue("<id>")`, as the guide opens after the gate), or a zone trigger whose `event` is its `opens_on` does. The fight holds while it's open. `Game::new` refuses a flow whose beats name a dialogue its scenario doesn't load. In the shipped run, `ward_flow`'s `what_the_run_names_resolves_and_plays` cooks each dialogue `ward-run` names and walks it to its end, `plays_the_ward` counts what the scenario names, and the whole walk's recorder answers whatever the run opens (Z5e).
5. **Look at it:** `--ui-game dialogue --ui-dialogue <name>:<n>` previews the panel on `<name>.dialogue.ron`, `<n>` lines in; with no name it's the ward's guide, as the `game-guide-*` goldens show it. The sandbox's build finds each dialogue in `content/dialogue/` by its sidecar (Z5e). In the game: `cargo test -p game_app --features dev --test opens_the_guide -- --ignored`, and `plays_the_ward` for the flow's.

### A cutscene

The worked example is [court_gate.timeline.ron](../content/zones/lantern_ward/court_gate.timeline.ron). Spec §5.23 is the reference.

1. **The timeline:** `<name>.timeline.ron` with a `Timeline` sidecar. It holds `length`; `clock`, which is `Real` for a cutscene (it holds the simulation at a step's edge) or `Steps` for a cinematic in a fight; `camera`, with blends in and out and keys; `characters`, by role, playing rows of their graph and placed by keys; `cues`; `post` offsets; `subtitles` by key; `time_scale` (not on a `Real` clock); and gameplay `events`. Every cutscene can be skipped. There's no field for it, since only the time scale and events reach the simulation.
2. **A new pose:** a `.keypose.ron` and its sidecar, named as a clip and a move in [character.animgraph.ron](../content/lookdev/character/character.animgraph.ron).
3. **Subtitles:** keys and speakers in `content/loc/{en,ja}/cutscenes.ftl`, a line under `subtitle-` and a speaker under `speaker-`: the HUD shows only keys under those prefixes. `tools/sapphire_pipeline/tests/cutscene_strings.rs` and `game_ui --test strings` hold them.
4. **Look at it:** `--arena --timeline <id>` plays one on the sandbox, and the editor's sequencer pane scrubs it.
5. **Starting it:** the run's scenario lists it in `timelines`, and a beat in [game.flow.ron](../content/game.flow.ron) plays it (`when: Started("<encounter>")`, `then: Timeline("<id>")`, with an optional `align` checkpoint the player is brought back to first), as the gate's does. `ward_flow` holds the shipped flow to what its scenario loads, and each timeline it names to its sidecar's ID and its place; the whole walk counts at least one cutscene (Z5e). A beat's timeline is recorded into the run's file, so the whole walk's replay holds it with no scripted input. **Rust steps,** for checks you choose to add: a replay that plays it on a scenario with no flow is a scripted input (`PlayTimeline` at a tick in `script.rs`), and its goldens are rows in `scenes.rs`.

### A game screen

The worked examples are the six documents in [content/ui/](../content/ui/). Spec §5.17 is the reference.

1. **The screen:** `content/ui/<name>.ui.ron` with its sidecar, naming the model it reads, a list of `(path, kind)` that a cook holds every binding to: `menu.model.ron` (a way out, a run to continue, the player's settings), or `character`, `dialogue` or `hud.model.ron` for a screen showing those live values. Its nodes are laid out by `taffy`, with `key_from` for a key chosen at run time under a prefix, and it has `timelines`, `states` and `transitions`.
2. **Strings:** keys in `content/loc/{en,ja}/game_ui.ftl`, the Japanese under the file's machine-quality mark, and new Han in `content/fonts/ui.charset.ron`. Then run `cargo test -p game_ui --test strings --test menus`.
3. **Its row** in `content/ui/menus.ron` (M3 U9): its `name`, its `document` by the sidecar's ID, its `model` (`Menu` unless it reads another), `modal: false` only for words under the fight, what `back` does beyond its machine, and a row for each focusable node: `Command(..)` for the flow, `Open("<screen>")`, `Step(..)` for a setting, or `Nothing`. A row on another screen that opens it is how it's reached; that screen's document needs the node. `game_ui`'s menus and strings tests read the list, so they hold its nodes and keys with no change to them, and `a_new_screen_is_documents` is the worked example.

   Changing an existing screen's look, layout, words or motion is documents alone: the editor's screen pane shows an edit on the next frame, and live link reaches a running game. A `.model.ron` is in field order by path, so on a merge, rebuild it from its model struct.
4. **Look at it:** run
   ```bash
   cargo xtask run --platform desktop -- --capture <png> --ui-plain --ui-game <name> --ui-locale ja
   ```
   with `--ui-pseudo` for pseudo-localization. **Rust step:** goldens at 4:3, 16:9 and 21:9 are rows in `scenes.rs`.

### Music by encounter phase, and voice lines

The worked examples are `content/audio/generated/music/lantern.score.ron` and `content/audio/generated/voice/{en,ja}.voice.ron`. Spec §5.15 is the reference.

1. **A score:** a `Score` document with `tempo_bpm`, `beats_per_bar` and `bars`; `stems`, each with its `.ogg` by ID; `phases`, each naming the stems it plays; and `fade_beats`, `volume_db` and `duck_db`. Its sidecar's importer is `Score(())`. The phases a fight chooses are `waiting`, `running` and `cleared`, and the flow's `music` names those for the title, a defeat and a win. A boss's named phase plays in place of `running` once its health is past it, if the score has a phase of that name (Z5a): the Knell Colossus's `tolling` is the worked example. A phase change fades only the stems it changes, from the next bar line.
2. **Which score:** `music:` in [default_cues.ron](../crates/sapphire_cues/default_cues.ron), by ID. That's a data edit, compiled in.
3. **Voice lines:** each line is a `Cue` document on the `Voice` bus with its `.ogg`. Each language's `VoiceLines` document lists `(key, cue)`, and `voices:` in `default_cues.ron` names each language's document. A cue-table row speaks a line with `line: Some("<key>")` when the simulation publishes that row's cue. The voice language follows the player's language setting.
4. **Rust steps:** a line spoken on anything but a published cue needs code, and a conversation's lines aren't voiced (see "Everything deferred"). New placeholder sounds are rows in `tools/sapphire_soundgen`; a recorded `.ogg` needs none.

## Rules the reviews made

**M2's, still binding** ([m3-handoff.md](m3-handoff.md), "Rules the reviews made", has each with its test): every struct in a move document denies unknown fields, and so does every M3 document; `MAX_MOVE_REACH` and `MIN_SHAPE_EXTENT`; a defeat takes the stick; `LiveMoves::start` sets up on the edit; a frame's clocks go `alpha` of the way along their last step; a fight starting over ends what the last one set going; a live cue table can't name a sound nobody loaded; `PointerState` decides who has a desktop's pointer.

**M3's packages' own,** each held by a test or a scan:

- **No allocation in a step.** M2's gate 4 holds through every new system. Around a script hook, the VM's allocations count apart (`script_allocations`), but a Rust allocation in a binding still fails. Warming, streaming and the collector run between steps.
- **A script is pure decisions.** Bindings take numbers only, and are generated from `crates/sapphire_script/api.toml` by `cargo xtask script-api`. A bad place, slot or actor is a fault that takes back the hook's commands. Shipping stays `panic = "abort"`, so a binding never returns a Rust `Err`.
- **Nothing reads the OS's math.** The simulation, Luau (libm builtins off, `-ffp-contract=off`) and anything a cook bakes for them use pinned functions only.
- **A cook is the same bytes everywhere** for what the simulation reads: navmesh bakes, zones and scripts. CI's three OSes hold each pin.
- **Kinds are contiguous** in the order they land. **A golden or replay that moves** moves in the package that moves it, with the other backends' and architectures' values from CI.

**M3's reviews',** by area:

- **Renderer and UI (Z3a):** anything whose plan is made in `prepare` settles before a lost frame's `recorded` check, as shadows and insets do. An inset's picture snaps to its plate's whole pixels. A bound key's problem clears once it reads under its prefix. The shape cache compares floats by the bits it hashes.
- **Editor and live link (Z3b):** every document kind in the content opens for editing, so a new kind needs an arm in `tests/fuzz.rs`. The screen pane follows the model file its document names. A drag held through an undo or a save stays one step. Live link counts the app's work (16 requests) apart from each connection's unwritten replies (16).
- **Simulation, scripts and saves (Z3c):** the game's save types (`ZoneStart`, `FlagValues`) keep their version 1 bytes in `crates/sapphire_gameplay/tests/save_formats.rs`, so a field change needs a new `VERSION` with an upgrade that reads them. `ScriptVm::ask` reads another VM's condition as false, as `fire` refuses its hooks. A failed directory flush leaves the new save in its slot, so the game writes again rather than calling it lost.
- **Content and streaming (Z3d):** a pack encoding is keyed by its kind's number, and a zone's kept parse is checked again on every cook. Files under `content/third_party/` stay as downloaded: a corrected copy lives beside its users with the original's sidecar ID, and the original gets a new ID and `cook: false`, as the look-dev character's model does.
- **Platform, input, audio and strings (Z3e):** a boss's name is `boss-<kind>` in both languages, held by `game_ui`'s strings test, and never in `.rs` text. The music bus carries the player's volume. The app thread's own events are never dropped (`Inbox::hold_own`). A captured input stays captured until it's at rest, and a finger slot past `MAX_FINGERS` is dropped. Windows' save rename waits out a held file.
- **The flow and the game (Z3f):** a pause holds a fall or a win too, a quit is heard anywhere, and a conversation that ends under a pause leaves the pause holding play. A won save continues without winning again, and a refused timeline ends at once. The flow's marks are taken only in the flow's zone, and a beat's step allocates nothing. A title choice made under its wipe is kept, and a restart waits for a trip to end. The game's captures refuse the flags they can't honour.

## Everything deferred

### Rust steps left in the "How to add" recipes

M3's first pillar counts each of these as a bug. The first three are test lists and a build-time table: a commit of documents alone lands without them, and they're the Rust a check someone chooses to add takes.

- **A golden scene** is a row in `tools/sapphire_testkit/src/scenes.rs`. Every recipe that wants a picture has this step.
- **A replay's scripted input** is a row in `crates/sapphire_gameplay/src/replay/script.rs`, timed buttons or a `PlayTimeline` at a tick. Recording on an existing script needs none, and a run through the flow records its beats' commands into its own file. Making scripts documents would be a format of their own: not done in Z5e.
- **A shelved zone** is a row of `BAKED_ZONES` in `replay/scenario.rs`, since the simulation keeps a baked copy of its navmesh for replays of a trip.
- **A new screen** is documents (M3 U9). Only one that shows a value no model holds, or has a row do something no `RowAction` does, is Rust.
- **A placeholder sound** is a row in `tools/sapphire_soundgen`.

**Fixed in M3:**

- **Starting a cutscene, and holding a dialogue in a scenario.** G1 fixed both: beats and a scenario's lists are documents.
- **Exit test 2's third clause** charged a boss to every zone that reached it through an exit. It now looks only in the zone whose spawner names it ("Look for a boss's commit in the zone that has it").
- **A cinematic and a boss's music** (Z5a): a move names its cinematic by its timeline's file name, and a boss's named phase chooses the score's phase of that name. **A new screen** (U9): which screens there are and what their rows do is `content/ui/menus.ron`.
- **One kind with a script per zone.** A fight bound one script, and a scenario had to list every kind its zone's spawners name. Now each kind's script is bound for its own fighters in the fight's one VM, and a scenario on a zone adds the kinds its encounters spawn ("Bind each kind's script for its own fighters"). The Lantern Ward's six replays moved, since the Knell Colossus's script no longer hears anyone else.
- **The tests that pinned the shipped Lantern Ward's content** (found by Z4's last part; Z5e). Each holds a relation read from the documents now: `a_zones_encounters_bring_the_kinds_they_spawn` reads the kinds from every zone's spawners, for every scenario on a zone with encounters; `room_for_everyone_counts_the_cast_and_every_fighter_to_come` holds every scenario's room to what its zone plans, with a built case of encounters fought; `zone_ward.rs` reads the ward's checkpoints; `ward_flow` resolves and walks each timeline and dialogue `ward-run` names; `plays_the_ward` counts the conversations the scenario names; the whole walk's recorder walks the zone's `tour` spline and answers any conversation, and the walk counts at least one cutscene. A checkpoint, a fighter, a kind, a cutscene or a conversation added to the ward or its run passes them with only `content/` changed.
- **A player's move** (Z5e): `player_moves.rs` reads its moves and attacks from `player.moves.ron`.
- **The sandbox's dialogue preview** (Z5e): `--ui-dialogue <name>:<n>` takes any dialogue in `content/dialogue/`, which the sandbox's build finds by its sidecar; the guide stays the default.

### Known to the run

- **Voice in conversations** (spec §5.30). A1's seam is Q2b's `DialogueRunner::lines_shown` and each line's `vo` key. Nothing plays it, since that needs the keys mapped to a `VoiceLines` document and the fight given a way to reach the device's audio.
- **Progress v1 holds one zone** (spec §5.30). Nothing is saved while a run is in another zone, and every restart stands its run in the flow's zone, the stage taken back there by a trip of its own (G1b). A run stood in the town's square can't be taken back, since a zone's arrival leaves the town; the ward has no exits, so the shipped game never trips.
- **`tables_hash` changes at a zone entry** (Z3c), since it takes the current statics digest. Nothing compares it after an entry, and nothing should start to. Take a run-wide identity at the run's start instead.
- **The script VM's heap isn't replayed.** Binding a dialogue's conditions and asking them between steps allocates in the fight's VM; since G1b a recording binds them all as it attaches, so they sit in its VM from tick 0, where a replay binds none. A hook that hits the VM's 2 MiB memory limit fails, and the checksum holds how each hook ended. A replay asks no conditions. So a recording whose panel asked conditions could diverge from its replay near that limit. Spec §5.23's "Can't do" covers the garbage that asking leaves, not this.
- **Kinds' scripts share one board** of 64 numbers, as they share flags (spec §5.24). Per-script boards would change the checksum's layout.
- **No production path loads a pack's `.sscript`.** Dialogues carry their conditions' source and compile at bind (Q2b), and a script could do the same (71 µs for `arena-scripted`). The cooked kind doesn't carry its script either, so a game reading kinds from packs needs a format change.
- **The editor's scenario choice.** Its viewport fights only `arena-fight`, so another kind never reaches it. **Live tuning of kinds:** neither a kind's `.enemy.ron` nor its graph reaches a running fight, only the warrior's graph. Each is about half a day (spec §5.22, C2 part 2's "Can't do, in the tools").
- **Post's gradient packing** (`pack_linear_gradient` and `pack_radial_gradient` in `crates/sapphire_render/src/post/layers.rs`) is heavy defensive code to keep up, not a bug (Z3a).
- **Secondary motion's worst frame** (Z1's fixes; spec §3.6, and §5's springs, "Clearance by tier and distance"). On Desktop-Low the clearance pass solves every third tick and on the phones' tiers every fourth, writing the last solve's corrections between, so p50 is inside §3.2's rows. p99 is still a solve frame's on every tier (2.10 ms on Desktop-Low and 1.28 on Mobile-Low with the machine to itself), and Desktop-Low's mean is 0.56 ms against its 0.5. Spreading a solve's parts over the frames it holds would cut both. Desktop-High keeps the full pass every frame, about 2.6 ms, which §3.2 doesn't budget.
- **CI's build jobs are near their limits.** On M3's last two runs the Windows golden build took 28 m 18 s and 29 m 58 s, and Lint and the lavapipe build each hit 30 minutes once on a slow runner (#266). All three now have 45 minutes, and the App tests job has 110 for the whole walk (G1c). A timed-out job reads as skipped, not failed, so each costs a CI round to notice. A build cache shared between jobs, or fewer sandbox builds, would buy the time back. Q4's six `game-ward-gate-*` scenes draw their first 160 frames at a tenth of the size (`--game-draw-from`), because drawing all 251 at full size cost 4 to 11 minutes a scene on lavapipe and WARP.

### From the spec's "Can't do" lines

Each is in its section, with what it would take:

- **Zones (§5.21):** placements bring no colliders; probes are one grid a zone; warm reach isn't per encounter; a hub's next zone is mounted again each visit; the enemy a trip leaves is drawn nowhere.
- **Navigation (§5.19):** nothing looks ahead, so two walkers meet before they part; every brain checks every other over a flat list; a scenario on a zone other than the arena has no navmesh path yet; nothing loads a zone's patrols into the game.
- **Kinds and bosses (§5.22):** a kind's held pose is the warrior's turns; `arena::framing` frames everyone at the warrior's size; a brain can't keep apart from allies or circle while it waits for a token; enemies have no meter; the boss's awaken and turns are unused.
- **Encounters (§5.22):** one side per encounter; `ReturnTo` names a checkpoint by its place in the zone's list.
- **Timelines and cinematics (§5.23):** a cinematic's run goes on if its move is cut short; a freeze can't be counter-scaled; the editor's live run plays a set without its cinematic; a `TodCutscene` isn't a track.
- **Dialogue (§5.23):** a dialogue's keys aren't added to font subsets; script writes aren't held to the registry; quests don't step.
- **Music (§5.15):** stems decode whole, about 7.7 MB for the score with Z5a's toll; no horizontal re-sequencing or stingers; one Language setting drives text and voice; two bosses up at once play the first's phase (Z5a).
- **Renderer (§5.12):** the mesh pool's buffers never shrink; no LOD selection; the inventory's warm-up blocks a load, which M4's warm-up screen and driver cache answer.
- **Scripts (plan L2):** no real type check of `sapphire.d.luau`, since there's no `luau-analyze` here.

### Carried from M3's handoff, unchanged

[m3-handoff.md](m3-handoff.md), "Everything deferred", has the detail:

- **Compile-time iteration:** no hot-patched Rust. **Sound** isn't hot-reloaded.
- **D3, D4 and N3–N5's limits:** statics fixed once built, a crossing judged by its end pose, `Jump_Start` and side dodges unplayed, no get-up clip, limits beating colliders, springs with no floor.
- **O2's and O3's reads, owed to a person.** **Camera framing,** counted not judged. **Balance:** mashing `light`. **Phone UX:** an iPhone has no Back, and nothing auto-pauses on return.
- **Z1's M2 recommendations:** parallel recording on a phone, a screen-effects share in §3.3's post row, per-cook texture size, the character's 11 opaque materials against 4, and footprint in `frame stats`.
- **CPU rows moving together ×2.6–3 between runs,** unexplained. **Toolchain:** wgpu stays on 30 and SDL on 3.4.18; reassess at this boundary.
- **Renderer:** reflection probes for characters, and SSR; cut-ins, halftone, C7's patterns; full PSO warm-up (M4), upscalers, local-light shadows, characters shadowing each other; depth of field, light shafts, water.
- **Assets:** memory-mapped packs, mip streaming, a shared cache, a cook-on-save watch. **Live link's pairing token.** **Store and signing (M4),** crash upload, weather per zone, the entity inspector, nightly device runs, and gate 5.
- **Plan §3's "Not in M3":** XPBD cloth, ragdolls, dynamic props, decals, GPU particles, lip-sync, a real second character, patching and accounts.

## Known limits and risks

- **Feel is unknown, and M3 built a game on it.** Nobody has played review 3, M2's fight or M3's slice, and every brain, encounter, number and clip is a first guess. All of M2 and M3 could be well built and still not fun. The answer is cheaper to act on now, since a move, a brain, an encounter or a boss is a document. If it's the off-ramp, the Rust systems in "What M4 builds on" stay, and `game/` and the editor are what go ([m3-plan.md](m3-plan.md) §6).
- **More bugs are likely.** The reviews landed 34 fixes and holds. Walking this handoff's recipes found two bugs, the exit run's rehearsal found one, and the whole walk found one: the ward's pool of 12 refused its boss, so the terrace cleared empty and the flow won with no fight (G1c). Most packages fixed something they found below them.
- **No phone has run anything,** after four milestones. Every mobile number is an M5 at the Mobile tiers' settings, the iOS Simulator or the Android emulator. Parley, ICU, Luau and zones land on phones untested.
- **Motion without an animator,** now for two enemies, a boss, a cutscene and an ultimate. The fallback is still a commissioned animator or a bought pack, and a pack that bars redistribution can't go in a public repo.
- **Machine translation** in a language nobody here reads.
- **Our own navigation runtime** is about 1,200 lines to own, polygons only, with no dynamic obstacles.
- **A VM in the step** makes gate 4 narrower than M2's: zero allocations everywhere but around a hook, where a counted budget holds.
- **A moved checksum costs two CI rounds,** because no machine here runs x86-64. Adding a kind to a zone moves every replay on that zone.
- **GitHub's macOS runners were out** for long stretches during M3, on 2026-11-09 among others. Landings then were verified by the local stand-in, not by CI's macOS jobs.
- **Software rasterizers aren't GPUs,** and the character shader is full (M2's handoff).
- **Scenarios are compiled in,** up to about 100 KB of zone document (B4). A game with large zones reads them from packs, which needs the cooked kind to carry its script.
