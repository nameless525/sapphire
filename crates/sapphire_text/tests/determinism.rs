//! Does shaping and rasterizing give the same numbers on every OS and CPU? Goldens with text depend
//! on it. Each test writes one `text-digest` line straight to stderr, past the test harness's capture,
//! so a passing CI job shows it. Compare the lines across jobs: they must be equal.
//!
//! Ubuntu Light and Hack come from `epaint_default_fonts`, so they run everywhere. The Japanese line
//! needs `SAPPHIRE_TEST_FONTS` (`tests/common`).

mod common;

use std::io::Write as _;
use std::sync::Arc;

use common::{
    Digest, FAMILY_BODY, FAMILY_DISPLAY_JA, FAMILY_DISPLAY_LATIN, distinct_japanese, menu_screen, style,
};
use sapphire_look::UiDrawList;
use sapphire_text::{AtlasConfig, GlyphAtlas, ShapedText, TextStyle, TextSystem};

const LATIN: &str = "The quick brown fox, AVATAR To Ta 1,284 ffi fl: jumps over the lazy dog.";

fn report(name: &str, positions: Digest, texels: Digest, glyphs: usize, texel_bytes: usize) {
    let line = format!(
        "text-digest {name} arch={} os={} glyphs={glyphs} positions={:016x} texel_bytes={texel_bytes} texels={:016x}\n",
        std::env::consts::ARCH,
        std::env::consts::OS,
        positions.0,
        texels.0,
    );
    // Not `eprint!`: the harness captures that from a test that passes.
    std::io::stderr().write_all(line.as_bytes()).expect("stderr");
}

fn digest_shape(digest: &mut Digest, shaped: &ShapedText) {
    digest.f32(shaped.width);
    digest.f32(shaped.height);
    for baseline in &shaped.baselines {
        digest.f32(*baseline);
    }
    for line in &shaped.lines {
        digest.u32(line.start);
        digest.u32(line.end);
    }
    for run in &shaped.runs {
        digest.f32(run.ppem);
        for coord in &run.coords {
            digest.bytes(&coord.to_le_bytes());
        }
        digest.u32(run.glyphs.start);
        digest.u32(run.glyphs.end);
    }
    for glyph in &shaped.glyphs {
        digest.u32(glyph.id);
        digest.f32(glyph.x);
        digest.f32(glyph.y);
    }
}

struct Run {
    positions: Digest,
    texels: Digest,
    glyphs: usize,
    texel_bytes: usize,
    text: TextSystem,
    atlas: GlyphAtlas,
    list: UiDrawList,
}

impl Run {
    fn new(text: TextSystem) -> Self {
        Self {
            positions: Digest::default(),
            texels: Digest::default(),
            glyphs: 0,
            texel_bytes: 0,
            text,
            atlas: GlyphAtlas::new(AtlasConfig { page_size: 2048, max_pages: 8, ..AtlasConfig::default() }),
            list: UiDrawList::default(),
        }
    }

    fn add(&mut self, string: &str, style: &TextStyle, scale: f32, width: Option<f32>) {
        let shaped = self.text.shape(string, style, scale, width);
        digest_shape(&mut self.positions, shaped);
        self.glyphs += shaped.glyphs.len();
        self.list.reset();
        self.atlas.draw(shaped, [3.0, 5.0], scale, [255; 4], &mut self.list);
        // Where glyphs land in a page is the allocator's business, but it's deterministic too, so
        // the digest takes the rectangles as well as the bytes.
        for upload in &self.list.page_uploads {
            self.texels.u32(u32::from(upload.x) << 16 | u32::from(upload.y));
            self.texels.u32(u32::from(upload.width) << 16 | u32::from(upload.height));
        }
        self.texels.bytes(&self.list.upload_texels);
        self.texel_bytes += self.list.upload_texels.len();
        for quad in &self.list.quads {
            for value in quad.origin.iter().chain(&quad.axis_x).chain(&quad.axis_y).chain(&quad.uv) {
                self.positions.f32(*value);
            }
        }
    }
}

#[test]
fn digest_of_latin_from_a_font_every_checkout_has() {
    let mut text = TextSystem::new();
    text.register_font(Arc::new(epaint_default_fonts::UBUNTU_LIGHT.to_vec()));
    text.register_font(Arc::new(epaint_default_fonts::HACK_REGULAR.to_vec()));
    let mut run = Run::new(text);
    for scale in [1.0, 1.5, 2.0, 3.0] {
        for (family, size) in [("Ubuntu", 14.0), ("Ubuntu", 21.5), ("Hack", 12.0)] {
            let style =
                TextStyle { families: family, size, weight: 400.0, line_height: 1.3, locale: Some("en") };
            run.add(LATIN, &style, scale, Some(180.0));
        }
    }
    report("embedded-latin", run.positions, run.texels, run.glyphs, run.texel_bytes);
    assert!(run.glyphs > 500 && run.texel_bytes > 10_000);
}

#[test]
fn digest_of_japanese_and_variable_fonts() {
    let Some(dir) = common::fonts_dir("digest_of_japanese_and_variable_fonts") else { return };
    let mut run = Run::new(common::system(&dir));
    for scale in [1.0, 1.5, 3.0] {
        for japanese in [true, false] {
            for item in menu_screen(japanese) {
                run.add(item.text, &item.style, scale, item.max_width);
            }
        }
        let hard = distinct_japanese(&common::read_font(&dir, common::MPLUS1), 300);
        for (family, weight) in [(FAMILY_BODY, 400.0), (FAMILY_BODY, 700.0), (FAMILY_DISPLAY_JA, 400.0)] {
            run.add(&hard, &style(family, 16.0, weight, "ja"), scale, Some(320.0));
        }
        run.add("Level 1,284 必殺技", &style(FAMILY_DISPLAY_LATIN, 24.0, 650.0, "ja"), scale, None);
    }
    report("japanese-variable", run.positions, run.texels, run.glyphs, run.texel_bytes);
    assert!(run.glyphs > 5_000);
}
