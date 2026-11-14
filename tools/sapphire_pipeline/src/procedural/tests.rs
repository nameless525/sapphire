use std::fs;

use sapphire_assets::AssetId;
use sapphire_assets::material::{EnvironmentMaterial, MaterialDocument};
use sapphire_assets::texture::{Texture, TextureFormat, TextureTail};
use sapphire_assets::texture_level::TextureLevel;

use super::*;
use crate::cook::{CookOptions, CookStatus, cook_dir};
use crate::meta::{AssetMeta, ImportSettings, sidecar_path};
use crate::test_util::TempDir;

/// The tests' sources' IDs are derived from this.
const TEST_ROOT: AssetId = AssetId::from_text("a41225c1-2b64-4592-ae56-ea2194950d97");

/// The module docs' example, at a small size.
const BRICK: &str = r##"(
    type: "sapphire::pipeline::ProceduralTexture",
    version: 1,
    data: (
        size: 32,
        seed: 7,
        pattern: Blocks((columns: 4, rows: 8, joint: 0.03, stagger: 0.5, bevel: 0.03)),
        grain: (scale: 4, octaves: 3, depth: 0.25),
        colors: [(0.0, "#7a5a48"), (1.0, "#b08870")],
        joint_color: "#4a4440",
        relief: 0.02,
        roughness: (0.6, 0.95),
        specular: 0.2,
    ),
)"##;

fn brick() -> ProceduralTexture {
    ProceduralTexture::read(BRICK.as_bytes()).unwrap()
}

fn stones() -> ProceduralTexture {
    ProceduralTexture {
        seed: 3,
        pattern: Pattern::Stones(Stones { cells: 3, joint: 0.02, bevel: 0.02 }),
        colors: vec![
            (0.0, SrgbColor([90, 92, 96, 255])),
            (0.6, SrgbColor([140, 136, 128, 255])),
            (1.0, SrgbColor([170, 168, 160, 255])),
        ],
        ..brick()
    }
}

fn plain() -> ProceduralTexture {
    ProceduralTexture { seed: 11, pattern: Pattern::Plain, joint_color: None, ..brick() }
}

fn texel_hash(image: &SourceImage) -> String {
    let mut hasher = blake3::Hasher::new();
    for pixel in &image.pixels {
        for value in pixel {
            hasher.update(&value.to_bits().to_le_bytes());
        }
    }
    hasher.finalize().to_hex().to_string()
}

#[test]
fn rasterizes_the_same_texels_on_every_machine() {
    // blake3 of every texel's f32 bits, little-endian: the same on x86-64 and AArch64, as the
    // rasterizer is the four operations, `floor` and `sqrt`, and det's sRGB decoding.
    let pinned: [(&str, ProceduralTexture, [&str; 3]); 3] = [
        (
            "brick",
            brick(),
            [
                "ed3877ba29caf6d559a66c230da8a345040ad432c899a41a1b9ae951d8187223",
                "151c63f4c08375ca1da8057a9f9b53d3233f939b728a221bc063b853f1e24e5d",
                "b850be35bd8423c09ec24506e2700083e7d8ca7c8e0f9b6c0d3b382d6d54ff6b",
            ],
        ),
        (
            "stones",
            stones(),
            [
                "e5dac446b71117016cbf7f68fbe230ce115ae5c2794b20bca03d4f54392181d8",
                "0d09eca32fb117c2f2b4b1199bb742e515b5d8a51d8a4626437f7b1d9bb3a35a",
                "e718274d731d8e149f53c97f7efecb7929ec56e95b5e82489868ddcf1875af71",
            ],
        ),
        (
            "plain",
            plain(),
            [
                "32ab3a85ec5bb49d0326cc23215d529b7cd83f349bce60b2e3658a6fe5454b80",
                "1e748ae1dcaf96eac15b4fcef1498c526a73834f8772026d2e1c0e53ddcb3455",
                "540f9a78834b2e74a783f91d33995ded03d92c8a55a6b641447ae9809ea40810",
            ],
        ),
    ];
    let mut wrong = Vec::new();
    for (name, surface, hashes) in &pinned {
        for (map, expected) in SurfaceMap::ALL.into_iter().zip(hashes) {
            let found = texel_hash(&surface.rasterize(map));
            if found != *expected {
                wrong.push(format!("{name} {map:?}: {found}"));
            }
        }
    }
    assert!(wrong.is_empty(), "rasterized texels moved:\n{}", wrong.join("\n"));
}

#[test]
fn rasterizing_twice_makes_the_same_texels() {
    for map in SurfaceMap::ALL {
        assert_eq!(stones().rasterize(map), stones().rasterize(map));
    }
}

#[test]
fn every_pattern_repeats_with_the_texture() {
    let seed = noise::hash(5);
    let blocks = Blocks { columns: 5, rows: 6, joint: 0.01, stagger: 0.5, bevel: 0.02 };
    let stones = Stones { cells: 4, joint: 0.01, bevel: 0.01 };
    for i in 0..97 {
        let (u, v) = ((i as f32 * 0.137).fract(), (i as f32 * 0.291).fract());
        for (du, dv) in [(1.0, 0.0), (0.0, 1.0), (-1.0, 1.0)] {
            // Joints line up across both edges. A block straddles only the side edges: past the top,
            // a running bond's next row is the first row again, a different block.
            let (a, b) = (blocks_place(blocks, u, v, seed), blocks_place(blocks, u + du, v + dv, seed));
            assert!((a.edge - b.edge).abs() < 1e-4, "blocks at ({u}, {v}) + ({du}, {dv})");
            assert!(dv != 0.0 || a.tone == b.tone, "blocks at ({u}, {v}) + ({du}, {dv})");
            let (a, b) = (stones_place(stones, u, v, seed), stones_place(stones, u + du, v + dv, seed));
            assert!(
                (a.edge - b.edge).abs() < 1e-4 && a.tone == b.tone,
                "stones at ({u}, {v}) + ({du}, {dv})"
            );
        }
    }
}

#[test]
fn a_flat_surface_has_a_flat_normal_map() {
    let flat = ProceduralTexture { grain: Grain { depth: 0.0, ..Grain::default() }, ..plain() };
    let normal = flat.rasterize(SurfaceMap::Normal);
    assert!(normal.pixels.iter().all(|&pixel| pixel == [0.5, 0.5, 1.0, 1.0]));
    // And a joint is rough, unlit by specular and dark with occlusion, whatever the grain.
    let wide = ProceduralTexture {
        pattern: Pattern::Blocks(Blocks { columns: 2, rows: 2, joint: 0.2, stagger: 0.0, bevel: 0.0 }),
        ..brick()
    };
    let mask = wide.rasterize(SurfaceMap::Mask);
    let field = Field::new(&wide);
    let deepest = (0..field.joint.len()).find(|&i| field.joint[i] == 1.0).expect("a texel all joint");
    let [occlusion, cavity, specular, roughness] = mask.pixels[deepest];
    assert_eq!([occlusion, cavity, specular], [0.45, 0.7, 0.0]);
    assert!((roughness - 1.0).abs() < 1e-6);
}

#[test]
fn every_check_names_the_field_it_failed() {
    let field_of = |surface: ProceduralTexture| match surface.check() {
        Err(ProceduralError::Invalid { field, .. }) => field,
        other => panic!("expected an invalid field, found {other:?}"),
    };
    let blocks = |change: fn(&mut Blocks)| {
        let mut blocks = Blocks { columns: 4, rows: 8, joint: 0.01, stagger: 0.5, bevel: 0.0 };
        change(&mut blocks);
        ProceduralTexture { pattern: Pattern::Blocks(blocks), ..brick() }
    };
    assert_eq!(field_of(ProceduralTexture { size: 30, ..brick() }), "size");
    assert_eq!(field_of(ProceduralTexture { size: 0, ..brick() }), "size");
    assert_eq!(field_of(blocks(|b| b.columns = 0)), "pattern");
    assert_eq!(field_of(blocks(|b| b.joint = 0.5)), "pattern");
    assert_eq!(
        field_of(blocks(|b| b.rows = 3)),
        "pattern",
        "half a block's stagger over 3 rows doesn't tile"
    );
    assert_eq!(
        field_of(ProceduralTexture {
            pattern: Pattern::Stones(Stones { cells: 0, joint: 0.0, bevel: 0.0 }),
            ..brick()
        }),
        "pattern"
    );
    assert_eq!(field_of(ProceduralTexture { joint_color: None, ..brick() }), "joint_color");
    assert_eq!(
        field_of(ProceduralTexture { grain: Grain { octaves: 13, ..Grain::default() }, ..brick() }),
        "grain"
    );
    assert_eq!(
        field_of(ProceduralTexture { grain: Grain { scale: 64, ..Grain::default() }, ..brick() }),
        "grain"
    );
    assert_eq!(field_of(ProceduralTexture { colors: Vec::new(), ..brick() }), "colors");
    let backwards = vec![(1.0, SrgbColor([0, 0, 0, 255])), (0.0, SrgbColor([255, 255, 255, 255]))];
    assert_eq!(field_of(ProceduralTexture { colors: backwards, ..brick() }), "colors");
    assert_eq!(field_of(ProceduralTexture { relief: -0.1, ..brick() }), "relief");
    assert_eq!(field_of(ProceduralTexture { roughness: (0.5, 1.5), ..brick() }), "roughness");
    assert_eq!(field_of(ProceduralTexture { specular: f32::NAN, ..brick() }), "specular");
    assert!(plain().check().is_ok() && stones().check().is_ok());
}

#[test]
fn a_document_that_doesnt_read_is_an_error() {
    let unknown = BRICK.replace("specular: 0.2,", "specular: 0.2, shine: 1.0,");
    assert!(matches!(ProceduralTexture::read(unknown.as_bytes()), Err(ProceduralError::Document(_))));
    assert!(matches!(ProceduralTexture::read(&[0xff, 0xfe]), Err(ProceduralError::NotUtf8)));
    assert_eq!(cook_memory(b"not a document"), 0);
    assert_eq!(
        cook_memory(BRICK.as_bytes()),
        32 * 32 * (texture::COOK_BYTES_PER_TEXEL + FIELD_BYTES_PER_TEXEL)
    );
}

#[test]
fn a_map_previews_as_a_png_of_its_size() {
    for map in SurfaceMap::ALL {
        let png = to_png(&brick().rasterize(map), map).unwrap();
        let decoded = image::load_from_memory(&png).unwrap();
        assert_eq!((decoded.width(), decoded.height()), (32, 32), "{map:?}");
    }
}

#[test]
fn sources_are_named_by_their_suffix() {
    assert!(is_source(Path::new("walls/brick.color.ptex.ron")));
    assert!(is_source(Path::new("BRICK.PTEX.RON")));
    assert!(!is_source(Path::new(".ptex.ron")));
    assert!(!is_source(Path::new("brick.ron")));
    assert_eq!(
        output_path(Path::new("walls/brick.mask.ptex.ron"), "ktx2"),
        Path::new("walls/brick.mask.ktx2")
    );
}

#[test]
fn each_map_cooks_as_its_class() {
    // (map, desktop's format, whether mobile's ASTC decodes sRGB).
    let expected = [
        (SurfaceMap::Color, TextureFormat::Bc7RgbaUnormSrgb, true),
        (SurfaceMap::Normal, TextureFormat::Bc5RgUnorm, false),
        (SurfaceMap::Mask, TextureFormat::Bc7RgbaUnorm, false),
    ];
    for (map, desktop, srgb) in expected {
        let settings = ProceduralSettings::from(map);
        let cooked = cook_source(BRICK.as_bytes(), &settings, Cook::Desktop).unwrap();
        let texture = Texture::from_ktx2(&cooked).unwrap();
        assert_eq!(texture.desc.format, desktop, "{map:?} on desktop");
        assert_eq!((texture.desc.width, texture.desc.height, texture.desc.mip_level_count), (32, 32, 6));
        let cooked = cook_source(BRICK.as_bytes(), &settings, Cook::Mobile).unwrap();
        let format = Texture::from_ktx2(&cooked).unwrap().desc.format;
        assert!(
            matches!(format, TextureFormat::Astc { srgb: s, .. } if s == srgb),
            "{map:?} on mobile: {format:?}"
        );
    }
}

/// A set's three maps, written under `dir` with sidecars binding each to its map.
fn write_set(root: &Path, name: &str, document: &str) -> [AssetId; 3] {
    SurfaceMap::ALL.map(|map| {
        let path = root.join(format!("{name}.{}.ptex.ron", format!("{map:?}").to_ascii_lowercase()));
        fs::write(&path, document).unwrap();
        let id = TEST_ROOT.sub_asset(&format!("{name}/{map:?}"));
        let meta = AssetMeta::new(id, ImportSettings::ProceduralTexture(ProceduralSettings::from(map)));
        fs::write(sidecar_path(&path), sapphire_serialize::to_ron(&meta).unwrap()).unwrap();
        id
    })
}

#[test]
fn an_environment_material_reads_a_procedural_set() {
    let dir = TempDir::new("procedural_set");
    let source = dir.path().join("content");
    fs::create_dir_all(&source).unwrap();
    let [color, normal, mask] = write_set(&source, "brick", BRICK);
    let material = EnvironmentMaterial {
        base_color_map: Some(color),
        normal_map: Some(normal),
        mask_map: Some(mask),
        ..EnvironmentMaterial::default()
    };
    material.validate().unwrap();
    let path = source.join("brick.material.ron");
    fs::write(&path, sapphire_serialize::to_ron(&material).unwrap()).unwrap();
    let meta =
        AssetMeta::new(TEST_ROOT.sub_asset("brick/material"), ImportSettings::Material(Default::default()));
    fs::write(sidecar_path(&path), sapphire_serialize::to_ron(&meta).unwrap()).unwrap();

    for cook in [Cook::Desktop, Cook::Mobile] {
        let out = dir.path().join("out").join(cook.name());
        let options = CookOptions { cook, cache: None, locked: true, editor: false };
        let report = cook_dir(&source, &out, &options).unwrap();
        assert_eq!(report.assets.len(), 4);
        assert!(report.assets.iter().all(|asset| asset.status == CookStatus::Cooked), "{report:?}");
        for map in ["color", "normal", "mask"] {
            assert!(out.join(format!("brick.{map}.ktx2")).is_file(), "{map} on {cook:?}");
        }
        assert!(out.join("brick.smat").is_file());
    }

    // At 512 texels each map splits into its tail and levels as a texture of its class does (M5 M3):
    // 512 and 256 above a tail whose top is 128, with the levels beside the tail, and the chain the
    // cook would write whole.
    let large = BRICK.replace("size: 32", "size: 512");
    for map in ["color", "normal", "mask"] {
        fs::write(source.join(format!("brick.{map}.ptex.ron")), &large).unwrap();
    }
    for cook in [Cook::Desktop, Cook::Mobile] {
        let out = dir.path().join("large").join(cook.name());
        let options = CookOptions { cook, cache: None, locked: true, editor: false };
        let report = cook_dir(&source, &out, &options).unwrap();
        assert_eq!(report.assets.len(), 4 + 3 * 2, "{cook:?}");
        for (map, surface) in
            [("color", SurfaceMap::Color), ("normal", SurfaceMap::Normal), ("mask", SurfaceMap::Mask)]
        {
            let settings = ProceduralSettings::from(surface);
            assert_eq!(planned_levels(large.as_bytes(), &settings, cook), 2, "{map} on {cook:?}");
            let tail_file = fs::read(out.join(format!("brick.{map}.ktx2"))).unwrap();
            let tail = TextureTail::from_ktx2(&tail_file).unwrap();
            let whole = cook_chain(large.as_bytes(), &settings, cook).unwrap();
            assert_eq!((tail.above, tail.whole()), (2, whole.desc), "{map} on {cook:?}");
            for level in 0..2 {
                let bytes = fs::read(out.join(format!("brick.{map}.mip{level}.smip"))).unwrap();
                let read = TextureLevel::read(&bytes).unwrap();
                assert!(read.data() == whole.levels[level as usize], "{map} level {level} on {cook:?}");
            }
        }
    }

    // A normal map in the base colour's slot is the material's error, as any texture's would be.
    let swapped = EnvironmentMaterial { base_color_map: Some(normal), ..material };
    fs::write(&path, sapphire_serialize::to_ron(&swapped).unwrap()).unwrap();
    let options = CookOptions { cook: Cook::Desktop, cache: None, locked: true, editor: false };
    assert!(cook_dir(&source, &dir.path().join("swapped"), &options).is_err());
}

/// A procedural map takes a texture's cap by the same field (M5 M1): one document cooks to its whole
/// size on desktop and to the levels under its cap on mobile, the uncapped cook's levels byte for byte.
#[test]
fn a_map_is_capped_per_cook_as_a_texture_is() {
    let large = BRICK.replace("size: 32", "size: 256");
    let max_size = MaxSize { mobile: std::num::NonZeroU32::new(64), ..MaxSize::NONE };
    for map in SurfaceMap::ALL {
        let whole = ProceduralSettings::from(map);
        let capped = ProceduralSettings { map, max_size };
        let desktop = cook_chain(large.as_bytes(), &capped, Cook::Desktop).unwrap();
        assert_eq!(desktop, cook_chain(large.as_bytes(), &whole, Cook::Desktop).unwrap(), "{map:?}");
        assert_eq!(cooked_size(large.as_bytes(), &capped, Cook::Desktop).unwrap(), (256, 256));

        let uncapped = cook_chain(large.as_bytes(), &whole, Cook::Mobile).unwrap();
        let mobile = cook_chain(large.as_bytes(), &capped, Cook::Mobile).unwrap();
        assert_eq!((mobile.desc.width, mobile.desc.height), (64, 64), "{map:?}");
        assert!(mobile.levels == uncapped.levels[2..], "{map:?}: the uncapped cook's levels under the cap");
        assert_eq!(cooked_size(large.as_bytes(), &capped, Cook::Mobile).unwrap(), (64, 64));
        // 64 is under the tail's 128, so the capped map stays whole.
        assert_eq!(planned_levels(large.as_bytes(), &capped, Cook::Mobile), 0);
        assert_eq!(planned_levels(large.as_bytes(), &capped, Cook::Desktop), 1);
    }
}
