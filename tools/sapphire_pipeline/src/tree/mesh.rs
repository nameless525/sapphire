//! A grown skeleton as a model: its wood as tubes and its leaves as cards on its twig tips, one binary glTF
//! with a slot each ([`BARK_SLOT`], [`LEAVES_SLOT`]), its origin at the root. See the
//! [tree module's docs](super).

use std::f32::consts::TAU;

use glam::Vec3;
use sapphire_math::det;
use serde_json::json;

use super::atlas;
use super::grow::Skeleton;
use super::{BARK_SLOT, Brush, LEAVES_SLOT, TreeSpecies};
use crate::color::unorm8;
use crate::gltf_writer::{Geometry, GltfWriter};
use crate::samples::noise::{Rng, smoothstep};

/// The share of the height over which the trunk's foot darkens, its vertex occlusion.
const FOOT: f32 = 0.12;

/// A willow's strand card is this many times as long as it's wide, its atlas cell's shape.
pub const STRAND_ASPECT: f32 = 4.0;

/// `species` at age `age`, grown as `skeleton`, as a binary glTF.
pub fn tree_glb(species: &TreeSpecies, age: usize, skeleton: &Skeleton) -> Vec<u8> {
    let wood = wood(species, skeleton);
    let leaves = cards(species, age, skeleton);
    let name = format!("{}_{}", species.name, species.ages[age].name);
    let mut gltf = GltfWriter::default();
    let mut parts = Vec::new();
    for (slot, geometry) in [(BARK_SLOT, wood), (LEAVES_SLOT, leaves)] {
        let material = gltf.add_material(json!({
            "name": slot,
            "pbrMetallicRoughness": { "baseColorFactor": [1.0, 1.0, 1.0, 1.0] },
        }));
        parts.push((geometry, material));
    }
    let primitives: Vec<(&Geometry, Option<usize>)> =
        parts.iter().map(|(geometry, material)| (geometry, Some(*material))).collect();
    let mesh = gltf.add_mesh(&name, &primitives);
    let root = gltf.add_node(json!({ "name": name, "mesh": mesh }));
    gltf.add_root(root);
    gltf.to_glb()
}

/// The wood: a tube along each chain of main children (the thickest child carries its parent's chain on;
/// every other child starts one of its own, from inside its parent), as far as it stays thicker than the
/// spec's `thinnest`, with fewer sides as it starts thinner, its end closed by a point.
pub(crate) fn wood(species: &TreeSpecies, skeleton: &Skeleton) -> Geometry {
    let nodes = &skeleton.nodes;
    let children = skeleton.children();
    let wood = &species.wood;
    // Each node's main child: its thickest, the first of equals.
    let main: Vec<Option<u32>> = children
        .iter()
        .map(|children| {
            children.iter().copied().fold(None, |best: Option<u32>, child| match best {
                Some(best) if nodes[best as usize].radius >= nodes[child as usize].radius => Some(best),
                _ => Some(child),
            })
        })
        .collect();
    let thickest = nodes[0].radius;
    let mut geometry = Geometry::default();
    let starts = std::iter::once(0u32).chain((0..nodes.len() as u32).filter(|&node| {
        nodes[node as usize].parent.is_some_and(|parent| main[parent as usize] != Some(node))
    }));
    for start in starts {
        if nodes[start as usize].radius < wood.thinnest {
            continue;
        }
        // From the parent's position (inside the parent), at the start's radius.
        let mut points = Vec::new();
        let mut radii = Vec::new();
        if let Some(parent) = nodes[start as usize].parent {
            points.push(nodes[parent as usize].position);
            radii.push(nodes[start as usize].radius);
        }
        let mut at = Some(start);
        while let Some(node) = at.filter(|&node| nodes[node as usize].radius >= wood.thinnest) {
            points.push(nodes[node as usize].position);
            radii.push(nodes[node as usize].radius);
            at = main[node as usize];
        }
        if points.len() < 2 {
            continue;
        }
        let share = ((radii[0] - wood.thinnest) / (thickest - wood.thinnest).max(1e-6)).clamp(0.0, 1.0);
        let (most, fewest) = (wood.sides.0 as f32, wood.sides.1 as f32);
        let sides = (fewest + (most - fewest) * share.sqrt()).round() as u32;
        append(&mut geometry, tube(&points, &radii, sides, skeleton.height));
    }
    geometry
}

/// A tube along `points` with a radius per point, `sides` around, its rings carried along by parallel
/// transport so they don't twist, its end closed by a point; darker toward the trunk's foot.
fn tube(points: &[Vec3], radii: &[f32], sides: u32, height: f32) -> Geometry {
    let mut geometry = Geometry::default();
    let count = points.len();
    let tangent = |index: usize| {
        (points[(index + 1).min(count - 1)] - points[index.saturating_sub(1)]).normalize_or_zero()
    };
    let first = tangent(0);
    let helper = if first.y.abs() < 0.9 { Vec3::Y } else { Vec3::X };
    let mut side = first.cross(helper).normalize();
    let mut length = 0.0;
    for (index, (&p, &radius)) in points.iter().zip(radii).enumerate() {
        let along = tangent(index);
        // Carry the last ring's side onto this one's plane.
        let carried = side - along * side.dot(along);
        if carried.length_squared() > 1e-8 {
            side = carried.normalize();
        }
        let other = along.cross(side);
        if index > 0 {
            length += points[index].distance(points[index - 1]);
        }
        let occlusion = 0.65 + 0.35 * smoothstep(0.0, FOOT * height, p.y);
        for segment in 0..=sides {
            let (sin, cos) = det::sin_cos(TAU * segment as f32 / sides as f32);
            let normal = side * cos + other * sin;
            geometry.positions.push((p + normal * radius).to_array());
            geometry.normals.push(normal.to_array());
            geometry.uvs.push([segment as f32 / sides as f32, length]);
            geometry.colors.push([unorm8(occlusion), 255, 255, 255]);
        }
    }
    let stride = sides + 1;
    for ring in 0..count as u32 - 1 {
        for segment in 0..sides {
            let a = ring * stride + segment;
            let (b, c, d) = (a + 1, a + stride, a + stride + 1);
            // Counter-clockwise seen from outside: around the ring (side toward other), then along.
            geometry.indices.extend([a, b, c, b, d, c]);
        }
    }
    let tip = geometry.positions.len() as u32;
    let along = tangent(count - 1);
    geometry.positions.push((points[count - 1] + along * radii[count - 1]).to_array());
    geometry.normals.push(along.to_array());
    geometry.uvs.push([0.5, length]);
    geometry.colors.push([255; 4]);
    let last = (count as u32 - 1) * stride;
    for segment in 0..sides {
        geometry.indices.extend([last + segment, last + segment + 1, tip]);
    }
    geometry
}

/// The leaf cards: the spec's `cards` on every twig tip, each showing one cell of the atlas, sized by the
/// age's `card` and set as the brush says ([`Brush`]).
pub(crate) fn cards(species: &TreeSpecies, age: usize, skeleton: &Skeleton) -> Geometry {
    let brush = species.leaves.brush;
    let (columns, rows) = atlas::cells(brush);
    let size = species.ages[age].card;
    let mut rng = Rng(species.age_seed(age) ^ 0x1eaf_ca2d);
    let mut geometry = Geometry::default();
    for tip in skeleton.tips() {
        let at = skeleton.nodes[tip as usize].position;
        let direction = skeleton.direction(tip);
        for _ in 0..species.leaves.cards {
            // Every card draws the same numbers, whatever its brush uses.
            let cell = (rng.next() * (columns * rows) as f32) as u32 % (columns * rows);
            let scale = rng.range(0.85, 1.15);
            let jitter = unit(&mut rng);
            let facing = unit(&mut rng);
            let spin = rng.range(0.0, TAU);
            let size = size * scale;
            let corners = match brush {
                Brush::Broad | Brush::Petal => {
                    let center = at + direction * (0.25 * size) + jitter * (0.25 * size);
                    let outward = (center - skeleton.center).normalize_or_zero();
                    let normal = (facing * 0.8 + outward + Vec3::Y * 0.3).normalize_or_zero();
                    let normal = if normal == Vec3::ZERO { Vec3::Y } else { normal };
                    let (u, v) = frame(normal, spin);
                    quad(center, u * (0.5 * size), v * (0.5 * size))
                }
                Brush::Needle => {
                    // Lying along its twig, the frond's tip at the twig's, rolled up to a radian either way from flat.
                    let across = horizontal_across(direction);
                    let (sin, cos) = det::sin_cos((spin / TAU - 0.5) * 2.0);
                    let up = direction.cross(across).normalize_or_zero();
                    let across = across * cos + up * sin;
                    let center = at - direction * (0.35 * size) + jitter * (0.1 * size);
                    quad(center, across * (0.5 * size), direction * (0.5 * size))
                }
                Brush::Strand => {
                    // Hanging straight down from its twig, its face turned out of the crown a little.
                    let outward = at - skeleton.center;
                    let yaw = det::vec2_from_angle(spin)
                        + glam::Vec2::new(outward.x, outward.z).normalize_or_zero();
                    let yaw = if yaw.length_squared() < 1e-6 { glam::Vec2::X } else { yaw.normalize() };
                    let across = Vec3::new(-yaw.y, 0.0, yaw.x);
                    let width = size / STRAND_ASPECT;
                    let center = at + jitter * (0.1 * width) - Vec3::Y * (0.5 * size);
                    quad(center, across * (0.5 * width), Vec3::Y * (0.5 * size))
                }
            };
            append(&mut geometry, card(corners, cell, columns, rows));
        }
    }
    geometry
}

/// A random unit vector.
fn unit(rng: &mut Rng) -> Vec3 {
    loop {
        let v = Vec3::new(rng.range(-1.0, 1.0), rng.range(-1.0, 1.0), rng.range(-1.0, 1.0));
        if (0.05..=1.0).contains(&v.length_squared()) {
            return v.normalize();
        }
    }
}

/// Two axes across `normal`, turned `spin` radians about it.
fn frame(normal: Vec3, spin: f32) -> (Vec3, Vec3) {
    let helper = if normal.y.abs() < 0.9 { Vec3::Y } else { Vec3::X };
    let u0 = normal.cross(helper).normalize();
    let v0 = normal.cross(u0);
    let (sin, cos) = det::sin_cos(spin);
    (u0 * cos + v0 * sin, v0 * cos - u0 * sin)
}

/// The level direction across a twig running along `direction`.
fn horizontal_across(direction: Vec3) -> Vec3 {
    let across = direction.cross(Vec3::Y);
    if across.length_squared() > 1e-6 { across.normalize() } else { Vec3::X }
}

/// A quad's corners about `center`, `u` and `v` its half sides: the corner at −u, +v first, then
/// counter-clockwise seen from `u × v`, the side toward which `v` points the card's top.
fn quad(center: Vec3, u: Vec3, v: Vec3) -> [Vec3; 4] {
    [center - u + v, center - u - v, center + u - v, center + u + v]
}

/// A card at `corners` showing atlas cell `cell` of `columns` × `rows`, the cell's top at the corners' first
/// and last.
fn card(corners: [Vec3; 4], cell: u32, columns: u32, rows: u32) -> Geometry {
    let (column, row) = (cell % columns, cell / columns);
    let (u0, v0) = (column as f32 / columns as f32, row as f32 / rows as f32);
    let (u1, v1) = ((column + 1) as f32 / columns as f32, (row + 1) as f32 / rows as f32);
    let normal = (corners[1] - corners[0]).cross(corners[2] - corners[1]).normalize_or_zero();
    Geometry {
        positions: corners.map(|corner| corner.to_array()).to_vec(),
        normals: vec![normal.to_array(); 4],
        uvs: vec![[u0, v0], [u0, v1], [u1, v1], [u1, v0]],
        colors: vec![[255; 4]; 4],
        indices: vec![0, 1, 2, 0, 2, 3],
        ..Geometry::default()
    }
}

/// Appends `part` to `geometry`.
fn append(geometry: &mut Geometry, part: Geometry) {
    let first = geometry.positions.len() as u32;
    geometry.positions.extend(part.positions);
    geometry.normals.extend(part.normals);
    geometry.uvs.extend(part.uvs);
    geometry.colors.extend(part.colors);
    geometry.indices.extend(part.indices.into_iter().map(|index| first + index));
}
