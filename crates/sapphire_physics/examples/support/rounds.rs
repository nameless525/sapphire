//! What a bench's alternating rounds add up to, shared by `sapphire_physics`' bench and
//! `sapphire_gameplay`'s, which include this file.
//!
//! A round's steps are read together, never column by column: reducing each column to its least across the
//! rounds made `worst` the least of the rounds' slowest steps, and let a median and a percentile come from
//! different rounds. So a row says two things. Every step of every round as one set, where `worst` is the
//! slowest there was, with how far the rounds' medians spread; and the quietest round, the one with the least
//! median, whole, as what the code costs with the least else going on.

use std::fmt;
use std::time::Duration;

/// A set of steps' median, higher percentile and slowest, all of the same steps.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Times {
    pub median: Duration,
    pub high: Duration,
    pub worst: Duration,
}

impl Times {
    /// Sorts `steps` to read them. `high` is its percentile in thousandths.
    fn of(steps: &mut [Duration], high: usize) -> Self {
        steps.sort_unstable();
        Self {
            median: steps[steps.len() / 2],
            high: steps[steps.len() * high / 1000],
            worst: steps[steps.len() - 1],
        }
    }
}

/// The steps of one row's rounds.
#[derive(Debug)]
pub struct Rounds {
    /// What a row calls its higher percentile, and that percentile in thousandths.
    high: (&'static str, usize),
    all: Vec<Duration>,
    each: Vec<Times>,
}

impl Rounds {
    pub const fn new(name: &'static str, thousandths: usize) -> Self {
        Self { high: (name, thousandths), all: Vec::new(), each: Vec::new() }
    }

    /// Adds a round's steps, which can't be empty.
    pub fn add(&mut self, mut steps: Vec<Duration>) {
        self.each.push(Times::of(&mut steps, self.high.1));
        self.all.append(&mut steps);
    }

    /// What they came to, once a round has run.
    pub fn summary(&mut self) -> Summary {
        let median = |times: &Times| times.median;
        let (index, quietest) =
            self.each.iter().enumerate().min_by_key(|(_, times)| median(times)).expect("a round ran");
        Summary {
            high: self.high.0,
            all: Times::of(&mut self.all, self.high.1),
            steps: self.all.len(),
            medians: (median(quietest), self.each.iter().map(median).max().expect("a round ran")),
            quietest: (index + 1, *quietest),
        }
    }
}

/// What a row's rounds came to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Summary {
    high: &'static str,
    /// Every step of every round, as one set.
    pub all: Times,
    pub steps: usize,
    /// The least and most of the rounds' medians.
    pub medians: (Duration, Duration),
    /// The round with the least median, counting from 1, and that round's own numbers.
    pub quietest: (usize, Times),
}

/// In microseconds, which a bench's heading says.
impl fmt::Display for Summary {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let micros = |time: Duration| time.as_secs_f64() * 1e6;
        let Self { high, all, steps, medians, quietest: (number, quietest) } = self;
        write!(
            f,
            "all {steps} steps: median {:.2} ({:.2} to {:.2} by round), {high} {:.2}, worst {:.2}; \
             quietest round, {number}: median {:.2}, {high} {:.2}, worst {:.2}",
            micros(all.median),
            micros(medians.0),
            micros(medians.1),
            micros(all.high),
            micros(all.worst),
            micros(quietest.median),
            micros(quietest.high),
            micros(quietest.worst),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn micros(values: &[u64]) -> Vec<Duration> {
        values.iter().copied().map(Duration::from_micros).collect()
    }

    /// One slow step in a round that isn't the quietest is still the worst there was, and nothing from
    /// another round gets into the quietest round's numbers.
    #[test]
    fn the_worst_is_the_slowest_of_every_step_and_the_quietest_round_is_one_round() {
        let (first, second, third): (Vec<u64>, Vec<u64>, Vec<u64>) = (
            (0..100).map(|_| 4).chain([500]).collect(),
            (0..100).map(|_| 2).chain([90]).collect(),
            (0..100).map(|_| 3).chain([7]).collect(),
        );
        let mut rounds = Rounds::new("p99", 990);
        for round in [&first, &second, &third] {
            rounds.add(micros(round));
        }
        let summary = rounds.summary();

        let us = Duration::from_micros;
        assert_eq!(summary.all.worst, us(500), "the slowest step of them all");
        assert_eq!(summary.steps, 303);
        assert_eq!(summary.medians, (us(2), us(4)));
        assert_eq!(summary.all.median, us(3));
        assert_eq!(summary.quietest.0, 2, "the second, counting from 1");
        assert_eq!(summary.quietest.1.median, us(2));
        assert_eq!(summary.quietest.1.worst, us(90), "its own worst, not the least of the rounds' 7");
    }

    /// A percentile is read from the same steps as the rest of its set, and a row says it by its name.
    #[test]
    fn a_rows_percentile_is_its_own() {
        let mut rounds = Rounds::new("p99.9", 999);
        rounds.add((1..=2000).map(Duration::from_micros).collect());
        let summary = rounds.summary();
        assert_eq!(summary.all.high, Duration::from_micros(1999));
        assert_eq!(summary.quietest.1, summary.all);
        assert!(summary.to_string().contains("p99.9 1999.00, worst 2000.00"), "{summary}");
    }
}
