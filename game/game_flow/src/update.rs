//! Patches in the game (M4 P4): which release the game starts on, and the boot's check for a newer one.
//!
//! [`Patches::open`] runs first, before anything reads the content. It opens what the app shipped and what's
//! installed over it ([`sapphire_patch::Installation`]), first installing a release a download staged whole
//! (one an earlier launch fetched and was killed before installing, or a killed install's: `patch/staged`
//! says so), and gives the content with the current release's
//! packs and files placed over the shipped ones ([`Patches::host`], `ContentFs::with_files_over`): what every
//! reader mounts and reads from then on, the release manifest a shipping mount holds packs to included.
//!
//! [`Patches::check`] starts a download from the build's host, if it has one, as an [`Update`], which the boot
//! moves on a frame at a time (`crate::boot`'s [`Step::Update`](crate::boot::Step::Update)): checking, fetching
//! into the staging, and installing on a thread of its own once it's staged whole. An install that switches
//! releases gives the boot the new content to stand on, in the same boot.
//!
//! **Nothing here stops the game.** No host, no network, a release the build refuses (older, expired, signed by
//! a key it doesn't trust, for a later app build), a download that fails or a disk without room: each is
//! logged, and the game starts on what it has. A kill mid-download leaves what was staged whole, and the next
//! launch's check fetches only the rest (`sapphire_patch`'s "Staging, and resuming").
//!
//! # Where patches live
//!
//! Under the player's directory (`sapphire_platform::user_dir`), in `patch/`: `patch/installed/` is the
//! installation's root and `patch/staging/` the download's. On iOS that's Application Support, kept out of
//! iCloud and the computer's backups (`sapphire_platform::exclude_from_backup`), as Apple asks of data an app
//! can fetch again; on Android it's internal storage (`files/patch/`), which the manifest's backup rules leave
//! out, and never `files/content/`, which SDL reads before the APK's assets. Nothing is made there until a
//! check or an install needs it, so a build with no host leaves no directory.

use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::mpsc;

use sapphire_assets::manifest::PublicKey;
use sapphire_patch::{
    Download, DownloadError, HttpTransport, InstallError, InstallOutcome, Installation, Outcome, Phase,
    Request, Retries, Roots, Timeouts,
};
use sapphire_platform::{ContentFs, Host};

use crate::LOG_TARGET;

/// Where patches live under the directory a [`PatchSetup`] names.
const INSTALLED: &str = "installed";
const STAGING: &str = "staging";
/// An empty file that says the staging holds a release whole, made once a download ends staged and removed once
/// its install ends: a launch finding it installs before anything mounts. A staging without it is a download
/// still under way, which the boot's check resumes, so a launch never assembles from half of one.
const STAGED: &str = "staged";

/// How a build patches: what it trusts, what it is and where it looks.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PatchSetup {
    /// Where patches install and stage: `<dir>/installed/` and `<dir>/staging/`.
    pub dir: PathBuf,
    /// The host's URL, `https://` (or `http://` to this machine) and ending in `/`: a release laid out as
    /// `xtask release` lays it.
    /// None, and nothing is checked.
    pub host: Option<String>,
    /// The keys the build trusts (`Installation::open`'s).
    pub root_keys: Vec<PublicKey>,
    /// The app's build number: a release that needs a later one is refused.
    pub app_build: u32,
}

/// Which release the game starts on, and the means to look for the next (see the [module docs](self)).
pub struct Patches {
    setup: PatchSetup,
    shipped: ContentFs,
    /// None if the shipped content can't be read as a release: then nothing's patched or checked.
    installation: Option<Installation<ContentFs>>,
    host: Host,
}

impl fmt::Debug for Patches {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let version = self.installation.as_ref().map(Installation::version);
        f.debug_struct("Patches")
            .field("setup", &self.setup)
            .field("version", &version)
            .finish_non_exhaustive()
    }
}

impl Patches {
    /// The content `host` shipped with and what's installed over it, as `setup` says: a release staged whole
    /// installed first. Never fails: what goes wrong is logged, and the content's what the app shipped.
    #[must_use]
    pub fn open(host: &Host, setup: PatchSetup) -> Self {
        let shipped = host.content().clone();
        let mut patches = Self { installation: None, shipped, setup, host: host.clone() };
        let root = patches.setup.dir.join(INSTALLED);
        let mut installation = match Installation::open(
            patches.shipped.clone(),
            &root,
            &patches.setup.root_keys,
        ) {
            Ok(installation) => installation,
            Err(error) => {
                tracing::warn!(target: LOG_TARGET, %error, "the content isn't a release, so it's never patched");
                return patches;
            }
        };
        let staging = patches.staging();
        let staged = patches.setup.dir.join(STAGED);
        if staged.exists() {
            let outcome = install(&mut installation, &staging, patches.setup.app_build);
            unmark(&staged);
            match outcome {
                Ok(InstallOutcome::Installed { from, to }) => {
                    tracing::info!(target: LOG_TARGET, from, to, "installed the release an earlier launch fetched");
                }
                Ok(InstallOutcome::UpToDate { .. }) => {}
                Err(error) => {
                    tracing::warn!(target: LOG_TARGET, %error, "the staged release isn't installed; the game starts on what it has");
                }
            }
        }
        patches.installation = Some(installation);
        patches.follow_current();
        patches
    }

    /// The device, with the current release's content: what the game reads and mounts.
    #[must_use]
    pub fn host(&self) -> &Host {
        &self.host
    }

    /// The current release's version, if the content is a release.
    #[must_use]
    pub fn version(&self) -> Option<u64> {
        self.installation.as_ref().map(Installation::version)
    }

    /// Starts looking for the next release on the build's host, if it has one: `None` if it has none, or the
    /// download couldn't start (logged).
    #[must_use]
    pub fn check(&self) -> Option<Update> {
        let url = self.setup.host.as_deref()?;
        let installation = self.installation.as_ref()?;
        let started = || -> Result<Update, String> {
            let transport = HttpTransport::new(url, Roots::Platform, Timeouts::default())
                .map_err(|error| error.to_string())?;
            let staging = self.staging();
            make_patch_dir(&self.setup.dir)?;
            let installed = installation.installed().map_err(|error| error.to_string())?;
            let request = Request {
                installed,
                staging: staging.clone(),
                now: sapphire_time::calendar_seconds(),
                app_build: self.setup.app_build,
                retries: Retries::default(),
            };
            let download = Download::start(request, transport).map_err(|error| error.to_string())?;
            let root = self.setup.dir.join(INSTALLED);
            // The boot's own, to install with on a thread of its own: opening again is a read and a stat a file.
            let installation = Installation::open(self.shipped.clone(), &root, &self.setup.root_keys)
                .map_err(|error| error.to_string())?;
            Ok(Update {
                state: State::Downloading(download),
                shipped: self.shipped.clone(),
                installation: Some(installation),
                staging,
                staged: self.setup.dir.join(STAGED),
                app_build: self.setup.app_build,
                received: 0,
                room_checked: false,
            })
        };
        match started() {
            Ok(update) => {
                tracing::info!(target: LOG_TARGET, host = url, version = installation.version(), "checking for an update");
                Some(update)
            }
            Err(error) => {
                tracing::warn!(target: LOG_TARGET, %error, "no update check; the game starts on what it has");
                None
            }
        }
    }

    /// Reads which release is current from the disk again, after a boot installed one: the content follows.
    pub fn reopen(&mut self) {
        let root = self.setup.dir.join(INSTALLED);
        if let Ok(installation) = Installation::open(self.shipped.clone(), &root, &self.setup.root_keys) {
            self.installation = Some(installation);
            self.follow_current();
        }
    }

    /// Says the game launched well on its release: what earlier releases and killed installs left goes
    /// (`Installation::confirm_launch`). Call it once a stood game has shown a frame.
    pub fn confirm_launch(&mut self) {
        let Some(installation) = &mut self.installation else { return };
        match installation.confirm_launch() {
            Ok(cleaned) if cleaned.removed + cleaned.left > 0 => {
                tracing::info!(target: LOG_TARGET, removed = cleaned.removed, left = cleaned.left, "earlier releases' files cleared");
            }
            Ok(_) => {}
            Err(error) => {
                tracing::warn!(target: LOG_TARGET, %error, "earlier releases' files stay until the next launch")
            }
        }
    }

    fn staging(&self) -> PathBuf {
        self.setup.dir.join(STAGING)
    }

    /// The content with the current release placed over what the app shipped.
    fn follow_current(&mut self) {
        let Some(installation) = &self.installation else { return };
        match content_of(&self.shipped, installation) {
            Ok(content) => {
                self.host = self.host.with_content(content);
                tracing::info!(
                    target: LOG_TARGET,
                    version = installation.version(),
                    patched = installation.is_patched(),
                    "the game's content"
                );
            }
            Err(error) => {
                self.host = self.host.with_content(self.shipped.clone());
                tracing::warn!(target: LOG_TARGET, %error, "the installed release can't be read; the game starts on what it shipped");
            }
        }
    }
}

/// The boot's update: a download, then its install (see the [module docs](self)).
pub struct Update {
    state: State,
    shipped: ContentFs,
    /// Taken by the installer's thread while it runs.
    installation: Option<Installation<ContentFs>>,
    staging: PathBuf,
    /// The [`STAGED`] marker's path.
    staged: PathBuf,
    app_build: u32,
    /// Bytes the download received from the host.
    received: u64,
    /// Whether the disk's room for what's left to stage has been asked once the download knows its size.
    room_checked: bool,
}

impl fmt::Debug for Update {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Update")
            .field("staging", &self.staging)
            .field("received", &self.received)
            .finish_non_exhaustive()
    }
}

enum State {
    Downloading(Download),
    Installing(mpsc::Receiver<(Installation<ContentFs>, Result<InstallOutcome, InstallError>)>),
    Done,
}

/// Where an [`Update`] is, as a frame asks.
#[derive(Clone, Debug, PartialEq)]
pub enum UpdateState {
    /// Asking the host for its release.
    Checking,
    /// Fetching what the release adds: how much of it is staged, 0 to 1.
    Downloading(f32),
    /// Assembling the release beside the current one.
    Installing,
    /// Over: with the content on the release it installed, or `None` if the game goes on with what it has
    /// (nothing newer, or what went wrong logged).
    Done(Option<ContentFs>),
}

impl Update {
    /// Moves it on as far as it goes without waiting, and says where it is. `Done` once.
    pub fn poll(&mut self) -> UpdateState {
        match &self.state {
            State::Downloading(download) => {
                let progress = download.progress();
                match progress.phase {
                    Phase::Checking => return UpdateState::Checking,
                    Phase::Reading | Phase::Fetching => {
                        self.received = progress.received;
                        if progress.phase == Phase::Fetching && !self.room_checked {
                            self.room_checked = true;
                            let left = progress.total.saturating_sub(progress.done);
                            if let Err(error) = room_for(&self.staging, left) {
                                tracing::warn!(target: LOG_TARGET, %error, "the update stops; the game starts on what it has");
                                download.cancel();
                            }
                        }
                        let fraction = if progress.total == 0 {
                            0.0
                        } else {
                            progress.done as f32 / progress.total as f32
                        };
                        return UpdateState::Downloading(fraction);
                    }
                    Phase::Stopped => {}
                }
                let State::Downloading(download) = std::mem::replace(&mut self.state, State::Done) else {
                    unreachable!("matched above");
                };
                self.downloaded(download.join())
            }
            State::Installing(installed) => match installed.try_recv() {
                Ok((installation, outcome)) => {
                    self.state = State::Done;
                    self.installed(&installation, outcome)
                }
                Err(mpsc::TryRecvError::Empty) => UpdateState::Installing,
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.state = State::Done;
                    tracing::warn!(target: LOG_TARGET, "the installer's thread panicked; the game starts on what it has");
                    UpdateState::Done(None)
                }
            },
            State::Done => UpdateState::Done(None),
        }
    }

    /// Bytes received from the host so far.
    #[must_use]
    pub const fn received(&self) -> u64 {
        self.received
    }

    /// A download's end: a release staged whole goes to the installer's thread.
    fn downloaded(&mut self, outcome: Result<Outcome, DownloadError>) -> UpdateState {
        match outcome {
            Ok(Outcome::UpToDate { version }) => {
                tracing::info!(target: LOG_TARGET, version, "the game is up to date");
                UpdateState::Done(None)
            }
            Ok(Outcome::Staged(staged)) => {
                self.received = staged.received;
                tracing::info!(
                    target: LOG_TARGET,
                    version = staged.manifest.version,
                    total = staged.total,
                    received = staged.received,
                    "an update is downloaded"
                );
                // A kill from here on leaves the next launch to install it.
                if let Err(error) = std::fs::write(&self.staged, []) {
                    tracing::warn!(target: LOG_TARGET, %error, "a kill before the install would download the update's last check again");
                }
                let mut installation = self.installation.take().expect("installed once");
                let (sender, receiver) = mpsc::channel();
                let (staging, staged, app_build) =
                    (self.staging.clone(), self.staged.clone(), self.app_build);
                let spawned = std::thread::Builder::new().name("installer".to_owned()).spawn(move || {
                    let outcome = install(&mut installation, &staging, app_build);
                    unmark(&staged);
                    // Nobody's waiting if the boot ended first.
                    let _ = sender.send((installation, outcome));
                });
                match spawned {
                    Ok(_) => {
                        self.state = State::Installing(receiver);
                        UpdateState::Installing
                    }
                    Err(error) => {
                        tracing::warn!(target: LOG_TARGET, %error, "the update installs at the next launch");
                        UpdateState::Done(None)
                    }
                }
            }
            Err(error) => {
                tracing::warn!(target: LOG_TARGET, %error, "no update; the game starts on what it has");
                UpdateState::Done(None)
            }
        }
    }

    /// An install's end: the content on the release it switched to, if it did.
    fn installed(
        &mut self,
        installation: &Installation<ContentFs>,
        outcome: Result<InstallOutcome, InstallError>,
    ) -> UpdateState {
        match outcome {
            Ok(InstallOutcome::Installed { from, to }) => match content_of(&self.shipped, installation) {
                Ok(content) => {
                    tracing::info!(target: LOG_TARGET, from, to, "the update is installed; the game stands on it");
                    UpdateState::Done(Some(content))
                }
                Err(error) => {
                    tracing::warn!(target: LOG_TARGET, %error, "the update is installed, and starts at the next launch");
                    UpdateState::Done(None)
                }
            },
            Ok(InstallOutcome::UpToDate { .. }) => UpdateState::Done(None),
            Err(error) => {
                tracing::warn!(target: LOG_TARGET, %error, "the update isn't installed; the game starts on what it has");
                UpdateState::Done(None)
            }
        }
    }
}

/// `shipped` with `installation`'s current release placed over it.
fn content_of(shipped: &ContentFs, installation: &Installation<ContentFs>) -> Result<ContentFs, String> {
    shipped.with_files_over(installation.files_over()).map_err(|error| error.to_string())
}

/// Installs what `staging` holds, at the calendar's time.
fn install(
    installation: &mut Installation<ContentFs>,
    staging: &Path,
    app_build: u32,
) -> Result<InstallOutcome, InstallError> {
    installation.install(staging, sapphire_time::calendar_seconds(), app_build)
}

/// Removes the [`STAGED`] marker: an install ended, so a failed one isn't tried again at every launch. The
/// next check finds the staging whole and installs from it again.
fn unmark(staged: &Path) {
    if let Err(error) = std::fs::remove_file(staged)
        && error.kind() != std::io::ErrorKind::NotFound
    {
        tracing::warn!(target: LOG_TARGET, %error, "the next launch tries the staged release's install again");
    }
}

/// Makes the directory patches live in, kept out of the OS's backups.
fn make_patch_dir(dir: &Path) -> Result<(), String> {
    std::fs::create_dir_all(dir).map_err(|error| format!("{}: {error}", dir.display()))?;
    sapphire_platform::exclude_from_backup(dir)
        .map_err(|error| format!("{} stays in backups: {error}", dir.display()))
}

/// Whether the disk holding `staging` has room for `bytes` more, with what an install keeps free.
fn room_for(staging: &Path, bytes: u64) -> Result<(), String> {
    let free =
        sapphire_platform::free_space(staging).map_err(|error| format!("{}: {error}", staging.display()))?;
    if bytes.saturating_add(sapphire_patch::DISK_RESERVE) > free {
        return Err(format!(
            "the update needs {bytes} bytes more on the disk, and {free} are free with {} kept for the device",
            sapphire_patch::DISK_RESERVE
        ));
    }
    Ok(())
}
