//! Patrols (M3 P3): places a brain walks between until it notices someone, from a document.
//!
//! A zone names one as a spline, from a spawner: `patrol: Some("yard_walk")`. A cook copies its points
//! into the `.szone`'s simulation, holding each to the navmesh by its field, so a new patrol is a line in
//! a document and no Rust. [`Simulation::add_patrol`](crate::Simulation::add_patrol) takes what a zone
//! holds, by name, before the fight. [`Simulation::give_patrol`](crate::Simulation::give_patrol) sets a
//! brain walking it, and [`crate::brain`] does the rest, on the [`BrainWatch`](sapphire_moves::BrainWatch)
//! its document has.
//!
//! **A closed one** runs on from its last point to its first. **An open one** turns round at each end.
//! **One of a single point** is a guard's post: it goes there, and stands.
//!
//! They go in [`Simulation::tables_hash`](crate::Simulation::tables_hash), by their bits, so a replay
//! refuses another patrol as it does another collider. A simulation with none hashes what it always did.
//! Which point a brain is on is in the checksum with the rest of its [`crate::Brain`].

use sapphire_core::Name;
use sapphire_ecs::prelude::*;
use sapphire_math::Vec3;

use crate::checksum::StateHasher;

/// The most points a patrol may hold: as many as a zone's spline may.
pub const MAX_PATROL_POINTS: usize = 1 << 16;

/// Most patrols a simulation may hold.
const MAX_PATROLS: usize = 1 << 16;

/// Which of a simulation's patrols: what [`Simulation::add_patrol`](crate::Simulation::add_patrol) gives.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PatrolId(u16);

/// Why a patrol can't be held.
#[derive(Debug, thiserror::Error, PartialEq)]
pub enum PatrolError {
    #[error("patrol `{name}` has no points, or more than {MAX_PATROL_POINTS}")]
    Points { name: Name },
    #[error("patrol `{name}`'s point {index} isn't a place: {point:?}")]
    Nowhere { name: Name, index: usize, point: [f32; 3] },
    #[error("there's already a patrol called `{name}`")]
    Taken { name: Name },
    #[error("a simulation holds no more than {MAX_PATROLS} patrols")]
    TooMany,
}

/// One patrol's place in [`Patrols::points`].
#[derive(Clone, Copy, Debug)]
struct Run {
    name: Name,
    start: u32,
    /// From 1 to [`MAX_PATROL_POINTS`].
    len: u32,
    closed: bool,
}

/// A simulation's patrols. A resource, filled before the fight and only read in it.
#[derive(Resource, Default)]
pub(crate) struct Patrols {
    runs: Vec<Run>,
    points: Vec<Vec3>,
}

impl Patrols {
    pub(crate) fn add(
        &mut self,
        name: Name,
        points: &[[f32; 3]],
        closed: bool,
    ) -> Result<PatrolId, PatrolError> {
        if points.is_empty() || points.len() > MAX_PATROL_POINTS {
            return Err(PatrolError::Points { name });
        }
        if let Some((index, point)) =
            points.iter().enumerate().find(|(_, point)| !point.iter().all(|value| value.is_finite()))
        {
            return Err(PatrolError::Nowhere { name, index, point: *point });
        }
        if self.named(name).is_some() {
            return Err(PatrolError::Taken { name });
        }
        if self.runs.len() >= MAX_PATROLS {
            return Err(PatrolError::TooMany);
        }
        let id = PatrolId(self.runs.len() as u16);
        let start = self.points.len() as u32;
        self.runs.push(Run { name, start, len: points.len() as u32, closed });
        self.points.extend(points.iter().map(|point| Vec3::from_array(*point)));
        Ok(id)
    }

    pub(crate) fn named(&self, name: Name) -> Option<PatrolId> {
        self.runs.iter().position(|run| run.name == name).map(|index| PatrolId(index as u16))
    }

    fn run(&self, id: PatrolId) -> Run {
        self.runs[id.0 as usize]
    }

    /// Where a walk of it is headed.
    pub(crate) fn point(&self, walk: PatrolWalk) -> Vec3 {
        self.points[self.run(walk.patrol).start as usize + walk.next as usize]
    }

    /// A walk of `patrol` from where someone's standing: to its nearest point across the ground, the first
    /// of those equally near.
    pub(crate) fn start(&self, patrol: PatrolId, position: Vec3) -> PatrolWalk {
        let run = self.run(patrol);
        let points = &self.points[run.start as usize..(run.start + run.len) as usize];
        let mut nearest = (f32::MAX, 0);
        for (index, point) in points.iter().enumerate() {
            let (x, z) = (point.x - position.x, point.z - position.z);
            let apart = x * x + z * z;
            if apart < nearest.0 {
                nearest = (apart, index);
            }
        }
        PatrolWalk { patrol, next: nearest.1 as u16, back: false }
    }

    /// `walk`, having got where it was headed: on to the next point, round a closed one and back along an
    /// open one.
    pub(crate) fn advance(&self, walk: &mut PatrolWalk) {
        let run = self.run(walk.patrol);
        let (next, last) = (u32::from(walk.next), run.len - 1);
        if last == 0 {
            return;
        }
        if run.closed {
            walk.next = if next == last { 0 } else { next as u16 + 1 };
            return;
        }
        if next == last {
            walk.back = true;
        } else if next == 0 {
            walk.back = false;
        }
        walk.next = if walk.back { next as u16 - 1 } else { next as u16 + 1 };
    }

    /// What a replay depends on: nothing at all with no patrols, so nothing recorded before them moved.
    pub(crate) fn write_tables(&self, hasher: &mut StateHasher) {
        if self.runs.is_empty() {
            return;
        }
        hasher.u32(self.runs.len() as u32);
        for run in &self.runs {
            hasher.u64(run.name.as_u64());
            hasher.u32(run.len);
            hasher.u8(u8::from(run.closed));
            for point in &self.points[run.start as usize..(run.start + run.len) as usize] {
                for value in point.to_array() {
                    hasher.bytes(&value.to_bits().to_le_bytes());
                }
            }
        }
    }
}

/// A brain's place on its patrol.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PatrolWalk {
    pub(crate) patrol: PatrolId,
    /// The point it's headed for.
    pub(crate) next: u16,
    /// Whether it's coming back along an open one.
    pub(crate) back: bool,
}

impl PatrolWalk {
    /// The point it's headed for, as a place in its patrol.
    #[must_use]
    pub const fn next(self) -> u16 {
        self.next
    }

    /// For a checksum: all of it.
    pub(crate) fn write_state(self, hasher: &mut StateHasher) {
        hasher.u32(u32::from(self.patrol.0));
        hasher.u32(u32::from(self.next));
        hasher.u8(u8::from(self.back));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn name(text: &str) -> Name {
        Name::intern(text)
    }

    fn square() -> [[f32; 3]; 4] {
        [[0.0, 0.0, 0.0], [4.0, 0.0, 0.0], [4.0, 0.0, 4.0], [0.0, 0.0, 4.0]]
    }

    /// The points `count` advances visit from point 0.
    fn walked(patrols: &Patrols, patrol: PatrolId, count: usize) -> Vec<u16> {
        let mut walk = PatrolWalk { patrol, next: 0, back: false };
        (0..count)
            .map(|_| {
                patrols.advance(&mut walk);
                walk.next
            })
            .collect()
    }

    #[test]
    fn a_closed_one_goes_round_and_an_open_one_turns_back_and_a_post_stays() {
        let mut patrols = Patrols::default();
        let closed = patrols.add(name("closed"), &square(), true).expect("a good one");
        let open = patrols.add(name("open"), &square(), false).expect("a good one");
        let post = patrols.add(name("post"), &square()[..1], false).expect("a good one");
        let pair = patrols.add(name("pair"), &square()[..2], false).expect("a good one");
        assert_eq!(walked(&patrols, closed, 6), [1, 2, 3, 0, 1, 2]);
        assert_eq!(walked(&patrols, open, 8), [1, 2, 3, 2, 1, 0, 1, 2]);
        assert_eq!(walked(&patrols, post, 3), [0, 0, 0]);
        assert_eq!(walked(&patrols, pair, 4), [1, 0, 1, 0]);
    }

    #[test]
    fn a_walk_starts_at_the_nearest_point() {
        let mut patrols = Patrols::default();
        let patrol = patrols.add(name("square"), &square(), true).expect("a good one");
        assert_eq!(patrols.start(patrol, Vec3::new(3.0, 9.0, 3.5)).next, 2, "height's no part of it");
        assert_eq!(
            patrols.start(patrol, Vec3::new(2.0, 0.0, 2.0)).next,
            0,
            "the first of those equally near"
        );
        assert_eq!(patrols.point(patrols.start(patrol, Vec3::new(5.0, 0.0, -1.0))), Vec3::new(4.0, 0.0, 0.0));
    }

    #[test]
    fn a_bad_one_says_what_is_wrong_with_it() {
        let mut patrols = Patrols::default();
        let yard = name("yard");
        assert_eq!(patrols.add(yard, &[], true), Err(PatrolError::Points { name: yard }));
        let nowhere = [[0.0; 3], [f32::NAN, 0.0, 0.0]];
        assert!(matches!(patrols.add(yard, &nowhere, true), Err(PatrolError::Nowhere { index: 1, .. })));
        let error = patrols.add(yard, &nowhere, true).expect_err("it's a bad one").to_string();
        assert!(error.contains("yard") && error.contains("point 1"), "{error}");
        patrols.add(yard, &square(), true).expect("a good one");
        assert_eq!(patrols.add(yard, &square(), true), Err(PatrolError::Taken { name: yard }));
    }

    #[test]
    fn the_tables_read_every_part_of_every_patrol_and_nothing_with_none() {
        let hash = |patrols: &Patrols| {
            let mut hasher = StateHasher::new();
            patrols.write_tables(&mut hasher);
            hasher.finish()
        };
        let none = hash(&Patrols::default());
        assert_eq!(none, StateHasher::new().finish());

        let with = |text: &str, points: &[[f32; 3]], closed: bool| {
            let mut patrols = Patrols::default();
            patrols.add(name(text), points, closed).expect("a good one");
            hash(&patrols)
        };
        let base = with("a", &square(), true);
        assert_ne!(base, none);
        assert_ne!(base, with("b", &square(), true), "its name");
        assert_ne!(base, with("a", &square(), false), "whether it's closed");
        assert_ne!(base, with("a", &square()[..3], true), "how long it is");
        let mut moved = square();
        moved[3][2] = 4.5;
        assert_ne!(base, with("a", &moved, true), "where a point is");
    }
}
