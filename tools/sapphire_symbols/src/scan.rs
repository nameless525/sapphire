//! The scan that proves a package carries none of the builder's directories (M4 C2): every file of the app
//! and its symbols is searched, byte for byte, for each forbidden path with either kind of slash.

use std::fs::{self, File};
use std::io::Read as _;
use std::path::{Path, PathBuf};

use anyhow::Context as _;
use memchr::memmem::Finder;

/// Where a forbidden path was found.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Found {
    pub file: PathBuf,
    pub offset: u64,
    pub forbidden: String,
    /// The printable bytes around it, for the report.
    pub context: String,
}

/// How much of a file is read at once.
const CHUNK: usize = 8 << 20;
/// How many places a file reports at most: one names the problem.
const MOST_PER_FILE: usize = 3;

/// Searches every file in `paths` (directories whole, following no links) for each of `forbidden`, as
/// written and with its slashes turned the other way.
pub fn scan(paths: &[PathBuf], forbidden: &[String]) -> anyhow::Result<Vec<Found>> {
    let mut needles: Vec<String> = Vec::new();
    for path in forbidden.iter().filter(|path| path.len() > 1) {
        for variant in [path.clone(), path.replace('/', "\\"), path.replace('\\', "/")] {
            if !needles.contains(&variant) {
                needles.push(variant);
            }
        }
    }
    let finders: Vec<Finder<'_>> = needles.iter().map(|needle| Finder::new(needle.as_bytes())).collect();
    let mut found = Vec::new();
    let mut files = Vec::new();
    for path in paths {
        collect_files(path, &mut files)?;
    }
    for file in files {
        scan_file(&file, &needles, &finders, &mut found)?;
    }
    Ok(found)
}

fn collect_files(path: &Path, files: &mut Vec<PathBuf>) -> anyhow::Result<()> {
    let metadata = fs::symlink_metadata(path).with_context(|| format!("can't read {}", path.display()))?;
    if metadata.is_dir() {
        let mut entries: Vec<PathBuf> = fs::read_dir(path)
            .with_context(|| format!("can't list {}", path.display()))?
            .map(|entry| entry.map(|entry| entry.path()))
            .collect::<Result<_, _>>()
            .with_context(|| format!("can't list {}", path.display()))?;
        entries.sort();
        for entry in entries {
            collect_files(&entry, files)?;
        }
    } else if metadata.is_file() {
        files.push(path.to_owned());
    }
    Ok(())
}

/// Reads `file` in chunks that overlap by the longest needle, so nothing is missed at a seam.
fn scan_file(
    file: &Path,
    needles: &[String],
    finders: &[Finder<'_>],
    found: &mut Vec<Found>,
) -> anyhow::Result<()> {
    let overlap = needles.iter().map(String::len).max().unwrap_or(0);
    let mut reader = File::open(file).with_context(|| format!("can't open {}", file.display()))?;
    let mut buffer = vec![0; CHUNK + overlap];
    let (mut kept, mut base, mut reported) = (0, 0u64, 0);
    loop {
        let read =
            reader.read(&mut buffer[kept..]).with_context(|| format!("can't read {}", file.display()))?;
        let filled = kept + read;
        let window = &buffer[..filled];
        for (needle, finder) in needles.iter().zip(finders) {
            for at in finder.find_iter(window) {
                // A match wholly inside the overlap was found in the last chunk.
                if at + needle.len() <= kept || reported >= MOST_PER_FILE {
                    continue;
                }
                reported += 1;
                let around = &window[at.saturating_sub(24)..(at + needle.len() + 48).min(filled)];
                let context =
                    around.iter().map(|&b| if (0x20..0x7f).contains(&b) { b as char } else { '.' }).collect();
                found.push(Found {
                    file: file.to_owned(),
                    offset: base + at as u64,
                    forbidden: needle.clone(),
                    context,
                });
            }
        }
        if read == 0 {
            return Ok(());
        }
        kept = overlap.min(filled);
        buffer.copy_within(filled - kept..filled, 0);
        base += (filled - kept) as u64;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_paths_with_either_slash_across_chunk_seams() {
        let dir = std::env::temp_dir().join(format!("sapphire_symbols-scan-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("nested")).unwrap();
        let mut big = vec![b'x'; CHUNK - 5];
        big.extend_from_slice(b"/srv/builds/work/a.rs");
        fs::write(dir.join("nested/big.bin"), &big).unwrap();
        fs::write(dir.join("windows.pdb"), b"\0C:\\x\\srv\\builds\\work\\b.rs\0").unwrap();
        fs::write(dir.join("clean.txt"), b"/sapphire/crates/a.rs /cargo/registry").unwrap();

        let found = scan(std::slice::from_ref(&dir), &["/srv/builds".to_owned()]).unwrap();
        let files: Vec<_> =
            found.iter().map(|found| found.file.strip_prefix(&dir).unwrap().to_owned()).collect();
        assert_eq!(files, [Path::new("nested/big.bin"), Path::new("windows.pdb")]);
        assert_eq!(found[0].offset, (CHUNK - 5) as u64);
        assert_eq!(found[1].forbidden, "\\srv\\builds");
        // The context ends where the chunk read so far does.
        assert!(found[0].context.contains("xx/srv/builds"), "{}", found[0].context);
        fs::remove_dir_all(dir).unwrap();
    }
}
