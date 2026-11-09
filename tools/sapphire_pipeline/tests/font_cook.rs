//! The shipped fonts down the whole road: the tree's sources and sidecars through a locked cook into
//! a pack, out of an asset server by way of the chains, and into the shaper. What comes out shapes
//! every string of the test menu as the whole fonts do, with nothing missing.

#![allow(clippy::print_stdout, reason = "the subsets' sizes are what this test reports")]

#[path = "../../../crates/sapphire_text/tests/common/mod.rs"]
mod common;

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use common::{DELA, MPLUS1, OXANIUM, menu_screen};
use sapphire_assets::font::{FONTS_PACK, Font, FontChains};
use sapphire_assets::{AssetBudgets, AssetId, AssetServer, Handle, MemoryPackSource};
use sapphire_pipeline::cook::{Cook, CookOptions, cook_dir};
use sapphire_pipeline::meta;
use sapphire_pipeline::pack::{NOTICES, pack_cooked};
use sapphire_text::{FaceOptions, TextStyle, TextSystem, family_list};

fn repo_content() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../content")
}

/// A folder of `content/` holding just the fonts, as committed, so a test cooks nothing else.
fn font_tree(name: &str) -> PathBuf {
    let root = Path::new(env!("CARGO_TARGET_TMPDIR")).join(name);
    let _ = fs::remove_dir_all(&root);
    let content = root.join("content");
    let copy = |relative: String| {
        let to = content.join(&relative);
        fs::create_dir_all(to.parent().unwrap()).unwrap();
        fs::copy(repo_content().join(&relative), &to).unwrap_or_else(|error| panic!("{relative}: {error}"));
    };
    for font in [MPLUS1, OXANIUM, DELA] {
        copy(format!("third_party/{font}"));
        copy(format!("third_party/{font}.meta"));
        let folder = Path::new(font).parent().unwrap().to_str().unwrap();
        copy(format!("third_party/{folder}/OFL.txt"));
    }
    for file in [
        "ui.charset.ron",
        "ui.fontchains.ron",
        "ui.fontchains.ron.meta",
        "display_latin.charset.ron",
        "display_ja.charset.ron",
    ] {
        copy(format!("fonts/{file}"));
    }
    root
}

/// A style's family list for `ids`, registering each font not yet registered, from the server.
fn load_fonts(
    server: &mut AssetServer,
    ids: &[AssetId],
    text: &mut TextSystem,
    registered: &mut BTreeMap<AssetId, String>,
) -> String {
    let mut families = Vec::new();
    for &id in ids {
        let family = registered.entry(id).or_insert_with(|| {
            let handle: Handle<Font> = server.load(id).unwrap();
            server.finish(&handle, |upload| panic!("{} was uploaded", upload.kind())).unwrap();
            let font = server.get(&handle).expect("the font loaded");
            let options =
                FaceOptions { baseline_shift: font.baseline_shift.to_native(), ..FaceOptions::default() };
            text.register_face(Arc::new(font.file.to_vec()), options);
            font.family.to_string()
        });
        families.push(family.clone());
    }
    family_list(families.iter().map(String::as_str))
}

#[test]
fn the_shipped_fonts_shape_the_menu_as_the_whole_fonts_do() {
    let root = font_tree("font_cook_shipped");
    let options = CookOptions { cook: Cook::Mobile, cache: None, locked: true };
    let report = cook_dir(&root.join("content"), &root.join("loose"), &options).expect("a locked cook");
    assert_eq!(report.assets.len(), 4, "three fonts and the chains");
    let packs = root.join("packs");
    let packed = pack_cooked(&report, &packs, None).unwrap();
    assert_eq!(packed.packs.len(), 1);
    assert_eq!(packed.packs[0].name, FONTS_PACK);
    println!("fonts.spack: {} bytes", packed.packs[0].bytes);
    for asset in &report.assets {
        let bytes = fs::metadata(&asset.output).unwrap().len();
        println!("{}: {bytes} bytes", asset.output.file_name().unwrap().to_string_lossy());
    }

    // OFL condition 2: every family's license beside the packs, whole.
    let notices = fs::read_to_string(packs.join(NOTICES)).unwrap();
    for (family, copyright) in [
        ("M PLUS 1", "The M+ FONTS Project Authors"),
        ("Oxanium", "The Oxanium Project Authors"),
        ("Dela Gothic One", "The Dela Gothic Project Authors"),
    ] {
        assert!(notices.contains(family) && notices.contains(copyright), "{family}");
    }
    assert_eq!(notices.matches("SIL OPEN FONT LICENSE Version 1.1").count(), 3);

    let source = Arc::new(MemoryPackSource::new());
    source.insert(FONTS_PACK, fs::read(packs.join("fonts.spack")).unwrap());
    let mut server = AssetServer::new(source, AssetBudgets::MOBILE);
    server.mount_now(FONTS_PACK).unwrap();
    let chains_id = meta::read(&repo_content().join("fonts/ui.fontchains.ron.meta")).unwrap().id;
    let chains: Handle<FontChains> = server.load(chains_id).unwrap();
    server.finish(&chains, |upload| panic!("{} was uploaded", upload.kind())).unwrap();
    let wanted: Vec<(&str, Vec<AssetId>)> = {
        let read = server.get(&chains).expect("the chains loaded");
        ["body", "display"]
            .into_iter()
            .map(|chain| (chain, read.fonts(chain).expect("a chain").to_vec()))
            .collect()
    };

    let mut cut = TextSystem::new();
    let mut registered = BTreeMap::new();
    let mut lists = BTreeMap::new();
    for (chain, ids) in &wanted {
        lists.insert(*chain, load_fonts(&mut server, ids, &mut cut, &mut registered));
    }
    assert_eq!(lists["body"], "'M PLUS 1'");
    assert_eq!(lists["display"], "'Oxanium', 'Dela Gothic One', 'M PLUS 1'");

    // The whole fonts, with what the cook says about them. A family's name is what the shaper finds it
    // by, so a cooked font's must be what the whole file says.
    let dir = common::fonts_dir();
    let mut whole = TextSystem::new();
    let shifted = cut_shift(&mut server, &registered, "Oxanium");
    for (font, baseline_shift) in [(MPLUS1, 0.0), (OXANIUM, shifted), (DELA, 0.0)] {
        whole.register_face(
            common::read_font(&dir, font),
            FaceOptions { baseline_shift, ..FaceOptions::default() },
        );
    }
    let (mut names_cut, mut names_whole) = (cut.family_names(), whole.family_names());
    names_cut.sort();
    names_whole.sort();
    assert_eq!(names_cut, names_whole);

    let mut glyphs = 0;
    for japanese in [true, false] {
        for item in menu_screen(japanese) {
            for scale in [1.0, 1.5] {
                let (a, b) = (
                    whole.shape(item.text, &item.style, scale, item.max_width).clone(),
                    cut.shape(item.text, &item.style, scale, item.max_width).clone(),
                );
                glyphs += b.glyphs.len();
                let positions = |text: &sapphire_text::ShapedText| {
                    text.glyphs.iter().map(|glyph| (glyph.x, glyph.y)).collect::<Vec<_>>()
                };
                assert!(b.glyphs.iter().all(|glyph| glyph.id != 0), "missing from a subset: {}", item.text);
                assert_eq!(positions(&a), positions(&b), "{}", item.text);
                assert_eq!(a.lines, b.lines, "{}", item.text);
                let coords = |text: &sapphire_text::ShapedText| {
                    text.runs.iter().map(|run| run.coords.clone()).collect::<Vec<_>>()
                };
                assert_eq!(coords(&a), coords(&b), "the axes survive: {}", item.text);
            }
        }
    }
    println!("{glyphs} glyphs shaped from the cut fonts, none missing, all where the whole fonts put them");

    // Oxanium's shift puts a Latin title on the baseline of a body line in the same box.
    let size = 32.0;
    let style = |families| TextStyle { families, size, weight: 700.0, line_height: 1.3, locale: Some("en") };
    let display = cut.shape("SETTINGS", &style(&lists["display"]), 1.0, None).baselines[0];
    let body = cut.shape("SETTINGS", &style(&lists["body"]), 1.0, None).baselines[0];
    assert!((display - body).abs() <= 1.0, "Oxanium {display}, M PLUS 1 {body}");

    // A display face's field is made from its cut, at the weight its sidecar names: every display
    // character, shaped in the face alone at that weight, lands on a glyph the field holds, at the
    // field's own variation, which is what sends a large run to it.
    for (family, characters, weight, locale) in [
        ("Oxanium", "display_latin.charset.ron", 700.0, "en"),
        ("Dela Gothic One", "display_ja.charset.ron", 400.0, "ja"),
    ] {
        let (&id, _) = registered.iter().find(|(_, name)| name.as_str() == family).expect("the family");
        let handle: Handle<Font> = server.load(id).unwrap();
        let font = server.get(&handle).expect("loaded");
        let field = font.field.as_ref().unwrap_or_else(|| panic!("{family} has no display field"));
        let held: BTreeSet<u32> = field.glyphs.iter().map(|glyph| glyph.id.to_native()).collect();
        let document = fs::read_to_string(repo_content().join("fonts").join(characters)).unwrap();
        let shown = sapphire_pipeline::font::Characters::from_document(&document).unwrap().to_text();
        let mut alone = TextSystem::new();
        alone.register_font(Arc::new(font.file.to_vec()));
        let families = family_list([family]);
        let style =
            TextStyle { families: &families, size: 48.0, weight, line_height: 1.0, locale: Some(locale) };
        let shaped = alone.shape(&shown, &style, 1.0, None);
        let missing: Vec<u32> =
            shaped.glyphs.iter().map(|glyph| glyph.id).filter(|id| !held.contains(id)).collect();
        assert!(missing.is_empty(), "{family}: glyphs shaped from its cut that its field lacks: {missing:?}");
        let field_coords: Vec<i16> = field.coords.iter().map(|coord| coord.to_native()).collect();
        for run in &shaped.runs {
            assert_eq!(
                run.coords, field_coords,
                "{family}: the shaper's instance at {weight} is the field's"
            );
        }
        println!(
            "{family}'s field: {} glyphs in {} × {}, {} bytes of texels",
            field.glyphs.len(),
            field.width.to_native(),
            field.height.to_native(),
            field.texels.len(),
        );
    }
}

/// What the cook said a font's shift is, by the family it says it's in.
fn cut_shift(server: &mut AssetServer, registered: &BTreeMap<AssetId, String>, family: &str) -> f32 {
    let (&id, _) = registered.iter().find(|(_, name)| name.as_str() == family).expect("the family");
    let handle: Handle<Font> = server.load(id).unwrap();
    server.get(&handle).expect("loaded").baseline_shift.to_native()
}

/// A font that isn't in a chain still cooks, and one whose character set can't be read fails by name:
/// nothing about the tree's shape is written in code.
#[test]
fn a_new_font_needs_only_its_sidecar() {
    let root = font_tree("font_cook_new");
    let content = root.join("content");
    let from = repo_content().join("third_party/oxanium/Oxanium[wght].ttf");
    fs::create_dir_all(content.join("third_party/another")).unwrap();
    fs::copy(&from, content.join("third_party/another/Another.ttf")).unwrap();
    fs::copy(repo_content().join("third_party/oxanium/OFL.txt"), content.join("third_party/another/OFL.txt"))
        .unwrap();
    let sidecar = "(type: \"sapphire::assets::AssetMeta\", version: 1, data: (id: \"11111111-2222-4333-8444-555555555555\", importer: Font((license: \"OFL.txt\", subsets: [(pack: \"fonts\", characters: \"../../fonts/ui.charset.ron\")]))))";
    fs::write(content.join("third_party/another/Another.ttf.meta"), sidecar).unwrap();
    let options = CookOptions { cook: Cook::Desktop, cache: None, locked: true };
    let report = cook_dir(&content, &root.join("loose"), &options).expect("a cook");
    assert_eq!(report.assets.len(), 5, "found through its sidecar");
}

/// A zone's subset can add to the base's: registered together, two cuts of one family are one family
/// to the shaper, which finds each character in whichever cut has it.
#[test]
fn two_subsets_of_one_family_answer_together() {
    let whole = common::read_font(&common::fonts_dir(), OXANIUM);
    let cut = |characters: &str| sapphire_pipeline::font_subset::subset(&whole, characters).unwrap();
    let mut text = TextSystem::new();
    text.register_font(Arc::new(cut("ABC")));
    text.register_font(Arc::new(cut("xyz")));
    assert_eq!(text.family_names(), ["Oxanium"]);
    let style = TextStyle { families: "Oxanium", size: 20.0, weight: 400.0, line_height: 1.3, locale: None };
    for sample in ["Ax", "xA", "AxBy"] {
        let shaped = text.shape(sample, &style, 1.0, None);
        assert!(shaped.glyphs.iter().all(|glyph| glyph.id != 0), "{sample}: {:?}", shaped.glyphs);
        let fonts: BTreeSet<u64> = shaped.runs.iter().map(|run| run.font.data.id()).collect();
        assert_eq!(fonts.len(), 2, "{sample}: a run from each cut");
    }
    // Past both, it's missing, and nothing else answers.
    let shaped = text.shape("q", &style, 1.0, None);
    assert!(shaped.glyphs.iter().all(|glyph| glyph.id == 0), "{:?}", shaped.glyphs);
}
