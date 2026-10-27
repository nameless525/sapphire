//! Map documents end to end: documents and masks in a content tree, cooked through the
//! content cache into KTX2.

use std::fs;
use std::path::{Path, PathBuf};

use sapphire_assets::character_maps::ramp::{Half, row, row_center};
use sapphire_assets::character_maps::{MaterialId, face_sdf, ilm as ilm_layout};
use sapphire_assets::texture::{AstcBlock, Texture, TextureFormat};
use sapphire_serialize::{from_ron, to_ron};

use super::face_sdf::ThresholdMask;
use super::ilm::{ChannelInput, Encoding, ImageChannel, MaterialIdInput};
use super::ramp::{ColorStop, Interpolation, MaterialRamps, Ramp, SrgbColor};
use super::*;
use crate::cook::{CookError, CookOptions, CookReport, CookStatus, cook_dir};
use crate::meta::{self, AssetMeta, ImportSettings, sidecar_path};
use crate::test_util::TempDir;
use crate::texture::{MaskSettings, SdfPrecision};

/// The lit region of the test face at a light angle: a disc whose radius shrinks with the
/// angle, centered left of the middle, so the shadow sweeps in from the right.
fn lit(degrees: f32, x: u32, y: u32) -> bool {
    let radius = 28.0 - f64::from(degrees) * 0.2;
    ((f64::from(x) - 24.5).powi(2) + (f64::from(y) - 31.5).powi(2)).sqrt() < radius
}

const SIZE: u32 = 64;
const ANGLES: [f32; 4] = [0.0, 30.0, 60.0, 120.0];

fn write_png(path: &Path, image: image::DynamicImage) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    image.save_with_format(path, image::ImageFormat::Png).unwrap();
}

fn mask_image(lit: impl Fn(u32, u32) -> bool) -> image::DynamicImage {
    image::GrayImage::from_fn(SIZE, SIZE, |x, y| image::Luma([if lit(x, y) { 255 } else { 0 }])).into()
}

fn face_document(vertical: bool) -> FaceSdfMap {
    let masks = |prefix: &str| {
        ANGLES
            .iter()
            .map(|&angle| ThresholdMask { angle, path: format!("masks/{prefix}_{angle:03}.png") })
            .collect()
    };
    FaceSdfMap {
        horizontal: masks("h"),
        vertical: if vertical { masks("v") } else { Vec::new() },
        allowed_violations: 0,
    }
}

/// A content tree with a face map (its document and masks), a ramp and an ILM document, and
/// an ordinary texture.
struct Tree {
    dir: TempDir,
}

impl Tree {
    fn new(name: &str) -> Self {
        let tree = Self { dir: TempDir::new(name) };
        let face = tree.source().join("face");
        for angle in ANGLES {
            write_png(&face.join(format!("masks/h_{angle:03}.png")), mask_image(|x, y| lit(angle, x, y)));
        }
        tree.write_document(&face.join("face.facesdf.ron"), &face_document(false));
        tree.write_document(&tree.source().join("body/body.ramp.ron"), &ramp_document());
        let body = tree.source().join("body");
        write_png(
            &body.join("masks/spec.png"),
            image::GrayImage::from_fn(16, 8, |x, _| image::Luma([(x * 17) as u8])).into(),
        );
        let ids = image::GrayImage::from_fn(16, 8, |x, _| image::Luma([if x < 8 { 0 } else { 75 }]));
        write_png(&body.join("masks/ids.png"), ids.into());
        tree.write_document(&body.join("body.ilm.ron"), &ilm_document());
        write_png(&tree.source().join("props/albedo.png"), image::RgbaImage::new(8, 8).into());
        tree
    }

    fn source(&self) -> PathBuf {
        self.dir.path().join("content")
    }

    fn out(&self, cook: Cook) -> PathBuf {
        self.dir.path().join("out").join(cook.name())
    }

    fn write_document<T: Versioned + Serialize + TypePath>(&self, path: &Path, document: &T) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, to_ron(document).unwrap()).unwrap();
    }

    fn try_cook(&self, cook: Cook) -> Result<CookReport, CookError> {
        let cache = self.dir.path().join("cache");
        cook_dir(&self.source(), &self.out(cook), &CookOptions { cook, cache: Some(&cache), locked: false })
    }

    fn cook(&self, cook: Cook) -> CookReport {
        self.try_cook(cook).unwrap()
    }

    /// Adds vertical masks to the face map: the horizontal shapes mirrored across the
    /// diagonal, for a shadow rising from below.
    fn add_vertical(&self) {
        let face = self.source().join("face");
        for angle in ANGLES {
            write_png(&face.join(format!("masks/v_{angle:03}.png")), mask_image(|x, y| lit(angle, y, x)));
        }
        self.write_document(&face.join("face.facesdf.ron"), &face_document(true));
    }

    fn set_settings(&self, relative: &str, settings: MapSettings) {
        let sidecar = sidecar_path(&self.source().join(relative));
        let mut meta = meta::read(&sidecar).unwrap();
        meta.importer = ImportSettings::CharacterMap(settings);
        fs::write(&sidecar, to_ron::<AssetMeta>(&meta).unwrap()).unwrap();
    }
}

fn ramp_document() -> RampMap {
    let stop = |at, color: &str| ColorStop { at, color: color.parse().unwrap(), interpolation: None };
    let ramp = |stops| Ramp { interpolation: None, stops };
    RampMap {
        interpolation: Interpolation::OkLab,
        materials: vec![
            MaterialRamps {
                name: "skin".into(),
                day: ramp(vec![stop(0.0, "#c07060"), stop(1.0, "#f0b090")]),
                night: Some(ramp(vec![stop(0.0, "#504070"), stop(1.0, "#9080b0")])),
            },
            MaterialRamps {
                name: "cloth".into(),
                day: ramp(vec![stop(0.0, "#203060"), stop(0.6, "#3050a0")]),
                night: None,
            },
        ],
    }
}

fn ilm_document() -> IlmMap {
    IlmMap {
        size: None,
        specular_mask: ChannelInput::Image {
            path: "masks/spec.png".into(),
            channel: ImageChannel::R,
            range: [0.0, 1.0],
            encoding: Encoding::Linear,
        },
        // The same mask, inverted and halved, as a shadow bias.
        shadow_bias: ChannelInput::Image {
            path: "masks/spec.png".into(),
            channel: ImageChannel::R,
            range: [0.5, 0.0],
            encoding: Encoding::Linear,
        },
        specular_threshold: ChannelInput::Constant(0.5),
        // Genshin-style levels: 75/255 ≈ 0.29 is nearest 0.3, ID 1.
        material_id: MaterialIdInput::Image {
            path: "masks/ids.png".into(),
            channel: ImageChannel::R,
            levels: vec![0.0, 0.3, 0.5, 0.7, 1.0],
        },
    }
}

fn texture(path: &Path) -> (sapphire_assets::texture::TextureDesc, Vec<u8>) {
    let bytes = fs::read(path).unwrap();
    let texture = Texture::from_ktx2(&bytes).unwrap();
    (texture.desc, texture.levels[0].to_vec())
}

fn statuses(report: &CookReport) -> Vec<(String, CookStatus)> {
    report
        .assets
        .iter()
        .map(|asset| (asset.source.file_name().unwrap().to_string_lossy().into_owned(), asset.status))
        .collect()
}

#[test]
fn documents_round_trip_and_read_as_documented() {
    let ramp = ramp_document();
    assert_eq!(from_ron::<RampMap>(&to_ron(&ramp).unwrap()).unwrap(), ramp);
    let ilm = ilm_document();
    assert_eq!(from_ron::<IlmMap>(&to_ron(&ilm).unwrap()).unwrap(), ilm);
    let face = face_document(true);
    assert_eq!(from_ron::<FaceSdfMap>(&to_ron(&face).unwrap()).unwrap(), face);

    // Hand-written, with everything optional left out.
    let face: FaceSdfMap = from_ron(
        r#"(type: "sapphire::pipeline::FaceSdfMap", version: 1, data: (
            horizontal: [(angle: 0.0, path: "h_000.png"), (angle: 45.0, path: "h_045.png")],
        ))"#,
    )
    .unwrap();
    assert_eq!((face.vertical.len(), face.allowed_violations), (0, 0));
    let ramp: RampMap = from_ron(
        r##"(type: "sapphire::pipeline::RampMap", version: 1, data: (
            materials: [(
                day: (stops: [(at: 0.0, color: "#c07060"), (at: 1.0, color: "#f0b090", interpolation: Step)]),
                night: (interpolation: Linear, stops: [(at: 0.5, color: "#50407080")]),
            )],
        ))"##,
    )
    .unwrap();
    assert_eq!(ramp.interpolation, Interpolation::OkLab);
    assert_eq!(ramp.materials[0].night.as_ref().unwrap().stops[0].color, SrgbColor([0x50, 0x40, 0x70, 0x80]));
    let ilm: IlmMap = from_ron(
        r#"(type: "sapphire::pipeline::IlmMap", version: 1, data: (
            shadow_bias: Image(path: "ao.png", range: (0.0, 0.5)),
            material_id: Constant(3),
        ))"#,
    )
    .unwrap();
    assert_eq!(ilm.specular_mask, ChannelInput::Constant(0.0), "left out: neutral");
    assert_eq!(ilm.specular_threshold, ChannelInput::Constant(0.5));
    assert_eq!(
        ilm.shadow_bias,
        ChannelInput::Image {
            path: "ao.png".into(),
            channel: ImageChannel::R,
            range: [0.0, 0.5],
            encoding: Encoding::Linear
        }
    );
    // Layer masks: a channel left out is 1, what an unbound mask reads; an input may be sRGB.
    let layers: LayerMaskMap = from_ron(
        r#"(type: "sapphire::pipeline::LayerMaskMap", version: 1, data: (
            emission: Image(path: "glow.png", channel: G, encoding: Srgb),
        ))"#,
    )
    .unwrap();
    assert_eq!(
        [&layers.matcap, &layers.metallic, &layers.stocking],
        [&ChannelInput::Constant(1.0); 3],
        "left out: unpainted"
    );
    assert_eq!(
        layers.emission,
        ChannelInput::Image {
            path: "glow.png".into(),
            channel: ImageChannel::G,
            range: [0.0, 1.0],
            encoding: Encoding::Srgb
        }
    );
    assert_eq!(from_ron::<LayerMaskMap>(&to_ron(&layers).unwrap()).unwrap(), layers);
    // A linear input, the default, isn't written out: ILM documents read as they did.
    assert!(!to_ron(&ilm).unwrap().contains("encoding"), "{}", to_ron(&ilm).unwrap());

    // Typos are errors, not silently ignored settings.
    for typo in [
        r#"(type: "sapphire::pipeline::FaceSdfMap", version: 1, data: (horizontal: [], vertcal: []))"#,
        r#"(type: "sapphire::pipeline::IlmMap", version: 1, data: (shadow_bias: Image(path: "a.png", rnage: (0.0, 1.0))))"#,
        r#"(type: "sapphire::pipeline::LayerMaskMap", version: 1, data: (emision: Constant(0.5)))"#,
    ] {
        assert!(
            from_ron::<FaceSdfMap>(typo).is_err()
                && from_ron::<IlmMap>(typo).is_err()
                && from_ron::<LayerMaskMap>(typo).is_err(),
            "{typo}"
        );
    }
    let error = from_ron::<RampMap>(
        r#"(type: "sapphire::pipeline::RampMap", version: 1, data: (materials: [(day: (stops: [(at: 0.0, color: "red")]))]))"#,
    )
    .unwrap_err();
    assert!(error.to_string().contains("\"red\" is not a color"), "{error}");
}

#[test]
fn document_kinds_go_by_their_compound_extensions() {
    assert_eq!(MapKind::for_path(Path::new("a/face.facesdf.ron")), Some(MapKind::FaceSdf));
    assert_eq!(MapKind::for_path(Path::new("Body.RAMP.RON")), Some(MapKind::Ramp));
    assert_eq!(MapKind::for_path(Path::new("body.ilm.ron")), Some(MapKind::Ilm));
    assert_eq!(MapKind::for_path(Path::new("hair.layers.ron")), Some(MapKind::LayerMask));
    assert_eq!(MapKind::for_path(Path::new(".ramp.ron")), None);
    assert_eq!(MapKind::for_path(Path::new("material.ron")), None);
    for kind in MapKind::ALL {
        let path = PathBuf::from(format!("x{}", kind.suffix()));
        assert_eq!(
            ImportSettings::for_source(&path),
            Some(ImportSettings::CharacterMap(kind.default_settings()))
        );
        assert_eq!(kind.default_settings().kind(), kind);
    }
    // Importers that decode masks carry the image importer's version.
    for kind in [MapKind::FaceSdf, MapKind::Ilm, MapKind::LayerMask] {
        assert_eq!(kind.importer().version & 0xffff, image_import::IMPORTER.version);
        assert_eq!(kind.importer().version >> 16, 1);
    }
    assert_eq!(MapKind::Ramp.importer().version, 1);
}

#[test]
fn inputs_resolve_relative_to_their_document() {
    let document = Path::new("content/characters/a/face.facesdf.ron");
    let resolve = |input| resolve_input(document, input).unwrap();
    assert_eq!(resolve("masks/h.png"), Path::new("content/characters/a/masks/h.png"));
    assert_eq!(resolve("./masks/../h.png"), Path::new("content/characters/a/h.png"));
    assert_eq!(resolve("../shared/h.png"), Path::new("content/characters/shared/h.png"));
    for bad in ["", "/abs/h.png"] {
        assert!(matches!(resolve_input(document, bad), Err(MapError::InputPath(_))), "{bad:?}");
    }
    // Past the start of a relative path, `..` is kept rather than swallowed.
    assert_eq!(resolve_input(Path::new("a/x.ilm.ron"), "../../b.png").unwrap(), Path::new("../b.png"));
    assert_eq!(resolve_input(Path::new("x.ilm.ron"), "../b.png").unwrap(), Path::new("../b.png"));
    assert_eq!(resolve_input(Path::new("../x.ilm.ron"), "../b.png").unwrap(), Path::new("../../b.png"));
}

#[test]
fn claimed_inputs_must_name_their_files_exactly_and_not_be_assets() {
    let tree = Tree::new("maps_claims");
    let face = tree.source().join("face/face.facesdf.ron");
    let map_error = |error: CookError| match error {
        CookError::Map { path, source } if path == face => *source,
        other => panic!("expected a map error for the face, got {other:?}"),
    };

    // A name whose case differs from the file's: some file systems would read it, others not.
    let mut document = face_document(false);
    document.horizontal[1].path = "masks/H_030.png".into();
    tree.write_document(&face, &document);
    let error = map_error(tree.try_cook(Cook::Desktop).unwrap_err());
    assert!(
        matches!(&error, MapError::MissingInput(path) if path.ends_with("face/masks/H_030.png")),
        "{error:?}"
    );

    // An image that is an asset of its own (it has a sidecar) can't also be an input.
    tree.write_document(&face, &face_document(false));
    let mask = tree.source().join("face/masks/h_030.png");
    let other = AssetMeta::new(meta::new_id().unwrap(), ImportSettings::for_source(&mask).unwrap());
    fs::write(sidecar_path(&mask), to_ron(&other).unwrap()).unwrap();
    let error = map_error(tree.try_cook(Cook::Desktop).unwrap_err());
    assert!(matches!(&error, MapError::InputsWithSidecars(paths) if *paths == [mask.clone()]), "{error:?}");
    assert!(error.to_string().contains("delete those sidecars"), "{error}");
    fs::remove_file(sidecar_path(&mask)).unwrap();

    // Names that aren't images claim nothing: a document naming itself stays a source and
    // fails as a map.
    document = face_document(false);
    document.horizontal[1].path = "face.facesdf.ron".into();
    tree.write_document(&face, &document);
    let error = map_error(tree.try_cook(Cook::Desktop).unwrap_err());
    assert!(matches!(&error, MapError::UnsupportedInput(path) if *path == face), "{error:?}");
}

#[test]
fn a_content_tree_cooks_its_maps_and_leaves_their_inputs_out() {
    let tree = Tree::new("maps_tree");
    let report = tree.cook(Cook::Desktop);
    let names: Vec<String> = statuses(&report).into_iter().map(|(name, _)| name).collect();
    assert_eq!(names, ["body.ilm.ron", "body.ramp.ron", "face.facesdf.ron", "albedo.png"]);
    // Masks are inputs: no sidecars, no cooked textures of their own.
    assert!(!sidecar_path(&tree.source().join("face/masks/h_000.png")).exists());
    assert!(!tree.out(Cook::Desktop).join("face/masks").exists());
    let importer =
        |relative: &str| meta::read(&sidecar_path(&tree.source().join(relative))).unwrap().importer;
    assert_eq!(
        importer("face/face.facesdf.ron"),
        ImportSettings::CharacterMap(MapKind::FaceSdf.default_settings())
    );

    // The face map: R16 on desktop, every mask reproduced exactly at its angle.
    let (desc, level0) = texture(&tree.out(Cook::Desktop).join("face/face.facesdf.ktx2"));
    assert_eq!(
        (desc.format, desc.width, desc.height, desc.mip_level_count),
        (TextureFormat::R16Unorm, SIZE, SIZE, 7)
    );
    let values: Vec<f32> =
        level0.as_chunks::<2>().0.iter().map(|b| f32::from(u16::from_le_bytes(*b)) / 65535.0).collect();
    for angle in ANGLES {
        let wrong = (0..SIZE * SIZE)
            .filter(|&i| face_sdf::is_lit(values[i as usize], angle) != lit(angle, i % SIZE, i / SIZE))
            .count();
        assert_eq!(wrong, 0, "{angle}°");
    }
    // Between masks, the edge sweeps: halfway from 30° to 60°, the disc is halfway in radius.
    let off = (0..SIZE * SIZE)
        .filter(|&i| face_sdf::is_lit(values[i as usize], 45.0) != lit(45.0, i % SIZE, i / SIZE))
        .filter(|&i| {
            // Off by more than a texel: no neighbor across the reference edge.
            let (x, y) = (i % SIZE, i / SIZE);
            let here = lit(45.0, x, y);
            ![(x.wrapping_sub(1), y), (x + 1, y), (x, y.wrapping_sub(1)), (x, y + 1)]
                .iter()
                .any(|&(nx, ny)| nx < SIZE && ny < SIZE && lit(45.0, nx, ny) != here)
        })
        .count();
    assert_eq!(off, 0);

    // The ramp: 256 × 16 rows of 4 texels, sRGB, the stops' colors at the ends of each row.
    let (desc, level0) = texture(&tree.out(Cook::Desktop).join("body/body.ramp.ktx2"));
    assert_eq!(
        (desc.format, desc.width, desc.height, desc.mip_level_count),
        (TextureFormat::Rgba8UnormSrgb, 256, 64, 1)
    );
    let texel = |x: u32, row: u32| {
        let y = (row_center(row) * 64.0) as u32;
        let at = ((y * 256 + x) * 4) as usize;
        [level0[at], level0[at + 1], level0[at + 2], level0[at + 3]]
    };
    let id = |k| MaterialId::new(k).unwrap();
    assert_eq!(texel(0, row(id(0), Half::Day)), [0xc0, 0x70, 0x60, 0xff]);
    assert_eq!(texel(255, row(id(0), Half::Night)), [0x90, 0x80, 0xb0, 0xff]);
    assert_eq!(texel(255, row(id(1), Half::Night)), [0x30, 0x50, 0xa0, 0xff], "cloth has no night ramp");
    assert_eq!(
        texel(128, row(id(7), Half::Day)),
        texel(128, row(id(1), Half::Day)),
        "IDs past the list repeat the last"
    );

    // The ILM map: BC7 on desktop and ASTC 4×4 on mobile, like any ILM texture.
    let (desc, _) = texture(&tree.out(Cook::Desktop).join("body/body.ilm.ktx2"));
    assert_eq!((desc.format, desc.width, desc.height), (TextureFormat::Bc7RgbaUnorm, 16, 8));

    let mobile = tree.cook(Cook::Mobile);
    assert_eq!(mobile.assets.len(), 4);
    let format = |relative: &str| texture(&tree.out(Cook::Mobile).join(relative)).0.format;
    assert_eq!(format("face/face.facesdf.ktx2"), TextureFormat::R8Unorm);
    assert_eq!(format("body/body.ilm.ktx2"), TextureFormat::Astc { block: AstcBlock::B4x4, srgb: false });
    assert_eq!(format("body/body.ramp.ktx2"), TextureFormat::Rgba8UnormSrgb);
}

#[test]
fn ilm_channels_pack_in_order_with_defaults() {
    let tree = Tree::new("maps_ilm");
    let path = tree.source().join("body/body.ilm.ron");
    let bytes = fs::read(&path).unwrap();
    let map = generate(&path, &bytes, &MapKind::Ilm.default_settings()).unwrap();
    assert_eq!(map.class, TextureClass::Ilm(IlmSettings::default()));
    let pixel = |x: u32| map.image.pixels[x as usize];
    for x in [0, 7, 8, 15] {
        let spec = f32::from((x * 17) as u8) / 255.0;
        let [r, g, b, a] = pixel(x);
        assert_eq!(r, spec, "R: the specular mask");
        assert!((g - 0.5 * (1.0 - spec)).abs() < 1e-6, "G: the same mask mapped onto [0.5, 0]");
        assert_eq!(b, 0.5, "B: the constant");
        let expected = if x < 8 { 0 } else { 1 };
        assert_eq!(ilm_layout::decode_material_id(a).get(), expected, "A: the nearest level's ID");
        assert_eq!(a, ilm_layout::encode_material_id(MaterialId::new(expected).unwrap()));
    }

    // Everything left out: the neutral texel, at the size the document gives.
    let constant = tree.source().join("flat.ilm.ron");
    let empty = r#"(type: "sapphire::pipeline::IlmMap", version: 1, data: ())"#;
    tree.write_document(&constant, &IlmMap { size: Some([8, 4]), ..from_ron::<IlmMap>(empty).unwrap() });
    let map = generate(&constant, &fs::read(&constant).unwrap(), &MapKind::Ilm.default_settings()).unwrap();
    assert_eq!((map.image.width, map.image.height), (8, 4));
    assert!(map.image.pixels.iter().all(|pixel| *pixel == ilm_layout::NEUTRAL));
    let sizeless = tree.source().join("sizeless.ilm.ron");
    fs::write(&sizeless, empty).unwrap();
    let error =
        generate(&sizeless, &fs::read(&sizeless).unwrap(), &MapKind::Ilm.default_settings()).unwrap_err();
    assert!(matches!(error, MapError::Ilm(IlmError::NoSize)), "{error:?}");
}

#[test]
fn layer_masks_pack_in_order_unpainted_at_one_and_decode_srgb_inputs() {
    let tree = Tree::new("maps_layers");
    let body = tree.source().join("body");
    // A color texture: G rises across, R and B constant.
    let glow = image::RgbImage::from_fn(16, 8, |x, _| image::Rgb([40, (x * 17) as u8, 200]));
    write_png(&body.join("textures/glow.png"), glow.into());
    let path = body.join("body.layers.ron");
    let document = LayerMaskMap {
        size: None,
        matcap: ChannelInput::Image {
            path: "masks/spec.png".into(),
            channel: ImageChannel::R,
            range: [1.0, 0.0],
            encoding: Encoding::Linear,
        },
        metallic: layer_mask::unpainted(),
        emission: ChannelInput::Image {
            path: "textures/glow.png".into(),
            channel: ImageChannel::G,
            range: [0.0, 1.0],
            encoding: Encoding::Srgb,
        },
        stocking: ChannelInput::Constant(0.25),
    };
    tree.write_document(&path, &document);
    let map = generate(&path, &fs::read(&path).unwrap(), &MapKind::LayerMask.default_settings()).unwrap();
    assert_eq!(map.class, TextureClass::Mask(MaskSettings::default()));
    assert_eq!((map.image.width, map.image.height), (16, 8));
    for x in [0, 5, 15] {
        let value = f32::from((x * 17) as u8) / 255.0;
        let [r, g, b, a] = map.image.pixels[x as usize];
        assert!((r - (1.0 - value)).abs() < 1e-6, "R: the matcap's mask, a linear mask inverted");
        assert_eq!(g, 1.0, "G: metallic, unpainted");
        assert!((b - crate::color::srgb_to_linear(value)).abs() < 1e-6, "B: the glow, decoded from sRGB");
        assert_eq!(a, 0.25, "A: the constant");
    }

    // The tree cooks it as a four-channel mask, its inputs part of it.
    let report = tree.cook(Cook::Desktop);
    let names: Vec<String> = statuses(&report).into_iter().map(|(name, _)| name).collect();
    assert!(names.contains(&"body.layers.ron".to_owned()), "{names:?}");
    assert!(!names.contains(&"glow.png".to_owned()), "an input, not an asset: {names:?}");
    let (desc, _) = texture(&tree.out(Cook::Desktop).join("body/body.layers.ktx2"));
    assert_eq!(desc.format, TextureFormat::Bc7RgbaUnorm);
    tree.cook(Cook::Mobile);
    let (desc, _) = texture(&tree.out(Cook::Mobile).join("body/body.layers.ktx2"));
    assert_eq!(desc.format, TextureFormat::Astc { block: AstcBlock::B4x4, srgb: false });

    // Inputs share a size, and a document of constants gives one.
    write_png(&body.join("textures/small.png"), image::RgbImage::new(8, 8).into());
    let mismatched = LayerMaskMap {
        emission: ChannelInput::Image {
            path: "textures/small.png".into(),
            channel: ImageChannel::R,
            range: [0.0, 1.0],
            encoding: Encoding::Srgb,
        },
        ..document
    };
    tree.write_document(&path, &mismatched);
    let error =
        generate(&path, &fs::read(&path).unwrap(), &MapKind::LayerMask.default_settings()).unwrap_err();
    assert!(matches!(error, MapError::InputSize { found: [8, 8], expected: [16, 8], .. }), "{error:?}");
    let empty = r#"(type: "sapphire::pipeline::LayerMaskMap", version: 1, data: ())"#;
    fs::write(&path, empty).unwrap();
    let error =
        generate(&path, &fs::read(&path).unwrap(), &MapKind::LayerMask.default_settings()).unwrap_err();
    assert!(matches!(error, MapError::LayerMask(LayerMaskError::NoSize)), "{error:?}");
    tree.write_document(&path, &LayerMaskMap { size: Some([4, 4]), ..from_ron(empty).unwrap() });
    let map = generate(&path, &fs::read(&path).unwrap(), &MapKind::LayerMask.default_settings()).unwrap();
    assert!(map.image.pixels.iter().all(|pixel| *pixel == [1.0; 4]), "unpainted everywhere");
}

#[test]
fn keys_cover_inputs_and_settings_and_survive_moves() {
    let tree = Tree::new("maps_keys");
    let first = tree.cook(Cook::Desktop);
    assert!(statuses(&tree.cook(Cook::Desktop)).iter().all(|(_, status)| *status == CookStatus::UpToDate));
    let key = |report: &CookReport, name: &str| {
        report.assets.iter().find(|asset| asset.source.file_name().unwrap() == name).unwrap().key
    };

    // Editing a mask recooks the face map alone.
    let mask = tree.source().join("face/masks/h_060.png");
    write_png(&mask, mask_image(|x, y| lit(65.0, x, y)));
    let report = tree.cook(Cook::Desktop);
    let cooked: Vec<String> = statuses(&report)
        .into_iter()
        .filter(|(_, status)| *status == CookStatus::Cooked)
        .map(|(name, _)| name)
        .collect();
    assert_eq!(cooked, ["face.facesdf.ron"]);
    assert_ne!(key(&report, "face.facesdf.ron"), key(&first, "face.facesdf.ron"));
    write_png(&mask, mask_image(|x, y| lit(60.0, x, y)));
    assert_eq!(key(&tree.cook(Cook::Desktop), "face.facesdf.ron"), key(&first, "face.facesdf.ron"));

    // Sidecar settings are part of the key, and reach the cook.
    tree.set_settings(
        "face/face.facesdf.ron",
        MapSettings::FaceSdf(FaceSdfSettings { mips: false, precision: SdfPrecision::Unorm8 }),
    );
    let report = tree.cook(Cook::Desktop);
    assert_ne!(key(&report, "face.facesdf.ron"), key(&first, "face.facesdf.ron"));
    let (desc, _) = texture(&tree.out(Cook::Desktop).join("face/face.facesdf.ktx2"));
    assert_eq!((desc.format, desc.mip_level_count), (TextureFormat::R8Unorm, 1));
    tree.set_settings(
        "body/body.ramp.ron",
        MapSettings::Ramp(RampBakeSettings { width: 64, row_height: 2, mips: false }),
    );
    tree.cook(Cook::Desktop);
    let (desc, _) = texture(&tree.out(Cook::Desktop).join("body/body.ramp.ktx2"));
    assert_eq!((desc.width, desc.height), (64, 32));

    // A vertical channel makes a two-channel map.
    tree.add_vertical();
    assert_eq!(tree.cook(Cook::Desktop).assets.len(), 4, "the new masks are inputs too");
    let (desc, level0) = texture(&tree.out(Cook::Desktop).join("face/face.facesdf.ktx2"));
    assert_eq!(desc.format, TextureFormat::Rg8Unorm);
    // At (x, y) the vertical channel holds what the horizontal one holds at (y, x).
    for (x, y) in [(10, 40), (40, 10), (30, 31), (50, 5)] {
        let at = |x: u32, y: u32| ((y * SIZE + x) * 2) as usize;
        assert_eq!(level0[at(x, y) + 1], level0[at(y, x)], "({x}, {y})");
    }

    // Moving the face directory, document and masks together, keeps the key.
    let before = key(&tree.cook(Cook::Desktop), "face.facesdf.ron");
    fs::rename(tree.source().join("face"), tree.source().join("heroine")).unwrap();
    let report = tree.cook(Cook::Desktop);
    let moved =
        report.assets.iter().find(|asset| asset.source.ends_with("heroine/face.facesdf.ron")).unwrap();
    assert_eq!((moved.key, moved.status), (before, CookStatus::Cached));
}

#[test]
fn bad_maps_fail_with_the_document_and_the_reason() {
    let tree = Tree::new("maps_errors");
    tree.cook(Cook::Desktop);
    let face = tree.source().join("face/face.facesdf.ron");
    let map_error = |error: CookError| match error {
        CookError::Map { path, source } => {
            assert_eq!(path, face);
            *source
        }
        other => panic!("expected a map error, got {other:?}"),
    };

    // A mask that lights texels the mask before it shadows.
    write_png(
        &tree.source().join("face/masks/h_060.png"),
        mask_image(|x, y| lit(60.0, x, y) || (x >= 60 && y < 4)),
    );
    let error = map_error(tree.try_cook(Cook::Desktop).unwrap_err());
    let message = error.to_string();
    assert!(matches!(error, MapError::FaceSdf(FaceSdfError::NotMonotonic { .. })), "{error:?}");
    assert!(
        message.contains("masks/h_060.png (60°) lights 16 texel(s) that masks/h_030.png (30°) shadows, within x 60..=63, y 0..=3"),
        "{message}"
    );
    // Allowing that many per pair accepts them.
    let mut document = face_document(false);
    document.allowed_violations = 16;
    tree.write_document(&face, &document);
    tree.cook(Cook::Desktop);
    document.allowed_violations = 0;
    tree.write_document(&face, &document);
    write_png(&tree.source().join("face/masks/h_060.png"), mask_image(|x, y| lit(60.0, x, y)));

    // A mask of another size.
    write_png(&tree.source().join("face/masks/h_120.png"), image::GrayImage::new(32, 32).into());
    let error = map_error(tree.try_cook(Cook::Desktop).unwrap_err());
    assert!(matches!(&error, MapError::InputSize { found: [32, 32], expected: [64, 64], .. }), "{error:?}");

    // A missing mask is named.
    let missing = tree.source().join("face/masks/h_120.png");
    fs::remove_file(&missing).unwrap();
    let error = map_error(tree.try_cook(Cook::Desktop).unwrap_err());
    assert!(matches!(&error, MapError::MissingInput(path) if *path == missing), "{error:?}");
    // Outside the content tree, it's a read error.
    let mut outside = face_document(false);
    outside.horizontal[3].path = "../../elsewhere/h_120.png".into();
    tree.write_document(&face, &outside);
    let error = tree.try_cook(Cook::Desktop).unwrap_err();
    assert!(
        matches!(&error, CookError::Read { path, .. } if path.ends_with("elsewhere/h_120.png")),
        "{error:?}"
    );
    tree.write_document(&face, &face_document(false));

    // A document that doesn't parse.
    fs::write(&face, "(type: \"sapphire::pipeline::FaceSdfMap\", version: 1, data: (horizontal: 3))")
        .unwrap();
    let error = map_error(tree.try_cook(Cook::Desktop).unwrap_err());
    assert!(matches!(error, MapError::Document { kind: "face SDF", .. }), "{error:?}");
}

#[test]
fn previews_are_pngs_of_the_generated_values() {
    let tree = Tree::new("maps_previews");
    for (relative, kind) in
        [("face/face.facesdf.ron", MapKind::FaceSdf), ("body/body.ramp.ron", MapKind::Ramp)]
    {
        let path = tree.source().join(relative);
        let map = generate(&path, &fs::read(&path).unwrap(), &kind.default_settings()).unwrap();
        let png = image::load_from_memory(&to_png(&map).unwrap()).unwrap();
        assert_eq!((png.width(), png.height()), (map.image.width, map.image.height));
        match kind {
            MapKind::FaceSdf => {
                let preview = png.into_luma16();
                for (texel, pixel) in preview.pixels().zip(&map.image.pixels) {
                    assert_eq!(texel.0[0], (pixel[0] * 65535.0).round() as u16);
                }
            }
            _ => {
                let preview = png.into_rgba8();
                let first = map.image.pixels[0];
                let srgb = |v| crate::color::unorm8(crate::color::linear_to_srgb(v));
                assert_eq!(preview.get_pixel(0, 0).0, [srgb(first[0]), srgb(first[1]), srgb(first[2]), 255]);
            }
        }
    }
}
