//! The table of desktop GPUs (M4 T4): the highest desktop tier each known family of adapters may take,
//! read before the tiers' RAM rule, which can't see a GPU's age or memory. Without it a GTX 1060 machine
//! with 16 GB of RAM qualified for Desktop-High.
//!
//! # The table
//!
//! `gpus.ron`, next to this crate's manifest, is a [`GpuTable`] document (schema 1), compiled in as the
//! tier table is ([`GpuTable::shipped`]), so a new GPU is a row or a name in one, not code. Each row is a
//! family ([`GpuFamily`]): its vendor, the names that recognise it, and its `ceiling`, the highest desktop
//! tier it may take. [`TierTable::select`](super::TierTable::select) gives a known adapter the lower of its
//! ceiling and what the rules give, so a row can only lower a tier, never raise one: the RAM rule's memory
//! budgets still hold under it. An adapter no row knows, every phone, and the software and paravirtual
//! adapters the golden images draw on, select as they did before the table.
//!
//! # Recognising an adapter
//!
//! By its name as the driver reports it (`wgpu::AdapterInfo::name`), gated by its PCI vendor id. Device
//! ids are stable, but a family has dozens (each board, memory size and laptop part has its own), the only
//! complete list is the PCI ID database, and Metal reports none at all; the marketing name carries the
//! family, and is what every backend reports. Metal reports no vendor either (0), so there the name alone
//! decides.
//!
//! A name is compared as tokens: lowercased, `(R)` and `(TM)` dropped, split at anything that isn't a
//! letter or a digit and between letters and digits, so `Intel(R) Arc(TM) A770 Graphics` reads
//! `intel arc a 770 graphics`. A row's name matches an adapter whose tokens hold its tokens as one run:
//! `GeForce GTX` matches every GTX board, `Radeon RX 580` the 580 but not the 5800. Where several match,
//! the longest wins, so `GeForce RTX 3060 Laptop GPU` overrides `GeForce RTX 3060`. Two names that could
//! meet in one adapter's name, sharing tokens, must nest one in the other ([`GpuTable::new`] refuses
//! them otherwise), so no adapter matches two rows ambiguously.

use std::cmp::Reverse;

use bevy_reflect::TypePath;
use sapphire_core::target::FormFactor;
use sapphire_reflect::Versioned;
use sapphire_serialize::DocumentError;
use serde::Deserialize;

use super::TierId;

/// The shipped table (see the module docs).
const SHIPPED: &str = include_str!("../../gpus.ron");

/// A GPU vendor, by the PCI vendor id its adapters report on Vulkan and DX12.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Deserialize)]
pub enum GpuVendor {
    Nvidia,
    Amd,
    Intel,
    Apple,
}

impl GpuVendor {
    /// Its PCI vendor id.
    pub fn pci_id(self) -> u32 {
        match self {
            Self::Nvidia => 0x10DE,
            Self::Amd => 0x1002,
            Self::Intel => 0x8086,
            Self::Apple => 0x106B,
        }
    }

    /// Whether an adapter reporting `vendor` may be this vendor's: its id, or none (0, as Metal reports).
    fn admits(self, vendor: u32) -> bool {
        vendor == 0 || vendor == self.pci_id()
    }
}

/// One family of adapters and the highest desktop tier it may take.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GpuFamily {
    /// What the log calls it: `GeForce GTX`.
    pub family: String,
    pub vendor: GpuVendor,
    /// The names that recognise it, each matched as a run of tokens (see the module docs).
    pub names: Vec<String>,
    /// The highest tier it may take: a desktop tier.
    pub ceiling: TierId,
}

/// The GPU table (`gpus.ron`): families whose ceilings are desktop tiers and whose names can't match one
/// adapter ambiguously. Every way of making one checks this, deserializing included.
#[derive(Clone, Debug, PartialEq, TypePath)]
#[type_path = "sapphire::render"]
pub struct GpuTable {
    families: Vec<GpuFamily>,
    /// Every family's names as tokens, with the family's index.
    names: Vec<(Vec<String>, usize)>,
}

impl Versioned for GpuTable {
    const VERSION: u32 = 1;
}

/// A table as written, before [`GpuTable::new`] checks it.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct UncheckedTable {
    families: Vec<GpuFamily>,
}

impl<'de> Deserialize<'de> for GpuTable {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let table = UncheckedTable::deserialize(deserializer)?;
        Self::new(table.families).map_err(serde::de::Error::custom)
    }
}

/// Why a GPU table is invalid.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum GpuTableError {
    #[error("a family has no name")]
    Unnamed,
    #[error("the table has `{0}` more than once")]
    DuplicateFamily(String),
    #[error("`{0}` has no names to recognise it by")]
    NoNames(String),
    #[error("`{family}`: the name {name:?} has no letters or digits")]
    EmptyName { family: String, name: String },
    #[error("`{family}`: its ceiling, {ceiling}, isn't a desktop tier")]
    NotDesktop { family: String, ceiling: TierId },
    #[error("{first:?} and {second:?} can match the same adapter with neither more specific than the other")]
    Ambiguous { first: String, second: String },
}

impl GpuTable {
    /// A table of `families`, checked.
    pub fn new(families: Vec<GpuFamily>) -> Result<Self, GpuTableError> {
        let mut names: Vec<(Vec<String>, usize)> = Vec::new();
        let mut written: Vec<&str> = Vec::new();
        for (index, family) in families.iter().enumerate() {
            if family.family.trim().is_empty() {
                return Err(GpuTableError::Unnamed);
            }
            if families[..index].iter().any(|other| other.family == family.family) {
                return Err(GpuTableError::DuplicateFamily(family.family.clone()));
            }
            if family.names.is_empty() {
                return Err(GpuTableError::NoNames(family.family.clone()));
            }
            if family.ceiling.class() != FormFactor::Desktop {
                return Err(GpuTableError::NotDesktop {
                    family: family.family.clone(),
                    ceiling: family.ceiling,
                });
            }
            for name in &family.names {
                let tokens = tokens(name);
                if tokens.is_empty() {
                    return Err(GpuTableError::EmptyName {
                        family: family.family.clone(),
                        name: name.clone(),
                    });
                }
                if let Some(other) = names.iter().position(|(other, _)| ambiguous(other, &tokens)) {
                    return Err(GpuTableError::Ambiguous {
                        first: written[other].to_owned(),
                        second: name.clone(),
                    });
                }
                names.push((tokens, index));
                written.push(name);
            }
        }
        Ok(Self { families, names })
    }

    /// The table compiled into the renderer (`gpus.ron`).
    pub fn shipped() -> Self {
        Self::from_ron(SHIPPED).expect("the shipped GPU table is checked by this module's tests")
    }

    /// Reads a table document, checking it.
    pub fn from_ron(text: &str) -> Result<Self, DocumentError> {
        sapphire_serialize::from_ron(text)
    }

    /// Every family, in the table's order.
    pub fn families(&self) -> &[GpuFamily] {
        &self.families
    }

    /// The family of the adapter named `name` that reports PCI vendor id `vendor` (0 for none), if a row
    /// knows it: the one whose matching name is longest.
    pub fn find(&self, vendor: u32, name: &str) -> Option<&GpuFamily> {
        let adapter = tokens(name);
        self.names
            .iter()
            .filter(|(tokens, family)| {
                self.families[*family].vendor.admits(vendor) && holds(&adapter, tokens)
            })
            // Names that match together nest, so the longest is the most specific; a tie can only be two
            // names apart in one adapter's name, and the lower ceiling is the cautious one.
            .max_by_key(|(tokens, family)| (tokens.len(), Reverse(self.families[*family].ceiling)))
            .map(|(_, family)| &self.families[*family])
    }
}

/// `name` as tokens (see the module docs).
fn tokens(name: &str) -> Vec<String> {
    let name = name.to_ascii_lowercase().replace("(r)", " ").replace("(tm)", " ");
    let mut tokens = Vec::new();
    let mut token = String::new();
    for c in name.chars() {
        let breaks = token.chars().last().is_some_and(|last| last.is_ascii_digit() != c.is_ascii_digit());
        if (!c.is_ascii_alphanumeric() || breaks) && !token.is_empty() {
            tokens.push(std::mem::take(&mut token));
        }
        if c.is_ascii_alphanumeric() {
            token.push(c);
        }
    }
    if !token.is_empty() {
        tokens.push(token);
    }
    tokens
}

/// Whether `tokens` holds `run` as consecutive tokens.
fn holds(tokens: &[String], run: &[String]) -> bool {
    tokens.windows(run.len()).any(|window| window == run)
}

/// Whether two names can match one adapter with neither more specific: the same tokens, or one's tail
/// the other's head without either holding the other (`GeForce RTX` and `RTX 3060 Laptop`).
fn ambiguous(a: &[String], b: &[String]) -> bool {
    if a == b {
        return true;
    }
    if holds(a, b) || holds(b, a) {
        return false;
    }
    let overlaps = |head: &[String], tail: &[String]| {
        (1..head.len().min(tail.len())).any(|n| tail[tail.len() - n..] == head[..n])
    };
    overlaps(a, b) || overlaps(b, a)
}

#[cfg(test)]
mod tests {
    use super::*;

    const NVIDIA: u32 = 0x10DE;
    const AMD: u32 = 0x1002;
    const INTEL: u32 = 0x8086;

    fn family(family: &str, names: &[&str], ceiling: TierId) -> GpuFamily {
        GpuFamily {
            family: family.into(),
            vendor: GpuVendor::Nvidia,
            names: names.iter().map(|&name| name.into()).collect(),
            ceiling,
        }
    }

    #[test]
    fn names_read_as_tokens() {
        assert_eq!(tokens("Intel(R) Arc(TM) A770 Graphics"), ["intel", "arc", "a", "770", "graphics"]);
        assert_eq!(tokens("NVIDIA GeForce GTX 1060 6GB"), ["nvidia", "geforce", "gtx", "1060", "6", "gb"]);
        assert_eq!(
            tokens("AMD Radeon RX 580 Series (RADV POLARIS10)"),
            ["amd", "radeon", "rx", "580", "series", "radv", "polaris", "10"]
        );
        assert_eq!(tokens("Apple M2 Pro"), ["apple", "m", "2", "pro"]);
        assert_eq!(tokens("Radeon™ RX 6500M"), ["radeon", "rx", "6500", "m"]);
        assert!(tokens(" (R) - ").is_empty());
    }

    /// Adapter names as their drivers report them (Windows and Linux, Vulkan and DX12; Metal for Apple),
    /// from the vendors' naming: not read from these devices.
    #[test]
    fn the_shipped_table_knows_each_family_by_its_reported_names() {
        let table = GpuTable::shipped();
        let known =
            |vendor, name| table.find(vendor, name).map(|family| (family.family.as_str(), family.ceiling));
        use TierId::{DesktopHigh as High, DesktopLow as Low};
        let cases: [(u32, &str, &str, TierId); 22] = [
            (NVIDIA, "NVIDIA GeForce GTX 1060 6GB", "GeForce GTX", Low),
            (NVIDIA, "NVIDIA GeForce GTX 1080 Ti", "GeForce GTX", Low),
            (NVIDIA, "NVIDIA GeForce GTX 1660 SUPER", "GeForce GTX", Low),
            (NVIDIA, "NVIDIA GeForce MX450", "GeForce GT and MX", Low),
            (NVIDIA, "NVIDIA GeForce GT 1030", "GeForce GT and MX", Low),
            (NVIDIA, "NVIDIA GeForce RTX 2060", "GeForce RTX, under 8 GB or entry", Low),
            (NVIDIA, "NVIDIA GeForce RTX 2060 SUPER", "GeForce RTX", High),
            (NVIDIA, "NVIDIA GeForce RTX 3050 Ti Laptop GPU", "GeForce RTX, under 8 GB or entry", Low),
            (NVIDIA, "NVIDIA GeForce RTX 3060 Laptop GPU", "GeForce RTX, under 8 GB or entry", Low),
            (NVIDIA, "NVIDIA GeForce RTX 3060", "GeForce RTX", High),
            (NVIDIA, "NVIDIA GeForce RTX 3070", "GeForce RTX", High),
            (NVIDIA, "NVIDIA GeForce RTX 4090", "GeForce RTX", High),
            (AMD, "AMD Radeon RX 580 Series (RADV POLARIS10)", "Radeon GCN", Low),
            (AMD, "Radeon RX Vega", "Radeon GCN", Low),
            (AMD, "AMD Radeon RX 6500 XT", "Radeon RDNA, under 8 GB or entry", Low),
            (AMD, "AMD Radeon RX 5600 XT", "Radeon RDNA, under 8 GB or entry", Low),
            (AMD, "AMD Radeon RX 6700 XT", "Radeon RDNA", High),
            (AMD, "AMD Radeon RX 7900 XTX", "Radeon RDNA", High),
            (INTEL, "Intel(R) Arc(TM) A380 Graphics", "Arc, under 8 GB or entry", Low),
            (INTEL, "Intel(R) Arc(TM) B580 Graphics", "Arc", High),
            (0, "Apple M1", "Apple M1 and M2", Low),
            (0, "Apple M2 Pro", "Apple M1 and M2 Pro, Max and Ultra", High),
        ];
        for (vendor, name, family, ceiling) in cases {
            assert_eq!(known(vendor, name), Some((family, ceiling)), "{name}");
        }
        // Unknown: integrated GPUs (the rules already keep them off Desktop-High but on Apple silicon),
        // Apple's newer chips, the software and paravirtual adapters, and a GTX on another vendor's id.
        for (vendor, name) in [
            (INTEL, "Intel(R) UHD Graphics 630"),
            (AMD, "AMD Radeon 780M Graphics"),
            (0, "Apple M5"),
            (0, "Apple M3 Max"),
            (0, "Apple Paravirtual device"),
            (0x10005, "llvmpipe (LLVM 15.0.7, 256 bits)"),
            (0x1414, "Microsoft Basic Render Driver"),
            (AMD, "NVIDIA GeForce GTX 1060 6GB"),
            (AMD, "AMD Radeon RX 5800"),
        ] {
            assert_eq!(known(vendor, name), None, "{name}");
        }
    }

    #[test]
    fn the_shipped_table_is_valid() {
        let table = GpuTable::shipped();
        assert!(!table.families().is_empty());
        for family in table.families() {
            assert_eq!(family.ceiling.class(), FormFactor::Desktop, "{}", family.family);
        }
        // `new` refuses names that could match one adapter ambiguously; the shipped table passed it, so
        // checking each pair again here would only repeat it. Each name finds its own family.
        for family in table.families() {
            for name in &family.names {
                assert_eq!(table.find(0, name), Some(family), "{name}");
            }
        }
    }

    #[test]
    fn bad_tables_are_refused() {
        use TierId::{DesktopHigh as High, DesktopLow as Low};
        let check = |families: Vec<GpuFamily>| GpuTable::new(families).unwrap_err();
        assert_eq!(check(vec![family(" ", &["GTX"], Low)]), GpuTableError::Unnamed);
        assert_eq!(check(vec![family("A", &[], Low)]), GpuTableError::NoNames("A".into()));
        assert!(matches!(check(vec![family("A", &["(TM)"], Low)]), GpuTableError::EmptyName { .. }));
        assert!(matches!(
            check(vec![family("A", &["GTX"], TierId::MobileHigh)]),
            GpuTableError::NotDesktop { .. }
        ));
        assert_eq!(
            check(vec![family("A", &["GTX"], Low), family("A", &["RTX"], Low)]),
            GpuTableError::DuplicateFamily("A".into())
        );
        // The same name twice, however it's spelled.
        assert!(matches!(
            check(vec![family("A", &["GeForce GTX"], Low), family("B", &["geforce-gtx"], High)]),
            GpuTableError::Ambiguous { .. }
        ));
        // Overlapping without nesting: `GeForce RTX 3060 Laptop` holds both.
        assert_eq!(
            check(vec![family("A", &["GeForce RTX 3060"], High), family("B", &["3060 Laptop"], Low)]),
            GpuTableError::Ambiguous { first: "GeForce RTX 3060".into(), second: "3060 Laptop".into() }
        );
        // Nested is fine: the longer is more specific.
        let nested = GpuTable::new(vec![
            family("A", &["GeForce RTX 3060"], High),
            family("B", &["GeForce RTX 3060 Laptop GPU"], Low),
        ])
        .unwrap();
        assert_eq!(nested.find(NVIDIA, "NVIDIA GeForce RTX 3060 Laptop GPU").unwrap().family, "B");
        assert_eq!(nested.find(NVIDIA, "NVIDIA GeForce RTX 3060 Ti").unwrap().family, "A");
    }

    #[test]
    fn documents_are_checked_as_they_load() {
        let error = GpuTable::from_ron(&SHIPPED.replacen("ceiling: DesktopLow", "ceiling: MobileHigh", 1))
            .unwrap_err()
            .to_string();
        assert!(error.contains("isn't a desktop tier"), "{error}");
        let unknown = SHIPPED.replacen("ceiling: DesktopLow", "ceiling: DesktopLow, measured: true", 1);
        assert!(GpuTable::from_ron(&unknown).is_err(), "unknown fields are refused");
        let newer = SHIPPED.replace("version: 1", "version: 2");
        assert!(matches!(GpuTable::from_ron(&newer), Err(DocumentError::TooNew { found: 2, current: 1 })));
    }
}
