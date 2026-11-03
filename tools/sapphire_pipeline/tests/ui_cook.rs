//! Screens through a whole cook: a document's own checks, its paths against its model, and what its
//! keys say to the fonts. A tree of its own with only Oxanium keeps most of these quick, and the last
//! cooks the test screen as committed, in both languages.

use std::error::Error;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use sapphire_assets::font::Font;
use sapphire_assets::ui::UI_PACK;
use sapphire_pipeline::cook::{Cook, CookError, CookOptions, CookReport, CookStatus, cook_dir};
use sapphire_pipeline::pack::pack_cooked;
use sapphire_serialize::cooked::AlignedVec;
use sapphire_text::{TextStyle, TextSystem};

const OXANIUM: &str = "third_party/oxanium/Oxanium[wght].ttf";
/// In Oxanium, and not in `ui.charset.ron`: it can only be in a subset because a screen shows it.
const ONLY_FROM_A_SCREEN: char = '\u{100}';
const ANOTHER_FROM_A_SCREEN: char = '\u{101}';
/// In no font of the tree.
const IN_NO_FONT: char = '\u{d55c}';

fn repo_content() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../content")
}

fn sidecar(id: &str, importer: &str) -> String {
    format!(
        "(type: \"sapphire::assets::AssetMeta\", version: 1, data: (id: \"{id}\", importer: {importer}))\n"
    )
}

fn write(path: &Path, text: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, text).unwrap();
}

fn model_file(paths: &[&str]) -> String {
    let fields: Vec<_> = paths.iter().map(|path| format!("(path: \"{path}\", kind: Number)")).collect();
    format!("(type: \"sapphire::assets::UiModel\", version: 1, data: (fields: [{}]))", fields.join(", "))
}

/// A document with one text node of `key`, in the role `role`, passing `args`.
fn model_file_of(fields: &[(&str, &str)]) -> String {
    let fields: Vec<String> =
        fields.iter().map(|(path, kind)| format!("(path: \"{path}\", kind: {kind})")).collect();
    format!("(type: \"sapphire::assets::UiModel\", version: 1, data: (fields: [{}]))", fields.join(", "))
}

fn screen_file(key: &str, role: &str, args: &str) -> String {
    format!(
        "(type: \"sapphire::assets::UiDocument\", version: 2, data: (
            model: \"a.model.ron\",
            styles: [(name: \"label\", role: \"{role}\", size: 16)],
            nodes: [
                (name: \"root\"),
                (name: \"line\", parent: \"root\", text: (key: \"{key}\", style: \"label\", color: (255, 255, 255, 255), args: [{args}])),
            ],
        ))"
    )
}

fn locale_file(fallback: &str, families: &str) -> String {
    format!(
        "(type: \"sapphire::strings::LocaleDocument\", version: 1, data: (
            fallback: [{fallback}],
            fonts: {{\"display\": [{families}]}},
        ))"
    )
}

struct Tree {
    root: PathBuf,
}

impl Tree {
    /// Oxanium, its chain, English and one screen, whose only key is `greeting`.
    fn new(name: &str) -> Self {
        let root = Path::new(env!("CARGO_TARGET_TMPDIR")).join(name);
        let _ = fs::remove_dir_all(&root);
        let tree = Self { root };
        let content = tree.content();
        for file in [OXANIUM.to_owned(), format!("{OXANIUM}.meta"), "fonts/ui.charset.ron".to_owned()] {
            fs::create_dir_all(content.join(&file).parent().unwrap()).unwrap();
            fs::copy(repo_content().join(&file), content.join(&file)).unwrap();
        }
        fs::copy(
            repo_content().join("third_party/oxanium/OFL.txt"),
            content.join("third_party/oxanium/OFL.txt"),
        )
        .unwrap();
        tree.chains("(name: \"display\", fonts: [\"ce9360bf-ac52-4ecc-9e73-75cd14b619d5\"])");
        write(
            &content.join("fonts/a.fontchains.ron.meta"),
            &sidecar("00000000-0000-4000-8000-000000000001", "FontChains(())"),
        );
        tree.locale("en", "", "\"Oxanium\"");
        write(
            &content.join("loc/en.locale.ron.meta"),
            &sidecar("00000000-0000-4000-8000-000000000002", "Locale(())"),
        );
        tree.strings("en", "greeting = Hello\n");
        write(
            &content.join("loc/en/a.ftl.meta"),
            &sidecar("00000000-0000-4000-8000-000000000003", "Fluent((locale: \"en\"))"),
        );
        write(&content.join("ui/a.model.ron"), &model_file(&["level"]));
        tree.screen(&screen_file("greeting", "display", ""));
        write(&content.join("ui/a.ui.ron.meta"), &sidecar("00000000-0000-4000-8000-000000000004", "Ui(())"));
        tree
    }

    fn content(&self) -> PathBuf {
        self.root.join("content")
    }

    fn chains(&self, chains: &str) {
        let file =
            format!("(type: \"sapphire::assets::FontChains\", version: 2, data: (chains: [{chains}]))");
        write(&self.content().join("fonts/a.fontchains.ron"), &file);
    }

    fn locale(&self, tag: &str, fallback: &str, families: &str) {
        write(&self.content().join(format!("loc/{tag}.locale.ron")), &locale_file(fallback, families));
    }

    fn strings(&self, tag: &str, fluent: &str) {
        write(&self.content().join(format!("loc/{tag}/a.ftl")), fluent);
    }

    fn screen(&self, document: &str) {
        write(&self.content().join("ui/a.ui.ron"), document);
    }

    fn cook_with(&self, cache: Option<&Path>) -> Result<CookReport, CookError> {
        let options = CookOptions { cook: Cook::Desktop, cache, locked: true };
        cook_dir(&self.content(), &self.root.join("loose"), &options)
    }

    fn cook(&self) -> Result<CookReport, CookError> {
        self.cook_with(None)
    }

    /// What a refused cook says, every level of its error together.
    fn refusal(&self) -> String {
        said_by(&self.cook().expect_err("a refused cook"))
    }
}

fn said_by(error: &CookError) -> String {
    let mut said = error.to_string();
    let mut source = error.source();
    while let Some(next) = source {
        said += &format!(": {next}");
        source = next.source();
    }
    said
}

fn says(said: &str, wanted: &[&str]) {
    for word in wanted {
        assert!(said.contains(word), "wanted `{word}` in: {said}");
    }
}

fn output<'a>(report: &'a CookReport, extension: &str) -> &'a sapphire_pipeline::cook::CookedAsset {
    report
        .assets
        .iter()
        .find(|asset| asset.output.extension().is_some_and(|found| found == extension))
        .unwrap_or_else(|| panic!("no .{extension} in {:#?}", report.assets))
}

/// Whether the cooked font `report` holds has a glyph for `character`, as the shaper finds it.
fn font_shows(report: &CookReport, family: &str, character: char) -> bool {
    let cooked = report
        .assets
        .iter()
        .filter(|asset| {
            asset.output.extension().is_some_and(|found| found == sapphire_assets::font::FONT_EXTENSION)
        })
        .map(|asset| {
            let mut bytes = AlignedVec::<16>::new();
            bytes.extend_from_slice(&fs::read(&asset.output).unwrap());
            Font::read(&bytes).unwrap().to_owned_font()
        })
        .find(|font| font.family == family)
        .expect("the family's font");
    let mut text = TextSystem::new();
    text.register_font(Arc::new(cooked.file.clone()));
    let style = TextStyle { families: family, size: 16.0, weight: 400.0, line_height: 1.3, locale: None };
    text.shape(&character.to_string(), &style, 1.0, None).glyphs.iter().all(|glyph| glyph.id != 0)
}

#[test]
fn what_a_screen_binds_joins_its_fonts_and_what_none_binds_does_not() {
    let tree = Tree::new("ui_cook_subset");
    let cache = tree.root.join("cache");

    // A table can hold it, and no screen shows it, so no font holds it.
    tree.strings("en", &format!("greeting = Hello\nunused = {ONLY_FROM_A_SCREEN}\n"));
    let report = tree.cook_with(Some(&cache)).expect("a cook");
    assert!(!font_shows(&report, "Oxanium", ONLY_FROM_A_SCREEN));
    assert!(font_shows(&report, "Oxanium", 'H'));

    // A screen shows it now: only the font's key had any reason to notice, and it does.
    tree.strings("en", &format!("greeting = Hello {ONLY_FROM_A_SCREEN}\nunused = {ONLY_FROM_A_SCREEN}\n"));
    let report = tree.cook_with(Some(&cache)).expect("a cook");
    assert_eq!(output(&report, "sfont").status, CookStatus::Cooked);
    assert!(font_shows(&report, "Oxanium", ONLY_FROM_A_SCREEN));

    // Nothing changed: nothing cooks.
    let report = tree.cook_with(Some(&cache)).expect("a cook");
    let cooked: Vec<_> = report.assets.iter().filter(|asset| asset.status == CookStatus::Cooked).collect();
    assert!(cooked.is_empty(), "{cooked:#?}");
}

#[test]
fn with_no_screen_a_tree_cooks_what_it_always_did() {
    let with = Tree::new("ui_cook_none_with");
    let report = with.cook().expect("a cook");
    let packed = pack_cooked(&report, &with.root.join("packs"), None).unwrap();
    assert!(packed.packs.iter().any(|pack| pack.name == UI_PACK), "{:?}", packed.packs);

    let without = Tree::new("ui_cook_none_without");
    fs::remove_file(without.content().join("ui/a.ui.ron")).unwrap();
    fs::remove_file(without.content().join("ui/a.ui.ron.meta")).unwrap();
    let report_without = without.cook().expect("a cook");
    let packed = pack_cooked(&report_without, &without.root.join("packs"), None).unwrap();
    assert!(packed.packs.iter().all(|pack| pack.name != UI_PACK), "no empty pack: {:?}", packed.packs);
    // "Hello" is all in the character set, so a screen showing it changes nothing about the font.
    assert_eq!(output(&report, "sfont").output_hash, output(&report_without, "sfont").output_hash);
}

#[test]
fn a_string_its_fonts_can_t_show_is_refused_by_locale_key_and_character() {
    let tree = Tree::new("ui_cook_uncovered");
    let hangul = IN_NO_FONT.to_string();
    let words = ["ui/a.ui.ron", "`line`", "`greeting`", "`en`", &hangul, "U+D55C", "`display`", "Oxanium"];

    // Plain, then from a term it refers to, then in the arm of a select no run of the game may reach.
    for fluent in [
        format!("greeting = Hello {IN_NO_FONT}\n"),
        format!("-name = {IN_NO_FONT}\ngreeting = Hello {{ -name }}\n"),
        format!("greeting = {{ $level ->\n    [one] {IN_NO_FONT}\n   *[other] Hello\n}}\n"),
    ] {
        tree.strings("en", &fluent);
        tree.screen(&screen_file("greeting", "display", "(name: \"level\", path: \"level\")"));
        says(&tree.refusal(), &words);
    }
}

#[test]
fn a_screen_a_language_cannot_show_says_what_is_missing() {
    let tree = Tree::new("ui_cook_screens");

    // A key no table has.
    tree.screen(&screen_file("farewell", "display", ""));
    says(&tree.refusal(), &["ui/a.ui.ron", "`line`", "`en`", "`farewell`"]);

    // A role the locale has no list for.
    tree.screen(&screen_file("greeting", "body", ""));
    says(&tree.refusal(), &["`line`", "`en`", "role `body`"]);

    // A message that reads a variable the node doesn't pass.
    tree.screen(&screen_file("greeting", "display", ""));
    tree.strings("en", "greeting = Level { $level }\n");
    says(&tree.refusal(), &["`line`", "`greeting`", "`en`", "$level"]);
    tree.strings("en", "greeting = Hello\n");

    // A family that's no font, then a font its role's chain doesn't load.
    tree.locale("en", "", "\"Missing Sans\"");
    says(&tree.refusal(), &["`en`", "`display`", "Missing Sans"]);
    tree.locale("en", "", "\"Oxanium\"");
    for file in ["DelaGothicOne-Regular.ttf", "DelaGothicOne-Regular.ttf.meta", "OFL.txt"] {
        let folder = tree.content().join("third_party/dela_gothic_one");
        fs::create_dir_all(&folder).unwrap();
        fs::copy(repo_content().join("third_party/dela_gothic_one").join(file), folder.join(file)).unwrap();
    }
    tree.chains("(name: \"display\", fonts: [\"9aab79e6-9051-4121-9a50-e20ce97db8f8\"])");
    says(&tree.refusal(), &["`en`", "`display`", "Oxanium", "chain"]);
    tree.chains("(name: \"other\", fonts: [\"ce9360bf-ac52-4ecc-9e73-75cd14b619d5\"])");
    says(&tree.refusal(), &["`display`", "font chain"]);
}

/// The keys a screen chooses among as it runs answer as its own key does, though none is named in it.
#[test]
fn a_key_chosen_from_the_model_is_held_to_what_its_prefix_finds() {
    let tree = Tree::new("ui_cook_bound_key");
    let chooses = screen_file("greeting", "display", "")
        .replace("args: []", "key_from: (path: \"name\", prefix: \"greeting\")");
    tree.screen(&chooses);
    write(&tree.content().join("ui/a.model.ron"), &model_file_of(&[("name", "Text")]));

    // Only a chosen key shows this, and only one under the prefix counts.
    tree.strings(
        "en",
        &format!(
            "greeting = Hello\ngreeting-other = {ONLY_FROM_A_SCREEN}\nunused = {ANOTHER_FROM_A_SCREEN}\n"
        ),
    );
    let report = tree.cook().expect("a cook");
    assert!(font_shows(&report, "Oxanium", ONLY_FROM_A_SCREEN));
    assert!(!font_shows(&report, "Oxanium", ANOTHER_FROM_A_SCREEN));

    // Each of them needs what the node passes.
    tree.strings("en", "greeting = Hello\ngreeting-other = Level { $level }\n");
    says(&tree.refusal(), &["`line`", "`greeting-other`", "`en`", "$level"]);

    // A path that isn't a string, and one that isn't there.
    tree.strings("en", "greeting = Hello\n");
    write(&tree.content().join("ui/a.model.ron"), &model_file_of(&[("name", "Number")]));
    says(&tree.refusal(), &[".text.key_from.path", "`name`"]);
    write(&tree.content().join("ui/a.model.ron"), &model_file_of(&[("level", "Number")]));
    says(&tree.refusal(), &[".text.key_from.path", "`name`"]);
}

#[test]
fn every_language_of_a_tree_answers_for_a_screen() {
    let tree = Tree::new("ui_cook_languages");
    tree.locale("ja", "\"en\"", "\"Oxanium\"");
    write(
        &tree.content().join("loc/ja.locale.ron.meta"),
        &sidecar("00000000-0000-4000-8000-000000000005", "Locale(())"),
    );

    // Japanese falls back to English's key, in Japanese's fonts.
    tree.cook().expect("a fallback answers");

    // A key only Japanese has isn't enough: English would show nothing.
    tree.strings("ja", "only-here = Hello\n");
    write(
        &tree.content().join("loc/ja/a.ftl.meta"),
        &sidecar("00000000-0000-4000-8000-000000000006", "Fluent((locale: \"ja\"))"),
    );
    tree.screen(&screen_file("only-here", "display", ""));
    says(&tree.refusal(), &["`only-here`", "`en`"]);
}

#[test]
fn a_document_is_refused_by_field_and_its_paths_by_its_model() {
    let tree = Tree::new("ui_cook_document");
    let cache = tree.root.join("cache");

    // Its own checks.
    let header = "(type: \"sapphire::assets::UiDocument\", version: 2, data: ";
    tree.screen(&format!("{header}(nodes: [])))"));
    says(&tree.refusal(), &["ui/a.ui.ron", "line"]);
    tree.screen(&format!("{header}(nodes: []))"));
    says(&tree.refusal(), &["ui/a.ui.ron", "nodes"]);
    tree.screen(&format!("{header}(nodes: [(name: \"root\", bogus: 1)]))"));
    says(&tree.refusal(), &["bogus"]);
    tree.screen(&format!("{header}(nodes: [(name: \"root\"), (name: \"a\", parent: \"nobody\")]))"));
    says(&tree.refusal(), &["`a`.parent", "nobody"]);

    // A path its model doesn't have, in the field that names it.
    tree.screen(&screen_file("greeting", "display", "(name: \"level\", path: \"mana\")"));
    says(&tree.refusal(), &["line", "args", "mana"]);

    // A good document, then the model it reads changing under it, which its cached result must notice.
    tree.screen(&screen_file("greeting", "display", "(name: \"level\", path: \"level\")"));
    let report = tree.cook_with(Some(&cache)).expect("a cook");
    assert_eq!(output(&report, sapphire_assets::ui::UI_EXTENSION).status, CookStatus::Cooked);
    write(&tree.content().join("ui/a.model.ron"), &model_file(&["mana"]));
    says(&said_by(&tree.cook_with(Some(&cache)).expect_err("the path went away")), &["level"]);
}

/// The test screen as committed, with all its fonts, which the goldens show.
#[test]
fn the_test_screen_cooks_in_both_languages_and_brings_its_characters() {
    let root = Path::new(env!("CARGO_TARGET_TMPDIR")).join("ui_cook_shipped");
    let _ = fs::remove_dir_all(&root);
    let content = root.join("content");
    let copy = |relative: String| {
        let to = content.join(&relative);
        fs::create_dir_all(to.parent().unwrap()).unwrap();
        fs::copy(repo_content().join(&relative), &to).unwrap_or_else(|error| panic!("{relative}: {error}"));
    };
    for font in
        ["mplus1/MPLUS1[wght].ttf", "oxanium/Oxanium[wght].ttf", "dela_gothic_one/DelaGothicOne-Regular.ttf"]
    {
        copy(format!("third_party/{font}"));
        copy(format!("third_party/{font}.meta"));
        copy(format!("third_party/{}/OFL.txt", Path::new(font).parent().unwrap().to_str().unwrap()));
    }
    for file in ["fonts/ui.charset.ron", "fonts/ui.fontchains.ron", "fonts/ui.fontchains.ron.meta"] {
        copy(file.to_owned());
    }
    for file in [
        "ui/test_screen.model.ron",
        "ui/test_screen.ui.ron",
        "ui/test_screen.ui.ron.meta",
        "loc/en.locale.ron",
        "loc/en.locale.ron.meta",
        "loc/ja.locale.ron",
        "loc/ja.locale.ron.meta",
        "loc/en/test_screen.ftl",
        "loc/en/test_screen.ftl.meta",
        "loc/ja/test_screen.ftl",
        "loc/ja/test_screen.ftl.meta",
    ] {
        copy(file.to_owned());
    }
    let options = CookOptions { cook: Cook::Desktop, cache: None, locked: true };
    let report = cook_dir(&content, &root.join("loose"), &options).expect("a locked cook");
    // Japanese's body has it, and the character set doesn't.
    let charset = fs::read_to_string(content.join("fonts/ui.charset.ron")).unwrap();
    assert!(!charset.contains('暁'), "this test wants a character only a screen brings");
    assert!(font_shows(&report, "M PLUS 1", '暁'));
    let packed = pack_cooked(&report, &root.join("packs"), None).unwrap();
    let names: Vec<_> = packed.packs.iter().map(|pack| pack.name.as_str()).collect();
    for pack in ["fonts", "loc", UI_PACK] {
        assert!(names.contains(&pack), "{names:?}");
    }
}
