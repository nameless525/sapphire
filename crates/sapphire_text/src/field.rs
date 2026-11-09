//! Display faces drawn from a multi-channel signed distance field, cooked ahead (M3 U8): one texture
//! per face, every size from it, nothing rasterized at run time.
//!
//! # Which text takes it
//!
//! A glyph draws from its face's field when the face has one ([`FaceOptions::field`]), the run is at
//! the weight the field was made at, the field holds the glyph, and the run is at least
//! [`FieldFace::em`] pixels to the em. Everything else is the pages' coverage, as before.
//!
//! **Decision: by face and size, not by a style flag.** The size threshold is where a field is
//! magnified, which is where it's at its best and where coverage costs most: one 192-pixel glyph
//! (64 points at 3×) takes 37,000 texels of a page and a fresh rasterization for every size, and a
//! title's few glyphs take more of a page than a menu. Under it, coverage is the sharper of the two,
//! since a minified field thins and aliases with no mip to fall back on. Only the display faces carry
//! a field, so body text never changes path, and a screen's author never has to know: the same style
//! draws a 16-point caption from the pages and a 96-point title from the field, both sharp, and a
//! character outside the field's set (a name, a rare kanji) falls back to coverage glyph by glyph
//! in the same run. A flag would let an author put 14-point text on a field, where it reads worse.
//!
//! [`FaceOptions::field`]: crate::FaceOptions::field

use std::sync::Arc;

use sapphire_look::{UiAtlas, UiFieldFormat};

/// One glyph's cell in a [`FieldFace`]'s atlas.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FieldGlyph {
    pub id: u32,
    /// The cell, in texels from the atlas's top-left: `x, y, width, height`. Empty for a glyph with
    /// no ink, such as a space.
    pub cell: [u16; 4],
    /// The cell's top-left from the glyph's origin on the baseline, in ems, +y down.
    pub offset: [f32; 2],
}

/// A display face's field: an atlas of [`UiFieldFormat::Multi`] texels and where each glyph is in it.
#[derive(Debug)]
pub struct FieldFace {
    atlas: Arc<UiAtlas>,
    em: f32,
    coords: Vec<i16>,
    /// By `id`, ascending.
    glyphs: Vec<FieldGlyph>,
}

/// A field that can't be drawn from.
#[derive(Debug, PartialEq, thiserror::Error)]
pub enum FieldError {
    #[error("a display field's atlas must be multi-channel")]
    NotMulti,
    #[error("a display field's atlas holds {bytes} bytes, not the {expected} its size says")]
    Size { bytes: usize, expected: u64 },
    #[error("a display field made at {em} texels to the em")]
    Em { em: f32 },
    #[error("the display field's glyph {id} lies outside its atlas")]
    Outside { id: u32 },
    #[error("the display field's glyphs aren't in ascending order of id at {id}")]
    Order { id: u32 },
}

impl FieldFace {
    /// A field made at `em` texels to the em, at the variation `coords` (normalized, in the font's
    /// axis order, empty for a font with no axes), holding `glyphs`.
    pub fn new(
        atlas: Arc<UiAtlas>,
        em: f32,
        coords: Vec<i16>,
        glyphs: Vec<FieldGlyph>,
    ) -> Result<Self, FieldError> {
        if atlas.format != UiFieldFormat::Multi {
            return Err(FieldError::NotMulti);
        }
        let expected = u64::from(atlas.width) * u64::from(atlas.height) * 4;
        if atlas.texels.len() as u64 != expected {
            return Err(FieldError::Size { bytes: atlas.texels.len(), expected });
        }
        if !(em.is_finite() && em >= 1.0) {
            return Err(FieldError::Em { em });
        }
        for (at, glyph) in glyphs.iter().enumerate() {
            let [x, y, width, height] = glyph.cell.map(u32::from);
            if x + width > atlas.width
                || y + height > atlas.height
                || !glyph.offset.iter().all(|v| v.is_finite())
            {
                return Err(FieldError::Outside { id: glyph.id });
            }
            if at > 0 && glyphs[at - 1].id >= glyph.id {
                return Err(FieldError::Order { id: glyph.id });
            }
        }
        Ok(Self { atlas, em, coords, glyphs })
    }

    pub fn atlas(&self) -> &Arc<UiAtlas> {
        &self.atlas
    }

    /// Texels to the em it was made at, which is also the smallest size in pixels to the em it draws:
    /// see the [module docs](self).
    pub fn em(&self) -> f32 {
        self.em
    }

    /// Whether a run at `ppem` pixels to the em and at `coords` draws from it.
    pub(crate) fn draws(&self, ppem: f32, coords: &[i16]) -> bool {
        ppem >= self.em && same_instance(&self.coords, coords)
    }

    pub(crate) fn glyph(&self, id: u32) -> Option<&FieldGlyph> {
        self.glyphs.binary_search_by_key(&id, |glyph| glyph.id).ok().map(|at| &self.glyphs[at])
    }
}

/// Whether two sets of normalized coordinates name one instance: a missing axis is at its default.
fn same_instance(field: &[i16], run: &[i16]) -> bool {
    let at = |coords: &[i16], axis: usize| coords.get(axis).copied().unwrap_or(0);
    (0..field.len().max(run.len())).all(|axis| at(field, axis) == at(run, axis))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn atlas(width: u32, height: u32) -> Arc<UiAtlas> {
        let texels = vec![0; (width * height * 4) as usize];
        Arc::new(UiAtlas { width, height, spread: 4.0, format: UiFieldFormat::Multi, texels })
    }

    fn glyph(id: u32, cell: [u16; 4]) -> FieldGlyph {
        FieldGlyph { id, cell, offset: [0.0, -1.0] }
    }

    #[test]
    fn a_field_that_lies_about_itself_is_refused_by_what_s_wrong() {
        let single = Arc::new(UiAtlas { format: UiFieldFormat::Single, ..(*atlas(4, 4)).clone() });
        assert_eq!(FieldFace::new(single, 48.0, vec![], vec![]).unwrap_err(), FieldError::NotMulti);
        let short = Arc::new(UiAtlas { texels: vec![0; 3], ..(*atlas(4, 4)).clone() });
        assert_eq!(
            FieldFace::new(short, 48.0, vec![], vec![]).unwrap_err(),
            FieldError::Size { bytes: 3, expected: 64 }
        );
        assert_eq!(FieldFace::new(atlas(4, 4), 0.0, vec![], vec![]).unwrap_err(), FieldError::Em { em: 0.0 });
        let outside = vec![glyph(3, [2, 0, 3, 1])];
        assert_eq!(
            FieldFace::new(atlas(4, 4), 48.0, vec![], outside).unwrap_err(),
            FieldError::Outside { id: 3 }
        );
        let unordered = vec![glyph(3, [0, 0, 1, 1]), glyph(3, [1, 0, 1, 1])];
        assert_eq!(
            FieldFace::new(atlas(4, 4), 48.0, vec![], unordered).unwrap_err(),
            FieldError::Order { id: 3 }
        );
    }

    #[test]
    fn a_run_draws_from_it_only_at_its_size_or_more_and_at_its_instance() {
        let face = FieldFace::new(
            atlas(4, 4),
            48.0,
            vec![0x2000],
            vec![glyph(5, [0, 0, 2, 2]), glyph(9, [2, 2, 2, 2])],
        )
        .unwrap();
        assert!(face.draws(48.0, &[0x2000]) && face.draws(300.0, &[0x2000]));
        assert!(!face.draws(47.0, &[0x2000]), "under its em, coverage is sharper");
        assert!(!face.draws(96.0, &[0]) && !face.draws(96.0, &[]), "another weight is another outline");
        assert_eq!(face.glyph(9).map(|glyph| glyph.cell), Some([2, 2, 2, 2]));
        assert_eq!(face.glyph(6), None);
        let plain = FieldFace::new(atlas(4, 4), 48.0, vec![], vec![]).unwrap();
        assert!(
            plain.draws(48.0, &[]) && plain.draws(48.0, &[0, 0]),
            "the default instance, however it's written"
        );
    }
}
