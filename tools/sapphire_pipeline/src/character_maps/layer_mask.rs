//! Layer masks (M1 C7): separate masks packed into the four channels of an anime character
//! body's layer mask.
//!
//! A body's layers are painted by one linear mask in its surface mask slot
//! (`sapphire_assets::character_maps::layer_mask`): R where the matcap shows, G metallic, B
//! emission and A the stockings' density, each scaling its region's value. A `.layers.ron`
//! document names an input per channel, as an ILM document does ([`super::ilm`]): one channel of
//! an image mapped onto a range, or a constant. An image's channel may be sRGB-encoded
//! (`encoding: Srgb`): a color texture's brightness, decoded to linear light, as a VRM's emissive
//! texture holds its glow. **A channel left out is 1**, what an unbound mask reads, so a document
//! names only what it paints. The map cooks as a four-channel mask (BC7 on desktop, ASTC on
//! mobile).
//!
//! `extract-materials` writes them for VRM materials whose emission or matcap a texture paints
//! (`import::vrm::mtoon`'s layer mask, M1 A6's MToon mapping).

use bevy_reflect::TypePath;
use sapphire_assets::texture::MAX_TEXTURE_DIMENSION;
use sapphire_reflect::Versioned;
use serde::{Deserialize, Serialize};

use super::ilm::{ChannelInput, InputImages};
use super::{GeneratedMap, Inputs, MapError, with_image_importer};
use crate::cache::Stage;
use crate::import::image::{ChannelLayout, SourceImage};
use crate::texture::{MaskSettings, TextureClass};

/// This importer, as cache keys know it: reading the document and its images, and packing
/// them. Bump the version whenever the packed map changes for the same inputs.
pub const IMPORTER: Stage = Stage { name: "layers", version: with_image_importer(1) };

/// A `.layers.ron` document: the input of each layer mask channel.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, TypePath)]
#[type_path = "sapphire::pipeline"]
#[serde(deny_unknown_fields)]
pub struct LayerMaskMap {
    /// The map's size in texels, `(width, height)`. Required when every channel is a constant;
    /// otherwise the inputs' size, which this must match if given.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size: Option<[u32; 2]>,
    /// R: where the regions' matcap shows, and how strongly.
    #[serde(default = "unpainted")]
    pub matcap: ChannelInput,
    /// G: scales the regions' metallic, which reflects above 0.9.
    #[serde(default = "unpainted")]
    pub metallic: ChannelInput,
    /// B: where the regions glow, and how strongly.
    #[serde(default = "unpainted")]
    pub emission: ChannelInput,
    /// A: the density of the regions' stockings.
    #[serde(default = "unpainted")]
    pub stocking: ChannelInput,
}

impl Versioned for LayerMaskMap {
    const VERSION: u32 = 1;
}

/// A channel nothing paints: 1, as an unbound mask reads.
pub fn unpainted() -> ChannelInput {
    ChannelInput::Constant(1.0)
}

/// Errors in a layer mask document.
#[derive(Debug, thiserror::Error)]
pub enum LayerMaskError {
    #[error("every channel is a constant, so the document must give the map's `size`")]
    NoSize,
    #[error("size {}x{} must be 1 to {MAX_TEXTURE_DIMENSION} texels on each side", size[0], size[1])]
    Size { size: [u32; 2] },
    #[error("{channel}: {value} is outside 0 to 1")]
    Value { channel: &'static str, value: f32 },
}

impl LayerMaskMap {
    /// The inputs with their field names, in channel order: R, G, B, A
    /// (`layer_mask::MATCAP` to `layer_mask::STOCKING`).
    fn channels(&self) -> [(&'static str, &ChannelInput); 4] {
        [
            ("matcap", &self.matcap),
            ("metallic", &self.metallic),
            ("emission", &self.emission),
            ("stocking", &self.stocking),
        ]
    }

    /// Every image path, in channel order. Repeats are kept.
    pub fn inputs(&self) -> Vec<String> {
        self.channels().into_iter().filter_map(|(_, input)| input.image()).map(|(path, _)| path).collect()
    }

    /// Checks the size and every constant and range.
    pub fn validate(&self) -> Result<(), LayerMaskError> {
        if let Some(size) = self.size
            && !size.iter().all(|side| (1..=MAX_TEXTURE_DIMENSION).contains(side))
        {
            return Err(LayerMaskError::Size { size });
        }
        self.channels().into_iter().try_for_each(|(channel, input)| {
            input.check_unit(|value| LayerMaskError::Value { channel, value })
        })
    }
}

/// Reads the images `map` names and packs them.
pub(super) fn generate(
    map: &LayerMaskMap,
    settings: &MaskSettings,
    inputs: &Inputs<'_>,
) -> Result<GeneratedMap, MapError> {
    map.validate()?;
    let channels = map.channels();
    let wanted = channels.iter().filter_map(|(_, input)| input.image());
    let (size, images) = InputImages::read(inputs, wanted, map.size)?;
    let [width, height] = size.ok_or(LayerMaskError::NoSize)?;
    let mut pixels = vec![[0.0; 4]; width as usize * height as usize];
    for (c, (_, input)) in channels.into_iter().enumerate() {
        input.fill(&mut pixels, c, &images);
    }
    let image = SourceImage::new(width, height, ChannelLayout::Rgba, pixels).expect("one texel per pixel");
    Ok(GeneratedMap { image, class: TextureClass::Mask(*settings) })
}

#[cfg(test)]
mod tests {
    use sapphire_assets::character_maps::layer_mask;

    use super::*;
    use crate::character_maps::ilm::{Encoding, ImageChannel};

    #[test]
    fn channels_are_in_the_layer_masks_order() {
        let map = LayerMaskMap {
            size: None,
            matcap: unpainted(),
            metallic: unpainted(),
            emission: unpainted(),
            stocking: unpainted(),
        };
        let order = [layer_mask::MATCAP, layer_mask::METALLIC, layer_mask::EMISSION, layer_mask::STOCKING];
        assert_eq!(order, [0, 1, 2, 3]);
        assert_eq!(map.channels().map(|(name, _)| name), ["matcap", "metallic", "emission", "stocking"]);
    }

    #[test]
    fn documents_validate_their_inputs() {
        let base = || LayerMaskMap {
            size: Some([4, 4]),
            matcap: unpainted(),
            metallic: unpainted(),
            emission: unpainted(),
            stocking: unpainted(),
        };
        assert!(base().validate().is_ok());
        let error = |edit: fn(&mut LayerMaskMap)| {
            let mut map = base();
            edit(&mut map);
            map.validate().unwrap_err().to_string()
        };
        assert_eq!(
            error(|m| m.size = Some([4, 0])),
            format!("size 4x0 must be 1 to {MAX_TEXTURE_DIMENSION} texels on each side")
        );
        assert_eq!(error(|m| m.stocking = ChannelInput::Constant(-0.5)), "stocking: -0.5 is outside 0 to 1");
        assert_eq!(
            error(|m| m.emission = ChannelInput::Image {
                path: "glow.png".into(),
                channel: ImageChannel::G,
                range: [0.0, 2.0],
                encoding: Encoding::Srgb,
            }),
            "emission: 2 is outside 0 to 1"
        );
    }
}
