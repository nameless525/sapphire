# Optimization pass two

This pass starts from `eddeaf68164c350b41284b9f17e190468f556cb4` (the first iteration pass).
It retains optimized test profiles, every existing correctness gate, all golden frames and thresholds,
and application release/shipping profiles.

## Changes and measured local results

| Workload | First-pass baseline | Second pass | Measurement |
| --- | ---: | ---: | --- |
| Live-link CLI build after a semantic shader edit | 1.589 s | 0.088 s | Five alternating pairs; candidate rebuilds zero targets |
| Desktop cold cook and pack | 14.750 s | 10.623 s | Five alternating pairs, byte-identical output |
| Mobile cold cook and pack | 10.358 s | 6.688 s | Five alternating pairs, byte-identical output |
| Four Android log-following tests | 7.240 s | 0.304 s | Five alternating pairs; production waits unchanged |
| Real child-timeout regression | 6.089 s | 3.085 s | Five alternating pairs; both three-second deadlines retained |
| Post-day capture, Metal | 148.27 ms | 129.38 ms | Five paired repetitions after warmup; identical PNGs |

These are distinct workloads, not additive whole-project speedups. Other sampled Metal captures
(foliage-ground and lookdev-town-day) showed no meaningful timing change. All 95 native off/on
capture pairs were byte-identical, and all 95 committed Metal references passed without rebaselining.
Hosted software-GPU and artifact-fanout results must be evaluated separately.

### Renderer-independent watching

`sapphire_watch` owns the existing watcher and repository source-path helpers. `sapphire_hot_reload`
reexports its prior public API and retains engine-specific reload behavior. The live-link CLI imports
the watcher directly, removing renderer, RHI and wgpu from its normal dependency graph. Desktop
notify behavior and mobile unsupported-backend behavior remain unchanged. A layer-check regression
test guards the dependency separation. The resolved CLI graph falls from 151 to 113 packages; that
count is not a cold-build timing benchmark.

### Bounded parallel packing

At most four workers encode independent content keys, largest inputs first. Results and errors are
consumed in asset-ID order, and duplicate cache keys share an encoding. Compression settings, pack
layout, corruption recovery and hash validation are unchanged. All 379 loose assets and one pack
match on desktop and mobile across 24 full cold cooks; warm-cache, deleted-pack and single-material
edit controls also pass. Pack-stage medians fall from 5.478 to 1.404 seconds on desktop and 4.913 to
1.277 seconds on mobile. CPU work remains essentially constant; scheduling reduces elapsed time.
Measured process peak RSS increases by about 1–4 MB. Asset cooking and internally parallel probe
bakes remain unchanged.

### Deterministic test waits

Android log-following tests supply short synthetic timing only to their fake process implementation.
Explicit probe/readiness markers establish ordering, and tests verify process and directory cleanup.
Production polling and final log drain remain one second each. The testkit runs two independent real
three-second timeout cases concurrently, each followed by a successful scene; a separate quick case
checks accumulation of consecutive failures. No deadline was shortened or test removed.

### Capture reuse without shared scene state

Testkit gives serial scene processes a fresh private pipeline-cache directory per run. Existing RHI
checksums, adapter/driver identity, atomic saves and crash sentinels protect reuse; a successful,
non-lost capture confirms/saves the cache. Unsupported backends do not write a cache. Benchmark
and serving modes do not use this capture cache. `SAPPHIRE_GOLDEN_PIPELINE_CACHE=on|off` controls
it for comparison, and disabling it removes inherited child cache configuration.

Eligible TAA warmup frames use an unconsumed transient display target. The existing graph compiler
keeps imported temporal history/depth and other persistent writes while removing unused bloom and
compositing. All simulation ticks, frame counts, waits, prepare/record calls and final capture remain.
Overlays, debug views, dynamic resolution and timing/serial instrumentation use full frames.
`SAPPHIRE_CAPTURE_HISTORY_ONLY=on|off` provides a same-executable reference path. Native graph
traces confirm 17→7 passes on an eligible warmup: nine bloom passes and compositing are removed.

### Bounded runtime work

Ambient capsule selection retains only the nearest 17 candidates when distances are distinct;
the extra candidate detects cutoff ties. Small inputs, ties and other ambiguous cases use the
original full selection, retaining exact ordering. Differential tests cover 9,984 prefix comparisons.
Measured distinct-distance workloads improve 6.4× at 256 bodies and 40.7× at 1,000 bodies; these are
selection-stage results, not frame-rate gains. Tiny scenes gain effectively nothing.

Asset residency collection records whether each kind has evictable assets during its existing scan,
then avoids futile over-budget rescans. Removing an asset forces subsequent rescans so dependency
release behavior is preserved. Sorting scratch is reused and released on explicit low-memory unload.
Held/pinned over-budget workloads improve roughly 4–5×. Small unused under-budget controls incur
extra bookkeeping (about 57 ns for 379 cached assets, 1.46 µs for 10,000). A handle dropped concurrently
after the snapshot can be collected on the next update; low-memory flushing retains the full scan.

## CI artifact fanout

Linux and Windows each build the capture executables and cook desktop content once, then supply
immutable artifacts to their existing golden shards. Metal stays inline because it has one shard.
The exact commit, backend family, file inventory, SHA256 hashes and executable modes are checked
before execution; traversal, links and case collisions are rejected. WARP stays adjacent to the
Windows executable. All prior check names, scenes, timeouts, crash diagnostics and replay behavior
remain, with two additional producer checks. No Cargo target directory is shared between runners.

The added artifact barrier may affect latency even when aggregate runner work falls. Measure producer
build/cook/upload, consumer download/verification and rendering separately. Keep per-family producer
dependencies independent so Linux does not wait for Windows. Artifacts expire after one day; rerun
producers if retrying consumers after expiry.

`Capture experiment` is a manual same-runner comparison of cache/history switches using one verified
producer artifact. Its lightweight PR registration check runs only when the workflow/helper changes;
it never substitutes for normal CI. Supply the producer run ID and its exact checkout SHA (a PR merge
SHA can differ from the branch head). A discarded warmup and three alternating measured rounds
retain per-process logs, PNG parity, binary/content hashes, timings and cache sizes.

## Experiment not adopted

A test opt0 prototype with the pipeline retained at opt1 cut a single dependencies-warm workspace
compilation from 83.99 to 41.49 seconds, but CPU execution rose from 45.23 to 49.62 seconds and the
GPU test executable grew by 67%. GPU runtime was not validated. It also changes sharing with dev
builds and optimized-code coverage, so the default optimized test profile remains unchanged.
