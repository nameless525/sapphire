//! Map documents end to end: documents and masks in a content tree, cooked through the
//! content cache into KTX2.

use std::fs;
use std::path::{Path, PathBuf};

use sapphire_assets::character_maps::ramp::{Half, row, row_center};
use sapphire_assets::character_maps::{MaterialId, face_sdf, ilm as ilm_layout};
use sapphire_assets::texture::{AstcBlock, Texture, TextureFormat};
use sapphire_serialize::{from_ron, to_ron};

use super::face_sdf::{Edges, ThresholdMask};
use super::ilm::{ChannelInput, Encoding, ImageChannel, MaterialIdInput};
use super::ramp::{ColorStop, Evaluate, ImageRamp, Interpolation, MaterialRamps, Ramp, SrgbColor, StopRamp};
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
        edges: Edges::Binary,
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
        cook_dir(
            &self.source(),
            &self.out(cook),
            &CookOptions { cook, cache: Some(&cache), locked: false, editor: false },
        )
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
        self.set_capped(relative, settings, MaxSize::NONE);
    }

    fn set_capped(&self, relative: &str, settings: MapSettings, max_size: MaxSize) {
        let sidecar = sidecar_path(&self.source().join(relative));
        let mut meta = meta::read(&sidecar).unwrap();
        meta.importer = ImportSettings::CharacterMap(settings, max_size);
        fs::write(&sidecar, to_ron::<AssetMeta>(&meta).unwrap()).unwrap();
    }
}

fn ramp_document() -> RampMap {
    let stop = |at, color: &str| ColorStop { at, color: color.parse().unwrap(), interpolation: None };
    let ramp = |stops| Ramp::Stops(StopRamp { interpolation: None, stops });
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
    assert_eq!((face.vertical.len(), face.allowed_violations, face.edges), (0, 0, Edges::Binary));
    // Binary edges, the default, aren't written, so documents from before M6 C6 keep their bytes.
    assert!(!to_ron(&face_document(false)).unwrap().contains("edges"));
    let anti_aliased = FaceSdfMap { edges: Edges::AntiAliased, ..face_document(false) };
    let text = to_ron(&anti_aliased).unwrap();
    assert!(text.contains("edges: AntiAliased"), "{text}");
    assert_eq!(from_ron::<FaceSdfMap>(&text).unwrap(), anti_aliased);
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
    let Some(Ramp::Stops(night)) = &ramp.materials[0].night else { panic!("{ramp:?}") };
    assert_eq!(night.stops[0].color, SrgbColor([0x50, 0x40, 0x70, 0x80]));
    // Version 2 writes each ramp's kind; version 1's stops read as the same document.
    let current: RampMap = from_ron(
        r##"(type: "sapphire::pipeline::RampMap", version: 2, data: (
            materials: [(
                day: Stops((stops: [(at: 0.0, color: "#c07060"), (at: 1.0, color: "#f0b090", interpolation: Step)])),
                night: Stops((interpolation: Linear, stops: [(at: 0.5, color: "#50407080")])),
            ), (
                name: "cloth",
                day: Image((path: "ramps/cloth.png", row: 3)),
            )],
        ))"##,
    )
    .unwrap();
    assert_eq!(current.materials[0], ramp.materials[0]);
    assert_eq!(current.materials[1].day, Ramp::Image(ImageRamp { path: "ramps/cloth.png".into(), row: 3 }));
    assert_eq!(current.inputs(), ["ramps/cloth.png"]);
    assert!(ramp.inputs().is_empty(), "stops read no images");
    // An image row names its row: left out, it's an error naming the field.
    let error = from_ron::<RampMap>(
        r#"(type: "sapphire::pipeline::RampMap", version: 2, data: (materials: [(day: Image((path: "a.png")))]))"#,
    )
    .unwrap_err();
    assert!(error.to_string().contains("row"), "{error}");
    // A version 1 document can't hold an image.
    assert!(
        from_ron::<RampMap>(
            r#"(type: "sapphire::pipeline::RampMap", version: 1, data: (materials: [(day: Image((path: "a.png", row: 0)))]))"#,
        )
        .is_err()
    );
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
            Some(ImportSettings::CharacterMap(kind.default_settings(), MaxSize::NONE))
        );
        assert_eq!(kind.default_settings().kind(), kind);
    }
    // Importers that decode images carry the image importer's version: the masks', and the ramp's for
    // its painted strips (since C7's review), so a change to the decoder recooks them all.
    for (kind, version) in
        [(MapKind::FaceSdf, 1), (MapKind::Ilm, 1), (MapKind::LayerMask, 1), (MapKind::Ramp, 3)]
    {
        assert_eq!(kind.importer().version & 0xffff, image_import::IMPORTER.version, "{kind:?}");
        assert_eq!(kind.importer().version >> 16, version, "{kind:?}");
    }
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
        ImportSettings::CharacterMap(MapKind::FaceSdf.default_settings(), MaxSize::NONE)
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

/// A map's sidecar caps it per cook as a texture's does (M5 M1): the capped cook keeps the uncapped
/// chain's lower levels byte for byte, the other cook doesn't move, and only that map recooks. An
/// uncapped sidecar reads, writes and keys as it did before maps had a cap.
#[test]
fn a_map_is_capped_per_cook_as_a_texture_is() {
    let tree = Tree::new("maps_capped");
    let before = Cook::ALL.map(|cook| tree.cook(cook));
    let face = |cook: Cook| fs::read(tree.out(cook).join("face/face.facesdf.ktx2")).unwrap();
    let uncapped = Cook::ALL.map(face);

    let max_size = MaxSize { mobile: std::num::NonZeroU32::new(SIZE / 4), ..MaxSize::NONE };
    tree.set_capped("face/face.facesdf.ron", MapKind::FaceSdf.default_settings(), max_size);
    for (index, cook) in Cook::ALL.into_iter().enumerate() {
        let report = tree.cook(cook);
        for (asset, old) in report.assets.iter().zip(&before[index].assets) {
            let face = asset.source.ends_with("face.facesdf.ron");
            let status = if face { CookStatus::Cooked } else { CookStatus::UpToDate };
            assert_eq!((asset.status, asset.key != old.key), (status, face), "{cook} {:?}", asset.source);
        }
    }
    assert_eq!(face(Cook::Desktop), uncapped[0], "the desktop cook doesn't move");
    let (old, new) = (uncapped[1].clone(), face(Cook::Mobile));
    let (old, new) = (Texture::from_ktx2(&old).unwrap(), Texture::from_ktx2(&new).unwrap());
    assert_eq!((new.desc.width, new.desc.height), (SIZE / 4, SIZE / 4));
    assert_eq!(new.desc.mip_level_count, old.desc.mip_level_count - 2);
    assert_eq!(new.levels[..], old.levels[2..]);

    // A committed sidecar writes back as it was, and keys as the variant's index then its settings.
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../content/combat/lancer/lancer.ilm.ron.meta");
    let text = fs::read_to_string(&path).unwrap();
    let meta: AssetMeta = from_ron(&text).unwrap();
    let ImportSettings::CharacterMap(settings, MaxSize::NONE) = &meta.importer else { panic!("{text}") };
    assert_eq!(to_ron(&meta).unwrap(), text);
    let key = |importer: &ImportSettings| sapphire_serialize::postcard::to_allocvec(importer).unwrap();
    let mut plain = vec![3];
    plain.extend(sapphire_serialize::postcard::to_allocvec(settings).unwrap());
    assert_eq!(key(&meta.importer), plain);
    let capped = ImportSettings::CharacterMap(*settings, max_size);
    let text = to_ron(&AssetMeta::new(meta.id, capped.clone())).unwrap();
    assert!(text.contains("mobile: Some(16)"), "{text}");
    assert_eq!(from_ron::<AssetMeta>(&text).unwrap().importer, capped);
    assert_ne!(key(&capped), key(&meta.importer));
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

/// The committed painted strip, `fixtures/ramp/painted.png`: [`painted_ramps`] baked at
/// [`STRIP`] texels, a row each, as an artist would export a gradient from an image editor.
fn painted_strip_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures/ramp/painted.png")
}

/// The painted strip's width: narrower than the default bake, so a bake at that default blends
/// between its texels.
const STRIP: u32 = 32;

/// The ramps the painted strip is painted from: a three-stop skin ramp, a warm-to-cool cloth
/// ramp, bands (`Step`), and one with alpha.
fn painted_ramps() -> Vec<StopRamp> {
    let stop = |at, color: &str| ColorStop { at, color: color.parse().unwrap(), interpolation: None };
    vec![
        StopRamp {
            interpolation: None,
            stops: vec![stop(0.0, "#8a4a4f"), stop(0.55, "#c8796c"), stop(1.0, "#f2b8a0")],
        },
        StopRamp { interpolation: None, stops: vec![stop(0.1, "#e0784a"), stop(0.9, "#4a78e0")] },
        StopRamp {
            interpolation: Some(Interpolation::Step),
            stops: vec![stop(0.0, "#3a6a5a"), stop(0.5, "#6aa88a"), stop(0.8, "#b8e0c0")],
        },
        StopRamp { interpolation: None, stops: vec![stop(0.0, "#2a1a4080"), stop(1.0, "#d080c0")] },
    ]
}

/// [`painted_ramps`] as 8-bit sRGB texels at their centers, a row each: what the fixture holds.
fn painted_strip() -> image::RgbaImage {
    let ramps = painted_ramps();
    image::RgbaImage::from_fn(STRIP, ramps.len() as u32, |x, y| {
        let [r, g, b, a] = ramps[y as usize].evaluate((x as f32 + 0.5) / STRIP as f32, Interpolation::OkLab);
        let srgb = |v: f32| crate::color::unorm8(crate::color::linear_to_srgb(v));
        image::Rgba([srgb(r), srgb(g), srgb(b), crate::color::unorm8(a)])
    })
}

/// A document of one material a ramp, each ramp from `ramp(index)`.
fn ramps_document(count: usize, interpolation: Interpolation, ramp: impl Fn(usize) -> Ramp) -> RampMap {
    RampMap {
        interpolation,
        materials: (0..count)
            .map(|index| MaterialRamps { name: format!("m{index}"), day: ramp(index), night: None })
            .collect(),
    }
}

/// The texels of the day rows of IDs `0..count` of a cooked ramp map, desktop.
fn cooked_rows(document: &RampMap, width: u32, count: usize) -> Vec<Vec<[u8; 4]>> {
    let path = painted_strip_path().with_file_name("painted.ramp.ron");
    let settings = MapSettings::Ramp(RampBakeSettings { width, row_height: 1, mips: false });
    let bytes = to_ron(document).unwrap();
    let cooked = cook_source(&path, bytes.as_bytes(), &settings, MaxSize::NONE, Cook::Desktop).unwrap();
    let texture = Texture::from_ktx2(&cooked).unwrap();
    assert_eq!(texture.desc.format, TextureFormat::Rgba8UnormSrgb);
    let texels = texture.levels[0].as_chunks::<4>().0.to_vec();
    (0..count)
        .map(|k| {
            let at = (row(MaterialId::new(k as u8).unwrap(), Half::Day) * width) as usize;
            texels[at..at + width as usize].to_vec()
        })
        .collect()
}

/// Whether two rows of texels are within one code value everywhere.
fn within_one(a: &[Vec<[u8; 4]>], b: &[Vec<[u8; 4]>]) -> bool {
    a.iter().flatten().zip(b.iter().flatten()).all(|(a, b)| a.iter().zip(b).all(|(a, b)| a.abs_diff(*b) <= 1))
}

/// The fixture strip is what [`painted_strip`] makes; if this test's ramps change, save
/// `painted_strip()` over it as a PNG.
#[test]
fn the_painted_strip_fixture_is_its_stops_baked() {
    let committed = image::open(painted_strip_path()).unwrap();
    assert_eq!(committed.color(), image::ColorType::Rgba8);
    assert!(committed.into_rgba8() == painted_strip(), "the fixture's texels differ from painted_strip()'s");
}

/// A ramp painted as a strip cooks as the stops it was painted from (M6 C7): at the strip's own
/// width, within one code value of the typed ramp (each texel is that ramp's color at the texel's
/// center, rounded once); and at any width, within one code value of the strip's own texels as
/// stops at their centers, in every blend space.
#[test]
fn a_painted_strip_cooks_as_its_stops() {
    let ramps = painted_ramps();
    let count = ramps.len();
    let image = |index: usize| Ramp::Image(ImageRamp { path: "painted.png".into(), row: index as u32 });
    let painted = ramps_document(count, Interpolation::OkLab, image);
    let typed = ramps_document(count, Interpolation::OkLab, |index| Ramp::Stops(ramps[index].clone()));
    let strip = cooked_rows(&painted, STRIP, count);
    assert!(within_one(&strip, &cooked_rows(&typed, STRIP, count)), "the strip is its ramps");
    let fixture = painted_strip();
    for (y, row) in strip.iter().enumerate() {
        let expected: Vec<[u8; 4]> = (0..STRIP).map(|x| fixture.get_pixel(x, y as u32).0).collect();
        assert_eq!(*row, expected, "row {y}: at the strip's width, texel for texel");
    }

    // The strip's texels as stops at their centers, the strip's equivalent in any bake.
    let texel_stops = |index: usize| {
        let stops = (0..STRIP)
            .map(|x| {
                let [r, g, b, a] = fixture.get_pixel(x, index as u32).0;
                ColorStop {
                    at: (x as f32 + 0.5) / STRIP as f32,
                    color: SrgbColor([r, g, b, a]),
                    interpolation: None,
                }
            })
            .collect();
        Ramp::Stops(StopRamp { interpolation: None, stops })
    };
    for interpolation in
        [Interpolation::OkLab, Interpolation::Linear, Interpolation::Srgb, Interpolation::Step]
    {
        for width in [STRIP, 100, 256] {
            let painted = cooked_rows(&ramps_document(count, interpolation, image), width, count);
            let stops = cooked_rows(&ramps_document(count, interpolation, texel_stops), width, count);
            assert!(within_one(&painted, &stops), "{interpolation:?} at {width}");
            if interpolation == Interpolation::Step {
                assert_eq!(painted, stops, "bands are exact");
            }
        }
    }
    // Between texel centers the strip blends in the document's space: across the bands' first
    // edge (texels 15 and 16 of row 2), three quarters of the way, at twice the strip's width.
    let wide = cooked_rows(&ramps_document(count, Interpolation::Srgb, image), STRIP * 2, count);
    let [a, b] = [fixture.get_pixel(15, 2).0, fixture.get_pixel(16, 2).0];
    assert_ne!(a, b, "an edge");
    let expected: Vec<f32> = (0..3).map(|c| f32::from(a[c]) * 0.25 + f32::from(b[c]) * 0.75).collect();
    let found = wide[2][32];
    assert!((0..3).all(|c| (f32::from(found[c]) - expected[c]).abs() <= 1.0), "{found:?} {expected:?}");
}

/// A version 1 document and its rewrite at version 2 bake the same rows: the upgrade keeps
/// every stop, interpolation and night ramp. The swatch is written at version 2 (M6 T4), so its
/// version 1 form is made from it here.
#[test]
fn a_version_1_document_bakes_as_its_version_2_rewrite() {
    let samples = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../content/samples");
    let path = samples.join("swatch.ramp.ron");
    let rewritten = fs::read_to_string(&path).unwrap();
    assert!(rewritten.contains("version: 2") && rewritten.contains("Stops(("), "{rewritten}");
    let current = from_ron::<RampMap>(&rewritten).unwrap();
    let stops = |ramp: &Ramp| match ramp {
        Ramp::Stops(stops) => stops.clone(),
        Ramp::Image(_) => panic!("the swatch's ramps are stops"),
    };
    let materials = current.materials.iter().map(|material| ramp::v1::MaterialRamps {
        name: material.name.clone(),
        day: stops(&material.day),
        night: material.night.as_ref().map(stops),
    });
    let old =
        to_ron(&ramp::v1::RampMap { interpolation: current.interpolation, materials: materials.collect() })
            .unwrap();
    assert!(old.contains("version: 1") && !old.contains("Stops("), "{old}");
    assert_eq!(from_ron::<RampMap>(&old).unwrap(), current);
    let settings = MapKind::Ramp.default_settings();
    let bake = |text: &str| generate(&path, text.as_bytes(), &settings).unwrap();
    assert_eq!(bake(&old), bake(&rewritten));
}

/// A ramp's painted strip is an input like a mask (M6 C7): claimed out of the cook, a dependency
/// of the ramp's key and of nothing else's, and held to the bake's width and its own rows.
#[test]
fn a_ramps_painted_strip_is_its_input() {
    let tree = Tree::new("maps_ramp_strip");
    let body = tree.source().join("body");
    let strip = body.join("ramps/painted.png");
    write_png(&strip, painted_strip().into());
    let ramp_path = body.join("body.ramp.ron");
    let mut document = ramp_document();
    let image = |row| Ramp::Image(ImageRamp { path: "ramps/painted.png".into(), row });
    document.materials[1].day = image(1);
    document.materials[1].night = Some(image(3));
    tree.write_document(&ramp_path, &document);
    let first = tree.cook(Cook::Desktop);
    let names: Vec<String> = statuses(&first).into_iter().map(|(name, _)| name).collect();
    assert_eq!(names, ["body.ilm.ron", "body.ramp.ron", "face.facesdf.ron", "albedo.png"], "no strip");
    assert!(!sidecar_path(&strip).exists());
    let key = |report: &CookReport| {
        report.assets.iter().find(|asset| asset.source == ramp_path).map(|asset| asset.key).unwrap()
    };

    // The cloth's rows are the strip's, blended up to the bake's 256 texels.
    let (_, level0) = texture(&tree.out(Cook::Desktop).join("body/body.ramp.ktx2"));
    let texel = |x: u32, row: u32| {
        let at = (((row_center(row) * 64.0) as u32 * 256 + x) * 4) as usize;
        [level0[at], level0[at + 1], level0[at + 2], level0[at + 3]]
    };
    let id = |k| MaterialId::new(k).unwrap();
    let painted = painted_strip();
    assert_eq!(texel(0, row(id(1), Half::Day)), painted.get_pixel(0, 1).0, "held before the first center");
    assert_eq!(texel(255, row(id(1), Half::Night)), painted.get_pixel(STRIP - 1, 3).0);

    // Editing the strip recooks the ramp alone, and putting it back finds the first key.
    let mut edited = painted_strip();
    edited.put_pixel(5, 3, image::Rgba([255, 0, 0, 255]));
    write_png(&strip, edited.into());
    let report = tree.cook(Cook::Desktop);
    let cooked: Vec<String> = statuses(&report)
        .into_iter()
        .filter(|(_, status)| *status == CookStatus::Cooked)
        .map(|(name, _)| name)
        .collect();
    assert_eq!(cooked, ["body.ramp.ron"]);
    assert_ne!(key(&report), key(&first));
    write_png(&strip, painted_strip().into());
    assert_eq!(key(&tree.cook(Cook::Desktop)), key(&first));

    // A row the strip doesn't have, and a strip wider than the bake, are errors naming the ramp.
    let ramp_error = |document: &RampMap, settings: RampBakeSettings| {
        tree.write_document(&ramp_path, document);
        let bytes = fs::read(&ramp_path).unwrap();
        generate(&ramp_path, &bytes, &MapSettings::Ramp(settings)).unwrap_err().to_string()
    };
    let mut missing = document.clone();
    missing.materials[1].night = Some(image(4));
    assert_eq!(
        ramp_error(&missing, RampBakeSettings::default()),
        "material 1 (cloth), night ramp: `row: 4` is past the last row of \"ramps/painted.png\", which is 4 \
         texels tall"
    );
    let narrow = RampBakeSettings { width: STRIP - 1, ..RampBakeSettings::default() };
    let error = ramp_error(&document, narrow);
    assert!(
        error.starts_with(
            "material 1 (cloth), day ramp: \"ramps/painted.png\" is 32 texels wide, wider than \
             the bake's 31"
        ),
        "{error}"
    );
    // A strip that isn't there fails the cook, naming it.
    let mut absent = document.clone();
    absent.materials[0].day = Ramp::Image(ImageRamp { path: "ramps/skin.png".into(), row: 0 });
    tree.write_document(&ramp_path, &absent);
    let error = tree.try_cook(Cook::Desktop).unwrap_err();
    assert!(
        matches!(&error, CookError::Map { source, .. } if matches!(&**source, MapError::MissingInput(path) if path.ends_with("ramps/skin.png"))),
        "{error:?}"
    );
}

/// Masks anti-aliased from the test face's shapes at four times their size: each texel the share
/// of its 16 points that the shape lights, in 8 bits.
fn anti_aliased_mask(degrees: f32) -> image::DynamicImage {
    image::GrayImage::from_fn(SIZE, SIZE, |x, y| {
        let lit = (0..16u32).filter(|k| {
            let fine = |c: u32, k: u32| (f64::from(c * 4 + k) + 0.5) / 4.0 - 0.5;
            let radius = 28.0 - f64::from(degrees) * 0.2;
            (fine(x, k % 4) - 24.5).hypot(fine(y, k / 4) - 31.5) < radius
        });
        image::Luma([(lit.count() * 255 / 16) as u8])
    })
    .into()
}

#[test]
fn anti_aliased_edges_recook_the_map_and_keep_every_mask_at_its_angle() {
    let tree = Tree::new("maps_anti_aliased");
    let face = tree.source().join("face");
    for angle in ANGLES {
        write_png(&face.join(format!("masks/h_{angle:03}.png")), anti_aliased_mask(angle));
    }
    let key = |report: &CookReport| {
        report.assets.iter().find(|asset| asset.source.ends_with("face.facesdf.ron")).unwrap().key
    };
    let read = || {
        let (_, level0) = texture(&tree.out(Cook::Desktop).join("face/face.facesdf.ktx2"));
        let values: Vec<f32> =
            level0.as_chunks::<2>().0.iter().map(|b| f32::from(u16::from_le_bytes(*b)) / 65535.0).collect();
        values
    };
    let binary = key(&tree.cook(Cook::Desktop));
    let binary_values = read();
    tree.write_document(
        &face.join("face.facesdf.ron"),
        &FaceSdfMap { edges: Edges::AntiAliased, ..face_document(false) },
    );
    let report = tree.cook(Cook::Desktop);
    assert_eq!(statuses(&report).iter().filter(|(_, status)| *status == CookStatus::Cooked).count(), 1);
    assert_ne!(key(&report), binary, "the edges are in the document, so in the key");
    let values = read();
    let document = fs::read(face.join("face.facesdf.ron")).unwrap();
    let generated: Vec<f32> =
        generate(&face.join("face.facesdf.ron"), &document, &MapKind::FaceSdf.default_settings())
            .unwrap()
            .image
            .pixels
            .iter()
            .map(|pixel| pixel[0])
            .collect();
    // As generated, the same texels light at each mask's angle: the masks binarized. Cooked,
    // binary's do too, while an anti-aliased texel can turn at its mask's angle only where the
    // edge runs through it, gray in its mask.
    for angle in ANGLES {
        let mask = anti_aliased_mask(angle).to_luma8();
        for (i, &painted) in mask.as_raw().iter().enumerate() {
            let expected = painted >= 128;
            assert_eq!(face_sdf::is_lit(generated[i], angle), expected, "anti-aliased, {angle}°, {i}");
            assert_eq!(face_sdf::is_lit(binary_values[i], angle), expected, "binary, {angle}°, {i}");
            if face_sdf::is_lit(values[i], angle) != expected {
                assert!((96..160).contains(&painted), "cooked, {angle}°, {i}: {painted}");
            }
        }
    }
    // Only where between texels the values cross moves.
    assert_ne!(values, binary_values);
}

#[test]
fn the_sheet_draws_each_angle_from_both_sides_beside_its_mask() {
    use super::face_sdf::{SHEET_BACKGROUND, SHEET_GUTTER, SHEET_LIT, SHEET_SHADOW, SHEET_WRONG};
    let tree = Tree::new("maps_sheet");
    let path = tree.source().join("face/face.facesdf.ron");
    let sheet = |angles: &[f32]| {
        let bytes = fs::read(&path).unwrap();
        face_sheet(&path, &bytes, &FaceSdfSettings::default(), angles).unwrap()
    };
    let shade = |lit: bool| if lit { SHEET_LIT } else { SHEET_SHADOW };
    // Cell `column` of row `row`'s pixel `(x, y)`.
    let at = |sheet: &image::RgbImage, row: u32, column: u32, x: u32, y: u32| {
        sheet
            .get_pixel(
                SHEET_GUTTER + column * (SIZE + SHEET_GUTTER) + x,
                SHEET_GUTTER + row * (SIZE + SHEET_GUTTER) + y,
            )
            .0
    };

    // 30° has a mask; 45° doesn't.
    let drawn = sheet(&[30.0, 45.0]);
    assert_eq!(drawn.dimensions(), (3 * SIZE + 4 * SHEET_GUTTER, 2 * SIZE + 3 * SHEET_GUTTER));
    assert_eq!(drawn.get_pixel(0, 0).0, SHEET_BACKGROUND, "the gutters");
    for (x, y) in (0..SIZE).flat_map(|y| (0..SIZE).map(move |x| (x, y))) {
        // From the head's right; from its left, the map read mirrored; the mask, reproduced.
        assert_eq!(at(&drawn, 0, 0, x, y), shade(lit(30.0, x, y)), "({x}, {y})");
        assert_eq!(at(&drawn, 0, 1, x, y), shade(lit(30.0, SIZE - 1 - x, y)), "({x}, {y}) mirrored");
        assert_eq!(at(&drawn, 0, 2, x, y), shade(lit(30.0, x, y)), "({x}, {y}) the mask");
        assert_eq!(at(&drawn, 1, 2, x, y), SHEET_BACKGROUND, "no mask at 45°");
    }

    // A vertical channel adds its rows, lit from above only.
    tree.add_vertical();
    let drawn = sheet(&[60.0]);
    assert_eq!(drawn.dimensions(), (3 * SIZE + 4 * SHEET_GUTTER, 2 * SIZE + 3 * SHEET_GUTTER));
    assert_eq!(at(&drawn, 1, 0, 0, 40), shade(lit(60.0, 40, 0)), "the vertical masks are transposed");
    assert_eq!(at(&drawn, 1, 1, 0, 40), SHEET_BACKGROUND);
    assert_eq!(at(&drawn, 1, 2, 0, 40), shade(lit(60.0, 40, 0)));

    // A mask's allowed violations are drawn in where the map can't reproduce them.
    tree.write_document(&path, &FaceSdfMap { allowed_violations: 16, ..face_document(false) });
    write_png(
        &tree.source().join("face/masks/h_060.png"),
        mask_image(|x, y| lit(60.0, x, y) || (x >= 60 && y < 4)),
    );
    let drawn = sheet(&[60.0]);
    let wrong: Vec<(u32, u32)> = (0..SIZE)
        .flat_map(|y| (0..SIZE).map(move |x| (x, y)))
        .filter(|&(x, y)| at(&drawn, 0, 2, x, y) == SHEET_WRONG)
        .collect();
    assert_eq!(wrong.len(), 16);
    assert!(wrong.iter().all(|&(x, y)| x >= 60 && y < 4), "{wrong:?}");
}
