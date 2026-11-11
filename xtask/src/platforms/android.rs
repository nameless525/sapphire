//! Android: build the app as a shared library, package it with the Gradle project in
//! `platform/android/`, and run it on a device or the emulator.
//!
//! Gradle owns the APK. xtask generates its inputs under
//! `target/android/build/<Name>/gen/<variant>/` and points the project's source sets there, and
//! Gradle's outputs at `target/android/build/<Name>/gradle/`, so nothing generated lands in the
//! source tree (spec §7.3) and each app keeps its own build:
//!
//! - `java/<bundle id as a path>/<Class>.java`: the app's launcher activity, from the one template
//!   `platform/android/app/src/main/AppActivity.java.in`.
//! - `java/org/libsdl/app/`: SDL's Java glue, copied from the `sdl3-src` crate that
//!   Cargo.lock pins, so the Java side always matches the SDL linked into the app.
//! - `jniLibs/arm64-v8a/lib<package>.so`: the app crate as a `cdylib`, with SDL and libc++
//!   linked in statically, exporting `JNI_OnLoad` and `SDL_main`.
//! - `assets/content/`: the mobile cook.
//!
//! App identity and version reach Gradle as `-Psapphire.*` properties.
//!
//! Needs a macOS or Linux host, NDK r28+ (16 KB page alignment, spec §6.7), the Android
//! SDK, and JDK 17+ for Gradle.

use std::ffi::OsString;
use std::fs::{self, File};
use std::io::{BufRead as _, BufReader, Write as _};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::time::Duration;
use std::{env, thread};

use anyhow::{Context as _, anyhow, bail, ensure};
use serde::Deserialize;

use super::{App, Bundle, BundleClaim, Options, Platform, aab};
use crate::content::{self, Cook};
use crate::version;
use crate::workspace::{self, BuildProfile, CargoBuild};

/// The Rust target the app's library builds for.
pub(super) const TARGET: &str = "aarch64-linux-android";
/// The one ABI the app ships: its library is at `base/lib/<ABI>/` in the AAB.
pub(super) const ABI: &str = "arm64-v8a";
/// `minSdk` in `platform/android/app/build.gradle.kts` (Android 10, spec §2). The NDK's
/// compiler drivers carry it in their names, and SDL's CMake build targets it too.
const MIN_SDK: u32 = 29;
/// `targetSdk` there: Android 16, which Play requires of new apps and updates from 2026-08-31.
const TARGET_SDK: u32 = 36;
/// The first NDK release that links 16 KB-aligned libraries by default (spec §6.7).
const MIN_NDK: u32 = 28;
/// The emulator `run` boots when no device is connected.
const AVD: &str = "sapphire_api36";
const BOOT_TIMEOUT_SECS: u32 = 300;
/// How long an emulator that holds the AVD's lock gets to show up in `adb devices`.
const REGISTER_TIMEOUT_SECS: u32 = 30;
/// The launcher activity's template, in the Gradle project. It sits outside the source sets: xtask
/// compiles only what it fills in.
const ACTIVITY_TEMPLATE: &str = "app/src/main/AppActivity.java.in";
/// The intent extra the activity's `getArguments()` reads: a base64-encoded JSON array
/// of strings, honored only by debuggable builds.
const ARGS_EXTRA: &str = "sapphire.args";

/// Builds the APK and copies it to `target/android/<Name>.apk`.
pub fn bundle(app: &App, options: &Options) -> anyhow::Result<Bundle> {
    ensure!(options.exe.is_none(), "--exe is not supported for android");
    let sdk = sdk_dir()?;
    let ndk = Ndk::locate(&sdk)?;
    // Fail now rather than after the native build.
    require_java()?;
    let claim = BundleClaim::take(Platform::Android)?;
    let library = build_library(app, &ndk, BuildProfile::app(options.release), &options.features, &[])?;
    let content = content::cook(Cook::Mobile)?;
    let task = if options.release { GradleTask::AssembleProfile } else { GradleTask::AssembleDebug };
    let build_dir = super::bundle_dir(Platform::Android).join("build").join(&app.bundle_name);
    let gradle = Gradle { sdk: &sdk, ndk: &ndk, build_dir: &build_dir };
    let built = gradle.build(app, version::development_number(), &library, &content.dir, task)?;
    drop(content);
    let apk = claim.staging(&format!("{}.apk", app.bundle_name))?;
    fs::copy(&built, &apk).with_context(|| format!("failed to copy {}", built.display()))?;
    claim.publish(&apk)
}

/// What a store package of the app holds for Android: the AAB Gradle's `bundleRelease` makes from a
/// `shipping` build of the library, that library before Gradle strips it, for its symbols, release lint's
/// report, and what the bundle's checks found ([`aab`]).
pub struct Packaged {
    pub aab: PathBuf,
    pub library: PathBuf,
    pub lint: PathBuf,
    pub checked: aab::Checked,
}

/// Builds the app's library in the `shipping` profile with `env` (the build's number and ID), and an AAB
/// around it and `content`, with Gradle working in `work_dir`, then checks it ([`aab::check`]). Release
/// lint fails it on any error. It's signed with the upload key if the environment names one
/// ([`aab::UPLOAD_KEYSTORE`]), and with the debug key otherwise.
pub fn package(
    app: &App,
    build: u32,
    env: &[(&str, OsString)],
    content: &Path,
    work_dir: &Path,
) -> anyhow::Result<Packaged> {
    let sdk = sdk_dir()?;
    let ndk = Ndk::locate(&sdk)?;
    require_java()?;
    let key = aab::Key::from_env();
    let library = build_library(app, &ndk, BuildProfile::Shipping, &[], env)?;
    let gradle = Gradle { sdk: &sdk, ndk: &ndk, build_dir: work_dir };
    let aab = gradle.build(app, build, &library, content, GradleTask::BundleRelease)?;
    let tools = aab::Tools::locate(&sdk, &gradle.dir().join(BUNDLETOOL_CLASSPATH))?;
    let expected = aab::Expected {
        package: &app.bundle_id,
        version_code: build,
        version_name: &app.version,
        min_sdk: MIN_SDK,
        target_sdk: TARGET_SDK,
        activity: activity(app).name(),
        abi: ABI,
        library: library_file(app),
        key,
    };
    let checked = aab::check(&aab, &expected, &tools, &work_dir.join("checks"))?;
    Ok(Packaged { aab, library, lint: gradle.dir().join(LINT_REPORT), checked })
}

/// What Gradle's `bundletoolClasspath` task writes, under its build directory.
const BUNDLETOOL_CLASSPATH: &str = "bundletool.classpath";
/// Release lint's text report, under Gradle's build directory.
const LINT_REPORT: &str = "reports/lint-results-release.txt";

/// The Gradle builds xtask runs, and what each leaves.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum GradleTask {
    /// `bundle` and `run`'s development APK.
    AssembleDebug,
    /// `bundle` and `run`'s `--release` APK, for profiling: optimized, but debuggable, so it takes launch
    /// arguments.
    AssembleProfile,
    /// `package`'s store bundle: not debuggable, with the library's symbols, held to release lint, and
    /// with bundletool's classpath listed for its checks.
    BundleRelease,
}

impl GradleTask {
    /// Gradle's arguments: the tasks, and for the store build, the configuration cache off, so the upload
    /// key's passwords never reach it.
    fn args(self) -> &'static [&'static str] {
        match self {
            Self::AssembleDebug => &["assembleDebug"],
            Self::AssembleProfile => &["assembleProfile"],
            Self::BundleRelease => {
                &["--no-configuration-cache", "bundleRelease", "lintRelease", "bundletoolClasspath"]
            }
        }
    }

    fn variant(self) -> &'static str {
        match self {
            Self::AssembleDebug => "debug",
            Self::AssembleProfile => "profile",
            Self::BundleRelease => "release",
        }
    }

    /// What it builds, under Gradle's build directory.
    fn output(self) -> PathBuf {
        let variant = self.variant();
        match self {
            Self::AssembleDebug | Self::AssembleProfile => format!("outputs/apk/{variant}/app-{variant}.apk"),
            Self::BundleRelease => format!("outputs/bundle/{variant}/app-{variant}.aab"),
        }
        .into()
    }
}

/// The Gradle project in `platform/android/`, building one app in `build_dir`: its generated inputs in
/// `gen/<variant>/`, and Gradle's own outputs in `gradle/`.
struct Gradle<'a> {
    sdk: &'a Path,
    ndk: &'a Ndk,
    build_dir: &'a Path,
}

impl Gradle<'_> {
    /// Gradle's own build directory.
    fn dir(&self) -> PathBuf {
        self.build_dir.join("gradle")
    }

    /// Generates the project's inputs for `app` (the activity, SDL's Java, `library`, `content`, the icon
    /// and the names) and runs `task` offline, returning what it built.
    fn build(
        &self,
        app: &App,
        build: u32,
        library: &Path,
        content: &Path,
        task: GradleTask,
    ) -> anyhow::Result<PathBuf> {
        let gen_dir = self.build_dir.join("gen").join(task.variant());
        let gradle_dir = self.dir();
        workspace::remove_dir_if_exists(&gen_dir)?;
        let project = workspace::root().join("platform/android");
        write_activity(&project.join(ACTIVITY_TEMPLATE), app, &gen_dir.join("java"))?;
        sync_sdl_java(&gen_dir.join("java/org/libsdl/app"))?;
        let abi_dir = gen_dir.join("jniLibs").join(ABI);
        fs::create_dir_all(&abi_dir).with_context(|| format!("failed to create {}", abi_dir.display()))?;
        fs::copy(library, abi_dir.join(library_file(app)))
            .with_context(|| format!("failed to copy {}", library.display()))?;
        workspace::copy_dir(content, &gen_dir.join("assets/content"))?;
        write_resources(app, &self.build_dir.join("icons"), &gen_dir.join("res"))?;

        // Offline unless `SAPPHIRE_GRADLE_ONLINE` is set: Gradle's Maven artifacts are downloads, which a build
        // on a developer's machine never makes, so one missing from Gradle's cache fails the build, naming it
        // (m4-plan §3, "Android packaging"). CI starts each job with an empty cache, so its Android job sets
        // the variable and Gradle fetches the Android plugin there, as it did before packaging went offline.
        let mut gradle = Command::new(project.join("gradlew"));
        gradle.current_dir(&project);
        if gradle_offline(env::var_os(GRADLE_ONLINE).as_deref()) {
            gradle.arg("--offline");
        }
        gradle
            .args(task.args())
            .args(gradle_properties(app, build, self.ndk, &gen_dir, &gradle_dir))
            .env("ANDROID_HOME", self.sdk);
        workspace::run(&mut gradle)?;
        Ok(gradle_dir.join(task.output()))
    }
}

/// The variable that lets Gradle fetch what its cache lacks: CI's Android job sets it, since each job starts
/// with an empty cache. Anywhere else Gradle runs offline.
const GRADLE_ONLINE: &str = "SAPPHIRE_GRADLE_ONLINE";

/// Whether Gradle runs `--offline`, given `SAPPHIRE_GRADLE_ONLINE`: unless it's set to something other than
/// empty or `0`.
fn gradle_offline(online: Option<&std::ffi::OsStr>) -> bool {
    online.is_none_or(|value| value.is_empty() || value == "0")
}

/// Writes the app's Android resources under `res`: its name in each language (`values/strings.xml`,
/// `values-ja/strings.xml`), and the launcher icon's mipmaps, made from the project's icon in `icons`.
fn write_resources(app: &App, icons: &Path, res: &Path) -> anyhow::Result<()> {
    super::make_icons(icons)?;
    workspace::copy_dir(&icons.join("android/res"), res)?;
    for (language, name) in app.display_name.by_language() {
        let dir = res.join(if language == "en" { "values".to_owned() } else { format!("values-{language}") });
        fs::create_dir_all(&dir).with_context(|| format!("failed to create {}", dir.display()))?;
        let path = dir.join("strings.xml");
        fs::write(&path, strings_xml(name)).with_context(|| format!("failed to write {}", path.display()))?;
    }
    Ok(())
}

/// A strings resource naming the app `app_name`, escaped as Android's resource compiler reads a string:
/// XML's metacharacters, quotes and backslashes, and a leading `@` or `?`, which would make it a reference.
fn strings_xml(name: &str) -> String {
    let mut escaped = String::with_capacity(name.len());
    for (i, c) in name.chars().enumerate() {
        match c {
            '&' => escaped.push_str("&amp;"),
            '<' => escaped.push_str("&lt;"),
            '>' => escaped.push_str("&gt;"),
            '\'' | '"' | '\\' => {
                escaped.push('\\');
                escaped.push(c);
            }
            '@' | '?' if i == 0 => {
                escaped.push('\\');
                escaped.push(c);
            }
            c => escaped.push(c),
        }
    }
    format!(
        "<?xml version=\"1.0\" encoding=\"utf-8\"?>\n<!-- Made by xtask from platform/identity.toml. -->\n<resources>\n    \
         <string name=\"app_name\">{escaped}</string>\n</resources>\n"
    )
}

/// Bundles, picks the connected device (booting the emulator if there is none), installs
/// and launches, then streams the app's logcat until the app exits or Ctrl-C. Succeeds only
/// if the app's main function returned, with status 0 where the app logs it: a nonzero
/// status, a process that ends without returning, a logcat that stops under a running app
/// and a device that drops all fail the run.
pub fn run(app: &App, options: &Options, args: &[String]) -> anyhow::Result<()> {
    let apk = bundle(app, options)?;
    let sdk = sdk_dir()?;
    let program = sdk.join("platform-tools/adb");
    // `-S` stops a running instance first: the activity is single-instance, so a running
    // one would get the new intent, and its arguments, only through `onNewIntent`.
    let component = activity(app).component();
    let start_args = |command: &mut Command| {
        command.args(["shell", "am", "start", "-S", "-W", "-n", &component]);
        if !args.is_empty() {
            command.args(["--es", ARGS_EXTRA, &encode_args(args)]);
        }
    };
    if options.dry_run {
        let mut install = Command::new(&program);
        install.args(["install", "-r"]).arg(&apk.path);
        let mut start = Command::new(&program);
        start_args(&mut start);
        for command in [&install, &start] {
            eprintln!("dry run: {}", workspace::shell_line(command));
        }
        return Ok(());
    }

    let adb = Adb::connect(&sdk, program)?;
    workspace::run(adb.command().args(["install", "-r"]).arg(&apk.path))?;
    drop(apk); // The device has its own copy now.
    let mut start = adb.command();
    start_args(&mut start);
    eprintln!("launching {component} on {}", adb.serial);
    // `am` prints its report, errors included, on stdout, and some versions exit 0 on errors.
    let launch = start.stderr(Stdio::inherit()).output().context("failed to run adb")?;
    let report = String::from_utf8_lossy(&launch.stdout);
    eprint!("{report}");
    ensure!(launch.status.success() && !report.contains("Error"), "failed to launch {component}");

    let pids = adb.pids(&app.bundle_id).map_err(|why| anyhow!("lost {} during launch: {why}", adb.serial))?;
    let pid = match pids.as_slice() {
        [pid] => pid,
        [] => bail!("{} exited during launch; see `adb logcat`", app.bundle_id),
        _ => bail!("{} runs as several processes ({})", app.bundle_id, pids.join(", ")),
    };
    eprintln!("streaming logcat for pid {pid}; Ctrl-C detaches and leaves the app running");
    follow_logcat(&adb, &app.bundle_id, pid)
}

/// SDLActivity logs this once `SDL_main` has returned, whatever it returned.
const MAIN_FINISHED: &str = "Finished main function";

/// The platform layer's `android_main` logs this (tag `sapphire`, info), then the integer
/// status, as the app's main function returns: `main returned status=<n>`. Builds from
/// before it have only [`MAIN_FINISHED`].
const MAIN_RETURNED: &str = "main returned status=";

/// What a process logs as it crashes: bionic for a fatal signal (a Rust panic aborts, and
/// crash capture passes the signal on), the Java runtime for an uncaught exception.
const CRASH_MARKERS: [&str; 2] = ["Fatal signal", "FATAL EXCEPTION"];

/// Streams process `pid`'s logcat to stdout until the app's main function returns (the
/// process itself lives on, cached), the process dies, logcat stops or the device drops
/// (`logcat --pid` would outlive the first two), then judges the run by [`AppLog::outcome`].
fn follow_logcat(adb: &Adb, package: &str, pid: &str) -> anyhow::Result<()> {
    follow_logcat_with_timing(
        adb,
        package,
        pid,
        LogcatTiming { process_poll: Duration::from_secs(1), drain: Duration::from_secs(1) },
    )
}

/// Real devices need time for their final log lines to arrive. Synthetic process fixtures can
/// acknowledge that their output is already in the pipe instead of waiting out those intervals.
struct LogcatTiming {
    process_poll: Duration,
    drain: Duration,
}

fn follow_logcat_with_timing(
    adb: &Adb,
    package: &str,
    pid: &str,
    timing: LogcatTiming,
) -> anyhow::Result<()> {
    let mut command = adb.command();
    command.arg("logcat").arg(format!("--pid={pid}")).stdout(Stdio::piped());
    eprintln!("$ adb -s {} logcat --pid={pid}", adb.serial);
    let mut logcat = command.spawn().context("failed to start adb logcat")?;
    let stdout = logcat.stdout.take().expect("stdout is piped");
    let (finished, finished_rx) = mpsc::channel();
    let forward = thread::spawn(move || -> std::io::Result<AppLog> {
        let mut log = AppLog::default();
        let mut out = std::io::stdout().lock();
        for line in BufReader::new(stdout).lines() {
            let line = line?;
            writeln!(out, "{line}")?;
            if log.observe(&line) {
                // The receiver is gone only if the caller already stopped following.
                let _ = finished.send(());
            }
        }
        Ok(log)
    });

    let stop = loop {
        let stop = match finished_rx.recv_timeout(timing.process_poll) {
            Ok(()) => break None,
            Err(mpsc::RecvTimeoutError::Disconnected) => match adb.process(package, pid) {
                Process::Running => Stop::LogcatEnded,
                Process::Gone => Stop::ProcessGone,
                Process::Unreachable(why) => Stop::DeviceLost(why),
            },
            Err(mpsc::RecvTimeoutError::Timeout) => match adb.process(package, pid) {
                Process::Running => continue,
                Process::Gone => Stop::ProcessGone,
                Process::Unreachable(why) => Stop::DeviceLost(why),
            },
        };
        break Some(stop);
    };
    thread::sleep(timing.drain); // Let logcat print the last lines.
    // Ok if it exited already: then `wait` returns its own status, which `Stop::LogcatEnded` reports.
    let _ = logcat.kill();
    let status = logcat.wait()?;
    let log = forward.join().expect("the logcat forwarder doesn't panic")?;
    eprintln!("{}", log.outcome(stop, &status.to_string())?);
    Ok(())
}

/// Why [`follow_logcat`] stopped before the app's main function returned.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Stop {
    /// `pidof` no longer lists the process.
    ProcessGone,
    /// `adb logcat` exited while the process still ran.
    LogcatEnded,
    /// adb no longer reaches the device: what it said.
    DeviceLost(String),
}

/// What the app's logcat showed.
#[derive(Debug, Default)]
struct AppLog {
    /// SDLActivity logged [`MAIN_FINISHED`].
    main_finished: bool,
    /// The status the app's main function returned, where the platform layer logged it
    /// ([`MAIN_RETURNED`]).
    status: Option<i32>,
    /// The first line that reported a crash.
    crash: Option<String>,
}

impl AppLog {
    /// Takes in one logcat line; true when it says the app's main function returned.
    fn observe(&mut self, line: &str) -> bool {
        if line.contains(MAIN_FINISHED) {
            self.main_finished = true;
            return true;
        }
        if let Some((_, rest)) = line.split_once(MAIN_RETURNED)
            && let Some(Ok(status)) = rest.split_whitespace().next().map(str::parse)
        {
            self.status = Some(status);
            return true;
        }
        if self.crash.is_none() && CRASH_MARKERS.iter().any(|marker| line.contains(marker)) {
            self.crash = Some(line.trim().to_owned());
        }
        false
    }

    /// The run's result once following stopped: `stop` is `None` when the main function
    /// returned, and `logcat` is how logcat exited. Only a returned main function succeeds,
    /// and only with status 0 where the platform layer logged it ([`MAIN_RETURNED`]). Without
    /// that line SDL's carries no status, so an app that fails from main passes there. Ctrl-C
    /// never gets here; it ends xtask and leaves the app running.
    fn outcome(&self, stop: Option<Stop>, logcat: &str) -> anyhow::Result<&'static str> {
        if let Some(status) = self.status {
            ensure!(status == 0, "the app's main function returned status {status}");
            return Ok("the app's main function returned status 0");
        }
        let Some(stop) = stop.filter(|_| !self.main_finished) else {
            return Ok("the app's main function returned");
        };
        match (stop, &self.crash) {
            (Stop::ProcessGone, Some(crash)) => bail!("the app crashed: {crash}"),
            (Stop::ProcessGone, None) => bail!(
                "the app's process ended without returning from its main function: it was stopped (swiped \
                 away, `am force-stop`, low memory) or crashed without saying why; see the log above"
            ),
            (Stop::LogcatEnded, _) => bail!("adb logcat ended ({logcat}) while the app still ran"),
            (Stop::DeviceLost(why), _) => {
                bail!("lost the device while the app ran ({why}); its outcome is unknown")
            }
        }
    }
}

/// What became of the app's process.
#[derive(Debug, PartialEq, Eq)]
enum Process {
    Running,
    Gone,
    /// adb no longer reaches the device: what it said.
    Unreachable(String),
}

/// The app's launcher activity: a class named for the bundle ID's last segment, in the Java package
/// the bundle ID names, which is also the Gradle namespace.
struct Activity<'a> {
    package: &'a str,
    class: String,
}

fn activity(app: &App) -> Activity<'_> {
    // `App::new` holds every segment to ASCII letters and digits, starting with a letter.
    let last = app.bundle_id.rsplit('.').next().expect("rsplit yields at least one piece");
    let (first, rest) = last.split_at(1);
    Activity { package: &app.bundle_id, class: format!("{}{rest}Activity", first.to_ascii_uppercase()) }
}

impl Activity<'_> {
    /// As `am start -n` takes it, and as the runbooks write it.
    fn component(&self) -> String {
        format!("{}/.{}", self.package, self.class)
    }

    /// As the manifest names it.
    fn name(&self) -> String {
        format!("{}.{}", self.package, self.class)
    }

    /// Where its source goes, under a Java source root.
    fn source_path(&self) -> PathBuf {
        let mut path: PathBuf = self.package.split('.').collect();
        path.push(format!("{}.java", self.class));
        path
    }
}

/// Fills the activity's template for `app`. The values are a bundle ID and a package name, which need
/// no escaping in Java.
fn render_activity(template: &str, app: &App) -> anyhow::Result<String> {
    let activity = activity(app);
    let variables = [
        ("PACKAGE", activity.package),
        ("CLASS", activity.class.as_str()),
        ("LIBRARY", app.package.as_str()),
    ];
    super::fill_template(template, "activity", &variables, String::push_str)
}

/// Writes `app`'s activity under the Java source root `java_dir`.
fn write_activity(template: &Path, app: &App, java_dir: &Path) -> anyhow::Result<()> {
    let text =
        fs::read_to_string(template).with_context(|| format!("failed to read {}", template.display()))?;
    let path = java_dir.join(activity(app).source_path());
    fs::create_dir_all(path.parent().expect("the path has a parent"))
        .with_context(|| format!("failed to create the directory of {}", path.display()))?;
    fs::write(&path, render_activity(&text, app)?)
        .with_context(|| format!("failed to write {}", path.display()))
}

/// Everything the Gradle project takes from xtask (see `app/build.gradle.kts`): `build` is the build
/// number, Android's `versionCode`.
fn gradle_properties(app: &App, build: u32, ndk: &Ndk, gen_dir: &Path, gradle_dir: &Path) -> Vec<OsString> {
    let path = |name: &str, path: &Path| {
        let mut property = OsString::from(format!("-Psapphire.{name}="));
        property.push(path);
        property
    };
    vec![
        path("genDir", gen_dir),
        path("buildDir", gradle_dir),
        path("ndkPath", &ndk.root),
        format!("-Psapphire.ndkVersion={}", ndk.revision).into(),
        format!("-Psapphire.applicationId={}", app.bundle_id).into(),
        format!("-Psapphire.activity={}", activity(app).name()).into(),
        format!("-Psapphire.versionName={}", app.version).into(),
        format!("-Psapphire.versionCode={build}").into(),
    ]
}

/// The NDK a package builds with, whose C and C++ headers' and runtime's paths a shipping build remaps
/// ([`super::symbols`]).
pub(super) fn ndk_root() -> anyhow::Result<PathBuf> {
    Ok(Ndk::locate(&sdk_dir()?)?.root)
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
    if env::var_os("JAVA_HOME").is_some_and(|home| !home.is_empty()) {
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

    /// The environment for cross-compiling to Android: cc-rs and CMake for C and C++, and
    /// the linker for rustc.
    fn cargo_env(&self) -> anyhow::Result<Vec<(&'static str, OsString)>> {
        let bin = self.root.join("toolchains/llvm/prebuilt").join(host_tag()?).join("bin");
        let clang = bin.join(format!("{TARGET}{MIN_SDK}-clang"));
        ensure!(clang.is_file(), "{} is missing from the NDK", clang.display());
        let mut clang_cxx = clang.clone().into_os_string();
        clang_cxx.push("++");
        Ok(vec![
            ("ANDROID_NDK_HOME", self.root.clone().into()),
            ("ANDROID_NDK_ROOT", self.root.clone().into()),
            // SDL's CMake build fails without the NDK's toolchain file.
            ("CMAKE_TOOLCHAIN_FILE_aarch64_linux_android", self.cmake_toolchain()?.into()),
            ("CC_aarch64_linux_android", clang.clone().into()),
            ("CXX_aarch64_linux_android", clang_cxx),
            ("AR_aarch64_linux_android", bin.join("llvm-ar").into()),
            ("CARGO_TARGET_AARCH64_LINUX_ANDROID_LINKER", clang.into()),
        ])
    }

    /// A CMake toolchain file that wraps the NDK's to build for [`MIN_SDK`]; the NDK's
    /// own defaults to its oldest supported API level (21). Written to
    /// `target/android/cmake/` only when it changes. It keeps the NDK file's name, which is
    /// how the `cmake` crate recognizes an NDK build. sdl3-sys doesn't rebuild when this
    /// changes: after an NDK or minSdk change, `cargo clean -p sdl3-sys --target
    /// aarch64-linux-android`.
    fn cmake_toolchain(&self) -> anyhow::Result<PathBuf> {
        let ndk_file = self.root.join("build/cmake/android.toolchain.cmake");
        let text = format!(
            "# Generated by cargo xtask (xtask/src/platforms/android.rs).\n\
             set(ANDROID_PLATFORM android-{MIN_SDK})\n\
             include(\"{}\")\n",
            ndk_file.display().to_string().replace('\\', "/")
        );
        let path = super::bundle_dir(Platform::Android).join("cmake/android.toolchain.cmake");
        if fs::read_to_string(&path).ok().as_deref() != Some(text.as_str()) {
            fs::create_dir_all(path.parent().expect("the path has a parent"))?;
            fs::write(&path, text).with_context(|| format!("failed to write {}", path.display()))?;
        }
        Ok(path)
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
pub(super) fn newest_version<'a>(names: impl IntoIterator<Item = &'a str>) -> Option<&'a str> {
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

/// Builds the app crate's library as a `cdylib` in `profile`, with `features` and the extra environment
/// `extra`, and returns its path.
fn build_library(
    app: &App,
    ndk: &Ndk,
    profile: BuildProfile,
    features: &[String],
    extra: &[(&str, OsString)],
) -> anyhow::Result<PathBuf> {
    eprintln!("using NDK {} at {}", ndk.revision, ndk.root.display());
    let mut env = ndk.cargo_env()?;
    env.extend(extra.iter().map(|(name, value)| (*name, value.clone())));
    let file = library_file(app);
    // `cargo rustc` adds the cdylib crate type for this build only, so desktop and iOS keep
    // linking the crate as an rlib. The soname names the library for Android's loader.
    let build = CargoBuild { package: &app.package, target: Some(TARGET), profile, features, env: &env };
    let mut command = build.command("rustc");
    command.args(["--lib", "--crate-type", "cdylib", "--", "-C"]);
    command.arg(format!("link-arg=-Wl,-soname,{file}"));
    workspace::build_artifact(&mut command, &file, |artifact| {
        artifact.filenames.into_iter().find(|path| path.file_name().is_some_and(|name| *name == *file))
    })
}

/// Copies SDL's Java glue from the `sdl3-src` crate that Cargo.lock pins into `dest`.
fn sync_sdl_java(dest: &Path) -> anyhow::Result<()> {
    let sdl = locked_package("sdl3-src")?;
    let source = sdl.dir.join("SDL/android-project/app/src/main/java/org/libsdl/app");
    workspace::copy_dir(&source, dest)?;
    eprintln!("copied SDL {} Java sources into {}", sdl.version, super::display(dest));
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

    let json = workspace::output(workspace::cargo("metadata").args([
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
    /// Picks the device: `ANDROID_SERIAL` if set, else the one connected device (an
    /// emulator that is still booting counts), else [`AVD`], reusing an instance that is
    /// already starting or booting it. Returns once Android has finished booting.
    fn connect(sdk: &Path, program: PathBuf) -> anyhow::Result<Self> {
        ensure!(program.is_file(), "adb not found; install the SDK's platform-tools");
        let mut emulator = None;
        let serial = match env::var("ANDROID_SERIAL") {
            Ok(serial) => serial,
            Err(_) => {
                let devices = list_devices(&workspace::output(Command::new(&program).arg("devices"))?);
                let usable: Vec<&Device> = devices.iter().filter(|d| d.usable()).collect();
                match usable.as_slice() {
                    [device] => device.serial.clone(),
                    [] => {
                        if let Some(device) = devices.iter().find(|d| !d.usable()) {
                            bail!(
                                "{} is {}; accept the USB debugging prompt on the device, or set ANDROID_SERIAL",
                                device.serial,
                                device.state
                            );
                        }
                        let (serial, child) = start_avd(sdk, &program)?;
                        emulator = child;
                        serial
                    }
                    _ => {
                        let serials: Vec<&str> = usable.iter().map(|d| d.serial.as_str()).collect();
                        bail!(
                            "several devices are connected ({}); pick one with ANDROID_SERIAL",
                            serials.join(", ")
                        )
                    }
                }
            }
        };
        let adb = Self { program, serial };
        adb.wait_for_boot(emulator)?;
        Ok(adb)
    }

    fn command(&self) -> Command {
        let mut command = Command::new(&self.program);
        command.args(["-s", &self.serial]);
        command
    }

    /// Waits until `sys.boot_completed` is set. `emulator` is the process this run started,
    /// if any, so its early exit is reported instead of waited out.
    fn wait_for_boot(&self, mut emulator: Option<(Child, PathBuf)>) -> anyhow::Result<()> {
        for second in 0..BOOT_TIMEOUT_SECS {
            if self.boot_completed() {
                return Ok(());
            }
            if second == 0 {
                eprintln!("waiting for {} to finish booting...", self.serial);
            }
            if let Some((child, log)) = &mut emulator
                && let Some(status) = child.try_wait()?
            {
                bail!("the emulator exited during boot ({status}); see {}", log.display());
            }
            thread::sleep(Duration::from_secs(1));
        }
        bail!("{} did not finish booting in {BOOT_TIMEOUT_SECS} s", self.serial)
    }

    /// Whether `package` still runs as process `pid`.
    fn process(&self, package: &str, pid: &str) -> Process {
        match self.pids(package) {
            Ok(pids) if pids.iter().any(|p| p == pid) => Process::Running,
            Ok(_) => Process::Gone,
            Err(why) => Process::Unreachable(why),
        }
    }

    /// The IDs of `package`'s processes, none once it has none, or what adb said when it no
    /// longer reaches the device.
    fn pids(&self, package: &str) -> Result<Vec<String>, String> {
        let output = self
            .command()
            .args(["shell", "pidof", package])
            .stderr(Stdio::null())
            .output()
            .map_err(|error| format!("adb didn't start: {error}"))?;
        let pids: Vec<String> =
            String::from_utf8_lossy(&output.stdout).split_whitespace().map(str::to_owned).collect();
        // `pidof` prints nothing and fails once the process is gone, as adb does once the
        // device is: only the device's state tells them apart.
        if pids.is_empty() {
            self.online()?;
        }
        Ok(pids)
    }

    /// Ok while adb reports the device online; otherwise what adb said.
    fn online(&self) -> Result<(), String> {
        let output =
            self.command().arg("get-state").output().map_err(|error| format!("adb didn't start: {error}"))?;
        if output.status.success() && output.stdout.trim_ascii() == b"device" {
            return Ok(());
        }
        let said =
            [&output.stdout, &output.stderr].map(|text| String::from_utf8_lossy(text).trim().to_owned());
        let said: Vec<&str> = said.iter().map(String::as_str).filter(|text| !text.is_empty()).collect();
        Err(if said.is_empty() { format!("`adb get-state` {}", output.status) } else { said.join("; ") })
    }

    fn boot_completed(&self) -> bool {
        let output =
            self.command().args(["shell", "getprop", "sys.boot_completed"]).stderr(Stdio::null()).output();
        output.is_ok_and(|output| output.status.success() && output.stdout.trim_ascii() == b"1")
    }
}

/// One line of `adb devices`.
#[derive(Debug, PartialEq, Eq)]
struct Device {
    serial: String,
    /// `device` when ready; `offline` while an emulator boots; `unauthorized`, ...
    state: String,
}

impl Device {
    fn usable(&self) -> bool {
        self.state == "device" || (self.state == "offline" && self.serial.starts_with("emulator-"))
    }
}

fn list_devices(adb_devices: &str) -> Vec<Device> {
    adb_devices
        .lines()
        .skip(1) // "List of devices attached"
        .filter_map(|line| {
            let mut fields = line.split_whitespace();
            Some(Device { serial: fields.next()?.to_owned(), state: fields.next()?.to_owned() })
        })
        .collect()
}

/// Returns the serial of the emulator running [`AVD`], starting one if none is. An
/// emulator that already holds the AVD's lock but hasn't reached `adb devices` yet is
/// waited for: a second instance would fail on the lock. Also returns the process this
/// call started, with its log, if any.
fn start_avd(sdk: &Path, adb: &Path) -> anyhow::Result<(String, Option<(Child, PathBuf)>)> {
    if avd_dir()?.join("hardware-qemu.ini.lock").exists() {
        eprintln!("an emulator for {AVD} is already starting; waiting for it");
        for _ in 0..REGISTER_TIMEOUT_SECS {
            if let Some(serial) = find_avd(adb)? {
                return Ok((serial, None));
            }
            thread::sleep(Duration::from_secs(1));
        }
        bail!(
            "{AVD} is locked ({}) but no emulator for it reached adb in {REGISTER_TIMEOUT_SECS} s; if \
             none is running, delete the lock file",
            avd_dir()?.join("hardware-qemu.ini.lock").display()
        );
    }

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
    // Headless, always. `host` gives the guest hardware Vulkan: gfxstream over the host's
    // Vulkan (MoltenVK on macOS). `auto` would pick the lavapipe software renderer.
    command.args(["-avd", AVD, "-no-window", "-gpu", "host", "-no-audio", "-no-boot-anim"]);
    command.stdin(Stdio::null()).stdout(log.try_clone()?).stderr(log);
    // Its own process group, so Ctrl-C on the logcat stream leaves the emulator running.
    #[cfg(unix)]
    std::os::unix::process::CommandExt::process_group(&mut command, 0);
    eprintln!("no device connected; booting {AVD} headless (log: {})", super::display(&log_path));
    let mut child = command.spawn().context("failed to start the emulator")?;

    for _ in 0..REGISTER_TIMEOUT_SECS {
        if let Some(status) = child.try_wait()? {
            bail!("the emulator exited during startup ({status}); see {}", log_path.display());
        }
        if let Some(serial) = find_avd(adb)? {
            return Ok((serial, Some((child, log_path))));
        }
        thread::sleep(Duration::from_secs(1));
    }
    bail!("the emulator did not reach adb in {REGISTER_TIMEOUT_SECS} s; see {}", log_path.display())
}

/// The serial of an emulator in `adb devices`, booting or booted, that runs [`AVD`].
fn find_avd(adb: &Path) -> anyhow::Result<Option<String>> {
    let devices = list_devices(&workspace::output(Command::new(adb).arg("devices"))?);
    Ok(devices.into_iter().filter(|d| d.serial.starts_with("emulator-")).map(|d| d.serial).find(|serial| {
        // The emulator console answers before adbd does, so this works while booting.
        let name =
            Command::new(adb).args(["-s", serial, "emu", "avd", "name"]).stderr(Stdio::null()).output();
        name.is_ok_and(|output| {
            String::from_utf8_lossy(&output.stdout).lines().next().map(str::trim) == Some(AVD)
        })
    }))
}

/// Where the emulator keeps [`AVD`]: `$ANDROID_AVD_HOME`, `$ANDROID_USER_HOME/avd` or
/// `~/.android/avd`.
fn avd_dir() -> anyhow::Result<PathBuf> {
    let home = env::var_os("ANDROID_AVD_HOME")
        .map(PathBuf::from)
        .or_else(|| env::var_os("ANDROID_USER_HOME").map(|home| PathBuf::from(home).join("avd")))
        .or_else(|| env::var_os("HOME").map(|home| PathBuf::from(home).join(".android/avd")))
        .context("HOME is not set")?;
    Ok(home.join(format!("{AVD}.avd")))
}

/// Encodes launch arguments for [`ARGS_EXTRA`]: JSON keeps every argument intact (empty
/// ones and trailing backslashes included), and base64 keeps it clear of the device
/// shell's and `am`'s quoting rules.
fn encode_args(args: &[String]) -> String {
    base64(serde_json::to_string(args).expect("strings serialize").as_bytes())
}

/// Standard base64 with padding (RFC 4648 §4), as `android.util.Base64.DEFAULT` decodes it.
fn base64(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let bits = chunk.iter().enumerate().fold(0u32, |bits, (i, &b)| bits | u32::from(b) << (16 - 8 * i));
        for i in 0..4 {
            out.push(if i <= chunk.len() {
                char::from(ALPHABET[(bits >> (18 - 6 * i) & 63) as usize])
            } else {
                '='
            });
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::platforms::{test_app, test_game};

    #[test]
    fn gradle_runs_offline_unless_the_environment_says_online() {
        use std::ffi::OsStr;
        assert!(gradle_offline(None));
        assert!(gradle_offline(Some(OsStr::new(""))));
        assert!(gradle_offline(Some(OsStr::new("0"))));
        assert!(!gradle_offline(Some(OsStr::new("1"))));
    }

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
        assert_eq!(ndk_revision("Pkg.Desc = Android NDK\r\nPkg.Revision = 28.2.1\r\n"), Some("28.2.1"));
        assert_eq!(ndk_revision("Pkg.Desc = Android NDK\n"), None);
    }

    #[test]
    fn treats_booting_emulators_as_devices() {
        let listing = "List of devices attached\nemulator-5554\toffline\nR5CT\tunauthorized\n\
                       0A1\tdevice product:x model:Pixel_8\n0B2\toffline\n\n";
        let devices = list_devices(listing);
        let usable: Vec<&str> = devices.iter().filter(|d| d.usable()).map(|d| d.serial.as_str()).collect();
        assert_eq!(usable, ["emulator-5554", "0A1"]);
        assert_eq!(devices.len(), 4);
        assert!(list_devices("List of devices attached\n\n").is_empty());
    }

    #[test]
    fn encodes_base64() {
        let cases = [("", ""), ("f", "Zg=="), ("fo", "Zm8="), ("foo", "Zm9v"), ("foobar", "Zm9vYmFy")];
        for (input, encoded) in cases {
            assert_eq!(base64(input.as_bytes()), encoded);
        }
        assert_eq!(base64(&[0xfb, 0xff]), "+/8=");
    }

    #[test]
    fn launch_arguments_round_trip() {
        let args = ["--frames", "120", "a,b", "it's", "", r"trailing\", "ünï"].map(String::from);
        let encoded = encode_args(&args);
        assert!(encoded.chars().all(|c| c.is_ascii_alphanumeric() || "+/=".contains(c)));
        // What the activity does: base64 → UTF-8 → JSON array of strings.
        let json = decode_base64(&encoded);
        assert_eq!(serde_json::from_slice::<Vec<String>>(&json).unwrap(), args);
    }

    fn decode_base64(text: &str) -> Vec<u8> {
        const ALPHABET: &str = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        let sextets: Vec<u32> =
            text.trim_end_matches('=').chars().map(|c| ALPHABET.find(c).unwrap() as u32).collect();
        sextets
            .chunks(4)
            .flat_map(|chunk| {
                let bits = chunk.iter().enumerate().fold(0, |bits, (i, s)| bits | s << (18 - 6 * i));
                (0..chunk.len() - 1).map(move |i| (bits >> (16 - 8 * i)) as u8)
            })
            .collect()
    }

    /// Feeds logcat lines to an [`AppLog`].
    fn app_log(lines: &[&str]) -> AppLog {
        let mut log = AppLog::default();
        for line in lines {
            log.observe(line);
        }
        log
    }

    #[test]
    fn only_a_returned_main_function_passes() {
        const RUNNING: &str = "10-27 12:00:00.000  4242  4260 V SDL     : Running main function SDL_main";
        const FINISHED: &str = "10-27 12:00:09.000  4242  4260 V SDL     : Finished main function";
        const NATIVE_CRASH: &str = "10-27 12:00:05.000  4242  4260 F libc    : Fatal signal 6 (SIGABRT), code -1 (SI_QUEUE) in tid 4260";
        const JAVA_CRASH: &str = "10-27 12:00:05.000  4242  4242 E AndroidRuntime: FATAL EXCEPTION: main";
        let lost = || Some(Stop::DeviceLost("error: device offline".into()));

        let finished = app_log(&[RUNNING, FINISHED]);
        assert!(finished.main_finished);
        assert!(finished.outcome(None, "signal: 9").is_ok());
        // Whatever comes after: the main function returned.
        for stop in [Stop::ProcessGone, Stop::LogcatEnded, Stop::DeviceLost("gone".into())] {
            assert!(finished.outcome(Some(stop), "exit status: 1").is_ok());
        }

        let running = app_log(&[RUNNING]);
        assert!(!running.main_finished && running.crash.is_none());
        let error =
            |log: &AppLog, stop: Option<Stop>| log.outcome(stop, "exit status: 7").unwrap_err().to_string();
        let killed = error(&running, Some(Stop::ProcessGone));
        assert!(
            killed.contains("without returning from its main function") && killed.contains("swiped away")
        );
        assert_eq!(
            error(&running, Some(Stop::LogcatEnded)),
            "adb logcat ended (exit status: 7) while the app still ran"
        );
        assert!(
            error(&running, lost()).contains("lost the device while the app ran (error: device offline)")
        );

        let native = app_log(&[RUNNING, NATIVE_CRASH, "F DEBUG   : backtrace:"]);
        assert_eq!(native.crash.as_deref(), Some(NATIVE_CRASH));
        assert_eq!(error(&native, Some(Stop::ProcessGone)), format!("the app crashed: {NATIVE_CRASH}"));
        let java = app_log(&[RUNNING, JAVA_CRASH]);
        assert_eq!(error(&java, Some(Stop::ProcessGone)), format!("the app crashed: {JAVA_CRASH}"));
        // A lost device stays the reason, crash or not: the process may still be running.
        assert!(error(&native, lost()).starts_with("lost the device"));
    }

    #[test]
    fn the_logged_status_decides_where_the_app_logs_one() {
        const FINISHED: &str = "10-27 12:00:09.000  4242  4260 V SDL     : Finished main function";
        let returned = |status: &str| {
            format!("10-27 12:00:09.000  4242  4260 I sapphire: main returned status={status}")
        };

        let mut log = AppLog::default();
        assert!(log.observe(&returned("0")), "the status line says main returned");
        log.observe(FINISHED);
        assert_eq!(log.status, Some(0));
        assert_eq!(log.outcome(None, "signal: 9").unwrap(), "the app's main function returned status 0");

        for (status, shown) in [("1", 1), ("-1", -1), ("101", 101)] {
            let failed = app_log(&[&returned(status), FINISHED]);
            assert_eq!(failed.status, Some(shown));
            let error = failed.outcome(None, "signal: 9").unwrap_err().to_string();
            assert_eq!(error, format!("the app's main function returned status {shown}"));
            // Whatever stopped the following: main returned, and with that status.
            assert!(failed.outcome(Some(Stop::ProcessGone), "exit status: 0").is_err());
        }
        assert!(app_log(&[&returned("0")]).outcome(Some(Stop::LogcatEnded), "exit status: 0").is_ok());

        // Not a status: the line is no status line, and SDL's line still decides.
        let garbled = app_log(&[&returned("x"), FINISHED]);
        assert_eq!(garbled.status, None);
        assert!(garbled.outcome(None, "signal: 9").is_ok());
    }

    /// A stand-in for adb that plays the scenario named by its serial (`-s <scenario>`): the
    /// app's logcat, `pidof` and `get-state`. It lives in `$TMPDIR/xtask-fake-adb-<pid>/`,
    /// which dropping it removes. One exists at a time, made during a [`Turn`]: the tests
    /// that use it take turns, so none writes the script while another's adb is being spawned
    /// (a child forked then would hold the script open for writing, and running it would fail
    /// with ETXTBSY on Linux).
    #[cfg(unix)]
    struct FakeAdb {
        dir: PathBuf,
        adb: Adb,
    }

    /// A test's turn with [`FakeAdb`].
    #[cfg(unix)]
    type Turn = std::sync::MutexGuard<'static, ()>;

    #[cfg(unix)]
    impl FakeAdb {
        const SCRIPT: &str = r#"#!/bin/sh
scenario=$2
shift 2
line() { printf '10-27 12:00:00.000  4242  4260 %s\n' "$1"; }
if [ "$1" = logcat ]; then echo "$$" > "$0.pid"; fi
before_death() {
    # Force a process probe before the log line, to exercise startup/forwarding races.
    attempt=0
    while [ ! -f "$0.probed" ] && [ "$attempt" -lt 300 ]; do
        sleep 0.01
        attempt=$((attempt + 1))
    done
    if [ ! -f "$0.probed" ]; then echo 'fake adb was never probed' >&2; exit 2; fi
    line "$1"
    # Only now may pidof report the process gone: the log bytes are already in the pipe.
    : > "$0.ready"
    exec sleep 30
}
case "$1 $scenario" in
    "logcat offline") echo 'error: device offline' >&2; exit 7 ;;
    "logcat finished") line 'V SDL     : Running main function SDL_main'; line 'V SDL     : Finished main function'; exec sleep 30 ;;
    "logcat killed") before_death 'I sapphire::sandbox: frame stats fps=30.0' ;;
    "logcat crashed") before_death 'F libc    : Fatal signal 6 (SIGABRT), code -1 (SI_QUEUE) in tid 4260 (SDLThread), pid 4242' ;;
    "logcat stopped") line 'I sapphire::sandbox: frame stats fps=30.0'; exit 0 ;;
    "shell finished"|"shell stopped") echo 4242 ;;
    "shell killed"|"shell crashed")
        : > "$0.probed"
        if [ -f "$0.ready" ]; then exit 1; else echo 4242; fi ;;
    "shell "*) exit 1 ;;
    "get-state offline") echo 'error: device offline' >&2; exit 1 ;;
    "get-state "*) echo device ;;
    *) echo "fake adb: unexpected $*" >&2; exit 2 ;;
esac
"#;

        /// Waits for this test's turn.
        fn turn() -> Turn {
            static TURN: std::sync::Mutex<()> = std::sync::Mutex::new(());
            // A test that failed during its turn left nothing behind: its fake adb dropped.
            TURN.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
        }

        fn new(_turn: &Turn, scenario: &str) -> Self {
            use std::os::unix::fs::PermissionsExt as _;
            let dir = env::temp_dir().join(format!("xtask-fake-adb-{}", std::process::id()));
            fs::create_dir_all(&dir).unwrap();
            let program = dir.join("adb");
            fs::write(&program, Self::SCRIPT).unwrap();
            fs::set_permissions(&program, fs::Permissions::from_mode(0o755)).unwrap();
            Self { dir, adb: Adb { program, serial: scenario.to_owned() } }
        }
    }

    #[cfg(unix)]
    impl Drop for FakeAdb {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.dir);
        }
    }

    #[cfg(unix)]
    fn follow(scenario: &str) -> anyhow::Result<()> {
        let turn = FakeAdb::turn();
        let fake = FakeAdb::new(&turn, scenario);
        let result = follow_logcat_with_timing(
            &fake.adb,
            "dev.sapphire.sandbox",
            "4242",
            LogcatTiming { process_poll: Duration::from_millis(5), drain: Duration::ZERO },
        );
        let dir = fake.dir.clone();
        assert!(fake.adb.program.is_file());
        if matches!(scenario, "killed" | "crashed") {
            assert!(dir.join("adb.probed").is_file(), "the process was probed before its log arrived");
            assert!(dir.join("adb.ready").is_file(), "the death marker followed the log write");
        }
        let pid = fs::read_to_string(dir.join("adb.pid")).unwrap();
        let alive = Command::new("kill")
            .args(["-0", pid.trim()])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .expect("the Unix kill command");
        assert!(!alive.success(), "the fake logcat process outlived follow_logcat");
        drop(fake);
        // Checked within the turn, before another test makes it again.
        assert!(!dir.exists(), "the fake adb's directory outlived it");
        drop(turn);
        result
    }

    #[cfg(unix)]
    #[test]
    fn a_run_succeeds_once_main_returns() {
        follow("finished").unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn a_run_fails_when_the_device_drops() {
        let error = follow("offline").unwrap_err().to_string();
        assert!(error.contains("device offline"), "{error}");
    }

    #[cfg(unix)]
    #[test]
    fn a_run_fails_when_the_app_dies() {
        let error = follow("killed").unwrap_err().to_string();
        assert!(error.contains("without returning from its main function"), "{error}");
        let error = follow("crashed").unwrap_err().to_string();
        assert!(error.contains("crashed") && error.contains("Fatal signal 6"), "{error}");
    }

    #[cfg(unix)]
    #[test]
    fn a_run_fails_when_logcat_stops_under_a_running_app() {
        // Logcat's own exit status, not the kill's.
        let error = follow("stopped").unwrap_err().to_string();
        assert_eq!(error, "adb logcat ended (exit status: 0) while the app still ran");
    }

    const ACTIVITY_SOURCE: &str = include_str!("../../../platform/android/app/src/main/AppActivity.java.in");

    /// The properties xtask passes are the ones the project reads, each way round.
    #[test]
    fn gradle_project_matches_xtask() {
        let gradle = include_str!("../../../platform/android/app/build.gradle.kts");
        let manifest = include_str!("../../../platform/android/app/src/main/AndroidManifest.xml");
        assert!(gradle.contains(&format!("minSdk = {MIN_SDK} ")));
        assert!(gradle.contains(&format!("targetSdk = {TARGET_SDK}\n")));
        assert!(gradle.contains(&format!("abiFilters += \"{ABI}\"")));

        let ndk = Ndk { root: "ndk".into(), revision: "29.0.1".into() };
        let passed = gradle_properties(&test_app(), 1, &ndk, Path::new("gen"), Path::new("gradle"));
        let mut passed: Vec<String> = passed
            .iter()
            .map(|property| {
                let property = property.to_str().unwrap().strip_prefix("-Psapphire.").unwrap();
                property.split_once('=').unwrap().0.to_owned()
            })
            .collect();
        let mut read: Vec<String> = gradle
            .split("sapphire(\"")
            .skip(1)
            .map(|rest| rest.split_once('"').unwrap().0.to_owned())
            .collect();
        passed.sort_unstable();
        read.sort_unstable();
        read.dedup();
        assert_eq!(passed, read);

        // The app's identity decides all of it: nothing in the project names one app.
        assert!(gradle.contains("namespace = sapphire(\"applicationId\")"));
        assert!(gradle.contains("applicationId = sapphire(\"applicationId\")"));
        assert!(gradle.contains("manifestPlaceholders[\"activity\"] = sapphire(\"activity\")"));
        assert!(gradle.contains("layout.buildDirectory.set(file(sapphire(\"buildDir\")))"));
        assert!(manifest.contains("android:name=\"${activity}\""));
        // The name and icon are resources xtask writes ([`write_resources`]).
        assert!(manifest.contains("android:label=\"@string/app_name\""));
        assert!(manifest.contains("android:icon=\"@mipmap/ic_launcher\""));
        assert!(gradle.contains("res.directories += \"$gen/res\""));
        for file in [gradle, manifest] {
            assert!(!file.to_lowercase().contains("sandbox"), "{file}");
        }
    }

    /// Notes and runbooks type the sandbox's launch line, so its identity must not move.
    #[test]
    fn each_app_has_its_own_activity() {
        for (app, class, component, name, source) in [
            (
                test_app(),
                "SandboxActivity",
                "dev.sapphire.sandbox/.SandboxActivity",
                "dev.sapphire.sandbox.SandboxActivity",
                "dev/sapphire/sandbox/SandboxActivity.java",
            ),
            (
                test_game(),
                "GameActivity",
                "com.example.sapphire.game/.GameActivity",
                "com.example.sapphire.game.GameActivity",
                "com/example/sapphire/game/GameActivity.java",
            ),
        ] {
            let activity = activity(&app);
            assert_eq!(activity.class, class);
            assert_eq!(activity.component(), component);
            assert_eq!(activity.name(), name);
            assert_eq!(activity.source_path(), Path::new(source));
        }
    }

    #[test]
    fn the_template_fills_for_each_app() {
        for (app, package, class, library) in [
            (test_app(), "dev.sapphire.sandbox", "SandboxActivity", "sapphire_sandbox"),
            (test_game(), "com.example.sapphire.game", "GameActivity", "game_app"),
        ] {
            let java = render_activity(ACTIVITY_SOURCE, &app).unwrap();
            assert!(java.starts_with(&format!("package {package};\n")), "{java}");
            assert!(java.contains(&format!("public final class {class} extends SDLActivity")), "{java}");
            assert!(java.contains(&format!("new String[] {{\"{library}\"}}")), "{java}");
            assert!(java.contains(&format!("{{@code lib{library}.so}}")), "{java}");
            // `cargo xtask run -- <args>` reaches every app the same way.
            assert!(java.contains(&format!("EXTRA_ARGS = \"{ARGS_EXTRA}\"")), "{java}");
            assert!(!java.contains("$("), "{java}");
        }
        assert!(!ACTIVITY_SOURCE.to_lowercase().contains("sandbox"), "the template names no app");
    }

    #[test]
    fn an_unknown_placeholder_in_the_template_fails() {
        let error = render_activity("package $(PACKAGE);\n$(TEAM)", &test_game()).unwrap_err();
        assert_eq!(error.to_string(), "unknown placeholder $(TEAM) in activity template");
    }

    #[test]
    fn writes_the_activity_where_its_package_says() {
        let dir = env::temp_dir().join(format!("xtask-activity-{}", std::process::id()));
        workspace::remove_dir_if_exists(&dir).unwrap();
        let template = dir.join("AppActivity.java.in");
        fs::create_dir_all(&dir).unwrap();
        fs::write(&template, ACTIVITY_SOURCE).unwrap();
        for app in [test_app(), test_game()] {
            write_activity(&template, &app, &dir.join("java")).unwrap();
        }
        let read = |path: &str| fs::read_to_string(dir.join("java").join(path)).unwrap();
        let sandbox = read("dev/sapphire/sandbox/SandboxActivity.java");
        let game = read("com/example/sapphire/game/GameActivity.java");
        workspace::remove_dir_if_exists(&dir).unwrap();
        assert!(sandbox.contains("class SandboxActivity"));
        assert!(game.contains("class GameActivity"));
    }

    #[test]
    fn gradle_gets_the_apps_identity() {
        let ndk = Ndk { root: "ndk".into(), revision: "29.0.1".into() };
        let properties = gradle_properties(&test_game(), 4123, &ndk, Path::new("gen"), Path::new("gradle"));
        let has = |expected: &str| properties.iter().any(|property| property == expected);
        assert!(has("-Psapphire.applicationId=com.example.sapphire.game"));
        assert!(has("-Psapphire.activity=com.example.sapphire.game.GameActivity"));
        assert!(has("-Psapphire.versionName=0.1.0"));
        assert!(has("-Psapphire.versionCode=4123"));
        assert!(has("-Psapphire.genDir=gen"));
        assert!(has("-Psapphire.buildDir=gradle"));
    }

    /// Names go into resources as written, whatever they hold.
    #[test]
    fn escapes_names_for_android_resources() {
        let string = |name: &str| {
            let xml = strings_xml(name);
            let start = xml.find("\"app_name\">").unwrap() + "\"app_name\">".len();
            xml[start..xml.find("</string>").unwrap()].to_owned()
        };
        assert_eq!(string("サファイア"), "サファイア");
        assert_eq!(string("Sword & Shield <1>"), "Sword &amp; Shield &lt;1&gt;");
        assert_eq!(string("Tom's \"Game\" \\o/"), "Tom\\'s \\\"Game\\\" \\\\o/");
        assert_eq!(string("@home?"), "\\@home?");
        assert_eq!(string("?quiz"), "\\?quiz");
    }

    #[test]
    fn each_gradle_task_says_what_it_builds() {
        assert_eq!(GradleTask::BundleRelease.output(), Path::new("outputs/bundle/release/app-release.aab"));
        assert_eq!(GradleTask::AssembleDebug.output(), Path::new("outputs/apk/debug/app-debug.apk"));
        assert_eq!(GradleTask::AssembleProfile.output(), Path::new("outputs/apk/profile/app-profile.apk"));
        assert_eq!(GradleTask::AssembleProfile.args(), ["assembleProfile"]);
        // The store build runs lint and lists bundletool's classpath, and never writes the upload key's
        // passwords into the configuration cache.
        let store = GradleTask::BundleRelease.args();
        assert!(store.contains(&"--no-configuration-cache") && store.contains(&"lintRelease"));
    }

    /// The store build type is the one `package` checks, the profiling one what `bundle --release` takes
    /// launch arguments in, and lint reads its configuration.
    #[test]
    fn the_store_build_is_release_and_profiling_builds_are_profile() {
        let gradle = include_str!("../../../platform/android/app/build.gradle.kts");
        let release = gradle.split("release {").nth(1).unwrap().split("create(\"profile\")").next().unwrap();
        assert!(release.contains("isDebuggable = false"));
        assert!(release.contains("debugSymbolLevel = \"FULL\""));
        assert!(
            release.contains("signingConfigs.findByName(\"upload\") ?: signingConfigs.getByName(\"debug\")")
        );
        let profile = gradle.split("create(\"profile\")").nth(1).unwrap();
        assert!(profile.contains("isDebuggable = true"));
        assert!(profile.contains("signingConfig = signingConfigs.getByName(\"debug\")"));
        assert!(gradle.contains(&format!("providers.environmentVariable(\"{}\")", aab::UPLOAD_KEYSTORE)));
        assert!(gradle.contains("lintConfig = file(\"lint.xml\")") && gradle.contains("abortOnError = true"));
        assert!(gradle.contains("tasks.register(\"bundletoolClasspath\")"));
        assert!(gradle.contains(&format!("file(\"{BUNDLETOOL_CLASSPATH}\")")));
    }

    /// Every value of `android:configChanges`, as the compile SDK lists them, but
    /// [`NOT_HANDLED_CONFIG_CHANGE`].
    const CONFIG_CHANGES: [&str; 19] = [
        "mcc",
        "mnc",
        "locale",
        "touchscreen",
        "keyboard",
        "keyboardHidden",
        "navigation",
        "orientation",
        "screenLayout",
        "uiMode",
        "screenSize",
        "smallestScreenSize",
        "density",
        "layoutDirection",
        "colorMode",
        "grammaticalGender",
        "fontScale",
        "fontWeightAdjustment",
        "assetsPaths",
    ];
    /// Says the activity uses no Android resources at all, and overrides every other value, so
    /// it isn't one more change to handle.
    const NOT_HANDLED_CONFIG_CHANGE: &str = "resourcesUnused";

    /// SDL exits the process when its activity is recreated, so a change the manifest doesn't
    /// list ends the run, with no crash report (review PN-1).
    #[test]
    fn the_activity_handles_every_configuration_change() {
        let manifest = include_str!("../../../platform/android/app/src/main/AndroidManifest.xml");
        let (_, rest) = manifest.split_once("android:configChanges=\"").expect("the activity lists them");
        let (listed, _) = rest.split_once('"').expect("the attribute ends");
        let listed: Vec<&str> = listed.split('|').collect();
        let missing: Vec<&str> = CONFIG_CHANGES.into_iter().filter(|value| !listed.contains(value)).collect();
        assert!(
            missing.is_empty(),
            "AndroidManifest.xml's configChanges leaves out {missing:?}: a change of one recreates the \
             activity, and SDL then exits the process"
        );
        let unknown: Vec<&str> = listed.into_iter().filter(|value| !CONFIG_CHANGES.contains(value)).collect();
        assert!(unknown.is_empty(), "add {unknown:?} to this test's list of configChanges values");
    }

    /// A value a new compile SDK adds fails here until it has been considered for the manifest.
    /// Machines without that SDK platform can't tell, and pass.
    #[test]
    fn the_list_of_configuration_changes_is_the_compile_sdks() {
        let gradle = include_str!("../../../platform/android/app/build.gradle.kts");
        let compile_sdk = gradle
            .lines()
            .find_map(|line| line.trim().strip_prefix("compileSdk = "))
            .expect("build.gradle.kts sets compileSdk");
        let Ok(sdk) = sdk_dir() else { return };
        let path = sdk.join(format!("platforms/android-{compile_sdk}/data/res/values/attrs_manifest.xml"));
        let Ok(attrs) = fs::read_to_string(&path) else { return };

        let (_, rest) = attrs.split_once("<attr name=\"configChanges\">").expect("the attribute is declared");
        let (flags, _) = rest.split_once("</attr>").expect("its declaration ends");
        let mut flags: Vec<&str> = flags
            .split("<flag name=\"")
            .skip(1)
            .map(|flag| flag.split_once('"').expect("the flag's name ends").0)
            .collect();
        flags.sort_unstable();
        let mut expected = CONFIG_CHANGES.to_vec();
        expected.push(NOT_HANDLED_CONFIG_CHANGE);
        expected.sort_unstable();
        assert_eq!(
            flags,
            expected,
            "{} lists other values: consider each for AndroidManifest.xml's configChanges and this test's list",
            path.display()
        );
    }
}
