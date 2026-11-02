//! The preview's character: a cooked mesh on a cooked skeleton, posed by `sapphire_anim` from the move's
//! own clip at the tick the simulation reads, as the sandbox poses it for `--clip`.
//!
//! Posed once a tick and kept, so scrubbing reads a table. Skinned on the CPU when a frame asks ([`Figure::
//! skinned`]), which is cheap at the coarse LOD it reads, because egui draws it: the editor has no renderer
//! and needs no GPU for this, so everything here runs in a test.
//!
//! What a real frame has and this doesn't: materials, outlines, faces, hair's springs and IK. The body is
//! where the pose says, and the hit volumes are where the simulation has them, which is what timing a move
//! needs. The look stays the sandbox's.

use std::collections::HashMap;
use std::path::Path;

use sapphire_anim::{CharacterPose, Clip};
use sapphire_assets::AssetId;
use sapphire_assets::rig::HumanBone;
use sapphire_assets::skeleton::NO_PARENT;
use sapphire_math::{Affine3A, Mat4, Vec3};
use sapphire_moves::MoveDocument;
use sapphire_serialize::cooked::AlignedVec;

use crate::cooked::{Cook, FigureMesh, FigureRig, clip_id, read_skeleton, sidecar_id};

/// A character to pose: what a cook made of its mesh, and its skeleton's parents, for its bones.
#[derive(Debug)]
pub struct Figure {
    mesh: Option<FigureMesh>,
    rig: FigureRig,
    skeleton: AlignedVec,
    parents: Vec<u16>,
    names: Vec<String>,
    /// Each clip's model transforms, tick by tick.
    poses: HashMap<AssetId, Vec<Vec<Affine3A>>>,
    /// The skeleton's rest pose, for a move with no clip.
    rest: Vec<Affine3A>,
}

impl Figure {
    /// The figure on skeleton `id`, with the largest mesh a cook made for it.
    fn on_skeleton(cook: &Cook, id: AssetId, mesh: Option<AssetId>) -> Result<Option<Self>, String> {
        let Some(skeleton) = cook.read(id)? else { return Ok(None) };
        let mesh_id = mesh.or_else(|| cook.mesh_on_skeleton(id));
        let mesh = mesh_id.map(|mesh| FigureMesh::read(cook, mesh)).transpose()?.flatten();
        let rig = mesh_id.map_or(Ok(FigureRig::default()), |mesh| FigureRig::read(cook, mesh))?;
        let (parents, names) = {
            let archived = read_skeleton(&skeleton)?;
            (
                archived.parents.iter().map(|parent| parent.to_native()).collect(),
                archived.names.iter().map(|name| name.as_str().to_owned()).collect(),
            )
        };
        let mut figure = Self { mesh, rig, skeleton, parents, names, poses: HashMap::new(), rest: Vec::new() };
        figure.rest = figure.pose()?.model().to_vec();
        Ok(Some(figure))
    }

    fn pose(&self) -> Result<CharacterPose, String> {
        let binds = self.mesh.as_ref().map_or(&[][..], |mesh| &mesh.binds);
        Ok(CharacterPose::new(read_skeleton(&self.skeleton)?, binds, &self.rig.constraints, Vec::new()))
    }

    /// The model transforms of `clip`, one list per tick up to and including its last.
    fn sample_clip(&mut self, cook: &Cook, clip: AssetId) -> Result<bool, String> {
        if self.poses.contains_key(&clip) {
            return Ok(true);
        }
        let Some(bytes) = cook.read(clip)? else { return Ok(false) };
        let archived = Clip::read(&bytes).map_err(|error| format!("clip {clip}: {error}"))?;
        let mut pose = self.pose()?;
        let ticks = u32::from(archived.ticks.to_native());
        let mut model = Vec::with_capacity(ticks as usize + 1);
        for tick in 0..=ticks {
            pose.sample(archived, tick as f32);
            pose.finish();
            model.push(pose.model().to_vec());
        }
        self.poses.insert(clip, model);
        Ok(true)
    }

    /// Every joint's model transform in `clip` at `tick`, or at rest with no clip. Past a clip's end it
    /// holds, as a sampler does.
    #[must_use]
    pub fn model_at(&self, clip: Option<AssetId>, tick: u32) -> &[Affine3A] {
        let Some(ticks) = clip.and_then(|clip| self.poses.get(&clip)) else { return &self.rest };
        ticks.get(tick as usize).or_else(|| ticks.last()).map_or(&self.rest, |model| model.as_slice())
    }

    /// Lines from each joint to its parent, in model space.
    pub fn bones<'a>(&'a self, model: &'a [Affine3A]) -> impl Iterator<Item = (Vec3, Vec3)> + 'a {
        self.parents.iter().zip(model).filter(|(parent, _)| **parent != NO_PARENT).map(|(parent, joint)| {
            (Vec3::from(model[usize::from(*parent)].translation), Vec3::from(joint.translation))
        })
    }

    /// The joint a humanoid bone is.
    #[must_use]
    pub fn joint_of_bone(&self, bone: HumanBone) -> Option<usize> {
        self.rig.humanoid.iter().find(|(found, _)| *found == bone).map(|(_, joint)| *joint)
    }

    /// A joint by its name.
    #[must_use]
    pub fn joint_named(&self, name: &str) -> Option<usize> {
        self.names.iter().position(|found| found == name)
    }

    /// Its triangles, if it has a mesh.
    #[must_use]
    pub fn triangles(&self) -> &[[u32; 3]] {
        self.mesh.as_ref().map_or(&[], |mesh| &mesh.triangles)
    }

    /// Every vertex posed by `model`, in model space. Empty with no mesh.
    #[must_use]
    pub fn skinned(&self, model: &[Affine3A]) -> Vec<Vec3> {
        let Some(mesh) = &self.mesh else { return Vec::new() };
        let palette: Vec<Affine3A> = mesh
            .binds
            .iter()
            .map(|bind| {
                let inverse = Affine3A::from_mat4(Mat4::from_cols_array_2d(&bind.inverse_bind_matrix));
                model[usize::from(bind.joint)] * inverse
            })
            .collect();
        mesh.positions
            .iter()
            .zip(&mesh.influences)
            .map(|(position, influence)| {
                let mut out = Vec3::ZERO;
                for (bind, weight) in influence.binds.iter().zip(influence.weights) {
                    if weight > 0 {
                        out += palette[usize::from(*bind)].transform_point3(*position) * (f32::from(weight) / 255.0);
                    }
                }
                out
            })
            .collect()
    }
}

/// The figures of a document, by what it names, from one cook.
#[derive(Debug, Default)]
pub struct Figures {
    by_skeleton: HashMap<AssetId, Figure>,
    /// The figure for a document's moves that name no clip: the mesh beside its file, if there is one.
    beside: Option<AssetId>,
}

/// What a move poses with: which figure, and which of its clips.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Casting {
    pub skeleton: AssetId,
    pub clip: Option<AssetId>,
}

impl Figures {
    /// For the document at `path`: a mesh named as it is, beside it, stands in for moves with no clip.
    pub fn for_document(path: &Path, cook: &Cook) -> Result<Self, String> {
        let mut figures = Self::default();
        let stem = path.file_name().and_then(|name| name.to_str()).and_then(|name| name.strip_suffix(".moves.ron"));
        let glb = stem.map(|stem| path.with_file_name(format!("{stem}.glb")));
        figures.beside = glb.and_then(|glb| sidecar_id(&glb));
        if let Some(mesh_id) = figures.beside
            && let Some(mesh) = FigureMesh::read(cook, mesh_id)?
        {
            let skeleton = mesh.skeleton;
            if let Some(figure) = Figure::on_skeleton(cook, skeleton, Some(mesh_id))? {
                figures.by_skeleton.insert(skeleton, figure);
            }
        }
        Ok(figures)
    }

    /// Loads what `mv` needs, and says what it poses with. `None` if the cook has nothing to pose it with.
    pub fn cast(&mut self, mv: &MoveDocument, cook: &Cook) -> Result<Option<Casting>, String> {
        if let Some(clip) = mv.motion.clip.as_ref().and_then(clip_id) {
            let clip = self.existing_clip(cook, mv, clip)?;
            if let Some(clip) = clip {
                let Some(bytes) = cook.read(clip)? else { return Ok(None) };
                let skeleton = Clip::read(&bytes)
                    .map_err(|error| format!("clip {clip}: {error}"))?
                    .skeleton
                    .as_ref()
                    .copied();
                let Some(skeleton) = skeleton else { return Ok(None) };
                if !self.by_skeleton.contains_key(&skeleton) {
                    let Some(figure) = Figure::on_skeleton(cook, skeleton, None)? else { return Ok(None) };
                    self.by_skeleton.insert(skeleton, figure);
                }
                let figure = self.by_skeleton.get_mut(&skeleton).expect("just there");
                if figure.sample_clip(cook, clip)? {
                    return Ok(Some(Casting { skeleton, clip: Some(clip) }));
                }
            }
        }
        // No clip, or none in this cook: at rest, if there's a mesh to rest.
        Ok(self.by_skeleton.keys().min().map(|skeleton| Casting { skeleton: *skeleton, clip: None }))
    }

    /// The clip a block names, in this cook: a key-pose document's own rig is the document's ID.
    fn existing_clip(&self, cook: &Cook, mv: &MoveDocument, clip: AssetId) -> Result<Option<AssetId>, String> {
        if cook.read(clip)?.is_some() {
            return Ok(Some(clip));
        }
        let source = mv.motion.clip.as_ref().and_then(|clip| clip.source.parse().ok());
        match source {
            Some(source) if cook.read(source)?.is_some() => Ok(Some(source)),
            _ => Ok(None),
        }
    }

    #[must_use]
    pub fn get(&self, casting: Casting) -> Option<&Figure> {
        self.by_skeleton.get(&casting.skeleton)
    }
}
