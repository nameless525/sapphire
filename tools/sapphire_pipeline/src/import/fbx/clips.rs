//! FBX animation stacks, sampled for the clip cook ([`crate::clip`]), to the same
//! [`SampledClip`] as glTF's animations.
//!
//! A sidecar's `clips: (…)` on an FBX mesh, or an `importer: FbxClips((…))` of its own, turns this
//! on. Then each animation stack can become a clip, named as a material slot is
//! (`slot_names`), and every node a stack's curves transform joins the
//! skeleton, as glTF's do. The scene loads as the [mesh importer](super)'s does, so joints mean
//! the same thing in a clip and in a mesh.
//!
//! **Space and units.** ufbx evaluates a node's local transform at a time, from its curves, its
//! default values, its pre- and post-rotations and pivots, and folds in what the load's conversion
//! did to it (the axes and units on top-level nodes), exactly as [its rest
//! transform](ufbx::Node::local_transform) is made. So a track holds the same numbers a joint's rest
//! does, and a clip is in meters whatever the file's unit.
//!
//! **Rest.** A joint's rest is its node's default transform. Blender's exporter writes that as the
//! pose of the frame it exported on, not the armature's rest (the skin keeps the true bind pose), so
//! a clip plays the same either way but a [retarget](crate::clip::retarget), which reads turns from
//! rest, needs the file exported on a frame in the rig's reference pose.
//!
//! **Time.** A clip runs over its stack's own range, `LocalStart` to `LocalStop`, which is what the
//! file says plays: keys outside it are cut. A stack with no such range runs from its earliest key
//! to its latest. The clip is a whole number of ticks long, the last rounded up, and every channel
//! is evaluated at every tick, through ufbx's curves (constant, linear, cubic) and layers. Morph
//! weights come from blend channels by name, the first of a name for a target's whole track, as a
//! glTF's first channel is.
//!
//! **Malformed files** are errors, with no panics: the stacks and samples are counted against
//! [`CLIP_BUDGET`] before any is evaluated (a few bytes can claim years of animation), a range must
//! be finite, and a joint that isn't finite at some tick (ASCII files can hold `nan`) names itself.

use std::collections::{HashMap, HashSet};

use sapphire_assets::clip::MAX_TICKS;
use sapphire_assets::skeleton::Joint;

use super::{FbxImportError, Skeleton, flatten, import_scene_with, joint_transform, load, name_of};
use crate::cache::Stage;
use crate::clip::{ClipSettings, SampledClip};
use crate::import::gltf::clips::{CLIP_BUDGET, ClipBudget, ClipImportError, lift_settings_root};
use crate::import::slot_names;
use crate::mesh::ImportedMesh;

/// This importer, as cache keys know it, carrying ufbx's version as the [mesh importer](super)'s
/// name does (a test keeps it in step with `Cargo.lock`). Bump the version whenever sampling changes
/// output for the same file.
///
/// 3: clamp the last sample to LocalStop and retain unnamed morph channels.
/// 2: root motion lifts in scalars, which can move a lifted track's last bit (M2 N1).
pub const IMPORTER: Stage = Stage { name: "fbx clips (ufbx 0.11.5)", version: 3 };

/// The transform properties whose curves move a node.
const TRANSFORM_PROPS: [&str; 3] = ["Lcl Translation", "Lcl Rotation", "Lcl Scaling"];

fn boxed(error: FbxImportError) -> ClipImportError {
    ClipImportError::Fbx(Box::new(error))
}

/// The nodes any of `scene`'s stacks move, by ufbx's typed ID, increasing.
pub(super) fn animated_nodes(scene: &ufbx::Scene) -> Vec<u32> {
    let mut nodes: Vec<u32> = scene
        .anim_layers
        .iter()
        .flat_map(|layer| layer.anim_props.iter())
        .filter(|prop| prop.element.type_ == ufbx::ElementType::Node)
        .filter(|prop| TRANSFORM_PROPS.contains(&&*prop.prop_name))
        .map(|prop| prop.element.typed_id)
        .collect();
    nodes.sort_unstable();
    nodes.dedup();
    nodes
}

fn names_of(scene: &ufbx::Scene) -> Vec<String> {
    let names: Vec<Option<&str>> = scene
        .anim_stacks
        .iter()
        .map(|stack| Some(&*stack.element.name).filter(|name| !name.is_empty()))
        .collect();
    slot_names(&names)
}

fn check_stacks(scene: &ufbx::Scene) -> Result<(), ClipImportError> {
    let found = scene.anim_stacks.len();
    if found > CLIP_BUDGET.animations {
        return Err(ClipImportError::TooManyAnimations { found, budget: CLIP_BUDGET.animations });
    }
    Ok(())
}

/// What planning a cook needs from an animation source: its clips' names, and the joints of its rig.
pub(crate) fn plan(bytes: &[u8]) -> Result<(Vec<String>, Vec<Joint>), ClipImportError> {
    let scene = load(bytes, false, true).map_err(boxed)?;
    check_stacks(&scene)?;
    let nodes = flatten(&scene);
    let skeleton = Skeleton::build(&nodes, &animated_nodes(&scene)).map_err(boxed)?;
    Ok((names_of(&scene), skeleton.map(|skeleton| skeleton.joints).unwrap_or_default()))
}

/// A mesh source's mesh when its animations cook: the importer's, with every node they move a joint
/// of its skeleton.
pub(crate) fn import_mesh(bytes: &[u8]) -> Result<ImportedMesh, FbxImportError> {
    let scene = load(bytes, false, true)?;
    Ok(import_scene_with(&scene, &animated_nodes(&scene))?.mesh)
}

/// An FBX's animation stacks, the rig they move, and what's needed to evaluate them.
pub(crate) struct FbxClips {
    scene: ufbx::SceneRoot,
    joints: Vec<Joint>,
    /// Each joint's node, by ufbx's typed ID.
    joint_nodes: Vec<u32>,
    /// Each morph target's first blend channel, by ufbx's typed ID, by target.
    channels: Vec<(u16, u32)>,
    morph_defaults: Vec<f32>,
}

impl FbxClips {
    /// Imports `bytes`. `with_mesh`: the scene's morph targets are clips' to drive, and its mesh
    /// is read to find them; an animation source reads none.
    pub fn import(bytes: &[u8], with_mesh: bool) -> Result<Self, ClipImportError> {
        let scene = load(bytes, false, true).map_err(boxed)?;
        check_stacks(&scene)?;
        let animated = animated_nodes(&scene);
        let (joints, joint_nodes) = {
            let nodes = flatten(&scene);
            let skeleton = Skeleton::build(&nodes, &animated).map_err(boxed)?;
            match skeleton {
                Some(skeleton) => {
                    let mut by_joint: Vec<(u16, u32)> =
                        skeleton.joint_of.iter().map(|(&node, &joint)| (joint, node)).collect();
                    by_joint.sort_unstable();
                    (skeleton.joints, by_joint.into_iter().map(|(_, node)| node).collect())
                }
                None => (Vec::new(), Vec::new()),
            }
        };
        let mut channels = Vec::new();
        let mut morph_defaults = Vec::new();
        if with_mesh {
            let mesh = import_scene_with(&scene, &animated).map_err(boxed)?.mesh;
            morph_defaults = mesh.morph_targets.iter().map(|target| target.default_weight).collect();
            let mut by_name: HashMap<String, u32> = scene
                .blend_channels
                .iter()
                .rev()
                .map(|c| (name_of(&c.element), c.element.typed_id))
                .collect();
            // An unnamed channel's generated name is local to its mesh, just as it is in
            // the geometry importer; its element ID is not the imported target's name.
            for mesh in scene.meshes.iter() {
                let mesh_name = name_of(&mesh.element);
                for (index, channel) in
                    mesh.blend_deformers.iter().flat_map(|deformer| deformer.channels.iter()).enumerate()
                {
                    if channel.element.name.is_empty() && channel.target_shape.is_some() {
                        by_name
                            .entry(format!("{mesh_name}/{index}"))
                            .and_modify(|seen| *seen = (*seen).min(channel.element.typed_id))
                            .or_insert(channel.element.typed_id);
                    }
                }
            }
            for (target, morph) in mesh.morph_targets.iter().enumerate() {
                let target =
                    u16::try_from(target).map_err(|_| ClipImportError::TooManyTargets(target as u32))?;
                if let Some(&channel) = by_name.get(&morph.name) {
                    channels.push((target, channel));
                }
            }
        }
        Ok(Self { scene, joints, joint_nodes, channels, morph_defaults })
    }

    /// The rig its clips index.
    pub fn joints(&self) -> &[Joint] {
        &self.joints
    }

    /// Each morph target's default weight, for a source with a mesh.
    pub fn morph_defaults(&self) -> &[f32] {
        &self.morph_defaults
    }

    /// Samples stack `index` under `settings`. The result is ready for [`crate::clip::cook`], its
    /// root motion lifted.
    pub fn sample(&self, index: usize, settings: &ClipSettings) -> Result<SampledClip, ClipImportError> {
        self.sample_within(index, settings, CLIP_BUDGET)
    }

    /// [`Self::sample`], within `budget`.
    pub(super) fn sample_within(
        &self,
        index: usize,
        settings: &ClipSettings,
        budget: ClipBudget,
    ) -> Result<SampledClip, ClipImportError> {
        let name = names_of(&self.scene).swap_remove(index);
        let stack = &self.scene.anim_stacks[index];
        let (begin, end) = time_range(stack);
        if !begin.is_finite() || !end.is_finite() || end < begin {
            return Err(ClipImportError::TimeRange { animation: name, begin, end });
        }
        let rate = f64::from(settings.rate);
        // A hair over a whole tick is the source's rounding, not a tick more.
        let ticks = ((end - begin) * rate - 1e-3).ceil().max(0.0);
        if ticks > f64::from(MAX_TICKS) {
            return Err(ClipImportError::TooLong {
                animation: name,
                seconds: (end - begin) as f32,
                ticks,
                rate: settings.rate,
            });
        }
        let ticks = ticks as u16;
        let count = usize::from(ticks) + 1;

        // Only what this stack's curves touch moves: the rest would sample to rest.
        let moved: HashSet<u32> = stack
            .layers
            .iter()
            .flat_map(|layer| layer.anim_props.iter())
            .filter(|prop| prop.element.type_ == ufbx::ElementType::Node)
            .filter(|prop| TRANSFORM_PROPS.contains(&&*prop.prop_name))
            .map(|prop| prop.element.typed_id)
            .collect();
        let moving: Vec<(u16, u32)> = self
            .joint_nodes
            .iter()
            .enumerate()
            .filter(|(_, node)| moved.contains(node))
            .map(|(joint, node)| (joint as u16, *node))
            .collect();
        let per_tick = moving.len().saturating_mul(10).saturating_add(self.channels.len());
        let samples = count.saturating_mul(per_tick);
        if samples > budget.samples {
            return Err(ClipImportError::TooManySamples {
                animation: name,
                found: samples,
                budget: budget.samples,
            });
        }

        let time_of = |tick: usize| (begin + tick as f64 / rate).min(end);
        let anim: &ufbx::Anim = &stack.anim;
        let mut clip = SampledClip {
            name: name.clone(),
            rate: settings.rate,
            ticks,
            rotations: Vec::with_capacity(moving.len()),
            translations: Vec::with_capacity(moving.len()),
            scales: Vec::with_capacity(moving.len()),
            weights: Vec::with_capacity(self.channels.len()),
            root: None,
        };
        for (joint, node) in moving {
            let node = &self.scene.nodes[node as usize];
            let mut rotations = Vec::with_capacity(count);
            let mut translations = Vec::with_capacity(count);
            let mut scales = Vec::with_capacity(count);
            for tick in 0..count {
                let transform = joint_transform(&ufbx::evaluate_transform(anim, node, time_of(tick)));
                let finite = transform
                    .translation
                    .iter()
                    .chain(&transform.rotation)
                    .chain(&transform.scale)
                    .all(|value| value.is_finite());
                if !finite {
                    return Err(ClipImportError::NonFiniteSample {
                        animation: name,
                        joint: self.joints[usize::from(joint)].name.clone(),
                        tick,
                    });
                }
                rotations.push(glam::Quat::from_array(transform.rotation));
                translations.push(glam::Vec3::from(transform.translation));
                scales.push(glam::Vec3::from(transform.scale));
            }
            clip.rotations.push((joint, rotations));
            clip.translations.push((joint, translations));
            clip.scales.push((joint, scales));
        }
        for &(target, channel) in &self.channels {
            let channel = &self.scene.blend_channels[channel as usize];
            let mut weights = Vec::with_capacity(count);
            for tick in 0..count {
                let weight = ufbx::evaluate_blend_weight(anim, channel, time_of(tick)) as f32;
                if !weight.is_finite() {
                    return Err(ClipImportError::NonFiniteSample {
                        animation: name,
                        joint: name_of(&channel.element),
                        tick,
                    });
                }
                weights.push(weight);
            }
            clip.weights.push((target, weights));
        }
        lift_settings_root(&mut clip, &self.joints, settings)?;
        Ok(clip)
    }
}

#[cfg(test)]
mod tests;

/// When a stack starts and ends: its own range, or with none its keys'.
fn time_range(stack: &ufbx::AnimStack) -> (f64, f64) {
    if stack.time_end > stack.time_begin {
        return (stack.time_begin, stack.time_end);
    }
    let mut range: Option<(f64, f64)> = None;
    for layer in stack.layers.iter() {
        for value in layer.anim_values.iter() {
            for curve in value.curves.iter().flatten() {
                if curve.keyframes.is_empty() {
                    continue;
                }
                let (from, to) = range.unwrap_or((curve.min_time, curve.max_time));
                range = Some((from.min(curve.min_time), to.max(curve.max_time)));
            }
        }
    }
    range.unwrap_or((stack.time_begin, stack.time_begin))
}
