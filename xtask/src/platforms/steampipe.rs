//! `cargo xtask steampipe`: Steam's depots, written as text from the desktop packages (M4 S4).
//!
//! It reads the packages `cargo xtask package` made for Windows, Linux and macOS (`<packages>/<platform>/`,
//! built on three hosts and gathered in one place, as `target/package/` or I2's artifacts are) and writes
//! `<packages>/steam/`:
//!
//! - `app_build_<app>.vdf`: SteamPipe's app build, with a depot for each package found;
//! - `depot_build_<depot>.vdf`: each depot's files, the package's app and nothing beside it (no symbols,
//!   no checks), at the depot's root;
//! - `depots.txt`: each depot's layout, every file with its size, and the launch option Steamworks needs.
//!
//! The scripts' paths are relative to them (Valve's `ContentRoot` is the script's folder's), so the folder
//! goes to the machine that uploads as it is. One depot per desktop, each holding its whole package: the
//! macOS bundle seals its content inside its signature, so it can't share a content depot, and a player
//! downloads one platform's depot only. IDs come from [`IDS_FILE`]; until the maintainer chooses them
//! they're Valve's examples, and every script says so. Nothing uploads: that's
//! `steamcmd +login <account> +run_app_build <app build> +quit`, the maintainer's (m4-plan §7).

use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context as _, bail, ensure};
use serde::Deserialize;

use crate::workspace;

/// The game's Steam IDs.
pub const IDS_FILE: &str = "platform/steam.toml";
/// Valve's examples' IDs (partner.steamgames.com/doc/sdk/uploading): what a placeholder uses.
const PLACEHOLDER_APP: u32 = 1000;
const PLACEHOLDER_DEPOTS: [u32; 3] = [1001, 1002, 1003];

/// The desktops, in depot order.
const PLATFORMS: [&str; 3] = ["windows", "linux", "macos"];

/// [`IDS_FILE`].
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
pub struct Ids {
    #[serde(default)]
    placeholder: bool,
    app_id: u32,
    depots: Depots,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Depots {
    windows: u32,
    linux: u32,
    macos: u32,
}

impl Ids {
    pub fn load() -> anyhow::Result<Self> {
        let path = workspace::root().join(IDS_FILE);
        let text = fs::read_to_string(&path).with_context(|| format!("failed to read {IDS_FILE}"))?;
        Self::parse(&text).with_context(|| format!("in {IDS_FILE}"))
    }

    fn parse(text: &str) -> anyhow::Result<Self> {
        let ids: Self = toml::from_str(text)?;
        let all = [ids.app_id, ids.depots.windows, ids.depots.linux, ids.depots.macos];
        ensure!(all.iter().all(|&id| id != 0), "an ID of 0 is no ID");
        let mut distinct = all.to_vec();
        distinct.sort_unstable();
        distinct.dedup();
        ensure!(distinct.len() == all.len(), "the app and its depots need four different IDs: {all:?}");
        let examples = all.iter().any(|id| *id == PLACEHOLDER_APP || PLACEHOLDER_DEPOTS.contains(id));
        if ids.placeholder {
            ensure!(
                ids.app_id == PLACEHOLDER_APP
                    && [ids.depots.windows, ids.depots.linux, ids.depots.macos] == PLACEHOLDER_DEPOTS,
                "`placeholder = true` goes only with Valve's example IDs ({PLACEHOLDER_APP}, {PLACEHOLDER_DEPOTS:?}): \
                 delete it once the IDs are the game's"
            );
        } else {
            ensure!(
                !examples,
                "{all:?} uses Valve's example IDs, which aren't the game's: mark them `placeholder = true` or put in \
                 the IDs Steamworks shows"
            );
        }
        Ok(ids)
    }

    fn depot(&self, platform: &str) -> u32 {
        match platform {
            "windows" => self.depots.windows,
            "linux" => self.depots.linux,
            _ => self.depots.macos,
        }
    }
}

/// What a desktop package's `package.json` says, as far as depots go.
#[derive(Debug, PartialEq, Eq)]
struct Package {
    platform: String,
    /// The executable's name.
    executable: String,
    /// The app's path in the package: a folder, or macOS's bundle.
    app_path: String,
    build_id: String,
    commit: String,
    dev: bool,
}

impl Package {
    fn read(dir: &Path, platform: &str) -> anyhow::Result<Self> {
        let path = dir.join("package.json");
        let text = fs::read_to_string(&path).with_context(|| format!("failed to read {}", path.display()))?;
        let json: serde_json::Value =
            serde_json::from_str(&text).with_context(|| format!("{} isn't JSON", path.display()))?;
        let field = |pointer: &str| -> anyhow::Result<String> {
            json.pointer(pointer)
                .and_then(serde_json::Value::as_str)
                .map(str::to_owned)
                .with_context(|| format!("{} has no {pointer}", path.display()))
        };
        let package = Self {
            platform: field("/platform")?,
            executable: field("/app")?,
            app_path: field("/app_path")?,
            build_id: field("/build/id")?,
            commit: field("/build/commit")?,
            dev: field("/build/kind")? == "dev",
        };
        ensure!(
            package.platform == platform,
            "{} is a {} package, not {platform}'s",
            path.display(),
            package.platform
        );
        ensure!(
            !package.app_path.is_empty()
                && !package.app_path.contains(['/', '\\'])
                && package.app_path != "..",
            "{} names app_path {:?}, which isn't a name in the package",
            path.display(),
            package.app_path
        );
        ensure!(dir.join(&package.app_path).is_dir(), "{} has no {}", dir.display(), package.app_path);
        Ok(package)
    }

    /// What Steamworks' launch option names, from the install folder.
    fn launch(&self) -> String {
        match self.platform.as_str() {
            "windows" => format!("{}.exe", self.executable),
            "macos" => self.app_path.clone(),
            _ => self.executable.clone(),
        }
    }

    /// Where the depot puts the package's app: macOS's bundle as itself, a folder's files at the root.
    fn depot_path(&self) -> &str {
        if self.platform == "macos" { &self.app_path } else { "." }
    }
}

/// Writes `<packages>/steam/` from the desktop packages there, and returns its path.
pub fn steampipe(packages: &Path) -> anyhow::Result<PathBuf> {
    let ids = Ids::load()?;
    let mut found = Vec::new();
    for platform in PLATFORMS {
        let dir = packages.join(platform);
        if dir.join("package.json").is_file() {
            let package = Package::read(&dir, platform)?;
            let files = files_of(&dir, &package)?;
            found.push((package, files));
        } else {
            eprintln!(
                "warning: no {platform} package in {}: the app build has no {platform} depot",
                packages.display()
            );
        }
    }
    ensure!(
        !found.is_empty(),
        "no desktop package in {}: run `cargo xtask package` for each first",
        packages.display()
    );
    let first = &found[0].0;
    for (package, _) in &found[1..] {
        ensure!(
            (&package.build_id, &package.commit) == (&first.build_id, &first.commit),
            "the {} package is {} at {}, but the {} package is {} at {}: a Steam build's depots come from one build",
            package.platform,
            package.build_id,
            package.commit,
            first.platform,
            first.build_id,
            first.commit
        );
    }
    if first.dev {
        eprintln!("warning: {} is a DEV build: no store should take it", first.build_id);
    }
    if ids.placeholder {
        eprintln!(
            "warning: {IDS_FILE}'s IDs are placeholders: steamcmd can't upload to them until the maintainer puts in the game's"
        );
    }

    let out = packages.join("steam");
    workspace::remove_dir_if_exists(&out)?;
    fs::create_dir_all(&out).with_context(|| format!("failed to create {}", out.display()))?;
    let packages: Vec<&Package> = found.iter().map(|(package, _)| package).collect();
    let mut written = vec![(format!("app_build_{}.vdf", ids.app_id), app_build(&ids, &packages))];
    for package in &packages {
        let depot = ids.depot(&package.platform);
        written.push((format!("depot_build_{depot}.vdf"), depot_build(&ids, package)));
    }
    written.push(("depots.txt".to_owned(), layouts(&ids, &found)));
    for (name, text) in written {
        fs::write(out.join(&name), text).with_context(|| format!("failed to write {name}"))?;
    }
    Ok(out)
}

/// The note every script starts with: what it is, what it's for, and whether its IDs are placeholders.
fn preamble(ids: &Ids, what: &str, build: &Package) -> String {
    let mut text = format!(
        "// SteamPipe's {what} for {} (commit {}), written by `cargo xtask steampipe` from the packages\n\
         // beside this folder. Nothing here uploads: the maintainer runs\n\
         //   steamcmd +login <account> +run_app_build app_build_{}.vdf +quit\n",
        build.build_id,
        &build.commit[..build.commit.len().min(12)],
        ids.app_id
    );
    if ids.placeholder {
        text.push_str(&format!(
            "// PLACEHOLDER IDs: {IDS_FILE} still has Valve's example IDs, which aren't the game's. Put in the\n\
             // IDs Steamworks shows, delete `placeholder = true` and write these again before uploading.\n"
        ));
    }
    if build.dev {
        text.push_str("// A DEV build (not one of main's commits): for no store.\n");
    }
    text
}

fn app_build(ids: &Ids, packages: &[&Package]) -> String {
    let build = packages[0];
    let mut text = preamble(ids, "app build", build);
    text.push_str(
        "// Add \"Preview\" \"1\" to see what would upload without uploading. It sets no branch live:\n",
    );
    text.push_str("// that's Steamworks' choice, after review.\n");
    text.push_str("\"AppBuild\"\n{\n");
    let _ = writeln!(text, "\t\"AppID\" \"{}\"{}", ids.app_id, mark(ids));
    let _ = writeln!(
        text,
        "\t\"Desc\" {}",
        quoted(&format!("Sapphire {} ({})", build.build_id, &build.commit[..build.commit.len().min(12)]))
    );
    text.push_str("\t\"ContentRoot\" \"../\"\n\t\"BuildOutput\" \"output/\"\n\t\"Depots\"\n\t{\n");
    for package in packages {
        let depot = ids.depot(&package.platform);
        let _ = writeln!(text, "\t\t\"{depot}\" \"depot_build_{depot}.vdf\"{}", mark(ids));
    }
    text.push_str("\t}\n}\n");
    text
}

fn depot_build(ids: &Ids, package: &Package) -> String {
    let depot = ids.depot(&package.platform);
    let mut text = preamble(ids, &format!("{} depot", package.platform), package);
    text.push_str("\"DepotBuild\"\n{\n");
    let _ = writeln!(text, "\t\"DepotID\" \"{depot}\"{}", mark(ids));
    text.push_str("\t\"FileMapping\"\n\t{\n");
    let _ = writeln!(
        text,
        "\t\t\"LocalPath\" {}",
        quoted(&format!("{}/{}/*", package.platform, package.app_path))
    );
    let _ = writeln!(text, "\t\t\"DepotPath\" {}", quoted(package.depot_path()));
    // Only the app: the symbols and checks beside it in the package never go in a depot.
    text.push_str("\t\t\"Recursive\" \"1\"\n\t}\n}\n");
    text
}

fn mark(ids: &Ids) -> &'static str {
    if ids.placeholder { " // PLACEHOLDER" } else { "" }
}

/// A VDF string: backslashes and quotes escaped.
fn quoted(text: &str) -> String {
    format!("\"{}\"", text.replace('\\', "\\\\").replace('"', "\\\""))
}

/// `depots.txt`: each depot's launch option and files.
fn layouts(ids: &Ids, found: &[(Package, Vec<(String, u64)>)]) -> String {
    let mut text = format!("Steam depots for {}, written by `cargo xtask steampipe`.\n", found[0].0.build_id);
    if ids.placeholder {
        text.push_str(&format!("The IDs are placeholders ({IDS_FILE}).\n"));
    }
    for (package, files) in found {
        let total: u64 = files.iter().map(|(_, size)| size).sum();
        let _ = write!(
            text,
            "\nDepot {} ({}): {}/{} as {}, {} files, {total} bytes\nLaunch option: {}\n",
            ids.depot(&package.platform),
            package.platform,
            package.platform,
            package.app_path,
            package.depot_path(),
            files.len(),
            package.launch()
        );
        for (name, size) in files {
            let _ = writeln!(text, "  {size:>12}  {name}");
        }
    }
    text
}

/// Every file under a package's app, as `<app path>/<path>` with forward slashes, sorted, with its size.
fn files_of(dir: &Path, package: &Package) -> anyhow::Result<Vec<(String, u64)>> {
    let mut files = Vec::new();
    walk(&dir.join(&package.app_path), &package.app_path, &mut files)?;
    files.sort();
    Ok(files)
}

fn walk(dir: &Path, prefix: &str, files: &mut Vec<(String, u64)>) -> anyhow::Result<()> {
    for entry in fs::read_dir(dir).with_context(|| format!("failed to read {}", dir.display()))? {
        let entry = entry?;
        let name = format!("{prefix}/{}", entry.file_name().to_string_lossy());
        let kind = entry.file_type()?;
        if kind.is_dir() {
            walk(&entry.path(), &name, files)?;
        } else if kind.is_file() {
            files.push((name, entry.metadata()?.len()));
        } else {
            bail!("{name} is neither a file nor a folder, which a depot can't hold");
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const PLACEHOLDERS: &str =
        "placeholder = true\napp-id = 1000\n[depots]\nwindows = 1001\nlinux = 1002\nmacos = 1003\n";

    #[test]
    fn the_committed_ids_are_marked_placeholders() {
        let ids = Ids::load().unwrap();
        assert!(ids.placeholder, "until the maintainer chooses, {IDS_FILE} holds placeholders");
        assert_eq!(ids.app_id, PLACEHOLDER_APP);
    }

    #[test]
    fn placeholders_and_chosen_ids_dont_mix() {
        Ids::parse(PLACEHOLDERS).unwrap();
        let chosen = "app-id = 3412340\n[depots]\nwindows = 3412341\nlinux = 3412342\nmacos = 3412343\n";
        assert!(!Ids::parse(chosen).unwrap().placeholder);
        let unmarked = PLACEHOLDERS.replace("placeholder = true\n", "");
        assert!(Ids::parse(&unmarked).unwrap_err().to_string().contains("example IDs"));
        let marked = format!("placeholder = true\n{chosen}");
        assert!(Ids::parse(&marked).unwrap_err().to_string().contains("delete it"));
        let shared = "app-id = 3412340\n[depots]\nwindows = 3412341\nlinux = 3412341\nmacos = 3412343\n";
        assert!(Ids::parse(shared).unwrap_err().to_string().contains("four different"));
        assert!(Ids::parse(&format!("{PLACEHOLDERS}steam = 1\n")).is_err(), "unknown fields are refused");
    }

    /// A gathered set of packages, as three hosts made them.
    fn packages(name: &str, platforms: &[&str], commit: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("xtask-steampipe-{name}-{}", std::process::id()));
        workspace::remove_dir_if_exists(&dir).unwrap();
        for platform in platforms {
            let app_path = if *platform == "macos" { "SapphireGame.app" } else { "SapphireGame" };
            let package = dir.join(platform);
            let app = package.join(app_path);
            fs::create_dir_all(app.join("content")).unwrap();
            fs::write(app.join("content/shared.spack"), [0u8; 10]).unwrap();
            let exe = if *platform == "windows" { "game_app.exe" } else { "game_app" };
            fs::write(app.join(exe), [0u8; 4]).unwrap();
            fs::create_dir_all(package.join("symbols")).unwrap();
            fs::write(package.join("symbols/game_app.pdb"), [0u8; 7]).unwrap();
            let json = serde_json::json!({
                "app": "game_app",
                "platform": platform,
                "build": { "id": "0.1.0+1188", "kind": "release", "commit": commit, "number": 1188 },
                "app_path": app_path,
            });
            fs::write(package.join("package.json"), json.to_string()).unwrap();
        }
        dir
    }

    #[test]
    fn writes_an_app_build_and_a_depot_for_each_package() {
        let dir = packages("all", &PLATFORMS, "1a2b3c4d5e6f7a8b");
        let out = steampipe(&dir).unwrap();
        let app = fs::read_to_string(out.join("app_build_1000.vdf")).unwrap();
        assert!(app.contains("// PLACEHOLDER IDs"), "{app}");
        assert!(app.contains("\t\"AppID\" \"1000\" // PLACEHOLDER\n"), "{app}");
        assert!(app.contains("\t\"ContentRoot\" \"../\"\n"), "{app}");
        for depot in [1001, 1002, 1003] {
            assert!(
                app.contains(&format!("\t\t\"{depot}\" \"depot_build_{depot}.vdf\" // PLACEHOLDER\n")),
                "{app}"
            );
        }
        assert!(!app.contains("SetLive"), "nothing goes live from a script");
        let windows = fs::read_to_string(out.join("depot_build_1001.vdf")).unwrap();
        assert!(windows.contains("\"LocalPath\" \"windows/SapphireGame/*\""), "{windows}");
        assert!(windows.contains("\"DepotPath\" \".\""), "{windows}");
        let macos = fs::read_to_string(out.join("depot_build_1003.vdf")).unwrap();
        assert!(macos.contains("\"LocalPath\" \"macos/SapphireGame.app/*\""), "{macos}");
        assert!(macos.contains("\"DepotPath\" \"SapphireGame.app\""), "{macos}");
        let layouts = fs::read_to_string(out.join("depots.txt")).unwrap();
        assert!(layouts.contains("Depot 1001 (windows): windows/SapphireGame as ., 2 files, 14 bytes\nLaunch option: game_app.exe\n"), "{layouts}");
        assert!(layouts.contains("Launch option: SapphireGame.app\n"), "{layouts}");
        assert!(layouts.contains("Launch option: game_app\n"), "{layouts}");
        assert!(!layouts.contains("symbols"), "symbols never go in a depot: {layouts}");
        workspace::remove_dir_if_exists(&dir).unwrap();
    }

    #[test]
    fn depots_come_from_one_build() {
        let dir = packages("mixed", &["windows", "macos"], "1a2b3c4d5e6f7a8b");
        let mac = dir.join("macos/package.json");
        let text = fs::read_to_string(&mac).unwrap().replace("1a2b3c4d5e6f7a8b", "ffffffffffffffff");
        fs::write(&mac, text).unwrap();
        assert!(steampipe(&dir).unwrap_err().to_string().contains("one build"));
        workspace::remove_dir_if_exists(&dir).unwrap();

        // A missing desktop is left out, with a warning; none at all is an error.
        let dir = packages("some", &["windows", "macos"], "1a2b3c4d5e6f7a8b");
        let app = fs::read_to_string(steampipe(&dir).unwrap().join("app_build_1000.vdf")).unwrap();
        assert!(app.contains("\"1001\"") && app.contains("\"1003\"") && !app.contains("\"1002\""), "{app}");
        workspace::remove_dir_if_exists(&dir).unwrap();
        let empty = packages("none", &[], "x");
        fs::create_dir_all(&empty).unwrap();
        assert!(steampipe(&empty).unwrap_err().to_string().contains("no desktop package"));
        workspace::remove_dir_if_exists(&empty).unwrap();
    }

    #[test]
    fn a_package_must_say_where_its_app_is() {
        let dir = packages("wrong", &["linux"], "1a2b3c4d5e6f7a8b");
        let json = dir.join("linux/package.json");
        let text = fs::read_to_string(&json).unwrap();
        fs::write(&json, text.replace("\"SapphireGame\"", "\"../elsewhere\"")).unwrap();
        assert!(Package::read(&dir.join("linux"), "linux").unwrap_err().to_string().contains("isn't a name"));
        fs::write(&json, text.replace("\"linux\"", "\"windows\"")).unwrap();
        assert!(Package::read(&dir.join("linux"), "linux").unwrap_err().to_string().contains("not linux's"));
        workspace::remove_dir_if_exists(&dir).unwrap();
        assert_eq!(quoted("a\"b\\c"), "\"a\\\"b\\\\c\"");
    }
}
