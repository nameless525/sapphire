//! `cargo xtask check-layers`: enforces `xtask/layers.toml` (spec §4.1, rules R1 and R8)
//! against `cargo metadata`, so it never compiles the crates it checks.
//!
//! Checked:
//!
//! - Every workspace member is placed, and every placement is a member.
//! - Runtime crates depend only on runtime crates in lower layers, or on same-layer
//!   crates through an `[allow]` edge. Apps depend only on runtime crates. Nothing
//!   depends on a tool.
//! - No `tool_only_dependencies` crate is reachable from a runtime or app crate through
//!   normal dependencies. Build-dependency subtrees (cmake, bindgen, ...) run on the
//!   build host and never ship, so they are not followed.
//! - Runtime crates declare no `dev` / `editor` features (R8).
//!
//! Dev-dependencies are exempt throughout: tests never ship, and test-support tool
//! crates (`en_testkit` in the spec) exist to be dev-depended on. The resolve graph is
//! taken with `--all-features` for every platform, so optional and target-specific
//! dependencies are covered too.

use std::collections::{BTreeMap, BTreeSet, HashMap, VecDeque};
use std::fmt;

use anyhow::{Context as _, bail};
use serde::Deserialize;

use crate::workspace;

/// Where a crate sits, from `layers.toml`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(try_from = "RawPlacement")]
pub enum Placement {
    /// Ships in the engine; may depend only on lower layers.
    Runtime { layer: u8 },
    /// A shipping binary; may depend on any runtime crate.
    App,
    /// Never shipped; may depend on anything, but nothing may depend on it.
    Tool,
}

/// `{ kind = "...", layer = N }` as written; validated into a [`Placement`].
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawPlacement {
    kind: Kind,
    layer: Option<u8>,
}

#[derive(Deserialize)]
#[serde(rename_all = "lowercase")]
enum Kind {
    Runtime,
    App,
    Tool,
}

impl TryFrom<RawPlacement> for Placement {
    type Error = &'static str;

    fn try_from(raw: RawPlacement) -> Result<Self, Self::Error> {
        match (raw.kind, raw.layer) {
            (Kind::Runtime, Some(layer)) => Ok(Self::Runtime { layer }),
            (Kind::Runtime, None) => Err("runtime crates need a `layer`"),
            (Kind::App | Kind::Tool, Some(_)) => Err("only runtime crates have a `layer`"),
            (Kind::App, None) => Ok(Self::App),
            (Kind::Tool, None) => Ok(Self::Tool),
        }
    }
}

impl fmt::Display for Placement {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Runtime { layer } => write!(f, "L{layer}"),
            Self::App => f.write_str("app"),
            Self::Tool => f.write_str("tool"),
        }
    }
}

/// The parsed `xtask/layers.toml`.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    crates: BTreeMap<String, Placement>,
    #[serde(default)]
    allow: Allow,
    #[serde(default)]
    tool_only_dependencies: ToolOnlyDependencies,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct Allow {
    /// Same-layer dependencies, as `[from, to]` pairs.
    #[serde(default)]
    edges: Vec<(String, String)>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct ToolOnlyDependencies {
    #[serde(default)]
    crates: Vec<String>,
}

impl Config {
    pub fn parse(text: &str) -> anyhow::Result<Self> {
        Ok(toml::from_str(text)?)
    }
}

/// The subset of `cargo metadata --format-version 1` output the check reads.
#[derive(Debug, Deserialize)]
pub struct Metadata {
    packages: Vec<Package>,
    workspace_members: Vec<String>,
    resolve: Option<Resolve>,
}

#[derive(Debug, Deserialize)]
struct Package {
    id: String,
    name: String,
    #[serde(default)]
    features: BTreeMap<String, Vec<String>>,
}

#[derive(Debug, Deserialize)]
struct Resolve {
    nodes: Vec<Node>,
}

#[derive(Debug, Deserialize)]
struct Node {
    id: String,
    #[serde(default)]
    deps: Vec<NodeDep>,
}

#[derive(Debug, Deserialize)]
struct NodeDep {
    pkg: String,
    #[serde(default)]
    dep_kinds: Vec<DepKindInfo>,
}

#[derive(Debug, Deserialize)]
struct DepKindInfo {
    /// `null` for normal dependencies, else `"dev"` or `"build"`.
    kind: Option<String>,
}

impl NodeDep {
    fn is_normal(&self) -> bool {
        // Cargo older than 1.41 omitted `dep_kinds`; treat that as normal.
        self.dep_kinds.is_empty() || self.dep_kinds.iter().any(|k| k.kind.is_none())
    }

    fn is_dev_only(&self) -> bool {
        !self.dep_kinds.is_empty() && self.dep_kinds.iter().all(|k| k.kind.as_deref() == Some("dev"))
    }
}

/// One broken rule.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Violation {
    Unplaced { krate: String },
    NotAMember { krate: String },
    BadAllowEdge { from: String, to: String },
    Dependency { from: String, from_placement: Placement, to: String, to_placement: Placement },
    ToolOnlyReachable { krate: String, dependency: String, path: Vec<String> },
    DevFeature { krate: String, feature: String },
}

// Sorting violations groups them by kind, then by crate.
impl PartialOrd for Placement {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Placement {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        let key = |p: &Self| match *p {
            Self::Runtime { layer } => (0, layer),
            Self::App => (1, 0),
            Self::Tool => (2, 0),
        };
        key(self).cmp(&key(other))
    }
}

impl fmt::Display for Violation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unplaced { krate } => {
                write!(f, "`{krate}` is a workspace member but has no placement in [crates]")
            }
            Self::NotAMember { krate } => {
                write!(f, "[crates] places `{krate}`, which is not a workspace member")
            }
            Self::BadAllowEdge { from, to } => {
                write!(f, "[allow] edge `{from}` -> `{to}` must join two runtime crates in the same layer")
            }
            Self::Dependency { from, from_placement, to, to_placement } => {
                let rule = match (from_placement, to_placement) {
                    (_, Placement::Tool) => "nothing may depend on a tool (R1)",
                    (Placement::App, _) => "apps may depend only on runtime crates",
                    (Placement::Runtime { .. }, Placement::App) => "runtime crates may not depend on apps",
                    (Placement::Runtime { layer: a }, Placement::Runtime { layer: b }) if a == b => {
                        "same-layer dependencies need an [allow] edge"
                    }
                    _ => "runtime crates may depend only on lower layers (R1)",
                };
                write!(f, "`{from}` ({from_placement}) depends on `{to}` ({to_placement}): {rule}")
            }
            Self::ToolOnlyReachable { krate, dependency, path } => write!(
                f,
                "tool-only crate `{dependency}` is in the dependency tree of `{krate}`: {}",
                path.join(" -> ")
            ),
            Self::DevFeature { krate, feature } => write!(
                f,
                "runtime crate `{krate}` declares feature `{feature}`: features select platform or \
                 backend only; dev tooling enters through the leaf app (R8)"
            ),
        }
    }
}

/// Runs `cargo metadata` on the workspace and checks it, printing every violation.
pub fn run() -> anyhow::Result<()> {
    let config_path = workspace::root().join("xtask/layers.toml");
    let config = std::fs::read_to_string(&config_path)
        .with_context(|| format!("failed to read {}", config_path.display()))?;
    let config = Config::parse(&config).with_context(|| format!("invalid {}", config_path.display()))?;

    let json =
        workspace::output(workspace::cargo().args(["metadata", "--format-version", "1", "--all-features"]))?;
    let metadata: Metadata = serde_json::from_str(&json).context("unexpected `cargo metadata` output")?;

    let violations = check(&config, &metadata)?;
    if violations.is_empty() {
        eprintln!("check-layers: {} crates placed, no violations", config.crates.len());
        return Ok(());
    }
    for violation in &violations {
        eprintln!("error: {violation}");
    }
    bail!(
        "{} layering violation(s); see xtask/layers.toml and docs/adr/0001-crate-layering.md",
        violations.len()
    )
}

/// Checks `metadata` against `config`, returning violations sorted and deduplicated.
pub fn check(config: &Config, metadata: &Metadata) -> anyhow::Result<Vec<Violation>> {
    let packages: HashMap<&str, &Package> = metadata.packages.iter().map(|p| (p.id.as_str(), p)).collect();
    let package = |id: &str| packages.get(id).copied().with_context(|| format!("unknown package id {id}"));
    let resolve = metadata.resolve.as_ref().context("`cargo metadata` output has no resolve graph")?;
    let nodes: HashMap<&str, &Node> = resolve.nodes.iter().map(|n| (n.id.as_str(), n)).collect();

    let members =
        metadata.workspace_members.iter().map(|id| package(id)).collect::<anyhow::Result<Vec<_>>>()?;
    let member_names: BTreeSet<&str> = members.iter().map(|p| p.name.as_str()).collect();
    let placement = |name: &str| config.crates.get(name).copied();

    let mut violations = Vec::new();
    for name in &member_names {
        if placement(name).is_none() {
            violations.push(Violation::Unplaced { krate: (*name).to_owned() });
        }
    }
    for name in config.crates.keys() {
        if !member_names.contains(name.as_str()) {
            violations.push(Violation::NotAMember { krate: name.clone() });
        }
    }
    for (from, to) in &config.allow.edges {
        let same_layer = matches!(
            (placement(from), placement(to)),
            (Some(Placement::Runtime { layer: a }), Some(Placement::Runtime { layer: b })) if a == b
        );
        if !same_layer {
            violations.push(Violation::BadAllowEdge { from: from.clone(), to: to.clone() });
        }
    }

    let tool_only: BTreeSet<&str> = config.tool_only_dependencies.crates.iter().map(String::as_str).collect();
    // A dependency on a workspace tool is reported directly; don't also report its tree.
    let tool_members: BTreeSet<&str> = members
        .iter()
        .filter(|p| placement(&p.name) == Some(Placement::Tool))
        .map(|p| p.id.as_str())
        .collect();
    for member in &members {
        let Some(from_placement) = placement(&member.name) else { continue };
        let node = nodes
            .get(member.id.as_str())
            .with_context(|| format!("{} is not in the resolve graph", member.name))?;

        for dep in node.deps.iter().filter(|d| !d.is_dev_only()) {
            let to = package(&dep.pkg)?;
            if !member_names.contains(to.name.as_str()) {
                continue;
            }
            let Some(to_placement) = placement(&to.name) else { continue };
            let allowed_edge = config.allow.edges.iter().any(|(f, t)| *f == member.name && *t == to.name);
            if !dependency_allowed(from_placement, to_placement, allowed_edge) {
                violations.push(Violation::Dependency {
                    from: member.name.clone(),
                    from_placement,
                    to: to.name.clone(),
                    to_placement,
                });
            }
        }

        if from_placement == Placement::Tool {
            continue;
        }
        for (dependency, path) in
            reachable_tool_only(&member.id, &tool_only, &tool_members, &nodes, &packages)
        {
            violations.push(Violation::ToolOnlyReachable { krate: member.name.clone(), dependency, path });
        }
        if matches!(from_placement, Placement::Runtime { .. }) {
            for feature in member.features.keys().filter(|f| is_dev_feature(f)) {
                violations
                    .push(Violation::DevFeature { krate: member.name.clone(), feature: feature.clone() });
            }
        }
    }

    violations.sort();
    violations.dedup();
    Ok(violations)
}

fn dependency_allowed(from: Placement, to: Placement, allowed_edge: bool) -> bool {
    match (from, to) {
        (_, Placement::Tool) => false,
        (Placement::Tool, _) => true,
        (Placement::App, Placement::Runtime { .. }) => true,
        (Placement::App, Placement::App) | (Placement::Runtime { .. }, Placement::App) => false,
        (Placement::Runtime { layer: from }, Placement::Runtime { layer: to }) => {
            to < from || (to == from && allowed_edge)
        }
    }
}

/// Breadth-first search over normal dependencies from `root`, returning each tool-only
/// crate reached with the shortest path to it (as package names, `root` first). The
/// search doesn't enter the package ids in `stop`.
fn reachable_tool_only(
    root: &str,
    tool_only: &BTreeSet<&str>,
    stop: &BTreeSet<&str>,
    nodes: &HashMap<&str, &Node>,
    packages: &HashMap<&str, &Package>,
) -> Vec<(String, Vec<String>)> {
    let name = |id: &str| packages.get(id).map_or_else(|| id.to_owned(), |p| p.name.clone());
    let mut parent: HashMap<&str, &str> = HashMap::new();
    let mut queue = VecDeque::from([root]);
    let mut found = Vec::new();
    while let Some(id) = queue.pop_front() {
        if id != root && tool_only.contains(name(id).as_str()) {
            let mut path = vec![name(id)];
            let mut cursor = id;
            while let Some(&up) = parent.get(cursor) {
                path.push(name(up));
                cursor = up;
            }
            path.reverse();
            found.push((name(id), path));
            continue; // Its own subtree adds nothing new to report.
        }
        let Some(node) = nodes.get(id) else { continue };
        for dep in node.deps.iter().filter(|d| d.is_normal()) {
            let dep_id = dep.pkg.as_str();
            if dep_id != root && !stop.contains(dep_id) && !parent.contains_key(dep_id) {
                parent.insert(dep_id, id);
                queue.push_back(dep_id);
            }
        }
    }
    found
}

/// Rule R8: runtime features select platform or backend; dev tooling enters through the
/// leaf binary.
fn is_dev_feature(feature: &str) -> bool {
    ["dev", "editor"].iter().any(|banned| {
        feature == *banned || feature.strip_prefix(banned).is_some_and(|rest| rest.starts_with(['-', '_']))
    })
}

#[cfg(test)]
mod tests {
    use serde_json::{Value, json};

    use super::*;

    const NORMAL: Option<&str> = None;
    const BUILD: Option<&str> = Some("build");
    const DEV: Option<&str> = Some("dev");

    fn id(name: &str) -> String {
        if name.starts_with("sapphire_") || name == "xtask" {
            format!("path+file:///ws/{name}#0.1.0")
        } else {
            format!("registry+https://github.com/rust-lang/crates.io-index#{name}@1.0.0")
        }
    }

    /// Builds `cargo metadata`-shaped JSON: `members` are workspace crates (with optional
    /// features), every other name in `deps` is a registry crate.
    fn metadata(members: &[(&str, &[&str])], deps: &[(&str, &str, Option<&str>)]) -> Metadata {
        let mut names: BTreeSet<&str> = members.iter().map(|(name, _)| *name).collect();
        names.extend(deps.iter().flat_map(|(from, to, _)| [*from, *to]));
        let features: HashMap<&str, &[&str]> = members.iter().copied().collect();
        let packages: Vec<Value> = names
            .iter()
            .map(|name| {
                let features: BTreeMap<&str, Vec<&str>> =
                    features.get(name).map_or(&[][..], |f| f).iter().map(|f| (*f, vec![])).collect();
                json!({ "id": id(name), "name": name, "version": "1.0.0", "features": features })
            })
            .collect();
        let nodes: Vec<Value> = names
            .iter()
            .map(|name| {
                let deps: Vec<Value> = deps
                    .iter()
                    .filter(|(from, _, _)| from == name)
                    .map(|(_, to, kind)| {
                        json!({ "name": to, "pkg": id(to), "dep_kinds": [{ "kind": kind, "target": null }] })
                    })
                    .collect();
                json!({ "id": id(name), "deps": deps, "features": [] })
            })
            .collect();
        let members: Vec<String> = members.iter().map(|(name, _)| id(name)).collect();
        serde_json::from_value(json!({
            "packages": packages,
            "workspace_members": members,
            "resolve": { "nodes": nodes, "root": null },
            "version": 1,
        }))
        .expect("fixture matches the cargo metadata schema")
    }

    fn config() -> Config {
        Config::parse(
            r#"
            [crates]
            sapphire_core = { kind = "runtime", layer = 0 }
            sapphire_math = { kind = "runtime", layer = 0 }
            sapphire_platform = { kind = "runtime", layer = 1 }
            sapphire_render = { kind = "runtime", layer = 3 }
            sapphire_sandbox = { kind = "app" }
            sapphire_pipeline = { kind = "tool" }
            xtask = { kind = "tool" }

            [allow]
            edges = [["sapphire_math", "sapphire_core"]]

            [tool_only_dependencies]
            crates = ["gltf", "toml"]
            "#,
        )
        .unwrap()
    }

    const MEMBERS: &[(&str, &[&str])] = &[
        ("sapphire_core", &[]),
        ("sapphire_math", &[]),
        ("sapphire_platform", &["vulkan"]),
        ("sapphire_render", &[]),
        ("sapphire_sandbox", &["dev"]),
        ("sapphire_pipeline", &[]),
        ("xtask", &[]),
    ];

    /// A dependency graph that follows every rule.
    const CLEAN: &[(&str, &str, Option<&str>)] = &[
        ("sapphire_math", "sapphire_core", NORMAL),
        ("sapphire_platform", "sapphire_core", NORMAL),
        ("sapphire_platform", "sdl3-sys", NORMAL),
        ("sdl3-sys", "cmake", BUILD),
        ("cmake", "toml", NORMAL), // Only reachable through a build-dependency.
        ("sapphire_render", "sapphire_platform", NORMAL),
        ("sapphire_render", "sapphire_pipeline", DEV), // Test support may be a tool.
        ("sapphire_core", "gltf", DEV),
        ("sapphire_sandbox", "sapphire_render", NORMAL),
        ("sapphire_pipeline", "sapphire_core", NORMAL),
        ("sapphire_pipeline", "gltf", NORMAL),
        ("xtask", "toml", NORMAL),
    ];

    fn check_with(extra: &[(&str, &str, Option<&str>)]) -> Vec<Violation> {
        let deps: Vec<_> = CLEAN.iter().chain(extra).copied().collect();
        check(&config(), &metadata(MEMBERS, &deps)).unwrap()
    }

    #[test]
    fn shipped_config_parses() {
        let config = Config::parse(include_str!("../layers.toml")).unwrap();
        assert_eq!(config.crates["sapphire_core"], Placement::Runtime { layer: 0 });
        assert_eq!(config.crates["sapphire_pipeline"], Placement::Tool);
    }

    #[test]
    fn config_rejects_malformed_placements() {
        let err = Config::parse("[crates]\na = { kind = \"runtime\" }").unwrap_err();
        assert!(err.to_string().contains("runtime crates need a `layer`"), "{err}");
        assert!(Config::parse("[crates]\na = { kind = \"plugin\" }").is_err());
        assert!(Config::parse("[crates]\na = { kind = \"tool\", layer = 2 }").is_err());
        assert!(Config::parse("[crates]\n[extra]").is_err());
    }

    #[test]
    fn clean_workspace_passes() {
        assert_eq!(check_with(&[]), []);
    }

    #[test]
    fn every_member_must_be_placed_and_every_placement_a_member() {
        let members = [MEMBERS, &[("sapphire_new", &[][..])]].concat();
        let violations = check(&config(), &metadata(&members[1..], CLEAN)).unwrap();
        assert_eq!(
            violations,
            [
                Violation::Unplaced { krate: "sapphire_new".into() },
                Violation::NotAMember { krate: "sapphire_core".into() },
            ]
        );
    }

    #[test]
    fn runtime_crates_depend_only_downward() {
        let violations = check_with(&[
            ("sapphire_core", "sapphire_platform", NORMAL),
            ("sapphire_core", "sapphire_math", BUILD),
            ("sapphire_core", "sapphire_render", DEV),
        ]);
        assert_eq!(
            violations,
            [
                Violation::Dependency {
                    from: "sapphire_core".into(),
                    from_placement: Placement::Runtime { layer: 0 },
                    to: "sapphire_math".into(),
                    to_placement: Placement::Runtime { layer: 0 },
                },
                Violation::Dependency {
                    from: "sapphire_core".into(),
                    from_placement: Placement::Runtime { layer: 0 },
                    to: "sapphire_platform".into(),
                    to_placement: Placement::Runtime { layer: 1 },
                },
            ]
        );
        assert_eq!(
            violations[1].to_string(),
            "`sapphire_core` (L0) depends on `sapphire_platform` (L1): runtime crates may depend only \
             on lower layers (R1)"
        );
        assert!(violations[0].to_string().ends_with("same-layer dependencies need an [allow] edge"));
    }

    #[test]
    fn nothing_depends_on_tools_or_apps() {
        let violations = check_with(&[
            ("sapphire_sandbox", "sapphire_pipeline", NORMAL),
            ("sapphire_render", "sapphire_pipeline", BUILD),
            ("xtask", "sapphire_pipeline", NORMAL),
            ("sapphire_render", "sapphire_sandbox", NORMAL),
            ("sapphire_pipeline", "sapphire_sandbox", NORMAL), // Tools may use apps.
        ]);
        let edges: Vec<(&str, &str)> = violations
            .iter()
            .map(|v| match v {
                Violation::Dependency { from, to, .. } => (from.as_str(), to.as_str()),
                other => panic!("unexpected {other}"),
            })
            .collect();
        assert_eq!(
            edges,
            [
                ("sapphire_render", "sapphire_pipeline"),
                ("sapphire_render", "sapphire_sandbox"),
                ("sapphire_sandbox", "sapphire_pipeline"),
                ("xtask", "sapphire_pipeline"),
            ]
        );
        assert!(violations[2].to_string().ends_with("nothing may depend on a tool (R1)"));
    }

    #[test]
    fn tool_only_crates_never_reach_shipping_crates() {
        let violations = check_with(&[
            ("sapphire_platform", "png-helper", NORMAL),
            ("png-helper", "gltf", NORMAL),
            ("sapphire_sandbox", "toml", NORMAL),
        ]);
        assert_eq!(
            violations,
            [
                Violation::ToolOnlyReachable {
                    krate: "sapphire_platform".into(),
                    dependency: "gltf".into(),
                    path: vec!["sapphire_platform".into(), "png-helper".into(), "gltf".into()],
                },
                // Reached again through the dependency on sapphire_platform.
                Violation::ToolOnlyReachable {
                    krate: "sapphire_render".into(),
                    dependency: "gltf".into(),
                    path: vec![
                        "sapphire_render".into(),
                        "sapphire_platform".into(),
                        "png-helper".into(),
                        "gltf".into(),
                    ],
                },
                Violation::ToolOnlyReachable {
                    krate: "sapphire_sandbox".into(),
                    dependency: "gltf".into(),
                    path: vec![
                        "sapphire_sandbox".into(),
                        "sapphire_render".into(),
                        "sapphire_platform".into(),
                        "png-helper".into(),
                        "gltf".into(),
                    ],
                },
                Violation::ToolOnlyReachable {
                    krate: "sapphire_sandbox".into(),
                    dependency: "toml".into(),
                    path: vec!["sapphire_sandbox".into(), "toml".into()],
                },
            ]
        );
        assert_eq!(
            violations[0].to_string(),
            "tool-only crate `gltf` is in the dependency tree of `sapphire_platform`: \
             sapphire_platform -> png-helper -> gltf"
        );
    }

    #[test]
    fn allow_edges_must_join_one_layer() {
        let mut config = config();
        config.allow.edges.push(("sapphire_render".into(), "sapphire_core".into()));
        config.allow.edges.push(("sapphire_sandbox".into(), "sapphire_render".into()));
        let violations = check(&config, &metadata(MEMBERS, CLEAN)).unwrap();
        assert_eq!(
            violations,
            [
                Violation::BadAllowEdge { from: "sapphire_render".into(), to: "sapphire_core".into() },
                Violation::BadAllowEdge { from: "sapphire_sandbox".into(), to: "sapphire_render".into() },
            ]
        );
    }

    #[test]
    fn runtime_crates_have_no_dev_features() {
        let members = [("sapphire_core", &["dev", "editor-gizmos", "developer", "metal"][..])];
        let violations = check(&config(), &metadata(&members, &[])).unwrap();
        let features: Vec<&str> = violations
            .iter()
            .filter_map(|v| match v {
                Violation::DevFeature { feature, .. } => Some(feature.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(features, ["dev", "editor-gizmos"]);
    }
}
