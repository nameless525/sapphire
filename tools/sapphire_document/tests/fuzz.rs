//! The fuzz the design leans on, over the real documents: every property of every file takes edits, each of
//! which either lands and undoes to the same bytes or is refused with the text as it was, and random runs of
//! edits undo and redo to the same bytes.

#![allow(clippy::print_stdout, reason = "with --nocapture, its numbers are what it is run for")]

mod support;

use bevy_reflect::{FromReflect, GetTypeRegistration, PartialReflect, Reflect, ReflectRef, Typed};
use sapphire_assets::AssetId;
use sapphire_document::identity;
use sapphire_document::text::comments;
use sapphire_document::walk::{self, PropertyKind};
use sapphire_document::{
    AnyDocument, Document, DocumentSet, HistoryError, Kind, Outcome, Property, PropertyPath, Refused, Schema,
    Step, Took, kinds, write,
};
use sapphire_time::Stopwatch;
use support::{Random, Tally, another_value, open, value_of};

/// How many values each property is tried with: enough to go through a float's hard spellings.
const CHOICES_PER_PROPERTY: usize = 8;

/// Whether to try everything: `SAPPHIRE_DOCUMENT_FUZZ=full`, some 22,000 edits and minutes of a debug build.
/// By default each test takes every seventh property, so `cargo test` stays quick and still covers each kind.
fn full() -> bool {
    std::env::var_os("SAPPHIRE_DOCUMENT_FUZZ").is_some_and(|value| value == "full")
}

fn stride() -> usize {
    if full() { 1 } else { 7 }
}

/// How many edits a random run makes before undoing them all.
fn run_length() -> usize {
    if full() { 400 } else { 80 }
}

/// Everything that must hold of one edit that landed.
fn hold_to_the_edit<T>(document: &Document<T>, old: &[&str], property: &Property)
where
    T: Reflect + FromReflect + Typed + GetTypeRegistration + Clone + PartialEq,
{
    let path = &property.path;
    let after = document.text();
    let step = document.last_step().expect("a step");
    let record = last_record(document);
    // A leaf or a variant changes inside its own bytes, and nothing around them moves. What's left out of
    // the file goes in with nothing removed.
    match (&property.written, property.kind) {
        (Some(span), PropertyKind::Leaf | PropertyKind::Enum) => {
            let changed = step.change.at..step.change.at + step.change.removed.len();
            assert!(
                span.start <= changed.start && changed.end <= span.end,
                "{path}: {changed:?} isn't inside {span:?}"
            );
        }
        (None, _) => assert_eq!(step.change.removed, "", "{path}: adding it took bytes away"),
        _ => {}
    }
    // A comment goes only with what's replaced around it, and the record says so.
    let new = comments(after);
    assert_eq!(old.len(), new.len() + record.comments_lost, "{path}: lost a comment and didn't say");
    if record.comments_lost == 0 {
        assert_eq!(old, new.as_slice(), "{path}: comments changed");
    }
}

fn last_record<T>(document: &Document<T>) -> &sapphire_document::ChangeRecord
where
    T: Reflect + FromReflect + Typed + GetTypeRegistration + Clone + PartialEq,
{
    document.last_step().expect("a step").records.last().expect("a record")
}

/// Each property, with each of its values, then undone: what the design's whole claim comes to.
fn every_property<T>(mut document: Document<T>, may_refuse: bool) -> Tally
where
    T: Reflect + FromReflect + Typed + GetTypeRegistration + Clone + PartialEq,
{
    assert_eq!(document.failure(), None, "a real file opens for editing");
    let original = document.text().to_owned();
    let original_comments = comments(&original);
    let mut tally = Tally::default();
    let properties = document.properties();
    assert!(!properties.is_empty());
    for property in properties.iter().step_by(stride()) {
        for choice in 0..CHOICES_PER_PROPERTY {
            let value = value_of(&document, property);
            let Some(new) = another_value(value, property.kind, document.registry(), choice) else {
                tally.skipped += 1;
                break;
            };
            match document.set("fuzz", &property.path, new.as_ref()) {
                Ok(Outcome::Changed) => {
                    tally.edits += 1;
                    if property.written.is_none() {
                        tally.wrote_absent += 1;
                    }
                    hold_to_the_edit(&document, &original_comments, property);
                    let record = last_record(&document);
                    tally.rewrote_whole += usize::from(record.rewrote_whole);
                    assert!(document.is_dirty(), "{}", property.path);
                    assert!(document.undo());
                    assert_eq!(document.text(), original, "{}: undo moved a byte", property.path);
                    assert!(document.redo());
                    assert!(document.undo());
                    assert_eq!(document.text(), original, "{}: redo and undo moved a byte", property.path);
                    assert!(!document.is_dirty());
                }
                Ok(Outcome::Unchanged) => tally.unchanged += 1,
                Err(Refused::Document(why)) if may_refuse => {
                    tally.refused_by_document += 1;
                    assert_eq!(document.text(), original, "{}: {why}", property.path);
                }
                Err(refused) => panic!("{}: {refused}", property.path),
            }
        }
    }
    tally
}

/// Lists: an entry copied onto each end and into the middle, each entry taken out, each moved.
fn every_list<T>(mut document: Document<T>, may_refuse: bool) -> Tally
where
    T: Reflect + FromReflect + Typed + GetTypeRegistration + Clone + PartialEq,
{
    let original = document.text().to_owned();
    let mut tally = Tally::default();
    let lists: Vec<_> =
        document.properties().into_iter().filter(|property| property.kind == PropertyKind::List).collect();
    let mut outcome =
        |document: &mut Document<T>, path: &PropertyPath, result: Result<Outcome, Refused>| match result {
            Ok(Outcome::Changed) => {
                tally.edits += 1;
                assert!(document.undo());
                assert_eq!(document.text(), original, "{path}: undo moved a byte");
            }
            Ok(Outcome::Unchanged) => tally.unchanged += 1,
            Err(Refused::Document(why)) if may_refuse => {
                tally.refused_by_document += 1;
                assert_eq!(document.text(), original, "{path}: {why}");
            }
            Err(refused) => panic!("{path}: {refused}"),
        };
    for list in lists.iter().step_by(stride()) {
        let length = list_length(value_of(&document, list));
        for at in 0..length {
            let entry = document.entry_path(&list.path, at).expect("an entry");
            let result = document.remove("fuzz", &entry);
            outcome(&mut document, &entry, result);
            let result = document.reorder("fuzz", &list.path, at, (at + 1) % length);
            outcome(&mut document, &list.path, result);
        }
        if length > 0 {
            for at in [0, length / 2, length] {
                let copy = {
                    let value = value_of(&document, list);
                    let ReflectRef::List(items) = value.reflect_ref() else { panic!("a list") };
                    items.get(0).expect("an entry").to_dynamic().expect("clones")
                };
                let result = document.insert("fuzz", &list.path, at, copy.as_ref());
                outcome(&mut document, &list.path, result);
            }
        }
    }
    tally
}

fn list_length(value: &dyn PartialReflect) -> usize {
    match walk::through(value).reflect_ref() {
        ReflectRef::List(items) => items.len(),
        _ => panic!("a list"),
    }
}

/// A run of edits on top of each other, then all of it undone, redone and undone again.
fn a_random_run<T>(mut document: Document<T>, seed: u64, may_refuse: bool) -> Tally
where
    T: Reflect + FromReflect + Typed + GetTypeRegistration + Clone + PartialEq,
{
    let original = document.text().to_owned();
    let mut random = Random(seed);
    let mut tally = Tally::default();
    let mut texts = Vec::with_capacity(run_length());
    for _ in 0..run_length() {
        let properties = document.properties();
        let property = &properties[random.below(properties.len())];
        let before = document.text().to_owned();
        let result = match property.kind {
            PropertyKind::List => {
                let length = list_length(value_of(&document, property));
                match (length, random.below(3)) {
                    (0, _) => continue,
                    (_, 0) => {
                        let entry =
                            document.entry_path(&property.path, random.below(length)).expect("an entry");
                        document.remove("fuzz", &entry)
                    }
                    (_, 1) => {
                        document.reorder("fuzz", &property.path, random.below(length), random.below(length))
                    }
                    _ => {
                        let copy = {
                            let ReflectRef::List(items) = value_of(&document, property).reflect_ref() else {
                                panic!("a list")
                            };
                            items.get(random.below(length)).expect("an entry").to_dynamic().expect("clones")
                        };
                        document.insert("fuzz", &property.path, random.below(length + 1), copy.as_ref())
                    }
                }
            }
            kind => {
                let value = value_of(&document, property);
                let Some(new) = another_value(value, kind, document.registry(), random.below(16)) else {
                    continue;
                };
                document.set("fuzz", &property.path, new.as_ref())
            }
        };
        match result {
            Ok(Outcome::Changed) => {
                tally.edits += 1;
                texts.push(before);
            }
            Ok(Outcome::Unchanged) => tally.unchanged += 1,
            Err(Refused::Document(why)) if may_refuse || is_a_duplicate(&why) => {
                tally.refused_by_document += 1;
                assert_eq!(document.text(), before, "{why}");
            }
            Err(refused) => panic!("{}: {refused}", property.path),
        }
    }
    let last = document.text().to_owned();
    for before in texts.iter().rev() {
        assert!(document.undo());
        assert_eq!(document.text(), before, "an undo in a run moved a byte");
    }
    assert_eq!(document.text(), original);
    while document.redo() {}
    assert_eq!(document.text(), last, "redo got somewhere else");
    while document.undo() {}
    assert_eq!(document.text(), original);
    tally
}

/// A document's parser may reject two fields of one name, though nothing here writes them.
fn is_a_duplicate(why: &str) -> bool {
    why.contains("duplicate")
}

fn run<T>(name: &str, relative: &str, schema: Schema<T>, may_refuse: bool)
where
    T: Reflect + FromReflect + Typed + GetTypeRegistration + Clone + PartialEq,
{
    assert!(fuzz(name, relative, schema, may_refuse) > 200, "{name}: the fuzz did too little");
}

/// The fuzz over one file, and how many edits landed.
fn fuzz<T>(name: &str, relative: &str, schema: Schema<T>, may_refuse: bool) -> usize
where
    T: Reflect + FromReflect + Typed + GetTypeRegistration + Clone + PartialEq,
{
    let clock = Stopwatch::start();
    let each = every_property(open(relative, schema), may_refuse);
    let lists = every_list(open(relative, schema), may_refuse);
    let mut random = Tally::default();
    for seed in 1..=3 {
        let run = a_random_run(open(relative, schema), seed, may_refuse);
        random.edits += run.edits;
        random.refused_by_document += run.refused_by_document;
    }
    println!(
        "{name}: {} property edits ({} of what the file leaves out, {} rewrote a whole value, {} refused by the \
         document, {} unchanged, {} properties with nothing to try), {} list edits ({} refused), {} in random \
         runs ({} refused), {:.1} s",
        each.edits,
        each.wrote_absent,
        each.rewrote_whole,
        each.refused_by_document,
        each.unchanged,
        each.skipped,
        lists.edits,
        lists.refused_by_document,
        random.edits,
        random.refused_by_document,
        clock.elapsed().as_secs_f64(),
    );
    each.edits + lists.edits + random.edits
}

#[test]
fn every_property_of_the_players_moves_undoes_to_the_same_bytes() {
    run("player.moves.ron", "content/combat/player/player.moves.ron", kinds::moves(), false);
}

#[test]
fn every_property_of_the_warriors_moves_undoes_to_the_same_bytes() {
    run("warrior.moves.ron", "content/combat/enemy/warrior.moves.ron", kinds::moves(), false);
}

#[test]
fn every_property_of_the_characters_graph_undoes_to_the_same_bytes() {
    run(
        "character.animgraph.ron",
        "content/lookdev/character/character.animgraph.ron",
        kinds::graph(),
        false,
    );
}

#[test]
fn every_property_of_a_cue_undoes_to_the_same_bytes() {
    run("hit_heavy.cue.ron", "content/audio/hit_heavy.cue.ron", kinds::cue(), false);
}

#[test]
fn every_property_of_a_timeline_undoes_to_the_same_bytes() {
    run(
        "cutscene.timeline.ron",
        "crates/sapphire_timeline/test_content/cutscene.timeline.ron",
        kinds::timeline(),
        false,
    );
}

#[test]
fn every_property_of_a_screen_undoes_to_the_same_bytes() {
    run("pause.ui.ron", "content/ui/pause.ui.ron", kinds::ui(), false);
    run("settings.ui.ron", "content/ui/settings.ui.ron", kinds::ui(), false);
    run("character.ui.ron", "content/ui/character.ui.ron", kinds::ui(), false);
}

/// A zone checks everything as it reads, so a value it can't take is refused with the text as it was.
#[test]
fn every_property_of_a_zone_undoes_or_is_refused() {
    run(
        "proving_ground.zone.ron",
        "content/zones/proving_ground/proving_ground.zone.ron",
        kinds::zone(),
        true,
    );
}

/// No shipped zone names foliage yet (M6 G1a), so the fuzz runs on the placement tests' grove. Its checks run beside
/// its reading, so no edit is refused.
#[test]
fn every_property_of_a_foliage_document_undoes_to_the_same_bytes() {
    run("grove.foliage.ron", "crates/sapphire_zone/testdata/grove.foliage.ron", kinds::foliage(), false);
}

/// The library checks everything as it reads, so many values are refused here, each with the text as it
/// was, and that's its rule at work.
#[test]
fn every_property_of_the_default_effects_undoes_or_is_refused() {
    run("default_effects.ron", "crates/sapphire_vfx/default_effects.ron", kinds::effects(), true);
}

/// Every shipped document of `kind`: each in the content and the benchmark's, and those compiled in beside a
/// crate (`extra`).
fn shipped(kind: Kind, extra: &[&str]) -> Vec<String> {
    let mut found: Vec<String> = documents_under_committed()
        .into_iter()
        .filter(|(_, of)| *of == kind)
        .map(|(relative, _)| relative)
        .chain(extra.iter().map(|&relative| relative.to_owned()))
        .collect();
    found.sort();
    assert!(!found.is_empty(), "{kind:?}: none shipped");
    found
}

/// Each shipped document of a kind T4 added, fuzzed whole: every property edit undoes to its bytes, and each
/// edit the kind's reading refuses leaves the text as it was. A small file has little to edit, so the kind's
/// files are held to the fuzz's floor together.
fn every_shipped<T>(kind: Kind, extra: &[&str], schema: Schema<T>, may_refuse: bool)
where
    T: Reflect + FromReflect + Typed + GetTypeRegistration + Clone + PartialEq,
{
    let edits: usize =
        shipped(kind, extra).iter().map(|relative| fuzz(relative, relative, schema, may_refuse)).sum();
    assert!(edits > 200, "{kind:?}: the fuzz did too little");
}

#[test]
fn every_property_of_every_character_material_undoes_to_the_same_bytes() {
    every_shipped(Kind::Material, &[], kinds::material(), false);
}

#[test]
fn every_property_of_every_environment_material_undoes_to_the_same_bytes() {
    every_shipped(Kind::EnvironmentMaterial, &[], kinds::environment_material(), false);
}

#[test]
fn every_property_of_every_ramp_undoes_to_the_same_bytes() {
    every_shipped(Kind::Ramp, &[], kinds::ramp(), false);
}

#[test]
fn every_property_of_every_prefab_undoes_to_the_same_bytes() {
    every_shipped(Kind::Assembly, &[], kinds::assembly(), false);
}

#[test]
fn every_property_of_every_encounter_undoes_to_the_same_bytes() {
    every_shipped(Kind::Encounter, &[], kinds::encounter(), false);
}

#[test]
fn every_property_of_every_dialogue_undoes_to_the_same_bytes() {
    every_shipped(Kind::Dialogue, &[], kinds::dialogue(), false);
}

/// A day checks everything as it reads, so a value it can't take is refused with the text as it was.
#[test]
fn every_property_of_every_day_undoes_or_is_refused() {
    every_shipped(Kind::Day, &["crates/sapphire_world/default_day.ron"], kinds::day(), true);
}

/// A library of light rigs checks everything as it reads, likewise.
#[test]
fn every_property_of_every_light_rig_library_undoes_or_is_refused() {
    every_shipped(
        Kind::LightRigs,
        &["crates/sapphire_world/default_light_rigs.ron"],
        kinds::light_rigs(),
        true,
    );
}

/// The cue table checks everything as it reads (M6 T19), likewise.
#[test]
fn every_property_of_every_cue_table_undoes_or_is_refused() {
    every_shipped(Kind::CueTable, &["crates/sapphire_cues/default_cues.ron"], kinds::cue_table(), true);
}

/// A zone's colliders, which T2 made a kind so a placement moves with what it names.
#[test]
fn every_property_of_every_colliders_document_undoes_to_the_same_bytes() {
    every_shipped(Kind::Colliders, &[], kinds::colliders(), false);
}

/// A path goes through its text and back.
#[test]
fn every_path_prints_and_parses_back() {
    let document = open("content/combat/player/player.moves.ron", kinds::moves());
    for property in document.properties() {
        let printed = property.path.to_string();
        assert_eq!(printed.parse::<PropertyPath>().expect(&printed), property.path);
    }
    let steps: Vec<_> = document.properties().into_iter().flat_map(|p| p.path.steps().to_vec()).collect();
    assert!(steps.iter().any(|step| matches!(step, Step::Key(_))));
    assert!(steps.iter().any(|step| matches!(step, Step::Some)));
}

/// Where the documents a person edits are committed: the content, and the benchmark's documents (`bench/`), which
/// its generator reads and T12 opens in the editor.
const COMMITTED: [&str; 2] = ["content", "bench"];

/// Every file under [`COMMITTED`] a kind knows, by its path from the repository's root, in no order. It walks
/// the folders rather than asking git which files are tracked, so the tests read files and not a repository,
/// and run the same from a source archive: nothing under them is untracked or ignored in a checkout (what's
/// cooked from them goes elsewhere), and a document added but not yet committed is one more to hold to the walker.
fn documents_under_committed() -> Vec<(String, Kind)> {
    let mut found = Vec::new();
    let mut stack: Vec<_> = COMMITTED.iter().map(|folder| support::repository_root().join(folder)).collect();
    while let Some(folder) = stack.pop() {
        for entry in std::fs::read_dir(folder).expect("a folder") {
            let path = entry.expect("an entry").path();
            if path.is_dir() {
                stack.push(path);
                continue;
            }
            let Some(kind) = Kind::of(&path) else { continue };
            let relative = path.strip_prefix(support::repository_root()).expect("inside");
            found.push((relative.to_string_lossy().into_owned(), kind));
        }
    }
    found
}

/// The walker agrees with every committed document of each kind, the benchmark's too, not just the one a kind's
/// fuzz runs on: a file it disagrees with opens read only, which nothing else would notice.
#[test]
fn every_committed_document_opens_for_editing() {
    let (mut seen, mut count, mut benchmark) = (std::collections::BTreeSet::new(), 0, 0);
    for (relative, kind) in documents_under_committed() {
        benchmark += usize::from(relative.starts_with("bench"));
        let failure = match kind {
            Kind::Moves => open(&relative, kinds::moves()).failure().cloned(),
            Kind::Graph => open(&relative, kinds::graph()).failure().cloned(),
            Kind::Cue => open(&relative, kinds::cue()).failure().cloned(),
            Kind::Effects => open(&relative, kinds::effects()).failure().cloned(),
            Kind::Timeline => open(&relative, kinds::timeline()).failure().cloned(),
            Kind::Ui => open(&relative, kinds::ui()).failure().cloned(),
            Kind::Zone => open(&relative, kinds::zone()).failure().cloned(),
            Kind::Material => open(&relative, kinds::material()).failure().cloned(),
            Kind::EnvironmentMaterial => open(&relative, kinds::environment_material()).failure().cloned(),
            Kind::Ramp => open(&relative, kinds::ramp()).failure().cloned(),
            Kind::Assembly => open(&relative, kinds::assembly()).failure().cloned(),
            Kind::Encounter => open(&relative, kinds::encounter()).failure().cloned(),
            Kind::Dialogue => open(&relative, kinds::dialogue()).failure().cloned(),
            Kind::Day => open(&relative, kinds::day()).failure().cloned(),
            Kind::LightRigs => open(&relative, kinds::light_rigs()).failure().cloned(),
            Kind::Colliders => open(&relative, kinds::colliders()).failure().cloned(),
            Kind::Foliage => open(&relative, kinds::foliage()).failure().cloned(),
            Kind::CueTable => open(&relative, kinds::cue_table()).failure().cloned(),
        };
        assert_eq!(failure, None, "{relative}");
        seen.insert(kind);
        count += 1;
    }
    let missing: Vec<_> = Kind::ALL.into_iter().filter(|kind| !seen.contains(kind)).collect();
    assert_eq!(
        missing,
        [Kind::Effects, Kind::Foliage, Kind::CueTable],
        "every kind but effects, foliage and cue tables, which the content has none of (no shipped zone grows plants \
         yet, M6 G1a; the cue table is compiled in, M6 T19)"
    );
    assert!(count >= 230, "{count}");
    // The district's ten prefabs, and the district's and the nature zone's days.
    assert!(benchmark >= 12, "{benchmark}");
}

/// A day and a library of light rigs have no cook's sidecar, so the editor's first open of one would write a
/// sidecar saying only which document it is: each committed one has it committed, as the effects library's is.
/// The benchmark's documents are left out: its generator names its sources by IDs of its own (`bench::asset_id`).
#[test]
fn every_committed_day_and_light_rig_library_has_its_sidecar() {
    let extras = ["crates/sapphire_world/default_day.ron", "crates/sapphire_world/default_light_rigs.ron"];
    let documents: Vec<String> = documents_under_committed()
        .into_iter()
        .filter(|(relative, kind)| {
            matches!(kind, Kind::Day | Kind::LightRigs) && relative.starts_with("content")
        })
        .map(|(relative, _)| relative)
        .chain(extras.map(str::to_owned))
        .collect();
    assert!(documents.len() >= 9, "{documents:?}");
    for relative in documents {
        let sidecar = identity::sidecar_path(&support::repository_root().join(&relative));
        let text = std::fs::read_to_string(&sidecar).unwrap_or_else(|error| panic!("{relative}: {error}"));
        assert!(text.contains(identity::DOCUMENT_TYPE), "{relative}: a document's sidecar, with no importer");
        identity::read_id(&sidecar).unwrap_or_else(|error| panic!("{relative}: {error}"));
    }
}

/// Every field a cook's refusal can name of a material (its texture slots' fields, M6 T5) is a path the walker
/// parses, and finds in a shipped document that has it: what Properties marks, and a click will select (T6).
#[test]
fn every_field_a_cook_refusal_names_is_a_path_the_walker_finds() {
    use sapphire_assets::material::MaterialDocument;
    fn look<T>(
        document: &Document<T>,
        named: &mut std::collections::BTreeSet<&'static str>,
        found: &mut std::collections::BTreeSet<&'static str>,
    ) where
        T: MaterialDocument + Reflect + FromReflect + Typed + GetTypeRegistration + Clone + PartialEq,
    {
        let value = document.value().expect("a document that reads");
        for field in value.texture_fields() {
            let path: PropertyPath = field.parse().unwrap_or_else(|error| panic!("{field}: {error}"));
            named.insert(field);
            if walk::get(value.as_partial_reflect(), &path).is_ok() {
                found.insert(field);
            }
        }
    }
    let (mut named, mut found) = (std::collections::BTreeSet::new(), std::collections::BTreeSet::new());
    for (relative, kind) in documents_under_committed() {
        match kind {
            Kind::Material => look(&open(&relative, kinds::material()), &mut named, &mut found),
            Kind::EnvironmentMaterial => {
                look(&open(&relative, kinds::environment_material()), &mut named, &mut found);
            }
            _ => {}
        }
    }
    let missing: Vec<_> = named.difference(&found).collect();
    assert!(missing.is_empty(), "named but found in no document: {missing:?}");
    assert!(found.len() >= 12, "{found:?}");
}

/// A material's kind is its envelope's family, and a family with no kind here is no kind.
#[test]
fn a_materials_kind_is_the_family_its_envelope_names() {
    let mut families = std::collections::BTreeMap::new();
    for (relative, kind) in documents_under_committed() {
        if relative.ends_with(".material.ron") {
            *families.entry(kind).or_insert(0) += 1;
        }
    }
    assert!(families[&Kind::Material] >= 50 && families[&Kind::EnvironmentMaterial] >= 30, "{families:?}");
    assert_eq!(families.len(), 2, "{families:?}");
    let sky = support::read("content/samples/sky.material.ron");
    assert_eq!(Kind::of_material(&sky), None, "a sky's material has no kind yet");
    assert_eq!(Kind::of(std::path::Path::new("nowhere/missing.material.ron")), None);
}

/// What one edit costs, for a person dragging: it prints, and asserts nothing, since this Mac's load moves it.
#[test]
fn what_an_edit_costs() {
    let mut document = open("content/combat/player/player.moves.ron", kinds::moves());
    let path: PropertyPath = "moves[0].startup".parse().expect("a path");
    let original = document.text().to_owned();
    let clock = Stopwatch::start();
    const EDITS: u32 = 200;
    for tick in 0..EDITS {
        let value = (tick % 40 + 1).to_string();
        assert_eq!(document.set_text("edit", &path, &value), Ok(Outcome::Changed));
    }
    let edit = clock.elapsed().as_secs_f64() / f64::from(EDITS);
    let clock = Stopwatch::start();
    let mut undone = 0;
    while document.undo() {
        undone += 1;
    }
    let undo = clock.elapsed().as_secs_f64() / f64::from(undone);
    assert_eq!(undone, EDITS);
    assert_eq!(document.text(), original);
    println!(
        "player.moves.ron, {} bytes: {:.2} ms an edit, {:.2} ms an undo",
        document.text().len(),
        edit * 1000.0,
        undo * 1000.0
    );
}

/// A copy of each list's first entry written new, as a value: it comes out spelling no field its type reads
/// the same without, in all five documents. A list of `None`s or a required `None` would stay, and none does.
fn entries_written_new<T>(mut document: Document<T>, may_refuse: bool)
where
    T: Reflect + FromReflect + Typed + GetTypeRegistration + Clone + PartialEq,
{
    let lists: Vec<_> =
        document.properties().into_iter().filter(|property| property.kind == PropertyKind::List).collect();
    let mut wrote = 0;
    for list in &lists {
        let length = list_length(value_of(&document, list));
        if length == 0 {
            continue;
        }
        let copy = {
            let value = value_of(&document, list);
            let ReflectRef::List(items) = value.reflect_ref() else { panic!("a list") };
            items.get(0).expect("an entry").to_dynamic().expect("clones")
        };
        match document.insert("copy", &list.path, length, copy.as_ref()) {
            Ok(Outcome::Changed) => {
                wrote += 1;
                let inserted = &document.last_step().expect("a step").change.inserted;
                assert!(!inserted.contains("None"), "{}: {inserted}", list.path);
                assert!(!inserted.contains(": []"), "{}: {inserted}", list.path);
                assert!(document.undo());
            }
            Ok(Outcome::Unchanged) => {}
            Err(Refused::Document(_)) if may_refuse => {}
            Err(refused) => panic!("{}: {refused}", list.path),
        }
    }
    assert!(wrote > 0);
}

#[test]
fn entries_written_new_spell_only_what_they_have_to() {
    entries_written_new(open("content/combat/player/player.moves.ron", kinds::moves()), false);
    entries_written_new(open("content/combat/enemy/warrior.moves.ron", kinds::moves()), false);
    entries_written_new(open("content/lookdev/character/character.animgraph.ron", kinds::graph()), false);
    entries_written_new(open("crates/sapphire_vfx/default_effects.ron", kinds::effects()), true);
}

/// A folder of copies of real documents, gone when the test ends.
struct Copies(std::path::PathBuf);

impl Copies {
    fn new(name: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("sapphire_document_fuzz_{name}_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("a temp dir");
        Self(dir)
    }
}

impl Drop for Copies {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// One edit of a property, chosen at random, that isn't a list's: what lands, or is refused with the text as it
/// was. `None` if it found nothing to try.
fn a_random_edit(document: &mut dyn AnyDocument, random: &mut Random) -> Option<Result<Outcome, Refused>> {
    let properties: Vec<_> =
        document.properties().into_iter().filter(|property| property.kind != PropertyKind::List).collect();
    let property = &properties[random.below(properties.len())];
    let ron = {
        let value = walk::get(document.value_reflect()?, &property.path).ok()?;
        let new = another_value(value, property.kind, document.registry(), random.below(16))?;
        write::canonical(new.as_ref(), document.registry()).ok()?
    };
    Some(document.set_ron("fuzz", &property.path, &ron))
}

/// Whether an undo or a redo did something. One refused because another document has a step of its own to
/// take first did nothing, and the loop that called it goes round again.
fn went(result: Result<Took, HistoryError>) -> bool {
    match result {
        Ok(took) => took.moved(),
        Err(HistoryError::Later { .. }) => false,
        Err(error) => panic!("{error}"),
    }
}

/// Undoes every document as far as it goes, set steps whole, until none can go further.
fn undo_everything(set: &mut DocumentSet, ids: &[AssetId]) {
    loop {
        let mut moved = false;
        for id in ids {
            while went(set.undo(*id)) {
                moved = true;
            }
        }
        if !moved {
            break;
        }
    }
}

/// The shipped documents the mixed fuzz copies: two kinds of moves, a cue, a ramp and a zone's colliders.
const MIXED: [(&str, &str); 5] = [
    ("player.moves.ron", "content/combat/player/player.moves.ron"),
    ("warrior.moves.ron", "content/combat/enemy/warrior.moves.ron"),
    ("hit_heavy.cue.ron", "content/audio/hit_heavy.cue.ron"),
    ("swatch.ramp.ron", "content/samples/swatch.ramp.ron"),
    ("lantern_ward.colliders.ron", "content/zones/lantern_ward/lantern_ward.colliders.ron"),
];

/// What the mixed fuzz did, for its report.
#[derive(Debug, Default)]
struct Mixed {
    own: usize,
    set_steps: usize,
    gestures: usize,
    refused_whole: usize,
    saves: usize,
    held: usize,
}

/// Mixed steps over several documents at once (M6 T2): edits of one document, set steps across several, set
/// gestures over a few edits, undos and redos of either at random, and saves between. Whatever happens, an
/// undo or a redo is whole or refused with every text as it was, a save writes its own file alone, and at the
/// end every document undoes to the bytes it opened with, which its file holds once saved.
#[test]
fn mixed_steps_across_documents_undo_every_file_to_its_bytes() {
    let mut tally = Mixed::default();
    for seed in 1..=3_u64 {
        mixed_run(seed, &mut tally);
    }
    println!("mixed steps: {tally:?}");
    assert!(tally.set_steps > 20 && tally.saves > 10 && tally.held > 5, "the fuzz did too little: {tally:?}");
}

fn mixed_run(seed: u64, tally: &mut Mixed) {
    let copies = Copies::new(&format!("mixed_{seed}"));
    let mut set = DocumentSet::new(vec![copies.0.clone()]);
    let (mut ids, mut originals) = (Vec::new(), Vec::new());
    for (name, relative) in MIXED {
        let path = copies.0.join(name);
        std::fs::write(&path, support::read(relative)).expect("a copy");
        let kind = Kind::of(&path).expect("a kind");
        ids.push(kind.open(&mut set, &path).expect("opens"));
        originals.push(support::read(relative));
    }
    let texts = |set: &DocumentSet| -> Vec<String> {
        ids.iter().map(|id| set.any(*id).expect("open").text().to_owned()).collect()
    };
    let on_disk = |set: &DocumentSet, id: AssetId| std::fs::read(set.path(id).expect("a path")).ok();
    let mut random = Random(seed);
    for _ in 0..run_length() {
        let before = texts(&set);
        let id = ids[random.below(ids.len())];
        match random.below(10) {
            // One document's own step.
            0..=2 => {
                if let Some(Ok(Outcome::Changed)) = a_random_edit(set.edit(id).expect("open"), &mut random) {
                    tally.own += 1;
                }
            }
            // A set step: each of two to four documents takes an edit, or none does.
            3 | 4 => {
                let picked: Vec<AssetId> =
                    (0..2 + random.below(3)).map(|_| ids[random.below(ids.len())]).collect();
                let result = set.together("fuzz together", |set| {
                    for id in &picked {
                        if let Some(Err(refused)) = a_random_edit(set.edit(*id).expect("open"), &mut random) {
                            return Err(refused);
                        }
                    }
                    Ok(())
                });
                match result {
                    Ok(()) => tally.set_steps += 1,
                    Err(_) => assert_eq!(texts(&set), before, "a refused set step changed a text"),
                }
            }
            // A set gesture over a few edits, a refused one leaving its text as it was.
            5 => {
                set.begin_gesture("fuzz gesture");
                for _ in 0..1 + random.below(4) {
                    let id = ids[random.below(ids.len())];
                    let _ = a_random_edit(set.edit(id).expect("open"), &mut random);
                }
                set.end_gesture();
                tally.gestures += 1;
            }
            6 | 7 => {
                let result = if random.below(3) == 0 { set.redo(id) } else { set.undo(id) };
                if result.is_err() {
                    tally.refused_whole += 1;
                    assert_eq!(texts(&set), before, "a refused undo or redo changed a text");
                }
            }
            // A save of one: its file holds its text, and no other file moves.
            _ => {
                let others = |set: &DocumentSet| -> Vec<_> {
                    ids.iter().filter(|other| **other != id).map(|other| on_disk(set, *other)).collect()
                };
                let was = others(&set);
                set.save(id).expect("saves");
                assert_eq!(on_disk(&set, id).as_deref(), Some(set.any(id).expect("open").text().as_bytes()));
                assert_eq!(others(&set), was, "a save wrote another file");
                tally.saves += 1;
            }
        }
    }
    tally.held += set.stats().set_steps;
    undo_everything(&mut set, &ids);
    assert_eq!(texts(&set), originals, "seed {seed}: undoing everything didn't give the bytes back");
    assert!(set.save_all().is_empty());
    for (id, original) in ids.iter().zip(&originals) {
        assert_eq!(on_disk(&set, *id).as_deref(), Some(original.as_bytes()), "seed {seed}");
    }
    // Everything redone that can be, then undone again, is the same bytes once more.
    loop {
        let mut moved = false;
        for id in &ids {
            while set.redo(*id).is_ok_and(|took| took.moved()) {
                moved = true;
            }
        }
        if !moved {
            break;
        }
    }
    undo_everything(&mut set, &ids);
    assert_eq!(texts(&set), originals, "seed {seed}: redoing and undoing again moved a byte");
}
