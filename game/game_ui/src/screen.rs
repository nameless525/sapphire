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

/// What [`ScreenStack::push_plain_under`] stands behind a screen. Not cooked: it says nothing, and no game
/// shows it.
const PLAIN_BACKDROP: &str = r#"(
    type: "sapphire::assets::UiDocument",
    version: 2,
    data: (nodes: [(name: "root", look: (fill: Color((74, 88, 112, 255))))]),
)"#;

/// What the backdrop's paths read: none.
#[derive(Clone, Debug, Default, bevy_reflect::Reflect)]
struct NoModel;

/// A frame shorter than this, in points, is a phone's, where the player's text grows to
/// [`PHONE_TEXT_SCALE`] at most (M4 A2): the game's screens have no room for more there, and a phone's held
/// closer than a Deck.
pub const PHONE_HEIGHT: f32 = 480.0;
/// See [`PHONE_HEIGHT`]: the player's "large" text size.
pub const PHONE_TEXT_SCALE: f32 = 1.25;

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
    /// Pixels per point of the display a frame with no window stands for, under [`Self::scale`]
    /// ([`Self::set_display_scale`]).
    display_scale: f32,
    /// What every screen's text is multiplied by: the player's text size.
    text_scale: f32,
    /// A window's metrics, read this frame. Offscreen there's only a target's pixels.
    metrics: Option<UiMetrics>,
    pages: UiPageState,
    now: Timestamp,
    /// The screen the last [`Self::input`] went to, whose events [`Self::drain_events`] reads: a press that
    /// closes it starts its leave, and its event still has to reach the game.
    took_input: Option<usize>,
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
            display_scale: 1.0,
            text_scale: 1.0,
            metrics: None,
            pages: UiPageState::NO_RENDERER,
            now: Timestamp::default(),
            took_input: None,
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

    /// Puts a flat color under all the rest, in place of the scene, so a capture moves for what the screens show
    /// and nothing else: what the `game-*` goldens stand behind a screen (the sandbox's `--ui-plain`), and an
    /// editor's preview.
    pub fn push_plain_under(&mut self, at: Timestamp) -> Result<(), String> {
        let backdrop = UiDocument::from_document(PLAIN_BACKDROP).map_err(|error| error.to_string())?;
        self.push_under(&backdrop, Box::new(NoModel), at)
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
        self.took_input = match self.took_input {
            Some(took) if took > index => Some(took - 1),
            Some(took) if took == index => None,
            took => took,
        };
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

    /// Where there's no window, draws as a display of `scale` pixels per point would, under
    /// [`Self::set_scale`]'s: a capture standing for a phone's frame (`game_ui::hud::OffscreenDisplay`).
    pub fn set_display_scale(&mut self, scale: f32) {
        self.display_scale = scale;
    }

    /// Draws every screen's text `scale` times its size from the next frame: the player's text size, over the
    /// interface's ([`UiTree::set_text_scale`]), and no more than [`PHONE_TEXT_SCALE`] on a phone's frame.
    pub fn set_text_scale(&mut self, scale: f32) {
        self.text_scale = scale;
    }

    /// Shows every screen in `locale`, from the next frame.
    pub fn set_locale(&mut self, locale: &str) -> Result<(), String> {
        self.loc.set_text_locale(locale).map_err(|error| format!("the language `{locale}`: {error}"))
    }

    /// `key`'s plain message in the stack's language, or the key itself where no table has one: for a
    /// string shown outside the screens, as a window's title is.
    #[must_use]
    pub fn text<'a>(&'a self, key: &'a str) -> &'a str {
        self.loc.text(key)
    }

    /// A frame's input: the screen [`Self::taking_input`] names, if any.
    pub fn input(&mut self, input: &UiInput<'_>) {
        self.took_input = self.taking_input();
        if let Some(index) = self.took_input {
            self.screens[index].tree.input(input, self.now);
        }
    }

    /// Which screen a frame's input goes to: the top one that isn't on its way out (M4 A8), once it has opened.
    /// A screen leaving takes no input, so a press in the frames it takes to go reaches what's under it; one
    /// not yet drawn takes none either, and nothing under it does.
    #[must_use]
    pub fn taking_input(&self) -> Option<usize> {
        let index = (0..self.screens.len()).rev().find(|&index| !self.is_leaving(index))?;
        self.screens[index].opened.then_some(index)
    }

    /// What's happened on the screen the last [`Self::input`] went to, into `events`, which is cleared first:
    /// which screen that is. It's that screen even when the input started its leave, as a back does.
    pub fn drain_events(&mut self, events: &mut Vec<UiEvent>) -> Option<usize> {
        events.clear();
        let index =
            self.took_input.or_else(|| self.taking_input()).filter(|&index| index < self.screens.len())?;
        events.extend(self.screens[index].tree.drain_events());
        Some(index)
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

    /// Whether the screen at `index` is on its way out, through its machine's `leaving`, or out already and
    /// waiting to be taken away.
    #[must_use]
    pub fn is_leaving(&self, index: usize) -> bool {
        self.screens[index].tree.state() == Some("leaving") || self.has_left(index)
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
        // What a cook checks with the model's document, from the model itself.
        top.model.get_represented_type_info().ok_or("a model of no known type")?;
        let model = UiModel::of_model(top.model.as_ref());
        self.replace_document(&document, &model)
    }

    /// Takes `document` for the top screen, read against `model` as a cook reads it, keeping what's on screen
    /// if a path isn't the model's: an editor's way in, whose model is the document's own model file rather
    /// than a type.
    pub fn replace_document(&mut self, document: &UiDocument, model: &UiModel) -> Result<(), String> {
        let top = self.screens.last_mut().ok_or("no screen is showing")?;
        if let Some(issue) = document.path_issues(model).into_iter().next() {
            return Err(issue.to_string());
        }
        top.tree = new_tree(document)?;
        // An editor's model is dynamic values, with no type for the tree to check its paths against.
        top.tree.declare_model(model.clone());
        top.opened = false;
        Ok(())
    }

    /// Has the top screen read `model` from the next draw: an editor's stand-in values, typed in.
    pub fn set_model(&mut self, model: Box<dyn PartialReflect>) {
        if let Some(top) = self.screens.last_mut() {
            top.model = model;
        }
    }

    /// What the renderer's glyph pages hold, for the next [`Self::draw`].
    pub fn follow_pages(&mut self, pages: UiPageState) {
        self.pages = pages;
    }

    /// Writes the screens into `list`, for a target of `size` pixels.
    pub fn draw(&mut self, size: (u32, u32), list: &mut UiDrawList) {
        let metrics = self.metrics.unwrap_or_else(|| {
            UiMetrics::of_pixels(size, self.display_scale * self.scale, Insets::default())
        });
        self.fonts.begin_frame(self.pages, &self.loc);
        let text_scale = if metrics.size.y < PHONE_HEIGHT {
            self.text_scale.min(PHONE_TEXT_SCALE)
        } else {
            self.text_scale
        };
        for screen in &mut self.screens {
            screen.tree.set_text_scale(text_scale);
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
