//! `sapphire_soundgen [--specs <dir>] [--out <dir>] [--force]`: writes each spec's sound under the
//! content directory, from `tools/sounds` into `content` by default, from the workspace root. A file
//! that's missing is written; one that differs is kept unless `--force` replaces it, since another
//! machine's floating point can make the committed sounds a few bits apart. See the library's docs.

use std::path::PathBuf;

use anyhow::{Context, bail};

const USAGE: &str = "usage: sapphire_soundgen [--specs <dir>] [--out <dir>] [--force]";

fn main() -> anyhow::Result<()> {
    let (mut specs, mut out, mut force) = (PathBuf::from("tools/sounds"), PathBuf::from("content"), false);
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--specs" => specs = args.next().context(USAGE)?.into(),
            "--out" => out = args.next().context(USAGE)?.into(),
            "--force" => force = true,
            _ => bail!("unknown argument {arg:?}\n{USAGE}"),
        }
    }
    let sounds = sapphire_soundgen::read_all(&specs)?;
    if sounds.is_empty() {
        bail!("no `*{}` specs in {}", sapphire_soundgen::SPEC_EXTENSION, specs.display());
    }
    for (_, sound) in &sounds {
        let mut file = sapphire_soundgen::generate(sound);
        let bytes = file.encode()?;
        let path = out.join(&file.path);
        let summary = format!("{} ({} frames, {} bytes)", file.path, file.samples.len(), bytes.len());
        match std::fs::read(&path) {
            Ok(existing) if existing == bytes => {
                eprintln!("unchanged {summary}");
                continue;
            }
            Ok(_) if !force => {
                eprintln!("differs, kept (--force replaces it) {summary}");
                continue;
            }
            Ok(_) => eprintln!("replaced {summary}"),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => eprintln!("created {summary}"),
            Err(error) => return Err(error).with_context(|| format!("reading {}", path.display())),
        }
        std::fs::create_dir_all(path.parent().context("a file has a folder")?)?;
        std::fs::write(&path, &bytes).with_context(|| format!("writing {}", path.display()))?;
    }
    Ok(())
}
