# T0: the look at every tier and scale

M4 T0's spike, measured on 2026-11-10. It answers three questions for T2's ladder and §3's "Dynamic resolution": which dynamic floors keep the look, what each optional pass costs the look when it goes, and what it saves.

## Conditions

- **Machine:** Apple M5, macOS, Metal. The sandbox was a release build of this branch, reading the desktop cook. The phone tiers ran on the Mac with the phone tiers' settings, not on a phone and not on the mobile cook.
- **Load:** other agents' builds kept the load between 10 and 25 during the captures. Captures don't depend on timing. Every bench ran alone under `slot --all --load 8`.
- **Outputs:** the desktop tiers at 1920×1080. The phone tiers at 2532×1170, an iPhone 12 or 13 held sideways. The phones cap their height, so at 1170 rows Mobile-High draws at a base scale of 0.632 (740 rows) and Mobile-Low at 0.504 (590 rows).
- **Scales:** each tier was drawn at four scales:
  - 1;
  - its own base scale;
  - the ladder's 0.85 of the base;
  - its dynamic floor: 0.7 of the base on desktop and 0.75 on phones.

  On the desktop tiers the base is 1. On a 1170-row phone the floors are 0.474 (Mobile-High) and 0.378 (Mobile-Low).
- **Frames:**
  - Desktops resolve with TAA. They drew 32 frames before the capture, or 16 frames a tick apart for the hit.
  - Phones resolve with 4× MSAA and a bilinear upscale in post. They drew 1 frame, or 2 for the hit.
- **Shots:**
  - **face:** the character's close-up, `--look-dev face --time-of-day 13`.
  - **ward:** the ward's street, `--zone lantern_ward:player_start --time-of-day 14`, with the character small in the frame.
  - **hit:** a hit's post with its screen effect up, `--look-dev hero --time-of-day 12 --replay tests/replays/fight.sreplay --cues` at tick 71. The bench uses `--vfx duel` in its place: about 250 particles, the duel's worst moment.

## How it's measured

Everything is in `sapphire_testkit::look`. The sandbox-facing commands are `sapphire_testkit measure <with> <without> [--region x,y,w,h]` and `sapphire_testkit compare <reference> <capture>`.

- **Lines.** Each shot is captured twice, with the lines and with `--outline-distance 0`. A pixel's darkness d = 1 − L_with / L_without, where L is linear luminance, is how much line covers it.
  - Strength Σd is how much line there is.
  - Contrast Σd²/Σd is how dark a line pixel is on average: 1 for a crisp line, less as the upscale smears it.
  - Width is strength / contrast, in output pixels.
- **The two-tone edge.**
  - **In captures,** "spread" is (Σ|∇L|)² / Σ|∇L|² over the character's region, with the lines off. It grows as edges get wider for the same total change. It is reported as a ratio to scale 1.
  - **In the GPU test,** the 10–90% width of the terminator's profile across a hard split, in output pixels: a step reads 0.8, a step antialiased across one pixel 1.6, and a 4-pixel ramp 4.0.
- **Whole frame:** FLIP, as the golden runner computes it: the mean, the 99th percentile, the worst 32-pixel tile, and the maximum. The golden gates are a mean of 0.01, a p99 of 0.12 and a worst tile of 0.15. Here they are a measure of how much changed, not pass or fail.

**A lesson:** a vertical terminator falls at the same sub-pixel phase on every row. That makes a bilinear upscale read anywhere from sharp to twice its width, and the first run's phone edges came out out of order (Mobile-Low: 3.35 px at its base, 2.36 px at its floor). The GPU test turns the key light 0.15 rad so the terminator slants across the rows and samples every phase.

## The GPU test: lines and the hard edge

`crates/sapphire_render/tests/gpu/look_tiers.rs` draws a unit sphere through each shipped tier's settings:

- the shading split hard (softness 0);
- the shadow tone at 0.4 of the lit tone;
- a black line of 5.5 px at 1080 rows (the look-dev character's width);
- a grey sky equal to the lit side.

The output is 640×1080 for desktops and 640×1170 for phones. Lines are read on 21 rows at the silhouette, and the edge on the same rows across the slanted terminator.

| Tier | Scale (absolute) | Line width, covered px | Line contrast | Two-tone edge, 10–90% px |
|---|---|---|---|---|
| Desktop-High, Desktop-Low | 1 | 6.01 | 1.00 | 1.58 |
| Desktop-High, Desktop-Low | 0.85 | 6.46 | 1.00 | 1.62 |
| Desktop-High, Desktop-Low | 0.7 (floor) | 6.47 | 1.00 | 1.65 |
| Mobile-High | 1 | 6.61 | 1.00 | 0.97 |
| Mobile-High | 0.632 (its own) | 7.36 | 0.903 | 2.31 |
| Mobile-High | 0.538 (0.85) | 7.68 | 0.875 | 2.64 |
| Mobile-High | 0.474 (floor) | 7.84 | 0.855 | 2.85 |
| Mobile-Low | 1 | 6.61 | 1.00 | 0.97 |
| Mobile-Low | 0.504 (its own) | 7.64 | 0.878 | 3.25 |
| Mobile-Low | 0.429 (0.85) | 8.01 | 0.836 | 3.16 |
| Mobile-Low | 0.378 (floor) | 8.42 | 0.785 | 3.58 |

- **Line strength** (how much line) stays within 0.999–1.016 of scale 1 everywhere. The line keeps its width in output pixels at every tier and scale. What changes is how crisp it is.
- **Desktops:** TAA rebuilds a still line and edge at every scale to the floor. This test is still. A turning line upsampled at 0.667 reads 118% wide and 88% contrast (the existing outline test, M1 R6).
- **Phones:** the height cap does most of the softening. A native 0.97 px edge becomes 2.3 px (Mobile-High) and 3.25 px (Mobile-Low) at the tier's own scale. The floor adds 10–25% on top, and takes line contrast to 0.86 and 0.79.
- **Its assertions:**
  - line strength within 5% of scale 1;
  - TAA tiers: contrast above 0.97 and edge under 1.8 px;
  - phones: contrast above 0.75, edge under 1.2 px at scale 1 and under 4 px otherwise, and no more than 1.3 times the edge at the tier's own scale.

  They pass in 14 s on Metal (warm).

## The captured shots

The ratios are to the same tier at scale 1. "Spread" is the character's edge spread. FLIP is against scale 1.

| Shot | Tier | Scale | Line strength | Line contrast | Spread | FLIP mean / p99 / worst tile |
|---|---|---|---|---|---|---|
| face | Desktop-Low | 0.85 | ×1.006 | ×0.998 | ×0.998 | 0.011 / 0.075 / 0.13 |
| face | Desktop-Low | 0.7 | ×1.007 | ×0.997 | ×0.987 | 0.016 / 0.099 / 0.19 |
| ward | Desktop-Low | 0.85 | ×0.981 | ×0.980 | ×0.969 | 0.015 / 0.096 / 0.28 |
| ward | Desktop-Low | 0.7 | ×0.886 | ×0.951 | ×0.935 | 0.024 / 0.153 / 0.32 |
| hit | Desktop-Low | 0.85 | ×0.978 | ×1.048 | ×0.992 | 0.018 / 0.142 / 0.23 |
| hit | Desktop-Low | 0.7 | ×0.997 | ×1.052 | ×0.995 | 0.022 / 0.168 / 0.25 |
| face | Mobile-High | 0.632 | ×1.051 | ×0.778 | ×1.322 | 0.029 / 0.204 / 0.22 |
| face | Mobile-High | 0.538 | ×1.062 | ×0.736 | ×1.426 | 0.036 / 0.247 / 0.29 |
| face | Mobile-High | 0.474 | ×1.074 | ×0.703 | ×1.507 | 0.041 / 0.285 / 0.32 |
| ward | Mobile-High | 0.632 | ×1.021 | ×0.744 | ×1.167 | 0.035 / 0.210 / 0.27 |
| ward | Mobile-High | 0.474 | ×1.021 | ×0.692 | ×1.249 | 0.051 / 0.290 / 0.40 |
| hit | Mobile-High | 0.632 | ×1.018 | ×0.869 | ×1.123 | 0.029 / 0.174 / 0.34 |
| hit | Mobile-High | 0.474 | ×1.046 | ×0.781 | ×1.240 | 0.043 / 0.255 / 0.46 |
| face | Mobile-Low | 0.504 | ×1.065 | ×0.720 | ×1.451 | 0.038 / 0.268 / 0.32 |
| face | Mobile-Low | 0.429 | ×1.092 | ×0.678 | ×1.584 | 0.045 / 0.317 / 0.35 |
| face | Mobile-Low | 0.378 | ×1.102 | ×0.647 | ×1.665 | 0.051 / 0.354 / 0.42 |
| ward | Mobile-Low | 0.504 | ×1.013 | ×0.690 | ×1.236 | 0.048 / 0.281 / 0.36 |
| ward | Mobile-Low | 0.429 | ×1.045 | ×0.665 | ×1.284 | 0.057 / 0.330 / 0.46 |
| ward | Mobile-Low | 0.378 | ×1.063 | ×0.661 | ×1.319 | 0.065 / 0.378 / 0.50 |
| hit | Mobile-Low | 0.504 | ×1.031 | ×0.798 | ×1.210 | 0.039 / 0.248 / 0.49 |
| hit | Mobile-Low | 0.429 | ×1.044 | ×0.745 | ×1.280 | 0.048 / 0.295 / 0.51 |
| hit | Mobile-Low | 0.378 | ×1.051 | ×0.716 | ×1.345 | 0.055 / 0.331 / 0.53 |

- **Desktop-High** reads the same as Desktop-Low to the third decimal at every scale. Both use the same TAA and the same lines.
- **Against the phone's own base**, the floor is FLIP p99 0.206 (Mobile-High face), 0.248 (Mobile-Low face) and 0.264 (Mobile-Low ward). The 0.85 rung is 0.176, 0.213 and 0.224.

## Each optional pass, off

Each pass was shed one step at the tier's own scale (`--shed`, below). FLIP is against all passes on. The GPU saving is the median of three alternating rounds of `--bench 240` (60 warmup frames, then 240 timed), with the range of the rounds' differences in brackets. Desktop-Low ran at 1920×1080, and Mobile-Low at 2532×1170 at its own 0.504.

| Pass | Desktop-Low saves (face / ward / duel), µs | Mobile-Low saves (face / ward / duel), µs | Look (FLIP mean / p99 / worst tile, all shots and tiers) |
|---|---|---|---|
| GTAO off (capsules stay) | 448 [438, 467] / 724 [720, 741] / 624 [606, 628]: 12–14% of the frame | none: Mobile-Low has no GTAO | 0.013–0.039 / 0.087–0.13 / 0.12–0.33: contact shadow under eaves, benches and the character's hair and collar |
| Cirrus off | −4 / 0 / 3, within noise | none: Mobile-Low has no cirrus | ≤ 0.004 / ≤ 0.019 / up to 0.64 where the cirrus is the sky's only detail (Mobile-High's hit) |
| A wind level fewer | −10 / −6 / 9, within noise | 11 / −13 / 4, within noise | 0.003–0.011 / 0.10–0.32 / 0.39–0.66: leaves stop fluttering, so every leaf moves |
| Sky faces half as often | 24 [8, 27] / 49 [40, 82] / 19 [18, 29]: 0.5–1% | 19 [−1, 29] / 59 [−12, 77] / 13 [4, 66]: 1–3% | exactly 0 in every still capture. A sun moving faster than a few degrees a minute would show it |
| A bloom level fewer | 7 / 13 / 12: 0.2–0.3% | 171 [166, 188] / 276 [196, 288] / 189 [153, 246]: 11–13% | 0.0014–0.005 / 0.015–0.083 / 0.03–0.45, the worst at the hit's glow |
| Volumetric fog | not built: nothing to shed | | |
| The particle cap | the duel's 250 particles and the hit's burst fit every tier's cap (Mobile-Low's is 512), so a lower cap saves nothing at today's loads | | |

- **Desktop-Low's frame** with all passes on is 3.81 ms (face), 5.00 ms (ward) and 4.47 ms (duel).
- **Mobile-Low's frame** is 1.30, 2.17 and 1.53 ms.
- **Mobile-Low's bloom saving** shows in the frame's span, not in the bloom pass's own time, which falls only 8–12 µs. One bloom level fewer is one dependent pass fewer at the end of the frame. On the M5 that shortens the frame by about 13%. A phone's tile-based GPU may differ, so T2 should check it on the emulator and owe a device check.
- **The wind level and the cirrus** save nothing a bench can see. The wind level is vertex work over a few hundred cards. The cirrus is a few instructions in the sky cube's faces, which update a sixth at a time.

## Which floors keep the look

- **Desktop, 0.7: keeps it.**
  - The character's close-up doesn't move: ×1.007 line strength and ×0.997 contrast, with spread ×0.987.
  - The hit's lines hold too.
  - The ward's wide shot loses 11% of its line strength at 0.7, against 2% at 0.85. Its lines are a pixel or less wide at that distance, and TAA can't rebuild coverage that a 0.7 grid misses.
  - That is thinning, not a broken look. The character's lines, which the look is judged by, hold.
- **Mobile-High, 0.75 (0.474): keeps it.** Its edge at the floor (2.85 px) is still harder than Mobile-Low's own base (3.25 px), and its line contrast is close to Mobile-Low's own base: 0.855 against 0.878 in the test, ×0.703 against ×0.720 in the face. It draws no softer than a tier already ships.
- **Mobile-Low, 0.75 (0.378, 442 rows on a 1170-row phone): at the edge.**
  - It is the softest frame any tier draws: a 3.6 px two-tone edge, line contrast 0.785 in the test and ×0.65 in the face, and the face's edge spread ×1.67.
  - Against Mobile-Low's own base it adds about 10% to the edge and takes about 10% from the line contrast.
  - It doesn't break the look outright. But no rung after it has anything left of the look to spend.
  - **Proposed:** keep 0.75 on Mobile-Low only as the ladder's third rung, after the optional passes, and never as dynamic resolution's everyday floor. Dynamic resolution on Mobile-Low would stop at 0.85 (0.429).

## The ladder's order

§3 puts resolution to 0.85 first, then the optional passes, then the floor. The evidence below says that order is right on desktop and wrong on phones. §3 is not edited here: the orchestrator decides.

**What resolution saves** was benched like the passes: three alternating rounds of `--bench 240`, with `--render-scale` absolute, against the tier's own scale.

| Shot | Desktop-Low 0.85 | Desktop-Low 0.7 | Mobile-Low 0.85 (0.429) | Mobile-Low floor (0.378) |
|---|---|---|---|---|
| face | 571 µs [557, 590], 15% | 1055 µs [1045, 1065], 28% | 142 µs [122, 191], 11% | 233 µs [211, 284], 18% |
| ward | 754 µs [710, 770], 15% | 1422 µs [1421, 1450], 28% | 46 µs [−107, 65], 2% | 172 µs [116, 182], 8% |
| duel | 612 µs [593, 630], 14% | 1122 µs [1103, 1182], 25% | 175 µs [167, 596], 11% | 359 µs [256, 747], 22% |

On Mobile-Low most of what a lower scale saves is the main pass. Post, the upscale and the MSAA resolve stay at the output's size.

**Desktop: §3's order holds.**
- 0.85 saves 14–15% of the frame. It keeps the character's lines and edge (×0.998 contrast in the close-up), and the wide shot loses 2% of its line strength.
- GTAO, the one optional pass worth anything, saves 12–14% and takes the contact shadow out of the environment and the character's collar and hair.
- These are near enough in cost and in look that either can go first. 0.85 first is what §3 says, and nothing here argues against it.

**Phones: passes before resolution.**
- On Mobile-Low, a bloom level saves 171–276 µs (11–13%), as much as 0.85 saves (2–11%). It leaves the lines and the two-tone edge as they were: FLIP mean ≤ 0.005, with the change confined to the glow.
- 0.85 takes the face's line contrast from ×0.720 to ×0.678 and widens its edges by 9%, on a tier whose cap has already softened them most.
- So on the phone tiers, resolution first spends the look the pillars rank highest (the hard two-tone edge and the stable lines) before the parts the look can spare.

**Proposed rungs.** The rungs are data per tier in `tiers.ron`, so this is a choice of data, not a new mechanism:

| Rung | Desktop-High, Desktop-Low | Mobile-High, Mobile-Low |
|---|---|---|
| 1 | resolution to 0.85 | the sky's face rate, a bloom level, and GTAO on Mobile-High |
| 2 | the sky's face rate, GTAO, a bloom level | resolution to 0.85 |
| 3 | resolution to the floor (0.7) | resolution to the floor: Mobile-High 0.75; Mobile-Low 0.75 as a ladder rung only |
| 4 | a 30 fps cap | a 30 fps cap |
| 5 | a lower tier at a pause or wipe | a lower tier at a pause or wipe |

- **Dynamic resolution's floor** stays 0.7 on desktop and 0.75 on Mobile-High. On Mobile-Low it stops at 0.85 (0.429 absolute on a 1170-row phone): its 0.75 floor is the softest frame any tier draws.
- **The cirrus and the wind level** save nothing measurable here. The wind level is also the one shed that changes the look visibly, since leaves stop fluttering. Propose leaving both out of the rungs, or listing them last, until a phone shows they cost something.
- **The particle cap** saves nothing at today's loads, which fit every tier's cap. It stays a guard for heavier fights.
- **Mobile-High** was not benched (the brief asked for the low tiers). Its GTAO is 1 slice × 4 steps at 0.75 m, so expect a smaller saving than Desktop-Low's.

## The golden scenes

These 12 new scenes are T2's baselines: each tier at its own scale and at its floor for the close-up, and the ward and the hit on both low tiers. Only their Metal goldens are committed. Existing goldens are untouched.

- **Desktop scenes** are 640×360 with 12 frames; the hit adds `--capture-step 1`.
- **Phone scenes** are 1280×592 with one frame, at the phone's absolute scale, so the upscale is the 1170-row phone's (about 2×).
- **Metal times:** each scene took 4–5 s on a warm Metal run, of which 0.7–1.7 s was the capture.
- **The lavapipe and WARP estimates** are scaled from today's costs of similar scenes: `lookdev-face` 34 s and 73 s at 32 frames, `hit-medium-noon` 16 s and 34 s, `vfx-duel-mobile-low` 19 s and 32 s, `lookdev-town-day` 64 s and 97 s. They are not measured.

| Scene | Tier, scale | Metal (s) | lavapipe est. (s) | WARP est. (s) |
|---|---|---|---|---|
| tier-face-desktop-high | Desktop-High, 1 | 5 | 15 | 32 |
| tier-face-desktop-high-floor | Desktop-High, 0.7 | 5 | 13 | 28 |
| tier-face-desktop-low | Desktop-Low, 1 | 4 | 15 | 32 |
| tier-face-desktop-low-floor | Desktop-Low, 0.7 | 4 | 13 | 28 |
| tier-face-mobile-high | Mobile-High, 0.632 | 5 | 8 | 20 |
| tier-face-mobile-high-floor | Mobile-High, 0.474 | 4 | 7 | 18 |
| tier-face-mobile-low | Mobile-Low, 0.504 | 5 | 7 | 18 |
| tier-face-mobile-low-floor | Mobile-Low, 0.378 | 4 | 6 | 16 |
| tier-ward-desktop-low | Desktop-Low, 1 | 4 | 25 | 45 |
| tier-ward-mobile-low | Mobile-Low, 0.504 | 5 | 15 | 30 |
| tier-hit-desktop-low | Desktop-Low, 1 | 5 | 16 | 34 |
| tier-hit-mobile-low | Mobile-Low, 0.504 | 4 | 12 | 28 |

That's about 150 s of lavapipe and 330 s of WARP across the shards. `scenes/costs.rs` has no rows for them yet, so each counts as its family's costliest until CI's timings give real ones.

## What was added

- **`QualitySettings::shed`** (`OptionalPasses`, a set of `OptionalPass`) is the ladder's second rung as data. Each pass sheds one step:
  - GTAO to none, with the capsules kept;
  - the cirrus off;
  - one wind level fewer;
  - the sky cube's interval doubled, to at most `SkySettings::MAX_CUBE_INTERVAL` (60);
  - one bloom level fewer.

  It applies after the player's own shadow and AO settings. It never touches the character family, the outlines, the ramps or the grade. The sandbox takes it as `--shed <pass>`, which repeats.
- **`sapphire_testkit::look`:** the measures above, as a library. `sapphire_testkit` is now a dev-dependency of `sapphire_render`.
- **`look_tiers`:** the GPU test above.
