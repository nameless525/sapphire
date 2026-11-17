//! A species' skeleton at an age, grown by space colonization: see the [tree module's docs](super).
//!
//! Every number comes from the spec, the age's seed and the samples' `Rng`; the arithmetic is the four operations,
//! `sqrt` and `sapphire_math::det`, in a fixed order, so a skeleton is the same on every machine.

use std::f32::consts::TAU;

use glam::{Vec2, Vec3};
use sapphire_math::det;

use super::{CrownShape, TreeSpecies};
use crate::samples::noise::Rng;

/// A point of the skeleton.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Node {
    pub position: Vec3,
    /// The node it grew from: always an earlier one. The root has none.
    pub parent: Option<u32>,
    /// The branch's radius here, in metres.
    pub radius: f32,
}

/// A grown tree: its nodes, the root first, each after its parent.
#[derive(Clone, Debug, PartialEq)]
pub struct Skeleton {
    pub nodes: Vec<Node>,
    /// The crown's middle, which leaf cards face away from.
    pub center: Vec3,
    /// Metres, root to crown.
    pub height: f32,
}

impl Skeleton {
    /// Each node's children, in the order they grew.
    pub fn children(&self) -> Vec<Vec<u32>> {
        let mut children = vec![Vec::new(); self.nodes.len()];
        for (index, node) in self.nodes.iter().enumerate() {
            if let Some(parent) = node.parent {
                children[parent as usize].push(index as u32);
            }
        }
        children
    }

    /// The nodes nothing grew from, the twig tips, in order.
    pub fn tips(&self) -> Vec<u32> {
        let children = self.children();
        (0..self.nodes.len() as u32).filter(|&node| children[node as usize].is_empty()).collect()
    }

    /// The direction the branch runs at `node`: from its parent to it (up, at the root).
    pub fn direction(&self, node: u32) -> Vec3 {
        let here = self.nodes[node as usize];
        here.parent
            .map(|parent| (here.position - self.nodes[parent as usize].position).normalize_or_zero())
            .filter(|direction| *direction != Vec3::ZERO)
            .unwrap_or(Vec3::Y)
    }
}

/// How many rounds of growth an age may take, in multiples of its height in steps: a crown is spent long
/// before.
const ROUNDS_PER_STEP: u32 = 4;

/// A node's child within this share of a step of where it would grow again: it grew that way already, pulled
/// evenly by points it can't reach, so it doesn't grow there twice.
const SAME_PLACE: f32 = 0.1;

/// Smoothing passes over the grown skeleton: space colonization's steps zigzag where two points pull
/// a branch in turn.
const SMOOTHING: u32 = 2;

/// The share of the height the trunk's foot flares over.
const FLARE_HEIGHT: f32 = 0.08;

/// Grows `species` at age `age`.
pub fn grow(species: &TreeSpecies, age: usize) -> Skeleton {
    let height = species.ages[age].height;
    let growth = species.growth;
    let step = height / growth.steps as f32;
    let mut rng = Rng(species.age_seed(age));
    let (points, center) = crown_points(species, age, &mut rng);

    // The trunk, straight to the leader's height, leaning a little more toward its top.
    let lean = det::vec2_from_angle(rng.range(0.0, TAU)) * (growth.lean * height);
    let trunk_at = |y: f32| {
        let t = y / height;
        let offset = lean * (t * t);
        Vec3::new(offset.x, y, offset.y)
    };
    let mut nodes = vec![Node { position: Vec3::ZERO, parent: None, radius: 0.0 }];
    let leader = (growth.leader * height).max(step);
    let mut y = 0.0;
    while y + step <= leader + 1e-4 {
        y += step;
        let parent = nodes.len() as u32 - 1;
        nodes.push(Node { position: trunk_at(y), parent: Some(parent), radius: 0.0 });
    }

    // Space colonization: each live point's nearest node within the influence distance.
    let (influence, kill) = (growth.influence * step, growth.kill * step);
    let (influence2, kill2) = (influence * influence, kill * kill);
    let mut alive = vec![true; points.len()];
    let mut nearest: Vec<Option<(u32, f32)>> = vec![None; points.len()];
    let mut children: Vec<Vec<u32>> = vec![Vec::new(); nodes.len()];
    for (index, node) in nodes.iter().enumerate().skip(1) {
        children[node.parent.expect("a trunk node's parent") as usize].push(index as u32);
    }
    let tropism = Vec3::Y * growth.tropism;
    meet(&nodes, 0, &points, &mut alive, &mut nearest, influence2, kill2);
    for _ in 0..growth.steps * ROUNDS_PER_STEP {
        let mut pull = vec![Vec3::ZERO; nodes.len()];
        let mut pulled = vec![false; nodes.len()];
        for (index, point) in points.iter().enumerate() {
            let Some((node, _)) = nearest[index].filter(|_| alive[index]) else { continue };
            pull[node as usize] += (*point - nodes[node as usize].position).normalize_or_zero();
            pulled[node as usize] = true;
        }
        let first = nodes.len();
        for node in 0..first {
            if !pulled[node] {
                continue;
            }
            let direction = (pull[node].normalize_or_zero() + tropism).normalize_or_zero();
            if direction == Vec3::ZERO {
                continue;
            }
            let position = nodes[node].position + direction * step;
            let near = |child: &u32| nodes[*child as usize].position.distance(position) < SAME_PLACE * step;
            if children[node].iter().any(near) {
                continue;
            }
            children[node].push(nodes.len() as u32);
            children.push(Vec::new());
            nodes.push(Node { position, parent: Some(node as u32), radius: 0.0 });
        }
        if nodes.len() == first {
            break;
        }
        meet(&nodes, first, &points, &mut alive, &mut nearest, influence2, kill2);
        if !alive.contains(&true) {
            break;
        }
    }

    smooth(&mut nodes, &children);
    let wood = &species.wood;
    // The pipe model, tips up: every child comes after its parent.
    let mut sum = vec![0.0f32; nodes.len()];
    for index in (0..nodes.len()).rev() {
        let radius =
            if children[index].is_empty() { wood.tip } else { det::pow(sum[index], 1.0 / wood.exponent) };
        nodes[index].radius = radius;
        if let Some(parent) = nodes[index].parent {
            sum[parent as usize] += det::pow(radius, wood.exponent);
        }
    }
    let flare_top = FLARE_HEIGHT * height;
    for node in &mut nodes {
        let below = (1.0 - node.position.y / flare_top).max(0.0);
        node.radius *= 1.0 + wood.flare * below * below;
    }
    Skeleton { nodes, center, height }
}

/// Updates each live point's nearest node with the nodes from `first` on, and spends the points a node has
/// come within the kill distance of.
fn meet(
    nodes: &[Node],
    first: usize,
    points: &[Vec3],
    alive: &mut [bool],
    nearest: &mut [Option<(u32, f32)>],
    influence2: f32,
    kill2: f32,
) {
    for (index, point) in points.iter().enumerate() {
        if !alive[index] {
            continue;
        }
        for (node, at) in nodes.iter().enumerate().skip(first) {
            let d2 = point.distance_squared(at.position);
            if d2 <= kill2 {
                alive[index] = false;
                break;
            }
            if d2 < influence2 && nearest[index].is_none_or(|(_, best)| d2 < best) {
                nearest[index] = Some((node as u32, d2));
            }
        }
    }
}

/// Moves each node between its root and its tips toward the middle of its parent and its straightest child,
/// [`SMOOTHING`] times, all at once each time: the root and the tips stay.
fn smooth(nodes: &mut [Node], children: &[Vec<u32>]) {
    for _ in 0..SMOOTHING {
        let before: Vec<Vec3> = nodes.iter().map(|node| node.position).collect();
        for (index, node) in nodes.iter_mut().enumerate() {
            let Some(parent) = node.parent else { continue };
            let here = before[index];
            let from = before[parent as usize];
            let along = (here - from).normalize_or_zero();
            // The child that carries on most nearly straight; the first of equals.
            let mut next: Option<(Vec3, f32)> = None;
            for &child in &children[index] {
                let to = before[child as usize];
                let straight = (to - here).normalize_or_zero().dot(along);
                if next.is_none_or(|(_, best)| straight > best) {
                    next = Some((to, straight));
                }
            }
            let Some((to, _)) = next else { continue };
            node.position = here * 0.5 + (from + to) * 0.25;
        }
    }
}

/// The crown's attraction points for `species` at `age`, and the crown's middle.
fn crown_points(species: &TreeSpecies, age: usize, rng: &mut Rng) -> (Vec<Vec3>, Vec3) {
    let height = species.ages[age].height;
    let crown = species.crown;
    let (bottom, top) = (crown.base * height, height);
    let radius = crown.radius * height;
    let center = match crown.shape {
        CrownShape::Ellipsoid => Vec3::new(0.0, (bottom + top) * 0.5, 0.0),
        // A cone's centroid, a quarter of the way up.
        CrownShape::Cone => Vec3::new(0.0, bottom + (top - bottom) * 0.25, 0.0),
    };
    // How far out `p` is, 0 at the crown's axis or middle and 1 on its surface; over 1 outside.
    let reach = |p: Vec3| -> f32 {
        match crown.shape {
            CrownShape::Ellipsoid => {
                let half = (top - bottom) * 0.5;
                let q = Vec3::new(p.x / radius, (p.y - center.y) / half, p.z / radius);
                q.length()
            }
            CrownShape::Cone => {
                if p.y < bottom || p.y > top {
                    return 2.0;
                }
                let width = radius * (top - p.y) / (top - bottom);
                if width <= 0.0 { 2.0 } else { Vec2::new(p.x, p.z).length() / width }
            }
        }
    };
    let count = species.ages[age].points as usize;
    let mut points = Vec::with_capacity(count);
    // Rejection sampling in the crown's bounds: a point is kept more often the nearer the surface it lies,
    // by the crown's shell. Every candidate draws its four numbers, kept or not.
    while points.len() < count {
        let p = Vec3::new(rng.range(-radius, radius), rng.range(bottom, top), rng.range(-radius, radius));
        let keep = rng.next();
        let out = reach(p);
        if out > 1.0 {
            continue;
        }
        if keep > 1.0 - crown.shell + crown.shell * out * out * out {
            continue;
        }
        points.push(p);
    }
    (points, center)
}
