//! The frame's on-screen interface as plain data (rule R4, spec §4.6 and §5.17): what
//! `sapphire_ui` paints and the renderer draws over the finished frame, after post, knowing nothing
//! of widgets, touches or fighters.
//!
//! A [`UiDrawList`] is a list of [`UiQuad`]s in UI points, each a parallelogram over one cell of a
//! [`UiAtlas`], a signed distance field, or of a glyph page, a single-channel texture of coverage that
//! the list's own uploads fill in. The list's own atlas is the HUD's; a display face's multi-channel
//! field is another atlas the list names ([`UiDrawList::fields`]). An empty list costs the renderer
//! nothing: it builds no pipeline and adds no pass.

use std::fmt;
use std::sync::Arc;

/// A signed distance field atlas with one mip level: a texel holds `0.5 + distance / (2 × spread)`,
/// distance in texels and positive inside a shape, so 0.5 is the shape's edge wherever the quad scales
/// or shears it.
///
/// The renderer uploads it the first time a list with quads names it, and again only if a list
/// names another: the [`Arc`] is what it compares, so a renderer made after device loss or for a
/// new tier uploads it without the app doing anything.
#[derive(Clone, PartialEq)]
pub struct UiAtlas {
    pub width: u32,
    pub height: u32,
    /// How far from an edge, in texels, the field reaches before it clamps.
    pub spread: f32,
    pub format: UiFieldFormat,
    /// `width × height` texels of [`UiFieldFormat::bytes_per_texel`] each, row by row from the top.
    pub texels: Vec<u8>,
}

/// What a [`UiAtlas`]'s texels hold.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum UiFieldFormat {
    /// `R8Unorm`: the distance, as the HUD's painted shapes are.
    #[default]
    Single,
    /// `Rgba8Unorm`: a multi-channel field in red, green and blue, whose median is the distance and
    /// keeps a corner sharp however far a quad magnifies it, and the true distance in alpha, smooth
    /// where the channels change over, which edges and outlines read. A display face's glyphs, cooked.
    Multi,
}

impl UiFieldFormat {
    pub const fn bytes_per_texel(self) -> u32 {
        match self {
            Self::Single => 1,
            Self::Multi => 4,
        }
    }
}

impl fmt::Debug for UiAtlas {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("UiAtlas")
            .field("width", &self.width)
            .field("height", &self.height)
            .field("spread", &self.spread)
            .field("format", &self.format)
            .field("bytes", &self.texels.len())
            .finish()
    }
}

/// One quad, as a game describes it. The renderer works each one out into its own form for its
/// shader (`sapphire_render`'s `ui` module), so this layout is the list's and not the GPU's.
///
/// **Colors are display-encoded:** sRGB bytes with straight alpha, as a designer picks them. The
/// renderer decodes them to linear, so an opaque color lands on exactly the code it names through the
/// target's encode, and blends in linear light (see `sapphire_render`'s `ui` module), which lets
/// more of a bright background through a translucent dark than a display-space blend does.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
pub struct UiQuad {
    /// The corner of the cell's first texel, in UI points from the viewport's top-left, +y down.
    pub origin: [f32; 2],
    /// From there across the quad's width. A quad may turn or scale freely.
    pub axis_x: [f32; 2],
    /// From there down the quad's height: not square to `axis_x` for a slanted quad.
    pub axis_y: [f32; 2],
    /// The atlas cell: `u0, v0, u1, v1`, normalized.
    pub uv: [f32; 4],
    pub color: [u8; 4],
    pub outline_color: [u8; 4],
    /// The field value of the fill's edge: 0.5 is the shape as painted, higher thins it.
    pub edge: f32,
    /// How far the fill's edge and the outline's feather, in field values each side, for shadows
    /// and glows. 0 is a crisp one-pixel edge at any scale.
    pub softness: f32,
    /// The outline's width outward from `edge`, in field values. 0 draws none.
    pub outline: f32,
    /// A share of a turn clockwise from the quad's top that draws, for a cooldown's sweep; 0
    /// draws the whole shape.
    pub sweep: f32,
    /// The range across the quad's width that draws, 0 to 1, with an antialiased cut: a bar's fill,
    /// which follows the quad's slant.
    pub clip: [f32; 2],
    /// A three-slice: the share of the quad's width each end takes from each half of the cell,
    /// the rest stretching the cell's middle column, so a bar's ends keep their shape at any
    /// length. 0 maps the cell across the quad.
    pub slice: f32,
    /// The fill's far color, display-encoded like [`Self::color`], which a gradient mixes toward in
    /// linear light. Nothing reads it while [`Self::gradient`] is all zeros.
    pub color_end: [u8; 4],
    /// How far along the gradient each point of the quad is: `x`, `y`, `offset`, so the share of
    /// [`Self::color_end`] at `local` is `x * local.x + y * local.y + offset`, clamped to 0 to 1, with
    /// `local` running 0 to 1 along the quad's own axes. A gradient across several quads gives each
    /// its own piece. All zeros is a plain fill.
    pub gradient: [f32; 3],
    /// What its texels hold: [`UI_FIELD`], [`UI_COVERAGE`], [`UI_MULTI_FIELD`] or [`UI_PICTURE`].
    pub texels: u32,
}

/// A quad's [`UiQuad::texels`] over a single-channel field ([`UiFieldFormat::Single`]).
pub const UI_FIELD: u32 = 0;
/// A quad's [`UiQuad::texels`] over coverage, not a field, as a glyph page holds: they draw as they
/// are, with no outline or softness.
pub const UI_COVERAGE: u32 = 1;
/// A quad's [`UiQuad::texels`] over a multi-channel field ([`UiFieldFormat::Multi`]): the median of
/// three is its edge, and the true distance its outline and softness.
pub const UI_MULTI_FIELD: u32 = 2;

/// A quad's [`UiQuad::texels`] over a picture: an inset's (M3 U6), linear color premultiplied by its
/// alpha, which the quad's color tints and fades.
pub const UI_PICTURE: u32 = 3;

impl UiQuad {
    /// A quad over `uv` in `color`, the shape as painted, nothing else on.
    pub const fn new(
        origin: [f32; 2],
        axis_x: [f32; 2],
        axis_y: [f32; 2],
        uv: [f32; 4],
        color: [u8; 4],
    ) -> Self {
        Self {
            origin,
            axis_x,
            axis_y,
            uv,
            color,
            outline_color: [0; 4],
            edge: 0.5,
            softness: 0.0,
            outline: 0.0,
            sweep: 0.0,
            clip: [0.0, 1.0],
            slice: 0.0,
            color_end: [0; 4],
            gradient: [0.0; 3],
            texels: UI_FIELD,
        }
    }

    /// A glyph's quad, with its top-left at `origin` and `size` across and down: coverage from a
    /// glyph page, which draws as it is.
    pub const fn glyph(origin: [f32; 2], size: [f32; 2], uv: [f32; 4], color: [u8; 4]) -> Self {
        let mut quad = Self::new(origin, [size[0], 0.0], [0.0, size[1]], uv, color);
        quad.texels = UI_COVERAGE;
        quad
    }

    /// A display glyph's quad, placed as [`Self::glyph`] places one, over a multi-channel field: crisp
    /// at any size from one cell, and it takes an outline and softness as the HUD's shapes do.
    pub const fn field_glyph(origin: [f32; 2], size: [f32; 2], uv: [f32; 4], color: [u8; 4]) -> Self {
        let mut quad = Self::new(origin, [size[0], 0.0], [0.0, size[1]], uv, color);
        quad.texels = UI_MULTI_FIELD;
        quad
    }

    /// An inset's picture over the rectangle from `origin` across `axis_x` and down `axis_y`, all of
    /// it, faded by `opacity` (0 to 255).
    pub const fn picture(origin: [f32; 2], axis_x: [f32; 2], axis_y: [f32; 2], opacity: u8) -> Self {
        let mut quad = Self::new(origin, axis_x, axis_y, [0.0, 0.0, 1.0, 1.0], [255, 255, 255, opacity]);
        quad.texels = UI_PICTURE;
        quad
    }
}

/// What a run of quads reads instead of the list's atlas.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UiSource {
    /// A glyph page, by its number.
    Page(u16),
    /// One of [`UiDrawList::fields`], by its place there.
    Field(u16),
}

/// A run of [`UiDrawList::quads`] that reads a glyph page or a field, not the list's atlas.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct UiRun {
    pub first: u32,
    pub count: u32,
    pub source: UiSource,
}

/// A run of [`UiDrawList::quads`] that shows an inset's picture (M3 U6): what the frame packet's inset
/// view numbered `inset` drew. A renderer drawing no such inset draws none of the run.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct UiInsetRun {
    pub first: u32,
    pub count: u32,
    pub inset: u16,
}

/// From quad `first` of [`UiDrawList::quads`] on, until the next of these, only what's inside `rect`
/// draws: a scissor, with no edge softer than a pixel, so a mask costs a draw call and no pipeline.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct UiMask {
    pub first: u32,
    /// In UI points: `x0, y0, x1, y1`. `None` draws everything again.
    pub rect: Option<[f32; 4]>,
}

/// New texels for a rectangle of a glyph page, tightly packed in [`UiDrawList::upload_texels`] from
/// `offset`, row by row from the top.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct UiPageUpload {
    pub page: u16,
    pub x: u16,
    pub y: u16,
    pub width: u16,
    pub height: u16,
    pub offset: u32,
}

/// What a renderer's glyph pages hold, which a glyph atlas reads at the start of every frame: the
/// way back from the renderer that the frame packet can't be. A new renderer has a new `store`, so
/// an atlas sees that everything it handed over is gone; a frame whose list was never prepared
/// leaves `batch` behind, so it sees that that frame's uploads are.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct UiPageState {
    /// Whose pages. A renderer's is unique in the process and never 0.
    pub store: u64,
    /// The last batch of uploads applied.
    pub batch: u64,
}

impl UiPageState {
    /// For an atlas with nothing behind it, as in a test or a tool that reads the list itself:
    /// nothing is ever lost. A renderer draws nothing from a list made this way.
    pub const NO_RENDERER: Self = Self { store: 0, batch: u64::MAX };
}

/// What a glyph atlas did this frame and holds now (rule R9), which the list carries to the
/// renderer's stats.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct UiGlyphStats {
    pub rasterized: u32,
    pub upload_bytes: u64,
    pub evicted: u32,
    /// Glyphs that didn't fit anywhere and didn't draw.
    pub dropped: u32,
    /// Whether it started over this frame, because the pages it knew were gone.
    pub forgot: bool,
    /// Glyphs drawn from a display face's field this frame, which take nothing from the pages.
    pub field_quads: u32,
    /// Pages that exist now.
    pub pages: u32,
    /// Glyphs held now.
    pub glyphs: u32,
    /// Bytes of the pages' area in use now, padding included.
    pub used_bytes: u64,
}

/// What the retained trees behind a list did this frame (rule R9), which the list carries to the
/// renderer's stats beside [`UiGlyphStats`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct UiTreeStats {
    /// Trees drawn.
    pub trees: u32,
    /// Nodes they hold.
    pub nodes: u32,
    /// Layout passes: 0 on a frame where nothing changed.
    pub layouts: u32,
    /// Strings resolved through the localizer: 0 on a frame where nothing changed.
    pub resolves: u32,
    /// Bound values that moved.
    pub bound_changes: u32,
    /// Texts the shaper had to shape, not find.
    pub shaped: u32,
    /// What's wrong with a tree that a person should see: a path that doesn't resolve, a role no font
    /// chain answers. They show on screen too.
    pub problems: u32,
    /// Timelines running: 0 on a screen at rest.
    pub playing: u32,
    /// Scrollers on their way somewhere.
    pub scrolling: u32,
    /// How long the trees took on the frame's thread, in microseconds.
    pub micros: u32,
}

/// [`UiDrawList::page_uploads`] joined into fewer rectangles, which is what makes a screen of new
/// glyphs cheap: a texture write has a fixed cost much larger than its bytes'. Kept between frames
/// for its vectors' capacity.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct UiPageBands {
    order: Vec<u32>,
    /// With `offset` into [`Self::texels`].
    pub bands: Vec<UiPageUpload>,
    pub texels: Vec<u8>,
}

/// What the frame draws over itself after post: the frame packet's `ui`, kept between frames for
/// its vector's capacity. `sapphire_ui` writes it; nothing above the renderer reads it back.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct UiDrawList {
    /// The frame's size in UI points, which every quad's position is in. Pixels never appear here:
    /// the shader's edges come from the field's screen-space slope, so a list scales to any target.
    pub viewport: [f32; 2],
    /// The atlas the quads read. A list with quads and no atlas draws nothing.
    pub atlas: Option<Arc<UiAtlas>>,
    /// Back to front.
    pub quads: Vec<UiQuad>,
    /// The quads that read a glyph page or a field, in order and apart. Every other quad reads `atlas`,
    /// but those of [`Self::inset_runs`].
    pub runs: Vec<UiRun>,
    /// The quads that show an inset's picture, in order and apart from each other and the runs'.
    pub inset_runs: Vec<UiInsetRun>,
    /// The display faces' fields the runs read, each once. The renderer uploads one the first time a
    /// list names it, compares them by [`Arc`] as it does `atlas`, and lets it go once nothing but the
    /// renderer holds it.
    pub fields: Vec<Arc<UiAtlas>>,
    /// Where masks start and stop, by ascending `first`. Empty is no mask at all.
    pub masks: Vec<UiMask>,
    /// A glyph page's side in texels, all of them `R8Unorm` with one mip level.
    pub page_size: u32,
    /// What changed in the pages since the last list, to apply before this list draws. A list that
    /// names a page the renderer doesn't have makes it, empty.
    ///
    /// Two rectangles at one `y` of a page, side by side, may be written as one as tall as the taller,
    /// so whoever fills this keeps nothing they need under the shorter ([`Self::page_bands`]).
    pub page_uploads: Vec<UiPageUpload>,
    /// The bytes [`Self::page_uploads`] name.
    pub upload_texels: Vec<u8>,
    /// What the pages hold once this list is applied. A renderer whose [`UiPageState::store`] this
    /// isn't draws none of the list's page quads and applies none of its uploads.
    pub page_state: UiPageState,
    /// What the glyph atlas did for this frame.
    pub glyphs: UiGlyphStats,
    /// What the retained trees did for this frame.
    pub tree: UiTreeStats,
}

impl UiDrawList {
    /// Empties the list for the next frame, keeping the vector's capacity.
    pub fn reset(&mut self) {
        self.viewport = [0.0; 2];
        self.atlas = None;
        self.quads.clear();
        self.runs.clear();
        self.inset_runs.clear();
        self.fields.clear();
        self.masks.clear();
        self.page_size = 0;
        self.page_uploads.clear();
        self.upload_texels.clear();
        self.page_state = UiPageState::default();
        self.glyphs = UiGlyphStats::default();
        self.tree = UiTreeStats::default();
    }

    /// Quads added from now on draw only inside `rect` (min x, min y, max x, max y, in points), or everywhere with `None`.
    pub fn set_mask(&mut self, rect: Option<[f32; 4]>) {
        let first = self.quads.len() as u32;
        match self.masks.last_mut() {
            Some(last) if last.first == first => last.rect = rect,
            Some(last) if last.rect == rect => {}
            None if rect.is_none() => {}
            _ => self.masks.push(UiMask { first, rect }),
        }
    }

    /// Joins [`Self::page_uploads`] that lie side by side at one `y` of a page into `out`, with zeros
    /// under the shorter ones, and leaves out any whose bytes aren't there: how many it left out.
    /// Allocates nothing once `out` has grown.
    pub fn page_bands(&self, out: &mut UiPageBands) -> u32 {
        out.order.clear();
        out.bands.clear();
        out.texels.clear();
        let bytes = |upload: &UiPageUpload| usize::from(upload.width) * usize::from(upload.height);
        let present = |upload: &UiPageUpload| {
            let start = upload.offset as usize;
            start.checked_add(bytes(upload)).is_some_and(|end| end <= self.upload_texels.len())
        };
        let uploads = &self.page_uploads;
        out.order.extend((0..uploads.len() as u32).filter(|&at| present(&uploads[at as usize])));
        let left_out = (uploads.len() - out.order.len()) as u32;
        out.order.sort_unstable_by_key(|&at| {
            let upload = &uploads[at as usize];
            (upload.page, upload.y, upload.x)
        });
        let mut next = 0;
        while next < out.order.len() {
            let first = uploads[out.order[next] as usize];
            let (mut end, mut height, mut x_end) =
                (next + 1, first.height, u32::from(first.x) + u32::from(first.width));
            while let Some(upload) = out.order.get(end).map(|&at| &uploads[at as usize]) {
                let joins = upload.page == first.page && upload.y == first.y && u32::from(upload.x) == x_end;
                if !joins || x_end + u32::from(upload.width) > u32::from(u16::MAX) {
                    break;
                }
                x_end += u32::from(upload.width);
                height = height.max(upload.height);
                end += 1;
            }
            let width = (x_end - u32::from(first.x)) as usize;
            let offset = out.texels.len();
            out.texels.resize(offset + width * usize::from(height), 0);
            let mut x = 0;
            for &at in &out.order[next..end] {
                let upload = &uploads[at as usize];
                let (w, start) = (usize::from(upload.width), upload.offset as usize);
                for row in 0..usize::from(upload.height) {
                    let from = start + row * w;
                    let to = offset + row * width + x;
                    out.texels[to..to + w].copy_from_slice(&self.upload_texels[from..from + w]);
                }
                x += w;
            }
            out.bands.push(UiPageUpload { width: width as u16, height, offset: offset as u32, ..first });
            next = end;
        }
        left_out
    }

    /// Adds a quad showing inset `inset`'s picture, joining the run before it if that's of the same
    /// inset.
    pub fn push_inset_quad(&mut self, inset: u16, quad: UiQuad) {
        let at = self.quads.len() as u32;
        self.quads.push(quad);
        match self.inset_runs.last_mut() {
            Some(run) if run.inset == inset && run.first + run.count == at => run.count += 1,
            _ => self.inset_runs.push(UiInsetRun { first: at, count: 1, inset }),
        }
    }

    /// Adds a quad over a glyph page, joining the run before it if that's on the same page.
    pub fn push_page_quad(&mut self, page: u16, quad: UiQuad) {
        self.push_run_quad(UiSource::Page(page), quad);
    }

    /// Adds a quad over `field`, naming it in [`Self::fields`] if this list doesn't yet, and joining
    /// the run before it if that reads the same field. Allocates nothing once the vectors have grown.
    pub fn push_field_quad(&mut self, field: &Arc<UiAtlas>, quad: UiQuad) {
        let at = match self.fields.iter().position(|named| Arc::ptr_eq(named, field)) {
            Some(at) => at,
            None => {
                self.fields.push(Arc::clone(field));
                self.fields.len() - 1
            }
        };
        self.push_run_quad(UiSource::Field(at as u16), quad);
    }

    fn push_run_quad(&mut self, source: UiSource, quad: UiQuad) {
        let at = self.quads.len() as u32;
        self.quads.push(quad);
        match self.runs.last_mut() {
            Some(run) if run.source == source && run.first + run.count == at => run.count += 1,
            _ => self.runs.push(UiRun { first: at, count: 1, source }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_mask_starts_where_the_quads_are_and_says_nothing_it_needn_t() {
        let quad = UiQuad::new([0.0; 2], [1.0, 0.0], [0.0, 1.0], [0.0; 4], [255; 4]);
        let rect = Some([0.0, 0.0, 8.0, 8.0]);
        let mut list = UiDrawList::default();
        list.set_mask(None);
        assert!(list.masks.is_empty(), "no mask to stop");
        list.quads.push(quad);
        list.set_mask(Some([1.0; 4]));
        list.set_mask(rect);
        assert_eq!(list.masks, [UiMask { first: 1, rect }], "nothing drew under the first");
        list.quads.push(quad);
        list.set_mask(rect);
        assert_eq!(list.masks.len(), 1, "the same mask goes on");
        list.set_mask(None);
        assert_eq!(list.masks[1], UiMask { first: 2, rect: None });
        list.reset();
        assert!(list.masks.is_empty());
    }

    #[test]
    fn page_quads_join_a_run_only_on_its_page_and_right_after_it() {
        let quad = UiQuad::glyph([0.0; 2], [1.0; 2], [0.0; 4], [255; 4]);
        let mut list = UiDrawList::default();
        list.push_page_quad(0, quad);
        list.push_page_quad(0, quad);
        list.push_page_quad(1, quad);
        list.quads.push(quad);
        list.push_page_quad(1, quad);
        assert_eq!(
            list.runs,
            [
                UiRun { first: 0, count: 2, source: UiSource::Page(0) },
                UiRun { first: 2, count: 1, source: UiSource::Page(1) },
                UiRun { first: 4, count: 1, source: UiSource::Page(1) },
            ],
            "a field quad between two quads of a page keeps its place",
        );
        list.reset();
        assert!(list.runs.is_empty() && list.quads.is_empty());
    }

    #[test]
    fn a_field_is_named_once_and_its_quads_run_apart_from_a_page_s() {
        let field = || {
            Arc::new(UiAtlas {
                width: 1,
                height: 1,
                spread: 4.0,
                format: UiFieldFormat::Multi,
                texels: vec![0; 4],
            })
        };
        let (one, two) = (field(), field());
        let quad = UiQuad::field_glyph([0.0; 2], [1.0; 2], [0.0; 4], [255; 4]);
        let mut list = UiDrawList::default();
        list.push_field_quad(&one, quad);
        list.push_field_quad(&one, quad);
        list.push_page_quad(0, UiQuad::glyph([0.0; 2], [1.0; 2], [0.0; 4], [255; 4]));
        list.push_field_quad(&two, quad);
        list.push_field_quad(&one, quad);
        assert_eq!(list.fields.len(), 2, "equal fields are still two: a field is its Arc");
        assert!(Arc::ptr_eq(&list.fields[0], &one) && Arc::ptr_eq(&list.fields[1], &two));
        assert_eq!(
            list.runs,
            [
                UiRun { first: 0, count: 2, source: UiSource::Field(0) },
                UiRun { first: 2, count: 1, source: UiSource::Page(0) },
                UiRun { first: 3, count: 1, source: UiSource::Field(1) },
                UiRun { first: 4, count: 1, source: UiSource::Field(0) },
            ],
        );
        assert_eq!(list.quads[0].texels, UI_MULTI_FIELD);
        list.reset();
        assert!(list.fields.is_empty() && Arc::strong_count(&one) == 1, "a reset list holds no field");
    }

    #[test]
    fn inset_quads_join_a_run_only_of_their_inset_and_right_after_it() {
        let quad = UiQuad::picture([0.0; 2], [1.0, 0.0], [0.0, 1.0], 255);
        let mut list = UiDrawList::default();
        list.push_inset_quad(0, quad);
        list.push_inset_quad(0, quad);
        list.push_page_quad(0, UiQuad::glyph([0.0; 2], [1.0; 2], [0.0; 4], [255; 4]));
        list.push_inset_quad(0, quad);
        list.push_inset_quad(1, quad);
        assert_eq!(
            list.inset_runs,
            [
                UiInsetRun { first: 0, count: 2, inset: 0 },
                UiInsetRun { first: 3, count: 1, inset: 0 },
                UiInsetRun { first: 4, count: 1, inset: 1 },
            ],
        );
        assert_eq!(list.runs, [UiRun { first: 2, count: 1, source: UiSource::Page(0) }]);
        assert_eq!(list.quads[0].texels, UI_PICTURE);
        list.reset();
        assert!(list.inset_runs.is_empty());
    }

    fn with_uploads(uploads: &[(u16, u16, u16, u16, u16, u8)]) -> UiDrawList {
        let mut list = UiDrawList::default();
        for &(page, x, y, width, height, value) in uploads {
            let offset = list.upload_texels.len() as u32;
            list.upload_texels.resize(offset as usize + usize::from(width) * usize::from(height), value);
            list.page_uploads.push(UiPageUpload { page, x, y, width, height, offset });
        }
        list
    }

    #[test]
    fn rectangles_side_by_side_join_with_zeros_under_the_shorter() {
        // In the order an atlas might hand them over, which isn't the page's.
        let list = with_uploads(&[(0, 3, 8, 1, 2, 7), (0, 0, 8, 2, 1, 5), (0, 2, 8, 1, 3, 6)]);
        let mut out = UiPageBands::default();
        assert_eq!(list.page_bands(&mut out), 0);
        assert_eq!(out.bands, [UiPageUpload { page: 0, x: 0, y: 8, width: 4, height: 3, offset: 0 }]);
        #[rustfmt::skip]
        assert_eq!(out.texels, [
            5, 5, 6, 7,
            0, 0, 6, 7,
            0, 0, 6, 0,
        ]);
    }

    #[test]
    fn a_gap_a_row_or_a_page_keeps_rectangles_apart() {
        let list =
            with_uploads(&[(0, 0, 0, 2, 1, 1), (0, 3, 0, 2, 1, 2), (0, 5, 4, 2, 1, 3), (1, 5, 0, 2, 1, 4)]);
        let mut out = UiPageBands::default();
        assert_eq!(list.page_bands(&mut out), 0);
        assert_eq!(out.bands.len(), 4);
        assert_eq!(out.texels, [1, 1, 2, 2, 3, 3, 4, 4]);
    }

    #[test]
    fn a_rectangle_past_its_bytes_is_left_out_and_counted() {
        let mut list = with_uploads(&[(0, 0, 0, 2, 2, 1), (0, 8, 0, 2, 2, 2)]);
        list.upload_texels.truncate(6);
        let mut out = UiPageBands::default();
        assert_eq!(list.page_bands(&mut out), 1);
        assert_eq!(out.bands.len(), 1);
        assert_eq!(out.texels, [1; 4]);
    }

    #[test]
    fn nothing_to_join_costs_nothing_once_it_has_grown() {
        let mut out = UiPageBands::default();
        with_uploads(&[(0, 0, 0, 8, 8, 1)]).page_bands(&mut out);
        let capacity = out.texels.capacity();
        UiDrawList::default().page_bands(&mut out);
        assert!(out.bands.is_empty() && out.texels.is_empty());
        assert_eq!(out.texels.capacity(), capacity);
    }
}
