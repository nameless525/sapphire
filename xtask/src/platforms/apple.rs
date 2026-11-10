//! Shared Apple bundling: Info.plist templates, `.app` assembly and ad-hoc signing.

use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context as _, ensure};

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
///
/// The values go into `<string>` elements, escaped for XML, and in one pass over the
/// template: a value is never scanned for placeholders, so a display name may hold `&`, `<`
/// or `$(` and comes out as written. The display name is English's, the development region's;
/// each language's is in its `InfoPlist.strings` ([`info_plist_strings`]). `build` is the build
/// number ([`crate::version`]).
pub fn render_info_plist(template: &str, app: &App, build: u32) -> anyhow::Result<String> {
    let build = build.to_string();
    let variables = [
        ("EXECUTABLE_NAME", app.package.as_str()),
        ("PRODUCT_BUNDLE_IDENTIFIER", &app.bundle_id),
        ("PRODUCT_NAME", &app.bundle_name),
        ("BUNDLE_DISPLAY_NAME", &app.display_name.en),
        ("MARKETING_VERSION", &app.version),
        ("CURRENT_PROJECT_VERSION", &build),
    ];
    super::fill_template(template, "Info.plist", &variables, push_xml_escaped)
}

/// Appends `text` escaped for an XML text node.
pub fn push_xml_escaped(out: &mut String, text: &str) {
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            c => out.push(c),
        }
    }
}

/// A language's `InfoPlist.strings`, which names the app under its icon in that language: a strings
/// file, UTF-8, with the name quoted and escaped as C strings are.
pub fn info_plist_strings(display_name: &str) -> String {
    let mut quoted = String::with_capacity(display_name.len());
    for c in display_name.chars() {
        match c {
            '"' => quoted.push_str("\\\""),
            '\\' => quoted.push_str("\\\\"),
            c => quoted.push(c),
        }
    }
    format!("/* Made by xtask from platform/identity.toml. */\n\"CFBundleDisplayName\" = \"{quoted}\";\n")
}

/// The `<string>` value of `key` in a plist's top-level dictionary.
pub fn plist_string<'a>(plist: &'a str, key: &str) -> Option<&'a str> {
    let after_key = &plist[plist.find(&format!("<key>{key}</key>"))?..];
    let start = after_key.find("<string>")? + "<string>".len();
    Some(&after_key[start..start + after_key[start..].find("</string>")?])
}

/// Fails unless `executable`'s minimum OS (`LC_BUILD_VERSION`) is `expected`, the
/// Info.plist's `minimum_key` value. The deployment targets in `.cargo/config.toml` set
/// it; a mismatch means the two drifted, or the build ignored them.
pub fn verify_minimum_os(executable: &Path, plist: &str, minimum_key: &str) -> anyhow::Result<()> {
    let expected =
        plist_string(plist, minimum_key).with_context(|| format!("Info.plist has no {minimum_key}"))?;
    let build = workspace::output(Command::new("xcrun").args(["vtool", "-show-build"]).arg(executable))?;
    let minos = build.lines().find_map(|line| line.trim().strip_prefix("minos ")).with_context(|| {
        format!("`vtool -show-build` reported no minimum OS for {}", executable.display())
    })?;
    ensure!(
        minos.trim() == expected,
        "{} is built for OS {} but Info.plist's {minimum_key} is {expected}; keep the deployment \
         targets in .cargo/config.toml in step with platform/*/Info.plist",
        executable.display(),
        minos.trim()
    );
    Ok(())
}

/// Assembles a fresh `.app`: Info.plist, each language's `InfoPlist.strings`, the executable and the
/// cooked content.
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
    for (language, name) in app.display_name.by_language() {
        let dir = layout.resources_dir.join(format!("{language}.lproj"));
        std::fs::create_dir_all(&dir).with_context(|| format!("failed to create {}", dir.display()))?;
        let path = dir.join("InfoPlist.strings");
        std::fs::write(&path, info_plist_strings(name))
            .with_context(|| format!("failed to write {}", path.display()))?;
    }
    let bundled_exe = layout.executable_dir.join(&app.package);
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
    use crate::platforms::{test_app, test_game};

    /// A template as checked in, with line endings normalized: a Windows checkout without
    /// .gitattributes has CRLF.
    fn template(text: &str) -> String {
        text.replace("\r\n", "\n")
    }

    fn ios_template() -> String {
        template(include_str!("../../../platform/ios/Info.plist"))
    }

    fn macos_template() -> String {
        template(include_str!("../../../platform/macos/Info.plist"))
    }

    fn value_of<'a>(plist: &'a str, key: &str) -> &'a str {
        plist_string(plist, key).unwrap_or_else(|| panic!("no string value for {key}"))
    }

    #[test]
    fn renders_the_ios_template() {
        let app = test_app();
        let plist = render_info_plist(&ios_template(), &app, 4123).unwrap();
        assert_eq!(value_of(&plist, "CFBundleExecutable"), "sapphire_sandbox");
        assert_eq!(value_of(&plist, "CFBundleIdentifier"), "dev.sapphire.sandbox");
        assert_eq!(value_of(&plist, "CFBundleDisplayName"), "Sapphire Sandbox");
        assert_eq!(value_of(&plist, "CFBundleName"), "SapphireSandbox");
        assert_eq!(value_of(&plist, "MinimumOSVersion"), "17.0");
        assert_eq!(value_of(&plist, "CFBundleShortVersionString"), app.version);
        assert_eq!(value_of(&plist, "CFBundleVersion"), "4123");
        assert!(plist.contains(
            "<key>CFBundleLocalizations</key>\n\t<array>\n\t\t<string>en</string>\n\t\t<string>ja</string>"
        ));
        assert!(plist.contains("<key>UILaunchScreen</key>"));
        assert!(plist.contains("<key>UIRequiresFullScreen</key>\n\t<true/>"));
        // SDL needs it to receive mouse and trackpad events as such on iPad.
        assert!(plist.contains("<key>UIApplicationSupportsIndirectInputEvents</key>\n\t<true/>"));
        assert!(!plist.contains("Portrait"), "the game is landscape only");
    }

    #[test]
    fn renders_the_macos_template() {
        let plist = render_info_plist(&macos_template(), &test_game(), 7).unwrap();
        assert_eq!(value_of(&plist, "CFBundleExecutable"), "game_app");
        assert_eq!(value_of(&plist, "CFBundleIdentifier"), "com.example.sapphire.game");
        assert_eq!(value_of(&plist, "CFBundleDisplayName"), "Sapphire");
        assert_eq!(value_of(&plist, "CFBundleVersion"), "7");
        assert_eq!(value_of(&plist, "LSMinimumSystemVersion"), "14.0");
    }

    /// The Info.plist minimums and .cargo/config.toml's deployment targets must agree;
    /// `verify_minimum_os` checks the built binaries against the plists.
    #[test]
    fn deployment_targets_match_the_templates() {
        let config: toml::Table = toml::from_str(include_str!("../../../.cargo/config.toml")).unwrap();
        let target = |name: &str| config["env"][name]["value"].as_str().unwrap().to_owned();
        assert_eq!(target("MACOSX_DEPLOYMENT_TARGET"), value_of(&macos_template(), "LSMinimumSystemVersion"));
        assert_eq!(target("IPHONEOS_DEPLOYMENT_TARGET"), value_of(&ios_template(), "MinimumOSVersion"));
    }

    /// The display name is the one identity field people write freely, so it may hold XML's
    /// metacharacters and even placeholder syntax; it must come out as the same text.
    #[test]
    fn display_names_round_trip_through_the_plist() {
        for name in [
            "Sword & Shield",
            "A < B > C",
            "Tom's \"Game\"",
            "サファイア ✦",
            "$(NAME)",
            "$(MARKETING_VERSION)",
            "&amp;",
            "$(",
        ] {
            let app = named(name);
            let plist = render_info_plist(&ios_template(), &app, 1).unwrap_or_else(|e| panic!("{name}: {e}"));
            let written = value_of(&plist, "CFBundleDisplayName");
            let bare = written.replace("&amp;", "").replace("&lt;", "").replace("&gt;", "");
            assert!(!bare.contains(['&', '<', '>']), "{name} written unescaped: {written}");
            assert_eq!(xml_unescape(written), name);
            // Every other value is as before.
            assert_eq!(value_of(&plist, "CFBundleShortVersionString"), app.version);
        }
    }

    /// macOS's own plist parser reads the name back as written.
    #[test]
    fn plutil_reads_escaped_display_names() {
        if std::env::consts::OS != "macos" {
            return;
        }
        let scratch = std::env::temp_dir().join(format!("xtask-plist-{}", std::process::id()));
        std::fs::create_dir_all(&scratch).unwrap();
        for (index, name) in
            ["Sword & Shield", "A < B > C", "Tom's \"Game\"", "$(NAME)"].into_iter().enumerate()
        {
            let app = named(name);
            let path = scratch.join(format!("{index}.plist"));
            std::fs::write(&path, render_info_plist(&macos_template(), &app, 1).unwrap()).unwrap();
            workspace::run(Command::new("plutil").arg("-lint").arg(&path)).unwrap();
            let read = workspace::output(
                Command::new("plutil").args(["-extract", "CFBundleDisplayName", "raw", "-o", "-"]).arg(&path),
            )
            .unwrap();
            assert_eq!(read.strip_suffix('\n').unwrap_or(&read), name);

            // Each language's strings file reads back as the same name too, Japanese as well.
            for name in [name, "サファイア"] {
                let strings = scratch.join(format!("{index}.strings"));
                std::fs::write(&strings, info_plist_strings(name)).unwrap();
                workspace::run(Command::new("plutil").arg("-lint").arg(&strings)).unwrap();
                let json = workspace::output(
                    Command::new("plutil").args(["-convert", "json", "-o", "-"]).arg(&strings),
                )
                .unwrap();
                let read: serde_json::Value = serde_json::from_str(&json).unwrap();
                assert_eq!(read["CFBundleDisplayName"], name);
            }
        }
        workspace::remove_dir_if_exists(&scratch).unwrap();
    }

    /// The sandbox, with another English display name.
    fn named(name: &str) -> App {
        let base = test_app();
        App {
            display_name: crate::platforms::DisplayName { en: name.into(), ..base.display_name.clone() },
            ..base
        }
    }

    /// Text-node unescaping, enough for what `render_info_plist` writes.
    fn xml_unescape(text: &str) -> String {
        text.replace("&lt;", "<").replace("&gt;", ">").replace("&amp;", "&")
    }

    #[test]
    fn rejects_unknown_placeholders() {
        let err = render_info_plist("<string>$(DEVELOPMENT_TEAM)</string>", &test_app(), 1).unwrap_err();
        assert_eq!(err.to_string(), "unknown placeholder $(DEVELOPMENT_TEAM) in Info.plist template");
        // After a known one too: the scan goes on past each value.
        let err = render_info_plist("$(PRODUCT_NAME)$(PRODUCT_NAME:rfc1034identifier)", &test_app(), 1)
            .unwrap_err();
        assert_eq!(
            err.to_string(),
            "unknown placeholder $(PRODUCT_NAME:rfc1034identifier) in Info.plist template"
        );
        let err = render_info_plist("<string>$(PRODUCT_NAME</string>", &test_app(), 1).unwrap_err();
        assert_eq!(err.to_string(), "unterminated placeholder in Info.plist template");
    }

    #[test]
    fn assembles_both_layouts() {
        let scratch = std::env::temp_dir().join(format!("xtask-bundle-{}", std::process::id()));
        let content = scratch.join("cooked");
        std::fs::create_dir_all(&content).unwrap();
        std::fs::write(content.join("shared.spack"), "pack").unwrap();
        std::fs::write(content.join("NOTICES.txt"), "the fonts' licenses").unwrap();
        let exe = scratch.join("exe");
        std::fs::write(&exe, "binary").unwrap();

        let ios = BundleLayout::ios(scratch.join("ios/SapphireSandbox.app"));
        let macos = BundleLayout::macos(scratch.join("macos/SapphireSandbox.app"));
        for layout in [&ios, &macos] {
            // Assembling twice must not keep stale files from the first bundle.
            std::fs::create_dir_all(&layout.root).unwrap();
            std::fs::write(layout.root.join("stale"), "").unwrap();
            assemble(layout, &test_app(), "<plist/>", &exe, &content).unwrap();
            assert!(!layout.root.join("stale").exists());
        }
        let app = scratch.join("ios/SapphireSandbox.app");
        for file in [
            "Info.plist",
            "sapphire_sandbox",
            "content/shared.spack",
            "content/NOTICES.txt",
            "en.lproj/InfoPlist.strings",
            "ja.lproj/InfoPlist.strings",
        ] {
            assert!(app.join(file).is_file(), "iOS bundle is missing {file}");
        }
        let contents = scratch.join("macos/SapphireSandbox.app/Contents");
        for file in [
            "Info.plist",
            "MacOS/sapphire_sandbox",
            "Resources/content/shared.spack",
            "Resources/content/NOTICES.txt",
            "Resources/ja.lproj/InfoPlist.strings",
        ] {
            assert!(contents.join(file).is_file(), "macOS bundle is missing {file}");
        }
        workspace::remove_dir_if_exists(&scratch).unwrap();
    }
}
