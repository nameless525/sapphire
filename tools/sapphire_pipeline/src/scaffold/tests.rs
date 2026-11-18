use std::collections::BTreeMap;

use sapphire_assets::material::{BandDiffuse, CharacterRegion, EyeShading};

use super::*;
use crate::meta::AssetMeta;
use crate::test_util::TempDir;

/// C3's test character, built by the Blender add-on: slots `Cloth` (two regions), `Face`, `Hair` and
/// `Skin`.
fn test_character() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures/character/test_character.glb")
}

fn request(out: &Path) -> NewCharacter {
    NewCharacter { model: test_character(), out: Some(out.to_owned()), ..NewCharacter::default() }
}

/// Every file under `dir` with its bytes, by path relative to it.
fn files(dir: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    let mut found = BTreeMap::new();
    let mut dirs = vec![dir.to_owned()];
    while let Some(next) = dirs.pop() {
        for entry in fs::read_dir(&next).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                dirs.push(path);
            } else {
                found.insert(path.strip_prefix(dir).unwrap().to_owned(), fs::read(&path).unwrap());
            }
        }
    }
    found
}

#[test]
fn parts_come_from_the_importer_then_vroids_names_then_words() {
    let body = AnimeCharacterMaterial::default();
    let part = |name| Part::of(name, &body);
    assert_eq!(part("Face_00_SKIN"), Part::Face);
    assert_eq!(part("N00_000_00_Body_00_SKIN (Instance)"), Part::Skin);
    assert_eq!(part("HairBack_00_HAIR"), Part::Hair);
    assert_eq!(part("Hair_00_HAIR_01"), Part::Hair);
    assert_eq!(part("FaceMouth_00_FACE"), Part::Skin, "the mouth's inside is on the face, not its skin");
    assert_eq!(part("Tops_01_CLOTH"), Part::Cloth);
    assert_eq!(part("Shoes_01_CLOTH"), Part::Cloth);
    assert_eq!(
        (part("Face"), part("hair_front"), part("Skin"), part("body")),
        (Part::Face, Part::Hair, Part::Skin, Part::Skin)
    );
    assert_eq!((part("Cloth"), part("Belt"), part("#0")), (Part::Cloth, Part::Cloth, Part::Cloth));
    // Whole words in any case, outside VRoid's names: never a word inside another.
    assert_eq!((part("FACE"), part("x_EYE"), part("HairFront")), (Part::Face, Part::Eye, Part::Hair));
    assert_eq!((part("Surface"), part("Hairpin"), part("Bodysuit")), (Part::Cloth, Part::Cloth, Part::Cloth));
    let eye = AnimeCharacterMaterial {
        surface: CharacterSurface::Eye(EyeShading::default()),
        ..AnimeCharacterMaterial::default()
    };
    assert_eq!(Part::of("FaceBrow_00_FACE", &eye), Part::Eye, "the importer's eyes stay eyes");
    assert_eq!("face".parse::<Part>().unwrap(), Part::Face);
    assert_eq!(
        "Face".parse::<Part>().unwrap_err().to_string(),
        "\"Face\" isn't a part: skin, face, hair, eye or cloth"
    );
}

#[test]
fn a_ramp_starts_as_the_tint_the_band_mode_gave() {
    let region = |shade: [f32; 3], deep: [f32; 3]| CharacterRegion {
        shade_color: shade,
        deep_shade_color: deep,
        ..CharacterRegion::default()
    };
    let mut material = AnimeCharacterMaterial {
        regions: vec![region([1.0, 0.5, 0.0], [0.2, 0.2, 0.2])],
        ..AnimeCharacterMaterial::default()
    };
    material.light_split.threshold = 0.0;
    let colors = |ramp: &RampMap, i: usize| match &ramp.materials[i].day {
        Ramp::Stops(stops) => stops.stops.iter().map(|stop| (stop.at, stop.color.0)).collect::<Vec<_>>(),
        Ramp::Image(_) => panic!("stops"),
    };
    let two = ramp("Skin", &material, None);
    assert_eq!(two.materials[0].name, "Skin");
    // Linear 1, 0.5 and 0 are sRGB 255, 188 and 0.
    assert_eq!(colors(&two, 0), [(0.0, [255, 188, 0, 255]), (1.0, [255, 188, 0, 255])]);

    // A deep band below the split's input at -0.5 ends halfway along a ramp from -1 to the threshold, 0.
    let three = ramp("Skin", &material, Some(-0.5));
    let deep = [124, 124, 124, 255];
    assert_eq!(
        colors(&three, 0),
        [(0.0, deep), (0.5, deep), (0.5, [255, 188, 0, 255]), (1.0, [255, 188, 0, 255])],
        "a hard edge where the deep band ended"
    );
    material.regions.push(material.regions[0]);
    let rows = ramp("Cloth", &material, None);
    assert_eq!(rows.materials.iter().map(|m| m.name.as_str()).collect::<Vec<_>>(), ["Cloth 0", "Cloth 1"]);
    assert_eq!(material.diffuse, DiffuseMode::Bands(BandDiffuse::default()), "the material is untouched");
}

#[test]
fn the_test_character_scaffolds_every_slot_by_its_part() {
    let dir = TempDir::new("scaffold_parts");
    let scaffold = new_character(&request(dir.path()), &mut IdSource::seeded(1)).unwrap();
    let parts: BTreeMap<&str, Part> =
        scaffold.parts.iter().map(|(slot, part)| (slot.as_str(), *part)).collect();
    assert_eq!(
        parts,
        BTreeMap::from([
            ("Cloth", Part::Cloth),
            ("Face", Part::Face),
            ("Hair", Part::Hair),
            ("Skin", Part::Skin)
        ])
    );
    assert_eq!(scaffold.unbound_ilm, ["Cloth"], "the cloth's vertices pick its two regions");

    let meta = meta::read(&meta::sidecar_path(&scaffold.model)).unwrap();
    assert_eq!(
        meta.budget.as_ref().and_then(|budget| budget.role),
        Some(Role::Hero),
        "the glTF character preset's"
    );
    let bindings = &meta.importer.mesh().unwrap().materials;
    assert_eq!(bindings.len(), 4, "every slot bound");
    let document = |slot: &str| {
        let path = dir.path().join(format!("test_character/materials/{slot}.material.ron"));
        let material = crate::material::read(&fs::read(&path).unwrap()).unwrap();
        assert_eq!(meta::read(&meta::sidecar_path(&path)).unwrap().id, bindings[slot]);
        let sapphire_assets::material::AnyMaterialDocument::AnimeCharacter(material) = material else {
            panic!("{slot}")
        };
        material
    };
    let id_of = |name: &str| {
        meta::read(&meta::sidecar_path(&dir.path().join(format!("test_character/shading/{name}"))))
            .unwrap()
            .id
    };

    let face = document("Face");
    let CharacterSurface::Face(shading) = &face.surface else { panic!("a face: {:?}", face.surface) };
    assert_eq!(shading.shadow_map, id_of("Face.facesdf.ron"));
    assert_eq!(face.diffuse, DiffuseMode::Ramp(RampDiffuse { map: id_of("Face.ramp.ron"), mix: 1.0 }));
    assert_eq!(face.control_map, Some(id_of("Face.ilm.ron")));
    assert_eq!(face.layer_mask, None, "a body's alone");
    assert!(!dir.path().join("test_character/shading/Face.layers.ron").exists());
    assert_eq!(document("Skin").layer_mask, Some(id_of("Skin.layers.ron")));
    for angle in face::ANGLES {
        let mask = dir.path().join(format!("test_character/shading/Face{}", face::mask_suffix(angle)));
        assert!(
            mask.is_file() && !meta::sidecar_path(&mask).exists(),
            "{}: an input, no asset",
            mask.display()
        );
    }
    assert!(matches!(document("Hair").surface, CharacterSurface::Hair(_)));
    assert_eq!(document("Skin").surface, CharacterSurface::Body);
    let cloth = document("Cloth");
    assert_eq!((cloth.surface.clone(), cloth.control_map), (CharacterSurface::Body, None));
    assert!(dir.path().join("test_character/shading/Cloth.ilm.ron").is_file(), "written, to paint");
}

#[test]
fn a_second_run_changes_nothing_and_a_seed_repeats_every_byte() {
    let (first, second) = (TempDir::new("scaffold_again_a"), TempDir::new("scaffold_again_b"));
    new_character(&request(first.path()), &mut IdSource::seeded(5)).unwrap();
    let written = files(first.path());
    let again = new_character(&request(first.path()), &mut IdSource::seeded(6)).unwrap();
    assert_eq!(again.written, Vec::<PathBuf>::new());
    assert_eq!(again.kept.len(), 4);
    assert_eq!(files(first.path()), written, "every byte as it was");

    new_character(&request(second.path()), &mut IdSource::seeded(5)).unwrap();
    assert_eq!(files(second.path()), written, "the same seed, the same tree");

    // A slot whose binding is deleted binds its documents again, as they are.
    let sidecar = meta::sidecar_path(&first.path().join("test_character.glb"));
    let mut meta: AssetMeta = meta::read(&sidecar).unwrap();
    meta.importer.mesh_mut().unwrap().materials.remove("Face");
    meta::rewrite(&sidecar, &meta).unwrap();
    let rebound = new_character(&request(first.path()), &mut IdSource::seeded(7)).unwrap();
    assert_eq!(rebound.written, [sidecar]);
    assert_eq!(files(first.path()), written);
}

#[test]
fn a_clash_names_the_file_and_writes_nothing_of_its_own() {
    let dir = TempDir::new("scaffold_clash");
    // Another model under the model's name.
    fs::write(dir.path().join("test_character.glb"), b"not the model").unwrap();
    let error = new_character(&request(dir.path()), &mut IdSource::seeded(1)).unwrap_err().to_string();
    assert!(error.contains("test_character.glb is another file than"), "{error}");
    assert_eq!(files(dir.path()).len(), 1);
    fs::remove_file(dir.path().join("test_character.glb")).unwrap();

    // A texture where the last slot's ramp goes, the hair's: every slot before it would be written
    // first, but the run is planned whole before its first write.
    let ramp = dir.path().join("test_character/shading/Hair.ramp.ron");
    fs::create_dir_all(ramp.parent().unwrap()).unwrap();
    fs::write(&ramp, b"").unwrap();
    let texture =
        AssetMeta::new(IdSource::seeded(9).next_id().unwrap(), ImportSettings::Texture(Default::default()));
    meta::create(&meta::sidecar_path(&ramp), &texture).unwrap();
    let before = files(dir.path());
    let error = new_character(&request(dir.path()), &mut IdSource::seeded(1)).unwrap_err().to_string();
    assert!(
        error.contains("Hair.ramp.ron is already there as a texture source, where a ramp source would be"),
        "{error}"
    );
    assert_eq!(files(dir.path()), before, "no model, sidecar or slot written");
}

#[test]
fn a_model_naming_files_beside_it_is_refused_before_a_copy() {
    let (dir, out) = (TempDir::new("scaffold_external"), TempDir::new("scaffold_external_out"));
    let model = dir.path().join("hero.gltf");
    let json = r#"{"asset": {"version": "2.0"}, "buffers": [{"uri": "hero.bin", "byteLength": 4}],
        "images": [{"uri": "data:image/png;base64,AAAA"}, {"uri": "skin.png"}]}"#;
    fs::write(&model, json).unwrap();
    let asked = NewCharacter { model, out: Some(out.path().join("hero")), ..NewCharacter::default() };
    let error = new_character(&asked, &mut IdSource::seeded(1)).unwrap_err().to_string();
    assert!(error.contains("leave behind (hero.bin, skin.png)"), "{error}");
    assert_eq!(fs::read_dir(out.path()).unwrap().count(), 0, "nothing written, not even a folder");
    let glb = fs::read(test_character()).unwrap();
    assert_eq!(external_files(Path::new("a.glb"), &glb), Vec::<String>::new(), "the add-on's GLB holds all");
}

#[test]
fn parts_named_on_the_command_line_win_and_must_name_slots() {
    let (dir, other) = (TempDir::new("scaffold_named"), TempDir::new("scaffold_named_other"));
    let mut asked = request(other.path());
    asked.parts.insert("Belt".into(), Part::Cloth);
    let error = new_character(&asked, &mut IdSource::seeded(1)).unwrap_err().to_string();
    assert_eq!(error, "--part names slot \"Belt\", but the model's slots are Skin, Cloth, Face, Hair");
    assert_eq!(fs::read_dir(other.path()).unwrap().count(), 0, "nothing written: no model, sidecar or slot");

    let mut asked = request(dir.path());
    asked.parts.insert("Hair".into(), Part::Cloth);
    asked.role = Some(Role::Enemy);
    let scaffold = new_character(&asked, &mut IdSource::seeded(1)).unwrap();
    assert!(scaffold.parts.contains(&("Hair".to_owned(), Part::Cloth)));
    let meta = meta::read(&meta::sidecar_path(&scaffold.model)).unwrap();
    assert_eq!(meta.budget.and_then(|budget| budget.role), Some(Role::Enemy));
}
