//! What kind of data an asset is. Packs record it in their table of contents, so the runtime
//! knows how to check, budget and upload an asset before reading it.

use std::fmt;

use sapphire_bytecode::{SCRIPT_EXTENSION, ScriptChunk, ScriptError};
use sapphire_colliders::{COLLIDERS_EXTENSION, ColliderError, ColliderSet};
use sapphire_dialogue::{DIALOGUE_EXTENSION, Dialogue, DialogueError};
use sapphire_moves::{
    ENEMY_EXTENSION, Enemy, EnemyError, MOVE_SET_EXTENSION, MoveSet, MoveSetError, PLAYABLE_EXTENSION,
    Playable, PlayableError,
};
use sapphire_navmesh::{NAVMESH_EXTENSION, NavMesh, NavMeshError};
use sapphire_serialize::cooked::AlignedVec;
use sapphire_strings::{STRINGS_EXTENSION, StringTable, StringsError};
use sapphire_timeline::{TIMELINE_EXTENSION, Timeline, TimelineError};
use sapphire_zone::{
    AMBIENT_EXTENSION, Ambient, ENCOUNTER_EXTENSION, Encounter, FOLIAGE_EXTENSION, Foliage,
    SCATTER_EXTENSION, ScatterSet, WATER_MESH_EXTENSION, WaterMesh, ZONE_EXTENSION, Zone, ZoneError,
};
use serde::{Deserialize, Serialize};

use crate::AssetId;
use crate::clip::{CLIP_EXTENSION, Clip, ClipError};
use crate::cue::{CUE_EXTENSION, Cue, CueError};
use crate::effect_sheet::{EffectSheet, EffectSheetError, SHEET_EXTENSION};
use crate::effects::{EFFECTS_EXTENSION, Effects, EffectsError};
use crate::font::{FONT_CHAINS_EXTENSION, FONT_EXTENSION, Font, FontChains, FontError};
use crate::graph::{AnimGraph, GRAPH_EXTENSION, GraphError};
use crate::material::{MATERIAL_EXTENSION, Material, MaterialError};
use crate::mesh::{MESH_EXTENSION, Mesh, MeshError};
use crate::mix::{MIX_EXTENSION, Mix, MixError};
use crate::motion::{MOTION_EXTENSION, Motion, MotionError};
use crate::probes::{PROBE_GRID_EXTENSION, ProbeGrid, ProbeGridError};
use crate::reflections::{REFLECTIONS_EXTENSION, ReflectionProbes, ReflectionProbesError};
use crate::rig::{CharacterRig, RIG_EXTENSION, RigError};
use crate::score::{SCORE_EXTENSION, Score, ScoreError};
use crate::skeleton::{SKELETON_EXTENSION, Skeleton, SkeletonError};
use crate::sound::{SOUND_EXTENSION, Sound, SoundError};
use crate::texture::{TEXTURE_EXTENSION, Texture, TextureFormatError};
use crate::texture_level::{TEXTURE_LEVEL_EXTENSION, TextureLevel, TextureLevelError};
use crate::ui::{UI_EXTENSION, UiDocument, UiError};
use crate::voice::{VOICE_LINES_EXTENSION, VoiceLines, VoiceLinesError};

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
    /// A script's bytecode (`.sscript`, M3 L2): [`ScriptChunk`], which the simulation's VM runs and
    /// nothing uploads.
    Script = 21,
    /// A conversation (`.sdialogue`, M3 Q2): [`Dialogue`], which a game walks and the simulation takes
    /// effects from. It names nothing: lines are keys, and voice lines cues by name.
    Dialogue = 22,
    /// One kind of enemy (`.senemy`, M3 B3): [`Enemy`], its brains and phases for the simulation beside its
    /// move set, which it depends on, and its look for the stage. CPU data, never uploaded.
    Enemy = 23,
    /// A score (`.sscore`, M3 A1): [`Score`], naming the stems it plays.
    Score = 24,
    /// One language's voice lines (`.svoice`, M3 A1): [`VoiceLines`], naming the cues they play.
    VoiceLines = 25,
    /// One level of a texture above its tail (`.smip`, M5 M3): [`TextureLevel`], depending on its
    /// tail, the texture's own asset.
    TextureLevel = 26,
    /// A zone's reflection probes (`.sreflect`, M5 L2): [`ReflectionProbes`], their cubes per time-of-day
    /// key, uploaded to the renderer.
    ReflectionProbes = 27,
    /// One playable kind (`.splayable`, M5 C8): [`Playable`], what the stage draws it by. Its set of moves
    /// is compiled into the simulation, so it depends on nothing. CPU data, never uploaded.
    Playable = 28,
    /// A character's motion (`.smotion`, M5 C4): [`Motion`], the springs' tuning, the IK's weights and the
    /// cloth a playable kind names when it moves by numbers of its own. It depends on nothing. CPU data, never
    /// uploaded.
    Motion = 29,
    /// A zone's ambient characters (`.sambient`, M5 C10): [`Ambient`], its posts and walks and who stands and
    /// walks there, the zone's own sub-asset beside its cells. Its cast and graph are the stage's to load, so it
    /// depends on nothing, but they ship with it ([`Self::reach`]). CPU data, never uploaded.
    Ambient = 30,
    /// A game's mix (`.smix`, M6 U2): [`Mix`], its buses, reverbs, snapshots, ducking and output profiles. It
    /// names buses by path and sounds not at all, so it depends on nothing. CPU data, never uploaded.
    Mix = 31,
    /// An effects library (`.seffects`, M6 N1): [`Effects`], `sapphire_vfx`'s library as its document, what a
    /// game's cues and sword trails play. It depends on the painted sheets its emitters name (M6 N4a). CPU
    /// data, never uploaded.
    Effects = 32,
    /// A painted effect sheet (`.ssheet`, M6 N4a): [`EffectSheet`], a flipbook's frames as layers of the
    /// effects atlas. It depends on nothing. Its library's atlas is built from its bytes, so it is never
    /// uploaded by itself and keeps them.
    EffectSheet = 33,
    /// A foliage document's kinds (`.sfoliage`, M6 G1a): [`Foliage`], what each kind of a zone's scatter sets
    /// draws. Its meshes ship with it ([`Self::reach`]) and are the renderer's to load, so it depends on nothing.
    /// CPU data, never uploaded.
    Foliage = 34,
    /// A cell's plants (`.sscatter`, M6 G1a): [`ScatterSet`], 16-byte instances by kind and chunk with their
    /// stable IDs, a zone's sub-asset its presentation names. It names its kinds by their index in the zone's
    /// foliage, so it depends on nothing. CPU data, never uploaded by the server: the renderer takes its
    /// instances (G2a).
    Scatter = 35,
    /// A lake's or a river's drawn surface (`.swater`, M6 W3a): [`WaterMesh`], its mesh and its info texture, a
    /// zone's sub-asset its presentation names. It depends on nothing. CPU data, never uploaded by the server: the
    /// renderer takes its vertices and texels (W3b).
    WaterMesh = 36,
}

impl AssetKind {
    /// Every kind, in a stable order.
    pub const ALL: [Self; 36] = [
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
        Self::Script,
        Self::Dialogue,
        Self::Enemy,
        Self::Score,
        Self::VoiceLines,
        Self::TextureLevel,
        Self::ReflectionProbes,
        Self::Playable,
        Self::Motion,
        Self::Ambient,
        Self::Mix,
        Self::Effects,
        Self::EffectSheet,
        Self::Foliage,
        Self::Scatter,
        Self::WaterMesh,
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
            Self::Script => SCRIPT_EXTENSION,
            Self::Dialogue => DIALOGUE_EXTENSION,
            Self::Enemy => ENEMY_EXTENSION,
            Self::Score => SCORE_EXTENSION,
            Self::VoiceLines => VOICE_LINES_EXTENSION,
            Self::TextureLevel => TEXTURE_LEVEL_EXTENSION,
            Self::ReflectionProbes => REFLECTIONS_EXTENSION,
            Self::Playable => PLAYABLE_EXTENSION,
            Self::Motion => MOTION_EXTENSION,
            Self::Ambient => AMBIENT_EXTENSION,
            Self::Mix => MIX_EXTENSION,
            Self::Effects => EFFECTS_EXTENSION,
            Self::EffectSheet => SHEET_EXTENSION,
            Self::Foliage => FOLIAGE_EXTENSION,
            Self::Scatter => SCATTER_EXTENSION,
            Self::WaterMesh => WATER_MESH_EXTENSION,
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
            Self::Script => "script",
            Self::Dialogue => "dialogue",
            Self::Enemy => "enemy",
            Self::Score => "score",
            Self::VoiceLines => "voice lines",
            Self::TextureLevel => "texture level",
            Self::ReflectionProbes => "reflection probes",
            Self::Playable => "playable kind",
            Self::Motion => "character motion",
            Self::Ambient => "ambient characters",
            Self::Mix => "mix",
            Self::Effects => "effects library",
            Self::EffectSheet => "effect sheet",
            Self::Foliage => "foliage",
            Self::Scatter => "scatter set",
            Self::WaterMesh => "water mesh",
        }
    }

    /// Whether assets of this kind go to the GPU, through the asset server's upload queue. A rig,
    /// a skeleton, a clip, a collider set, a sound, a cue, a move set, an animation graph, a zone, a
    /// string table, a font, font chains, a navmesh, a timeline, a UI document, an encounter, a script, a
    /// dialogue, an enemy, a score, voice lines, a playable kind, a character motion, a mix, an effects library, an
    /// effect sheet, a foliage's kinds, a scatter set or a water mesh is ready once it and its dependencies are decoded.
    pub const fn uploads(self) -> bool {
        !matches!(
            self,
            Self::Mix
                | Self::Effects
                | Self::EffectSheet
                | Self::Foliage
                | Self::Scatter
                | Self::WaterMesh
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
                | Self::Script
                | Self::Dialogue
                | Self::Enemy
                | Self::Score
                | Self::VoiceLines
                | Self::Playable
                | Self::Motion
                | Self::Ambient
        )
    }

    /// Whether the asset server keeps an asset's decoded bytes once it is ready. Meshes, textures,
    /// texture levels and reflection probes are GPU data, so their bytes go once uploaded (a new renderer
    /// has them read again).
    /// Materials are small and CPU code reads them (live link, the editor). A probe grid is read on the CPU
    /// after its upload: the renderer holds one, so a zone coming next keeps its grid aside until a trip
    /// arrives in it, and a tier's new renderer takes the grid on stage from its bytes (M5 M6). Rigs,
    /// skeletons, clips, collider sets,
    /// sounds, cues, move sets, animation graphs, zones, string tables, fonts, font chains, navmeshes,
    /// timelines, UI documents, encounters, scripts, dialogues, enemies, scores, voice lines, playable
    /// kinds, character motions, mixes, effects libraries, foliages' kinds, scatter sets and water meshes are read on the CPU
    /// only, so they stay, and so does an effect sheet, whose library's atlas is built from its bytes (M6 N4b).
    pub const fn keeps_bytes(self) -> bool {
        matches!(
            self,
            Self::Mix
                | Self::Effects
                | Self::EffectSheet
                | Self::Foliage
                | Self::Scatter
                | Self::WaterMesh
                | Self::Material
                | Self::ProbeGrid
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
                | Self::Script
                | Self::Dialogue
                | Self::Enemy
                | Self::Score
                | Self::VoiceLines
                | Self::Playable
                | Self::Motion
                | Self::Ambient
        )
    }

    /// Whether uploading an asset of this kind binds its dependencies' GPU objects, so it must be
    /// sent again when hot reload replaces one: a material binds its textures. A mesh names its
    /// materials by the renderer's handles, which a material reloaded in place keeps. Such kinds
    /// keep their bytes, which they are sent again from.
    pub const fn binds_dependencies(self) -> bool {
        matches!(self, Self::Material)
    }

    /// Whether an asset of this kind may depend on one of `kind`: meshes on materials and skeletons,
    /// materials on textures, texture levels on their textures, clips on skeletons, cues and scores on sounds, voice lines on cues, animation
    /// graphs on clips, zones on collider sets, navmeshes and encounters, font chains on fonts, enemies on
    /// move sets, effects libraries on sheets, and textures, rigs, probe grids, skeletons, collider sets, sounds, move sets, string
    /// tables, fonts, navmeshes, timelines, UI documents, encounters, scripts, dialogues, reflection
    /// probes, playable kinds, character motions, ambient characters, sheets, foliages, scatter sets and water meshes on nothing. A zone's meshes and probes and an enemy's or a playable kind's
    /// look aren't dependencies ([`Self::reach`]). The allowed pairs form no loop, so chains are at most three long and
    /// cycles can't exist, whatever a pack claims. A new kind takes its place in this order.
    pub const fn may_depend_on(self, kind: AssetKind) -> bool {
        matches!(
            (self, kind),
            (Self::Mesh, Self::Material | Self::Skeleton)
                | (Self::Material, Self::Texture)
                | (Self::TextureLevel, Self::Texture)
                | (Self::Clip, Self::Skeleton)
                | (Self::Cue | Self::Score, Self::Sound)
                | (Self::VoiceLines, Self::Cue)
                | (Self::AnimGraph, Self::Clip)
                | (Self::Zone, Self::Colliders | Self::NavMesh | Self::Encounter)
                | (Self::FontChains, Self::Font)
                | (Self::Enemy, Self::MoveSet)
                | (Self::Effects, Self::EffectSheet)
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
            21 => Some(Self::Script),
            22 => Some(Self::Dialogue),
            23 => Some(Self::Enemy),
            24 => Some(Self::Score),
            25 => Some(Self::VoiceLines),
            26 => Some(Self::TextureLevel),
            27 => Some(Self::ReflectionProbes),
            28 => Some(Self::Playable),
            29 => Some(Self::Motion),
            30 => Some(Self::Ambient),
            31 => Some(Self::Mix),
            32 => Some(Self::Effects),
            33 => Some(Self::EffectSheet),
            34 => Some(Self::Foliage),
            35 => Some(Self::Scatter),
            36 => Some(Self::WaterMesh),
            _ => None,
        }
    }

    /// Cooked bytes of this kind at a version it still reads but no longer writes, as its current
    /// version's; any other bytes as they are. Clips have one (version 1, M6 A1), and zones (version 8, M6
    /// W2). Decode jobs call it before [`Self::references`], so what the runtime reads in place is always
    /// current.
    pub fn upgrade(self, bytes: AlignedVec) -> Result<AlignedVec, AssetError> {
        Ok(match self {
            Self::Clip => Clip::upgrade(&bytes)?.unwrap_or(bytes),
            Self::Zone => Zone::upgrade(&bytes)?.unwrap_or(bytes),
            _ => bytes,
        })
    }

    /// Checks cooked bytes of this kind exactly as the runtime reads them (`Mesh::read`,
    /// `Texture::from_ktx2`, `Material::read`, `CharacterRig::read`, `ProbeGrid::read`, `Skeleton::read`,
    /// `Clip::read`, `ColliderSet::read`, `Sound::read`, `Cue::read`, `MoveSet::from_cooked`,
    /// `AnimGraph::read`, `Zone::read`, `StringTable::read`, `Font::read`, `FontChains::read`,
    /// `NavMesh::read`, `Timeline::read`, `UiDocument::read`, `Encounter::read`, `ScriptChunk::read`,
    /// `Dialogue::read`, `Enemy::from_cooked`, `Score::read`, `VoiceLines::read`, `TextureLevel::read`,
    /// `ReflectionProbes::read`, `Playable::from_cooked`, `Motion::read`,
    /// `Ambient::read`, `Mix::read`, `Effects::read`, `EffectSheet::read`, `Foliage::read`, `ScatterSet::read`,
    /// `WaterMesh::read`), and
    /// returns the assets they reference, sorted and without repeats: a mesh's material slots bound to material
    /// assets and its skeleton, a material's textures, a texture level's texture, a clip's skeleton, a cue's sounds, a score's stems' sounds, voice
    /// lines' cues, a graph's clips, what the simulation reads of a zone, font chains' fonts, an enemy's move
    /// set, an effects library's sheets. The packer records these as the asset's dependencies, and decode jobs check that a decoded asset
    /// references exactly what its pack says it does. Bytes of a version a kind still reads but no
    /// longer writes go through [`Self::upgrade`] first.
    ///
    /// Everything but textures must start 16-byte aligned, as decoded assets do.
    pub fn references(self, bytes: &[u8]) -> Result<Vec<AssetId>, AssetError> {
        sapphire_core::frame_loads::count(sapphire_core::frame_loads::LoadWork::Build);
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
            Self::Cue => Cue::read(bytes)?.to_owned_cue().sounds(),
            Self::Score => Score::read(bytes)?.stems.iter().map(|stem| stem.sound).collect(),
            Self::VoiceLines => VoiceLines::read(bytes)?.lines.iter().map(|line| line.cue).collect(),
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
            // A script's names are its own to look up in a set of moves: it holds no asset.
            Self::Script => {
                ScriptChunk::read(bytes)?;
                Vec::new()
            }
            // Its lines are keys and its voice lines cues by name: it holds no asset.
            Self::Dialogue => {
                Dialogue::read(bytes)?;
                Vec::new()
            }
            // Its brains press its set's intents, so the set comes first. Its look is the stage's to load.
            Self::Enemy => vec![Enemy::from_cooked(bytes)?.moves],
            // A level is written into its texture, which comes first.
            Self::TextureLevel => vec![TextureLevel::read(bytes)?.texture()],
            // Probes are baked from a zone's meshes and materials, but read none of them.
            Self::ReflectionProbes => {
                ReflectionProbes::read(bytes)?;
                Vec::new()
            }
            // Its set of moves is compiled in, and its look is the stage's to load.
            Self::Playable => {
                Playable::from_cooked(bytes)?;
                Vec::new()
            }
            // Its cloth names a rig's chains by name, and the kind naming it says which rig.
            Self::Motion => {
                Motion::read(bytes)?;
                Vec::new()
            }
            // Its cast's meshes and its graph are the stage's to load as the zone opens.
            Self::Ambient => {
                Ambient::read(bytes)?;
                Vec::new()
            }
            // Its buses are paths of its own, and it names no sound.
            Self::Mix => {
                Mix::read(bytes)?;
                Vec::new()
            }
            // Its effects are named by name, and its shapes and patterns are generated; its painted sheets are
            // assets, which its atlas is built from.
            Self::Effects => Effects::read(bytes)?.sheets().to_vec(),
            // A sheet holds its layers itself.
            Self::EffectSheet => {
                EffectSheet::read(bytes)?;
                Vec::new()
            }
            // Its kinds' meshes are the renderer's to load with the zone's scatter, so they are reach, not
            // dependencies.
            Self::Foliage => {
                Foliage::read(bytes)?;
                Vec::new()
            }
            // It names its kinds by their index in the zone's foliage.
            Self::Scatter => {
                ScatterSet::read(bytes)?;
                Vec::new()
            }
            // Its body's material is the zone's to name.
            Self::WaterMesh => {
                WaterMesh::read(bytes)?;
                Vec::new()
            }
        };
        references.sort_unstable();
        references.dedup();
        Ok(references)
    }

    /// [`Self::references`], and what an asset names without holding: a zone's cell and instance meshes
    /// and its probe grid, an enemy's meshes and graph, a playable kind's meshes, rig, graph and motion, a
    /// zone's ambient characters' cast meshes and graph, and a foliage's kinds' meshes, which
    /// loading it doesn't load, so they aren't
    /// dependencies, but which must ship with it. This is what decides which pack an asset goes in.
    pub fn reach(self, bytes: &[u8]) -> Result<Vec<AssetId>, AssetError> {
        let mut reach = self.references(bytes)?;
        match self {
            Self::Zone => reach.extend(Zone::read(bytes)?.presentation_references()),
            Self::Enemy => {
                let look = Enemy::from_cooked(bytes)?.look;
                reach.extend([look.mesh, look.graph].into_iter().chain(look.held.map(|held| held.mesh)));
            }
            Self::Playable => reach.extend(Playable::from_cooked(bytes)?.named()),
            Self::Ambient => reach.extend(Ambient::read(bytes)?.named()),
            Self::Foliage => reach.extend(Foliage::read(bytes)?.meshes()),
            _ => {}
        }
        reach.sort_unstable();
        reach.dedup();
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
    #[error("invalid score")]
    Score(#[from] ScoreError),
    #[error("invalid voice lines")]
    VoiceLines(#[from] VoiceLinesError),
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
    #[error("invalid script")]
    Script(#[from] ScriptError),
    #[error("invalid dialogue")]
    Dialogue(#[from] DialogueError),
    #[error("invalid enemy")]
    Enemy(#[from] EnemyError),
    #[error("invalid texture level")]
    TextureLevel(#[from] TextureLevelError),
    #[error("invalid reflection probes")]
    ReflectionProbes(#[from] ReflectionProbesError),
    #[error("invalid playable kind")]
    Playable(#[from] PlayableError),
    #[error("invalid character motion")]
    Motion(#[from] MotionError),
    #[error("invalid mix")]
    Mix(#[from] MixError),
    #[error("invalid effects library")]
    Effects(#[from] EffectsError),
    #[error("invalid effect sheet")]
    EffectSheet(#[from] EffectSheetError),
}
