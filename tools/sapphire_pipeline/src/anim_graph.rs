//! Animation graph documents (`.animgraph.ron`) cook to `.sgraph` (M2 N3, spec §5.13): how a character's
//! clips play, checked by [`sapphire_assets::graph`], with each clip's derived ID filled in so the
//! asset server can load the clips as the graph's dependencies.
//!
//! A person names a clip by the file it comes from, its name in that file and the character it was
//! retargeted to, because that's what a person knows. Those three hash to the same ID the clip's own
//! cook gave it ([`clip_id`]), and a cook fails if nothing in the content tree has that ID.

use std::path::{Path, PathBuf};

pub use sapphire_assets::graph::clip_id;
use sapphire_assets::graph::{AnimGraph, GraphError};
use sapphire_serialize::cooked::CookedError;
use serde::{Deserialize, Serialize};

use crate::cache::Stage;

/// Reading the document, as cache keys know it. Bump the version whenever its output changes for the
/// same file.
pub const IMPORTER: Stage = Stage { name: "animation graph document", version: 1 };

/// Checking it and deriving its clips' IDs. Bump the version whenever they change output for the same
/// document.
pub const COOKER: Stage = Stage { name: "sgraph", version: 1 };

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
    #[error(transparent)]
    Cooked(#[from] CookedError),
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

/// Reads and checks a document's bytes, with its clips' IDs filled in.
pub fn read(bytes: &[u8]) -> Result<AnimGraph, AnimGraphCookError> {
    let text = std::str::from_utf8(bytes).map_err(AnimGraphCookError::NotUtf8)?;
    let mut graph = AnimGraph::from_document(text)?;
    graph.resolve(clip_id);
    Ok(graph)
}

/// The bytes of a graph's cooked file, read back through the runtime's check.
pub fn cook(bytes: &[u8]) -> Result<Vec<u8>, AnimGraphCookError> {
    let cooked = read(bytes)?.to_cooked()?;
    AnimGraph::read(&cooked)?;
    Ok(cooked.into_vec())
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

        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../content/third_party/vrm1_constraint_twist_sample/VRM1_Constraint_Twist_Sample.vrm");
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

    /// What the look-dev character's locomotion plays: its states' clips and its blend space's samples.
    pub fn clips() -> Vec<sapphire_assets::graph::GraphClip> {
        use sapphire_assets::graph::StateMotion;

        let text = include_str!("../../../content/lookdev/character/character.animgraph.ron");
        let graph = read(text.as_bytes()).expect("it reads");
        let named: Vec<&str> = graph
            .locomotion
            .states
            .iter()
            .flat_map(|state| match &state.plays {
                StateMotion::Clip { clip, .. } => vec![clip.as_str()],
                StateMotion::Blend(space) => {
                    space.samples.iter().map(|sample| sample.clip.as_str()).collect()
                }
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
            meta::sub_asset_id(source, &format!("clip/Hit_A@{target}"))
        );
        assert_eq!(clip_id(&clip(Some("Hit_A"), None)), meta::sub_asset_id(source, "clip/Hit_A"));
        assert_eq!(clip_id(&clip(None, None)), source);
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
            &CookOptions { cook: Cook::Desktop, cache: None, locked: false },
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
}
