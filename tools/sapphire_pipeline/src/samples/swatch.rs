//! The shader swatches' content (M1 C1): a ramp and a control (ILM) map, written as anime map
//! documents with generated masks so they cook through the same tools (A7's) as a character's
//! maps, and the three materials the sandbox's `--swatches` spheres draw with, as material
//! documents naming the maps by asset ID:
//!
//! - `swatch_ramp.material.ron`: ramp mode over the maps.
//! - `swatch_subsurface.material.ron`: the same with WuWa's subsurface mix at 35%, the tints
//!   mostly warm.
//! - `swatch_bands.material.ron`: three bands in per-region colors, no textures; the spheres'
//!   vertices give the regions.
//!
//! The spheres' UVs run around them in `u` and from the top pole down in `v`, so the maps are
//! laid out in latitude bands:
//!
//! | `v` | Material ID | Ramp (day; night) |
//! |---|---|---|
//! | 0–¼ | 3, hair | plum to orchid; violet |
//! | ¼–½ | 0, skin | rosewood to peach; dusk lilac |
//! | ½–¾ | 1, cloth | navy to periwinkle; ink blue |
//! | ¾–1 | 2, accent | three hard green steps; slate teal |
//!
//! The shadow bias alternates between neutral and 0.75 (N·L + 0.5, lit further into the shadow)
//! in 16 stripes around the sphere, so the terminator jogs at each stripe's edge. Two patches use
//! the locks: one in the cloth band, toward the sandbox's sun, is 0 (always in shadow, a painted
//! shadow), and one in the skin band, on the far side, is 1 (always lit, as eyes are).

use std::io;

use sapphire_assets::AssetId;
use sapphire_assets::character_maps::ilm;
use sapphire_assets::material::{
    AnimeCharacterMaterial, BandDiffuse, CharacterRegion, DiffuseMode, LightSplit, Outline, RampDiffuse,
};
use sapphire_serialize::to_ron;

use crate::character_maps::ilm::{ChannelInput, Encoding, ImageChannel, MaterialIdInput};
use crate::character_maps::ramp::{ColorStop, Interpolation, MaterialRamps, Ramp, SrgbColor};
use crate::character_maps::{IlmMap, RampMap};

/// File names, in the samples directory.
pub const RAMP_FILE_NAME: &str = "swatch.ramp.ron";
pub const ILM_FILE_NAME: &str = "swatch.ilm.ron";
pub const MATERIAL_ID_FILE_NAME: &str = "swatch_ids.png";
pub const SHADOW_BIAS_FILE_NAME: &str = "swatch_bias.png";
pub const RAMP_MATERIAL_FILE_NAME: &str = "swatch_ramp.material.ron";
pub const SUBSURFACE_MATERIAL_FILE_NAME: &str = "swatch_subsurface.material.ron";
pub const BAND_MATERIAL_FILE_NAME: &str = "swatch_bands.material.ron";

/// The maps' asset IDs, which their committed sidecars (`swatch.ramp.ron.meta`,
/// `swatch.ilm.ron.meta`) give them: material documents name textures by ID.
pub const RAMP_ID: AssetId = AssetId::from_text("1ace5ab9-aceb-42fa-bbcc-953141739d56");
pub const ILM_ID: AssetId = AssetId::from_text("e3b9cd39-a767-4d71-ac5e-9ce6b1f0d248");

/// The masks' size in texels.
const SIZE: u32 = 128;
/// Material IDs of the latitude bands, top to bottom.
const BANDS: [u8; 4] = [3, 0, 1, 2];
/// Stripes of shadow bias around the sphere.
const BIAS_STRIPES: u32 = 16;
/// The shadow bias offset of every other stripe: N·L + 0.5, lit further into the shadow side.
const BIAS_STRIPE_OFFSET: f32 = 0.5;
/// The patches at the locks, as `(u range, v range, bias)`.
const BIAS_PATCHES: [([f32; 2], [f32; 2], f32); 2] =
    [([0.24, 0.32], [0.55, 0.7], 0.0), ([0.04, 0.12], [0.3, 0.45], 1.0)];

/// The ramp document: day and night ramps for IDs 0 to 3; IDs past the last repeat it.
pub fn ramp_document() -> String {
    let ramp = |interpolation, stops: &[(f32, &str)]| Ramp {
        interpolation,
        stops: stops
            .iter()
            .map(|&(at, color)| ColorStop {
                at,
                color: color.parse::<SrgbColor>().expect("a hex color"),
                interpolation: None,
            })
            .collect(),
    };
    let material = |name: &str, day, night| MaterialRamps { name: name.to_owned(), day, night: Some(night) };
    let document = RampMap {
        interpolation: Interpolation::OkLab,
        materials: vec![
            material(
                "skin",
                ramp(None, &[(0.0, "#8a4a4f"), (0.55, "#c8796c"), (1.0, "#f2b8a0")]),
                ramp(None, &[(0.0, "#3d3554"), (1.0, "#8a7fa8")]),
            ),
            material(
                "cloth",
                ramp(None, &[(0.0, "#272a52"), (0.6, "#4f5aa0"), (1.0, "#9aa8e0")]),
                ramp(None, &[(0.0, "#14162e"), (1.0, "#4a5288")]),
            ),
            material(
                "accent",
                ramp(Some(Interpolation::Step), &[(0.0, "#3a6a5a"), (0.5, "#6aa88a"), (0.8, "#b8e0c0")]),
                ramp(None, &[(0.0, "#1f3a3a"), (1.0, "#5a8a80")]),
            ),
            material(
                "hair",
                ramp(None, &[(0.0, "#5a2a6a"), (1.0, "#d080c0")]),
                ramp(None, &[(0.0, "#2a1a40"), (1.0, "#7050a0")]),
            ),
        ],
    };
    document.validate().expect("the swatch ramps are valid");
    to_ron(&document).expect("a ramp document serializes")
}

/// The ILM document: material IDs and shadow bias from the two masks, the specular channels
/// neutral.
pub fn ilm_document() -> String {
    let document = IlmMap {
        size: None,
        specular_mask: ChannelInput::Constant(0.0),
        shadow_bias: ChannelInput::Image {
            path: SHADOW_BIAS_FILE_NAME.to_owned(),
            channel: ImageChannel::R,
            range: [0.0, 1.0],
            encoding: Encoding::Linear,
        },
        specular_threshold: ChannelInput::Constant(0.5),
        material_id: MaterialIdInput::Image {
            path: MATERIAL_ID_FILE_NAME.to_owned(),
            channel: ImageChannel::R,
            levels: Vec::new(),
        },
    };
    document.validate().expect("the swatch ILM map is valid");
    to_ron(&document).expect("an ILM document serializes")
}

/// What every swatch material shares: a pale albedo, a crisp split at N·L 0.45 (so the column,
/// lit from the side by the sandbox's sun, shows much of its shadow side), the default rim and a
/// thin outline in each band's color.
fn swatch_material(regions: [CharacterRegion; 4]) -> AnimeCharacterMaterial {
    let outlines = [[0.32, 0.12, 0.12], [0.06, 0.06, 0.18], [0.05, 0.16, 0.11], [0.22, 0.06, 0.24]];
    AnimeCharacterMaterial {
        base_color: [0.95, 0.93, 0.9, 1.0],
        light_split: LightSplit { threshold: 0.45, softness: 0.01 },
        regions: regions
            .into_iter()
            .zip(outlines)
            .map(|(region, outline_color)| CharacterRegion { outline_color, ..region })
            .collect(),
        outline: Outline { width: 3.5, ..Outline::default() },
        ..AnimeCharacterMaterial::default()
    }
}

/// Ramp mode over the swatch maps, the rows mixed `mix` into each region's subsurface tint.
fn ramp_material(mix: f32) -> AnimeCharacterMaterial {
    AnimeCharacterMaterial {
        control_map: Some(ILM_ID),
        diffuse: DiffuseMode::Ramp(RampDiffuse { map: RAMP_ID, mix }),
        ..swatch_material([CharacterRegion::default(); 4])
    }
}

/// The ramp-mode material document.
pub fn ramp_material_document() -> String {
    material_document(&ramp_material(1.0))
}

/// The ramp-mode material at WuWa's skin mix, with subsurface tints per region (warm on skin).
pub fn subsurface_material_document() -> String {
    let tints = [[0.94, 0.6, 0.41], [0.62, 0.66, 0.95], [0.62, 0.88, 0.72], [0.9, 0.6, 0.85]];
    let mut material = ramp_material(0.35);
    for (region, subsurface_color) in material.regions.iter_mut().zip(tints) {
        region.subsurface_color = subsurface_color;
    }
    material_document(&material)
}

/// The band-mode material document: three bands in per-region colors.
pub fn band_material_document() -> String {
    let region = |shade_color, deep_shade_color| CharacterRegion {
        shade_color,
        deep_shade_color,
        ..CharacterRegion::default()
    };
    material_document(&AnimeCharacterMaterial {
        diffuse: DiffuseMode::Bands(BandDiffuse { deep_threshold: Some(-0.4) }),
        ..swatch_material([
            region([0.93, 0.62, 0.58], [0.66, 0.38, 0.45]),
            region([0.52, 0.56, 0.9], [0.3, 0.32, 0.62]),
            region([0.56, 0.8, 0.66], [0.32, 0.52, 0.48]),
            region([0.78, 0.5, 0.82], [0.48, 0.3, 0.6]),
        ])
    })
}

fn material_document(material: &AnimeCharacterMaterial) -> String {
    use sapphire_assets::material::MaterialDocument;
    material.validate().expect("the swatch materials are valid");
    to_ron(material).expect("a material document serializes")
}

/// The material ID mask: each latitude band's ID as the gray level `id / 7`.
pub fn material_id_png() -> io::Result<Vec<u8>> {
    gray_png(|_, v| f32::from(BANDS[((v * 4.0) as usize).min(3)]) / 7.0)
}

/// The shadow bias mask (see the module docs).
pub fn shadow_bias_png() -> io::Result<Vec<u8>> {
    gray_png(|u, v| {
        let inside = |[low, high]: [f32; 2], x: f32| (low..high).contains(&x);
        let patch = BIAS_PATCHES.iter().find(|(us, vs, _)| inside(*us, u) && inside(*vs, v));
        match patch {
            Some(&(_, _, bias)) => bias,
            None if (u * BIAS_STRIPES as f32) as u32 % 2 == 1 => ilm::encode_shadow_bias(BIAS_STRIPE_OFFSET),
            None => ilm::encode_shadow_bias(0.0),
        }
    })
}

/// An 8-bit gray PNG [`SIZE`] square, each texel `value(u, v)` at its center.
fn gray_png(value: impl Fn(f32, f32) -> f32) -> io::Result<Vec<u8>> {
    let center = |i: u32| (i as f32 + 0.5) / SIZE as f32;
    let image = image::GrayImage::from_fn(SIZE, SIZE, |x, y| {
        image::Luma([(value(center(x), center(y)).clamp(0.0, 1.0) * 255.0).round() as u8])
    });
    let mut bytes = io::Cursor::new(Vec::new());
    image.write_to(&mut bytes, image::ImageFormat::Png).map_err(io::Error::other)?;
    Ok(bytes.into_inner())
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use sapphire_assets::character_maps::{MaterialId, ilm, ramp as rows};

    use super::*;
    use crate::character_maps::{MapKind, generate};
    use crate::test_util::TempDir;

    #[test]
    fn the_maps_generate_with_bands_stripes_and_rows() {
        let dir = TempDir::new("swatch_maps");
        for (name, bytes) in [
            (MATERIAL_ID_FILE_NAME, material_id_png().unwrap()),
            (SHADOW_BIAS_FILE_NAME, shadow_bias_png().unwrap()),
        ] {
            std::fs::write(dir.path().join(name), bytes).unwrap();
        }
        let ilm_path = dir.path().join(ILM_FILE_NAME);
        let ilm_map =
            generate(&ilm_path, ilm_document().as_bytes(), &MapKind::Ilm.default_settings()).unwrap();
        let texel = |u: f32, v: f32| {
            let (x, y) = ((u * SIZE as f32) as u32, (v * SIZE as f32) as u32);
            ilm_map.image.pixels[(y * SIZE + x) as usize]
        };
        // Bands top to bottom, decoded as the shader decodes them.
        for (band, &id) in BANDS.iter().enumerate() {
            let v = (band as f32 + 0.5) / 4.0;
            let decoded = ilm::decode_material_id(texel(0.1, v)[ilm::MATERIAL_ID]);
            assert_eq!(decoded, MaterialId::new(id).unwrap(), "band {band}");
        }
        let bias = |u, v| texel(u, v)[ilm::SHADOW_BIAS];
        let stripe = |u| ilm::decode_shadow_bias(bias(u, 0.2));
        assert!(stripe(0.01).abs() < 0.01 && (stripe(0.07) - BIAS_STRIPE_OFFSET).abs() < 0.01);
        assert_eq!(bias(0.28, 0.6), 0.0, "the painted shadow");
        assert_eq!(bias(0.08, 0.4), 1.0, "always lit");

        let ramp_path = Path::new("swatch.ramp.ron");
        let ramp_map =
            generate(ramp_path, ramp_document().as_bytes(), &MapKind::Ramp.default_settings()).unwrap();
        assert_eq!(ramp_map.image.height % rows::ROWS, 0, "whole rows");
    }

    #[test]
    fn the_materials_name_the_maps_by_their_sidecars_ids() {
        use sapphire_assets::material::AnyMaterialDocument;
        let samples = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../content/samples");
        for (map, id) in [(RAMP_FILE_NAME, RAMP_ID), (ILM_FILE_NAME, ILM_ID)] {
            let meta = crate::meta::read(&crate::meta::sidecar_path(&samples.join(map))).expect("a sidecar");
            assert_eq!(meta.id, id, "{map}");
        }
        let documents = [ramp_material_document(), subsurface_material_document(), band_material_document()];
        let textures: Vec<Vec<Option<AssetId>>> = documents
            .iter()
            .map(|text| AnyMaterialDocument::from_ron(text).expect("a material document").textures())
            .collect();
        for ramp_mode in &textures[..2] {
            assert!(ramp_mode.contains(&Some(RAMP_ID)) && ramp_mode.contains(&Some(ILM_ID)));
        }
        assert!(textures[2].iter().all(Option::is_none), "band mode reads no textures");
    }

    #[test]
    fn generation_is_deterministic() {
        assert_eq!(material_id_png().unwrap(), material_id_png().unwrap());
        assert_eq!(shadow_bias_png().unwrap(), shadow_bias_png().unwrap());
        assert_eq!(ramp_document(), ramp_document());
    }
}
