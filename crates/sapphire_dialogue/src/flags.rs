//! The flag registry (`content/flags.ron`): every flag a game's dialogues may read or set, by name.
//!
//! ```ron
//! (
//!     type: "sapphire::dialogue::FlagRegistry",
//!     version: 1,
//!     data: (flags: ["met_guide", "ward_lit"]),
//! )
//! ```
//!
//! A flag is a whole number the simulation keeps (`sapphire_gameplay`'s `Flags`), 0 until something sets
//! it. The registry is the cook's: it refuses a dialogue that names a flag it doesn't list, and a registry
//! past [`MAX_FLAGS`], so the simulation's fixed table can never be full of flags a game meant to have.

use bevy_reflect::Reflect;
use sapphire_reflect::Versioned;
use serde::{Deserialize, Serialize};

use crate::document::{DialogueError, check_name};

/// The most flags a game has, and so the simulation's table holds: spec §5's budget.
pub const MAX_FLAGS: usize = 256;

/// The registry: see the [module docs](self).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, Reflect)]
#[type_path = "sapphire::dialogue"]
#[serde(deny_unknown_fields)]
pub struct FlagRegistry {
    pub flags: Vec<String>,
}

impl Versioned for FlagRegistry {
    /// 1: M3 Q2.
    const VERSION: u32 = 1;
}

impl FlagRegistry {
    /// Reads a registry from RON text and checks it.
    pub fn from_ron(text: &str) -> Result<Self, DialogueError> {
        let registry: Self = sapphire_serialize::from_ron(text).map_err(Box::new)?;
        registry.check()?;
        Ok(registry)
    }

    /// Checks everything, naming the field.
    pub fn check(&self) -> Result<(), DialogueError> {
        if self.flags.len() > MAX_FLAGS {
            let problem = format!("{} flags, past the simulation's {MAX_FLAGS}", self.flags.len());
            return Err(DialogueError::invalid("flags", problem));
        }
        for (index, flag) in self.flags.iter().enumerate() {
            check_name(|| format!("flags[{index}]"), flag)?;
            if self.flags[..index].contains(flag) {
                return Err(DialogueError::invalid(
                    format!("flags[{index}]"),
                    format!("{flag:?} is listed already"),
                ));
            }
        }
        Ok(())
    }

    /// Whether it lists `flag`.
    #[must_use]
    pub fn has(&self, flag: &str) -> bool {
        self.flags.iter().any(|listed| listed == flag)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn registry(flags: &[&str]) -> FlagRegistry {
        FlagRegistry { flags: flags.iter().map(|&flag| flag.to_owned()).collect() }
    }

    #[test]
    fn a_registry_is_held_to_the_simulations_table() {
        let full: Vec<String> = (0..MAX_FLAGS).map(|index| format!("flag_{index}")).collect();
        let mut registry = FlagRegistry { flags: full };
        registry.check().expect("256 fit");
        registry.flags.push("one_more".to_owned());
        let error = registry.check().unwrap_err().to_string();
        assert!(error.contains("`flags`") && error.contains("257"), "{error}");
    }

    #[test]
    fn checks_name_the_flag() {
        let error = registry(&["a", "b", "a"]).check().unwrap_err().to_string();
        assert!(error.contains("`flags[2]`"), "{error}");
        let error = registry(&["a", "no spaces"]).check().unwrap_err().to_string();
        assert!(error.contains("`flags[1]`"), "{error}");
    }

    #[test]
    fn a_field_nobody_knows_is_refused() {
        let text = r#"(type: "sapphire::dialogue::FlagRegistry", version: 1, data: (flags: [], quests: []))"#;
        let error = FlagRegistry::from_ron(text).unwrap_err().to_string();
        assert!(error.contains("quests"), "{error}");
    }
}
