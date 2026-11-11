//! Breakpad `.sym` files, written from the symbols a link kept (M4 C2): the text `minidump-processor` and
//! every Breakpad reader looks addresses up in.
//!
//! `dwarf` reads a dSYM's or an unstripped ELF's DWARF with `gimli`, and `pdb` a PDB with `pdb`, into
//! the records written here, as Mozilla's `dump_syms` writes them (which can't be used itself: its
//! dependencies fail `cargo deny`, C1's As built note):
//!
//! - `MODULE` and `INFO CODE_ID`, from the module [`crate::check`] made;
//! - `FILE`, every source file a line names, as the build recorded it (a shipping build's are remapped,
//!   `/sapphire/…` and `/cargo/…`);
//! - `INLINE_ORIGIN`, every function inlined somewhere;
//! - `FUNC`, each function's range and name, demangled (Rust's without its hash), then its `INLINE` records,
//!   each inlined call's ranges with the line it was called from, and its line records, which name the
//!   innermost code at each address, so a crash inside an inlined call names it and then its callers;
//! - `PUBLIC`, the symbol table's names for code no function covers.
//!
//! No `STACK CFI` or `STACK WIN`: unwinding past the crash's frame uses frame pointers, which every Apple
//! arm64 frame keeps, and stack scanning elsewhere (the M4 plan's C2 row says what that costs). A PDB's
//! inlined calls aren't read either: Windows names the function an inlined call was inlined into.

mod dwarf;
mod pdb;

use std::borrow::Cow;
use std::collections::{BTreeMap, HashMap};
use std::io::{self, Write};
use std::ops::Range;
use std::path::Path;

use anyhow::{Context as _, ensure};

use crate::{DebugId, Format, Module};

/// Writes the `.sym` text for `module` from `symbols`, its dSYM, unstripped ELF or PDB, to `out`. Fails
/// unless `symbols` carries `module`'s debug ID.
pub fn write(module: &Module, symbols: &Path, out: &mut impl Write) -> anyhow::Result<()> {
    let mut dump = Dump::default();
    match module.format {
        Format::MachO => {
            let file = crate::dsym_dwarf(symbols)?;
            dwarf::read(&crate::read(&file)?, module, &mut dump)
                .with_context(|| format!("in {}", file.display()))?;
        }
        Format::Elf => {
            dwarf::read(&crate::read(symbols)?, module, &mut dump)
                .with_context(|| format!("in {}", symbols.display()))?;
        }
        Format::Pe => {
            pdb::read(symbols, module, &mut dump).with_context(|| format!("in {}", symbols.display()))?
        }
    }
    ensure!(!dump.functions.is_empty(), "{} names no functions", symbols.display());
    dump.write(module, out).with_context(|| format!("failed to write the symbols of {}", module.debug_file))
}

/// Fails unless the symbols' ID is the module's.
fn same_id(found: DebugId, module: &Module) -> anyhow::Result<()> {
    ensure!(
        found == module.debug_id,
        "the symbols' debug ID is {found}, not {}: they're from another link",
        module.debug_id
    );
    Ok(())
}

/// What a `.sym` holds, gathered before it's written: files and inline origins are numbered as they're met.
#[derive(Default)]
struct Dump {
    files: Numbered,
    origins: Numbered,
    /// By start address, relative to the module's load address.
    functions: BTreeMap<u64, Func>,
    publics: BTreeMap<u64, String>,
}

/// Strings numbered in the order they're first seen.
#[derive(Default)]
struct Numbered {
    ids: HashMap<String, u32>,
    names: Vec<String>,
}

impl Numbered {
    fn id(&mut self, name: Cow<'_, str>) -> u32 {
        if let Some(&id) = self.ids.get(name.as_ref()) {
            return id;
        }
        let id = u32::try_from(self.names.len()).expect("fewer than 4 billion names");
        self.names.push(name.clone().into_owned());
        self.ids.insert(name.into_owned(), id);
        id
    }
}

struct Func {
    size: u64,
    name: String,
    /// Sorted by address; each the innermost code at its addresses.
    lines: Vec<Line>,
    inlines: Vec<Inline>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Line {
    address: u64,
    size: u64,
    line: u64,
    file: u32,
}

/// A call inlined into a function: how deep (0 for a call in the function's own code), the line it was
/// called from, what was called, and the addresses its code took.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Inline {
    depth: u32,
    call_line: u64,
    call_file: u32,
    origin: u32,
    ranges: Vec<Range<u64>>,
}

impl Dump {
    /// Adds a function at `range`, with those of `lines` (sorted) and `inlines` that fall in it, unless one
    /// already starts there.
    fn add_function(&mut self, range: Range<u64>, name: &str, lines: &[Line], inlines: &[Inline]) {
        if range.is_empty() || self.functions.contains_key(&range.start) {
            return;
        }
        let first = lines.partition_point(|line| line.address + line.size <= range.start);
        let lines = lines[first..]
            .iter()
            .take_while(|line| line.address < range.end)
            .filter_map(|line| {
                let start = line.address.max(range.start);
                let end = (line.address + line.size).min(range.end);
                (start < end).then_some(Line { address: start, size: end - start, ..*line })
            })
            .collect();
        let inlines = inlines
            .iter()
            .filter_map(|inline| {
                let ranges: Vec<Range<u64>> = inline
                    .ranges
                    .iter()
                    .map(|part| part.start.max(range.start)..part.end.min(range.end))
                    .filter(|part| !part.is_empty())
                    .collect();
                (!ranges.is_empty()).then(|| Inline { ranges, ..inline.clone() })
            })
            .collect();
        self.functions.insert(
            range.start,
            Func { size: range.end - range.start, name: name.to_owned(), lines, inlines },
        );
    }

    /// Adds a function for each of `symbols` (a symbol table's sized functions: their code and names) that
    /// no function covers and that `lines` (sorted) has lines in. LLVM's line tables alone give a function
    /// no entry of its own, except on Apple platforms, unless something was inlined into it, so on ELF a
    /// leaf such as `forced_panic` would read as a `PUBLIC` with no line.
    fn add_symbol_functions(&mut self, symbols: &[(Range<u64>, String)], lines: &[Line]) {
        for (range, name) in symbols {
            let covered = self
                .functions
                .range(..range.end)
                .next_back()
                .is_some_and(|(&start, func)| start + func.size > range.start);
            let first = lines.partition_point(|line| line.address + line.size <= range.start);
            let has_lines = lines.get(first).is_some_and(|line| line.address < range.end);
            if !covered && has_lines {
                self.add_function(range.clone(), &demangle(name), lines, &[]);
            }
        }
    }

    fn add_public(&mut self, address: u64, name: &str) {
        if !name.is_empty() {
            self.publics.entry(address).or_insert_with(|| demangle(name));
        }
    }

    fn write(mut self, module: &Module, out: &mut impl Write) -> io::Result<()> {
        let os = match module.format {
            Format::MachO => "mac",
            Format::Elf => "Linux",
            Format::Pe => "windows",
        };
        writeln!(out, "MODULE {os} {} {} {}", module.arch, module.debug_id, module.debug_file)?;
        if !module.code_id.is_empty() {
            writeln!(out, "INFO CODE_ID {}", module.code_id)?;
        }
        for (id, name) in self.files.names.iter().enumerate() {
            writeln!(out, "FILE {id} {}", one_line(name))?;
        }
        for (id, name) in self.origins.names.iter().enumerate() {
            writeln!(out, "INLINE_ORIGIN {id} {}", one_line(name))?;
        }
        // DWARF with line tables only names a function without its path (`forced_fault`): the symbol table's
        // name at the same address, demangled, says the rest (`sapphire_crash::forced::forced_fault`).
        for (start, func) in &mut self.functions {
            if let Some(public) = self.publics.get(start)
                && public.len() > func.name.len()
                && public.ends_with(&format!("::{}", func.name))
            {
                func.name.clone_from(public);
            }
        }
        // A public symbol inside a function names the same code worse.
        let mut covered = self.functions.iter().map(|(&start, func)| start..start + func.size).peekable();
        self.publics.retain(|&address, _| {
            while covered.next_if(|range| range.end <= address).is_some() {}
            covered.peek().is_none_or(|range| !range.contains(&address))
        });
        // Functions may overlap (identical code folded, or a DWARF error): each ends where the next starts.
        let starts: Vec<u64> = self.functions.keys().copied().collect();
        for (index, (&start, func)) in self.functions.iter_mut().enumerate() {
            if let Some(&next) = starts.get(index + 1) {
                func.size = func.size.min(next - start);
            }
        }
        for (&start, func) in &self.functions {
            let end = start + func.size;
            writeln!(out, "FUNC {start:x} {:x} 0 {}", func.size, one_line(&func.name))?;
            let mut inlines: Vec<&Inline> = func.inlines.iter().collect();
            inlines.sort_by_key(|inline| (inline.ranges.iter().map(|range| range.start).min(), inline.depth));
            for inline in inlines {
                let ranges: Vec<Range<u64>> =
                    merged(&inline.ranges).into_iter().map(|range| range.start..range.end.min(end)).collect();
                if ranges.iter().all(Range::is_empty) {
                    continue;
                }
                write!(
                    out,
                    "INLINE {} {} {} {}",
                    inline.depth, inline.call_line, inline.call_file, inline.origin
                )?;
                for range in ranges.iter().filter(|range| !range.is_empty()) {
                    write!(out, " {:x} {:x}", range.start, range.end - range.start)?;
                }
                writeln!(out)?;
            }
            for line in &func.lines {
                let size = line.size.min(end.saturating_sub(line.address));
                if size > 0 {
                    writeln!(out, "{:x} {size:x} {} {}", line.address, line.line, line.file)?;
                }
            }
        }
        for (address, name) in &self.publics {
            writeln!(out, "PUBLIC {address:x} 0 {}", one_line(name))?;
        }
        Ok(())
    }
}

/// A function's name as a crash should read it: Rust's without its hash, C++'s without its parameters, and
/// anything else as it is.
fn demangle(name: &str) -> String {
    if let Ok(demangled) = rustc_demangle::try_demangle(name) {
        return format!("{demangled:#}");
    }
    if name.starts_with("_Z")
        && let Ok(symbol) = cpp_demangle::Symbol::new(name.as_bytes())
        && let Ok(demangled) =
            symbol.demangle(&cpp_demangle::DemangleOptions::new().no_params().no_return_type())
    {
        return demangled;
    }
    name.to_owned()
}

/// Ranges sorted, with touching ones joined.
fn merged(ranges: &[Range<u64>]) -> Vec<Range<u64>> {
    let mut sorted = ranges.to_vec();
    sorted.sort_by_key(|range| range.start);
    let mut merged: Vec<Range<u64>> = Vec::with_capacity(sorted.len());
    for range in sorted {
        match merged.last_mut() {
            Some(last) if last.end >= range.start => last.end = last.end.max(range.end),
            _ => merged.push(range),
        }
    }
    merged
}

/// `part` under `base`, as a build records a source file's path: unless `part` is already absolute. A
/// Windows path keeps its backslashes.
fn joined(base: &str, part: &str) -> String {
    if base.is_empty() || is_absolute(part) {
        return part.to_owned();
    }
    if part.is_empty() {
        return base.to_owned();
    }
    let separator = if base.contains('\\') && !base.contains('/') { '\\' } else { '/' };
    format!("{}{separator}{part}", base.trim_end_matches(['/', '\\']))
}

/// A Unix path, a Windows drive's, or a UNC one.
fn is_absolute(path: &str) -> bool {
    let bytes = path.as_bytes();
    path.starts_with('/')
        || path.starts_with("\\\\")
        || (bytes.len() > 2
            && bytes[0].is_ascii_alphabetic()
            && bytes[1] == b':'
            && matches!(bytes[2], b'\\' | b'/'))
}

/// A `.sym` record ends at its line.
fn one_line(text: &str) -> Cow<'_, str> {
    if text.contains(['\n', '\r']) {
        Cow::Owned(text.replace(['\n', '\r'], " "))
    } else {
        Cow::Borrowed(text)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn source_paths_join_as_the_build_recorded_them() {
        assert!(is_absolute("/sapphire/crates/a.rs"));
        assert!(is_absolute(r"C:\work\a.rs"));
        assert!(is_absolute(r"\\server\share\a.rs"));
        assert!(!is_absolute("crates/a.rs"));
        assert_eq!(joined("/sapphire", "crates/a.rs"), "/sapphire/crates/a.rs");
        assert_eq!(joined("/sapphire/", "/cargo/registry/x.rs"), "/cargo/registry/x.rs");
        assert_eq!(joined(r"C:\work", r"src\a.rs"), r"C:\work\src\a.rs");
        assert_eq!(merged(&[4..6, 0..2, 2..3, 5..9]), [0..3, 4..9]);
    }

    #[test]
    fn names_read_as_a_crash_should() {
        assert_eq!(
            demangle("_ZN14sapphire_crash6forced12forced_fault17h0123456789abcdefE"),
            "sapphire_crash::forced::forced_fault"
        );
        assert_eq!(demangle("_ZN4Luau8Compiler7compileEPKcm"), "Luau::Compiler::compile");
        assert_eq!(demangle("SDL_Init"), "SDL_Init");
    }

    /// A function keeps only its own lines and inlined calls, clipped to its range, and the next function's
    /// start ends it.
    #[test]
    fn functions_are_written_with_what_falls_in_them() {
        let mut dump = Dump::default();
        let file = dump.files.id(Cow::Borrowed("/sapphire/a.rs"));
        let origin = dump.origins.id(Cow::Borrowed("a::inner"));
        let lines = [
            Line { address: 0x0, size: 0x10, line: 1, file },
            Line { address: 0x10, size: 0x10, line: 7, file },
            Line { address: 0x20, size: 0x20, line: 9, file },
        ];
        let inlines = [Inline {
            depth: 0,
            call_line: 2,
            call_file: file,
            origin,
            ranges: std::iter::once(0x18..0x30).collect(),
        }];
        dump.add_function(0x10..0x28, "a::outer", &lines, &inlines);
        dump.add_function(0x24..0x40, "a::next", &[], &[]);
        // Named as line tables alone name it, then by its symbol.
        dump.add_function(0x50..0x58, "after", &[], &[]);
        dump.add_public(0x10, "_ZN1a5outer17h0123456789abcdefE");
        dump.add_public(0x50, "_ZN1a5after17h0123456789abcdefE");
        dump.add_public(0x60, "_ZN1a4last17h0123456789abcdefE");
        let module = Module {
            format: Format::Elf,
            arch: "arm64",
            debug_file: "liba.so".into(),
            debug_id: DebugId { guid: [0xab; 16], age: 0 },
            code_id: "abab".into(),
        };
        let mut out = Vec::new();
        dump.write(&module, &mut out).unwrap();
        let text = String::from_utf8(out).unwrap();
        let expected = "\
MODULE Linux arm64 ABABABABABABABABABABABABABABABAB0 liba.so
INFO CODE_ID abab
FILE 0 /sapphire/a.rs
INLINE_ORIGIN 0 a::inner
FUNC 10 14 0 a::outer
INLINE 0 2 0 0 18 c
10 10 7 0
20 4 9 0
FUNC 24 1c 0 a::next
FUNC 50 8 0 a::after
PUBLIC 60 0 a::last
";
        assert_eq!(text, expected);
    }

    /// A function the DWARF has no entry for, as line tables alone leave a leaf on ELF, is read from the
    /// symbol table with its lines; one the DWARF covers, or with no lines, isn't.
    #[test]
    fn functions_with_lines_but_no_entry_come_from_the_symbol_table() {
        let mut dump = Dump::default();
        let file = dump.files.id(Cow::Borrowed("/sapphire/a.rs"));
        let lines = [
            Line { address: 0x10, size: 0x8, line: 3, file },
            Line { address: 0x40, size: 0x10, line: 85, file },
            Line { address: 0x50, size: 0x5, line: 86, file },
        ];
        dump.add_function(0x10..0x20, "a::described", &lines, &[]);
        let symbols = [
            (0x18..0x30, "_ZN1a7overlap17h0123456789abcdefE".to_owned()),
            (0x40..0x55, "_ZN1a12forced_panic17h0123456789abcdefE".to_owned()),
            (0x60..0x70, "_ZN1a7nolines17h0123456789abcdefE".to_owned()),
        ];
        dump.add_symbol_functions(&symbols, &lines);
        let names: Vec<&str> = dump.functions.values().map(|func| func.name.as_str()).collect();
        assert_eq!(names, ["a::described", "a::forced_panic"]);
        let leaf = &dump.functions[&0x40];
        assert_eq!(leaf.size, 0x15);
        assert_eq!(leaf.lines.iter().map(|line| line.line).collect::<Vec<_>>(), [85, 86]);
    }
}
