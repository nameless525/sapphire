//! The cooked collider set (`.scol`): an rkyv archive the runtime reads in place.
//!
//! Sorted by name, so a collider's place is its stable ID. No upgrades, like every cooked format:
//! a changed layout takes a version bump and recooks from its document.

use bevy_reflect::TypePath;
use sapphire_reflect::Versioned;
use sapphire_serialize::cooked::AlignedVec;

use crate::document::{
    ColliderError, MAX_COLLIDERS, check_box, check_capsule, check_memberships, check_mesh_with,
    check_position, check_quaternion, check_sphere,
};

/// Conventional file extension for cooked collider sets.
pub const COLLIDERS_EXTENSION: &str = "scol";

/// A zone's static colliders. See the [module docs](self).
#[derive(Clone, Debug, Default, PartialEq, TypePath, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
#[rkyv(derive(Debug))]
#[type_path = "sapphire::colliders"]
pub struct ColliderSet {
    /// Sorted by name. A collider's index is its ID.
    pub colliders: Vec<CookedCollider>,
    /// The meshes [`CookedShape::Mesh`] names.
    pub meshes: Vec<CookedMesh>,
}

impl Versioned for ColliderSet {
    /// 1: M2 D3.
    const VERSION: u32 = 1;
}

/// One static collider.
#[derive(Clone, Debug, PartialEq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
#[rkyv(derive(Debug))]
pub struct CookedCollider {
    pub name: String,
    pub shape: CookedShape,
    pub position: [f32; 3],
    /// A unit quaternion: x, y, z, w.
    pub rotation: [f32; 4],
    /// The [`GroupMask`](crate::GroupMask)'s bits.
    pub memberships: u32,
}

/// A collider's geometry, in metres.
#[derive(Clone, Copy, Debug, PartialEq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
#[rkyv(derive(Debug))]
pub enum CookedShape {
    Box {
        half_extents: [f32; 3],
    },
    Sphere {
        radius: f32,
    },
    /// Along local +Y.
    Capsule {
        half_height: f32,
        radius: f32,
    },
    /// An index into [`ColliderSet::meshes`].
    Mesh {
        mesh: u32,
    },
}

/// A two-sided triangle mesh, in its collider's space.
#[derive(Clone, Debug, PartialEq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
#[rkyv(derive(Debug))]
pub struct CookedMesh {
    pub vertices: Vec<[f32; 3]>,
    pub indices: Vec<[u32; 3]>,
}

impl ColliderSet {
    pub fn to_cooked(&self) -> Result<AlignedVec, ColliderError> {
        Ok(sapphire_serialize::to_cooked(self)?)
    }

    /// Reads cooked bytes in place, checking the envelope, the archive and every value the world
    /// builds from ([`ArchivedColliderSet::validate`]).
    pub fn read(bytes: &[u8]) -> Result<&ArchivedColliderSet, ColliderError> {
        let set = sapphire_serialize::access::<Self>(bytes)?;
        set.validate()?;
        Ok(set)
    }

    /// [`Self::read`] into owned data, every value's bits as they were cooked: for a bake, which reads the
    /// bytes the simulation reads.
    pub fn from_cooked(bytes: &[u8]) -> Result<Self, ColliderError> {
        let set = rkyv::deserialize::<Self, rkyv::rancor::Error>(Self::read(bytes)?)
            .map_err(sapphire_serialize::CookedError::from)?;
        Ok(set)
    }
}

fn native3(values: &[rkyv::rend::f32_le; 3]) -> [f32; 3] {
    values.map(|v| v.to_native())
}

impl ArchivedCookedCollider {
    pub fn position(&self) -> [f32; 3] {
        native3(&self.position)
    }

    /// A unit quaternion: x, y, z, w.
    pub fn rotation(&self) -> [f32; 4] {
        self.rotation.map(|v| v.to_native())
    }
}

impl ArchivedCookedMesh {
    pub fn vertex(&self, index: usize) -> [f32; 3] {
        native3(&self.vertices[index])
    }

    pub fn triangle(&self, index: usize) -> [u32; 3] {
        self.indices[index].map(|v| v.to_native())
    }
}

impl ArchivedColliderSet {
    /// The ID of the collider named `name`.
    pub fn id_of(&self, name: &str) -> Option<u32> {
        let index = self.colliders.binary_search_by(|c| c.name.as_str().cmp(name)).ok()?;
        Some(index as u32)
    }

    /// Checks what building a world relies on, whatever wrote these bytes: the order of the names
    /// (the IDs), and every placement, extent, group and mesh index.
    pub fn validate(&self) -> Result<(), ColliderError> {
        if self.colliders.is_empty() {
            return Err(ColliderError::Empty);
        }
        if self.colliders.len() > MAX_COLLIDERS {
            return Err(ColliderError::TooMany { count: self.colliders.len() });
        }
        let mut previous: Option<&str> = None;
        for (index, collider) in self.colliders.iter().enumerate() {
            let name = collider.name.as_str();
            if name.is_empty() {
                return Err(ColliderError::Unnamed { index });
            }
            if let Some(previous) = previous {
                if previous == name {
                    return Err(ColliderError::DuplicateName { name: name.to_owned() });
                }
                if previous > name {
                    return Err(ColliderError::OutOfOrder { name: name.to_owned() });
                }
            }
            previous = Some(name);

            let invalid = |problem: String| ColliderError::Invalid { name: name.to_owned(), problem };
            check_position(collider.position()).map_err(invalid)?;
            check_quaternion(collider.rotation()).map_err(invalid)?;
            check_memberships(collider.memberships.to_native()).map_err(invalid)?;
            match &collider.shape {
                ArchivedCookedShape::Box { half_extents } => check_box(native3(half_extents)),
                ArchivedCookedShape::Sphere { radius } => check_sphere(radius.to_native()),
                ArchivedCookedShape::Capsule { half_height, radius } => {
                    check_capsule(half_height.to_native(), radius.to_native())
                }
                ArchivedCookedShape::Mesh { mesh } => match self.meshes.get(mesh.to_native() as usize) {
                    Some(mesh) => check_mesh_with(
                        mesh.vertices.len(),
                        mesh.indices.len(),
                        |i| mesh.vertex(i),
                        |i| mesh.triangle(i),
                    ),
                    None => Err(format!("names mesh {}, of {}", mesh.to_native(), self.meshes.len())),
                },
            }
            .map_err(invalid)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ColliderDocument, ColliderEntry, ColliderShape, Group, Rotation};

    fn set() -> ColliderSet {
        let cube = |name: &str| ColliderEntry {
            name: name.to_owned(),
            shape: ColliderShape::Box { half_extents: [1.0; 3] },
            position: [1.0, 2.0, 3.0],
            rotation: Rotation::None,
            groups: vec![Group::World],
        };
        let ramp = ColliderEntry {
            shape: ColliderShape::Mesh {
                vertices: vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 0.0, 1.0]],
                indices: vec![[0, 1, 2]],
            },
            ..cube("ramp")
        };
        ColliderDocument { colliders: vec![cube("b"), ramp, cube("a")] }.cook().expect("cook")
    }

    #[test]
    fn a_cooked_set_reads_back_in_place() {
        let bytes = set().to_cooked().expect("write");
        let read = ColliderSet::read(&bytes).expect("read");
        assert_eq!(read.colliders.len(), 3);
        assert_eq!(read.colliders[0].name.as_str(), "a");
        assert_eq!(read.colliders[0].position(), [1.0, 2.0, 3.0]);
        assert_eq!(read.colliders[0].rotation(), [0.0, 0.0, 0.0, 1.0]);
        assert_eq!(read.id_of("ramp"), Some(2));
        assert_eq!(read.id_of("nothing"), None);
        assert_eq!(read.meshes[0].triangle(0), [0, 1, 2]);
    }

    /// Whatever wrote these, `read` must refuse them: a pack's bytes needn't come from our cooker.
    #[test]
    fn cooked_data_the_document_would_refuse_is_refused() {
        type Tamper = fn(&mut ColliderSet);
        let cases: [(&str, &str, Tamper); 8] = [
            ("empty", "no colliders", |set| set.colliders.clear()),
            ("order", "out of order", |set| set.colliders.swap(0, 1)),
            ("duplicate", "two colliders", |set| set.colliders[1].name = "a".to_owned()),
            ("position", "`a`", |set| set.colliders[0].position[1] = f32::NAN),
            ("rotation", "unit", |set| set.colliders[0].rotation = [0.0, 0.0, 0.0, 2.0]),
            ("groups", "no group", |set| set.colliders[0].memberships = 0),
            ("mesh index", "names mesh", |set| set.colliders[2].shape = CookedShape::Mesh { mesh: 7 }),
            ("triangle", "names vertex", |set| set.meshes[0].indices[0] = [0, 1, 3]),
        ];
        for (what, expected, tamper) in cases {
            let mut tampered = set();
            tamper(&mut tampered);
            let bytes = tampered.to_cooked().expect("write");
            let message = ColliderSet::read(&bytes).expect_err(what).to_string();
            assert!(message.contains(expected), "{what}: `{message}` must say `{expected}`");
        }
    }

    #[test]
    fn another_types_bytes_are_refused() {
        let bytes = set().to_cooked().expect("write");
        #[derive(Clone, TypePath, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
        #[type_path = "sapphire::colliders"]
        struct Other {
            value: u32,
        }
        impl Versioned for Other {
            const VERSION: u32 = 1;
        }
        assert!(sapphire_serialize::access::<Other>(&bytes).is_err());
    }
}
