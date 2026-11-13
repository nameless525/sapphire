# Renderer scale machinery: a survey for M5

A design survey's read of the code on 2026-11-13, at 27f29b5f (release 0.1.0), for [the M5 plan](../../docs/m5-plan.md). Nothing was built or run: every finding is from reading the source, the spec and the handoffs, and "unverified" marks what the source could not confirm. Line numbers are that commit's. Sizes are the surveyor's, in agent-days; the plan splits and re-sizes its packages. A package that leans on a finding checks it against the code first.

The renderer is CPU-driven only, draws LOD0 everywhere, and has never been measured near §3.4's budgets. My recommendation is to make the CPU path scale-proof in M5 and keep GPU-driven work to a spike, because Metal gains nothing from multi-draw in wgpu 30.

## 1. As built

**Draw path**
- `DrawList::build` (`crates/sapphire_render/src/scene.rs:730-833`) makes one draw per LOD0 submesh (line 771), grouped by position format and surface variant. Inside a group draws keep slot order; only eye layers sort (828-832). It rebuilds only when the drawn set changes.
- `draw.rs:373-385`: every draw binds its 48-byte record as an instance-step vertex buffer and calls `draw_indexed(indices, 0, 0..1)`. Nothing is instanced, and nothing sorts by material or mesh.
- Every pass walks the whole command list through filters (`draw.rs:178-193`): the view mask, cascade bits, hull flags, inset.
- Recording is parallel per pass, never inside one: at most 6 encoders (`graph/executor.rs:12-22`). The main pass and scene prepass each record in 30-45 µs on today's 100-160 draws (`renderer.rs:231-240`).

**Culling**
- Main view: a serial frustum-sphere test of static environment and foliage instances (`view_cull.rs:120-158`). Characters, skinned instances and fog cards are never culled. There is no distance, small-object or occlusion culling.
- Shadows: every live instance is tested against every drawn cascade each frame (`shadows/plan.rs:254-285`). Deformable environment instances enter every cascade.
- Skinning poses every posed instance, visible or not, at LOD0 (`skinning.rs:97`).

**LODs**
- Cook: up to three levels after LOD0 (½, ¼, ⅛ of the triangles), deformation-aware, morph vertices locked; the mobile cook drops the first (`tools/sapphire_pipeline/src/optimize.rs:1-45, 141-152`).
- Each `Lod` carries its error in object-space meters and reads a vertex prefix (`crates/sapphire_assets/src/mesh/mod.rs:60-71`).
- Runtime: every LOD's indices are uploaded (`mesh_pool.rs:21-22`) and nothing selects. `lod_bias` (`quality.rs:252-254`; 0, 0.5, 0, 0.5 in `tiers.ron`) is validated and unread.
- Zone cell meshes cook with no LODs (`tools/sapphire_pipeline/src/zone.rs:31, 155, 768`).

**Shadows:** 4 cascades on desktop and 2 on phones, with stable texel-snapped fits. The far half is cached and drawn in turns. Desktop has 8 character maps; phones use capsules (`shadows.rs:38-90`).

**Residency:** frees pass through a frame fence (`retire.rs`). Pool buffers never shrink and cap at 256 MiB each. Mesh uploads use `queue.write_buffer` (`mesh_pool.rs:956`).

**AA and output:** desktop resolves with a TAAU compute pass; phones use 4× MSAA on tile and a bilinear upscale in post's fused pass. Dynamic resolution assumes GPU time scales with pixels (`quality/dynamic.rs:5-9`). The surface is always 8-bit sRGB with `SurfaceColorSpace::Auto` (`crates/sapphire_rhi/src/surface.rs:293-297, 449-462`).

**Measured so far, all on one Apple M5**
- Peak 162 surface draws in a fight (§3.5), 158 in the look-dev town, about 1,000 instances on the synthetic walk.
- Three enemies at most (`arena-skirmish`).
- The hero is 36,470 triangles on desktop (spec line 3577), against 80k.
- Never measured: record time at 2,500 draws, shadows over a dense caster set, 12 enemies, an 80k hero, 64 lights' shading cost, exclusive GPU pass times (Metal overlaps passes), any phone.
- CPU rows still move ×2.6-3 between runs, unexplained, so benches must alternate and take the least.

## 2. Already designed for later
- **Crate docs, "Submission paths"** (`lib.rs:169-182`): GPU culling writing multi-draw-indirect records, one batch per position format.
- **Crate docs, "Later milestones"** (`lib.rs:780-809`): staging ring, material and mesh sort, skinning only visible instances at their LOD prefix, the main pass reusing prepass depth with `Equal`, LOD selection by projected error, EASU+RCAS or MetalFX on phones.
- **Spec §5.12, "Scene and draws"** (line 1151): v1 is CPU frustum, distance and small-object culling, parallel per view; Hi-Z and multi-draw come later on desktop.
- **Module docs:** `shadows.rs:142-161` (static far term, drawing only uncovered ground), `outlines.rs` (hulls stop at a LOD), `foliage.rs:93-110` (impostors, caster LOD), `temporal.rs:83-107` (upscalers behind `TemporalInputs`).

## 3. Hard constraints
- **Metal has no native multi-draw in wgpu-hal 30.0.1.** `draw_indexed_indirect` loops one Metal call per record (`metal/command.rs:1608-1629`), and the count variant is an empty TODO (1661-1670). `MULTI_DRAW_INDIRECT_COUNT` is DX12 and Vulkan 1.2+ only (`wgpu-types/src/features.rs:870-887`).
- **Indirect validation is always on** (`sapphire_rhi/src/backend.rs:47-49`). wgpu-core then adds a CPU entry per non-count indirect draw and injects a compute pass (`wgpu-core/src/command/render.rs:3308-3371`). Count variants bypass it.
- **Main-pass multi-draw needs bindless or per-material batches:** group 3 binds per material, and the character family uses all 16 sampled textures. Depth-only passes have no such limit.
- **iOS Simulator:** no base vertex, first instance or indirect execution (ADR 0002 line 25), so the CPU path stays universal. The Android emulator's caps are unverified.
- **Phones have no depth for Hi-Z:** Mobile-Low culls the scene prepass and discards MSAA depth on tile.
- **Goldens:** reordering draws can move pixels at depth ties and in blended draws. WARP recompiles per scene, so new compute pipelines must build only when used.
- **Hal access exists:** `Texture::as_hal`, `Device::as_hal`, `CommandEncoder::as_hal_mut`; Metal exposes `raw_handle`, `raw_device`, `raw_command_buffer`. `objc2-metal-fx` and `dlss_wgpu` are not in the cargo registry.
- **No sparse textures:** `wgpu-types` 30's `features.rs` has no sparse or partially resident texture feature.
- **HDR is reachable:** `SurfaceColorSpace` has PQ, HLG and extended sRGB (`wgpu-types/src/surface.rs:242`).

## 4. Proposed work packages

Packages 1 and 2 are prerequisites for the rest. Sizes are agent-days.

| # | Package and goal | Touches | Needs | Headless proof | Tiers | Size | Main risk |
|---|---|---|---|---|---|---|---|
| 1 | **Scale bench and district scene.** A deterministic hub district at §3.4's budgets, with counters for submitted draw calls, triangles and LODs | `sapphire_synthetic_zone`, sandbox `bench.rs`, `fight_bench/zone.rs`, `RenderStats` | none | `--bench --zone` rows against §3.2 and §3.3 on four tiers; a `district-*` golden | all | M, 3-4 | Synthetic content hides real overdraw |
| 2 | **Per-view draw lists, sort, instanced runs.** Each view gets a compacted, sorted list; runs of equal mesh, LOD and material draw as `0..n` with slot 0 bound at the run's first record, which needs no first instance | `scene.rs`, `draw.rs`, `view_cull.rs`, `renderer.rs`, `shadows/plan.rs`, `outlines.rs` | 1 | Goldens byte-identical on three backends; a temporary old-against-new readback test; record µs at 2,500 draws | all | L, 6-8 | Pixels move at depth ties |
| 3 | **Culling for every view.** Distance and small-object culling; characters by posed bounds plus hull margin; parallel per view; skin only what a view draws | `view_cull.rs`, `bounds.rs`, `shadows/plan.rs`, `skinning.rs` | 2 | On and off draw the same image; bench rows | all | M, 4 | Stale previous positions on re-entry ghost under TAA |
| 4 | **LOD selection.** Project `Lod::error` to pixels with hysteresis; shadows pick by cascade texel size; skin the finest prefix needed; hulls stop at a LOD; `lod_bias` live and a ladder rung; cell meshes get LODs with locked borders | `scene.rs`, `skinning.rs`, `outlines.rs`, `quality.rs`, pipeline `zone.rs` | 2, 3 | `sapphire_testkit measure` and `compare` gates on lines and the two-tone edge at each switch; `lod-*` goldens; a no-flap test | bias per `tiers.ron` | L, 6-8 | The two-tone edge follows normals and jumps farther than the geometric error |
| 5 | **Cooked zone visibility.** Per-cell visible sets from the zone cook: the phones' occlusion answer | pipeline `zone.rs`, `sapphire_zone/src/cooked.rs` (format bump), `view_cull.rs` | 3, spike | Walk replays draw identical pixels on and off | all | L, 6-8 after a 2-day spike | Cameras outside sampled space; cooks agreeing across architectures |
| 6 | **Shadows at scale.** Per-cascade caster lists and LODs; cached cascades keep static depth and redraw only movers; distances per zone | `shadows.rs`, `shadows/plan.rs`, `shadows/fit.rs` | 2-4 | `shadows-*` goldens unmoved; shadows row within 2 ms Desktop-Low and 3 ms Mobile-Low on the district | all | M-L, 5 | Shimmer; a depth copy is a full pass on tile GPUs |
| 7 | **Depth-equal main pass.** Load prepass depth and test `Equal` | `renderer.rs`, `pipelines.rs` | 2 | Byte-identical goldens; main-pass row | desktop only | M, 3 | New pipelines in the warm-up ledger |
| 8 | **Pool residency.** Compact at zone trips; staging ring for mesh uploads | `mesh_pool.rs`, `buffer.rs`, `retire.rs` | none | `zone_tests.rs` walk; capacity after a trip; upload p99 | all | M, 4 | Compaction against the frame fence |
| 9 | **Mobile spatial upscaler.** EASU+RCAS in WGSL after the fused pass | `post.rs`, shaders, `tiers.ron`, warm-up `Held` | none | `look_tiers` gates beat bilinear; post row at native size | phones only | M, 4 | Ringing on outlines; cost unmeasured on a phone |
| 10 | **GPU-driven spike.** Compute culling and multi-draw-count for cascades and the scene prepass | `caps.rs`, a new cull module | 1, 2 | lavapipe and WARP match the CPU path's pixels | Desktop-High, Vulkan and DX12 | S, 2 (L if built) | Saves nothing on Metal |

## 5. Decisions and spikes
1. **What §3.4's "draw calls" counts.** Recommend main-view surface draws after culling, plus a second row for submitted calls.
2. **GPU-driven in M5.** Recommend the spike only. Today's cost extrapolates to about 0.9 ms a pass at 2,500 draws on the M5 (unmeasured). With a prepass and three cascades that nears Desktop-Low's 4 ms, which packages 2-4 cut first.
3. **Occlusion.** Recommend cooked visibility on every tier, with Hi-Z later on desktop only. wgpu 30 has occlusion queries (`render_pass.rs:559`), a frame late.
4. **District content.** Recommend a merged cell shell with LODs, plus instanced repeated props.
5. **LOD transitions.** Recommend pops with hysteresis under a measured gate, and a dithered crossfade only where the gate fails.
6. **`lod_bias` 0.5.** Recommend defining it as a threshold multiplier of 2^bias.
7. **Pool.** Recommend compaction under a trip's wipe first; chunked slabs only if that falls short.
8. **Upscalers and HDR.** Recommend a 2-day MetalFX spike inside `sapphire_rhi` if the new crate is approved. Leave DLSS out (no NVIDIA machine) and HDR out (a second grading path, not scale).
9. **The ×2.6-3 CPU spread.** Package 1 should log each frame's core type. Efficiency-core scheduling is my guess, unverified.

## 6. Corrections
- **Posed bounds exist.** `view_cull.rs:16-24`, spec line 1174 and `shadows.rs:60-62` say they don't. `bounds::posed_bounds` and `InstanceSlots::world_bounds` (`scene.rs:561`) have existed since M2 N4; only outlines and character maps use them.
- **Shadows do cull per cascade.** "Shadows still test every sector" (m2-handoff line 155) undersells it: casters are culled by sphere (`plan.rs:263-283`). What is true is that every instance is tested every frame and each pass walks the whole list.
- **Residency with generational handles landed.** m2-handoff line 171 still lists it as open; it shipped in M3 W3 (`retire.rs`).
- **LOD counts.** The default chain is LOD0-3, not LOD0/LOD1; only the two enemy figures stop at LOD1, and cell meshes have none.
- **The draw-count comparison is loose.** §3.5's "162 against 700" compares draw-list entries (`RenderStats::draws`) with a draw-call budget; submitted calls are counted nowhere.
- **Nothing is instanced.** Spec §5.12 says "with instancing" and "foliage: instanced"; every draw is `0..1`.
- **Multi-draw caps differ by backend.** Spec §5.11 lists multi-draw and its count variant as equal caps; on Metal neither is native.
