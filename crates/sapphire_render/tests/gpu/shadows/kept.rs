//! Kept cascades (M5 D8, `CascadeCache`): a cascade that isn't drawn again holds the depth that
//! drawing it would write. Two renderers see the same frames, one drawing in turns as before and
//! one keeping its cascades; wherever the two hold a cascade under the same fit, their maps agree
//! texel for texel, over frames where a caster moves and stops, one leaves and another takes its
//! slot, frames are lost, and the view moves and the cascades refit.

use super::*;
use sapphire_render::CascadeCache;

/// Desktop-High's cascades at 1024, kept or drawn in turns, without spots.
fn cached(cache: CascadeCache) -> RenderSettings {
    let shadows = ShadowSettings { resolution: 1024, spots: 0, cache, ..ShadowSettings::default() };
    RenderSettings { shadows: Some(shadows), ..RenderSettings::default() }
}

/// A ground and a box in each cascade's reach, seen along the ground: the box at 30 m is the one
/// that moves, and the one at 75 m the one that leaves and comes back as another.
struct Street {
    harness: Harness,
    mover: MeshInstance,
    far: InstanceId,
}

fn street(settings: RenderSettings) -> Street {
    let mut harness = Harness::new(settings);
    harness.ground();
    let mut boxes = Vec::new();
    for (index, z) in [-3.0, -12.0, -30.0, -75.0].into_iter().enumerate() {
        let size = Vec3::new(1.0, 2.0, 1.0) * (1.0 + index as f32);
        harness.cuboid(Vec3::new(1.5, size.y * 0.5, z), size);
        boxes.extend(harness.added.last().copied());
    }
    harness.sun_toward(toward(40.0, 30.0));
    harness.camera = view_from(0.0);
    Street { harness, mover: boxes[2], far: boxes[3].id }
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
    /// The box at 75 m leaves, and another of its size takes its slot `x` across.
    Swap(f32),
    /// The view moves to `x` across.
    Walk(f32),
    /// The frame is lost before its passes run, after the step.
    Lost(Loss, &'static Step),
}

impl Street {
    fn apply(&mut self, step: &Step) {
        match *step {
            Step::Move(x) => {
                let size = Vec3::new(3.0, 6.0, 3.0);
                let place =
                    Mat4::from_scale_rotation_translation(size, Quat::IDENTITY, Vec3::new(x, 3.0, -30.0));
                self.harness.added.push(MeshInstance { transform: place, ..self.mover });
            }
            Step::Swap(x) => {
                self.harness.removed.push(self.far);
                self.far = self.harness.cuboid(Vec3::new(x, 4.0, -75.0), Vec3::new(4.0, 8.0, 4.0));
            }
            Step::Walk(x) => self.harness.camera = view_from(x),
            Step::Still | Step::Lost(..) => {}
        }
    }
}

const SCRIPT: [Step; 33] = {
    use Step::{Lost, Move, Still, Swap, Walk};
    [
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
        Swap(2.5),
        Still,
        Still,
        Lost(Loss::Empty, &Swap(1.5)),
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
    ]
};

/// Plays [`SCRIPT`] to a renderer drawing in turns and one with `cache`. Wherever the two hold a
/// cascade under the same fit, their maps agree texel for texel, but for two frames after a lost
/// frame, while a kept cascade holds what it held, under the fit it held it under, until its turn
/// (drawing in turns draws every cascade again after a loss). Returns the frames where every
/// cascade agrees and the cascades each drew.
fn against_turns(cache: CascadeCache) -> (usize, [u32; 2]) {
    let mut turns = street(cached(CascadeCache::Turns));
    let mut kept = street(cached(cache));
    let (mut agreeing, mut drawn) = (0, [0u32; 2]);
    let mut since_loss = usize::MAX;
    let mut before: Vec<([f32; 8], Vec<f32>)> = Vec::new();
    for (frame, step) in SCRIPT.iter().enumerate() {
        if let Step::Lost(loss, inner) = step {
            for street in [&mut turns, &mut kept] {
                street.apply(inner);
                street.harness.lose(*loss);
            }
            since_loss = 0;
            continue;
        }
        since_loss = since_loss.saturating_add(1);
        let frames = [&mut turns, &mut kept].map(|street| {
            street.apply(step);
            let frame = street.harness.render();
            (frame, street.harness.renderer.shadow_cascades())
        });
        let [(turns_frame, turns_maps), (kept_frame, kept_maps)] = &frames;
        drawn[0] += turns_frame.stats.shadows.cascades_drawn;
        drawn[1] += kept_frame.stats.shadows.cascades_drawn;
        let mut agree = true;
        for (index, ((fit, map), kept_cascade)) in turns_maps.iter().zip(kept_maps).enumerate() {
            let same = |a: &[f32], b: &[f32]| a.iter().zip(b).all(|(a, b)| a.to_bits() == b.to_bits());
            if *fit == kept_cascade.0 && same(map, &kept_cascade.1) {
                continue;
            }
            agree = false;
            let as_it_was = before.get(index) == Some(kept_cascade);
            // Held (`CascadeCache::Held`), a cached cascade's fit trails a moving view's.
            let trailing = cache == CascadeCache::Held && *fit != kept_cascade.0;
            assert!(
                trailing || (since_loss <= 2 && as_it_was),
                "{cache:?}, frame {frame} ({step:?}), cascade {index}: the maps differ {since_loss} frames after \
                 a loss (fits equal: {}, as it was: {as_it_was}); turns {:?}, kept {:?}",
                *fit == kept_cascade.0,
                turns_frame.stats.shadows,
                kept_frame.stats.shadows,
            );
        }
        if agree {
            agreeing += 1;
            assert_eq!(
                differing(turns_frame, kept_frame),
                0,
                "{cache:?}, frame {frame} ({step:?}): pixels differ"
            );
        }
        before.clone_from(kept_maps);
    }
    (agreeing, drawn)
}

#[test]
#[ignore = "needs a GPU"]
fn kept_cascades_hold_what_drawing_in_turns_draws_under_the_same_fits() {
    let frames = SCRIPT.iter().filter(|step| !matches!(step, Step::Lost(..))).count();
    let losses = SCRIPT.len() - frames;
    let (agreeing, drawn) = against_turns(CascadeCache::Kept);
    assert!(agreeing + 2 * losses >= frames, "{agreeing} of {frames} frames agree");
    assert!(drawn[1] * 2 < drawn[0], "kept draws fewer cascades: {drawn:?}");
    // Held, the frames still from the start agree whole; a held fit moves pixels only once the view
    // has moved.
    let (agreeing, held) = against_turns(CascadeCache::Held);
    assert!(agreeing >= 10, "{agreeing} of {frames} frames agree");
    assert!(held[1] <= drawn[1], "held draws no more than kept: {held:?} against {drawn:?}");
}
