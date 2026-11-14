//! The benchmark's cast (M5 B3): figures at spec §3.4's rows, built from specs ([`crate::figure`]) in
//! the anime character family, as many copies of each as a preset says.
//!
//! `bench/cast/` holds:
//!
//! | File | What |
//! |---|---|
//! | `cast.ron` | [`BenchCast`]: each figure, and what each preset makes of it |
//! | `figures/<spec>.figure.ron` | Figure specs, whose `dir` the cast replaces with its own |
//!
//! It generates, for copy `c` of each spec, under `cast/<name>_<cc>/` in the output: the files
//! [`figure::figure_files`] makes, and for each of the figure's materials an ILM document over its
//! control map, a ramp document with a row per region, for a face material a face SDF document over
//! its threshold masks, and an anime character material, which the model's sidecar binds to its
//! slot. Copies are the same bytes under different IDs: as many meshes and textures for a pool to
//! hold as distinct characters would be.
//!
//! A preset scales a figure: its copies, and its `detail`, map size and exact triangle count over
//! the spec's own. Maps larger than spec §3.4's 2K a part on desktop or 1K on mobile are capped on that
//! cook in their sidecars ([`PART_MAP`], M5 M1): the budget hero's 2K maps store 1K on mobile. A figure may share another's skeleton (`skeleton`): it takes that figure's joints
//! and its model's sidecar names the first copy of that figure's model, so their poses are each
//! other's (M5 C11's crowd).

use std::num::NonZeroU32;
use std::path::Path;

use bevy_reflect::TypePath;
use sapphire_assets::material::{
    AnimeCharacterMaterial, CharacterRegion, CharacterSurface, DiffuseMode, FaceShading, HairShading, Matcap,
    MaterialDocument, RampDiffuse,
};
use sapphire_reflect::Versioned;
use serde::{Deserialize, Serialize};

use super::{BenchError, Preset, Presets, Source, asset_id, document_bytes, read_document};
use crate::character_maps::MapKind;
use crate::figure::{self, Figure, FigureFile, FileKind, MaterialGroup, Surface};
use crate::import::vrm::{VrmSettings, VrmVersion};
use crate::material::MaterialSettings;
use crate::meta::{ImportSettings, MeshSettings};
use crate::optimize::LodSettings;
use crate::texture::{ColorSettings, MaskChannels, MaskSettings, MaxSize, TextureClass, TextureSettings};

/// This part's folder, under `bench/` and in the output.
pub const PART: &str = "cast";

/// The cast's document, `bench/cast/cast.ron` (`sapphire::pipeline::BenchCast`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, TypePath)]
#[type_path = "sapphire::pipeline"]
#[serde(deny_unknown_fields)]
pub struct BenchCast {
    pub figures: Vec<CastFigure>,
}

impl Versioned for BenchCast {
    /// 1: M5 B1's stand-in, copies of every spec. 2: M5 B3, figure by figure.
    const VERSION: u32 = 2;
}

/// One figure of the cast.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CastFigure {
    /// Its spec, `figures/<spec>.figure.ron`.
    pub spec: String,
    /// The figure whose skeleton it shares: it lists no joints of its own and takes that one's.
    #[serde(default)]
    pub skeleton: Option<String>,
    /// Its model's levels of detail, if not the importer's default chain.
    #[serde(default)]
    pub lods: Option<LodSettings>,
    /// What each preset makes of it: none leaves it out.
    pub presets: Presets<Option<FigureScale>>,
}

/// What a preset makes of a figure.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FigureScale {
    pub copies: u32,
    /// Over the spec's `detail`.
    #[serde(default)]
    pub detail: Option<u32>,
    /// Over the size of the spec's `maps`.
    #[serde(default)]
    pub maps: Option<u32>,
    /// Over the spec's exact count, and its budget with it.
    #[serde(default)]
    pub triangles: Option<u32>,
}

/// The most copies of a figure.
const MAX_COPIES: u32 = 100;

/// Every ramp row: the shadow side's tint from the deepest shadow to the terminator, by day a cool
/// violet that keeps a region's hue, by night a deeper blue, as the committed figures' are.
const RAMP_DAY: [(f32, &str); 3] = [(0.0, "#6a6aa0"), (0.7, "#8484b6"), (1.0, "#9c9cc6")];
const RAMP_NIGHT: [(f32, &str); 2] = [(0.0, "#565aa4"), (1.0, "#8086bc")];

/// A copy's folder in the output: `<name>_<cc>`.
fn folder(name: &str, copy: u32) -> String {
    format!("{name}_{copy:02}")
}

/// Spec §3.4's longest side for a character part's map, per cook (M5 M1).
pub const PART_MAP: MaxSize = MaxSize { desktop: NonZeroU32::new(2048), mobile: NonZeroU32::new(1024) };

/// The cap a figure's maps take: spec §3.4's on each cook its maps are over, so a map within both is
/// written as it always was.
fn map_cap(figure: &Figure) -> MaxSize {
    let size = figure.maps.as_ref().map_or(0, |maps| maps.size);
    let over = |cap: Option<NonZeroU32>| cap.filter(|cap| size > cap.get());
    MaxSize { desktop: over(PART_MAP.desktop), mobile: over(PART_MAP.mobile) }
}

/// One figure of the cast at a preset's scale: its spec as the preset makes it, with its own folder
/// for each copy still to set.
#[derive(Clone, Debug)]
pub struct CastMember {
    pub entry: CastFigure,
    pub figure: Figure,
    pub copies: u32,
    /// The model whose skeleton it shares, if it does.
    pub skeleton: Option<sapphire_assets::AssetId>,
    /// Its spec's path.
    pub spec: std::path::PathBuf,
}

impl CastMember {
    /// Copy `copy` of it, in its own folder.
    pub fn copy(&self, copy: u32) -> Figure {
        Figure { dir: folder(&self.figure.name, copy), ..self.figure.clone() }
    }
}

/// Generates the cast from the documents in `dir` (`bench/cast/`) at `preset`'s scale.
pub fn generate(dir: &Path, preset: Preset) -> Result<Vec<Source>, BenchError> {
    let mut sources = Vec::new();
    for member in members(dir, preset)? {
        // Copies differ only in their folder: build and paint the figure once.
        let first = member.copy(0);
        let files = figure::figure_files(&first)?;
        for copy in 0..member.copies {
            let figure = member.copy(copy);
            let files: Vec<FigureFile> = files
                .iter()
                .map(|file| FigureFile {
                    path: format!("{}{}", figure.dir, &file.path[first.dir.len()..]),
                    ..file.clone()
                })
                .collect();
            figure_sources(&figure, files, &member.entry, member.skeleton, &member.spec, &mut sources)?;
        }
    }
    Ok(sources)
}

/// The figures `preset` makes of the cast in `dir`, in the document's order.
pub fn members(dir: &Path, preset: Preset) -> Result<Vec<CastMember>, BenchError> {
    let document_path = dir.join("cast.ron");
    let cast: BenchCast = read_document(&document_path)?;
    let invalid = |why: String| BenchError::Invalid { path: document_path.clone(), why };
    let spec_path = |spec: &str| dir.join("figures").join(format!("{spec}{}", figure::SPEC_EXTENSION));
    let mut members = Vec::new();
    for entry in &cast.figures {
        let Some(scale) = *entry.presets.get(preset) else { continue };
        if !(1..=MAX_COPIES).contains(&scale.copies) {
            return Err(invalid(format!("{}, {preset}: 1 to {MAX_COPIES} copies", entry.spec)));
        }
        let path = spec_path(&entry.spec);
        let mut spec = figure::read(&path)?;
        if let Some(detail) = scale.detail {
            spec.detail = detail;
        }
        if let Some(size) = scale.maps {
            let maps = spec
                .maps
                .as_mut()
                .ok_or_else(|| invalid(format!("{}: its spec paints no `maps`", entry.spec)))?;
            maps.size = size;
        }
        if let Some(triangles) = scale.triangles {
            spec.triangles = Some(triangles);
            spec.max_triangles = spec.max_triangles.max(triangles);
        }
        // A shared skeleton is the owner's joints, and its first copy's.
        let skeleton = match &entry.skeleton {
            None => None,
            Some(owner) => {
                let owner_entry = cast
                    .figures
                    .iter()
                    .find(|other| &other.spec == owner && other.skeleton.is_none())
                    .ok_or_else(|| {
                        invalid(format!(
                            "{}: shares {owner:?}, which isn't a figure with its own skeleton",
                            entry.spec
                        ))
                    })?;
                if owner_entry.presets.get(preset).is_none() {
                    return Err(invalid(format!(
                        "{}: shares {owner:?}, which {preset} leaves out",
                        entry.spec
                    )));
                }
                if !spec.joints.is_empty() || !spec.chains.is_empty() {
                    return Err(invalid(format!(
                        "{}: shares {owner:?}'s skeleton, so lists no joints",
                        entry.spec
                    )));
                }
                let owner_spec = figure::read(&spec_path(owner))?;
                let model = asset_id(PART, &model_path(&owner_spec, &folder(&owner_spec.name, 0)));
                spec.joints = owner_spec.joints;
                spec.chains = owner_spec.chains;
                Some(model)
            }
        };
        if members.iter().any(|member: &CastMember| member.figure.name == spec.name) {
            return Err(invalid(format!("two figures are called {:?}", spec.name)));
        }
        members.push(CastMember {
            entry: entry.clone(),
            figure: spec,
            copies: scale.copies,
            skeleton,
            spec: path,
        });
    }
    Ok(members)
}

/// A figure's model's path in its folder.
fn model_path(figure: &Figure, folder: &str) -> String {
    let extension = if figure.character.is_some() { "vrm" } else { "glb" };
    format!("{folder}/{}.{extension}", figure.name)
}

/// A figure's `files`, made in its folder, and for each material its ILM, ramp, face SDF and
/// material documents.
fn figure_sources(
    figure: &Figure,
    files: Vec<FigureFile>,
    entry: &CastFigure,
    skeleton: Option<sapphire_assets::AssetId>,
    spec: &Path,
    sources: &mut Vec<Source>,
) -> Result<(), BenchError> {
    let groups = figure.material_groups()?;
    let in_dir = |name: String| format!("{}/{name}", figure.dir);
    let stem = |group: &MaterialGroup, suffix: &str| in_dir(format!("{}{suffix}", group.stem));
    let id = |path: &str| asset_id(PART, path);
    let max_size = map_cap(figure);
    let texture = |class| ImportSettings::Texture(TextureSettings { class, max_size });
    let color = texture(TextureClass::Color(ColorSettings::default()));
    let matcap = in_dir(format!("{}_matcap.png", figure.name));

    let mesh = MeshSettings {
        materials: groups
            .iter()
            .map(|group| (group.name.clone(), id(&stem(group, ".material.ron"))))
            .collect(),
        lods: entry.lods.clone().unwrap_or_default(),
        skeleton,
        ..MeshSettings::default()
    };
    let mut masks: Vec<Vec<(f32, String)>> = vec![Vec::new(); groups.len()];
    for file in files {
        let importer = match file.kind {
            FileKind::Model if figure.character.is_some() => {
                Some(ImportSettings::Vrm(VrmSettings { mesh: mesh.clone(), version: Some(VrmVersion::V1) }))
            }
            FileKind::Model => Some(ImportSettings::Gltf(mesh.clone())),
            FileKind::Palette(_) => Some(color.clone()),
            // The matcap is the warrior's small painted one, whatever the figure's maps.
            FileKind::Matcap => Some(ImportSettings::Texture(TextureSettings::from(TextureClass::Color(
                ColorSettings::default(),
            )))),
            FileKind::Layers(_) => Some(texture(TextureClass::Mask(MaskSettings {
                channels: MaskChannels::Four,
                ..MaskSettings::default()
            }))),
            // The control map and the face's masks are documents' inputs, not textures of their own.
            FileKind::Control(_) => None,
            FileKind::FaceMask(group, angle) => {
                let name = file.path.rsplit_once('/').map_or(file.path.as_str(), |(_, name)| name);
                masks[group].push((angle, name.to_owned()));
                None
            }
        };
        sources.push(match importer {
            Some(importer) => Source::asset(file.path, file.bytes, importer),
            None => Source::input(file.path, file.bytes),
        });
    }

    for (index, group) in groups.iter().enumerate() {
        let file_name = |suffix: &str| format!("{}{suffix}", group.stem);
        let regions: Vec<&str> =
            group.regions.iter().map(|&region| figure.regions[region].name.as_str()).collect();
        sources.push(Source::asset(
            stem(group, ".ilm.ron"),
            ilm_document(&file_name("_control.png")),
            ImportSettings::CharacterMap(MapKind::Ilm.default_settings(), max_size),
        ));
        sources.push(Source::asset(
            stem(group, ".ramp.ron"),
            ramp_document(&regions),
            ImportSettings::CharacterMap(MapKind::Ramp.default_settings(), MaxSize::NONE),
        ));
        let surface = match group.surface {
            Surface::Body => CharacterSurface::Body,
            Surface::Hair => CharacterSurface::Hair(HairShading::default()),
            Surface::Face => {
                sources.push(Source::asset(
                    stem(group, ".facesdf.ron"),
                    face_sdf_document(&masks[index]),
                    ImportSettings::CharacterMap(MapKind::FaceSdf.default_settings(), max_size),
                ));
                CharacterSurface::Face(FaceShading::new(id(&stem(group, ".facesdf.ron"))))
            }
        };
        let body = group.surface == Surface::Body;
        let material = AnimeCharacterMaterial {
            surface,
            base_color: [1.0; 4],
            base_color_map: Some(id(&stem(group, "_palette.png"))),
            control_map: Some(id(&stem(group, ".ilm.ron"))),
            diffuse: DiffuseMode::Ramp(RampDiffuse { map: id(&stem(group, ".ramp.ron")), mix: 1.0 }),
            regions: vec![CharacterRegion::default(); group.regions.len()],
            // Only a body takes the layers and the matcap: a face's and hair's shading reads masks of
            // their own there, which these figures don't paint.
            matcap: body.then(|| Matcap { map: id(&matcap), strength: 1.0, shadow_strength: 0.55 }),
            layer_mask: body.then(|| id(&stem(group, "_layers.png"))),
            double_sided: group.double_sided,
            ..AnimeCharacterMaterial::default()
        };
        material.validate().map_err(|error| BenchError::Invalid {
            path: spec.to_owned(),
            why: format!("material {:?}: {error:#}", group.name),
        })?;
        sources.push(Source::asset(
            stem(group, ".material.ron"),
            document_bytes(&material),
            ImportSettings::Material(MaterialSettings::default()),
        ));
    }
    Ok(())
}

/// The ILM document that packs a figure's control map, channel for channel, as a figure's own does.
fn ilm_document(control: &str) -> Vec<u8> {
    let channel = |name: &str, channel: &str| {
        format!("        {name}: Image(path: {control:?}, channel: {channel}),\n")
    };
    let mut text =
        String::from("(\n    type: \"sapphire::pipeline::IlmMap\",\n    version: 1,\n    data: (\n");
    text += &channel("specular_mask", "R");
    text += &channel("shadow_bias", "G");
    text += &channel("specular_threshold", "B");
    text += &channel("material_id", "A");
    text += "    ),\n)\n";
    text.into_bytes()
}

/// A ramp document with a row for each of a material's regions.
fn ramp_document(regions: &[&str]) -> Vec<u8> {
    let stops = |stops: &[(f32, &str)]| {
        let stops: Vec<String> =
            stops.iter().map(|(at, color)| format!("(at: {at:?}, color: {color:?})")).collect();
        format!("(stops: [{}])", stops.join(", "))
    };
    let mut text = String::from(
        "(\n    type: \"sapphire::pipeline::RampMap\",\n    version: 1,\n    data: (\n        materials: [\n",
    );
    for region in regions {
        text += &format!(
            "            (name: {region:?}, day: {}, night: Some({})),\n",
            stops(&RAMP_DAY),
            stops(&RAMP_NIGHT)
        );
    }
    text += "        ],\n    ),\n)\n";
    text.into_bytes()
}

/// A face SDF document over a face material's threshold masks, by angle.
fn face_sdf_document(masks: &[(f32, String)]) -> Vec<u8> {
    let mut text = String::from(
        "(\n    type: \"sapphire::pipeline::FaceSdfMap\",\n    version: 1,\n    data: (\n        horizontal: [\n",
    );
    for (angle, path) in masks {
        text += &format!("            (angle: {angle:?}, path: {path:?}),\n");
    }
    text += "        ],\n    ),\n)\n";
    text.into_bytes()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::character_maps::face_sdf::FaceSdfMap;
    use crate::character_maps::ilm::IlmMap;
    use crate::character_maps::ramp::RampMap;
    use crate::figure::face;

    #[test]
    fn the_documents_read() {
        let text = String::from_utf8(ilm_document("figure_control.png")).unwrap();
        sapphire_serialize::from_ron::<IlmMap>(&text).unwrap();
        let text = String::from_utf8(ramp_document(&["skin", "cloth"])).unwrap();
        assert_eq!(sapphire_serialize::from_ron::<RampMap>(&text).unwrap().materials.len(), 2);
        let masks: Vec<(f32, String)> =
            face::ANGLES.iter().map(|&angle| (angle, format!("f{}", face::mask_suffix(angle)))).collect();
        let text = String::from_utf8(face_sdf_document(&masks)).unwrap();
        assert_eq!(
            sapphire_serialize::from_ron::<FaceSdfMap>(&text).unwrap().horizontal.len(),
            face::ANGLES.len()
        );
    }

    #[test]
    fn copies_have_ids_of_their_own() {
        assert_ne!(asset_id(PART, "a_00/a.glb"), asset_id(PART, "a_01/a.glb"));
    }

    fn cast_dir() -> std::path::PathBuf {
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../bench/cast")
    }

    fn member(preset: Preset, name: &str) -> CastMember {
        let members = members(&cast_dir(), preset).unwrap();
        members.into_iter().find(|member| member.figure.name == name).unwrap_or_else(|| panic!("no {name}"))
    }

    /// The enemy stands on the Lancer's skeleton, joint for joint, so the Lancer's clips play on it.
    #[test]
    fn the_enemy_has_the_lancer_s_skeleton() {
        let lancer = figure::read(&cast_dir().join("../../tools/figures/lancer.figure.ron")).unwrap();
        for preset in [Preset::Ci, Preset::Budget] {
            assert_eq!(member(preset, "enemy").figure.joints, lancer.joints, "{preset}");
        }
    }

    /// Every ambient figure but the first shares its skeleton: the same joints, and its model's
    /// sidecar names the first's model.
    #[test]
    fn the_ambient_figures_share_a_skeleton() {
        let owner = member(Preset::Budget, "ambient_a");
        let id = asset_id(PART, "ambient_a_00/ambient_a.glb");
        assert_eq!(owner.skeleton, None);
        let members = members(&cast_dir(), Preset::Budget).unwrap();
        let sharing: Vec<&CastMember> = members
            .iter()
            .filter(|member| member.figure.name.starts_with("ambient_") && member.skeleton.is_some())
            .collect();
        assert_eq!(sharing.len(), 5);
        for member in sharing {
            assert_eq!(member.skeleton, Some(id), "{}", member.figure.name);
            assert_eq!(member.figure.joints, owner.figure.joints, "{}", member.figure.name);
        }
        let sources = generate(&cast_dir(), Preset::Ci).unwrap();
        let model = sources
            .iter()
            .find(|source| source.path == "ambient_b_00/ambient_b.glb")
            .expect("ci's second ambient");
        let Some((ImportSettings::Gltf(mesh), true)) = &model.sidecar else { panic!("{:?}", model.sidecar) };
        assert_eq!(mesh.skeleton, Some(id));
    }

    /// The hero is a VRM 1.0 file whose rig imports with its humanoid, the 18 expression presets bound
    /// to the face's morph targets, and a spring for every chain copy.
    #[test]
    fn the_hero_is_a_vrm_whose_rig_imports() {
        use sapphire_assets::rig::ExpressionPreset;
        let hero = member(Preset::Ci, "hero");
        let files = figure::figure_files(&hero.copy(0)).unwrap();
        assert_eq!(files[0].path, "hero_00/hero.vrm");
        let rig = crate::import::vrm::import_rig(&files[0].bytes, Some(VrmVersion::V1)).unwrap();
        assert_eq!(rig.joint_count, 200);
        assert_eq!(rig.material_slot_count, 6);
        assert_eq!(rig.morph_target_count, 18);
        assert!(!rig.humanoid.is_empty());
        let presets: Vec<ExpressionPreset> =
            rig.expressions.iter().filter_map(|expression| expression.preset).collect();
        assert_eq!(presets, ExpressionPreset::ALL);
        assert!(rig.expressions.iter().all(|expression| expression.morph_targets.len() == 1));
        let springs: Vec<&str> = rig.springs.iter().map(|spring| spring.name.as_str()).collect();
        assert_eq!(springs.len(), 12 + 8 + 5 + 4, "{springs:?}");
        assert!(
            springs.contains(&"skirt_00")
                && springs.contains(&"hair_back_07")
                && springs.contains(&"ribbon_r")
        );
        assert_eq!(rig.springs.iter().map(|spring| spring.joints.len()).sum::<usize>(), 135);
        // A face material's threshold masks, and a face SDF document over them.
        let masks: Vec<image::GrayImage> = files
            .iter()
            .filter(|file| matches!(file.kind, FileKind::FaceMask(..)))
            .map(|file| image::load_from_memory(&file.bytes).unwrap().to_luma8())
            .collect();
        assert_eq!(masks.len(), face::ANGLES.len());
        // Each angle's lit texels are within the one before's: the shape a threshold map holds.
        let lit = |mask: &image::GrayImage| mask.pixels().filter(|texel| texel.0[0] > 127).count();
        assert!(
            lit(&masks[0]) > 0 && lit(&masks[12]) < lit(&masks[0]) / 4,
            "{} {}",
            lit(&masks[0]),
            lit(&masks[12])
        );
        for pair in masks.windows(2) {
            assert!(pair[1].pixels().zip(pair[0].pixels()).all(|(after, before)| after.0[0] <= before.0[0]));
        }
    }

    /// What a cook makes of a model: LOD0's triangles, joints, material slots and morph targets.
    #[derive(Debug, PartialEq)]
    struct Cooked {
        triangles: u32,
        joints: u32,
        slots: usize,
        morphs: usize,
    }

    fn cooked(path: &Path, lods: Option<LodSettings>, cook: crate::cook::Cook) -> Cooked {
        let settings = MeshSettings { lods: lods.unwrap_or_default(), ..MeshSettings::default() };
        let mesh = crate::cook::cook_mesh(path, &settings, cook).unwrap();
        Cooked {
            triangles: mesh.lods[0].submeshes.iter().map(|submesh| submesh.index_count / 3).sum(),
            joints: mesh.skin.as_ref().map_or(0, |skin| skin.joint_count),
            slots: mesh.materials.len(),
            morphs: mesh.morph_targets.len(),
        }
    }

    /// A PNG's width and height, from its header.
    fn png_size(bytes: &[u8]) -> (u32, u32) {
        let word = |at: usize| u32::from_be_bytes(bytes[at..at + 4].try_into().unwrap());
        (word(16), word(20))
    }

    /// Every figure of the `budget` cast cooks at spec §3.4's row: the hero exactly on the desktop
    /// (80,000 triangles, 200 joints, 6 materials, 2K maps, the 18 expressions), and its mobile LOD0 within
    /// 30,000; an enemy at 8,000 on 24 joints; an ambient figure within 3,200 on the same 24, one material.
    /// What the mobile cook can't yet meet (joints, materials) is held as it is, and named in the
    /// spec's §3.4.
    #[test]
    fn every_budget_figure_cooks_at_its_row() {
        use crate::cook::Cook;
        let dir = crate::test_util::TempDir::new("cast_budget_rows");
        for member in members(&cast_dir(), Preset::Budget).unwrap() {
            let figure = member.copy(0);
            let files = figure::figure_files(&figure).unwrap();
            let model = &files[0];
            let path = dir.path().join(model.path.rsplit_once('/').unwrap().1);
            std::fs::write(&path, &model.bytes).unwrap();
            let maps: Vec<(u32, u32)> = files
                .iter()
                .filter(|file| matches!(file.kind, FileKind::Palette(_)))
                .map(|file| png_size(&file.bytes))
                .collect();
            let (desktop, mobile) = (
                cooked(&path, member.entry.lods.clone(), Cook::Desktop),
                cooked(&path, member.entry.lods.clone(), Cook::Mobile),
            );
            let name = figure.name.as_str();
            let row = format!("{name}: desktop {desktop:?}, mobile {mobile:?}, maps {maps:?}");
            eprintln!("{row}");
            match name {
                "hero" => {
                    assert_eq!(
                        desktop,
                        Cooked { triangles: 80_000, joints: 200, slots: 6, morphs: 18 },
                        "{row}"
                    );
                    assert!((29_000..=30_000).contains(&mobile.triangles), "{row}");
                    assert_eq!((mobile.joints, mobile.slots, mobile.morphs), (200, 6, 18), "{row}");
                    assert_eq!(maps, [(2048, 2048); 6], "{row}");
                }
                "enemy" => {
                    assert_eq!(
                        desktop,
                        Cooked { triangles: 8_000, joints: 24, slots: 1, morphs: 0 },
                        "{row}"
                    );
                    assert!(mobile.triangles < desktop.triangles && mobile.joints == 24, "{row}");
                    assert_eq!(maps, [(1024, 1024)], "{row}");
                }
                _ => {
                    assert!(name.starts_with("ambient_"), "{row}");
                    assert!(member.skeleton.is_some() || name == "ambient_a", "{row}");
                    assert!((2_500..=3_200).contains(&desktop.triangles), "{row}");
                    assert_eq!((desktop.joints, desktop.slots, mobile.joints), (24, 1, 24), "{row}");
                    assert_eq!(maps, [(512, 512)], "{row}");
                }
            }
        }
    }

    /// Every map of the `budget` cast cooks within spec §3.4's part budget on each cook (M5 M1): the hero's
    /// 2K maps carry the mobile cook's 1K cap in their sidecars, and a map within both carries none.
    #[test]
    fn every_budget_map_cooks_within_the_part_budget() {
        use crate::character_maps::MapSettings;
        use crate::cook::Cook;
        let mut capped = 0;
        for member in members(&cast_dir(), Preset::Budget).unwrap() {
            let figure = member.copy(0);
            let files = figure::figure_files(&figure).unwrap();
            let mut sources = Vec::new();
            figure_sources(&figure, files, &member.entry, member.skeleton, &member.spec, &mut sources)
                .unwrap();
            let size =
                |path: &str| png_size(&sources.iter().find(|source| source.path == path).unwrap().bytes);
            for source in &sources {
                // A ramp is 256 × N and uncompressed; a face SDF is a quarter of its figure's maps a side.
                let (settings, (width, height)) = match &source.sidecar {
                    Some((ImportSettings::Texture(settings), _)) => (*settings, png_size(&source.bytes)),
                    Some((ImportSettings::CharacterMap(MapSettings::Ilm(ilm), max_size), _)) => {
                        // An ILM is its control map's size.
                        let control = size(&source.path.replace(".ilm.ron", "_control.png"));
                        (TextureSettings { class: TextureClass::Ilm(*ilm), max_size: *max_size }, control)
                    }
                    _ => continue,
                };
                for (cook, budget) in [(Cook::Desktop, PART_MAP.desktop), (Cook::Mobile, PART_MAP.mobile)] {
                    let (w, h) = crate::texture::cooked_size(&settings, cook, width, height).unwrap();
                    assert!(w.max(h) <= budget.unwrap().get(), "{} ({cook}): {w} × {h}", source.path);
                }
                let within = width.max(height) <= PART_MAP.mobile.unwrap().get();
                assert_eq!(settings.max_size.is_none(), within, "{}: {width} × {height}", source.path);
                capped += usize::from(!within);
            }
        }
        // The hero's six palettes and six ILMs, and its four body materials' layer masks.
        assert_eq!(capped, 6 + 6 + 4, "capped maps");
    }

    /// The look-dev character, measured against the hero's row as spec §3.4 reports it: every level's
    /// triangles in each cook, its joints, material slots, morph targets and springs, by its own sidecar.
    #[test]
    fn the_look_dev_character_measured_against_the_hero_row() {
        use crate::cook::{Cook, cook_mesh};
        let path = cast_dir().join("../../content/lookdev/character/character.vrm");
        let meta = crate::meta::read(&crate::meta::sidecar_path(&path)).unwrap();
        let settings = meta.importer.mesh().expect("a mesh's sidecar").clone();
        let mut measured = Vec::new();
        for cook in Cook::ALL {
            let mesh = cook_mesh(&path, &settings, cook).unwrap();
            let levels: Vec<u32> = mesh
                .lods
                .iter()
                .map(|lod| lod.submeshes.iter().map(|submesh| submesh.index_count / 3).sum())
                .collect();
            let joints = mesh.skin.as_ref().map_or(0, |skin| skin.joint_count);
            measured.push((cook.to_string(), levels, joints, mesh.materials.len(), mesh.morph_targets.len()));
        }
        let rig =
            crate::import::vrm::import_rig(&std::fs::read(&path).unwrap(), Some(VrmVersion::V1)).unwrap();
        let springs =
            (rig.springs.len(), rig.springs.iter().map(|spring| spring.joints.len()).sum::<usize>());
        let found = format!("{measured:?} springs {springs:?} expressions {}", rig.expressions.len());
        // Within the hero's 80,000 and 30,000 triangles and 200 joints; over its 6 and 4 materials. A
        // simplified level's count is this Mac's (22,216, 13,747 and 9,953 on 2026-11-13), so only LOD0's is
        // held exactly.
        let (desktop, mobile) = (&measured[0], &measured[1]);
        assert_eq!((desktop.1[0], desktop.2, desktop.3, desktop.4), (36_470, 155, 13, 66), "{found}");
        assert_eq!(mobile.1[..], desktop.1[1..], "{found}");
        assert!(mobile.1[0] <= 30_000 && desktop.1.windows(2).all(|pair| pair[1] < pair[0]), "{found}");
        assert_eq!((springs, rig.expressions.len()), ((22, 79), 18), "{found}");
    }
}
