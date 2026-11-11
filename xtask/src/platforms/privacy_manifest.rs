//! Apple's privacy manifest (`PrivacyInfo.xcprivacy`) and the required-reason APIs it must declare (M4 S2).
//!
//! Since 2024-05-01 App Store Connect refuses an upload whose code uses an API on Apple's required-reason
//! list without a reason for it in a privacy manifest (ITMS-91053). The list and its reasons are Apple's
//! "Describing use of required reason API" (developer.apple.com, read 2026-11-10, as S0's spike found it).
//! SDL, Luau and every crate link into the one executable, so there's no other bundle to hold a manifest:
//! the app's own, [`MANIFEST_FILE`], declares them all.
//!
//! [`Linked::scan`] reads what a built executable uses: its undefined symbols (`nm -u`), which are the C
//! functions, Foundation constants and Objective-C classes it takes from the system's libraries, and the
//! Objective-C selectors it sends (its `__objc_methname` section). [`check`] refuses an executable that
//! uses an API of a category the manifest doesn't declare, naming the APIs. A category declared but used
//! by nothing is reported and allowed: it costs nothing at review, but say why it's kept.
//!
//! The scan sees calls into the system, which is where every API on the list lives. Who in the link calls
//! one is a person's question, for when the check fails: S0's spike (`research_notes/Ship readiness for
//! M4/s0-spike.md` §3) reads each input rlib with `nm` to say.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::path::Path;
use std::process::Command;

use anyhow::{Context as _, bail, ensure};
use serde::Deserialize;

use crate::workspace;

/// The app's privacy manifest, which iOS packages carry at the `.app`'s root.
pub const MANIFEST_FILE: &str = "platform/ios/PrivacyInfo.xcprivacy";

/// A category of Apple's required-reason APIs.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Category {
    FileTimestamp,
    SystemBootTime,
    DiskSpace,
    ActiveKeyboards,
    UserDefaults,
}

impl Category {
    const ALL: [Self; 5] = [
        Self::FileTimestamp,
        Self::SystemBootTime,
        Self::DiskSpace,
        Self::ActiveKeyboards,
        Self::UserDefaults,
    ];

    /// Its `NSPrivacyAccessedAPIType`.
    pub fn key(self) -> &'static str {
        match self {
            Self::FileTimestamp => "NSPrivacyAccessedAPICategoryFileTimestamp",
            Self::SystemBootTime => "NSPrivacyAccessedAPICategorySystemBootTime",
            Self::DiskSpace => "NSPrivacyAccessedAPICategoryDiskSpace",
            Self::ActiveKeyboards => "NSPrivacyAccessedAPICategoryActiveKeyboards",
            Self::UserDefaults => "NSPrivacyAccessedAPICategoryUserDefaults",
        }
    }

    fn from_key(key: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|category| category.key() == key)
    }

    /// The reasons Apple accepts for it.
    fn reasons(self) -> &'static [&'static str] {
        match self {
            Self::FileTimestamp => &["DDA9.1", "C617.1", "3B52.1", "0A2A.1"],
            Self::SystemBootTime => &["35F9.1", "8FFB.1", "3D61.1"],
            Self::DiskSpace => &["85F4.1", "E174.1", "7D9E.1", "B728.1"],
            Self::ActiveKeyboards => &["3EC4.1", "54BD.1"],
            Self::UserDefaults => &["CA92.1", "1C8F.1", "C56D.1", "AC6B.1"],
        }
    }

    /// Its APIs as a link names them: C functions, Foundation's constants and Objective-C classes, without
    /// the leading underscore of their symbols. `getattrlist` and its kin are in two categories.
    fn symbols(self) -> &'static [&'static str] {
        match self {
            Self::FileTimestamp => &[
                "stat",
                "fstat",
                "fstatat",
                "lstat",
                "getattrlist",
                "fgetattrlist",
                "getattrlistat",
                "getattrlistbulk",
                "NSFileCreationDate",
                "NSFileModificationDate",
                "NSURLContentModificationDateKey",
                "NSURLCreationDateKey",
            ],
            Self::SystemBootTime => &["mach_absolute_time"],
            Self::DiskSpace => &[
                "statfs",
                "statvfs",
                "fstatfs",
                "fstatvfs",
                "getattrlist",
                "fgetattrlist",
                "getattrlistat",
                "NSFileSystemFreeSize",
                "NSFileSystemSize",
                "NSURLVolumeAvailableCapacityKey",
                "NSURLVolumeAvailableCapacityForImportantUsageKey",
                "NSURLVolumeAvailableCapacityForOpportunisticUsageKey",
                "NSURLVolumeTotalCapacityKey",
            ],
            Self::ActiveKeyboards => &[],
            Self::UserDefaults => &["OBJC_CLASS_$_NSUserDefaults"],
        }
    }

    /// Its Objective-C selectors: properties reached by message rather than through a linked symbol.
    fn selectors(self) -> &'static [&'static str] {
        match self {
            Self::FileTimestamp => &["fileModificationDate"],
            Self::SystemBootTime => &["systemUptime"],
            Self::ActiveKeyboards => &["activeInputModes"],
            Self::DiskSpace | Self::UserDefaults => &[],
        }
    }
}

/// What an executable takes from the system: its undefined symbols, without the leading underscore or a
/// `$` variant's suffix (`stat$INODE64`), and the Objective-C selectors it sends.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Linked {
    pub symbols: BTreeSet<String>,
    pub selectors: BTreeSet<String>,
}

impl Linked {
    /// Reads `executable` with Xcode's `nm` and `otool`.
    pub fn scan(executable: &Path) -> anyhow::Result<Self> {
        let symbols = workspace::output(Command::new("xcrun").args(["nm", "-u", "-j"]).arg(executable))?;
        let methnames = workspace::output(
            Command::new("xcrun").args(["otool", "-v", "-s", "__TEXT", "__objc_methname"]).arg(executable),
        )?;
        Ok(Self::parse(&symbols, &methnames))
    }

    /// From `nm -u -j`'s lines and `otool -v -s __TEXT __objc_methname`'s, whose lines after the header are
    /// an address and a selector.
    fn parse(symbols: &str, methnames: &str) -> Self {
        let symbols = symbols
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .map(|symbol| {
                let symbol = symbol.strip_prefix('_').unwrap_or(symbol);
                // Objective-C's class symbols hold a `$` of their own.
                let symbol = if symbol.starts_with("OBJC_") {
                    symbol
                } else {
                    symbol.split('$').next().unwrap_or(symbol)
                };
                symbol.to_owned()
            })
            .collect();
        let selectors = methnames
            .lines()
            .filter_map(|line| {
                let (address, selector) = line.trim().split_once(char::is_whitespace)?;
                address.chars().all(|c| c.is_ascii_hexdigit()).then(|| selector.trim().to_owned())
            })
            .collect();
        Self { symbols, selectors }
    }

    /// Each category the executable uses, with the APIs that use it.
    pub fn required_reason_apis(&self) -> BTreeMap<Category, Vec<&'static str>> {
        let mut used = BTreeMap::new();
        for category in Category::ALL {
            let symbols = category.symbols().iter().filter(|api| self.symbols.contains(**api));
            let selectors = category.selectors().iter().filter(|api| self.selectors.contains(**api));
            let apis: Vec<&'static str> = symbols.chain(selectors).copied().collect();
            if !apis.is_empty() {
                used.insert(category, apis);
            }
        }
        used
    }
}

/// A privacy manifest's declarations, as `plutil -convert json` writes the file.
#[derive(Debug, PartialEq, Eq)]
pub struct Manifest {
    pub tracking: bool,
    pub tracking_domains: Vec<String>,
    /// How many data types it says the app collects.
    pub collected_data_types: usize,
    /// Each category declared, with its reasons.
    pub accessed: BTreeMap<Category, Vec<String>>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ManifestFile {
    #[serde(rename = "NSPrivacyTracking")]
    tracking: bool,
    #[serde(rename = "NSPrivacyTrackingDomains", default)]
    tracking_domains: Vec<String>,
    #[serde(rename = "NSPrivacyCollectedDataTypes", default)]
    collected_data_types: Vec<serde_json::Value>,
    #[serde(rename = "NSPrivacyAccessedAPITypes", default)]
    accessed: Vec<AccessedApi>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct AccessedApi {
    #[serde(rename = "NSPrivacyAccessedAPIType")]
    category: String,
    #[serde(rename = "NSPrivacyAccessedAPITypeReasons")]
    reasons: Vec<String>,
}

impl Manifest {
    /// Reads a manifest through `plutil`, which also lints it.
    pub fn read(path: &Path) -> anyhow::Result<Self> {
        let json = workspace::output(Command::new("plutil").args(["-convert", "json", "-o", "-"]).arg(path))?;
        Self::parse(&json).with_context(|| format!("in {}", path.display()))
    }

    /// Refuses a category or reason Apple doesn't list, a category declared twice, and one with no reason.
    fn parse(json: &str) -> anyhow::Result<Self> {
        let file: ManifestFile = serde_json::from_str(json).context("not a privacy manifest")?;
        let mut accessed = BTreeMap::new();
        for api in file.accessed {
            let category = Category::from_key(&api.category)
                .with_context(|| format!("`{}` is not a required-reason API category", api.category))?;
            ensure!(!api.reasons.is_empty(), "{} gives no reason", api.category);
            for reason in &api.reasons {
                ensure!(
                    category.reasons().contains(&reason.as_str()),
                    "{reason} is not a reason for {}: Apple's are {}",
                    api.category,
                    category.reasons().join(", ")
                );
            }
            ensure!(accessed.insert(category, api.reasons).is_none(), "{} is declared twice", api.category);
        }
        Ok(Self {
            tracking: file.tracking,
            tracking_domains: file.tracking_domains,
            collected_data_types: file.collected_data_types.len(),
            accessed,
        })
    }
}

/// Refuses an executable that uses a required-reason API whose category `manifest` doesn't declare, and
/// otherwise returns a report: each category, its reasons and the APIs that use it.
pub fn check(manifest: &Manifest, linked: &Linked) -> anyhow::Result<String> {
    let used = linked.required_reason_apis();
    let undeclared: Vec<String> = used
        .iter()
        .filter(|(category, _)| !manifest.accessed.contains_key(category))
        .map(|(category, apis)| format!("{} ({})", category.key(), apis.join(", ")))
        .collect();
    if !undeclared.is_empty() {
        bail!(
            "the executable uses required-reason APIs that {MANIFEST_FILE} doesn't declare, and App Store \
             Connect refuses such an upload (ITMS-91053): {}. Declare each with the reason that's true of \
             the game, from Apple's \"Describing use of required reason API\"",
            undeclared.join("; ")
        );
    }
    let mut report = String::new();
    for (category, reasons) in &manifest.accessed {
        match used.get(category) {
            Some(apis) => {
                writeln!(report, "{}: {} for {}", category.key(), reasons.join(", "), apis.join(", "))
            }
            None => writeln!(
                report,
                "{}: {}, declared but used by nothing linked",
                category.key(),
                reasons.join(", ")
            ),
        }
        .expect("writing to a String succeeds");
    }
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The game's manifest as `plutil -convert json` gives it.
    const DECLARED: &str = r#"{
        "NSPrivacyTracking": false,
        "NSPrivacyTrackingDomains": [],
        "NSPrivacyCollectedDataTypes": [],
        "NSPrivacyAccessedAPITypes": [
            { "NSPrivacyAccessedAPIType": "NSPrivacyAccessedAPICategoryFileTimestamp",
              "NSPrivacyAccessedAPITypeReasons": ["C617.1"] },
            { "NSPrivacyAccessedAPIType": "NSPrivacyAccessedAPICategorySystemBootTime",
              "NSPrivacyAccessedAPITypeReasons": ["35F9.1"] }
        ]
    }"#;

    /// What S0's spike found the release device binary takes, among the rest of its imports.
    const SYMBOLS: &str = "_stat\n_fstat\n_fstatat\n_mach_absolute_time\n_malloc\n_OBJC_CLASS_$_NSBundle\n\
                           _objc_msgSend\n_clock_gettime\n";
    const METHNAMES: &str = "game_app:\nContents of (__TEXT,__objc_methname) section\n\
                             0000000100c0ee10  bundlePath\n0000000100c0ee1b  initWithFrame:\n";

    #[test]
    fn reads_what_the_executable_takes() {
        let linked =
            Linked::parse("_stat$INODE64\n_OBJC_CLASS_$_NSUserDefaults\n\n_mach_absolute_time\n", METHNAMES);
        assert_eq!(
            linked.symbols.iter().map(String::as_str).collect::<Vec<_>>(),
            ["OBJC_CLASS_$_NSUserDefaults", "mach_absolute_time", "stat"]
        );
        assert_eq!(
            linked.selectors.iter().map(String::as_str).collect::<Vec<_>>(),
            ["bundlePath", "initWithFrame:"]
        );
        let used = linked.required_reason_apis();
        assert_eq!(used[&Category::FileTimestamp], ["stat"]);
        assert_eq!(used[&Category::SystemBootTime], ["mach_absolute_time"]);
        assert_eq!(used[&Category::UserDefaults], ["OBJC_CLASS_$_NSUserDefaults"]);
    }

    #[test]
    fn passes_what_the_manifest_declares() {
        let manifest = Manifest::parse(DECLARED).unwrap();
        let report = check(&manifest, &Linked::parse(SYMBOLS, METHNAMES)).unwrap();
        assert_eq!(
            report,
            "NSPrivacyAccessedAPICategoryFileTimestamp: C617.1 for stat, fstat, fstatat\n\
             NSPrivacyAccessedAPICategorySystemBootTime: 35F9.1 for mach_absolute_time\n"
        );
        // A category linked by nothing passes, said so.
        let report = check(&manifest, &Linked::parse("_mach_absolute_time\n", "")).unwrap();
        assert!(report.contains("FileTimestamp: C617.1, declared but used by nothing linked"), "{report}");
    }

    /// The test S2's row asks for: a required-reason API the manifest doesn't declare fails the package,
    /// whether it's linked as a function, a constant, a class or sent as a selector.
    #[test]
    fn refuses_what_the_manifest_does_not_declare() {
        let manifest = Manifest::parse(DECLARED).unwrap();
        let cases = [
            ("_statvfs\n", "", "NSPrivacyAccessedAPICategoryDiskSpace (statvfs)"),
            (
                "_NSURLVolumeAvailableCapacityKey\n",
                "",
                "NSPrivacyAccessedAPICategoryDiskSpace (NSURLVolumeAvailableCapacityKey)",
            ),
            (
                "_OBJC_CLASS_$_NSUserDefaults\n",
                "",
                "NSPrivacyAccessedAPICategoryUserDefaults (OBJC_CLASS_$_NSUserDefaults)",
            ),
            (
                "",
                "0000000100c0ee10  activeInputModes\n",
                "NSPrivacyAccessedAPICategoryActiveKeyboards (activeInputModes)",
            ),
            // getattrlist is in two categories, and needs both.
            ("_getattrlist\n", "", "NSPrivacyAccessedAPICategoryDiskSpace (getattrlist)"),
        ];
        for (symbols, methnames, named) in cases {
            let linked = Linked::parse(&format!("{SYMBOLS}{symbols}"), &format!("{METHNAMES}{methnames}"));
            let error = check(&manifest, &linked).unwrap_err().to_string();
            assert!(error.contains(named), "{error}");
            assert!(error.contains("ITMS-91053"), "{error}");
        }
        // With nothing declared, both of the game's categories are named.
        let bare = Manifest::parse(r#"{ "NSPrivacyTracking": false }"#).unwrap();
        let error = check(&bare, &Linked::parse(SYMBOLS, METHNAMES)).unwrap_err().to_string();
        assert!(error.contains("FileTimestamp (stat, fstat, fstatat); NSPrivacyAccessedAPICategorySystemBootTime (mach_absolute_time)"), "{error}");
    }

    #[test]
    fn refuses_a_manifest_apple_would_not_read() {
        let manifest = |accessed: &str| {
            Manifest::parse(&format!(
                r#"{{ "NSPrivacyTracking": false, "NSPrivacyAccessedAPITypes": [{accessed}] }}"#
            ))
            .map(drop)
            .unwrap_err()
            .to_string()
        };
        let entry = |category: &str, reasons: &str| {
            format!(
                r#"{{ "NSPrivacyAccessedAPIType": "{category}", "NSPrivacyAccessedAPITypeReasons": [{reasons}] }}"#
            )
        };
        assert_eq!(
            manifest(&entry("NSPrivacyAccessedAPICategoryClock", r#""35F9.1""#)),
            "`NSPrivacyAccessedAPICategoryClock` is not a required-reason API category"
        );
        assert_eq!(
            manifest(&entry("NSPrivacyAccessedAPICategorySystemBootTime", r#""C617.1""#)),
            "C617.1 is not a reason for NSPrivacyAccessedAPICategorySystemBootTime: Apple's are 35F9.1, 8FFB.1, 3D61.1"
        );
        assert_eq!(
            manifest(&entry("NSPrivacyAccessedAPICategoryDiskSpace", "")),
            "NSPrivacyAccessedAPICategoryDiskSpace gives no reason"
        );
        let twice = entry("NSPrivacyAccessedAPICategoryDiskSpace", r#""E174.1""#);
        assert_eq!(
            manifest(&format!("{twice}, {twice}")),
            "NSPrivacyAccessedAPICategoryDiskSpace is declared twice"
        );
        let unknown =
            Manifest::parse(r#"{ "NSPrivacyTracking": false, "NSPrivacyTrackingDomain": [] }"#).unwrap_err();
        assert!(format!("{unknown:#}").contains("unknown field `NSPrivacyTrackingDomain`"), "{unknown:#}");
    }

    /// The committed manifest, read as `package` reads it: the categories S0 found the game linking and P4's
    /// free-space check (disk space), and nothing tracked or collected, which the store's privacy answers
    /// (S5) say.
    #[test]
    fn the_games_manifest_declares_what_it_links_and_collects_nothing() {
        if std::env::consts::OS != "macos" {
            return;
        }
        let manifest = Manifest::read(&workspace::root().join(MANIFEST_FILE)).unwrap();
        let committed = DECLARED.replace(
            r#""NSPrivacyAccessedAPITypeReasons": ["35F9.1"] }"#,
            r#""NSPrivacyAccessedAPITypeReasons": ["35F9.1"] },
            { "NSPrivacyAccessedAPIType": "NSPrivacyAccessedAPICategoryDiskSpace",
              "NSPrivacyAccessedAPITypeReasons": ["E174.1"] }"#,
        );
        assert_eq!(manifest, Manifest::parse(&committed).unwrap());
        assert!(!manifest.tracking);
        assert!(manifest.tracking_domains.is_empty());
        assert_eq!(manifest.collected_data_types, 0);
    }

    /// Each category's reasons and APIs are its own, and every API is spelled as a symbol is.
    #[test]
    fn the_tables_hold_together() {
        let mut reasons = BTreeSet::new();
        for category in Category::ALL {
            assert_eq!(Category::from_key(category.key()), Some(category));
            for reason in category.reasons() {
                assert!(reasons.insert(*reason), "{reason} is in two categories");
            }
            for api in category.symbols().iter().chain(category.selectors()) {
                assert!(!api.starts_with('_') && !api.contains(char::is_whitespace), "{api}");
            }
        }
        assert!(
            Category::ALL
                .iter()
                .all(|category| !category.symbols().is_empty() || !category.selectors().is_empty())
        );
    }
}
