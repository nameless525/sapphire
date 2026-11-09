//! A figure's skeleton and shapes, made into one skinned primitive (`build`).
//!
//! Everything is placed in the model's space at the bind pose, so a vertex's place is where it's
//! drawn at rest, and each joint's inverse bind is the inverse of its rest frame. Shapes are smooth
//! within themselves (each vertex's normal is the area-weighted sum of its faces') unless `faceted`;
//! the ends of a shell and the sides of a slab always shade flat, as a cut edge does. Every triangle's
//! UVs lie in its region's cell: U at the cell's middle, V down the cell as the spec says.

use std::f32::consts::TAU;

use glam::{Mat3, Vec3};
use sapphire_math::det;
use serde_json::json;

use super::{Figure, FigureError, Loft, MAX_REGIONS, Shape, Slab};
use crate::gltf_writer::{Geometry, GltfWriter, InfluenceSet};

/// How far a leaf joint's bone reaches along its +Y, for skinning, in metres.
const LEAF_BONE: f32 = 0.05;

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

/// A figure, built: its skeleton and its one primitive.
#[derive(Clone, Debug)]
pub(crate) struct Built {
    pub name: String,
    pub rig: Rig,
    pub geometry: Geometry,
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

fn rig(figure: &Figure) -> Result<Rig, FigureError> {
    let joint_error = |joint: &str, why: String| FigureError::Joint { joint: joint.to_owned(), why };
    if figure.joints.is_empty() {
        return Err(FigureError::Spec("a figure needs joints".to_owned()));
    }
    let mut names: Vec<String> = Vec::with_capacity(figure.joints.len());
    let mut parents = Vec::with_capacity(figure.joints.len());
    for joint in &figure.joints {
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
    let at: Vec<Vec3> = figure.joints.iter().map(|joint| Vec3::from(joint.at)).collect();
    let first_child = |index: usize| parents.iter().position(|&parent| parent == Some(index));
    let mut frames: Vec<Mat3> = Vec::with_capacity(at.len());
    let mut tails = Vec::with_capacity(at.len());
    for (index, joint) in figure.joints.iter().enumerate() {
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
    /// V down the region's cell.
    v: Vec<f32>,
    indices: Vec<u32>,
}

impl Piece {
    fn push(&mut self, position: Vec3, normal: Vec3, v: f32) -> u32 {
        self.positions.push(position);
        self.normals.push(normal);
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
                let index = self.push(self.positions[corner], normal, self.v[corner]);
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

fn loft(loft: &Loft) -> Result<Piece, String> {
    if loft.rings.len() < 2 {
        return Err("a loft needs two rings or more".to_owned());
    }
    if loft.rings.windows(2).any(|pair| pair[1].at < pair[0].at) {
        return Err("rings go by increasing `at`".to_owned());
    }
    if loft.sides < 3 {
        return Err("a loft needs three sides or more".to_owned());
    }
    if loft.arc.is_some() && loft.thickness.is_none() {
        return Err("an arc needs a thickness: a solid can't be open".to_owned());
    }
    let (from, to) = (Vec3::from(loft.from), Vec3::from(loft.to));
    let length = from.distance(to);
    if length < 1e-4 {
        return Err("`from` and `to` are the same place".to_owned());
    }
    let axis = (to - from) / length;
    let (across, around) = loft_frame(loft, axis)?;
    let sides = loft.sides as usize;
    // An arc's last column is its own; a whole ring's wraps to its first.
    let (columns, closed) = match loft.arc {
        Some(_) => (sides + 1, false),
        None => (sides, true),
    };
    let angle = |column: usize| match loft.arc {
        Some((start, end)) => (start + (end - start) * column as f32 / sides as f32).to_radians(),
        None => TAU * column as f32 / sides as f32,
    };
    let point = |ring: usize, column: usize, inset: f32| {
        let ring = &loft.rings[ring];
        let (sin, cos) = det::sin_cos(angle(column));
        let radius = ((ring.radius.0 - inset).max(0.0), (ring.radius.1 - inset).max(0.0));
        from + axis * (ring.at * length)
            + across * (radius.0 * cos + ring.shift.0)
            + around * (radius.1 * sin + ring.shift.1)
    };
    let v_at = |ring: usize| loft.v.0 + (loft.v.1 - loft.v.0) * loft.rings[ring].at;
    let mut piece = Piece::default();

    // One surface: outward, or inward at an inset for a shell's inside.
    let surface = |piece: &mut Piece, inset: f32, inward: bool| -> Vec<u32> {
        let first = piece.positions.len() as u32;
        for ring in 0..loft.rings.len() {
            for column in 0..columns {
                piece.push(point(ring, column, inset), Vec3::ZERO, v_at(ring));
            }
        }
        let first_index = piece.indices.len();
        let at = |ring: usize, column: usize| first + (ring * columns + column % columns) as u32;
        for ring in 0..loft.rings.len() - 1 {
            for column in 0..sides {
                let quad =
                    [at(ring, column), at(ring, column + 1), at(ring + 1, column + 1), at(ring + 1, column)];
                piece.quad(if inward { [quad[3], quad[2], quad[1], quad[0]] } else { quad });
            }
        }
        if loft.faceted {
            piece.facet(first_index);
        } else {
            piece.smooth(first as usize, first_index);
        }
        (0..loft.rings.len() * columns).map(|offset| first + offset as u32).collect()
    };

    match loft.thickness {
        None => {
            surface(&mut piece, 0.0, false);
            if loft.caps {
                for (ring, outward) in [(0, -axis), (loft.rings.len() - 1, axis)] {
                    let r = &loft.rings[ring];
                    if r.radius.0 < 1e-4 && r.radius.1 < 1e-4 {
                        continue;
                    }
                    let first_index = piece.indices.len();
                    let center = from + axis * (r.at * length) + across * r.shift.0 + around * r.shift.1;
                    let middle = piece.push(center, outward, v_at(ring));
                    let corners: Vec<u32> = (0..sides)
                        .map(|column| piece.push(point(ring, column, 0.0), outward, v_at(ring)))
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
                let last = loft.rings.len() - 1;
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
                for ring in 0..loft.rings.len() - 1 {
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
    let v_at = |w_at: f32| slab.v.0 + (slab.v.1 - slab.v.0) * (top - w_at) / (top - bottom).max(1e-6);
    let origin = Vec3::from(slab.origin);
    let place =
        |(a, b): (f32, f32), side: f32| origin + u * a + w * b + normal * (side * slab.thickness * 0.5);
    let mut piece = Piece::default();
    for (side, facing) in [(1.0, normal), (-1.0, -normal)] {
        let middle = piece.push(place(centroid, side), facing, v_at(centroid.1));
        let corners: Vec<u32> =
            outline.iter().map(|&corner| piece.push(place(corner, side), facing, v_at(corner.1))).collect();
        for k in 0..corners.len() {
            let (a, b) = (corners[k], corners[(k + 1) % corners.len()]);
            piece.indices.extend(if side > 0.0 { [middle, a, b] } else { [middle, b, a] });
        }
    }
    let first_index = piece.indices.len();
    let front: Vec<u32> =
        outline.iter().map(|&corner| piece.push(place(corner, 1.0), normal, v_at(corner.1))).collect();
    let back: Vec<u32> =
        outline.iter().map(|&corner| piece.push(place(corner, -1.0), -normal, v_at(corner.1))).collect();
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

/// Builds `figure`'s skeleton and primitive.
pub(crate) fn build(figure: &Figure) -> Result<Built, FigureError> {
    let rig = rig(figure)?;
    if figure.regions.is_empty() || figure.regions.len() > MAX_REGIONS {
        return Err(FigureError::Spec(format!(
            "a figure has one to {MAX_REGIONS} regions, not {}",
            figure.regions.len()
        )));
    }
    let cells = figure.regions.len() as f32;
    let mut geometry = Geometry::default();
    let mut influence_set = InfluenceSet::default();
    for (index, shape) in figure.shapes.iter().enumerate() {
        let shape_error = |why: String| FigureError::Shape { index, why };
        let (region, bones, blend, mirror, piece) = match shape {
            Shape::Loft(spec) => (&spec.region, &spec.bones, spec.blend, spec.mirror, loft(spec)),
            Shape::Slab(spec) => (&spec.region, &spec.bones, spec.blend, spec.mirror, slab(spec)),
        };
        let piece = piece.map_err(shape_error)?;
        let cell = figure
            .regions
            .iter()
            .position(|known| &known.name == region)
            .ok_or_else(|| shape_error(format!("no region {region:?}")))?;
        if bones.is_empty() {
            return Err(shape_error("a shape needs a bone".to_owned()));
        }
        if blend.is_nan() || blend <= 0.0 {
            return Err(shape_error("`blend` must be more than 0".to_owned()));
        }
        let find = |name: &str| rig.find(name).ok_or_else(|| shape_error(format!("no joint {name:?}")));
        let joints: Vec<usize> = bones.iter().map(|name| find(name)).collect::<Result<_, _>>()?;
        let copies: &[bool] = if mirror { &[false, true] } else { &[false] };
        for &across in copies {
            let joints: Vec<usize> = if across {
                bones.iter().map(|name| find(&mirrored(name))).collect::<Result<_, _>>()?
            } else {
                joints.clone()
            };
            let flip = |point: Vec3| if across { Vec3::new(-point.x, point.y, point.z) } else { point };
            let first = geometry.positions.len() as u32;
            let u = (cell as f32 + 0.5) / cells;
            for ((&position, &normal), &v) in piece.positions.iter().zip(&piece.normals).zip(&piece.v) {
                let position = flip(position);
                let (joint, weight) = influences(&rig, &joints, blend, position);
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
    if geometry.indices.is_empty() {
        return Err(FigureError::Spec("a figure needs shapes".to_owned()));
    }
    let triangles = geometry.indices.len() / 3;
    if triangles > figure.max_triangles as usize {
        return Err(FigureError::Spec(format!(
            "{triangles} triangles, over the figure's budget of {}",
            figure.max_triangles
        )));
    }
    geometry.influences.push(influence_set);
    Ok(Built { name: figure.name.clone(), rig, geometry })
}

impl Built {
    /// The model as a binary glTF: the joints as nodes with their rest translations and rotations,
    /// a skin over them in the spec's order with their inverse binds, and the primitive under a node
    /// that holds the skin, both under one root.
    pub fn to_glb(&self) -> Vec<u8> {
        let rig = &self.rig;
        let mut writer = GltfWriter::default();
        let material = writer.add_material(json!({ "name": self.name }));
        let mesh = writer.add_mesh(&self.name, &[(&self.geometry, Some(material))]);
        let children = |index: usize| -> Vec<usize> {
            (0..rig.names.len()).filter(|&child| rig.parents[child] == Some(index)).collect()
        };
        // Joints are nodes 0..n, in the spec's order, so a joint's node is its index.
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
        let mut roots: Vec<usize> =
            (0..rig.names.len()).filter(|&index| rig.parents[index].is_none()).collect();
        roots.push(body);
        let root = writer.add_node(json!({ "name": "rig", "children": roots }));
        writer.add_root(root);
        writer.to_glb()
    }
}
