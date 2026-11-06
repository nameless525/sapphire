//! What the last cook made: the tables a move's clip gave it, its clip, its skeleton and the mesh on it.
//!
//! The editor reads the cook's packs directly, by asset ID, as any tool can: only the tables of contents
//! and the assets asked for, so a large town in the same pack costs nothing. It never cooks. A document's
//! move set is what its sidecar's ID finds, and it's what a device has too, so a window checks here as it
//! will there ([`crate::judge`]). With no cook, moves with clips can't be previewed and the
//! editor says so: their windows still edit, save and push.

use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, PoisonError};
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

use sapphire_document::text;

/// Triangles the preview's mesh aims for: about what a clay figure needs to read, drawn by egui. The
/// finest LOD at or under this, or the coarsest if none is.
const FIGURE_TRIANGLES: usize = 14_000;

/// A pack's table of contents, and where the pack is.
#[derive(Debug)]
struct OpenPack {
    path: PathBuf,
    file: Mutex<File>,
    toc: Toc,
}

/// The packs of one cook, read as needed.
#[derive(Debug)]
pub struct Cook {
    dir: PathBuf,
    packs: Vec<OpenPack>,
    stamp: Vec<PackStamp>,
}

/// Header identity includes the TOC hash, so equal/restored modification times still detect a recook.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct PackStamp {
    path: PathBuf,
    len: u64,
    modified: Option<SystemTime>,
    header: Vec<u8>,
}

pub(crate) fn pack_stamp(dir: &Path) -> Result<Vec<PackStamp>, String> {
    let result = || -> std::io::Result<Vec<PackStamp>> {
        let mut stamps = Vec::new();
        for entry in std::fs::read_dir(dir)? {
            let path = entry?.path();
            if path.extension().is_some_and(|extension| extension == "spack") {
                let file = File::open(&path)?;
                let metadata = file.metadata()?;
                let mut header = Vec::with_capacity(HEADER_LEN);
                file.take(HEADER_LEN as u64).read_to_end(&mut header)?;
                stamps.push(PackStamp {
                    path,
                    len: metadata.len(),
                    modified: metadata.modified().ok(),
                    header,
                });
            }
        }
        stamps.sort_by(|a, b| a.path.cmp(&b.path));
        Ok(stamps)
    };
    result().map_err(|error| format!("{}: {error}", dir.display()))
}

impl Cook {
    /// The cook in `dir`: `target/content/desktop`. `Err` says why there's none to read.
    pub fn open(dir: &Path) -> Result<Self, String> {
        let stamp = pack_stamp(dir)?;
        let mut packs = Vec::new();
        for entry in &stamp {
            let path = entry.path.clone();
            let mut file = File::open(&path).map_err(|error| format!("{}: {error}", path.display()))?;
            let toc = read_toc(&mut file).map_err(|error| format!("{}: {error}", path.display()))?;
            packs.push(OpenPack { path, file: Mutex::new(file), toc });
        }
        if packs.is_empty() {
            return Err(format!("{} holds no packs", dir.display()));
        }
        if pack_stamp(dir)? != stamp {
            return Err("the cook changed while opening its packs".to_owned());
        }
        Ok(Self { dir: dir.to_owned(), packs, stamp })
    }

    /// Whether the pack set or any pack identity differs, regardless of modification-time ordering.
    #[must_use]
    pub fn is_stale(&self) -> bool {
        pack_stamp(&self.dir).as_ref() != Ok(&self.stamp)
    }

    /// Asset `id`, decoded, from whichever pack holds it. `None` if none does.
    pub fn read(&self, id: AssetId) -> Result<Option<AlignedVec>, String> {
        for pack in &self.packs {
            let Some(entry) = pack.toc.find(id) else { continue };
            let range = pack.toc.stored_range(entry);
            let mut stored = vec![0; (range.end - range.start) as usize];
            let error = |error: std::io::Error| format!("{}: {error}", pack.path.display());
            {
                // Keep the same open file as its TOC across atomic recooks; never mix two generations.
                let mut file = pack.file.lock().unwrap_or_else(PoisonError::into_inner);
                file.seek(SeekFrom::Start(range.start)).map_err(error)?;
                file.read_exact(&mut stored).map_err(error)?;
            }
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

fn read_toc(file: &mut File) -> Result<Toc, String> {
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
        (Some(name), Some(rig)) => {
            sapphire_assets::graph::imported_clip_id(source, name, Some(rig.parse().ok()?))
        }
        (Some(name), None) => sapphire_assets::graph::imported_clip_id(source, name, None),
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

#[cfg(test)]
mod tests {
    use std::fs::{self, FileTimes};
    use std::time::Duration;

    use sapphire_assets::pack::{Codec, PackAsset, write_pack};

    use super::*;

    fn pack(id: AssetId, byte: u8) -> Vec<u8> {
        write_pack(vec![
            PackAsset::encode(id, AssetKind::Sound, Codec::None, &[byte; 8], Vec::new(), 1)
                .expect("encoded asset"),
        ])
        .expect("pack")
    }

    #[test]
    fn recooks_use_coherent_open_files_and_detect_each_pack_even_at_equal_timestamps() {
        let dir = std::env::temp_dir().join(format!("sapphire_editor_pack_snapshot_{}", std::process::id()));
        fs::create_dir_all(&dir).expect("directory");
        let a = dir.join("a.spack");
        let b = dir.join("b.spack");
        let id = AssetId::from_bytes([1; 16]);
        fs::write(&a, pack(id, 1)).expect("first pack");
        fs::write(&b, pack(AssetId::from_bytes([2; 16]), 3)).expect("second pack");
        let set_time = |path: &Path, seconds| {
            File::options()
                .write(true)
                .open(path)
                .expect("file")
                .set_times(
                    FileTimes::new().set_modified(SystemTime::UNIX_EPOCH + Duration::from_secs(seconds)),
                )
                .expect("timestamp");
        };
        set_time(&a, 1);
        set_time(&b, 2);
        let cook = Cook::open(&dir).expect("cook");
        assert!(!cook.is_stale());
        let staged = dir.join("replacement");
        fs::write(&staged, pack(id, 2)).expect("new generation");
        set_time(&staged, 1);
        fs::rename(staged, &a).expect("atomic recook with the same timestamp and size");
        assert!(cook.is_stale(), "the non-newest pack changed");
        assert_eq!(&cook.read(id).expect("old generation").expect("asset")[..], &[1; 8]);
        let fresh = Cook::open(&dir).expect("new cook");
        assert_eq!(&fresh.read(id).expect("new generation").expect("asset")[..], &[2; 8]);
        fs::remove_file(&a).expect("remove a non-newest pack");
        assert!(fresh.is_stale());
        drop((cook, fresh));
        fs::remove_dir_all(dir).expect("cleanup");
    }
}
