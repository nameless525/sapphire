//! `--display-text latin|japanese` (M3 U8): a title in the display chain's face at four sizes over the
//! frame, the smaller two drawn from the glyph pages and the larger two from the face's cooked field,
//! so one capture shows both paths and where they meet. The `display-text-*` golden scenes capture it.
//!
//! Its fonts come as a screen's would: the fonts pack, the font chains' `display` chain, and each
//! font's cooked file and field, which [`sapphire_ui::face_of`] turns into what `sapphire_text` takes.
//! Its family list is the one its language's locale document gives `display` (`content/loc/`). It
//! draws straight from the shaper, with no document, over the HUD and any screen, through
//! [`ShownFight::over`]; a screen's display text takes the same fields through `UiFonts::register`.
//!
//! The frame is 360 points tall, whatever its pixels, as a phone's is: a 640 × 360 capture is one
//! pixel to the point, and 1920 × 1080 three.
//!
//! [`ShownFight::over`]: game_ui::hud::ShownFight::over
use sapphire_assets::AssetId;
use sapphire_assets::font::{FONTS_PACK, Font, FontChains};
use sapphire_platform::AppInitError;
use sapphire_render::{PipelineInventory, Renderer, UiDrawList, UiPageState};
use sapphire_rhi::GpuContext;
use sapphire_stage::{Content, SceneShow};
use sapphire_text::{AtlasConfig, GlyphAtlas, TextStyle, TextSystem, family_list};

/// `content/fonts/ui.fontchains.ron`'s ID.
const FONT_CHAINS: AssetId = AssetId::from_text("6258611d-eb23-4958-9680-4e262a60a6ea");
/// The frame's height in points.
const POINTS_TALL: f32 = 360.0;
/// Each line's size in points and its baseline's height, in points from the top: 12 and 24 points
/// are under a field's 48 pixels to the em at one pixel to the point, and 48 and 96 over it.
const LINES: [(f32, f32); 4] = [(12.0, 40.0), (24.0, 80.0), (48.0, 150.0), (96.0, 280.0)];
/// Where each line starts, in points from the left.
const MARGIN: f32 = 24.0;
/// A pale ink, nearly opaque, over a night street.
const INK: [u8; 4] = [244, 240, 255, 255];

/// What `--display-text` shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum DisplaySample {
    /// The game's name in Oxanium.
    Latin,
    /// A result screen's words in Dela Gothic One, one Han of which (城) isn't in the field, so it
    /// draws from the pages beside the field's glyphs at every size.
    Japanese,
}

impl DisplaySample {
    pub(crate) fn parse(text: &str) -> Result<Self, String> {
        match text {
            "latin" => Ok(Self::Latin),
            "japanese" => Ok(Self::Japanese),
            other => Err(format!("--display-text expects latin or japanese, got {other:?}")),
        }
    }

    fn text(self) -> &'static str {
        match self {
            Self::Latin => "SAPPHIRE",
            Self::Japanese => "勝利の城へ",
        }
    }

    /// What its language shows `display` text in, first choice first, as `content/loc/`'s locale
    /// documents say.
    fn families(self) -> &'static [&'static str] {
        match self {
            Self::Latin => &["Oxanium", "M PLUS 1"],
            Self::Japanese => &["Dela Gothic One", "Oxanium", "M PLUS 1"],
        }
    }

    fn locale(self) -> &'static str {
        match self {
            Self::Latin => "en",
            Self::Japanese => "ja",
        }
    }
}

/// See the [module docs](self).
pub(crate) struct DisplayText {
    sample: DisplaySample,
    text: TextSystem,
    atlas: GlyphAtlas,
    families: String,
}

impl DisplayText {
    /// Mounts the fonts pack and loads the display chain's fonts.
    pub(crate) fn load(
        sample: DisplaySample,
        content: &mut Content,
        renderer: &mut Renderer,
        gpu: &GpuContext,
    ) -> Result<Self, AppInitError> {
        content.mount_pack(FONTS_PACK)?;
        let chains = content.hold::<FontChains>(FONT_CHAINS, renderer, gpu)?;
        let chains =
            content.read(&chains).ok_or_else(|| AppInitError::msg("the font chains aren't loaded"))?;
        let ids = chains.fonts("display").ok_or_else(|| AppInitError::msg("no display chain"))?.to_vec();
        let mut text = TextSystem::new();
        for id in ids {
            let handle = content.hold::<Font>(id, renderer, gpu)?;
            let font = content
                .read(&handle)
                .ok_or_else(|| AppInitError::msg(format!("the font {id} isn't loaded")))?;
            let (file, options) = sapphire_ui::face_of(font.to_owned_font())
                .map_err(|error| AppInitError::msg(format!("the font {id}: {error}")))?;
            text.register_face(file, options);
        }
        let families = family_list(sample.families().iter().copied());
        Ok(Self { sample, text, atlas: GlyphAtlas::new(AtlasConfig::default()), families })
    }
}

impl SceneShow for DisplayText {
    fn declare_pipelines(&self, inventory: &mut PipelineInventory) {
        inventory.ui = true;
    }

    fn begin_interface(&mut self, pages: UiPageState) {
        self.text.begin_frame();
        self.atlas.begin_frame(pages);
    }

    fn draw_interface(&mut self, size: (u32, u32), list: &mut UiDrawList) {
        if list.viewport == [0.0; 2] {
            list.viewport = [size.0 as f32 * POINTS_TALL / size.1 as f32, POINTS_TALL];
        }
        let scale = size.1 as f32 / list.viewport[1];
        let weight = match self.sample {
            DisplaySample::Latin => 700.0,
            DisplaySample::Japanese => 400.0,
        };
        for (size, baseline) in LINES {
            let style = TextStyle {
                families: &self.families,
                size,
                weight,
                line_height: 1.0,
                locale: Some(self.sample.locale()),
            };
            let shaped = self.text.shape(self.sample.text(), &style, scale, None);
            let top = baseline - shaped.baselines.first().copied().unwrap_or(0.0) / scale;
            self.atlas.draw(shaped, [MARGIN, top], scale, INK, list);
        }
    }
}
