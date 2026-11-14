//! The asset server (spec §5.10): pack mounting, handles, the load pipeline, the GPU upload
//! queue and residency, all of which [`AssetServer`]'s docs describe.

mod admission;
mod handle;
mod mounts;
mod reload;
mod source;

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet, VecDeque};
use std::panic::AssertUnwindSafe;
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, Sender};
use std::time::Duration;
use std::{error, fmt, io};

use sapphire_core::target::FormFactor;
use sapphire_serialize::cooked::AlignedVec;
use slotmap::SlotMap;

use self::admission::ReadOrder;
pub use self::handle::{Asset, Handle, TextureAsset, UntypedHandle, WeakHandle};
use self::handle::{HandleInner, SlotKey};
use self::mounts::{Mount, MountKey, OpenPack, PendingMount};
pub use self::mounts::{MountState, UnmountReport};
pub use self::reload::{AssetChange, RemountReport};
use self::reload::{Next, NextStage};
pub use self::source::{LoadPriority, MemoryPackSource, PackFile, PackSource, ReadDone};
use crate::manifest::{Manifest, ReleasedPack};
use crate::pack::{Decoder, Entry, PackError, PackMetadataBudget, PackMetadataRead, Toc};
use crate::texture_level::{MAX_LEVELS, TextureLevel};
use crate::{AssetId, AssetKind, PipelineKey};

const LOG_TARGET: &str = "sapphire::assets";
const MIB: u64 = 1 << 20;

/// Per-frame upload, load admission and per-kind residency budgets.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AssetBudgets {
    /// Bytes handed to the GPU per frame. One asset always goes, however large: splitting an
    /// asset across frames waits for texture mip streaming (later).
    pub upload_per_frame: u64,
    /// Bytes that may be read or decoded and not yet on the GPU at once ([admission](AssetServer#admission)):
    /// what a burst of loads can hold in memory before the uploads catch up. One read always goes, however
    /// large.
    pub admission: u64,
    /// Bytes of each kind resident, decoded, indexed by [`AssetKind::index`]. A kind counted in another's
    /// budget ([`Self::pool`]: texture levels, in the textures' pool) has no budget of its own, and its
    /// entry is unused.
    pub resident: [u64; AssetKind::ALL.len()],
}

impl AssetBudgets {
    /// Desktop (spec §5.10, §3.4): 16 MiB of uploads a frame and 256 MiB admitted, sixteen frames of them;
    /// meshes 384 MiB, textures 2.5 GiB (the zone texture pool), materials 32 MiB, rigs 64 MiB, probe grids
    /// 64 MiB, skeletons 16 MiB, clips 128 MiB, collider sets 16 MiB, sounds 32 MiB, cues 1 MiB, move sets
    /// 16 MiB, animation graphs 1 MiB, zones 16 MiB, string tables 8 MiB, fonts 64 MiB, font chains 1 MiB,
    /// navmeshes 16 MiB, timelines 4 MiB, UI documents 4 MiB, encounters 1 MiB, scripts 4 MiB, dialogues 1
    /// MiB, enemies 1 MiB, scores 1 MiB, voice lines 1 MiB; texture levels count in the textures' pool with
    /// their tails ([`Self::pool`], M5 M2). The numbers besides the texture pool's are proposals, to set by
    /// measuring zones (M2, M3). A quality tier sets its own texture pool on top ([`Self::with_resident`];
    /// `texture_pool_mib` in the renderer's `tiers.ron`, M5 M2).
    pub const DESKTOP: Self = Self {
        upload_per_frame: 16 * MIB,
        admission: 256 * MIB,
        resident: Self::by_kind([
            // What the pool holds under the device floor's 256 MiB a buffer: indices fill first, at a
            // bit over half a mesh's bytes (M3 W3).
            384 * MIB,
            2560 * MIB,
            32 * MIB,
            64 * MIB,
            64 * MIB,
            16 * MIB,
            128 * MIB,
            16 * MIB,
            32 * MIB,
            MIB,
            16 * MIB,
            MIB,
            16 * MIB,
            8 * MIB,
            64 * MIB,
            MIB,
            16 * MIB,
            4 * MIB,
            4 * MIB,
            MIB,
            4 * MIB,
            MIB,
            MIB,
            MIB,
            MIB,
            // Texture levels: counted in the textures' pool.
            0,
        ]),
    };
    /// Mobile: 4 MiB of uploads a frame and 64 MiB admitted, sixteen frames of them; meshes 192 MiB,
    /// textures 600 MiB (Mobile-High's pool: Mobile-Low's tier sets 400), materials 8 MiB, rigs 16 MiB,
    /// probe grids 16 MiB, skeletons 4 MiB, clips 32 MiB, collider sets 4 MiB, sounds 16 MiB, cues 1 MiB,
    /// move sets 8 MiB, animation graphs 1 MiB, zones 4 MiB, string tables 4 MiB, fonts 32 MiB, font chains
    /// 1 MiB, navmeshes 4 MiB, timelines 2 MiB, UI documents 4 MiB, encounters 1 MiB, scripts 2 MiB,
    /// dialogues 1 MiB, enemies 1 MiB, scores 1 MiB, voice lines 1 MiB; texture levels count in the textures'
    /// pool.
    pub const MOBILE: Self = Self {
        upload_per_frame: 4 * MIB,
        admission: 64 * MIB,
        resident: Self::by_kind([
            192 * MIB,
            600 * MIB,
            8 * MIB,
            16 * MIB,
            16 * MIB,
            4 * MIB,
            32 * MIB,
            4 * MIB,
            16 * MIB,
            MIB,
            8 * MIB,
            MIB,
            4 * MIB,
            4 * MIB,
            32 * MIB,
            MIB,
            4 * MIB,
            2 * MIB,
            4 * MIB,
            MIB,
            2 * MIB,
            MIB,
            MIB,
            MIB,
            MIB,
            // Texture levels: counted in the textures' pool.
            0,
        ]),
    };

    /// No limits at all: every upload goes in one frame, every read is admitted and nothing is evicted. For
    /// tools and tests that load everything and measure nothing.
    pub const UNLIMITED: Self =
        Self { upload_per_frame: u64::MAX, admission: u64::MAX, resident: [u64::MAX; AssetKind::ALL.len()] };

    /// The defaults for a form factor.
    pub const fn for_form_factor(form_factor: FormFactor) -> Self {
        match form_factor {
            FormFactor::Desktop => Self::DESKTOP,
            FormFactor::Mobile => Self::MOBILE,
        }
    }

    /// The residency budget `kind`'s bytes count against: its pool's ([`Self::pool`]).
    pub const fn resident(&self, kind: AssetKind) -> u64 {
        self.resident[Self::pool(kind).index()]
    }

    /// The kind whose residency budget `kind`'s bytes count against: a texture level's are its texture's,
    /// so a tail and the levels above it share the tier's one texture pool (m5-plan §3, "Mip streaming");
    /// every other kind's are its own. Eviction measures and frees a pool as one, least recently used
    /// first across its kinds.
    pub const fn pool(kind: AssetKind) -> AssetKind {
        match kind {
            AssetKind::TextureLevel => AssetKind::Texture,
            kind => kind,
        }
    }

    /// These budgets with `kind`'s residency budget `bytes`: a quality tier's texture pool, say.
    #[must_use]
    pub const fn with_resident(mut self, kind: AssetKind, bytes: u64) -> Self {
        self.resident[Self::pool(kind).index()] = bytes;
        self
    }

    /// Budgets in [`AssetKind::ALL`]'s order: meshes, textures, materials, rigs, probe grids,
    /// skeletons, clips, collider sets, sounds, cues, move sets, animation graphs, zones, string tables,
    /// fonts, font chains, navmeshes, timelines, UI documents, encounters, scripts, dialogues, enemies,
    /// scores, voice lines, texture levels.
    const fn by_kind(budgets: [u64; AssetKind::ALL.len()]) -> [u64; AssetKind::ALL.len()] {
        budgets
    }
}

impl Default for AssetBudgets {
    fn default() -> Self {
        Self::for_form_factor(FormFactor::CURRENT)
    }
}

/// Where an asset is in the load pipeline.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LoadState {
    /// Being read, decoded, or waiting for its dependencies or the GPU.
    Loading,
    /// It and all its dependencies are uploaded.
    Ready,
    Failed(LoadError),
}

/// Why an asset didn't load.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum LoadError {
    #[error("asset {0} is in no mounted pack")]
    NotFound(AssetId),
    #[error("asset {id} is a {found}, not a {expected}")]
    WrongKind { id: AssetId, expected: AssetKind, found: AssetKind },
    #[error("asset {id}, a {kind}, can't depend on asset {dependency}, a {dependency_kind}")]
    DependencyKind { id: AssetId, kind: AssetKind, dependency: AssetId, dependency_kind: AssetKind },
    #[error("dependency {dependency} of asset {id} failed to load")]
    Dependency { id: AssetId, dependency: AssetId },
    #[error("reading asset {id} failed: {message}")]
    Io { id: AssetId, message: String },
    #[error(transparent)]
    Pack(#[from] PackError),
    #[error("asset {id} isn't a valid {kind}: {message}")]
    Invalid { id: AssetId, kind: AssetKind, message: String },
    #[error("asset {id} references other assets than its pack lists")]
    References { id: AssetId },
    #[error("uploading asset {id} failed: {message}")]
    Upload { id: AssetId, message: String },
    #[error("decoding asset {id} panicked")]
    Panicked { id: AssetId },
    #[error("loading asset {id} can't make progress")]
    Stalled { id: AssetId },
    #[error("asset {0} released its decoded CPU bytes after upload")]
    CpuBytesReleased(AssetId),
    #[error("asset {0} left its pack, or its pack was unmounted, before it could be read")]
    Removed(AssetId),
}

/// A pack that couldn't be mounted.
#[derive(Debug, thiserror::Error)]
pub enum MountError {
    #[error("pack {0:?} is already mounted")]
    AlreadyMounted(String),
    #[error("failed to read pack {name:?}")]
    Io {
        name: String,
        #[source]
        source: io::Error,
    },
    #[error("pack {name:?} could not be loaded")]
    Pack {
        name: String,
        #[source]
        source: PackError,
    },
    #[error("asset {id} is in both pack {first:?} and pack {second:?}")]
    Duplicate { id: AssetId, first: String, second: String },
    #[error("pack {0:?} isn't mounted")]
    NotMounted(String),
    #[error("pack {0:?} is bound to the running stock fight")]
    BoundToStockFight(String),
    #[error("pack {0:?} isn't in this install's release")]
    NotReleased(String),
    #[error("pack {name:?} isn't the one this install's release names: {reason}")]
    NotAsReleased { name: String, reason: &'static str },
}

/// What an uploader returns for an asset it couldn't upload.
pub type UploadError = Box<dyn error::Error + Send + Sync>;

/// An asset's identity within one server's lifetime: unlike its [`AssetId`], an asset unloaded and
/// loaded again gets a new one. Apps key what they upload by it, so freeing an unloaded asset's
/// GPU copy can never free a newer copy of the same asset. Hot reload keeps it: a new version of
/// a loaded asset comes with the same slot and a higher [generation](Upload::generation).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct AssetSlot(SlotKey);

/// An asset handed to the app's uploader ([`AssetServer::upload`]): its decoded, checked bytes.
/// Its fields are read-only, since [`Self::get`] trusts the kind to read the bytes unchecked.
#[derive(Clone, Copy)]
pub struct Upload<'a> {
    id: AssetId,
    kind: AssetKind,
    slot: AssetSlot,
    generation: u32,
    data: &'a Arc<AlignedVec>,
}

impl<'a> Upload<'a> {
    pub fn id(&self) -> AssetId {
        self.id
    }

    pub fn kind(&self) -> AssetKind {
        self.kind
    }

    pub fn slot(&self) -> AssetSlot {
        self.slot
    }

    /// Which version of the asset this is: 0 when first loaded, one more for each version hot
    /// reload put behind its handles since, and for each time a dependency it binds was replaced
    /// ([`AssetKind::binds_dependencies`]). An app that holds an uploaded copy of the same
    /// [`slot`](Self::slot) at a lower generation replaces it in place; one at this generation is
    /// already current (assets are sent again after [`AssetServer::reupload_all`]).
    pub fn generation(&self) -> u32 {
        self.generation
    }

    /// The cooked bytes, 16-byte aligned.
    pub fn bytes(&self) -> &'a [u8] {
        self.data
    }

    /// The asset read as `T`, if it is of that kind: an `&ArchivedMesh`, an `&ArchivedMaterial` or
    /// a `Texture`, read in place without checking it again.
    pub fn get<T: Asset>(&self) -> Option<T::View<'a>> {
        // SAFETY: the bytes passed their kind's check in the decode job, and the server holds them
        // unchanged behind an `Arc` for as long as this borrow.
        (self.kind == T::KIND).then(|| unsafe { T::view(self.data) })
    }

    /// The asset as `T`, if it is of that kind, kept past the upload: the same checked bytes, shared, for
    /// an uploader that applies it later (a texture's level, until the levels between it and its texture's
    /// top have come, M5 M4). The server lets go of them as it would have. Kept bytes are the uploader's to
    /// bound: the upload ends their [admission](AssetServer#admission), since what's kept may wait for
    /// reads still held back, and their kind's pool counts them as the asset's size, as it does from the
    /// asset's load until it goes, kept or not.
    pub fn keep<T: Asset>(&self) -> Option<CpuResident<T>> {
        (self.kind == T::KIND)
            .then(|| CpuResident { data: Arc::clone(self.data), kind: std::marker::PhantomData })
    }
}

/// An immutable, checked CPU asset resident at the time the server issued it.
pub struct CpuResident<T: Asset> {
    data: Arc<AlignedVec>,
    kind: std::marker::PhantomData<fn() -> T>,
}

impl<T: Asset> Clone for CpuResident<T> {
    fn clone(&self) -> Self {
        Self { data: self.data.clone(), kind: std::marker::PhantomData }
    }
}

impl<T: Asset> CpuResident<T> {
    /// The same digest the pack verifies when it decodes this asset.
    #[must_use]
    pub fn content_hash(&self) -> [u8; 32] {
        *blake3::hash(&self.data).as_bytes()
    }

    /// Checks and keeps an immutable cooked asset without a server or a renderer.
    pub fn from_cooked(bytes: AlignedVec) -> Result<Self, crate::AssetError> {
        T::KIND.references(&bytes)?;
        Ok(Self { data: Arc::new(bytes), kind: std::marker::PhantomData })
    }

    /// The exact cooked bytes, for a package identity.
    #[must_use]
    pub fn cooked_bytes(&self) -> &[u8] {
        &self.data
    }

    pub fn view(&self) -> T::View<'_> {
        // SAFETY: obtained from the validated slot of a typed handle. The Arc keeps its
        // checked, aligned bytes alive and unchanged even if the server reloads the handle.
        unsafe { T::view(&self.data) }
    }
}

impl fmt::Debug for Upload<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Upload")
            .field("id", &self.id)
            .field("kind", &self.kind)
            .field("slot", &self.slot)
            .field("generation", &self.generation)
            .field("bytes", &self.data.len())
            .finish()
    }
}

/// An asset the server unloaded, whose uploaded copy the app may free.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Unloaded {
    pub id: AssetId,
    pub kind: AssetKind,
    pub slot: AssetSlot,
}

/// What one [`AssetServer::upload`] call did.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct UploadReport {
    pub assets: u32,
    pub bytes: u64,
    /// Assets still queued for the GPU.
    pub pending: u32,
}

/// The server's counters (rule R9), from [`AssetServer::stats`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct AssetStats {
    /// Assets with a slot: loading, loaded or failed.
    pub assets: u32,
    /// Being read or decoded.
    pub loading: u32,
    /// Decoded, waiting for dependencies or in the upload queue.
    pub waiting: u32,
    pub ready: u32,
    pub failed: u32,
    /// Resident without users: the cache residency evicts from.
    pub unused: u32,
    pub pinned: u32,
    /// Bytes resident or on their way, by [`AssetKind::index`].
    pub resident: [u64; AssetKind::ALL.len()],
    /// The most each budget held at an [`AssetServer::update`] since the server started, before that update
    /// trimmed it, by its pool's kind ([`AssetBudgets::pool`]): the texture pool's high-water mark, tails and
    /// levels together, against its budget, for one. A kind counted in another's pool has none of its own.
    pub resident_peak: [u64; AssetKind::ALL.len()],
    pub budgets: AssetBudgets,
    /// Bytes read or decoded and not yet on the GPU ([admission](AssetServer#admission)), against
    /// [`AssetBudgets::admission`], and the most there have been since the server started.
    pub admitted: u64,
    pub admitted_peak: u64,
    /// Reads waiting for admission: loading, but not yet asked of the source.
    pub held_back: u32,
    pub reads_in_flight: u32,
    /// Packs mounted, and packs still having their header read ([`AssetServer::mount`]).
    pub mounts: u32,
    pub mounting: u32,
    /// The last [`AssetServer::upload`] call.
    pub last_upload: UploadReport,
    /// Since the server started.
    pub loads: u64,
    pub unloads: u64,
    pub failures: u64,
    /// Ready assets loading a new version (hot reload).
    pub reloading: u32,
    /// New versions that replaced loaded assets, and those that failed, since the server
    /// started (hot reload).
    pub reloads: u64,
    pub reload_failures: u64,
}

/// An asset a mounted pack holds with a name ([`AssetServer::named`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NamedAsset<'a> {
    pub id: AssetId,
    pub name: &'a str,
    /// What it references, as its pack's table says: a clip's skeleton, for one.
    pub dependencies: &'a [AssetId],
}

/// One asset's place in the server.
struct Slot {
    /// The server's own reference: while its count is 1, nothing else uses the asset.
    handle: UntypedHandle,
    /// Where it is: a mount, and its entry in the mount's table of contents, which a remount
    /// moves to the new table's, or `None` once its pack no longer holds it (hot reload) or was
    /// unmounted, until a pack mounted or remounted later holds it: that pack's mount takes it over.
    /// The mount is live while there's an entry, and means nothing otherwise.
    mount: MountKey,
    entry: Option<u32>,
    /// The size and hash of the version `state` holds or is loading.
    size: u64,
    hash: [u8; 32],
    priority: LoadPriority,
    state: SlotState,
    /// Counts the reads started for it, so a completion of an earlier one is ignored. At most
    /// one is current: the main state's while it loads, else the next version's.
    request: u32,
    /// Decoded bytes, while it has them.
    data: Option<Arc<AlignedVec>>,
    /// Its dependencies, kept loaded by these handles.
    children: Vec<UntypedHandle>,
    /// The assets that depend on it, which may be waiting for it, or bind it.
    parents: Vec<SlotKey>,
    pins: u32,
    /// The bytes its main state holds against [admission](AssetServer#admission), from its read's admission
    /// until it's on the GPU or gone.
    admitted: Option<u64>,
    /// Where its read waits for admission, the main state's or the next version's, whichever is loading.
    held: Option<ReadOrder>,
    /// The last update that saw it in use, for LRU eviction.
    last_used: u64,
    /// Versions that replaced the first one behind its handles (see [`Upload::generation`]).
    generation: u32,
    /// It has been ready: the app holds a version of it.
    loaded: bool,
    /// The version `state` loads replaces one the app holds, so uploading it is a new
    /// generation: a reload that [`AssetServer::reupload_all`] took over. It stays set through a
    /// failure, since what is tried again still isn't what the app holds.
    replaces: bool,
    /// A new version on its way while the asset stays ready with the one it has (hot reload).
    /// Only a ready asset has one.
    next: Option<Next>,
}

impl Slot {
    fn users(&self) -> usize {
        Arc::strong_count(&self.handle.inner) - 1
    }

    /// A replacement reserves its decoded bytes while the current version stays live.
    /// Rebinding identical bytes shares the existing allocation and counts it once.
    fn resident_size(&self) -> u64 {
        if matches!(self.state, SlotState::Failed(_)) {
            return 0;
        }
        self.size + self.next.as_ref().filter(|next| next.hash != self.hash).map_or(0, |next| next.size)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum SlotState {
    /// Being read or decoded.
    Loading,
    /// Decoded; waiting for its dependencies.
    Waiting,
    /// In the upload queue.
    Queued,
    Ready,
    Failed(LoadError),
}

/// Which of a slot's versions an upload-queue entry is for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Version {
    /// The main state's: a load, or the version a new renderer is sent again.
    Main,
    /// [`Slot::next`]: a hot-reloaded version, or a rebinding.
    Next,
}

/// Something that finished off the server's thread.
enum Completion {
    /// A decode job's result.
    Asset { key: SlotKey, request: u32, result: Result<Arc<AlignedVec>, LoadError> },
    /// A pack opened, with its header and table of contents read and checked ([`AssetServer::mount`]).
    Mount { request: u64, result: Result<(OpenPack, Toc), MountError>, metadata: PackMetadataRead },
}

/// Mounts packs and loads the assets in them (spec §5.10).
///
/// # The load pipeline
///
/// ```text
/// load(id) ─► dependencies first ─► admission (bytes waiting for the GPU, by priority)
///          ─► IO read (PackSource, by priority) ─► decode job (worker)
///          ─► waits for its dependencies ─► upload queue (per-frame budget) ─► ready
/// ```
///
/// - **Request.** [`AssetServer::load`] looks the ID up in the mounted packs' tables of contents
///   and returns a [`Handle`] at once. The asset's dependencies, which its pack lists, are
///   requested first, so their reads queue ahead of its own.
/// - **Admission.** A read of a kind that goes to the GPU waits its turn until the bytes it will
///   decode to fit under [`AssetBudgets::admission`] (see [admission](AssetServer#admission)).
/// - **Read.** One read per asset, of its blocks' range, through the app's [`PackSource`]: the
///   platform's IO thread, which serves reads by [`LoadPriority`].
/// - **Decode job.** The read's completion starts a job on the async compute pool
///   (`sapphire_jobs::spawn_async`): each block's hash is checked, the block decompressed, the
///   asset's hash checked ([`Toc::decode_asset`]), then the asset is validated as its kind
///   ([`AssetKind::references`]) and its references compared with what its pack lists. Cooked
///   formats use relative pointers, so that validation is the whole fix-up: the decoded bytes are
///   read in place from then on. The result comes back over a channel that
///   [`AssetServer::update`] drains.
/// - **Dependencies.** A decoded asset waits until every dependency is ready, then joins the upload
///   queue. A failed dependency fails it. Children are therefore always on the GPU before the
///   assets that use them. A dependency must be of a lower kind ([`AssetKind::may_depend_on`]), so
///   no pack, however malformed, can make chains deep or circular. A failed asset stays failed
///   until it is loaded again, which tries it and its failed dependencies again, or until a pack
///   mounted or remounted may have fixed it ([hot reload](AssetServer#hot-reload)).
/// - **Upload.** [`AssetServer::upload`] hands queued assets to the app's uploader, in order,
///   until the frame's budget is spent ([`AssetBudgets::upload_per_frame`]; one asset always goes,
///   however large). The uploader gets plain cooked data ([`Upload`]) and calls the renderer, which
///   never sees handles or the server (rule R4). An uploaded asset is ready; its decoded bytes go
///   unless its kind keeps them ([`AssetKind::keeps_bytes`]). Kinds that never go to the GPU
///   (rigs, [`AssetKind::uploads`]) are ready as soon as their dependencies are, without the
///   queue.
///
/// # Texture levels
///
/// A large texture is cooked as a tail and levels ([`crate::texture_level`], M5 M3): the texture's own
/// asset is its tail, and each level above it is a [`TextureLevel`] that depends on the tail. For now,
/// whatever loads a texture as a dependency also loads every level of it the mounted packs hold, as
/// children of its own, after the tail: so a material is ready only once its textures are whole, and the
/// picture is what it was before textures split. Raising and lowering a texture a level at a time, by
/// distance and budget, replaces this (M5 M4, M5).
///
/// # Admission
///
/// A burst of loads (a zone's open, a new renderer's [`AssetServer::reupload_all`], a streamer
/// catching up) would otherwise read and decode everything at once and hold it all until the GPU
/// takes it a frame's budget at a time. So an asset of a kind that goes to the GPU holds its
/// decoded size against [`AssetBudgets::admission`] from the moment its read is asked of the source
/// until it's uploaded, fails or goes ([`AssetStats::admitted`]). A read that doesn't fit is held
/// back ([`AssetStats::held_back`]), and held reads are admitted as uploads give bytes back, most
/// urgent first, oldest first within a priority. One read is always admitted when nothing else is,
/// however large. Kinds that never go to the GPU aren't held: their decoded bytes are what stays
/// resident, and nothing waits on a frame's uploads to take them.
///
/// Admission can't stall a load behind what it waits for:
///
/// - **Dependencies first.** A read is admitted only once no dependency of its asset, at any depth, is
///   still held back, so an admitted asset never holds bytes while waiting for a read that can't be
///   admitted. Everything admitted reaches the GPU without anything more being admitted, and gives its
///   bytes back. A dependency read again while something admitted waits for it (a remount, hot reload)
///   is admitted at once, over the cap if it must be, and so is one an admitted asset comes to depend
///   on when it's read again with other bytes, though it was held back for another load.
/// - **Priority.** An asset's dependencies load at least as urgently as it does: asking for an asset
///   at a higher priority, by loading it again ([`AssetServer::load_with`]) or by
///   [`AssetServer::raise_priority`], raises it and every dependency it has, and a held read moves up
///   the queue with it, keeping its age. So a high-priority load waits for no low-priority read that
///   isn't admitted yet, and for an admitted one's bytes only: at most the cap, which the uploads drain
///   in [`AssetBudgets::admission`] over [`AssetBudgets::upload_per_frame`] frames. A read already asked
///   of the source keeps the priority it went with, and the upload queue stays in the order assets
///   decoded in.
///
/// # Handles
///
/// A [`Handle`] is strong: one pointer, cloned by bumping an atomic count. A [`WeakHandle`] is an
/// index and a generation, `Copy`, that resolves to nothing once its asset is unloaded. Dropping
/// handles allocates nothing and sends nothing, so it is allowed in the fixed step (rule R6); the
/// server sees an asset's last user go on its next [`AssetServer::update`].
///
/// A strong handle is always safe to hold, read and drop, whatever has happened to the pack its asset
/// came from: see [unmounting](AssetServer#unmounting).
///
/// # Mounting
///
/// [`AssetServer::mount`] returns at once. A worker opens the pack and reads and checks its header and
/// table of contents, so a frame never waits on a disk for them; the pack's assets become loadable on
/// the [`AssetServer::update`] after that, and [`AssetServer::mount_state`] says when, or why not. A
/// load screen or a tool that has nothing to do until then calls [`AssetServer::mount_now`].
/// [`AssetServer::remount`] still waits for the file on the calling thread: it runs when a pack was just
/// rewritten on a development machine or pushed to a device, and never in a shipped frame.
///
/// A shipping build holds its mounts to its release ([`AssetServer::mount_only_released`], M4 P1): a pack the
/// release manifest doesn't name, or whose length or header hash isn't the one it names, fails to mount
/// ([`MountError::NotReleased`], [`MountError::NotAsReleased`]), and since the header hash pins the table of
/// contents, which pins every block, the pack's bytes are the release's or fail their hash when read.
///
/// # Unmounting
///
/// [`AssetServer::unmount`] closes a pack, at a zone change say, and hands back what nothing wants of
/// it. From that call on, for each asset the pack held:
///
/// - **Nothing uses or pins it:** it's unloaded there and then, and [`AssetServer::take_unloaded`]
///   reports it. A [`WeakHandle`] to it resolves to nothing. Its dependencies in the same pack go with
///   it, and those in other packs lose a user and stay cached, as any unused asset does.
/// - **A handle or a pin holds it, and it's ready:** it stays ready, for as long as that lasts. The
///   handle reads what it read ([`AssetServer::get`] for a kind that keeps its bytes), the app's uploaded
///   copy stays valid, and its dependencies stay loaded. It's unloaded on the first
///   [`AssetServer::update`] after the last user goes, without waiting for its kind's budget, since its
///   bytes can't come back from any pack mounted now.
/// - **A handle holds it, and it's still loading** (a read queued or running, a decode running, waiting
///   for a dependency or for the GPU): it fails with [`LoadError::Removed`], at once, and so does
///   whatever was waiting for it. It never reaches the uploader. The read or decode in flight finishes
///   and is ignored.
/// - **A new version of it was on its way** (hot reload): that's dropped, and the asset keeps the version
///   it has.
/// - **A [`AssetServer::reupload_all`] later,** for a new renderer, an asset that kept its bytes goes
///   again from them, and one that didn't fails with [`LoadError::Removed`], there being nowhere to read
///   it from.
/// - **Mounting a pack that holds it again** takes it back as a remount would: the same bytes stay ready
///   behind the same handles, other bytes load as a new version, and a failed asset something still
///   wants loads again.
///
/// Nothing dangles either way: a strong handle owns its slot, and a slot owns its bytes. Asking for an
/// ID the pack held gives the live asset while there is one, and [`LoadError::NotFound`] after. An asset
/// of another pack that depends on one of this pack's keeps its dependency while it's ready, and fails
/// while it's still waiting for it. Unmounting while the pack is still mounting cancels it.
///
/// # Residency
///
/// Each [`AssetKind`] has a budget ([`AssetBudgets`]). Assets nothing uses stay resident, as a
/// cache, until their kind is over budget; then the least recently used of them are unloaded,
/// skipping pinned ones ([`AssetServer::pin`]: the current party, the active encounter). Assets in
/// use are never unloaded, so a kind can stay over budget, which [`AssetStats`] shows. An
/// unloaded asset is reported through [`AssetServer::take_unloaded`] so the app can free what it
/// uploaded. [`AssetServer::unload_unused`] drops the whole cache (`LowMemory`, spec §6.3).
///
/// # Hot reload
///
/// [`AssetServer::remount`] reads a mounted pack again after it changed: desktop dev builds
/// remount a pack when its file is rewritten (M1 A8, the desktop trigger of spec §5.10), and
/// live link (A9) when it pushes one. Assets the new pack holds with other bytes than the loaded
/// ones (by their blake3 hashes) load again through the same pipeline, while the version they
/// have stays ready and in use:
///
/// - The new version is read, decoded and checked on the same IO thread and workers, and its
///   dependencies, which its new table of contents lists, load first. It joins the upload queue
///   once they are ready, and waits for a dependency that is itself reloading, so a material
///   binds its texture's new version. It goes to the uploader within the same per-frame budget
///   as any load, as the asset's next [generation](Upload::generation) for the same
///   [`AssetSlot`]: the app replaces its copy in place.
/// - Once uploaded (or decoded, for kinds that never go to the GPU) it replaces the old version
///   behind every handle: [`AssetServer::get`] reads it, the asset's generation goes up
///   ([`AssetServer::generation`]), dependencies it no longer has are released, and an
///   [`AssetChange::Reloaded`] is reported ([`AssetServer::take_changes`]), so CPU systems that
///   derive data from it (animation from a rig) can tell.
/// - Assets that bind their dependencies' GPU objects ([`AssetKind::binds_dependencies`]: a
///   material its textures) are sent again from their kept bytes, as a generation of their own,
///   when a dependency is replaced.
/// - A new version that fails (a corrupt or invalid asset, a missing dependency, a refused
///   upload) is dropped and reported ([`AssetChange::ReloadFailed`]); the asset keeps the version
///   it had, though a new renderer gets the new version if the asset kept no bytes, since its pack
///   holds no other ([`AssetServer::reupload_all`]). An asset its pack no longer holds keeps its
///   version too ([`AssetChange::Removed`]), until nothing uses it. Unused cached assets that
///   changed are unloaded rather than reloaded, and assets still loading start again from the new
///   pack.
/// - An asset that left its pack moves to the next pack mounted or remounted that holds it, as
///   the same kind, and is compared with its entry there as a remount compares any asset. So an
///   asset moves between mounted packs by remounting the pack it left, then the pack it moved
///   to; the other order is refused, since an asset may be in one mounted pack only
///   ([`MountError::Duplicate`]), and the refused pack stays as it was mounted.
/// - A failed asset of another pack, still in use, that failed on a dependency a pack mounted or
///   remounted now holds loads again, with the failed assets that depend on it. One that failed
///   for itself (a refused upload, a corrupt block) waits for its own pack's remount or a load.
///
/// # Threading
///
/// The server lives on one thread (the main thread in apps) and takes `&mut self`. Handles are
/// `Send + Sync`. Reads complete on the IO thread and decodes on workers; neither touches the
/// server. Dropping the server closes the packs it holds open, without waiting for reads still in
/// flight: those may fail on the closed file, and their results go nowhere. A source may outlive it (an
/// `Arc` the app keeps, as live link's is), holding nothing open for it.
pub struct AssetServer {
    source: Arc<dyn PackSource>,
    budgets: AssetBudgets,
    metadata_budget: PackMetadataBudget,
    /// The packs a shipping build may mount, by name, if it's held to a release.
    release: Option<Arc<[ReleasedPack]>>,
    mounts: SlotMap<MountKey, Mount>,
    /// Packs a worker is opening, oldest first.
    pending_mounts: Vec<PendingMount>,
    /// Packs that couldn't be mounted, until they're asked for again or unmounted.
    failed_mounts: Vec<(String, MountError)>,
    /// Numbers [`Self::mount`]'s jobs, so the end of a cancelled one finds nothing.
    mount_requests: u64,
    /// Every mounted asset: its mount and entry.
    index: HashMap<AssetId, (MountKey, u32)>,
    slots: SlotMap<SlotKey, Slot>,
    by_id: HashMap<AssetId, SlotKey>,
    upload_queue: VecDeque<(SlotKey, Version)>,
    /// Reads waiting for admission, in the order they're admitted.
    held_back: BTreeMap<ReadOrder, SlotKey>,
    /// Numbers held reads, so they're admitted oldest first within a priority.
    read_sequence: u64,
    /// Bytes admitted and not yet on the GPU, and the most there have been.
    admitted: u64,
    admitted_peak: u64,
    /// Reused while choosing what to admit.
    admission_scratch: Vec<ReadOrder>,
    resident_peak: [u64; AssetKind::ALL.len()],
    sender: Sender<Completion>,
    receiver: Receiver<Completion>,
    reads_in_flight: u32,
    /// Counts updates: the clock LRU eviction runs on.
    frame: u64,
    unloaded: Vec<Unloaded>,
    /// Reused only while sorting an over-budget kind's unused assets.
    eviction_scratch: Vec<(u64, SlotKey)>,
    /// Hot reload's changes since the last [`Self::take_changes`].
    changes: Vec<AssetChange>,
    last_upload: UploadReport,
    loads: u64,
    unloads: u64,
    failures: u64,
    reloads: u64,
    reload_failures: u64,
}

impl fmt::Debug for AssetServer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let packs: Vec<&str> = self.mounts.values().map(|mount| mount.name.as_str()).collect();
        f.debug_struct("AssetServer")
            .field("packs", &packs)
            .field("stats", &self.stats())
            .finish_non_exhaustive()
    }
}

impl AssetServer {
    /// A server reading packs from `source`, within `budgets`.
    pub fn new(source: impl PackSource + 'static, budgets: AssetBudgets) -> Self {
        Self::with_metadata_budget(source, budgets, PackMetadataBudget::default())
    }

    /// Reads packs with an explicit transient metadata budget. Pass clones of one budget to
    /// readers whose queued and in-flight metadata work should share capacity.
    pub fn with_metadata_budget(
        source: impl PackSource + 'static,
        budgets: AssetBudgets,
        metadata_budget: PackMetadataBudget,
    ) -> Self {
        let (sender, receiver) = mpsc::channel();
        Self {
            source: Arc::new(source),
            budgets,
            metadata_budget,
            release: None,
            mounts: SlotMap::with_key(),
            pending_mounts: Vec::new(),
            failed_mounts: Vec::new(),
            mount_requests: 0,
            index: HashMap::new(),
            slots: SlotMap::with_key(),
            by_id: HashMap::new(),
            upload_queue: VecDeque::new(),
            held_back: BTreeMap::new(),
            read_sequence: 0,
            admitted: 0,
            admitted_peak: 0,
            admission_scratch: Vec::new(),
            resident_peak: [0; AssetKind::ALL.len()],
            sender,
            receiver,
            reads_in_flight: 0,
            frame: 0,
            unloaded: Vec::new(),
            eviction_scratch: Vec::new(),
            changes: Vec::new(),
            last_upload: UploadReport::default(),
            loads: 0,
            unloads: 0,
            failures: 0,
            reloads: 0,
            reload_failures: 0,
        }
    }

    pub fn budgets(&self) -> &AssetBudgets {
        &self.budgets
    }

    /// Holds every mount and remount from here on to `release`: a pack it doesn't name, or whose file isn't
    /// the length and header it names, fails to mount (see [mounting](AssetServer#mounting)). A shipping
    /// build's call, on its install's manifest, before it mounts anything; development builds, whose packs
    /// a cook or live link rewrites, don't make it.
    pub fn mount_only_released(&mut self, release: &Manifest) {
        self.release = Some(release.packs.clone().into());
    }

    /// Changes the budgets; a kind now over budget is trimmed on the next update.
    pub fn set_budgets(&mut self, budgets: AssetBudgets) {
        self.budgets = budgets;
    }

    /// What the meshes of every mounted pack draw through, each key once: what a load prewarms (M3 K1). A pack
    /// mounted later adds its own ([`Self::pipeline_keys_of`]).
    pub fn pipeline_keys(&self) -> BTreeSet<PipelineKey> {
        self.mounts.values().flat_map(|mount| mount.toc.pipeline_keys().iter().copied()).collect()
    }

    /// What the meshes of the mounted pack `pack` draw through, each key once: nothing if it isn't mounted.
    pub fn pipeline_keys_of(&self, pack: &str) -> BTreeSet<PipelineKey> {
        let mount = self.mounts.values().find(|mount| mount.name == pack);
        mount.iter().flat_map(|mount| mount.toc.pipeline_keys().iter().copied()).collect()
    }

    /// The assets of `kind` in the mounted pack `pack`, in its order: nothing if it isn't mounted. What finds a
    /// zone by its pack, so that no code has to name its ID.
    pub fn assets_of_kind(&self, pack: &str, kind: AssetKind) -> Vec<AssetId> {
        let mount = self.mounts.values().find(|mount| mount.name == pack);
        let entries = mount.iter().flat_map(|mount| mount.toc.entries());
        entries.filter(|entry| entry.kind == kind).map(|entry| entry.id).collect()
    }

    /// Every asset of `kind` the mounted packs hold with a name, pack by pack and by ID in each. It walks
    /// the packs' tables, so it's for a tool's start or a person's question rather than a frame: a game
    /// keeps the [`AssetId`].
    pub fn named(&self, kind: AssetKind) -> impl Iterator<Item = NamedAsset<'_>> {
        self.mounts.values().flat_map(move |mount| {
            mount.toc.named().filter(move |(entry, _)| entry.kind == kind).map(|(entry, name)| NamedAsset {
                id: entry.id,
                name,
                dependencies: mount.toc.dependencies(entry),
            })
        })
    }

    /// Whether a mounted pack holds asset `id`.
    pub fn contains(&self, id: AssetId) -> bool {
        self.index.contains_key(&id)
    }

    /// Whether asset `id` has a slot here, loading or loaded, used or cached.
    pub fn is_resident(&self, id: AssetId) -> bool {
        self.by_id.contains_key(&id)
    }

    /// Loads asset `id` as a `T` at [`LoadPriority::Normal`]: see [`Self::load_with`].
    pub fn load<T: Asset>(&mut self, id: AssetId) -> Result<Handle<T>, LoadError> {
        self.load_with(id, LoadPriority::Normal)
    }

    /// Loads asset `id` as a `T`, with its dependencies, and returns a handle to it at once. An
    /// asset already loaded or loading is shared, and loads at the more urgent of its priority and
    /// `priority` from then on ([`Self::raise_priority`]). Fails here only if no mounted pack has `id`
    /// or it isn't a `T`; later failures show in [`Self::state`].
    pub fn load_with<T: Asset>(
        &mut self,
        id: AssetId,
        priority: LoadPriority,
    ) -> Result<Handle<T>, LoadError> {
        let found = self.kind_of(id).ok_or(LoadError::NotFound(id))?;
        if found != T::KIND {
            return Err(LoadError::WrongKind { id, expected: T::KIND, found });
        }
        self.load_untyped(id, priority).map(Handle::new)
    }

    /// [`Self::load_with`] for an asset of any kind. An asset that failed is tried again, with its
    /// dependencies, so a transient failure (a read error, a refused upload) doesn't stick.
    pub fn load_untyped(&mut self, id: AssetId, priority: LoadPriority) -> Result<UntypedHandle, LoadError> {
        if let Some(&key) = self.by_id.get(&id) {
            let slot = &mut self.slots[key];
            slot.last_used = self.frame;
            let handle = slot.handle.clone();
            // One that no mounted pack holds has nothing to try again.
            let retry = matches!(slot.state, SlotState::Failed(_)) && slot.entry.is_some();
            self.raise(key, priority);
            if retry {
                self.resolve(key);
            }
            return Ok(handle);
        }
        let &(mount, entry_index) = self.index.get(&id).ok_or(LoadError::NotFound(id))?;
        let toc = self.mounts[mount].toc.clone();
        let entry = toc.entries()[entry_index as usize];
        let key = self.slots.insert_with_key(|key| Slot {
            handle: UntypedHandle { inner: Arc::new(HandleInner { key, id, kind: entry.kind }) },
            mount,
            entry: Some(entry_index),
            size: entry.size,
            hash: entry.hash,
            priority,
            state: SlotState::Loading,
            request: 0,
            data: None,
            children: Vec::new(),
            parents: Vec::new(),
            pins: 0,
            admitted: None,
            held: None,
            last_used: self.frame,
            generation: 0,
            loaded: false,
            replaces: false,
            next: None,
        });
        self.by_id.insert(id, key);
        self.loads += 1;
        self.resolve(key);
        Ok(self.slots[key].handle.clone())
    }

    /// Requests an asset's dependencies, which its pack lists, then its own read, so the
    /// dependencies' reads queue first. Dependencies must be of a lower kind
    /// ([`AssetKind::may_depend_on`]), so this recursion is at most three deep, and four through a
    /// texture's levels, which depend only on their texture ("Texture levels"). Dependencies it
    /// already holds are kept, failed ones are tried again, and ones its entry no longer lists
    /// (after a remount) are released.
    fn resolve(&mut self, key: SlotKey) {
        let slot = &self.slots[key];
        let id = slot.handle.id();
        let Some(entry) = slot.entry else {
            self.fail(key, LoadError::Removed(id));
            return;
        };
        let toc = self.mounts[slot.mount].toc.clone();
        let entry = toc.entries()[entry as usize];
        match self.load_dependencies(key, toc.dependencies(&entry)) {
            Ok(children) => {
                self.set_children(key, children);
                let slot = &mut self.slots[key];
                // Other bytes than the last version's are a new generation for an app holding it.
                slot.replaces |= slot.loaded && slot.hash != entry.hash;
                (slot.size, slot.hash) = (entry.size, entry.hash);
                self.request_read(key);
            }
            Err(error) => self.fail(key, error),
        }
    }

    /// Loads `ids`, the dependencies of the asset in `key`, and returns their handles, or why one
    /// can't load. They become its children only through [`Self::set_children`] or
    /// [`Self::link`].
    fn load_dependencies(&mut self, key: SlotKey, ids: &[AssetId]) -> Result<Vec<UntypedHandle>, LoadError> {
        let slot = &self.slots[key];
        let (id, kind, priority) = (slot.handle.id(), slot.handle.kind(), slot.priority);
        let mut children = Vec::with_capacity(ids.len());
        for &dependency in ids {
            let child = match self.kind_of(dependency) {
                None => Err(LoadError::NotFound(dependency)),
                Some(dependency_kind) if !kind.may_depend_on(dependency_kind) => {
                    Err(LoadError::DependencyKind { id, kind, dependency, dependency_kind })
                }
                Some(_) => self.load_untyped(dependency, priority),
            };
            match child {
                Ok(child) => {
                    // What holds a texture holds its levels too, for now: see "Texture levels". A level
                    // holds only its tail.
                    let texture = child.kind() == AssetKind::Texture && kind != AssetKind::TextureLevel;
                    children.push(child);
                    for level in if texture { self.levels_of(dependency) } else { Vec::new() } {
                        match self.load_untyped(level, priority) {
                            Ok(level) => children.push(level),
                            Err(error) => {
                                tracing::warn!(target: LOG_TARGET, %id, dependency = %level, %error, "texture level failed to load");
                                return Err(LoadError::Dependency { id, dependency: level });
                            }
                        }
                    }
                }
                Err(error) => {
                    tracing::warn!(target: LOG_TARGET, %id, %dependency, %error, "dependency failed to load");
                    return Err(match error {
                        LoadError::DependencyKind { .. } => error,
                        _ => LoadError::Dependency { id, dependency },
                    });
                }
            }
        }
        Ok(children)
    }

    /// The levels a mounted pack holds of texture `texture` above its tail, from its top: levels 0, 1
    /// and so on while there's one ([`TextureLevel::id`]).
    fn levels_of(&self, texture: AssetId) -> Vec<AssetId> {
        (0..MAX_LEVELS)
            .map(|level| TextureLevel::id(texture, level))
            .take_while(|&level| self.kind_of(level) == Some(AssetKind::TextureLevel))
            .collect()
    }

    /// Makes `children` the dependencies of the asset in `key`, releasing those it had and no
    /// longer has.
    fn set_children(&mut self, key: SlotKey, children: Vec<UntypedHandle>) {
        self.link(key, &children);
        let old = std::mem::replace(&mut self.slots[key].children, children);
        self.unlink(key, &old);
    }

    /// The assets that depend on the asset in `key`, as they are now: what each of them goes on to
    /// do may change the list (a parent's next version that fails unlinks its dependencies).
    fn parents(&self, key: SlotKey) -> Vec<SlotKey> {
        self.slots[key].parents.clone()
    }

    /// Records the asset in `key` as a parent of each of `children`.
    fn link(&mut self, key: SlotKey, children: &[UntypedHandle]) {
        for child in children {
            let parents = &mut self.slots[child.key()].parents;
            if !parents.contains(&key) {
                parents.push(key);
            }
        }
    }

    /// Forgets the asset in `key` as a parent of those of `released` that neither its
    /// dependencies nor its next version's still name.
    fn unlink(&mut self, key: SlotKey, released: &[UntypedHandle]) {
        let slot = &self.slots[key];
        let held = |child: &UntypedHandle| {
            slot.children.contains(child)
                || slot.next.as_ref().is_some_and(|next| next.children.contains(child))
        };
        let gone: Vec<SlotKey> =
            released.iter().filter(|child| !held(child)).map(UntypedHandle::key).collect();
        for child in gone {
            if let Some(child) = self.slots.get_mut(child) {
                child.parents.retain(|&parent| parent != key);
            }
        }
    }

    /// The kind of asset `id`, loaded or in a mounted pack.
    fn kind_of(&self, id: AssetId) -> Option<AssetKind> {
        if let Some(&key) = self.by_id.get(&id) {
            return Some(self.slots[key].handle.kind());
        }
        let &(mount, entry) = self.index.get(&id)?;
        Some(self.mounts[mount].toc.entries()[entry as usize].kind)
    }

    /// A strong handle from a weak one, if its asset is still loaded. A weak handle is only an
    /// index and a generation, so one from another server may find an unrelated asset of its kind
    /// here; strong handles are checked against the server they came from.
    pub fn upgrade<T: Asset>(&mut self, weak: WeakHandle<T>) -> Option<Handle<T>> {
        // The kind check keeps a foreign weak handle from typing another kind's slot as `T`.
        let slot = self.slots.get_mut(weak.key).filter(|slot| slot.handle.kind() == T::KIND)?;
        slot.last_used = self.frame;
        Some(Handle::new(slot.handle.clone()))
    }

    /// Where `handle`'s asset is in the load pipeline.
    pub fn state<T: Asset>(&self, handle: &Handle<T>) -> LoadState {
        self.untyped_state(handle.untyped())
    }

    /// [`Self::state`] for an untyped handle.
    pub fn untyped_state(&self, handle: &UntypedHandle) -> LoadState {
        match self.slot(handle).map(|slot| &slot.state) {
            Some(SlotState::Ready) => LoadState::Ready,
            Some(SlotState::Failed(error)) => LoadState::Failed(error.clone()),
            Some(_) => LoadState::Loading,
            None => LoadState::Failed(LoadError::NotFound(handle.id())),
        }
    }

    /// The asset read in place, while the server holds its decoded bytes: from decoding until it
    /// is uploaded, and after that for kinds that keep them ([`AssetKind::keeps_bytes`]).
    pub fn get<T: Asset>(&self, handle: &Handle<T>) -> Option<T::View<'_>> {
        let slot = self.slot(handle.untyped())?;
        let data = slot.data.as_ref()?;
        // SAFETY: the slot is the handle's own, so its kind is `T::KIND` (handles are typed from
        // it), its bytes passed that kind's check in the decode job, and they stay unchanged
        // behind the `Arc`.
        Some(unsafe { T::view(data) })
    }

    /// Retains the validated current bytes independently of future changes to the slot.
    pub fn cpu_resident<T: Asset>(&self, handle: &Handle<T>) -> Option<CpuResident<T>> {
        let data = self.slot(handle.untyped())?.data.as_ref()?.clone();
        Some(CpuResident { data, kind: std::marker::PhantomData })
    }

    /// Retains a decoded asset before any GPU upload. Loading screens can bind CPU
    /// consumers without advancing the renderer or marking a GPU asset as uploaded.
    /// `remaining` reads the caller's loading budget before each wait.
    pub fn finish_cpu<T: Asset>(
        &mut self,
        handle: &Handle<T>,
        remaining: &impl Fn() -> Duration,
    ) -> Result<CpuResident<T>, LoadError> {
        loop {
            if let Some(asset) = self.cpu_resident(handle) {
                return Ok(asset);
            }
            match self.state(handle) {
                LoadState::Ready => return Err(LoadError::CpuBytesReleased(handle.id())),
                LoadState::Failed(error) => return Err(error),
                LoadState::Loading => {}
            }
            let timeout = remaining();
            if timeout.is_zero() || !self.wait(timeout) {
                return Err(LoadError::Stalled { id: handle.id() });
            }
        }
    }

    /// The cooked bytes of asset `id`, 16-byte aligned, while the server holds them, as [`Self::get`] says:
    /// for a reader that goes by bytes, as a replay's digest of a zone does.
    pub fn cooked_bytes(&self, id: AssetId) -> Option<&[u8]> {
        let &key = self.by_id.get(&id)?;
        self.slots[key].data.as_ref().map(|data| data.as_slice())
    }

    /// Keeps `handle`'s asset resident even when nothing uses it, until as many [`Self::unpin`]s.
    pub fn pin<T: Asset>(&mut self, handle: &Handle<T>) {
        if self.slot(handle.untyped()).is_some() {
            self.slots[handle.key()].pins += 1;
        }
    }

    pub fn unpin<T: Asset>(&mut self, handle: &Handle<T>) {
        if self.slot(handle.untyped()).is_some() {
            let slot = &mut self.slots[handle.key()];
            debug_assert!(slot.pins > 0, "unpinned more than pinned");
            slot.pins = slot.pins.saturating_sub(1);
        }
    }

    /// The slot behind `handle`, if the handle is this server's: a strong handle keeps its slot,
    /// so only a handle from another server finds none, or another asset's.
    fn slot(&self, handle: &UntypedHandle) -> Option<&Slot> {
        self.slots.get(handle.key()).filter(|slot| Arc::ptr_eq(&slot.handle.inner, &handle.inner))
    }

    /// Applies finished reads and decodes, trims kinds over budget, then admits what fits of the reads
    /// held back. Call once a frame, outside the fixed step, before [`Self::upload`].
    pub fn update(&mut self) {
        profiling::scope!("AssetServer::update");
        self.frame += 1;
        while let Ok(completion) = self.receiver.try_recv() {
            self.complete(completion);
        }
        self.collect();
        self.admit();
    }

    /// Blocks until a read or decode in flight finishes, or `timeout` passes, and applies it.
    /// Returns whether one finished. For load screens and tools; frames use [`Self::update`].
    pub fn wait(&mut self, timeout: Duration) -> bool {
        self.admit();
        if self.reads_in_flight == 0 {
            return false;
        }
        match self.receiver.recv_timeout(timeout) {
            Ok(completion) => {
                self.complete(completion);
                true
            }
            Err(_) => false,
        }
    }

    /// Hands queued assets to `upload`, oldest first, until the frame's upload budget is spent
    /// (see [the load pipeline](AssetServer#the-load-pipeline)). An asset `upload` accepts is
    /// ready, and may let assets that depend on it join the queue, in this call too; one it
    /// rejects fails, with every asset waiting on it. Then reads held back are admitted into the
    /// bytes that went.
    pub fn upload(&mut self, mut upload: impl FnMut(Upload<'_>) -> Result<(), UploadError>) -> UploadReport {
        profiling::scope!("AssetServer::upload");
        let mut report = UploadReport::default();
        while let Some(&(key, version)) = self.upload_queue.front() {
            // An entry is stale once its version left the queue another way (failed, superseded,
            // unloaded).
            let queued = self.slots.get(key).and_then(|slot| match version {
                Version::Main => (slot.state == SlotState::Queued)
                    .then(|| (slot.size, slot.data.clone(), slot.generation + u32::from(slot.replaces))),
                Version::Next => {
                    let next = slot.next.as_ref().filter(|next| next.stage == NextStage::Queued)?;
                    Some((next.size, next.data.clone(), slot.generation + 1))
                }
            });
            let Some((size, data, generation)) = queued else {
                self.upload_queue.pop_front();
                continue;
            };
            if report.assets > 0 && report.bytes + size > self.budgets.upload_per_frame {
                break;
            }
            self.upload_queue.pop_front();
            let data = data.expect("queued versions hold their bytes");
            let handle = &self.slots[key].handle;
            let (id, kind) = (handle.id(), handle.kind());
            report.assets += 1;
            report.bytes += size;
            let result = upload(Upload { id, kind, slot: AssetSlot(key), generation, data: &data });
            let error = |error: UploadError| LoadError::Upload { id, message: error_chain(&*error) };
            match (version, result) {
                (Version::Main, Ok(())) => self.uploaded(key),
                (Version::Main, Err(error_)) => self.fail(key, error(error_)),
                (Version::Next, Ok(())) => self.swap(key),
                (Version::Next, Err(error_)) => self.fail_next(key, error(error_)),
            }
        }
        report.pending = self.upload_queue.len() as u32;
        self.last_upload = report;
        // What went gave its bytes back.
        self.admit();
        report
    }

    /// Drives the pipeline until `handle`'s asset is ready or has failed, uploading through
    /// `upload` within the per-frame budget each round: a load screen's loop, and how tools load.
    pub fn finish<T: Asset>(
        &mut self,
        handle: &Handle<T>,
        mut upload: impl FnMut(Upload<'_>) -> Result<(), UploadError>,
    ) -> Result<(), LoadError> {
        loop {
            self.update();
            let report = self.upload(&mut upload);
            match self.state(handle) {
                LoadState::Ready => return Ok(()),
                LoadState::Failed(error) => return Err(error),
                LoadState::Loading => {}
            }
            if report.assets == 0 && !self.wait(Duration::from_millis(100)) && self.reads_in_flight == 0 {
                return Err(LoadError::Stalled { id: handle.id() });
            }
        }
    }

    /// As [`Self::finish`], for an asset that needs no uploader: one of a kind that doesn't upload, whose
    /// dependencies don't either (a zone and its colliders). It waits until the asset and its dependencies are
    /// ready and hands nothing to an uploader, so uploads queued for anything else stay queued for the frame's
    /// (M4 H8: `finish` drains the whole queue through the uploader it's given). An asset that would need an
    /// upload to be ready stalls. `remaining` reads the caller's budget before each wait.
    pub fn finish_without_uploads<T: Asset>(
        &mut self,
        handle: &Handle<T>,
        remaining: &impl Fn() -> Duration,
    ) -> Result<(), LoadError> {
        loop {
            self.update();
            match self.state(handle) {
                LoadState::Ready => return Ok(()),
                LoadState::Failed(error) => return Err(error),
                LoadState::Loading => {}
            }
            let timeout = remaining();
            if timeout.is_zero() || !self.wait(timeout) {
                return Err(LoadError::Stalled { id: handle.id() });
            }
        }
    }

    /// Sends every uploaded asset through the upload queue again, dependencies first: for a new
    /// renderer, after device loss or a quality-tier change. Assets that kept their bytes are
    /// queued from them; the others are read and decoded again. An asset with a new version on
    /// its way (hot reload) is sent that one instead, as its next generation, since its pack may
    /// no longer hold the version it has. So is one whose new version failed to load and that
    /// kept no bytes: its pack holds only the new version, which loads as any new version does,
    /// dependencies first, and if it fails again the asset fails, with the reason. One its pack
    /// no longer holds at all fails, unless it kept its bytes.
    pub fn reupload_all(&mut self) {
        self.upload_queue.clear();
        // Dependencies first, so their reads are held back ahead of the assets waiting for them, and
        // admission never lets in an asset whose dependency it can't yet (see [admission](AssetServer#admission)).
        let mut keys = Vec::with_capacity(self.slots.len());
        let mut seen = HashSet::with_capacity(self.slots.len());
        for key in self.slots.keys() {
            self.dependencies_first(key, &mut seen, &mut keys);
        }
        for &key in &keys {
            let slot = &mut self.slots[key];
            if !(slot.handle.kind().uploads() && matches!(slot.state, SlotState::Ready | SlotState::Queued)) {
                continue;
            }
            if slot.next.is_some() {
                self.promote(key);
            } else if slot.data.is_some() {
                slot.state = SlotState::Waiting;
            } else if let Some(entry) = slot.entry {
                let entry = self.mounts[slot.mount].toc.entries()[entry as usize];
                if entry.hash == slot.hash { self.request_read(key) } else { self.resolve(key) }
            } else {
                let id = slot.handle.id();
                self.fail(key, LoadError::Removed(id));
            }
        }
        for key in keys {
            self.try_queue(key);
        }
    }

    /// Adds the asset in `key` to `order` after its dependencies, at any depth, each once.
    fn dependencies_first(&self, key: SlotKey, seen: &mut HashSet<SlotKey>, order: &mut Vec<SlotKey>) {
        if !seen.insert(key) {
            return;
        }
        let slot = &self.slots[key];
        let next_children = slot.next.iter().flat_map(|next| &next.children);
        for child in slot.children.iter().chain(next_children) {
            self.dependencies_first(child.key(), seen, order);
        }
        order.push(key);
    }

    /// Unloads asset `id` now, if nothing uses or pins it and it's loaded or failed, as a trim of its
    /// kind would, with any bytes a version reloading holds against [admission](AssetServer#admission),
    /// and reports it through [`Self::take_unloaded`]. Returns whether it went. For what an app gives back
    /// of its own accord: a texture's levels it lowered ("Texture levels"), which the next load must read
    /// and upload again.
    pub fn unload(&mut self, id: AssetId) -> bool {
        let Some(&key) = self.by_id.get(&id) else { return false };
        let evictable = Self::evictable(&self.slots[key]);
        if evictable {
            self.remove(key);
        }
        evictable
    }

    /// Unloads every asset nothing uses or pins, dependencies freed by that included: the
    /// response to `LowMemory` (spec §6.3).
    pub fn unload_unused(&mut self) {
        // A low-memory response must also release the high-water mark of LRU scratch.
        self.eviction_scratch = Vec::new();
        loop {
            let unused: Vec<SlotKey> =
                self.slots.iter().filter(|(_, slot)| Self::evictable(slot)).map(|(key, _)| key).collect();
            if unused.is_empty() {
                break;
            }
            for key in unused {
                self.remove(key);
            }
        }
    }

    /// The assets unloaded since the last call, for the app to free what it uploaded of them.
    /// Take them after [`Self::update`] and before [`Self::upload`].
    pub fn take_unloaded(&mut self) -> Vec<Unloaded> {
        std::mem::take(&mut self.unloaded)
    }

    /// The server's counters (rule R9).
    pub fn stats(&self) -> AssetStats {
        let mut stats = AssetStats {
            assets: self.slots.len() as u32,
            budgets: self.budgets,
            resident_peak: self.resident_peak,
            admitted: self.admitted,
            admitted_peak: self.admitted_peak,
            held_back: self.held_back.len() as u32,
            reads_in_flight: self.reads_in_flight,
            mounts: self.mounts.len() as u32,
            mounting: self.pending_mounts.len() as u32,
            last_upload: self.last_upload,
            loads: self.loads,
            unloads: self.unloads,
            failures: self.failures,
            reloads: self.reloads,
            reload_failures: self.reload_failures,
            ..AssetStats::default()
        };
        for slot in self.slots.values() {
            match slot.state {
                SlotState::Loading => stats.loading += 1,
                SlotState::Waiting | SlotState::Queued => stats.waiting += 1,
                SlotState::Ready => stats.ready += 1,
                SlotState::Failed(_) => stats.failed += 1,
            }
            stats.resident[slot.handle.kind().index()] += slot.resident_size();
            stats.unused += u32::from(slot.users() == 0);
            stats.pinned += u32::from(slot.pins > 0);
            stats.reloading += u32::from(slot.next.is_some());
        }
        stats
    }

    /// Starts reading and decoding the asset in `key` into its main state.
    fn request_read(&mut self, key: SlotKey) {
        let slot = &mut self.slots[key];
        slot.state = SlotState::Loading;
        slot.data = None;
        self.read(key);
    }

    /// Asks the source for the read of the asset in `key` that [`Self::read`] admitted, as its current
    /// request, and decodes it on a job.
    fn issue(&mut self, key: SlotKey) {
        let slot = &self.slots[key];
        let (request, priority) = (slot.request, slot.priority);
        let mount = &self.mounts[slot.mount];
        let (file, toc) = (mount.file.file(), mount.toc.clone());
        let entry = toc.entries()[slot.entry.expect("only assets their pack holds are read") as usize];
        let range = toc.stored_range(&entry);
        let sender = self.sender.clone();
        self.reads_in_flight += 1;
        let done: ReadDone = Box::new(move |result| match result {
            Ok(stored) => sapphire_jobs::spawn_async(move || {
                let result = std::panic::catch_unwind(AssertUnwindSafe(|| decode(&toc, &entry, &stored)))
                    .unwrap_or(Err(LoadError::Panicked { id: entry.id }));
                // A closed channel means the server is gone, and the result with it.
                let _ = sender.send(Completion::Asset { key, request, result });
            }),
            Err(error) => {
                let result = Err(LoadError::Io { id: entry.id, message: error.to_string() });
                let _ = sender.send(Completion::Asset { key, request, result });
            }
        });
        match usize::try_from(range.end - range.start) {
            Ok(len) => self.source.read(file, range.start, len, priority, done),
            Err(_) => done(Err(io::Error::other("the asset is larger than this platform's address space"))),
        }
    }

    /// Applies what finished off the thread.
    fn complete(&mut self, completion: Completion) {
        self.reads_in_flight -= 1;
        match completion {
            Completion::Asset { key, request, result } => self.complete_read(key, request, result),
            Completion::Mount { request, result, metadata } => {
                self.complete_mount(request, result);
                drop(metadata);
            }
        }
    }

    /// Applies a decode job's result, to the main state or the next version, whichever waits
    /// for it.
    fn complete_read(&mut self, key: SlotKey, request: u32, result: Result<Arc<AlignedVec>, LoadError>) {
        // A newer read replaced this one, or the version it was for failed or went meanwhile (a
        // dependency failed, a remount superseded it, the asset was unloaded).
        let Some(slot) = self.slots.get_mut(key).filter(|slot| slot.request == request) else { return };
        if slot.state == SlotState::Loading {
            match result {
                Ok(data) => {
                    slot.data = Some(data);
                    slot.state = SlotState::Waiting;
                    self.try_queue(key);
                }
                Err(error) => self.fail(key, error),
            }
        } else if let Some(next) = slot.next.as_mut().filter(|next| next.stage == NextStage::Loading) {
            match result {
                Ok(data) => {
                    next.data = Some(data);
                    next.stage = NextStage::Waiting;
                    self.try_queue_next(key);
                }
                Err(error) => self.fail_next(key, error),
            }
        }
    }

    /// Queues a decoded asset for the GPU once every dependency is ready, or fails it if one
    /// failed.
    fn try_queue(&mut self, key: SlotKey) {
        let Some(slot) = self.slots.get(key).filter(|slot| slot.state == SlotState::Waiting) else { return };
        let mut failed = None;
        for child in &slot.children {
            match &self.slots[child.key()].state {
                SlotState::Ready => {}
                SlotState::Failed(_) => {
                    failed = Some(LoadError::Dependency { id: slot.handle.id(), dependency: child.id() });
                    break;
                }
                _ => return,
            }
        }
        match failed {
            Some(error) => self.fail(key, error),
            // Kinds that don't go to the GPU are ready once their dependencies are.
            None if !self.slots[key].handle.kind().uploads() => self.uploaded(key),
            None => {
                self.slots[key].state = SlotState::Queued;
                self.upload_queue.push_back((key, Version::Main));
            }
        }
    }

    /// An asset is on the GPU, or needs no GPU: it is ready, and assets waiting on it may be too.
    fn uploaded(&mut self, key: SlotKey) {
        let slot = &mut self.slots[key];
        slot.state = SlotState::Ready;
        slot.loaded = true;
        let admitted = slot.admitted.take();
        self.release(admitted);
        let slot = &mut self.slots[key];
        if !slot.handle.kind().keeps_bytes() {
            slot.data = None;
        }
        if std::mem::take(&mut slot.replaces) {
            self.replaced(key);
        }
        for parent in self.parents(key) {
            self.try_queue(parent);
            self.try_queue_next(parent);
        }
    }

    /// Fails an asset, and every asset loading or waiting on it, or reloading with it.
    fn fail(&mut self, key: SlotKey, error: LoadError) {
        let slot = &mut self.slots[key];
        tracing::warn!(target: LOG_TARGET, id = %slot.handle.id(), kind = %slot.handle.kind(), %error, "asset failed to load");
        slot.state = SlotState::Failed(error);
        slot.data = None;
        let admitted = slot.admitted.take();
        self.release(admitted);
        self.failures += 1;
        // A failed asset has no version to replace.
        self.cancel_next(key);
        // A read still held back was its own.
        self.unhold(key);
        let child = self.slots[key].handle.clone();
        for parent in self.parents(key) {
            let Some(parent_slot) = self.slots.get(parent) else { continue };
            let error = LoadError::Dependency { id: parent_slot.handle.id(), dependency: child.id() };
            if matches!(parent_slot.state, SlotState::Loading | SlotState::Waiting | SlotState::Queued) {
                self.fail(parent, error);
            } else if parent_slot.next.as_ref().is_some_and(|next| next.children.contains(&child)) {
                self.fail_next(parent, error);
            }
        }
    }

    /// Whether residency may unload the asset: nothing uses or pins it, and it isn't in the
    /// pipeline.
    fn evictable(slot: &Slot) -> bool {
        slot.users() == 0 && slot.pins == 0 && matches!(slot.state, SlotState::Ready | SlotState::Failed(_))
    }

    /// Refreshes the use of assets in use, drops failed ones nothing uses and ones no mounted pack
    /// holds, whose bytes no pack would bring back, and trims each kind over budget, least recently
    /// used first.
    fn collect(&mut self) {
        let mut resident = [0u64; AssetKind::ALL.len()];
        let mut failed = Vec::new();
        let mut has_candidates = false;
        for (key, slot) in &mut self.slots {
            let users = slot.users();
            if users > 0 {
                slot.last_used = self.frame;
            }
            let evictable =
                users == 0 && slot.pins == 0 && matches!(slot.state, SlotState::Ready | SlotState::Failed(_));
            has_candidates |= evictable;
            match slot.state {
                SlotState::Failed(_) if evictable => failed.push(key),
                SlotState::Failed(_) => {}
                _ if evictable && slot.entry.is_none() => failed.push(key),
                _ => resident[AssetBudgets::pool(slot.handle.kind()).index()] += slot.resident_size(),
            }
        }
        for (peak, &bytes) in self.resident_peak.iter_mut().zip(&resident) {
            *peak = (*peak).max(bytes);
        }
        // Removal can drop the last dependency handle, which makes a candidate of what wasn't one, so
        // any removal keeps the scans going. A handle dropped concurrently after the snapshot may
        // instead be collected on the next update, just as one dropped after a kind's scan always was.
        let mut removed = !failed.is_empty();
        for key in failed {
            self.remove(key);
        }
        // Each pool once, by its own kind: texture levels go with the textures' pool.
        for kind in AssetKind::ALL.into_iter().filter(|&kind| AssetBudgets::pool(kind) == kind) {
            // An earlier kind's eviction can also remove holders of this kind. Keep the
            // initial one-pass snapshot on frames that have removed nothing.
            if removed {
                resident[kind.index()] = self
                    .slots
                    .values()
                    .filter(|slot| AssetBudgets::pool(slot.handle.kind()) == kind)
                    .map(Slot::resident_size)
                    .sum();
            }
            let excess = resident[kind.index()].saturating_sub(self.budgets.resident(kind));
            if excess > 0 && (removed || has_candidates) {
                removed |= self.evict(kind, excess);
            }
        }
    }

    /// Unloads unused assets in `kind`'s pool ([`AssetBudgets::pool`]), least recently used first, until
    /// `excess` bytes are gone or none are left. Reports whether removing one could have released
    /// dependencies.
    fn evict(&mut self, kind: AssetKind, mut excess: u64) -> bool {
        let mut candidates = std::mem::take(&mut self.eviction_scratch);
        candidates.clear();
        candidates.extend(
            self.slots
                .iter()
                .filter(|(_, slot)| AssetBudgets::pool(slot.handle.kind()) == kind && Self::evictable(slot))
                .map(|(key, slot)| (slot.last_used, key)),
        );
        candidates.sort_unstable();
        let mut removed = false;
        for &(_, key) in &candidates {
            if excess == 0 {
                break;
            }
            excess = excess.saturating_sub(self.slots[key].resident_size());
            self.remove(key);
            removed = true;
        }
        self.eviction_scratch = candidates;
        if excess > 0 {
            removed |= self.evict_holders(kind, excess);
        }
        removed
    }

    /// Unloads the unused assets that hold `kind`'s for nobody, least recently used first, with all they
    /// alone hold, until `excess` bytes of `kind`'s pool are gone or none are left. A cached mesh keeps its
    /// material in use, and that its textures, so without this a kind could stay over budget with nothing
    /// the app holds to blame. What the app holds or pins, and all that holds up, stays, and a cached
    /// asset that shares only that isn't touched: it would free nothing. Reports whether it removed any.
    fn evict_holders(&mut self, kind: AssetKind, mut excess: u64) -> bool {
        // Down from what the app holds: a handle beyond the parents' is the app's.
        let mut held = HashSet::new();
        let mut stack: Vec<SlotKey> = self
            .slots
            .iter()
            .filter(|(_, slot)| slot.pins > 0 || slot.users() > slot.parents.len())
            .map(|(key, _)| key)
            .collect();
        while let Some(key) = stack.pop() {
            if held.insert(key) {
                let slot = &self.slots[key];
                let next_children = slot.next.iter().flat_map(|next| &next.children);
                stack.extend(slot.children.iter().chain(next_children).map(UntypedHandle::key));
            }
        }
        // Up from what's left of this kind.
        let mut stack: Vec<SlotKey> = self
            .slots
            .iter()
            .filter(|&(key, slot)| AssetBudgets::pool(slot.handle.kind()) == kind && !held.contains(&key))
            .map(|(key, _)| key)
            .collect();
        let mut seen = HashSet::new();
        let mut holders = Vec::new();
        while let Some(key) = stack.pop() {
            for &parent in &self.slots[key].parents {
                if seen.insert(parent) {
                    stack.push(parent);
                    if Self::evictable(&self.slots[parent]) {
                        holders.push((self.slots[parent].last_used, parent));
                    }
                }
            }
        }
        holders.sort_unstable();
        let removed = !holders.is_empty();
        for (_, key) in holders {
            if excess == 0 {
                break;
            }
            // What an earlier one held alone went with it.
            if self.slots.contains_key(key) {
                excess = excess.saturating_sub(self.remove_with_what_it_holds(key, kind));
            }
        }
        removed
    }

    /// Unloads an asset nothing uses, and those it was the last user of. Returns how many bytes of
    /// `kind`'s pool went.
    fn remove_with_what_it_holds(&mut self, key: SlotKey, kind: AssetKind) -> u64 {
        let mut bytes = 0;
        let mut stack = vec![key];
        while let Some(key) = stack.pop() {
            let slot = &self.slots[key];
            if AssetBudgets::pool(slot.handle.kind()) == kind {
                bytes += slot.resident_size();
            }
            let next_children = slot.next.iter().flat_map(|next| &next.children);
            let children: Vec<SlotKey> =
                slot.children.iter().chain(next_children).map(UntypedHandle::key).collect();
            self.remove(key);
            for child in children {
                if !stack.contains(&child) && self.slots.get(child).is_some_and(Self::evictable) {
                    stack.push(child);
                }
            }
        }
        bytes
    }

    /// Unloads an asset nothing uses. Its dependencies lose a user, and go in a later collection
    /// if that was their last.
    fn remove(&mut self, key: SlotKey) {
        self.unhold(key);
        let slot = self.slots.remove(key).expect("removing a live slot");
        self.release(slot.admitted);
        self.release(slot.next.as_ref().and_then(|next| next.admitted));
        let next_children = slot.next.iter().flat_map(|next| &next.children);
        for child in slot.children.iter().chain(next_children) {
            self.slots[child.key()].parents.retain(|&parent| parent != key);
        }
        let (id, kind) = (slot.handle.id(), slot.handle.kind());
        self.by_id.remove(&id);
        self.unloads += 1;
        self.unloaded.push(Unloaded { id, kind, slot: AssetSlot(key) });
        tracing::debug!(target: LOG_TARGET, %id, %kind, size = slot.size, "asset unloaded");
    }
}

/// A decode job: blocks to asset, checked (see [the load pipeline](AssetServer#the-load-pipeline)).
fn decode(toc: &Toc, entry: &Entry, stored: &[u8]) -> Result<Arc<AlignedVec>, LoadError> {
    profiling::scope!("asset decode");
    // A decoder per job rather than per worker: its zstd context lives only while it decodes, so
    // idle workers hold no decoding memory.
    let bytes = toc.decode_asset(entry, stored, &mut Decoder::default())?;
    let references = entry.kind.references(&bytes).map_err(|error| LoadError::Invalid {
        id: entry.id,
        kind: entry.kind,
        message: error_chain(&error),
    })?;
    if references != toc.dependencies(entry) {
        return Err(LoadError::References { id: entry.id });
    }
    Ok(Arc::new(bytes))
}

/// An error and its sources, as one line.
fn error_chain(error: &(dyn error::Error + 'static)) -> String {
    let mut message = error.to_string();
    let mut source = error.source();
    while let Some(cause) = source {
        message.push_str(": ");
        message.push_str(&cause.to_string());
        source = cause.source();
    }
    message
}

#[cfg(test)]
mod tests;
