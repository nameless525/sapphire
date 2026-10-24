//! Build automation for the Sapphire workspace (spec §7.4), run as `cargo xtask <command>`.
//!
//! Owns layer checking, content cooking, and building, bundling and launching the apps
//! per platform. Asset processing itself lives in `sapphire_pipeline`, which xtask runs
//! as a process. `package` (store artifacts) and `golden` / `replay` come later.

mod cli;
mod content;
mod layers;
mod platforms;
mod workspace;

use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::bail;

use crate::content::Cook;
use crate::platforms::{Options, Platform, SANDBOX};

const USAGE: &str = "\
Usage: cargo xtask <command> [options]

Commands:
  check-layers
      Check crate layering against xtask/layers.toml (spec §4.1, rules R1 and R8).
  content [--cook desktop|mobile]
      Generate missing sample sources under content/, then cook content/ into
      target/content/<cook>/ (default cook: desktop).
  bundle --platform macos|ios-sim|android [--release] [--exe <path>]
      Cook, build and assemble target/<platform>/SapphireSandbox.app (ad-hoc signed),
      or for android target/android/SapphireSandbox.apk (Gradle, debug-signed).
  run --platform macos|ios-sim|android [--release] [--exe <path>] [-- <app args>]
      Cook, build and launch. macOS runs the binary with SAPPHIRE_CONTENT_DIR set;
      ios-sim bundles, boots a simulator if needed, installs and streams the console;
      android bundles, uses the connected device or boots the sapphire_api36 emulator
      headless, installs, launches and streams the app's logcat.
  help
      Show this message.

Options:
  --exe <path>   Use a prebuilt sapphire_sandbox executable instead of building one
                 (macos and ios-sim).

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
            let out = content::cook(cook)?;
            eprintln!("cooked content for {cook} into {}", out.display());
            Ok(())
        }
        "bundle" => {
            let (platform, options) = platform_options(&mut args)?;
            args.finish(false)?;
            platforms::bundle(platform, &SANDBOX, &options).map(drop)
        }
        "run" => {
            let (platform, options) = platform_options(&mut args)?;
            let app_args = args.finish(true)?;
            platforms::run(platform, &SANDBOX, &options, &app_args)
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
    let options = Options { release: args.flag("--release"), exe: args.value::<PathBuf>("--exe")? };
    Ok((platform, options))
}
