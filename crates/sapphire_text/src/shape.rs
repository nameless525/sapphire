//! Text to positioned glyphs, through `parley`, with a cache in front so a frame that changed nothing
//! costs a hash per string.

use std::collections::HashMap;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::ops::Range;
use std::sync::Arc;

use parley::fontique::{Blob, Collection, CollectionOptions, FamilyId, SourceCache};
use parley::style::{FontFamily, FontWeight, LineHeight, StyleProperty};
use parley::{
    Alignment, AlignmentOptions, FontContext, FontData, Layout, LayoutContext, PositionedLayoutItem,
};

/// Layouts unused for this many frames go when the cache is over [`MAX_LAYOUTS`].
const LAYOUT_KEEP_FRAMES: u64 = 120;
/// How many layouts the cache holds before it looks for old ones: several screens' worth.
const MAX_LAYOUTS: usize = 4096;

/// The sizes text comes in, in UI points. Each is a whole number of pixels at the scales a device
/// has, or rounds to one, and each size a screen uses holds its own glyphs in the pages, so a screen
/// picks from these and a size never moves: a tween scales the quads.
pub const TYPE_SCALE: [f32; 9] = [12.0, 14.0, 16.0, 18.0, 20.0, 24.0, 32.0, 48.0, 64.0];

/// What a font needs besides its bytes.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct FaceOptions {
    /// How far down its lines' baselines move, in ems, from where the font's own metrics put them:
    /// what puts a title in this face on the same baseline as one in another, in the same box. A
    /// line takes it from the face of its first glyph.
    pub baseline_shift: f32,
}

/// How a string looks.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TextStyle<'a> {
    /// A CSS font family list, in order: `"Oxanium, 'M PLUS 1'"`, which [`family_list`] makes. This is
    /// how a script gets its fallbacks: the shaper's own table has no entry for hiragana or katakana
    /// with a locale, and a list answers for any script. Past its end a character draws as the first
    /// font's missing glyph, and nothing comes from the OS.
    pub families: &'a str,
    /// In UI points, from [`TYPE_SCALE`]. Times the scale, it rounds to a whole number of pixels.
    pub size: f32,
    /// 100 to 900, along a variable font's `wght` axis.
    pub weight: f32,
    /// A multiple of the font's own line height.
    pub line_height: f32,
    /// A BCP 47 tag, which picks Han forms and fallbacks. It says what language the text is in.
    pub locale: Option<&'a str>,
}

impl TextStyle<'_> {
    fn hash_into(&self, state: &mut impl Hasher) {
        self.families.hash(state);
        self.size.to_bits().hash(state);
        self.weight.to_bits().hash(state);
        self.line_height.to_bits().hash(state);
        self.locale.hash(state);
    }
}

/// Family names, in the order they answer, as [`TextStyle::families`] takes them. A font chain's
/// fonts say their families, and this is what a style is made from.
pub fn family_list<'a>(families: impl IntoIterator<Item = &'a str>) -> String {
    let quoted: Vec<String> = families.into_iter().map(|family| format!("'{family}'")).collect();
    quoted.join(", ")
}

/// A run of glyphs from one font at one size. Nothing in it needs `parley` to read.
#[derive(Clone, Debug)]
pub struct ShapedRun {
    pub font: FontData,
    /// The font's variation coordinates, normalized, in the font's axis order.
    pub coords: Vec<i16>,
    /// The size in pixels.
    pub ppem: f32,
    /// This run's slice of [`ShapedText::glyphs`].
    pub glyphs: Range<u32>,
}

/// One glyph, in pixels from the text's top-left, its baseline at `y`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ShapedGlyph {
    pub id: u32,
    pub x: f32,
    pub y: f32,
}

/// A string shaped and broken into lines at some scale and width.
#[derive(Clone, Debug, Default)]
pub struct ShapedText {
    /// In pixels.
    pub width: f32,
    pub height: f32,
    pub runs: Vec<ShapedRun>,
    pub glyphs: Vec<ShapedGlyph>,
    /// Each line's byte range of the string, in order.
    pub lines: Vec<Range<u32>>,
    /// Each line's baseline, in pixels from the top.
    pub baselines: Vec<f32>,
}

/// What shaping has done since the last [`TextSystem::take_stats`] (rule R9).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ShapeStats {
    pub hits: u32,
    pub misses: u32,
    /// Layouts in the cache now.
    pub cached: u32,
}

struct CachedLayout {
    text: Box<str>,
    families: Box<str>,
    size: f32,
    weight: f32,
    line_height: f32,
    locale: Option<Box<str>>,
    scale: f32,
    max_width: Option<f32>,
    last_used: u64,
    shaped: ShapedText,
}

/// The fonts, the shaper's state and the cache of what it made.
pub struct TextSystem {
    fonts: FontContext,
    layouts: LayoutContext<()>,
    scratch: Layout<()>,
    /// Each font's [`FaceOptions::baseline_shift`], by its blob, where it isn't 0.
    shifts: HashMap<u64, f32>,
    cache: HashMap<u64, CachedLayout>,
    frame: u64,
    stats: ShapeStats,
}

impl std::fmt::Debug for TextSystem {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TextSystem").field("stats", &self.stats).finish_non_exhaustive()
    }
}

impl Default for TextSystem {
    fn default() -> Self {
        Self::new()
    }
}

impl TextSystem {
    /// No fonts yet, and none will come from the OS.
    pub fn new() -> Self {
        let collection = Collection::new(CollectionOptions { shared: false, system_fonts: false });
        Self {
            fonts: FontContext { collection, source_cache: SourceCache::default() },
            layouts: LayoutContext::new(),
            scratch: Layout::new(),
            shifts: HashMap::new(),
            cache: HashMap::new(),
            frame: 0,
            stats: ShapeStats::default(),
        }
    }

    /// Adds every face in `data`, shared rather than copied, and returns the families they joined.
    pub fn register_font(&mut self, data: Arc<Vec<u8>>) -> Vec<FamilyId> {
        self.register_face(data, FaceOptions::default())
    }

    /// As [`Self::register_font`], with what the font needs besides its bytes. A new font can change
    /// how any string shapes, so what's cached goes.
    pub fn register_face(&mut self, data: Arc<Vec<u8>>, options: FaceOptions) -> Vec<FamilyId> {
        let blob = Blob::new(data);
        if options.baseline_shift != 0.0 {
            self.shifts.insert(blob.id(), options.baseline_shift);
        }
        self.cache.clear();
        let families = self.fonts.collection.register_fonts(blob, None);
        families.into_iter().map(|(family, _)| family).collect()
    }

    /// The names of the families in the collection, which are only ever what was registered.
    pub fn family_names(&mut self) -> Vec<String> {
        self.fonts.collection.family_names().map(str::to_owned).collect()
    }

    /// Moves to the next frame, dropping old layouts if the cache is full.
    pub fn begin_frame(&mut self) {
        self.frame += 1;
        if self.cache.len() > MAX_LAYOUTS {
            let frame = self.frame;
            self.cache.retain(|_, layout| layout.last_used + LAYOUT_KEEP_FRAMES >= frame);
        }
    }

    /// `text` shaped in `style` at `scale` pixels per point, wrapped to `max_width` points. The same
    /// arguments again allocate nothing and return what they did last time.
    pub fn shape(
        &mut self,
        text: &str,
        style: &TextStyle<'_>,
        scale: f32,
        max_width: Option<f32>,
    ) -> &ShapedText {
        let mut hasher = DefaultHasher::new();
        text.hash(&mut hasher);
        style.hash_into(&mut hasher);
        scale.to_bits().hash(&mut hasher);
        max_width.map(f32::to_bits).hash(&mut hasher);
        let key = hasher.finish();
        let matches = |layout: &CachedLayout| {
            *layout.text == *text
                && *layout.families == *style.families
                && layout.size == style.size
                && layout.weight == style.weight
                && layout.line_height == style.line_height
                && layout.locale.as_deref() == style.locale
                && layout.scale == scale
                && layout.max_width == max_width
        };
        let hit = self.cache.get(&key).is_some_and(matches);
        if hit {
            self.stats.hits += 1;
        } else {
            self.stats.misses += 1;
            let shaped = self.layout(text, style, scale, max_width);
            let layout = CachedLayout {
                text: text.into(),
                families: style.families.into(),
                size: style.size,
                weight: style.weight,
                line_height: style.line_height,
                locale: style.locale.map(Into::into),
                scale,
                max_width,
                last_used: self.frame,
                shaped,
            };
            self.cache.insert(key, layout);
        }
        self.stats.cached = self.cache.len() as u32;
        let layout = self.cache.get_mut(&key).expect("just found or inserted");
        layout.last_used = self.frame;
        &layout.shaped
    }

    /// What shaping did since the last call.
    pub fn take_stats(&mut self) -> ShapeStats {
        let cached = self.stats.cached;
        std::mem::replace(&mut self.stats, ShapeStats { hits: 0, misses: 0, cached })
    }

    /// Empties the layout cache, for measuring a first layout.
    pub fn clear_cache(&mut self) {
        self.cache.clear();
    }

    fn layout(
        &mut self,
        text: &str,
        style: &TextStyle<'_>,
        scale: f32,
        max_width: Option<f32>,
    ) -> ShapedText {
        // Laid out in pixels, at a whole pixel size, so every scale gets glyphs made for it.
        let mut builder = self.layouts.ranged_builder(&mut self.fonts, text, 1.0, true);
        builder.push_default(StyleProperty::FontFamily(FontFamily::Source(style.families.into())));
        builder.push_default(StyleProperty::FontSize((style.size * scale).round().max(1.0)));
        builder.push_default(StyleProperty::FontWeight(FontWeight::new(style.weight)));
        builder.push_default(StyleProperty::LineHeight(LineHeight::FontSizeRelative(style.line_height)));
        let locale = style.locale.and_then(|tag| parley::style::Language::parse(tag).ok());
        builder.push_default(StyleProperty::Locale(locale));
        builder.build_into(&mut self.scratch, text);
        self.scratch.break_all_lines(max_width.map(|width| width * scale));
        self.scratch.align(Alignment::Start, AlignmentOptions::default());
        flatten(&self.scratch, &self.shifts)
    }
}

fn flatten(layout: &Layout<()>, shifts: &HashMap<u64, f32>) -> ShapedText {
    let mut shaped = ShapedText { width: layout.width(), height: layout.height(), ..ShapedText::default() };
    for line in layout.lines() {
        let range = line.text_range();
        shaped.lines.push(range.start as u32..range.end as u32);
        let mut shift = None;
        for item in line.items() {
            let PositionedLayoutItem::GlyphRun(run) = item else { continue };
            let source = run.run();
            // In whole pixels, as a baseline lands on one.
            let shift = *shift.get_or_insert_with(|| {
                let em = shifts.get(&source.font().data.id()).copied().unwrap_or(0.0);
                (em * source.font_size()).round()
            });
            let start = shaped.glyphs.len() as u32;
            shaped.glyphs.extend(run.positioned_glyphs().map(|glyph| ShapedGlyph {
                id: glyph.id,
                x: glyph.x,
                y: glyph.y + shift,
            }));
            shaped.runs.push(ShapedRun {
                font: source.font().clone(),
                coords: source.normalized_coords().iter().map(|coord| coord.to_bits()).collect(),
                ppem: source.font_size(),
                glyphs: start..shaped.glyphs.len() as u32,
            });
        }
        shaped.baselines.push(line.metrics().baseline + shift.unwrap_or(0.0));
    }
    shaped
}
