//! Glyphs as coverage in pages, rasterized the first time they're drawn and handed to the frame's
//! draw list, which carries them to the renderer (rule R4).
//!
//! A glyph lives at one pixel size, so a quad maps it one to one and the page's filter never blurs
//! it. Baselines snap to whole pixels, and across a line a glyph snaps to a share of a pixel
//! ([`AtlasConfig::subpixel_bins`]). There are no pages per size: glyphs of every size share pages,
//! and sizes come from a type scale ([`TYPE_SCALE`](crate::TYPE_SCALE)), since a size that moves would
//! fill them. A tween scales quads.
//!
//! # What's in the renderer's pages
//!
//! Nothing here keeps a page's texels, which would be a second copy of up to 16 MiB on a phone to
//! save what a loss costs about once a run. [`GlyphAtlas::begin_frame`] takes the renderer's
//! [`UiPageState`] instead, so it can't be called without one: a new renderer, or a frame whose
//! uploads never reached one, makes the atlas forget what it placed, and the frames after rasterize
//! what they draw again.
//!
//! # Eviction
//!
//! A glyph drawn in the last [`AtlasConfig::keep_frames`] frames is never taken out, so what's on
//! screen can't thrash. When every page is full, the least recently drawn glyphs go until an eighth
//! of a page has been freed, so one eviction pays for many glyphs. An atlas with nothing left to give
//! leaves the glyph undrawn this frame and counts it ([`UiGlyphStats::dropped`]), and rasterizes
//! nothing else this frame, so a screen bigger than the atlas costs no more than a full one.
//!
//! # Warming
//!
//! A full screen of new text costs more than a frame on a phone. [`GlyphAtlas::warm`] rasterizes a
//! screen's glyphs and hands them over without drawing them, up to a budget a frame, for a fade or a
//! loading frame to spend, so the frame that shows the screen has nothing left to do.

use std::collections::HashMap;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

use etagere::{AllocId, BucketedAtlasAllocator, size2};
use sapphire_look::{UiDrawList, UiGlyphStats, UiPageState, UiPageUpload, UiQuad};
use swash::scale::image::Image;
use swash::scale::{Render, ScaleContext, Source};
use swash::zeno::{Format, Vector};
use swash::{CacheKey, FontRef};

use crate::shape::{ShapedGlyph, ShapedText};

/// Empty texels around each glyph, so a page's filter never reads a neighbour.
const PADDING: u32 = 1;
/// How much of a page one round of eviction frees.
const EVICT_SHARE: i32 = 8;

/// How the atlas is laid out.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AtlasConfig {
    /// A page's side in texels.
    pub page_size: u32,
    /// The most pages it makes, one R8 texture each.
    pub max_pages: u16,
    /// How many positions within a pixel a glyph can land on across a line.
    pub subpixel_bins: u8,
    /// How many frames a glyph is safe for after it's drawn.
    pub keep_frames: u32,
}

impl Default for AtlasConfig {
    fn default() -> Self {
        Self { page_size: 2048, max_pages: 4, subpixel_bins: 4, keep_frames: 2 }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
struct GlyphKey {
    /// The font's blob and its index in it.
    font: (u64, u32),
    coords: u64,
    glyph: u32,
    /// The size in 1/64 pixels.
    ppem: u32,
    subpixel: u8,
}

#[derive(Clone, Copy, Debug)]
struct Entry {
    /// `None` for a glyph with no ink.
    place: Option<Place>,
    last_used: u64,
}

#[derive(Clone, Copy, Debug)]
struct Place {
    page: u16,
    alloc: AllocId,
    /// The ink's corner in the page, past the padding.
    x: u16,
    y: u16,
    width: u16,
    height: u16,
    /// From the glyph's snapped origin to the ink's corner, in pixels, +y down.
    left: i16,
    top: i16,
}

/// See the module docs.
pub struct GlyphAtlas {
    config: AtlasConfig,
    pages: Vec<BucketedAtlasAllocator>,
    glyphs: HashMap<GlyphKey, Entry>,
    /// A swash cache key for each font, which it must keep to reuse what it read from the font.
    keys: HashMap<(u64, u32), (CacheKey, u32)>,
    scaler: ScaleContext,
    image: Image,
    /// Scratch for eviction's sort.
    oldest: Vec<(u64, GlyphKey)>,
    /// Nothing more fits this frame.
    full: bool,
    frame: u64,
    /// Whose pages this has been filling.
    store: u64,
    /// The last batch of uploads handed over.
    sent: u64,
    /// Whether this frame has opened a batch.
    sending: bool,
    stats: UiGlyphStats,
}

impl std::fmt::Debug for GlyphAtlas {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GlyphAtlas")
            .field("config", &self.config)
            .field("stats", &self.stats)
            .finish_non_exhaustive()
    }
}

impl GlyphAtlas {
    pub fn new(config: AtlasConfig) -> Self {
        Self {
            config,
            pages: Vec::new(),
            glyphs: HashMap::new(),
            keys: HashMap::new(),
            scaler: ScaleContext::new(),
            image: Image::new(),
            oldest: Vec::new(),
            full: false,
            frame: 0,
            store: 0,
            sent: 0,
            sending: false,
            stats: UiGlyphStats::default(),
        }
    }

    pub fn config(&self) -> &AtlasConfig {
        &self.config
    }

    /// Moves to the next frame, given what the renderer's pages hold now. If what this handed over
    /// isn't there, it starts over: see the module docs.
    pub fn begin_frame(&mut self, pages: UiPageState) {
        self.frame += 1;
        self.full = false;
        self.sending = false;
        self.stats = UiGlyphStats::default();
        if pages.store != self.store || pages.batch < self.sent {
            self.stats.forgot = !self.glyphs.is_empty();
            self.pages.clear();
            self.glyphs.clear();
        }
        self.store = pages.store;
    }

    /// What it did this frame, and how full it is.
    pub fn stats(&self) -> UiGlyphStats {
        UiGlyphStats {
            pages: self.pages.len() as u32,
            glyphs: self.glyphs.len() as u32,
            used_bytes: self.pages.iter().map(|page| page.allocated_space() as u64).sum(),
            ..self.stats
        }
    }

    /// Rasterizes what `text` will need and adds it to `list`'s uploads without drawing it, spending
    /// `budget`, which is glyphs rasterized and can be shared across texts. Whether all of `text` is
    /// in the atlas now: if not, call again next frame. `text` must have been shaped at the scale it
    /// will be drawn at.
    pub fn warm(&mut self, text: &ShapedText, budget: &mut u32, list: &mut UiDrawList) -> bool {
        let mut done = true;
        'runs: for run in &text.runs {
            let coords = coords_key(&run.coords);
            for glyph in &text.glyphs[run.glyphs.start as usize..run.glyphs.end as usize] {
                let (_, share, key) = self.key(run, coords, glyph, glyph.x);
                let known = self.glyphs.contains_key(&key);
                if !known && *budget == 0 {
                    done = false;
                    break 'runs;
                }
                let before = self.stats.rasterized;
                self.entry(key, run, share, list);
                *budget -= (self.stats.rasterized - before).min(*budget);
                done &= !self.full;
            }
        }
        self.publish(list);
        done
    }

    /// Where a glyph at `x` pixels lands: its whole pixel, its share of one, and its key.
    fn key(&self, run: &crate::ShapedRun, coords: u64, glyph: &ShapedGlyph, x: f32) -> (f32, f32, GlyphKey) {
        let bins = f32::from(self.config.subpixel_bins.max(1));
        let snapped = (x * bins).round() / bins;
        let (whole, share) = (snapped.floor(), snapped - snapped.floor());
        let key = GlyphKey {
            font: (run.font.data.id(), run.font.index),
            coords,
            glyph: glyph.id,
            ppem: (run.ppem * 64.0) as u32,
            subpixel: (share * bins) as u8,
        };
        (whole, share, key)
    }

    /// Tells `list` what it now carries.
    fn publish(&self, list: &mut UiDrawList) {
        list.page_size = self.config.page_size;
        list.page_state = UiPageState { store: self.store, batch: self.sent };
        list.glyphs = self.stats();
    }

    /// Adds `text` to `list` with its top-left at `origin` points, `scale` pixels to a point, and any
    /// new glyphs' texels. `text` must have been shaped at that scale.
    pub fn draw(
        &mut self,
        text: &ShapedText,
        origin: [f32; 2],
        scale: f32,
        color: [u8; 4],
        list: &mut UiDrawList,
    ) {
        let side = self.config.page_size as f32;
        // The whole text snaps first, so every quad lands on whole pixels.
        let origin_px = [(origin[0] * scale).round(), (origin[1] * scale).round()];
        for run in &text.runs {
            let coords = coords_key(&run.coords);
            for glyph in &text.glyphs[run.glyphs.start as usize..run.glyphs.end as usize] {
                let (whole, share, key) = self.key(run, coords, glyph, origin_px[0] + glyph.x);
                let Some(place) = self.entry(key, run, share, list) else { continue };
                let (x0, y0) =
                    (whole + f32::from(place.left), (origin_px[1] + glyph.y).round() + f32::from(place.top));
                let (w, h) = (f32::from(place.width), f32::from(place.height));
                let uv = [
                    f32::from(place.x) / side,
                    f32::from(place.y) / side,
                    (f32::from(place.x) + w) / side,
                    (f32::from(place.y) + h) / side,
                ];
                let quad = UiQuad::glyph([x0 / scale, y0 / scale], [w / scale, h / scale], uv, color);
                list.push_page_quad(place.page, quad);
            }
        }
        self.publish(list);
    }

    /// Where `key` is, rasterizing it if it's new. `None` for a glyph with no ink or no room.
    fn entry(
        &mut self,
        key: GlyphKey,
        run: &crate::ShapedRun,
        share: f32,
        list: &mut UiDrawList,
    ) -> Option<Place> {
        let frame = self.frame;
        if let Some(entry) = self.glyphs.get_mut(&key) {
            entry.last_used = frame;
            return entry.place;
        }
        if self.full {
            self.stats.dropped += 1;
            return None;
        }
        let place = self.rasterize(key, run, share, list);
        if !self.full {
            self.glyphs.insert(key, Entry { place, last_used: frame });
        }
        place
    }

    fn rasterize(
        &mut self,
        key: GlyphKey,
        run: &crate::ShapedRun,
        share: f32,
        list: &mut UiDrawList,
    ) -> Option<Place> {
        let data = run.font.data.as_ref();
        let (cache_key, offset) = *self.keys.entry(key.font).or_insert_with(|| {
            let font = FontRef::from_index(data, run.font.index as usize).unwrap_or(FontRef {
                data,
                offset: 0,
                key: CacheKey::new(),
            });
            (font.key, font.offset)
        });
        let font = FontRef { data, offset, key: cache_key };
        let mut scaler = self
            .scaler
            .builder(font)
            .size(run.ppem)
            .hint(false)
            .normalized_coords(run.coords.iter().copied())
            .build();
        self.image.clear();
        let drawn = Render::new(&[Source::Outline])
            .format(Format::Alpha)
            .offset(Vector::new(share, 0.0))
            .render_into(&mut scaler, key.glyph as u16, &mut self.image);
        let placement = self.image.placement;
        if !drawn || placement.width == 0 || placement.height == 0 {
            return None;
        }
        self.stats.rasterized += 1;
        let (outer_w, outer_h) = (placement.width + 2 * PADDING, placement.height + 2 * PADDING);
        let Some((page, alloc, x, y)) = self.allocate(outer_w, outer_h) else {
            self.stats.dropped += 1;
            self.full = true;
            return None;
        };
        if !self.sending {
            self.sending = true;
            self.sent += 1;
        }
        let offset = list.upload_texels.len() as u32;
        write_padded(&mut list.upload_texels, &self.image.data, placement.width, placement.height);
        list.page_uploads.push(UiPageUpload {
            page,
            x,
            y,
            width: outer_w as u16,
            height: outer_h as u16,
            offset,
        });
        self.stats.upload_bytes += u64::from(outer_w * outer_h);
        Some(Place {
            page,
            alloc,
            x: x + PADDING as u16,
            y: y + PADDING as u16,
            width: placement.width as u16,
            height: placement.height as u16,
            left: placement.left as i16,
            top: -placement.top as i16,
        })
    }

    /// Room for `width × height` texels, making a page or evicting if it has to.
    fn allocate(&mut self, width: u32, height: u32) -> Option<(u16, AllocId, u16, u16)> {
        let side = self.config.page_size as i32;
        let size = size2(width as i32, height as i32);
        if size.width > side || size.height > side {
            return None;
        }
        // A freed slot may be the wrong shape for this glyph, so it can take more than one round.
        loop {
            for (index, page) in self.pages.iter_mut().enumerate() {
                if let Some(allocation) = page.allocate(size) {
                    let corner = allocation.rectangle.min;
                    return Some((index as u16, allocation.id, corner.x as u16, corner.y as u16));
                }
            }
            if usize::from(self.config.max_pages) > self.pages.len() {
                let mut page = BucketedAtlasAllocator::new(size2(side, side));
                let allocation = page.allocate(size)?;
                self.pages.push(page);
                let corner = allocation.rectangle.min;
                return Some((self.pages.len() as u16 - 1, allocation.id, corner.x as u16, corner.y as u16));
            }
            if !self.evict() {
                return None;
            }
        }
    }

    /// Frees the least recently drawn glyphs, up to a share of a page. Whether it freed any.
    fn evict(&mut self) -> bool {
        let safe_from = self.frame.saturating_sub(u64::from(self.config.keep_frames));
        self.oldest.clear();
        self.oldest.extend(
            self.glyphs
                .iter()
                .filter(|(_, entry)| entry.last_used < safe_from && entry.place.is_some())
                .map(|(key, entry)| (entry.last_used, *key)),
        );
        // Keys break ties, so which glyphs go doesn't hang on the map's order.
        self.oldest.sort_unstable_by_key(|&(used, key)| {
            (used, key.font, key.coords, key.glyph, key.ppem, key.subpixel)
        });
        let side = self.config.page_size as i32;
        let mut left = side * side / EVICT_SHARE;
        let mut freed = false;
        for &(_, key) in &self.oldest {
            if left <= 0 {
                break;
            }
            let Some(Entry { place: Some(place), .. }) = self.glyphs.remove(&key) else { continue };
            self.pages[usize::from(place.page)].deallocate(place.alloc);
            left -= (i32::from(place.width) + 2) * (i32::from(place.height) + 2);
            self.stats.evicted += 1;
            freed = true;
        }
        freed
    }
}

/// `coords` as a key: a font's axes seldom number more than a few, and most runs have one set.
fn coords_key(coords: &[i16]) -> u64 {
    let mut hasher = DefaultHasher::new();
    coords.hash(&mut hasher);
    hasher.finish()
}

/// Appends `ink` with a zero border, so a rectangle reused from a bigger glyph holds nothing old.
fn write_padded(out: &mut Vec<u8>, ink: &[u8], width: u32, height: u32) {
    let (width, height, padding) = (width as usize, height as usize, PADDING as usize);
    let outer = width + 2 * padding;
    out.resize(out.len() + outer * padding, 0);
    for row in ink.chunks_exact(width).take(height) {
        out.resize(out.len() + padding, 0);
        out.extend_from_slice(row);
        out.resize(out.len() + padding, 0);
    }
    out.resize(out.len() + outer * padding, 0);
}
