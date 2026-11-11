//! Where a crash's modules find their symbols (M4 C2): the packages' `package.json` `modules`, each a
//! shipped binary's debug file and ID beside its native symbols, and a Breakpad store of the `.sym` files
//! made from them, written the first time a crash needs one.

use std::fs;
use std::io::{BufWriter, Write as _};
use std::path::{Path, PathBuf};

use anyhow::{Context as _, anyhow, bail};

use crate::{DebugId, Format, Module};

/// A module a package shipped, and where its native symbols are.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Packaged {
    pub module: Module,
    /// The dSYM, the unstripped ELF or the PDB.
    pub symbols: PathBuf,
}

impl Packaged {
    /// Whether a crash's module, named by its build's ID as the crash records it (a Mach-O UUID with or
    /// without dashes, an ELF build ID in hex, or a Breakpad debug ID), is this one.
    pub fn is(&self, id: &str) -> bool {
        let id: String = id.chars().filter(|c| *c != '-').collect::<String>().to_uppercase();
        id == self.module.debug_id.to_string()
            || (!self.module.code_id.is_empty() && id == self.module.code_id.to_uppercase())
    }
}

/// Every module the packages in `dirs` shipped.
pub fn packaged(dirs: &[PathBuf]) -> anyhow::Result<Vec<Packaged>> {
    let mut modules = Vec::new();
    for dir in dirs {
        let manifest = dir.join("package.json");
        let text = fs::read_to_string(&manifest)
            .with_context(|| format!("failed to read {}", manifest.display()))?;
        let json: serde_json::Value =
            serde_json::from_str(&text).with_context(|| format!("{} isn't JSON", manifest.display()))?;
        let listed = json["modules"].as_array().with_context(|| {
            format!(
                "{} lists no modules: a package made before M4 C1, whose symbols can't be found",
                manifest.display()
            )
        })?;
        for module in listed {
            modules.push(read_module(dir, module).with_context(|| format!("in {}", manifest.display()))?);
        }
    }
    Ok(modules)
}

fn read_module(dir: &Path, json: &serde_json::Value) -> anyhow::Result<Packaged> {
    let field = |key: &str| json[key].as_str().ok_or_else(|| anyhow!("a module has no `{key}`"));
    let format = match field("format")? {
        "mach-o" => Format::MachO,
        "elf" => Format::Elf,
        "pe" => Format::Pe,
        other => bail!("a module's format is {other:?}"),
    };
    let arch = match field("arch")? {
        "arm64" => "arm64",
        "x86_64" => "x86_64",
        other => bail!("a module's arch is {other:?}"),
    };
    let module = Module {
        format,
        arch,
        debug_file: field("debug_file")?.to_owned(),
        debug_id: field("debug_id")?.parse()?,
        code_id: field("code_id")?.to_owned(),
    };
    let symbols = field("symbols")?.split('/').fold(dir.to_owned(), |path, part| path.join(part));
    Ok(Packaged { module, symbols })
}

/// A Breakpad symbol store: `<root>/<debug file>/<debug ID>/<name>.sym`, which `minidump-processor` reads.
pub struct Store {
    pub root: PathBuf,
}

impl Store {
    /// The `.sym` of `packaged`, written from its native symbols unless the store has it already.
    pub fn ensure(&self, packaged: &Packaged) -> anyhow::Result<PathBuf> {
        let path = self.root.join(packaged.module.store_path());
        if path.is_file() {
            return Ok(path);
        }
        let dir = path.parent().expect("a store path has a directory");
        fs::create_dir_all(dir).with_context(|| format!("failed to create {}", dir.display()))?;
        let partial = path.with_extension("sym.partial");
        let written = (|| {
            let file = fs::File::create(&partial)?;
            let mut out = BufWriter::new(file);
            crate::sym::write(&packaged.module, &packaged.symbols, &mut out)?;
            out.flush()?;
            fs::rename(&partial, &path)?;
            anyhow::Ok(())
        })();
        if let Err(error) = written {
            let _ = fs::remove_file(&partial);
            return Err(error.context(format!("failed to write {}", path.display())));
        }
        eprintln!("wrote {} from {}", path.display(), packaged.symbols.display());
        Ok(path)
    }
}

impl std::str::FromStr for DebugId {
    type Err = anyhow::Error;

    /// 32 hex digits of GUID and the age's, as [`DebugId`]'s `Display` writes them.
    fn from_str(text: &str) -> anyhow::Result<Self> {
        let bad = || anyhow!("{text:?} isn't a debug ID: 32 hex digits and an age");
        if text.len() < 33 || text.len() > 40 || !text.is_ascii() {
            return Err(bad());
        }
        let (guid_hex, age_hex) = text.split_at(32);
        let mut guid = [0; 16];
        for (index, byte) in guid.iter_mut().enumerate() {
            *byte = u8::from_str_radix(&guid_hex[index * 2..index * 2 + 2], 16).map_err(|_| bad())?;
        }
        let age = u32::from_str_radix(age_hex, 16).map_err(|_| bad())?;
        Ok(Self { guid, age })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn debug_ids_read_back() {
        let id: DebugId = "000102030405060708090A0B0C0D0E0F2A".parse().unwrap();
        assert_eq!(id.age, 0x2a);
        assert_eq!(id.to_string(), "000102030405060708090A0B0C0D0E0F2A");
        assert!("0001".parse::<DebugId>().is_err());
        assert!("ZZ0102030405060708090A0B0C0D0E0F0".parse::<DebugId>().is_err());
    }

    #[test]
    fn modules_are_found_by_any_name_their_crash_gives_them() {
        let dir = Path::new("pkg");
        let json = serde_json::json!({
            "format": "mach-o", "arch": "arm64", "debug_file": "game_app",
            "debug_id": "0A6B168B11223344556677889900AABB0", "code_id": "0A6B168B11223344556677889900AABB",
            "symbols": "symbols/game_app.dSYM",
        });
        let packaged = read_module(dir, &json).unwrap();
        assert_eq!(packaged.symbols, dir.join("symbols").join("game_app.dSYM"));
        assert!(packaged.is("0A6B168B-1122-3344-5566-77889900AABB"));
        assert!(packaged.is("0a6b168b11223344556677889900aabb"));
        assert!(packaged.is("0A6B168B11223344556677889900AABB0"));
        assert!(!packaged.is("1A6B168B11223344556677889900AABB"));
        let mut wrong = json.clone();
        wrong["format"] = "coff".into();
        assert!(read_module(dir, &wrong).is_err());
    }
}
