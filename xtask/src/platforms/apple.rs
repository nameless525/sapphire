//! Shared Apple bundling: Info.plist templates, `.app` assembly and ad-hoc signing.

use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context as _, bail};

use super::App;
use crate::workspace;

/// Where things go inside a `.app`. iOS bundles are flat; macOS bundles nest under
/// `Contents/`.
#[derive(Debug)]
pub struct BundleLayout {
    pub root: PathBuf,
    pub info_plist: PathBuf,
    pub executable_dir: PathBuf,
    /// The read-only resource root; cooked content goes in its `content/` (spec §2).
    pub resources_dir: PathBuf,
}

impl BundleLayout {
    pub fn ios(root: PathBuf) -> Self {
        Self {
            info_plist: root.join("Info.plist"),
            executable_dir: root.clone(),
            resources_dir: root.clone(),
            root,
        }
    }

    pub fn macos(root: PathBuf) -> Self {
        let contents = root.join("Contents");
        Self {
            info_plist: contents.join("Info.plist"),
            executable_dir: contents.join("MacOS"),
            resources_dir: contents.join("Resources"),
            root,
        }
    }
}

/// Fills an Info.plist template's Xcode-style `$(NAME)` placeholders, so the same
/// template can later be handed to an Xcode project unchanged.
pub fn render_info_plist(template: &str, app: &App) -> anyhow::Result<String> {
    // The workspace version; per-build numbers come from CI once builds are uploaded.
    let version = env!("CARGO_PKG_VERSION");
    let variables = [
        ("EXECUTABLE_NAME", app.package),
        ("PRODUCT_BUNDLE_IDENTIFIER", app.bundle_id),
        ("PRODUCT_NAME", app.bundle_name),
        ("BUNDLE_DISPLAY_NAME", app.display_name),
        ("MARKETING_VERSION", version),
        ("CURRENT_PROJECT_VERSION", version),
    ];
    let mut plist = template.to_owned();
    for (name, value) in variables {
        plist = plist.replace(&format!("$({name})"), value);
    }
    if let Some(start) = plist.find("$(") {
        let placeholder: String = plist[start..].chars().take_while(|&c| c != ')').chain([')']).collect();
        bail!("unknown placeholder {placeholder} in Info.plist template");
    }
    Ok(plist)
}

/// Assembles a fresh `.app`: Info.plist, the executable and the cooked content.
pub fn assemble(
    layout: &BundleLayout,
    app: &App,
    info_plist: &str,
    executable: &Path,
    content: &Path,
) -> anyhow::Result<()> {
    workspace::remove_dir_if_exists(&layout.root)?;
    for dir in [&layout.executable_dir, &layout.resources_dir] {
        std::fs::create_dir_all(dir).with_context(|| format!("failed to create {}", dir.display()))?;
    }
    std::fs::write(&layout.info_plist, info_plist)
        .with_context(|| format!("failed to write {}", layout.info_plist.display()))?;
    let bundled_exe = layout.executable_dir.join(app.package);
    std::fs::copy(executable, &bundled_exe)
        .with_context(|| format!("failed to copy {} into the bundle", executable.display()))?;
    workspace::copy_dir(content, &layout.resources_dir.join("content"))
}

/// Signs a bundle ad hoc (no identity). Enough for the simulator and for running locally
/// on Apple Silicon; distribution signing and notarization come with `xtask package`.
pub fn codesign_ad_hoc(bundle: &Path) -> anyhow::Result<()> {
    workspace::run(Command::new("codesign").args(["--force", "--sign", "-", "--timestamp=none"]).arg(bundle))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::platforms::SANDBOX;

    const IOS_TEMPLATE: &str = include_str!("../../../platform/ios/Info.plist");
    const MACOS_TEMPLATE: &str = include_str!("../../../platform/macos/Info.plist");

    fn value_of<'a>(plist: &'a str, key: &str) -> &'a str {
        let after_key =
            &plist[plist.find(&format!("<key>{key}</key>")).unwrap_or_else(|| panic!("no {key}"))..];
        let start = after_key.find("<string>").expect("string value") + "<string>".len();
        &after_key[start..start + after_key[start..].find("</string>").unwrap()]
    }

    #[test]
    fn renders_the_ios_template() {
        let plist = render_info_plist(IOS_TEMPLATE, &SANDBOX).unwrap();
        assert_eq!(value_of(&plist, "CFBundleExecutable"), "sapphire_sandbox");
        assert_eq!(value_of(&plist, "CFBundleIdentifier"), "dev.sapphire.sandbox");
        assert_eq!(value_of(&plist, "CFBundleDisplayName"), "Sapphire Sandbox");
        assert_eq!(value_of(&plist, "CFBundleName"), "SapphireSandbox");
        assert_eq!(value_of(&plist, "MinimumOSVersion"), "17.0");
        assert_eq!(value_of(&plist, "CFBundleShortVersionString"), env!("CARGO_PKG_VERSION"));
        assert!(plist.contains("<key>UILaunchScreen</key>"));
        assert!(plist.contains("<key>UIRequiresFullScreen</key>\n\t<true/>"));
        // SDL needs it to receive mouse and trackpad events as such on iPad.
        assert!(plist.contains("<key>UIApplicationSupportsIndirectInputEvents</key>\n\t<true/>"));
        assert!(!plist.contains("Portrait"), "the game is landscape only");
    }

    #[test]
    fn renders_the_macos_template() {
        let plist = render_info_plist(MACOS_TEMPLATE, &SANDBOX).unwrap();
        assert_eq!(value_of(&plist, "CFBundleExecutable"), "sapphire_sandbox");
        assert_eq!(value_of(&plist, "CFBundleIdentifier"), "dev.sapphire.sandbox");
        assert_eq!(value_of(&plist, "LSMinimumSystemVersion"), "14.0");
    }

    #[test]
    fn rejects_unknown_placeholders() {
        let err = render_info_plist("<string>$(DEVELOPMENT_TEAM)</string>", &SANDBOX).unwrap_err();
        assert_eq!(err.to_string(), "unknown placeholder $(DEVELOPMENT_TEAM) in Info.plist template");
    }

    #[test]
    fn assembles_both_layouts() {
        let scratch = std::env::temp_dir().join(format!("xtask-bundle-{}", std::process::id()));
        let content = scratch.join("cooked");
        std::fs::create_dir_all(content.join("samples")).unwrap();
        std::fs::write(content.join("samples/mannequin.smesh"), "mesh").unwrap();
        let exe = scratch.join("exe");
        std::fs::write(&exe, "binary").unwrap();

        let ios = BundleLayout::ios(scratch.join("ios/SapphireSandbox.app"));
        let macos = BundleLayout::macos(scratch.join("macos/SapphireSandbox.app"));
        for layout in [&ios, &macos] {
            // Assembling twice must not keep stale files from the first bundle.
            std::fs::create_dir_all(&layout.root).unwrap();
            std::fs::write(layout.root.join("stale"), "").unwrap();
            assemble(layout, &SANDBOX, "<plist/>", &exe, &content).unwrap();
            assert!(!layout.root.join("stale").exists());
        }
        let app = scratch.join("ios/SapphireSandbox.app");
        for file in ["Info.plist", "sapphire_sandbox", "content/samples/mannequin.smesh"] {
            assert!(app.join(file).is_file(), "iOS bundle is missing {file}");
        }
        let contents = scratch.join("macos/SapphireSandbox.app/Contents");
        for file in ["Info.plist", "MacOS/sapphire_sandbox", "Resources/content/samples/mannequin.smesh"] {
            assert!(contents.join(file).is_file(), "macOS bundle is missing {file}");
        }
        workspace::remove_dir_if_exists(&scratch).unwrap();
    }
}
