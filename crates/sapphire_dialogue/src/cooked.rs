//! The cooked dialogue (`.sdialogue`): an rkyv archive, which the runtime copies out once at load.
//!
//! Two halves, as a timeline has. [`DialogueSimulation`] is what a dialogue can do to the simulation: its
//! effects, each a list of flag writes, which a replay pins by [`DialogueSimulation::digest`] and records
//! by number when one's applied. [`DialoguePresentation`] is everything else: the graph, the keys a panel
//! shows, the trigger that opens it and the conditions' source. Reading a dialogue, asking its conditions
//! and walking its graph all leave the simulation as it was, so none of that half can move a checksum.
//!
//! [`Dialogue::read`] takes bytes that came from a pack, which needn't have come from our cooker:
//! bytecheck rules out a bad archive, and [`Dialogue::validate`] everything the walk would otherwise have
//! to trust.
//!
//! **No bytecode in the file (M3 Q2b):** the conditions ship as the Luau source the cook made of them, and
//! whoever asks them compiles that with the build's own pinned compiler as it binds them
//! (`sapphire_script::ScriptHost::bind_conditions`). Luau's loader trusts its bytecode's structure, so
//! bytes read from a file could only be run behind an `unsafe` promise about where they came from, and a
//! pack's hashes say only that it's the file that was written, not who wrote it. Compiling a few
//! conditions costs tens of microseconds, once per dialogue, so nothing here has to be trusted.

use bevy_reflect::TypePath;
use sapphire_core::Name;
use sapphire_reflect::Versioned;
use sapphire_serialize::cooked::AlignedVec;
use xxhash_rust::xxh3::Xxh3;

use crate::document::{
    DialogueError, MAX_ARMS, MAX_CHOICES, MAX_NODES, MAX_SOURCE, MAX_WRITES, check_cue, check_graph,
    check_shown_key,
};

/// Conventional file extension for cooked dialogues.
pub const DIALOGUE_EXTENSION: &str = "sdialogue";

/// A cooked dialogue. See the [module docs](self).
#[derive(Clone, Debug, PartialEq, Eq, TypePath, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
#[rkyv(derive(Debug))]
#[type_path = "sapphire::dialogue"]
pub struct Dialogue {
    pub simulation: DialogueSimulation,
    pub presentation: DialoguePresentation,
}

impl Versioned for Dialogue {
    /// 1: M3 Q2. 2: M3 Q2b's conditions as source, not bytecode, and the trigger that opens it.
    const VERSION: u32 = 2;
}

/// What a dialogue can do to the simulation.
#[derive(Clone, Debug, Default, PartialEq, Eq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
#[rkyv(derive(Debug))]
pub struct DialogueSimulation {
    /// By [`EffectId`](crate::EffectId): every `set` of the document that writes anything, nodes first to
    /// last and each node's own before its choices'.
    pub effects: Vec<Effect>,
}

/// One `set`: flag writes, applied in order.
#[derive(Clone, Debug, PartialEq, Eq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
#[rkyv(derive(Debug))]
pub struct Effect {
    pub writes: Vec<FlagWrite>,
}

/// A flag, by its name's hash, and the value it takes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
#[rkyv(derive(Debug))]
pub struct FlagWrite {
    pub flag: u64,
    pub value: i32,
}

/// What only presentation reads: the graph and what it shows.
#[derive(Clone, Debug, PartialEq, Eq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
#[rkyv(derive(Debug))]
pub struct DialoguePresentation {
    /// The node it starts at.
    pub start: u16,
    pub nodes: Vec<CookedNode>,
    /// How many functions the chunk's `conditions` holds, by [`ConditionId`](crate::ConditionId).
    pub conditions: u16,
    /// The conditions' chunk in Luau source, as [`DialogueDocument::conditions_source`] wrote it, or empty
    /// with no conditions: see the [module docs](self) for why it isn't bytecode.
    ///
    /// [`DialogueDocument::conditions_source`]: crate::DialogueDocument::conditions_source
    pub source: String,
    /// The hash of the trigger event that opens it, wherever a zone's trigger fires one: see
    /// [`DialogueDocument::opens_on`](crate::DialogueDocument::opens_on).
    pub opens_on: Option<u64>,
}

/// A node, as the document's, with names made places.
#[derive(Clone, Debug, PartialEq, Eq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
#[rkyv(derive(Debug))]
pub struct CookedNode {
    /// Its name's hash.
    pub name: u64,
    pub say: Option<CookedSay>,
    pub effect: Option<u16>,
    pub next: Option<u16>,
    pub choices: Vec<CookedChoice>,
    pub branch: Vec<CookedArm>,
    pub otherwise: Option<u16>,
}

#[derive(Clone, Debug, PartialEq, Eq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
#[rkyv(derive(Debug))]
pub struct CookedSay {
    pub speaker: String,
    pub line: String,
    pub vo: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
#[rkyv(derive(Debug))]
pub struct CookedChoice {
    pub line: String,
    pub to: Option<u16>,
    pub when: Option<u16>,
    pub effect: Option<u16>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
#[rkyv(derive(Debug))]
pub struct CookedArm {
    pub when: u16,
    pub to: u16,
}

impl CookedNode {
    /// Every node it can go to.
    pub(crate) fn leads_to(&self) -> impl Iterator<Item = u16> + '_ {
        let choices = self.choices.iter().filter_map(|choice| choice.to);
        let arms = self.branch.iter().map(|arm| arm.to);
        self.next.into_iter().chain(choices).chain(arms).chain(self.otherwise)
    }
}

/// One line, as Q3's subtitles and anything else outside a conversation read it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LineView<'a> {
    pub speaker: &'a str,
    pub line: &'a str,
    pub vo: Option<&'a str>,
}

impl Dialogue {
    /// The cooked file.
    pub fn to_cooked(&self) -> Result<AlignedVec, DialogueError> {
        Ok(sapphire_serialize::to_cooked(self)?)
    }

    /// Reads cooked bytes, checking the envelope, the archive and everything the walk relies on
    /// ([`Self::validate`]). What it allocates, the bytes paid for.
    pub fn read(bytes: &[u8]) -> Result<Self, DialogueError> {
        let dialogue = Self::from_archived(sapphire_serialize::access::<Self>(bytes)?);
        dialogue.validate()?;
        Ok(dialogue)
    }

    /// The dialogue, from where the asset server holds it.
    #[must_use]
    pub fn from_archived(archived: &ArchivedDialogue) -> Self {
        rkyv::deserialize::<Self, rkyv::rancor::Error>(archived)
            .expect("an archive bytecheck accepted deserializes: it holds only plain values and strings")
    }

    /// Whether asking its conditions takes the script VM: a dialogue with none can be walked in a game
    /// with no scripts.
    #[must_use]
    pub fn needs_scripts(&self) -> bool {
        self.presentation.conditions > 0
    }

    /// The line of the node named `name`, if it has one: the seam Q3's subtitles read.
    #[must_use]
    pub fn line(&self, name: Name) -> Option<LineView<'_>> {
        let node = self.presentation.nodes.iter().find(|node| node.name == name.as_u64())?;
        node.say.as_ref().map(CookedSay::view)
    }

    /// Everything the walk relies on without looking, for bytes that may be anyone's: what a document's
    /// checks hold it to, with places for names.
    pub fn validate(&self) -> Result<(), DialogueError> {
        self.simulation.validate()?;
        let presentation = &self.presentation;
        let nodes = &presentation.nodes;
        if nodes.is_empty() || nodes.len() > MAX_NODES {
            return Err(DialogueError::invalid(
                "nodes",
                format!("{} nodes, outside 1 to {MAX_NODES}", nodes.len()),
            ));
        }
        let node_count = nodes.len();
        let place = |field: &dyn Fn() -> String, at: u16| {
            if usize::from(at) < node_count {
                Ok(())
            } else {
                Err(DialogueError::invalid(field(), format!("node {at} is past the {node_count} there are")))
            }
        };
        place(&|| "start".to_owned(), presentation.start)?;
        if presentation.source.is_empty() != (presentation.conditions == 0) {
            return Err(DialogueError::invalid(
                "source",
                "a dialogue has its conditions' source exactly when it has conditions",
            ));
        }
        if presentation.source.len() > MAX_SOURCE {
            let problem = format!("{} bytes, past the limit of {MAX_SOURCE}", presentation.source.len());
            return Err(DialogueError::invalid("source", problem));
        }
        let effects = self.simulation.effects.len();
        let effect = |field: &dyn Fn() -> String, at: Option<u16>| match at {
            Some(at) if usize::from(at) >= effects => {
                Err(DialogueError::invalid(field(), format!("effect {at} is past the {effects} there are")))
            }
            _ => Ok(()),
        };
        let condition = |field: &dyn Fn() -> String, at: u16| {
            if at < presentation.conditions {
                Ok(())
            } else {
                let problem = format!("condition {at} is past the {} there are", presentation.conditions);
                Err(DialogueError::invalid(field(), problem))
            }
        };
        for (index, node) in nodes.iter().enumerate() {
            let at = |field: &str| format!("nodes[{index}].{field}");
            if let Some(say) = &node.say {
                check_shown_key(|| at("say.speaker"), &say.speaker)?;
                check_shown_key(|| at("say.line"), &say.line)?;
                if let Some(vo) = &say.vo {
                    check_cue(|| at("say.vo"), vo)?;
                }
                if !node.branch.is_empty() || node.otherwise.is_some() {
                    return Err(DialogueError::invalid(
                        at("branch"),
                        "a node that says a line doesn't branch",
                    ));
                }
                if node.next.is_some() && !node.choices.is_empty() {
                    return Err(DialogueError::invalid(
                        at("next"),
                        "a node with choices goes where they lead",
                    ));
                }
            } else if node.branch.is_empty()
                || node.otherwise.is_none()
                || node.next.is_some()
                || node.effect.is_some()
                || !node.choices.is_empty()
            {
                return Err(DialogueError::invalid(
                    at("say"),
                    "a node says a line, or it branches and nothing else",
                ));
            }
            effect(&|| at("effect"), node.effect)?;
            if let Some(next) = node.next {
                place(&|| at("next"), next)?;
            }
            if let Some(otherwise) = node.otherwise {
                place(&|| at("otherwise"), otherwise)?;
            }
            if node.choices.len() > MAX_CHOICES {
                return Err(DialogueError::invalid(
                    at("choices"),
                    format!("more than {MAX_CHOICES} choices"),
                ));
            }
            if !node.choices.is_empty() && node.choices.iter().all(|choice| choice.when.is_some()) {
                return Err(DialogueError::invalid(at("choices"), "one choice has no `when`"));
            }
            for (choice_index, choice) in node.choices.iter().enumerate() {
                let field = |part: &str| format!("nodes[{index}].choices[{choice_index}].{part}");
                check_shown_key(|| field("line"), &choice.line)?;
                if let Some(to) = choice.to {
                    place(&|| field("to"), to)?;
                }
                if let Some(when) = choice.when {
                    condition(&|| field("when"), when)?;
                }
                effect(&|| field("effect"), choice.effect)?;
            }
            if node.branch.len() > MAX_ARMS {
                return Err(DialogueError::invalid(at("branch"), format!("more than {MAX_ARMS} arms")));
            }
            for (arm_index, arm) in node.branch.iter().enumerate() {
                let field = |part: &str| format!("nodes[{index}].branch[{arm_index}].{part}");
                condition(&|| field("when"), arm.when)?;
                place(&|| field("to"), arm.to)?;
            }
        }
        check_graph(nodes, presentation.start, |index| format!("nodes[{index}]"))
    }
}

impl CookedSay {
    fn view(&self) -> LineView<'_> {
        LineView { speaker: &self.speaker, line: &self.line, vo: self.vo.as_deref() }
    }
}

impl DialogueSimulation {
    /// What a simulation relies on of a pack's bytes.
    pub fn validate(&self) -> Result<(), DialogueError> {
        if self.effects.len() > usize::from(u16::MAX) {
            return Err(DialogueError::invalid("effects", "more effects than an ID holds"));
        }
        for (index, effect) in self.effects.iter().enumerate() {
            if effect.writes.is_empty() || effect.writes.len() > MAX_WRITES {
                let problem = format!("{} writes, outside 1 to {MAX_WRITES}", effect.writes.len());
                return Err(DialogueError::invalid(format!("effects[{index}].writes"), problem));
            }
        }
        Ok(())
    }

    /// What a replay pins: every effect's writes, by value.
    #[must_use]
    pub fn digest(&self) -> u64 {
        let mut hasher = Xxh3::new();
        hasher.update(&(self.effects.len() as u64).to_le_bytes());
        for effect in &self.effects {
            hasher.update(&(effect.writes.len() as u64).to_le_bytes());
            for write in &effect.writes {
                hasher.update(&write.flag.to_le_bytes());
                hasher.update(&write.value.to_le_bytes());
            }
        }
        hasher.digest()
    }
}
