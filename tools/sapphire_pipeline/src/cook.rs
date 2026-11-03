//! Cooking: source assets and their sidecars in, the runtime's cooked files out, through the
//! content cache (spec §5.27).
//!
//! [`cook_dir`] cooks a content tree:
//!
//! 1. Finds the source assets, leaving out the images that map documents read (they are
//!    part of those assets), and resolves their sidecars ([`meta`]), creating missing ones.
//!    Sources whose sidecars say `cook: false` go no further: the report lists them. A map
//!    document that doesn't cook isn't checked, but still leaves out the images it names, so
//!    they don't become textures of their own; if its document doesn't parse, nothing says
//!    which images those are.
//! 2. Plans each asset's output: its path under the content root, mirrored under the output
//!    directory with the cooked format's extension (`samples/mannequin.glb` →
//!    `samples/mannequin.smesh`). Tools read cooked files by these paths; the runtime reads
//!    packs ([`crate::pack`], the stage after this one) and addresses assets by ID. Two assets
//!    may not cook to one path, nor to paths that differ only in case (`tex.png` and `TEX.tga`),
//!    which are one file on macOS's and Windows' default file systems: case is folded with
//!    Unicode's lowercase mapping, and Unicode normalization is left out. The cook fails, naming
//!    both sources, before it changes any output.
//! 3. Removes stale files from the output directory: cooked files no source produces any
//!    more, and temporary files from an interrupted run. Other files are left alone.
//! 4. Keys each asset ([`CacheKey`]) and takes its output from the [`Cache`], or imports,
//!    bakes, validates and cooks it and stores the result. An output is written only when its
//!    bytes change, so a file watcher (hot reload, A8) sees exactly the assets that changed.
//!    Materials read the cooked textures they reference, so they cook after every other asset
//!    but probe grids, which read cooked meshes and materials; their keys depend on those
//!    assets' keys.
//!
//! Each kind of source cooks through a [`Recipe`]: the importer, the cooker and the cooked
//! format, whose versions are part of the cache key:
//!
//! - **Meshes** cook glTF and FBX → `.smesh` (A1, A5, A10): import, outline bake,
//!   [optimization and LODs](crate::optimize), packing, then a read-back of the cooked bytes
//!   through the runtime's own checks. The two cooks differ in the LOD chain: the mobile cook
//!   drops its first levels, for a lower-detail LOD0 and fewer LODs (spec §5.27).
//! - **Textures** cook PNG, TGA and EXR → `.ktx2` (A2), BCn on desktop and ASTC on mobile
//!   ([`texture`]).
//! - **Anime character maps** cook `.facesdf.ron`, `.ramp.ron` and `.ilm.ron` documents →
//!   `.ktx2` (A7): the map a document describes is generated, then cooked as its texture
//!   class ([`character_maps`]).
//! - **Materials** cook `.material.ron` documents → `.smat` (A3), checked against the textures
//!   they reference ([`material`]). A mesh's slots embed the materials the importer generated,
//!   or name the material assets its sidecar binds them to, which must be materials of the same
//!   tree.
//! - **VRM characters** cook `.vrm` → `.smesh` like glTF meshes, plus a `.srig` rig (A6,
//!   [`vrm`]).
//! - **Ambient probe grids** cook `.probes.ron` documents → `.sprobe` (W5, [`probes`]): a bake of
//!   the meshes the document names, lit by their materials under time of day's keys. They read
//!   cooked meshes and materials' sources, so they cook last; their keys depend on those assets'
//!   keys.
//! - **Assemblies** cook `.assembly.ron` documents → `.smesh` like glTF meshes: kit pieces
//!   placed into one mesh in slots of the document's own (M1 X1, [`assembly`]). The pieces'
//!   files are dependencies of the key.
//! - **Skeletons** cook beside every skinned mesh → `.sskel` (M2 N1): the import's joints, which
//!   the mesh's skin names by a derived ID, as a clip does. A mesh or an animation source may share
//!   another source's instead (`skeleton:`, `rig:`).
//! - **Clips** cook a glTF's or an FBX's animations and a VRMA's → `.sclip` when its sidecar asks
//!   (M2 N1, [`clip`]), one per animation chosen (`run.glb` → `run.Walk.sclip`) and one more per rig
//!   it retargets to, its target's ID in the file name (`run.Walk.6f8a81b6.sclip`). Sources wanted
//!   for their animations alone cook no asset of their own.
//! - **Key-pose documents** cook `.keypose.ron` → `.sclip` (M2 N1, [`keypose`]), for the rigs
//!   their sidecar names.
//! - Key-pose documents, every clip and a mesh sharing a skeleton read skeletons and rigs that
//!   other sources cook, so they cook after every other asset but materials and probe grids; their
//!   keys depend on those assets' keys. Such a mesh cooks before the clips, since a key-pose document
//!   for its rig reads its morph targets.
//! - **Collider documents** cook `.colliders.ron` → `.scol` (M2 D3, [`colliders`]): a zone's static
//!   collision, checked entry by entry, reading nothing besides themselves.
//! - **Sounds and cues** cook `.ogg` → `.ssound` and `.cue.ron` documents → `.scue` (M2 E1,
//!   [`audio`]). A cue holds its sounds' IDs, so it reads none of their contents, but they must be
//!   sounds of the tree.
//! - **Fonts** cook `.ttf` and `.otf` → an `.sfont` for each pack that shows them, and `.fontchains.ron`
//!   documents → `.sfontc` (M3 U3, [`font`]). A font reads its license and character sets; chains hold
//!   IDs, so they read no font, but they must be fonts of the tree.
//! - **Move sets** cook `.moves.ron` documents → `.smoves` (M2 G2, [`moves`]): frame data and tables
//!   the simulation reads, checked by move and track. A move that names a clip reads what that clip's
//!   own cook read, its source and its rig (M2 N1), so a set cooks after every clip, and its key depends
//!   on their keys.
//! - **Zones** cook `.zone.ron` documents → `.szone` and a `.smesh` per cell (M3 W1, [`zone`]). Their
//!   cells' meshes merge prefabs, assemblies by their sidecars' IDs, and are keyed by what's in each, so
//!   a save to one placed piece recooks the cells it's in. A `.szone` reads its meshes' bounds, so it
//!   cooks last of all; its key depends on those assets' keys.
//! - **Localized strings** cook `.ftl` sources and `.locale.ron` documents → `.sloc`, one table per
//!   locale (M3 V2, [`loc`]), reading nothing another asset cooks. After every asset has cooked, the
//!   tables are checked against each other, whatever came from the cache, and the report lists what's
//!   untranslated.
//!
//! A source may cook to more than one asset: its own, and sub-assets, each with its own recipe,
//! output beside the source's (`alicia.smesh`, `alicia.sskel`, `alicia.srig`) and an ID derived
//! from the source's ([`meta::sub_asset_id`]). Each is planned, keyed and cached as an asset of its
//! own, so a report lists it and a pack (A4) can address it; each imports the source itself, but
//! for clips, which share one import (`clip::source`). Which ones a glTF or an FBX has depends on
//! the file (is it skinned? what animations?), so planning reads those.
//!
//! # Threads
//!
//! Assets that read nothing another asset cooks (textures, maps, meshes of their own, a zone's cells,
//! sounds, and the documents that only name others) cook first, on up to four threads (`COOK_WORKERS`),
//! a source's parts together but for a zone's cells, each of which is a whole mesh's work; the rest cook
//! after them, one after another, in the order above. To those first assets nothing has cooked, so one
//! that came to read another's output would fail the same way every time rather than race it. Their
//! keys all come first, so a key's first asset in plan order is the one that cooks, and the others take
//! its output from the cache, as on one thread. Neither a cooked file nor the report depends on the
//! worker count, and a cook fails with the first failure in plan order. A key can come seconds before
//! its cook, so a file saved in between, the source or anything else its key covers, fails the cook
//! (`SourceChanged`) rather than store another version's output under that key. So does a save after
//! the plan read a source, since what it took from the file holds for that version alone, and a save of
//! a clip's source after the clip cooked and before a move set bakes from it.
//!
//! Texture encoding is what a cold cook spends its time on, 58% of 17.6 s on the M5, in encoders that
//! run a level on one thread. astcenc shares two things: it rewrites its angle tables with the same
//! values at every context, so a reader sees the same value either way, though it's a race as C++
//! counts them; and a pointer to the preset table of a block size, whose value does reach the output,
//! so levels of different tables never encode at once ([`texture`]'s `encode`). Six cold cooks of each
//! target on four workers under load matched one thread's bytes. Medians of six alternating rounds at a
//! load of 13 to 20 take the desktop cook from 25.4 s to 12.2 s and the mobile cook from 12.3 s to 5.6 s,
//! for the same CPU.
//!
//! What it costs is memory: a texture's cook holds several expanded copies of its image, so besides the
//! worker count what they hold at once is limited (`COOK_MEMORY`). Peak resident size goes from about
//! 680 MiB to about 1040 MiB; eight workers took it to 1328 MiB for a tenth less time.

use std::cell::{Cell, RefCell};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::fmt;
use std::fs;
use std::io;
use std::panic::{self, AssertUnwindSafe};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::str::FromStr;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Condvar, Mutex, PoisonError};
use std::thread;

use sapphire_assets::AssetId;
use sapphire_assets::clip::{CLIP_EXTENSION, Clip};
use sapphire_assets::cue::{CUE_EXTENSION, Cue};
use sapphire_assets::font::{FONT_CHAINS_EXTENSION, FONT_EXTENSION, Font, FontChains};
use sapphire_assets::graph::{AnimGraph, GRAPH_EXTENSION};
use sapphire_assets::material::{MATERIAL_EXTENSION, Material};
use sapphire_assets::mesh::{MESH_EXTENSION, Mesh, MeshError};
use sapphire_assets::probes::{PROBE_GRID_EXTENSION, ProbeGrid};
use sapphire_assets::rig::{CharacterRig, HumanBone, RIG_EXTENSION, RigError};
use sapphire_assets::skeleton::{Joint, SKELETON_EXTENSION, Skeleton};
use sapphire_assets::sound::{SOUND_EXTENSION, Sound};
use sapphire_assets::texture::{TEXTURE_EXTENSION, TEXTURE_VERSION};
use sapphire_colliders::{COLLIDERS_EXTENSION, ColliderSet};
use sapphire_moves::{BakedTables, ClipMotionDocument, MOVE_SET_EXTENSION, MoveSet};
use sapphire_reflect::Versioned;
use sapphire_strings::{STRINGS_EXTENSION, StringTable};
use sapphire_zone::{Place, ZONE_EXTENSION, Zone};

use crate::anim_graph::{self, AnimGraphCookError};
use crate::assembly::{self, AssemblyError};
use crate::audio::{self, AudioCookError};
use crate::bake::foliage::FoliageError;
use crate::cache::{Cache, CacheError, CacheKey, Dependency, KeyInputs, Stage};
use crate::character_maps::{self, MapError, MapKind};
use crate::clip::retarget::{self, HumanMap, RetargetOptions, Rig};
use crate::clip::source::{ClipSource, ClipSources, joint_remap, remap_tracks};
use crate::clip::{self, ClipCookError, ClipSettings, ClipTarget, SampledClip};
use crate::colliders::{self, ColliderCookError};
use crate::font::{self, FontCookError};
use crate::import::fbx::{self, FbxImportError};
use crate::import::gltf::{self, GltfImportError};
use crate::import::image;
use crate::import::vrm::{self, VrmImportError, VrmSettings};
use crate::import::vrma;
use crate::keypose::{self, KeyPoseError};
use crate::loc::{self, FluentFile, FluentFiles, LocCheckError, LocCookError, LocReport};
use crate::material::{self, MaterialCookError, Referenced, TextureKind};
use crate::mesh::ImportedMesh;
use crate::mesh::MaterialSlotError;
use crate::meta::{self, ImportSettings, MeshSettings, MetaError, SourceAsset, SourceTree};
use crate::moves::{self, BakeError, MoveSetCookError, TableInput};
use crate::probes::{self, ProbeCookError, ProbeInput};
use crate::texture::{self, TextureError};
use crate::zone::{self, CookedPlace, ZoneCookError, ZonePlan};
use crate::{bake, optimize};

/// A target cook (spec §2): `desktop` for macOS, Windows and Linux, `mobile` for iOS and
/// Android.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Cook {
    Desktop,
    Mobile,
}

impl Cook {
    /// Every cook, in a stable order.
    pub const ALL: [Self; 2] = [Self::Desktop, Self::Mobile];

    /// The name used on command lines, in output directories and in cache keys.
    pub fn name(self) -> &'static str {
        match self {
            Self::Desktop => "desktop",
            Self::Mobile => "mobile",
        }
    }
}

impl fmt::Display for Cook {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

/// Returned when parsing an unknown cook name.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
#[error("unknown cook {0:?}; expected `desktop` or `mobile`")]
pub struct UnknownCook(pub String);

impl FromStr for Cook {
    type Err = UnknownCook;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::ALL.into_iter().find(|cook| cook.name() == s).ok_or_else(|| UnknownCook(s.to_owned()))
    }
}

/// How one kind of source is cooked: the stages that run, and the file they write.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Recipe {
    pub importer: Stage,
    pub cooker: Stage,
    /// The cooked format's own version, so a format change recooks its assets even without a
    /// stage bump.
    pub format: u32,
    /// The cooked file's extension.
    pub extension: &'static str,
}

/// Bake, optimization and LODs, packing and the `.smesh` writer. Bump its version whenever
/// they change output for the same imported mesh. 3: deformation-aware LODs (the review's
/// AP-005): what morph targets move is locked, and skinned meshes simplify in test poses too.
/// 4: a shared skeleton's repeated joint name is an error, where the palette aliased two joints (the
/// review's IMP-001).
pub const MESH_COOKER: Stage = Stage { name: "smesh", version: 4 };

/// Every extension a recipe writes: stale files with these are removed from the output.
pub(crate) const COOKED_EXTENSIONS: &[&str] = &[
    MESH_EXTENSION,
    TEXTURE_EXTENSION,
    MATERIAL_EXTENSION,
    RIG_EXTENSION,
    PROBE_GRID_EXTENSION,
    SKELETON_EXTENSION,
    CLIP_EXTENSION,
    COLLIDERS_EXTENSION,
    SOUND_EXTENSION,
    CUE_EXTENSION,
    FONT_EXTENSION,
    FONT_CHAINS_EXTENSION,
    MOVE_SET_EXTENSION,
    ZONE_EXTENSION,
    STRINGS_EXTENSION,
];

/// What a source's skeleton's ID is derived from.
pub const SKELETON: &str = "skeleton";

/// An asset every source of a kind cooks to besides its own (see the [module docs](self)).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SubAsset {
    /// Unique among the source's sub-assets, and never changed: the sub-asset's ID is derived
    /// from it.
    pub name: &'static str,
    pub recipe: Recipe,
}

/// Extension of the temporary files outputs are written through.
const TEMP_EXTENSION: &str = "tmp";

impl Recipe {
    /// The recipe for sources read with `settings`.
    pub fn for_settings(settings: &ImportSettings) -> Self {
        match settings {
            ImportSettings::Gltf(_) => Self {
                importer: gltf::IMPORTER,
                cooker: MESH_COOKER,
                format: Mesh::VERSION,
                extension: MESH_EXTENSION,
            },
            ImportSettings::Fbx(_) => Self {
                importer: fbx::IMPORTER,
                cooker: MESH_COOKER,
                format: Mesh::VERSION,
                extension: MESH_EXTENSION,
            },
            ImportSettings::Texture(_) => Self {
                importer: image::IMPORTER,
                cooker: texture::COOKER,
                format: TEXTURE_VERSION,
                extension: TEXTURE_EXTENSION,
            },
            // The importer generates the map as an image, which the texture cooker cooks.
            ImportSettings::CharacterMap(settings) => Self {
                importer: settings.kind().importer(),
                cooker: texture::COOKER,
                format: TEXTURE_VERSION,
                extension: TEXTURE_EXTENSION,
            },
            ImportSettings::Material(_) => Self {
                importer: material::IMPORTER,
                cooker: material::COOKER,
                format: Material::VERSION,
                extension: MATERIAL_EXTENSION,
            },
            ImportSettings::Vrm(_) => Self {
                importer: vrm::IMPORTER,
                cooker: MESH_COOKER,
                format: Mesh::VERSION,
                extension: MESH_EXTENSION,
            },
            ImportSettings::ProbeGrid(_) => Self {
                importer: probes::IMPORTER,
                cooker: probes::COOKER,
                format: ProbeGrid::VERSION,
                extension: PROBE_GRID_EXTENSION,
            },
            ImportSettings::Assembly(_) => Self {
                importer: assembly::IMPORTER,
                cooker: MESH_COOKER,
                format: Mesh::VERSION,
                extension: MESH_EXTENSION,
            },
            ImportSettings::KeyPose(_) => Self {
                importer: keypose::IMPORTER,
                cooker: clip::COOKER,
                format: Clip::VERSION,
                extension: CLIP_EXTENSION,
            },
            ImportSettings::Colliders(_) => Self {
                importer: colliders::IMPORTER,
                cooker: colliders::COOKER,
                format: ColliderSet::VERSION,
                extension: COLLIDERS_EXTENSION,
            },
            ImportSettings::Sound(_) => Self {
                importer: audio::SOUND_IMPORTER,
                cooker: audio::SOUND_COOKER,
                format: Sound::VERSION,
                extension: SOUND_EXTENSION,
            },
            ImportSettings::Cue(_) => Self {
                importer: audio::CUE_IMPORTER,
                cooker: audio::CUE_COOKER,
                format: Cue::VERSION,
                extension: CUE_EXTENSION,
            },
            ImportSettings::Font(_) => Self {
                importer: font::IMPORTER,
                cooker: font::COOKER,
                format: Font::VERSION,
                extension: FONT_EXTENSION,
            },
            ImportSettings::FontChains(_) => Self {
                importer: font::CHAINS_IMPORTER,
                cooker: font::CHAINS_COOKER,
                format: FontChains::VERSION,
                extension: FONT_CHAINS_EXTENSION,
            },
            ImportSettings::MoveSet(_) => Self {
                importer: moves::IMPORTER,
                cooker: moves::COOKER,
                format: MoveSet::VERSION,
                extension: MOVE_SET_EXTENSION,
            },
            ImportSettings::AnimGraph(_) => Self {
                importer: anim_graph::IMPORTER,
                cooker: anim_graph::COOKER,
                format: AnimGraph::VERSION,
                extension: GRAPH_EXTENSION,
            },
            // Fluent sources have no asset of their own: this is their locale's table.
            ImportSettings::Fluent(_) | ImportSettings::Locale(_) => Self {
                importer: loc::IMPORTER,
                cooker: loc::COOKER,
                format: StringTable::VERSION,
                extension: STRINGS_EXTENSION,
            },
            ImportSettings::GltfClips(_) => Self::gltf_clip(),
            ImportSettings::FbxClips(_) => Self::fbx_clip(),
            ImportSettings::Vrma(_) => Self { importer: vrma::IMPORTER, ..Self::gltf_clip() },
            ImportSettings::Zone(_) => Self {
                importer: zone::IMPORTER,
                cooker: zone::COOKER,
                format: Zone::VERSION,
                extension: ZONE_EXTENSION,
            },
        }
    }

    /// The recipe of a skeleton beside a mesh cooked with this recipe.
    fn skeleton(self) -> Self {
        Self {
            importer: self.importer,
            cooker: clip::SKELETON_COOKER,
            format: Skeleton::VERSION,
            extension: SKELETON_EXTENSION,
        }
    }

    /// The recipe of a zone's merged mesh for a place.
    fn zone_cell() -> Self {
        Self {
            importer: zone::CELL_IMPORTER,
            cooker: MESH_COOKER,
            format: Mesh::VERSION,
            extension: MESH_EXTENSION,
        }
    }

    /// The recipe of a clip from a glTF's animation.
    fn gltf_clip() -> Self {
        Self {
            importer: gltf::clips::IMPORTER,
            cooker: clip::COOKER,
            format: Clip::VERSION,
            extension: CLIP_EXTENSION,
        }
    }

    /// The recipe of a clip from an FBX's animation stack.
    fn fbx_clip() -> Self {
        Self { importer: fbx::clips::IMPORTER, ..Self::gltf_clip() }
    }

    /// This recipe of a clip, retargeted onto another rig.
    fn retargeted(self) -> Self {
        Self { cooker: retarget::COOKER, ..self }
    }

    /// Where a source at `relative` cooks to, relative to the output root: its path with the
    /// cooked extension in place of the source's (`samples/mannequin.glb` →
    /// `samples/mannequin.smesh`, `skin.material.ron` → `skin.smat`, `courtyard.probes.ron` →
    /// `courtyard.sprobe`, `arena.colliders.ron` → `arena.scol`, `fighter.moves.ron` → `fighter.smoves`,
    /// `ui.fontchains.ron` → `ui.sfontc`).
    pub fn output_path(&self, relative: &Path) -> PathBuf {
        if material::is_source(relative) {
            material::output_path(relative, self.extension)
        } else if probes::is_source(relative) {
            probes::output_path(relative, self.extension)
        } else if assembly::is_source(relative) {
            assembly::output_path(relative, self.extension)
        } else if keypose::is_source(relative) {
            keypose::output_path(relative, self.extension)
        } else if colliders::is_source(relative) {
            colliders::output_path(relative, self.extension)
        } else if audio::is_cue_source(relative) {
            audio::cue_output_path(relative, self.extension)
        } else if font::is_chains_source(relative) {
            font::chains_output_path(relative, self.extension)
        } else if moves::is_source(relative) {
            moves::output_path(relative, self.extension)
        } else if anim_graph::is_source(relative) {
            anim_graph::output_path(relative, self.extension)
        } else if zone::is_source(relative) {
            zone::output_path(relative, self.extension)
        } else if loc::is_locale_source(relative) {
            loc::output_path(relative, self.extension)
        } else {
            relative.with_extension(self.extension)
        }
    }
}

/// Errors from cooking. Each names the file it concerns.
#[derive(Debug, thiserror::Error)]
pub enum CookError {
    #[error("{} is not a supported mesh source (expected .gltf, .glb, .fbx or .vrm)", path.display())]
    UnsupportedSource { path: PathBuf },
    #[error(transparent)]
    Meta(#[from] MetaError),
    #[error("{} or a file it reads changed during the cook; cook again", path.display())]
    SourceChanged { path: PathBuf },
    #[error("failed to import {}", path.display())]
    Import {
        path: PathBuf,
        #[source]
        source: GltfImportError,
    },
    #[error("failed to import FBX {}", path.display())]
    Fbx {
        path: PathBuf,
        /// Boxed, as VRM errors are.
        #[source]
        source: Box<FbxImportError>,
    },
    #[error("failed to assemble {}", path.display())]
    Assembly {
        path: PathBuf,
        /// Boxed: document and glTF errors are large, and every cook result carries this type.
        #[source]
        source: Box<AssemblyError>,
    },
    #[error("failed to import VRM {}", path.display())]
    Vrm {
        path: PathBuf,
        /// Boxed: glTF and JSON errors are large, and every cook result carries this type.
        #[source]
        source: Box<VrmImportError>,
    },
    #[error("failed to cook clip or skeleton from {}", path.display())]
    Clip {
        path: PathBuf,
        /// Boxed, as VRM errors are.
        #[source]
        source: Box<ClipCookError>,
    },
    #[error("cooked rig from {} is invalid", path.display())]
    InvalidRig {
        path: PathBuf,
        #[source]
        source: RigError,
    },
    #[error("failed to cook texture {}", path.display())]
    Texture {
        path: PathBuf,
        /// Boxed: decoder and encoder errors are large, and every cook result carries this type.
        #[source]
        source: Box<TextureError>,
    },
    #[error("failed to cook character map {}", path.display())]
    Map {
        path: PathBuf,
        /// Boxed, as texture errors are.
        #[source]
        source: Box<MapError>,
    },
    #[error("cooked mesh from {} is invalid", path.display())]
    Invalid {
        path: PathBuf,
        #[source]
        source: MeshError,
    },
    #[error("failed to cook material {}", path.display())]
    Material {
        path: PathBuf,
        /// Boxed: document errors are large, and every cook result carries this type.
        #[source]
        source: Box<MaterialCookError>,
    },
    #[error("failed to bake the foliage of {}", path.display())]
    Foliage {
        path: PathBuf,
        #[source]
        source: FoliageError,
    },
    #[error("failed to bake probe grid {}", path.display())]
    ProbeGrid {
        path: PathBuf,
        /// Boxed, as material errors are.
        #[source]
        source: Box<ProbeCookError>,
    },
    #[error("failed to cook collider document {}", path.display())]
    Colliders {
        path: PathBuf,
        /// Boxed, as material errors are.
        #[source]
        source: Box<ColliderCookError>,
    },
    #[error("failed to cook locale {}", path.display())]
    Locale {
        path: PathBuf,
        /// Boxed, as material errors are.
        #[source]
        source: Box<LocCookError>,
    },
    #[error("localized strings can't ship as they are")]
    Localization(#[from] Box<LocCheckError>),
    #[error("failed to cook sound or cue {}", path.display())]
    Audio {
        path: PathBuf,
        /// Boxed, as material errors are.
        #[source]
        source: Box<AudioCookError>,
    },
    #[error("cue {} names {id}, which is not a sound of this content tree that cooks", path.display())]
    NotASound { path: PathBuf, id: AssetId },
    #[error("failed to cook zone {}", path.display())]
    Zone {
        path: PathBuf,
        /// Boxed, as material errors are.
        #[source]
        source: Box<ZoneCookError>,
    },
    #[error("zone {} names {id} in `{field}`, which is not {what} of this content tree that cooks", path.display())]
    ZoneReference { path: PathBuf, field: String, what: &'static str, id: AssetId },
    #[error("failed to cook font or font chains {}", path.display())]
    Font {
        path: PathBuf,
        /// Boxed, as material errors are.
        #[source]
        source: Box<FontCookError>,
    },
    #[error("font chains {} name {id}, which is not a font of this content tree that cooks", path.display())]
    NotAFont { path: PathBuf, id: AssetId },
    #[error("failed to cook move set {}", path.display())]
    MoveSet {
        path: PathBuf,
        /// Boxed, as material errors are.
        #[source]
        source: Box<MoveSetCookError>,
    },
    #[error("failed to cook animation graph {}", path.display())]
    AnimGraph {
        path: PathBuf,
        /// Boxed, as material errors are.
        #[source]
        source: Box<AnimGraphCookError>,
    },
    #[error(
        "animation graph {} names {id} (`{name}`), which is not a clip of this content tree that cooks \
         (on a key-pose document, `target` names a rig of its sidecar's `also`, and no `target` its `rig`)",
        path.display()
    )]
    NotAClip { path: PathBuf, name: String, id: AssetId },
    #[error("failed to resolve the material slots of {}", path.display())]
    MaterialSlot {
        path: PathBuf,
        #[source]
        source: MaterialSlotError,
    },
    #[error(
        "{} binds material slot {slot:?} to {id}, which is not a material document of this content tree",
        path.display()
    )]
    NotAMaterial { path: PathBuf, slot: String, id: AssetId },
    #[error("{} and {} would both cook to {}", first.display(), second.display(), output.display())]
    OutputCollision { first: PathBuf, second: PathBuf, output: PathBuf },
    #[error(
        "{} and {} would cook to {} and {}, which differ only in case: one file where case doesn't \
         count (macOS's and Windows' default file systems), so rename one of the sources",
        first.display(),
        second.display(),
        outputs[0].display(),
        outputs[1].display()
    )]
    CaseCollision { first: PathBuf, second: PathBuf, outputs: [PathBuf; 2] },
    #[error("cooking {} panicked: {message}", path.display())]
    Panicked { path: PathBuf, message: String },
    #[error("failed to read {}", path.display())]
    Read {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("failed to write {}", path.display())]
    Write {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error(transparent)]
    Cache(#[from] CacheError),
}

/// Options for [`cook_dir`].
#[derive(Clone, Copy, Debug)]
pub struct CookOptions<'a> {
    pub cook: Cook,
    /// The content cache's directory. Without one, every asset is cooked.
    pub cache: Option<&'a Path>,
    /// Fail on a source without a sidecar instead of creating one (CI; see [`meta`]).
    pub locked: bool,
}

/// What a cook did with one asset.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CookStatus {
    /// Cooked from its source: a cache miss, or no cache.
    Cooked,
    /// Taken from the cache and written to its output.
    Cached,
    /// The output already held the cached bytes, so nothing was written.
    UpToDate,
}

/// One asset of a [`CookReport`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CookedAsset {
    pub id: AssetId,
    pub source: PathBuf,
    pub output: PathBuf,
    pub key: CacheKey,
    /// blake3 of the cooked file's bytes, as written: what the packer keys its encoding by.
    pub output_hash: blake3::Hash,
    pub status: CookStatus,
}

/// What [`cook_dir`] did.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CookReport {
    /// Every asset, in source path order.
    pub assets: Vec<CookedAsset>,
    /// Sidecars this run created, to commit with their sources.
    pub created_sidecars: Vec<PathBuf>,
    /// Sidecars whose source doesn't exist.
    pub orphaned_sidecars: Vec<PathBuf>,
    /// Sources whose sidecars keep them out of the cook (`cook: false`, see [`meta`]).
    pub uncooked: Vec<PathBuf>,
    /// Stale files removed from the output directory.
    pub removed: Vec<PathBuf>,
    /// What's untranslated, and what's translated from nothing ([`loc::check()`]).
    pub localization: LocReport,
}

impl CookReport {
    /// How many assets ended with `status`.
    pub fn count(&self, status: CookStatus) -> usize {
        self.assets.iter().filter(|asset| asset.status == status).count()
    }
}

/// Assets cooking at once, at most: as many as the packer's encoders ([`crate::pack`]), which leaves
/// the host usable during a cook.
const COOK_WORKERS: usize = 4;

/// A worker's stack: what a main thread has on every desktop this runs on (Windows' is set by
/// `.cargo/config.toml`), since importers that ran on one now run here.
const WORKER_STACK: usize = 8 << 20;

/// What the assets cooking at once may hold, as far as they can say beforehand: past four of the largest
/// committed textures (2048 × 2048, 411 MiB each), so only a source near
/// [`image::SOURCE_BUDGET`], whose cook holds several GiB, or a mesh source of some MiB
/// ([`MESH_MEMORY_PER_SOURCE_BYTE`]), makes the others wait.
const COOK_MEMORY: u64 = 4 << 30;

/// What a map document's cook is taken to hold at least, for a document whose inputs don't say, as one of
/// constants doesn't: its images, as large as the largest committed, and the texture they make.
const MAP_MEMORY: u64 = 1 << 30;

/// What a mesh's cook is taken to hold for each byte it reads, at the top of what was measured, so most
/// hold far less and a source of 16 MiB or more runs alone. Peaks per byte: 11 to 16 for a glTF with
/// normals, 59 to 88 for one the importer must make them for, 5 to 44 for FBX by how it's compressed, 5
/// for a VRM with its textures, 175 for the town's assembly, whose shapes are built from numbers, and
/// 234 for a glTF placing one mesh 16 times. Bytes can't see further: more instances of a mesh, or a
/// document of more shapes, hold more than this says.
const MESH_MEMORY_PER_SOURCE_BYTE: u64 = 256;

/// Memory the assets cooking at once share.
struct CookMemory {
    limit: u64,
    free: Mutex<u64>,
    freed: Condvar,
}

impl CookMemory {
    fn new(limit: u64) -> Self {
        Self { limit, free: Mutex::new(limit), freed: Condvar::new() }
    }

    /// Waits until `bytes` are free and holds them. A cook needing more than the limit waits for all of
    /// it, so it runs alone. Nothing holding some waits for more, so every wait ends.
    fn hold(&self, bytes: u64) -> HeldMemory<'_> {
        let bytes = bytes.min(self.limit);
        let mut free = self.free.lock().unwrap_or_else(PoisonError::into_inner);
        while *free < bytes {
            free = self.freed.wait(free).unwrap_or_else(PoisonError::into_inner);
        }
        *free -= bytes;
        HeldMemory { memory: self, bytes }
    }
}

/// Memory held until this drops, a panicking cook's included.
struct HeldMemory<'a> {
    memory: &'a CookMemory,
    bytes: u64,
}

impl Drop for HeldMemory<'_> {
    fn drop(&mut self) {
        *self.memory.free.lock().unwrap_or_else(PoisonError::into_inner) += self.bytes;
        self.memory.freed.notify_all();
    }
}

/// Cooks every source asset under `source_root` into `out_root`, as the [module docs](self)
/// describe.
pub fn cook_dir(
    source_root: &Path,
    out_root: &Path,
    options: &CookOptions<'_>,
) -> Result<CookReport, CookError> {
    let workers = thread::available_parallelism().map_or(1, usize::from).min(COOK_WORKERS);
    cook_dir_on(source_root, out_root, options, workers)
}

/// [`cook_dir`], on up to `workers` threads.
fn cook_dir_on(
    source_root: &Path,
    out_root: &Path,
    options: &CookOptions<'_>,
    workers: usize,
) -> Result<CookReport, CookError> {
    cook_dir_through(source_root, out_root, options, workers, &|_| {})
}

/// Where a cook has got to, as far as a file saved meanwhile can tell.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CookStage {
    /// Which assets there are is decided, from the files as they stood, and none has its key yet.
    Planned,
    /// Clips have cooked, and the move sets that read their sources haven't.
    ClipsCooked,
}

/// [`cook_dir_on`], calling `reached` at each [`CookStage`], which is where a test saves a file.
fn cook_dir_through(
    source_root: &Path,
    out_root: &Path,
    options: &CookOptions<'_>,
    workers: usize,
    reached: &dyn Fn(CookStage),
) -> Result<CookReport, CookError> {
    let mut tree = SourceTree::scan(source_root)?;
    let inputs = map_inputs(source_root, &tree)?;
    tree.sources.retain(|source| !inputs.contains(source));
    let resolved = meta::resolve(&tree, options.locked)?;
    let (cooked_assets, uncooked): (Vec<SourceAsset>, Vec<SourceAsset>) =
        resolved.assets.into_iter().partition(|asset| asset.meta.cook);
    let prefabs = zone::prefab_sources(cooked_assets.iter().chain(&uncooked));
    let mut zones: HashMap<PathBuf, ZonePlan> = HashMap::new();
    let cache = options.cache.map(Cache::new);

    let mut outputs: HashMap<PathBuf, PathBuf> = HashMap::new();
    // Each output's path folded to lowercase, with its source and output: one file where case
    // doesn't count.
    let mut folded: HashMap<String, (PathBuf, PathBuf)> = HashMap::new();
    let mut claim = |source: &Path, relative: &Path| -> Result<PathBuf, CookError> {
        let output = out_root.join(relative);
        if let Some(first) = outputs.insert(output.clone(), source.to_owned()) {
            return Err(CookError::OutputCollision { first, second: source.to_owned(), output });
        }
        let key = relative.to_string_lossy().to_lowercase();
        if let Some((first, first_output)) = folded.insert(key, (source.to_owned(), output.clone())) {
            return Err(CookError::CaseCollision {
                first,
                second: source.to_owned(),
                outputs: [first_output, output],
            });
        }
        Ok(output)
    };
    let relative_of = |asset: &SourceAsset| -> PathBuf {
        asset.path.strip_prefix(source_root).expect("scanned under the source root").to_owned()
    };
    let fluent = fluent_files(source_root, &cooked_assets)?;
    // Every source's own output first, so a collision says so before any file is read. A source
    // wanted for its animations has none, nor has a Fluent source.
    let mut own_outputs = Vec::with_capacity(cooked_assets.len());
    for asset in &cooked_assets {
        let recipe = Recipe::for_settings(&asset.meta.importer);
        own_outputs.push(if asset.meta.importer.has_own_asset() {
            Some(claim(&asset.path, &recipe.output_path(&relative_of(asset)))?)
        } else {
            None
        });
    }
    let mut plan = Vec::with_capacity(cooked_assets.len());
    for (asset, output) in cooked_assets.into_iter().zip(own_outputs) {
        // Which sub-assets there are may take reading the file, which could panic in a crate.
        let planned_from = Cell::new(None);
        let parts =
            guarded(&asset.path, || sub_assets(&asset, &planned_from, &prefabs, &mut zones, cache.as_ref()))?;
        let planned_from = planned_from.get();
        let skeleton =
            asset.meta.importer.shared_rig().map(|owner| meta::sub_asset_id(owner, SKELETON)).or_else(|| {
                parts
                    .iter()
                    .any(|sub| sub.part == Part::Skeleton)
                    .then(|| meta::sub_asset_id(asset.meta.id, SKELETON))
            });
        let recipe = Recipe::for_settings(&asset.meta.importer);
        let source_relative = relative_of(&asset);
        if let Some(output) = output {
            plan.push(Planned {
                asset: asset.clone(),
                id: asset.meta.id,
                part: Part::Own,
                planned_from,
                skeleton,
                recipe,
                output,
            });
        }
        for PlannedPart { id, part, recipe } in parts {
            let relative = match &part {
                Part::Clip { file, .. } => {
                    let file = format!("{}.{file}.{}", file_stem(&source_relative), recipe.extension);
                    source_relative.with_file_name(file)
                }
                Part::KeyPose { file, .. } => {
                    let file = format!("{}.{file}.{}", keypose::stem(&source_relative), recipe.extension);
                    source_relative.with_file_name(file)
                }
                Part::Cell(place) => zone::part_path(&source_relative, *place, recipe.extension),
                Part::FontSubset { pack, .. } => {
                    let file = format!("{}.{pack}.{}", file_stem(&source_relative), recipe.extension);
                    source_relative.with_file_name(file)
                }
                Part::Own | Part::Rig | Part::Skeleton => recipe.output_path(&source_relative),
            };
            let output = claim(&asset.path, &relative)?;
            plan.push(Planned { asset: asset.clone(), id, part, planned_from, skeleton, recipe, output });
        }
    }
    // Before writing, so an output whose name only changed case (on a case-insensitive file
    // system) is replaced rather than removed as stale.
    let removed = prune(out_root, &|path| outputs.contains_key(path), COOKED_EXTENSIONS)?;

    let mut by_id = HashMap::with_capacity(plan.len());
    for (index, planned) in plan.iter().enumerate() {
        // Sidecar IDs are unique (checked by `meta::resolve`) and random; a derived ID equal to
        // another is beyond unlikely, but would silently alias two assets.
        if let Some(first) = by_id.insert(planned.id, index) {
            return Err(CookError::Meta(MetaError::DuplicateId {
                id: planned.id,
                first: plan[first].asset.path.clone(),
                second: planned.asset.path.clone(),
            }));
        }
    }
    // Materials read the cooked textures they reference, so everything else cooks first; clips,
    // key-pose documents and meshes sharing a skeleton read skeletons and rigs, so those go before
    // them; key-pose documents read those meshes' morph targets, so the meshes go before clips, and
    // move sets read clips' keys, so those go before move sets; probe grids read cooked meshes and
    // their materials, and a zone's `.szone` reads its meshes, so those cook last. `others` read none
    // of what those cook.
    let (own_zones, rest): (Vec<usize>, Vec<usize>) =
        (0..plan.len()).partition(|&index| plan[index].is_own_zone());
    let (probe_grids, rest): (Vec<usize>, Vec<usize>) =
        rest.into_iter().partition(|&index| plan[index].is_probe_grid());
    let (materials, rest): (Vec<usize>, Vec<usize>) =
        rest.into_iter().partition(|&index| plan[index].is_material());
    let (move_sets, rest): (Vec<usize>, Vec<usize>) =
        rest.into_iter().partition(|&index| plan[index].is_move_set());
    let (rig_readers, others): (Vec<usize>, Vec<usize>) =
        rest.into_iter().partition(|&index| plan[index].reads_rigs());
    let (clips, shared_meshes): (Vec<usize>, Vec<usize>) =
        rig_readers.into_iter().partition(|&index| plan[index].is_clip());
    let mut sources: HashMap<AssetId, &SourceAsset> = HashMap::new();
    for planned in &plan {
        sources.entry(planned.asset.meta.id).or_insert(&planned.asset);
    }
    let memory = CookMemory::new(COOK_MEMORY);
    let tree = PlannedTree {
        plan: &plan,
        by_id: &by_id,
        sources: &sources,
        zones: &zones,
        cache: cache.as_ref(),
        cook: options.cook,
        memory: &memory,
        fluent: &fluent,
    };
    let mut cooked: Vec<Option<CookedAsset>> = vec![None; plan.len()];
    reached(CookStage::Planned);
    cook_independent(tree, &others, workers, &mut cooked)?;
    let remembered = Remembered::default();
    let mut in_turn = |indices: Vec<Vec<usize>>| {
        for index in indices.into_iter().flatten() {
            let assets = tree.assets(&cooked, &remembered);
            let keyed = tree.key(index, &assets)?;
            cooked[index] = Some(tree.finish(index, keyed, &assets)?);
        }
        Ok::<(), CookError>(())
    };
    in_turn(vec![shared_meshes, clips])?;
    reached(CookStage::ClipsCooked);
    in_turn(vec![move_sets, materials, probe_grids, own_zones])?;
    let assets: Vec<CookedAsset> =
        cooked.into_iter().map(|asset| asset.expect("every planned asset was cooked")).collect();
    let localization = check_localization(&assets)?;
    Ok(CookReport {
        assets,
        created_sidecars: resolved.created,
        orphaned_sidecars: resolved.orphaned,
        uncooked: uncooked.into_iter().map(|asset| asset.path).collect(),
        removed,
        localization,
    })
}

/// Each `.ftl` source by the locale its sidecar names, which a locale document must state, and only
/// one: said before any file is read.
fn fluent_files(source_root: &Path, assets: &[SourceAsset]) -> Result<FluentFiles, CookError> {
    let fail = |error: LocCheckError| CookError::Localization(Box::new(error));
    let mut documents: HashMap<&str, &Path> = HashMap::new();
    for asset in assets.iter().filter(|asset| matches!(asset.meta.importer, ImportSettings::Locale(_))) {
        let locale = loc::locale_of_document(&asset.path);
        if let Some(first) = documents.insert(locale, &asset.path) {
            return Err(fail(LocCheckError::DuplicateLocale {
                locale: locale.to_owned(),
                first: first.to_owned(),
                second: asset.path.clone(),
            }));
        }
    }
    let mut files = FluentFiles::new();
    for asset in assets {
        let ImportSettings::Fluent(settings) = &asset.meta.importer else { continue };
        if !documents.contains_key(settings.locale.as_str()) {
            return Err(fail(LocCheckError::NoLocaleDocument {
                locale: settings.locale.clone(),
                path: asset.path.clone(),
            }));
        }
        let relative = asset.path.strip_prefix(source_root).expect("scanned under the source root");
        let name = relative.to_string_lossy().replace('\\', "/");
        files.entry(settings.locale.clone()).or_default().push(FluentFile { path: asset.path.clone(), name });
    }
    Ok(files)
}

/// Checks the cooked tables against each other, from the files as they now are: a table from the cache
/// can disagree with one just cooked, and neither's key covers the other's sources.
fn check_localization(assets: &[CookedAsset]) -> Result<LocReport, CookError> {
    let mut tables = Vec::new();
    for asset in
        assets.iter().filter(|asset| asset.output.extension().is_some_and(|e| e == STRINGS_EXTENSION))
    {
        let bytes = fs::read(&asset.output)
            .map_err(|source| CookError::Read { path: asset.output.clone(), source })?;
        let table = StringTable::read(&bytes).map_err(|source| {
            Box::new(LocCheckError::Read { path: asset.output.clone(), source: Box::new(source) })
        })?;
        tables.push(table);
    }
    loc::check(&tables).map_err(|error| CookError::Localization(Box::new(error)))
}

/// Cooks `indices` of the plan, ascending, assets that read nothing another asset cooks, on up to
/// `workers` threads. Failing, it fails as one thread would, with the first failure in plan order.
///
/// Keys come first: one cook serves every asset of a key, and which of them that is, and so what
/// each reports, mustn't depend on which got there first. The first in plan order of each key finishes
/// on the workers, and the others after them, from the cache, where nothing is left to cook.
fn cook_independent(
    tree: PlannedTree<'_>,
    indices: &[usize],
    workers: usize,
    cooked: &mut [Option<CookedAsset>],
) -> Result<(), CookError> {
    // A zone's cells cook apart: each is a whole mesh's work, reading only what its own key holds.
    let same_source = |&a: &usize, &b: &usize| {
        tree.plan[a].asset.path == tree.plan[b].asset.path && !matches!(tree.plan[b].part, Part::Cell(_))
    };
    let mut failure = None;
    let mut keyed: Vec<Option<Keyed>> = tree.plan.iter().map(|_| None).collect();
    // A source's parts stay together, to share a read of it.
    let runs: Vec<&[usize]> = indices.chunk_by(same_source).collect();
    for (index, result) in on_workers(tree, &runs, workers, |index, assets| tree.key(index, assets)) {
        match result {
            Ok(key) => keyed[index] = Some(key),
            Err(error) => keep_first(&mut failure, index, error),
        }
    }
    let before_failure = |failure: &Option<(usize, CookError)>, index: usize| {
        keyed[index].is_some() && failure.as_ref().is_none_or(|(first, _)| index < *first)
    };
    let mut keys = HashSet::new();
    let (first_of_key, repeats): (Vec<usize>, Vec<usize>) =
        indices.iter().copied().filter(|&index| before_failure(&failure, index)).partition(|&index| {
            // With no cache to serve the others, every asset cooks.
            tree.cache.is_none() || keys.insert(keyed[index].as_ref().expect("filtered on its key").key)
        });
    let runs: Vec<&[usize]> = first_of_key.chunk_by(same_source).collect();
    let finish = |index, assets: &Assets<'_>| {
        tree.finish(index, keyed[index].as_ref().expect("only keyed assets finish").clone(), assets)
    };
    for (index, result) in on_workers(tree, &runs, workers, finish) {
        match result {
            Ok(asset) => cooked[index] = Some(asset),
            Err(error) => keep_first(&mut failure, index, error),
        }
    }
    let remembered = Remembered::default();
    let assets = tree.assets(&[], &remembered);
    for index in repeats {
        if !before_failure(&failure, index) {
            break;
        }
        match finish(index, &assets) {
            Ok(asset) => cooked[index] = Some(asset),
            Err(error) => keep_first(&mut failure, index, error),
        }
    }
    failure.map_or(Ok(()), |(_, error)| Err(error))
}

/// Keeps the failure of the asset earliest in the plan.
fn keep_first(failure: &mut Option<(usize, CookError)>, index: usize, error: CookError) {
    if failure.as_ref().is_none_or(|(first, _)| index < *first) {
        *failure = Some((index, error));
    }
}

/// `job` over the assets of `runs`, on up to `workers` threads, each run's on one, in order. As far as
/// the jobs can tell, nothing has cooked.
///
/// Runs start in plan order, so when an asset fails, every asset before it has started, and whatever
/// comes after is left: the earliest failure is among what this returns.
fn on_workers<T: Send>(
    tree: PlannedTree<'_>,
    runs: &[&[usize]],
    workers: usize,
    job: impl Fn(usize, &Assets<'_>) -> Result<T, CookError> + Sync,
) -> Vec<(usize, Result<T, CookError>)> {
    let next = AtomicUsize::new(0);
    let failed = AtomicUsize::new(usize::MAX);
    let work = || {
        let remembered = Remembered::default();
        let assets = tree.assets(&[], &remembered);
        let mut done = Vec::new();
        while let Some(run) = runs.get(next.fetch_add(1, Ordering::Relaxed)) {
            if run[0] > failed.load(Ordering::Relaxed) {
                break;
            }
            for &index in *run {
                let result = job(index, &assets);
                let ok = result.is_ok();
                done.push((index, result));
                if !ok {
                    failed.fetch_min(index, Ordering::Relaxed);
                    break;
                }
            }
        }
        done
    };
    thread::scope(|scope| {
        let handles: Vec<_> = (0..workers.min(runs.len()))
            .map(|_| {
                let worker = thread::Builder::new().stack_size(WORKER_STACK);
                worker.spawn_scoped(scope, work).expect("the system starts a cook worker")
            })
            .collect();
        handles.into_iter().flat_map(|handle| handle.join().expect("a cook worker panicked")).collect()
    })
}

/// Which of a source's assets an output is.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Part {
    /// The source's own asset.
    Own,
    /// A VRM's rig.
    Rig,
    /// A skinned source's skeleton.
    Skeleton,
    /// A source's animation: its index in the file, its clip's unique name, the part of its output's
    /// file name that stands for that name, and the entry of the sidecar's `retarget` it's for,
    /// `None` for the source's own rig.
    Clip { index: usize, name: String, file: String, target: Option<usize> },
    /// A key-pose document, for a further rig its sidecar names, and what stands for it in the
    /// output's file name.
    KeyPose { rig: AssetId, file: String },
    /// A zone's merged mesh for one place.
    Cell(Place),
    /// A font's cut for a further pack: its entry in the sidecar's `subsets`, and that pack.
    FontSubset { index: usize, pack: String },
}

/// A part of a source, its ID and its recipe.
struct PlannedPart {
    id: AssetId,
    part: Part,
    recipe: Recipe,
}

/// The sub-assets `asset` cooks to besides its own asset, reading the file where that depends on
/// what it holds, and saying the hash of what it read in `planned_from`. Its `clips` settings are
/// checked here, before anything cooks, and a zone's plan is made, from `prefabs`, and left in `zones`.
fn sub_assets(
    asset: &SourceAsset,
    planned_from: &Cell<Option<blake3::Hash>>,
    prefabs: &HashMap<AssetId, SourceAsset>,
    zones: &mut HashMap<PathBuf, ZonePlan>,
    cache: Option<&Cache>,
) -> Result<Vec<PlannedPart>, CookError> {
    let SourceAsset { path, meta } = asset;
    let clip_error = |source: ClipCookError| CookError::Clip { path: path.clone(), source: Box::new(source) };
    let unsupported = |importer| clip_error(ClipCookError::ClipsUnsupported { importer });
    let import_error = |source: gltf::clips::ClipImportError| clip_error(source.into());
    let read = || {
        let bytes = fs::read(path).map_err(|source| CookError::Read { path: path.clone(), source })?;
        planned_from.set(Some(blake3::hash(&bytes)));
        Ok::<_, CookError>(bytes)
    };
    let skeleton = |recipe: Recipe| PlannedPart {
        id: meta::sub_asset_id(meta.id, SKELETON),
        part: Part::Skeleton,
        recipe: recipe.skeleton(),
    };
    let own = Recipe::for_settings(&meta.importer);
    match &meta.importer {
        ImportSettings::Vrm(settings) => {
            if settings.mesh.clips.is_some() || settings.mesh.skeleton.is_some() {
                return Err(unsupported("VRM"));
            }
            let rig = PlannedPart {
                id: meta::sub_asset_id(meta.id, vrm::RIG.name),
                part: Part::Rig,
                recipe: vrm::RIG.recipe,
            };
            Ok(vec![rig, skeleton(own)])
        }
        ImportSettings::Gltf(settings) => {
            let bytes = read()?;
            let skinned = gltf::has_skeleton(&bytes, settings.clips.is_some())
                .map_err(|source| CookError::Import { path: path.clone(), source })?;
            let mut parts = Vec::new();
            if skinned && settings.skeleton.is_none() {
                parts.push(skeleton(own));
            }
            if let Some(clips) = &settings.clips {
                let (names, joints) = gltf::clips::plan(&bytes, |_| Ok(Vec::new())).map_err(import_error)?;
                parts.extend(
                    clip_parts(meta.id, clips, true, &names, &joints, Recipe::gltf_clip())
                        .map_err(clip_error)?,
                );
            }
            Ok(parts)
        }
        ImportSettings::Fbx(settings) => {
            let bytes = read()?;
            let skinned = fbx::has_skeleton(&bytes, settings.clips.is_some())
                .map_err(|source| CookError::Fbx { path: path.clone(), source: Box::new(source) })?;
            let mut parts = Vec::new();
            if skinned && settings.skeleton.is_none() {
                parts.push(skeleton(own));
            }
            if let Some(clips) = &settings.clips {
                let (names, joints) = fbx::clips::plan(&bytes).map_err(import_error)?;
                parts.extend(
                    clip_parts(meta.id, clips, true, &names, &joints, Recipe::fbx_clip())
                        .map_err(clip_error)?,
                );
            }
            Ok(parts)
        }
        ImportSettings::GltfClips(settings) | ImportSettings::Vrma(settings) => {
            let is_vrma = matches!(meta.importer, ImportSettings::Vrma(_));
            if is_vrma && !settings.humanoid.is_empty() {
                return Err(clip_error(ClipCookError::VrmaHumanoid));
            }
            let (names, joints) = gltf::clips::plan(&read()?, |document| {
                if is_vrma { vrma::humanoid_nodes(document) } else { Ok(Vec::new()) }
            })
            .map_err(import_error)?;
            animation_parts(meta.id, settings, &names, &joints, own, skeleton(own)).map_err(clip_error)
        }
        ImportSettings::FbxClips(settings) => {
            let (names, joints) = fbx::clips::plan(&read()?).map_err(import_error)?;
            animation_parts(meta.id, settings, &names, &joints, own, skeleton(own)).map_err(clip_error)
        }
        ImportSettings::Assembly(settings) => {
            if settings.clips.is_some() || settings.skeleton.is_some() {
                return Err(unsupported("assembly"));
            }
            Ok(Vec::new())
        }
        ImportSettings::KeyPose(settings) => Ok(settings
            .also
            .iter()
            .map(|&rig| PlannedPart {
                id: meta::sub_asset_id(meta.id, &format!("rig/{rig}")),
                part: Part::KeyPose { rig, file: short_id(rig) },
                recipe: own,
            })
            .collect()),
        // The first subset is the source's own asset. Checked here, before anything cooks.
        ImportSettings::Font(settings) => {
            font::check_settings(settings).map_err(font_error(path))?;
            Ok(settings
                .subsets
                .iter()
                .enumerate()
                .skip(1)
                .map(|(index, subset)| PlannedPart {
                    id: meta::sub_asset_id(meta.id, &format!("pack/{}", subset.pack)),
                    part: Part::FontSubset { index, pack: subset.pack.clone() },
                    recipe: own,
                })
                .collect())
        }
        ImportSettings::Texture(_)
        | ImportSettings::CharacterMap(_)
        | ImportSettings::Material(_)
        | ImportSettings::ProbeGrid(_)
        | ImportSettings::Colliders(_)
        | ImportSettings::Sound(_)
        | ImportSettings::Cue(_)
        | ImportSettings::FontChains(_)
        | ImportSettings::MoveSet(_)
        | ImportSettings::AnimGraph(_)
        | ImportSettings::Fluent(_)
        | ImportSettings::Locale(_) => Ok(Vec::new()),
        ImportSettings::Zone(_) => {
            let plan = ZonePlan::new(meta.id, &read()?, prefabs, cache).map_err(zone_error(path))?;
            let parts = plan.places().map(|place| PlannedPart {
                id: plan.part_id(place),
                part: Part::Cell(place),
                recipe: Recipe::zone_cell(),
            });
            let parts = parts.collect();
            zones.insert(path.clone(), plan);
            Ok(parts)
        }
    }
}

/// What stands for an asset in a file name: the start of its ID.
fn short_id(id: AssetId) -> String {
    id.to_string().chars().take(8).collect()
}

/// An animation source's parts: its skeleton, unless it shares a rig, and its clips.
fn animation_parts(
    id: AssetId,
    settings: &ClipSettings,
    names: &[String],
    joints: &[Joint],
    recipe: Recipe,
    skeleton: PlannedPart,
) -> Result<Vec<PlannedPart>, ClipCookError> {
    if joints.is_empty() {
        return Err(ClipCookError::NoSkeleton);
    }
    let mut parts = Vec::new();
    if settings.rig.is_none() {
        parts.push(skeleton);
    }
    parts.extend(clip_parts(id, settings, false, names, joints, recipe)?);
    Ok(parts)
}

/// The clips a source's sidecar asks for. Checks its settings against the file, before anything
/// cooks: a name or joint it doesn't have is the sidecar's error, and names it.
fn clip_parts(
    id: AssetId,
    settings: &ClipSettings,
    on_mesh: bool,
    names: &[String],
    joints: &[Joint],
    recipe: Recipe,
) -> Result<Vec<PlannedPart>, ClipCookError> {
    if !settings.is_valid() {
        return Err(ClipCookError::Settings);
    }
    if on_mesh && settings.rig.is_some() {
        return Err(ClipCookError::RigOnMesh);
    }
    if !settings.humanoid.is_empty() {
        if settings.rig.is_some() {
            return Err(ClipCookError::RigAndHumanoid);
        }
        HumanMap::from_names(&settings.humanoid, &Skeleton::new(joints))?;
    }
    let files = clip_files(names);
    let index_of = |name: &str| {
        names
            .iter()
            .position(|candidate| candidate == name)
            .ok_or_else(|| ClipCookError::UnknownClip { name: name.to_owned() })
    };
    let mut seen = HashSet::new();
    let mut parts = Vec::new();
    let own: Vec<usize> = match &settings.names {
        None => (0..names.len()).collect(),
        Some(list) => list.iter().map(|name| index_of(name)).collect::<Result<_, _>>()?,
    };
    for index in own {
        let name = names[index].clone();
        if !seen.insert((index, None)) {
            return Err(ClipCookError::DuplicateClip { name });
        }
        parts.push(PlannedPart {
            id: meta::sub_asset_id(id, &format!("clip/{name}")),
            part: Part::Clip { index, name, file: files[index].clone(), target: None },
            recipe,
        });
    }
    for (entry, retarget) in settings.retarget.iter().enumerate() {
        for name in &retarget.names {
            let index = index_of(name)?;
            if !seen.insert((index, Some(retarget.target))) {
                return Err(ClipCookError::DuplicateClip { name: name.clone() });
            }
            parts.push(PlannedPart {
                id: meta::sub_asset_id(id, &format!("clip/{name}@{}", retarget.target)),
                part: Part::Clip {
                    index,
                    name: name.clone(),
                    file: format!("{}.{}", files[index], short_id(retarget.target)),
                    target: Some(entry),
                },
                recipe: recipe.retargeted(),
            });
        }
    }
    Ok(parts)
}

/// What stands for each clip name in its output's file name: the name, with anything a file system
/// might object to replaced, and its index added where that would make two names one file. Names
/// are the file's own and can't be asked to change.
fn clip_files(names: &[String]) -> Vec<String> {
    let safe = |name: &String| -> String {
        name.chars()
            .map(|c| if c.is_ascii_alphanumeric() || c == '_' || c == '-' { c } else { '_' })
            .collect()
    };
    let mut files: Vec<String> = names.iter().map(safe).collect();
    // A literal name can match a made-up one (`a_b-0`): suffix the clashing names until none clashes.
    // A suffix ends in its own name's index, so two names that have each taken one never meet again,
    // and this ends.
    loop {
        let folded: Vec<String> = files.iter().map(|file| file.to_lowercase()).collect();
        let clashing: Vec<usize> = (0..files.len())
            .filter(|&index| folded.iter().filter(|other| **other == folded[index]).count() > 1)
            .collect();
        if clashing.is_empty() {
            return files;
        }
        for index in clashing {
            files[index] = format!("{}-{index}", files[index]);
        }
    }
}

/// `path`'s file name without its extension.
fn file_stem(path: &Path) -> String {
    path.file_stem().map(|stem| stem.to_string_lossy().into_owned()).unwrap_or_default()
}

/// A source asset, its recipe and where it cooks to: the source's own asset, or one of its
/// sub-assets.
struct Planned {
    asset: SourceAsset,
    /// The sidecar's ID, or the sub-asset's derived one.
    id: AssetId,
    part: Part,
    /// The hash of the source as the plan read it, if it did: which parts there are and which animation
    /// a clip is hold for that version alone. A name wouldn't do instead, as a file's unnamed or repeated
    /// animations are named by their place in it.
    planned_from: Option<blake3::Hash>,
    /// The ID of the source's skeleton, if it cooks one: what its mesh and its clips name.
    skeleton: Option<AssetId>,
    recipe: Recipe,
    output: PathBuf,
}

impl Planned {
    fn is_material(&self) -> bool {
        matches!(self.asset.meta.importer, ImportSettings::Material(_))
    }

    fn is_probe_grid(&self) -> bool {
        matches!(self.asset.meta.importer, ImportSettings::ProbeGrid(_))
    }

    fn is_move_set(&self) -> bool {
        matches!(self.asset.meta.importer, ImportSettings::MoveSet(_))
    }

    /// Whether it's a zone's `.szone`, which reads its meshes.
    fn is_own_zone(&self) -> bool {
        self.part == Part::Own && matches!(self.asset.meta.importer, ImportSettings::Zone(_))
    }

    /// Whether its cook reads skeletons or rigs that other assets cook.
    fn reads_rigs(&self) -> bool {
        let importer = &self.asset.meta.importer;
        matches!(self.part, Part::Clip { .. } | Part::KeyPose { .. })
            || matches!(importer, ImportSettings::KeyPose(_))
            || (self.part == Part::Own && importer.shared_rig().is_some())
    }

    fn is_sound(&self) -> bool {
        matches!(self.asset.meta.importer, ImportSettings::Sound(_))
    }

    fn is_font(&self) -> bool {
        matches!(self.asset.meta.importer, ImportSettings::Font(_))
    }

    /// Whether it cooks to a clip: a sub-asset of an animation source, or a key-pose document.
    fn is_clip(&self) -> bool {
        matches!(self.part, Part::Clip { .. } | Part::KeyPose { .. })
            || (self.part == Part::Own && matches!(self.asset.meta.importer, ImportSettings::KeyPose(_)))
    }
}

/// What a cook of the planned tree shares between threads.
#[derive(Clone, Copy)]
struct PlannedTree<'a> {
    plan: &'a [Planned],
    by_id: &'a HashMap<AssetId, usize>,
    sources: &'a HashMap<AssetId, &'a SourceAsset>,
    /// Zones' plans by their sources' paths.
    zones: &'a HashMap<PathBuf, ZonePlan>,
    cache: Option<&'a Cache>,
    cook: Cook,
    memory: &'a CookMemory,
    fluent: &'a FluentFiles,
}

impl<'a> PlannedTree<'a> {
    /// The tree's assets, as a thread that remembers `remembered` sees them with `cooked` cooked: an
    /// asset past the end of `cooked` hasn't.
    fn assets<'b>(&self, cooked: &'b [Option<CookedAsset>], remembered: &'b Remembered) -> Assets<'b>
    where
        'a: 'b,
    {
        Assets {
            plan: self.plan,
            by_id: self.by_id,
            cooked,
            sources: self.sources,
            zones: self.zones,
            clip_sources: &remembered.clip_sources,
            last_source: &remembered.last_source,
            memory: self.memory,
            fluent: self.fluent,
        }
    }

    /// The plan's asset `index`'s key.
    fn key(&self, index: usize, assets: &Assets<'_>) -> Result<Keyed, CookError> {
        let planned = &self.plan[index];
        guarded(&planned.asset.path, || key_asset(planned, assets, self.cook))
    }

    /// Brings the output of the plan's asset `index` up to date, by its key.
    fn finish(&self, index: usize, keyed: Keyed, assets: &Assets<'_>) -> Result<CookedAsset, CookError> {
        let planned = &self.plan[index];
        guarded(&planned.asset.path, || finish_asset(planned, keyed, assets, self.cache, self.cook))
    }
}

/// What a thread keeps from one asset to the next, each holding one source's data.
#[derive(Default)]
struct Remembered {
    clip_sources: ClipSources,
    last_source: LastSource,
}

/// The content tree's assets by ID, with what has cooked so far.
struct Assets<'a> {
    plan: &'a [Planned],
    by_id: &'a HashMap<AssetId, usize>,
    cooked: &'a [Option<CookedAsset>],
    /// Sources by their sidecars' IDs, whether they have an asset of that ID or not.
    sources: &'a HashMap<AssetId, &'a SourceAsset>,
    zones: &'a HashMap<PathBuf, ZonePlan>,
    clip_sources: &'a ClipSources,
    last_source: &'a LastSource,
    memory: &'a CookMemory,
    fluent: &'a FluentFiles,
}

/// A source's rig as other assets read it: its cooked skeleton and where its humanoid map comes
/// from. A mesh that shares a skeleton stands for its owner's.
#[derive(Clone, Copy)]
struct RigSource<'a> {
    skeleton_id: AssetId,
    skeleton: &'a CookedAsset,
    /// A VRM's.
    rig: Option<&'a CookedAsset>,
    /// A sidecar's.
    names: Option<&'a BTreeMap<HumanBone, String>>,
    /// The source's own mesh, whose morph targets a key-pose document names.
    mesh: Option<&'a CookedAsset>,
    /// The source the skeleton is cooked from, as it's read.
    importer: &'a ImportSettings,
    /// That source's ID.
    owner: AssetId,
}

impl RigSource<'_> {
    fn has_map(&self) -> bool {
        self.rig.is_some() || self.names.is_some()
    }

    /// Its humanoid map, on its cooked `skeleton`.
    fn map(&self, skeleton: &Skeleton) -> Result<Option<HumanMap>, ClipCookError> {
        if let Some(rig) = self.rig {
            let bytes = read_cooked(rig).map_err(KeyPoseError::Read)?;
            let rig = CharacterRig::from_cooked(&bytes).map_err(KeyPoseError::Rig)?;
            return Ok(Some(HumanMap::from_rig(&rig)));
        }
        self.names.map(|names| HumanMap::from_names(names, skeleton)).transpose().map_err(Into::into)
    }

    /// Its cooked skeleton.
    fn read_skeleton(&self) -> Result<Skeleton, ClipCookError> {
        let bytes = read_cooked(self.skeleton).map_err(KeyPoseError::Read)?;
        Ok(Skeleton::from_cooked(&bytes).map_err(KeyPoseError::Skeleton)?)
    }

    /// What keys its map, for an asset cooked through it.
    fn dependencies(&self, name: &str) -> Vec<Dependency> {
        let asset = |asset: &CookedAsset| Dependency::Asset { id: asset.id, key: asset.key };
        let mut dependencies = vec![asset(self.skeleton)];
        dependencies.extend(self.rig.map(asset));
        if let Some(names) = self.names {
            let text: String =
                names.iter().map(|(bone, joint)| format!("{}={joint};", bone.name())).collect();
            dependencies.push(Dependency::File {
                path: format!("#humanoid {name}"),
                hash: blake3::hash(text.as_bytes()),
            });
        }
        dependencies
    }
}

/// An asset's cooked file, aligned as its readers need it.
fn read_cooked(asset: &CookedAsset) -> Result<sapphire_serialize::cooked::AlignedVec<16>, io::Error> {
    let bytes = fs::read(&asset.output)?;
    let mut aligned = sapphire_serialize::cooked::AlignedVec::<16>::with_capacity(bytes.len());
    aligned.extend_from_slice(&bytes);
    Ok(aligned)
}

impl<'a> Assets<'a> {
    fn get(&self, id: AssetId) -> Option<&'a Planned> {
        self.by_id.get(&id).map(|&index| &self.plan[index])
    }

    /// What a material's cook knows about asset `id`.
    fn referenced(&self, id: AssetId) -> Option<Referenced<'a>> {
        let index = *self.by_id.get(&id)?;
        let planned = &self.plan[index];
        Some(Referenced {
            source: &planned.asset.path,
            class: TextureKind::of(&planned.asset.meta.importer),
            cooked: self
                .cooked
                .get(index)
                .and_then(Option::as_ref)
                .map(|asset| (asset.output.as_path(), asset.key)),
        })
    }

    /// What a probe grid's cook knows about asset `id`.
    fn probe_input(&self, id: AssetId) -> Option<ProbeInput<'a>> {
        let index = *self.by_id.get(&id)?;
        let planned = &self.plan[index];
        Some(ProbeInput {
            source: &planned.asset.path,
            settings: &planned.asset.meta.importer,
            cooked: self
                .cooked
                .get(index)
                .and_then(Option::as_ref)
                .map(|asset| (asset.output.as_path(), asset.key)),
        })
    }

    /// The cooked asset `id`, if it has cooked.
    fn cooked(&self, id: AssetId) -> Option<&'a CookedAsset> {
        self.cooked.get(*self.by_id.get(&id)?)?.as_ref()
    }

    /// The rig of source `id`: its own, or the one it shares.
    fn rig_source(&self, id: AssetId) -> Option<RigSource<'a>> {
        let source = self.sources.get(&id)?;
        let owner = source.meta.importer.shared_rig().unwrap_or(id);
        let owner_source = self.sources.get(&owner)?;
        if owner_source.meta.importer.shared_rig().is_some() {
            return None;
        }
        let skeleton_id = meta::sub_asset_id(owner, SKELETON);
        Some(RigSource {
            skeleton_id,
            skeleton: self.cooked(skeleton_id)?,
            rig: self.cooked(meta::sub_asset_id(owner, vrm::RIG.name)),
            names: owner_source
                .meta
                .importer
                .clips()
                .map(|clips| &clips.humanoid)
                .filter(|names| !names.is_empty()),
            mesh: source.meta.importer.mesh().and_then(|_| self.cooked(id)),
            importer: &owner_source.meta.importer,
            owner,
        })
    }

    /// The rig a sidecar's `rig` or `skeleton` shares: `id` must cook it. What shares a skeleton names
    /// `id`'s as its own, and a source sharing another's cooks none, so a mesh going through one would
    /// name a skeleton nothing makes.
    fn shared_rig_source(&self, id: AssetId) -> Result<RigSource<'a>, ClipCookError> {
        self.rig_source(id).filter(|rig| rig.owner == id).ok_or(ClipCookError::UnknownRig { id })
    }

    /// The rig a key-pose document cooks for: `id`, which the sidecar must name, must have a map. Its
    /// mesh has cooked if it has one: a rig with none skips a document's morph weights, and a mesh yet
    /// to cook must never read as that, or the order of file names would decide what a clip holds.
    fn key_pose_rig(&self, id: Option<AssetId>) -> Result<RigSource<'a>, KeyPoseError> {
        let id = id.ok_or(KeyPoseError::NoRig)?;
        let rig = self.rig_source(id).filter(RigSource::has_map).ok_or(KeyPoseError::UnknownRig { id })?;
        if rig.mesh.is_none() && self.get(id).is_some() {
            return Err(KeyPoseError::MeshNotCooked { id });
        }
        Ok(rig)
    }

    /// The clip a move names ([`moves`]), by what its document says.
    fn clip_of_move(&self, clip: &ClipMotionDocument) -> Result<&'a Planned, BakeError> {
        let id_of = |field, text: &str| {
            text.parse::<AssetId>().map_err(|_| BakeError::NotAnId { field, text: text.to_owned() })
        };
        let source_id = id_of("source", &clip.source)?;
        let rig = clip.rig.as_deref().map(|text| id_of("rig", text)).transpose()?;
        let source = self.sources.get(&source_id).ok_or(BakeError::UnknownSource { id: source_id })?;
        let id = match &source.meta.importer {
            ImportSettings::KeyPose(settings) => {
                if clip.name.is_some() {
                    return Err(BakeError::NameOnKeyPose);
                }
                match rig.filter(|rig| Some(*rig) != settings.rig) {
                    Some(rig) => meta::sub_asset_id(source_id, &format!("rig/{rig}")),
                    None => source_id,
                }
            }
            importer if importer.clips().is_some() => {
                let name = clip.name.as_deref().ok_or(BakeError::NoName)?;
                match rig {
                    Some(rig) => meta::sub_asset_id(source_id, &format!("clip/{name}@{rig}")),
                    None => meta::sub_asset_id(source_id, &format!("clip/{name}")),
                }
            }
            _ => return Err(BakeError::NotAClipSource { id: source_id }),
        };
        self.get(id).ok_or(BakeError::NotCooked { source_id })
    }

    /// The rig the clip of `planned` is for.
    fn rig_of_clip(&self, planned: &Planned) -> Option<RigSource<'a>> {
        let importer = &planned.asset.meta.importer;
        let id = match (&planned.part, importer) {
            (Part::KeyPose { rig, .. }, _) => *rig,
            (_, ImportSettings::KeyPose(settings)) => settings.rig?,
            (Part::Clip { target: Some(entry), .. }, _) => importer.clips()?.retarget[*entry].target,
            (Part::Clip { target: None, .. }, _) => planned.asset.meta.id,
            _ => return None,
        };
        self.rig_source(id)
    }
}

/// Whether a source's numbers come through `ufbx`'s C: see [`BakeError::FbxSource`].
fn is_fbx(importer: &ImportSettings) -> bool {
    matches!(importer, ImportSettings::Fbx(_) | ImportSettings::FbxClips(_))
}

/// A move's tables, baked from the clip it names.
fn move_tables(clip: &ClipMotionDocument, assets: &Assets<'_>) -> Result<BakedTables, BakeError> {
    let planned = assets.clip_of_move(clip)?;
    let SourceAsset { path, meta } = &planned.asset;
    if is_fbx(&meta.importer) {
        return Err(BakeError::FbxSource { id: meta.id });
    }
    let rig = assets.rig_of_clip(planned).ok_or(BakeError::NotCooked { source_id: meta.id })?;
    if is_fbx(rig.importer) {
        return Err(BakeError::FbxSource { id: rig.owner });
    }
    let bytes = fs::read(path).map_err(BakeError::Read)?;
    let samples = clip_samples(planned, &bytes, assets).map_err(Box::new)?;
    let skeleton = samples.skeleton.as_ref().ok_or_else(|| Box::new(ClipCookError::NoSkeleton))?;
    let map = rig.map(skeleton).map_err(Box::new)?;
    let cooked = |error: KeyPoseError| Box::new(ClipCookError::KeyPose(error));
    let constraints = match rig.rig {
        Some(rig) => {
            let bytes = read_cooked(rig).map_err(|error| cooked(KeyPoseError::Read(error)))?;
            CharacterRig::from_cooked(&bytes).map_err(|error| cooked(KeyPoseError::Rig(error)))?.constraints
        }
        None => Vec::new(),
    };
    moves::bake(
        clip,
        &TableInput { sampled: &samples.sampled, skeleton, map: map.as_ref(), constraints: &constraints },
    )
}

/// What a clip's cook and a move's bake both start from: its samples, and the rig they index.
struct ClipSamples {
    sampled: SampledClip,
    skeleton: Option<Skeleton>,
    /// What the clip names as its skeleton.
    skeleton_id: Option<AssetId>,
    morph_defaults: Vec<f32>,
}

impl ClipSamples {
    /// What the clip's cook reduces against.
    fn target(&self) -> ClipTarget<'_> {
        ClipTarget {
            skeleton: self.skeleton_id.zip(self.skeleton.as_ref()),
            morph_defaults: &self.morph_defaults,
        }
    }
}

/// The samples of the clip `planned`, whose source's contents are `bytes`.
fn clip_samples(planned: &Planned, bytes: &[u8], assets: &Assets<'_>) -> Result<ClipSamples, ClipCookError> {
    let SourceAsset { path, meta } = &planned.asset;
    if let Part::Clip { index, target, .. } = &planned.part {
        let clips = meta.importer.clips().ok_or(ClipCookError::NoSkeleton)?;
        let input = ClipInput {
            path,
            bytes,
            importer: &meta.importer,
            clips,
            index: *index,
            skeleton: planned.skeleton,
        };
        return source_samples(&input, target.map(|entry| &clips.retarget[entry]), assets);
    }
    let ImportSettings::KeyPose(settings) = &meta.importer else {
        return Err(ClipCookError::NoSkeleton);
    };
    let rig = if let Part::KeyPose { rig, .. } = &planned.part { Some(*rig) } else { settings.rig };
    key_pose_samples(path, bytes, rig, assets)
}

/// A source's contents and their hash.
struct SourceBytes {
    bytes: Vec<u8>,
    hash: blake3::Hash,
}

/// The source read last: a source's parts cook one after another, so a file of 100 clips is read and
/// hashed once rather than 100 times, on every cook, and all its parts read one version of it. Only
/// one source's bytes are ever held, as [`ClipSources`] holds one source's import.
#[derive(Default)]
struct LastSource(RefCell<Option<(PathBuf, Rc<SourceBytes>)>>);

impl LastSource {
    /// The source at `path`.
    fn read(&self, path: &Path) -> Result<Rc<SourceBytes>, CookError> {
        if let Some((last, source)) = &*self.0.borrow()
            && last == path
        {
            return Ok(Rc::clone(source));
        }
        // Before the read, so two sources are never held at once.
        *self.0.borrow_mut() = None;
        let bytes = fs::read(path).map_err(|source| CookError::Read { path: path.to_owned(), source })?;
        let source = Rc::new(SourceBytes { hash: blake3::hash(&bytes), bytes });
        *self.0.borrow_mut() = Some((path.to_owned(), Rc::clone(&source)));
        Ok(source)
    }

    /// The source at `path`, which must be what `hash` was made from: a key mustn't cover what another
    /// version of its source cooks to, for as long as the cache lasts.
    fn read_as(&self, path: &Path, hash: blake3::Hash) -> Result<Rc<SourceBytes>, CookError> {
        let source = self.read(path)?;
        if source.hash == hash { Ok(source) } else { Err(CookError::SourceChanged { path: path.to_owned() }) }
    }
}

/// Runs `cook` for the source at `path`, a panic becoming an error that names it. Importers and
/// cookers report what they find wrong themselves; this is the last line of defence, for what
/// slips past them or panics inside a crate they call, so that the file is named all the same.
fn guarded<T>(path: &Path, cook: impl FnOnce() -> Result<T, CookError>) -> Result<T, CookError> {
    panic::catch_unwind(AssertUnwindSafe(cook)).unwrap_or_else(|payload| {
        let text = payload.downcast_ref::<&str>().map(|text| (*text).to_owned());
        let message = text.or_else(|| payload.downcast_ref::<String>().cloned());
        Err(CookError::Panicked { path: path.to_owned(), message: message.unwrap_or_default() })
    })
}

/// An asset's key, and the hashes of what it was made from and of its output as it stands.
#[derive(Clone)]
struct Keyed {
    key: CacheKey,
    source: blake3::Hash,
    current: Option<blake3::Hash>,
}

/// Keys one asset.
fn key_asset(planned: &Planned, assets: &Assets<'_>, cook: Cook) -> Result<Keyed, CookError> {
    let path = &planned.asset.path;
    let source = match planned.planned_from {
        Some(hash) => assets.last_source.read_as(path, hash)?,
        None => assets.last_source.read(path)?,
    };
    let key = asset_key(planned, &source, assets, cook)?;
    Ok(Keyed { key, source: source.hash, current: file_hash(&planned.output)? })
}

/// The key of one asset cooked from `source`, by everything else it reads as that stands now.
fn asset_key(
    planned: &Planned,
    source: &SourceBytes,
    assets: &Assets<'_>,
    cook: Cook,
) -> Result<CacheKey, CookError> {
    let Planned { asset: SourceAsset { path, meta }, part, skeleton, recipe, .. } = planned;
    let mut dependencies = dependencies(path, &source.bytes, &meta.importer, part, assets)?;
    dependencies.extend(part_dependencies(path, part, *skeleton, &meta.importer));
    // A cell's key holds what's in it, rather than the whole document (see `zone`).
    let keyed_source = match part {
        Part::Cell(place) => assets.zones[path].recipe(*place),
        _ => source.hash,
    };
    Ok(CacheKey::new(&KeyInputs {
        source: keyed_source,
        recipe,
        settings: &meta.importer,
        cook,
        dependencies: &dependencies,
    }))
}

/// Brings one asset's output up to date, by its key.
fn finish_asset(
    planned: &Planned,
    Keyed { key, source, current }: Keyed,
    assets: &Assets<'_>,
    cache: Option<&Cache>,
    cook: Cook,
) -> Result<CookedAsset, CookError> {
    let Planned { asset: SourceAsset { path, meta }, id, part, skeleton, output, .. } = planned;
    let done = |status, output_hash| CookedAsset {
        id: *id,
        source: path.clone(),
        output: output.clone(),
        key,
        output_hash,
        status,
    };

    let mut cached = None;
    if let Some(cache) = cache
        && let Some(hash) = cache.output_hash(&key)?
    {
        if current == Some(hash) {
            return Ok(done(CookStatus::UpToDate, hash));
        }
        cached = cache.get(&key)?;
    }
    let (status, cooked) = match cached {
        Some(cooked) => (CookStatus::Cached, cooked),
        None => {
            let source = assets.last_source.read_as(path, source)?;
            let cooked = cook_source(path, &source.bytes, &meta.importer, part, *skeleton, cook, assets)?;
            // A cook reads its other files for itself, as long after its key as the assets ahead of it
            // take, and a key mustn't cover what other versions of them cook to. Their keys are made from
            // the files as they stand, so this sees a save from before or during the cook.
            if asset_key(planned, &source, assets, cook)? != key
                || !baked_clips_as_keyed(planned, &source.bytes, assets, cook)?
            {
                return Err(CookError::SourceChanged { path: path.clone() });
            }
            if let Some(cache) = cache {
                cache.put(&key, &cooked)?;
            }
            (CookStatus::Cooked, cooked)
        }
    };
    let hash = blake3::hash(&cooked);
    if current != Some(hash) {
        write_atomically(output, &cooked)?;
    }
    Ok(done(status, hash))
}

/// Whether the clips a move set baked from would key now as they did when they cooked. A bake reads a
/// clip's source for itself, and a set's key holds only the clip's key, which making the set's key again
/// takes from what has cooked, so that can't see a save. The other parts of a cook have no such reads.
fn baked_clips_as_keyed(
    planned: &Planned,
    bytes: &[u8],
    assets: &Assets<'_>,
    cook: Cook,
) -> Result<bool, CookError> {
    if !planned.is_move_set() {
        return Ok(true);
    }
    for clip in clips_of_set(bytes, assets) {
        if let Some(cooked) = assets.cooked(clip.id) {
            let source = assets.last_source.read(&clip.asset.path)?;
            if asset_key(clip, &source, assets, cook)? != cooked.key {
                return Ok(false);
            }
        }
    }
    Ok(true)
}

/// What keys a part of a source apart from its other parts, and from the same bytes under another
/// sidecar, besides what every part's key holds. A key leaves a source's ID out, so that a copied
/// source is a hit, until a cooked format holds an ID: a mesh's skin and a clip name their skeleton.
fn part_dependencies(
    path: &Path,
    part: &Part,
    skeleton: Option<AssetId>,
    settings: &ImportSettings,
) -> Vec<Dependency> {
    let marker = |path: String| Dependency::File { path, hash: blake3::hash(b"") };
    let mut dependencies = Vec::new();
    if matches!(part, Part::Own | Part::Clip { .. })
        && let Some(id) = skeleton
    {
        dependencies.push(marker(format!("#skeleton {id}")));
    }
    if let Part::Clip { name, target, .. } = part {
        dependencies.push(marker(format!("#clip {name}")));
        if let (Some(entry), Some(clips)) = (target, settings.clips()) {
            dependencies.push(marker(format!("#target {}", clips.retarget[*entry].target)));
        }
    }
    // Subsets of a font share its sidecar and differ by their entry in it.
    if let Part::FontSubset { index, .. } = part {
        dependencies.push(marker(format!("#subset {index}")));
    }
    // A key-pose clip takes its name from its file, which a key doesn't otherwise hold.
    if matches!(settings, ImportSettings::KeyPose(_)) {
        dependencies.push(marker(format!("#name {}", keypose::stem(path))));
    }
    dependencies
}

/// Imports, bakes and cooks one source, or the sub-asset `part` of it: the bytes of its cooked file.
fn cook_source(
    path: &Path,
    bytes: &[u8],
    settings: &ImportSettings,
    part: &Part,
    skeleton: Option<AssetId>,
    cook: Cook,
    assets: &Assets<'_>,
) -> Result<Vec<u8>, CookError> {
    let clip_error =
        |source: ClipCookError| CookError::Clip { path: path.to_owned(), source: Box::new(source) };
    match part {
        Part::Own | Part::Rig | Part::KeyPose { .. } | Part::FontSubset { .. } => {}
        Part::Cell(place) => {
            let ImportSettings::Zone(settings) = settings else {
                unreachable!("only zones plan cells");
            };
            return cook_cell(path, *place, settings, cook, assets);
        }
        Part::Skeleton => {
            let joints = if settings.is_animation_only() {
                assets.clip_sources.get(path, bytes, settings).map_err(clip_error)?.joints.clone()
            } else {
                let _held = assets.memory.hold(mesh_memory(path, bytes, settings));
                let mesh = imported_mesh(path, bytes, settings, cook)?;
                mesh.skin.ok_or_else(|| clip_error(ClipCookError::NoSkeleton))?.joints
            };
            let cooked = Skeleton::new(&joints).to_cooked().map_err(|error| clip_error(error.into()))?;
            // The runtime's checks, so a cook never writes a skeleton animation would refuse.
            Skeleton::read(&cooked).map_err(|error| clip_error(error.into()))?;
            return Ok(cooked.into_vec());
        }
        Part::Clip { index, target, .. } => {
            let clips = settings.clips().expect("clips are planned from these settings");
            let input = ClipInput { path, bytes, importer: settings, clips, index: *index, skeleton };
            return cook_clip(&input, target.map(|entry| &clips.retarget[entry]), assets).map_err(clip_error);
        }
    }
    match settings {
        ImportSettings::Gltf(_)
        | ImportSettings::Fbx(_)
        | ImportSettings::Vrm(_)
        | ImportSettings::Assembly(_)
            if *part == Part::Own =>
        {
            let shared = shared_skeleton(settings, assets).map_err(clip_error)?;
            let _held = assets.memory.hold(mesh_memory(path, bytes, settings));
            let mut mesh = import_mesh(path, bytes, settings, cook, shared.as_ref())?;
            if let Some(skin) = &mut mesh.skin {
                skin.skeleton = skeleton;
            }
            Ok(cooked_bytes(path, &mesh)?.into_vec())
        }
        ImportSettings::Vrm(vrm_settings) => {
            debug_assert_eq!(*part, Part::Rig, "a VRM's other sub-asset is its rig");
            // Reading a rig imports the file's whole scene.
            let _held = assets.memory.hold(mesh_memory(path, bytes, settings));
            let rig = vrm::import_rig(bytes, vrm_settings.version).map_err(vrm_error(path))?;
            let invalid = |source| CookError::InvalidRig { path: path.to_owned(), source };
            let cooked = rig.to_cooked().map_err(|error| invalid(RigError::Cooked(error)))?;
            // The runtime's checks, so a cook never writes a rig animation would refuse.
            CharacterRig::read(&cooked).map_err(invalid)?;
            Ok(cooked.into_vec())
        }
        ImportSettings::Gltf(_) | ImportSettings::Fbx(_) | ImportSettings::Assembly(_) => {
            unreachable!("rigs are VRM sources' alone")
        }
        ImportSettings::GltfClips(_) | ImportSettings::FbxClips(_) | ImportSettings::Vrma(_) => {
            unreachable!("animation sources have their clips and skeleton, and nothing of their own")
        }
        ImportSettings::Texture(settings) => {
            let _held = assets.memory.hold(texture::cook_memory(path, bytes));
            texture::cook_source(path, bytes, settings, cook)
                .map_err(|source| CookError::Texture { path: path.to_owned(), source: Box::new(source) })
        }
        ImportSettings::CharacterMap(settings) => {
            let _held = assets.memory.hold(map_memory(path, bytes, settings));
            character_maps::cook_source(path, bytes, settings, cook).map_err(map_error(path))
        }
        ImportSettings::Material(_) => material::read(bytes)
            .and_then(|document| material::cook(&document, |id| assets.referenced(id)))
            .map_err(|source| CookError::Material { path: path.to_owned(), source: Box::new(source) }),
        ImportSettings::ProbeGrid(_) => probes::read(bytes)
            .and_then(|document| probes::cook(&document, cook, |id| assets.probe_input(id)))
            .map_err(probe_error(path)),
        ImportSettings::KeyPose(settings) => {
            let rig = if let Part::KeyPose { rig, .. } = part { Some(*rig) } else { settings.rig };
            cook_key_pose(path, bytes, settings, rig, assets).map_err(clip_error)
        }
        ImportSettings::Colliders(_) => colliders::cook(bytes)
            .map_err(|source| CookError::Colliders { path: path.to_owned(), source: Box::new(source) }),
        ImportSettings::Sound(_) => audio::cook_sound(bytes).map_err(audio_error(path)),
        ImportSettings::Cue(_) => audio::cook_cue(bytes).map_err(audio_error(path)),
        ImportSettings::Font(settings) => {
            font::cook(path, bytes, settings, font_subset_of(part)).map_err(font_error(path))
        }
        ImportSettings::FontChains(_) => font::cook_chains(bytes).map_err(font_error(path)),
        ImportSettings::MoveSet(_) => moves::cook(bytes, |clip| move_tables(clip, assets))
            .map_err(|source| CookError::MoveSet { path: path.to_owned(), source: Box::new(source) }),
        ImportSettings::AnimGraph(_) => anim_graph::cook(bytes)
            .map_err(|source| CookError::AnimGraph { path: path.to_owned(), source: Box::new(source) }),
        ImportSettings::Zone(_) => cook_zone(path, assets),
        ImportSettings::Locale(_) => {
            let files = fluent_of(path, assets);
            loc::cook(path, bytes, files)
                .map_err(|source| CookError::Locale { path: path.to_owned(), source: Box::new(source) })
        }
        // Never planned: it cooks into its locale's table.
        ImportSettings::Fluent(_) => Err(CookError::UnsupportedSource { path: path.to_owned() }),
    }
}

/// A zone's merged mesh for `place`, as cooked bytes.
fn cook_cell(
    path: &Path,
    place: Place,
    settings: &zone::ZoneSettings,
    cook: Cook,
    assets: &Assets<'_>,
) -> Result<Vec<u8>, CookError> {
    let plan = &assets.zones[path];
    let _held = assets.memory.hold(plan.source_bytes_in(place) * MESH_MEMORY_PER_SOURCE_BYTE);
    let (mut mesh, bindings) = plan.merge(place, assembly_detail(cook)).map_err(zone_error(path))?;
    if let Some(vertex) = mesh.vertices.iter().position(|v| !v.position.iter().all(|c| c.is_finite())) {
        return Err(CookError::Invalid {
            path: path.to_owned(),
            source: MeshError::NonFinitePosition { vertex },
        });
    }
    bake::outline_normals(&mut mesh, &settings.outline);
    optimize::optimize(&mut mesh, &settings.lods, settings.outline.weld_tolerance(), cook);
    let packed = mesh
        .pack(settings.position_format, &bindings)
        .map_err(|source| CookError::MaterialSlot { path: path.to_owned(), source })?;
    Ok(cooked_bytes(path, &packed)?.into_vec())
}

/// A zone's `.szone`, from its plan and its cooked meshes.
fn cook_zone(path: &Path, assets: &Assets<'_>) -> Result<Vec<u8>, CookError> {
    let plan = &assets.zones[path];
    let bounds_of = |id: AssetId| -> Result<sapphire_zone::Bounds, CookError> {
        let cooked = assets.cooked(id).ok_or_else(|| CookError::ZoneReference {
            path: path.to_owned(),
            field: "instances".to_owned(),
            what: "a mesh",
            id,
        })?;
        let read_error = |source| CookError::Read { path: cooked.output.clone(), source };
        let bytes = read_cooked(cooked).map_err(read_error)?;
        let mesh = Mesh::read(&bytes)
            .map_err(|source| CookError::Invalid { path: cooked.output.clone(), source })?;
        let sapphire_assets::mesh::Bounds { min, max } = mesh.streams.bounds.to_native();
        Ok(sapphire_zone::Bounds { min, max })
    };
    let places = plan
        .places()
        .map(|place| {
            let id = plan.part_id(place);
            Ok(CookedPlace { place, id, bounds: bounds_of(id)? })
        })
        .collect::<Result<Vec<_>, CookError>>()?;
    let instances = plan
        .document()
        .instances
        .iter()
        .map(|instance| bounds_of(instance.mesh))
        .collect::<Result<Vec<_>, CookError>>()?;
    zone::cook(plan, path, places, instances).map_err(zone_error(path))
}

/// Maps a zone's cook error to one that names the source.
fn zone_error(path: &Path) -> impl Fn(ZoneCookError) -> CookError + '_ {
    |source| CookError::Zone { path: path.to_owned(), source: Box::new(source) }
}

/// About what cooking the map document in `bytes` holds at its peak: a texture of its first input's size,
/// since a map is its inputs' size and they must share it, and [`MAP_MEMORY`] at least. An input that
/// doesn't read counts for nothing here: its cook is about to say so.
fn map_memory(path: &Path, bytes: &[u8], settings: &character_maps::MapSettings) -> u64 {
    let first =
        character_maps::inputs(settings.kind(), bytes).ok().and_then(|inputs| inputs.into_iter().next());
    let input = first.and_then(|input| character_maps::resolve_input(path, &input).ok());
    let holds = input.and_then(|input| Some(texture::cook_memory(&input, &fs::read(&input).ok()?)));
    holds.unwrap_or(0).max(MAP_MEMORY)
}

/// About what importing and cooking the mesh source in `bytes` holds at its peak, from all the source
/// bytes it reads ([`MESH_MEMORY_PER_SOURCE_BYTE`]). Whatever else it names that doesn't read counts for
/// nothing here: its cook is about to say so.
fn mesh_memory(path: &Path, bytes: &[u8], settings: &ImportSettings) -> u64 {
    let others = match settings {
        // VRM files are GLB, and the FBX importer never opens the files an FBX names.
        ImportSettings::Vrm(_) | ImportSettings::Fbx(_) => 0,
        ImportSettings::Assembly(_) => assembly_piece_bytes(path, bytes),
        _ => external_buffer_bytes(path, bytes),
    };
    (bytes.len() as u64).saturating_add(others).saturating_mul(MESH_MEMORY_PER_SOURCE_BYTE)
}

/// The sizes of the pieces the assembly at `path` places, with their external buffers.
fn assembly_piece_bytes(path: &Path, bytes: &[u8]) -> u64 {
    let files = assembly::read(bytes).and_then(|assembly| assembly.files()).unwrap_or_default();
    let piece_bytes = |file: String| {
        let full = character_maps::resolve_input(path, &file).ok()?;
        let piece = fs::read(&full).ok()?;
        Some((piece.len() as u64).saturating_add(external_buffer_bytes(&full, &piece)))
    };
    files.into_iter().filter_map(piece_bytes).fold(0, u64::saturating_add)
}

/// The sizes of the external buffers of the glTF at `path`.
fn external_buffer_bytes(path: &Path, bytes: &[u8]) -> u64 {
    let dir = path.parent().unwrap_or(Path::new(""));
    let files = gltf::external_files(bytes).unwrap_or_default();
    files
        .iter()
        .filter_map(|file| fs::metadata(dir.join(file)).ok())
        .map(|file| file.len())
        .fold(0, u64::saturating_add)
}

/// Cooks the document in `bytes` for the rig of source `rig`.
fn cook_key_pose(
    path: &Path,
    bytes: &[u8],
    settings: &keypose::KeyPoseSettings,
    rig: Option<AssetId>,
    assets: &Assets<'_>,
) -> Result<Vec<u8>, ClipCookError> {
    let samples = key_pose_samples(path, bytes, rig, assets)?;
    written_clip(&clip::cook(&samples.sampled, &samples.target(), &settings.error))
}

/// The samples of the document in `bytes`, for the rig of source `rig`.
fn key_pose_samples(
    path: &Path,
    bytes: &[u8],
    rig: Option<AssetId>,
    assets: &Assets<'_>,
) -> Result<ClipSamples, ClipCookError> {
    let document = keypose::read(bytes)?;
    let source = assets.key_pose_rig(rig)?;
    let joints = source.read_skeleton()?;
    let map = source.map(&joints)?.ok_or(KeyPoseError::NotHumanoid)?;
    let targets = morph_targets_of(&document, source.mesh)?;
    let sampled = keypose::sample(&document, &keypose::stem(path), &joints, &map, &targets)?;
    Ok(ClipSamples {
        sampled,
        skeleton: Some(joints),
        skeleton_id: Some(source.skeleton_id),
        morph_defaults: targets.iter().map(|target| target.default_weight).collect(),
    })
}

/// The morph targets a document's weights name on `mesh`: none if it names no weights or there is
/// no mesh, which leaves them out of a rig without targets, as a bone it lacks is. Reading a cooked
/// mesh takes it whole, so only a document that needs it does.
fn morph_targets_of(
    document: &keypose::KeyPoseClip,
    mesh: Option<&CookedAsset>,
) -> Result<Vec<keypose::MorphTarget>, ClipCookError> {
    let Some(mesh) = mesh.filter(|_| document.names_morphs()) else { return Ok(Vec::new()) };
    let bytes = read_cooked(mesh).map_err(KeyPoseError::Read)?;
    let mesh = Mesh::read(&bytes).map_err(KeyPoseError::Mesh)?;
    Ok(mesh
        .morph_targets
        .iter()
        .map(|target| keypose::MorphTarget {
            name: target.name.as_str().to_owned(),
            default_weight: target.default_weight.to_native(),
        })
        .collect())
}

/// The skeleton a mesh source's sidecar says it shares, cooked.
fn shared_skeleton(
    settings: &ImportSettings,
    assets: &Assets<'_>,
) -> Result<Option<Skeleton>, ClipCookError> {
    settings.shared_rig().map(|id| assets.shared_rig_source(id)?.read_skeleton()).transpose()
}

/// `clip` as cooked bytes, read back through the runtime's checks ([`Clip::read`]), so a cook never
/// writes a clip the runtime would refuse.
fn written_clip(clip: &Clip) -> Result<Vec<u8>, ClipCookError> {
    let cooked = clip.to_cooked()?;
    Clip::read(&cooked)?;
    Ok(cooked.into_vec())
}

/// One clip of a source, as its cook reads it.
struct ClipInput<'a> {
    path: &'a Path,
    bytes: &'a [u8],
    importer: &'a ImportSettings,
    clips: &'a ClipSettings,
    /// The animation in the file.
    index: usize,
    /// What the plan says the source's clips name: its skeleton's ID, or the one it shares.
    skeleton: Option<AssetId>,
}

/// Cooks animation `input.index` for the source's own rig, or for `retarget`'s.
fn cook_clip(
    input: &ClipInput<'_>,
    retarget: Option<&retarget::RetargetSettings>,
    assets: &Assets<'_>,
) -> Result<Vec<u8>, ClipCookError> {
    let samples = source_samples(input, retarget, assets)?;
    written_clip(&clip::cook(&samples.sampled, &samples.target(), &input.clips.error))
}

/// The samples of animation `input.index`, for the source's own rig or for `retarget`'s. The file
/// imports once for all its clips ([`ClipSources`]).
fn source_samples(
    input: &ClipInput<'_>,
    retarget: Option<&retarget::RetargetSettings>,
    assets: &Assets<'_>,
) -> Result<ClipSamples, ClipCookError> {
    let source = assets.clip_sources.get(input.path, input.bytes, input.importer)?;
    let sampled = source.sample(input.index, input.clips)?;
    let joints = (!source.joints.is_empty()).then(|| Skeleton::new(&source.joints));
    let Some(retarget) = retarget else {
        return match input.importer.shared_rig() {
            Some(id) => {
                let shared = assets.shared_rig_source(id)?;
                let skeleton = shared.read_skeleton()?;
                let remap = joint_remap(&source.joints, &skeleton)?;
                Ok(ClipSamples {
                    sampled: remap_tracks(sampled, &remap),
                    skeleton: Some(skeleton),
                    skeleton_id: Some(shared.skeleton_id),
                    morph_defaults: source.morph_defaults.clone(),
                })
            }
            None => Ok(ClipSamples {
                sampled,
                skeleton: joints,
                skeleton_id: input.skeleton,
                morph_defaults: source.morph_defaults.clone(),
            }),
        };
    };
    let joints = joints.ok_or(ClipCookError::NoSkeleton)?;
    let map = source_map(input, &source, &joints, assets)?;
    let id = retarget.target;
    let target_rig =
        assets.rig_source(id).filter(RigSource::has_map).ok_or(ClipCookError::UnknownTarget { id })?;
    let target_joints = target_rig.read_skeleton()?;
    let target_map = target_rig.map(&target_joints)?.ok_or(ClipCookError::UnknownTarget { id })?;
    let retargeted = retarget::retarget(
        &sampled,
        Rig { skeleton: &joints, map: &map },
        Rig { skeleton: &target_joints, map: &target_map },
        &RetargetOptions {
            scale: retarget.scale,
            ground: retarget.ground,
            adjust: retarget.adjust.clone(),
            gain: retarget.gain.clone(),
            follow: retarget.follow.clone(),
        },
    )?;
    Ok(ClipSamples {
        sampled: retargeted,
        skeleton: Some(target_joints),
        skeleton_id: Some(target_rig.skeleton_id),
        morph_defaults: Vec::new(),
    })
}

/// The humanoid map a source retargets from, on its own joints: a VRMA's, its sidecar's, or the
/// sidecar's of the source whose rig it shares.
fn source_map(
    input: &ClipInput<'_>,
    source: &ClipSource,
    joints: &Skeleton,
    assets: &Assets<'_>,
) -> Result<HumanMap, ClipCookError> {
    if let Some(map) = &source.map {
        return Ok(map.clone());
    }
    let names = match input.importer.shared_rig() {
        Some(id) => assets.shared_rig_source(id)?.names,
        None => Some(&input.clips.humanoid).filter(|names| !names.is_empty()),
    };
    Ok(HumanMap::from_names(names.ok_or(ClipCookError::NoHumanoid)?, joints)?)
}

/// Wraps a sound or cue error as the cook error for the source at `path`.
fn audio_error(path: &Path) -> impl Fn(AudioCookError) -> CookError + '_ {
    move |source| CookError::Audio { path: path.to_owned(), source: Box::new(source) }
}

/// Wraps a font error as the cook error for the file at `path`.
fn font_error(path: &Path) -> impl Fn(FontCookError) -> CookError + '_ {
    move |source| CookError::Font { path: path.to_owned(), source: Box::new(source) }
}

/// Which entry of a font's `subsets` a part of it is: the first is its own asset.
fn font_subset_of(part: &Part) -> usize {
    if let Part::FontSubset { index, .. } = part { *index } else { 0 }
}

/// Wraps a probe grid error as the cook error for the document at `path`.
fn probe_error(path: &Path) -> impl Fn(ProbeCookError) -> CookError + '_ {
    move |source| CookError::ProbeGrid { path: path.to_owned(), source: Box::new(source) }
}

/// Wraps a VRM import error as the cook error for the source at `path`.
fn vrm_error(path: &Path) -> impl Fn(VrmImportError) -> CookError + '_ {
    move |source| CookError::Vrm { path: path.to_owned(), source: Box::new(source) }
}

/// Wraps a map error as the cook error for the document at `path`.
fn map_error(path: &Path) -> impl Fn(MapError) -> CookError + '_ {
    move |source| CookError::Map { path: path.to_owned(), source: Box::new(source) }
}

/// The images that the map documents in `tree` read: inputs of those assets rather than
/// assets of their own. Names that aren't images are left for the map's cook to reject.
///
/// An input inside the content tree must name a scanned file exactly, case included, so a
/// cook claims the same files on every file system. It must not have a sidecar either: that
/// would make it an asset of its own, which claiming it would silently drop.
///
/// A map whose sidecar says `cook: false` is looked at before its document (review BR-001): no
/// cook reads it, so nothing in it can fail the cook, but the images it names without sidecars
/// of their own are still claimed, as far as its document reads, so they stay out of the cook
/// rather than become textures of their own.
fn map_inputs(source_root: &Path, tree: &SourceTree) -> Result<HashSet<PathBuf>, CookError> {
    let sources: HashSet<&Path> = tree.sources.iter().map(PathBuf::as_path).collect();
    let sidecars: HashSet<&Path> = tree.sidecars.iter().map(PathBuf::as_path).collect();
    let mut inputs = HashSet::new();
    for document in &tree.sources {
        let Some(kind) = MapKind::for_path(document) else { continue };
        let sidecar = meta::sidecar_path(document);
        // A sidecar that doesn't parse is left for `meta::resolve` to report.
        if sidecars.contains(sidecar.as_path()) && meta::read(&sidecar).is_ok_and(|meta| !meta.cook) {
            inputs.extend(uncooked_map_inputs(document, kind, &sidecars));
            continue;
        }
        let fail = map_error(document);
        let bytes =
            fs::read(document).map_err(|source| CookError::Read { path: document.clone(), source })?;
        let mut with_sidecars = Vec::new();
        for input in character_maps::inputs(kind, &bytes).map_err(&fail)? {
            let path = character_maps::resolve_input(document, &input).map_err(&fail)?;
            if image::SourceFormat::from_path(&path).is_none() {
                continue;
            }
            let in_tree = path.strip_prefix(source_root).is_ok_and(|relative| !is_hidden(relative));
            if in_tree && !sources.contains(path.as_path()) {
                return Err(fail(MapError::MissingInput(path)));
            }
            if sidecars.contains(meta::sidecar_path(&path).as_path()) {
                with_sidecars.push(path.clone());
            }
            inputs.insert(path);
        }
        if !with_sidecars.is_empty() {
            return Err(fail(MapError::InputsWithSidecars(with_sidecars)));
        }
    }
    Ok(inputs)
}

/// The images a map that doesn't cook names, besides those with sidecars of their own (assets
/// of their own): none if its document can't be read, and unchecked otherwise.
fn uncooked_map_inputs(document: &Path, kind: MapKind, sidecars: &HashSet<&Path>) -> Vec<PathBuf> {
    let names = fs::read(document).ok().and_then(|bytes| character_maps::inputs(kind, &bytes).ok());
    names
        .unwrap_or_default()
        .iter()
        .filter_map(|input| character_maps::resolve_input(document, input).ok())
        .filter(|path| image::SourceFormat::from_path(path).is_some())
        .filter(|path| !sidecars.contains(meta::sidecar_path(path).as_path()))
        .collect()
}

/// Whether a scan skips `relative`: some component of it is hidden, as [`SourceTree::scan`]
/// treats names starting with a dot.
fn is_hidden(relative: &Path) -> bool {
    relative.components().any(|component| {
        matches!(component, std::path::Component::Normal(name) if name.to_str().is_some_and(|name| name.starts_with('.')))
    })
}

/// Everything besides its own bytes that cooking `path` reads: part of its cache key. Also
/// checks the asset references the source makes, on every cook, cached or not, since a
/// referenced asset can change or disappear while the source stays the same.
fn dependencies(
    path: &Path,
    bytes: &[u8],
    settings: &ImportSettings,
    part: &Part,
    assets: &Assets<'_>,
) -> Result<Vec<Dependency>, CookError> {
    let mut rigs = rig_dependencies(settings, part, assets)
        .map_err(|source| CookError::Clip { path: path.to_owned(), source: Box::new(source) })?;
    rigs.extend(source_dependencies(path, bytes, settings, part, assets)?);
    Ok(rigs)
}

/// The cooked skeletons and rigs a part of a source reads: the skeleton it shares, the map it reads
/// through another source's sidecar, and what a retarget targets.
fn rig_dependencies(
    settings: &ImportSettings,
    part: &Part,
    assets: &Assets<'_>,
) -> Result<Vec<Dependency>, ClipCookError> {
    let mut dependencies = Vec::new();
    let reads_shared =
        matches!(part, Part::Own | Part::Clip { .. }) && !matches!(settings, ImportSettings::KeyPose(_));
    if let Some(id) = settings.shared_rig().filter(|_| reads_shared) {
        let rig = assets.shared_rig_source(id)?;
        dependencies.extend(rig.dependencies("shared"));
    }
    if let (Part::Clip { target: Some(entry), .. }, Some(clips)) = (part, settings.clips()) {
        let id = clips.retarget[*entry].target;
        let rig = assets.rig_source(id).filter(RigSource::has_map);
        dependencies.extend(rig.ok_or(ClipCookError::UnknownTarget { id })?.dependencies("target"));
    }
    Ok(dependencies)
}

/// What cooking a source reads besides its own bytes and the rigs of [`rig_dependencies`].
fn source_dependencies(
    path: &Path,
    bytes: &[u8],
    settings: &ImportSettings,
    part: &Part,
    assets: &Assets<'_>,
) -> Result<Vec<Dependency>, CookError> {
    match settings {
        ImportSettings::Gltf(_)
        | ImportSettings::Fbx(_)
        | ImportSettings::Vrm(_)
        | ImportSettings::Assembly(_) => {
            let mesh = settings.mesh().expect("mesh importers have mesh settings");
            // The mesh holds only the bound materials' IDs, so their contents aren't inputs.
            for (slot, &id) in &mesh.materials {
                if !assets.get(id).is_some_and(Planned::is_material) {
                    return Err(CookError::NotAMaterial { path: path.to_owned(), slot: slot.clone(), id });
                }
            }
            if matches!(settings, ImportSettings::Vrm(_) | ImportSettings::Fbx(_)) {
                // VRM files are GLB, and the FBX importer never opens the files an FBX names:
                // they read nothing besides themselves.
                return Ok(Vec::new());
            }
            if matches!(settings, ImportSettings::Assembly(_)) {
                return assembly_dependencies(path, bytes);
            }
            let files = gltf::external_files(bytes)
                .map_err(|source| CookError::Import { path: path.to_owned(), source })?;
            let dir = path.parent().unwrap_or(Path::new(""));
            files
                .into_iter()
                .map(|file| {
                    let full = dir.join(&file);
                    let bytes = fs::read(&full).map_err(|source| CookError::Read { path: full, source })?;
                    Ok(Dependency::File { path: file, hash: blake3::hash(&bytes) })
                })
                .collect()
        }
        // Images and collider documents are self-contained.
        ImportSettings::Texture(_) | ImportSettings::Colliders(_) | ImportSettings::Fluent(_) => {
            Ok(Vec::new())
        }
        ImportSettings::Zone(_) => zone_dependencies(path, part, assets),
        // A locale's table is its sources': another locale's aren't in its key (`loc`'s docs).
        ImportSettings::Locale(_) => fluent_of(path, assets)
            .iter()
            .map(|file| {
                let bytes = fs::read(&file.path)
                    .map_err(|source| CookError::Read { path: file.path.clone(), source })?;
                Ok(Dependency::File { path: file.name.clone(), hash: blake3::hash(&bytes) })
            })
            .collect(),
        ImportSettings::MoveSet(_) => Ok(move_set_dependencies(bytes, assets)),
        ImportSettings::CharacterMap(settings) => {
            let inputs = character_maps::inputs(settings.kind(), bytes).map_err(map_error(path))?;
            inputs
                .into_iter()
                .map(|input| {
                    let full = character_maps::resolve_input(path, &input).map_err(map_error(path))?;
                    let bytes = fs::read(&full).map_err(|source| CookError::Read { path: full, source })?;
                    Ok(Dependency::File { path: input, hash: blake3::hash(&bytes) })
                })
                .collect()
        }
        ImportSettings::Material(_) => material::read(bytes)
            .and_then(|document| material::dependencies(&document, |id| assets.referenced(id)))
            .map_err(|source| CookError::Material { path: path.to_owned(), source: Box::new(source) }),
        ImportSettings::ProbeGrid(_) => probes::read(bytes)
            .and_then(|document| probes::dependencies(&document, |id| assets.probe_input(id)))
            .map_err(probe_error(path)),
        ImportSettings::KeyPose(settings) => {
            let clip_error =
                |source: ClipCookError| CookError::Clip { path: path.to_owned(), source: Box::new(source) };
            let id = if let Part::KeyPose { rig, .. } = part { Some(*rig) } else { settings.rig };
            let rig = assets.key_pose_rig(id).map_err(|source| clip_error(source.into()))?;
            let document = keypose::read(bytes).map_err(|source| clip_error(source.into()))?;
            let mut dependencies = rig.dependencies("key pose");
            // Only a document that names morph weights reads the mesh.
            if document.names_morphs()
                && let Some(mesh) = rig.mesh
            {
                dependencies.push(Dependency::Asset { id: mesh.id, key: mesh.key });
            }
            Ok(dependencies)
        }
        // A sound is the one file it reads.
        ImportSettings::Sound(_) => Ok(Vec::new()),
        // A cue holds only its sounds' IDs, so their contents aren't inputs.
        ImportSettings::Cue(_) => {
            for id in audio::read_cue(bytes).map_err(audio_error(path))?.sounds {
                if !assets.get(id).is_some_and(Planned::is_sound) {
                    return Err(CookError::NotASound { path: path.to_owned(), id });
                }
            }
            Ok(Vec::new())
        }
        ImportSettings::Font(settings) => {
            font::dependencies(path, settings, font_subset_of(part)).map_err(font_error(path))
        }
        // Chains hold only their fonts' IDs, so their contents aren't inputs.
        ImportSettings::FontChains(_) => {
            let chains = font::read_chains(bytes).map_err(font_error(path))?;
            for id in chains.chains.iter().flat_map(|chain| &chain.scripts).flat_map(|entry| &entry.fonts) {
                if !assets.get(*id).is_some_and(Planned::is_font) {
                    return Err(CookError::NotAFont { path: path.to_owned(), id: *id });
                }
            }
            Ok(Vec::new())
        }
        // A graph holds only its clips' IDs, so their contents aren't inputs.
        ImportSettings::AnimGraph(_) => {
            let graph = anim_graph::read(bytes)
                .map_err(|source| CookError::AnimGraph { path: path.to_owned(), source: Box::new(source) })?;
            for clip in graph.clips {
                if !assets.get(clip.id).is_some_and(Planned::is_clip) {
                    return Err(CookError::NotAClip { path: path.to_owned(), name: clip.name, id: clip.id });
                }
            }
            Ok(Vec::new())
        }
        ImportSettings::GltfClips(_) | ImportSettings::Vrma(_) => gltf::external_files(bytes)
            .map_err(|source| CookError::Import { path: path.to_owned(), source })?
            .into_iter()
            .map(|file| {
                let full = path.parent().unwrap_or(Path::new("")).join(&file);
                let bytes = fs::read(&full).map_err(|source| CookError::Read { path: full, source })?;
                Ok(Dependency::File { path: file, hash: blake3::hash(&bytes) })
            })
            .collect(),
        // The importer never opens the files an FBX names.
        ImportSettings::FbxClips(_) => Ok(Vec::new()),
    }
}

/// The clips a move set's document names and their rigs, by their own keys. A document that doesn't
/// read, or a clip that doesn't resolve, keys nothing here: its cook fails by move and track, with
/// everything else wrong with the document.
fn move_set_dependencies(bytes: &[u8], assets: &Assets<'_>) -> Vec<Dependency> {
    let mut dependencies = Vec::new();
    for planned in clips_of_set(bytes, assets) {
        if let Some(cooked) = assets.cooked(planned.id) {
            dependencies.push(Dependency::Asset { id: cooked.id, key: cooked.key });
        }
        if let Some(rig) = assets.rig_of_clip(planned) {
            dependencies.extend(rig.dependencies("move"));
        }
    }
    dependencies
}

/// The clips a move set's document names, as far as it reads and they resolve.
fn clips_of_set<'a>(bytes: &[u8], assets: &Assets<'a>) -> Vec<&'a Planned> {
    let Ok(document) = moves::read(bytes) else { return Vec::new() };
    let clips = document.moves.iter().filter_map(|mv| mv.motion.clip.as_ref());
    clips.filter_map(|clip| assets.clip_of_move(clip).ok()).collect()
}

/// What a zone's part reads besides its own document, and the check of what it names, on every cook.
fn zone_dependencies(path: &Path, part: &Part, assets: &Assets<'_>) -> Result<Vec<Dependency>, CookError> {
    let plan = &assets.zones[path];
    if let Part::Cell(place) = part {
        for (field, id) in plan.materials_in(*place) {
            if !assets.get(id).is_some_and(Planned::is_material) {
                return Err(CookError::NotAMaterial { path: path.to_owned(), slot: field, id });
            }
        }
        let mut dependencies = Vec::new();
        for (id, prefab, hash) in plan.prefab_files_in(*place) {
            let bytes =
                fs::read(prefab).map_err(|source| CookError::Read { path: prefab.to_owned(), source })?;
            // What's merged is the document the plan read: a save since then cooks again from the top.
            if blake3::hash(&bytes) != hash {
                return Err(CookError::SourceChanged { path: prefab.to_owned() });
            }
            dependencies.push(Dependency::File { path: format!("#prefab {id}"), hash });
            dependencies.extend(assembly_dependencies(prefab, &bytes)?);
        }
        return Ok(dependencies);
    }

    let document = plan.document();
    let reference = |field: String, what, id, good: fn(&Planned) -> bool| {
        if assets.get(id).is_some_and(good) {
            Ok(())
        } else {
            Err(CookError::ZoneReference { path: path.to_owned(), field, what, id })
        }
    };
    if let Some(id) = document.colliders {
        reference("colliders".to_owned(), "a collider document", id, |planned| {
            matches!(planned.asset.meta.importer, ImportSettings::Colliders(_))
        })?;
    }
    if let Some(id) = document.probes {
        reference("probes".to_owned(), "a probe grid", id, Planned::is_probe_grid)?;
    }
    let mut dependencies = Vec::new();
    let cooked_key = |id: AssetId, field: String, what| {
        let cooked = assets.cooked(id);
        let cooked = cooked.ok_or(CookError::ZoneReference { path: path.to_owned(), field, what, id })?;
        Ok::<Dependency, CookError>(Dependency::Asset { id, key: cooked.key })
    };
    for (index, instance) in document.instances.iter().enumerate() {
        let field = format!("instances[{index}].mesh");
        reference(field.clone(), "a static mesh", instance.mesh, |planned| {
            planned.recipe.extension == MESH_EXTENSION
                && planned.skeleton.is_none()
                && !matches!(planned.part, Part::Cell(_))
        })?;
        dependencies.push(cooked_key(instance.mesh, field, "a static mesh")?);
    }
    for place in plan.places() {
        dependencies.push(cooked_key(plan.part_id(place), zone::part_name(place), "a mesh")?);
    }
    for (field, file) in [("day", &document.day), ("rigs", &document.rigs)] {
        if let Some(file) = file {
            let (_, bytes) = zone::read_document(path, field, file).map_err(zone_error(path))?;
            dependencies.push(Dependency::File { path: file.clone(), hash: blake3::hash(&bytes) });
        }
    }
    Ok(dependencies)
}

/// The pieces an assembly at `path` places: files it reads, by the path it names them with.
fn assembly_dependencies(path: &Path, bytes: &[u8]) -> Result<Vec<Dependency>, CookError> {
    let fail = |source| CookError::Assembly { path: path.to_owned(), source: Box::new(source) };
    let assembly = assembly::read(bytes).map_err(fail)?;
    let mut dependencies = Vec::new();
    for file in assembly.files().map_err(fail)? {
        let full = character_maps::resolve_input(path, &file)
            .map_err(|_| fail(AssemblyError::PiecePath(file.clone())))?;
        let piece = fs::read(&full).map_err(|source| CookError::Read { path: full.clone(), source })?;
        // A piece's external buffers are read with it.
        let dir = Path::new(&file).parent().unwrap_or(Path::new(""));
        for buffer in
            gltf::external_files(&piece).map_err(|source| CookError::Import { path: full.clone(), source })?
        {
            let buffer_path = full.parent().unwrap_or(Path::new("")).join(&buffer);
            let bytes =
                fs::read(&buffer_path).map_err(|source| CookError::Read { path: buffer_path, source })?;
            let name = dir.join(&buffer).to_string_lossy().replace('\\', "/");
            dependencies.push(Dependency::File { path: name, hash: blake3::hash(&bytes) });
        }
        dependencies.push(Dependency::File { path: file, hash: blake3::hash(&piece) });
    }
    Ok(dependencies)
}

/// The `.ftl` sources of the locale document at `path`.
fn fluent_of<'a>(path: &Path, assets: &Assets<'a>) -> &'a [FluentFile] {
    assets.fluent.get(loc::locale_of_document(path)).map_or(&[], Vec::as_slice)
}

/// The detail an assembly's shapes are built in for `cook`: the mobile cook's lighter.
fn assembly_detail(cook: Cook) -> assembly::Detail {
    match cook {
        Cook::Desktop => assembly::Detail::Full,
        Cook::Mobile => assembly::Detail::Light,
    }
}

/// Imports, bakes, optimizes and packs one mesh source read from `path` with `importer` (glTF,
/// FBX, VRM or an assembly), for `cook`.
///
/// With a `shared` skeleton the mesh is built on its own joints, and its palette then follows the
/// same joints in `shared`'s order.
fn import_mesh(
    path: &Path,
    bytes: &[u8],
    importer: &ImportSettings,
    cook: Cook,
    shared: Option<&Skeleton>,
) -> Result<Mesh, CookError> {
    let mut mesh = imported_mesh(path, bytes, importer, cook)?;
    let clip_error =
        |source: ClipCookError| CookError::Clip { path: path.to_owned(), source: Box::new(source) };
    let remap = shared
        .map(|shared| {
            let skin = mesh.skin.as_ref().ok_or_else(|| clip_error(ClipCookError::NoSkeleton))?;
            joint_remap(&skin.joints, shared).map_err(|source| clip_error(source.into()))
        })
        .transpose()?;
    let settings = importer.mesh().expect("only mesh importers import a mesh");
    // Packing would quantize a NaN to a corner of the bounds. glTF's importer says which primitive;
    // this holds for the others.
    if let Some(vertex) = mesh.vertices.iter().position(|v| !v.position.iter().all(|c| c.is_finite())) {
        return Err(CookError::Invalid {
            path: path.to_owned(),
            source: MeshError::NonFinitePosition { vertex },
        });
    }
    bake::outline_normals(&mut mesh, &settings.outline);
    if let Some(foliage) = &settings.foliage {
        bake::foliage::bake(&mut mesh, foliage, settings.outline.weld_tolerance())
            .map_err(|source| CookError::Foliage { path: path.to_owned(), source })?;
    }
    optimize::optimize(&mut mesh, &settings.lods, settings.outline.weld_tolerance(), cook);
    let mut packed = mesh
        .pack(settings.position_format, &settings.materials)
        .map_err(|source| CookError::MaterialSlot { path: path.to_owned(), source })?;
    if let (Some(remap), Some(shared), Some(skin)) = (remap, shared, &mut packed.skin) {
        skin.joint_count = shared.parents.len() as u32;
        for bind in &mut skin.binds {
            bind.joint = remap[usize::from(bind.joint)];
        }
    }
    Ok(packed)
}

/// The mesh a mesh importer reads from `bytes`, before anything derived from it. A glTF that cooks
/// clips counts every node its animations move as a joint.
fn imported_mesh(
    path: &Path,
    bytes: &[u8],
    importer: &ImportSettings,
    cook: Cook,
) -> Result<ImportedMesh, CookError> {
    match importer {
        ImportSettings::Gltf(settings) if settings.clips.is_some() => {
            let (_, _, scene) = gltf::clips::import_scene(bytes, path.parent()).map_err(|source| {
                CookError::Clip { path: path.to_owned(), source: Box::new(source.into()) }
            })?;
            Ok(scene.mesh)
        }
        ImportSettings::Gltf(_) => gltf::import_slice(bytes, path.parent())
            .map_err(|source| CookError::Import { path: path.to_owned(), source }),
        ImportSettings::Fbx(settings) if settings.clips.is_some() => fbx::clips::import_mesh(bytes)
            .map_err(|source| CookError::Fbx { path: path.to_owned(), source: Box::new(source) }),
        ImportSettings::Fbx(_) => fbx::import_slice(bytes)
            .map_err(|source| CookError::Fbx { path: path.to_owned(), source: Box::new(source) }),
        ImportSettings::Vrm(settings) => {
            Ok(vrm::import(bytes, settings.version).map_err(vrm_error(path))?.mesh)
        }
        ImportSettings::Assembly(_) => assembly::import(path, bytes, assembly_detail(cook))
            .map_err(|source| CookError::Assembly { path: path.to_owned(), source: Box::new(source) }),
        ImportSettings::Texture(_)
        | ImportSettings::CharacterMap(_)
        | ImportSettings::Material(_)
        | ImportSettings::ProbeGrid(_)
        | ImportSettings::KeyPose(_)
        | ImportSettings::Colliders(_)
        | ImportSettings::Sound(_)
        | ImportSettings::Cue(_)
        | ImportSettings::Font(_)
        | ImportSettings::FontChains(_)
        | ImportSettings::MoveSet(_)
        | ImportSettings::AnimGraph(_)
        | ImportSettings::Fluent(_)
        | ImportSettings::Locale(_)
        | ImportSettings::GltfClips(_)
        | ImportSettings::FbxClips(_)
        | ImportSettings::Vrma(_)
        | ImportSettings::Zone(_) => Err(CookError::UnsupportedSource { path: path.to_owned() }),
    }
}

/// `mesh` as cooked bytes, read back through the runtime's checks ([`Mesh::read`]) so a cook
/// never writes a mesh the runtime would refuse.
fn cooked_bytes(path: &Path, mesh: &Mesh) -> Result<sapphire_serialize::cooked::AlignedVec, CookError> {
    let invalid = |source| CookError::Invalid { path: path.to_owned(), source };
    let bytes = mesh.to_cooked().map_err(|error| invalid(MeshError::Cooked(error)))?;
    Mesh::read(&bytes).map_err(invalid)?;
    Ok(bytes)
}

/// Imports, bakes, optimizes, packs and validates one mesh source.
pub fn cook_mesh(source: &Path, settings: &MeshSettings, cook: Cook) -> Result<Mesh, CookError> {
    cook_mesh_with_bytes(source, settings, cook).map(|(mesh, _)| mesh)
}

/// Cooks one mesh source to `output` (a `.smesh` file), creating parent directories. It
/// reads and writes no sidecar and uses no cache.
pub fn cook_mesh_file(
    source: &Path,
    output: &Path,
    settings: &MeshSettings,
    cook: Cook,
) -> Result<(), CookError> {
    let (_, bytes) = cook_mesh_with_bytes(source, settings, cook)?;
    write_atomically(output, &bytes)
}

/// [`cook_mesh`], with the checked cooked bytes.
fn cook_mesh_with_bytes(
    source: &Path,
    settings: &MeshSettings,
    cook: Cook,
) -> Result<(Mesh, sapphire_serialize::cooked::AlignedVec), CookError> {
    // The importer by extension, with `settings` for the mesh; a VRM's version isn't checked.
    let importer = match ImportSettings::for_source(source) {
        Some(ImportSettings::Gltf(_)) => ImportSettings::Gltf(settings.clone()),
        Some(ImportSettings::Fbx(_)) => ImportSettings::Fbx(settings.clone()),
        Some(ImportSettings::Vrm(_)) => {
            ImportSettings::Vrm(VrmSettings { mesh: settings.clone(), version: None })
        }
        Some(ImportSettings::Assembly(_)) => ImportSettings::Assembly(settings.clone()),
        _ => return Err(CookError::UnsupportedSource { path: source.to_owned() }),
    };
    let bytes = fs::read(source).map_err(|e| CookError::Read { path: source.to_owned(), source: e })?;
    let mesh = guarded(source, || import_mesh(source, &bytes, &importer, cook, None))?;
    let cooked = cooked_bytes(source, &mesh)?;
    Ok((mesh, cooked))
}

/// The blake3 hash of the file at `path`, streamed, or `None` if there is none.
fn file_hash(path: &Path) -> Result<Option<blake3::Hash>, CookError> {
    let read_error = |source| CookError::Read { path: path.to_owned(), source };
    let file = match fs::File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(read_error(error)),
    };
    let mut hasher = blake3::Hasher::new();
    hasher.update_reader(file).map_err(read_error)?;
    Ok(Some(hasher.finalize()))
}

/// Removes files under `out_root` with one of `extensions`, or temporary ones, that `keep`
/// doesn't keep, then any directories that leaves empty. Returns the removed files. The cook
/// prunes its cooked files and the packer its packs.
pub(crate) fn prune(
    out_root: &Path,
    keep: &dyn Fn(&Path) -> bool,
    extensions: &[&str],
) -> Result<Vec<PathBuf>, CookError> {
    let mut removed = Vec::new();
    if out_root.is_dir() {
        prune_dir(out_root, keep, extensions, &mut removed)?;
    }
    Ok(removed)
}

/// [`prune`] for one directory; returns whether it is now empty.
fn prune_dir(
    dir: &Path,
    keep: &dyn Fn(&Path) -> bool,
    extensions: &[&str],
    removed: &mut Vec<PathBuf>,
) -> Result<bool, CookError> {
    let read_error = |source| CookError::Read { path: dir.to_owned(), source };
    let mut entries =
        fs::read_dir(dir).map_err(read_error)?.collect::<Result<Vec<_>, _>>().map_err(read_error)?;
    entries.sort_by_key(fs::DirEntry::path);
    let mut empty = true;
    for entry in entries {
        let path = entry.path();
        let is_dir =
            entry.file_type().map_err(|source| CookError::Read { path: path.clone(), source })?.is_dir();
        let remove_error = |source| CookError::Write { path: path.clone(), source };
        if is_dir {
            if prune_dir(&path, keep, extensions, removed)? {
                fs::remove_dir(&path).map_err(remove_error)?;
            } else {
                empty = false;
            }
        } else if has_extension(&path, extensions) && !keep(&path) {
            fs::remove_file(&path).map_err(remove_error)?;
            removed.push(path);
        } else {
            empty = false;
        }
    }
    Ok(empty)
}

/// Whether `path` is a temporary file or has one of `extensions`.
fn has_extension(path: &Path, extensions: &[&str]) -> bool {
    let extension = path.extension().and_then(|extension| extension.to_str());
    extension.is_some_and(|extension| extension == TEMP_EXTENSION || extensions.contains(&extension))
}

/// Writes via a temporary sibling and a rename, so a watcher (hot reload, M1) never sees
/// a half-written file.
pub(crate) fn write_atomically(path: &Path, bytes: &[u8]) -> Result<(), CookError> {
    let write_error = |source| CookError::Write { path: path.to_owned(), source };
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(write_error)?;
    }
    let mut temp = path.as_os_str().to_owned();
    temp.push(".");
    temp.push(TEMP_EXTENSION);
    let temp = PathBuf::from(temp);
    fs::write(&temp, bytes).map_err(write_error)?;
    fs::rename(&temp, path).map_err(write_error)
}

#[cfg(test)]
mod tests {
    use glam::Vec3;
    use sapphire_assets::material::{
        AnimeCharacterMaterial, CharacterSurface, DiffuseMode, HairShading, RampDiffuse, TextureBinding,
    };
    use sapphire_assets::mesh::{PositionFormat, SlotMaterial};
    use sapphire_assets::texture::{Component, Swizzle};
    use serde_json::json;

    use super::*;
    use crate::bake::OutlineSettings;
    use crate::gltf_writer::{Geometry, GltfWriter, InfluenceSet, MorphGeometry};
    use crate::material::MaterialSettings;
    use crate::meta::{AssetMeta, sidecar_path};
    use crate::optimize::LodSettings;
    use crate::samples;
    use crate::test_util::{TempDir, read_aligned};
    use crate::texture::{
        ColorSettings, MaskChannels, MaskSettings, RampSettings, TextureClass, TextureSettings,
    };

    /// A source tree, an output directory and a cache, all in one scratch directory.
    struct Scratch {
        dir: TempDir,
    }

    impl Scratch {
        fn new(name: &str) -> Self {
            Self { dir: TempDir::new(name) }
        }

        fn source(&self) -> PathBuf {
            self.dir.path().join("content")
        }

        /// The desktop cook's output.
        fn out(&self) -> PathBuf {
            self.out_for(Cook::Desktop)
        }

        fn out_for(&self, cook: Cook) -> PathBuf {
            self.dir.path().join("out").join(cook.name())
        }

        /// Writes the mannequin to `relative` under the source root.
        fn add_mannequin(&self, relative: &str) -> PathBuf {
            let path = self.source().join(relative);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(&path, samples::mannequin_glb()).unwrap();
            path
        }

        fn cook_with(&self, cook: Cook, cache: bool) -> CookReport {
            let cache_dir = self.dir.path().join("cache");
            let options = CookOptions { cook, cache: cache.then_some(cache_dir.as_path()), locked: false };
            cook_dir(&self.source(), &self.out_for(cook), &options).unwrap()
        }

        fn cook(&self) -> CookReport {
            self.cook_with(Cook::Desktop, true)
        }
    }

    fn statuses(report: &CookReport) -> Vec<CookStatus> {
        report.assets.iter().map(|asset| asset.status).collect()
    }

    fn set_weld_tolerance(source: &Path, meters: f32) {
        let sidecar = sidecar_path(source);
        let mut meta = meta::read(&sidecar).unwrap();
        meta.importer = ImportSettings::Gltf(MeshSettings {
            outline: OutlineSettings::with_weld_tolerance(meters).unwrap(),
            ..MeshSettings::default()
        });
        fs::write(&sidecar, sapphire_serialize::to_ron::<AssetMeta>(&meta).unwrap()).unwrap();
    }

    /// The cooked mesh at `path`.
    fn read_mesh(path: &Path) -> Mesh {
        Mesh::from_cooked(&read_aligned(path)).unwrap()
    }

    #[test]
    fn a_cooked_files_hash_is_of_its_bytes_as_they_stand() {
        let dir = TempDir::new("cook_file_hash");
        let path = dir.path().join("cooked");
        assert_eq!(file_hash(&path).unwrap(), None, "no file");
        // Past a read's buffer, so more than one read goes into the hash.
        let bytes: Vec<u8> = (0..300_000u32).map(|i| (i % 251) as u8).collect();
        fs::write(&path, &bytes).unwrap();
        assert_eq!(file_hash(&path).unwrap(), Some(blake3::hash(&bytes)));
        fs::write(&path, &bytes[..1000]).unwrap();
        assert_eq!(file_hash(&path).unwrap(), Some(blake3::hash(&bytes[..1000])), "cut short");
        fs::write(&path, b"").unwrap();
        assert_eq!(file_hash(&path).unwrap(), Some(blake3::hash(b"")), "empty");
        let error = file_hash(dir.path()).unwrap_err();
        assert!(matches!(&error, CookError::Read { path: at, .. } if at == dir.path()), "{error:?}");
    }

    #[test]
    fn a_sources_parts_read_it_once() {
        let dir = TempDir::new("cook_last_source");
        let (a, b) = (dir.path().join("a"), dir.path().join("b"));
        fs::write(&a, b"first").unwrap();
        fs::write(&b, b"other").unwrap();
        let last = LastSource::default();
        let read = last.read(&a).unwrap();
        assert_eq!((read.bytes.as_slice(), read.hash), (&b"first"[..], blake3::hash(b"first")));
        // One version of it serves all its parts.
        fs::write(&a, b"saved since").unwrap();
        assert!(Rc::ptr_eq(&read, &last.read(&a).unwrap()));
        // The next source replaces it, so a later read of it sees the file as it stands.
        assert_eq!(last.read(&b).unwrap().bytes, b"other");
        assert_eq!(last.read(&a).unwrap().bytes, b"saved since");
        fs::remove_file(&b).unwrap();
        let error = last.read(&b).err().expect("the file is gone");
        assert!(matches!(&error, CookError::Read { path, .. } if *path == b), "{error:?}");
    }

    /// The assets of `scratch`'s tree cooked on `workers` threads: what each reports, and its file.
    fn cooked_on(scratch: &Scratch, workers: usize, cache: bool) -> Vec<(AssetId, CookStatus, Vec<u8>)> {
        let cache_dir = scratch.dir.path().join(format!("cache{workers}"));
        let options =
            CookOptions { cook: Cook::Desktop, cache: cache.then_some(cache_dir.as_path()), locked: false };
        let out = scratch.dir.path().join(format!("out{workers}"));
        let report = cook_dir_on(&scratch.source(), &out, &options, workers).unwrap();
        let read = |asset: &CookedAsset| fs::read(&asset.output).unwrap();
        report.assets.iter().map(|asset| (asset.id, asset.status, read(asset))).collect()
    }

    #[test]
    fn a_cook_is_the_same_on_any_number_of_threads() {
        let scratch = Scratch::new("cook_threads");
        for mesh in ["a.glb", "b/b.glb", "c.glb", "d.glb"] {
            scratch.add_mannequin(mesh);
        }
        for (index, name) in ["e.png", "f.png", "g.png", "h.png", "i.png", "j.png"].iter().enumerate() {
            let shade = (index as u8) / 2;
            let image = ::image::RgbaImage::from_pixel(32, 32, ::image::Rgba([shade, 20, 30, 255]));
            image.save(scratch.source().join(name)).unwrap();
        }
        let one = cooked_on(&scratch, 1, true);
        let statuses: Vec<_> = one.iter().map(|asset| asset.1).collect();
        assert!(statuses.contains(&CookStatus::Cached), "a repeated source is served by one cook");
        let files = |cook: &[(AssetId, CookStatus, Vec<u8>)]| -> Vec<(AssetId, Vec<u8>)> {
            cook.iter().map(|(id, _, bytes)| (*id, bytes.clone())).collect()
        };
        for _ in 0..4 {
            assert_eq!(cooked_on(&scratch, 4, true), one, "what each asset reports too");
            fs::remove_dir_all(scratch.dir.path().join("out4")).unwrap();
            fs::remove_dir_all(scratch.dir.path().join("cache4")).unwrap();
            assert_eq!(files(&cooked_on(&scratch, 4, false)), files(&one), "with no cache");
            fs::remove_dir_all(scratch.dir.path().join("out4")).unwrap();
        }
    }

    #[test]
    fn a_cook_fails_on_the_first_failure_in_plan_order_on_any_number_of_threads() {
        let scratch = Scratch::new("cook_first_failure");
        fs::create_dir_all(scratch.source()).unwrap();
        let good = ::image::RgbaImage::from_pixel(64, 64, ::image::Rgba([1, 2, 3, 255]));
        for name in ["a.png", "b.png", "d.png", "f.png", "g.png"] {
            good.save(scratch.source().join(name)).unwrap();
        }
        // A broken file fails at once, so a later one is often found first.
        for name in ["c.png", "e.png", "h.png"] {
            fs::write(scratch.source().join(name), b"not an image").unwrap();
        }
        for workers in [1, 2, 4, 8] {
            for _ in 0..8 {
                let options = CookOptions { cook: Cook::Desktop, cache: None, locked: false };
                let error = cook_dir_on(&scratch.source(), &scratch.out(), &options, workers).unwrap_err();
                let CookError::Texture { path, .. } = &error else { panic!("{error:?}") };
                assert_eq!(path.file_name().unwrap(), "c.png", "{workers} workers");
            }
        }
    }

    #[test]
    fn cooks_share_their_memory_and_a_large_one_runs_alone() {
        let memory = CookMemory::new(100);
        let free = || *memory.free.lock().unwrap();
        let some = memory.hold(60);
        assert_eq!(free(), 40);
        let more = memory.hold(40);
        assert_eq!(free(), 0);
        drop(more);

        let started = std::sync::atomic::AtomicBool::new(false);
        thread::scope(|scope| {
            scope.spawn(|| {
                // Past the limit: it waits for all of it.
                let large = memory.hold(1000);
                started.store(true, Ordering::SeqCst);
                drop(large);
            });
            thread::sleep(std::time::Duration::from_millis(20));
            assert!(!started.load(Ordering::SeqCst), "it waits for what's held");
            drop(some);
        });
        assert!(started.load(Ordering::SeqCst));
        assert_eq!(free(), 100, "all of it comes back");

        let panicking = panic::catch_unwind(AssertUnwindSafe(|| {
            let _held = memory.hold(100);
            panic!("a cook's panic");
        }));
        assert!(panicking.is_err());
        assert_eq!(free(), 100, "a panicking cook's too");
    }

    #[test]
    fn a_source_saved_between_its_key_and_its_cook_fails_rather_than_poisons_the_cache() {
        let dir = TempDir::new("cook_source_changed");
        let path = dir.path().join("source");
        fs::write(&path, b"keyed").unwrap();
        let hash = LastSource::default().read(&path).unwrap().hash;
        assert_eq!(LastSource::default().read_as(&path, hash).unwrap().bytes, b"keyed");
        fs::write(&path, b"saved since").unwrap();
        let error = LastSource::default().read_as(&path, hash).err().expect("it changed");
        assert!(matches!(&error, CookError::SourceChanged { path: at } if *at == path), "{error:?}");
    }

    /// A map at the size limit holds what a texture of that size does, not what a small one holds.
    #[test]
    fn a_maps_cook_says_what_it_holds_from_its_inputs() {
        let scratch = Scratch::new("cook_map_memory");
        fs::create_dir_all(scratch.source()).unwrap();
        let map = scratch.source().join("body.ilm.ron");
        let bytes = ilm_document("bias.png");
        let settings = MapKind::Ilm.default_settings();
        let memory = || map_memory(&map, bytes.as_bytes(), &settings);
        assert_eq!(memory(), MAP_MEMORY, "an input that doesn't read is the cook's to report");
        let input = scratch.source().join("bias.png");
        ::image::GrayImage::new(8, 8).save(&input).unwrap();
        assert_eq!(memory(), MAP_MEMORY, "a small map is taken to hold that much anyway");
        ::image::GrayImage::new(8192, 8192).save(&input).unwrap();
        let large = texture::cook_memory(&input, &fs::read(&input).unwrap());
        assert!(large > MAP_MEMORY);
        assert_eq!(memory(), large);
        let constants = br#"(type: "sapphire::pipeline::IlmMap", version: 1, data: (size: Some((4, 4))))"#;
        assert_eq!(map_memory(&map, constants, &settings), MAP_MEMORY);
    }

    /// A mesh's cook counts all it reads, and what doesn't read counts for nothing rather than failing.
    #[test]
    fn a_meshs_cook_says_what_it_holds_from_all_it_reads() {
        let scratch = Scratch::new("cook_mesh_memory");
        let (source, pieces) = (scratch.source(), scratch.source().join("pieces"));
        fs::create_dir_all(&pieces).unwrap();
        let gltf_settings = ImportSettings::Gltf(MeshSettings::default());
        let holds = |bytes: usize| bytes as u64 * MESH_MEMORY_PER_SOURCE_BYTE;

        let glb = samples::mannequin_glb();
        let memory = mesh_memory(&source.join("a.glb"), &glb, &gltf_settings);
        assert_eq!(memory, holds(glb.len()), "a GLB holds all it reads in itself");
        let fbx = ImportSettings::Fbx(MeshSettings::default());
        assert_eq!(mesh_memory(&source.join("a.fbx"), b"Kaydara", &fbx), holds(7));
        let path = source.join("broken.glb");
        assert_eq!(mesh_memory(&path, b"not glTF", &gltf_settings), holds(8), "the cook reports the rest");

        // A glTF's external buffer, counted once it's there.
        let mut writer = GltfWriter::default();
        let triangle = Geometry {
            positions: vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]],
            indices: vec![0, 1, 2],
            ..Geometry::default()
        };
        let mesh = writer.add_mesh("Triangle", &[(&triangle, None)]);
        let node = writer.add_node(json!({ "mesh": mesh }));
        writer.add_root(node);
        let gltf = serde_json::to_vec(&writer.document(Some("wall.bin"))).unwrap();
        let path = pieces.join("wall.glb");
        assert_eq!(mesh_memory(&path, &gltf, &gltf_settings), holds(gltf.len()), "no buffer yet");
        fs::write(pieces.join("wall.bin"), writer.bin()).unwrap();
        let with_buffer = gltf.len() + writer.bin().len();
        assert_eq!(mesh_memory(&path, &gltf, &gltf_settings), holds(with_buffer));

        // An assembly's pieces, with their buffers, once each however often they're placed.
        let assembly = br#"(
            type: "sapphire::pipeline::Assembly",
            version: 1,
            data: (
                kits: { "kit": "pieces" },
                palette: (columns: 2, rows: 1, gradient_columns: [1]),
                slots: [(name: "Plaster")],
                surfaces: [],
                place: [(piece: "kit/wall"), (piece: "kit/wall", turn: 90.0)],
            ),
        )"#;
        let (path, settings) =
            (source.join("town.assembly.ron"), ImportSettings::Assembly(MeshSettings::default()));
        assert_eq!(mesh_memory(&path, assembly, &settings), holds(assembly.len()), "no piece yet");
        fs::write(pieces.join("wall.glb"), &gltf).unwrap();
        assert_eq!(mesh_memory(&path, assembly, &settings), holds(assembly.len() + with_buffer));
        assert_eq!(mesh_memory(&path, b"not a document", &settings), holds(14));
    }

    /// Every part of a mesh's source that imports it waits for what it holds, so that all of them
    /// count against what the assets cooking at once may hold.
    #[test]
    fn a_meshs_cook_waits_for_the_memory_it_holds() {
        let scratch = Scratch::new("cook_mesh_waits");
        let mannequin = scratch.add_mannequin("mannequin.glb");
        let character = scratch.source().join("character.vrm");
        fs::write(&character, vrm::samples::vrm1().to_glb()).unwrap();
        let cases = [
            (&mannequin, ImportSettings::Gltf(MeshSettings::default()), Part::Own),
            (&character, ImportSettings::for_new_sidecar(&character).unwrap(), Part::Own),
            (&character, ImportSettings::for_new_sidecar(&character).unwrap(), Part::Skeleton),
            (&character, ImportSettings::for_new_sidecar(&character).unwrap(), Part::Rig),
        ];
        for (path, importer, part) in cases {
            let meta = AssetMeta::new(AssetId::from_bytes([7; 16]), importer);
            let recipe = Recipe::for_settings(&meta.importer);
            let plan = [Planned {
                id: meta.id,
                asset: SourceAsset { path: path.clone(), meta },
                part: part.clone(),
                planned_from: None,
                skeleton: None,
                recipe,
                output: scratch.out().join("asset"),
            }];
            let (by_id, sources, memory) = (HashMap::new(), HashMap::new(), CookMemory::new(COOK_MEMORY));
            let zones = HashMap::new();
            let tree = PlannedTree {
                plan: &plan,
                by_id: &by_id,
                sources: &sources,
                zones: &zones,
                cache: None,
                cook: Cook::Desktop,
                memory: &memory,
                fluent: &FluentFiles::new(),
            };
            let remembered = Remembered::default();
            let keyed = tree.key(0, &tree.assets(&[], &remembered)).unwrap();
            let held = memory.hold(COOK_MEMORY);
            let finished = std::sync::atomic::AtomicBool::new(false);
            thread::scope(|scope| {
                let cook = scope.spawn(|| {
                    let remembered = Remembered::default();
                    let result = tree.finish(0, keyed, &tree.assets(&[], &remembered));
                    finished.store(true, Ordering::SeqCst);
                    result
                });
                thread::sleep(std::time::Duration::from_millis(200));
                assert!(!finished.load(Ordering::SeqCst), "{part:?} of {path:?} waits for what's held");
                drop(held);
                assert_eq!(cook.join().unwrap().unwrap().status, CookStatus::Cooked);
            });
            assert_eq!(*memory.free.lock().unwrap(), COOK_MEMORY, "and gives it back");
        }
    }

    /// Keys the source at `path`, runs `save`, and finishes it, as a worker reaches an asset long after
    /// its key: the result, and what the cache then holds under that key.
    fn finish_after(
        scratch: &Scratch,
        path: &Path,
        importer: ImportSettings,
        save: impl FnOnce(),
    ) -> (Result<CookedAsset, CookError>, Option<Vec<u8>>) {
        let cache = Cache::new(scratch.dir.path().join("cache"));
        let meta = AssetMeta::new(AssetId::from_bytes([7; 16]), importer);
        let recipe = Recipe::for_settings(&meta.importer);
        let output = scratch.out().join("asset").with_extension(recipe.extension);
        let plan = [Planned {
            id: meta.id,
            asset: SourceAsset { path: path.to_owned(), meta },
            part: Part::Own,
            planned_from: None,
            skeleton: None,
            recipe,
            output,
        }];
        let (by_id, sources, memory) = (HashMap::new(), HashMap::new(), CookMemory::new(COOK_MEMORY));
        let zones = HashMap::new();
        let tree = PlannedTree {
            plan: &plan,
            by_id: &by_id,
            sources: &sources,
            zones: &zones,
            cache: Some(&cache),
            cook: Cook::Desktop,
            memory: &memory,
            fluent: &FluentFiles::new(),
        };
        let remembered = Remembered::default();
        let keyed = tree.key(0, &tree.assets(&[], &remembered)).unwrap();
        let key = keyed.key;
        save();
        let remembered = Remembered::default();
        let result = tree.finish(0, keyed, &tree.assets(&[], &remembered));
        (result, cache.get(&key).unwrap())
    }

    #[test]
    fn a_file_saved_between_a_key_and_its_cook_fails_rather_than_poisons_the_cache() {
        let scratch = Scratch::new("cook_input_changed");
        fs::create_dir_all(scratch.source()).unwrap();
        let shade = |value| ::image::RgbaImage::from_pixel(8, 8, ::image::Rgba([value, value, value, 255]));
        // A map's input.
        let input = scratch.source().join("bias.png");
        shade(10).save(&input).unwrap();
        let map = scratch.source().join("body.ilm.ron");
        fs::write(&map, ilm_document("bias.png")).unwrap();
        let settings = ImportSettings::CharacterMap(MapKind::Ilm.default_settings());
        let (result, cached) =
            finish_after(&scratch, &map, settings.clone(), || shade(200).save(&input).unwrap());
        let error = result.unwrap_err();
        assert!(matches!(&error, CookError::SourceChanged { path } if *path == map), "{error:?}");
        assert!(error.to_string().contains("or a file it reads changed"), "{error}");
        assert_eq!(cached, None, "nothing is stored under the key the other version made");
        assert!(!scratch.out().join("asset.ktx2").exists(), "and nothing is written");
        // With nothing saved it cooks, and its input as it stands now.
        let (result, cached) = finish_after(&scratch, &map, settings, || {});
        assert_eq!(result.unwrap().status, CookStatus::Cooked);
        assert!(cached.is_some());

        // A glTF's external buffer.
        let mut writer = GltfWriter::default();
        let triangle = Geometry {
            positions: vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]],
            indices: vec![0, 1, 2],
            ..Geometry::default()
        };
        let mesh = writer.add_mesh("Triangle", &[(&triangle, None)]);
        let node = writer.add_node(json!({ "mesh": mesh }));
        writer.add_root(node);
        let gltf = scratch.source().join("triangle.gltf");
        fs::write(&gltf, serde_json::to_vec(&writer.document(Some("triangle.bin"))).unwrap()).unwrap();
        let buffer = scratch.source().join("triangle.bin");
        fs::write(&buffer, writer.bin()).unwrap();
        let mut moved = writer.bin().to_vec();
        moved[..4].copy_from_slice(&0.5f32.to_le_bytes());
        let settings = ImportSettings::Gltf(MeshSettings::default());
        let (result, cached) =
            finish_after(&scratch, &gltf, settings, || fs::write(&buffer, &moved).unwrap());
        let error = result.unwrap_err();
        assert!(matches!(&error, CookError::SourceChanged { path } if *path == gltf), "{error:?}");
        assert_eq!(cached, None);

        // A file gone by then is the error that says so.
        let settings = ImportSettings::CharacterMap(MapKind::Ilm.default_settings());
        let other = scratch.source().join("other.ilm.ron");
        let gone = scratch.source().join("gone.png");
        shade(30).save(&gone).unwrap();
        fs::write(&other, ilm_document("gone.png")).unwrap();
        let (result, cached) = finish_after(&scratch, &other, settings, || fs::remove_file(&gone).unwrap());
        assert!(result.is_err());
        assert_eq!(cached, None);
    }

    #[test]
    fn a_panic_cooking_a_source_is_an_error_naming_it() {
        let path = Path::new("content/bad.glb");
        let error = guarded::<()>(path, || panic!("a crate's assert")).unwrap_err();
        assert!(
            matches!(&error, CookError::Panicked { path: at, message } if at == path && message == "a crate's assert"),
            "{error:?}"
        );
        assert_eq!(error.to_string(), "cooking content/bad.glb panicked: a crate's assert");
        assert_eq!(guarded(path, || Ok(5)).unwrap(), 5);
        let refused = guarded::<()>(path, || Err(CookError::UnsupportedSource { path: path.to_owned() }));
        assert!(matches!(refused, Err(CookError::UnsupportedSource { .. })));
    }

    #[test]
    fn parses_cook_names() {
        for cook in Cook::ALL {
            assert_eq!(cook.name().parse(), Ok(cook));
            assert_eq!(cook.to_string(), cook.name());
        }
        assert_eq!("console".parse::<Cook>(), Err(UnknownCook("console".into())));
    }

    #[test]
    fn cooks_a_tree_into_mirrored_paths() {
        let scratch = Scratch::new("cook_tree");
        let source = scratch.add_mannequin("characters/mannequin/mannequin.glb");
        scratch.add_mannequin(".hidden/mannequin.glb");
        fs::write(scratch.source().join("notes.txt"), "not an asset").unwrap();

        for cook in Cook::ALL {
            let report = scratch.cook_with(cook, false);
            let expected = scratch.out_for(cook).join("characters/mannequin/mannequin.smesh");
            let [asset] = &report.assets[..] else { panic!("one asset") };
            assert_eq!(
                (&asset.source, &asset.output, asset.status),
                (&source, &expected, CookStatus::Cooked)
            );
            assert_eq!(asset.id, meta::read(&sidecar_path(&source)).unwrap().id);

            let mesh = read_mesh(&expected);
            // Outline normals are baked: at a pedestal corner they differ from the split
            // shading normals.
            let corner = (0..mesh.streams.len())
                .filter_map(|index| mesh.streams.vertex(index))
                .find(|v| Vec3::from(v.position).abs_diff_eq(Vec3::new(0.55, 0.0, 0.55), 1e-4))
                .expect("pedestal bottom corner");
            assert_ne!(corner.outline_normal, corner.normal);
            assert!(
                Vec3::from(corner.outline_normal).abs_diff_eq(Vec3::new(1.0, -1.0, 1.0).normalize(), 1e-4)
            );
        }
        assert!(!scratch.out().join(".hidden").exists());
        assert!(!scratch.source().join(".hidden/mannequin.glb.meta").exists());
    }

    #[test]
    fn the_first_cook_creates_sidecars_and_later_ones_keep_them() {
        let scratch = Scratch::new("cook_sidecars");
        let source = scratch.add_mannequin("mannequin.glb");
        let first = scratch.cook();
        assert_eq!(first.created_sidecars, [sidecar_path(&source)]);
        let sidecar = fs::read_to_string(sidecar_path(&source)).unwrap();

        let second = scratch.cook();
        assert!(second.created_sidecars.is_empty());
        assert_eq!(fs::read_to_string(sidecar_path(&source)).unwrap(), sidecar);
        assert_eq!(second.assets[0].id, first.assets[0].id);

        let locked = CookOptions { cook: Cook::Desktop, cache: None, locked: true };
        let unsidecarred = scratch.add_mannequin("new.glb");
        let error = cook_dir(&scratch.source(), &scratch.out(), &locked).unwrap_err();
        assert!(matches!(error, CookError::Meta(MetaError::Missing { .. })), "{error:?}");
        assert!(!sidecar_path(&unsidecarred).exists());
    }

    #[test]
    fn unchanged_assets_come_from_the_cache_without_rewriting_outputs() {
        let scratch = Scratch::new("cook_cache_hits");
        scratch.add_mannequin("mannequin.glb");
        let output = scratch.out().join("mannequin.smesh");
        assert_eq!(statuses(&scratch.cook()), [CookStatus::Cooked]);
        let written = fs::metadata(&output).unwrap().modified().unwrap();
        let cooked = fs::read(&output).unwrap();

        // Touching or rewriting a source with the same bytes changes nothing.
        let source = scratch.source().join("mannequin.glb");
        fs::write(&source, samples::mannequin_glb()).unwrap();
        let report = scratch.cook();
        assert_eq!(statuses(&report), [CookStatus::UpToDate]);
        assert_eq!(fs::metadata(&output).unwrap().modified().unwrap(), written, "not rewritten");

        // Deleted or edited outputs come back from the cache.
        fs::remove_file(&output).unwrap();
        assert_eq!(statuses(&scratch.cook()), [CookStatus::Cached]);
        assert_eq!(fs::read(&output).unwrap(), cooked);
        fs::write(&output, b"edited by hand").unwrap();
        assert_eq!(statuses(&scratch.cook()), [CookStatus::Cached]);
        assert_eq!(fs::read(&output).unwrap(), cooked);

        // The other cook has keys of its own.
        assert_eq!(statuses(&scratch.cook_with(Cook::Mobile, true)), [CookStatus::Cooked]);
        assert_eq!(statuses(&scratch.cook_with(Cook::Mobile, true)), [CookStatus::UpToDate]);

        // Without a cache every asset cooks, but unchanged outputs still aren't rewritten.
        let written = fs::metadata(&output).unwrap().modified().unwrap();
        assert_eq!(statuses(&scratch.cook_with(Cook::Desktop, false)), [CookStatus::Cooked]);
        assert_eq!(fs::metadata(&output).unwrap().modified().unwrap(), written, "not rewritten");
    }

    #[test]
    fn only_assets_whose_inputs_changed_are_cooked() {
        let scratch = Scratch::new("cook_changes");
        let a = scratch.add_mannequin("a.glb");
        scratch.add_mannequin("b/b.glb");
        // Same bytes and settings: one cook serves both.
        assert_eq!(statuses(&scratch.cook()), [CookStatus::Cooked, CookStatus::Cached]);

        set_weld_tolerance(&a, 0.05);
        let report = scratch.cook();
        assert_eq!(statuses(&report), [CookStatus::Cooked, CookStatus::UpToDate]);
        let [a_report, b_report] = &report.assets[..] else { panic!("two assets") };
        assert_ne!(a_report.key, b_report.key);
        assert_ne!(fs::read(&a_report.output).unwrap(), fs::read(&b_report.output).unwrap());

        // Files no asset reads don't matter.
        fs::write(scratch.source().join("notes.txt"), "edited").unwrap();
        assert_eq!(statuses(&scratch.cook()), [CookStatus::UpToDate, CookStatus::UpToDate]);
    }

    #[test]
    fn moved_sources_keep_their_id_and_cached_output() {
        let scratch = Scratch::new("cook_moves");
        let source = scratch.add_mannequin("old/mannequin.glb");
        let first = scratch.cook();
        let moved = scratch.source().join("new/renamed.glb");
        fs::create_dir_all(moved.parent().unwrap()).unwrap();
        fs::rename(&source, &moved).unwrap();
        fs::rename(sidecar_path(&source), sidecar_path(&moved)).unwrap();
        fs::write(scratch.out().join("old/leftover.smesh.tmp"), b"interrupted").unwrap();
        fs::write(scratch.out().join("readme.txt"), b"not ours").unwrap();

        let report = scratch.cook();
        let [asset] = &report.assets[..] else { panic!("one asset") };
        assert_eq!(
            (asset.id, asset.key, asset.status),
            (first.assets[0].id, first.assets[0].key, CookStatus::Cached)
        );
        assert_eq!(asset.output, scratch.out().join("new/renamed.smesh"));
        assert_eq!(
            report.removed,
            [scratch.out().join("old/leftover.smesh.tmp"), scratch.out().join("old/mannequin.smesh")]
        );
        assert!(!scratch.out().join("old").exists(), "emptied directories are removed");
        assert!(scratch.out().join("readme.txt").exists(), "only cooked and temporary files are pruned");
    }

    #[test]
    fn damaged_cache_entries_are_cooked_again() {
        let scratch = Scratch::new("cook_damaged");
        scratch.add_mannequin("mannequin.glb");
        let report = scratch.cook();
        let entry = Cache::new(scratch.dir.path().join("cache")).entry_path(&report.assets[0].key);
        let mut bytes = fs::read(&entry).unwrap();
        let middle = bytes.len() / 2;
        bytes[middle] ^= 0xff;
        fs::write(&entry, bytes).unwrap();
        fs::remove_file(&report.assets[0].output).unwrap();
        assert_eq!(statuses(&scratch.cook()), [CookStatus::Cooked]);
        assert_eq!(statuses(&scratch.cook()), [CookStatus::UpToDate]);
    }

    #[test]
    fn external_buffers_are_part_of_the_key() {
        let scratch = Scratch::new("cook_external");
        let mut writer = GltfWriter::default();
        let triangle = Geometry {
            positions: vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]],
            indices: vec![0, 1, 2],
            ..Geometry::default()
        };
        let mesh = writer.add_mesh("Triangle", &[(&triangle, None)]);
        let node = writer.add_node(json!({ "mesh": mesh }));
        writer.add_root(node);
        fs::create_dir_all(scratch.source()).unwrap();
        let document = serde_json::to_vec(&writer.document(Some("triangle%20data.bin"))).unwrap();
        fs::write(scratch.source().join("triangle.gltf"), document).unwrap();
        let buffer = scratch.source().join("triangle data.bin");
        fs::write(&buffer, writer.bin()).unwrap();
        assert_eq!(statuses(&scratch.cook()), [CookStatus::Cooked]);
        assert_eq!(statuses(&scratch.cook()), [CookStatus::UpToDate]);

        // Move the triangle's first vertex.
        let mut bin = writer.bin().to_vec();
        bin[..4].copy_from_slice(&0.5f32.to_le_bytes());
        fs::write(&buffer, &bin).unwrap();
        assert_eq!(statuses(&scratch.cook()), [CookStatus::Cooked]);

        fs::remove_file(&buffer).unwrap();
        let error = scratch_error(&scratch);
        assert!(matches!(&error, CookError::Read { path, .. } if *path == buffer), "{error:?}");
    }

    fn scratch_error(scratch: &Scratch) -> CookError {
        let options = CookOptions { cook: Cook::Desktop, cache: None, locked: false };
        cook_dir(&scratch.source(), &scratch.out(), &options).unwrap_err()
    }

    #[test]
    fn settings_reach_the_bake() {
        let source = TempDir::new("cook_settings");
        let path = samples::write_all(source.path(), samples::Existing::Keep).unwrap().created.remove(0);
        // Without LODs, so the vertex order can't change with the seams the weld finds.
        let no_lods = MeshSettings { lods: LodSettings::none(), ..MeshSettings::default() };
        let default = cook_mesh(&path, &no_lods, Cook::Desktop).unwrap();
        let coarse =
            MeshSettings { outline: OutlineSettings::with_weld_tolerance(0.05).unwrap(), ..no_lods.clone() };
        let welded = cook_mesh(&path, &coarse, Cook::Desktop).unwrap();
        assert_eq!(default.streams.len(), welded.streams.len());
        assert_eq!(default.streams.positions, welded.streams.positions);
        assert_ne!(
            default.streams.shading, welded.streams.shading,
            "a 5 cm weld must change some outline normals"
        );
    }

    #[test]
    fn lod_and_position_settings_reach_the_cook() {
        let source = TempDir::new("cook_lod_settings");
        let path = samples::write_all(source.path(), samples::Existing::Keep).unwrap().created.remove(0);
        let cook = |settings: &MeshSettings, cook| cook_mesh(&path, settings, cook).unwrap();
        let default = cook(&MeshSettings::default(), Cook::Desktop);
        assert!(default.lods.len() > 1, "the mannequin simplifies");
        assert_eq!(default.streams.position_format(), PositionFormat::Unorm16);
        let single = MeshSettings { lods: LodSettings::none(), ..MeshSettings::default() };
        for target in Cook::ALL {
            assert_eq!(cook(&single, target).lods.len(), 1);
        }
        let float = MeshSettings { position_format: PositionFormat::Float32, ..MeshSettings::default() };
        let float = cook(&float, Cook::Desktop);
        assert_eq!(float.streams.position_format(), PositionFormat::Float32);
        assert_eq!((float.indices, float.streams.shading), (default.indices, default.streams.shading));
    }

    #[test]
    fn the_mobile_cook_has_a_lower_detail_lod0_and_fewer_lods() {
        let source = TempDir::new("cook_mobile");
        let path = samples::write_all(source.path(), samples::Existing::Keep).unwrap().created.remove(0);
        let desktop = cook_mesh(&path, &MeshSettings::default(), Cook::Desktop).unwrap();
        let mobile = cook_mesh(&path, &MeshSettings::default(), Cook::Mobile).unwrap();
        let triangles = |mesh: &Mesh, lod: usize| -> u32 {
            mesh.lods[lod].submeshes.iter().map(|submesh| submesh.index_count / 3).sum()
        };
        assert_eq!(mobile.lods.len(), desktop.lods.len() - 1);
        for lod in 0..mobile.lods.len() {
            assert_eq!(triangles(&mobile, lod), triangles(&desktop, lod + 1), "mobile LOD{lod}");
            assert_eq!(mobile.lods[lod].error, desktop.lods[lod + 1].error);
        }
        assert!(mobile.lods[0].error > 0.0, "mobile LOD0 is simplified");
        assert!(mobile.streams.len() < desktop.streams.len(), "vertices only desktop LOD0 used are dropped");
        assert_eq!(mobile.streams.len() as u32, desktop.lods[1].vertex_count);
        eprintln!(
            "mannequin: desktop LOD triangles {:?}, errors {:?} m; {} vertices desktop, {} mobile",
            (0..desktop.lods.len()).map(|lod| triangles(&desktop, lod)).collect::<Vec<_>>(),
            desktop.lods.iter().map(|lod| lod.error).collect::<Vec<_>>(),
            desktop.streams.len(),
            mobile.streams.len()
        );
    }

    #[test]
    fn cooks_are_deterministic() {
        let scratch = Scratch::new("cook_determinism");
        scratch.add_mannequin("mannequin.glb");
        let output = scratch.out().join("mannequin.smesh");
        scratch.cook_with(Cook::Desktop, false);
        let first = fs::read(&output).unwrap();
        fs::remove_file(&output).unwrap();
        scratch.cook_with(Cook::Desktop, false);
        assert!(fs::read(&output).unwrap() == first, "two cooks of one source write the same bytes");
        scratch.cook_with(Cook::Mobile, false);
        assert!(fs::read(scratch.out_for(Cook::Mobile).join("mannequin.smesh")).unwrap() != first);
    }

    /// A hard-edged roof: two quads meeting at 90° along the x axis, with split normals.
    fn roof(outline_normals: bool) -> Geometry {
        let (left, right) = (Vec3::new(0.0, 1.0, -1.0).normalize(), Vec3::new(0.0, 1.0, 1.0).normalize());
        Geometry {
            positions: vec![
                [0.0, 0.0, -1.0],
                [1.0, 0.0, -1.0],
                [1.0, 1.0, 0.0],
                [0.0, 1.0, 0.0],
                [0.0, 1.0, 0.0],
                [1.0, 1.0, 0.0],
                [1.0, 0.0, 1.0],
                [0.0, 0.0, 1.0],
            ],
            normals: [left; 4].into_iter().chain([right; 4]).map(|n| n.to_array()).collect(),
            outline_normals: if outline_normals { vec![[0.0, 1.0, 0.0]; 8] } else { Vec::new() },
            indices: vec![0, 2, 1, 0, 3, 2, 4, 6, 5, 4, 7, 6],
            ..Geometry::default()
        }
    }

    #[test]
    fn authored_outline_normals_win_over_the_welded_bake() {
        let source = TempDir::new("cook_authored_outline");
        let cook = |outline_normals: bool| {
            let mut writer = GltfWriter::default();
            let mesh = writer.add_mesh("Roof", &[(&roof(outline_normals), None)]);
            let node = writer.add_node(json!({ "mesh": mesh }));
            writer.add_root(node);
            let path = source.path().join("roof.glb");
            fs::write(&path, writer.to_glb()).unwrap();
            let mesh = cook_mesh(&path, &MeshSettings::default(), Cook::Desktop).unwrap();
            (0..mesh.streams.len()).map(|index| mesh.streams.vertex(index).unwrap()).collect::<Vec<_>>()
        };
        // Baked: the ridge vertices (y = 1) average both faces, the eaves keep their face's.
        for vertex in cook(false) {
            let expected = if vertex.position[1] == 1.0 { Vec3::Y } else { Vec3::from(vertex.normal) };
            assert!(Vec3::from(vertex.outline_normal).abs_diff_eq(expected, 1e-4), "{vertex:?}");
        }
        // Authored: every vertex keeps the authored direction, eaves included.
        for vertex in cook(true) {
            assert!(Vec3::from(vertex.outline_normal).abs_diff_eq(Vec3::Y, 1e-4), "{vertex:?}");
            assert!(!Vec3::from(vertex.normal).abs_diff_eq(Vec3::Y, 0.1), "shading normals stay split");
        }
    }

    /// Vertices above this height are raised by [`skinned_strip`]'s morph target. It lies
    /// between rows, so quantization can't move a vertex across it, and high enough that the
    /// strip still simplifies on both cooks: the simplifier never moves a vertex a morph target
    /// moves, nor its neighbors (the review's AP-005).
    const RAISED_ABOVE: f32 = 0.76;

    /// A vertical strip of `rows` (40) quads from y = 0 to 1, skinned to two joints (the upper
    /// joint's weight is y) with one morph target, `raise`, that lifts the top half by 0.1.
    fn skinned_strip(rows: u32) -> Vec<u8> {
        let mut geometry = Geometry::default();
        let mut influences = InfluenceSet::default();
        let mut raise = MorphGeometry::default();
        for row in 0..=rows {
            let y = row as f32 / rows as f32;
            for x in [0.0, 0.2] {
                geometry.positions.push([x, y, 0.0]);
                geometry.normals.push([0.0, 0.0, 1.0]);
                influences.joints.push([0, 1, 0, 0]);
                influences.weights.push([1.0 - y, y, 0.0, 0.0]);
                raise.positions.push(if y > RAISED_ABOVE { [0.0, 0.1, 0.0] } else { [0.0; 3] });
            }
        }
        for row in 0..rows {
            let a = 2 * row;
            geometry.indices.extend([a, a + 1, a + 3, a, a + 3, a + 2]);
        }
        geometry.influences = vec![influences];
        geometry.targets = vec![raise];
        let mut writer = GltfWriter::default();
        let lower = writer.add_node(json!({ "name": "Lower" }));
        let upper = writer.add_node(json!({ "name": "Upper", "translation": [0.0, 1.0, 0.0] }));
        let mut doc_nodes = vec![lower, upper];
        let skin = writer.add_skin(&doc_nodes, None);
        let mesh = writer.add_mesh("Strip", &[(&geometry, None)]);
        writer.set_mesh_field(mesh, "extras", json!({ "targetNames": ["raise"] }));
        let body = writer.add_node(json!({ "name": "Body", "mesh": mesh, "skin": skin }));
        doc_nodes.push(body);
        for node in doc_nodes {
            writer.add_root(node);
        }
        writer.to_glb()
    }

    #[test]
    fn skins_and_morph_targets_follow_the_vertex_reorder() {
        let source = TempDir::new("cook_skinned");
        let path = source.path().join("strip.glb");
        fs::write(&path, skinned_strip(40)).unwrap();
        for target in Cook::ALL {
            let mesh = cook_mesh(&path, &MeshSettings::default(), target).unwrap();
            assert!(mesh.lods.len() > 1, "{target}: a flat strip simplifies");
            let skin = mesh.skin.as_ref().expect("skinned");
            assert_eq!(skin.joint_count, 2);
            for index in 0..mesh.streams.len() {
                let y = mesh.streams.vertex(index).unwrap().position[1];
                let influence = skin.influences.get(index).unwrap();
                let upper = influence.binds.iter().zip(influence.weights).find(|(bind, _)| **bind == 1);
                let upper = upper.map_or(0.0, |(_, weight)| f32::from(weight) / 255.0);
                assert!((upper - y).abs() <= 1.0 / 255.0, "{target}: vertex {index} at y {y} weighs {upper}");
            }
            let [raise] = &mesh.morph_targets[..] else { panic!("one target") };
            assert_eq!(raise.name, "raise");
            let deltas = &mesh.morph_deltas[raise.first_delta as usize..][..raise.delta_count as usize];
            let height = |index: usize| mesh.streams.vertex(index).unwrap().position[1];
            let raised = (0..mesh.streams.len()).filter(|&index| height(index) > RAISED_ABOVE).count();
            assert_eq!(deltas.len(), raised, "{target}: one delta per raised vertex");
            for delta in deltas {
                let y = height(delta.vertex() as usize);
                assert!(
                    y > RAISED_ABOVE && (delta.position()[1] - 0.1).abs() < 1e-4,
                    "{target}: {delta:?} at y {y}"
                );
            }
        }
    }

    const ALBEDO: &str = "a1b2c3d4-0000-4000-8000-000000000001";
    const RAMP: &str = "a1b2c3d4-0000-4000-8000-000000000002";
    const HAIR_MASK: &str = "a1b2c3d4-0000-4000-8000-000000000003";
    const HAIR: &str = "a1b2c3d4-0000-4000-8000-000000000004";

    fn id(text: &str) -> AssetId {
        text.parse().unwrap()
    }

    /// Writes a sidecar with a known ID, so documents can reference the asset.
    fn write_sidecar(source: &Path, id: &str, importer: ImportSettings) {
        let meta = AssetMeta::new(id.parse().unwrap(), importer);
        fs::write(sidecar_path(source), sapphire_serialize::to_ron(&meta).unwrap()).unwrap();
    }

    impl Scratch {
        /// A `width × height` RGBA PNG at `relative`, cooked as `class`, with ID `id`.
        fn add_texture(&self, relative: &str, id: &str, class: TextureClass, seed: u8) -> PathBuf {
            let path = self.source().join(relative);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            // 16 rows tall, as ramps must be (A7's layout).
            let image = ::image::RgbaImage::from_fn(16, 16, |x, y| {
                ::image::Rgba([(x * 16) as u8 ^ seed, (y * 16) as u8, seed, 255 - (x * 8) as u8])
            });
            image.save_with_format(&path, ::image::ImageFormat::Png).unwrap();
            write_sidecar(&path, id, ImportSettings::Texture(TextureSettings { class }));
            path
        }

        /// A material document at `relative` with ID `id`.
        fn add_material(&self, relative: &str, id: &str, material: &AnimeCharacterMaterial) -> PathBuf {
            let path = self.source().join(relative);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(&path, sapphire_serialize::to_ron(material).unwrap()).unwrap();
            write_sidecar(&path, id, ImportSettings::Material(MaterialSettings::default()));
            path
        }

        /// Hair textures and a hair material using them: albedo, a ramp, and a two-channel
        /// highlight mask (which ASTC stores with a swizzle).
        fn add_hair(&self) -> PathBuf {
            self.add_texture("hair/albedo.png", ALBEDO, TextureClass::Color(ColorSettings::default()), 1);
            self.add_texture("hair/ramp.png", RAMP, TextureClass::Ramp(RampSettings::default()), 2);
            let two = MaskSettings { channels: MaskChannels::Two, ..MaskSettings::default() };
            self.add_texture("hair/mask.png", HAIR_MASK, TextureClass::Mask(two), 3);
            self.add_material("hair/hair.material.ron", HAIR, &hair_material(id(RAMP)))
        }
    }

    fn hair_material(ramp: AssetId) -> AnimeCharacterMaterial {
        AnimeCharacterMaterial {
            surface: CharacterSurface::Hair(HairShading {
                mask_map: Some(id(HAIR_MASK)),
                ..HairShading::default()
            }),
            base_color_map: Some(id(ALBEDO)),
            diffuse: DiffuseMode::Ramp(RampDiffuse { map: ramp, mix: 0.5 }),
            ..AnimeCharacterMaterial::default()
        }
    }

    fn asset<'a>(report: &'a CookReport, source: &Path) -> &'a CookedAsset {
        report.assets.iter().find(|asset| asset.source == source).expect("the asset was cooked")
    }

    #[test]
    fn materials_cook_against_the_textures_they_reference() {
        let scratch = Scratch::new("cook_materials");
        let hair = scratch.add_hair();
        let ra01 = Swizzle([Component::R, Component::A, Component::Zero, Component::One]);
        for (cook, mask_swizzle) in [(Cook::Desktop, Swizzle::IDENTITY), (Cook::Mobile, ra01)] {
            let report = scratch.cook_with(cook, true);
            let material = asset(&report, &hair);
            assert_eq!(material.output, scratch.out_for(cook).join("hair/hair.smat"));
            assert_eq!(material.id, id(HAIR));
            let cooked = Material::from_cooked(&read_aligned(&material.output)).unwrap();
            let binding = |slot: usize| cooked.textures[slot];
            assert_eq!(binding(AnimeCharacterMaterial::BASE_COLOR).texture, Some(id(ALBEDO)));
            assert_eq!(binding(AnimeCharacterMaterial::RAMP).texture, Some(id(RAMP)));
            let mask = binding(AnimeCharacterMaterial::SURFACE_MASK);
            assert_eq!((mask.texture, mask.swizzle), (Some(id(HAIR_MASK)), mask_swizzle.to_ktx()), "{cook}");
            assert_eq!(binding(AnimeCharacterMaterial::CONTROL), TextureBinding::UNBOUND);
        }
        // Deterministic: without a cache, a second cook writes the same bytes.
        let output = scratch.out().join("hair/hair.smat");
        let first = fs::read(&output).unwrap();
        fs::remove_file(&output).unwrap();
        scratch.cook_with(Cook::Desktop, false);
        assert!(fs::read(&output).unwrap() == first);
    }

    #[test]
    fn a_material_recooks_when_a_texture_it_references_changes() {
        let scratch = Scratch::new("cook_material_dependencies");
        let hair = scratch.add_hair();
        let first = scratch.cook();
        assert!(statuses(&scratch.cook()).iter().all(|&status| status == CookStatus::UpToDate));

        // New ramp pixels: the ramp and the material (whose key holds the ramp's) recook; the
        // other textures don't.
        let ramp = scratch.add_texture("hair/ramp.png", RAMP, TextureClass::Ramp(RampSettings::default()), 9);
        let report = scratch.cook();
        assert_eq!(asset(&report, &ramp).status, CookStatus::Cooked);
        assert_eq!(asset(&report, &hair).status, CookStatus::Cooked);
        assert_ne!(asset(&report, &hair).key, asset(&first, &hair).key);
        let albedo = scratch.source().join("hair/albedo.png");
        assert_eq!(asset(&report, &albedo).status, CookStatus::UpToDate);

        // A texture that disappears fails the material, cached or not.
        fs::remove_file(&ramp).unwrap();
        fs::remove_file(sidecar_path(&ramp)).unwrap();
        let error = scratch_error(&scratch);
        let CookError::Material { path, source } = &error else { panic!("{error:?}") };
        assert_eq!(path, &hair);
        assert!(matches!(**source, MaterialCookError::MissingTexture { slot: "ramp", .. }), "{source}");
    }

    #[test]
    fn material_references_must_be_textures_of_the_right_class() {
        let scratch = Scratch::new("cook_material_references");
        scratch.add_hair();
        let mannequin = scratch.add_mannequin("mannequin.glb");
        let mesh = "a1b2c3d4-0000-4000-8000-000000000005";
        write_sidecar(&mannequin, mesh, ImportSettings::Gltf(MeshSettings::default()));
        let error_for = |ramp: AssetId| {
            scratch.add_material("hair/hair.material.ron", HAIR, &hair_material(ramp));
            let CookError::Material { source, .. } = scratch_error(&scratch) else {
                panic!("a material error")
            };
            *source
        };
        let error = error_for(id(ALBEDO));
        assert!(
            matches!(&error, MaterialCookError::WrongClass { slot: "ramp", class: "color", .. }),
            "{error}"
        );
        assert!(error.to_string().contains("needs a ramp"), "{error}");
        assert!(matches!(error_for(id(mesh)), MaterialCookError::NotATexture { slot: "ramp", .. }));
        let unknown = id("a1b2c3d4-0000-4000-8000-0000000000ff");
        assert!(matches!(error_for(unknown), MaterialCookError::MissingTexture { id, .. } if id == unknown));

        // A ramp that doesn't have A7's 16 rows.
        scratch.add_material("hair/hair.material.ron", HAIR, &hair_material(id(RAMP)));
        let ramp = scratch.source().join("hair/ramp.png");
        ::image::RgbaImage::new(16, 8).save_with_format(&ramp, ::image::ImageFormat::Png).unwrap();
        let CookError::Material { source, .. } = scratch_error(&scratch) else { panic!("a material error") };
        assert!(matches!(*source, MaterialCookError::RampRows { height: 8 }), "{source}");
    }

    #[test]
    fn mesh_sidecars_bind_slots_to_material_documents() {
        let scratch = Scratch::new("cook_material_bindings");
        scratch.add_hair();
        let mannequin = scratch.add_mannequin("mannequin.glb");
        let mut settings = MeshSettings::default();
        let bind = |settings: &MeshSettings| {
            let sidecar = sidecar_path(&mannequin);
            let id = meta::read(&sidecar).map_or_else(|_| meta::new_id().unwrap(), |meta| meta.id);
            let meta = AssetMeta::new(id, ImportSettings::Gltf(settings.clone()));
            fs::write(sidecar, sapphire_serialize::to_ron(&meta).unwrap()).unwrap();
        };
        settings.materials.insert("Hair".into(), id(HAIR));
        bind(&settings);
        scratch.cook();
        let mesh = read_mesh(&scratch.out().join("mannequin.smesh"));
        for slot in &mesh.materials {
            match (&slot.material, slot.name.as_str()) {
                (SlotMaterial::Asset(bound), "Hair") => assert_eq!(*bound, id(HAIR)),
                (SlotMaterial::Embedded(_), name) => assert_ne!(name, "Hair"),
                (other, name) => panic!("slot {name}: {other:?}"),
            }
        }
        assert_eq!(mesh.materials.len(), 5);

        settings.materials.insert("Cape".into(), id(HAIR));
        bind(&settings);
        let error = scratch_error(&scratch);
        assert!(
            matches!(&error, CookError::MaterialSlot { source: MaterialSlotError::UnknownSlot { name, .. }, .. } if name == "Cape"),
            "{error:?}"
        );
        settings.materials.remove("Cape");
        settings.materials.insert("Skin".into(), id(ALBEDO));
        bind(&settings);
        let error = scratch_error(&scratch);
        assert!(matches!(&error, CookError::NotAMaterial { slot, .. } if slot == "Skin"), "{error:?}");
    }

    #[test]
    fn reports_bad_inputs_with_their_paths() {
        let scratch = Scratch::new("cook_errors");
        fs::create_dir_all(scratch.source()).unwrap();
        fs::write(scratch.source().join("model.gltf"), "{}").unwrap();
        scratch.add_mannequin("model.glb");
        let error = scratch_error(&scratch);
        assert!(matches!(error, CookError::OutputCollision { .. }), "{error:?}");

        fs::remove_file(scratch.source().join("model.glb")).unwrap();
        fs::remove_file(sidecar_path(&scratch.source().join("model.glb"))).unwrap();
        let error = scratch_error(&scratch);
        assert!(
            matches!(&error, CookError::Import { path, .. } if path.ends_with("model.gltf")),
            "{error:?}"
        );

        let error = cook_mesh(Path::new("model.obj"), &MeshSettings::default(), Cook::Desktop).unwrap_err();
        assert!(matches!(error, CookError::UnsupportedSource { .. }));
    }

    /// The FBX importer's fixtures (`tools/blender/fbx_fixtures.py`), by file name.
    fn fbx_fixture(name: &str) -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures/fbx").join(name)
    }

    #[test]
    fn fbx_sources_cook_as_their_gltf_twins_do() {
        let scratch = Scratch::new("cook_fbx");
        let add = |name: &str| {
            let path = scratch.source().join(name);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::copy(fbx_fixture(Path::new(name).file_name().unwrap().to_str().unwrap()), &path).unwrap();
            path
        };
        let pairs = ["static_multi", "skinned_morph"]
            .map(|name| (add(&format!("fbx/{name}.fbx")), add(&format!("gltf/{name}.glb"))));
        for cook in Cook::ALL {
            let report = scratch.cook_with(cook, false);
            for (fbx, gltf) in &pairs {
                assert!(matches!(meta::read(&sidecar_path(fbx)).unwrap().importer, ImportSettings::Fbx(_)));
                let (fbx_mesh, gltf_mesh) =
                    (read_mesh(&asset(&report, fbx).output), read_mesh(&asset(&report, gltf).output));
                // The same vertices and triangles, through the same bake, LODs and packing.
                assert_eq!(fbx_mesh.streams.len(), gltf_mesh.streams.len(), "{cook}: {}", fbx.display());
                let triangles = |mesh: &Mesh| -> Vec<u32> {
                    mesh.lods
                        .iter()
                        .map(|lod| lod.submeshes.iter().map(|s| s.index_count / 3).sum())
                        .collect()
                };
                assert_eq!(triangles(&fbx_mesh), triangles(&gltf_mesh), "{cook}: {}", fbx.display());
                let slots = |mesh: &Mesh| -> Vec<String> {
                    let mut names: Vec<String> =
                        mesh.materials.iter().map(|slot| slot.name.clone()).collect();
                    names.sort();
                    names
                };
                assert_eq!(slots(&fbx_mesh), slots(&gltf_mesh));
                let joints = |source: &Path| -> Vec<String> {
                    let id = meta::sub_asset_id(meta::read(&sidecar_path(source)).unwrap().id, SKELETON);
                    let cooked = report.assets.iter().find(|asset| asset.id == id);
                    cooked.map_or(Vec::new(), |asset| {
                        Skeleton::from_cooked(&read_aligned(&asset.output)).unwrap().names
                    })
                };
                assert_eq!(joints(fbx), joints(gltf));
                let targets = |mesh: &Mesh| -> Vec<String> {
                    let mut names: Vec<String> = mesh.morph_targets.iter().map(|t| t.name.clone()).collect();
                    names.sort();
                    names
                };
                assert_eq!(targets(&fbx_mesh), targets(&gltf_mesh));
            }
        }
    }

    #[test]
    fn sources_whose_sidecars_say_so_are_not_cooked() {
        let scratch = Scratch::new("cook_uncooked");
        let source = scratch.add_mannequin("props/mannequin.glb");
        let output = scratch.out().join("props/mannequin.smesh");
        scratch.cook();
        assert!(output.exists());
        let sidecar = sidecar_path(&source);
        let text = fs::read_to_string(&sidecar).unwrap();
        assert!(!text.contains("cook:"), "written only when false: {text}");

        let mut meta = meta::read(&sidecar).unwrap();
        meta.cook = false;
        fs::write(&sidecar, sapphire_serialize::to_ron(&meta).unwrap()).unwrap();
        assert!(fs::read_to_string(&sidecar).unwrap().contains("cook: false"));
        let report = scratch.cook();
        assert!(report.assets.is_empty());
        assert_eq!((report.uncooked, report.removed), (vec![source.clone()], vec![output.clone()]));
        assert!(!output.exists());
        // It keeps its ID, and a locked cook still needs its sidecar.
        assert_eq!(meta::read(&sidecar).unwrap().id, meta.id);
        fs::remove_file(&sidecar).unwrap();
        let options = CookOptions { cook: Cook::Desktop, cache: None, locked: true };
        let error = cook_dir(&scratch.source(), &scratch.out(), &options).unwrap_err();
        assert!(matches!(error, CookError::Meta(MetaError::Missing { .. })), "{error:?}");

        // Nothing may reference it: a slot bound to an uncooked material fails as a missing one.
        scratch.add_hair();
        let hair = scratch.source().join("hair/hair.material.ron");
        let mut hair_meta = meta::read(&sidecar_path(&hair)).unwrap();
        hair_meta.cook = false;
        fs::write(sidecar_path(&hair), sapphire_serialize::to_ron(&hair_meta).unwrap()).unwrap();
        let mut settings = MeshSettings::default();
        settings.materials.insert("Hair".into(), id(HAIR));
        let bound = AssetMeta::new(meta.id, ImportSettings::Gltf(settings));
        fs::write(&sidecar, sapphire_serialize::to_ron(&bound).unwrap()).unwrap();
        let error = scratch_error(&scratch);
        assert!(matches!(&error, CookError::NotAMaterial { slot, .. } if slot == "Hair"), "{error:?}");
    }

    /// A collider document with one box, named `name`, reaching `half_extent` along x each way.
    fn collider_document(name: &str, half_extent: f32) -> String {
        format!(
            r#"(
                type: "sapphire::colliders::ColliderDocument",
                version: 1,
                data: (colliders: [
                    (name: "{name}", shape: Box(half_extents: ({half_extent}, 0.5, 5.0)), position: (0.0, -0.5, 0.0)),
                ]),
            )"#
        )
    }

    #[test]
    fn collider_documents_cook_and_pack() {
        let scratch = Scratch::new("cook_colliders");
        let source = scratch.source().join("zones/arena.colliders.ron");
        fs::create_dir_all(source.parent().unwrap()).unwrap();
        fs::write(&source, collider_document("ground", 5.0)).unwrap();

        let report = scratch.cook();
        assert_eq!(report.created_sidecars, [sidecar_path(&source)]);
        assert!(fs::read_to_string(sidecar_path(&source)).unwrap().contains("Colliders"));
        let output = scratch.out().join("zones/arena.scol");
        assert_eq!(asset(&report, &source).output, output);
        let cooked = read_aligned(&output);
        assert_eq!(ColliderSet::read(&cooked).unwrap().id_of("ground"), Some(0));
        assert_eq!(crate::pack::kind(&output), Some(sapphire_assets::AssetKind::Colliders));

        // Unchanged, it comes from the cache; changed, it cooks again under the same ID.
        assert_eq!(statuses(&scratch.cook()), [CookStatus::UpToDate]);
        let id = report.assets[0].id;
        fs::write(&source, collider_document("ground", 6.0)).unwrap();
        let report = scratch.cook();
        assert_eq!(statuses(&report), [CookStatus::Cooked]);
        assert_eq!(report.assets[0].id, id);
    }

    #[test]
    fn a_removed_collider_documents_output_is_pruned() {
        let scratch = Scratch::new("cook_colliders_pruned");
        let source = scratch.source().join("arena.colliders.ron");
        fs::create_dir_all(scratch.source()).unwrap();
        fs::write(&source, collider_document("ground", 5.0)).unwrap();
        scratch.cook();
        let output = scratch.out().join("arena.scol");
        assert!(output.exists());

        fs::remove_file(&source).unwrap();
        fs::remove_file(sidecar_path(&source)).unwrap();
        let report = scratch.cook();
        assert!(!output.exists());
        assert!(report.removed.contains(&output), "{:?}", report.removed);
    }

    #[test]
    fn a_locked_cook_needs_a_collider_documents_sidecar() {
        let scratch = Scratch::new("cook_colliders_locked");
        let source = scratch.source().join("arena.colliders.ron");
        fs::create_dir_all(scratch.source()).unwrap();
        fs::write(&source, collider_document("ground", 5.0)).unwrap();
        let locked = CookOptions { cook: Cook::Desktop, cache: None, locked: true };
        let error = cook_dir(&scratch.source(), &scratch.out(), &locked).unwrap_err();
        assert!(
            matches!(&error, CookError::Meta(MetaError::Missing { sources }) if *sources == [source.clone()])
        );
        assert!(!sidecar_path(&source).exists());

        // With its sidecar written, the locked cook runs.
        scratch.cook();
        assert!(cook_dir(&scratch.source(), &scratch.out(), &locked).is_ok());
    }

    #[test]
    fn a_malformed_collider_document_names_its_file_and_its_entry() {
        let scratch = Scratch::new("cook_colliders_malformed");
        let source = scratch.source().join("arena.colliders.ron");
        fs::create_dir_all(scratch.source()).unwrap();
        fs::write(&source, collider_document("the_bad_wall", -1.0)).unwrap();
        let error = scratch_error(&scratch);
        assert!(matches!(&error, CookError::Colliders { path, .. } if *path == source), "{error:?}");
        let message = format!("{:#}", anyhow::Error::from(error));
        assert!(message.contains("arena.colliders.ron"), "{message}");
        assert!(message.contains("collider `the_bad_wall`: half extent -1"), "{message}");

        // Syntax too, with where it went wrong.
        fs::write(
            &source,
            "(type: \"sapphire::colliders::ColliderDocument\", version: 1, data: (colliders: [(",
        )
        .unwrap();
        let message = format!("{:#}", anyhow::Error::from(scratch_error(&scratch)));
        assert!(message.contains("arena.colliders.ron") && message.contains("1:"), "{message}");
    }

    /// The names in `dir`, sorted.
    fn listing(dir: &Path) -> Vec<String> {
        let mut names: Vec<String> = fs::read_dir(dir)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().into_string().unwrap())
            .collect();
        names.sort();
        names
    }

    /// An ILM map document whose shadow bias is `image` (relative to the document), its other
    /// channels constant.
    fn ilm_document(image: &str) -> String {
        format!(
            r#"(type: "sapphire::pipeline::IlmMap", version: 1, data: (shadow_bias: Image(path: "{image}")))"#
        )
    }

    /// A 4 × 4 gray PNG at `path`.
    fn add_mask(path: &Path) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        ::image::GrayImage::from_pixel(4, 4, ::image::Luma([128])).save(path).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn cooks_stay_inside_their_content_root() {
        // Review AP-004: a link under the root is refused before anything is created or cooked.
        let scratch = Scratch::new("cook_root");
        let mannequin = scratch.add_mannequin("a.glb");
        let outside = scratch.dir.path().join("outside");
        add_mask(&outside.join("bias.png"));
        let link = scratch.source().join("linked");
        std::os::unix::fs::symlink("../outside", &link).unwrap();
        let error = scratch_error(&scratch);
        assert!(matches!(&error, CookError::Meta(MetaError::Symlink { path }) if *path == link), "{error:?}");
        assert_eq!(listing(&outside), ["bias.png"], "no sidecar outside the root");
        assert!(!sidecar_path(&mannequin).exists() && !scratch.out().exists());

        // A map may name an image outside the root by a `..` path: the cook reads it, as an input
        // of the map's asset, and creates nothing there.
        fs::remove_file(&link).unwrap();
        let map = scratch.source().join("maps/body.ilm.ron");
        fs::create_dir_all(map.parent().unwrap()).unwrap();
        fs::write(&map, ilm_document("../../outside/bias.png")).unwrap();
        let report = scratch.cook();
        assert_eq!(report.created_sidecars, [sidecar_path(&mannequin), sidecar_path(&map)]);
        assert_eq!(asset(&report, &map).output, scratch.out().join("maps/body.ilm.ktx2"));
        assert_eq!(listing(&outside), ["bias.png"]);
    }

    #[test]
    fn outputs_that_differ_only_in_case_are_refused() {
        // Review AP-002: `tex.png` and `TEX.tga` cook to `tex.ktx2` and `TEX.ktx2`, one file where
        // case doesn't count, so the second overwrote the first and the report's hash went stale.
        let scratch = Scratch::new("cook_case");
        let color = || TextureClass::Color(ColorSettings::default());
        let lower = scratch.add_texture("props/tex.png", ALBEDO, color(), 1);
        scratch.cook();
        let out = scratch.out().join("props");
        let cooked = fs::read(out.join("tex.ktx2")).unwrap();
        let upper = scratch.source().join("props/TEX.tga");
        ::image::RgbaImage::from_pixel(16, 16, ::image::Rgba([9, 8, 7, 255]))
            .save_with_format(&upper, ::image::ImageFormat::Tga)
            .unwrap();
        write_sidecar(&upper, RAMP, ImportSettings::Texture(TextureSettings { class: color() }));
        let error = scratch_error(&scratch);
        let CookError::CaseCollision { first, second, outputs } = &error else { panic!("{error:?}") };
        assert_eq!((first, second), (&upper, &lower), "both sources are named, in tree order");
        assert_eq!(outputs, &[out.join("TEX.ktx2"), out.join("tex.ktx2")]);
        assert!(error.to_string().contains("differ only in case"), "{error}");
        assert_eq!(listing(&out), ["tex.ktx2"], "no output is written or removed");
        assert_eq!(fs::read(out.join("tex.ktx2")).unwrap(), cooked);

        // One source renamed only in case still cooks, its output renamed with it.
        fs::remove_file(&upper).unwrap();
        fs::remove_file(sidecar_path(&upper)).unwrap();
        let renamed = scratch.source().join("props/Tex.png");
        fs::rename(&lower, &renamed).unwrap();
        fs::rename(sidecar_path(&lower), sidecar_path(&renamed)).unwrap();
        let report = scratch.cook();
        assert_eq!(asset(&report, &renamed).output, out.join("Tex.ktx2"));
        assert_eq!(listing(&out), ["Tex.ktx2"]);
        assert_eq!(fs::read(out.join("Tex.ktx2")).unwrap(), cooked);
    }

    #[test]
    fn maps_that_dont_cook_are_not_checked() {
        // Review BR-001: a map whose sidecar says `cook: false` was parsed and its inputs checked
        // before the flag was read, so a draft or broken map failed the whole cook.
        let scratch = Scratch::new("cook_uncooked_maps");
        let map = scratch.source().join("maps/body.ilm.ron");
        fs::create_dir_all(map.parent().unwrap()).unwrap();
        let mut meta =
            AssetMeta::new(id(HAIR), ImportSettings::CharacterMap(MapKind::Ilm.default_settings()));
        meta.cook = false;
        let write_meta = |meta: &AssetMeta| {
            fs::write(sidecar_path(&map), sapphire_serialize::to_ron(meta).unwrap()).unwrap();
        };
        write_meta(&meta);

        // A document that doesn't parse.
        fs::write(&map, "not a map").unwrap();
        let report = scratch.cook();
        assert!(report.assets.is_empty());
        assert_eq!(report.uncooked, std::slice::from_ref(&map));

        // One naming a missing image and an image with a sidecar of its own: the images it names
        // without sidecars stay out of the cook rather than become textures, and those with
        // sidecars cook as the assets they are.
        let bias = scratch.source().join("maps/bias.png");
        add_mask(&bias);
        let ids =
            scratch.add_texture("maps/ids.png", ALBEDO, TextureClass::Color(ColorSettings::default()), 1);
        let document = r#"(type: "sapphire::pipeline::IlmMap", version: 1, data: (
            specular_mask: Image(path: "missing.png"),
            shadow_bias: Image(path: "bias.png"),
            specular_threshold: Image(path: "ids.png"),
        ))"#;
        fs::write(&map, document).unwrap();
        let report = scratch.cook();
        assert_eq!(report.assets.iter().map(|asset| &asset.source).collect::<Vec<_>>(), [&ids]);
        assert_eq!(report.uncooked, std::slice::from_ref(&map));
        assert!(report.created_sidecars.is_empty() && !sidecar_path(&bias).exists());

        // Cooked again, the map is checked again.
        meta.cook = true;
        write_meta(&meta);
        let error = scratch_error(&scratch);
        assert!(matches!(&error, CookError::Map { path, .. } if *path == map), "{error:?}");

        // A locked cook still needs the map's sidecar.
        fs::write(&map, ilm_document("bias.png")).unwrap();
        assert!(asset(&scratch.cook(), &map).output.exists());
        fs::remove_file(sidecar_path(&map)).unwrap();
        let options = CookOptions { cook: Cook::Desktop, cache: None, locked: true };
        let error = cook_dir(&scratch.source(), &scratch.out(), &options).unwrap_err();
        assert!(
            matches!(&error, CookError::Meta(MetaError::Missing { sources }) if *sources == [map.clone()]),
            "{error:?}"
        );
    }
}

#[cfg(test)]
mod clip_tests;

#[cfg(test)]
mod animation_tests;
#[cfg(test)]
mod move_tests;
#[cfg(test)]
mod zone_tests;
