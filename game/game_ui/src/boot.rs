//! The boot screen (M4 H2): a retained screen (`content/ui/boot.ui.ron`) the game shows from its first frame
//! until the title, while what it draws through is built off the frame (`game_flow::boot`). It says what's under
//! way, by a key under `boot-step-`, and how much of the whole boot is done.
//!
//! It has nothing under it and needs no renderer: `sapphire_render::InterfaceScreen` draws its list alone, over
//! a target cleared to [`CLEAR`]. One look, and a first draft: the document's to change, not this.

use bevy_reflect::Reflect;
use sapphire_assets::AssetId;
use sapphire_look::{UiDrawList, UiPageState};
use sapphire_platform::Timestamp;
use sapphire_stage::content::ScreenAssets;
use sapphire_ui::Atlas;

use crate::screen::{ScreenStack, StackSetup};

/// The boot screen's document: `content/ui/boot.ui.ron`, by its sidecar's ID.
pub const BOOT_SCREEN: AssetId = AssetId::from_text("0f41511c-ae0b-4052-b53a-190707a97431");

/// What the target's cleared to under it, display-encoded: the gradient's dark end, for a frame before its
/// first layout.
pub const CLEAR: [u8; 3] = [14, 18, 46];

/// How many points high it lays out on any target, as the wipe does.
const POINTS_HIGH: f32 = 720.0;

/// What the boot screen's paths read.
#[derive(Clone, Debug, Default, Reflect)]
pub struct BootModel {
    /// The key of what's under way, under `boot-step-`. Empty reads the document's own.
    pub step: String,
    /// How much of the boot is done, 0 to 1.
    pub progress: f32,
    /// The same, in whole percent, for its text.
    pub percent: i32,
}

impl BootModel {
    /// A boot under way, as a golden shows it: building pipelines, two fifths done.
    #[must_use]
    pub fn sample() -> Self {
        Self { step: "boot-step-shaders".to_owned(), progress: 0.4, percent: 40 }
    }
}

/// The boot screen: see the [module docs](self).
#[derive(Debug)]
pub struct BootScreen {
    layer: ScreenStack,
    /// Its clock, in seconds since it was made.
    now: f64,
}

impl BootScreen {
    /// The screen from what `Content::load_screens_alone` gave for [`BOOT_SCREEN`], in `locale`.
    pub fn new(assets: ScreenAssets, locale: &str) -> Result<Self, String> {
        let setup = StackSetup { locale: locale.to_owned(), pseudo: false, scale: 1.0 };
        let document = assets.documents.first().ok_or("no boot screen document")?;
        let mut layer = ScreenStack::new(assets.tables, assets.fonts, Atlas::paint(), setup)?;
        layer.push(document, Box::new(BootModel::default()), Timestamp::from_nanos(0), Vec::new())?;
        Ok(Self { layer, now: 0.0 })
    }

    /// Shows `step`, a key under `boot-step-`, with `progress` of the whole boot done, from the next draw.
    pub fn show(&mut self, step: &str, progress: f32) {
        let progress = progress.clamp(0.0, 1.0);
        let model = self.layer.model_mut::<BootModel>(0).expect("the boot screen reads a BootModel");
        if model.step != step {
            model.step.clear();
            model.step.push_str(step);
        }
        model.progress = progress;
        model.percent = (progress * 100.0).floor() as i32;
    }

    /// What the glyph pages it draws into hold, for the next draw.
    pub fn follow_pages(&mut self, pages: UiPageState) {
        self.layer.follow_pages(pages);
    }

    /// Writes the screen into `list`, for a target of `size` pixels, `dt` real seconds after the last draw.
    pub fn draw(&mut self, size: (u32, u32), dt: f32, list: &mut UiDrawList) {
        self.now += f64::from(dt.max(0.0));
        self.layer.set_now(Timestamp::from_nanos((self.now * 1e9) as u64));
        self.layer.set_scale((size.1 as f32 / POINTS_HIGH).max(f32::MIN_POSITIVE));
        self.layer.draw(size, list);
    }

    /// The tree, for a test of what it shows.
    #[must_use]
    pub fn tree(&self) -> &sapphire_ui::UiTree {
        self.layer.tree(0)
    }
}

#[cfg(test)]
mod tests {
    use bevy_reflect::Typed;
    use sapphire_assets::ui::{UiDocument, UiModel};

    use super::*;

    const DOCUMENT: &str = include_str!("../../../content/ui/boot.ui.ron");

    #[test]
    fn the_screen_is_named_by_its_sidecar_and_reads_its_model() {
        let sidecar = include_str!("../../../content/ui/boot.ui.ron.meta");
        assert!(sidecar.contains(&BOOT_SCREEN.to_string()), "{BOOT_SCREEN} isn't in {sidecar}");
        let document = UiDocument::from_document(DOCUMENT).expect("it reads");
        assert_eq!(document.model.as_deref(), Some("boot.model.ron"));
        let model =
            UiModel::from_document(include_str!("../../../content/ui/boot.model.ron")).expect("it reads");
        assert_eq!(model, UiModel::of_type(BootModel::type_info()));
        let issues = document.path_issues(&model);
        assert!(issues.is_empty(), "{issues:?}");
    }

    #[test]
    fn it_shows_the_step_and_how_far_the_boot_has_got() {
        let document = UiDocument::from_document(DOCUMENT).expect("it reads");
        let table =
            sapphire_loc::StringTable { locale: "en".to_owned(), ..sapphire_loc::StringTable::default() };
        let assets = ScreenAssets { documents: vec![document], tables: vec![table], fonts: Vec::new() };
        let mut screen = BootScreen::new(assets, "en").expect("a screen");
        screen.show("boot-step-loading", 1.4);
        let model = screen.layer.model_mut::<BootModel>(0).expect("its model");
        assert_eq!((model.step.as_str(), model.progress, model.percent), ("boot-step-loading", 1.0, 100));
        screen.show("boot-step-shaders", 0.257);
        let mut list = UiDrawList::default();
        screen.draw((320, 180), 1.0 / 60.0, &mut list);
        assert!(!list.quads.is_empty(), "it covers the screen from its first frame");
        let model = screen.layer.model_mut::<BootModel>(0).expect("its model");
        assert_eq!((model.step.as_str(), model.percent), ("boot-step-shaders", 25));
        let tree = screen.tree();
        assert!(tree.is_visible(tree.find("bar").expect("its bar")));
    }
}
