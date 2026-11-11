//! What a Linux executable needs of the system it runs on, read from its bytes (M4 S4): the libraries it
//! names (`DT_NEEDED`) and the symbol versions it needs of each (`.gnu.version_r`), which say the oldest
//! glibc and libstdc++ it runs on. Read from the section headers, which `strip --strip-all` keeps for
//! everything the loader uses. The formats are the System V ABI's and the Linux Standard Base's
//! ("Symbol Versioning"). Only 64-bit little-endian files are read, and anything malformed is an error,
//! never a panic.

use anyhow::{Context as _, ensure};

/// `EM_X86_64`.
pub const X86_64: u16 = 62;

const SHT_DYNAMIC: u32 = 6;
const SHT_GNU_VERNEED: u32 = 0x6fff_fffe;
const DT_NULL: u64 = 0;
const DT_NEEDED: u64 = 1;

/// One version a binary needs of a library: `GLIBC_2.31` of `libc.so.6`.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct VersionNeed {
    pub library: String,
    pub version: String,
}

/// What a binary needs to load.
#[derive(Debug, PartialEq, Eq)]
pub struct Needs {
    /// `e_machine`.
    pub machine: u16,
    /// The libraries it names, in its order.
    pub libraries: Vec<String>,
    /// Every version it needs, sorted.
    pub versions: Vec<VersionNeed>,
}

impl Needs {
    pub fn read(bytes: &[u8]) -> anyhow::Result<Self> {
        let file = Bytes(bytes);
        ensure!(bytes.starts_with(b"\x7fELF"), "not an ELF file");
        ensure!(bytes.get(4..6) == Some(&[2, 1]), "not a 64-bit little-endian ELF file");
        let machine = file.u16(0x12)?;
        let (table, size, count) = (file.u64(0x28)?, u64::from(file.u16(0x3a)?), file.u16(0x3c)?);
        ensure!(table != 0 && count != 0, "the ELF file has no section headers");
        ensure!(size >= 64, "section headers of {size} bytes are too small");
        let sections: Vec<Section> = (0..u64::from(count))
            .map(|index| Section::read(file, table + index * size))
            .collect::<anyhow::Result<_>>()?;
        let strings = |section: &Section| -> anyhow::Result<Strings<'_>> {
            let table = sections
                .get(usize::try_from(section.link)?)
                .context("a section links to a string table that isn't there")?;
            Ok(Strings(file.slice(table.offset, table.size)?))
        };

        let mut libraries = Vec::new();
        for dynamic in sections.iter().filter(|section| section.kind == SHT_DYNAMIC) {
            let names = strings(dynamic)?;
            for entry in 0..dynamic.size / 16 {
                let at = dynamic.offset + entry * 16;
                match file.u64(at)? {
                    DT_NULL => break,
                    DT_NEEDED => libraries.push(names.get(file.u64(at + 8)?)?),
                    _ => {}
                }
            }
        }

        let mut versions = Vec::new();
        for needed in sections.iter().filter(|section| section.kind == SHT_GNU_VERNEED) {
            let names = strings(needed)?;
            let mut at = needed.offset;
            for _ in 0..needed.info {
                let count = file.u16(at + 2)?;
                let library = names.get(u64::from(file.u32(at + 4)?))?;
                let mut aux = at + u64::from(file.u32(at + 8)?);
                for _ in 0..count {
                    let version = names.get(u64::from(file.u32(aux + 8)?))?;
                    versions.push(VersionNeed { library: library.clone(), version });
                    aux += u64::from(file.u32(aux + 12)?);
                }
                let next = file.u32(at + 12)?;
                if next == 0 {
                    break;
                }
                at += u64::from(next);
            }
        }
        versions.sort();
        versions.dedup();
        Ok(Self { machine, libraries, versions })
    }
}

struct Section {
    kind: u32,
    offset: u64,
    size: u64,
    link: u32,
    info: u32,
}

impl Section {
    fn read(file: Bytes<'_>, at: u64) -> anyhow::Result<Self> {
        Ok(Self {
            kind: file.u32(at + 4)?,
            offset: file.u64(at + 24)?,
            size: file.u64(at + 32)?,
            link: file.u32(at + 40)?,
            info: file.u32(at + 44)?,
        })
    }
}

/// A string table.
struct Strings<'a>(&'a [u8]);

impl Strings<'_> {
    fn get(&self, at: u64) -> anyhow::Result<String> {
        let tail =
            usize::try_from(at).ok().and_then(|at| self.0.get(at..)).context("a name is past its table")?;
        let end = tail.iter().position(|&b| b == 0).context("a name runs past its table")?;
        String::from_utf8(tail[..end].to_vec()).context("a name isn't UTF-8")
    }
}

/// Bounds-checked little-endian reads.
#[derive(Clone, Copy)]
struct Bytes<'a>(&'a [u8]);

impl<'a> Bytes<'a> {
    fn slice(self, at: u64, len: u64) -> anyhow::Result<&'a [u8]> {
        let at = usize::try_from(at).ok();
        let len = usize::try_from(len).ok();
        at.zip(len)
            .and_then(|(at, len)| self.0.get(at..at.checked_add(len)?))
            .context("the ELF file is cut short")
    }

    fn u16(self, at: u64) -> anyhow::Result<u16> {
        Ok(u16::from_le_bytes(self.slice(at, 2)?.try_into()?))
    }

    fn u32(self, at: u64) -> anyhow::Result<u32> {
        Ok(u32::from_le_bytes(self.slice(at, 4)?.try_into()?))
    }

    fn u64(self, at: u64) -> anyhow::Result<u64> {
        Ok(u64::from_le_bytes(self.slice(at, 8)?.try_into()?))
    }
}

/// A writer of what [`Needs::read`] reads, for tests here and in the package's checks: an x86-64 ELF
/// with a dynamic section, a version needs section and their one string table, as a linker lays them
/// out (section headers last).
#[cfg(test)]
pub fn build(libraries: &[&str], needs: &[(&str, &[&str])]) -> Vec<u8> {
    let mut strings = vec![0u8];
    let mut name = |text: &str| {
        let at = strings.len() as u64;
        strings.extend_from_slice(text.as_bytes());
        strings.push(0);
        at
    };
    let mut dynamic = Vec::new();
    for library in libraries {
        dynamic.extend_from_slice(&DT_NEEDED.to_le_bytes());
        dynamic.extend_from_slice(&name(library).to_le_bytes());
    }
    dynamic.extend_from_slice(&[0; 16]);
    let mut verneed = Vec::new();
    for (index, (library, versions)) in needs.iter().enumerate() {
        let file = name(library) as u32;
        let size = 16 + versions.len() * 16;
        let next = if index + 1 == needs.len() { 0 } else { size as u32 };
        verneed.extend_from_slice(&1u16.to_le_bytes());
        verneed.extend_from_slice(&(versions.len() as u16).to_le_bytes());
        verneed.extend_from_slice(&file.to_le_bytes());
        verneed.extend_from_slice(&16u32.to_le_bytes());
        verneed.extend_from_slice(&next.to_le_bytes());
        for (aux, version) in versions.iter().enumerate() {
            let name = name(version) as u32;
            let next: u32 = if aux + 1 == versions.len() { 0 } else { 16 };
            verneed.extend_from_slice(&0u32.to_le_bytes());
            verneed.extend_from_slice(&0u16.to_le_bytes());
            verneed.extend_from_slice(&((aux + 2) as u16).to_le_bytes());
            verneed.extend_from_slice(&name.to_le_bytes());
            verneed.extend_from_slice(&next.to_le_bytes());
        }
    }

    let mut out = vec![0u8; 64];
    out[..4].copy_from_slice(b"\x7fELF");
    out[4..7].copy_from_slice(&[2, 1, 1]);
    out[0x12..0x14].copy_from_slice(&X86_64.to_le_bytes());
    let place = |out: &mut Vec<u8>, data: &[u8]| {
        let at = out.len().next_multiple_of(8) as u64;
        out.resize(at as usize, 0);
        out.extend_from_slice(data);
        at
    };
    let strings_at = place(&mut out, &strings);
    let dynamic_at = place(&mut out, &dynamic);
    let verneed_at = place(&mut out, &verneed);
    let table = out.len().next_multiple_of(8) as u64;
    out.resize(table as usize, 0);
    // The null section, then .dynstr (1), .dynamic and .gnu.version_r, both linking to it.
    let headers: [(u32, u64, usize, u32, u32); 4] = [
        (0, 0, 0, 0, 0),
        (3, strings_at, strings.len(), 0, 0),
        (SHT_DYNAMIC, dynamic_at, dynamic.len(), 1, 0),
        (SHT_GNU_VERNEED, verneed_at, verneed.len(), 1, needs.len() as u32),
    ];
    for (kind, offset, size, link, info) in headers {
        let mut header = [0u8; 64];
        header[4..8].copy_from_slice(&kind.to_le_bytes());
        header[24..32].copy_from_slice(&offset.to_le_bytes());
        header[32..40].copy_from_slice(&(size as u64).to_le_bytes());
        header[40..44].copy_from_slice(&link.to_le_bytes());
        header[44..48].copy_from_slice(&info.to_le_bytes());
        out.extend_from_slice(&header);
    }
    out[0x28..0x30].copy_from_slice(&table.to_le_bytes());
    out[0x3a..0x3c].copy_from_slice(&64u16.to_le_bytes());
    out[0x3c..0x3e].copy_from_slice(&(headers.len() as u16).to_le_bytes());
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn need(library: &str, version: &str) -> VersionNeed {
        VersionNeed { library: library.into(), version: version.into() }
    }

    #[test]
    fn reads_libraries_and_version_needs() {
        let elf = build(
            &["libm.so.6", "libgcc_s.so.1", "libc.so.6"],
            &[("libgcc_s.so.1", &["GCC_3.0"]), ("libc.so.6", &["GLIBC_2.2.5", "GLIBC_2.28", "GLIBC_2.3"])],
        );
        let needs = Needs::read(&elf).unwrap();
        assert_eq!(needs.machine, X86_64);
        assert_eq!(needs.libraries, ["libm.so.6", "libgcc_s.so.1", "libc.so.6"]);
        assert_eq!(
            needs.versions,
            [
                need("libc.so.6", "GLIBC_2.2.5"),
                need("libc.so.6", "GLIBC_2.28"),
                need("libc.so.6", "GLIBC_2.3"),
                need("libgcc_s.so.1", "GCC_3.0"),
            ]
        );
        let none = Needs::read(&build(&["libc.so.6"], &[])).unwrap();
        assert!(none.versions.is_empty());
    }

    #[test]
    fn refuses_what_isnt_a_64_bit_elf_file() {
        assert!(Needs::read(b"MZ\x90\x00").unwrap_err().to_string().contains("not an ELF"));
        let mut elf32 = build(&[], &[]);
        elf32[4] = 1;
        assert!(Needs::read(&elf32).unwrap_err().to_string().contains("64-bit"));
        let elf = build(&["libc.so.6"], &[("libc.so.6", &["GLIBC_2.17"])]);
        for cut in [32, 70, elf.len() - 20] {
            assert!(Needs::read(&elf[..cut]).is_err(), "cut at {cut}");
        }
        let mut no_sections = elf.clone();
        no_sections[0x3c..0x3e].copy_from_slice(&[0, 0]);
        assert!(Needs::read(&no_sections).unwrap_err().to_string().contains("no section headers"));
    }
}
