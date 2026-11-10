//! Shipped binaries and their symbols (M4 C1): checks that a binary a package ships is stripped, that its
//! symbols (a dSYM, an unstripped ELF or a PDB) came from the same link, and names the module the way
//! Breakpad and minidumps do, so a crash's module finds them.
//!
//! A module's identity is its **debug file** and **debug ID**, which a Breakpad symbol store keys on
//! (`<debug file>/<debug ID>/<debug file without .pdb>.sym`):
//!
//! | Format | Debug file | Debug ID | Code ID |
//! |---|---|---|---|
//! | Mach-O | the executable's name | `LC_UUID`, age 0 | `LC_UUID` |
//! | ELF | the file's name (Android: the library's soname) | the GNU build ID's first 16 bytes read as a little-endian GUID, age 0 | the whole build ID |
//! | PE | the PDB's name in the CodeView record | the CodeView GUID and age | `TimeDateStamp` and `SizeOfImage` |
//!
//! Each ID is read from both files, never assumed. What writes the store's `.sym` files isn't here: §3 of
//! the M4 plan chose `dump_syms`, whose dependencies don't pass `cargo deny` yet (C1's As built note).
//! `cargo xtask package` runs this crate's binary on every package it makes. It never ships.

use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context as _, anyhow, bail, ensure};
use goblin::elf::Elf;
use goblin::mach::{Mach, MachO};
use goblin::pe::PE;

/// A binary's object format.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Format {
    MachO,
    Elf,
    Pe,
}

impl Format {
    pub fn name(self) -> &'static str {
        match self {
            Self::MachO => "mach-o",
            Self::Elf => "elf",
            Self::Pe => "pe",
        }
    }
}

/// A Breakpad debug ID: a GUID, in the byte order it prints in, and an age.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DebugId {
    pub guid: [u8; 16],
    pub age: u32,
}

impl DebugId {
    /// From a GUID as Windows lays it out in memory: the first three fields little-endian. That's the
    /// CodeView record's, and Breakpad reads an ELF build ID's first 16 bytes the same way.
    fn from_le_guid(raw: [u8; 16], age: u32) -> Self {
        let mut guid = raw;
        guid[0..4].reverse();
        guid[4..6].reverse();
        guid[6..8].reverse();
        Self { guid, age }
    }
}

/// 32 uppercase hex digits of GUID, then the age in hex: what a `.sym`'s `MODULE` line and the store's
/// path say.
impl fmt::Display for DebugId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for byte in self.guid {
            write!(f, "{byte:02X}")?;
        }
        write!(f, "{:X}", self.age)
    }
}

/// A shipped binary and its symbols, checked against each other.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Module {
    pub format: Format,
    /// `arm64` or `x86_64`.
    pub arch: &'static str,
    pub debug_file: String,
    pub debug_id: DebugId,
    pub code_id: String,
}

impl Module {
    /// Where a Breakpad store keeps this module's symbols, relative to the store.
    pub fn store_path(&self) -> PathBuf {
        let stem = self.debug_file.strip_suffix(".pdb").unwrap_or(&self.debug_file);
        Path::new(&self.debug_file).join(self.debug_id.to_string()).join(format!("{stem}.sym"))
    }

    pub fn to_json(&self) -> serde_json::Value {
        serde_json::json!({
            "format": self.format.name(),
            "arch": self.arch,
            "debug_file": self.debug_file,
            "debug_id": self.debug_id.to_string(),
            "code_id": self.code_id,
        })
    }
}

/// Checks `shipped`, the binary a package ships, against `symbols`, the debug file the same link left
/// (a `.dSYM` bundle, the unstripped ELF or the PDB): `shipped` is stripped, `symbols` holds debug
/// information, and both carry one ID. Returns the module they make.
pub fn check(shipped: &Path, symbols: &Path) -> anyhow::Result<Module> {
    let bytes = read(shipped)?;
    let binary = Binary::parse(&bytes, shipped)?;
    let name = file_name(shipped)?;
    let context = || format!("{} against {}", shipped.display(), symbols.display());
    let module = match binary {
        Binary::MachO(macho) => {
            let shipped_id = mach_uuid(&macho).with_context(|| format!("in {}", shipped.display()))?;
            ensure!(mach_is_stripped(&macho)?, "{} isn't stripped", shipped.display());
            let dwarf = dsym_dwarf(symbols)?;
            let dwarf_bytes = read(&dwarf)?;
            let Binary::MachO(debug) = Binary::parse(&dwarf_bytes, &dwarf)? else {
                bail!("{} isn't Mach-O", dwarf.display());
            };
            let debug_id = mach_uuid(&debug).with_context(|| format!("in {}", dwarf.display()))?;
            same_link(&hex(&shipped_id), &hex(&debug_id)).with_context(context)?;
            ensure!(mach_has_dwarf(&debug)?, "{} holds no DWARF line tables", dwarf.display());
            Module {
                format: Format::MachO,
                arch: mach_arch(macho.header.cputype)?,
                debug_file: name,
                debug_id: DebugId { guid: shipped_id, age: 0 },
                code_id: hex(&shipped_id).to_uppercase(),
            }
        }
        Binary::Elf(elf) => {
            let shipped_id = build_id(&elf, &bytes).with_context(|| format!("in {}", shipped.display()))?;
            let sections = elf_sections(&elf);
            ensure!(
                !sections.iter().any(|name| *name == ".symtab" || is_dwarf_section(name)),
                "{} isn't stripped: it has a symbol table or DWARF",
                shipped.display()
            );
            let debug_bytes = read(symbols)?;
            let Binary::Elf(debug) = Binary::parse(&debug_bytes, symbols)? else {
                bail!("{} isn't ELF", symbols.display());
            };
            let debug_id =
                build_id(&debug, &debug_bytes).with_context(|| format!("in {}", symbols.display()))?;
            same_link(&hex(&shipped_id), &hex(&debug_id)).with_context(context)?;
            let sections = elf_sections(&debug);
            for wanted in [".symtab", ".debug_info", ".debug_line"] {
                ensure!(sections.contains(&wanted), "{} has no {wanted}", symbols.display());
            }
            let mut first = [0; 16];
            first.copy_from_slice(&shipped_id[..16]);
            Module {
                format: Format::Elf,
                arch: elf_arch(elf.header.e_machine)?,
                debug_file: name,
                debug_id: DebugId::from_le_guid(first, 0),
                code_id: hex(&shipped_id),
            }
        }
        Binary::Pe(pe) => {
            let header = &pe.header.coff_header;
            ensure!(
                header.pointer_to_symbol_table == 0 && header.number_of_symbol_table == 0,
                "{} isn't stripped: it has a COFF symbol table",
                shipped.display()
            );
            let codeview = pe
                .debug_data
                .as_ref()
                .and_then(|data| data.codeview_pdb70_debug_info.as_ref())
                .with_context(|| format!("{} names no PDB (no CodeView record)", shipped.display()))?;
            let shipped_id = DebugId::from_le_guid(codeview.signature, codeview.age);
            let pdb_path = String::from_utf8_lossy(codeview.filename).trim_end_matches('\0').to_owned();
            let debug_file = pdb_path.rsplit(['\\', '/']).next().unwrap_or(&pdb_path).to_owned();
            ensure!(
                debug_file == file_name(symbols)?,
                "{} names its PDB {pdb_path:?}, not {}",
                shipped.display(),
                symbols.display()
            );
            let pdb_id = pdb_id(symbols)?;
            same_link(&shipped_id.to_string(), &pdb_id.to_string()).with_context(context)?;
            let size_of_image = pe
                .header
                .optional_header
                .map(|optional| optional.windows_fields.size_of_image)
                .with_context(|| format!("{} has no optional header", shipped.display()))?;
            Module {
                format: Format::Pe,
                arch: pe_arch(header.machine)?,
                debug_file,
                debug_id: shipped_id,
                code_id: format!("{:08X}{size_of_image:x}", header.time_date_stamp),
            }
        }
    };
    ensure!(module.debug_id.guid != [0; 16], "{} has a nil ID", shipped.display());
    Ok(module)
}

enum Binary<'a> {
    MachO(Box<MachO<'a>>),
    Elf(Box<Elf<'a>>),
    Pe(Box<PE<'a>>),
}

impl<'a> Binary<'a> {
    /// Tells the format by its magic number, then reads it.
    fn parse(bytes: &'a [u8], path: &Path) -> anyhow::Result<Self> {
        let context = || format!("can't read {}", path.display());
        Ok(match bytes.get(..4) {
            Some([0x7f, b'E', b'L', b'F']) => Self::Elf(Box::new(Elf::parse(bytes).with_context(context)?)),
            Some([b'M', b'Z', ..]) => Self::Pe(Box::new(PE::parse(bytes).with_context(context)?)),
            Some(
                [0xcf, 0xfa, 0xed, 0xfe]
                | [0xce, 0xfa, 0xed, 0xfe]
                | [0xca, 0xfe, 0xba, 0xbe]
                | [0xbe, 0xba, 0xfe, 0xca],
            ) => match Mach::parse(bytes).with_context(context)? {
                Mach::Binary(macho) => Self::MachO(Box::new(macho)),
                Mach::Fat(_) => {
                    bail!("{} is a universal binary: packages ship one architecture each", path.display())
                }
            },
            _ => bail!("{} isn't a Mach-O, ELF or PE binary", path.display()),
        })
    }
}

/// Fails unless both files carry one ID.
fn same_link(shipped: &str, symbols: &str) -> anyhow::Result<()> {
    ensure!(
        shipped == symbols,
        "the symbols come from another link: the binary's ID is {shipped}, the symbols' {symbols}"
    );
    Ok(())
}

/// Lowercase hex, as `readelf` and `dwarfdump` print IDs.
fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn read(path: &Path) -> anyhow::Result<Vec<u8>> {
    fs::read(path).with_context(|| format!("failed to read {}", path.display()))
}

fn file_name(path: &Path) -> anyhow::Result<String> {
    let name = path.file_name().with_context(|| format!("{} names no file", path.display()))?;
    name.to_str().map(str::to_owned).with_context(|| format!("{} isn't UTF-8", path.display()))
}

/// The one DWARF file inside a `.dSYM` bundle.
fn dsym_dwarf(dsym: &Path) -> anyhow::Result<PathBuf> {
    let dir = dsym.join("Contents/Resources/DWARF");
    let entries = fs::read_dir(&dir).with_context(|| format!("{} isn't a dSYM", dsym.display()))?;
    let mut files = Vec::new();
    for entry in entries {
        files.push(entry.with_context(|| format!("failed to list {}", dir.display()))?.path());
    }
    match files.as_slice() {
        [file] => Ok(file.clone()),
        _ => Err(anyhow!("{} holds {} files, not one", dir.display(), files.len())),
    }
}

fn mach_uuid(macho: &MachO<'_>) -> anyhow::Result<[u8; 16]> {
    macho
        .load_commands
        .iter()
        .find_map(|command| match &command.command {
            goblin::mach::load_command::CommandVariant::Uuid(uuid) => Some(uuid.uuid),
            _ => None,
        })
        .context("no LC_UUID")
}

/// The sections of `macho` as `(segment, section)` names.
fn mach_sections(macho: &MachO<'_>) -> anyhow::Result<Vec<(String, String)>> {
    let mut names = Vec::new();
    for segment in &macho.segments {
        for (section, _) in segment.sections()? {
            names.push((section.segname()?.to_owned(), section.name()?.to_owned()));
        }
    }
    Ok(names)
}

/// No DWARF, and a symbol table that defines nothing local and holds no debug map (STABS), which is
/// what `strip` leaves of an executable: what dyld binds and the few symbols it exports.
fn mach_is_stripped(macho: &MachO<'_>) -> anyhow::Result<bool> {
    if mach_sections(macho)?.iter().any(|(segment, _)| segment == "__DWARF") {
        return Ok(false);
    }
    for symbol in macho.symbols() {
        let (_, nlist) = symbol?;
        if nlist.is_stab() || (!nlist.is_global() && !nlist.is_undefined()) {
            return Ok(false);
        }
    }
    Ok(true)
}

fn mach_has_dwarf(macho: &MachO<'_>) -> anyhow::Result<bool> {
    let sections = mach_sections(macho)?;
    let has = |name: &str| sections.iter().any(|(segment, section)| segment == "__DWARF" && section == name);
    Ok(has("__debug_info") && has("__debug_line"))
}

fn mach_arch(cputype: u32) -> anyhow::Result<&'static str> {
    use goblin::mach::cputype::{CPU_TYPE_ARM64, CPU_TYPE_X86_64};
    match cputype {
        CPU_TYPE_ARM64 => Ok("arm64"),
        CPU_TYPE_X86_64 => Ok("x86_64"),
        other => bail!("unexpected Mach-O CPU type {other:#x}"),
    }
}

/// The GNU build ID note: at least 16 bytes, since a debug ID takes 16 (`--build-id=sha1` gives 20; lld's
/// default `fast` gives 8, which Breakpad pads with zeros).
fn build_id(elf: &Elf<'_>, bytes: &[u8]) -> anyhow::Result<Vec<u8>> {
    const NT_GNU_BUILD_ID: u32 = 3;
    let notes = elf.iter_note_sections(bytes, None).context("no note sections, so no build ID")?;
    for note in notes {
        let note = note.context("a note doesn't read")?;
        if note.n_type == NT_GNU_BUILD_ID && note.name == "GNU" {
            ensure!(note.desc.len() >= 16, "the build ID has {} bytes, not 16 or more", note.desc.len());
            return Ok(note.desc.to_vec());
        }
    }
    bail!("no GNU build ID: link with `-Wl,--build-id=sha1`")
}

fn elf_sections<'a>(elf: &'a Elf<'_>) -> Vec<&'a str> {
    elf.section_headers.iter().filter_map(|header| elf.shdr_strtab.get_at(header.sh_name)).collect()
}

fn is_dwarf_section(name: &str) -> bool {
    name.starts_with(".debug_") || name.starts_with(".zdebug_")
}

fn elf_arch(machine: u16) -> anyhow::Result<&'static str> {
    use goblin::elf::header::{EM_AARCH64, EM_X86_64};
    match machine {
        EM_AARCH64 => Ok("arm64"),
        EM_X86_64 => Ok("x86_64"),
        other => bail!("unexpected ELF machine {other}"),
    }
}

/// A PDB's GUID, and the age its debug information stream records, which is the one the image's
/// CodeView record carries.
fn pdb_id(path: &Path) -> anyhow::Result<DebugId> {
    let file = fs::File::open(path).with_context(|| format!("failed to open {}", path.display()))?;
    let mut pdb = pdb::PDB::open(file).with_context(|| format!("{} isn't a PDB", path.display()))?;
    let info =
        pdb.pdb_information().with_context(|| format!("{} has no information stream", path.display()))?;
    let debug = pdb.debug_information().with_context(|| format!("{} has no DBI stream", path.display()))?;
    let age = debug.age().unwrap_or(info.age);
    let mut modules = debug.modules().with_context(|| format!("{}'s modules don't read", path.display()))?;
    ensure!(
        pdb::FallibleIterator::next(&mut modules)?.is_some(),
        "{} holds no modules' debug information",
        path.display()
    );
    let (data1, data2, data3, data4) = info.guid.as_fields();
    let mut guid = [0; 16];
    guid[0..4].copy_from_slice(&data1.to_be_bytes());
    guid[4..6].copy_from_slice(&data2.to_be_bytes());
    guid[6..8].copy_from_slice(&data3.to_be_bytes());
    guid[8..].copy_from_slice(data4);
    Ok(DebugId { guid, age })
}

fn pe_arch(machine: u16) -> anyhow::Result<&'static str> {
    use goblin::pe::header::{COFF_MACHINE_ARM64, COFF_MACHINE_X86_64};
    match machine {
        COFF_MACHINE_X86_64 => Ok("x86_64"),
        COFF_MACHINE_ARM64 => Ok("arm64"),
        other => bail!("unexpected PE machine {other:#x}"),
    }
}

#[cfg(test)]
mod tests;
