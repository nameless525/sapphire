//! The store's Android App Bundle, checked offline (M4 S3): what Play's own tools say of it, run here
//! with no account, no device and no download.
//!
//! `xtask package --platform android` checks the AAB Gradle's `bundleRelease` made, and publishes no
//! package that fails a check:
//!
//! - `bundletool validate`, and what the bundle holds: one module, the base, with the library for
//!   arm64-v8a alone, the cook under `assets/content/`, and the library's symbols for Play's crash
//!   reports in `BUNDLE-METADATA` (AGP's `debugSymbolLevel`);
//! - the library's `LOAD` segments aligned to 16 KB, for devices with 16 KB pages (spec §6.7);
//! - `bundletool build-apks --local-testing`: the APKs Play would make from it, which an emulator
//!   installs with `bundletool install-apks`. Every one passes `zipalign -c -P 16`, and the most any device
//!   downloads (`get-size total`) is under Play's cap ([`DOWNLOAD_CAP`]);
//! - `aapt2 dump badging` of the base and the library's APKs: the identity, version, SDK levels and
//!   launcher activity xtask built, not debuggable, and the one ABI;
//! - the bundle's signer, read with `keytool`: the upload key when the environment names one, the debug
//!   key otherwise.
//!
//! bundletool is AGP's own copy: Gradle's `bundletoolClasspath` task lists the plugin's classpath, which it
//! resolved from its cache to load AGP. `aapt2` and `zipalign` are the SDK's build tools, `keytool` the
//! JDK's, and `unzip` reads the archives.

use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context as _, bail, ensure};
use serde_json::json;

use crate::workspace;

/// Play's cap on the base module's compressed download: 500 MB, of 4 GB for everything installed with the
/// app (Play Console Help, "Maximum size limits", support.google.com/googleplay/android-developer/answer/9859372,
/// read 2026-11-10; the older 200 MB is now only where Play asks before downloading on mobile data). The
/// game ships its cook in the base and no asset packs (m4-plan §3, "Where content lives").
pub const DOWNLOAD_CAP: u64 = 500 * 1000 * 1000;
/// The page size Play requires native libraries to load at: 16 KB.
const PAGE: u64 = 16 * 1024;
/// bundletool's entry point, in AGP's copy as in Google's own jar.
const BUNDLETOOL_MAIN: &str = "com.android.tools.build.bundletool.BundleToolMain";
/// The environment variable naming the upload key's keystore (`app/build.gradle.kts` reads it too).
pub const UPLOAD_KEYSTORE: &str = "SAPPHIRE_UPLOAD_KEYSTORE";

/// The key a bundle is signed with.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Key {
    /// The maintainer's upload key, from [`UPLOAD_KEYSTORE`]: what Play takes.
    Upload,
    /// The SDK's debug key: every local check takes it, and Play refuses it.
    Debug,
}

impl Key {
    /// The one the store build signs with: the upload key if the environment names it.
    pub fn from_env() -> Self {
        if std::env::var_os(UPLOAD_KEYSTORE).is_some() { Self::Upload } else { Self::Debug }
    }

    fn name(self) -> &'static str {
        match self {
            Self::Upload => "upload",
            Self::Debug => "debug",
        }
    }
}

/// What the bundle must say it is: the app xtask built.
pub struct Expected<'a> {
    pub package: &'a str,
    pub version_code: u32,
    pub version_name: &'a str,
    pub min_sdk: u32,
    pub target_sdk: u32,
    /// The launcher activity's class, as the manifest names it.
    pub activity: String,
    pub abi: &'a str,
    /// The library's file name, `lib<package>.so`.
    pub library: String,
    pub key: Key,
}

/// The tools the checks run.
pub struct Tools {
    java: PathBuf,
    keytool: PathBuf,
    /// bundletool's classpath, joined for `java -cp`.
    classpath: OsString,
    build_tools: PathBuf,
}

impl Tools {
    /// The JDK Gradle runs on (`JAVA_HOME`, else `PATH`'s), bundletool on the classpath listed in
    /// `classpath_list`, one file per line, and the newest build tools in `sdk`.
    pub fn locate(sdk: &Path, classpath_list: &Path) -> anyhow::Result<Self> {
        let jdk = |tool: &str| match std::env::var_os("JAVA_HOME").filter(|home| !home.is_empty()) {
            Some(home) => Path::new(&home).join("bin").join(tool),
            None => tool.into(),
        };
        let list = fs::read_to_string(classpath_list).with_context(|| {
            format!("failed to read bundletool's classpath from {}", classpath_list.display())
        })?;
        let jars: Vec<&str> = list.lines().filter(|line| !line.is_empty()).collect();
        ensure!(
            jars.iter().any(|jar| file_name(jar).starts_with("bundletool-")),
            "{} lists no bundletool jar: AGP's classpath has changed",
            classpath_list.display()
        );
        let classpath = std::env::join_paths(&jars).context("a jar's path holds the path separator")?;

        let dir = sdk.join("build-tools");
        let names = fs::read_dir(&dir).into_iter().flatten().flatten().map(|entry| entry.file_name());
        let names: Vec<String> = names.filter_map(|name| name.into_string().ok()).collect();
        let newest = super::android::newest_version(names.iter().map(String::as_str))
            .with_context(|| format!("no build tools in {}", dir.display()))?;
        Ok(Self { java: jdk("java"), keytool: jdk("keytool"), classpath, build_tools: dir.join(newest) })
    }

    fn bundletool(&self) -> Command {
        let mut command = Command::new(&self.java);
        command.arg("-cp").arg(&self.classpath).arg(BUNDLETOOL_MAIN);
        command
    }

    fn build_tool(&self, name: &str) -> Command {
        Command::new(self.build_tools.join(name))
    }
}

/// What the checks found, for `package.json` and the files beside the bundle.
pub struct Checked {
    /// The APKs Play would serve, as `build-apks --local-testing` made them.
    pub apks: PathBuf,
    /// `aapt2 dump badging` of the base APK.
    pub badging: String,
    pub key: Key,
    pub certificate: Certificate,
    /// The fewest and most bytes a device downloads.
    pub download: (u64, u64),
    /// The library's symbols, inside the bundle.
    pub symbols: String,
}

impl Checked {
    /// The checks' summary, for `package.json`.
    pub fn summary(&self, apks: &str, badging: &str, lint: &str) -> serde_json::Value {
        json!({
            "signed_with": self.key.name(),
            "certificate": { "owner": self.certificate.owner, "sha256": self.certificate.sha256 },
            "download_bytes": { "min": self.download.0, "max": self.download.1 },
            "symbols_in_bundle": self.symbols,
            "local_testing_apks": apks,
            "badging": badging,
            "lint": lint,
        })
    }
}

/// Checks `aab` against `expected` with `tools`, working in `work`.
pub fn check(aab: &Path, expected: &Expected<'_>, tools: &Tools, work: &Path) -> anyhow::Result<Checked> {
    workspace::remove_dir_if_exists(work)?;
    fs::create_dir_all(work).with_context(|| format!("failed to create {}", work.display()))?;

    quiet(tools.bundletool().arg("validate").arg(arg("--bundle=", aab)))?;
    let entries = workspace::output(Command::new("unzip").arg("-Z1").arg(aab))?;
    let symbols = check_contents(entries.lines(), expected)?;
    let library = format!("base/lib/{}/{}", expected.abi, expected.library);
    let elf = Command::new("unzip").arg("-p").arg(aab).arg(&library).output();
    let elf = elf.with_context(|| format!("failed to read {library} from {}", aab.display()))?;
    ensure!(elf.status.success(), "failed to read {library} from {}", aab.display());
    check_page_alignment(&load_segments(&elf.stdout).with_context(|| format!("in {library}"))?)
        .with_context(|| format!("{library} can't load on devices with 16 KB pages"))?;
    eprintln!("checked {}: validated, one base module, 16 KB segments, symbols in {symbols}", aab.display());

    // Signed with the debug key, as `--local-testing` is for an emulator: Play signs what it serves.
    let apks = work.join("local-testing.apks");
    quiet(
        tools
            .bundletool()
            .arg("build-apks")
            .arg(arg("--bundle=", aab))
            .arg(arg("--output=", &apks))
            .arg(arg("--aapt2=", &tools.build_tools.join("aapt2")))
            .args(["--local-testing", "--overwrite"]),
    )?;
    let size = workspace::output(tools.bundletool().arg("get-size").arg("total").arg(arg("--apks=", &apks)))?;
    let download = download_size(&size)?;
    ensure!(
        download.1 < DOWNLOAD_CAP,
        "a device downloads up to {} bytes of it, over Play's cap of {DOWNLOAD_CAP}",
        download.1
    );

    let unpacked = work.join("apks");
    quiet(Command::new("unzip").arg("-q").arg(&apks).arg("-d").arg(&unpacked))?;
    let splits = unpacked.join("splits");
    let mut names: Vec<String> = fs::read_dir(&splits)
        .with_context(|| format!("{} made no splits", apks.display()))?
        .flatten()
        .filter_map(|entry| entry.file_name().into_string().ok())
        .filter(|name| name.ends_with(".apk"))
        .collect();
    names.sort_unstable();
    for name in &names {
        quiet(tools.build_tool("zipalign").args(["-c", "-P", "16", "4"]).arg(splits.join(name)))
            .with_context(|| format!("{name} isn't aligned for 16 KB pages"))?;
    }
    let abi_split = format!("base-{}.apk", expected.abi.replace('-', "_"));
    ensure!(names.contains(&abi_split), "no {abi_split} among {}'s splits: {names:?}", apks.display());
    let dump = |name: &str| {
        workspace::output(tools.build_tool("aapt2").args(["dump", "badging"]).arg(splits.join(name)))
    };
    let badging = dump("base-master.apk")?;
    Badging::parse(&badging)?.check(expected).context("in base-master.apk's badging")?;
    let native = Badging::parse(&dump(&abi_split)?)?.native_code;
    ensure!(
        native == [expected.abi],
        "{abi_split} holds native code for {native:?}, not just {}",
        expected.abi
    );
    eprintln!(
        "checked {}: {} APKs, all aligned for 16 KB pages, {}–{} bytes to download; badging as built",
        apks.display(),
        names.len(),
        download.0,
        download.1
    );

    let printed = workspace::output(Command::new(&tools.keytool).arg("-printcert").arg("-jarfile").arg(aab))?;
    let certificate = Certificate::parse(&printed)?;
    ensure!(
        certificate.is_debug() == (expected.key == Key::Debug),
        "{} is signed by {}, but the {} key was to sign it",
        aab.display(),
        certificate.owner,
        expected.key.name()
    );
    Ok(Checked { apks, badging, key: expected.key, certificate, download, symbols })
}

/// `--name=<path>`, as bundletool takes paths.
fn arg(name: &str, path: &Path) -> OsString {
    let mut arg = OsString::from(name);
    arg.push(path);
    arg
}

/// Runs `command` and fails with what it printed, keeping it out of the log when it succeeds.
fn quiet(command: &mut Command) -> anyhow::Result<()> {
    let line = workspace::shell_line(command);
    let output = command.output().with_context(|| format!("failed to start `{line}`"))?;
    if !output.status.success() {
        bail!(
            "`{line}` failed ({}):\n{}{}",
            output.status,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }
    Ok(())
}

fn file_name(path: &str) -> &str {
    path.rsplit(['/', '\\']).next().unwrap_or(path)
}

/// Checks the bundle's entries: the base module alone, holding the library for one ABI and the cook, and
/// the library's symbols in its metadata, which it returns the path of.
fn check_contents<'a>(
    entries: impl Iterator<Item = &'a str>,
    expected: &Expected<'_>,
) -> anyhow::Result<String> {
    let library = format!("base/lib/{}/{}", expected.abi, expected.library);
    let symbols = format!(
        "BUNDLE-METADATA/com.android.tools.build.debugsymbols/{}/{}.dbg",
        expected.abi, expected.library
    );
    let (mut has_library, mut has_symbols, mut has_content) = (false, false, false);
    for entry in entries.filter(|entry| !entry.ends_with('/')) {
        let (top, rest) = entry.split_once('/').unwrap_or((entry, ""));
        match top {
            "base" => {
                if rest.starts_with("lib/") {
                    ensure!(
                        entry == library,
                        "the bundle holds {entry}: the base's only library is {library}"
                    );
                    has_library = true;
                }
                has_content |= rest.starts_with("assets/content/");
            }
            "BUNDLE-METADATA" => has_symbols |= entry == symbols,
            "META-INF" | "BundleConfig.pb" => {}
            _ => bail!("the bundle holds {entry}, outside the base module: the game ships one module"),
        }
    }
    ensure!(has_library, "the bundle has no {library}");
    ensure!(has_content, "the bundle's base has no cook under assets/content/");
    ensure!(has_symbols, "the bundle carries no {symbols}: is AGP's debugSymbolLevel set?");
    Ok(symbols)
}

/// A `PT_LOAD` program header's placement.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Segment {
    offset: u64,
    address: u64,
    align: u64,
}

/// The `LOAD` segments of a 64-bit little-endian ELF file, as its program headers place them.
fn load_segments(elf: &[u8]) -> anyhow::Result<Vec<Segment>> {
    const PT_LOAD: u32 = 1;
    ensure!(elf.starts_with(b"\x7fELF"), "not an ELF file");
    ensure!(elf.get(4..6) == Some(&[2, 1]), "not a 64-bit little-endian ELF file");
    let bytes = |at: u64, len: usize| {
        usize::try_from(at)
            .ok()
            .and_then(|at| elf.get(at..at.checked_add(len)?))
            .context("the ELF file is cut short")
    };
    let u16_at =
        |at: u64| -> anyhow::Result<u64> { Ok(u16::from_le_bytes(bytes(at, 2)?.try_into()?).into()) };
    let u32_at = |at: u64| -> anyhow::Result<u32> { Ok(u32::from_le_bytes(bytes(at, 4)?.try_into()?)) };
    let u64_at = |at: u64| -> anyhow::Result<u64> { Ok(u64::from_le_bytes(bytes(at, 8)?.try_into()?)) };
    let (table, size, count) = (u64_at(0x20)?, u16_at(0x36)?, u16_at(0x38)?);
    ensure!(size >= 56, "program headers of {size} bytes are too small");
    let mut segments = Vec::new();
    for i in 0..count {
        let header = table + i * size;
        if u32_at(header)? == PT_LOAD {
            segments.push(Segment {
                offset: u64_at(header + 8)?,
                address: u64_at(header + 16)?,
                align: u64_at(header + 48)?,
            });
        }
    }
    ensure!(!segments.is_empty(), "the ELF file has no LOAD segments");
    Ok(segments)
}

/// Fails unless every segment loads at a multiple of [`PAGE`]: aligned to it, and placed in the file
/// where its address is within a page.
fn check_page_alignment(segments: &[Segment]) -> anyhow::Result<()> {
    for segment in segments {
        ensure!(
            segment.align >= PAGE && segment.align.is_power_of_two(),
            "a LOAD segment aligns to {:#x}, under {PAGE:#x}",
            segment.align
        );
        ensure!(
            segment.offset % PAGE == segment.address % PAGE,
            "a LOAD segment at offset {:#x} maps to {:#x}, which a {PAGE:#x} page can't hold",
            segment.offset,
            segment.address
        );
    }
    Ok(())
}

/// `bundletool get-size total`'s answer: the fewest and most bytes a device downloads.
fn download_size(csv: &str) -> anyhow::Result<(u64, u64)> {
    let mut lines = csv.lines().filter(|line| !line.trim().is_empty());
    ensure!(lines.next().map(str::trim) == Some("MIN,MAX"), "unexpected get-size output: {csv}");
    let row = lines.next().with_context(|| format!("unexpected get-size output: {csv}"))?;
    let (min, max) =
        row.trim().split_once(',').with_context(|| format!("unexpected get-size output: {csv}"))?;
    Ok((min.parse()?, max.parse()?))
}

/// What `aapt2 dump badging` says of an APK, as far as the checks read it.
#[derive(Debug, Default, PartialEq, Eq)]
struct Badging {
    package: String,
    version_code: String,
    version_name: String,
    min_sdk: String,
    target_sdk: String,
    debuggable: bool,
    launchable: Option<String>,
    vulkan: bool,
    native_code: Vec<String>,
}

impl Badging {
    fn parse(text: &str) -> anyhow::Result<Self> {
        let mut badging = Self::default();
        // Features are indented under their `feature-group`.
        for line in text.lines().map(str::trim_start) {
            let (key, value) = line.split_once(':').unwrap_or((line, ""));
            match key {
                "package" => {
                    badging.package = attribute(value, "name").unwrap_or_default();
                    badging.version_code = attribute(value, "versionCode").unwrap_or_default();
                    badging.version_name = attribute(value, "versionName").unwrap_or_default();
                }
                "sdkVersion" | "minSdkVersion" => badging.min_sdk = quoted(value).collect(),
                "targetSdkVersion" => badging.target_sdk = quoted(value).collect(),
                "application-debuggable" => badging.debuggable = true,
                "launchable-activity" => badging.launchable = attribute(value, "name"),
                "uses-feature" => {
                    badging.vulkan |= attribute(value, "name").as_deref() == Some(VULKAN_FEATURE)
                }
                "native-code" => badging.native_code = quoted(value).map(str::to_owned).collect(),
                _ => {}
            }
        }
        ensure!(!badging.package.is_empty(), "aapt2's badging names no package:\n{text}");
        Ok(badging)
    }

    fn check(&self, expected: &Expected<'_>) -> anyhow::Result<()> {
        let pairs: [(&str, &str, String); 5] = [
            ("package", &self.package, expected.package.to_owned()),
            ("versionCode", &self.version_code, expected.version_code.to_string()),
            ("versionName", &self.version_name, expected.version_name.to_owned()),
            ("minSdkVersion", &self.min_sdk, expected.min_sdk.to_string()),
            ("targetSdkVersion", &self.target_sdk, expected.target_sdk.to_string()),
        ];
        for (what, found, wanted) in pairs {
            ensure!(found == wanted, "its {what} is {found}, but xtask built {wanted}");
        }
        ensure!(!self.debuggable, "it's debuggable: a store build never is");
        let launchable = self.launchable.as_deref().unwrap_or("none");
        ensure!(
            launchable == expected.activity,
            "its launcher activity is {launchable}, but xtask built {}",
            expected.activity
        );
        ensure!(self.vulkan, "it doesn't require {VULKAN_FEATURE}");
        Ok(())
    }
}

/// The feature that keeps the game off devices without Vulkan 1.1 (the manifest's `uses-feature`).
const VULKAN_FEATURE: &str = "android.hardware.vulkan.version";

/// The single-quoted strings in `text`, as aapt2 prints values.
fn quoted(text: &str) -> impl Iterator<Item = &str> {
    text.split('\'').skip(1).step_by(2)
}

/// `name='value'` in one of aapt2's lines.
fn attribute(line: &str, name: &str) -> Option<String> {
    let at = line.split(' ').find_map(|part| part.strip_prefix(name)?.strip_prefix("='"))?;
    Some(at.split('\'').next()?.to_owned())
}

/// The certificate a bundle is signed with, as `keytool -printcert -jarfile` prints it.
#[derive(Debug, PartialEq, Eq)]
pub struct Certificate {
    pub owner: String,
    pub sha256: String,
}

impl Certificate {
    fn parse(text: &str) -> anyhow::Result<Self> {
        let field = |name: &str| {
            text.lines().find_map(|line| Some(line.trim().strip_prefix(name)?.trim().to_owned()))
        };
        let owner = field("Owner:").with_context(|| format!("keytool names no signer:\n{text}"))?;
        let sha256 =
            field("SHA256:").with_context(|| format!("keytool gives no SHA-256 fingerprint:\n{text}"))?;
        Ok(Self { owner, sha256 })
    }

    /// Whether it's the SDK's debug certificate, which AGP and bundletool make as `CN=Android Debug`.
    fn is_debug(&self) -> bool {
        self.owner.split(',').any(|part| part.trim() == "CN=Android Debug")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn expected() -> Expected<'static> {
        Expected {
            package: "com.example.sapphire.game",
            version_code: 1180,
            version_name: "0.1.0",
            min_sdk: 29,
            target_sdk: 36,
            activity: "com.example.sapphire.game.GameActivity".into(),
            abi: "arm64-v8a",
            library: "libgame_app.so".into(),
            key: Key::Debug,
        }
    }

    const ENTRIES: &str = "BundleConfig.pb\n\
        BUNDLE-METADATA/com.android.tools.build.debugsymbols/arm64-v8a/libgame_app.so.dbg\n\
        base/manifest/AndroidManifest.xml\n\
        base/assets/content/\n\
        base/assets/content/core.spack\n\
        base/dex/classes.dex\n\
        base/lib/arm64-v8a/libgame_app.so\n\
        META-INF/MANIFEST.MF\n";

    /// One module, one library, the cook in the base and the symbols beside it; anything else fails.
    #[test]
    fn the_bundle_holds_one_module_with_its_library_cook_and_symbols() {
        let symbols = check_contents(ENTRIES.lines(), &expected()).unwrap();
        assert_eq!(
            symbols,
            "BUNDLE-METADATA/com.android.tools.build.debugsymbols/arm64-v8a/libgame_app.so.dbg"
        );

        let fails = |entries: String, says: &str| {
            let error = format!("{:#}", check_contents(entries.lines(), &expected()).unwrap_err());
            assert!(error.contains(says), "{error}");
        };
        fails(
            ENTRIES.replace("BUNDLE-METADATA/com.android.tools.build.debugsymbols", "x/y"),
            "outside the base",
        );
        fails(
            ENTRIES.lines().filter(|line| !line.starts_with("BUNDLE")).collect::<Vec<_>>().join("\n"),
            "debugSymbolLevel",
        );
        fails(ENTRIES.replace("base/assets/content/core.spack\n", ""), "no cook");
        fails(format!("{ENTRIES}base/lib/x86_64/libgame_app.so\n"), "only library");
        fails(format!("{ENTRIES}content/assets/content/a.spack\n"), "one module");
    }

    /// A 64-bit little-endian ELF header with these program headers, each `(type, offset, address, align)`.
    fn elf(headers: &[(u32, u64, u64, u64)]) -> Vec<u8> {
        let mut elf = vec![0u8; 64];
        elf[..6].copy_from_slice(b"\x7fELF\x02\x01");
        elf[0x20..0x28].copy_from_slice(&64u64.to_le_bytes());
        elf[0x36..0x38].copy_from_slice(&56u16.to_le_bytes());
        elf[0x38..0x3a].copy_from_slice(&u16::try_from(headers.len()).unwrap().to_le_bytes());
        for &(kind, offset, address, align) in headers {
            let mut header = [0u8; 56];
            header[..4].copy_from_slice(&kind.to_le_bytes());
            header[8..16].copy_from_slice(&offset.to_le_bytes());
            header[16..24].copy_from_slice(&address.to_le_bytes());
            header[48..56].copy_from_slice(&align.to_le_bytes());
            elf.extend_from_slice(&header);
        }
        elf
    }

    #[test]
    fn load_segments_hold_16_kb_pages() {
        // As NDK r29 links the game: a LOAD at 0, then one placed a page on (a note between is ignored).
        let good = elf(&[(1, 0, 0, 0x4000), (4, 0x238, 0x238, 4), (1, 0x1e5_c000, 0x1e6_0000, 0x4000)]);
        let segments = load_segments(&good).unwrap();
        assert_eq!(segments.len(), 2);
        check_page_alignment(&segments).unwrap();

        let four_kb = load_segments(&elf(&[(1, 0, 0, 0x1000)])).unwrap();
        assert!(check_page_alignment(&four_kb).unwrap_err().to_string().contains("aligns to 0x1000"));
        let misplaced = load_segments(&elf(&[(1, 0x1000, 0x4000, 0x4000)])).unwrap();
        assert!(check_page_alignment(&misplaced).unwrap_err().to_string().contains("offset 0x1000"));

        // Not an ELF file, or one cut short, is an error, never a panic.
        assert!(load_segments(b"PK\x03\x04").is_err());
        let cut = elf(&[(1, 0, 0, 0x4000)]);
        assert!(load_segments(&cut[..80]).unwrap_err().to_string().contains("cut short"));
        assert!(load_segments(&elf(&[(4, 0, 0, 4)])).unwrap_err().to_string().contains("no LOAD"));
    }

    #[test]
    fn reads_the_download_size() {
        assert_eq!(download_size("MIN,MAX\n38253307,38253319\n").unwrap(), (38_253_307, 38_253_319));
        assert!(download_size("SDK,MIN,MAX\n29,1,2\n").is_err());
    }

    /// As aapt2 36 printed the game's base APK's badging, cut to what the checks read and a little around it.
    const BADGING: &str = "package: name='com.example.sapphire.game' versionCode='1180' versionName='0.1.0' \
        platformBuildVersionName='16' platformBuildVersionCode='36' compileSdkVersion='36' compileSdkVersionCodename='16'\n\
        minSdkVersion:'29'\n\
        targetSdkVersion:'36'\n\
        uses-permission: name='android.permission.VIBRATE'\n\
        application-label:'Sapphire'\n\
        application-label-ja:'サファイア'\n\
        application: label='Sapphire' icon='res/mipmap-anydpi-v26/ic_launcher.xml'\n\
        launchable-activity: name='com.example.sapphire.game.GameActivity'  label='' icon=''\n\
        feature-group: label=''\n  \
          uses-feature-not-required: name='android.hardware.touchscreen'\n  \
          uses-feature: name='android.hardware.vulkan.version' version='4198400'\n  \
          uses-feature: name='android.hardware.screen.landscape'\n\
        main\n\
        supports-screens: 'small' 'normal' 'large' 'xlarge'\n";

    #[test]
    fn badging_must_say_what_xtask_built() {
        let badging = Badging::parse(BADGING).unwrap();
        assert_eq!(badging.version_code, "1180");
        assert_eq!(badging.launchable.as_deref(), Some("com.example.sapphire.game.GameActivity"));
        badging.check(&expected()).unwrap();

        let fails = |text: String, says: &str| {
            let error = Badging::parse(&text).unwrap().check(&expected()).unwrap_err().to_string();
            assert!(error.contains(says), "{error}");
        };
        fails(
            BADGING.replace("versionCode='1180'", "versionCode='1'"),
            "versionCode is 1, but xtask built 1180",
        );
        fails(BADGING.replace("targetSdkVersion:'36'", "targetSdkVersion:'35'"), "targetSdkVersion");
        fails(format!("{BADGING}application-debuggable\n"), "debuggable");
        fails(BADGING.replace("GameActivity'", "OtherActivity'"), "launcher activity");
        fails(BADGING.replace("vulkan.version", "vulkan.level"), "doesn't require");

        let split = Badging::parse(
            "package: name='com.example.sapphire.game' split='config.arm64_v8a'\nnative-code: 'arm64-v8a'\n",
        );
        assert_eq!(split.unwrap().native_code, ["arm64-v8a"]);
        assert!(Badging::parse("nothing here").is_err());
    }

    #[test]
    fn reads_the_signer() {
        let printed = "Signer #1:\n\nSignature:\n\nOwner: C=US, O=Android, CN=Android Debug\n\
            Issuer: C=US, O=Android, CN=Android Debug\nCertificate fingerprints:\n\t SHA1: AA:BB\n\t SHA256: 01:02:03\n";
        let certificate = Certificate::parse(printed).unwrap();
        assert_eq!(certificate.sha256, "01:02:03");
        assert!(certificate.is_debug());
        let upload = Certificate::parse(&printed.replace("CN=Android Debug", "CN=Sapphire Upload")).unwrap();
        assert!(!upload.is_debug());
        assert!(Certificate::parse("jar is unsigned.").is_err());
    }
}
