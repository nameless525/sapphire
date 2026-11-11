//! Build automation for the Sapphire workspace (spec §7.4), run as `cargo xtask <command>`.
//!
//! Owns layer checking, content cooking, and building, bundling and launching the apps
//! (`--app`, the sandbox by default) per platform. Asset processing itself lives in `sapphire_pipeline`, which xtask runs
//! as a process, and golden-image tests (`golden`), which compare sandbox captures with
//! per-backend references; live link's desktop tool (`link`) and the replay runner (`replay`) run
//! as processes too. `package` builds the game for a store: the shipping profile, one version and build
//! number, one identity and the project's icon, staged for each store's own package.

mod cli;
mod content;
mod editor;
mod golden;
mod layers;
mod link;
mod platforms;
mod release;
mod replay;
mod script_api;
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
        "link" => link::run(args.rest()),
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
        "script-api" => {
            let check = args.flag("--check");
            args.finish(false)?;
            script_api::run(check)
        }
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
