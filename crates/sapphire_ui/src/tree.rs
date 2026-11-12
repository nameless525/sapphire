//! The retained tree (M3 U4, spec §5.17): a [`UiDocument`] run as a screen. Every screen of the game
//! stands on it, and its output is plain quads and glyphs in the frame's [`UiDrawList`], so the renderer
//! sees nothing of nodes, layout or text (rule R4).
//!
//! # A frame
//!
//! ```ignore
//! // Once a frame, with what the renderer's glyph pages hold and the language in force.
//! fonts.begin_frame(renderer.ui_page_state(), &localizer);
//! // Each tree: the model's values, strings and layout, of which only what changed costs anything.
//! tree.input(&input, now);
//! tree.update(&UiFrame { metrics, now, loc: &localizer, model: Some(&model) }, &mut fonts);
//! tree.draw(&mut fonts, &atlas, &mut packet.ui);
//! for event in tree.drain_events() { /* ... */ }
//! ```
//!
//! # What's retained
//!
//! `taffy`'s tree and every node's rectangle, resolved string and last bound value. A frame reads each
//! bound value through its reflection path and compares it with the last, which allocates nothing. Only
//! a value that moved resolves its string again or marks its box for layout, and the layout pass runs
//! only if something is marked, or the frame's metrics or language moved. The quads are written again
//! every frame, since drawing a glyph is also what tells the atlas it's still in use, at the cost of a
//! hash a glyph (`tests/tree_allocations.rs` holds the frame to no allocations, and spec §5.17 has its cost).
//!
//! # When it's wrong
//!
//! A path that doesn't resolve against the model, or a role no font chain answers, is a [`UiProblem`]
//! with the node and field: logged once, counted in [`UiTreeStats::problems`] (the overlay's "Interface"
//! row), and drawn, as a frame in [`PROBLEM_COLOR`] around the node, with `{the.path}` standing where
//! its value would. A cook refuses both before a shipped build can meet them (`sapphire_pipeline::ui`),
//! so this is what a pushed document or an edit in progress shows. A path that the model's type has and
//! whose value is absent right now, through an `Option` holding nothing or past a list's end, isn't
//! wrong: its node reads as hidden, transparent, empty. Paths are checked against the model's type (and the
//! names a dynamic field of it holds), or, for a model of dynamic values with no type, the model document
//! it declares ([`UiTree::declare_model`]).
//!
//! A key the tables don't hold shows as itself and counts in `LocStats`. Under pseudo-localization,
//! [`UiTree::unframed`] finds it, along with anything else that didn't come through a message.
//!
//! # Time and motion
//!
//! The app hands in `now`, on whatever clock it likes: the tree reads none (rule R2), and nothing here
//! touches the simulation. Timelines, a screen's machine and scrolling are all functions of that time
//! (`motion`, `scroll`), so a capture of a given time is the same bytes however it got there. Motion
//! changes a node's opacity, offset and scale, never its layout, so a fade or a scroll costs no layout
//! pass and allocates nothing once warm (`tests/tree_allocations.rs`). Text about to appear warms its
//! glyphs while it's still invisible, a few a frame, so the frame it shows has nothing to rasterize.
//!
//! # Focus
//!
//! `focus` has what a keyboard, a pad and a pointer do. A screen's own input stops at [`UiEvent`]s, and
//! what they mean is the game's.
//!
//! # Units and pixels
//!
//! Layout runs in UI points with `taffy`'s rounding off, and rectangles snap to whole pixels as they
//! draw, so one document lays out alike at any scale and edges stay crisp. Text sizes are whole pixels
//! ([`TYPE_SCALE`](sapphire_text::TYPE_SCALE)).

mod bind;
mod focus;
mod inset;
mod motion;
mod paint;
mod scroll;
mod style;

use std::any::TypeId;

use sapphire_assets::font::Font;
use sapphire_assets::ui::{
    UiBindTarget, UiColor, UiDocument, UiLayout, UiLook, UiModel, UiScrollAxis, UiTextStyle, UiValueKind,
};
use sapphire_loc::{Arg, Localizer};
use sapphire_look::{UiDrawList, UiPageState, UiTreeStats};
use sapphire_math::Vec2;
use sapphire_platform::Timestamp;
use sapphire_reflect::PartialReflect;
use sapphire_reflect::bevy_reflect::ParsedPath;
use sapphire_text::{
    AtlasConfig, FaceOptions, FieldError, FieldFace, FieldGlyph, GlyphAtlas, TextStyle, TextSystem,
    family_list,
};
use sapphire_time::Stopwatch;
use taffy::prelude::{AvailableSpace, Display, NodeId, Size as TaffySize, Style as TaffyStyle, TaffyTree};
use taffy::{LayoutInput, compute_leaf_layout};

use self::bind::{BoundText, read};
use self::focus::Hold;
pub use self::focus::{UiEvent, UiHit, UiInput, UiPointer, UiPointerKind, UiPointerPhase, UiStep};
pub use self::inset::UiInsetShot;
use self::motion::{Placement, Player, Pose};
use self::scroll::Scroll;
use crate::atlas::Atlas;
use crate::layout::{Insets, Rect, UiMetrics};

/// A cooked font as `sapphire_text` takes it: its file, and its baseline shift and field (M3 U8).
pub fn face_of(font: Font) -> Result<(std::sync::Arc<Vec<u8>>, FaceOptions), FieldError> {
    let Font { baseline_shift, file, field, .. } = font;
    let field = field
        .map(|field| {
            let atlas = sapphire_look::UiAtlas {
                width: u32::from(field.width),
                height: u32::from(field.height),
                spread: field.spread,
                format: sapphire_look::UiFieldFormat::Multi,
                texels: field.texels,
            };
            let glyphs = field
                .glyphs
                .iter()
                .map(|glyph| FieldGlyph {
                    id: glyph.id,
                    cell: [glyph.x, glyph.y, glyph.width, glyph.height],
                    offset: [glyph.left, glyph.top],
                })
                .collect();
            let atlas = std::sync::Arc::new(atlas);
            FieldFace::new(atlas, f32::from(field.em), field.coords, glyphs).map(std::sync::Arc::new)
        })
        .transpose()?;
    Ok((std::sync::Arc::new(file), FaceOptions { baseline_shift, field }))
}

/// The color a node with something wrong is framed in: nothing a screen would pick on purpose.
pub const PROBLEM_COLOR: UiColor = [255, 0, 200, 255];

/// The most glyphs invisible text rasterizes in a frame, across every tree: a screen's worth of new text
/// over a fade of a few frames, at a tenth of a millisecond or so.
const WARM_PER_FRAME: u32 = 64;

/// The field a bound key's problems are said of.
const KEY_FROM_PATH: &str = "text.key_from.path";

/// Fonts, shaping and the glyph atlas, which every tree on screen shares.
#[derive(Debug)]
pub struct UiFonts {
    pub text: TextSystem,
    pub atlas: GlyphAtlas,
    /// A role's family list for the language in force, made the first time something asks.
    families: Vec<(Box<str>, Option<String>)>,
    /// What [`Self::families`] was made for.
    generation: u64,
    /// What a tree's layout depends on here: the language and the fonts.
    revision: u64,
    locale: String,
    /// What this frame may still rasterize for text that doesn't show yet.
    warm_left: u32,
}

impl Default for UiFonts {
    fn default() -> Self {
        Self::new(AtlasConfig::default())
    }
}

impl UiFonts {
    pub fn new(config: AtlasConfig) -> Self {
        Self {
            text: TextSystem::new(),
            atlas: GlyphAtlas::new(config),
            families: Vec::new(),
            generation: u64::MAX,
            revision: 0,
            locale: String::new(),
            warm_left: WARM_PER_FRAME,
        }
    }

    /// Adds a font, as a cooked font's `file` and `baseline_shift` give it. What's laid out lays out
    /// again.
    pub fn register_font(&mut self, file: std::sync::Arc<Vec<u8>>, baseline_shift: f32) {
        self.text.register_face(file, FaceOptions { baseline_shift, ..FaceOptions::default() });
        self.revision += 1;
    }

    /// Adds a cooked font with all it carries: its baseline shift and, for a display face, its field
    /// (M3 U8), which large text in it draws from. What's laid out lays out again.
    pub fn register(&mut self, font: Font) -> Result<(), FieldError> {
        let (file, options) = face_of(font)?;
        self.text.register_face(file, options);
        self.revision += 1;
        Ok(())
    }

    /// Moves to the next frame, given what the renderer's glyph pages hold and the language in force.
    pub fn begin_frame(&mut self, pages: UiPageState, loc: &Localizer) {
        self.text.begin_frame();
        self.atlas.begin_frame(pages);
        self.warm_left = WARM_PER_FRAME;
        if self.generation != loc.generation() {
            self.generation = loc.generation();
            self.families.clear();
            self.locale.clear();
            self.locale.push_str(loc.text_locale());
            self.revision += 1;
        }
    }

    /// Where `role`'s family list is for the language in force, if a font chain answers. A locale's
    /// `font_chain` says the order, as spec §5.17 has it: the families and the order text shows in are
    /// the locale's, and `FontChains` only says which files to load for them.
    fn family_index(&mut self, role: &str, loc: &Localizer) -> Option<u16> {
        let at = match self.families.iter().position(|(known, _)| **known == *role) {
            Some(at) => at,
            None => {
                let list = loc.font_chain(role).map(|chain| family_list(chain.iter().map(String::as_str)));
                self.families.push((role.into(), list));
                self.families.len() - 1
            }
        };
        self.families[at].1.is_some().then_some(at as u16)
    }
}

/// What a frame's update reads.
#[derive(Clone, Copy)]
pub struct UiFrame<'a> {
    pub metrics: UiMetrics,
    /// The time motion reads, on any clock that only goes forward.
    pub now: Timestamp,
    pub loc: &'a Localizer,
    /// What the document's paths read. A document that binds nothing needs none.
    pub model: Option<&'a dyn PartialReflect>,
}

/// Something wrong with a tree that a person should see: see the [module docs](self).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UiProblem {
    /// The node's name.
    pub node: String,
    /// The field, as a document names it.
    pub field: String,
    pub problem: String,
}

impl std::fmt::Display for UiProblem {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "`{}`.{}: {}", self.node, self.field, self.problem)
    }
}

/// A node, by where it is in its tree. It stays good until the tree is replaced.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct UiNodeId(u32);

/// One bound property: a path, and what it last read.
struct Bound {
    target: UiBindTarget,
    source: Box<str>,
    path: ParsedPath,
    failed: bool,
}

/// How a node with `focus` answers, by where its names are in the tree.
struct FocusRule {
    up: Option<u32>,
    down: Option<u32>,
    left: Option<u32>,
    right: Option<u32>,
    focused: Option<UiLook>,
    pressed: Option<UiLook>,
    fires: Option<Box<str>>,
}

/// One node as it runs.
struct Node {
    name: Box<str>,
    parent: Option<u32>,
    id: NodeId,
    layout: UiLayout,
    look: Option<UiLook>,
    mask: bool,
    opacity: f32,
    text: Option<BoundText>,
    binds: Vec<Bound>,
    /// What the model says right now.
    shown: bool,
    bound_opacity: f32,
    fill: f32,
    /// What the model says its text's size is multiplied by ([`UiBindTarget::TextScale`]).
    bound_text_scale: f32,
    /// Has a problem, and draws a frame saying so.
    failed: bool,
    focus: Option<FocusRule>,
    scroll: Option<Scroll>,
    /// Its parent scrolls, so it keeps its size rather than shrinking to fit.
    scrolled: bool,
    /// Where layout put it, in points from the frame's top-left.
    rect: Rect,
    /// Inside its padding.
    content: Rect,
    /// False under a node the model hides.
    visible: bool,
    /// What timelines that have ended left on it.
    held: Pose,
    /// What timelines say of it this frame.
    pose: Pose,
    /// From [`Self::rect`] to the screen this frame, for its own quads and for its children's.
    placed: Placement,
    child_placed: Placement,
    /// Where it is on the screen this frame.
    screen: Rect,
    /// The mask its own quads draw under, and the one its children's do.
    own_mask: Option<[f32; 4]>,
    child_mask: Option<[f32; 4]>,
    /// Multiplied down from the root, for this frame.
    final_opacity: f32,
    /// Shows a 3D inset's picture over its look.
    inset: Option<inset::InsetState>,
}

impl Node {
    /// Whether it has a problem now, from any cause: what its frame says.
    fn has_problem(&self) -> bool {
        self.binds.iter().any(|bind| bind.failed) || self.text.as_ref().is_some_and(BoundText::has_problem)
    }
}

/// Whether `rect` leaves `holder` by more than `slack`, along the axes asked about.
fn outside(rect: Rect, holder: Rect, along_x: bool, along_y: bool, slack: f32) -> bool {
    let leaves = |low: f32, high: f32, holder_low: f32, holder_high: f32| {
        low < holder_low - slack || high > holder_high + slack
    };
    (along_x && leaves(rect.min.x, rect.max.x, holder.min.x, holder.max.x))
        || (along_y && leaves(rect.min.y, rect.max.y, holder.min.y, holder.max.y))
}

/// A screen: see the [module docs](self).
pub struct UiTree {
    taffy: TaffyTree<u32>,
    /// In drawing order, each node's parent before it, and its subtree right after.
    nodes: Vec<Node>,
    root: NodeId,
    styles: Vec<UiTextStyle>,
    problems: Vec<UiProblem>,
    /// What the paths were last checked against, or none yet: whether there was a model, and its type if
    /// it has one (a model of dynamic values has none).
    checked_model: Option<(bool, Option<TypeId>)>,
    /// What the paths are checked against in place of the model's type, if [`Self::declare_model`] said.
    declared: Option<UiModel>,
    metrics: Option<UiMetrics>,
    revision: Option<u64>,
    dirty: bool,
    scale: f32,
    /// What every text's size is multiplied by: the player's text size ([`Self::set_text_scale`]).
    text_scale: f32,
    size: Vec2,
    scratch: String,
    player: Player,
    /// Those that take focus, in drawing order.
    focusable: Vec<u32>,
    /// Where focus starts, and where it is.
    first: Option<u32>,
    focus: Option<u32>,
    focus_shown: bool,
    /// The pointer that's down, of those the tree follows: one at a time.
    hold: Option<Hold>,
    /// Where the mouse last was.
    hover: Option<Vec2>,
    events: Vec<UiEvent>,
    stats: UiTreeStats,
    /// How long this frame's update took.
    update_cost: std::time::Duration,
}

impl std::fmt::Debug for UiTree {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("UiTree")
            .field("nodes", &self.nodes.len())
            .field("problems", &self.problems)
            .finish_non_exhaustive()
    }
}

/// A tree that can't be built: only a document that didn't pass its own checks.
#[derive(Debug, thiserror::Error)]
pub enum UiTreeError {
    #[error(transparent)]
    Issue(#[from] sapphire_assets::ui::UiIssue),
    #[error("layout: {0}")]
    Layout(String),
}

impl UiTree {
    /// Builds a tree from `document`, whose checks it runs again. A hot reload makes a new one and
    /// keeps the old if this fails.
    pub fn new(document: &UiDocument) -> Result<Self, UiTreeError> {
        if let Some(issue) = document.issues().into_iter().next() {
            return Err(issue.into());
        }
        let order = drawing_order(document);
        let mut taffy: TaffyTree<u32> = TaffyTree::with_capacity(document.nodes.len());
        // Points, snapped as they draw, so a layout is the same at any scale.
        taffy.disable_rounding();
        let layout_error = |error: taffy::TaffyError| UiTreeError::Layout(error.to_string());
        let mut nodes: Vec<Node> = Vec::with_capacity(order.len());
        let mut parents: Vec<Option<u32>> = vec![None; document.nodes.len()];
        for (at, &source) in order.iter().enumerate() {
            let node = &document.nodes[source];
            let parent = node.parent.as_deref().and_then(|name| {
                document.nodes.iter().position(|other| other.name == name).and_then(|at| parents[at])
            });
            parents[source] = Some(at as u32);
            let style = style::taffy_style(&node.layout, Insets::default());
            let id = if node.text.is_some() {
                taffy.new_leaf_with_context(style, at as u32).map_err(layout_error)?
            } else {
                taffy.new_leaf(style).map_err(layout_error)?
            };
            if let Some(parent) = parent {
                taffy.add_child(nodes[parent as usize].id, id).map_err(layout_error)?;
            }
            let binds = node
                .binds
                .iter()
                .map(|bind| {
                    Ok(Bound {
                        target: bind.to,
                        source: bind.path.as_str().into(),
                        path: ParsedPath::parse(&bind.path)
                            .map_err(|error| UiTreeError::Layout(error.to_string()))?,
                        failed: false,
                    })
                })
                .collect::<Result<Vec<_>, UiTreeError>>()?;
            let text = node
                .text
                .as_ref()
                .map(|text| {
                    let style = document.styles.iter().position(|style| style.name == text.style);
                    BoundText::new(text, style.unwrap_or(0) as u16)
                })
                .transpose()?;
            nodes.push(Node {
                name: node.name.as_str().into(),
                parent,
                id,
                layout: node.layout.clone(),
                look: node.look,
                mask: node.mask,
                opacity: node.opacity,
                text,
                binds,
                shown: true,
                bound_opacity: 1.0,
                fill: 1.0,
                bound_text_scale: 1.0,
                failed: false,
                focus: None,
                scroll: node.scroll.map(Scroll::new),
                scrolled: parent.is_some_and(|parent: u32| nodes[parent as usize].scroll.is_some()),
                rect: Rect::default(),
                content: Rect::default(),
                visible: true,
                held: Pose::REST,
                pose: Pose::REST,
                placed: Placement::IDENTITY,
                child_placed: Placement::IDENTITY,
                screen: Rect::default(),
                own_mask: None,
                child_mask: None,
                final_opacity: 1.0,
                inset: node.inset.as_ref().map(inset::InsetState::new),
            });
        }
        let root =
            nodes.first().map(|node| node.id).ok_or_else(|| UiTreeError::Layout("no root".to_owned()))?;
        let at_name = |name: &str| parents[document.nodes.iter().position(|node| node.name == name)?];
        let mut focusable = Vec::new();
        let mut first = None;
        for (at, &source) in order.iter().enumerate() {
            let Some(focus) = &document.nodes[source].focus else { continue };
            let target = |name: &Option<String>| name.as_deref().and_then(at_name);
            nodes[at].focus = Some(FocusRule {
                up: target(&focus.up),
                down: target(&focus.down),
                left: target(&focus.left),
                right: target(&focus.right),
                focused: focus.focused,
                pressed: focus.pressed,
                fires: focus.fires.as_deref().map(Into::into),
            });
            focusable.push(at as u32);
            if focus.first {
                first = Some(at as u32);
            }
        }
        let player = Player::new(document, at_name);
        Ok(Self {
            taffy,
            nodes,
            root,
            styles: document.styles.clone(),
            problems: Vec::new(),
            checked_model: None,
            declared: None,
            metrics: None,
            revision: None,
            dirty: true,
            scale: 1.0,
            text_scale: 1.0,
            size: Vec2::ZERO,
            scratch: String::new(),
            player,
            first: first.or(focusable.first().copied()),
            focus: first.or(focusable.first().copied()),
            focusable,
            focus_shown: true,
            hold: None,
            hover: None,
            // Most frames add nothing, and a walk or a mash adds a few.
            events: Vec::with_capacity(16),
            stats: UiTreeStats::default(),
            update_cost: std::time::Duration::ZERO,
        })
    }

    /// The node called `name`.
    #[must_use]
    pub fn find(&self, name: &str) -> Option<UiNodeId> {
        self.nodes.iter().position(|node| *node.name == *name).map(|at| UiNodeId(at as u32))
    }

    /// Where `node` was last laid out, in points from the frame's top-left, before snapping, and with
    /// no timeline or scroll on it.
    #[must_use]
    pub fn rect(&self, node: UiNodeId) -> Rect {
        self.nodes[node.0 as usize].rect
    }

    /// Where `node` was last drawn: timelines and scrolling on it, and its parents'.
    #[must_use]
    pub fn screen_rect(&self, node: UiNodeId) -> Rect {
        self.nodes[node.0 as usize].screen
    }

    /// How opaque `node` was last drawn, with all that's over it.
    #[must_use]
    pub fn opacity(&self, node: UiNodeId) -> f32 {
        self.nodes[node.0 as usize].final_opacity
    }

    /// `node`'s name.
    #[must_use]
    pub fn name(&self, node: UiNodeId) -> &str {
        &self.nodes[node.0 as usize].name
    }

    /// Starts a timeline from its beginning. Whether the document has one by that name.
    pub fn play(&mut self, timeline: &str, now: Timestamp) -> bool {
        self.player.play(timeline, now, &mut self.nodes)
    }

    /// Stops a timeline where it is, its values staying. Whether it was running.
    pub fn stop(&mut self, timeline: &str, now: Timestamp) -> bool {
        self.player.stop(timeline, now, &mut self.nodes)
    }

    /// Gives the screen's machine an event. Whether it moved: an event with no transition from the
    /// current state does nothing.
    pub fn fire(&mut self, event: &str, now: Timestamp) -> bool {
        self.player.fire(event, now, &mut self.nodes)
    }

    /// The state the screen's machine is in, if the document has one.
    #[must_use]
    pub fn state(&self) -> Option<&str> {
        self.player.state()
    }

    /// Whether anything is moving, as of the last update. A screen that's hidden and still can stop being
    /// updated.
    #[must_use]
    pub fn is_moving(&self) -> bool {
        self.stats.playing > 0 || self.stats.scrolling > 0
    }

    /// How far `node` has scrolled, as of the last update.
    #[must_use]
    pub fn scroll_offset(&self, node: UiNodeId) -> Option<f32> {
        let node = &self.nodes[node.0 as usize];
        node.scroll.as_ref().map(|scroll| {
            -scroll.along(node.child_placed.offset - node.placed.offset) / node.placed.scale.max(f32::EPSILON)
        })
    }

    /// Whether `node` and everything over it are shown.
    #[must_use]
    pub fn is_visible(&self, node: UiNodeId) -> bool {
        self.nodes[node.0 as usize].visible
    }

    /// Checks the paths against `model` from the next update, rather than against the model's type: how a
    /// model of dynamic values, such as an editor's stand-ins, which has no type to read, says what it holds.
    pub fn declare_model(&mut self, model: UiModel) {
        self.declared = Some(model);
        self.checked_model = None;
    }

    /// What's wrong with this tree: see the [module docs](self).
    #[must_use]
    pub fn problems(&self) -> &[UiProblem] {
        &self.problems
    }

    /// Every text node's name and the string it last resolved.
    pub fn strings(&self) -> impl Iterator<Item = (&str, &str)> {
        self.nodes
            .iter()
            .filter_map(|node| node.text.as_ref().map(|text| (&*node.name, text.resolved.as_str())))
    }

    /// The text nodes whose strings didn't come through a message. Only means something with the
    /// localizer's pseudo mode on, under which every message comes back framed: this is how a whole
    /// screen is searched for what isn't localized, and how a key missing from the tables shows.
    pub fn unframed(&self) -> impl Iterator<Item = (&str, &str)> {
        self.strings().filter(|(_, string)| !sapphire_loc::is_framed(string))
    }

    /// The shown text nodes that reach outside what holds them, or outside the frame, by more than half a
    /// point, each with the name of what it leaves: a string too long for its screen. What a scroller holds
    /// may run on along its axis. Goes by layout, so neither timelines nor scrolling move it.
    #[must_use]
    pub fn overflowing(&self) -> Vec<(&str, &str)> {
        const SLACK: f32 = 0.5;
        let mut found = Vec::new();
        for node in self.nodes.iter().filter(|node| node.text.is_some() && node.visible) {
            let rect = node.rect;
            let (mut along_x, mut along_y) = (true, true);
            let mut holder = node.parent;
            let mut left = None;
            while let Some(at) = holder {
                let parent = &self.nodes[at as usize];
                match parent.scroll.map(|scroll| scroll.axis) {
                    Some(UiScrollAxis::Vertical) => along_y = false,
                    Some(UiScrollAxis::Horizontal) => along_x = false,
                    None => {}
                }
                if outside(rect, parent.rect, along_x, along_y, SLACK) {
                    left = Some(&*parent.name);
                    break;
                }
                holder = parent.parent;
            }
            let frame = Rect::new(Vec2::ZERO, self.size);
            if left.is_none() && outside(rect, frame, along_x, along_y, SLACK) {
                left = Some("the frame");
            }
            if let Some(left) = left {
                found.push((&*node.name, left));
            }
        }
        found
    }

    /// Reads the model, resolves what changed and lays out what moved. Allocates nothing if nothing did.
    pub fn update(&mut self, frame: &UiFrame<'_>, fonts: &mut UiFonts) {
        profiling::scope!("UiTree::update");
        let stopwatch = Stopwatch::start();
        self.stats = UiTreeStats {
            trees: 1,
            nodes: self.nodes.len() as u32,
            problems: self.problems.len() as u32,
            ..UiTreeStats::default()
        };
        self.player.advance(frame.now, &mut self.nodes);
        self.player.pose(frame.now, &mut self.nodes);
        self.check_model(frame.model);
        let new_language = self.revision != Some(fonts.revision);
        if new_language {
            self.revision = Some(fonts.revision);
            self.follow_language(frame.loc, fonts);
        }
        self.read_model(frame);
        self.resolve_strings(frame.loc);
        let metrics = UiMetrics { scale: frame.metrics.pixels_per_point(), ..frame.metrics };
        if self.metrics != Some(metrics) {
            self.metrics = Some(metrics);
            self.follow_metrics(metrics);
            self.scale = metrics.scale;
            self.size = metrics.size;
        }
        if self.dirty {
            self.dirty = false;
            self.lay_out(fonts);
        }
        self.resolve(frame.now);
        self.settle_focus();
        self.stats.playing = self.player.playing() as u32;
        self.stats.problems = self.problems.len() as u32;
        self.update_cost = stopwatch.elapsed();
    }

    /// Checks the paths against the model's type, the first time it's seen, with the names a dynamic field
    /// of that first value holds ([`UiModel::of_model`]): what a cook does with a model document, here
    /// with the real thing.
    fn check_model(&mut self, model: Option<&dyn PartialReflect>) {
        let info = model.and_then(PartialReflect::get_represented_type_info);
        let seen = (model.is_some(), info.map(|info| info.type_id()));
        if self.checked_model == Some(seen) {
            return;
        }
        self.checked_model = Some(seen);
        // Everything found last time was about the last model.
        self.problems.retain(|problem| !problem.field.contains("path"));
        let model = match (model, &self.declared) {
            (Some(_), Some(declared)) => Some(declared.clone()),
            // Its type's paths, and a dynamic field's names, which its type doesn't know.
            (Some(model), None) if info.is_some() => Some(UiModel::of_model(model)),
            _ => None,
        };
        for node in &mut self.nodes {
            let mut report = |field: String, problem: String| {
                tracing::error!(target: crate::LOG_TARGET, node = %node.name, field, problem, "a screen's binding");
                self.problems.push(UiProblem { node: node.name.to_string(), field, problem });
            };
            for (at, bind) in node.binds.iter_mut().enumerate() {
                let kind = model.as_ref().and_then(|model| model.kind_of(&bind.source));
                bind.failed = kind != Some(bind.target.kind());
                if bind.failed {
                    report(
                        format!("binds[{at}].path"),
                        match (&model, kind) {
                            (None, _) => format!("`{}`: there's no model to read", bind.source),
                            (_, Some(kind)) => format!(
                                "`{}` is a {kind:?}, and {:?} reads a {:?}",
                                bind.source,
                                bind.target,
                                bind.target.kind()
                            ),
                            (_, None) => format!("`{}` isn't in the model", bind.source),
                        },
                    );
                }
            }
            if let Some(text) = &mut node.text {
                for (at, arg) in text.args.iter_mut().enumerate() {
                    let known = model.as_ref().is_some_and(|model| model.kind_of(&arg.source).is_some());
                    arg.set_failed(!known);
                    if !known {
                        report(
                            format!("text.args[{at}].path"),
                            match &model {
                                None => format!("`{}`: there's no model to read", arg.source),
                                Some(_) => format!("`{}` isn't in the model", arg.source),
                            },
                        );
                    }
                }
                if let Some(from) = &mut text.key_from {
                    let kind = model.as_ref().and_then(|model| model.kind_of(&from.source));
                    from.failed = kind != Some(UiValueKind::Text);
                    // Its key's problems went with the last model's: judge it again.
                    from.told = false;
                    if from.failed {
                        report(
                            KEY_FROM_PATH.to_owned(),
                            match (&model, kind) {
                                (None, _) => format!("`{}`: there's no model to read", from.source),
                                (_, Some(kind)) => {
                                    format!("`{}` is a {kind:?}, and a key is a Text", from.source)
                                }
                                (_, None) => format!("`{}` isn't in the model", from.source),
                            },
                        );
                    }
                }
                text.stale = true;
            }
            node.failed = node.has_problem();
        }
    }

    /// Resolves every string and each style's fonts again, and lays out again.
    fn follow_language(&mut self, loc: &Localizer, fonts: &mut UiFonts) {
        self.problems.retain(|problem| !problem.field.ends_with("style"));
        for node in &mut self.nodes {
            let Some(text) = &mut node.text else { continue };
            text.stale = true;
            let role = &self.styles[usize::from(text.style)].role;
            text.family = fonts.family_index(role, loc);
            if text.family.is_none() {
                let (field, problem) = (
                    "text.style".to_owned(),
                    format!("no font chain of `{}` answers to the role `{role}`", loc.text_locale()),
                );
                tracing::error!(target: crate::LOG_TARGET, node = %node.name, field, problem, "a screen's text");
                self.problems.push(UiProblem { node: node.name.to_string(), field, problem });
            }
            node.failed = node.has_problem();
            self.taffy.mark_dirty(node.id).expect("the tree's own node");
        }
        self.dirty = true;
    }

    /// Compares each bound value with the last. Only what moved costs more than that.
    fn read_model(&mut self, frame: &UiFrame<'_>) {
        let Some(model) = frame.model else { return };
        for (at, node) in self.nodes.iter_mut().enumerate() {
            for bind in &node.binds {
                if bind.failed {
                    continue;
                }
                let value = read(&bind.path, model);
                match bind.target {
                    UiBindTarget::Shown => {
                        let shown = value.as_bool();
                        if shown != node.shown {
                            node.shown = shown;
                            self.stats.bound_changes += 1;
                            let style = node_style(node, at == 0, self.metrics);
                            self.taffy.set_style(node.id, style).expect("the tree's own node");
                            self.dirty = true;
                        }
                    }
                    UiBindTarget::Opacity => {
                        let share = value.as_share();
                        if share != node.bound_opacity {
                            node.bound_opacity = share;
                            self.stats.bound_changes += 1;
                        }
                    }
                    UiBindTarget::Fill => {
                        let share = value.as_share();
                        if share != node.fill {
                            node.fill = share;
                            self.stats.bound_changes += 1;
                        }
                    }
                    UiBindTarget::TextScale => {
                        let scale = value.as_text_scale();
                        if scale != node.bound_text_scale {
                            node.bound_text_scale = scale;
                            self.stats.bound_changes += 1;
                            self.taffy.mark_dirty(node.id).expect("the tree's own node");
                            self.dirty = true;
                        }
                    }
                }
            }
            if let Some(text) = &mut node.text {
                self.stats.bound_changes += text.read(model);
            }
        }
    }

    /// Resolves the strings whose language or values moved.
    fn resolve_strings(&mut self, loc: &Localizer) {
        let scratch = &mut self.scratch;
        for node in &mut self.nodes {
            let Some(text) = &mut node.text else { continue };
            if !text.stale {
                continue;
            }
            text.stale = false;
            self.stats.resolves += 1;
            // A key read anew replaces what was said of the last: a problem only while it lasts.
            if let Some(verdict) = text.judge_chosen() {
                let name = &*node.name;
                self.problems.retain(|problem| !(problem.node == name && problem.field == KEY_FROM_PATH));
                if let Some(problem) = verdict {
                    let field = KEY_FROM_PATH.to_owned();
                    tracing::error!(target: crate::LOG_TARGET, node = %node.name, field, problem, "a screen's text");
                    self.problems.push(UiProblem { node: node.name.to_string(), field, problem });
                }
                node.failed = node.binds.iter().any(|bind| bind.failed) || text.has_problem();
            }
            let args: Vec<Arg<'_>> = text.args.iter().map(|arg| (&*arg.name, arg.value())).collect();
            let mut shown = std::mem::take(&mut text.resolved);
            let resolved = loc.resolve(text.current_key(), &args, scratch);
            if resolved != shown {
                shown.clear();
                shown.push_str(resolved);
                self.taffy.mark_dirty(node.id).expect("the tree's own node");
                self.dirty = true;
            }
            drop(args);
            text.resolved = shown;
        }
    }

    /// The frame's size in points, as of the last update.
    #[must_use]
    pub const fn size(&self) -> Vec2 {
        self.size
    }

    /// Multiplies every text's size by `scale` from the next update, over what its style says and any
    /// [`UiBindTarget::TextScale`]: the player's text size (spec §6.8, M4 A2). It lays out again only if
    /// it moved; one outside [`TEXT_SCALES`](sapphire_assets::ui::TEXT_SCALES) reads as its nearest end.
    pub fn set_text_scale(&mut self, scale: f32) {
        let scale = text_scale_within(scale);
        if scale == self.text_scale {
            return;
        }
        self.text_scale = scale;
        for node in self.nodes.iter().filter(|node| node.text.is_some()) {
            self.taffy.mark_dirty(node.id).expect("the tree's own node");
        }
        self.dirty = true;
    }

    /// Every text that shows, by its node's name, with the size it's set at in pixels, its style's times
    /// every scale on it, as of the last update: what a legibility floor is held to (M4 A2). A node out of
    /// the layout or faded right out isn't there, and neither is one that says nothing.
    pub fn text_pixels(&self) -> impl Iterator<Item = (&str, f32)> + '_ {
        self.nodes.iter().filter_map(|node| {
            let text = node.text.as_ref()?;
            let shows = node.visible && node.final_opacity > 0.0 && !text.resolved.trim().is_empty();
            let style = &self.styles[usize::from(text.style)];
            shows.then_some((&*node.name, style.size * self.text_scale * node.bound_text_scale * self.scale))
        })
    }

    /// New metrics: the root takes the frame's size, and what asks for the safe area its insets.
    fn follow_metrics(&mut self, metrics: UiMetrics) {
        for (at, node) in self.nodes.iter().enumerate() {
            let style = node_style(node, at == 0, Some(metrics));
            self.taffy.set_style(node.id, style).expect("the tree's own node");
        }
        self.dirty = true;
    }

    fn lay_out(&mut self, fonts: &mut UiFonts) {
        profiling::scope!("UiTree layout");
        self.stats.layouts += 1;
        let (scale, size, text_scale) = (self.scale, self.size, self.text_scale);
        let UiFonts { text: system, families, locale, .. } = fonts;
        let (nodes, styles) = (&self.nodes, &self.styles);
        let available =
            TaffySize { width: AvailableSpace::Definite(size.x), height: AvailableSpace::Definite(size.y) };
        let measured = self.taffy.compute_layout_with_measure(
            self.root,
            available,
            |input: LayoutInput, _, at: Option<&mut u32>, style: &TaffyStyle| {
                let node = at.map(|at| &nodes[*at as usize]);
                let text = node.and_then(|node| node.text.as_ref());
                let times = node.map_or(1.0, |node| text_scale * node.bound_text_scale);
                // Padding and limits are the leaf's own, and what's measured is inside them.
                compute_leaf_layout(
                    input,
                    style,
                    |_, _| 0.0,
                    |known, available| {
                        let Some(text) = text else { return known.unwrap_or(TaffySize::ZERO) };
                        let Some(family) = text.family else { return known.unwrap_or(TaffySize::ZERO) };
                        let width = text.wrap.then(|| {
                            known.width.or(match available.width {
                                AvailableSpace::Definite(width) => Some(width),
                                AvailableSpace::MinContent => Some(0.0),
                                AvailableSpace::MaxContent => None,
                            })
                        });
                        let style =
                            text_style(&styles[usize::from(text.style)], times, families, family, locale);
                        let shaped = system.shape(&text.resolved, &style, scale, width.flatten());
                        TaffySize {
                            // Whole pixels up, so the same text fits the box it asked for.
                            width: known.width.unwrap_or(shaped.width.ceil() / scale),
                            height: known.height.unwrap_or(shaped.height.ceil() / scale),
                        }
                    },
                )
            },
        );
        if let Err(error) = measured {
            tracing::error!(target: crate::LOG_TARGET, %error, "a screen's layout");
        }
        self.place();
    }

    /// Every node's rectangle in the frame, from `taffy`'s relative ones.
    fn place(&mut self) {
        for at in 0..self.nodes.len() {
            let (parent_min, parent_visible) = match self.nodes[at].parent {
                Some(parent) => {
                    let parent = &self.nodes[parent as usize];
                    (parent.rect.min, parent.visible)
                }
                None => (Vec2::ZERO, true),
            };
            let node = &mut self.nodes[at];
            let layout = self.taffy.layout(node.id).expect("the tree's own node");
            let min = parent_min + Vec2::new(layout.location.x, layout.location.y);
            node.rect = Rect::new(min, min + Vec2::new(layout.size.width, layout.size.height));
            node.content = Rect::new(
                min + Vec2::new(layout.padding.left, layout.padding.top),
                node.rect.max - Vec2::new(layout.padding.right, layout.padding.bottom),
            );
            node.visible = parent_visible && node.shown;
        }
        self.measure_scrollers();
    }

    /// How far each scroller can go: what its shown children overflow it by, its end's padding included.
    fn measure_scrollers(&mut self) {
        for at in 0..self.nodes.len() {
            if let Some(scroll) = &mut self.nodes[at].scroll {
                scroll.extent = 0.0;
            }
            let node = &self.nodes[at];
            let Some(parent) = node.parent.map(|parent| parent as usize) else { continue };
            if !node.scrolled || !node.shown {
                continue;
            }
            let margin = node.layout.margin;
            let far = node.rect.max + Vec2::new(margin.right, margin.bottom);
            let over = &mut self.nodes[parent];
            let padding = over.rect.max - over.content.max;
            let end = over.rect.max;
            if let Some(scroll) = &mut over.scroll {
                scroll.extent = scroll.extent.max(scroll.along(far + padding - end));
            }
        }
    }

    /// What this frame's timelines and scrolling make of what layout said: where each node is on the
    /// screen, how opaque, under what mask. Input reads this between frames and drawing writes it out.
    fn resolve(&mut self, now: Timestamp) {
        let scale = self.scale;
        for at in 0..self.nodes.len() {
            let (parent_opacity, parent_placed, mask) = match self.nodes[at].parent {
                Some(parent) => {
                    let parent = &self.nodes[parent as usize];
                    (parent.final_opacity, parent.child_placed, parent.child_mask)
                }
                None => (1.0, Placement::IDENTITY, None),
            };
            let node = &mut self.nodes[at];
            node.final_opacity = parent_opacity * node.opacity * node.bound_opacity * node.pose.opacity;
            let own = Placement::of_pose(node.pose, node.rect.center());
            node.placed = parent_placed.after(own).snapped(scale);
            node.screen = node.placed.rect(node.rect);
            node.own_mask = mask;
            node.child_mask = if node.mask {
                let snapped = |value: f32| (value * scale).round() / scale;
                let edges = [
                    snapped(node.screen.min.x),
                    snapped(node.screen.min.y),
                    snapped(node.screen.max.x),
                    snapped(node.screen.max.y),
                ];
                Some(mask.map_or(edges, |outer| {
                    [
                        edges[0].max(outer[0]),
                        edges[1].max(outer[1]),
                        edges[2].min(outer[2]),
                        edges[3].min(outer[3]),
                    ]
                }))
            } else {
                mask
            };
            node.child_placed = match &node.scroll {
                Some(scroll) => {
                    if scroll.moving(now) {
                        self.stats.scrolling += 1;
                    }
                    let moved = Placement { scale: 1.0, offset: -scroll.vector(scroll.offset(now)) };
                    node.placed.after(moved).snapped(scale)
                }
                None => node.placed,
            };
        }
    }

    /// Writes the screen into `list`, over whatever's there, with any new glyphs' texels.
    pub fn draw(&mut self, fonts: &mut UiFonts, atlas: &Atlas, list: &mut UiDrawList) {
        profiling::scope!("UiTree::draw");
        let stopwatch = Stopwatch::start();
        list.viewport = self.size.to_array();
        list.atlas = Some(std::sync::Arc::clone(&atlas.image));
        let (scale, size) = (self.scale, self.size);
        for at in 0..self.nodes.len() {
            let node = &self.nodes[at];
            if !node.visible {
                continue;
            }
            let shows = node.final_opacity > 0.0 && paint::on_screen(node.screen, node.own_mask, size);
            let first = list.quads.len();
            if shows {
                list.set_mask(node.own_mask);
                if let Some(look) = self.look_of(at) {
                    paint::look(
                        look,
                        node.rect,
                        node.fill,
                        node.final_opacity,
                        scale,
                        &atlas.sprites,
                        &mut list.quads,
                    );
                }
                if let Some(inset) = &node.inset {
                    let rect = paint::snapped(node.rect, scale);
                    list.push_inset_quad(inset.setup.inset, inset::quad(rect, node.final_opacity));
                }
            }
            if let Some(text) = &node.text
                && let Some(family) = text.family
            {
                let UiFonts { text: system, atlas: glyphs, families, locale, warm_left, .. } = fonts;
                let times = self.text_scale * node.bound_text_scale;
                let style =
                    text_style(&self.styles[usize::from(text.style)], times, families, family, locale);
                let width = text.wrap.then_some(node.content.width());
                let shaped = system.shape(&text.resolved, &style, scale, width);
                if shows {
                    let origin = node.content.min;
                    if let Some(shadow) = &text.shadow {
                        // Whole pixels from the text, wherever that lands.
                        let [x, y] = shadow.offset.map(|value| (value * scale).round() / scale);
                        let color = crate::paint::faded(shadow.color, node.final_opacity);
                        glyphs.draw(shaped, (origin + Vec2::new(x, y)).to_array(), scale, color, list);
                    }
                    let color = crate::paint::faded(text.color, node.final_opacity);
                    glyphs.draw(shaped, origin.to_array(), scale, color, list);
                } else {
                    // On its way in: the frame it shows has nothing to do.
                    glyphs.warm(shaped, warm_left, list);
                }
            }
            if shows && node.failed {
                paint::problem_frame(node.rect, scale, &atlas.sprites, &mut list.quads);
            }
            node.placed.apply(&mut list.quads[first..]);
        }
        list.set_mask(None);
        let shaped = fonts.text.take_stats().misses;
        let cost = self.update_cost + stopwatch.elapsed();
        let stats = &mut list.tree;
        stats.trees += self.stats.trees;
        stats.nodes += self.stats.nodes;
        stats.layouts += self.stats.layouts;
        stats.resolves += self.stats.resolves;
        stats.bound_changes += self.stats.bound_changes;
        stats.shaped += shaped;
        stats.problems += self.stats.problems;
        stats.playing += self.stats.playing;
        stats.scrolling += self.stats.scrolling;
        stats.micros += cost.as_micros() as u32;
    }
}

/// A node's style in full: its layout, the safe area if it asks, and what the model says of it. The
/// root takes the frame's size whatever it says.
fn node_style(node: &Node, root: bool, metrics: Option<UiMetrics>) -> TaffyStyle {
    let insets =
        metrics.filter(|_| node.layout.safe_area).map_or_else(Insets::default, |metrics| metrics.insets);
    let mut style = style::taffy_style(&node.layout, insets);
    if node.scroll.is_some() {
        // What it holds may overflow it, and doesn't size it.
        style.overflow =
            taffy::geometry::Point { x: taffy::style::Overflow::Hidden, y: taffy::style::Overflow::Hidden };
    }
    if node.scrolled {
        style.flex_shrink = 0.0;
    }
    if root && let Some(metrics) = metrics {
        style.size = TaffySize {
            width: taffy::style::Dimension::length(metrics.size.x),
            height: taffy::style::Dimension::length(metrics.size.y),
        };
        style.min_size = TaffySize::auto();
        style.max_size = TaffySize::auto();
    }
    if !node.shown {
        style.display = Display::None;
    }
    style
}

/// A style as shaping reads it, its size `times` as large: one of its role's family lists.
fn text_style<'a>(
    style: &UiTextStyle,
    times: f32,
    families: &'a [(Box<str>, Option<String>)],
    family: u16,
    locale: &'a str,
) -> TextStyle<'a> {
    TextStyle {
        families: families[usize::from(family)].1.as_deref().unwrap_or_default(),
        size: style.size * times,
        weight: style.weight,
        line_height: style.line_height,
        locale: Some(locale),
    }
}

/// `document`'s nodes depth first, each followed by its subtree, siblings as the document lists them:
/// the order they draw in. A document lists a parent before its children, and siblings may be anywhere
/// after it.
fn drawing_order(document: &UiDocument) -> Vec<usize> {
    let mut children: Vec<Vec<usize>> = vec![Vec::new(); document.nodes.len()];
    for (at, node) in document.nodes.iter().enumerate() {
        if let Some(parent) = node.parent.as_deref()
            && let Some(parent) = document.nodes.iter().position(|other| other.name == parent)
        {
            children[parent].push(at);
        }
    }
    let mut order = Vec::with_capacity(document.nodes.len());
    let mut stack = vec![0];
    while let Some(at) = stack.pop() {
        order.push(at);
        stack.extend(children[at].iter().rev());
    }
    order
}

/// A text scale as a tree takes it: within [`TEXT_SCALES`](sapphire_assets::ui::TEXT_SCALES), and 1 for
/// what isn't a number.
pub(crate) fn text_scale_within(scale: f32) -> f32 {
    let scales = sapphire_assets::ui::TEXT_SCALES;
    if scale.is_nan() { 1.0 } else { scale.clamp(*scales.start(), *scales.end()) }
}
