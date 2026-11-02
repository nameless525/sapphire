//! What the last cook made: the tables a move's clip gave it, its clip, its skeleton and the mesh on it.
//!
//! The editor reads the cook's packs directly, by asset ID, as any tool can: only the tables of contents
//! and the assets asked for, so a large town in the same pack costs nothing. It never cooks. A document's
//! move set is what its sidecar's ID finds, and it's what a device has too, so a window checks here as it
//! will there ([`crate::file::judge`]). With no cook, moves with clips can't be previewed and the
//! editor says so: their windows still edit, save and push.

use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use sapphire_assets::AssetId;
use sapphire_assets::AssetKind;
use sapphire_assets::mesh::{ArchivedMesh, Mesh, SkinBind, VertexInfluence};
use sapphire_assets::pack::{Decoder, HEADER_LEN, PackHeader, Toc};
use sapphire_assets::rig::{
    ArchivedCharacterRig, ArchivedConstraintKind, CharacterRig, Constraint, ConstraintKind, HumanBone,
};
use sapphire_assets::skeleton::{ArchivedSkeleton, Skeleton};
use sapphire_math::Vec3;
use sapphire_moves::{ClipMotionDocument, MoveSet};
use sapphire_serialize::cooked::AlignedVec;

use crate::text;

/// Triangles the preview's mesh aims for: about what a clay figure needs to read, drawn by egui. The
/// finest LOD at or under this, or the coarsest if none is.
const FIGURE_TRIANGLES: usize = 14_000;

/// A pack's table of contents, and where the pack is.
#[derive(Debug)]
struct OpenPack {
    path: PathBuf,
    toc: Toc,
}

/// The packs of one cook, read as needed.
#[derive(Debug)]
pub struct Cook {
    dir: PathBuf,
    packs: Vec<OpenPack>,
    /// The newest pack's time, which says a cook has run since.
    stamp: Option<SystemTime>,
}

impl Cook {
    /// The cook in `dir`: `target/content/desktop`. `Err` says why there's none to read.
    pub fn open(dir: &Path) -> Result<Self, String> {
        let entries = std::fs::read_dir(dir).map_err(|error| format!("{}: {error}", dir.display()))?;
        let mut packs = Vec::new();
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().is_some_and(|extension| extension == "spack") {
                let toc = read_toc(&path).map_err(|error| format!("{}: {error}", path.display()))?;
                packs.push(OpenPack { path, toc });
            }
        }
        if packs.is_empty() {
            return Err(format!("{} holds no packs", dir.display()));
        }
        packs.sort_by(|a, b| a.path.cmp(&b.path));
        Ok(Self { dir: dir.to_owned(), packs, stamp: newest_pack(dir) })
    }

    /// Whether the packs on disk are newer than these.
    #[must_use]
    pub fn is_stale(&self) -> bool {
        newest_pack(&self.dir) != self.stamp
    }

    /// Asset `id`, decoded, from whichever pack holds it. `None` if none does.
    pub fn read(&self, id: AssetId) -> Result<Option<AlignedVec>, String> {
        for pack in &self.packs {
            let Some(entry) = pack.toc.find(id) else { continue };
            let range = pack.toc.stored_range(entry);
            let mut stored = vec![0; (range.end - range.start) as usize];
            let error = |error: std::io::Error| format!("{}: {error}", pack.path.display());
            let mut file = File::open(&pack.path).map_err(error)?;
            file.seek(SeekFrom::Start(range.start)).map_err(error)?;
            file.read_exact(&mut stored).map_err(error)?;
            let decoded = pack
                .toc
                .decode_asset(entry, &stored, &mut Decoder::default())
                .map_err(|error| format!("{}: {error}", pack.path.display()))?;
            return Ok(Some(decoded));
        }
        Ok(None)
    }

    /// The largest mesh in the cook that depends on `skeleton`: the character on it, not an accessory.
    #[must_use]
    pub fn mesh_on_skeleton(&self, skeleton: AssetId) -> Option<AssetId> {
        self.packs
            .iter()
            .flat_map(|pack| {
                pack.toc.entries().iter().filter(move |entry| {
                    entry.kind == AssetKind::Mesh && pack.toc.dependencies(entry).contains(&skeleton)
                })
            })
            .max_by_key(|entry| (entry.size, entry.id))
            .map(|entry| entry.id)
    }

    /// The set a cook made of the document at `path`, by its sidecar's ID.
    pub fn move_set_of(&self, path: &Path) -> Result<Option<MoveSet>, String> {
        let Some(id) = sidecar_id(path) else { return Ok(None) };
        let Some(bytes) = self.read(id)? else { return Ok(None) };
        MoveSet::from_cooked(&bytes).map(Some).map_err(|error| format!("{id}: {error}"))
    }
}

fn read_toc(path: &Path) -> Result<Toc, String> {
    let mut file = File::open(path).map_err(|error| error.to_string())?;
    let file_len = file.metadata().map_err(|error| error.to_string())?.len();
    let mut head = [0; HEADER_LEN];
    file.read_exact(&mut head).map_err(|error| error.to_string())?;
    let header = PackHeader::read(&head).map_err(|error| error.to_string())?;
    let mut toc = vec![0; header.toc_len() as usize];
    file.read_exact(&mut toc).map_err(|error| error.to_string())?;
    Toc::read(header, &toc, file_len).map_err(|error| error.to_string())
}

/// When the newest pack in `dir` was written, if it holds any.
#[must_use]
pub fn newest_pack(dir: &Path) -> Option<SystemTime> {
    std::fs::read_dir(dir)
        .ok()?
        .flatten()
        .filter(|entry| entry.path().extension().is_some_and(|extension| extension == "spack"))
        .filter_map(|entry| entry.metadata().ok()?.modified().ok())
        .max()
}

/// The ID in the sidecar beside `path`, if it has one that reads.
#[must_use]
pub fn sidecar_id(path: &Path) -> Option<AssetId> {
    let mut sidecar = path.as_os_str().to_owned();
    sidecar.push(".meta");
    let text = std::fs::read_to_string(sidecar).ok()?;
    let tree = text::scan(&text).ok()?;
    tree.field("data")?.field("id")?.string(&text)?.parse().ok()
}

/// The clip a move's `clip` block names, as a cook derives its ID: see `sapphire_moves`' "From a clip".
#[must_use]
pub fn clip_id(clip: &ClipMotionDocument) -> Option<AssetId> {
    let source: AssetId = clip.source.parse().ok()?;
    Some(match (&clip.name, &clip.rig) {
        (Some(name), Some(rig)) => source.sub_asset(&format!("clip/{name}@{rig}")),
        (Some(name), None) => source.sub_asset(&format!("clip/{name}")),
        (None, Some(rig)) => source.sub_asset(&format!("rig/{rig}")),
        (None, None) => source,
    })
}

/// A cooked mesh, as much of it as the preview draws, read out of its archive.
#[derive(Clone, Debug)]
pub struct FigureMesh {
    /// In bind space.
    pub positions: Vec<Vec3>,
    pub influences: Vec<VertexInfluence>,
    pub triangles: Vec<[u32; 3]>,
    pub binds: Vec<SkinBind>,
    pub skeleton: AssetId,
}

impl FigureMesh {
    /// Reads asset `id`, if it's a skinned mesh with a skeleton.
    pub fn read(cook: &Cook, id: AssetId) -> Result<Option<Self>, String> {
        let Some(bytes) = cook.read(id)? else { return Ok(None) };
        let mesh = Mesh::read(&bytes).map_err(|error| format!("{id}: {error}"))?;
        Ok(Self::of(mesh))
    }

    fn of(mesh: &ArchivedMesh) -> Option<Self> {
        let skin = mesh.skin.as_ref()?;
        let skeleton = *skin.skeleton.as_ref()?;
        let lods = &mesh.lods;
        let triangles_of = |lod: &sapphire_assets::mesh::ArchivedLod| -> usize {
            lod.submeshes.iter().map(|submesh| submesh.index_count.to_native() as usize / 3).sum()
        };
        let lod = lods.iter().find(|lod| triangles_of(lod) <= FIGURE_TRIANGLES).or_else(|| lods.last())?;
        let count = lod.vertex_count.to_native() as usize;
        let positions: Vec<Vec3> = (0..count)
            .map(|index| mesh.streams.position(index).map(Vec3::from_array))
            .collect::<Option<_>>()?;
        let influences: Vec<VertexInfluence> =
            (0..count).map(|index| skin.influences.get(index)).collect::<Option<_>>()?;
        let mut triangles = Vec::with_capacity(triangles_of(lod));
        for submesh in lod.submeshes.iter() {
            let first = submesh.first_index.to_native() as usize;
            let indices = &mesh.indices[first..first + submesh.index_count.to_native() as usize];
            triangles.extend(
                indices
                    .as_chunks::<3>()
                    .0
                    .iter()
                    .map(|[a, b, c]| [a.to_native(), b.to_native(), c.to_native()]),
            );
        }
        let binds = skin
            .binds
            .iter()
            .map(|bind| SkinBind {
                joint: bind.joint.to_native(),
                inverse_bind_matrix: bind
                    .inverse_bind_matrix
                    .map(|column| column.map(|value| value.to_native())),
            })
            .collect();
        Some(Self { positions, influences, triangles, binds, skeleton })
    }
}

/// What a figure's pose reads of its rig, if it has one.
#[derive(Clone, Debug, Default)]
pub struct FigureRig {
    pub constraints: Vec<Constraint>,
    pub humanoid: Vec<(HumanBone, usize)>,
}

impl FigureRig {
    /// The `rig` sub-asset of mesh `id`: a VRM has one, and most others don't.
    pub fn read(cook: &Cook, id: AssetId) -> Result<Self, String> {
        let Some(bytes) = cook.read(id.sub_asset("rig"))? else { return Ok(Self::default()) };
        let rig = CharacterRig::read(&bytes).map_err(|error| format!("{id}'s rig: {error}"))?;
        Ok(Self::of(rig))
    }

    fn of(rig: &ArchivedCharacterRig) -> Self {
        Self {
            humanoid: rig
                .humanoid
                .iter()
                .map(|entry| (entry.bone, usize::from(entry.joint.to_native())))
                .collect(),
            constraints: rig
                .constraints
                .iter()
                .map(|constraint| Constraint {
                    joint: constraint.joint.to_native(),
                    source: constraint.source.to_native(),
                    weight: constraint.weight.to_native(),
                    kind: match constraint.kind {
                        ArchivedConstraintKind::Roll { axis } => ConstraintKind::Roll { axis },
                        ArchivedConstraintKind::Aim { axis } => ConstraintKind::Aim { axis },
                        ArchivedConstraintKind::Rotation => ConstraintKind::Rotation,
                    },
                })
                .collect(),
        }
    }
}

/// Reads a skeleton's bytes, for whoever keeps them alive.
pub fn read_skeleton(bytes: &[u8]) -> Result<&ArchivedSkeleton, String> {
    Skeleton::read(bytes).map_err(|error| error.to_string())
}
