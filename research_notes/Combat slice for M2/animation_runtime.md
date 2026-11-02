# Animation runtime for M2

Research for [the M2 plan](../../docs/m2-plan.md), read from public sources on 2026-11-01. It checks the spec's choice of `ozz-animation-rs` against a stable toolchain, and Rapier's determinism and allocation claims. Inferences are marked. It's a snapshot: check primary sources before relying on a detail.

All six answered. Inferences are marked. Repo files below are at https://github.com/SlimeYummy/ozz-animation-rs/blob/master/<path>.

**1. ozz-animation-rs at HEAD**
- Last commit is 2026-10-30. Cargo.toml says 0.12.0; crates.io still has 0.11.0 (2025-10-14).
- **Still nightly.** The `rust-toolchain` file contains `nightly`. `src/lib.rs` has `#![feature(portable_simd)]` and `#![feature(likely_unlikely)]`. The README says: "Since rust simd features are not stable, you need a nightly version rust to compile this library." Cargo.toml enables glam/glam-ext `core-simd`. glam describes that feature as "experimental nightly portable-simd support" (https://github.com/bitshifter/glam-rs/blob/main/Cargo.toml).
- **Size** (I counted raw files by line): `src/` is 14.1k lines in 17 files, about 8.8k before each file's first `#[cfg(test)]` (inferred cutoff).
  - Six files `use std::simd` directly. Non-test lines: math 1401, sampling_job 1501, animation 1208, blending_job 563, ik_two_bone 490, ik_aim 310, about 5.5k together.
  - local_to_model (285) and skeleton (526) use the SoA types. About 2.5k lines are SIMD-free (archive, base, skinning, tracks, motion blending).
  - There is no `wide` and no scalar path. `SoaVec3`, `SoaQuat` and `SoaTransform` are `std::simd` `f32x4` types.
- **Determinism** is empirical, not structural:
  - `fx4_sin_cos` and `fx4_acos` are hand-written polynomials.
  - `fx4_sign` masks NaN sign, with the comment "x86_64 and aarch64 may produce different sign NaN".
  - No `mul_add` appears (grep).
  - Committed `expected/*.rkyv` goldens are compared with `PartialEq` (exact f32). CI runs on Linux x64 and arm64, Windows x64, macOS arm64 and wasm (`.circleci/config.yml`, `tests/common.rs`).
  - `skinning_job.rs` itself says "TODO: Deterministic tests."
- **Formats:**
  - The archive is C++ ozz's binary: `Animation::tag()` is "ozz-animation" version 7, and the skeleton is "ozz-skeleton" version 2. The README says offline features are "not supported, and no plans".
  - The rkyv form mirrors ozz's compressed arrays (`ArchivedAnimation`: u16-triplet keys, ratios, previouses, iframe tables). Skeleton rest poses are raw `SoaTransform` memory, so the layout is tied to `std::simd`.
  - Our cooker could write either format without C++ only by reimplementing the offline builder (C++ `animation_builder.cc`, 23.8 KB) and matching the layout. It could not share the crate's types on stable.
- Forks: six, none above 1 star. I did not inspect them.

**2. `portable_simd`**
- Not stabilized, and nothing is scheduled. The 1.99.0 and 1.99.1 release notes mention no stabilization (https://raw.githubusercontent.com/rust-lang/rust/main/RELEASES.md).
- Tracking issue https://github.com/rust-lang/rust/issues/86656 is open. Labels include `needs-rfc` and `T-libs`, there is no milestone, and the "write an RFC" and "stabilization PR" items are unchecked.
- https://github.com/rust-lang/portable-simd/issues/364 ("Critical issues before stabilization", updated 2026-08-04). The maintainer says: "The thing we are waiting on right now are two major structural changes to the API" (PRs #493 and #483).

**3. Other runtimes**
- None credible outside Bevy.
- `fyrox-animation` 1.0.1 (tracks, poses, state machine, root motion) depends on `fyrox-core` and `fyrox-resource` (crates.io deps). Inferred: not separable.
- `bevy_animation` 0.20 depends on bevy_ecs, bevy_asset and bevy_app.
- `skelly` (skeleton and IK) is stale since 2021, and `skeletal_animation` since 2023.
- `rebirth-ik` (FABRIK and two-bone) has 11 downloads, `anymotion` is a "Prototype", and `mmd-anim-runtime` targets MMD formats (inferred from its description).

**4. Writing our own** (all numbers inferred)
- Measured references:
  - C++ ozz runtime `.cc` files: sampling 605, blending 454, local_to_model 104, ik_two_bone 381, ik_aim 211, about 1.75k lines in all.
  - Rust port non-test lines: 1501, 563, 285, 490, 310.
- A scalar glam runtime for sampling, additive and masked blending, local-to-model, two-bone IK and aim IK should be about 1.2–1.8k lines.
- Inertialization appears in neither ozz repo (grep returned 0). Budget about 250–400 lines.
- Our own clip format and cooker, with key reduction, adds about 0.5–1k lines.
- Total is roughly 2–3k lines plus tests, about 3–5 engineer-weeks.
- Scalar code will be slower than ozz's SoA SIMD. I did not measure this.
- **Licensing:** the Rust port is MPL-2.0 (file-level copyleft). The original C++ ozz is MIT (https://github.com/guillaumeblanc/ozz-animation/blob/master/LICENSE.md). Port from the MIT C++ and keep its notice on any closely translated code. Don't line-translate the MPL port's files. This is not legal advice.

**5. Rapier 0.36.1** (https://github.com/dimforge/rapier/blob/master/website/docs/user_guides/templates/determinism.mdx)
- Feature: `enhanced-determinism`, which is `["simba/libm_force", "parry3d/enhanced-determinism"]`. It "cannot be enabled at the same time as the `simd8` feature". `parallel` can be combined with it.
- Platforms: "must strictly comply to the IEEE 754-2008 ... most modern mainstream processors as well as WASM targets."
- Init values must use nalgebra's `ComplexField::sin(0.4)`, not `0.4.sin()`.
- CHANGELOG: "cross-platform reproducibility still requires matching numeric features (`block-solver`, f32 vs f64)".
- `rapier-ci-build.yml` runs the determinism goldens on x86_64 Linux, `ubuntu-24.04-arm` and wasm32. I saw no Windows, macOS, iOS or Android determinism job.
- **Allocation:** no claim that `step` is allocation-free. The only text is the `PhysicsPipeline` doc: "Rapier can reuse allocated memory" (`src/pipeline/physics_pipeline/mod.rs`).
  - Inferred: `step` can allocate when contact pairs or manifolds appear. `ContactPair.manifolds` is a `Vec`, parry's `ContactManifold.points` is a `Vec` in one configuration, and `parallel` uses rayon.
  - Test it with a counting global allocator around a warmed-up step.

**6. egui_kittest**
- 0.37.0 was published 2026-10-15 and depends on `egui ^0.37.0`, optional `wgpu ^30.0` and `kittest ^0.5`: https://crates.io/crates/egui_kittest/0.37.0.
- Inferred headless: it has no windowing dependency without the `eframe` feature. Snapshots need the `snapshot` and `wgpu` features.

**Recommendation: write our own runtime**, ported from the MIT C++ ozz, with a scalar glam core. Three facts decide it:
1. HEAD still needs nightly and hard-codes `std::simd` with no fallback. `portable_simd` is blocked on API redesign with no date.
2. Vendoring means rewriting about 5.5k of 8.8k non-test lines and staying MPL-2.0. Its formats still need ozz's offline builder, and inertialization has to be written either way.
3. The C++ original is MIT, and the jobs we need total about 1.75k lines.

For replays, only root motion and hit-volume bones need bit-exact output. Rapier forces glam's scalar core under `enhanced-determinism`, so use glam `scalar-math` plus `libm` there (inferred).

**Not verified:** a speed comparison of scalar against ozz SIMD, whether `wide` covers every op a patched ozz would need, and whether egui_kittest runs with no GPU adapter. I also did not confirm Rapier determinism on Windows, macOS, iOS or Android, or measure `step` allocations. The `#86656` timeline had 44 hidden items.
