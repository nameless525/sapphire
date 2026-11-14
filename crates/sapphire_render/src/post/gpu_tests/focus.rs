//! Depth of field (M5 L8), rendered and read back: what stays exact, how the blur grows, its edges, its
//! size against the picture's, the tiers and frames that draw none, and effects drawn over it unblurred. They need a GPU:
//! `cargo test -p sapphire_render --lib -- --ignored post::gpu_tests::focus`.
//!
//! Every frame is temporal AA's (the main view's depth of field reads the scene prepass's depth, which only
//! it keeps), a fresh renderer's after the same number of frames, so two frames differ only by post.

use glam::{Quat, Vec2, Vec3};

use super::{Harness, flat, passthrough, srgb_to_linear};
use crate::post::PostStats;
use crate::{
    DepthOfFieldSettings, Focus, FramePacket, Msaa, PostParams, RenderSettings, VfxLight, VfxMesh,
    VfxMeshShape, VfxPattern, VfxShape, VfxSprite,
};

/// Enough frames for temporal AA's history to settle on still edges.
const FRAMES: u32 = 12;

/// Temporal AA, with depth of field as `depth_of_field` says.
fn temporal(depth_of_field: DepthOfFieldSettings) -> RenderSettings {
    RenderSettings { msaa: Msaa::Off, temporal: true, depth_of_field, ..RenderSettings::default() }
}

const DESKTOP: DepthOfFieldSettings = DepthOfFieldSettings { view: true, insets: true };

fn with(focus: Focus) -> PostParams {
    PostParams { focus, ..passthrough() }
}

/// A fresh renderer's last of [`FRAMES`] frames of `scene` at `size` with `post`, and what post drew.
fn frame(
    settings: RenderSettings,
    size: (u32, u32),
    post: PostParams,
    scene: impl Fn(&mut Harness),
) -> (Vec<[u8; 4]>, PostStats) {
    let mut harness = Harness::new(settings, size);
    scene(&mut harness);
    let image = harness.render_frames(FRAMES, post, |_| {});
    (image, harness.renderer.stats().post)
}

fn luminance(texel: [u8; 4]) -> f32 {
    Vec3::new(0.2126, 0.7152, 0.0722).dot(Vec3::from_array([0, 1, 2].map(|c| srgb_to_linear(texel[c]))))
}

/// The pixels within `radius` of `center` (pixel centers), as (x, y, index).
fn disc(size: (u32, u32), center: Vec2, radius: f32) -> impl Iterator<Item = (Vec2, usize)> {
    (0..size.1).flat_map(move |y| (0..size.0).map(move |x| (x, y))).filter_map(move |(x, y)| {
        let at = Vec2::new(x as f32 + 0.5, y as f32 + 0.5);
        (at.distance(center) <= radius).then_some((at, (y * size.0 + x) as usize))
    })
}

/// How far `image`'s light spreads around `center`, within `window` pixels: the luminance-weighted mean
/// squared distance from its centroid, in pixels squared.
fn spread(image: &[[u8; 4]], size: (u32, u32), center: Vec2, window: f32) -> f32 {
    let (mut total, mut sum) = (0.0, Vec2::ZERO);
    for (at, index) in disc(size, center, window) {
        let light = luminance(image[index]);
        total += light;
        sum += at * light;
    }
    let centroid = sum / total;
    disc(size, center, window)
        .map(|(at, index)| luminance(image[index]) * at.distance_squared(centroid))
        .sum::<f32>()
        / total
}

/// The subject: a red character sphere of radius 0.5 at the origin, 4.5 to 5.5 m from the camera.
fn subject(harness: &mut Harness) {
    harness.sphere(Vec3::ZERO, 0.5, flat([1.5, 0.1, 0.1]), true);
}

/// The subject's silhouette on a picture of `size`: its center and radius in pixels.
fn subject_disc(size: (u32, u32)) -> (Vec2, f32) {
    let harness_camera = |point: Vec3| {
        // The harness's camera: at (0, 0, 5) looking at the origin, 0.8 rad of vertical field.
        let focal = size.1 as f32 * 0.5 / 0.4f32.tan();
        let depth = 5.0 - point.z;
        Vec2::new(
            size.0 as f32 * 0.5 + point.x / depth * focal,
            size.1 as f32 * 0.5 - point.y / depth * focal,
        )
    };
    let center = harness_camera(Vec3::ZERO);
    // The silhouette's radius: the sphere seen from 5 m subtends asin(0.5 / 5) either side.
    let radius = (0.1f32.asin().tan()) * size.1 as f32 * 0.5 / 0.4f32.tan();
    (center, radius)
}

/// Sharp on the subject: 5 m, its whole depth within the sharp range, everything from 9.6 m back at the
/// largest blur.
const ON_SUBJECT: Focus = Focus { distance: 5.0, sharp: 0.6, near: 1.0, far: 4.0, blur: 0.015 };

#[test]
#[ignore = "needs a GPU"]
fn the_sharp_range_stays_exact_and_what_lies_behind_blurs() {
    let size = (320, 180);
    let scene = |harness: &mut Harness| {
        subject(harness);
        // A grid of blue and green spheres 20 m away, at the largest blur.
        for (i, x) in (-6..=6).enumerate() {
            for (j, y) in (-3..=3).enumerate() {
                let color = if (i + j) % 2 == 0 { [0.1, 0.8, 0.2] } else { [0.2, 0.3, 0.9] };
                harness.sphere(Vec3::new(x as f32 * 2.4, y as f32 * 2.4, -15.0), 0.9, flat(color), false);
            }
        }
    };
    let (plain, plain_stats) = frame(temporal(DESKTOP), size, passthrough(), scene);
    assert!(!plain_stats.focus, "no key draws no depth of field");
    let (without, _) = frame(temporal(DepthOfFieldSettings::OFF), size, passthrough(), scene);
    assert_eq!(
        plain, without,
        "with no key, a tier with depth of field draws the frame without it, bit for bit"
    );
    assert_eq!(
        frame(temporal(DESKTOP), size, with(Focus { blur: 0.0, ..ON_SUBJECT }), scene).0,
        plain,
        "nor does a key with no blur"
    );

    let (focused, stats) = frame(temporal(DESKTOP), size, with(ON_SUBJECT), scene);
    assert!(stats.focus, "the key draws depth of field");
    let (center, radius) = subject_disc(size);
    let mut subject_pixels = 0;
    for (_, index) in disc(size, center, radius - 1.5) {
        assert_eq!(focused[index], plain[index], "pixel {index}, within the sharp range, is exact");
        subject_pixels += 1;
    }
    assert!(subject_pixels > 500, "{subject_pixels}");
    // The grid behind blurs: its edges soften, so the frame moves around them.
    let behind: Vec<usize> = (0..focused.len())
        .filter(|&index| {
            let at = Vec2::new((index as u32 % size.0) as f32 + 0.5, (index as u32 / size.0) as f32 + 0.5);
            at.distance(center) > radius + 3.0
        })
        .collect();
    let moved = behind.iter().filter(|&&index| focused[index] != plain[index]).count();
    let step = behind
        .iter()
        .map(|&index| {
            (0..3).map(|c| f64::from(focused[index][c].abs_diff(plain[index][c]))).sum::<f64>() / 3.0
        })
        .sum::<f64>()
        / behind.len() as f64;
    assert!(
        moved * 2 > behind.len() && step > 2.0,
        "the grid blurs: {moved} of {} moved, {step:.2}",
        behind.len()
    );
    eprintln!(
        "{subject_pixels} subject pixels exact; {moved} of {} behind moved, by {step:.2} steps on average",
        behind.len()
    );
}

/// The spread around each of six white spheres, each the same size on the picture, at 5 (in focus), 8, 9.5,
/// 11.25 (the largest blur from there), 20 and 40 m, at `size`: without depth of field and with it.
fn spreads(size: (u32, u32), focus: Focus) -> [Vec<f32>; 2] {
    const DEPTHS: [f32; 6] = [5.0, 8.0, 9.5, 11.25, 20.0, 40.0];
    let window = 0.125 * size.1 as f32;
    let centers: Vec<Vec2> =
        (0..DEPTHS.len()).map(|i| Vec2::new((i as f32 + 0.5) / DEPTHS.len() as f32, 0.5)).collect();
    let scene = |harness: &mut Harness| {
        for (uv, depth) in centers.iter().zip(DEPTHS) {
            let toward = harness.direction(*uv);
            let along = depth / toward.dot(Vec3::NEG_Z);
            harness.sphere(harness.camera.position + toward * along, 0.02 * depth, flat([1.0; 3]), false);
        }
    };
    [passthrough(), with(focus)].map(|post| {
        let (image, _) = frame(temporal(DESKTOP), size, post, scene);
        let pixels = Vec2::new(size.0 as f32, size.1 as f32);
        centers.iter().map(|uv| spread(&image, size, *uv * pixels, window)).collect()
    })
}

/// Sharp at 5 m and 25 cm either side, the largest blur from 11.25 m back.
const GROWING: Focus = Focus { distance: 5.0, sharp: 0.25, near: 1.0, far: 6.0, blur: 0.02 };

#[test]
#[ignore = "needs a GPU"]
fn blur_grows_with_distance_up_to_the_largest() {
    let size = (640, 360);
    let [plain, focused] = spreads(size, GROWING);
    let added: Vec<f32> = plain.iter().zip(&focused).map(|(plain, focused)| focused - plain).collect();
    eprintln!("spread without {plain:.2?}, with {focused:.2?}, added {added:.2?} (px²)");
    // Without depth of field each sphere spreads about alike: they're the same size on the picture, but for
    // perspective stretching those toward the sides.
    let mean = plain.iter().sum::<f32>() / plain.len() as f32;
    assert!(plain.iter().all(|spread| (spread / mean - 1.0).abs() < 0.15), "{plain:?}");
    assert!(added[0].abs() < 0.03 * mean, "in focus, nothing added: {added:?}");
    // A disc of radius r adds r²/2: at 8 m about 3.3 px (5.4 px²), at 9.5 m 5.1 px (13 px²).
    for pair in added[..4].windows(2) {
        assert!(pair[1] > pair[0] + 2.0, "blur grows with distance: {added:?}");
    }
    // From 11.25 m back every sphere takes the largest blur.
    let largest = added[3];
    assert!(
        added[4..].iter().all(|added| (added / largest - 1.0).abs() < 0.1),
        "it stops at the largest: {added:?}"
    );
    // A disc of radius r adds r²/2: the largest blur's radius is 2% of 360 rows, 7.2 px.
    let radius = (2.0 * largest).sqrt();
    assert!(radius > 0.5 * 7.2 && radius < 1.2 * 7.2, "the largest blur's radius: {radius:.2} px");
}

#[test]
#[ignore = "needs a GPU"]
fn the_same_key_blurs_the_same_fraction_of_any_size() {
    let fractions: Vec<f32> = [(640, 360), (1280, 720)]
        .into_iter()
        .map(|size| {
            let [plain, focused] = spreads(size, GROWING);
            // The farthest sphere's added spread, over the picture's height squared.
            (focused[5] - plain[5]) / (size.1 as f32).powi(2)
        })
        .collect();
    eprintln!("the largest blur's added spread over the height squared: {fractions:?}");
    assert!((fractions[0] / fractions[1] - 1.0).abs() < 0.2, "{fractions:?}");
}

#[test]
#[ignore = "needs a GPU"]
fn a_sharp_edge_against_a_far_background_leaks_neither_way() {
    let size = (320, 180);
    let scene = |harness: &mut Harness| {
        subject(harness);
        // A green wall 15 m away and more, filling the view: a sphere seen from outside.
        harness.sphere(Vec3::new(0.0, 0.0, -40.0), 30.0, flat([0.05, 0.6, 0.1]), false);
    };
    let (plain, _) = frame(temporal(DESKTOP), size, passthrough(), scene);
    let (focused, stats) = frame(temporal(DESKTOP), size, with(ON_SUBJECT), scene);
    assert!(stats.focus);
    let (center, radius) = subject_disc(size);
    // The wall doesn't spread over the subject: within its silhouette, every pixel is exact.
    for (_, index) in disc(size, center, radius - 1.5) {
        assert_eq!(focused[index], plain[index], "pixel {index}");
    }
    // The subject doesn't spread into the wall: from two pixels out, no more red than without.
    let (mut worst, mut out) = (0.0f32, 0.0);
    for (at, index) in disc(size, center, radius + 14.0) {
        let red = |texel: [u8; 4]| srgb_to_linear(texel[0]);
        let added = red(focused[index]) - red(plain[index]);
        if at.distance(center) >= radius + 2.0 && added > worst {
            (worst, out) = (added, at.distance(center) - radius);
        }
    }
    assert!(worst < 0.01, "red leaked into the wall: {worst}, {out:.1} px out");
    eprintln!("red added outside the subject at most {worst:.4} (linear), {out:.1} px out");
}

#[test]
#[ignore = "needs a GPU"]
fn a_tier_without_it_draws_and_builds_nothing_for_a_key() {
    let size = (160, 90);
    let scene = |harness: &mut Harness| {
        subject(harness);
        harness.sphere(Vec3::new(1.5, 0.5, -10.0), 1.0, flat([0.1, 0.6, 0.9]), false);
    };
    let tiers = [
        ("temporal AA, no depth of field", temporal(DepthOfFieldSettings::OFF)),
        ("soft inset backdrops only", temporal(DepthOfFieldSettings { view: false, insets: true })),
        // A desktop tier pinned to MSAA, as the older goldens run it: no prepass depth, so no focus.
        (
            "a desktop tier with 4x MSAA",
            RenderSettings { msaa: Msaa::X4, depth_of_field: DESKTOP, ..RenderSettings::default() },
        ),
    ];
    for (name, settings) in tiers {
        let (plain, plain_stats) = frame(settings, size, passthrough(), scene);
        let (keyed, stats) = frame(settings, size, with(ON_SUBJECT), scene);
        assert_eq!(keyed, plain, "{name}: a key changes nothing");
        assert!(!stats.focus, "{name}: draws no depth of field");
        assert_eq!(stats.pipelines, plain_stats.pipelines, "{name}: builds nothing for it");
        assert_eq!(stats.first_use_builds, plain_stats.first_use_builds, "{name}");
    }
}

/// Where `point` lands on a picture of `size` from the harness's camera (at (0, 0, 5), looking at the origin,
/// 0.8 rad of vertical field), in pixels.
fn on_screen(size: (u32, u32), point: Vec3) -> Vec2 {
    let focal = size.1 as f32 * 0.5 / 0.4f32.tan();
    let depth = 5.0 - point.z;
    Vec2::new(size.0 as f32 * 0.5 + point.x / depth * focal, size.1 as f32 * 0.5 - point.y / depth * focal)
}

/// Effects draw after depth of field's passes: an effect in front of the sharp subject that crosses its edge
/// onto the blurred grid behind is drawn exactly as it is without focus, sharp on both sides and never
/// blurred; what no effect covers is the focused frame bit for bit; and a translucent effect over the blurred
/// grid composites over the blurred frame, not the sharp one. The effects are an opaque bar (one of L5's
/// effect meshes, a card) across the subject's right edge, an opaque chevron sprite across its left, and a
/// half-transparent card over the grid alone. Without temporal AA, as under MSAA, effects draw in the main
/// pass and the main view has no focus at all, so nothing here applies.
#[test]
#[ignore = "needs a GPU"]
fn effects_draw_over_the_blurred_frame_and_stay_sharp_across_the_subjects_edge() {
    let size = (320, 180);
    let scene = |harness: &mut Harness| {
        subject(harness);
        for (i, x) in (-6..=6).enumerate() {
            for (j, y) in (-3..=3).enumerate() {
                let color = if (i + j) % 2 == 0 { [0.1, 0.8, 0.2] } else { [0.2, 0.3, 0.9] };
                harness.sphere(Vec3::new(x as f32 * 2.4, y as f32 * 2.4, -15.0), 0.9, flat(color), false);
            }
        }
    };
    // In front of the subject, 4.45 m from the camera: inside the sharp range.
    const Z: f32 = 0.55;
    let card = |position: Vec3, scale: Vec3, color: Vec3, alpha: f32| VfxMesh {
        shape: VfxMeshShape::Card,
        position,
        rotation: Quat::IDENTITY,
        scale,
        color,
        alpha,
        additive: 0.0,
        soft: 1e-3,
        pattern: VfxPattern::Solid,
        tiling: Vec2::ONE,
        scroll: Vec2::ZERO,
        erosion: None,
        flipbook: None,
    };
    let bar = card(Vec3::new(0.6, 0.0, Z), Vec3::new(1.0, 0.2, 1.0), Vec3::new(0.9, 0.8, 0.05), 1.0);
    let veil = (Vec3::new(-1.7, 0.85, Z), Vec3::new(0.9, 0.5, 1.0));
    let translucent = card(veil.0, veil.1, Vec3::new(0.6, 0.1, 0.7), 0.5);
    let chevron = VfxSprite {
        position: Vec3::new(-0.5, -0.05, Z),
        axis: Vec3::ZERO,
        rotation: 0.0,
        half_width: 0.18,
        half_length: 0.18,
        color: Vec3::new(0.05, 0.9, 0.9),
        alpha: 1.0,
        additive: 0.0,
        soft: 1e-3,
        shape: VfxShape::Chevron,
        light: VfxLight::Emissive,
    };
    let effects = |packet: &mut FramePacket| {
        packet.vfx.meshes.push(bar);
        packet.vfx.meshes.push(translucent);
        packet.vfx.sprites.push(chevron);
    };
    let render = |post: PostParams, fill: &dyn Fn(&mut FramePacket)| {
        let mut harness = Harness::new(temporal(DESKTOP), size);
        scene(&mut harness);
        harness.render_frames(FRAMES, post, fill)
    };
    let plain = render(passthrough(), &|_| {});
    let plain_effects = render(passthrough(), &effects);
    let focused = render(with(ON_SUBJECT), &|_| {});
    let focused_effects = render(with(ON_SUBJECT), &effects);

    let (center, radius) = subject_disc(size);
    let (width, height) = (size.0 as i32, size.1 as i32);
    let index = |x: i32, y: i32| (y.clamp(0, height - 1) * width + x.clamp(0, width - 1)) as usize;
    // The half-transparent card's place on the picture: its corners, in pixels.
    let corner =
        |sign: f32| on_screen(size, veil.0 + Vec3::new(sign * veil.1.x, -sign * veil.1.y, 0.0) * 0.5);
    let (top_left, bottom_right) = (corner(-1.0), corner(1.0));
    let under_veil = |x: i32, y: i32, margin: f32| {
        let at = Vec2::new(x as f32 + 0.5, y as f32 + 0.5);
        at.cmpge(top_left - margin).all() && at.cmple(bottom_right + margin).all()
    };
    // An opaque effect's own pixels: covered (the effect changed them), with all eight neighbours covered the
    // same, so the effect's flat color fills them whatever is behind; the half-transparent card's aren't.
    let (mut over_subject, mut over_grid, mut untouched, mut blurred) = (0, 0, 0, 0);
    for y in 0..height {
        for x in 0..width {
            let at = index(x, y);
            // No effect's: nothing within two pixels changed, so not even an antialiased edge's last trace (too
            // faint to move a code over one backdrop, it can move one over another) reaches it.
            let clear = (-2..=2).all(|dy| {
                (-2..=2).all(|dx| plain_effects[index(x + dx, y + dy)] == plain[index(x + dx, y + dy)])
            });
            if clear {
                assert_eq!(
                    focused_effects[at], focused[at],
                    "({x}, {y}), no effect's, is the focused frame's"
                );
                untouched += 1;
                blurred += usize::from(focused[at] != plain[at]);
                continue;
            }
            if plain_effects[at] == plain[at] {
                continue;
            }
            let flat = (-1..=1).all(|dy| {
                (-1..=1).all(|dx| {
                    let near = index(x + dx, y + dy);
                    plain_effects[near] != plain[near] && plain_effects[near] == plain_effects[at]
                })
            });
            if !flat || under_veil(x, y, 3.0) {
                continue;
            }
            assert_eq!(
                focused_effects[at], plain_effects[at],
                "({x}, {y}), an opaque effect's, is unblurred"
            );
            let from_center = Vec2::new(x as f32 + 0.5, y as f32 + 0.5).distance(center);
            over_subject += usize::from(from_center < radius - 2.0);
            over_grid += usize::from(from_center > radius + 2.0);
        }
    }
    assert!(
        over_subject > 40 && over_grid > 40,
        "opaque effect pixels: {over_subject} on the subject, {over_grid} off it"
    );
    assert!(blurred * 3 > untouched, "the grid still blurs: {blurred} of {untouched} untouched pixels moved");

    // The half-transparent card, two pixels inside its edges: half its color over what's behind. Over the
    // focused frame, its change from the unfocused frame is half the blur's change.
    let (low, high) = (top_left + 2.0, bottom_right - 2.0);
    let linear = |texel: [u8; 4]| Vec3::from_array([0, 1, 2].map(|c| srgb_to_linear(texel[c])));
    let (mut error, mut blur, mut count) = (0.0, 0.0, 0);
    for y in low.y as i32..=high.y as i32 {
        for x in low.x as i32..=high.x as i32 {
            let at = index(x, y);
            let expected = (linear(focused[at]) - linear(plain[at])) * 0.5;
            let seen = linear(focused_effects[at]) - linear(plain_effects[at]);
            error += (seen - expected).abs().max_element();
            blur += expected.abs().max_element();
            count += 1;
        }
    }
    let (error, blur) = (error / count as f32, blur / count as f32);
    assert!(count > 200, "{count} pixels under the half-transparent card");
    assert!(blur > 0.004, "the grid under the card blurs: {blur}");
    assert!(
        error < blur * 0.25 && error < 0.004,
        "it composites over the blurred frame: off by {error}, against a blur of {blur}"
    );
    eprintln!(
        "opaque effect pixels unblurred: {over_subject} on the subject, {over_grid} on the grid; {blurred} of \
         {untouched} untouched pixels blurred, each the focused frame's; under the half-transparent card \
         ({count} px) the change is half the blur's ({blur:.4}) to within {error:.4}"
    );
}
