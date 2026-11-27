//! Ground cover (M6 G6b): bushes, ferns, flowers, grass clumps and rocks, each built from a spec beside the
//! trees, on the same deterministic maths and with the same atlas painter, cards and foliage bake.
//!
//! A species is a spec, `<name>.cover.ron` (`sapphire::pipeline::CoverSpecies`), which names its form and
//! that form's parameters, and one to [`MAX_VARIANTS`] variants, each a model `<name>_<variant>.glb` grown
//! from a seed of its own at its own size. Nothing is modelled by hand and nothing is downloaded. Each form
//! is built as suits what it is, and as foliage draws today (cards cut out of a painted atlas, opaque wood
//! and blades, the bake's volume normals), so the foliage field (G2) draws each model as one instance of a
//! kind:
//!
//! | Form | Built | Slots | Atlas | Normals |
//! |---|---|---|---|---|
//! | [`Bush`] | grown by the tree generator ([`super::grow`]): a crown reaching the ground, so the root forks into stems at once, cards on its twig tips | `Bark`, `Leaves` | the tree's brush | its clumps |
//! | [`Fern`] | fronds rising from its heart and arching out along a curve, each a strip of quads showing one painted frond | `Leaves` | fronds, four tall cells | one rounded volume |
//! | [`Flower`] | stalks as thin tubes, a head card on each facing up and out, leaf cards standing at their foot | `Stems`, `Leaves` | three heads and a fan of leaves | up, as the ground |
//! | [`Grass`] | opaque blades in a disc, each a tapering strip arching out from its root, both faces wound outward | `Blades` | none: the material's gradient | up, as the ground |
//! | [`Rock`] | a sphere pushed out onto a few cutting planes: broad flat faces with narrow bevels between them | `Rock` | flat swatches | its own faces |
//!
//! **Grass is blades, not cards.** §3 of the plan draws grass as opaque blades that cast no shadow on any
//! tier: an alpha-tested card costs a tiler its hidden-surface removal, and a clump of blades is one mesh
//! the field instances as it does every other kind, the shape the nature zone's tuft already has. A clump's
//! value steps are its blades' tones, three flat levels of vertex occlusion, darker toward its middle, under
//! the material's gradient from root to tip. The no-shadow half is the field's rule, not these models': no
//! material holds a shadow flag, so the field keeps grass out of its shadow views.
//!
//! **Rocks take the world's stylized material** (an environment material, as the nature zone's grounds
//! are): flat faces, each one flat swatch of a few painted tones (lighter on the bevels, as a painted stone's
//! edges catch the light, moss on the faces turned up), darker at the foot. No scan, and no noise. So a zone
//! places rocks as props, the large ones with authored colliders, rather than the foliage field, which draws
//! `foliage.wgsl` kinds alone until it gains an environment-family scatter entry (G2a).

mod build;
mod paint;
#[cfg(test)]
mod tests;

use std::io;
use std::path::Path;

use bevy_reflect::TypePath;
use sapphire_assets::AssetId;
use sapphire_assets::material::{
    DistanceStylization, EnvironmentMaterial, FoliageGradient, FoliageMaterial, Terminator, Translucency,
};
use sapphire_reflect::Versioned;
use serde::{Deserialize, Serialize};

use super::{
    Age, BARK_SLOT, Crown, Growth, LEAVES_SLOT, Leaves, TreeError, TreeSpecies, TreeWind, Wood, atlas,
    card_material, color, grow, mesh, plain_name, wood_material,
};
use crate::bake::foliage::{FoliageSettings, MAX_CLUMPS, NormalTransfer};

/// A ground-cover spec's file name ends with this.
pub const SPEC_EXTENSION: &str = ".cover.ron";

/// A flower's stalks.
pub const STEMS_SLOT: &str = "Stems";
/// A grass clump's blades.
pub const BLADES_SLOT: &str = "Blades";
/// A rock.
pub const ROCK_SLOT: &str = "Rock";

/// The most variants a species has.
pub const MAX_VARIANTS: usize = 4;

/// A rock's swatches' side, in texels, at every preset: each face samples one flat swatch, so more texels
/// show nothing more.
pub const SWATCH_SIZE: u32 = 64;

/// A species of ground cover: see the [module docs](self).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, TypePath)]
#[type_path = "sapphire::pipeline"]
#[serde(deny_unknown_fields)]
pub struct CoverSpecies {
    /// Lower-case ASCII: its files' names.
    pub name: String,
    /// Each variant grows from its own seed, derived from this and the variant's index.
    pub seed: u32,
    pub form: Form,
    /// Smallest first.
    pub variants: Vec<Variant>,
}

impl Versioned for CoverSpecies {
    const VERSION: u32 = 1;
}

/// One model of a species.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Variant {
    /// Lower-case ASCII: `<species>_<variant>.glb`.
    pub name: String,
    /// Its size in metres, which every share in its form is of: a bush's height, a fern's longest frond, a
    /// flower's or a grass clump's tallest stem or blade, a rock's width.
    pub size: f32,
}

/// What a species is, and how it's built.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Form {
    Bush(Bush),
    Fern(Fern),
    Flower(Flower),
    Grass(Grass),
    Rock(Rock),
}

/// A bush: a small tree grown low by the tree generator, its crown, growth, wood and leaves a tree's
/// ([`TreeSpecies`]); each variant an age at its size.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Bush {
    pub crown: Crown,
    pub growth: Growth,
    pub wood: Wood,
    pub leaves: Leaves,
    pub wind: TreeWind,
    /// Attraction points in its crown.
    pub points: u32,
    /// A leaf card's size, as a share of the size.
    pub card: f32,
    /// The clumps its leaves' normals come from.
    pub clumps: u32,
}

/// A fern: fronds from its heart, each rising at an angle and arching out under its weight.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Fern {
    pub fronds: u32,
    /// The least and greatest angle a frond leaves its heart at, in degrees above level.
    pub rise: (f32, f32),
    /// How far a frond's tip falls below its straight line, as a share of its length.
    pub arch: f32,
    /// A frond's least and greatest length, as shares of the size.
    pub length: (f32, f32),
    /// A frond's width, as a share of its length.
    pub width: f32,
    /// Quads along a frond: more, a smoother arch.
    pub segments: u32,
    pub leaves: Paint,
    pub wind: TreeWind,
}

/// Flowers: a few stalks from one foot, a head on each, leaves round the foot.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Flower {
    pub stems: u32,
    /// The radius of the ground the stalks rise from, as a share of the size.
    pub spread: f32,
    /// The shortest stalk, as a share of the size (the tallest is the size).
    pub shortest: f32,
    /// How far a stalk's head leans out over its foot, as a share of its height.
    pub lean: f32,
    /// A head's radius, as a share of the size.
    pub head: f32,
    pub petals: u32,
    /// A petal's width, as a share of its length: a daisy's narrow rays to a broad round petal.
    pub petal_width: f32,
    /// The heart's radius, as a share of a petal's length.
    pub heart: f32,
    /// Leaf cards standing round the foot.
    pub leaves: u32,
    /// The stalks' colour at the foot and at the head (`"#rrggbb"`).
    pub stalk: (String, String),
    /// The heads' colours: the petals' and, as the accent, the heart's.
    pub bloom: Paint,
    /// The leaves' fill, lit side and shade (`"#rrggbb"`).
    pub leaf: (String, String, String),
    pub wind: TreeWind,
}

/// A clump of grass: blades in a disc.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Grass {
    pub blades: u32,
    /// The disc the blades' roots stand in, its radius as a share of the size.
    pub radius: f32,
    /// The shortest blade, as a share of the size.
    pub shortest: f32,
    /// A blade's width at its root, as a share of the size.
    pub width: f32,
    /// How far a blade's tip leans out over its root, as a share of its height.
    pub lean: f32,
    /// Quads along a blade, its tip a point: more, a smoother arch.
    pub segments: u32,
    /// The material's gradient: the colour at the roots and at the tips (`"#rrggbb"`).
    pub root: String,
    pub tip: String,
    pub wind: TreeWind,
}

/// A rock: a sphere pushed out onto cutting planes.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Rock {
    /// Cutting planes: few, a blocky stone; many, a rounder one.
    pub planes: u32,
    /// Its height, above and below the ground together, as a share of its width.
    pub height: f32,
    /// The share of its height below the ground.
    pub sink: f32,
    /// How unevenly the planes cut: 0 cuts every one at the same depth.
    pub rough: f32,
    /// The stone's colours (`"#rrggbb"`): its faces' fill, the lighter bevels, the shade its faces turned
    /// down take, and the moss.
    pub fill: String,
    pub light: String,
    pub shade: String,
    pub moss: String,
    /// How far down from the top the moss reaches: a face takes it where its normal's upward part is over
    /// `1 − mossy`, so 0 grows none and 1 covers every face turned up at all.
    pub mossy: f32,
}

/// A painted atlas's four colours and its material's two (`"#rrggbb"`), as a tree's leaves have.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Paint {
    /// A leaf's or a petal's flat fill, the lighter stroke on its lit side, the shade inner ones take, and an
    /// accent (a frond's stalk, a flower's heart).
    pub fill: String,
    pub light: String,
    pub shade: String,
    pub accent: String,
    /// The material's gradient at the inside, and what the cards glow against a low sun.
    pub inner: String,
    pub translucency: String,
}

/// A species' atlas: a plant's painted leaves, fronds or flowers, or a rock's swatches.
#[derive(Clone, Debug, PartialEq)]
pub struct Atlas {
    /// Its file's name in the species' folder.
    pub name: &'static str,
    /// Its side, in texels.
    pub side: u32,
    /// A PNG: colour in sRGB, coverage in alpha.
    pub png: Vec<u8>,
}

/// One of a species' materials: what its slot draws with.
#[derive(Clone, Debug, PartialEq)]
pub enum CoverMaterial {
    Foliage(FoliageMaterial),
    Environment(EnvironmentMaterial),
}

/// Reads and checks the ground-cover spec at `path`.
pub fn read(path: &Path) -> Result<CoverSpecies, TreeError> {
    let text = std::fs::read_to_string(path)?;
    let species: CoverSpecies = sapphire_serialize::from_ron(&text)
        .map_err(|error| TreeError::Document { path: path.to_owned(), source: Box::new(error) })?;
    species.validate().map_err(|why| TreeError::Spec { path: path.to_owned(), why })?;
    Ok(species)
}

fn within(what: &str, value: f32, low: f32, high: f32) -> Result<(), String> {
    if (low..=high).contains(&value) {
        Ok(())
    } else {
        Err(format!("{what} is {value}, outside {low} to {high}"))
    }
}

fn colours<'a>(colours: impl IntoIterator<Item = (&'a str, &'a String)>) -> Result<(), String> {
    for (what, text) in colours {
        color(text).ok_or_else(|| format!("{what} is {text:?}, not \"#rrggbb\""))?;
    }
    Ok(())
}

fn wind(what: &str, TreeWind { sway, bend, flutter, flutter_frequency }: TreeWind) -> Result<(), String> {
    within(&format!("{what}.sway"), sway, 0.0, 1.0)?;
    within(&format!("{what}.bend"), bend, 0.0, 1.0)?;
    within(&format!("{what}.flutter"), flutter, 0.0, 1.0)?;
    within(&format!("{what}.flutter_frequency"), flutter_frequency, 0.0, 10.0)
}

impl Paint {
    fn validate(&self, what: &str) -> Result<(), String> {
        let named = |field: &str| format!("{what}.{field}");
        let names = ["fill", "light", "shade", "accent", "inner", "translucency"].map(named);
        let values = [&self.fill, &self.light, &self.shade, &self.accent, &self.inner, &self.translucency];
        colours(names.iter().map(String::as_str).zip(values))
    }

    fn palette(&self) -> io::Result<atlas::Palette> {
        atlas::Palette::read(&self.fill, &self.light, &self.shade, &self.accent)
    }

    fn rgb(text: &str) -> [f32; 3] {
        color(text).expect("a checked colour").to_array()
    }

    fn material(&self, atlas: AssetId, wind: TreeWind) -> FoliageMaterial {
        card_material(atlas, Self::rgb(&self.inner), Self::rgb(&self.translucency), wind.into())
    }
}

impl CoverSpecies {
    /// Checks every number is in its range and every colour reads, saying what isn't.
    pub fn validate(&self) -> Result<(), String> {
        if !plain_name(&self.name) {
            return Err(format!("name {:?}: lower-case ASCII letters, digits and `_`", self.name));
        }
        if !(1..=MAX_VARIANTS).contains(&self.variants.len()) {
            return Err(format!("{} variants: 1 to {MAX_VARIANTS}", self.variants.len()));
        }
        for (index, variant) in self.variants.iter().enumerate() {
            if !plain_name(&variant.name)
                || self.variants[..index].iter().any(|other| other.name == variant.name)
            {
                return Err(format!(
                    "variants[{index}].name: {:?} isn't a plain name of its own",
                    variant.name
                ));
            }
            within(&format!("variants[{index}].size"), variant.size, 0.05, 10.0)?;
        }
        match &self.form {
            Form::Bush(bush) => {
                within("form.points", bush.points as f32, 8.0, super::MAX_POINTS as f32)?;
                within("form.card", bush.card, 0.02, 1.0)?;
                within("form.clumps", bush.clumps as f32, 1.0, MAX_CLUMPS as f32)?;
                // What the tree's check holds an age to, said by the cover's own fields.
                for (index, variant) in self.variants.iter().enumerate() {
                    within(&format!("variants[{index}].size"), variant.size, 0.5, 10.0)?;
                    let card = bush.card * variant.size;
                    within(&format!("form.card × variants[{index}].size"), card, 0.05, 5.0)?;
                }
                self.bush(bush).validate().map_err(|why| format!("form: {why}"))
            }
            Form::Fern(fern) => {
                within("form.fronds", fern.fronds as f32, 3.0, 48.0)?;
                within("form.rise.0", fern.rise.0, 0.0, 90.0)?;
                within("form.rise.1", fern.rise.1, fern.rise.0, 90.0)?;
                within("form.arch", fern.arch, 0.0, 1.5)?;
                within("form.length.0", fern.length.0, 0.1, 1.0)?;
                within("form.length.1", fern.length.1, fern.length.0, 1.0)?;
                within("form.width", fern.width, 0.05, 0.6)?;
                within("form.segments", fern.segments as f32, 1.0, 12.0)?;
                fern.leaves.validate("form.leaves")?;
                wind("form.wind", fern.wind)
            }
            Form::Flower(flower) => {
                within("form.stems", flower.stems as f32, 1.0, 24.0)?;
                within("form.spread", flower.spread, 0.0, 0.5)?;
                within("form.shortest", flower.shortest, 0.2, 1.0)?;
                within("form.lean", flower.lean, 0.0, 0.8)?;
                within("form.head", flower.head, 0.02, 0.3)?;
                within("form.petals", flower.petals as f32, 3.0, 24.0)?;
                within("form.petal_width", flower.petal_width, 0.1, 1.0)?;
                within("form.heart", flower.heart, 0.05, 0.6)?;
                within("form.leaves", flower.leaves as f32, 0.0, 12.0)?;
                let (root, top) = &flower.stalk;
                let (fill, light, shade) = &flower.leaf;
                colours([
                    ("form.stalk.0", root),
                    ("form.stalk.1", top),
                    ("form.leaf.0", fill),
                    ("form.leaf.1", light),
                    ("form.leaf.2", shade),
                ])?;
                flower.bloom.validate("form.bloom")?;
                wind("form.wind", flower.wind)
            }
            Form::Grass(grass) => {
                within("form.blades", grass.blades as f32, 1.0, 256.0)?;
                within("form.radius", grass.radius, 0.0, 1.0)?;
                within("form.shortest", grass.shortest, 0.1, 1.0)?;
                within("form.width", grass.width, 0.005, 0.2)?;
                within("form.lean", grass.lean, 0.0, 1.0)?;
                within("form.segments", grass.segments as f32, 1.0, 6.0)?;
                colours([("form.root", &grass.root), ("form.tip", &grass.tip)])?;
                wind("form.wind", grass.wind)
            }
            Form::Rock(rock) => {
                within("form.planes", rock.planes as f32, 6.0, 48.0)?;
                within("form.height", rock.height, 0.2, 2.0)?;
                within("form.sink", rock.sink, 0.0, 0.5)?;
                within("form.rough", rock.rough, 0.0, 0.6)?;
                within("form.mossy", rock.mossy, 0.0, 1.0)?;
                colours([
                    ("form.fill", &rock.fill),
                    ("form.light", &rock.light),
                    ("form.shade", &rock.shade),
                    ("form.moss", &rock.moss),
                ])
            }
        }
    }

    /// The seed variant `variant` grows from, as a tree's age's is.
    fn variant_seed(&self, variant: usize) -> u32 {
        crate::samples::noise::hash(self.seed ^ crate::samples::noise::hash(variant as u32 + 1))
    }

    /// A bush as the tree generator grows it: its variants its ages.
    fn bush(&self, bush: &Bush) -> TreeSpecies {
        let ages = self.variants.iter().map(|variant| Age {
            name: variant.name.clone(),
            height: variant.size,
            points: bush.points,
            card: bush.card * variant.size,
            clumps: bush.clumps,
        });
        TreeSpecies {
            name: self.name.clone(),
            seed: self.seed,
            crown: bush.crown,
            growth: bush.growth,
            wood: bush.wood.clone(),
            leaves: bush.leaves.clone(),
            wind: bush.wind,
            ages: ages.collect(),
        }
    }

    /// Its model's slots, in order.
    pub fn slots(&self) -> &'static [&'static str] {
        match self.form {
            Form::Bush(_) => &[BARK_SLOT, LEAVES_SLOT],
            Form::Fern(_) => &[LEAVES_SLOT],
            Form::Flower(_) => &[STEMS_SLOT, LEAVES_SLOT],
            Form::Grass(_) => &[BLADES_SLOT],
            Form::Rock(_) => &[ROCK_SLOT],
        }
    }

    /// Variant `variant`'s model, a binary glTF with [`Self::slots`], its origin on the ground at its foot.
    pub fn model_glb(&self, variant: usize) -> Vec<u8> {
        let name = format!("{}_{}", self.name, self.variants[variant].name);
        let size = self.variants[variant].size;
        let seed = self.variant_seed(variant);
        match &self.form {
            Form::Bush(bush) => {
                let tree = self.bush(bush);
                mesh::tree_glb(&tree, variant, &grow::grow(&tree, variant))
            }
            Form::Fern(fern) => mesh::slots_glb(&name, &[(LEAVES_SLOT, &build::fern(fern, size, seed))]),
            Form::Flower(flower) => {
                let (stalks, cards) = build::flower(flower, size, seed);
                mesh::slots_glb(&name, &[(STEMS_SLOT, &stalks), (LEAVES_SLOT, &cards)])
            }
            Form::Grass(grass) => mesh::slots_glb(&name, &[(BLADES_SLOT, &build::grass(grass, size, seed))]),
            Form::Rock(rock) => mesh::slots_glb(&name, &[(ROCK_SLOT, &build::rock(rock, size, seed))]),
        }
    }

    /// Its atlas at `size` texels a side (a rock's swatches are [`SWATCH_SIZE`] at every size); a grass clump
    /// paints none.
    pub fn atlas(&self, size: u32) -> io::Result<Option<Atlas>> {
        let leaves = |png| Atlas { name: "leaves.png", side: size, png };
        Ok(Some(match &self.form {
            Form::Bush(bush) => leaves(atlas::atlas_png(&self.bush(bush), size)?),
            Form::Fern(fern) => leaves(paint::fern(self.seed, &fern.leaves.palette()?, size)?),
            Form::Flower(flower) => {
                let (fill, light, shade) = &flower.leaf;
                let leaf = atlas::Palette::read(fill, light, shade, shade)?;
                leaves(paint::flower(self.seed, flower, &flower.bloom.palette()?, &leaf, size)?)
            }
            Form::Grass(_) => return Ok(None),
            Form::Rock(rock) => {
                let stone = atlas::Palette::read(&rock.fill, &rock.light, &rock.shade, &rock.moss)?;
                Atlas { name: "stone.png", side: SWATCH_SIZE, png: paint::swatches(&stone, SWATCH_SIZE)? }
            }
        }))
    }

    /// The material each slot draws with, by slot, and the file each is written to; `atlas` is the atlas's ID,
    /// if it paints one.
    pub fn materials(&self, atlas: Option<AssetId>) -> Vec<(&'static str, &'static str, CoverMaterial)> {
        let atlas = || atlas.expect("a species that paints an atlas is given its ID");
        let rgb = Paint::rgb;
        match &self.form {
            Form::Bush(bush) => {
                let tree = self.bush(bush);
                vec![
                    (BARK_SLOT, "bark.material.ron", CoverMaterial::Foliage(tree.bark_material())),
                    (
                        LEAVES_SLOT,
                        "leaves.material.ron",
                        CoverMaterial::Foliage(tree.leaves_material(atlas())),
                    ),
                ]
            }
            Form::Fern(fern) => vec![(
                LEAVES_SLOT,
                "leaves.material.ron",
                CoverMaterial::Foliage(fern.leaves.material(atlas(), fern.wind)),
            )],
            Form::Flower(flower) => {
                let (root, top) = &flower.stalk;
                vec![
                    (
                        STEMS_SLOT,
                        "stems.material.ron",
                        CoverMaterial::Foliage(wood_material(rgb(root), rgb(top), flower.wind.into())),
                    ),
                    (
                        LEAVES_SLOT,
                        "leaves.material.ron",
                        CoverMaterial::Foliage(flower.bloom.material(atlas(), flower.wind)),
                    ),
                ]
            }
            Form::Grass(grass) => vec![(
                BLADES_SLOT,
                "blades.material.ron",
                CoverMaterial::Foliage(blades_material(rgb(&grass.root), rgb(&grass.tip), grass.wind)),
            )],
            Form::Rock(_) => {
                vec![(ROCK_SLOT, "rock.material.ron", CoverMaterial::Environment(stone_material(atlas())))]
            }
        }
    }

    /// The foliage bake variant `variant`'s sidecar asks for: a bush's leaves' normals from its clumps, a
    /// fern's from one rounded volume, a flower's and a grass clump's straight up, so they light as the ground
    /// they stand on; a rock is no foliage.
    pub fn foliage(&self, variant: usize) -> Option<FoliageSettings> {
        let up = |slot: &str, blend: f32| FoliageSettings {
            normals: NormalTransfer::Up,
            slots: vec![slot.to_owned()],
            blend,
            canopy_from: 0.0,
        };
        match &self.form {
            Form::Bush(bush) => Some(self.bush(bush).foliage(variant)),
            Form::Fern(_) => Some(FoliageSettings {
                normals: NormalTransfer::Sphere,
                slots: vec![LEAVES_SLOT.to_owned()],
                blend: 0.8,
                canopy_from: 0.0,
            }),
            Form::Flower(_) => Some(up(LEAVES_SLOT, 0.8)),
            Form::Grass(_) => Some(up(BLADES_SLOT, 1.0)),
            Form::Rock(_) => None,
        }
    }
}

/// A grass clump's blades: opaque, their gradient root to tip, swaying in `wind`, with the painted falloff
/// every tree material has.
fn blades_material(root: [f32; 3], tip: [f32; 3], wind: TreeWind) -> FoliageMaterial {
    FoliageMaterial {
        base_color: [1.0; 4],
        alpha_cutoff: 0.0,
        canopy_occlusion: 0.0,
        translucency: Translucency { strength: 0.0, ..Translucency::default() },
        gradient: FoliageGradient { root, tip },
        wind: wind.into(),
        ..super::painted()
    }
}

/// A rock's material, the world's: the environment family's, as the nature zone's grounds draw, its colour
/// the swatches `swatches`, rough, with a soft painted falloff and a narrow terminator.
fn stone_material(swatches: AssetId) -> EnvironmentMaterial {
    EnvironmentMaterial {
        base_color: [1.0; 4],
        base_color_map: Some(swatches),
        roughness: 1.0,
        roughness_range: [0.4, 1.0],
        specular_clamp: 0.25,
        wrap: 0.4,
        terminator: Terminator { width: 0.06, growth: 0.008, band: None },
        distance: DistanceStylization { start: 30.0, end: 400.0, desaturation: 0.4 },
        ..EnvironmentMaterial::default()
    }
}
