//! CPU topology detection for sizing the task pool (spec §4.3).
//!
//! The per-OS `#[cfg]` code in this module is an accepted exception to rule R7 (ADR 0001): core
//! counts come from OS-specific sources, and moving them into `sapphire_platform` would make
//! the job system depend on SDL. Everything platform-specific stays in this file.
//!
//! Sources:
//! - **Apple:** `sysctl` `hw.physicalcpu` and `hw.perflevel0.physicalcpu` (the P-cores).
//! - **Linux and Android:** `/sys/devices/system/cpu`, classified per CPU from `cpu_capacity` or
//!   `cpuinfo_max_freq` (the heuristic is documented on `classify_sysfs`).
//! - **Windows:** `GetLogicalProcessorInformationEx`, one record per physical core with its
//!   `EfficiencyClass`.
//! - **Elsewhere:** `std::thread::available_parallelism` for everything.

use sapphire_core::target::FormFactor;

/// Core counts of the machine, as far as the OS reports them.
///
/// Invariant: `logical >= physical >= performance >= 1`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct CpuTopology {
    /// Hardware threads this process may run on (`std::thread::available_parallelism`).
    pub logical: usize,
    /// Physical cores. Equals `logical` where the OS gives no better answer.
    pub physical: usize,
    /// Physical cores in the performance cluster(s): the cores worth putting workers on. Equals
    /// `physical` on CPUs without separate efficiency cores, or when unknown.
    pub performance: usize,
}

/// Most compute workers on desktop: beyond this, more threads stop paying for themselves.
const MAX_DESKTOP_WORKERS: usize = 16;

impl CpuTopology {
    /// Queries the OS. Never fails: anything it cannot learn falls back to the logical count.
    #[must_use]
    pub fn detect() -> Self {
        let logical = std::thread::available_parallelism().map_or(1, usize::from);
        let (physical, performance) = detect_cores().unwrap_or((logical, logical));
        Self::clamped(logical, physical, performance)
    }

    /// Builds a topology, restoring the field invariant.
    fn clamped(logical: usize, physical: usize, performance: usize) -> Self {
        let logical = logical.max(1);
        let physical = physical.clamp(1, logical);
        Self { logical, physical, performance: performance.clamp(1, physical) }
    }

    /// Compute workers per spec §4.3: desktop runs one per physical core except the main
    /// thread's (at most 16); mobile runs one per performance core except the main thread's.
    /// Always at least 1.
    #[must_use]
    pub fn default_compute_workers(&self, form_factor: FormFactor) -> usize {
        match form_factor {
            FormFactor::Desktop => self.physical.saturating_sub(1).clamp(1, MAX_DESKTOP_WORKERS),
            FormFactor::Mobile => self.performance.saturating_sub(1).max(1),
        }
    }

    /// Threads for long-running background tasks: 2 on mobile (spec §5.5), and a quarter of the
    /// physical cores (1–4) on desktop.
    #[must_use]
    pub fn default_async_compute_workers(&self, form_factor: FormFactor) -> usize {
        match form_factor {
            FormFactor::Desktop => (self.physical / 4).clamp(1, 4),
            FormFactor::Mobile => 2,
        }
    }
}

/// `(physical, performance)` core counts, or `None` if the OS gave no usable answer.
#[cfg(target_vendor = "apple")]
fn detect_cores() -> Option<(usize, usize)> {
    let physical = sysctl_count(c"hw.physicalcpu")?;
    // Apple Silicon numbers its performance levels from fastest; perflevel0 is the P-cores.
    // Older or virtualized machines lack the key and have only one kind of core.
    let performance = sysctl_count(c"hw.perflevel0.physicalcpu").unwrap_or(physical);
    Some((physical, performance))
}

#[cfg(target_vendor = "apple")]
fn sysctl_count(name: &std::ffi::CStr) -> Option<usize> {
    let mut value: libc::c_int = 0;
    let mut size = size_of::<libc::c_int>();
    // SAFETY: `name` is NUL-terminated, `value` and `size` describe a writable `c_int`, and the
    // null `newp` with length 0 means nothing is written to the kernel.
    let status = unsafe {
        libc::sysctlbyname(name.as_ptr(), (&raw mut value).cast(), &raw mut size, std::ptr::null_mut(), 0)
    };
    if status != 0 || size != size_of::<libc::c_int>() {
        return None;
    }
    usize::try_from(value).ok().filter(|&count| count > 0)
}

#[cfg(any(target_os = "linux", target_os = "android"))]
fn detect_cores() -> Option<(usize, usize)> {
    classify_sysfs(&read_sysfs(std::path::Path::new("/sys/devices/system/cpu")))
}

/// What sysfs says about one logical CPU (`cpuN`).
#[cfg(any(target_os = "linux", target_os = "android", test))]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct SysfsCpu {
    /// Lowest CPU number among this CPU's SMT siblings (itself without SMT), which identifies
    /// its physical core.
    core: usize,
    /// `cpu_capacity`: the scheduler's relative compute capacity, 1024 for the biggest core.
    capacity: Option<u64>,
    /// `cpufreq/cpuinfo_max_freq`, in kHz.
    max_freq_khz: Option<u64>,
}

/// Reads every `cpuN` directory under `root`. Missing or unreadable files leave fields unset.
#[cfg(any(target_os = "linux", target_os = "android", test))]
fn read_sysfs(root: &std::path::Path) -> Vec<SysfsCpu> {
    use std::fs;

    let read = |path: std::path::PathBuf| fs::read_to_string(path).ok();
    let number = |path| read(path)?.trim().parse::<u64>().ok();
    let Ok(entries) = fs::read_dir(root) else { return Vec::new() };
    entries
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let name = entry.file_name();
            let index = name.to_str()?.strip_prefix("cpu")?;
            if index.is_empty() || !index.bytes().all(|b| b.is_ascii_digit()) {
                return None;
            }
            let id: usize = index.parse().ok()?;
            let dir = entry.path();
            // `core_cpus_list` replaced `thread_siblings_list` in Linux 5.6; older kernels and
            // some vendor trees only have the latter.
            let siblings = read(dir.join("topology/core_cpus_list"))
                .or_else(|| read(dir.join("topology/thread_siblings_list")))
                .and_then(|list| lowest_cpu_in_list(&list));
            Some(SysfsCpu {
                core: siblings.unwrap_or(id),
                capacity: number(dir.join("cpu_capacity")),
                max_freq_khz: number(dir.join("cpufreq/cpuinfo_max_freq")),
            })
        })
        .collect()
}

/// The lowest CPU number in a sysfs CPU list such as `0-3,8,10-11`.
#[cfg(any(target_os = "linux", target_os = "android", test))]
fn lowest_cpu_in_list(list: &str) -> Option<usize> {
    list.trim().split(',').filter_map(|range| range.split('-').next()?.trim().parse::<usize>().ok()).min()
}

/// Counts physical and performance cores from sysfs data.
///
/// Physical cores are CPUs grouped by their SMT sibling list, which (unlike
/// `physical_package_id` + `core_id`) stays unique on recent arm64 kernels, where every CPU is
/// in package 0 and `core_id` restarts in each cluster.
///
/// Each CPU is classified on its own, from the best data it has:
/// - **`cpu_capacity`** (arm64 and Android kernels): performance if at least half the largest
///   capacity. Little cores sit around 20–40 % of the prime core; mid and big cores well above
///   half. On a Snapdragon 8 Elite (2 × 4.32 GHz + 6 × 3.53 GHz, no little cores) all 8 qualify.
/// - **`cpuinfo_max_freq`** otherwise: performance if at least 75 % of the highest maximum
///   frequency. Frequency is a weaker proxy than capacity, hence the stricter threshold: little
///   cores run at 55–65 % of the prime core's clock, big cores above 80 %, and Intel E-cores
///   below 75 % of the P-cores.
/// - **No data** (a hotplugged-off CPU, a VM): counted as performance, so a missing file never
///   shrinks the pool.
///
/// A physical core counts as performance if any of its threads does.
#[cfg(any(target_os = "linux", target_os = "android", test))]
fn classify_sysfs(cpus: &[SysfsCpu]) -> Option<(usize, usize)> {
    use std::collections::HashSet;

    if cpus.is_empty() {
        return None;
    }
    let max_capacity = cpus.iter().filter_map(|cpu| cpu.capacity).max();
    let max_freq = cpus.iter().filter_map(|cpu| cpu.max_freq_khz).max();
    let is_performance = |cpu: &SysfsCpu| match (cpu.capacity, max_capacity, cpu.max_freq_khz, max_freq) {
        (Some(capacity), Some(max), _, _) => capacity * 2 >= max,
        (None, _, Some(freq), Some(max)) => freq * 4 >= max * 3,
        _ => true,
    };
    let physical = cpus.iter().map(|cpu| cpu.core).collect::<HashSet<_>>().len();
    let performance =
        cpus.iter().filter(|cpu| is_performance(cpu)).map(|cpu| cpu.core).collect::<HashSet<_>>().len();
    Some((physical, performance))
}

#[cfg(windows)]
fn detect_cores() -> Option<(usize, usize)> {
    use std::ptr;

    use windows_sys::Win32::System::SystemInformation::{
        GetLogicalProcessorInformationEx, RelationProcessorCore, SYSTEM_LOGICAL_PROCESSOR_INFORMATION_EX,
    };

    let mut length = 0u32;
    // SAFETY: a null buffer with length 0 only asks for the required size; the call writes
    // nothing but `length`.
    unsafe { GetLogicalProcessorInformationEx(RelationProcessorCore, ptr::null_mut(), &raw mut length) };
    if length == 0 {
        return None;
    }
    // `u64` elements keep the buffer 8-byte aligned, as the records expect.
    let mut buffer = vec![0u64; usize::try_from(length).ok()?.div_ceil(8)];
    // SAFETY: `buffer` is writable for at least `length` bytes and suitably aligned.
    let ok = unsafe {
        GetLogicalProcessorInformationEx(RelationProcessorCore, buffer.as_mut_ptr().cast(), &raw mut length)
    };
    if ok == 0 {
        return None;
    }

    let bytes = usize::try_from(length).ok()?.min(buffer.len() * 8);
    let base = buffer.as_ptr().cast::<u8>();
    // Header (relationship + size) plus `Flags` and `EfficiencyClass`.
    const MIN_RECORD: usize = 10;
    let mut classes = Vec::new();
    let mut offset = 0;
    while offset + MIN_RECORD <= bytes {
        // SAFETY: `offset + MIN_RECORD <= bytes`, so the header and the two leading bytes of the
        // processor record are inside the buffer the API filled; fields are read unaligned and
        // no reference to the (possibly larger) union is created.
        let (relationship, size, efficiency_class) = unsafe {
            let record = base.add(offset).cast::<SYSTEM_LOGICAL_PROCESSOR_INFORMATION_EX>();
            (
                ptr::read_unaligned(&raw const (*record).Relationship),
                ptr::read_unaligned(&raw const (*record).Size),
                ptr::read_unaligned(&raw const (*record).Anonymous.Processor.EfficiencyClass),
            )
        };
        let size = usize::try_from(size).ok()?;
        if size < MIN_RECORD || offset + size > bytes {
            break;
        }
        if relationship == RelationProcessorCore {
            classes.push(efficiency_class);
        }
        offset += size;
    }
    classify_efficiency_classes(&classes)
}

/// `(physical, performance)` from one `EfficiencyClass` per physical core: Windows ranks core
/// types from 0 (most efficient) upward, so the performance cores are those in the highest
/// class. On CPUs with one core type every core reports the same class.
#[cfg(any(windows, test))]
fn classify_efficiency_classes(classes: &[u8]) -> Option<(usize, usize)> {
    let top = *classes.iter().max()?;
    Some((classes.len(), classes.iter().filter(|&&class| class == top).count()))
}

/// Elsewhere there is no detection: every hardware thread counts as a performance core.
#[cfg(not(any(target_vendor = "apple", target_os = "linux", target_os = "android", windows)))]
fn detect_cores() -> Option<(usize, usize)> {
    None
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use super::*;

    #[test]
    fn detected_topology_is_sane() {
        let topology = CpuTopology::detect();
        assert!(topology.logical >= topology.physical, "{topology:?}");
        assert!(topology.physical >= topology.performance, "{topology:?}");
        assert!(topology.performance >= 1, "{topology:?}");
        assert_eq!(topology.logical, std::thread::available_parallelism().map_or(1, usize::from));
    }

    #[test]
    fn worker_defaults() {
        let laptop = CpuTopology { logical: 10, physical: 10, performance: 4 };
        assert_eq!(laptop.default_compute_workers(FormFactor::Desktop), 9);
        assert_eq!(laptop.default_compute_workers(FormFactor::Mobile), 3);
        assert_eq!(laptop.default_async_compute_workers(FormFactor::Desktop), 2);
        assert_eq!(laptop.default_async_compute_workers(FormFactor::Mobile), 2);

        let threadripper = CpuTopology { logical: 128, physical: 64, performance: 64 };
        assert_eq!(threadripper.default_compute_workers(FormFactor::Desktop), 16);
        assert_eq!(threadripper.default_async_compute_workers(FormFactor::Desktop), 4);

        let single = CpuTopology { logical: 1, physical: 1, performance: 1 };
        assert_eq!(single.default_compute_workers(FormFactor::Desktop), 1);
        assert_eq!(single.default_compute_workers(FormFactor::Mobile), 1);
        assert_eq!(single.default_async_compute_workers(FormFactor::Desktop), 1);
    }

    #[test]
    fn clamping_restores_the_invariant() {
        assert_eq!(CpuTopology::clamped(8, 16, 32), CpuTopology { logical: 8, physical: 8, performance: 8 });
        assert_eq!(CpuTopology::clamped(0, 0, 0), CpuTopology { logical: 1, physical: 1, performance: 1 });
    }

    /// One CPU in a fake sysfs tree. `None` leaves the file out.
    #[derive(Clone, Default)]
    struct Cpu {
        siblings: Option<String>,
        legacy_siblings: Option<String>,
        capacity: Option<u64>,
        max_freq_khz: Option<u64>,
    }

    /// A fake `/sys/devices/system/cpu` in the system temp directory, removed on drop.
    struct FakeSysfs(PathBuf);

    impl FakeSysfs {
        fn new(name: &str, cpus: &[Cpu]) -> Self {
            let root = std::env::temp_dir().join(format!("sapphire-sysfs-{}-{name}", std::process::id()));
            let _ = std::fs::remove_dir_all(&root);
            let write = |path: PathBuf, contents: String| {
                std::fs::create_dir_all(path.parent().expect("file paths have parents"))
                    .expect("create fake sysfs");
                std::fs::write(path, contents).expect("write fake sysfs");
            };
            for (id, cpu) in cpus.iter().enumerate() {
                let dir = root.join(format!("cpu{id}"));
                std::fs::create_dir_all(&dir).expect("create fake sysfs");
                if let Some(list) = &cpu.siblings {
                    write(dir.join("topology/core_cpus_list"), format!("{list}\n"));
                }
                if let Some(list) = &cpu.legacy_siblings {
                    write(dir.join("topology/thread_siblings_list"), format!("{list}\n"));
                }
                if let Some(capacity) = cpu.capacity {
                    write(dir.join("cpu_capacity"), format!("{capacity}\n"));
                }
                if let Some(freq) = cpu.max_freq_khz {
                    write(dir.join("cpufreq/cpuinfo_max_freq"), format!("{freq}\n"));
                }
            }
            // Siblings of `cpuN` that are not CPUs, as in the real tree.
            for other in ["cpufreq", "cpuidle", "possible", "online", "cpu"] {
                std::fs::create_dir_all(root.join(other)).expect("create fake sysfs");
            }
            Self(root)
        }

        fn path(&self) -> &Path {
            &self.0
        }

        fn classify(&self) -> Option<(usize, usize)> {
            classify_sysfs(&read_sysfs(self.path()))
        }
    }

    impl Drop for FakeSysfs {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// `count` single-threaded cores, each its own sibling list, starting at CPU `first`.
    fn cluster(first: usize, count: usize, capacity: Option<u64>, max_freq_khz: Option<u64>) -> Vec<Cpu> {
        (first..first + count)
            .map(|id| Cpu { siblings: Some(id.to_string()), capacity, max_freq_khz, ..Cpu::default() })
            .collect()
    }

    fn mobile_workers(counts: Option<(usize, usize)>) -> usize {
        let (physical, performance) = counts.expect("sysfs data");
        CpuTopology::clamped(physical, physical, performance).default_compute_workers(FormFactor::Mobile)
    }

    #[test]
    fn snapdragon_8_elite_has_no_little_cores() {
        // 6 × 3.53 GHz + 2 × 4.32 GHz Oryon cores.
        let mut cpus = cluster(0, 6, Some(870), Some(3_532_800));
        cpus.extend(cluster(6, 2, Some(1024), Some(4_320_000)));
        let with_capacity = FakeSysfs::new("elite", &cpus);
        assert_eq!(with_capacity.classify(), Some((8, 8)));
        assert_eq!(mobile_workers(with_capacity.classify()), 7);

        // Without cpu_capacity, the frequency rule agrees (3.53 / 4.32 = 82 %).
        let freq_only: Vec<_> = cpus.iter().map(|cpu| Cpu { capacity: None, ..cpu.clone() }).collect();
        assert_eq!(mobile_workers(FakeSysfs::new("elite-freq", &freq_only).classify()), 7);
    }

    #[test]
    fn three_cluster_phone() {
        // 4 little + 3 mid + 1 prime, laid out as GKI 6.1+ kernels do (package 0, core_id
        // restarting per cluster), which only the sibling lists tell apart.
        let mut cpus = cluster(0, 4, Some(260), Some(1_950_000));
        cpus.extend(cluster(4, 3, Some(640), Some(2_600_000)));
        cpus.extend(cluster(7, 1, Some(1024), Some(3_100_000)));
        let sysfs = FakeSysfs::new("4-3-1", &cpus);
        assert_eq!(sysfs.classify(), Some((8, 4)));
        assert_eq!(mobile_workers(sysfs.classify()), 3);

        let freq_only: Vec<_> = cpus.iter().map(|cpu| Cpu { capacity: None, ..cpu.clone() }).collect();
        assert_eq!(FakeSysfs::new("4-3-1-freq", &freq_only).classify(), Some((8, 4)));
    }

    #[test]
    fn four_plus_four_phone() {
        let mut cpus = cluster(0, 4, Some(380), Some(1_800_000));
        cpus.extend(cluster(4, 4, Some(1024), Some(2_800_000)));
        let sysfs = FakeSysfs::new("4-4", &cpus);
        assert_eq!(sysfs.classify(), Some((8, 4)));
        assert_eq!(mobile_workers(sysfs.classify()), 3);
    }

    #[test]
    fn each_cpu_is_classified_from_its_own_data() {
        // A big core with no cpufreq (hotplugged off) still counts; a little core missing
        // cpu_capacity is still recognized by its frequency.
        let mut cpus: Vec<_> = cluster(0, 4, None, Some(1_800_000));
        cpus.extend(cluster(4, 4, None, Some(2_800_000)));
        cpus[7].max_freq_khz = None;
        assert_eq!(mobile_workers(FakeSysfs::new("4-4-hotplug", &cpus).classify()), 3);

        let mut mixed = cluster(0, 4, Some(380), Some(1_800_000));
        mixed.extend(cluster(4, 4, Some(1024), Some(2_800_000)));
        mixed[0].capacity = None;
        assert_eq!(FakeSysfs::new("4-4-mixed", &mixed).classify(), Some((8, 4)));
    }

    #[test]
    fn smt_siblings_share_a_core() {
        // Intel hybrid: 6 P-cores with two threads each (two at a higher turbo bin), then 8
        // E-cores. Linux numbers the P-core threads 0–11 in sibling pairs.
        let mut cpus: Vec<Cpu> = (0..12)
            .map(|id| Cpu {
                siblings: Some(["0-1", "2-3", "4-5", "6-7", "8-9", "10-11"][id / 2].to_owned()),
                max_freq_khz: Some(if id < 4 { 5_800_000 } else { 5_400_000 }),
                ..Cpu::default()
            })
            .collect();
        cpus.extend(cluster(12, 8, None, Some(4_300_000)));
        assert_eq!(FakeSysfs::new("hybrid", &cpus).classify(), Some((14, 6)));

        // Older kernels: `thread_siblings_list` only, with comma-separated siblings.
        let legacy: Vec<Cpu> = (0..8)
            .map(|id| Cpu {
                legacy_siblings: Some(["0,4", "1,5", "2,6", "3,7"][id % 4].to_owned()),
                ..Cpu::default()
            })
            .collect();
        assert_eq!(FakeSysfs::new("legacy-smt", &legacy).classify(), Some((4, 4)));
    }

    #[test]
    fn missing_data_falls_back_to_all_performance() {
        assert_eq!(FakeSysfs::new("empty", &[]).classify(), None);
        assert_eq!(classify_sysfs(&read_sysfs(Path::new("/nonexistent/sapphire"))), None);
        // A VM: CPUs with no topology, capacity or cpufreq files.
        assert_eq!(FakeSysfs::new("vm", &vec![Cpu::default(); 4]).classify(), Some((4, 4)));
        let homogeneous = cluster(0, 8, None, Some(3_000_000));
        assert_eq!(FakeSysfs::new("homogeneous", &homogeneous).classify(), Some((8, 8)));
    }

    #[test]
    fn cpu_lists() {
        assert_eq!(lowest_cpu_in_list("3\n"), Some(3));
        assert_eq!(lowest_cpu_in_list("8-11,2"), Some(2));
        assert_eq!(lowest_cpu_in_list("0,4"), Some(0));
        assert_eq!(lowest_cpu_in_list(""), None);
        assert_eq!(lowest_cpu_in_list("garbage"), None);
    }

    #[test]
    fn windows_efficiency_classes() {
        assert_eq!(classify_efficiency_classes(&[]), None);
        // Homogeneous: every core reports class 0.
        assert_eq!(classify_efficiency_classes(&[0; 8]), Some((8, 8)));
        // Raptor Lake: 8 P-cores (class 1) and 16 E-cores (class 0).
        let raptor: Vec<u8> = [1u8; 8].into_iter().chain([0u8; 16]).collect();
        assert_eq!(classify_efficiency_classes(&raptor), Some((24, 8)));
    }
}
