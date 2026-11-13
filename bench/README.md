# The benchmark's content

M5 measures the engine at a shipped game's scale: a district of cells, a cast, and hundreds of 2K texture
sets filling the desktop texture pool. That much content would be gigabytes in git, so this folder holds
**documents only**, and a generator makes the sources they describe at cook time:

```sh
cargo xtask bench-content --preset ci                  # small: the route end to end, in seconds
cargo xtask bench-content --preset budget --cook mobile # the shipped game's load: gigabytes, minutes
cargo xtask bench-content --preset double              # the budget at twice the texture pool: only when asked
```

Each run writes the preset's sources to `target/bench/<preset>/sources/`, with their sidecars, and cooks them
to `target/bench/<preset>/<cook>/` (packs) and `target/bench/<preset>/loose/<cook>/`, with a cache of its own
in `target/bench/cache/`. `rm -rf target/bench` takes all of it. Generating twice writes the same bytes.

**Nothing cooked or generated from here is committed, and nothing here ships.** Apps, packages and releases
cook `content/` alone; `bench/` sits beside it, never in it.

| Folder | Whose | What |
|---|---|---|
| `district/` | M5 B2 | `district.ron`: the streets, building variants, props, lanterns, dressing and look, and each preset's scale. `district_day.ron`: its dusk and night keys. `prefabs/`: the props, from parametric shapes and builtins. `surfaces/`: procedural texture specs (`.ptex.ron`), the families the texture sets are varied from. The plaza's fight is code (`sapphire_synthetic_zone::district`) |
| `cast/` | M5 B3 (B1's placeholder) | `cast.ron`: each preset's scale. `figures/`: figure specs, built as `tools/figures/` specs are |

Each folder is a part with a generator of its own in `sapphire_pipeline::bench` (`district.rs`, `cast.rs`),
which reads only its folder, so the district and the cast grow without touching each other's files. A part
names the other's assets by ID (`bench::asset_id`), derived from their generated paths.

The presets:

| Preset | District | Cast |
|---|---|---|
| `ci` | 4 × 4 cells (the plaza and a ring of twelve blocks): 95 buildings of the 40 variants, 1,565 props, 133 lights, 4 texture sets at 256 texels | each figure once |
| `budget` | 8 × 8 cells, 256 m square: 469 buildings of 40 variants, 6,943 props, 518 lights, 160 texture sets at 2048 texels: 2.5 GiB of maps on desktop, the texture pool | each figure twice |
| `double` | the budget's district at 320 texture sets: twice the pool | the budget's |

To open the district after a cook, a `dev` sandbox reads the shipped packs and the district's pack side by
side (a shipping build mounts only what its release manifest names): put `target/content/desktop/*` and
`target/bench/<preset>/desktop/zone_district.spack` in one folder and run, for example,
`SAPPHIRE_CONTENT_DIR=<folder>/ sapphire_sandbox --capture dusk.png --size 1280x720 --zone district
--zone-walk tour --time-of-day 19.75 --capture-ticks 300` (`--time-of-day 23` for the night key, `--bench
<frames>` in place of `--capture` to time the walk).

See `docs/m5-plan.md` (B1 to B4) and the spec's §5.27 for the route, and `sapphire_pipeline::procedural` for
the procedural texture documents.
