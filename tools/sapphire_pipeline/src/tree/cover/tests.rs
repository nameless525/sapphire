use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use glam::Vec3;

use super::paint::{FLOWER_CELLS, FROND_CELLS, SWATCHES};
use super::*;
use crate::bench::cover::{self, PART};
use crate::bench::{Preset, Source, asset_id};
use crate::check::{self, Role};
use crate::import;
use crate::meta::{self, AssetMeta};
use crate::test_util::TempDir;

/// The repository's `bench/cover/`.
fn cover_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../bench/cover")
}

fn species() -> Vec<CoverSpecies> {
    cover::species(&cover_dir()).unwrap()
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

/// The row's ground cover (m6-plan, G6): a bush, a fern, two flowers, two grass clumps and rocks.
#[test]
fn the_species_are_a_bush_a_fern_two_flowers_two_grasses_and_rocks() {
    let forms: Vec<(String, &str, usize)> = species()
        .iter()
        .map(|species| {
            let form = match species.form {
                Form::Bush(_) => "bush",
                Form::Fern(_) => "fern",
                Form::Flower(_) => "flower",
                Form::Grass(_) => "grass",
                Form::Rock(_) => "rock",
            };
            (species.name.clone(), form, species.variants.len())
        })
        .collect();
    let expected = [
        ("bush", "bush", 2),
        ("fern", "fern", 2),
        ("daisy", "flower", 1),
        ("starflower", "flower", 1),
        ("meadow_grass", "grass", 1),
        ("tall_grass", "grass", 1),
        ("rock", "rock", 3),
    ];
    assert_eq!(forms, expected.map(|(name, form, variants)| (name.to_owned(), form, variants)));
}

/// The ground cover is the same bytes however often it's made, wherever the checkout is, and whether one
/// thread makes it or eight make it at once: nothing of the path, the thread or the order reaches it.
/// Across machines, the benchmark's pinned digests hold it
/// (`bench::tests::the_ci_preset_generates_the_same_bytes_on_every_machine`).
#[test]
fn cover_is_the_same_bytes_twice_from_two_checkouts_and_on_eight_threads() {
    let scratch = TempDir::new("cover_two_roots");
    let mut made = Vec::new();
    for root in ["a", "elsewhere/deeper/b"] {
        let dir = scratch.path().join(root);
        std::fs::create_dir_all(dir.join("species")).unwrap();
        std::fs::copy(cover_dir().join("cover.ron"), dir.join("cover.ron")).unwrap();
        for species in species() {
            let name = format!("{}{SPEC_EXTENSION}", species.name);
            std::fs::copy(cover_dir().join("species").join(&name), dir.join("species").join(&name)).unwrap();
        }
        made.push(cover::generate(&dir, Preset::Ci, crate::cook::Cook::Desktop).unwrap());
    }
    let again = cover::generate(&cover_dir(), Preset::Ci, crate::cook::Cook::Desktop).unwrap();
    assert_eq!(bytes(&made[0]), bytes(&made[1]));
    assert_eq!(bytes(&made[0]), bytes(&again));
    let eight: Vec<Vec<Source>> = std::thread::scope(|scope| {
        let threads: Vec<_> = (0..8)
            .map(|_| {
                scope.spawn(|| cover::generate(&cover_dir(), Preset::Ci, crate::cook::Cook::Desktop).unwrap())
            })
            .collect();
        threads.into_iter().map(|thread| thread.join().unwrap()).collect()
    });
    for sources in &eight {
        assert_eq!(bytes(sources), bytes(&again));
    }
    // Eleven models; an atlas and its materials a species, the grass clumps' one material and no atlas each.
    assert_eq!(again.iter().filter(|source| source.path.ends_with(".glb")).count(), 11);
    assert_eq!(again.iter().filter(|source| source.path.ends_with(".png")).count(), 5);
    assert_eq!(again.iter().filter(|source| source.path.ends_with(".material.ron")).count(), 10);
}

/// Every variant of every species meets the cover row on both cooks, as `sapphire_pipeline check` judges a
/// tree of its sources (cooked to memory, its foliage baked), with the budget's 512 atlases: no waiver.
#[test]
fn every_variant_passes_the_check_at_the_cover_row() {
    let scratch = TempDir::new("cover_check");
    let mut sources = Vec::new();
    let species = species();
    for species in &species {
        cover::species_sources(species, 512, &mut sources).unwrap();
    }
    write(scratch.path(), &sources);
    let report = check::check(scratch.path()).unwrap();
    assert!(report.passes(), "{report}");
    assert!(report.findings.is_empty(), "{report}");
    assert_eq!(report.meshes.len(), 11, "{report}");
    let slots: BTreeMap<String, usize> = species
        .iter()
        .flat_map(|species| {
            (0..species.variants.len())
                .map(|variant| (cover::model_path(species, variant), species.slots().len()))
        })
        .collect();
    for mesh in &report.meshes {
        assert_eq!(mesh.role, Role::Cover, "{}", mesh.source);
        let [desktop, mobile] = [&mesh.cooks[0], &mesh.cooks[1]];
        let source = mesh.source.trim_start_matches(&format!("{PART}/"));
        assert_eq!((desktop.slots, desktop.joints), (slots[source] as u32, 0), "{}", mesh.source);
        // A plant's atlas at the budget's 512, capped on the phones; a rock's swatches at every size; a grass
        // clump's blades draw no map.
        let form =
            &species.iter().find(|species| source.starts_with(&format!("{}/", species.name))).unwrap().form;
        let maps = match form {
            Form::Bush(_) | Form::Fern(_) | Form::Flower(_) => (512, 256),
            Form::Rock(_) => (SWATCH_SIZE, SWATCH_SIZE),
            Form::Grass(_) => (0, 0),
        };
        assert_eq!((desktop.map, mobile.map), maps, "{}", mesh.source);
        assert_eq!(desktop.triangles, mobile.triangles, "{}: no levels", mesh.source);
        eprintln!("{} {} triangles", mesh.source, desktop.triangles);
    }
}

/// Each model stands on the ground at its origin, as tall as its form says for its size, with the slots its
/// sidecar binds: a plant's lowest point on the ground, a rock's foot sunk by its share.
#[test]
fn each_model_stands_on_the_ground_at_its_size() {
    let mut wrong = Vec::new();
    for species in species() {
        for (index, variant) in species.variants.iter().enumerate() {
            let mesh = import::gltf::import_slice(&species.model_glb(index), None).unwrap();
            let what = format!("{} {}", species.name, variant.name);
            let names: Vec<&str> = mesh.materials.iter().map(|slot| slot.name.as_str()).collect();
            assert_eq!(names, species.slots(), "{what}");
            let lowest = mesh.vertices.iter().map(|v| v.position[1]).fold(f32::INFINITY, f32::min);
            let top = mesh.vertices.iter().map(|v| v.position[1]).fold(f32::NEG_INFINITY, f32::max);
            let size = variant.size;
            let (low, high) = match &species.form {
                Form::Rock(rock) => {
                    let height = rock.height * size;
                    ((-rock.sink * height - 0.02 * size, -0.01 * size), (0.6 * height, 1.05 * height))
                }
                // A fern's fronds arch over, so it stands lower than they're long.
                Form::Fern(_) => ((-0.05 * size, 0.01), (0.3 * size, size)),
                // Its lowest leaves' cards may dip a little into the ground, as a bush's do.
                Form::Bush(_) => ((-0.08 * size, 0.01), (0.8 * size, 1.25 * size)),
                Form::Flower(_) | Form::Grass(_) => ((-0.01, 0.01), (0.8 * size, 1.25 * size)),
            };
            if !(low.0..=low.1).contains(&lowest) {
                wrong.push(format!("{what}: its lowest point is {lowest} m"));
            }
            if !(high.0..=high.1).contains(&top) {
                wrong.push(format!("{what}: {top} m tall for {size} m"));
            }
        }
    }
    assert!(wrong.is_empty(), "{wrong:#?}");
}

/// Grass is opaque blades lit as the ground, each triangle with its back wound too; plants' leaves are
/// alpha-tested cards; a rock draws the world's environment material and bakes no foliage.
#[test]
fn grass_is_opaque_blades_plants_are_cards_and_rocks_are_the_worlds() {
    for species in species() {
        let atlas = species.atlas(128).unwrap();
        let materials = species.materials(atlas.as_ref().map(|_| asset_id(PART, "atlas.png")));
        let slots: Vec<&str> = materials.iter().map(|(slot, _, _)| *slot).collect();
        assert_eq!(slots, species.slots(), "{}", species.name);
        let foliage = species.foliage(0);
        match &species.form {
            Form::Grass(_) => {
                assert!(atlas.is_none(), "{}", species.name);
                let [(_, _, CoverMaterial::Foliage(blades))] = materials.as_slice() else {
                    panic!("{}", species.name)
                };
                assert_eq!((blades.alpha_cutoff, blades.base_color_map), (0.0, None), "{}", species.name);
                assert_eq!(foliage.unwrap().normals, NormalTransfer::Up);
                let mesh = import::gltf::import_slice(&species.model_glb(0), None).unwrap();
                let triangles: Vec<[u32; 3]> = mesh.indices.as_chunks::<3>().0.to_vec();
                for pair in triangles.chunks(2) {
                    let [[a, b, c], [d, e, f]] = [pair[0], pair[1]];
                    assert_eq!(
                        (d, e, f),
                        (a, c, b),
                        "{}: a blade's back is its front reversed",
                        species.name
                    );
                }
            }
            Form::Rock(_) => {
                assert!(foliage.is_none());
                assert!(matches!(materials[0].2, CoverMaterial::Environment(_)), "{}", species.name);
            }
            Form::Bush(_) | Form::Fern(_) | Form::Flower(_) => {
                assert!(atlas.is_some_and(|atlas| atlas.name == "leaves.png"), "{}", species.name);
                let (_, _, CoverMaterial::Foliage(leaves)) = materials.last().unwrap() else {
                    panic!("{}", species.name)
                };
                assert!(leaves.alpha_cutoff > 0.0 && leaves.base_color_map.is_some(), "{}", species.name);
                assert!(foliage.is_some(), "{}", species.name);
            }
        }
    }
}

/// A plant's atlas cells are clear at their borders, so no sample or mip reaches a neighbour's, and painted
/// inside: neither a speck nor a filled square. A rock's swatches are opaque, each one flat colour.
#[test]
fn atlases_are_clear_at_their_cells_borders_and_swatches_flat() {
    for species in species() {
        let Some(atlas) = species.atlas(256).unwrap() else { continue };
        let image = image::load_from_memory(&atlas.png).unwrap().to_rgba8();
        assert_eq!(image.dimensions(), (atlas.side, atlas.side), "{}", species.name);
        let (columns, rows) = match &species.form {
            Form::Bush(bush) => atlas::cells(bush.leaves.brush),
            Form::Fern(_) => FROND_CELLS,
            Form::Flower(_) => FLOWER_CELLS,
            Form::Rock(_) => {
                let (columns, rows) = SWATCHES;
                let (width, height) = (atlas.side / columns, atlas.side / rows);
                let mut tones = std::collections::BTreeSet::new();
                for (x, y, pixel) in image.enumerate_pixels() {
                    assert_eq!(pixel[3], 255);
                    assert_eq!(
                        pixel,
                        image.get_pixel(x / width * width, y / height * height),
                        "a flat swatch"
                    );
                    tones.insert(pixel.0);
                }
                assert_eq!(tones.len(), (columns * rows) as usize, "a tone a swatch");
                continue;
            }
            Form::Grass(_) => unreachable!("a grass clump paints none"),
        };
        let (width, height) = (atlas.side / columns, atlas.side / rows);
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
            assert!((0.12..0.8).contains(&covered), "{} cell {cell}: {covered} covered", species.name);
            eprintln!("{} cell {cell}: {covered:.2} covered", species.name);
        }
    }
}

/// A rock is mostly broad flat faces: most of its area lies in faces of many triangles sharing one normal,
/// the rest the bevels between them.
#[test]
fn a_rocks_faces_are_flat_and_broad() {
    for species in species() {
        let Form::Rock(rock) = &species.form else { continue };
        for (index, variant) in species.variants.iter().enumerate() {
            let mesh = import::gltf::import_slice(&species.model_glb(index), None).unwrap();
            let mut faces: BTreeMap<[i32; 3], f32> = BTreeMap::new();
            let mut total = 0.0;
            for triangle in mesh.indices.as_chunks::<3>().0 {
                let [a, b, c] = triangle.map(|i| Vec3::from(mesh.vertices[i as usize].position));
                let cross = (b - a).cross(c - a);
                let area = 0.5 * cross.length();
                let key = (cross.normalize() * 1000.0).round().to_array().map(|v| v as i32);
                *faces.entry(key).or_default() += area;
                total += area;
            }
            let mut areas: Vec<f32> = faces.into_values().collect();
            areas.sort_by(|a, b| b.total_cmp(a));
            let broad: f32 = areas.iter().take(rock.planes as usize).sum();
            assert!(
                broad > 0.5 * total,
                "{} {}: {broad} of {total} m² in its broadest faces",
                species.name,
                variant.name
            );
        }
    }
}

/// A spec out of range is refused, saying which field; a bush's tree fields are checked as a tree's.
#[test]
fn a_spec_out_of_range_is_refused_by_its_field() {
    let by_name = |name: &str| species().into_iter().find(|species| species.name == name).unwrap();
    let mut fern = by_name("fern");
    let Form::Fern(form) = &mut fern.form else { unreachable!() };
    form.width = 0.9;
    assert!(fern.validate().unwrap_err().contains("form.width"));
    let mut rock = by_name("rock");
    let Form::Rock(form) = &mut rock.form else { unreachable!() };
    form.moss = "moss".to_owned();
    assert!(rock.validate().unwrap_err().contains("form.moss"));
    let mut rock = by_name("rock");
    rock.variants[2].name = "small".to_owned();
    assert!(rock.validate().unwrap_err().contains("variants[2].name"));
    // A bush's size and card are refused under the cover's own names, before the tree's check sees them.
    let mut bush = by_name("bush");
    bush.variants[0].size = 0.4;
    assert!(bush.validate().unwrap_err().starts_with("variants[0].size"));
    let mut bush = by_name("bush");
    let Form::Bush(form) = &mut bush.form else { unreachable!() };
    form.card = 0.05;
    assert!(bush.validate().unwrap_err().starts_with("form.card × variants[0].size"));
    let mut bush = by_name("bush");
    let Form::Bush(form) = &mut bush.form else { unreachable!() };
    form.growth.kill = form.growth.influence + 1.0;
    assert!(bush.validate().unwrap_err().contains("form: growth.kill"));
    let text = std::fs::read_to_string(cover_dir().join("species/daisy.cover.ron")).unwrap();
    let unknown = text.replace("stems: 7,", "stems: 7, petal_count: 3,");
    assert!(sapphire_serialize::from_ron::<CoverSpecies>(&unknown).is_err(), "unknown fields are refused");
}
