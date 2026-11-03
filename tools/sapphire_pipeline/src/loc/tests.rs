//! Cooking locales: the real path through `cook_dir` and the packer, with `.ftl` files on disk.

use std::fs;
use std::path::{Path, PathBuf};

use sapphire_assets::AssetKind;
use sapphire_strings::{Pattern, StringTable};

use super::*;
use crate::cook::{Cook, CookError, CookOptions, CookReport, CookStatus, cook_dir};
use crate::meta::sidecar_path;
use crate::pack::{self, pack_cooked};
use crate::test_util::TempDir;

const EN: &str = r#"(type: "sapphire::strings::LocaleDocument", version: 1, data: ())"#;
const JA: &str = r#"(type: "sapphire::strings::LocaleDocument", version: 1, data: (fallback: ["en"]))"#;

struct Tree {
    dir: TempDir,
}

impl Tree {
    /// English and Japanese, each with `en/a.ftl` or `ja/a.ftl` holding what's given.
    fn new(name: &str, en: &str, ja: &str) -> Self {
        let tree = Self { dir: TempDir::new(&format!("loc-{name}")) };
        tree.write("en.locale.ron", EN);
        tree.write("ja.locale.ron", JA);
        tree.write("en/a.ftl", en);
        tree.write("ja/a.ftl", ja);
        tree
    }

    fn source(&self) -> PathBuf {
        self.dir.path().join("source")
    }

    fn write(&self, relative: &str, text: &str) {
        let path = self.source().join(relative);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    }

    fn cook(&self) -> Result<CookReport, CookError> {
        let cache = self.dir.path().join("cache");
        let options = CookOptions { cook: Cook::Desktop, cache: Some(&cache), locked: false };
        cook_dir(&self.source(), &self.dir.path().join("out"), &options)
    }

    fn table(&self, locale: &str) -> StringTable {
        let bytes = fs::read(self.dir.path().join(format!("out/{locale}.sloc"))).unwrap();
        StringTable::read(&bytes).unwrap()
    }

    /// The error a cook fails with, as it reads.
    fn failure(&self) -> String {
        let error = self.cook().expect_err("the cook should fail");
        let mut message = error.to_string();
        let mut source = std::error::Error::source(&error);
        while let Some(cause) = source {
            message.push_str(": ");
            message.push_str(&cause.to_string());
            source = cause.source();
        }
        message
    }
}

fn status_of(report: &CookReport, locale: &str) -> CookStatus {
    let name = format!("{locale}.sloc");
    report.assets.iter().find(|asset| asset.output.ends_with(&name)).unwrap().status
}

#[test]
fn locales_cook_to_a_table_each_found_by_their_sidecars() {
    let tree = Tree::new("cook", "title = Menu\n", "title = メニュー\n");
    tree.write("en/more/b.ftl", "more = More\n");
    tree.write(
        "en/more/b.ftl.meta",
        r#"(type: "sapphire::assets::AssetMeta", version: 1, data: (id: "0e1f2a3b-4c5d-4e6f-8a9b-0c1d2e3f4a5b", importer: Fluent((locale: "en"))))"#,
    );
    // A Japanese file anywhere is Japanese if its sidecar says so.
    tree.write("elsewhere/c.ftl", "other = その他\n");
    let id = "4f6b2c1e-8a3d-4e5f-9b7c-0d1e2f3a4b5c";
    tree.write(
        "elsewhere/c.ftl.meta",
        &format!(
            r#"(type: "sapphire::assets::AssetMeta", version: 1, data: (id: "{id}", importer: Fluent((locale: "ja"))))"#
        ),
    );
    let report = tree.cook().unwrap();
    assert_eq!(report.assets.len(), 2, "a table per locale, and nothing for a .ftl: {report:#?}");
    let ja_meta = fs::read_to_string(sidecar_path(&tree.source().join("ja/a.ftl"))).unwrap();
    assert!(ja_meta.contains("Fluent") && ja_meta.contains("\"ja\""), "{ja_meta}");
    let ja = tree.table("ja");
    assert_eq!(ja.locale, "ja");
    assert_eq!(ja.fallback, ["en"]);
    assert!(ja.entry("title").is_some() && ja.entry("other").is_some());
    assert!(tree.table("en").entry("more").is_some(), "{report:#?}");
}

#[test]
fn a_ftl_whose_locale_has_no_document_is_named() {
    let tree = Tree::new("orphan", "title = Menu\n", "title = メニュー\n");
    tree.write("fr/a.ftl", "title = Menu\n");
    let message = tree.failure();
    assert!(
        message.contains("`fr`") && message.contains("fr.locale.ron") && message.contains("a.ftl"),
        "{message}"
    );
}

#[test]
fn a_locale_with_two_documents_is_named() {
    let tree = Tree::new("twice", "title = Menu\n", "title = メニュー\n");
    tree.write("elsewhere/ja.locale.ron", JA);
    let message = tree.failure();
    assert!(message.contains("`ja` has two documents"), "{message}");
}

/// Every source the cook refuses, with what its message must name: all at once, so one run shows every
/// row that's wrong.
#[test]
fn what_cant_cook_fails_by_name() {
    let good = "title = Menu\n";
    let rows: &[(&str, &str, &str, &[&str])] = &[
        ("parse error", "title = Menu\nbad line here\n", "", &["a.ftl", ":2:"]),
        ("duplicate key", "title = One\ntitle = Two\n", "", &["`title`", "a.ftl"]),
        ("duplicate attribute", "title = One\n    .tip = A\n    .tip = B\n", "", &["`title.tip`"]),
        ("duplicate term", "-brand = One\n-brand = Two\n", "", &["`-brand`"]),
        ("missing message", "title = { nothing }\n", "", &["`title`", "`nothing`"]),
        ("missing attribute", "title = { other.tip }\nother = x\n", "", &["`title`", "`other.tip`"]),
        ("missing term", "title = { -nothing }\n", "", &["`title`", "`-nothing`"]),
        ("a message with no value", "title = { other }\nother =\n    .tip = x\n", "", &["`other`"]),
        ("cycle", "a = { b }\nb = { c }\nc = { a }\n", "", &["a -> b -> c -> a"]),
        ("cycle through a term", "-a = { -b }\n-b = { -a }\n", "", &["-a -> -b -> -a"]),
        ("cycle in a variant", "a = { $n ->\n   *[other] { a }\n}\n", "", &["a -> a"]),
        ("another function", "title = { DATETIME($d) }\n", "", &["`title`", "DATETIME"]),
        ("another option", "title = { NUMBER($n, style: \"percent\") }\n", "", &["`title`", "`style`"]),
        ("too many digits", "title = { NUMBER($n, minimumFractionDigits: 99) }\n", "", &["`title`", "`99`"]),
        (
            "digits the wrong way",
            "title = { NUMBER($n, minimumFractionDigits: 3, maximumFractionDigits: 1) }\n",
            "",
            &["`title`", "3..1"],
        ),
        ("NUMBER of text", "title = { NUMBER(\"x\") }\n", "", &["`title`", "NUMBER"]),
        (
            "a variable Japanese lacks",
            "title = { $name }\n",
            "title = x\n",
            &["`title`", "`$name`", "`en`", "`ja`"],
        ),
        ("a variable English lacks", good, "title = { $name }\n", &["`title`", "`$name`", "`ja`", "`en`"]),
        (
            "a selector kind that differs",
            "title = { $n ->\n    [one] a\n   *[other] b\n}\n",
            "title = { $n ->\n    [high] a\n   *[other] b\n}\n",
            &["`$n`", "`title`", "`en`", "`ja`"],
        ),
        (
            "a variable differing through a reference",
            "title = { other }\nother = { $n }\n",
            "title = x\nother = { $n }\n",
            &["`$n`", "`title`"],
        ),
    ];
    let mut wrong = Vec::new();
    for (index, (name, en, ja, names)) in rows.iter().enumerate() {
        let ja = if ja.is_empty() { "unrelated = x\n" } else { ja };
        let tree = Tree::new(&format!("refuse-{index}"), en, ja);
        match tree.cook() {
            Ok(_) => wrong.push(format!("{name}: cooked")),
            Err(_) => {
                let message = tree.failure();
                if let Some(missing) = names.iter().find(|needed| !message.contains(**needed)) {
                    wrong.push(format!("{name}: no {missing} in: {message}"));
                }
            }
        }
    }
    assert_eq!(wrong, Vec::<String>::new());
}

#[test]
fn a_bad_chain_fails_by_name() {
    let tree = Tree::new("chain", "title = Menu\n", "title = メニュー\n");
    let document = |fallback: &str| {
        format!(r#"(type: "sapphire::strings::LocaleDocument", version: 1, data: (fallback: [{fallback}]))"#)
    };
    tree.write("ja.locale.ron", &document(r#""fr""#));
    let message = tree.failure();
    assert!(message.contains("`ja` falls back to `fr`"), "{message}");
    tree.write("ja.locale.ron", &document(r#""ja""#));
    assert!(tree.failure().contains("`ja` names `ja`"));
    tree.write("ja.locale.ron", &document(r#""en", "en""#));
    assert!(tree.failure().contains("`ja` names `en`"));
    tree.write("ja.locale.ron", &document(r#""not a tag""#));
    assert!(tree.failure().contains("`not a tag` isn't a locale tag"));
}

#[test]
fn a_locale_with_a_bad_file_name_fails_by_name() {
    let tree = Tree::new("tag", "title = Menu\n", "title = メニュー\n");
    tree.write("not a tag.locale.ron", EN);
    assert!(tree.failure().contains("`not a tag` isn't a locale tag"));
}

#[test]
fn the_document_is_read_strictly() {
    let tree = Tree::new("document", "title = Menu\n", "title = メニュー\n");
    tree.write(
        "ja.locale.ron",
        r#"(type: "sapphire::strings::LocaleDocument", version: 1, data: (fallbacks: ["en"]))"#,
    );
    assert!(tree.failure().contains("fallbacks"));
    tree.write(
        "ja.locale.ron",
        r#"(type: "sapphire::strings::LocaleDocument", version: 1, data: (fonts: {"body": []}))"#,
    );
    assert!(tree.failure().contains("`body`"));
}

#[test]
fn a_duplicate_across_a_locales_files_names_both() {
    let tree = Tree::new("across", "title = One\n", "title = メニュー\n");
    tree.write("en/b.ftl", "title = Two\n");
    let message = tree.failure();
    assert!(message.contains("en/a.ftl") && message.contains("en/b.ftl"), "{message}");
}

#[test]
fn the_limits_a_format_runs_under_are_refused_at_cook() {
    // Sixty-four references deep.
    let mut deep = String::from("m0 = end\n");
    for level in 1..64 {
        deep.push_str(&format!("m{level} = {{ m{} }}\n", level - 1));
    }
    let tree = Tree::new("deep", &deep, "unrelated = x\n");
    assert!(tree.failure().contains("deep"));
    // Doubling at every level: 2^16 steps, in 17 messages.
    let mut wide = String::from("m0 = end\n");
    for level in 1..17 {
        wide.push_str(&format!("m{level} = {{ m{0} }}{{ m{0} }}\n", level - 1));
    }
    let tree = Tree::new("wide", &wide, "unrelated = x\n");
    assert!(tree.failure().contains("steps"));
}

/// Japanese may drop a selector, and keep the variable, or may keep the select with only a default;
/// a plural in one and a plain show in the other agree.
#[test]
fn a_translation_may_drop_what_its_language_doesnt_need() {
    let en = "title = { $n ->\n    [one] a\n   *[other] { $n } b\n}\ngender = { $g ->\n    [high] a\n   *[other] b\n}\n";
    let ja = "title = { $n }個\ngender = { $g ->\n   *[other] x\n}\n";
    let report = Tree::new("drops", en, ja).cook().unwrap();
    assert_eq!(report.localization, LocReport::default());
}

#[test]
fn the_report_lists_whats_untranslated_and_whats_stale_on_every_cook() {
    let tree = Tree::new(
        "report",
        "title = Menu\nmore = More\n    .tip = Tip\n-brand = B\n",
        "title = メニュー\ngone = x\n",
    );
    let expect = LocReport {
        untranslated: ["-brand", "more.tip"]
            .map(|key| Untranslated { locale: "ja".into(), key: key.into(), served_by: "en".into() })
            .to_vec(),
        stale: vec![Stale { locale: "ja".into(), key: "gone".into() }],
    };
    let mut first = tree.cook().unwrap().localization;
    // `more` has only that attribute's name on the English side, plus a value.
    first.untranslated.retain(|item| item.key != "more");
    assert_eq!(first, expect);
    let again = tree.cook().unwrap();
    assert_eq!(status_of(&again, "ja"), CookStatus::UpToDate);
    assert_eq!(again.localization.untranslated.len(), 3, "and from the cache too");
}

#[test]
fn an_edit_recooks_its_own_locale_and_still_checks_the_others() {
    let tree = Tree::new("edit", "title = { $n }\n", "title = { $n }\n");
    tree.cook().unwrap();
    tree.write("en/a.ftl", "title = Number { $n }\n");
    let report = tree.cook().unwrap();
    assert_eq!(status_of(&report, "en"), CookStatus::Cooked);
    assert_eq!(status_of(&report, "ja"), CookStatus::UpToDate);
    // English no longer takes `$n`, and Japanese wasn't touched: that's still this cook's failure.
    tree.write("en/a.ftl", "title = Number\n");
    assert!(tree.failure().contains("`$n`"));
    tree.write("en/a.ftl", "title = Number { $n }\n");
    let report = tree.cook().unwrap();
    assert_eq!(status_of(&report, "en"), CookStatus::Cached, "that version's table was kept");
    // A file added to a locale counts for its key.
    tree.write("ja/b.ftl", "unrelated = x\n");
    assert_eq!(status_of(&tree.cook().unwrap(), "ja"), CookStatus::Cooked);
    // And a font in the document does.
    tree.write(
        "ja.locale.ron",
        r#"(type: "sapphire::strings::LocaleDocument", version: 1, data: (fallback: ["en"], fonts: {"body": ["M PLUS 1"]}))"#,
    );
    assert_eq!(status_of(&tree.cook().unwrap(), "ja"), CookStatus::Cooked);
    assert_eq!(tree.table("ja").font_chain("body").unwrap(), ["M PLUS 1"]);
}

#[test]
fn text_with_nothing_to_work_out_is_one_span() {
    let en = "plain = Hello\n    world\nliteral = Hello { \"{\" } there\nescaped = { \"\\u3042\\\\\" }\nworked = Hi { $n }\n";
    let tree = Tree::new("plain", en, "unrelated = x\n");
    tree.cook().unwrap();
    let table = tree.table("en");
    let plain = |key: &str| match table.entry(key).unwrap().pattern {
        Pattern::Plain(span) => Some(table.str(span).to_owned()),
        Pattern::Parts(_) => None,
    };
    assert_eq!(plain("plain").as_deref(), Some("Hello\nworld"));
    assert_eq!(plain("literal").as_deref(), Some("Hello { there"));
    assert_eq!(plain("escaped").as_deref(), Some("あ\\"));
    assert_eq!(plain("worked"), None);
}

#[test]
fn the_same_sources_make_the_same_bytes() {
    let cook = |name: &str| {
        let tree = Tree::new(name, "-b = B\nb = { -b } { $n }\na = A\n    .x = { $n }\n", "a = あ\n");
        tree.cook().unwrap();
        fs::read(tree.dir.path().join("out/en.sloc")).unwrap()
    };
    assert_eq!(cook("same-1"), cook("same-2"));
}

#[test]
fn tables_pack_apart_from_everything_else() {
    let tree = Tree::new("pack", "title = Menu\n", "title = メニュー\n");
    let report = tree.cook().unwrap();
    assert_eq!(pack::group(Some(AssetKind::Strings)), pack::LOC);
    let packs = tree.dir.path().join("packs");
    let packed = pack_cooked(&report, &packs, None).unwrap();
    let names: Vec<&str> = packed.packs.iter().map(|pack| pack.name.as_str()).collect();
    assert_eq!(names, [pack::LOC]);
    assert_eq!(packed.packs[0].assets, 2);
}

#[test]
fn a_locked_cook_needs_a_ftls_sidecar() {
    let tree = Tree::new("locked", "title = Menu\n", "title = メニュー\n");
    let cache = tree.dir.path().join("cache");
    let locked = CookOptions { cook: Cook::Desktop, cache: Some(&cache), locked: true };
    let out = tree.dir.path().join("out");
    let error = cook_dir(&tree.source(), &out, &locked).expect_err("no sidecars yet");
    assert!(error.to_string().contains("a.ftl") || error.to_string().contains(".locale.ron"), "{error}");
    tree.cook().unwrap();
    cook_dir(&tree.source(), &out, &locked).unwrap();
}

/// `sapphire_loc`'s tests read these bytes, on phones as well, where no cooker runs: they stay what
/// their sources cook to. After changing either or the format:
/// `SAPPHIRE_UPDATE_LOC_FIXTURES=1 cargo test -p sapphire_pipeline loc::tests::the_committed`.
#[test]
fn the_committed_tables_are_what_their_sources_cook_to() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../crates/sapphire_loc/testdata");
    let scratch = TempDir::new("loc-committed");
    let options = CookOptions { cook: Cook::Desktop, cache: None, locked: true };
    let report = cook_dir(&root.join("content"), scratch.path(), &options).unwrap();
    let mut keys: Vec<(&str, &str)> = report
        .localization
        .untranslated
        .iter()
        .map(|item| (item.locale.as_str(), item.key.as_str()))
        .collect();
    keys.sort_unstable();
    assert_eq!(
        keys,
        [("ja", "only-english"), ("ja", "only-english-brand"), ("ja", "only-english-count")],
        "the fixture's own report"
    );
    assert_eq!(report.localization.stale, []);
    for locale in ["en", "ja"] {
        let name = format!("{locale}.sloc");
        let cooked = fs::read(scratch.path().join(&name)).unwrap();
        let committed = root.join("cooked").join(&name);
        if std::env::var_os("SAPPHIRE_UPDATE_LOC_FIXTURES").is_some() {
            fs::write(&committed, &cooked).unwrap();
        }
        assert!(
            fs::read(&committed).is_ok_and(|bytes| bytes == cooked),
            "{name} is stale: see this test's docs"
        );
    }
}
