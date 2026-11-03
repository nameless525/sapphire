//! A retained screen over the scene (M3 U4): a tree with its own language, fonts and atlas, reading a model
//! the caller hands in. The sandbox's test screens show through this, and `sapphire_stage` only says what
//! loading one hands back ([`ScreenAssets`]).

use bevy_reflect::PartialReflect;
use sapphire_assets::ui::{UiDocument, UiModel};
use sapphire_loc::{LocaleSettings, Localizer};
use sapphire_look::{UiDrawList, UiPageState};
use sapphire_platform::Timestamp;
use sapphire_stage::content::ScreenAssets;
use sapphire_ui::{Atlas, Insets, UiFonts, UiFrame, UiMetrics, UiTree};

/// How a [`ScreenLayer`] shows what it's handed.
pub struct ScreenSetup {
    /// The language.
    pub locale: String,
    /// Whether under pseudo-localization.
    pub pseudo: bool,
    /// Pixels per point.
    pub scale: f32,
    /// What its paths read.
    pub model: Box<dyn PartialReflect>,
    /// The time on its clock, which stands still: a capture of a time is that time.
    pub time: Timestamp,
    /// Steps of focus taken when it opens, as [`UiTree::walk`] names them.
    pub walk: Vec<String>,
}

/// A screen, ready to draw over a scene.
pub struct ScreenLayer {
    tree: UiTree,
    fonts: UiFonts,
    atlas: Atlas,
    loc: Localizer,
    model: Box<dyn PartialReflect>,
    scale: f32,
    pages: UiPageState,
    /// Whether the tree's been opened and walked, which waits for its first layout.
    opened: bool,
    now: Timestamp,
    walk: Vec<String>,
}

impl std::fmt::Debug for ScreenLayer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ScreenLayer")
            .field("tree", &self.tree)
            .field("scale", &self.scale)
            .finish_non_exhaustive()
    }
}

impl ScreenLayer {
    /// A layer showing `assets` as `setup` says.
    pub fn new(assets: ScreenAssets, setup: ScreenSetup) -> Result<Self, String> {
        let ScreenSetup { locale, pseudo, scale, model, time, walk } = setup;
        let mut loc = Localizer::new(LocaleSettings { text: locale.clone(), voice: locale.clone() });
        for table in assets.tables {
            loc.install(table).map_err(|error| error.to_string())?;
        }
        loc.set_text_locale(&locale).map_err(|error| format!("the language `{locale}`: {error}"))?;
        loc.set_pseudo(pseudo);
        let mut fonts = UiFonts::default();
        for (file, baseline_shift) in assets.fonts {
            fonts.register_font(file, baseline_shift);
        }
        Ok(Self {
            tree: new_tree(&assets.document)?,
            fonts,
            atlas: Atlas::paint(),
            loc,
            model,
            scale,
            pages: UiPageState::NO_RENDERER,
            opened: false,
            now: time,
            walk,
        })
    }

    /// Takes a new document, keeping what's on screen if it has anything wrong with it: hot reload's and
    /// live link's way in.
    pub fn replace(&mut self, text: &str) -> Result<(), String> {
        let document = UiDocument::from_document(text).map_err(|error| error.to_string())?;
        // What a cook checks with the model's document, from the type itself.
        let info = self.model.get_represented_type_info().ok_or("a model of no known type")?;
        let model = UiModel::of_type(info);
        if let Some(issue) = document.path_issues(&model).into_iter().next() {
            return Err(issue.to_string());
        }
        self.tree = new_tree(&document)?;
        self.opened = false;
        Ok(())
    }

    /// What the renderer's glyph pages hold, for the next [`Self::draw`].
    pub fn follow_pages(&mut self, pages: UiPageState) {
        self.pages = pages;
    }

    /// Writes the screen into `list`, for a target of `size` pixels.
    pub fn draw(&mut self, size: (u32, u32), list: &mut UiDrawList) {
        let metrics = UiMetrics::of_pixels(size, self.scale, Insets::default());
        self.fonts.begin_frame(self.pages, &self.loc);
        if !self.opened {
            self.opened = true;
            let start = Timestamp::from_nanos(0);
            let frame =
                UiFrame { metrics, now: start, loc: &self.loc, model: Some(self.model.as_partial_reflect()) };
            self.tree.update(&frame, &mut self.fonts);
            self.tree.fire("open", start);
            // Checked when it was asked for, and what a test's document has isn't this one's business.
            let walked = self.tree.walk(self.walk.iter().map(String::as_str), start);
            debug_assert!(walked.is_ok(), "{walked:?}");
        }
        let frame =
            UiFrame { metrics, now: self.now, loc: &self.loc, model: Some(self.model.as_partial_reflect()) };
        self.tree.update(&frame, &mut self.fonts);
        self.tree.draw(&mut self.fonts, &self.atlas, list);
    }

    /// The tree on screen.
    #[must_use]
    pub fn tree(&self) -> &UiTree {
        &self.tree
    }
}

fn new_tree(document: &UiDocument) -> Result<UiTree, String> {
    UiTree::new(document).map_err(|error| error.to_string())
}
