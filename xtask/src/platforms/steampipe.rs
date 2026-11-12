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
//!
//! **Read back** (M4 S7): once written, the scripts are read as SteamPipe reads them ([`check`]): the app
//! build's ID, content root and depots, each depot's ID and mapping, the mapped folder's files against
//! `depots.txt`, and each launch option's executable in its depot. `--all` also fails unless all three
//! desktops are there, as `packages.yml`'s `steampipe` job runs it on the three jobs' artifacts.

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

/// Writes `<packages>/steam/` from the desktop packages there, reads it back ([`check`]) and returns its path
/// and what the check found. With `all`, a missing desktop is an error rather than a warning.
pub fn steampipe(packages: &Path, all: bool) -> anyhow::Result<(PathBuf, String)> {
    let ids = Ids::load()?;
    let mut found = Vec::new();
    for platform in PLATFORMS {
        let dir = packages.join(platform);
        if dir.join("package.json").is_file() {
            let package = Package::read(&dir, platform)?;
            let files = files_of(&dir, &package)?;
            found.push((package, files));
        } else {
            ensure!(
                !all,
                "no {platform} package in {}, and --all needs all three desktops",
                packages.display()
            );
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
    let checked = check(&out, &ids, &found).context("the SteamPipe scripts don't read back as written")?;
    Ok((out, checked))
}

/// Reads the scripts in `out` back as SteamPipe reads them, against the packages they were written from: the
/// app build names the app, a content root and one depot script per package; each depot script names its
/// depot and maps its package's app folder, recursively, to the depot's path; that folder, found from the
/// content root, holds the files `depots.txt` lists; and each launch option's executable is in its depot.
/// Returns two lines per depot.
fn check(out: &Path, ids: &Ids, found: &[(Package, Vec<(String, u64)>)]) -> anyhow::Result<String> {
    let read = |name: &str| -> anyhow::Result<Vdf> {
        let text = fs::read_to_string(out.join(name)).with_context(|| format!("failed to read {name}"))?;
        Vdf::parse(&text).with_context(|| format!("{name} isn't VDF"))
    };
    let app_name = format!("app_build_{}.vdf", ids.app_id);
    let app_file = read(&app_name)?;
    let app = app_file.get("AppBuild").with_context(|| format!("{app_name} has no AppBuild"))?;
    ensure!(app.text("AppID")? == ids.app_id.to_string(), "{app_name} names app {}", app.text("AppID")?);
    let root = out.join(app.text("ContentRoot")?);
    let depots = app.get("Depots").with_context(|| format!("{app_name} has no Depots"))?;
    let wanted: Vec<String> =
        found.iter().map(|(package, _)| ids.depot(&package.platform).to_string()).collect();
    let named: Vec<&str> = depots.keys().collect();
    ensure!(named == wanted, "{app_name} names depots {named:?}, not the packages' {wanted:?}");

    let listed = fs::read_to_string(out.join("depots.txt")).context("failed to read depots.txt")?;
    let mut report = String::new();
    for (package, files) in found {
        let depot = ids.depot(&package.platform).to_string();
        let script_name = depots.text(&depot)?;
        let script = read(script_name)?;
        let build = script.get("DepotBuild").with_context(|| format!("{script_name} has no DepotBuild"))?;
        ensure!(build.text("DepotID")? == depot, "{script_name} names depot {}", build.text("DepotID")?);
        let mapping =
            build.get("FileMapping").with_context(|| format!("{script_name} has no FileMapping"))?;
        ensure!(mapping.text("Recursive")? == "1", "{script_name} doesn't map recursively");
        ensure!(
            mapping.text("DepotPath")? == package.depot_path(),
            "{script_name} puts the app at {}",
            mapping.text("DepotPath")?
        );
        let local = mapping.text("LocalPath")?;
        let folder = local
            .strip_suffix("/*")
            .with_context(|| format!("{script_name} maps {local}, not a folder's files"))?;
        ensure!(
            folder == format!("{}/{}", package.platform, package.app_path),
            "{script_name} maps {folder}, not the {} package's app",
            package.platform
        );
        let mut mapped = Vec::new();
        walk(&root.join(folder), &package.app_path, &mut mapped)
            .with_context(|| format!("{script_name} maps {folder}, which its content root doesn't hold"))?;
        mapped.sort();
        ensure!(mapped == *files, "{folder} changed while the scripts were written");
        let total: u64 = files.iter().map(|(_, size)| size).sum();
        let line = format!(
            "Depot {depot} ({}): {folder} as {}, {} files, {total} bytes\n",
            package.platform,
            package.depot_path(),
            files.len()
        );
        ensure!(listed.contains(&line), "depots.txt doesn't list {line:?}");
        let executable = match package.platform.as_str() {
            "macos" => format!("{}/Contents/MacOS/{}", package.app_path, package.executable),
            _ => format!("{}/{}", package.app_path, package.launch()),
        };
        ensure!(
            files.iter().any(|(name, _)| *name == executable),
            "the {} depot's launch option needs {executable}, which the depot doesn't hold",
            package.platform
        );
        let _ = writeln!(report, "{script_name}: {}", line.trim_end());
        let _ = writeln!(report, "  launches {executable}");
    }
    Ok(report)
}

/// A VDF document as SteamPipe's scripts are written: quoted keys, each with a quoted value or a block in
/// braces, and `//` comments. Keys keep their order.
#[derive(Debug, PartialEq, Eq)]
enum Vdf {
    Text(String),
    Block(Vec<(String, Vdf)>),
}

#[derive(Debug, PartialEq, Eq)]
enum Token {
    Open,
    Close,
    Text(String),
}

impl Vdf {
    fn parse(text: &str) -> anyhow::Result<Self> {
        let mut tokens = Vec::new();
        let mut chars = text.chars().peekable();
        while let Some(c) = chars.next() {
            match c {
                '{' => tokens.push(Token::Open),
                '}' => tokens.push(Token::Close),
                '"' => {
                    let mut value = String::new();
                    loop {
                        match chars.next() {
                            Some('"') => break,
                            Some('\\') => value.push(chars.next().context("a string ends in a backslash")?),
                            Some(c) => value.push(c),
                            None => bail!("a string isn't closed"),
                        }
                    }
                    tokens.push(Token::Text(value));
                }
                '/' if chars.peek() == Some(&'/') => while chars.next_if(|&c| c != '\n').is_some() {},
                c if c.is_whitespace() => {}
                c => bail!("{c:?} outside a string"),
            }
        }
        let mut tokens = tokens.into_iter();
        Self::block(&mut tokens, false)
    }

    fn block(tokens: &mut impl Iterator<Item = Token>, nested: bool) -> anyhow::Result<Self> {
        let mut entries = Vec::new();
        loop {
            let key = match tokens.next() {
                Some(Token::Text(key)) => key,
                Some(Token::Close) if nested => return Ok(Self::Block(entries)),
                None if !nested => return Ok(Self::Block(entries)),
                other => bail!("expected a key, found {other:?}"),
            };
            let value = match tokens.next() {
                Some(Token::Text(value)) => Self::Text(value),
                Some(Token::Open) => Self::block(tokens, true)?,
                other => bail!("{key} has no value, but {other:?}"),
            };
            entries.push((key, value));
        }
    }

    fn get(&self, key: &str) -> Option<&Self> {
        match self {
            Self::Block(entries) => entries.iter().find(|(name, _)| name == key).map(|(_, value)| value),
            Self::Text(_) => None,
        }
    }

    fn text(&self, key: &str) -> anyhow::Result<&str> {
        match self.get(key) {
            Some(Self::Text(text)) => Ok(text),
            _ => bail!("no {key} value"),
        }
    }

    fn keys(&self) -> impl Iterator<Item = &str> {
        let entries = match self {
            Self::Block(entries) => entries.as_slice(),
            Self::Text(_) => &[],
        };
        entries.iter().map(|(name, _)| name.as_str())
    }
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
            let exe = match *platform {
                "windows" => "game_app.exe",
                "macos" => "Contents/MacOS/game_app",
                _ => "game_app",
            };
            fs::create_dir_all(app.join(exe).parent().unwrap()).unwrap();
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
        let (out, checked) = steampipe(&dir, true).unwrap();
        assert_eq!(
            checked,
            "depot_build_1001.vdf: Depot 1001 (windows): windows/SapphireGame as ., 2 files, 14 bytes\n  \
             launches SapphireGame/game_app.exe\n\
             depot_build_1002.vdf: Depot 1002 (linux): linux/SapphireGame as ., 2 files, 14 bytes\n  \
             launches SapphireGame/game_app\n\
             depot_build_1003.vdf: Depot 1003 (macos): macos/SapphireGame.app as SapphireGame.app, 2 files, 14 bytes\n  \
             launches SapphireGame.app/Contents/MacOS/game_app\n"
        );
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
        assert!(steampipe(&dir, false).unwrap_err().to_string().contains("one build"));
        workspace::remove_dir_if_exists(&dir).unwrap();

        // A missing desktop is left out, with a warning, unless all are wanted; none at all is an error.
        let dir = packages("some", &["windows", "macos"], "1a2b3c4d5e6f7a8b");
        let app = fs::read_to_string(steampipe(&dir, false).unwrap().0.join("app_build_1000.vdf")).unwrap();
        assert!(app.contains("\"1001\"") && app.contains("\"1003\"") && !app.contains("\"1002\""), "{app}");
        let error = steampipe(&dir, true).unwrap_err().to_string();
        assert!(error.starts_with("no linux package in") && error.contains("--all"), "{error}");
        workspace::remove_dir_if_exists(&dir).unwrap();
        let empty = packages("none", &[], "x");
        fs::create_dir_all(&empty).unwrap();
        assert!(steampipe(&empty, false).unwrap_err().to_string().contains("no desktop package"));
        workspace::remove_dir_if_exists(&empty).unwrap();
    }

    /// What the read-back refuses: a script that doesn't say what was written, or a depot whose launch
    /// option's executable isn't in it.
    #[test]
    fn the_scripts_are_read_back_as_steampipe_reads_them() {
        let dir = packages("check", &PLATFORMS, "1a2b3c4d5e6f7a8b");
        let (out, _) = steampipe(&dir, true).unwrap();
        let ids = Ids::load().unwrap();
        let found: Vec<_> = PLATFORMS
            .iter()
            .map(|platform| {
                let package = Package::read(&dir.join(platform), platform).unwrap();
                let files = files_of(&dir.join(platform), &package).unwrap();
                (package, files)
            })
            .collect();
        check(&out, &ids, &found).unwrap();

        let script = out.join("depot_build_1002.vdf");
        let text = fs::read_to_string(&script).unwrap();
        fs::write(&script, text.replace("linux/SapphireGame/*", "linux/SapphireGame/content/*")).unwrap();
        assert!(
            format!("{:#}", check(&out, &ids, &found).unwrap_err()).contains("not the linux package's app")
        );
        fs::write(&script, text.replace("\"DepotID\" \"1002\"", "\"DepotID\" \"1003\"")).unwrap();
        assert!(format!("{:#}", check(&out, &ids, &found).unwrap_err()).contains("names depot 1003"));
        fs::write(&script, text.replace("\"Recursive\" \"1\"\n\t}", "\"Recursive\" \"1\"\n")).unwrap();
        assert!(format!("{:#}", check(&out, &ids, &found).unwrap_err()).contains("isn't VDF"));
        fs::write(&script, &text).unwrap();

        let app = out.join("app_build_1000.vdf");
        let text = fs::read_to_string(&app).unwrap();
        fs::write(&app, text.replace("\"ContentRoot\" \"../\"", "\"ContentRoot\" \"./\"")).unwrap();
        assert!(
            format!("{:#}", check(&out, &ids, &found).unwrap_err()).contains("content root doesn't hold")
        );
        fs::write(&app, &text).unwrap();

        fs::remove_file(dir.join("linux/SapphireGame/game_app")).unwrap();
        let error = format!("{:#}", steampipe(&dir, true).unwrap_err());
        assert!(error.contains("needs SapphireGame/game_app, which the depot doesn't hold"), "{error}");
        workspace::remove_dir_if_exists(&dir).unwrap();

        let parsed =
            Vdf::parse("// a note\n\"A\" { \"b\" \"c \\\"d\\\"\" // trailing\n \"E\" {} }\n").unwrap();
        let a = parsed.get("A").unwrap();
        assert_eq!(a.text("b").unwrap(), "c \"d\"");
        assert_eq!(a.keys().collect::<Vec<_>>(), ["b", "E"]);
        assert!(Vdf::parse("\"A\" {").is_err() && Vdf::parse("A").is_err() && Vdf::parse("\"A\"").is_err());
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
