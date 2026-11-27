//! The malformed-document fixtures (M6 T6): each gives exactly one problem, on the field that's wrong, or on the
//! document, saying why, where its text stops being RON before any field.

use std::path::{Path, PathBuf};

use sapphire_document::problem::{self, At};
use sapphire_document::{Kind, Origin, PropertyPath, walk};

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("test_content/problems")
}

/// Each fixture, the tool that finds it and the field it's on (`None`: the whole document).
const FIXTURES: &[(&str, Origin, Option<&str>)] = &[
    ("flat_scale.zone.ron", Origin::Parse, Some("placements[1].scale")),
    ("unknown_field.zone.ron", Origin::Parse, Some("placements[0].colour")),
    ("unclosed.zone.ron", Origin::Parse, None),
    ("loud.cue.ron", Origin::Parse, Some("volume_db")),
    ("rough.material.ron", Origin::Parse, Some("roughness")),
    ("unknown_kind.foliage.ron", Origin::Check, Some("rules[1].kind")),
    ("second_start.encounter.ron", Origin::Check, Some("waves[1].when")),
    ("unknown_move.moves.ron", Origin::Check, Some("neutral")),
    ("dark_sun_day.ron", Origin::Parse, Some("keys[0].sun.intensity")),
    ("overfollow_rigs.ron", Origin::Parse, Some("rigs[1].rig.key.camera_follow")),
    ("loud_rumble_cues.ron", Origin::Parse, Some("rows[0].rumble.low")),
];

#[test]
fn each_malformed_fixture_is_one_problem_on_its_field() {
    let mut seen = Vec::new();
    for entry in std::fs::read_dir(fixtures()).expect("the fixtures") {
        let path = entry.expect("a fixture").path();
        let name = path.file_name().and_then(|name| name.to_str()).expect("a name").to_owned();
        let (_, origin, field) = FIXTURES
            .iter()
            .find(|(fixture, ..)| *fixture == name)
            .unwrap_or_else(|| panic!("{name} has no row in FIXTURES"));
        let text = std::fs::read_to_string(&path).expect("its text");
        let kind = Kind::of(&path).unwrap_or_else(|| panic!("{name} is a kind the editor opens"));
        let problems = problem::of_text(Path::new(&name), kind, &text);
        assert_eq!(problems.len(), 1, "{name}: {}", problem::report(&problems));
        let found = &problems[0];
        assert_eq!(found.origin, *origin, "{name}: {found}");
        match (field, &found.at) {
            (Some(field), At::Field(at)) => {
                assert_eq!(at, field, "{name}: {found}");
                assert_resolves(&path, kind, &text, at);
            }
            (None, At::Document(why)) => assert!(!why.is_empty(), "{name}: {found}"),
            _ => panic!("{name}: expected {field:?}, found {found}"),
        }
        seen.push(name);
    }
    assert_eq!(seen.len(), FIXTURES.len(), "every row has its fixture: {seen:?}");
}

/// A problem's field is one the document has: what the text reads as once its fault is mended (the fixture's
/// source, named on its first line), or, if it reads, the value itself, the path resolving with `walk::named`.
fn assert_resolves(path: &Path, kind: Kind, text: &str, field: &str) {
    let written: PropertyPath = field.parse().expect("a path");
    let resolves = |text: &str| -> Option<bool> {
        let value = kind.value(text)?;
        Some(walk::named(value.as_ref(), &written).is_ok())
    };
    let mended = mended(path);
    let found = resolves(text).or_else(|| mended.as_deref().and_then(resolves));
    if let Some(found) = found {
        assert!(found, "{}: `{field}` isn't a path of the document", path.display());
    }
}

/// The fixtures a value was changed in, from a shipped document: that document's text, which reads.
fn mended(path: &Path) -> Option<String> {
    let name = path.file_name()?.to_str()?;
    let source = match name {
        "dark_sun_day.ron" => "content/zones/proving_hub/proving_hub_day.ron",
        "overfollow_rigs.ron" => "content/lookdev/lookdev_light_rigs.ron",
        "loud_rumble_cues.ron" => "crates/sapphire_cues/default_cues.ron",
        _ => return None,
    };
    std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("../..").join(source)).ok()
}

/// A check's field, which a document that reads has, is a path of its value: the one Properties shows its row by.
#[test]
fn a_fields_path_in_the_text_is_a_path_of_the_value() {
    let path = fixtures().join("unknown_kind.foliage.ron");
    let text = std::fs::read_to_string(&path).expect("its text");
    let document: sapphire_zone::FoliageDocument = sapphire_serialize::from_ron(&text).expect("it reads");
    let written: PropertyPath = "rules[1].kind".parse().expect("a path");
    let named = walk::named(&document, &written).expect("it's there");
    assert_eq!(named.to_string(), r#"rules["maples"].kind"#, "an entry with a name goes by it");
    assert!(walk::get(&document, &named).is_ok());
    let missing: PropertyPath = "rules[7].kind".parse().expect("a path");
    assert!(walk::named(&document, &missing).is_err());
}

/// What `content-check` reads of every shipped document finds nothing: a problem there would fail every commit
/// the hook checks.
#[test]
fn the_shipped_documents_read_with_no_problem() {
    let mut folders = vec![Path::new(env!("CARGO_MANIFEST_DIR")).join("../../content")];
    let (mut read, mut problems) = (0, Vec::new());
    while let Some(folder) = folders.pop() {
        for entry in std::fs::read_dir(folder).expect("a folder") {
            let path = entry.expect("an entry").path();
            if path.is_dir() {
                folders.push(path);
            } else if let Some(kind) = Kind::of(&path) {
                read += 1;
                let text = std::fs::read_to_string(&path).expect("its text");
                problems.extend(problem::of_text(&path, kind, &text));
            }
        }
    }
    assert!(read > 200, "the content's documents: {read}");
    assert!(problems.is_empty(), "{}", problem::report(&problems));
}

/// Every field the anime character material's validators name on its `surface` resolves on a document of that
/// surface (a face, hair, an iris and a highlight), so a range refusal of any of them lands on its row: the
/// validators write `surface.softness`, and `walk::named` steps into the `Face((..))` variant it's in.
#[test]
fn every_character_surface_field_a_validator_names_is_a_path_of_its_document() {
    let source = include_str!("../../../crates/sapphire_assets/src/material/character.rs");
    let mut fields: Vec<String> = source
        .split('"')
        .skip(1)
        .step_by(2)
        .filter(|literal| literal.starts_with("surface.") && *literal != "surface.through_hair")
        .map(ToOwned::to_owned)
        .collect();
    // `ThroughHair::validate` names its fields under the prefix it's given.
    for child in ["opacity", "fade_yaw", "fade_pitch", "distance"] {
        fields.push(format!("surface.through_hair.{child}"));
    }
    assert!(fields.len() > 25, "{fields:?}");
    let content = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../content");
    let read = |path: &str, from: &str, to: &str| {
        let text = std::fs::read_to_string(content.join(path)).expect("a shipped material");
        assert!(text.contains(from), "{path}");
        Kind::Material.value(&text.replacen(from, to, 1)).unwrap_or_else(|| panic!("{path} reads"))
    };
    let surfaces = [
        read(
            "lookdev/character/materials/Face_00_SKIN.material.ron",
            "through_hair: None,",
            "through_hair: Some((opacity: 0.5, fade_yaw: 70.0, fade_pitch: 45.0, distance: 0.15)),",
        ),
        read("combat/hero/hero_hair.material.ron", "", ""),
        read("lookdev/character/materials/EyeIris_00_EYE.material.ron", "", ""),
        read("lookdev/character/materials/EyeHighlight_00_EYE.material.ron", "", ""),
    ];
    for field in &fields {
        let written: PropertyPath = field.parse().unwrap_or_else(|_| panic!("`{field}` is a path"));
        let resolved: Vec<String> = surfaces
            .iter()
            .filter_map(|surface| walk::named(surface.as_ref(), &written).ok())
            .map(|path| path.to_string())
            .collect();
        assert!(!resolved.is_empty(), "`{field}` is on no surface");
        assert!(resolved.iter().all(|path| path.starts_with("surface[0].")), "{field}: {resolved:?}");
    }
}
