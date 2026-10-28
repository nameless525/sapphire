# Development iteration

Keep each checkout's Cargo target directory separate. Copying a source tree while reusing another
checkout's target directory can retain path-dependent artifacts; never point two worktrees at the
same `CARGO_TARGET_DIR`.

## Choose the smallest useful check

- While editing one crate: `cargo test -p <crate> --lib --locked`.
- Before handing off a change: `cargo test-fast --locked` runs workspace unit and integration tests.
- Full CPU coverage: `cargo test --workspace --locked --no-fail-fast`, including doctests. CI keeps this gate.
- Renderer GPU fixtures: `cargo test -p sapphire_render --test gpu --locked -- --ignored --test-threads=1`.
- One renderer fixture family: `cargo test -p sapphire_render --test gpu ambient:: --locked -- --ignored --test-threads=1`.
- All runtime GPU tests: `cargo test -p sapphire_render -p sapphire_rhi -p sapphire_world --tests --locked -- --ignored --test-threads=1`.

The renderer's integration tests live in `crates/sapphire_render/tests/gpu/`, with one module per
fixture family. One integration executable links the renderer once instead of once per family.
All 200 existing GPU tests remain ignored by ordinary CPU runs and execute serially in the GPU gate.
For example, the former `--test ambient` selection is now `--test gpu ambient::`.

A deterministic channel handshake replaces one 200 ms delay in the live-link shutdown regression.
Timeout and rate-limit tests retain the timing behavior they are intended to check.

## Build profiles and content

`cargo xtask --locked content --cook desktop` builds the cooker with the `tools` profile. That profile
inherits release optimization but disables cross-crate LTO. It has its own `target/tools` output,
so cooker iteration does not invalidate release app builds. The existing `--release` app switch and
shipping profile retain their previous optimization and symbol policies.

For a direct cooker invocation, use `cargo run -p sapphire_pipeline --profile tools -- <arguments>`.
The first use compiles this profile's dependencies. A fresh profile is not a warm-cache speedup.
xtask reads the executable from Cargo's artifact messages, so custom target directories work too.

Build the development sandbox and live-link CLI in one graph when both are needed:

```sh
cargo build -p sapphire_sandbox -p sapphire_livelink --features sapphire_sandbox/dev --locked
```

This unifies their requested dependency features before compilation. Separate commands can build
and link different renderer variants in succession.

## Golden comparisons

Exact RGB matches skip FLIP's perceptual calculation. The result still uses the same metric pooling
and colorized error-map path, and alpha remains ignored as before. Any changed RGB channel takes
the original FLIP path. Captures, scene isolation, thresholds, golden references and sharding are
unchanged. This optimization helps the comparison stage; it does not accelerate scene rendering.

## CI setup and cache identity

The local setup action reads the exact compiler pin from `rust-toolchain.toml`. Each job installs
only its host and required cross-compilation targets: host for lint/test/goldens, iOS device and
simulator for iOS, and Android arm64 for Android. Lint alone needs the manifest's lint components.
`RUSTUP_TOOLCHAIN` keeps subsequent Cargo calls from reinstalling every target in the manifest.

The pinned rust-cache action hashes every installed Rust toolchain. Different preinstalled compiler
versions on otherwise identical hosted images therefore produce different cache keys. The setup
helper removes unused toolchains only after installing and verifying the pinned compiler, and only
on disposable GitHub-hosted runners. It refuses local and self-hosted environments before mutation.
Do not invoke it as a local setup script. Unit tests mock every rustup command.

The change retains all 14 expanded jobs and their existing platform, golden, GPU, lint and security
gates. Requested target combinations fall from 98 to 17. This count is not an installation-time
benchmark. The new profile/environment initially creates cold caches; evaluate subsequent complete
runs as well as the first run. Compare cache restore logs, compilation steps, total critical-path
wall time and failed/retried jobs separately.

## Measurements

Controlled local measurements use Rust 1.99.0 on an Apple M5, four Cargo build jobs, isolated target
directories and alternating baseline/candidate order. Baseline commit:
`c2fe88353d618ef74039f9052378c17800a663b7`.

| Workload | Baseline median | Candidate median | Scope |
| --- | ---: | ---: | --- |
| Warm renderer source edit, workspace test build, 5 pairs | 3.804 s | 2.524 s | 33.65% less time; source comment forces rebuild/link; no tests executed |
| One gated shutdown regression, 10 pairs | 215.46 ms | 10.92 ms | 204.54 ms saved in this test only |
| 95 identical golden comparisons, 7 pairs | 3.002 s | 0.332 s | Includes two PNG decodes; identical metric digests |
| 95 alpha-only differences, 7 pairs | 3.101 s | 0.363 s | Same RGB semantics and output digests |
| Five changed-pixel controls, 7 pairs | about 0.164 s | about 0.166 s | No meaningful performance difference established |

The renderer rebuild's first pair included another candidate xtask compilation. Excluding that
pair still gives about a 33% reduction. The measurement does not establish a cold-build gain.
Golden comparison savings are about 2.67 seconds per identical 95-image pass, not a ninefold
improvement to the whole golden job. These savings cannot simply be added into an overall CI claim.
