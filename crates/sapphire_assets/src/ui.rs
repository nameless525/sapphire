//! UI documents (M3 U4, spec §5.17): what a screen is, as data the cooker checks, the editor edits and
//! `sapphire_ui`'s retained tree runs.
//!
//! A [`UiDocument`] (`.ui.ron`, cooked to `.sui`) is a flat list of [`UiNode`]s, each naming its parent,
//! as Godot's scenes are. Nothing nests, so nothing recurses: not a parse, not a cooked archive's check
//! of hostile bytes, not an edit's path.
//!
//! ```ron
//! (
//!     type: "sapphire::assets::UiDocument",
//!     version: 1,
//!     data: (
//!         model: "hud.model.ron",
//!         styles: [(name: "label", role: "body", size: 14)],
//!         nodes: [
//!             (name: "root", layout: (direction: Column, safe_area: true)),
//!             (
//!                 name: "health",
//!                 parent: "root",
//!                 look: (shape: Slice(sprite: Bar, corner: 4), fill: Color((30, 200, 160, 255))),
//!                 binds: [(to: Fill, path: "player.health")],
//!             ),
//!             (
//!                 name: "health-label",
//!                 parent: "health",
//!                 text: (
//!                     key: "hud-health",
//!                     style: "label",
//!                     color: (255, 255, 255, 255),
//!                     args: [(name: "hp", path: "player.hp")],
//!                 ),
//!             ),
//!         ],
//!     ),
//! )
//! ```
//!
//! **No text lives here.** A text node names a key, and a style names a role, so a cook can work out
//! what each locale shows in which fonts, and refuse what it can't.
//!
//! **Paths.** A bind reads its value through a reflection path into the model an app hands the tree. A
//! [`UiModel`] (`hud.model.ron`, named by [`UiDocument::model`]) says what paths a model has, so a cook
//! refuses a path that doesn't resolve without linking the game. [`UiModel::of_type`] makes one from a
//! Rust type's reflection, and a test beside the type holds the two together.

use std::collections::BTreeSet;

use bevy_reflect::enums::VariantInfo;
use bevy_reflect::{ParsedPath, Reflect, ReflectDeserialize, ReflectSerialize, TypeInfo};
use sapphire_reflect::Versioned;
use sapphire_reflect::bevy_reflect::std_traits::ReflectDefault;
use sapphire_serialize::DocumentError;
use sapphire_serialize::cooked::{AlignedVec, CookedError};
use serde::{Deserialize, Serialize};

/// Conventional file extension for cooked UI documents.
pub const UI_EXTENSION: &str = "sui";

/// The pack UI documents go in, so a change of screen leaves the shared pack alone and what shows no
/// screen mounts none of them.
pub const UI_PACK: &str = "ui";

/// The sizes text comes in, in UI points: `sapphire_text`'s `TYPE_SCALE`, which a test in `sapphire_ui`
/// holds this to. A document's size is checked here because the cooker links no shaper.
pub const TYPE_SCALE: [f32; 9] = [12.0, 14.0, 16.0, 18.0, 20.0, 24.0, 32.0, 48.0, 64.0];

/// The most nodes a document holds: past this it's a generated mistake.
pub const MAX_NODES: usize = 4096;

/// The deepest a tree goes, which is what `taffy` recurses on.
pub const MAX_DEPTH: usize = 64;

/// The widest a length can be, in points: past this it's a typing mistake.
const MAX_POINTS: f32 = 100_000.0;

/// A color as a designer picks it: display-encoded bytes, straight alpha.
pub type UiColor = [u8; 4];

/// A screen. See the [module docs](self).
#[derive(
    Reflect,
    Clone,
    Debug,
    Default,
    PartialEq,
    Serialize,
    Deserialize,
    rkyv::Archive,
    rkyv::Serialize,
    rkyv::Deserialize,
)]
#[reflect(Serialize, Deserialize, Default)]
#[rkyv(derive(Debug))]
#[serde(deny_unknown_fields)]
#[type_path = "sapphire::assets"]
pub struct UiDocument {
    /// The [`UiModel`] document its paths are checked against, relative to this file. A document that
    /// binds nothing names none.
    #[serde(default)]
    pub model: Option<String>,
    /// What its text can look like.
    #[serde(default)]
    pub styles: Vec<UiTextStyle>,
    /// Its nodes. The first is the root, and a node's parent comes before it.
    pub nodes: Vec<UiNode>,
}

impl Versioned for UiDocument {
    const VERSION: u32 = 1;
}

/// How a text looks, which any text node of the document can name.
#[derive(
    Reflect,
    Clone,
    Debug,
    Default,
    PartialEq,
    Serialize,
    Deserialize,
    rkyv::Archive,
    rkyv::Serialize,
    rkyv::Deserialize,
)]
#[reflect(Serialize, Deserialize, Default)]
#[rkyv(derive(Debug))]
#[serde(deny_unknown_fields)]
pub struct UiTextStyle {
    pub name: String,
    /// What the text is for, `body` or `display`: the name a locale's `font_chain` answers to.
    pub role: String,
    /// In points, from [`TYPE_SCALE`].
    pub size: f32,
    /// 100 to 900.
    #[serde(default = "regular")]
    pub weight: f32,
    /// A multiple of the font's own line height.
    #[serde(default = "normal_line")]
    pub line_height: f32,
}

fn regular() -> f32 {
    400.0
}

fn normal_line() -> f32 {
    1.3
}

/// One node of a screen.
#[derive(
    Reflect,
    Clone,
    Debug,
    Default,
    PartialEq,
    Serialize,
    Deserialize,
    rkyv::Archive,
    rkyv::Serialize,
    rkyv::Deserialize,
)]
#[reflect(Serialize, Deserialize, Default)]
#[rkyv(derive(Debug))]
#[serde(deny_unknown_fields)]
pub struct UiNode {
    /// What code and other nodes find it by, unique in its document.
    pub name: String,
    /// Its parent's name. Only the first node has none.
    #[serde(default)]
    pub parent: Option<String>,
    #[serde(default)]
    pub layout: UiLayout,
    /// What it draws behind its children.
    #[serde(default)]
    pub look: Option<UiLook>,
    /// The text it shows. A node with text sizes itself from it.
    #[serde(default)]
    pub text: Option<UiText>,
    /// Whether its children draw only inside it.
    #[serde(default)]
    pub mask: bool,
    /// How opaque it and everything under it are: 0 to 1.
    #[serde(default = "opaque")]
    pub opacity: f32,
    /// What the model drives, beyond its text's arguments.
    #[serde(default)]
    pub binds: Vec<UiBind>,
}

fn opaque() -> f32 {
    1.0
}

/// How a node sits among its neighbours: a subset of `taffy`'s flexbox and grid, and only what a screen
/// has needed. Lengths are in UI points, a size that's the same to a thumb on any screen.
#[derive(
    Reflect, Clone, Debug, PartialEq, Serialize, Deserialize, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize,
)]
#[reflect(Serialize, Deserialize, Default)]
#[rkyv(derive(Debug))]
#[serde(default, deny_unknown_fields)]
pub struct UiLayout {
    pub display: UiDisplay,
    /// The way a flex container's children run.
    pub direction: UiDirection,
    /// Whether they wrap onto further rows.
    pub wrap: bool,
    /// Where its children go along that way.
    pub justify: UiJustify,
    /// Where they go across it.
    pub align: Option<UiAlign>,
    /// Where this node goes across its parent's way, over its parent's `align`.
    pub align_self: Option<UiAlign>,
    /// The space between its children.
    pub gap: f32,
    pub padding: UiEdges,
    pub margin: UiEdges,
    /// Adds what the device covers to its padding, so a full-bleed parent can hold a child that stays
    /// clear of a notch.
    pub safe_area: bool,
    pub width: UiLength,
    pub height: UiLength,
    pub min_width: UiLength,
    pub min_height: UiLength,
    pub max_width: UiLength,
    pub max_height: UiLength,
    /// How much of its parent's spare room it takes.
    pub grow: f32,
    /// How much it gives up when there's too little.
    pub shrink: f32,
    /// Width over height, where one of them is free.
    pub aspect: Option<f32>,
    /// `None` flows among its siblings, and `Some` is placed against its parent's edges, ignoring them.
    pub anchored: Option<UiAnchors>,
    /// A grid container's column tracks.
    pub columns: Vec<UiTrack>,
    /// A grid container's row tracks.
    pub rows: Vec<UiTrack>,
    /// Where it goes in its parent's grid, if not the next free cell.
    pub cell: Option<UiCell>,
}

impl Default for UiLayout {
    fn default() -> Self {
        Self {
            display: UiDisplay::default(),
            direction: UiDirection::default(),
            wrap: false,
            justify: UiJustify::default(),
            align: None,
            align_self: None,
            gap: 0.0,
            padding: UiEdges::default(),
            margin: UiEdges::default(),
            safe_area: false,
            width: UiLength::Auto,
            height: UiLength::Auto,
            min_width: UiLength::Auto,
            min_height: UiLength::Auto,
            max_width: UiLength::Auto,
            max_height: UiLength::Auto,
            grow: 0.0,
            shrink: 1.0,
            aspect: None,
            anchored: None,
            columns: Vec::new(),
            rows: Vec::new(),
            cell: None,
        }
    }
}

/// How a node lays out its children.
#[derive(
    Reflect,
    Clone,
    Copy,
    Debug,
    Default,
    PartialEq,
    Eq,
    Serialize,
    Deserialize,
    rkyv::Archive,
    rkyv::Serialize,
    rkyv::Deserialize,
)]
#[reflect(Serialize, Deserialize, Default)]
#[rkyv(derive(Debug))]
pub enum UiDisplay {
    #[default]
    Flex,
    Grid,
}

/// The way a flex container's children run.
#[derive(
    Reflect,
    Clone,
    Copy,
    Debug,
    Default,
    PartialEq,
    Eq,
    Serialize,
    Deserialize,
    rkyv::Archive,
    rkyv::Serialize,
    rkyv::Deserialize,
)]
#[reflect(Serialize, Deserialize, Default)]
#[rkyv(derive(Debug))]
pub enum UiDirection {
    #[default]
    Row,
    Column,
}

/// Where children go along the way they run.
#[derive(
    Reflect,
    Clone,
    Copy,
    Debug,
    Default,
    PartialEq,
    Eq,
    Serialize,
    Deserialize,
    rkyv::Archive,
    rkyv::Serialize,
    rkyv::Deserialize,
)]
#[reflect(Serialize, Deserialize, Default)]
#[rkyv(derive(Debug))]
pub enum UiJustify {
    #[default]
    Start,
    End,
    Center,
    SpaceBetween,
    SpaceAround,
    SpaceEvenly,
}

/// Where a node goes across the way its parent runs.
#[derive(
    Reflect,
    Clone,
    Copy,
    Debug,
    Default,
    PartialEq,
    Eq,
    Serialize,
    Deserialize,
    rkyv::Archive,
    rkyv::Serialize,
    rkyv::Deserialize,
)]
#[reflect(Serialize, Deserialize, Default)]
#[rkyv(derive(Debug))]
pub enum UiAlign {
    #[default]
    Start,
    End,
    Center,
    Stretch,
}

/// A distance on each side, in points.
#[derive(
    Reflect,
    Clone,
    Copy,
    Debug,
    Default,
    PartialEq,
    Serialize,
    Deserialize,
    rkyv::Archive,
    rkyv::Serialize,
    rkyv::Deserialize,
)]
#[reflect(Serialize, Deserialize, Default)]
#[rkyv(derive(Debug))]
#[serde(default, deny_unknown_fields)]
pub struct UiEdges {
    pub left: f32,
    pub top: f32,
    pub right: f32,
    pub bottom: f32,
}

/// A length that may depend on the parent.
#[derive(
    Reflect,
    Clone,
    Copy,
    Debug,
    Default,
    PartialEq,
    Serialize,
    Deserialize,
    rkyv::Archive,
    rkyv::Serialize,
    rkyv::Deserialize,
)]
#[reflect(Serialize, Deserialize, Default)]
#[rkyv(derive(Debug))]
pub enum UiLength {
    #[default]
    Auto,
    Points(f32),
    /// A share of the parent's: 100 is all of it.
    Percent(f32),
}

/// What a node is placed against, each side free unless named.
#[derive(
    Reflect,
    Clone,
    Copy,
    Debug,
    Default,
    PartialEq,
    Serialize,
    Deserialize,
    rkyv::Archive,
    rkyv::Serialize,
    rkyv::Deserialize,
)]
#[reflect(Serialize, Deserialize, Default)]
#[rkyv(derive(Debug))]
#[serde(default, deny_unknown_fields)]
pub struct UiAnchors {
    pub left: UiLength,
    pub top: UiLength,
    pub right: UiLength,
    pub bottom: UiLength,
}

/// One grid track.
#[derive(
    Reflect,
    Clone,
    Copy,
    Debug,
    Default,
    PartialEq,
    Serialize,
    Deserialize,
    rkyv::Archive,
    rkyv::Serialize,
    rkyv::Deserialize,
)]
#[reflect(Serialize, Deserialize, Default)]
#[rkyv(derive(Debug))]
pub enum UiTrack {
    #[default]
    Auto,
    Points(f32),
    Percent(f32),
    /// A share of what's left over.
    Fr(f32),
}

/// Where a node goes in a grid, by track from 1.
#[derive(
    Reflect,
    Clone,
    Copy,
    Debug,
    Default,
    PartialEq,
    Serialize,
    Deserialize,
    rkyv::Archive,
    rkyv::Serialize,
    rkyv::Deserialize,
)]
#[reflect(Serialize, Deserialize, Default)]
#[rkyv(derive(Debug))]
#[serde(deny_unknown_fields)]
pub struct UiCell {
    pub column: u16,
    pub row: u16,
    #[serde(default = "one")]
    pub column_span: u16,
    #[serde(default = "one")]
    pub row_span: u16,
}

fn one() -> u16 {
    1
}

/// What a node draws.
#[derive(
    Reflect,
    Clone,
    Copy,
    Debug,
    Default,
    PartialEq,
    Serialize,
    Deserialize,
    rkyv::Archive,
    rkyv::Serialize,
    rkyv::Deserialize,
)]
#[reflect(Serialize, Deserialize, Default)]
#[rkyv(derive(Debug))]
#[serde(deny_unknown_fields)]
pub struct UiLook {
    #[serde(default)]
    pub shape: UiShape,
    pub fill: UiFill,
    /// A line around the shape's edge, which needs a shape with an edge: not [`UiShape::Rect`].
    #[serde(default)]
    pub outline: Option<UiOutline>,
    /// How far its top leans to the right, in points per point of height, as the HUD's shapes do. Its
    /// children and text stay upright.
    #[serde(default)]
    pub slant: f32,
}

/// The shape a node fills.
#[derive(
    Reflect,
    Clone,
    Copy,
    Debug,
    Default,
    PartialEq,
    Serialize,
    Deserialize,
    rkyv::Archive,
    rkyv::Serialize,
    rkyv::Deserialize,
)]
#[reflect(Serialize, Deserialize, Default)]
#[rkyv(derive(Debug))]
pub enum UiShape {
    /// The node's rectangle, with hard edges on whole pixels.
    #[default]
    Rect,
    /// A nine-slice of `sprite`: its four corners stay `corner` points large and its edges and middle
    /// stretch, so a panel keeps its shape at any size. Over a signed distance field, so it stays sharp
    /// at any scale and takes an outline.
    Slice { sprite: UiSprite, corner: f32 },
    /// `sprite`, stretched over the node.
    Sprite(UiSprite),
}

/// A shape of the interface's atlas (`sapphire_ui::atlas`): a cell of a signed distance field, and so
/// one color, whatever fills it. There are no color images yet.
#[derive(
    Reflect,
    Clone,
    Copy,
    Debug,
    Default,
    PartialEq,
    Eq,
    Serialize,
    Deserialize,
    rkyv::Archive,
    rkyv::Serialize,
    rkyv::Deserialize,
)]
#[reflect(Serialize, Deserialize, Default)]
#[rkyv(derive(Debug))]
pub enum UiSprite {
    /// A square with small rounded corners.
    #[default]
    Bar,
    /// A square with two corners cut hard and two barely.
    Button,
    Disc,
    Sword,
    Chevrons,
    Spark,
    Wedge,
}

/// What fills a shape.
#[derive(
    Reflect,
    Clone,
    Copy,
    Debug,
    PartialEq,
    Serialize,
    Deserialize,
    rkyv::Archive,
    rkyv::Serialize,
    rkyv::Deserialize,
)]
#[reflect(Serialize, Deserialize, Default)]
#[rkyv(derive(Debug))]
pub enum UiFill {
    Color(UiColor),
    /// From one color to another across the node, in linear light. `angle` is in degrees clockwise from
    /// left to right, so 90 runs top to bottom.
    Gradient { from: UiColor, to: UiColor, angle: f32 },
}

impl Default for UiFill {
    fn default() -> Self {
        Self::Color([255; 4])
    }
}

/// A line around a shape.
#[derive(
    Reflect,
    Clone,
    Copy,
    Debug,
    Default,
    PartialEq,
    Serialize,
    Deserialize,
    rkyv::Archive,
    rkyv::Serialize,
    rkyv::Deserialize,
)]
#[reflect(Serialize, Deserialize, Default)]
#[rkyv(derive(Debug))]
#[serde(deny_unknown_fields)]
pub struct UiOutline {
    pub color: UiColor,
    /// In points.
    pub width: f32,
}

/// The text a node shows.
#[derive(
    Reflect,
    Clone,
    Debug,
    Default,
    PartialEq,
    Serialize,
    Deserialize,
    rkyv::Archive,
    rkyv::Serialize,
    rkyv::Deserialize,
)]
#[reflect(Serialize, Deserialize, Default)]
#[rkyv(derive(Debug))]
#[serde(deny_unknown_fields)]
pub struct UiText {
    /// The string's key, in every locale's tables.
    pub key: String,
    /// One of the document's [`UiTextStyle`]s, by name.
    pub style: String,
    pub color: UiColor,
    /// Whether it wraps to its node's width, or stays one line that sizes the node.
    #[serde(default)]
    pub wrap: bool,
    /// The message's variables, each read from the model.
    #[serde(default)]
    pub args: Vec<UiArg>,
}

/// A message's variable, and where its value comes from.
#[derive(
    Reflect,
    Clone,
    Debug,
    Default,
    PartialEq,
    Serialize,
    Deserialize,
    rkyv::Archive,
    rkyv::Serialize,
    rkyv::Deserialize,
)]
#[reflect(Serialize, Deserialize, Default)]
#[rkyv(derive(Debug))]
#[serde(deny_unknown_fields)]
pub struct UiArg {
    pub name: String,
    /// A reflection path into the model.
    pub path: String,
}

/// A property of a node that reads the model.
#[derive(
    Reflect,
    Clone,
    Debug,
    Default,
    PartialEq,
    Serialize,
    Deserialize,
    rkyv::Archive,
    rkyv::Serialize,
    rkyv::Deserialize,
)]
#[reflect(Serialize, Deserialize, Default)]
#[rkyv(derive(Debug))]
#[serde(deny_unknown_fields)]
pub struct UiBind {
    pub to: UiBindTarget,
    /// A reflection path into the model.
    pub path: String,
}

/// What a bound value drives.
#[derive(
    Reflect,
    Clone,
    Copy,
    Debug,
    Default,
    PartialEq,
    Eq,
    Serialize,
    Deserialize,
    rkyv::Archive,
    rkyv::Serialize,
    rkyv::Deserialize,
)]
#[reflect(Serialize, Deserialize, Default)]
#[rkyv(derive(Debug))]
pub enum UiBindTarget {
    /// A true/false: false takes the node out of the layout, and its children with it.
    #[default]
    Shown,
    /// A number from 0 to 1, multiplying [`UiNode::opacity`].
    Opacity,
    /// A number from 0 to 1: how much of the node's look draws, from its left, as a bar's fill.
    Fill,
}

impl UiBindTarget {
    /// The kind of value it reads.
    #[must_use]
    pub const fn kind(self) -> UiValueKind {
        match self {
            Self::Shown => UiValueKind::Bool,
            Self::Opacity | Self::Fill => UiValueKind::Number,
        }
    }
}

/// A problem with a document, naming the field. See [`UiDocument::issues`].
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error("{field}: {problem}")]
pub struct UiIssue {
    pub field: String,
    pub problem: String,
}

/// A UI document or model that can't be read.
#[derive(Debug, thiserror::Error)]
pub enum UiError {
    /// The document isn't one at all: the wrong type or a version out of reach.
    #[error(transparent)]
    Document(Box<DocumentError>),
    /// Malformed RON, or a value of the wrong kind. `text` is the document's own line, which names the
    /// field.
    #[error("line {line}, column {column}: {message}: `{text}`")]
    Malformed { line: usize, column: usize, message: String, text: String },
    /// A value that parsed but can't be used.
    #[error(transparent)]
    Field(#[from] UiIssue),
    #[error(transparent)]
    Cooked(#[from] CookedError),
}

impl UiDocument {
    /// Reads and checks a document, refusing the first thing wrong with it.
    pub fn from_document(text: &str) -> Result<Self, UiError> {
        let document = parse::<Self>(text)?;
        if let Some(issue) = document.issues().into_iter().next() {
            return Err(UiError::Field(issue));
        }
        Ok(document)
    }

    /// Everything wrong with this document that it can see on its own, each naming its field.
    #[must_use]
    pub fn issues(&self) -> Vec<UiIssue> {
        let mut issues = Vec::new();
        let mut styles = BTreeSet::new();
        for (index, style) in self.styles.iter().enumerate() {
            let mut issue = |field: &str, problem: String| {
                issues.push(UiIssue { field: format!("styles[{index}].{field}"), problem });
            };
            if style.name.is_empty() {
                issue("name", "a style has a name".to_owned());
            } else if !styles.insert(style.name.as_str()) {
                issue("name", format!("a second style called `{}`", style.name));
            }
            if style.role.is_empty() {
                issue("role", "a style says what its text is for, such as body".to_owned());
            }
            if !TYPE_SCALE.contains(&style.size) {
                issue("size", format!("{} isn't on the type scale, {TYPE_SCALE:?}", style.size));
            }
            if !(100.0..=900.0).contains(&style.weight) {
                issue("weight", format!("{} isn't from 100 to 900", style.weight));
            }
            if !(style.line_height > 0.0 && style.line_height <= 4.0) {
                issue("line_height", format!("{} isn't a line height", style.line_height));
            }
        }
        if self.nodes.is_empty() {
            issues.push(UiIssue { field: "nodes".to_owned(), problem: "a screen has a root".to_owned() });
        }
        if self.nodes.len() > MAX_NODES {
            issues.push(UiIssue {
                field: "nodes".to_owned(),
                problem: format!("{} nodes, past the {MAX_NODES} a screen may hold", self.nodes.len()),
            });
        }
        let mut seen: Vec<(&str, usize, bool)> = Vec::with_capacity(self.nodes.len());
        for (index, node) in self.nodes.iter().enumerate() {
            let field = |field: &str| format!("nodes[{index}] `{}`{field}", node.name);
            let mut issue = |name: &str, problem: String| {
                issues.push(UiIssue { field: field(name), problem });
            };
            if node.name.is_empty() {
                issue(".name", "a node has a name".to_owned());
            } else if seen.iter().any(|(name, ..)| *name == node.name) {
                issue(".name", "a second node has this name".to_owned());
            }
            let depth = match (index, node.parent.as_deref()) {
                (0, None) => 1,
                (0, Some(parent)) => {
                    issue(".parent", format!("the first node is the root, and has no parent, not `{parent}`"));
                    1
                }
                (_, None) => {
                    issue(".parent", "only the first node has no parent".to_owned());
                    1
                }
                (_, Some(parent)) => match seen.iter().find(|(name, ..)| *name == parent) {
                    Some((.., true)) => {
                        issue(".parent", format!("`{parent}` shows text, which sizes it, so it has no children"));
                        1
                    }
                    Some((_, depth, _)) => depth + 1,
                    None => {
                        issue(".parent", format!("`{parent}` isn't a node before this one"));
                        1
                    }
                },
            };
            if depth > MAX_DEPTH {
                issue(".parent", format!("{depth} levels deep, past the {MAX_DEPTH} a tree may go"));
            }
            seen.push((node.name.as_str(), depth, node.text.is_some()));
            if !(0.0..=1.0).contains(&node.opacity) {
                issue(".opacity", format!("{} isn't from 0 to 1", node.opacity));
            }
            check_layout(&node.layout, &mut issue);
            if let Some(look) = &node.look {
                check_look(look, &mut issue);
            }
            if let Some(text) = &node.text {
                check_text(text, &self.styles, &mut issue);
            }
            for (at, bind) in node.binds.iter().enumerate() {
                if let Err(problem) = check_path(&bind.path) {
                    issue(&format!(".binds[{at}].path"), problem);
                }
                if node.binds[..at].iter().any(|earlier| earlier.to == bind.to) {
                    issue(&format!(".binds[{at}].to"), format!("{:?} is bound twice", bind.to));
                }
            }
        }
        if self.model.is_none()
            && let Some((index, node)) = self
                .nodes
                .iter()
                .enumerate()
                .find(|(_, node)| !node.binds.is_empty() || node.text.as_ref().is_some_and(|t| !t.args.is_empty()))
        {
            issues.push(UiIssue {
                field: format!("nodes[{index}] `{}`", node.name),
                problem: "it binds a value, and the document names no model to check it against".to_owned(),
            });
        }
        issues
    }

    /// Everything wrong with this document's paths against `model`, each naming its field.
    #[must_use]
    pub fn path_issues(&self, model: &UiModel) -> Vec<UiIssue> {
        let mut issues = Vec::new();
        for (index, node) in self.nodes.iter().enumerate() {
            let mut issue = |name: String, problem: String| {
                issues.push(UiIssue { field: format!("nodes[{index}] `{}`{name}", node.name), problem });
            };
            for (at, bind) in node.binds.iter().enumerate() {
                match model.kind_of(&bind.path) {
                    Some(kind) if kind == bind.to.kind() => {}
                    Some(kind) => issue(
                        format!(".binds[{at}].path"),
                        format!("`{}` is a {kind:?}, and {:?} reads a {:?}", bind.path, bind.to, bind.to.kind()),
                    ),
                    None => issue(
                        format!(".binds[{at}].path"),
                        format!("`{}` isn't in the model", bind.path),
                    ),
                }
            }
            for (at, arg) in node.text.iter().flat_map(|text| &text.args).enumerate() {
                if model.kind_of(&arg.path).is_none() {
                    issue(format!(".text.args[{at}].path"), format!("`{}` isn't in the model", arg.path));
                }
            }
        }
        issues
    }

    /// The nodes that show text, with each one's key and the role of its style. What a cook works out a
    /// locale's characters from.
    pub fn texts(&self) -> impl Iterator<Item = (&UiNode, &UiText, &str)> {
        self.nodes.iter().filter_map(|node| {
            let text = node.text.as_ref()?;
            let style = self.styles.iter().find(|style| style.name == text.style)?;
            Some((node, text, style.role.as_str()))
        })
    }

    pub fn to_cooked(&self) -> Result<AlignedVec, CookedError> {
        sapphire_serialize::to_cooked(self)
    }

    /// Reads cooked bytes in place, checking the envelope and the archive, and every field the way a
    /// document's are, so hostile bytes get no further than a typed one.
    pub fn read(bytes: &[u8]) -> Result<&ArchivedUiDocument, UiError> {
        let document = sapphire_serialize::access::<Self>(bytes)?;
        let owned = document.to_owned_document()?;
        if let Some(issue) = owned.issues().into_iter().next() {
            return Err(UiError::Field(issue));
        }
        Ok(document)
    }
}

impl ArchivedUiDocument {
    /// The document as plain values, which a tree is built from.
    pub fn to_owned_document(&self) -> Result<UiDocument, UiError> {
        rkyv::deserialize::<UiDocument, rkyv::rancor::Error>(self)
            .map_err(|error| UiError::Cooked(CookedError::Rkyv(error)))
    }
}

fn check_length(length: UiLength) -> Result<(), String> {
    match length {
        UiLength::Auto => Ok(()),
        UiLength::Points(points) if points.is_finite() && points.abs() <= MAX_POINTS => Ok(()),
        UiLength::Percent(percent) if percent.is_finite() && percent.abs() <= 1000.0 => Ok(()),
        other => Err(format!("{other:?} isn't a length")),
    }
}

fn check_points(points: f32) -> Result<(), String> {
    if points.is_finite() && (0.0..=MAX_POINTS).contains(&points) {
        Ok(())
    } else {
        Err(format!("{points} isn't a distance in points"))
    }
}

fn check_layout(layout: &UiLayout, issue: &mut impl FnMut(&str, String)) {
    let mut report = |name: &str, result: Result<(), String>| {
        if let Err(problem) = result {
            issue(&format!(".layout.{name}"), problem);
        }
    };
    for (name, length) in [
        ("width", layout.width),
        ("height", layout.height),
        ("min_width", layout.min_width),
        ("min_height", layout.min_height),
        ("max_width", layout.max_width),
        ("max_height", layout.max_height),
    ] {
        report(name, check_length(length));
    }
    report("gap", check_points(layout.gap));
    for (name, edges) in [("padding", layout.padding), ("margin", layout.margin)] {
        for value in [edges.left, edges.top, edges.right, edges.bottom] {
            report(name, check_points(value));
        }
    }
    for (name, share) in [("grow", layout.grow), ("shrink", layout.shrink)] {
        if !share.is_finite() || share < 0.0 {
            report(name, Err(format!("{share} isn't a share")));
        }
    }
    if let Some(aspect) = layout.aspect
        && !(aspect.is_finite() && aspect > 0.0)
    {
        report("aspect", Err(format!("{aspect} isn't a width over a height")));
    }
    if let Some(anchors) = layout.anchored {
        for length in [anchors.left, anchors.top, anchors.right, anchors.bottom] {
            report("anchored", check_length(length));
        }
    }
    for (name, tracks) in [("columns", &layout.columns), ("rows", &layout.rows)] {
        for track in tracks {
            let good = match *track {
                UiTrack::Auto => true,
                UiTrack::Points(value) | UiTrack::Percent(value) | UiTrack::Fr(value) => {
                    value.is_finite() && (0.0..=MAX_POINTS).contains(&value)
                }
            };
            if !good {
                report(name, Err(format!("{track:?} isn't a track")));
            }
        }
    }
    if let Some(cell) = layout.cell
        && (cell.column == 0 || cell.row == 0 || cell.column_span == 0 || cell.row_span == 0)
    {
        report("cell", Err("tracks count from 1, and a span is at least 1".to_owned()));
    }
}

fn check_look(look: &UiLook, issue: &mut impl FnMut(&str, String)) {
    if let UiShape::Slice { corner, .. } = look.shape
        && !(corner.is_finite() && corner > 0.0 && corner <= MAX_POINTS)
    {
        issue(".look.shape.corner", format!("{corner} isn't a size in points"));
    }
    if let UiFill::Gradient { angle, .. } = look.fill
        && !angle.is_finite()
    {
        issue(".look.fill.angle", format!("{angle} isn't an angle"));
    }
    if let Some(outline) = look.outline {
        if matches!(look.shape, UiShape::Rect) {
            issue(".look.outline", "a Rect has no edge to draw a line around: use a Slice".to_owned());
        }
        if !(outline.width.is_finite() && outline.width > 0.0 && outline.width <= MAX_POINTS) {
            issue(".look.outline.width", format!("{} isn't a width in points", outline.width));
        }
    }
    if !look.slant.is_finite() || look.slant.abs() > 4.0 {
        issue(".look.slant", format!("{} isn't a lean", look.slant));
    }
}

fn check_text(text: &UiText, styles: &[UiTextStyle], issue: &mut impl FnMut(&str, String)) {
    if text.key.is_empty() {
        issue(".text.key", "a text names a key".to_owned());
    }
    if !styles.iter().any(|style| style.name == text.style) {
        issue(".text.style", format!("`{}` isn't one of the document's styles", text.style));
    }
    for (at, arg) in text.args.iter().enumerate() {
        if arg.name.is_empty() {
            issue(&format!(".text.args[{at}].name"), "a variable has a name".to_owned());
        }
        if text.args[..at].iter().any(|earlier| earlier.name == arg.name) {
            issue(&format!(".text.args[{at}].name"), format!("`{}` is passed twice", arg.name));
        }
        if let Err(problem) = check_path(&arg.path) {
            issue(&format!(".text.args[{at}].path"), problem);
        }
    }
}

/// Whether `path` is a reflection path at all.
fn check_path(path: &str) -> Result<(), String> {
    if path.is_empty() {
        return Err("a path names a field".to_owned());
    }
    ParsedPath::parse(path).map(drop).map_err(|error| format!("`{path}` isn't a path: {error}"))
}

/// The kinds of value a bind can read.
#[derive(
    Reflect,
    Clone,
    Copy,
    Debug,
    Default,
    PartialEq,
    Eq,
    Serialize,
    Deserialize,
)]
#[reflect(Serialize, Deserialize, Default)]
pub enum UiValueKind {
    /// Any integer or float.
    #[default]
    Number,
    Bool,
    /// A string, or a fieldless enum's variant, by name.
    Text,
}

/// One path a model has.
#[derive(Reflect, Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[reflect(Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct UiModelField {
    /// As a bind writes it, with `[*]` for a list's index.
    pub path: String,
    pub kind: UiValueKind,
}

/// What paths a model has, as a cook can read them with no game linked: see the [module docs](self).
/// Not a source: the UI documents that name one read it, and a change to it cooks them again.
#[derive(Reflect, Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[reflect(Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
#[type_path = "sapphire::assets"]
pub struct UiModel {
    pub fields: Vec<UiModelField>,
}

impl Versioned for UiModel {
    const VERSION: u32 = 1;
}

/// How deep [`UiModel::of_type`] follows a type before calling it recursive.
const MODEL_DEPTH: usize = 12;

impl UiModel {
    /// Reads a model document.
    pub fn from_document(text: &str) -> Result<Self, UiError> {
        parse::<Self>(text)
    }

    /// The model a type's reflection describes, its paths sorted. A type that holds itself stops at
    /// [`MODEL_DEPTH`] levels, and what's past that isn't a path a screen can bind.
    #[must_use]
    pub fn of_type(info: &'static TypeInfo) -> Self {
        let mut fields = Vec::new();
        walk_type(info, &mut String::new(), 0, &mut fields);
        fields.sort_by(|a: &UiModelField, b| a.path.cmp(&b.path));
        Self { fields }
    }

    /// What kind of value `path` reads, if the model has it. Any index of a list is that list's `[*]`.
    #[must_use]
    pub fn kind_of(&self, path: &str) -> Option<UiValueKind> {
        let wanted = without_indices(path);
        self.fields.iter().find(|field| field.path == wanted).map(|field| field.kind)
    }
}

/// `path` with every list index as `[*]`.
fn without_indices(path: &str) -> String {
    let mut out = String::with_capacity(path.len());
    let mut rest = path;
    while let Some(open) = rest.find('[') {
        let Some(close) = rest[open..].find(']') else { break };
        out.push_str(&rest[..open]);
        out.push_str("[*]");
        rest = &rest[open + close + 1..];
    }
    out.push_str(rest);
    out
}

fn value_kind(type_path: &str) -> Option<UiValueKind> {
    match type_path {
        "f32" | "f64" | "i8" | "i16" | "i32" | "i64" | "isize" | "u8" | "u16" | "u32" | "u64" | "usize" => {
            Some(UiValueKind::Number)
        }
        "bool" => Some(UiValueKind::Bool),
        "alloc::string::String" | "&str" | "char" => Some(UiValueKind::Text),
        _ => None,
    }
}

fn walk_type(info: &'static TypeInfo, path: &mut String, depth: usize, fields: &mut Vec<UiModelField>) {
    if depth > MODEL_DEPTH {
        return;
    }
    let mut child = |name: &str, list: bool, info: Option<&'static TypeInfo>, fields: &mut Vec<UiModelField>| {
        let Some(info) = info else { return };
        let end = path.len();
        if list {
            path.push_str(name);
        } else {
            if !path.is_empty() {
                path.push('.');
            }
            path.push_str(name);
        }
        walk_type(info, path, depth + 1, fields);
        path.truncate(end);
    };
    match info {
        TypeInfo::Struct(info) => {
            for field in info.iter() {
                child(field.name(), false, field.type_info(), fields);
            }
        }
        TypeInfo::TupleStruct(info) => {
            for field in info.iter() {
                child(&field.index().to_string(), false, field.type_info(), fields);
            }
        }
        TypeInfo::Tuple(info) => {
            for field in info.iter() {
                child(&field.index().to_string(), false, field.type_info(), fields);
            }
        }
        TypeInfo::List(info) => child("[*]", true, info.item_info(), fields),
        TypeInfo::Array(info) => child("[*]", true, info.item_info(), fields),
        TypeInfo::Enum(info) => {
            if info.iter().all(|variant| matches!(variant, VariantInfo::Unit(_))) {
                fields.push(UiModelField { path: path.clone(), kind: UiValueKind::Text });
                return;
            }
            for variant in info.iter() {
                match variant {
                    VariantInfo::Struct(variant) => {
                        for field in variant.iter() {
                            child(field.name(), false, field.type_info(), fields);
                        }
                    }
                    VariantInfo::Tuple(variant) => {
                        for field in variant.iter() {
                            child(&field.index().to_string(), false, field.type_info(), fields);
                        }
                    }
                    VariantInfo::Unit(_) => {}
                }
            }
        }
        TypeInfo::Opaque(info) => {
            if let Some(kind) = value_kind(info.type_path()) {
                fields.push(UiModelField { path: path.clone(), kind });
            }
        }
        TypeInfo::Map(_) | TypeInfo::Set(_) => {}
    }
}

/// `T` from its document, with a parse error quoting the document's own line, which names the field.
fn parse<T>(text: &str) -> Result<T, UiError>
where
    T: Versioned + serde::de::DeserializeOwned + bevy_reflect::TypePath,
{
    sapphire_serialize::from_ron::<T>(text).map_err(|error| match error {
        DocumentError::Parse(spanned) => {
            let line = spanned.span.start.line;
            let quoted = text.lines().nth(line.saturating_sub(1)).unwrap_or_default().trim();
            UiError::Malformed {
                line,
                column: spanned.span.start.col,
                message: spanned.code.to_string(),
                text: quoted.to_owned(),
            }
        }
        other => UiError::Document(Box::new(other)),
    })
}

#[cfg(test)]
mod tests {
    use bevy_reflect::Typed;
    use sapphire_serialize::to_ron;

    use super::*;

    fn document(nodes: &str) -> String {
        format!(
            r#"(
    type: "sapphire::assets::UiDocument",
    version: 1,
    data: (
        model: "m.model.ron",
        styles: [(name: "label", role: "body", size: 14)],
        nodes: [{nodes}],
    ),
)"#
        )
    }

    fn one_issue(nodes: &str) -> UiIssue {
        let parsed = UiDocument::from_document(&document(nodes));
        match parsed {
            Err(UiError::Field(issue)) => issue,
            other => panic!("expected a field's issue, got {other:?}"),
        }
    }

    const ROOT: &str = r#"(name: "root"),"#;

    #[test]
    fn a_document_reads_with_its_defaults() {
        let text = document(concat!(
            r#"(name: "root", layout: (direction: Column, width: Percent(100), safe_area: true)),"#,
            r#"(name: "panel", parent: "root", mask: true, look: (shape: Slice(sprite: Button, corner: 8), "#,
            r#"fill: Gradient(from: (1, 2, 3, 255), to: (4, 5, 6, 255), angle: 90), outline: (color: (255, 255, 255, 255), width: 1), slant: 0.3)),"#,
            r#"(name: "title", parent: "panel", text: (key: "a-key", style: "label", color: (9, 9, 9, 255), args: [(name: "hp", path: "player.hp")])),"#,
        ));
        let read = UiDocument::from_document(&text).expect("a good document");
        assert_eq!(read.nodes.len(), 3);
        assert_eq!(read.nodes[1].opacity, 1.0);
        assert_eq!(read.nodes[0].layout.shrink, 1.0);
        assert_eq!(read.styles[0].weight, 400.0);
        let texts: Vec<_> = read.texts().map(|(node, text, role)| (node.name.as_str(), text.key.as_str(), role)).collect();
        assert_eq!(texts, [("title", "a-key", "body")]);
    }

    #[test]
    fn a_field_it_does_not_know_is_refused_with_its_line() {
        let text = document(r#"(name: "root", colour: (1, 2, 3, 4)),"#);
        let error = UiDocument::from_document(&text).expect_err("no such field");
        assert!(matches!(&error, UiError::Malformed { text, .. } if text.contains("colour")), "{error}");
    }

    #[test]
    fn each_check_names_its_field() {
        let cases: [(&str, &str); 19] = [
            (r#"(name: "root", parent: "x"),"#, "nodes[0] `root`.parent"),
            (r#"(name: "root"),(name: "a"),"#, "nodes[1] `a`.parent"),
            (r#"(name: "root"),(name: "a", parent: "b"),(name: "b", parent: "root"),"#, "nodes[1] `a`.parent"),
            (r#"(name: "root"),(name: "root", parent: "root"),"#, "nodes[1] `root`.name"),
            (r#"(name: ""),"#, "nodes[0] ``.name"),
            (r#"(name: "root", opacity: 2),"#, "nodes[0] `root`.opacity"),
            (r#"(name: "root", layout: (width: Points(inf))),"#, "nodes[0] `root`.layout.width"),
            (r#"(name: "root", layout: (gap: -1)),"#, "nodes[0] `root`.layout.gap"),
            (r#"(name: "root", layout: (aspect: 0)),"#, "nodes[0] `root`.layout.aspect"),
            (r#"(name: "root", layout: (cell: (column: 0, row: 1))),"#, "nodes[0] `root`.layout.cell"),
            (r#"(name: "root", layout: (columns: [Fr(-1)])),"#, "nodes[0] `root`.layout.columns"),
            (r#"(name: "root", look: (shape: Rect, fill: Color((1,2,3,4)), outline: (color: (1,2,3,4), width: 1))),"#, "nodes[0] `root`.look.outline"),
            (r#"(name: "root", look: (shape: Slice(sprite: Bar, corner: 0), fill: Color((1,2,3,4)))),"#, "nodes[0] `root`.look.shape.corner"),
            (r#"(name: "root", look: (shape: Rect, fill: Color((1,2,3,4)), slant: 9)),"#, "nodes[0] `root`.look.slant"),
            (r#"(name: "root", text: (key: "", style: "label", color: (1,2,3,4))),"#, "nodes[0] `root`.text.key"),
            (r#"(name: "root", text: (key: "k", style: "nope", color: (1,2,3,4))),"#, "nodes[0] `root`.text.style"),
            (r#"(name: "root", text: (key: "k", style: "label", color: (1,2,3,4), args: [(name: "a", path: "x"), (name: "a", path: "y")])),"#, "nodes[0] `root`.text.args[1].name"),
            (r#"(name: "root", binds: [(to: Fill, path: "a..b")]),"#, "nodes[0] `root`.binds[0].path"),
            (r#"(name: "root", binds: [(to: Fill, path: "a"), (to: Fill, path: "b")]),"#, "nodes[0] `root`.binds[1].to"),
        ];
        for (nodes, field) in cases {
            assert_eq!(one_issue(nodes).field, field, "{nodes}");
        }
    }

    #[test]
    fn styles_are_checked_too() {
        let text = |style: &str| {
            format!(
                r#"(type: "sapphire::assets::UiDocument", version: 1, data: (styles: [{style}], nodes: [(name: "root")]))"#
            )
        };
        let field = |style: &str| match UiDocument::from_document(&text(style)) {
            Err(UiError::Field(issue)) => issue.field,
            other => panic!("{other:?}"),
        };
        assert_eq!(field(r#"(name: "a", role: "body", size: 15)"#), "styles[0].size");
        assert_eq!(field(r#"(name: "a", role: "", size: 14)"#), "styles[0].role");
        assert_eq!(field(r#"(name: "a", role: "body", size: 14, weight: 1000)"#), "styles[0].weight");
        assert_eq!(field(r#"(name: "a", role: "body", size: 14, line_height: 0)"#), "styles[0].line_height");
        assert_eq!(
            field(r#"(name: "a", role: "body", size: 14),(name: "a", role: "body", size: 14)"#),
            "styles[1].name",
        );
    }

    #[test]
    fn a_bind_with_no_model_is_refused() {
        let text = r#"(type: "sapphire::assets::UiDocument", version: 1, data: (nodes: [(name: "root", binds: [(to: Fill, path: "a")])]))"#;
        match UiDocument::from_document(text) {
            Err(UiError::Field(issue)) => {
                assert_eq!(issue.field, "nodes[0] `root`");
                assert!(issue.problem.contains("no model"), "{issue}");
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn too_many_nodes_and_too_deep_are_refused() {
        let mut document = UiDocument { model: None, styles: Vec::new(), nodes: Vec::new() };
        document.nodes.push(UiNode { name: "n0".to_owned(), opacity: 1.0, ..UiNode::default() });
        for level in 1..=MAX_DEPTH {
            document.nodes.push(UiNode {
                name: format!("n{level}"),
                parent: Some(format!("n{}", level - 1)),
                opacity: 1.0,
                ..UiNode::default()
            });
        }
        assert!(document.issues().iter().any(|issue| issue.problem.contains("levels deep")));
        document.nodes = vec![UiNode { name: "n".to_owned(), opacity: 1.0, ..UiNode::default() }; MAX_NODES + 1];
        assert!(document.issues().iter().any(|issue| issue.field == "nodes" && issue.problem.contains("past")));
    }

    #[derive(Reflect)]
    struct Fighter {
        name: String,
        health: f32,
        alive: bool,
        state: Stance,
    }

    #[derive(Reflect)]
    enum Stance {
        Idle,
        Guard,
    }

    #[derive(Reflect)]
    struct Model {
        level: u32,
        player: Fighter,
        enemy: Option<Fighter>,
        party: Vec<Fighter>,
        pair: (i32, f64),
    }

    #[test]
    fn a_model_comes_from_a_type_s_reflection() {
        let model = UiModel::of_type(Model::type_info());
        let kind = |path: &str| model.kind_of(path);
        assert_eq!(kind("level"), Some(UiValueKind::Number));
        assert_eq!(kind("player.name"), Some(UiValueKind::Text));
        assert_eq!(kind("player.alive"), Some(UiValueKind::Bool));
        assert_eq!(kind("player.state"), Some(UiValueKind::Text), "a fieldless enum reads by name");
        assert_eq!(kind("enemy.0.health"), Some(UiValueKind::Number), "through an Option");
        assert_eq!(kind("party[3].health"), Some(UiValueKind::Number), "any index");
        assert_eq!(kind("pair.1"), Some(UiValueKind::Number));
        assert_eq!(kind("player"), None, "a struct has no value to show");
        assert_eq!(kind("player.mana"), None);
        let _ = (Stance::Idle, Stance::Guard);
    }

    #[test]
    fn a_model_round_trips_its_document() {
        let model = UiModel::of_type(Model::type_info());
        let text = to_ron(&model).expect("writes");
        assert_eq!(UiModel::from_document(&text).expect("reads"), model);
    }

    #[test]
    fn paths_are_checked_against_a_model_by_field_and_kind() {
        let model = UiModel::of_type(Model::type_info());
        let document = UiDocument::from_document(&document(concat!(
            r#"(name: "root", binds: [(to: Shown, path: "player.alive"), (to: Fill, path: "player.alive"), (to: Opacity, path: "player.mana")],"#,
            r#"text: (key: "k", style: "label", color: (1,2,3,4), args: [(name: "a", path: "player.health"), (name: "b", path: "nope")])),"#,
        )))
        .expect("the paths are paths");
        let issues = document.path_issues(&model);
        let fields: Vec<_> = issues.iter().map(|issue| issue.field.as_str()).collect();
        assert_eq!(
            fields,
            [
                "nodes[0] `root`.binds[1].path",
                "nodes[0] `root`.binds[2].path",
                "nodes[0] `root`.text.args[1].path",
            ],
        );
        assert!(issues[0].problem.contains("reads a Number"), "{}", issues[0]);
        assert!(issues[2].problem.contains("`nope`"), "{}", issues[2]);
    }

    fn cooked() -> Vec<u8> {
        let text = document(r#"(name: "root"),(name: "title", parent: "root", text: (key: "k", style: "label", color: (1,2,3,4))),"#);
        UiDocument::from_document(&text).expect("good").to_cooked().expect("cooks").to_vec()
    }

    fn aligned(bytes: &[u8]) -> AlignedVec {
        let mut out = AlignedVec::with_capacity(bytes.len());
        out.extend_from_slice(bytes);
        out
    }

    #[test]
    fn cooked_bytes_come_back_as_they_went_in() {
        let bytes = aligned(&cooked());
        let read = UiDocument::read(&bytes).expect("reads");
        let owned = read.to_owned_document().expect("converts");
        assert_eq!(owned.nodes[1].text.as_ref().map(|text| text.key.as_str()), Some("k"));
    }

    #[test]
    fn hostile_bytes_are_refused_or_read_as_a_checked_document() {
        let good = cooked();
        for length in 0..good.len() {
            assert!(UiDocument::read(&aligned(&good[..length])).is_err(), "truncated to {length}");
        }
        for at in 0..good.len() {
            for flip in [0x01u8, 0x80, 0xff] {
                let mut bad = good.clone();
                bad[at] ^= flip;
                // Most flips are refused. One that lands in a text is another good document, and one
                // that reads has passed every check a document's fields have.
                if let Ok(read) = UiDocument::read(&aligned(&bad)) {
                    assert!(read.to_owned_document().expect("converts").issues().is_empty());
                }
            }
        }
    }

    #[test]
    fn the_wrong_type_or_a_new_version_is_refused() {
        let wrong = r#"(type: "sapphire::assets::Cue", version: 1, data: (nodes: []))"#;
        assert!(matches!(UiDocument::from_document(wrong), Err(UiError::Document(_))));
        let newer = r#"(type: "sapphire::assets::UiDocument", version: 99, data: (nodes: []))"#;
        assert!(matches!(UiDocument::from_document(newer), Err(UiError::Document(_))));
    }
}
