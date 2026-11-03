//! What kind of data an asset is. Packs record it in their table of contents, so the runtime
//! knows how to check, budget and upload an asset before reading it.

use std::fmt;

use sapphire_colliders::{COLLIDERS_EXTENSION, ColliderError, ColliderSet};
use sapphire_moves::{MOVE_SET_EXTENSION, MoveSet, MoveSetError};
use sapphire_navmesh::{NAVMESH_EXTENSION, NavMesh, NavMeshError};
use sapphire_strings::{STRINGS_EXTENSION, StringTable, StringsError};
use sapphire_timeline::{TIMELINE_EXTENSION, Timeline, TimelineError};
use sapphire_zone::{ENCOUNTER_EXTENSION, Encounter, ZONE_EXTENSION, Zone, ZoneError};
use serde::{Deserialize, Serialize};

use crate::AssetId;
use crate::clip::{CLIP_EXTENSION, Clip, ClipError};
use crate::cue::{CUE_EXTENSION, Cue, CueError};
use crate::font::{FONT_CHAINS_EXTENSION, FONT_EXTENSION, Font, FontChains, FontError};
use crate::graph::{AnimGraph, GRAPH_EXTENSION, GraphError};
use crate::material::{MATERIAL_EXTENSION, Material, MaterialError};
use crate::mesh::{MESH_EXTENSION, Mesh, MeshError};
use crate::probes::{PROBE_GRID_EXTENSION, ProbeGrid, ProbeGridError};
use crate::rig::{CharacterRig, RIG_EXTENSION, RigError};
use crate::skeleton::{SKELETON_EXTENSION, Skeleton, SkeletonError};
use crate::sound::{SOUND_EXTENSION, Sound, SoundError};
use crate::texture::{TEXTURE_EXTENSION, Texture, TextureFormatError};
use crate::ui::{UI_EXTENSION, UiDocument, UiError};

/// The kinds of cooked asset. Each is also a residency category with a budget of its own
/// ([`AssetBudgets`](crate::AssetBudgets)).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[repr(u8)]
pub enum AssetKind {
    /// A cooked mesh (`.smesh`): [`Mesh`].
    Mesh = 1,
    /// A cooked texture (KTX2): [`Texture`].
    Texture = 2,
    /// A cooked material (`.smat`): [`Material`].
    Material = 3,
    /// A character rig (`.srig`, M1 A6): [`CharacterRig`]. Animation's CPU data, never
    /// uploaded.
    Rig = 4,
    /// An ambient probe grid (`.sprobe`, M1 W5): [`ProbeGrid`], uploaded to the renderer.
    ProbeGrid = 5,
    /// A skeleton (`.sskel`, M2 N1): [`Skeleton`]. Animation's CPU data, never uploaded.
    Skeleton = 6,
    /// An animation clip (`.sclip`, M2 N1): [`Clip`]. Animation's CPU data, never uploaded.
    Clip = 7,
    /// A zone's static colliders (`.scol`, M2 D3): [`ColliderSet`]. CPU data, never uploaded.
    Colliders = 8,
    /// A cooked sound (`.ssound`, M2 E1): [`Sound`]. The mixer's CPU data, never uploaded.
    Sound = 9,
    /// A cue (`.scue`, M2 E1): [`Cue`], naming the sounds it picks from.
    Cue = 10,
    /// A character's moves (`.smoves`, M2 G2): [`MoveSet`], which the simulation reads and which
    /// never goes near a GPU.
    MoveSet = 11,
    /// A character's animation graph (`.sgraph`, M2 N3): [`AnimGraph`], naming the clips it plays.
    /// Animation's CPU data, never uploaded.
    AnimGraph = 12,
    /// A cooked zone (`.szone`, M3 W1): [`Zone`], which the simulation and the world read and which
    /// never goes near a GPU itself. What it holds and what it only names: [`Self::reach`].
    Zone = 13,
    /// One locale's strings (`.sloc`, M3 V2): [`StringTable`]. CPU data that `sapphire_loc` reads, never
    /// uploaded.
    Strings = 14,
    /// A font cut down for a pack (`.sfont`, M3 U3): [`Font`]. The shaper's CPU data, never uploaded.
    Font = 15,
    /// Which fonts answer for each use and script (`.sfontc`, M3 U3): [`FontChains`], naming them.
    FontChains = 16,
    /// A zone's navmesh (`.snav`, M3 P1): [`NavMesh`], which the simulation reads and which never goes
    /// near a GPU.
    NavMesh = 17,
    /// A cooked timeline (`.stimeline`, M3 Q1): [`Timeline`], which the simulation and the sequencer read.
    /// It names what it plays by name, so it depends on nothing.
    Timeline = 18,
    /// A screen (`.sui`, M3 U4): [`UiDocument`], which `sapphire_ui` builds a tree from. CPU data, never
    /// uploaded.
    Ui = 19,
    /// A zone's encounter (`.sencounter`, M3 B2): [`Encounter`], which the simulation reads when it's
    /// given a zone and which never goes near a GPU.
    Encounter = 20,
}

impl AssetKind {
    /// Every kind, in a stable order.
    pub const ALL: [Self; 20] = [
        Self::Mesh,
        Self::Texture,
        Self::Material,
        Self::Rig,
        Self::ProbeGrid,
        Self::Skeleton,
        Self::Clip,
        Self::Colliders,
        Self::Sound,
        Self::Cue,
        Self::MoveSet,
        Self::AnimGraph,
        Self::Zone,
        Self::Strings,
        Self::Font,
        Self::FontChains,
        Self::NavMesh,
        Self::Timeline,
        Self::Ui,
        Self::Encounter,
    ];

    /// The kind's position in [`Self::ALL`].
    pub const fn index(self) -> usize {
        self as usize - 1
    }

    /// The file extension of the kind's cooked format, by which a cook names its outputs, prunes stale
    /// ones and tells a packer what a file is.
    pub const fn extension(self) -> &'static str {
        match self {
            Self::Mesh => MESH_EXTENSION,
            Self::Texture => TEXTURE_EXTENSION,
            Self::Material => MATERIAL_EXTENSION,
            Self::Rig => RIG_EXTENSION,
            Self::ProbeGrid => PROBE_GRID_EXTENSION,
            Self::Skeleton => SKELETON_EXTENSION,
            Self::Clip => CLIP_EXTENSION,
            Self::Colliders => COLLIDERS_EXTENSION,
            Self::Sound => SOUND_EXTENSION,
            Self::Cue => CUE_EXTENSION,
            Self::MoveSet => MOVE_SET_EXTENSION,
            Self::AnimGraph => GRAPH_EXTENSION,
            Self::Zone => ZONE_EXTENSION,
            Self::Strings => STRINGS_EXTENSION,
            Self::Font => FONT_EXTENSION,
            Self::FontChains => FONT_CHAINS_EXTENSION,
            Self::NavMesh => NAVMESH_EXTENSION,
            Self::Timeline => TIMELINE_EXTENSION,
            Self::Ui => UI_EXTENSION,
            Self::Encounter => ENCOUNTER_EXTENSION,
        }
    }

    /// The name used in messages.
    pub const fn name(self) -> &'static str {
        match self {
            Self::Mesh => "mesh",
            Self::Texture => "texture",
            Self::Material => "material",
            Self::Rig => "rig",
            Self::ProbeGrid => "probe grid",
            Self::Skeleton => "skeleton",
            Self::Clip => "clip",
            Self::Colliders => "colliders",
            Self::Sound => "sound",
            Self::Cue => "cue",
            Self::MoveSet => "move set",
            Self::AnimGraph => "animation graph",
            Self::Zone => "zone",
            Self::Strings => "string table",
            Self::Font => "font",
            Self::FontChains => "font chains",
            Self::NavMesh => "navmesh",
            Self::Timeline => "timeline",
            Self::Ui => "UI document",
            Self::Encounter => "encounter",
        }
    }

    /// Whether assets of this kind go to the GPU, through the asset server's upload queue. A rig,
    /// a skeleton, a clip, a collider set, a sound, a cue, a move set, an animation graph, a zone, a
    /// string table, a font, font chains, a navmesh or a timeline is ready once it and its dependencies
    /// are decoded.
    pub const fn uploads(self) -> bool {
        !matches!(
            self,
            Self::Rig
                | Self::Skeleton
                | Self::Clip
                | Self::Colliders
                | Self::Sound
                | Self::Cue
                | Self::MoveSet
                | Self::AnimGraph
                | Self::Zone
                | Self::Strings
                | Self::Font
                | Self::FontChains
                | Self::NavMesh
                | Self::Timeline
                | Self::Ui
                | Self::Encounter
        )
    }

    /// Whether the asset server keeps an asset's decoded bytes once it is ready. Meshes,
    /// textures and probe grids are GPU data, so their bytes go once uploaded (a new renderer
    /// has them read again). Materials are small and CPU code reads them (live link, the editor), and
    /// rigs, skeletons, clips, collider sets, sounds, cues, move sets, animation graphs, zones, string
    /// tables, fonts, font chains, navmeshes and timelines are read on the CPU only, so they stay.
    pub const fn keeps_bytes(self) -> bool {
        matches!(
            self,
            Self::Material
                | Self::Rig
                | Self::Skeleton
                | Self::Clip
                | Self::Colliders
                | Self::Sound
                | Self::Cue
                | Self::MoveSet
                | Self::AnimGraph
                | Self::Zone
                | Self::Strings
                | Self::Font
                | Self::FontChains
                | Self::NavMesh
                | Self::Timeline
                | Self::Ui
                | Self::Encounter
        )
    }

    /// Whether uploading an asset of this kind binds its dependencies' GPU objects, so it must be
    /// sent again when hot reload replaces one: a material binds its textures. A mesh names its
    /// materials by the renderer's handles, which a material reloaded in place keeps. Such kinds
    /// keep their bytes, which they are sent again from.
    pub const fn binds_dependencies(self) -> bool {
        matches!(self, Self::Material)
    }

    /// Whether an asset of this kind may depend on one of `kind`: meshes on materials and
    /// skeletons, materials on textures, clips on skeletons, cues on sounds, animation graphs on
    /// clips, zones on collider sets and navmeshes, font chains on fonts, and textures, rigs, probe grids,
    /// skeletons, collider sets, sounds, move sets, string tables, fonts, navmeshes, timelines and UI documents on
    /// nothing. A zone's meshes and probes aren't dependencies ([`Self::reach`]). The allowed pairs form no
    /// loop, so chains are at most three long and cycles can't exist, whatever a pack claims. A new kind
    /// takes its place in this order.
    pub const fn may_depend_on(self, kind: AssetKind) -> bool {
        matches!(
            (self, kind),
            (Self::Mesh, Self::Material | Self::Skeleton)
                | (Self::Material, Self::Texture)
                | (Self::Clip, Self::Skeleton)
                | (Self::Cue, Self::Sound)
                | (Self::AnimGraph, Self::Clip)
                | (Self::Zone, Self::Colliders | Self::NavMesh | Self::Encounter)
                | (Self::FontChains, Self::Font)
        )
    }

    /// The kind a pack stores as `value`, if any.
    pub const fn from_u8(value: u8) -> Option<Self> {
        match value {
            1 => Some(Self::Mesh),
            2 => Some(Self::Texture),
            3 => Some(Self::Material),
            4 => Some(Self::Rig),
            5 => Some(Self::ProbeGrid),
            6 => Some(Self::Skeleton),
            7 => Some(Self::Clip),
            8 => Some(Self::Colliders),
            9 => Some(Self::Sound),
            10 => Some(Self::Cue),
            11 => Some(Self::MoveSet),
            12 => Some(Self::AnimGraph),
            13 => Some(Self::Zone),
            14 => Some(Self::Strings),
            15 => Some(Self::Font),
            16 => Some(Self::FontChains),
            17 => Some(Self::NavMesh),
            18 => Some(Self::Timeline),
            19 => Some(Self::Ui),
            20 => Some(Self::Encounter),
            _ => None,
        }
    }

    /// Checks cooked bytes of this kind exactly as the runtime reads them (`Mesh::read`,
    /// `Texture::from_ktx2`, `Material::read`, `CharacterRig::read`, `ProbeGrid::read`,
    /// `Skeleton::read`, `Clip::read`, `ColliderSet::read`, `Sound::read`, `Cue::read`,
    /// `MoveSet::from_cooked`, `AnimGraph::read`, `Zone::read`, `StringTable::read`, `Font::read`,
    /// `FontChains::read`, `NavMesh::read`, `Timeline::read`, `UiDocument::read`), and returns the assets they reference, sorted and without
    /// repeats: a mesh's material slots bound to material assets and its skeleton, a material's textures,
    /// a clip's skeleton, a cue's sounds, a graph's clips, what the simulation reads of a zone, font chains'
    /// fonts. The packer records these as the asset's dependencies, and decode jobs check that a decoded
    /// asset references exactly what its pack says it does.
    ///
    /// Everything but textures must start 16-byte aligned, as decoded assets do.
    pub fn references(self, bytes: &[u8]) -> Result<Vec<AssetId>, AssetError> {
        let mut references: Vec<AssetId> = match self {
            Self::Mesh => {
                let mesh = Mesh::read(bytes)?;
                let skeleton = mesh.skin.as_ref().and_then(|skin| skin.skeleton.as_ref().copied());
                let materials = mesh.materials.iter().filter_map(|slot| match &slot.material {
                    crate::mesh::ArchivedSlotMaterial::Asset(id) => Some(*id),
                    crate::mesh::ArchivedSlotMaterial::Embedded(_) => None,
                });
                materials.chain(skeleton).collect()
            }
            Self::Texture => {
                Texture::from_ktx2(bytes)?;
                Vec::new()
            }
            Self::Material => {
                Material::read(bytes)?.textures.iter().filter_map(|binding| binding.texture()).collect()
            }
            // A rig refers to its mesh's joints, morph targets and slots by index, not to assets.
            Self::Rig => {
                CharacterRig::read(bytes)?;
                Vec::new()
            }
            // A probe grid is baked from meshes and materials, but reads none of them.
            Self::ProbeGrid => {
                ProbeGrid::read(bytes)?;
                Vec::new()
            }
            Self::Skeleton => {
                Skeleton::read(bytes)?;
                Vec::new()
            }
            Self::Clip => Clip::read(bytes)?.skeleton.as_ref().copied().into_iter().collect(),
            // Colliders hold their geometry themselves.
            Self::Colliders => {
                ColliderSet::read(bytes)?;
                Vec::new()
            }
            Self::Sound => {
                Sound::read(bytes)?;
                Vec::new()
            }
            Self::Cue => Cue::read(bytes)?.sounds.iter().copied().collect(),
            // A move set's tables are baked into it, and it refers to nothing else.
            Self::MoveSet => {
                MoveSet::from_cooked(bytes)?;
                Vec::new()
            }
            Self::AnimGraph => AnimGraph::read(bytes)?.clip_ids().collect(),
            Self::Zone => Zone::read(bytes)?.references(),
            // An encounter names a zone's things and moves by name, so it needs nothing loaded first.
            Self::Encounter => {
                Encounter::read(bytes)?;
                Vec::new()
            }
            // A table holds its text, and its fonts only by family name.
            Self::Strings => {
                StringTable::read(bytes)?;
                Vec::new()
            }
            Self::Font => {
                Font::read(bytes)?;
                Vec::new()
            }
            Self::FontChains => FontChains::read(bytes)?.font_ids().collect(),
            // A navmesh is baked from colliders, but reads none of them.
            Self::NavMesh => {
                NavMesh::read(bytes)?;
                Vec::new()
            }
            // It plays what its stage has loaded, by name.
            Self::Timeline => {
                Timeline::read(bytes)?;
                Vec::new()
            }
            // A screen names its strings by key and its fonts by role, not by asset.
            Self::Ui => {
                UiDocument::read(bytes)?;
                Vec::new()
            }
        };
        references.sort_unstable();
        references.dedup();
        Ok(references)
    }

    /// [`Self::references`], and what an asset names without holding: a zone's cell and instance meshes
    /// and its probe grid, which loading the zone doesn't load, so they aren't dependencies, but which
    /// must ship with it. This is what decides which pack an asset goes in.
    pub fn reach(self, bytes: &[u8]) -> Result<Vec<AssetId>, AssetError> {
        let mut reach = self.references(bytes)?;
        if self == Self::Zone {
            reach.extend(Zone::read(bytes)?.presentation_references());
            reach.sort_unstable();
            reach.dedup();
        }
        Ok(reach)
    }
}

// Kinds sent again when a dependency is replaced are sent from their kept bytes.
const _: () = {
    let mut index = 0;
    while index < AssetKind::ALL.len() {
        let kind = AssetKind::ALL[index];
        assert!(!kind.binds_dependencies() || kind.keeps_bytes());
        index += 1;
    }
};

// A packer tells a kind by its extension, so no two share one.
const _: () = {
    let mut first = 0;
    while first < AssetKind::ALL.len() {
        let mut second = first + 1;
        while second < AssetKind::ALL.len() {
            assert!(
                !AssetKind::ALL[first].extension().eq_ignore_ascii_case(AssetKind::ALL[second].extension())
            );
            second += 1;
        }
        first += 1;
    }
};

impl fmt::Display for AssetKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

/// Cooked bytes the runtime would refuse, by the check of their kind.
#[derive(Debug, thiserror::Error)]
pub enum AssetError {
    #[error("invalid mesh")]
    Mesh(#[from] MeshError),
    #[error("invalid texture")]
    Texture(#[from] TextureFormatError),
    #[error("invalid material")]
    Material(#[from] MaterialError),
    #[error("invalid rig")]
    Rig(#[from] RigError),
    #[error("invalid probe grid")]
    ProbeGrid(#[from] ProbeGridError),
    #[error("invalid skeleton")]
    Skeleton(#[from] SkeletonError),
    #[error("invalid clip")]
    Clip(#[from] ClipError),
    #[error("invalid collider set")]
    Colliders(#[from] ColliderError),
    #[error("invalid sound")]
    Sound(#[from] SoundError),
    #[error("invalid cue")]
    Cue(#[from] CueError),
    #[error("invalid move set")]
    MoveSet(#[from] MoveSetError),
    #[error("invalid animation graph")]
    AnimGraph(#[from] GraphError),
    #[error("invalid zone")]
    Zone(#[from] ZoneError),
    #[error("invalid string table")]
    Strings(#[from] StringsError),
    #[error("invalid font or font chains")]
    Font(#[from] FontError),
    #[error("invalid navmesh")]
    NavMesh(#[from] NavMeshError),
    #[error("invalid timeline")]
    Timeline(#[from] TimelineError),
    #[error("invalid UI document")]
    Ui(#[from] UiError),
}
