//! Kept cascades (M5 D8, `CascadeCache`): a cascade that isn't drawn again holds the depth that
//! drawing it would write. Two renderers see the same frames, one drawing in turns as before and
//! one keeping its cascades; wherever the two hold a cascade under the same fit, their maps agree
//! texel for texel, over frames where a caster moves and stops, frames are lost, and the view
//! moves and the cascades refit.

use super::*;
use sapphire_render::CascadeCache;

/// Desktop-High's cascades at 1024, kept or drawn in turns, without spots.
fn cached(cache: CascadeCache) -> RenderSettings {
    let shadows = ShadowSettings { resolution: 1024, spots: 0, cache, ..ShadowSettings::default() };
    RenderSettings { shadows: Some(shadows), ..RenderSettings::default() }
}

/// A ground and a box in each cascade's reach, seen along the ground; the box at 30 m is the one
/// that moves, returned with its mesh.
fn street(settings: RenderSettings) -> (Harness, MeshInstance) {
    let mut harness = Harness::new(settings);
    harness.ground();
    let mut mover = None;
    for (index, z) in [-3.0, -12.0, -30.0, -75.0].into_iter().enumerate() {
        let size = Vec3::new(1.0, 2.0, 1.0) * (1.0 + index as f32);
        harness.cuboid(Vec3::new(1.5, size.y * 0.5, z), size);
        if z == -30.0 {
            mover = harness.added.last().copied();
        }
    }
    harness.sun_toward(toward(40.0, 30.0));
    harness.camera = view_from(0.0);
    (harness, mover.expect("the box at 30 m"))
}

/// The street's view from `x` meters across.
fn view_from(x: f32) -> CameraData {
    looking(Vec3::new(x, 2.0, 6.0), Vec3::new(x, 1.0, -20.0))
}

/// What one frame does to both renderers.
#[derive(Clone, Copy, Debug)]
enum Step {
    Still,
    /// The box at 30 m moves to `x` across.
    Move(f32),
    /// The view moves to `x` across.
    Walk(f32),
    /// The frame is lost before its passes run, after the step.
    Lost(Loss, &'static Step),
}

#[test]
#[ignore = "needs a GPU"]
fn kept_cascades_hold_what_drawing_in_turns_draws_under_the_same_fits() {
    use Step::{Lost, Move, Still, Walk};
    let script = [
        Still,
        Still,
        Still,
        Move(2.5),
        Still,
        Still,
        Move(3.5),
        Move(4.5),
        Still,
        Still,
        Lost(Loss::Dropped, &Move(1.5)),
        Still,
        Still,
        Still,
        Walk(0.3),
        Walk(0.6),
        Walk(0.9),
        Still,
        Still,
        Lost(Loss::Unrecorded, &Walk(1.4)),
        Still,
        Still,
        Still,
        Lost(Loss::Rejected, &Still),
        Still,
        Still,
    ];
    let mut turns = street(cached(CascadeCache::Turns));
    let mut kept = street(cached(CascadeCache::Kept));
    let (mut compared, mut agreeing, mut drawn) = (0, 0, [0u32; 2]);
    let mut since_loss = usize::MAX;
    let mut held: Vec<([f32; 8], Vec<f32>)> = Vec::new();
    for (frame, step) in script.iter().enumerate() {
        let apply = |(harness, mover): &mut (Harness, MeshInstance), step: &Step| match *step {
            Move(x) => {
                let size = Vec3::new(3.0, 6.0, 3.0);
                let place =
                    Mat4::from_scale_rotation_translation(size, Quat::IDENTITY, Vec3::new(x, 3.0, -30.0));
                harness.added.push(MeshInstance { transform: place, ..*mover });
            }
            Walk(x) => harness.camera = view_from(x),
            Still | Lost(..) => {}
        };
        if let Lost(loss, inner) = step {
            for street in [&mut turns, &mut kept] {
                apply(street, inner);
                street.0.lose(*loss);
            }
            since_loss = 0;
            continue;
        }
        since_loss = since_loss.saturating_add(1);
        let frames = [&mut turns, &mut kept].map(|street| {
            apply(street, step);
            let frame = street.0.render();
            (frame, street.0.renderer.shadow_cascades())
        });
        let [(turns_frame, turns_maps), (kept_frame, kept_maps)] = &frames;
        drawn[0] += turns_frame.stats.shadows.cascades_drawn;
        drawn[1] += kept_frame.stats.shadows.cascades_drawn;
        let mut agree = true;
        for (index, ((fit, map), kept_cascade)) in turns_maps.iter().zip(kept_maps).enumerate() {
            let same = |a: &[f32], b: &[f32]| a.iter().zip(b).all(|(a, b)| a.to_bits() == b.to_bits());
            if *fit == kept_cascade.0 && same(map, &kept_cascade.1) {
                compared += 1;
                continue;
            }
            agree = false;
            // Drawing in turns draws every cascade again after a lost frame. A kept one holds what
            // it held, under the fit it held it under, until its turn: never what the lost frame
            // would have drawn. Two frames on, each has had its turn.
            let kept_as_it_was = held.get(index) == Some(kept_cascade);
            assert!(
                since_loss <= 2 && kept_as_it_was,
                "frame {frame} ({step:?}), cascade {index}: the maps differ {since_loss} frames after a loss \
                 (fits equal: {}, kept as it was: {kept_as_it_was}); turns {:?}, kept {:?}",
                *fit == kept_cascade.0,
                turns_frame.stats.shadows,
                kept_frame.stats.shadows,
            );
        }
        if agree {
            agreeing += 1;
            assert_eq!(differing(turns_frame, kept_frame), 0, "frame {frame} ({step:?}): pixels differing");
        }
        held.clone_from(kept_maps);
    }
    let frames = script.iter().filter(|step| !matches!(step, Lost(..))).count();
    let losses = script.len() - frames;
    assert!(agreeing + 2 * losses >= frames, "{agreeing} of {frames} frames agree");
    assert!(compared >= 4 * agreeing, "{compared} cascades compared");
    assert!(drawn[1] * 2 < drawn[0], "kept draws fewer cascades: {drawn:?}");
}
