//! VRM animations (`.vrma`, `VRMC_vrm_animation` 1.0, M2 N1): a glTF whose animations move nodes the
//! extension names as humanoid bones, cooked to clips by the same path as any glTF's
//! ([`gltf::clips`](super::gltf::clips)), read through pixiv's `vrm-spec`.
//!
//! The file's map is its own, so its sidecar (`importer: Vrma((…))`) names none, and each bone it
//! names joins the skeleton whether its animations move it or not. A retarget (`retarget: [(target:
//! "<character>", names: […])]`, [`retarget`](crate::clip::retarget)) carries them onto a character.
//! The specification rests its nodes in a T-pose, as VRM 1.0's does, and that's what the retarget's
//! rest-relative rotations need.
//!
//! **Dropped:** the extension's `expressions` (nodes whose X translation is a preset's weight) and
//! `lookAt`. Clips drive morph targets, so an expression track would need the target's expressions
//! and mesh at cook time, for clips nothing licensed fills: no VRMA can be committed here (pixiv's
//! pack bars extraction), so its fixtures are built in code.

use std::collections::{BTreeMap, HashMap};

use gltf::Document;
use sapphire_assets::rig::HumanBone;
use serde::Deserialize;
use vrm_spec::vrmc_vrm_animation_1_0::{VRMC_VRM_ANIMATION, VRMCVrmAnimationSchema};

use super::gltf::clips::ClipImportError;
use crate::cache::Stage;
use crate::clip::retarget::HumanMap;

/// This importer, as cache keys know it. Bump the version whenever output changes for the same
/// file. It samples through the glTF clip importer, whose version is in its low 16 bits.
pub const IMPORTER: Stage = Stage { name: "vrma", version: (1 << 16) | super::gltf::clips::IMPORTER.version };

/// The extension of the document, checked.
fn schema(document: &Document) -> Result<VRMCVrmAnimationSchema, ClipImportError> {
    let value = document.extension_value(VRMC_VRM_ANIMATION).ok_or(ClipImportError::NotVrma)?;
    let schema: VRMCVrmAnimationSchema =
        Deserialize::deserialize(value).map_err(ClipImportError::VrmaExtension)?;
    if schema.spec_version != "1.0" {
        return Err(ClipImportError::VrmaVersion(schema.spec_version));
    }
    Ok(schema)
}

/// The humanoid bones of `document`, with their nodes, checked against it, by bone.
fn humanoid(document: &Document) -> Result<BTreeMap<HumanBone, usize>, ClipImportError> {
    let schema = schema(document)?;
    let mut bones = BTreeMap::new();
    let Some(humanoid) = &schema.humanoid else { return Err(ClipImportError::VrmaNoHips) };
    for (name, bone) in &humanoid.human_bones.0 {
        let spelled = serde_json::to_value(name)
            .ok()
            .and_then(|value| value.as_str().map(str::to_owned))
            .unwrap_or_default();
        let fault = |problem: String| ClipImportError::VrmaBone { bone: spelled.clone(), problem };
        let node = bone
            .as_ref()
            .and_then(|bone| bone.node)
            .ok_or_else(|| fault("names no node".to_owned()))?
            .value();
        if node >= document.nodes().len() {
            return Err(fault(format!("names node {node}, which isn't there")));
        }
        let bone = HumanBone::from_name(&spelled).ok_or_else(|| fault("is not a VRM 1.0 bone".to_owned()))?;
        bones.insert(bone, node);
    }
    if !bones.contains_key(&HumanBone::Hips) {
        return Err(ClipImportError::VrmaNoHips);
    }
    Ok(bones)
}

/// The nodes `document` names as humanoid bones: they all join its skeleton.
pub(crate) fn humanoid_nodes(document: &Document) -> Result<Vec<usize>, ClipImportError> {
    Ok(humanoid(document)?.into_values().collect())
}

/// The file's map, by the joints the importer made of its nodes. A node outside the scene has
/// none: it can't be a bone of the skeleton.
pub(crate) fn humanoid_map(
    document: &Document,
    joint_of_node: &HashMap<usize, u16>,
) -> Result<HumanMap, ClipImportError> {
    let mut joints = BTreeMap::new();
    for (bone, node) in humanoid(document)? {
        let joint = joint_of_node.get(&node).copied().ok_or_else(|| ClipImportError::VrmaBone {
            bone: bone.name().to_owned(),
            problem: format!("names node {node}, which isn't in the scene"),
        })?;
        joints.insert(bone, joint);
    }
    Ok(HumanMap::from_joints(joints))
}

#[cfg(test)]
pub(crate) mod tests;
