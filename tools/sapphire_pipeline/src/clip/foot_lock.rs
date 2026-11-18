//! Foot locking after a retarget (M6 A4): each planted foot held where it landed, by a two-bone solve
//! on its leg, so a clip carried onto other proportions doesn't slide its feet.
//!
//! # Why a retarget slides
//!
//! A retarget turns every bone as the source's turns and moves the hips by the legs' ratio
//! ([`retarget`](super::retarget)'s "Translations"). That holds a planted foot still only where the
//! rigs are the same shape below the hips and the source's foot is still: a wider stance, a longer
//! foot or a socket sliding across the source's hips moves the target's foot while the source's
//! stays, and whatever the source lets its foot drift the ratio scales up. KayKit's clips drift a
//! good deal by A1's report (11 cm through `Walking_B`'s stance, 30 cm through a slash), so the
//! shipped retargets slid from 5 mm to half a meter before this pass.
//!
//! # What holds still
//!
//! A foot is planted where [`contacts`](super::contacts) finds it planted on the retargeted clip,
//! which is what its report measures: each sole's runs, against the ground the clip carries (a clip
//! that runs in place moves its ground under its feet). From each tick to the next, one sole planted
//! on both holds still across the ground, the one whose run goes on longest, and the whole foot
//! moves with it; another sole planted on both keeps its bearing from that one, the foot turning
//! about +Y as much as the retarget would have swung it. That's a correction for each tick, an
//! ankle's move across the ground and a turn, that adds up from where the run lands. The sole that
//! isn't holding still moves only as the foot rolls about the one that is, as the source's does:
//! a toe landing that drops its heel through A1's 3 cm band moves the heel's horizontal place by the
//! foot's pitch, which the report counts as slide (about 4 cm on the warrior's long feet walking
//! backward). [`held_runs`] measures the sole that holds instead.
//!
//! A run that starts the clip holds from its first tick and one that ends it from its last, so those
//! ticks keep their poses and a loop its seam (only a foot planted from the first tick to the last
//! moves its last); any other holds halfway across the range its correction would take, so the leg
//! moves least. The correction fades in before a run lands and out after it lifts, over [`BLEND`],
//! eased, and within the gap to the next run or the clip's end.
//!
//! Locking moves the leg, so the contacts found again afterwards differ a little (an edge tick, the
//! ground's speed under a clip that runs in place): it runs again on what it made, up to [`PASSES`]
//! times, until no correction is over a tenth of a millimeter.
//!
//! # The solve
//!
//! Only the leg's three joints turn: the upper leg, the lower leg, and the foot, which keeps its turn
//! in model space but for the correction's turn about +Y, so the sole's roll and the toes stay as the
//! retarget had them. The knee bends in its own plane until the hip-to-ankle span is the one wanted,
//! then the leg swings about the hip onto it. A leg reaches no further than [`STRAIGHT`] of its
//! length, or as far as the retarget already had it, and never bends the other way, so a foot out of
//! reach goes as near as the leg reaches, on the line from the hip, so it stops short and a little
//! high (`Hit_A` on the character lifts its held foot up to 1.5 cm as the body sways back, though
//! it holds across the ground). Heights are otherwise the
//! retarget's: a correction is across the ground only, so grounding's work stands.
//!
//! **Nothing else moves:** the hips, root motion and every joint above the legs keep their bits, so
//! the tables a move bakes from these samples (root motion and paths on the arms and hands) are what
//! they were.
//!
//! In scalars and `det`, as everything on a clip's path is ([`clip`](super)'s "Byte identity").

use std::cmp::Reverse;

use glam::{Quat, Vec3};
use sapphire_assets::rig::HumanBone;
use sapphire_assets::skeleton::{NO_PARENT, Skeleton};
use sapphire_math::det;

use super::contacts::{ContactSettings, PlantedFoot, PlantedRun, Planting, planting};
use super::report::Positions;
use super::retarget::Rig;
use super::{SampledClip, rest_rotation};

/// How long a correction takes to fade out after a foot lifts, or in before it lands, in seconds:
/// see the [module docs](self).
pub const BLEND: f32 = 0.1;

/// The share of a leg's length its solve reaches at most: see the [module docs](self).
pub const STRAIGHT: f32 = 0.999;

/// A leg's joints, and both hips, which say which way is across the body.
struct Leg {
    upper: usize,
    lower: usize,
    foot: usize,
    left_hip: usize,
    right_hip: usize,
}

impl Leg {
    /// `foot`'s leg on `rig`, if the map names all of it as a chain.
    fn of(rig: Rig<'_>, foot: HumanBone) -> Option<Self> {
        use HumanBone::{LeftFoot, LeftLowerLeg, LeftUpperLeg, RightLowerLeg, RightUpperLeg};
        let (upper, lower) =
            if foot == LeftFoot { (LeftUpperLeg, LeftLowerLeg) } else { (RightUpperLeg, RightLowerLeg) };
        let joint = |bone| rig.map.joint(bone).map(usize::from);
        let leg = Self {
            upper: joint(upper)?,
            lower: joint(lower)?,
            foot: joint(foot)?,
            left_hip: joint(LeftUpperLeg)?,
            right_hip: joint(RightUpperLeg)?,
        };
        let skeleton = rig.skeleton;
        (below(skeleton, leg.lower, leg.upper) && below(skeleton, leg.foot, leg.lower)).then_some(leg)
    }
}

/// Whether `joint` hangs somewhere below `above`.
fn below(skeleton: &Skeleton, joint: usize, above: usize) -> bool {
    let mut at = joint;
    while skeleton.parents[at] != NO_PARENT {
        at = usize::from(skeleton.parents[at]);
        if at == above {
            return true;
        }
    }
    false
}

/// Holds each planted foot of `sampled` still, on `rig`, as `settings` find its contacts: see the
/// [module docs](self). `sampled` should be a retarget's, on `rig`'s joints.
pub fn lock_feet(sampled: &mut SampledClip, rig: Rig<'_>, settings: &ContactSettings) {
    for _ in 0..PASSES {
        if lock_once(sampled, rig, settings) < SETTLED {
            break;
        }
    }
}

/// How many times locking runs at most: see the [module docs](self).
pub const PASSES: usize = 4;

/// A correction small enough that locking stops, in meters: the largest an ankle moves in a pass.
const SETTLED: f32 = 1e-4;

/// One pass of [`lock_feet`]: the largest correction it made, in meters.
fn lock_once(sampled: &mut SampledClip, rig: Rig<'_>, settings: &ContactSettings) -> f32 {
    let skeleton = rig.skeleton;
    let positions = Positions::of(sampled, skeleton);
    let planting = planting(sampled, skeleton, rig.map, &positions, settings);
    let rate = f32::from(sampled.rate);
    let blend = ((BLEND * rate) as usize).max(1);
    let mut largest = 0.0f32;
    for foot in &planting.feet {
        let Some(leg) = Leg::of(rig, foot.foot) else { continue };
        let corrections = corrections(&planting, foot, rate, blend);
        largest = corrections.iter().map(|correction| correction.across.length()).fold(largest, det::max);
        if corrections.iter().all(|correction| *correction == Correction::NONE) {
            continue;
        }
        for joint in [leg.upper, leg.lower, leg.foot] {
            ensure_track(sampled, skeleton, joint);
        }
        for (tick, correction) in corrections.iter().enumerate() {
            if *correction != Correction::NONE {
                // Across the ground is across the model, and a turn about +Y the same turn: root
                // motion only turns about +Y.
                let across = sampled.root.as_ref().map_or(correction.across, |root| {
                    det::quat_rotate(det::quat_conjugate(root.rotation[tick]), correction.across)
                });
                place(sampled, skeleton, &leg, &positions, tick, Correction { across, ..*correction });
            }
        }
    }
    largest
}

/// What locking does to a foot at a tick: moves its ankle across the ground and turns the foot about
/// +Y, in meters and radians.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Correction {
    across: Vec3,
    turn: f32,
}

impl Correction {
    const NONE: Self = Self { across: Vec3::ZERO, turn: 0.0 };

    fn scaled(self, share: f32) -> Self {
        Self { across: self.across * share, turn: self.turn * share }
    }
}

/// A turn about +Y, then a move: what a run does to its foot across the ground, by tick.
#[derive(Clone, Copy)]
struct Hold {
    turn: f32,
    moved: Vec3,
}

impl Hold {
    const NONE: Self = Self { turn: 0.0, moved: Vec3::ZERO };

    fn apply(self, at: Vec3) -> Vec3 {
        det::rotate_y(at, self.turn) + self.moved
    }

    /// `self`, after a turn by `turn` and a move by `moved`.
    fn then(self, turn: f32, moved: Vec3) -> Self {
        Self { turn: self.turn + turn, moved: det::rotate_y(self.moved, turn) + moved }
    }
}

/// The midpoint of the range `values` take.
fn halfway(values: impl Iterator<Item = f32>) -> f32 {
    let (low, high) = values
        .fold((f32::INFINITY, f32::NEG_INFINITY), |(low, high), at| (det::min(low, at), det::max(high, at)));
    (low + high) * 0.5
}

/// How `foot` moves across the ground and turns at each tick, to hold still where it's planted: see
/// the [module docs](self).
fn corrections(planting: &Planting, foot: &PlantedFoot, rate: f32, blend: usize) -> Vec<Correction> {
    let count = foot.foot_planted.len();
    let ground = on_ground(planting, foot, rate);
    let held_by = Anchors::of(foot);
    // The hold at `tick`, from the one at `tick - 1`: the sole planted across the step whose run goes
    // on longest stays where it was, and another planted across it keeps its bearing from that one.
    let step = |hold: Hold, tick: usize| {
        let Some(anchor) = held_by.at(tick) else {
            return hold;
        };
        let bearing =
            |sole: usize, tick: usize| det::yaw_of_direction(ground[sole][tick] - ground[anchor][tick]);
        let turn = (0..foot.planted.len())
            .filter(|&sole| sole != anchor && held_by.planted_across(sole, tick))
            .map(|sole| -det::wrap_angle(bearing(sole, tick) - bearing(sole, tick - 1)))
            .next()
            .unwrap_or(0.0);
        let turned = Hold { turn: hold.turn + turn, moved: Vec3::ZERO };
        let moved = hold.apply(ground[anchor][tick - 1]) - turned.apply(ground[anchor][tick]);
        Hold { moved, ..turned }
    };

    let held: Vec<bool> = (0..count).map(|tick| foot.planted.iter().any(|sole| sole[tick])).collect();
    let runs = runs(&held);
    let mut correction = vec![Correction::NONE; count];
    let mut holds = vec![Hold::NONE; count];
    for &(first, last) in &runs {
        holds[first] = Hold::NONE;
        for tick in first + 1..=last {
            holds[tick] = step(holds[tick - 1], tick);
        }
        // Where the foot holds: at the clip's first or last tick where the run has one, so those keep
        // their poses, or else halfway between the turns and the ground it would have slid, so it
        // moves least.
        let run = &holds[first..=last];
        let turn = if first == 0 {
            0.0
        } else if last == count - 1 {
            -run[run.len() - 1].turn
        } else {
            -halfway(run.iter().map(|hold| hold.turn))
        };
        let across = |hold: Hold, tick: usize| {
            let ankle = ground[0][tick];
            hold.then(turn, Vec3::ZERO).apply(ankle) - ankle
        };
        let moved = if first == 0 {
            Vec3::ZERO
        } else if last == count - 1 {
            -across(run[run.len() - 1], last)
        } else {
            let slid: Vec<Vec3> = run.iter().zip(first..).map(|(hold, tick)| across(*hold, tick)).collect();
            -Vec3::new(halfway(slid.iter().map(|at| at.x)), 0.0, halfway(slid.iter().map(|at| at.z)))
        };
        for (tick, hold) in (first..=last).zip(run) {
            let hold = hold.then(turn, moved);
            correction[tick] =
                Correction { across: hold.apply(ground[0][tick]) - ground[0][tick], turn: hold.turn };
        }
    }

    // The gaps: before the first run, between two, after the last.
    let mut gaps: Vec<(Option<usize>, Option<usize>)> = Vec::with_capacity(runs.len() + 1);
    let mut before = None;
    for &(first, last) in &runs {
        gaps.push((before, Some(first)));
        before = Some(last);
    }
    gaps.push((before, None));
    for (lifted, lands) in gaps {
        let (from, to) = (lifted.map_or(0, |tick| tick + 1), lands.unwrap_or(count));
        let span = match (lifted, lands) {
            (Some(lifted), Some(lands)) => lands - lifted,
            (Some(lifted), None) => count - 1 - lifted,
            (None, Some(lands)) => lands,
            (None, None) => 0,
        }
        .min(blend);
        for tick in from..to {
            let out = lifted
                .map_or(Correction::NONE, |lifted| correction[lifted].scaled(fade(tick - lifted, span)));
            let into =
                lands.map_or(Correction::NONE, |lands| correction[lands].scaled(fade(lands - tick, span)));
            correction[tick] = Correction { across: out.across + into.across, turn: out.turn + into.turn };
        }
    }
    correction
}

/// Each sole of `foot` across the ground, tick by tick, its height dropped.
fn on_ground(planting: &Planting, foot: &PlantedFoot, rate: f32) -> Vec<Vec<Vec3>> {
    foot.paths
        .iter()
        .map(|path| {
            (0..path.len())
                .map(|tick| {
                    let at = planting.on_ground(path, tick, rate);
                    Vec3::new(at.x, 0.0, at.z)
                })
                .collect()
        })
        .collect()
}

/// Which sole of a foot holds still from one tick to the next: see the [module docs](self).
struct Anchors<'a> {
    planted: &'a [Vec<bool>],
    /// Each tick of a sole's run, the run's last tick.
    ends: Vec<Vec<usize>>,
}

impl<'a> Anchors<'a> {
    fn of(foot: &'a PlantedFoot) -> Self {
        Self { planted: &foot.planted, ends: foot.planted.iter().map(|sole| run_ends(sole)).collect() }
    }

    fn planted_across(&self, sole: usize, tick: usize) -> bool {
        self.planted[sole][tick - 1] && self.planted[sole][tick]
    }

    /// The sole that holds from `tick - 1` to `tick`: of those planted on both, the one whose run goes
    /// on longest, the first on a tie; none when no sole is planted on both.
    fn at(&self, tick: usize) -> Option<usize> {
        (0..self.planted.len())
            .filter(|&sole| self.planted_across(sole, tick))
            .max_by_key(|&sole| (self.ends[sole][tick], Reverse(sole)))
    }
}

/// How far each run of `sampled`'s planted feet strays across the ground at the sole that holds it,
/// on `rig`, as `settings` find its contacts: a foot's held point, the sole locking keeps still, from
/// one tick to the next (see the [module docs](self)), its moves added up from where the run lands.
///
/// It differs from A1's slide ([`contacts`](super::contacts)'s "Slide") only where a second sole is
/// planted too: A1's counts that sole's own travel, which on a foot rolling about the held one (a toe
/// landing dropping the heel through the 3 cm band, a heel strike lowering the toes) is the roll, not
/// a slide. Each run is reported as [`PlantedRun`]'s are, the foot's ticks planted on either sole.
pub fn held_runs(sampled: &SampledClip, rig: Rig<'_>, settings: &ContactSettings) -> Vec<PlantedRun> {
    let positions = Positions::of(sampled, rig.skeleton);
    let planting = planting(sampled, rig.skeleton, rig.map, &positions, settings);
    let rate = f32::from(sampled.rate);
    let mut held = Vec::new();
    for foot in &planting.feet {
        let ground = on_ground(&planting, foot, rate);
        let held_by = Anchors::of(foot);
        let planted: Vec<bool> =
            (0..foot.foot_planted.len()).map(|tick| foot.planted.iter().any(|sole| sole[tick])).collect();
        for (first, last) in runs(&planted) {
            let (mut strayed, mut slide) = (Vec3::ZERO, 0.0f32);
            for tick in first + 1..=last {
                if let Some(sole) = held_by.at(tick) {
                    strayed += ground[sole][tick] - ground[sole][tick - 1];
                    slide = det::max(slide, strayed.length());
                }
            }
            held.push(PlantedRun { foot: foot.foot, first: first as u16, last: last as u16, slide });
        }
    }
    held
}

/// How much of a correction is left `ticks` from its run, fading over `span`: eased, 1 at the run,
/// 0 from `span` on.
fn fade(ticks: usize, span: usize) -> f32 {
    if ticks >= span {
        return 0.0;
    }
    let share = ticks as f32 / span as f32;
    1.0 - share * share * (3.0 - 2.0 * share)
}

/// The first and last tick of each run of `planted`.
fn runs(planted: &[bool]) -> Vec<(usize, usize)> {
    let mut runs = Vec::new();
    let mut start = None;
    for tick in 0..=planted.len() {
        match (start, planted.get(tick).copied().unwrap_or(false)) {
            (None, true) => start = Some(tick),
            (Some(first), false) => {
                runs.push((first, tick - 1));
                start = None;
            }
            _ => {}
        }
    }
    runs
}

/// For each tick of a run of `planted`, the run's last tick; 0 elsewhere.
fn run_ends(planted: &[bool]) -> Vec<usize> {
    let mut ends = vec![0; planted.len()];
    for (first, last) in runs(planted) {
        ends[first..=last].fill(last);
    }
    ends
}

/// Gives `joint` a rotation track at its rest, if it has none.
fn ensure_track(sampled: &mut SampledClip, skeleton: &Skeleton, joint: usize) {
    let joint16 = joint as u16;
    if let Err(at) = sampled.rotations.binary_search_by_key(&joint16, |(joint, _)| *joint) {
        let rest = rest_rotation(skeleton, joint);
        sampled.rotations.insert(at, (joint16, vec![rest; usize::from(sampled.ticks) + 1]));
    }
}

/// `joint`'s local rotation at `tick`, normalized.
fn local(sampled: &SampledClip, skeleton: &Skeleton, joint: usize, tick: usize) -> Quat {
    match sampled.rotations.binary_search_by_key(&(joint as u16), |(joint, _)| *joint) {
        Ok(track) => det::quat_normalize(sampled.rotations[track].1[tick]),
        Err(_) => rest_rotation(skeleton, joint),
    }
}

/// `joint`'s rotation in model space at `tick`, from normalized local rotations.
fn model(sampled: &SampledClip, skeleton: &Skeleton, joint: usize, tick: usize) -> Quat {
    let mut chain = vec![joint];
    while let Some(&at) = chain.last() {
        match skeleton.parents[at] {
            NO_PARENT => break,
            parent => chain.push(usize::from(parent)),
        }
    }
    chain
        .iter()
        .rev()
        .fold(Quat::IDENTITY, |model, &joint| det::quat_mul(model, local(sampled, skeleton, joint, tick)))
}

fn set_local(sampled: &mut SampledClip, joint: usize, tick: usize, rotation: Quat) {
    let track = sampled.rotations.binary_search_by_key(&(joint as u16), |(joint, _)| *joint);
    let track = track.expect("`ensure_track` gave each leg joint a track");
    sampled.rotations[track].1[tick] = det::quat_normalize(rotation);
}

/// Moves `leg`'s foot at `tick` by `across`, in model space, turning only the leg: see the
/// [module docs](self)' "The solve".
fn place(
    sampled: &mut SampledClip,
    skeleton: &Skeleton,
    leg: &Leg,
    positions: &Positions,
    tick: usize,
    correction: Correction,
) {
    let at = |joint| positions.at(tick, joint);
    let (hip, knee, ankle) = (at(leg.upper), at(leg.lower), at(leg.foot));
    let left = at(leg.left_hip) - at(leg.right_hip);
    let wanted = ankle + correction.across;
    let Some((swing, bend)) = solve(hip, knee, ankle, wanted, left) else { return };

    let parent = |joint: usize| usize::from(skeleton.parents[joint]);
    let (upper_parent, lower_parent, foot_parent) = (parent(leg.upper), parent(leg.lower), parent(leg.foot));
    let rotation = |joint| model(sampled, skeleton, joint, tick);
    let (above_upper, upper) = (rotation(upper_parent), rotation(leg.upper));
    let (above_lower, lower) = (rotation(lower_parent), rotation(leg.lower));
    let (above_foot, foot) = (rotation(foot_parent), rotation(leg.foot));
    // The upper leg swings; the lower leg bends under it; the foot keeps its turn in model space, and
    // turns about +Y by the correction's.
    let upper_local = det::quat_mul(det::quat_conjugate(above_upper), det::quat_mul(swing, upper));
    let lower_local = det::quat_mul(det::quat_conjugate(above_lower), det::quat_mul(bend, lower));
    let moved_above_foot = det::quat_mul(det::quat_mul(swing, bend), above_foot);
    let foot = det::quat_mul(det::quat_from_rotation_y(correction.turn), foot);
    let foot_local = det::quat_mul(det::quat_conjugate(moved_above_foot), foot);
    set_local(sampled, leg.upper, tick, upper_local);
    set_local(sampled, leg.lower, tick, lower_local);
    set_local(sampled, leg.foot, tick, foot_local);
}

/// The swing about the hip and the bend at the knee, both in model space, that take a leg at `hip`,
/// `knee` and `ankle` as near `wanted` as it reaches. `left` points from the right hip to the left,
/// for a leg too straight to say which way its knee bends. `None` for a leg with no length.
fn solve(hip: Vec3, knee: Vec3, ankle: Vec3, wanted: Vec3, left: Vec3) -> Option<(Quat, Quat)> {
    let (thigh, shin) = (knee.distance(hip), ankle.distance(knee));
    let span = wanted - hip;
    let distance = span.length();
    if thigh <= 0.0 || shin <= 0.0 || distance <= 0.0 || left.length() <= 0.0 {
        return None;
    }
    // Out of reach, the ankle goes as near where it's wanted as the leg reaches: along the line from
    // the hip, so a straightening leg lifts it a little.
    let reach = det::max((thigh + shin) * STRAIGHT, ankle.distance(hip));
    let distance_wanted = det::max(det::min(distance, reach), (thigh - shin).abs());
    let cosine = (thigh * thigh + shin * shin - distance_wanted * distance_wanted) / (2.0 * thigh * shin);
    let angle = det::acos(det::max(det::min(cosine, 1.0), -1.0));

    let (to_hip, to_ankle) = (hip - knee, ankle - knee);
    let normal = to_hip.cross(to_ankle);
    // A knee bends forward, which turns the shin about the body's left-to-right axis.
    let axis = if normal.length() > 1e-4 * thigh * shin { normal.normalize() } else { -left.normalize() };
    let bend = det::quat_from_axis_angle(axis, angle - det::angle_between(to_hip, to_ankle));
    let bent = knee + det::quat_rotate(bend, to_ankle);
    let swing = det::quat_from_rotation_arc((bent - hip).normalize(), span / distance);
    Some((swing, bend))
}

#[cfg(test)]
mod tests;
