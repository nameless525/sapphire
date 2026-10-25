//! The asset server (spec §5.10): pack mounting, handles, the load pipeline, the GPU upload
//! queue and residency.
//!
//! # The load pipeline
//!
//! ```text
//! load(id) ─► dependencies first ─► IO read (PackSource, by priority) ─► decode job (worker)
//!          ─► waits for its dependencies ─► upload queue (per-frame budget) ─► ready
//! ```
//!
//! - **Request.** [`AssetServer::load`] looks the ID up in the mounted packs' tables of contents
//!   and returns a [`Handle`] at once. The asset's dependencies, which its pack lists, are
//!   requested first, so their reads queue ahead of its own.
//! - **Read.** One read per asset, of its blocks' range, through the app's [`PackSource`]: the
//!   platform's IO thread, which serves reads by [`LoadPriority`].
//! - **Decode job.** The read's completion starts a job on the async compute pool
//!   (`sapphire_jobs::spawn_async`): each block's hash is checked, the block decompressed, the
//!   asset's hash checked ([`Toc::decode_asset`]), then the asset is validated as its kind
//!   ([`AssetKind::references`]) and its references compared with what its pack lists. Cooked
//!   formats use relative pointers, so that validation is the whole fix-up: the decoded bytes are
//!   read in place from then on. The result comes back over a channel that
//!   [`AssetServer::update`] drains.
//! - **Dependencies.** A decoded asset waits until every dependency is ready, then joins the upload
//!   queue. A failed dependency fails it. Children are therefore always on the GPU before the
//!   assets that use them. A dependency must be of a lower kind ([`AssetKind::may_depend_on`]), so
//!   no pack, however malformed, can make chains deep or circular. A failed asset stays failed
//!   until it is loaded again, which tries it and its failed dependencies again.
//! - **Upload.** [`AssetServer::upload`] hands queued assets to the app's uploader, in order,
//!   until the frame's budget is spent ([`AssetBudgets::upload_per_frame`]; one asset always goes,
//!   however large). The uploader gets plain cooked data ([`Upload`]) and calls the renderer, which
//!   never sees handles or the server (rule R4). An uploaded asset is ready; its decoded bytes go
//!   unless its kind keeps them ([`AssetKind::keeps_bytes`]). Kinds that never go to the GPU
//!   (rigs, [`AssetKind::uploads`]) are ready as soon as their dependencies are, without the
//!   queue.
//!
//! # Handles
//!
//! A [`Handle`] is strong: one pointer, cloned by bumping an atomic count. A [`WeakHandle`] is an
//! index and a generation, `Copy`, that resolves to nothing once its asset is unloaded. Dropping
//! handles allocates nothing and sends nothing, so it is allowed in the fixed step (rule R6); the
//! server sees an asset's last user go on its next [`AssetServer::update`].
//!
//! # Residency
//!
//! Each [`AssetKind`] has a budget ([`AssetBudgets`]). Assets nothing uses stay resident, as a
//! cache, until their kind is over budget; then the least recently used of them are unloaded,
//! skipping pinned ones ([`AssetServer::pin`]: the current party, the active encounter). Assets in
//! use are never unloaded, so a kind can stay over budget, which [`AssetStats`] shows. An
//! unloaded asset is reported through [`AssetServer::take_unloaded`] so the app can free what it
//! uploaded. [`AssetServer::unload_unused`] drops the whole cache (`LowMemory`, spec §6.3).
//!
//! # Threading
//!
//! The server lives on one thread (the main thread in apps) and takes `&mut self`. Handles are
//! `Send + Sync`. Reads complete on the IO thread and decodes on workers; neither touches the
//! server.

mod handle;
mod source;

use std::collections::{HashMap, VecDeque};
use std::panic::AssertUnwindSafe;
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, Sender};
use std::time::Duration;
use std::{error, fmt, io};

use sapphire_core::target::FormFactor;
use sapphire_serialize::cooked::AlignedVec;
use slotmap::SlotMap;

pub use self::handle::{Asset, Handle, TextureAsset, UntypedHandle, WeakHandle};
use self::handle::{HandleInner, SlotKey};
pub use self::source::{LoadPriority, MemoryPackSource, PackFile, PackSource, ReadDone};
use crate::pack::{Decoder, Entry, HEADER_LEN, PackError, PackHeader, Toc};
use crate::{AssetId, AssetKind};

const LOG_TARGET: &str = "sapphire::assets";
const MIB: u64 = 1 << 20;

/// Per-frame upload and per-kind residency budgets.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AssetBudgets {
    /// Bytes handed to the GPU per frame. One asset always goes, however large: splitting an
    /// asset across frames waits for texture mip streaming (later).
    pub upload_per_frame: u64,
    /// Bytes of each kind resident, decoded, indexed by [`AssetKind::index`].
    pub resident: [u64; AssetKind::ALL.len()],
}

impl AssetBudgets {
    /// Desktop (spec §5.10, §3.4): 16 MiB of uploads a frame; meshes 768 MiB, textures 2.5 GiB
    /// (the zone texture pool), materials 32 MiB, rigs 64 MiB. The numbers besides the texture
    /// pool's are proposals, to set by measuring zones (M2, M3).
    pub const DESKTOP: Self = Self {
        upload_per_frame: 16 * MIB,
        resident: Self::by_kind(768 * MIB, 2560 * MIB, 32 * MIB, 64 * MIB),
    };
    /// Mobile: 4 MiB of uploads a frame; meshes 192 MiB, textures 600 MiB, materials 8 MiB, rigs
    /// 16 MiB.
    pub const MOBILE: Self =
        Self { upload_per_frame: 4 * MIB, resident: Self::by_kind(192 * MIB, 600 * MIB, 8 * MIB, 16 * MIB) };

    /// The defaults for a form factor.
    pub const fn for_form_factor(form_factor: FormFactor) -> Self {
        match form_factor {
            FormFactor::Desktop => Self::DESKTOP,
            FormFactor::Mobile => Self::MOBILE,
        }
    }

    /// The residency budget of `kind`.
    pub const fn resident(&self, kind: AssetKind) -> u64 {
        self.resident[kind.index()]
    }

    const fn by_kind(mesh: u64, texture: u64, material: u64, rig: u64) -> [u64; AssetKind::ALL.len()] {
        let mut budgets = [0; AssetKind::ALL.len()];
        budgets[AssetKind::Mesh.index()] = mesh;
        budgets[AssetKind::Texture.index()] = texture;
        budgets[AssetKind::Material.index()] = material;
        budgets[AssetKind::Rig.index()] = rig;
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
    #[error("pack {name:?} is invalid")]
    Pack {
        name: String,
        #[source]
        source: PackError,
    },
    #[error("asset {id} is in both pack {first:?} and pack {second:?}")]
    Duplicate { id: AssetId, first: String, second: String },
}

/// What an uploader returns for an asset it couldn't upload.
pub type UploadError = Box<dyn error::Error + Send + Sync>;

/// An asset's identity within one server's lifetime: unlike its [`AssetId`], a reloaded asset gets
/// a new one. Apps key what they upload by it, so freeing an unloaded asset's GPU copy can never
/// free a newer copy of the same asset.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct AssetSlot(SlotKey);

/// An asset handed to the app's uploader ([`AssetServer::upload`]): its decoded, checked bytes.
/// Its fields are read-only, since [`Self::get`] trusts the kind to read the bytes unchecked.
#[derive(Clone, Copy)]
pub struct Upload<'a> {
    id: AssetId,
    kind: AssetKind,
    slot: AssetSlot,
    bytes: &'a [u8],
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

    /// The cooked bytes, 16-byte aligned.
    pub fn bytes(&self) -> &'a [u8] {
        self.bytes
    }

    /// The asset read as `T`, if it is of that kind: an `&ArchivedMesh`, an `&ArchivedMaterial` or
    /// a `Texture`, read in place without checking it again.
    pub fn get<T: Asset>(&self) -> Option<T::View<'a>> {
        // SAFETY: the bytes passed their kind's check in the decode job, and the server holds them
        // unchanged behind an `Arc` for as long as this borrow.
        (self.kind == T::KIND).then(|| unsafe { T::view(self.bytes) })
    }
}

impl fmt::Debug for Upload<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Upload")
            .field("id", &self.id)
            .field("kind", &self.kind)
            .field("slot", &self.slot)
            .field("bytes", &self.bytes.len())
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
    pub budgets: AssetBudgets,
    pub reads_in_flight: u32,
    /// The last [`AssetServer::upload`] call.
    pub last_upload: UploadReport,
    /// Since the server started.
    pub loads: u64,
    pub unloads: u64,
    pub failures: u64,
}

/// A mounted pack.
struct Mount {
    name: String,
    file: PackFile,
    toc: Arc<Toc>,
}

/// One asset's place in the server.
struct Slot {
    /// The server's own reference: while its count is 1, nothing else uses the asset.
    handle: UntypedHandle,
    /// Where it is: a mount and an entry of its table of contents.
    mount: u32,
    entry: u32,
    size: u64,
    priority: LoadPriority,
    state: SlotState,
    /// Counts the reads started for it, so a completion of an earlier one is ignored.
    request: u32,
    /// Decoded bytes, while it has them.
    data: Option<Arc<AlignedVec>>,
    /// Its dependencies, kept loaded by these handles.
    children: Vec<UntypedHandle>,
    /// The assets that depend on it, which may be waiting for it.
    parents: Vec<SlotKey>,
    pins: u32,
    /// The last update that saw it in use, for LRU eviction.
    last_used: u64,
}

impl Slot {
    fn users(&self) -> usize {
        Arc::strong_count(&self.handle.inner) - 1
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

/// A decode job's result.
struct Completion {
    key: SlotKey,
    request: u32,
    result: Result<Arc<AlignedVec>, LoadError>,
}

/// Mounts packs and loads the assets in them. See the [module docs](self).
pub struct AssetServer {
    source: Box<dyn PackSource>,
    budgets: AssetBudgets,
    mounts: Vec<Mount>,
    /// Every mounted asset: its mount and entry.
    index: HashMap<AssetId, (u32, u32)>,
    slots: SlotMap<SlotKey, Slot>,
    by_id: HashMap<AssetId, SlotKey>,
    upload_queue: VecDeque<SlotKey>,
    sender: Sender<Completion>,
    receiver: Receiver<Completion>,
    reads_in_flight: u32,
    /// Counts updates: the clock LRU eviction runs on.
    frame: u64,
    unloaded: Vec<Unloaded>,
    last_upload: UploadReport,
    loads: u64,
    unloads: u64,
    failures: u64,
}

impl fmt::Debug for AssetServer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let packs: Vec<&str> = self.mounts.iter().map(|mount| mount.name.as_str()).collect();
        f.debug_struct("AssetServer")
            .field("packs", &packs)
            .field("stats", &self.stats())
            .finish_non_exhaustive()
    }
}

impl AssetServer {
    /// A server reading packs from `source`, within `budgets`.
    pub fn new(source: impl PackSource + 'static, budgets: AssetBudgets) -> Self {
        let (sender, receiver) = mpsc::channel();
        Self {
            source: Box::new(source),
            budgets,
            mounts: Vec::new(),
            index: HashMap::new(),
            slots: SlotMap::with_key(),
            by_id: HashMap::new(),
            upload_queue: VecDeque::new(),
            sender,
            receiver,
            reads_in_flight: 0,
            frame: 0,
            unloaded: Vec::new(),
            last_upload: UploadReport::default(),
            loads: 0,
            unloads: 0,
            failures: 0,
        }
    }

    pub fn budgets(&self) -> &AssetBudgets {
        &self.budgets
    }

    /// Changes the budgets; a kind now over budget is trimmed on the next update.
    pub fn set_budgets(&mut self, budgets: AssetBudgets) {
        self.budgets = budgets;
    }

    /// Mounts the pack called `name` ([`crate::pack::file_name`] in the source): reads and checks its
    /// header and table of contents, blocking until they are read, and makes its assets loadable.
    /// An asset may be in one mounted pack only.
    pub fn mount(&mut self, name: &str) -> Result<(), MountError> {
        profiling::scope!("AssetServer::mount");
        if self.mounts.iter().any(|mount| mount.name == name) {
            return Err(MountError::AlreadyMounted(name.to_owned()));
        }
        let io_error = |source| MountError::Io { name: name.to_owned(), source };
        let pack_error = |source| MountError::Pack { name: name.to_owned(), source };
        let (file, len) = self.source.open(name).map_err(io_error)?;
        let header = self.read_now(file, 0, HEADER_LEN.min(len as usize)).map_err(io_error)?;
        let header = PackHeader::read(&header).map_err(pack_error)?;
        let toc_range = header.toc_range();
        if toc_range.end > len {
            return Err(pack_error(PackError::Truncated { expected: header.file_len(), found: len }));
        }
        let toc = self
            .read_now(file, toc_range.start, (toc_range.end - toc_range.start) as usize)
            .map_err(io_error)?;
        let toc = Toc::read(header, &toc, len).map_err(pack_error)?;
        for entry in toc.entries() {
            if let Some(&(other, _)) = self.index.get(&entry.id) {
                let first = self.mounts[other as usize].name.clone();
                return Err(MountError::Duplicate { id: entry.id, first, second: name.to_owned() });
            }
        }
        let mount = self.mounts.len() as u32;
        self.index
            .extend(toc.entries().iter().enumerate().map(|(index, entry)| (entry.id, (mount, index as u32))));
        tracing::info!(
            target: LOG_TARGET,
            pack = name,
            assets = toc.entries().len(),
            bytes = len,
            "pack mounted"
        );
        self.mounts.push(Mount { name: name.to_owned(), file, toc: Arc::new(toc) });
        Ok(())
    }

    /// Whether a mounted pack holds asset `id`.
    pub fn contains(&self, id: AssetId) -> bool {
        self.index.contains_key(&id)
    }

    /// Loads asset `id` as a `T` at [`LoadPriority::Normal`]: see [`Self::load_with`].
    pub fn load<T: Asset>(&mut self, id: AssetId) -> Result<Handle<T>, LoadError> {
        self.load_with(id, LoadPriority::Normal)
    }

    /// Loads asset `id` as a `T`, with its dependencies, and returns a handle to it at once. An
    /// asset already loaded or loading is shared. Fails here only if no mounted pack has `id` or
    /// it isn't a `T`; later failures show in [`Self::state`].
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
            if matches!(slot.state, SlotState::Failed(_)) {
                slot.priority = priority;
                self.resolve(key);
            }
            return Ok(handle);
        }
        let &(mount, entry_index) = self.index.get(&id).ok_or(LoadError::NotFound(id))?;
        let toc = self.mounts[mount as usize].toc.clone();
        let entry = toc.entries()[entry_index as usize];
        let key = self.slots.insert_with_key(|key| Slot {
            handle: UntypedHandle { inner: Arc::new(HandleInner { key, id, kind: entry.kind }) },
            mount,
            entry: entry_index,
            size: entry.size,
            priority,
            state: SlotState::Loading,
            request: 0,
            data: None,
            children: Vec::new(),
            parents: Vec::new(),
            pins: 0,
            last_used: self.frame,
        });
        self.by_id.insert(id, key);
        self.loads += 1;
        self.resolve(key);
        Ok(self.slots[key].handle.clone())
    }

    /// Requests an asset's dependencies, which its pack lists, then its own read, so the
    /// dependencies' reads queue first. Dependencies must be of a lower kind
    /// ([`AssetKind::may_depend_on`]), so this recursion is at most three deep. Dependencies it
    /// already holds are kept; failed ones are tried again.
    fn resolve(&mut self, key: SlotKey) {
        let slot = &self.slots[key];
        let (id, kind, priority) = (slot.handle.id(), slot.handle.kind(), slot.priority);
        let toc = self.mounts[slot.mount as usize].toc.clone();
        let entry = toc.entries()[slot.entry as usize];
        let mut failure = None;
        for &dependency in toc.dependencies(&entry) {
            let child = match self.kind_of(dependency) {
                None => Err(LoadError::NotFound(dependency)),
                Some(dependency_kind) if !kind.may_depend_on(dependency_kind) => {
                    Err(LoadError::DependencyKind { id, kind, dependency, dependency_kind })
                }
                Some(_) => self.load_untyped(dependency, priority),
            };
            match child {
                Ok(child) if !self.slots[key].children.contains(&child) => {
                    self.slots[child.key()].parents.push(key);
                    self.slots[key].children.push(child);
                }
                Ok(_) => {}
                Err(error) => {
                    tracing::warn!(target: LOG_TARGET, %id, %dependency, %error, "dependency failed to load");
                    failure = Some(match error {
                        LoadError::DependencyKind { .. } => error,
                        _ => LoadError::Dependency { id, dependency },
                    });
                    break;
                }
            }
        }
        match failure {
            Some(error) => self.fail(key, error),
            None => self.request_read(key),
        }
    }

    /// The kind of asset `id`, loaded or in a mounted pack.
    fn kind_of(&self, id: AssetId) -> Option<AssetKind> {
        if let Some(&key) = self.by_id.get(&id) {
            return Some(self.slots[key].handle.kind());
        }
        let &(mount, entry) = self.index.get(&id)?;
        Some(self.mounts[mount as usize].toc.entries()[entry as usize].kind)
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

    /// Applies finished reads and decodes, then trims kinds over budget. Call once a frame,
    /// outside the fixed step, before [`Self::upload`].
    pub fn update(&mut self) {
        profiling::scope!("AssetServer::update");
        self.frame += 1;
        while let Ok(completion) = self.receiver.try_recv() {
            self.complete(completion);
        }
        self.collect();
    }

    /// Blocks until a read or decode in flight finishes, or `timeout` passes, and applies it.
    /// Returns whether one finished. For load screens and tools; frames use [`Self::update`].
    pub fn wait(&mut self, timeout: Duration) -> bool {
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
    /// (see the [module docs](self)). An asset `upload` accepts is ready, and may let assets that
    /// depend on it join the queue, in this call too; one it rejects fails, with every asset
    /// waiting on it.
    pub fn upload(&mut self, mut upload: impl FnMut(Upload<'_>) -> Result<(), UploadError>) -> UploadReport {
        profiling::scope!("AssetServer::upload");
        let mut report = UploadReport::default();
        while let Some(&key) = self.upload_queue.front() {
            let Some(slot) = self.slots.get(key).filter(|slot| slot.state == SlotState::Queued) else {
                self.upload_queue.pop_front();
                continue;
            };
            if report.assets > 0 && report.bytes + slot.size > self.budgets.upload_per_frame {
                break;
            }
            self.upload_queue.pop_front();
            let data = slot.data.clone().expect("queued assets hold their bytes");
            let (id, kind) = (slot.handle.id(), slot.handle.kind());
            report.assets += 1;
            report.bytes += slot.size;
            match upload(Upload { id, kind, slot: AssetSlot(key), bytes: &data }) {
                Ok(()) => self.uploaded(key),
                Err(error) => self.fail(key, LoadError::Upload { id, message: error_chain(&*error) }),
            }
        }
        report.pending = self.upload_queue.len() as u32;
        self.last_upload = report;
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

    /// Sends every uploaded asset through the upload queue again, dependencies first: for a new
    /// renderer, after device loss or a quality-tier change. Assets that kept their bytes are
    /// queued from them; the others are read and decoded again.
    pub fn reupload_all(&mut self) {
        self.upload_queue.clear();
        let keys: Vec<SlotKey> = self.slots.keys().collect();
        for &key in &keys {
            let slot = &mut self.slots[key];
            if slot.handle.kind().uploads() && matches!(slot.state, SlotState::Ready | SlotState::Queued) {
                if slot.data.is_some() {
                    slot.state = SlotState::Waiting;
                } else {
                    self.request_read(key);
                }
            }
        }
        for key in keys {
            self.try_queue(key);
        }
    }

    /// Unloads every asset nothing uses or pins, dependencies freed by that included: the
    /// response to `LowMemory` (spec §6.3).
    pub fn unload_unused(&mut self) {
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
            reads_in_flight: self.reads_in_flight,
            last_upload: self.last_upload,
            loads: self.loads,
            unloads: self.unloads,
            failures: self.failures,
            ..AssetStats::default()
        };
        for slot in self.slots.values() {
            match slot.state {
                SlotState::Loading => stats.loading += 1,
                SlotState::Waiting | SlotState::Queued => stats.waiting += 1,
                SlotState::Ready => stats.ready += 1,
                SlotState::Failed(_) => stats.failed += 1,
            }
            if !matches!(slot.state, SlotState::Failed(_)) {
                stats.resident[slot.handle.kind().index()] += slot.size;
            }
            stats.unused += u32::from(slot.users() == 0);
            stats.pinned += u32::from(slot.pins > 0);
        }
        stats
    }

    /// Reads a range of `file` and waits for it.
    fn read_now(&self, file: PackFile, offset: u64, len: usize) -> io::Result<Vec<u8>> {
        let (sender, receiver) = mpsc::sync_channel(1);
        let done = Box::new(move |result| {
            // The receiver waits below, so the send can't fail.
            let _ = sender.send(result);
        });
        self.source.read(file, offset, len, LoadPriority::Critical, done);
        receiver.recv().unwrap_or_else(|_| Err(io::Error::other("the pack source dropped a read")))
    }

    /// Starts reading and decoding the asset in `key`.
    fn request_read(&mut self, key: SlotKey) {
        let slot = &mut self.slots[key];
        slot.request = slot.request.wrapping_add(1);
        slot.state = SlotState::Loading;
        slot.data = None;
        let (request, priority) = (slot.request, slot.priority);
        let mount = &self.mounts[slot.mount as usize];
        let (file, toc) = (mount.file, mount.toc.clone());
        let entry = toc.entries()[slot.entry as usize];
        let range = toc.stored_range(&entry);
        let sender = self.sender.clone();
        self.reads_in_flight += 1;
        let done: ReadDone = Box::new(move |result| match result {
            Ok(stored) => sapphire_jobs::spawn_async(move || {
                let result = std::panic::catch_unwind(AssertUnwindSafe(|| decode(&toc, &entry, &stored)))
                    .unwrap_or(Err(LoadError::Panicked { id: entry.id }));
                // A closed channel means the server is gone, and the result with it.
                let _ = sender.send(Completion { key, request, result });
            }),
            Err(error) => {
                let result = Err(LoadError::Io { id: entry.id, message: error.to_string() });
                let _ = sender.send(Completion { key, request, result });
            }
        });
        match usize::try_from(range.end - range.start) {
            Ok(len) => self.source.read(file, range.start, len, priority, done),
            Err(_) => done(Err(io::Error::other("the asset is larger than this platform's address space"))),
        }
    }

    /// Applies a decode job's result.
    fn complete(&mut self, completion: Completion) {
        self.reads_in_flight -= 1;
        let Completion { key, request, result } = completion;
        // A newer read replaced this one, or the asset failed meanwhile (a dependency did).
        let current = self
            .slots
            .get(key)
            .is_some_and(|slot| slot.request == request && slot.state == SlotState::Loading);
        if !current {
            return;
        }
        match result {
            Ok(data) => {
                let slot = &mut self.slots[key];
                slot.data = Some(data);
                slot.state = SlotState::Waiting;
                self.try_queue(key);
            }
            Err(error) => self.fail(key, error),
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
                self.upload_queue.push_back(key);
            }
        }
    }

    /// An asset is on the GPU, or needs no GPU: it is ready, and assets waiting on it may be too.
    fn uploaded(&mut self, key: SlotKey) {
        let slot = &mut self.slots[key];
        slot.state = SlotState::Ready;
        if !slot.handle.kind().keeps_bytes() {
            slot.data = None;
        }
        for index in 0..self.slots[key].parents.len() {
            let parent = self.slots[key].parents[index];
            self.try_queue(parent);
        }
    }

    /// Fails an asset, and every asset loading or waiting on it.
    fn fail(&mut self, key: SlotKey, error: LoadError) {
        let slot = &mut self.slots[key];
        tracing::warn!(target: LOG_TARGET, id = %slot.handle.id(), kind = %slot.handle.kind(), %error, "asset failed to load");
        slot.state = SlotState::Failed(error);
        slot.data = None;
        self.failures += 1;
        for index in 0..self.slots[key].parents.len() {
            let parent = self.slots[key].parents[index];
            let waiting = self.slots.get(parent).is_some_and(|slot| {
                matches!(slot.state, SlotState::Loading | SlotState::Waiting | SlotState::Queued)
            });
            if waiting {
                let error = LoadError::Dependency {
                    id: self.slots[parent].handle.id(),
                    dependency: self.slots[key].handle.id(),
                };
                self.fail(parent, error);
            }
        }
    }

    /// Whether residency may unload the asset: nothing uses or pins it, and it isn't in the
    /// pipeline.
    fn evictable(slot: &Slot) -> bool {
        slot.users() == 0 && slot.pins == 0 && matches!(slot.state, SlotState::Ready | SlotState::Failed(_))
    }

    /// Refreshes the use of assets in use, drops failed ones nothing uses, and trims each kind over
    /// budget, least recently used first.
    fn collect(&mut self) {
        let mut resident = [0u64; AssetKind::ALL.len()];
        let mut failed = Vec::new();
        for (key, slot) in &mut self.slots {
            if slot.users() > 0 {
                slot.last_used = self.frame;
            }
            match slot.state {
                SlotState::Failed(_) if Self::evictable(slot) => failed.push(key),
                SlotState::Failed(_) => {}
                _ => resident[slot.handle.kind().index()] += slot.size,
            }
        }
        for key in failed {
            self.remove(key);
        }
        for kind in AssetKind::ALL {
            let excess = resident[kind.index()].saturating_sub(self.budgets.resident(kind));
            if excess > 0 {
                self.evict(kind, excess);
            }
        }
    }

    /// Unloads unused assets of `kind`, least recently used first, until `excess` bytes are gone
    /// or none are left.
    fn evict(&mut self, kind: AssetKind, mut excess: u64) {
        let mut candidates: Vec<(u64, SlotKey)> = self
            .slots
            .iter()
            .filter(|(_, slot)| slot.handle.kind() == kind && Self::evictable(slot))
            .map(|(key, slot)| (slot.last_used, key))
            .collect();
        candidates.sort_unstable();
        for (_, key) in candidates {
            if excess == 0 {
                break;
            }
            excess = excess.saturating_sub(self.slots[key].size);
            self.remove(key);
        }
    }

    /// Unloads an asset nothing uses. Its dependencies lose a user, and go in a later collection
    /// if that was their last.
    fn remove(&mut self, key: SlotKey) {
        let slot = self.slots.remove(key).expect("removing a live slot");
        for child in &slot.children {
            self.slots[child.key()].parents.retain(|&parent| parent != key);
        }
        let (id, kind) = (slot.handle.id(), slot.handle.kind());
        self.by_id.remove(&id);
        self.unloads += 1;
        self.unloaded.push(Unloaded { id, kind, slot: AssetSlot(key) });
        tracing::debug!(target: LOG_TARGET, %id, %kind, size = slot.size, "asset unloaded");
    }
}

/// A decode job: blocks to asset, checked (see the [module docs](self)).
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
