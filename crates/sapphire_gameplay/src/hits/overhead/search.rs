// Pure shadow interval search. Probes are supplied by the bound private mesh and solved bodies.

use super::{CAPACITY, OverheadShadowFailure, OverheadShadowUncertainty};

#[derive(Clone, Copy, Debug)]
pub(super) struct Interval {
    pub(super) from: f32,
    pub(super) to: f32,
}

impl Interval {
    fn width(self) -> f64 {
        f64::from(self.to) - f64::from(self.from)
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum Probe<W, G> {
    Tested { possible: u16, witness: Option<(u16, W)>, geometry: G },
    Unsupported,
}

#[derive(Clone, Copy)]
pub(super) struct Motion {
    pub(super) rate: f64,
    pub(super) margin: f64,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum Outcome<W> {
    Separated,
    Witness { earliest_bound: f32, fraction: f32, travel_bound: f64, evidence: W },
    Unresolved(OverheadShadowUncertainty),
}

#[derive(Clone, Copy)]
pub(super) struct Policy {
    pub(super) leaves: u8,
    pub(super) max_probes: u16,
    pub(super) max_bracket_fraction: f64,
    pub(super) max_bracket_travel: f64,
    pub(super) motion: [Motion; CAPACITY],
}

struct Search<'a, W, G, C> {
    earliest_possible: Option<(f32, G)>,
    witness: Option<(f32, u16, W, G)>,
    uncertain_regions: u16,
    probes: u16,
    policy: Policy,
    all_regions: u16,
    chord: &'a C,
}

impl<W: Copy, G: Copy, C: Fn(&G, &G, u16) -> f64> Search<'_, W, G, C> {
    fn probe(
        &mut self,
        fraction: f32,
        bounds: Option<[&G; 2]>,
        active: u16,
        mut probe: impl FnMut(f32, Option<[&G; 2]>, u16) -> Probe<W, G>,
    ) -> Result<(u16, G), OverheadShadowFailure> {
        if self.probes >= self.policy.max_probes {
            return Err(OverheadShadowFailure::ProbeBudget);
        }
        self.probes += 1;
        match probe(fraction, bounds, active) {
            Probe::Unsupported => Err(OverheadShadowFailure::UnsupportedSample),
            Probe::Tested { possible, witness, geometry } => {
                debug_assert_eq!(possible & active, possible);
                if let Some((region, evidence)) = witness
                    && self.witness.is_none_or(|(known, _, _, _)| fraction < known)
                {
                    debug_assert_ne!(region & possible, 0);
                    self.witness = Some((fraction, region, evidence, geometry));
                }
                Ok((possible, geometry))
            }
        }
    }

    fn motion(&self, mask: u16) -> Motion {
        let mut result = Motion { rate: 0.0, margin: 0.0 };
        for (index, part) in self.policy.motion.iter().enumerate() {
            if mask & (1 << index) != 0 {
                result.rate = result.rate.max(part.rate);
                result.margin = result.margin.max(part.margin);
            }
        }
        result
    }

    fn travel(&self, first: (f32, &G), last: (f32, &G), mask: u16) -> f64 {
        let width = (Interval { from: first.0, to: last.0 }).width();
        let motion = self.motion(mask);
        let speed_bound = width * motion.rate + motion.margin;
        speed_bound.min((self.chord)(first.1, last.1, mask))
    }

    fn bracket(&self, mask: u16) -> Option<Outcome<W>> {
        let (first, (fraction, _, evidence, end)) = self.earliest_possible.zip(self.witness)?;
        let width = (Interval { from: first.0, to: fraction }).width();
        if !(0.0..=self.policy.max_bracket_fraction).contains(&width) {
            return None;
        }
        let travel_bound = self.travel((first.0, &first.1), (fraction, &end), mask);
        (travel_bound <= self.policy.max_bracket_travel).then_some(Outcome::Witness {
            earliest_bound: first.0,
            fraction,
            travel_bound,
            evidence,
        })
    }

    fn note_uncertain(&mut self, at: f32, geometry: G, possible: u16) -> Option<Outcome<W>> {
        self.uncertain_regions |= possible;
        if self.earliest_possible.is_some() {
            return None;
        }
        // The left-first walk already certified all time before this leaf as separated.
        // Unvisited time inside the bracket must be bounded for every region.
        self.earliest_possible = Some((at, geometry));
        self.bracket(self.all_regions)
    }

    fn unresolved(&self, cause: OverheadShadowFailure) -> Outcome<W> {
        let mask = self.uncertain_regions | self.witness.map_or(0, |(_, region, _, _)| region);
        let motion = self.motion(mask);
        let sampled_bracket_travel = self
            .earliest_possible
            .zip(self.witness)
            .map(|(first, (at, _, _, end))| self.travel((first.0, &first.1), (at, &end), mask));
        Outcome::Unresolved(OverheadShadowUncertainty {
            cause,
            earliest_possible: self.earliest_possible.map(|(at, _)| at),
            sampled_witness: self.witness.map(|(at, _, _, _)| at),
            relative_rate: motion.rate,
            numerical_margin: motion.margin,
            sampled_bracket_travel,
        })
    }

    fn refine(
        &mut self,
        interval: Interval,
        bounds: [&G; 2],
        depth: u8,
        active: u16,
        probe: &mut impl FnMut(f32, Option<[&G; 2]>, u16) -> Probe<W, G>,
    ) -> Result<Option<Outcome<W>>, OverheadShadowFailure> {
        if self.witness.is_some_and(|(at, _, _, _)| interval.from >= at) {
            return Ok(None);
        }
        if self.earliest_possible.is_some_and(|(start, _)| {
            f64::from(interval.from) - f64::from(start) > self.policy.max_bracket_fraction
        }) {
            return Err(OverheadShadowFailure::ContactBracket);
        }
        let mid = interval.from + (interval.to - interval.from) * 0.5;
        if mid == interval.from || mid == interval.to {
            return Ok(self.note_uncertain(interval.from, *bounds[0], active));
        }
        let (possible, center) = self.probe(mid, Some(bounds), active, &mut *probe)?;
        if possible == 0 {
            return Ok(None);
        }
        if self.witness.is_some_and(|(at, _, _, _)| at == mid)
            && let Some(bracket) = self.bracket(self.all_regions)
        {
            return Ok(Some(bracket));
        }
        if depth == self.policy.leaves {
            return Ok(self.note_uncertain(interval.from, *bounds[0], possible));
        }
        let left = self.refine(
            Interval { from: interval.from, to: mid },
            [bounds[0], &center],
            depth + 1,
            possible,
            probe,
        )?;
        if left.is_some() {
            return Ok(left);
        }
        self.refine(Interval { from: mid, to: interval.to }, [&center, bounds[1]], depth + 1, possible, probe)
    }

    fn sweep(
        &mut self,
        interval: Interval,
        probe: &mut impl FnMut(f32, Option<[&G; 2]>, u16) -> Probe<W, G>,
    ) -> Result<Outcome<W>, OverheadShadowFailure> {
        let active = self.all_regions;
        let (first_possible, first) = self.probe(interval.from, None, active, &mut *probe)?;
        if let Some((fraction, _, evidence, _)) = self.witness {
            return Ok(Outcome::Witness { earliest_bound: fraction, fraction, travel_bound: 0.0, evidence });
        }
        if interval.from == interval.to {
            return if first_possible == 0 {
                Ok(Outcome::Separated)
            } else {
                Err(OverheadShadowFailure::NoWitness)
            };
        }
        let (_, last) = self.probe(interval.to, None, active, &mut *probe)?;
        if let Some(bracket) = self.refine(interval, [&first, &last], 0, active, probe)? {
            return Ok(bracket);
        }
        match (self.earliest_possible, self.witness) {
            (None, None) => Ok(Outcome::Separated),
            (Some(_), Some((_, region, _, _))) => {
                self.bracket(self.uncertain_regions | region).ok_or(OverheadShadowFailure::ContactBracket)
            }
            (_, None) => Err(OverheadShadowFailure::NoWitness),
            _ => Err(OverheadShadowFailure::ContactBracket),
        }
    }
}

pub(super) fn sweep<W: Copy, G: Copy>(
    interval: Interval,
    active: u16,
    policy: Policy,
    mut probe: impl FnMut(f32, Option<[&G; 2]>, u16) -> Probe<W, G>,
    chord: impl Fn(&G, &G, u16) -> f64,
) -> (Outcome<W>, u16) {
    let mut state = Search {
        earliest_possible: None,
        witness: None,
        uncertain_regions: 0,
        probes: 0,
        policy,
        all_regions: active,
        chord: &chord,
    };
    let result = state.sweep(interval, &mut probe).unwrap_or_else(|cause| state.unresolved(cause));
    (result, state.probes)
}

#[cfg(test)]
mod tests {
    use super::*;

    enum Point<W> {
        Tested { possible: u16, witness: Option<(u16, W)> },
        Unsupported,
    }

    fn sweep<W: Copy>(
        interval: Interval,
        active: u16,
        policy: Policy,
        mut point: impl FnMut(f32, f64, u16) -> Point<W>,
    ) -> (Outcome<W>, u16) {
        super::sweep(
            interval,
            active,
            policy,
            |at, bounds, mask| {
                let reach = bounds.map_or(0.0, |[&a, &b]| {
                    (f64::from(at) - f64::from(a)).abs().max((f64::from(at) - f64::from(b)).abs())
                });
                match point(at, reach, mask) {
                    Point::Tested { possible, witness } => Probe::Tested { possible, witness, geometry: at },
                    Point::Unsupported => Probe::Unsupported,
                }
            },
            |_, _, _| f64::INFINITY,
        )
    }

    const WHOLE: Interval = Interval { from: 0.0, to: 1.0 };
    const POLICY: Policy = Policy {
        leaves: 12,
        max_probes: 128,
        max_bracket_fraction: 1.0 / 1024.0,
        max_bracket_travel: 0.001,
        motion: [Motion { rate: 1.0, margin: 0.00002 }; CAPACITY],
    };

    #[test]
    fn real_witness_closes_the_narrow_first_crossing_bracket() {
        let cross = 0.381_815_f32;
        let (outcome, _) = sweep(WHOLE, 1, POLICY, |at, reach, _| {
            let gap = f64::from(cross - at);
            if gap < 0.0 {
                Point::Tested { possible: 1, witness: Some((1, at)) }
            } else if gap > reach + POLICY.motion[0].margin {
                Point::Tested { possible: 0, witness: None }
            } else {
                Point::Tested { possible: 1, witness: None }
            }
        });
        let Outcome::Witness { earliest_bound, fraction, evidence, .. } = outcome else {
            panic!("{outcome:?}");
        };
        assert!(earliest_bound <= cross && cross < fraction);
        assert!(f64::from(fraction - earliest_bound) <= POLICY.max_bracket_fraction);
        assert_eq!(fraction.to_bits(), evidence.to_bits());
    }

    #[test]
    fn earlier_grazing_uncertainty_suppresses_a_later_real_witness() {
        let (outcome, _) = sweep(WHOLE, 1, POLICY, |at, reach, _| {
            if at > 0.6 {
                Point::Tested { possible: 1, witness: Some((1, at)) }
            } else if (f64::from(at) - 0.25).abs() <= reach + POLICY.motion[0].margin {
                Point::Tested { possible: 1, witness: None }
            } else {
                Point::Tested { possible: 0, witness: None }
            }
        });
        assert!(matches!(outcome, Outcome::Unresolved(report)
            if report.cause == OverheadShadowFailure::ContactBracket));
    }

    #[test]
    fn angular_crossing_is_visible_between_clear_endpoints() {
        let policy = Policy {
            motion: [Motion { rate: std::f64::consts::TAU, margin: POLICY.motion[0].margin }; CAPACITY],
            max_bracket_travel: 0.004,
            ..POLICY
        };
        let gap = |at: f32| f64::from(sapphire_math::det::cos(std::f32::consts::TAU * at).abs()) - 0.06;
        assert!(gap(0.0) > 0.0 && gap(1.0) > 0.0);
        let (outcome, _) = sweep(WHOLE, 1, policy, |at, reach, _| {
            let gap = gap(at);
            if gap < 0.0 {
                Point::Tested { possible: 1, witness: Some((1, at)) }
            } else if gap > reach * policy.motion[0].rate + policy.motion[0].margin {
                Point::Tested { possible: 0, witness: None }
            } else {
                Point::Tested { possible: 1, witness: None }
            }
        });
        let Outcome::Witness { earliest_bound, fraction, .. } = outcome else {
            panic!("{outcome:?}");
        };
        assert!(earliest_bound > 0.23 && fraction < 0.25);
    }

    #[test]
    fn a_small_time_bracket_must_still_meet_the_spatial_budget() {
        let policy = Policy { max_bracket_travel: 0.000_01, ..POLICY };
        let (outcome, _) = sweep(WHOLE, 1, policy, |at, reach, _| {
            let gap = f64::from(0.42 - at);
            if gap < 0.0 {
                Point::Tested { possible: 1, witness: Some((1, at)) }
            } else if gap > reach + policy.motion[0].margin {
                Point::Tested { possible: 0, witness: None }
            } else {
                Point::Tested { possible: 1, witness: None }
            }
        });
        assert!(matches!(outcome, Outcome::Unresolved(report)
            if report.cause == OverheadShadowFailure::ContactBracket));
    }

    #[test]
    fn descendant_probes_inherit_region_clearance_and_never_repeat_an_endpoint() {
        let mut sampled = std::collections::BTreeSet::new();
        let mut policy = POLICY;
        // A cleared region with a huge rate must not expand another region's bracket.
        policy.motion[1].rate = 100.0;
        let (result, _) = sweep(WHOLE, 0b11, policy, |at, reach, active| {
            assert!(sampled.insert(at.to_bits()), "duplicate fraction {at}");
            assert_eq!(active, if [0.0, 0.5, 1.0].contains(&at) { 0b11 } else { 0b01 });
            let gap = f64::from(0.381_815 - at);
            if gap < 0.0 {
                Point::Tested { possible: 1, witness: Some((1, at)) }
            } else {
                Point::Tested { possible: u16::from(gap <= reach + POLICY.motion[0].margin), witness: None }
            }
        });
        assert!(matches!(result, Outcome::Witness { .. }));
    }

    #[test]
    fn the_certified_chord_can_pass_the_spatial_budget_when_the_speed_bound_cannot() {
        let mut policy = POLICY;
        policy.motion[0].rate = 1000.0;
        let (result, _) = super::sweep(
            WHOLE,
            1,
            policy,
            |at, bounds: Option<[&f32; 2]>, _| {
                let reach =
                    bounds.map_or(0.0, |[&a, &b]| f64::from(at - a).abs().max(f64::from(at - b).abs()));
                let gap = f64::from(0.381_815 - at);
                Probe::Tested {
                    possible: u16::from(gap <= reach + POLICY.motion[0].margin),
                    witness: (gap < 0.0).then_some((1, at)),
                    geometry: at,
                }
            },
            |a, b, _| f64::from(*b - *a) + POLICY.motion[0].margin,
        );
        let Outcome::Witness { earliest_bound, fraction, travel_bound, .. } = result else {
            panic!("{result:?}");
        };
        assert!(travel_bound < policy.max_bracket_travel);
        assert!(f64::from(fraction - earliest_bound) * 1000.0 > policy.max_bracket_travel);
    }

    #[test]
    fn can_close_a_proven_bracket_before_refining_every_interior_leaf() {
        let policy =
            Policy { max_probes: 32, max_bracket_fraction: 1.0 / 64.0, max_bracket_travel: 0.012, ..POLICY };
        let points = |at: f32, reach: f64, active: u16| {
            let reaches = |onset: f64| f64::from(at) + reach >= onset;
            let possible = u16::from(reaches(0.49)) | (u16::from(reaches(0.498)) << 1);
            Point::Tested {
                possible: possible & active,
                witness: (at == 0.5 && active & 1 != 0).then_some((1, at)),
            }
        };

        let (outcome, probes) = sweep(WHOLE, 0b11, policy, points);
        let Outcome::Witness { earliest_bound, fraction, travel_bound, evidence } = outcome else {
            panic!("{outcome:?}");
        };
        assert!(earliest_bound <= 0.49 && earliest_bound > 0.489);
        assert_eq!(fraction.to_bits(), 0.5_f32.to_bits());
        assert_eq!(fraction.to_bits(), evidence.to_bits());
        assert!(travel_bound <= policy.max_bracket_travel);
        assert!(probes < policy.max_probes);

        // The second region is clear in the first uncertain leaf, but it has
        // not been cleared throughout the span to the real witness.
        let mut fast = policy;
        fast.motion[1].rate = 10.0;
        let (outcome, probes) = sweep(WHOLE, 0b11, fast, points);
        assert!(matches!(outcome, Outcome::Unresolved(report)
            if report.cause == OverheadShadowFailure::ProbeBudget));
        assert_eq!(probes, fast.max_probes);
    }

    #[test]
    fn unsupported_budget_and_graze_never_mint_a_contact() {
        assert!(matches!(sweep::<()>(WHOLE, 1, POLICY, |_, _, _| Point::Unsupported).0,
            Outcome::Unresolved(report) if report.cause == OverheadShadowFailure::UnsupportedSample));
        let budget = Policy { max_probes: 3, ..POLICY };
        let (result, probes) =
            sweep::<()>(WHOLE, 1, budget, |_, _, _| Point::Tested { possible: 1, witness: None });
        assert!(matches!(result, Outcome::Unresolved(report)
            if report.cause == OverheadShadowFailure::ProbeBudget));
        assert_eq!(probes, 3);
        assert_eq!(
            sweep::<()>(WHOLE, 1, POLICY, |_, _, _| Point::Tested { possible: 0, witness: None }).0,
            Outcome::Separated
        );
    }
}
