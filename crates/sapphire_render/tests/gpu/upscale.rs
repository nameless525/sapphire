//! The phone tiers' upscaler (M5 L14, `post`'s `upscale` module): below render scale 1 the edge upscale and
//! sharpen bring a dark line and a hard two-tone edge closer to what scale 1 draws than the fused pass's bilinear
//! read does, with no halo beside a line against a bright wall or the sky, no ring on the edge, and a flat field
//! flat to the code value; neighbouring scales differ no more than bilinear's do; at scale 1 and on a tier without
//! it nothing of it is built or drawn, and its two passes count exactly in their own row. Run with
//! `cargo test -p sapphire_render --test gpu upscale:: -- --ignored --test-threads=1 --nocapture`.
//!
//! The scenes are `vfx`'s: linear values as they are, the camera at the origin looking down -Z over 90° of the
//! frame's height, 4× MSAA as on the phones. Lines and edges slant, so their rows sample every phase of a scaled
//! scene's pixel grid.

use glam::{Mat4, Quat, Vec3};
use sapphire_render::quality::{TierId, TierTable};
use sapphire_render::{FrameCounts, Msaa, PassCounts, RenderSettings, SkyGradient, Upscaler};
use sapphire_testkit::look::Edge;

use super::vfx::{Frame, Harness};

/// A phone's shape, narrowed: only the height sets what a scale draws.
const SIZE: (u32, u32) = (192, 256);
/// The tiers' floors on a 1170-row phone (`look_tiers`): Mobile-High's 0.75 of its 740 rows, Mobile-Low's
/// ladder's 0.75 of its 590.
const FLOORS: [(&str, f32); 2] = [("Mobile-High's floor", 0.474_359), ("Mobile-Low's floor", 0.378_205)];
/// The tiers' sharpen.
const SHARPEN: u8 = 50;
/// How far the surfaces are, and the wall behind a line.
const NEAR: f32 = 4.0;
const FAR: f32 = 4.5;
/// How far lines and edges turn from upright, in radians.
const SLANT: f32 = 0.12;
/// A metre at `NEAR` in output pixels: the view's 8 m are the frame's height.
const PIXELS_PER_METRE: f32 = SIZE.1 as f32 / (2.0 * NEAR);

fn settings(upscaler: Upscaler) -> RenderSettings {
    RenderSettings { msaa: Msaa::X4, bloom_levels: 4, upscaler, ..RenderSettings::default() }
}

const EDGE: Upscaler = Upscaler::Edge { sharpen: SHARPEN };

/// A band `width` metres wide at depth `z`, its middle `across` metres from the view's axis, slanted, glowing
/// `emission`.
fn add_band(harness: &mut Harness, across: f32, width: f32, z: f32, emission: f32) {
    // Turned about the view's axis, so every band's edges slant through the same place on the middle row.
    let transform = Mat4::from_rotation_z(SLANT)
        * Mat4::from_scale_rotation_translation(
            Vec3::new(width * 0.5, 40.0, 1.0),
            Quat::IDENTITY,
            Vec3::new(across, 0.0, -z),
        );
    harness.add_surface(transform, Vec3::splat(emission));
}

/// What a scene shows.
#[derive(Clone, Copy, Debug)]
enum Scene {
    /// The sky alone, at this brightness.
    Flat(f32),
    /// A dark line four output pixels wide over a bright wall.
    LineOnWall,
    /// The same line against a bright sky.
    LineOnSky,
    /// A hard edge between two tones, the shade a sixth of the light.
    TwoTone,
}

const BRIGHT: f32 = 0.8;
const DARK: f32 = 0.02;

impl Scene {
    fn sky(self) -> f32 {
        match self {
            Self::Flat(level) => level,
            Self::LineOnSky => BRIGHT,
            Self::LineOnWall | Self::TwoTone => 0.25,
        }
    }

    fn harness(self, upscaler: Upscaler) -> Harness {
        let mut harness = Harness::sized(settings(upscaler), SIZE);
        let line = 4.0 / PIXELS_PER_METRE;
        match self {
            Self::Flat(_) => {}
            Self::LineOnWall => {
                add_band(&mut harness, 0.0, 200.0, FAR, BRIGHT);
                add_band(&mut harness, 0.0, line, NEAR, DARK);
            }
            Self::LineOnSky => add_band(&mut harness, 0.0, line, NEAR, DARK),
            Self::TwoTone => {
                add_band(&mut harness, -50.0, 100.0, NEAR, 0.6);
                add_band(&mut harness, 50.0, 100.0, NEAR, 0.1);
            }
        }
        harness
    }

    /// Two frames at `scale`, the last read back.
    fn shot(self, harness: &mut Harness, scale: f32) -> Frame {
        harness.renderer.set_render_scale(scale);
        self.frame(harness);
        self.frame(harness)
    }

    /// One frame at the harness's scale, read back.
    fn frame(self, harness: &mut Harness) -> Frame {
        let sky = Vec3::splat(self.sky());
        harness.render(|packet| packet.sky = SkyGradient { zenith: sky, horizon: sky, ground: sky })
    }
}

/// The red channel's code value at `(x, y)`: every scene is grey.
fn code(frame: &Frame, (x, y): (u32, u32)) -> i32 {
    i32::from(frame.pixels[(y * frame.width + x) as usize][0])
}

/// Rows through the middle half of the frame, clear of its edges.
fn rows() -> impl Iterator<Item = u32> {
    (SIZE.1 / 4..SIZE.1 * 3 / 4).step_by(3)
}

/// The columns round where the slanted line or edge crosses `row`, `half` pixels either side.
fn columns(row: u32, half: u32) -> std::ops::Range<u32> {
    let dy = row as f32 + 0.5 - SIZE.1 as f32 / 2.0;
    // The band turns by `SLANT` about the view's axis, so its middle crosses a row `dy` below the middle (image
    // rows run down, the world's up) at `dy × tan(SLANT)` across.
    let middle = SIZE.0 as f32 / 2.0 + dy * SLANT.tan();
    (middle as u32 - half)..(middle as u32 + half)
}

/// The mean absolute difference from `reference` in code values over the windows round the line or edge.
fn error(frame: &Frame, reference: &Frame) -> f32 {
    let (mut sum, mut count) = (0, 0);
    for row in rows() {
        for x in columns(row, 10) {
            sum += (code(frame, (x, row)) - code(reference, (x, row))).abs();
            count += 1;
        }
    }
    sum as f32 / count as f32
}

/// The most any pixel of `frame` passes `reference`'s whole range, in code values: above its brightest, or
/// below its darkest.
fn overshoot(frame: &Frame, reference: &Frame) -> (i32, i32) {
    let (low, high) = reference
        .pixels
        .iter()
        .fold((255, 0), |(low, high), pixel| (low.min(i32::from(pixel[0])), high.max(i32::from(pixel[0]))));
    frame.pixels.iter().fold((0, 0), |(above, below), pixel| {
        let value = i32::from(pixel[0]);
        (above.max(value - high), below.max(low - value))
    })
}

/// The mean of each row's darkest pixel across the line, and how far it is from `reference`'s, in code values.
fn line_depth(frame: &Frame, reference: &Frame) -> f32 {
    let darkest = |frame: &Frame, row| columns(row, 10).map(|x| code(frame, (x, row))).min().unwrap_or(0);
    let count = rows().count() as f32;
    rows().map(|row| (darkest(frame, row) - darkest(reference, row)).abs() as f32).sum::<f32>() / count
}

/// The two-tone edge's mean 10–90% width in output pixels.
fn edge_width(frame: &Frame) -> f32 {
    let count = rows().count() as f32;
    rows()
        .map(|row| {
            let profile: Vec<f32> = columns(row, 12).map(|x| frame.linear((x, row)).x).collect();
            Edge::of(&profile, 0.05).expect("an edge across the window").width
        })
        .sum::<f32>()
        / count
}

/// At each phone floor, the edge upscaler's line and edge are nearer scale 1's than bilinear's, by every
/// measure: the line's pixels and its darkest, and the edge's width; and nothing of either goes past scale 1's
/// range, beside a line against a wall or the sky, or round the edge.
#[test]
#[ignore = "needs a GPU"]
fn the_edge_upscaler_beats_bilinear_with_no_halo_or_ring() {
    for scene in [Scene::LineOnWall, Scene::LineOnSky, Scene::TwoTone] {
        let mut edge = scene.harness(EDGE);
        let mut bilinear = scene.harness(Upscaler::Bilinear);
        let reference = scene.shot(&mut edge, 1.0);
        let at_one = scene.shot(&mut bilinear, 1.0);
        assert_eq!(reference.pixels, at_one.pixels, "{scene:?}: at scale 1 the two draw the same");
        for (floor, scale) in FLOORS {
            let sharp = scene.shot(&mut edge, scale);
            assert!(edge.renderer.stats().post.upscale, "{scene:?} at {floor}: the upscaler drew");
            let soft = scene.shot(&mut bilinear, scale);
            let (sharp_error, soft_error) = (error(&sharp, &reference), error(&soft, &reference));
            let (above, below) = overshoot(&sharp, &reference);
            let what = format!("{scene:?} at {floor} ({scale})");
            match scene {
                Scene::TwoTone => {
                    let (sharp_width, soft_width, full) =
                        (edge_width(&sharp), edge_width(&soft), edge_width(&reference));
                    eprintln!(
                        "{what}: error {sharp_error:.2} codes against bilinear's {soft_error:.2}; edge \
                         {sharp_width:.2} px against {soft_width:.2} (scale 1: {full:.2}); past scale 1's range \
                         {above} up, {below} down"
                    );
                    assert!(
                        sharp_width < soft_width * 0.9,
                        "{what}: edge {sharp_width} against {soft_width}"
                    );
                }
                _ => {
                    let (sharp_depth, soft_depth) =
                        (line_depth(&sharp, &reference), line_depth(&soft, &reference));
                    eprintln!(
                        "{what}: error {sharp_error:.2} codes against bilinear's {soft_error:.2}; darkest off by \
                         {sharp_depth:.2} against {soft_depth:.2}; past scale 1's range {above} up, {below} down"
                    );
                    assert!(sharp_depth < soft_depth, "{what}: darkest {sharp_depth} against {soft_depth}");
                }
            }
            assert!(sharp_error < soft_error * 0.9, "{what}: error {sharp_error} against {soft_error}");
            // No halo, no ring: nothing brighter than scale 1's brightest, nothing darker than its darkest.
            assert!(above <= 0 && below <= 0, "{what}: {above} codes above, {below} below");
        }
    }
}

/// A flat field comes out flat, to the code value, at every scale from the floors to 1, whatever its level.
#[test]
#[ignore = "needs a GPU"]
fn a_flat_field_stays_flat() {
    for level in [0.002, 0.05, 0.25, 0.8, 0.97] {
        let scene = Scene::Flat(level);
        let mut harness = scene.harness(EDGE);
        let reference = scene.shot(&mut harness, 1.0);
        let value = code(&reference, (0, 0));
        assert!(
            reference.pixels.iter().all(|pixel| i32::from(pixel[0]) == value),
            "{level}: flat at scale 1"
        );
        for scale in [0.378_205, 0.474_359, 0.6, 0.85, 0.99] {
            let frame = scene.shot(&mut harness, scale);
            let off = frame.pixels.iter().map(|pixel| (i32::from(pixel[0]) - value).abs()).max();
            assert_eq!(off, Some(0), "{level} at {scale}: codes off a flat {value}");
        }
    }
}

/// Neighbouring scales as dynamic resolution steps between them differ by no more than either differs from scale
/// 1, so a step is no pop past what drawing at either scale already costs; and the last steps to 1, where the
/// sharpen fades out and the passes stop, move the frame no more than bilinear's do.
#[test]
#[ignore = "needs a GPU"]
fn a_step_in_scale_is_no_pop() {
    let scene = Scene::LineOnWall;
    let mut edge = scene.harness(EDGE);
    let mut bilinear = scene.harness(Upscaler::Bilinear);
    let reference = scene.shot(&mut edge, 1.0);
    // Mobile-High's own scale and its 5% steps down to its floor, and Mobile-Low's ladder's last step.
    let base = 0.632_479;
    let inside = [(base, 0.95 * base), (0.8 * base, 0.75 * base), (0.504_274 * 0.8, 0.504_274 * 0.75)];
    for (from, to) in inside {
        let (a, b) = (scene.shot(&mut edge, from), scene.shot(&mut edge, to));
        let (step, own) = (error(&a, &b), error(&a, &reference).max(error(&b, &reference)));
        let (c, d) = (scene.shot(&mut bilinear, from), scene.shot(&mut bilinear, to));
        eprintln!(
            "{from:.3} to {to:.3}: the edge upscaler moves {step:.2} codes, against {own:.2} from scale 1 (bilinear \
             {:.2})",
            error(&c, &d)
        );
        assert!(step <= own, "{from} to {to}: a step of {step} against {own} from scale 1");
    }
    for from in [0.9, 0.95, 0.99] {
        let differ = |harness: &mut Harness| {
            let (a, b) = (scene.shot(harness, from), scene.shot(harness, 1.0));
            error(&a, &b)
        };
        let (sharp, soft) = (differ(&mut edge), differ(&mut bilinear));
        eprintln!("{from:.3} to 1: the edge upscaler moves {sharp:.2} codes, bilinear {soft:.2}");
        assert!(sharp <= soft, "{from} to 1: {sharp} against bilinear's {soft}");
    }
}

/// The stage sets a frame's render scale after its `prepare`, from the frame's timing: the frame that steps below
/// scale 1 draws the upscaler at the scale it draws at, exactly as a frame whose scale was set before, so no
/// bilinear frame shows at the step; and the step back to 1 draws none.
#[test]
#[ignore = "needs a GPU"]
fn a_scale_set_after_prepare_upscales_its_own_frame() {
    let scene = Scene::LineOnWall;
    let (_, floor) = FLOORS[0];
    let mut before = scene.harness(EDGE);
    let expected = scene.shot(&mut before, floor);
    let mut after = scene.harness(EDGE);
    let at_one = scene.shot(&mut after, 1.0);
    after.scale_after_prepare = Some(floor);
    let stepped = scene.frame(&mut after);
    assert!(after.renderer.stats().post.upscale, "the step's own frame upscaled");
    assert_eq!(after.renderer.stats().passes.upscale.draws, 2);
    assert!(stepped.pixels == expected.pixels, "the step's frame is the floor's, drawn as any other");
    after.scale_after_prepare = Some(1.0);
    let back = scene.frame(&mut after);
    assert!(!after.renderer.stats().post.upscale, "back at 1, nothing upscaled");
    assert_eq!(after.renderer.stats().passes.upscale.draws, 0);
    assert!(back.pixels == at_one.pixels, "back at 1, the frame is scale 1's");
}

/// The upscaler's two passes count exactly in their own row, two fullscreen triangles below scale 1 and nothing
/// at 1, and touch no other pass's counts; its two pipelines build with the renderer, so a frame below scale 1
/// builds nothing, and a tier without it, bilinear or with temporal AA, builds and draws nothing of it.
#[test]
#[ignore = "needs a GPU"]
fn its_passes_count_apart_and_build_with_the_renderer_alone() {
    let scene = Scene::LineOnWall;
    let mut edge = scene.harness(EDGE);
    let mut bilinear = scene.harness(Upscaler::Bilinear);
    scene.shot(&mut edge, 1.0);
    scene.shot(&mut bilinear, 1.0);
    let (with, without) = (edge.renderer.stats(), bilinear.renderer.stats());
    assert_eq!(with.passes.upscale, PassCounts::default(), "nothing at scale 1");
    assert!(!with.post.upscale);
    assert_eq!(with.passes, without.passes, "at scale 1 the frame is bilinear's");
    assert_eq!(with.post.pipelines, without.post.pipelines + 2, "its two pipelines, built with the renderer");
    for (floor, scale) in FLOORS {
        scene.shot(&mut edge, scale);
        scene.shot(&mut bilinear, scale);
        let (with, without) = (edge.renderer.stats(), bilinear.renderer.stats());
        assert_eq!(
            with.passes.upscale,
            PassCounts { draws: 2, instances: 2, triangles: 2, ..PassCounts::default() },
            "{floor}"
        );
        let others = |counts: &FrameCounts| FrameCounts { upscale: PassCounts::default(), ..*counts };
        assert_eq!(others(&with.passes), others(&without.passes), "{floor}: no other pass's counts move");
        assert_eq!(with.post.first_use_builds, 0, "{floor}: nothing built on the frame");
        assert_eq!(with.pipelines.frame_thread_builds, without.pipelines.frame_thread_builds, "{floor}");
    }
    // A desktop tier, its temporal AA upsampling: nothing of the upscaler even if asked for.
    let desktop = TierTable::shipped().get(TierId::DesktopHigh).render_settings();
    let mut asked = Harness::sized(RenderSettings { upscaler: EDGE, ..desktop }, SIZE);
    let mut plain = Harness::sized(desktop, SIZE);
    for harness in [&mut asked, &mut plain] {
        scene.shot(harness, 0.7);
    }
    let (asked, plain) = (asked.renderer.stats(), plain.renderer.stats());
    assert_eq!(asked.post.pipelines, plain.post.pipelines, "a desktop tier builds none of it");
    assert_eq!(asked.passes.upscale, PassCounts::default());
    assert!(!asked.post.upscale);
    // The shipped tiers: the phones upscale along edges, the desktops don't.
    for tier in TierTable::shipped().tiers() {
        let phone = matches!(tier.id, TierId::MobileHigh | TierId::MobileLow);
        assert_eq!(tier.render_settings().upscaler.edge(), phone, "{}", tier.id);
    }
}
