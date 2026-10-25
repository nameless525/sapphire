//! A VRM's character data in VRM 1.0 semantics, by glTF index: what VRM 1.0 files hold
//! ([`VrmData::from_v1`]), and what VRM 0.x files migrate to (`v0`). [`super::rig`] resolves
//! the indices against the imported mesh.
//!
//! Values outside the specification's ranges clamp into them (weights and drag to `[0, 1]`,
//! radii, stiffness and gravity to at least 0, a zero gravity direction to down), so a
//! sloppy export still cooks; references to nodes, meshes, morph targets, materials and
//! colliders that don't exist are errors.

use gltf::Document;
use sapphire_assets::rig::{
    AimAxis, Axis, ColliderGroup, ColliderShape, ConstraintKind, ExpressionOverride, ExpressionPreset,
    HumanBone, LookAt, LookAtKind, LookAtRange, MaterialColor, SpringTip,
};
use serde::{Deserialize, Serialize};
use vrm_spec::vrmc_spring_bone_1_0::VrmcSpringBoneSchema;
use vrm_spec::vrmc_vrm_1_0::{Expression, LookAtRangeMap, VRMCVrmSchema};

use super::{CONSTRAINT, SPRING_BONE, VrmImportError};

/// See the [module docs](self).
#[derive(Clone, Debug, Default, PartialEq)]
pub(super) struct VrmData {
    /// Sorted by bone.
    pub humanoid: Vec<(HumanBone, usize)>,
    /// Presets in [`ExpressionPreset::ALL`] order, then custom expressions by name.
    pub expressions: Vec<ExpressionData>,
    pub look_at: Option<LookAt>,
    pub springs: Vec<SpringData>,
    pub colliders: Vec<ColliderData>,
    /// Indices into `colliders`, checked.
    pub collider_groups: Vec<ColliderGroup>,
    pub constraints: Vec<ConstraintData>,
}

#[derive(Clone, Debug, PartialEq)]
pub(super) struct ExpressionData {
    pub name: String,
    pub preset: Option<ExpressionPreset>,
    pub binary: bool,
    /// Blink, look-at, mouth.
    pub overrides: [ExpressionOverride; 3],
    pub morph_targets: Vec<MorphBind>,
    /// glTF material, color, linear RGBA.
    pub material_colors: Vec<(usize, MaterialColor, [f32; 4])>,
    /// glTF material, scale, offset.
    pub texture_transforms: Vec<(usize, [f32; 2], [f32; 2])>,
}

/// Morph target `target` of glTF mesh `mesh`, checked against the mesh.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct MorphBind {
    pub mesh: usize,
    pub target: usize,
    pub weight: f32,
}

#[derive(Clone, Debug, PartialEq)]
pub(super) struct SpringData {
    pub name: String,
    /// Nodes from the root toward the tip.
    pub joints: Vec<(usize, JointParams)>,
    pub tip: SpringTip,
    /// Indices into the collider groups, checked.
    pub collider_groups: Vec<u32>,
    pub center: Option<usize>,
}

/// A spring joint's parameters.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct JointParams {
    pub hit_radius: f32,
    pub stiffness: f32,
    pub gravity_power: f32,
    pub gravity_dir: [f32; 3],
    pub drag_force: f32,
}

impl JointParams {
    /// The parameters with every value in its range.
    pub fn clamped(self) -> Self {
        let length = self.gravity_dir.iter().map(|c| c * c).sum::<f32>().sqrt();
        Self {
            hit_radius: self.hit_radius.max(0.0),
            stiffness: self.stiffness.max(0.0),
            gravity_power: self.gravity_power.max(0.0),
            gravity_dir: if length > 1e-6 { self.gravity_dir.map(|c| c / length) } else { [0.0, -1.0, 0.0] },
            drag_force: self.drag_force.clamp(0.0, 1.0),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct ColliderData {
    pub node: usize,
    pub shape: ColliderShape,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct ConstraintData {
    /// The constrained node.
    pub node: usize,
    pub source: usize,
    pub weight: f32,
    pub kind: ConstraintKind,
}

/// The name a vrm-spec enum serializes to: VRM's own spelling, which `sapphire_assets::rig`'s
/// enums share.
pub(super) fn spelling<T: Serialize>(value: &T) -> String {
    serde_json::to_value(value).ok().and_then(|value| value.as_str().map(str::to_owned)).unwrap_or_default()
}

/// Checks a node index against the document.
pub(super) fn node(
    document: &Document,
    what: impl FnOnce() -> String,
    node: usize,
) -> Result<usize, VrmImportError> {
    let count = document.nodes().len();
    if node < count {
        Ok(node)
    } else {
        Err(VrmImportError::reference(what(), format!("names node {node}, but the document has {count}")))
    }
}

/// Checks a material index against the document.
pub(super) fn material(
    document: &Document,
    what: impl FnOnce() -> String,
    material: usize,
) -> Result<usize, VrmImportError> {
    let count = document.materials().len();
    if material < count {
        Ok(material)
    } else {
        Err(VrmImportError::reference(
            what(),
            format!("names material {material}, but the document has {count}"),
        ))
    }
}

/// Checks that mesh `mesh` has a morph target `target`.
pub(super) fn morph_bind(
    document: &Document,
    what: &dyn Fn() -> String,
    mesh: usize,
    target: usize,
    weight: f32,
) -> Result<MorphBind, VrmImportError> {
    let Some(gltf_mesh) = document.meshes().nth(mesh) else {
        let count = document.meshes().len();
        return Err(VrmImportError::reference(
            what(),
            format!("names mesh {mesh}, but the document has {count}"),
        ));
    };
    let targets = gltf_mesh.primitives().map(|primitive| primitive.morph_targets().len()).max().unwrap_or(0);
    if target >= targets {
        return Err(VrmImportError::reference(
            what(),
            format!("names morph target {target} of mesh {mesh}, which has {targets}"),
        ));
    }
    Ok(MorphBind { mesh, target, weight: weight.clamp(0.0, 1.0) })
}

/// `values` as `N` floats, or `default` when absent or of another length.
fn floats<const N: usize>(values: Option<&Vec<f64>>, default: [f32; N]) -> [f32; N] {
    values
        .filter(|values| values.len() == N)
        .map_or(default, |values| std::array::from_fn(|i| values[i] as f32))
}

/// The `VRMC_node_constraint` extension on a node (VRM 1.0). pixiv's vrm-spec crate has no
/// types for it, so these follow the specification's schema.
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct NodeConstraintExtension {
    spec_version: String,
    constraint: ConstraintJson,
}

/// Exactly one of the three.
#[derive(Clone, Debug, Deserialize)]
struct ConstraintJson {
    roll: Option<RollConstraint>,
    aim: Option<AimConstraint>,
    rotation: Option<RotationConstraint>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RollConstraint {
    source: usize,
    roll_axis: String,
    weight: Option<f32>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AimConstraint {
    source: usize,
    aim_axis: String,
    weight: Option<f32>,
}

#[derive(Clone, Debug, Deserialize)]
struct RotationConstraint {
    source: usize,
    weight: Option<f32>,
}

impl VrmData {
    /// The data of a VRM 1.0 file: its `VRMC_vrm`, `VRMC_springBone` (if any) and every
    /// node's `VRMC_node_constraint`.
    pub fn from_v1(
        document: &Document,
        vrm: &VRMCVrmSchema,
        springs: Option<&VrmcSpringBoneSchema>,
    ) -> Result<Self, VrmImportError> {
        let mut data = Self::default();
        for (name, bone) in &vrm.humanoid.human_bones.0 {
            let name = spelling(name);
            let what = || format!("humanoid bone {name}");
            let bone_index = bone
                .as_ref()
                .and_then(|bone| bone.node)
                .ok_or_else(|| VrmImportError::reference(what(), "names no node".into()))?;
            let bone = HumanBone::from_name(&name)
                .ok_or_else(|| VrmImportError::reference(what(), "is not a VRM 1.0 bone".into()))?;
            data.humanoid.push((bone, node(document, what, bone_index.value())?));
        }
        data.humanoid.sort_by_key(|&(bone, _)| bone);

        if let Some(expressions) = &vrm.expressions {
            let mut presets: Vec<(ExpressionPreset, &Expression)> = Vec::new();
            for (name, expression) in expressions.preset.iter().flat_map(|preset| &preset.0) {
                let name = spelling(name);
                let preset = ExpressionPreset::from_name(&name).ok_or_else(|| {
                    VrmImportError::reference(
                        format!("expression {name:?}"),
                        "is not a VRM 1.0 preset".into(),
                    )
                })?;
                presets.push((preset, expression));
            }
            presets.sort_by_key(|&(preset, _)| preset);
            for (preset, expression) in presets {
                data.expressions.push(expression_v1(document, preset.name(), Some(preset), expression)?);
            }
            let mut custom: Vec<(&String, &Expression)> = expressions.custom.iter().flatten().collect();
            custom.sort_by_key(|&(name, _)| name);
            for (name, expression) in custom {
                if ExpressionPreset::from_name(name).is_some() {
                    return Err(VrmImportError::reference(
                        format!("custom expression {name:?}"),
                        "has a preset's name".into(),
                    ));
                }
                data.expressions.push(expression_v1(document, name, None, expression)?);
            }
        }

        if let Some(look_at) = &vrm.look_at {
            let kind = look_at
                .look_at_type
                .and_then(|kind| LookAtKind::from_name(&spelling(&kind)))
                .unwrap_or(LookAtKind::Bone);
            // A full response is 10 degrees of eye rotation, or an expression at full weight.
            let output = if kind == LookAtKind::Bone { 10.0 } else { 1.0 };
            let range = |map: &Option<LookAtRangeMap>| LookAtRange {
                input_max_degrees: map
                    .as_ref()
                    .and_then(|m| m.input_max_value)
                    .map_or(90.0, |v| v as f32)
                    .max(0.0),
                output_scale: map.as_ref().and_then(|m| m.output_scale).map_or(output, |v| v as f32),
            };
            data.look_at = Some(LookAt {
                offset_from_head: floats(look_at.offset_from_head_bone.as_ref(), [0.0; 3]),
                kind,
                horizontal_inner: range(&look_at.range_map_horizontal_inner),
                horizontal_outer: range(&look_at.range_map_horizontal_outer),
                vertical_down: range(&look_at.range_map_vertical_down),
                vertical_up: range(&look_at.range_map_vertical_up),
            });
        }

        if let Some(springs) = springs {
            data.add_springs_v1(document, springs)?;
        }
        data.add_constraints(document)?;
        Ok(data)
    }

    fn add_springs_v1(
        &mut self,
        document: &Document,
        schema: &VrmcSpringBoneSchema,
    ) -> Result<(), VrmImportError> {
        if schema.spec_version != "1.0" {
            return Err(VrmImportError::SpecVersion {
                extension: SPRING_BONE,
                found: schema.spec_version.clone(),
            });
        }
        for (index, collider) in schema.colliders.iter().flatten().enumerate() {
            let what = || format!("spring collider {index}");
            let collider_node = node(document, what, collider.node.value())?;
            let shape = match (&collider.shape.sphere, &collider.shape.capsule) {
                (Some(sphere), None) => ColliderShape::Sphere {
                    offset: sphere.offset.map_or([0.0; 3], |v| v.map(|c| c as f32)),
                    radius: sphere.radius.map_or(0.0, |r| r as f32).max(0.0),
                },
                (None, Some(capsule)) => ColliderShape::Capsule {
                    offset: capsule.offset.map_or([0.0; 3], |v| v.map(|c| c as f32)),
                    radius: capsule.radius.map_or(0.0, |r| r as f32).max(0.0),
                    tail: capsule.tail.map_or([0.0; 3], |v| v.map(|c| c as f32)),
                },
                _ => {
                    return Err(VrmImportError::reference(
                        what(),
                        "must be either a sphere or a capsule".into(),
                    ));
                }
            };
            self.colliders.push(ColliderData { node: collider_node, shape });
        }
        for (index, group) in schema.collider_groups.iter().flatten().enumerate() {
            let what = || format!("spring collider group {index}");
            let colliders = group
                .colliders
                .iter()
                .map(|&collider| checked_index(what, collider as i64, self.colliders.len(), "collider"))
                .collect::<Result<_, _>>()?;
            self.collider_groups
                .push(ColliderGroup { name: group.name.clone().unwrap_or_default(), colliders });
        }
        for (index, spring) in schema.springs.iter().flatten().enumerate() {
            let name = spring.name.clone().unwrap_or_default();
            let what = |detail: &str| format!("spring {index} ({name:?}) {detail}");
            let joints = spring
                .joints
                .iter()
                .enumerate()
                .map(|(k, joint)| {
                    let f = |value: Option<f64>, default: f32| value.map_or(default, |v| v as f32);
                    let params = JointParams {
                        hit_radius: f(joint.hit_radius, 0.0),
                        stiffness: f(joint.stiffness, 1.0),
                        gravity_power: f(joint.gravity_power, 0.0),
                        gravity_dir: joint.gravity_dir.map_or([0.0, -1.0, 0.0], |v| v.map(|c| c as f32)),
                        drag_force: f(joint.drag_force, 0.5),
                    };
                    Ok((
                        node(document, || what(&format!("joint {k}")), joint.node.value())?,
                        params.clamped(),
                    ))
                })
                .collect::<Result<Vec<_>, VrmImportError>>()?;
            if joints.is_empty() {
                return Err(VrmImportError::reference(
                    what("").trim_end().to_owned(),
                    "has no joints".into(),
                ));
            }
            let collider_groups = spring
                .collider_groups
                .iter()
                .flatten()
                .map(|&group| {
                    checked_index(|| what("collider group"), group, self.collider_groups.len(), "group")
                })
                .collect::<Result<_, _>>()?;
            let center =
                spring.center.map(|center| node(document, || what("center"), center.value())).transpose()?;
            self.springs.push(SpringData { name, joints, tip: SpringTip::Joint, collider_groups, center });
        }
        Ok(())
    }

    /// Every node's `VRMC_node_constraint`, in node order.
    fn add_constraints(&mut self, document: &Document) -> Result<(), VrmImportError> {
        for gltf_node in document.nodes() {
            let Some(value) = gltf_node.extension_value(CONSTRAINT) else { continue };
            let index = gltf_node.index();
            let what = || format!("node {index}'s constraint");
            let extension = NodeConstraintExtension::deserialize(value)
                .map_err(|source| VrmImportError::Extension { extension: CONSTRAINT, source })?;
            if extension.spec_version != "1.0" {
                return Err(VrmImportError::SpecVersion {
                    extension: CONSTRAINT,
                    found: extension.spec_version,
                });
            }
            let ConstraintJson { roll, aim, rotation } = extension.constraint;
            let (source, weight, kind) = match (roll, aim, rotation) {
                (Some(roll), None, None) => {
                    let axis = Axis::from_name(&roll.roll_axis).ok_or_else(|| {
                        VrmImportError::reference(
                            what(),
                            format!("has the unknown roll axis {:?}", roll.roll_axis),
                        )
                    })?;
                    (roll.source, roll.weight, ConstraintKind::Roll { axis })
                }
                (None, Some(aim), None) => {
                    let axis = AimAxis::from_name(&aim.aim_axis).ok_or_else(|| {
                        VrmImportError::reference(
                            what(),
                            format!("has the unknown aim axis {:?}", aim.aim_axis),
                        )
                    })?;
                    (aim.source, aim.weight, ConstraintKind::Aim { axis })
                }
                (None, None, Some(rotation)) => (rotation.source, rotation.weight, ConstraintKind::Rotation),
                _ => {
                    return Err(VrmImportError::reference(
                        what(),
                        "must be exactly one of roll, aim and rotation".into(),
                    ));
                }
            };
            let source = node(document, what, source)?;
            if source == index {
                return Err(VrmImportError::reference(what(), "names its own node as the source".into()));
            }
            let weight = weight.unwrap_or(1.0).clamp(0.0, 1.0);
            self.constraints.push(ConstraintData { node: index, source, weight, kind });
        }
        Ok(())
    }

    /// Every node the data names, which must be joints of the imported skeleton.
    pub fn nodes(&self) -> Vec<usize> {
        let mut nodes: Vec<usize> = self.humanoid.iter().map(|&(_, node)| node).collect();
        for spring in &self.springs {
            nodes.extend(spring.joints.iter().map(|&(node, _)| node));
            nodes.extend(spring.center);
        }
        nodes.extend(self.colliders.iter().map(|collider| collider.node));
        nodes.extend(self.constraints.iter().flat_map(|constraint| [constraint.node, constraint.source]));
        nodes.sort_unstable();
        nodes.dedup();
        nodes
    }
}

/// An index into a list of `count`, as a `u32`.
pub(super) fn checked_index(
    what: impl FnOnce() -> String,
    index: i64,
    count: usize,
    noun: &str,
) -> Result<u32, VrmImportError> {
    match u32::try_from(index) {
        Ok(checked) if (checked as usize) < count => Ok(checked),
        _ => Err(VrmImportError::reference(what(), format!("names {noun} {index}, but there are {count}"))),
    }
}

/// One VRM 1.0 expression.
fn expression_v1(
    document: &Document,
    name: &str,
    preset: Option<ExpressionPreset>,
    expression: &Expression,
) -> Result<ExpressionData, VrmImportError> {
    let what = || format!("expression {name:?}");
    let override_of = |value: &Option<vrm_spec::vrmc_vrm_1_0::ExpressionOverrideType>| {
        value
            .and_then(|value| ExpressionOverride::from_name(&spelling(&value)))
            .unwrap_or(ExpressionOverride::None)
    };
    let mut data = ExpressionData {
        name: name.to_owned(),
        preset,
        binary: expression.is_binary.unwrap_or(false),
        overrides: [
            override_of(&expression.override_blink),
            override_of(&expression.override_look_at),
            override_of(&expression.override_mouth),
        ],
        morph_targets: Vec::new(),
        material_colors: Vec::new(),
        texture_transforms: Vec::new(),
    };
    for bind in expression.morph_target_binds.iter().flatten() {
        let bind_node = node(document, what, bind.node.value())?;
        let mesh = document.nodes().nth(bind_node).and_then(|node| node.mesh()).ok_or_else(|| {
            VrmImportError::reference(
                what(),
                format!("binds a morph target of node {bind_node}, which has no mesh"),
            )
        })?;
        data.morph_targets.push(morph_bind(document, &what, mesh.index(), bind.index, bind.weight as f32)?);
    }
    for bind in expression.material_color_binds.iter().flatten() {
        let index = material(document, what, bind.material.value())?;
        let color = MaterialColor::from_name(&spelling(&bind.r#type)).ok_or_else(|| {
            VrmImportError::reference(
                what(),
                format!("binds the unknown material color {}", spelling(&bind.r#type)),
            )
        })?;
        let value = match bind.target_value[..] {
            [r, g, b] => [r as f32, g as f32, b as f32, 1.0],
            [r, g, b, a] => [r as f32, g as f32, b as f32, a as f32],
            _ => return Err(VrmImportError::reference(what(), "binds a color without 3 or 4 values".into())),
        };
        data.material_colors.push((index, color, value.map(|c| c.max(0.0))));
    }
    for bind in expression.texture_transform_binds.iter().flatten() {
        // vrm-spec 0.1.1 types this index as a node's; it is a material's.
        let index = material(document, what, bind.material.value())?;
        let scale = floats(bind.scale.as_ref(), [1.0; 2]);
        let offset = floats(bind.offset.as_ref(), [0.0; 2]);
        data.texture_transforms.push((index, scale, offset));
    }
    Ok(data)
}
