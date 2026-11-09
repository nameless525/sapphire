//! `sapphire_soundgen [content dir]`: writes the placeholder music and voice lines under the content
//! directory, `content` by default. See the library's docs.

use std::path::PathBuf;

use anyhow::Context;

fn main() -> anyhow::Result<()> {
    let content = PathBuf::from(std::env::args().nth(1).unwrap_or_else(|| "content".to_owned()));
    for mut file in sapphire_soundgen::generate() {
        let bytes = file.encode()?;
        let path = content.join(file.path);
        std::fs::create_dir_all(path.parent().context("a file has a folder")?)?;
        std::fs::write(&path, &bytes).with_context(|| format!("writing {}", path.display()))?;
        eprintln!("{} ({} frames, {} bytes)", file.path, file.samples.len(), bytes.len());
    }
    Ok(())
}
