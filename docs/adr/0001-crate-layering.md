# ADR 0001: Crate layering and naming

- **Status:** accepted (M0)
- **Date:** 2026-10-24

## Context

The engine spec (§4.1) put the platform crate at L0, with `core` / `math` / `jobs` above it. In practice the platform layer itself needs logging, profiling and error types. If those lived above the platform crate, every tool and test that only wants `core` would also link SDL3.

## Decision

- Crates are named `sapphire_<area>`. No crate is called `core`, `std` or `alloc`.
- **The layers are:**

  | Layer | Crates |
  |---|---|
  | L0 Foundation (pure Rust, no OS windowing) | `sapphire_core`, `sapphire_math` |
  | L1 Platform and system | `sapphire_platform`, `sapphire_jobs`, `sapphire_time`, `sapphire_crash` |
  | L2 Services | `sapphire_assets`, `sapphire_rhi` (later `sapphire_ecs`, `sapphire_reflect`, `sapphire_serialize`) |
  | L3 Subsystems | `sapphire_render` (later animation, physics, audio, VFX, UI, input, navigation, debug) |
  | L4 Framework, L5 Game | as in the spec |
  | Apps | `apps/` (`sapphire_sandbox` now; the game app later) |
  | Tools | `tools/` and `xtask/` |

- `sapphire_core` provides a log-sink hook. The platform layer registers SDL's logger on mobile, so `sapphire_core` never depends on SDL.
- `xtask/layers.toml` is the source of truth, and `cargo xtask check-layers` enforces it in CI.
  - Every workspace member and every vendored (path) crate must be placed there. Git dependencies are third-party code like registry crates: `deny.toml`'s `[sources]` admits each one, and the tool-only and dev-only lists police their reach.
  - Each entry lists the cargo features the crate may declare (rule R8: features select platform or backend only).
  - `kind = "dev_only"` marks crates such as M1's `sapphire_debug` that only an app may use, through an optional dependency, so they never ship by accident.
- **Rule R7 exceptions.** Platform `cfg` is confined to `sapphire_platform`, `sapphire_crash` and the backend selection in `sapphire_rhi`, plus two documented exceptions:
  - `sapphire_jobs::topology` detects CPU topology (sysfs, `sysctl`, `GetLogicalProcessorInformationEx`) before SDL starts.
  - `sapphire_core` classifies the target (`FormFactor`) and the thread-local storage model the counting allocator relies on.

  Both exceptions are compile-time facts that lower-layer code needs without linking SDL.

## Consequences

- Tools such as the pipeline can use `sapphire_core`, `sapphire_math` and `sapphire_assets` without pulling in SDL3 or wgpu.
- Spec §4.1 is updated to match.
