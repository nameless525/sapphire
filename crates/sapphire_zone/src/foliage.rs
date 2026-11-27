//! Foliage as a field (M6 G1a, m6-plan §3 "Foliage as a field"): the `.foliage.ron` document that says what
//! grows where, the cooked kinds (`.sfoliage`) and the per-cell scatter sets (`.sscatter`) a cook places from it.
//!
//! ```ron
//! (
//!     type: "sapphire::zone::FoliageDocument",
//!     version: 1,
//!     data: (
//!         // What grows: a species the tree generator grows, at one age, or a ground-cover model, each a
//!         // mesh by its sidecar's ID.
//!         kinds: [
//!             (name: "oak", model: Tree("2b0f0d43-6d4e-4b59-9b1f-5f3c6f0b8e21")),
//!             (name: "fern", model: Cover("8f5a7c1e-0c3d-4e1b-a2f6-3d9e4b7c5a10")),
//!         ],
//!         // Where it may grow: named outlines on the ground, in metres in X and Z.
//!         layers: [(name: "wood", outlines: [[(-40.0, -40.0), (40.0, -40.0), (40.0, 40.0), (-40.0, 40.0)]])],
//!         // How it grows, in order: a rule may keep away from what an earlier one placed.
//!         rules: [
//!             (name: "oaks", kind: "oak", on: ["wood"], per_100m2: 1.5, spacing: 4.0,
//!              slope_degrees: (0.0, 30.0), cluster: Some((scale: 24.0, contrast: 0.6)), scale: (0.8, 1.3)),
//!             (name: "ferns", kind: "fern", on: ["wood"], per_100m2: 20.0, spacing: 1.0,
//!              avoid: [Kinds(kinds: ["oak"], distance: 1.5), Area(position: (0.0, 0.0, 0.0), shape: Sphere(radius: 6.0))]),
//!         ],
//!         // Grass: dense clumps by the square metre, after every rule.
//!         grass: [(name: "floor", kind: "fern", on: ["wood"], per_m2: 2.0, avoid: [Kinds(kinds: ["oak"], distance: 0.6)])],
//!     ),
//! )
//! ```
//!
//! A zone names one by its `foliage` field, and its cook places it into a scatter set for each cell its layers
//! reach, in the zone's presentation half: nothing here is the simulation's, so no digest moves with it (trunk
//! colliders are G1b's). Plants stand on the zone's static colliders, so a zone with foliage names `colliders`.
//!
//! # Placing
//!
//! [`place_rules`] and [`place_grass`] are pure functions of the document, the ground and a region, on the
//! deterministic maths ([`place`]'s docs have how): every candidate is keyed by the lattice square of world
//! space it comes from, so a cell, a chunk, a thread or a cut of the grid at another size can't move a plant
//! or change its ID. A cook calls them for each cell ([`scatter_set`]); an editor's preview and the run time's
//! grass rings (W15a) can call them on any region and get the same plants there.
//!
//! # Stable IDs
//!
//! Each instance has a 64-bit ID, a hash of its rule's name and its lattice square: an edit elsewhere in the
//! document, a reseed, or another rule's addition leaves it alone, so a stroke (G7) can erase a plant by its ID
//! and a collider (G1b) can name the trunk it stands for. Renaming a rule is a new rule. IDs are unique within
//! a set: one rule's are a bijection of its lattice squares (SplitMix64's finalizer is one), and two rules'
//! meet by a 2⁻⁶⁴ chance a pair, which the cook refuses rather than write. 32 bits would not do: a dense 64 m
//! cell of grass would expect about half a match. The IDs sit beside the instances, never uploaded, so the
//! instance stays 16 bytes.
//!
//! # The instance
//!
//! [`ScatterInstance`] is 16 bytes, what the GPU reads per plant (G2a): its root's position as three `f32`s in
//! the zone's space, at full precision so a root sits on the ground it was cast to, and one `u32` packing yaw
//! (8 bits, 1.4° steps, more than a plant shows), scale (8 bits, a geometric step of 0.54% from 0.5 to 2),
//! tint (8 bits, chosen now for G4's palettes) and flags (8 bits, zero, for G and W's later levels and cuts).
//! The ID stays beside it in the set, never uploaded: what draws needs no identity, and what edits needs no
//! GPU.

pub mod place;
#[cfg(test)]
mod tests;

use std::collections::{BTreeMap, BTreeSet};

use bevy_reflect::{Reflect, TypePath};
use sapphire_core::Name;
use sapphire_reflect::Versioned;
use sapphire_serialize::AssetId;
use sapphire_serialize::cooked::AlignedVec;
use serde::{Deserialize, Serialize};

pub use place::{Ground, GroundPoint, Placed, Region, grass_reach, place_grass, place_rules, scatter_set};

use crate::document::{ZoneError, check_finite, check_name, check_names, check_position};
use crate::grid::{CellCoord, Grid, MAX_COORDINATE};
use crate::shape::ZoneShape;

/// Conventional file extension for a cooked foliage document's kinds.
pub const FOLIAGE_EXTENSION: &str = "sfoliage";
/// Conventional file extension for a cell's cooked scatter set.
pub const SCATTER_EXTENSION: &str = "sscatter";

/// The most kinds a document may hold.
pub const MAX_KINDS: usize = 256;
/// The most layers, rules or grass rules a document may hold, each.
pub const MAX_RULES: usize = 256;
/// The most points an outline may hold.
pub const MAX_OUTLINE_POINTS: usize = 4096;
/// The most outlines a layer may hold.
pub const MAX_OUTLINES: usize = 256;
/// The closest a rule's plants may stand, in metres: below it a rule is grass.
pub const MIN_SPACING: f32 = 0.25;
/// The densest grass may grow, in clumps a square metre.
pub const MAX_GRASS_PER_M2: f32 = 64.0;
/// The farthest a plant may keep away from another kind, in metres.
pub const MAX_AVOID_DISTANCE: f32 = 64.0;
/// The most cells a zone's foliage may reach.
pub const MAX_CELLS: usize = 1 << 16;
/// The most instances a cell's set may hold: 4 MiB of them.
pub const MAX_SET_INSTANCES: usize = 1 << 18;
/// A chunk's side, in metres: the grain a renderer culls a set's instances by (G2a), in world space so a
/// cell of any size holds whole chunks.
pub const CHUNK_SIZE: f32 = 16.0;
/// The smallest scale an instance can hold.
pub const MIN_SCALE: f32 = 0.5;
/// The largest scale an instance can hold.
pub const MAX_SCALE: f32 = 2.0;

/// A foliage document (see the [module docs](self)).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, Reflect)]
#[type_path = "sapphire::zone"]
#[serde(deny_unknown_fields)]
pub struct FoliageDocument {
    pub kinds: Vec<FoliageKind>,
    #[serde(default)]
    pub layers: Vec<FoliageLayer>,
    /// Placed in order, so a rule may keep away from what an earlier rule placed.
    #[serde(default)]
    pub rules: Vec<FoliageRule>,
    /// Placed after every rule.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub grass: Vec<GrassRule>,
}

impl Versioned for FoliageDocument {
    /// 1: M6 G1a.
    const VERSION: u32 = 1;
}

/// One kind of plant: what a rule places and a renderer draws, its index in [`FoliageDocument::kinds`] being
/// what a scatter set names.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Reflect)]
#[type_path = "sapphire::zone"]
#[serde(deny_unknown_fields)]
pub struct FoliageKind {
    /// Unique among the document's kinds: its identity.
    pub name: String,
    pub model: FoliageModel,
}

/// What a kind draws, by a mesh's sidecar ID. The same plain value in the document and the cooked kinds.
#[derive(
    Clone,
    Copy,
    Debug,
    PartialEq,
    Eq,
    Serialize,
    Deserialize,
    Reflect,
    rkyv::Archive,
    rkyv::Serialize,
    rkyv::Deserialize,
)]
#[type_path = "sapphire::zone"]
#[rkyv(derive(Debug))]
pub enum FoliageModel {
    /// A species the tree generator grows, at one age (`sapphire_pipeline::tree`): what G1b gives a trunk and
    /// W16 an impostor.
    Tree(AssetId),
    /// Ground cover: a bush, a fern, flowers, a grass clump, rocks.
    Cover(AssetId),
}

impl FoliageModel {
    /// Its mesh.
    #[must_use]
    pub fn mesh(self) -> AssetId {
        match self {
            Self::Tree(mesh) | Self::Cover(mesh) => mesh,
        }
    }
}

impl ArchivedFoliageModel {
    #[must_use]
    pub fn to_native(&self) -> FoliageModel {
        match self {
            Self::Tree(mesh) => FoliageModel::Tree(*mesh),
            Self::Cover(mesh) => FoliageModel::Cover(*mesh),
        }
    }
}

/// Ground a rule may place on: outlines in X and Z, each closed back to its first point, a point inside an odd
/// number of them inside the layer (so one outline inside another is a hole). Terrain's material layers (W12a)
/// will be layers too.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Reflect)]
#[type_path = "sapphire::zone"]
#[serde(deny_unknown_fields)]
pub struct FoliageLayer {
    /// Unique among the document's layers: what rules name it by.
    pub name: String,
    pub outlines: Vec<Vec<[f32; 2]>>,
}

/// Sparse plants by rule: trees, bushes, rocks, flowers (see [`place`]).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Reflect)]
#[type_path = "sapphire::zone"]
#[serde(deny_unknown_fields)]
pub struct FoliageRule {
    /// Unique among the document's rules and grass: what its plants' IDs hash, so renaming it is a new rule.
    pub name: String,
    /// One of the document's kinds, by name.
    pub kind: String,
    /// The layers it places on, by name: at least one.
    pub on: Vec<String>,
    /// Plants a hundred square metres, on average, before clustering: at most `100 / (4 · spacing²)`.
    pub per_100m2: f32,
    /// The closest two of its plants stand, in metres, from [`MIN_SPACING`].
    pub spacing: f32,
    /// The ground's slope it grows on, from and to, in degrees from level: 0 to 90 by default.
    #[serde(default = "any_slope", skip_serializing_if = "is_any_slope")]
    pub slope_degrees: [f32; 2],
    /// The ground's height it grows between, in metres: anywhere when left out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub height: Option<[f32; 2]>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cluster: Option<Cluster>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub avoid: Vec<Avoid>,
    /// Its plants' scales, from and to, within [`MIN_SCALE`] and [`MAX_SCALE`]: 1 by default.
    #[serde(default = "unit_scale", skip_serializing_if = "is_unit_scale")]
    pub scale: [f32; 2],
    /// Moves where its plants stand, never their IDs.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub seed: u32,
}

/// Grass: dense clumps by the square metre, with no spacing, placed after every rule (see [`place`]).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Reflect)]
#[type_path = "sapphire::zone"]
#[serde(deny_unknown_fields)]
pub struct GrassRule {
    /// Unique among the document's rules and grass.
    pub name: String,
    /// One of the document's kinds, by name.
    pub kind: String,
    /// The layers it grows on, by name: at least one.
    pub on: Vec<String>,
    /// Clumps a square metre, before clustering, up to [`MAX_GRASS_PER_M2`].
    pub per_m2: f32,
    #[serde(default = "any_slope", skip_serializing_if = "is_any_slope")]
    pub slope_degrees: [f32; 2],
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub height: Option<[f32; 2]>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cluster: Option<Cluster>,
    /// What it keeps away from: kinds the rules placed, and areas.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub avoid: Vec<Avoid>,
    #[serde(default = "unit_scale", skip_serializing_if = "is_unit_scale")]
    pub scale: [f32; 2],
    #[serde(default, skip_serializing_if = "is_zero")]
    pub seed: u32,
}

/// Patches: a value noise of `scale` metres that thins and thickens a rule's plants, by `contrast` from 0 (even
/// everywhere) to 1 (bare between patches), its average density the rule's.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, Reflect)]
#[type_path = "sapphire::zone"]
#[serde(deny_unknown_fields)]
pub struct Cluster {
    pub scale: f32,
    pub contrast: f32,
}

/// What a rule's plants keep away from.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Reflect)]
#[type_path = "sapphire::zone"]
#[serde(deny_unknown_fields)]
pub enum Avoid {
    /// Plants of these kinds an earlier rule placed, by more than `distance` metres across the ground: the
    /// ferns that keep off an oak's trunk. Grass may name any kind a rule places.
    Kinds { kinds: Vec<String>, distance: f32 },
    /// A clearing: no root inside the shape about `position`.
    Area { position: [f32; 3], shape: ZoneShape },
}

fn any_slope() -> [f32; 2] {
    [0.0, 90.0]
}

#[allow(clippy::trivially_copy_pass_by_ref, reason = "serde's `skip_serializing_if` passes a reference")]
fn is_any_slope(value: &[f32; 2]) -> bool {
    *value == any_slope()
}

fn unit_scale() -> [f32; 2] {
    [1.0, 1.0]
}

#[allow(clippy::trivially_copy_pass_by_ref, reason = "serde's `skip_serializing_if` passes a reference")]
fn is_unit_scale(value: &[f32; 2]) -> bool {
    *value == unit_scale()
}

#[allow(clippy::trivially_copy_pass_by_ref, reason = "serde's `skip_serializing_if` passes a reference")]
fn is_zero(value: &u32) -> bool {
    *value == 0
}

/// What every rule and every grass rule share, as the checks and the placement read them.
pub(crate) struct RuleView<'a> {
    pub name: &'a str,
    pub kind: &'a str,
    pub on: &'a [String],
    pub slope_degrees: [f32; 2],
    pub height: Option<[f32; 2]>,
    pub cluster: Option<Cluster>,
    pub avoid: &'a [Avoid],
    pub scale: [f32; 2],
    pub seed: u32,
}

impl FoliageRule {
    pub(crate) fn view(&self) -> RuleView<'_> {
        RuleView {
            name: &self.name,
            kind: &self.kind,
            on: &self.on,
            slope_degrees: self.slope_degrees,
            height: self.height,
            cluster: self.cluster,
            avoid: &self.avoid,
            scale: self.scale,
            seed: self.seed,
        }
    }
}

impl GrassRule {
    pub(crate) fn view(&self) -> RuleView<'_> {
        RuleView {
            name: &self.name,
            kind: &self.kind,
            on: &self.on,
            slope_degrees: self.slope_degrees,
            height: self.height,
            cluster: self.cluster,
            avoid: &self.avoid,
            scale: self.scale,
            seed: self.seed,
        }
    }
}

impl FoliageDocument {
    /// Reads a document from RON text and checks it.
    pub fn from_ron(text: &str) -> Result<Self, ZoneError> {
        let document: Self = sapphire_serialize::from_ron(text).map_err(Box::new)?;
        document.check()?;
        Ok(document)
    }

    /// The index of the kind named `name`.
    #[must_use]
    pub fn kind_index(&self, name: &str) -> Option<usize> {
        self.kinds.iter().position(|kind| kind.name == name)
    }

    /// Checks everything that needs nothing outside the document, naming the field. Whether each kind's mesh
    /// is a mesh is the cook's to say.
    pub fn check(&self) -> Result<(), ZoneError> {
        for (list, count, most) in [
            ("kinds", self.kinds.len(), MAX_KINDS),
            ("layers", self.layers.len(), MAX_RULES),
            ("rules", self.rules.len(), MAX_RULES),
            ("grass", self.grass.len(), MAX_RULES),
        ] {
            if count > most {
                return Err(ZoneError::invalid(list, format!("{count} is more than the {most} it may hold")));
            }
        }
        check_names("kinds", self.kinds.iter().map(|kind| kind.name.as_str()))?;
        check_names("layers", self.layers.iter().map(|layer| layer.name.as_str()))?;
        for (index, layer) in self.layers.iter().enumerate() {
            check_layer(layer).map_err(|(field, problem)| {
                ZoneError::invalid(format!("layers[{index}].outlines{field}"), problem)
            })?;
        }
        check_names("rules", self.rules.iter().map(|rule| rule.name.as_str()))?;
        check_names("grass", self.grass.iter().map(|rule| rule.name.as_str()))?;
        // Rules and grass share one space of names, as their plants' IDs do.
        if let Some(index) =
            self.grass.iter().position(|grass| self.rules.iter().any(|rule| rule.name == grass.name))
        {
            let problem =
                format!("`{}` is a rule's name too, and names are identities", self.grass[index].name);
            return Err(ZoneError::invalid(format!("grass[{index}].name"), problem));
        }
        for (index, rule) in self.rules.iter().enumerate() {
            let field = |name: &str| format!("rules[{index}].{name}");
            let earlier: BTreeSet<&str> = self.rules[..index].iter().map(|rule| rule.kind.as_str()).collect();
            self.check_rule(&rule.view(), &earlier)
                .map_err(|(name, problem)| ZoneError::invalid(field(&name), problem))?;
            check_positive_in(rule.spacing, MIN_SPACING, MAX_COORDINATE)
                .map_err(|problem| ZoneError::invalid(field("spacing"), problem))?;
            let most = place::most_per_100m2(rule.spacing);
            if !(rule.per_100m2.is_finite() && rule.per_100m2 > 0.0 && rule.per_100m2 <= most) {
                let problem = format!(
                    "{} isn't above 0 and at most the {most} a spacing of {} m allows",
                    rule.per_100m2, rule.spacing
                );
                return Err(ZoneError::invalid(field("per_100m2"), problem));
            }
        }
        let placed: BTreeSet<&str> = self.rules.iter().map(|rule| rule.kind.as_str()).collect();
        for (index, grass) in self.grass.iter().enumerate() {
            let field = |name: &str| format!("grass[{index}].{name}");
            self.check_rule(&grass.view(), &placed)
                .map_err(|(name, problem)| ZoneError::invalid(field(&name), problem))?;
            check_positive_in(grass.per_m2, f32::MIN_POSITIVE, MAX_GRASS_PER_M2)
                .map_err(|problem| ZoneError::invalid(field("per_m2"), problem))?;
        }
        Ok(())
    }

    /// What's wrong with what a rule and grass share, with its field: `earlier` is the kinds it may avoid.
    fn check_rule(&self, rule: &RuleView<'_>, earlier: &BTreeSet<&str>) -> Result<(), (String, String)> {
        let at = |name: &'static str| move |problem: String| (name.to_owned(), problem);
        if self.kind_index(rule.kind).is_none() {
            return Err(("kind".to_owned(), format!("`{}` isn't one of the document's kinds", rule.kind)));
        }
        if rule.on.is_empty() {
            return Err(("on".to_owned(), "it names no layer, so it places nothing".to_owned()));
        }
        for (index, layer) in rule.on.iter().enumerate() {
            if !self.layers.iter().any(|known| known.name == *layer) {
                return Err((
                    format!("on[{index}]"),
                    format!("`{layer}` isn't one of the document's layers"),
                ));
            }
        }
        let [low, high] = rule.slope_degrees;
        if !(low.is_finite() && high.is_finite() && 0.0 <= low && low <= high && high <= 90.0) {
            return Err((
                "slope_degrees".to_owned(),
                format!("({low}, {high}) doesn't rise from 0 to at most 90"),
            ));
        }
        if let Some([low, high]) = rule.height {
            check_finite(low).and_then(|()| check_finite(high)).map_err(at("height"))?;
            if !(low <= high && low.abs() <= MAX_COORDINATE && high.abs() <= MAX_COORDINATE) {
                return Err((
                    "height".to_owned(),
                    format!("({low}, {high}) doesn't rise within {MAX_COORDINATE} m"),
                ));
            }
        }
        if let Some(cluster) = rule.cluster {
            check_positive_in(cluster.scale, 1.0, MAX_COORDINATE).map_err(at("cluster.scale"))?;
            if !(cluster.contrast.is_finite() && (0.0..=1.0).contains(&cluster.contrast)) {
                return Err((
                    "cluster.contrast".to_owned(),
                    format!("{} isn't from 0 to 1", cluster.contrast),
                ));
            }
        }
        for (index, avoid) in rule.avoid.iter().enumerate() {
            let field = |name: &str| format!("avoid[{index}].{name}");
            match avoid {
                Avoid::Kinds { kinds, distance } => {
                    if kinds.is_empty() {
                        return Err((field("kinds"), "it names no kind".to_owned()));
                    }
                    for (which, kind) in kinds.iter().enumerate() {
                        if !earlier.contains(kind.as_str()) {
                            let problem = format!("`{kind}` is placed by no rule before this one");
                            return Err((field(&format!("kinds[{which}]")), problem));
                        }
                    }
                    check_positive_in(*distance, f32::MIN_POSITIVE, MAX_AVOID_DISTANCE)
                        .map_err(|problem| (field("distance"), problem))?;
                }
                Avoid::Area { position, shape } => {
                    check_position(*position).map_err(|problem| (field("position"), problem))?;
                    shape.check().map_err(|problem| (field("shape"), problem))?;
                }
            }
        }
        let [low, high] = rule.scale;
        if !(low.is_finite() && high.is_finite() && MIN_SCALE <= low && low <= high && high <= MAX_SCALE) {
            let problem = format!("({low}, {high}) doesn't rise within {MIN_SCALE} to {MAX_SCALE}");
            return Err(("scale".to_owned(), problem));
        }
        Ok(())
    }

    /// The cells a zone on `grid` gives a scatter set: each whose square meets the bounds of a layer a rule or
    /// grass places on, by ascending `[x, z]`. Some may hold no plant. The document must be [checked](Self::check).
    pub fn cells(&self, grid: Grid) -> Result<Vec<CellCoord>, ZoneError> {
        let used: BTreeSet<&str> = (self.rules.iter().flat_map(|rule| &rule.on))
            .chain(self.grass.iter().flat_map(|grass| &grass.on))
            .map(String::as_str)
            .collect();
        let mut cells = BTreeSet::new();
        for layer in self.layers.iter().filter(|layer| used.contains(layer.name.as_str())) {
            for outline in &layer.outlines {
                let [mut min, mut max] = [outline[0], outline[0]];
                for point in outline {
                    min = [min[0].min(point[0]), min[1].min(point[1])];
                    max = [max[0].max(point[0]), max[1].max(point[1])];
                }
                let low = grid.cell_of([min[0], 0.0, min[1]]);
                let high = grid.cell_of([max[0], 0.0, max[1]]);
                let span = (i64::from(high.x - low.x) + 1) * (i64::from(high.z - low.z) + 1);
                if span > MAX_CELLS as i64 {
                    let problem = format!("its outlines reach more than the {MAX_CELLS} cells foliage may");
                    return Err(ZoneError::invalid(format!("layers `{}`", layer.name), problem));
                }
                for x in low.x..=high.x {
                    for z in low.z..=high.z {
                        cells.insert(CellCoord { x, z });
                    }
                }
                if cells.len() > MAX_CELLS {
                    let problem = format!("its layers reach more than the {MAX_CELLS} cells foliage may");
                    return Err(ZoneError::invalid("layers", problem));
                }
            }
        }
        Ok(cells.into_iter().collect())
    }

    /// Its kinds, cooked: what a renderer loads beside a zone's scatter sets.
    #[must_use]
    pub fn cook(&self) -> Foliage {
        let kinds = self
            .kinds
            .iter()
            .map(|kind| CookedFoliageKind {
                id: Name::new(&kind.name).as_u64(),
                name: kind.name.clone(),
                model: kind.model,
            })
            .collect();
        Foliage { kinds }
    }
}

/// Whether a value is finite and within `low..=high`, or what's wrong with it.
fn check_positive_in(value: f32, low: f32, high: f32) -> Result<(), String> {
    if value.is_finite() && low <= value && value <= high {
        Ok(())
    } else {
        Err(format!("{value} isn't from {low} to {high}"))
    }
}

/// What's wrong with a layer's outlines: where (`[2][5]`) and what.
fn check_layer(layer: &FoliageLayer) -> Result<(), (String, String)> {
    if layer.outlines.is_empty() {
        return Err((String::new(), "it has no outline".to_owned()));
    }
    if layer.outlines.len() > MAX_OUTLINES {
        let problem = format!("{} is more than the {MAX_OUTLINES} a layer may hold", layer.outlines.len());
        return Err((String::new(), problem));
    }
    for (index, outline) in layer.outlines.iter().enumerate() {
        if !(3..=MAX_OUTLINE_POINTS).contains(&outline.len()) {
            let problem = format!("{} points isn't from 3 to {MAX_OUTLINE_POINTS}", outline.len());
            return Err((format!("[{index}]"), problem));
        }
        for (point, &[x, z]) in outline.iter().enumerate() {
            check_position([x, 0.0, z]).map_err(|problem| (format!("[{index}][{point}]"), problem))?;
        }
    }
    Ok(())
}

/// A foliage document's kinds, cooked (`.sfoliage`): what each kind of a zone's scatter sets draws. An rkyv
/// archive read in place.
#[derive(Clone, Debug, PartialEq, TypePath, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
#[rkyv(derive(Debug))]
#[type_path = "sapphire::zone"]
pub struct Foliage {
    /// In the document's order: a scatter set's kind is an index here.
    pub kinds: Vec<CookedFoliageKind>,
}

impl Versioned for Foliage {
    /// 1: M6 G1a.
    const VERSION: u32 = 1;
}

/// A kind, cooked: see [`FoliageKind`].
#[derive(Clone, Debug, PartialEq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
#[rkyv(derive(Debug))]
pub struct CookedFoliageKind {
    /// The name's hash ([`Name`]).
    pub id: u64,
    pub name: String,
    pub model: FoliageModel,
}

impl Foliage {
    pub fn to_cooked(&self) -> Result<AlignedVec, ZoneError> {
        Ok(sapphire_serialize::to_cooked(self)?)
    }

    /// Reads cooked bytes in place, checking the envelope, the archive and every value a reader leans on.
    pub fn read(bytes: &[u8]) -> Result<&ArchivedFoliage, ZoneError> {
        let foliage = sapphire_serialize::access::<Self>(bytes)?;
        foliage.validate()?;
        Ok(foliage)
    }
}

impl ArchivedFoliage {
    /// Every mesh its kinds draw, in their order: what ships in its zone's pack.
    #[must_use]
    pub fn meshes(&self) -> Vec<AssetId> {
        self.kinds.iter().map(|kind| kind.model.to_native().mesh()).collect()
    }

    /// Checks everything a reader leans on, whatever wrote these bytes: at most [`MAX_KINDS`], each named, its
    /// ID its name's hash, no two alike.
    pub fn validate(&self) -> Result<(), ZoneError> {
        if self.kinds.len() > MAX_KINDS {
            let problem =
                format!("{} kinds is more than the {MAX_KINDS} a document may hold", self.kinds.len());
            return Err(ZoneError::invalid("kinds", problem));
        }
        let mut seen = BTreeSet::new();
        for kind in self.kinds.iter() {
            let invalid =
                |problem: String| ZoneError::invalid(format!("kinds `{}`", kind.name.as_str()), problem);
            check_name(kind.name.as_str()).map_err(invalid)?;
            if kind.id.to_native() != Name::new(kind.name.as_str()).as_u64() {
                return Err(invalid("its ID isn't its name's hash".to_owned()));
            }
            if !seen.insert(kind.id.to_native()) {
                return Err(invalid("another kind has its ID".to_owned()));
            }
        }
        Ok(())
    }
}

/// One plant as the GPU reads it: 16 bytes (see the [module docs](self#the-instance)).
#[derive(
    Clone,
    Copy,
    Debug,
    PartialEq,
    bytemuck::Pod,
    bytemuck::Zeroable,
    rkyv::Archive,
    rkyv::Serialize,
    rkyv::Deserialize,
)]
#[rkyv(derive(Debug))]
#[repr(C)]
pub struct ScatterInstance {
    /// Its root, in metres in the zone's space.
    pub position: [f32; 3],
    /// Yaw in bits 0 to 7, scale in 8 to 15, tint in 16 to 23, flags in 24 to 31.
    pub packed: u32,
}

const _: () = assert!(size_of::<ArchivedScatterInstance>() == 16);
const _: () = assert!(size_of::<ScatterInstance>() == 16);

impl ScatterInstance {
    /// An instance at `position`, packed from its four bytes.
    #[must_use]
    pub fn new(position: [f32; 3], yaw: u8, scale: u8, tint: u8, flags: u8) -> Self {
        let packed = u32::from_le_bytes([yaw, scale, tint, flags]);
        Self { position, packed }
    }

    /// Its yaw's byte: `yaw · 2π / 256` radians about +Y, counter-clockwise seen from above.
    #[must_use]
    pub fn yaw_byte(self) -> u8 {
        self.packed.to_le_bytes()[0]
    }

    /// Its scale's byte: `0.5 · 4^(byte / 255)`.
    #[must_use]
    pub fn scale_byte(self) -> u8 {
        self.packed.to_le_bytes()[1]
    }

    /// Its tint's byte, for a palette to choose by (G4).
    #[must_use]
    pub fn tint(self) -> u8 {
        self.packed.to_le_bytes()[2]
    }

    /// Its flags, all zero until a later row gives one a meaning.
    #[must_use]
    pub fn flags(self) -> u8 {
        self.packed.to_le_bytes()[3]
    }

    /// Its yaw, in radians.
    #[must_use]
    pub fn yaw(self) -> f32 {
        f32::from(self.yaw_byte()) * (std::f32::consts::TAU / 256.0)
    }

    /// Its scale, from [`MIN_SCALE`] to [`MAX_SCALE`].
    #[must_use]
    pub fn scale(self) -> f32 {
        MIN_SCALE * sapphire_math::det::exp2(2.0 * f32::from(self.scale_byte()) / 255.0)
    }
}

/// A cell's plants, cooked (`.sscatter`): its instances by kind, then by chunk, then by ID, each kind's chunks a
/// run of [`Self::chunks`] and each chunk a run of [`Self::instances`]. An rkyv archive read in place.
#[derive(Clone, Debug, Default, PartialEq, TypePath, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
#[rkyv(derive(Debug))]
#[type_path = "sapphire::zone"]
pub struct ScatterSet {
    /// The cell's `[x, z]`.
    pub coord: [i32; 2],
    /// One for each kind it holds, by ascending kind.
    pub runs: Vec<ScatterRun>,
    pub chunks: Vec<ScatterChunk>,
    pub instances: Vec<ScatterInstance>,
    /// Each instance's stable ID, beside it (see the [module docs](self#stable-ids)): never uploaded.
    pub ids: Vec<u64>,
}

impl Versioned for ScatterSet {
    /// 1: M6 G1a.
    const VERSION: u32 = 1;
}

/// One kind's chunks in a [`ScatterSet`].
#[derive(Clone, Copy, Debug, PartialEq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
#[rkyv(derive(Debug))]
pub struct ScatterRun {
    /// The kind's index in its foliage's kinds.
    pub kind: u32,
    pub first_chunk: u32,
    pub chunk_count: u32,
}

/// One chunk of a kind's instances: those whose roots lie in one [`CHUNK_SIZE`] square of world space.
#[derive(Clone, Copy, Debug, PartialEq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
#[rkyv(derive(Debug))]
pub struct ScatterChunk {
    /// Its square's `[x, z]`, in chunks.
    pub coord: [i32; 2],
    /// Around its roots, in metres: a renderer grows it by its kind's mesh at its largest scale.
    pub roots_min: [f32; 3],
    pub roots_max: [f32; 3],
    pub first: u32,
    pub count: u32,
}

/// The chunk a root is in.
#[must_use]
pub fn chunk_of(position: [f32; 3]) -> [i32; 2] {
    [(position[0] / CHUNK_SIZE).floor() as i32, (position[2] / CHUNK_SIZE).floor() as i32]
}

impl ScatterSet {
    pub fn to_cooked(&self) -> Result<AlignedVec, ZoneError> {
        Ok(sapphire_serialize::to_cooked(self)?)
    }

    /// Reads cooked bytes in place, checking the envelope, the archive and every value a reader leans on.
    pub fn read(bytes: &[u8]) -> Result<&ArchivedScatterSet, ZoneError> {
        let set = sapphire_serialize::access::<Self>(bytes)?;
        set.validate()?;
        Ok(set)
    }

    /// A set of `placed`, in the cell `coord`, in any order: sorted, chunked and run by kind.
    #[must_use]
    pub fn from_placed(coord: [i32; 2], mut placed: Vec<Placed>) -> Self {
        // A placed set's IDs are unique; the bits make the order total whatever a caller passes.
        let order = |placed: &Placed| {
            let position = placed.instance.position.map(f32::to_bits);
            (placed.kind, chunk_of(placed.instance.position), placed.id, position, placed.instance.packed)
        };
        placed.sort_unstable_by_key(order);
        let mut set = Self { coord, ..Self::default() };
        let mut chunks: BTreeMap<(u32, [i32; 2]), usize> = BTreeMap::new();
        for placed in &placed {
            let key = (placed.kind, chunk_of(placed.instance.position));
            let index = *chunks.entry(key).or_insert_with(|| {
                set.chunks.push(ScatterChunk {
                    coord: key.1,
                    roots_min: placed.instance.position,
                    roots_max: placed.instance.position,
                    first: set.instances.len() as u32,
                    count: 0,
                });
                if set.runs.last().is_none_or(|run| run.kind != placed.kind) {
                    let first_chunk = set.chunks.len() as u32 - 1;
                    set.runs.push(ScatterRun { kind: placed.kind, first_chunk, chunk_count: 0 });
                }
                set.runs.last_mut().expect("just pushed").chunk_count += 1;
                set.chunks.len() - 1
            });
            let chunk = &mut set.chunks[index];
            for axis in 0..3 {
                chunk.roots_min[axis] = chunk.roots_min[axis].min(placed.instance.position[axis]);
                chunk.roots_max[axis] = chunk.roots_max[axis].max(placed.instance.position[axis]);
            }
            chunk.count += 1;
            set.instances.push(placed.instance);
            set.ids.push(placed.id);
        }
        set
    }
}

impl ArchivedScatterSet {
    /// Checks everything a reader leans on, whatever wrote these bytes: one ID an instance; runs by ascending
    /// kind sharing out the chunks, and chunks sharing out the instances, exactly and in order; each root
    /// finite and inside its chunk's square and bounds, and each chunk within the grid's coordinates; IDs
    /// strictly rising within a chunk. The set holds no cell size, so whether its chunks lie in its cell `coord`
    /// is the reader's to check, as is whether each kind is one of its foliage's, holding both.
    pub fn validate(&self) -> Result<(), ZoneError> {
        let invalid = |problem: String| ZoneError::invalid("scatter", problem);
        let count = self.instances.len();
        if count > MAX_SET_INSTANCES || self.ids.len() != count {
            return Err(invalid(format!(
                "{count} instances and {} IDs, at most {MAX_SET_INSTANCES}",
                self.ids.len()
            )));
        }
        let mut next_chunk = 0u32;
        let mut previous_kind = None;
        for run in self.runs.iter() {
            let kind = run.kind.to_native();
            if previous_kind.is_some_and(|previous| previous >= kind) || run.chunk_count == 0 {
                return Err(invalid(format!("kind {kind}'s run is out of order or empty")));
            }
            if run.first_chunk.to_native() != next_chunk {
                return Err(invalid(format!("kind {kind}'s chunks don't follow the run before")));
            }
            next_chunk = next_chunk.saturating_add(run.chunk_count.to_native());
            previous_kind = Some(kind);
        }
        if next_chunk as usize != self.chunks.len() {
            return Err(invalid(format!("the runs hold {next_chunk} of {} chunks", self.chunks.len())));
        }
        let mut next = 0u32;
        let reach = (MAX_COORDINATE / CHUNK_SIZE) as i32 + 1;
        for (index, chunk) in self.chunks.iter().enumerate() {
            let [x, z] = chunk.coord.each_ref().map(|value| value.to_native());
            let (first, length) = (chunk.first.to_native(), chunk.count.to_native());
            if first != next || length == 0 || x.abs() > reach || z.abs() > reach {
                return Err(invalid(format!(
                    "chunk {index} doesn't follow the one before, or is empty or far"
                )));
            }
            let end = first.saturating_add(length);
            if end as usize > count {
                return Err(invalid(format!("chunk {index} runs past the instances")));
            }
            let (min, max) = (
                chunk.roots_min.each_ref().map(|v| v.to_native()),
                chunk.roots_max.each_ref().map(|v| v.to_native()),
            );
            let mut previous_id = None;
            for at in first..end {
                let instance = &self.instances[at as usize];
                let position = instance.position.each_ref().map(|value| value.to_native());
                check_position(position).map_err(invalid)?;
                let inside = (0..3).all(|axis| min[axis] <= position[axis] && position[axis] <= max[axis]);
                if !inside || chunk_of(position) != [x, z] {
                    return Err(invalid(format!("instance {at} lies outside its chunk {index}")));
                }
                let id = self.ids[at as usize].to_native();
                if previous_id.is_some_and(|previous| previous >= id) {
                    return Err(invalid(format!("instance {at}'s ID is out of order in chunk {index}")));
                }
                previous_id = Some(id);
            }
            next = end;
        }
        if next as usize != count {
            return Err(invalid(format!("the chunks hold {next} of {count} instances")));
        }
        Ok(())
    }

    /// How many instances it holds.
    #[must_use]
    pub fn len(&self) -> usize {
        self.instances.len()
    }

    /// Whether it holds none.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.instances.is_empty()
    }

    /// The instance at `index`, as a plain value.
    #[must_use]
    pub fn instance(&self, index: usize) -> ScatterInstance {
        let instance = &self.instances[index];
        ScatterInstance {
            position: instance.position.each_ref().map(|value| value.to_native()),
            packed: instance.packed.to_native(),
        }
    }
}

/// The part a cell's scatter set is among its zone's sub-assets: `scatter/<x>/<z>`. Never changed: its ID is
/// derived from it.
#[must_use]
pub fn scatter_part(cell: CellCoord) -> String {
    format!("scatter/{}/{}", cell.x, cell.z)
}
