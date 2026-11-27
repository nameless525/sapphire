//! Animation graph documents (`.animgraph.ron`) cook to `.sgraph` (M2 N3, M6 A5c, spec §5.13): how a
//! character's clips play, compiled by [`sapphire_assets::graph::v2`] to the [`GraphProgram`] a character
//! binds, with each clip's derived ID in it so the asset server can load the clips as the graph's
//! dependencies.
//!
//! A person names a clip by the file it comes from, its name in that file and the character it was
//! retargeted to, because that's what a person knows. Those three hash to the same ID the clip's own
//! cook gave it ([`clip_id`]), and a cook fails if nothing in the content tree has that ID.
//!
//! **Every graph compiles for the fixed step** ([`Target::COOKED`], which says why): what only
//! presentation may run (a game slot, a move's side variants, an event) is refused by its path at cook. A version 1 document still cooks, converted; `upgrade-graphs` moves one to
//! version 2 on disk ([`upgrade`]). An editor's layout sidecar (`<name>.animgraph.layout.ron`) is no
//! source.

use std::path::{Path, PathBuf};

use sapphire_assets::graph::GraphError;
pub use sapphire_assets::graph::clip_id;
use sapphire_assets::graph::v2::{self, CompileError, GraphProgram, Target};
use sapphire_serialize::DocumentError;
use sapphire_serialize::cooked::CookedError;
use serde::{Deserialize, Serialize};

use crate::cache::Stage;

/// Reading the document, as cache keys know it. Bump the version whenever its output changes for the
/// same file.
pub const IMPORTER: Stage = Stage { name: "animation graph document", version: 1 };

/// Checking it and deriving its clips' IDs. Bump the version whenever they change output for the same
/// document.
///
/// 2: a `target` on a key-pose document names that rig's clip, where it was ignored for the first rig's
/// (the review's AST-001).
/// 3: imported clips use the unambiguous v2 name/target identity encoding.
/// 4: also compiles every graph as version 2 (M6 A5a), so a warm cache checks each graph once; the
/// same bytes, and since a pack keys its assets by their bytes, the same packs.
/// 5: writes the compiled program, for the fixed step (M6 A5c).
pub const COOKER: Stage = Stage { name: "sgraph", version: 5 };

/// What a graph source's file name ends with.
pub const SOURCE_SUFFIX: &str = ".animgraph.ron";

/// Import settings for graphs: none yet. A struct, so settings can join without changing existing
/// sidecars.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct AnimGraphSettings {}

/// A document that didn't cook.
#[derive(Debug, thiserror::Error)]
pub enum AnimGraphCookError {
    #[error("the document isn't UTF-8")]
    NotUtf8(#[source] std::str::Utf8Error),
    #[error(transparent)]
    Graph(#[from] GraphError),
    /// What graph v2's compiler refuses, by its path in the document (in a version 1 document, converted).
    #[error(transparent)]
    Compile(#[from] CompileError),
    #[error(transparent)]
    Cooked(#[from] CookedError),
    #[error(transparent)]
    Document(#[from] DocumentError),
}

/// Whether `path` names a graph source.
pub fn is_source(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.to_ascii_lowercase().ends_with(SOURCE_SUFFIX))
}

/// The cooked file a graph source at `relative` writes: `fighter.animgraph.ron` → `fighter.sgraph`.
pub fn output_path(relative: &Path, extension: &str) -> PathBuf {
    let name = relative.file_name().and_then(|name| name.to_str()).unwrap_or_default();
    let stem = &name[..name.len().saturating_sub(SOURCE_SUFFIX.len())];
    relative.with_file_name(format!("{stem}.{extension}"))
}

/// Reads a document's bytes, of either version, with its clips' IDs filled in.
pub fn read(bytes: &[u8]) -> Result<v2::AnimGraph, AnimGraphCookError> {
    let text = std::str::from_utf8(bytes).map_err(AnimGraphCookError::NotUtf8)?;
    let mut graph = v2::AnimGraph::from_document(text)?;
    graph.resolve(clip_id);
    Ok(graph)
}

/// A graph document compiled as its cook compiles it: for the fixed step, with its clips' IDs derived.
pub fn compile(bytes: &[u8]) -> Result<GraphProgram, AnimGraphCookError> {
    Ok(read(bytes)?.compile_for(Target::COOKED)?)
}

/// The bytes of a graph's cooked file, its program, read back through the runtime's check.
pub fn cook(bytes: &[u8]) -> Result<Vec<u8>, AnimGraphCookError> {
    let cooked = compile(bytes)?.to_cooked()?;
    GraphProgram::read(&cooked)?;
    Ok(cooked.into_vec())
}

/// A version 1 document's text at version 2, or `None` for a document already at version 2, which is
/// left as its author wrote it. What converts is what cooks: the program compiled from the text written
/// is the one compiled from the text read.
pub fn upgrade(text: &str) -> Result<Option<String>, AnimGraphCookError> {
    #[derive(Deserialize)]
    struct Envelope {
        version: u32,
    }
    let envelope: Envelope = sapphire_serialize::ron::from_str(text).map_err(DocumentError::Parse)?;
    if envelope.version >= <v2::AnimGraph as sapphire_reflect::Versioned>::VERSION {
        return Ok(None);
    }
    let graph = v2::AnimGraph::from_document(text)?;
    Ok(Some(graph.to_document()?))
}

/// The lines of a document holding a comment (`//` outside a string), which the converter drops: it
/// writes the document it read, and a comment isn't part of one.
pub fn comment_lines(text: &str) -> usize {
    text.lines()
        .filter(|line| {
            let mut quoted = false;
            let mut escaped = false;
            let mut slash = false;
            for character in line.chars() {
                if quoted {
                    match character {
                        _ if escaped => escaped = false,
                        '\\' => escaped = true,
                        '"' => quoted = false,
                        _ => {}
                    }
                    continue;
                }
                match character {
                    '"' => quoted = true,
                    '/' if slash => return true,
                    _ => {}
                }
                slash = character == '/';
            }
            false
        })
        .count()
}

/// Moves every version 1 graph document under `content` to version 2, returning the files it rewrote,
/// each with the count of comment lines it dropped ([`comment_lines`]).
pub fn upgrade_tree(content: &Path) -> anyhow::Result<Vec<(PathBuf, usize)>> {
    use anyhow::Context;

    let mut written = Vec::new();
    let mut folders = vec![content.to_owned()];
    while let Some(folder) = folders.pop() {
        let mut entries = std::fs::read_dir(&folder)
            .with_context(|| format!("reading {}", folder.display()))?
            .map(|entry| entry.map(|entry| entry.path()))
            .collect::<Result<Vec<_>, _>>()?;
        entries.sort();
        for path in entries {
            if path.is_dir() {
                folders.push(path);
            } else if is_source(&path) {
                let text =
                    std::fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
                if let Some(upgraded) = upgrade(&text).with_context(|| path.display().to_string())? {
                    std::fs::write(&path, upgraded).with_context(|| format!("writing {}", path.display()))?;
                    written.push((path, comment_lines(&text)));
                }
            }
        }
    }
    written.sort();
    Ok(written)
}

/// What keeps walking inside the character's cooked bounds, for the tests that hold locomotion to them.
/// Any clip may leave: `posed_bounds` in `sapphire_render::bounds` grows the sphere where a pose does,
/// so a death's shadow, outline and lights hold. What's left of N3's rule is what that growth costs:
/// inside the sphere a frame has the cooked bounds exactly, and past it each new pose re-reads the
/// character's light lists and widens what it casts into. A move can pay that for a moment. Locomotion
/// is most of a character's frames, so it stays inside.
#[cfg(test)]
pub(crate) struct LocomotionBounds {
    center: sapphire_math::Vec3,
    pub radius: f32,
    positions: Vec<sapphire_math::Vec3>,
    weights: Vec<crate::mesh::SkinWeights>,
    binds: Vec<(usize, sapphire_math::Affine3A)>,
}

#[cfg(test)]
impl LocomotionBounds {
    /// The look-dev character, as its VRM imports.
    pub fn of_the_character() -> Self {
        use sapphire_math::{Affine3A, Mat4, Vec3};

        let path =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../content/lookdev/character/character.vrm");
        let bytes = std::fs::read(path).expect("the character");
        let mesh = crate::import::vrm::import(&bytes, None).expect("the character").mesh;
        let positions: Vec<Vec3> = mesh.vertices.iter().map(|vertex| Vec3::from(vertex.position)).collect();
        let (mut min, mut max) = (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN));
        for &position in &positions {
            min = min.min(position);
            max = max.max(position);
        }
        let skin = mesh.skin.expect("skinned");
        let binds = skin
            .binds
            .iter()
            .map(|bind| {
                let inverse = Mat4::from_cols_array_2d(&bind.inverse_bind_matrix);
                (usize::from(bind.joint), Affine3A::from_mat4(inverse))
            })
            .collect();
        Self {
            center: (min + max) * 0.5,
            radius: (max - min).length() * 0.5,
            positions,
            weights: skin.weights,
            binds,
        }
    }

    /// How far from the sphere's center the skin gets with its joints at `model`, each vertex skinned as
    /// the GPU would. Rig constraints don't run here, and they only twist forearms in place.
    pub fn reach(&self, model: &[sapphire_math::Affine3A]) -> f32 {
        let palette: Vec<sapphire_math::Affine3A> =
            self.binds.iter().map(|&(joint, inverse)| model[joint] * inverse).collect();
        let mut farthest = 0.0_f32;
        for (&position, weights) in self.positions.iter().zip(&self.weights) {
            let mut skinned = sapphire_math::Vec3::ZERO;
            for (&bind, &weight) in weights.binds.iter().zip(&weights.weights) {
                if weight > 0.0 {
                    skinned += palette[usize::from(bind)].transform_point3(position) * weight;
                }
            }
            farthest = farthest.max(skinned.distance_squared(self.center));
        }
        farthest.sqrt()
    }

    /// What the look-dev character's locomotion plays: its states' clips and its blend spaces' samples.
    pub fn clips() -> Vec<sapphire_assets::graph::GraphClip> {
        use sapphire_assets::graph::v2::Motion;

        let text = include_str!("../../../content/lookdev/character/character.animgraph.ron");
        let graph = read(text.as_bytes()).expect("it reads");
        let named: Vec<&str> = graph
            .machines
            .iter()
            .flat_map(|machine| &machine.states)
            .flat_map(|state| match &state.plays {
                Motion::Clip { clip, .. } => vec![clip.as_str()],
                Motion::Blend1D(blend) => blend.samples.iter().map(|sample| sample.clip.as_str()).collect(),
                Motion::Blend2D(blend) => blend.samples.iter().map(|sample| sample.clip.as_str()).collect(),
                Motion::Takeoff(takeoff) => std::iter::once(takeoff.standing.as_str())
                    .chain(takeoff.moving.iter().map(|sample| sample.clip.as_str()))
                    .collect(),
                Motion::Landing(entry) => std::iter::once(entry.standing.as_str())
                    .chain(entry.moving.iter().map(|sample| sample.clip.as_str()))
                    .collect(),
                Motion::Machine(_) => Vec::new(),
            })
            .collect();
        graph.clips.iter().filter(|clip| named.contains(&clip.name.as_str())).cloned().collect()
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use sapphire_assets::AssetId;
    use sapphire_assets::graph::GraphClip;

    use super::*;
    use crate::cook::{Cook, CookError, CookOptions, CookStatus, cook_dir};
    use crate::meta;
    use crate::test_util::TempDir;

    /// The character's own graph, as the look-dev stage plays it.
    const CHARACTER: &str = include_str!("../../../content/lookdev/character/character.animgraph.ron");

    #[test]
    fn a_source_is_known_by_its_suffix_and_cooks_beside_itself() {
        assert!(is_source(Path::new("a/fighter.animgraph.ron")));
        assert!(!is_source(Path::new("a/fighter.ron")));
        assert_eq!(
            output_path(Path::new("a/fighter.animgraph.ron"), "sgraph"),
            Path::new("a/fighter.sgraph")
        );
    }

    /// The same derivation a clip's own cook uses, and N1's row names.
    #[test]
    fn a_clips_id_is_the_one_its_own_cook_gives_it() {
        let source = AssetId::from_text("9a29e8c0-f131-4ea4-98b9-04879740413d");
        let target = AssetId::from_text("6f8a81b6-29c4-485f-b161-847b001d561f");
        let id = AssetId::from_bytes([0; 16]);
        let clip = |name: Option<&str>, target| GraphClip {
            name: "x".to_owned(),
            source,
            clip: name.map(str::to_owned),
            target,
            id,
        };
        assert_eq!(
            clip_id(&clip(Some("Hit_A"), Some(target))),
            sapphire_assets::graph::imported_clip_id(source, "Hit_A", Some(target))
        );
        assert_eq!(
            clip_id(&clip(Some("Hit_A"), None)),
            sapphire_assets::graph::imported_clip_id(source, "Hit_A", None)
        );
        assert_eq!(clip_id(&clip(None, None)), source);
        assert_eq!(clip_id(&clip(None, Some(target))), meta::sub_asset_id(source, &format!("rig/{target}")));
    }

    /// The character's document reads, with every clip's ID derived.
    #[test]
    fn the_characters_graph_reads() {
        let graph = read(CHARACTER.as_bytes()).expect("it reads");
        assert!(graph.clips.iter().all(|clip| clip.id != AssetId::from_bytes([0; 16])));
    }

    fn cook_one(dir: &Path, text: &str) -> Result<CookStatus, CookError> {
        fs::create_dir_all(dir.join("in")).expect("a folder");
        fs::write(dir.join("in/fighter.animgraph.ron"), text).expect("a document");
        let report = cook_dir(
            &dir.join("in"),
            &dir.join("out"),
            &CookOptions { cook: Cook::Desktop, cache: None, locked: false, editor: false },
        )?;
        Ok(report.assets[0].status)
    }

    #[test]
    fn a_graph_naming_a_clip_nothing_cooks_fails_the_cook_by_name() {
        let dir = TempDir::new("anim_graph_cook");
        let text = r#"(
    type: "sapphire::assets::AnimGraph",
    version: 1,
    data: (
        clips: [(name: "idle", source: "9a29e8c0-f131-4ea4-98b9-04879740413d", clip: "Idle_A")],
        locomotion: (start: "ground", states: [(name: "ground", plays: Clip(clip: "idle"))]),
    ),
)"#;
        let error = cook_one(dir.path(), text).expect_err("nothing cooks that clip");
        let text = error.to_string();
        assert!(text.contains("fighter") && text.contains("not a clip"), "{text}");
    }

    /// Every shipped graph converts to version 2 and compiles, and each graph the code may name at the cost
    /// pinned here: the most clips an evaluation of its layers samples, pose registers, ops, condition steps
    /// from one state.
    #[test]
    fn every_shipped_graph_converts_and_compiles() {
        let content = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../content");
        let mut costs = Vec::new();
        let mut folders = vec![content.clone()];
        while let Some(folder) = folders.pop() {
            for entry in fs::read_dir(&folder).expect("a content folder") {
                let path = entry.expect("an entry").path();
                if path.is_dir() {
                    folders.push(path);
                } else if is_source(&path) {
                    let bytes = fs::read(&path).expect("a graph");
                    let program =
                        compile(&bytes).unwrap_or_else(|error| panic!("{}: {error}", path.display()));
                    let relative = path.strip_prefix(&content).expect("in content");
                    let name = relative
                        .components()
                        .map(|part| part.as_os_str().to_string_lossy())
                        .collect::<Vec<_>>();
                    let cost = program.cost;
                    let row = [cost.clips_sampled, u16::from(cost.registers), cost.ops, cost.condition_ops];
                    costs.push((name.join("/"), row));
                }
            }
        }
        // The graphs the code may name, each pinned. A boss's graph is content no code names (gameplay's
        // `bosses_are_content`), so it and any other graph are held by compiling within the cap above.
        let pinned: &[(&str, [u16; 4])] = &[
            ("ambient/ambient.animgraph.ron", [2, 2, 3, 0]),
            ("combat/enemy/warrior.animgraph.ron", [6, 2, 3, 0]),
            ("combat/hero/hero.animgraph.ron", [7, 3, 5, 0]),
            ("combat/lancer/lancer.animgraph.ron", [6, 2, 3, 0]),
            ("combat/wren/wren.animgraph.ron", [7, 3, 5, 0]),
            ("lookdev/character/character.animgraph.ron", [10, 3, 8, 6]),
        ];
        for (name, expected) in pinned {
            let found = costs.iter().find(|(path, _)| path == name).unwrap_or_else(|| panic!("{name} ships"));
            assert_eq!(found.1, *expected, "{name}'s cost an evaluation");
        }
        assert!(costs.len() > pinned.len(), "a graph the code doesn't name compiles too: {}", costs.len());
    }

    /// Each of the heroine's transitions says the rules v1's runtime keyed on its condition's variant,
    /// and her program carries them: none rides on a condition's shape.
    #[test]
    fn the_heroines_transitions_say_their_runtime_rules() {
        let content = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../content");
        let bytes = fs::read(content.join("lookdev/character/character.animgraph.ron")).expect("her graph");
        let graph = v2::AnimGraph::from_document(std::str::from_utf8(&bytes).expect("UTF-8")).expect("reads");
        let none = v2::TransitionRules::default();
        let rebound = v2::TransitionRules { rebound: true, ..none };
        let expected = [
            ("ground", "air", none),
            ("ground", "jump_prepare", rebound),
            ("jump_prepare", "air", none),
            ("jump_prepare", "ground", none),
            ("air", "land", v2::TransitionRules { lands: true, ..none }),
            ("air", "ground", none),
            ("land", "air", none),
            ("land", "jump_prepare", rebound),
            ("land", "ground", v2::TransitionRules { at_drawn: true, catch: true, ..none }),
            ("land", "ground", v2::TransitionRules { waits_for_landing: true, ..none }),
            ("land", "ground", v2::TransitionRules { at_drawn: true, ..none }),
        ];
        let found: Vec<(&str, &str, v2::TransitionRules)> = graph.machines[0]
            .transitions
            .iter()
            .map(|transition| (transition.from.as_str(), transition.to.as_str(), transition.rules))
            .collect();
        assert_eq!(found, expected);
        let program = compile(&bytes).expect("compiles");
        let carried: Vec<v2::TransitionRules> =
            program.transitions.iter().map(|transition| transition.rules).collect();
        assert_eq!(carried, expected.map(|(.., rules)| rules));
    }

    /// An editor's layout sidecar is no graph, so moving a node recooks nothing.
    #[test]
    fn a_layout_sidecar_is_not_a_source() {
        assert!(!is_source(Path::new("a/fighter.animgraph.layout.ron")));
    }

    #[test]
    fn a_bad_document_fails_the_cook_with_its_issues() {
        let dir = TempDir::new("anim_graph_cook_bad");
        let text = r#"(
    type: "sapphire::assets::AnimGraph",
    version: 1,
    data: (
        clips: [],
        locomotion: (start: "nowhere", states: []),
    ),
)"#;
        let error = cook_one(dir.path(), text).expect_err("it doesn't check");
        let source = std::error::Error::source(&error).map(ToString::to_string).unwrap_or_default();
        assert!(source.contains("nowhere"), "{error}: {source}");
    }

    /// A version 1 document with a machine of states, a blend space and a rule, as one was written.
    const VERSION_1: &str = r#"(
    type: "sapphire::assets::AnimGraph",
    version: 1,
    data: (
        clips: [
            (name: "idle", source: "9a29e8c0-f131-4ea4-98b9-04879740413d", clip: "Idle_A"),
            (name: "walk", source: "9a29e8c0-f131-4ea4-98b9-04879740413d", clip: "Walk_A"),
            (name: "fall", source: "9a29e8c0-f131-4ea4-98b9-04879740413d", clip: "Fall_A"),
        ],
        locomotion: (
            start: "ground",
            states: [
                (name: "ground", plays: Blend((samples: [
                    (clip: "idle", at: (0.0, 0.0), sync: false),
                    (clip: "walk", at: (0.0, 1.5)),
                ]))),
                (name: "air", plays: Clip(clip: "fall", looping: true)),
            ],
            transitions: [
                (from: "ground", to: "air", when: Airborne, decay: 8),
                (from: "air", to: "ground", when: Landed, decay: 4),
            ],
        ),
    ),
)"#;

    /// A version 1 document upgrades to version 2 text that compiles to the program the version 1 text
    /// did, and that text, upgraded again, is left alone.
    #[test]
    fn a_version_1_document_upgrades_to_what_it_cooked() {
        let upgraded = upgrade(VERSION_1).expect("it upgrades").expect("version 1 moves");
        assert!(upgraded.contains("version: 2"), "{upgraded}");
        assert_eq!(
            compile(upgraded.as_bytes()).expect("compiles"),
            compile(VERSION_1.as_bytes()).expect("compiles")
        );
        assert_eq!(upgrade(&upgraded).expect("it reads"), None);
    }

    /// `upgrade-graphs` rewrites the version 1 documents in a tree and leaves the rest as they were.
    #[test]
    fn upgrading_a_tree_rewrites_only_version_1_graphs() {
        let dir = TempDir::new("anim_graph_upgrade");
        let upgraded = upgrade(VERSION_1).expect("it upgrades").expect("version 1 moves");
        fs::create_dir_all(dir.path().join("a/b")).expect("folders");
        let commented = format!("// A note.\n{}", VERSION_1.replacen("(", "( // \"Its\" note.", 1));
        fs::write(dir.path().join("a/b/old.animgraph.ron"), &commented).expect("a document");
        fs::write(dir.path().join("a/new.animgraph.ron"), &upgraded).expect("a document");
        fs::write(dir.path().join("a/other.ron"), VERSION_1).expect("not a graph source");
        let written = upgrade_tree(dir.path()).expect("it upgrades");
        assert_eq!(written, vec![(dir.path().join("a/b/old.animgraph.ron"), 2)], "both notes go");
        assert_eq!(fs::read_to_string(dir.path().join("a/b/old.animgraph.ron")).expect("read"), upgraded);
        assert_eq!(fs::read_to_string(dir.path().join("a/other.ron")).expect("read"), VERSION_1);
        assert!(upgrade_tree(dir.path()).expect("it upgrades").is_empty());
    }

    /// Every shipped graph is at version 2 on disk: nothing in the content tree is left for
    /// `upgrade-graphs` to move (M6 A5c).
    #[test]
    fn every_shipped_graph_is_version_2() {
        let content = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../content");
        let mut folders = vec![content];
        let mut found = 0;
        while let Some(folder) = folders.pop() {
            for entry in fs::read_dir(&folder).expect("a content folder") {
                let path = entry.expect("an entry").path();
                if path.is_dir() {
                    folders.push(path);
                } else if is_source(&path) {
                    let text = fs::read_to_string(&path).expect("a graph");
                    assert_eq!(upgrade(&text).expect("it reads"), None, "{} is at version 1", path.display());
                    found += 1;
                }
            }
        }
        assert!(found > 0, "graphs ship");
    }
}
