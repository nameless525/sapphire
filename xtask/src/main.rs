//! Build automation for the Sapphire workspace (spec §7.4), run as `cargo xtask <command>`.
//!
//! Owns layer checking, content cooking, and building, bundling and launching the apps
//! per platform. Asset processing itself lives in `sapphire_pipeline`, which xtask runs
//! as a process, and golden-image tests (`golden`), which compare sandbox captures with
//! per-backend references; live link's desktop tool (`link`) runs as a process too.
//! `package` (store artifacts) and `replay` come later.

mod cli;
mod content;
mod golden;
mod layers;
mod link;
mod platforms;
mod workspace;

use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::bail;

use crate::content::Cook;
use crate::platforms::{App, Options, Platform, SANDBOX};

const USAGE: &str = "\
Usage: cargo xtask [--locked] <command> [options]

Commands:
  check-layers
      Check crate layering against xtask/layers.toml (spec §4.1, rules R1 and R8).
  content [--cook desktop|mobile]
      Generate missing sample sources under content/, then cook content/ into
      target/content/loose/<cook>/ and pack it into target/content/<cook>/ (default cook:
      desktop). Only assets whose inputs changed are cooked or compressed again (cache:
      target/content/cache/). Sources without a .meta sidecar get one, to commit with them.
  bundle --platform <platform> [--release] [--features <list>] [--exe <path>]
      [--identity <name> --profile <file>]
      Cook, build and assemble the app for <platform> under target/<platform>/:
        desktop   SapphireSandbox/ with the executable and content/ (any host)
        macos     SapphireSandbox.app, ad-hoc signed
        ios-sim   SapphireSandbox.app for the simulator, ad-hoc signed
        ios       SapphireSandbox.app for devices, signed with --identity and --profile
                  (ad hoc without them, which no device installs)
        android   SapphireSandbox.apk, debug-signed (Gradle)
  run --platform <platform> [--release] [--features <list>] [--exe <path>] [--dry-run]
      [--show] [--identity <name> --profile <file> --device <name>] [-- <app args>]
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
  golden [--update] [--scene <name> | --shard <index>/<count>]
      Render the golden scenes with the sandbox's offscreen capture (640x360, no window)
      and compare them with tests/golden/<scene>.<backend family>.png using FLIP.
      Failures leave .actual.png and .diff.png files under target/golden/. A scene with
      no golden for this backend family fails in CI and is skipped locally. --update
      writes this family's goldens instead. --shard renders every <count>-th scene from
      the <index>-th (from 1); CI splits the slow backends' scenes across runners with it.
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
  help
      Show this message.

Options:
  --locked       Pass --locked to every cargo command xtask runs, and fail the content
                 cook on a source without a committed .meta sidecar (CI).
  --features <list>
                 Build the app with these Cargo features (comma-separated), such as `dev`
                 for the debug overlay.
  --exe <path>   Use a prebuilt sapphire_sandbox executable instead of building one
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
        "bundle" => {
            let (platform, options) = platform_options(&mut args)?;
            args.finish(false)?;
            platforms::bundle(platform, &App::load(SANDBOX)?, &options).map(drop)
        }
        "run" => {
            let (platform, mut options) = platform_options(&mut args)?;
            options.show = args.flag("--show");
            options.dry_run = args.flag("--dry-run");
            let app_args = args.finish(true)?;
            platforms::run(platform, &App::load(SANDBOX)?, &options, &app_args)
        }
        "link" => link::run(args.rest()),
        "golden" => {
            let update = args.flag("--update");
            let scene = args.value::<String>("--scene")?;
            let shard = args.value::<String>("--shard")?;
            args.finish(false)?;
            golden::run(update, scene.as_deref(), shard.as_deref())
        }
        "help" | "--help" | "-h" => {
            eprintln!("{USAGE}");
            Ok(())
        }
        other => bail!("unknown command `{other}`\n\n{USAGE}"),
    }
}

fn platform_options(args: &mut cli::Args) -> anyhow::Result<(Platform, Options)> {
    let platform = args.required::<Platform>("--platform")?;
    let features = args.value::<String>("--features")?.unwrap_or_default();
    let options = Options {
        release: args.flag("--release"),
        features: features.split(',').map(str::trim).filter(|f| !f.is_empty()).map(str::to_owned).collect(),
        exe: args.value::<PathBuf>("--exe")?,
        identity: args.value::<String>("--identity")?,
        profile: args.value::<PathBuf>("--profile")?,
        device: args.value::<String>("--device")?,
        ..Options::default()
    };
    Ok((platform, options))
}
