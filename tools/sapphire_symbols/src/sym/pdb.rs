//! A PDB, read with `pdb`: each module's procedures and their lines, and the public symbols.

use std::fs::File;
use std::path::Path;

use anyhow::Context as _;
use pdb::{FallibleIterator as _, SymbolData};

use super::{Dump, Line, same_id};
use crate::Module;

/// Reads the PDB at `path`, which must carry `module`'s ID, into `dump`.
pub(super) fn read(path: &Path, module: &Module, dump: &mut Dump) -> anyhow::Result<()> {
    same_id(crate::pdb_id(path)?, module)?;
    let file = File::open(path).with_context(|| format!("failed to open {}", path.display()))?;
    let mut pdb = pdb::PDB::open(file)?;
    let address_map = pdb.address_map()?;
    let strings = pdb.string_table()?;
    let debug = pdb.debug_information()?;
    let mut modules = debug.modules()?;
    while let Some(unit) = modules.next()? {
        let Some(info) = pdb.module_info(&unit)? else { continue };
        let program = info.line_program()?;
        let mut symbols = info.symbols()?;
        while let Some(symbol) = symbols.next()? {
            let Ok(SymbolData::Procedure(procedure)) = symbol.parse() else { continue };
            let Some(start) = procedure.offset.to_rva(&address_map) else { continue };
            let range = u64::from(start.0)..u64::from(start.0) + u64::from(procedure.len);
            let mut lines = Vec::new();
            let mut iter = program.lines_for_symbol(procedure.offset);
            while let Some(line) = iter.next()? {
                let Some(address) = line.offset.to_rva(&address_map) else { continue };
                let path = program.get_file_info(line.file_index)?.name.to_string_lossy(&strings)?;
                let file = dump.files.id(path);
                let size = line.length.map_or(0, u64::from);
                lines.push(Line {
                    address: u64::from(address.0),
                    size,
                    line: u64::from(line.line_start),
                    file,
                });
            }
            lines.sort_by_key(|line| line.address);
            // A record with no length runs to the next, or to the procedure's end.
            for index in 0..lines.len() {
                if lines[index].size == 0 {
                    let next = lines.get(index + 1).map_or(range.end, |next| next.address);
                    lines[index].size = next.saturating_sub(lines[index].address);
                }
            }
            lines.retain(|line| line.line != 0 && line.size > 0);
            let name = super::demangle(&procedure.name.to_string());
            dump.add_function(range, &name, &lines, &[]);
        }
    }
    let globals = pdb.global_symbols()?;
    let mut iter = globals.iter();
    while let Some(symbol) = iter.next()? {
        if let Ok(SymbolData::Public(public)) = symbol.parse()
            && public.function
            && let Some(address) = public.offset.to_rva(&address_map)
        {
            dump.add_public(u64::from(address.0), &public.name.to_string());
        }
    }
    Ok(())
}
