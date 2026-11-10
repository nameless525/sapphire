//! The download itself, on the patcher's thread (see the [crate docs](crate)).

use std::collections::{HashMap, VecDeque};
use std::io;
use std::ops::Range;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, PoisonError};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use sapphire_assets::manifest::{self, Manifest, ManifestError, ReleasedPack};
use sapphire_assets::pack::{HEADER_LEN, PackHeader, Toc};

use crate::plan::{Installed, Plan, PlanError, PlannedBlock};
use crate::staging::{Staging, StagingError};
use crate::transport::{Body, FetchError, Transport};
use crate::{MANIFEST_PATH, file_path, pack_path};

/// The most one range request of blocks asks for, so a failure costs at most this much again.
const MOST_PER_REQUEST: u64 = 1 << 20;
/// The most padding between two blocks that lie next to each other: blocks start 16-byte aligned
/// (`sapphire_assets::pack`'s layout).
const MOST_PADDING: u64 = 15;

/// What a download needs to know.
#[derive(Debug)]
pub struct Request {
    /// The install it patches.
    pub installed: Installed,
    /// Its staging directory ([`Staging`]).
    pub staging: PathBuf,
    /// Seconds since the Unix epoch, from the caller's clock: a release expired by then is refused.
    pub now: u64,
    /// The app's build number: a release that needs a later one is refused.
    pub app_build: u32,
    pub retries: Retries,
}

/// How often a failed request is tried, and how long it waits between tries.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Retries {
    /// Tries of one item (a manifest, a table, a file, a block) before the download stops, at least 1.
    pub attempts: u32,
    /// The wait after the first failure, doubling after each one after it.
    pub first_wait: Duration,
    /// The longest wait.
    pub longest_wait: Duration,
}

impl Default for Retries {
    /// Five tries, waiting half a second, then 1, 2 and 4: a host down for about eight seconds is waited out.
    fn default() -> Self {
        Self { attempts: 5, first_wait: Duration::from_millis(500), longest_wait: Duration::from_secs(8) }
    }
}

impl Retries {
    /// The wait after an item's `failures`-th failure.
    fn wait(&self, failures: u32) -> Duration {
        let doublings = failures.saturating_sub(1).min(16);
        self.first_wait.saturating_mul(1 << doublings).min(self.longest_wait)
    }
}

/// Where a download is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum Phase {
    /// Fetching and checking the host's manifest.
    Checking = 0,
    /// Fetching the changed packs' tables.
    Reading = 1,
    /// Fetching blocks and files.
    Fetching = 2,
    /// Done, failed or cancelled: [`Download::join`] says which.
    Stopped = 3,
}

impl Phase {
    fn from_u8(value: u8) -> Self {
        match value {
            0 => Self::Checking,
            1 => Self::Reading,
            2 => Self::Fetching,
            _ => Self::Stopped,
        }
    }
}

/// A download's progress, as [`Download::progress`] reads it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Progress {
    pub phase: Phase,
    /// Bytes checked and staged, those staged by an earlier run included. At most `total`.
    pub done: u64,
    /// Bytes the download moves in all: the manifest, the tables, the blocks and the files. It grows while
    /// the tables are read, and holds from [`Phase::Fetching`].
    pub total: u64,
    /// Bytes received from the host by this run, those it refused or couldn't use included.
    pub received: u64,
}

/// How a download ended well.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// The host's release is the installed one: nothing to fetch, and nothing left staged.
    UpToDate { version: u64 },
    /// The host's release is staged whole, for an installer.
    Staged(Staged),
}

/// A release staged whole.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Staged {
    /// The release, as the installed one accepted it.
    pub manifest: Manifest,
    /// Bytes the download moved in all: [`Progress::total`].
    pub total: u64,
    /// Bytes this run received from the host.
    pub received: u64,
}

/// Why a download stopped short.
#[derive(Debug, thiserror::Error)]
pub enum DownloadError {
    #[error(transparent)]
    Staging(#[from] StagingError),
    #[error("the host's release is refused: {0}")]
    Refused(#[from] ManifestError),
    #[error(transparent)]
    NotPatchable(#[from] PlanError),
    #[error("gave up on {path} after {attempts} tries: {last}")]
    Host { path: String, attempts: u32, last: FetchError },
    #[error("the download was cancelled")]
    Cancelled,
    #[error("the patcher's thread panicked")]
    Panicked,
}

/// A download running on its own thread (see the [crate docs](crate)).
#[derive(Debug)]
pub struct Download {
    shared: Arc<Shared>,
    thread: Option<JoinHandle<Result<Outcome, DownloadError>>>,
}

impl Download {
    /// Starts `request` on a thread of its own, named `patcher`, fetching through `transport`.
    pub fn start(request: Request, transport: impl Transport + 'static) -> io::Result<Self> {
        let shared = Arc::new(Shared::default());
        let mut patcher = Patcher { shared: shared.clone(), transport, retries: request.retries };
        let thread = thread::Builder::new().name("patcher".to_owned()).spawn(move || {
            let outcome = patcher.run(request);
            match &outcome {
                Ok(outcome) => tracing::info!(target: "sapphire::patch", ?outcome, "download done"),
                Err(error) => tracing::warn!(target: "sapphire::patch", %error, "download stopped"),
            }
            patcher.shared.phase.store(Phase::Stopped as u8, Ordering::Release);
            outcome
        })?;
        Ok(Self { shared, thread: Some(thread) })
    }

    /// Where it is, without blocking: atomics only.
    pub fn progress(&self) -> Progress {
        let shared = &self.shared;
        Progress {
            phase: Phase::from_u8(shared.phase.load(Ordering::Acquire)),
            done: shared.done.load(Ordering::Relaxed),
            total: shared.total.load(Ordering::Relaxed),
            received: shared.received.load(Ordering::Relaxed),
        }
    }

    /// Asks it to stop, without waiting: at its next read, or when a stalled read times out. What it staged
    /// stays for the next download.
    pub fn cancel(&self) {
        self.shared.cancel();
    }

    /// Whether it has stopped, so [`Self::join`] won't block.
    pub fn is_finished(&self) -> bool {
        self.thread.as_ref().is_none_or(JoinHandle::is_finished)
    }

    /// Waits for it to stop, and says how.
    pub fn join(mut self) -> Result<Outcome, DownloadError> {
        let thread = self.thread.take().expect("joined once, since join takes the download");
        thread.join().unwrap_or(Err(DownloadError::Panicked))
    }
}

impl Drop for Download {
    /// Cancels it without waiting, so a screen that drops it doesn't stall. Its thread lets go of the staging
    /// directory when it stops; a download started before then finds it [`StagingError::Busy`].
    fn drop(&mut self) {
        self.shared.cancel();
    }
}

/// What the download's thread and its handle share.
#[derive(Debug, Default)]
struct Shared {
    phase: AtomicU8,
    done: AtomicU64,
    total: AtomicU64,
    received: AtomicU64,
    cancelled: AtomicBool,
    /// What a wait between tries sleeps on, so a cancel ends it.
    wake: (Mutex<()>, Condvar),
}

impl Shared {
    fn cancel(&self) {
        let _guard = self.wake.0.lock().unwrap_or_else(PoisonError::into_inner);
        self.cancelled.store(true, Ordering::Relaxed);
        self.wake.1.notify_all();
    }

    fn check_cancel(&self) -> Result<(), DownloadError> {
        if self.cancelled.load(Ordering::Relaxed) { Err(DownloadError::Cancelled) } else { Ok(()) }
    }

    /// Sleeps for `wait`, or until cancelled.
    fn wait(&self, wait: Duration) -> Result<(), DownloadError> {
        let guard = self.wake.0.lock().unwrap_or_else(PoisonError::into_inner);
        let _guard = self
            .wake
            .1
            .wait_timeout_while(guard, wait, |()| !self.cancelled.load(Ordering::Relaxed))
            .unwrap_or_else(PoisonError::into_inner);
        self.check_cancel()
    }

    fn set_phase(&self, phase: Phase) {
        self.phase.store(phase as u8, Ordering::Release);
    }

    /// Counts `len` bytes both done and in the total: what's known only as it arrives.
    fn found(&self, len: u64) {
        self.total.fetch_add(len, Ordering::Relaxed);
        self.done.fetch_add(len, Ordering::Relaxed);
    }

    fn done(&self, len: u64) {
        self.done.fetch_add(len, Ordering::Relaxed);
    }
}

struct Patcher<T> {
    shared: Arc<Shared>,
    transport: T,
    retries: Retries,
}

impl<T: Transport> Patcher<T> {
    fn run(&mut self, request: Request) -> Result<Outcome, DownloadError> {
        let staging = Staging::open(&request.staging)?;
        let installed = &request.installed;
        let current = installed.manifest();

        self.shared.set_phase(Phase::Checking);
        let (signed, ()) = self.fetch(MANIFEST_PATH, None, manifest::MAX_LEN, |_| Some(()))?;
        let next = match current.accept_successor(&signed, request.now, request.app_build) {
            Ok(next) => next,
            Err(ManifestError::NotNewer { installed, offered }) if offered == installed => {
                staging.clear()?;
                return Ok(Outcome::UpToDate { version: installed });
            }
            Err(error) => return Err(error.into()),
        };
        let (from, to) = (current.version, next.version);
        tracing::info!(target: "sapphire::patch", from, to, "fetching a release");
        let plan = Plan::new(installed, next)?;
        staging.set_manifest(&signed)?;
        self.shared.found(signed.len() as u64);

        self.shared.set_phase(Phase::Reading);
        let mut tables = Vec::with_capacity(plan.packs().len());
        for pack in plan.packs() {
            let path = staging.table_path(&pack.header_hash);
            let toc = match staging.read_checked(&path, |bytes| table(pack, bytes))? {
                Some(toc) => toc,
                None => {
                    let (bytes, toc) = self.fetch_table(pack)?;
                    staging.write(&path, &bytes)?;
                    toc
                }
            };
            self.shared.found(toc.header().data_offset);
            tables.push(toc);
        }

        let blocks = plan.blocks(installed, &tables);
        staging.keep_only(
            &plan.packs().iter().map(|pack| pack.header_hash).collect(),
            &blocks.iter().map(|block| block.hash).collect(),
            &plan.files().iter().map(|file| file.hash).collect(),
        )?;
        let shared = &self.shared;
        shared.total.fetch_add(
            blocks.iter().map(PlannedBlock::stored_len).sum::<u64>()
                + plan.files().iter().map(|file| file.size).sum::<u64>(),
            Ordering::Relaxed,
        );
        let mut missing = Vec::new();
        for block in blocks {
            let hash = block.hash;
            if staging.read_checked(&staging.block_path(&hash), |bytes| hashes_to(bytes, &hash))?.is_some() {
                shared.done(block.stored_len());
            } else {
                missing.push(block);
            }
        }
        let mut missing_files = Vec::new();
        for file in plan.files() {
            let check = |bytes: &[u8]| {
                (bytes.len() as u64 == file.size).then_some(()).and(hashes_to(bytes, &file.hash))
            };
            if staging.read_checked(&staging.file_path(&file.hash), check)?.is_some() {
                shared.done(file.size);
            } else {
                missing_files.push(file);
            }
        }

        shared.set_phase(Phase::Fetching);
        for file in missing_files {
            let remote = file_path(&file.hash);
            let bytes = match range_of(0..file.size) {
                // A file is at most 64 MiB (`manifest::MAX_FILE_SIZE`).
                Some(range) => {
                    self.fetch(&remote, Some(range), file.size as usize, |bytes| {
                        hashes_to(bytes, &file.hash)
                    })?
                    .0
                }
                // An empty file has nothing to fetch, and a range can't ask for nothing.
                None if hashes_to(&[], &file.hash).is_some() => Vec::new(),
                None => {
                    return Err(DownloadError::Host {
                        path: remote,
                        attempts: 0,
                        last: FetchError::Mismatch,
                    });
                }
            };
            staging.write(&staging.file_path(&file.hash), &bytes)?;
            self.shared.done(file.size);
        }
        self.fetch_blocks(&staging, &plan, missing)?;

        let progress =
            (self.shared.total.load(Ordering::Relaxed), self.shared.received.load(Ordering::Relaxed));
        Ok(Outcome::Staged(Staged {
            manifest: plan.manifest().clone(),
            total: progress.0,
            received: progress.1,
        }))
    }

    /// A changed pack's header and table of contents, each checked against what the manifest names.
    fn fetch_table(&mut self, pack: &ReleasedPack) -> Result<(Vec<u8>, Toc), DownloadError> {
        let remote = pack_path(&pack.header_hash);
        let (mut bytes, header) = self.fetch(&remote, Some(0..HEADER_LEN as u64), HEADER_LEN, |bytes| {
            PackHeader::read(bytes)
                .ok()
                .filter(|header| header.hash() == pack.header_hash && header.file_len() == pack.size)
        })?;
        let toc = match range_of(header.toc_range()) {
            Some(range) => {
                let len = usize::try_from(range.end - range.start).unwrap_or(usize::MAX);
                let (toc_bytes, toc) =
                    self.fetch(&remote, Some(range), len, |toc| Toc::read(header, toc, pack.size).ok())?;
                bytes.extend_from_slice(&toc_bytes);
                toc
            }
            None => Toc::read(header, &[], pack.size).map_err(|_| DownloadError::Host {
                path: remote,
                attempts: 1,
                last: FetchError::Mismatch,
            })?,
        };
        Ok((bytes, toc))
    }

    /// Fetches `path` (`range` of it, if given), at most `limit` bytes and exactly that many for a range, and
    /// what `check` makes of them, trying again as [`Retries`] says.
    fn fetch<V>(
        &mut self,
        path: &str,
        range: Option<Range<u64>>,
        limit: usize,
        check: impl Fn(&[u8]) -> Option<V>,
    ) -> Result<(Vec<u8>, V), DownloadError> {
        let mut attempts = 0;
        loop {
            attempts += 1;
            self.shared.check_cancel()?;
            let mut body = Body::new(limit, &self.shared.cancelled, &self.shared.received);
            let result = self.transport.get(path, range.clone(), &mut body);
            let bytes = body.into_bytes();
            let error = match result {
                Err(FetchError::Cancelled) => return Err(DownloadError::Cancelled),
                Err(error) => error,
                Ok(()) if range.is_some() && bytes.len() != limit => {
                    FetchError::Truncated { expected: limit, got: bytes.len() }
                }
                Ok(()) => match check(&bytes) {
                    Some(value) => return Ok((bytes, value)),
                    None => FetchError::Mismatch,
                },
            };
            self.failed(path, attempts, error)?;
        }
    }

    /// Fetches `blocks`, in runs of neighbours, and stages each that arrives whole and right; a failed run's
    /// other blocks are asked for again.
    fn fetch_blocks(
        &mut self,
        staging: &Staging,
        plan: &Plan,
        blocks: Vec<PlannedBlock>,
    ) -> Result<(), DownloadError> {
        let mut failures: HashMap<[u8; 32], u32> = HashMap::new();
        let mut queue: VecDeque<Vec<PlannedBlock>> = runs(blocks).into();
        while let Some(run) = queue.pop_front() {
            self.shared.check_cancel()?;
            let span = run[0].range.start..run[run.len() - 1].range.end;
            let path = pack_path(&plan.packs()[run[0].pack].header_hash);
            let limit = (span.end - span.start) as usize;
            let mut body = Body::new(limit, &self.shared.cancelled, &self.shared.received);
            let result = self.transport.get(&path, Some(span.clone()), &mut body);
            let received = body.into_bytes();
            let error = match result {
                Err(FetchError::Cancelled) => return Err(DownloadError::Cancelled),
                Err(error) => Some(error),
                Ok(()) if received.len() != limit => {
                    Some(FetchError::Truncated { expected: limit, got: received.len() })
                }
                Ok(()) => None,
            };
            let mut failed = Vec::new();
            let mut last = None;
            for block in run {
                let at = (block.range.start - span.start) as usize..(block.range.end - span.start) as usize;
                let why = match received.get(at) {
                    Some(bytes) if hashes_to(bytes, &block.hash).is_some() => {
                        staging.write(&staging.block_path(&block.hash), bytes)?;
                        self.shared.done(block.stored_len());
                        continue;
                    }
                    // Its bytes came whole, and wrong.
                    Some(_) => FetchError::Mismatch,
                    None => error
                        .clone()
                        .unwrap_or(FetchError::Truncated { expected: limit, got: received.len() }),
                };
                let count = failures.entry(block.hash).or_default();
                *count += 1;
                if last.as_ref().is_none_or(|(most, _)| *count > *most) {
                    last = Some((*count, why));
                }
                failed.push(block);
            }
            if let Some((attempts, why)) = last {
                self.failed(&path, attempts, why)?;
                queue.extend(runs(failed));
            }
        }
        Ok(())
    }

    /// Stops the download if the item at `path` has failed `attempts` times or can't succeed; else waits
    /// before its next try.
    fn failed(&self, path: &str, attempts: u32, error: FetchError) -> Result<(), DownloadError> {
        if attempts >= self.retries.attempts || !error.is_retryable() {
            return Err(DownloadError::Host { path: path.to_owned(), attempts, last: error });
        }
        let wait = self.retries.wait(attempts);
        tracing::warn!(target: "sapphire::patch", path, attempts, %error, ?wait, "trying a failed request");
        self.shared.wait(wait)
    }
}

/// `Some(())` if `bytes` hash to `hash`.
fn hashes_to(bytes: &[u8], hash: &[u8; 32]) -> Option<()> {
    (blake3::hash(bytes).as_bytes() == hash).then_some(())
}

/// A non-empty range, as a request asks for one.
fn range_of(range: Range<u64>) -> Option<Range<u64>> {
    (range.start < range.end).then_some(range)
}

/// A staged table's table of contents, if it's the one `pack` names: its header, by hash, size and data
/// offset, then its table of contents by the header's hash of it.
pub(crate) fn table(pack: &ReleasedPack, bytes: &[u8]) -> Option<Toc> {
    let header = PackHeader::read(bytes).ok()?;
    if header.hash() != pack.header_hash
        || header.file_len() != pack.size
        || bytes.len() as u64 != header.data_offset
    {
        return None;
    }
    Toc::read(header, &bytes[HEADER_LEN..], pack.size).ok()
}

/// `blocks`, in order, as range requests: neighbours in one pack share one, up to [`MOST_PER_REQUEST`].
fn runs(blocks: Vec<PlannedBlock>) -> Vec<Vec<PlannedBlock>> {
    let mut runs: Vec<Vec<PlannedBlock>> = Vec::new();
    for block in blocks {
        if let Some(run) = runs.last_mut() {
            let (first, last) = (&run[0], &run[run.len() - 1]);
            let next_to = last.pack == block.pack
                && block.range.start >= last.range.end
                && block.range.start - last.range.end <= MOST_PADDING;
            if next_to && block.range.end - first.range.start <= MOST_PER_REQUEST {
                run.push(block);
                continue;
            }
        }
        runs.push(vec![block]);
    }
    runs
}

#[cfg(test)]
mod tests {
    use super::*;

    fn block(pack: usize, range: Range<u64>) -> PlannedBlock {
        PlannedBlock { hash: [range.start as u8; 32], pack, range }
    }

    #[test]
    fn neighbours_in_one_pack_share_a_request_up_to_a_mebibyte() {
        let spans = |blocks: Vec<PlannedBlock>| -> Vec<Vec<(u64, u64)>> {
            let ends = |block: PlannedBlock| (block.range.start, block.range.end);
            runs(blocks).into_iter().map(|run| run.into_iter().map(ends).collect()).collect()
        };
        // Padding of up to 15 bytes joins; a block between that isn't fetched, or another pack, splits.
        assert_eq!(
            spans(vec![block(0, 0..100), block(0, 112..200), block(0, 300..400), block(1, 400..500)]),
            [vec![(0, 100), (112, 200)], vec![(300, 400)], vec![(400, 500)]]
        );
        let full: Vec<PlannedBlock> =
            (0..20).map(|index| block(0, index * 65_536..(index + 1) * 65_536)).collect();
        assert_eq!(spans(full).iter().map(Vec::len).collect::<Vec<_>>(), [16, 4]);
    }

    #[test]
    fn waits_double_up_to_the_longest() {
        let retries = Retries {
            attempts: 9,
            first_wait: Duration::from_millis(100),
            longest_wait: Duration::from_millis(500),
        };
        let waits: Vec<u128> = (1..6).map(|failures| retries.wait(failures).as_millis()).collect();
        assert_eq!(waits, [100, 200, 400, 500, 500]);
    }
}
