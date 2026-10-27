//! Time of day as a whole: the default day, documents, timelines, zones and cutscenes.

use std::sync::Arc;

use sapphire_math::Vec3;
use sapphire_math::color::{linear_srgb_to_oklab, luminance};
use sapphire_render::{
    CharacterBloom, CharacterShade, ColorGrade, DirectionalLight, FramePacket, GradientBlend, GradientLayer,
    GradientShape, PostParams, SkyGradient, SkyLayers,
};
use sapphire_serialize::DocumentError;
use sapphire_time::{ScaleSource, ScaledClock};

use super::*;

/// The sun the sandbox drew with before time of day, and the reference time that reproduces it.
const DAY_SUN: DirectionalLight = DirectionalLight {
    direction: Vec3::new(0.55, -0.7, -0.45),
    color: Vec3::new(1.0, 0.97, 0.92),
    intensity: 1.0,
};
const DAY: f32 = 10.0;

fn time_of_day(hours: f32) -> (ScaledClock, TimeOfDay) {
    let mut world = ScaledClock::new();
    let clock = world.new_local_clock();
    let tod = TimeOfDay::new(default_day(), clock, hours).expect("complete");
    (world, tod)
}

/// A key setting only the groups a test needs.
fn key(time: f32) -> TodKey {
    TodKey { time, ..TodKey::default() }
}

fn sky(color: [f32; 3]) -> Option<SkyKey> {
    Some(SkyKey { zenith: color, horizon: color, ground: color })
}

#[test]
fn the_default_day_reproduces_the_default_look_exactly() {
    let day = default_day();
    assert!(day.is_complete());
    let look = day.evaluate(DAY);
    assert_eq!(look.key_light(), DAY_SUN, "the moon is dark at 10:00");
    assert_eq!(look.sky, SkyGradient::default());
    // Post as R3 and W1 drew it; the compositing layers (M1 P2) come on top.
    assert_eq!(look.post().plain(), PostParams::default());
    // Through the whole resolve, as the app runs it.
    let (_, mut tod) = time_of_day(DAY);
    let mut packet = FramePacket::default();
    tod.resolve(Vec3::new(1.0, 2.0, 3.0), 0.6).write(&mut packet);
    assert_eq!(
        (packet.sun, packet.sky, packet.post.plain()),
        (DAY_SUN, SkyGradient::default(), PostParams::default())
    );
    assert_eq!(packet.day_night, 0.0);
    // Bit for bit: the floats compare equal, but -0.0 == 0.0 too.
    assert_eq!(
        packet.sun.direction.to_array().map(f32::to_bits),
        DAY_SUN.direction.to_array().map(f32::to_bits)
    );
}

#[test]
fn the_default_day_round_trips_as_a_document() {
    let day = default_day();
    let text = day.to_ron().expect("write");
    assert!(text.starts_with("(\n    type: \"sapphire::world::TodTimeline\",\n    version: 1,\n"), "{text}");
    assert_eq!(TodTimeline::from_ron(&text).expect("read"), day);
    assert_eq!(day.keys().len(), 8);
    assert!(day.keys().windows(2).all(|pair| pair[0].time < pair[1].time));
}

#[test]
fn documents_are_checked_when_read() {
    let read = |keys: &str| {
        TodTimeline::from_ron(&format!(
            r#"(type: "sapphire::world::TodTimeline", version: 1, data: (keys: [{keys}]))"#
        ))
    };
    let invalid = |keys: &str| match read(keys) {
        Err(DocumentError::Parse(error)) => error.to_string(),
        other => panic!("expected a validation error, got {other:?}"),
    };
    assert!(read("(time: 3.0, exposure: 1.0)").is_ok());
    assert!(invalid("").contains("no keys"));
    assert!(invalid("(time: 24.0, exposure: 1.0)").contains("outside 0 ≤ hours < 24"));
    assert!(
        invalid("(time: 5.0, exposure: 1.0), (time: 2.0, exposure: 1.0)").contains("times must increase")
    );
    let mixed = invalid("(time: 1.0, exposure: 1.0), (time: 2.0, exposure: 1.0, day_night: 1.0)");
    assert!(mixed.contains("key 1 sets day_night, exposure but the first key sets exposure"), "{mixed}");
    assert!(invalid("(time: 1.0, day_night: 2.0)").contains("`day_night` must be within 0..=1"));
    assert!(invalid("(time: 1.0, exposre: 1.0)").contains("exposre"), "typos are errors, not ignored");
    let extra = TodTimeline::from_ron(
        r#"(type: "sapphire::world::TodTimeline", version: 1, data: (keys: [(time: 1.0, exposure: 1.0)], sky: ()))"#,
    );
    assert!(matches!(extra, Err(DocumentError::Parse(_))), "unknown fields outside keys too");
    let wrong = r#"(type: "sapphire::world::TodCutscene", version: 1, data: (duration: 1.0))"#;
    assert!(matches!(TodTimeline::from_ron(wrong), Err(DocumentError::WrongType { .. })));
    let newer = r#"(type: "sapphire::world::TodTimeline", version: 2, data: (keys: []))"#;
    assert!(matches!(TodTimeline::from_ron(newer), Err(DocumentError::TooNew { found: 2, current: 1 })));
}

#[test]
fn only_complete_timelines_can_be_the_base() {
    let partial = TodTimeline::new(vec![TodKey { exposure: Some(1.0), ..key(0.0) }]).expect("valid");
    assert!(!partial.is_complete());
    let clock = ScaledClock::new().new_local_clock();
    assert_eq!(TimeOfDay::new(partial.clone(), clock, 0.0).unwrap_err(), TodError::Incomplete(TodGroup::Sun));
    let (_, mut tod) = time_of_day(DAY);
    assert!(tod.set_timeline(partial).is_err());
    assert_eq!(tod.revision(), 0);
    assert!(tod.set_timeline(default_day()).is_ok());
    assert_eq!(tod.revision(), 1);
}

#[test]
fn colors_blend_in_oklab() {
    let (warm, cool) = ([1.0, 0.45, 0.2], [0.1, 0.15, 0.6]);
    let timeline =
        TodTimeline::new(vec![TodKey { sky: sky(warm), ..key(6.0) }, TodKey { sky: sky(cool), ..key(12.0) }])
            .expect("valid");
    let lab = |color: Vec3| Vec3::from(linear_srgb_to_oklab(color.to_array()));
    let (warm_lab, cool_lab) = (lab(Vec3::from(warm)), lab(Vec3::from(cool)));
    for (hours, t) in [(7.5, 0.25), (9.0, 0.5), (10.5, 0.75)] {
        let horizon = timeline.evaluate(hours).sky.horizon;
        assert!(
            lab(horizon).abs_diff_eq(warm_lab.lerp(cool_lab, t), 1e-4),
            "{hours}: a straight line in OkLab"
        );
    }
    assert_eq!(timeline.evaluate(6.0).sky.horizon, Vec3::from(warm), "exact on a key");
}

#[test]
fn timelines_wrap_at_midnight() {
    let timeline = TodTimeline::new(vec![
        TodKey { exposure: Some(1.0), ..key(4.0) },
        TodKey { exposure: Some(2.0), ..key(12.0) },
        TodKey { exposure: Some(3.0), ..key(22.0) },
    ])
    .expect("valid");
    let exposure = |hours: f32| timeline.evaluate(hours).exposure;
    // 22:00 to 04:00 is six hours across midnight.
    assert_eq!(timeline.segment(1.0), Segment { from: 2, to: 0, weight: 0.5 });
    assert_eq!(exposure(1.0), 2.0);
    assert_eq!(exposure(23.5), 2.5);
    assert!((exposure(0.0) - 7.0 / 3.0).abs() < 1e-6);
    assert_eq!(exposure(25.0), 2.0, "times past the day wrap into it");
    assert_eq!(exposure(-23.0), 2.0);
    assert_eq!(exposure(4.0), 1.0);
    assert_eq!(exposure(8.0), 1.5);
    // Continuous through midnight.
    assert!((exposure(23.999) - exposure(0.0)).abs() < 1e-3);
    // One key holds all day.
    let still = TodTimeline::new(vec![TodKey { exposure: Some(5.0), ..key(13.0) }]).expect("valid");
    assert_eq!(still.evaluate(2.0).exposure, 5.0);
}

#[test]
fn easing_shapes_each_segment() {
    let keys = |ease| {
        TodTimeline::new(vec![
            TodKey { ease, exposure: Some(0.0), ..key(0.0) },
            TodKey { exposure: Some(1.0), ..key(4.0) },
        ])
        .expect("valid")
    };
    let at = |ease, hours| keys(ease).evaluate(hours).exposure;
    assert_eq!(at(TodEase::Linear, 1.0), 0.25);
    assert_eq!(at(TodEase::Smooth, 2.0), 0.5);
    assert!(at(TodEase::Smooth, 0.4) < 0.1);
    assert_eq!(at(TodEase::In, 2.0), 0.25);
    assert_eq!(at(TodEase::Out, 2.0), 0.75);
    assert_eq!(at(TodEase::Hold, 3.9), 0.0);
    assert_eq!(at(TodEase::Hold, 4.0), 1.0);
    // The second key's own ease shapes the way back around the day.
    assert_eq!(at(TodEase::Hold, 14.0), 1.0 - 0.5);
}

#[test]
fn the_default_day_never_lights_from_below() {
    let day = default_day();
    for minute in 0..24 * 60 {
        let key = day.evaluate(minute as f32 / 60.0).key_light();
        assert!(key.intensity == 0.0 || key.direction.y < 0.0, "{minute} min: {key:?} travels upward");
    }
}

/// The default day's character key, as it reaches characters, against the world's key light, as
/// the screen shows them (luminance, exposed): the same light by day, never dimmer, and from
/// sunset to sunrise a little brighter, so characters read, yet well below their noon brightness
/// at night, so they read as night. A night key set apart from the world's light kept
/// characters at 80–100% of noon once exposure lifted the night, while the world dropped to a
/// quarter to a half.
///
/// Every quarter hour, and every 5 minutes through dusk and dawn, where the sun sets and rises
/// and the moon rises: the world's light fades over a body's last degree above the horizon,
/// and a character key that didn't fade with it flashed to twice the world's for the minutes
/// in between.
#[test]
fn the_default_days_characters_stay_a_little_brighter_than_the_world_at_night() {
    let day = default_day();
    let shown =
        |look: &TodLook, color: Vec3, intensity: f32| luminance(color) * intensity * look.exposure.exp2();
    let noon = day.evaluate(13.0);
    let noon_key = noon.character_key();
    let noon_character = shown(&noon, noon_key.color, noon_key.intensity);
    let quarters = (0..24 * 4).map(|quarter| quarter as f32 / 4.0);
    // 17:00 to 21:00 and 4:00 to 7:00, every 5 minutes.
    let crossings = (17 * 12..=21 * 12).chain(4 * 12..=7 * 12).map(|step| step as f32 / 12.0);
    for hours in quarters.chain(crossings) {
        let look = day.evaluate(hours);
        let key = look.key_light();
        let character_key = look.character_key();
        let character = shown(&look, character_key.color, character_key.intensity);
        let lift = character / shown(&look, key.color, key.intensity);
        assert!(lift >= 0.999, "{hours}: characters are never lit below the world ({lift})");
        assert!(lift <= 1.6, "{hours}: never more than a little brighter than the world ({lift})");
        if (10.0..=14.0).contains(&hours) {
            assert!((lift - 1.0).abs() < 1e-4, "{hours}: by day the same light ({lift})");
        }
        // From sunset (18:45) through the night to sunrise (6:30).
        if hours >= 18.75 || hours <= 6.5 {
            assert!((1.2..=1.6).contains(&lift), "{hours}: a little brighter than the world ({lift})");
        }
        // From dusk (20:00) through the blue hour (5:00).
        if hours >= 20.0 || hours <= 5.0 {
            let share = character / noon_character;
            assert!(share <= 0.7, "{hours}: at night, at most 70% of noon's brightness ({share})");
        }
    }
}

#[test]
fn the_sun_moves_along_a_great_circle() {
    let to_sun = |direction: Vec3| -direction.normalize();
    let day = default_day();
    let (morning, afternoon) =
        (to_sun(day.evaluate(10.0).sun.direction), to_sun(day.evaluate(14.0).sun.direction));
    let angle = |a: Vec3, b: Vec3| a.dot(b).clamp(-1.0, 1.0).acos();
    let total = angle(morning, afternoon);
    for hours in [10.5, 11.0, 12.0, 13.5] {
        let sun = to_sun(day.evaluate(hours).sun.direction);
        let progress = (hours - 10.0) / 4.0;
        assert!((angle(morning, sun) - progress * total).abs() < 1e-4, "{hours}: constant angular speed");
        assert!((angle(morning, sun) + angle(sun, afternoon) - total).abs() < 1e-4, "{hours}: on the arc");
    }
}

#[test]
fn neighboring_grades_blend_as_two_luts_and_a_single_grade_uses_one() {
    let day = default_day();
    let grade =
        |hours: f32| ColorGrade::from(day.keys().iter().find(|k| k.time == hours).unwrap().grade.unwrap());
    // Golden hour to sunset, 40% of the way.
    let post = day.evaluate(18.0).post();
    assert_eq!((post.grade_a, post.grade_b), (grade(17.5), grade(18.75)));
    assert!((post.grade_blend - 0.4).abs() < 1e-5);
    // Day and afternoon share a grade: one LUT, no blend.
    let post = day.evaluate(12.0).post();
    assert_eq!((post.grade_a, post.grade_b, post.grade_blend), (grade(10.0), grade(10.0), 0.0));
}

fn zone(groups: TodKey) -> Arc<TodTimeline> {
    Arc::new(TodTimeline::new(vec![groups]).expect("valid"))
}

#[test]
fn zones_blend_their_groups_by_weight_and_priority() {
    let (_, mut tod) = time_of_day(DAY);
    let base = *tod.resolve(Vec3::ZERO, 0.0);
    // A cave: dark, cold sky and +1 stop, with a 10 m blend around a 5 m sphere.
    let cave = TodKey { sky: sky([0.0, 0.0, 0.1]), exposure: Some(1.0), ..key(0.0) };
    let shape = VolumeShape::Sphere { center: Vec3::new(20.0, 0.0, 0.0), radius: 5.0 };
    let id = tod.add_volume(TodVolume { blend_distance: 10.0, ..TodVolume::new(shape, zone(cave)) });
    let at = |tod: &mut TimeOfDay, x: f32| *tod.resolve(Vec3::new(x, 0.0, 0.0), 0.0);

    let inside = at(&mut tod, 20.0);
    assert_eq!(inside.look.exposure, 1.0);
    assert_eq!(inside.look.sky.zenith, Vec3::new(0.0, 0.0, 0.1));
    assert_eq!(inside.look.sun, base.look.sun, "groups the zone doesn't set stay the base's");
    assert_eq!(tod.stats().active_volumes, 1);

    let edge = at(&mut tod, 35.0);
    assert_eq!(edge.look, base.look, "beyond the blend distance");
    assert_eq!(tod.stats().active_volumes, 0);

    let halfway = at(&mut tod, 30.0);
    assert_eq!(halfway.look.exposure, 0.5, "halfway through the blend");
    let lab = |color: Vec3| Vec3::from(linear_srgb_to_oklab(color.to_array()));
    let expected = lab(base.look.sky.zenith).lerp(lab(Vec3::new(0.0, 0.0, 0.1)), 0.5);
    assert!(lab(halfway.look.sky.zenith).abs_diff_eq(expected, 1e-4));

    // A higher priority volume applies over it, whatever order they were added in.
    let bright = TodKey { exposure: Some(-2.0), ..key(0.0) };
    let cover = VolumeShape::Box { center: Vec3::new(20.0, 0.0, 0.0), half_extents: Vec3::splat(2.0) };
    tod.add_volume(TodVolume { priority: 1, ..TodVolume::new(cover, zone(bright)) });
    let lower = TodKey { exposure: Some(4.0), ..key(0.0) };
    tod.add_volume(TodVolume { priority: -1, ..TodVolume::new(cover, zone(lower)) });
    assert_eq!(at(&mut tod, 20.0).look.exposure, -2.0);
    assert_eq!(tod.volumes().map(|(_, volume)| volume.priority).collect::<Vec<_>>(), [-1, 0, 1]);

    assert!(tod.remove_volume(id).is_some());
    assert!(tod.remove_volume(id).is_none());
    assert_eq!(tod.volumes().len(), 2);
}

#[test]
fn a_zone_grade_over_a_time_blend_keeps_the_two_heaviest_luts() {
    let (_, mut tod) = time_of_day(18.0);
    let zone_grade = ColorGrade { saturation: 0.5, ..ColorGrade::NEUTRAL };
    let grey = TodKey { grade: Some(zone_grade.into()), ..key(0.0) };
    let shape = VolumeShape::Sphere { center: Vec3::ZERO, radius: 1.0 };
    tod.add_volume(TodVolume { strength: 0.2, ..TodVolume::new(shape, zone(grey)) });
    tod.resolve(Vec3::ZERO, 0.0);
    // Golden hour 0.48, sunset 0.32, the zone 0.2: the zone's grade is dropped.
    let stats = tod.stats();
    assert!((stats.dropped_grade_weight - 0.2).abs() < 1e-5, "{stats:?}");
    let post = tod.frame().look.post();
    assert_ne!(post.grade_b, zone_grade);
    assert!((post.grade_blend - 0.4).abs() < 1e-5, "the remaining pair renormalized");
}

#[test]
fn cutscenes_blend_in_and_out() {
    let (mut world, mut tod) = time_of_day(DAY);
    let base = *tod.resolve(Vec3::ZERO, 0.0);
    // A 4 s shot at sunset with a second's blend each way, which also warms exposure.
    let shot = TodCutscene::new(
        4.0,
        1.0,
        1.0,
        Some(ShotTime { hours: 18.75, hours_per_second: 0.0 }),
        vec![TodKey { exposure: Some(1.0), ..key(0.0) }, TodKey { exposure: Some(2.0), ..key(4.0) }],
        None,
    )
    .expect("valid");
    let sunset = default_day().evaluate(18.75);
    tod.play_cutscene(Arc::new(shot.clone()));
    let mut weights = Vec::new();
    for _ in 0..300 {
        world.tick();
        tod.tick(&world);
        let frame = *tod.resolve(Vec3::ZERO, 0.0);
        weights.push(tod.stats().cutscene_weight);
        assert_eq!(frame.hours, DAY, "the world's clock is untouched");
        if let Some((_, seconds)) = tod.cutscene()
            && (1.5..2.5).contains(&seconds)
        {
            assert_eq!(frame.look.sky, sunset.sky, "fully in: the sunset's look");
            assert!((frame.look.exposure - (1.0 + seconds / 4.0)).abs() < 1e-3, "the shot's own keys");
        }
    }
    // Eased in over the first second, held, eased out over the last, then gone.
    assert!(weights[0] > 0.0 && weights[0] < 0.01);
    assert!(weights[29] > 0.4 && weights[29] < 0.6, "halfway in: {}", weights[29]);
    assert!(weights[60..180].iter().all(|&weight| weight == 1.0));
    assert!(weights[209] > 0.4 && weights[209] < 0.6, "halfway out: {}", weights[209]);
    assert!(weights[240..].iter().all(|&weight| weight == 0.0));
    assert!(tod.cutscene().is_none(), "it ended by itself");
    assert_eq!(tod.resolve(Vec3::ZERO, 0.0).look, base.look);

    // Partway in, the look is a crossfade of the two.
    tod.play_cutscene(Arc::new(shot));
    tod.seek_cutscene(0.5);
    let partway = *tod.resolve(Vec3::ZERO, 0.0);
    let weight = tod.stats().cutscene_weight;
    assert!(weight > 0.0 && weight < 1.0);
    assert_eq!(partway.look.day_night, sunset.day_night * weight);
    tod.stop_cutscene();
    assert_eq!(tod.resolve(Vec3::ZERO, 0.0).look, base.look);
}

#[test]
fn cutscene_documents_are_checked() {
    let read = |data: &str| {
        TodCutscene::from_ron(&format!(
            r#"(type: "sapphire::world::TodCutscene", version: 1, data: ({data}))"#
        ))
    };
    let shot =
        read("duration: 3.0, blend_in: 0.5, time: (hours: 6.5, hours_per_second: 0.5)").expect("valid");
    assert_eq!(shot.hours(2.0), Some(7.5), "a time-lapse");
    assert_eq!(shot.weight(-0.1), 0.0);
    assert_eq!(shot.weight(3.0), 1.0, "no blend out: on to the last instant");
    assert_eq!(shot.weight(3.1), 0.0);
    assert_eq!(TodCutscene::from_ron(&shot.to_ron().expect("write")).expect("read"), shot);
    let midnight = read("duration: 10.0, time: (hours: 23.0, hours_per_second: 0.25)").expect("valid");
    assert_eq!(midnight.hours(8.0), Some(1.0), "wraps past midnight");
    for (data, problem) in [
        ("duration: 0.0, time: (hours: 1.0)", "duration"),
        ("duration: 1.0, blend_in: 0.6, blend_out: 0.6, time: (hours: 1.0)", "fit in the duration"),
        ("duration: 1.0, time: (hours: 25.0)", "time.hours"),
        ("duration: 1.0", "would change nothing"),
        ("duration: 1.0, keys: [(time: 2.0, exposure: 1.0)]", "the shot's duration"),
    ] {
        let error = read(data).expect_err(data).to_string();
        assert!(error.contains(problem), "{data}: {error}");
    }
}

#[test]
fn resolving_is_deterministic() {
    // Twice from scratch, through a running cycle with a zone and a cutscene: the same bits.
    let run = || {
        let (mut world, mut tod) = time_of_day(4.0);
        tod.clock_mut().set_day_length(Some(30.0));
        let shape = VolumeShape::Box { center: Vec3::ZERO, half_extents: Vec3::splat(3.0) };
        let fog = TodKey { fog: Some(FogKey::default()), exposure: Some(0.3), ..key(0.0) };
        tod.add_volume(TodVolume { blend_distance: 5.0, strength: 0.7, ..TodVolume::new(shape, zone(fog)) });
        let shot = TodCutscene::new(
            2.0,
            0.5,
            0.5,
            Some(ShotTime { hours: 19.0, hours_per_second: 1.0 }),
            Vec::new(),
            None,
        )
        .expect("valid");
        let mut frames = Vec::new();
        for step in 0..1800_u32 {
            world.tick();
            tod.tick(&world);
            if step == 600 {
                tod.play_cutscene(Arc::new(shot.clone()));
            }
            let probe = Vec3::new((step as f32 * 0.01).sin() * 8.0, 0.0, 0.0);
            let mut packet = FramePacket::default();
            tod.resolve(probe, 0.5).write(&mut packet);
            frames.push(format!(
                "{:?}",
                (packet.sun, packet.celestial, packet.sky, packet.sky_layers, packet.fog, packet.post)
            ));
        }
        frames
    };
    assert_eq!(run(), run());
}

#[test]
fn the_cycle_pauses_with_the_world() {
    let (mut world, mut tod) = time_of_day(6.0);
    tod.clock_mut().set_day_length(Some(24.0));
    world.push_scale(ScaleSource::Pause, 0).expect("room");
    for _ in 0..60 {
        world.tick();
        tod.tick(&world);
    }
    assert_eq!(tod.resolve(Vec3::ZERO, 1.0).hours, 6.0);
}

/// A complete timeline as written before the sky's layers (M1 W1): every group it had then, and
/// none of `discs`, `stars` or `clouds`.
const TIMELINE_BEFORE_SKY_LAYERS: &str = r#"(
    type: "sapphire::world::TodTimeline",
    version: 1,
    data: (
        keys: [
            (
                time: 6.0,
                sun: (direction: (0.9, -0.1, -0.4), color: (1.0, 0.6, 0.4), intensity: 0.8),
                moon: (direction: (-0.9, 0.1, 0.4), color: (0.6, 0.7, 1.0), intensity: 0.0),
                sky: (zenith: (0.1, 0.2, 0.6), horizon: (1.0, 0.7, 0.5), ground: (0.1, 0.1, 0.2)),
                ambient: (sky: (1.0, 0.7, 0.6), ground: (0.1, 0.1, 0.2), intensity: 0.1),
                fog: (far: (color: (1.0, 0.7, 0.5), density: 0.005, start: 20.0)),
                shadow_tint: (0.4, 0.35, 0.5),
                character: (color: (1.0, 0.7, 0.6), intensity: 1.0),
                day_night: 0.3,
                exposure: 0.2,
                grade: (saturation: 1.1),
                bloom: (intensity: 0.3),
                vignette: (intensity: 0.2),
            ),
        ],
    ),
)"#;

#[test]
fn timelines_without_sky_layers_load_and_draw_the_sky_as_before() {
    let old = TodTimeline::from_ron(TIMELINE_BEFORE_SKY_LAYERS).expect("an older document loads");
    assert!(old.is_complete(), "it can still be the base timeline");
    for group in TodGroup::SKY_LAYERS {
        assert!(!old.groups().contains(group), "{group}");
    }
    let clock = ScaledClock::new().new_local_clock();
    let mut tod = TimeOfDay::new(old, clock, 6.0).expect("a complete base");
    let mut packet = FramePacket::default();
    tod.resolve(Vec3::ZERO, 0.0).write(&mut packet);
    assert_eq!(packet.sky_layers, SkyLayers::default(), "no discs, stars or clouds: the gradient alone");
    assert_eq!(packet.sky.horizon, Vec3::new(1.0, 0.7, 0.5), "its own keys still draw");
}

#[test]
fn sky_layer_groups_round_trip_and_are_checked() {
    let layers = TodKey {
        discs: Some(DiscsKey {
            sun: DiscKey { color: [1.0, 0.9, 0.8], intensity: 30.0, glow: [0.5, 0.3, 0.1], glow_size: 35.0 },
            moon: DiscKey { color: [1.0, 1.0, 0.9], intensity: 2.0, ..DiscKey::default() },
        }),
        stars: Some(0.8),
        nebula: Some(NebulaKey {
            clouds: [0.05, 0.05, 0.2],
            wisps: [0.1, 0.4, 0.4],
            accents: [0.3, 0.0, 0.2],
        }),
        clouds: Some(CloudsKey {
            lit: [1.0, 0.9, 0.8],
            shade: [0.5, 0.5, 0.7],
            rim: [1.2, 1.0, 0.8],
            coverage: 0.7,
            cirrus: 0.4,
        }),
        ..key(3.0)
    };
    let timeline = TodTimeline::new(vec![layers]).expect("valid");
    let text = timeline.to_ron().expect("write");
    assert!(
        text.contains("discs:")
            && text.contains("stars: Some(0.8)")
            && text.contains("nebula:")
            && text.contains("clouds:"),
        "{text}"
    );
    let look = timeline.evaluate(3.0);
    assert_eq!(look.nebula.wisps, Vec3::new(0.1, 0.4, 0.4), "the nebula's colors reach the look");
    assert_eq!(TodTimeline::from_ron(&text).expect("read"), timeline);
    // Left-out glow and cloud fields default; left-out groups stay unset.
    let partial = TodTimeline::from_ron(
        r#"(type: "sapphire::world::TodTimeline", version: 1, data: (keys: [(time: 1.0, discs: (
            sun: (color: (1.0, 1.0, 1.0), intensity: 10.0), moon: (color: (1.0, 1.0, 1.0), intensity: 0.0)),
            clouds: (lit: (1.0, 1.0, 1.0), shade: (0.5, 0.5, 0.5)))]))"#,
    )
    .expect("valid");
    let look = partial.evaluate(1.0);
    assert_eq!((look.sun_disc.glow, look.sun_disc.glow_radius), (Vec3::ZERO, 0.0));
    assert_eq!((look.clouds.coverage, look.clouds.rim), (0.0, Vec3::ZERO));
    assert_eq!(partial.groups().iter().collect::<Vec<_>>(), [TodGroup::Discs, TodGroup::Clouds]);
    // Values out of range name their field.
    let invalid = |groups: &str| {
        let text = format!(
            r#"(type: "sapphire::world::TodTimeline", version: 1, data: (keys: [(time: 1.0, {groups})]))"#
        );
        TodTimeline::from_ron(&text).expect_err(groups).to_string()
    };
    assert!(invalid("stars: -1.0").contains("`stars`"));
    assert!(invalid("nebula: (accents: (0.1, -0.2, 0.1))").contains("nebula.accents"));
    assert!(invalid("nebula: (glow: (0.1, 0.2, 0.1))").contains("glow"), "typos are errors");
    let black = TodTimeline::from_ron(
        r#"(type: "sapphire::world::TodTimeline", version: 1, data: (keys: [(time: 1.0, nebula: ())]))"#,
    )
    .expect("a nebula left black");
    assert_eq!(black.evaluate(1.0).nebula, sapphire_render::Nebula::default(), "black, as by day");
    let cover = "clouds: (lit: (1.0, 1.0, 1.0), shade: (0.0, 0.0, 0.0), coverage: 1.5)";
    assert!(invalid(cover).contains("clouds.coverage"));
    assert!(invalid("clouds: (lit: (1.0, 1.0, 1.0), shade: (0.0, -1.0, 0.0))").contains("clouds.shade"));
    let glow = "discs: (sun: (color: (1.0, 1.0, 1.0), intensity: 1.0, glow_size: 200.0), \
                moon: (color: (1.0, 1.0, 1.0), intensity: 0.0))";
    assert!(invalid(glow).contains("discs.sun.glow_size"));
    let typo = "clouds: (lit: (1.0, 1.0, 1.0), shade: (0.0, 0.0, 0.0), cover: 0.5)";
    assert!(invalid(typo).contains("cover"), "typos are errors");
}

#[test]
fn sky_layers_blend_in_oklab_like_the_rest() {
    let clouds = |lit: [f32; 3], coverage: f32| {
        Some(CloudsKey { lit, shade: lit, rim: lit, coverage, cirrus: coverage })
    };
    let timeline = TodTimeline::new(vec![
        TodKey { clouds: clouds([1.0, 0.45, 0.2], 0.2), stars: Some(0.0), ..key(6.0) },
        TodKey { clouds: clouds([0.1, 0.15, 0.6], 1.0), stars: Some(1.0), ..key(12.0) },
    ])
    .expect("valid");
    let lab = |color: Vec3| Vec3::from(linear_srgb_to_oklab(color.to_array()));
    let look = timeline.evaluate(9.0);
    let expected = lab(Vec3::new(1.0, 0.45, 0.2)).lerp(lab(Vec3::new(0.1, 0.15, 0.6)), 0.5);
    assert!(lab(look.clouds.lit).abs_diff_eq(expected, 1e-4), "colors in OkLab");
    // The nebula's colors too, fading out from night to day through their own hues.
    let teal = [0.1, 0.4, 0.4];
    let night = TodKey { nebula: Some(NebulaKey { wisps: teal, ..NebulaKey::default() }), ..key(0.0) };
    let day = TodKey { nebula: Some(NebulaKey::default()), ..key(12.0) };
    let fading = TodTimeline::new(vec![night, day]).expect("valid").evaluate(6.0).nebula.wisps;
    let expected = lab(Vec3::from(teal)).lerp(lab(Vec3::ZERO), 0.5);
    assert!(lab(fading).abs_diff_eq(expected, 1e-4), "in OkLab: {fading}");
    assert!((look.clouds.coverage - 0.6).abs() < 1e-6, "scalars linearly");
    assert!((look.stars - 0.5).abs() < 1e-6);
    assert_eq!(timeline.evaluate(6.0).clouds.lit, Vec3::new(1.0, 0.45, 0.2), "exact on a key");
    // Glow sizes are authored in degrees and drawn in radians.
    let discs = TodKey {
        discs: Some(DiscsKey {
            sun: DiscKey { glow_size: 30.0, ..DiscKey::default() },
            moon: DiscKey::default(),
        }),
        ..key(0.0)
    };
    let look = TodTimeline::new(vec![discs]).expect("valid").evaluate(0.0);
    assert!((look.sun_disc.glow_radius - 30f32.to_radians()).abs() < 1e-6);
}

#[test]
fn zones_and_cutscenes_override_the_sky_layers() {
    let (_, mut tod) = time_of_day(DAY);
    let base = *tod.resolve(Vec3::ZERO, 0.0);
    // A stormy zone: full cover and grey clouds, its discs and stars the base's.
    let storm = TodKey {
        clouds: Some(CloudsKey { lit: [0.4; 3], shade: [0.2; 3], coverage: 1.0, ..CloudsKey::default() }),
        ..key(0.0)
    };
    let shape = VolumeShape::Sphere { center: Vec3::ZERO, radius: 10.0 };
    tod.add_volume(TodVolume::new(shape, zone(storm)));
    let inside = *tod.resolve(Vec3::ZERO, 0.0);
    assert_eq!(inside.look.clouds.coverage, 1.0);
    assert_eq!(inside.look.clouds.lit, Vec3::splat(0.4));
    assert_eq!((inside.look.sun_disc, inside.look.stars), (base.look.sun_disc, base.look.stars));
    // A shot that brings the stars out by day.
    let stars = vec![TodKey { stars: Some(1.0), ..key(0.0) }];
    let shot = TodCutscene::new(2.0, 0.0, 0.0, None, stars, None).expect("valid");
    tod.play_cutscene(Arc::new(shot));
    assert_eq!(tod.resolve(Vec3::ZERO, 0.0).look.stars, 1.0);
}

#[test]
fn the_stars_turn_with_the_hour_about_the_sky_pole() {
    let pole = celestial_sphere(0.0) * Vec3::Y;
    let angle = |a: Vec3, b: Vec3| a.dot(b).clamp(-1.0, 1.0).acos().to_degrees();
    for hours in [0.0, 5.5, 13.0, 23.9] {
        assert!((celestial_sphere(hours) * Vec3::Y).abs_diff_eq(pole, 1e-6), "{hours}: the pole stays put");
    }
    assert!(pole.y > 0.5 && pole.z < 0.0, "up over the far horizon: {pole}");
    // A star on the celestial equator turns 15° an hour, and once a day.
    let at = |hours: f32| celestial_sphere(hours) * Vec3::X;
    assert!((angle(at(3.0), at(4.0)) - 15.0).abs() < 1e-3);
    assert!(at(0.0).abs_diff_eq(at(24.0), 1e-5));
    // Rising on the left (−X) and setting on the right: a star crossing the meridian moves
    // toward +X.
    let meridian = (Vec3::Z - pole * pole.z).normalize();
    let star = celestial_sphere(10.0).inverse() * meridian;
    assert!((celestial_sphere(11.0) * star).x > 0.0, "it moves toward +X");
    // The packet carries the hour's turn, and the default day's night has stars and a moon.
    let (_, mut tod) = time_of_day(22.0);
    let mut packet = FramePacket::default();
    tod.resolve(Vec3::ZERO, 0.0).write(&mut packet);
    assert_eq!(packet.celestial.sphere, celestial_sphere(22.0));
    assert!(packet.sky_layers.stars > 0.5 && packet.sky_layers.moon.intensity > 0.0);
    let day = default_day().evaluate(DAY);
    assert_eq!(day.stars, 0.0, "none by day");
    assert!(day.sun_disc.intensity > 1.0 && day.clouds.coverage > 0.0, "a bright sun and clouds by day");
}

#[test]
fn timelines_without_post_layers_load_and_draw_post_as_before() {
    let old = TodTimeline::from_ron(TIMELINE_BEFORE_SKY_LAYERS).expect("an older document loads");
    assert!(old.is_complete(), "it can still be the base timeline");
    for group in TodGroup::POST_LAYERS {
        assert!(!old.groups().contains(group) && !TodGroups::REQUIRED.contains(group), "{group}");
    }
    let clock = ScaledClock::new().new_local_clock();
    let mut tod = TimeOfDay::new(old, clock, 6.0).expect("a complete base");
    let mut packet = FramePacket::default();
    tod.resolve(Vec3::ZERO, 0.0).write(&mut packet);
    assert_eq!(packet.post, packet.post.plain(), "no compositing layers: post as before");
    assert_eq!(packet.post.vignette.intensity, 0.2, "its own keys still draw");
    // The default day sets every layer at every key.
    let day = default_day();
    for group in TodGroup::POST_LAYERS {
        assert!(day.groups().contains(group), "{group}");
    }
    for hours in [0.0, 6.5, 10.0, 18.75] {
        let post = day.evaluate(hours).post();
        assert!(post.diffusion.intensity > 0.0, "{hours}: a glow");
        assert_ne!(post.character_bloom, CharacterBloom::NEUTRAL, "{hours}: faces kept from blowing out");
        assert_eq!(post.gradients.iter().filter(|layer| layer.opacity > 0.0).count(), 3, "{hours}");
    }
    assert!(day.evaluate(18.75).post().lens_flare.intensity > 0.0, "a flare at sunset");
    assert_eq!(day.evaluate(0.0).post().lens_flare.intensity, 0.0, "none at midnight");
    assert_ne!(day.evaluate(0.0).post().vignette.color, Vec3::ZERO, "a tinted vignette at night");
}

/// A key with every compositing layer (M1 P2) set.
fn post_layers(time: f32) -> TodKey {
    TodKey {
        vignette: Some(VignetteKey { intensity: 0.3, start: 0.4, color: [0.05, 0.02, 0.15] }),
        diffusion: Some(DiffusionKey { intensity: 1.5, threshold: 0.2, color: [1.0, 0.7, 0.5] }),
        character_bloom: Some(CharacterBloomKey { threshold: 1.5, strength: 0.4 }),
        gradients: Some(vec![
            GradientKey {
                shape: GradientShapeKey::Linear { edge: 0.0 },
                blend: GradientBlendKey::Multiply,
                color: [0.6, 0.5, 0.9],
                opacity: 0.4,
                start: 0.0,
                end: 0.5,
            },
            GradientKey {
                shape: GradientShapeKey::Radial { center: [0.5, 0.4] },
                blend: GradientBlendKey::SoftLight,
                color: [1.0, 0.8, 0.6],
                opacity: 0.2,
                start: 0.0,
                end: 0.9,
            },
        ]),
        lens_flare: Some(LensFlareKey { intensity: 1.2, color: [1.0, 0.6, 0.4] }),
        ..key(time)
    }
}

#[test]
fn post_layer_groups_round_trip_and_are_checked() {
    let timeline = TodTimeline::new(vec![post_layers(3.0)]).expect("valid");
    let text = timeline.to_ron().expect("write");
    for group in
        ["diffusion:", "character_bloom:", "gradients:", "Linear(", "Radial(", "SoftLight", "lens_flare:"]
    {
        assert!(text.contains(group), "{group}: {text}");
    }
    assert_eq!(TodTimeline::from_ron(&text).expect("read"), timeline);
    let look = timeline.evaluate(3.0);
    assert_eq!(look.gradients[0].shape, GradientShape::Linear { edge: 0.0 });
    assert_eq!(look.gradients[1].blend, GradientBlend::SoftLight);
    assert_eq!(look.gradients[2], GradientLayer::NONE, "slots past the key's layers are empty");
    assert_eq!(look.vignette.color, Vec3::new(0.05, 0.02, 0.15));
    // Left-out fields default: a white glow and flare, a normal, opaque layer over the whole
    // frame, characters as the world, and a black vignette.
    let partial = TodTimeline::from_ron(
        r#"(type: "sapphire::world::TodTimeline", version: 1, data: (keys: [(time: 1.0,
            diffusion: (intensity: 1.0), character_bloom: (strength: 0.5),
            gradients: [(shape: Linear(edge: 90.0), color: (1.0, 0.5, 0.0))],
            lens_flare: (intensity: 0.5), vignette: (intensity: 0.1))]))"#,
    )
    .expect("valid");
    let look = partial.evaluate(1.0);
    assert_eq!((look.diffusion.color, look.diffusion.threshold), (Vec3::ONE, 0.0));
    assert_eq!(look.character_bloom, CharacterBloom { threshold: 1.0, strength: 0.5 });
    let layer = look.gradients[0];
    assert_eq!((layer.blend, layer.opacity, layer.start, layer.end), (GradientBlend::Normal, 1.0, 0.0, 1.0));
    let GradientShape::Linear { edge } = layer.shape else { panic!("linear") };
    assert!((edge - std::f32::consts::FRAC_PI_2).abs() < 1e-6, "edges authored in degrees arrive in radians");
    assert_eq!((look.lens_flare.color, look.vignette.color), (Vec3::ONE, Vec3::ZERO));
    // Values out of range name their field; typos and unknown shapes are errors.
    let invalid = |groups: &str| {
        let text = format!(
            r#"(type: "sapphire::world::TodTimeline", version: 1, data: (keys: [(time: 1.0, {groups})]))"#
        );
        TodTimeline::from_ron(&text).expect_err(groups).to_string()
    };
    assert!(invalid("diffusion: (intensity: -1.0)").contains("diffusion.intensity"));
    assert!(invalid("diffusion: (intensity: 1.0, color: (1.0, -1.0, 1.0))").contains("diffusion.color"));
    assert!(invalid("character_bloom: (threshold: -0.5)").contains("character_bloom.threshold"));
    let opacity = "gradients: [(shape: Linear(edge: 0.0), color: (1.0, 1.0, 1.0), opacity: 1.5)]";
    assert!(invalid(opacity).contains("gradients.opacity"));
    let layer = "(shape: Radial(center: (0.5, 0.5)), color: (1.0, 1.0, 1.0))";
    let five = format!("gradients: [{layer}, {layer}, {layer}, {layer}, {layer}]");
    assert!(invalid(&five).contains("at most 4"));
    assert!(invalid("lens_flare: (intensity: 1.0, color: (1.0, 1.0, -0.1))").contains("lens_flare.color"));
    assert!(invalid("vignette: (color: (-1.0, 0.0, 0.0))").contains("vignette.color"));
    let typo = "gradients: [(shape: Linear(edge: 0.0), colour: (1.0, 1.0, 1.0))]";
    assert!(invalid(typo).contains("colour"), "typos are errors");
    assert!(!invalid("gradients: [(shape: Diagonal, color: (1.0, 1.0, 1.0))]").is_empty());
    // Every key of a timeline sets the same groups, the new ones too.
    let mixed = TodTimeline::new(vec![post_layers(1.0), key(2.0)]);
    assert!(matches!(mixed, Err(TodError::MixedGroups { .. })), "{mixed:?}");
}

#[test]
fn post_layers_blend_in_oklab_and_gradient_layers_slot_by_slot() {
    let lab = |color: Vec3| Vec3::from(linear_srgb_to_oklab(color.to_array()));
    let warm = post_layers(6.0);
    let mut cool = post_layers(12.0);
    cool.diffusion = Some(DiffusionKey { intensity: 0.5, threshold: 0.4, color: [0.3, 0.4, 1.0] });
    cool.vignette = Some(VignetteKey { color: [0.0; 3], ..VignetteKey::default() });
    let gradients = cool.gradients.as_mut().expect("set");
    // The first layer changes only its color and opacity; the second its blend mode; a third
    // appears.
    gradients[0].color = [0.2, 0.3, 0.9];
    gradients[0].opacity = 0.8;
    gradients[1].blend = GradientBlendKey::Screen;
    gradients.push(GradientKey {
        shape: GradientShapeKey::Linear { edge: 180.0 },
        blend: GradientBlendKey::Add,
        color: [1.0, 0.5, 0.2],
        opacity: 0.6,
        start: 0.0,
        end: 0.3,
    });
    let timeline = TodTimeline::new(vec![warm, cool]).expect("valid");
    let at = |hours: f32| timeline.evaluate(hours);
    let middle = at(9.0);
    let expected = lab(Vec3::new(1.0, 0.7, 0.5)).lerp(lab(Vec3::new(0.3, 0.4, 1.0)), 0.5);
    assert!(lab(middle.diffusion.color).abs_diff_eq(expected, 1e-4), "glow colors in OkLab");
    assert!((middle.diffusion.intensity - 1.0).abs() < 1e-6, "numbers linearly");
    assert!(lab(middle.vignette.color).abs_diff_eq(lab(Vec3::new(0.05, 0.02, 0.15)) * 0.5, 1e-4));
    // A layer whose shape and mode match blends.
    let first = middle.gradients[0];
    let expected = lab(Vec3::new(0.6, 0.5, 0.9)).lerp(lab(Vec3::new(0.2, 0.3, 0.9)), 0.5);
    assert!(lab(first.color).abs_diff_eq(expected, 1e-4) && (first.opacity - 0.6).abs() < 1e-6);
    // One whose mode changes fades out by halfway, then the other fades in: never a jump.
    let second = |hours: f32| at(hours).gradients[1];
    assert_eq!(second(7.5).blend, GradientBlend::SoftLight);
    assert!((second(7.5).opacity - 0.1).abs() < 1e-6, "{:?}", second(7.5));
    assert!(second(9.0).opacity.abs() < 1e-6, "nothing at the switch");
    assert_eq!(second(10.5).blend, GradientBlend::Screen);
    assert!((second(10.5).opacity - 0.1).abs() < 1e-6);
    // A layer only the later key has fades in from nothing, in its own color all along.
    let third = |hours: f32| at(hours).gradients[2];
    assert_eq!(third(6.0), GradientLayer::NONE, "exact on the key");
    assert_eq!(third(7.5).color, Vec3::new(1.0, 0.5, 0.2));
    assert!((third(7.5).opacity - 0.15).abs() < 1e-6 && third(7.5).blend == GradientBlend::Add);
    let later = TodTimeline::new(vec![timeline.keys()[1].clone()]).expect("valid");
    assert_eq!(at(12.0).post(), later.evaluate(12.0).post(), "exact on the other key");
}

#[test]
fn zones_and_cutscenes_override_the_post_layers() {
    let (_, mut tod) = time_of_day(DAY);
    let base = *tod.resolve(Vec3::ZERO, 0.0);
    // A misty zone: a stronger glow, the base's gradients and flare kept.
    let mist = TodKey {
        diffusion: Some(DiffusionKey { intensity: 3.0, threshold: 0.0, color: [0.9, 0.95, 1.0] }),
        ..key(0.0)
    };
    let shape = VolumeShape::Sphere { center: Vec3::ZERO, radius: 10.0 };
    let zone_id = tod.add_volume(TodVolume::new(shape, zone(mist)));
    let inside = *tod.resolve(Vec3::ZERO, 0.0);
    assert_eq!(inside.look.diffusion.intensity, 3.0);
    assert_eq!((inside.look.gradients, inside.look.lens_flare), (base.look.gradients, base.look.lens_flare));
    tod.remove_volume(zone_id);
    // A shot with gradient layers of its own, eased in and out like its other values.
    let shot_key = post_layers(0.0);
    let shot_gradients = TodTimeline::new(vec![shot_key.clone()]).expect("valid").evaluate(0.0).gradients;
    let keys = vec![TodKey { gradients: shot_key.gradients, ..key(0.0) }];
    let shot = TodCutscene::new(4.0, 1.0, 1.0, None, keys, None).expect("valid");
    tod.play_cutscene(Arc::new(shot));
    tod.seek_cutscene(2.0);
    let held = *tod.resolve(Vec3::ZERO, 0.0);
    assert_eq!(held.look.gradients, shot_gradients, "fully in: the shot's layers");
    assert_eq!(held.look.diffusion, base.look.diffusion, "groups the shot doesn't set stay the world's");
    tod.seek_cutscene(0.5);
    let easing = tod.resolve(Vec3::ZERO, 0.0).look.gradients;
    assert!(easing != shot_gradients && easing != base.look.gradients, "easing in");
    tod.stop_cutscene();
    assert_eq!(tod.resolve(Vec3::ZERO, 0.0).look.gradients, base.look.gradients);
}

#[test]
fn the_default_day_fogs_every_hour_and_keeps_characters_readable() {
    let day = default_day();
    for step in 0..48 {
        let hours = step as f32 * 0.5;
        let fog = day.evaluate(hours).fog;
        assert!(fog.is_visible(), "{hours}: some fog at every hour");
        assert!((0.35..=0.55).contains(&fog.characters), "{hours}: characters take part of it");
        // The far layer tints the sky's horizon: near white, so the distance melts into the sky.
        let tint = fog.far.color;
        assert!(tint.min_element() >= 0.8 && tint.max_element() <= 1.001, "{hours}: a tint, {tint}");
    }
}

#[test]
fn fog_colors_blend_by_density_in_oklab() {
    // Morning mist in a valley, then a clear noon with only the distance's haze.
    let mist = FogKey {
        far: FogLayerKey { color: [1.0; 3], density: 0.002, start: 30.0 },
        height: HeightFogKey { color: [1.0, 0.6, 0.4], density: 0.04, falloff: 0.3, base: 0.0 },
        characters: 0.6,
        ..FogKey::default()
    };
    let noon = FogKey {
        far: FogLayerKey { color: [0.8, 0.9, 1.0], density: 0.004, start: 50.0 },
        characters: 0.2,
        ..FogKey::default()
    };
    let timeline = TodTimeline::new(vec![
        TodKey { fog: Some(mist), ..key(6.0) },
        TodKey { fog: Some(noon), ..key(12.0) },
    ])
    .expect("valid");
    let halfway = timeline.evaluate(9.0).fog;
    // A layer only one side has keeps its color as it thins away, never darkening toward the
    // other side's unused black.
    assert!(halfway.height.color.abs_diff_eq(Vec3::new(1.0, 0.6, 0.4), 1e-5), "{}", halfway.height.color);
    assert!((halfway.height.density - 0.02).abs() < 1e-7);
    // Both sides have the far layer: their colors blend weighted by density, here two thirds of
    // the way to the denser noon's.
    let lab = |color: Vec3| Vec3::from(linear_srgb_to_oklab(color.to_array()));
    let expected = lab(Vec3::ONE).lerp(lab(Vec3::new(0.8, 0.9, 1.0)), 2.0 / 3.0);
    assert!(lab(halfway.far.color).abs_diff_eq(expected, 1e-4), "{}", halfway.far.color);
    assert!((halfway.far.density - 0.003).abs() < 1e-7 && (halfway.far.start - 40.0).abs() < 1e-5);
    assert!((halfway.characters - 0.4).abs() < 1e-6, "the characters' share blends linearly");
    // Exact on the keys.
    assert_eq!(timeline.evaluate(6.0).fog.height.color, Vec3::new(1.0, 0.6, 0.4));
    assert_eq!(timeline.evaluate(12.0).fog.far.color, Vec3::new(0.8, 0.9, 1.0));
}

#[test]
fn zones_and_cutscenes_override_the_fog() {
    let (_, mut tod) = time_of_day(DAY);
    let base = *tod.resolve(Vec3::ZERO, 0.0);
    // A misty valley: thick height fog lying below the camera, characters taking less of it.
    let valley = FogKey {
        far: FogLayerKey { color: [0.9, 0.95, 1.0], density: 0.006, start: 10.0 },
        height: HeightFogKey { color: [0.8, 0.85, 0.9], density: 0.08, falloff: 0.5, base: -2.0 },
        characters: 0.3,
        ..FogKey::default()
    };
    let valley_key = TodKey { fog: Some(valley), ..key(0.0) };
    let expected = TodTimeline::new(vec![valley_key.clone()]).expect("valid").evaluate(0.0).fog;
    let shape = VolumeShape::Sphere { center: Vec3::ZERO, radius: 10.0 };
    let zone_id = tod.add_volume(TodVolume::new(shape, zone(valley_key.clone())));
    let inside = *tod.resolve(Vec3::ZERO, 0.0);
    assert_eq!(inside.look.fog, expected, "inside the zone: its fog");
    assert_eq!(inside.look.sky, base.look.sky, "groups the zone doesn't set stay the base's");
    let outside = *tod.resolve(Vec3::new(100.0, 0.0, 0.0), 0.0);
    assert_eq!(outside.look.fog, base.look.fog, "outside: the day's");
    tod.remove_volume(zone_id);
    // A shot's fog, eased in and out like its other values, and what the renderer reads.
    let shot = TodCutscene::new(4.0, 1.0, 1.0, None, vec![valley_key], None).expect("valid");
    tod.play_cutscene(Arc::new(shot));
    tod.seek_cutscene(2.0);
    let mut packet = FramePacket::default();
    tod.resolve(Vec3::ZERO, 0.0).write(&mut packet);
    assert_eq!(packet.fog, expected, "fully in: the shot's fog, in the frame packet");
    tod.seek_cutscene(0.5);
    let easing = tod.resolve(Vec3::ZERO, 0.0).look.fog;
    assert!(easing != expected && easing != base.look.fog, "easing in");
    tod.stop_cutscene();
    assert_eq!(tod.resolve(Vec3::ZERO, 0.0).look.fog, base.look.fog);
}

#[test]
fn probe_keys_follow_the_hour_and_cutscenes() {
    let day = default_day();
    let baked: Vec<f32> = day.keys().iter().map(|key| key.time).collect();
    let weights = |keys: sapphire_render::ProbeKeys| -> Vec<(u32, f32)> {
        keys.keys
            .iter()
            .zip(keys.weights)
            .filter(|(_, weight)| *weight > 0.0)
            .map(|(&key, w)| (key, w))
            .collect()
    };
    // On a key: that key alone.
    let (_, mut tod) = time_of_day(DAY);
    tod.resolve(Vec3::ZERO, 0.0);
    assert_eq!(weights(tod.probe_keys(&baked)), [(3, 1.0)], "10:00 is the day key");
    // Between keys baked at the timeline's own: the timeline's eased weight, so the bounce moves
    // with the look.
    let (_, mut tod) = time_of_day(12.0);
    tod.resolve(Vec3::ZERO, 0.0);
    let segment = day.segment(12.0);
    assert_eq!(
        weights(tod.probe_keys(&baked)),
        [(segment.from as u32, 1.0 - segment.weight), (segment.to as u32, segment.weight)]
    );
    // Other baked hours: linear in the hour, wrapping at midnight.
    let (_, mut tod) = time_of_day(23.0);
    tod.resolve(Vec3::ZERO, 0.0);
    assert_eq!(
        weights(tod.probe_keys(&[6.0, 20.0])),
        [(1, 0.7), (0, 0.3)],
        "three of the ten hours from 20:00 to 6:00"
    );
    assert_eq!(weights(tod.probe_keys(&[12.0])), [(0, 1.0)], "one key holds all day");
    assert_eq!(weights(tod.probe_keys(&[])), [], "no grid, no keys");
    // A cutscene at its own hour crossfades to that hour's keys by its weight.
    let shot = TodCutscene::new(
        4.0,
        1.0,
        1.0,
        Some(ShotTime { hours: 18.75, hours_per_second: 0.0 }),
        vec![key(0.0)],
        None,
    )
    .expect("valid");
    let (_, mut tod) = time_of_day(DAY);
    tod.play_cutscene(Arc::new(shot));
    tod.seek_cutscene(0.5);
    tod.resolve(Vec3::ZERO, 0.0);
    let weight = tod.stats().cutscene_weight;
    assert!(weight > 0.0 && weight < 1.0);
    let keys = weights(tod.probe_keys(&baked));
    assert_eq!(keys, [(3, 1.0 - weight), (6, weight)], "the day key, and the sunset key");
    assert!((keys.iter().map(|(_, w)| w).sum::<f32>() - 1.0).abs() < 1e-6);
}

/// The default day with the world's emission keyed for lanterns and lit windows (M1 W1's keyed
/// emission): full through the night and the blue hour, dimming after sunrise to a tenth by day,
/// coming back from golden hour.
fn lantern_day() -> TodTimeline {
    let scales = [1.0, 1.0, 0.4, 0.1, 0.1, 0.25, 0.6, 1.0];
    let day = default_day();
    let keys =
        day.keys().iter().zip(scales).map(|(key, scale)| TodKey { emission: Some(scale), ..key.clone() });
    TodTimeline::new(keys.collect()).expect("valid")
}

#[test]
fn timelines_without_an_emission_scale_show_emission_as_authored() {
    let old = TodTimeline::from_ron(TIMELINE_BEFORE_SKY_LAYERS).expect("an older document loads");
    let day = default_day();
    for timeline in [&old, &day] {
        assert!(timeline.is_complete(), "it can still be the base timeline");
        assert!(!timeline.groups().contains(TodGroup::Emission));
        for quarter in 0..96 {
            assert_eq!(timeline.evaluate(quarter as f32 / 4.0).emission, 1.0, "{}", quarter as f32 / 4.0);
        }
    }
    // Through the resolve, as the packet's default draws it.
    let (_, mut tod) = time_of_day(22.0);
    let mut packet = FramePacket { emission_scale: 0.5, ..FramePacket::default() };
    tod.resolve(Vec3::ZERO, 0.0).write(&mut packet);
    assert_eq!(packet.emission_scale, FramePacket::default().emission_scale);
    assert_eq!(packet.emission_scale, 1.0);
}

#[test]
fn the_emission_scale_is_keyed_like_any_value() {
    // Exactly each key's value on it, and between keys the first key's eased weight of the way
    // to the next, as every scalar blends.
    let keyed = lantern_day();
    assert!(keyed.is_complete() && keyed.groups().contains(TodGroup::Emission));
    for key in keyed.keys() {
        let expected = key.emission.expect("every key sets it");
        assert_eq!(keyed.evaluate(key.time).emission.to_bits(), expected.to_bits(), "{}", key.time);
    }
    for hours in [3.0, 6.0, 8.25, 12.0, 16.0, 18.0, 19.5, 22.0] {
        let Segment { from, to, weight } = keyed.segment(hours);
        let (a, b) = (keyed.keys()[from].emission.unwrap(), keyed.keys()[to].emission.unwrap());
        let evaluated = keyed.evaluate(hours).emission;
        assert!((evaluated - (a + (b - a) * weight)).abs() < 1e-6, "{hours}: {evaluated}");
        assert!((a.min(b)..=a.max(b)).contains(&evaluated), "{hours}: between its keys");
    }
    // Linear keys wrap at midnight like the rest: from 20:00's 1.0 to 6:00's 0.2 over ten hours.
    let wrapping = TodTimeline::new(vec![
        TodKey { emission: Some(0.2), ..key(6.0) },
        TodKey { emission: Some(1.0), ..key(20.0) },
    ])
    .expect("valid");
    assert!((wrapping.evaluate(1.0).emission - 0.6).abs() < 1e-6, "halfway through the night");
    assert!((wrapping.evaluate(4.0).emission - 0.36).abs() < 1e-6, "toward dawn");
    assert!((wrapping.evaluate(13.0).emission - 0.6).abs() < 1e-6, "halfway through the day");

    // Resolved into the frame packet, with every other value as the default day's.
    let clock = ScaledClock::new().new_local_clock();
    let mut tod = TimeOfDay::new(keyed, clock, DAY).expect("a complete base");
    let mut packet = FramePacket::default();
    tod.resolve(Vec3::ZERO, 0.0).write(&mut packet);
    assert_eq!(packet.emission_scale, 0.1, "dim by day");
    let (_, mut plain) = time_of_day(DAY);
    let mut expected = FramePacket::default();
    plain.resolve(Vec3::ZERO, 0.0).write(&mut expected);
    assert_eq!(packet, FramePacket { emission_scale: 0.1, ..expected });
    tod.clock_mut().set_hours(22.0);
    tod.resolve(Vec3::ZERO, 0.0).write(&mut packet);
    assert_eq!(packet.emission_scale, 1.0, "full at night");
}

#[test]
fn emission_scale_keys_round_trip_and_are_checked() {
    let day = lantern_day();
    let text = day.to_ron().expect("write");
    assert!(text.contains("emission: Some(0.1),"), "written beside the other groups");
    assert_eq!(TodTimeline::from_ron(&text).expect("read"), day);
    // Every key of a timeline sets it, or none does.
    let mut keys = day.keys().to_vec();
    keys[4].emission = None;
    let error = TodTimeline::new(keys).unwrap_err();
    assert!(matches!(error, TodError::MixedGroups { key: 4, .. }), "{error}");
    let negative = TodTimeline::new(vec![TodKey { emission: Some(-0.5), ..key(12.0) }]).unwrap_err();
    let problem = "must be finite and not negative";
    assert_eq!(negative, TodError::Invalid { key: 0, field: "emission", problem });
}

#[test]
fn zones_and_cutscenes_override_the_emission_scale() {
    let clock = ScaledClock::new().new_local_clock();
    let mut tod = TimeOfDay::new(lantern_day(), clock, DAY).expect("a complete base");
    let base = *tod.resolve(Vec3::ZERO, 0.0);
    assert_eq!(base.look.emission, 0.1);
    // A festival street whose lanterns burn all day: a zone that sets the emission alone.
    let festival = TodKey { emission: Some(1.0), ..key(0.0) };
    let shape = VolumeShape::Sphere { center: Vec3::ZERO, radius: 10.0 };
    let street = TodVolume { blend_distance: 4.0, ..TodVolume::new(shape, zone(festival.clone())) };
    let zone_id = tod.add_volume(street);
    let inside = *tod.resolve(Vec3::ZERO, 0.0);
    assert_eq!(inside.look.emission, 1.0, "inside the zone: its lanterns");
    assert_eq!(inside.look.sky, base.look.sky, "groups the zone doesn't set stay the base's");
    let edge = tod.resolve(Vec3::new(12.0, 0.0, 0.0), 0.0).look.emission;
    assert!(edge > 0.1 && edge < 1.0, "blending out over its edge: {edge}");
    assert_eq!(tod.resolve(Vec3::new(100.0, 0.0, 0.0), 0.0).look.emission, 0.1, "outside: the day's");
    tod.remove_volume(zone_id);
    // A shot that lights them, eased in like its other values, and what the renderer reads.
    let shot = TodCutscene::new(4.0, 1.0, 1.0, None, vec![festival], None).expect("valid");
    tod.play_cutscene(Arc::new(shot));
    tod.seek_cutscene(2.0);
    let mut packet = FramePacket::default();
    tod.resolve(Vec3::ZERO, 0.0).write(&mut packet);
    assert_eq!(packet.emission_scale, 1.0, "fully in: the shot's, in the frame packet");
    tod.seek_cutscene(0.5);
    let easing = tod.resolve(Vec3::ZERO, 0.0).look.emission;
    assert!(easing > 0.1 && easing < 1.0, "easing in: {easing}");
    tod.stop_cutscene();
    assert_eq!(tod.resolve(Vec3::ZERO, 0.0).look.emission, 0.1);
}

/// A cool blue-violet hue for characters' shadow sides (M1 C8).
const SHADE_HUE: [f32; 3] = [0.55, 0.6, 1.0];

/// The default day with characters' shadow hue keyed (M1 C8): a share of it through the night,
/// little by day, most at golden hour, when the key is warmest.
fn shaded_day() -> TodTimeline {
    let shares = [0.6, 0.6, 0.3, 0.0, 0.0, 0.5, 0.8, 0.6];
    let day = default_day();
    let keys = day.keys().iter().zip(shares).map(|(key, share)| TodKey {
        character_shade: Some(CharacterShadeKey { hue: SHADE_HUE, share }),
        ..key.clone()
    });
    TodTimeline::new(keys.collect()).expect("valid")
}

/// A character's lighting by the shipped default rig, seen from the front, at `frame`.
fn character_lighting(frame: &TodFrame) -> sapphire_render::CharacterLighting {
    let camera =
        sapphire_render::CameraData::look_at(Vec3::new(0.0, 1.5, 3.0), Vec3::Y, Vec3::Y, 0.8, 1.0, 0.1);
    frame.character_lighting(&crate::light_rig::LightRig::default(), &camera, Vec3::new(0.0, 1.5, 0.0))
}

#[test]
fn timelines_without_a_shadow_hue_light_shadow_sides_by_the_key() {
    let old = TodTimeline::from_ron(TIMELINE_BEFORE_SKY_LAYERS).expect("an older document loads");
    let day = default_day();
    for timeline in [&old, &day] {
        assert!(timeline.is_complete(), "it can still be the base timeline");
        assert!(!timeline.groups().contains(TodGroup::CharacterShade));
        for quarter in 0..96 {
            let hours = quarter as f32 / 4.0;
            assert_eq!(timeline.evaluate(hours).character_shade, CharacterShade::default(), "{hours}");
        }
    }
    // A character's lighting is the rig's, its shadow side the key light's.
    let (_, mut tod) = time_of_day(18.0);
    let frame = *tod.resolve(Vec3::ZERO, 0.0);
    let lighting = character_lighting(&frame);
    assert_eq!(lighting.shade, CharacterShade::default());
    assert_eq!(lighting.shade.share, 0.0);
}

#[test]
fn the_shadow_hue_is_keyed_like_any_value() {
    // Exactly each key's value on it, and between keys the first key's eased weight of the way
    // to the next: the share linearly, the hue exactly itself where both keys hold it.
    let keyed = shaded_day();
    assert!(keyed.is_complete() && keyed.groups().contains(TodGroup::CharacterShade));
    for key in keyed.keys() {
        let expected = key.character_shade.expect("every key sets it");
        let shade = keyed.evaluate(key.time).character_shade;
        assert_eq!(shade.share.to_bits(), expected.share.to_bits(), "{}", key.time);
        assert_eq!(shade.hue, Vec3::from(expected.hue), "{}", key.time);
    }
    for hours in [3.0, 6.0, 8.25, 12.0, 16.0, 18.0, 19.5, 22.0] {
        let Segment { from, to, weight } = keyed.segment(hours);
        let share = |index: usize| keyed.keys()[index].character_shade.unwrap().share;
        let (a, b) = (share(from), share(to));
        let evaluated = keyed.evaluate(hours).character_shade;
        assert!((evaluated.share - (a + (b - a) * weight)).abs() < 1e-6, "{hours}: {evaluated:?}");
        assert_eq!(evaluated.hue, Vec3::from(SHADE_HUE), "{hours}");
    }
    // Hues blend in OkLab, and linear keys wrap at midnight like the rest: from 20:00's to
    // 6:00's over ten hours.
    let (warm, cool) = ([1.0, 0.6, 0.3], [0.3, 0.4, 1.0]);
    let wrapping = TodTimeline::new(vec![
        TodKey { character_shade: Some(CharacterShadeKey { hue: warm, share: 0.2 }), ..key(6.0) },
        TodKey { character_shade: Some(CharacterShadeKey { hue: cool, share: 1.0 }), ..key(20.0) },
    ])
    .expect("valid");
    let night = wrapping.evaluate(1.0).character_shade;
    assert!((night.share - 0.6).abs() < 1e-6, "halfway through the night: {night:?}");
    let halfway = sapphire_math::color::mix_oklab(Vec3::from(cool), Vec3::from(warm), 0.5);
    assert!(night.hue.abs_diff_eq(halfway, 1e-6), "{night:?}");
    assert!((wrapping.evaluate(13.0).character_shade.share - 0.6).abs() < 1e-6, "halfway through the day");

    // Characters take it through their lighting, whatever their rig.
    let clock = ScaledClock::new().new_local_clock();
    let mut tod = TimeOfDay::new(keyed, clock, 18.0).expect("a complete base");
    let frame = *tod.resolve(Vec3::ZERO, 0.0);
    assert!(frame.look.character_shade.share > 0.5, "golden hour: {:?}", frame.look.character_shade);
    assert_eq!(character_lighting(&frame).shade, frame.look.character_shade);
    // Nothing else changes: the frame packet and the lighting are the default day's.
    let (_, mut plain) = time_of_day(18.0);
    let plain = *plain.resolve(Vec3::ZERO, 0.0);
    let (mut packet, mut expected) = (FramePacket::default(), FramePacket::default());
    frame.write(&mut packet);
    plain.write(&mut expected);
    assert_eq!(packet, expected);
    let lighting = character_lighting(&frame);
    assert_eq!(
        sapphire_render::CharacterLighting { shade: CharacterShade::default(), ..lighting },
        character_lighting(&plain)
    );
}

#[test]
fn shadow_hue_keys_round_trip_and_are_checked() {
    let day = shaded_day();
    let text = day.to_ron().expect("write");
    assert!(text.contains("character_shade: Some("), "written beside the other groups");
    assert_eq!(TodTimeline::from_ron(&text).expect("read"), day);
    // Every key of a timeline sets it, or none does.
    let mut keys = day.keys().to_vec();
    keys[4].character_shade = None;
    let error = TodTimeline::new(keys).unwrap_err();
    assert!(matches!(error, TodError::MixedGroups { key: 4, .. }), "{error}");
    let shaded = |hue, share| TodKey { character_shade: Some(CharacterShadeKey { hue, share }), ..key(12.0) };
    let over = TodTimeline::new(vec![shaded(SHADE_HUE, 1.5)]).unwrap_err();
    let problem = "must be within 0..=1";
    assert_eq!(over, TodError::Invalid { key: 0, field: "character_shade.share", problem });
    let black = TodTimeline::new(vec![shaded([0.0; 3], 0.5)]).unwrap_err();
    let problem = "must not be black";
    assert_eq!(black, TodError::Invalid { key: 0, field: "character_shade.hue", problem });
}

#[test]
fn zones_and_cutscenes_override_the_shadow_hue() {
    let clock = ScaledClock::new().new_local_clock();
    let mut tod = TimeOfDay::new(default_day(), clock, DAY).expect("a complete base");
    // A grotto whose shadows go teal: a zone that sets the shadow hue alone.
    let grotto =
        TodKey { character_shade: Some(CharacterShadeKey { hue: [0.2, 0.8, 0.7], share: 0.7 }), ..key(0.0) };
    let shape = VolumeShape::Sphere { center: Vec3::ZERO, radius: 10.0 };
    let zone_id =
        tod.add_volume(TodVolume { blend_distance: 4.0, ..TodVolume::new(shape, zone(grotto.clone())) });
    let inside = *tod.resolve(Vec3::ZERO, 0.0);
    assert_eq!(inside.look.character_shade.share, 0.7, "inside the zone");
    assert_eq!(character_lighting(&inside).shade, inside.look.character_shade);
    let edge = tod.resolve(Vec3::new(12.0, 0.0, 0.0), 0.0).look.character_shade.share;
    assert!(edge > 0.0 && edge < 0.7, "blending out over its edge: {edge}");
    assert_eq!(tod.resolve(Vec3::new(100.0, 0.0, 0.0), 0.0).look.character_shade, CharacterShade::default());
    tod.remove_volume(zone_id);
    // A shot that sets it, eased in like its other values.
    let shot = TodCutscene::new(4.0, 1.0, 1.0, None, vec![grotto], None).expect("valid");
    tod.play_cutscene(Arc::new(shot));
    tod.seek_cutscene(2.0);
    assert_eq!(tod.resolve(Vec3::ZERO, 0.0).look.character_shade.share, 0.7, "fully in");
    tod.seek_cutscene(0.5);
    let easing = tod.resolve(Vec3::ZERO, 0.0).look.character_shade.share;
    assert!(easing > 0.0 && easing < 0.7, "easing in: {easing}");
}
