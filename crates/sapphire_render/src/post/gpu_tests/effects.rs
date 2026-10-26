//! The screen effects (M1 P3), rendered and read back: each effect's output on a known frame, the
//! character mask, and an empty stack drawing exactly the frame from before effects. They need a
//! GPU: `cargo test -p sapphire_render --lib -- --ignored post::gpu_tests::effects`.

use glam::{Vec2, Vec3};

use super::{Harness, composited, flat, linear_to_srgb, max_step, mean_step, passthrough, srgb_to_linear};
use crate::quality::{TierId, TierTable};
use crate::{
    ChromaticAberration, Duotone, EffectCoverage, Inversion, Msaa, PostEffectSettings, PostEffects,
    PostParams, RadialBlur, RenderSettings, SkyGradient, SpeedLines,
};

fn grey_sky(value: f32) -> SkyGradient {
    SkyGradient { zenith: Vec3::splat(value), horizon: Vec3::splat(value), ground: Vec3::splat(value) }
}

fn with(effects: PostEffects) -> PostParams {
    PostParams { effects, ..passthrough() }
}

fn texel(image: &[[u8; 4]], size: (u32, u32), x: u32, y: u32) -> [u8; 4] {
    image[(y * size.0 + x) as usize]
}

/// Every channel of every pixel of `image` within `steps` of `expected`'s.
fn each_within(image: &[[u8; 4]], expected: impl Fn(usize, usize) -> f32, steps: f32) -> Result<(), String> {
    for (index, texel) in image.iter().enumerate() {
        for (channel, &value) in texel[..3].iter().enumerate() {
            let want = expected(index, channel);
            if (f32::from(value) - want).abs() > steps {
                return Err(format!("pixel {index} channel {channel}: {value} vs {want}"));
            }
        }
    }
    Ok(())
}

#[test]
#[ignore = "needs a GPU"]
fn no_effects_draw_exactly_the_frame_from_before_effects() {
    let size = (160, 90);
    let mut harness = Harness::new(RenderSettings::default(), size);
    harness.sky = grey_sky(0.4);
    harness.sphere(Vec3::ZERO, 0.6, flat([1.5, 0.8, 0.4]), true);
    let silent = PostEffects {
        radial_blur: RadialBlur { center: Vec2::new(0.2, 0.3), strength: 0.0 },
        chromatic_aberration: ChromaticAberration { strength: 0.0, ..ChromaticAberration::NONE },
        inversion: Inversion { amount: 0.0, coverage: EffectCoverage::WORLD },
        duotone: Duotone { amount: 0.0, ..Duotone::NONE },
        speed_lines: SpeedLines { opacity: 0.0, ..SpeedLines::NONE },
    };
    let inverted =
        PostEffects { inversion: Inversion { amount: 1.0, ..Inversion::NONE }, ..PostEffects::NONE };
    for params in [passthrough(), composited()] {
        let plain = harness.render(params);
        assert!(!harness.renderer.stats().post.effects.any());
        assert_eq!(
            harness.render(PostParams { effects: silent, ..params }),
            plain,
            "effects at 0 draw nothing"
        );
        assert!(!harness.renderer.stats().post.effects.any(), "and pick no effect variant");
        let flash = harness.render(PostParams { effects: inverted, ..params });
        assert!(harness.renderer.stats().post.effects.inversion);
        assert!(max_step(&flash, &plain) > 100);
        assert_eq!(harness.render(params), plain, "nothing lingers after an effect");
    }
}

#[test]
#[ignore = "needs a GPU"]
fn inversion_and_the_duotone_map_the_display_values() {
    let size = (64, 36);
    let mut harness = Harness::new(RenderSettings::default(), size);
    harness.sky = grey_sky(0.3);
    let plain = harness.render(passthrough());
    let base = |index: usize, channel: usize| f32::from(plain[index][channel]);
    let inversion =
        |amount| PostEffects { inversion: Inversion { amount, ..Inversion::NONE }, ..PostEffects::NONE };
    let inverted = harness.render(with(inversion(1.0)));
    each_within(&inverted, |index, channel| 255.0 - base(index, channel), 1.0).expect("1 − c");
    let half = harness.render(with(inversion(0.5)));
    each_within(&half, |_, _| 127.5, 1.0).expect("halfway to the negative is mid grey");

    // A grey of display value ~0.58 against thresholds either side, and a smooth gradient map.
    let (dark, light) = (Vec3::new(0.2, 0.0, 0.0), Vec3::new(0.0, 0.0, 1.0));
    let duotone = |threshold, softness| PostEffects {
        duotone: Duotone {
            amount: 1.0,
            dark,
            light,
            threshold,
            softness,
            coverage: EffectCoverage::EVERYWHERE,
        },
        ..PostEffects::NONE
    };
    let encode = |color: Vec3| color.to_array().map(|c| linear_to_srgb(c) * 255.0);
    let (dark_display, light_display) = (encode(dark), encode(light));
    let light_side = harness.render(with(duotone(0.4, 0.01)));
    each_within(&light_side, |_, channel| light_display[channel], 1.0).expect("brighter than 0.4: light");
    let dark_side = harness.render(with(duotone(0.8, 0.01)));
    each_within(&dark_side, |_, channel| dark_display[channel], 1.0).expect("darker than 0.8: dark");
    let mapped = harness.render(with(duotone(0.5, 1.0)));
    let grey = base(0, 1) / 255.0;
    let ramp = grey * grey * (3.0 - 2.0 * grey);
    each_within(
        &mapped,
        |_, channel| dark_display[channel] + (light_display[channel] - dark_display[channel]) * ramp,
        1.5,
    )
    .expect("a gradient map along the luminance");
    assert!(
        harness.renderer.stats().post.effects.duotone
            && !harness.renderer.stats().post.effects.character_mask
    );
}

#[test]
#[ignore = "needs a GPU"]
fn the_character_mask_keeps_characters_colored_to_within_a_pixel_with_and_without_temporal_aa() {
    let size = (320, 180);
    let paths = [
        ("4x MSAA", RenderSettings::default(), 1),
        ("TAA", RenderSettings { msaa: Msaa::Off, temporal: true, ..RenderSettings::default() }, 24),
    ];
    for (path, settings, frames) in paths {
        let mut harness = Harness::new(settings, size);
        harness.sky = grey_sky(0.3);
        // A character on the left and the world on the right, alike.
        harness.sphere(Vec3::new(-1.0, 0.0, 0.0), 0.5, flat([0.8, 0.3, 0.1]), true);
        harness.sphere(Vec3::new(1.0, 0.0, 0.0), 0.5, flat([0.8, 0.3, 0.1]), false);
        let (left, right) = (harness.project(Vec3::new(-1.0, 0.0, 0.0)), harness.project(Vec3::X));
        let radius = harness.project(Vec3::new(-0.5, 0.0, 0.0)).x - left.x;
        let plain = harness.render_frames(frames, passthrough(), |_| {});
        let duotone = |coverage| PostEffects {
            duotone: Duotone {
                amount: 1.0,
                dark: Vec3::ZERO,
                light: Vec3::ONE,
                threshold: 0.5,
                softness: 0.01,
                coverage,
            },
            ..PostEffects::NONE
        };
        let world_only = harness.render_frames(frames, with(duotone(EffectCoverage::WORLD)), |_| {});
        assert!(harness.renderer.stats().post.effects.character_mask, "{path}");
        let at = |image: &[[u8; 4]], point: Vec2| texel(image, size, point.x as u32, point.y as u32);
        let close = |a: [u8; 4], b: [u8; 4]| (0..3).all(|c| a[c].abs_diff(b[c]) <= 1);
        assert!(close(at(&world_only, left), at(&plain, left)), "{path}: the character keeps its colors");
        assert_eq!(at(&world_only, right)[..3], [255; 3], "{path}: the world's sphere goes light");
        assert_eq!(at(&world_only, Vec2::new(5.0, 5.0))[..3], [255; 3], "{path}: so does the sky");
        let characters_only =
            harness.render_frames(frames, with(duotone(EffectCoverage::CHARACTERS)), |_| {});
        assert_eq!(at(&characters_only, left)[..3], [255; 3], "{path}");
        assert!(close(at(&characters_only, right), at(&plain, right)), "{path}");

        // Along the middle row through the character: kept pixels well outside its silhouette, or
        // mapped ones well inside it, would be the mask out of place.
        let y = left.y as u32;
        let (mut misplaced, mut partial) = (0, 0);
        for x in 0..size.0 / 2 {
            let distance = (x as f32 + 0.5 - left.x).abs() - radius;
            let (kept, mapped) = (texel(&world_only, size, x, y), texel(&plain, size, x, y));
            let is_kept = close(kept, mapped);
            let is_mapped = kept[..3] == [255; 3] || kept[..3] == [0; 3];
            if (is_kept && distance > 1.0) || (is_mapped && !is_kept && distance < -1.0) {
                misplaced += 1;
            }
            partial += usize::from(!is_kept && !is_mapped);
        }
        eprintln!(
            "{path}: {misplaced} pixels of the mask misplaced by over a pixel, {partial} partial along a row"
        );
        assert_eq!(misplaced, 0, "{path}");
        assert!(partial <= 4, "{path}: {partial} partly masked pixels across two edges");
    }
}

/// The light-weighted centroid of `channel` within `reach` pixels of `around`.
fn channel_centroid(image: &[[u8; 4]], size: (u32, u32), channel: usize, around: Vec2, reach: f32) -> Vec2 {
    let (mut sum, mut weight) = (Vec2::ZERO, 0.0);
    for (index, texel) in image.iter().enumerate() {
        let pixel = Vec2::new((index as u32 % size.0) as f32 + 0.5, (index as u32 / size.0) as f32 + 0.5);
        let light = srgb_to_linear(texel[channel]);
        if pixel.distance(around) < reach {
            sum += pixel * light;
            weight += light;
        }
    }
    sum / weight.max(1e-9)
}

#[test]
#[ignore = "needs a GPU"]
fn chromatic_aberration_splits_red_and_blue_radially() {
    let size = (320, 180);
    let mut harness = Harness::new(RenderSettings::default(), size);
    harness.sphere(Vec3::new(1.4, 0.3, 0.0), 0.25, flat([0.9; 3]), false);
    harness.sphere(Vec3::ZERO, 0.25, flat([0.9; 3]), false);
    let (side, middle) = (harness.project(Vec3::new(1.4, 0.3, 0.0)), harness.project(Vec3::ZERO));
    let plain = harness.render(passthrough());
    let strength = 0.2;
    let split = PostEffects {
        chromatic_aberration: ChromaticAberration { center: Vec2::splat(0.5), strength },
        ..PostEffects::NONE
    };
    let image = harness.render(with(split));
    let green_of = |image: &[[u8; 4]]| image.iter().map(|texel| texel[1]).collect::<Vec<_>>();
    assert_eq!(green_of(&image), green_of(&plain), "green is the frame's own");
    let reach = 40.0;
    let [red, green, blue] = [0, 1, 2].map(|channel| channel_centroid(&image, size, channel, side, reach));
    // Red is read nearer the center by the split, so its image lies farther out; blue nearer in.
    let frame = Vec2::new(size.0 as f32, size.1 as f32);
    let offset = green - frame * 0.5;
    let aspect = frame.x / frame.y;
    let half_diagonal = 0.5 * (aspect * aspect + 1.0).sqrt();
    let s = strength * (offset / frame.y).length() / half_diagonal;
    let (red_shift, blue_shift) =
        ((red - green).dot(offset.normalize()), (blue - green).dot(offset.normalize()));
    let expected = (offset.length() * s / (1.0 - s), -offset.length() * s / (1.0 + s));
    eprintln!(
        "red {red_shift:.2} px (expected {:.2}), blue {blue_shift:.2} px (expected {:.2})",
        expected.0, expected.1
    );
    assert!((red_shift - expected.0).abs() < 1.0 && (blue_shift - expected.1).abs() < 1.0);
    // The middle of the frame stays clean.
    let [red, green, blue] = [0, 1, 2].map(|channel| channel_centroid(&image, size, channel, middle, reach));
    assert!(red.distance(green) < 0.2 && blue.distance(green) < 0.2, "{red} {green} {blue}");
    assert!(harness.renderer.stats().post.effects.chromatic_aberration);
}

/// The variances of the light around `around` along `direction` and across it.
fn spread(image: &[[u8; 4]], size: (u32, u32), around: Vec2, reach: f32, direction: Vec2) -> (f32, f32) {
    let center = channel_centroid(image, size, 1, around, reach);
    let across = direction.perp();
    let (mut along_sum, mut across_sum, mut weight) = (0.0, 0.0, 0.0);
    for (index, texel) in image.iter().enumerate() {
        let pixel = Vec2::new((index as u32 % size.0) as f32 + 0.5, (index as u32 / size.0) as f32 + 0.5);
        let light = srgb_to_linear(texel[1]);
        if pixel.distance(around) < reach {
            let offset = pixel - center;
            along_sum += offset.dot(direction).powi(2) * light;
            across_sum += offset.dot(across).powi(2) * light;
            weight += light;
        }
    }
    (along_sum / weight, across_sum / weight)
}

#[test]
#[ignore = "needs a GPU"]
fn the_radial_blur_streaks_outward_from_its_center() {
    let size = (320, 180);
    let blurred = |taps: u32| {
        let settings = RenderSettings {
            post_effects: PostEffectSettings { radial_blur_taps: taps },
            ..RenderSettings::default()
        };
        let mut harness = Harness::new(settings, size);
        harness.sphere(Vec3::new(1.3, 0.5, 0.0), 0.15, flat([0.9; 3]), false);
        let sphere = harness.project(Vec3::new(1.3, 0.5, 0.0));
        let plain = harness.render(passthrough());
        let blur = PostEffects {
            radial_blur: RadialBlur { center: Vec2::splat(0.5), strength: 0.3 },
            ..PostEffects::NONE
        };
        let image = harness.render(with(blur));
        assert!(harness.renderer.stats().post.effects.radial_blur);
        (plain, image, sphere)
    };
    let (plain, image, sphere) = blurred(12);
    let frame = Vec2::new(size.0 as f32, size.1 as f32);
    let outward = (sphere - frame * 0.5).normalize();
    let reach = 70.0;
    let (along, across) = spread(&image, size, sphere, reach, outward);
    let (plain_along, plain_across) = spread(&plain, size, sphere, reach, outward);
    eprintln!(
        "variance along the ray {along:.1} px² (was {plain_along:.1}), across {across:.1} (was {plain_across:.1})"
    );
    assert!(along / across > 1.8 * plain_along / plain_across, "streaked along the ray, not across it");
    // The streak runs outward: the light's centroid moves away from the center.
    let before = channel_centroid(&plain, size, 1, sphere, reach) - frame * 0.5;
    let after = channel_centroid(&image, size, 1, sphere, reach) - frame * 0.5;
    let ratio = after.length() / before.length();
    assert!(ratio > 1.08 && ratio < 1.3 && after.normalize().dot(outward) > 0.999, "{ratio}");
    // Fewer taps draw the same blur with coarser grain.
    let (_, coarse, _) = blurred(4);
    let difference = mean_step(&coarse, &image);
    eprintln!("4 taps against 12: {difference:.3} mean steps apart");
    assert!(difference < 1.0, "{difference}");
}

#[test]
#[ignore = "needs a GPU"]
fn speed_lines_radiate_outside_their_clear_radius() {
    let size = (640, 360);
    let mut harness = Harness::new(RenderSettings::default(), size);
    harness.sky = grey_sky(0.5);
    let plain = harness.render(passthrough());
    let lines = |seed, coverage| PostEffects {
        speed_lines: SpeedLines {
            opacity: 1.0,
            center: Vec2::splat(0.5),
            count: 48,
            width: 0.5,
            inner_radius: 0.05,
            spread: 0.05,
            color: Vec3::ZERO,
            seed,
            coverage,
        },
        ..PostEffects::NONE
    };
    let image = harness.render(with(lines(7, EffectCoverage::EVERYWHERE)));
    assert!(harness.renderer.stats().post.effects.speed_lines);
    let middle = Vec2::new(size.0 as f32, size.1 as f32) * 0.5;
    let height = size.1 as f32;
    // Nothing inside the clear radius.
    for (index, (texel, plain)) in image.iter().zip(&plain).enumerate() {
        let pixel = Vec2::new((index as u32 % size.0) as f32 + 0.5, (index as u32 / size.0) as f32 + 0.5);
        if pixel.distance(middle) < 0.05 * height {
            assert_eq!(texel, plain, "clear at {pixel}");
        }
    }
    // Around a ring past every tip, one dark run per line.
    let ring = |image: &[[u8; 4]]| -> Vec<u8> {
        (0..4096)
            .map(|step| {
                let angle = step as f32 / 4096.0 * std::f32::consts::TAU;
                let pixel = middle + Vec2::new(angle.cos(), angle.sin()) * 0.45 * height;
                texel(image, size, pixel.x as u32, pixel.y as u32)[1]
            })
            .collect()
    };
    let grey = plain[0][1];
    let samples = ring(&image);
    let dark: Vec<bool> = samples.iter().map(|&value| value + 40 < grey).collect();
    let runs = (0..dark.len()).filter(|&i| dark[i] && !dark[(i + dark.len() - 1) % dark.len()]).count();
    assert_eq!(runs, 48, "one line per slot");
    assert_eq!(samples.iter().min(), Some(&0), "full black at a line's middle");
    // The seed redraws them.
    assert_eq!(
        harness.render(with(lines(7, EffectCoverage::EVERYWHERE))),
        image,
        "the same seed, the same lines"
    );
    let other = harness.render(with(lines(8, EffectCoverage::EVERYWHERE)));
    assert!(max_step(&other, &image) > 100, "another seed, other lines");

    // Behind a character: none on it.
    harness.sphere(Vec3::ZERO, 1.2, flat([0.6, 0.4, 0.3]), true);
    let without = harness.render(passthrough());
    let behind = harness.render(with(lines(7, EffectCoverage::WORLD)));
    let radius = harness.project(Vec3::new(1.2, 0.0, 0.0)).x - middle.x;
    let mut on_character = 0;
    for (index, (texel, plain)) in behind.iter().zip(&without).enumerate() {
        let pixel = Vec2::new((index as u32 % size.0) as f32 + 0.5, (index as u32 / size.0) as f32 + 0.5);
        if pixel.distance(middle) < radius - 2.0 && (0..3).any(|c| texel[c].abs_diff(plain[c]) > 1) {
            on_character += 1;
        }
    }
    assert_eq!(on_character, 0, "no line crosses the character");
    assert!(ring(&behind).iter().any(|&value| value + 40 < grey), "around it, the lines");
}

#[test]
#[ignore = "needs a GPU"]
fn mobile_lows_effects_draw_as_desktop_lows_do() {
    // As P2's test of the compositing layers: each tier at its own scale, MSAA, the same scene.
    let size = (320, 180);
    let table = TierTable::shipped();
    let impact = PostEffects {
        radial_blur: RadialBlur { center: Vec2::new(0.45, 0.5), strength: 0.08 },
        chromatic_aberration: ChromaticAberration { center: Vec2::splat(0.5), strength: 0.04 },
        duotone: Duotone {
            amount: 1.0,
            dark: Vec3::splat(0.01),
            light: Vec3::new(1.0, 0.95, 0.85),
            threshold: 0.45,
            // Soft, so the tiers' slightly different frames don't flip pixels between the tones.
            softness: 0.3,
            coverage: EffectCoverage::WORLD,
        },
        speed_lines: SpeedLines {
            opacity: 1.0,
            color: Vec3::splat(0.01),
            coverage: EffectCoverage::WORLD,
            ..SpeedLines::NONE
        },
        ..PostEffects::NONE
    };
    let mut frames = Vec::new();
    for id in [TierId::DesktopLow, TierId::MobileLow] {
        let tier = table.get(id);
        let settings = RenderSettings { msaa: Msaa::X4, temporal: false, ..tier.render_settings() };
        let mut harness = Harness::new(settings, size);
        harness.renderer.set_render_scale(tier.resolution.base_scale(1080));
        harness.sky = SkyGradient {
            zenith: Vec3::new(0.15, 0.3, 0.7),
            horizon: Vec3::new(0.9, 0.7, 0.5),
            ground: Vec3::new(0.2, 0.15, 0.25),
        };
        harness.sphere(Vec3::new(-0.6, 0.0, 0.0), 0.5, flat([0.9, 0.5, 0.3]), true);
        harness.sphere(Vec3::new(0.9, 0.3, 0.0), 0.35, flat([0.3, 0.6, 0.9]), false);
        frames.push((
            harness.render(composited()),
            harness.render(PostParams { effects: impact, ..composited() }),
        ));
    }
    let [(desktop_plain, desktop), (mobile_plain, mobile)] = &frames[..] else { panic!("two tiers") };
    let plain_difference = mean_step(desktop_plain, mobile_plain);
    let effects_difference = mean_step(desktop, mobile);
    let effect = mean_step(desktop, desktop_plain);
    eprintln!(
        "Mobile-Low vs Desktop-Low: with the impact effects {effects_difference:.3}, without {plain_difference:.3} \
         mean steps apart; the effects move Desktop-Low's frame by {effect:.3}"
    );
    assert!(effect > 20.0, "{effect}");
    assert!(effects_difference < plain_difference + 2.0, "{effects_difference} vs {plain_difference}");
}
