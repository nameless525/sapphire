# The benchmark's content

M5 measures the engine at a shipped game's scale: a district of cells, a cast, and hundreds of 2K texture
sets filling the desktop texture pool. That much content would be gigabytes in git, so this folder holds
**documents only**, and a generator makes the sources they describe at cook time:

```sh
cargo xtask bench-content --preset ci                  # small: the route end to end, in seconds
cargo xtask bench-content --preset budget --cook mobile # the shipped game's load: gigabytes, minutes
cargo xtask bench-content --preset double              # the budget at twice the texture pool: only when asked
```

Each run writes the preset's sources to `target/bench/<preset>/sources/`, with their sidecars, and the phones'
to `target/bench/<preset>/sources-mobile/` (M5 B6: the same district, its cells drawing four materials where
the desktop's draw sixteen, thirteen at `budget`, as a phone build's content would; the engine reduces no cell's
materials by itself). Each cook reads its own tree and refuses the other's. It cooks them to `target/bench/<preset>/<cook>/`
(packs) and `target/bench/<preset>/loose/<cook>/`, with a cache of its own in `target/bench/cache/`. `rm -rf target/bench` takes all of it. Generating twice writes the same bytes.

**Nothing cooked or generated from here is committed, and nothing here ships.** Apps, packages and releases
cook `content/` alone; `bench/` sits beside it, never in it.

| Folder | Whose | What |
|---|---|---|
| `district/` | M5 B2 | `district.ron`: the streets, building variants, props, lanterns, dressing and look, and each preset's scale. `district_day.ron`: its dusk and night keys. `prefabs/`: the props, from parametric shapes and builtins. `surfaces/`: procedural texture specs (`.ptex.ron`), the families the texture sets are varied from. The plaza's fight is code (`sapphire_synthetic_zone::district`), and so is what a body meets (M5 B5): the ground and a box a block, the same at every preset, which the generator writes as `district.colliders.ron` |
| `cast/` | M5 B3 | `cast.ron`: each figure and what each preset makes of it. `figures/`: figure specs (the hero, an enemy, six ambient figures), built as `tools/figures/` specs are |
| `trees/` | M6 G6a | `trees.ron`: the species and each preset's leaf atlas size. `species/`: a spec each (`.tree.ron`, `sapphire_pipeline::tree`) for a broadleaf, a blossom, a conifer and a willow, each grown at three ages by space colonization, with a leaf atlas painted in brush strokes: the trees the nature zone's forest places (M6 G2b) |
| `kinds.ron` | M5 C12b | The shipped kinds a preset makes again at spec §3.4's rows, from their specs in `tools/figures/`: see "The kinds", below |

Each folder is a part with a generator of its own in `sapphire_pipeline::bench` (`district.rs`, `cast.rs`, `trees.rs`),
which reads only its folder, so the parts grow without touching each other's files. A part
names the other's assets by ID (`bench::asset_id`), derived from their generated paths.

The presets:

| Preset | District | Cast | Trees |
|---|---|---|---|
| `ci` | 4 × 4 cells (the plaza and a ring of twelve blocks): 95 buildings of the 40 variants, 1,565 props, 133 lights, 4 texture sets at 256 texels | a small hero, an enemy and two ambient figures, maps at 256 texels | four species at three ages, atlases at 256 texels |
| `budget` | 8 × 8 cells, 256 m square: 469 buildings of 40 variants, 6,943 props, 518 lights, 160 texture sets at 2048 texels: 2.5 GiB of maps on desktop, the texture pool. A desktop cell draws 13 of them, not 16 (M5 B7), so the tour's worst frame sits at spec §3.4's 2,500 main draws | three heroes at spec §3.4's hero row, three enemies at 8,000 triangles, six ambient figures sharing a skeleton | the same twelve trees, atlases at 1024 texels (512 on the phones) |
| `double` | the budget's district at 320 texture sets: twice the pool | the budget's | the budget's |

To open the district after a cook, a `dev` sandbox reads the shipped packs and the district's pack side by
side (a shipping build mounts only what its release manifest names; `cargo xtask bench-content
--with-content` makes the `ci` preset's folder for a shipping build, below): put `target/content/desktop/*` and
`target/bench/<preset>/desktop/zone_district.spack` in one folder and run, for example,
`SAPPHIRE_CONTENT_DIR=<folder>/ sapphire_sandbox --capture dusk.png --size 1280x720 --zone district
--zone-walk tour --time-of-day 19.75 --capture-ticks 300` (`--time-of-day 23` for the night key, `--bench
<frames>` in place of `--capture` to time the walk).

A `--bench` of the walk (M5 B4) measures it as a fight is measured, by phases: a cold walk from the tour's
start, then a walk again for the CPU rows and another for the GPU rows, each `<frames>` long (a lap of the
tour at run speed is about 7,560 frames, half that on Mobile-Low's two steps a frame). It logs every row's
median, p95 and worst with the metres walked at the worst frame, each counter of what the frame drew against
spec §3.4's budgets (`count row`), and how the frames split between performance and efficiency cores (`frames
by core`); `--bench-trace <csv>` writes every frame. Spec §3.8 is its first reading.

The district's fight (M5 B5) is `content/scenarios/district-encounter.scenario.ron` and its replay: a party of
three up the avenue into the plaza, whose twelve come at it. Only a `dev` build's fight and the replay runner
hand the district's builder over, so a `dev` sandbox plays it, with the hub's people about the plaza:
`SAPPHIRE_CONTENT_DIR=<folder>/ sapphire_sandbox --arena --replay tests/replays/district-encounter.sreplay --party
--mount zone_proving_hub --bench 1 --draw-every 50 --tier desktop-high --size 64x36`.

## The kinds (M5 C12b)

The party's kinds ship small, which is what the goldens and CI cook: Aster 6,944 triangles, Wren 7,056, maps
of 256. Exit test 5 asks for them at the hero row, so every preset past `ci` also makes the **kinds part**:
`sapphire_pipeline gen-kinds` links `content/` into `target/bench/<preset>/kinds/source/`, writes over it the
figures `kinds.ron` names, made again from their committed specs at the preset's scale (the cast's
`FigureScale`: detail, maps, exact triangles), and the tree cooks as `content/` does, through its cache, into
`target/bench/<preset>/kinds/<cook>/`. Only generated files are written over (the model, its maps, the face's
masks, the matcap), and a map's sidecar where its size takes a cook's cap: every hand-written sidecar and
document is the shipped one, so every ID stands and the same scenario, replay and documents play at the row.
Nothing in `content/` changes; a file is unlinked before it's written.

At `budget`: Aster at 80,000 triangles (the row), Wren at 83,808 (her detail 4: 4.8% over the row, since
detail is a whole step and her spec wasn't shaped to land on it as Aster's was), both with 2K maps that store
1K on mobile, and the Ember Lancer's maps at 1K (its 8,000 triangles are its row). The heroine's model is
authored, not generated, and within the row at 36,470: the party is 200,278 triangles against the row's
240,000 for three. `every_budget_kind_cooks_at_its_row` holds each cell.

`cargo xtask bench-content --preset budget [--cook mobile]` cooks the kinds part after the district, and
`--with-content` puts the two together in `target/bench/budget/with-content/<cook>/` with a manifest naming
both: **the one root a run at the preset mounts**, the district's walks, the arena's fight of the party
(`--arena --party --replay tests/replays/party-three-kinds.sreplay`) and the district's encounter.
`cargo xtask bench-scale prepare` makes its roots the same way.

## In CI (M5 X4)

The `ci` preset is what CI holds at scale, through a shipping sandbox. `cargo xtask bench-content
--with-content` cooks `content/` and the preset for desktop and puts them together in
`target/bench/ci/with-content/desktop/`: the shipped cook's files, linked, with `zone_district.spack` beside
them and a release manifest naming both (`sapphire_pipeline manifest`). The golden jobs bundle that folder, so:

- **Every pull request** draws three district goldens (`zone-district-street-dusk`, `zone-district-plaza-night`,
  `zone-district-plaza-phone` in `tests/golden/scenes.ron`), and the cooks-agree jobs hold the district's
  pack to the same bytes on Linux, Windows and macOS as they hold the shipped packs.
- **Every night** (and on a dispatch of CI), the scale lane walks the tour at each tier's settings, dusk and
  night, a job each, and holds what each walk counted to `tests/counts/district-ci.txt` (`cargo xtask
  counts`; `docs/development-iteration.md` says what to run when a count moves). The phone tiers' walks read
  the mobile pair, which `--with-content --cook mobile` puts in `target/bench/ci/with-content/mobile/` and
  those runs alone cook and bundle (M5 Z3).

The `ci` cook took 19 s on the M5 (cold cache, under a load of about 20): 158 assets into a 10.8 MB
`zone_district.spack` and a 1.2 MB `shared.spack` of the cast, which no test mounts.

**A deliberate change to the generator or to `bench/`** moves, in the same change:

1. the pipeline's pinned digests (`the_ci_preset_generates_the_same_bytes_on_every_machine`), and the
   `sources:` digest in the counts table's header, which `cargo xtask counts` checks;
2. the counts: `cargo xtask counts --update` on any machine, since every row it holds is exact and none depends on
   the GPU (two Metal runs agree; that lavapipe agrees is the first nightly's to show), then read the diff
   and say why each row moved;
3. the three district goldens on all three backends, nine images: Metal here with `cargo xtask golden --update
   --scene <name>` once for each, then lavapipe's and WARP's from the pull request's first CI run, as a new
   scene's are taken (`tests/golden/scenes.ron`'s header).

## At scale (M5 Z1)

`bench/scale.ron` lists every run of the benches at scale: B4's walks of the `budget` preset, the exit tests'
runs and each package's pairs. `cargo xtask bench-scale` prepares, runs and reports them under
`target/bench-scale/<commit>/`; `docs/development-iteration.md` ("The benches at scale") says how to run
the batch and read its report.

See `docs/m5-plan.md` (B1 to B4) and the spec's §5.27 for the route, and `sapphire_pipeline::procedural` for
the procedural texture documents.
