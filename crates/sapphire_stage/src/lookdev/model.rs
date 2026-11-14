//! A model on the look-dev stage (`--model`, M3 C2): any skinned mesh whose skeleton names its head
//! joint `head`, as KayKit's do and every generated figure's does, standing where the enemy shot's
//! warrior stands, at scale 1, in its rest pose or in a clip's pose at a tick. It's for looking at a
//! new enemy before anything plays it: no graph, brain or table here names one, so a model is asked
//! for by its mesh's ID alone, and its clip by the clip's.
//!
//! The model's entity holds its assets ([`ModelAssets`]), so they stay loaded while it stands. A clip's
//! root motion moves it from its place, as the contact sheet shows travel.
//!
//! A model cooked from a VRM file has a rig beside its mesh (M5 B3's hero): `+<preset>` shows one of its
//! expression presets at full weight through a face layer, and its spring chains settle under gravity
//! for [`SETTLE_TICKS`] before the pose is drawn, so hair and a skirt hang as they would at rest.

use std::str::FromStr;

use sapphire_anim::{
    CharacterPose, Clip, FaceLayer, FaceOrder, Skeleton, SpringBones, SpringFrame, root_motion,
};
use sapphire_assets::mesh::Mesh;
use sapphire_assets::rig::{CharacterRig, ExpressionPreset};
use sapphire_assets::{AssetId, Handle};
use sapphire_ecs::prelude::*;
use sapphire_math::{Affine3A, Mat4, Quat, Vec3};
use sapphire_platform::AppInitError;
use sapphire_render::{FaceExpression, MeshHandle, Renderer};
use sapphire_rhi::GpuContext;
use sapphire_world::light_rig::{CharacterLightRig, LightRigId};

use crate::content::Content;
use crate::extract::{RenderMesh, RenderPose, RenderTransform};
use crate::scene::{Figure, Scene};

/// Where a model stands: where the enemy shot's warrior does, facing the character
/// ([`super::Shot::Enemy`]).
const STANDS_AT: Vec3 = Vec3::new(-2.0, 0.0, 0.5);
const FACES_DEGREES: f32 = 52.0;
/// Its head joint's name.
const HEAD: &str = "head";
/// From the head joint to the middle of the skull, and its body capsule's radius, as shares of its
/// height: a model says neither, and these fit a figure in proportion.
const HEAD_CENTER_SHARE: f32 = 0.06;
const BODY_RADIUS_SHARE: f32 = 0.11;

/// How long a model's springs settle before it's drawn, in ticks (60 a second).
pub const SETTLE_TICKS: u32 = 240;

/// The rig a VRM's cook makes beside its mesh: the mesh's sub-asset.
const RIG_SUB_ASSET: &str = "rig";

/// A model to stand on the stage: `<mesh>[:<clip>[@<tick>]][+<preset>]`, by asset IDs and a VRM 1.0
/// expression preset's name.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Model {
    pub mesh: AssetId,
    /// The clip whose pose it holds, at `tick`: none holds its rest pose.
    pub clip: Option<AssetId>,
    pub tick: u32,
    /// The expression it shows, through its rig: one is needed for this.
    pub face: Option<ExpressionPreset>,
}

impl FromStr for Model {
    type Err = String;

    fn from_str(text: &str) -> Result<Self, String> {
        let id = |text: &str| text.parse::<AssetId>().map_err(|_| format!("{text:?} isn't an asset ID"));
        let (text, face) = match text.split_once('+') {
            Some((text, name)) => {
                let preset = ExpressionPreset::from_name(name)
                    .ok_or_else(|| format!("{name:?} isn't one of VRM 1.0's expression presets"))?;
                (text, Some(preset))
            }
            None => (text, None),
        };
        let (mesh, clip) = match text.split_once(':') {
            Some((mesh, clip)) => (mesh, Some(clip)),
            None => (text, None),
        };
        let (clip, tick) = match clip.map(|clip| clip.split_once('@').unwrap_or((clip, "0"))) {
            Some((clip, tick)) => {
                let tick = tick.parse().map_err(|_| format!("{tick:?} isn't a tick"))?;
                (Some(id(clip)?), tick)
            }
            None => (None, 0),
        };
        Ok(Self { mesh: id(mesh)?, clip, tick, face })
    }
}

/// What keeps a model's assets loaded: its entity holds them.
#[derive(Component, Debug)]
pub struct ModelAssets {
    _mesh: Handle<Mesh>,
    _skeleton: Handle<Skeleton>,
    _clip: Option<Handle<Clip>>,
    _rig: Option<Handle<CharacterRig>>,
}

/// A model loaded and posed, ready to [`spawn`].
#[derive(Debug)]
pub struct HeldModel {
    pub mesh: MeshHandle,
    pub pose: RenderPose,
    pub figure: Figure,
    /// Its model space to the world: its place, and its clip's root motion at the tick.
    pub place: Mat4,
    assets: ModelAssets,
}

/// Loads `model`'s mesh, the skeleton its skin names and its clip, and poses it.
pub fn hold(
    content: &mut Content,
    renderer: &mut Renderer,
    gpu: &GpuContext,
    model: Model,
) -> Result<HeldModel, AppInitError> {
    let failed = |error: String| AppInitError::msg(format!("the model {} didn't load: {error}", model.mesh));
    content.keep_mesh_copy(model.mesh);
    let mesh: Handle<Mesh> = content.hold(model.mesh, renderer, gpu)?;
    let skeleton_id = content.mesh_copy(model.mesh).and_then(|mesh| mesh.skin.as_ref()?.skeleton);
    let skeleton_id = skeleton_id.ok_or_else(|| failed("its mesh has no skin with a skeleton".to_owned()))?;
    let skeleton: Handle<Skeleton> = content.hold(skeleton_id, renderer, gpu)?;
    let clip: Option<Handle<Clip>> = model.clip.map(|id| content.hold(id, renderer, gpu)).transpose()?;
    // A rig is held only for an expression, and before the reads below borrow `content`.
    let rig: Option<Handle<CharacterRig>> =
        model.face.map(|_| content.hold(model.mesh.sub_asset(RIG_SUB_ASSET), renderer, gpu)).transpose()?;
    let copy = content.mesh_copy(model.mesh).expect("a ready mesh's copy was kept here");
    let skin = copy.skin.as_ref().expect("its skin was read above");
    let skeleton_view =
        content.read(&skeleton).ok_or_else(|| failed("its skeleton isn't resident".to_owned()))?;
    let head = skeleton_view.find(HEAD).ok_or_else(|| failed(format!("its skeleton has no {HEAD:?}")))?;
    let mut pose = CharacterPose::try_new(skeleton_view, &skin.binds, &[], Vec::new())
        .map_err(|error| failed(error.to_string()))?;
    let mut root = Mat4::IDENTITY;
    if let Some(clip) = &clip {
        let clip = content.read(clip).ok_or_else(|| failed("its clip isn't resident".to_owned()))?;
        clip.check_skeleton(skeleton_view).map_err(|error| failed(error.to_string()))?;
        let position = model.tick as f32;
        pose.sample(clip, position);
        root = root_motion(clip, position).map_or(Mat4::IDENTITY, |root| root.to_mat4());
    }
    pose.finish();
    let place = Mat4::from_rotation_translation(Quat::from_rotation_y(FACES_DEGREES.to_radians()), STANDS_AT);
    let morph_weights = match (model.face, &rig) {
        (Some(preset), Some(rig)) => {
            let rig_view = content.read(rig).ok_or_else(|| failed("its rig isn't resident".to_owned()))?;
            if let Some(mut springs) = SpringBones::new(rig_view, &pose) {
                let world_from_model = Affine3A::from_mat4(place * root);
                for tick in 0..=SETTLE_TICKS {
                    let frame = SpringFrame {
                        world_from_model,
                        ticks: f64::from(tick),
                        pose_moved: tick == 0,
                        active: true,
                    };
                    springs.update(&mut pose, &frame);
                }
            }
            let mut weights = vec![0.0; copy.morph_targets.len()];
            let mut face = FaceLayer::new(rig_view, weights.len(), None)
                .ok_or_else(|| failed("its rig has no expression presets".to_owned()))?;
            let expression = sapphire_core::Name::new(preset.name());
            face.show(FaceOrder { key: 1, expression, weight: 1.0, blend: 0.0, since: 0.0 });
            face.apply(&mut weights);
            weights
        }
        _ => Vec::new(),
    };
    let height = copy.streams.bounds.max[1].max(0.1);
    let (_, head_rotation, head) = pose.model()[head].to_scale_rotation_translation();
    let figure = Figure {
        head: head + head_rotation * Vec3::new(0.0, HEAD_CENTER_SHARE * height, 0.0),
        feet: Vec3::new(head.x, 0.0, head.z),
        body_radius: BODY_RADIUS_SHARE * height,
        face: Quat::IDENTITY,
        expression: FaceExpression { blush: 0.0, gloom: 0.0 },
    };
    let mut palette = vec![Affine3A::IDENTITY; pose.palette_len()];
    pose.write_palette(&mut palette);
    let handle = content.mesh_on_renderer(model.mesh).expect("a ready mesh was uploaded here");
    Ok(HeldModel {
        mesh: handle,
        pose: RenderPose { palette, morph_weights },
        figure,
        place: place * root,
        assets: ModelAssets { _mesh: mesh, _skeleton: skeleton, _clip: clip, _rig: rig },
    })
}

/// Stands `model` on the stage, lit by the default rig.
pub fn spawn(scene: &mut Scene, model: HeldModel) {
    scene.extract_poses();
    let rig = CharacterLightRig { rig: LightRigId::DEFAULT, head: model.figure.head };
    scene.world_mut().spawn((
        RenderMesh(model.mesh),
        RenderTransform(model.place),
        rig,
        model.figure,
        model.pose,
        model.assets,
    ));
}

#[cfg(test)]
mod tests {
    use super::*;

    const MESH: &str = "41e229ad-28a3-41f9-aeb8-ba214e06be6a";
    const CLIP: &str = "0853fc78-9a5d-4f9d-bcfe-1c8a485a0db4";

    #[test]
    fn a_model_reads_as_a_mesh_and_maybe_a_clip_at_a_tick() {
        let mesh = AssetId::from_text(MESH);
        let clip = AssetId::from_text(CLIP);
        assert_eq!(MESH.parse(), Ok(Model { mesh, clip: None, tick: 0, face: None }));
        assert_eq!(
            format!("{MESH}:{CLIP}").parse(),
            Ok(Model { mesh, clip: Some(clip), tick: 0, face: None })
        );
        assert_eq!(
            format!("{MESH}:{CLIP}@19").parse(),
            Ok(Model { mesh, clip: Some(clip), tick: 19, face: None })
        );
        let happy = Some(ExpressionPreset::Happy);
        assert_eq!(format!("{MESH}+happy").parse(), Ok(Model { mesh, clip: None, tick: 0, face: happy }));
        assert_eq!(
            format!("{MESH}:{CLIP}@3+happy").parse(),
            Ok(Model { mesh, clip: Some(clip), tick: 3, face: happy })
        );
        assert!(format!("{MESH}+grin").parse::<Model>().unwrap_err().contains("expression presets"));
        assert!("lancer".parse::<Model>().unwrap_err().contains("asset ID"));
        assert!(format!("{MESH}:{CLIP}@soon").parse::<Model>().unwrap_err().contains("tick"));
    }
}
