//! Character rigs (`.srig`, M1 A6): what a character source says about its skeleton and face
//! beyond the mesh, kept for animation (`sapphire_anim`, M2) to read. Nothing simulates or
//! applies it yet.
//!
//! A [`CharacterRig`] holds, in VRM 1.0's model (the pipeline migrates VRM 0.x into it):
//!
//! - **Humanoid:** which skeleton joint is each [`HumanBone`], for retargeting clips and for
//!   look-at and foot IK (spec §5.13).
//! - **Expressions:** presets (`happy`, `blink`, `aa`, …) and custom ones, each a set of morph
//!   target weights, plus the material colors and UV transforms it animates (blush, eye
//!   swaps; C3's animated face channels). Faces become `MorphWeights` (spec §5.13).
//! - **Look-at:** the eyes' origin and range maps, driving eye joints or expressions.
//! - **Spring bones:** chains of joints swayed by stiffness, gravity and drag, with sphere and
//!   capsule colliders in groups: the data spec §5.13's secondary motion is ported from.
//! - **Constraints:** joints that follow a fraction of another's roll, aim or rotation (twist
//!   bones).
//!
//! # Indices
//!
//! The rig is cooked from the same import as its mesh, and refers to that mesh's skeleton
//! joints, morph targets and material slots by index. It records how many of each the mesh
//! has; [`ArchivedCharacterRig::check_mesh`] confirms a pairing before anything indexes the
//! mesh through it, and [`CharacterRig::read`] checks every index against the counts. Joint
//! indices are `u16`, as a skin's are.
//!
//! The pipeline cooks a rig as a sub-asset of its character source: its ID is derived from the
//! source's, and its file sits beside the mesh's (`alicia.smesh`, `alicia.srig`). A character
//! definition (M2) names the two by ID.

use bevy_reflect::TypePath;
use sapphire_reflect::Versioned;
use sapphire_serialize::cooked::{AlignedVec, CookedError};

use crate::mesh::ArchivedMesh;

/// Conventional file extension for cooked rigs.
pub const RIG_EXTENSION: &str = "srig";

/// A character's rig. See the [module docs](self).
#[derive(Clone, Debug, Default, PartialEq, TypePath, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
#[rkyv(derive(Debug))]
#[type_path = "sapphire::assets"]
pub struct CharacterRig {
    /// Joints in the mesh's skeleton (0 for an unskinned mesh).
    pub joint_count: u32,
    /// Morph targets in the mesh.
    pub morph_target_count: u32,
    /// Material slots in the mesh.
    pub material_slot_count: u32,
    /// Sorted by bone, each bone and each joint at most once. Empty for a rig that isn't
    /// humanoid; otherwise it has every [`HumanBone::REQUIRED`] bone.
    pub humanoid: Vec<HumanoidJoint>,
    /// Presets in [`ExpressionPreset::ALL`] order, then custom expressions by name.
    pub expressions: Vec<Expression>,
    pub look_at: Option<LookAt>,
    pub springs: Vec<Spring>,
    pub colliders: Vec<Collider>,
    pub collider_groups: Vec<ColliderGroup>,
    pub constraints: Vec<Constraint>,
}

impl Versioned for CharacterRig {
    /// 1: M1 A6.
    const VERSION: u32 = 1;
}

/// Declares [`HumanBone`] with VRM 1.0's names, in the specification's order.
macro_rules! human_bones {
    ($($bone:ident => $name:literal,)+) => {
        /// A bone of the humanoid skeleton, as VRM 1.0 names them.
        #[derive(
            Clone,
            Copy,
            Debug,
            PartialEq,
            Eq,
            PartialOrd,
            Ord,
            Hash,
            rkyv::Archive,
            rkyv::Serialize,
            rkyv::Deserialize,
            rkyv::Portable,
            rkyv::bytecheck::CheckBytes,
        )]
        #[rkyv(as = HumanBone)]
        #[bytecheck(crate = rkyv::bytecheck)]
        #[repr(u8)]
        pub enum HumanBone {
            $($bone,)+
        }

        impl HumanBone {
            /// Every bone, in order.
            pub const ALL: &'static [Self] = &[$(Self::$bone,)+];

            /// VRM 1.0's name: `hips`, `leftUpperArm`, `rightThumbMetacarpal`.
            pub fn name(self) -> &'static str {
                match self {
                    $(Self::$bone => $name,)+
                }
            }
        }
    };
}

human_bones! {
    Hips => "hips",
    Spine => "spine",
    Chest => "chest",
    UpperChest => "upperChest",
    Neck => "neck",
    Head => "head",
    LeftEye => "leftEye",
    RightEye => "rightEye",
    Jaw => "jaw",
    LeftUpperLeg => "leftUpperLeg",
    LeftLowerLeg => "leftLowerLeg",
    LeftFoot => "leftFoot",
    LeftToes => "leftToes",
    RightUpperLeg => "rightUpperLeg",
    RightLowerLeg => "rightLowerLeg",
    RightFoot => "rightFoot",
    RightToes => "rightToes",
    LeftShoulder => "leftShoulder",
    LeftUpperArm => "leftUpperArm",
    LeftLowerArm => "leftLowerArm",
    LeftHand => "leftHand",
    RightShoulder => "rightShoulder",
    RightUpperArm => "rightUpperArm",
    RightLowerArm => "rightLowerArm",
    RightHand => "rightHand",
    LeftThumbMetacarpal => "leftThumbMetacarpal",
    LeftThumbProximal => "leftThumbProximal",
    LeftThumbDistal => "leftThumbDistal",
    LeftIndexProximal => "leftIndexProximal",
    LeftIndexIntermediate => "leftIndexIntermediate",
    LeftIndexDistal => "leftIndexDistal",
    LeftMiddleProximal => "leftMiddleProximal",
    LeftMiddleIntermediate => "leftMiddleIntermediate",
    LeftMiddleDistal => "leftMiddleDistal",
    LeftRingProximal => "leftRingProximal",
    LeftRingIntermediate => "leftRingIntermediate",
    LeftRingDistal => "leftRingDistal",
    LeftLittleProximal => "leftLittleProximal",
    LeftLittleIntermediate => "leftLittleIntermediate",
    LeftLittleDistal => "leftLittleDistal",
    RightThumbMetacarpal => "rightThumbMetacarpal",
    RightThumbProximal => "rightThumbProximal",
    RightThumbDistal => "rightThumbDistal",
    RightIndexProximal => "rightIndexProximal",
    RightIndexIntermediate => "rightIndexIntermediate",
    RightIndexDistal => "rightIndexDistal",
    RightMiddleProximal => "rightMiddleProximal",
    RightMiddleIntermediate => "rightMiddleIntermediate",
    RightMiddleDistal => "rightMiddleDistal",
    RightRingProximal => "rightRingProximal",
    RightRingIntermediate => "rightRingIntermediate",
    RightRingDistal => "rightRingDistal",
    RightLittleProximal => "rightLittleProximal",
    RightLittleIntermediate => "rightLittleIntermediate",
    RightLittleDistal => "rightLittleDistal",
}

impl HumanBone {
    /// The bones every humanoid has (VRM 1.0's required set): the torso's hips and spine, the
    /// head, and both legs and arms down to the feet and hands.
    pub const REQUIRED: &'static [Self] = &[
        Self::Hips,
        Self::Spine,
        Self::Head,
        Self::LeftUpperLeg,
        Self::LeftLowerLeg,
        Self::LeftFoot,
        Self::RightUpperLeg,
        Self::RightLowerLeg,
        Self::RightFoot,
        Self::LeftUpperArm,
        Self::LeftLowerArm,
        Self::LeftHand,
        Self::RightUpperArm,
        Self::RightLowerArm,
        Self::RightHand,
    ];

    /// The bone VRM 1.0 calls `name`.
    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.iter().copied().find(|bone| bone.name() == name)
    }
}

/// A humanoid bone and the joint that is it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
#[rkyv(derive(Debug))]
pub struct HumanoidJoint {
    pub bone: HumanBone,
    pub joint: u16,
}

/// Declares a fieldless enum archived as itself, with VRM 1.0's names.
macro_rules! named_enum {
    ($(#[$meta:meta])* $enum:ident { $($(#[$variant_meta:meta])* $variant:ident => $name:literal,)+ }) => {
        $(#[$meta])*
        #[derive(
            Clone,
            Copy,
            Debug,
            PartialEq,
            Eq,
            PartialOrd,
            Ord,
            Hash,
            rkyv::Archive,
            rkyv::Serialize,
            rkyv::Deserialize,
            rkyv::Portable,
            rkyv::bytecheck::CheckBytes,
        )]
        #[rkyv(as = $enum)]
        #[bytecheck(crate = rkyv::bytecheck)]
        #[repr(u8)]
        pub enum $enum {
            $($(#[$variant_meta])* $variant,)+
        }

        impl $enum {
            /// Every value, in order.
            pub const ALL: &'static [Self] = &[$(Self::$variant,)+];

            /// VRM 1.0's name.
            pub fn name(self) -> &'static str {
                match self {
                    $(Self::$variant => $name,)+
                }
            }

            /// The value VRM 1.0 calls `name`.
            pub fn from_name(name: &str) -> Option<Self> {
                Self::ALL.iter().copied().find(|value| value.name() == name)
            }
        }
    };
}

named_enum! {
    /// VRM 1.0's preset expressions.
    ExpressionPreset {
        Happy => "happy",
        Angry => "angry",
        Sad => "sad",
        Relaxed => "relaxed",
        Surprised => "surprised",
        Aa => "aa",
        Ih => "ih",
        Ou => "ou",
        Ee => "ee",
        Oh => "oh",
        Blink => "blink",
        BlinkLeft => "blinkLeft",
        BlinkRight => "blinkRight",
        LookUp => "lookUp",
        LookDown => "lookDown",
        LookLeft => "lookLeft",
        LookRight => "lookRight",
        Neutral => "neutral",
    }
}

named_enum! {
    /// What an active expression does to the blink, look-at or mouth expressions.
    ExpressionOverride {
        /// Nothing.
        None => "none",
        /// Turns them off while this expression is on at all.
        Block => "block",
        /// Scales them down by this expression's weight.
        Blend => "blend",
    }
}

named_enum! {
    /// A material color an expression animates. The anime character family carries base, shade,
    /// rim and outline colors; emission and matcap tints wait for C7's materials.
    MaterialColor {
        Base => "color",
        Emission => "emissionColor",
        Shade => "shadeColor",
        Matcap => "matcapColor",
        Rim => "rimColor",
        Outline => "outlineColor",
    }
}

/// An expression: what it does at full weight. At weight `w`, morph targets take `w ×` theirs
/// and colors and UV transforms move `w` of the way from the material's own.
#[derive(Clone, Debug, PartialEq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
#[rkyv(derive(Debug))]
pub struct Expression {
    /// The preset's name ([`ExpressionPreset::name`]) or the custom expression's; unique.
    pub name: String,
    pub preset: Option<ExpressionPreset>,
    /// The weight snaps to 0 or 1 (at 0.5).
    pub binary: bool,
    pub override_blink: ExpressionOverride,
    pub override_look_at: ExpressionOverride,
    pub override_mouth: ExpressionOverride,
    /// By increasing target, each target once.
    pub morph_targets: Vec<MorphTargetWeight>,
    pub material_colors: Vec<MaterialColorBind>,
    pub texture_transforms: Vec<TextureTransformBind>,
}

/// A morph target's weight in an expression.
#[derive(Clone, Copy, Debug, PartialEq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
#[rkyv(derive(Debug))]
pub struct MorphTargetWeight {
    /// An index into the mesh's morph targets.
    pub target: u32,
    /// 0 to 1.
    pub weight: f32,
}

/// A material color an expression sets.
#[derive(Clone, Copy, Debug, PartialEq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
#[rkyv(derive(Debug))]
pub struct MaterialColorBind {
    /// An index into the mesh's material slots.
    pub slot: u32,
    pub color: MaterialColor,
    /// Linear RGBA at full weight.
    pub value: [f32; 4],
}

/// A UV transform an expression sets on a material's textures (an eye or mouth atlas swap).
#[derive(Clone, Copy, Debug, PartialEq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
#[rkyv(derive(Debug))]
pub struct TextureTransformBind {
    /// An index into the mesh's material slots.
    pub slot: u32,
    /// `uv × scale + offset`, in glTF's UV space (origin at the image's top left).
    pub scale: [f32; 2],
    pub offset: [f32; 2],
}

/// Where the eyes look from, and how gaze angles map onto them.
#[derive(Clone, Copy, Debug, PartialEq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
#[rkyv(derive(Debug))]
pub struct LookAt {
    /// The eyes' origin, in the head joint's space, meters.
    pub offset_from_head: [f32; 3],
    pub kind: LookAtKind,
    /// Yaw toward the nose (each eye's inner side).
    pub horizontal_inner: LookAtRange,
    pub horizontal_outer: LookAtRange,
    pub vertical_down: LookAtRange,
    pub vertical_up: LookAtRange,
}

named_enum! {
    /// What the look-at moves.
    LookAtKind {
        /// The eye joints, by degrees.
        Bone => "bone",
        /// The `lookUp`/`lookDown`/`lookLeft`/`lookRight` expressions, by weight.
        Expression => "expression",
    }
}

/// A linear map from a gaze angle to the eyes' response.
#[derive(Clone, Copy, Debug, PartialEq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
#[rkyv(derive(Debug))]
pub struct LookAtRange {
    /// The gaze angle, in degrees, at which the response is full; larger angles clamp.
    pub input_max_degrees: f32,
    /// The full response: degrees of eye rotation, or an expression weight.
    pub output_scale: f32,
}

/// A spring-bone chain: joints from the root toward the tip, each a descendant of the one
/// before. Each joint but the tip swings toward the next, its tail.
#[derive(Clone, Debug, PartialEq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
#[rkyv(derive(Debug))]
pub struct Spring {
    pub name: String,
    /// At least one.
    pub joints: Vec<SpringJoint>,
    pub tip: SpringTip,
    /// Indices into [`CharacterRig::collider_groups`].
    pub collider_groups: Vec<u32>,
    /// The joint whose space the simulation runs in, so moving it (a vehicle, a warp) doesn't
    /// swing the chain; model space when `None`.
    pub center: Option<u16>,
}

/// One joint of a [`Spring`] and its parameters.
#[derive(Clone, Copy, Debug, PartialEq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
#[rkyv(derive(Debug))]
pub struct SpringJoint {
    pub joint: u16,
    /// The radius colliders push the joint's tail out by, meters.
    pub hit_radius: f32,
    /// The pull back toward the rest pose.
    pub stiffness: f32,
    /// Gravity's acceleration on the tail.
    pub gravity_power: f32,
    /// A unit vector in model space (wind is a sideways gravity).
    pub gravity_dir: [f32; 3],
    /// Velocity lost per step, 0 to 1.
    pub drag_force: f32,
}

/// How a spring's last joint ends it.
#[derive(Clone, Copy, Debug, PartialEq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
#[rkyv(derive(Debug))]
pub enum SpringTip {
    /// The last joint is the tail of the one before and doesn't swing itself (VRM 1.0).
    Joint,
    /// The last joint swings too, toward a virtual tail this many meters past it along the
    /// bone from its parent (VRM 0.x, whose leaves swing toward a 7 cm tail).
    Extended { length: f32 },
}

/// A collider on a joint.
#[derive(Clone, Copy, Debug, PartialEq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
#[rkyv(derive(Debug))]
pub struct Collider {
    pub joint: u16,
    pub shape: ColliderShape,
}

/// A collider's shape, in its joint's space, meters.
#[derive(Clone, Copy, Debug, PartialEq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
#[rkyv(derive(Debug))]
pub enum ColliderShape {
    Sphere {
        offset: [f32; 3],
        radius: f32,
    },
    /// A sphere swept from `offset` to `tail`.
    Capsule {
        offset: [f32; 3],
        radius: f32,
        tail: [f32; 3],
    },
}

/// Colliders that springs collide with together.
#[derive(Clone, Debug, PartialEq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
#[rkyv(derive(Debug))]
pub struct ColliderGroup {
    pub name: String,
    /// Indices into [`CharacterRig::colliders`].
    pub colliders: Vec<u32>,
}

/// A joint that follows part of another joint's motion.
#[derive(Clone, Copy, Debug, PartialEq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
#[rkyv(derive(Debug))]
pub struct Constraint {
    /// The constrained joint; a joint has at most one constraint.
    pub joint: u16,
    /// The joint it follows; never itself, and constraints form no cycle.
    pub source: u16,
    /// How much of the source's motion it takes, 0 to 1.
    pub weight: f32,
    pub kind: ConstraintKind,
}

/// What a [`Constraint`] transfers, each relative to the joints' rest poses.
#[derive(Clone, Copy, Debug, PartialEq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
#[rkyv(derive(Debug))]
pub enum ConstraintKind {
    /// The source's twist around one of its local axes (forearm twist bones).
    Roll { axis: Axis },
    /// Turns the joint's local axis toward the source.
    Aim { axis: AimAxis },
    /// The source's whole rotation.
    Rotation,
}

named_enum! {
    /// A local axis.
    Axis {
        X => "X",
        Y => "Y",
        Z => "Z",
    }
}

named_enum! {
    /// A signed local axis.
    AimAxis {
        PositiveX => "PositiveX",
        NegativeX => "NegativeX",
        PositiveY => "PositiveY",
        NegativeY => "NegativeY",
        PositiveZ => "PositiveZ",
        NegativeZ => "NegativeZ",
    }
}

/// A cooked rig that can't be read, or breaks an invariant of the format.
#[derive(Debug, thiserror::Error)]
pub enum RigError {
    /// The envelope, the schema version or the archive itself.
    #[error(transparent)]
    Cooked(#[from] CookedError),
    #[error("{what} names joint {joint}, but the mesh has {joint_count}")]
    Joint { what: String, joint: u16, joint_count: u32 },
    #[error("expression {expression:?} names morph target {target}, but the mesh has {count}")]
    MorphTarget { expression: String, target: u32, count: u32 },
    #[error("expression {expression:?} names material slot {slot}, but the mesh has {count}")]
    MaterialSlot { expression: String, slot: u32, count: u32 },
    #[error("{what} names {index}, but there are {count}")]
    Index { what: String, index: u32, count: usize },
    #[error("{what} {reason}")]
    Invalid { what: String, reason: &'static str },
    #[error("the humanoid has no {} bone", .0.name())]
    MissingBone(HumanBone),
    #[error("the rig expects a mesh with {expected}, but it has {found}")]
    MeshMismatch { expected: String, found: String },
}

fn invalid(what: impl Into<String>, reason: &'static str) -> RigError {
    RigError::Invalid { what: what.into(), reason }
}

impl CharacterRig {
    /// The rig as cooked bytes: the envelope header, then the archive.
    pub fn to_cooked(&self) -> Result<AlignedVec, CookedError> {
        sapphire_serialize::to_cooked(self)
    }

    /// Reads cooked rig bytes in place and checks them ([`ArchivedCharacterRig::validate`]).
    /// The bytes must start 16-byte aligned.
    pub fn read(bytes: &[u8]) -> Result<&ArchivedCharacterRig, RigError> {
        let rig = sapphire_serialize::access::<Self>(bytes)?;
        rig.validate()?;
        Ok(rig)
    }

    /// [`Self::read`], then a copy out of the archive, for tools and tests.
    pub fn from_cooked(bytes: &[u8]) -> Result<Self, RigError> {
        let archived = Self::read(bytes)?;
        rkyv::deserialize::<Self, rkyv::rancor::Error>(archived)
            .map_err(|error| RigError::Cooked(error.into()))
    }

    /// The expression named `name`.
    pub fn expression(&self, name: &str) -> Option<&Expression> {
        self.expressions.iter().find(|expression| expression.name == name)
    }

    /// The joint that is `bone`.
    pub fn human_joint(&self, bone: HumanBone) -> Option<u16> {
        self.humanoid.iter().find(|entry| entry.bone == bone).map(|entry| entry.joint)
    }
}

impl ArchivedCharacterRig {
    /// Checks what animation relies on beyond bytecheck: every joint, morph target, material
    /// slot, collider and collider group index is in range; the humanoid is sorted, unique and
    /// complete; expression names are unique and presets carry their own; every number is
    /// finite and in its range (gravity directions unit); and each joint has at most one
    /// constraint, and constraints form no cycle. Whether a spring's joints and center descend
    /// from one another is the skeleton's to say; the pipeline checks the joints.
    pub fn validate(&self) -> Result<(), RigError> {
        let joint_count = self.joint_count.to_native();
        let joint = |what: &dyn Fn() -> String, joint: u16| {
            if u32::from(joint) < joint_count {
                Ok(())
            } else {
                Err(RigError::Joint { what: what(), joint, joint_count })
            }
        };

        let mut joints_seen = std::collections::HashSet::new();
        for (index, entry) in self.humanoid.iter().enumerate() {
            let bone = entry.bone;
            joint(&|| format!("humanoid bone {}", bone.name()), entry.joint.to_native())?;
            if index > 0 && self.humanoid[index - 1].bone >= bone {
                return Err(invalid(format!("humanoid bone {}", bone.name()), "is out of order or repeated"));
            }
            if !joints_seen.insert(entry.joint.to_native()) {
                return Err(invalid(
                    format!("humanoid bone {}", bone.name()),
                    "shares its joint with another bone",
                ));
            }
        }
        if !self.humanoid.is_empty()
            && let Some(&missing) = HumanBone::REQUIRED
                .iter()
                .find(|&&bone| !self.humanoid.iter().any(|entry| entry.bone == bone))
        {
            return Err(RigError::MissingBone(missing));
        }

        self.validate_expressions()?;

        if let Some(look_at) = self.look_at.as_ref() {
            finite_all("look-at offset", &look_at.offset_from_head.map(|v| v.to_native()))?;
            for (name, range) in [
                ("horizontal inner", &look_at.horizontal_inner),
                ("horizontal outer", &look_at.horizontal_outer),
                ("vertical down", &look_at.vertical_down),
                ("vertical up", &look_at.vertical_up),
            ] {
                non_negative(&format!("look-at {name} input"), range.input_max_degrees.to_native())?;
                finite(&format!("look-at {name} output"), range.output_scale.to_native())?;
            }
        }

        let groups = self.collider_groups.len();
        for (index, spring) in self.springs.iter().enumerate() {
            let what = |detail: &str| format!("spring {index} ({:?}) {detail}", spring.name.as_str());
            if spring.joints.is_empty() {
                return Err(invalid(format!("spring {index} ({:?})", spring.name.as_str()), "has no joints"));
            }
            for (k, entry) in spring.joints.iter().enumerate() {
                joint(&|| what(&format!("joint {k}")), entry.joint.to_native())?;
                let field = |name: &str| what(&format!("joint {k} {name}"));
                non_negative(&field("hit radius"), entry.hit_radius.to_native())?;
                non_negative(&field("stiffness"), entry.stiffness.to_native())?;
                non_negative(&field("gravity power"), entry.gravity_power.to_native())?;
                let gravity = entry.gravity_dir.map(|v| v.to_native());
                let length = gravity.iter().map(|c| c * c).sum::<f32>().sqrt();
                if length.is_nan() || (length - 1.0).abs() > 1e-3 {
                    return Err(invalid(field("gravity direction"), "must be a unit vector"));
                }
                let drag = entry.drag_force.to_native();
                if !(0.0..=1.0).contains(&drag) {
                    return Err(invalid(field("drag force"), "must be between 0 and 1"));
                }
            }
            if let ArchivedSpringTip::Extended { length } = &spring.tip
                && !(length.to_native().is_finite() && length.to_native() > 0.0)
            {
                return Err(invalid(what("tip length"), "must be finite and positive"));
            }
            for &group in spring.collider_groups.iter() {
                let group = group.to_native();
                if group as usize >= groups {
                    return Err(RigError::Index {
                        what: what("collider group"),
                        index: group,
                        count: groups,
                    });
                }
            }
            if let Some(center) = spring.center.as_ref() {
                joint(&|| what("center"), center.to_native())?;
            }
        }

        for (index, collider) in self.colliders.iter().enumerate() {
            let what = || format!("collider {index}");
            joint(&what, collider.joint.to_native())?;
            let (offset, radius, tail) = match &collider.shape {
                ArchivedColliderShape::Sphere { offset, radius } => (offset, radius, None),
                ArchivedColliderShape::Capsule { offset, radius, tail } => (offset, radius, Some(tail)),
            };
            finite_all(&format!("{} offset", what()), &offset.map(|v| v.to_native()))?;
            non_negative(&format!("{} radius", what()), radius.to_native())?;
            if let Some(tail) = tail {
                finite_all(&format!("{} tail", what()), &tail.map(|v| v.to_native()))?;
            }
        }
        let colliders = self.colliders.len();
        for (index, group) in self.collider_groups.iter().enumerate() {
            for &collider in group.colliders.iter() {
                let collider = collider.to_native();
                if collider as usize >= colliders {
                    let what = format!("collider group {index} ({:?})", group.name.as_str());
                    return Err(RigError::Index { what, index: collider, count: colliders });
                }
            }
        }

        let mut constrained = std::collections::HashSet::new();
        for (index, constraint) in self.constraints.iter().enumerate() {
            let what = |detail: &str| format!("constraint {index} {detail}");
            joint(&|| what("joint"), constraint.joint.to_native())?;
            joint(&|| what("source"), constraint.source.to_native())?;
            if constraint.joint == constraint.source {
                return Err(invalid(format!("constraint {index}"), "follows its own joint"));
            }
            if !constrained.insert(constraint.joint.to_native()) {
                return Err(invalid(
                    format!("constraint {index}"),
                    "constrains a joint another constraint does",
                ));
            }
            let weight = constraint.weight.to_native();
            if !(0.0..=1.0).contains(&weight) {
                return Err(invalid(what("weight"), "must be between 0 and 1"));
            }
        }
        self.check_constraint_cycles()
    }

    fn validate_expressions(&self) -> Result<(), RigError> {
        let mut names = std::collections::HashSet::new();
        for expression in self.expressions.iter() {
            let name = expression.name.as_str();
            let what = |detail: &str| format!("expression {name:?} {detail}");
            if name.is_empty() || !names.insert(name) {
                return Err(invalid(format!("expression {name:?}"), "has an empty or repeated name"));
            }
            if let Some(preset) = expression.preset.as_ref()
                && preset.name() != name
            {
                return Err(invalid(format!("expression {name:?}"), "must be named after its preset"));
            }
            let mut previous = None;
            for bind in expression.morph_targets.iter() {
                let target = bind.target.to_native();
                if target >= self.morph_target_count.to_native() {
                    let count = self.morph_target_count.to_native();
                    return Err(RigError::MorphTarget { expression: name.to_owned(), target, count });
                }
                if previous.is_some_and(|previous| previous >= target) {
                    return Err(invalid(what("morph targets"), "are out of order or repeated"));
                }
                previous = Some(target);
                let weight = bind.weight.to_native();
                if !(0.0..=1.0).contains(&weight) {
                    return Err(invalid(what("morph target weight"), "must be between 0 and 1"));
                }
            }
            let slot = |slot: u32| {
                let count = self.material_slot_count.to_native();
                if slot < count {
                    Ok(())
                } else {
                    Err(RigError::MaterialSlot { expression: name.to_owned(), slot, count })
                }
            };
            for bind in expression.material_colors.iter() {
                slot(bind.slot.to_native())?;
                let value = bind.value.map(|v| v.to_native());
                if !value.iter().all(|c| c.is_finite() && *c >= 0.0) {
                    return Err(invalid(what("material color"), "must be finite and at least 0"));
                }
            }
            for bind in expression.texture_transforms.iter() {
                slot(bind.slot.to_native())?;
                finite_all(&what("UV scale"), &bind.scale.map(|v| v.to_native()))?;
                finite_all(&what("UV offset"), &bind.offset.map(|v| v.to_native()))?;
            }
        }
        Ok(())
    }

    /// A cycle would make the evaluation order undefined. Each joint has at most one constraint
    /// (checked before), so following sources from a joint is a single path.
    fn check_constraint_cycles(&self) -> Result<(), RigError> {
        let source_of = |joint: u16| {
            self.constraints.iter().find(|c| c.joint.to_native() == joint).map(|c| c.source.to_native())
        };
        for (index, constraint) in self.constraints.iter().enumerate() {
            let mut current = constraint.source.to_native();
            // A path longer than the constraint count has repeated a joint.
            for _ in 0..=self.constraints.len() {
                if current == constraint.joint.to_native() {
                    return Err(invalid(format!("constraint {index}"), "is part of a cycle of constraints"));
                }
                match source_of(current) {
                    Some(next) => current = next,
                    None => break,
                }
            }
        }
        Ok(())
    }

    /// Checks that `mesh` is the mesh this rig was cooked with, as far as the rig's indices go:
    /// the same number of joints, morph targets and material slots.
    pub fn check_mesh(&self, mesh: &ArchivedMesh) -> Result<(), RigError> {
        let joints = mesh.skin.as_ref().map_or(0, |skin| skin.joints.len());
        let found = [joints, mesh.morph_targets.len(), mesh.materials.len()];
        let expected = [self.joint_count, self.morph_target_count, self.material_slot_count]
            .map(|count| count.to_native() as usize);
        if expected == found {
            return Ok(());
        }
        let describe = |[joints, targets, slots]: [usize; 3]| {
            format!("{joints} joints, {targets} morph targets and {slots} material slots")
        };
        Err(RigError::MeshMismatch { expected: describe(expected), found: describe(found) })
    }
}

fn finite(what: &str, value: f32) -> Result<(), RigError> {
    if value.is_finite() { Ok(()) } else { Err(invalid(what, "must be finite")) }
}

fn finite_all(what: &str, values: &[f32]) -> Result<(), RigError> {
    if values.iter().all(|v| v.is_finite()) { Ok(()) } else { Err(invalid(what, "must be finite")) }
}

fn non_negative(what: &str, value: f32) -> Result<(), RigError> {
    if value.is_finite() && value >= 0.0 {
        Ok(())
    } else {
        Err(invalid(what, "must be finite and at least 0"))
    }
}

#[cfg(test)]
mod tests;
