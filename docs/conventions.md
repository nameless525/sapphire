# Sapphire code conventions

These apply to every crate. The engine spec ([engine-spec.md](engine-spec.md)) says *what* to build; this file says how the code should read.

## Workspace

- **Layout:**
  - `crates/` holds runtime crates (they ship).
  - `apps/` holds shipping binaries.
  - `tools/` holds never-shipped tools.
  - `xtask/` holds build automation.
- **Layering:** every crate is placed in [`xtask/layers.toml`](../xtask/layers.toml). `cargo xtask check-layers` enforces it, and adding a crate means placing it there deliberately.
- **Dependencies** are declared once in the root `[workspace.dependencies]` and used with `dep.workspace = true`. Never put a version number in a member crate.
- **Features** select platform or backend only (rule R8). Dev tooling enters through the leaf app crate.
- **Lints:** every crate has `[lints] workspace = true`. Code must be clean under `cargo clippy --workspace --all-targets`.

## Code

- **Docs:** every crate starts with a `//!` doc comment saying what it owns and what it deliberately does not. Public items get a `///` comment when the name alone doesn't say what it does or what invariant it keeps.
- **Comments** explain *why*: constraints, platform quirks, links to the spec section. Don't narrate what the code obviously does.
- **Errors:**
  - Libraries use `thiserror` enums; `anyhow` is for tools, xtask and app `main`s.
  - No `unwrap()` in runtime code. Use `expect("reason the invariant holds")` only for genuine invariants.
- **Unsafe:**
  - Keep it in the smallest possible scope.
  - Every `unsafe` block has a `// SAFETY:` comment.
  - FFI wrappers expose safe APIs; raw pointers don't leak out of the module that owns them.
- **Time:** never read the OS clock outside `sapphire_time` and `sapphire_platform` (rule R2, enforced by `clippy.toml`). Call sites that must read it carry `#[allow(clippy::disallowed_methods)]` plus a justification.
- **Platform code:** `#[cfg(target_os = ...)]` belongs in `sapphire_platform`, `sapphire_crash` and the backend selection in `sapphire_rhi` (rule R7). Anywhere else it is a review flag.
- **Logging:** use `tracing` macros, with targets in the form `sapphire::<subsystem>`. Don't use `println!` in runtime code.
- **Profiling:** use `profiling::scope!("Name")` / `#[profiling::function]` on anything that could appear in a frame budget.
- **GPU structs:** use `#[repr(C)]` plus `bytemuck::Pod` with plain arrays (`[f32; 4]`), padded explicitly to WGSL's alignment rules. Convert from `glam` types at the boundary.
- **Fragment stages that cut pixels out** take their derivatives first, decide the cut, and `discard` once, at their exit, after the shading; only a 2 × 2 quad cut whole may leave early (eyes through hair's quad test in `anime_character.wgsl`). Apple's Metal compiler has miscompiled a per-pixel `discard` taken before the end: in M1 C7's dithered fade the pixels it cut came out black, with their depth written. Whether it does depends on the whole function, down to branches no pixel takes, so the single exit is the only shape known to be safe; the early cuts that predate this rule (the environment's alpha test, foliage's prepass) were checked pixel for pixel against single-exit twins on every tier. Every stage that cuts pixels out has a GPU test whose holes must show a background that isn't black (`sapphire_render`'s `tests/cutouts.rs`): over a black sky a black hole passes. WARP's compiler wants the single exit too: it crashes on an early `return` under a per-pixel condition.

## Tests

- Unit tests sit next to the code (`#[cfg(test)] mod tests`).
- Tests that need a GPU or window are `#[ignore]` by default, with a note saying how to run them, so `cargo test --workspace` stays headless and deterministic.
- Determinism-sensitive code (`sapphire_time`, `sapphire_math::det`) gets exhaustive edge-case tests.
