//! A panic report (`sapphire_crash`'s `<run>.panic-<n>.txt`): the panicking thread's raw return addresses,
//! and the modules they're in by load address and build ID. iOS has no other report.

use anyhow::{Context as _, ensure};

use super::{Crash, Reader, Thread};

pub(super) const TITLE: &str = "Sapphire panic report v2\n";
const MODULES: &str = "\n\nmodules:\n";
const FRAMES: &str = "\n\nframes:\n";
const MESSAGE: &str = "\n\nmessage:\n";
const BACKTRACE: &str = "\n\nbacktrace:\n";

/// A loaded module as a report or record lists it: where it's loaded, its build's ID, its path.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Loaded {
    pub base: u64,
    pub id: String,
    pub name: String,
}

impl Loaded {
    /// `0x<base> <id> <name>`, the name last since it may hold spaces.
    pub fn parse(line: &str) -> anyhow::Result<Self> {
        let mut parts = line.splitn(3, ' ');
        let (base, id, name) = (parts.next(), parts.next(), parts.next());
        let base = base.and_then(hex).with_context(|| format!("a module line {line:?} has no base"))?;
        Ok(Self { base, id: id.unwrap_or_default().to_owned(), name: name.unwrap_or_default().to_owned() })
    }
}

/// `0x`-prefixed hex.
pub(super) fn hex(text: &str) -> Option<u64> {
    u64::from_str_radix(text.trim().strip_prefix("0x")?, 16).ok()
}

/// The module `address` is in: the one loaded nearest below it. A report lists only modules that hold its
/// frames (Apple) or every module (Linux), so nearest below is the one.
pub(super) fn containing(modules: &[Loaded], address: u64) -> Option<&Loaded> {
    modules.iter().filter(|module| module.base <= address).max_by_key(|module| module.base)
}

pub(super) fn read(text: &str, reader: &Reader<'_>) -> anyhow::Result<Crash> {
    let rest = text.strip_prefix(TITLE).context("not a v2 panic report")?;
    let (header, rest) = rest.split_once(MODULES).context("a panic report with no modules")?;
    let (modules, rest) = rest.split_once(FRAMES).context("a panic report with no frames")?;
    let (frames, rest) = rest.split_once(MESSAGE).context("a panic report with no message")?;
    let message = rest.split_once(BACKTRACE).map_or(rest, |(message, _)| message);
    let field =
        |key: &str| header.lines().find_map(|line| line.strip_prefix(key)?.strip_prefix(": ")).unwrap_or("?");

    let modules: Vec<Loaded> =
        modules.lines().filter(|line| !line.is_empty()).map(Loaded::parse).collect::<anyhow::Result<_>>()?;
    let mut thread = Thread { name: field("thread").to_owned(), crashed: true, frames: Vec::new() };
    for line in frames.lines().filter(|line| !line.is_empty()) {
        let address = hex(line).with_context(|| format!("a frame {line:?} isn't an address"))?;
        // Every frame is a return address: the call is the byte before.
        let frame = match containing(&modules, address) {
            Some(loaded) => {
                let module = reader.module(&loaded.id)?;
                let offset = address - loaded.base;
                reader.frame(address, &loaded.name, module, offset, offset.saturating_sub(1))
            }
            None => super::Frame { address, ..super::Frame::default() },
        };
        thread.frames.push(frame);
    }
    ensure!(
        !thread.frames.is_empty() || !modules.is_empty(),
        "a panic report with neither frames nor modules"
    );
    let summary = vec![
        format!("panicked at {}: {}", field("location"), message.lines().next().unwrap_or_default()),
        format!("{} {} (run {})", field("app"), field("release"), field("run")),
    ];
    Ok(Crash { kind: "panic report", summary, threads: vec![thread] })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn module_lines_read_and_contain_their_frames() {
        let modules = [
            Loaded::parse("0x0000000100000000 0A6B168B-0000-0000-0000-000000000000 /x/My Game.app/Contents/MacOS/game_app").unwrap(),
            Loaded::parse("0x0000000180000000 1B6B168B-0000-0000-0000-000000000000 /usr/lib/libc.dylib").unwrap(),
        ];
        assert_eq!(modules[0].name, "/x/My Game.app/Contents/MacOS/game_app");
        assert_eq!(containing(&modules, 0x1_0000_1234).map(|m| m.base), Some(0x1_0000_0000));
        assert_eq!(containing(&modules, 0x1_8000_0010).map(|m| m.base), Some(0x1_8000_0000));
        assert_eq!(containing(&modules, 0x10), None);
        assert!(Loaded::parse("nonsense").is_err());
    }
}
