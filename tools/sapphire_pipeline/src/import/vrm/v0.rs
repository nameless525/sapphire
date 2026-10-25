//! VRM 0.x → VRM 1.0 semantics: the `VRM` extension's humanoid, blend shape groups, first
//! person look-at and secondary animation, as UniVRM's and three-vrm's migrations (both MIT)
//! read them. Materials migrate in [`super::mtoon`]; the mesh turns to face +Z in
//! [`super::turn_to_face_forward`].
//!
//! # Coordinates
//!
//! UniVRM 0.x exported Unity's left-handed scenes by negating Z, so 0.x models face −Z, where
//! VRM 1.0 and Sapphire face +Z: the import turns the whole model half a turn about Y
//! (`(x, y, z) → (−x, y, −z)`), node by node, so every node-local space turns with it. The
//! secondary animation's vectors (collider offsets, gravity) and the first person bone offset
//! were written in Unity's coordinates as they were, so they get Z negated first, then the
//! turn: `(x, y, z) → (−x, y, z)` overall.
//!
//! # What becomes what
//!
//! - **Humanoid:** bone names carry over, except the thumbs: 0.x's proximal, intermediate and
//!   distal are 1.0's metacarpal, proximal and distal.
//! - **Blend shape groups → expressions:** presets map by name (`joy` → `happy`, `sorrow` →
//!   `sad`, `fun` → `relaxed`, `a`/`i`/`u`/`e`/`o` → `aa`/`ih`/`ou`/`ee`/`oh`, `blink_l` →
//!   `blinkLeft`, `lookup` → `lookUp`, …); `unknown` groups are custom expressions by name.
//!   Binds name a mesh and a morph target with a 0–100 weight (÷100). Material values set
//!   `_Color`, `_ShadeColor`, `_EmissionColor`, `_RimColor` and `_OutlineColor` (sRGB,
//!   decoded) and `_MainTex_ST` (a UV transform, flipped from Unity's bottom-left origin to
//!   glTF's top-left); other properties have no 1.0 counterpart and are dropped. 0.x has no
//!   overrides, so they are `none`.
//! - **Look-at:** `firstPersonBoneOffset`, `lookAtTypeName` and the degree maps' `xRange`
//!   (input) and `yRange` (output); the maps' curves are dropped, as 1.0 has none.
//! - **Secondary animation → springs:** each bone group lists root nodes, and UniVRM 0.x swings
//!   every node of each root's subtree, toward its first child, or toward a virtual tail 7 cm
//!   past it for a leaf. That becomes one spring per branch: from each root, follow first
//!   children to a leaf; every other child starts a spring of its own. Springs end in
//!   [`SpringTip::Extended`] tails of 7 cm. The group's stiffness, gravity, drag and hit
//!   radius apply to every joint, and its comment names the springs. Collider groups' sphere
//!   colliders become colliders on the group's node, one 1.0 collider group per 0.x group.

use std::collections::{HashSet, VecDeque};

use gltf::Document;
use sapphire_assets::rig::{
    ColliderGroup, ColliderShape, ExpressionOverride, ExpressionPreset, HumanBone, LookAt, LookAtKind,
    LookAtRange, MaterialColor, SpringTip,
};
use vrm_spec::vrm_0_0::{OptionalVector3, PresetName, VRM0Schema, VRMFirstPersonDegreeMap};

use super::VrmImportError;
use super::data::{
    ColliderData, ExpressionData, JointParams, SpringData, VrmData, checked_index, material, morph_bind,
    node, spelling,
};
use crate::color::srgb_to_linear;

/// The virtual tail UniVRM 0.x gives a spring's leaves, in meters.
pub(super) const LEAF_TAIL: f32 = 0.07;

/// A 0.x vector written in Unity's coordinates, in the turned model's: Z negated (Unity to
/// glTF), then half a turn about Y.
fn from_unity([x, y, z]: [f32; 3]) -> [f32; 3] {
    [-x, y, z]
}

fn vector(value: Option<&OptionalVector3>, default: [f32; 3]) -> [f32; 3] {
    value.map_or(default, |v| {
        let pick = |c: Option<f64>, d: f32| c.map_or(d, |c| c as f32);
        [pick(v.x, default[0]), pick(v.y, default[1]), pick(v.z, default[2])]
    })
}

/// The 1.0 preset a 0.x preset name became, or `None` for `unknown` (a custom expression).
fn preset(name: PresetName) -> Option<ExpressionPreset> {
    use ExpressionPreset as P;
    Some(match name {
        PresetName::Joy => P::Happy,
        PresetName::Angry => P::Angry,
        PresetName::Sorrow => P::Sad,
        PresetName::Fun => P::Relaxed,
        PresetName::A => P::Aa,
        PresetName::I => P::Ih,
        PresetName::U => P::Ou,
        PresetName::E => P::Ee,
        PresetName::O => P::Oh,
        PresetName::Blink => P::Blink,
        PresetName::BlinkL => P::BlinkLeft,
        PresetName::BlinkR => P::BlinkRight,
        PresetName::Lookup => P::LookUp,
        PresetName::Lookdown => P::LookDown,
        PresetName::Lookleft => P::LookLeft,
        PresetName::Lookright => P::LookRight,
        PresetName::Neutral => P::Neutral,
        PresetName::Unknown => return None,
    })
}

/// The 1.0 bone a 0.x bone name became: the thumbs gained a metacarpal.
fn human_bone(name: &str) -> Option<HumanBone> {
    let renamed = match name {
        "leftThumbProximal" => "leftThumbMetacarpal",
        "leftThumbIntermediate" => "leftThumbProximal",
        "rightThumbProximal" => "rightThumbMetacarpal",
        "rightThumbIntermediate" => "rightThumbProximal",
        other => other,
    };
    HumanBone::from_name(renamed)
}

impl VrmData {
    /// The data of a VRM 0.x file's `VRM` extension, in 1.0 semantics (see the
    /// [module docs](self)).
    pub fn from_v0(document: &Document, vrm: &VRM0Schema) -> Result<Self, VrmImportError> {
        let mut data = Self::default();
        for (index, bone) in
            vrm.humanoid.iter().flat_map(|humanoid| humanoid.human_bones.iter().flatten()).enumerate()
        {
            let name = bone.bone.as_ref().map(spelling).unwrap_or_default();
            let what = || format!("humanoid bone {index} ({name})");
            let human = human_bone(&name)
                .ok_or_else(|| VrmImportError::reference(what(), "is not a VRM 0.x bone".into()))?;
            let bone_node =
                bone.node.ok_or_else(|| VrmImportError::reference(what(), "names no node".into()))?;
            if data.humanoid.iter().any(|&(known, _)| known == human) {
                return Err(VrmImportError::reference(what(), "appears twice".into()));
            }
            data.humanoid.push((human, node(document, what, bone_node.value())?));
        }
        data.humanoid.sort_by_key(|&(bone, _)| bone);

        data.add_expressions(document, vrm)?;

        if let Some(first_person) = &vrm.first_person {
            let range = |map: &Option<VRMFirstPersonDegreeMap>, output: f32| LookAtRange {
                input_max_degrees: map.as_ref().and_then(|m| m.x_range).map_or(90.0, |v| v as f32).max(0.0),
                output_scale: map.as_ref().and_then(|m| m.y_range).map_or(output, |v| v as f32),
            };
            let kind = match first_person.look_at_type_name.map(|kind| spelling(&kind)).as_deref() {
                Some("BlendShape") => LookAtKind::Expression,
                _ => LookAtKind::Bone,
            };
            // UniVRM 0.x's defaults: the eyes 6 cm above the head joint; ranges of 10 degrees,
            // or full weight for blend shapes.
            let output = if kind == LookAtKind::Bone { 10.0 } else { 1.0 };
            data.look_at = Some(LookAt {
                offset_from_head: from_unity(vector(
                    first_person.first_person_bone_offset.as_ref(),
                    [0.0, 0.06, 0.0],
                )),
                kind,
                horizontal_inner: range(&first_person.look_at_horizontal_inner, output),
                horizontal_outer: range(&first_person.look_at_horizontal_outer, output),
                vertical_down: range(&first_person.look_at_vertical_down, output),
                vertical_up: range(&first_person.look_at_vertical_up, output),
            });
        }

        data.add_secondary_animation(document, vrm)?;
        Ok(data)
    }

    fn add_expressions(&mut self, document: &Document, vrm: &VRM0Schema) -> Result<(), VrmImportError> {
        let groups =
            vrm.blend_shape_master.iter().flat_map(|master| master.blend_shape_groups.iter().flatten());
        let (mut presets, mut custom) = (Vec::new(), Vec::new());
        for (index, group) in groups.enumerate() {
            let group_name = group.name.clone().unwrap_or_default();
            let what = || format!("blend shape group {index} ({group_name:?})");
            let preset = group.preset_name.and_then(preset);
            let name = match preset {
                Some(preset) => preset.name().to_owned(),
                None if group_name.is_empty() => {
                    return Err(VrmImportError::reference(
                        what(),
                        "is a custom expression without a name".into(),
                    ));
                }
                None => group_name.clone(),
            };
            let mut expression = ExpressionData {
                name,
                preset,
                binary: group.is_binary.unwrap_or(false),
                overrides: [ExpressionOverride::None; 3],
                morph_targets: Vec::new(),
                material_colors: Vec::new(),
                texture_transforms: Vec::new(),
            };
            for bind in group.binds.iter().flatten() {
                let mesh =
                    bind.mesh.ok_or_else(|| VrmImportError::reference(what(), "binds no mesh".into()))?;
                let target = bind
                    .index
                    .and_then(|index| usize::try_from(index).ok())
                    .ok_or_else(|| VrmImportError::reference(what(), "binds no morph target".into()))?;
                let weight = bind.weight.map_or(100.0, |weight| weight as f32) / 100.0;
                expression.morph_targets.push(morph_bind(document, &what, mesh.value(), target, weight)?);
            }
            for value in group.material_values.iter().flatten() {
                let material_name = value.material_name.as_deref().unwrap_or_default();
                let found =
                    document.materials().find(|m| m.name() == Some(material_name)).ok_or_else(|| {
                        VrmImportError::reference(
                            what(),
                            format!("sets material {material_name:?}, which doesn't exist"),
                        )
                    })?;
                let index = material(document, what, found.index().unwrap_or_default())?;
                let target: Vec<f32> = value.target_value.iter().flatten().map(|&v| v as f32).collect();
                let color = |color: MaterialColor| -> Option<(usize, MaterialColor, [f32; 4])> {
                    let [r, g, b, a] = <[f32; 4]>::try_from(target.get(..4)?).ok()?;
                    Some((index, color, [srgb_to_linear(r), srgb_to_linear(g), srgb_to_linear(b), a]))
                };
                match value.property_name.as_deref() {
                    Some("_Color") => expression.material_colors.extend(color(MaterialColor::Base)),
                    Some("_ShadeColor") => expression.material_colors.extend(color(MaterialColor::Shade)),
                    Some("_EmissionColor") => {
                        expression.material_colors.extend(color(MaterialColor::Emission))
                    }
                    Some("_RimColor") => expression.material_colors.extend(color(MaterialColor::Rim)),
                    Some("_OutlineColor") => expression.material_colors.extend(color(MaterialColor::Outline)),
                    Some("_MainTex_ST") => {
                        if let [sx, sy, ox, oy] = target[..] {
                            // Unity's V runs up from the bottom; glTF's down from the top.
                            expression.texture_transforms.push((index, [sx, sy], [ox, 1.0 - sy - oy]));
                        }
                    }
                    _ => {}
                }
            }
            if preset.is_some() { presets.push(expression) } else { custom.push(expression) }
        }
        presets.sort_by_key(|expression| expression.preset);
        custom.sort_by(|a, b| a.name.cmp(&b.name));
        for list in [&presets, &custom] {
            for pair in list.windows(2) {
                if pair[0].name == pair[1].name {
                    return Err(VrmImportError::reference(
                        format!("expression {:?}", pair[0].name),
                        "is defined twice".into(),
                    ));
                }
            }
        }
        if let Some(clash) =
            custom.iter().find(|expression| ExpressionPreset::from_name(&expression.name).is_some())
        {
            return Err(VrmImportError::reference(
                format!("custom expression {:?}", clash.name),
                "has a VRM 1.0 preset's name".into(),
            ));
        }
        self.expressions = presets.into_iter().chain(custom).collect();
        Ok(())
    }

    fn add_secondary_animation(
        &mut self,
        document: &Document,
        vrm: &VRM0Schema,
    ) -> Result<(), VrmImportError> {
        let Some(secondary) = &vrm.secondary_animation else { return Ok(()) };
        for (index, group) in secondary.collider_groups.iter().flatten().enumerate() {
            let what = || format!("secondary animation collider group {index}");
            let group_node =
                group.node.ok_or_else(|| VrmImportError::reference(what(), "names no node".into()))?;
            let group_node = node(document, what, group_node.value())?;
            let first = self.colliders.len();
            for collider in group.colliders.iter().flatten() {
                let offset = from_unity(vector(collider.offset.as_ref(), [0.0; 3]));
                let radius = collider.radius.map_or(0.0, |r| r as f32).max(0.0);
                self.colliders
                    .push(ColliderData { node: group_node, shape: ColliderShape::Sphere { offset, radius } });
            }
            let colliders = (first..self.colliders.len()).map(|collider| collider as u32).collect();
            self.collider_groups.push(ColliderGroup { name: String::new(), colliders });
        }
        for (index, group) in secondary.bone_groups.iter().flatten().enumerate() {
            let name = group.comment.clone().unwrap_or_default();
            let what = |detail: &str| format!("secondary animation bone group {index} ({name:?}) {detail}");
            // UniVRM 0.x's component defaults.
            let f = |value: Option<f64>, default: f32| value.map_or(default, |v| v as f32);
            let params = JointParams {
                hit_radius: f(group.hit_radius, 0.02),
                stiffness: f(group.stiffiness, 1.0),
                gravity_power: f(group.gravity_power, 0.0),
                gravity_dir: from_unity(vector(group.gravity_dir.as_ref(), [0.0, -1.0, 0.0])),
                drag_force: f(group.drag_force, 0.4),
            }
            .clamped();
            let collider_groups = group
                .collider_groups
                .iter()
                .flatten()
                .map(|&group| {
                    checked_index(|| what("collider group"), group, self.collider_groups.len(), "group")
                })
                .collect::<Result<Vec<_>, _>>()?;
            let center =
                group.center.map(|center| node(document, || what("center"), center.value())).transpose()?;
            for root in group.bones.iter().flatten() {
                let root = node(document, || what("bone"), root.value())?;
                for chain in branches(document, root) {
                    self.springs.push(SpringData {
                        name: name.clone(),
                        joints: chain.into_iter().map(|joint| (joint, params)).collect(),
                        tip: SpringTip::Extended { length: LEAF_TAIL },
                        collider_groups: collider_groups.clone(),
                        center,
                    });
                }
            }
        }
        Ok(())
    }
}

/// The chains UniVRM 0.x swings below `root`: from each start, first children down to a leaf;
/// every other child starts a chain of its own, in breadth-first order of their starts.
fn branches(document: &Document, root: usize) -> Vec<Vec<usize>> {
    let children = |node: usize| -> Vec<usize> {
        document
            .nodes()
            .nth(node)
            .map(|node| node.children().map(|child| child.index()).collect())
            .unwrap_or_default()
    };
    let mut chains = Vec::new();
    let mut starts = VecDeque::from([root]);
    // glTF node hierarchies are trees, which the scene import checks; this only keeps a
    // malformed file from looping here first.
    let mut seen = HashSet::from([root]);
    while let Some(start) = starts.pop_front() {
        let mut chain = vec![start];
        let mut current = start;
        loop {
            let next: Vec<usize> =
                children(current).into_iter().filter(|child| seen.insert(*child)).collect();
            let Some((&first, rest)) = next.split_first() else { break };
            starts.extend(rest);
            chain.push(first);
            current = first;
        }
        chains.push(chain);
    }
    chains
}
