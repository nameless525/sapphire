//! The collider document (`.colliders.ron`): a zone's static collision, written by hand.
//!
//! ```ron
//! (
//!     type: "sapphire::colliders::ColliderDocument",
//!     version: 1,
//!     data: (
//!         colliders: [
//!             (name: "ground", shape: Box(half_extents: (20.0, 0.5, 20.0)), position: (0.0, -0.5, 0.0)),
//!             (
//!                 name: "crate",
//!                 shape: Box(half_extents: (0.5, 0.5, 0.5)),
//!                 position: (2.0, 0.5, 1.0),
//!                 rotation: AxisAngle(axis: (0.0, 1.0, 0.0), degrees: 30.0),
//!             ),
//!             (name: "pillar", shape: Capsule(half_height: 1.5, radius: 0.4), position: (4.0, 1.9, 0.0)),
//!             (
//!                 name: "edge",
//!                 shape: Box(half_extents: (20.0, 2.0, 0.5)),
//!                 position: (0.0, 2.0, 20.0),
//!                 groups: [CharacterBlocker],
//!             ),
//!         ],
//!     ),
//! )
//! ```
//!
//! [`ColliderDocument::cook`] checks it and writes the cooked [`ColliderSet`]. Every refusal names
//! the entry it concerns.
//!
//! Capsules stand along their local +Y, as characters' do. A mesh's vertices are in its collider's
//! space, and its triangles are two-sided.

use std::collections::BTreeSet;

use bevy_reflect::TypePath;
use glam::{Quat, Vec3};
use sapphire_math::det;
use sapphire_reflect::Versioned;
use sapphire_serialize::{CookedError, DocumentError};
use serde::{Deserialize, Serialize};

use crate::cooked::{ColliderSet, CookedCollider, CookedMesh, CookedShape};
use crate::groups::{Group, GroupMask};

/// The most colliders a set may hold.
pub const MAX_COLLIDERS: usize = 1 << 16;
/// The most triangles a mesh collider may hold. Documents are written by hand: a mesh near this
/// comes from a tool that should be cooking a zone's geometry instead (M3).
pub const MAX_MESH_TRIANGLES: usize = 1 << 18;
/// How far from the origin anything may lie or reach, in metres. A zone stays within about 4 km of
/// its own origin (spec §5.4), and `f32` loses its millimetres beyond this.
pub const MAX_COORDINATE: f32 = 16_384.0;
/// The thinnest extent or radius, in metres. Thinner shapes are too small for the sweeps' tolerance.
pub const MIN_EXTENT: f32 = 0.001;
/// Twice the smallest triangle's area, in square metres. Thinner triangles have no usable normal.
pub const MIN_TRIANGLE_AREA: f32 = 1e-8;

/// A collider document (see the [module docs](self)).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, TypePath)]
#[type_path = "sapphire::colliders"]
#[serde(deny_unknown_fields)]
pub struct ColliderDocument {
    pub colliders: Vec<ColliderEntry>,
}

impl Versioned for ColliderDocument {
    /// 1: M2 D3.
    const VERSION: u32 = 1;
}

/// One static collider.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ColliderEntry {
    /// Unique in the document. The cooked set sorts by it, so a collider's ID doesn't depend on
    /// where the document lists it.
    pub name: String,
    pub shape: ColliderShape,
    /// World position of the shape's center, in metres (a mesh's origin).
    pub position: [f32; 3],
    #[serde(default)]
    pub rotation: Rotation,
    /// The groups this collider is in.
    #[serde(default = "default_groups")]
    pub groups: Vec<Group>,
}

fn default_groups() -> Vec<Group> {
    vec![Group::World]
}

/// A collider's geometry, in metres.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub enum ColliderShape {
    Box {
        half_extents: [f32; 3],
    },
    Sphere {
        radius: f32,
    },
    /// Along local +Y: its cylinder's half height, then the radius of its caps.
    Capsule {
        half_height: f32,
        radius: f32,
    },
    /// A two-sided triangle mesh. Prefer boxes: a capsule resting on a box costs one sweep test,
    /// on a mesh one for each triangle it might reach.
    Mesh {
        vertices: Vec<[f32; 3]>,
        indices: Vec<[u32; 3]>,
    },
}

/// A collider's rotation.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub enum Rotation {
    #[default]
    None,
    /// A turn about an axis, which needn't be unit length.
    AxisAngle { axis: [f32; 3], degrees: f32 },
    /// A unit quaternion: x, y, z, w.
    Quaternion([f32; 4]),
}

impl Rotation {
    /// The rotation as a quaternion, or what's wrong with it. Computed through `det` alone: this
    /// value reaches the simulation, and each OS cooks for itself.
    fn resolve(self) -> Result<Quat, String> {
        match self {
            Self::None => Ok(Quat::IDENTITY),
            Self::AxisAngle { axis, degrees } => {
                let axis = Vec3::from_array(axis);
                if !axis.is_finite() || !degrees.is_finite() {
                    return Err(format!("rotation axis {axis} or angle {degrees} isn't finite"));
                }
                let Some(unit) = axis.try_normalize() else {
                    return Err(format!("rotation axis {axis} has no direction"));
                };
                Ok(det::quat_from_axis_angle(unit, degrees.to_radians()))
            }
            Self::Quaternion(q) => {
                check_quaternion(q)?;
                Ok(Quat::from_array(q))
            }
        }
    }
}

/// A collider document that can't cook, or cooked colliders the runtime refuses.
#[derive(Debug, thiserror::Error)]
pub enum ColliderError {
    #[error(transparent)]
    Document(#[from] Box<DocumentError>),
    #[error(transparent)]
    Cooked(#[from] CookedError),
    #[error("the set holds no colliders")]
    Empty,
    #[error("{count} colliders is more than the {MAX_COLLIDERS} a set may hold")]
    TooMany { count: usize },
    #[error("collider at index {index} has no name")]
    Unnamed { index: usize },
    #[error("two colliders are named `{name}`")]
    DuplicateName { name: String },
    /// A cooked set out of name order, whose IDs would depend on how it was written.
    #[error("collider `{name}` is out of order")]
    OutOfOrder { name: String },
    #[error("collider `{name}`: {problem}")]
    Invalid { name: String, problem: String },
}

impl ColliderDocument {
    /// Reads a document from RON text.
    pub fn from_ron(text: &str) -> Result<Self, ColliderError> {
        sapphire_serialize::from_ron(text).map_err(|error| ColliderError::Document(Box::new(error)))
    }

    /// Checks the document and writes its cooked form, sorted by name.
    pub fn cook(&self) -> Result<ColliderSet, ColliderError> {
        if self.colliders.is_empty() {
            return Err(ColliderError::Empty);
        }
        if self.colliders.len() > MAX_COLLIDERS {
            return Err(ColliderError::TooMany { count: self.colliders.len() });
        }
        let mut names = BTreeSet::new();
        for (index, entry) in self.colliders.iter().enumerate() {
            if entry.name.is_empty() {
                return Err(ColliderError::Unnamed { index });
            }
            if !names.insert(entry.name.as_str()) {
                return Err(ColliderError::DuplicateName { name: entry.name.clone() });
            }
        }

        let mut order: Vec<&ColliderEntry> = self.colliders.iter().collect();
        order.sort_unstable_by(|a, b| a.name.cmp(&b.name));

        let mut set = ColliderSet { colliders: Vec::with_capacity(order.len()), meshes: Vec::new() };
        for entry in order {
            let invalid = |problem: String| ColliderError::Invalid { name: entry.name.clone(), problem };
            let rotation = entry.rotation.resolve().map_err(invalid)?;
            check_position(entry.position).map_err(invalid)?;
            let memberships = entry.groups.iter().fold(GroupMask::NONE, |mask, &group| mask | group.mask());
            check_memberships(memberships.bits()).map_err(invalid)?;
            let shape = match &entry.shape {
                ColliderShape::Box { half_extents } => {
                    check_box(*half_extents).map_err(invalid)?;
                    CookedShape::Box { half_extents: *half_extents }
                }
                ColliderShape::Sphere { radius } => {
                    check_sphere(*radius).map_err(invalid)?;
                    CookedShape::Sphere { radius: *radius }
                }
                ColliderShape::Capsule { half_height, radius } => {
                    check_capsule(*half_height, *radius).map_err(invalid)?;
                    CookedShape::Capsule { half_height: *half_height, radius: *radius }
                }
                ColliderShape::Mesh { vertices, indices } => {
                    check_mesh(vertices, indices).map_err(invalid)?;
                    let mesh = set.meshes.len() as u32;
                    set.meshes.push(CookedMesh { vertices: vertices.clone(), indices: indices.clone() });
                    CookedShape::Mesh { mesh }
                }
            };
            set.colliders.push(CookedCollider {
                name: entry.name.clone(),
                shape,
                position: entry.position,
                rotation: rotation.to_array(),
                memberships: memberships.bits(),
            });
        }
        Ok(set)
    }
}

// These checks read plain values, so the document's and the archive's run the same ones.

/// Whether a position lies within [`MAX_COORDINATE`], or what's wrong with it.
pub fn check_position(position: [f32; 3]) -> Result<(), String> {
    if !position.iter().all(|v| v.is_finite()) {
        return Err(format!("position {position:?} isn't finite"));
    }
    if position.iter().any(|v| v.abs() > MAX_COORDINATE) {
        return Err(format!("position {position:?} lies past {MAX_COORDINATE} m"));
    }
    Ok(())
}

pub(crate) fn check_quaternion(q: [f32; 4]) -> Result<(), String> {
    if !q.iter().all(|v| v.is_finite()) {
        return Err(format!("rotation {q:?} isn't finite"));
    }
    let length_squared: f32 = q.iter().map(|v| v * v).sum();
    // The same tolerance as glam's `is_normalized`, which the physics world asks of its poses.
    if (length_squared - 1.0).abs() > 2e-4 {
        return Err(format!("rotation {q:?} isn't a unit quaternion"));
    }
    Ok(())
}

pub(crate) fn check_memberships(bits: u32) -> Result<(), String> {
    if bits == 0 { Err("belongs to no group, so nothing could ever hit it".to_owned()) } else { Ok(()) }
}

fn check_extent(what: &str, value: f32) -> Result<(), String> {
    if !value.is_finite() {
        return Err(format!("{what} {value} isn't finite"));
    }
    if value < MIN_EXTENT {
        return Err(format!("{what} {value} is under the {MIN_EXTENT} m minimum"));
    }
    if value > MAX_COORDINATE {
        return Err(format!("{what} {value} is over the {MAX_COORDINATE} m maximum"));
    }
    Ok(())
}

pub(crate) fn check_box(half_extents: [f32; 3]) -> Result<(), String> {
    half_extents.iter().try_for_each(|&v| check_extent("half extent", v))
}

pub(crate) fn check_sphere(radius: f32) -> Result<(), String> {
    check_extent("radius", radius)
}

pub(crate) fn check_capsule(half_height: f32, radius: f32) -> Result<(), String> {
    check_extent("half height", half_height)?;
    check_extent("radius", radius)
}

/// Reads a mesh's vertices and triangles through accessors, so the document and the archive share
/// one check whatever their storage. Counts come first, so a hostile count stops before any walk.
pub(crate) fn check_mesh_with(
    vertices: usize,
    triangles: usize,
    vertex: impl Fn(usize) -> [f32; 3],
    triangle: impl Fn(usize) -> [u32; 3],
) -> Result<(), String> {
    if triangles == 0 {
        return Err("mesh has no triangles".to_owned());
    }
    if triangles > MAX_MESH_TRIANGLES {
        return Err(format!("mesh has {triangles} triangles, over the {MAX_MESH_TRIANGLES} maximum"));
    }
    if vertices > u32::MAX as usize {
        return Err(format!("mesh has {vertices} vertices, more than indices can name"));
    }
    for index in 0..vertices {
        let position = vertex(index);
        if !position.iter().all(|v| v.is_finite()) {
            return Err(format!("mesh vertex {index} is {position:?}, not finite"));
        }
        if position.iter().any(|v| v.abs() > MAX_COORDINATE) {
            return Err(format!("mesh vertex {index} is {position:?}, past {MAX_COORDINATE} m"));
        }
    }
    for index in 0..triangles {
        let corners = triangle(index);
        if let Some(&corner) = corners.iter().find(|&&corner| corner as usize >= vertices) {
            return Err(format!("mesh triangle {index} names vertex {corner}, of {vertices} vertices"));
        }
        let [a, b, c] = corners.map(|corner| Vec3::from_array(vertex(corner as usize)));
        if (b - a).cross(c - a).length_squared() < MIN_TRIANGLE_AREA * MIN_TRIANGLE_AREA {
            return Err(format!("mesh triangle {index} ({corners:?}) has no area"));
        }
    }
    Ok(())
}

/// Whether a mesh has triangles, all within bounds, naming vertices it has, with area, or what's
/// wrong with it.
pub fn check_mesh(vertices: &[[f32; 3]], indices: &[[u32; 3]]) -> Result<(), String> {
    check_mesh_with(vertices.len(), indices.len(), |i| vertices[i], |i| indices[i])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(name: &str, shape: ColliderShape) -> ColliderEntry {
        ColliderEntry {
            name: name.to_owned(),
            shape,
            position: [0.0; 3],
            rotation: Rotation::None,
            groups: vec![Group::World],
        }
    }

    fn cube(name: &str) -> ColliderEntry {
        entry(name, ColliderShape::Box { half_extents: [1.0; 3] })
    }

    fn triangle_mesh(vertices: Vec<[f32; 3]>, indices: Vec<[u32; 3]>) -> ColliderShape {
        ColliderShape::Mesh { vertices, indices }
    }

    #[track_caller]
    fn refusal(colliders: Vec<ColliderEntry>) -> String {
        ColliderDocument { colliders }.cook().expect_err("this document must be refused").to_string()
    }

    #[track_caller]
    fn assert_names(message: &str, entry: &str, problem: &str) {
        assert!(message.contains(entry), "`{message}` must name `{entry}`");
        assert!(message.contains(problem), "`{message}` must say `{problem}`");
    }

    #[test]
    fn reads_the_documented_form() {
        let text = r#"(
            type: "sapphire::colliders::ColliderDocument",
            version: 1,
            data: (
                colliders: [
                    (name: "ground", shape: Box(half_extents: (20.0, 0.5, 20.0)), position: (0.0, -0.5, 0.0)),
                    (
                        name: "crate",
                        shape: Box(half_extents: (0.5, 0.5, 0.5)),
                        position: (2.0, 0.5, 1.0),
                        rotation: AxisAngle(axis: (0.0, 1.0, 0.0), degrees: 90.0),
                    ),
                    (name: "pillar", shape: Capsule(half_height: 1.5, radius: 0.4), position: (4.0, 1.9, 0.0)),
                    (name: "ball", shape: Sphere(radius: 0.5), position: (0.0, 3.0, 0.0), groups: [CameraBlocker]),
                    (
                        name: "ramp",
                        shape: Mesh(vertices: [(0.0, 0.0, 0.0), (1.0, 0.0, 0.0), (0.0, 1.0, 1.0)], indices: [(0, 1, 2)]),
                        position: (0.0, 0.0, 0.0),
                    ),
                ],
            ),
        )"#;
        let set = ColliderDocument::from_ron(text).expect("read").cook().expect("cook");
        let names: Vec<&str> = set.colliders.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, ["ball", "crate", "ground", "pillar", "ramp"]);
        assert_eq!(set.colliders[0].memberships, Group::CameraBlocker.mask().bits());
        assert_eq!(set.colliders[1].memberships, Group::World.mask().bits(), "the default group");
        assert_eq!(set.colliders[4].shape, CookedShape::Mesh { mesh: 0 });
        // 90° about Y, by hand: (0, sin 45°, 0, cos 45°).
        let [x, y, z, w] = set.colliders[1].rotation;
        assert_eq!((x, z), (0.0, 0.0));
        assert!((y - std::f32::consts::FRAC_1_SQRT_2).abs() < 1e-6, "y {y}");
        assert!((w - std::f32::consts::FRAC_1_SQRT_2).abs() < 1e-6, "w {w}");
    }

    #[test]
    fn a_syntax_error_says_where() {
        let text = "(\n  type: \"sapphire::colliders::ColliderDocument\",\n  version: 1,\n  data: (colliders: [(name: )]),\n)";
        let message = ColliderDocument::from_ron(text).expect_err("malformed").to_string();
        assert!(message.contains("4:"), "the error must point into the document: {message}");
    }

    #[test]
    fn an_unknown_field_is_refused() {
        let text = r#"(
            type: "sapphire::colliders::ColliderDocument",
            version: 1,
            data: (colliders: [(name: "a", shape: Sphere(radius: 1.0), position: (0.0, 0.0, 0.0), mass: 3.0)]),
        )"#;
        let message = ColliderDocument::from_ron(text).expect_err("unknown field").to_string();
        assert!(message.contains("mass"), "{message}");
    }

    #[test]
    fn a_newer_version_is_refused() {
        let text = r#"(
            type: "sapphire::colliders::ColliderDocument",
            version: 99,
            data: (colliders: []),
        )"#;
        let message = ColliderDocument::from_ron(text).expect_err("too new").to_string();
        assert!(message.contains("newer"), "{message}");
    }

    #[test]
    fn empty_and_unnamed_and_duplicate_documents_are_refused() {
        assert_eq!(refusal(vec![]), "the set holds no colliders");
        assert_names(&refusal(vec![cube("a"), cube("")]), "index 1", "no name");
        assert_names(&refusal(vec![cube("wall"), cube("floor"), cube("wall")]), "`wall`", "two colliders");
    }

    #[test]
    fn bad_values_name_their_entry() {
        let shapes = [
            (ColliderShape::Box { half_extents: [1.0, 0.0, 1.0] }, "under"),
            (ColliderShape::Box { half_extents: [1.0, f32::NAN, 1.0] }, "finite"),
            (ColliderShape::Box { half_extents: [1.0, 1e6, 1.0] }, "over"),
            (ColliderShape::Sphere { radius: -1.0 }, "under"),
            (ColliderShape::Sphere { radius: f32::INFINITY }, "finite"),
            (ColliderShape::Capsule { half_height: 0.0, radius: 1.0 }, "half height"),
            (ColliderShape::Capsule { half_height: 1.0, radius: 0.0 }, "radius"),
            (triangle_mesh(vec![], vec![]), "no triangles"),
            (triangle_mesh(vec![[0.0; 3]; 3], vec![[0, 1, 3]]), "names vertex 3"),
            (triangle_mesh(vec![[0.0; 3]; 3], vec![[0, 1, 2]]), "no area"),
            (
                triangle_mesh(vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [f32::NAN, 0.0, 0.0]], vec![[0, 1, 2]]),
                "vertex 2",
            ),
            (
                triangle_mesh(vec![[0.0, 0.0, 0.0], [1e6, 0.0, 0.0], [0.0, 0.0, 1.0]], vec![[0, 1, 2]]),
                "vertex 1",
            ),
        ];
        for (shape, problem) in shapes {
            let message = refusal(vec![cube("fine"), entry("bad", shape.clone())]);
            assert_names(&message, "`bad`", problem);
        }
    }

    #[test]
    fn bad_placements_name_their_entry() {
        let moved = |position| ColliderEntry { position, ..cube("bad") };
        assert_names(&refusal(vec![moved([f32::NAN, 0.0, 0.0])]), "`bad`", "finite");
        assert_names(&refusal(vec![moved([1e7, 0.0, 0.0])]), "`bad`", "past");
        let turned = |rotation| ColliderEntry { rotation, ..cube("bad") };
        assert_names(
            &refusal(vec![turned(Rotation::AxisAngle { axis: [0.0; 3], degrees: 10.0 })]),
            "`bad`",
            "no direction",
        );
        assert_names(
            &refusal(vec![turned(Rotation::AxisAngle { axis: [0.0, 1.0, 0.0], degrees: f32::NAN })]),
            "`bad`",
            "finite",
        );
        assert_names(&refusal(vec![turned(Rotation::Quaternion([0.0, 0.0, 0.0, 2.0]))]), "`bad`", "unit");
        let lonely = ColliderEntry { groups: vec![], ..cube("bad") };
        assert_names(&refusal(vec![lonely]), "`bad`", "no group");
    }

    #[test]
    fn listing_order_changes_nothing_cooked() {
        let mesh = entry(
            "mesh",
            triangle_mesh(vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 0.0, 1.0]], vec![[0, 1, 2]]),
        );
        let other_mesh = entry(
            "other mesh",
            triangle_mesh(vec![[0.0, 1.0, 0.0], [1.0, 1.0, 0.0], [0.0, 1.0, 1.0]], vec![[2, 1, 0]]),
        );
        let forward = vec![cube("a"), mesh.clone(), other_mesh.clone(), cube("z")];
        let backward = vec![cube("z"), other_mesh, mesh, cube("a")];
        let forward = ColliderDocument { colliders: forward }.cook().expect("cook");
        let backward = ColliderDocument { colliders: backward }.cook().expect("cook");
        assert_eq!(forward, backward);
        assert_eq!(
            forward.to_cooked().expect("write").as_slice(),
            backward.to_cooked().expect("write").as_slice(),
        );
    }
}
