# Changelog

Sapphire's releases. Each release is tagged `v<version>` on `main`, and its version is the workspace's (`[workspace.package]` in [Cargo.toml](Cargo.toml)), which every package and app carries. Until 1.0, each milestone that lands is a minor release.

## [0.1.0] - 2026-11-13

The first release: milestones M0 to M4, from bring-up to ship readiness. The renderer, the content pipeline, combat, the world and the tools are all built. The game is the **Lantern Ward**, a slice from the title to its boss, in English and Japanese. It runs on macOS, the iOS Simulator and the Android emulator. Windows and Linux build, pass their tests and render the golden images in CI. [The M5 handoff](docs/m5-handoff.md) says what each milestone left owed.

### M0: Bring-up
- A workspace whose crate layers `cargo xtask check-layers` enforces. CI on three operating systems.
- `sapphire_platform` on SDL3 and `sapphire_rhi` on `wgpu` (Metal, Vulkan, DX12), on all five platforms.
- Tracy profiling, and crash reports with minidumps on desktop.

### M1: Data path
- Content: `bevy_ecs`, reflection and versioned serialization. An asset runtime that loads by ID from packs.
- The pipeline cooks glTF, VRM 1.0 and FBX per platform (BC7 or ASTC) through a content-hash cache.
- The renderer: a frame graph, compute skinning, forward lighting, shadows, ambient probes and AO, HDR, bloom, a diffusion glow and a custom grade.
- Anime character shading: authored ramps and ILM maps, SDF face shadows, layered eyes, hair highlights, depth-offset rims and stable inverted-hull outlines, under the character's own light rig.
- One time-of-day key that drives the sky, fog, ambient light, shadow tint, the character's light and the grade together.
- Iteration: hot reload, live link to any device, and the debug overlay.
- Golden images on Metal, Vulkan (lavapipe) and DX12 (WARP).

### M2: Combat slice
- Animation: our own runtime with inertialization, IK and spring bones.
- Physics, and a deterministic fixed 60 Hz simulation whose replays are checked tick by tick.
- The combat core: moves as data, hit windows, hitstop, perfect-dodge slow motion, cancels and party swaps.
- The camera director, enemy AI, and audio on `kira`.
- Visual effects and combat screen effects, with live tuning.
- Touch controls and the timeline editor.

### M3: World and tools
- Zones and their transitions, navigation, the sequencer, dialogue and subtitles.
- Luau scripting for encounters.
- The UI with its HUD and menus, saves with migrations, and localization in English and Japanese.
- The editor's document model with undo and play-in-editor.
- Content as documents: a new enemy, encounter, boss or cutscene needs no Rust.
- The Lantern Ward, from the title to the boss.

### M4: Ship readiness
- Pipelines warm off the frame, behind a boot screen, so a scripted playthrough draws with zero shader hitches.
- Signed releases that patch themselves by downloading only the blocks that changed.
- Four quality tiers, with dynamic resolution and a ladder that steps down under heat.
- Crash symbols, so a crash in a shipping build names its function, file and line.
- Accessibility settings: remapping, subtitles, reduced flashes and larger text.
- `cargo xtask package` builds store packages for all five platforms, nightly in CI.
- Playtest builds that bring sessions back as replays.

### Known limitations
- Nobody has played the game yet, and nothing has run on a physical phone, a Steam Deck, or Windows or Linux hardware.
- The game's identity is a placeholder (`com.example.sapphire.game` in [platform/identity.toml](platform/identity.toml)). The release keys and patch host in `platform/patch.toml` are the maintainer's to set. No store has seen a build.
- [The M5 handoff](docs/m5-handoff.md) and the spec's [milestones](docs/engine-spec.md#10-milestones) list what each exit test still needs from a device, an account or a person.

[0.1.0]: https://github.com/nameless525/sapphire/releases/tag/v0.1.0
