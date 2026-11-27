use std::fs;
use std::path::Path;
use std::sync::Arc;

use sapphire_assets::effect_sheet::{ArchivedEffectSheet, EffectSheet, LAYER_BYTES, sheet_format};
use sapphire_assets::effects::Effects;
use sapphire_assets::{AssetBudgets, AssetId, AssetServer, Handle, MemoryPackSource};

use super::*;
use crate::cook::{CookError, CookOptions, cook_dir};
use crate::meta::{self, ImportSettings};
use crate::pack::{self, pack_cooked};
use crate::test_util::TempDir;

/// A placeholder sheet's document and painting, as the generator makes them.
fn placeholder(name: &str) -> (EffectSheetDocument, Vec<u8>) {
    let made = crate::generators::made(paint::outputs(Path::new(""))).expect("painted");
    let file = |suffix: &str| {
        let wanted = format!("sheets/{name}{suffix}");
        made.iter().find(|(file, _)| *file == wanted).map(|(_, bytes)| bytes.clone()).expect("generated")
    };
    (EffectSheetDocument::read(&file(SOURCE_SUFFIX)).expect("a valid document"), file(IMAGE_SUFFIX))
}

fn document() -> EffectSheetDocument {
    EffectSheetDocument {
        image: "puff.sheet.png".to_owned(),
        columns: 4,
        rows: 2,
        frames: 6,
        timing: SheetTiming::PerSecond { rate: 12.0, looped: false },
    }
}

/// The field an edit of a valid document makes it name.
fn field_of(edit: impl FnOnce(&mut EffectSheetDocument)) -> &'static str {
    let mut bad = document();
    edit(&mut bad);
    bad.problem().expect_err("a refusal").0
}

#[test]
fn every_bad_field_is_named() {
    assert_eq!(document().problem(), Ok(()));
    assert_eq!(
        field_of(|d| d.image = "puff.png".to_owned()),
        "image",
        "a painting a cook would take as a texture"
    );
    assert_eq!(field_of(|d| d.image = "art/puff.sheet.png".to_owned()), "image", "beside the document");
    assert_eq!(field_of(|d| d.image = ".sheet.png".to_owned()), "image");
    assert_eq!(field_of(|d| d.columns = 0), "columns");
    assert_eq!(field_of(|d| d.columns = 9), "columns");
    assert_eq!(field_of(|d| d.rows = 0), "rows");
    assert_eq!(field_of(|d| d.frames = 0), "frames");
    assert_eq!(field_of(|d| d.frames = 9), "frames");
    let still = SheetTiming::PerSecond { rate: 0.0, looped: true };
    assert_eq!(field_of(|d| d.timing = still), "timing");
    let endless = SheetTiming::PerSecond { rate: f32::INFINITY, looped: true };
    assert_eq!(field_of(|d| d.timing = endless), "timing");
    let text = "(type: \"sapphire::pipeline::EffectSheetDocument\", version: 1, data: (image: \"a.sheet.png\", \
                columns: 2, rows: 2, frames: 5, timing: Life))";
    let error = EffectSheetDocument::read(text.as_bytes()).expect_err("too many frames").to_string();
    assert!(error.contains("`frames`"), "{error}");
    let unknown = text.replace("timing: Life", "timing: Life, fps: 12");
    let error = EffectSheetDocument::read(unknown.as_bytes()).expect_err("an unknown field").to_string();
    assert!(error.contains("fps"), "{error}");
}

/// A sheet cooks to the same bytes twice and on one thread or eight, on both cooks: layers of 256² with their
/// whole chains, in the cook's format, each frame its own cell of the painting.
#[test]
fn a_sheet_cooks_the_same_bytes_twice_and_on_one_thread_or_eight() {
    let (document, painting) = placeholder("smoke_puff");
    for cook in [Cook::Desktop, Cook::Mobile] {
        let one = cook_on(&document, &painting, cook, 1).expect("cooks");
        assert_eq!(cook_on(&document, &painting, cook, 1).unwrap(), one, "{cook:?}: twice");
        assert_eq!(cook_on(&document, &painting, cook, 8).unwrap(), one, "{cook:?}: on eight threads");
        assert_eq!(
            cook_on(&document, &painting, cook, 64).unwrap(),
            one,
            "{cook:?}: on more than its frames"
        );
        let mut aligned: sapphire_serialize::cooked::AlignedVec =
            sapphire_serialize::cooked::AlignedVec::new();
        aligned.extend_from_slice(&one);
        let sheet: &ArchivedEffectSheet = EffectSheet::read(&aligned).expect("reads");
        assert_eq!(sheet.frames(), 16);
        assert_eq!(sheet.format(), sheet_format(cook == Cook::Mobile));
        assert_eq!(sheet.data.len() as u64, 16 * LAYER_BYTES);
        assert_eq!(sheet.timing(), SheetTiming::Life);
        // Frames differ: each is its own cell.
        assert_ne!(sheet.level(0, 0), sheet.level(15, 0), "{cook:?}");
    }
}

/// The painting must be grey and alpha, and its grid's size.
#[test]
fn a_painting_of_the_wrong_size_or_channels_is_refused() {
    let (document, painting) = placeholder("spark_burst");
    let small = EffectSheetDocument { columns: 2, rows: 2, frames: 4, ..document.clone() };
    let error = cook_on(&small, &painting, Cook::Desktop, 1).expect_err("1024 for a 2x2 grid");
    assert!(
        matches!(error, SheetCookError::ImageSize { width: 1024, height: 1024, columns: 2, rows: 2 }),
        "{error}"
    );
    let rgba = ::image::RgbaImage::from_pixel(1024, 1024, ::image::Rgba([1, 2, 3, 4]));
    let mut bytes = std::io::Cursor::new(Vec::new());
    rgba.write_to(&mut bytes, ::image::ImageFormat::Png).unwrap();
    let error = cook_on(&document, bytes.get_ref(), Cook::Desktop, 1).expect_err("colour");
    assert!(matches!(error, SheetCookError::Channels(ChannelLayout::Rgba)), "{error}");
}

/// Every placeholder frame paints something, and each sheet's largest frame a real share of its cell (2%,
/// 1,311 texels), so a painter that shrinks its frames to specks fails. Each frame's marks stay inside it as
/// painted, before the border's fade: nothing within six texels of its border, where the fade begins to cut,
/// so no mark is clipped at its quad's edge. The painting keeps to few values, steps and their soft edges.
#[test]
fn every_placeholder_frame_is_painted_inside_its_frame() {
    let side = LAYER_SIZE as usize;
    for name in paint::names() {
        let mut largest = 0;
        for (frame, coverage) in paint::coverage(name).into_iter().enumerate() {
            let covered = coverage.iter().filter(|&&coverage| coverage >= 0.5).count();
            assert!(covered > 0, "{name} frame {frame}: nothing painted");
            largest = largest.max(covered);
            let overrun = (0..side * side)
                .filter(|&i| {
                    let (x, y) = (i % side, i / side);
                    coverage[i] > 0.0 && x.min(y).min(side - 1 - x).min(side - 1 - y) < 6
                })
                .count();
            assert_eq!(overrun, 0, "{name} frame {frame}: painted within six texels of its border");
        }
        assert!(largest * 50 >= side * side, "{name}: its largest frame covers only {largest} texels");

        let (_, painting) = placeholder(name);
        let image = ::image::load_from_memory(&painting).expect("a PNG").into_luma_alpha8();
        assert_eq!(image.dimensions(), (4 * LAYER_SIZE, 4 * LAYER_SIZE), "{name}");
        let values: std::collections::BTreeSet<u8> =
            image.pixels().filter(|pixel| pixel.0[1] == 255).map(|pixel| pixel.0[0]).collect();
        assert!(values.len() <= 160, "{name}: {} values, not steps", values.len());
    }
}

/// The cook spreads a shape's value into the empty texels about it before its levels are made, so a lower
/// level's rim keeps the shape's value rather than darkening toward the empty texels' 0: at level 2 (64²),
/// every texel with any coverage on a disc's edge is the disc's value, on both cooks.
#[test]
fn a_frame_s_lower_levels_keep_its_value_to_the_edge() {
    let side = LAYER_SIZE as usize;
    let pixels = (0..side * side)
        .map(|i| {
            let (x, y) = ((i % side) as f32 - 127.5, (i / side) as f32 - 127.5);
            if x * x + y * y < 70.0 * 70.0 { [0.66, 0.66, 0.66, 1.0] } else { [0.0, 0.0, 0.0, 0.0] }
        })
        .collect();
    let mut frame = SourceImage::new(LAYER_SIZE, LAYER_SIZE, ChannelLayout::GrayAlpha, pixels).unwrap();
    dilate(&mut frame);
    assert!(frame.pixels.iter().all(|pixel| pixel[0] == 0.66), "every empty texel takes the disc's value");
    assert!(
        frame.pixels.iter().filter(|pixel| pixel[3] == 0.0).count() > side * side / 2,
        "coverage untouched"
    );
    for cook in [Cook::Desktop, Cook::Mobile] {
        let prepared = texture::prepare(&frame, &frame_settings(), cook).unwrap();
        let level = &prepared.levels[2];
        let rim: Vec<[f32; 4]> = level
            .pixels
            .iter()
            .copied()
            .filter(|&[_, coverage, ..]| coverage > 0.0 && coverage < 1.0)
            .collect();
        assert!(!rim.is_empty(), "{cook:?}: a level-2 rim");
        assert!(rim.iter().all(|&[value, ..]| (value - 0.66).abs() < 1e-6), "{cook:?}: {rim:?}");
    }
}

/// The committed placeholders are the generator's, byte for byte: the painting is integer hashes, the four
/// operations, `sqrt` and `sapphire_math::det`, so it's the same on every machine, and CI holds that on
/// Linux and Windows as well as here.
#[test]
fn the_committed_sheets_are_what_the_generator_paints() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../content/effects");
    let made = crate::generators::made(paint::outputs(&dir)).expect("painted");
    assert_eq!(made.len(), 10);
    for (name, bytes) in made {
        let committed = fs::read(dir.join(&name)).unwrap_or_else(|error| panic!("{name}: {error}"));
        assert!(committed == bytes, "{name} differs from what gen-effects paints: run it with --force");
    }
}

/// A library naming a sheet cooks beside it in a content tree: the sheet gets its sidecar and its painting
/// none, the library lists the sheet and depends on it, and both load through the asset server, the sheet
/// first. A library naming what isn't a sheet fails its cook, naming the emitter.
#[test]
fn a_library_and_its_sheet_cook_pack_and_load() {
    let dir = TempDir::new("effect_sheet_library");
    let content = dir.path().join("content");
    fs::create_dir_all(content.join("sheets")).unwrap();
    let made = crate::generators::made(paint::outputs(&content)).expect("painted");
    for (name, bytes) in made.iter().filter(|(name, _)| name.contains("flame")) {
        fs::write(content.join(name), bytes).unwrap();
    }
    let options = CookOptions { cook: Cook::Mobile, cache: None, locked: false, editor: false };
    // A first cook writes the sheet's sidecar, whose ID the library names.
    cook_dir(&content, &dir.path().join("first"), &options).expect("the sheet cooks alone");
    assert!(
        !meta::sidecar_path(&content.join("sheets/flame.sheet.png")).exists(),
        "the painting is no asset"
    );
    let sidecar =
        meta::read(&meta::sidecar_path(&content.join("sheets/flame.sheet.ron"))).expect("a sidecar");
    assert_eq!(sidecar.importer, ImportSettings::EffectSheet(EffectSheetSettings::default()));
    let library = |sheet: AssetId| {
        format!(
            "(type: \"sapphire::vfx::EffectLibrary\", version: 5, data: (effects: [(name: \"fire\", emitters: [(\
             name: \"tongues\", spawn: Burst(count: 4), lifetime: (0.5, 1.0), size: [(0.0, 0.2)], \
             color: [(0.0, (2.0, 1.0, 0.4))], sheet: Some(\"{sheet}\"))])]))"
        )
    };
    fs::write(content.join("fire.effects.ron"), library(sidecar.id)).unwrap();
    let report = cook_dir(&content, &dir.path().join("loose"), &options).expect("cooks");
    let library_id = meta::read(&meta::sidecar_path(&content.join("fire.effects.ron"))).unwrap().id;

    let packs = dir.path().join("packs");
    pack_cooked(&report, &packs, None).unwrap();
    let source = Arc::new(MemoryPackSource::new());
    source.insert(pack::SHARED, fs::read(packs.join("shared.spack")).unwrap());
    let mut server = AssetServer::new(source, AssetBudgets::MOBILE);
    server.mount_now(pack::SHARED).unwrap();
    let handle: Handle<Effects> = server.load(library_id).unwrap();
    server.finish(&handle, |upload| panic!("{} was sent to the uploader", upload.kind())).unwrap();
    assert_eq!(server.get(&handle).expect("loaded").sheets(), [sidecar.id]);
    let sheet: Handle<EffectSheet> = server.load(sidecar.id).unwrap();
    server.finish(&sheet, |upload| panic!("{} was sent to the uploader", upload.kind())).unwrap();
    let sheet = server.get(&sheet).expect("loaded with its library");
    assert_eq!(sheet.timing(), SheetTiming::PerSecond { rate: 12.0, looped: true });
    assert_eq!(sheet.format(), sheet_format(true));

    let stranger = AssetId::from_bytes([7; 16]);
    fs::write(content.join("fire.effects.ron"), library(stranger)).unwrap();
    let error = cook_dir(&content, &dir.path().join("again"), &options).expect_err("no such sheet");
    let found = std::iter::successors(Some(&error as &dyn std::error::Error), |error| error.source())
        .find_map(|error| error.downcast_ref::<CookError>())
        .map(ToString::to_string)
        .unwrap_or_default();
    assert!(found.contains("fire/tongues") && found.contains(&stranger.to_string()), "{error}");
}
