# Navigation for M3

A scout's read for [the M3 plan](../../docs/m3-plan.md), 2026-11-03: read-only, nothing built or run, on `main` at 1335f14d and crate sources from crates.io. Its claims are an agent's reading, and a package proves them before leaning on them.

**Verdict:** bake with `rerecast` in the cooker. Don't use `landmass` as the runtime: write a small own runtime inside the simulation. Nothing was built or run; the claims below come from reading sources and registry data. R = read, I = inferred.

**1. Versions and dependencies** (crates.io API and index, 2026-11-03)
- **`rerecast` 0.5.0** (R): released 2026-10-26, MIT OR Apache-2.0, owners janhohenheim and andriyDev. The spec's 0.4 (§5.19, Appendix A) is one version behind.
  - Direct deps: anyhow, bitflags, glam 0.33 (matches ours), slotmap, thiserror. Optional: bevy_reflect, libm, serde, tracing.
  - bevy comes only through its optional `bevy_reflect` feature; the bevy integration is a separate crate, `bevy_rerecast` 0.6.0.
- **`landmass` 0.9.3** (R): released 2026-10-11, MIT OR Apache-2.0, one owner (andriyDev).
  - Direct deps: disjoint 0.8, dodgy_2d ^0.5.4, geo 0.31, **glam 0.30.5**, kdtree 0.7, slotmap, thiserror. No bevy.
  - Newest dodgy_2d (0.5.5) needs **glam 0.29** and rand 0.9 (index). geo 0.31 adds i_overlay, rstar, robust, geographiclib-rs and others (index).
  - That makes three glam lines. `CoordinateSystem::to_landmass` returns landmass's glam `Vec3` (`coords.rs:20`), so we'd need a glam 0.30 dep at that boundary, or a vendored bump.
- **`deny.toml`** (I, `cargo deny` not run): I saw no landmass or rerecast dep in the deny list I read (lines 76-100), and licences look permissive but weren't checked per crate. `multiple-versions` is only `warn`, so the glam split wouldn't fail, but it is real compile weight.

**2. `landmass` in a fixed step** (R, except where marked)
- **Allocates on every update:** `Archipelago::update` builds new HashMaps (`lib.rs:283-284,329,343`), and these allocate once an agent is inserted (I). Avoidance builds a HashMap and kd-trees (`avoidance.rs:31-32,60`).
- **Allocates on every search:** `astar.rs:90,131-134`; `pathfinding.rs:325-334`; `query.rs:229` for the query path. Meshes are `Vec<Vec<usize>>` (`nav_mesh.rs:26`). Gate 4 would fail.
- **Hash iteration:** default-hasher `HashSet` iteration exists.
  - `avoidance.rs:254,364` feeds border edges to dodgy in per-process random order.
  - `nav_data.rs:475` iterates dirty islands.
- **Randomness and libm:** dodgy_2d calls `rand::random()` on a degenerate case, followed by std `cos`/`sin` (`lib.rs:367-368`). It also uses std `sin` and `atan2` (`visibility_set.rs:347,395`). I read those in 0.5.4; 0.5.5 is what would resolve.
- **landmass's own math:** no transcendentals in astar, pathfinding, path, agent, avoidance, lib, query, nav_data or util. Not grepped: nav_mesh, geometry, coords.
- **Query path alone:** A* looks orderable, since its hash use appears to be lookup only (I). But `astar.rs:71` compares `self.cost` with `other.estimate` in its tie-break, which looks like a bug, and `Ord` unwraps a float comparison.
- **Dynamic obstacles:** node modification uses geo boolean ops (`nav_data.rs:9,~925`), which allocate through `clip` and `union` (I).
- **Conclusion:** usable only outside the step, unless forked across about 6.8k lines plus dodgy_2d and geo.

**3. `rerecast` in the cooker** (R, except where marked)
- **No bevy needed:** it is a `no_std` + alloc low-level API. The pipeline is `Heightfield` -> `into_compact` -> `build_contours` -> `into_polygon_mesh` -> `DetailNavmesh::new`. It has no HashMap, HashSet, rand or threads in `src/`.
- **Tested for parity:** `tests/cpp_comparison.rs` compares its output with C++ Recast.
- **Determinism:** one run should repeat on one host (I). Bytes across architectures are not promised:
  - `detail_mesh.rs` does its arithmetic in `Vec3A` (see `.claude/notes/sim.md` and the comment in `movement.rs`).
  - `ops.rs` uses std floats unless the `libm` feature is on.
  - `tables_hash` needs byte-identical input, so either commit the cooked bytes, or prove them identical on CI and keep the detail mesh out of the hash.
- **Tile data:** one `PolygonNavmesh` per build, in plain `Vec`s:
  - `vertices: Vec<U16Vec3>`, voxel coordinates relative to `aabb` + `cell_size`/`cell_height`.
  - `polygons`/`polygon_neighbors: Vec<u16>`, with fixed stride and `0xffff` sentinels.
  - `flags`, `regions`, `areas`, `border_size`, `max_vertices_per_polygon`. `DetailNavmesh` holds per-polygon height submeshes.
- **Own format:** trivially cookable into our plain format, and the integer vertices suit it well.
- **Tile stitching:** the crate has `tile_size` and `border_size` config, but I found no multi-tile stitch or Detour-style link step in it (grep "tile"). Expect to write that in the cooker by matching boundary edges.

**4. Smallest own runtime** (I; rough non-test Rust, based on the structures above)
- Plain format, reader and validator, in an L2 crate like `sapphire_colliders`: 300 lines.
- Point location with a uniform grid: 200.
- A* over a preallocated node pool, fixed binary heap and generation-stamped visited array, with ties broken by polygon ID: 250.
- Funnel into a caller-owned fixed waypoint buffer: 150.
- Corridor following, repath policy and a per-tick search budget (node cap, requests served in actor-ID order): 200.
- Simple separation only, no ORCA: 100.
- **Total about 1,200 lines,** plus 500-800 of tests and 400-600 in the cooker for the rerecast adapter and tile stitching.
- All maths is scalar `Vec3` + `+ - * / sqrt` or `det`, and capacities are set at load (pool sized from the largest tile set). Dynamic obstacles are left out of this first version.

**5. Recommendation**
- **Where it lives:** in the simulation. Enemy movement drives `ActorTransform` and combat, so it is checksummed state. Running nav as presentation would make outcomes depend on something outside the replay, unless its output were recorded as input, and that loses cross-machine reproduction.
- **Crates:**
  - `sapphire_nav` (L3) joins `[simulation]` in `xtask/layers.toml`.
  - The cooked format goes in a plain lower crate that the cooker can link.
  - `rerecast` goes only in `sapphire_pipeline`; `landmass` stays out.
  - Update spec §5.19 and Appendix A.
- **Gameplay:** arena enemies keep steering directly (`brain.rs`). The brain asks nav only when a chase, patrol or zone needs it, and gets one desired direction per tick. Actor nav state goes through `ActorBundle`, `state_checksum` and the other steps in `sim.md`.
- **First spike should prove:**
  1. A headless rerecast bake of one stage collider set into the plain format, repeated twice with identical bytes, then compared across arm64 and x86-64 on CI (Linux, Windows, macOS). This decides between committing bytes and re-cooking.
  2. Two adjacent tiles stitched, with a path crossing the seam.
  3. A replay with N chasing enemies around a wall, with zero step allocations under `CountingAllocator`. It should use the worst-case search, and the checksums should match across OSes for each architecture.
  4. Worst-tick search cost against a chosen budget, with mobile cost measured on the simulator only as a rough guide.
  5. Whether a poly-only runtime (height from the collider or controller) is enough, so the `Vec3A` detail mesh can stay out of the hash.

**Couldn't confirm**
- Full resolved dep tree and licences: no `cargo tree` or `cargo deny` run, and transitive deps are one level from the index only.
- Whether rerecast's `libm` feature also turns on glam's `libm`. The manifest output was truncated, and `sapphire_physics`'s `Cargo.toml` comment warns against that for glam. Check before enabling it.
- Whether the rerecast repo's other crates provide stitching, or how `bevy_rerecast` tiles; I didn't read them.
- Whether landmass's avoidance can be turned off. `lib.rs:525` calls it from `update`, and I didn't read past that. It wouldn't change the allocation verdict.
- Whether rerecast's non-`ops` code actually calls transcendentals. `ops.rs` defines them, and the first hits hid any call sites. I also didn't measure any path-query or bake speed.
- `polyanya` 0.18.0 (MIT OR Apache-2.0, active): metadata only, source not read.
