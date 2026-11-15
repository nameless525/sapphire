use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use super::*;
use crate::bench::asset_id;
use crate::character_maps::MapKind;
use crate::cook::Cook;
use crate::meta::{AssetMeta, MeshSettings};
use crate::test_util::TempDir;
use crate::texture::{ColorSettings, MaskChannels, MaskSettings, TextureClass, TextureSettings};

/// The committed specs.
fn specs() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../figures")
}

/// Every file under `dir`, by its path from `root`, with its bytes.
fn tree(root: &Path, dir: &Path, files: &mut BTreeMap<String, Vec<u8>>) {
    for entry in fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            tree(root, &path, files);
        } else {
            let name = path.strip_prefix(root).unwrap().to_string_lossy().replace('\\', "/");
            files.insert(name, fs::read(&path).unwrap());
        }
    }
}

fn files_of(root: &Path) -> BTreeMap<String, Vec<u8>> {
    let mut files = BTreeMap::new();
    tree(root, root, &mut files);
    files
}

/// `figure`'s files as they ship under `content`, each map and document with a hand-written sidecar's
/// settings, and a file of something else beside them.
fn ship(content: &Path, figure: &Figure) {
    let mut sidecars: Vec<(String, ImportSettings)> = Vec::new();
    for file in figure::figure_files(figure).unwrap() {
        let path = content.join(&file.path);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, &file.bytes).unwrap();
        let color =
            || ImportSettings::Texture(TextureSettings::from(TextureClass::Color(ColorSettings::default())));
        let importer = match file.kind {
            FileKind::Model => ImportSettings::Gltf(MeshSettings::default()),
            FileKind::Palette(_) | FileKind::Matcap => color(),
            FileKind::Layers(_) => {
                ImportSettings::Texture(TextureSettings::from(TextureClass::Mask(MaskSettings {
                    channels: MaskChannels::Four,
                    ..MaskSettings::default()
                })))
            }
            FileKind::Control(_) | FileKind::FaceMask(..) => continue,
        };
        sidecars.push((file.path, importer));
    }
    for group in figure.material_groups().unwrap() {
        let ilm = format!("{}/{}.ilm.ron", figure.dir, group.stem);
        fs::write(content.join(&ilm), "an ILM document").unwrap();
        sidecars.push((ilm, ImportSettings::CharacterMap(MapKind::Ilm.default_settings(), MaxSize::NONE)));
    }
    for (path, importer) in sidecars {
        let asset = AssetMeta { id: asset_id("shipped", &path), importer, cook: true };
        let text = sapphire_serialize::to_ron(&asset).unwrap();
        fs::write(meta::sidecar_path(&content.join(path)), text).unwrap();
    }
    fs::write(content.join("readme.txt"), "not a figure's").unwrap();
}

/// LOD0's triangles in the model at `path`, cooked for the desktop.
fn triangles(path: &Path) -> u32 {
    let mesh = crate::cook::cook_mesh(path, &MeshSettings::default(), Cook::Desktop).unwrap();
    mesh.lods[0].submeshes.iter().map(|submesh| submesh.index_count / 3).sum()
}

/// One small figure made again under an override: the same file names, every ID as it shipped, the
/// triangles and maps moved, the maps over the mobile size capped there in their sidecars, and the content it
/// was linked from untouched.
#[test]
fn a_kind_made_again_keeps_its_files_names_and_ids() {
    let dir = TempDir::new("kinds_override");
    let (bench, content, out) =
        (dir.path().join("bench"), dir.path().join("content"), dir.path().join("out"));
    // The Ember Lancer's spec with room to grow: no exact count, and a budget ten times its own.
    let specs = dir.path().join("specs");
    fs::create_dir_all(&specs).unwrap();
    let text = fs::read_to_string(self::specs().join("ember_lancer.figure.ron")).unwrap();
    assert!(text.contains("max_triangles: 8000,\n        triangles: Some(8000),\n"));
    let text =
        text.replace("max_triangles: 8000,\n        triangles: Some(8000),\n", "max_triangles: 80000,\n");
    fs::write(specs.join("ember_lancer.figure.ron"), text).unwrap();
    let shipped = figure::read(&specs.join("ember_lancer.figure.ron")).unwrap();
    ship(&content, &shipped);
    fs::create_dir_all(&bench).unwrap();
    fs::write(
        bench.join(DOCUMENT),
        "(type: \"sapphire::pipeline::BenchKinds\", version: 1, data: (kinds: [
            (spec: \"ember_lancer\", presets: (ci: None, budget: Some((detail: Some(2), maps: Some(2048))))),
        ]))",
    )
    .unwrap();
    let before = files_of(&content);

    // The ci preset makes nothing again: the tree is the content.
    let done = generate(&bench, &specs, Preset::Ci, &content, &out).unwrap();
    assert!(done.figures.is_empty() && done.replaced.is_empty());
    let mut linked = files_of(&out);
    assert!(linked.remove(MARKER).is_some());
    assert_eq!(linked, before);

    let done = generate(&bench, &specs, Preset::Budget, &content, &out).unwrap();
    assert_eq!(done.figures, ["ember_lancer"]);
    let mut made = files_of(&out);
    made.remove(MARKER);
    assert_eq!(made.keys().collect::<Vec<_>>(), before.keys().collect::<Vec<_>>(), "the same names");
    assert_eq!(files_of(&content), before, "the content it links is untouched");

    let model = "combat/ember_lancer/ember_lancer.glb";
    let (was, is) = (triangles(&content.join(model)), triangles(&out.join(model)));
    assert!(was == 8000 && is > 2 * was, "{was} to {is}");
    let palette = "combat/ember_lancer/ember_lancer_palette.png";
    let size = |bytes: &[u8]| u32::from_be_bytes(bytes[16..20].try_into().unwrap());
    assert_eq!((size(&before[palette]), size(&made[palette])), (256, 2048));

    // Every sidecar keeps its ID; the maps over 1K take the mobile cook's cap, and nothing else changes.
    let capped = MaxSize { desktop: None, mobile: std::num::NonZeroU32::new(1024) };
    for name in before.keys().filter(|name| name.ends_with(".meta")) {
        let (was, is) = (meta::read(&content.join(name)).unwrap(), meta::read(&out.join(name)).unwrap());
        assert_eq!(was.id, is.id, "{name}");
        let source = name.trim_end_matches(".meta");
        let expected =
            match (source.ends_with("_palette.png") || source.ends_with("_layers.png"), &was.importer) {
                (true, ImportSettings::Texture(settings)) => {
                    ImportSettings::Texture(TextureSettings { max_size: capped, ..*settings })
                }
                (false, ImportSettings::CharacterMap(settings, _)) if source.ends_with(".ilm.ron") => {
                    ImportSettings::CharacterMap(*settings, capped)
                }
                _ => was.importer.clone(),
            };
        assert_eq!(is.importer, expected, "{name}");
    }
    let replaced: Vec<&str> = done.replaced.iter().map(String::as_str).collect();
    assert!(replaced.contains(&model) && replaced.contains(&"combat/ember_lancer/ember_lancer.ilm.ron.meta"));
    assert!(!replaced.contains(&"combat/ember_lancer/ember_lancer_matcap.png.meta"), "{replaced:?}");
}

/// A tree that isn't one it wrote is never emptied, and a kind of more than one copy is refused.
#[test]
fn a_kinds_tree_is_its_own() {
    let dir = TempDir::new("kinds_refusals");
    let (bench, content, out) =
        (dir.path().join("bench"), dir.path().join("content"), dir.path().join("out"));
    for folder in [&bench, &content, &out] {
        fs::create_dir_all(folder).unwrap();
    }
    fs::write(out.join("mine.txt"), "someone's").unwrap();
    let document = |copies: u32| {
        format!(
            "(type: \"sapphire::pipeline::BenchKinds\", version: 1, data: (kinds: [
                (spec: \"ember_lancer\", presets: (ci: None, budget: Some((copies: {copies}, maps: Some(512))))),
            ]))"
        )
    };
    fs::write(bench.join(DOCUMENT), document(1)).unwrap();
    assert!(matches!(
        generate(&bench, &specs(), Preset::Ci, &content, &out),
        Err(BenchError::NotGenerated(_))
    ));
    assert!(out.join("mine.txt").is_file());
    fs::write(bench.join(DOCUMENT), document(2)).unwrap();
    assert!(matches!(figures(&bench, &specs(), Preset::Budget), Err(BenchError::Invalid { .. })));
    // A figure the content doesn't ship can't be made again: its files would have no sidecars.
    fs::write(bench.join(DOCUMENT), document(1)).unwrap();
    let empty = dir.path().join("empty");
    assert!(matches!(
        generate(&bench, &specs(), Preset::Budget, &content, &empty),
        Err(BenchError::Invalid { .. })
    ));
}

/// The committed document: Aster at the hero row's 80,000 triangles and Wren at the same detail's 83,808, 4.8%
/// over it, both with its 2K maps, and the Ember Lancer's maps at the enemy row's 1K; nothing on the ci preset.
#[test]
fn the_committed_kinds_are_the_party_s_and_the_wave_s() {
    let bench = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../bench");
    assert!(figures(&bench, &specs(), Preset::Ci).unwrap().is_empty());
    let budget = figures(&bench, &specs(), Preset::Budget).unwrap();
    let rows: Vec<(&str, Option<u32>, u32)> = budget
        .iter()
        .map(|figure| (figure.name.as_str(), figure.triangles, figure.maps.as_ref().unwrap().size))
        .collect();
    assert_eq!(
        rows,
        [("wren", Some(83_808), 2048), ("hero", Some(80_000), 2048), ("ember_lancer", Some(8_000), 1024)]
    );
}

/// What a cook of a kind's model makes, in its own folder by its committed sidecar: LOD0's triangles, the
/// joints cooked and the material slots.
fn cooked_in(folder: &Path, model: &Path, cook: Cook) -> (u32, u32, usize) {
    let mesh = crate::cook::cook_mesh_in(folder, model, cook).unwrap();
    let triangles = mesh.lods[0].submeshes.iter().map(|submesh| submesh.index_count / 3).sum();
    (triangles, mesh.skin.as_ref().map_or(0, |skin| skin.joint_count), mesh.materials.len())
}

/// The `budget` kinds, made again in a tree of the shipped content and cooked by their committed sidecars, at
/// spec §3.4's rows (M5 C12b): Wren and Aster within 5% of the hero row at desktop LOD0 (76,000 to 84,000)
/// and of its 30,000 on mobile (Wren the one over both, by 4.8%), at most 200 joints and 6 materials (4 on
/// mobile), 2K maps that store 1K on mobile by their cap; the Ember Lancer at 8,000 with 1K maps and no cap.
/// What a kind has against mobile's 140 joints is printed, as the spec's §3.4 says it, and not held.
#[test]
fn every_budget_kind_cooks_at_its_row() {
    let dir = TempDir::new("kinds_budget_rows");
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let out = dir.path().join("content");
    generate(&root.join("bench"), &specs(), Preset::Budget, &root.join("content"), &out).unwrap();
    let mobile_cap = MaxSize { desktop: None, mobile: std::num::NonZeroU32::new(1024) };
    for figure in figures(&root.join("bench"), &specs(), Preset::Budget).unwrap() {
        let files = figure::figure_files(&figure).unwrap();
        let folder = out.join(&figure.dir);
        let model = out.join(&files[0].path);
        let (desktop, mobile) =
            (cooked_in(&folder, &model, Cook::Desktop), cooked_in(&folder, &model, Cook::Mobile));
        let maps: Vec<((u32, u32), MaxSize)> = files
            .iter()
            .filter(|file| matches!(file.kind, FileKind::Palette(_)))
            .map(|file| {
                let bytes = fs::read(out.join(&file.path)).unwrap();
                let word = |at: usize| u32::from_be_bytes(bytes[at..at + 4].try_into().unwrap());
                let ImportSettings::Texture(settings) =
                    meta::read(&meta::sidecar_path(&out.join(&file.path))).unwrap().importer
                else {
                    panic!("{}: a texture", file.path)
                };
                ((word(16), word(20)), settings.max_size)
            })
            .collect();
        let row = format!("{}: desktop {desktop:?}, mobile {mobile:?}, maps {maps:?}", figure.name);
        eprintln!("{row}");
        match figure.name.as_str() {
            "wren" | "hero" => {
                // Within 5% of the row: Wren is the one over it (83,808), since detail is a whole step and her
                // spec wasn't shaped to land on the row as Aster's was. The spec's §3.4 says so.
                assert!((76_000..=84_000).contains(&desktop.0), "{row}");
                // Her mobile LOD0 is the same 4.8% over the mobile row (31,428 of 30,000); Aster's is on it.
                let mobile_row = if figure.name == "wren" { 31_500 } else { 30_000 };
                assert!(mobile.0 <= mobile_row, "{row}");
                assert!(desktop.1 <= 200 && mobile.1 == desktop.1, "{row}");
                assert!(desktop.2 <= 6 && mobile.2 <= 4, "{row}");
                assert!(
                    !maps.is_empty() && maps.iter().all(|&map| map == ((2048, 2048), mobile_cap)),
                    "{row}"
                );
            }
            "ember_lancer" => {
                assert_eq!(desktop.0, 8_000, "{row}");
                assert!(mobile.0 < desktop.0, "{row}");
                assert_eq!(maps, [((1024, 1024), MaxSize::NONE)], "{row}");
            }
            other => panic!("{other} isn't a kind of the party's or the wave's"),
        }
    }
}
