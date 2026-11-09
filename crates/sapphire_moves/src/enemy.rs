//! One kind of enemy (M3 B3): `<name>.enemy.ron`, cooked to `.senemy`.
//!
//! ```ron
//! ( type: "sapphire::moves::EnemyDocument", version: 1, data: ( // Its `.moves.ron`, by the ID in its
//! sidecar. moves: "1c2bf2a5-5d7e-4a43-bb4e-0b3a5b7f1d10", // How it fights, by name, and the one it starts
//! with. brains: { "steady": (spacing: (near: 1.4, far: 2.6), ...) }, brain: "steady", // Where its health's
//! phases fall, in thousandths of the most it has, first to last. A phase may // change its brain, and one
//! with a name says so to presentation as it's crossed. phases: [(at: 500, name: Some("enraged"), brain:
//! Some("fierce")), (at: 0)], boss: Some((name: "boss-warden")), look: (mesh: "...", graph: "...", scale:
//! 1.0, height: 2.0, eyes_below_crown: 0.2, framed_radius: 0.45), ), )
//! ```
//!
//! **Two halves.** [`Enemy::mind`] is the simulation's: its brains, compiled against its set of moves, its
//! phases, and whether it's a boss, by its name's key. A simulation takes it beside that set
//! (`sapphire_gameplay`'s `Simulation::add_enemy`) and a replay's `tables_hash` holds it, so a HUD reads a
//! boss's bar, phases and name from the simulation alone. [`Enemy::look`] is the stage's: what draws one and
//! how it's framed, none of which a step reads.
//!
//! **Brains are compiled here, not in the moves:** the same moves can fight two ways, as two kinds of enemy,
//! and a boss's phases choose among its own. Each brain is checked against the set's document, naming the
//! brain ([`MoveSetDocument::compile_brain`]), so the cook needs the moves document as an input.
//!
//! **A phase** is health at or below `at` thousandths of the most: `at` falls strictly from first to last,
//! below 1000, and the n-th crossed is phase n, as `on_phase` numbers them. [`phase_of`] is the one rule.

use std::collections::{BTreeMap, BTreeSet};

use bevy_reflect::{Reflect, TypePath};
use sapphire_core::Name;
use sapphire_reflect::Versioned;
use sapphire_serialize::cooked::{self, AlignedVec};
use sapphire_serialize::{AssetId, CookedError, DocumentError};
use serde::{Deserialize, Serialize};

use crate::brain::BrainDef;
use crate::document::{BrainDocument, MoveSetDocument};
use crate::set::{MoveSet, MoveSetError};

/// What a cooked enemy's file name ends with.
pub const ENEMY_EXTENSION: &str = "senemy";

/// Most brains one kind of enemy has: a fighter's choice is a `u8`, and a handful is plenty.
pub const MAX_BRAINS: usize = 8;

/// Most phases one kind of enemy's health has.
pub const MAX_ENEMY_PHASES: usize = 8;

/// What a phase's `at` counts in: thousandths of the most health.
pub const PER_MILLE: u16 = 1000;

/// The phases of a fighter no document gives any: its health's quarters, as M3 L2's `on_phase` began.
pub const QUARTERS: [u16; 4] = [750, 500, 250, 0];

/// How many of `thresholds`, first to last, health is at or below, as thousandths of `max`: the phase it's
/// in, 0 being none crossed. `None` for a fighter with no health to fall.
#[must_use]
pub fn phase_of(thresholds: impl IntoIterator<Item = u16>, health: u32, max: u32) -> Option<u8> {
    if max == 0 {
        return None;
    }
    let health = u64::from(health.min(max)) * u64::from(PER_MILLE);
    let crossed = thresholds.into_iter().take_while(|&at| health <= u64::from(at) * u64::from(max)).count();
    Some(u8::try_from(crossed).unwrap_or(u8::MAX))
}

/// An enemy document, `.enemy.ron`: one kind of enemy, which compiles to an [`Enemy`].
#[derive(Reflect, Clone, Debug, PartialEq, Serialize, Deserialize)]
#[type_path = "sapphire::moves"]
#[serde(deny_unknown_fields)]
pub struct EnemyDocument {
    /// Its set of moves: the `.moves.ron`'s ID.
    pub moves: AssetId,
    /// How it fights, by name.
    pub brains: BTreeMap<String, BrainDocument>,
    /// The brain each one starts with.
    pub brain: String,
    /// Where its health's phases fall, first to last. None: it has no phases at all.
    pub phases: Vec<EnemyPhaseDocument>,
    /// It's a boss: a boss bar shows it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub boss: Option<BossDocument>,
    pub look: EnemyLookDocument,
    /// Its script (M3 C2): a `.luau`'s ID, which a fight with this kind in it binds, as a boss's own way of
    /// fighting that a brain can't say. Scenario documents read it from here. The cooked kind doesn't hold
    /// it, so nothing cooked changes, and a game that reads kinds from packs needs it added there.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub script: Option<AssetId>,
}

impl Versioned for EnemyDocument {
    /// 1: M3 B3.
    const VERSION: u32 = 1;
}

/// One phase of an enemy's health.
#[derive(Reflect, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EnemyPhaseDocument {
    /// Health at or below this many thousandths of the most is this phase.
    pub at: u16,
    /// What presentation hears as the phase is crossed: a cue of this name, from the fighter.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// The brain it fights with from here, one of `brains`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub brain: Option<String>,
}

/// What makes an enemy a boss.
#[derive(Reflect, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BossDocument {
    /// The key of its name in the game's words, a string table's message, for the boss bar.
    pub name: String,
}

/// What draws one. Its material comes with its mesh.
#[derive(Reflect, Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EnemyLookDocument {
    /// A skinned mesh, on a skeleton with the joint names the stage poses by.
    pub mesh: AssetId,
    /// Its animation graph: the clips it plays.
    pub graph: AssetId,
    /// What it holds that isn't skinned in, if anything: a mesh drawn at one of its joints.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub held: Option<HeldDocument>,
    /// How much bigger than its mesh it's drawn.
    pub scale: f32,
    /// How high its crown stands over its feet, in metres, at a scale of 1.
    pub height: f32,
    /// How far under its crown, as drawn, its eyes are, in metres: what a foe looks at.
    pub eyes_below_crown: f32,
    /// How wide a camera frames it, in metres from its middle.
    pub framed_radius: f32,
}

/// A mesh held at a joint.
#[derive(Reflect, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HeldDocument {
    pub mesh: AssetId,
    pub joint: String,
}

/// An enemy that can't be read, compiled or cooked.
#[derive(Debug, thiserror::Error)]
pub enum EnemyError {
    /// Boxed: RON's errors are large.
    #[error(transparent)]
    Document(Box<DocumentError>),
    #[error("`{field}`: {problem}")]
    Invalid { field: String, problem: String },
    #[error("brain `{name}`: {source}")]
    Brain {
        name: String,
        #[source]
        source: MoveSetError,
    },
    #[error(transparent)]
    Cooked(#[from] CookedError),
    /// Cooked data that isn't what any document compiles to.
    #[error("cooked enemy is damaged: {0}")]
    Damaged(String),
}

fn invalid(field: impl Into<String>, problem: impl Into<String>) -> EnemyError {
    EnemyError::Invalid { field: field.into(), problem: problem.into() }
}

impl EnemyDocument {
    /// Reads a document and checks what it can say by itself.
    pub fn parse(text: &str) -> Result<Self, EnemyError> {
        let document: Self =
            sapphire_serialize::from_ron(text).map_err(|error| EnemyError::Document(Box::new(error)))?;
        document.check()?;
        Ok(document)
    }

    /// Checks everything that needs nothing outside the document, naming the field.
    pub fn check(&self) -> Result<(), EnemyError> {
        if self.brains.is_empty() {
            return Err(invalid("brains", "an enemy needs a brain"));
        }
        if self.brains.len() > MAX_BRAINS {
            return Err(invalid(
                "brains",
                format!("{} brains, past the limit of {MAX_BRAINS}", self.brains.len()),
            ));
        }
        if let Some(name) = self.brains.keys().find(|name| name.is_empty()) {
            return Err(invalid("brains", format!("`{name}`: a name is empty")));
        }
        if !self.brains.contains_key(&self.brain) {
            return Err(invalid("brain", format!("`{}` isn't one of `brains`", self.brain)));
        }
        if self.phases.len() > MAX_ENEMY_PHASES {
            let count = self.phases.len();
            return Err(invalid("phases", format!("{count} phases, past the limit of {MAX_ENEMY_PHASES}")));
        }
        let mut names = BTreeSet::new();
        let mut above = PER_MILLE;
        for (index, phase) in self.phases.iter().enumerate() {
            let field = || format!("phases[{index}]");
            if phase.at >= above {
                let problem = format!("at {} isn't below {above}: phases fall from first to last", phase.at);
                return Err(invalid(field(), problem));
            }
            above = phase.at;
            if let Some(name) = &phase.name
                && (name.is_empty() || !names.insert(name.as_str()))
            {
                return Err(invalid(field(), format!("`{name}` is empty or another phase's")));
            }
            if let Some(brain) = &phase.brain
                && !self.brains.contains_key(brain)
            {
                return Err(invalid(field(), format!("`{brain}` isn't one of `brains`")));
            }
        }
        if self.boss.as_ref().is_some_and(|boss| boss.name.is_empty()) {
            return Err(invalid("boss.name", "a name is empty"));
        }
        let look = &self.look;
        for (what, value) in [
            ("scale", look.scale),
            ("height", look.height),
            ("eyes_below_crown", look.eyes_below_crown),
            ("framed_radius", look.framed_radius),
        ] {
            let least = if what == "eyes_below_crown" { 0.0 } else { f32::MIN_POSITIVE };
            if !(value.is_finite() && value >= least) {
                return Err(invalid(format!("look.{what}"), format!("{value} isn't a length")));
            }
        }
        if look.eyes_below_crown >= look.height * look.scale {
            return Err(invalid("look.eyes_below_crown", "its eyes would be under its feet"));
        }
        if look.held.as_ref().is_some_and(|held| held.joint.is_empty()) {
            return Err(invalid("look.held.joint", "a name is empty"));
        }
        Ok(())
    }

    /// Compiles it against its set of moves' document, which must be the one `moves` names: each brain
    /// resolves against that set, the one it starts with first.
    pub fn compile(&self, moves: &MoveSetDocument) -> Result<Enemy, EnemyError> {
        self.check()?;
        let order =
            std::iter::once(&self.brain).chain(self.brains.keys().filter(|name| **name != self.brain));
        let mut brains = Vec::with_capacity(self.brains.len());
        for name in order {
            let document = &self.brains[name];
            let def = moves
                .compile_brain(document)
                .map_err(|source| EnemyError::Brain { name: name.clone(), source })?;
            let presses =
                document.attacks.iter().map(|attack| Name::intern(&attack.intent).as_u64()).collect();
            let rises = document.rise.as_ref().map(|rise| Name::intern(&rise.intent).as_u64());
            brains.push(NamedBrain { name: Name::intern(name).as_u64(), def, presses, rises });
        }
        let place = |brain: &str| brains.iter().position(|named| named.name == Name::new(brain).as_u64());
        let phases = self.phases.iter().map(|phase| EnemyPhase {
            at: phase.at,
            name: phase.name.as_deref().map(|name| Name::intern(name).as_u64()),
            brain: phase.brain.as_deref().and_then(place).map(|at| at as u8),
        });
        let phases = phases.collect();
        let look = &self.look;
        Ok(Enemy {
            moves: self.moves,
            mind: EnemyMind { brains, phases, boss: self.boss.as_ref().map(|boss| boss.name.clone()) },
            look: EnemyLook {
                mesh: look.mesh,
                graph: look.graph,
                held: look.held.as_ref().map(|held| HeldLook { mesh: held.mesh, joint: held.joint.clone() }),
                scale: look.scale,
                height: look.height,
                eyes_below_crown: look.eyes_below_crown,
                framed_radius: look.framed_radius,
            },
        })
    }
}

/// A brain, by its name's hash, with the names of the intents it presses, by which it's fitted to a set.
#[derive(Clone, Debug, PartialEq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub struct NamedBrain {
    name: u64,
    pub def: BrainDef,
    /// Each attack's intent's name, in [`BrainDef::attacks`]' order.
    presses: Vec<u64>,
    /// Its rise's intent's name.
    rises: Option<u64>,
}

impl NamedBrain {
    #[must_use]
    pub const fn name(&self) -> Name {
        Name::from_u64(self.name)
    }
}

/// One phase, cooked.
#[derive(Clone, Copy, Debug, PartialEq, Eq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub struct EnemyPhase {
    /// Health at or below this many thousandths of the most.
    pub at: u16,
    name: Option<u64>,
    /// The brain it fights with from here, as its place in [`EnemyMind::brains`].
    pub brain: Option<u8>,
}

impl EnemyPhase {
    /// The cue presentation hears as it's crossed, if it has a name.
    #[must_use]
    pub fn name(&self) -> Option<Name> {
        self.name.map(Name::from_u64)
    }
}

/// The simulation's half of an enemy: how it fights, where its phases fall and whether it's a boss.
#[derive(Clone, Debug, PartialEq, TypePath, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
#[type_path = "sapphire::moves"]
pub struct EnemyMind {
    /// The first is what each one starts with.
    brains: Vec<NamedBrain>,
    phases: Vec<EnemyPhase>,
    /// Its name's key in the game's words, if it's a boss.
    boss: Option<String>,
}

impl Versioned for EnemyMind {
    /// 1: M3 B3.
    const VERSION: u32 = 1;
}

impl EnemyMind {
    /// Its brains: the first is what each one starts with, and a fighter's choice is a place in these.
    #[must_use]
    pub fn brains(&self) -> &[NamedBrain] {
        &self.brains
    }

    /// The brain at a place in [`Self::brains`].
    #[must_use]
    pub fn brain(&self, place: u8) -> Option<&BrainDef> {
        self.brains.get(usize::from(place)).map(|named| &named.def)
    }

    /// The place of the brain with this name.
    #[must_use]
    pub fn brain_named(&self, name: Name) -> Option<u8> {
        self.brains.iter().position(|named| named.name() == name).map(|place| place as u8)
    }

    /// Its phases, first to last.
    #[must_use]
    pub fn phases(&self) -> &[EnemyPhase] {
        &self.phases
    }

    /// The key of its name in the game's words (a string table's message), if it's a boss: what a boss bar
    /// names over its phases.
    #[must_use]
    pub fn boss(&self) -> Option<&str> {
        self.boss.as_deref()
    }

    /// Its bytes, for a replay's `tables_hash`: the same on every target.
    pub fn to_cooked(&self) -> Result<AlignedVec, CookedError> {
        cooked::to_cooked(self)
    }

    /// Its brains fitted to `set`: each intent it presses found in the set by its name, so a set it didn't
    /// compile against, or a new revision of the one it did whose intents moved, presses what was meant, and
    /// each brain held to [`MoveSet::check_brain`]. What a simulation runs.
    pub fn fitted_to(&self, set: &MoveSet) -> Result<Self, EnemyError> {
        let mut fitted = self.clone();
        for named in &mut fitted.brains {
            let brain = named.name();
            let place = |name: u64| {
                let place = set.intent(Name::from_u64(name)).map(|(place, _)| place as u32);
                place.ok_or_else(|| EnemyError::Brain {
                    name: brain.to_string(),
                    source: MoveSetError::Damaged(format!(
                        "it presses {}, which the set hasn't got",
                        Name::from_u64(name)
                    )),
                })
            };
            for (attack, &name) in named.def.attacks.iter_mut().zip(&named.presses) {
                attack.repoint(place(name)?);
            }
            if let (Some(rise), Some(name)) = (named.def.rise.as_mut(), named.rises) {
                rise.repoint(place(name)?);
            }
            set.check_brain(&named.def)
                .map_err(|source| EnemyError::Brain { name: brain.to_string(), source })?;
        }
        Ok(fitted)
    }

    /// What it relies on of itself, for bytes from anywhere.
    fn check(&self) -> Result<(), EnemyError> {
        let damaged = |what: &str| Err(EnemyError::Damaged(what.to_owned()));
        if self.brains.is_empty() || self.brains.len() > MAX_BRAINS {
            return damaged("its brains aren't 1 to MAX_BRAINS");
        }
        let names: BTreeSet<u64> = self.brains.iter().map(|named| named.name).collect();
        if names.len() != self.brains.len() {
            return damaged("two brains have one name");
        }
        let named_whole = |named: &NamedBrain| {
            named.presses.len() == named.def.attacks.len()
                && named.rises.is_some() == named.def.rise.is_some()
        };
        if !self.brains.iter().all(named_whole) {
            return damaged("a brain's intents aren't all named");
        }
        if self.phases.len() > MAX_ENEMY_PHASES {
            return damaged("more phases than MAX_ENEMY_PHASES");
        }
        let mut above = PER_MILLE;
        for phase in &self.phases {
            if phase.at >= above {
                return damaged("its phases don't fall from first to last");
            }
            above = phase.at;
            if phase.brain.is_some_and(|place| usize::from(place) >= self.brains.len()) {
                return damaged("a phase names no brain");
            }
        }
        Ok(())
    }
}

/// What it holds that isn't skinned in.
#[derive(Clone, Debug, PartialEq, Eq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub struct HeldLook {
    pub mesh: AssetId,
    pub joint: String,
}

/// The stage's half of an enemy. See [`EnemyLookDocument`] for each.
#[derive(Clone, Debug, PartialEq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub struct EnemyLook {
    pub mesh: AssetId,
    pub graph: AssetId,
    pub held: Option<HeldLook>,
    pub scale: f32,
    pub height: f32,
    pub eyes_below_crown: f32,
    pub framed_radius: f32,
}

impl EnemyLook {
    /// How high its crown stands over its feet as drawn, in metres.
    #[must_use]
    pub fn crown(&self) -> f32 {
        self.height * self.scale
    }

    /// How high its eyes stand over its feet as drawn, in metres.
    #[must_use]
    pub fn eyes(&self) -> f32 {
        self.crown() - self.eyes_below_crown
    }
}

/// One kind of enemy, cooked (`.senemy`): its [`EnemyMind`] for the simulation and its [`EnemyLook`] for
/// the stage.
#[derive(Clone, Debug, PartialEq, TypePath, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
#[type_path = "sapphire::moves"]
pub struct Enemy {
    /// Its set of moves.
    pub moves: AssetId,
    pub mind: EnemyMind,
    pub look: EnemyLook,
}

impl Versioned for Enemy {
    /// 1: M3 B3.
    const VERSION: u32 = 1;
}

impl Enemy {
    /// Writes it: the same bytes for the same enemy, every time.
    pub fn to_cooked(&self) -> Result<AlignedVec, EnemyError> {
        Ok(cooked::to_cooked(self)?)
    }

    /// Reads a cooked enemy, whose bytes must start 16-byte aligned, and checks what its mind relies on of
    /// itself. Whether its brains fit its set is [`EnemyMind::fitted_to`]'s, once the set is loaded.
    pub fn from_cooked(bytes: &[u8]) -> Result<Self, EnemyError> {
        let archived = cooked::access::<Self>(bytes)?;
        let enemy = rkyv::deserialize::<Self, rkyv::rancor::Error>(archived).map_err(CookedError::from)?;
        enemy.mind.check()?;
        let look = &enemy.look;
        let lengths = [look.scale, look.height, look.framed_radius];
        if !lengths.iter().all(|length| length.is_finite() && *length > 0.0)
            || !(look.eyes_below_crown.is_finite() && (0.0..look.crown()).contains(&look.eyes_below_crown))
        {
            return Err(EnemyError::Damaged("its look's lengths aren't lengths".to_owned()));
        }
        Ok(enemy)
    }
}

impl ArchivedEnemy {
    /// Its look, out of the archive: what a stage keeps while it draws one. The archive was checked when
    /// it was read ([`Enemy::from_cooked`]).
    #[must_use]
    pub fn look(&self) -> EnemyLook {
        rkyv::deserialize::<EnemyLook, rkyv::rancor::Error>(&self.look)
            .expect("a checked look deserializes: nothing in it can fail")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SLICE: &str = include_str!("../test_content/slice.moves.ron");
    const ENEMY: &str = include_str!("../test_content/slice.enemy.ron");

    fn moves() -> MoveSetDocument {
        MoveSetDocument::parse(SLICE).expect("parses")
    }

    fn document() -> EnemyDocument {
        EnemyDocument::parse(ENEMY).expect("parses")
    }

    #[test]
    fn it_compiles_with_the_brain_it_starts_with_first_and_cooks_the_same_bytes() {
        let enemy = document().compile(&moves()).expect("compiles");
        let set = moves().compile().expect("compiles");
        assert_eq!(enemy.mind.fitted_to(&set).expect("its brains press the set's intents"), enemy.mind);
        assert_eq!(enemy.mind.brains()[0].name(), Name::new("steady"));
        assert_eq!(enemy.mind.brain_named(Name::new("fierce")), Some(1));
        let phase = enemy.mind.phases()[1];
        assert_eq!((phase.at, phase.name(), phase.brain), (500, Some(Name::new("enraged")), Some(1)));
        let cooked = enemy.to_cooked().expect("cooks");
        assert_eq!(Enemy::from_cooked(&cooked).expect("reads"), enemy);
        assert_eq!(cooked.as_slice(), document().compile(&moves()).unwrap().to_cooked().unwrap().as_slice());
        assert_eq!(enemy.mind.boss(), Some("boss-slice"));
    }

    /// A kind's script is for whoever sets up its fights: the document reads and writes it, and the kind it
    /// compiles to cooks the same bytes with it as without.
    #[test]
    fn a_kinds_script_reads_and_leaves_its_cooked_bytes_alone() {
        let script = "a1ff800a-047f-4375-ac6c-aec395df870d";
        let text =
            ENEMY.replacen("        look: (", &format!("        script: \"{script}\",\n        look: ("), 1);
        assert_ne!(text, ENEMY, "the test's kind has a look to put it before");
        let with = EnemyDocument::parse(&text).expect("parses");
        assert_eq!(with.script.map(|id| id.to_string()), Some(script.to_owned()));
        assert_eq!(document().script, None);
        let cooked = |document: &EnemyDocument| document.compile(&moves()).unwrap().to_cooked().unwrap();
        assert_eq!(cooked(&with).as_slice(), cooked(&document()).as_slice());
    }

    /// A brain presses intents by name, so a set whose intents moved is pressed as meant, and one without an
    /// intent it presses can't have it.
    #[test]
    fn a_mind_fits_a_set_by_its_intents_names() {
        let enemy = document().compile(&moves()).expect("compiles");
        let mut moved = moves();
        moved.intents.reverse();
        let set = moved.compile().expect("compiles");
        let fitted = enemy.mind.fitted_to(&set).expect("the same names");
        let light = set.intent(Name::new("light")).expect("declared").0 as u32;
        assert_ne!(enemy.mind.brain(0).unwrap().attacks[0].intent(), light, "the test needs it to move");
        assert_eq!(fitted.brain(0).unwrap().attacks[0].intent(), light);
        assert_eq!(fitted.brain(0).unwrap().rise.unwrap().intent(), light);

        let mut ghost = enemy.mind.clone();
        ghost.brains[1].presses[0] = Name::new("ghost").as_u64();
        let error = ghost.fitted_to(&set).expect_err("no such intent").to_string();
        assert!(error.contains("hasn't got"), "{error}");
    }

    /// A phase is health at or below its share, so L2's quarters come out as its formula had them.
    #[test]
    fn phases_are_crossed_at_or_below_their_share() {
        for max in [1, 3, 100, 997, 1_000_000, u32::MAX] {
            for health in [0, 1, max / 4, max / 2, max / 2 + 1, max - max / 4, max - 1, max] {
                let quarters = ((u64::from(max) - u64::from(health)) * 4 / u64::from(max)) as u8;
                assert_eq!(phase_of(QUARTERS, health, max), Some(quarters), "{health} of {max}");
            }
        }
        assert_eq!(phase_of(QUARTERS, 0, 0), None, "no health, no phases");
        assert_eq!(phase_of([], 0, 10), Some(0));
    }

    /// One bad document per check: the message names the field.
    #[test]
    fn a_bad_document_names_the_field() {
        type Spoil = fn(&mut EnemyDocument);
        let cases: &[(&str, Spoil)] = &[
            ("`brains`", |d| d.brains.clear()),
            ("`brain`", |d| d.brain = "ghost".to_owned()),
            ("`phases[1]`", |d| d.phases[1].at = 900),
            ("`phases[1]`", |d| d.phases[1].brain = Some("ghost".to_owned())),
            ("`phases[2]`", |d| d.phases[2].name = d.phases[1].name.clone()),
            ("`phases[0]`", |d| d.phases[0].at = PER_MILLE),
            ("`boss.name`", |d| d.boss.as_mut().unwrap().name.clear()),
            ("`look.scale`", |d| d.look.scale = 0.0),
            ("`look.height`", |d| d.look.height = f32::NAN),
            ("`look.eyes_below_crown`", |d| d.look.eyes_below_crown = 9.0),
        ];
        for (field, spoil) in cases {
            let mut document = document();
            spoil(&mut document);
            let error = document.check().expect_err(field).to_string();
            assert!(error.contains(field), "{field}: {error}");
        }
        let mut document = document();
        document.brains.get_mut("fierce").unwrap().attacks[0].intent = "ghost".to_owned();
        let error = document.compile(&moves()).expect_err("no such intent").to_string();
        assert!(error.contains("brain `fierce`") && error.contains("`ghost`"), "{error}");
    }

    #[test]
    fn a_field_nobody_knows_is_refused() {
        let error =
            EnemyDocument::parse(&ENEMY.replacen("brain: \"steady\",", "brain: \"steady\", tokens: 2,", 1))
                .expect_err("no such field");
        assert!(error.to_string().contains("tokens"), "{error}");
    }

    /// Bytes from anywhere: what no document makes is refused by name.
    #[test]
    fn a_damaged_mind_is_refused() {
        type Damage = fn(&mut Enemy);
        let good = document().compile(&moves()).expect("compiles");
        let rows: &[(&str, Damage)] = &[
            ("no brains", |e| e.mind.brains.clear()),
            ("a phase naming no brain", |e| e.mind.phases[0].brain = Some(9)),
            ("phases rising", |e| e.mind.phases[1].at = 999),
            ("an attack's intent unnamed", |e| e.mind.brains[0].presses.clear()),
            ("two brains of a name", |e| {
                let copy = e.mind.brains[0].clone();
                e.mind.brains[1] = copy;
            }),
            ("a scale of nothing", |e| e.look.scale = 0.0),
        ];
        for (what, damage) in rows {
            let mut enemy = good.clone();
            damage(&mut enemy);
            let cooked = enemy.to_cooked().expect(what);
            assert!(matches!(Enemy::from_cooked(&cooked), Err(EnemyError::Damaged(_))), "{what}");
        }
    }
}
