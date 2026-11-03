# The editor and the toolchain for M3

A scout's read for [the M3 plan](../../docs/m3-plan.md), 2026-11-03: read-only, nothing built or run, on `main` at 1335f14d and crate sources from crates.io. Its claims are an agent's reading, and a package proves them before leaning on them.

Bottom line: stay on every pin. wgpu 31 isn't out. The editor can dock with `egui_tiles` on egui 0.37 today, but the gizmo and snarl crates are still on egui 0.36. All facts as of 2026-11-03; R = read, I = inferred.

**1. Toolchain.** crates.io versions and dep requirements are R, via the crates.io API.
- **wgpu/naga:** newest is 30.0.1 (2026-08-22), which we pin (Cargo.toml:198). No 31: trunk's Cargo.toml still says 30.0.0, and the v31 milestone was due 2026-10-09 with one issue open (#10659, naga debugPrintf) (R). `egui-wgpu` 0.37.0 needs wgpu ^30, and egui's main branch is still on 30, so nothing is on 31 yet (R).
- **What 31 will break** (changelog "Unreleased", may still change; all R):
  - `DeviceDescriptor` gains `default_queue`; `TextureViewDescriptor` gains `swizzle`.
  - `Queue::submit` returns `Option<SubmissionIndex>`; `is_srgb()` becomes `has_srgb_suffix()`.
  - `BufferView`s block unmapping on native, and aren't supported on write-mapped buffers.
  - Inter-stage numeric types must match exactly; pipeline stage interfaces are validated after overrides.
  - `clear_texture` rejects clearing only some aspects of a texture.
  - naga: new `Expression::Bitcast`; `@interpolate(linear)` needs a new capability. `debug_draw.wgsl` uses it (R). Whether wgpu grants that capability automatically is unconfirmed.
- **What 31 buys:** bug and lock-ordering fixes, and early-depth on DX12/Metal. Nothing the editor needs (I). Stay; recheck when both wgpu 31 and an egui-wgpu on it exist, since egui-wgpu blocks the bump anyway.
- **bevy_ecs/bevy_reflect:** 0.20.1 (2026-10-28) is newest and pinned. Nothing to do.
- **glam:** newest is 0.34.2, we pin 0.33.12. Stay: `bevy_reflect` 0.20.1 requires glam ^0.33.2, and our Reflect derives use its `glam` feature (R). `kira` already brings 0.34.2 as a second copy (Cargo.lock, R).
- **sdl3-sys:** 0.7.2+SDL-3.4.18 is newest and pinned exactly. Nothing to do.
- **Also:** `ron` 0.12.3 (2026-10-28) is out, we have 0.12.2. I didn't read the diff; it's a patch to take when convenient.

**2. egui crates against egui 0.37** (requirements R, from the crates.io dependency API).
| Crate | Newest | Needs | On 0.37? | Licence | Status |
|---|---|---|---|---|---|
| `egui_tiles` | 0.18.0 (10-19) | ^0.37 | yes | MIT/Apache | rerun-io, pushed 11-02 |
| `egui_plot` | 0.38.0 (10-19) | ^0.37 | yes | MIT/Apache | fine |
| `egui_dock` | 0.21.1 (08-06) | ^0.36 | no; PR #354 open, mergeable | MIT only | code last pushed 08/09 |
| `transform-gizmo` + `-egui` | 0.11.0 (08-17) | ^0.36, glam ^0.32 | no; PR #114 open, merge blocked | MIT/Apache | no commits since 08-17 |
| `egui-snarl` | 0.12.0 (08-26) | ^0.36 | no; PR #106 open, mergeable | MIT/Apache | no commits since 08-26 |
- None is archived or abandoned, but gizmo and snarl depend on community PRs to keep up with egui (I).
- Spec §5.28's gizmo 0.11 and snarl 0.12 are real versions but don't fit its egui 0.37.
- The whole set does exist on egui 0.36: tiles 0.17.1, plot 0.37, dock 0.21.1, gizmo 0.11, snarl 0.12, egui-wgpu 0.36.2 on wgpu 30. Downgrading is possible, but I wouldn't.
- gizmo would add a third glam copy. Its API probably crosses by `mint`, a dependency I saw but whose API I didn't read (I).
- **Pick `egui_tiles`:** on our line now, dual-licensed, serde on by default so layouts persist (R), and its maintainer ships egui updates within days (I). The cost is writing tab behaviour ourselves; `egui_dock` gives more out of the box (from memory, not read).
- **Gizmo:** it isn't needed for the first spike. Wait for #114, or write a small translate gizmo.

**3. `tools/sapphire_editor` modules** (R, module docs and Cargo.toml).
- **Carry over as they are:** `window`, `input`, `recovery`, `capture` (font atlas only: its rasterizer draws "one font atlas", so a viewport texture won't appear in it), `cli`.
- **Generalize:**
  - `text`: `scan` is a generic RON byte-range tree, with one `sapphire_moves` mention.
  - `file`: `History`/`Change`, gestures, disk conflict and atomic save are generic, but sit in `MovesFile` on `MovesText`.
  - `link`: push queue and stale-reply handling.
- **Moves-only:** `edits`, `timeline`, `run`, `figure`, `preview`, `cooked`; most of `app` (1,782 lines) is the shell plus moves state, unseparated.
- **Does `ron` keep comments? No** (R, ron 0.12.3):
  - `parse.rs` `skip_comment` discards them, and `Value` has nowhere to hold them.
  - `RawValue` only keeps text verbatim for its span.
  - `ser::path_meta` writes doc comments you supply per field path on save; it doesn't round-trip existing ones.
  - So a save has to patch text, or lose comments.
- **Proposal, one model where text wins on disk:**
  - The document is typed objects with GUIDs in memory, the change record `(object, property path, old, new)` is the only edit, and undo and live link come from it.
  - Each document type has a backing with one operation, `apply(record)`. The default is a new `RonPatch`: find the path's byte range with `text::scan`, splice in the value's RON, re-parse, and check the result equals the typed value. That check generalizes `edits`' `agreement`.
  - Structure changes (insert or delete an object) splice whole entries. A brand-new document is written once with `sapphire_serialize::to_ron`.
  - `file`'s byte-minimal `Change` stays as the on-disk undo safety net, and external edits re-parse and diff into records.
  - Spec and T3 then both hold: records drive everything, and untouched bytes never move. A serializer-only save is rejected because it destroys comments; text-only is rejected because inspectors, copy/paste and non-file state need typed properties.
- **Risk:** GUIDs. Moves are named, not GUID'd, and adding GUIDs to hand-written files is noisy. Let a moves file's identity be file GUID + stable path/name; give objects a stored GUID only where documents are new (I).

**4. `crates/sapphire_reflect`** (R).
- **Has:**
  - re-exports of `Reflect`, `PartialReflect`, `FromReflect`, `TypeRegistry`, `bevy_reflect` itself, whose `GetPath`/`ParsedPath` give `reflect_path_mut`;
  - stable names and hashes, `Versioned` migrations;
  - `Guid` in `sapphire_core`;
  - dynamic document load and save by registry in `sapphire_serialize::document`, with `ReflectDocument`.
- **Coverage:** Reflect derives in tod 23, render 20, input 18, world 7. **`sapphire_moves` and `sapphire_vfx` have none, TypePath only.** I found no `reflect_path`, `GetPath` or `ReflectMut` use outside the dependency (grep, R), so an inspector has no walker to start from.
- **Missing:**
  - the change record and a path-to-RON-range mapping;
  - value capture (old/new) as serialized RON rather than `Box<dyn PartialReflect>`, also needed for postcard live link;
  - GUID-keyed object store, widget registry per type, property attributes (range, unit, hidden; `bevy_reflect` custom attributes should fit, not read);
  - Reflect derives on moves, whose rkyv derives may need care (I).

**5. Viewport and headless.**
- **Route (R):** `Renderer::prepare` and `record(graph, target)` at renderer.rs:1543/1803, `FrameGraph::import_texture(name, view)` at graph.rs:121, `egui_wgpu::Renderer::register_native_texture` (renderer.rs:809) and `update_egui_texture_from_wgpu_texture`, then `ui.image(SizedTexture)`.
  - `sapphire_rhi` re-exports `wgpu` and gives `adapter/device/queue`, so egui and the renderer can share one device. The editor window already builds `egui_wgpu::Renderer` on `gpu.device()` (window.rs:57).
  - Frame order: render to a sampled, non-surface texture, then egui to the surface. Re-register the view on resize. The overlay's `Overlay::record(graph, target)` is the pattern.
  - Check the format: egui's filtered sampling and the renderer's output encoding could mismatch, needing an sRGB view (I).
- **Blocker (R):** `layers.toml:98-99` and `preview.rs` keep the editor free of renderer and world, and `Scene` and `Content` are 8,440 lines in `apps/sapphire_sandbox` (scene.rs 4,976, content.rs 3,464) that only an app can link. `offscreen::Stage` also lives there. The viewport needs a stage crate first, or a thin scene of its own. This is the same move as the missing `game_app` (m3-handoff:98).
- **Headless tests:**
  - `egui_kittest` 0.37 is in the workspace with `default-features = false`; its `wgpu` and `snapshot` features exist (R). The editor's `tests/ui.rs` already uses it with no GPU.
  - `WgpuTestRenderer::from_render_state` accepts an existing device, and `RenderState` takes adapter, device, queue and renderer, all available from `GpuContext` (R). So a kittest can draw a real renderer texture in the shell with no window (I; untried).
  - Panels and logic run with no GPU, and the viewport as a GPU test alongside `tests/gpu`. Snapshots aren't goldens across backends: use them on Metal, or assert sampled pixels (I).
  - No test covers real windows, drag feel or focus. The editor window has still never opened (m3-handoff:31), so a person must try it.

**6. Recommendation.**
- **Toolchain:** no bumps at the start. Add `egui_tiles` 0.18 and `egui_plot` 0.38. Amend §5.28 and Appendix A: gizmo and snarl wait for egui 0.37 releases. Don't fork unless a package needs them and the PRs are still unmerged.
- **Packages, in order:**
  - **E0 spike** (below).
  - **E1 `sapphire_document`:** a new crate with no egui: object store, GUIDs, change records, history, `RonPatch` backing with `text` and `file` moved in. T3's tests must pass unchanged, with moves as first client.
  - **E2 shell:** split `app`, tiles layout and persistence, panel trait, timeline as one panel, inspector from reflection on one reflected document (time of day has derives).
  - **E3 stage extraction:** move `Scene`, `Content` and `Stage` out of the sandbox. Goldens must not move. It conflicts with other M3 work, so land it early.
  - **E4 viewport:** renderer in a texture, camera, later gizmo.
  - **E5 play-in-editor:** fresh world from a document snapshot, documents read-only to game code. It needs E3 and whichever M3 package owns the game app.
- **The spike should prove, on a throwaway branch:**
  1. A commented `.moves.ron` and a time-of-day document each take a reflected path edit by record, change only the expected bytes, undo to identical bytes, and pass the re-parse check. Moves need reflection or a hand-written adapter; this tests the riskiest idea.
  2. A kittest shows a `sapphire_render` texture inside an `egui_tiles` pane, with no window, on Metal, asserting a sampled pixel.
  3. How little of the sandbox a viewport can link. Measure this rather than guess; it sizes E3.

**Couldn't confirm.**
- wgpu 31's date or final break list.
- Whether wgpu 31 moves goldens; nothing was built.
- egui_tiles' tab and multi-window limits, and gizmo's public types; I only read Cargo metadata for these.
- PR #114's reason for being blocked, and when #354, #114 or #106 will release.
- The 0.12.3 `ron` changelog.
- Nothing in this report was run: the kittest texture route and the record-to-text model are both untried.
