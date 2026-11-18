//! The batch's manifest, `bench/scale.ron` (M5 Z1): every run the batch makes, as data, and the plan it makes of
//! them: one unit for each run, tier, variant and round, packed into sittings no longer than a whole-machine hold.
//! The document's own comments say what each field means to someone adding a run.

use std::collections::{BTreeMap, BTreeSet};

use bevy_reflect::TypePath;
use sapphire_reflect::Versioned;
use serde::Deserialize;

/// The manifest as written.
#[derive(Debug, Deserialize, TypePath)]
#[type_path = "sapphire::testkit"]
#[serde(deny_unknown_fields)]
pub struct ScaleBench {
    /// The benchmark preset the district's runs mount (`cargo xtask bench-content --preset`).
    pub preset: String,
    /// Rounds of each run, unless the run says otherwise.
    pub rounds: u32,
    /// The longest a sitting may plan, in seconds, by its units' stated lengths: one whole-machine hold.
    pub sitting_seconds: u32,
    /// The machine's one-minute load above which a timed reading is marked as read above it.
    pub load_bound: f32,
    /// What the batch can't measure yet, each said once at the top of the report: a run that closes one takes
    /// its line out.
    #[serde(default)]
    pub gaps: Vec<String>,
    /// Where the batch measures an exit test otherwise than its words say, by a ruling: each said in a line of
    /// its own beside the verdicts.
    #[serde(default)]
    pub deviations: Vec<String>,
    /// The tiers, in the order the batch runs them.
    pub tiers: Vec<Tier>,
    /// The runs, in the order each tier's sittings run them.
    pub runs: Vec<Run>,
    /// The devices a run may name to walk there in place of this Mac (M5 Z2).
    #[serde(default)]
    pub devices: Vec<Device>,
}

/// A device beside this Mac that a run walks on (M5 Z2): the iOS Simulator or the Android emulator, which
/// `cargo xtask bench-scale device --platform <name>` boots, installs a bundle on, walks and shuts down.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Device {
    /// `ios-sim` or `android`, as `bench-scale device --platform` takes it: its units' and logs' part.
    pub name: String,
    /// What it is, in the report's words.
    pub about: String,
    /// The benchmark preset whose district its bundle carries, in place of the manifest's: a phone's bundle
    /// holds the district whole, so the `budget` one's gigabytes don't go on a device.
    pub preset: String,
}

impl Versioned for ScaleBench {
    /// 1: M5 Z1.
    const VERSION: u32 = 1;
}

/// A tier, as the batch runs it.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Tier {
    /// The sandbox's `--tier`.
    pub name: String,
    /// The sandbox's `--size`, unless a run gives its own.
    pub size: String,
    pub cook: Cook,
    /// Frames of a whole lap of the district's tour at this tier's steps a frame: what `Frames::Lap` means.
    pub lap: u32,
    /// What the bench draws a second on this machine at this tier, for planning only: frames over this give a
    /// unit's stated length.
    pub frames_a_second: u32,
    /// Seconds a unit takes before its first frame: the load and the warm-up.
    pub startup_seconds: u32,
    /// Spec §3.1's memory budget for the tier, in MiB (its RAM on a desktop, the whole on a phone): what exit test
    /// 2 holds the process's most to. None judges nothing.
    #[serde(default)]
    pub memory_mib: Option<u32>,
    /// The ambient characters exit test 6 asks the district to hold on the tier (m5-plan §1: 60 on a desktop, 30
    /// on a phone). None judges nothing.
    #[serde(default)]
    pub crowd: Option<u32>,
}

/// Which cook a tier mounts.
#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum Cook {
    Desktop,
    Mobile,
}

impl Cook {
    pub fn name(self) -> &'static str {
        match self {
            Self::Desktop => "desktop",
            Self::Mobile => "mobile",
        }
    }
}

/// How many frames a run's bench measures in each phase.
#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
pub enum Frames {
    /// A lap of the tour at the tier's steps a frame ([`Tier::lap`]).
    Lap,
    /// This many.
    Count(u32),
}

/// What a run's logs are read for: which of the report's tables it fills.
#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
pub enum Read {
    /// §3.2's rows, from the bench's `cpu` phase.
    Cpu,
    /// §3.3's rows, from its `gpu` phase.
    Gpu,
    /// What a frame submits, against §3.4.
    Counts,
    /// The process's memory and the pools through the walk, against §3.1, and the load's time.
    Memory,
    /// The texture streamer and admission, exit test 2.
    Streaming,
    /// A zone trip's steps and the mesh pool's floor after it.
    Trips,
    /// The characters' rows, exit test 5.
    Cast,
    /// What the load built and what its frames built: exit test 7, by counts.
    Pipelines,
}

/// One variant of a pair: its name and the arguments it adds.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Variant {
    pub name: String,
    pub args: Vec<String>,
    /// Another cook of the same content to mount, beside the tier's: its folder is the tier's cook's with
    /// `-<content>` (`desktop-no-cell-levels`, M5 Z2), which `cargo xtask bench-scale prepare` makes. `None`
    /// mounts the tier's.
    #[serde(default)]
    pub content: Option<String>,
}

impl Variant {
    /// The content folder this variant mounts, given the tier's.
    pub fn content_dir(&self, tier: &std::path::Path) -> std::path::PathBuf {
        match &self.content {
            None => tier.to_owned(),
            Some(beside) => {
                let name =
                    tier.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default();
                tier.with_file_name(format!("{name}-{beside}"))
            }
        }
    }
}

/// One run of the batch.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Run {
    /// Its name, lowercase letters, digits and hyphens: its logs' stem.
    pub name: String,
    /// What it measures, in the report's words.
    pub about: String,
    /// The sandbox's arguments, after `--tier` and `--size`. `{replays}` is the batch's prepared replays'
    /// folder.
    pub args: Vec<String>,
    /// Arguments one tier adds, by its name.
    #[serde(default)]
    pub tier_args: BTreeMap<String, Vec<String>>,
    pub frames: Frames,
    /// The bench's phases of `frames`: 3 for a walk or a fight (cold, CPU, GPU), 1 for a plain bench.
    #[serde(default = "three")]
    pub phases: u32,
    /// The tiers it runs on: every tier if empty.
    #[serde(default)]
    pub tiers: Vec<String>,
    /// Its own `--size`, in place of the tier's (a package's script that drew at another size).
    #[serde(default)]
    pub size: Option<String>,
    pub read: Vec<Read>,
    /// A pair's (or more) variants, whose rounds alternate their order; none for a run of one.
    #[serde(default)]
    pub variants: Vec<Variant>,
    /// Whether the unit writes every frame to `<unit>.csv` (`--bench-trace`): the 99th percentiles the bench's
    /// rows don't log, and a trip's last frames.
    #[serde(default)]
    pub trace: bool,
    /// The rows a pair's table shows, each a bench row, a count row or a plain bench's field.
    #[serde(default)]
    pub rows: Vec<String>,
    /// Its rounds, in place of the manifest's.
    #[serde(default)]
    pub rounds: Option<u32>,
    /// Whether it's timed, and so planned into sittings. A run of counts alone (`--draw-every`) isn't: it runs
    /// in an ordinary slot (`run --counts`), and its rounds must count the same, unless it's paced
    /// ([`Run::paced`]): a walk at a frame's time is timed work, planned into sittings, though read as counts.
    #[serde(default = "yes")]
    pub timed: bool,
    /// The baseline's run it stands beside, by name, read from the baseline's folder for the same tier.
    #[serde(default)]
    pub beside: Option<String>,
    /// The exit tests (m5-plan §1) it is read for.
    #[serde(default)]
    pub serves: Vec<u32>,
    /// What it stands in for, where the exit test's own run can't be made yet: the report prints its rows as
    /// "not measured as written", never as a pass.
    #[serde(default)]
    pub stands_in: Option<String>,
    /// The devices it walks on, by name, in place of this Mac (M5 Z2): such a run is of counts alone, has no
    /// variants, and only `bench-scale device` runs it, its units apart from the batch's.
    #[serde(default)]
    pub devices: Vec<String>,
}

fn three() -> u32 {
    3
}

fn yes() -> bool {
    true
}

/// One sandbox run of the batch: a run on a tier, one variant, one round, on this Mac or a device.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Unit {
    /// `<run>.<tier>[.<variant>][.<device>].r<round>`: its logs' stem.
    pub name: String,
    pub run: usize,
    pub tier: usize,
    pub variant: Option<usize>,
    /// The device it walks on, by its index in [`ScaleBench::devices`]; `None` on this Mac.
    pub device: Option<usize>,
    pub round: u32,
    /// Its stated length, in whole seconds.
    pub seconds: u32,
}

/// A sitting: units in order, each tier's alone, their stated lengths within [`ScaleBench::sitting_seconds`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Sitting {
    pub tier: usize,
    pub units: Vec<Unit>,
}

impl Sitting {
    pub fn seconds(&self) -> u32 {
        self.units.iter().map(|unit| unit.seconds).sum()
    }
}

/// Settings a smoke run changes: every run's frames and size, so the whole batch proves it starts in minutes.
#[derive(Clone, Copy, Debug)]
pub struct Smoke {
    pub frames: u32,
    pub size: &'static str,
}

/// The smoke run's: 200 frames at 320×180.
pub const SMOKE: Smoke = Smoke { frames: 200, size: "320x180" };

impl ScaleBench {
    /// The manifest in `text`, checked whole: the first problem found, as words.
    pub fn parse(text: &str) -> Result<Self, String> {
        let manifest: Self = sapphire_serialize::from_ron(text).map_err(|error| error.to_string())?;
        manifest.check()?;
        Ok(manifest)
    }

    fn check(&self) -> Result<(), String> {
        let name_ok = |name: &str| {
            !name.is_empty() && name.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
        };
        if self.rounds == 0 || self.sitting_seconds == 0 || self.load_bound.is_nan() || self.load_bound <= 0.0
        {
            return Err("rounds, sitting_seconds and load_bound must be above 0".into());
        }
        if self.tiers.is_empty() || self.runs.is_empty() {
            return Err("the manifest lists no tier or no run".into());
        }
        let mut tiers = BTreeSet::new();
        for tier in &self.tiers {
            if !tiers.insert(tier.name.as_str()) {
                return Err(format!("tier {:?} is listed twice", tier.name));
            }
            if tier.lap == 0 || tier.frames_a_second == 0 || size(&tier.size).is_none() {
                return Err(format!(
                    "tier {:?}: a lap, a rate and a size <width>x<height> are needed",
                    tier.name
                ));
            }
        }
        let mut runs = BTreeSet::new();
        for run in &self.runs {
            let what = format!("run {:?}", run.name);
            if !name_ok(&run.name) || !runs.insert(run.name.as_str()) {
                return Err(format!("{what}: names are lowercase letters, digits and hyphens, each once"));
            }
            if let Some(unknown) =
                run.tiers.iter().chain(run.tier_args.keys()).find(|name| !tiers.contains(name.as_str()))
            {
                return Err(format!("{what} names the tier {unknown:?}, which the manifest doesn't list"));
            }
            if let Some(owned) = run.args.iter().find(|arg| RUNNER_FLAGS.contains(&arg.as_str())) {
                return Err(format!("{what}: {owned} is the runner's: the tier, `size` or `frames` give it"));
            }
            if run.read.is_empty() {
                return Err(format!("{what} is read for nothing"));
            }
            if run.phases == 0 || run.rounds == Some(0) || run.frames == Frames::Count(0) {
                return Err(format!("{what}: phases, rounds and frames must be above 0"));
            }
            if let Some(tier) =
                self.tiers.iter().find(|tier| run.on(&tier.name) && run.pace(&tier.name) == Some(0))
            {
                return Err(format!("{what}: {PACE_FLAG} on {} takes frames a second above 0", tier.name));
            }
            if run.size.as_deref().is_some_and(|text| size(text).is_none()) {
                return Err(format!("{what}: its size isn't <width>x<height>"));
            }
            let mut variants = BTreeSet::new();
            if run.variants.len() == 1
                || run.variants.iter().any(|variant| {
                    !name_ok(&variant.name)
                        || !variants.insert(&variant.name)
                        || variant.content.as_deref().is_some_and(|content| !name_ok(content))
                })
            {
                return Err(format!("{what}: a pair has two variants or more, each named once"));
            }
            if !run.rows.is_empty() && run.variants.is_empty() {
                return Err(format!("{what}: `rows` are a pair's table, and it has no variants"));
            }
        }
        for run in &self.runs {
            if let Some(beside) = &run.beside
                && !runs.contains(beside.as_str())
            {
                return Err(format!("run {:?} stands beside {beside:?}, which isn't a run", run.name));
            }
        }
        let mut devices = BTreeSet::new();
        for device in &self.devices {
            if !name_ok(&device.name) || !devices.insert(device.name.as_str()) {
                return Err(format!(
                    "device {:?}: names are lowercase letters, digits and hyphens, each once",
                    device.name
                ));
            }
        }
        for run in self.runs.iter().filter(|run| !run.devices.is_empty()) {
            if let Some(unknown) = run.devices.iter().find(|name| !devices.contains(name.as_str())) {
                return Err(format!(
                    "run {:?} walks on {unknown:?}, which the manifest doesn't list",
                    run.name
                ));
            }
            if run.timed || !run.variants.is_empty() || run.beside.is_some() {
                return Err(format!(
                    "run {:?} walks on a device: it is of counts alone (`timed: false`), with no variants and \
                     nothing beside it, since a device's time is never read",
                    run.name
                ));
            }
        }
        // Each unit fits a sitting.
        for unit in self.units(None).into_iter().chain(self.counting(None)) {
            if unit.seconds > self.sitting_seconds {
                return Err(format!(
                    "{} states {} s, past a sitting's {} s: give it fewer frames",
                    unit.name, unit.seconds, self.sitting_seconds
                ));
            }
        }
        Ok(())
    }

    /// The tier called `name`.
    pub fn tier(&self, name: &str) -> Option<&Tier> {
        self.tiers.iter().find(|tier| tier.name == name)
    }

    /// The run called `name`.
    pub fn run(&self, name: &str) -> Option<&Run> {
        self.runs.iter().find(|run| run.name == name)
    }

    /// The device called `name`, by its index.
    pub fn device(&self, name: &str) -> Option<usize> {
        self.devices.iter().position(|device| device.name == name)
    }

    /// Every unit that walks on device `device` (by its index), on the tiers `tiers` names (every tier its runs
    /// are on, when empty), in the batch's order: rounds, then tiers, then runs.
    pub fn on_device(&self, device: usize, tiers: &[String], smoke: Option<Smoke>) -> Vec<Unit> {
        let name = &self.devices[device].name;
        let walks: Vec<(usize, &Run)> =
            self.runs.iter().enumerate().filter(|(_, run)| run.devices.contains(name)).collect();
        let most = walks.iter().map(|(_, run)| self.rounds_of(run)).max().unwrap_or(0);
        let mut units = Vec::new();
        for round in 1..=most {
            for (tier_index, tier) in self.tiers.iter().enumerate() {
                if !tiers.is_empty() && !tiers.contains(&tier.name) {
                    continue;
                }
                for &(run_index, run) in &walks {
                    if round > self.rounds_of(run) || !run.on(&tier.name) {
                        continue;
                    }
                    units.push(Unit {
                        name: unit_name(run, tier, None, Some(name), round),
                        run: run_index,
                        tier: tier_index,
                        variant: None,
                        device: Some(device),
                        round,
                        seconds: self.seconds(run, tier, smoke),
                    });
                }
            }
        }
        units
    }

    /// Every unit, this Mac's (timed and counting) and every device's: what the report reads.
    pub fn every_unit(&self, smoke: Option<Smoke>) -> Vec<Unit> {
        let devices = (0..self.devices.len()).flat_map(|device| self.on_device(device, &[], smoke));
        self.units(smoke).into_iter().chain(self.counting(smoke)).chain(devices).collect()
    }

    fn rounds_of(&self, run: &Run) -> u32 {
        run.rounds.unwrap_or(self.rounds)
    }

    /// Every timed unit in the order it runs: rounds, then tiers, then runs; a pair's variants in their order on
    /// odd rounds and the other way on even ones, so neither always goes first.
    pub fn units(&self, smoke: Option<Smoke>) -> Vec<Unit> {
        self.ordered(smoke, true)
    }

    /// Every unit of counts alone, in the same order.
    pub fn counting(&self, smoke: Option<Smoke>) -> Vec<Unit> {
        self.ordered(smoke, false)
    }

    fn ordered(&self, smoke: Option<Smoke>, timed: bool) -> Vec<Unit> {
        let most = self.runs.iter().map(|run| self.rounds_of(run)).max().unwrap_or(0);
        let mut units = Vec::new();
        for round in 1..=most {
            for (tier_index, tier) in self.tiers.iter().enumerate() {
                for (run_index, run) in self.runs.iter().enumerate() {
                    if (run.timed || run.paced()) != timed
                        || round > self.rounds_of(run)
                        || !run.on(&tier.name)
                        || !run.devices.is_empty()
                    {
                        continue;
                    }
                    let seconds = self.seconds(run, tier, smoke);
                    let mut variants: Vec<Option<usize>> = if run.variants.is_empty() {
                        vec![None]
                    } else {
                        (0..run.variants.len()).map(Some).collect()
                    };
                    if round % 2 == 0 {
                        variants.reverse();
                    }
                    for variant in variants {
                        units.push(Unit {
                            name: unit_name(run, tier, variant, None, round),
                            run: run_index,
                            tier: tier_index,
                            variant,
                            device: None,
                            round,
                            seconds,
                        });
                    }
                }
            }
        }
        units
    }

    /// The frames a run's bench measures on `tier`.
    pub fn frames(&self, run: &Run, tier: &Tier, smoke: Option<Smoke>) -> u32 {
        let frames = match run.frames {
            Frames::Lap => tier.lap,
            Frames::Count(count) => count,
        };
        smoke.map_or(frames, |smoke| frames.min(smoke.frames))
    }

    /// A unit's stated length: its load, then its phases' frames at the tier's rate, or its pace where that's
    /// slower, in whole seconds.
    fn seconds(&self, run: &Run, tier: &Tier, smoke: Option<Smoke>) -> u32 {
        let frames = u64::from(self.frames(run, tier, smoke)) * u64::from(run.phases);
        let rate = run.pace(&tier.name).map_or(tier.frames_a_second, |pace| pace.min(tier.frames_a_second));
        let drawing = frames.div_ceil(u64::from(rate.max(1)));
        u32::try_from(u64::from(tier.startup_seconds) + drawing).unwrap_or(u32::MAX)
    }

    /// The timed units packed into sittings, in order: each sitting one tier's, none past
    /// [`Self::sitting_seconds`] by its units' stated lengths, and a round's units of a pair in one sitting.
    pub fn sittings(&self, smoke: Option<Smoke>) -> Vec<Sitting> {
        let units = self.units(smoke);
        let mut sittings: Vec<Sitting> = Vec::new();
        let mut index = 0;
        while index < units.len() {
            // A pair's variants for one round go together, so they alternate inside one hold.
            let unit = &units[index];
            let group = units[index..]
                .iter()
                .take_while(|next| next.run == unit.run && next.tier == unit.tier && next.round == unit.round)
                .count();
            let block = &units[index..index + group];
            let seconds: u32 = block.iter().map(|unit| unit.seconds).sum();
            let fits = sittings.last().is_some_and(|sitting| {
                sitting.tier == unit.tier && sitting.seconds() + seconds <= self.sitting_seconds
            });
            if fits {
                sittings.last_mut().expect("a sitting").units.extend_from_slice(block);
            } else if seconds <= self.sitting_seconds {
                sittings.push(Sitting { tier: unit.tier, units: block.to_vec() });
            } else {
                // A pair too long to share a hold: each variant in a hold of its own, still alternating.
                for unit in block {
                    sittings.push(Sitting { tier: unit.tier, units: vec![unit.clone()] });
                }
            }
            index += group;
        }
        sittings
    }

    /// The sandbox's arguments for `unit`: the tier and size, the frames, the trace if the run takes one, then
    /// the run's, its tier's and its variant's arguments, with `{replays}` filled in.
    pub fn args(&self, unit: &Unit, smoke: Option<Smoke>, replays: &str, trace: &str) -> Vec<String> {
        let (run, tier) = (&self.runs[unit.run], &self.tiers[unit.tier]);
        let size = match smoke {
            Some(smoke) => smoke.size,
            None => run.size.as_deref().unwrap_or(&tier.size),
        };
        let frames = self.frames(run, tier, smoke).to_string();
        let mut args: Vec<String> =
            ["--bench", &frames, "--tier", &tier.name, "--size", size].map(str::to_owned).to_vec();
        if run.trace {
            args.extend(["--bench-trace".to_owned(), trace.to_owned()]);
        }
        let extra = run.tier_args.get(&tier.name).into_iter().flatten();
        let variant = unit.variant.map(|index| &run.variants[index].args).into_iter().flatten();
        for arg in run.args.iter().chain(extra).chain(variant) {
            args.push(arg.replace("{replays}", replays));
        }
        args
    }
}

impl Run {
    /// Whether it runs on the tier called `tier`.
    pub fn on(&self, tier: &str) -> bool {
        self.tiers.is_empty() || self.tiers.iter().any(|name| name == tier)
    }

    /// Whether its counts must read the same in every round: a run of counts alone (`--draw-every`) that sees
    /// every texture whole. A timed run streams textures as a game does, and a counting run told to stream
    /// does too, so their streaming rows follow when each read lands.
    pub fn exact(&self) -> bool {
        let streams =
            |args: &[String]| args.windows(2).any(|pair| pair[0] == "--texture-streaming" && pair[1] == "on");
        !self.timed
            && !streams(&self.args)
            && !self.variants.iter().any(|variant| streams(&variant.args))
            && !self.tier_args.values().any(|args| streams(args))
    }

    /// Whether it runs a day along its walk (`--day-length`), whose phases must start at one hour.
    pub fn runs_the_day(&self) -> bool {
        self.args.iter().any(|arg| arg == "--day-length")
    }

    /// Its frames a second on the tier called `tier` under `--bench-pace` (M5 Z2): a walk paced at the tier's
    /// frame time, each frame a frame's time as a game's is, so the streamer's waits are read in frames that
    /// last one. `None` unpaced; `Some(0)` for a pace that isn't a count, which the check refuses.
    pub fn pace(&self, tier: &str) -> Option<u32> {
        let mut args = self.args.iter().chain(self.tier_args.get(tier).into_iter().flatten());
        args.position(|arg| arg == PACE_FLAG)?;
        Some(args.next().and_then(|rate| rate.parse().ok()).unwrap_or(0))
    }

    /// Whether any tier's walk is paced: timed work all the same, planned into sittings, even of counts alone.
    pub fn paced(&self) -> bool {
        self.args.iter().chain(self.tier_args.values().flatten()).any(|arg| arg == PACE_FLAG)
    }
}

/// The sandbox's bench paced at a tier's frame rate (M5 Z2).
const PACE_FLAG: &str = "--bench-pace";

/// Arguments the runner gives every unit, so a run can't.
const RUNNER_FLAGS: [&str; 4] = ["--bench", "--tier", "--size", "--bench-trace"];

/// A unit's name: `<run>.<tier>[.<variant>][.<device>].r<round>`.
fn unit_name(run: &Run, tier: &Tier, variant: Option<usize>, device: Option<&str>, round: u32) -> String {
    let mut name = format!("{}.{}", run.name, tier.name);
    for part in [variant.map(|index| run.variants[index].name.as_str()), device].into_iter().flatten() {
        name.push('.');
        name.push_str(part);
    }
    format!("{name}.r{round}")
}

/// `<width>x<height>`, both above 0.
fn size(text: &str) -> Option<(u32, u32)> {
    let (width, height) = text.split_once('x')?;
    let (width, height) = (width.parse().ok()?, height.parse().ok()?);
    (width > 0 && height > 0).then_some((width, height))
}

#[cfg(test)]
mod tests {
    use super::*;

    pub(crate) const SMALL: &str = r#"(
        type: "sapphire::testkit::ScaleBench",
        version: 1,
        data: (
            preset: "ci",
            rounds: 2,
            sitting_seconds: 600,
            load_bound: 8.0,
            tiers: [
                (name: "desktop-low", size: "1920x1080", cook: desktop, lap: 7600, frames_a_second: 100,
                 startup_seconds: 10),
                (name: "mobile-low", size: "1280x590", cook: mobile, lap: 3800, frames_a_second: 100,
                 startup_seconds: 10),
            ],
            runs: [
                (name: "tour", about: "the tour", args: ["--zone", "district", "--zone-walk", "tour"],
                 frames: Lap, read: [Cpu, Gpu, Counts]),
                (name: "lods", about: "levels", args: ["--zone", "district", "--zone-walk", "tour"],
                 frames: Count(600), read: [Counts], tiers: ["desktop-low"],
                 variants: [(name: "on", args: ["--lods", "on"]), (name: "off", args: ["--lods", "off"])],
                 rows: ["triangles"], beside: Some("tour")),
                (name: "counted", about: "counts", args: ["--draw-every", "7", "--bench-trace", "x"],
                 frames: Count(200), read: [Pipelines], timed: false),
            ],
        ),
    )"#;

    fn small() -> String {
        // The counting run's `--bench-trace` is the runner's: take it out for the manifest that parses.
        SMALL.replace(r#", "--bench-trace", "x""#, "")
    }

    #[test]
    fn the_manifest_parses_and_refuses_an_unknown_field() {
        let manifest = ScaleBench::parse(&small()).expect("parses");
        assert_eq!(manifest.runs.len(), 3);
        assert_eq!(manifest.tier("mobile-low").map(|tier| tier.lap), Some(3800));
        let unknown = small().replace("rounds: 2,", "rounds: 2, laps: 3,");
        assert!(ScaleBench::parse(&unknown).unwrap_err().contains("laps"), "a field it doesn't know");
        let in_a_run = small().replace("frames: Lap,", "frames: Lap, colour: 3,");
        assert!(ScaleBench::parse(&in_a_run).is_err(), "nor in a run");
    }

    #[test]
    fn the_manifest_refuses_what_the_runner_owns_and_names_that_dont_resolve() {
        assert!(ScaleBench::parse(SMALL).unwrap_err().contains("--bench-trace"));
        let tier = small().replace(r#"tiers: ["desktop-low"]"#, r#"tiers: ["desktop-medium"]"#);
        assert!(ScaleBench::parse(&tier).unwrap_err().contains("desktop-medium"));
        let beside = small().replace(r#"beside: Some("tour")"#, r#"beside: Some("walk")"#);
        assert!(ScaleBench::parse(&beside).unwrap_err().contains("walk"));
        let lone = small().replace(r#", (name: "off", args: ["--lods", "off"])"#, "");
        assert!(ScaleBench::parse(&lone).unwrap_err().contains("two variants"));
        let long = small().replace("frames: Count(600)", "frames: Count(60000)");
        assert!(
            ScaleBench::parse(&long).unwrap_err().contains("past a sitting"),
            "a unit longer than a hold"
        );
    }

    #[test]
    fn a_pairs_rounds_alternate_and_each_round_stays_in_one_sitting() {
        let manifest = ScaleBench::parse(&small()).unwrap();
        let names: Vec<String> = manifest.units(None).into_iter().map(|unit| unit.name).collect();
        assert_eq!(
            names,
            [
                "tour.desktop-low.r1",
                "lods.desktop-low.on.r1",
                "lods.desktop-low.off.r1",
                "tour.mobile-low.r1",
                "tour.desktop-low.r2",
                "lods.desktop-low.off.r2",
                "lods.desktop-low.on.r2",
                "tour.mobile-low.r2",
            ]
        );
        for sitting in manifest.sittings(None) {
            let on = sitting.units.iter().position(|unit| unit.name.contains(".on."));
            let off = sitting.units.iter().position(|unit| unit.name.contains(".off."));
            assert_eq!(on.is_some(), off.is_some(), "a round's pair is held together: {sitting:?}");
        }
        assert_eq!(manifest.counting(None).len(), 4, "counts alone run apart, two rounds a tier");
    }

    #[test]
    fn no_sitting_plans_past_its_length() {
        let manifest = ScaleBench::parse(&small()).unwrap();
        // tour: 10 + 3 × 7600 / 100 = 238 s; lods: 10 + 3 × 600 / 100 = 28 s each.
        let sittings = manifest.sittings(None);
        assert!(sittings.iter().all(|sitting| sitting.seconds() <= 600), "{sittings:?}");
        assert!(sittings.iter().all(|sitting| sitting.units.iter().all(|unit| unit.tier == sitting.tier)));
        assert_eq!(sittings.iter().map(Sitting::seconds).sum::<u32>(), 2 * (238 + 28 + 28 + 124));
        let tight =
            ScaleBench::parse(&small().replace("sitting_seconds: 600", "sitting_seconds: 260")).unwrap();
        let sittings = tight.sittings(None);
        assert!(sittings.iter().all(|sitting| sitting.seconds() <= 260), "{sittings:?}");
        assert!(sittings.len() > 4);
    }

    #[test]
    fn a_units_arguments_put_the_runners_first_and_fill_the_folders() {
        let traced = small().replace(r#"rows: ["triangles"]"#, r#"rows: ["triangles"], trace: true"#);
        let manifest =
            ScaleBench::parse(&traced.replace(r#""--lods", "on""#, r#""--lods", "on", "{replays}/a""#))
                .unwrap();
        let unit =
            manifest.units(None).into_iter().find(|unit| unit.name == "lods.desktop-low.on.r1").unwrap();
        let args = manifest.args(&unit, None, "/r", "/t.csv");
        assert_eq!(
            args[..8],
            ["--bench", "600", "--tier", "desktop-low", "--size", "1920x1080", "--bench-trace", "/t.csv"]
        );
        assert!(args.ends_with(&["--lods".to_owned(), "on".to_owned(), "/r/a".to_owned()]));
        let smoke = manifest.args(&unit, Some(SMOKE), "/r", "/t.csv");
        assert_eq!(smoke[..6], ["--bench", "200", "--tier", "desktop-low", "--size", "320x180"]);
    }

    #[test]
    fn a_variant_mounts_the_cook_beside_the_tiers_it_names() {
        let manifest = ScaleBench::parse(include_str!("../../../../bench/scale.ron")).unwrap();
        let pair = manifest.run("cell-levels").expect("D6's pair (M5 Z2)");
        let tier = std::path::Path::new("batch/content/budget/mobile");
        let dirs: Vec<_> = pair.variants.iter().map(|variant| variant.content_dir(tier)).collect();
        assert_eq!(
            dirs,
            [tier.to_owned(), std::path::PathBuf::from("batch/content/budget/mobile-no-cell-levels")]
        );
        let bad = SMALL.replacen("args: [\"--lods\", \"off\"]", "args: [], content: Some(\"No Levels\")", 1);
        assert_ne!(bad, SMALL, "the small manifest has a pair to break");
        assert!(ScaleBench::parse(&bad).is_err(), "a content's name is a run's");
    }

    #[test]
    fn the_committed_manifest_parses_and_plans_every_tier() {
        let manifest = ScaleBench::parse(include_str!("../../../../bench/scale.ron")).unwrap();
        let sittings = manifest.sittings(None);
        for tier in 0..manifest.tiers.len() {
            assert!(
                sittings.iter().any(|sitting| sitting.tier == tier),
                "no sitting for {}",
                manifest.tiers[tier].name
            );
        }
        assert!(sittings.iter().all(|sitting| sitting.seconds() <= manifest.sitting_seconds));
        // B4's eight walks, each beside itself.
        let baseline =
            manifest.runs.iter().filter(|run| run.beside.as_deref() == Some(run.name.as_str())).count();
        assert_eq!(baseline * manifest.tiers.len(), 8);
        assert!(!manifest.counting(None).is_empty());
    }

    /// A phone draws its tier's capped scene onto a native panel and upscales it (spec §3.1's "~1600×740 →
    /// native"), so every phone unit is at a panel's size, never the scene's; and exit test 2's runs give every
    /// tier a pool of exactly half what its cook's district holds whole, in whole MiB rounded down (M5 Z2): the
    /// `texture pool bytes` `counted` holds at the tour's end on B7's district, as bench/scale.ron says.
    #[test]
    fn the_phones_draw_at_a_panel_and_every_tier_streams_under_half_its_cook() {
        let manifest = ScaleBench::parse(include_str!("../../../../bench/scale.ron")).unwrap();
        for tier in manifest.tiers.iter().filter(|tier| tier.cook == Cook::Mobile) {
            assert_eq!(tier.size, "2532x1170", "{} draws at its scene's size", tier.name);
        }
        let whole = |cook: Cook| match cook {
            Cook::Desktop => 2_733_906_860_u64,
            Cook::Mobile => 699_285_048,
        };
        for run in
            manifest.runs.iter().filter(|run| run.name == "streaming" || run.name == "counted-streaming")
        {
            for tier in &manifest.tiers {
                let pool = run.tier_args.get(&tier.name).and_then(|args| {
                    let at = args.iter().position(|arg| arg == "--texture-pool")?;
                    args.get(at + 1)?.parse::<u64>().ok()
                });
                assert_eq!(
                    pool,
                    Some(whole(tier.cook) / 2 / (1 << 20)),
                    "{} on {}: half its cook's whole",
                    run.name,
                    tier.name
                );
                // The desktops' pool holds the views' worst ask on the desktop cook (Desktop-High's, B7's district).
                if tier.cook == Cook::Desktop {
                    assert!(pool.is_some_and(|pool| pool << 20 >= 1_263_504_874), "{}", tier.name);
                }
            }
        }
    }

    /// The counting walk under a binding pool is paced at each tier's frame rate (tiers.ron's `frame_rate`), so
    /// it's timed work, planned into the sittings and never run beside them, lasting its frames at that rate.
    #[test]
    fn the_streaming_counting_walk_is_paced_and_planned_into_the_sittings() {
        let manifest = ScaleBench::parse(include_str!("../../../../bench/scale.ron")).unwrap();
        let run = manifest.run("counted-streaming").unwrap();
        let paces: Vec<_> = manifest.tiers.iter().map(|tier| run.pace(&tier.name)).collect();
        assert_eq!(paces, [Some(60), Some(60), Some(60), Some(30)]);
        assert!(run.paced() && !run.timed, "read as counts");
        let named =
            |units: Vec<Unit>| units.into_iter().any(|unit| unit.name.starts_with("counted-streaming."));
        assert!(named(manifest.units(None)) && !named(manifest.counting(None)));
        let unit =
            manifest.units(None).into_iter().find(|unit| unit.name == "counted-streaming.mobile-low.r1");
        let tier = &manifest.tiers[3];
        assert_eq!(unit.unwrap().seconds, tier.startup_seconds + tier.lap.div_ceil(30));
        assert!(manifest.run("counted").unwrap().pace("desktop-high").is_none());
        let unpaced = include_str!("../../../../bench/scale.ron")
            .replace("\"--bench-pace\", \"30\"", "\"--bench-pace\"");
        assert!(ScaleBench::parse(&unpaced).unwrap_err().contains("--bench-pace on mobile-low"));
    }

    /// A run on a device (M5 Z2) is apart from the batch: none of its units is the Mac's, each device has its
    /// own, named for it, and a device's run must be of counts alone on a device the manifest lists.
    #[test]
    fn a_devices_units_are_its_own_and_its_run_counts_alone() {
        let with_devices = small().replace(
            "            ],\n        ),\n    )",
            r#"                (name: "device", about: "on a device", args: ["--draw-every", "7"], frames: Lap,
                 tiers: ["mobile-low"], rounds: Some(1), read: [Counts], timed: false, devices: ["sim", "emu"]),
            ],
            devices: [
                (name: "sim", about: "a simulator", preset: "ci"),
                (name: "emu", about: "an emulator", preset: "ci"),
            ],
        ),
    )"#,
        );
        assert_ne!(with_devices, small(), "the small manifest has a device's run");
        let manifest = ScaleBench::parse(&with_devices).expect("parses");
        assert!(
            manifest.units(None).iter().chain(&manifest.counting(None)).all(|unit| unit.device.is_none())
        );
        assert!(!manifest.counting(None).iter().any(|unit| unit.name.starts_with("device.")));
        let emu = manifest.device("emu").expect("a device");
        let names: Vec<String> =
            manifest.on_device(emu, &[], None).into_iter().map(|unit| unit.name).collect();
        assert_eq!(names, ["device.mobile-low.emu.r1"]);
        assert!(manifest.on_device(emu, &["desktop-low".to_owned()], None).is_empty());
        assert_eq!(
            manifest.every_unit(None).len(),
            manifest.units(None).len() + manifest.counting(None).len() + 2
        );
        let unknown = with_devices.replace("\"sim\", \"emu\"]", "\"sim\", \"phone\"]");
        assert!(ScaleBench::parse(&unknown).unwrap_err().contains("phone"));
        let timed = with_devices.replace("read: [Counts], timed: false, devices", "read: [Counts], devices");
        assert!(ScaleBench::parse(&timed).unwrap_err().contains("counts alone"));
    }

    /// The devices' walk is `counted-streaming`'s, paced as it is, under the `ci` district's pool on the phone
    /// tiers alone: 28 MiB, the orchestrator's ruling (M5 Z2), which holds the views' worst ask of 28,903,381
    /// bytes and is within a MiB of the 28,932,504 its phone tree holds whole (tests/counts/district-ci.txt's
    /// `texture pool bytes` on the phones).
    #[test]
    fn the_devices_walk_counted_streamings_walk_under_half_the_ci_districts_textures() {
        let manifest = ScaleBench::parse(include_str!("../../../../bench/scale.ron")).unwrap();
        let (device, counted) =
            (manifest.run("device-streaming").unwrap(), manifest.run("counted-streaming").unwrap());
        assert_eq!(device.args, counted.args);
        assert_eq!(device.devices, ["ios-sim", "android"]);
        assert!(manifest.devices.iter().all(|device| device.preset == "ci"));
        let table = include_str!("../../../../tests/counts/district-ci.txt");
        for tier in ["mobile-high", "mobile-low"] {
            let whole: u64 = table
                .lines()
                .find(|line| line.starts_with(&format!("{tier}-dusk | texture pool bytes |")))
                .and_then(|line| line.split(" worst ").nth(1)?.split_whitespace().next()?.parse().ok())
                .expect("the ci walk's whole textures on the phone");
            let pool: u64 = device.tier_args[tier][1].parse().unwrap();
            assert_eq!(device.tier_args[tier][0], "--texture-pool");
            assert_eq!(pool, 28, "{tier}: the device rows' pool");
            assert!(pool << 20 >= 28_903_381, "{tier}: it holds the views' worst ask");
            assert_eq!(whole >> 20, pool - 1, "{tier}: within a MiB of the whole");
            assert_eq!(device.pace(tier), counted.pace(tier), "{tier}: paced as the batch's walk");
        }
        assert!(
            device.tiers.iter().all(|tier| manifest.tier(tier).is_some_and(|tier| tier.cook == Cook::Mobile))
        );
    }

    /// M6 B7's two runs are the batch's district fight, by its own arguments: on the phones alone with the compute
    /// pool at one thread, three and the machine's own, and on every tier waiting for every frame and overlapped.
    #[test]
    fn the_pool_and_overlap_arms_are_the_district_fights() {
        let manifest = ScaleBench::parse(include_str!("../../../../bench/scale.ron")).unwrap();
        let fight = manifest.run("district-fight").unwrap();
        let arms = |run: &Run| -> Vec<(String, Vec<String>)> {
            run.variants.iter().map(|variant| (variant.name.clone(), variant.args.clone())).collect()
        };
        let owned = |args: &[&str]| -> Vec<String> { args.iter().map(|arg| (*arg).to_owned()).collect() };
        let pool = manifest.run("fight-pool").unwrap();
        assert_eq!((&pool.args, pool.frames, pool.trace), (&fight.args, fight.frames, true));
        assert_eq!(pool.tiers, ["mobile-high", "mobile-low"]);
        assert!(
            pool.tiers.iter().all(|tier| manifest.tier(tier).is_some_and(|tier| tier.cook == Cook::Mobile))
        );
        assert_eq!(
            arms(pool),
            [
                ("one".to_owned(), owned(&["--compute-threads", "1"])),
                ("three".to_owned(), owned(&["--compute-threads", "3"])),
                ("mac".to_owned(), Vec::new()),
            ]
        );
        let overlap = manifest.run("fight-overlap").unwrap();
        assert_eq!((&overlap.args, overlap.frames), (&fight.args, fight.frames));
        assert!(overlap.tiers.is_empty(), "every tier");
        assert_eq!(
            arms(overlap),
            [("waits".to_owned(), Vec::new()), ("overlaps".to_owned(), owned(&["--bench-overlap"]))]
        );
        let units = manifest.units(None);
        for (name, count) in [("fight-pool.", 3 * 2 * 2), ("fight-overlap.", 2 * 4 * 2)] {
            assert_eq!(units.iter().filter(|unit| unit.name.starts_with(name)).count(), count, "{name}");
        }
    }

    /// Z1's counting walks count the crowd the timed walks draw: the proving hub's crowd mounted, the ambient
    /// characters on, as `streaming` and the exit walk have them.
    #[test]
    fn the_counting_walks_count_the_crowd() {
        let manifest = ScaleBench::parse(include_str!("../../../../bench/scale.ron")).unwrap();
        for run in manifest.runs.iter().filter(|run| run.name == "counted" || run.name == "counted-streaming")
        {
            assert!(run.args.windows(2).any(|pair| pair == ["--mount", "zone_proving_hub"]), "{}", run.name);
            assert!(run.args.windows(2).any(|pair| pair == ["--ambient", "on"]), "{}", run.name);
        }
    }
}
