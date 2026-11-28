use std::num::NonZeroU32;

use sapphire_assets::effect_sheet::SheetTiming;
use sapphire_assets::material::AnimeCharacterMaterial;
use serde_json::json;

use super::*;
use crate::cook::{CookOptions, CookReport, CookStatus, cook_dir};
use crate::effect_sheet::EffectSheetSettings;
use crate::effects::EffectsSettings;
use crate::gltf_writer::{Geometry, GltfWriter, InfluenceSet};
use crate::import::vrm::{VrmSettings, VrmVersion, samples};
use crate::meta::{self, AssetMeta, MeshSettings};
use crate::test_util::TempDir;
use crate::texture::{ColorSettings, MaxSize, TextureClass};

/// A source tree being written.
struct Tree(TempDir);

impl Tree {
    fn new(name: &str) -> Self {
        Self(TempDir::new(&format!("check_{name}")))
    }

    fn root(&self) -> &Path {
        self.0.path()
    }

    /// Writes `path` (under the root) with a sidecar holding `importer` and `budget`; returns its ID.
    fn source(&self, path: &str, bytes: &[u8], importer: ImportSettings, budget: Option<Budget>) -> AssetId {
        let id = meta::new_id().unwrap();
        let path = self.root().join(path);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, bytes).unwrap();
        let sidecar = AssetMeta { budget, ..AssetMeta::new(id, importer) };
        fs::write(meta::sidecar_path(&path), sapphire_serialize::to_ron(&sidecar).unwrap()).unwrap();
        id
    }

    /// A PNG texture of one colour, `size` a side.
    fn texture(&self, path: &str, size: u32, max_size: MaxSize, budget: Option<Budget>) -> AssetId {
        let png = samples::png(size, |_, _| [200, 120, 80, 255]);
        let settings = TextureSettings { class: TextureClass::Color(ColorSettings::default()), max_size };
        self.source(path, &png, ImportSettings::Texture(settings), budget)
    }

    /// An anime character material whose base colour is `map`.
    fn material(&self, path: &str, map: AssetId) -> AssetId {
        let document = AnyMaterialDocument::AnimeCharacter(AnimeCharacterMaterial {
            base_color_map: Some(map),
            ..AnimeCharacterMaterial::default()
        });
        let importer = ImportSettings::for_source(Path::new(path)).unwrap();
        self.source(path, document.to_ron().unwrap().as_bytes(), importer, None)
    }

    /// Gives the sidecar of `path` the budget `budget`, keeping everything else.
    fn set_budget(&self, path: &str, budget: Option<Budget>) {
        let sidecar = meta::sidecar_path(&self.root().join(path));
        let meta = AssetMeta { budget, ..meta::read(&sidecar).unwrap() };
        fs::write(sidecar, sapphire_serialize::to_ron(&meta).unwrap()).unwrap();
    }

    fn check(&self) -> Report {
        check(self.root()).unwrap()
    }
}

fn role(role: Role) -> Option<Budget> {
    Some(Budget { role: Some(role), waive: Vec::new() })
}

fn waiving(rule: Rule, why: &str) -> Option<Budget> {
    Some(Budget { role: None, waive: vec![Waiver { rule, cook: None, why: why.to_owned() }] })
}

/// Each finding as (source, rule, cook, waived).
fn found(report: &Report) -> Vec<(&str, Rule, Option<Cook>, bool)> {
    report
        .findings
        .iter()
        .map(|finding| (finding.source.as_str(), finding.rule, finding.cook, finding.waived.is_some()))
        .collect()
}

/// A skinned glTF: a grid of `side` × `side` quads in the XY plane, split across `slots` materials
/// (`slot0`, `slot1`, …) by rows, every vertex on the first of a chain of `joints` joints. The chain hangs
/// under a `root` node, which the cooked skin keeps as a joint: it measures `joints + 1`.
fn figure(side: u32, slots: usize, joints: usize) -> Vec<u8> {
    let mut writer = GltfWriter::default();
    let materials: Vec<usize> =
        (0..slots).map(|slot| writer.add_material(json!({ "name": format!("slot{slot}") }))).collect();
    let rows_per_slot = side.div_ceil(slots as u32);
    let parts: Vec<Geometry> = (0..slots as u32)
        .map(|slot| {
            let rows = (slot * rows_per_slot)..((slot + 1) * rows_per_slot).min(side);
            let mut geometry = Geometry::default();
            let mut set = InfluenceSet::default();
            for y in rows.start..=rows.end {
                for x in 0..=side {
                    let at = [x as f32 / side as f32, y as f32 / side as f32];
                    geometry.positions.push([at[0], at[1], 0.0]);
                    geometry.normals.push([0.0, 0.0, 1.0]);
                    geometry.uvs.push(at);
                    set.joints.push([0; 4]);
                    set.weights.push([1.0, 0.0, 0.0, 0.0]);
                }
            }
            let width = side + 1;
            for row in 0..rows.len() as u32 {
                for x in 0..side {
                    let corner = row * width + x;
                    let quad =
                        [corner, corner + 1, corner + width, corner + 1, corner + width + 1, corner + width];
                    geometry.indices.extend(quad);
                }
            }
            geometry.influences.push(set);
            geometry
        })
        .collect();
    let primitives: Vec<(&Geometry, Option<usize>)> =
        parts.iter().zip(&materials).map(|(geometry, &material)| (geometry, Some(material))).collect();
    let mesh = writer.add_mesh("figure", &primitives);
    // The chain from its tip: each joint the parent of the one added before it.
    let mut chain: Vec<usize> = Vec::with_capacity(joints);
    for joint in (0..joints).rev() {
        let mut node = json!({ "name": format!("joint{joint}"), "translation": [0.0, 0.01, 0.0] });
        if let Some(&child) = chain.last() {
            node["children"] = json!([child]);
        }
        chain.push(writer.add_node(node));
    }
    chain.reverse();
    let skin = writer.add_skin(&chain, None);
    let body = writer.add_node(json!({ "name": "body", "mesh": mesh, "skin": skin }));
    let root = writer.add_node(json!({ "name": "root", "children": [chain[0], body] }));
    writer.add_root(root);
    writer.to_glb()
}

#[test]
fn a_figure_over_its_role_s_row_fails_each_row_by_name_on_each_cook() {
    let tree = Tree::new("rows");
    // 2 × 46² = 4,232 triangles, over the ambient row's 4,000 on desktop; 71 joints (the chain's 70 and
    // its root), over its 64; 3 slots, over its 2 and 1; and a 2048 map, over its 1024 and 512.
    let map = tree.texture("figure/figure_palette.png", 2048, MaxSize::NONE, None);
    let material = tree.material("figure/figure.material.ron", map);
    let mut settings = MeshSettings::default();
    settings.materials.insert("slot0".to_owned(), material);
    tree.source("figure/figure.glb", &figure(46, 3, 70), ImportSettings::Gltf(settings), role(Role::Ambient));

    let report = tree.check();
    let mesh = "figure/figure.glb";
    let (desktop, mobile) = (Some(Cook::Desktop), Some(Cook::Mobile));
    let errors: Vec<_> =
        report.failures().map(|finding| (finding.source.as_str(), finding.rule, finding.cook)).collect();
    assert_eq!(
        errors,
        [
            (mesh, Rule::Triangles, desktop),
            (mesh, Rule::Joints, desktop),
            (mesh, Rule::Joints, mobile),
            (mesh, Rule::Slots, desktop),
            (mesh, Rule::Slots, mobile),
            (mesh, Rule::MapSize, desktop),
            (mesh, Rule::MapSize, mobile),
            ("figure/figure_palette.png", Rule::UncappedMap, mobile),
        ],
        "{report}"
    );
    // The two slots no document is bound to draw what the importer made.
    assert_eq!(report.of(mesh, Rule::UnboundSlot).count(), 2, "{report}");
    assert!(!report.passes());

    let [desktop, mobile] = [&report.meshes[0].cooks[0], &report.meshes[0].cooks[1]];
    assert_eq!((desktop.triangles, desktop.joints, desktop.slots, desktop.map), (4_232, 71, 3, 2048));
    assert!(mobile.triangles < desktop.triangles, "the mobile cook skips LOD0: {mobile:?}");
    assert_eq!((mobile.joints, mobile.slots, mobile.map), (71, 3, 2048));
    assert_eq!(report.character_maps, 1);
}

#[test]
fn a_figure_within_its_row_passes_and_its_map_cap_counts() {
    let tree = Tree::new("within");
    // At the hero's row, a 2048 map capped to 1024 on mobile fits both cooks.
    let map = tree.texture(
        "hero/palette.png",
        2048,
        MaxSize { desktop: None, mobile: NonZeroU32::new(1024) },
        None,
    );
    let material = tree.material("hero/hero.material.ron", map);
    let mut settings = MeshSettings::default();
    settings.materials.insert("slot0".to_owned(), material);
    tree.source("hero/hero.glb", &figure(8, 1, 3), ImportSettings::Gltf(settings), role(Role::Hero));
    let report = tree.check();
    assert!(report.passes() && report.findings.is_empty(), "{report}");
    let maps: Vec<u32> = report.meshes[0].cooks.iter().map(|measured| measured.map).collect();
    assert_eq!(maps, [2048, 1024]);
}

#[test]
fn a_vrm_s_unrecognized_vroid_names_and_shared_chain_names_warn() {
    let tree = Tree::new("vroid");
    let mut vrm = samples::vrm1();
    // Named as VRoid Studio names its parts, but an eye part the importer doesn't know, and skin whose group
    // isn't written as VRoid writes it.
    vrm.document["materials"][samples::material::HAIR]["name"] = json!("EyeExtra_00_EYE");
    vrm.document["materials"][samples::material::SKIN]["name"] = json!("Body_00_skin");
    // A second chain, under the first's name.
    let springs = &mut vrm.document["extensions"]["VRMC_springBone"]["springs"];
    let mut second = springs[0].clone();
    second["joints"] = json!([{ "node": samples::node::HAIR_SIDE }]);
    springs.as_array_mut().unwrap().push(second);
    let importer =
        ImportSettings::Vrm(VrmSettings { version: Some(VrmVersion::V1), ..VrmSettings::default() });
    tree.source("character/a.vrm", &vrm.to_glb(), importer, None);

    let report = tree.check();
    let source = "character/a.vrm";
    assert_eq!(
        found(&report),
        [
            (source, Rule::VroidEyeLayer, None, false),
            (source, Rule::VroidRegionLight, None, false),
            (source, Rule::DuplicateSpringName, None, false),
        ],
        "{report}"
    );
    // Warnings, all: no role, so no row and no unbound slots.
    assert!(report.passes() && report.meshes.is_empty(), "{report}");
    assert!(report.findings.iter().all(|finding| finding.severity == Severity::Warning));
}

#[test]
fn paths_copies_misplaced_roles_and_waivers_are_found_by_name() {
    let tree = Tree::new("hygiene");
    tree.texture("maps/a.png", 4, MaxSize::NONE, None);
    // The same bytes again: once waived, once not.
    tree.texture("maps/b.png", 4, MaxSize::NONE, waiving(Rule::DuplicateTexture, "the ward's own copy"));
    tree.texture("maps/c.png", 4, MaxSize::NONE, None);
    tree.texture("maps/\u{82b1}.png", 8, MaxSize::NONE, None);
    tree.texture("maps/hero.png", 16, MaxSize::NONE, role(Role::Hero));
    // A waiver nothing needs, and one without its reason.
    tree.texture("maps/stale.png", 32, MaxSize::NONE, waiving(Rule::MapSize, "too big"));
    tree.texture("maps/unexplained.png", 32, MaxSize::NONE, waiving(Rule::DuplicateTexture, " "));

    let report = tree.check();
    assert_eq!(
        found(&report),
        [
            ("maps/b.png", Rule::DuplicateTexture, None, true),
            ("maps/c.png", Rule::DuplicateTexture, None, false),
            ("maps/hero.png", Rule::MisplacedRole, None, false),
            ("maps/stale.png", Rule::StaleWaiver, None, false),
            ("maps/unexplained.png", Rule::DuplicateTexture, None, true),
            ("maps/unexplained.png", Rule::UnexplainedWaiver, None, false),
            ("maps/\u{82b1}.png", Rule::NonAsciiPath, None, false),
        ],
        "{report}"
    );
    let failing: Vec<Rule> = report.failures().map(|finding| finding.rule).collect();
    assert_eq!(
        failing,
        [Rule::MisplacedRole, Rule::StaleWaiver, Rule::UnexplainedWaiver, Rule::NonAsciiPath]
    );
}

#[test]
fn a_waived_row_passes_and_a_cook_lists_the_waiver() {
    let tree = Tree::new("waived");
    let budget = Budget {
        role: Some(Role::Ambient),
        waive: vec![Waiver { rule: Rule::Joints, cook: None, why: "a test of seventy".to_owned() }],
    };
    tree.source("a/a.glb", &figure(4, 1, 70), ImportSettings::Gltf(MeshSettings::default()), Some(budget));
    let report = tree.check();
    let waived: Vec<_> =
        report.findings.iter().filter(|finding| finding.waived.is_some()).map(|f| (f.rule, f.cook)).collect();
    assert_eq!(waived, [(Rule::Joints, Some(Cook::Desktop)), (Rule::Joints, Some(Cook::Mobile))], "{report}");
    assert!(report.passes(), "{report}");
    assert!(report.to_string().contains(
        "waived[joints] a/a.glb (mobile): 71 joints, over the ambient row's 64\n  waived: a test of seventy"
    ));

    let out = TempDir::new("check_waived_out");
    let options = CookOptions { cook: Cook::Desktop, cache: None, locked: true, editor: false };
    let cooked = cook_dir(tree.root(), out.path(), &options).unwrap();
    assert_eq!(
        cooked.waivers,
        [Waived {
            source: tree.root().join("a/a.glb"),
            rule: Rule::Joints,
            why: "a test of seventy".to_owned()
        }]
    );
}

/// A waiver that names a cook waives that cook's findings alone: the phones' waiver of a figure's joints leaves
/// its desktop joints failing, and only the mobile cook lists it. One that finds nothing on its cook is stale,
/// and so is one that names a cook for a rule that depends on none.
#[test]
fn a_mobile_waiver_leaves_the_desktop_finding_standing() {
    let tree = Tree::new("waived_by_cook");
    let on = |rule, cook| Waiver { rule, cook: Some(cook), why: "the phones' cut".to_owned() };
    let budget = Budget {
        role: Some(Role::Ambient),
        waive: vec![on(Rule::Joints, Cook::Mobile), on(Rule::Triangles, Cook::Desktop)],
    };
    tree.source("a/a.glb", &figure(4, 1, 70), ImportSettings::Gltf(MeshSettings::default()), Some(budget));
    tree.texture("maps/a.png", 4, MaxSize::NONE, None);
    let copy = Some(Budget { role: None, waive: vec![on(Rule::DuplicateTexture, Cook::Mobile)] });
    tree.texture("maps/b.png", 4, MaxSize::NONE, copy);

    let report = tree.check();
    let (desktop, mobile) = (Some(Cook::Desktop), Some(Cook::Mobile));
    let joints: Vec<_> = report.of("a/a.glb", Rule::Joints).map(|f| (f.cook, f.waived.is_some())).collect();
    assert_eq!(joints, [(desktop, false), (mobile, true)], "{report}");
    let failing: Vec<_> =
        report.failures().map(|finding| (finding.source.as_str(), finding.rule, finding.cook)).collect();
    assert_eq!(
        failing,
        [
            ("a/a.glb", Rule::Joints, desktop),
            ("a/a.glb", Rule::StaleWaiver, desktop),
            ("maps/b.png", Rule::StaleWaiver, mobile),
        ],
        "{report}"
    );
    let stale = |source| report.of(source, Rule::StaleWaiver).next().unwrap().message.clone();
    assert_eq!(
        stale("a/a.glb"),
        "waives `triangles` on the desktop cook, which finds nothing there: remove the waiver"
    );
    assert!(stale("maps/b.png").contains("finds this on no cook in particular"), "{report}");
    assert_eq!(report.of("maps/b.png", Rule::DuplicateTexture).next().unwrap().waived, None);

    let listed = |cook| {
        let out = TempDir::new(&format!("check_waived_by_cook_{cook}"));
        let options = CookOptions { cook, cache: None, locked: true, editor: false };
        let report = cook_dir(tree.root(), out.path(), &options).unwrap();
        report.waivers.iter().map(|waived| (waived.rule, waived.source.clone())).collect::<Vec<_>>()
    };
    let (glb, png) = (tree.root().join("a/a.glb"), tree.root().join("maps/b.png"));
    assert_eq!(listed(Cook::Desktop), [(Rule::Triangles, glb.clone())]);
    assert_eq!(listed(Cook::Mobile), [(Rule::Joints, glb), (Rule::DuplicateTexture, png)]);

    // Written only when set, as `cook: Some(Mobile)`, and read back.
    let meta = AssetMeta {
        budget: Some(Budget { role: None, waive: vec![on(Rule::Joints, Cook::Mobile)] }),
        ..AssetMeta::new(meta::new_id().unwrap(), ImportSettings::Gltf(MeshSettings::default()))
    };
    let text = sapphire_serialize::to_ron(&meta).unwrap();
    assert!(text.contains("cook: Some(Mobile)"), "{text}");
    assert_eq!(sapphire_serialize::from_ron::<AssetMeta>(&text).unwrap(), meta);
}

/// A sidecar's `budget` is the check's alone: it's written only when set, reads back as it was written, and
/// changes no cook key and no cooked byte, so committed sidecars key as they did before it existed.
#[test]
fn a_budget_changes_no_key_and_no_cooked_byte() {
    let id = meta::new_id().unwrap();
    let plain = AssetMeta::new(id, ImportSettings::Gltf(MeshSettings::default()));
    assert!(!sapphire_serialize::to_ron(&plain).unwrap().contains("budget"));
    let budget = Budget {
        role: Some(Role::Enemy),
        waive: vec![Waiver { rule: Rule::Slots, cook: None, why: "a reason".to_owned() }],
    };
    let with = AssetMeta { budget: Some(budget), ..plain.clone() };
    let text = sapphire_serialize::to_ron(&with).unwrap();
    assert!(text.contains("budget: Some((\n            role: Some(Enemy),"), "{text}");
    assert_eq!(sapphire_serialize::from_ron::<AssetMeta>(&text).unwrap(), with);

    // A tree cooked, then given budgets on a mesh, a material and a map: the second cook, on the same
    // cache, keys every asset as the first did and cooks nothing.
    let tree = Tree::new("keys");
    let map = tree.texture("a/palette.png", 64, MaxSize::NONE, None);
    let material = tree.material("a/a.material.ron", map);
    let mut settings = MeshSettings::default();
    settings.materials.insert("slot0".to_owned(), material);
    tree.source("a/a.glb", &figure(4, 1, 3), ImportSettings::Gltf(settings), None);
    let (out, cache) = (TempDir::new("check_keys_out"), TempDir::new("check_keys_cache"));
    let cook = || {
        let options =
            CookOptions { cook: Cook::Desktop, cache: Some(cache.path()), locked: true, editor: false };
        cook_dir(tree.root(), out.path(), &options).unwrap()
    };
    let keys = |report: &CookReport| -> Vec<_> {
        report.assets.iter().map(|asset| (asset.output.clone(), asset.key, asset.output_hash)).collect()
    };
    let first = cook();
    let reason = |rule| {
        Some(Budget { role: None, waive: vec![Waiver { rule, cook: None, why: "a reason".to_owned() }] })
    };
    tree.set_budget("a/a.glb", Some(Budget { role: Some(Role::Hero), waive: Vec::new() }));
    tree.set_budget("a/a.material.ron", reason(Rule::StaleWaiver));
    tree.set_budget("a/palette.png", reason(Rule::DuplicateTexture));
    let second = cook();
    assert_eq!(keys(&second), keys(&first));
    assert!(second.assets.iter().all(|asset| asset.status == CookStatus::UpToDate), "{:?}", second.assets);
    assert_eq!(second.waivers.len(), 2);
}

/// A sheet's document and painting, `columns` × `rows` cells of `frames`, its painting `side` a side.
fn sheet(tree: &Tree, name: &str, (columns, rows, frames): (u32, u32, u32), side: u32) -> AssetId {
    let image = format!("{name}.sheet.png");
    fs::create_dir_all(tree.root().join("fx")).unwrap();
    fs::write(tree.root().join("fx").join(&image), samples::png(side, |_, _| [9, 9, 9, 255])).unwrap();
    let document = EffectSheetDocument { image, columns, rows, frames, timing: SheetTiming::Life };
    let ron = sapphire_serialize::to_ron(&document).unwrap();
    let importer = ImportSettings::EffectSheet(EffectSheetSettings::default());
    tree.source(&format!("fx/{name}.sheet.ron"), ron.as_bytes(), importer, None)
}

/// An effects library whose emitters each name one of `sheets`.
fn library(tree: &Tree, path: &str, sheets: &[AssetId]) {
    let emitters: Vec<String> = (sheets.iter().enumerate())
        .map(|(i, sheet)| {
            format!(
                "(name: \"e{i}\", spawn: Burst(count: 1), lifetime: (1.0, 1.0), size: [(0.0, 0.1)], \
                 color: [(0.0, (1.0, 1.0, 1.0))], sheet: Some(\"{sheet}\"))"
            )
        })
        .collect();
    let ron = format!(
        "(type: \"sapphire::vfx::EffectLibrary\", version: 5, data: (effects: [(name: \"fx\", emitters: [{}])]))",
        emitters.join(", ")
    );
    tree.source(path, ron.as_bytes(), ImportSettings::Effects(EffectsSettings::default()), None);
}

/// A sheet's wrong field and wrong painting are each named, an emitter naming what's no sheet is found on its
/// library, and a library whose sheets pass the desktops' atlas but not the phones' is over on the mobile
/// cook alone.
#[test]
fn sheets_are_checked_by_field_and_libraries_by_their_atlas() {
    let tree = Tree::new("sheets");
    let good = sheet(&tree, "good", (2, 2, 4), 512);
    let bad = sheet(&tree, "bad", (2, 2, 9), 512);
    sheet(&tree, "small", (2, 2, 4), 256);
    let many = sheet(&tree, "many", (8, 8, 64), 2048);
    let more = sheet(&tree, "more", (8, 8, 64), 2048);
    let map = tree.texture("maps/a.png", 4, MaxSize::NONE, None);
    library(&tree, "fx/fine.effects.ron", &[good, bad]);
    library(&tree, "fx/stray.effects.ron", &[good, map]);
    library(&tree, "fx/full.effects.ron", &[many, more, good]);
    let importer = ImportSettings::Effects(EffectsSettings::default());
    tree.source(
        "fx/broken.effects.ron",
        b"(type: \"sapphire::vfx::EffectLibrary\", version: 5, data: (",
        importer,
        None,
    );

    let report = tree.check();
    assert_eq!(
        found(&report),
        [
            ("fx/bad.sheet.ron", Rule::Sheet, None, false),
            ("fx/broken.effects.ron", Rule::Sheet, None, false),
            ("fx/full.effects.ron", Rule::AtlasBudget, Some(Cook::Mobile), false),
            ("fx/small.sheet.ron", Rule::Sheet, None, false),
            ("fx/stray.effects.ron", Rule::Sheet, None, false),
        ],
        "{report}"
    );
    let message = |source: &str, rule: Rule| report.of(source, rule).next().unwrap().message.clone();
    assert!(message("fx/bad.sheet.ron", Rule::Sheet).starts_with("`frames`"), "{report}");
    assert!(message("fx/small.sheet.ron", Rule::Sheet).contains("256 × 256"), "{report}");
    assert!(
        message("fx/stray.effects.ron", Rule::Sheet).contains(&format!("`fx/e1` names {map}")),
        "{report}"
    );
    assert!(message("fx/full.effects.ron", Rule::AtlasBudget).contains("132 frames"), "{report}");
    assert!(message("fx/broken.effects.ron", Rule::Sheet).contains("doesn't read"), "{report}");
}

/// A static glTF plant: a unit square of two triangles, with no skin.
fn plant() -> Vec<u8> {
    let mut writer = GltfWriter::default();
    let square = Geometry {
        positions: vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [1.0, 1.0, 0.0], [0.0, 1.0, 0.0]],
        indices: vec![0, 1, 2, 0, 2, 3],
        ..Geometry::default()
    };
    let mesh = writer.add_mesh("Plant", &[(&square, None)]);
    let node = writer.add_node(json!({ "mesh": mesh }));
    writer.add_root(node);
    writer.to_glb()
}

/// A foliage document (M6 G1a) that doesn't read, or names what isn't a static mesh (a texture, a skinned
/// mesh), is refused by its field as the cook refuses it, and one that's right passes.
#[test]
fn a_malformed_foliage_document_is_refused_by_its_field() {
    let tree = Tree::new("foliage");
    let gltf = || ImportSettings::Gltf(MeshSettings::default());
    let meshes: Vec<AssetId> =
        (0..3).map(|n| tree.source(&format!("plants/{n}.glb"), &plant(), gltf(), None)).collect();
    let skinned = tree.source("plants/rigged.glb", &figure(2, 1, 1), gltf(), None);
    let map = tree.texture("maps/leaf.png", 4, MaxSize::NONE, None);
    let grove = include_str!("../../../../crates/sapphire_zone/testdata/grove.foliage.ron");
    let ids = [
        "2b0f0d43-6d4e-4b59-9b1f-5f3c6f0b8e21",
        "8f5a7c1e-0c3d-4e1b-a2f6-3d9e4b7c5a10",
        "4d2c6b8a-1e3f-4a5b-9c7d-0e1f2a3b4c5d",
    ];
    let grove = ids
        .into_iter()
        .zip(&meshes)
        .fold(grove.to_owned(), |text, (from, to)| text.replace(from, &to.to_string()));
    let importer = || ImportSettings::Foliage(foliage::FoliageDocumentSettings::default());
    tree.source("wood/grove.foliage.ron", grove.as_bytes(), importer(), None);
    let leafy = grove.replace(&meshes[2].to_string(), &map.to_string());
    tree.source("wood/leafy.foliage.ron", leafy.as_bytes(), importer(), None);
    let rigged = grove.replace(&meshes[1].to_string(), &skinned.to_string());
    tree.source("wood/rigged.foliage.ron", rigged.as_bytes(), importer(), None);
    let steep = grove.replace("slope_degrees: (0.0, 30.0)", "slope_degrees: (30.0, 0.0)");
    tree.source("wood/steep.foliage.ron", steep.as_bytes(), importer(), None);

    let report = tree.check();
    let foliage: Vec<(&str, &str)> = (report.findings.iter())
        .filter(|finding| finding.rule == Rule::FoliageDocument)
        .map(|finding| (finding.source.as_str(), finding.message.as_str()))
        .collect();
    assert_eq!(foliage.len(), 3, "{report}");
    assert_eq!(foliage[0].0, "wood/leafy.foliage.ron");
    assert!(foliage[0].1.contains("kinds[2].model"), "{}", foliage[0].1);
    assert_eq!(foliage[1].0, "wood/rigged.foliage.ron");
    assert!(foliage[1].1.contains("kinds[1].model"), "{}", foliage[1].1);
    assert_eq!(foliage[2].0, "wood/steep.foliage.ron");
    assert!(foliage[2].1.contains("rules[0].slope_degrees"), "{}", foliage[2].1);
    assert!(report.failures().all(|finding| finding.rule == Rule::FoliageDocument), "{report}");
}

/// A zone's atmosphere volume (M6 V1) naming a look that's missing, isn't a day or keys `grade`, or with a
/// negative blend, is refused by its field as the cook refuses it, and a zone whose volume names a day passes.
#[test]
fn a_zone_s_bad_atmosphere_volume_is_refused_by_its_field() {
    let tree = Tree::new("zone_volumes");
    let day = |group: &str| {
        format!(r#"(type: "sapphire::world::TodTimeline", version: 1, data: (keys: [(time: 0.0, {group})]))"#)
    };
    fs::create_dir_all(tree.root().join("looks")).unwrap();
    fs::write(tree.root().join("looks/cave_day.ron"), day("exposure: 1.0")).unwrap();
    fs::write(tree.root().join("looks/graded_day.ron"), day("grade: (saturation: 1.2)")).unwrap();
    fs::write(tree.root().join("looks/notes.ron"), "(not: \"a day\")").unwrap();
    let text = |look: &str, blend: &str| {
        format!(
            r##"(type: "sapphire::zone::ZoneDocument", version: 1, data: (grid: (cell_size: 16.0), volumes: [
                (name: "grotto", position: (0.0, 1.0, 0.0), shape: Sphere(radius: 2.0), look: "{look}", blend: {blend}),
            ]))"##
        )
    };
    let importer = || ImportSettings::Zone(zone::ZoneSettings::default());
    for (name, look, blend) in [
        ("cave", "../looks/cave_day.ron", "2.0"),
        ("graded", "../looks/graded_day.ron", "2.0"),
        ("missing", "../looks/gone_day.ron", "2.0"),
        ("notes", "../looks/notes.ron", "2.0"),
        ("steep", "../looks/cave_day.ron", "-1.0"),
    ] {
        tree.source(&format!("zones/{name}.zone.ron"), text(look, blend).as_bytes(), importer(), None);
    }

    let report = tree.check();
    let zones: Vec<(&str, &str)> = (report.findings.iter())
        .filter(|finding| finding.rule == Rule::ZoneDocument)
        .map(|finding| (finding.source.as_str(), finding.message.as_str()))
        .collect();
    let expected = [
        ("zones/graded.zone.ron", "volumes[0].look", "keys `grade`"),
        ("zones/missing.zone.ron", "volumes[0].look", "can't be read"),
        ("zones/notes.zone.ron", "volumes[0].look", "doesn't read as a day"),
        ("zones/steep.zone.ron", "volumes[0].blend", ""),
    ];
    assert_eq!(zones.len(), expected.len(), "{report}");
    for (&(source, message), (want, field, says)) in zones.iter().zip(expected) {
        assert_eq!(source, want);
        assert!(message.contains(field) && message.contains(says), "{source}: {message}");
    }
    assert!(report.failures().all(|finding| finding.rule == Rule::ZoneDocument), "{report}");
}

/// A version 2 zone (M6 T8a) with an ID two entries share, or an entry with both a `turn` and a `rotation`, is
/// refused by its field as the cook refuses it; one with its IDs its own and a rotation alone passes.
#[test]
fn a_zone_s_shared_id_or_double_orientation_is_refused_by_its_field() {
    let tree = Tree::new("zone_ids");
    let text = |light: &str, turn: &str| {
        format!(
            r#"(type: "sapphire::zone::ZoneDocument", version: 2, data: (grid: (cell_size: 16.0),
                instances: [(id: "3fa91c07", mesh: "f6dcae8b-5937-4299-ba5e-e2f026fc2762", at: (0.0, 0.0, 0.0),
                    {turn}rotation: Some((0.0, 45.0, 10.0)))],
                lights: [(id: "{light}", position: (0.0, 2.0, 0.0), color: (1.0, 1.0, 1.0), intensity: 1.0,
                    range: 4.0)],
            ))"#
        )
    };
    let importer = || ImportSettings::Zone(zone::ZoneSettings::default());
    for (name, light, turn) in
        [("fine", "8b2e5d40", ""), ("shared", "3fa91c07", ""), ("twice", "8b2e5d40", "turn: 30.0, ")]
    {
        tree.source(&format!("zones/{name}.zone.ron"), text(light, turn).as_bytes(), importer(), None);
    }

    let report = tree.check();
    let zones: Vec<(&str, &str)> = (report.findings.iter())
        .filter(|finding| finding.rule == Rule::ZoneDocument)
        .map(|finding| (finding.source.as_str(), finding.message.as_str()))
        .collect();
    let expected = [
        ("zones/shared.zone.ron", "lights[0].id", "instances[0]"),
        ("zones/twice.zone.ron", "instances[0].rotation", "turn"),
    ];
    assert_eq!(zones.len(), expected.len(), "{report}");
    for (&(source, message), (want, field, says)) in zones.iter().zip(expected) {
        assert_eq!(source, want);
        assert!(message.contains(field) && message.contains(says), "{source}: {message}");
    }
}

/// A cue naming a voice group no mix of the tree has (M6 U3, fixed after its review) is refused by its field; one
/// naming a mix's group, or none, passes.
#[test]
fn a_cue_whose_group_no_mix_has_is_refused_by_its_field() {
    let tree = Tree::new("cue_groups");
    let importer = |path: &str| ImportSettings::for_source(Path::new(path)).unwrap();
    let mix = include_str!("../../../../content/audio/game.mix.ron");
    tree.source("audio/game.mix.ron", mix.as_bytes(), importer("game.mix.ron"), None);
    let cue = |group: &str| {
        format!(
            "(type: \"sapphire::assets::Cue\", version: 3, data: (play: Sound(\"5c0e0000-0000-4000-8000-0000000000a1\"), \
             bus: Sfx{group}))"
        )
    };
    tree.source(
        "audio/step.cue.ron",
        cue(", group: Some(\"footsteps\")").as_bytes(),
        importer("a.cue.ron"),
        None,
    );
    tree.source(
        "audio/stomp.cue.ron",
        cue(", group: Some(\"footstep\")").as_bytes(),
        importer("a.cue.ron"),
        None,
    );
    tree.source("audio/bell.cue.ron", cue("").as_bytes(), importer("a.cue.ron"), None);

    let report = tree.check();
    let groups: Vec<(&str, &str)> = (report.findings.iter())
        .filter(|finding| finding.rule == Rule::CueGroup)
        .map(|finding| (finding.source.as_str(), finding.message.as_str()))
        .collect();
    assert_eq!(groups.len(), 1, "{report}");
    assert_eq!(groups[0].0, "audio/stomp.cue.ron");
    assert!(groups[0].1.contains("`group`") && groups[0].1.contains("footsteps"), "{}", groups[0].1);
    assert!(report.failures().all(|finding| finding.rule == Rule::CueGroup), "{report}");
}
