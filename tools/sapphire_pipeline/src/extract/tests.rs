use sapphire_assets::material::{Matcap, Material};
use sapphire_assets::mesh::{Mesh, SlotMaterial};

use super::*;
use crate::cook::{Cook, CookOptions, CookReport, cook_dir};
use crate::import::vrm::VrmVersion;
use crate::import::vrm::samples::{self, SampleVrm};
use crate::test_util::{TempDir, read_aligned};
use crate::texture::{ColorSettings, MaskSettings, NormalSettings};

/// A content tree holding `sample` as `characters/sample.vrm`, cooked once so it has a sidecar.
fn tree(scratch: &TempDir, sample: &SampleVrm) -> PathBuf {
    let source = scratch.path().join("content/characters/sample.vrm");
    fs::create_dir_all(source.parent().unwrap()).unwrap();
    fs::write(&source, sample.to_glb()).unwrap();
    cook(scratch);
    source
}

fn cook(scratch: &TempDir) -> CookReport {
    let options = CookOptions { cook: Cook::Desktop, cache: None, locked: false };
    cook_dir(&scratch.path().join("content"), &scratch.path().join("out"), &options).expect("the tree cooks")
}

fn extracted(scratch: &TempDir, relative: &str) -> PathBuf {
    scratch.path().join("content/characters/sample").join(relative)
}

fn importer(path: &Path) -> ImportSettings {
    meta::read(&meta::sidecar_path(path)).unwrap().importer
}

fn id(path: &Path) -> AssetId {
    meta::read(&meta::sidecar_path(path)).unwrap().id
}

fn bindings(source: &Path) -> BTreeMap<String, AssetId> {
    importer(source).mesh().unwrap().materials.clone()
}

fn document(path: &Path) -> AnimeCharacterMaterial {
    sapphire_serialize::from_ron(&fs::read_to_string(path).unwrap()).unwrap()
}

#[test]
fn extraction_writes_textures_maps_and_bound_documents() {
    let scratch = TempDir::new("extract_vrm");
    let source = tree(&scratch, &samples::vrm1());
    let extraction = extract_materials(&source, &ExtractOptions::default()).unwrap();

    // Skin is a textured MToon and Lashes a textured unlit material; Hair (untextured MToon) and
    // Plain (glTF) draw as authored from their generated materials.
    let bound: Vec<&str> = extraction.bound.iter().map(|(slot, _)| slot.as_str()).collect();
    assert_eq!(bound, ["Skin", "Lashes"]);
    assert!(extraction.kept.is_empty());
    assert_eq!(bindings(&source), extraction.bound.iter().cloned().collect());

    let texture = |name: &str| extracted(&scratch, &format!("textures/{name}.png"));
    let class = |name: &str| match importer(&texture(name)) {
        ImportSettings::Texture(settings) => settings.class,
        other => panic!("{name}: {other:?}"),
    };
    assert_eq!(class("body_albedo"), TextureClass::Color(ColorSettings::default()), "base and shade");
    assert_eq!(class("body_normal"), TextureClass::Normal(NormalSettings::default()));
    assert_eq!(class("body_rim"), TextureClass::Mask(MaskSettings::default()), "rim and UV animation masks");
    assert_eq!(class("body_outline"), TextureClass::Mask(MaskSettings::default()));
    assert_eq!(class("matcap"), TextureClass::Color(ColorSettings::default()));
    // PNG images are copied byte for byte, JPEG ones decoded and written as PNG.
    let bytes = samples::vrm1().to_glb();
    let import = vrm::import(&bytes, None).unwrap();
    assert_eq!(
        fs::read(texture("body_albedo")).unwrap(),
        import.image(samples::image::ALBEDO).unwrap().bytes
    );
    let matcap =
        image::load_from_memory_with_format(&fs::read(texture("matcap")).unwrap(), image::ImageFormat::Png)
            .expect("a PNG");
    assert_eq!((matcap.width(), matcap.height()), (samples::image::SIZE, samples::image::SIZE));

    // The shading shift texture is an ILM document's input, without a sidecar of its own.
    let input = extracted(&scratch, "maps/body_shift.png");
    assert!(input.exists() && !meta::sidecar_path(&input).exists());
    let ilm_path = extracted(&scratch, "maps/body_shift.ilm.ron");
    let ilm: IlmMap = sapphire_serialize::from_ron(&fs::read_to_string(&ilm_path).unwrap()).unwrap();
    assert_eq!(
        ilm.shadow_bias,
        ChannelInput::Image { path: "body_shift.png".into(), channel: ImageChannel::R, range: [0.5, 0.75] }
    );
    assert_eq!(ilm.material_id, MaterialIdInput::Constant(0));
    assert!(matches!(importer(&ilm_path), ImportSettings::CharacterMap(MapSettings::Ilm(_))));

    // The Skin document is the mapping with the extracted textures bound.
    let skin_path = extracted(&scratch, "materials/Skin.material.ron");
    assert!(matches!(importer(&skin_path), ImportSettings::Material(_)));
    let skin = document(&skin_path);
    assert_eq!(skin.base_color_map, Some(id(&texture("body_albedo"))));
    assert_eq!(skin.normal_map, Some(id(&texture("body_normal"))));
    assert_eq!(skin.control_map, Some(id(&ilm_path)));
    assert_eq!(skin.matcap.map(|matcap: Matcap| matcap.map), Some(id(&texture("matcap"))));
    let mapped = import.slots[0].as_ref().unwrap().to_material(|_| None);
    assert_eq!(
        AnimeCharacterMaterial {
            base_color_map: None,
            normal_map: None,
            control_map: None,
            matcap: None,
            light_split: mapped.light_split,
            ..skin.clone()
        },
        AnimeCharacterMaterial { matcap: None, ..mapped }
    );
    // With its control map, the shading shift is per texel, so the threshold is the factor's.
    assert!((skin.light_split.threshold - 0.1).abs() < 1e-6, "{:?}", skin.light_split);
    let lashes = document(&extracted(&scratch, "materials/Lashes.material.ron"));
    assert_eq!(lashes.base_color_map, Some(id(&texture("body_albedo"))));
    assert_eq!(lashes.control_map, None);

    // The whole tree cooks: textures, the ILM map, the documents against them, and the mesh,
    // whose extracted slots now name their documents.
    let report = cook(&scratch);
    let mesh_output =
        report.assets.iter().find(|a| a.source == source && a.output.extension().unwrap() == "smesh");
    let mesh = Mesh::from_cooked(&read_aligned(&mesh_output.unwrap().output)).unwrap();
    for slot in &mesh.materials {
        match (slot.name.as_str(), &slot.material) {
            ("Skin" | "Lashes", SlotMaterial::Asset(bound)) => {
                assert_eq!(Some(bound), bindings(&source).get(&slot.name))
            }
            ("Hair" | "Plain", SlotMaterial::Embedded(_)) => {}
            (name, material) => panic!("slot {name}: {material:?}"),
        }
    }
    let skin_output = report.assets.iter().find(|asset| asset.source == skin_path).unwrap();
    let cooked = Material::from_cooked(&read_aligned(&skin_output.output)).unwrap();
    let bound: Vec<Option<AssetId>> = cooked.textures.iter().map(|binding| binding.texture).collect();
    assert_eq!(bound[AnimeCharacterMaterial::CONTROL], Some(id(&ilm_path)));
    assert_eq!(bound[AnimeCharacterMaterial::MATCAP], Some(id(&texture("matcap"))));
    assert!(report.assets.iter().any(|asset| asset.source == ilm_path), "the ILM map cooks");
    assert!(!report.assets.iter().any(|asset| asset.source == input), "its input is part of it");
}

#[test]
fn extraction_keeps_ids_and_edits_across_runs() {
    let scratch = TempDir::new("extract_rerun");
    let source = tree(&scratch, &samples::vrm1());
    let first = extract_materials(&source, &ExtractOptions::default()).unwrap();
    let sidecar = fs::read_to_string(meta::sidecar_path(&source)).unwrap();

    // Again: every slot it would extract is bound, so nothing is written.
    let again = extract_materials(&source, &ExtractOptions::default()).unwrap();
    assert_eq!((again.written.len(), again.bound.len()), (0, 0));
    assert_eq!(again.kept, ["Skin", "Lashes"]);
    assert_eq!(fs::read_to_string(meta::sidecar_path(&source)).unwrap(), sidecar);

    // An edited document whose binding was deleted is bound again, by its ID, unchanged.
    let skin_path = extracted(&scratch, "materials/Skin.material.ron");
    let original = fs::read_to_string(&skin_path).unwrap();
    let edited = original.replace("outline: (\n            width:", "outline: (\n            width: 2.0, //");
    assert_ne!(edited, original);
    fs::write(&skin_path, &edited).unwrap();
    let mut meta = meta::read(&meta::sidecar_path(&source)).unwrap();
    meta.importer.mesh_mut().unwrap().materials.remove("Skin");
    meta::rewrite(&meta::sidecar_path(&source), &meta).unwrap();
    let rebound = extract_materials(&source, &ExtractOptions::default()).unwrap();
    assert_eq!(rebound.bound, [first.bound[0].clone()]);
    assert_eq!(rebound.written, [meta::sidecar_path(&source)], "only the binding is new");
    assert_eq!(fs::read_to_string(&skin_path).unwrap(), edited);

    // `all` adds the untextured slots, as their generated materials.
    let all = extract_materials(&source, &ExtractOptions { all: true }).unwrap();
    let bound: Vec<&str> = all.bound.iter().map(|(slot, _)| slot.as_str()).collect();
    assert_eq!(bound, ["Hair", "Plain"]);
    let bytes = samples::vrm1().to_glb();
    let import = vrm::import(&bytes, None).unwrap();
    assert_eq!(
        document(&extracted(&scratch, "materials/Hair.material.ron")),
        import.mesh.materials[1].generated
    );
    assert_eq!(
        document(&extracted(&scratch, "materials/Plain.material.ron")),
        import.mesh.materials[2].generated
    );
    // IDs never changed.
    for (slot, id) in &first.bound {
        assert_eq!(bindings(&source)[slot], *id);
    }
    cook(&scratch);
}

#[test]
fn names_do_not_depend_on_what_a_run_extracts() {
    let listing = |scratch: &TempDir| -> [BTreeSet<String>; 3] {
        ["textures", "maps", "materials"].map(|dir| {
            let entries = fs::read_dir(extracted(scratch, dir)).unwrap();
            entries.map(|entry| entry.unwrap().file_name().into_string().unwrap()).collect()
        })
    };
    let in_one_run = {
        let scratch = TempDir::new("extract_names_once");
        let source = tree(&scratch, &samples::vrm1());
        extract_materials(&source, &ExtractOptions::default()).unwrap();
        listing(&scratch)
    };

    // The same slots over two runs: Skin is bound elsewhere for the first, then unbound.
    let scratch = TempDir::new("extract_names_twice");
    let source = tree(&scratch, &samples::vrm1());
    let sidecar = meta::sidecar_path(&source);
    let mut meta = meta::read(&sidecar).unwrap();
    meta.importer.mesh_mut().unwrap().materials.insert("Skin".into(), meta::new_id().unwrap());
    meta::rewrite(&sidecar, &meta).unwrap();
    let first = extract_materials(&source, &ExtractOptions::default()).unwrap();
    assert_eq!(first.bound.iter().map(|(slot, _)| slot.as_str()).collect::<Vec<_>>(), ["Lashes"]);
    let mut meta = meta::read(&sidecar).unwrap();
    meta.importer.mesh_mut().unwrap().materials.remove("Skin");
    meta::rewrite(&sidecar, &meta).unwrap();
    let second = extract_materials(&source, &ExtractOptions::default()).unwrap();
    assert_eq!(second.bound.iter().map(|(slot, _)| slot.as_str()).collect::<Vec<_>>(), ["Skin"]);
    assert_eq!(listing(&scratch), in_one_run);
    // The albedo both use is one texture.
    let skin = document(&extracted(&scratch, "materials/Skin.material.ron"));
    let lashes = document(&extracted(&scratch, "materials/Lashes.material.ron"));
    assert_eq!(skin.base_color_map, lashes.base_color_map);
    cook(&scratch);
}

#[test]
fn sources_sharing_a_stem_cannot_both_extract() {
    let scratch = TempDir::new("extract_shared");
    let source = tree(&scratch, &samples::vrm1());
    let other = source.with_extension("glb");
    fs::write(&other, crate::samples::mannequin_glb()).unwrap();
    let error = extract_materials(&source, &ExtractOptions::default()).unwrap_err();
    assert!(
        matches!(&error, ExtractError::SharedFolder { other: found, .. } if *found == other),
        "{error:?}"
    );
    assert!(!extracted(&scratch, "").exists(), "nothing is written");
}

#[test]
fn vrm_0_x_files_extract_too() {
    let scratch = TempDir::new("extract_vrm0");
    let source = tree(&scratch, &samples::vrm0());
    let ImportSettings::Vrm(settings) = importer(&source) else { panic!("a VRM sidecar") };
    assert_eq!(settings.version, Some(VrmVersion::V0));
    let extraction = extract_materials(&source, &ExtractOptions::default()).unwrap();
    let bound: Vec<&str> = extraction.bound.iter().map(|(slot, _)| slot.as_str()).collect();
    assert_eq!(bound, ["Skin", "Lashes"]);
    // `_ShadingGradeTexture` at rate 0.5: the bias spans [0.5, 0.75], as 1.0's scale 0.5 does.
    let ilm: IlmMap = sapphire_serialize::from_ron(
        &fs::read_to_string(extracted(&scratch, "maps/body_shift.ilm.ron")).unwrap(),
    )
    .unwrap();
    assert!(
        matches!(ilm.shadow_bias, ChannelInput::Image { range: [0.5, 0.75], .. }),
        "{:?}",
        ilm.shadow_bias
    );
    assert!(extracted(&scratch, "textures/body_outline_r.png").exists(), "0.x's own outline texture");
    cook(&scratch);
}

#[test]
fn gltf_sources_extract_their_generated_materials() {
    let scratch = TempDir::new("extract_gltf");
    let source = scratch.path().join("content/mannequin.glb");
    fs::create_dir_all(source.parent().unwrap()).unwrap();
    fs::write(&source, crate::samples::mannequin_glb()).unwrap();
    cook(&scratch);
    let none = extract_materials(&source, &ExtractOptions::default()).unwrap();
    assert!(none.bound.is_empty() && none.written.is_empty(), "glTF materials reference no textures");
    let all = extract_materials(&source, &ExtractOptions { all: true }).unwrap();
    assert_eq!(all.bound.len(), 5);
    let mesh = gltf::import_slice(&crate::samples::mannequin_glb(), None).unwrap();
    for (slot, imported) in all.bound.iter().zip(&mesh.materials) {
        assert_eq!(slot.0, imported.name);
        let path = scratch
            .path()
            .join("content/mannequin/materials")
            .join(format!("{}.material.ron", file_name(&slot.0)));
        assert_eq!(document(&path), imported.generated);
    }
    cook(&scratch);
}

#[test]
fn extraction_needs_a_mesh_with_a_sidecar() {
    let scratch = TempDir::new("extract_errors");
    let source = scratch.path().join("sample.vrm");
    fs::write(&source, samples::vrm1().to_glb()).unwrap();
    let error = extract_materials(&source, &ExtractOptions::default()).unwrap_err();
    assert!(matches!(&error, ExtractError::NoSidecar(path) if *path == source), "{error:?}");
    assert!(error.to_string().contains("cook the content tree once"), "{error}");

    let texture = scratch.path().join("albedo.png");
    fs::write(&texture, samples::png(4, |_, _| [255; 4])).unwrap();
    let meta = AssetMeta {
        id: meta::new_id().unwrap(),
        importer: ImportSettings::Texture(TextureSettings::default()),
    };
    meta::create(&meta::sidecar_path(&texture), &meta).unwrap();
    assert!(matches!(
        extract_materials(&texture, &ExtractOptions::default()),
        Err(ExtractError::NotAMesh(_))
    ));
}

#[test]
fn names_are_file_names_unique_ignoring_case() {
    assert_eq!(file_name("N00_000_00_Face_00_SKIN (Instance)"), "N00_000_00_Face_00_SKIN__Instance");
    assert_eq!(file_name("#default"), "default");
    assert_eq!(file_name("../.hidden"), "hidden");
    assert_eq!(file_name("髪 Hair"), "髪_Hair");
    assert_eq!(file_name("()"), "");
    let mut names = UniqueNames::default();
    assert_eq!(names.claim("Skin", "material"), "Skin");
    assert_eq!(names.claim("skin", "material"), "skin-2");
    assert_eq!(names.claim("SKIN", "material"), "SKIN-3");
    assert_eq!(names.claim("", "material"), "material");
    assert_eq!(names.claim("", "material"), "material-2");
}
