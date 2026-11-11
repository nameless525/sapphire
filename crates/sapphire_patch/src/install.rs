//! Installing a staged release, and which release an app starts on (see the [crate docs](crate)).

use std::collections::HashMap;
use std::fs::{self, File, OpenOptions, TryLockError};
use std::io::{self, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use sapphire_assets::manifest::{self, Manifest, ManifestError, PublicKey, ReleasedFile, ReleasedPack};
use sapphire_assets::pack::{self, HEADER_LEN, PackHeader, Toc};
use sapphire_platform::{ContentFs, OsSteps, ReplaceSteps, durable_replace_with};

use crate::download::table;
use crate::plan::{Installed, InstalledError};
use crate::staging::{Staging, StagingError};
use crate::{file_path, pack_path};

/// The content an app ships with, read-only: a directory on the desktop and in an iOS bundle, the APK's
/// assets on Android. Paths are from the content's root, `/`-separated: `release.smanifest`, `shared.spack`,
/// `documents/game.flow.ron`.
pub trait Shipped {
    /// The whole file at `path`.
    fn read(&self, path: &str) -> io::Result<Vec<u8>>;

    /// `len` bytes of the file at `path`, from `offset`: fewer only where the file ends.
    fn read_at(&self, path: &str, offset: u64, len: usize) -> io::Result<Vec<u8>>;
}

/// An app's own content, as the platform reads it: its bundle's directory, or its APK's assets.
impl Shipped for ContentFs {
    fn read(&self, path: &str) -> io::Result<Vec<u8>> {
        ContentFs::read(self, path).map_err(io::Error::other)
    }

    fn read_at(&self, path: &str, offset: u64, len: usize) -> io::Result<Vec<u8>> {
        let mut reader = self.open_reader(path).map_err(io::Error::other)?;
        let len = len.min(usize::try_from(reader.len().saturating_sub(offset)).unwrap_or(usize::MAX));
        let mut bytes = vec![0; len];
        reader.read_exact_at(offset, &mut bytes)?;
        Ok(bytes)
    }
}

/// Where a pack or a file of the current release is.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Location {
    /// In the content the app shipped with, under its own name: `<name>.spack`, or its path.
    Shipped,
    /// Installed by a patch, at this path: `<root>/packs/<header hash>.spack` or `<root>/files/<blake3>`.
    Patched(PathBuf),
}

/// The steps an install takes on the disk, so a test can fail or stop any one of them: [`ReplaceSteps`]'
/// for every file it writes (each pack and file is written beside its place, flushed and renamed into it, and
/// the manifest goes through [`durable_replace_with`]), and removing a file.
pub trait InstallSteps: ReplaceSteps {
    fn remove(&mut self, path: &Path) -> io::Result<()> {
        fs::remove_file(path)
    }

    /// Bytes free on the volume holding `dir`, which exists ([`sapphire_platform::free_space`]).
    fn free_space(&mut self, dir: &Path) -> io::Result<u64> {
        sapphire_platform::free_space(dir)
    }
}

impl InstallSteps for OsSteps {}

/// An install that went well.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InstallOutcome {
    /// The staged release is the current one: nothing was installed, and the staging is cleared.
    UpToDate { version: u64 },
    /// The staged release is current now. The release before it stays on the disk until
    /// [`Installation::confirm_launch`].
    Installed { from: u64, to: u64 },
}

/// Why an install didn't happen, or an installation can't be opened. Until the new manifest is in place, a
/// failed install leaves the current release as it was.
#[derive(Debug, thiserror::Error)]
pub enum InstallError {
    #[error("the content the app shipped with has no readable {}: {source}", manifest::FILE_NAME)]
    ShippedUnreadable {
        #[source]
        source: io::Error,
    },
    #[error("the content the app shipped with has a broken {}: {source}", manifest::FILE_NAME)]
    ShippedManifest {
        #[source]
        source: ManifestError,
    },
    #[error("{} is held by another installer", dir.display())]
    Busy { dir: PathBuf },
    #[error(transparent)]
    Staging(#[from] StagingError),
    #[error("nothing is staged to install")]
    NothingStaged,
    #[error("the staged release is refused: {0}")]
    Refused(ManifestError),
    #[error(
        "release {version} changes {path}, which the simulation reads, so only an app build may change it"
    )]
    NotPatchable { version: u64, path: String },
    #[error(transparent)]
    Installed(#[from] InstalledError),
    #[error("nothing staged or installed holds {what}: download the release again")]
    Unavailable { what: String },
    #[error("pack {pack} doesn't verify whole once assembled")]
    NotWhole { pack: String },
    #[error(
        "release {version} needs {needed} bytes more on the disk, and {free} are free with {} kept for the device",
        DISK_RESERVE
    )]
    NoRoom { version: u64, needed: u64, free: u64 },
    #[error("{}: {source}", path.display())]
    Io {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
}

/// What [`Installation::confirm_launch`] removed.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Cleaned {
    /// Files of earlier releases, and leftovers of killed installs, removed.
    pub removed: usize,
    /// Those that couldn't be: held open by another process, say. The next confirmed launch tries again.
    pub left: usize,
}

/// Bytes an install leaves free on the disk it writes to, so a patch never fills a device: past this it's
/// refused ([`InstallError::NoRoom`]) and the app goes on with what it has.
pub const DISK_RESERVE: u64 = 64 << 20;

const LOCK: &str = "lock";
const PART: &str = "part";
const PACKS: &str = "packs";
const FILES: &str = "files";

/// An app's content: what it shipped with, and the patches installed over it in a directory of its own (see
/// the [crate docs](crate) for the layout and the guarantees).
#[derive(Debug)]
pub struct Installation<S> {
    shipped: S,
    root: PathBuf,
    /// The keys this build trusts, compiled into it (m4-plan §3 "What's signed").
    root_keys: Vec<PublicKey>,
    /// The release the app shipped with, taking `root_keys` as its successors' if it names none.
    shipped_release: Manifest,
    /// The installed patch's release, if it's the current one.
    patch: Option<Manifest>,
    /// The patch's release the app opened, if any: what this process may have mounted, which a release it
    /// installs since doesn't change.
    opened: Option<Manifest>,
}

impl<S: Shipped> Installation<S> {
    /// Opens the content `shipped` holds and the patches installed under `root`, which needn't exist yet, in a
    /// build that trusts `root_keys`. The current release is the installed patch's, if `root` has one and it's
    /// whole, usable and signed by a key the build trusts, else the shipped one (see [`Self::current`]).
    ///
    /// **Keys** (M4 P4): a build compiles in the keys it trusts. A shipped release that names no keys for its
    /// successor, a cook's own unsigned one, takes `root_keys` as those, so the build's keys may sign the first
    /// patch of an unsigned package. A patch starts only if a key in `root_keys`, or one the shipped release
    /// names, signed it (`verify_strict`): one signed only by a key a later patch rotated to starts nothing until
    /// an app build trusts that key, so a release that rotates is signed by the old key and the new while builds
    /// move ([`Manifest`]'s "Keys").
    pub fn open(shipped: S, root: &Path, root_keys: &[PublicKey]) -> Result<Self, InstallError> {
        let bytes =
            shipped.read(manifest::FILE_NAME).map_err(|source| InstallError::ShippedUnreadable { source })?;
        let mut shipped_release =
            Manifest::read(&bytes).map_err(|source| InstallError::ShippedManifest { source })?;
        if shipped_release.successor_keys.is_empty() {
            shipped_release.successor_keys = root_keys.to_vec();
        }
        let mut installation = Self {
            shipped,
            root: root.to_owned(),
            root_keys: root_keys.to_vec(),
            shipped_release,
            patch: None,
            opened: None,
        };
        installation.reload();
        installation.opened = installation.patch.clone();
        Ok(installation)
    }

    /// The directory patches install into.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// The current release: the installed patch's if it's newer than the shipped release, of the same cook,
    /// leaves what the simulation reads as the app shipped it, and has every pack and file it installed at its
    /// size; else the shipped release. So an app updated past a patch starts on its own release, and a patch
    /// that lost a file starts nothing of its own.
    pub fn current(&self) -> &Manifest {
        self.patch.as_ref().unwrap_or(&self.shipped_release)
    }

    /// The current release's version.
    pub fn version(&self) -> u64 {
        self.current().version
    }

    /// Whether the current release is a patch's rather than the shipped one.
    pub fn is_patched(&self) -> bool {
        self.patch.is_some()
    }

    /// Where the current release's pack called `name` is, if it has one.
    pub fn pack(&self, name: &str) -> Option<Location> {
        self.current().pack(name).map(|pack| self.pack_location(pack))
    }

    /// Where the current release's file at `path` is, if it has one.
    pub fn file(&self, path: &str) -> Option<Location> {
        self.current().file(path).map(|file| self.file_location(file))
    }

    /// What the current release placed over the shipped content, by content path: each pack installed under
    /// the root (`<name>.spack`), each file (its path), and the root's manifest (`release.smanifest`). None
    /// when the current release is the shipped one. What `ContentFs::with_files_over` takes, so the patch's
    /// packs mount over the shipped ones.
    pub fn files_over(&self) -> Vec<(String, PathBuf)> {
        let Some(patch) = &self.patch else { return Vec::new() };
        let packs = patch.packs.iter().map(|pack| (pack::file_name(&pack.name), self.pack_location(pack)));
        let files = patch.files.iter().map(|file| (file.path.clone(), self.file_location(file)));
        let mut over: Vec<(String, PathBuf)> = packs
            .chain(files)
            .filter_map(|(path, location)| match location {
                Location::Patched(file) => Some((path, file)),
                Location::Shipped => None,
            })
            .collect();
        over.push((manifest::FILE_NAME.to_owned(), self.root.join(manifest::FILE_NAME)));
        over
    }

    /// Bytes an install of `next` would write beside the current release: each of its packs and files not
    /// shipped and not under the root already.
    pub fn bytes_to_install(&self, next: &Manifest) -> u64 {
        let packs = next.packs.iter().map(|pack| (self.pack_location(pack), pack.size));
        let files = next.files.iter().map(|file| (self.file_location(file), file.size));
        packs
            .chain(files)
            .filter_map(|(location, size)| match location {
                Location::Patched(path) if !path.exists() => Some(size),
                _ => None,
            })
            .sum()
    }

    /// The current release as a download plans over it (`Request::installed`): every pack's tables, read
    /// where it is.
    pub fn installed(&self) -> Result<Installed, InstalledError> {
        let mut readers = PackReaders::default();
        Installed::read(self.current().clone(), |pack, offset, len| {
            readers.read_at(self, &self.pack_location(pack), pack, offset, len)
        })
    }

    /// Installs the release a download staged in `staging`, at `now` (seconds since the Unix epoch) in app
    /// build `app_build` (see the [crate docs](crate)).
    pub fn install(
        &mut self,
        staging: &Path,
        now: u64,
        app_build: u32,
    ) -> Result<InstallOutcome, InstallError> {
        self.install_with(staging, now, app_build, &mut OsSteps)
    }

    /// [`Self::install`], through `steps`, so a test can fail or stop it at any step.
    pub fn install_with(
        &mut self,
        staging: &Path,
        now: u64,
        app_build: u32,
        steps: &mut impl InstallSteps,
    ) -> Result<InstallOutcome, InstallError> {
        let staging = Staging::open(staging)?;
        let _lock = self.lock()?;
        let signed = staging.manifest()?.ok_or(InstallError::NothingStaged)?;
        let from = self.version();
        let next = match self.current().accept_successor(&signed, now, app_build) {
            Ok(next) => next,
            Err(ManifestError::NotNewer { installed, offered }) if offered == installed => {
                clear(&staging, steps);
                return Ok(InstallOutcome::UpToDate { version: installed });
            }
            Err(error) => return Err(InstallError::Refused(error)),
        };
        if let Some(path) = self.current().unpatchable_change(&next) {
            return Err(InstallError::NotPatchable { version: next.version, path: path.to_owned() });
        }
        let installed = self.installed()?;
        // The packs dir exists: the lock made it.
        let needed = self.bytes_to_install(&next);
        let packs = self.root.join(PACKS);
        let free = steps.free_space(&packs).map_err(|source| InstallError::Io { path: packs, source })?;
        if needed.saturating_add(DISK_RESERVE) > free {
            return Err(InstallError::NoRoom { version: next.version, needed, free });
        }
        tracing::info!(target: "sapphire::patch", from, to = next.version, needed, "installing a release");

        let mut assembler = Assembler {
            installation: self,
            installed: &installed,
            staging: &staging,
            readers: PackReaders::default(),
        };
        for pack in &next.packs {
            assembler.pack(pack, steps)?;
        }
        for file in &next.files {
            assembler.file(file, steps)?;
        }
        for sub in [PACKS, FILES] {
            let dir = self.root.join(sub);
            steps.sync_dir(&dir).map_err(|source| InstallError::Io { path: dir, source })?;
        }

        // The one step that switches releases: everything it names is in place, whole and flushed.
        let pointer = self.root.join(manifest::FILE_NAME);
        let committed = durable_replace_with(&pointer, &signed, steps);
        // The rename may have happened even if a step after it failed: read back what's current.
        self.reload();
        committed.map_err(|source| InstallError::Io { path: pointer, source })?;
        tracing::info!(target: "sapphire::patch", from, to = self.version(), "release installed");
        clear(&staging, steps);
        Ok(InstallOutcome::Installed { from, to: self.version() })
    }

    /// Says the app launched well on the release it opened: removes what belongs neither to that release nor
    /// to the current one (which is another if this process has installed one since), so the release before
    /// them and whatever a killed install left half made go, and an installed patch the app no longer starts
    /// on. Until then the release before stays whole, since a process that started on it may still mount its
    /// packs; a process that installs a release keeps the one it opened until the next launch confirms.
    pub fn confirm_launch(&mut self) -> Result<Cleaned, InstallError> {
        if !self.root.exists() {
            return Ok(Cleaned::default());
        }
        let _lock = self.lock()?;
        // Another process may have installed a release since this one read the disk.
        self.reload();
        let mut cleaned = Cleaned::default();
        let mut remove = |path: &Path| match fs::remove_file(path) {
            Ok(()) => cleaned.removed += 1,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => {
                let shown = path.display();
                tracing::warn!(target: "sapphire::patch", path = %shown, %error, "an earlier release's file stays");
                cleaned.left += 1;
            }
        };
        let pointer = self.root.join(manifest::FILE_NAME);
        if self.patch.is_none() {
            remove(&pointer);
        }
        remove(&pointer.with_added_extension("tmp"));
        let mut keep: Vec<PathBuf> = Vec::new();
        for patch in [&self.patch, &self.opened].into_iter().flatten() {
            let packs = patch.packs.iter().map(|pack| self.pack_location(pack));
            let files = patch.files.iter().map(|file| self.file_location(file));
            keep.extend(packs.chain(files).filter_map(|location| match location {
                Location::Patched(path) => Some(path),
                Location::Shipped => None,
            }));
        }
        for sub in [PACKS, FILES] {
            let dir = self.root.join(sub);
            let entries = match fs::read_dir(&dir) {
                Ok(entries) => entries,
                Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
                Err(source) => return Err(InstallError::Io { path: dir, source }),
            };
            for entry in entries {
                let path = entry.map_err(|source| InstallError::Io { path: dir.clone(), source })?.path();
                if !keep.contains(&path) {
                    remove(&path);
                }
            }
        }
        tracing::info!(target: "sapphire::patch", version = self.version(), ?cleaned, "launch confirmed");
        Ok(cleaned)
    }

    /// Reads which release is current from the disk again.
    fn reload(&mut self) {
        self.patch = match self.read_patch() {
            Ok(patch) => patch,
            Err(why) => {
                let root = self.root.display();
                tracing::warn!(target: "sapphire::patch", root = %root, why, "the installed patch isn't used");
                None
            }
        };
    }

    /// The installed patch's release, if there is one and it's usable (see [`Self::current`]).
    fn read_patch(&self) -> Result<Option<Manifest>, String> {
        let path = self.root.join(manifest::FILE_NAME);
        let bytes = match fs::read(&path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(format!("its manifest can't be read: {error}")),
        };
        let shipped = &self.shipped_release;
        let trusted: Vec<PublicKey> = self.root_keys.iter().chain(&shipped.successor_keys).copied().collect();
        let patch = Manifest::verify(&bytes, &trusted)
            .map_err(|error| format!("its manifest isn't one this build takes: {error}"))?;
        if patch.version <= shipped.version {
            return Err(format!("it's release {}, and the app shipped {}", patch.version, shipped.version));
        }
        if patch.cook != shipped.cook {
            return Err(format!("it's the {} cook, and the app's is {}", patch.cook, shipped.cook));
        }
        if let Some(path) = shipped.unpatchable_change(&patch) {
            return Err(format!("{path} isn't what the app shipped, and the simulation reads it"));
        }
        let sizes = patch
            .packs
            .iter()
            .map(|pack| (self.pack_location(pack), pack.size))
            .chain(patch.files.iter().map(|file| (self.file_location(file), file.size)));
        for (location, size) in sizes {
            if let Location::Patched(path) = location
                && fs::metadata(&path).map(|metadata| metadata.len()).ok() != Some(size)
            {
                return Err(format!("{} is missing or the wrong size", path.display()));
            }
        }
        Ok(Some(patch))
    }

    /// Where any release keeps `pack`: in the shipped content if the app shipped the same pack under the same
    /// name, else under the root by its header hash.
    fn pack_location(&self, pack: &ReleasedPack) -> Location {
        if self.shipped_release.pack(&pack.name) == Some(pack) {
            Location::Shipped
        } else {
            Location::Patched(self.root.join(pack_path(&pack.header_hash)))
        }
    }

    /// Where any release keeps `file`: as [`Self::pack_location`], by path and blake3.
    fn file_location(&self, file: &ReleasedFile) -> Location {
        if self.shipped_release.file(&file.path) == Some(file) {
            Location::Shipped
        } else {
            Location::Patched(self.root.join(file_path(&file.hash)))
        }
    }

    /// Holds the root against another installer or a confirmed launch's cleaning, making it if it isn't there.
    fn lock(&self) -> Result<File, InstallError> {
        let io_error = |path: &Path| {
            let path = path.to_owned();
            move |source| InstallError::Io { path, source }
        };
        for sub in [PACKS, FILES] {
            let dir = self.root.join(sub);
            fs::create_dir_all(&dir).map_err(io_error(&dir))?;
        }
        let path = self.root.join(LOCK);
        let lock = OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(&path)
            .map_err(io_error(&path))?;
        match lock.try_lock() {
            Ok(()) => Ok(lock),
            Err(TryLockError::WouldBlock) => Err(InstallError::Busy { dir: self.root.clone() }),
            Err(TryLockError::Error(source)) => Err(InstallError::Io { path, source }),
        }
    }
}

/// Clears what was staged once it's installed, or was already. A failure here leaves the install as it is:
/// a staging that still names the current release is cleared by the next install or download.
fn clear(staging: &Staging, steps: &mut impl InstallSteps) {
    if let Err(error) = staging.clear_with(|path| steps.remove(path)) {
        tracing::warn!(target: "sapphire::patch", %error, "the staging stays until the next install");
    }
}

/// Open packs installed under the root, so reading a pack's blocks one by one doesn't open it each time.
#[derive(Default)]
struct PackReaders {
    open: HashMap<PathBuf, File>,
}

impl PackReaders {
    /// `len` bytes of `pack` from `offset`, wherever `location` says it is: fewer only where it ends.
    fn read_at<S: Shipped>(
        &mut self,
        installation: &Installation<S>,
        location: &Location,
        pack: &ReleasedPack,
        offset: u64,
        len: usize,
    ) -> io::Result<Vec<u8>> {
        match location {
            Location::Shipped => installation.shipped.read_at(&pack::file_name(&pack.name), offset, len),
            Location::Patched(path) => {
                let file = match self.open.entry(path.clone()) {
                    std::collections::hash_map::Entry::Occupied(open) => open.into_mut(),
                    std::collections::hash_map::Entry::Vacant(vacant) => vacant.insert(File::open(path)?),
                };
                read_at(file, offset, len)
            }
        }
    }
}

/// `len` bytes of `file` from `offset`: fewer only where it ends.
fn read_at(file: &mut File, offset: u64, len: usize) -> io::Result<Vec<u8>> {
    file.seek(SeekFrom::Start(offset))?;
    let mut bytes = Vec::with_capacity(len);
    file.take(len as u64).read_to_end(&mut bytes)?;
    Ok(bytes)
}

/// What an install assembles a release's packs and files from: the current release's, and what's staged.
struct Assembler<'a, S> {
    installation: &'a Installation<S>,
    installed: &'a Installed,
    staging: &'a Staging,
    readers: PackReaders,
}

impl<S: Shipped> Assembler<'_, S> {
    /// Puts the new release's `pack` in place, unless it's shipped or installed already: assembled beside its
    /// place (`<hash>.spack.part`) from its tables and its blocks, flushed, verified whole, and renamed into it.
    fn pack(&mut self, pack: &ReleasedPack, steps: &mut impl InstallSteps) -> Result<(), InstallError> {
        let installation = self.installation;
        let Location::Patched(target) = installation.pack_location(pack) else { return Ok(()) };
        // The current release's, named by the same header hash, and checked whole when it was installed.
        let current = installation.current();
        let held =
            |held: &ReleasedPack| installation.pack_location(held) == Location::Patched(target.clone());
        if current.packs.iter().any(held) {
            return Ok(());
        }
        let io_error = |path: &Path| {
            let path = path.to_owned();
            move |source| InstallError::Io { path, source }
        };
        // A killed install's, or an earlier release's: kept if it's whole, else made again.
        if target.exists() {
            if verify_pack(&target, pack).map_err(io_error(&target))? {
                return Ok(());
            }
            steps.remove(&target).map_err(io_error(&target))?;
        }

        let (tables, toc) = self.tables(pack)?;
        let part = target.with_added_extension(PART);
        let mut file = File::create(&part).map_err(io_error(&part))?;
        steps.write_temp(&mut file, &tables).map_err(io_error(&part))?;
        let mut at = tables.len() as u64;
        for entry in toc.entries() {
            for block in toc.blocks(entry) {
                let padding = block.offset.checked_sub(at).filter(|padding| *padding < 16);
                let Some(padding) = padding else {
                    return Err(InstallError::NotWhole { pack: pack.name.clone() });
                };
                let mut bytes = vec![0; padding as usize];
                bytes.extend(self.block(&block.hash)?);
                steps.write_temp(&mut file, &bytes).map_err(io_error(&part))?;
                at = block.range().end;
            }
        }
        if at != pack.size {
            return Err(InstallError::NotWhole { pack: pack.name.clone() });
        }
        steps.sync_temp(&file).map_err(io_error(&part))?;
        drop(file);
        if !verify_pack(&part, pack).map_err(io_error(&part))? {
            return Err(InstallError::NotWhole { pack: pack.name.clone() });
        }
        steps.rename(&part, &target).map_err(io_error(&target))
    }

    /// Puts the new release's `file` in place, unless it's shipped or installed already, as [`Self::pack`] does.
    fn file(&mut self, file: &ReleasedFile, steps: &mut impl InstallSteps) -> Result<(), InstallError> {
        let installation = self.installation;
        let Location::Patched(target) = installation.file_location(file) else { return Ok(()) };
        let io_error = |path: &Path| {
            let path = path.to_owned();
            move |source| InstallError::Io { path, source }
        };
        let whole =
            |bytes: &[u8]| bytes.len() as u64 == file.size && *blake3::hash(bytes).as_bytes() == file.hash;
        match fs::read(&target) {
            Ok(bytes) if whole(&bytes) => return Ok(()),
            Ok(_) => steps.remove(&target).map_err(io_error(&target))?,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(source) => return Err(InstallError::Io { path: target, source }),
        }
        let bytes = self.file_bytes(file).filter(|bytes| whole(bytes)).ok_or_else(|| {
            InstallError::Unavailable { what: format!("{} ({} bytes)", file.path, file.size) }
        })?;
        let part = target.with_added_extension(PART);
        let mut written = File::create(&part).map_err(io_error(&part))?;
        steps.write_temp(&mut written, &bytes).map_err(io_error(&part))?;
        steps.sync_temp(&written).map_err(io_error(&part))?;
        drop(written);
        if !fs::read(&part).is_ok_and(|bytes| whole(&bytes)) {
            return Err(InstallError::Unavailable { what: file.path.clone() });
        }
        steps.rename(&part, &target).map_err(io_error(&target))
    }

    /// A pack's tables, the bytes before its data: from a current pack of the same header hash, or staged.
    fn tables(&mut self, pack: &ReleasedPack) -> Result<(Vec<u8>, Toc), InstallError> {
        let installation = self.installation;
        let current = installation.current();
        let same =
            current.packs.iter().find(|held| held.header_hash == pack.header_hash && held.size == pack.size);
        if let Some(held) = same {
            let location = installation.pack_location(held);
            let mut read = |len: usize| self.readers.read_at(installation, &location, held, 0, len).ok();
            let data_offset = read(HEADER_LEN)
                .and_then(|header| usize::try_from(PackHeader::read(&header).ok()?.data_offset).ok());
            if let Some(tables) = data_offset.and_then(&mut read)
                && let Some(toc) = table(pack, &tables)
            {
                return Ok((tables, toc));
            }
        }
        let path = self.staging.table_path(&pack.header_hash);
        let staged =
            self.staging.read_checked(&path, |bytes| table(pack, bytes).map(|toc| (bytes.to_vec(), toc)))?;
        staged.ok_or_else(|| InstallError::Unavailable { what: format!("the tables of pack {}", pack.name) })
    }

    /// A block's stored bytes, checked against its hash: from the current release's packs, or staged.
    fn block(&mut self, hash: &[u8; 32]) -> Result<Vec<u8>, InstallError> {
        let hashes_to = |bytes: &[u8]| blake3::hash(bytes).as_bytes() == hash;
        if let Some(kept) = self.installed.block(hash) {
            let installation = self.installation;
            let current = installation.current();
            if let Some(pack) = current.pack(&kept.pack) {
                let location = installation.pack_location(pack);
                let len = (kept.range.end - kept.range.start) as usize;
                let bytes = self.readers.read_at(installation, &location, pack, kept.range.start, len);
                if let Ok(bytes) = bytes
                    && hashes_to(&bytes)
                {
                    return Ok(bytes);
                }
            }
        }
        let path = self.staging.block_path(hash);
        let staged = self.staging.read_checked(&path, |bytes| hashes_to(bytes).then(|| bytes.to_vec()))?;
        staged.ok_or_else(|| InstallError::Unavailable {
            what: format!("block {}", blake3::Hash::from_bytes(*hash).to_hex()),
        })
    }

    /// A file's bytes: from the current release, if it holds the same bytes anywhere, or staged.
    fn file_bytes(&self, file: &ReleasedFile) -> Option<Vec<u8>> {
        let installation = self.installation;
        let current = installation.current();
        if let Some(held) = current.files.iter().find(|held| held.hash == file.hash) {
            let bytes = match installation.file_location(held) {
                Location::Shipped => installation.shipped.read(&held.path).ok(),
                Location::Patched(path) => fs::read(path).ok(),
            };
            if bytes.is_some() {
                return bytes;
            }
        }
        fs::read(self.staging.file_path(&file.hash)).ok()
    }
}

/// Whether the file at `path` is `pack` whole: its length and header hash, its table of contents, and every
/// block's stored bytes by their hash. Every byte a reader uses, so a pack that passes reads as the release
/// built it.
fn verify_pack(path: &Path, pack: &ReleasedPack) -> io::Result<bool> {
    let mut file = File::open(path)?;
    if file.metadata()?.len() != pack.size {
        return Ok(false);
    }
    let header = read_at(&mut file, 0, HEADER_LEN)?;
    let Some(header) = PackHeader::read(&header).ok().filter(|header| header.hash() == pack.header_hash)
    else {
        return Ok(false);
    };
    let toc_range = header.toc_range();
    let Ok(toc_len) = usize::try_from(toc_range.end - toc_range.start) else { return Ok(false) };
    let toc_bytes = read_at(&mut file, toc_range.start, toc_len)?;
    let Ok(toc) = Toc::read(header, &toc_bytes, pack.size) else { return Ok(false) };
    for entry in toc.entries() {
        for block in toc.blocks(entry) {
            let bytes = read_at(&mut file, block.offset, block.stored_len as usize)?;
            if blake3::hash(&bytes).as_bytes() != &block.hash {
                return Ok(false);
            }
        }
    }
    Ok(true)
}
