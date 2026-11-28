//! A river as its cook reads it along its own polyline (M6 H3a): each stretch's current and slope, its falls
//! and the places its water throws spray and mist, in the zone's presentation half
//! ([`CookedWaterSurface::river`](super::CookedWaterSurface::river)), outside the digest. The pipeline's
//! `zone::river` makes it; nothing here moves anything in the simulation, whose current is the volume's.
//!
//! # Stretches
//!
//! A river's points run straight between them (W2), so a **stretch** is the run from one point to the next, in
//! order: the way its current runs across the ground, its speed at either end (its points'), how far its
//! surface falls for each metre across the ground, and where it lies along the river in the metres its drawn
//! ribbon's `v` counts (`sapphire_zone::water::drawn`: across the ground from the first point), so a vertex's
//! `v` finds its stretch.
//!
//! # Falls
//!
//! A stretch whose surface falls more steeply than [`FALL_DEGREES`] (its slope past [`FALL_SLOPE`]) is a
//! **fall**: never a vertical one, since a stretch is at least `MIN_STRETCH` across the ground. Neighbouring
//! steep stretches are one fall, its **lip** the surface's middle where the first begins and its **foot** where
//! the last ends: where H3b draws its streaks between and its foam bands at either end.
//!
//! # Sites
//!
//! Where a world effect stands for the water (N9 spawns them by residency): each a position on the surface, a
//! [`WaterSiteKind`] and a strength from 0 to 1 that says how much of it: **rapids** where fast, shallow water
//! runs past an obstacle, and each fall's **foot**, where it lands. The cook states the rules
//! (`sapphire_pipeline::zone::river`).

use crate::document::{ZoneError, check_position};

/// How steep a river's surface falls, in degrees from level, past which a stretch is a fall.
pub const FALL_DEGREES: f32 = 50.0;

/// The tangent of [`FALL_DEGREES`], as a constant rather than a sine over a cosine, so every machine compares
/// against the same number: a stretch falls more than this for each metre across the ground is a fall.
pub const FALL_SLOPE: f64 = 1.191_753_592_594_209_7;

/// The most sites a river may hold.
pub const MAX_SITES: usize = 512;

/// The current at which a river's rapids begin, in metres a second: its speed's share of a rapids' score is 0
/// here, rising straight to 1 at [`RAPIDS_FAST`]. The cook scores its sites by these, and H3b's foam may too.
pub const RAPIDS_SLOW: f64 = 1.5;
/// The current at which rapids are at their fullest.
pub const RAPIDS_FAST: f64 = 4.0;
/// The depth at which rapids begin, in metres: shallowness's share is 0 here, rising to 1 at
/// [`RAPIDS_SHALLOW`].
pub const RAPIDS_DEEP: f64 = 1.5;
/// The depth at which rapids are at their fullest.
pub const RAPIDS_SHALLOW: f64 = 0.3;
/// How far from an obstacle rapids begin, in metres: nearness's share is 0 here, rising to 1 at
/// [`RAPIDS_NEAR`].
pub const RAPIDS_FAR: f64 = 3.0;
/// How near an obstacle rapids are at their fullest.
pub const RAPIDS_NEAR: f64 = 0.5;

/// A river's cooked reading (see the [module docs](self)).
#[derive(Clone, Debug, PartialEq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
#[rkyv(derive(Debug))]
pub struct CookedRiver {
    /// One for each pair of its points, in order: stretch `i` runs from point `i` to point `i + 1`.
    pub stretches: Vec<CookedStretch>,
    /// Each run of neighbouring steep stretches, downstream.
    pub falls: Vec<CookedFall>,
    /// Where its water throws spray and mist, downstream.
    pub sites: Vec<WaterSite>,
}

/// One stretch of a river, between two of its points.
#[derive(Clone, Copy, Debug, PartialEq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
#[rkyv(derive(Debug))]
pub struct CookedStretch {
    /// The way its current runs, `[x, z]`: a unit vector across the ground, downstream.
    pub way: [f32; 2],
    /// Its current's speed where it begins and where it ends, in metres a second: its points'.
    pub speed: [f32; 2],
    /// How far its surface falls for each metre across the ground: positive downhill, negative where it rises.
    pub slope: f32,
    /// Where it begins and ends along the river, in metres across the ground from the first point: its drawn
    /// ribbon's `v` there.
    pub along: [f32; 2],
    /// Whether it's part of a fall: steeper than [`FALL_DEGREES`].
    pub fall: bool,
}

/// One fall of a river: neighbouring steep stretches.
#[derive(Clone, Copy, Debug, PartialEq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
#[rkyv(derive(Debug))]
pub struct CookedFall {
    /// Its first and last stretch, both its.
    pub stretches: [u16; 2],
    /// The middle of the surface where it goes over, in the zone's space.
    pub lip: [f32; 3],
    /// The middle of the surface where it lands.
    pub foot: [f32; 3],
    /// Its lip's and its foot's place along the river, as [`CookedStretch::along`].
    pub along: [f32; 2],
    /// From bank to bank at its lip and at its foot, in metres.
    pub width: [f32; 2],
}

/// What a site is.
#[derive(
    Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize,
)]
#[rkyv(derive(Debug, PartialEq, Eq), compare(PartialEq))]
pub enum WaterSiteKind {
    /// Fast, shallow water breaking past an obstacle: spray.
    Rapids,
    /// Where a fall lands: mist and a splash ring.
    FallFoot,
}

/// Where a world effect stands for a river's water.
#[derive(Clone, Copy, Debug, PartialEq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
#[rkyv(derive(Debug))]
pub struct WaterSite {
    pub kind: WaterSiteKind,
    /// On the surface at rest, in the zone's space.
    pub position: [f32; 3],
    /// How much of it, over 0 and at most 1.
    pub strength: f32,
}

impl ArchivedCookedRiver {
    /// Checks everything a reader leans on against its river's `points` count: a stretch for each pair of them,
    /// each finite, a unit way, a current's speeds and running on along the river from 0; its falls exactly the
    /// runs of its stretches flagged steep, in order, with their places finite and within the zone's reach; at
    /// most [`MAX_SITES`] sites, each within the zone's reach with a strength over 0 and at most 1.
    pub fn validate(&self, points: usize) -> Result<(), String> {
        let stretches = self.stretches.as_slice();
        if stretches.len() + 1 != points {
            return Err(format!("{} stretches for {points} points", stretches.len()));
        }
        let mut reached = 0.0f32;
        for (index, stretch) in stretches.iter().enumerate() {
            let problem = |what: &str| Err(format!("stretch {index}: {what}"));
            let way = stretch.way.each_ref().map(|value| value.to_native());
            let length = way[0] * way[0] + way[1] * way[1];
            if !(length.is_finite() && (length - 1.0).abs() <= 1e-3) {
                return problem("its way isn't a unit vector");
            }
            let speed = stretch.speed.each_ref().map(|value| value.to_native());
            if !speed.iter().all(|speed| speed.is_finite() && (0.0..=super::MAX_RIVER_SPEED).contains(speed))
            {
                return problem("its speeds aren't a current's");
            }
            if !stretch.slope.to_native().is_finite() {
                return problem("its slope isn't a number");
            }
            let along = stretch.along.each_ref().map(|value| value.to_native());
            if along[0] != reached || !(along[1].is_finite() && along[0] <= along[1]) {
                return problem("it doesn't run on from the stretch before");
            }
            reached = along[1];
        }
        let mut next = 0usize;
        for (index, fall) in self.falls.iter().enumerate() {
            let invalid = |what: &str| Err(format!("fall {index}: {what}"));
            let [first, last] = fall.stretches.each_ref().map(|value| usize::from(value.to_native()));
            if first < next || last < first || last >= stretches.len() {
                return invalid("its stretches aren't the river's, after the fall before");
            }
            let steep = |stretch: usize| stretches.get(stretch).is_some_and(|stretch| stretch.fall);
            let unflagged = (next..first).any(steep);
            let whole = (first..=last).all(steep) && (first == 0 || !steep(first - 1)) && !steep(last + 1);
            if unflagged || !whole {
                return invalid("it isn't a whole run of steep stretches");
            }
            next = last + 1;
            for place in [&fall.lip, &fall.foot] {
                check_position(place.each_ref().map(|value| value.to_native()))
                    .map_err(|problem| format!("fall {index}: {problem}"))?;
            }
            let along = fall.along.each_ref().map(|value| value.to_native());
            if along != [stretches[first].along[0].to_native(), stretches[last].along[1].to_native()] {
                return invalid("its place along the river isn't its stretches'");
            }
            let widths = fall.width.each_ref().map(|value| value.to_native());
            if !widths.iter().all(|width| (super::MIN_RIVER_WIDTH..=super::MAX_RIVER_WIDTH).contains(width)) {
                return invalid("its widths aren't a river's");
            }
        }
        if (next..stretches.len()).any(|stretch| stretches[stretch].fall) {
            return Err("a steep stretch is in no fall".to_owned());
        }
        if self.sites.len() > MAX_SITES {
            return Err(format!("{} sites is more than the {MAX_SITES} a river may hold", self.sites.len()));
        }
        for (index, site) in self.sites.iter().enumerate() {
            check_position(site.position.each_ref().map(|value| value.to_native()))
                .map_err(|problem| format!("site {index}: {problem}"))?;
            let strength = site.strength.to_native();
            if !(strength > 0.0 && strength <= 1.0) {
                return Err(format!("site {index}: its strength {strength} isn't over 0 and at most 1"));
            }
        }
        Ok(())
    }
}

impl ArchivedCookedRiver {
    /// It as a native value, as an editor or a test reads it whole.
    #[must_use]
    pub fn to_native(&self) -> CookedRiver {
        let pair = |values: &rkyv::Archived<[f32; 2]>| values.each_ref().map(|value| value.to_native());
        let triple = |values: &rkyv::Archived<[f32; 3]>| values.each_ref().map(|value| value.to_native());
        CookedRiver {
            stretches: (self.stretches.iter())
                .map(|stretch| CookedStretch {
                    way: pair(&stretch.way),
                    speed: pair(&stretch.speed),
                    slope: stretch.slope.to_native(),
                    along: pair(&stretch.along),
                    fall: stretch.fall,
                })
                .collect(),
            falls: (self.falls.iter())
                .map(|fall| CookedFall {
                    stretches: fall.stretches.each_ref().map(|value| value.to_native()),
                    lip: triple(&fall.lip),
                    foot: triple(&fall.foot),
                    along: pair(&fall.along),
                    width: pair(&fall.width),
                })
                .collect(),
            sites: (self.sites.iter())
                .map(|site| WaterSite {
                    kind: site.kind.to_native(),
                    position: triple(&site.position),
                    strength: site.strength.to_native(),
                })
                .collect(),
        }
    }
}

impl ArchivedWaterSiteKind {
    #[must_use]
    pub fn to_native(&self) -> WaterSiteKind {
        match self {
            Self::Rapids => WaterSiteKind::Rapids,
            Self::FallFoot => WaterSiteKind::FallFoot,
        }
    }
}

/// A river's reading that fails its check, as the zone's.
pub(crate) fn invalid(body: &str, problem: String) -> ZoneError {
    ZoneError::invalid(format!("water `{body}`.river"), problem)
}
