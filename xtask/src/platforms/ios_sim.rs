//! iOS Simulator: bundle a flat `.app`, boot a simulator headless, then install and
//! launch it with the app's console attached. `--show` also opens Simulator.app.
//!
//! M0 bundles the cargo-built executable directly. The spec's Xcode project (§7.3), which
//! device builds need for SDL3.xcframework, entitlements and provisioning, comes later.

use std::cmp::Reverse;
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::process::Command;

use anyhow::Context as _;
use serde::Deserialize;

use super::apple::{self, BundleLayout};
use super::{App, Options, Platform};
use crate::content::{self, Cook};
use crate::workspace;

const TARGET: &str = "aarch64-apple-ios-sim";
const IOS_RUNTIME_PREFIX: &str = "com.apple.CoreSimulator.SimRuntime.iOS-";
/// Matches `MinimumOSVersion` in `platform/ios/Info.plist` (spec §2).
const MINIMUM_IOS: (u32, u32) = (17, 0);

/// Builds `target/ios-sim/<Name>.app` with the mobile cook in `content/`.
pub fn bundle(app: &App, options: &Options) -> anyhow::Result<PathBuf> {
    super::require_macos_host(Platform::IosSim)?;
    let content = content::cook(Cook::Mobile)?;
    // No extra environment: the deployment target comes from .cargo/config.toml, and
    // cargo and CMake pick the simulator SDK for the target, so SDKROOT stays unset.
    let executable = super::executable(app, Some(TARGET), &[], options)?;
    let (template_path, template) = super::info_plist_template("ios")?;
    apple::verify_minimum_os(&executable, &template, "MinimumOSVersion")?;
    let info_plist = apple::render_info_plist(&template, app)
        .map_err(|e| e.context(format!("in {}", template_path.display())))?;

    let layout =
        BundleLayout::ios(super::bundle_dir(Platform::IosSim).join(format!("{}.app", app.bundle_name)));
    apple::assemble(&layout, app, &info_plist, &executable, &content.dir)?;
    apple::codesign_ad_hoc(&layout.root)?;
    eprintln!("bundled {}", super::display(&layout.root));
    Ok(layout.root)
}

/// Bundles, boots a simulator if none is running, installs, and launches with the app's
/// stdout and stderr streamed here until it exits.
pub fn run(app: &App, options: &Options, args: &[String]) -> anyhow::Result<()> {
    let bundle = bundle(app, options)?;
    let simulator = choose_simulator_or_fail()?;
    let mut install = Command::new("xcrun");
    install.args(["simctl", "install", &simulator.udid]).arg(&bundle);
    let mut launch = Command::new("xcrun");
    launch
        .args(["simctl", "launch", "--console-pty", "--terminate-running-process", &simulator.udid])
        .arg(&app.bundle_id)
        .args(args);
    if options.dry_run {
        for command in [&install, &launch] {
            eprintln!("dry run: {}", workspace::shell_line(command));
        }
        return Ok(());
    }

    boot(&simulator, options.show)?;
    workspace::run(&mut install)?;
    eprintln!(
        "launching {} on {} (iOS {}.{}); Ctrl-C detaches",
        app.bundle_id, simulator.name, simulator.runtime.0, simulator.runtime.1
    );
    workspace::run(&mut launch)
}

#[derive(Debug, Deserialize)]
struct DeviceList {
    /// Runtime identifier → devices.
    devices: BTreeMap<String, Vec<Device>>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Device {
    udid: String,
    name: String,
    state: String,
    #[serde(default)]
    is_available: bool,
}

#[derive(Debug, PartialEq, Eq)]
struct Simulator {
    udid: String,
    name: String,
    runtime: (u32, u32),
    booted: bool,
}

fn choose_simulator_or_fail() -> anyhow::Result<Simulator> {
    let json =
        workspace::output(Command::new("xcrun").args(["simctl", "list", "devices", "available", "--json"]))?;
    let list: DeviceList = serde_json::from_str(&json).context("unexpected `simctl list` output")?;
    choose_simulator(&list).with_context(|| {
        format!(
            "no iOS {}.{}+ simulator is available; install a runtime with `xcodebuild -downloadPlatform iOS`",
            MINIMUM_IOS.0, MINIMUM_IOS.1
        )
    })
}

/// Boots `simulator` headless if needed and waits until it has finished booting. With
/// `show`, also brings up its window in Simulator.app.
fn boot(simulator: &Simulator, show: bool) -> anyhow::Result<()> {
    if !simulator.booted {
        eprintln!("booting {} (iOS {}.{})...", simulator.name, simulator.runtime.0, simulator.runtime.1);
    }
    // `-b` boots the device if needed, then waits until it has finished booting. Its
    // once-a-second progress report is captured rather than shown.
    workspace::output(Command::new("xcrun").args(["simctl", "bootstatus", &simulator.udid, "-b"]))?;
    if show {
        workspace::run(Command::new("open").args([
            "-a",
            "Simulator",
            "--args",
            "-CurrentDeviceUDID",
            &simulator.udid,
        ]))?;
    }
    Ok(())
}

/// Picks a simulator: one that is already booted, else a recent iPhone (newest runtime,
/// newest generation, Pro before base models).
fn choose_simulator(list: &DeviceList) -> Option<Simulator> {
    list.devices
        .iter()
        .filter_map(|(runtime, devices)| Some((ios_runtime_version(runtime)?, devices)))
        .filter(|(version, _)| *version >= MINIMUM_IOS)
        .flat_map(|(runtime, devices)| {
            devices.iter().filter(|d| d.is_available).map(move |d| Simulator {
                udid: d.udid.clone(),
                name: d.name.clone(),
                runtime,
                booted: d.state == "Booted",
            })
        })
        .max_by_key(|s| {
            (
                s.booted,
                s.name.starts_with("iPhone"),
                s.runtime,
                iphone_generation(&s.name),
                s.name.contains("Pro"),
                Reverse(s.name.len()), // "iPhone 17 Pro" over "iPhone 17 Pro Max".
            )
        })
}

/// `com.apple.CoreSimulator.SimRuntime.iOS-26-3` → `(26, 3)`. Other OSes give `None`.
fn ios_runtime_version(runtime: &str) -> Option<(u32, u32)> {
    let mut parts = runtime.strip_prefix(IOS_RUNTIME_PREFIX)?.split('-').map(str::parse::<u32>);
    Some((parts.next()?.ok()?, parts.next().unwrap_or(Ok(0)).ok()?))
}

/// `iPhone 17 Pro` → 17, `iPhone 16e` → 16, `iPhone Air` → 0.
fn iphone_generation(name: &str) -> u32 {
    name.split_whitespace()
        .nth(1)
        .and_then(|word| word.trim_end_matches(|c: char| c.is_ascii_alphabetic()).parse().ok())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn device(name: &str, state: &str) -> serde_json::Value {
        serde_json::json!({ "udid": name.replace(' ', "-"), "name": name, "state": state, "isAvailable": true })
    }

    fn list(devices: serde_json::Value) -> DeviceList {
        serde_json::from_value(serde_json::json!({ "devices": devices })).unwrap()
    }

    #[test]
    fn prefers_a_recent_pro_iphone() {
        let list = list(serde_json::json!({
            "com.apple.CoreSimulator.SimRuntime.iOS-18-6": [device("iPhone 16 Pro", "Shutdown")],
            "com.apple.CoreSimulator.SimRuntime.iOS-26-3": [
                device("iPhone 17 Pro Max", "Shutdown"),
                device("iPhone 17 Pro", "Shutdown"),
                device("iPhone Air", "Shutdown"),
                device("iPhone 17", "Shutdown"),
                device("iPhone 16e", "Shutdown"),
                device("iPad Pro 13-inch (M5)", "Shutdown"),
            ],
            "com.apple.CoreSimulator.SimRuntime.watchOS-26-0": [device("Apple Watch Ultra 3", "Booted")],
        }));
        let chosen = choose_simulator(&list).unwrap();
        assert_eq!((chosen.name.as_str(), chosen.runtime, chosen.booted), ("iPhone 17 Pro", (26, 3), false));
    }

    #[test]
    fn reuses_a_booted_simulator() {
        let list = list(serde_json::json!({
            "com.apple.CoreSimulator.SimRuntime.iOS-26-3": [
                device("iPhone 17 Pro", "Shutdown"),
                device("iPad Air 11-inch (M3)", "Booted"),
            ],
        }));
        assert_eq!(choose_simulator(&list).unwrap().name, "iPad Air 11-inch (M3)");
    }

    #[test]
    fn ignores_old_and_unavailable_runtimes() {
        let mut unavailable = device("iPhone 17 Pro", "Shutdown");
        unavailable["isAvailable"] = false.into();
        let list = list(serde_json::json!({
            "com.apple.CoreSimulator.SimRuntime.iOS-16-4": [device("iPhone 14", "Booted")],
            "com.apple.CoreSimulator.SimRuntime.iOS-26-3": [unavailable],
        }));
        assert_eq!(choose_simulator(&list), None);
    }

    #[test]
    fn minimum_matches_the_info_plist() {
        let template = include_str!("../../../platform/ios/Info.plist");
        let minimum = apple::plist_string(template, "MinimumOSVersion").unwrap();
        assert_eq!(minimum, format!("{}.{}", MINIMUM_IOS.0, MINIMUM_IOS.1));
    }

    #[test]
    fn parses_runtime_and_model_names() {
        assert_eq!(ios_runtime_version("com.apple.CoreSimulator.SimRuntime.iOS-26-3"), Some((26, 3)));
        assert_eq!(ios_runtime_version("com.apple.CoreSimulator.SimRuntime.iOS-18"), Some((18, 0)));
        assert_eq!(ios_runtime_version("com.apple.CoreSimulator.SimRuntime.tvOS-26-0"), None);
        assert_eq!(iphone_generation("iPhone 17 Pro"), 17);
        assert_eq!(iphone_generation("iPhone 16e"), 16);
        assert_eq!(iphone_generation("iPhone Air"), 0);
    }
}
