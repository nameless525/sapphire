//! The frame's on-screen interface as plain data (rule R4, spec §4.6 and §5.17): what
//! `sapphire_ui` paints and the renderer draws over the finished frame, after post, knowing nothing
//! of widgets, touches or fighters.
//!
//! A [`UiDrawList`] is a list of [`UiQuad`]s in UI points, each a parallelogram over one cell of a
//! [`UiAtlas`], a single-channel signed distance field, or of a glyph page, a single-channel texture
//! of coverage that the list's own uploads fill in. An empty list costs the renderer nothing: it
//! builds no pipeline and adds no pass.

use std::fmt;
use std::sync::Arc;

/// A signed distance field atlas, `R8Unorm` with one mip level: a texel holds
/// `0.5 + distance / (2 × spread)`, distance in texels and positive inside a shape, so 0.5 is the
/// shape's edge wherever the quad scales or shears it.
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
    /// `width × height` bytes, row by row from the top.
    pub texels: Vec<u8>,
}

impl fmt::Debug for UiAtlas {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("UiAtlas")
            .field("width", &self.width)
            .field("height", &self.height)
            .field("spread", &self.spread)
            .field("bytes", &self.texels.len())
            .finish()
    }
}

/// One quad, as the renderer's vertex stage reads it (`shaders/ui.wgsl`, one instance per quad, so
/// the fields are an append-only layout).
///
/// **Colors are display-encoded:** sRGB bytes with straight alpha, as a designer picks them. The
/// shader decodes them to linear, so an opaque color lands on exactly the code it names through the
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
}

/// A quad's [`UiQuad::softness`] when its texels are coverage, not a field: they draw as they are.
pub const UI_COVERAGE: f32 = -1.0;

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
        }
    }

    /// A glyph's quad, with its top-left at `origin` and `size` across and down: coverage from a
    /// glyph page, which draws as it is.
    pub const fn glyph(origin: [f32; 2], size: [f32; 2], uv: [f32; 4], color: [u8; 4]) -> Self {
        let mut quad = Self::new(origin, [size[0], 0.0], [0.0, size[1]], uv, color);
        quad.softness = UI_COVERAGE;
        quad
    }
}

/// A run of [`UiDrawList::quads`] that reads a glyph page, not the list's atlas.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct UiPageRun {
    pub first: u32,
    pub count: u32,
    pub page: u16,
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
    /// The quads that read a glyph page, in order and apart. Every other quad reads `atlas`.
    pub page_runs: Vec<UiPageRun>,
    /// A glyph page's side in texels, all of them `R8Unorm` with one mip level.
    pub page_size: u32,
    /// What changed in the pages since the last list, to apply before this list draws. A list that
    /// names a page the renderer doesn't have makes it, empty.
    pub page_uploads: Vec<UiPageUpload>,
    /// The bytes [`Self::page_uploads`] name.
    pub upload_texels: Vec<u8>,
}

impl UiDrawList {
    /// Empties the list for the next frame, keeping the vector's capacity.
    pub fn reset(&mut self) {
        self.viewport = [0.0; 2];
        self.atlas = None;
        self.quads.clear();
        self.page_runs.clear();
        self.page_size = 0;
        self.page_uploads.clear();
        self.upload_texels.clear();
    }

    /// Adds a quad over a glyph page, joining the run before it if that's on the same page.
    pub fn push_page_quad(&mut self, page: u16, quad: UiQuad) {
        let at = self.quads.len() as u32;
        self.quads.push(quad);
        match self.page_runs.last_mut() {
            Some(run) if run.page == page && run.first + run.count == at => run.count += 1,
            _ => self.page_runs.push(UiPageRun { first: at, count: 1, page }),
        }
    }
}
