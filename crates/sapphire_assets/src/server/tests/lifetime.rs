//! A server's packs close with it (review AP-001): a source can outlive the server it fed.

use std::collections::BTreeSet;
use std::fs::{self, File};
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

use super::*;

/// A read the source holds back, as an IO thread's queue does.
type HeldRead = (PackFile, u64, usize, LoadPriority, ReadDone);

/// Packs in memory, counting the files open now, and holding reads back while asked to.
#[derive(Default)]
struct CountedSource {
    inner: MemoryPackSource,
    /// The files open now. The inner source gives a closed file's number out again, so a file
    /// closed twice closes whatever file has its number by then, which shows in this count;
    /// [`Self::bad_closes`] counts closes of numbers no file has.
    open: Mutex<BTreeSet<u64>>,
    opened: Mutex<u32>,
    bad_closes: Mutex<u32>,
    hold: AtomicBool,
    held: Mutex<Vec<HeldRead>>,
}

impl CountedSource {
    fn open_files(&self) -> usize {
        self.open.lock().unwrap().len()
    }

    /// Runs the reads held back, in order, as an IO thread does after its server has gone.
    fn release(&self) {
        self.hold.store(false, Ordering::SeqCst);
        let held = std::mem::take(&mut *self.held.lock().unwrap());
        for (file, offset, len, priority, done) in held {
            self.inner.read(file, offset, len, priority, done);
        }
    }
}

impl PackSource for CountedSource {
    fn open(&self, name: &str) -> io::Result<(PackFile, u64)> {
        let (file, len) = self.inner.open(name)?;
        assert!(self.open.lock().unwrap().insert(file.0), "a file number is given out once while open");
        *self.opened.lock().unwrap() += 1;
        Ok((file, len))
    }

    fn read(&self, file: PackFile, offset: u64, len: usize, priority: LoadPriority, done: ReadDone) {
        if self.hold.load(Ordering::SeqCst) {
            self.held.lock().unwrap().push((file, offset, len, priority, done));
        } else {
            self.inner.read(file, offset, len, priority, done);
        }
    }

    fn close(&self, file: PackFile) {
        if !self.open.lock().unwrap().remove(&file.0) {
            *self.bad_closes.lock().unwrap() += 1;
        }
        self.inner.close(file);
    }
}

/// A pack of one texture, `id(9)`, every byte `fill`.
fn extra(fill: u8) -> Vec<u8> {
    write_pack(vec![packed(id(9), AssetKind::Texture, &texture_bytes(8, 8, fill))]).unwrap()
}

#[test]
fn dropping_a_server_closes_every_pack_it_holds_open() {
    let source = Arc::new(CountedSource::default());
    source.inner.insert("shared", write_pack(character()).unwrap());
    source.inner.insert("extra", extra(1));
    // Texture 1 is in `shared` too.
    source.inner.insert("zone", write_pack(textures(1)).unwrap());
    let mut server = AssetServer::new(source.clone(), unlimited());
    server.mount("shared").unwrap();
    server.mount("extra").unwrap();
    // A mount that fails closes what it opened at once.
    assert!(matches!(server.mount("zone"), Err(MountError::Duplicate { .. })));
    assert_eq!(source.open_files(), 2);
    // So do remounts: an unchanged pack's new file, a changed pack's old one.
    server.remount("extra").unwrap();
    assert_eq!(source.open_files(), 2);
    source.inner.insert("extra", extra(2));
    assert_eq!(server.remount("extra").unwrap().changed, 1);
    assert_eq!(source.open_files(), 2);

    // A second server on the same source, with a pack of its own open.
    let mut other = AssetServer::new(source.clone(), unlimited());
    other.mount("extra").unwrap();
    assert_eq!(source.open_files(), 3);

    // The first goes with reads still in flight: it closes its two files without waiting.
    source.hold.store(true, Ordering::SeqCst);
    let mesh: Handle<Mesh> = server.load(id(MESH)).unwrap();
    assert!(server.stats().reads_in_flight > 0);
    drop(server);
    assert_eq!(source.open_files(), 1, "the other server's pack stays open");
    // Its reads finish after it, failing on their closed files, and reach nothing.
    source.release();
    drop(mesh);

    // The other server reads its pack as before.
    let texture: Handle<TextureAsset> = other.load(id(9)).unwrap();
    other
        .finish(&texture, |upload| {
            assert_eq!(upload.get::<TextureAsset>().unwrap().levels[0][0], 2);
            Ok(())
        })
        .unwrap();
    drop(other);
    assert_eq!(source.open_files(), 0, "every file opened is closed");
    assert_eq!(*source.opened.lock().unwrap(), 6);
    assert_eq!(*source.bad_closes.lock().unwrap(), 0, "and none twice");
}

/// Packs in files, opened as the platform's IO thread opens them: one `File` a pack file.
#[derive(Default)]
struct FilePacks {
    root: PathBuf,
    files: Mutex<Vec<Option<File>>>,
}

impl FilePacks {
    fn open_files(&self) -> usize {
        self.files.lock().unwrap().iter().flatten().count()
    }
}

impl PackSource for FilePacks {
    fn open(&self, name: &str) -> io::Result<(PackFile, u64)> {
        let file = File::open(self.root.join(crate::pack::file_name(name)))?;
        let len = file.metadata()?.len();
        let mut files = self.files.lock().unwrap();
        files.push(Some(file));
        Ok((PackFile(files.len() as u64 - 1), len))
    }

    fn read(&self, file: PackFile, offset: u64, len: usize, _priority: LoadPriority, done: ReadDone) {
        let mut files = self.files.lock().unwrap();
        let result = match files.get_mut(file.0 as usize).and_then(Option::as_mut) {
            Some(open) => open.seek(SeekFrom::Start(offset)).and_then(|_| {
                let mut bytes = vec![0; len];
                open.read_exact(&mut bytes).map(|()| bytes)
            }),
            None => Err(io::Error::new(io::ErrorKind::NotFound, "no such open pack file")),
        };
        drop(files);
        done(result);
    }

    fn close(&self, file: PackFile) {
        // Dropping the `File` closes it.
        if let Some(open) = self.files.lock().unwrap().get_mut(file.0 as usize) {
            *open = None;
        }
    }
}

/// A scratch directory, removed on drop.
struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Self {
        let path = std::env::temp_dir().join(format!("sapphire_assets-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).unwrap();
        Self(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn a_pack_file_can_be_deleted_or_replaced_once_its_server_is_gone() {
    // Windows refuses to delete or replace a file some process holds open without sharing
    // deletion; elsewhere it succeeds either way. The open-file count is the check on every OS.
    let scratch = Scratch::new("pack_file_lifetime");
    let path = scratch.path().join(crate::pack::file_name("extra"));
    fs::write(&path, extra(1)).unwrap();
    let source = Arc::new(FilePacks { root: scratch.path().to_owned(), ..FilePacks::default() });
    let mut server = AssetServer::new(source.clone(), unlimited());
    server.mount("extra").unwrap();
    let texture: Handle<TextureAsset> = server.load(id(9)).unwrap();
    server.finish(&texture, |_| Ok(())).unwrap();
    assert_eq!(source.open_files(), 1);
    drop(server);
    assert_eq!(source.open_files(), 0, "the pack's file closed with its server");

    // Replaced, deleted and written again, then read by a new server on the same source.
    fs::write(&path, extra(2)).unwrap();
    fs::remove_file(&path).unwrap();
    fs::write(&path, extra(3)).unwrap();
    let mut server = AssetServer::new(source.clone(), unlimited());
    server.mount("extra").unwrap();
    let texture: Handle<TextureAsset> = server.load(id(9)).unwrap();
    server
        .finish(&texture, |upload| {
            assert_eq!(upload.get::<TextureAsset>().unwrap().levels[0][0], 3);
            Ok(())
        })
        .unwrap();
    drop(server);
    assert_eq!(source.open_files(), 0);
}
