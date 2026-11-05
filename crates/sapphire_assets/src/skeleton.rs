//! Cooked skeletons (`.sskel`, M2 N1): a character's joints, as an asset of their own that meshes
//! and clips refer to by ID.
//!
//! A [`Skeleton`] is three parallel arrays over the joints, parents before children (the shape of
//! ozz-animation's runtime skeleton, spec §5.13):
//!
//! - **`parents`:** what local-to-model walks, once a frame, in order.
//! - **`rest`:** the rest pose as local transforms, copied whole before a clip's tracks overlay
//!   it, since a clip leaves out every channel that stays at rest ([`clip`](crate::clip)).
//! - **`names`:** what the cook maps tracks by, and the only strings here: nothing at run time
//!   reads them on a hot path, so they don't sit between the other two.
//!
//! The pipeline cooks one as a sub-asset of every skinned source: a mesh's skin names it
//! ([`Skin::skeleton`](crate::mesh::Skin::skeleton)), so it loads with the mesh, and its file sits
//! beside the mesh's (`alicia.smesh`, `alicia.sskel`, and a VRM's rig, `alicia.srig`). The
//! renderer never reads one: its palette arrives as finished matrices (spec §5.12).

use bevy_reflect::TypePath;
use sapphire_reflect::Versioned;
use sapphire_serialize::cooked::{AlignedVec, CookedError};

/// Conventional file extension for cooked skeletons.
pub const SKELETON_EXTENSION: &str = "sskel";

/// What [`Skeleton::parents`] holds for a root.
pub const NO_PARENT: u16 = u16::MAX;

/// The most joints a skeleton may have: an index is a u16, and [`NO_PARENT`] takes the last value.
pub const MAX_JOINTS: usize = NO_PARENT as usize;

/// Below this a rest rotation normalizes to noise, or to NaN at zero: well under what any
/// exporter's rounding leaves of a unit quaternion.
const MIN_ROTATION_LENGTH_SQUARED: f32 = 1e-6;

/// A joint's local transform: scale, then rotation, then translation.
#[derive(Clone, Copy, Debug, PartialEq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
#[rkyv(derive(Debug))]
pub struct JointTransform {
    pub translation: [f32; 3],
    /// A quaternion, `[x, y, z, w]`, as the source wrote it: normalize before using it.
    pub rotation: [f32; 4],
    pub scale: [f32; 3],
}

impl JointTransform {
    pub const IDENTITY: Self =
        Self { translation: [0.0; 3], rotation: [0.0, 0.0, 0.0, 1.0], scale: [1.0; 3] };
}

impl ArchivedJointTransform {
    pub fn to_native(&self) -> JointTransform {
        JointTransform {
            translation: self.translation.map(|c| c.to_native()),
            rotation: self.rotation.map(|c| c.to_native()),
            scale: self.scale.map(|c| c.to_native()),
        }
    }
}

/// One joint as importers and tests build it, before [`Skeleton::new`] lays the joints out.
#[derive(Clone, Debug, PartialEq)]
pub struct Joint {
    /// The source node's name, which animation clips and attachments find the joint by.
    pub name: String,
    /// The parent's index, always lower than this joint's; `None` for a root.
    pub parent: Option<u16>,
    /// The rest pose, relative to the parent (to model space for a root).
    pub rest: JointTransform,
}

/// A cooked skeleton. See the [module docs](self).
#[derive(Clone, Debug, Default, PartialEq, TypePath, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
#[rkyv(derive(Debug))]
#[type_path = "sapphire::assets"]
pub struct Skeleton {
    /// Each joint's parent, always lower than the joint itself, or [`NO_PARENT`] for a root.
    pub parents: Vec<u16>,
    /// Each joint's rest pose, relative to its parent (to model space for a root).
    pub rest: Vec<JointTransform>,
    /// Each joint's name: the source node's.
    pub names: Vec<String>,
}

impl Versioned for Skeleton {
    /// 1: the first, when skeletons left the mesh (M2 N1).
    const VERSION: u32 = 1;
}

/// A cooked skeleton that can't be read, or breaks an invariant of the format.
#[derive(Debug, thiserror::Error)]
pub enum SkeletonError {
    /// The envelope, the schema version or the archive itself.
    #[error(transparent)]
    Cooked(#[from] CookedError),
    #[error("a skeleton has at least one joint")]
    Empty,
    #[error("{0} joints, over the limit of {MAX_JOINTS}")]
    TooManyJoints(usize),
    #[error("{parents} parents, {rest} rest poses and {names} names: one of each per joint")]
    Lengths { parents: usize, rest: usize, names: usize },
    #[error("joint {joint}'s parent {parent} doesn't come before it")]
    JointOrder { joint: usize, parent: u16 },
    #[error("joint {joint}'s rest pose is not finite")]
    NonFinite { joint: usize },
    #[error("joint {joint}'s rest rotation has no length")]
    ZeroRotation { joint: usize },
    #[error("joint {joint}'s rest rotation is too large to normalize")]
    RotationOverflow { joint: usize },
}

impl Skeleton {
    /// Lays `joints` out. They must already be in the order [`Self::read`] requires.
    pub fn new(joints: &[Joint]) -> Self {
        Self {
            parents: joints.iter().map(|joint| joint.parent.unwrap_or(NO_PARENT)).collect(),
            rest: joints.iter().map(|joint| joint.rest).collect(),
            names: joints.iter().map(|joint| joint.name.clone()).collect(),
        }
    }

    /// The skeleton as cooked bytes: the envelope header, then the archive.
    pub fn to_cooked(&self) -> Result<AlignedVec, CookedError> {
        sapphire_serialize::to_cooked(self)
    }

    /// Reads cooked skeleton bytes in place and checks them ([`ArchivedSkeleton::validate`]). The
    /// bytes must start 16-byte aligned.
    pub fn read(bytes: &[u8]) -> Result<&ArchivedSkeleton, SkeletonError> {
        let skeleton = sapphire_serialize::access::<Self>(bytes)?;
        skeleton.validate()?;
        Ok(skeleton)
    }

    /// [`Self::read`], then a copy out of the archive, for tools and tests.
    pub fn from_cooked(bytes: &[u8]) -> Result<Self, SkeletonError> {
        let archived = Self::read(bytes)?;
        rkyv::deserialize::<Self, rkyv::rancor::Error>(archived)
            .map_err(|error| SkeletonError::Cooked(error.into()))
    }
}

impl ArchivedSkeleton {
    /// Checks every invariant in [`SkeletonError`]. [`Skeleton::read`] calls it.
    pub fn validate(&self) -> Result<(), SkeletonError> {
        let (parents, rest, names) = (self.parents.len(), self.rest.len(), self.names.len());
        if parents != rest || parents != names {
            return Err(SkeletonError::Lengths { parents, rest, names });
        }
        if parents == 0 {
            return Err(SkeletonError::Empty);
        }
        if parents > MAX_JOINTS {
            return Err(SkeletonError::TooManyJoints(parents));
        }
        for (joint, (parent, rest)) in self.parents.iter().zip(self.rest.iter()).enumerate() {
            let parent = parent.to_native();
            if parent != NO_PARENT && usize::from(parent) >= joint {
                return Err(SkeletonError::JointOrder { joint, parent });
            }
            let rest = rest.to_native();
            if !rest.translation.iter().chain(&rest.rotation).chain(&rest.scale).all(|c| c.is_finite()) {
                return Err(SkeletonError::NonFinite { joint });
            }
            let length_squared = rest.rotation.iter().map(|c| c * c).sum::<f32>();
            if !length_squared.is_finite() {
                return Err(SkeletonError::RotationOverflow { joint });
            }
            if length_squared < MIN_ROTATION_LENGTH_SQUARED {
                return Err(SkeletonError::ZeroRotation { joint });
            }
        }
        Ok(())
    }

    /// How many joints it has.
    pub fn len(&self) -> usize {
        self.parents.len()
    }

    pub fn is_empty(&self) -> bool {
        self.parents.is_empty()
    }

    /// The parent of `joint`, or `None` for a root.
    pub fn parent(&self, joint: usize) -> Option<usize> {
        let parent = self.parents[joint].to_native();
        (parent != NO_PARENT).then_some(usize::from(parent))
    }

    /// The first joint named `name`.
    pub fn find(&self, name: &str) -> Option<usize> {
        self.names.iter().position(|joint| joint.as_str() == name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn joints() -> Vec<Joint> {
        let joint = |name: &str, parent: Option<u16>, y: f32| Joint {
            name: name.to_owned(),
            parent,
            rest: JointTransform { translation: [0.0, y, 0.0], ..JointTransform::IDENTITY },
        };
        vec![joint("hips", None, 1.0), joint("spine", Some(0), 0.2), joint("head", Some(1), 0.4)]
    }

    #[test]
    fn a_skeleton_reads_in_place() {
        let skeleton = Skeleton::new(&joints());
        let bytes = skeleton.to_cooked().unwrap();
        let archived = Skeleton::read(&bytes).unwrap();
        assert_eq!(archived.len(), 3);
        assert_eq!((archived.parent(0), archived.parent(1), archived.parent(2)), (None, Some(0), Some(1)));
        assert_eq!(archived.find("head"), Some(2));
        assert_eq!(archived.find("tail"), None);
        assert_eq!(archived.rest[1].to_native().translation, [0.0, 0.2, 0.0]);
        assert_eq!(Skeleton::from_cooked(&bytes).unwrap(), skeleton);
    }

    /// Each row breaks one invariant and must come back as its error.
    #[test]
    fn a_bad_skeleton_is_refused() {
        type Edit = fn(&mut Skeleton);
        type Expected = fn(&SkeletonError) -> bool;
        let rows: [(&str, Edit, Expected); 6] = [
            ("empty", |s| *s = Skeleton::default(), |e| matches!(e, SkeletonError::Empty)),
            ("short names", |s| drop(s.names.pop()), |e| matches!(e, SkeletonError::Lengths { .. })),
            (
                "a parent after its child",
                |s| s.parents[1] = 2,
                |e| matches!(e, SkeletonError::JointOrder { joint: 1, parent: 2 }),
            ),
            (
                "its own parent",
                |s| s.parents[1] = 1,
                |e| matches!(e, SkeletonError::JointOrder { joint: 1, parent: 1 }),
            ),
            (
                "a NaN rest",
                |s| s.rest[2].scale[1] = f32::NAN,
                |e| matches!(e, SkeletonError::NonFinite { joint: 2 }),
            ),
            (
                "a zero rotation",
                |s| s.rest[2].rotation = [0.0; 4],
                |e| matches!(e, SkeletonError::ZeroRotation { joint: 2 }),
            ),
        ];
        let mut failures = Vec::new();
        for (name, edit, expected) in rows {
            let mut skeleton = Skeleton::new(&joints());
            edit(&mut skeleton);
            let bytes = skeleton.to_cooked().unwrap();
            match Skeleton::read(&bytes) {
                Err(error) if expected(&error) => {}
                other => failures.push(format!("{name}: {:?}", other.map(ArchivedSkeleton::len))),
            }
        }
        assert!(failures.is_empty(), "{failures:#?}");
    }

    #[test]
    fn rest_rotations_must_have_a_normalizable_finite_length() {
        for rotation in [[f32::MAX; 4], [f32::MAX, 0.0, 0.0, 0.0]] {
            let mut skeleton = Skeleton::new(&joints());
            skeleton.rest[0].rotation = rotation;
            let bytes = skeleton.to_cooked().unwrap();
            assert!(matches!(Skeleton::read(&bytes), Err(SkeletonError::RotationOverflow { joint: 0 })));
        }
        for rotation in [[0.0, 0.0, 0.0, 2.0], [1e18; 4]] {
            let mut skeleton = Skeleton::new(&joints());
            skeleton.rest[0].rotation = rotation;
            let bytes = skeleton.to_cooked().unwrap();
            assert!(Skeleton::read(&bytes).is_ok());
        }
    }

    #[test]
    fn too_many_joints_are_refused() {
        let joint = Joint { name: String::new(), parent: None, rest: JointTransform::IDENTITY };
        let skeleton = Skeleton::new(&vec![joint; MAX_JOINTS + 1]);
        let bytes = skeleton.to_cooked().unwrap();
        assert!(matches!(Skeleton::read(&bytes), Err(SkeletonError::TooManyJoints(_))));
    }
}
