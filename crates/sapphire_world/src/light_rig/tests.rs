//! The character light rig: presets as documents, the key's limits, time of day's curves, and
//! per-shot overrides.

use std::sync::Arc;

use sapphire_math::Vec3;
use sapphire_math::color::luminance;
use sapphire_render::{CameraData, CharacterGi, CharacterLight, CharacterLighting, DirectionalLight};
use sapphire_serialize::DocumentError;
use sapphire_time::ScaledClock;

use super::*;
use crate::tod::{TimeOfDay, TodCutscene, TodTimeline};

/// The character's head, which the cameras look at.
const HEAD: Vec3 = Vec3::Y;

/// A camera at `eye` looking at the head, as the sandbox frames a character.
fn camera_at(eye: Vec3) -> CameraData {
    CameraData::look_at(eye, HEAD, Vec3::Y, 0.8, 16.0 / 9.0, 0.1)
}

/// The sandbox's camera: on +Z, a little to the right and above.
fn front_camera() -> CameraData {
    camera_at(Vec3::new(1.2, 1.6, 3.4))
}

/// The unit vector toward a light that travels along `direction`.
fn toward(direction: Vec3) -> Vec3 {
    (-direction).normalize()
}

/// `toward`'s elevation above the horizon, in degrees.
fn elevation(direction: Vec3) -> f32 {
    toward(direction).y.asin().to_degrees()
}

/// `toward`'s azimuth around the head from the camera's side, in degrees, positive toward its
/// right.
fn azimuth(direction: Vec3, camera: &CameraData) -> f32 {
    let frame = CameraYaw::of(camera, HEAD);
    frame.angles(toward(direction)).0
}

fn sun_toward(azimuth_degrees: f32, elevation_degrees: f32, camera: &CameraData) -> DirectionalLight {
    let direction = -CameraYaw::of(camera, HEAD).toward(azimuth_degrees, elevation_degrees);
    DirectionalLight { direction, color: Vec3::new(1.0, 0.9, 0.8), intensity: 1.0 }
}

fn gameplay() -> LightRig {
    *LightRigLibrary::default_rigs().rig(LightRigId::DEFAULT)
}

fn portrait() -> LightRig {
    let rigs = LightRigLibrary::default_rigs();
    *rigs.rig(rigs.find("portrait").expect("shipped"))
}

#[test]
fn the_shipped_presets_are_valid_and_round_trip() {
    let rigs = LightRigLibrary::default_rigs();
    assert_eq!(rigs.names().collect::<Vec<_>>(), ["gameplay", "portrait"]);
    assert_eq!(rigs.find("gameplay"), Some(LightRigId::DEFAULT));
    assert_eq!(rigs.find("cinematic"), None);
    assert_eq!(rigs.name(LightRigId(1)), Some("portrait"));
    assert_eq!(rigs.rig(LightRigId(7)), rigs.rig(LightRigId::DEFAULT), "past the end: the default");
    let text = rigs.to_ron().expect("write");
    assert!(
        text.starts_with("(\n    type: \"sapphire::world::LightRigLibrary\",\n    version: 1,\n"),
        "{text}"
    );
    assert_eq!(LightRigLibrary::from_ron(&text).expect("read"), rigs);
    let gameplay = gameplay();
    assert_eq!((gameplay.key.min_elevation, gameplay.key.max_azimuth), (5.0, 120.0));
    assert_eq!(gameplay.gi.max_value, Some(0.5));
    assert_eq!(portrait().key.camera_follow, 1.0);
}

#[test]
fn documents_leave_out_what_is_neutral_and_are_checked() {
    let read = |rigs: &str| {
        LightRigLibrary::from_ron(&format!(
            r#"(type: "sapphire::world::LightRigLibrary", version: 1, data: (rigs: [{rigs}]))"#
        ))
    };
    let partial = read(r#"(name: "warm", rig: (key: (tint: (1.0, 0.8, 0.6)), rim: 0.5))"#).expect("valid");
    let warm = partial.rig(LightRigId::DEFAULT);
    assert_eq!(warm.key.tint, [1.0, 0.8, 0.6]);
    assert_eq!(warm.rim, 0.5);
    assert_eq!(
        (warm.key.max_azimuth, warm.gi, warm.local_light_ceiling),
        (180.0, LightRigGi::default(), 2.0),
        "the rest neutral"
    );
    let invalid = |rigs: &str| match read(rigs) {
        Err(DocumentError::Parse(error)) => error.to_string(),
        other => panic!("expected a validation error, got {other:?}"),
    };
    assert!(invalid("").contains("no presets"));
    assert!(invalid(r#"(name: "", rig: ())"#).contains("name can't be empty"));
    assert!(invalid(r#"(name: "a", rig: ()), (name: "a", rig: ())"#).contains("`a` appears twice"));
    let backwards = invalid(r#"(name: "a", rig: (key: (min_elevation: 30.0, max_elevation: 10.0)))"#);
    assert!(backwards.contains("`key.max_elevation` must be within `min_elevation`"), "{backwards}");
    assert!(invalid(r#"(name: "a", rig: (key: (max_azimuth: 200.0)))"#).contains("key.max_azimuth"));
    assert!(invalid(r#"(name: "a", rig: (gi: (max_saturation: 1.5)))"#).contains("gi.max_saturation"));
    assert!(invalid(r#"(name: "a", rig: (gi: (max_value: Some(-1.0))))"#).contains("gi.max_value"));
    assert!(invalid(r#"(name: "a", rig: (key: (camera_follow: -0.1)))"#).contains("key.camera_follow"));
    assert!(invalid(r#"(name: "a", rig: (key: (tnit: (1.0, 1.0, 1.0))))"#).contains("tnit"), "typos fail");
}

#[test]
fn the_neutral_rig_is_the_world_key_light_exactly() {
    let camera = front_camera();
    let sun = DirectionalLight { direction: Vec3::new(0.55, -0.7, -0.45), color: Vec3::ONE, intensity: 1.0 };
    let character = CharacterLight { color: Vec3::new(1.0, 0.97, 0.92), intensity: 1.0 };
    let head = Vec3::new(0.0, 1.67, 0.0);
    let lighting = LightRig::default().resolve(&sun, &character, &camera, head);
    assert_eq!(
        lighting,
        CharacterLighting {
            key: DirectionalLight { direction: sun.direction, color: character.color, intensity: 1.0 },
            gi: CharacterGi { head: Some(head), ..CharacterGi::default() },
            ..CharacterLighting::unstyled(sun)
        }
    );
    // The default day's 10:00 sun is within the gameplay preset's limits too: unchanged, bit for
    // bit, so the default look is the world's light.
    let direction = gameplay().key.direction(sun.direction, &camera, head);
    assert_eq!(direction.to_array().map(f32::to_bits), sun.direction.to_array().map(f32::to_bits));
}

#[test]
fn the_key_stays_within_its_limits() {
    let camera = front_camera();
    let key = gameplay().key;
    let resolve = |azimuth_degrees, elevation_degrees| {
        key.direction(sun_toward(azimuth_degrees, elevation_degrees, &camera).direction, &camera, HEAD)
    };
    // Inside the limits: where the sun is.
    let inside = resolve(-70.0, 45.0);
    assert!((azimuth(inside, &camera) + 70.0).abs() < 1e-3 && (elevation(inside) - 45.0).abs() < 1e-3);
    // A grazing sun is raised to the floor, an overhead one lowered to the ceiling, and each
    // keeps its azimuth.
    let low = resolve(30.0, 1.0);
    assert!((elevation(low) - 5.0).abs() < 1e-3, "{}", elevation(low));
    assert!((azimuth(low, &camera) - 30.0).abs() < 1e-3);
    let high = resolve(-20.0, 85.0);
    assert!((elevation(high) - 65.0).abs() < 1e-3);
    assert!((azimuth(high, &camera) + 20.0).abs() < 1e-3);
    // A sun behind the character folds back from 120° toward the camera's side, on its own
    // side: halfway from 120° to 180° is halfway back to 0°.
    for (side, expected) in [(120.0, 120.0), (150.0, 60.0), (-165.0, -30.0), (180.0, 0.0)] {
        let behind = resolve(side, 30.0);
        let found = azimuth(behind, &camera);
        assert!((found - expected).abs() < 1e-3, "{side}: {found}");
        assert!((elevation(behind) - 30.0).abs() < 1e-3);
    }
    // So a sun passing behind the character turns the key smoothly through the camera's side
    // instead of jumping from one side of the face to the other.
    let mut previous: Option<Vec3> = None;
    for step in 0..=400 {
        let side = 100.0 + step as f32 * 0.4;
        let side = if side > 180.0 { side - 360.0 } else { side };
        let key = toward(resolve(side, 20.0));
        if let Some(previous) = previous {
            let turned = previous.dot(key).clamp(-1.0, 1.0).acos().to_degrees();
            assert!(turned < 1.0, "{side}°: the key turned {turned}° in one 0.4° step");
        }
        previous = Some(key);
    }
    // As the camera orbits round to face the sun, the key comes round with it.
    let sun = sun_toward(0.0, 30.0, &camera);
    let facing_sun = camera_at(Vec3::new(-1.2, 1.6, -3.4));
    assert!(azimuth(key.direction(sun.direction, &facing_sun, HEAD), &facing_sun).abs() < 1e-3);
    // The camera's side is judged from the head: a character at the edge of the screen, where
    // the camera's own axis would put the sun at 120°, is lit as a backlit one.
    let off_center = HEAD + CameraYaw::of(&camera, HEAD).right * 2.0;
    let to_the_side = CameraYaw::of(&camera, off_center);
    let edge_azimuth = |direction: Vec3| to_the_side.angles(toward(direction)).0;
    let sun = DirectionalLight { direction: -to_the_side.toward(150.0, 20.0), ..sun };
    let folded = key.direction(sun.direction, &camera, off_center);
    assert!((edge_azimuth(folded) - 60.0).abs() < 1e-3, "{}", edge_azimuth(folded));
    // A camera straight above the head looks at it along its own axis: looking straight down,
    // the screen's bottom edge is the camera's side, and looking straight up, its top edge.
    let above = CameraData::look_at(HEAD + Vec3::new(0.0, 6.0, 0.0), HEAD, Vec3::NEG_Z, 0.8, 1.0, 0.1);
    assert!(CameraYaw::of(&above, HEAD).back.abs_diff_eq(Vec3::Z, 1e-6));
    let below = CameraData::look_at(HEAD - Vec3::new(0.0, 6.0, 0.0), HEAD, Vec3::NEG_Z, 0.8, 1.0, 0.1);
    assert!(CameraYaw::of(&below, HEAD).back.abs_diff_eq(Vec3::NEG_Z, 1e-6));
}

#[test]
fn a_camera_fixed_key_ignores_the_sun() {
    let rig = portrait();
    let character = CharacterLight::default();
    let head = Vec3::Y;
    for eye in [Vec3::new(0.0, 1.2, 4.0), Vec3::new(3.0, 2.0, -2.0), Vec3::new(-4.0, 0.5, 0.5)] {
        let camera = camera_at(eye);
        for sun in [sun_toward(150.0, 10.0, &camera), sun_toward(-40.0, 60.0, &camera)] {
            let key = rig.resolve(&sun, &character, &camera, head).key;
            assert!((azimuth(key.direction, &camera) + 35.0).abs() < 1e-3, "{eye}");
            assert!((elevation(key.direction) - 30.0).abs() < 1e-3, "{eye}");
        }
    }
    // Halfway: halfway along the great circle from the sun to the camera's key.
    let camera = front_camera();
    let sun = sun_toward(60.0, 20.0, &camera);
    let half = LightRig { key: LightRigKey { camera_follow: 0.5, ..rig.key }, ..rig };
    let direction = half.resolve(&sun, &character, &camera, head).key.direction;
    let fixed = CameraYaw::of(&camera, head).toward(-35.0, 30.0);
    let angle = |a: Vec3, b: Vec3| a.dot(b).clamp(-1.0, 1.0).acos();
    let (from_sun, to_fixed) =
        (angle(toward(sun.direction), toward(direction)), angle(toward(direction), fixed));
    assert!((from_sun - to_fixed).abs() < 1e-4, "{from_sun} vs {to_fixed}");
}

#[test]
fn the_key_takes_time_of_days_character_curve() {
    let mut world = ScaledClock::new();
    let clock = world.new_local_clock();
    let mut tod = TimeOfDay::new(TodTimeline::default_day(), clock, 0.0).expect("complete");
    let rig = LightRig {
        key: LightRigKey { tint: [1.0, 0.5, 0.25], intensity: 2.0, ..LightRigKey::default() },
        ..gameplay()
    };
    let camera = front_camera();
    for hours in [0.0, 6.5, 10.0, 18.5, 21.0] {
        tod.clock_mut().set_hours(hours);
        let frame = *tod.resolve(Vec3::ZERO, 0.0);
        let curve = frame.look.character_light;
        let key = frame.character_lighting(&rig, &camera, Vec3::Y).key;
        assert_eq!(key.color, curve.color * Vec3::new(1.0, 0.5, 0.25), "{hours}");
        assert_eq!(key.intensity, curve.intensity * 2.0, "{hours}");
    }
    // At midnight characters get a key of their own a little brighter than the moon gives the
    // world (in luminance, about 1.25 times: the default day's night lift, `tod`'s tests).
    tod.clock_mut().set_hours(0.0);
    let frame = *tod.resolve(Vec3::ZERO, 0.0);
    let key = frame.character_lighting(&gameplay(), &camera, Vec3::Y).key;
    let moon = frame.look.key_light();
    let (character, moonlight) =
        (luminance(key.color) * key.intensity, luminance(moon.color) * moon.intensity);
    assert!(character > moonlight * 1.15, "{character} vs the moon's {moonlight}");
}

#[test]
fn a_shot_rig_blends_in_and_out_with_its_cutscene() {
    let mut world = ScaledClock::new();
    let clock = world.new_local_clock();
    let mut tod = TimeOfDay::new(TodTimeline::default_day(), clock, 10.0).expect("complete");
    let camera = camera_at(Vec3::new(-3.0, 1.5, 1.0));
    let head = Vec3::new(0.0, 1.6, 0.0);
    let own = *tod.resolve(Vec3::ZERO, 0.0);
    let gameplay = gameplay();
    let before = own.character_lighting(&gameplay, &camera, head);
    let shot_rig = portrait();
    let shot = TodCutscene::new(2.0, 0.5, 0.5, None, Vec::new(), Some(shot_rig)).expect("a rig alone");
    let in_shot = shot_rig.resolve(&own.look.key_light(), &own.look.character_light, &camera, head);
    tod.play_cutscene(Arc::new(shot));
    let angle = |a: Vec3, b: Vec3| toward(a).dot(toward(b)).clamp(-1.0, 1.0).acos();
    let total = angle(before.key.direction, in_shot.key.direction);
    assert!(total > 0.5, "the two rigs light from different sides: {total}");
    for (seconds, weight) in [(0.25, None), (1.0, Some(1.0)), (1.75, None), (2.5, Some(0.0))] {
        tod.seek_cutscene(seconds);
        let frame = *tod.resolve(Vec3::ZERO, 0.0);
        let track_weight = tod.stats().cutscene_weight;
        if let Some(weight) = weight {
            assert_eq!(track_weight, weight, "{seconds}");
        }
        let lighting = frame.character_lighting(&gameplay, &camera, head);
        if track_weight <= 0.0 {
            assert_eq!((frame.shot_rig, lighting), (None, before), "{seconds}: its own rig");
        } else if track_weight >= 1.0 {
            assert_eq!(lighting, in_shot, "{seconds}: the shot's rig");
        } else {
            // Along the great circle by the track's weight.
            let turned = angle(before.key.direction, lighting.key.direction);
            let expected = track_weight * total;
            assert!((turned - expected).abs() < 1e-3, "{seconds}: {turned} of {total} at {track_weight}");
            let bias =
                before.gi.camera_bias + (in_shot.gi.camera_bias - before.gi.camera_bias) * track_weight;
            assert!((lighting.gi.camera_bias - bias).abs() < 1e-6);
        }
    }
}

#[test]
fn shot_rigs_are_checked_and_written() {
    let bad = LightRig { rim: -1.0, ..LightRig::default() };
    let error = TodCutscene::new(1.0, 0.0, 0.0, None, Vec::new(), Some(bad)).unwrap_err();
    assert_eq!(error.to_string(), "cutscene: `light_rig.rim` must be finite and not negative");
    let shot = TodCutscene::new(1.0, 0.0, 0.0, None, Vec::new(), Some(portrait())).expect("valid");
    let text = shot.to_ron().expect("write");
    assert!(text.contains("light_rig: Some("), "{text}");
    assert_eq!(TodCutscene::from_ron(&text).expect("read"), shot);
}
