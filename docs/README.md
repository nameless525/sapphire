# Sapphire docs

Every doc in `docs/`, grouped by what you'd read it for. New here? Start with [AGENTS.md](../AGENTS.md) at the repository root, then the first group below.

## Start here

| Doc | What it holds | Read it when |
|---|---|---|
| [../AGENTS.md](../AGENTS.md) | The repository's map: crates by layer, the gates a change passes, the rules CI enforces, common tasks, gotchas, a glossary | First, always |
| [getting-started.md](getting-started.md) | Requirements, running on each platform, the sandbox's controls, the debug overlay, hot reload and live link, offscreen captures, everyday commands, troubleshooting | Before building or running anything |
| [architecture.md](architecture.md) | How it fits together: the layers, a frame, the content pipeline, determinism, the test pyramid, CI | Before your first change |
| [conventions.md](conventions.md) | How code reads: workspace rules, docs, errors, unsafe, time, platform code, logging, GPU structs, tests | Before writing code |
| [development-iteration.md](development-iteration.md) | The smallest useful check for each kind of edit; replays and their checksums; tuning a fight; the timeline editor; build profiles | Whenever you need to prove a change |

## Architecture and spec

| Doc | What it holds | Read it when |
|---|---|---|
| [engine-spec.md](engine-spec.md) | The design reference: scope and engine rules (§1), platforms (§2), tiers and budgets (§3), architecture (§4), a section per crate with "As built" notes (§5), the cross-platform checklist (§6), layout, profiles and xtask commands (§7), CI (§8), quality gates (§9), milestones (§10), open questions (§11) | Before changing a crate: read its §5 section, not the whole file |
| [adr/0001-crate-layering.md](adr/0001-crate-layering.md) | Why the layers are numbered as they are, and crate naming | Before adding a crate or moving code between layers |
| [adr/0002-platform-entry-and-packaging.md](adr/0002-platform-entry-and-packaging.md) | SDL3 main callbacks everywhere, iOS as a plain executable, Android as a cdylib, the portable draw path, headless verification | Touching platform entry, bundling or the draw path |
| [adr/0003-ios-frame-pacing.md](adr/0003-ios-frame-pacing.md) | The engine's display link driving SDL's callbacks on iOS | Touching iOS pacing or frame timing |
| [adr/0004-android-frame-pacing.md](adr/0004-android-frame-pacing.md) | The choreographer on SDL's thread for Android pacing | Touching Android pacing or frame timing |

## Working in the code

| Doc | What it holds | Read it when |
|---|---|---|
| [m5-handoff.md](m5-handoff.md) | Where things stand; "How to add" recipes (a move, enemy, boss, encounter, zone, dialogue, cutscene, screen, golden, patch, GPU, tier, setting); rules the reviews made; everything deferred; known risks | Adding content or features; picking up the next milestone |
| [blender-mask-bake.md](blender-mask-bake.md) | Baking the anime character shader's SDF face map, ramps and ILM map, from documents or headless Blender | Authoring a character's masks |
| [playtest.md](playtest.md) | Watching someone play a playtest build and turning the session into answers that point at a file | Running or reading a playtest |
| [optimization-pass-two.md](optimization-pass-two.md) | The second iteration-speed pass: watcher, packing, tests, captures, CI fan-out, with measurements | Changing build, test or CI speed |
| [privacy.md](privacy.md) | Keeping private data out of the repository; the privacy guard and its hooks | Before your first commit |

## Shipping and stores

| Doc | What it holds | Read it when |
|---|---|---|
| [store/apple.md](store/apple.md) | The App Store checklist and store text for the iOS archive | Preparing an App Store submission |
| [store/google-play.md](store/google-play.md) | The Google Play checklist and store text for the AAB | Preparing a Play submission |
| [store/steam.md](store/steam.md) | The Steam checklist, store text and Steam Deck self-check for the desktop packages | Preparing a Steam release |
| [store/privacy-policy.md](store/privacy-policy.md) | The game's privacy policy, a draft in English and Japanese | Changing what the game collects, or the in-game privacy page |

## Milestone history

Each milestone has a plan (work packages and their "As built" notes), an exit run (the runbook for its exit tests) and a handoff (what the next milestone inherits). Read them for why something is the way it is; the newest handoff is the current map.

| Doc | What it holds |
|---|---|
| [m1-plan.md](m1-plan.md) | M1, the data path and the anime look: the look's pillars (§2), third-party content rules (§3), work packages |
| [m1-exit-run.md](m1-exit-run.md) | M1's exit-test runbook: desktop, simulator, emulator and phones |
| [m2-plan.md](m2-plan.md) | M2, the combat slice: the headless simulation's crate rules (§3), work packages |
| [m2-exit-run.md](m2-exit-run.md) | M2's exit-test runbook, with the feel reviews |
| [m2-go-no-go.md](m2-go-no-go.md) | The go/no-go report at M2's end: carry on in Rust or take the off-ramp |
| [m2-handoff.md](m2-handoff.md) | What M2 inherited from M1 |
| [m3-plan.md](m3-plan.md) | M3, world and tools: zones, navigation, sequencer, Luau, UI, saves, localization, the editor |
| [m3-exit-run.md](m3-exit-run.md) | M3's exit-test runbook |
| [m3-handoff.md](m3-handoff.md) | What M3 inherited from M2 |
| [m4-plan.md](m4-plan.md) | M4, ship readiness: exit tests (§1), decisions (§3, patches among them), work packages by track (§4), what's owed to the maintainer (§7) |
| [m4-exit-run.md](m4-exit-run.md) | M4's exit-test runbook: what to measure, and what only a device or an account proves |
| [m4-handoff.md](m4-handoff.md) | What M4 inherited from M3, with the first "How to add" recipes |
| [m5-handoff.md](m5-handoff.md) | What the next milestone inherits from M4 (also under "Working in the code") |

`docs/media/` holds screenshots the older docs show.

## Research

Background reading behind the packages: web research, with each claim marked unverified where it is. Read the folder of the area you're working in before redesigning it.

| Folder | What it covers |
|---|---|
| [../reports/](../reports/) | The original report on a Rust engine for anime action games |
| [../research_notes/Rust anime action game engine/](../research_notes/Rust%20anime%20action%20game%20engine/) | The first round: architecture, rendering and GPU, gameplay libraries, Rust's viability, tooling and platform libraries |
| [../research_notes/Anime look for M1/](../research_notes/Anime%20look%20for%20M1/) | Character rendering, world lighting and post, content and CI crates |
| [../research_notes/Combat slice for M2/](../research_notes/Combat%20slice%20for%20M2/) | The animation runtime, content and audio |
| [../research_notes/World and tools for M3/](../research_notes/World%20and%20tools%20for%20M3/) | The editor, navigation, scripting, text, zones |
| [../research_notes/Ship readiness for M4/](../research_notes/Ship%20readiness%20for%20M4/) | Accessibility, the three stores, patching, symbols, thermal, and M4's spikes' findings (H0, P0, S0, T0) |
