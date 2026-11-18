//! `sapphire_pipeline check` (M6 C1): spec §3.4's content budgets and the content rules, read from a source
//! tree, as one rule set for the content tests, CI and an artist's tools.
//!
//! # Roles and their rows
//!
//! A mesh's sidecar names the role it is cast in, and so the row of [`ROWS`] it must meet on each cook:
//!
//! ```ron
//! budget: (role: Hero),
//! ```
//!
//! The hero's row is spec §3.4's: 80,000 triangles at LOD0 on desktop and 30,000 on mobile, 200 and 140
//! joints, 6 and 4 material slots after merges, and maps of 2048 and 1024 a side. The others are proposals
//! (spec §3.4 has an enemy's 8,000 triangles and a boss's 20,000 on desktop, and says a phone draws the
//! desktop's LOD1, which the mobile cook keeps by skipping LOD0; nothing about the ambient cast): the
//! per-character rows, joints, slots and maps, are every role's, and an ambient figure, drawn by the crowd,
//! is held to less. A tree (M6 G6a) is no character: its row is proposed for the generated species, a mesh of
//! bark and leaf cards with no skin, its two slots, and a leaf atlas of 1024 a side, 512 on the phones. Its
//! triangles are the same on both cooks, since a tree's levels are off (simplification would merge its
//! cards) until G3's card-thinned levels give the phones one of their own.
//!
//! Each role mesh is cooked to memory on both cooks as a cook of its tree would cook it, merges included
//! ([`cook::cook_mesh_in`]'s way), and measured there: LOD0's triangles, the skin's joints, and the material
//! slots left after its merges. Its maps are every texture the materials its sidecar binds name, sized as
//! [`texture::cooked_size`] plans each cook's top level under its sidecar's cap, with nothing encoded: a
//! generated character map is generated to learn its size. A merge's atlases aren't sized here: the merge
//! refuses one past its own limit when it plans.
//!
//! # The rules
//!
//! | Rule | Severity | What it finds |
//! |---|---|---|
//! | `triangles`, `joints`, `slots`, `map-size` | error | A role mesh over its row on a cook. |
//! | `uncapped-map` | error | A map an anime character material names that stores over 1024 a side on mobile: a map over 1K with no mobile cap. |
//! | `non-ascii-path` | error | A source whose path under the root isn't ASCII: file systems store such names in different forms (macOS decomposes them), so a tree checked out elsewhere can name another file. |
//! | `misplaced-role` | error | A role on a sidecar that isn't a mesh's. |
//! | `unbound-slot` | warning | A role mesh's slot no material document is bound to: it draws the material its importer made. |
//! | `vroid-eye-layer` | warning | A VRM's unbound slot named as VRoid Studio names an eye's part (`EyeExtra_00_EYE`) that [`vroid_eye_layer`] doesn't know: it fell back to drawing no eye layer. |
//! | `vroid-region-light` | warning | A VRM's unbound slot named as VRoid names hair or skin but without its group (`Hair_00_hair`): it fell back to no region light ([`vroid_region_gi`]). |
//! | `duplicate-spring-name` | warning | A VRM whose spring chains share a name: a cloth sheet names chains, so it can't name these. |
//! | `duplicate-texture` | warning | A texture source with the same bytes as another, under its own ID. |
//! | `stale-waiver`, `unexplained-waiver` | error | A waiver nothing needs, or one with no reason. |
//!
//! Warnings never fail a check; errors do, unless waived.
//!
//! # Waivers
//!
//! A source's sidecar may waive a rule for that source, with its reason:
//!
//! ```ron
//! budget: (
//!     role: Hero,
//!     waive: [(rule: Slots, why: "pixiv's sample keeps VRoid's 13 materials as authored: its face, eyes and hair can't merge")],
//! ),
//! ```
//!
//! A waived finding is still reported, with its reason. A waiver that finds nothing to waive is an error, so
//! one outlives its cause only until the next check, and so is one whose `why` is empty. Every cook lists every
//! waiver in its report ([`CookReport::waivers`](crate::cook::CookReport::waivers)); no cook judges a source by
//! these rules or stops on them. The `budget` section is read by the check alone: it's in no cache key, so
//! adding one to a sidecar recooks nothing.

use std::collections::{BTreeMap, HashMap};
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};

use sapphire_assets::AssetId;
use sapphire_assets::material::{AnyMaterialDocument, RegionGi};
use sapphire_assets::mesh::{Mesh, SlotMaterial};
use serde::{Deserialize, Serialize, Serializer};

use crate::character_maps;
use crate::cook::{self, Cook, CookError};
use crate::import::image::{SourceFormat, header_size};
use crate::import::vrm::{self, vroid_eye_layer, vroid_region_gi};
use crate::material;
use crate::meta::{ImportSettings, Resolved, SourceAsset};
use crate::procedural::ProceduralTexture;
use crate::texture::{self, TextureSettings};

/// What a sidecar's `budget` says: see the [module docs](self).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Budget {
    /// The role a mesh is cast in, whose row of [`ROWS`] it must meet.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role: Option<Role>,
    /// The rules this source waives, each with its reason.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub waive: Vec<Waiver>,
}

/// What a character is cast as, which picks its row of spec §3.4 ([`ROWS`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum Role {
    /// A playable character.
    Hero,
    /// A kind of enemy, twelve to a fight on desktop and six on mobile.
    Enemy,
    /// A boss: one to a fight.
    Boss,
    /// A zone's ambient cast, the crowd.
    Ambient,
    /// A tree a foliage field places (M6 G6a): bark and leaf cards, no skin.
    Tree,
}

impl Role {
    /// Every role, in [`ROWS`]' order.
    pub const ALL: [Self; 5] = [Self::Hero, Self::Enemy, Self::Boss, Self::Ambient, Self::Tree];

    /// The role's name in reports.
    pub fn name(self) -> &'static str {
        match self {
            Self::Hero => "hero",
            Self::Enemy => "enemy",
            Self::Boss => "boss",
            Self::Ambient => "ambient",
            Self::Tree => "tree",
        }
    }

    /// The role [`Self::name`] calls `name`, as the command line names it (`new-character --role enemy`).
    pub fn named(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|role| role.name() == name)
    }

    /// The role's row on `cook`.
    pub fn limits(self, cook: Cook) -> Limits {
        let [desktop, mobile] = ROWS[self as usize];
        match cook {
            Cook::Desktop => desktop,
            Cook::Mobile => mobile,
        }
    }
}

/// What a role allows on one cook.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct Limits {
    /// Triangles at LOD0, as the cook stores it (the mobile cook's LOD0 is the first level it keeps).
    pub triangles: u32,
    /// Joints in the skin, secondary (spring) joints included.
    pub joints: u32,
    /// Material slots, after the cook's merges.
    pub slots: u32,
    /// The longest side any of its maps stores.
    pub map: u32,
}

const fn limits(triangles: u32, joints: u32, slots: u32, map: u32) -> Limits {
    Limits { triangles, joints, slots, map }
}

/// Each role's row, in [`Role::ALL`]'s order, desktop then mobile: spec §3.4's hero; the enemy's and the
/// boss's desktop triangles are its proposals, and their mobile ones the same, since spec §3.4's phone draws
/// the desktop's LOD1 (a level the mobile cook keeps by skipping LOD0, so under the same number); the ambient
/// row is proposed here (M6 C1), and the tree's (M6 G6a), for the maintainer.
pub const ROWS: [[Limits; 2]; 5] = [
    [limits(80_000, 200, 6, 2048), limits(30_000, 140, 4, 1024)],
    [limits(8_000, 200, 6, 2048), limits(8_000, 140, 4, 1024)],
    [limits(20_000, 200, 6, 2048), limits(20_000, 140, 4, 1024)],
    [limits(4_000, 64, 2, 1024), limits(4_000, 64, 1, 512)],
    [limits(12_000, 0, 2, 1024), limits(12_000, 0, 2, 512)],
];

/// The longest side a character's map may store on mobile, whatever its role (spec §3.4's part budget):
/// past it, `uncapped-map`.
pub const MOBILE_MAP: u32 = 1024;

/// One waived rule, with its reason.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Waiver {
    pub rule: Rule,
    /// Why the source may break the rule: a sentence a reviewer can judge.
    pub why: String,
}

/// A rule of the check: see the table in the [module docs](self).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum Rule {
    Triangles,
    Joints,
    Slots,
    MapSize,
    UncappedMap,
    NonAsciiPath,
    MisplacedRole,
    UnboundSlot,
    VroidEyeLayer,
    VroidRegionLight,
    DuplicateSpringName,
    DuplicateTexture,
    StaleWaiver,
    UnexplainedWaiver,
}

impl Rule {
    /// The rule's name in reports.
    pub fn name(self) -> &'static str {
        match self {
            Self::Triangles => "triangles",
            Self::Joints => "joints",
            Self::Slots => "slots",
            Self::MapSize => "map-size",
            Self::UncappedMap => "uncapped-map",
            Self::NonAsciiPath => "non-ascii-path",
            Self::MisplacedRole => "misplaced-role",
            Self::UnboundSlot => "unbound-slot",
            Self::VroidEyeLayer => "vroid-eye-layer",
            Self::VroidRegionLight => "vroid-region-light",
            Self::DuplicateSpringName => "duplicate-spring-name",
            Self::DuplicateTexture => "duplicate-texture",
            Self::StaleWaiver => "stale-waiver",
            Self::UnexplainedWaiver => "unexplained-waiver",
        }
    }

    pub fn severity(self) -> Severity {
        match self {
            Self::UnboundSlot
            | Self::VroidEyeLayer
            | Self::VroidRegionLight
            | Self::DuplicateSpringName
            | Self::DuplicateTexture => Severity::Warning,
            Self::Triangles
            | Self::Joints
            | Self::Slots
            | Self::MapSize
            | Self::UncappedMap
            | Self::NonAsciiPath
            | Self::MisplacedRole
            | Self::StaleWaiver
            | Self::UnexplainedWaiver => Severity::Error,
        }
    }
}

impl fmt::Display for Rule {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

/// Whether an unwaived finding fails the check.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    Error,
    Warning,
}

/// A waiver a cooked source's sidecar holds, as a cook lists it ([`CookReport::waivers`](crate::cook::CookReport::waivers)).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Waived {
    pub source: PathBuf,
    pub rule: Rule,
    pub why: String,
}

/// Every waiver `assets`' sidecars hold, in their order.
pub fn waivers(assets: &[SourceAsset]) -> Vec<Waived> {
    assets
        .iter()
        .flat_map(|asset| {
            let waive = asset.meta.budget.as_ref().map_or(&[][..], |budget| &budget.waive);
            waive.iter().map(|waiver| Waived {
                source: asset.path.clone(),
                rule: waiver.rule,
                why: waiver.why.clone(),
            })
        })
        .collect()
}

/// What a check found: see the [module docs](self).
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Report {
    /// Each mesh judged at its role, by source.
    pub meshes: Vec<Judged>,
    /// How many maps anime character materials name, each held to `uncapped-map`.
    pub character_maps: u32,
    /// Every finding, waived ones too, by source, rule and cook.
    pub findings: Vec<Finding>,
}

impl Report {
    /// The findings that fail the check: errors no waiver covers.
    pub fn failures(&self) -> impl Iterator<Item = &Finding> {
        self.findings.iter().filter(|finding| finding.fails())
    }

    /// Whether the check passes: no [`Self::failures`].
    pub fn passes(&self) -> bool {
        self.failures().next().is_none()
    }

    /// The findings of `rule` about `source` (a path under the root, `/` between its parts).
    pub fn of<'a>(&'a self, source: &'a str, rule: Rule) -> impl Iterator<Item = &'a Finding> {
        self.findings.iter().filter(move |finding| finding.source == source && finding.rule == rule)
    }
}

/// A mesh measured against its role's row.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Judged {
    pub source: String,
    #[serde(serialize_with = "role_name")]
    pub role: Role,
    /// Desktop, then mobile.
    pub cooks: Vec<Measured>,
}

/// A role mesh on one cook, with its row.
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub struct Measured {
    #[serde(serialize_with = "cook_name")]
    pub cook: Cook,
    pub triangles: u32,
    pub joints: u32,
    pub slots: u32,
    /// The longest side of its largest map (0 with none).
    pub map: u32,
    pub limits: Limits,
}

/// One thing a rule found.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Finding {
    /// The source it's about, under the root, with `/` between its parts: the one whose sidecar may waive it.
    pub source: String,
    #[serde(serialize_with = "rule_name")]
    pub rule: Rule,
    pub severity: Severity,
    /// The cook it was found on, for the rules that depend on one.
    #[serde(serialize_with = "optional_cook_name")]
    pub cook: Option<Cook>,
    pub message: String,
    /// The waiver's reason, if the source's sidecar waives the rule.
    pub waived: Option<String>,
}

impl Finding {
    /// Whether it fails the check: an error with no waiver.
    pub fn fails(&self) -> bool {
        self.severity == Severity::Error && self.waived.is_none()
    }
}

fn role_name<S: Serializer>(role: &Role, serializer: S) -> Result<S::Ok, S::Error> {
    serializer.serialize_str(role.name())
}

fn rule_name<S: Serializer>(rule: &Rule, serializer: S) -> Result<S::Ok, S::Error> {
    serializer.serialize_str(rule.name())
}

fn cook_name<S: Serializer>(cook: &Cook, serializer: S) -> Result<S::Ok, S::Error> {
    serializer.serialize_str(cook.name())
}

fn optional_cook_name<S: Serializer>(cook: &Option<Cook>, serializer: S) -> Result<S::Ok, S::Error> {
    match cook {
        Some(cook) => serializer.serialize_some(cook.name()),
        None => serializer.serialize_none(),
    }
}

/// A check that couldn't be made: its tree doesn't resolve, or a source it reads doesn't read or cook.
#[derive(Debug, thiserror::Error)]
pub enum CheckError {
    #[error("checking {}", path.display())]
    Cook {
        path: PathBuf,
        #[source]
        source: Box<CookError>,
    },
    #[error("checking {}: failed to read it", path.display())]
    Read {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("checking {}: {why}", path.display())]
    Invalid { path: PathBuf, why: String },
}

/// Checks the source tree under `root`, as a locked cook resolves it, by every rule: see the
/// [module docs](self). It reads the tree's files alone, and writes and caches nothing.
pub fn check(root: &Path) -> Result<Report, CheckError> {
    let resolved = cook::resolve_tree(root)
        .map_err(|source| CheckError::Cook { path: root.to_owned(), source: Box::new(source) })?;
    check_resolved(root, &resolved)
}

/// [`check()`], of the tree under `root` as `resolved` already says it resolves: what a cook's asset index
/// records ([`crate::index`]).
pub fn check_resolved(root: &Path, resolved: &Resolved) -> Result<Report, CheckError> {
    let assets: Vec<&SourceAsset> = resolved.assets.iter().filter(|asset| asset.meta.cook).collect();
    let by_id: HashMap<AssetId, &SourceAsset> = assets.iter().map(|asset| (asset.meta.id, *asset)).collect();
    let mut check = Checker { root, by_id: &by_id, maps: HashMap::new(), findings: Vec::new() };

    for asset in &assets {
        check.path(asset);
    }
    let character_maps = check.character_maps(&assets)?;
    let mut meshes = Vec::new();
    for asset in &assets {
        let role = asset.meta.budget.as_ref().and_then(|budget| budget.role);
        let Some(settings) = asset.meta.importer.mesh() else {
            if role.is_some() {
                let message = "a role is a mesh's, and this source isn't one: it has no triangles or joints to \
                               judge";
                check.find(asset, Rule::MisplacedRole, None, message.to_owned());
            }
            continue;
        };
        let vrm = match &asset.meta.importer {
            ImportSettings::Vrm(settings) => Some(settings.version),
            _ => None,
        };
        if role.is_none() && vrm.is_none() {
            continue;
        }
        let desktop = check.cook(asset, Cook::Desktop)?;
        if let Some(role) = role {
            let mobile = check.cook(asset, Cook::Mobile)?;
            meshes.push(check.judge(asset, role, &settings.materials, [&desktop, &mobile])?);
            check.unbound_slots(asset, &desktop);
        }
        if let Some(version) = vrm {
            check.vroid_names(asset, &desktop);
            check.spring_names(asset, version)?;
        }
    }
    check.duplicate_textures(&assets)?;
    let Checker { mut findings, .. } = check;
    waive(&assets, root, &mut findings);
    findings.sort_by(|a, b| {
        (&a.source, a.rule, a.cook.map(|cook| cook as u8)).cmp(&(
            &b.source,
            b.rule,
            b.cook.map(|cook| cook as u8),
        ))
    });
    meshes.sort_by(|a, b| a.source.cmp(&b.source));
    Ok(Report { meshes, character_maps, findings })
}

/// A source's path under `root`, with `/` between its parts on every host.
fn relative(path: &Path, root: &Path) -> String {
    let relative = path.strip_prefix(root).unwrap_or(path);
    relative.components().map(|part| part.as_os_str().to_string_lossy()).collect::<Vec<_>>().join("/")
}

/// A map's size, before any cook's cap, and the settings it cooks with.
#[derive(Clone, Copy)]
struct MapShape {
    width: u32,
    height: u32,
    settings: TextureSettings,
}

impl MapShape {
    /// The size `cook` stores, as the texture cooker plans it.
    fn cooked(&self, cook: Cook, path: &Path) -> Result<(u32, u32), CheckError> {
        texture::cooked_size(&self.settings, cook, self.width, self.height)
            .map_err(|error| CheckError::Invalid { path: path.to_owned(), why: error.to_string() })
    }
}

/// A check under way: the tree's cooked sources by ID, each map's shape once read, and what's been found.
struct Checker<'a> {
    root: &'a Path,
    by_id: &'a HashMap<AssetId, &'a SourceAsset>,
    maps: HashMap<AssetId, Option<MapShape>>,
    findings: Vec<Finding>,
}

impl Checker<'_> {
    fn name(&self, path: &Path) -> String {
        relative(path, self.root)
    }

    fn find(&mut self, asset: &SourceAsset, rule: Rule, cook: Option<Cook>, message: String) {
        let source = self.name(&asset.path);
        self.findings.push(Finding { source, rule, severity: rule.severity(), cook, message, waived: None });
    }

    /// `non-ascii-path`.
    fn path(&mut self, asset: &SourceAsset) {
        let name = self.name(&asset.path);
        if let Some(part) = name.split('/').find(|part| !part.is_ascii()) {
            self.find(
                asset,
                Rule::NonAsciiPath,
                None,
                format!(
                    "\"{part}\" isn't ASCII: file systems store such names in different forms, so a checkout \
                     elsewhere can spell another file; rename it in ASCII"
                ),
            );
        }
    }

    /// The shape of the map `id`, or `None` for an ID that isn't a map source of the tree (which its
    /// material's cook refuses).
    fn shape(&mut self, id: AssetId) -> Result<Option<MapShape>, CheckError> {
        if let Some(shape) = self.maps.get(&id) {
            return Ok(*shape);
        }
        let shape = match self.by_id.get(&id) {
            Some(source) => map_shape(source)?,
            None => None,
        };
        self.maps.insert(id, shape);
        Ok(shape)
    }

    /// `uncapped-map`, over every map an anime character material names; returns how many there are.
    fn character_maps(&mut self, assets: &[&SourceAsset]) -> Result<u32, CheckError> {
        let mut maps: Vec<AssetId> = Vec::new();
        for asset in assets.iter().filter(|asset| matches!(asset.meta.importer, ImportSettings::Material(_)))
        {
            let document = read_material(asset)?;
            if let AnyMaterialDocument::AnimeCharacter(_) = document {
                for id in document.textures().into_iter().flatten() {
                    if !maps.contains(&id) {
                        maps.push(id);
                    }
                }
            }
        }
        let mut count = 0;
        for id in maps {
            let Some(shape) = self.shape(id)? else { continue };
            count += 1;
            let map = self.by_id[&id];
            let (width, height) = shape.cooked(Cook::Mobile, &map.path)?;
            if width.max(height) > MOBILE_MAP {
                self.find(
                    map,
                    Rule::UncappedMap,
                    Some(Cook::Mobile),
                    format!(
                        "a character's map, it stores {width} × {height} on mobile, over {MOBILE_MAP} a side: \
                         cap it in its sidecar (`max_size: (desktop: None, mobile: Some({MOBILE_MAP}))`)"
                    ),
                );
            }
        }
        Ok(count)
    }

    /// `asset`'s mesh as `cook` would cook it in this tree.
    fn cook(&self, asset: &SourceAsset, cook: Cook) -> Result<Mesh, CheckError> {
        let by_id = self.by_id;
        cook::cook_mesh_of(asset, &|id| by_id.get(&id).copied(), cook)
            .map_err(|source| CheckError::Cook { path: asset.path.clone(), source: Box::new(source) })
    }

    /// `triangles`, `joints`, `slots` and `map-size`: the mesh against its role's row on each cook.
    fn judge(
        &mut self,
        asset: &SourceAsset,
        role: Role,
        bindings: &BTreeMap<String, AssetId>,
        meshes: [&Mesh; 2],
    ) -> Result<Judged, CheckError> {
        // Every map the bound materials name, each once, in binding order.
        let mut maps: Vec<AssetId> = Vec::new();
        for id in bindings.values() {
            let Some(material) = self.by_id.get(id) else { continue };
            if !matches!(material.meta.importer, ImportSettings::Material(_)) {
                continue;
            }
            for map in read_material(material)?.textures().into_iter().flatten() {
                if !maps.contains(&map) {
                    maps.push(map);
                }
            }
        }
        let mut cooks = Vec::with_capacity(2);
        for (cook, mesh) in Cook::ALL.into_iter().zip(meshes) {
            let limits = role.limits(cook);
            let role_name = role.name();
            let triangles: u32 = mesh
                .lods
                .first()
                .map_or(0, |lod| lod.submeshes.iter().map(|submesh| submesh.index_count / 3).sum());
            let joints = mesh.skin.as_ref().map_or(0, |skin| skin.joint_count);
            let slots = mesh.materials.len() as u32;
            let mut largest = 0;
            for &id in &maps {
                let Some(shape) = self.shape(id)? else { continue };
                let path = &self.by_id[&id].path;
                let (width, height) = shape.cooked(cook, path)?;
                largest = largest.max(width.max(height));
                if width.max(height) > limits.map {
                    let map = self.name(path);
                    self.find(
                        asset,
                        Rule::MapSize,
                        Some(cook),
                        format!(
                            "{map} stores {width} × {height}, over the {role_name} row's {} a side",
                            limits.map
                        ),
                    );
                }
            }
            for (rule, found, limit, what) in [
                (Rule::Triangles, triangles, limits.triangles, "triangles at LOD0"),
                (Rule::Joints, joints, limits.joints, "joints"),
                (Rule::Slots, slots, limits.slots, "material slots after merges"),
            ] {
                if found > limit {
                    self.find(
                        asset,
                        rule,
                        Some(cook),
                        format!("{found} {what}, over the {role_name} row's {limit}"),
                    );
                }
            }
            cooks.push(Measured { cook, triangles, joints, slots, map: largest, limits });
        }
        Ok(Judged { source: self.name(&asset.path), role, cooks })
    }

    /// `unbound-slot`, on a role mesh's desktop cook.
    fn unbound_slots(&mut self, asset: &SourceAsset, mesh: &Mesh) {
        for slot in &mesh.materials {
            if let SlotMaterial::Embedded(_) = slot.material {
                self.find(
                    asset,
                    Rule::UnboundSlot,
                    None,
                    format!(
                        "slot \"{}\" draws the material its importer made: bind a material document to it in \
                         the sidecar's `materials`",
                        slot.name
                    ),
                );
            }
        }
    }

    /// `vroid-eye-layer` and `vroid-region-light`, over a VRM's unbound slots: a bound one draws its document.
    fn vroid_names(&mut self, asset: &SourceAsset, mesh: &Mesh) {
        for slot in &mesh.materials {
            if !matches!(slot.material, SlotMaterial::Embedded(_)) {
                continue;
            }
            let name = slot.name.as_str();
            let parts: Vec<String> =
                name.split(|c: char| c == '_' || c.is_whitespace()).map(str::to_ascii_lowercase).collect();
            // VRoid Studio numbers a part (`<Part>_<nn>_<GROUP>`): a name without a number isn't its.
            let numbered =
                parts.iter().any(|part| !part.is_empty() && part.bytes().all(|b| b.is_ascii_digit()));
            if !numbered {
                continue;
            }
            let mentions =
                |words: &[&str]| parts.iter().any(|part| words.iter().any(|word| part.contains(word)));
            if mentions(&["eye", "brow", "lash"]) && vroid_eye_layer(name).is_none() {
                self.find(
                    asset,
                    Rule::VroidEyeLayer,
                    None,
                    format!(
                        "slot \"{name}\" is named as VRoid names an eye's part, but not one the importer knows \
                         (EyeWhite, EyeIris, EyeHighlight, FaceEyeline, FaceEyelash, FaceBrow), so it draws no \
                         eye layer: rename it, or bind a material document"
                    ),
                );
            }
            let RegionGi { sphere, skin } = vroid_region_gi(name);
            if mentions(&["hair", "skin"]) && sphere == 0.0 && skin == 0.0 {
                self.find(
                    asset,
                    Rule::VroidRegionLight,
                    None,
                    format!(
                        "slot \"{name}\" is named as VRoid names hair or skin, but not with its group (`_HAIR`, \
                         `_SKIN`), so it takes no region light: rename it, or bind a material document"
                    ),
                );
            }
        }
    }

    /// `duplicate-spring-name`, over a VRM's rig.
    fn spring_names(
        &mut self,
        asset: &SourceAsset,
        version: Option<vrm::VrmVersion>,
    ) -> Result<(), CheckError> {
        let path = &asset.path;
        let bytes = fs::read(path).map_err(|source| CheckError::Read { path: path.clone(), source })?;
        let rig = vrm::import_rig(&bytes, version)
            .map_err(|error| CheckError::Invalid { path: path.clone(), why: error.to_string() })?;
        let mut chains: BTreeMap<&str, u32> = BTreeMap::new();
        for spring in &rig.springs {
            *chains.entry(spring.name.as_str()).or_default() += 1;
        }
        for (name, count) in chains.into_iter().filter(|&(_, count)| count > 1) {
            self.find(
                asset,
                Rule::DuplicateSpringName,
                None,
                format!(
                    "{count} spring chains are named \"{name}\": a cloth sheet names its chains, so it can't \
                     name these; give each its own name"
                ),
            );
        }
        Ok(())
    }

    /// `duplicate-texture`: each texture source with the same bytes as one earlier in path order.
    fn duplicate_textures(&mut self, assets: &[&SourceAsset]) -> Result<(), CheckError> {
        let mut first: HashMap<blake3::Hash, PathBuf> = HashMap::new();
        for asset in assets.iter().filter(|asset| matches!(asset.meta.importer, ImportSettings::Texture(_))) {
            let path = &asset.path;
            let bytes = fs::read(path).map_err(|source| CheckError::Read { path: path.clone(), source })?;
            match first.get(&blake3::hash(&bytes)) {
                Some(original) => {
                    let original = self.name(original);
                    self.find(
                        asset,
                        Rule::DuplicateTexture,
                        None,
                        format!(
                            "the same bytes as {original}, under an ID of its own: bind that one, or waive this \
                             with the reason the copy is needed"
                        ),
                    );
                }
                None => {
                    first.insert(blake3::hash(&bytes), path.clone());
                }
            }
        }
        Ok(())
    }
}

fn read_material(asset: &SourceAsset) -> Result<AnyMaterialDocument, CheckError> {
    let path = &asset.path;
    let bytes = fs::read(path).map_err(|source| CheckError::Read { path: path.clone(), source })?;
    material::read(&bytes).map_err(|error| CheckError::Invalid { path: path.clone(), why: error.to_string() })
}

/// The shape of the map `source` cooks to, or `None` for a source that isn't a map.
fn map_shape(source: &SourceAsset) -> Result<Option<MapShape>, CheckError> {
    let path = &source.path;
    let invalid = |why: String| CheckError::Invalid { path: path.clone(), why };
    let read = || fs::read(path).map_err(|error| CheckError::Read { path: path.clone(), source: error });
    Ok(match &source.meta.importer {
        ImportSettings::Texture(settings) => {
            let format = SourceFormat::from_path(path).ok_or_else(|| invalid("not an image".to_owned()))?;
            let (width, height) =
                header_size(&read()?, format).ok_or_else(|| invalid("its header doesn't read".to_owned()))?;
            Some(MapShape { width, height, settings: *settings })
        }
        ImportSettings::ProceduralTexture(settings) => {
            let document = ProceduralTexture::read(&read()?).map_err(|error| invalid(error.to_string()))?;
            Some(MapShape { width: document.size, height: document.size, settings: settings.texture() })
        }
        // A generated map is the size of what it's generated from.
        ImportSettings::CharacterMap(settings, max_size) => {
            let map = character_maps::generate(path, &read()?, settings)
                .map_err(|error| invalid(error.to_string()))?;
            let settings = TextureSettings { class: map.class, max_size: *max_size };
            Some(MapShape { width: map.image.width, height: map.image.height, settings })
        }
        _ => None,
    })
}

/// Marks each finding its source's sidecar waives with the waiver's reason, and adds `stale-waiver` for each
/// waiver that waived nothing and `unexplained-waiver` for each with no reason.
fn waive(assets: &[&SourceAsset], root: &Path, findings: &mut Vec<Finding>) {
    let mut added = Vec::new();
    for asset in assets {
        let Some(budget) = &asset.meta.budget else { continue };
        let source = relative(&asset.path, root);
        for waiver in &budget.waive {
            let mut used = false;
            for finding in
                findings.iter_mut().filter(|finding| finding.source == source && finding.rule == waiver.rule)
            {
                finding.waived = Some(waiver.why.clone());
                used = true;
            }
            let mut add = |rule: Rule, message: String| {
                added.push(Finding {
                    source: source.clone(),
                    rule,
                    severity: rule.severity(),
                    cook: None,
                    message,
                    waived: None,
                });
            };
            if !used {
                add(
                    Rule::StaleWaiver,
                    format!("waives `{}`, which finds nothing here: remove the waiver", waiver.rule),
                );
            }
            if waiver.why.trim().is_empty() {
                add(
                    Rule::UnexplainedWaiver,
                    format!("waives `{}` with no reason: say why in `why`", waiver.rule),
                );
            }
        }
    }
    findings.extend(added);
}

/// The text `sapphire_pipeline check` prints: each mesh at its role, each finding, then a summary.
impl fmt::Display for Report {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for mesh in &self.meshes {
            writeln!(f, "{} at the {} row:", mesh.source, mesh.role.name())?;
            for measured in &mesh.cooks {
                let Measured { cook, triangles, joints, slots, map, limits } = measured;
                writeln!(
                    f,
                    "  {cook:<7} {triangles} / {} triangles, {joints} / {} joints, {slots} / {} slots, maps {map} / {}",
                    limits.triangles, limits.joints, limits.slots, limits.map
                )?;
            }
        }
        for finding in &self.findings {
            let level = match (finding.waived.is_some(), finding.severity) {
                (true, _) => "waived",
                (false, Severity::Error) => "error",
                (false, Severity::Warning) => "warning",
            };
            let cook = finding.cook.map(|cook| format!(" ({cook})")).unwrap_or_default();
            writeln!(f, "{level}[{}] {}{cook}: {}", finding.rule, finding.source, finding.message)?;
            if let Some(why) = &finding.waived {
                writeln!(f, "  waived: {why}")?;
            }
        }
        let count =
            |level: fn(&Finding) -> bool| self.findings.iter().filter(|finding| level(finding)).count();
        write!(
            f,
            "check: {} mesh(es) at their roles, {} character map(s); {} error(s), {} warning(s), {} waived",
            self.meshes.len(),
            self.character_maps,
            count(Finding::fails),
            count(|finding| finding.severity == Severity::Warning && finding.waived.is_none()),
            count(|finding| finding.waived.is_some()),
        )
    }
}

#[cfg(test)]
mod tests;
