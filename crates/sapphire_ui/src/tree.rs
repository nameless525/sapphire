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
//! tree.update(&UiFrame { metrics, scale, loc: &localizer, model: Some(&model) }, &mut fonts);
//! tree.draw(&mut fonts, &atlas, &mut packet.ui);
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
//! wrong: its node reads as hidden, transparent, empty.
//!
//! A key the tables don't hold shows as itself and counts in `LocStats`. Under pseudo-localization,
//! [`UiTree::unframed`] finds it, along with anything else that didn't come through a message.
//!
//! # Units and pixels
//!
//! Layout runs in UI points with `taffy`'s rounding off, and rectangles snap to whole pixels as they
//! draw, so one document lays out alike at any scale and edges stay crisp. Text sizes are whole pixels
//! ([`TYPE_SCALE`](sapphire_text::TYPE_SCALE)).

mod bind;
mod paint;
mod style;

use std::any::TypeId;

use sapphire_assets::ui::{UiBindTarget, UiColor, UiDocument, UiLayout, UiLook, UiModel, UiTextStyle};
use sapphire_loc::{Arg, Localizer};
use sapphire_look::{UiDrawList, UiPageState, UiTreeStats};
use sapphire_math::Vec2;
use sapphire_reflect::PartialReflect;
use sapphire_reflect::bevy_reflect::ParsedPath;
use sapphire_text::{AtlasConfig, FaceOptions, GlyphAtlas, TextStyle, TextSystem, family_list};
use sapphire_time::Stopwatch;
use taffy::prelude::{AvailableSpace, Display, NodeId, Size as TaffySize, Style as TaffyStyle, TaffyTree};
use taffy::{LayoutInput, compute_leaf_layout};

use self::bind::{BoundText, read};
use crate::atlas::Atlas;
use crate::layout::{Insets, Rect, UiMetrics};

/// The color a node with something wrong is framed in: nothing a screen would pick on purpose.
pub const PROBLEM_COLOR: UiColor = [255, 0, 200, 255];

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
        }
    }

    /// Adds a font, as a cooked font's `file` and `baseline_shift` give it. What's laid out lays out
    /// again.
    pub fn register_font(&mut self, file: std::sync::Arc<Vec<u8>>, baseline_shift: f32) {
        self.text.register_face(file, FaceOptions { baseline_shift });
        self.revision += 1;
    }

    /// Moves to the next frame, given what the renderer's glyph pages hold and the language in force.
    pub fn begin_frame(&mut self, pages: UiPageState, loc: &Localizer) {
        self.text.begin_frame();
        self.atlas.begin_frame(pages);
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
    /// Pixels per point.
    pub scale: f32,
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
    /// Has a problem, and draws a frame saying so.
    failed: bool,
    /// Where it is, in points from the frame's top-left.
    rect: Rect,
    /// Inside its padding.
    content: Rect,
    /// False under a node the model hides.
    visible: bool,
    /// The mask its own quads draw under, and the one its children's do.
    own_mask: Option<[f32; 4]>,
    child_mask: Option<[f32; 4]>,
    /// Multiplied down from the root, for this frame.
    final_opacity: f32,
}

/// A screen: see the [module docs](self).
pub struct UiTree {
    taffy: TaffyTree<u32>,
    /// In drawing order, each node's parent before it, and its subtree right after.
    nodes: Vec<Node>,
    root: NodeId,
    styles: Vec<UiTextStyle>,
    problems: Vec<UiProblem>,
    /// The model type the paths were last checked against, or none yet.
    checked_model: Option<Option<TypeId>>,
    metrics: Option<(UiMetrics, u32)>,
    revision: Option<u64>,
    dirty: bool,
    scale: f32,
    size: Vec2,
    scratch: String,
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
                failed: false,
                rect: Rect::default(),
                content: Rect::default(),
                visible: true,
                own_mask: None,
                child_mask: None,
                final_opacity: 1.0,
            });
        }
        let root =
            nodes.first().map(|node| node.id).ok_or_else(|| UiTreeError::Layout("no root".to_owned()))?;
        Ok(Self {
            taffy,
            nodes,
            root,
            styles: document.styles.clone(),
            problems: Vec::new(),
            checked_model: None,
            metrics: None,
            revision: None,
            dirty: true,
            scale: 1.0,
            size: Vec2::ZERO,
            scratch: String::new(),
            stats: UiTreeStats::default(),
            update_cost: std::time::Duration::ZERO,
        })
    }

    /// The node called `name`.
    #[must_use]
    pub fn find(&self, name: &str) -> Option<UiNodeId> {
        self.nodes.iter().position(|node| *node.name == *name).map(|at| UiNodeId(at as u32))
    }

    /// Where `node` was last laid out, in points from the frame's top-left, before snapping.
    #[must_use]
    pub fn rect(&self, node: UiNodeId) -> Rect {
        self.nodes[node.0 as usize].rect
    }

    /// Whether `node` and everything over it are shown.
    #[must_use]
    pub fn is_visible(&self, node: UiNodeId) -> bool {
        self.nodes[node.0 as usize].visible
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
        self.check_model(frame.model);
        let new_language = self.revision != Some(fonts.revision);
        if new_language {
            self.revision = Some(fonts.revision);
            self.follow_language(frame.loc, fonts);
        }
        self.read_model(frame);
        self.resolve_strings(frame.loc);
        let positive = if frame.scale.is_finite() && frame.scale > 0.0 { frame.scale } else { 1.0 };
        let metrics = (frame.metrics, positive.to_bits());
        if self.metrics != Some(metrics) {
            self.metrics = Some(metrics);
            self.follow_metrics(frame.metrics);
            self.scale = positive;
            self.size = frame.metrics.size;
        }
        if self.dirty {
            self.dirty = false;
            self.lay_out(fonts);
        }
        self.stats.problems = self.problems.len() as u32;
        self.update_cost = stopwatch.elapsed();
    }

    /// Checks the paths against the model's type, the first time it's seen: what a cook does with a
    /// model document, here with the real thing.
    fn check_model(&mut self, model: Option<&dyn PartialReflect>) {
        let info = model.and_then(PartialReflect::get_represented_type_info);
        let seen = info.map(|info| info.type_id());
        if self.checked_model == Some(seen) {
            return;
        }
        self.checked_model = Some(seen);
        // Everything found last time was about the last model.
        self.problems.retain(|problem| !problem.field.contains("path"));
        let model = info.map(UiModel::of_type);
        for node in &mut self.nodes {
            let mut failed = false;
            let mut report = |field: String, problem: String| {
                tracing::error!(target: crate::LOG_TARGET, node = %node.name, field, problem, "a screen's binding");
                self.problems.push(UiProblem { node: node.name.to_string(), field, problem });
                failed = true;
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
                text.stale = true;
            }
            node.failed = failed;
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
                node.failed = true;
            } else {
                node.failed =
                    node.binds.iter().any(|bind| bind.failed) || text.args.iter().any(|arg| arg.failed);
            }
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
                            let metrics = self.metrics.map(|(metrics, _)| metrics);
                            let style = node_style(node, at == 0, metrics);
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
            let args: Vec<Arg<'_>> = text.args.iter().map(|arg| (&*arg.name, arg.value())).collect();
            let resolved = loc.resolve(&text.key, &args, scratch);
            if resolved != text.resolved {
                text.resolved.clear();
                text.resolved.push_str(resolved);
                self.taffy.mark_dirty(node.id).expect("the tree's own node");
                self.dirty = true;
            }
        }
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
        let (scale, size) = (self.scale, self.size);
        let UiFonts { text: system, families, locale, .. } = fonts;
        let (nodes, styles) = (&self.nodes, &self.styles);
        let available =
            TaffySize { width: AvailableSpace::Definite(size.x), height: AvailableSpace::Definite(size.y) };
        let measured = self.taffy.compute_layout_with_measure(
            self.root,
            available,
            |input: LayoutInput, _, at: Option<&mut u32>, style: &TaffyStyle| {
                let text = at.and_then(|at| nodes[*at as usize].text.as_ref());
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
                        let style = text_style(&styles[usize::from(text.style)], families, family, locale);
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
            let (parent_min, parent_visible, mask) = match self.nodes[at].parent {
                Some(parent) => {
                    let parent = &self.nodes[parent as usize];
                    (parent.rect.min, parent.visible, parent.child_mask)
                }
                None => (Vec2::ZERO, true, None),
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
            node.own_mask = mask;
            node.child_mask = if node.mask {
                let scale = self.scale;
                let snapped = |value: f32| (value * scale).round() / scale;
                let edges = [
                    snapped(node.rect.min.x),
                    snapped(node.rect.min.y),
                    snapped(node.rect.max.x),
                    snapped(node.rect.max.y),
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
        }
    }

    /// Writes the screen into `list`, over whatever's there, with any new glyphs' texels.
    pub fn draw(&mut self, fonts: &mut UiFonts, atlas: &Atlas, list: &mut UiDrawList) {
        profiling::scope!("UiTree::draw");
        let stopwatch = Stopwatch::start();
        list.viewport = self.size.to_array();
        list.atlas = Some(std::sync::Arc::clone(&atlas.image));
        let (scale, size) = (self.scale, self.size);
        let UiFonts { text: system, atlas: glyphs, families, locale, .. } = fonts;
        for at in 0..self.nodes.len() {
            let parent_opacity =
                self.nodes[at].parent.map_or(1.0, |parent| self.nodes[parent as usize].final_opacity);
            let node = &mut self.nodes[at];
            node.final_opacity = parent_opacity * node.opacity * node.bound_opacity;
            if !node.visible || !paint::on_screen(node.rect, node.own_mask, size) {
                continue;
            }
            list.set_mask(node.own_mask);
            if let Some(look) = &node.look {
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
            if let Some(text) = &node.text
                && let Some(family) = text.family
                && node.final_opacity > 0.0
            {
                let style = text_style(&self.styles[usize::from(text.style)], families, family, locale);
                let width = text.wrap.then_some(node.content.width());
                let shaped = system.shape(&text.resolved, &style, scale, width);
                let color = crate::paint::faded(text.color, node.final_opacity);
                glyphs.draw(shaped, node.content.min.to_array(), scale, color, list);
            }
            if node.failed {
                paint::problem_frame(node.rect, scale, &atlas.sprites, &mut list.quads);
            }
        }
        list.set_mask(None);
        let shaped = system.take_stats().misses;
        let cost = self.update_cost + stopwatch.elapsed();
        let stats = &mut list.tree;
        stats.trees += self.stats.trees;
        stats.nodes += self.stats.nodes;
        stats.layouts += self.stats.layouts;
        stats.resolves += self.stats.resolves;
        stats.bound_changes += self.stats.bound_changes;
        stats.shaped += shaped;
        stats.problems += self.stats.problems;
        stats.micros += cost.as_micros() as u32;
    }

    /// Rasterizes this screen's glyphs ahead of showing it, up to `budget` glyphs, which can be shared
    /// across trees: for a fade or a loading frame to spend. Whether it's all in the atlas now; if not,
    /// call it again next frame. Call [`Self::update`] first.
    pub fn warm(&self, fonts: &mut UiFonts, budget: &mut u32, list: &mut UiDrawList) -> bool {
        let scale = self.scale;
        let UiFonts { text: system, atlas: glyphs, families, locale, .. } = fonts;
        let mut done = true;
        for node in &self.nodes {
            let Some(text) = &node.text else { continue };
            let Some(family) = text.family else { continue };
            let style = text_style(&self.styles[usize::from(text.style)], families, family, locale);
            let width = text.wrap.then_some(node.content.width());
            let shaped = system.shape(&text.resolved, &style, scale, width);
            done &= glyphs.warm(shaped, budget, list);
        }
        done
    }
}

/// A node's style in full: its layout, the safe area if it asks, and what the model says of it. The
/// root takes the frame's size whatever it says.
fn node_style(node: &Node, root: bool, metrics: Option<UiMetrics>) -> TaffyStyle {
    let insets =
        metrics.filter(|_| node.layout.safe_area).map_or_else(Insets::default, |metrics| metrics.insets);
    let mut style = style::taffy_style(&node.layout, insets);
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

/// A style as shaping reads it: one of its role's family lists.
fn text_style<'a>(
    style: &UiTextStyle,
    families: &'a [(Box<str>, Option<String>)],
    family: u16,
    locale: &'a str,
) -> TextStyle<'a> {
    TextStyle {
        families: families[usize::from(family)].1.as_deref().unwrap_or_default(),
        size: style.size,
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
