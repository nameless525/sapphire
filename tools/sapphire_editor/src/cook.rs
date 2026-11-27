//! Cooking from the editor (M6 T5): a save asks for a cook of the content, and the stage takes the packs it rewrites.
//!
//! A tool can't link another (R1), so the cook is the pipeline's own binary run as a process, the one `cargo xtask
//! content` builds in the `tools` profile, with `cargo xtask content`'s own arguments ([`CookPlan`]): the packs it
//! writes are the bytes that command writes. It takes the cook's lock as `cargo xtask content` does, so the two
//! never cook into one folder at once.
//!
//! [`Cooker`] decides when: a request waits [`DEBOUNCE_SECONDS`] of quiet, a newer one starting the wait again,
//! so a burst of saves is one cook; one cook runs at a time, and requests made while it runs are one more cook
//! after it, of whatever is saved by then. A cook that fails says why by the pipeline's refusal ([`CookRefusal`]):
//! the source, the field of its document where the cook knows it, and the message. The process's start is about
//! 6 ms against a no-op cook's 2.6 to 3.2 s (an Apple M5 under a load of 9), so a long-lived cooker would save
//! nothing worth its weight (`docs/m6-plan.md`, T5).

use std::fmt;
use std::fs::{self, File, TryLockError};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};

use bevy_reflect::TypePath;
use sapphire_assets::pack::pack_of_part;
use sapphire_reflect::{Decoder, VersionError, Versioned};
use sapphire_time::Stopwatch;
use serde::{Deserialize, Serialize};

use crate::cooked::{self, PackStamp};

/// How long a request waits for no newer one before its cook starts.
pub const DEBOUNCE_SECONDS: f64 = 0.3;

/// The name of the counter of the last cook's time, in milliseconds (R9).
pub const COOK_MS: &str = "editor.cook_ms";
/// The name of the counter of cooks that failed (R9).
pub const COOKS_FAILED: &str = "editor.cooks_failed";

/// Why a cook stopped, as the pipeline's `cook --refusal` writes it: `sapphire_pipeline::refusal::Refusal`, read here
/// by its name and version, since the editor can't link the pipeline.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TypePath)]
#[type_path = "sapphire::pipeline"]
#[serde(deny_unknown_fields)]
pub struct CookRefusal {
    /// The source the cook refused, if the refusal is one source's.
    pub source: Option<PathBuf>,
    /// The field of its document, as a property path, where the refusal names one.
    pub field: Option<String>,
    /// What went wrong, then what caused it, a line each.
    pub message: String,
}

impl Versioned for CookRefusal {
    /// 1: M6 T5.
    const VERSION: u32 = 1;

    fn upgrade<D: Decoder>(version: u32, _data: D) -> Result<Self, VersionError<D::Error>> {
        Err(VersionError::TooOld { found: version, oldest: Self::VERSION })
    }
}

impl CookRefusal {
    /// A refusal of no source in particular: a cook that couldn't start, or stopped with nothing written.
    #[must_use]
    pub fn general(message: impl Into<String>) -> Self {
        Self { source: None, field: None, message: message.into() }
    }

    /// Its first line, for a status line.
    #[must_use]
    pub fn headline(&self) -> &str {
        self.message.lines().next().unwrap_or_default()
    }
}

/// What a cook that finished rewrote: the packs whose files changed, by name.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Cooked {
    pub packs: Vec<String>,
    /// The `.spack` files it wrote, each part of a pack in parts on its own: what a device is sent, each by its
    /// file's stem, as `cargo xtask link push` sends them (M6 T19). A pack the cook took away has none.
    pub files: Vec<PathBuf>,
}

/// What a cook came to.
pub type CookResult = Result<Cooked, CookRefusal>;

/// A cook as `cargo xtask content --cook <cook>` runs it: where the pipeline is, and what it reads and writes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CookPlan {
    /// The pipeline's binary.
    pub pipeline: PathBuf,
    /// The content tree: `content/`.
    pub source: PathBuf,
    /// Its cooked files: `target/content/loose/<cook>/`.
    pub loose: PathBuf,
    /// Its packs: `target/content/<cook>/`, the folder the editor reads.
    pub packs: PathBuf,
    /// The cache every cook shares: `target/content/cache/`.
    pub cache: PathBuf,
    /// The code whose named assets must stay in the shared packs: the repository.
    pub code: Option<PathBuf>,
    /// The pipeline's `--cook`: `desktop` or `mobile`.
    pub target: &'static str,
    /// The editor's cook, with each zone's shapes.
    pub editor: bool,
}

impl CookPlan {
    /// The plan for the packs in `packs`, `<repository>/target/content/<cook>`, as `cargo xtask content --cook <cook>`
    /// runs it, with the pipeline at `pipeline`, or where that command builds it if none is named
    /// (`<repository>/target/tools/sapphire_pipeline`). `Err` says why there's none.
    pub fn for_cook(packs: &Path, pipeline: Option<&Path>) -> Result<Self, String> {
        let cook = packs.file_name().and_then(|name| name.to_str()).unwrap_or_default();
        let (target, editor) = match cook {
            "desktop" => ("desktop", false),
            "editor" => ("desktop", true),
            "mobile" => ("mobile", false),
            _ => return Err(format!("{} isn't a cook `cargo xtask content` makes", packs.display())),
        };
        let content = packs.parent().filter(|dir| dir.file_name().is_some_and(|name| name == "content"));
        let build = content.and_then(Path::parent);
        let root = build.and_then(Path::parent);
        let (Some(content), Some(build), Some(root)) = (content, build, root) else {
            return Err(format!("{} isn't under a repository's target/content", packs.display()));
        };
        let source = root.join("content");
        if !source.is_dir() {
            return Err(format!("{} has no content tree beside it to cook", packs.display()));
        }
        // Absolute, so the file checked before a cook is the file spawned: a bare name would be checked here and
        // looked up on `PATH` by the spawn.
        let pipeline = match pipeline {
            Some(pipeline) => std::path::absolute(pipeline)
                .map_err(|error| format!("the cooker {}: {error}", pipeline.display()))?,
            None => build.join("tools").join(format!("sapphire_pipeline{}", std::env::consts::EXE_SUFFIX)),
        };
        Ok(Self {
            pipeline,
            source,
            loose: content.join("loose").join(cook),
            packs: packs.to_owned(),
            cache: content.join("cache"),
            code: Some(root.to_owned()),
            target,
            editor,
        })
    }

    /// The cook's lock, which `cargo xtask content` holds while it cooks or reads these packs: `<packs>.lock`.
    #[must_use]
    pub fn lock(&self) -> PathBuf {
        self.packs.with_extension("lock")
    }

    /// Where the last cook's refusal is, while it stands: `<packs>.refusal.ron`.
    #[must_use]
    pub fn refusal(&self) -> PathBuf {
        self.packs.with_extension("refusal.ron")
    }

    /// Where the last cook's output is: `<packs>.cook.log`.
    #[must_use]
    pub fn log(&self) -> PathBuf {
        self.packs.with_extension("cook.log")
    }

    /// The pipeline's command line for it: `cargo xtask content`'s, with the refusal's file.
    #[must_use]
    pub fn command(&self) -> Command {
        let mut command = Command::new(&self.pipeline);
        command
            .arg("cook")
            .arg("--source")
            .arg(&self.source)
            .arg("--out")
            .arg(&self.loose)
            .arg("--packs")
            .arg(&self.packs)
            .args(["--cook", self.target])
            .arg("--cache")
            .arg(&self.cache);
        if let Some(code) = &self.code {
            command.arg("--code").arg(code);
        }
        if self.editor {
            command.arg("--editor");
        }
        command.arg("--refusal").arg(self.refusal());
        command
    }
}

/// What starts cooks: the pipeline's process ([`Pipeline`]), or a test's stand-in.
pub trait Runner: fmt::Debug {
    /// Whether a save of the file at `path` is this cook's concern: whether it's under the content it cooks.
    fn reads(&self, path: &Path) -> bool;

    /// Starts a cook. `Ok(None)` when another holds the cook's lock (`cargo xtask content`, say) and `wait` is
    /// false: ask again later. With `wait`, it waits for the lock.
    fn start(&mut self, wait: bool) -> Result<Option<Box<dyn Running>>, CookRefusal>;
}

/// A cook under way.
pub trait Running: fmt::Debug {
    /// What it came to, once it has ended; `None` while it runs, unless `wait`, which waits for its end.
    fn finish(&mut self, wait: bool) -> Option<CookResult>;
}

/// What cooks have done (R9): [`COOK_MS`] and [`COOKS_FAILED`], and the rest a panel or a test reads.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CookStats {
    /// Requests, each save that asked for one.
    pub asked: u64,
    /// Cooks started.
    pub started: u64,
    /// Cooks that ended, either way, and those that failed or couldn't start.
    pub cooks: u64,
    pub cooks_failed: u64,
    /// Cooks that landed: each rewrote the packs, whatever came after it.
    pub landed: u64,
    /// The last cook's time, from its start to its end, in milliseconds: a cook that couldn't start leaves it.
    pub cook_ms: u64,
}

impl CookStats {
    /// The counters by name.
    #[must_use]
    pub fn counters(&self) -> [(&'static str, u64); 2] {
        [(COOK_MS, self.cook_ms), (COOKS_FAILED, self.cooks_failed)]
    }
}

/// When cooks happen: see the [module docs](self).
#[derive(Debug)]
pub struct Cooker {
    runner: Box<dyn Runner>,
    /// When the newest request's quiet ends, if there's one waiting.
    due: Option<f64>,
    running: Option<(Box<dyn Running>, Stopwatch)>,
    stats: CookStats,
    last: Option<CookResult>,
}

impl Cooker {
    #[must_use]
    pub fn new(runner: Box<dyn Runner>) -> Self {
        Self { runner, due: None, running: None, stats: CookStats::default(), last: None }
    }

    /// Asks for a cook at `now`, on the editor's clock in seconds: it starts once [`DEBOUNCE_SECONDS`] pass with no
    /// newer request, and after the one running, if one is.
    pub fn ask(&mut self, now: f64) {
        self.stats.asked += 1;
        self.due = Some(now + DEBOUNCE_SECONDS);
    }

    /// Whether a save of the file at `path` is the cook's concern.
    #[must_use]
    pub fn reads(&self, path: &Path) -> bool {
        self.runner.reads(path)
    }

    /// Looks at the cook running and starts one that's due. Returns what a cook that ended since came to.
    pub fn poll(&mut self, now: f64) -> Option<CookResult> {
        let ended = self.take_ended(false);
        let unstarted = if self.running.is_none() && self.due.is_some_and(|due| now >= due) {
            self.start(false)
        } else {
            None
        };
        ended.or(unstarted)
    }

    /// Runs what's asked for now, without its wait, and waits for every cook to end: what a script does at its
    /// end, so the cooks its saves asked for land before anything is drawn. Returns what the last came to.
    pub fn settle(&mut self) -> Option<CookResult> {
        let mut last = None;
        loop {
            if self.running.is_some() {
                last = self.take_ended(true);
            } else if self.due.is_some() {
                if let Some(unstarted) = self.start(true) {
                    last = Some(unstarted);
                }
            } else {
                return last;
            }
        }
    }

    /// Whether a cook runs, or one is waiting.
    #[must_use]
    pub fn is_busy(&self) -> bool {
        self.running.is_some() || self.due.is_some()
    }

    /// Whether a cook runs.
    #[must_use]
    pub fn is_running(&self) -> bool {
        self.running.is_some()
    }

    #[must_use]
    pub const fn stats(&self) -> CookStats {
        self.stats
    }

    /// What the last cook came to.
    #[must_use]
    pub fn last(&self) -> Option<&CookResult> {
        self.last.as_ref()
    }

    /// Starts the cook asked for. Returns what it came to if it couldn't start, which counts as a cook that failed.
    fn start(&mut self, wait: bool) -> Option<CookResult> {
        match self.runner.start(wait) {
            Ok(Some(running)) => {
                self.due = None;
                self.stats.started += 1;
                self.running = Some((running, Stopwatch::start()));
                None
            }
            // The lock's held elsewhere: the request stays, and the next look tries again.
            Ok(None) => None,
            Err(refusal) => {
                self.due = None;
                self.end(Err(refusal.clone()), None);
                Some(Err(refusal))
            }
        }
    }

    fn take_ended(&mut self, wait: bool) -> Option<CookResult> {
        let (running, watch) = self.running.as_mut()?;
        let result = running.finish(wait)?;
        let ms = u64::try_from(watch.elapsed().as_millis()).unwrap_or(u64::MAX);
        self.running = None;
        self.end(result.clone(), Some(ms));
        Some(result)
    }

    /// Counts a cook's end: `ms` is how long it ran, if it ran.
    fn end(&mut self, result: CookResult, ms: Option<u64>) {
        self.stats.cooks += 1;
        if let Some(ms) = ms {
            self.stats.cook_ms = ms;
        }
        let ms = self.stats.cook_ms;
        match &result {
            Ok(cooked) => {
                self.stats.landed += 1;
                tracing::info!(
                    editor.cook_ms = ms,
                    editor.cooks_failed = self.stats.cooks_failed,
                    packs = ?cooked.packs,
                    "the editor's cook landed"
                );
            }
            Err(refusal) => {
                self.stats.cooks_failed += 1;
                tracing::warn!(
                    editor.cook_ms = ms,
                    editor.cooks_failed = self.stats.cooks_failed,
                    source = ?refusal.source,
                    field = ?refusal.field,
                    message = refusal.message,
                    "the editor's cook was refused"
                );
            }
        }
        self.last = Some(result);
    }
}

/// How long an editor closing mid-cook waits for the cook to end before it stops it.
pub const CLOSE_WAIT: std::time::Duration = std::time::Duration::from_secs(10);

/// Cooks by the pipeline's binary, as [`CookPlan`] says.
#[derive(Debug)]
pub struct Pipeline {
    plan: CookPlan,
}

impl Pipeline {
    #[must_use]
    pub const fn new(plan: CookPlan) -> Self {
        Self { plan }
    }

    #[must_use]
    pub const fn plan(&self) -> &CookPlan {
        &self.plan
    }
}

impl Runner for Pipeline {
    fn reads(&self, path: &Path) -> bool {
        let source = self.plan.source.canonicalize().unwrap_or_else(|_| self.plan.source.clone());
        path.canonicalize().is_ok_and(|path| path.starts_with(source))
    }

    fn start(&mut self, wait: bool) -> Result<Option<Box<dyn Running>>, CookRefusal> {
        let plan = &self.plan;
        let lock = File::options()
            .create(true)
            .truncate(false)
            .write(true)
            .open(plan.lock())
            .map_err(|error| CookRefusal::general(format!("{}: {error}", plan.lock().display())))?;
        if wait {
            lock.lock()
                .map_err(|error| CookRefusal::general(format!("{}: {error}", plan.lock().display())))?;
        } else {
            match lock.try_lock() {
                Ok(()) => {}
                Err(TryLockError::WouldBlock) => return Ok(None),
                Err(TryLockError::Error(error)) => {
                    return Err(CookRefusal::general(format!("{}: {error}", plan.lock().display())));
                }
            }
        }
        // The last cook's refusal goes before this one starts: a cook that ends without writing its own (a panic
        // past the per-asset catch, a kill) must never be read as the last one's.
        match fs::remove_file(plan.refusal()) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(CookRefusal::general(format!("{}: {error}", plan.refusal().display()))),
        }
        if !plan.pipeline.is_file() {
            return Err(CookRefusal::general(format!(
                "there's no cooker at {}: `cargo xtask content` builds it",
                plan.pipeline.display()
            )));
        }
        let before = cooked::pack_stamp(&plan.packs).unwrap_or_default();
        let log = File::create(plan.log())
            .map_err(|error| CookRefusal::general(format!("{}: {error}", plan.log().display())))?;
        let output = log.try_clone().map_err(|error| CookRefusal::general(error.to_string()))?;
        let child =
            plan.command().stdin(Stdio::null()).stdout(output).stderr(log).spawn().map_err(|error| {
                CookRefusal::general(format!("the cooker {} didn't start: {error}", plan.pipeline.display()))
            })?;
        Ok(Some(Box::new(PipelineCook { child, _lock: lock, plan: plan.clone(), before })))
    }
}

/// The pipeline's process, cooking: it holds the cook's lock until it ends.
#[derive(Debug)]
struct PipelineCook {
    child: Child,
    _lock: File,
    plan: CookPlan,
    /// The packs as they were before it started.
    before: Vec<PackStamp>,
}

impl Running for PipelineCook {
    fn finish(&mut self, wait: bool) -> Option<CookResult> {
        let status = if wait { self.child.wait().map(Some) } else { self.child.try_wait() };
        let status = match status {
            Ok(Some(status)) => status,
            Ok(None) => return None,
            Err(error) => return Some(Err(CookRefusal::general(format!("the cooker's process: {error}")))),
        };
        if status.success() {
            let after = cooked::pack_stamp(&self.plan.packs).unwrap_or_default();
            let (packs, files) = (rewritten(&self.before, &after), written(&self.before, &after));
            return Some(Ok(Cooked { packs, files }));
        }
        let refusal = fs::read_to_string(self.plan.refusal())
            .ok()
            .and_then(|text| sapphire_serialize::from_ron::<CookRefusal>(&text).ok());
        Some(Err(refusal.unwrap_or_else(|| {
            // It stopped before it could say why: what it printed last is all there is.
            let log = fs::read_to_string(self.plan.log()).unwrap_or_default();
            let tail: Vec<&str> = log.lines().rev().take(6).collect();
            let tail: Vec<&str> = tail.into_iter().rev().collect();
            CookRefusal::general(format!("the cooker stopped ({status}):\n{}", tail.join("\n")))
        })))
    }
}

impl Drop for PipelineCook {
    /// An editor that goes while it cooks waits up to [`CLOSE_WAIT`] for the cook to end (a no-op cook takes about
    /// 3 s), since one killed between its packs and its manifest leaves a cook no build without `dev` stands on.
    /// Then it takes the cook with it, and the lock with that: a cook left running would hold no lock against the
    /// next.
    fn drop(&mut self) {
        let waited = Stopwatch::start();
        while matches!(self.child.try_wait(), Ok(None)) {
            if waited.elapsed() >= CLOSE_WAIT {
                let _ = self.child.kill();
                let _ = self.child.wait();
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
    }
}

/// The packs, by name, whose files differ between two looks at a cook's folder: a pack in parts is one pack.
fn rewritten(before: &[PackStamp], after: &[PackStamp]) -> Vec<String> {
    let mut names: Vec<String> = after
        .iter()
        .filter(|stamp| !before.contains(stamp))
        .chain(before.iter().filter(|stamp| !after.iter().any(|now| now.path() == stamp.path())))
        .filter_map(|stamp| stamp.path().file_stem()?.to_str().map(|stem| pack_of_part(stem).0.to_owned()))
        .collect();
    names.sort();
    names.dedup();
    names
}

/// The pack files in `after` that weren't in `before` as they are now: each part on its own, none taken away.
fn written(before: &[PackStamp], after: &[PackStamp]) -> Vec<PathBuf> {
    after.iter().filter(|stamp| !before.contains(stamp)).map(|stamp| stamp.path().to_owned()).collect()
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::rc::Rc;

    use super::*;

    /// A cook a test ends when it likes.
    #[derive(Debug, Default)]
    struct Script {
        /// Starts so far.
        started: u32,
        /// Whether the lock is held elsewhere.
        locked: bool,
        /// What the running cook comes to, once the test says it has ended.
        end: Option<CookResult>,
        /// What the cook after it comes to.
        then: Option<CookResult>,
    }

    #[derive(Debug)]
    struct Fake(Rc<RefCell<Script>>);

    #[derive(Debug)]
    struct FakeCook(Rc<RefCell<Script>>);

    impl Runner for Fake {
        fn reads(&self, _: &Path) -> bool {
            true
        }

        fn start(&mut self, wait: bool) -> Result<Option<Box<dyn Running>>, CookRefusal> {
            let mut script = self.0.borrow_mut();
            if script.locked && !wait {
                return Ok(None);
            }
            script.started += 1;
            Ok(Some(Box::new(FakeCook(Rc::clone(&self.0)))))
        }
    }

    impl Running for FakeCook {
        fn finish(&mut self, wait: bool) -> Option<CookResult> {
            let mut script = self.0.borrow_mut();
            let ended = script.end.take();
            let ended = if wait { Some(ended.unwrap_or(Ok(Cooked::default()))) } else { ended };
            if ended.is_some() {
                script.end = script.then.take();
            }
            ended
        }
    }

    fn cooker() -> (Cooker, Rc<RefCell<Script>>) {
        let script = Rc::new(RefCell::new(Script::default()));
        (Cooker::new(Box::new(Fake(Rc::clone(&script)))), script)
    }

    fn started(script: &Rc<RefCell<Script>>) -> u32 {
        script.borrow().started
    }

    #[test]
    fn a_request_waits_for_quiet_and_a_newer_one_waits_again() {
        let (mut cooker, script) = cooker();
        cooker.ask(10.0);
        assert!(cooker.poll(10.29).is_none());
        assert_eq!(started(&script), 0, "the quiet isn't over");
        cooker.ask(10.2);
        assert!(cooker.poll(10.31).is_none());
        assert_eq!(started(&script), 0, "the newer request started the wait again");
        cooker.poll(10.5);
        assert_eq!(started(&script), 1, "one cook for both");
        assert!(cooker.is_running());
    }

    #[test]
    fn one_cook_runs_at_a_time_and_requests_meanwhile_are_one_more_after_it() {
        let (mut cooker, script) = cooker();
        cooker.ask(0.0);
        cooker.poll(0.3);
        assert_eq!(started(&script), 1);
        for at in [0.4, 0.6, 0.9, 1.5] {
            cooker.ask(at);
            assert!(cooker.poll(at + 1.0).is_none());
        }
        assert_eq!(started(&script), 1, "never two at once");
        script.borrow_mut().end = Some(Ok(Cooked { packs: vec!["shared".to_owned()], ..Cooked::default() }));
        let ended = cooker.poll(2.6).expect("the first ended");
        assert_eq!(ended, Ok(Cooked { packs: vec!["shared".to_owned()], ..Cooked::default() }));
        assert_eq!(started(&script), 2, "the newest request's cook starts as the first ends");
        script.borrow_mut().end = Some(Ok(Cooked::default()));
        cooker.poll(3.0);
        cooker.poll(9.0);
        assert_eq!(started(&script), 2, "four requests during one cook are one cook");
        assert!(!cooker.is_busy());
        let stats = cooker.stats();
        assert_eq!((stats.asked, stats.started, stats.cooks, stats.cooks_failed), (5, 2, 2, 0));
    }

    #[test]
    fn the_newest_request_wins_over_a_cook_another_holds_the_lock_for() {
        let (mut cooker, script) = cooker();
        script.borrow_mut().locked = true;
        cooker.ask(0.0);
        cooker.poll(0.5);
        assert_eq!(started(&script), 0, "the lock is held: the request waits");
        cooker.ask(0.6);
        script.borrow_mut().locked = false;
        cooker.poll(0.7);
        assert_eq!(started(&script), 0, "the newest request's wait isn't over");
        cooker.poll(0.9);
        assert_eq!(started(&script), 1);
    }

    #[test]
    fn the_counters_count_the_last_cooks_time_and_the_cooks_that_failed() {
        let (mut cooker, script) = cooker();
        cooker.ask(0.0);
        cooker.poll(0.3);
        std::thread::sleep(std::time::Duration::from_millis(15));
        let refusal = CookRefusal {
            source: Some("content/broken.material.ron".into()),
            field: Some("base_color_map".to_owned()),
            message: "failed to cook material\ntexture slot `base_color` references nothing".to_owned(),
        };
        script.borrow_mut().end = Some(Err(refusal.clone()));
        assert_eq!(cooker.poll(0.4), Some(Err(refusal.clone())));
        let stats = cooker.stats();
        assert_eq!(stats.cooks_failed, 1);
        assert!(stats.cook_ms >= 15, "the cook's own time: {}", stats.cook_ms);
        assert_eq!(stats.counters(), [(COOK_MS, stats.cook_ms), (COOKS_FAILED, 1)]);
        assert_eq!(cooker.last(), Some(&Err(refusal)));

        cooker.ask(1.0);
        assert_eq!(cooker.settle(), Some(Ok(Cooked::default())), "a script's end runs what's asked at once");
        let stats = cooker.stats();
        assert_eq!(
            (stats.cooks, stats.cooks_failed, stats.landed),
            (2, 1, 1),
            "a cook that lands fails nothing"
        );
    }

    /// A script's end settles a cook running and the one asked for after it: the first landing counts though the
    /// second fails, and a cook that couldn't start leaves the last cook's time.
    #[test]
    fn settling_counts_every_cook_that_landed_and_a_cook_that_never_ran_keeps_the_last_time() {
        let (mut cooker, script) = cooker();
        cooker.ask(0.0);
        cooker.poll(0.3);
        cooker.ask(0.4);
        // The first lands when settled (a waited cook with no scripted end lands); the second is refused.
        let refusal = CookRefusal::general("refused");
        script.borrow_mut().then = Some(Err(refusal.clone()));
        assert_eq!(cooker.settle(), Some(Err(refusal)));
        assert_eq!(started(&script), 2);
        let stats = cooker.stats();
        assert_eq!((stats.cooks, stats.landed, stats.cooks_failed), (2, 1, 1));
        let ms = 42;

        let mut cooker = Cooker::new(Box::new(NoStart));
        cooker.stats.cook_ms = ms;
        cooker.ask(0.0);
        assert!(matches!(cooker.poll(0.3), Some(Err(_))));
        assert_eq!(cooker.stats().cook_ms, ms, "a cook that couldn't start ran no time");
    }

    #[derive(Debug)]
    struct NoStart;

    impl Runner for NoStart {
        fn reads(&self, _: &Path) -> bool {
            true
        }

        fn start(&mut self, _: bool) -> Result<Option<Box<dyn Running>>, CookRefusal> {
            Err(CookRefusal::general("no cooker"))
        }
    }

    /// A cook that ends without writing a refusal is never read as the last cook's: the refusal left from before
    /// goes when the next starts, and what the cooker printed is the reason.
    #[cfg(unix)]
    #[test]
    fn a_cook_that_stops_without_its_refusal_never_reads_the_last_ones() {
        let dir = std::env::temp_dir().join(format!("sapphire_editor_stale_refusal_{}", std::process::id()));
        let packs = dir.join("target/content/editor");
        fs::create_dir_all(&packs).unwrap();
        fs::create_dir_all(dir.join("content")).unwrap();
        // A cooker that stops at once with nothing written: `false`.
        let plan = CookPlan::for_cook(&packs, Some(Path::new("/usr/bin/false"))).expect("a plan");
        let stale = CookRefusal {
            source: Some("content/fixed.material.ron".into()),
            field: Some("base_color_map".to_owned()),
            message: "the last cook's".to_owned(),
        };
        fs::write(plan.refusal(), sapphire_serialize::to_ron(&stale).unwrap()).unwrap();
        let mut cooker = Cooker::new(Box::new(Pipeline::new(plan.clone())));
        cooker.ask(0.0);
        let Some(Err(refusal)) = cooker.settle() else { panic!("the cook failed") };
        assert_ne!(refusal, stale);
        assert_eq!((refusal.source, refusal.field), (None, None), "{}", refusal.message);
        assert!(refusal.message.starts_with("the cooker stopped"), "{}", refusal.message);
        assert!(!plan.refusal().exists());
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_bare_cooker_name_is_the_file_checked_and_spawned() {
        let dir = std::env::temp_dir().join(format!("sapphire_editor_bare_{}", std::process::id()));
        fs::create_dir_all(dir.join("content")).unwrap();
        let plan =
            CookPlan::for_cook(&dir.join("target/content/editor"), Some(Path::new("sapphire_pipeline")))
                .expect("a plan");
        assert!(plan.pipeline.is_absolute(), "{}", plan.pipeline.display());
        assert_eq!(plan.pipeline, std::env::current_dir().unwrap().join("sapphire_pipeline"));
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_cook_that_cannot_start_fails_once_and_says_why() {
        let dir = std::env::temp_dir().join(format!("sapphire_editor_no_cooker_{}", std::process::id()));
        let packs = dir.join("target/content/editor");
        fs::create_dir_all(&packs).unwrap();
        fs::create_dir_all(dir.join("content")).unwrap();
        let plan = CookPlan::for_cook(&packs, Some(&dir.join("nowhere"))).expect("a plan");
        let mut cooker = Cooker::new(Box::new(Pipeline::new(plan)));
        cooker.ask(0.0);
        let Some(Err(refusal)) = cooker.poll(0.3) else { panic!("refused") };
        assert!(refusal.message.contains("no cooker"), "{}", refusal.message);
        assert_eq!(cooker.poll(0.6), None, "said once");
        assert_eq!(cooker.stats().cooks_failed, 1);
        assert!(!cooker.is_busy());
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_plan_is_cargo_xtask_contents_own_cook() {
        let dir = std::env::temp_dir().join(format!("sapphire_editor_plan_{}", std::process::id()));
        fs::create_dir_all(dir.join("content")).unwrap();
        let packs = dir.join("target/content/editor");
        let plan = CookPlan::for_cook(&packs, None).expect("a plan");
        let exe = std::env::consts::EXE_SUFFIX;
        assert_eq!(plan.pipeline, dir.join(format!("target/tools/sapphire_pipeline{exe}")));
        assert_eq!(plan.loose, dir.join("target/content/loose/editor"));
        assert_eq!(plan.cache, dir.join("target/content/cache"));
        assert_eq!(
            plan.lock(),
            dir.join("target/content/editor.lock"),
            "the lock `cargo xtask content` takes"
        );
        let command = plan.command();
        let args: Vec<_> = command.get_args().map(|arg| arg.to_string_lossy().into_owned()).collect();
        let path = |path: &Path| path.to_string_lossy().into_owned();
        assert_eq!(
            args,
            [
                "cook".to_owned(),
                "--source".to_owned(),
                path(&dir.join("content")),
                "--out".to_owned(),
                path(&plan.loose),
                "--packs".to_owned(),
                path(&packs),
                "--cook".to_owned(),
                "desktop".to_owned(),
                "--cache".to_owned(),
                path(&plan.cache),
                "--code".to_owned(),
                path(&dir),
                "--editor".to_owned(),
                "--refusal".to_owned(),
                path(&dir.join("target/content/editor.refusal.ron")),
            ]
        );
        let desktop = CookPlan::for_cook(&dir.join("target/content/desktop"), None).expect("a plan");
        assert!(!desktop.editor);
        for wrong in ["target/content/town", "target/elsewhere/editor", "editor"] {
            assert!(CookPlan::for_cook(&dir.join(wrong), None).is_err(), "{wrong}");
        }
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_refusal_reads_as_the_pipeline_writes_it() {
        // `sapphire_pipeline::refusal`'s own text for a material's missing texture.
        let text = r#"(
    type: "sapphire::pipeline::CookRefusal",
    version: 1,
    data: (
        source: Some("content/broken.material.ron"),
        field: Some("base_color_map"),
        message: "failed to cook material content/broken.material.ron\ntexture slot `base_color` references 07070707-0707-0707-0707-070707070707, which no source in the content tree has",
    ),
)"#;
        let refusal: CookRefusal = sapphire_serialize::from_ron(text).expect("reads");
        assert_eq!(refusal.field.as_deref(), Some("base_color_map"));
        assert_eq!(refusal.headline(), "failed to cook material content/broken.material.ron");
    }

    #[test]
    fn a_pack_in_parts_is_one_pack_rewritten() {
        let dir = std::env::temp_dir().join(format!("sapphire_editor_rewritten_{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        for name in ["shared.spack", "shared.1.spack", "zone_a.spack", "audio.spack"] {
            fs::write(dir.join(name), name).unwrap();
        }
        let before = cooked::pack_stamp(&dir).unwrap();
        fs::write(dir.join("shared.1.spack"), "another part").unwrap();
        fs::remove_file(dir.join("audio.spack")).unwrap();
        fs::write(dir.join("zone_b.spack"), "new").unwrap();
        let after = cooked::pack_stamp(&dir).unwrap();
        assert_eq!(rewritten(&before, &after), ["audio", "shared", "zone_b"]);
        let files = written(&before, &after);
        let names: Vec<_> = files.iter().map(|file| file.file_name().unwrap().to_str().unwrap()).collect();
        assert_eq!(names, ["shared.1.spack", "zone_b.spack"], "each part written, none taken away");
        fs::remove_dir_all(dir).unwrap();
    }
}
