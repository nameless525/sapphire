# The benchmark's content

M5 measures the engine at a shipped game's scale: a district of cells, a cast, and hundreds of 2K texture
sets filling the desktop texture pool. That much content would be gigabytes in git, so this folder holds
**documents only**, and a generator makes the sources they describe at cook time:

```sh
cargo xtask bench-content --preset ci                  # small: the route end to end, in seconds
cargo xtask bench-content --preset budget --cook mobile # the shipped game's load: gigabytes, minutes
```

Each run writes the preset's sources to `target/bench/<preset>/sources/`, with their sidecars, and cooks them
to `target/bench/<preset>/<cook>/` (packs) and `target/bench/<preset>/loose/<cook>/`, with a cache of its own
in `target/bench/cache/`. `rm -rf target/bench` takes all of it. Generating twice writes the same bytes.

**Nothing cooked or generated from here is committed, and nothing here ships.** Apps, packages and releases
cook `content/` alone; `bench/` sits beside it, never in it.

| Folder | Whose | What |
|---|---|---|
| `district/` | M5 B2 (B1's placeholder) | `district.ron`: the grid, the block of prefabs each cell holds, each preset's scale. `prefabs/`: prefabs from parametric shapes. `surfaces/`: procedural texture specs (`.ptex.ron`), the families the texture sets are varied from |
| `cast/` | M5 B3 (B1's placeholder) | `cast.ron`: each preset's scale. `figures/`: figure specs, built as `tools/figures/` specs are |

Each folder is a part with a generator of its own in `sapphire_pipeline::bench` (`district.rs`, `cast.rs`),
which reads only its folder, so the district and the cast grow without touching each other's files. A part
names the other's assets by ID (`bench::asset_id`), derived from their generated paths.

The presets:

| Preset | District | Cast |
|---|---|---|
| `ci` | 2 × 2 cells, 4 texture sets at 256 texels | each figure once |
| `budget` | 6 × 6 cells, 160 texture sets at 2048 texels: 2.5 GiB of maps on desktop | each figure twice |

See `docs/m5-plan.md` (B1 to B4) and the spec's §5.27 for the route, and `sapphire_pipeline::procedural` for
the procedural texture documents.
