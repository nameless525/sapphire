# Rust viability for a custom 3D stylized anime character-action game engine (state as of October 2026)

Scope note: research done 2026-10-24. "Verified" here means read on the cited page during this research pass. Items marked (opinion) are the author's or a developer's view, not fact. Items marked (2024) or (2025) may be outdated. Secondary or aggregator sources are flagged as such.

## 1. What commercial or notable 3D games have shipped with Rust, and what did they report?

### Takeaway
Rust has shipped real, well-reviewed commercial games, but they are small-team indie titles (Tiny Glade being the flagship). I found no publicly confirmed AAA or large-scale 3D action game built on a Rust engine. The biggest Rust-gamedev studio, Embark, ships its actual games (ARC Raiders, The Finals) on Unreal Engine 5. Its Rust work (rust-gpu) has been handed to the community.

### Cited Findings
**Tiny Glade (Pounce Light): the strongest Rust 3D success story**
- Two-person Swedish studio (Anastasia Opara, Tomasz Stachowiak), who are also the credited programmers. Full release 23 Sept 2024 on Windows/Linux. **macOS was added in an update on 13 May 2026.** No console version is listed. It got over 800k Steam wishlists before launch and was nominated for the BAFTA Games Awards 2025 Technical Achievement. Built with "a custom version of Bevy." — [Wikipedia: Tiny Glade](https://en.wikipedia.org/wiki/Tiny_Glade)
- Interview of 30 May 2024 (may be outdated): the prototype began on Bevy, and they are "still using a modified version of Bevy." They use Bevy's ECS for game logic, but rendering outgrew Bevy and Anastasia wrote custom rendering. They started on OpenGL through Bevy, which Tomasz found unshippable, then moved to Vulkan for compute shaders and Linux and to control "pretty much the whole pipeline." It began as a weekend project while Anastasia worked full-time at Embark Studios. — [80.lv interview](https://80.lv/articles/exclusive-tiny-glade-developers-discuss-bevy-proceduralism-publishers-cozy-games/)
- (opinion, Tomasz Stachowiak, 2024) Rust is ergonomic and "doesn't crash in your face," and compares favorably with C++. Anastasia was productive in two days. Rust "has a bit of a reputation for being difficult to learn," and Bevy suits people not "scared of heavy engineering." — [80.lv](https://80.lv/articles/exclusive-tiny-glade-developers-discuss-bevy-proceduralism-publishers-cozy-games/)
- I found no console port announcement for Tiny Glade. One undated article says there is "no word of a console port." — [Insider Gaming](https://insider-gaming.com/can-you-play-tiny-glade-on-nintendo-switch/) (undated, secondary)

**Embark Studios**
- Embark's `rust-gpu` repo was archived on 31 Oct 2025 and moved to community ownership under the Rust-GPU GitHub org. The archived page says it is still used by Embark for Embark-specific work. — [GitHub EmbarkStudios/rust-gpu](https://github.com/EmbarkStudios/rust-gpu)
- ARC Raiders is built on a modified Unreal Engine 5, chosen for its editor tools and world-building (World Partition, Runtime Virtual Texturing). Backend services come from Epic Online Services. — [Unreal Engine developer interview](https://www.unrealengine.com/en-US/developer-interviews/embark-studios-build-the-award-winning-arc-raiders-with-unreal-engine); [80.lv](https://80.lv/articles/how-embark-studios-used-unreal-engine-to-create-arc-raiders)
- I found no evidence that Rust is part of ARC Raiders' shipped client (search, not exhaustive).

**Bevy-based commercial releases (from Bevy's lead, Aug 2026)**
- Released Steam games: Toroban, Gob Johnson's Downhill Marmalade, Simulo, Court Wizard, Insanio. Wee Boats has a demo. Willcaster and One Planet are announced. Exofactory, Jarl, Polders and Unhaunter are in progress. Non-game users include Design Once (a manufacturing platform that renders with Bevy), Foresight and Nominal. — [Bevy's Sixth Birthday, 10 Aug 2026](https://bevy.org/news/bevys-sixth-birthday/)

**Hytale (not Rust, checked as requested)**
- Released in early access 13 Jan 2026. — [Wikipedia: Hytale](https://en.wikipedia.org/wiki/Hytale)
- Secondary sources (hosting blogs, unofficial) say Riot's C++ rewrite was cancelled in mid-2025. After the founders re-acquired the game in Nov 2025, they went back to the legacy engine (described as a C# client and Java server), because the C++ rewrite was about two years behind on gameplay features. — [Supercraft wiki](https://supercraft.host/wiki/hytale/legacy-engine-reversion); [Contabo explainer (DE)](https://contabo.com/blog/de/hytale-erklaert-sandbox-rpg-von-hypixel-studios/). The language split is reported inconsistently, so treat it as unverified.

### Inferences
- Every Rust 3D game shipped so far is far smaller in scope than WuWa or ZZZ. Tiny Glade is a cozy diorama builder with no combat, characters, animation-heavy content or online services. Its success was driven by two very senior graphics engineers who used only Bevy's ECS and wrote their own renderer. That is evidence that **Rust plus parts of Bevy plus a custom renderer** works at indie scale. It is not evidence about big content-heavy action games.
- Tiny Glade took about 20 months after the Windows/Linux launch to ship on macOS (Sept 2024 to May 2026), and there is no announced console version. Even a successful Rust title did not reach all platforms quickly.
- Hytale is a C++ story, not a Rust one, but it shows a general lesson: an engine rewrite can fall years behind on gameplay features.

### Gaps
- I did not verify in this pass whether Tomasz Stachowiak wrote Embark's `kajiya` renderer, or whether kajiya is archived. Search gave only an unofficial mirror that lists it as a public "experimental real-time GI renderer."
- I did not verify release details for Veloren, Fish Folk, Thetawave, Tunnet or The Gnorp Apologue (no primary source fetched).
- I found no sales figures for Tiny Glade or any Bevy title from primary sources.
- I found no 2025-2026 release of a 3D character-action game built in Rust.

## 2. Postmortems and critiques: what are the strongest arguments against Rust gamedev, and what has changed since?

### Takeaway
The best-known critiques, LogLog Games (Apr 2024) and Architect of Ruin (Apr 2025), are about **iteration speed, churn and missing high-level tooling**, not performance. Both teams went to Unity. Bevy has since added experimental hotpatching (0.17, Sept 2025) and the BSN scene macro (0.19, Jun 2026). As of Aug 2026 it still has no editor, and it still breaks APIs every release. So the core critique is only partly addressed.

### Cited Findings
**LogLog Games, "Leaving Rust gamedev after 3 years" (26 Apr 2024; may be partly outdated)**
- A two-person indie team with well over 100k lines of Rust over 3+ years. — [Stacker News repost](https://stacker.news/items/520201); [original](https://loglog.games/blog/leaving-rust-gamedev/)
- (opinion) Core claims:
  - The borrow checker forces refactors at inconvenient moments, even on throwaway code.
  - Prototyping speed matters more than maintainability for indie games.
  - Workarounds (events, command buffers, reserved entity IDs) scatter gameplay logic across the codebase.
  - True hot reloading is essential, and Rust's options (e.g. `hot-lib-reloader`) are unreliable compared with .NET/Unity.
  - Bevy's all-in ECS, including UI, and its parallel system scheduling add ordering overhead for little gain in game code.
  - Rust lacks UI libraries for stylized, animated game UI.
  - Proc macros and serde slow incremental builds.
  - The orphan rule should be optional for application crates.
  - Global state is practical in single-threaded game logic, while dynamic borrow checking (RefCell, ECS queries) crashes at runtime after refactors.
  - The community values hype and tech purity over shipped games.
  - Outcome: moving back to Unity, citing hot reload as a key reason. — [loglog.games](https://loglog.games/blog/leaving-rust-gamedev/)
- Hacker News discussion is item 40172033 (comments not retrieved). — [HN](https://news.ycombinator.com/item?id=40172033), linked from [D forum](https://lists.puremagic.com/pipermail/digitalmars-d/2024-May/340586.html)

**Architect of Ruin (Brandon Reinhart, deadmoney.gg, ~25 Apr 2025)**
- (opinion and experience) Moved from Bevy/Rust to Unity/C# for five reasons:
  - collaboration: slow onboarding of a non-programmer sibling;
  - abstraction: Rust's low level made gameplay prototyping slow;
  - migration: frequent Bevy API changes and regressions, such as a sprite rendering issue;
  - learning: AI tools gave poorer guidance for fast-moving Bevy;
  - modding: no clear scripting story and an unstable ABI.
- A 3-day experiment in Jan 2025 was followed by a ~6-week full port. Code "shrank substantially," mostly from removed ECS boilerplate. Iteration felt faster and migration anxiety went away. The one loss was localization: Unity had no equivalent of Rust's Fluent.
- The author still "deeply enjoy[s]" Rust, found Bevy's ECS "a joy," and says compile-time checks made refactoring confident. He may use Bevy again. — [deadmoney.gg](https://deadmoney.gg/news/articles/migrating-away-from-rust)

**What has changed since (verified from Bevy release notes)**
- **Hot reload of code:** Bevy 0.17 (30 Sept 2025) added `hotpatching` via Dioxus's `subsecond` (`dx serve --hot-patch`). Limits: binary crate only; no Wasm; systems whose parameters change are not reloaded; sensitive to linker config; ECS systems only. — [Bevy 0.17](https://bevy.org/news/bevy-0-17/)
- Subsecond only tracks the "tip" crate (not workspace deps), ignores static initializer changes, and is officially experimental and may crash. A community crate, `bevy_hotpatching_experiments`, calls itself a stopgap. — [Dioxus hot reload docs](https://dioxuslabs.com/learn/0.7/essentials/ui/hotreload); [docs.rs bevy_hotpatching_experiments](https://docs.rs/crate/bevy_hotpatching_experiments/latest); [Bevy planning note](https://hackmd.io/@bevy/SkdMQIYWle)
- **Scenes:** BSN landed in 0.19 (19 Jun 2026) as the `bsn!` macro with runtime scene support. A first-party `.bsn` file loader is still in progress. — [Bevy 0.19](https://bevy.org/news/bevy-0-19/); [Bevy 0.20](https://bevy.org/news/bevy-0-20/)
- **Editor:** As of 10 Aug 2026 there is no upstream editor, only a community prototype, "Jackdaw" (visual BSN editor, ECS inspection, undo/redo). Bevy's lead calls the editor "our (and my) next and overriding priority" and admits he has promised an editor "next year" for many years. — [Bevy's Sixth Birthday](https://bevy.org/news/bevys-sixth-birthday/)
- **ECS ordering pain:** 0.20 added `chain_weak()` (orders only conflicting systems) and schedule randomization (`shuffle_seed`) to surface hidden ordering bugs. — [Bevy 0.20](https://bevy.org/news/bevy-0-20/)
- **Compile times:** The Rust project's 2026 "Fast Builds" roadmap covers better Cranelift debug builds, stabilizing the parallel frontend (rustc still processes each crate on one thread), integrating the Wild linker ("2026 and beyond"), and "Relink-Don't-Rebuild" aiming at 5-10x faster rebuilds for function-body-only changes. **These are goals, not shipped defaults.** — [Rust Project Goals 2026: Fast Builds](https://rust-lang.github.io/goals/2026/roadmap-fast-builds.html)
- **Acknowledged weaknesses in Bevy (Aug 2026):** shader and custom-material complexity grew during renderer work ("a pivot still to come"); reactivity (UI) has no converged upstream design; contributor burnout; the Project Goals rollout felt "dictatorial"; the AI policy strained the community. — [Bevy's Sixth Birthday](https://bevy.org/news/bevys-sixth-birthday/)

### Inferences
- **Strongest case against Rust for this goal:** an anime action game is tuned by feel. Hit-stop, i-frames, animation cancels, camera, VFX timing and UI flourish all depend on very fast iteration and artist-facing tools. LogLog's and Reinhart's complaints (iteration, churn, no editor, weak game-UI story) land exactly there. Bevy's hotpatching is real but narrow (systems only, binary crate only), and there is still no editor in Oct 2026.
- **Strongest counterargument:** both teams praised Rust's refactoring safety, and Reinhart praised Bevy's ECS. A solo developer who owns the whole codebase avoids the collaboration and onboarding problem. Building your own engine means you choose the abstraction level, so you are not forced into "ECS-everything." Tiny Glade shows that keeping Bevy's ECS and replacing the rest works. Data-driven tuning (hot-reloaded assets and config) can cover much of the gameplay-tuning loop without code hot reload.
- The critique that remains fully valid in 2026 is **churn**: Bevy breaks APIs every release (about every 3-5 months), and ecosystem crates lag (see §3).

### Gaps
- Did not read HN or r/rust_gamedev rebuttals in detail, so community counterarguments come from the posts themselves.
- Did not verify whether any 2026 Rust release made the parallel frontend or Wild linker the default.
- Did not find a 2025-2026 Rust gamedev postmortem specific to 3D action or animation-heavy games.

## 3. What is the state of Rust engines as of 2026 (Bevy, Fyrox, godot-rust, others), and would starting from Bevy beat writing from scratch?

### Takeaway
Bevy is the clear leader: version 0.20 (8 Oct 2026), releases every ~3-5 months, 200-280 contributors per release, and fast rendering progress (GPU-driven rendering, experimental Solari path tracing, mesh shaders, Metal support). It is still pre-1.0 with no editor and is underfunded. Fyrox reached 1.0 in 2026 with a real editor, but it is a very small team with no GI and no iOS or console path. godot-rust (gdext, v0.5.x in 2026) is the pragmatic hybrid. For a solo dev, **using Bevy crates selectively** (ECS, assets, wgpu-based renderer as a reference or base) almost certainly beats writing everything from scratch. Plan to own your anime or NPR renderer, though, as Tiny Glade did.

### Cited Findings
**Bevy versions and cadence (verified from [bevy.org/news](https://bevy.org/news/))**

| Version | Date | Pull requests / contributors | Headline items |
|---|---|---|---|
| 0.16 | 24 Apr 2025 | 1,244 PRs / 261 | GPU-driven rendering, procedural atmosphere, decals, experimental occlusion culling, ECS relationships, `no_std` |
| 0.17 | 30 Sept 2025 | 1,311 PRs / 278 | Solari experimental raytraced lighting ("not yet production ready"; realtime mode diffuse-only, denoised by DLSS-RR on NVIDIA); DLSS (RTX + Vulkan only; no DirectX, macOS, web or mobile); hotpatching; data-driven renderer mid/low-level API refactor (custom materials no longer need the `Material` trait) |
| 0.18 | 13 Jan 2026 | 659 PRs / 174-175 | Atmosphere affects scene lighting; Solari specular and soft shadows; PBR fixes; `FullscreenMaterial` for post-processing; Free/Pan camera controllers "for an eventual Bevy Editor"; gamepad UI navigation; `GltfExtensionHandler` |
| 0.19 | 19 Jun 2026 | 1,185 PRs / 259-261 | BSN `bsn!` macro; contact shadows; physically based SSR; rectangular area lights; parallax-corrected cubemaps; `many_cubes` 49.47ms to 18.77ms; skinned-mesh culling bounds from joints; `EditableText` (IME, clipboard); text moved to `parley`; `RenderErrorHandler` for device loss; partial Metal bindless |
| 0.20 | 8 Oct 2026 | 817 PRs / 226-227 | Solari "mostly unbiased," runs on macOS via Metal (no built-in macOS denoiser); DLSS-RR 4.5; mesh shaders as a low-level feature (no `StandardMaterial` integration yet, not on web); WESL replaces Bevy's custom WGSL dialect; `ctt` texture compressor to BCn (desktop) / ASTC (mobile); BSN syntax changes; more Feathers editor-UI widgets; `PanOrbitCamera`; per-column change ticks (one 132x gain in mesh extraction) |

Sources: [0.16](https://bevy.org/news/bevy-0-16/), [0.17](https://bevy.org/news/bevy-0-17/), [0.18](https://bevy.org/news/bevy-0-18/), [0.19](https://bevy.org/news/bevy-0-19/), [0.20](https://bevy.org/news/bevy-0-20/)

**Bevy project health and roadmap**
- In Aug 2026 Bevy had 1,527 unique contributors, 47.5k GitHub stars and 6.66M crates.io downloads.
- The Bevy Foundation is "drastically underfunded," and its developers are paid roughly 54% below baseline salaries.
- There is no formal roadmap or 1.0 date. The lead's priorities are Assets-as-Entities, the `.bsn` format, an upstream editor MVP, reactivity, and better shader/material workflows.
- Source: [Bevy's Sixth Birthday](https://bevy.org/news/bevys-sixth-birthday/)

**Bevy ecosystem relevant to anime action**
- NPR shading crates lag behind Bevy releases:
  - `bevy_shader_mtoon` (MToon, the VRM anime shader) 0.0.10 targets Bevy 0.13 — [docs.rs](https://docs.rs/bevy_shader_mtoon)
  - `bevy_toon_shader` 0.3.0 targets Bevy 0.12 and is 0% documented — [docs.rs](https://docs.rs/bevy_toon_shader)
  - `bevy_mod_outline` (vertex-extrusion outlines) has had 22 releases — [lib.rs](https://www.lib.rs/crates/bevy_mod_outline)
- Physics:
  - Avian 0.7 is the line for Bevy 0.19; companion crate `avian_pickup` 0.6.0 was published 6 Aug 2026 — [docs.rs avian_pickup](https://docs.rs/crate/avian_pickup/0.6.0)
  - Avian's docs say it is young but near feature parity with Rapier, and that `bevy_rapier` is "the more mature and tested" option — [docs.rs avian3d](https://docs.rs/avian3d)
- Meshlets / virtual geometry: not mentioned in the 0.16-0.20 notes I read (see Gaps).

**Fyrox**
- Fyrox 1.0.0 is stable after ~7 years (formerly rg3d).
  - Editor: project manager, animation editor, ABSM (animation blending state machine) editor; `export-cli` for CI builds.
  - Platforms: Windows, Linux, macOS, WebAssembly (tested on `wasm32-unknown-unknown`) and Android. iOS and consoles are not mentioned.
  - Physics: Rapier 0.32.
  - The team is "quite small with little to no funding," and new major features are on hold for a few months.
  - Source: [Fyrox 1.0.0 blog](https://fyrox.rs/blog/post/fyrox-game-engine-1-0-0/)
- The page shows no exact date (footer "2019-2026"). — [GameFromScratch](https://gamefromscratch.com/after-7-years-of-development-fyrox-1-0-is-here/)
- The 1.0 RC had a completed PBR pipeline with IBL, reflection probes and camera render targets. Global illumination is "planned for the future releases (most likely for Fyrox 2.0)." — [Fyrox 1.0 RC1](https://fyrox.rs/blog/post/fyrox-game-engine-1-0-0-rc-1/)

**godot-rust (gdext)**
- GitHub releases list: v0.5.0 (27 Mar; targets Godot 4.6 and Rust 2024; typed dictionaries; Wasm no longer needs LLVM/bindgen) through v0.5.5 (9 Aug; async virtual funcs for GDScript coroutines). v0.5.4 (23 Jun) added `api-4-7`. v0.5.3 (19 May) no longer needs bindgen, LLVM or a C compiler. The page shows no years; 2026 is inferred from the Godot 4.6/4.7 targets. — [gdext releases](https://github.com/godot-rust/gdext/releases)
- crates.io releases follow SemVer but lag master, and breaking changes still happen. gdnative (the Godot 3 binding) is unmaintained. — [lib.rs godot-classes](https://lib.rs/crates/godot-classes); [gdnative repo](https://github.com/godot-rust/gdnative)

### Inferences
- **Bevy versus from scratch:** Bevy gives a solo dev a production-grade ECS, asset pipeline, glTF loading, skinned animation, PBR/atmosphere/SSR/contact shadows, wgpu portability (Vulkan, Metal, DX12, web) and a texture compressor that targets mobile ASTC. That is easily years of solo work. The costs are churn (a breaking release every 3-5 months, plus waiting for ecosystem crates) and a renderer tuned for PBR, not anime NPR. Tiny Glade's pattern (keep the ECS, fork or replace the renderer, pin a Bevy version) is the most proven Rust path.
- **Bevy's rendering roadmap aims at photoreal** (Solari path tracing, DLSS, atmosphere). A cel-shaded game needs ramp/two-tone lighting, outlines, face shadow maps (SDF), rim lights and stylized post-processing. You would write all of these yourself, and Bevy's lead admits its custom-material workflow is currently complex.
- **Fyrox** is a more complete "engine with editor" today, but its funding, its lack of iOS and console paths, and its small team are risks for a long-horizon project.
- **godot-rust** is the lowest-risk "Rust plus a real editor" option: Godot owns the editor, animation tooling and UI, and Rust handles performance-critical gameplay.

### Gaps
- Did not verify the current status of Bevy's meshlet / virtual geometry feature (it was experimental in earlier releases; not covered in 0.18-0.20 notes).
- Did not verify Bevy's animation feature set (animation graph, IK, root motion, retargeting) in 2026.
- No 2026 information found on other Rust engines or frameworks (e.g. Ambient, macroquad, Comfy) beyond Bevy, Fyrox and gdext.
- Did not confirm whether `bevy_mod_outline`'s latest release supports Bevy 0.20.

## 4. Platform reach: desktop, mobile (iOS/Android) and consoles (PS5, Xbox, Switch/Switch 2)

### Takeaway
Desktop (including macOS via Metal) is solid. Android and iOS are workable through wgpu, but less proven at commercial scale. **Consoles are the biggest unknown**: Bevy itself said in Nov 2024 that no modern console has tier-1 support because of NDAs. I found **no publicly confirmed commercial Rust-engine game on PS5, Xbox Series or Switch**. Public Switch work is homebrew only. Any studio that ships Rust on console does it under NDA with private ports.

### Cited Findings
**Consoles**
- Bevy 0.15 (29 Nov 2024): "Even modern consoles such as the Nintendo Switch, PlayStation 5, or Xbox Series don't have tier 1 support" due to "non-disclosure agreements and platform specifics." `no_std` support is meant to "make it easier for commercial teams developing for these platforms." — [Bevy 0.15](https://bevy.org/news/bevy-0-15/)
- Bevy 0.16 (Apr 2025) shipped `no_std` (shown working on bare-metal desktops, embedded devices and the GameBoy Advance). Rendering, audio and assets are not available under `no_std`. — [Bevy 0.16](https://bevy.org/news/bevy-0-16/)
- Rust has an `aarch64-nintendo-switch-freestanding` target, which is homebrew and pure Rust, not Nintendo's SDK. — [rustc book](https://doc.rust-lang.org/rustc/platform-support/aarch64-nintendo-switch-freestanding.html)
- Bevy on Switch exists only as homebrew:
  - `bevy-horizon` runs on Atmosphère custom firmware with NVK (Mesa Vulkan), tested on a single Switch Lite — [GitHub](https://github.com/JohnUzoka/bevy-horizon)
  - `switchbrew_bevy` is "highly experimental," built on a Tier-3 target, and aimed at emulators — [GitHub](https://github.com/ibrahimcesar/switchbrew_bevy)
- Rust forum (Nov 2024, anecdotal): console SDKs are behind NDA, and Rust toolchain availability depends on the vendor. Xbox is "pretty close to regular Windows Direct3D 12." PlayStation is more secretive, with a low-level Vulkan-like API, and may need `no_std` plus bindgen. — [users.rust-lang.org](https://users.rust-lang.org/t/games-how-many-consoles-supported-rust/121384)
- Secondhand (unverified): Chucklefish developers said getting Rust running on the big-three consoles was feasible but could not be upstreamed because of NDAs. — [users.rust-lang.org thread](https://users.rust-lang.org/t/is-there-a-reliable-way-to-support-mainstream-console-including-std/64451)
- Tiny Glade, the most successful Rust 3D game, lists no console version as of Oct 2026. — [Wikipedia](https://en.wikipedia.org/wiki/Tiny_Glade)

**Mobile**
- Bevy 0.15 made GameActivity the Android default (NativeActivity is behind a feature flag). — [Bevy 0.15](https://bevy.org/news/bevy-0-15/)
- Bevy 0.20's `ctt` compressor outputs ASTC for mobile. — [Bevy 0.20](https://bevy.org/news/bevy-0-20/)
- Bevy reflection auto-registration supports iOS and Android. DLSS does not support mobile. — [Bevy 0.17](https://bevy.org/news/bevy-0-17/)
- Fyrox 1.0 supports Android (32/64-bit). iOS is not mentioned. — [Fyrox 1.0](https://fyrox.rs/blog/post/fyrox-game-engine-1-0-0/)
- The comparison targets are mobile-first: WuWa is "a cross-platform title for PC and mobile." — [Unreal Engine interview](https://www.unrealengine.com/en-US/developer-interviews/exploring-the-post-apocalyptic-charm-of-asg-open-worlds-in-wuthering-waves) (403 on fetch; quote from search snippet)
- Genshin maintained two customized rendering pipelines, one for PC/console and one for mobile. — [Unity Japan Dojo 2021, miHoYo talk](https://docswell.com/s/UnityJapan/KWRPQ5-210617-unity-dojo20211mihoyozhenzhongyi)

**macOS (the user's development platform)**
- Bevy 0.20's Solari runs on macOS via Metal, with no built-in macOS denoiser. — [Bevy 0.20](https://bevy.org/news/bevy-0-20/)
- 0.19 added partial Metal bindless. — [Bevy 0.19](https://bevy.org/news/bevy-0-19/)
- DLSS requires Vulkan on Windows/Linux. — [Bevy 0.17](https://bevy.org/news/bevy-0-17/)

### Inferences
- For a solo developer, consoles will in practice require (a) becoming a licensed developer, (b) porting Rust std/runtime and a graphics backend (wgpu has no public PS5/Switch backend) under NDA, or (c) paying a porting house. This is a significant risk to "ship like WuWa/ZZZ," since both have PS5 versions.
- Mobile is reachable through wgpu (Vulkan on Android, Metal on iOS). But anime gacha-scale mobile performance (thermal limits, separate low-end pipelines, as Genshin did) means a second render path you would maintain yourself.
- Developing on macOS is fine for Bevy/wgpu (Metal), but NVIDIA-only features (DLSS, DLSS-RR denoising for Solari) are unavailable there.

### Gaps
- Could not find any public confirmation of a shipped commercial Rust-engine console title (2024-2026). Studios rarely disclose it, and NDAs prevent it.
- Did not verify whether console vendors' SDKs officially include or support Rust in 2026.
- Did not verify the state of wgpu backends for consoles, or whether W4 Games' Godot console ports support GDExtension/gdext.
- Bevy iOS maturity in 2026 was not explicitly verified (no release-note statement found).

## 5. Rust versus C++ (and versus Unreal, Unity or Godot) for this goal, and how Kuro and HoYoverse built theirs

### Takeaway
Rust's language advantages (memory safety, fearless multithreading, no GC, LLVM codegen, a good ECS) are real. But the WuWa/ZZZ benchmark was hit with **heavily customized commercial engines** (UE4 and Unity) by teams of hundreds, on budgets near $100M. Rust's middleware story (Jolt, FMOD, Wwise) is thin community bindings, and its console story is NDA-gated. For this specific game type, the engine language matters less than tools, content pipelines and platform access, which is where Unreal and Unity dominate.

### Cited Findings
**How the reference games were built**
- **Wuthering Waves:** Wikipedia's infobox gives Unreal Engine 4; the body text oddly mentions a UE5 move, which looks inconsistent. — [Wikipedia: Wuthering Waves](https://en.wikipedia.org/wiki/Wuthering_Waves)
  - A developer column (speculative; the author was not involved) calls WuWa's use of UE "borderline perverse," "a UE4 game decked out in custom technology." It suggests the hair uses the community KawaiiPhysics plugin. — [AUTOMATON](https://automaton-media.com/en/column/even-from-a-developers-perspective-wuthering-waves-use-of-unreal-engine-is-borderline-perverse-a-ue4-game-decked-out-in-custom-technology/); [AUTOMATON part 2](https://automaton-media.com/en/column/wuthering-waves-is-full-of-beautiful-technical-headaches-that-would-make-the-average-ue-developer-say-please-dont-make-me-do-this/)
  - Secondary press says Kuro deliberately stayed on modified UE4 rather than UE5. — [PPE.pl](https://www.ppe.pl/news/407578/postawili-na-unreal-engine-4-i-wygrali-na-the-game-awards-2025-darmowa-gra-staje-sie-coraz-piekniejsza)
- **Zenless Zone Zero:** Unity engine (Wikipedia infobox). One SEO site claims UE4, which is an outlier. — [Wikipedia: ZZZ](https://en.wikipedia.org/wiki/Zenless_Zone_Zero)
- **Genshin Impact (HoYoverse):** Unity, with two customized rendering pipelines (PC/console and mobile). — [Unity Japan Dojo 2021](https://docswell.com/s/UnityJapan/KWRPQ5-210617-unity-dojo20211mihoyozhenzhongyi)
  - Cost about $100M to make, with ~$200M/year expected for continued development (miHoYo president Cai Haoyu, 2020-21). — [GamingBolt](https://gamingbolt.com/genshin-impact-developer-says-it-cost-100-million-to-create-200-million-cost-per-year-expected-for-continued-development)
  - About 700 people on Genshin, out of ~2,400 miHoYo staff (2020). — [Anime Corner](https://animecorner.me/mihoyo-reveals-goal-to-create-a-massive-virtual-world/) (secondary)
- **Embark** (the most Rust-invested AAA-adjacent studio) shipped ARC Raiders on modified UE5. — [Unreal Engine interview](https://www.unrealengine.com/en-US/developer-interviews/embark-studios-build-the-award-winning-arc-raiders-with-unreal-engine)

**Middleware interop (C/C++ from Rust)**
- **Jolt:** `rolt` wraps Jolt 5.0.0 with C++-like ergonomics, but its safety is "best-effort." The raw bindings are in `joltc-sys`. — [docs.rs rolt](https://docs.rs/rolt)
  - SecondHalfGames' `jolt-rust` is an "early work in progress." — [GitHub jolt-rust](https://github-redirect.dependabot.com/SecondHalfGames/jolt-rust)
- **FMOD:** `libfmod` wraps the FMOD Engine (dynamic linking only). — [GitHub libfmod](https://github.com/SecondHalfGames/libfmod)
- **Wwise:** `rrise` is a binding meant as a starting point, not a full integration; last pushed Jan 2023 (likely stale). — [lib.rs rrise](https://lib.rs/crates/rrise); [ecosyste.ms](https://awesome.ecosyste.ms/projects/github.com%2Fdtaralla%2Frrise)
- **Steam Audio:** `audionimbus-sys` provides bindings. — [docs.rs](https://docs.rs/audionimbus-sys)
- **Native Rust physics:** Avian and Rapier (see §3).

**Rust-specific language and tooling claims (opinion and practitioner reports)**
- Pro: Rust "doesn't crash in your face" compared with C++ (Tomasz Stachowiak, 2024). — [80.lv](https://80.lv/articles/exclusive-tiny-glade-developers-discuss-bevy-proceduralism-publishers-cozy-games/)
- Pro: compile-time checks make refactoring confident (Reinhart, 2025). — [deadmoney.gg](https://deadmoney.gg/news/articles/migrating-away-from-rust)
- Con: no compile-time reflection, so proc macros are needed; proc macros and serde slow builds (LogLog, 2024). — [loglog.games](https://loglog.games/blog/leaving-rust-gamedev/)
- Con: thinner AI-assistant knowledge for fast-moving Bevy (Reinhart, 2025). — [deadmoney.gg](https://deadmoney.gg/news/articles/migrating-away-from-rust)

### Inferences
- **Strongest case for Rust over C++:** a solo or small team writing a multithreaded engine (job system, asset streaming, render-graph parallelism) gets data-race freedom and far fewer memory-corruption bugs. Those bugs are what kill custom C++ engines with no QA team. Cargo plus crates.io (wgpu, glam, Avian/Rapier, egui, Bevy crates) also beats C++ dependency management.
- **Strongest case against:** every important commercial middleware (Wwise, FMOD Studio tooling, Havok, PhysX, Bink, platform SDKs) is C/C++ first. Rust users depend on community bindings of uneven maturity, and some are stale (rrise). Unreal and Unity give WuWa-class teams editor, animation, sequencer, profiler and platform support out of the box. Even so, Kuro and HoYoverse still did enormous custom engine work on top.
- **Scale implication:** WuWa and ZZZ come from studios with hundreds of developers (Genshin alone had ~700 people and ~$100M). They still chose to heavily modify an existing commercial engine rather than write one. A solo dev writing an engine and a game is attempting something those studios avoided.
- Unity has the anime-rendering ecosystem (Unity Toon Shader, the Genshin/ZZZ lineage). Unreal has the WuWa lineage and console-ready pipelines. Godot plus gdext lets you keep Rust for gameplay and systems while getting an editor and (via third parties) console ports.

### Gaps
- No official Kuro Games technical talk was found (Unreal Fest or GDC) detailing WuWa's anime rendering or mobile tech. The AUTOMATON pieces are speculative.
- No official HoYoverse talk on ZZZ's rendering was found. The Unity Dojo talk covers Genshin (2021).
- No ZZZ team-size or budget figures were found. No Kuro team size was found.
- Did not verify Rust debugger and profiler maturity (Tracy, Superluminal, RenderDoc, Xcode/Instruments on macOS) in this pass.
- No benchmark data comparing Rust and C++ engine performance for this workload was found.

## 6. What can a solo dev or small team realistically achieve with a custom Rust engine, and what should be bought or borrowed versus built?

### Takeaway
Realistically: a polished, small-scope, stylized 3D action game (arena or zone-based, a few characters, desktop first) is achievable in Rust over several years if the developer builds on Bevy crates and middleware rather than everything from scratch. A WuWa/ZZZ-scale open-world, live-service, multi-platform game is not a realistic solo target in any language, and Rust widens the gap on tools and consoles. Build what makes the game look distinctive (anime renderer, combat and animation runtime). Borrow everything else.

### Cited Findings
- **Proven small-team ceiling:** two expert engineers shipped Tiny Glade (no combat or characters) by using Bevy's ECS and writing a custom Vulkan renderer. The game began as a weekend side project. — [80.lv](https://80.lv/articles/exclusive-tiny-glade-developers-discuss-bevy-proceduralism-publishers-cozy-games/); [Wikipedia](https://en.wikipedia.org/wiki/Tiny_Glade)
- **Iteration risk:** both migrations away from Rust were by small indie teams who found Unity faster for gameplay iteration. — [LogLog](https://loglog.games/blog/leaving-rust-gamedev/); [deadmoney.gg](https://deadmoney.gg/news/articles/migrating-away-from-rust)
  - LogLog: two people, 3+ years, 100k+ lines of Rust.
  - Architect of Ruin: Rust from Dec 2023 to early 2025, then a 6-week port to Unity.
- **Fyrox 1.0** took ~7 years, with a tiny, barely funded team. — [Fyrox blog](https://fyrox.rs/blog/post/fyrox-game-engine-1-0-0/)
- **Bevy's editor** is still unshipped after 6 years and 1,500+ contributors. — [Bevy's Sixth Birthday](https://bevy.org/news/bevys-sixth-birthday/)
- **Engine-rewrite risk (C++, not Rust):** Hytale's C++ rewrite was reportedly about two years behind on gameplay features and was abandoned for the legacy engine. — [Supercraft wiki](https://supercraft.host/wiki/hytale/legacy-engine-reversion) (secondary)
- **Borrowable Rust pieces in 2026:**
  - Bevy ECS, assets and renderer (0.20) — [Bevy 0.20](https://bevy.org/news/bevy-0-20/)
  - Avian 0.7 or Rapier for physics — [docs.rs avian3d](https://docs.rs/avian3d)
  - Jolt via `rolt` / `joltc-sys` — [docs.rs rolt](https://docs.rs/rolt)
  - FMOD via `libfmod` — [GitHub](https://github.com/SecondHalfGames/libfmod)
  - Fluent for localization (praised by Reinhart) — [deadmoney.gg](https://deadmoney.gg/news/articles/migrating-away-from-rust)
  - Bevy's `ctt` texture compressor (BCn/ASTC) — [Bevy 0.20](https://bevy.org/news/bevy-0-20/)
  - Feathers widgets and BSN for building tools — [Bevy 0.19](https://bevy.org/news/bevy-0-19/)

### Inferences
- **Build:**
  - the anime NPR renderer (ramp shading, outlines, SDF face shadows, stylized post-processing);
  - the combat runtime (hitboxes, hit-stop, cancel windows, input buffering);
  - the animation state machine and blending layer, plus camera, if Bevy's is insufficient;
  - a minimal in-game debug UI (e.g. egui) and live-tuning tools.
- **Borrow or buy:**
  - ECS, assets, windowing and graphics abstraction (Bevy crates and wgpu);
  - physics (Avian, Rapier or Jolt);
  - audio middleware (FMOD via bindings);
  - DCC tools and the content pipeline (Blender to glTF; Bevy has a `GltfExtensionHandler`);
  - localization (Fluent);
  - console porting (a third-party porting studio later, if ever).
- **Suggested de-risking path:**
  1. Use Bevy as-is with a custom anime character material and pin the version.
  2. Prototype one character's combat loop to "feel good" quality within months, testing whether Rust iteration (subsecond hotpatching plus data-driven tuning) is fast enough for you.
  3. Fork or replace only the parts Bevy blocks (Tiny Glade's pattern).
  4. Target desktop (macOS, Windows, Linux) first, then Android/iOS. Treat consoles as a separate, funded, later project.
- **Fallback if iteration speed fails:** Godot plus gdext (Rust for systems, Godot for editor, UI and animation tooling) keeps the Rust investment while removing editor risk. Unity or Unreal are the routes the reference studios themselves took.
- **Timelines:** a WuWa-like scope needs hundreds of people in Unity or UE (Genshin: ~700 people, ~$100M). A solo engine-plus-game in Rust should be planned as a multi-year project with sharply reduced scope (e.g. ZZZ-style zones and arenas, not a seamless open world).

### Gaps
- No primary source gives solo-developer timelines for a 3D action game in Rust. The timeline guidance above is inference from adjacent data points (Tiny Glade, LogLog, Fyrox, Bevy).
- No verified data on Bevy's animation tooling (retargeting, root motion, IK) for character-action needs in 2026.
- No verified information on Rust-based open-world streaming solutions at scale.
