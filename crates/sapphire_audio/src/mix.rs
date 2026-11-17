//! The mix at run time (M6 U2, spec §5.15): a [`Mix`] document's snapshots and ducking rules, moved once a
//! frame in plain numbers, into what each bus does now.
//!
//! A snapshot is in by a depth from 0 to 1 that follows its driver over real time: up with its attack and down
//! with its release, each a time constant (the seconds to go about two thirds of the way). The perfect dodge's
//! slow motion was the one snapshot before M6, `ClockSnapshot`; its arithmetic is unchanged here, so the shipped
//! mix's `slow_motion` bends and darkens the world exactly as v0.2.0 did. A ducking rule's amount follows the
//! loudest of its bus's voices the same way, which [`AudioSystem`](crate::AudioSystem) reads from their mouth
//! curves at their playheads.
//!
//! Nothing here allocates once made: every bus, snapshot and rule has its slot from [`MixState::new`].

use sapphire_assets::mix::{DuckFollow, Mix, SnapshotDriver};
use sapphire_core::Name;

/// The cutoff a low-pass closes from, above anything heard, in hertz.
const OPEN_HZ: f32 = 20_000.0;

/// Below this depth a snapshot or a duck that's leaving counts as gone, rather than approaching forever.
const GONE: f32 = 0.002;

/// A snapshot's depth moved `dt` real seconds toward `target`, as M2's `ClockSnapshot` moved it.
fn approach(depth: f32, target: f32, attack: f32, release: f32, dt: f32) -> f32 {
    let time_constant = if target > depth { attack } else { release };
    let share = if dt > 0.0 { 1.0 - (-dt / time_constant.max(1e-4)).exp() } else { 0.0 };
    let depth = depth + (target - depth) * share;
    if target == 0.0 && depth < GONE { 0.0 } else { depth }
}

/// What the snapshots and ducks do to one bus this frame.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct BusNow {
    /// Added to the bus's own volume, in decibels.
    pub gain_db: f32,
    /// The snapshot filter's cutoff: [`f32::INFINITY`] is open.
    pub low_pass_hz: f32,
    /// Its voices' pitch, with every bus above it.
    pub pitch: f32,
    /// Paused by a snapshot itself: what the mixer is told.
    pub paused: bool,
    /// How long its pause or the pause's end takes: the attack of the snapshot that pauses it, or the release of
    /// one that would.
    pub pause_fade: f32,
    /// Waiting where it is, paused itself or under a bus that is: its voices' playheads wait too.
    pub held: bool,
}

impl BusNow {
    const NEUTRAL: Self = Self {
        gain_db: 0.0,
        low_pass_hz: f32::INFINITY,
        pitch: 1.0,
        paused: false,
        pause_fade: 0.0,
        held: false,
    };
}

/// One snapshot's change to one bus, by the bus's place.
#[derive(Clone, Copy, Debug)]
struct Change {
    bus: usize,
    gain_db: f32,
    low_pass_hz: Option<f32>,
    pitch: Option<f32>,
    pause: bool,
}

#[derive(Clone, Debug)]
struct SnapshotNow {
    name: Name,
    driver: SnapshotDriver,
    attack: f32,
    release: f32,
    changes: Vec<Change>,
    depth: f32,
    /// Asked for, or its clock slowed, this frame.
    engaged: bool,
}

#[derive(Clone, Debug)]
struct RuleNow {
    when: usize,
    follow: DuckFollow,
    attack: f32,
    release: f32,
    /// The buses it takes down, by place, and how far at the fullest.
    duck: Vec<(usize, f32)>,
    /// How far in it is, 0 to 1.
    amount: f32,
}

/// A mix's moving parts. See the [module docs](self).
#[derive(Clone, Debug)]
pub(crate) struct MixState {
    /// Each bus's parent, by place: every parent comes before its children.
    parents: Vec<Option<usize>>,
    /// Each bus's own volume, as the mix document says.
    volumes: Vec<f32>,
    /// Whether some snapshot closes a low-pass on the bus: only those buses have a filter.
    filtered: Vec<bool>,
    /// By priority, lowest first; of one priority, in the document's order.
    snapshots: Vec<SnapshotNow>,
    rules: Vec<RuleNow>,
    now: Vec<BusNow>,
}

impl MixState {
    /// The mix's state with nothing in, from a checked `mix`.
    pub fn new(mix: &Mix) -> Self {
        let parents = (0..mix.buses.len()).map(|bus| mix.parent(bus)).collect();
        let bus = |name: &str| mix.bus(name).expect("a checked mix names its own buses");
        let mut snapshots: Vec<SnapshotNow> = mix
            .snapshots
            .iter()
            .map(|snapshot| SnapshotNow {
                name: Name::new(&snapshot.name),
                driver: snapshot.driver,
                attack: snapshot.attack,
                release: snapshot.release,
                changes: snapshot
                    .buses
                    .iter()
                    .map(|change| Change {
                        bus: bus(&change.bus),
                        gain_db: change.gain_db,
                        low_pass_hz: change.low_pass_hz,
                        pitch: change.pitch,
                        pause: change.pause,
                    })
                    .collect(),
                depth: 0.0,
                engaged: false,
            })
            .collect();
        // Stable: of one priority, the document's order.
        let priorities: Vec<u8> = mix.snapshots.iter().map(|snapshot| snapshot.priority).collect();
        let mut order: Vec<usize> = (0..snapshots.len()).collect();
        order.sort_by_key(|&index| priorities[index]);
        let mut by_place: Vec<Option<SnapshotNow>> = snapshots.drain(..).map(Some).collect();
        let snapshots = order.into_iter().filter_map(|index| by_place[index].take()).collect();
        let mut filtered = vec![false; mix.buses.len()];
        for change in mix.snapshots.iter().flat_map(|snapshot| &snapshot.buses) {
            if change.low_pass_hz.is_some() {
                filtered[bus(&change.bus)] = true;
            }
        }
        let rules = mix
            .ducking
            .iter()
            .map(|rule| RuleNow {
                when: bus(&rule.when),
                follow: rule.follow,
                attack: rule.attack,
                release: rule.release,
                duck: rule.duck.iter().map(|target| (bus(&target.bus), target.gain_db)).collect(),
                amount: 0.0,
            })
            .collect();
        Self {
            parents,
            volumes: mix.buses.iter().map(|bus| bus.volume_db).collect(),
            filtered,
            snapshots,
            rules,
            now: vec![BusNow::NEUTRAL; mix.buses.len()],
        }
    }

    /// The bus's own volume, as the document says.
    pub fn volume_db(&self, bus: usize) -> f32 {
        self.volumes[bus]
    }

    /// Whether the bus has a snapshot's filter.
    pub fn filtered(&self, bus: usize) -> bool {
        self.filtered[bus]
    }

    /// What the bus does now.
    pub fn now(&self, bus: usize) -> BusNow {
        self.now[bus]
    }

    /// Whether `bus` is `ancestor` or under it.
    pub fn is_under(&self, mut bus: usize, ancestor: usize) -> bool {
        loop {
            if bus == ancestor {
                return true;
            }
            match self.parents[bus] {
                Some(parent) => bus = parent,
                None => return false,
            }
        }
    }

    /// The ducking rules: the bus each follows, and how.
    pub fn rules(&self) -> impl Iterator<Item = (usize, DuckFollow)> + '_ {
        self.rules.iter().map(|rule| (rule.when, rule.follow))
    }

    /// [`Self::step_snapshots`] and then [`Self::step_ducks`].
    #[cfg(test)]
    pub fn step(&mut self, asked: &[Name], world_time_scale: f32, levels: &[f32], dt: f32) {
        self.step_snapshots(asked, world_time_scale, dt);
        self.step_ducks(levels, dt);
    }

    /// Moves every rule `dt` real seconds toward its `levels` (one a rule, 0 to 1, as [`Self::rules`] lists them),
    /// and takes its buses down by it: once a frame, after [`Self::step_snapshots`], once the frame's voices have
    /// started.
    pub fn step_ducks(&mut self, levels: &[f32], dt: f32) {
        for (rule, &level) in self.rules.iter_mut().zip(levels) {
            let target = if level.is_finite() { level.clamp(0.0, 1.0) } else { 0.0 };
            rule.amount = approach(rule.amount, target, rule.attack, rule.release, dt);
            for &(bus, gain_db) in &rule.duck {
                self.now[bus].gain_db += gain_db * rule.amount;
            }
        }
    }

    /// Moves every snapshot `dt` real seconds toward where `asked` and the world's clock put it, and works out
    /// each bus but for the ducks: once a frame, first.
    pub fn step_snapshots(&mut self, asked: &[Name], world_time_scale: f32, dt: f32) {
        for snapshot in &mut self.snapshots {
            let target = match snapshot.driver {
                SnapshotDriver::Asked => {
                    if asked.contains(&snapshot.name) {
                        1.0
                    } else {
                        0.0
                    }
                }
                SnapshotDriver::WorldTimeScale { floor } if world_time_scale.is_finite() => {
                    ((1.0 - world_time_scale) / (1.0 - floor).max(1e-4)).clamp(0.0, 1.0)
                }
                SnapshotDriver::WorldTimeScale { .. } => 0.0,
            };
            snapshot.engaged = target > 0.0;
            snapshot.depth = approach(snapshot.depth, target, snapshot.attack, snapshot.release, dt);
        }
        for now in &mut self.now {
            *now = BusNow { pause_fade: now.pause_fade, ..BusNow::NEUTRAL };
        }
        for snapshot in &self.snapshots {
            for change in &snapshot.changes {
                let now = &mut self.now[change.bus];
                if change.pause && snapshot.engaged {
                    now.paused = true;
                    now.pause_fade = snapshot.attack;
                } else if change.pause && !now.paused {
                    now.pause_fade = snapshot.release;
                }
                if snapshot.depth == 0.0 {
                    continue;
                }
                let depth = snapshot.depth;
                now.gain_db += change.gain_db * depth;
                // Log-spaced, as pitch and cutoff are heard: from what lower snapshots made toward its own.
                if let Some(cutoff) = change.low_pass_hz {
                    let from = if now.low_pass_hz.is_finite() { now.low_pass_hz } else { OPEN_HZ };
                    now.low_pass_hz = from * (cutoff / from).powf(depth);
                }
                if let Some(pitch) = change.pitch {
                    now.pitch *= (pitch / now.pitch).powf(depth);
                }
            }
        }
        // Down the tree: a bus bends and waits with every bus above it.
        for bus in 0..self.now.len() {
            self.now[bus].held = self.now[bus].paused;
            if let Some(parent) = self.parents[bus] {
                let above = self.now[parent];
                let now = &mut self.now[bus];
                now.pitch *= above.pitch;
                now.held |= above.held;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use sapphire_assets::cue::Bus;

    use super::*;

    const FRAME: f32 = 1.0 / 60.0;

    fn shipped() -> Mix {
        Mix::from_document(include_str!("../../../content/audio/game.mix.ron")).unwrap()
    }

    fn sfx() -> usize {
        Bus::Sfx.index()
    }

    /// What a bus does, but for how long a pause it isn't in would take.
    fn doing(now: BusNow) -> BusNow {
        BusNow { pause_fade: 0.0, ..now }
    }

    fn settled(state: &mut MixState, asked: &[Name], scale: f32) -> BusNow {
        for _ in 0..600 {
            state.step(asked, scale, &[0.0], FRAME);
        }
        state.now(sfx())
    }

    /// M2's `ClockSnapshot` at its default tuning, which the shipped `slow_motion` is: the mix it made after each
    /// frame, as (pitch, cutoff).
    fn clock_snapshot(scales: &[f32]) -> Vec<(f32, f32)> {
        let (floor_scale, floor_pitch, floor_low_pass, attack, release) =
            (0.2f32, 0.75f32, 900.0f32, 0.03, 0.25);
        let mut depth = 0.0f32;
        scales
            .iter()
            .map(|&scale| {
                let target = ((1.0 - scale) / (1.0 - floor_scale).max(1e-4)).clamp(0.0, 1.0);
                let time_constant = if target > depth { attack } else { release };
                let share = 1.0 - (-FRAME / f32::max(time_constant, 1e-4)).exp();
                depth += (target - depth) * share;
                if target == 0.0 && depth < GONE {
                    depth = 0.0;
                }
                if depth == 0.0 {
                    return (1.0, f32::INFINITY);
                }
                let toward = |open: f32, floor: f32| open * (floor / open).powf(depth);
                (toward(1.0, floor_pitch), toward(OPEN_HZ, floor_low_pass))
            })
            .collect()
    }

    #[test]
    fn slow_motion_moves_as_the_clock_snapshot_did_to_the_bit() {
        let mut scales = vec![1.0; 10];
        scales.extend([0.2; 40]);
        scales.extend([0.6; 30]);
        scales.extend([1.0; 200]);
        let expected = clock_snapshot(&scales);
        let mut state = MixState::new(&shipped());
        for (frame, (&scale, &(pitch, cutoff))) in scales.iter().zip(&expected).enumerate() {
            state.step(&[], scale, &[0.0], FRAME);
            for bus in [Bus::Sfx, Bus::Ambience] {
                let now = state.now(bus.index());
                assert_eq!((now.pitch, now.low_pass_hz), (pitch, cutoff), "frame {frame}, {bus:?}");
            }
            for bus in [Bus::Ui, Bus::Music, Bus::Voice] {
                assert_eq!(
                    doing(state.now(bus.index())),
                    BusNow::NEUTRAL,
                    "frame {frame}: {bus:?} keeps its time"
                );
            }
        }
        assert_eq!(doing(state.now(sfx())), BusNow::NEUTRAL, "and in the end it's gone, not nearly");
    }

    #[test]
    fn a_world_clock_that_isnt_a_number_or_runs_fast_is_no_slow_motion() {
        let mut state = MixState::new(&shipped());
        for scale in [1.0, 2.0, f32::NAN, f32::INFINITY] {
            state.step(&[], scale, &[0.0], FRAME);
            assert_eq!(doing(state.now(sfx())), BusNow::NEUTRAL, "{scale}");
        }
        state.step(&[], 0.2, &[0.0], 0.0);
        assert_eq!(doing(state.now(sfx())), BusNow::NEUTRAL, "no time, no change");
    }

    #[test]
    fn pause_holds_the_world_and_lets_it_go_with_its_own_fades() {
        let mut state = MixState::new(&shipped());
        let pause = [Name::new("pause")];
        state.step(&pause, 1.0, &[0.0], FRAME);
        for bus in [Bus::Sfx, Bus::Ambience, Bus::Voice] {
            let now = state.now(bus.index());
            assert!(now.paused && now.pause_fade == 0.05, "{bus:?}: {now:?}");
        }
        assert!(!state.now(Bus::Music.index()).paused && !state.now(Bus::Ui.index()).paused);
        assert!(settled(&mut state, &pause, 1.0).paused);
        let music = state.now(Bus::Music.index());
        assert!((music.gain_db + 6.0).abs() < 1e-3 && (music.low_pass_hz - 2_000.0).abs() < 1.0, "{music:?}");
        state.step(&[], 1.0, &[0.0], FRAME);
        let sfx_now = state.now(sfx());
        assert!(!sfx_now.paused && sfx_now.pause_fade == 0.2, "{sfx_now:?}");
        settled(&mut state, &[], 1.0);
        assert_eq!(state.now(Bus::Music.index()).low_pass_hz, f32::INFINITY);
    }

    #[test]
    fn a_higher_priority_takes_a_cutoff_from_a_lower_as_far_as_its_in() {
        let mut mix = shipped();
        // Slow motion (10) under pause (20): the music isn't in slow motion's, so give it some.
        mix.snapshots[0].buses[0].bus = "music".to_owned();
        let mut state = MixState::new(&mix);
        let music = Bus::Music.index();
        for _ in 0..600 {
            state.step(&[], 0.2, &[0.0], FRAME);
        }
        assert!((state.now(music).low_pass_hz - 900.0).abs() < 1.0);
        for _ in 0..600 {
            state.step(&[Name::new("pause")], 0.2, &[0.0], FRAME);
        }
        assert!((state.now(music).low_pass_hz - 2_000.0).abs() < 1.0, "pause's own, wholly in");
        // Pause half out: halfway, on the log scale, from slow motion's to its own.
        let state_half = {
            let mut state = state.clone();
            state.snapshots[1].depth = 0.5;
            state.step(&[Name::new("pause")], 0.2, &[0.0], 0.0);
            state.now(music).low_pass_hz
        };
        assert!((state_half - (900.0f32 * 2_000.0).sqrt()).abs() < 1.0, "{state_half}");
    }

    #[test]
    fn a_rule_ducks_its_buses_by_its_level_and_lets_them_back() {
        let mut state = MixState::new(&shipped());
        let (music, effects) = (Bus::Music.index(), sfx());
        for _ in 0..60 {
            state.step(&[], 1.0, &[1.0], FRAME);
        }
        assert!((state.now(music).gain_db + 9.0).abs() < 1e-3, "{:?}", state.now(music));
        assert!((state.now(effects).gain_db + 4.0).abs() < 1e-3);
        state.step(&[], 1.0, &[0.5], FRAME);
        assert!(state.now(music).gain_db > -9.0);
        for _ in 0..30 {
            state.step(&[], 1.0, &[0.0], FRAME);
        }
        assert_eq!(state.now(music).gain_db, 0.0, "all the way back");
        // A level that isn't a number is none.
        state.step(&[], 1.0, &[f32::NAN], FRAME);
        assert_eq!(state.now(music).gain_db, 0.0);
    }

    #[test]
    fn a_child_bends_and_waits_with_its_parent() {
        let mut mix = shipped();
        mix.buses.push(sapphire_assets::mix::MixBus {
            name: "sfx/hits".to_owned(),
            volume_db: -3.0,
            effects: Vec::new(),
            sends: Vec::new(),
        });
        mix.check().unwrap();
        let mut state = MixState::new(&mix);
        let hits = mix.bus("sfx/hits").unwrap();
        assert!(state.is_under(hits, sfx()) && !state.is_under(sfx(), hits));
        assert_eq!(state.volume_db(hits), -3.0);
        for _ in 0..600 {
            state.step(&[Name::new("pause")], 0.2, &[0.0], FRAME);
        }
        let now = state.now(hits);
        assert!((now.pitch - 0.75).abs() < 1e-3 && now.held && !now.paused, "{now:?}: its parent is paused");
        assert!(!state.filtered(hits), "its parent's filter is the one it goes through");
    }
}
