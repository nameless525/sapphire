//! Where an animation graph's nodes sit on the editor's canvas (M6 A5a, spec §5.13's "Graph v2"): a
//! sidecar beside the graph, `<name>.animgraph.layout.ron`, that no cook reads. So a dragged node
//! changes this file alone, recooks nothing, and no editor type enters the graph's.
//!
//! A node is named by its path in the graph document, the one the compiler names a refusal by
//! ([`AnimGraph::node_paths`]): `machines["locomotion"].states["ground"]`, `layers["combat"]`. A node the
//! layout doesn't place goes where the canvas puts new ones; a place for a node the graph no longer has
//! is [stale](GraphLayout::stale), which the pane can drop.

use std::path::{Path, PathBuf};

use bevy_reflect::{Reflect, ReflectDeserialize, ReflectSerialize};
use sapphire_assets::graph::v2::AnimGraph;
use sapphire_reflect::Versioned;
use sapphire_serialize::DocumentError;
use serde::{Deserialize, Serialize};

/// What a graph's layout sidecar's file name ends with.
pub const LAYOUT_SUFFIX: &str = ".animgraph.layout.ron";

/// The graph document's suffix, which a layout's path replaces.
const GRAPH_SUFFIX: &str = ".animgraph.ron";

/// A graph's layout. See the [module docs](self).
#[derive(Reflect, Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[reflect(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[type_path = "sapphire::editor"]
pub struct GraphLayout {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub nodes: Vec<NodePlace>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub comments: Vec<Comment>,
}

impl Versioned for GraphLayout {
    /// 1: M6 A5a.
    const VERSION: u32 = 1;
}

/// Where one node sits.
#[derive(Reflect, Clone, Debug, PartialEq, Serialize, Deserialize)]
#[reflect(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NodePlace {
    /// The node's path in the graph document.
    pub node: String,
    /// Its top-left corner on the canvas, in canvas units.
    pub at: [f32; 2],
}

/// A note on the canvas.
#[derive(Reflect, Clone, Debug, PartialEq, Serialize, Deserialize)]
#[reflect(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Comment {
    pub at: [f32; 2],
    pub size: [f32; 2],
    pub text: String,
}

impl GraphLayout {
    pub fn from_document(text: &str) -> Result<Self, DocumentError> {
        sapphire_serialize::from_ron(text)
    }

    pub fn to_document(&self) -> Result<String, DocumentError> {
        sapphire_serialize::to_ron(self)
    }

    /// Where `node` sits, if the layout places it.
    #[must_use]
    pub fn place(&self, node: &str) -> Option<[f32; 2]> {
        self.nodes.iter().find(|place| place.node == node).map(|place| place.at)
    }

    /// The nodes it places that `graph` doesn't have, in its order.
    #[must_use]
    pub fn stale(&self, graph: &AnimGraph) -> Vec<&str> {
        let nodes: Vec<String> = graph.node_paths().collect();
        self.nodes
            .iter()
            .map(|place| place.node.as_str())
            .filter(|node| !nodes.iter().any(|path| path == node))
            .collect()
    }
}

/// The layout sidecar of the graph at `graph`: `fighter.animgraph.ron` → `fighter.animgraph.layout.ron`.
/// `None` for a path that isn't a graph's.
#[must_use]
pub fn layout_path(graph: &Path) -> Option<PathBuf> {
    let name = graph.file_name()?.to_str()?;
    let stem = name.strip_suffix(GRAPH_SUFFIX)?;
    Some(graph.with_file_name(format!("{stem}{LAYOUT_SUFFIX}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    const GRAPH: &str = r#"(
    type: "sapphire::assets::AnimGraph",
    version: 2,
    data: (
        clips: [(name: "idle", source: "9a29e8c0-f131-4ea4-98b9-04879740413d", clip: "Idle_A")],
        machines: [(name: "locomotion", start: "ground", states: [(name: "ground", plays: Clip(clip: "idle"))])],
        layers: [(name: "locomotion", plays: Machine((machine: "locomotion")))],
    ),
)"#;

    fn layout() -> GraphLayout {
        GraphLayout {
            nodes: vec![
                NodePlace {
                    node: r#"machines["locomotion"].states["ground"]"#.to_owned(),
                    at: [120.0, -40.5],
                },
                NodePlace { node: r#"machines["air"]"#.to_owned(), at: [0.0, 0.0] },
            ],
            comments: vec![Comment {
                at: [10.0, 10.0],
                size: [200.0, 80.0],
                text: "starts and stops".to_owned(),
            }],
        }
    }

    #[test]
    fn a_layout_round_trips() {
        let text = layout().to_document().expect("writes");
        assert!(text.contains("sapphire::editor::GraphLayout"), "{text}");
        assert_eq!(GraphLayout::from_document(&text).expect("reads"), layout());
        assert!(
            GraphLayout::from_document(&text.replace("text:", "words:")).is_err(),
            "unknown fields are refused"
        );
    }

    #[test]
    fn a_layout_finds_places_and_stale_nodes_by_the_graphs_paths() {
        let graph = AnimGraph::from_document(GRAPH).expect("a graph");
        let layout = layout();
        assert_eq!(layout.place(r#"machines["locomotion"].states["ground"]"#), Some([120.0, -40.5]));
        assert_eq!(layout.place(r#"layers["locomotion"]"#), None);
        assert_eq!(layout.stale(&graph), [r#"machines["air"]"#]);
    }

    #[test]
    fn a_layouts_path_sits_beside_its_graph() {
        assert_eq!(
            layout_path(Path::new("combat/fighter.animgraph.ron")),
            Some(PathBuf::from("combat/fighter.animgraph.layout.ron"))
        );
        assert_eq!(layout_path(Path::new("combat/fighter.ron")), None);
    }
}
