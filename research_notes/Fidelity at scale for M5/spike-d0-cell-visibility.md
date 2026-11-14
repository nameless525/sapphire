# What cooked visibility by cell would save (M5 D0)

D0's spike, run on 2026-11-14 on B2's generated benchmark district. Unless a line says otherwise: an Apple M5 (24 GB), macOS 27.0, wgpu 30.0.1, the dev profile (dependencies at opt-level 3) for the sandbox, the `tools` profile for the cook, the `ci` district for tests and the `budget` district (8 × 8 cells of 32 m, 469 buildings of 40 variants, 6,943 props) for the numbers. "ours:" marks our own inference.

**The prototype is not on main.** It is kept on the branch `keep/m5-d0-cell-visibility` at `cb210d988855b187f7fd55b28a2fc77da5034e70`, and every command, flag, module, type and test this note names for it is that branch's: the cook `sapphire_pipeline cell-visibility`, the sandbox's dev-only `--cell-visibility`, `sapphire_zone::visibility` and its `CellVisibility`, `FramePacket::main_view_hidden`, `Streamer::placed_by_cell` and the tests in §6. Main has only this note and the plan's rows.

**Verdict: cut D7.** Cooked per-cell visible sets, conservative and exact, save the budget district's main view a median 2% of its draws at both tiers' radii (the bar was a third), nothing on the median frame and nothing on the worst; they hide 13 to 18% at the 95th percentile, on frames facing into the district, and sets for a fight's cameras hold everything. The sets are cheap (25 KB, a lookup a frame, 4 s of cook), change no pixel, and are exact by construction; what defeats them is the cell. From a point on the tour the eye sees a median 125 of the district's 533 objects past its buildings, but some place in a 32 m cell sees 524 of them: straight streets on the cells' grid, gaps between houses and cell meshes seen from everywhere. Smaller cells (untested) might close part of that gap at a higher cook cost, but D3's distance and small-object culling takes most of the far objects any set could still hide, and the sets can't help shadows. If the main view is still over budget after D2 and D3, test from the eye at runtime instead (Hi-Z on desktop, or a CPU test against this spike's building boxes, which are proven closed), and measure that before planning it.

## 1. What a set holds

A **set** says, for a camera standing in one cell, which of the zone's objects it may see. An **object** is what the main view already culls one by one by its bounds (`DrawList::cullable`): each cell's merged mesh (ground, props and dressing in one, a cell's draws in its materials) and each instance (on the district, the 469 buildings). Lights aren't objects. Two granularities were compared:

| | Cell → objects | Cell → cells |
|---|---|---|
| What it holds | a bit per object per cell | a bit per cell per cell; a visible cell draws all its objects |
| Budget district | 64 × 533 bits: 4,608 B | 64 × 64 bits: 512 B |
| Per frame | one cell lookup, then a bit test per placed object | one cell lookup, then a bit test per placed cell |
| Holds on the budget district (walk) | of 533: least 371, median 490, most 532, mean 477 | of 64: least 59, median 64, mean 63.3; as objects, median 533, mean 530 |

Both are far under the 1 MiB bar. The prototype's file (`sapphire_zone::visibility`, `SZVS` version 1) also carries each cell's object list as mesh IDs, so a sandbox checks the sets fit the zone it opened (`CellVisibility::check_fits`), and each building's whole bounds, which a camera must stand outside of: 25,188 bytes in all for the budget district, 4,532 for `ci`. The cell → cells figures come from the same file (`CellVisibility::by_cells`), so the two are measured on the same occluders.

**Lookup cost:** the eye's cell is a floor and a binary search; the stage then walks what the streamer has placed (`Streamer::placed_by_cell`, a cell's items in cooked order) and names each object the set leaves out in `FramePacket::main_view_hidden`. The renderer marks those instance slots and skips them where the frustum would have passed them. That is a bit test per placed object, a few thousand at the desktop radius, against a frustum test per candidate that the renderer already makes.

## 2. Where the samples stand

A camera may stand anywhere over a cell at the heights a sampling names. The cook lays samples on a grid over each cell, a metre apart across, a layer apart upward, each the middle of a small box half a metre either side across and half a layer up and down: every point of the cell's space is in some sample's box. Two samplings:

| Sampling | Heights | Samples a cell | Reach (x, y, z) | For |
|---|---|---|---|---|
| `WALK` | 2.0 to 3.0 m | 32 × 32 × 1 = 1,024 | 0.5, 0.5, 0.5 m | the zone walk's camera, 5.5 m behind and 1.5 m over the character, fov 55°: about 2.46 m up and 5.4 m back on flat ground |
| `COMBAT` | 0.5 to 8.0 m | 32 × 32 × 5 = 5,120 | 0.5, 0.75, 0.5 m | a fight's cameras too: `default_camera.ron`'s pivots stand 0.8 to 1.5 m up, pulled back to 12 m and pitched to 0.55 rad, about 7.8 m at most |

**Outside the sampled space the frame is the frustum's alone,** and the runtime knows it from the eye alone (`CellVisibility::cell_for`, `Outside`): an eye lower or higher than the heights sampled (a free camera over the roofs, a cutscene's crane), over no cell of the zone, or inside a building's whole bounds (where walls are seen from behind and the occluder argument doesn't hold) hides nothing. Only the main view and its prepass read a set: a shadow cascade, a character map, a reflection probe or an inset sees from somewhere else, so it draws its whole list as today. A camera that isn't the zone stage's, such as a cutscene's, is still the main view: the eye test decides, and nothing else has to say which camera it is.

ours: on terrain that isn't flat the walk's band would follow the ground (heights over the ground under the sample) and the combat band would be the default; the district is flat, so the prototype keeps zone-space heights.

## 3. How a sample decides

**Occluders are the buildings, and only the buildings:** each storey's walls (`#hall`) as a box inside its faces, from its plinth's top to its walls' top, and under each main roof (`#roof`, not a storey's skirt) two boxes under the tiles over the walls' top, a wider lower one and a narrower higher one, as high as the slope lets them stand halfway and four fifths of the way to the ridge. A storey's box reaches down through the storeys under it. The box stands in from the walls by the openings' recess (0.16 m, where paper, a door leaf or a dark doorway backs every window and door) and 2 cm more. Props, plants, stalls, the boundary wall (its gates go through), the ground and the lane closures (which a run may open) never hide anything: they are only objects.

Why a box is safe: a test casts lines from cameras outside each of the 40 variants' bounds, at both cooks' detail, through every box, 127,200 lines in all, and each meets a triangle drawn toward it (front-facing, the cooks cull back faces) before it leaves the box. The plinth stays out of the box because the test found where it isn't closed: where a plinth stops at a door's jambs, a sliver a few centimetres deep opens into the wall under the plinth's top. Only a ray going down from a camera over 0.5 m passes it, and such a ray never rises into a box that starts at the plinth's top. ours: that sliver is a hole in B2's building mesh worth closing; it isn't D0's to fix.

**The cook is conservative by construction** (occluder shrinking, after Wonka, Wimmer and Schmalstieg, *Visibility Preprocessing with Occluder Fusion for Urban Walkthroughs*, 2000): each sample asks what it sees past the occluders shrunk by its box's half-size on each axis. If every segment from the sample to an object meets a shrunk box, the segment from any camera in the sample's box to the same point stays within that box's offset of it at every step, so it meets the occluder as it is. Shrinking by axis rather than by the box's diagonal matters: the walk's camera rides about a metre under a storey's walls' top, and a diagonal of 0.87 m would have taken most of that.

**One sample, one object:** an object is hidden if every piece of its box is hidden by a single shrunk occluder. A piece is hidden by an occluder when the rays through its eight corners all meet the occluder (the occluder is convex, so every ray into the piece does) and the occluder comes first: an axis plane has the occluder and the eye on one side and the piece on the other, or the piece is farther than the occluder's farthest corner, or the occluder holds the piece. A piece no single occluder hides splits in two along its longest side, nearer half first, down to a metre; that is where occluders fuse. Once a sample sees an object it's in the set and no later sample asks. The occluders are found through a 16 m bucket grid.

**Exact on every machine by construction:** `f32` with the four operations, comparisons, `floor` and `sqrt` only, in a fixed order; transforms through the zone cook's own `placement_transform` (`sapphire_math::det` quaternions); occluders snapped inward and objects outward to a 64th of a metre; cells cooked on worker threads, each alone, the results placed by index, so the bytes don't depend on the threads (a test cooks on one and on four). The file's numbers are little-endian. What has been run: the same bytes on one and ten threads on AArch64 (the Mac), and the `ci` district's digests pinned there. Nothing has been run on x86-64.

## 4. What it saves along the tour

The sandbox walked the budget district's tour (756 m at the run speed, 7,500 ticks, every tick drawn) at 640 × 360 and dusk, with the walk's sets, at each tier's streaming radius: Desktop-High activates cells within 256 m (on this district, all of it from most of the tour) and Mobile-High within 128 m. Both read the desktop cook, since only the radius differs for the main view's candidates. A **draw** is one of the main pass's surface draws, and a frame's draws with the frustum alone are those it drew plus those the set hid; **triangles** are the candidates' first levels. The tally is the dev build's (`--cell-visibility`, logged as `cell sets`), over the 7,471 ticks whose eye stood in a cell (the other 29, at the tour's start south of the district, are the frustum's alone and count as saving nothing).

| Per frame, p50 / p95 / worst | Desktop-High, 256 m | Mobile-High, 128 m |
|---|---|---|
| Draws, frustum alone | 1,113 / 2,253 / 2,564 | 949 / 1,818 / 2,068 |
| Draws, frustum and sets by object | 1,088 / 1,851 / 2,564 | 929 / 1,616 / 2,068 |
| Draws, frustum and sets by cell | 1,113 / 2,239 / 2,564 | 949 / 1,806 / 2,153 † |
| Triangles, frustum alone | 1.83 M / 3.68 M / 4.21 M | 1.48 M / 2.92 M / 3.39 M |
| Triangles, frustum and sets by object | 1.78 M / 3.11 M / 4.21 M | 1.45 M / 2.66 M / 3.39 M |
| A frame's draws saved, by object | 0 / 18.2% / 24.3% | 0 / 12.9% / 18.3% |
| A frame's draws saved, by cell | 0 / 0.6% / 0.8% | 0 / 0.7% / 1.0% |

† A separate run, whose streaming placed slightly differently at its worst frame.

**The median falls about 2% (1,113 to 1,088 draws, 949 to 929), and the median frame saves nothing.** The worst frames, the ones a budget is read on, aren't helped at all: the worst frame with the sets is the worst frame without. What the sets hide is the tail between: frames facing down a row of buildings into the district's interior, where p95 falls by 13 to 18%. Sets by cell are worth nothing here, since nearly every cell sees part of nearly every other.

**Why so little: the cell, not the occluders.** Along the same tour, every 2 m from where the walk's camera stands, the cook also asked what the eye itself sees past the occluders as they are, a set for that one point (`probe_walk`, in the tool's output):

| Objects of 533, along the tour every 2 m | least | median | most | mean |
|---|---|---|---|---|
| What the eye's cell's set holds | 430 | 524 | 532 | 508 |
| What the eye itself sees | 63 | 125 | 300 | 146 |

A point on the tour sees about a quarter of the district; the union over a 32 m cell's thousand places sees nearly all of it. Part of that difference is the shrinking that keeps a set conservative (on `ci`'s one-metre spaces it accounts for up to 14% of what a set holds, §6); most of it is the cell. The district's streets are straight, wide and laid on the same grid as its cells, the gaps between buildings in a row are 1.2 to 2.6 m, and from somewhere in nearly every cell one of those gaps or a cross street opens a long view. The tour walks the streets and plazas, the most open cells, so its sets are fuller than the median cell's (524 against 490). The frustum already drops what's behind the camera, which is a good share of what a point's set would hide, so the 3/4 a point hides isn't 3/4 of the draws either: ours, a runtime test from the eye would remove well under that but far more than 2%.

ours: three things in the district's make-up hold the sets open and would hold in any district of this style: the cell meshes (ground, props and dressing merged, a cell's draws in its materials) are seen from almost everywhere because the ground is; a building's bounds is one object, so its upper storey or roof showing over a lower roof keeps all of it; and the wide streets that a combat game needs to fight in are view corridors.

## 5. What the cook costs

One timed run, alone on the machine (all four gate slots held, load 7.9 at the start), of `sapphire_pipeline cell-visibility` in the `tools` profile on 10 threads, `/usr/bin/time -l`. `none` reads the district (generating its 40 variants' assemblies and placing everything) and stops; the others also cook, and run the tour's probe of §4.

| | Wall | CPU | Peak memory | Samples asked | Pieces asked | File |
|---|---|---|---|---|---|---|
| `ci`, read only | 2.53 s | 2.22 s | 38 MiB | | | |
| `ci`, walk | 2.31 s | 2.31 s | 39 MiB | 1,163 | 51,126 | 4,532 B |
| `ci`, combat | 2.30 s | 2.29 s | 39 MiB | 16 | 21,829 | 4,532 B |
| budget, read only | 38.83 s | 38.78 s | 43 MiB | | | |
| budget, walk | 42.68 s | 59.95 s | 44 MiB | 65,536 | 14,766,039 | 25,188 B |
| budget, combat | 40.65 s | 40.61 s | 45 MiB | 64 | 422,080 | 25,188 B |

**The sets cost the budget district about 4 s of wall time and 21 s of CPU** over the read, the tour's probe included; the read is work the zone cook already does, so that is what D7 would add to it, against the budget district's whole cook (`cargo xtask bench-content --preset budget --cook desktop`), which took 624 s here on a shared machine (not a measurement). A cell stops asking once its set holds everything, which is why the combat sampling asks one sample a cell, and **every combat set holds all 533 objects.** The first sample stands at 1.25 m, the lowest layer's middle, and the method itself defeats it there: a wall's box starts at its plinth's top (about 0.47 m) and shrinks upward by the layer's reach (0.75 m), so under every shrunk box is a slot to about 1.22 m, and a ray from a sample that low runs under the boxes to everything. ours: making the plinth an occluder (closing its door slivers first, §3) and shrinking the layers less would tighten the low layers, but a fight's camera also rises to 7.8 m, over the one-storey roofs, so the combat band's sets would stay near full on this district either way. On `ci` (4 × 4 cells, mostly the plaza and its edges) every set holds every object at either sampling.

## 6. Proof that it changes no pixel

**Against a brute force** (`a_set_holds_everything_a_brute_force_sees`, on `ci`): 48 one-sample spaces at the walk's heights, each cooked as a set; from 30 cameras spread through each space's box, 16 rays to points of every object the set leaves out, tested against the buildings' real solids, not the shrunk boxes. 457,920 rays, and none reaches an object the set left out. The same test measures how tight the sets are: of 4,374 objects the sets held, the test's rays found 3,646 (83%) visible past the occluders as they are and 4,272 (98%) past the shrunk ones. So at least 83% of what a set holds a camera in its space really sees, the shrinking that makes the sets conservative accounts for up to 14%, and the cook's piece-by-piece test (against the rays') for 2%.

**The boxes against the meshes** (`a_halls_box_is_closed_by_what_its_building_draws`): 127,200 lines, none unmet (§3).

**Captures, on and off** (the dev build, Metal, 640 × 360): the budget district's tour, a frame every 300 ticks from tick 0 to 6,900 (24 points along the walk, 36 m apart), each pair compared byte for byte.

| | Dusk (19.75) | Night (23) |
|---|---|---|
| Desktop-High, 256 m | 24 of 24 identical | 24 of 24 identical |
| Mobile-High, 128 m | 24 of 24 identical (a second pair; the first, 18) | 24 of 24 identical (a second pair; the first, 18) |

On the frames compared the sets hid up to 523 draws (Desktop-High, at the tour's first point) and nothing at all on 12 of the 24. **Mobile-High needed a second pair, and the reason is streaming, not the sets.** At 128 m cells stream in and out during the walk, and streaming's budget is in wall-clock time, so two runs can place a cell a frame apart and draw different frames whatever the sets do. Two runs with the sets off differ from each other, as two runs with them on do:

| Mobile-High, 24 frames each | Dusk | Night |
|---|---|---|
| off against off | 18 identical | 20 identical |
| on against on | 24 identical | 20 identical |
| off against on, the first pair | 18 identical | 18 identical |
| off against on, the second pair | **24 identical** | **24 identical** |

ours: the frames that differ are different ones each time, frames where the set hid nothing differ as often as the rest, and nothing but the main pass's record reads the hidden mask, so a set can't reach a later frame. A deterministic proof at the phone's radius would want streaming budgeted by count alone in captures, which is outside D0.

**The fallback:** the camera over the plaza's roofs (`--zone district:plaza_spawn_01 --view 30,55 --zoom 3`, 120 ticks) looks up as `Outside(Height)`, hides nothing (212 draws either way) and draws the same bytes on and off.

**Determinism:** the budget district's walk sets cooked on 10 threads and on 1 are the same 25,188 bytes (digest `0xb1bed19b77c0559d`), as is a third cook after the tool was rebuilt with the tour's probe; `ci`'s two samplings are pinned in `the_ci_districts_sets_are_pinned`.

## 7. Against the bars, and what D3 changes

| Bar | Result |
|---|---|
| Median main-view draws fall by a third at both radii | **Fails:** 2.2% at 256 m, 2.1% at 128 m; triangles 2.6% and 1.5% |
| No pixel changes | Passes (§6) |
| The budget cook grows by under a minute | Passes: about 4 s of wall time, 21 s of CPU (§5) |
| Sets under 1 MiB | Passes: 25,188 bytes, of which 4,608 are the bits |
| Exact across architectures by construction | Holds by construction (§3), and held where it ran: the same bytes on one and ten threads, and the `ci` district's two pinned digests the same on AArch64 macOS, x86-64 Linux and x86-64 Windows |

**What D3 does to the same frames.** D3's distance and small-object culling removes, by tier, the far and small objects. Those are most of what a cell's set could still hide here: a far building's bounds seen down a street is exactly what a distance cull drops, and what's near is what a set can't hide, because some place in the cell sees it. ours: after D3 the sets' share of the main view's draws would be smaller than the 2% measured, not larger, and D2's instanced runs lower what each remaining draw costs. Nothing here argues for D7 after D3.

**Shadows don't gain.** D7's row says every view reads the sets. A camera's cell says nothing about what a cascade sees from the sun, a character map or a probe, so only the main view and its prepass can read them; the shadow row (D8) needs its own caster culling.

**What would pay instead.** ours, untested: a test from the eye itself, at runtime, against the same occluder boxes, which hides about three quarters of the district from a point on the tour (§4), with no cook and no sampled space to fall outside. The hall and roof boxes and their closure test are the piece worth keeping for that: they're proven conservative against the 40 variants' meshes, a few thousand boxes for the district, and a frame would ask a few hundred objects against the boxes near the eye. On desktop the same job is Hi-Z on the prepass's depth (the plan's alternative), which also catches characters and props. Neither was measured here; a frame's cost for the CPU test is the first thing to measure if the baseline (B4) says the main view's draws are over budget after D2 and D3.

## What stays unverified

- **x86-64:** the sets were cooked on AArch64. The `ci` district's two pinned digests and the one-against-many-threads test are on the keep branch, and main's CI never runs them, so one CI run was made on the keep branch (2026-11-14): both tests pass in the test jobs on x86-64 Linux, x86-64 Windows and AArch64 macOS, so the three cook the same bytes. That run's lint job fails on the prototype's bench code (clippy with every feature on), which a branch kept for reference and never merged doesn't need fixed.
- **Captures at the phone's radius aren't reproducible run to run,** with the sets off as with them on: streaming is budgeted by wall-clock time (§6). Any on/off comparison at a radius that streams during a walk needs a repeated pair, or streaming budgeted by count alone.
- **The phone cook:** the Mobile-High rows read the desktop cook at the phone's radius; the mobile cook's merged meshes and levels differ, so its draws would too, not its occlusion.
- **Devices and other backends:** the captures are Metal on the Mac's GPU through the dev build; the sets change only which candidates the main pass records, so nothing backend-specific is in play, but no Vulkan or DX12 run was made.
- **Other districts:** one generated district, flat, with straight streets on its cells' grid. A town of winding lanes, or terrain that hides what's over a hill, would hold its sets tighter; the point probe (§4) is the bound on what any set could do here.
- **A runtime test's cost:** §7's alternative is unmeasured.
