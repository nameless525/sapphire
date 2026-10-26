//! `cargo xtask check-layers`: enforces `xtask/layers.toml` (spec §4.1, rules R1 and R8)
//! against `cargo metadata`, so it never compiles the crates it checks.
//!
//! The checked crates are the *local* packages: workspace members plus every path or git
//! dependency outside the workspace (vendored crates). Checked:
//!
//! - Every local package is placed, and every placement names a local package.
//! - Dependencies between local packages follow the placements: runtime crates depend
//!   only on lower layers, or on the same layer through an `[allow]` edge; apps and
//!   dev-only crates depend on runtime crates; only apps reach dev-only crates, and only
//!   through a direct optional dependency; nothing depends on a tool or an app. An edge
//!   counts even when it runs through registry crates or unplaced local packages.
//! - No `tool_only_dependencies` crate is reachable from a shipping (runtime, app or
//!   dev-only) package through normal dependencies, including through tools. Build-
//!   dependency subtrees (cmake, bindgen, ...) run on the build host and never ship, so
//!   they are not followed.
//! - No `dev_only_dependencies` crate (the overlay's egui, hot reload's file watching) is
//!   reachable from a runtime or app package except through a dev-only crate, so a build
//!   without the app's `dev` feature never links one.
//! - Every feature a local package declares is listed in its placement, and every listed
//!   feature exists (R8: runtime features select platform or backend; dev tooling enters
//!   through apps and dev-only crates).
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
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Placement {
    /// Ships in the engine; may depend only on lower layers.
    Runtime { layer: u8 },
    /// A shipping binary; may depend on runtime crates, and on dev-only crates through
    /// optional dependencies.
    App,
    /// Ships only in development builds (debug overlay, live link): an app enables it
    /// through an optional dependency. May depend on runtime and other dev-only crates.
    DevOnly,
    /// Never shipped; may depend on anything, but nothing may depend on it.
    Tool,
}

/// One `[crates]` entry: a placement and the features the crate may declare.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(try_from = "RawEntry")]
pub struct Entry {
    pub placement: Placement,
    pub features: BTreeSet<String>,
}

/// `{ kind = "...", layer = N, features = [...] }` as written; validated into an [`Entry`].
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawEntry {
    kind: Kind,
    layer: Option<u8>,
    #[serde(default)]
    features: BTreeSet<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
enum Kind {
    Runtime,
    App,
    DevOnly,
    Tool,
}

impl TryFrom<RawEntry> for Entry {
    type Error = &'static str;

    fn try_from(raw: RawEntry) -> Result<Self, Self::Error> {
        let placement = match (raw.kind, raw.layer) {
            (Kind::Runtime, Some(layer)) => Placement::Runtime { layer },
            (Kind::Runtime, None) => return Err("runtime crates need a `layer`"),
            (Kind::App | Kind::DevOnly | Kind::Tool, Some(_)) => {
                return Err("only runtime crates have a `layer`");
            }
            (Kind::App, None) => Placement::App,
            (Kind::DevOnly, None) => Placement::DevOnly,
            (Kind::Tool, None) => Placement::Tool,
        };
        Ok(Self { placement, features: raw.features })
    }
}

impl Placement {
    /// Whether the crate ends up in a binary we hand to players or testers.
    fn ships(self) -> bool {
        self != Self::Tool
    }
}

impl fmt::Display for Placement {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Runtime { layer } => write!(f, "L{layer}"),
            Self::App => f.write_str("app"),
            Self::DevOnly => f.write_str("dev-only"),
            Self::Tool => f.write_str("tool"),
        }
    }
}

/// The parsed `xtask/layers.toml`.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    crates: BTreeMap<String, Entry>,
    #[serde(default)]
    allow: Allow,
    #[serde(default)]
    tool_only_dependencies: ToolOnlyDependencies,
    #[serde(default)]
    dev_only_dependencies: DevOnlyDependencies,
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

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct DevOnlyDependencies {
    #[serde(default)]
    crates: Vec<String>,
}

impl Config {
    pub fn parse(text: &str) -> anyhow::Result<Self> {
        Ok(toml::from_str(text)?)
    }

    fn placement(&self, name: &str) -> Option<Placement> {
        self.crates.get(name).map(|entry| entry.placement)
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
    /// `None` for path packages; `registry+…`, `sparse+…` or `git+…` otherwise.
    source: Option<String>,
    #[serde(default)]
    features: BTreeMap<String, Vec<String>>,
    #[serde(default)]
    dependencies: Vec<Declaration>,
}

impl Package {
    /// Path and git packages: ours, or vendored into the repository.
    fn is_local(&self) -> bool {
        self.source.as_deref().is_none_or(|source| source.starts_with("git+"))
    }

    /// Whether every normal dependency on package `name` is declared optional.
    fn depends_optionally_on(&self, name: &str) -> bool {
        let mut normal = self.dependencies.iter().filter(|d| d.name == name && d.kind.is_none());
        normal.next().is_some_and(|first| first.optional && normal.all(|d| d.optional))
    }
}

/// A dependency as the manifest declares it.
#[derive(Debug, Deserialize)]
struct Declaration {
    /// The package name, even when the dependency is renamed.
    name: String,
    /// `null` for normal dependencies, else `"dev"` or `"build"`.
    kind: Option<String>,
    #[serde(default)]
    optional: bool,
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
    Unplaced {
        krate: String,
        member: bool,
    },
    NotLocal {
        krate: String,
    },
    BadAllowEdge {
        from: String,
        to: String,
    },
    /// `via` lists the packages between `from` and `to`; empty for a direct dependency.
    Dependency {
        from: String,
        from_placement: Placement,
        to: String,
        to_placement: Placement,
        via: Vec<String>,
    },
    ToolOnlyReachable {
        krate: String,
        dependency: String,
        path: Vec<String>,
    },
    DevOnlyReachable {
        krate: String,
        dependency: String,
        path: Vec<String>,
    },
    UnlistedFeature {
        krate: String,
        feature: String,
    },
    MissingFeature {
        krate: String,
        feature: String,
    },
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
            Self::DevOnly => (2, 0),
            Self::Tool => (3, 0),
        };
        key(self).cmp(&key(other))
    }
}

impl fmt::Display for Violation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unplaced { krate, member: true } => {
                write!(f, "`{krate}` is a workspace member but has no placement in [crates]")
            }
            Self::Unplaced { krate, member: false } => write!(
                f,
                "`{krate}` is a path or git dependency (a vendored crate) but has no placement in [crates]"
            ),
            Self::NotLocal { krate } => write!(
                f,
                "[crates] places `{krate}`, which is neither a workspace member nor a path or git dependency"
            ),
            Self::BadAllowEdge { from, to } => {
                write!(f, "[allow] edge `{from}` -> `{to}` must join two runtime crates in the same layer")
            }
            Self::Dependency { from, from_placement, to, to_placement, via } => {
                write!(f, "`{from}` ({from_placement}) depends on `{to}` ({to_placement})")?;
                if !via.is_empty() {
                    write!(f, " through {}", via.join(" -> "))?;
                }
                write!(f, ": {}", rule(*from_placement, *to_placement))
            }
            Self::ToolOnlyReachable { krate, dependency, path } => write!(
                f,
                "tool-only crate `{dependency}` is in the dependency tree of `{krate}`: {}",
                path.join(" -> ")
            ),
            Self::DevOnlyReachable { krate, dependency, path } => write!(
                f,
                "dev-only crate `{dependency}` is in the dependency tree of `{krate}` outside any dev-only \
                 crate, so builds without `dev` would link it (R8): {}",
                path.join(" -> ")
            ),
            Self::UnlistedFeature { krate, feature } => write!(
                f,
                "`{krate}` declares feature `{feature}`, which its [crates] entry doesn't list: runtime \
                 features select platform or backend only; dev tooling enters through apps and \
                 dev-only crates (R8)"
            ),
            Self::MissingFeature { krate, feature } => {
                write!(f, "[crates] lists feature `{feature}` for `{krate}`, which doesn't declare it")
            }
        }
    }
}

/// The rule a dependency from `from` to `to` breaks.
fn rule(from: Placement, to: Placement) -> &'static str {
    match (from, to) {
        (_, Placement::Tool) => "nothing may depend on a tool (R1)",
        (_, Placement::App) => "nothing may depend on an app",
        (Placement::App, Placement::DevOnly) => {
            "apps may reach a dev-only crate only through a direct optional dependency (R8)"
        }
        (_, Placement::DevOnly) => "only apps may depend on dev-only crates (R8)",
        (Placement::Runtime { layer: a }, Placement::Runtime { layer: b }) if a == b => {
            "same-layer dependencies need an [allow] edge"
        }
        _ => "runtime crates may depend only on lower layers (R1)",
    }
}

/// Runs `cargo metadata` on the workspace and checks it, printing every violation.
pub fn run() -> anyhow::Result<()> {
    let config_path = workspace::root().join("xtask/layers.toml");
    let config = std::fs::read_to_string(&config_path)
        .with_context(|| format!("failed to read {}", config_path.display()))?;
    let config = Config::parse(&config).with_context(|| format!("invalid {}", config_path.display()))?;

    let json =
        workspace::output(workspace::cargo("metadata").args(["--format-version", "1", "--all-features"]))?;
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
    let resolve = metadata.resolve.as_ref().context("`cargo metadata` output has no resolve graph")?;
    let graph = Graph { packages, nodes: resolve.nodes.iter().map(|n| (n.id.as_str(), n)).collect() };

    let members: BTreeSet<&str> = metadata.workspace_members.iter().map(String::as_str).collect();
    // Only packages in the resolve graph count; `packages` may list more.
    let local: Vec<&Package> = resolve
        .nodes
        .iter()
        .map(|node| graph.package(&node.id))
        .collect::<anyhow::Result<Vec<_>>>()?
        .into_iter()
        .filter(|package| members.contains(package.id.as_str()) || package.is_local())
        .collect();
    let local_names: BTreeSet<&str> = local.iter().map(|p| p.name.as_str()).collect();

    let mut violations = Vec::new();
    for package in &local {
        if !config.crates.contains_key(&package.name) {
            let member = members.contains(package.id.as_str());
            violations.push(Violation::Unplaced { krate: package.name.clone(), member });
        }
    }
    for name in config.crates.keys() {
        if !local_names.contains(name.as_str()) {
            violations.push(Violation::NotLocal { krate: name.clone() });
        }
    }
    for (from, to) in &config.allow.edges {
        let same_layer = matches!(
            (config.placement(from), config.placement(to)),
            (Some(Placement::Runtime { layer: a }), Some(Placement::Runtime { layer: b })) if a == b
        );
        if !same_layer {
            violations.push(Violation::BadAllowEdge { from: from.clone(), to: to.clone() });
        }
    }

    let tool_only: BTreeSet<&str> = config.tool_only_dependencies.crates.iter().map(String::as_str).collect();
    let dev_only: BTreeSet<&str> = config.dev_only_dependencies.crates.iter().map(String::as_str).collect();
    for package in &local {
        let Some(entry) = config.crates.get(&package.name) else { continue };
        let from_placement = entry.placement;

        for (to, via) in graph.placed_dependencies(&package.id, config) {
            let to_package = graph.package(to)?;
            let to_placement =
                config.placement(&to_package.name).expect("placed_dependencies stops at placed crates");
            let allowed_edge =
                config.allow.edges.iter().any(|(f, t)| *f == package.name && *t == to_package.name);
            let direct_optional = via.is_empty() && package.depends_optionally_on(&to_package.name);
            if !dependency_allowed(from_placement, to_placement, allowed_edge, direct_optional) {
                violations.push(Violation::Dependency {
                    from: package.name.clone(),
                    from_placement,
                    to: to_package.name.clone(),
                    to_placement,
                    via,
                });
            }
        }

        if from_placement.ships() {
            for (dependency, path) in graph.reachable_tool_only(&package.id, &tool_only) {
                violations.push(Violation::ToolOnlyReachable {
                    krate: package.name.clone(),
                    dependency,
                    path,
                });
            }
        }
        if matches!(from_placement, Placement::Runtime { .. } | Placement::App) {
            for (dependency, path) in graph.reachable_dev_only(&package.id, &dev_only, config) {
                violations.push(Violation::DevOnlyReachable {
                    krate: package.name.clone(),
                    dependency,
                    path,
                });
            }
        }

        for feature in package.features.keys().filter(|f| !entry.features.contains(*f)) {
            violations
                .push(Violation::UnlistedFeature { krate: package.name.clone(), feature: feature.clone() });
        }
        for feature in entry.features.iter().filter(|f| !package.features.contains_key(*f)) {
            violations
                .push(Violation::MissingFeature { krate: package.name.clone(), feature: feature.clone() });
        }
    }

    violations.sort();
    violations.dedup();
    Ok(violations)
}

fn dependency_allowed(from: Placement, to: Placement, allowed_edge: bool, direct_optional: bool) -> bool {
    match (from, to) {
        (_, Placement::Tool) => false,
        (Placement::Tool, _) => true,
        (_, Placement::App) => false,
        (Placement::App, Placement::DevOnly) => direct_optional,
        (Placement::DevOnly, Placement::DevOnly) => true,
        (Placement::Runtime { .. }, Placement::DevOnly) => false,
        (Placement::App | Placement::DevOnly, Placement::Runtime { .. }) => true,
        (Placement::Runtime { layer: from }, Placement::Runtime { layer: to }) => {
            to < from || (to == from && allowed_edge)
        }
    }
}

/// The resolve graph with its packages.
struct Graph<'a> {
    packages: HashMap<&'a str, &'a Package>,
    nodes: HashMap<&'a str, &'a Node>,
}

impl<'a> Graph<'a> {
    fn package(&self, id: &str) -> anyhow::Result<&'a Package> {
        self.packages.get(id).copied().with_context(|| format!("unknown package id {id}"))
    }

    fn name(&self, id: &str) -> String {
        self.packages.get(id).map_or_else(|| id.to_owned(), |p| p.name.clone())
    }

    /// The placed local packages `root` depends on. A direct dependency comes with an empty
    /// chain. Every placed package reached through unplaced ones (registry crates, unplaced
    /// local crates) comes once more with the shortest chain in between, even if `root` also
    /// depends on it directly: an app's optional dependency on a dev-only crate doesn't
    /// excuse an indirect one. Dev-only edges are skipped, and the search stops at placed
    /// packages, whose own edges are checked separately.
    fn placed_dependencies(&self, root: &'a str, config: &Config) -> Vec<(&'a str, Vec<String>)> {
        let placed = |id: &str| {
            self.packages.get(id).is_some_and(|p| p.is_local() && config.crates.contains_key(&p.name))
        };
        let Some(node) = self.nodes.get(root) else { return Vec::new() };
        let mut direct = Vec::new();
        let mut indirect: BTreeMap<&'a str, Vec<String>> = BTreeMap::new();
        for dep in node.deps.iter().filter(|d| !d.is_dev_only()) {
            let first = dep.pkg.as_str();
            if placed(first) {
                direct.push((first, Vec::new()));
                continue;
            }
            self.search(first, NodeDep::is_dev_only, |id, path| {
                if !placed(id) {
                    return true;
                }
                let via: Vec<String> = path[..path.len() - 1].iter().map(|id| self.name(id)).collect();
                if indirect.get(id).is_none_or(|known| via.len() < known.len()) {
                    indirect.insert(id, via);
                }
                false
            });
        }
        direct.into_iter().chain(indirect).collect()
    }

    /// Each tool-only crate reachable from `root` through normal dependencies, with the
    /// shortest path to it (as package names, `root` first). The search passes through
    /// everything, tools included, but not into a tool-only crate's own subtree.
    fn reachable_tool_only(&self, root: &'a str, tool_only: &BTreeSet<&str>) -> Vec<(String, Vec<String>)> {
        let mut found = Vec::new();
        self.search(
            root,
            |dep| !dep.is_normal(),
            |id, path| {
                let name = self.name(id);
                if tool_only.contains(name.as_str()) {
                    found.push((name, path.iter().map(|id| self.name(id)).collect()));
                    return false;
                }
                true
            },
        );
        found
    }

    /// Each dev-only third-party crate reachable from `root` through normal dependencies without
    /// passing through a dev-only local crate, which apps reach only through their `dev`
    /// feature, with the shortest path to it.
    fn reachable_dev_only(
        &self,
        root: &'a str,
        dev_only: &BTreeSet<&str>,
        config: &Config,
    ) -> Vec<(String, Vec<String>)> {
        let mut found = Vec::new();
        self.search(
            root,
            |dep| !dep.is_normal(),
            |id, path| {
                let name = self.name(id);
                let local = self.packages.get(id).is_some_and(|package| package.is_local());
                if local && config.placement(&name) == Some(Placement::DevOnly) {
                    return false;
                }
                if dev_only.contains(name.as_str()) {
                    found.push((name, path.iter().map(|id| self.name(id)).collect()));
                    return false;
                }
                true
            },
        );
        found
    }

    /// Breadth-first search from `root`, skipping edges `skip` rejects. `visit` sees each
    /// newly reached package with its path from `root` (both ends included) and returns
    /// whether to search past it.
    fn search(
        &self,
        root: &'a str,
        skip: impl Fn(&NodeDep) -> bool,
        mut visit: impl FnMut(&'a str, &[&'a str]) -> bool,
    ) {
        let mut parent: HashMap<&'a str, &'a str> = HashMap::new();
        let mut queue = VecDeque::from([root]);
        while let Some(id) = queue.pop_front() {
            let Some(node) = self.nodes.get(id) else { continue };
            for dep in node.deps.iter().filter(|d| !skip(d)) {
                let dep_id = dep.pkg.as_str();
                if dep_id == root || parent.contains_key(dep_id) {
                    continue;
                }
                parent.insert(dep_id, id);
                let mut path = vec![dep_id];
                let mut cursor = dep_id;
                while let Some(&up) = parent.get(cursor) {
                    path.push(up);
                    cursor = up;
                }
                path.reverse();
                if visit(dep_id, &path) {
                    queue.push_back(dep_id);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use serde_json::{Value, json};

    use super::*;

    #[derive(Clone, Copy, PartialEq)]
    enum Dep {
        Normal,
        Optional,
        Build,
        Dev,
    }
    use Dep::*;

    /// Vendored crates in the fixtures: local, but not workspace members.
    const VENDORED: &[&str] = &["shim", "shim2", "editor_hooks", "ozz-animation-rs"];

    fn id(name: &str) -> String {
        if name.starts_with("sapphire_") || name == "xtask" || VENDORED.contains(&name) {
            format!("path+file:///ws/{name}#0.1.0")
        } else {
            format!("registry+https://github.com/rust-lang/crates.io-index#{name}@1.0.0")
        }
    }

    /// Builds `cargo metadata`-shaped JSON. `members` are workspace crates with their
    /// declared features; names in [`VENDORED`] are local non-members; every other name is
    /// a registry crate.
    fn metadata(members: &[(&str, &[&str])], deps: &[(&str, &str, Dep)]) -> Metadata {
        let mut names: BTreeSet<&str> = members.iter().map(|(name, _)| *name).collect();
        names.extend(deps.iter().flat_map(|(from, to, _)| [*from, *to]));
        let features: HashMap<&str, &[&str]> = members.iter().copied().collect();
        let packages: Vec<Value> = names
            .iter()
            .map(|name| {
                let features: BTreeMap<&str, Vec<&str>> =
                    features.get(name).map_or(&[][..], |f| f).iter().map(|f| (*f, vec![])).collect();
                let declared: Vec<Value> = deps
                    .iter()
                    .filter(|(from, _, _)| from == name)
                    .map(|(_, to, kind)| json!({ "name": to, "kind": kind_json(*kind), "optional": *kind == Optional }))
                    .collect();
                let source = if id(name).starts_with("path+") { Value::Null } else { json!("registry+x") };
                json!({ "id": id(name), "name": name, "source": source, "features": features,
                        "dependencies": declared })
            })
            .collect();
        let nodes: Vec<Value> = names
            .iter()
            .map(|name| {
                let deps: Vec<Value> = deps
                    .iter()
                    .filter(|(from, _, _)| from == name)
                    .map(|(_, to, kind)| {
                        json!({ "name": to, "pkg": id(to), "dep_kinds": [{ "kind": kind_json(*kind), "target": null }] })
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

    fn kind_json(kind: Dep) -> Value {
        match kind {
            Build => json!("build"),
            Dev => json!("dev"),
            Normal | Optional => Value::Null,
        }
    }

    fn config() -> Config {
        Config::parse(
            r#"
            [crates]
            sapphire_core = { kind = "runtime", layer = 0, features = ["mimalloc"] }
            sapphire_math = { kind = "runtime", layer = 0 }
            sapphire_platform = { kind = "runtime", layer = 1, features = ["vulkan"] }
            sapphire_render = { kind = "runtime", layer = 3 }
            sapphire_debug = { kind = "dev_only" }
            sapphire_sandbox = { kind = "app", features = ["dev"] }
            sapphire_pipeline = { kind = "tool" }
            xtask = { kind = "tool" }

            [allow]
            edges = [["sapphire_math", "sapphire_core"]]

            [tool_only_dependencies]
            crates = ["gltf", "toml"]

            [dev_only_dependencies]
            crates = ["egui", "notify"]
            "#,
        )
        .unwrap()
    }

    const MEMBERS: &[(&str, &[&str])] = &[
        ("sapphire_core", &["mimalloc"]),
        ("sapphire_math", &[]),
        ("sapphire_platform", &["vulkan"]),
        ("sapphire_render", &[]),
        ("sapphire_debug", &[]),
        ("sapphire_sandbox", &["dev"]),
        ("sapphire_pipeline", &[]),
        ("xtask", &[]),
    ];

    /// A dependency graph that follows every rule.
    const CLEAN: &[(&str, &str, Dep)] = &[
        ("sapphire_math", "sapphire_core", Normal),
        ("sapphire_platform", "sapphire_core", Normal),
        ("sapphire_platform", "sdl3-sys", Normal),
        ("sdl3-sys", "cmake", Build),
        ("cmake", "toml", Normal), // Only reachable through a build-dependency.
        ("sapphire_render", "sapphire_platform", Normal),
        ("sapphire_render", "sapphire_pipeline", Dev), // Test support may be a tool.
        ("sapphire_core", "gltf", Dev),
        ("sapphire_debug", "sapphire_render", Normal),
        ("sapphire_debug", "egui", Normal),
        ("sapphire_sandbox", "sapphire_render", Normal),
        ("sapphire_sandbox", "sapphire_debug", Optional),
        ("sapphire_pipeline", "sapphire_core", Normal),
        ("sapphire_pipeline", "gltf", Normal),
        ("xtask", "toml", Normal),
    ];

    fn check_with(extra: &[(&str, &str, Dep)]) -> Vec<Violation> {
        let deps: Vec<_> = CLEAN.iter().chain(extra).copied().collect();
        check(&config(), &metadata(MEMBERS, &deps)).unwrap()
    }

    /// The dependency violations as `(from, to, via)` triples.
    fn edges(violations: &[Violation]) -> Vec<(&str, &str, Vec<&str>)> {
        violations
            .iter()
            .filter_map(|v| match v {
                Violation::Dependency { from, to, via, .. } => {
                    Some((from.as_str(), to.as_str(), via.iter().map(String::as_str).collect()))
                }
                _ => None,
            })
            .collect()
    }

    #[test]
    fn shipped_config_parses() {
        let config = Config::parse(include_str!("../layers.toml")).unwrap();
        assert_eq!(config.placement("sapphire_core"), Some(Placement::Runtime { layer: 0 }));
        assert_eq!(config.placement("sapphire_pipeline"), Some(Placement::Tool));
        assert!(config.crates["sapphire_core"].features.contains("mimalloc"));
    }

    #[test]
    fn config_rejects_malformed_placements() {
        let err = Config::parse("[crates]\na = { kind = \"runtime\" }").unwrap_err();
        assert!(err.to_string().contains("runtime crates need a `layer`"), "{err}");
        assert!(Config::parse("[crates]\na = { kind = \"plugin\" }").is_err());
        assert!(Config::parse("[crates]\na = { kind = \"tool\", layer = 2 }").is_err());
        assert!(Config::parse("[crates]\na = { kind = \"dev_only\", layer = 2 }").is_err());
        assert!(Config::parse("[crates]\na = { kind = \"app\", feature = [] }").is_err());
        assert!(Config::parse("[crates]\n[extra]").is_err());
    }

    #[test]
    fn clean_workspace_passes() {
        assert_eq!(check_with(&[]), []);
    }

    #[test]
    fn every_local_package_must_be_placed_and_every_placement_local() {
        let mut config = config();
        config.crates.insert("sapphire_gone".into(), config.crates["sapphire_math"].clone());
        let members = [MEMBERS, &[("sapphire_new", &[][..])]].concat();
        let deps = [CLEAN, &[("sapphire_render", "ozz-animation-rs", Normal)]].concat();
        let violations = check(&config, &metadata(&members, &deps)).unwrap();
        assert_eq!(
            violations,
            [
                Violation::Unplaced { krate: "ozz-animation-rs".into(), member: false },
                Violation::Unplaced { krate: "sapphire_new".into(), member: true },
                Violation::NotLocal { krate: "sapphire_gone".into() },
            ]
        );
        assert!(violations[0].to_string().contains("(a vendored crate)"));
    }

    #[test]
    fn runtime_crates_depend_only_downward() {
        let violations = check_with(&[
            ("sapphire_core", "sapphire_platform", Normal),
            ("sapphire_core", "sapphire_math", Build),
            ("sapphire_core", "sapphire_render", Dev),
        ]);
        assert_eq!(
            violations,
            [
                Violation::Dependency {
                    from: "sapphire_core".into(),
                    from_placement: Placement::Runtime { layer: 0 },
                    to: "sapphire_math".into(),
                    to_placement: Placement::Runtime { layer: 0 },
                    via: vec![],
                },
                Violation::Dependency {
                    from: "sapphire_core".into(),
                    from_placement: Placement::Runtime { layer: 0 },
                    to: "sapphire_platform".into(),
                    to_placement: Placement::Runtime { layer: 1 },
                    via: vec![],
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
            ("sapphire_sandbox", "sapphire_pipeline", Normal),
            ("sapphire_render", "sapphire_pipeline", Build),
            ("xtask", "sapphire_pipeline", Normal),
            ("sapphire_render", "sapphire_sandbox", Normal),
            ("sapphire_pipeline", "sapphire_sandbox", Normal), // Tools may use apps.
        ]);
        assert_eq!(
            edges(&violations),
            [
                ("sapphire_render", "sapphire_pipeline", vec![]),
                ("sapphire_render", "sapphire_sandbox", vec![]),
                ("sapphire_sandbox", "sapphire_pipeline", vec![]),
                ("xtask", "sapphire_pipeline", vec![]),
            ]
        );
        assert!(violations[0].to_string().ends_with("nothing may depend on a tool (R1)"));
    }

    #[test]
    fn dev_only_crates_are_reached_only_through_optional_app_dependencies() {
        let violations = check_with(&[
            ("sapphire_render", "sapphire_debug", Optional),
            ("sapphire_sandbox", "debug-shim", Normal),
            ("debug-shim", "sapphire_debug", Normal),
        ]);
        assert_eq!(
            edges(&violations),
            [
                ("sapphire_render", "sapphire_debug", vec![]),
                ("sapphire_sandbox", "sapphire_debug", vec!["debug-shim"]),
            ]
        );
        assert!(violations[0].to_string().ends_with("only apps may depend on dev-only crates (R8)"));
        assert!(violations[1].to_string().ends_with("through a direct optional dependency (R8)"));

        let mut deps: Vec<_> = CLEAN.iter().copied().filter(|d| d.1 != "sapphire_debug").collect();
        deps.push(("sapphire_sandbox", "sapphire_debug", Normal));
        let violations = check(&config(), &metadata(MEMBERS, &deps)).unwrap();
        assert_eq!(edges(&violations), [("sapphire_sandbox", "sapphire_debug", vec![])]);
    }

    /// The reviewer's chains: a runtime crate reaching a tool and a higher layer through
    /// vendored path crates outside the workspace.
    #[test]
    fn edges_through_unplaced_crates_count() {
        let violations = check_with(&[
            ("sapphire_core", "shim", Normal),
            ("shim", "sapphire_pipeline", Normal),
            ("shim", "shim2", Normal),
            ("shim2", "sapphire_render", Normal),
            ("sapphire_math", "libfoo", Normal),
            ("libfoo", "sapphire_platform", Normal),
        ]);
        assert_eq!(
            edges(&violations),
            [
                ("sapphire_core", "sapphire_pipeline", vec!["shim"]),
                ("sapphire_core", "sapphire_render", vec!["shim", "shim2"]),
                ("sapphire_math", "sapphire_platform", vec!["libfoo"]),
            ]
        );
        assert!(violations.contains(&Violation::Unplaced { krate: "shim".into(), member: false }));
        assert!(violations.contains(&Violation::Unplaced { krate: "shim2".into(), member: false }));
        let render = violations
            .iter()
            .find(|v| matches!(v, Violation::Dependency { to, .. } if to == "sapphire_render"))
            .unwrap();
        assert_eq!(
            render.to_string(),
            "`sapphire_core` (L0) depends on `sapphire_render` (L3) through shim -> shim2: runtime \
             crates may depend only on lower layers (R1)"
        );
    }

    #[test]
    fn tool_only_crates_never_reach_shipping_crates() {
        let violations = check_with(&[
            ("sapphire_platform", "png-helper", Normal),
            ("png-helper", "gltf", Normal),
            ("sapphire_sandbox", "toml", Normal),
        ]);
        let reached: Vec<(&str, &str, Vec<&str>)> = violations
            .iter()
            .filter_map(|v| match v {
                Violation::ToolOnlyReachable { krate, dependency, path } => {
                    Some((krate.as_str(), dependency.as_str(), path.iter().map(String::as_str).collect()))
                }
                _ => None,
            })
            .collect();
        assert_eq!(
            reached,
            [
                (
                    "sapphire_debug",
                    "gltf",
                    vec!["sapphire_debug", "sapphire_render", "sapphire_platform", "png-helper", "gltf"]
                ),
                ("sapphire_platform", "gltf", vec!["sapphire_platform", "png-helper", "gltf"]),
                (
                    "sapphire_render",
                    "gltf",
                    vec!["sapphire_render", "sapphire_platform", "png-helper", "gltf"]
                ),
                (
                    "sapphire_sandbox",
                    "gltf",
                    vec!["sapphire_sandbox", "sapphire_render", "sapphire_platform", "png-helper", "gltf"]
                ),
                ("sapphire_sandbox", "toml", vec!["sapphire_sandbox", "toml"]),
            ]
        );
        assert_eq!(
            violations.iter().find(|v| matches!(v, Violation::ToolOnlyReachable { .. })).unwrap().to_string(),
            "tool-only crate `gltf` is in the dependency tree of `sapphire_debug`: \
             sapphire_debug -> sapphire_render -> sapphire_platform -> png-helper -> gltf"
        );
    }

    #[test]
    fn tool_only_search_continues_through_tools() {
        // The direct dependency on a tool is reported, and so is what the tool drags in.
        let violations = check_with(&[("sapphire_platform", "sapphire_pipeline", Normal)]);
        assert!(violations.contains(&Violation::ToolOnlyReachable {
            krate: "sapphire_platform".into(),
            dependency: "gltf".into(),
            path: vec!["sapphire_platform".into(), "sapphire_pipeline".into(), "gltf".into()],
        }));
        assert_eq!(edges(&violations)[0], ("sapphire_platform", "sapphire_pipeline", vec![]));
    }

    #[test]
    fn dev_only_crates_are_reached_only_through_dev_only_crates() {
        // A runtime crate taking one in, and an app reaching one besides its dev-only crate.
        let violations = check_with(&[
            ("sapphire_render", "file-watch", Normal),
            ("file-watch", "notify", Normal),
            ("sapphire_sandbox", "egui", Optional),
            ("sapphire_core", "notify", Dev),
        ]);
        let reached: Vec<(&str, &str, Vec<&str>)> = violations
            .iter()
            .filter_map(|v| match v {
                Violation::DevOnlyReachable { krate, dependency, path } => {
                    Some((krate.as_str(), dependency.as_str(), path.iter().map(String::as_str).collect()))
                }
                _ => None,
            })
            .collect();
        assert_eq!(
            reached,
            [
                ("sapphire_render", "notify", vec!["sapphire_render", "file-watch", "notify"]),
                ("sapphire_sandbox", "egui", vec!["sapphire_sandbox", "egui"]),
                (
                    "sapphire_sandbox",
                    "notify",
                    vec!["sapphire_sandbox", "sapphire_render", "file-watch", "notify"]
                ),
            ]
        );
        // Through `sapphire_debug` the app reaches egui, which is fine.
        assert!(!check_with(&[]).iter().any(|v| matches!(v, Violation::DevOnlyReachable { .. })));
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
    fn features_must_be_listed() {
        // The reviewer's case: dev features on a runtime crate under other names, and an
        // optional dependency's implicit feature.
        let members = [
            ("sapphire_core", &["mimalloc"][..]),
            ("sapphire_math", &["devtools", "inspector", "cheats", "Dev", "editor_hooks"][..]),
            ("sapphire_platform", &[][..]),
        ];
        let violations = check(&config(), &metadata(&members, &[])).unwrap();
        let features: Vec<(&str, &str)> = violations
            .iter()
            .filter_map(|v| match v {
                Violation::UnlistedFeature { krate, feature } => {
                    (krate == "sapphire_math").then_some(("unlisted", feature.as_str()))
                }
                Violation::MissingFeature { krate, feature } => {
                    (krate == "sapphire_platform").then_some(("missing", feature.as_str()))
                }
                _ => None,
            })
            .collect();
        assert_eq!(
            features,
            [
                ("unlisted", "Dev"),
                ("unlisted", "cheats"),
                ("unlisted", "devtools"),
                ("unlisted", "editor_hooks"),
                ("unlisted", "inspector"),
                ("missing", "vulkan"),
            ]
        );
    }
}
