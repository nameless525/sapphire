# M4 handoff notes

What M4 inherits from M3, for whoever starts M4. Like [m3-handoff.md](m3-handoff.md), it's a map. Each item gets a line or two and a pointer to where the detail lives: the spec ([engine-spec.md](engine-spec.md)) by section, the plan ([m3-plan.md](m3-plan.md)) by package ID, the runbook ([m3-exit-run.md](m3-exit-run.md)), or the code.

**A draft.** It was written on 2026-11-09 on the top of the B3 → B3b → A1 → C2 part 2 chain, before four of M3's last packages landed:

- G1, the game's flow;
- Z1, the zone bench;
- Z2, the final exit run;
- Z3, the reviews.

Sections that wait for one of them say **lands with** that package, and are filled in at M3's close.

**Nobody has played anything.** The maintainer started M3 on 2026-11-03 without having played M2's fight, so review 3 from [m2-exit-run.md](m2-exit-run.md) was never run, and nobody has played M3's slice either. Whether any of it is fun is unknown. That is still the largest risk in the project (see "Known limits and risks").

## Where M3 stands

M3 built a small game, `game_app`:

- a title;
- a walk through the Lantern Ward, with fights in three rooms and a boss on the terrace;
- a cutscene at the court's gate;
- checkpoints, a pause, settings and a character screen, in English and Japanese, on a pad, a keyboard and touch.

Almost everything in it is documents under `content/`, and the exceptions are listed under "How to add" and "Everything deferred". Nothing in it has been judged by a person.

| Exit test ([m3-plan.md](m3-plan.md) §1) | Where it stands |
|---|---|
| 1. A zone with three encounters, walked to its boss | Replays of the ward's walk and its fights hold on all three desktop OSes, with no allocation in a step. **Lands with G1:** the walk through the game's flow, from the title to the boss. |
| 2. A boss with no Rust changes | Holds. `crates/sapphire_gameplay/tests/bosses_are_content.rs` finds the Knell Colossus by being a boss and checks three things. Its name is in no `.rs` file. Its files, script included, are reached from the zone's document. The commit that put it in the zone touched only `content/`, `tests/replays/` and `tests/golden/`. CI's Linux leg reads the whole history for the third clause. A second boss was added by the recipe below with no Rust, and the test held it once a bug in its third clause was fixed (see "Everything deferred"). |
| 3. One cutscene | A watch and a skip reach one checksum, and the subtitles resolve in both languages. **Lands with G1:** subtitles drawn on screen, and the cutscene started by the court's trigger rather than at a fixed tick. |
| 4. HUD and menus in two languages | The menus are walked by pad, keyboard and touch at three aspects in both languages, with goldens. The Japanese is machine-quality, and a person who reads it is owed. **Lands with G1:** title, new game, continue and return to title in a real run. |
| 5. Five platforms | Done, pending devices, as M1 and M2 closed. Linux and Windows run on CI's software rasterizers, macOS on Metal. The iOS Simulator and the Android emulator each played a scripted fight to this Mac's checksum ([m3-exit-run.md](m3-exit-run.md), "Rehearsal"). No phone has run anything. |
| 6. The editor | Proved by tests with no window. The window is owed to a person. |
| 7. The inventory | 0 first-use pipeline builds after load on Desktop-Low and Mobile-Low, in every app lane and bench. **Lands with G1:** the replay of the whole walk through the flow. |
| 8. Budgets and durability | Saves survive a kill at every step of a write, and old saves migrate. **Lands with Z1:** the zone bench's §3.2 rows, memory across the walk and a transition's time, in spec §3.5. |

**Lands with Z2:** the final run's head, its numbers and its date. **Lands with Z3:** what the reviews found and fixed, and the bug count.

## Device checks still owed

Nothing here has been run, for want of a device or a person. [m3-exit-run.md](m3-exit-run.md) has M3's procedures, and [m2-exit-run.md](m2-exit-run.md) and [m1-exit-run.md](m1-exit-run.md) have the older ones.

### M3's

- **Feel, review 3 and everything after it.** Thirty minutes with a gamepad on [m2-exit-run.md](m2-exit-run.md)'s sheet, then the same for M3's slice:
  - the Lancer and the boss;
  - three enemies in a room with tokens;
  - the boss's second wind;
  - the ultimate and its meter.

  Every brain, encounter and number in `content/combat/` is a first guess. In `ward-market`'s replay the scripted player loses the room.
- **Motion nobody has judged.** The Lancer's and the Knell Colossus's key-posed and retargeted clips (C2 part 1), the gate cutscene's poses (Q3) and the ultimate's chop (Q3b).
- **A reader of Japanese** for every string in `content/loc/ja/` (C3). Both files' resource comments say they're machine-quality.
- **Anything heard.** Nobody has heard A1's music or voice lines, which are placeholders `tools/sapphire_soundgen` synthesizes, or any of M2's cues. On the emulator, audio warns that real-time scheduling was refused, and nobody has listened.
- **A window on this Mac, or any desktop.** Agents may not open one here.
  - The game's first run in a window.
  - The editor (`cargo xtask editor`): docking by hand, a real drag on the gizmo, keyboard focus, and a lost device (E3, E4, E5).
  - A cutscene's blend, whose camera numbers are bounded but which nobody has watched.
- **A phone.** None of these has run on one, only their sizes were measured on the Android and iOS builds:
  - Luau, `parley` text, ICU plurals, zones and streaming, saves (`SDL_GetPrefPath`'s real paths, a phone's flash, a power cut), and the 3D inset;
  - a phone driver's cold compile of the packs' pipelines, which blocks a load for 6–7 s on an M5 (K1).

  Mobile-Low's numbers come from this Mac at Mobile-Low's settings.
- **Touch on iOS.** `simctl` can't touch, so iOS touch is unproved. The Android emulator took real taps (S2).
- **x86-64 Android** links Luau and has never run it (L1).

### M2's, carried

[m3-handoff.md](m3-handoff.md), "Device checks still owed", has each with its pointer. M3 settled none of them:

- a desktop window's pointer and focus, by hand;
- a gamepad on a desktop, rumble, and whether the pad layout is right;
- sound on any speaker, and an audio device with no `f32` format;
- a phone: touch, exit test 4's budgets (Mobile-Low held at 30 fps, throttled, after 15 minutes), exit test 2 over live link, the rest of §3.5's list, and audio sessions;
- the timeline editor's window, now a panel in `sapphire_editor`'s shell, which nobody has opened either;
- `xcrun devicectl device copy from`;
- end-to-end latency on glass;
- the emulator crash nobody cleared, and the Windows stack fix that rests on dumps.

### M1's, carried

[m2-handoff.md](m2-handoff.md), "Device checks still owed", has each. None was settled in M2 or M3:

- exit tests on an iPhone and an Android phone;
- ProMotion and 90/120 Hz pacing (F7, F8);
- on-tile MSAA (R7, Q5), and post on an A14-class phone;
- the performance hunt's numbers on phones, and a phone's cold start;
- a phone driver's shader compile time;
- Q14 (Android pre-rotation), Q9 (device floors), and tier selection;
- Windows and Linux hardware, Q3 (the Steam Deck) and Q4 (Windows' default backend);
- Android crash capture.

## What M3 settled from its handoff

These items in [m3-handoff.md](m3-handoff.md) are closed, and don't carry:

- **Not in M2, now built:**
  - Luau (L0–L3);
  - navigation (P0–P3);
  - music (A1);
  - a second enemy type and a boss (B3, C2);
  - zones (W1–W5);
  - saves (V1).

  XPBD cloth, ragdolls, decals, GPU particles and lip-sync stay out (spec §10).
- **One stage, compiled in:** zones are documents (W1), found by name with `--zone <name>`. Scenarios are documents (B2), compiled in by a build script with no Rust edit.
- **The town as one mesh:** cut into cells that stream (W2), and memory given back (W3).
- **G6, one enemy and no director:** several enemies with factions and soft-lock choice (B1), encounters with waves and tokens (B2), paths across a zone, separation and patrols (P2, P3).
- **G3, everyone hits everyone and a fallen capsule blocks:** factions, and a fallen capsule in no group (B1).
- **G1's missing teleport, and the curb's throw:** `Simulation::teleport` and checkpoints (B4). The throw is fixed in the controller and no replay moved.
- **G5's empty takeover slot:** timelines drive the camera's takeover (Q1).
- **O4's hand-placed blade:** a kind's look holds a mesh at a joint, or nothing when the weapon is skinned in (B3, B3b).
- **Numbers that were code:** springs, IK controls, targeting's weights and the HUD's layout are documents. Bindings and effects reload through live tuning's route (S3).
- **Single-example tools and scenes:** three kinds of enemy (warrior, Lancer, the Knell Colossus) plus the captain, and six zones. This handoff's recipes were walked on new ones.
- **PSO warm-up's first half:** a pack lists its pipelines and a load builds them, with a first-use count that fails a bench (K1). The blocking screen and a driver cache are M4's.
- **Group packs beyond `audio.spack`:** one pack per zone, plus `fonts`, `loc` and `ui`.

## What M4 builds on

### Crates and layering

There are 46 crates in `crates/`, three in `game/`, the sandbox, and the tools. Each is placed in [xtask/layers.toml](../xtask/layers.toml), which `cargo xtask check-layers` enforces. Each crate's `//!` doc says what it owns, and [docs/adr/](adr/) has the reasons.

**M3's new crates:**
- **L2 (plain data):**
  - `sapphire_zone`;
  - `sapphire_navmesh`;
  - `sapphire_timeline`;
  - `sapphire_dialogue`;
  - `sapphire_bytecode`, a Luau compile and scan the cooker can link;
  - `sapphire_strings`;
  - `sapphire_loc`.
- **L3:**
  - `sapphire_text` (`parley`, `swash`, the glyph atlas);
  - `sapphire_body_pose`, the CPU pose evaluator that presentation and the simulation's bodies share.
- **Simulation and above:**
  - `sapphire_nav`;
  - `sapphire_script` (Luau);
  - `sapphire_sequencer`;
  - `sapphire_save` (L4);
  - `sapphire_stage` (L5), the one road from a simulation and cooked content to a `FramePacket`, which the sandbox, the game and the editor's viewport share.
- **L6, the game:**
  - `game_combat`, a fight's app side;
  - `game_ui`, HUD, touch, menus and the dialogue panel;
  - `game_app`.
- **Tools and dev:**
  - `sapphire_document`, the editor's change records;
  - `sapphire_soundgen`;
  - `sapphire_synthetic_zone`, dev only.

**`[simulation]`** now lists twelve crates: those above that a step runs, plus M2's. Their trees may hold no SDL, wgpu, renderer, world, anim, input or platform code. A new simulation crate joins the list and holds the same `clippy.toml`.

**`[cooker]`** keeps the renderer and world out of `sapphire_pipeline`, which is why `sapphire_bytecode` exists: the cooker can't link `sapphire_script`'s physics.

**Kinds and versions follow the landing order.** `AssetKind` numbers are contiguous, up to `VoiceLines` 25. `MoveSet` is version 9, `Zone` 5, the replay format 3 (4 **lands with G1**) and `CHECKSUM_LAYOUT` 8. A new kind or version takes the next number at its final rebase, and a renumbered kind needs `target/content/cache` cleared.

### Data path

- **New cooked kinds** (spec §5.10):
  - zone (`.szone`) and its cells' meshes;
  - navmesh (`.snav`);
  - encounter (`.sencounter`);
  - timeline;
  - Luau script (`.sscript`);
  - dialogue (`.sdialogue`);
  - font (`.sfont`) and font chains (`.sfontc`);
  - locale (`.sloc`);
  - UI screen (`.sui`);
  - kind of enemy (`.senemy`);
  - `Score`;
  - `VoiceLines`.
- **Packs:**
  - one per zone, `zone_<name>.spack`, holding what only that zone reaches (W1);
  - `fonts`, `loc` and `ui`.

  A pack's table lists each submesh's pipeline key (K1) and the names of clips and move sets (S3).
- **What the simulation reads is pinned:**
  - a zone's digest;
  - each script's bytes;
  - each timeline's and dialogue's simulation half;
  - each kind's mind.

  All go into `tables_hash`, so a replay can't run on other data. Zones load whole before the first step. Only presentation streams (plan §3, "The simulation streams nothing").
- **Compiled in:**
  - scenario documents, by `crates/sapphire_gameplay/build.rs`, which also embeds every `.luau` a kind names;
  - the cue table, `crates/sapphire_cues/default_cues.ron`, with `include_str!`.

  Editing either is a data edit that needs a rebuild.

### The combat stack

From the bottom up, on M2's ([m3-handoff.md](m3-handoff.md)). Spec §5.19, §5.21–§5.23 are the reference.

- **Factions, soft lock and fallen capsules** (B1).
- **Encounters** (B2): triggers fire events, waves come by `Start`, `Remaining(n)` or `After(ticks)`, and tokens gate who swings.
- **Kinds of enemy** (B3):
  - a kind's mind (brains by name, phases in thousandths of health) is the simulation's half;
  - its look is the stage's;
  - a boss has a bar and named phases.
- **Navigation** (P1–P3): a flat graph per zone, A* on a preallocated pool, a repath budget per tick, line of sight across the mesh, separation, patrols.
- **Scripts** (L1–L3):
  - one Luau VM, event hooks only;
  - commands into a fixed queue applied at one point in the step, and a hashed board of 64 numbers;
  - hot reload between steps.
- **Timelines** (Q1, Q3, Q3b): a cutscene holds the simulation at a step's edge, and a skip is recorded. A move may name a cinematic.
- **Energy** (Q3c): a fighter's meter and a move's cost.
- **Dialogue and flags** (Q2, Q2b): 256 `i32` flags, conditions asked of the VM between steps, and a choice's effect recorded.
- **Checkpoints and a run's start** (B4): a save holds a `ZoneStart`, and `Simulation::start_at` restores it.

### World and streaming

- **`sapphire_world::streaming`** (W2) activates cells by distance within a budget of N entities or M µs a frame. A hold (`Streamer::hold`) freezes it while an encounter runs. A hold only takes once the zone has caught up, so a fight that starts at once still gets its ground (C2 part 2).
- **Memory comes back** (W3): `AssetServer::unmount`, `Renderer::free_mesh` and `free_material` after the frames that read them, and generational handles.
- **Trips between zones** (W4): a zone's `exits`, a wipe that hides the load, and a prefetch from a hub. The simulation enters the next zone between steps, at a recorded tick.
- **Encounter volumes** (W5): spares of each kind stand before a trigger fires, so a wave's arrival builds nothing.
- **The pipeline inventory** (K1): `PipelineKey` per material slot, `Renderer::prewarm`, and `PipelineBuilds` counting every build. A bench fails on a first use.

### Tools

- **The editor** (E1–E5): `tools/sapphire_editor` is a docking shell over `sapphire_document`. Every edit is a change record that splices the fewest bytes, and undo returns the same bytes. It has:
  - the moves timeline;
  - the real renderer in a viewport;
  - play-in-editor from a snapshot;
  - zone, sequencer and screen panes.

  Nobody has opened its window.
- **Live tuning and live link** (protocol 6) carry:
  - moves, camera, cues, the S3 documents and screens;
  - a `.luau` reloaded between steps (`link push`, `link watch`).

  Kinds of enemy don't reach a running fight yet (see "Everything deferred").
- **Benches and lanes:**
  - `--bench` for a fight or a zone walk, failing on a first use;
  - the sandbox's `--ignored` replay lane;
  - `game_app`'s lanes, `reaches_the_fight`, `zone_trip` and `opens_the_guide`;
  - **lands with Z1:** the zone bench's flags and rows.
- **Look-dev for a new figure:** `--look-dev enemy --model <mesh>[:<clip>[@<tick>]]`.
- **Generators:**
  - `sapphire_pipeline gen-figures --specs tools/figures --out content` for figures from specs;
  - `tools/motion/` for key poses, plain Python;
  - `tools/sapphire_soundgen` for placeholder sound.

## How to add

M3's first pillar is that a new enemy, encounter, stage, boss, cutscene or screen is documents ([m3-plan.md](m3-plan.md) §2). The recipes below say where that holds. **A step that still edits Rust is marked "Rust step", and each is listed under "Everything deferred" as a bug against the pillar.**

Every recipe ends the same way:

- `cargo xtask content` to cook;
- `cargo xtask replay` for the replays;
- the sandbox's replay lane for the app;
- a commit;
- then x86-64's checksums from CI's artifact for any replay that's new or moved ([development-iteration.md](development-iteration.md), "Filling in x86-64's checksums"). That costs two CI rounds.

**Walked for real.** The enemy-kind and boss recipes were followed end to end on a scratch branch on 2026-11-09, with no Rust edit:

- a new kind on the Lancer's figure with moves and two brains of its own, in the ward's hub;
- a new boss on the Knell Colossus's figure at four fifths, with three phases and a script of its own, in the proving ground.

Both cooked and recorded replays. Both played beside the renderer to their checksums with no first-use build, and the captures showed each kind drawn and fighting. A one-off test read the replays:

- the kind changed brain at its phase;
- the boss's script changed its brain and forced its move on the crossing step;
- the boss's document changed its brain at its next phase.

`bosses_are_content.rs` held the new boss to exit test 2, its third clause on the scratch commit. Then the branch was deleted. One thing the walk found is in the recipes: phases and names count from 1 in a script. It also found that a zone could hold only one kind that named a script, and that every scenario on a zone had to list every kind the zone spawns. Both are fixed, as is a bug it found in the exit test (see "Everything deferred").

### A move

For the player, on a clip of its own ([m3-handoff.md](m3-handoff.md) has M2's version):

1. **Pose it.** Add a function in [tools/motion/moves.py](../tools/motion/moves.py) to its `MOVES`, and give it an ID in `frames.py`'s `IDS`. `--write` writes `<name>.keypose.ron` and its sidecar. `--write-numbers` puts `frames.py`'s draft frame numbers into the moves document. Read a sheet with `sheet.py`.
2. **Frame it.** Add a row to [player.moves.ron](../content/combat/player/player.moves.ron), last in `moves`, with:
   - `name`;
   - `startup`, `active` and `recovery`, adding up to the clip's length;
   - `motion`;
   - `hitboxes` on path `blade`;
   - `cancels`, `warps`, `cues`;
   - optionally `invulnerable`.

   A row has no intent. A press reaches a move through the top-level `intents` and `neutral`, or through another move's `cancels`. M3 added two optional fields:
   - `cost`, against the fighter's `energy` (Q3c);
   - `cinematic`, naming a timeline (Q3b). **Rust step:** only the name `ultimate` is loaded (`ULTIMATE_CINEMATIC` in `crates/sapphire_gameplay/src/replay/scenario.rs`).
3. **Rust step:** add it to `MOVES`, and to `ATTACKS` if it's an attack, in [player_moves.rs](../tools/sapphire_pipeline/tests/player_moves.rs). `each_move_plays_the_clip_of_its_name` fails without it.
4. **Give it a row in the graph:** [character.animgraph.ron](../content/lookdev/character/character.animgraph.ron)'s `clips` and `combat.moves`. Without it, `every_move_a_fighter_has_is_in_its_graph` fails.
5. **Give any new cue a row** in [default_cues.ron](../crates/sapphire_cues/default_cues.ron). Without it, `every_cue_the_recordings_publish_has_a_row` fails.
6. **Cook and rewrite the simulation's copy:**
   ```bash
   cargo xtask content
   SAPPHIRE_BAKE_MOVES=1 cargo test -p sapphire_pipeline --test player_moves the_simulations_copy
   cargo xtask replay --update
   ```
   The copy is `crates/sapphire_gameplay/baked/player.smoves`.

### A kind of enemy, with its brains

Walked. The worked examples are [content/combat/lancer/](../content/combat/lancer/) and [warrior.enemy.ron](../content/combat/enemy/warrior.enemy.ron).

1. **A figure, if it's a new body.** Write `tools/figures/<name>.figure.ron`: KayKit's joint names at the figure's own places, up to six painted regions, and the shapes along the bones, with the weapon weighted to `handslot.r`. Then:
   ```bash
   cargo run -p sapphire_pipeline --profile tools -- gen-figures --specs tools/figures --out content
   ```
   This writes `content/combat/<name>/`'s model and maps. The material, ramp, ILM and sidecars are hand-written after the Lancer's. Stand it with `--look-dev enemy --model <mesh>`.

   The walk reused the Lancer's figure and clips, so it started at step 3.
2. **Clips.**
   - Retargeted: add the mesh's ID as a `retarget:` target in the KayKit packs' sidecars (`content/third_party/kaykit_character_animations/**/*.glb.meta`). The `.glb` files themselves stay byte-identical.
   - Key-posed: a module per figure in `tools/motion/` (as `lancer.py` does through `figure_pose.py`) writes `.keypose.ron` documents with `--write`.
3. **Moves:** `<name>.moves.ron` with a `MoveSet` sidecar, as [lancer.moves.ron](../content/combat/lancer/lancer.moves.ron) is. It holds `intents` with no buttons, `fighter` (health, poise, capsule), `moves`, and the five reactions under the warrior's names, which a hit forces. Two kinds may share a moves document, and each plays a copy.
4. **Graph:** `<name>.animgraph.ron`, with a row for every move and reaction. Kinds that name the same mesh and graph share them.
5. **The kind:** `<name>.enemy.ron` with an `Enemy` sidecar ([lancer.enemy.ron](../content/combat/lancer/lancer.enemy.ron)). It names:
   - `moves`, by ID;
   - `brains`, by name. Each has spacing, pace, patience, rest, `attacks` pressing intents by name, and `rise`;
   - the starting `brain`;
   - `phases`, in thousandths of health, where a phase may name a `brain` to change to;
   - `look`: `mesh`, `graph`, `held` (`None` when the weapon is skinned in), `scale`, `height`, `eyes_below_crown` and `framed_radius`.

   The walk's kind had a second brain at its 500 phase, and changed to it on the hit that crossed it.
6. **Into a zone:** see "An encounter in a zone". A spawner's `spawns` names the kind by its file's stem.

### A boss, with phases and a script

Walked. The worked example is [content/combat/knell_colossus/](../content/combat/knell_colossus/), and B3's test boss is [captain.enemy.ron](../content/combat/enemy/captain.enemy.ron).

1. **A kind of enemy, as above,** with three more things in its `.enemy.ron`:
   - `boss: Some((name: "boss-<name>"))`, its bar's name key. The key has no string yet for any boss, and nothing checks one.
   - `phases` with names, `(at: 500, name: Some("tolling"))`. The hit that crosses a phase says its name as the boss's cue and changes the brain the phase names. The HUD's boss bar marks each phase.
   - `script: "<id>"`, naming a `.luau` by its sidecar's ID. The sidecar's importer is `Script(())`.
2. **The script.** It returns `names` and its hooks: `on_phase(actor, phase, health)`, `on_move`, `on_hit`, `on_defeat` and `on_beat` are in [sapphire.d.luau](../crates/sapphire_script/sapphire.d.luau).
   - **Phases count from 1, and so do places in `names`.** The walk's first try checked `phase == 0` and never fired.
   - Commands such as `sapphire.set_brain(actor, place)` and `sapphire.force_move(actor, place)` take a place in `names`.
   - A script is pure decisions, with no strings at run time (plan §3, "Luau's job"). The scan refuses string concatenation, and a bare `for` over a table.
3. **Into a zone, content only.** Add a spawner whose `spawns` is the kind, an encounter that calls it, and the scenario updates the next recipe names. The commit that does this must touch only `content/`, `tests/replays/` and `tests/golden/`, or `bosses_are_content.rs` fails. Its name must be in no `.rs` file, and its script must be reached from the zone.
4. **Kinds with scripts can share a zone.** Each kind's script hears only its own fighters: their phases, moves and defeats, the hits they land or take, and every beat. A fight's scripts share one VM, with its memory limit and each step's budget. They also share one board of 64 numbers, as they share flags. So a script that keeps numbers on the board should use slots that no other script in its zones uses.
5. **Hot reload:** in a `dev` build, `link push <file>.luau` reloads the script into the running fight between steps, and the board is kept.

### An encounter in a zone

Walked, as part of both recipes above.

1. **The encounter:** `<name>.encounter.ron` beside the zone, with an `Encounter` sidecar ([market.encounter.ron](../content/zones/lantern_ward/market.encounter.ron)). It holds:
   - `starts_on`, a trigger's event;
   - `faction`;
   - `tokens`, how many may swing at once, and `token_rest`;
   - `waves`, each with `when` (`Start`, `Remaining(n)` or `After(ticks)`), a `gap` between spawns, and `spawners` by name.
2. **In the zone:**
   - spawners for its fighters (`spawns` is the kind's stem);
   - a trigger (`Box` or `Sphere`) whose `event` starts it. A ground-level box must reach about 0.25 m under the paving, or feet never enter it;
   - its ID in `encounters`.

   The cook refuses a spawner off the navmesh, and an encounter naming a spawner or trigger the zone lacks.
3. **Every scenario on that zone** (`content/scenarios/*.scenario.ron` whose `zone` is it) must list the new encounter in `encounters`, and in `cleared` if it shouldn't run there. A scenario that doesn't fails to set up.

   Its kinds needn't be listed anywhere else. The zone's spawners say what it spawns, so a scenario adds each kind that its encounters' spawners name after the kinds it lists, with that kind's moves and script. Cleared encounters count too. The kind is the `<spawns>.enemy.ron` under `content/combat/`, which the build compiles in. A kind's name is its file's, so two `.enemy.ron` files with one name fail the build. A name with no kind fails the scenario's setup, naming the spawner.
4. **A scenario of its own:** a `cast` member with `moves: "player"` standing at a player spawner inside the trigger, as [ward-terrace.scenario.ron](../content/scenarios/ward-terrace.scenario.ron) does. Then record a replay:
   ```bash
   cargo xtask replay --record <name> --scenario <scenario> --script fight-back --ticks <n>
   ```
   The script is any row of `SCRIPTS`. **Rust step:** a new scripted input is a row in `crates/sapphire_gameplay/src/replay/script.rs`.
5. **Every replay on the zone moves,** because the zone's digest is in `tables_hash`. Re-stamp them with `cargo xtask replay --update`. The walk moved six. x86-64's lines are owed from CI.
6. **A golden** of the fight in its zone. **Rust step:** a golden scene is a row in `tools/sapphire_testkit/src/scenes.rs`, with the replay's `--arena --replay` capture and a tick. Exit test 2 allows the PNG in the boss's commit, and the row goes in a commit of its own, as the ward's did.

### A zone, with checkpoints and exits

The worked examples are [content/zones/lantern_ward/](../content/zones/lantern_ward/), [proving_hub/](../content/zones/proving_hub/) and [proving_ground/](../content/zones/proving_ground/). Spec §5.21 is the reference.

1. **The zone:** `content/zones/<name>/<name>.zone.ron` with a `Zone` sidecar. The cook finds it by its suffix and packs `zone_<name>.spack`. The `ZoneDocument` denies unknown fields:
   - `grid: (cell_size)`;
   - `placements` of prefabs by GUID (`.assembly.ron`), and `instances` of meshes;
   - `lights`;
   - `probes` (a `.probes.ron` by ID);
   - `day` and `rigs`, as relative paths;
   - `colliders` (by ID);
   - `navigation: Some((min, max, seeds))`, where `seeds` is optional;
   - `spawners`, `triggers`, `checkpoints`, `encounters`, `splines` and `exits`.
2. **Colliders:** `<name>.colliders.ron` with a `Colliders` sidecar. This is what bodies walk on and what the navmesh bakes from, since placements bring no colliders. A blocker needs its `groups` (`CharacterBlocker`, `CameraBlocker`); a `block_` or `camera_` prefix is only a convention, which a test holds for the arena alone.
3. **Checkpoints:** `(name, position, yaw_degrees, volume)`, reached outside a fight. A save names the latest. The cook refuses one off the navmesh.
4. **Exits:** `(name, to: "<zone id>", arrive: "<spawner there>", prefetch)`, with a trigger whose `event` is the exit's name. The cook reads the other zone to check `arrive`. A hub's `prefetch` brings the next zone in ahead of time.
5. **Look at it:** `--zone <name>[:<spawner>]` on the sandbox, with `--capture`. In the editor, `cargo xtask content --cook editor` first, then the zone pane moves placements.
6. **A scenario and a walk:** a scenario document on the zone, as above. **Rust step:** a replay that walks between zones names them by `BAKED_ZONES` and `ZoneEntry` constants in `crates/sapphire_gameplay/src/replay/`. A trip in the game itself is data.

### A dialogue, with story flags

The worked example is [ward_guide.dialogue.ron](../content/dialogue/ward_guide.dialogue.ron). Spec §5.23 is the reference.

1. **Flags:** add the names to [content/flags.ron](../content/flags.ron), at most 256. A flag is an `i32` that reads 0 until set. The cook refuses a flag that isn't listed.
2. **The dialogue:** `content/dialogue/<name>.dialogue.ron` with a `Dialogue` sidecar. It holds:
   - `start`, and `opens_on`, an event name;
   - `nodes`. Each node has `say: (speaker, line, vo)`, `set` flag writes, `next`, up to 4 `choices` (each with `line`, `to`, `when`, `set`) and `branch` arms with `otherwise`.

   A condition is a small language over flags: `flag.x >= 1 and not (flag.y == 2)`.
3. **Strings:** every key starts `dialogue-`. Add them to `content/loc/en/dialogue.ftl` and `content/loc/ja/dialogue.ftl`, and new Han to `content/fonts/ui.charset.ron`.
4. **Opening it:** a zone trigger whose `event` is the dialogue's `opens_on`. The fight holds while it's open.
   - **Rust step:** a scenario or replay that holds a dialogue adds it in code (`add_dialogue` in `crates/sapphire_script/src/scenarios.rs`).
   - **Lands with G1:** the game's flow loading the run's dialogues.
5. **Look at it:** `--ui-game dialogue --ui-dialogue <n>` previews the panel. **Rust step:** it always shows the ward's guide (`apps/sapphire_sandbox/src/screen.rs`).

   In the game: `cargo test -p game_app --features dev --test opens_the_guide -- --ignored`.

### A cutscene

The worked example is [court_gate.timeline.ron](../content/zones/lantern_ward/court_gate.timeline.ron). Spec §5.23 is the reference.

1. **The timeline:** `<name>.timeline.ron` with a `Timeline` sidecar. It holds:
   - `length`;
   - `clock`, which is `Real` for a cutscene (it holds the simulation at a step's edge) or `Steps` for a cinematic in a fight;
   - `camera`, with blends in and out and keys;
   - `characters`, by role, playing rows of their graph and placed by keys;
   - `cues`;
   - `post` offsets;
   - `subtitles` by key;
   - `time_scale` (not on a `Real` clock);
   - gameplay `events`.

   Every cutscene can be skipped. There's no field for it, since only the time scale and events reach the simulation.
2. **A new pose:** a `.keypose.ron` and its sidecar, named as a clip and a move in [character.animgraph.ron](../content/lookdev/character/character.animgraph.ron).
3. **Subtitles:** keys and speakers in `content/loc/{en,ja}/cutscenes.ftl`. `tools/sapphire_pipeline/tests/cutscene_strings.rs` holds them.
4. **Look at it:** `--arena --timeline <id>` plays one on the sandbox, and the editor's sequencer pane scrubs it.
5. **Starting it.** **Lands with G1:** the game's flow starting it at the court's trigger. Here, it's a Rust step: `crates/sapphire_gameplay/src/replay/ward.rs` compiles the gate in and plays it at a fixed tick, and its replays are rows in `script.rs`. Its goldens are rows in `scenes.rs`.

### A game screen

The worked examples are the six documents in [content/ui/](../content/ui/). Spec §5.17 is the reference.

1. **The screen:**
   - `content/ui/<name>.ui.ron` with its sidecar, naming its model `<name>.model.ron`, a list of `(path, kind)` that a cook holds every binding to;
   - nodes laid out by `taffy`, `key_from` for a key chosen at run time under a prefix;
   - `timelines`, `states` and `transitions`.
2. **Strings:** keys in `content/loc/{en,ja}/game_ui.ftl`, the Japanese under the file's machine-quality mark, and new Han in `content/fonts/ui.charset.ron`. Then run `cargo test -p game_ui --test strings --test menus`.
3. **Rust step, today: a new screen is code.**
   - `game_ui::menus::MenuScreen` lists the screens, with a model struct for each, `ALL`, `ids()`, `stem()` and the table of what each row does.
   - `game_ui`'s strings test holds the documents to that list.

   Changing an existing screen's look, layout, words or motion is documents alone: the editor's screen pane shows an edit on the next frame, and live link reaches a running game.
4. **Look at it:** run
   ```bash
   cargo xtask run --platform desktop -- --capture <png> --ui-plain --ui-game <name> --ui-locale ja
   ```
   with `--ui-pseudo` for pseudo-localization. Goldens at 4:3, 16:9 and 21:9 are rows in `scenes.rs`.

### Music by encounter phase, and voice lines

The worked examples are `content/audio/generated/music/lantern.score.ron` and `content/audio/generated/voice/{en,ja}.voice.ron`. Spec §5.15 is the reference.

1. **A score:** a `Score` document with these fields:
   - `tempo_bpm`, `beats_per_bar` and `bars`;
   - `stems`, each with its `.ogg` by ID;
   - `phases`, each naming the stems it plays;
   - `fade_beats`, `volume_db` and `duck_db`.

   Its sidecar's importer is `Score(())`. The phases a fight chooses are `waiting`, `running` and `cleared`. A phase change fades only the stems it changes, from the next bar line.
2. **Which score:** `music:` in [default_cues.ron](../crates/sapphire_cues/default_cues.ron), by ID. That's a data edit, compiled in.
3. **Voice lines:** each line is a `Cue` document on the `Voice` bus with its `.ogg`. Each language's `VoiceLines` document lists `(key, cue)`, and `voices:` in `default_cues.ron` names each language's document. A cue-table row speaks a line with `line: Some("<key>")` when the simulation publishes that row's cue. The voice language follows the player's language setting.
4. **Rust steps:**
   - A phase name other than the three, such as a boss's `tolling`, is chosen in `game_combat::cues::music_phase`.
   - A line spoken on anything but a published cue needs code.
   - A conversation's lines aren't voiced (see "Everything deferred").
   - New placeholder sounds are rows in `tools/sapphire_soundgen`. A recorded `.ogg` needs none.

## Rules the reviews made

### M2's, still binding

[m3-handoff.md](m3-handoff.md), "Rules the reviews made", has each with its test:

- every struct in a move document denies unknown fields, and so does every M3 document;
- `MAX_MOVE_REACH` and `MIN_SHAPE_EXTENT`;
- a defeat takes the stick;
- `LiveMoves::start` sets up on the edit;
- a frame's clocks go `alpha` of the way along their last step;
- a fight starting over ends what the last one set going;
- a live cue table can't name a sound nobody loaded;
- `PointerState` decides who has a desktop's pointer.

### M3's packages' own

Each is held by a test or a scan:

- **No allocation in a step:**
  - M2's gate 4 holds through every new system.
  - Around a script hook the VM's allocations count apart (`script_allocations`). A Rust allocation in a binding still fails.
  - Warming, streaming and the collector run between steps.
- **A script is pure decisions:**
  - Bindings take numbers only, and are generated from `crates/sapphire_script/api.toml` by `cargo xtask script-api`, with a test holding the output to the table.
  - A bad place, slot or actor is a fault that takes back the hook's commands.
  - Shipping stays `panic = "abort"`, so a binding never returns a Rust `Err`.
- **Nothing reads the OS's math:**
  - Pinned functions only, in the simulation, in Luau (libm builtins off, `-ffp-contract=off`) and in anything a cook bakes for it.
  - The town's nav input moved off `std`'s `cos` and `sin` for this.
- **A cook is the same bytes everywhere** for what the simulation reads: navmesh bakes, zones and scripts. CI's three OSes hold each pin.
- **Kinds are contiguous** in the order they land.
- **A golden or replay that moves** moves in the package that moves it, with the other backends' and architectures' values from CI.

### M3's reviews

**Lands with Z3:** the rules Z3's reviews make, as M2's did. Fresh agents are reading the renderer, the editor, the simulation and scripts, and content and streaming. The flow, UI and audio are read after G1 lands.

## Everything deferred

### Rust steps left in the "How to add" recipes

M3's first pillar counts each of these as a bug:

- **A golden scene** is a row in `tools/sapphire_testkit/src/scenes.rs`. Every recipe that wants a picture has this step.
- **A replay's scripted input** is a row in `crates/sapphire_gameplay/src/replay/script.rs`. Recording on an existing script needs none.
- **A new player move** joins `MOVES` (and `ATTACKS`) in `tools/sapphire_pipeline/tests/player_moves.rs`.
- **A cinematic other than `ultimate`** needs its name loaded in `replay/scenario.rs` and `game_combat::arena`.
- **A cutscene is started by code.** `replay/ward.rs` plays the gate at a fixed tick. **Lands with G1** for the game's flow.
- **A dialogue in a scenario or replay** is added in code (`sapphire_script::scenarios`). The sandbox's dialogue preview always shows the guide.
- **A new screen** is a `MenuScreen` variant with a model struct.
- **A music phase other than `waiting`, `running` and `cleared`** is chosen in `game_combat::cues::music_phase`. A boss's named phase is the natural next one. Its stems have to be written first.
- **A replay that crosses zones** names them by constants in `replay/`.
- **A placeholder sound** is a row in `tools/sapphire_soundgen`.

**Fixed while walking:**

- Exit test 2's third clause charged a boss to every zone that reached it through an exit. It now looks only in the zone whose spawner names it. The fix is the commit "Look for a boss's commit in the zone that has it".
- **One kind with a script per zone.** A fight bound one script, and a scenario had to list every kind its zone's spawners name. Now each kind's script is bound for its own fighters, in the fight's one VM, and a scenario on a zone adds the kinds its encounters spawn. The fix is the commit "Bind each kind's script for its own fighters". The Lantern Ward's six replays moved (the walk, the market, the terrace and the court's three), since the Knell Colossus's script no longer hears anyone else.

### Known to the run

- **Feel is unknown.** Nobody has played review 3, M2's fight or M3's slice. The maintainer started M3 without it. Every brain, encounter, number and clip is a first guess, and three enemies with tokens in a room have never been in front of a person.
- **Android plays no replay.** The windowed sandbox refuses `--replay` ("--replay needs --capture or --bench"), and Android has no offscreen entry, since it enters through `SDL_main`. What stands in is the game's scripted fight, `--open-fight --script fight-back`, whose checksum matched this Mac's.
- **Insets don't redraw after a hot reload or a pipeline rebuild.** A held picture keeps what it last drew (U6).
- **The editor's screen pane doesn't watch its model file.** An edit to a `.model.ron` doesn't reach an open pane.
- **Voice in conversations.** A1's seam is Q2b's `DialogueRunner::lines_shown` and each line's `vo` key. Nothing plays it, since that needs the keys mapped to a `VoiceLines` document and the fight given a way to reach the device's audio.
- **The editor's scenario choice.** Its viewport fights only `arena-fight`, so another kind never reaches it. **Live tuning of kinds:** neither a kind's `.enemy.ron` nor its graph reaches a running fight, only the warrior's graph. Each is about half a day (spec §5.22, C2 part 2's "Can't do, in the tools").
- **The window title isn't localized** (`AppMetadata`, "Sapphire").
- **`sapphire_stage`'s `pub` surface** is about 594 lines, of which a game names about 220. It wants a `samples` feature for the rest (spec §5.0, `sapphire_stage`).
- **Progress v1 holds one zone.** Lands with G1, with what else G1 leaves: a restart after a trip keeps the stage on its zone, and volumes aren't reset for a new run in the same zone.
- **E3 and E4's window checks** need a person: a lost device in a real window, docking, the gizmo and focus.
- **The emulator's audio** warns that real-time scheduling was refused. Nobody has listened.
- **The bodies' foot and look settings** reach the simulation's own bodies only as defaults. Taking a game's document is a simulation table in `tables_hash`, which moves every checksum (S3).
- **The script VM's heap isn't replayed.** Binding a dialogue's conditions and asking them between steps allocates in the fight's VM. A hook that hits the VM's 2 MiB memory limit fails, and the checksum holds how each hook ended. A replay asks no conditions. So a recording whose panel asked conditions could diverge from its replay near that limit. Spec §5.23's "Can't do" covers the garbage that asking leaves, not this.
- **No production path loads a pack's `.sscript`.** Dialogues carry their conditions' source and compile at bind (Q2b). A script could do the same (71 µs for `arena-scripted`). The cooked kind doesn't carry its script either, so a game reading kinds from packs needs a format change.

### From the spec's "Can't do" lines

Each is in its section, with what it would take:

- **Zones (§5.21):**
  - placements bring no colliders;
  - probes are one grid a zone;
  - warm reach isn't per encounter;
  - a hub's next zone is mounted again each visit;
  - the enemy a trip leaves is drawn nowhere.
- **Navigation (§5.19):**
  - nothing looks ahead, so two walkers meet before they part;
  - every brain checks every other over a flat list;
  - a scenario on a zone other than the arena has no navmesh path yet;
  - nothing loads a zone's patrols into the game.
- **Kinds and bosses (§5.22):**
  - a kind's held pose is the warrior's turns;
  - `arena::framing` frames everyone at the warrior's size;
  - a brain can't keep apart from allies or circle while it waits for a token;
  - enemies have no meter;
  - the boss's awaken and turns are unused.
- **Encounters (§5.22):**
  - one side per encounter;
  - `ReturnTo` names a checkpoint by its place in the zone's list.
- **Timelines and cinematics (§5.23):**
  - a cinematic's run goes on if its move is cut short;
  - a freeze can't be counter-scaled;
  - the editor's live run plays a set without its cinematic;
  - a `TodCutscene` isn't a track.
- **Dialogue (§5.23):**
  - a dialogue's keys aren't added to font subsets;
  - script writes aren't held to the registry;
  - quests don't step.
- **Music (§5.15):**
  - stems decode whole, about 6.1 MB for the score;
  - no horizontal re-sequencing or stingers;
  - one Language setting drives text and voice.
- **Renderer (§5.12):**
  - the mesh pool's buffers never shrink;
  - no LOD selection;
  - the inventory's warm-up blocks a load, which M4's warm-up screen and driver cache answer.
- **Scripts (plan L2):** no real type check of `sapphire.d.luau`, since there's no `luau-analyze` here.

### Carried from M3's handoff, unchanged

[m3-handoff.md](m3-handoff.md), "Everything deferred", has the detail:

- **Compile-time iteration:** no hot-patched Rust.
- **Sound** isn't hot-reloaded.
- **D3, D4 and N3–N5's limits:** statics fixed once built, a crossing judged by its end pose, `Jump_Start` and side dodges unplayed, no get-up clip, limits beating colliders, springs with no floor.
- **O2's and O3's reads, owed to a person.**
- **Camera framing, counted not judged.**
- **Balance:** mashing `light`.
- **Phone UX:** an iPhone has no Back, and nothing auto-pauses on return.
- **Z1's M2 recommendations:**
  - parallel recording on a phone;
  - a screen-effects share in §3.3's post row;
  - per-cook texture size;
  - the character's 11 opaque materials against 4;
  - footprint in `frame stats`.
- **CPU rows moving together ×2.6–3 between runs,** unexplained.
- **Toolchain:** wgpu stays on 30 and SDL on 3.4.18. Reassess at this boundary.
- **Renderer:**
  - reflection probes for characters, and SSR;
  - cut-ins, halftone, C7's patterns;
  - full PSO warm-up (M4), upscalers, local-light shadows, characters shadowing each other;
  - depth of field, light shafts, water.
- **Assets:** memory-mapped packs, mip streaming, a shared cache, a cook-on-save watch.
- **Live link's pairing token.**
- **Store and signing (M4),** crash upload, weather per zone, the entity inspector, nightly device runs, and gate 5.
- **Plan §3's "Not in M3":** XPBD cloth, ragdolls, dynamic props, decals, GPU particles, lip-sync, a real second character, patching and accounts.

## Known limits and risks

- **Feel is unknown, and M3 built a game on it.** All of M2 and M3 could be well built and still not fun. The answer is cheaper to act on now, since a move, a brain, an encounter or a boss is a document. If it's the off-ramp, the Rust systems in "What M4 builds on" stay, and `game/` and the editor are what go ([m3-plan.md](m3-plan.md) §6).
- **More bugs are likely.** **Lands with Z3:** what its reviews found. Before them:
  - walking this handoff's recipes found two (exit test 2's zone and the phase numbering a recipe has to state);
  - the exit run's rehearsal found one (a capture hung on a zone trip);
  - most packages fixed something they found below them.
- **No phone has run anything,** after four milestones. Every mobile number is an M5 at the Mobile tiers' settings, the iOS Simulator or the Android emulator. Parley, ICU, Luau and zones land on phones untested.
- **Motion without an animator,** now for two enemies, a boss, a cutscene and an ultimate. The fallback is still a commissioned animator or a bought pack, and a pack that bars redistribution can't go in a public repo.
- **Machine translation** in a language nobody here reads.
- **Our own navigation runtime** is about 1,200 lines to own, polygons only, with no dynamic obstacles.
- **A VM in the step** makes gate 4 narrower than M2's: zero allocations everywhere but around a hook, where a counted budget holds.
- **A moved checksum costs two CI rounds,** because no machine here runs x86-64. Adding a kind to a zone moves every replay on that zone.
- **GitHub's macOS runners were out** for long stretches during M3, on 2026-11-09 among others. Landings then were verified by the local stand-in, not by CI's macOS jobs.
- **Software rasterizers aren't GPUs,** and the character shader is full (M2's handoff).
- **Scenarios are compiled in,** up to about 100 KB of zone document (B4). A game with large zones reads them from packs, which needs the cooked kind to carry its script.
