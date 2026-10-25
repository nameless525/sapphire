//! Build automation for the Sapphire workspace (spec §7.4), run as `cargo xtask <command>`.
//!
//! Owns layer checking, content cooking, and building, bundling and launching the apps
//! per platform. Asset processing itself lives in `sapphire_pipeline`, which xtask runs
//! as a process, and golden-image tests (`golden`), which compare sandbox captures with
//! per-backend references. `package` (store artifacts) and `replay` come later.

mod cli;
mod content;
mod golden;
mod layers;
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
      Cook, build and assemble the app for <platform> under target/<platform>/:
        desktop   SapphireSandbox/ with the executable and content/ (any host)
        macos     SapphireSandbox.app, ad-hoc signed
        ios-sim   SapphireSandbox.app for the simulator, ad-hoc signed
        android   SapphireSandbox.apk, debug-signed (Gradle)
  run --platform <platform> [--release] [--features <list>] [--exe <path>] [--dry-run]
      [--show] [-- <app args>]
      Cook, build and launch, with the app's output streamed here:
        desktop   runs the build in place with SAPPHIRE_CONTENT_DIR set (any host)
        macos     bundles, then runs the executable inside the .app
        ios-sim   bundles, boots a simulator headless if none is booted, installs and
                  launches with the console attached
        android   bundles, uses the connected device or boots the sapphire_api36
                  emulator headless, installs, launches and streams the app's logcat
                  until it exits
  golden [--update] [--scene <name>]
      Render the golden scenes with the sandbox's offscreen capture (640x360, no window)
      and compare them with tests/golden/<scene>.<backend family>.png using FLIP.
      Failures leave .actual.png and .diff.png files under target/golden/. A scene with
      no golden for this backend family fails in CI and is skipped locally. --update
      writes this family's goldens instead.
  help
      Show this message.

Options:
  --locked       Pass --locked to every cargo command xtask runs, and fail the content
                 cook on a source without a committed .meta sidecar (CI).
  --features <list>
                 Build the app with these Cargo features (comma-separated), such as `dev`
                 for the debug overlay.
  --exe <path>   Use a prebuilt sapphire_sandbox executable instead of building one
                 (desktop, macos and ios-sim).
  --dry-run      Build and bundle, then print the launch commands instead of running them.
  --show         ios-sim: also show the simulator in Simulator.app.

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
        "golden" => {
            let update = args.flag("--update");
            let scene = args.value::<String>("--scene")?;
            args.finish(false)?;
            golden::run(update, scene.as_deref())
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
        ..Options::default()
    };
    Ok((platform, options))
}
