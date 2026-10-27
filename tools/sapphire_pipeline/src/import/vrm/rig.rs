//! Resolving a VRM's data ([`VrmData`], by glTF index) against the imported mesh into a
//! [`CharacterRig`], by joint, morph target and material slot.

use std::collections::BTreeMap;

use gltf::Document;
use sapphire_assets::rig::{
    CharacterRig, Collider, Constraint, Expression, HumanoidJoint, MaterialColorBind, MorphTargetWeight,
    Spring, SpringJoint, TextureTransformBind,
};

use super::VrmImportError;
use super::data::VrmData;
use crate::import::gltf::{SceneImport, morph_target_name, morph_target_names};

/// The rig `data` describes for the mesh `scene` imported from `document`.
pub(super) fn build(
    document: &Document,
    data: &VrmData,
    scene: &SceneImport,
) -> Result<CharacterRig, VrmImportError> {
    let skeleton = scene.mesh.skin.as_ref().map(|skin| skin.joints.as_slice()).unwrap_or_default();
    let joint = |what: &dyn Fn() -> String, node: usize| {
        scene.joint_of_node.get(&node).copied().ok_or_else(|| {
            VrmImportError::reference(what(), format!("names node {node}, which isn't in the imported scene"))
        })
    };

    let humanoid = data
        .humanoid
        .iter()
        .map(|&(bone, node)| {
            Ok(HumanoidJoint { bone, joint: joint(&|| format!("humanoid bone {}", bone.name()), node)? })
        })
        .collect::<Result<_, VrmImportError>>()?;

    let mut expressions = Vec::with_capacity(data.expressions.len());
    for expression in &data.expressions {
        let what = || format!("expression {:?}", expression.name);
        // Targets merge by name across meshes (the glTF importer's rule), so binds of a face's
        // and its lashes' `blink` are one target; it keeps the largest weight. A bind drives its
        // mesh's target on every instance, which is more than one target where instances rest at
        // different weights; a mesh outside the scene binds the target of its target's name.
        let mut weights: BTreeMap<u32, f32> = BTreeMap::new();
        for bind in &expression.morph_targets {
            let targets = match scene.targets_of_morph.get(&(bind.mesh, bind.target)) {
                Some(targets) => targets.clone(),
                None => {
                    let mesh = document.meshes().nth(bind.mesh).expect("checked when the data was read");
                    let name = morph_target_name(&mesh, &morph_target_names(&mesh), bind.target);
                    let target = scene.mesh.morph_targets.iter().position(|target| target.name == name);
                    vec![target.ok_or_else(|| {
                        VrmImportError::reference(
                            what(),
                            format!(
                                "binds morph target {name:?} of mesh {}, which isn't in the imported scene",
                                bind.mesh
                            ),
                        )
                    })? as u32]
                }
            };
            for target in targets {
                let weight = weights.entry(target).or_insert(0.0);
                *weight = weight.max(bind.weight);
            }
        }
        // Binds of materials no primitive uses change nothing that draws, so they are dropped.
        let slot = |material: usize| scene.slot_of_material.get(&Some(material)).copied();
        let [override_blink, override_look_at, override_mouth] = expression.overrides;
        expressions.push(Expression {
            name: expression.name.clone(),
            preset: expression.preset,
            binary: expression.binary,
            override_blink,
            override_look_at,
            override_mouth,
            morph_targets: weights
                .into_iter()
                .map(|(target, weight)| MorphTargetWeight { target, weight })
                .collect(),
            material_colors: expression
                .material_colors
                .iter()
                .filter_map(|&(material, color, value)| {
                    Some(MaterialColorBind { slot: slot(material)?, color, value })
                })
                .collect(),
            texture_transforms: expression
                .texture_transforms
                .iter()
                .filter_map(|&(material, scale, offset)| {
                    Some(TextureTransformBind { slot: slot(material)?, scale, offset })
                })
                .collect(),
        });
    }

    let mut springs = Vec::with_capacity(data.springs.len());
    for (index, spring) in data.springs.iter().enumerate() {
        let what = |detail: String| format!("spring {index} ({:?}) {detail}", spring.name);
        let mut joints: Vec<SpringJoint> = Vec::with_capacity(spring.joints.len());
        for (k, &(node, params)) in spring.joints.iter().enumerate() {
            let joint_index = joint(&|| what(format!("joint {k}")), node)?;
            // Each joint after the first descends from the one before (VRM 1.0).
            if let Some(previous) = joints.last()
                && !descends(skeleton, joint_index, previous.joint)
            {
                return Err(VrmImportError::reference(
                    what(format!("joint {k}")),
                    format!("names node {node}, which doesn't descend from the joint before it"),
                ));
            }
            joints.push(SpringJoint {
                joint: joint_index,
                hit_radius: params.hit_radius,
                stiffness: params.stiffness,
                gravity_power: params.gravity_power,
                gravity_dir: params.gravity_dir,
                drag_force: params.drag_force,
            });
        }
        let center = spring.center.map(|node| joint(&|| what("center".into()), node)).transpose()?;
        springs.push(Spring {
            name: spring.name.clone(),
            joints,
            tip: spring.tip,
            collider_groups: spring.collider_groups.clone(),
            center,
        });
    }

    let colliders = data
        .colliders
        .iter()
        .enumerate()
        .map(|(index, collider)| {
            Ok(Collider {
                joint: joint(&|| format!("collider {index}"), collider.node)?,
                shape: collider.shape,
            })
        })
        .collect::<Result<_, VrmImportError>>()?;
    let constraints = data
        .constraints
        .iter()
        .map(|constraint| {
            let what = || format!("node {}'s constraint", constraint.node);
            Ok(Constraint {
                joint: joint(&what, constraint.node)?,
                source: joint(&what, constraint.source)?,
                weight: constraint.weight,
                kind: constraint.kind,
            })
        })
        .collect::<Result<_, VrmImportError>>()?;

    let count = |len: usize| u32::try_from(len).expect("a cooked mesh's counts fit u32");
    Ok(CharacterRig {
        joint_count: count(skeleton.len()),
        morph_target_count: count(scene.mesh.morph_targets.len()),
        material_slot_count: count(scene.mesh.materials.len()),
        humanoid,
        expressions,
        look_at: data.look_at,
        springs,
        colliders,
        collider_groups: data.collider_groups.clone(),
        constraints,
    })
}

/// Whether `joint` is a descendant of `ancestor` in `skeleton`.
fn descends(skeleton: &[sapphire_assets::mesh::Joint], joint: u16, ancestor: u16) -> bool {
    let mut current = skeleton.get(usize::from(joint)).and_then(|joint| joint.parent);
    while let Some(parent) = current {
        if parent == ancestor {
            return true;
        }
        current = skeleton.get(usize::from(parent)).and_then(|joint| joint.parent);
    }
    false
}
