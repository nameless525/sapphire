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
//! cards) until G3's card-thinned levels give the phones one of their own. Ground cover (M6 G6b), a bush, a
//! fern, flowers, a grass clump or a rock, is held to a smaller row of the same shape: two slots, maps of 512
//! a side and 256 on the phones.
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
//! | `uncapped-map` | error | A map an anime character material names that stores over 1024 a side on mobile: a map over 1K with no mobile cap. The mobile cook alone: on desktop, only a role mesh's maps are held, by `map-size`. |
//! | `non-ascii-path` | error | A source whose path under the root isn't ASCII: file systems store such names in different forms (macOS decomposes them), so a tree checked out elsewhere can name another file. |
//! | `misplaced-role` | error | A role on a sidecar that isn't a mesh's. |
//! | `unbound-slot` | warning | A role mesh's slot no material document is bound to: it draws the material its importer made. |
//! | `vroid-eye-layer` | warning | A VRM's unbound slot named as VRoid Studio names an eye's part (`EyeExtra_00_EYE`) that [`vroid_eye_layer`] doesn't know: it fell back to drawing no eye layer. |
//! | `vroid-region-light` | warning | A VRM's unbound slot named as VRoid names hair or skin but without its group (`Hair_00_hair`): it fell back to no region light ([`vroid_region_gi`]). |
//! | `duplicate-spring-name` | warning | A VRM whose spring chains share a name: a cloth sheet names chains, so it can't name these. |
//! | `duplicate-texture` | warning | A texture source with the same bytes as another, under its own ID. |
//! | `unmarked-export` | warning | A glTF source that authors outline normals (`_OUTLINE_NORMAL`) with no mark of the Sapphire Blender add-on ([`gltf::addon_export`]): if Blender's exporter wrote it without the add-on, or the add-on before 0.3.1, its outline normals on a turned or scaled rig may be in another space than its normals, so re-export it with the add-on. Also a glTF whose JSON doesn't read, so its mark can't be (the cook refuses it with the reason). Only the JSON is read, a GLB's JSON chunk without its buffers ([`gltf::read_json`]). |
//! | `sheet` | error | An effect sheet whose document names a wrong field ([`EffectSheetDocument::problem`]) or doesn't read, whose painting is missing or isn't its grid's size, an effects library that doesn't read (so its sheets can't be checked), or one whose emitter names an ID that's no sheet of the tree (M6 N4a). |
//! | `atlas-budget` | error | An effects library whose sheets' frames take more than its atlas holds on a cook: [`ATLAS_BUDGET_DESKTOP`] or [`ATLAS_BUDGET_MOBILE`], each layer at its whole chain. |
//! | `stale-waiver`, `unexplained-waiver` | error | A waiver nothing needs, or one with no reason. |
//! | `foliage-document` | error | A foliage document (M6 G1a) that doesn't read, a field of it that's wrong, named, or a kind's model that isn't a static mesh source of the tree. |
//! | `zone-document` | error | A zone document that doesn't read, a field of it that's wrong, named (an atmosphere volume's shape, blend or strength among them), or an atmosphere volume's look (M6 V1) that's missing, doesn't read as a day or keys `grade`. |
//! | `cue-group` | error | A cue whose `group` names a voice group no mix of the tree has (M6 U3's backfill): it would play limited by no group. A cue that doesn't read is the cook's to refuse. |
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
//! A waiver covers every cook unless it names one, so a rule broken on the phones alone is waived there alone,
//! and the same rule broken on desktop still fails:
//!
//! ```ron
//! waive: [(rule: Joints, cook: Some(Mobile), why: "…")],
//! ```
//!
//! A waived finding is still reported, with its reason. A waiver that finds nothing to waive on its cooks is an
//! error, so one outlives its cause only until the next check, and so is one whose `why` is empty. Every cook
//! lists the waivers that cover it in its report ([`CookReport::waivers`](crate::cook::CookReport::waivers)); no
//! cook judges a source by these rules or stops on them. The `budget` section is read by the check alone: it's
//! in no cook key, so adding one to a sidecar recooks nothing (the asset index, which records the check, keys
//! by every sidecar's bytes: [`crate::index`]).
//!
//! **The sidecar's version:** a waiver's `cook` is written only when set, so every sidecar written before it
//! reads and writes the same bytes, and `AssetMeta` stays at version 1, as it did when `budget` came; a sidecar
//! whose waiver names a cook doesn't read in a pipeline from before it, which refuses the field by name.

use std::collections::{BTreeMap, HashMap};
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};

use sapphire_assets::AssetId;
use sapphire_assets::effect_sheet::{ATLAS_BUDGET_DESKTOP, ATLAS_BUDGET_MOBILE, LAYER_SIZE, atlas_bytes};
use sapphire_assets::material::{AnyMaterialDocument, RegionGi};
use sapphire_assets::mesh::{Mesh, SlotMaterial};
use serde::{Deserialize, Serialize, Serializer};

use crate::audio;
use crate::cook::{self, Cook, CookError};
use crate::{audit, character_maps, procedural};
use crate::effect_sheet::EffectSheetDocument;
use crate::effects;
use crate::foliage;
use crate::import::gltf;
use crate::import::image::{SourceFormat, header_size};
use crate::import::vrm::{self, vroid_eye_layer, vroid_region_gi};
use crate::material;
use crate::meta::{ImportSettings, Resolved, SourceAsset};
use crate::texture::{self, TextureSettings};
use crate::zone;

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
    /// Ground cover a foliage field places (M6 G6b): a bush, a fern, flowers, a grass clump or a rock.
    Cover,
}

impl Role {
    /// Every role, in [`ROWS`]' order.
    pub const ALL: [Self; 6] = [Self::Hero, Self::Enemy, Self::Boss, Self::Ambient, Self::Tree, Self::Cover];

    /// The role's name in reports.
    pub fn name(self) -> &'static str {
        match self {
            Self::Hero => "hero",
            Self::Enemy => "enemy",
            Self::Boss => "boss",
            Self::Ambient => "ambient",
            Self::Tree => "tree",
            Self::Cover => "cover",
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
/// row is proposed here (M6 C1), the tree's (M6 G6a) and ground cover's (M6 G6b), for the maintainer.
pub const ROWS: [[Limits; 2]; 6] = [
    [limits(80_000, 200, 6, 2048), limits(30_000, 140, 4, 1024)],
    [limits(8_000, 200, 6, 2048), limits(8_000, 140, 4, 1024)],
    [limits(20_000, 200, 6, 2048), limits(20_000, 140, 4, 1024)],
    [limits(4_000, 64, 2, 1024), limits(4_000, 64, 1, 512)],
    [limits(12_000, 0, 2, 1024), limits(12_000, 0, 2, 512)],
    [limits(2_000, 0, 2, 512), limits(2_000, 0, 2, 256)],
];

/// The longest side a character's map may store on mobile, whatever its role (spec §3.4's part budget):
/// past it, `uncapped-map`.
pub const MOBILE_MAP: u32 = 1024;

/// One waived rule, with its reason.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Waiver {
    pub rule: Rule,
    /// The cook it waives the rule on: every cook when `None`. Written only when set.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cook: Option<Cook>,
    /// Why the source may break the rule: a sentence a reviewer can judge.
    pub why: String,
}

impl Waiver {
    /// Whether it covers a finding on `cook` (`None` for a rule that depends on no cook): one that names a
    /// cook covers that cook's findings alone.
    pub fn covers(&self, cook: Option<Cook>) -> bool {
        self.cook.is_none_or(|waived| cook == Some(waived))
    }
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
    UnmarkedExport,
    Sheet,
    AtlasBudget,
    StaleWaiver,
    UnexplainedWaiver,
    FoliageDocument,
    ZoneDocument,
    CueGroup,
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
            Self::UnmarkedExport => "unmarked-export",
            Self::Sheet => "sheet",
            Self::AtlasBudget => "atlas-budget",
            Self::StaleWaiver => "stale-waiver",
            Self::UnexplainedWaiver => "unexplained-waiver",
            Self::FoliageDocument => "foliage-document",
            Self::ZoneDocument => "zone-document",
            Self::CueGroup => "cue-group",
        }
    }

    pub fn severity(self) -> Severity {
        match self {
            Self::UnboundSlot
            | Self::VroidEyeLayer
            | Self::VroidRegionLight
            | Self::DuplicateSpringName
            | Self::DuplicateTexture
            | Self::UnmarkedExport => Severity::Warning,
            Self::Triangles
            | Self::Joints
            | Self::Slots
            | Self::MapSize
            | Self::UncappedMap
            | Self::NonAsciiPath
            | Self::MisplacedRole
            | Self::Sheet
            | Self::AtlasBudget
            | Self::StaleWaiver
            | Self::UnexplainedWaiver
            | Self::FoliageDocument
            | Self::ZoneDocument
            | Self::CueGroup => Severity::Error,
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

/// A waiver a cooked source's sidecar holds that covers a cook, as that cook lists it
/// ([`CookReport::waivers`](crate::cook::CookReport::waivers)).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Waived {
    pub source: PathBuf,
    pub rule: Rule,
    pub why: String,
}

/// Every waiver `assets`' sidecars hold that covers `cook`, in their order.
pub fn waivers(assets: &[SourceAsset], cook: Cook) -> Vec<Waived> {
    assets
        .iter()
        .flat_map(|asset| {
            let waive = asset.meta.budget.as_ref().map_or(&[][..], |budget| &budget.waive);
            waive.iter().filter(|waiver| waiver.covers(Some(cook))).map(|waiver| Waived {
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
        if matches!(asset.meta.importer, ImportSettings::Gltf(_)) {
            check.addon_mark(asset);
        }
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
    check.sheets(&assets)?;
    check.foliage_documents(&assets)?;
    check.zone_documents(&assets)?;
    check.cue_groups(&assets)?;
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

/// The size a map stores on each cook, under its sidecar's cap, as the texture cooker plans it: desktop, then
/// mobile ([`Cook::ALL`]'s order).
#[derive(Clone, Copy)]
struct MapShape([(u32, u32); 2]);

impl MapShape {
    /// Each cook's size, as `plan` gives it.
    fn planned<E: fmt::Display>(plan: impl Fn(Cook) -> Result<(u32, u32), E>) -> Result<Self, String> {
        let size = |cook| plan(cook).map_err(|error| error.to_string());
        Ok(Self([size(Cook::Desktop)?, size(Cook::Mobile)?]))
    }

    /// The size `cook` stores.
    fn cooked(&self, cook: Cook) -> (u32, u32) {
        self.0[cook as usize]
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
        audit::relative(path, self.root)
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
            let (width, height) = shape.cooked(Cook::Mobile);
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

    /// `sheet` and `atlas-budget`: each sheet's document, by field, and its painting's size; then each effects
    /// library's sheets, which must be the tree's, against its atlas's budget on each cook.
    fn sheets(&mut self, assets: &[&SourceAsset]) -> Result<(), CheckError> {
        let mut frames: HashMap<AssetId, u32> = HashMap::new();
        for asset in
            assets.iter().filter(|asset| matches!(asset.meta.importer, ImportSettings::EffectSheet(_)))
        {
            let document = match EffectSheetDocument::read(&read(asset)?) {
                Ok(document) => document,
                Err(error) => {
                    self.find(asset, Rule::Sheet, None, error.to_string());
                    continue;
                }
            };
            let image = document.image_path(&asset.path);
            let size =
                fs::read(&image).ok().and_then(|bytes| header_size(&bytes, SourceFormat::from_path(&image)?));
            let grid = (document.columns * LAYER_SIZE, document.rows * LAYER_SIZE);
            match size {
                None => {
                    let message = format!("`image`: its painting, {}, doesn't read as a PNG", document.image);
                    self.find(asset, Rule::Sheet, None, message);
                }
                Some(size) if size != grid => {
                    let message = format!(
                        "`image`: its painting is {} × {}, and a grid of {} × {} frames of {LAYER_SIZE} needs {} × {}",
                        size.0, size.1, document.columns, document.rows, grid.0, grid.1
                    );
                    self.find(asset, Rule::Sheet, None, message);
                }
                Some(_) => {
                    frames.insert(asset.meta.id, document.frames);
                }
            }
        }
        for asset in assets.iter().filter(|asset| matches!(asset.meta.importer, ImportSettings::Effects(_))) {
            let library = match effects::read(&read(asset)?) {
                Ok(library) => library,
                Err(error) => {
                    let message =
                        format!("the library doesn't read, so its sheets can't be checked: {error}");
                    self.find(asset, Rule::Sheet, None, message);
                    continue;
                }
            };
            let mut layers = 0;
            for effects::NamedSheet { emitter, field, sheet: id } in effects::named_sheets(&library) {
                let sheet = self
                    .by_id
                    .get(&id)
                    .filter(|sheet| matches!(sheet.meta.importer, ImportSettings::EffectSheet(_)));
                match sheet {
                    None => {
                        let message = format!(
                            "`{field}`: `{emitter}` names {id} as its sheet, and it's no sheet of the tree"
                        );
                        self.find(asset, Rule::Sheet, None, message);
                    }
                    Some(_) => layers += u64::from(frames.get(&id).copied().unwrap_or_default()),
                }
            }
            for (cook, budget) in [(Cook::Desktop, ATLAS_BUDGET_DESKTOP), (Cook::Mobile, ATLAS_BUDGET_MOBILE)]
            {
                let bytes = atlas_bytes(layers);
                if bytes > budget {
                    let message = format!(
                        "its sheets' {layers} frames take {:.1} MiB of atlas at {LAYER_SIZE}² with their chains, \
                         over the {} cook's {} MiB: name fewer sheets or frames, or split the library",
                        bytes as f64 / f64::from(1 << 20),
                        cook.name(),
                        budget >> 20
                    );
                    self.find(asset, Rule::AtlasBudget, Some(cook), message);
                }
            }
        }
        Ok(())
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
                let (width, height) = shape.cooked(cook);
                largest = largest.max(width.max(height));
                if width.max(height) > limits.map {
                    let map = self.name(&self.by_id[&id].path);
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

    /// `unmarked-export`, on a glTF source, from its JSON document alone (a GLB's JSON chunk, not its
    /// buffers). A document that doesn't read is a finding of its own, by path: the cook refuses the file
    /// with the reason.
    fn addon_mark(&mut self, asset: &SourceAsset) {
        let export = gltf::read_json(&asset.path).and_then(|json| gltf::addon_export(&json));
        let message = match export {
            Ok(export) if export.outline_normals && export.version.is_none() => {
                "it authors `_OUTLINE_NORMAL` but carries no mark of the Sapphire Blender add-on (0.3.1 or later). \
                 If Blender's exporter wrote it without the add-on, or with the add-on before 0.3.1, its outline \
                 normals on a turned or scaled rig may be in another space than its normals: re-export it with \
                 the add-on"
                    .to_owned()
            }
            Ok(_) => return,
            Err(error) => format!("its mark can't be read: {}", crate::refusal::chain(&error).replace('\n', ": ")),
        };
        self.find(asset, Rule::UnmarkedExport, None, message);
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

    /// `foliage-document`: each foliage document read and checked as a cook would, and each kind's model held to a
    /// static mesh source of the tree. A cook refuses the same; this says so before one, by the field.
    fn foliage_documents(&mut self, assets: &[&SourceAsset]) -> Result<(), CheckError> {
        for asset in assets.iter().filter(|asset| matches!(asset.meta.importer, ImportSettings::Foliage(_))) {
            let path = &asset.path;
            let bytes = fs::read(path).map_err(|source| CheckError::Read { path: path.clone(), source })?;
            let document = match foliage::read(&bytes) {
                Ok(document) => document,
                Err(error) => {
                    self.find(asset, Rule::FoliageDocument, None, error.to_string());
                    continue;
                }
            };
            for (index, kind) in document.kinds.iter().enumerate() {
                let id = kind.model.mesh();
                // The cook's own test, so a skinned model is refused here as it is there.
                let is_static = match self.by_id.get(&id) {
                    Some(source) => cook::is_static_mesh_source(source).map_err(|source| {
                        CheckError::Cook { path: path.clone(), source: Box::new(source) }
                    })?,
                    None => false,
                };
                if !is_static {
                    let message = format!(
                        "`kinds[{index}].model`: {id} isn't a static mesh source of this content tree"
                    );
                    self.find(asset, Rule::FoliageDocument, None, message);
                }
            }
        }
        Ok(())
    }

    /// `zone-document`: each zone document read and checked as a cook would, and each look its atmosphere volumes
    /// name (M6 V1) read from its directory as a day. A cook refuses the same; this says so before one, by the
    /// field.
    fn zone_documents(&mut self, assets: &[&SourceAsset]) -> Result<(), CheckError> {
        for asset in assets.iter().filter(|asset| matches!(asset.meta.importer, ImportSettings::Zone(_))) {
            let path = &asset.path;
            let bytes = fs::read(path).map_err(|source| CheckError::Read { path: path.clone(), source })?;
            let read = std::str::from_utf8(&bytes).map_err(|error| error.to_string()).and_then(|text| {
                sapphire_zone::ZoneDocument::from_ron(text).map_err(|error| error.to_string())
            });
            let looks = read.and_then(|document| {
                zone::volume_looks(path, &document).map(drop).map_err(|error| error.to_string())
            });
            if let Err(message) = looks {
                self.find(asset, Rule::ZoneDocument, None, message);
            }
        }
        Ok(())
    }

    /// `cue-group`: each cue's voice group held to the groups the tree's mixes name, since a group no mix has limits
    /// nothing (the mixer counts such a start, `AudioStats::groups_unknown`). A cue or a mix that doesn't read is
    /// the cook's to refuse, by its field.
    fn cue_groups(&mut self, assets: &[&SourceAsset]) -> Result<(), CheckError> {
        let mut groups: Vec<String> = Vec::new();
        for asset in assets.iter().filter(|asset| matches!(asset.meta.importer, ImportSettings::Mix(_))) {
            if let Ok(mix) = audio::read_mix(&read(asset)?) {
                groups.extend(mix.groups.into_iter().map(|group| group.name));
            }
        }
        for asset in assets.iter().filter(|asset| matches!(asset.meta.importer, ImportSettings::Cue(_))) {
            let Ok(cue) = audio::read_cue(&read(asset)?) else { continue };
            if let Some(group) = cue.group.filter(|group| !groups.contains(group)) {
                let known = if groups.is_empty() { "none".to_owned() } else { groups.join(", ") };
                let message = format!(
                    "`group`: {group:?} is no voice group of a mix in this tree (they're {known}), so it would \
                     limit nothing"
                );
                self.find(asset, Rule::CueGroup, None, message);
            }
        }
        Ok(())
    }
}

/// A source's bytes.
fn read(asset: &SourceAsset) -> Result<Vec<u8>, CheckError> {
    fs::read(&asset.path).map_err(|source| CheckError::Read { path: asset.path.clone(), source })
}

fn read_material(asset: &SourceAsset) -> Result<AnyMaterialDocument, CheckError> {
    let path = &asset.path;
    let bytes = read(asset)?;
    material::read(&bytes).map_err(|error| CheckError::Invalid { path: path.clone(), why: error.to_string() })
}

/// The shape of the map `source` cooks to, or `None` for a source that isn't a map.
fn map_shape(source: &SourceAsset) -> Result<Option<MapShape>, CheckError> {
    let path = &source.path;
    let invalid = |why: String| CheckError::Invalid { path: path.clone(), why };
    let read = || fs::read(path).map_err(|error| CheckError::Read { path: path.clone(), source: error });
    let shape = match &source.meta.importer {
        ImportSettings::Texture(settings) => {
            let format = SourceFormat::from_path(path).ok_or_else(|| invalid("not an image".to_owned()))?;
            let (width, height) =
                header_size(&read()?, format).ok_or_else(|| invalid("its header doesn't read".to_owned()))?;
            MapShape::planned(|cook| texture::cooked_size(settings, cook, width, height))
        }
        ImportSettings::ProceduralTexture(settings) => {
            let bytes = read()?;
            MapShape::planned(|cook| procedural::cooked_size(&bytes, settings, cook))
        }
        // A generated map is the size of what it's generated from.
        ImportSettings::CharacterMap(settings, max_size) => {
            let map = character_maps::generate(path, &read()?, settings)
                .map_err(|error| invalid(error.to_string()))?;
            let settings = TextureSettings { class: map.class, max_size: *max_size };
            let (width, height) = (map.image.width, map.image.height);
            MapShape::planned(|cook| texture::cooked_size(&settings, cook, width, height))
        }
        _ => return Ok(None),
    };
    shape.map(Some).map_err(invalid)
}

/// Marks each finding its source's sidecar waives on the finding's cook with the waiver's reason, and adds
/// `stale-waiver` for each waiver that waived nothing on its cooks and `unexplained-waiver` for each with no
/// reason.
fn waive(assets: &[&SourceAsset], root: &Path, findings: &mut Vec<Finding>) {
    let mut added = Vec::new();
    for asset in assets {
        let Some(budget) = &asset.meta.budget else { continue };
        let source = audit::relative(&asset.path, root);
        for waiver in &budget.waive {
            let mut used = false;
            let mut on_no_cook = false;
            for finding in
                findings.iter_mut().filter(|finding| finding.source == source && finding.rule == waiver.rule)
            {
                if waiver.covers(finding.cook) {
                    finding.waived = Some(waiver.why.clone());
                    used = true;
                }
                on_no_cook |= finding.cook.is_none();
            }
            let mut add = |rule: Rule, message: String| {
                added.push(Finding {
                    source: source.clone(),
                    rule,
                    severity: rule.severity(),
                    cook: waiver.cook,
                    message,
                    waived: None,
                });
            };
            if !used {
                let rule = waiver.rule;
                let message = match waiver.cook {
                    None => format!("waives `{rule}`, which finds nothing here: remove the waiver"),
                    Some(cook) if on_no_cook => format!(
                        "waives `{rule}` on the {cook} cook, and `{rule}` finds this on no cook in particular: \
                         remove the waiver's `cook`"
                    ),
                    Some(cook) => {
                        format!("waives `{rule}` on the {cook} cook, which finds nothing there: remove the waiver")
                    }
                };
                add(Rule::StaleWaiver, message);
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
