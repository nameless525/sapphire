//! A figure's skeleton and shapes, made into skinned primitives, one a material (`build`).
//!
//! Everything is placed in the model's space at the bind pose, so a vertex's place is where it's
//! drawn at rest, and each joint's inverse bind is the inverse of its rest frame. Shapes are smooth
//! within themselves (each vertex's normal is the area-weighted sum of its faces') unless `faceted`;
//! the ends of a shell and the sides of a slab always shade flat, as a cut edge does. Every triangle's
//! UVs lie in its region's cell of its material's maps: U at the cell's middle, or across the cell
//! where the figure's maps unwrap it, and V down the cell as the spec says.

use std::f32::consts::TAU;

use glam::{Mat3, Vec3};
use sapphire_assets::rig::ExpressionPreset;
use sapphire_math::det;
use serde_json::json;

use super::{Chain, Figure, FigureError, JointSpec, Loft, MaterialGroup, Move, Ring, Shape, Slab, Surface};
use crate::gltf_writer::{Geometry, GltfWriter, InfluenceSet, MorphGeometry};

/// How far a leaf joint's bone reaches along its +Y, for skinning, in metres.
const LEAF_BONE: f32 = 0.05;

/// Unwrapped, a shape keeps this many texels of its map clear of its cell's sides.
const CELL_MARGIN_TEXELS: f32 = 1.5;

/// The skeleton at rest.
#[derive(Clone, Debug)]
pub struct Rig {
    pub names: Vec<String>,
    pub parents: Vec<Option<usize>>,
    /// Each joint's place and frame in the model's space.
    pub at: Vec<Vec3>,
    pub frames: Vec<Mat3>,
    /// Where each joint's bone ends, for skinning.
    pub tails: Vec<Vec3>,
}

impl Rig {
    /// The joint called `name`.
    pub fn find(&self, name: &str) -> Option<usize> {
        self.names.iter().position(|known| known == name)
    }
}

/// One material's primitive.
#[derive(Clone, Debug)]
pub(crate) struct Primitive {
    /// The material's slot name.
    pub material: String,
    pub geometry: Geometry,
}

/// A figure, built: its skeleton and its primitives, one a material in [`Figure::material_groups`]'
/// order. A face primitive carries the expressions' morph targets, named by `morphs`.
#[derive(Clone, Debug)]
pub(crate) struct Built {
    pub name: String,
    pub rig: Rig,
    pub primitives: Vec<Primitive>,
    pub morphs: Vec<String>,
}

/// One copy of a chain: its name as a spring, and its joints from the root.
#[derive(Clone, Debug, PartialEq)]
pub struct ChainCopy {
    pub chain: String,
    pub name: String,
    pub joints: Vec<String>,
}

/// A frame whose +Y points along `direction`, rolled so its +Z leans toward the model's +Z, or toward
/// +Y for a bone that runs along Z.
fn aimed(direction: Vec3) -> Mat3 {
    let y = direction.normalize();
    let toward = if y.z.abs() > 0.9 { Vec3::Y } else { Vec3::Z };
    let x = y.cross(toward).normalize();
    let z = x.cross(y);
    Mat3::from_cols(x, y, z)
}

/// `point` turned `degrees` about the model's Y axis, from +X toward −Z.
fn turned(point: Vec3, degrees: f32) -> Vec3 {
    let (sin, cos) = det::sin_cos(degrees.to_radians());
    Vec3::new(point.x * cos + point.z * sin, point.y, point.z * cos - point.x * sin)
}

/// Every copy of every chain, in the spec's order: see [`Chain`].
pub fn chain_copies(figure: &Figure) -> Result<Vec<(ChainCopy, Vec<Vec3>)>, FigureError> {
    let mut copies = Vec::new();
    for chain in &figure.chains {
        let chain_error = |why: &str| FigureError::Joint { joint: chain.name.clone(), why: why.to_owned() };
        if chain.joints < 2 {
            return Err(chain_error("a chain has two joints or more"));
        }
        let count = match chain.around {
            Some(around) if around.count < 2 => return Err(chain_error("`around` makes two copies or more")),
            Some(around) => around.count,
            None => 1,
        };
        for copy in 0..count {
            let degrees = chain.around.map_or(0.0, |around| around.step * copy as f32);
            let (from, to) = (turned(Vec3::from(chain.from), degrees), turned(Vec3::from(chain.to), degrees));
            let name = if count == 1 { chain.name.clone() } else { format!("{}_{copy:02}", chain.name) };
            let last = (chain.joints - 1) as f32;
            let places: Vec<Vec3> =
                (0..chain.joints).map(|k| from + (to - from) * (k as f32 / last)).collect();
            let joints = (0..chain.joints).map(|k| format!("{name}_{k}")).collect();
            copies.push((ChainCopy { chain: chain.name.clone(), name, joints }, places));
        }
    }
    Ok(copies)
}

/// The figure's joints: its own, then its chains', level by level.
pub fn all_joints(figure: &Figure) -> Result<Vec<JointSpec>, FigureError> {
    let mut joints = figure.joints.clone();
    let copies = chain_copies(figure)?;
    let parent_of = |chain: &Chain| -> Result<(), FigureError> {
        let chain_error = |why: String| FigureError::Joint { joint: chain.name.clone(), why };
        let parent = figure
            .joints
            .iter()
            .position(|joint| joint.name == chain.parent)
            .ok_or_else(|| chain_error(format!("hangs from {:?}, which isn't a joint", chain.parent)))?;
        let has_child =
            figure.joints.iter().any(|joint| joint.parent.as_deref() == Some(chain.parent.as_str()));
        if figure.joints[parent].aim.is_none() && !has_child {
            // A leaf aims at its first child: the chain would turn it.
            return Err(chain_error(format!("hangs from {:?}, a leaf without an aim", chain.parent)));
        }
        Ok(())
    };
    for chain in &figure.chains {
        parent_of(chain)?;
    }
    let deepest = figure.chains.iter().map(|chain| chain.joints).max().unwrap_or(0) as usize;
    for level in 0..deepest {
        for (copy, places) in &copies {
            let Some(&at) = places.get(level) else { continue };
            let chain = figure.chains.iter().find(|chain| chain.name == copy.chain).expect("its own chain");
            let parent = if level == 0 { chain.parent.clone() } else { copy.joints[level - 1].clone() };
            joints.push(JointSpec {
                name: copy.joints[level].clone(),
                parent: Some(parent),
                at: at.to_array(),
                aim: None,
            });
        }
    }
    Ok(joints)
}

fn rig(figure: &Figure) -> Result<Rig, FigureError> {
    let joint_error = |joint: &str, why: String| FigureError::Joint { joint: joint.to_owned(), why };
    let specs = all_joints(figure)?;
    if specs.is_empty() {
        return Err(FigureError::Spec("a figure needs joints".to_owned()));
    }
    let mut names: Vec<String> = Vec::with_capacity(specs.len());
    let mut parents = Vec::with_capacity(specs.len());
    for joint in &specs {
        if names.contains(&joint.name) {
            return Err(joint_error(&joint.name, "is listed twice".to_owned()));
        }
        let parent = match &joint.parent {
            None => None,
            Some(parent) => Some(names.iter().position(|known| known == parent).ok_or_else(|| {
                joint_error(&joint.name, format!("its parent {parent:?} isn't listed before it"))
            })?),
        };
        if joint.at.iter().any(|value| !value.is_finite()) {
            return Err(joint_error(&joint.name, "isn't at a finite place".to_owned()));
        }
        names.push(joint.name.clone());
        parents.push(parent);
    }
    let at: Vec<Vec3> = specs.iter().map(|joint| Vec3::from(joint.at)).collect();
    let first_child = |index: usize| parents.iter().position(|&parent| parent == Some(index));
    let mut frames: Vec<Mat3> = Vec::with_capacity(at.len());
    let mut tails = Vec::with_capacity(at.len());
    for (index, joint) in specs.iter().enumerate() {
        let target = joint.aim.map(Vec3::from).or_else(|| first_child(index).map(|child| at[child]));
        let frame = match target {
            Some(target) => {
                let direction = target - at[index];
                if direction.length() < 1e-4 {
                    return Err(joint_error(&joint.name, "aims at where it stands".to_owned()));
                }
                aimed(direction)
            }
            None => parents[index].map_or(Mat3::IDENTITY, |parent| frames[parent]),
        };
        tails.push(match first_child(index) {
            Some(child) if joint.aim.is_none() => at[child],
            _ => at[index] + frame.y_axis * LEAF_BONE,
        });
        frames.push(frame);
    }
    Ok(Rig { names, parents, at, frames, tails })
}

/// One shape's vertices before they join the primitive.
#[derive(Default)]
struct Piece {
    positions: Vec<Vec3>,
    normals: Vec<Vec3>,
    /// U across the region's cell, 0 to 1, where the figure's maps unwrap it.
    u: Vec<f32>,
    /// V down the region's cell.
    v: Vec<f32>,
    indices: Vec<u32>,
}

impl Piece {
    fn push(&mut self, position: Vec3, normal: Vec3, u: f32, v: f32) -> u32 {
        self.positions.push(position);
        self.normals.push(normal);
        self.u.push(u);
        self.v.push(v);
        (self.positions.len() - 1) as u32
    }

    /// Faces sharing vertices, each vertex's normal the area-weighted sum of its faces'.
    fn smooth(&mut self, first_vertex: usize, first_index: usize) {
        let mut sums = vec![Vec3::ZERO; self.positions.len() - first_vertex];
        for triangle in self.indices[first_index..].as_chunks::<3>().0 {
            let [a, b, c] = triangle.map(|corner| corner as usize);
            let normal = (self.positions[b] - self.positions[a]).cross(self.positions[c] - self.positions[a]);
            for corner in [a, b, c] {
                sums[corner - first_vertex] += normal;
            }
        }
        for (offset, sum) in sums.into_iter().enumerate() {
            let index = first_vertex + offset;
            self.normals[index] = sum.try_normalize().unwrap_or(self.normals[index]);
        }
    }

    /// The faces from `first_index` on, each given vertices of its own and its own normal.
    fn facet(&mut self, first_index: usize) {
        let triangles: Vec<u32> = self.indices.split_off(first_index);
        for triangle in triangles.as_chunks::<3>().0 {
            let [a, b, c] = triangle.map(|corner| corner as usize);
            let normal = (self.positions[b] - self.positions[a])
                .cross(self.positions[c] - self.positions[a])
                .try_normalize()
                .unwrap_or(Vec3::Y);
            for corner in [a, b, c] {
                let index = self.push(self.positions[corner], normal, self.u[corner], self.v[corner]);
                self.indices.push(index);
            }
        }
    }

    /// A quad as two triangles, `a b c d` counter-clockwise from outside.
    fn quad(&mut self, [a, b, c, d]: [u32; 4]) {
        self.indices.extend([a, b, c, a, c, d]);
    }
}

/// A loft's frame: its axis, across, and axis × across.
fn loft_frame(loft: &Loft, axis: Vec3) -> Result<(Vec3, Vec3), String> {
    let across = match loft.across {
        Some(across) => Vec3::from(across),
        None if axis.x.abs() > 0.9 => Vec3::Z,
        None => Vec3::X,
    };
    let across = (across - axis * axis.dot(across)).try_normalize().ok_or("`across` runs along the axis")?;
    Ok((across, axis.cross(across)))
}

/// A loft's rings with `detail − 1` more between each two, in proportion.
fn finer_rings(rings: &[Ring], detail: u32) -> Vec<Ring> {
    if detail == 1 {
        return rings.to_vec();
    }
    let mut finer = Vec::with_capacity((rings.len() - 1) * detail as usize + 1);
    for pair in rings.windows(2) {
        let (a, b) = (&pair[0], &pair[1]);
        for step in 0..detail {
            let t = step as f32 / detail as f32;
            let mix = |x: f32, y: f32| x + (y - x) * t;
            finer.push(Ring {
                at: mix(a.at, b.at),
                radius: (mix(a.radius.0, b.radius.0), mix(a.radius.1, b.radius.1)),
                shift: (mix(a.shift.0, b.shift.0), mix(a.shift.1, b.shift.1)),
            });
        }
    }
    finer.push(rings[rings.len() - 1].clone());
    finer
}

fn loft(spec: &Loft, detail: u32, unwrap: bool) -> Result<Piece, String> {
    if spec.rings.len() < 2 {
        return Err("a loft needs two rings or more".to_owned());
    }
    if spec.rings.windows(2).any(|pair| pair[1].at < pair[0].at) {
        return Err("rings go by increasing `at`".to_owned());
    }
    if spec.sides < 3 {
        return Err("a loft needs three sides or more".to_owned());
    }
    if spec.arc.is_some() && spec.thickness.is_none() {
        return Err("an arc needs a thickness: a solid can't be open".to_owned());
    }
    let (from, to) = (Vec3::from(spec.from), Vec3::from(spec.to));
    let length = from.distance(to);
    if length < 1e-4 {
        return Err("`from` and `to` are the same place".to_owned());
    }
    let rings = finer_rings(&spec.rings, detail);
    let axis = (to - from) / length;
    let (across, around) = loft_frame(spec, axis)?;
    let sides = (spec.sides * detail) as usize;
    // An arc's last column is its own; a whole ring's wraps to its first, unless it's unwrapped,
    // where the seam has a column of each side's U.
    let (columns, closed) = match spec.arc {
        Some(_) => (sides + 1, false),
        None if unwrap => (sides + 1, true),
        None => (sides, true),
    };
    let angle = |column: usize| match spec.arc {
        Some((start, end)) => (start + (end - start) * column as f32 / sides as f32).to_radians(),
        None => TAU * (column % sides) as f32 / sides as f32,
    };
    let u_at = |column: usize| column as f32 / sides as f32;
    let point = |ring: usize, column: usize, inset: f32| {
        let ring = &rings[ring];
        let (sin, cos) = det::sin_cos(angle(column));
        let radius = ((ring.radius.0 - inset).max(0.0), (ring.radius.1 - inset).max(0.0));
        from + axis * (ring.at * length)
            + across * (radius.0 * cos + ring.shift.0)
            + around * (radius.1 * sin + ring.shift.1)
    };
    let v_at = |ring: usize| spec.v.0 + (spec.v.1 - spec.v.0) * rings[ring].at;
    let mut piece = Piece::default();

    // One surface: outward, or inward at an inset for a shell's inside.
    let surface = |piece: &mut Piece, inset: f32, inward: bool| -> Vec<u32> {
        let first = piece.positions.len() as u32;
        for ring in 0..rings.len() {
            for column in 0..columns {
                piece.push(point(ring, column, inset), Vec3::ZERO, u_at(column), v_at(ring));
            }
        }
        let first_index = piece.indices.len();
        let at = |ring: usize, column: usize| first + (ring * columns + column % columns) as u32;
        for ring in 0..rings.len() - 1 {
            for column in 0..sides {
                let quad =
                    [at(ring, column), at(ring, column + 1), at(ring + 1, column + 1), at(ring + 1, column)];
                piece.quad(if inward { [quad[3], quad[2], quad[1], quad[0]] } else { quad });
            }
        }
        if spec.faceted {
            piece.facet(first_index);
        } else {
            piece.smooth(first as usize, first_index);
            if closed && columns > sides {
                // The seam's two columns are one place: one normal.
                for ring in 0..rings.len() {
                    let (a, b) = (at(ring, 0) as usize, (first as usize) + ring * columns + sides);
                    let normal =
                        (piece.normals[a] + piece.normals[b]).try_normalize().unwrap_or(piece.normals[a]);
                    piece.normals[a] = normal;
                    piece.normals[b] = normal;
                }
            }
        }
        (0..rings.len() * columns).map(|offset| first + offset as u32).collect()
    };

    match spec.thickness {
        None => {
            surface(&mut piece, 0.0, false);
            if spec.caps {
                for (ring, outward) in [(0, -axis), (rings.len() - 1, axis)] {
                    let r = &rings[ring];
                    if r.radius.0 < 1e-4 && r.radius.1 < 1e-4 {
                        continue;
                    }
                    let first_index = piece.indices.len();
                    let center = from + axis * (r.at * length) + across * r.shift.0 + around * r.shift.1;
                    let middle = piece.push(center, outward, 0.5, v_at(ring));
                    let corners: Vec<u32> = (0..sides)
                        .map(|column| piece.push(point(ring, column, 0.0), outward, u_at(column), v_at(ring)))
                        .collect();
                    for column in 0..sides {
                        let (a, b) = (corners[column], corners[(column + 1) % sides]);
                        // Round the ring the faces point along the axis, so the start cap turns back.
                        piece.indices.extend(if ring == 0 { [middle, b, a] } else { [middle, a, b] });
                    }
                    piece.facet(first_index);
                }
            }
        }
        Some(thickness) => {
            if thickness.is_nan() || thickness <= 0.0 {
                return Err("a shell's thickness must be more than 0".to_owned());
            }
            let outer = surface(&mut piece, 0.0, false);
            let inner = surface(&mut piece, thickness, true);
            let first_index = piece.indices.len();
            let at = |ring: usize, column: usize| ring * columns + column;
            // The ends' rims, round the first ring and the last.
            for column in 0..sides {
                let next = if closed { (column + 1) % columns } else { column + 1 };
                let last = rings.len() - 1;
                piece.quad([
                    outer[at(0, next)],
                    outer[at(0, column)],
                    inner[at(0, column)],
                    inner[at(0, next)],
                ]);
                piece.quad([
                    outer[at(last, column)],
                    outer[at(last, next)],
                    inner[at(last, next)],
                    inner[at(last, column)],
                ]);
            }
            // An arc's edges, down its first column and its last.
            if !closed {
                for ring in 0..rings.len() - 1 {
                    let (a, b) = (at(ring, 0), at(ring + 1, 0));
                    piece.quad([outer[a], outer[b], inner[b], inner[a]]);
                    let (a, b) = (at(ring, sides), at(ring + 1, sides));
                    piece.quad([outer[b], outer[a], inner[a], inner[b]]);
                }
            }
            piece.facet(first_index);
        }
    }
    Ok(piece)
}

fn slab(slab: &Slab) -> Result<Piece, String> {
    if slab.outline.len() < 3 {
        return Err("an outline needs three corners or more".to_owned());
    }
    if slab.thickness.is_nan() || slab.thickness <= 0.0 {
        return Err("a slab's thickness must be more than 0".to_owned());
    }
    let (u, w) = (Vec3::from(slab.u), Vec3::from(slab.w));
    let u = u.try_normalize().ok_or("`u` has no length")?;
    let w = (w - u * u.dot(w)).try_normalize().ok_or("`w` runs along `u`")?;
    let normal = u.cross(w);
    let mut outline: Vec<(f32, f32)> = slab.outline.clone();
    let area: f32 = (0..outline.len())
        .map(|k| {
            let (a, b) = (outline[k], outline[(k + 1) % outline.len()]);
            a.0 * b.1 - b.0 * a.1
        })
        .sum();
    if area < 0.0 {
        outline.reverse();
    }
    let count = outline.len() as f32;
    let centroid = outline.iter().fold((0.0, 0.0), |sum, corner| (sum.0 + corner.0, sum.1 + corner.1));
    let centroid = (centroid.0 / count, centroid.1 / count);
    for k in 0..outline.len() {
        let (a, b) = (outline[k], outline[(k + 1) % outline.len()]);
        let wedge = (a.0 - centroid.0) * (b.1 - centroid.1) - (b.0 - centroid.0) * (a.1 - centroid.1);
        if wedge <= 0.0 {
            return Err("the outline isn't star-shaped about its centroid".to_owned());
        }
    }
    let (top, bottom) = outline
        .iter()
        .fold((f32::MIN, f32::MAX), |(top, bottom), corner| (top.max(corner.1), bottom.min(corner.1)));
    let (left, right) = outline
        .iter()
        .fold((f32::MAX, f32::MIN), |(left, right), corner| (left.min(corner.0), right.max(corner.0)));
    let v_at = |w_at: f32| slab.v.0 + (slab.v.1 - slab.v.0) * (top - w_at) / (top - bottom).max(1e-6);
    let u_at = |u_at: f32| (u_at - left) / (right - left).max(1e-6);
    let origin = Vec3::from(slab.origin);
    let place =
        |(a, b): (f32, f32), side: f32| origin + u * a + w * b + normal * (side * slab.thickness * 0.5);
    let mut piece = Piece::default();
    for (side, facing) in [(1.0, normal), (-1.0, -normal)] {
        let middle = piece.push(place(centroid, side), facing, u_at(centroid.0), v_at(centroid.1));
        let corners: Vec<u32> = outline
            .iter()
            .map(|&corner| piece.push(place(corner, side), facing, u_at(corner.0), v_at(corner.1)))
            .collect();
        for k in 0..corners.len() {
            let (a, b) = (corners[k], corners[(k + 1) % corners.len()]);
            piece.indices.extend(if side > 0.0 { [middle, a, b] } else { [middle, b, a] });
        }
    }
    let first_index = piece.indices.len();
    let front: Vec<u32> = outline
        .iter()
        .map(|&corner| piece.push(place(corner, 1.0), normal, u_at(corner.0), v_at(corner.1)))
        .collect();
    let back: Vec<u32> = outline
        .iter()
        .map(|&corner| piece.push(place(corner, -1.0), -normal, u_at(corner.0), v_at(corner.1)))
        .collect();
    for k in 0..outline.len() {
        let next = (k + 1) % outline.len();
        piece.quad([front[k], back[k], back[next], front[next]]);
    }
    piece.facet(first_index);
    Ok(piece)
}

/// The distance from `point` to the segment from `a` to `b`.
fn to_segment(point: Vec3, a: Vec3, b: Vec3) -> f32 {
    let along = b - a;
    let share = (point - a).dot(along) / along.length_squared().max(1e-12);
    point.distance(a + along * share.clamp(0.0, 1.0))
}

/// Up to four joints and their weights for `point` among `bones`: see [`Shape`].
fn influences(rig: &Rig, bones: &[usize], blend: f32, point: Vec3) -> ([u16; 4], [f32; 4]) {
    let distances: Vec<f32> =
        bones.iter().map(|&bone| to_segment(point, rig.at[bone], rig.tails[bone])).collect();
    let nearest = distances.iter().copied().fold(f32::MAX, f32::min);
    let mut weighted: Vec<(f32, usize)> = bones
        .iter()
        .zip(&distances)
        .map(|(&bone, &distance)| {
            let x = (distance - nearest) / blend;
            let falloff = 1.0 / (1.0 + x * x);
            (falloff * falloff, bone)
        })
        .collect();
    // Heaviest first, ties by joint, so the order never depends on the list's.
    weighted.sort_by(|a, b| b.0.total_cmp(&a.0).then(a.1.cmp(&b.1)));
    weighted.truncate(4);
    weighted.retain(|&(weight, _)| weight >= 0.02);
    let total: f32 = weighted.iter().map(|&(weight, _)| weight).sum();
    let (mut joints, mut weights) = ([0u16; 4], [0.0f32; 4]);
    for (slot, &(weight, bone)) in weighted.iter().enumerate() {
        joints[slot] = bone as u16;
        weights[slot] = weight / total;
    }
    (joints, weights)
}

/// The name a joint takes across X: `.l` and `.r` swapped.
fn mirrored(name: &str) -> String {
    if let Some(stem) = name.strip_suffix(".l") {
        format!("{stem}.r")
    } else if let Some(stem) = name.strip_suffix(".r") {
        format!("{stem}.l")
    } else {
        name.to_owned()
    }
}

/// The joints `name` names: itself, or with a trailing `*` every joint whose name starts with the
/// rest, in the skeleton's order.
fn bones_named(rig: &Rig, name: &str) -> Vec<usize> {
    match name.strip_suffix('*') {
        Some(prefix) => (0..rig.names.len()).filter(|&joint| rig.names[joint].starts_with(prefix)).collect(),
        None => rig.find(name).into_iter().collect(),
    }
}

/// How far `move_` displaces `point`, at full weight.
fn displacement(move_: &Move, point: Vec3) -> Vec3 {
    let falloff = |at: Vec3, radius: f32| {
        let share = point.distance_squared(at) / (radius * radius);
        if share < 1.0 { (1.0 - share) * (1.0 - share) } else { 0.0 }
    };
    let flip = |value: [f32; 3], across: bool| {
        let value = Vec3::from(value);
        if across { Vec3::new(-value.x, value.y, value.z) } else { value }
    };
    let (sides, mirror): (&[bool], bool) = match move_ {
        Move::Pull { mirror, .. } | Move::Squash { mirror, .. } => (&[false, true], *mirror),
    };
    let mut sum = Vec3::ZERO;
    for &across in &sides[..if mirror { 2 } else { 1 }] {
        sum += match *move_ {
            Move::Pull { at, radius, by, .. } => flip(by, across) * falloff(flip(at, across), radius),
            Move::Squash { at, radius, axis, scale, .. } => {
                let (at, axis) = (flip(at, across), flip(axis, across).normalize());
                axis * ((point - at).dot(axis) * (scale - 1.0) * falloff(at, radius))
            }
        };
    }
    sum
}

/// Checks a move's numbers.
fn check_move(move_: &Move) -> Result<(), String> {
    let (at, radius) = match move_ {
        Move::Pull { at, radius, by, .. } => {
            if by.iter().any(|value| !value.is_finite()) {
                return Err("a pull's `by` isn't finite".to_owned());
            }
            (at, *radius)
        }
        Move::Squash { at, radius, axis, scale, .. } => {
            if !scale.is_finite() || Vec3::from(*axis).try_normalize().is_none() {
                return Err("a squash needs an axis and a finite scale".to_owned());
            }
            (at, *radius)
        }
    };
    if at.iter().any(|value| !value.is_finite()) || radius.is_nan() || radius <= 0.0 {
        return Err("a move needs a finite place and a radius over 0".to_owned());
    }
    Ok(())
}

/// The expressions' morph targets over a face primitive's vertices, and their names.
fn morphs(figure: &Figure, geometry: &Geometry) -> Result<(Vec<MorphGeometry>, Vec<String>), FigureError> {
    let mut targets = Vec::with_capacity(figure.expressions.len());
    let mut names: Vec<String> = Vec::with_capacity(figure.expressions.len());
    for expression in &figure.expressions {
        let error = |why: String| FigureError::Spec(format!("expression {:?}: {why}", expression.preset));
        if ExpressionPreset::from_name(&expression.preset).is_none() {
            return Err(error("isn't one of VRM 1.0's presets".to_owned()));
        }
        if names.contains(&expression.preset) {
            return Err(error("is listed twice".to_owned()));
        }
        for move_ in &expression.moves {
            check_move(move_).map_err(error)?;
        }
        let positions: Vec<[f32; 3]> = geometry
            .positions
            .iter()
            .map(|&position| {
                let point = Vec3::from(position);
                expression.moves.iter().map(|move_| displacement(move_, point)).sum::<Vec3>().to_array()
            })
            .collect();
        if positions.iter().all(|delta| *delta == [0.0; 3]) {
            return Err(error("moves none of the face's vertices".to_owned()));
        }
        targets.push(MorphGeometry { positions, normals: Vec::new() });
        names.push(expression.preset.clone());
    }
    Ok((targets, names))
}

/// Builds `figure`'s skeleton and primitives.
pub(crate) fn build(figure: &Figure) -> Result<Built, FigureError> {
    let rig = rig(figure)?;
    let groups: Vec<MaterialGroup> = figure.material_groups()?;
    let maps = figure.checked_maps()?;
    if figure.detail == 0 {
        return Err(FigureError::Spec("`detail` is 1 or more".to_owned()));
    }
    // Each region's material and its cell there.
    let cell_of = |region: usize| -> (usize, usize, usize) {
        let (group, cells) = groups
            .iter()
            .enumerate()
            .find_map(|(index, group)| {
                group.regions.contains(&region).then_some((index, group.regions.len()))
            })
            .expect("every region has a material");
        let cell = groups[group].regions.iter().position(|&known| known == region).expect("in its group");
        (group, cell, cells)
    };
    let mut geometries: Vec<(Geometry, InfluenceSet)> = vec![Default::default(); groups.len()];
    for (index, shape) in figure.shapes.iter().enumerate() {
        let shape_error = |why: String| FigureError::Shape { index, why };
        let (region, bones, blend, mirror, piece) = match shape {
            Shape::Loft(spec) => (
                &spec.region,
                &spec.bones,
                spec.blend,
                spec.mirror,
                loft(spec, figure.detail, maps.is_some()),
            ),
            Shape::Slab(spec) => (&spec.region, &spec.bones, spec.blend, spec.mirror, slab(spec)),
        };
        let piece = piece.map_err(shape_error)?;
        let region_index = figure
            .regions
            .iter()
            .position(|known| &known.name == region)
            .ok_or_else(|| shape_error(format!("no region {region:?}")))?;
        let (group, cell, cells) = cell_of(region_index);
        if bones.is_empty() {
            return Err(shape_error("a shape needs a bone".to_owned()));
        }
        if blend.is_nan() || blend <= 0.0 {
            return Err(shape_error("`blend` must be more than 0".to_owned()));
        }
        let find = |names: &mut dyn Iterator<Item = String>| -> Result<Vec<usize>, FigureError> {
            let mut joints = Vec::new();
            for name in names {
                let found = bones_named(&rig, &name);
                if found.is_empty() {
                    return Err(shape_error(format!("no joint {name:?}")));
                }
                joints.extend(found);
            }
            Ok(joints)
        };
        let joints = find(&mut bones.iter().cloned())?;
        let copies: &[bool] = if mirror { &[false, true] } else { &[false] };
        let (cells, cell) = (cells as f32, cell as f32);
        // U across the cell, clear of its sides by a margin, when unwrapped.
        let margin = maps.map_or(0.0, |maps| CELL_MARGIN_TEXELS / maps.size as f32 * cells);
        let (geometry, influence_set) = &mut geometries[group];
        for &across in copies {
            let joints: Vec<usize> =
                if across { find(&mut bones.iter().map(|name| mirrored(name)))? } else { joints.clone() };
            let flip = |point: Vec3| if across { Vec3::new(-point.x, point.y, point.z) } else { point };
            let first = geometry.positions.len() as u32;
            for (((&position, &normal), &u), &v) in
                piece.positions.iter().zip(&piece.normals).zip(&piece.u).zip(&piece.v)
            {
                let position = flip(position);
                let (joint, weight) = influences(&rig, &joints, blend, position);
                let u = match maps {
                    None => (cell + 0.5) / cells,
                    Some(_) => (cell + margin + u * (1.0 - 2.0 * margin)) / cells,
                };
                geometry.positions.push(position.to_array());
                geometry.normals.push(flip(normal).to_array());
                geometry.uvs.push([u, v.clamp(0.002, 0.998)]);
                influence_set.joints.push(joint);
                influence_set.weights.push(weight);
            }
            for triangle in piece.indices.as_chunks::<3>().0 {
                let [a, b, c] = triangle.map(|index| first + index);
                // Across X the winding turns over with the shape.
                geometry.indices.extend(if across { [a, c, b] } else { [a, b, c] });
            }
        }
    }
    let triangles: usize = geometries.iter().map(|(geometry, _)| geometry.indices.len() / 3).sum();
    if triangles == 0 {
        return Err(FigureError::Spec("a figure needs shapes".to_owned()));
    }
    if let Some(group) = groups.iter().zip(&geometries).find(|(_, (geometry, _))| geometry.indices.is_empty())
    {
        return Err(FigureError::Spec(format!("material {:?} has no shapes", group.0.name)));
    }
    if triangles > figure.max_triangles as usize {
        return Err(FigureError::Spec(format!(
            "{triangles} triangles, over the figure's budget of {}",
            figure.max_triangles
        )));
    }
    if let Some(exact) = figure.triangles
        && triangles != exact as usize
    {
        return Err(FigureError::Spec(format!("{triangles} triangles, where the figure must have {exact}")));
    }
    let face = groups.iter().position(|group| group.surface == Surface::Face);
    let mut morph_names = Vec::new();
    if !figure.expressions.is_empty() {
        let face =
            face.ok_or_else(|| FigureError::Spec("expressions move a face material: list one".to_owned()))?;
        let (targets, names) = morphs(figure, &geometries[face].0)?;
        geometries[face].0.targets = targets;
        morph_names = names;
    }
    let primitives = groups
        .iter()
        .zip(geometries)
        .map(|(group, (mut geometry, influence_set))| {
            geometry.influences.push(influence_set);
            Primitive { material: group.name.clone(), geometry }
        })
        .collect();
    Ok(Built { name: figure.name.clone(), rig, primitives, morphs: morph_names })
}

impl Built {
    /// The model in `writer`: the joints as nodes with their rest translations and rotations, a skin
    /// over them in the skeleton's order with their inverse binds, and the primitives under a node
    /// that holds the skin (the face, with morphs, under one of its own), all under one root. Returns
    /// the face's node, if it has one; the joints are nodes `0..n`, in the skeleton's order.
    pub fn scene(&self, writer: &mut GltfWriter) -> Option<usize> {
        let rig = &self.rig;
        let materials: Vec<usize> = self
            .primitives
            .iter()
            .map(|primitive| writer.add_material(json!({ "name": primitive.material })))
            .collect();
        let with_morphs = |primitive: &Primitive| !primitive.geometry.targets.is_empty();
        let body: Vec<(&Geometry, Option<usize>)> = self
            .primitives
            .iter()
            .zip(&materials)
            .filter(|(primitive, _)| !with_morphs(primitive))
            .map(|(primitive, &material)| (&primitive.geometry, Some(material)))
            .collect();
        let mesh = writer.add_mesh(&self.name, &body);
        let face_mesh =
            self.primitives.iter().zip(&materials).find(|(primitive, _)| with_morphs(primitive)).map(
                |(primitive, &material)| {
                    let mesh = writer.add_mesh("face", &[(&primitive.geometry, Some(material))]);
                    writer.set_mesh_field(mesh, "extras", json!({ "targetNames": self.morphs }));
                    mesh
                },
            );
        let children = |index: usize| -> Vec<usize> {
            (0..rig.names.len()).filter(|&child| rig.parents[child] == Some(index)).collect()
        };
        // Joints are nodes 0..n, in the skeleton's order, so a joint's node is its index.
        for index in 0..rig.names.len() {
            let (frame, at) = (rig.frames[index], rig.at[index]);
            let (local_frame, local_at) = match rig.parents[index] {
                Some(parent) => {
                    let inverse = rig.frames[parent].transpose();
                    (inverse * frame, inverse * (at - rig.at[parent]))
                }
                None => (frame, at),
            };
            let rotation = det::quat_from_mat3(&local_frame);
            let mut node = json!({
                "name": rig.names[index],
                "translation": local_at.to_array(),
                "rotation": rotation.to_array(),
            });
            let children = children(index);
            if !children.is_empty() {
                node["children"] = children.into();
            }
            writer.add_node(node);
        }
        let joints: Vec<usize> = (0..rig.names.len()).collect();
        let binds: Vec<[f32; 16]> = (0..rig.names.len())
            .map(|index| {
                let inverse = rig.frames[index].transpose();
                glam::Mat4::from_cols(
                    inverse.x_axis.extend(0.0),
                    inverse.y_axis.extend(0.0),
                    inverse.z_axis.extend(0.0),
                    (-(inverse * rig.at[index])).extend(1.0),
                )
                .to_cols_array()
            })
            .collect();
        let skin = writer.add_skin(&joints, Some(&binds));
        let body = writer.add_node(json!({ "name": self.name, "mesh": mesh, "skin": skin }));
        let face =
            face_mesh.map(|mesh| writer.add_node(json!({ "name": "face", "mesh": mesh, "skin": skin })));
        let mut roots: Vec<usize> =
            (0..rig.names.len()).filter(|&index| rig.parents[index].is_none()).collect();
        roots.push(body);
        roots.extend(face);
        let root = writer.add_node(json!({ "name": "rig", "children": roots }));
        writer.add_root(root);
        face
    }

    /// The model as a binary glTF: [`Self::scene`].
    pub fn to_glb(&self) -> Vec<u8> {
        let mut writer = GltfWriter::default();
        self.scene(&mut writer);
        writer.to_glb()
    }

    /// Its triangles, every primitive's.
    #[cfg(test)]
    pub fn triangles(&self) -> usize {
        self.primitives.iter().map(|primitive| primitive.geometry.indices.len() / 3).sum()
    }
}
