//! A dSYM's or an unstripped ELF's DWARF, read with `gimli`: each unit's line table, and its tree of
//! functions and the calls inlined into them.

use std::cell::RefCell;
use std::collections::HashMap;
use std::ops::Range;
use std::rc::Rc;

use anyhow::{Context as _, bail, ensure};
use gimli::{AttributeValue, DebugInfoOffset, EndianSlice, RunTimeEndian, SectionId, UnitHeader, constants};
use goblin::elf::Elf;
use goblin::mach::{Mach, MachO};

use super::{Dump, Inline, Line, demangle, joined, same_id};
use crate::{DebugId, Format, Module};

type Reader<'a> = EndianSlice<'a, RunTimeEndian>;
type Unit<'a> = gimli::Unit<Reader<'a>>;

/// How many references (`DW_AT_specification`, `DW_AT_abstract_origin`) a name is followed through.
const MAX_REFERENCES: u32 = 16;

/// Reads `bytes`, a Mach-O DWARF file or an unstripped ELF carrying `module`'s ID, into `dump`.
pub(super) fn read(bytes: &[u8], module: &Module, dump: &mut Dump) -> anyhow::Result<()> {
    let object = Object::parse(bytes, module.format)?;
    same_id(object.id, module)?;
    for (address, name) in &object.publics {
        dump.add_public(*address, name);
    }
    let endian = RunTimeEndian::Little;
    let dwarf = gimli::Dwarf::load(|id: SectionId| -> Result<Reader<'_>, gimli::Error> {
        Ok(EndianSlice::new(object.section(id), endian))
    })?;
    let mut headers = Vec::new();
    let mut iter = dwarf.units();
    while let Some(header) = iter.next()? {
        headers.push(header);
    }
    ensure!(!headers.is_empty(), "no DWARF units");
    let names =
        Names { dwarf: &dwarf, headers: &headers, units: RefCell::default(), cache: RefCell::default() };
    let mut uncovered = Vec::new();
    for header in &headers {
        let unit = dwarf.unit(*header)?;
        uncovered.extend(read_unit(&dwarf, &unit, &names, object.base, dump)?);
    }
    uncovered.sort_by_key(|line| line.address);
    dump.add_symbol_functions(&object.sized, &uncovered);
    Ok(())
}

/// What a symbol file holds for this: its sections, the address the module loads at, its ID and its symbol
/// table's code: every function's address, and, where the table says (ELF), each one's extent.
struct Object<'a> {
    sections: HashMap<String, &'a [u8]>,
    format: Format,
    base: u64,
    id: DebugId,
    publics: Vec<(u64, String)>,
    sized: Vec<(Range<u64>, String)>,
}

impl<'a> Object<'a> {
    fn parse(bytes: &'a [u8], format: Format) -> anyhow::Result<Self> {
        match format {
            Format::MachO => {
                let Mach::Binary(macho) = Mach::parse(bytes)? else { bail!("a universal binary") };
                Self::macho(&macho)
            }
            Format::Elf => Self::elf(&Elf::parse(bytes)?, bytes),
            Format::Pe => bail!("a PE's symbols are in its PDB"),
        }
    }

    fn macho(macho: &MachO<'a>) -> anyhow::Result<Self> {
        let mut sections = HashMap::new();
        let mut base = None;
        for segment in &macho.segments {
            if segment.name()? == "__TEXT" {
                base = Some(segment.vmaddr);
            }
            for (section, data) in segment.sections()? {
                sections.insert(section.name()?.to_owned(), data);
            }
        }
        let base = base.context("no __TEXT segment")?;
        let mut publics = Vec::new();
        for symbol in macho.symbols() {
            let (name, nlist) = symbol?;
            // Defined in a section, and not a debugger's (STABS) entry.
            if !nlist.is_stab() && nlist.get_type() == goblin::mach::symbols::N_SECT && nlist.n_value >= base
            {
                publics.push((nlist.n_value - base, name.strip_prefix('_').unwrap_or(name).to_owned()));
            }
        }
        let id = DebugId { guid: crate::mach_uuid(macho)?, age: 0 };
        // A Mach-O symbol has no size, and LLVM gives every function on Apple platforms an entry of its own.
        Ok(Self { sections, format: Format::MachO, base, id, publics, sized: Vec::new() })
    }

    fn elf(elf: &Elf<'a>, bytes: &'a [u8]) -> anyhow::Result<Self> {
        use goblin::elf::section_header::{SHF_COMPRESSED, SHT_NOBITS};
        use goblin::elf::sym::STT_FUNC;
        ensure!(elf.little_endian, "a big-endian ELF");
        let mut sections = HashMap::new();
        for header in &elf.section_headers {
            let Some(name) = elf.shdr_strtab.get_at(header.sh_name) else { continue };
            if !crate::is_dwarf_section(name) || header.sh_type == SHT_NOBITS {
                continue;
            }
            ensure!(header.sh_flags & u64::from(SHF_COMPRESSED) == 0, "{name} is compressed");
            let range =
                usize::try_from(header.sh_offset)?..usize::try_from(header.sh_offset + header.sh_size)?;
            sections.insert(
                name.to_owned(),
                bytes.get(range).with_context(|| format!("{name} runs past the file"))?,
            );
        }
        // Breakpad's addresses are relative to the lowest loaded address, 0 for a library or a PIE.
        let base = elf
            .program_headers
            .iter()
            .filter(|header| header.p_type == goblin::elf::program_header::PT_LOAD)
            .map(|header| header.p_vaddr)
            .min()
            .context("no loaded segment")?;
        let mut publics = Vec::new();
        let mut sized = Vec::new();
        for symbol in elf.syms.iter() {
            if symbol.st_type() == STT_FUNC
                && symbol.st_shndx != 0
                && symbol.st_value >= base
                && symbol.st_value != 0
                && let Some(name) = elf.strtab.get_at(symbol.st_name)
            {
                let start = symbol.st_value - base;
                publics.push((start, name.to_owned()));
                if let Some(end) = start.checked_add(symbol.st_size).filter(|&end| end > start) {
                    sized.push((start..end, name.to_owned()));
                }
            }
        }
        let build_id = crate::build_id(elf, bytes)?;
        let mut first = [0; 16];
        first.copy_from_slice(&build_id[..16]);
        let id = DebugId::from_le_guid(first, 0);
        Ok(Self { sections, format: Format::Elf, base, id, publics, sized })
    }

    /// A DWARF section's bytes, empty if it's missing. Mach-O names `.debug_info` `__debug_info`, cut to 16
    /// characters.
    fn section(&self, id: SectionId) -> &'a [u8] {
        let name = match self.format {
            Format::MachO => {
                let name = format!("__{}", &id.name()[1..]);
                name.chars().take(16).collect()
            }
            _ => id.name().to_owned(),
        };
        self.sections.get(&name).copied().unwrap_or_default()
    }
}

/// Function names, found through the references DWARF makes between entries, in any unit.
struct Names<'d, 'a> {
    dwarf: &'d gimli::Dwarf<Reader<'a>>,
    headers: &'d [UnitHeader<Reader<'a>>],
    /// Units parsed to follow a reference into them, by their offset.
    units: RefCell<HashMap<usize, Rc<Unit<'a>>>>,
    /// By the entry's offset in `.debug_info`.
    cache: RefCell<HashMap<usize, Option<Rc<str>>>>,
}

impl<'a> Names<'_, 'a> {
    /// The name of the function `entry` of `unit` is, or describes: its linkage name demangled, else that of
    /// the entry it refers to, else its plain name.
    fn of(
        &self,
        unit: &Unit<'a>,
        entry: &gimli::DebuggingInformationEntry<Reader<'a>>,
        depth: u32,
    ) -> anyhow::Result<Option<Rc<str>>> {
        let key = entry.offset().to_debug_info_offset(&unit.header).map(|offset| offset.0);
        if let Some(key) = key
            && let Some(name) = self.cache.borrow().get(&key)
        {
            return Ok(name.clone());
        }
        let name = self.find(unit, entry, depth)?;
        if let Some(key) = key {
            self.cache.borrow_mut().insert(key, name.clone());
        }
        Ok(name)
    }

    fn find(
        &self,
        unit: &Unit<'a>,
        entry: &gimli::DebuggingInformationEntry<Reader<'a>>,
        depth: u32,
    ) -> anyhow::Result<Option<Rc<str>>> {
        let string = |value| -> anyhow::Result<String> {
            Ok(self.dwarf.attr_string(unit, value)?.to_string_lossy().into_owned())
        };
        for name in [constants::DW_AT_linkage_name, constants::DW_AT_MIPS_linkage_name] {
            if let Some(value) = entry.attr_value(name) {
                return Ok(Some(demangle(&string(value)?).into()));
            }
        }
        if depth < MAX_REFERENCES {
            for reference in [constants::DW_AT_abstract_origin, constants::DW_AT_specification] {
                let found = match entry.attr_value(reference) {
                    Some(AttributeValue::UnitRef(offset)) => {
                        self.of(unit, &unit.entry(offset)?, depth + 1)?
                    }
                    Some(AttributeValue::DebugInfoRef(offset)) => self.elsewhere(offset, depth + 1)?,
                    _ => None,
                };
                if found.is_some() {
                    return Ok(found);
                }
            }
        }
        entry.attr_value(constants::DW_AT_name).map(|value| string(value).map(Rc::from)).transpose()
    }

    /// The name of the entry at `offset`, in whichever unit holds it.
    fn elsewhere(&self, offset: DebugInfoOffset, depth: u32) -> anyhow::Result<Option<Rc<str>>> {
        let start = |header: &UnitHeader<Reader<'a>>| header.debug_info_offset().map_or(0, |start| start.0);
        let index = self.headers.partition_point(|header| start(header) <= offset.0);
        let Some(header) = index.checked_sub(1).map(|index| self.headers[index]) else { return Ok(None) };
        let Some(unit_offset) = offset.to_unit_offset(&header) else { return Ok(None) };
        let unit = {
            let mut units = self.units.borrow_mut();
            match units.get(&start(&header)) {
                Some(unit) => unit.clone(),
                None => {
                    let unit = Rc::new(self.dwarf.unit(header)?);
                    units.insert(start(&header), unit.clone());
                    unit
                }
            }
        };
        self.of(&unit, &unit.entry(unit_offset)?, depth)
    }
}

/// A function as its unit's tree describes it, before its lines are found.
struct Pending {
    name: Rc<str>,
    ranges: Vec<Range<u64>>,
    inlines: Vec<Inline>,
}

/// Adds `unit`'s functions to `dump`, and returns its lines that none of them covers: the code of
/// functions its tree has no entry for.
fn read_unit<'a>(
    dwarf: &gimli::Dwarf<Reader<'a>>,
    unit: &Unit<'a>,
    names: &Names<'_, 'a>,
    base: u64,
    dump: &mut Dump,
) -> anyhow::Result<Vec<Line>> {
    let mut files = UnitFiles { ids: HashMap::new() };
    let lines = unit_lines(dwarf, unit, base, &mut files, dump)?;
    let mut functions = Vec::new();
    let mut tree = unit.entries_tree(None)?;
    let mut walk =
        Walk { dwarf, unit, names, base, files: &mut files, dump: &mut *dump, functions: &mut functions };
    walk.node(tree.root()?, None, 0)?;
    let mut covered = Vec::new();
    for function in functions {
        for range in &function.ranges {
            dump.add_function(range.clone(), &function.name, &lines, &function.inlines);
            covered.push(range.clone());
        }
    }
    let covered = super::merged(&covered);
    let mut ranges = covered.iter().peekable();
    Ok(lines
        .into_iter()
        .filter(|line| {
            while ranges.next_if(|range| range.end <= line.address).is_some() {}
            ranges
                .peek()
                .is_none_or(|range| range.start > line.address || range.end < line.address + line.size)
        })
        .collect())
}

/// A unit's files by their index in its line table, numbered in the dump.
struct UnitFiles {
    ids: HashMap<u64, u32>,
}

impl UnitFiles {
    fn id<'a>(
        &mut self,
        dwarf: &gimli::Dwarf<Reader<'a>>,
        unit: &Unit<'a>,
        index: u64,
        dump: &mut Dump,
    ) -> anyhow::Result<u32> {
        if let Some(&id) = self.ids.get(&index) {
            return Ok(id);
        }
        let path = file_path(dwarf, unit, index)?.unwrap_or_else(|| "<unknown>".to_owned());
        let id = dump.files.id(path.into());
        self.ids.insert(index, id);
        Ok(id)
    }
}

/// File `index` of `unit`'s line table: its directory and name under the unit's compilation directory, as
/// the build recorded them.
fn file_path<'a>(
    dwarf: &gimli::Dwarf<Reader<'a>>,
    unit: &Unit<'a>,
    index: u64,
) -> anyhow::Result<Option<String>> {
    let Some(program) = &unit.line_program else { return Ok(None) };
    let header = program.header();
    let Some(file) = header.file(index) else { return Ok(None) };
    let string = |value| -> anyhow::Result<String> {
        Ok(dwarf.attr_string(unit, value)?.to_string_lossy().into_owned())
    };
    let name = string(file.path_name())?;
    let dir = file.directory(header).map(string).transpose()?.unwrap_or_default();
    let comp_dir = unit.comp_dir.map(|dir| dir.to_string_lossy().into_owned()).unwrap_or_default();
    Ok(Some(joined(&joined(&comp_dir, &dir), &name)))
}

/// `unit`'s line table as the innermost code at each address, relative to `base`, sorted. A sequence the
/// linker dropped (its code discarded, left at address 0) isn't.
fn unit_lines<'a>(
    dwarf: &gimli::Dwarf<Reader<'a>>,
    unit: &Unit<'a>,
    base: u64,
    files: &mut UnitFiles,
    dump: &mut Dump,
) -> anyhow::Result<Vec<Line>> {
    let Some(program) = unit.line_program.clone() else { return Ok(Vec::new()) };
    let mut lines = Vec::new();
    let mut sequence: Vec<(u64, u64, u64)> = Vec::new();
    let mut rows = program.rows();
    while let Some((_, row)) = rows.next_row()? {
        if !row.end_sequence() {
            sequence.push((row.address(), row.line().map_or(0, |line| line.get()), row.file_index()));
            continue;
        }
        let end = row.address();
        let kept = sequence.first().is_some_and(|&(start, ..)| start != 0 && start >= base);
        if kept {
            for (index, &(address, line, file)) in sequence.iter().enumerate() {
                let next = sequence.get(index + 1).map_or(end, |&(next, ..)| next);
                if next > address && line != 0 {
                    let file = files.id(dwarf, unit, file, dump)?;
                    lines.push(Line { address: address - base, size: next - address, line, file });
                }
            }
        }
        sequence.clear();
    }
    lines.sort_by_key(|line| line.address);
    // Consecutive records of one line read as one.
    lines.dedup_by(|next, line| {
        let joins =
            line.address + line.size == next.address && line.line == next.line && line.file == next.file;
        if joins {
            line.size += next.size;
        }
        joins
    });
    Ok(lines)
}

/// Walks a unit's tree, gathering its functions and the calls inlined into each.
struct Walk<'w, 'a> {
    dwarf: &'w gimli::Dwarf<Reader<'a>>,
    unit: &'w Unit<'a>,
    names: &'w Names<'w, 'a>,
    base: u64,
    files: &'w mut UnitFiles,
    dump: &'w mut Dump,
    functions: &'w mut Vec<Pending>,
}

impl<'a> Walk<'_, 'a> {
    /// `function` is the function this node is code of, if any, and `depth` how many inlined calls deep.
    fn node(
        &mut self,
        node: gimli::EntriesTreeNode<'_, '_, Reader<'a>>,
        function: Option<&mut Pending>,
        depth: u32,
    ) -> anyhow::Result<()> {
        let entry = node.entry().clone();
        match entry.tag() {
            constants::DW_TAG_subprogram => {
                let ranges = self.ranges(&entry)?;
                if ranges.is_empty() {
                    // A declaration or an abstract instance: no code of its own.
                    return self.children(node, None, 0);
                }
                let name = self.names.of(self.unit, &entry, 0)?.unwrap_or_else(|| "<unnamed>".into());
                let mut pending = Pending { name, ranges, inlines: Vec::new() };
                self.children(node, Some(&mut pending), 0)?;
                self.functions.push(pending);
                Ok(())
            }
            constants::DW_TAG_inlined_subroutine if function.is_some() => {
                let function = function.expect("checked");
                let ranges = self.ranges(&entry)?;
                if ranges.is_empty() {
                    return self.children(node, Some(function), depth);
                }
                let name = self.names.of(self.unit, &entry, 0)?.unwrap_or_else(|| "<unnamed>".into());
                let origin = self.dump.origins.id(name.as_ref().into());
                let call_file = match entry.attr_value(constants::DW_AT_call_file) {
                    Some(AttributeValue::FileIndex(index) | AttributeValue::Udata(index)) => {
                        self.files.id(self.dwarf, self.unit, index, self.dump)?
                    }
                    _ => self.dump.files.id("<unknown>".into()),
                };
                let call_line = match entry.attr_value(constants::DW_AT_call_line) {
                    Some(AttributeValue::Udata(line)) => line,
                    Some(value) => value.udata_value().unwrap_or(0),
                    None => 0,
                };
                function.inlines.push(Inline { depth, call_line, call_file, origin, ranges });
                self.children(node, Some(function), depth + 1)
            }
            _ => self.children(node, function, depth),
        }
    }

    fn children(
        &mut self,
        node: gimli::EntriesTreeNode<'_, '_, Reader<'a>>,
        mut function: Option<&mut Pending>,
        depth: u32,
    ) -> anyhow::Result<()> {
        let mut children = node.children();
        while let Some(child) = children.next()? {
            self.node(child, function.as_deref_mut(), depth)?;
        }
        Ok(())
    }

    /// The entry's code, relative to the module's load address; none where the linker dropped it.
    fn ranges(
        &self,
        entry: &gimli::DebuggingInformationEntry<Reader<'a>>,
    ) -> anyhow::Result<Vec<Range<u64>>> {
        let mut ranges = Vec::new();
        let mut iter = self.dwarf.die_ranges(self.unit, entry)?;
        while let Some(range) = iter.next()? {
            if range.begin < range.end && range.begin != 0 && range.begin >= self.base {
                ranges.push(range.begin - self.base..range.end - self.base);
            }
        }
        Ok(ranges)
    }
}
