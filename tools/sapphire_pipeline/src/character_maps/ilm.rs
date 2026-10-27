//! ILM maps: separate grayscale masks packed into the ILM texture's four channels.
//!
//! The anime character shader reads four painted masks from one texture (spec §5.12; the
//! channel assignment is `sapphire_assets::character_maps`): the specular mask in R, the
//! shadow bias in G, the specular threshold in B and the material ID in A. Artists paint or
//! bake them as separate images; an `.ilm.ron` document names an input per channel and the
//! packer writes the texture:
//!
//! - **A channel's input** is one channel of an image (`R` by default, which is the gray of
//!   a grayscale image), mapped from `[0, 1]` onto a `range` (by default `[0, 1]`; `[1, 0]`
//!   inverts, `[0, 0.5]` turns an AO bake into a shadow bias that only darkens), or a
//!   constant. An image's channel is linear data unless the input says it is sRGB-encoded
//!   ([`Encoding`]), as a color texture's are. Layer masks ([`super::layer_mask`]) take the
//!   same inputs.
//! - **A channel left out** takes its neutral value: no specular, a shadow bias of 0.5 (no
//!   shift), a specular threshold of 0.5, material ID 0.
//! - **Material IDs** are constants, or gray levels painted in an image: each texel takes the
//!   ID of the nearest listed level (the first is ID 0), by default the eight even levels the
//!   map stores (0, 1/7, … 1). Snapping removes painting and anti-aliasing noise, so IDs are
//!   flat regions that block compression keeps exact.
//!
//! One image may feed several channels (a bake with AO in R and a specular mask in G) and is
//! read once. Every image input must have the map's size; a document whose channels are all
//! constant gives the size itself.

use std::fmt;

use bevy_reflect::TypePath;
use sapphire_assets::character_maps::{MATERIAL_IDS, MaterialId, ilm};
use sapphire_assets::texture::MAX_TEXTURE_DIMENSION;
use sapphire_reflect::Versioned;
use serde::{Deserialize, Serialize};

use super::{GeneratedMap, Inputs, MapError, check_size, with_image_importer};
use crate::cache::Stage;
use crate::import::image::{ChannelLayout, SourceImage, Transfer};
use crate::texture::{IlmSettings, TextureClass};

/// This importer, as cache keys know it: reading the document and its masks, and packing
/// them. Bump the version whenever the packed map changes for the same inputs.
pub const IMPORTER: Stage = Stage { name: "ilm", version: with_image_importer(1) };

/// An `.ilm.ron` document: the input of each ILM channel.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, TypePath)]
#[type_path = "sapphire::pipeline"]
#[serde(deny_unknown_fields)]
pub struct IlmMap {
    /// The map's size in texels, `(width, height)`. Required when every channel is a constant;
    /// otherwise the inputs' size, which this must match if given.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size: Option<[u32; 2]>,
    /// R: where highlights may appear, and how strongly.
    #[serde(default = "neutral_specular_mask")]
    pub specular_mask: ChannelInput,
    /// G: shifts the two-tone light split; 0.5 is neutral.
    #[serde(default = "neutral_shadow_bias")]
    pub shadow_bias: ChannelInput,
    /// B: the highlight's size.
    #[serde(default = "neutral_specular_threshold")]
    pub specular_threshold: ChannelInput,
    /// A: picks the ramp rows and the per-ID parameters.
    #[serde(default)]
    pub material_id: MaterialIdInput,
}

impl Versioned for IlmMap {
    const VERSION: u32 = 1;
}

fn neutral_specular_mask() -> ChannelInput {
    ChannelInput::Constant(ilm::NEUTRAL[ilm::SPECULAR_MASK])
}

fn neutral_shadow_bias() -> ChannelInput {
    ChannelInput::Constant(ilm::NEUTRAL[ilm::SHADOW_BIAS])
}

fn neutral_specular_threshold() -> ChannelInput {
    ChannelInput::Constant(ilm::NEUTRAL[ilm::SPECULAR_THRESHOLD])
}

/// Where one of the R, G and B channels comes from (and each of a layer mask's four).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub enum ChannelInput {
    /// Every texel holds this value, 0 to 1.
    Constant(f32),
    /// One channel of an image, decoded as `encoding` says, mapped from `[0, 1]` onto `range`.
    Image {
        /// Relative to the document.
        path: String,
        #[serde(default)]
        channel: ImageChannel,
        #[serde(default = "full_range", skip_serializing_if = "is_full_range")]
        range: [f32; 2],
        #[serde(default, skip_serializing_if = "Encoding::is_linear")]
        encoding: Encoding,
    },
}

impl ChannelInput {
    /// The image it reads, with its encoding, if any.
    pub(super) fn image(&self) -> Option<(String, Encoding)> {
        match self {
            Self::Image { path, encoding, .. } => Some((path.clone(), *encoding)),
            Self::Constant(_) => None,
        }
    }

    /// Fails with `error` unless the constant, or both ends of the range, are within 0 to 1.
    pub(super) fn check_unit<E>(&self, error: impl Fn(f32) -> E) -> Result<(), E> {
        let unit = |value: f32| if (0.0..=1.0).contains(&value) { Ok(()) } else { Err(error(value)) };
        match self {
            Self::Constant(value) => unit(*value),
            Self::Image { range, .. } => range.iter().try_for_each(|&value| unit(value)),
        }
    }

    /// Writes this input into channel `c` of `pixels`, reading its image from `images`.
    pub(super) fn fill(&self, pixels: &mut [[f32; 4]], c: usize, images: &InputImages) {
        match self {
            Self::Constant(value) => pixels.iter_mut().for_each(|pixel| pixel[c] = *value),
            Self::Image { path, channel, range: [low, high], encoding } => {
                for (pixel, source) in pixels.iter_mut().zip(&images.get(path, *encoding).pixels) {
                    pixel[c] = (low + (high - low) * source[channel.index()]).clamp(0.0, 1.0);
                }
            }
        }
    }
}

/// How an input image's samples encode a channel.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Encoding {
    /// Linear data, taken as it is: a painted mask.
    #[default]
    Linear,
    /// sRGB: a color texture, its R, G and B decoded to linear light first, as a VRM's emissive
    /// texture holds its glow (M1 C7's layer masks). Alpha and float images are linear whatever
    /// this says, as the image importer reads them.
    Srgb,
}

impl Encoding {
    fn is_linear(&self) -> bool {
        *self == Self::Linear
    }

    fn transfer(self) -> Transfer {
        match self {
            Self::Linear => Transfer::Linear,
            Self::Srgb => Transfer::Srgb,
        }
    }
}

/// The images a document's inputs read, each decoded once for each encoding it is read in.
pub(super) struct InputImages(Vec<((String, Encoding), SourceImage)>);

impl InputImages {
    /// Reads every image `wanted` names, checking each has the map's size: `size` if the
    /// document gives one, else the first image's. Returns that size too, `None` when the
    /// document neither names an image nor gives one.
    pub(super) fn read(
        inputs: &Inputs<'_>,
        wanted: impl IntoIterator<Item = (String, Encoding)>,
        mut size: Option<[u32; 2]>,
    ) -> Result<(Option<[u32; 2]>, Self), MapError> {
        let mut images: Vec<((String, Encoding), SourceImage)> = Vec::new();
        for (input, encoding) in wanted {
            if images.iter().any(|((path, read), _)| *path == input && *read == encoding) {
                continue;
            }
            let (path, image) = inputs.image_as(&input, encoding.transfer())?;
            let expected = *size.get_or_insert([image.width, image.height]);
            check_size(&path, &image, expected)?;
            images.push(((input, encoding), image));
        }
        Ok((size, Self(images)))
    }

    /// The image read for `path` in `encoding`, which [`Self::read`] was given.
    fn get(&self, path: &str, encoding: Encoding) -> &SourceImage {
        let found = self.0.iter().find(|((input, read), _)| input == path && *read == encoding);
        &found.expect("every input was read").1
    }
}

fn full_range() -> [f32; 2] {
    [0.0, 1.0]
}

fn is_full_range(range: &[f32; 2]) -> bool {
    *range == full_range()
}

/// Where the material IDs come from.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub enum MaterialIdInput {
    /// Every texel has this ID, below [`MATERIAL_IDS`].
    Constant(u8),
    /// IDs painted as gray levels in one channel of an image: each texel takes the ID of the
    /// nearest of `levels`, the first being ID 0. Left empty, the levels are the ones the map
    /// stores, `k / 7`.
    Image {
        /// Relative to the document.
        path: String,
        #[serde(default)]
        channel: ImageChannel,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        levels: Vec<f32>,
    },
}

impl Default for MaterialIdInput {
    fn default() -> Self {
        Self::Constant(0)
    }
}

/// A channel of an input image. A grayscale image's gray is `R` (and `G` and `B`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum ImageChannel {
    #[default]
    R,
    G,
    B,
    A,
}

impl ImageChannel {
    fn index(self) -> usize {
        match self {
            Self::R => 0,
            Self::G => 1,
            Self::B => 2,
            Self::A => 3,
        }
    }
}

/// An ILM channel, by its field name in the document.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct IlmChannel(&'static str);

impl fmt::Display for IlmChannel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.0)
    }
}

/// Errors in an ILM document.
#[derive(Debug, thiserror::Error)]
pub enum IlmError {
    #[error("every channel is a constant, so the document must give the map's `size`")]
    NoSize,
    #[error("size {}x{} must be 1 to {MAX_TEXTURE_DIMENSION} texels on each side", size[0], size[1])]
    Size { size: [u32; 2] },
    #[error("{channel}: {value} is outside 0 to 1")]
    Value { channel: IlmChannel, value: f32 },
    #[error("material_id: ID {0} isn't below {MATERIAL_IDS}")]
    MaterialId(u8),
    #[error("material_id: {0} levels; the map tells at most {MATERIAL_IDS} IDs apart")]
    TooManyLevels(usize),
}

impl IlmMap {
    /// The R, G and B inputs, with their channel names.
    fn color_channels(&self) -> [(IlmChannel, &ChannelInput); 3] {
        [
            (IlmChannel("specular_mask"), &self.specular_mask),
            (IlmChannel("shadow_bias"), &self.shadow_bias),
            (IlmChannel("specular_threshold"), &self.specular_threshold),
        ]
    }

    /// Every image path, in channel order. Repeats are kept.
    pub fn inputs(&self) -> Vec<String> {
        self.images().into_iter().map(|(path, _)| path).collect()
    }

    /// Every image path with the encoding it is read in, in channel order: the material IDs
    /// are linear.
    fn images(&self) -> Vec<(String, Encoding)> {
        let colors = self.color_channels().into_iter().filter_map(|(_, input)| input.image());
        let ids = match &self.material_id {
            MaterialIdInput::Image { path, .. } => Some((path.clone(), Encoding::Linear)),
            MaterialIdInput::Constant(_) => None,
        };
        colors.chain(ids).collect()
    }

    /// Checks sizes, constants, ranges and levels.
    pub fn validate(&self) -> Result<(), IlmError> {
        if let Some(size) = self.size
            && !size.iter().all(|side| (1..=MAX_TEXTURE_DIMENSION).contains(side))
        {
            return Err(IlmError::Size { size });
        }
        let unit = |channel, value: f32| {
            if (0.0..=1.0).contains(&value) { Ok(()) } else { Err(IlmError::Value { channel, value }) }
        };
        for (channel, input) in self.color_channels() {
            input.check_unit(|value| IlmError::Value { channel, value })?;
        }
        match &self.material_id {
            MaterialIdInput::Constant(id) if MaterialId::new(*id).is_none() => Err(IlmError::MaterialId(*id)),
            MaterialIdInput::Image { levels, .. } if levels.len() > usize::from(MATERIAL_IDS) => {
                Err(IlmError::TooManyLevels(levels.len()))
            }
            MaterialIdInput::Image { levels, .. } => {
                levels.iter().try_for_each(|&level| unit(IlmChannel("material_id"), level))
            }
            MaterialIdInput::Constant(_) => Ok(()),
        }
    }
}

/// Reads the images `map` names and packs them.
pub(super) fn generate(
    map: &IlmMap,
    settings: &IlmSettings,
    inputs: &Inputs<'_>,
) -> Result<GeneratedMap, MapError> {
    map.validate()?;
    let (size, images) = InputImages::read(inputs, map.images(), map.size)?;
    let [width, height] = size.ok_or(IlmError::NoSize)?;
    let texels = width as usize * height as usize;

    let mut pixels = vec![[0.0; 4]; texels];
    for (c, (_, input)) in map.color_channels().into_iter().enumerate() {
        input.fill(&mut pixels, c, &images);
    }
    let id = ilm::MATERIAL_ID;
    match &map.material_id {
        MaterialIdInput::Constant(value) => {
            let value = ilm::encode_material_id(MaterialId::new(*value).expect("validated"));
            pixels.iter_mut().for_each(|pixel| pixel[id] = value);
        }
        MaterialIdInput::Image { path, channel, levels } => {
            for (pixel, source) in pixels.iter_mut().zip(&images.get(path, Encoding::Linear).pixels) {
                pixel[id] = ilm::encode_material_id(nearest_id(source[channel.index()], levels));
            }
        }
    }
    let image = SourceImage::new(width, height, ChannelLayout::Rgba, pixels).expect("one texel per pixel");
    Ok(GeneratedMap { image, class: TextureClass::Ilm(*settings) })
}

/// The ID whose level is nearest `value`: the stored levels when `levels` is empty, otherwise
/// the first of the nearest listed ones.
fn nearest_id(value: f32, levels: &[f32]) -> MaterialId {
    if levels.is_empty() {
        return ilm::decode_material_id(value);
    }
    let mut best = 0;
    for (index, level) in levels.iter().enumerate() {
        if (value - level).abs() < (value - levels[best]).abs() {
            best = index;
        }
    }
    MaterialId::new(best as u8).expect("validated: at most MATERIAL_IDS levels")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_snap_to_the_nearest_level() {
        let id = |value, levels: &[f32]| nearest_id(value, levels).get();
        // The stored levels: k / 7.
        assert_eq!([0.0, 0.07, 0.08, 0.55, 0.93, 1.0].map(|v| id(v, &[])), [0, 0, 1, 4, 7, 7]);
        // Genshin-style levels for five IDs.
        let genshin = [0.0, 0.3, 0.5, 0.7, 1.0];
        assert_eq!([0.1, 0.2, 0.42, 0.61, 0.69, 0.9].map(|v| id(v, &genshin)), [0, 1, 2, 3, 3, 4]);
        // Ties go to the first level.
        assert_eq!(id(0.4, &genshin), 1);
    }

    #[test]
    fn documents_validate_their_inputs() {
        let base = || IlmMap {
            size: Some([4, 4]),
            specular_mask: neutral_specular_mask(),
            shadow_bias: neutral_shadow_bias(),
            specular_threshold: neutral_specular_threshold(),
            material_id: MaterialIdInput::default(),
        };
        assert!(base().validate().is_ok());
        let error = |edit: fn(&mut IlmMap)| {
            let mut map = base();
            edit(&mut map);
            map.validate().unwrap_err().to_string()
        };
        assert_eq!(
            error(|m| m.size = Some([0, 4])),
            format!("size 0x4 must be 1 to {MAX_TEXTURE_DIMENSION} texels on each side")
        );
        assert_eq!(
            error(|m| m.shadow_bias = ChannelInput::Constant(1.5)),
            "shadow_bias: 1.5 is outside 0 to 1"
        );
        assert_eq!(
            error(|m| m.specular_mask = ChannelInput::Image {
                path: "a.png".into(),
                channel: ImageChannel::R,
                range: [0.0, f32::NAN],
                encoding: Encoding::Linear,
            }),
            "specular_mask: NaN is outside 0 to 1"
        );
        assert_eq!(
            error(|m| m.material_id = MaterialIdInput::Constant(8)),
            "material_id: ID 8 isn't below 8"
        );
        assert!(
            error(|m| m.material_id = MaterialIdInput::Image {
                path: "ids.png".into(),
                channel: ImageChannel::R,
                levels: vec![0.0; 9],
            })
            .contains("9 levels")
        );
    }
}
