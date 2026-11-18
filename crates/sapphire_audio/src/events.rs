//! A cue's tree as a start walks it (M6 U4a): [`CuePlan`], made once as the cue is added, and the sounds a start
//! picks from its one number and the [`CueParameters`] it was asked with.
//!
//! The tree is laid out flat, in the order a document names its nodes, each knowing where the nodes under it end,
//! with what each random node and sequence remembers beside it: a start walks it allocating nothing.
//!
//! **The draws.** A start draws the same count of numbers from the caller's [`CueRandom`] whatever its tree, so a
//! cue's shape moves no other cue's picks. Only the pick's number reaches the tree: the first random node the walk
//! reaches takes it as it is, which is how a version 2 cue's list read it, and each later one takes the next
//! number of a [`CueRng`] seeded by it.

use sapphire_assets::AssetId;
use sapphire_assets::cue::{CueNode, MAX_AVOID_REPEAT, MAX_START_SOUNDS, SILENCE_DB};
use sapphire_core::Name;

use crate::rng::{CueRandom, CueRng};

/// The most parameters a cue can be asked for with at once.
pub const MAX_CUE_PARAMETERS: usize = 4;

/// The parameter a footstep's surface is named by, which the shipped `step` cue switches on: `stone`, `grass`.
/// Nothing passes it yet, so a footstep plays its switch's default (spec §5.15, "As built (M6 U4a)").
pub const SURFACE: Name = Name::new("surface");

/// A parameter's value: a number for a blend, a case's name for a switch.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ParameterValue {
    Number(f32),
    Case(Name),
}

/// The parameters a cue is asked for with, by name, [`MAX_CUE_PARAMETERS`] at most: what its switches and blends
/// read. Plain data, so a request stays a copy.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct CueParameters {
    entries: [Option<(Name, ParameterValue)>; MAX_CUE_PARAMETERS],
}

impl CueParameters {
    /// None: every switch plays its default, and every blend its first point.
    pub const NONE: Self = Self { entries: [None; MAX_CUE_PARAMETERS] };

    /// These, with `name` set to `value`.
    #[must_use]
    pub fn with(mut self, name: Name, value: ParameterValue) -> Self {
        self.set(name, value);
        self
    }

    /// Sets `name` to `value`, in place of what it was: whether there was room for it.
    pub fn set(&mut self, name: Name, value: ParameterValue) -> bool {
        let slot = self
            .entries
            .iter()
            .position(|entry| entry.is_some_and(|(held, _)| held == name))
            .or_else(|| self.entries.iter().position(Option::is_none));
        if let Some(slot) = slot {
            self.entries[slot] = Some((name, value));
        }
        slot.is_some()
    }

    /// `name`'s value, if it has one.
    #[must_use]
    pub fn get(&self, name: Name) -> Option<ParameterValue> {
        self.entries.iter().flatten().find(|(held, _)| *held == name).map(|&(_, value)| value)
    }
}

/// The sounds a start plays, each with what the tree takes off its gain in decibels (a blend's share), in the
/// order the walk reached them: [`MAX_START_SOUNDS`] at most.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Picks {
    sounds: [(AssetId, f32); MAX_START_SOUNDS],
    len: usize,
}

impl Picks {
    const NONE: Self = Self { sounds: [(AssetId::from_bytes([0; 16]), 0.0); MAX_START_SOUNDS], len: 0 };

    pub(crate) fn as_slice(&self) -> &[(AssetId, f32)] {
        &self.sounds[..self.len]
    }

    /// Past [`MAX_START_SOUNDS`] a sound is left out: a checked cue never reaches it.
    fn push(&mut self, sound: AssetId, gain_db: f32) {
        if let Some(slot) = self.sounds.get_mut(self.len) {
            *slot = (sound, gain_db);
            self.len += 1;
        }
    }
}

#[derive(Clone, Copy, Debug)]
enum Kind {
    Sound(AssetId),
    Random {
        avoid: u8,
    },
    Sequence,
    Layer,
    /// The case played with no value, or one it hasn't, by its place among the cases.
    Switch {
        on: Name,
        default: usize,
    },
    Blend {
        on: Name,
    },
}

/// What a node's parent reads of it.
#[derive(Clone, Copy, Debug)]
enum Edge {
    None,
    /// Its chance in a random node.
    Weight(u32),
    /// Its case in a switch.
    Case(Name),
    /// Its point in a blend.
    At(f32),
}

#[derive(Clone, Copy, Debug)]
struct Node {
    kind: Kind,
    edge: Edge,
    /// One past the last of the nodes under it.
    end: usize,
}

/// What a node remembers from start to start.
#[derive(Clone, Copy, Debug, Default)]
struct Memory {
    /// A random node's last picks, by place among the nodes under it, the latest first.
    recent: [u16; MAX_AVOID_REPEAT as usize],
    /// How many of `recent` there are yet.
    kept: u8,
    /// A sequence's next, by place.
    next: u16,
}

/// A cue's tree, laid out for starts. See the [module docs](self).
#[derive(Clone, Debug)]
pub(crate) struct CuePlan {
    nodes: Vec<Node>,
    /// What each node remembers from the starts let in.
    memory: Vec<Memory>,
    /// The same, moved on by the last pick until [`Self::settle`] keeps it or puts it back.
    pending: Vec<Memory>,
    /// The nodes the last pick moved on: each at most once, so never more than the nodes.
    touched: Vec<usize>,
}

impl CuePlan {
    /// The plan of the tree under `root`, remembering nothing yet. Load-time work.
    pub(crate) fn new(root: &CueNode) -> Self {
        let mut nodes = Vec::new();
        lay_out(root, Edge::None, &mut nodes);
        let memory = vec![Memory::default(); nodes.len()];
        let touched = Vec::with_capacity(nodes.len());
        Self { nodes, pending: memory.clone(), memory, touched }
    }

    /// What a start plays, with `number` its pick's draw and asked for with `parameters`. The random nodes and
    /// sequences it reaches remember what they played only once [`Self::settle`] says the start was let in.
    /// Allocates nothing.
    pub(crate) fn pick(&mut self, number: u32, parameters: &CueParameters) -> Picks {
        self.settle(false);
        let mut walk = Walk {
            nodes: &self.nodes,
            memory: &mut self.pending,
            touched: &mut self.touched,
            parameters,
            numbers: Numbers { first: Some(number), rest: CueRng::new(u64::from(number)) },
            picks: Picks::NONE,
        };
        if !walk.nodes.is_empty() {
            walk.walk(0, 0.0);
        }
        walk.picks
    }

    /// Keeps what the last pick's nodes remember if its start was `let_in`, and otherwise forgets it, so a start
    /// refused moves no sequence on and counts toward no avoid-repeat.
    pub(crate) fn settle(&mut self, let_in: bool) {
        for &at in &self.touched {
            if let_in {
                self.memory[at] = self.pending[at];
            } else {
                self.pending[at] = self.memory[at];
            }
        }
        self.touched.clear();
    }
}

fn lay_out(node: &CueNode, edge: Edge, nodes: &mut Vec<Node>) {
    let at = nodes.len();
    let kind = match node {
        CueNode::Sound(sound) => Kind::Sound(*sound),
        CueNode::Random(random) => Kind::Random { avoid: random.avoid_repeat },
        CueNode::Sequence(_) => Kind::Sequence,
        CueNode::Layer(_) => Kind::Layer,
        CueNode::Switch(switch) => Kind::Switch {
            on: Name::new(&switch.on),
            default: switch.cases.iter().position(|case| case.when == switch.default).unwrap_or(0),
        },
        CueNode::Blend(blend) => Kind::Blend { on: Name::new(&blend.on) },
    };
    nodes.push(Node { kind, edge, end: at + 1 });
    match node {
        CueNode::Sound(_) => {}
        CueNode::Random(random) => {
            for (index, child) in random.of.iter().enumerate() {
                let weight = random.weights.get(index).map_or(1, |&weight| u32::from(weight));
                lay_out(child, Edge::Weight(weight), nodes);
            }
        }
        CueNode::Sequence(children) | CueNode::Layer(children) => {
            for child in children {
                lay_out(child, Edge::None, nodes);
            }
        }
        CueNode::Switch(switch) => {
            for case in &switch.cases {
                lay_out(&case.play, Edge::Case(Name::new(&case.when)), nodes);
            }
        }
        CueNode::Blend(blend) => {
            for point in &blend.points {
                lay_out(&point.play, Edge::At(point.at), nodes);
            }
        }
    }
    nodes[at].end = nodes.len();
}

/// The numbers a start's random nodes take: its pick's draw, then a stream seeded by it.
struct Numbers {
    first: Option<u32>,
    rest: CueRng,
}

impl Numbers {
    fn next(&mut self) -> u32 {
        self.first.take().unwrap_or_else(|| self.rest.next_u32())
    }
}

/// The places of the nodes right under the node at `at`, in order.
fn children(nodes: &[Node], at: usize) -> impl Iterator<Item = usize> + '_ {
    let end = nodes[at].end;
    let within = move |child: usize| Some(child).filter(|&child| child < end);
    std::iter::successors(within(at + 1), move |&child| within(nodes[child].end))
}

struct Walk<'a> {
    nodes: &'a [Node],
    memory: &'a mut [Memory],
    touched: &'a mut Vec<usize>,
    parameters: &'a CueParameters,
    numbers: Numbers,
    picks: Picks,
}

impl Walk<'_> {
    /// Plays the node at `at`, its sounds `gain_db` down.
    fn walk(&mut self, at: usize, gain_db: f32) {
        let nodes = self.nodes;
        match nodes[at].kind {
            Kind::Sound(sound) => self.picks.push(sound, gain_db),
            Kind::Layer => {
                for child in children(nodes, at) {
                    self.walk(child, gain_db);
                }
            }
            Kind::Random { avoid } => {
                if let Some(child) = self.pick_random(at, avoid) {
                    self.walk(child, gain_db);
                }
            }
            Kind::Sequence => {
                let count = children(nodes, at).count();
                // A walk reaches each node once, so each is touched at most once.
                self.touched.push(at);
                let memory = &mut self.memory[at];
                let place = usize::from(memory.next) % count.max(1);
                memory.next = ((place + 1) % count.max(1)) as u16;
                if let Some(child) = children(nodes, at).nth(place) {
                    self.walk(child, gain_db);
                }
            }
            Kind::Switch { on, default } => {
                let case = match self.parameters.get(on) {
                    Some(ParameterValue::Case(case)) => children(nodes, at)
                        .find(|&child| matches!(nodes[child].edge, Edge::Case(name) if name == case)),
                    _ => None,
                };
                if let Some(child) = case.or_else(|| children(nodes, at).nth(default)) {
                    self.walk(child, gain_db);
                }
            }
            Kind::Blend { on } => self.blend(at, on, gain_db),
        }
    }

    /// The random node at `at`'s pick, by weight among the nodes under it but its last `avoid`, remembered.
    fn pick_random(&mut self, at: usize, avoid: u8) -> Option<usize> {
        let nodes = self.nodes;
        let count = children(nodes, at).count();
        self.touched.push(at);
        let memory = &mut self.memory[at];
        // Never so many that nothing is left to pick.
        let avoid = usize::from(avoid.min(MAX_AVOID_REPEAT))
            .min(count.saturating_sub(1))
            .min(usize::from(memory.kept));
        let recent = &memory.recent[..avoid];
        let weight = |child: usize| match nodes[child].edge {
            Edge::Weight(weight) => weight,
            _ => 1,
        };
        let open = || children(nodes, at).enumerate().filter(|(place, _)| !recent.contains(&(*place as u16)));
        let total: u32 = open().map(|(_, child)| weight(child)).sum();
        if total == 0 {
            return None;
        }
        let mut target = self.numbers.next() % total;
        let (place, child) = open().find(|&(_, child)| {
            let weight = weight(child);
            let here = target < weight;
            target = target.wrapping_sub(weight);
            here
        })?;
        memory.recent.copy_within(..usize::from(MAX_AVOID_REPEAT) - 1, 1);
        memory.recent[0] = place as u16;
        memory.kept = (memory.kept + 1).min(MAX_AVOID_REPEAT);
        Some(child)
    }

    /// Plays the blend at `at`: the points either side of `on`'s value, by equal power, or the nearest alone.
    fn blend(&mut self, at: usize, on: Name, gain_db: f32) {
        let nodes = self.nodes;
        let point = |child: usize| match nodes[child].edge {
            Edge::At(point) => point,
            _ => 0.0,
        };
        // With no value, the first point.
        let value = match self.parameters.get(on) {
            Some(ParameterValue::Number(value)) if value.is_finite() => value,
            _ => f32::NEG_INFINITY,
        };
        // The last point below the value, and the first at or above it.
        let mut below = None;
        let above = children(nodes, at).find(|&child| {
            let reached = value <= point(child);
            if !reached {
                below = Some(child);
            }
            reached
        });
        match (below, above) {
            (Some(only), None) | (None, Some(only)) => self.walk(only, gain_db),
            (Some(below), Some(above)) => {
                let (from, to) = (point(below), point(above));
                let angle = ((value - from) / (to - from)).clamp(0.0, 1.0) * std::f32::consts::FRAC_PI_2;
                for (node, share) in [(below, angle.cos()), (above, angle.sin())] {
                    let share_db = if share > 0.0 { 20.0 * share.log10() } else { SILENCE_DB };
                    if share_db > SILENCE_DB {
                        self.walk(node, gain_db + share_db);
                    }
                }
            }
            (None, None) => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use sapphire_assets::cue::{BlendNode, BlendPoint, RandomNode, SwitchCase, SwitchNode};

    use super::*;

    fn sound(n: u8) -> AssetId {
        AssetId::from_bytes([n; 16])
    }

    fn sounds(picks: &Picks) -> Vec<AssetId> {
        picks.as_slice().iter().map(|&(sound, _)| sound).collect()
    }

    /// A start let in: what it picks, remembered.
    fn play(plan: &mut CuePlan, number: u32, parameters: &CueParameters) -> Picks {
        let picks = plan.pick(number, parameters);
        plan.settle(true);
        picks
    }

    fn random(of: Vec<CueNode>, weights: Vec<u16>, avoid_repeat: u8) -> CueNode {
        CueNode::Random(RandomNode { of, weights, avoid_repeat })
    }

    /// A list read as a random node picks the sound its number's remainder names, as version 2 did.
    #[test]
    fn an_even_random_node_picks_by_the_remainder() {
        let list: Vec<AssetId> = (1..=5).map(sound).collect();
        let mut plan = CuePlan::new(&CueNode::one_of(list.clone()));
        let mut rng = CueRng::new(9);
        for _ in 0..500 {
            let number = rng.next_u32();
            assert_eq!(
                sounds(&play(&mut plan, number, &CueParameters::NONE)),
                [list[number as usize % list.len()]]
            );
        }
    }

    /// Over 1,000 draws a random node never picks one of its last `avoid_repeat`, by weight or evenly, and still
    /// picks every one it has.
    #[test]
    fn avoid_repeat_holds_over_a_thousand_draws() {
        for (count, weights, avoid) in
            [(2, vec![], 1), (5, vec![], 1), (5, vec![], 3), (5, vec![9, 1, 1, 1, 1], 2), (9, vec![], 8)]
        {
            let of = (0..count).map(|n| CueNode::Sound(sound(n))).collect();
            let mut plan = CuePlan::new(&random(of, weights.clone(), avoid));
            let mut rng = CueRng::new(u64::from(avoid) * 31 + u64::from(count));
            let draws: Vec<AssetId> = (0..1000)
                .map(|_| sounds(&play(&mut plan, rng.next_u32(), &CueParameters::NONE))[0])
                .collect();
            for (at, pick) in draws.iter().enumerate() {
                let window = &draws[at.saturating_sub(usize::from(avoid))..at];
                assert!(
                    !window.contains(pick),
                    "{count} of {weights:?}, avoiding {avoid}: draw {at} repeats"
                );
            }
            for n in 0..count {
                assert!(draws.contains(&sound(n)), "{count} of {weights:?}, avoiding {avoid}: never {n}");
            }
        }
    }

    #[test]
    fn weights_share_the_picks_out() {
        let mut plan =
            CuePlan::new(&random(vec![CueNode::Sound(sound(1)), CueNode::Sound(sound(2))], vec![3, 1], 0));
        let mut rng = CueRng::new(4);
        let firsts = (0..4000)
            .filter(|_| sounds(&play(&mut plan, rng.next_u32(), &CueParameters::NONE))[0] == sound(1))
            .count();
        assert!((2900..3100).contains(&firsts), "{firsts} of 4000 at three to one");
    }

    #[test]
    fn a_sequence_goes_round_and_a_layer_plays_all() {
        let three = || (1..=3).map(|n| CueNode::Sound(sound(n))).collect::<Vec<_>>();
        let mut plan = CuePlan::new(&CueNode::Sequence(three()));
        let played: Vec<_> = (0..7).map(|n| sounds(&play(&mut plan, n, &CueParameters::NONE))[0]).collect();
        assert_eq!(played, [1, 2, 3, 1, 2, 3, 1].map(sound));
        let mut plan = CuePlan::new(&CueNode::Layer(three()));
        assert_eq!(sounds(&play(&mut plan, 0, &CueParameters::NONE)), [1, 2, 3].map(sound));
    }

    /// M6 U4a: a start refused after its pick moves no sequence on and counts toward no avoid-repeat.
    #[test]
    fn a_refused_start_is_forgotten() {
        let three = || (1..=3).map(|n| CueNode::Sound(sound(n))).collect::<Vec<_>>();
        let mut plan = CuePlan::new(&CueNode::Sequence(three()));
        let mut played = Vec::new();
        for n in 0..6 {
            let picks = plan.pick(n, &CueParameters::NONE);
            // Every other start is refused, and the next plays what it would have.
            plan.settle(n % 2 == 1);
            played.push(sounds(&picks)[0]);
        }
        assert_eq!(played, [1, 1, 2, 2, 3, 3].map(sound));

        // Avoid-repeat over two of two: a refused pick doesn't stop the next from taking it.
        let mut plan =
            CuePlan::new(&random(vec![CueNode::Sound(sound(1)), CueNode::Sound(sound(2))], vec![], 1));
        assert_eq!(sounds(&play(&mut plan, 0, &CueParameters::NONE)), [sound(1)]);
        assert_eq!(sounds(&plan.pick(1, &CueParameters::NONE)), [sound(2)]);
        plan.settle(false);
        assert_eq!(
            sounds(&play(&mut plan, 1, &CueParameters::NONE)),
            [sound(2)],
            "not barred by the refused"
        );
        assert_eq!(sounds(&play(&mut plan, 1, &CueParameters::NONE)), [sound(1)], "barred by the kept");
        // A pick never settled is forgotten by the next.
        let _ = plan.pick(0, &CueParameters::NONE);
        assert_eq!(sounds(&play(&mut plan, 0, &CueParameters::NONE)), [sound(2)]);
    }

    /// Two random nodes in a layer take different numbers: the first the draw, the second one made from it.
    #[test]
    fn random_nodes_after_the_first_take_numbers_made_from_the_draw() {
        let pair = || random((1..=8).map(|n| CueNode::Sound(sound(n))).collect(), Vec::new(), 0);
        let mut plan = CuePlan::new(&CueNode::Layer(vec![pair(), pair()]));
        let mut rng = CueRng::new(2);
        let mut differ = 0;
        for _ in 0..200 {
            let number = rng.next_u32();
            let picks = sounds(&play(&mut plan, number, &CueParameters::NONE));
            assert_eq!(picks[0], sound(1 + (number % 8) as u8));
            assert_eq!(picks[1], sound(1 + (CueRng::new(u64::from(number)).next_u32() % 8) as u8));
            differ += usize::from(picks[0] != picks[1]);
        }
        assert!(differ > 150, "{differ}");
    }

    #[test]
    fn a_switch_plays_the_case_asked_for_or_its_default() {
        let case = |when: &str, n| SwitchCase { when: when.to_owned(), play: CueNode::Sound(sound(n)) };
        let switch = CueNode::Switch(SwitchNode {
            on: "surface".to_owned(),
            cases: vec![case("grass", 1), case("stone", 2)],
            default: "stone".to_owned(),
        });
        let mut plan = CuePlan::new(&switch);
        let on = |case: &str| CueParameters::NONE.with(SURFACE, ParameterValue::Case(Name::new(case)));
        assert_eq!(sounds(&play(&mut plan, 0, &on("grass"))), [sound(1)]);
        assert_eq!(sounds(&play(&mut plan, 0, &on("stone"))), [sound(2)]);
        assert_eq!(sounds(&play(&mut plan, 0, &on("sand"))), [sound(2)], "a case it hasn't");
        assert_eq!(sounds(&play(&mut plan, 0, &CueParameters::NONE)), [sound(2)], "no value");
        let number = CueParameters::NONE.with(SURFACE, ParameterValue::Number(1.0));
        assert_eq!(sounds(&play(&mut plan, 0, &number)), [sound(2)], "a number is no case");
    }

    #[test]
    fn a_blend_crossfades_its_neighbours_by_equal_power() {
        let point = |at, n| BlendPoint { at, play: CueNode::Sound(sound(n)) };
        let blend = CueNode::Blend(BlendNode {
            on: "speed".to_owned(),
            points: vec![point(0.0, 1), point(4.0, 2), point(8.0, 3)],
        });
        let mut plan = CuePlan::new(&blend);
        let at = |plan: &mut CuePlan, speed| {
            play(plan, 0, &CueParameters::NONE.with(Name::new("speed"), ParameterValue::Number(speed)))
        };
        assert_eq!(at(&mut plan, -3.0).as_slice(), [(sound(1), 0.0)]);
        assert_eq!(at(&mut plan, 0.0).as_slice(), [(sound(1), 0.0)]);
        assert_eq!(sounds(&at(&mut plan, 4.0)), [sound(2)], "on a point, it alone");
        assert_eq!(at(&mut plan, 9.0).as_slice(), [(sound(3), 0.0)]);
        assert_eq!(
            play(&mut plan, 0, &CueParameters::NONE).as_slice(),
            [(sound(1), 0.0)],
            "no value: the first"
        );
        let half = at(&mut plan, 6.0);
        let [(lower, lower_db), (upper, upper_db)] = half.as_slice() else { panic!("{half:?}") };
        assert_eq!((*lower, *upper), (sound(2), sound(3)));
        assert!(
            (lower_db + 3.0103).abs() < 1e-3 && (upper_db + 3.0103).abs() < 1e-3,
            "{lower_db} {upper_db}"
        );
        // Equal power all the way across.
        for step in 1..40 {
            let picks = at(&mut plan, 4.0 + 4.0 * step as f32 / 40.0);
            let power: f32 = picks.as_slice().iter().map(|(_, db)| 10f32.powf(db / 10.0)).sum();
            assert!((power - 1.0).abs() < 1e-4, "{step}: {power}");
        }
    }

    #[test]
    fn parameters_hold_four_and_a_set_replaces() {
        let mut parameters = CueParameters::NONE;
        for n in 0..MAX_CUE_PARAMETERS {
            assert!(parameters.set(Name::new(&n.to_string()), ParameterValue::Number(n as f32)));
        }
        assert!(!parameters.set(Name::new("one more"), ParameterValue::Number(9.0)));
        assert!(parameters.set(Name::new("2"), ParameterValue::Number(7.0)));
        assert_eq!(parameters.get(Name::new("2")), Some(ParameterValue::Number(7.0)));
        assert_eq!(parameters.get(Name::new("one more")), None);
    }

    /// An unchecked tree plays what it can and never panics: nothing to pick plays nothing.
    #[test]
    fn a_tree_with_nothing_to_pick_plays_nothing() {
        let mut plan = CuePlan::new(&CueNode::one_of(Vec::new()));
        assert_eq!(play(&mut plan, 3, &CueParameters::NONE).as_slice(), []);
        let mut plan = CuePlan::new(&random(vec![CueNode::Sound(sound(1))], vec![0], 4));
        assert_eq!(play(&mut plan, 3, &CueParameters::NONE).as_slice(), []);
        // An empty node last of all.
        let mut plan =
            CuePlan::new(&CueNode::Layer(vec![CueNode::Sound(sound(1)), CueNode::Sequence(Vec::new())]));
        assert_eq!(sounds(&play(&mut plan, 3, &CueParameters::NONE)), [sound(1)]);
        let mut plan = CuePlan::new(&CueNode::Layer((0..12).map(|n| CueNode::Sound(sound(n))).collect()));
        assert_eq!(play(&mut plan, 3, &CueParameters::NONE).as_slice().len(), MAX_START_SOUNDS);
    }
}
