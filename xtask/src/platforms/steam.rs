//! Steam's desktop packages (M4 S4): what `cargo xtask package` adds to the Windows, Linux and macOS
//! packages for Steam, and the checks each passes offline before it's published. The depots are made from
//! the three packages by [`super::steampipe()`].
//!
//! | Platform | Added | Checked (`checks/`) |
//! |---|---|---|
//! | `windows` | the manifest, VERSIONINFO and the icon, linked by `game_app`'s build script; a GUI program | the resources and subsystem read back from the executable ([`super::pe`]): `windows-resources.txt` |
//! | `linux` | `<bundle id>.desktop` beside the executable and its icon | every library and symbol version it needs is one Steam Linux Runtime 3.0 has ([`super::elf`]): `linux-needs.txt` |
//! | `macos` | an ad hoc signature with the hardened runtime and [`MACOS_ENTITLEMENTS`] (none) | `codesign --verify --strict --deep`, the runtime flag, the entitlements read back, and `AppIcon.icns` named by Info.plist: `codesign.txt`, `entitlements.plist`, `signature.txt` |
//!
//! Nothing here signs for a store, notarizes or uploads: Windows' Authenticode, macOS's Developer ID and
//! notarization, and `steamcmd` are the maintainer's (m4-plan §7).

use std::ffi::OsString;
use std::fs;
use std::path::Path;
use std::process::Command;

use anyhow::{Context as _, bail, ensure};
use serde_json::json;

use super::App;
use super::apple::{self, BundleLayout, Runtime};
use super::elf::{self, Needs};
use super::package::Target;
use super::pe::{self, Executable, VersionInfo};
use crate::version::Build;
use crate::workspace;

/// The entitlements macOS's package is signed with: none (the file says why).
pub const MACOS_ENTITLEMENTS: &str = "platform/macos/package.entitlements";
/// The application manifest `game_app`'s build script links into the Windows executable.
pub const WINDOWS_MANIFEST: &str = "platform/windows/game_app.manifest";
/// The icon a Windows package's build links in, which `game_app`'s build script reads.
const WINDOWS_ICON_ENV: &str = "SAPPHIRE_WINDOWS_ICON";

/// The newest symbol version Steam Linux Runtime 3.0 ("sniper") has of each family a binary can need: its
/// glibc is Debian 11's 2.31 (`libc6 2.31-13+deb11u14` in the runtime's package manifest, read 2026-11-10
/// from repo.steampowered.com/steamrt3/images), and its libstdc++ and libgcc_s are GCC 10.3's
/// (`libstdc++6 10.3.0-3+steamrt3.1`), whose newest labels are `GLIBCXX_3.4.28`, `CXXABI_1.3.12` and
/// `GCC_9.0.0` (GCC's libstdc++ manual, "ABI Policy and Guidelines", read 2026-11-10). Its ALSA library
/// is alsa-lib 1.2.4 (`libasound2 1.2.4-1.1+deb11u1`, the same manifest), whose labels name the release
/// that added a symbol (`ALSA_0.9.0rc4`, a release candidate, counts as its release).
const RUNTIME_VERSIONS: [(&str, &[u32]); 5] = [
    ("GLIBC_", &[2, 31]),
    ("GLIBCXX_", &[3, 4, 28]),
    ("CXXABI_", &[1, 3, 12]),
    ("GCC_", &[9, 0, 0]),
    ("ALSA_", &[1, 2, 4]),
];

/// The libraries a Linux build may name: glibc's, GCC's runtime libraries, and ALSA's, which cpal links
/// (`alsa-sys`), all in the runtime. SDL is linked statically, and Vulkan, X11, Wayland, udev and the
/// sound servers are opened at run time, never linked. A library not here needs checking against the
/// runtime's manifest before it's added.
const RUNTIME_LIBRARIES: [&str; 9] = [
    "libc.so.6",
    "libm.so.6",
    "libdl.so.2",
    "libpthread.so.0",
    "librt.so.1",
    "ld-linux-x86-64.so.2",
    "libgcc_s.so.1",
    "libstdc++.so.6",
    "libasound.so.2",
];

/// Whether a package is one of Steam's.
pub fn ships_on_steam(target: Target) -> bool {
    matches!(target, Target::Windows | Target::Linux | Target::MacOs)
}

/// What a package's build adds to Cargo's environment: Windows' icon, from the package's icons.
pub fn build_env(target: Target, icons: &Path) -> Vec<(&'static str, OsString)> {
    match target {
        Target::Windows => vec![(WINDOWS_ICON_ENV, icons.join("windows").join("app.ico").into_os_string())],
        _ => Vec::new(),
    }
}

/// Signs macOS's bundle for Steam: ad hoc, with the hardened runtime and no entitlements, verified, with
/// the runtime flag read back (`checks/signature.txt`). The maintainer's Developer ID signature and
/// notarization replace it (m4-plan §7).
pub fn sign_macos(layout: &BundleLayout, checks: &Path) -> anyhow::Result<()> {
    apple::sign_for_store(&layout.root, MACOS_ENTITLEMENTS, Runtime::Hardened, checks)?;
    let shown = Command::new("codesign").args(["--display", "--verbose=2"]).arg(&layout.root).output();
    let shown = shown.context("failed to run `codesign --display`")?;
    ensure!(shown.status.success(), "`codesign --display` failed on {}", layout.root.display());
    // codesign describes a signature on stderr.
    let text = String::from_utf8_lossy(&shown.stderr).into_owned();
    let flags = signature_flags(&text).context("`codesign --display` named no flags")?;
    ensure!(flags.contains(&"runtime"), "the bundle isn't signed with the hardened runtime: flags {flags:?}");
    let package = checks.parent().context("checks/ is in the package")?;
    let kept = text.replace(&format!("{}/", package.display()), "");
    fs::write(checks.join("signature.txt"), kept).context("failed to write checks/signature.txt")
}

/// The names in `codesign --display`'s `flags=0x10002(adhoc,runtime)`.
fn signature_flags(text: &str) -> Option<Vec<&str>> {
    let flags = text.split_whitespace().find_map(|word| word.strip_prefix("flags="))?;
    let names = flags.split_once('(')?.1.strip_suffix(')')?;
    Some(names.split(',').collect())
}

/// Finishes and checks a Steam package staged at `stage`, and returns what its checks found, for
/// `package.json`'s `checks`. Linux gets its `.desktop` file here.
pub fn finish(target: Target, app: &App, build: &Build, stage: &Path) -> anyhow::Result<serde_json::Value> {
    let checks = stage.join("checks");
    fs::create_dir_all(&checks).with_context(|| format!("failed to create {}", checks.display()))?;
    match target {
        Target::Windows => {
            let path = stage.join(&app.bundle_name).join(format!("{}.exe", app.package));
            let bytes = fs::read(&path).with_context(|| format!("failed to read {}", path.display()))?;
            let manifest = fs::read(workspace::root().join(WINDOWS_MANIFEST))
                .with_context(|| format!("failed to read {WINDOWS_MANIFEST}"))?;
            let expected = WindowsExpected {
                manifest: &manifest,
                version: version_parts(&app.version)?,
                number: u16::try_from(build.number)
                    .context("the build number is past VERSIONINFO's 16 bits")?,
                id: &build.id(&app.version),
                name: &app.display_name.en,
            };
            let exe = Executable::read(&bytes).with_context(|| format!("in {}", path.display()))?;
            let found = check_windows(&exe, &expected).with_context(|| format!("in {}", path.display()))?;
            fs::write(checks.join("windows-resources.txt"), &found.report)
                .context("failed to write checks/windows-resources.txt")?;
            Ok(json!({
                "subsystem": "gui",
                "manifest": WINDOWS_MANIFEST,
                "file_version": found.file_version,
                "product_version": build.id(&app.version),
                "icon_images": found.icon_images,
            }))
        }
        Target::Linux => {
            let dir = stage.join(&app.bundle_name);
            let icon = format!("{}.png", app.bundle_id);
            ensure!(dir.join(&icon).is_file(), "the Linux package has no {icon} for its .desktop file");
            let entry = format!("{}.desktop", app.bundle_id);
            fs::write(dir.join(&entry), desktop_entry(app))
                .with_context(|| format!("failed to write {entry}"))?;
            let path = dir.join(&app.package);
            let bytes = fs::read(&path).with_context(|| format!("failed to read {}", path.display()))?;
            let needs = Needs::read(&bytes).with_context(|| format!("in {}", path.display()))?;
            let report = check_linux(&needs).with_context(|| {
                format!(
                    "{} won't load in Steam Linux Runtime 3.0: build it in the runtime's SDK",
                    path.display()
                )
            })?;
            fs::write(checks.join("linux-needs.txt"), &report)
                .context("failed to write checks/linux-needs.txt")?;
            let glibc = newest(&needs, "GLIBC_");
            Ok(json!({
                "runtime": "Steam Linux Runtime 3.0 (sniper)",
                "newest_glibc": glibc,
                "libraries": needs.libraries,
                "desktop_entry": format!("{}/{entry}", app.bundle_name),
            }))
        }
        Target::MacOs => {
            let layout = BundleLayout::macos(stage.join(format!("{}.app", app.bundle_name)));
            let icns = layout.resources_dir.join("AppIcon.icns");
            let head = fs::read(&icns).with_context(|| format!("failed to read {}", icns.display()))?;
            ensure!(head.starts_with(b"icns"), "{} isn't an icns file", icns.display());
            let info = apple::plist_json(&layout.info_plist)?;
            ensure!(
                info.get("CFBundleIconFile").and_then(|name| name.as_str()) == Some("AppIcon"),
                "Info.plist's CFBundleIconFile isn't AppIcon: {:?}",
                info.get("CFBundleIconFile")
            );
            Ok(json!({
                "hardened_runtime": true,
                "entitlements": MACOS_ENTITLEMENTS,
                "icon": "Contents/Resources/AppIcon.icns",
            }))
        }
        Target::Ios | Target::IosSim | Target::Android => bail!("{target} isn't a Steam package"),
    }
}

/// What the Windows executable's resources must say.
struct WindowsExpected<'a> {
    manifest: &'a [u8],
    version: [u16; 3],
    number: u16,
    id: &'a str,
    name: &'a str,
}

/// What the check found.
#[derive(Debug)]
struct WindowsFound {
    report: String,
    file_version: String,
    icon_images: usize,
}

/// Fails unless the executable is a GUI program, its one manifest (resource 1) is [`WINDOWS_MANIFEST`], its
/// VERSIONINFO has the version and build number, and its icon 1 has every image its group names.
fn check_windows(exe: &Executable, expected: &WindowsExpected<'_>) -> anyhow::Result<WindowsFound> {
    ensure!(
        exe.subsystem == pe::SUBSYSTEM_GUI,
        "the executable's subsystem is {}, not a GUI program's ({}): Steam would open a console beside it",
        exe.subsystem,
        pe::SUBSYSTEM_GUI
    );
    let manifests: Vec<_> = exe.of_kind(pe::RT_MANIFEST).collect();
    ensure!(
        manifests.len() == 1 && manifests[0].id == Some(1),
        "the executable has {} manifests, not one as resource 1, which the loader reads",
        manifests.len()
    );
    ensure!(
        normalized(&manifests[0].data) == normalized(expected.manifest),
        "the executable's manifest isn't {WINDOWS_MANIFEST}"
    );

    let versions: Vec<_> = exe.of_kind(pe::RT_VERSION).collect();
    ensure!(versions.len() == 1, "the executable has {} VERSIONINFO resources, not one", versions.len());
    let info = VersionInfo::read(&versions[0].data)?;
    let [major, minor, patch] = expected.version;
    let four = [major, minor, patch, expected.number];
    ensure!(info.file_version == four, "VERSIONINFO's file version is {:?}, not {four:?}", info.file_version);
    ensure!(
        info.product_version == four,
        "VERSIONINFO's product version is {:?}, not {four:?}",
        info.product_version
    );
    let dotted = format!("{major}.{minor}.{patch}.{}", expected.number);
    for (key, wanted) in
        [("FileVersion", dotted.as_str()), ("ProductVersion", expected.id), ("ProductName", expected.name)]
    {
        let found = info.strings.get(key).map(String::as_str);
        ensure!(found == Some(wanted), "VERSIONINFO's {key} is {found:?}, not {wanted:?}");
    }

    let group = exe
        .of_kind(pe::RT_GROUP_ICON)
        .find(|resource| resource.id == Some(1))
        .context("the executable has no icon 1 (the package's build names it in SAPPHIRE_WINDOWS_ICON)")?;
    let images = icon_group(&group.data)?;
    ensure!(!images.is_empty(), "icon 1 has no images");
    for image in &images {
        ensure!(
            exe.of_kind(pe::RT_ICON).any(|icon| icon.id == Some(image.id)),
            "icon 1 names image {}, which isn't in the executable",
            image.id
        );
    }

    let sizes: Vec<String> = images.iter().map(|image| image.size.to_string()).collect();
    let mut report = format!(
        "subsystem: {} (GUI)\nmanifest: resource 1, {} bytes, as {WINDOWS_MANIFEST}\n",
        exe.subsystem,
        manifests[0].data.len()
    );
    report.push_str(&format!("VERSIONINFO: file {dotted}, product {dotted}, table {}\n", info.table));
    for (key, value) in &info.strings {
        report.push_str(&format!("  {key}: {value}\n"));
    }
    report.push_str(&format!("icon 1: {} images, {} px\n", images.len(), sizes.join(", ")));
    Ok(WindowsFound { report, file_version: dotted, icon_images: images.len() })
}

/// The manifest's text as compared: no byte order mark, LF line ends.
fn normalized(text: &[u8]) -> Vec<u8> {
    let text = text.strip_prefix(b"\xef\xbb\xbf").unwrap_or(text);
    String::from_utf8_lossy(text).replace("\r\n", "\n").into_bytes()
}

/// One image an icon group names.
#[derive(Debug, PartialEq, Eq)]
struct IconImage {
    id: u16,
    /// Its width in pixels (256 written as 0).
    size: u16,
}

/// `GRPICONDIR`: a reserved word, type 1, the count, then 14 bytes for each image, its ID last.
fn icon_group(data: &[u8]) -> anyhow::Result<Vec<IconImage>> {
    let word = |at: usize| {
        data.get(at..at + 2).map(|b| u16::from_le_bytes([b[0], b[1]])).context("icon group cut short")
    };
    ensure!(word(2)? == 1, "icon 1's group isn't an icon's");
    (0..usize::from(word(4)?))
        .map(|index| {
            let entry = 6 + index * 14;
            let width = data.get(entry).copied().context("icon group cut short")?;
            Ok(IconImage { id: word(entry + 12)?, size: if width == 0 { 256 } else { u16::from(width) } })
        })
        .collect()
}

/// The workspace version's three numbers, for VERSIONINFO.
fn version_parts(version: &str) -> anyhow::Result<[u16; 3]> {
    let core = version.split(['-', '+']).next().unwrap_or(version);
    let parts: Vec<u16> = core
        .split('.')
        .map(str::parse)
        .collect::<Result<_, _>>()
        .with_context(|| format!("version {version:?}"))?;
    parts.try_into().map_err(|_| anyhow::anyhow!("version {version:?} isn't three numbers"))
}

/// Fails unless an x86-64 binary names only [`RUNTIME_LIBRARIES`] and needs no symbol version newer than
/// [`RUNTIME_VERSIONS`]. Returns what it needs, for `checks/linux-needs.txt`.
fn check_linux(needs: &Needs) -> anyhow::Result<String> {
    ensure!(needs.machine == elf::X86_64, "the executable is for machine {}, not x86-64", needs.machine);
    let strangers: Vec<_> =
        needs.libraries.iter().filter(|library| !RUNTIME_LIBRARIES.contains(&library.as_str())).collect();
    ensure!(strangers.is_empty(), "it links {strangers:?}, which the check doesn't know the runtime has");
    let mut too_new = Vec::new();
    for need in &needs.versions {
        let Some((family, bound)) =
            RUNTIME_VERSIONS.iter().find(|(family, _)| need.version.starts_with(family))
        else {
            bail!(
                "it needs {} of {}, a version family the check has no bound for",
                need.version,
                need.library
            );
        };
        let numbers = version_numbers(&need.version[family.len()..])
            .with_context(|| format!("{} isn't a numbered version", need.version))?;
        if numbers.as_slice() > *bound {
            too_new.push(format!("{} of {}", need.version, need.library));
        }
    }
    ensure!(too_new.is_empty(), "it needs {}, newer than the runtime has", too_new.join(", "));

    let mut report = String::from("Bounds (Steam Linux Runtime 3.0):");
    for (family, bound) in RUNTIME_VERSIONS {
        let bound: Vec<String> = bound.iter().map(u32::to_string).collect();
        report.push_str(&format!(" {family}{}", bound.join(".")));
    }
    report.push_str(&format!("\nLibraries: {}\nVersions needed:\n", needs.libraries.join(", ")));
    for need in &needs.versions {
        report.push_str(&format!("  {} {}\n", need.library, need.version));
    }
    Ok(report)
}

/// A label's numbers, each part's leading digits: `0.9.0rc4` is 0, 9 and 0. `None` when a part has none.
fn version_numbers(label: &str) -> Option<Vec<u32>> {
    label
        .split('.')
        .map(|part| {
            let digits = part.find(|c: char| !c.is_ascii_digit()).unwrap_or(part.len());
            part[..digits].parse().ok()
        })
        .collect()
}

/// The newest version of `family` a binary needs, as its name.
fn newest(needs: &Needs, family: &str) -> Option<String> {
    needs
        .versions
        .iter()
        .filter(|need| need.version.starts_with(family))
        .max_by_key(|need| version_numbers(&need.version[family.len()..]))
        .map(|need| need.version.clone())
}

/// The `.desktop` file (freedesktop.org's Desktop Entry Specification 1.5), named by the bundle ID as the
/// specification asks, as is the icon, and as SDL names the window's app ID and class. It's for an
/// install that puts the executable on the path and the icon in the icon theme; Steam makes its own
/// shortcuts from the store's icon and reads neither.
fn desktop_entry(app: &App) -> String {
    format!(
        "[Desktop Entry]\nType=Application\nVersion=1.5\nName={}\nName[ja]={}\nExec={}\nIcon={}\n\
         StartupWMClass={}\nTerminal=false\nCategories=Game;ActionGame;\n",
        app.display_name.en, app.display_name.ja, app.package, app.bundle_id, app.bundle_id
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::platforms::pe::{Resource, build};
    use crate::platforms::test_game;

    const MANIFEST: &[u8] = include_bytes!("../../../platform/windows/game_app.manifest");

    fn resource(kind: u16, id: u16, data: Vec<u8>) -> Resource {
        Resource { kind, id: Some(id), language: 0x409, data }
    }

    /// An icon group naming images 1 and 2, of 16 and 256 px.
    fn group() -> Vec<u8> {
        let mut data = vec![0, 0, 1, 0, 2, 0];
        for (width, id) in [(16u8, 1u16), (0, 2)] {
            let mut entry = [0u8; 14];
            entry[0] = width;
            entry[12..14].copy_from_slice(&id.to_le_bytes());
            data.extend_from_slice(&entry);
        }
        data
    }

    /// What game_app's build script links into a package's executable, as rc.exe writes it.
    fn resources(manifest: &[u8], file: [u16; 4], id: &str) -> Vec<Resource> {
        let dotted = format!("{}.{}.{}.{}", file[0], file[1], file[2], file[3]);
        let strings = [("FileVersion", dotted.as_str()), ("ProductName", "Sapphire"), ("ProductVersion", id)];
        vec![
            resource(pe::RT_ICON, 1, vec![1; 40]),
            resource(pe::RT_ICON, 2, vec![2; 80]),
            resource(pe::RT_GROUP_ICON, 1, group()),
            resource(pe::RT_VERSION, 1, build::version_info(file, file, "040904B0", &strings)),
            resource(pe::RT_MANIFEST, 1, manifest.to_vec()),
        ]
    }

    fn expected() -> WindowsExpected<'static> {
        WindowsExpected {
            manifest: MANIFEST,
            version: [0, 1, 0],
            number: 1188,
            id: "0.1.0+1188",
            name: "Sapphire",
        }
    }

    fn check(subsystem: u16, resources: &[Resource]) -> anyhow::Result<WindowsFound> {
        check_windows(&Executable::read(&build::executable(subsystem, resources))?, &expected())
    }

    #[test]
    fn a_windows_package_says_what_it_is() {
        let found = check(pe::SUBSYSTEM_GUI, &resources(MANIFEST, [0, 1, 0, 1188], "0.1.0+1188")).unwrap();
        assert_eq!(found.file_version, "0.1.0.1188");
        assert_eq!(found.icon_images, 2);
        assert!(found.report.contains("icon 1: 2 images, 16, 256 px"), "{}", found.report);
        assert!(found.report.contains("ProductVersion: 0.1.0+1188"), "{}", found.report);
        // rc.exe embeds the file's bytes: a checkout with CRLF makes the same manifest.
        let crlf = String::from_utf8(MANIFEST.to_vec()).unwrap().replace('\n', "\r\n");
        check(pe::SUBSYSTEM_GUI, &resources(crlf.as_bytes(), [0, 1, 0, 1188], "0.1.0+1188")).unwrap();
    }

    #[test]
    fn a_windows_package_that_isnt_what_it_says_fails() {
        let good = resources(MANIFEST, [0, 1, 0, 1188], "0.1.0+1188");
        let error =
            |subsystem: u16, resources: &[Resource]| check(subsystem, resources).unwrap_err().to_string();

        assert!(error(3, &good).contains("console"), "a console program");
        let other = resources(b"<assembly/>", [0, 1, 0, 1188], "0.1.0+1188");
        assert!(error(pe::SUBSYSTEM_GUI, &other).contains("manifest isn't"));
        let no_manifest: Vec<_> = good.iter().filter(|r| r.kind != pe::RT_MANIFEST).cloned().collect();
        assert!(error(pe::SUBSYSTEM_GUI, &no_manifest).contains("0 manifests"));
        let old = resources(MANIFEST, [0, 1, 0, 1187], "0.1.0+1188");
        assert!(error(pe::SUBSYSTEM_GUI, &old).contains("file version"));
        let local = resources(MANIFEST, [0, 1, 0, 1188], "0.1.0+local");
        assert!(error(pe::SUBSYSTEM_GUI, &local).contains("ProductVersion"));
        let no_icon: Vec<_> = good.iter().filter(|r| r.kind != pe::RT_GROUP_ICON).cloned().collect();
        assert!(error(pe::SUBSYSTEM_GUI, &no_icon).contains("no icon 1"));
        let missing_image: Vec<_> =
            good.iter().filter(|r| !(r.kind == pe::RT_ICON && r.id == Some(2))).cloned().collect();
        assert!(error(pe::SUBSYSTEM_GUI, &missing_image).contains("image 2"));
    }

    /// The committed manifest says what the plan asks of it (s0-spike.md §6).
    #[test]
    fn the_manifest_asks_for_what_the_game_needs() {
        let text = String::from_utf8(MANIFEST.to_vec()).unwrap();
        for wanted in [
            ">PerMonitorV2</dpiAwareness>",
            ">true/pm</dpiAware>",
            ">true</longPathAware>",
            ">UTF-8</activeCodePage>",
            "level=\"asInvoker\"",
            // Windows 10 and 11.
            "{8e0f7a12-bfb3-4fe8-b9a5-48fd50a15a9a}",
        ] {
            assert!(text.contains(wanted), "{WINDOWS_MANIFEST} lacks {wanted}");
        }
    }

    #[test]
    fn a_linux_build_needs_only_what_the_runtime_has() {
        let elf = elf::build(
            &[
                "libasound.so.2",
                "libstdc++.so.6",
                "libgcc_s.so.1",
                "libm.so.6",
                "libc.so.6",
                "ld-linux-x86-64.so.2",
            ],
            &[
                ("libasound.so.2", &["ALSA_0.9", "ALSA_0.9.0rc4"]),
                ("libstdc++.so.6", &["GLIBCXX_3.4.28", "CXXABI_1.3.12", "GLIBCXX_3.4"]),
                ("libgcc_s.so.1", &["GCC_4.2.0", "GCC_3.0"]),
                ("libc.so.6", &["GLIBC_2.31", "GLIBC_2.2.5", "GLIBC_2.18"]),
            ],
        );
        let needs = Needs::read(&elf).unwrap();
        let report = check_linux(&needs).unwrap();
        assert!(report.contains("GLIBC_2.31 GLIBCXX_3.4.28 CXXABI_1.3.12 GCC_9.0.0 ALSA_1.2.4"), "{report}");
        assert!(report.contains("  libc.so.6 GLIBC_2.31\n"), "{report}");
        assert_eq!(newest(&needs, "GLIBC_").as_deref(), Some("GLIBC_2.31"));
        assert_eq!(newest(&needs, "GLIBCXX_").as_deref(), Some("GLIBCXX_3.4.28"));
    }

    #[test]
    fn a_linux_build_too_new_for_the_runtime_fails() {
        let error = |libraries: &[&str], needs: &[(&str, &[&str])]| {
            check_linux(&Needs::read(&elf::build(libraries, needs)).unwrap()).unwrap_err().to_string()
        };
        // Built on ubuntu-24.04, whose glibc is 2.39.
        let ubuntu = error(&["libc.so.6"], &[("libc.so.6", &["GLIBC_2.34", "GLIBC_2.2.5", "GLIBC_2.39"])]);
        assert!(ubuntu.contains("GLIBC_2.34 of libc.so.6, GLIBC_2.39 of libc.so.6"), "{ubuntu}");
        let gcc14 = error(&["libstdc++.so.6"], &[("libstdc++.so.6", &["GLIBCXX_3.4.32"])]);
        assert!(gcc14.contains("GLIBCXX_3.4.32"), "{gcc14}");
        assert!(error(&["libSDL3.so.0"], &[]).contains("libSDL3.so.0"));
        assert!(error(&["libc.so.6"], &[("libc.so.6", &["GLIBC_PRIVATE"])]).contains("numbered"));
        assert!(error(&["libasound.so.2"], &[("libasound.so.2", &["ALSA_1.2.13"])]).contains("ALSA_1.2.13"));
        assert!(error(&["libz.so.1"], &[]).contains("doesn't know"));
        assert!(error(&["libc.so.6"], &[("libfoo.so", &["FOO_1.0"])]).contains("no bound"));
    }

    #[test]
    fn the_desktop_entry_names_the_game_as_sdl_does() {
        let entry = desktop_entry(&test_game());
        assert!(entry.starts_with("[Desktop Entry]\nType=Application\n"));
        for line in [
            "Name=Sapphire",
            "Name[ja]=サファイア",
            "Exec=game_app",
            "Icon=com.example.sapphire.game",
            "StartupWMClass=com.example.sapphire.game",
            "Categories=Game;ActionGame;",
        ] {
            assert!(entry.lines().any(|l| l == line), "no {line} in\n{entry}");
        }
    }

    #[test]
    fn reads_codesigns_flags() {
        let shown = "Executable=/x/SapphireGame.app/Contents/MacOS/game_app\nIdentifier=com.example.sapphire.game\n\
                     Format=app bundle with Mach-O thin (arm64)\nCodeDirectory v=20500 size=1 flags=0x10002(adhoc,runtime) \
                     hashes=1+7 location=embedded\n";
        assert_eq!(signature_flags(shown).unwrap(), ["adhoc", "runtime"]);
        assert!(signature_flags("Signature=adhoc\n").is_none());
    }

    #[test]
    fn the_macos_entitlements_grant_nothing() {
        let text = fs::read_to_string(workspace::root().join(MACOS_ENTITLEMENTS)).unwrap();
        assert!(
            text.contains("<dict/>"),
            "{MACOS_ENTITLEMENTS} grants something: say why here and in the file"
        );
        // codesign's parser refuses a comment holding `--`, which XML forbids there, though plutil reads it.
        let comment = text.split("<!--").nth(1).and_then(|rest| rest.split("-->").next()).unwrap();
        assert!(!comment.contains("--"), "{MACOS_ENTITLEMENTS}'s comment holds `--`");
    }

    #[test]
    fn only_desktops_ship_on_steam() {
        assert!(
            ships_on_steam(Target::Windows) && ships_on_steam(Target::Linux) && ships_on_steam(Target::MacOs)
        );
        assert!(!ships_on_steam(Target::Ios) && !ships_on_steam(Target::Android));
        assert_eq!(version_parts("0.1.0").unwrap(), [0, 1, 0]);
        assert!(version_parts("0.1").is_err() && version_parts("0.1.x").is_err());
        let env = build_env(Target::Windows, Path::new("icons"));
        assert_eq!(env[0].0, WINDOWS_ICON_ENV);
        assert!(build_env(Target::Linux, Path::new("icons")).is_empty());
    }
}
