//! Build automation for the Sapphire workspace (spec §7.4), run as `cargo xtask <command>`.
//!
//! Owns layer checking, content cooking, and building, bundling and launching the apps
//! (`--app`, the sandbox by default) per platform. Asset processing itself lives in `sapphire_pipeline`, which xtask runs
//! as a process, and golden-image tests (`golden`), which compare sandbox captures with
//! per-backend references; live link's desktop tool (`link`) and the replay runner (`replay`) run
//! as processes too. `package` builds the game for a store: the shipping profile, one version and build
//! number, one identity and the project's icon, staged for each store's own package; `symbolicate` reads its
//! crashes back with the symbols kept beside it, `symbols` writes their whole Breakpad store, and
//! `store-shots` draws the stores' screenshots by the goldens' road. `patch` serves a release from this
//! machine and runs Exit test 2's lane (M4 P4).

mod cli;
mod content;
mod editor;
mod golden;
mod layers;
mod link;
mod patch;
mod platforms;
mod playtest;
mod release;
mod replay;
mod script_api;
mod store_shots;
mod version;
mod workspace;

use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::{Context as _, bail};

use crate::content::Cook;
use crate::platforms::{App, Options, PackageTarget, Platform, SANDBOX};

const USAGE: &str = "\
Usage: cargo xtask [--locked] <command> [options]

Commands:
  check-layers
      Check crate layering against xtask/layers.toml (spec §4.1, rules R1 and R8).
  content [--cook desktop|mobile|editor]
      Generate missing sample sources under content/, then cook content/ into
      target/content/loose/<cook>/ and pack it into target/content/<cook>/ (default cook:
      desktop). Only assets whose inputs changed are cooked or compressed again (cache:
      target/content/cache/). Sources without a .meta sidecar get one, to commit with them.
      `editor` is the desktop cook with what only an editor reads (each zone's shapes), in
      target/content/editor/, which no app ships.
  release --cook desktop|mobile --out <dir> --release-version <n> --expires <YYYY-MM-DD>
      --min-build <n> [--next-keys <hex>[,<hex>...]] [--patch-from <manifest>]
      Cook, then sign the cook's release with the keys in SAPPHIRE_RELEASE_KEY (each a 32-byte
      ed25519 seed in hex, comma-separated; only the maintainer has them) and lay it out in <dir>
      for a static host: packs/ and files/ by hash, manifests/<n>.smanifest, and release.smanifest,
      written last and copied to a host last. <n> must be higher than <dir>'s current release's.
      With --patch-from, the release is a patch of the one that signed manifest is: refused,
      naming the file, if it changes anything the simulation reads, and unless an install of the
      earlier release would take it. Nothing is uploaded.
  release-key new <file>
      Write a new release key to <file>, which mustn't be there: an ed25519 seed from the OS's
      generator, in hex, readable only by its owner, as SAPPHIRE_RELEASE_KEY takes it
      (SAPPHIRE_RELEASE_KEY=$(cat <file>)). Prints its public half as platform/patch.toml's
      release-keys takes it. The key is the maintainer's: keep <file> offline, never in the
      repository.
  release-key public <file>
      Print the public half of the release key in <file>, as platform/patch.toml's release-keys
      takes it (--next-keys takes the quoted digits). The seed is never printed.
  patch serve <dir> [--port <n>]
      Serve a release directory laid out by `release` at http://127.0.0.1:<n>/ (default 8475):
      GET with byte ranges, kept alive, each request and its bytes printed. The iOS Simulator
      reaches it there, the Android emulator after `adb reverse tcp:<n> tcp:<n>`, and a dev
      build of the game with `--update-host http://127.0.0.1:<n>/` (M4 P4).
  patch rehearse [--out <dir>]
      Exit test 2's lane (M4 P4): cook release 1 and, with one screen's edit, release 2 as its
      patch, both signed with the test key's public seed; then the game (dev, no window) patches
      from release 1 in its boot, stands on release 2 again at its next launch, refuses a release
      signed by a key it doesn't trust, and resumes a download killed mid-block. Fails unless
      each draws what it should; the bytes each fetched go to <dir>/report.txt (default
      target/patch-rehearsal/).
  bundle --platform <platform> [--app <package>] [--release | --incremental] [--features <list>]
      [--exe <path>] [--identity <name> --profile <file>]
      Cook, build and assemble the app for <platform> under target/<platform>/, by the
      bundle-name in its Cargo.toml (<Name>, SapphireSandbox for the sandbox):
        desktop   <Name>/ with the executable and content/ (any host)
        macos     <Name>.app, ad-hoc signed
        ios-sim   <Name>.app for the simulator, ad-hoc signed
        ios       <Name>.app for devices, signed with --identity and --profile
                  (ad hoc without them, which no device installs)
        android   <Name>.apk, debug-signed (Gradle)
  run --platform <platform> [--app <package>] [--release | --incremental] [--features <list>]
      [--exe <path>] [--dry-run] [--show] [--identity <name> --profile <file> --device <name>]
      [-- <app args>]
      Cook, build and launch, with the app's output streamed here:
        desktop   runs the build in place with SAPPHIRE_CONTENT_DIR set (any host)
        macos     bundles, then runs the executable inside the .app
        ios-sim   bundles, boots a simulator headless if none is booted, installs and
                  launches with the console attached
        ios       bundles and signs, installs on --device and launches it there with
                  the console attached (devicectl, Xcode 15+)
        android   bundles, uses the connected device or boots the sapphire_api36
                  emulator headless, installs, launches and streams the app's logcat
                  until it exits
  package --platform <platform> [--out <dir>] [--dev]
      [--release-version <n> --expires <YYYY-MM-DD> --min-build <n> [--next-keys <hex>[,<hex>...]]
      [--patch-from <manifest>]]
      Build the game for a store under <dir>/<platform>/ (default target/package/): game_app in the
      shipping profile (fat LTO: minutes, and gigabytes), the platform's cook, the workspace's
      version and a build number counted from git, the identity in platform/identity.toml and
      the icon from platform/icon/icon.png, with package.json saying what it is and symbols/
      holding what the link split out:
        ios       <Name>.xcarchive: the app for devices, with actool's Assets.car, the store's
                  Info.plist keys, its privacy manifest and entitlements, signed ad hoc, and its
                  dSYM, checked offline (checks/)
        ios-sim   the same app for the Simulator, with no archive
        macos     <Name>.app with Assets.car and AppIcon.icns, signed ad hoc with the hardened
                  runtime and no entitlements, verified, read back (checks/)
        android   <Name>.aab (Gradle's bundleRelease, offline), held to release lint and checked
                  with bundletool, aapt2 and zipalign; checks/ has the local-testing APKs.
                  Signed with the upload key if SAPPHIRE_UPLOAD_KEYSTORE names it (with
                  SAPPHIRE_UPLOAD_KEYSTORE_PASSWORD, SAPPHIRE_UPLOAD_KEY_ALIAS and
                  SAPPHIRE_UPLOAD_KEY_PASSWORD), else with the debug key, which Play refuses
        windows   <Name>/ with the executable and content (a Windows host): the executable a GUI
                  program with platform/windows/game_app.manifest, the icon and VERSIONINFO
                  linked in, read back (checks/)
        linux     <Name>/ with the executable, content, icon and .desktop file (a Linux host,
                  in the Steam Linux Runtime 3.0 SDK): fails if the executable needs a library
                  or symbol version the runtime lacks (checks/)
      The build number is the count of commits on main at HEAD. Off main, with changes not
      committed or untracked files, it fails, unless --dev packages it anyway, marked dev in its
      build ID and package.json. A shallow clone always fails. Nothing is uploaded or notarized,
      and nothing but Android's upload key signs for a store. With a release's options, the
      content's release is signed as `release` signs it (SAPPHIRE_RELEASE_KEY), laid out for its
      host in release/, and the app carries the signed manifest; without them the app keeps the
      cook's unsigned one, and package.json says so.
  steampipe [--packages <dir>]
      Write Steam's depots as SteamPipe scripts in <dir>/steam/ (default target/package/) from the
      windows, linux and macos packages there, built from one commit: an app build, a depot build
      for each package found, and depots.txt with each depot's files and launch option. The IDs
      are platform/steam.toml's (placeholders until the maintainer's). Nothing is uploaded: the
      maintainer runs `steamcmd +login <account> +run_app_build <app build> +quit`.
  symbolicate [--package <dir>]... [--store <dir>] <report>...
      Read crash reports offline with the symbols kept beside each package (every package under
      target/package/ unless --package names them), found by each module's debug file and ID in
      package.json: a desktop minidump (<run>.dmp), a panic report (<run>.panic-<n>.txt, iOS's
      too), an Android signal record (<run>.native.txt) or tombstone (<run>.tombstone.pb).
      Prints each thread's frames with function, file and line, inlined calls included, with
      the paths a shipping build remaps (/sapphire, /cargo) mapped back to this machine's.
      The .sym files it makes are kept in <dir> (default target/symbols/), a Breakpad store.
  symbols [--package <dir>]... [--store <dir>]
      Write the Breakpad .sym of every module the packages shipped (every package under
      target/package/ unless --package names them) into <dir> (default target/symbols/), from
      the symbols kept beside each: the whole store, where symbolicate writes only what a crash
      names. A .sym the store has already is kept.
  editor [<file.moves.ron>] [--link <address:port>]
      Cook, build and open the combat timeline editor. It opens a window, for a person at the
      machine; `--capture <png>` draws a frame with none (docs/development-iteration.md).
  golden [--update] [--scene <name> | --shard <index>/<count>] [--scene-timeout <seconds>]
      Render the golden scenes with the sandbox's offscreen capture (640x360, no window)
      and compare them with tests/golden/<scene>.<backend family>.png using FLIP.
      Failures leave .actual.png and .diff.png files under target/golden/. A scene with
      no golden for this backend family fails in CI and is skipped locally. --update
      writes this family's goldens instead. --shard renders slice <index> of <count>
      (from 1), split by the scenes' CI times on the backend family SAPPHIRE_GPU_BACKEND
      forces; CI splits the slow backends' scenes across runners with it. A scene still
      running after --scene-timeout (default 900) is stopped and fails. Each scene's
      output is in target/golden/<scene>.log, and their times in timings.json there.
  store-shots [--scene-timeout <seconds>]
      Draw the stores' screenshots into target/store-shots/<store>/<size>/<language>/, which is
      never committed: golden scenes of the game in play, by the goldens' sandbox capture, at each
      size docs/store/<store>.md's Screenshots table names (Apple's iPhones and 13-inch iPad, Play's
      phone and tablet, Steam's desktop and the Deck's 1280x800), each as that screen shows the
      menus and the HUD, in English and Japanese. Captures' logs are in target/store-shots/logs/.
  link <command> [options]
      Live link (spec §5.29): build the desktop tool and run it against a running dev build
      (`--features dev`), with this workspace's cooked content:
        status               the device's app, cook, frame time, assets and their generations
        cvar <name> <value>  set a cvar
        push [<file>...]     push packs (only the assets the device lacks are sent), RON
                             documents and WGSL sources; with no files, the packs of the
                             device's cook in target/content/<cook>/
        capture <png>        write the next frame the device draws to <png>
        watch [<doc>...]     push what changes: packs after `cargo xtask content --cook
                             <cook>`, the renderer's WGSL sources, the default day, the
                             light rig and screen effect presets, and the RON documents named
      Each command says how long the device took to answer.
      --addr <host:port> is the device (default 127.0.0.1:7811: desktop, the iOS Simulator,
      `iproxy` and `adb forward`); --listen <host:port> waits for a device that dials, behind
      `adb reverse` (the app's `--live-link connect=<addr>`).
  playtest --platform <platform> [--features <list>] [--dry-run]
      [--identity <name> --profile <file> --device <name>] [-- <app args>]
      Build the game (game_app) for release with its `playtest` feature, which keeps a session of
      each time someone plays under the player's directory (`sessions/`), then install and launch
      it as `run` does. No store build has the feature (xtask/layers.toml).
  playtest pull --platform <platform> [--from <dir>] [--device <id>] [--no-replay]
      Copy every session from the device to target/playtest/<platform>/sessions/, and replay each
      run in it to the device's checksums; a failure names the first tick that differs.
        desktop, macos  the player's directory here, or --from (a --save-dir)
        ios-sim         the booted simulator, or --device <UDID>
        android         ANDROID_SERIAL's device or the one connected, or --device <serial>,
                        through `run-as` (a playtest build is debuggable)
        ios             --device <name or UDID>, through devicectl
  playtest report <session dir>
      Replay a session's runs here and read them out per encounter: time, damage taken and falls,
      dodges, perfect dodges and counters, presses a second and idle time, frame times and heat,
      and each mark with the command that draws its moment. Prints it, and writes report.ron
      beside the session.
  replay [--update] [--replay <name>]
      Run the recorded combat replays in tests/replays/ with no window or GPU (spec §9, gates 2
      and 4): each must reach this architecture's checksums (checksums.<arch>.txt) with no heap
      allocation in the fixed step. A failure names the first tick that differs. --update takes
      this architecture's new values, leaving every other architecture's file alone, and stamps
      a changed move's hash into the replays: then review what moved, as with a golden.
      target/replays/ always holds this architecture's file as it should be.
  replay --record <name> --scenario <name> --script <name> [--ticks <n>] [--seed <n>]
      Record a new tests/replays/<name>.sreplay from a registered script, and this architecture's
      checksums for it.
  script-api [--check]
      Make crates/sapphire_script's wrappers and sapphire.d.luau from its api.toml, or fail if what's
      checked in isn't what it makes (spec §5.24).
  replay --list
      List the scenarios, scripts and replays.
  help
      Show this message.

Options:
  --app <package>
                 run, bundle: the app, a workspace package with a table in
                 platform/identity.toml (default: sapphire_sandbox, the only app `golden` runs).
                 `package` packages game_app, and takes none.
  --locked       Pass --locked to every cargo command xtask runs, and fail the content
                 cook on a source without a committed .meta sidecar (CI).
  --incremental  desktop, macos: build the app optimized, with no LTO and incrementally (the
                 tools profile), so a one-line edit rebuilds in seconds, not minutes.
  --features <list>
                 Build the app with these Cargo features (comma-separated), such as `dev`
                 for the debug overlay.
  --exe <path>   Use a prebuilt executable of the app instead of building one
                 (desktop, macos, ios-sim and ios).
  --dry-run      Build and bundle, then print the launch commands instead of running them.
  --show         ios-sim: also show the simulator in Simulator.app.
  --identity <name or SHA-1>, --profile <file.mobileprovision>
                 ios: the Apple Development signing identity (`security find-identity -v
                 -p codesigning`) and a development provisioning profile covering the
                 bundle identifier and the device.
  --device <name or UDID>
                 ios: the device to run on (`xcrun devicectl list devices`).

Android environment:
  ANDROID_HOME      SDK location (default: ~/Library/Android/sdk, ~/Android/Sdk on Linux).
  ANDROID_NDK_HOME  NDK r28+ (default: the newest NDK in the SDK).
  JAVA_HOME         JDK 17+ for Gradle (default: `java` in PATH).
  ANDROID_SERIAL    The device to use when several are connected.";

fn main() -> ExitCode {
    match run(cli::Args::new(std::env::args().skip(1))) {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("error: {err:#}");
            ExitCode::FAILURE
        }
    }
}

fn run(mut args: cli::Args) -> anyhow::Result<()> {
    workspace::set_locked(args.flag("--locked"));
    let Some(command) = args.command() else {
        bail!("no command given\n\n{USAGE}");
    };
    match command.as_str() {
        "check-layers" => {
            args.finish(false)?;
            layers::run()
        }
        "content" => {
            let cook = args.value::<Cook>("--cook")?.unwrap_or(Cook::Desktop);
            args.finish(false)?;
            let cooked = content::cook(cook)?;
            eprintln!("cooked content for {cook} into {}", cooked.dir.display());
            Ok(())
        }
        "release" => {
            let cook = args.required::<Cook>("--cook")?;
            let out = args.required::<PathBuf>("--out")?;
            let release = release::Release::from_args(&mut args)?
                .context("a release needs --release-version, --expires and --min-build")?;
            args.finish(false)?;
            if cook == Cook::Editor {
                bail!("the editor's cook ships nowhere, so it has no release");
            }
            let cooked = content::cook(cook)?;
            let manifest = release::publish(&cooked, &out, &release)?;
            eprintln!("released {cook} content as {}", manifest.display());
            Ok(())
        }
        "release-key" => {
            let rest = args.rest();
            let [action, file] = rest.as_slice() else {
                bail!("release-key takes `new <file>` or `public <file>`");
            };
            release::key(action.parse()?, std::path::Path::new(file))
        }
        "bundle" => {
            let package = app_package(&mut args)?;
            let (platform, options) = platform_options(&mut args)?;
            args.finish(false)?;
            platforms::bundle(platform, &App::load(&package)?, &options).map(drop)
        }
        "run" => {
            let package = app_package(&mut args)?;
            let (platform, mut options) = platform_options(&mut args)?;
            options.show = args.flag("--show");
            options.dry_run = args.flag("--dry-run");
            let app_args = args.finish(true)?;
            platforms::run(platform, &App::load(&package)?, &options, &app_args)
        }
        "package" => {
            let target = args.required::<PackageTarget>("--platform")?;
            let out = args.value::<PathBuf>("--out")?.unwrap_or_else(platforms::default_package_dir);
            let dev = args.flag("--dev");
            let release = release::Release::from_args(&mut args)?;
            args.finish(false)?;
            let packaged = platforms::package(target, &out, dev, release.as_ref())?;
            eprintln!("packaged into {}", packaged.display());
            Ok(())
        }
        "steampipe" => {
            let packages =
                args.value::<PathBuf>("--packages")?.unwrap_or_else(platforms::default_package_dir);
            args.finish(false)?;
            let written = platforms::steampipe(&packages)?;
            eprintln!("wrote SteamPipe's scripts into {}", written.display());
            Ok(())
        }
        "symbolicate" => {
            let mut packages = Vec::new();
            while let Some(package) = args.value::<PathBuf>("--package")? {
                packages.push(package);
            }
            if packages.is_empty() {
                packages = made_packages()?;
            }
            let store = args.value::<PathBuf>("--store")?.unwrap_or_else(platforms::default_symbol_store);
            let reports: Vec<PathBuf> = args.rest().into_iter().map(PathBuf::from).collect();
            if let Some(flag) = reports.iter().find(|report| report.to_string_lossy().starts_with("--")) {
                bail!("unexpected argument `{}`", flag.display());
            }
            if reports.is_empty() {
                bail!("symbolicate needs a report to read\n\n{USAGE}");
            }
            platforms::symbolicate(&packages, &store, &reports)
        }
        "symbols" => {
            let mut packages = Vec::new();
            while let Some(package) = args.value::<PathBuf>("--package")? {
                packages.push(package);
            }
            let store = args.value::<PathBuf>("--store")?.unwrap_or_else(platforms::default_symbol_store);
            args.finish(false)?;
            if packages.is_empty() {
                packages = made_packages()?;
            }
            platforms::write_symbol_store(&packages, &store)
        }
        "link" => link::run(args.rest()),
        "patch" => patch::run(args.rest()),
        "editor" => editor::run(args.rest()),
        "golden" => {
            let update = args.flag("--update");
            let scene = args.value::<String>("--scene")?;
            let shard = args.value::<String>("--shard")?;
            let scene_timeout = args.value::<u64>("--scene-timeout")?;
            args.finish(false)?;
            golden::run(&golden::Golden {
                update,
                scene: scene.as_deref(),
                shard: shard.as_deref(),
                scene_timeout,
            })
        }
        "store-shots" => {
            let scene_timeout = args.value::<u64>("--scene-timeout")?;
            args.finish(false)?;
            store_shots::run(scene_timeout)
        }
        "script-api" => {
            let check = args.flag("--check");
            args.finish(false)?;
            script_api::run(check)
        }
        "playtest" => playtest::run(&playtest::Command::parse(args)?),
        "replay" => {
            let command = replay::Command::parse(&mut args)?;
            args.finish(false)?;
            replay::run(&command)
        }
        "help" | "--help" | "-h" => {
            eprintln!("{USAGE}");
            Ok(())
        }
        other => bail!("unknown command `{other}`\n\n{USAGE}"),
    }
}

/// Every package made under the default directory, which `symbolicate` reads symbols from unless told.
fn made_packages() -> anyhow::Result<Vec<PathBuf>> {
    let dir = platforms::default_package_dir();
    let entries = std::fs::read_dir(&dir)
        .map_err(|error| anyhow::anyhow!("no packages in {}: {error}; pass --package", dir.display()))?;
    let mut packages: Vec<PathBuf> = entries
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.join("package.json").is_file())
        .collect();
    packages.sort();
    Ok(packages)
}

/// The app `run` or `bundle` builds.
fn app_package(args: &mut cli::Args) -> anyhow::Result<String> {
    Ok(args.value::<String>("--app")?.unwrap_or_else(|| SANDBOX.to_owned()))
}

fn platform_options(args: &mut cli::Args) -> anyhow::Result<(Platform, Options)> {
    let platform = args.required::<Platform>("--platform")?;
    let features = args.value::<String>("--features")?.unwrap_or_default();
    let options = Options {
        release: args.flag("--release"),
        incremental: args.flag("--incremental"),
        features: features.split(',').map(str::trim).filter(|f| !f.is_empty()).map(str::to_owned).collect(),
        exe: args.value::<PathBuf>("--exe")?,
        identity: args.value::<String>("--identity")?,
        profile: args.value::<PathBuf>("--profile")?,
        device: args.value::<String>("--device")?,
        ..Options::default()
    };
    Ok((platform, options))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(line: &str) -> cli::Args {
        cli::Args::new(line.split_whitespace().map(str::to_owned))
    }

    #[test]
    fn the_app_is_the_sandbox_unless_named() {
        assert_eq!(app_package(&mut args("--platform android")).unwrap(), SANDBOX);
        for line in ["--app game_app --platform android", "--platform android --app=game_app"] {
            let mut args = args(line);
            assert_eq!(app_package(&mut args).unwrap(), "game_app");
            assert_eq!(args.required::<String>("--platform").unwrap(), "android");
            args.finish(false).unwrap();
        }
        let error = app_package(&mut args("--platform android --app")).unwrap_err();
        assert_eq!(error.to_string(), "--app needs a value");
    }

    /// Past `--` it's the app's argument, not this one's.
    #[test]
    fn an_app_argument_named_app_stays_the_apps() {
        let mut args = args("--platform android -- --app other");
        assert_eq!(app_package(&mut args).unwrap(), SANDBOX);
        args.required::<String>("--platform").unwrap();
        assert_eq!(args.finish(true).unwrap(), ["--app", "other"]);
    }

    /// Goldens are the sandbox's scenes on the sandbox's flags.
    #[test]
    fn golden_takes_no_app() {
        let error = run(args("golden --app game_app")).unwrap_err();
        assert_eq!(error.to_string(), "unexpected argument `--app`");
    }
}
