//! The pipeline inventory (M3 K1, exit test 7): a real cook's table holds what its meshes' uploads draw through,
//! so a prewarm from it leaves them nothing to build, and a key left out is named. These hold the cooker's keys
//! to the renderer's derivation: the skinned mesh is what they'd drift on, since it's cooked in one position format
//! and drawn in another. They need a GPU: run with `cargo test -p sapphire_render --test gpu inventory:: -- --ignored`.

use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::{fs, process};

use sapphire_assets::material::{
    AlphaMode, AnimeCharacterMaterial, EnvironmentMaterial, FogCardMaterial, Material, MaterialFamily,
    MaterialFlags,
};
use sapphire_assets::mesh::{Mesh, PositionFormat};
use sapphire_assets::pack::PackReader;
use sapphire_assets::probes::ProbeGrid;
use sapphire_assets::{AssetId, AssetKind, PipelineKey};
use sapphire_pipeline::cook::{Cook, CookOptions, cook_dir};
use sapphire_pipeline::material::MaterialSettings;
use sapphire_pipeline::meta::{AssetMeta, ImportSettings, MeshSettings, sidecar_path};
use sapphire_pipeline::pack::pack_cooked;
use sapphire_pipeline::samples;
use sapphire_render::{PipelineInventory, PipelineStats, RenderSettings, Renderer};
use sapphire_rhi::{GpuContext, GpuContextDesc, GpuInstance, GpuInstanceDesc, wgpu};

const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8UnormSrgb;
const HAIR: &str = "9a8b7c6d-5e4f-4a3b-9c2d-1e0f9a8b7c6d";
const MANNEQUIN: &str = "1f2e3d4c-5b6a-4978-8695-a4b3c2d1e0f9";
const HEAD: &str = "2e3d4c5b-6a79-4886-95a4-b3c2d1e0f91f";
const WALL: &str = "3d4c5b6a-7988-4695-a4b3-c2d1e0f91f2e";
const WALL_MATERIAL: &str = "4c5b6a79-8896-45a4-b3c2-d1e0f91f2e3d";
const FOG: &str = "5b6a7988-9695-44b3-82d1-e0f91f2e3d4c";
const FOG_MATERIAL: &str = "6a798896-95a4-43c2-91e0-f91f2e3d4c5b";

fn gpu() -> GpuContext {
    let instance = GpuInstance::new(&GpuInstanceDesc::default()).expect("instance");
    GpuContext::new(&instance, &GpuContextDesc::default(), None).expect("device")
}

/// The pipelines alive on `gpu`'s device, of both kinds.
fn alive(gpu: &GpuContext) -> usize {
    let report = gpu.instance().raw().generate_report().expect("a wgpu-core backend");
    report.hub.render_pipelines.num_kept_from_user + report.hub.compute_pipelines.num_kept_from_user
}

fn renderer(gpu: &GpuContext) -> Renderer {
    Renderer::new(gpu, FORMAT, RenderSettings::default()).expect("renderer")
}

/// A directory of this test's own, gone when it ends.
struct Temp(PathBuf);

impl Temp {
    fn new(name: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("sapphire_inventory_{name}_{}", process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("content")).expect("a temp dir");
        Self(dir)
    }
}

impl Drop for Temp {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn write(source: &Path, name: &str, bytes: &[u8], id: &str, importer: ImportSettings) {
    let path = source.join(name);
    fs::write(&path, bytes).expect("a source file");
    let meta = AssetMeta::new(id.parse().expect("an ID"), importer);
    fs::write(sidecar_path(&path), sapphire_serialize::to_ron(&meta).expect("a sidecar")).expect("written");
}

/// A card with one slot, drawn with `material`, a document of `family`'s.
fn write_card(source: &Path, name: &str, ids: (&str, &str), material: &str) {
    let settings = ImportSettings::Material(MaterialSettings::default());
    write(source, &format!("{name}.material.ron"), material.as_bytes(), ids.1, settings);
    let mut settings = MeshSettings::default();
    settings.materials.insert("Card".into(), ids.1.parse().expect("an ID"));
    let card = samples::environment::card_glb("Card");
    write(source, &format!("{name}.glb"), &card, ids.0, ImportSettings::Gltf(settings));
}

/// A cook's shared pack: a static character, its hair slot bound to an alpha-tested material asset; a head with
/// morph targets, cooked quantized and drawn in floats; and an environment card and a fog card.
fn cooked_pack(name: &str, cook: Cook) -> Vec<u8> {
    let temp = Temp::new(name);
    let source = temp.0.join("content");
    let hair = AnimeCharacterMaterial {
        alpha: AlphaMode::Mask { cutoff: 0.5 },
        ..AnimeCharacterMaterial::default()
    };
    let hair = sapphire_serialize::to_ron(&hair).expect("a material document");
    let settings = ImportSettings::Material(MaterialSettings::default());
    write(&source, "hair.material.ron", hair.as_bytes(), HAIR, settings);
    let mut settings = MeshSettings::default();
    settings.materials.insert("Hair".into(), HAIR.parse().expect("an ID"));
    write(&source, "mannequin.glb", &samples::mannequin_glb(), MANNEQUIN, ImportSettings::Gltf(settings));
    let settings = ImportSettings::Gltf(MeshSettings::default());
    write(&source, "head.glb", &samples::face::head_glb(), HEAD, settings);
    let wall = sapphire_serialize::to_ron(&EnvironmentMaterial::default()).expect("a material document");
    write_card(&source, "wall", (WALL, WALL_MATERIAL), &wall);
    let fog = sapphire_serialize::to_ron(&FogCardMaterial::default()).expect("a material document");
    write_card(&source, "fog", (FOG, FOG_MATERIAL), &fog);
    let options = CookOptions { cook, cache: None, locked: false };
    let report = cook_dir(&source, &temp.0.join("loose"), &options).expect("it cooks");
    let packs = temp.0.join("packs");
    pack_cooked(&report, &packs, None).expect("it packs");
    fs::read(packs.join("shared.spack")).expect("the shared pack")
}

/// Every material and mesh in the pack, uploaded the way the runtime loads them.
fn upload_all(renderer: &mut Renderer, pack: &[u8]) {
    let reader = PackReader::new(pack).expect("a valid pack");
    let mut materials = HashMap::<AssetId, _>::new();
    for entry in reader.toc().entries().iter().filter(|entry| entry.kind == AssetKind::Material) {
        let bytes = reader.read(entry.id).expect("a material");
        let material = Material::read(&bytes).expect("a valid material");
        materials.insert(entry.id, renderer.upload_material(material, |_| None).expect("uploaded"));
    }
    for entry in reader.toc().entries().iter().filter(|entry| entry.kind == AssetKind::Mesh) {
        let bytes = reader.read(entry.id).expect("a mesh");
        let mesh = Mesh::read(&bytes).expect("a valid mesh");
        renderer.upload_mesh_with_slots(mesh, |id| materials.get(&id).copied()).expect("uploaded");
    }
}

fn keys_of(pack: &[u8]) -> BTreeSet<PipelineKey> {
    let reader = PackReader::new(pack).expect("a valid pack");
    reader.toc().pipeline_keys().iter().copied().collect()
}

/// The table's keys are exact: a renderer prewarmed from them and one that only uploads build the same pipelines,
/// the first with no first use and the second with nothing else. wgpu's own count stands beside the ledger's, so
/// a pipeline built past the route shows here too.
#[test]
#[ignore = "needs a GPU"]
fn a_cooks_keys_build_exactly_what_its_uploads_draw_through() {
    for (name, cook) in [("keys_desktop", Cook::Desktop), ("keys_mobile", Cook::Mobile)] {
        let pack = cooked_pack(name, cook);
        let keys = keys_of(&pack);
        for format in PositionFormat::ALL {
            assert!(keys.iter().any(|key| key.format == format), "{format:?} isn't in {keys:?}");
        }
        for family in [MaterialFamily::AnimeCharacter, MaterialFamily::Environment, MaterialFamily::FogCard] {
            assert!(keys.iter().any(|key| key.family == family), "{family:?} isn't in {keys:?}");
        }
        let gpu = gpu();

        let before = alive(&gpu);
        let mut warmed = renderer(&gpu);
        let at_start = warmed.stats().pipelines.prewarmed;
        let inventory = PipelineInventory { meshes: keys, skinning: true, ..PipelineInventory::default() };
        let built = warmed.prewarm(&inventory);
        assert!(built > 0);
        assert_eq!(warmed.prewarm(&inventory), 0, "it only adds");
        upload_all(&mut warmed, &pack);
        let report = warmed.first_use_report();
        let stats = warmed.stats().pipelines;
        assert_eq!(report.builds, [], "{report}");
        assert_eq!(stats, PipelineStats { prewarmed: at_start + built, first_use_builds: 0, rebuilt: 0 });
        assert!(alive(&gpu) - before <= stats.built() as usize, "wgpu holds one the ledger didn't count");
        drop(warmed);

        let before = alive(&gpu);
        let mut cold = renderer(&gpu);
        upload_all(&mut cold, &pack);
        let stats = cold.stats().pipelines;
        assert_eq!(
            stats.first_use_builds,
            built,
            "uploads build what the keys say: {}",
            cold.first_use_report()
        );
        assert_eq!(stats.prewarmed, at_start);
        assert!(alive(&gpu) - before <= stats.built() as usize, "wgpu holds one the ledger didn't count");
    }
}

/// A key left out comes back named, and only it.
#[test]
#[ignore = "needs a GPU"]
fn a_key_left_out_of_the_inventory_is_named() {
    let pack = cooked_pack("left_out", Cook::Desktop);
    let keys = keys_of(&pack);
    let hair = keys
        .iter()
        .copied()
        .find(|key| key.flags.contains(MaterialFlags::ALPHA_TEST))
        .unwrap_or_else(|| panic!("an alpha-tested key in {keys:?}"));
    let mut inventory = PipelineInventory { meshes: keys, skinning: true, ..PipelineInventory::default() };
    assert!(inventory.entries().contains(&format!("mesh {hair}")), "{:?}", inventory.entries());
    assert!(inventory.remove(&format!("mesh {hair}")));
    assert!(!inventory.remove(&format!("mesh {hair}")), "it's gone");

    let gpu = gpu();
    let mut renderer = renderer(&gpu);
    renderer.prewarm(&inventory);
    upload_all(&mut renderer, &pack);
    let report = renderer.first_use_report();
    assert_eq!(report.keys, [hair], "{report}");
    assert!(!report.builds.is_empty());
    assert!(report.to_string().contains(&format!("mesh {hair}")), "{report}");
    assert_eq!(renderer.stats().pipelines.first_use_builds as usize, report.builds.len());
}

/// A probe grid's arrival rebuilds the environment and foliage surfaces with its constants on and needs the
/// ambient pass's pipelines (M3 C1): a load that declares it has already built them, and one that doesn't is
/// told what its first frame had to.
#[test]
#[ignore = "needs a GPU"]
fn a_probe_grid_a_load_declares_arrives_with_nothing_to_build() {
    let pack = cooked_pack("probe_grid", Cook::Desktop);
    let keys = keys_of(&pack);
    let grid = super::ambient::grid(&[12.0], 1.0, |_, _| glam::Vec3::ZERO, super::ambient::open);
    let bytes = grid.to_cooked().expect("cook");
    let archived = ProbeGrid::read(&bytes).expect("a valid grid");
    let gpu = gpu();

    let mut declared = renderer(&gpu);
    declared.prewarm(&PipelineInventory {
        meshes: keys.clone(),
        skinning: true,
        probe_grid: true,
        ..PipelineInventory::default()
    });
    upload_all(&mut declared, &pack);
    declared.set_probe_grid(Some(archived)).expect("the grid fits");
    let report = declared.first_use_report();
    assert!(report.builds.is_empty() && report.keys.is_empty(), "{report}");
    assert_eq!(declared.stats().pipelines.rebuilt, 0);

    let mut undeclared = renderer(&gpu);
    undeclared.prewarm(&PipelineInventory { meshes: keys, skinning: true, ..PipelineInventory::default() });
    upload_all(&mut undeclared, &pack);
    assert_eq!(undeclared.stats().pipelines.first_use_builds, 0, "only the grid is missing");
    undeclared.set_probe_grid(Some(archived)).expect("the grid fits");
    let report = undeclared.first_use_report().to_string();
    assert!(report.contains("ambient: cs_relight"), "{report}");
    assert!(report.contains("AMBIENT_PROBES=1"), "the surfaces it rebuilds are first uses too: {report}");
}

/// What no table can say is named by its family when it's left out.
#[test]
#[ignore = "needs a GPU"]
fn skinning_left_out_of_the_inventory_is_named() {
    let pack = cooked_pack("skinning_left_out", Cook::Desktop);
    let mut inventory =
        PipelineInventory { meshes: keys_of(&pack), skinning: true, ..PipelineInventory::default() };
    assert!(inventory.remove("skinning"));
    let gpu = gpu();
    let mut renderer = renderer(&gpu);
    renderer.prewarm(&inventory);
    upload_all(&mut renderer, &pack);
    let report = renderer.first_use_report();
    assert_eq!(report.keys, [], "every key was listed: {report}");
    assert!(report.builds.iter().all(|build| build.family.name() == "skinning"), "{report}");
    assert!(report.to_string().contains("skinning: "), "{report}");
}
