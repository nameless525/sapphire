//! Trees (M6 G6a): species grown by space colonization from a spec, and written as ordinary sources: a model
//! an age, a leaf atlas painted in brush strokes, and the two foliage materials they draw with.
//!
//! A species starts as a spec, `<name>.tree.ron` (`sapphire::pipeline::TreeSpecies`), which says the shape of
//! its crown, how it grows, its wood, its leaves' brush and palette, its wind and its ages. Nothing is
//! modelled by hand and nothing is downloaded: every branch and every texel comes from the spec and its seed,
//! on the deterministic maths, so a species writes the same bytes on every machine. The benchmark's trees
//! (`bench::trees`) are the species the repository grows; nothing in `content/` is one yet. Ground cover, the
//! bushes, ferns, flowers, grass and rocks beneath them, is built beside them in [`cover`] with the same
//! painter, cards and bake.
//!
//! # Growing
//!
//! [`grow`] fills the crown with attraction points and grows a skeleton toward them by Runions, Lane and
//! Prusinkiewicz's space colonization (2007): each point pulls the node nearest it within the influence
//! distance, each pulled node grows a step toward the mean of its pulls (plus the species' tropism), and a
//! point a node comes within the kill distance of is spent. The trunk first grows straight to the spec's
//! `leader`, so a conifer keeps its central stem and a broadleaf its clear bole. The skeleton is smoothed,
//! and each node's radius is the pipe model's: a twig tip's radius, and at a fork
//! `r = (Σ rᵢ^e)^(1/e)` over its children.
//!
//! # The model
//!
//! [`mesh`] writes one binary glTF a species and age, its origin at the root, with two slots: `Bark`, the
//! branches thicker than the spec's `thinnest` as tubes (each chain of main children one tube, its sides
//! fewer as it thins), and `Leaves`, cards on every twig tip, thin twigs included, each showing one cell of
//! the species' atlas. Its sidecar bakes foliage with `Clumps` over the leaves, so a canopy lights as a few
//! rounded masses rather than as single cards ([`crate::bake::foliage`]), with as many clumps as the age
//! says. Its levels are off: simplification would merge cards (G3's card-thinned levels come later).
//!
//! # The atlas
//!
//! [`atlas`] paints the species' leaves, a few variants of one sprig, in brush strokes: each leaf, petal or
//! needle a flat fill whose edge wobbles as a brush's does, a lighter stroke on its lit side, inner leaves
//! in the shade colour and outer ones in the fill, coverage in alpha and every cell clear at its border.
//! Never photographic: two tones and a hand-painted edge, as the reference games' foliage reads.

pub mod atlas;
pub mod cover;
pub mod grow;
pub mod mesh;
#[cfg(test)]
mod tests;

use std::path::{Path, PathBuf};

use bevy_reflect::TypePath;
use glam::Vec3;
use sapphire_assets::AssetId;
use sapphire_assets::material::{DistanceStylization, FoliageGradient, FoliageMaterial, Translucency, Wind};
use sapphire_reflect::Versioned;
use sapphire_serialize::DocumentError;
use serde::{Deserialize, Serialize};

use crate::bake::foliage::{FoliageSettings, MAX_CLUMPS, NormalTransfer};
use crate::color::srgb_to_linear;

/// A species spec's file name ends with this.
pub const SPEC_EXTENSION: &str = ".tree.ron";

/// The slot the wood draws in.
pub const BARK_SLOT: &str = "Bark";
/// The slot the leaf cards draw in.
pub const LEAVES_SLOT: &str = "Leaves";

/// The most ages a species has.
pub const MAX_AGES: usize = 4;
/// The most attraction points an age's crown holds.
pub const MAX_POINTS: u32 = 4000;

/// A species: see the [module docs](self).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, TypePath)]
#[type_path = "sapphire::pipeline"]
#[serde(deny_unknown_fields)]
pub struct TreeSpecies {
    /// Lower-case ASCII: its files' names.
    pub name: String,
    /// Each age grows from its own seed, derived from this and the age's index.
    pub seed: u32,
    pub crown: Crown,
    pub growth: Growth,
    pub wood: Wood,
    pub leaves: Leaves,
    pub wind: TreeWind,
    /// Youngest first.
    pub ages: Vec<Age>,
}

impl Versioned for TreeSpecies {
    const VERSION: u32 = 1;
}

/// The volume the attraction points fill, in fractions of the tree's height.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Crown {
    pub shape: CrownShape,
    /// Where the crown starts above the root.
    pub base: f32,
    /// Its widest radius.
    pub radius: f32,
    /// How far its points crowd toward its surface: 0 fills it evenly, 1 puts them all on it, so the
    /// branches reach out to a canopy of clumps rather than filling it.
    pub shell: f32,
}

/// A crown's shape, from its base to the tree's top.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum CrownShape {
    /// An ellipsoid: a broadleaf's round head, a blossom's wide one, a willow's dome.
    Ellipsoid,
    /// A cone, widest at its base: a conifer's.
    Cone,
}

/// How the skeleton grows.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Growth {
    /// The tree's height in growth steps: every age grows at the same resolution for its size.
    pub steps: u32,
    /// How far a point pulls a node, in steps.
    pub influence: f32,
    /// How near a node comes to a point to spend it, in steps.
    pub kill: f32,
    /// How high the trunk grows straight before the crown pulls it, as a fraction of the height.
    pub leader: f32,
    /// Added to every step's direction, up (positive) or down (negative), in steps.
    pub tropism: f32,
    /// How far the top of the trunk leans from over its root, as a fraction of the height.
    pub lean: f32,
}

/// The wood: its radii and its colour.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Wood {
    /// A twig tip's radius, in metres.
    pub tip: f32,
    /// The pipe model's exponent at a fork, 2 to 3.
    pub exponent: f32,
    /// Branches thinner than this, in metres, aren't modelled: the cards hide them.
    pub thinnest: f32,
    /// Sides around the trunk, and around the thinnest branch modelled.
    pub sides: (u32, u32),
    /// How much wider the trunk's foot flares, as a share of its radius.
    pub flare: f32,
    /// The bark's colour at the root and at the crown (`"#rrggbb"`), the material's gradient.
    pub root: String,
    pub top: String,
}

/// The leaves: their brush, their cards and their palette.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Leaves {
    pub brush: Brush,
    /// Cards on each twig tip.
    pub cards: u32,
    /// The atlas's colours (`"#rrggbb"`): a leaf's flat fill, the lighter stroke on its lit side, the
    /// shade the inner leaves take, and an accent (twigs, a flower's heart).
    pub fill: String,
    pub light: String,
    pub shade: String,
    pub accent: String,
    /// The material's gradient at the canopy's inside: the atlas's colours times this deep in the canopy,
    /// times white at its surface.
    pub inner: String,
    /// What a leaf glows against a low sun.
    pub translucency: String,
}

/// What a leaf card shows, and how its cards sit.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Brush {
    /// A sprig of pointed leaves, cards facing every way out of the crown: a broadleaf's.
    Broad,
    /// A puff of five-petalled flowers, cards as a broadleaf's: a blossom's.
    Petal,
    /// A frond of needles along a stem, cards lying along their twigs: a conifer's.
    Needle,
    /// Strands of narrow leaves hanging from their twigs, cards hanging straight down: a willow's.
    Strand,
}

/// The wind's response, the material's ([`Wind`]), which bark and leaves share so the canopy stays on its
/// branches.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TreeWind {
    pub sway: f32,
    pub bend: f32,
    pub flutter: f32,
    pub flutter_frequency: f32,
}

/// One age of a species.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Age {
    /// Lower-case ASCII: `<species>_<age>.glb`.
    pub name: String,
    /// Metres, root to crown.
    pub height: f32,
    /// Attraction points in its crown: more points, more branches.
    pub points: u32,
    /// A leaf card's size, in metres.
    pub card: f32,
    /// The clumps its canopy's normals come from ([`NormalTransfer::Clumps`]).
    pub clumps: u32,
}

/// Why a species can't be read or grown.
#[derive(Debug, thiserror::Error)]
pub enum TreeError {
    #[error("{path}: {source}")]
    Document { path: PathBuf, source: Box<DocumentError> },
    #[error("{path}: {why}")]
    Spec { path: PathBuf, why: String },
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

/// Reads and checks the species spec at `path`.
pub fn read(path: &Path) -> Result<TreeSpecies, TreeError> {
    let text = std::fs::read_to_string(path)?;
    let species: TreeSpecies = sapphire_serialize::from_ron(&text)
        .map_err(|error| TreeError::Document { path: path.to_owned(), source: Box::new(error) })?;
    species.validate().map_err(|why| TreeError::Spec { path: path.to_owned(), why })?;
    Ok(species)
}

/// Whether `name` is a file name's part: lower-case ASCII letters, digits and `_`, not empty.
fn plain_name(name: &str) -> bool {
    !name.is_empty() && name.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
}

/// Linear RGB from `"#rrggbb"`, or `None` if it isn't one.
pub fn color(text: &str) -> Option<Vec3> {
    let hex = text.strip_prefix('#').filter(|hex| hex.len() == 6 && hex.is_ascii())?;
    let channel = |at: usize| u8::from_str_radix(&hex[at..at + 2], 16).ok();
    let [r, g, b] = [channel(0)?, channel(2)?, channel(4)?];
    let linear = |v: u8| srgb_to_linear(f32::from(v) / 255.0);
    Some(Vec3::new(linear(r), linear(g), linear(b)))
}

impl TreeSpecies {
    /// Checks every number is in its range and every colour reads, saying what isn't.
    pub fn validate(&self) -> Result<(), String> {
        let within = |what: &str, value: f32, low: f32, high: f32| {
            if (low..=high).contains(&value) {
                Ok(())
            } else {
                Err(format!("{what} is {value}, outside {low} to {high}"))
            }
        };
        if !plain_name(&self.name) {
            return Err(format!("name {:?}: lower-case ASCII letters, digits and `_`", self.name));
        }
        let Crown { base, radius, shell, .. } = self.crown;
        within("crown.base", base, 0.0, 0.9)?;
        within("crown.radius", radius, 0.05, 1.0)?;
        within("crown.shell", shell, 0.0, 1.0)?;
        let Growth { steps, influence, kill, leader, tropism, lean } = self.growth;
        within("growth.steps", steps as f32, 8.0, 200.0)?;
        within("growth.influence", influence, 1.0, 30.0)?;
        within("growth.kill", kill, 0.5, influence)?;
        within("growth.leader", leader, 0.0, 1.0)?;
        within("growth.tropism", tropism, -2.0, 2.0)?;
        within("growth.lean", lean, 0.0, 0.5)?;
        let wood = &self.wood;
        within("wood.tip", wood.tip, 0.001, 0.2)?;
        within("wood.exponent", wood.exponent, 1.5, 3.5)?;
        within("wood.thinnest", wood.thinnest, wood.tip, 1.0)?;
        within("wood.sides.0", wood.sides.0 as f32, 3.0, 32.0)?;
        within("wood.sides.1", wood.sides.1 as f32, 3.0, wood.sides.0 as f32)?;
        within("wood.flare", wood.flare, 0.0, 2.0)?;
        within("leaves.cards", self.leaves.cards as f32, 1.0, 8.0)?;
        let leaves = &self.leaves;
        for (what, text) in [
            ("wood.root", &wood.root),
            ("wood.top", &wood.top),
            ("leaves.fill", &leaves.fill),
            ("leaves.light", &leaves.light),
            ("leaves.shade", &leaves.shade),
            ("leaves.accent", &leaves.accent),
            ("leaves.inner", &leaves.inner),
            ("leaves.translucency", &leaves.translucency),
        ] {
            color(text).ok_or_else(|| format!("{what} is {text:?}, not \"#rrggbb\""))?;
        }
        let TreeWind { sway, bend, flutter, flutter_frequency } = self.wind;
        within("wind.sway", sway, 0.0, 1.0)?;
        within("wind.bend", bend, 0.0, 1.0)?;
        within("wind.flutter", flutter, 0.0, 1.0)?;
        within("wind.flutter_frequency", flutter_frequency, 0.0, 10.0)?;
        if !(1..=MAX_AGES).contains(&self.ages.len()) {
            return Err(format!("{} ages: 1 to {MAX_AGES}", self.ages.len()));
        }
        for (index, age) in self.ages.iter().enumerate() {
            let what = |field: &str| format!("ages[{index}].{field}");
            if !plain_name(&age.name) || self.ages[..index].iter().any(|other| other.name == age.name) {
                return Err(format!("{}: {:?} isn't a plain name of its own", what("name"), age.name));
            }
            within(&what("height"), age.height, 0.5, 60.0)?;
            within(&what("points"), age.points as f32, 8.0, MAX_POINTS as f32)?;
            within(&what("card"), age.card, 0.05, 5.0)?;
            within(&what("clumps"), age.clumps as f32, 1.0, MAX_CLUMPS as f32)?;
        }
        Ok(())
    }

    /// The seed age `age` grows from.
    fn age_seed(&self, age: usize) -> u32 {
        crate::samples::noise::hash(self.seed ^ crate::samples::noise::hash(age as u32 + 1))
    }

    /// The foliage bake an age's model's sidecar asks for: its leaves' normals from its clumps.
    pub fn foliage(&self, age: usize) -> FoliageSettings {
        FoliageSettings {
            normals: NormalTransfer::Clumps { count: self.ages[age].clumps },
            slots: vec![LEAVES_SLOT.to_owned()],
            blend: 0.8,
            canopy_from: 0.0,
        }
    }

    /// The bark: opaque, its gradient root to crown, swaying with its leaves.
    pub fn bark_material(&self) -> FoliageMaterial {
        let rgb = |text: &str| color(text).expect("a checked colour").to_array();
        wood_material(rgb(&self.wood.root), rgb(&self.wood.top), self.wind.into())
    }

    /// The leaves: the atlas `atlas`, alpha-tested, darker toward the canopy's inside, glowing against a
    /// low sun.
    pub fn leaves_material(&self, atlas: AssetId) -> FoliageMaterial {
        let rgb = |text: &str| color(text).expect("a checked colour").to_array();
        card_material(atlas, rgb(&self.leaves.inner), rgb(&self.leaves.translucency), self.wind.into())
    }
}

impl From<TreeWind> for Wind {
    fn from(TreeWind { sway, bend, flutter, flutter_frequency }: TreeWind) -> Self {
        Self { sway, bend, flutter, flutter_frequency }
    }
}

/// Wood, or a flower's stalks: opaque, its gradient `root` to `top` (linear RGB), swaying in `wind`.
fn wood_material(root: [f32; 3], top: [f32; 3], wind: Wind) -> FoliageMaterial {
    FoliageMaterial {
        base_color: [1.0; 4],
        alpha_cutoff: 0.0,
        wrap: 0.35,
        terminator_width: 0.08,
        canopy_occlusion: 0.0,
        translucency: Translucency { strength: 0.0, ..Translucency::default() },
        gradient: FoliageGradient { root, tip: top },
        wind,
        ..painted()
    }
}

/// Cards showing `atlas`: alpha-tested, `inner` (linear RGB) deep in the canopy and white at its surface,
/// glowing `translucency` against a low sun, swaying in `wind`.
fn card_material(atlas: AssetId, inner: [f32; 3], translucency: [f32; 3], wind: Wind) -> FoliageMaterial {
    FoliageMaterial {
        base_color: [1.0; 4],
        base_color_map: Some(atlas),
        alpha_cutoff: 0.45,
        canopy_occlusion: 0.5,
        translucency: Translucency { color: translucency, strength: 0.6, power: 4.0 },
        gradient: FoliageGradient { root: inner, tip: [1.0; 3] },
        wind,
        ..painted()
    }
}

/// What every tree material shares, as the foliage samples' do: a soft painted falloff, a terminator that
/// softens far away under a lavender shade, and distant colour fading toward grey.
fn painted() -> FoliageMaterial {
    FoliageMaterial {
        wrap: 0.5,
        terminator_width: 0.16,
        terminator_growth: 0.006,
        shadow_tint: [0.95, 0.95, 1.08],
        distance: DistanceStylization { start: 15.0, end: 140.0, desaturation: 0.45 },
        ..FoliageMaterial::default()
    }
}
