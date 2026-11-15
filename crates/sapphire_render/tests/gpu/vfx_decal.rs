//! Ground decals (M5 L7): a decal lying where the ground is never fights it, near and far, head-on and at a
//! grazing angle, and never shows over a sole a centimetre above it close up; each shape covers its footprint and
//! nothing past it; the fill grows over its reach; light adds and darkening multiplies, and fog in front takes
//! its share; decals count in the main pass; and nothing of the family is built until a decal draws or a load
//! declares one. Run with
//! `cargo test -p sapphire_render --test gpu vfx_decal:: -- --ignored --test-threads=1`, and with
//! `-- --nocapture` to see the margins each backend reads back.
//!
//! The scenes are `vfx`'s: linear values as they are, over a flat grey sky, the camera at the origin looking down
//! -Z over 90°. The ground is a glowing square turned to face the camera at an angle, so a pixel's ray says where
//! on it, and on the decal, it lands.

use glam::{Mat4, Quat, Vec2, Vec3};
use sapphire_render::{
    Fog, FogLayer, FrameCounts, HeightFog, Msaa, PassCounts, PipelineInventory, RenderSettings, VfxDecal,
    VfxDecalBlend, VfxDecalShape,
};

use super::vfx::{GREY, Harness, SIDE, TOLERANCE, assert_near, flat_settings, temporal_settings};

/// What the ground glows, linear.
const GROUND: f32 = 0.2;
/// The light a test's decal adds.
const LIGHT: Vec3 = Vec3::new(0.3, 0.3, 0.3);

/// The paths a decal draws through: the main pass with no samples, with four, and under temporal AA. Every
/// tier draws decals in the main pass; the paths differ in how its depth is sampled.
fn paths() -> [(&'static str, RenderSettings); 3] {
    [
        ("no MSAA", flat_settings()),
        ("4x MSAA", RenderSettings { msaa: Msaa::X4, bloom_levels: 5, ..RenderSettings::default() }),
        ("temporal AA", temporal_settings()),
    ]
}

/// A plane facing the camera from straight ahead, `distance` down -Z, its normal tilted `angle` degrees from
/// facing it, toward +Y: 0 is ground seen from straight above, 85 ground seen five degrees above its horizon.
#[derive(Clone, Copy, Debug)]
struct View {
    distance: f32,
    angle: f32,
}

impl View {
    fn middle(self) -> Vec3 {
        Vec3::new(0.0, 0.0, -self.distance)
    }

    fn normal(self) -> Vec3 {
        let (sin, cos) = self.angle.to_radians().sin_cos();
        Vec3::new(0.0, sin, cos)
    }

    /// The decal's reach, and the ground's half side past it: wide enough to fill most of the view.
    fn reach(self) -> f32 {
        (self.distance * 40.0).min(900.0)
    }

    /// The ground, a square glowing [`GROUND`], `rise` metres along its normal.
    fn ground(self, rise: f32) -> Mat4 {
        let rotation = Quat::from_rotation_arc(Vec3::Z, self.normal());
        let at = self.middle() + self.normal() * rise;
        Mat4::from_scale_rotation_translation(Vec3::splat(self.reach() * 1.2), rotation, at)
    }

    /// How a decal lies on the ground: its +Y the ground's normal.
    fn rotation(self) -> Quat {
        Quat::from_rotation_arc(Vec3::Y, self.normal())
    }

    /// A disc of light over the view's middle, `offset` metres along the normal from the ground: above it if
    /// positive.
    fn decal(self, offset: f32) -> VfxDecal {
        self.decal_of(self.reach(), offset)
    }

    /// [`Self::decal`] of radius `reach`.
    fn decal_of(self, reach: f32, offset: f32) -> VfxDecal {
        VfxDecal {
            position: self.middle() + self.normal() * offset,
            ..decal(VfxDecalShape::Disc { radius: reach }, self.rotation(), LIGHT)
        }
    }

    /// How far the camera stands over the ground.
    fn height(self) -> f32 {
        self.normal().dot(-self.middle())
    }

    /// Where on the plane pixel `(x, y)`'s middle looks, if it looks at it.
    fn hit(self, at: (u32, u32)) -> Option<Vec3> {
        self.hit_over(at, 0.0)
    }

    /// Where on the plane raised `rise` metres along its normal pixel `(x, y)`'s middle looks, if it looks at it.
    fn hit_over(self, (x, y): (u32, u32), rise: f32) -> Option<Vec3> {
        let direction = ray((x, y));
        let facing = self.normal().dot(direction);
        let plane = self.middle() + self.normal() * rise;
        (facing < -1e-4).then(|| direction * (self.normal().dot(plane) / facing))
    }

    /// The pixels the view is about: where they look lands within twice its distance of the camera, so a margin
    /// read in metres is one at that distance, and three times the near plane's distance (5 cm) away or more, so
    /// neither the ground nor a decal half way up to the camera is clipped. All well inside the decal.
    fn inside(self) -> Vec<(u32, u32)> {
        self.inside_of(self.reach())
    }

    /// [`Self::inside`] for a decal of radius `reach`.
    fn inside_of(self, reach: f32) -> Vec<(u32, u32)> {
        let inside = |hit: Vec3| {
            hit.distance(self.middle()) < 0.8 * reach && hit.length() < 2.0 * self.distance && -hit.z > 0.15
        };
        pixels().filter(|&at| self.hit(at).is_some_and(inside)).collect()
    }
}

/// Pixel `(x, y)`'s middle's ray from the camera.
fn ray((x, y): (u32, u32)) -> Vec3 {
    let side = SIDE as f32;
    Vec3::new((x as f32 + 0.5) / side * 2.0 - 1.0, 1.0 - (y as f32 + 0.5) / side * 2.0, -1.0).normalize()
}

fn pixels() -> impl Iterator<Item = (u32, u32)> {
    (0..SIDE).flat_map(|y| (0..SIDE).map(move |x| (x, y)))
}

/// A whole decal at the origin, filled, no outline or noise, of light `color`.
fn decal(shape: VfxDecalShape, rotation: Quat, color: Vec3) -> VfxDecal {
    VfxDecal {
        shape,
        position: Vec3::ZERO,
        rotation,
        color,
        alpha: 1.0,
        blend: VfxDecalBlend::Light,
        fill: 1.0,
        outline: 0.0,
        body: 1.0,
        noise: 0.0,
    }
}

/// A frame of `decal` over the harness's surfaces; under temporal AA the fourth alike, so the history holds
/// nothing of the frames before.
fn settled(harness: &mut Harness, temporal: bool, decal: VfxDecal) -> super::vfx::Frame {
    let frames = if temporal { 4 } else { 1 };
    let mut frame = harness.render(|packet| packet.vfx.decals.push(decal));
    for _ in 1..frames {
        frame = harness.render(|packet| packet.vfx.decals.push(decal));
    }
    frame
}

/// How many of `inside` show none of the decal's light.
fn misses(frame: &super::vfx::Frame, inside: &[(u32, u32)]) -> usize {
    inside.iter().filter(|&&at| frame.linear(at).x - GROUND < LIGHT.x * 0.5).count()
}

/// Offsets along the ground's normal, metres, in order: from `-reach` to `reach` in quarter octaves each way,
/// down to a 2^30th of it.
fn offsets(reach: f32) -> Vec<f32> {
    let step = |k: i32| reach * 2_f32.powf(-0.25 * k as f32);
    let below = (0..=120).map(move |k| -step(k));
    let above = (0..=120).rev().map(step);
    below.chain([0.0]).chain(above).collect()
}

/// The least offset of [`offsets`]`(top)` at which `decal` (given its offset) shows whole at the pixels `inside`,
/// and its step in them: negative is how far under the ground a decal can sink and still show, the pull's reach
/// past the depths' own mismatch. `None` if it doesn't show whole even `top` over the ground.
fn least_whole_offset(
    harness: &mut Harness,
    decal: impl Fn(f32) -> VfxDecal,
    top: f32,
    inside: &[(u32, u32)],
    temporal: bool,
) -> Option<(usize, f32)> {
    let offsets = offsets(top);
    let whole = |harness: &mut Harness, offset: f32| {
        let frame = settled(harness, temporal, decal(offset));
        misses(&frame, inside) == 0
    };
    // The decal shows whole that far over the ground; it's hidden as far under it but where the slope term
    // reaches further (far and grazing, where a pixel spans tens of metres of ground). Bisect between.
    let (mut low, mut high) = (0, offsets.len() - 1);
    if !whole(harness, offsets[high]) {
        return None;
    }
    if whole(harness, offsets[low]) {
        return Some((low, offsets[low]));
    }
    while high - low > 1 {
        let mid = (low + high) / 2;
        if whole(harness, offsets[mid]) {
            high = mid;
        } else {
            low = mid;
        }
    }
    Some((high, offsets[high]))
}

/// What one view read on one path.
#[derive(Clone, Debug)]
struct Reading {
    path: &'static str,
    view: View,
    /// The pixels measured.
    inside: usize,
    /// Of those, how many miss the decal lying on the ground...
    lying_misses: usize,
    /// ...and of every seventeenth, how many are off its color.
    lying_off: usize,
    /// [`least_whole_offset`]'s.
    least: Option<f32>,
}

impl Reading {
    /// What's wrong with it, if anything.
    fn faults(&self) -> Vec<String> {
        let mut faults = Vec::new();
        if self.inside <= 300 {
            faults.push(format!("the decal fills too little of the view: {} pixels", self.inside));
        }
        if self.lying_misses > 0 {
            faults.push(format!("lying on the ground, {} pixels miss it", self.lying_misses));
        }
        if self.lying_off > 0 {
            faults.push(format!("lying on the ground, {} pixels are off its color", self.lying_off));
        }
        match self.least {
            None => faults.push("it doesn't show whole even over the ground".to_owned()),
            Some(least) if least >= 0.0 => faults.push(format!("no margin under the ground: {least} m")),
            Some(least) if least <= -2e-3 * self.view.distance => faults.push(format!("floats {least} m")),
            Some(_) => {}
        }
        faults
    }

    fn row(&self) -> String {
        let least = self.least.map_or_else(
            || "never whole".to_owned(),
            |least| format!("{least:+.3e} m ({:+.3e} of the distance)", least / self.view.distance),
        );
        let faults = self.faults();
        let verdict =
            if faults.is_empty() { String::new() } else { format!("  FAILS: {}", faults.join("; ")) };
        format!(
            "{:>11}: {:>5} m, {:>2}°: {:>5} px, shows whole down to {least}{verdict}",
            self.path, self.view.distance, self.view.angle, self.inside
        )
    }
}

/// What one view read on one path with a small decal and with its whole reach, at the small one's pixels.
#[derive(Clone, Debug)]
struct SizeReading {
    path: &'static str,
    view: View,
    /// The pixels measured, the small decal's.
    inside: usize,
    /// [`least_whole_offset`]'s for the small decal and for the view's whole reach.
    small: Option<(usize, f32)>,
    whole: Option<(usize, f32)>,
}

impl SizeReading {
    fn faults(&self) -> Vec<String> {
        let mut faults = Vec::new();
        if self.inside <= 20 {
            faults.push(format!("the small decal covers too few pixels: {}", self.inside));
        }
        match (self.small, self.whole) {
            (Some((small, _)), Some((whole, _))) if small.abs_diff(whole) <= 1 => {}
            (Some(_), Some(_)) => faults.push("their margins differ by more than a step".to_owned()),
            _ => faults.push("one of them never shows whole".to_owned()),
        }
        faults
    }

    fn row(&self) -> String {
        let margin = |reading: Option<(usize, f32)>| {
            reading.map_or_else(|| "never whole".to_owned(), |(_, least)| format!("{least:+.3e} m"))
        };
        let faults = self.faults();
        let verdict =
            if faults.is_empty() { String::new() } else { format!("  FAILS: {}", faults.join("; ")) };
        format!(
            "{:>11}: {:>5} m, {:>2}°: {:>5} px, {SMALL_REACH} m round down to {}, {} m round down to {}{verdict}",
            self.path,
            self.view.distance,
            self.view.angle,
            self.inside,
            margin(self.small),
            self.view.reach(),
            margin(self.whole),
        )
    }
}

/// The small decal's radius, a telegraph's size or so.
const SMALL_REACH: f32 = 12.0;

/// The views where a decal of the whole reach crosses the near plane, the camera standing over it, and one of
/// [`SMALL_REACH`] doesn't come near the camera.
const SIZE_VIEWS: [View; 2] = [View { distance: 20.0, angle: 60.0 }, View { distance: 150.0, angle: 60.0 }];

/// Every view's reading on every path, none stopping the rest; and at [`SIZE_VIEWS`], a small decal's margin and
/// the whole reach's at the same pixels.
fn readings() -> (Vec<Reading>, Vec<SizeReading>) {
    let mut readings = Vec::new();
    let mut sizes = Vec::new();
    for (path, settings) in paths() {
        let mut harness = Harness::new(settings);
        let mut ground = harness.add_surface(Mat4::IDENTITY, Vec3::splat(GROUND));
        harness.render(|_| {});
        for view in VIEWS {
            let inside = view.inside();
            ground.transform = view.ground(0.0);
            harness.place(ground);
            let lying = settled(&mut harness, settings.temporal, view.decal(0.0));
            let expected = Vec3::splat(GROUND) + LIGHT;
            let lying_off = inside
                .iter()
                .step_by(17)
                .filter(|&&at| (lying.linear(at) - expected).abs().max_element() >= TOLERANCE)
                .count();
            readings.push(Reading {
                path,
                view,
                inside: inside.len(),
                lying_misses: misses(&lying, &inside),
                lying_off,
                // Half the way from the plane to the camera, which a decal above it mustn't pass.
                least: least_whole_offset(
                    &mut harness,
                    |offset| view.decal(offset),
                    0.5 * view.height(),
                    &inside,
                    settings.temporal,
                )
                .map(|(_, least)| least),
            });
        }
        for view in SIZE_VIEWS {
            ground.transform = view.ground(0.0);
            harness.place(ground);
            let inside = view.inside_of(SMALL_REACH);
            // Both on the same steps, up to where the small one raised still covers its pixels.
            let top = (0.5 * view.height()).min(0.1 * SMALL_REACH);
            let mut least = |reach: f32| {
                let decal = |offset| view.decal_of(reach, offset);
                least_whole_offset(&mut harness, decal, top, &inside, settings.temporal)
            };
            let small = least(SMALL_REACH);
            let whole = least(view.reach());
            sizes.push(SizeReading { path, view, inside: inside.len(), small, whole });
        }
    }
    (readings, sizes)
}

/// The readings as a table, one row a view and path, with what fails.
fn table(readings: &[Reading], sizes: &[SizeReading]) -> String {
    let rows: Vec<String> =
        readings.iter().map(Reading::row).chain(sizes.iter().map(SizeReading::row)).collect();
    rows.join("\n")
}

const VIEWS: [View; 12] = [
    View { distance: 0.3, angle: 0.0 },
    View { distance: 0.3, angle: 60.0 },
    View { distance: 0.3, angle: 85.0 },
    View { distance: 3.0, angle: 0.0 },
    View { distance: 3.0, angle: 60.0 },
    View { distance: 3.0, angle: 85.0 },
    View { distance: 20.0, angle: 0.0 },
    View { distance: 20.0, angle: 60.0 },
    View { distance: 20.0, angle: 85.0 },
    View { distance: 150.0, angle: 0.0 },
    View { distance: 150.0, angle: 60.0 },
    View { distance: 150.0, angle: 85.0 },
];

/// A decal lying exactly where the ground is shows whole over it, at every pixel, from 30 cm to 150 m away and
/// from straight above to five degrees over the ground's horizon, on every path: no z-fighting. And it draws
/// whole sunk a little under the ground, by its pull's reach past the depths' own mismatch, under two millimetres
/// per metre away everywhere at this test's 128 pixels over 90°, where a pixel is 15 times as wide as at 1080p
/// over 60°: so a decal never visibly floats. The decal is as wide as the view's reach (up to 900 m round), so at
/// 60° and 85° the camera stands over it and its quad crosses the near plane.
///
/// And the margin doesn't hang on the decal's size: at 60°, 20 m and 150 m away, a decal of [`SMALL_REACH`] and
/// one of the whole reach read the same margin at the small one's pixels, a step apart at most. That's what a
/// rasterizer's constant depth bias lacks on Direct3D and Vulkan, which size it by the quad's nearest depth: WARP
/// let the large decal sink 4.6e-3 of the distance at 150 m (the module's docs in `vfx/decal.rs`).
///
/// Every view is read on every path before anything is judged, and the whole table is printed (`--nocapture`) and
/// given with any failure. The pull and the slope term were chosen from these readbacks, with nothing at all to
/// see the mismatch they must cover.
#[test]
#[ignore = "needs a GPU"]
fn a_decal_on_the_ground_never_fights_it_near_or_far_or_grazing() {
    let (readings, sizes) = readings();
    let table = table(&readings, &sizes);
    eprintln!("{table}");
    let failing = readings.iter().filter(|reading| !reading.faults().is_empty()).count()
        + sizes.iter().filter(|reading| !reading.faults().is_empty()).count();
    assert_eq!(failing, 0, "{failing} of {} readings fail:\n{table}", readings.len() + sizes.len());
}

/// A pixel here, radians across: 90° over 128 pixels. And one at 1080p over a 60° view.
const TEST_PIXEL: f32 = 2.0 / SIDE as f32;
const HD_PIXEL: f32 = 1.0717e-3;

/// A sole a centimetre over a decal hides it at 1080p, from a metre and a half to six metres away and steeply or at
/// a player's camera's angle: a foot on the ground covers a telegraph as the ground would be covered. The bias's
/// slope term is a share of the depth's change across a pixel, so the sole here is as thick against this test's
/// pixels as a centimetre is against 1080p's: 14.6 cm.
#[test]
#[ignore = "needs a GPU"]
fn a_sole_a_centimetre_over_a_decal_hides_it_at_1080p() {
    let thickness = 0.01 * TEST_PIXEL / HD_PIXEL;
    let sole_color = Vec3::new(0.05, 0.1, 0.4);
    for (path, settings) in paths() {
        let mut harness = Harness::new(settings);
        let mut ground = harness.add_surface(Mat4::IDENTITY, Vec3::splat(GROUND));
        let mut sole = harness.add_surface(Mat4::IDENTITY, sole_color);
        for view in [
            View { distance: 1.5, angle: 30.0 },
            View { distance: 1.5, angle: 70.0 },
            View { distance: 6.0, angle: 30.0 },
            View { distance: 6.0, angle: 70.0 },
        ] {
            ground.transform = view.ground(0.0);
            harness.place(ground);
            // A sole a quarter of the distance across over the middle.
            let half = 0.12 * view.distance;
            sole.transform = Mat4::from_translation(view.normal() * thickness)
                * Mat4::from_scale_rotation_translation(
                    Vec3::new(half, half, 1.0),
                    Quat::from_rotation_arc(Vec3::Z, view.normal()),
                    view.middle(),
                );
            harness.place(sole);
            let frame = settled(&mut harness, settings.temporal, view.decal(0.0));
            // Pixels whose ray lands well inside the sole's square.
            let rotation = Quat::from_rotation_arc(Vec3::Z, view.normal());
            let on_sole: Vec<_> = pixels()
                .filter(|&at| {
                    let Some(hit) = view.hit_over(at, thickness) else { return false };
                    let local = rotation.inverse() * (hit - view.middle() - view.normal() * thickness);
                    local.x.abs() < half * 0.7 && local.y.abs() < half * 0.7
                })
                .collect();
            assert!(on_sole.len() > 20, "{path} {view:?}: the sole is in the frame: {}", on_sole.len());
            for at in on_sole {
                assert_near(frame.linear(at), sole_color, &format!("{path} {view:?}: the sole at {at:?}"));
            }
            sole.transform = Mat4::from_translation(Vec3::new(0.0, 0.0, 10.0));
            harness.place(sole);
        }
    }
}

/// Facing the camera two metres away a pixel is 1/32 m: each shape's decal shows whole more than a pixel inside
/// its edge and not at all more than a pixel outside, its signed distance as `sapphire_look` has it.
#[test]
#[ignore = "needs a GPU"]
fn each_shape_covers_its_footprint_and_nothing_past_it() {
    let view = View { distance: 2.0, angle: 0.0 };
    let pixel = 1.0 / 32.0;
    let shapes = [
        VfxDecalShape::Disc { radius: 0.9 },
        VfxDecalShape::Ring { inner: 0.4, outer: 0.9 },
        VfxDecalShape::Sector { radius: 1.6, angle: 100.0, inner: 0.2 },
        VfxDecalShape::Lane { width: 0.8, length: 1.5 },
    ];
    for (path, settings) in paths() {
        let mut harness = Harness::new(settings);
        let mut ground = harness.add_surface(Mat4::IDENTITY, Vec3::splat(GROUND));
        ground.transform = view.ground(0.0);
        harness.place(ground);
        // Turned so its +Z runs across the frame toward -Y, the sector's and lane's reach down from the middle.
        let rotation = view.rotation();
        // Temporal AA's jitter and filter soften an edge by about a pixel more.
        let margin = if settings.temporal { 2.5 * pixel } else { 1.5 * pixel };
        for shape in shapes {
            let placed = VfxDecal { position: view.middle(), ..decal(shape, rotation, LIGHT) };
            let frame = settled(&mut harness, settings.temporal, placed);
            let (mut inside, mut outside) = (0, 0);
            for at in pixels() {
                let hit = view.hit(at).expect("facing the camera");
                let local = rotation.inverse() * (hit - view.middle());
                let distance = shape.distance(Vec2::new(local.x, local.z));
                let what = format!("{path} {shape:?} at {at:?}, {distance:+.3} m from its edge");
                if distance < -margin {
                    assert_near(frame.linear(at), Vec3::splat(GROUND) + LIGHT, &what);
                    inside += 1;
                } else if distance > margin {
                    assert_near(frame.linear(at), Vec3::splat(GROUND), &what);
                    outside += 1;
                }
            }
            assert!(inside > 200 && outside > 200, "{path} {shape:?}: {inside} inside, {outside} outside");
        }
    }
}

/// A lane half filled, its body a fifth: whole light where the fill has reached, a fifth past it, and its
/// outline whole all round.
#[test]
#[ignore = "needs a GPU"]
fn the_fill_grows_along_the_reach() {
    let view = View { distance: 2.0, angle: 0.0 };
    let pixel = 1.0 / 32.0;
    let shape = VfxDecalShape::Lane { width: 1.0, length: 1.6 };
    let mut harness = Harness::new(flat_settings());
    let mut ground = harness.add_surface(Mat4::IDENTITY, Vec3::splat(GROUND));
    ground.transform = view.ground(0.0);
    harness.place(ground);
    let rotation = view.rotation();
    let lane = VfxDecal {
        position: view.middle(),
        fill: 0.5,
        body: 0.2,
        outline: 0.1,
        ..decal(shape, rotation, LIGHT)
    };
    let frame = harness.render(|packet| packet.vfx.decals.push(lane));
    let mut counts = [0; 3];
    for at in pixels() {
        let hit = view.hit(at).expect("facing the camera");
        let local = rotation.inverse() * (hit - view.middle());
        let point = Vec2::new(local.x, local.z);
        let distance = shape.distance(point);
        if distance > -0.1 - 1.5 * pixel {
            // The outline, its edges and outside it: the shape's test has them.
            continue;
        }
        let progress = shape.progress(point);
        let what = format!("at {at:?}, progress {progress:.3}");
        if progress < 0.5 - 1.5 * pixel / 1.6 {
            assert_near(frame.linear(at), Vec3::splat(GROUND) + LIGHT, &what);
            counts[0] += 1;
        } else if progress > 0.5 + 1.5 * pixel / 1.6 {
            assert_near(frame.linear(at), Vec3::splat(GROUND) + LIGHT * 0.2, &what);
            counts[1] += 1;
        }
    }
    for at in pixels() {
        let local = rotation.inverse() * (view.hit(at).expect("facing") - view.middle());
        let distance = shape.distance(Vec2::new(local.x, local.z));
        if distance < -1.5 * pixel && distance > -0.1 + 1.5 * pixel {
            assert_near(frame.linear(at), Vec3::splat(GROUND) + LIGHT, &format!("the outline at {at:?}"));
            counts[2] += 1;
        }
    }
    assert!(counts.iter().all(|&count| count > 50), "filled, unfilled and outline pixels: {counts:?}");
}

/// Light adds its color, darkening multiplies the ground toward its own, half opacity does half of either, a
/// telegraph over a scorch glows over the darkened ground, and fog in front takes its share of the light.
#[test]
#[ignore = "needs a GPU"]
fn light_adds_darkening_multiplies_and_fog_takes_its_share() {
    let view = View { distance: 4.0, angle: 0.0 };
    let shape = VfxDecalShape::Disc { radius: 3.0 };
    let glow = Vec3::new(0.3, 0.12, 0.05);
    let tint = Vec3::new(0.5, 0.25, 1.0);
    let ground_color = Vec3::splat(0.4);
    for (path, settings) in paths() {
        let mut harness = Harness::new(settings);
        let mut ground = harness.add_surface(Mat4::IDENTITY, ground_color);
        ground.transform = view.ground(0.0);
        harness.place(ground);
        let light = decal(shape, view.rotation(), glow);
        let light = VfxDecal { position: view.middle(), ..light };
        let darken = VfxDecal { color: tint, blend: VfxDecalBlend::Darken, ..light };
        let shown = |harness: &mut Harness, decals: &[VfxDecal]| {
            harness.render(|packet| packet.vfx.decals.extend_from_slice(decals)).middle()
        };
        assert_near(shown(&mut harness, &[]), ground_color, &format!("{path}: the ground"));
        assert_near(shown(&mut harness, &[light]), ground_color + glow, &format!("{path}: light"));
        assert_near(shown(&mut harness, &[darken]), ground_color * tint, &format!("{path}: darkening"));
        let half = |decal: VfxDecal| VfxDecal { alpha: 0.5, ..decal };
        assert_near(
            shown(&mut harness, &[half(light)]),
            ground_color + glow * 0.5,
            &format!("{path}: half light"),
        );
        let half_dark = ground_color * (Vec3::ONE + (tint - Vec3::ONE) * 0.5);
        assert_near(shown(&mut harness, &[half(darken)]), half_dark, &format!("{path}: half darkening"));
        // Whatever order the frame has them in.
        let both = ground_color * tint + glow;
        assert_near(shown(&mut harness, &[light, darken]), both, &format!("{path}: light over darkening"));

        let fog = Fog {
            near: FogLayer { color: Vec3::new(0.6, 0.5, 0.4), density: 0.15, start: 0.5 },
            far: FogLayer::default(),
            height: HeightFog::default(),
            sun_inscatter: Vec3::ZERO,
            characters: 1.0,
        };
        let ray = fog.ray(Vec3::ZERO, Vec3::NEG_Z, view.distance, 1.0, Vec3::splat(GREY), Vec3::NEG_Z);
        assert!((0.2..0.8).contains(&ray.transmittance), "the test's fog: {}", ray.transmittance);
        let fogged = |harness: &mut Harness, decals: &[VfxDecal]| {
            harness
                .render(|packet| {
                    packet.fog = fog;
                    packet.vfx.decals.extend_from_slice(decals);
                })
                .middle()
        };
        let behind = fogged(&mut harness, &[]);
        let lit = fogged(&mut harness, &[light]);
        assert_near(lit, behind + glow * ray.transmittance, &format!("{path}: light through the fog"));
    }
}

/// A frame with no decals builds nothing of them, nor does a frame of sprites; the first decal builds both of
/// the family's pipelines, once, and no pass of its own. Declared, they're built ahead, so the first decal builds
/// none.
#[test]
#[ignore = "needs a GPU"]
fn nothing_of_the_family_is_built_until_a_decal_draws_or_is_declared() {
    let view = View { distance: 3.0, angle: 60.0 };
    let sprite = super::vfx::dot(Vec3::new(0.0, 0.0, -4.0), 0.5, Vec3::ONE);
    for (path, settings) in paths() {
        let mut harness = Harness::new(settings);
        for _ in 0..2 {
            harness.render(|_| {});
        }
        let pipelines = harness.render_pipelines();
        let passes = harness.graph.stats().passes;
        for _ in 0..2 {
            harness.render(|packet| packet.vfx.sprites.push(sprite));
        }
        assert_eq!(harness.render_pipelines(), pipelines + 1, "{path}: the sprites' color pipeline alone");
        assert_eq!(harness.renderer.stats().vfx.decals, 0, "{path}");
        let resources = harness.resources();
        harness.render(|packet| packet.vfx.decals.push(view.decal(0.0)));
        assert_eq!(harness.render_pipelines(), pipelines + 3, "{path}: the decals' two at the first decal");
        assert!(harness.resources().0 > resources.0, "{path}: the decals' buffers come with the first decal");
        assert_eq!(harness.renderer.stats().vfx.decals, 1, "{path}");
        harness.render(|packet| packet.vfx.decals.push(view.decal(0.0)));
        assert_eq!(harness.render_pipelines(), pipelines + 3, "{path}: none again");

        let mut decals_only = Harness::new(settings);
        for _ in 0..2 {
            decals_only.render(|_| {});
        }
        let passes_only = decals_only.graph.stats().passes;
        assert_eq!(passes_only, passes, "{path}");
        decals_only.render(|packet| packet.vfx.decals.push(view.decal(0.0)));
        assert_eq!(decals_only.render_pipelines(), pipelines + 2, "{path}: the decals' two alone");
        assert_eq!(decals_only.renderer.stats().vfx.pipelines, 2, "{path}");
        assert_eq!(
            decals_only.graph.stats().passes,
            passes,
            "{path}: in the main pass, no pass of their own"
        );

        let mut declared = Harness::new(settings);
        declared.render(|_| {});
        let before = declared.render_pipelines();
        declared.renderer.prewarm(&PipelineInventory { decals: true, ..PipelineInventory::default() });
        assert_eq!(declared.render_pipelines(), before + 2, "{path}: the decals' two");
        let first_uses = declared.renderer.stats().pipelines.first_use_builds;
        declared.render(|packet| packet.vfx.decals.push(view.decal(0.0)));
        assert_eq!(declared.render_pipelines(), before + 2, "{path}: none at the first decal");
        assert_eq!(declared.renderer.stats().pipelines.first_use_builds, first_uses, "{path}");
    }
}

/// Decals count in the main pass, where they draw on every path (M5 B4's counts): a call a blend that has any,
/// each decal an instance of a quad's two triangles, with no level (it isn't a scene mesh). No other pass moves,
/// temporal AA's neither, since decals don't wait for its resolve.
#[test]
#[ignore = "needs a GPU"]
fn decals_count_in_the_main_pass() {
    let view = View { distance: 4.0, angle: 0.0 };
    let light = view.decal(0.0);
    let darken = VfxDecal { blend: VfxDecalBlend::Darken, ..light };
    for (path, settings) in paths() {
        let mut harness = Harness::new(settings);
        let mut ground = harness.add_surface(Mat4::IDENTITY, Vec3::splat(GROUND));
        ground.transform = view.ground(0.0);
        harness.place(ground);
        for _ in 0..2 {
            harness.render(|_| {});
        }
        let without = harness.renderer.stats().passes;
        for (decals, calls) in [(&[light, light, darken, light, darken][..], 2), (&[light; 3][..], 1)] {
            harness.render(|packet| packet.vfx.decals.extend_from_slice(decals));
            let with = harness.renderer.stats().passes;
            let (before, after) = (without.main, with.main);
            let decals = decals.len() as u32;
            assert_eq!(after.draws - before.draws, calls, "{path}: a call a blend");
            assert_eq!(after.instances - before.instances, decals, "{path}: an instance a decal");
            assert_eq!(
                after.triangles - before.triangles,
                2 * u64::from(decals),
                "{path}: two triangles a decal"
            );
            assert_eq!(
                (after.considered, after.levels),
                (before.considered, before.levels),
                "{path}: no mesh draw"
            );
            let others = |counts: &FrameCounts| FrameCounts { main: PassCounts::default(), ..*counts };
            assert_eq!(others(&with), others(&without), "{path}: no other pass moves");
        }
    }
}
