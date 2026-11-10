# M4 plan: ship readiness

- **Status:** planned (2026-11-10). That day the maintainer asked for M3 to be finished and M4 started, and told the run to carry on without asking them anything. Nobody has played M2's fight or M3's slice, so whether any of it is fun is still unknown (§6, §7). Every decision here was taken by the agent who wrote this plan, and each can be undone by saying so.
- **Milestone:** M4 (spec §10). It builds on M3 as it lands at 4fe253e6, with every M3 package built, Z2's final exit run still measuring, and nothing judged by a person.
- **Inputs:**
  - The spec: §2 platform matrix, §3 tiers and budgets, the module specs (§5.1, §5.2, §5.10–§5.12, §5.15, §5.17, §5.18, §5.25–§5.27), §6's cross-platform checklist, §7 build, §8 CI, §9 gates, §10 milestones and §11 open questions.
  - [m4-handoff.md](m4-handoff.md).
  - Seven scouts' notes under [research_notes/Ship readiness for M4/](../research_notes/Ship%20readiness%20for%20M4/): `apple.md`, `google-play.md`, `steam.md`, `thermal.md`, `accessibility.md`, `patching.md` and `symbols.md`. They're web reading on 2026-11-10, with nothing built or run, and they mark many of their facts unverified. A package that leans on one checks it against its source first, and the spikes (§4) exist to do that.

## 1. What M4 is for

M4 answers one question: **can what M3 built leave this machine as a product?** Some games aren't products however well they play:

- one a store won't take;
- one that stutters the first time it draws something;
- one that can't be fixed after release;
- one that overheats a phone;
- one that shuts some players out;
- one that crashes without a trace.

M4 builds what stands between `game_app` and a player: warm-up, patching, tiers that answer heat, symbols, accessibility and store packaging.

The slice stays M3's: the Lantern Ward, from the title to the Knell Colossus. M4 adds no content and tunes no fight. It ships that game packaged for three stores and five platforms, patchable, and readable when it breaks.

### Exit tests

Each test says what this run can prove and what it can't. The run has no phone, no Windows or Linux machine, no desktop GPU but this Mac's, and no account anywhere. It may not open a window, sign in, upload or publish.

1. **No shader hitches (from the spec).** The test is a scripted playthrough from empty pipeline caches:
   - the title and a new game;
   - the Lantern Ward's whole walk, with its cutscene, conversation, encounters and boss;
   - a language switch, and every setting that changes what's drawn;
   - a tier change at a pause, and a forced walk down and back up the thermal ladder;
   - back to the title.

   It must build no pipeline on the frame's thread after the boot screen's first frame, and count no first use. Loads keep drawing: on Metal, from a cold cache, no frame of a load screen takes more than twice the tier's interval.
   - **Proved here:** Metal on this Mac, cold and warm; lavapipe and WARP in CI, by counts; the Simulator and the emulator in the exit run, by counts, with times from the Mac's GPU.
   - **Owed:** a phone driver's cold compile, a desktop GPU's driver cache, and a store-installed build's first launch.
2. **Patches download only changed chunks (from the spec).** The test covers a fixed set of edits: a texture, a mesh, a sound, a string, a screen and a zone's look. For each, a patch from the last release's install downloads exactly three things: the new manifest, the changed packs' tables, and the blocks whose hashes are new. A localhost server counts the bytes. A download that's killed resumes. One that's corrupted, older or expired is refused. The game then starts on the old version or the new, never on a mix.
   - **Proved here:** all of it, on the desktop, and on the emulator and the Simulator against a server on this Mac.
   - **Owed:** a real CDN, the maintainer's release key, and TLS against a real certificate on each OS's trust store. Here the platform verifier is only shown to refuse a certificate it shouldn't trust.
3. **Builds pass TestFlight, Play internal testing and Steam review (from the spec).** The run builds a shipping package of the game for each store:
   - for iOS, an `.xcarchive`;
   - for Android, an AAB;
   - for Steam, depots for Windows, Linux and macOS.

   Each passes every validator that runs without an account (S2–S4 name them). A checklist for each store gives each rule's source and the test or command that meets it.
   - **Proved here:** that half.
   - **Owed:** distribution signing, the uploads, notarization, and each store's own server-side checks and review. All of it needs the maintainer's accounts (§7).
4. **The look at every tier, scale and rung (new).** The character's close-up, the ward, and a hit's post each get goldens at each tier, at each tier's dynamic floor and at each rung of the thermal ladder. A GPU test holds outline widths in output pixels, and the shading's two-tone edge. The ladder walks down and back up without oscillating or hitching, on modelled traces and under the emulator's thermal override.
   - **Proved here:** Metal, lavapipe, WARP and the emulator's override.
   - **Owed:** a phone thermally throttled after 15 minutes, which is M2's exit test 4, still open.
5. **Every crash is readable (new).** A forced crash in a shipping build symbolicates offline to its function, file and line, from symbols kept from the same link:
   - on macOS here;
   - on Windows and Linux in CI;
   - on the Android emulator, from its signal record and tombstone;
   - on the iOS Simulator, from its panic report.

   Every shipped binary is stripped, and its ID matches its symbols.
   - **Owed:** a crash on a device, and the stores' own crash reports.
6. **Combat that more people can play (new).** Each item of §3's accessibility list is a setting. Each is saved and migrated, reached by pad, keyboard and touch, and held by a test of what it changes. With flashes reduced, the scripted fight passes WCAG 2.3.1's general and red flash tests. The smallest text is at least 12 px at 1280×800 at the default size, in both languages. Every cue that colour carries stays apart under simulated protanopia, deuteranopia and tritanopia.
   - **Owed:** players who need these settings, and a reader of Japanese.
7. **Still authorable (pillar 1, carried from M3).** The "How to add" recipes have no Rust step but one: showing a kind of value no model holds, which is code by design.
   - **Proved here:** all of it.
8. **Ready to play (new).** One command builds, installs and starts a playtest build on each target the maintainer may hold. A played session comes back as a replay that reaches its checksum here, with a report.
   - **Proved here:** sessions driven by scripts on the desktop, the Simulator and the emulator.
   - **Owed:** someone playing.

**Feel stays unknown.** No test here asks whether the fight is fun, and agents can't say. M4 builds nothing that assumes the answer. It makes the answer cheap to get, through track F.

## 2. What "done well" means

Ranked. Each has a measure a test or a bench can read.

| # | Pillar | Measure |
|---|---|---|
| 1 | **The look holds everywhere.** Every tier, scale, ladder rung and accessibility setting keeps the anime character shading, the outlines and the grade. | Goldens per tier, floor and rung. Outline widths in output pixels within a stated tolerance of scale 1. No rung or setting changes the character family's pipelines. |
| 2 | **Nothing hitches.** No frame builds a pipeline after the first, and loads keep drawing. | 0 first-use builds and 0 frame-thread builds in exit test 1 on every backend. Load-screen frames within twice the tier's interval on Metal from a cold cache. |
| 3 | **Safe to update.** A patch is signed, checked block by block, resumable and atomic. A bad or old patch is refused, and the game still starts. | Fault injection at every step of a download and an install. Bytes fetched equal changed blocks plus manifest plus tables. |
| 4 | **Playable by more people.** The guidelines' list for action combat. | Settings held by tests, the flash analysis, legibility floors and the colour simulation (exit test 6). |
| 5 | **Honest about the stores.** Every rule has its source and a check, or is listed as owed. | The checklists. Every local validator passing in the nightly package job. |
| 6 | **Readable when it breaks.** | A symbolicated forced crash on every platform this run can reach. |
| 7 | **Small and cool on a phone.** New runtime dependencies pay for themselves, and heat is answered before the OS answers it. | Binary size per package on Android and iOS. The ladder's trace under the emulator's thermal override. |

## 3. Decisions this plan takes

Where a decision changes what the spec says, the spec says so with this plan, as a "Decision" bullet in its section. §11's answers below go in with it. The package that builds a decision adds its "As built" note there.

| Topic | Decision | Why | To undo |
|---|---|---|---|
| **The go-ahead and feel** | M4 starts in Rust with nobody having played anything. It adds no content and tunes nothing. Track F makes the maintainer's playtest cheap: a playtest install keeps every run as a replay with its stats and the player's marks, and one command brings it back and reads it. | The maintainer's instruction. The run can't play, so the best it can do is turn thirty minutes of someone's play into data agents can act on. | If review 3 takes the off-ramp, the engine-side work stays: warm-up, patching, tiers, symbols and packaging serve any game. What goes is in `game/`. |
| **What a hitch is** | A frame that builds a pipeline or shader module on the frame's thread, or waits on one, after the boot screen's first frame. Load screens count. | K1's `PipelineBuilds` already sees every build, and only a build on the frame's thread stalls a frame. A frame-time gate alone is noise on CI's rasterizers. | Add a frame-time gate once devices measure. |
| **Warm-up** | Pipelines build on job workers behind screens that keep drawing. A load waits for its pipelines; nothing is drawn hidden. A hub's prefetch warms on workers too, where today it builds a pipeline a frame on the frame's thread (M3 W4). | The spec's "an object stays hidden until its pipelines are ready" would show as a pop. K1's inventory already knows everything a load needs, so the load can wait behind the boot screen or the wipe. A cold Metal compile took 6–7 s on an M5 (K1). | Hide-until-ready for an object whose key nothing declared, counted as a first use. |
| **Driver caches** | Vulkan keeps the persisted cache it has (`sapphire_rhi::pipeline_cache`). Metal and DX12 rely on the OS's and the driver's own caches. | wgpu 30 has no other route: in wgpu-hal 30.0.1's source, `create_pipeline_cache` on Metal and DX12 returns an empty object, and Metal binary archives are detected but never used. This answers Q5's Metal part. | A wgpu that exposes binary archives plugs into the same `PipelineCacheDesc`. |
| **What a patch moves** | The pack's stored block: 64 KiB, compressed on its own, named by its blake3. No content-defined chunking, unless P0's two real cooks say an edit shifts too many blocks. | Packs are already block-addressed (§5.10), so an unchanged asset's blocks are byte-identical between cooks. CDC would mean chunking uncompressed data and compressing again on the client. SteamPipe's 1 MB chunks diff our packs as they are (`steam.md`). | CDC inside an asset, listed in the same manifest as a block list per asset. |
| **What's signed** | One release manifest, signed with ed25519 (`ed25519-dalek` 3, `verify_strict`). It holds a version that only grows, an expiry, the minimum app build, and each pack's name, size and header hash. The binary embeds the root keys, and a signed successor rotates them, one hop at a time. Packs stay unsigned: their header's signature block stays reserved. In a shipping build, a pack mounts only if the current manifest names its header. | One signature covers the whole set (mix-and-match) and its version (rollback), and the expiry covers a freeze. TUF's four roles are more than one publisher needs, and `tough` brings an HTTP stack of its own (`patching.md`). | Sign packs in the reserved block. Adopt TUF's roles. |
| **What a patch may carry** | Presentation only: meshes, textures, sounds, strings, fonts, screens and a zone's look. What the simulation reads ships in the binary: scenarios, kinds, scripts, the player's baked moves, and every document `sapphire_gameplay`'s build script compiles in. `xtask package` refuses a patch that changes any of it. No code is ever downloaded. | It's all compiled in today, so a pack's copy would split the simulation from what's drawn. App Review's 2.5.2 is ambiguous about downloaded bytecode (`apple.md`). A store update is an acceptable price for a single-player game, and the rule answers Q10 for now. | Move the simulation's data into packs (m4-handoff, "Scenarios are compiled in"), then lift the refusal where a platform allows it. |
| **HTTP** | `ureq` 3, blocking, on a patcher thread of its own, with `rustls` on the `ring` provider and `rustls-platform-verifier`. Not `reqwest`, which the spec's Appendix A named. Tests run against a localhost server written in the tests. | A few range requests don't need tokio and hyper in a phone's binary (pillar 7). A server of our own can inject drops, short reads, bad bytes and an ignored `Range`. | The transport is a trait, and `reqwest` slots in. |
| **Where content lives** | Everything installs with the app, and patches come from a host the maintainer chooses. No Play Asset Delivery and no Apple-hosted background assets in M4. | The last cook is about 27 MB for mobile and 37 MB for desktop, and the game's release binary 27.5 MB (M3 S2). That's far under every limit the notes found. Apple-hosted packs need iOS 26 while the floor is iOS 17 (`apple.md`, `google-play.md`). This answers Q11 for now. | Play's install-time asset packs keep the `AAssetManager` read path, so moving content there changes no reader. |
| **iOS packaging** | xtask assembles the `.xcarchive` itself, with no Xcode project. The maintainer distributes it from Xcode's Organizer, or with `xcodebuild -exportArchive`. | One build route, and no project file for agents to keep in step. The spec wanted an Xcode project for sentry-cocoa, entitlements, the privacy manifest and archives. Crash upload is deferred, and xtask can write the rest. S0 checks that Xcode accepts such an archive up to the signing it can't do. | Generate a minimal Xcode project around the same binary, if S0 or the maintainer's first upload says so. |
| **Android packaging** | An AAB from the same Gradle project (`bundleRelease`), with content in the base module. AGP's `debugSymbolLevel` carries native symbols. The upload key is the maintainer's and enters through the environment; local checks sign with the debug key. Agents run Gradle `--offline`: anything not in its cache stops the package, and the orchestrator decides. | Play requires an AAB. Gradle's Maven artifacts are downloads, and the run's rule on downloads covers them. | Content moves to install-time asset packs (above). |
| **Steam** | Native builds for all three desktops. Linux builds in the Steam Linux Runtime 3.0 SDK container, in CI only, which pulls it as it pulls WARP. No Steamworks SDK in M4. A Deck runs the native Linux build: Q3's answer, with a Deck's check owed. | SDL3 and Vulkan are native, and CI already tests Linux. Review doesn't need the SDK, and downloading it takes an account. | Ship Windows only and let Proton run it, a depot setting. |
| **Symbols and crashes** | Symbols come from the shipping build itself: the dSYM, the PDB, and the unstripped ELF with an explicit build ID. `dump_syms` and `minidump-processor`, linked as libraries by xtask, make and read a Breakpad store offline. The stores get their symbols in the archive and the AAB. Crash upload stays deferred, since there's no account. On Android API 31 and later, the app reads its own tombstone at its next launch (`ApplicationExitInfo`). | Symbols from a rebuild don't match. The tombstone changes Q12's answer: symbolicated native crashes need no Sentry SDK. | `sentry-cli` takes the same files once there's a project to send to. |
| **Keys** | No key file is committed. Tests derive their keys from fixed seeds in code. Release keys stay with the maintainer and enter through the environment at package time. A shipping package refuses to embed a test key's public half, and a test holds it. | [privacy.md](privacy.md) keeps signing material out of tracked files. | None needed. |
| **The ladder** | The response to heat spends what the look can spare first. The rungs are data per tier in `tiers.ron`: 1. resolution down to 0.85; 2. optional passes off (volumetric fog, AO, the cirrus, foliage's wind levels, the sky's face rate, the particle cap, a bloom level); 3. resolution down to the tier's floor; 4. a 30 fps cap; 5. a lower tier at the next pause or wipe, never in a fight. A rung comes back only after a hold (about 10 s, and 60 s for the frame rate). No rung touches the character family, the outlines, the ramps or the grade. Every rung's pipelines are declared. | Spec §6.3's ladder put resolution first. On the phone tiers there's no temporal upsampling, so a bilinear upscale softens the hard two-tone edges and lines the look depends on (M1 R6 argued the same). A tier change builds a renderer. Thresholds come from Google's headroom guidance (`thermal.md`). | Edit the rungs in `tiers.ron`. |
| **Dynamic resolution** | On by default in the game on every tier, at today's floors (0.7 on desktop, 0.75 on phones), unless T0 finds a floor that breaks the look. The sandbox and the goldens keep it off. | The controller exists and is measured (M1 R6). Off, it does nothing for heat. | One setting, which the player also has. |
| **Heat readings** | Android: thermal status, the headroom forecast, and an ADPF hint session from API 33, all resolved at run time as the status is today. iOS: `thermalState` and Low Power Mode. Desktops: none. Each maps onto one `ThermalReading`, and a dev cvar forces any reading. | Spec §6.3. The emulator's `cmd thermalservice override-status` already reaches the log (M2 Z3), so it can drive a test. | None needed. |
| **Accessibility's scope** | §6.8's list, plus separate volumes, mono, camera motion, a flash limiter, a first-launch photosensitivity notice, a remap screen, hold or toggle, cues by shape, and a timing assist (cuttable). Not in M4: a screen reader for menus, audio description, or difficulty beyond the assist. | What the guidelines ask of action combat (`accessibility.md`). | Each is a setting row. |
| **Flashes** | WCAG 2.3.1 is the test, with its three-a-second rule and its general and red flash thresholds. With flashes reduced, the scripted fight must pass. The default is measured and reported, and a first launch offers the setting. | Anime combat is flash-heavy by design. Impact frames are what trip the test, and no store requires one (`accessibility.md`). Capping the default would change the look the plan's first pillar protects, so the player chooses. | Make the reduced setting the default. |
| **Colour** | Every cue that colour carries also has a shape or an icon. There's no full-screen recolour filter. Tests simulate colour vision with Machado et al. (2009). | A recolour filter shifts every colour of the art and still leaves pairs confusable (`accessibility.md`). | None needed. |
| **Settings and the checksum** | A setting that changes what a step reads enters the run's tables and its recording, as `TargetingTuning` does: auto-target strength and the timing assist. Everything else is presentation and never reaches a checksum, and a test holds that. Defaults equal what the committed replays ran on. | Gates 2 and 4. | None needed. |
| **Versions** | One version, the workspace's. The build number is the count of commits on `main` at packaging, and it feeds every platform's fields, the manifest and the pipeline cache's build ID. It replaces `[package.metadata.sapphire]`'s hand-raised `build`. | Stores need a number that only grows. `main` lands only by fast-forward, so its count only grows, and nobody has to remember to raise it. | Back to the hand-raised field. |
| **CI** | Shipping builds, packaging, symbols and validators run nightly, on demand, and on PRs that touch packaging. No PR job grows. I1 buys back the time CI's build jobs are short of (m4-handoff, "Known to the run"). | Fat LTO and five packages would put every PR past its limits. A timed-out job reads as skipped. | Promote a nightly step to PRs when it's cheap. |
| **Pins** | wgpu stays on 30.0.1, egui on 0.37 and bevy on 0.20.1, the newest of each on 2026-11-10. SDL 3.4.20 is out (`sdl3-sys` 0.7.3), but SDL stays on 3.4.18 unless a package needs a fix from it. | Spec §5.6. SDL's internals sit under compile-time assertions, so an upgrade is a review of its own. | A package that needs SDL 3.4.20 takes it and re-reads the assertions. |
| **Accounts and publishing** | Nothing in M4 signs in, uploads, notarizes or publishes. Every such step is in §7 with its command ready. | The run's rules. | The maintainer's. |

### Spec §11's open questions

- **Answered with this plan, without devices:**
  - **Q5's Metal part:** no binary archives in wgpu 30 ("Driver caches").
  - **Q10:** no code is ever downloaded ("What a patch may carry"). A live-service plan would reopen it.
  - **Q11:** content far under every limit ("Where content lives").
  - **Q3:** native Linux, with a Deck's check owed ("Steam").
  - **Q12:** revised for the tombstone ("Symbols and crashes").
- **Still open, all needing devices:** Q4 (Windows' default backend), Q9 (device floors, throttled) and Q14 (Android pre-rotation), and Q5's MSAA and timestamp measurements on phones.

### From m4-handoff's deferred list

- **Taken into M4:**
  - The Rust steps left in the recipes (track R).
  - CI's build jobs near their limits (I1).
  - The inventory's blocking warm-up (H1, H2).
  - Secondary motion's worst frame (H5, cuttable).
  - The phone's missing Back, and nothing pausing on return (S6).
  - The game ending its run on a lost device (S6).
  - "Store and signing (M4)" (track S).
- **Waiting, because none of it stands between the slice and a store:**
  - voice in conversations;
  - Progress v1's one zone;
  - the script VM's heap in replays;
  - per-script boards;
  - loading a pack's `.sscript` (which "What a patch may carry" keeps unneeded);
  - the editor's scenario choice and live tuning of kinds;
  - the spec's "Can't do" lines for zones, navigation, kinds, encounters, timelines, dialogue and music;
  - memory-mapped packs and mip streaming;
  - live link's pairing token (live link isn't in a shipping build);
  - nightly device runs and gate 5, which need devices.

**Not in M4:**

- crash upload and telemetry;
- accounts and online services;
- iOS background downloads (`URLSession`);
- Play Asset Delivery and Apple-hosted packs;
- the Steamworks SDK (achievements, overlay, cloud saves);
- upscalers and HDR;
- a screen reader;
- patching the simulation's data;
- Windows code signing;
- the Mac App Store;
- consoles;
- what the M3 plan's "Not in M3" left out: XPBD cloth, ragdolls, dynamic props, decals, GPU particles, lip-sync and a real second character.

Every device check [m4-handoff.md](m4-handoff.md) lists stays owed.

## 4. Work packages

Each package is one pull request into `main`, or a few. **Sizes are an agent's:** S is a few hours, and M about a day. Nothing here is larger: anything that was got split. Each finished package gets an "As built" note in its row and in its spec §5 section.

The usual rules carry over:

- Existing goldens stay byte-identical unless a row says otherwise.
- A new scene commits only a Metal golden.
- A feature builds its pipelines only when its work can run, and declares them to the inventory.
- Every system publishes a stat counter (R9) against its budget.
- No checksum moves unless a row says so.
- Each new document struct denies unknown fields.
- A rule from [m4-handoff.md](m4-handoff.md)'s "Rules the reviews made" binds whatever it touches.
- A package that adds a setting a player can reach adds its row to H3's walk.

### Track R: recipes with no Rust (pillar 1's bugs, first)

| ID | Package | Size | Depends on | As built |
|---|---|---|---|---|
| R1 | **Golden scenes are documents.** The rows of `tools/sapphire_testkit/src/scenes.rs` become a document under `tests/golden/` that the testkit reads and a test holds whole: capture arguments, tick, pins and gates, and the shard cost table. A new golden is a row and its PNG. No golden moves. | S | Plan | |
| R2 | **Scripted inputs are documents.** `crates/sapphire_gameplay/src/replay/script.rs`'s scripts (timed buttons, `PlayTimeline` at a tick) become `.inputs.ron` documents found by name. The gameplay build script compiles them in as it does scenarios, so a headless runner still needs no assets. Every committed replay keeps its checksums. | M | Plan | |
| R3 | **Shelved zones are found.** The build script builds `BAKED_ZONES` from the zones that scenarios name in `shelf`, with their navmesh bakes, as it finds kinds of enemy. A replay of a new trip needs no Rust. | S | Plan | |
| R4 | **Placeholder sounds are documents.** `tools/sapphire_soundgen` reads `.sound.ron` specs, as `gen-figures` reads `tools/figures/`, and its outputs stay byte-identical. Each R package takes its step out of m4-handoff's recipe. What's left is a screen showing a value no model holds, which is code by design. | S | Plan | |

### Track I: CI

| ID | Package | Size | Depends on | As built |
|---|---|---|---|---|
| I1 | **CI's time, back.** First read every job's times over recent runs. Then cut the PR's longest path: `cargo-hakari` or a shared build where the numbers say so, Lint's two clippy passes as parallel jobs, and the Windows golden build's duplicate compiles. Every PR job ends under two thirds of its limit on a typical run. | M | Plan | |
| I2 | **Nightly packages.** A scheduled, dispatchable workflow, which PRs run only when they touch packaging. It builds the game's shipping packages for all five platforms, runs every local validator (S2–S4) and writes the symbol store (C1), all kept as artifacts. | M | S2, S3, S4, C1 | |

### Track H: hitches (exit test 1)

| ID | Package | Size | Depends on | As built |
|---|---|---|---|---|
| H0 | **Spike: warming while frames draw.** Some of this is known. wgpu 30 builds pipelines from several threads without a lock across the compile, and error scopes are per thread (spec §5.17, M1 A8 follow-up 2). `Renderer::new` already builds batches on the job pool (`BuildOn`), and a shader edit already builds on a worker and swaps in at `prepare` (`shader_builds`). What isn't known, on Metal, lavapipe, WARP, the Simulator and the emulator: frame times while a warm-up's batches build beside a drawing frame; a cold Metal cache from a fresh bundle ID, and how long it stays warm; the Vulkan cache across runs on lavapipe; what one shader module costs the CPU; and how a worker reports a build wgpu refused. Its report changes this plan if it must. | S | Plan | |
| H1 | **Warm-up off the frame.** `Renderer::prewarm` becomes a warm-up on the job pool, built as `shader_builds` builds an edit: owners hand over steps, workers build, and the frame's `prepare` takes in what's done. The frame polls it for progress. Boot, `Content::open_zone` and a hub's prefetch all warm through it. `PipelineBuilds` tells a worker's build from the frame thread's. After boot's first frame, a frame-thread build counts as a hitch in `RenderStats` and fails a bench as a first use does. | M | H0 | |
| H2 | **Screens that draw while it warms.** The game boots to a screen drawn by the interface's one pipeline, built first, with progress shown, while the global pipelines and the first zone's packs warm. A zone's wipe holds until its pipelines are warm. Both run at the tier's rate from a cold cache. Their look is a first draft. | M | H1 | |
| H3 | **Everything a run can reach is declared.** A test walks the settings a player can reach, from their documents: tiers, rungs, flashes, subtitles, language, insets. It holds every pipeline they draw with to the inventory, so a setting that switches a variant can't land undeclared. | S | H1 | |
| H4 | **The scripted playthrough.** A `zero_hitches` lane in `game_app` that plays exit test 1's walk and fails on any first use or frame-thread build. CI runs it on lavapipe. Locally it runs on Metal from a cold cache, alone on the machine, and also reads load-screen frames. | M | H2, H3, T3, A2 | |
| H5 | **Secondary motion's solve, spread.** The clearance pass's solve frames hold p99 over §3.2's row on every tier (m4-handoff, "Known to the run"). Their work spreads over the frames the pass already skips. The first to cut in this track. | S | Plan | |

### Track P: patches (exit test 2)

| ID | Package | Size | Depends on | As built |
|---|---|---|---|---|
| P0 | **Spike: what a patch costs.** Two cooks of the repository, with one edit between them or none: a texture, a mesh, a sound, a string, a screen, a zone's placement. It reads the share of each pack's stored blocks that stay byte-identical, and whether a cook repeats itself byte for byte. It also checks `ureq` 3 with `rustls` on `ring` and the platform verifier on five targets: whether it builds, the bytes it adds, and what Android's verifier needs in Gradle. | S | Plan | |
| P1 | **The release manifest.** `sapphire_assets::manifest`, as §3 has it, with a shipping mount that refuses a pack the manifest doesn't name. `xtask content` writes an unsigned manifest. `xtask package` signs one with a key from the environment and lays out a directory any static host can serve. A hostile or corrupt manifest is an error, never a panic: every byte gets flipped. Cooks repeat themselves byte for byte, if P0 found they don't. | M | P0 | |
| P2 | **`sapphire_patch`: the download.** It compares the installed manifest with a new one and plans the blocks it lacks, by hash. It fetches them with HTTP range requests on a thread of its own, checks each block's blake3 before writing it, and resumes from a state file after a kill. It caps every read at the manifest's sizes, and refuses an older or expired version. Tests run it against a localhost server that drops, truncates, corrupts, ignores ranges and stalls. | M | P1 | |
| P3 | **`sapphire_patch`: the install.** New packs assemble beside the old, from kept and fetched blocks, under names by hash. They verify whole, then become current by one rename of the manifest pointer, through `sapphire_platform`'s durable writes. The old set goes after the next good launch. A kill at any step leaves an install that starts on one whole version, proved by fault injection as V1's saves were. `xtask package --patch-from <manifest>` refuses what §3 says a patch may not carry. | M | P2 | |
| P4 | **Patching in the game.** The boot screen checks for an update, shows progress, and resumes at the next launch. Downloaded packs mount over the installed ones. On iOS they go in Application Support, kept out of iCloud backup; on Android, in internal storage. The emulator (through `adb reverse`) and the Simulator patch from a server on this Mac. Exit test 2's lane counts bytes per edit. | M | P3, H2 | |

### Track T: tiers, resolution and heat (exit test 4)

| ID | Package | Size | Depends on | As built |
|---|---|---|---|---|
| T0 | **Spike: the look at every tier and scale.** The character's close-up, the ward's street and a hit's post: at each tier, at each tier's dynamic floor, and with each optional pass off. It commits new Metal goldens and measures outline widths in output pixels and the two-tone edge at each. It reports which floors keep the look, and what each optional pass saves on Desktop-Low's and Mobile-Low's settings. | S | Plan | |
| T1 | **Heat and power, read.** `sapphire_platform` gets each platform's readings, as §3's "Heat readings" lists, as one `ThermalReading` with stats. A dev cvar forces a reading, and the emulator's thermal override reaches it. | M | Plan | |
| T2 | **The ladder.** A pure controller in `sapphire_render::quality` over the rungs `tiers.ron` lists, fed by `ThermalReading` and by dynamic resolution's pressure, with its holds on the way back. Tested on modelled traces (heating, oscillation, recovery) and on the emulator's override. Each rung gets a golden, from T0's scenes. | M | T0, T1, H1 | |
| T3 | **Tiers in the game.** `game_app` follows the ladder. Settings gain document rows for quality (automatic or a tier), a frame cap (30, 60, 120 or none on desktops; 30 or 60 on phones), dynamic resolution and a battery saver. A tier change waits for a pause or a wipe and warms the new tier's pipelines on workers first. The desktop cap paces as §6.2 sketches. A Steam Deck starts on Desktop-Low at 1280×800. | M | T2, S6 | |
| T4 | **A table of desktop GPUs.** Tier selection reads a data table of known adapter names before its RAM rule, so a GTX 1060 with 16 GB no longer counts as Desktop-High. It's seeded from vendors' public specs, with nothing measured. The first to cut. | S | Plan | |

### Track A: accessibility (exit test 6)

| ID | Package | Size | Depends on | As built |
|---|---|---|---|---|
| A1 | **Settings that last.** `PlayerSettings` version 2, migrated from 1. It adds subtitles (on, size, background, speakers), text and interface scale, shake, flashes, camera motion, separate music, effects and voice volumes, mono, auto-target strength, and hold or toggle per action. They're rows in `menus.ron`, on screens in both languages, with goldens at three aspects, walked by pad, keyboard and touch. The settings screen's goldens move, on purpose. | M | Plan | |
| A2 | **What the settings change.** Shake scales the camera's cues and rumble. Flashes scale post's screen effects. Camera motion damps punches and zooms. Volumes and mono reach the mixer's buses, and the scale reaches the HUD. Subtitles get their size and box, and voice lines in a fight get captions. A test measures every screen's smallest text at 1280×800, 1920×1080 and a phone's size, in both languages: at least 12 px at 1280×800 at the default size, and 9 at the smallest (`steam.md`). | M | A1 | |
| A3 | **Flash safety.** An analyser in `sapphire_testkit` runs WCAG 2.3.1's general and red flash tests over a capture sequence, by tiles at any size. With flashes reduced, post's impact frames, inversions and blurs cap their luminance swing, and the cue dispatcher allows at most three full-screen flashes a second. Then the scripted fight and a stress fight both pass. The first launch shows a photosensitivity notice that offers the setting. | M | A1 | |
| A4 | **Remapping, and hold or toggle.** A remap screen for the pad and the keyboard through `capture_next`, with conflicts shown and a reset. Bindings save over the compiled-in document, and glyphs follow. A `Toggle` trigger in `sapphire_input` that a held action takes from the settings. The simulation sees the same `TickActions`, so no checksum moves. | M | A1 | |
| A5 | **Cues by shape.** Every cue that colour carries gets a shape or an icon, from documents: attack telegraphs, a perfect dodge's window and the boss's phases. A debug view simulates the three dichromacies. A test holds each pair of telegraphs apart under all three, by shape where colour fails. | M | A1 | |
| A6 | **Timing assist.** A run setting that widens the perfect-dodge and counter windows, recorded into the run's tables so replays hold. The first to cut in this track: nobody has judged the windows it widens. | S | A1 | |

### Track S: stores (exit test 3)

| ID | Package | Size | Depends on | As built |
|---|---|---|---|---|
| S0 | **Spike: what each store's tools accept here.** Whether `xcodebuild -exportArchive` takes an archive xtask assembled, up to the signing it can't do. `actool` compiling an icon into `Assets.car` from the command line. Scanning a linked binary for required-reason APIs. `gradlew bundleRelease --offline` and lint, from Gradle's cache alone. `bundletool validate`, `build-apks --local-testing` and an install on the emulator. A macOS bundle signed ad hoc with the hardened runtime, run headless. How Windows takes a manifest and an icon on CI. | S | Plan | |
| S1 | **`xtask package`, one version, one identity.** The command every store builds through: the shipping profile, a staging directory, and outputs per platform. The version and build number go everywhere §3 says. Bundle IDs and display names come in both languages, as placeholders the maintainer replaces. An icon of the project's own, drawn by a generator, is compiled for every platform. | M | S0 | |
| S2 | **iOS: the archive.** A shipping `.xcarchive` holding: Info.plist's store keys (the SDK's `DT*` keys, `ITSAppUsesNonExemptEncryption`, the games category); `PrivacyInfo.xcprivacy`, with a test that fails on a required-reason API the binary links and the manifest doesn't declare; entitlements; `Assets.car`; and the dSYM in `dSYMs/`. It's checked offline with `plutil -lint`, `codesign --verify --strict --deep` and the archive's structure. The same recipe's Simulator build runs the title, headless. | M | S1 | |
| S3 | **Android: the bundle.** An AAB as §3 has it, with S1's version code. It's checked offline: `bundletool validate`; `build-apks --local-testing`, then an install and a capture on the emulator; release lint failing on error; `aapt2 dump badging`; `zipalign -c -P 16` and the library's segment alignment; Back and edge-to-edge at target API 36, on the emulator. | M | S1 | |
| S4 | **Desktop builds for Steam.** Windows: an executable with its manifest (per-monitor DPI), icon and version. Linux: a build whose glibc symbols are no newer than Steam Linux Runtime 3.0's, a `.desktop` file and an icon. macOS: a bundle signed with the hardened runtime and no entitlement it doesn't need, and an `.icns`. Depot layouts and SteamPipe build scripts are written as text. Deck: its glyphs, every screen captured at 1280×800, the pad walk, no launcher and no text entry. | M | S1 | |
| S5 | **Checklists and store text.** `docs/store/apple.md`, `google-play.md` and `steam.md`: each rule with its source and date, and the test or command that meets it, or "owed". Drafts, in both languages, of each store's text, the privacy policy ("collects nothing"), Play's data safety and content rating answers, Apple's accessibility labels and Steam's Deck self-check, each tied to track A's tests. Store screenshots are captures at each store's sizes, made by one command and not committed. | M | S2, S3, S4, A5 | |
| S6 | **The game as a phone app.** A trip to the background pauses a fight, and the return shows a tap to continue (§6.5). An iPhone reaches the pause with no Back. On the way out, saves flush and audio suspends. A lost device rebuilds the renderer and reloads, where today it ends the run. | M | Plan | |

### Track C: crashes and symbols (exit test 5)

| ID | Package | Size | Depends on | As built |
|---|---|---|---|---|
| C1 | **Symbols from the same link.** Per target, from the shipping build itself: the dSYM, the PDB, or the unstripped ELF with an explicit build ID, beside a stripped binary. `xtask symbols` writes a Breakpad store with `dump_syms` as a library, and checks each stripped binary's ID against its `.sym`. | M | S1 | |
| C2 | **Every crash, read here.** `xtask symbolicate` reads: a desktop minidump, with `minidump-processor` offline; an Android signal record, and from API 31 on, the tombstone the app reads from `ApplicationExitInfo` at its next launch; and an iOS panic report. A forced crash in a shipping build names its function, file and line on macOS here, on Windows and Linux in CI, on the emulator and on the Simulator. | M | C1 | |

### Track F: feel, made cheap to judge (exit test 8)

| ID | Package | Size | Depends on | As built |
|---|---|---|---|---|
| F1 | **Sessions keep themselves.** A playtest install is the shipping game with session keeping turned on. It keeps under `user://` every run's recording, frame stats, heat readings, settings and version, and the marks the player sets from the pause menu. `xtask playtest --platform desktop\|ios-sim\|ios\|android` installs it, and `xtask playtest pull` brings sessions back. A session replays here to its checksum. | M | Plan | |
| F2 | **What a session says.** `xtask playtest report` gives, per encounter: its time; damage taken and falls; dodges, perfect dodges and counters; presses a second and idle time; each mark, with its tick to replay from; frame times and heat. `docs/playtest.md` carries m2-exit-run's review sheet to M3's slice and M4's settings, with the document and field each answer points at. | S | F1 | |

### Track Z: exit

| ID | Package | Size | Depends on | As built |
|---|---|---|---|---|
| Z1 | **Shipping benches.** Measures binary sizes, stripped and not; cold and warm starts to the title (Metal, Simulator, emulator); warm-up per tier; memory; patch sizes for P0's edits; and the ladder's trace under the emulator's override. They go into spec §3 as "Measured as shipped". | M | H4, P4, T3, S2, S3, S4 | |
| Z2 | **The exit run.** `docs/m4-exit-run.md`. It's rehearsed headless on the desktop, the Simulator and the emulator before the last packages land, then run on the head. | M | All above | |
| Z3 | **Reviews.** Fresh agents read each area after it lands and fix what they find, as M3's six did. The areas: the patcher, read as an attacker would; packaging, against the checklists; warm-up and tiers; accessibility; crash capture. | M | All above | |
| Z4 | **Handoff.** `docs/m5-handoff.md`, spec §10's M4 status, and the stores' next deadlines. | S | Z2, Z3 | |

## 5. Order

The packages fall into five phases. Within a phase, packages that don't depend on each other run as parallel PRs.

1. **Prove it, with the plan:** the spikes H0, P0, S0 and T0, track R, I1, H5, T1, T4, A1, S6 and F1. All of them can start on the first day. Their reports change §3 if they must, in a docs commit before the packages that lean on them.
2. **Foundations:** H1, P1, S1 and A2–A6.
3. **Build out:** H2, H3, P2, P3, T2, S2, S3, S4, C1 and F2.
4. **Assemble:** P4, T3, C2, S5, I2 and H4.
5. **Exit:** Z1, Z2, Z3 and Z4.

**Critical path:** P0 → P1 → P2 → P3 → P4 → Z1 → Z2 → Z4. The heat chain, H0 → H1 → T2 → T3 → H4 → Z1, is as long, so both start on day one and neither has slack.

**Six agents at once.** Phase 1 alone has fifteen packages, so the orchestrator starts the two chains' heads first (P0, H0, T0, S0), then fills from the rest.

**Load:** the Mac also runs the gates. Heavy work queues for a slot (`slot`). These run alone, through `slot --all --load 8`: shipping builds with fat LTO, Metal's cold-cache runs (H0, H4, Z1) and every bench. At most one simulator or emulator is booted.

**If time runs short, cut in this order:**

1. T4;
2. A6;
3. H5;
4. F2's report (keep the sheet);
5. S5's screenshots;
6. the Linux depot, so Steam ships Windows and macOS.

**Never cut** H1, H2, H4, P1–P4, T1–T3, A1–A3, S1–S4, S6, C1, C2 or F1. The spec's row names them, or the playtest needs them.

**Landing:**

- Goldens move only where a row says so: A1's settings screens. T0's and T2's scenes are new, with Metal goldens only, and the other backends' come from CI.
- No row moves a checksum. A6 moves `tables_hash` only for a run with the assist on.
- Packaging and symbol PRs prove themselves through I2's dispatch before they land, since PR CI doesn't run them.

## 6. Risks

| Risk | Mitigation |
|---|---|
| **Feel is still unknown,** and M4 packages a game nobody has played. | Track F makes the playtest one command and turns it into replays and a report. Nothing in M4 tunes the fight, so a bad answer costs M4 nothing it built. |
| **No accounts:** every store's last step is the maintainer's, and the stores' own checks see what local ones can't (Apple's server-side validation, Play's pre-launch report, Steam's review). | Checklists with each rule's source, S0's spikes, and a fix round budgeted after the maintainer's first upload of each. |
| **A hand-made archive refused** by Xcode's Organizer. | S0 tests `-exportArchive` as far as it goes without signing. The fallback is a generated Xcode project around the same binary. |
| **A patcher is security code,** and easy to get subtly wrong. | One signed document, `verify_strict`, sizes capped by the manifest, pack names validated so none can name a path, fault injection at every step, and Z3's attacker's read. |
| **A cook that doesn't repeat itself** re-downloads everything. | P0 measures it, and P1 makes cooks repeat byte for byte. |
| **No phone, still.** The ladder, ADPF and a phone driver's cold compile land untested on a device. The emulator likely reports no headroom. | Modelled traces, the emulator's thermal override, and the Simulator's and emulator's cold starts. Nothing here makes m4-handoff's device list shorter. |
| **Gradle offline, and Steam's runtime only in CI.** A cache gap stops S3, and the Linux depot is proved only by CI. | S0 finds gaps first, and the orchestrator decides on each. The sniper build is a CI job like WARP's. |
| **CI grows with shipping builds.** | Nightly and on demand only, after I1 buys time back. |
| **Store rules change every year:** Play's target API (36 now, 37 expected in 2027), Apple's SDK minimum (Xcode 26 since 2026-04-28), Steam's review times (up to 12 business days since November 2026). | The checklists carry dates and sources, and Z4's handoff names the next deadlines. |
| **Accessibility built by people who don't need it.** | Settings follow the guidelines' items and are held by tests. A review by players who need them is owed (§7). |
| **Disk and load:** shipping builds per target add to every agent's `target/`. | The briefing note's disk and slot rules. Shipping builds run in I2's CI and in packaging packages only. |

## 7. Needed from you

Nothing here blocks the work, and it will carry on without an answer. All of it is owed from you whenever you're back:

1. **Review 3 and the slice,** now through a playtest build (F1): one command per platform, and the sessions come back as data. It's still the most useful thing anyone could do for this project.
2. **The accounts, and the steps only they allow.**
   - **Apple:** the Developer Program (an Apple Distribution and a Developer ID certificate, provisioning, App Store Connect's app record and agreements), then the TestFlight upload from the archive, internal testers, and notarization of the macOS build.
   - **Google:** a Play Console account, the app record, Play App Signing and an upload key, the data safety and content rating forms from S5's drafts, then the AAB upload to internal testing. A new personal account needs a closed test of 12 testers for 14 days before Production.
   - **Steam:** Steamworks, the app fee, tax and bank details, the store page, depots uploaded with `steamcmd`, and review.
3. **The identity:** the game's name, its bundle IDs (permanent once uploaded), the icon, and an age rating. The run uses placeholders.
4. **A release key and a host for patches:** an ed25519 key kept offline, and a static host or CDN for the directory `xtask package` writes.
5. **Devices:**
   - a phone of each OS: the ladder throttled, a cold compile, a store install's first launch, a crash;
   - a Steam Deck, for the Verified self-check;
   - a Windows or Linux GPU.
6. **People:** a reader of Japanese, now for the store text too, and players who use the accessibility settings.
7. **The decisions above.** Each stands until you say otherwise. The ones most worth a look:
   - patches carrying presentation only;
   - no Xcode project;
   - native Linux on the Deck;
   - dynamic resolution on by default;
   - the default flashes left as they are, with the reduced setting offered;
   - no CDN service picked.
