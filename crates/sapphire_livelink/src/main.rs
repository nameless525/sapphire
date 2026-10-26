//! The desktop half of live link (see the crate docs): pushes to a development build and reads its
//! state. `cargo xtask link <command>` builds and runs it, passing `--content target/content`.

use std::ffi::OsString;
use std::net::{SocketAddr, TcpListener};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Duration;

use anyhow::{Context as _, bail};
use sapphire_hot_reload::{DEBOUNCE, FileWatcher, WatchId, sources};
use sapphire_livelink::protocol::{DeviceStatus, PackOutcome, SourceFile};
use sapphire_livelink::{Client, DEFAULT_PORT, LinkError};

const USAGE: &str = "\
Usage: sapphire_livelink [--addr <host:port> | --listen <host:port>] [--content <dir>] <command>

Talks to a development build's live link (spec §5.29): connects to it at --addr (default
127.0.0.1:7811), or with --listen waits for a device that dials (behind `adb reverse`).

Commands:
  status
      The device's app, platform and cook, its frame time, its assets, and the generation of
      every asset replaced since it started.
  cvar <name> <value>
      Set a cvar, as the dev console does.
  push [<file>...]
      Push packs (.spack, named by their stem: only the assets the device lacks are sent), RON
      documents (.ron: the device applies them by the type their envelope names) and WGSL
      sources (.wgsl). With no files, push every pack of the device's cook in --content.
  watch
      Push what changes, until interrupted: the packs of the device's cook in --content when
      `cargo xtask content --cook <cook>` rewrites them, the renderer's WGSL sources, the default
      day's timeline and the character light rig presets.

Options:
  --addr <host:port>    The device to connect to.
  --listen <host:port>  Wait for the device to dial here instead.
  --content <dir>       Where cooked content is, a directory per cook (`target/content`).";

/// How often `watch` collects what its watcher reported.
const POLL: Duration = Duration::from_millis(50);

fn main() -> ExitCode {
    match run(std::env::args().skip(1).collect()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("error: {error:#}");
            ExitCode::FAILURE
        }
    }
}

fn run(args: Vec<String>) -> anyhow::Result<()> {
    let mut target = Target::Connect(SocketAddr::from(([127, 0, 0, 1], DEFAULT_PORT)));
    let mut content = None;
    let mut rest = Vec::new();
    let mut args = args.into_iter();
    while let Some(arg) = args.next() {
        let mut value = |name: &str| args.next().with_context(|| format!("{name} needs a value"));
        match arg.as_str() {
            "--addr" => target = Target::Connect(address(&value("--addr")?)?),
            "--listen" => target = Target::Listen(address(&value("--listen")?)?, None),
            "--content" => content = Some(PathBuf::from(value("--content")?)),
            "help" | "--help" | "-h" => {
                eprintln!("{USAGE}");
                return Ok(());
            }
            _ => rest.push(arg),
        }
    }
    let Some((command, rest)) = rest.split_first() else { bail!("no command given\n\n{USAGE}") };
    match (command.as_str(), rest) {
        ("status", []) => {
            let status = target.connect()?.status()?;
            print_status(&status);
            Ok(())
        }
        ("cvar", [name, value]) => {
            let (name, value) = target.connect()?.set_cvar(name, value)?;
            eprintln!("{name} = {value}");
            Ok(())
        }
        ("push", files) => push(&mut target, content.as_deref(), files),
        ("watch", []) => watch(&mut target, content.as_deref()),
        (command, rest) => bail!("unknown command or arguments: {command} {}\n\n{USAGE}", rest.join(" ")),
    }
}

fn address(text: &str) -> anyhow::Result<SocketAddr> {
    text.parse()
        .with_context(|| format!("`{text}` isn't an address and port, such as 127.0.0.1:{DEFAULT_PORT}"))
}

/// Where the device is.
enum Target {
    Connect(SocketAddr),
    /// Listening for the device to dial, once bound.
    Listen(SocketAddr, Option<TcpListener>),
}

impl Target {
    fn connect(&mut self) -> anyhow::Result<Client> {
        match self {
            Self::Connect(address) => Client::connect(*address).with_context(|| {
                format!(
                    "no live link at {address}: is a dev build running (`--features dev`), with its port forwarded \
                     on a phone?"
                )
            }),
            Self::Listen(address, listener) => {
                let listener = match listener {
                    Some(listener) => listener,
                    None => listener.insert(TcpListener::bind(*address).with_context(|| format!("can't listen at {address}"))?),
                };
                eprintln!("waiting for a device to dial {address}");
                // A device that dialed and went (an app restarted) left a connection in the backlog
                // that handshakes from its buffer and then reads as closed: take the next.
                loop {
                    let mut client = Client::accept(listener)?;
                    if client.status().is_ok() {
                        return Ok(client);
                    }
                }
            }
        }
    }

    /// Connects again after the connection broke, until it works.
    fn reconnect(&mut self) -> Client {
        eprintln!("the device is gone; waiting for it");
        loop {
            match self.connect() {
                Ok(client) => {
                    eprintln!("connected again");
                    return client;
                }
                Err(_) => std::thread::sleep(Duration::from_secs(1)),
            }
        }
    }
}

/// Something to push.
#[derive(Debug)]
enum Item {
    Pack(PathBuf),
    Document(PathBuf),
    /// WGSL sources, pushed together.
    Shaders(Vec<PathBuf>),
}

impl Item {
    /// The files as items, by their extensions; WGSL files become one.
    fn from_files(files: Vec<PathBuf>) -> anyhow::Result<Vec<Self>> {
        let mut items = Vec::new();
        let mut shaders = Vec::new();
        for file in files {
            match file.extension().and_then(|extension| extension.to_str()) {
                Some(sapphire_assets::pack::PACK_EXTENSION) => items.push(Self::Pack(file)),
                Some("ron") => items.push(Self::Document(file)),
                Some("wgsl") => shaders.push(file),
                _ => bail!("{}: not a pack (.spack), document (.ron) or shader (.wgsl)", file.display()),
            }
        }
        if !shaders.is_empty() {
            items.push(Self::Shaders(shaders));
        }
        Ok(items)
    }

    /// Pushes it, and says what the device did.
    fn push(&self, client: &mut Client) -> anyhow::Result<()> {
        match self {
            Self::Pack(path) => {
                let name = path
                    .file_stem()
                    .and_then(|stem| stem.to_str())
                    .context("a pack's name is its file's stem")?;
                let bytes = std::fs::read(path).with_context(|| format!("can't read {}", path.display()))?;
                let outcome = client.push_pack(name, &bytes)?;
                print_pack(name, &outcome);
            }
            Self::Document(path) => {
                let text = std::fs::read_to_string(path)
                    .with_context(|| format!("can't read {}", path.display()))?;
                let origin = file_name(path);
                let owner = client.push_document(&origin, text)?;
                eprintln!("{origin}: replaced {owner}");
            }
            Self::Shaders(paths) => {
                let files = paths
                    .iter()
                    .map(|path| {
                        let text = std::fs::read_to_string(path)
                            .with_context(|| format!("can't read {}", path.display()))?;
                        Ok(SourceFile { name: file_name(path), text })
                    })
                    .collect::<anyhow::Result<Vec<_>>>()?;
                let names: Vec<String> = files.iter().map(|file| file.name.clone()).collect();
                let (programs, pipelines) = client.push_shaders(files)?;
                eprintln!("{}: {programs} programs and {pipelines} pipelines rebuilt", names.join(", "));
            }
        }
        Ok(())
    }
}

fn file_name(path: &Path) -> String {
    path.file_name().map_or_else(|| path.display().to_string(), |name| name.to_string_lossy().into_owned())
}

/// `push`: the files given, or every pack of the device's cook.
fn push(target: &mut Target, content: Option<&Path>, files: &[String]) -> anyhow::Result<()> {
    let mut client = target.connect()?;
    let files = if files.is_empty() {
        let content = content.context("give files to push, or --content to push the device's cook")?;
        let status = client.status()?;
        let dir = content.join(&status.cook);
        let packs = packs_in(&dir)?;
        if packs.is_empty() {
            bail!(
                "no packs in {}: cook them with `cargo xtask content --cook {}`",
                dir.display(),
                status.cook
            );
        }
        packs
    } else {
        files.iter().map(PathBuf::from).collect()
    };
    let mut failed = 0;
    for item in Item::from_files(files)? {
        if let Err(error) = item.push(&mut client) {
            eprintln!("error: {error:#}");
            failed += 1;
        }
    }
    if failed > 0 {
        bail!("{failed} of the pushes failed; the device kept what it had for them");
    }
    Ok(())
}

/// The packs in `dir`, by name.
fn packs_in(dir: &Path) -> anyhow::Result<Vec<PathBuf>> {
    let mut packs = Vec::new();
    for entry in std::fs::read_dir(dir).with_context(|| format!("can't read {}", dir.display()))? {
        let path = entry?.path();
        if path.extension().is_some_and(|extension| extension == sapphire_assets::pack::PACK_EXTENSION) {
            packs.push(path);
        }
    }
    packs.sort();
    Ok(packs)
}

/// `watch`: pushes what changes, reconnecting when the device goes away.
fn watch(target: &mut Target, content: Option<&Path>) -> anyhow::Result<()> {
    let mut client = target.connect()?;
    let status = client.status()?;
    print_status(&status);
    let mut watcher = FileWatcher::new(DEBOUNCE)?;
    let packs = match content {
        Some(content) => {
            let dir = content.join(&status.cook);
            std::fs::create_dir_all(&dir)?;
            eprintln!(
                "watching {} (cook it with `cargo xtask content --cook {}`)",
                dir.display(),
                status.cook
            );
            Some((watcher.watch(&dir)?, dir))
        }
        None => {
            eprintln!("not watching packs: no --content");
            None
        }
    };
    let shader_dir = sources::shader_dir();
    let shaders = watcher.watch(&shader_dir)?;
    eprintln!("watching {}", shader_dir.display());
    let mut documents: Vec<(WatchId, OsString, PathBuf)> = Vec::new();
    for path in [sources::default_day(), sources::default_light_rigs()] {
        let dir = path.parent().context("a document is in a directory")?;
        let name = path.file_name().context("a document is a file")?.to_owned();
        documents.push((watcher.watch(dir)?, name, path.clone()));
        eprintln!("watching {}", path.display());
    }
    loop {
        let mut items = Vec::new();
        let events = watcher.poll();
        for error in events.errors {
            eprintln!("watching: {error}");
        }
        let mut shaders_changed = false;
        for changed in events.changed {
            let name = Path::new(&changed.name);
            let extension = name.extension().and_then(|extension| extension.to_str());
            if let Some((watch, dir)) = &packs
                && *watch == changed.watch
                && extension == Some(sapphire_assets::pack::PACK_EXTENSION)
            {
                let path = dir.join(name);
                // A pack removed, or the temporary name it was written under, is no push.
                if path.is_file() {
                    items.push(Item::Pack(path));
                }
            }
            if changed.watch == shaders && extension == Some("wgsl") {
                shaders_changed = true;
            }
            for (watch, file, path) in &documents {
                if *watch == changed.watch && *file == changed.name {
                    items.push(Item::Document(path.clone()));
                }
            }
        }
        if shaders_changed {
            // Every file, so a device that restarted since gets them all.
            let mut files: Vec<PathBuf> = std::fs::read_dir(&shader_dir)?
                .filter_map(|entry| Some(entry.ok()?.path()))
                .filter(|path| path.extension().is_some_and(|extension| extension == "wgsl"))
                .collect();
            files.sort();
            items.push(Item::Shaders(files));
        }
        for item in items {
            if let Err(error) = item.push(&mut client) {
                let broken = error.downcast_ref::<LinkError>().is_some_and(|error| {
                    matches!(error, LinkError::Io(_) | LinkError::Closed | LinkError::TimedOut(_))
                });
                if !broken {
                    eprintln!("error: {error:#}");
                    continue;
                }
                client = target.reconnect();
                if let Err(error) = item.push(&mut client) {
                    eprintln!("error: {error:#}");
                }
            }
        }
        std::thread::sleep(POLL);
    }
}

fn print_status(status: &DeviceStatus) {
    eprintln!("{} on {}, reading the {} cook", status.app, status.platform, status.cook);
    eprintln!("frame {}, {:.2} ms", status.frame, f64::from(status.frame_time_us) / 1000.0);
    let assets = &status.assets;
    eprintln!(
        "assets: {} ({} ready, {} loading, {} failed), {} reloading; {} reloads, {} failed",
        assets.assets,
        assets.ready,
        assets.loading,
        assets.failed,
        assets.reloading,
        assets.reloads,
        assets.reload_failures
    );
    eprintln!(
        "live link: {} applied, {} refused, {} received",
        status.applied,
        status.refused,
        size(status.received)
    );
    for asset in &status.generations {
        eprintln!("  {} {} is generation {}", asset.kind, asset.id, asset.generation);
    }
}

fn print_pack(name: &str, outcome: &PackOutcome) {
    let applied = if outcome.mounted {
        format!("{} changed, {} removed, {} reloading", outcome.changed, outcome.removed, outcome.reloading)
    } else {
        "not mounted by the app; served to whatever mounts it".to_owned()
    };
    eprintln!("pack {name}: sent {} of {}; {applied}", size(outcome.received), size(outcome.len));
    for asset in &outcome.reloaded {
        eprintln!("  {} {} is now generation {}", asset.kind, asset.id, asset.generation);
    }
    for asset in &outcome.failed {
        eprintln!("  {} {} kept its version: {}", asset.kind, asset.id, asset.error);
    }
}

/// `bytes` for people.
fn size(bytes: u64) -> String {
    match bytes {
        0..1_000 => format!("{bytes} B"),
        1_000..1_000_000 => format!("{:.1} KB", bytes as f64 / 1e3),
        _ => format!("{:.1} MB", bytes as f64 / 1e6),
    }
}
