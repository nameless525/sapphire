//! Encounters (M3 B2): `.encounter.ron` to `.sencounter`, a fight's waves and how many may swing at once.
//!
//! ```ron
//! (
//!     type: "sapphire::zone::EncounterDocument",
//!     version: 1,
//!     data: (
//!         starts_on: "gate_entered",
//!         faction: 2,
//!         tokens: 2,
//!         token_rest: 30,
//!         waves: [
//!             (when: Start, spawners: ["north_door", "south_door"]),
//!             (when: Remaining(1), gap: 20, spawners: ["north_door", "south_door", "east_door"]),
//!         ],
//!     ),
//! )
//! ```
//!
//! **What it names:** a zone's trigger by its event and a zone's spawners by their names, so one encounter
//! fits any zone that has those, and what a spawner spawns is the spawner's (`spawns`). A zone's cook holds an
//! encounter it lists to the zone's own names, and [`ArchivedEncounter::check_in`] says the same at load.
//!
//! **It names no moves by ID.** A spawner's `spawns` is a move set's name, so an encounter's enemies don't
//! leave `shared` for a zone's pack while the app still finds them by name there. What pre-warms them (W5)
//! goes by those names.
//!
//! **Time is in ticks of the simulation** rather than of anyone's clock: hitstop on one fighter doesn't
//! hold a wave back.
//!
//! The rules are three and data: when a wave comes ([`WaveStart`]), the gap between its spawns, and the tokens.
//! A token is what a brain has to hold to press an attack, and it rests when it comes back: see
//! `sapphire_gameplay::encounter`, which runs all this.

use bevy_reflect::{Reflect, TypePath};
use sapphire_core::Name;
use sapphire_reflect::Versioned;
use sapphire_serialize::cooked::AlignedVec;
use serde::{Deserialize, Serialize};

use crate::cooked::{ArchivedZoneSimulation, ZoneSimulation};
use crate::document::{MAX_NAME, ZoneError, check_name};

/// Conventional file extension for cooked encounters.
pub const ENCOUNTER_EXTENSION: &str = "sencounter";

/// The most tokens an encounter may have. A simulation keeps this many slots for each.
pub const MAX_TOKENS: u8 = 8;
/// The most waves in an encounter.
pub const MAX_WAVES: usize = 32;
/// The most spawns in a wave.
pub const MAX_WAVE_SPAWNS: usize = 64;
/// The highest faction's number (`sapphire_gameplay::Faction::COUNT`, held to this by a test there). A
/// faction of 0 is no side, which tokens would have nobody to share with.
pub const MAX_FACTION: u8 = 16;

/// When a wave comes, once the one before it has come.
#[derive(
    Clone,
    Copy,
    Debug,
    PartialEq,
    Eq,
    Serialize,
    Deserialize,
    Reflect,
    rkyv::Archive,
    rkyv::Serialize,
    rkyv::Deserialize,
)]
#[type_path = "sapphire::zone"]
#[rkyv(derive(Debug, Clone, Copy, PartialEq))]
pub enum WaveStart {
    /// As the encounter starts. Only the first wave comes this way, and it always does.
    Start,
    /// When at most this many of the encounter's fighters are still standing.
    Remaining(u16),
    /// This many ticks after the wave before it started.
    After(u32),
}

/// One wave, in a document.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Reflect)]
#[type_path = "sapphire::zone"]
#[serde(deny_unknown_fields)]
pub struct Wave {
    pub when: WaveStart,
    /// Ticks between one spawn of this wave and the next, so a crowd arrives rather than appears.
    #[serde(default)]
    pub gap: u32,
    /// The zone's spawners, by name, in the order they go. One may appear more than once.
    pub spawners: Vec<String>,
}

/// An encounter document (see the [module docs](self)).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Reflect)]
#[type_path = "sapphire::zone"]
#[serde(deny_unknown_fields)]
pub struct EncounterDocument {
    /// The event of the zone's trigger that starts it.
    pub starts_on: String,
    /// Whose side its fighters are on: everyone it spawns shares this one, and its tokens.
    pub faction: u8,
    /// How many of its fighters may be swinging at once.
    pub tokens: u8,
    /// Ticks a token rests after it comes back, before anyone can take it.
    #[serde(default)]
    pub token_rest: u32,
    pub waves: Vec<Wave>,
}

impl Versioned for EncounterDocument {
    /// 1: M3 B2.
    const VERSION: u32 = 1;
}

impl EncounterDocument {
    /// Reads a document from RON text and checks it.
    pub fn from_ron(text: &str) -> Result<Self, ZoneError> {
        let document: Self = sapphire_serialize::from_ron(text).map_err(Box::new)?;
        document.check()?;
        Ok(document)
    }

    /// Checks everything that needs nothing outside the document, naming the field.
    pub fn check(&self) -> Result<(), ZoneError> {
        check_name(&self.starts_on).map_err(|problem| ZoneError::invalid("starts_on", problem))?;
        if self.faction == 0 || self.faction > MAX_FACTION {
            let problem = format!("{} isn't a side: they go from 1 to {MAX_FACTION}", self.faction);
            return Err(ZoneError::invalid("faction", problem));
        }
        if self.tokens == 0 || self.tokens > MAX_TOKENS {
            let problem = format!("{} isn't from 1 to {MAX_TOKENS}: nobody could ever swing", self.tokens);
            return Err(ZoneError::invalid("tokens", problem));
        }
        if self.waves.is_empty() || self.waves.len() > MAX_WAVES {
            let problem = format!("{} isn't from 1 to {MAX_WAVES}", self.waves.len());
            return Err(ZoneError::invalid("waves", problem));
        }
        for (index, wave) in self.waves.iter().enumerate() {
            let field = |name: &str| format!("waves[{index}].{name}");
            if (index == 0) != (wave.when == WaveStart::Start) {
                let problem = if index == 0 {
                    "the first wave comes as the encounter starts: it says `Start`"
                } else {
                    "only the first wave says `Start`"
                };
                return Err(ZoneError::invalid(field("when"), problem));
            }
            if wave.spawners.is_empty() || wave.spawners.len() > MAX_WAVE_SPAWNS {
                let problem = format!("{} isn't from 1 to {MAX_WAVE_SPAWNS}", wave.spawners.len());
                return Err(ZoneError::invalid(field("spawners"), problem));
            }
            for (at, spawner) in wave.spawners.iter().enumerate() {
                check_name(spawner).map_err(|problem| {
                    ZoneError::invalid(format!("waves[{index}].spawners[{at}]"), problem)
                })?;
            }
        }
        Ok(())
    }

    /// The cooked encounter.
    pub fn cook(&self) -> Result<Encounter, ZoneError> {
        self.check()?;
        let waves = self
            .waves
            .iter()
            .map(|wave| CookedWave { when: wave.when, gap: wave.gap, spawners: wave.spawners.clone() })
            .collect();
        Ok(Encounter {
            starts_on: self.starts_on.clone(),
            faction: self.faction,
            tokens: self.tokens,
            token_rest: self.token_rest,
            waves,
        })
    }

    /// Checks what it names against what a zone has, naming the field.
    pub fn check_in(&self, zone: &ZoneSimulation) -> Result<(), ZoneError> {
        if !zone.triggers.iter().any(|trigger| trigger.event == self.starts_on) {
            let problem = format!("no trigger of the zone fires `{}`", self.starts_on);
            return Err(ZoneError::invalid("starts_on", problem));
        }
        for (index, wave) in self.waves.iter().enumerate() {
            for (at, name) in wave.spawners.iter().enumerate() {
                if !zone.spawners.iter().any(|spawner| spawner.name == *name) {
                    let problem = format!("the zone has no spawner `{name}`");
                    return Err(ZoneError::invalid(format!("waves[{index}].spawners[{at}]"), problem));
                }
            }
        }
        Ok(())
    }
}

/// A wave, cooked.
#[derive(Clone, Debug, PartialEq, Eq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
#[rkyv(derive(Debug))]
pub struct CookedWave {
    pub when: WaveStart,
    pub gap: u32,
    pub spawners: Vec<String>,
}

/// A cooked encounter (`.sencounter`): an rkyv archive read in place. The simulation copies what it needs
/// when it's given a zone, and doesn't read this again.
#[derive(Clone, Debug, PartialEq, Eq, TypePath, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
#[rkyv(derive(Debug))]
#[type_path = "sapphire::zone"]
pub struct Encounter {
    pub starts_on: String,
    pub faction: u8,
    pub tokens: u8,
    pub token_rest: u32,
    pub waves: Vec<CookedWave>,
}

impl Versioned for Encounter {
    /// 1: M3 B2.
    const VERSION: u32 = 1;
}

impl Encounter {
    /// The encounter as bytes, for a pack.
    pub fn to_cooked(&self) -> Result<AlignedVec, ZoneError> {
        Ok(sapphire_serialize::to_cooked(self)?)
    }

    /// Reads cooked bytes in place, checking the envelope, the archive and every value the runtime reads.
    pub fn read(bytes: &[u8]) -> Result<&ArchivedEncounter, ZoneError> {
        let encounter = sapphire_serialize::access::<Self>(bytes)?;
        encounter.validate()?;
        Ok(encounter)
    }
}

impl ArchivedEncounter {
    /// Checks everything the runtime reads, whatever wrote these bytes.
    pub fn validate(&self) -> Result<(), ZoneError> {
        check_name(self.starts_on.as_str()).map_err(|problem| ZoneError::invalid("starts_on", problem))?;
        if self.faction == 0 || self.faction > MAX_FACTION {
            return Err(ZoneError::invalid("faction", format!("{} isn't a side", self.faction)));
        }
        if self.tokens == 0 || self.tokens > MAX_TOKENS {
            return Err(ZoneError::invalid("tokens", format!("{} is out of range", self.tokens)));
        }
        if self.waves.is_empty() || self.waves.len() > MAX_WAVES {
            return Err(ZoneError::invalid("waves", format!("{} is out of range", self.waves.len())));
        }
        for (index, wave) in self.waves.iter().enumerate() {
            let starts = matches!(wave.when, ArchivedWaveStart::Start);
            if (index == 0) != starts {
                return Err(ZoneError::invalid(
                    format!("waves[{index}].when"),
                    "only the first says `Start`",
                ));
            }
            if wave.spawners.is_empty() || wave.spawners.len() > MAX_WAVE_SPAWNS {
                let problem = format!("{} is out of range", wave.spawners.len());
                return Err(ZoneError::invalid(format!("waves[{index}].spawners"), problem));
            }
            for spawner in wave.spawners.iter() {
                if spawner.is_empty() || spawner.len() > MAX_NAME {
                    return Err(ZoneError::invalid(format!("waves[{index}].spawners"), "a bad name"));
                }
            }
        }
        Ok(())
    }

    /// Checks what it names against what a zone has, naming the field.
    pub fn check_in(&self, zone: &ArchivedZoneSimulation) -> Result<(), ZoneError> {
        if !zone.triggers.iter().any(|trigger| trigger.event == self.starts_on) {
            let problem = format!("no trigger of the zone fires `{}`", self.starts_on.as_str());
            return Err(ZoneError::invalid("starts_on", problem));
        }
        for (index, wave) in self.waves.iter().enumerate() {
            for (at, name) in wave.spawners.iter().enumerate() {
                if zone.spawner(Name::new(name.as_str())).is_none() {
                    let problem = format!("the zone has no spawner `{}`", name.as_str());
                    return Err(ZoneError::invalid(format!("waves[{index}].spawners[{at}]"), problem));
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const GOOD: &str = r#"(
        type: "sapphire::zone::EncounterDocument",
        version: 1,
        data: (
            starts_on: "gate_entered",
            faction: 2,
            tokens: 2,
            token_rest: 30,
            waves: [
                (when: Start, spawners: ["north", "south"]),
                (when: Remaining(1), gap: 20, spawners: ["north"]),
                (when: After(600), spawners: ["south"]),
            ],
        ),
    )"#;

    fn good() -> EncounterDocument {
        EncounterDocument::from_ron(GOOD).expect("good")
    }

    #[test]
    fn it_cooks_and_reads_back() {
        let cooked = good().cook().expect("cooks").to_cooked().expect("archives");
        let read = Encounter::read(&cooked).expect("reads");
        assert_eq!(read.tokens, 2);
        assert_eq!(read.token_rest.to_native(), 30);
        assert_eq!(read.waves.len(), 3);
        assert_eq!(read.waves[1].gap.to_native(), 20);
        assert!(matches!(read.waves[1].when, ArchivedWaveStart::Remaining(left) if left.to_native() == 1));
        assert_eq!(read.waves[0].spawners[1].as_str(), "south");
    }

    #[test]
    fn an_unknown_field_is_named() {
        let message = EncounterDocument::from_ron(&GOOD.replace("token_rest", "token_reset"))
            .expect_err("no such field")
            .to_string();
        assert!(message.contains("token_reset"), "{message}");
    }

    /// A field's name, and what spoils it.
    type Spoiled = (&'static str, fn(&mut EncounterDocument));

    #[test]
    fn a_bad_value_names_its_field() {
        let cases: [Spoiled; 9] = [
            ("starts_on", |d| d.starts_on.clear()),
            ("faction", |d| d.faction = 0),
            ("faction", |d| d.faction = MAX_FACTION + 1),
            ("tokens", |d| d.tokens = 0),
            ("tokens", |d| d.tokens = MAX_TOKENS + 1),
            ("waves", |d| d.waves.clear()),
            ("waves[0].when", |d| d.waves[0].when = WaveStart::After(1)),
            ("waves[2].when", |d| d.waves[2].when = WaveStart::Start),
            ("waves[1].spawners", |d| d.waves[1].spawners.clear()),
        ];
        for (field, spoil) in cases {
            let mut document = good();
            spoil(&mut document);
            let message = document.check().expect_err(field).to_string();
            assert!(message.contains(&format!("`{field}`")), "{field}: {message}");
        }
    }

    #[test]
    fn what_it_names_must_be_in_the_zone() {
        let zone = |event: &str, spawner: &str| ZoneSimulation {
            colliders: None,
            spawners: vec![crate::CookedSpawner {
                id: Name::new(spawner).as_u64(),
                name: spawner.to_owned(),
                position: [0.0; 3],
                yaw_degrees: 0.0,
                spawns: "warrior".to_owned(),
            }],
            triggers: vec![crate::CookedTrigger {
                id: 1,
                name: "gate".to_owned(),
                position: [0.0; 3],
                volume: crate::CookedVolume::Sphere { radius: 1.0 },
                event: event.to_owned(),
            }],
            navigation: None,
            encounters: Vec::new(),
        };
        let document = EncounterDocument {
            waves: vec![Wave { when: WaveStart::Start, gap: 0, spawners: vec!["north".to_owned()] }],
            ..good()
        };
        document.check_in(&zone("gate_entered", "north")).expect("it has them");
        let message = document.check_in(&zone("elsewhere", "north")).expect_err("no event").to_string();
        assert!(message.contains("`starts_on`"), "{message}");
        let message = document.check_in(&zone("gate_entered", "west")).expect_err("no spawner").to_string();
        assert!(message.contains("`waves[0].spawners[0]`"), "{message}");
    }

    /// One byte wrong anywhere is refused, by the envelope or by what the values say.
    #[test]
    fn damage_is_refused_by_the_envelope() {
        let cooked = good().cook().expect("cooks").to_cooked().expect("archives");
        let mut bytes = cooked.to_vec();
        let last = bytes.len() - 1;
        bytes[last] ^= 0xff;
        let mut aligned = AlignedVec::<16>::with_capacity(bytes.len());
        aligned.extend_from_slice(&bytes);
        assert!(Encounter::read(&aligned).is_err());
    }
}
