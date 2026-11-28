//! Foot contacts, found at cook (M6 A1): where each foot of a clip is planted, as the
//! [`ContactTrack`]s graph sync groups and footsteps read, and how far it slides while it is.
//!
//! # The travel
//!
//! A foot is judged where the clip carries it: its model-space position with the clip's root motion
//! applied, `root(t) × pose(t)`. A clip that runs in place (its root, if it has one, travels no
//! further than [`IN_PLACE`] across the ground; the controller moves it) carries the ground under its
//! feet instead, so a planted foot moves backward at about the run's speed. How fast is the
//! animator's: a stylized run's planted foot speeds up through its contact (KayKit's `Running_B` on
//! the character goes from 5.3 to 8.0 m/s in five ticks), so no one ground speed holds a foot still
//! against it to a few tenths of a meter a second. Such a clip's feet land by height and *vertical*
//! speed; its *ground velocity* is the median horizontal velocity, each axis on its own, of every sole
//! while it is low (below [`ContactSettings::height`]), which is what slide is measured against. A
//! clip whose root travels judges its feet by their whole speed, against a ground that stays put.
//!
//! # A foot's soles
//!
//! A foot is planted when its foot joint or its toes, where the rig maps them, is: a heel strike plants
//! the ankle and a toe-off the toes, and the other is off the ground for part of each. A sole's height
//! is measured from its height at rest, which stands on the ground, as retargeting's grounding does.
//!
//! # Detection, with hysteresis
//!
//! A sole lands on a tick when it is under [`ContactSettings::height`] above its rest and moves slower
//! than [`ContactSettings::speed`] (only up or down, in place); it lifts once either is over
//! [`RELEASE`] times that, so a foot near a threshold doesn't flicker. Tick 0 is judged by the landing
//! test. Then, for the foot (planted while either sole is): a lift between two contacts shorter than
//! [`ContactSettings::ticks`] closes, and a contact shorter than that inside the clip is dropped. A
//! contact touching either end is kept whatever its length, since the clip cuts it.
//!
//! # Slide
//!
//! For each run of ticks a sole is planted while its foot is, the furthest it strays across the ground
//! (horizontally, after the ground's own travel) from where the run began. A foot's slide is the worst
//! of its soles' runs. A planted foot that stays put slides 0; a retarget whose stride doesn't match its
//! hips' travel shows here.
//!
//! **What it misses: a fast skate reads as a lift.** In a clip whose root travels, a sole is planted
//! only while its whole speed stays under [`ContactSettings::speed`] (to land) and [`RELEASE`] times it
//! (to stay): 0.3 and 0.45 m/s by default. A planted foot skating faster near the ground has no run, so
//! no slide, and nothing that plants by these contacts holds it (the content check, the clip inspector,
//! M6 A4's foot lock). A clip that runs in place judges its soles by vertical speed alone, so its slides
//! are all counted, against the ground's travel.
//!
//! # Contacts are a simulation input, two ways
//!
//! - **At run time, through a sync group:** a graph's sync group counts its synced clips' cycles from a
//!   foot's marker, and the fixed step plays the graph of a fixed body, so a change here that moves a
//!   marker changes that body's simulation if its graph syncs (`sapphire_assets::clip`'s docs say
//!   which do, and what holds it).
//! - **At cook time, through the foot lock:** M6 A4's [`foot_lock`](super::foot_lock) plants every
//!   retarget's feet by `planting` and turns its legs to match, so a change to how this judges a sole
//!   (the settings' defaults, [`RELEASE`], `planting` itself) moves every retarget's leg tracks, synced
//!   or not. A fixed body that plays a retarget then samples other legs, as a move table baked from a
//!   retarget's legs would (none is, A4's As built).
//!
//! Either way a change here changes cooked clips' bytes, so it is a simulation change and bumps the clip
//! cooker's stage ([`COOKER`](super::COOKER), which the retarget's carries); a fixed body's fingerprint
//! takes its clips' bytes, so a recording made before it is refused rather than played differently.
//!
//! In scalars and `det`, as everything on a clip's path is, and deterministic: the same samples give
//! the same tracks on every architecture.

use glam::{Affine3, Vec3};
use sapphire_assets::clip::{ContactTrack, FootMarker, Marker};
use sapphire_assets::rig::HumanBone;
use sapphire_assets::skeleton::Skeleton;
use serde::{Deserialize, Serialize};

use super::SampledClip;
use super::report::Positions;
use super::retarget::HumanMap;

/// How far past a landing threshold a planted sole may go before it lifts: see the [module docs](self).
pub const RELEASE: f32 = 1.5;

/// How far across the ground a clip's root may travel and the clip still run in place, in meters: see
/// the [module docs](self).
pub const IN_PLACE: f32 = 0.01;

/// How a clip's contacts are found: see the [module docs](self). A sidecar's `contacts: (…)`, written
/// only when it isn't the default.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ContactSettings {
    /// How high above where it rests a sole may be and land, in meters.
    pub height: f32,
    /// How fast a sole may move and land, in meters a second: only up or down in a clip that runs in
    /// place.
    pub speed: f32,
    /// The fewest ticks a contact, or a lift between two, lasts.
    pub ticks: u16,
}

impl Default for ContactSettings {
    fn default() -> Self {
        Self { height: 0.03, speed: 0.3, ticks: 3 }
    }
}

impl ContactSettings {
    /// Whether the thresholds are finite and above 0.
    pub fn is_valid(&self) -> bool {
        self.height.is_finite() && self.height > 0.0 && self.speed.is_finite() && self.speed > 0.0
    }

    /// Whether these are the defaults, which a sidecar leaves out.
    pub fn is_default(&self) -> bool {
        *self == Self::default()
    }
}

/// One sole: a joint, and its height at rest.
struct Sole {
    joint: usize,
    rest_height: f32,
}

/// A run of ticks one sole is planted while its foot is, and how far it strays across it: a foot's
/// [`ContactTrack::slide`] is the worst of its runs'. Where a slide is, for the clip inspector and a
/// test that a slide is reported where it was put (M6 A3).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PlantedRun {
    pub foot: HumanBone,
    /// The run's first and last ticks.
    pub first: u16,
    pub last: u16,
    /// In meters, as the [module docs](self)' "Slide" measures it.
    pub slide: f32,
}

/// Each foot's contact track, left then right, for the feet `map` names: see the [module docs](self).
/// `positions` are `sampled`'s on `skeleton`.
pub fn contacts(
    sampled: &SampledClip,
    skeleton: &Skeleton,
    map: &HumanMap,
    positions: &Positions,
    settings: &ContactSettings,
) -> Vec<ContactTrack> {
    feet(sampled, skeleton, map, positions, settings).into_iter().map(|(track, _)| track).collect()
}

/// Every [`PlantedRun`] of the feet `map` names, left foot's first, each foot's soles in turn and each
/// sole's runs in order.
pub fn planted_runs(
    sampled: &SampledClip,
    skeleton: &Skeleton,
    map: &HumanMap,
    positions: &Positions,
    settings: &ContactSettings,
) -> Vec<PlantedRun> {
    feet(sampled, skeleton, map, positions, settings).into_iter().flat_map(|(_, runs)| runs).collect()
}

/// Where a clip's feet are planted, as its contacts and their slide are judged: what foot locking
/// (M6 A4) holds still.
pub(crate) struct Planting {
    /// How fast the ground moves under a clip that runs in place, in meters a second; zero under one
    /// whose root travels.
    pub ground: Vec3,
    /// Each foot the map names, left then right.
    pub feet: Vec<PlantedFoot>,
}

/// One foot of a [`Planting`].
pub(crate) struct PlantedFoot {
    pub foot: HumanBone,
    /// Its soles' joints: the foot, then its toes where the map names them.
    pub soles: Vec<usize>,
    /// Each sole's travel, tick by tick: where the clip carries it, root motion applied.
    pub paths: Vec<Vec<Vec3>>,
    /// Each sole's planted ticks, tick by tick: the ticks a sole is planted while its foot is, which
    /// its runs are.
    pub planted: Vec<Vec<bool>>,
    /// The foot's own: planted while either sole is, after [`ContactSettings::ticks`] tidies it.
    pub foot_planted: Vec<bool>,
}

impl Planting {
    /// Where a sole on `path` is at `tick`, against the ground, which has moved by
    /// [`ground`](Self::ground) since tick 0.
    pub fn on_ground(&self, path: &[Vec3], tick: usize, rate: f32) -> Vec3 {
        path[tick] - self.ground * (tick as f32 / rate)
    }
}

/// Each foot's contact track and its runs.
fn feet(
    sampled: &SampledClip,
    skeleton: &Skeleton,
    map: &HumanMap,
    positions: &Positions,
    settings: &ContactSettings,
) -> Vec<(ContactTrack, Vec<PlantedRun>)> {
    let planting = planting(sampled, skeleton, map, positions, settings);
    let rate = f32::from(sampled.rate);
    planting
        .feet
        .iter()
        .map(|foot| {
            let mut runs = Vec::new();
            for (path, sole) in foot.paths.iter().zip(&foot.planted) {
                // The run's first tick, where it began, and its slide so far.
                let mut run: Option<(usize, Vec3, f32)> = None;
                for tick in 0..=sole.len() {
                    if tick == sole.len() || !sole[tick] {
                        if let Some((first, _, slide)) = run.take() {
                            let (first, last) = (first as u16, (tick - 1) as u16);
                            runs.push(PlantedRun { foot: foot.foot, first, last, slide });
                        }
                        continue;
                    }
                    let here = planting.on_ground(path, tick, rate);
                    let (_, from, slide) = run.get_or_insert((tick, here, 0.0));
                    *slide = slide.max(Vec3::new(here.x - from.x, 0.0, here.z - from.z).length());
                }
            }
            let slide = runs.iter().map(|run| run.slide).fold(0.0f32, f32::max);
            let planted = &foot.foot_planted;
            let markers = (1..planted.len())
                .filter(|&tick| planted[tick] != planted[tick - 1])
                .map(|tick| FootMarker {
                    tick: tick as u16,
                    marker: if planted[tick] { Marker::Down } else { Marker::Up },
                })
                .collect();
            (
                ContactTrack {
                    foot: foot.foot,
                    joint: foot.soles[0] as u16,
                    planted: planted.first().copied().unwrap_or(false),
                    markers,
                    slide,
                },
                runs,
            )
        })
        .collect()
}

/// Where `sampled`'s feet are planted: see the [module docs](self).
pub(crate) fn planting(
    sampled: &SampledClip,
    skeleton: &Skeleton,
    map: &HumanMap,
    positions: &Positions,
    settings: &ContactSettings,
) -> Planting {
    let rest = super::rest_model(skeleton);
    let sole = |bone| {
        map.joint(bone).map(usize::from).map(|joint| Sole { joint, rest_height: rest[joint].translation.y })
    };
    let feet = [(HumanBone::LeftFoot, HumanBone::LeftToes), (HumanBone::RightFoot, HumanBone::RightToes)];
    let soles: Vec<(HumanBone, Vec<Sole>)> = feet
        .into_iter()
        .filter_map(|(foot, toes)| Some((foot, [sole(foot)?].into_iter().chain(sole(toes)).collect())))
        .collect();

    // Where each sole travels, tick by tick.
    let count = positions.ticks();
    let travel = |joint: usize| -> Vec<Vec3> {
        (0..count)
            .map(|tick| {
                let at = positions.at(tick, joint);
                sampled.root.as_ref().map_or(at, |root| {
                    Affine3::from_rotation_translation(root.rotation[tick], root.translation[tick])
                        .transform_point3(at)
                })
            })
            .collect()
    };
    let rate = f32::from(sampled.rate);
    let velocity = |path: &[Vec3], tick: usize| -> Vec3 {
        if path.len() < 2 {
            return Vec3::ZERO;
        }
        let (before, after) = (tick.saturating_sub(1), (tick + 1).min(path.len() - 1));
        (path[after] - path[before]) * (rate / (after - before) as f32)
    };
    let paths: Vec<Vec<(Vec<Vec3>, f32)>> = soles
        .iter()
        .map(|(_, soles)| soles.iter().map(|sole| (travel(sole.joint), sole.rest_height)).collect())
        .collect();

    let travels = sampled.root.as_ref().is_some_and(|root| {
        root.translation.iter().any(|at| {
            let from = root.translation[0];
            let (x, z) = (at.x - from.x, at.z - from.z);
            x * x + z * z > IN_PLACE * IN_PLACE
        })
    });
    let ground = if travels {
        Vec3::ZERO
    } else {
        let (mut across, mut along) = (Vec::new(), Vec::new());
        for (path, rest_height) in paths.iter().flatten() {
            for tick in 0..count {
                if path[tick].y - rest_height < settings.height {
                    let moving = velocity(path, tick);
                    across.push(moving.x);
                    along.push(moving.z);
                }
            }
        }
        Vec3::new(median(&mut across), 0.0, median(&mut along))
    };

    let feet = soles
        .iter()
        .zip(paths)
        .map(|((foot, soles), paths)| {
            let planted_soles: Vec<Vec<bool>> = paths
                .iter()
                .map(|(path, rest_height)| {
                    let mut planted = false;
                    (0..count)
                        .map(|tick| {
                            let height = path[tick].y - rest_height;
                            let moving = velocity(path, tick);
                            let speed = if travels { moving.length() } else { moving.y.abs() };
                            let (high, fast) = if planted {
                                (settings.height * RELEASE, settings.speed * RELEASE)
                            } else {
                                (settings.height, settings.speed)
                            };
                            planted = height < high && speed < fast;
                            planted
                        })
                        .collect()
                })
                .collect();
            let mut planted: Vec<bool> =
                (0..count).map(|tick| planted_soles.iter().any(|sole| sole[tick])).collect();
            tidy(&mut planted, usize::from(settings.ticks));
            let sole_runs = planted_soles
                .iter()
                .map(|sole| sole.iter().zip(&planted).map(|(sole, foot)| *sole && *foot).collect())
                .collect();
            PlantedFoot {
                foot: *foot,
                soles: soles.iter().map(|sole| sole.joint).collect(),
                paths: paths.into_iter().map(|(path, _)| path).collect(),
                planted: sole_runs,
                foot_planted: planted,
            }
        })
        .collect();
    Planting { ground, feet }
}

/// The middle of `values`, the upper of the two middles for an even count; 0 for none. Sorted by
/// [`f32::total_cmp`], so the same values give the same bits in any order.
fn median(values: &mut [f32]) -> f32 {
    values.sort_unstable_by(f32::total_cmp);
    values.get(values.len() / 2).copied().unwrap_or(0.0)
}

/// Closes lifts between two contacts shorter than `fewest` ticks, then drops contacts that short
/// inside the clip. A run touching either end stays.
fn tidy(planted: &mut [bool], fewest: usize) {
    for state in [false, true] {
        let mut start = 0;
        while start < planted.len() {
            let end =
                (start..planted.len()).find(|&tick| planted[tick] != planted[start]).unwrap_or(planted.len());
            let inside = start > 0 && end < planted.len();
            if planted[start] == state && inside && end - start < fewest {
                planted[start..end].fill(!state);
            }
            start = end;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn runs(planted: &[u8]) -> Vec<bool> {
        planted.iter().map(|&state| state == 1).collect()
    }

    #[test]
    fn short_lifts_close_and_short_contacts_inside_go() {
        let mut planted = runs(&[1, 1, 0, 1, 1, 1, 0, 0, 0, 1, 0, 0, 0, 0, 1]);
        tidy(&mut planted, 3);
        assert_eq!(planted, runs(&[1, 1, 1, 1, 1, 1, 0, 0, 0, 0, 0, 0, 0, 0, 1]));
    }
}
