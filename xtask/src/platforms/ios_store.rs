//! iOS's store package (M4 S2): the shipping app finished for the App Store, and the `.xcarchive` that
//! Xcode's Organizer and `xcodebuild -exportArchive` distribute, made with no Xcode project (m4-plan §3,
//! "iOS packaging").
//!
//! [`finish_app`] takes S1's assembled app, for a device or the Simulator, before it's signed:
//!
//! - Info.plist gets the keys Xcode writes when it builds: `CFBundleSupportedPlatforms`, the `DT*` keys and
//!   `BuildMachineOSBuild`, read from the installed Xcode and SDK and checked against the SDK the
//!   executable was linked with. Without `DTPlatformName` Xcode takes a hand-made archive for a macOS one
//!   (S0's spike). The template holds the store's other keys: `ITSAppUsesNonExemptEncryption` (false), the
//!   games category and `MinimumOSVersion`, which S1 checks against the executable.
//! - `PrivacyInfo.xcprivacy` goes at the app's root, and an executable that uses a required-reason API the
//!   manifest doesn't declare is refused ([`super::privacy_manifest`]).
//! - The app is signed ad hoc with [`ENTITLEMENTS_FILE`], then verified (`codesign --verify --strict
//!   --deep`) and its entitlements read back.
//!
//! [`archive`] then puts a device's app and its dSYM in `<Name>.xcarchive`, laid out as Organizer reads
//! one, and checks it offline: the layout, every plist (`plutil -lint`), the signature, the executable's
//! UUID against the dSYM's (`dwarfdump --uuid`), and line tables for SDL's and Luau's C as well as the
//! game's Rust in the dSYM, which the `release` profile's lacked (S0). What each check printed is kept in
//! the package's `checks/`.
//!
//! Nothing here signs for distribution, which needs the maintainer's team, certificate and profile.
//! `platform/ios/ExportOptions.plist` has the command that does it; S0 found Xcode takes such an archive
//! as far as signing.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context as _, bail, ensure};

use super::App;
use super::apple::{self, BundleLayout, check_command, plist_json, verify_signature};
use super::privacy_manifest::{self, Linked, Manifest};
use crate::version::Build;
use crate::workspace;

/// The entitlements the game's iOS app is signed with: none, since it uses no capability.
pub const ENTITLEMENTS_FILE: &str = "platform/ios/package.entitlements";

/// The App Store's category for games, which also makes iOS start Game Mode for the app.
const GAMES_CATEGORY: &str = "public.app-category.games";

/// The compiler Xcode names in `DTCompiler` for every clang build. SDL and Luau are clang's, through cc.
const DT_COMPILER: &str = "com.apple.compilers.llvm.clang.1_0";

/// Functions whose lines the dSYM must have: one from each C library whose lines the `release` profile's
/// dSYM lost (S0's spike), and one Rust function the game's crate compiled, each with the file its lines
/// are in.
const LINE_PROBES: [(Probe, &str); 3] = [
    (Probe::Symbol("SDL_GetTicksNS"), "SDL_timer.c"),
    (Probe::Symbol("lua_pcall"), "lapi.cpp"),
    (Probe::RustCrate("game_app"), ".rs"),
];

/// How a line probe finds its function in a symbol table.
#[derive(Clone, Copy, Debug)]
enum Probe {
    /// A C function, by name.
    Symbol(&'static str),
    /// The first function of a Rust crate, by its name in Rust's v0 mangling (`_8game_app`).
    RustCrate(&'static str),
}

impl Probe {
    fn matches(self, name: &str) -> bool {
        let Some(name) = name.strip_prefix('_') else { return false };
        match self {
            Self::Symbol(symbol) => name == symbol,
            Self::RustCrate(krate) => {
                name.starts_with("_R") && name.contains(&format!("_{}{krate}", krate.len()))
            }
        }
    }
}

/// Finishes the iOS app at `layout` for a store, before anything else signs it: the build's keys in its
/// Info.plist, the privacy manifest at its root, and an ad hoc signature with the game's entitlements,
/// verified. `device` is false for the Simulator's. What the checks print goes in `checks`.
pub fn finish_app(layout: &BundleLayout, app: &App, device: bool, checks: &Path) -> anyhow::Result<()> {
    fs::create_dir_all(checks).with_context(|| format!("failed to create {}", checks.display()))?;
    let executable = layout.executable_dir.join(&app.package);
    let toolchain = Toolchain::installed(if device { "iphoneos" } else { "iphonesimulator" })?;
    toolchain.check_linked(&executable)?;

    let keys = checks.join("store-keys.plist");
    let store_keys = toolchain.store_keys();
    write(&keys, &plist(&store_keys))?;
    let merge = format!("Merge \"{}\"", keys.display());
    workspace::run(Command::new("/usr/libexec/PlistBuddy").args(["-c", &merge]).arg(&layout.info_plist))?;
    fs::remove_file(&keys).with_context(|| format!("failed to remove {}", keys.display()))?;
    check_store_keys(&plist_json(&layout.info_plist)?, &store_keys)?;

    let manifest_path = layout.root.join("PrivacyInfo.xcprivacy");
    fs::copy(workspace::root().join(privacy_manifest::MANIFEST_FILE), &manifest_path)
        .with_context(|| format!("failed to copy {}", privacy_manifest::MANIFEST_FILE))?;
    let report = privacy_manifest::check(&Manifest::read(&manifest_path)?, &Linked::scan(&executable)?)?;
    write(&checks.join("required-reason-apis.txt"), &report)?;

    apple::sign_for_store(&layout.root, ENTITLEMENTS_FILE, apple::Runtime::Plain, checks)
}

/// Moves the device app `app_name` and its one dSYM (`dsyms`, paths in `stage`) from `stage` into
/// `<Name>.xcarchive` there, and checks the archive. Returns the archive's path in `stage`, and the dSYM's.
pub fn archive(
    app: &App,
    build: &Build,
    stage: &Path,
    app_name: &str,
    dsyms: &[String],
) -> anyhow::Result<(String, String)> {
    let [dsym] = dsyms else { bail!("an archive holds the app's one dSYM, and the build left {dsyms:?}") };
    let name = format!("{}.xcarchive", app.bundle_name);
    let root = stage.join(&name);
    workspace::remove_dir_if_exists(&root)?;
    let applications = root.join("Products/Applications");
    let dsyms_dir = root.join("dSYMs");
    for dir in [&applications, &dsyms_dir] {
        fs::create_dir_all(dir).with_context(|| format!("failed to create {}", dir.display()))?;
    }
    let archived_app = applications.join(app_name);
    rename(&stage.join(app_name), &archived_app)?;
    // Xcode names an app's dSYM after the app.
    let dsym_name = format!("{app_name}.dSYM");
    let archived_dsym = dsyms_dir.join(&dsym_name);
    rename(&stage.join(dsym), &archived_dsym)?;
    remove_dir_if_empty(stage.join(dsym).parent().context("a dSYM is in a directory")?)?;
    name_dsym_as_xcode_does(&archived_dsym, app)?;

    let executable = archived_app.join(&app.package);
    let architectures = workspace::output(Command::new("xcrun").args(["lipo", "-archs"]).arg(&executable))?;
    let info = ArchiveInfo {
        app_path: format!("Applications/{app_name}"),
        architectures: architectures.split_whitespace().map(str::to_owned).collect(),
        bundle_id: app.bundle_id.clone(),
        version: app.version.clone(),
        build: build.number,
        created: commit_date(&build.commit)?,
        name: app.bundle_name.clone(),
    };
    write(&root.join("Info.plist"), &plist(&info.entries()))?;

    let checks = stage.join("checks");
    check_layout(&root, &info, &app.package)?;
    lint_plists(&root, &checks)?;
    verify_signature(&archived_app, &checks)?;
    let dwarf = archived_dsym.join("Contents/Resources/DWARF").join(&app.package);
    check_uuids(&executable, &dwarf, &checks)?;
    check_lines(&dwarf, &checks)?;
    Ok((name.clone(), format!("{name}/dSYMs/{dsym_name}")))
}

/// Names what's inside a dSYM rustc made after the app, as Xcode names its own: rustc names its DWARF file
/// and relocations after the executable's build artifact (`game_app-1cd20dc7273c9c1f`), and its bundle
/// identifier after that, where Xcode uses the executable's name and the app's identifier. Symbolication
/// finds a dSYM by its UUID, which this leaves alone.
fn name_dsym_as_xcode_does(dsym: &Path, app: &App) -> anyhow::Result<()> {
    let resources = dsym.join("Contents/Resources");
    let only_file = |dir: &Path, extension: Option<&str>| -> anyhow::Result<PathBuf> {
        let files: Vec<PathBuf> = fs::read_dir(dir)
            .with_context(|| format!("failed to read {}", dir.display()))?
            .map(|entry| Ok(entry?.path()))
            .collect::<anyhow::Result<_>>()?;
        match &files[..] {
            [file] if file.extension().and_then(|ext| ext.to_str()) == extension => Ok(file.clone()),
            _ => bail!("{} holds {files:?}, where a dSYM holds one file", dir.display()),
        }
    };
    let dwarf_dir = resources.join("DWARF");
    rename(&only_file(&dwarf_dir, None)?, &dwarf_dir.join(&app.package))?;
    let relocations = resources.join("Relocations");
    if relocations.is_dir() {
        for arch in
            fs::read_dir(&relocations).with_context(|| format!("failed to read {}", relocations.display()))?
        {
            let arch = arch?.path();
            rename(&only_file(&arch, Some("yml"))?, &arch.join(format!("{}.yml", app.package)))?;
        }
    }
    let identifier = format!("com.apple.xcode.dsym.{}", app.bundle_id);
    workspace::run(
        Command::new("plutil")
            .args(["-replace", "CFBundleIdentifier", "-string", &identifier])
            .arg(dsym.join("Contents/Info.plist")),
    )
}

/// What Xcode and the SDK the app is built against say about themselves, for the keys Xcode writes into an
/// app's Info.plist.
#[derive(Debug, PartialEq, Eq)]
struct Toolchain {
    /// xcrun's name for the SDK, which is `DTPlatformName`: `iphoneos` or `iphonesimulator`.
    platform: &'static str,
    /// Its canonical name (`iphoneos26.2`), version and build.
    sdk_name: String,
    sdk_version: String,
    sdk_build: String,
    platform_version: String,
    /// Xcode's version (`26.2`) and build.
    xcode_version: String,
    xcode_build: String,
    /// The Mac's OS build.
    os_build: String,
}

impl Toolchain {
    fn installed(platform: &'static str) -> anyhow::Result<Self> {
        let xcrun = |flag: &str| -> anyhow::Result<String> {
            Ok(workspace::output(Command::new("xcrun").args(["--sdk", platform, flag]))?.trim().to_owned())
        };
        let settings = PathBuf::from(xcrun("--show-sdk-path")?).join("SDKSettings.plist");
        let sdk_name = workspace::output(
            Command::new("plutil").args(["-extract", "CanonicalName", "raw", "-o", "-"]).arg(&settings),
        )?;
        let xcode = workspace::output(Command::new("xcodebuild").arg("-version"))?;
        let (xcode_version, xcode_build) = parse_xcode_version(&xcode)?;
        Ok(Self {
            platform,
            sdk_name: sdk_name.trim().to_owned(),
            sdk_version: xcrun("--show-sdk-version")?,
            sdk_build: xcrun("--show-sdk-build-version")?,
            platform_version: xcrun("--show-sdk-platform-version")?,
            xcode_version,
            xcode_build,
            os_build: workspace::output(Command::new("sw_vers").arg("-buildVersion"))?.trim().to_owned(),
        })
    }

    /// Fails unless `executable` was linked for this platform against this SDK (`LC_BUILD_VERSION`), so
    /// the keys say what built it.
    fn check_linked(&self, executable: &Path) -> anyhow::Result<()> {
        let build = workspace::output(Command::new("xcrun").args(["vtool", "-show-build"]).arg(executable))?;
        let field = |name: &str| {
            build
                .lines()
                .find_map(|line| Some(line.trim().strip_prefix(name)?.trim().to_owned()))
                .with_context(|| {
                    format!("`vtool -show-build` reported no {name}for {}", executable.display())
                })
        };
        let (platform, sdk) = (field("platform ")?, field("sdk ")?);
        let expected = if self.platform == "iphoneos" { "IOS" } else { "IOSSIMULATOR" };
        ensure!(platform == expected, "{} is built for {platform}, not {expected}", executable.display());
        ensure!(
            sdk == self.sdk_version,
            "{} was linked against the {sdk} SDK, but the installed {} SDK is {}: rebuild it",
            executable.display(),
            self.platform,
            self.sdk_version
        );
        Ok(())
    }

    /// The keys Xcode writes into an app's Info.plist when it builds one.
    fn store_keys(&self) -> Vec<(&'static str, Value)> {
        let supported = if self.platform == "iphoneos" { "iPhoneOS" } else { "iPhoneSimulator" };
        vec![
            ("BuildMachineOSBuild", Value::string(&self.os_build)),
            ("CFBundleSupportedPlatforms", Value::Array(vec![Value::string(supported)])),
            ("DTCompiler", Value::string(DT_COMPILER)),
            ("DTPlatformBuild", Value::string(&self.sdk_build)),
            ("DTPlatformName", Value::string(self.platform)),
            ("DTPlatformVersion", Value::string(&self.platform_version)),
            ("DTSDKBuild", Value::string(&self.sdk_build)),
            ("DTSDKName", Value::string(&self.sdk_name)),
            ("DTXcode", Value::String(dt_xcode(&self.xcode_version))),
            ("DTXcodeBuild", Value::string(&self.xcode_build)),
        ]
    }
}

/// `xcodebuild -version`'s Xcode version and build.
fn parse_xcode_version(text: &str) -> anyhow::Result<(String, String)> {
    let line = |prefix: &str| {
        text.lines()
            .find_map(|line| line.strip_prefix(prefix))
            .map(|value| value.trim().to_owned())
            .with_context(|| format!("`xcodebuild -version` printed no `{prefix}` line: {text}"))
    };
    let version = line("Xcode ")?;
    ensure!(
        version.split('.').all(|part| !part.is_empty() && part.bytes().all(|b| b.is_ascii_digit())),
        "`xcodebuild -version` printed Xcode `{version}`, not a version"
    );
    Ok((version, line("Build version ")?))
}

/// `DTXcode` for an Xcode version, as Xcode writes it: the major version, then the minor and the patch
/// as a digit each (`26.2` is `2620`, `16.0.1` is `1601`).
fn dt_xcode(version: &str) -> String {
    let mut parts = version.split('.');
    let major = parts.next().unwrap_or("0");
    let minor = parts.next().unwrap_or("0");
    let patch = parts.next().unwrap_or("0");
    format!("{major:0>2}{minor}{patch}")
}

/// Fails unless the Info.plist (as JSON) holds every key `finish_app` merged with its value, and the
/// store's keys from the template.
fn check_store_keys(info: &serde_json::Value, keys: &[(&str, Value)]) -> anyhow::Result<()> {
    for (key, value) in keys {
        ensure!(
            info.get(key) == Some(&value.json()),
            "Info.plist's {key} is {:?}, not {}",
            info.get(key),
            value.json()
        );
    }
    let wanted = [
        ("ITSAppUsesNonExemptEncryption", serde_json::Value::Bool(false)),
        ("LSApplicationCategoryType", GAMES_CATEGORY.into()),
    ];
    for (key, value) in wanted {
        ensure!(info.get(key) == Some(&value), "Info.plist's {key} is {:?}, not {value}", info.get(key));
    }
    for key in ["MinimumOSVersion", "CFBundleIcons", "UILaunchScreen"] {
        ensure!(info.get(key).is_some(), "Info.plist has no {key}");
    }
    Ok(())
}

/// The archive's own Info.plist: what Organizer lists it by.
#[derive(Clone, Debug, PartialEq, Eq)]
struct ArchiveInfo {
    /// The app, from `Products/`.
    app_path: String,
    architectures: Vec<String>,
    bundle_id: String,
    version: String,
    build: u32,
    /// When the archive's commit was made, which keeps the archive the same however often it's made.
    created: String,
    name: String,
}

impl ArchiveInfo {
    fn entries(&self) -> Vec<(&'static str, Value)> {
        let properties = vec![
            ("ApplicationPath", Value::string(&self.app_path)),
            (
                "Architectures",
                Value::Array(self.architectures.iter().map(|arch| Value::string(arch)).collect()),
            ),
            ("CFBundleIdentifier", Value::string(&self.bundle_id)),
            ("CFBundleShortVersionString", Value::string(&self.version)),
            ("CFBundleVersion", Value::String(self.build.to_string())),
        ];
        vec![
            ("ApplicationProperties", Value::Dict(properties)),
            ("ArchiveVersion", Value::Integer(2)),
            ("CreationDate", Value::Date(self.created.clone())),
            ("Name", Value::string(&self.name)),
            ("SchemeName", Value::string(&self.name)),
        ]
    }
}

/// When `commit` was made, in UTC, as a plist date.
fn commit_date(commit: &str) -> anyhow::Result<String> {
    let date =
        workspace::output(Command::new("git").current_dir(workspace::root()).env("TZ", "UTC").args([
            "show",
            "-s",
            "--format=%cd",
            "--date=format-local:%Y-%m-%dT%H:%M:%SZ",
            commit,
        ]))?;
    Ok(date.trim().to_owned())
}

/// Fails unless `root` is laid out as Organizer reads an archive (S0's spike): its Info.plist naming the
/// app under `Products/`, which holds the executable, and the app's dSYM under `dSYMs/`, and nothing else.
fn check_layout(root: &Path, info: &ArchiveInfo, executable: &str) -> anyhow::Result<()> {
    // JSON has no dates, so `plutil` reads the plist key by key.
    let plist = root.join("Info.plist");
    let extract = |key: &str, format: &str| {
        workspace::output(Command::new("plutil").args(["-extract", key, format, "-o", "-"]).arg(&plist))
    };
    let expected = plist_value(&info.entries());
    let properties: serde_json::Value = serde_json::from_str(&extract("ApplicationProperties", "json")?)?;
    ensure!(
        properties == expected["ApplicationProperties"],
        "the archive's ApplicationProperties read back as {properties}"
    );
    for key in ["ArchiveVersion", "CreationDate", "Name", "SchemeName"] {
        let read = extract(key, "raw")?;
        let wanted = match &expected[key] {
            serde_json::Value::String(text) => text.clone(),
            other => other.to_string(),
        };
        ensure!(read.trim() == wanted, "the archive's {key} reads back as {}, not {wanted}", read.trim());
    }
    let mut top: Vec<String> = fs::read_dir(root)
        .with_context(|| format!("failed to read {}", root.display()))?
        .map(|entry| Ok(entry?.file_name().to_string_lossy().into_owned()))
        .collect::<anyhow::Result<_>>()?;
    top.sort();
    ensure!(top == ["Info.plist", "Products", "dSYMs"], "the archive holds {top:?}");
    let app = root.join("Products").join(&info.app_path);
    ensure!(app.join(executable).is_file(), "the archive has no {}", app.join(executable).display());
    let app_name = app.file_name().context("the app has a name")?.to_string_lossy();
    let dwarf = root.join("dSYMs").join(format!("{app_name}.dSYM/Contents/Resources/DWARF")).join(executable);
    ensure!(dwarf.is_file(), "the archive has no {}", dwarf.display());
    Ok(())
}

/// Lints every plist in the archive (Info.plists, the privacy manifest, the strings files) and the
/// entitlements read back from its signature, writing what `plutil` said to `checks/plists.txt`.
fn lint_plists(root: &Path, checks: &Path) -> anyhow::Result<()> {
    let mut plists = vec![checks.join("entitlements.plist")];
    find_plists(root, &mut plists)?;
    plists.sort();
    let mut lint = Command::new("plutil");
    lint.arg("-lint").args(&plists);
    check_command(&mut lint, &checks.join("plists.txt"))
}

fn find_plists(dir: &Path, out: &mut Vec<PathBuf>) -> anyhow::Result<()> {
    for entry in fs::read_dir(dir).with_context(|| format!("failed to read {}", dir.display()))? {
        let path = entry?.path();
        if path.is_dir() {
            find_plists(&path, out)?;
        } else if path
            .extension()
            .is_some_and(|ext| ["plist", "xcprivacy", "strings"].contains(&&*ext.to_string_lossy()))
        {
            out.push(path);
        }
    }
    Ok(())
}

/// Fails unless the executable's UUIDs (one per architecture) are its dSYM's: a dSYM from another link,
/// even of the same source, symbolicates nothing.
fn check_uuids(executable: &Path, dwarf: &Path, checks: &Path) -> anyhow::Result<()> {
    let uuids = |file: &Path| -> anyhow::Result<Vec<(String, String)>> {
        let text = workspace::output(Command::new("xcrun").args(["dwarfdump", "--uuid"]).arg(file))?;
        let uuids = parse_uuids(&text);
        ensure!(!uuids.is_empty(), "`dwarfdump --uuid` found no UUID in {}", file.display());
        Ok(uuids)
    };
    let (built, symbols) = (uuids(executable)?, uuids(dwarf)?);
    ensure!(built == symbols, "the executable's UUIDs {built:?} aren't its dSYM's {symbols:?}");
    let lines: String =
        built.iter().map(|(uuid, arch)| format!("{uuid} ({arch}): executable and dSYM\n")).collect();
    write(&checks.join("uuids.txt"), &lines)
}

/// `dwarfdump --uuid`'s `UUID: <uuid> (<arch>) <path>` lines, as (UUID, architecture), sorted.
fn parse_uuids(text: &str) -> Vec<(String, String)> {
    let mut uuids: Vec<(String, String)> = text
        .lines()
        .filter_map(|line| {
            let rest = line.trim().strip_prefix("UUID: ")?;
            let (uuid, rest) = rest.split_once(' ')?;
            let arch = rest.strip_prefix('(')?.split_once(')')?.0;
            Some((uuid.to_owned(), arch.to_owned()))
        })
        .collect();
    uuids.sort();
    uuids
}

/// Fails unless the dSYM gives a file and line for each of [`LINE_PROBES`], looked up by `atos` at the
/// address the dSYM's own symbol table gives it, so it holds whether or not the app's executable is
/// stripped.
fn check_lines(dwarf: &Path, checks: &Path) -> anyhow::Result<()> {
    let symbols = workspace::output(Command::new("xcrun").args(["nm", "--defined-only"]).arg(dwarf))?;
    let mut lines = String::new();
    for (probe, file) in LINE_PROBES {
        let (name, address) = function(&symbols, probe).with_context(|| {
            format!("the dSYM's symbol table has no function for {probe:?}: pick another probe")
        })?;
        let found = workspace::output(
            Command::new("xcrun")
                .args(["atos", "-arch", "arm64", "-o"])
                .arg(dwarf)
                .arg(format!("0x{address:x}")),
        )?;
        let found = found.trim();
        ensure!(
            has_line_in(found, file),
            "the dSYM has no line in {file} for {name}: atos says `{found}`. LTO's objects must still exist when \
             dsymutil runs, as split-debuginfo = \"packed\" sees to"
        );
        lines.push_str(&format!("{probe:?}: {found}\n"));
    }
    write(&checks.join("dsym-lines.txt"), &lines)
}

/// The first function in `nm`'s lines that `probe` matches: its symbol and address.
fn function(nm: &str, probe: Probe) -> Option<(&str, u64)> {
    nm.lines().find_map(|line| {
        let mut fields = line.split_whitespace();
        let (address, kind, name) = (fields.next()?, fields.next()?, fields.next()?);
        let code = kind.eq_ignore_ascii_case("t");
        (code && probe.matches(name)).then(|| Some((name, u64::from_str_radix(address, 16).ok()?))).flatten()
    })
}

/// Whether `atos`'s answer (`name (in binary) (file.c:123)`) names a line in a file ending in `file`.
fn has_line_in(atos: &str, file: &str) -> bool {
    let Some(location) = atos.rsplit_once('(').map(|(_, location)| location.trim_end_matches(')')) else {
        return false;
    };
    location.rsplit_once(':').is_some_and(|(path, line)| {
        path.ends_with(file) && !line.is_empty() && line.bytes().all(|b| b.is_ascii_digit())
    })
}

/// A value of a plist this module writes.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Value {
    String(String),
    Integer(i64),
    /// An ISO 8601 date in UTC, `2026-11-10T12:00:00Z`.
    Date(String),
    Array(Vec<Value>),
    Dict(Vec<(&'static str, Value)>),
}

impl Value {
    fn string(text: &str) -> Self {
        Self::String(text.to_owned())
    }

    /// As `plutil -convert json` writes it, dates aside.
    fn json(&self) -> serde_json::Value {
        match self {
            Self::String(text) | Self::Date(text) => text.as_str().into(),
            Self::Integer(value) => (*value).into(),
            Self::Array(values) => values.iter().map(Self::json).collect(),
            Self::Dict(entries) => plist_value(entries),
        }
    }

    fn write(&self, out: &mut String, depth: usize) {
        let indent = "\t".repeat(depth);
        match self {
            Self::String(text) => push_element(out, "string", text),
            Self::Date(date) => push_element(out, "date", date),
            Self::Integer(value) => push_element(out, "integer", &value.to_string()),
            Self::Array(values) => {
                out.push_str("<array>\n");
                for value in values {
                    out.push_str(&indent);
                    out.push('\t');
                    value.write(out, depth + 1);
                    out.push('\n');
                }
                out.push_str(&indent);
                out.push_str("</array>");
            }
            Self::Dict(entries) => {
                out.push_str("<dict>\n");
                for (key, value) in entries {
                    out.push_str(&indent);
                    out.push('\t');
                    push_element(out, "key", key);
                    out.push('\n');
                    out.push_str(&indent);
                    out.push('\t');
                    value.write(out, depth + 1);
                    out.push('\n');
                }
                out.push_str(&indent);
                out.push_str("</dict>");
            }
        }
    }
}

fn push_element(out: &mut String, tag: &str, text: &str) {
    out.push_str(&format!("<{tag}>"));
    apple::push_xml_escaped(out, text);
    out.push_str(&format!("</{tag}>"));
}

fn plist_value(entries: &[(&str, Value)]) -> serde_json::Value {
    entries
        .iter()
        .map(|(key, value)| ((*key).to_owned(), value.json()))
        .collect::<serde_json::Map<_, _>>()
        .into()
}

/// An XML property list whose top level is a dictionary of `entries`.
fn plist(entries: &[(&'static str, Value)]) -> String {
    let mut out = String::from(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \
         \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n<plist version=\"1.0\">\n",
    );
    Value::Dict(entries.to_vec()).write(&mut out, 0);
    out.push_str("\n</plist>\n");
    out
}

fn write(path: &Path, text: &str) -> anyhow::Result<()> {
    fs::write(path, text).with_context(|| format!("failed to write {}", path.display()))
}

fn rename(from: &Path, to: &Path) -> anyhow::Result<()> {
    fs::rename(from, to).with_context(|| format!("failed to move {} to {}", from.display(), to.display()))
}

/// Removes `dir` if nothing is left in it.
fn remove_dir_if_empty(dir: &Path) -> anyhow::Result<()> {
    if fs::read_dir(dir).with_context(|| format!("failed to read {}", dir.display()))?.next().is_none() {
        fs::remove_dir(dir).with_context(|| format!("failed to remove {}", dir.display()))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::platforms::test_game;
    use crate::version::Kind;

    fn toolchain(platform: &'static str) -> Toolchain {
        Toolchain {
            platform,
            sdk_name: format!("{platform}26.2"),
            sdk_version: "26.2".into(),
            sdk_build: "23C53".into(),
            platform_version: "26.2".into(),
            xcode_version: "26.2".into(),
            xcode_build: "17C52".into(),
            os_build: "26A434".into(),
        }
    }

    fn template() -> String {
        include_str!("../../../platform/ios/Info.plist").replace("\r\n", "\n")
    }

    #[test]
    fn reads_xcodes_version_as_it_writes_it() {
        let (version, build) = parse_xcode_version("Xcode 26.2\nBuild version 17C52\n").unwrap();
        assert_eq!((version.as_str(), build.as_str()), ("26.2", "17C52"));
        assert_eq!(dt_xcode(&version), "2620");
        assert_eq!(dt_xcode("16.0.1"), "1601");
        assert_eq!(dt_xcode("9.4"), "0940");
        assert_eq!(dt_xcode("27"), "2700");
        assert!(parse_xcode_version("Xcode beta\nBuild version 1\n").is_err());
        assert!(parse_xcode_version("Build version 17C52\n").is_err());
    }

    /// The keys a device's app gets, as S0's spike found Xcode needs them, and the Simulator's.
    #[test]
    fn writes_the_keys_xcode_writes() {
        let keys = toolchain("iphoneos").store_keys();
        let json = plist_value(&keys);
        assert_eq!(json["DTPlatformName"], "iphoneos");
        assert_eq!(json["DTSDKName"], "iphoneos26.2");
        assert_eq!(json["DTXcode"], "2620");
        assert_eq!(json["DTXcodeBuild"], "17C52");
        assert_eq!(json["DTSDKBuild"], "23C53");
        assert_eq!(json["CFBundleSupportedPlatforms"], serde_json::json!(["iPhoneOS"]));
        let simulator = plist_value(&toolchain("iphonesimulator").store_keys());
        assert_eq!(simulator["CFBundleSupportedPlatforms"], serde_json::json!(["iPhoneSimulator"]));
        assert_eq!(simulator["DTPlatformName"], "iphonesimulator");

        // The template holds none of them, so merging adds every one, and holds the store's own keys.
        let template = template();
        for (key, _) in &keys {
            assert!(!template.contains(&format!("<key>{key}</key>")), "the template has {key}");
        }
        assert!(template.contains("<key>ITSAppUsesNonExemptEncryption</key>\n\t<false/>"));
        assert_eq!(apple::plist_string(&template, "LSApplicationCategoryType"), Some(GAMES_CATEGORY));
    }

    #[test]
    fn checks_the_merged_keys() {
        let keys = toolchain("iphoneos").store_keys();
        let mut info = plist_value(&keys);
        let object = info.as_object_mut().unwrap();
        object.insert("ITSAppUsesNonExemptEncryption".into(), false.into());
        object.insert("LSApplicationCategoryType".into(), GAMES_CATEGORY.into());
        for key in ["MinimumOSVersion", "UILaunchScreen", "CFBundleIcons"] {
            object.insert(key.into(), "17.0".into());
        }
        check_store_keys(&info, &keys).unwrap();

        let mut wrong = info.clone();
        wrong["DTPlatformName"] = "macosx".into();
        let error = check_store_keys(&wrong, &keys).unwrap_err().to_string();
        assert_eq!(error, r#"Info.plist's DTPlatformName is Some(String("macosx")), not "iphoneos""#);
        let mut wrong = info.clone();
        wrong["ITSAppUsesNonExemptEncryption"] = true.into();
        assert!(check_store_keys(&wrong, &keys).is_err());
        let mut wrong = info;
        wrong.as_object_mut().unwrap().remove("CFBundleIcons");
        assert_eq!(
            check_store_keys(&wrong, &keys).unwrap_err().to_string(),
            "Info.plist has no CFBundleIcons"
        );
    }

    #[test]
    fn writes_plists_escaped_and_indented() {
        let text = plist(&[
            ("Name", Value::string("A & <B>")),
            ("List", Value::Array(vec![Value::Integer(2)])),
            ("Inner", Value::Dict(vec![("When", Value::Date("2026-11-10T12:00:00Z".into()))])),
        ]);
        assert!(text.contains("<dict>\n\t<key>Name</key>\n\t<string>A &amp; &lt;B&gt;</string>\n"), "{text}");
        assert!(
            text.contains("\t<key>List</key>\n\t<array>\n\t\t<integer>2</integer>\n\t</array>\n"),
            "{text}"
        );
        assert!(
            text.contains(
                "\t<dict>\n\t\t<key>When</key>\n\t\t<date>2026-11-10T12:00:00Z</date>\n\t</dict>\n"
            ),
            "{text}"
        );
        assert!(text.ends_with("</dict>\n</plist>\n"));
    }

    #[test]
    fn reads_uuids_and_lines() {
        let text = "UUID: 1A2B3C4D-0000-1111-2222-333344445555 (arm64) /x/SapphireGame.app/game_app\n";
        assert_eq!(
            parse_uuids(text),
            [("1A2B3C4D-0000-1111-2222-333344445555".to_owned(), "arm64".to_owned())]
        );
        assert!(parse_uuids("game_app: no UUID\n").is_empty());

        let nm = "0000000100000000 T __mh_execute_header\n0000000100a1b2c4 T _SDL_GetTicksNS\n\
                  0000000100c00000 d _SDL_GetTicks\n\
                  0000000100dc9f84 t __RINvMCs9NHbHcnAOmd_14sapphire_crashNtB3_11CrashConfig3newECsbDLP55hJppc_8game_app\n\
                  0000000100dc9f90 t __RNvCsbDLP55hJppc_17game_app_extra4main\n";
        assert_eq!(function(nm, Probe::Symbol("SDL_GetTicksNS")), Some(("_SDL_GetTicksNS", 0x1_00a1_b2c4)));
        assert_eq!(function(nm, Probe::Symbol("SDL_GetTicks")), None, "data, not a function");
        assert_eq!(
            function(nm, Probe::RustCrate("game_app")).map(|(_, address)| address),
            Some(0x1_00dc_9f84)
        );
        assert_eq!(function(nm, Probe::RustCrate("game_ap")), None);

        assert!(has_line_in("SDL_GetTicksNS (in game_app) (SDL_timer.c:612)", "SDL_timer.c"));
        assert!(has_line_in("rust_begin_unwind (in game_app) (panicking.rs:689)", ".rs"));
        // No line table: atos gives the offset into the function instead.
        assert!(!has_line_in("SDL_GetTicksNS (in game_app) + 12", "SDL_timer.c"));
        assert!(has_line_in("lua_pcall (in game_app) (lapi.cpp:1177)", "lapi.cpp"));
        assert!(!has_line_in("lua_pcall (in game_app) (ldo.cpp:12)", "lapi.cpp"));
        assert!(!has_line_in("0x100a1b2c4", ".rs"));
    }

    /// An archive as `archive` lays one out passes `check_layout`, through the real `plutil`; one with a
    /// stray file, a missing dSYM or another app path doesn't.
    #[test]
    fn checks_the_archives_layout() {
        if std::env::consts::OS != "macos" {
            return;
        }
        let scratch = std::env::temp_dir().join(format!("xtask-xcarchive-{}", std::process::id()));
        let root = scratch.join("SapphireGame.xcarchive");
        let app = test_game();
        let build = Build { number: 1178, kind: Kind::Release, commit: "0".into() };
        let info = ArchiveInfo {
            app_path: "Applications/SapphireGame.app".into(),
            architectures: vec!["arm64".into()],
            bundle_id: app.bundle_id.clone(),
            version: app.version.clone(),
            build: build.number,
            created: "2026-11-10T12:16:03Z".into(),
            name: app.bundle_name.clone(),
        };
        let lay_out = || {
            workspace::remove_dir_if_exists(&scratch).unwrap();
            let app_dir = root.join("Products/Applications/SapphireGame.app");
            let dwarf = root.join("dSYMs/SapphireGame.app.dSYM/Contents/Resources/DWARF");
            fs::create_dir_all(&app_dir).unwrap();
            fs::create_dir_all(&dwarf).unwrap();
            fs::write(app_dir.join("game_app"), "").unwrap();
            fs::write(dwarf.join("game_app"), "").unwrap();
            write(&root.join("Info.plist"), &plist(&info.entries())).unwrap();
        };
        lay_out();
        workspace::run(Command::new("plutil").arg("-lint").arg(root.join("Info.plist"))).unwrap();
        check_layout(&root, &info, "game_app").unwrap();
        let date = workspace::output(
            Command::new("plutil")
                .args(["-extract", "CreationDate", "raw", "-o", "-"])
                .arg(root.join("Info.plist")),
        )
        .unwrap();
        assert_eq!(date.trim(), "2026-11-10T12:16:03Z", "a date, read back as written");
        let changed = ArchiveInfo { build: 1179, ..info.clone() };
        assert!(check_layout(&root, &changed, "game_app").unwrap_err().to_string().contains("1178"));

        fs::write(root.join("stray"), "").unwrap();
        assert!(check_layout(&root, &info, "game_app").unwrap_err().to_string().contains("\"stray\""));
        lay_out();
        workspace::remove_dir_if_exists(&root.join("dSYMs/SapphireGame.app.dSYM")).unwrap();
        assert!(check_layout(&root, &info, "game_app").unwrap_err().to_string().contains("has no"));
        lay_out();
        let other = ArchiveInfo { app_path: "Applications/Other.app".into(), ..info };
        assert!(check_layout(&root, &other, "game_app").is_err());
        workspace::remove_dir_if_exists(&scratch).unwrap();
    }

    /// The committed entitlements grant nothing: the game uses no capability, and a key here needs one on
    /// the App ID, which only the maintainer's account can add.
    #[test]
    fn the_game_is_signed_with_no_entitlement() {
        if std::env::consts::OS != "macos" {
            return;
        }
        let entitlements = plist_json(&workspace::root().join(ENTITLEMENTS_FILE)).unwrap();
        assert_eq!(entitlements, serde_json::json!({}));
        for file in ["platform/ios/PrivacyInfo.xcprivacy", "platform/ios/ExportOptions.plist"] {
            workspace::run(Command::new("plutil").arg("-lint").arg(workspace::root().join(file))).unwrap();
        }
        let export = plist_json(&workspace::root().join("platform/ios/ExportOptions.plist")).unwrap();
        assert_eq!(export["method"], "app-store-connect");
        assert_eq!(export["manageAppVersionAndBuildNumber"], false, "the build number is git's");
    }
}
