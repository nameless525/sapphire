use std::collections::BTreeSet;

use sapphire_assets::sound::decode_vorbis;

use super::*;
use crate::cook::{Cook, CookOptions, cook_dir};
use crate::pack::pack_cooked;
use crate::test_util::TempDir;

fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// The committed masters were imported here: macOS on arm64, as `sapphire_soundgen`'s files were made.
fn made_here() -> bool {
    (std::env::consts::OS, std::env::consts::ARCH) == ("macos", "aarch64")
}

/// Every file `vo` writes from the committed script and masters is as committed, on every machine: the
/// voice lines, the recording scripts, the reports, and the Oggs and cues by their recorded hashes.
#[test]
fn the_committed_voice_over_is_current() {
    let imported = import(&repo().join("content"), Mode::Check).unwrap();
    assert_eq!(imported.stale, Vec::<PathBuf>::new(), "run `sapphire_pipeline vo --content content`");
    assert_eq!(imported.missing, Vec::new());
    assert_eq!(imported.recorded.len(), 6, "three lines in two languages");
}

/// What `the_committed_voice_over_is_current` can't see on another machine: each committed Ogg is its
/// master's encode. Where they were made, to the byte; elsewhere libvorbis's floating point may choose
/// differently, so the decoded sounds are held within what `sapphire_soundgen`'s own test allows.
#[test]
fn the_committed_oggs_are_their_masters_encodes() {
    let folder = repo().join("content").join(FOLDER);
    let script = read_script(&folder.join(SCRIPT)).unwrap();
    for language in &script.languages {
        for line in &script.lines {
            let master =
                wav::read(&fs::read(folder.join(language).join(format!("{}.wav", line.key))).unwrap())
                    .unwrap();
            let relative = format!("{FOLDER}/{language}/{}.ogg", line.key);
            let ours = encode::encode_line(&master.samples, master.sample_rate, encode::serial_of(&relative))
                .unwrap();
            let committed = fs::read(repo().join("content").join(&relative)).unwrap();
            if made_here() {
                assert!(ours == committed, "{relative}: the encode differs from the committed Ogg");
                continue;
            }
            let (ours, theirs) = (decode_vorbis(&ours).unwrap(), decode_vorbis(&committed).unwrap());
            assert_eq!(ours.frames(), theirs.frames(), "{relative}");
            let difference: f64 =
                ours.samples.iter().zip(&theirs.samples).map(|(a, b)| f64::from(a - b).powi(2)).sum();
            let sound: f64 = ours.samples.iter().map(|a| f64::from(*a).powi(2)).sum();
            assert!((difference / sound).sqrt() < 0.05, "{relative}");
        }
    }
}

/// The recording script of each language lists every line the game voices: each the script holds, each a
/// conversation's `vo` names, and each the fight's cue table says.
#[test]
fn each_recording_script_lists_every_voiced_key() {
    let content = repo().join("content");
    let mut voiced = BTreeSet::new();
    for path in walk(&content, ".dialogue.ron").unwrap() {
        let document = DialogueDocument::from_ron(&fs::read_to_string(path).unwrap()).unwrap();
        voiced.extend(document.nodes.iter().filter_map(|node| node.say.as_ref()?.vo.clone()));
    }
    let table = fs::read_to_string(repo().join("crates/sapphire_cues/default_cues.ron")).unwrap();
    voiced
        .extend(table.split("line: Some(\"").skip(1).map(|rest| rest.split('"').next().unwrap().to_owned()));
    assert!(voiced.len() >= 3, "{voiced:?}");
    let script = read_script(&content.join(FOLDER).join(SCRIPT)).unwrap();
    for language in &script.languages {
        let csv = fs::read_to_string(content.join(FOLDER).join(language).join(SCRIPT_CSV)).unwrap();
        let keys: BTreeSet<String> =
            csv.lines().skip(1).map(|row| row.split(',').next().unwrap().to_owned()).collect();
        assert!(voiced.is_subset(&keys), "{language}: {voiced:?} against {keys:?}");
        assert_eq!(keys.len(), script.lines.len(), "{language}");
        assert!(csv.starts_with(BYTE_ORDER_MARK), "{language}");
    }
}

/// M6 U11's cross-machine proof: the shipped voice cooks into `vo_en` and `vo_ja`, the same bytes on every
/// machine CI cooks on (macOS, Linux and Windows run this), and the same on a second cook. The packs hold
/// copies of the committed Oggs and mouth curves by the deterministic maths, so a change here is a change to
/// the voice, its cook or the packer: pin the new hashes in the same change. M6 U14 moved both for
/// `perfect-dodge`'s sound alone, whose take gained phoneme timings in each language
/// (`a_sidecar_moves_only_its_own_takes_sound`).
#[test]
fn the_voice_packs_cook_to_pinned_bytes_on_every_machine() {
    let pinned = [
        ("vo_en", "bad5cd854af473cb3adc1bea1fb6be2f8f1a39439facae52b217d14a08e4e3ab"),
        ("vo_ja", "666b3bdc35224196094b47afc962c1fc9a261ecf8017046b5dd1030bdf808372"),
    ];
    let tree = TempDir::new("vo_packs");
    let cooked = |name: &str| {
        let options = CookOptions { cook: Cook::Desktop, cache: None, locked: true, editor: false };
        let report =
            cook_dir(&repo().join("content").join(FOLDER), &tree.path().join(name), &options).unwrap();
        let packs = tree.path().join(format!("{name}-packs"));
        pack_cooked(&report, &packs, None).unwrap();
        let mut found: Vec<(String, String)> = fs::read_dir(&packs)
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .filter(|path| path.extension().is_some_and(|extension| extension == "spack"))
            .map(|path| {
                let hash = blake3::hash(&fs::read(&path).unwrap()).to_hex().to_string();
                (path.file_stem().unwrap().to_str().unwrap().to_owned(), hash)
            })
            .collect();
        found.sort();
        found
    };
    let first = cooked("first");
    assert_eq!(first, cooked("second"), "two cooks agree byte for byte");
    let pinned: Vec<(String, String)> =
        pinned.iter().map(|(name, hash)| ((*name).to_owned(), (*hash).to_owned())).collect();
    assert_eq!(first, pinned);
}

/// M6 U14: a phoneme sidecar moves its own take's cooked sound and nothing else. The committed voice over cooks
/// with its sidecars and again without them, and only the sounds of the lines that have one differ, so what
/// moves the packs' pins above is those sounds alone.
#[test]
fn a_sidecar_moves_only_its_own_takes_sound() {
    fn copy(from: &Path, to: &Path) {
        fs::create_dir_all(to).unwrap();
        for entry in fs::read_dir(from).unwrap() {
            let path = entry.unwrap().path();
            let name = path.file_name().unwrap().to_str().unwrap();
            if path.is_dir() {
                copy(&path, &to.join(name));
            } else if !name.ends_with(PHONEMES_SUFFIX) {
                fs::copy(&path, to.join(name)).unwrap();
            }
        }
    }
    fn outputs(dir: &Path, under: &Path, found: &mut BTreeMap<PathBuf, Vec<u8>>) {
        for entry in fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                outputs(&path, under, found);
            } else {
                found.insert(path.strip_prefix(under).unwrap().to_owned(), fs::read(&path).unwrap());
            }
        }
    }
    let committed = repo().join("content").join(FOLDER);
    let tree = TempDir::new("vo_sidecars");
    let bare = tree.path().join("bare");
    copy(&committed, &bare);
    let options = CookOptions { cook: Cook::Desktop, cache: None, locked: true, editor: false };
    let mut cooked = Vec::new();
    for (name, source) in [("with", committed.as_path()), ("without", bare.as_path())] {
        let loose = tree.path().join(name);
        cook_dir(source, &loose, &options).unwrap();
        let mut found = BTreeMap::new();
        outputs(&loose, &loose, &mut found);
        cooked.push(found);
    }
    let (with, without) = (&cooked[0], &cooked[1]);
    assert_eq!(with.keys().collect::<Vec<_>>(), without.keys().collect::<Vec<_>>());
    let moved: Vec<String> = with
        .iter()
        .filter(|(path, bytes)| without[*path] != **bytes)
        .map(|(path, _)| path.to_string_lossy().replace('\\', "/"))
        .collect();
    let mut sidecars: Vec<String> = Vec::new();
    for language in ["en", "ja"] {
        for path in files(&committed.join(language), PHONEMES_SUFFIX).unwrap() {
            sidecars.push(format!("{language}/{}.ssound", stem(&path, PHONEMES_SUFFIX)));
        }
    }
    assert_eq!(moved, sidecars, "only the sounds of the takes with timings");
    assert!(!sidecars.is_empty());
}

/// A content tree with a script of two lines in `en` and `ja`, their strings, and the masters `masters`
/// (language, key), each a tone of its own.
struct Tree {
    dir: TempDir,
}

const TREE_SCRIPT: &str = r#"(
    type: "sapphire::pipeline::VoiceScript",
    version: 1,
    data: (
        source: "en",
        languages: ["en", "ja"],
        lines: [
            (key: "hello", text: "line-hello", speaker: Some("line-name"), context: "A test, \"quoted\", with commas.", direction: "Warm."),
            (key: "bye", text: "line-bye", context: "A test.", direction: "Short.", approved: ["en"]),
        ],
    ),
)"#;

impl Tree {
    fn new(name: &str, masters: &[(&str, &str)]) -> Self {
        let dir = TempDir::new(name);
        let content = dir.path().join("content");
        for (language, fallback, strings) in [
            ("en", "", "line-name = Guide\nline-hello = Hello, there.\nline-bye = Bye.\n"),
            ("ja", "\"en\"", "line-name = 案内人\nline-hello = こんにちは。\n"),
        ] {
            fs::create_dir_all(content.join("loc").join(language)).unwrap();
            let ftl = content.join("loc").join(language).join("lines.ftl");
            fs::write(&ftl, strings).unwrap();
            // A locale for the cook, which the strings need, and both with sidecars, since it's locked.
            let locale = format!(
                "(type: \"sapphire::strings::LocaleDocument\", version: 1, data: (fallback: [{fallback}], fonts: {{}}))"
            );
            let document = content.join("loc").join(format!("{language}.locale.ron"));
            fs::write(&document, locale).unwrap();
            for source in [ftl, document] {
                let importer = ImportSettings::for_source(&source).unwrap();
                meta::create(
                    &meta::sidecar_path(&source),
                    &AssetMeta::new(meta::new_id().unwrap(), importer),
                )
                .unwrap();
            }
            fs::create_dir_all(content.join(FOLDER).join(language)).unwrap();
        }
        fs::write(content.join(FOLDER).join(SCRIPT), TREE_SCRIPT).unwrap();
        let tree = Self { dir };
        for (n, &(language, key)) in masters.iter().enumerate() {
            tree.master(language, key, 300.0 + 50.0 * n as f32);
        }
        tree
    }

    fn content(&self) -> PathBuf {
        self.dir.path().join("content")
    }

    fn vo(&self, file: &str) -> PathBuf {
        self.content().join(FOLDER).join(file)
    }

    /// A 16-bit master at 48 kHz: half a second of a tone at `hz`.
    fn master(&self, language: &str, key: &str, hz: f32) {
        let samples: Vec<i16> = (0..24_000)
            .map(|n| (0.5 * (std::f32::consts::TAU * hz * n as f32 / 48_000.0).sin() * 32_767.0) as i16)
            .collect();
        let mut bytes = b"RIFF\0\0\0\0WAVEfmt \x10\0\0\0\x01\0\x01\0".to_vec();
        bytes.extend_from_slice(&48_000_u32.to_le_bytes());
        bytes.extend_from_slice(&96_000_u32.to_le_bytes());
        bytes.extend_from_slice(b"\x02\0\x10\0data");
        bytes.extend_from_slice(&(samples.len() as u32 * 2).to_le_bytes());
        bytes.extend(samples.iter().flat_map(|sample| sample.to_le_bytes()));
        fs::write(self.vo(&format!("{language}/{key}.wav")), bytes).unwrap();
    }
}

#[test]
fn an_import_writes_what_a_cook_packs_by_language_and_a_second_changes_nothing() {
    let tree = Tree::new("vo_import", &[("en", "hello"), ("en", "bye"), ("ja", "hello")]);
    let imported = import(&tree.content(), Mode::Write).unwrap();
    assert_eq!(imported.missing, [("ja".to_owned(), "bye".to_owned())]);
    assert_eq!(imported.encoded.len(), 3);
    for file in
        ["en/hello.ogg", "en/hello.ogg.meta", "en/bye.cue.ron.meta", "ja/hello.cue.ron", "en.voice.ron.meta"]
    {
        assert!(tree.vo(file).exists(), "{file}");
    }
    assert_eq!(
        meta::read(&tree.vo("en/hello.ogg.meta")).unwrap().importer,
        ImportSettings::Sound(SoundSettings { mouth: true }),
        "a line's sound cooks its mouth curve"
    );
    let ja = VoiceLines::from_document(&fs::read_to_string(tree.vo("ja.voice.ron")).unwrap()).unwrap();
    assert_eq!(ja.lines.iter().map(|line| line.key.as_str()).collect::<Vec<_>>(), ["hello"]);

    // The recording scripts: quoted where a cell needs it, each line's status, the speaker in the language,
    // and the length against the source's.
    let en = fs::read_to_string(tree.vo("en/script.csv")).unwrap();
    assert!(en.contains("hello,Guide,\"Hello, there.\",\"Hello, there.\",\"A test, \"\"quoted\"\", with commas.\",Warm.,0.500,0.500,recorded\n"), "{en}");
    assert!(en.contains(&format!("bye,{IN_PLAY},Bye.,Bye.,A test.,Short.,0.500,0.500,approved\n")), "{en}");
    let ja = fs::read_to_string(tree.vo("ja/script.csv")).unwrap();
    assert!(ja.contains("hello,案内人,\"Hello, there.\",こんにちは。"), "{ja}");
    assert!(ja.contains(&format!("bye,{IN_PLAY},Bye.,,A test.,Short.,0.500,,untranslated\n")), "{ja}");
    let report = fs::read_to_string(tree.vo("ja/import.csv")).unwrap();
    assert!(report.contains("hello,recorded,16-bit,48000,0.500,1.00,-6.0,-9.0,"), "{report}");

    // What it wrote is what a cook takes, and the packs go by language.
    let options = CookOptions { cook: Cook::Desktop, cache: None, locked: true, editor: false };
    let report = cook_dir(&tree.content(), &tree.dir.path().join("loose"), &options).unwrap();
    let packed = pack_cooked(&report, &tree.dir.path().join("packs"), None).unwrap();
    let names: BTreeSet<&str> = packed.packs.iter().map(|pack| pack.name.as_str()).collect();
    assert_eq!(names, BTreeSet::from(["loc", "vo_en", "vo_ja"]));
    let en = packed.packs.iter().find(|pack| pack.name == "vo_en").unwrap();
    assert_eq!(en.assets, 5, "the lines, and two cues and two sounds");

    // Imported again, nothing changes; a master changed is encoded again, alone.
    assert_eq!(import(&tree.content(), Mode::Check).unwrap().stale, Vec::<PathBuf>::new());
    assert_eq!(import(&tree.content(), Mode::Write).unwrap().stale, Vec::<PathBuf>::new());
    tree.master("en", "bye", 440.0);
    let checked = import(&tree.content(), Mode::Check).unwrap();
    assert_eq!(checked.encoded, [tree.vo("en/bye.ogg")]);
    assert!(checked.stale.contains(&tree.vo("en/import.csv")), "{:?}", checked.stale);
    let imported = import(&tree.content(), Mode::Write).unwrap();
    assert_eq!(imported.encoded, [tree.vo("en/bye.ogg")]);
    assert_eq!(import(&tree.content(), Mode::Check).unwrap().stale, Vec::<PathBuf>::new());
}

#[test]
fn a_master_the_script_doesnt_list_fails_naming_it() {
    let tree = Tree::new("vo_extra", &[("en", "hello"), ("ja", "goodbye")]);
    let error = import(&tree.content(), Mode::Write).unwrap_err().to_string();
    assert!(error.contains("goodbye.wav") && error.contains("`goodbye` isn't a line"), "{error}");
    fs::remove_file(tree.vo("ja/goodbye.wav")).unwrap();
    fs::write(tree.vo("ja/bye.wav"), b"RIFF\0\0\0\0WAVEfmt ").unwrap();
    let error = import(&tree.content(), Mode::Write).unwrap_err().to_string();
    assert!(error.contains("bye.wav") && error.contains("no `fmt ` chunk"), "{error}");
}

#[test]
fn a_conversation_voicing_what_the_script_doesnt_say_fails_naming_both() {
    let tree = Tree::new("vo_dialogue", &[("en", "hello"), ("en", "bye")]);
    let dialogue = |vo: &str, line: &str| {
        format!(
            "(type: \"sapphire::dialogue::DialogueDocument\", version: 1, data: (start: \"a\", nodes: [\
             (name: \"a\", say: (speaker: \"line-name\", line: \"{line}\", vo: \"{vo}\"))]))"
        )
    };
    let path = tree.content().join("talk.dialogue.ron");
    fs::write(&path, dialogue("hello", "line-hello")).unwrap();
    import(&tree.content(), Mode::Write).unwrap();
    fs::write(&path, dialogue("hi", "line-hello")).unwrap();
    let error = import(&tree.content(), Mode::Write).unwrap_err().to_string();
    assert!(error.contains("talk.dialogue.ron") && error.contains("voices `hi`"), "{error}");
    fs::write(&path, dialogue("hello", "line-bye")).unwrap();
    let error = import(&tree.content(), Mode::Write).unwrap_err().to_string();
    assert!(error.contains("say `line-bye`") && error.contains("say `line-hello`"), "{error}");
}

#[test]
fn a_script_that_cant_be_voiced_says_why() {
    let tree = Tree::new("vo_script", &[]);
    for (from, to, why) in [
        ("languages: [\"en\", \"ja\"]", "languages: [\"ja\", \"en\"]", "comes first"),
        ("languages: [\"en\", \"ja\"]", "languages: [\"en\", \"en\"]", "twice"),
        ("key: \"bye\"", "key: \"hello\"", "`hello` is there twice"),
        ("key: \"bye\"", "key: \"2bye\"", "isn't a line's key"),
        ("approved: [\"en\"]", "approved: [\"fr\"]", "isn't voiced"),
        ("text: \"line-bye\"", "text: \"line-none\"", "has no en string"),
        ("approved: [\"en\"]", "approved: [\"ja\"]", "with no master"),
    ] {
        fs::write(tree.vo(SCRIPT), TREE_SCRIPT.replace(from, to)).unwrap();
        let error = import(&tree.content(), Mode::Check).unwrap_err().to_string();
        assert!(error.contains(why), "{to}: {error}");
    }
}

#[test]
fn levels_read_the_same_on_every_machine() {
    assert_eq!(decibels(1.0), "0.0");
    assert_eq!(decibels(0.5), "-6.0");
    assert_eq!(decibels(0.0), "-inf");
    assert_eq!(csv_row(&["a", "b,c", "say \"hi\""]), "a,\"b,c\",\"say \"\"hi\"\"\"\n");
}

/// M6 U14: a take's phoneme timings stand beside its master, in its folder's language, or the import says why.
#[test]
fn phoneme_timings_without_their_take_or_in_another_language_fail_naming_them() {
    let tree = Tree::new("vo_phonemes", &[("en", "hello"), ("en", "bye")]);
    let timings = |locale: &str| {
        format!(
            "(type: \"sapphire::assets::PhonemeTimings\", version: 1, data: (locale: \"{locale}\", phonemes: [\
             (phoneme: \"HH\", from_ms: 0, to_ms: 50)]))"
        )
    };
    fs::write(tree.vo("en/hello.phonemes.ron"), timings("en")).unwrap();
    import(&tree.content(), Mode::Write).unwrap();
    fs::write(tree.vo("ja/hello.phonemes.ron"), timings("ja")).unwrap();
    let error = import(&tree.content(), Mode::Check).unwrap_err().to_string();
    assert!(error.contains("hello.phonemes.ron") && error.contains("has no master beside it"), "{error}");
    fs::remove_file(tree.vo("ja/hello.phonemes.ron")).unwrap();
    fs::write(tree.vo("en/bye.phonemes.ron"), timings("ja")).unwrap();
    let error = import(&tree.content(), Mode::Check).unwrap_err().to_string();
    assert!(error.contains("bye.phonemes.ron") && error.contains("is in ja, in en's folder"), "{error}");
    fs::write(tree.vo("en/bye.phonemes.ron"), timings("en").replace("to_ms: 50", "to_ms: 0")).unwrap();
    let error = import(&tree.content(), Mode::Check).unwrap_err().to_string();
    assert!(error.contains("bye.phonemes.ron") && error.contains("must end after it starts"), "{error}");
}
