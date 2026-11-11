//! Crash reports read back to function, file and line (M4 C2), offline, with the symbols a package kept:
//!
//! | Report | From | Read by |
//! |---|---|---|
//! | `<run>.dmp`, `<run>.<n>.dmp` | the desktop crash monitor | `minidump-processor`, which walks every thread's stack |
//! | `<run>.panic-<n>.txt` | the panic hook, every platform (the only report iOS has) | its frames' addresses, in the modules it lists |
//! | `<run>.native.txt` | Android's signal handler | its PC and return address, in the modules its header lists |
//! | `<run>.tombstone.pb` | `debuggerd`, through `ApplicationExitInfo` (Android 12+) | every thread's frames, by build ID |
//!
//! Each frame's module is found among the packages' by its debug ID or build ID, its `.sym` is written into
//! the store the first time, and its address is looked up there with `breakpad-symbols`, inlined calls
//! included. Paths a shipping build remapped read as this machine's through `maps`.

mod android;
mod minidump;
mod panic;
mod protobuf;

use std::fmt::Write as _;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::pin;
use std::task::{Context, Poll, Waker};

use anyhow::Context as _;
use breakpad_symbols::{FrameSymbolizer, SimpleModule, SimpleSymbolSupplier, Symbolizer};

use crate::store::{Packaged, Store};

/// A crash, read: what happened, and the threads' frames, the crashing thread's first.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Crash {
    /// What kind of report it was.
    pub kind: &'static str,
    /// What happened, a line each: the signal or exception, the panic's message.
    pub summary: Vec<String>,
    pub threads: Vec<Thread>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Thread {
    pub name: String,
    pub crashed: bool,
    pub frames: Vec<Frame>,
}

/// A frame: an address, and what the symbols say is there. Calls inlined at the address come first, the
/// innermost first, each with its own file and line, as a debugger lists them.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Frame {
    pub address: u64,
    /// The module's file name, and the address's offset in it.
    pub module: Option<(String, u64)>,
    pub inlined: Vec<Location>,
    pub location: Location,
    /// How the frame was found, where a stack walk found it (`context`, `cfi`, `frame pointer`, `scan`).
    pub found_by: Option<&'static str>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Location {
    pub function: Option<String>,
    pub file: Option<String>,
    pub line: Option<u32>,
}

/// Reads `report`, whichever kind it is, with the modules of `packaged` and the `.sym` files in `store`.
pub fn read(report: &Path, packaged: &[Packaged], store: &Store) -> anyhow::Result<Crash> {
    let bytes = std::fs::read(report).with_context(|| format!("failed to read {}", report.display()))?;
    let reader = Reader::new(packaged, store);
    let crash = if bytes.starts_with(b"MDMP") {
        minidump::read(bytes, &reader)
    } else if bytes.starts_with(panic::TITLE.as_bytes()) {
        panic::read(&String::from_utf8_lossy(&bytes), &reader)
    } else if bytes.starts_with(android::SIGNAL_RECORD_TITLE.as_bytes()) {
        android::read_signal_record(&String::from_utf8_lossy(&bytes), &reader)
    } else if report.to_string_lossy().ends_with(".tombstone.pb") {
        android::read_tombstone(&bytes, &reader)
    } else {
        anyhow::bail!("{} isn't a minidump, a panic report, a signal record or a tombstone", report.display())
    };
    crash.with_context(|| format!("in {}", report.display()))
}

/// What every reader looks frames up with.
pub(crate) struct Reader<'a> {
    packaged: &'a [Packaged],
    store: &'a Store,
    symbolizer: Symbolizer,
}

impl<'a> Reader<'a> {
    fn new(packaged: &'a [Packaged], store: &'a Store) -> Self {
        let symbolizer = Symbolizer::new(SimpleSymbolSupplier::new(vec![store.root.clone()]));
        Self { packaged, store, symbolizer }
    }

    /// The packaged module a crash names by `id` (its debug ID or build ID), with its `.sym` in the store.
    fn module(&self, id: &str) -> anyhow::Result<Option<&'a Packaged>> {
        let Some(packaged) = self.packaged.iter().find(|packaged| packaged.is(id)) else {
            return Ok(None);
        };
        self.store.ensure(packaged)?;
        Ok(Some(packaged))
    }

    /// A frame at `address`, `offset` into `module` (named by `name` in the report); `lookup` is the offset
    /// to look up, one less than `offset` for a return address, so the call, not what follows it, is named.
    fn frame(&self, address: u64, name: &str, module: Option<&Packaged>, offset: u64, lookup: u64) -> Frame {
        let mut frame = Frame { address, module: Some((leaf(name).to_owned(), offset)), ..Frame::default() };
        if let Some(packaged) = module {
            let simple =
                SimpleModule::new(&packaged.module.debug_file, breakpad_id(packaged.module.debug_id));
            let mut found = Found { instruction: lookup, ..Found::default() };
            if block_on(self.symbolizer.fill_symbol(&simple, &mut found)).is_ok() {
                frame.location = found.location;
                // Breakpad gives inlined calls outermost first.
                frame.inlined = found.inlined.into_iter().rev().collect();
            }
        }
        frame
    }
}

/// The same ID as `debugid` holds it, through the text both write.
fn breakpad_id(id: crate::DebugId) -> debugid::DebugId {
    debugid::DebugId::from_breakpad(&id.to_string()).expect("a debug ID's text reads back")
}

/// What `breakpad-symbols` finds at an address.
#[derive(Default)]
struct Found {
    instruction: u64,
    location: Location,
    inlined: Vec<Location>,
}

impl FrameSymbolizer for Found {
    fn get_instruction(&self) -> u64 {
        self.instruction
    }
    fn set_function(&mut self, name: &str, _base: u64, _parameter_size: u32) {
        self.location.function = Some(name.to_owned());
    }
    fn set_source_file(&mut self, file: &str, line: u32, _base: u64) {
        self.location.file = Some(file.to_owned());
        self.location.line = Some(line);
    }
    fn add_inline_frame(&mut self, name: &str, file: Option<&str>, line: Option<u32>) {
        self.inlined.push(Location { function: Some(name.to_owned()), file: file.map(str::to_owned), line });
    }
}

/// The last part of a path, either kind of slash.
fn leaf(path: &str) -> &str {
    path.rsplit(['/', '\\']).next().unwrap_or(path)
}

/// Runs `future` to its end on this thread. Everything `breakpad-symbols` and `minidump-processor` wait on
/// here is a local file, so nothing ever waits on another thread.
pub(crate) fn block_on<F: Future>(future: F) -> F::Output {
    let mut future = pin!(future);
    let mut context = Context::from_waker(Waker::noop());
    loop {
        if let Poll::Ready(output) = future.as_mut().poll(&mut context) {
            return output;
        }
        std::thread::yield_now();
    }
}

/// A shipped path's prefix, and the directory of this machine's it stands for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PathMap {
    pub shipped: String,
    pub local: PathBuf,
}

impl PathMap {
    /// `/sapphire=/path/to/workspace`.
    pub fn parse(text: &str) -> anyhow::Result<Self> {
        let (shipped, local) =
            text.split_once('=').with_context(|| format!("{text:?} isn't <shipped>=<local>"))?;
        Ok(Self { shipped: shipped.trim_end_matches('/').to_owned(), local: PathBuf::from(local) })
    }
}

/// `file` with a remapped prefix read as this machine's directory.
fn local_path(file: &str, maps: &[PathMap]) -> String {
    for map in maps {
        if let Some(rest) = file.strip_prefix(&map.shipped).filter(|rest| rest.starts_with(['/', '\\'])) {
            return rest[1..]
                .split(['/', '\\'])
                .fold(map.local.clone(), |path, part| path.join(part))
                .display()
                .to_string();
        }
    }
    file.to_owned()
}

/// The crash as text: what happened, then each thread's frames, the crashing thread's first.
pub fn render(crash: &Crash, maps: &[PathMap]) -> String {
    let mut out = String::new();
    // Writing to a String can't fail.
    let _ = writeln!(out, "{}", crash.kind);
    for line in &crash.summary {
        let _ = writeln!(out, "  {line}");
    }
    for thread in &crash.threads {
        let crashed = if thread.crashed { " (crashed)" } else { "" };
        let _ = writeln!(out, "\nthread {}{crashed}", thread.name);
        for (index, frame) in thread.frames.iter().enumerate() {
            let module = frame
                .module
                .as_ref()
                .map_or_else(String::new, |(name, offset)| format!("{name} + {offset:#x}"));
            let found_by = frame.found_by.map_or_else(String::new, |how| format!(" (found by {how})"));
            let _ = writeln!(out, "  #{index:<2} {:#018x} {module}{found_by}", frame.address);
            for location in frame.inlined.iter().chain([&frame.location]) {
                if location.function.is_none() && location.file.is_none() {
                    continue;
                }
                let function = location.function.as_deref().unwrap_or("??");
                let inlined = if std::ptr::eq(location, &frame.location) { "" } else { "inlined: " };
                let _ = write!(out, "      {inlined}{function}");
                if let Some(file) = &location.file {
                    let _ =
                        write!(out, "\n        at {}:{}", local_path(file, maps), location.line.unwrap_or(0));
                }
                let _ = writeln!(out);
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remapped_paths_read_as_this_machines() {
        let maps =
            [PathMap::parse("/sapphire=/work/engine").unwrap(), PathMap::parse("/cargo=/u/.cargo").unwrap()];
        let local = |path: &str| local_path(path, &maps);
        // The rest of the path takes this machine's separators.
        let under = |root: &str, parts: &[&str]| {
            parts.iter().fold(PathBuf::from(root), |path, part| path.join(part)).display().to_string()
        };
        assert_eq!(
            local("/sapphire/crates/a/src/b.rs"),
            under("/work/engine", &["crates", "a", "src", "b.rs"])
        );
        assert_eq!(
            local("/cargo/registry/src/x/y.rs"),
            under("/u/.cargo", &["registry", "src", "x", "y.rs"])
        );
        assert_eq!(local("/sapphirex/a.rs"), "/sapphirex/a.rs");
        assert_eq!(local("/rustc/abc/library/core/src/ptr.rs"), "/rustc/abc/library/core/src/ptr.rs");
        assert!(PathMap::parse("nothing").is_err());
    }

    #[test]
    fn frames_print_inlined_calls_innermost_first() {
        let crash = Crash {
            kind: "minidump",
            summary: vec!["EXC_BAD_ACCESS at 0x10".into()],
            threads: vec![Thread {
                name: "0 main".into(),
                crashed: true,
                frames: vec![Frame {
                    address: 0x1_0000_1234,
                    module: Some(("game_app".into(), 0x1234)),
                    inlined: vec![Location {
                        function: Some("core::ptr::write".into()),
                        file: Some("/rustc/x/ptr.rs".into()),
                        line: Some(9),
                    }],
                    location: Location {
                        function: Some("game::f".into()),
                        file: Some("/sapphire/game/f.rs".into()),
                        line: Some(3),
                    },
                    found_by: Some("context"),
                }],
            }],
        };
        let text = render(&crash, &[PathMap { shipped: "/sapphire".into(), local: PathBuf::from("/w") }]);
        let ptr = text.find("inlined: core::ptr::write").expect("the inlined call");
        let function = text.find("      game::f").expect("the function");
        assert!(ptr < function, "{text}");
        assert!(text.contains("game_app + 0x1234 (found by context)"), "{text}");
        assert!(
            text.contains(&format!("at {}:3", Path::new("/w").join("game").join("f.rs").display())),
            "{text}"
        );
    }
}
