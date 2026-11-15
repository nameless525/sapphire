//! Light shafts from a low sun (M5 L13, `post`'s `shafts` module): off draws nothing and builds nothing; only open
//! sky casts, so a wall between a pixel and the sun leaves it dark while the sky beside it is lit, and neither a
//! glowing lantern nor a bright wall casts; the open sky's ramp steps no more than a code value; a still camera
//! draws the same bytes twice; the shafts fade continuously as the sun leaves the frame and are gone behind the
//! camera; the sun lands on the same pixel at any render scale; bloom's intensity neither scales nor cuts them; and
//! their three pipelines build with the renderer, so the frame a key first asks for them builds nothing. Run with
//! `cargo test -p sapphire_render --test gpu shafts:: -- --ignored --test-threads=1 --nocapture`.
//!
//! The scenes are `vfx`'s: linear values as they are, the camera at the origin looking down -Z over 90° of the
//! frame's height, a dark sky. 4× MSAA with Mobile-High's GTAO, whose prepass depth the shafts read without
//! temporal AA, so a single frame shows them completely; and the desktops' temporal AA where a test says.

use glam::{Mat4, Quat, Vec3};
use sapphire_render::quality::{TierId, TierTable};
use sapphire_render::{BloomParams, LightShafts, Msaa, RenderSettings, SkyGradient};

use super::vfx::{Frame, Harness, passthrough};

const SIZE: (u32, u32) = (256, 256);
/// The sky, dark, so the shafts show.
const SKY: f32 = 0.04;
/// Soft and long, so the open sky's ramp is gentle at this size.
const SHAFTS: LightShafts = LightShafts { strength: 0.3, tint: Vec3::new(1.0, 0.8, 0.6), length: 1.0 };

/// 4× MSAA, five bloom levels and Mobile-High's GTAO, with shafts or not.
fn settings(shafts: bool) -> RenderSettings {
    let mobile_high = TierTable::shipped().get(TierId::MobileHigh).render_settings();
    RenderSettings {
        msaa: Msaa::X4,
        bloom_levels: 5,
        ambient_occlusion: mobile_high.ambient_occlusion,
        shafts,
        ..RenderSettings::default()
    }
}

/// The sun `yaw` degrees right of the view's axis and `pitch` above the horizon.
fn sun(yaw: f32, pitch: f32) -> Vec3 {
    let (yaw, pitch) = (yaw.to_radians(), pitch.to_radians());
    Vec3::new(yaw.sin() * pitch.cos(), pitch.sin(), -yaw.cos() * pitch.cos())
}

/// One frame with the sun `toward`, shafts as `shafts` asks, and bloom at `intensity` with a threshold nothing in
/// these scenes reaches, so bloom itself adds nothing.
fn frame(harness: &mut Harness, toward: Vec3, shafts: LightShafts, intensity: f32) -> Frame {
    harness.render(|packet| {
        let sky = Vec3::splat(SKY);
        packet.sky = SkyGradient { zenith: sky, horizon: sky, ground: sky };
        packet.celestial.sun.direction = -toward;
        packet.post = sapphire_render::PostParams {
            bloom: BloomParams { intensity, threshold: 8.0, knee: 0.0, ..BloomParams::default() },
            shafts,
            ..passthrough()
        };
    })
}

fn harness(shafts: bool) -> Harness {
    Harness::sized(settings(shafts), SIZE)
}

/// A wall `half` metres either side of `centre` (x, y) at depth `z`, glowing `emission`.
fn add_block(harness: &mut Harness, centre: (f32, f32), half: (f32, f32), z: f32, emission: f32) {
    let transform = Mat4::from_scale_rotation_translation(
        Vec3::new(half.0, half.1, 1.0),
        Quat::IDENTITY,
        Vec3::new(centre.0, centre.1, -z),
    );
    harness.add_surface(transform, Vec3::splat(emission));
}

fn code(frame: &Frame, (x, y): (u32, u32)) -> i32 {
    i32::from(frame.pixels[(y * frame.width + x) as usize][0])
}

/// The largest difference in code values between two frames.
fn largest_difference(a: &Frame, b: &Frame) -> i32 {
    a.pixels
        .iter()
        .zip(&b.pixels)
        .map(|(a, b)| {
            (0..3).map(|channel| (i32::from(a[channel]) - i32::from(b[channel])).abs()).max().unwrap_or(0)
        })
        .max()
        .unwrap_or(0)
}

/// The pixel the sun's direction projects into.
fn pixel_of(toward: Vec3) -> (u32, u32) {
    let camera = super::vfx::camera_for(SIZE);
    let clip = (camera.proj * camera.view) * toward.extend(0.0);
    let ndc = clip.truncate() / clip.w;
    (((ndc.x + 1.0) * 0.5 * SIZE.0 as f32) as u32, ((1.0 - ndc.y) * 0.5 * SIZE.1 as f32) as u32)
}

/// Off is nothing: a tier with shafts and a key asking for none, or a key asking on a tier without them, draws the
/// tier without's bytes, records no pass of theirs and builds none of their pipelines without them; a tier with
/// them builds their three with the renderer, so the frame a key first asks builds nothing.
#[test]
#[ignore = "needs a GPU"]
fn off_draws_nothing_and_their_pipelines_build_with_the_renderer() {
    let toward = sun(0.0, 20.0);
    for (path, temporal) in [("MSAA with GTAO", false), ("temporal AA", true)] {
        let with = |shafts| RenderSettings {
            temporal,
            msaa: if temporal { Msaa::Off } else { Msaa::X4 },
            ..settings(shafts)
        };
        let mut base = Harness::sized(with(false), SIZE);
        let mut asked_without = Harness::sized(with(false), SIZE);
        let mut tier_with = Harness::sized(with(true), SIZE);
        let mut expected = frame(&mut base, toward, LightShafts::NONE, 0.2);
        let mut without = frame(&mut asked_without, toward, SHAFTS, 0.2);
        let mut none = frame(&mut tier_with, toward, LightShafts::NONE, 0.2);
        for _ in 0..3 {
            expected = frame(&mut base, toward, LightShafts::NONE, 0.2);
            without = frame(&mut asked_without, toward, SHAFTS, 0.2);
            none = frame(&mut tier_with, toward, LightShafts::NONE, 0.2);
        }
        assert!(without.pixels == expected.pixels, "{path}: a tier without them draws none");
        assert!(none.pixels == expected.pixels, "{path}: a key asking none draws none");
        let (base_stats, with_stats) = (base.renderer.stats(), tier_with.renderer.stats());
        assert!(!asked_without.renderer.stats().post.shafts && !with_stats.post.shafts, "{path}");
        assert_eq!(base_stats.passes, with_stats.passes, "{path}: no pass of theirs");
        assert_eq!(asked_without.renderer.stats().post.pipelines, base_stats.post.pipelines, "{path}");
        assert_eq!(
            with_stats.post.pipelines,
            base_stats.post.pipelines + 3,
            "{path}: their three, at the start"
        );
        // The key's strength rises from zero: drawn, nothing built on the frame.
        let builds = with_stats.pipelines.frame_thread_builds;
        let lit = frame(&mut tier_with, toward, SHAFTS, 0.2);
        let stats = tier_with.renderer.stats();
        assert!(stats.post.shafts, "{path}: drawn");
        assert_eq!(stats.post.first_use_builds, 0, "{path}: nothing built on the frame");
        assert_eq!(stats.pipelines.frame_thread_builds, builds, "{path}");
        assert_eq!(
            stats.passes.post.draws,
            base_stats.passes.post.draws + 2,
            "{path}: the mask and the march"
        );
        assert!(largest_difference(&lit, &expected) > 10, "{path}: they show");
    }
}

/// Behind the camera, or set, the sun casts nothing: no pass, and the bytes of a frame without.
#[test]
#[ignore = "needs a GPU"]
fn a_sun_behind_the_camera_or_set_records_nothing() {
    let mut shafted = harness(true);
    let mut plain = harness(true);
    for toward in [sun(180.0, 20.0), sun(0.0, -5.0)] {
        let expected = frame(&mut plain, toward, LightShafts::NONE, 0.2);
        let drawn = frame(&mut shafted, toward, SHAFTS, 0.2);
        assert!(!shafted.renderer.stats().post.shafts, "{toward}");
        assert!(drawn.pixels == expected.pixels, "{toward}");
    }
}

/// A wall between a pixel and the sun leaves it as it was, while open sky beside the wall is lit; and neither a
/// glowing lantern nor a bright wall in front of the dark sky casts anything.
#[test]
#[ignore = "needs a GPU"]
fn only_open_sky_casts() {
    // The wall covers the frame's right half, the sun behind it.
    let toward = sun(30.0, 15.0);
    let (sun_x, sun_y) = pixel_of(toward);
    assert!(sun_x > SIZE.0 * 5 / 8, "the sun behind the wall");
    let mut shafted = harness(true);
    let mut plain = harness(true);
    for harness in [&mut shafted, &mut plain] {
        add_block(harness, (10.0, 0.0), (10.0, 20.0), 5.0, 0.1);
    }
    let lit = frame(&mut shafted, toward, SHAFTS, 0.2);
    let base = frame(&mut plain, toward, LightShafts::NONE, 0.2);
    // On the wall, a mask texel or more in from its edge, every line to the sun stays on it: exactly nothing.
    for y in (0..SIZE.1).step_by(4) {
        for x in (SIZE.0 / 2 + 12..SIZE.0).step_by(4) {
            assert_eq!(code(&lit, (x, y)), code(&base, (x, y)), "on the wall at ({x}, {y})");
        }
    }
    // The open sky beside it, on the sun's row: lit.
    let beside = (SIZE.0 / 2 - 24, sun_y);
    assert!(
        code(&lit, beside) > code(&base, beside) + 8,
        "{} against {}",
        code(&lit, beside),
        code(&base, beside)
    );

    // A bright wall covering the frame and a lantern glowing far brighter in front of it, the sun behind both.
    let toward = sun(0.0, 15.0);
    let mut shafted = harness(true);
    let mut plain = harness(true);
    for harness in [&mut shafted, &mut plain] {
        add_block(harness, (0.0, 0.0), (40.0, 40.0), 6.0, 0.9);
        add_block(harness, (0.0, 1.0), (0.3, 0.3), 3.0, 6.0);
    }
    let lit = frame(&mut shafted, toward, SHAFTS, 0.2);
    let base = frame(&mut plain, toward, LightShafts::NONE, 0.2);
    assert!(shafted.renderer.stats().post.shafts, "drawn, over no sky");
    assert!(lit.pixels == base.pixels, "neither the wall nor the lantern casts");
}

/// Over open sky the shafts' ramp steps at most a code value between neighbours along a line from the sun, and a
/// still camera draws the same bytes frame after frame.
#[test]
#[ignore = "needs a GPU"]
fn the_open_sky_ramp_is_smooth_and_holds_still() {
    let toward = sun(0.0, 20.0);
    let (sun_x, sun_y) = pixel_of(toward);
    let mut harness = harness(true);
    let first = frame(&mut harness, toward, SHAFTS, 0.2);
    let second = frame(&mut harness, toward, SHAFTS, 0.2);
    assert!(first.pixels == second.pixels, "a still camera draws the same bytes");
    for (dx, dy) in [(1i32, 0i32), (-1, 0), (0, 1), (1, 1)] {
        let (mut x, mut y) = (sun_x as i32, sun_y as i32);
        let mut last = code(&first, (x as u32, y as u32));
        loop {
            (x, y) = (x + dx, y + dy);
            if x < 0 || y < 0 || x >= SIZE.0 as i32 || y >= SIZE.1 as i32 {
                break;
            }
            let value = code(&first, (x as u32, y as u32));
            assert!((value - last).abs() <= 1, "a step of {} at ({x}, {y}) along ({dx}, {dy})", value - last);
            last = value;
        }
    }
    // Brightest at the sun, falling away from it.
    assert!(code(&first, (sun_x, sun_y)) > code(&first, (sun_x, SIZE.1 - 1)) + 20);
}

/// As the sun leaves the frame past its right edge the light at a pixel near that edge falls continuously, a code
/// value or two a step of a quarter degree, to none.
#[test]
#[ignore = "needs a GPU"]
fn the_fade_is_continuous_across_the_margin() {
    let mut harness = harness(true);
    let probe = (SIZE.0 - 4, SIZE.1 / 2);
    let base = code(&frame(&mut harness, sun(0.0, 20.0), LightShafts::NONE, 0.2), probe);
    let mut last: Option<i32> = None;
    let mut yaw = 35.0;
    while yaw < 80.0 {
        let value = code(&frame(&mut harness, sun(yaw, 2.0), SHAFTS, 0.2), probe);
        if let Some(last) = last {
            assert!((value - last).abs() <= 2, "a step of {} at {yaw} degrees", value - last);
        }
        last = Some(value);
        yaw += 0.25;
    }
    assert_eq!(last, Some(base), "gone well outside the frame");
}

/// At render scale 0.7 the shafts' brightest place, the sun, lands where it does at 1; and bloom's intensity
/// neither scales nor cuts them: at two intensities the frame is the same within a code value.
#[test]
#[ignore = "needs a GPU"]
fn neither_the_render_scale_nor_bloom_moves_them() {
    let toward = sun(10.0, 20.0);
    let centroid = |frame: &Frame, base: &Frame| {
        let (mut sum, mut weight) = (glam::Vec2::ZERO, 0.0);
        for y in 0..SIZE.1 {
            for x in 0..SIZE.0 {
                let lift = (code(frame, (x, y)) - code(base, (x, y))).max(0) as f32;
                let lift = (lift - 80.0).max(0.0);
                sum += glam::Vec2::new(x as f32 + 0.5, y as f32 + 0.5) * lift;
                weight += lift;
            }
        }
        sum / weight
    };
    let mut places = Vec::new();
    for scale in [1.0, 0.7] {
        let mut harness = harness(true);
        harness.renderer.set_render_scale(scale);
        let base = frame(&mut harness, toward, LightShafts::NONE, 0.2);
        let lit = frame(&mut harness, toward, SHAFTS, 0.2);
        places.push(centroid(&lit, &base));
    }
    assert!(places[0].distance(places[1]) < 1.0, "{places:?}");
    let sun = pixel_of(toward);
    assert!(places[0].distance(glam::Vec2::new(sun.0 as f32, sun.1 as f32)) < 3.0, "{places:?} {sun:?}");

    let mut harness = harness(true);
    let low = frame(&mut harness, toward, SHAFTS, 0.05);
    let high = frame(&mut harness, toward, SHAFTS, 0.8);
    assert!(largest_difference(&low, &high) <= 1, "{}", largest_difference(&low, &high));
}
