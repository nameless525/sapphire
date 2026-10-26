//! The material layer samples' content (M1 C7): five anime character materials over the shader's
//! layers, for the sandbox's `--materials`, with the painted matcap and the layer mask they read,
//! written as sources so they cook as an artist's would. No third-party art: every texel is
//! generated here.
//!
//! - **`lacquer.material.ron`**: crimson lacquer (a hair ornament's, a scabbard's): a crisp,
//!   small Blinn highlight barely tinted by the albedo, and a faint color matcap for its gloss.
//! - **`gold.material.ron`**: gold trim: the color matcap added in a gold tint, a highlight
//!   taking the albedo, and the reflection add on the layer mask's metallic stripes.
//! - **`leather.material.ron`**: brown leather: the matcap's grayscale A channel multiplying the
//!   albedo (its painted falloff, darker toward the silhouette), and a soft, wide GGX highlight.
//! - **`stockings.material.ron`**: skin in thigh-high black stockings: denier by N·V, a sheen
//!   along the leg (V) and a fine weave, the layer mask's A leaving the thigh bare above the
//!   stocking's top.
//! - **`glow.material.ron`**: a dark suit with cyan light lines (the layer mask's B), breathing
//!   over three seconds, its glow blooming as the world's light.
//! - **`matcap.png`** (color): a painted spheremap, sampled by the view-space normal: RGB a soft
//!   key highlight high on the left over a dark middle, with a cool bounce along the lower rim; A
//!   a grayscale falloff, white facing the camera to darker toward the rim.
//! - **`layers.png`** (a four-channel mask, `character_maps::layer_mask`'s layout): R the matcap
//!   everywhere; G metallic, six reflective stripes around the shape (at 1, above the reflection
//!   add's gate) between plain metal (0.5); B emission, five thin light lines across it and one
//!   along it; A stocking density, bare above `v` 0.12 and the full fabric from 0.17 down.
//!
//! The sandbox's shapes run `u` around them and `v` from the top down (the leg from the thigh to
//! the ankle), which the masks are laid out by. The materials name the textures by the IDs their
//! committed sidecars give them.

use std::io;

use sapphire_assets::AssetId;
use sapphire_assets::material::{
    AnimeCharacterMaterial, CharacterRegion, Emission, FabricDirection, LightSplit, Matcap, MatcapBlend,
    MatcapChannel, MaterialDocument, Outline, RegionMatcap, Specular, SpecularModel, Stockings,
};
use sapphire_serialize::to_ron;

use super::noise::smoothstep;
use crate::color::{linear_to_srgb, srgb8, unorm8};

/// The directory of the material layer samples, under the samples directory.
pub const DIR: &str = "materials";
/// File names, in [`DIR`].
pub const MATCAP_FILE_NAME: &str = "matcap.png";
pub const LAYERS_FILE_NAME: &str = "layers.png";
pub const LACQUER_FILE_NAME: &str = "lacquer.material.ron";
pub const GOLD_FILE_NAME: &str = "gold.material.ron";
pub const LEATHER_FILE_NAME: &str = "leather.material.ron";
pub const STOCKINGS_FILE_NAME: &str = "stockings.material.ron";
pub const GLOW_FILE_NAME: &str = "glow.material.ron";

/// The textures' IDs, which their committed sidecars give them: material documents name
/// textures by ID.
pub const MATCAP_ID: AssetId = AssetId::from_text("8abeeb58-7147-41c5-8b08-3ec8a616675a");
pub const LAYERS_ID: AssetId = AssetId::from_text("d9ef1a29-1b3d-4cc6-bcf9-be551a72c794");

/// The masks' size in texels.
const SIZE: u32 = 128;
/// Reflective stripes around the shape, and the share of each period they cover.
const STRIPES: f32 = 6.0;
const STRIPE_SHARE: f32 = 0.35;
/// Metallic between the stripes: below the reflection add's gate, so the matcap alone shows.
const PLAIN_METAL: f32 = 0.5;
/// The light lines across the shape (their `v`) and along it (its `u`), and their half width.
const LINES_ACROSS: [f32; 5] = [0.2, 0.35, 0.5, 0.65, 0.8];
const LINE_ALONG: f32 = 0.25;
const LINE_HALF_WIDTH: f32 = 0.012;
/// Where a stocking's top begins and where it is the full fabric, in `v`.
const STOCKING_TOP: [f32; 2] = [0.12, 0.17];
/// The matcap's sampling radius, as the shader takes it (`MATCAP_RADIUS`): view-space normals
/// cover the disc of this radius around the map's center.
const MATCAP_RADIUS: f32 = 0.49;

/// Every material sample file, relative to the samples directory, with its bytes.
pub fn files() -> io::Result<Vec<(String, Vec<u8>)>> {
    let path = |name: &str| format!("{DIR}/{name}");
    Ok(vec![
        (path(MATCAP_FILE_NAME), matcap_png()?),
        (path(LAYERS_FILE_NAME), layers_png()?),
        (path(LACQUER_FILE_NAME), document(&lacquer())),
        (path(GOLD_FILE_NAME), document(&gold())),
        (path(LEATHER_FILE_NAME), document(&leather())),
        (path(STOCKINGS_FILE_NAME), document(&stockings())),
        (path(GLOW_FILE_NAME), document(&glow())),
    ])
}

fn document(material: &AnimeCharacterMaterial) -> Vec<u8> {
    material.validate().expect("the material samples are valid");
    to_ron(material).expect("a material document serializes").into_bytes()
}

/// Linear RGB from 8-bit sRGB, the way an artist picks it.
fn rgb(r: u8, g: u8, b: u8) -> [f32; 3] {
    let [r, g, b, _] = srgb8(r, g, b);
    [r, g, b]
}

/// What every sample shares: a crisp split, a single region in `base`'s colors with its shadow
/// tone and line, and the default rim and outline at the swatches' width.
fn sample(base: [f32; 3], shade: [f32; 3], line: [f32; 3]) -> AnimeCharacterMaterial {
    AnimeCharacterMaterial {
        base_color: [base[0], base[1], base[2], 1.0],
        light_split: LightSplit { threshold: 0.05, softness: 0.02 },
        regions: vec![CharacterRegion {
            shade_color: shade,
            outline_color: line,
            ..CharacterRegion::default()
        }],
        outline: Outline { width: 3.5, ..Outline::default() },
        ..AnimeCharacterMaterial::default()
    }
}

/// Crimson lacquer: a small, crisp, near-white highlight and a little of the color matcap.
pub fn lacquer() -> AnimeCharacterMaterial {
    let mut material = sample(rgb(150, 24, 40), [0.55, 0.32, 0.5], rgb(52, 8, 18));
    material.specular = Specular { model: SpecularModel::Blinn, softness: 0.02, albedo: 0.15 };
    material.matcap = Some(Matcap { map: MATCAP_ID, strength: 0.3, shadow_strength: 0.12 });
    let region = &mut material.regions[0];
    region.specular_color = [1.0, 0.94, 0.92];
    region.specular_size = 56.0;
    material
}

/// Gold trim: the color matcap in a gold tint, a highlight taking the albedo, and the reflection
/// add on the mask's stripes.
pub fn gold() -> AnimeCharacterMaterial {
    let mut material = sample(rgb(226, 168, 70), [0.62, 0.42, 0.36], rgb(78, 44, 12));
    material.specular = Specular { model: SpecularModel::Blinn, softness: 0.03, albedo: 1.0 };
    material.matcap = Some(Matcap { map: MATCAP_ID, strength: 0.9, shadow_strength: 0.45 });
    material.layer_mask = Some(LAYERS_ID);
    let region = &mut material.regions[0];
    region.specular_color = [1.0, 0.92, 0.75];
    region.specular_size = 24.0;
    region.matcap =
        RegionMatcap { channel: MatcapChannel::Rgb, blend: MatcapBlend::Add, color: [1.0, 0.8, 0.45] };
    region.metallic = 1.0;
    region.roughness = 0.15;
    material
}

/// Brown leather: the matcap's A multiplying the albedo, and a soft, wide GGX highlight.
pub fn leather() -> AnimeCharacterMaterial {
    let mut material = sample(rgb(128, 76, 50), [0.6, 0.42, 0.5], rgb(40, 20, 12));
    material.specular = Specular { model: SpecularModel::Ggx, softness: 0.08, albedo: 0.6 };
    material.matcap = Some(Matcap { map: MATCAP_ID, strength: 1.0, shadow_strength: 0.8 });
    material.layer_mask = Some(LAYERS_ID);
    let region = &mut material.regions[0];
    region.specular_color = [0.5, 0.44, 0.4];
    region.specular_size = 0.45;
    region.matcap = RegionMatcap { channel: MatcapChannel::A, blend: MatcapBlend::Multiply, color: [1.0; 3] };
    material
}

/// Skin in thigh-high black stockings, sheer facing the camera, with a sheen along the leg and a
/// fine weave.
pub fn stockings() -> AnimeCharacterMaterial {
    let mut material = sample(rgb(255, 224, 206), rgb(236, 170, 168), rgb(110, 52, 48));
    material.layer_mask = Some(LAYERS_ID);
    material.stockings = Stockings {
        color: [0.012, 0.01, 0.016],
        edge_opacity: 0.98,
        facing_opacity: 0.82,
        range: [0.05, 0.95],
        sheen: [0.3, 0.3, 0.36],
        sheen_sharpness: 10.0,
        direction: FabricDirection::V,
        weave_scale: 600.0,
        weave_strength: 0.3,
    };
    material.regions[0].stocking = 1.0;
    material
}

/// A dark suit with breathing cyan light lines.
pub fn glow() -> AnimeCharacterMaterial {
    let mut material = sample(rgb(30, 34, 52), [0.7, 0.68, 0.95], rgb(10, 12, 22));
    material.layer_mask = Some(LAYERS_ID);
    material.emission = Emission { period: 3.0, depth: 0.45, bloom: 1.0 };
    material.regions[0].emission = [0.25, 1.5, 1.8];
    material
}

/// The matcap (see the module docs): RGB sRGB-encoded, A linear.
pub fn matcap_png() -> io::Result<Vec<u8>> {
    let texel = 1.0 / SIZE as f32;
    let image = image::RgbaImage::from_fn(SIZE, SIZE, |x, y| {
        let (u, v) = ((x as f32 + 0.5) * texel, (y as f32 + 0.5) * texel);
        // The view-space normal this texel shows, y up; past the rim, the rim's.
        let n = glam::Vec2::new(u - 0.5, 0.5 - v) / MATCAP_RADIUS;
        let n = if n.length() > 1.0 { n.normalize() } else { n };
        let facing = (1.0 - n.length_squared()).max(0.0).sqrt();
        // A soft key highlight high on the left, a cool bounce along the lower rim.
        let key = (-(n - glam::Vec2::new(-0.38, 0.42)).length_squared() * 7.0).exp();
        let bounce = smoothstep(0.65, 0.95, n.length()) * smoothstep(0.1, -0.6, n.y);
        let base = 0.04 + 0.1 * facing;
        let color = [
            base + 0.95 * key + 0.25 * bounce,
            base + 0.9 * key + 0.32 * bounce,
            base + 0.82 * key + 0.42 * bounce,
        ];
        // The grayscale falloff: white facing the camera, darker toward the rim, a little lifted
        // under the key.
        let falloff = (0.5 + 0.5 * facing.powf(0.6) + 0.1 * key).min(1.0);
        let [r, g, b] = color.map(|c| unorm8(linear_to_srgb(c)));
        image::Rgba([r, g, b, unorm8(falloff)])
    });
    png(image)
}

/// The layer mask (see the module docs), linear.
pub fn layers_png() -> io::Result<Vec<u8>> {
    let texel = 1.0 / SIZE as f32;
    let edge = 0.6 * texel;
    let image = image::RgbaImage::from_fn(SIZE, SIZE, |x, y| {
        let (u, v) = ((x as f32 + 0.5) * texel, (y as f32 + 0.5) * texel);
        // Metallic stripes around the shape, their edges a texel soft.
        let phase = (u * STRIPES).fract();
        let stripe = smoothstep(-edge * STRIPES, edge * STRIPES, STRIPE_SHARE - phase);
        let metallic = PLAIN_METAL + (1.0 - PLAIN_METAL) * stripe;
        // Light lines across and along it.
        let line = |distance: f32| smoothstep(LINE_HALF_WIDTH + edge, LINE_HALF_WIDTH - edge, distance);
        let across = LINES_ACROSS.iter().map(|&at| line((v - at).abs())).fold(0.0, f32::max);
        let along = line((u - LINE_ALONG).abs());
        let emission = across.max(along);
        let stocking = smoothstep(STOCKING_TOP[0], STOCKING_TOP[1], v);
        image::Rgba([unorm8(1.0), unorm8(metallic), unorm8(emission), unorm8(stocking)])
    });
    png(image)
}

fn png(image: image::RgbaImage) -> io::Result<Vec<u8>> {
    let mut bytes = io::Cursor::new(Vec::new());
    image.write_to(&mut bytes, image::ImageFormat::Png).map_err(io::Error::other)?;
    Ok(bytes.into_inner())
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use sapphire_assets::character_maps::layer_mask;
    use sapphire_assets::material::AnyMaterialDocument;

    use super::*;

    fn decode(bytes: &[u8]) -> image::RgbaImage {
        image::load_from_memory(bytes).expect("a PNG").to_rgba8()
    }

    #[test]
    fn the_layer_mask_paints_stripes_lines_and_a_stocking_top() {
        let mask = decode(&layers_png().unwrap());
        let at = |u: f32, v: f32| *mask.get_pixel((u * SIZE as f32) as u32, (v * SIZE as f32) as u32);
        let reflects =
            |texel: image::Rgba<u8>| f32::from(texel[layer_mask::METALLIC]) / 255.0 > layer_mask::REFLECTS;
        assert!(reflects(at(0.02, 0.5)), "inside a stripe");
        assert!(!reflects(at(0.12, 0.5)), "between stripes: plain metal");
        assert_eq!(at(0.6, 0.5)[layer_mask::EMISSION], 255, "on a line across");
        assert_eq!(at(0.6, 0.42)[layer_mask::EMISSION], 0, "between lines");
        assert_eq!(at(0.25, 0.42)[layer_mask::EMISSION], 255, "on the line along");
        assert_eq!(at(0.5, 0.05)[layer_mask::STOCKING], 0, "the bare thigh");
        assert_eq!(at(0.5, 0.5)[layer_mask::STOCKING], 255, "the full fabric");
        assert!(mask.pixels().all(|texel| texel[layer_mask::MATCAP] == 255), "the matcap everywhere");
    }

    #[test]
    fn the_matcap_is_lit_high_on_the_left_and_falls_off_toward_its_rim() {
        let matcap = decode(&matcap_png().unwrap());
        let at = |nx: f32, ny: f32| {
            let (u, v) = (0.5 + MATCAP_RADIUS * nx, 0.5 - MATCAP_RADIUS * ny);
            *matcap.get_pixel((u * SIZE as f32) as u32, (v * SIZE as f32) as u32)
        };
        assert!(at(-0.38, 0.42)[0] > 200, "the key highlight");
        assert!(at(0.3, 0.0)[0] < at(-0.38, 0.42)[0] / 2, "the dark middle");
        let (facing, rim) = (at(0.0, 0.0)[3], at(0.97, 0.0)[3]);
        assert!(facing > 240 && rim < 200, "the falloff: {facing} facing, {rim} at the rim");
    }

    #[test]
    fn the_materials_name_the_textures_by_their_sidecars_ids() {
        let samples = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../content/samples").join(DIR);
        for (texture, id) in [(MATCAP_FILE_NAME, MATCAP_ID), (LAYERS_FILE_NAME, LAYERS_ID)] {
            let meta =
                crate::meta::read(&crate::meta::sidecar_path(&samples.join(texture))).expect("a sidecar");
            assert_eq!(meta.id, id, "{texture}");
        }
        for material in [lacquer(), gold(), leather(), stockings(), glow()] {
            let text = String::from_utf8(document(&material)).expect("UTF-8");
            let read = AnyMaterialDocument::from_ron(&text).expect("a material document");
            let named = read.textures();
            assert!(named.iter().flatten().all(|id| [MATCAP_ID, LAYERS_ID].contains(id)), "{named:?}");
        }
    }

    #[test]
    fn generation_is_deterministic() {
        assert_eq!(matcap_png().unwrap(), matcap_png().unwrap());
        assert_eq!(layers_png().unwrap(), layers_png().unwrap());
        assert_eq!(document(&gold()), document(&gold()));
    }
}
