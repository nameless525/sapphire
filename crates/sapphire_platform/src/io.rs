//! The IO thread (spec §4.3, §5.1): one thread serving positioned reads of content files, in
//! priority order, with cancellation and completion callbacks.
//!
//! Callers open content files with [`IoThread::open`] and queue reads with [`IoThread::read`].
//! The thread takes the most urgent queued read ([`IoPriority`], oldest first within a priority),
//! reads it, and calls its callback with the bytes, on the IO thread. Callbacks should be quick:
//! the asset server's starts a decode job on the job system and returns, so decompression runs on
//! workers, never here. A queued read can be cancelled; its callback still runs, with
//! [`ReadError::Cancelled`], so whoever queued it always hears back.
//!
//! Files are read with positioned reads where content is a plain file (desktop and iOS, whose
//! content is in the app bundle): `read_at` on Unix, `seek_read` on Windows, so reads never
//! share a file position. Android's packaged content is inside the APK, which only SDL's asset
//! streams read, so there a file is an SDL stream that seeks then reads; one IO thread serializes
//! those reads anyway. Packs stored uncompressed in the APK (Gradle's `noCompress`) seek in
//! constant time.

use std::cmp::Ordering;
use std::collections::BinaryHeap;
use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::thread::JoinHandle;

use crate::LOG_TARGET;
use crate::content::{ContentError, ContentFile, ContentFs};

/// How urgently a read is wanted. Queued reads are served most urgent first, oldest first within
/// a priority.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum IoPriority {
    /// What the next frame needs.
    Critical,
    High,
    #[default]
    Normal,
    /// Speculative reads, served when nothing else waits.
    Prefetch,
}

/// A file opened on an [`IoThread`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct IoFile(u32);

impl IoFile {
    /// The file's number, for code that keeps it as plain data.
    pub fn to_raw(self) -> u32 {
        self.0
    }

    /// The file numbered `raw` by [`Self::to_raw`].
    pub fn from_raw(raw: u32) -> Self {
        Self(raw)
    }
}

/// A queued read, for [`IoThread::cancel`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct IoTicket(u64);

/// Why a read returned no bytes.
#[derive(Debug, thiserror::Error)]
pub enum ReadError {
    #[error("the read was cancelled")]
    Cancelled,
    #[error("the file was closed")]
    Closed,
    #[error(transparent)]
    Io(#[from] io::Error),
}

/// Called with a read's bytes, on the IO thread.
pub type ReadCallback = Box<dyn FnOnce(Result<Vec<u8>, ReadError>) + Send>;

/// A content file opened for positioned reads ([`ContentFs::open_reader`]).
#[derive(Debug)]
pub struct ContentReader {
    source: Source,
    len: u64,
}

#[derive(Debug)]
enum Source {
    /// A plain file, read with positioned reads.
    File(File),
    /// An SDL stream, which reads APK assets on Android.
    Stream(ContentFile),
}

impl ContentReader {
    fn file(file: File) -> io::Result<Self> {
        let len = file.metadata()?.len();
        Ok(Self { source: Source::File(file), len })
    }

    fn stream(mut stream: ContentFile) -> io::Result<Self> {
        let len = stream.len()?;
        Ok(Self { source: Source::Stream(stream), len })
    }

    /// The file's length in bytes.
    pub fn len(&self) -> u64 {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Fills `buf` from `offset`; reading past the end is an error.
    pub fn read_exact_at(&mut self, offset: u64, buf: &mut [u8]) -> io::Result<()> {
        let end = offset.checked_add(buf.len() as u64).filter(|&end| end <= self.len);
        if end.is_none() {
            return Err(past_the_end());
        }
        match &mut self.source {
            Source::File(file) => read_exact_at(file, offset, buf),
            Source::Stream(stream) => {
                stream.seek(SeekFrom::Start(offset))?;
                stream.read_exact(buf)
            }
        }
    }
}

#[cfg(unix)]
fn read_exact_at(file: &File, offset: u64, buf: &mut [u8]) -> io::Result<()> {
    std::os::unix::fs::FileExt::read_exact_at(file, buf, offset)
}

#[cfg(windows)]
fn read_exact_at(file: &File, mut offset: u64, mut buf: &mut [u8]) -> io::Result<()> {
    // `seek_read` may read less than asked, like `read`.
    while !buf.is_empty() {
        match std::os::windows::fs::FileExt::seek_read(file, buf, offset) {
            Ok(0) => return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "file ended early")),
            Ok(read) => {
                buf = &mut buf[read..];
                offset += read as u64;
            }
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            Err(error) => return Err(error),
        }
    }
    Ok(())
}

impl ContentFs {
    /// Opens a content file for positioned reads: a plain file where content is a directory
    /// (desktop, iOS), an SDL stream inside the APK (Android). Path rules are [`Self::open`]'s.
    pub fn open_reader(&self, path: &str) -> Result<ContentReader, ContentError> {
        let read_error = |source| ContentError::Read { path: path.to_owned(), source };
        if cfg!(target_os = "android") {
            return ContentReader::stream(self.open(path)?).map_err(read_error);
        }
        crate::content::validate_path(path)?;
        let file = File::open(format!("{}{path}", self.root()))
            .map_err(|error| ContentError::Open { path: path.to_owned(), message: error.to_string() })?;
        ContentReader::file(file).map_err(read_error)
    }
}

/// A queued read.
struct Request {
    priority: IoPriority,
    ticket: u64,
    file: IoFile,
    offset: u64,
    len: usize,
    done: ReadCallback,
}

impl PartialEq for Request {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other) == Ordering::Equal
    }
}

impl Eq for Request {}

impl PartialOrd for Request {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Request {
    /// The read to serve next is the greatest: most urgent, then oldest.
    fn cmp(&self, other: &Self) -> Ordering {
        other.priority.cmp(&self.priority).then(other.ticket.cmp(&self.ticket))
    }
}

/// What the IO thread and its callers share.
#[derive(Default)]
struct Shared {
    state: Mutex<State>,
    wake: Condvar,
}

#[derive(Default)]
struct State {
    queue: BinaryHeap<Request>,
    /// Open files by number; `None` once closed. Each is locked only while it is read.
    files: Vec<Option<Arc<Mutex<ContentReader>>>>,
    next_ticket: u64,
    shutdown: bool,
}

impl Shared {
    fn lock(&self) -> MutexGuard<'_, State> {
        // A panicking callback can't poison this: callbacks run without the lock held.
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// The IO thread: see the [module docs](self). Dropping it cancels queued reads (their callbacks
/// run with [`ReadError::Cancelled`]) and joins the thread after the read in progress.
pub struct IoThread {
    content: ContentFs,
    shared: Arc<Shared>,
    thread: Option<JoinHandle<()>>,
}

impl std::fmt::Debug for IoThread {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let state = self.shared.lock();
        f.debug_struct("IoThread")
            .field("content", &self.content)
            .field("queued", &state.queue.len())
            .field("files", &state.files.iter().filter(|file| file.is_some()).count())
            .finish()
    }
}

impl IoThread {
    /// Starts the thread, serving reads of files in `content`.
    pub fn start(content: ContentFs) -> io::Result<Self> {
        let shared = Arc::new(Shared::default());
        let thread = std::thread::Builder::new().name("sapphire-io".into()).spawn({
            let shared = shared.clone();
            move || serve(&shared)
        })?;
        Ok(Self { content, shared, thread: Some(thread) })
    }

    /// Opens content file `path` for reads, on the calling thread, and returns it with its
    /// length.
    pub fn open(&self, path: &str) -> Result<(IoFile, u64), ContentError> {
        let reader = self.content.open_reader(path)?;
        let len = reader.len();
        let mut state = self.shared.lock();
        let number = u32::try_from(state.files.len()).expect("fewer than 2^32 files opened");
        state.files.push(Some(Arc::new(Mutex::new(reader))));
        Ok((IoFile(number), len))
    }

    /// Closes `file`. Reads of it still queued fail with [`ReadError::Closed`]; a read in
    /// progress finishes.
    pub fn close(&self, file: IoFile) {
        if let Some(slot) = self.shared.lock().files.get_mut(file.0 as usize) {
            *slot = None;
        }
    }

    /// Queues a read of `len` bytes of `file` from `offset`; `done` gets the bytes, or the error,
    /// on the IO thread.
    pub fn read(
        &self,
        file: IoFile,
        offset: u64,
        len: usize,
        priority: IoPriority,
        done: ReadCallback,
    ) -> IoTicket {
        let mut state = self.shared.lock();
        let ticket = state.next_ticket;
        state.next_ticket += 1;
        state.queue.push(Request { priority, ticket, file, offset, len, done });
        drop(state);
        self.shared.wake.notify_one();
        IoTicket(ticket)
    }

    /// Cancels a queued read: its callback runs now, on this thread, with
    /// [`ReadError::Cancelled`]. Returns `false` if the read had already started or finished.
    pub fn cancel(&self, ticket: IoTicket) -> bool {
        let cancelled = {
            let mut state = self.shared.lock();
            let mut requests = std::mem::take(&mut state.queue).into_vec();
            let cancelled = requests
                .iter()
                .position(|request| request.ticket == ticket.0)
                .map(|index| requests.swap_remove(index));
            state.queue = requests.into();
            cancelled
        };
        // The callback runs after the lock is released, as on the IO thread.
        let found = cancelled.is_some();
        if let Some(request) = cancelled {
            call_back(request.done, Err(ReadError::Cancelled));
        }
        found
    }

    /// Reads queued and not yet started.
    pub fn queued(&self) -> usize {
        self.shared.lock().queue.len()
    }
}

impl Drop for IoThread {
    fn drop(&mut self) {
        let queued = {
            let mut state = self.shared.lock();
            state.shutdown = true;
            std::mem::take(&mut state.queue)
        };
        self.shared.wake.notify_one();
        for request in queued {
            call_back(request.done, Err(ReadError::Cancelled));
        }
        if let Some(thread) = self.thread.take()
            && thread.join().is_err()
        {
            tracing::error!(target: LOG_TARGET, "the IO thread panicked");
        }
    }
}

/// The IO thread's loop: serve the most urgent read until shut down.
fn serve(shared: &Shared) {
    loop {
        let (request, reader) = {
            let mut state = shared.lock();
            loop {
                if state.shutdown {
                    return;
                }
                if let Some(request) = state.queue.pop() {
                    let reader = state.files.get(request.file.0 as usize).cloned().flatten();
                    break (request, reader);
                }
                state = shared.wake.wait(state).unwrap_or_else(PoisonError::into_inner);
            }
        };
        let Request { offset, len, done, .. } = request;
        let result = match reader {
            None => Err(ReadError::Closed),
            Some(reader) => {
                let mut reader = reader.lock().unwrap_or_else(PoisonError::into_inner);
                // Checked before allocating, so a bad request can't ask for more than the file.
                if offset.checked_add(len as u64).is_none_or(|end| end > reader.len()) {
                    Err(ReadError::Io(past_the_end()))
                } else {
                    let mut bytes = vec![0; len];
                    reader.read_exact_at(offset, &mut bytes).map(|()| bytes).map_err(ReadError::Io)
                }
            }
        };
        call_back(done, result);
    }
}

/// Runs a read's callback, catching a panic in it, so one bad callback can't stop the thread and
/// leave every later read without its own.
fn call_back(done: ReadCallback, result: Result<Vec<u8>, ReadError>) {
    if std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| done(result))).is_err() {
        tracing::error!(target: LOG_TARGET, "an IO completion callback panicked");
    }
}

fn past_the_end() -> io::Error {
    io::Error::new(io::ErrorKind::UnexpectedEof, "read past the end of the file")
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc;
    use std::time::Duration;

    use super::*;

    /// A content root holding `data.bin` (bytes 0, 1, 2, … mod 251), removed on drop.
    struct Content {
        dir: std::path::PathBuf,
        fs: ContentFs,
    }

    impl Content {
        fn new(name: &str) -> Self {
            let dir =
                std::env::temp_dir().join(format!("sapphire_platform_io_{name}_{}", std::process::id()));
            std::fs::create_dir_all(&dir).expect("create temp content dir");
            std::fs::write(dir.join("data.bin"), Self::bytes()).expect("write temp content file");
            let fs = ContentFs::new(dir.to_string_lossy().into_owned());
            Self { dir, fs }
        }

        fn bytes() -> Vec<u8> {
            (0..10_000u32).map(|index| (index % 251) as u8).collect()
        }
    }

    impl Drop for Content {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    #[test]
    fn readers_read_at_any_offset_from_files_and_streams() {
        let content = Content::new("readers");
        let expected = Content::bytes();
        let file = content.fs.open_reader("data.bin").expect("open");
        let stream = ContentReader::stream(content.fs.open("data.bin").expect("open")).expect("stream");
        for mut reader in [file, stream] {
            assert_eq!(reader.len(), 10_000);
            for (offset, len) in [(9_000, 1_000), (0, 10), (4_321, 100), (0, 10_000), (10_000, 0)] {
                let mut buf = vec![0; len];
                reader.read_exact_at(offset as u64, &mut buf).expect("read");
                assert_eq!(buf, expected[offset..offset + len]);
            }
            let mut past = [0; 2];
            assert_eq!(
                reader.read_exact_at(9_999, &mut past).unwrap_err().kind(),
                io::ErrorKind::UnexpectedEof
            );
        }
        assert!(matches!(content.fs.open_reader("../data.bin"), Err(ContentError::InvalidPath { .. })));
        assert!(matches!(content.fs.open_reader("missing.bin"), Err(ContentError::Open { .. })));
    }

    /// Queues a read that reports its bytes, or its error's text, on `sender`.
    fn read(
        io: &IoThread,
        file: IoFile,
        offset: u64,
        len: usize,
        priority: IoPriority,
        sender: &mpsc::Sender<(u64, Result<Vec<u8>, String>)>,
    ) -> IoTicket {
        let sender = sender.clone();
        io.read(
            file,
            offset,
            len,
            priority,
            Box::new(move |result| {
                let _ = sender.send((offset, result.map_err(|error| error.to_string())));
            }),
        )
    }

    #[test]
    fn reads_complete_most_urgent_first_and_can_be_cancelled() {
        let content = Content::new("priority");
        let expected = Content::bytes();
        let io = IoThread::start(content.fs.clone()).expect("start");
        let (file, len) = io.open("data.bin").expect("open");
        assert_eq!(len, 10_000);

        // Hold the IO thread inside a callback while the rest queue up behind it.
        let (release, held) = mpsc::channel::<()>();
        let (started, wait_started) = mpsc::channel();
        io.read(
            file,
            0,
            1,
            IoPriority::Normal,
            Box::new(move |_| {
                started.send(()).expect("the test waits");
                held.recv_timeout(Duration::from_secs(10)).expect("released");
            }),
        );
        wait_started.recv_timeout(Duration::from_secs(10)).expect("the first read started");
        let (sender, results) = mpsc::channel();
        read(&io, file, 1, 4, IoPriority::Prefetch, &sender);
        read(&io, file, 2, 4, IoPriority::Normal, &sender);
        let cancelled = read(&io, file, 3, 4, IoPriority::Critical, &sender);
        read(&io, file, 4, 4, IoPriority::Critical, &sender);
        read(&io, file, 5, 4, IoPriority::Normal, &sender);
        read(&io, file, 9_999, 4, IoPriority::High, &sender);
        assert_eq!(io.queued(), 6);
        assert!(io.cancel(cancelled));
        assert!(!io.cancel(cancelled), "already gone");
        assert_eq!(results.recv().expect("cancelled at once"), (3, Err("the read was cancelled".into())));
        release.send(()).expect("the IO thread waits");

        let order: Vec<(u64, Result<Vec<u8>, String>)> =
            (0..5).map(|_| results.recv().expect("served")).collect();
        let offsets: Vec<u64> = order.iter().map(|(offset, _)| *offset).collect();
        assert_eq!(offsets, [4, 9_999, 2, 5, 1], "critical, high, normal oldest first, prefetch");
        for (offset, result) in order {
            match offset {
                9_999 => assert!(result.unwrap_err().contains("past the end")),
                _ => assert_eq!(result.unwrap(), expected[offset as usize..][..4]),
            }
        }
    }

    #[test]
    fn a_panicking_callback_leaves_the_thread_serving() {
        let content = Content::new("panic");
        let io = IoThread::start(content.fs.clone()).expect("start");
        let (file, _) = io.open("data.bin").expect("open");
        io.read(file, 0, 1, IoPriority::Normal, Box::new(|_| panic!("a callback that panics")));
        let (sender, results) = mpsc::channel();
        read(&io, file, 5, 4, IoPriority::Normal, &sender);
        // A read longer than the file fails before anything is allocated for it.
        read(&io, file, 0, usize::MAX, IoPriority::Normal, &sender);
        let served = results.recv_timeout(Duration::from_secs(10)).expect("served");
        assert_eq!(served, (5, Ok(Content::bytes()[5..9].to_vec())));
        let (_, too_long) = results.recv_timeout(Duration::from_secs(10)).expect("served");
        assert!(too_long.unwrap_err().contains("past the end"));
    }

    #[test]
    fn closed_files_and_shutdown_still_call_back() {
        let content = Content::new("shutdown");
        let io = IoThread::start(content.fs.clone()).expect("start");
        let (file, _) = io.open("data.bin").expect("open");
        let (release, held) = mpsc::channel::<()>();
        let (started, wait_started) = mpsc::channel();
        io.read(
            file,
            0,
            1,
            IoPriority::Critical,
            Box::new(move |_| {
                started.send(()).expect("the test waits");
                held.recv_timeout(Duration::from_secs(10)).expect("released");
            }),
        );
        wait_started.recv_timeout(Duration::from_secs(10)).expect("the first read started");
        let (sender, results) = mpsc::channel();
        read(&io, file, 10, 4, IoPriority::Normal, &sender);
        io.close(file);
        read(&io, file, 20, 4, IoPriority::Normal, &sender);
        release.send(()).expect("the IO thread waits");
        assert_eq!(results.recv().expect("served").1, Err("the file was closed".into()));
        assert_eq!(results.recv().expect("served").1, Err("the file was closed".into()));

        // Queued reads are cancelled when the thread stops.
        let (file, _) = io.open("data.bin").expect("open again");
        let (release, held) = mpsc::channel::<()>();
        let (started, wait_started) = mpsc::channel();
        io.read(
            file,
            0,
            1,
            IoPriority::Critical,
            Box::new(move |_| {
                started.send(()).expect("the test waits");
                let _ = held.recv_timeout(Duration::from_secs(10));
            }),
        );
        wait_started.recv_timeout(Duration::from_secs(10)).expect("the first read started");
        read(&io, file, 30, 4, IoPriority::Normal, &sender);
        let stopper = std::thread::spawn(move || drop(io));
        assert_eq!(results.recv().expect("cancelled").1, Err("the read was cancelled".into()));
        release.send(()).expect("the IO thread waits");
        stopper.join().expect("the IO thread stops");
    }
}
