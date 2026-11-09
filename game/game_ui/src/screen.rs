//! Retained screens over the scene (M3 U4, U7): a stack of trees sharing one language, one set of fonts and
//! one atlas, each reading a model the caller hands in. The game's menus and the sandbox's test screens show
//! through this, and `sapphire_stage` only says what loading some hands back
//! ([`ScreenAssets`](sapphire_stage::content::ScreenAssets)).
//!
//! Every screen on the stack draws, the bottom first, and only the top one takes input. What input means is
//! the caller's: this reads no device and no clock.

use bevy_reflect::PartialReflect;
use sapphire_assets::font::Font;
use sapphire_assets::ui::{UiDocument, UiModel};
use sapphire_loc::{LocaleSettings, Localizer, StringTable};
use sapphire_look::{UiDrawList, UiPageState};
use sapphire_platform::Timestamp;
use sapphire_ui::{Atlas, Insets, UiEvent, UiFonts, UiFrame, UiInput, UiMetrics, UiTree};

/// How a [`ScreenStack`] shows what it's handed.
#[derive(Clone, Debug, PartialEq)]
pub struct StackSetup {
    /// The language.
    pub locale: String,
    /// Whether under pseudo-localization.
    pub pseudo: bool,
    /// Pixels per point, where no window says otherwise ([`ScreenStack::set_metrics`]).
    pub scale: f32,
}

/// One screen on the stack.
struct Screen {
    tree: UiTree,
    model: Box<dyn PartialReflect>,
    /// When it opens. It waits for its first layout, so this is the first draw's business.
    opened_at: Timestamp,
    opened: bool,
    /// Steps of focus taken as it opens, as [`UiTree::walk`] names them.
    walk: Vec<String>,
}

/// Screens, ready to draw over a scene: see this module.
pub struct ScreenStack {
    screens: Vec<Screen>,
    fonts: UiFonts,
    atlas: Atlas,
    loc: Localizer,
    scale: f32,
    /// A window's metrics, read this frame. Offscreen there's only a target's pixels.
    metrics: Option<UiMetrics>,
    pages: UiPageState,
    now: Timestamp,
}

impl std::fmt::Debug for ScreenStack {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ScreenStack")
            .field("screens", &self.screens.len())
            .field("scale", &self.scale)
            .finish_non_exhaustive()
    }
}

impl ScreenStack {
    /// An empty stack, with the tables of its languages and each font as it's cooked (a display face's
    /// field and all), over `atlas`: the one the HUD draws with, since a frame's list has one atlas.
    pub fn new(
        tables: Vec<StringTable>,
        fonts: Vec<Font>,
        atlas: Atlas,
        setup: StackSetup,
    ) -> Result<Self, String> {
        let StackSetup { locale, pseudo, scale } = setup;
        let mut loc = Localizer::new(LocaleSettings { text: locale.clone(), voice: locale.clone() });
        for table in tables {
            loc.install(table).map_err(|error| error.to_string())?;
        }
        loc.set_text_locale(&locale).map_err(|error| format!("the language `{locale}`: {error}"))?;
        loc.set_pseudo(pseudo);
        let mut shaped = UiFonts::default();
        for font in fonts {
            let family = font.family.clone();
            shaped.register(font).map_err(|error| format!("the font {family}: {error}"))?;
        }
        Ok(Self {
            screens: Vec::new(),
            fonts: shaped,
            atlas,
            loc,
            scale,
            metrics: None,
            pages: UiPageState::NO_RENDERER,
            now: Timestamp::default(),
        })
    }

    /// Puts `document` on top, reading `model`, opening at `at`, taking the steps of `walk` as it does.
    pub fn push(
        &mut self,
        document: &UiDocument,
        model: Box<dyn PartialReflect>,
        at: Timestamp,
        walk: Vec<String>,
    ) -> Result<(), String> {
        self.insert(self.screens.len(), document, model, at, walk)
    }

    /// Puts `document` under all the rest, for what's behind them when they go.
    pub fn push_under(
        &mut self,
        document: &UiDocument,
        model: Box<dyn PartialReflect>,
        at: Timestamp,
    ) -> Result<(), String> {
        self.insert(0, document, model, at, Vec::new())
    }

    fn insert(
        &mut self,
        index: usize,
        document: &UiDocument,
        model: Box<dyn PartialReflect>,
        opened_at: Timestamp,
        walk: Vec<String>,
    ) -> Result<(), String> {
        let tree = new_tree(document)?;
        self.screens.insert(index, Screen { tree, model, opened_at, opened: false, walk });
        Ok(())
    }

    /// Takes off the screen at `index`, counting from the bottom.
    pub fn remove(&mut self, index: usize) {
        self.screens.remove(index);
    }

    /// How many screens are on it.
    #[must_use]
    pub fn len(&self) -> usize {
        self.screens.len()
    }

    /// Whether none are.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.screens.is_empty()
    }

    /// The time on the stack's clock, as of the next [`Self::draw`].
    pub fn set_now(&mut self, now: Timestamp) {
        self.now = now;
    }

    /// A window's metrics, for the next [`Self::draw`].
    pub fn set_metrics(&mut self, metrics: UiMetrics) {
        self.metrics = Some(metrics);
    }

    /// Pixels per point where there's no window.
    pub fn set_scale(&mut self, scale: f32) {
        self.scale = scale;
    }

    /// Shows every screen in `locale`, from the next frame.
    pub fn set_locale(&mut self, locale: &str) -> Result<(), String> {
        self.loc.set_text_locale(locale).map_err(|error| format!("the language `{locale}`: {error}"))
    }

    /// A frame's input: the top screen's, if it's open.
    pub fn input(&mut self, input: &UiInput<'_>) {
        if let Some(top) = self.screens.last_mut().filter(|top| top.opened) {
            top.tree.input(input, self.now);
        }
    }

    /// What's happened on the top screen, into `events`, which is cleared first.
    pub fn drain_events(&mut self, events: &mut Vec<UiEvent>) {
        events.clear();
        if let Some(top) = self.screens.last_mut() {
            events.extend(top.tree.drain_events());
        }
    }

    /// The screen at `index`'s tree, counting from the bottom.
    #[must_use]
    pub fn tree(&self, index: usize) -> &UiTree {
        &self.screens[index].tree
    }

    /// The same, to play or fire on.
    pub fn tree_mut(&mut self, index: usize) -> &mut UiTree {
        &mut self.screens[index].tree
    }

    /// The top screen's tree.
    #[must_use]
    pub fn top(&self) -> Option<&UiTree> {
        self.screens.last().map(|screen| &screen.tree)
    }

    /// Whether the screen at `index` has been drawn, which is when it opens.
    #[must_use]
    pub fn has_opened(&self, index: usize) -> bool {
        self.screens[index].opened
    }

    /// Whether the screen at `index` has opened and gone back to hidden, so it's done with.
    #[must_use]
    pub fn has_left(&self, index: usize) -> bool {
        let screen = &self.screens[index];
        screen.opened && !screen.tree.is_moving() && screen.tree.state() == Some("hidden")
    }

    /// The model of the screen at `index`, if it's a `T`.
    pub fn model_mut<T: PartialReflect>(&mut self, index: usize) -> Option<&mut T> {
        self.screens[index].model.as_mut().try_downcast_mut::<T>()
    }

    /// Takes a new document for the top screen, keeping what's on screen if it has anything wrong with it:
    /// hot reload's and live link's way in.
    pub fn replace(&mut self, text: &str) -> Result<(), String> {
        let top = self.screens.last_mut().ok_or("no screen is showing")?;
        let document = UiDocument::from_document(text).map_err(|error| error.to_string())?;
        // What a cook checks with the model's document, from the type itself.
        let info = top.model.get_represented_type_info().ok_or("a model of no known type")?;
        let model = UiModel::of_type(info);
        if let Some(issue) = document.path_issues(&model).into_iter().next() {
            return Err(issue.to_string());
        }
        top.tree = new_tree(&document)?;
        top.opened = false;
        Ok(())
    }

    /// What the renderer's glyph pages hold, for the next [`Self::draw`].
    pub fn follow_pages(&mut self, pages: UiPageState) {
        self.pages = pages;
    }

    /// Writes the screens into `list`, for a target of `size` pixels.
    pub fn draw(&mut self, size: (u32, u32), list: &mut UiDrawList) {
        let metrics =
            self.metrics.unwrap_or_else(|| UiMetrics::of_pixels(size, self.scale, Insets::default()));
        self.fonts.begin_frame(self.pages, &self.loc);
        for screen in &mut self.screens {
            let model = screen.model.as_partial_reflect();
            if !screen.opened {
                screen.opened = true;
                let start = screen.opened_at;
                let frame = UiFrame { metrics, now: start, loc: &self.loc, model: Some(model) };
                screen.tree.update(&frame, &mut self.fonts);
                screen.tree.fire("open", start);
                // Checked when it was asked for, and what a test's document has isn't this one's business.
                let walked = screen.tree.walk(screen.walk.iter().map(String::as_str), start);
                debug_assert!(walked.is_ok(), "{walked:?}");
            }
            let frame = UiFrame { metrics, now: self.now, loc: &self.loc, model: Some(model) };
            screen.tree.update(&frame, &mut self.fonts);
            screen.tree.draw(&mut self.fonts, &self.atlas, list);
        }
    }

    /// Turns the insets of every screen on it as a drag `across` that share of their width would (M3 U6,
    /// `UiTree::turn_insets`).
    pub fn turn_insets(&mut self, across: f32) {
        for screen in &mut self.screens {
            screen.tree.turn_insets(across);
        }
    }

    /// Whether any screen on it holds an inset, which needs the inset's pipeline declared.
    #[must_use]
    pub fn holds_insets(&self) -> bool {
        self.screens.iter().any(|screen| screen.tree.holds_insets())
    }

    /// Hands `visit` each inset the last [`Self::draw`] showed, on every screen, as a [`SceneShow`] does.
    ///
    /// [`SceneShow`]: sapphire_stage::SceneShow
    pub fn inset_shots(&self, visit: &mut dyn FnMut(sapphire_stage::InsetShot<'_>)) {
        for shot in self.screens.iter().flat_map(|screen| screen.tree.insets()) {
            visit(sapphire_stage::InsetShot {
                inset: shot.setup.inset,
                size: shot.size,
                yaw: shot.yaw,
                setup: shot.setup,
            });
        }
    }
}

fn new_tree(document: &UiDocument) -> Result<UiTree, String> {
    UiTree::new(document).map_err(|error| error.to_string())
}
