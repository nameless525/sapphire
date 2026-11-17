//! How long a cook took (M6 C12): the wall time of each phase of the run, and each asset's time to key and to
//! cook, read through [`sapphire_time::Stopwatch`], the cooker's only clock (rule R2).
//!
//! The times live in the [`CookReport`]'s [`CookTimings`] and in what `sapphire_pipeline cook --report`
//! prints ([`Slowest`]), and nowhere a cook writes: no cooked output, cache key, pack, release manifest, cook
//! audit or asset index holds one, and none of them reads [`CookReport::timings`]. So a cook's bytes stay the
//! same whatever its times (m6-plan's cooker rule 8; `cook`'s `a_cooks_times_reach_nothing_it_writes`).
//!
//! What a time holds:
//!
//! - **A phase** is wall time, so the phases add up to the run's. Its assets ran on up to four workers in
//!   [`Phase::Independent`] and [`Phase::Fonts`], and one at a time after them.
//! - **An asset's** key is reading its source and what it depends on, and hashing them; its cook is bringing
//!   its output up to date, from the cache or by cooking it. A cook includes any wait for the memory the
//!   cooks at once share (`CookMemory`), and a texture's chain is made by whichever of its parts a worker
//!   comes to first, the others taking it from that worker: an asset's time is its thread's, not its own
//!   work's alone.
//! - **A stage** is the sum of its assets' times, by the importer and cooker that made them, so stages run at
//!   once add up to more than the phases that held them.

use std::fmt;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use sapphire_time::Stopwatch;

use crate::cook::{Cook, CookReport, CookStatus};

/// How many stages and assets `cook --report` prints.
pub const SLOWEST: usize = 10;

/// A part of a cook's run, in the order a run goes through them.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Phase {
    /// Scanning the tree, reading its sidecars, planning each source's assets and removing stale outputs.
    Plan,
    /// Keying and cooking every asset that reads no other's output, on the workers.
    Independent,
    /// Reading the cooked screens, then keying and cooking the fonts that hold their characters, on the
    /// workers.
    Fonts,
    /// Meshes that share another's skeleton, then clips, one at a time.
    Clips,
    /// Move sets, materials, probe grids, navmeshes and zones, one at a time.
    Dependent,
    /// The checks across what cooked: strings, dialogues and screens.
    Checks,
    /// The cook audit (M6 K1, [`crate::audit`]).
    Audit,
    /// The asset index (M6 C2, [`crate::index`]).
    Index,
    /// Checking the names the code holds and packing ([`crate::pack`]).
    Pack,
    /// The game's documents beside the packs, and the release manifest.
    Ship,
}

impl Phase {
    /// What `cook --report` calls it.
    pub fn name(self) -> &'static str {
        match self {
            Self::Plan => "plan",
            Self::Independent => "cook what reads nothing cooked",
            Self::Fonts => "fonts",
            Self::Clips => "shared skeletons and clips",
            Self::Dependent => "move sets, materials, probes, navmeshes, zones",
            Self::Checks => "checks across tables and screens",
            Self::Audit => "audit",
            Self::Index => "index",
            Self::Pack => "pack",
            Self::Ship => "documents and manifest",
        }
    }
}

/// One asset's times, and the stage that made it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AssetTime {
    /// Its importer's stage name, as cache keys hold it (`gltf`, `image`, `zone document`).
    pub importer: &'static str,
    /// Its cooker's (`smesh`, `ktx2 (ktx2 0.5.0, ctt 0.6.1)`).
    pub cooker: &'static str,
    /// Making its key.
    pub key: Duration,
    /// Bringing its output up to date.
    pub cook: Duration,
}

impl AssetTime {
    /// Its key and its cook.
    pub fn total(&self) -> Duration {
        self.key + self.cook
    }

    /// Its stage as `cook --report` prints it: importer, then cooker, without the crates' versions.
    pub fn stage(&self) -> String {
        let short = |name: &'static str| name.split(" (").next().unwrap_or(name);
        format!("{} -> {}", short(self.importer), short(self.cooker))
    }
}

/// A cook's times: see the [module docs](self).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CookTimings {
    /// Each phase the run went through, in order, with its wall time.
    pub phases: Vec<(Phase, Duration)>,
    /// Each asset's, in the order of [`CookReport::assets`].
    pub assets: Vec<AssetTime>,
}

impl CookTimings {
    /// Ends `phase` on `watch`: its time is the watch's since it started or last ended a phase, and the next
    /// phase starts now.
    pub fn ended(&mut self, phase: Phase, watch: &mut Stopwatch) {
        self.phases.push((phase, watch.lap()));
    }

    /// The run's wall time, as far as its phases go.
    pub fn total(&self) -> Duration {
        self.phases.iter().map(|(_, time)| *time).sum()
    }
}

/// Which of an asset's times a [`AssetClocks::time`] adds to.
#[derive(Clone, Copy, Debug)]
pub(crate) enum Step {
    Key,
    Cook,
}

/// Each planned asset's key and cook times as threads add them, in nanoseconds.
pub(crate) struct AssetClocks(Vec<[AtomicU64; 2]>);

impl AssetClocks {
    /// Clocks for `assets` planned assets, at zero.
    pub(crate) fn new(assets: usize) -> Self {
        Self((0..assets).map(|_| [AtomicU64::new(0), AtomicU64::new(0)]).collect())
    }

    /// Runs `work` as the planned asset `index`'s `step`, adding its time.
    pub(crate) fn time<T>(&self, index: usize, step: Step, work: impl FnOnce() -> T) -> T {
        let watch = Stopwatch::start();
        let result = work();
        let nanos = u64::try_from(watch.elapsed().as_nanos()).unwrap_or(u64::MAX);
        self.0[index][step as usize].fetch_add(nanos, Ordering::Relaxed);
        result
    }

    /// The planned asset `index`'s key and cook times.
    pub(crate) fn read(&self, index: usize) -> (Duration, Duration) {
        let [key, cook] = &self.0[index];
        (
            Duration::from_nanos(key.load(Ordering::Relaxed)),
            Duration::from_nanos(cook.load(Ordering::Relaxed)),
        )
    }
}

/// What `cook --report` prints: the run's phases in order, then its slowest stages and assets, at most
/// `count` of each, ties in the report's order. Assets are named relative to the source and output roots.
pub struct Slowest<'a> {
    pub report: &'a CookReport,
    pub cook: Cook,
    pub source: &'a Path,
    pub out: &'a Path,
    pub count: usize,
}

impl fmt::Display for Slowest<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let Self { report, cook, source, out, count } = *self;
        let timings = &report.timings;
        let seconds = |time: Duration| format!("{:>8.3} s", time.as_secs_f64());
        writeln!(
            f,
            "{cook}: {} in all, {} asset(s)",
            seconds(timings.total()).trim_start(),
            report.assets.len()
        )?;
        writeln!(f, "  phases, wall time:")?;
        for (phase, time) in &timings.phases {
            writeln!(f, "  {}  {}", seconds(*time), phase.name())?;
        }

        // By first appearance, so equal times keep the report's order.
        let mut stages: Vec<(String, Duration, usize)> = Vec::new();
        for asset in &timings.assets {
            let stage = asset.stage();
            match stages.iter_mut().find(|(name, ..)| *name == stage) {
                Some((_, time, assets)) => {
                    *time += asset.total();
                    *assets += 1;
                }
                None => stages.push((stage, asset.total(), 1)),
            }
        }
        stages.sort_by_key(|(_, time, _)| std::cmp::Reverse(*time));
        writeln!(f, "  slowest stages, their assets' time on threads:")?;
        for (stage, time, assets) in stages.iter().take(count) {
            writeln!(f, "  {}  {stage} ({assets} asset(s))", seconds(*time))?;
        }

        let mut assets: Vec<usize> = (0..timings.assets.len().min(report.assets.len())).collect();
        assets.sort_by_key(|&index| std::cmp::Reverse(timings.assets[index].total()));
        let relative =
            |path: &Path, root: &Path| path.strip_prefix(root).unwrap_or(path).display().to_string();
        writeln!(f, "  slowest assets, key and cook:")?;
        for index in assets.into_iter().take(count) {
            let (asset, time) = (&report.assets[index], &timings.assets[index]);
            let how = match asset.status {
                CookStatus::Cooked => "cooked",
                CookStatus::Cached => "from cache",
                CookStatus::UpToDate => "up to date",
            };
            writeln!(
                f,
                "  {}  {} -> {} ({how}; key {})",
                seconds(time.total()),
                relative(&asset.source, source),
                relative(&asset.output, out),
                seconds(time.key).trim_start()
            )?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use sapphire_assets::AssetId;

    use super::*;
    use crate::cache::{CacheKey, Stage};
    use crate::cook::CookedAsset;

    fn millis(ms: u64) -> Duration {
        Duration::from_millis(ms)
    }

    /// A report of four assets with times fixed, as a cook under `/content` into `/loose` would make it.
    fn fixed_report() -> CookReport {
        let asset = |n: u8, source: &str, output: &str, status| CookedAsset {
            id: AssetId::from_bytes([n; 16]),
            source: PathBuf::from("/content").join(source),
            output: PathBuf::from("/loose").join(output),
            key: CacheKey::reach(&blake3::hash(&[n]), Stage { name: "test", version: 1 }),
            output_hash: blake3::hash(&[n]),
            status,
            name: String::new(),
            texture: None,
        };
        let time = |importer, cooker, key, cook| AssetTime {
            importer,
            cooker,
            key: millis(key),
            cook: millis(cook),
        };
        CookReport {
            assets: vec![
                asset(1, "hero/hero.vrm", "hero/hero.smesh", CookStatus::Cooked),
                asset(2, "hero/albedo.png", "hero/albedo.ktx2", CookStatus::Cooked),
                asset(3, "hero/normal.png", "hero/normal.ktx2", CookStatus::Cached),
                asset(4, "hero/hero.material.ron", "hero/hero.smat", CookStatus::UpToDate),
            ],
            timings: CookTimings {
                phases: vec![
                    (Phase::Plan, millis(120)),
                    (Phase::Independent, millis(2_500)),
                    (Phase::Dependent, millis(30)),
                    (Phase::Pack, millis(1_250)),
                ],
                assets: vec![
                    time("vrm", "smesh", 15, 1_985),
                    time("image", "ktx2 (ktx2 0.5.0, ctt 0.6.1)", 5, 700),
                    time("image", "ktx2 (ktx2 0.5.0, ctt 0.6.1)", 5, 2),
                    time("material", "smat", 1, 1),
                ],
            },
            ..CookReport::default()
        }
    }

    #[test]
    fn the_report_prints_the_phases_and_the_slowest_stages_and_assets() {
        let report = fixed_report();
        let slowest = Slowest {
            report: &report,
            cook: Cook::Desktop,
            source: Path::new("/content"),
            out: Path::new("/loose"),
            count: 2,
        };
        let expected = "\
desktop: 3.900 s in all, 4 asset(s)
  phases, wall time:
     0.120 s  plan
     2.500 s  cook what reads nothing cooked
     0.030 s  move sets, materials, probes, navmeshes, zones
     1.250 s  pack
  slowest stages, their assets' time on threads:
     2.000 s  vrm -> smesh (1 asset(s))
     0.712 s  image -> ktx2 (2 asset(s))
  slowest assets, key and cook:
     2.000 s  hero/hero.vrm -> hero/hero.smesh (cooked; key 0.015 s)
     0.705 s  hero/albedo.png -> hero/albedo.ktx2 (cooked; key 0.005 s)
";
        assert_eq!(slowest.to_string(), expected);

        // Every stage and asset when there's room, ties in the report's order.
        let all = Slowest { count: 10, ..slowest };
        let text = all.to_string();
        let stages = text.split("slowest stages").nth(1).unwrap();
        assert!(stages.contains("material -> smat (1 asset(s))"));
        let assets: Vec<&str> =
            text.split("slowest assets, key and cook:\n").nth(1).unwrap().lines().collect();
        assert_eq!(assets.len(), 4);
        assert!(assets[2].ends_with("hero/normal.png -> hero/normal.ktx2 (from cache; key 0.005 s)"));
        assert!(assets[3].ends_with("(up to date; key 0.001 s)"));
    }

    #[test]
    fn an_assets_clock_adds_each_steps_time() {
        let clocks = AssetClocks::new(2);
        let busy = |ms| std::thread::sleep(millis(ms));
        assert_eq!(clocks.time(1, Step::Key, || 7), 7, "the work's own result");
        clocks.time(1, Step::Cook, || busy(2));
        clocks.time(1, Step::Cook, || busy(2));
        let (key, cook) = clocks.read(1);
        assert!(key < cook && cook >= millis(4), "{key:?} {cook:?}");
        assert_eq!(clocks.read(0), (Duration::ZERO, Duration::ZERO), "another asset's is its own");

        let mut timings = CookTimings::default();
        let mut watch = Stopwatch::start();
        busy(1);
        timings.ended(Phase::Plan, &mut watch);
        timings.ended(Phase::Pack, &mut watch);
        let [(Phase::Plan, plan), (Phase::Pack, pack)] = timings.phases[..] else { panic!("{timings:?}") };
        assert!(plan >= millis(1), "{plan:?}");
        assert_eq!(timings.total(), plan + pack);
    }
}
