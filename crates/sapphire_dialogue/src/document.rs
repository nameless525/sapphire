//! The dialogue document (`.dialogue.ron`): a conversation as a graph of nodes, written by hand or by a
//! tool.
//!
//! ```ron
//! (
//!     type: "sapphire::dialogue::DialogueDocument",
//!     version: 2,
//!     data: (
//!         start: "greet",
//!         // A zone's trigger whose event this is opens it, as someone goes in. Without, nothing does.
//!         opens_on: "court_entered",
//!         nodes: [
//!             // A line, and the node after it. Keys are `sapphire_loc`'s, never text, and under
//!             // `dialogue-`, which is what a panel reads.
//!             (name: "greet", say: (speaker: "dialogue-guide-name", line: "dialogue-guide-greet",
//!              vo: "guide_greet"), next: "ask"),
//!             // A line and up to four choices. Each shows only while its `when` holds, and one of them
//!             // has no `when`, so a player is never left with nothing to pick.
//!             (name: "ask", say: (speaker: "dialogue-guide-name", line: "dialogue-guide-ask"), choices: [
//!                 (line: "dialogue-light-ward", to: "lit", set: [("ward_lit", 1)]),
//!                 (line: "dialogue-ask-again", to: "again", when: "flag.ward_lit >= 1"),
//!                 // No `to`: the conversation ends.
//!                 (line: "dialogue-leave"),
//!             ]),
//!             // Saying a line can set flags too, as it's shown, and faces (version 2, M5 C5): a role's
//!             // expression preset, as a timeline's face key has it, held until a line sets that role's
//!             // again or the conversation ends.
//!             (name: "lit", say: (speaker: "dialogue-guide-name", line: "dialogue-guide-thanks",
//!              faces: [(role: "hero", expression: "happy", weight: 0.7)]), set: [("met_guide", 1)]),
//!             // A branch says nothing: it goes to the first arm whose `when` holds, or `otherwise`.
//!             (name: "again", branch: [(when: "flag.met_guide == 0", to: "greet")], otherwise: "lit"),
//!         ],
//!     ),
//! )
//! ```
//!
//! [`DialogueDocument::check`] names the field of everything wrong (`nodes[1].choices[2].when`), and
//! unknown fields are errors. A condition is the small language in [`crate::condition`]. Flags are
//! [`FlagRegistry`]'s: a document naming one it doesn't list doesn't cook.

use std::collections::BTreeMap;
use std::fmt::Write as _;

use bevy_reflect::Reflect;
use sapphire_core::Name;
use sapphire_reflect::{Decoder, VersionError, Versioned};
use sapphire_serialize::{CookedError, DocumentError};
use serde::{Deserialize, Serialize};

use crate::condition::Condition;
use crate::cooked::{
    CookedArm, CookedChoice, CookedFace, CookedNode, CookedSay, Dialogue, DialoguePresentation,
    DialogueSimulation, Effect, FlagWrite,
};
use crate::flags::FlagRegistry;

/// The most nodes a dialogue may hold: past any conversation, and a node's place fits a `u16`.
pub const MAX_NODES: usize = 4096;
/// The most choices a node offers: what a panel shows at once.
pub const MAX_CHOICES: usize = 4;
/// The most arms a branch has.
pub const MAX_ARMS: usize = 16;
/// The most flags one `set` writes.
pub const MAX_WRITES: usize = 16;
/// The longest node's or flag's name, in bytes.
pub const MAX_NAME: usize = 64;
/// The longest key or voice cue, in bytes.
pub const MAX_KEY: usize = 256;
/// The longest conditions' source a cooked dialogue holds, in bytes: far past what [`MAX_NODES`] nodes'
/// conditions could ask, and what a load compiles.
pub const MAX_SOURCE: usize = 1 << 20;
/// What every key a dialogue shows starts with: what the panel's document reads by (`key_from`'s `prefix`
/// in `content/ui/dialogue.ui.ron`), and so what a UI cook holds to the panel's fonts and variables.
pub const KEY_PREFIX: &str = "dialogue-";

/// A dialogue document: see the [module docs](self).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Reflect)]
#[type_path = "sapphire::dialogue"]
#[serde(deny_unknown_fields)]
pub struct DialogueDocument {
    /// The node it starts at.
    pub start: String,
    /// The event of the trigger that opens it, in whichever zone has one: what a game listens for to start
    /// the conversation as someone walks in. A dialogue without is opened by whatever asks for it by name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub opens_on: Option<String>,
    pub nodes: Vec<NodeDocument>,
}

impl Versioned for DialogueDocument {
    /// 1: M3 Q2, with `opens_on` since M3 Q2b, which a document without needn't say. 2: M5 C5's faces on a
    /// line, which default to none, so a version 1 document decodes as it is.
    const VERSION: u32 = 2;

    fn upgrade<D: Decoder>(version: u32, data: D) -> Result<Self, VersionError<D::Error>> {
        match version {
            1 => data.decode().map_err(VersionError::Decode),
            found => Err(VersionError::TooOld { found, oldest: 1 }),
        }
    }
}

/// A node: a line, with a `next` or `choices` or neither, or a branch.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Reflect)]
#[type_path = "sapphire::dialogue"]
#[serde(deny_unknown_fields)]
pub struct NodeDocument {
    /// What `next`, `to` and `otherwise` call it, and what [`Dialogue::line`] finds it by.
    pub name: String,
    /// The line it shows. A branch has none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub say: Option<SayDocument>,
    /// Flags it sets as its line is shown.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub set: Vec<(String, i32)>,
    /// Where it goes once its line is read. None ends the conversation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub choices: Vec<ChoiceDocument>,
    /// A branch's arms, in the order they're tried.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub branch: Vec<BranchArm>,
    /// Where a branch goes when no arm holds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub otherwise: Option<String>,
}

/// Who says what, by `sapphire_loc`'s keys.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Reflect)]
#[type_path = "sapphire::dialogue"]
#[serde(deny_unknown_fields)]
pub struct SayDocument {
    pub speaker: String,
    pub line: String,
    /// A voice line's cue, by name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vo: Option<String>,
    /// Faces the line sets as it's shown (M5 C5), each held until a later line sets that role's again or
    /// the conversation ends.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub faces: Vec<SayFace>,
}

/// An expression a line sets on a character's face, as a timeline's face key does.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Reflect)]
#[type_path = "sapphire::dialogue"]
#[serde(deny_unknown_fields)]
pub struct SayFace {
    /// Who, as a timeline's track names them: `hero`, `foe`, or a kind of enemy by its name.
    pub role: String,
    /// A VRM 1.0 expression preset, by its name (`happy`, `sad`). The cook refuses any other.
    pub expression: String,
    /// 0 to 1: 1 with none.
    #[serde(default = "full")]
    pub weight: f32,
    /// Seconds from the face before; [`DEFAULT_FACE_BLEND`] with none.
    #[serde(default = "default_face_blend")]
    pub blend: f32,
}

/// How long a face takes to change when a line doesn't say, in seconds: a timeline's.
pub const DEFAULT_FACE_BLEND: f32 = 0.2;
/// The most faces one line sets.
pub const MAX_FACES: usize = 4;
/// The longest blend, in seconds.
pub const MAX_FACE_BLEND: f32 = 60.0;

fn full() -> f32 {
    1.0
}

fn default_face_blend() -> f32 {
    DEFAULT_FACE_BLEND
}

/// One of a node's choices.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Reflect)]
#[type_path = "sapphire::dialogue"]
#[serde(deny_unknown_fields)]
pub struct ChoiceDocument {
    /// What the choice says: a key.
    pub line: String,
    /// The node it leads to. None ends the conversation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub to: Option<String>,
    /// It shows only while this holds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub when: Option<String>,
    /// Flags it sets when it's chosen.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub set: Vec<(String, i32)>,
}

/// One arm of a branch.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Reflect)]
#[type_path = "sapphire::dialogue"]
#[serde(deny_unknown_fields)]
pub struct BranchArm {
    pub when: String,
    pub to: String,
}

/// A dialogue or flag registry that can't cook, or a cooked dialogue the runtime refuses.
#[derive(Debug, thiserror::Error)]
pub enum DialogueError {
    #[error(transparent)]
    Document(#[from] Box<DocumentError>),
    #[error(transparent)]
    Cooked(#[from] CookedError),
    #[error("`{field}`: {problem}")]
    Invalid { field: String, problem: String },
    /// The conditions' chunk didn't compile: the cook's compiler said why.
    #[error("the conditions don't compile: {0}")]
    Compile(String),
}

impl DialogueError {
    pub(crate) fn invalid(field: impl Into<String>, problem: impl Into<String>) -> Self {
        Self::Invalid { field: field.into(), problem: problem.into() }
    }
}

/// A node's or a flag's name: what a condition can say after `flag.`, and short.
pub(crate) fn check_name(field: impl FnOnce() -> String, name: &str) -> Result<(), DialogueError> {
    let mut bytes = name.bytes();
    let good = (1..=MAX_NAME).contains(&name.len())
        && bytes.next().is_some_and(|first| first.is_ascii_lowercase())
        && bytes.all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_');
    if good {
        Ok(())
    } else {
        let problem =
            format!("{name:?} isn't a name: 1 to {MAX_NAME} of a-z, 0-9 and _, starting with a letter");
        Err(DialogueError::invalid(field(), problem))
    }
}

/// A key `sapphire_loc` can look up: a Fluent identifier, optionally with an attribute after a dot.
pub(crate) fn check_key(field: impl FnOnce() -> String, key: &str) -> Result<(), DialogueError> {
    let good = (1..=MAX_KEY).contains(&key.len())
        && key.split('.').count() <= 2
        && key.split('.').all(|part| {
            let mut bytes = part.bytes();
            bytes.next().is_some_and(|first| first.is_ascii_alphabetic())
                && bytes.all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
        });
    if good {
        Ok(())
    } else {
        Err(DialogueError::invalid(
            field(),
            format!("{key:?} is no localization key: lines carry keys, not text"),
        ))
    }
}

/// A key a panel shows: [`check_key`]'s, under [`KEY_PREFIX`].
pub(crate) fn check_shown_key(field: impl Fn() -> String, key: &str) -> Result<(), DialogueError> {
    check_key(&field, key)?;
    if key.starts_with(KEY_PREFIX) {
        Ok(())
    } else {
        let problem =
            format!("{key:?} doesn't start with `{KEY_PREFIX}`, which is all a dialogue panel reads");
        Err(DialogueError::invalid(field(), problem))
    }
}

/// A line's faces: a role and an expression each, by name, and a weight and blend that are numbers.
pub(crate) fn check_faces(
    field: impl Fn() -> String,
    count: usize,
    faces: impl Iterator<Item = (bool, f32, f32)>,
) -> Result<(), DialogueError> {
    if count > MAX_FACES {
        let problem = format!("{count} faces, past the limit of {MAX_FACES}");
        return Err(DialogueError::invalid(field(), problem));
    }
    for (index, (named, weight, blend)) in faces.enumerate() {
        let at = |part: &str| format!("{}[{index}].{part}", field());
        if !named {
            return Err(DialogueError::invalid(at("role"), format!("a name is 1 to {MAX_KEY} bytes")));
        }
        if !(0.0..=1.0).contains(&weight) {
            return Err(DialogueError::invalid(at("weight"), format!("{weight} is outside 0 to 1")));
        }
        if !(0.0..=MAX_FACE_BLEND).contains(&blend) {
            return Err(DialogueError::invalid(
                at("blend"),
                format!("{blend} is outside 0 to {MAX_FACE_BLEND}"),
            ));
        }
    }
    Ok(())
}

pub(crate) fn check_cue(field: impl FnOnce() -> String, cue: &str) -> Result<(), DialogueError> {
    if (1..=MAX_KEY).contains(&cue.len()) {
        Ok(())
    } else {
        Err(DialogueError::invalid(field(), format!("a cue's name is 1 to {MAX_KEY} bytes")))
    }
}

/// What a document's names resolve to, while it's checked and cooked.
struct Resolved<'a> {
    places: BTreeMap<&'a str, u16>,
    registry: &'a FlagRegistry,
}

impl Resolved<'_> {
    fn node(&self, field: impl FnOnce() -> String, name: &str) -> Result<u16, DialogueError> {
        self.places
            .get(name)
            .copied()
            .ok_or_else(|| DialogueError::invalid(field(), format!("there's no node {name:?}")))
    }

    fn writes(&self, field: &str, set: &[(String, i32)]) -> Result<Vec<FlagWrite>, DialogueError> {
        if set.len() > MAX_WRITES {
            return Err(DialogueError::invalid(
                field,
                format!("{} writes, past the limit of {MAX_WRITES}", set.len()),
            ));
        }
        let mut writes = Vec::with_capacity(set.len());
        for (index, (flag, value)) in set.iter().enumerate() {
            let field = || format!("{field}[{index}]");
            self.flag(field, flag)?;
            if set[..index].iter().any(|(other, _)| other == flag) {
                return Err(DialogueError::invalid(field(), format!("{flag:?} is set already")));
            }
            writes.push(FlagWrite { flag: Name::new(flag).as_u64(), value: *value });
        }
        Ok(writes)
    }

    fn flag(&self, field: impl Fn() -> String, flag: &str) -> Result<(), DialogueError> {
        check_name(&field, flag)?;
        if self.registry.has(flag) {
            Ok(())
        } else {
            Err(DialogueError::invalid(field(), format!("flag {flag:?} isn't in the registry, `flags.ron`")))
        }
    }

    fn condition(&self, field: impl Fn() -> String, text: &str) -> Result<Condition, DialogueError> {
        let condition = Condition::parse(text).map_err(|problem| DialogueError::invalid(field(), problem))?;
        for flag in condition.flags() {
            self.flag(&field, flag)?;
        }
        Ok(condition)
    }
}

/// Everything a cook gathers before it writes: conditions in the order they're met, and effects.
#[derive(Default)]
struct Gathered {
    conditions: Vec<Condition>,
    effects: Vec<Effect>,
}

impl Gathered {
    fn effect(&mut self, writes: Vec<FlagWrite>) -> Option<u16> {
        if writes.is_empty() {
            return None;
        }
        self.effects.push(Effect { writes });
        Some((self.effects.len() - 1) as u16)
    }

    fn condition(&mut self, condition: Condition) -> u16 {
        self.conditions.push(condition);
        (self.conditions.len() - 1) as u16
    }
}

impl DialogueDocument {
    /// Reads a document from RON text. [`Self::cook`] checks it, against a registry.
    pub fn from_ron(text: &str) -> Result<Self, DialogueError> {
        Ok(sapphire_serialize::from_ron(text).map_err(Box::new)?)
    }

    /// Checks everything, naming the field, against the flags `registry` lists.
    pub fn check(&self, registry: &FlagRegistry) -> Result<(), DialogueError> {
        if let Some(event) = &self.opens_on {
            check_name(|| "opens_on".to_owned(), event)?;
        }
        self.gather(registry).map(|_| ())
    }

    /// The conditions' chunk in Luau source, or `None` for a dialogue with none: a table of `names`,
    /// the flags the conditions read, and `conditions`, one function each in the order they're met, nodes
    /// first to last and each node's choices, then its arms. A cook compiles exactly this.
    pub fn conditions_source(&self, registry: &FlagRegistry) -> Result<Option<String>, DialogueError> {
        let (_, gathered) = self.gather(registry)?;
        Ok(source_of(&gathered.conditions))
    }

    /// Its conditions, parsed, by [`ConditionId`](crate::ConditionId): what the chunk's functions do, for a
    /// tool to preview a walk with no VM ([`Condition::holds`]).
    pub fn conditions(&self, registry: &FlagRegistry) -> Result<Vec<Condition>, DialogueError> {
        Ok(self.gather(registry)?.1.conditions)
    }

    /// Cooks it, checking everything first. `compile` is shown [`Self::conditions_source`]'s text, and
    /// isn't called for a dialogue with no conditions: the cooker passes `sapphire_bytecode::cook`, whose
    /// scan refuses what a replay can't survive, so a dialogue whose conditions won't load never cooks. The
    /// cooked dialogue keeps the source, which a load compiles again (see [`crate::cooked`]).
    pub fn cook(
        &self,
        registry: &FlagRegistry,
        compile: impl FnOnce(&str) -> Result<(), String>,
    ) -> Result<Dialogue, DialogueError> {
        let (nodes, gathered) = self.gather(registry)?;
        let source = source_of(&gathered.conditions).unwrap_or_default();
        if !source.is_empty() {
            compile(&source).map_err(DialogueError::Compile)?;
        }
        if let Some(event) = &self.opens_on {
            check_name(|| "opens_on".to_owned(), event)?;
        }
        let resolved = self.resolve(registry)?;
        let dialogue = Dialogue {
            simulation: DialogueSimulation { effects: gathered.effects },
            presentation: DialoguePresentation {
                start: resolved.node(|| "start".to_owned(), &self.start)?,
                nodes,
                conditions: gathered.conditions.len() as u16,
                source,
                opens_on: self.opens_on.as_deref().map(|event| Name::new(event).as_u64()),
            },
        };
        dialogue.validate()?;
        Ok(dialogue)
    }

    fn resolve<'a>(&'a self, registry: &'a FlagRegistry) -> Result<Resolved<'a>, DialogueError> {
        if self.nodes.is_empty() {
            return Err(DialogueError::invalid("nodes", "a dialogue has at least one node"));
        }
        if self.nodes.len() > MAX_NODES {
            let problem = format!("{} nodes, past the limit of {MAX_NODES}", self.nodes.len());
            return Err(DialogueError::invalid("nodes", problem));
        }
        let mut places = BTreeMap::new();
        for (index, node) in self.nodes.iter().enumerate() {
            check_name(|| format!("nodes[{index}].name"), &node.name)?;
            if places.insert(node.name.as_str(), index as u16).is_some() {
                let problem = format!("{:?} is another node's name", node.name);
                return Err(DialogueError::invalid(format!("nodes[{index}].name"), problem));
            }
        }
        Ok(Resolved { places, registry })
    }

    fn gather(&self, registry: &FlagRegistry) -> Result<(Vec<CookedNode>, Gathered), DialogueError> {
        let resolved = self.resolve(registry)?;
        resolved.node(|| "start".to_owned(), &self.start)?;
        let mut gathered = Gathered::default();
        let mut nodes = Vec::with_capacity(self.nodes.len());
        for (index, node) in self.nodes.iter().enumerate() {
            nodes.push(cook_node(&resolved, &mut gathered, index, node)?);
        }
        check_graph(&nodes, resolved.places[self.start.as_str()], |index| format!("nodes[{index}]"))?;
        Ok((nodes, gathered))
    }
}

fn cook_node(
    resolved: &Resolved<'_>,
    gathered: &mut Gathered,
    index: usize,
    node: &NodeDocument,
) -> Result<CookedNode, DialogueError> {
    let at = |field: &str| format!("nodes[{index}].{field}");
    let name = Name::new(&node.name).as_u64();
    let Some(say) = &node.say else {
        if node.branch.is_empty() {
            return Err(DialogueError::invalid(at("say"), "a node says a line, or it branches"));
        }
        for (field, present) in [
            ("set", !node.set.is_empty()),
            ("next", node.next.is_some()),
            ("choices", !node.choices.is_empty()),
        ] {
            if present {
                return Err(DialogueError::invalid(
                    at(field),
                    "a branch says nothing, so it has no `set`, `next` or `choices`",
                ));
            }
        }
        if node.branch.len() > MAX_ARMS {
            let problem = format!("{} arms, past the limit of {MAX_ARMS}", node.branch.len());
            return Err(DialogueError::invalid(at("branch"), problem));
        }
        let mut branch = Vec::with_capacity(node.branch.len());
        for (arm_index, arm) in node.branch.iter().enumerate() {
            let field = |part: &str| format!("nodes[{index}].branch[{arm_index}].{part}");
            let condition = resolved.condition(|| field("when"), &arm.when)?;
            let to = resolved.node(|| field("to"), &arm.to)?;
            branch.push(CookedArm { when: gathered.condition(condition), to });
        }
        let Some(otherwise) = &node.otherwise else {
            return Err(DialogueError::invalid(at("otherwise"), "a branch goes somewhere when no arm holds"));
        };
        let otherwise = Some(resolved.node(|| at("otherwise"), otherwise)?);
        return Ok(CookedNode {
            name,
            say: None,
            effect: None,
            next: None,
            choices: Vec::new(),
            branch,
            otherwise,
        });
    };
    for (field, present) in [("branch", !node.branch.is_empty()), ("otherwise", node.otherwise.is_some())] {
        if present {
            return Err(DialogueError::invalid(at(field), "a node that says a line doesn't branch"));
        }
    }
    check_shown_key(|| at("say.speaker"), &say.speaker)?;
    check_shown_key(|| at("say.line"), &say.line)?;
    if let Some(vo) = &say.vo {
        check_cue(|| at("say.vo"), vo)?;
    }
    let effect = gathered.effect(resolved.writes(&at("set"), &node.set)?);
    if node.next.is_some() && !node.choices.is_empty() {
        return Err(DialogueError::invalid(at("next"), "a node with choices goes where they lead"));
    }
    let next = node.next.as_deref().map(|next| resolved.node(|| at("next"), next)).transpose()?;
    if node.choices.len() > MAX_CHOICES {
        let problem = format!("{} choices, past the {MAX_CHOICES} a panel shows", node.choices.len());
        return Err(DialogueError::invalid(at("choices"), problem));
    }
    let mut choices = Vec::with_capacity(node.choices.len());
    for (choice_index, choice) in node.choices.iter().enumerate() {
        let field = |part: &str| format!("nodes[{index}].choices[{choice_index}].{part}");
        check_shown_key(|| field("line"), &choice.line)?;
        let to = choice.to.as_deref().map(|to| resolved.node(|| field("to"), to)).transpose()?;
        let when = match &choice.when {
            Some(text) => Some(gathered.condition(resolved.condition(|| field("when"), text)?)),
            None => None,
        };
        let effect = gathered.effect(resolved.writes(&field("set"), &choice.set)?);
        choices.push(CookedChoice { line: choice.line.clone(), to, when, effect });
    }
    if !choices.is_empty() && choices.iter().all(|choice| choice.when.is_some()) {
        let problem = "one choice has no `when`, so there's always something to pick";
        return Err(DialogueError::invalid(at("choices"), problem));
    }
    for (face_index, face) in say.faces.iter().enumerate() {
        let field = |part: &str| format!("nodes[{index}].say.faces[{face_index}].{part}");
        if !(1..=MAX_KEY).contains(&face.expression.len()) {
            return Err(DialogueError::invalid(
                field("expression"),
                format!("a name is 1 to {MAX_KEY} bytes"),
            ));
        }
    }
    let named = |name: &str| (1..=MAX_KEY).contains(&name.len());
    check_faces(
        || at("say.faces"),
        say.faces.len(),
        say.faces.iter().map(|face| (named(&face.role), face.weight, face.blend)),
    )?;
    let faces = say
        .faces
        .iter()
        .map(|face| CookedFace {
            role: Name::new(&face.role).as_u64(),
            expression: Name::new(&face.expression).as_u64(),
            weight: face.weight,
            blend: face.blend,
        })
        .collect();
    let say = CookedSay { speaker: say.speaker.clone(), line: say.line.clone(), vo: say.vo.clone(), faces };
    Ok(CookedNode { name, say: Some(say), effect, next, choices, branch: Vec::new(), otherwise: None })
}

/// That every node can be reached from `start`, and that no loop of branches goes round without a line,
/// where a walk would never stop. `field` names a node by its place.
pub(crate) fn check_graph(
    nodes: &[CookedNode],
    start: u16,
    field: impl Fn(usize) -> String,
) -> Result<(), DialogueError> {
    let mut reached = vec![false; nodes.len()];
    let mut stack = vec![start];
    while let Some(at) = stack.pop() {
        let Some(seen) = reached.get_mut(usize::from(at)) else { continue };
        if std::mem::replace(seen, true) {
            continue;
        }
        stack.extend(nodes[usize::from(at)].leads_to());
    }
    if let Some(unreached) = reached.iter().position(|&reached| !reached) {
        return Err(DialogueError::invalid(field(unreached), "nothing leads here from `start`"));
    }
    // Branches alone: a node is done once every branch it leads to is, and a loop never gets there.
    let mut done = vec![false; nodes.len()];
    for _ in 0..nodes.len() {
        let mut moved = false;
        for (index, node) in nodes.iter().enumerate() {
            if done[index] {
                continue;
            }
            let ready = node.say.is_some()
                || node.leads_to().all(|to| done.get(usize::from(to)).copied().unwrap_or(true));
            if ready {
                done[index] = true;
                moved = true;
            }
        }
        if !moved {
            break;
        }
    }
    if let Some(looping) = done.iter().position(|&done| !done) {
        return Err(DialogueError::invalid(
            field(looping),
            "its branches go round in a loop with no line in it",
        ));
    }
    Ok(())
}

/// The conditions' chunk: see [`DialogueDocument::conditions_source`].
fn source_of(conditions: &[Condition]) -> Option<String> {
    if conditions.is_empty() {
        return None;
    }
    let mut names: Vec<&str> = Vec::new();
    for condition in conditions {
        for flag in condition.flags() {
            if !names.contains(&flag) {
                names.push(flag);
            }
        }
    }
    let place_of = |flag: &str| names.iter().position(|&listed| listed == flag).map_or(0, |place| place + 1);
    let mut source = String::from("-- Made by a dialogue's cook from its conditions.\nreturn {\n\tnames = {");
    for name in &names {
        let _ = write!(source, " {name:?},");
    }
    source.push_str(" },\n\tconditions = {\n");
    for condition in conditions {
        source.push_str("\t\tfunction() return ");
        condition.to_luau(&place_of, &mut source);
        source.push_str(" end,\n");
    }
    source.push_str("\t},\n}\n");
    Some(source)
}
