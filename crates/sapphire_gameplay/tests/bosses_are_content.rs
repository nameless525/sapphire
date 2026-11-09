//! Exit test 2 (m3-plan §1): a boss is documents and a script under `content/`. For every boss a zone's
//! document reaches, this holds that:
//!
//! 1. its name is in no `.rs` file: no word of its kind's file name or of its name's key;
//! 2. everything it needs is reached from the zone's document, through files under `content/`: the zone's
//!    encounters and spawners to its kind, and from its kind every asset by ID, its script among them;
//! 3. the commit that put it in a zone touched only `content/`, `tests/replays/` and `tests/golden/`.
//!
//! The third reads the repository's history, so it holds where there's a whole history to read: a checkout,
//! not CI's shallow clone, where it says so and lets the other two stand.
//!
//! What "reached" means: a document names another by its ID (a `.meta` sidecar's `id`), by its path under
//! `content/`, or, a spawner, by a kind's name, which is a `<name>.enemy.ron`'s. A source file a document
//! never names, such as a clip nothing plays yet, isn't needed. Nothing here names a boss: they're found by
//! being one, a kind whose document has a `boss`.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::process::Command;

use sapphire_moves::EnemyDocument;

/// Where the parts of a boss's name are looked for: every `.rs` file in the repository.
const CODE: &str = "rs";

/// The trees the commit that adds a boss to a zone may touch.
const CONTENT_ONLY: [&str; 3] = ["content/", "tests/replays/", "tests/golden/"];

/// Words in a boss's names too plain to be its own: its name's key starts with one.
const PLAIN: [&str; 1] = ["boss"];

fn repository() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..").canonicalize().expect("the repository is there")
}

/// Every file under `dir` ending `.{extension}`, or every file if `None`, past hidden folders and builds.
fn files_under(dir: &Path, extension: Option<&str>, out: &mut Vec<PathBuf>) {
    let mut entries: Vec<_> =
        std::fs::read_dir(dir).expect("reads").map(|entry| entry.unwrap().path()).collect();
    entries.sort();
    for path in entries {
        let name = path.file_name().and_then(|name| name.to_str()).unwrap_or_default();
        if path.is_dir() {
            if !name.starts_with('.') && name != "target" {
                files_under(&path, extension, out);
            }
        } else if extension.is_none_or(|extension| path.extension().is_some_and(|it| it == extension)) {
            out.push(path);
        }
    }
}

/// A document's text without its `//` comments, so a comment naming a file doesn't count as reaching it.
fn code_of(text: &str) -> String {
    let mut code = String::with_capacity(text.len());
    for line in text.lines() {
        let mut quoted = false;
        let mut previous = '\0';
        let mut end = line.len();
        for (at, c) in line.char_indices() {
            match c {
                '"' if previous != '\\' => quoted = !quoted,
                '/' if !quoted && previous == '/' => {
                    end = at - 1;
                    break;
                }
                _ => {}
            }
            previous = c;
        }
        code.push_str(&line[..end]);
        code.push('\n');
    }
    code
}

/// Each `"..."` in `code`, with what came just before it.
fn quoted(code: &str) -> Vec<(&str, &str)> {
    let mut found = Vec::new();
    let mut rest = code;
    let mut before_start = 0;
    while let Some(open) = rest.find('"') {
        let after = &rest[open + 1..];
        let Some(close) = after.find('"') else { break };
        let before = &code[before_start..before_start + open];
        found.push((before, &after[..close]));
        let consumed = open + 1 + close + 1;
        before_start += consumed;
        rest = &rest[consumed..];
    }
    found
}

fn is_id(text: &str) -> bool {
    text.len() == 36
        && text
            .char_indices()
            .all(|(at, c)| if [8, 13, 18, 23].contains(&at) { c == '-' } else { c.is_ascii_hexdigit() })
}

/// The files under `content/`, by the ID their sidecar gives them and by name.
struct Content {
    root: PathBuf,
    by_id: BTreeMap<String, PathBuf>,
    kinds: BTreeMap<String, PathBuf>,
}

/// What a walk from a file reached, and what it named that isn't there.
#[derive(Default)]
struct Reached {
    files: BTreeSet<PathBuf>,
    missing: Vec<String>,
}

impl Content {
    fn read(root: PathBuf) -> Self {
        let mut files = Vec::new();
        files_under(&root.join("content"), None, &mut files);
        let (mut by_id, mut kinds) = (BTreeMap::new(), BTreeMap::new());
        for path in files {
            let name = path.file_name().unwrap().to_str().unwrap().to_owned();
            if let Some(kind) = name.strip_suffix(".enemy.ron") {
                kinds.insert(kind.to_owned(), path.clone());
            }
            if let Some(file) = name.strip_suffix(".meta") {
                let text = std::fs::read_to_string(&path).expect("a sidecar is text");
                let id = quoted(&code_of(&text))
                    .into_iter()
                    .find(|(before, _)| before.trim_end().ends_with("id:"))
                    .map(|(_, id)| id.to_owned())
                    .unwrap_or_else(|| panic!("{} gives an ID", path.display()));
                by_id.insert(id, path.with_file_name(file));
            }
        }
        Self { root, by_id, kinds }
    }

    fn relative(&self, path: &Path) -> String {
        path.strip_prefix(&self.root).unwrap_or(path).to_string_lossy().replace('\\', "/")
    }

    /// Everything `from` reaches, itself and its sidecar included.
    fn reach(&self, from: &Path) -> Reached {
        let mut reached = Reached::default();
        let mut to_read = vec![from.to_owned()];
        while let Some(path) = to_read.pop() {
            if !reached.files.insert(path.clone()) {
                continue;
            }
            let sidecar = PathBuf::from(format!("{}.meta", path.display()));
            if sidecar.is_file() {
                to_read.push(sidecar);
            }
            let named = path.extension().is_some_and(|it| it == "ron" || it == "meta");
            let Some(text) = named.then(|| std::fs::read_to_string(&path).ok()).flatten() else { continue };
            for (before, text) in quoted(&code_of(&text)) {
                let found = if is_id(text) {
                    self.by_id.get(text).cloned()
                } else if text.starts_with("content/") {
                    Some(self.root.join(text)).filter(|path| path.is_file())
                } else if before.trim_end().ends_with("spawns:") {
                    // A spawner of something that isn't a kind (the player, a checkpoint) reaches nothing.
                    self.kinds.get(text).cloned().or(Some(path.clone()))
                } else {
                    continue;
                };
                match found {
                    Some(found) => to_read.push(found),
                    None => reached.missing.push(format!("{} names {text}", self.relative(&path))),
                }
            }
        }
        reached
    }
}

/// A boss a zone reaches: its kind's document, and its words, for the code not to have.
struct Boss {
    kind: PathBuf,
    document: EnemyDocument,
    words: BTreeSet<String>,
}

fn bosses(content: &Content, zone: &Reached) -> Vec<Boss> {
    let mut bosses = Vec::new();
    for kind in zone.files.iter().filter(|path| path.to_string_lossy().ends_with(".enemy.ron")) {
        let document = EnemyDocument::parse(&std::fs::read_to_string(kind).unwrap()).expect("a kind parses");
        let Some(boss) = &document.boss else { continue };
        let stem = kind.file_name().unwrap().to_str().unwrap().trim_end_matches(".enemy.ron");
        let words = [stem, boss.name.as_str()]
            .iter()
            .flat_map(|names| names.split(|c: char| !c.is_ascii_alphanumeric()))
            .map(str::to_ascii_lowercase)
            .filter(|word| word.len() >= 4 && !PLAIN.contains(&word.as_str()))
            .collect();
        assert!(!bosses.iter().any(|known: &Boss| known.kind == *kind), "{}", content.relative(kind));
        bosses.push(Boss { kind: kind.clone(), document, words });
    }
    bosses
}

/// Every zone's document, and every boss it reaches.
fn zones_and_bosses(content: &Content) -> Vec<(PathBuf, Reached, Vec<Boss>)> {
    let mut documents = Vec::new();
    files_under(&content.root.join("content"), Some("ron"), &mut documents);
    documents
        .into_iter()
        .filter(|path| path.to_string_lossy().ends_with(".zone.ron"))
        .map(|zone| {
            let reached = content.reach(&zone);
            let bosses = bosses(content, &reached);
            (zone, reached, bosses)
        })
        .collect()
}

#[test]
fn a_zones_boss_is_content_its_code_never_names() {
    let content = Content::read(repository());
    let zones = zones_and_bosses(&content);
    let bosses: Vec<(&PathBuf, &Reached, &Boss)> = zones
        .iter()
        .flat_map(|(zone, reached, bosses)| bosses.iter().map(move |boss| (zone, reached, boss)))
        .collect();
    assert!(!bosses.is_empty(), "some zone's document reaches a boss");

    let mut code = Vec::new();
    files_under(&content.root, Some(CODE), &mut code);
    assert!(code.len() > 100, "the repository's code is there: {}", code.len());
    for (zone, reached, boss) in bosses {
        let kind = content.relative(&boss.kind);
        assert!(!boss.words.is_empty(), "{kind} has a name to look for");
        // 1. No code names it.
        for path in &code {
            let text = std::fs::read_to_string(path).unwrap_or_default().to_ascii_lowercase();
            for word in &boss.words {
                assert!(!text.contains(word.as_str()), "{} names {kind}'s boss", content.relative(path));
            }
        }
        // 2. Its zone reaches it and all it needs, through content, its script among them.
        assert!(reached.missing.is_empty(), "{}: {:?}", content.relative(zone), reached.missing);
        let own = content.reach(&boss.kind);
        assert!(own.missing.is_empty(), "{kind}: {:?}", own.missing);
        assert!(own.files.is_subset(&reached.files), "the zone reaches all {kind} does");
        assert!(own.files.iter().all(|path| path.starts_with(content.root.join("content"))));
        let script = boss.document.script.unwrap_or_else(|| panic!("{kind} names its script"));
        let script =
            content.by_id.get(&script.to_string()).unwrap_or_else(|| panic!("{kind}'s script is there"));
        assert!(script.extension().is_some_and(|it| it == "luau"), "{}", content.relative(script));
        assert!(own.files.contains(script));
    }
}

/// Clause 3, where the history is there to read: the oldest commit that put a boss's kind on one of its zone's
/// spawners touched only content, replays and goldens.
#[test]
fn the_commit_that_put_a_boss_in_its_zone_touched_only_content() {
    let content = Content::read(repository());
    let git = |args: &[&str]| {
        let output = Command::new("git").arg("-C").arg(&content.root).args(args).output().ok()?;
        output.status.success().then(|| String::from_utf8(output.stdout).expect("git speaks UTF-8"))
    };
    match git(&["rev-parse", "--is-shallow-repository"]).as_deref().map(str::trim) {
        Some("false") => {}
        other => {
            eprintln!("no whole history to read ({other:?}): clause 3 is checked where there is one");
            return;
        }
    }
    for (zone, _, bosses) in zones_and_bosses(&content) {
        let zone = content.relative(&zone);
        for boss in bosses {
            let kind = boss.kind.file_name().unwrap().to_str().unwrap().trim_end_matches(".enemy.ron");
            let put = format!("spawns: \"{kind}\"");
            let log = git(&["log", "--format=%H", "-S", &put, "--", &zone]).expect("git reads the log");
            let commit = log.lines().last().unwrap_or_else(|| panic!("a commit put {put} in {zone}"));
            let touched = git(&["diff-tree", "--no-commit-id", "--name-only", "-r", "--root", commit])
                .expect("git reads the commit");
            let outside: Vec<&str> = touched
                .lines()
                .filter(|path| !CONTENT_ONLY.iter().any(|tree| path.starts_with(tree)))
                .collect();
            assert!(outside.is_empty(), "{commit} put a boss in {zone} and touched {outside:?}");
        }
    }
}
