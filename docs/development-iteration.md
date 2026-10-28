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
inherits release optimization, disables cross-crate LTO and enables incremental compilation for local
importer edits. CI explicitly retains `CARGO_INCREMENTAL=0`. It has its own `target/tools` output,
so cooker iteration does not invalidate release app builds. The existing `--release` app switch and
shipping profile retain their previous optimization and symbol policies.

For a direct cooker invocation, use `cargo run -p sapphire_pipeline --profile tools -- <arguments>`.
The first use compiles this profile's dependencies. A fresh profile is not a warm-cache speedup.
xtask reads the executable from Cargo's artifact messages, so custom target directories work too. In the measured five-edit experiment, incremental compilation
added about 558 MiB of unique logical file storage (hardlinks counted once); this is a local disk
tradeoff, not an increase in CI caches where incremental compilation is disabled.

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

All existing platform, golden, GPU, lint and security gates remain. Windows golden images expand
from four to six shards, taking the workflow from 14 to 16 jobs; Linux keeps two shards. Requested
target combinations fall from the original 98 to 19, including the extra Windows jobs. This count is not an installation-time
benchmark. The new profile/environment initially creates cold caches; evaluate subsequent complete
runs as well as the first run. Compare cache restore logs, compilation steps, total critical-path
wall time and failed/retried jobs separately.

Evaluating the existing cost table against all 95 scene times from main run `40628572200` projects
that WARP 4→6 lowers the maximum scene load from 523.1 to 367.8 seconds. Including observed
cache-restoring job overhead and the other jobs, the model projects roughly 2m35s less critical-path
time for 13.6 additional runner-minutes. Additional Linux shards do not reduce that modeled critical
path. Each family still covers every scene exactly once. This is a scheduling projection; runner
speed, cache state and queuing vary, so validate it against complete hosted runs.

## Measurements

Controlled local measurements use Rust 1.99.0 on an Apple M5, four Cargo build jobs, isolated target
directories and alternating baseline/candidate order. Baseline commit:
`c2fe88353d618ef74039f9052378c17800a663b7`.

| Workload | Baseline median | Candidate median | Scope |
| --- | ---: | ---: | --- |
| Cooker-library semantic edit, original release → final tools, 5 pairs | 24.971 s | 1.702 s | 93.18% less time; every edited error message verified in the executable |
| Warm renderer source edit, workspace test build, 5 pairs | 3.804 s | 2.524 s | 33.65% less time; source comment forces rebuild/link; no tests executed |
| One gated shutdown regression, 10 pairs | 215.46 ms | 10.92 ms | 204.54 ms saved in this test only |
| 95 identical golden comparisons, 7 pairs | 2.987 s | 0.330 s | Includes two PNG decodes; identical metric digests |
| 95 alpha-only differences, 7 pairs | 3.001 s | 0.346 s | Same RGB semantics and output digests |
| Five changed-pixel controls, 7 pairs | about 0.162 s | about 0.164 s | No meaningful performance difference established |

The renderer rebuild's first pair included another candidate xtask compilation. Excluding that
pair still gives about a 33% reduction. The measurement does not establish a cold-build gain.
Golden comparison savings are about 2.66 seconds per identical 95-image pass, not a ninefold
improvement to the whole golden job. These savings cannot simply be added into an overall CI claim.

The cooker headline uses a reachable error-message edit, with a distinct marker for each sample,
and verifies the marker by invoking the built binary. Both variants rebuild the library and CLI.
It measures a localized edit with warm dependencies; broad refactors and first builds have different
costs. Initial profile preparation is excluded and does not establish a cold-build improvement.

Intermediate controls separated the two changes: disabling LTO reduced comment-only library
rebuilds from 24.926 to 15.298 seconds and CLI-only rebuilds from 17.539 to 0.609 seconds. Adding
incremental compilation reduced semantic edits from 15.307 to 2.074 seconds. These experiments
are separate from the direct original/final comparison and their speedups must not be multiplied.

All 379 loose assets and one pack matched byte for byte on both desktop and mobile in the direct
original/final comparison. Desktop full-cook samples were 15.078 and 15.177 seconds; mobile samples
were 10.352 and 10.284 seconds (one pair each, exploratory). Five paired full desktop cooks with
incremental off/on were 15.127 and 15.006 seconds at the median, with overlapping ranges. The
earlier release/no-LTO-only experiment was 14.757 versus 14.992 seconds, a 1.6% ratio-of-medians
overhead in that sample. The change targets rebuild latency; it does not claim faster cooking.

Golden validation compares complete metric and error-map byte streams as well as digests for
exact, alpha-only and changed-pixel cases. No images were rebaselined.

The final manifest was also exercised through unmodified `cargo --locked xtask --locked content
--cook desktop`, with no profile environment overrides. Five warm invocations took 0.545 seconds
at the median, and every output matched the original release cook. This is a candidate-only
measurement, not a before/after claim.
