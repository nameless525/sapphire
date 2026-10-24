//! Android: build the app as a shared library, package it with the Gradle project in
//! `platform/android/`, and run it on a device or the emulator.
//!
//! Gradle owns the APK. xtask first generates its gitignored inputs in `app/src/main/`
//! (spec §7.3):
//!
//! - `java/org/libsdl/app/`: SDL's Java glue, copied from the `sdl3-src` crate that
//!   Cargo.lock pins, so the Java side always matches the SDL linked into the app.
//! - `jniLibs/arm64-v8a/lib<package>.so`: the app crate as a `cdylib`, with SDL and libc++
//!   linked in statically, exporting `JNI_OnLoad` and `SDL_main`.
//! - `assets/content/`: the mobile cook.
//!
//! Needs a macOS or Linux host, NDK r28+ (16 KB page alignment, spec §6.7), the Android
//! SDK, and JDK 17+ for Gradle.

use std::ffi::OsString;
use std::fs::{self, File};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;
use std::{env, thread};

use anyhow::{Context as _, bail, ensure};
use serde::Deserialize;

use super::{App, Options, Platform};
use crate::content::{self, Cook};
use crate::workspace::{self, CargoBuild};

const TARGET: &str = "aarch64-linux-android";
const ABI: &str = "arm64-v8a";
/// `minSdk` in `platform/android/app/build.gradle.kts` (Android 10, spec §2). The NDK's
/// compiler drivers carry it in their names.
const MIN_SDK: u32 = 29;
/// The first NDK release that links 16 KB-aligned libraries by default (spec §6.7).
const MIN_NDK: u32 = 28;
/// The emulator `run` boots when no device is connected.
const AVD: &str = "sapphire_api36";
const BOOT_TIMEOUT_SECS: u32 = 300;
/// The launcher activity, relative to the application id.
const ACTIVITY: &str = ".SandboxActivity";
/// The string-array intent extra `SandboxActivity.getArguments()` reads.
const ARGS_EXTRA: &str = "sapphire.args";

/// Builds the APK and copies it to `target/android/<Name>.apk`.
pub fn bundle(app: &App, options: &Options) -> anyhow::Result<PathBuf> {
    ensure!(options.exe.is_none(), "--exe is not supported for android");
    let sdk = sdk_dir()?;
    let ndk = Ndk::locate(&sdk)?;
    // Fail now rather than after the native build.
    require_java()?;
    let library = build_library(app, &ndk, options.release)?;
    let content = content::cook(Cook::Mobile)?;

    let project = workspace::root().join("platform/android");
    let main = project.join("app/src/main");
    sync_sdl_java(&main.join("java/org/libsdl/app"))?;
    let jni_libs = main.join("jniLibs");
    workspace::remove_dir_if_exists(&jni_libs)?;
    let abi_dir = jni_libs.join(ABI);
    fs::create_dir_all(&abi_dir).with_context(|| format!("failed to create {}", abi_dir.display()))?;
    fs::copy(&library, abi_dir.join(library_file(app)))
        .with_context(|| format!("failed to copy {} into the Gradle project", library.display()))?;
    let assets = main.join("assets/content");
    workspace::remove_dir_if_exists(&assets)?;
    workspace::copy_dir(&content, &assets)?;

    let variant = if options.release { "release" } else { "debug" };
    let mut ndk_path = OsString::from("-Psapphire.ndkPath=");
    ndk_path.push(&ndk.root);
    workspace::run(
        Command::new(project.join("gradlew"))
            .current_dir(&project)
            .arg(if options.release { "assembleRelease" } else { "assembleDebug" })
            .arg(ndk_path)
            .arg(format!("-Psapphire.ndkVersion={}", ndk.revision))
            .arg(format!("-Psapphire.version={}", env!("CARGO_PKG_VERSION")))
            .env("ANDROID_HOME", &sdk),
    )?;

    let built = project.join(format!("app/build/outputs/apk/{variant}/app-{variant}.apk"));
    let dir = super::bundle_dir(Platform::Android);
    fs::create_dir_all(&dir).with_context(|| format!("failed to create {}", dir.display()))?;
    let apk = dir.join(format!("{}.apk", app.bundle_name));
    fs::copy(&built, &apk).with_context(|| format!("failed to copy {}", built.display()))?;
    eprintln!("bundled {}", super::display(&apk));
    Ok(apk)
}

/// Bundles, picks the connected device (booting the emulator if there is none), installs
/// and launches, then streams the app's logcat until Ctrl-C.
pub fn run(app: &App, options: &Options, args: &[String]) -> anyhow::Result<()> {
    let apk = bundle(app, options)?;
    let adb = Adb::connect(&sdk_dir()?)?;
    workspace::run(adb.command().args(["install", "-r"]).arg(&apk))?;

    // `-S` stops a running instance first: the activity is single-instance, so a running
    // one would get the new intent, and its arguments, only through `onNewIntent`.
    let component = format!("{}/{ACTIVITY}", app.bundle_id);
    let mut start = adb.command();
    start.args(["shell", "am", "start", "-S", "-W", "-n", &component]);
    if !args.is_empty() {
        start.args(["--esa", ARGS_EXTRA, &shell_quote(&string_array_extra(args))]);
    }
    eprintln!("launching {component} on {}", adb.serial);
    // `am` prints its report, errors included, on stdout, and some versions exit 0 on errors.
    let launch = start.stderr(Stdio::inherit()).output().context("failed to run adb")?;
    let report = String::from_utf8_lossy(&launch.stdout);
    eprint!("{report}");
    ensure!(launch.status.success() && !report.contains("Error"), "failed to launch {component}");

    let pid = workspace::output(adb.command().args(["shell", "pidof", app.bundle_id]))?;
    let pid = pid.trim();
    ensure!(!pid.is_empty(), "{} exited during launch; see `adb logcat`", app.bundle_id);
    eprintln!("streaming logcat for pid {pid}; Ctrl-C detaches and leaves the app running");
    workspace::run(adb.command().arg("logcat").arg(format!("--pid={pid}")))
}

/// The Android SDK: `ANDROID_HOME`, else the deprecated `ANDROID_SDK_ROOT`, else Android
/// Studio's default location.
fn sdk_dir() -> anyhow::Result<PathBuf> {
    let configured = env::var_os("ANDROID_HOME").or_else(|| env::var_os("ANDROID_SDK_ROOT"));
    match configured.map(PathBuf::from).or_else(default_sdk_dir) {
        Some(dir) if dir.is_dir() => Ok(dir),
        Some(dir) => bail!("no Android SDK at {}; set ANDROID_HOME", dir.display()),
        None => bail!("Android SDK not found; set ANDROID_HOME"),
    }
}

fn default_sdk_dir() -> Option<PathBuf> {
    let home = PathBuf::from(env::var_os("HOME")?);
    match env::consts::OS {
        "macos" => Some(home.join("Library/Android/sdk")),
        "linux" => Some(home.join("Android/Sdk")),
        _ => None,
    }
}

/// Gradle runs on `JAVA_HOME`, or else on the `java` in `PATH`.
fn require_java() -> anyhow::Result<()> {
    if env::var_os("JAVA_HOME").is_some() {
        return Ok(());
    }
    let java = Command::new("java").arg("-version").stdout(Stdio::null()).stderr(Stdio::null()).status();
    ensure!(
        java.is_ok_and(|status| status.success()),
        "Gradle needs a JDK 17+: set JAVA_HOME (for example to /opt/homebrew/opt/openjdk@17)"
    );
    Ok(())
}

/// An Android NDK.
struct Ndk {
    root: PathBuf,
    /// `Pkg.Revision`, for example `29.0.14206865`.
    revision: String,
}

impl Ndk {
    /// `ANDROID_NDK_HOME`, else the newest NDK installed in the SDK.
    fn locate(sdk: &Path) -> anyhow::Result<Self> {
        let root = match env::var_os("ANDROID_NDK_HOME") {
            Some(root) => PathBuf::from(root),
            None => {
                let dir = sdk.join("ndk");
                let names = fs::read_dir(&dir).into_iter().flatten().flatten().map(|entry| entry.file_name());
                let names: Vec<String> = names.filter_map(|name| name.into_string().ok()).collect();
                let newest = newest_version(names.iter().map(String::as_str)).with_context(|| {
                    format!(
                        "no NDK in {}; install r{MIN_NDK}+ with `sdkmanager \"ndk;<version>\"` or set ANDROID_NDK_HOME",
                        dir.display()
                    )
                })?;
                dir.join(newest)
            }
        };
        let properties = root.join("source.properties");
        let text = fs::read_to_string(&properties)
            .with_context(|| format!("{} is not an NDK: cannot read source.properties", root.display()))?;
        let revision = ndk_revision(&text)
            .with_context(|| format!("no Pkg.Revision in {}", properties.display()))?
            .to_owned();
        let major = revision.split('.').next().and_then(|major| major.parse::<u32>().ok());
        ensure!(
            major.is_some_and(|major| major >= MIN_NDK),
            "NDK {revision} at {} is too old: 16 KB page alignment needs r{MIN_NDK}+ (spec §6.7)",
            root.display()
        );
        Ok(Self { root, revision })
    }

    /// The environment for cross-compiling to Android: cc-rs and CMake (SDL's build fails
    /// without the NDK's toolchain file) for C and C++, and the linker for rustc.
    fn cargo_env(&self) -> anyhow::Result<Vec<(&'static str, OsString)>> {
        let bin = self.root.join("toolchains/llvm/prebuilt").join(host_tag()?).join("bin");
        let clang = bin.join(format!("{TARGET}{MIN_SDK}-clang"));
        ensure!(clang.is_file(), "{} is missing from the NDK", clang.display());
        let mut clang_cxx = clang.clone().into_os_string();
        clang_cxx.push("++");
        Ok(vec![
            ("ANDROID_NDK_HOME", self.root.clone().into()),
            ("ANDROID_NDK_ROOT", self.root.clone().into()),
            (
                "CMAKE_TOOLCHAIN_FILE_aarch64_linux_android",
                self.root.join("build/cmake/android.toolchain.cmake").into(),
            ),
            ("CC_aarch64_linux_android", clang.clone().into()),
            ("CXX_aarch64_linux_android", clang_cxx),
            ("AR_aarch64_linux_android", bin.join("llvm-ar").into()),
            ("CARGO_TARGET_AARCH64_LINUX_ANDROID_LINKER", clang.into()),
        ])
    }
}

/// The NDK's prebuilt toolchain directory for this host. The macOS NDK ships universal
/// binaries under the `darwin-x86_64` name.
fn host_tag() -> anyhow::Result<&'static str> {
    match env::consts::OS {
        "macos" => Ok("darwin-x86_64"),
        "linux" => Ok("linux-x86_64"),
        os => bail!("Android builds need a macOS or Linux host, not {os} (spec §2)"),
    }
}

/// The highest dotted version among `names`, compared numerically; other names are ignored.
fn newest_version<'a>(names: impl IntoIterator<Item = &'a str>) -> Option<&'a str> {
    let parse = |name: &str| name.split('.').map(|part| part.parse::<u32>().ok()).collect::<Option<Vec<_>>>();
    names.into_iter().filter_map(|name| Some((parse(name)?, name))).max().map(|(_, name)| name)
}

/// `Pkg.Revision` from an NDK's `source.properties`.
fn ndk_revision(source_properties: &str) -> Option<&str> {
    source_properties.lines().find_map(|line| {
        let (key, value) = line.split_once('=')?;
        (key.trim() == "Pkg.Revision").then(|| value.trim())
    })
}

fn library_file(app: &App) -> String {
    format!("lib{}.so", app.package)
}

/// Builds the app crate's library as a `cdylib` and returns its path.
fn build_library(app: &App, ndk: &Ndk, release: bool) -> anyhow::Result<PathBuf> {
    eprintln!("using NDK {} at {}", ndk.revision, ndk.root.display());
    let env = ndk.cargo_env()?;
    let file = library_file(app);
    // `cargo rustc` adds the cdylib crate type for this build only, so desktop and iOS keep
    // linking the crate as an rlib. The soname names the library for Android's loader.
    let build = CargoBuild { package: app.package, target: Some(TARGET), release, env: &env };
    let mut command = build.command("rustc");
    command.args(["--lib", "--crate-type", "cdylib", "--", "-C"]);
    command.arg(format!("link-arg=-Wl,-soname,{file}"));
    workspace::build_artifact(&mut command, &file, |artifact| {
        artifact.filenames.into_iter().find(|path| path.file_name().is_some_and(|name| *name == *file))
    })
}

/// Replaces `dest` with SDL's Java glue from the `sdl3-src` crate that Cargo.lock pins.
fn sync_sdl_java(dest: &Path) -> anyhow::Result<()> {
    let sdl = locked_package("sdl3-src")?;
    let source = sdl.dir.join("SDL/android-project/app/src/main/java/org/libsdl/app");
    workspace::remove_dir_if_exists(dest)?;
    workspace::copy_dir(&source, dest)?;
    eprintln!("synced SDL {} Java sources into {}", sdl.version, super::display(dest));
    Ok(())
}

/// A dependency as Cargo.lock resolves it.
struct LockedPackage {
    version: String,
    /// Where cargo unpacked its sources.
    dir: PathBuf,
}

/// Finds `name` among the packages Cargo.lock resolves for the Android target.
fn locked_package(name: &str) -> anyhow::Result<LockedPackage> {
    #[derive(Deserialize)]
    struct Metadata {
        packages: Vec<Package>,
    }
    #[derive(Deserialize)]
    struct Package {
        name: String,
        version: String,
        manifest_path: PathBuf,
    }

    let json = workspace::output(workspace::cargo().args([
        "metadata",
        "--format-version",
        "1",
        "--filter-platform",
        TARGET,
    ]))?;
    let metadata: Metadata = serde_json::from_str(&json).context("unexpected `cargo metadata` output")?;
    let mut found = metadata.packages.into_iter().filter(|package| package.name == name);
    match (found.next(), found.next()) {
        (Some(package), None) => {
            let dir = package.manifest_path.parent().context("manifest path has no parent")?.to_owned();
            Ok(LockedPackage { version: package.version, dir })
        }
        (None, _) => bail!("`{name}` is not in Cargo.lock"),
        (Some(_), Some(_)) => bail!("Cargo.lock has several versions of `{name}`"),
    }
}

/// `adb` talking to one device.
struct Adb {
    program: PathBuf,
    serial: String,
}

impl Adb {
    /// Uses `ANDROID_SERIAL` if set, else the one connected device, else boots [`AVD`].
    fn connect(sdk: &Path) -> anyhow::Result<Self> {
        let program = sdk.join("platform-tools/adb");
        ensure!(program.is_file(), "adb not found; install the SDK's platform-tools");
        let serial = match env::var("ANDROID_SERIAL") {
            Ok(serial) => serial,
            Err(_) => {
                let devices = online_devices(&workspace::output(Command::new(&program).arg("devices"))?);
                match devices.as_slice() {
                    [] => boot_emulator(sdk, &program)?,
                    [serial] => serial.clone(),
                    _ => bail!(
                        "several devices are connected ({}); pick one with ANDROID_SERIAL",
                        devices.join(", ")
                    ),
                }
            }
        };
        Ok(Self { program, serial })
    }

    fn command(&self) -> Command {
        let mut command = Command::new(&self.program);
        command.args(["-s", &self.serial]);
        command
    }
}

/// Serials of the devices `adb devices` lists as ready.
fn online_devices(adb_devices: &str) -> Vec<String> {
    adb_devices
        .lines()
        .skip(1) // "List of devices attached"
        .filter_map(|line| match line.split_whitespace().collect::<Vec<_>>()[..] {
            [serial, "device", ..] => Some(serial.to_owned()),
            _ => None,
        })
        .collect()
}

/// Boots [`AVD`] headless and waits until Android has finished booting. The emulator keeps
/// running after xtask exits, for the next run.
fn boot_emulator(sdk: &Path, adb: &Path) -> anyhow::Result<String> {
    let emulator = sdk.join("emulator/emulator");
    let avds = workspace::output(Command::new(&emulator).arg("-list-avds"))
        .context("the Android emulator is not installed")?;
    ensure!(
        avds.lines().any(|avd| avd.trim() == AVD),
        "no device is connected and there is no `{AVD}` emulator to boot; connect a device, or create \
         the AVD (Pixel 8, system-images;android-36;google_apis;arm64-v8a) with avdmanager"
    );

    let dir = super::bundle_dir(Platform::Android);
    fs::create_dir_all(&dir).with_context(|| format!("failed to create {}", dir.display()))?;
    let log_path = dir.join("emulator.log");
    let log = File::create(&log_path).with_context(|| format!("failed to create {}", log_path.display()))?;
    let mut command = Command::new(&emulator);
    // `host` gives the guest hardware Vulkan, also headless: gfxstream over the host's Vulkan
    // (MoltenVK on macOS). `auto` would pick the lavapipe software renderer when headless.
    command.args(["-avd", AVD, "-no-window", "-gpu", "host", "-no-audio", "-no-boot-anim"]);
    command.stdin(Stdio::null()).stdout(log.try_clone()?).stderr(log);
    // Its own process group, so Ctrl-C on the logcat stream leaves the emulator running.
    #[cfg(unix)]
    std::os::unix::process::CommandExt::process_group(&mut command, 0);
    eprintln!("no device connected; booting {AVD} headless (log: {})", super::display(&log_path));
    let mut child = command.spawn().context("failed to start the emulator")?;

    for _ in 0..BOOT_TIMEOUT_SECS {
        if let Some(status) = child.try_wait()? {
            bail!("the emulator exited during boot ({status}); see {}", log_path.display());
        }
        let devices = online_devices(&workspace::output(Command::new(adb).arg("devices"))?);
        if let Some(serial) = devices.into_iter().find(|serial| boot_completed(adb, serial)) {
            return Ok(serial);
        }
        thread::sleep(Duration::from_secs(1));
    }
    bail!("the emulator did not finish booting in {BOOT_TIMEOUT_SECS} s; see {}", log_path.display())
}

fn boot_completed(adb: &Path, serial: &str) -> bool {
    let output = Command::new(adb)
        .args(["-s", serial, "shell", "getprop", "sys.boot_completed"])
        .stderr(Stdio::null())
        .output();
    output.is_ok_and(|output| output.status.success() && output.stdout.trim_ascii() == b"1")
}

/// `am start --esa` takes one comma-separated argument. Commas inside elements are escaped,
/// and `SandboxActivity` unescapes them.
fn string_array_extra(args: &[String]) -> String {
    args.iter().map(|arg| arg.replace(',', r"\,")).collect::<Vec<_>>().join(",")
}

/// Quotes `arg` for the device's shell, which runs the command line `adb shell` sends.
fn shell_quote(arg: &str) -> String {
    format!("'{}'", arg.replace('\'', r"'\''"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::platforms::SANDBOX;

    #[test]
    fn picks_the_newest_ndk_numerically() {
        let names = ["9.9.1", "28.2.13676358", "29.0.14206865", "ndk-bundle", ".DS_Store", "29.0.9"];
        assert_eq!(newest_version(names), Some("29.0.14206865"));
        assert_eq!(newest_version(["ndk-bundle"]), None);
    }

    #[test]
    fn reads_the_ndk_revision() {
        let properties = "Pkg.Desc = Android NDK\nPkg.Revision = 29.0.14206865\nPkg.ReleaseName = r29\n";
        assert_eq!(ndk_revision(properties), Some("29.0.14206865"));
        assert_eq!(ndk_revision("Pkg.Desc = Android NDK\n"), None);
    }

    #[test]
    fn lists_only_ready_devices() {
        let listing = "List of devices attached\nemulator-5554\tdevice\nR5CT\tunauthorized\n\
                       0A1\tdevice product:x model:Pixel_8\n\n";
        assert_eq!(online_devices(listing), ["emulator-5554", "0A1"]);
        assert!(online_devices("List of devices attached\n\n").is_empty());
    }

    #[test]
    fn quotes_launch_arguments() {
        let args = ["--frames", "120", "a,b", "it's"].map(String::from);
        let extra = string_array_extra(&args);
        assert_eq!(extra, r"--frames,120,a\,b,it's");
        assert_eq!(shell_quote(&extra), r"'--frames,120,a\,b,it'\''s'");
    }

    #[test]
    fn gradle_project_matches_the_app() {
        let gradle = include_str!("../../../platform/android/app/build.gradle.kts");
        let activity = include_str!(
            "../../../platform/android/app/src/main/java/dev/sapphire/sandbox/SandboxActivity.java"
        );
        assert!(gradle.contains(&format!("applicationId = \"{}\"", SANDBOX.bundle_id)));
        assert!(gradle.contains(&format!("minSdk = {MIN_SDK} ")));
        assert!(gradle.contains(&format!("abiFilters += \"{ABI}\"")));
        assert!(activity.contains(&format!("new String[] {{\"{}\"}}", SANDBOX.package)));
        assert!(activity.contains(&format!("EXTRA_ARGS = \"{ARGS_EXTRA}\"")));
        let manifest = include_str!("../../../platform/android/app/src/main/AndroidManifest.xml");
        assert!(manifest.contains(&format!("android:name=\"{ACTIVITY}\"")));
    }
}
