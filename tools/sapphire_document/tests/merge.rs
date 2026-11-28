//! The merge driver's merge (M6 T7): two sides' changes by property path, laid on ours, a marker where they clash,
//! and a line merge, said so, where there's no model. A small zone shows each rule; the corpus holds every kind
//! of document to the row's three cases.

mod support;

use std::path::Path;
use std::sync::Arc;

use bevy_reflect::{
    FromReflect, GetTypeRegistration, PartialReflect, Reflect, ReflectMut, ReflectRef, Typed,
};
use sapphire_document::merge::{How, Merged, Side, merge};
use sapphire_document::problem::{self, At};
use sapphire_document::walk::{self, PropertyKind};
use sapphire_document::{Document, Kind, Outcome, Property, PropertyPath, Schema, Step, kinds, registry_of};
use sapphire_zone::ZoneDocument;
use support::{another_value, value_of};

/// A version 2 zone: comments, entries on lines of their own and one with fields on several lines, a list on one
/// line, and IDs in four lists.
const YARD: &str = r##"// A yard.
(
    type: "sapphire::zone::ZoneDocument",
    version: 2,
    data: (
        grid: (cell_size: 16.0),
        placements: [
            // A post.
            (id: "post", prefab: "1ff3290e-5fa9-4f61-b891-2ca6d76b285f", at: (-6.0, 0.0, -6.0)),
            (
                id: "gate",
                // A turned one.
                prefab: "1ff3290e-5fa9-4f61-b891-2ca6d76b285f",
                at: (6.0, 0.0, -6.0),
                turn: 30.0,
            ),
            (id: "well", prefab: "1ff3290e-5fa9-4f61-b891-2ca6d76b285f", at: (0.0, 0.0, 6.0)),
        ],
        instances: [(id: "crate", mesh: "f6dcae8b-5937-4299-ba5e-e2f026fc2762", at: (4.0, 0.0, 6.0))],
        lights: [
            (id: "lamp", position: (4.0, 2.5, 6.0), color: (1.0, 0.72, 0.42), intensity: 6.0, range: 8.0),
        ],
        spawners: [],
    ),
)
"##;

const ZONE: &str = "content/zones/yard/yard.zone.ron";

fn path(text: &str) -> PropertyPath {
    text.parse().expect(text)
}

fn read_zone(text: &str) -> ZoneDocument {
    ZoneDocument::from_ron(text).unwrap_or_else(|error| panic!("{error}\n{text}"))
}

/// `text` with every marker resolved to one side's section.
fn resolve(text: &str, keep: Side) -> String {
    #[derive(PartialEq)]
    enum In {
        Text,
        Ours,
        Base,
        Theirs,
    }
    let mut state = In::Text;
    let mut out = String::new();
    for line in text.split_inclusive('\n') {
        let bare = line.trim_end();
        if bare.starts_with("<<<<<<< ") {
            state = In::Ours;
        } else if bare.starts_with("||||||| ") {
            state = In::Base;
        } else if bare == "=======" {
            state = In::Theirs;
        } else if bare.starts_with(">>>>>>> ") {
            state = In::Text;
        } else if state == In::Text
            || (state == In::Ours && keep == Side::Ours)
            || (state == In::Theirs && keep == Side::Theirs)
        {
            out.push_str(line);
        }
    }
    out
}

fn by_path(merged: &Merged) -> usize {
    match merged.how {
        How::ByPath { laid, .. } => laid,
        ref how => panic!("{how:?}:\n{}", merged.text),
    }
}

/// Both sides add an entry to the same list: both stay, ours first, each as its side wrote it, with the comment
/// they wrote above theirs; and their comment on an entry nobody's values changed is carried.
#[test]
fn entries_both_sides_add_both_go_in_with_their_comments() {
    let ours = YARD.replace(
        "            (id: \"well\", prefab: \"1ff3290e-5fa9-4f61-b891-2ca6d76b285f\", at: (0.0, 0.0, 6.0)),\n",
        "            (id: \"well\", prefab: \"1ff3290e-5fa9-4f61-b891-2ca6d76b285f\", at: (0.0, 0.0, 6.0)),\n            \
         // Ours.\n            (id: \"bench\", prefab: \"1ff3290e-5fa9-4f61-b891-2ca6d76b285f\", at: (2.0, 0.0, 2.0)),\n",
    );
    let theirs = YARD
        .replace("            // A post.\n", "            // A post, by the road.\n")
        .replace(
            "            (id: \"well\", prefab: \"1ff3290e-5fa9-4f61-b891-2ca6d76b285f\", at: (0.0, 0.0, 6.0)),\n",
            "            (id: \"well\", prefab: \"1ff3290e-5fa9-4f61-b891-2ca6d76b285f\", at: (0.0, 0.0, 6.0)),\n            \
             // Theirs.\n            (id: \"cart\", prefab: \"1ff3290e-5fa9-4f61-b891-2ca6d76b285f\", at: (3.0, 0.0, 3.0)),\n",
        );
    let merged = merge(Path::new(ZONE), YARD, &ours, &theirs);
    assert!(merged.is_clean(), "{merged:?}");
    assert_eq!(by_path(&merged), 1);
    let expected = ours.replace("            // A post.\n", "            // A post, by the road.\n").replace(
        "(2.0, 0.0, 2.0)),\n",
        "(2.0, 0.0, 2.0)),\n            // Theirs.\n            (id: \"cart\", prefab: \
             \"1ff3290e-5fa9-4f61-b891-2ca6d76b285f\", at: (3.0, 0.0, 3.0)),\n",
    );
    assert_eq!(merged.text, expected);
}

/// Two fields of one entry, one on each side, both land, and so does a value inside a tuple (a position's `y`
/// beside the other side's `x`).
#[test]
fn fields_of_one_entry_both_sides_edit_both_land() {
    let ours = YARD.replace("                at: (6.0, 0.0, -6.0),", "                at: (7.5, 0.0, -6.0),");
    let theirs = YARD
        .replace("                turn: 30.0,", "                turn: 45.0,")
        .replace("(4.0, 2.5, 6.0), color", "(4.0, 3.5, 6.0), color");
    let merged = merge(Path::new(ZONE), YARD, &ours, &theirs);
    assert!(merged.is_clean(), "{merged:?}");
    assert_eq!(by_path(&merged), 2);
    assert_eq!(
        merged.text,
        ours.replace("                turn: 30.0,", "                turn: 45.0,")
            .replace("(4.0, 2.5, 6.0), color", "(4.0, 3.5, 6.0), color")
    );
    // A position is a tuple, so its places are its axes: one side's x beside the other's y.
    let ours = YARD.replace("at: (-6.0, 0.0, -6.0)", "at: (-5.0, 0.0, -6.0)");
    let theirs = YARD.replace("at: (-6.0, 0.0, -6.0)", "at: (-6.0, 1.0, -6.0)");
    let merged = merge(Path::new(ZONE), YARD, &ours, &theirs);
    assert!(merged.is_clean(), "{merged:?}");
    assert!(merged.text.contains("at: (-5.0, 1.0, -6.0)"), "{}", merged.text);
}

/// One field both sides set apart is a conflict, marked at that field on lines of its own, in the diff3 style;
/// the file doesn't read until it's resolved, and either side's section resolves to that side.
#[test]
fn one_field_both_sides_set_is_marked_at_the_value() {
    let ours = YARD.replace("                turn: 30.0,", "                turn: 40.0,");
    let theirs = YARD.replace("                turn: 30.0,", "                turn: 50.0,");
    let merged = merge(Path::new(ZONE), YARD, &ours, &theirs);
    assert_eq!(merged.conflicts.len(), 1, "{merged:?}");
    assert_eq!(merged.conflicts[0].at, r#"placements["gate"].turn"#);
    assert_eq!(
        merged.text,
        YARD.replace(
            "                turn: 30.0,\n",
            "<<<<<<< ours: placements[\"gate\"].turn\n                turn: 40.0,\n||||||| base\n                turn: \
             30.0,\n=======\n                turn: 50.0,\n>>>>>>> theirs\n"
        )
    );
    assert!(ZoneDocument::from_ron(&merged.text).is_err(), "a marked file doesn't read");
    assert_eq!(resolve(&merged.text, Side::Ours), ours);
    assert_eq!(resolve(&merged.text, Side::Theirs), theirs);
}

/// A field on a line with others is marked on lines of its own all the same, so the markers start lines, and a
/// side that doesn't write it has an empty section.
#[test]
fn a_value_inside_a_line_is_marked_on_lines_of_its_own() {
    let ours = YARD.replace("intensity: 6.0, range", "intensity: 7.0, range");
    let theirs = YARD.replace("intensity: 6.0, range", "intensity: 9.0, range");
    let merged = merge(Path::new(ZONE), YARD, &ours, &theirs);
    assert_eq!(merged.conflicts.len(), 1, "{merged:?}");
    assert!(
        merged.text.contains(
            "color: (1.0, 0.72, 0.42), \n<<<<<<< ours: lights[\"lamp\"].intensity\nintensity: 7.0,\n||||||| \
             base\nintensity: 6.0,\n=======\nintensity: 9.0,\n>>>>>>> theirs\n range: 8.0)"
        ),
        "{}",
        merged.text
    );
    assert_eq!(read_zone(&resolve(&merged.text, Side::Ours)), read_zone(&ours));
    assert_eq!(read_zone(&resolve(&merged.text, Side::Theirs)), read_zone(&theirs));
}

/// An entry one side took out and the other changed is a conflict on the entry: where ours took it out, the
/// marker stands where it stood, with an empty section of ours.
#[test]
fn an_entry_taken_out_and_changed_is_marked_where_it_stood() {
    let well = "            (id: \"well\", prefab: \"1ff3290e-5fa9-4f61-b891-2ca6d76b285f\", at: (0.0, 0.0, 6.0)),\n";
    let ours = YARD.replace(well, "");
    let theirs = YARD.replace("at: (0.0, 0.0, 6.0)", "at: (0.0, 0.0, 8.0)");
    let merged = merge(Path::new(ZONE), YARD, &ours, &theirs);
    assert_eq!(merged.conflicts.len(), 1, "{merged:?}");
    assert_eq!(merged.conflicts[0].at, r#"placements["well"]"#);
    assert!(ZoneDocument::from_ron(&merged.text).is_err());
    assert_eq!(resolve(&merged.text, Side::Ours), ours);
    assert_eq!(resolve(&merged.text, Side::Theirs), theirs);
    // The other way round: theirs took it out.
    let merged = merge(Path::new(ZONE), YARD, &theirs, &ours);
    assert_eq!(merged.conflicts.len(), 1, "{merged:?}");
    assert_eq!(resolve(&merged.text, Side::Ours), theirs);
    assert_eq!(resolve(&merged.text, Side::Theirs), ours);
}

/// Two entries both sides add under one ID are a conflict, never renumbered; and an ID their entry brings that
/// one of ours took in another list is refused by the zone's own reading, at their entry.
#[test]
fn an_id_both_sides_take_is_a_conflict_never_renumbered() {
    let add = |entry: &str| {
        YARD.replace(
            "            ),\n            (id: \"well\"",
            &format!("            ),\n            {entry},\n            (id: \"well\""),
        )
    };
    let ours = add(r#"(id: "shed", prefab: "1ff3290e-5fa9-4f61-b891-2ca6d76b285f", at: (1.0, 0.0, 1.0))"#);
    let theirs = add(r#"(id: "shed", prefab: "1ff3290e-5fa9-4f61-b891-2ca6d76b285f", at: (9.0, 0.0, 9.0))"#);
    let merged = merge(Path::new(ZONE), YARD, &ours, &theirs);
    assert_eq!(merged.conflicts.len(), 1, "{merged:?}");
    assert_eq!(merged.conflicts[0].at, r#"placements["shed"]"#);
    assert_eq!(resolve(&merged.text, Side::Theirs), theirs);

    let theirs = YARD.replace(
        "        lights: [\n",
        "        lights: [\n            (id: \"shed\", position: (0.0, 1.0, 0.0), color: (1.0, 1.0, 1.0), intensity: 2.0, range: \
         4.0),\n",
    );
    let merged = merge(Path::new(ZONE), YARD, &ours, &theirs);
    assert_eq!(merged.conflicts.len(), 1, "{merged:?}");
    assert_eq!(merged.conflicts[0].at, r#"lights["shed"]"#);
    assert!(merged.conflicts[0].why.as_deref().is_some_and(|why| why.contains("shed")), "{merged:?}");
    assert!(ZoneDocument::from_ron(&merged.text).is_err());
}

/// What the model can't merge by path merges by lines, and says why: sides at different versions, a side that
/// doesn't read, a file of no kind. A clean line merge that doesn't read is a conflict on the whole document.
#[test]
fn what_has_no_model_merges_by_lines_and_says_so() {
    let ours = YARD.replace("cell_size: 16.0", "cell_size: 32.0");
    let older = YARD.replace("version: 2", "version: 1").replace("intensity: 6.0", "intensity: 5.0");
    let merged = merge(Path::new(ZONE), YARD, &ours, &older);
    let How::ByLines { why } = &merged.how else { panic!("{merged:?}") };
    assert!(why.contains("different versions") && why.contains("theirs 1"), "{why}");

    let broken = YARD.replace("cell_size: 16.0", "cell_size: \"sixteen\"");
    let merged = merge(Path::new(ZONE), YARD, &ours, &broken);
    let How::ByLines { why } = &merged.how else { panic!("{merged:?}") };
    assert!(why.starts_with("theirs doesn't read"), "{why}");
    assert_eq!(merged.conflicts.len(), 1, "ours and theirs changed one line: {merged:?}");

    let merged = merge(Path::new("notes/plan.txt"), "a\nb\nc\n", "A\nb\nc\n", "a\nb\nC\n");
    assert_eq!(merged.how, How::ByLines { why: "no document kind has a model for this file".to_owned() });
    assert_eq!((merged.text.as_str(), merged.is_clean()), ("A\nb\nC\n", true));

    let lights = YARD.replace("intensity: 6.0", "intensity: 7.0,,");
    let merged = merge(Path::new(ZONE), YARD, &ours, &lights);
    assert!(
        !merged.is_clean() && merged.conflicts.last().is_some_and(|last| last.at.is_empty()),
        "{merged:?}"
    );
}

/// A side that changed nothing, or both alike, is the other's bytes.
#[test]
fn one_sided_merges_take_that_side() {
    let ours = YARD.replace("cell_size: 16.0", "cell_size: 32.0");
    assert_eq!(merge(Path::new(ZONE), YARD, &ours, YARD).how, How::Took(Side::Ours));
    assert_eq!(merge(Path::new(ZONE), YARD, &ours, &ours).how, How::Took(Side::Ours));
    let merged = merge(Path::new(ZONE), YARD, YARD, &ours);
    assert_eq!((merged.how, merged.text), (How::Took(Side::Theirs), ours));
}

/// What one case of the corpus came to, for a kind.
#[derive(Debug, Default)]
struct Ran {
    appended: Option<String>,
    fields: Option<String>,
    clash: Option<String>,
}

/// Each kind's documents: those in the content, and those compiled in beside a crate.
fn documents_of(kind: Kind) -> Vec<String> {
    let extra: &[&str] = match kind {
        Kind::Effects => &["crates/sapphire_vfx/default_effects.ron"],
        Kind::Foliage => &["crates/sapphire_zone/testdata/grove.foliage.ron"],
        Kind::CueTable => &["crates/sapphire_cues/default_cues.ron"],
        Kind::Day => &["crates/sapphire_world/default_day.ron"],
        Kind::LightRigs => &["crates/sapphire_world/default_light_rigs.ron"],
        Kind::Timeline => &["crates/sapphire_timeline/test_content/cutscene.timeline.ron"],
        _ => &[],
    };
    let mut found = Vec::new();
    let mut stack = vec![support::repository_root().join("content")];
    while let Some(folder) = stack.pop() {
        for entry in std::fs::read_dir(folder).expect("a folder") {
            let path = entry.expect("an entry").path();
            if path.is_dir() {
                stack.push(path);
            } else if Kind::of(&path) == Some(kind) {
                let relative = path.strip_prefix(support::repository_root()).expect("inside");
                found.push(relative.to_string_lossy().replace('\\', "/"));
            }
        }
    }
    found.sort();
    found.extend(extra.iter().map(|&relative| relative.to_owned()));
    found
}

/// What a merge takes `path` as part of: the first list it goes through by place, which is one value to a merge,
/// or else `path` itself.
fn unit_of(root: &dyn PartialReflect, path: &PropertyPath) -> PropertyPath {
    let mut at = root;
    for (taken, step) in path.steps().iter().enumerate() {
        if matches!(step, Step::Index(_)) && matches!(walk::through(at).reflect_ref(), ReflectRef::List(_)) {
            return path.prefix(taken);
        }
        let Some(next) = walk::child(at, step) else { break };
        at = next;
    }
    path.clone()
}

/// Whether two paths are apart: neither holds the other.
fn apart(a: &PropertyPath, b: &PropertyPath) -> bool {
    !a.steps().starts_with(b.steps()) && !b.steps().starts_with(a.steps())
}

/// Whether `path` is an entry's `id` or `name`, what a path calls it by.
fn is_a_key(path: &PropertyPath) -> bool {
    matches!(path.split_last(), Some((_, Step::Field(field))) if field == "id" || field == "name")
}

/// A value of the property other than what `document` holds, that the document takes, from the choice `from` on.
fn set_another<T>(document: &mut Document<T>, property: &Property, from: usize) -> Option<usize>
where
    T: Reflect + FromReflect + Typed + GetTypeRegistration + Clone + PartialEq,
{
    for choice in from..from + 8 {
        let value = value_of(document, property);
        let new = another_value(value, property.kind, document.registry(), choice)?;
        if let Ok(Outcome::Changed) = document.set("corpus", &property.path, new.as_ref()) {
            return Some(choice);
        }
    }
    None
}

/// The corpus's three cases on documents of one kind, each on the first document that has what it needs.
fn corpus<T>(kind: Kind, schema: Schema<T>) -> Ran
where
    T: Reflect + FromReflect + Typed + GetTypeRegistration + Clone + PartialEq,
{
    let registry = Arc::new(registry_of::<T>());
    let open = |text: &str| Document::open(text.to_owned(), schema, Arc::clone(&registry));
    let reads = |text: &str| (schema.parse)(text).unwrap_or_else(|error| panic!("{kind:?}: {error}\n{text}"));
    let mut ran = Ran::default();
    for relative in documents_of(kind) {
        let base = support::read(&relative);
        let name = Path::new(&relative);
        let document = open(&base);
        assert_eq!(document.failure(), None, "{relative}");
        let root = document.value().expect("reads").as_partial_reflect();
        let leaves: Vec<Property> = document
            .properties()
            .into_iter()
            .filter(|property| property.kind == PropertyKind::Leaf && property.written.is_some())
            .collect();

        // Both sides append a different named entry to one list.
        if ran.appended.is_none() {
            ran.appended = appended(&open, &reads, name, &base, root);
        }

        // Each side edits a different field of one entry (the deepest named one there is, or the document), apart
        // to a merge: not in one list of unnamed entries, which is one value.
        if ran.fields.is_none() {
            let mut objects: Vec<PropertyPath> = leaves.iter().map(|leaf| leaf.path.object()).collect();
            objects.sort_by_key(|object| std::cmp::Reverse(object.steps().len()));
            objects.dedup();
            'objects: for object in objects {
                // An entry's own key renames it, which is another case.
                let fields: Vec<&Property> = leaves
                    .iter()
                    .filter(|leaf| leaf.path.object() == object && !is_a_key(&leaf.path))
                    .collect();
                let pairs = fields
                    .iter()
                    .enumerate()
                    .flat_map(|(at, first)| fields[at + 1..].iter().map(move |second| (*first, *second)));
                for (first, second) in pairs.take(64) {
                    if !apart(&unit_of(root, &first.path), &unit_of(root, &second.path)) {
                        continue;
                    }
                    let (mut ours, mut theirs) = (open(&base), open(&base));
                    let Some(_) = set_another(&mut ours, first, 0) else { continue };
                    let Some(choice) = set_another(&mut theirs, second, 0) else { continue };
                    let mut expected = open(ours.text());
                    if set_another(&mut expected, second, choice) != Some(choice) {
                        continue;
                    }
                    let merged = merge(name, &base, ours.text(), theirs.text());
                    assert!(merged.is_clean(), "{relative}: {} and {}: {merged:?}", first.path, second.path);
                    assert!(matches!(merged.how, How::ByPath { laid: 1, .. }), "{relative}: {merged:?}");
                    assert!(
                        reads(&merged.text) == *expected.value().expect("reads"),
                        "{relative}: {}",
                        merged.text
                    );
                    ran.fields = Some(format!("{relative}: {} and {}", first.path, second.path));
                    break 'objects;
                }
            }
        }

        // Both sides set one field apart: a conflict, marked, which doesn't read until resolved.
        if ran.clash.is_none() {
            // Not an entry's key: two sides renaming one entry apart is a removal each and two additions.
            for leaf in leaves.iter().filter(|leaf| !is_a_key(&leaf.path)) {
                let (mut ours, mut theirs) = (open(&base), open(&base));
                let Some(choice) = set_another(&mut ours, leaf, 0) else { continue };
                if set_another(&mut theirs, leaf, choice + 1).is_none() || ours.value() == theirs.value() {
                    continue;
                }
                let merged = merge(name, &base, ours.text(), theirs.text());
                assert_eq!(merged.conflicts.len(), 1, "{relative}: {}: {merged:?}", leaf.path);
                assert!(matches!(merged.how, How::ByPath { .. }), "{relative}: {}: {merged:?}", leaf.path);
                let at: PropertyPath = merged.conflicts[0]
                    .at
                    .parse()
                    .unwrap_or_else(|_| panic!("{relative}: {}: {merged:?}", leaf.path));
                assert!(leaf.path.steps().starts_with(at.steps()), "{relative}: {at} for {}", leaf.path);
                assert!(
                    (schema.parse)(&merged.text).is_err(),
                    "{relative}: a marked file reads:\n{}",
                    merged.text
                );
                assert!(
                    reads(&resolve(&merged.text, Side::Ours)) == *ours.value().expect("reads"),
                    "{relative}"
                );
                assert!(
                    reads(&resolve(&merged.text, Side::Theirs)) == *theirs.value().expect("reads"),
                    "{relative}"
                );
                ran.clash = Some(format!("{relative}: {}", leaf.path));
                break;
            }
        }
        if ran.appended.is_some() && ran.fields.is_some() && ran.clash.is_some() {
            break;
        }
    }
    ran
}

/// Both sides append a copy of a list's last named entry, each under a new name: the merge has both, theirs after
/// ours, and reads as ours with theirs appended.
fn appended<T>(
    open: &impl Fn(&str) -> Document<T>,
    reads: &impl Fn(&str) -> T,
    name: &Path,
    base: &str,
    root: &dyn PartialReflect,
) -> Option<String>
where
    T: Reflect + FromReflect + Typed + GetTypeRegistration + Clone + PartialEq,
{
    let lists: Vec<Property> =
        open(base).properties().into_iter().filter(|property| property.kind == PropertyKind::List).collect();
    for list in lists {
        let Ok(value) = walk::get(root, &list.path) else { continue };
        let ReflectRef::List(items) = value.reflect_ref() else { continue };
        let named = !items.is_empty() && items.iter().all(|entry| walk::name_of(entry).is_some());
        if !named || unit_of(root, &list.path) != list.path {
            continue;
        }
        let last = items.get(items.len() - 1).expect("an entry");
        let copy = |key: &str| -> Option<Box<dyn PartialReflect>> {
            let mut copy = walk::through(last).to_dynamic().ok()?;
            let ReflectMut::Struct(fields) = copy.reflect_mut() else { return None };
            let field = if fields.field("id").is_some_and(|id| id.try_downcast_ref::<String>().is_some()) {
                "id"
            } else {
                "name"
            };
            fields.field_mut(field)?.try_apply(&key.to_owned()).ok()?;
            Some(copy)
        };
        let (Some(mine), Some(yours)) = (copy("merge_ours"), copy("merge_theirs")) else { continue };
        let (mut ours, mut theirs) = (open(base), open(base));
        let at = items.len();
        let landed = |document: &mut Document<T>, entry: &dyn PartialReflect| {
            matches!(document.insert("corpus", &list.path, at, entry), Ok(Outcome::Changed))
        };
        if !landed(&mut ours, mine.as_ref()) || !landed(&mut theirs, yours.as_ref()) {
            continue;
        }
        let mut expected = open(ours.text());
        if !matches!(expected.insert("corpus", &list.path, at + 1, yours.as_ref()), Ok(Outcome::Changed)) {
            continue;
        }
        let merged = merge(name, base, ours.text(), theirs.text());
        assert!(merged.is_clean(), "{}: {}: {merged:?}", name.display(), list.path);
        assert!(matches!(merged.how, How::ByPath { laid: 1, .. }), "{}: {merged:?}", name.display());
        assert!(
            reads(&merged.text) == *expected.value().expect("reads"),
            "{}:\n{}",
            name.display(),
            merged.text
        );
        return Some(format!("{}: {}", name.display(), list.path));
    }
    None
}

/// The row's corpus, over every kind: both sides append different named entries, edit different fields of one
/// entry, and set one field apart. Each merges as stated, and every merged file reads, or is marked in the clash.
/// A kind none of whose documents has a list of named entries can't append one, and one with no two fields apart
/// from each other can't edit them apart; each is named here.
#[test]
fn every_kind_merges_the_three_cases_as_stated() {
    let (mut no_named_lists, mut no_fields_apart) = (Vec::new(), Vec::new());
    for kind in Kind::ALL {
        let ran = match kind {
            Kind::Moves => corpus(kind, kinds::moves()),
            Kind::Graph => corpus(kind, kinds::graph()),
            Kind::Cue => corpus(kind, kinds::cue()),
            Kind::Effects => corpus(kind, kinds::effects()),
            Kind::Timeline => corpus(kind, kinds::timeline()),
            Kind::Ui => corpus(kind, kinds::ui()),
            Kind::Zone => corpus(kind, kinds::zone()),
            Kind::Material => corpus(kind, kinds::material()),
            Kind::EnvironmentMaterial => corpus(kind, kinds::environment_material()),
            Kind::Ramp => corpus(kind, kinds::ramp()),
            Kind::Assembly => corpus(kind, kinds::assembly()),
            Kind::Encounter => corpus(kind, kinds::encounter()),
            Kind::Dialogue => corpus(kind, kinds::dialogue()),
            Kind::Day => corpus(kind, kinds::day()),
            Kind::LightRigs => corpus(kind, kinds::light_rigs()),
            Kind::Colliders => corpus(kind, kinds::colliders()),
            Kind::Foliage => corpus(kind, kinds::foliage()),
            Kind::CueTable => corpus(kind, kinds::cue_table()),
        };
        eprintln!("{kind:?}: {ran:?}");
        assert!(ran.clash.is_some(), "{kind:?}: no field to clash on: {ran:?}");
        if ran.appended.is_none() {
            no_named_lists.push(kind);
        }
        if ran.fields.is_none() {
            no_fields_apart.push(kind);
        }
    }
    // Their lists' entries have no names, so two appends to one are a clash on the list
    // (`appends_to_a_list_of_unnamed_entries_clash_on_the_list`).
    assert_eq!(
        no_named_lists,
        [
            Kind::Cue,
            Kind::Timeline,
            Kind::Material,
            Kind::EnvironmentMaterial,
            Kind::Encounter,
            Kind::Day,
            Kind::CueTable
        ],
        "kinds with no list of named entries to append to"
    );
    // A day is its list of keys, by time, with no names: every edit of one is an edit of the list.
    assert_eq!(no_fields_apart, [Kind::Day], "kinds with no two fields of one entry apart");
}

/// Files the attributes name for the driver that don't read as their kind, each on purpose.
const NOT_DOCUMENTS: &[(&str, &str)] = &[(
    "tools/sapphire_document/test_content/problems/",
    "fixtures written not to read, for the problems pane",
)];

/// `.gitattributes` names the driver for every document a kind knows, and for nothing a kind doesn't: each
/// document of each kind matches one of its patterns, each pattern ends in what a kind's file names end in, and
/// every tracked file git gives the driver reads as its kind, or is one of [`NOT_DOCUMENTS`].
#[test]
fn the_attributes_name_the_driver_for_every_kind() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let git = |args: &[&str], input: &[u8]| {
        use std::io::Write as _;
        let mut child = std::process::Command::new("git")
            .args(args)
            .current_dir(&root)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .spawn()
            .expect("git runs");
        // Written as git answers, so neither waits on the other's full pipe.
        let mut stdin = child.stdin.take().expect("its input");
        let input = input.to_vec();
        let writer = std::thread::spawn(move || stdin.write_all(&input).expect("written"));
        let output = child.wait_with_output().expect("git ends");
        writer.join().expect("the input is written");
        assert!(output.status.success(), "git {args:?}");
        output.stdout
    };
    let tracked = git(&["ls-files", "-z"], b"");
    let attributes = git(&["check-attr", "-z", "--stdin", "merge"], &tracked);
    let fields: Vec<&[u8]> = attributes.split(|byte| *byte == 0).collect();
    let named: Vec<String> = fields
        .chunks(3)
        .filter(|entry| entry.len() == 3 && entry[2] == b"sapphire-document")
        .map(|entry| String::from_utf8(entry[0].to_vec()).expect("a path"))
        .collect();
    assert!(named.len() > 50, "the attributes name the documents: {}", named.len());
    let mut wrong = Vec::new();
    for relative in &named {
        if NOT_DOCUMENTS.iter().any(|(prefix, _)| relative.starts_with(prefix)) {
            continue;
        }
        let text = support::read(relative);
        match Kind::of_text(Path::new(relative), || Some(text.clone())) {
            // Material families share the suffix, and one with no kind merges by lines, unchecked.
            None if relative.ends_with(".material.ron") => {}
            None => wrong.push(format!("{relative}: no kind has it")),
            Some(kind) => {
                if let Err(error) = kind.read(&text) {
                    wrong.push(format!("{relative}: it doesn't read as a {kind:?} document: {error}"));
                }
            }
        }
    }
    assert!(wrong.is_empty(), "files the attributes give the driver:\n{}", wrong.join("\n"));

    // A pattern is a suffix (`*.moves.ron`), or with no `*` one file's whole name at any depth
    // (`default_effects.ron`).
    let attributes = support::read(".gitattributes");
    let patterns: Vec<&str> =
        attributes.lines().filter_map(|line| line.strip_suffix(" merge=sapphire-document")).collect();
    let matches = |pattern: &str, document: &str| match pattern.strip_prefix('*') {
        Some(suffix) => document.ends_with(suffix),
        None => document.rsplit('/').next() == Some(pattern),
    };
    for kind in Kind::ALL {
        let documents = documents_of(kind);
        assert!(!documents.is_empty(), "{kind:?}");
        for document in documents {
            assert!(patterns.iter().any(|pattern| matches(pattern, &document)), "{document}: no driver");
        }
    }
    let material = support::read(&documents_of(Kind::Material)[0]);
    for pattern in patterns {
        let named = format!(
            "content/{}",
            pattern.strip_prefix('*').map_or(pattern.to_owned(), |suffix| format!("any{suffix}"))
        );
        assert!(
            Kind::of_text(Path::new(&named), || Some(material.clone())).is_some(),
            "{pattern}: no kind's"
        );
    }
}

/// A list whose entries have no names is one value: two sides appending to it clash on the list, and either side's
/// section resolves to that side.
#[test]
fn appends_to_a_list_of_unnamed_entries_clash_on_the_list() {
    let relative = "content/lookdev/lookdev_day.ron";
    let base = support::read(relative);
    let open = |text: &str| {
        Document::open(text.to_owned(), kinds::day(), Arc::new(registry_of::<sapphire_tod::TodTimeline>()))
    };
    let keys = path("keys");
    // A copy of the last key, later in the night, with no other key between.
    let append = |time: f32| {
        let mut document = open(&base);
        let value = document.value().expect("reads");
        let at = value.keys().len();
        let entry = sapphire_tod::TodKey { time, ..value.keys()[at - 1].clone() };
        assert_eq!(document.insert("test", &keys, at, &entry), Ok(Outcome::Changed));
        document
    };
    let (ours, theirs) = (append(22.0), append(23.0));
    let merged = merge(Path::new(relative), &base, ours.text(), theirs.text());
    assert_eq!(merged.conflicts.len(), 1, "{merged:?}");
    assert_eq!(merged.conflicts[0].at, "keys");
    let reads = |text: &str| (kinds::day().parse)(text).expect("reads");
    assert!(reads(&resolve(&merged.text, Side::Ours)) == *ours.value().expect("reads"));
    assert!(reads(&resolve(&merged.text, Side::Theirs)) == *theirs.value().expect("reads"));
}

const WELL: &str =
    "            (id: \"well\", prefab: \"1ff3290e-5fa9-4f61-b891-2ca6d76b285f\", at: (0.0, 0.0, 6.0)),\n";
const BENCH: &str =
    "            (id: \"bench\", prefab: \"1ff3290e-5fa9-4f61-b891-2ca6d76b285f\", at: (2.0, 0.0, 2.0)),\n";
const CART: &str =
    "            (id: \"cart\", prefab: \"1ff3290e-5fa9-4f61-b891-2ca6d76b285f\", at: (3.0, 0.0, 3.0)),\n";

/// Both append, and theirs wrote its entry by hand, laid out as the editor wouldn't (no comma after the last
/// entry, a blank line above it): the values still merge, and the layout that didn't carry is a note.
#[test]
fn an_entry_theirs_laid_out_by_hand_still_merges() {
    let ours = YARD.replace(WELL, &format!("{WELL}{BENCH}"));
    let expected = read_zone(&YARD.replace(WELL, &format!("{WELL}{BENCH}{CART}")));
    let no_comma = CART.replace("),\n", ")\n");
    for theirs in [format!("{WELL}{no_comma}"), format!("{WELL}\n{CART}")] {
        let theirs = YARD.replace(WELL, &theirs);
        let merged = merge(Path::new(ZONE), YARD, &ours, &theirs);
        assert!(merged.is_clean(), "{merged:?}");
        by_path(&merged);
        assert_eq!(read_zone(&merged.text), expected, "{}", merged.text);
    }
}

/// A marked document names its conflict where the editor and `content-check` read it: at the value the
/// marker's label gives, as an unresolved merge conflict.
#[test]
fn a_marked_document_names_its_unresolved_conflict() {
    let ours = YARD.replace("turn: 30.0", "turn: 45.0");
    let theirs = YARD.replace("turn: 30.0", "turn: 60.0");
    let merged = merge(Path::new(ZONE), YARD, &ours, &theirs);
    assert_eq!(merged.conflicts.len(), 1, "{merged:?}");
    let problems = problem::of_text(Path::new(ZONE), Kind::Zone, &merged.text);
    assert_eq!(problems.len(), 1, "{problems:?}");
    assert_eq!(problems[0].at, At::Field(r#"placements["gate"].turn"#.to_owned()));
    assert!(
        problems[0].message.starts_with("an unresolved merge conflict (line "),
        "{}",
        problems[0].message
    );
    let lines = merge(Path::new("notes/plan.txt"), "a\n", "b\n", "c\n");
    let problem = problem::Problem::parse(Path::new(ZONE), &lines.text, "1:1: expected a value");
    assert!(matches!(problem.at, At::Document(_)), "a line merge's marker names no field");
    assert!(problem.message.starts_with("an unresolved merge conflict (line 1)"), "{}", problem.message);
}

/// One of their edits ours made alike is skipped, and the rest are laid; a removal both made is one removal.
#[test]
fn edits_both_made_alike_are_made_once() {
    let alike = |text: &str| text.replace("turn: 30.0", "turn: 45.0");
    let brighter = |text: &str| text.replace("intensity: 6.0", "intensity: 7.0");
    let merged = merge(Path::new(ZONE), YARD, &alike(YARD), &brighter(&alike(YARD)));
    assert!(merged.is_clean(), "{merged:?}");
    assert_eq!(by_path(&merged), 1, "the alike edit is skipped, the other laid");
    assert_eq!(read_zone(&merged.text), read_zone(&brighter(&alike(YARD))));

    let gone = |text: &str| text.replace(WELL, "");
    let merged = merge(Path::new(ZONE), YARD, &gone(YARD), &brighter(&gone(YARD)));
    assert!(merged.is_clean(), "{merged:?}");
    assert_eq!(read_zone(&merged.text), read_zone(&brighter(&gone(YARD))));
}

/// A field the base doesn't write, both sides add apart, and an option both sides take from none to some apart:
/// each is one conflict, and each side's sections resolve to that side.
#[test]
fn a_field_or_an_option_the_base_lacks_both_add_apart_is_marked() {
    let post = "at: (-6.0, 0.0, -6.0))";
    let grid = "        grid: (cell_size: 16.0),\n";
    let cases = [
        (
            YARD.replace(post, "at: (-6.0, 0.0, -6.0), turn: 10.0)"),
            YARD.replace(post, "at: (-6.0, 0.0, -6.0), turn: 20.0)"),
        ),
        (
            YARD.replace(grid, &format!("{grid}        day: Some(\"dawn\"),\n")),
            YARD.replace(grid, &format!("{grid}        day: Some(\"dusk\"),\n")),
        ),
    ];
    for (ours, theirs) in cases {
        let merged = merge(Path::new(ZONE), YARD, &ours, &theirs);
        assert_eq!(merged.conflicts.len(), 1, "{merged:?}");
        assert!(ZoneDocument::from_ron(&merged.text).is_err(), "{}", merged.text);
        assert_eq!(read_zone(&resolve(&merged.text, Side::Ours)), read_zone(&ours), "{}", merged.text);
        assert_eq!(read_zone(&resolve(&merged.text, Side::Theirs)), read_zone(&theirs), "{}", merged.text);
    }
}

/// Both sides adding a file give git an empty base: that's the reason given, not a version or a kind.
#[test]
fn a_file_both_sides_add_merges_by_lines_and_says_so() {
    let merged = merge(Path::new(ZONE), "", YARD, &YARD.replace("turn: 30.0", "turn: 45.0"));
    assert_eq!(merged.how, How::ByLines { why: "both sides added it, so it has no base".to_owned() });
    assert_eq!(merged.conflicts.len(), 1, "{merged:?}");
}

/// Sides none of which reads as its kind (a newer version than this build knows) keep the line merge's own
/// result: a clean one isn't turned into a conflict.
#[test]
fn sides_this_build_cant_read_keep_the_line_merge() {
    let newer = YARD.replace("version: 2", "version: 99");
    let ours = newer.replace("// A yard.", "// The yard.");
    let theirs = newer.replace("intensity: 6.0", "intensity: 7.0");
    let merged = merge(Path::new(ZONE), &newer, &ours, &theirs);
    assert!(matches!(merged.how, How::ByLines { .. }), "{merged:?}");
    assert!(merged.is_clean(), "{merged:?}");
    assert_eq!(merged.text, ours.replace("intensity: 6.0", "intensity: 7.0"));
}
