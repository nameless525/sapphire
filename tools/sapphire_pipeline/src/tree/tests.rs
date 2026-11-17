use std::path::{Path, PathBuf};

use glam::Vec3;

use super::*;
use crate::bench::trees::{self, PART};
use crate::bench::{Preset, Source, asset_id};
use crate::check::{self, Role};
use crate::cook::Cook;
use crate::import;
use crate::meta::{self, AssetMeta, ImportSettings};
use crate::test_util::TempDir;

/// The repository's `bench/trees/`.
fn trees_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../bench/trees")
}

fn species() -> Vec<TreeSpecies> {
    trees::species(&trees_dir()).unwrap()
}

/// Writes `sources` under `root` as `bench::generate` writes them, each with its sidecar.
fn write(root: &Path, sources: &[Source]) {
    for source in sources {
        let path = root.join(&source.path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        if let Some((importer, cook)) = &source.sidecar {
            let meta = AssetMeta {
                id: asset_id(PART, &source.path),
                importer: importer.clone(),
                cook: *cook,
                budget: source.budget.clone(),
            };
            std::fs::write(meta::sidecar_path(&path), sapphire_serialize::to_ron(&meta).unwrap()).unwrap();
        }
        std::fs::write(&path, &source.bytes).unwrap();
    }
}

/// Each source's path and bytes, in order.
fn bytes(sources: &[Source]) -> Vec<(&str, &[u8])> {
    sources.iter().map(|source| (source.path.as_str(), source.bytes.as_slice())).collect()
}

/// The four default species (m6-plan §7, item 8), three ages each, youngest first and growing.
#[test]
fn the_four_species_grow_at_three_ages() {
    let species = species();
    let names: Vec<&str> = species.iter().map(|species| species.name.as_str()).collect();
    assert_eq!(names, ["broadleaf", "blossom", "conifer", "willow"]);
    for species in &species {
        let ages: Vec<&str> = species.ages.iter().map(|age| age.name.as_str()).collect();
        assert_eq!(ages, ["young", "mature", "old"], "{}", species.name);
        assert!(species.ages.windows(2).all(|pair| pair[0].height < pair[1].height), "{}", species.name);
    }
}

/// The trees are the same bytes however often they're made and wherever the checkout is: nothing of the
/// path they're read from reaches them. Across machines, the benchmark's pinned digests hold them
/// (`bench::tests::the_ci_preset_generates_the_same_bytes_on_every_machine`).
#[test]
fn trees_are_the_same_bytes_twice_and_from_two_checkouts() {
    let scratch = TempDir::new("trees_two_roots");
    let mut made = Vec::new();
    for root in ["a", "elsewhere/deeper/b"] {
        let dir = scratch.path().join(root);
        std::fs::create_dir_all(dir.join("species")).unwrap();
        std::fs::copy(trees_dir().join("trees.ron"), dir.join("trees.ron")).unwrap();
        for species in species() {
            let name = format!("{}{SPEC_EXTENSION}", species.name);
            std::fs::copy(trees_dir().join("species").join(&name), dir.join("species").join(&name)).unwrap();
        }
        made.push(trees::generate(&dir, Preset::Ci, Cook::Desktop).unwrap());
    }
    let again = trees::generate(&trees_dir(), Preset::Ci, Cook::Desktop).unwrap();
    assert_eq!(bytes(&made[0]), bytes(&made[1]));
    assert_eq!(bytes(&made[0]), bytes(&again));
    assert_eq!(made[0].len(), 4 * (3 + 3), "an atlas, two materials and three ages a species");
}

/// Every species at every age meets the tree row on both cooks, as `sapphire_pipeline check` judges a tree
/// of its sources (cooked to memory, its foliage baked), with the budget's 1024 atlases: no waiver.
#[test]
fn every_species_at_every_age_passes_the_check_at_the_tree_row() {
    let scratch = TempDir::new("trees_check");
    let mut sources = Vec::new();
    for species in species() {
        trees::species_sources(&species, 1024, &mut sources).unwrap();
    }
    write(scratch.path(), &sources);
    let report = check::check(scratch.path()).unwrap();
    assert!(report.passes(), "{report}");
    assert!(report.findings.is_empty(), "{report}");
    let judged: Vec<(&str, Role)> =
        report.meshes.iter().map(|mesh| (mesh.source.as_str(), mesh.role)).collect();
    assert_eq!(judged.len(), 12, "{report}");
    assert!(judged.iter().all(|(_, role)| *role == Role::Tree), "{report}");
    for mesh in &report.meshes {
        let [desktop, mobile] = [&mesh.cooks[0], &mesh.cooks[1]];
        assert_eq!((desktop.slots, desktop.joints, desktop.map), (2, 0, 1024), "{}", mesh.source);
        assert_eq!(mobile.map, 512, "{}: the atlas capped on the phones", mesh.source);
        assert_eq!(desktop.triangles, mobile.triangles, "{}: no levels", mesh.source);
        eprintln!("{} {} triangles", mesh.source, desktop.triangles);
    }
}

/// Each tree stands on its root at its age's height (cards reaching a little past it), its wood's
/// triangles wound outward, and has the two slots its sidecar binds.
#[test]
fn trees_stand_on_their_roots_at_their_heights_with_their_wood_wound_outward() {
    for species in species() {
        for (index, age) in species.ages.iter().enumerate() {
            let skeleton = grow::grow(&species, index);
            let mesh = import::gltf::import_slice(&mesh::tree_glb(&species, index, &skeleton), None).unwrap();
            let what = format!("{} {}", species.name, age.name);
            let names: Vec<&str> = mesh.materials.iter().map(|slot| slot.name.as_str()).collect();
            assert_eq!(names, [BARK_SLOT, LEAVES_SLOT], "{what}");
            let lowest = mesh.vertices.iter().map(|v| v.position[1]).fold(f32::INFINITY, f32::min);
            let top = mesh.vertices.iter().map(|v| v.position[1]).fold(f32::NEG_INFINITY, f32::max);
            // A willow's strands hang below its crown, but never to the ground.
            assert!((-0.01..=0.01).contains(&lowest), "{what}: its lowest point is {lowest} m");
            assert!(
                (0.85 * age.height..=1.2 * age.height).contains(&top),
                "{what}: {top} m tall for {} m",
                age.height
            );
            let wood = &mesh.lods[0].submeshes[0];
            let range = wood.first_index as usize..(wood.first_index + wood.index_count) as usize;
            assert!(wood.index_count > 0, "{what}: no wood");
            for triangle in mesh.indices[range].as_chunks::<3>().0 {
                let [a, b, c] = [0, 1, 2].map(|i| Vec3::from(mesh.vertices[triangle[i] as usize].position));
                let face = (b - a).cross(c - a);
                let normals: Vec3 =
                    triangle.iter().map(|&i| Vec3::from(mesh.vertices[i as usize].normal)).sum();
                assert!(face.dot(normals) >= -1e-7, "{what}: {triangle:?} is wound against its normals");
            }
        }
    }
}

/// Cards sit on twig tips: every card's middle is within a card of one.
#[test]
fn cards_sit_on_twig_tips() {
    for species in species() {
        for (index, age) in species.ages.iter().enumerate() {
            let skeleton = grow::grow(&species, index);
            let tips: Vec<Vec3> =
                skeleton.tips().iter().map(|&tip| skeleton.nodes[tip as usize].position).collect();
            let cards = mesh::cards(&species, index, &skeleton);
            assert_eq!(cards.positions.len(), tips.len() * species.leaves.cards as usize * 4);
            for corners in cards.positions.as_chunks::<4>().0 {
                let middle = corners.iter().map(|&p| Vec3::from(p)).sum::<Vec3>() / 4.0;
                let near = tips.iter().map(|tip| tip.distance(middle)).fold(f32::INFINITY, f32::min);
                assert!(near <= age.card * 1.2, "{} {}: a card {near} m from a tip", species.name, age.name);
            }
        }
    }
}

/// Space colonization grows a tree, not a stick: an old tree has many more tips than a young one, its
/// wood is thickest at the root and thins to the tip's radius, and every node is its parent's child.
#[test]
fn the_skeleton_branches_and_thins_by_the_pipe_model() {
    for species in species() {
        let tips: Vec<usize> =
            (0..species.ages.len()).map(|age| grow::grow(&species, age).tips().len()).collect();
        assert!(tips[0] >= 20 && tips[2] > tips[0] * 2, "{}: {tips:?} tips", species.name);
        let skeleton = grow::grow(&species, 1);
        let root = skeleton.nodes[0].radius;
        for (index, node) in skeleton.nodes.iter().enumerate() {
            assert!(node.radius <= root + 1e-6, "{}: node {index} thicker than the root", species.name);
            assert!(node.parent.is_none_or(|parent| (parent as usize) < index));
        }
        for tip in skeleton.tips() {
            assert_eq!(skeleton.nodes[tip as usize].radius, species.wood.tip, "{}", species.name);
        }
    }
}

/// The atlas's cells are clear at their borders, so no sample or mip reaches a neighbour's leaves, and
/// painted inside: neither a speck nor a filled square.
#[test]
fn atlas_cells_are_clear_at_their_borders_and_covered_inside() {
    for species in species() {
        let image = image::load_from_memory(&atlas::atlas_png(&species, 256).unwrap()).unwrap().to_rgba8();
        assert_eq!(image.dimensions(), (256, 256));
        let (columns, rows) = atlas::cells(species.leaves.brush);
        let (width, height) = (256 / columns, 256 / rows);
        for cell in 0..columns * rows {
            let (left, top) = ((cell % columns) * width, (cell / columns) * height);
            let alpha = |x: u32, y: u32| image.get_pixel(left + x, top + y)[3];
            for x in 0..width {
                assert_eq!((alpha(x, 0), alpha(x, height - 1)), (0, 0), "{} cell {cell}", species.name);
            }
            for y in 0..height {
                assert_eq!((alpha(0, y), alpha(width - 1, y)), (0, 0), "{} cell {cell}", species.name);
            }
            let covered = (0..height)
                .flat_map(|y| (0..width).map(move |x| (x, y)))
                .filter(|&(x, y)| alpha(x, y) > 128)
                .count() as f32
                / (width * height) as f32;
            assert!((0.15..0.8).contains(&covered), "{} cell {cell}: {covered} covered", species.name);
        }
    }
}

/// A spec out of range is refused, saying which field.
#[test]
fn a_spec_out_of_range_is_refused_by_its_field() {
    let mut spec = species().remove(0);
    spec.growth.kill = spec.growth.influence + 1.0;
    assert!(spec.validate().unwrap_err().contains("growth.kill"));
    let mut spec = species().remove(0);
    spec.leaves.fill = "green".to_owned();
    assert!(spec.validate().unwrap_err().contains("leaves.fill"));
    let mut spec = species().remove(0);
    spec.ages[2].name = "young".to_owned();
    assert!(spec.validate().unwrap_err().contains("ages[2].name"));
    let text = std::fs::read_to_string(trees_dir().join("species/broadleaf.tree.ron")).unwrap();
    let unknown = text.replace("seed: 1101,", "seed: 1101, height: 3.0,");
    assert!(sapphire_serialize::from_ron::<TreeSpecies>(&unknown).is_err(), "unknown fields are refused");
}

/// Every age's sidecar bakes its leaves' normals from its clumps, binds both slots, and keeps one level.
#[test]
fn every_model_bakes_its_clumps_and_binds_its_materials() {
    let mut sources = Vec::new();
    for species in species() {
        trees::species_sources(&species, 256, &mut sources).unwrap();
    }
    let models: Vec<&Source> = sources.iter().filter(|source| source.path.ends_with(".glb")).collect();
    assert_eq!(models.len(), 12);
    for model in models {
        let Some((ImportSettings::Gltf(settings), true)) = &model.sidecar else { panic!("{}", model.path) };
        let foliage = settings.foliage.as_ref().expect("a foliage bake");
        assert!(matches!(foliage.normals, NormalTransfer::Clumps { .. }), "{}", model.path);
        assert_eq!(foliage.slots, [LEAVES_SLOT]);
        let bound: Vec<&str> = settings.materials.keys().map(String::as_str).collect();
        assert_eq!(bound, [BARK_SLOT, LEAVES_SLOT], "{}", model.path);
        let budget = model.budget.as_ref().expect("a role");
        assert_eq!(budget.role, Some(Role::Tree));
    }
}
