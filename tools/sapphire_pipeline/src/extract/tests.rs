use sapphire_assets::material::{Matcap, Material};
use sapphire_assets::mesh::{Mesh, SlotMaterial};

use super::*;
use crate::cook::{Cook, CookOptions, CookReport, cook_dir};
use crate::import::vrm::VrmVersion;
use crate::import::vrm::mtoon::TextureUse;
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
    // The rim image is Skin's UV animation mask and its rim multiply texture, which its layer mask
    // holds (below).
    assert_eq!(class("body_rim"), TextureClass::Mask(MaskSettings::default()), "the UV animation mask");
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
        ChannelInput::Image {
            path: "body_shift.png".into(),
            channel: ImageChannel::R,
            range: [0.5, 0.75],
            encoding: Encoding::Linear
        }
    );
    assert_eq!(ilm.material_id, MaterialIdInput::Constant(0));
    assert!(matches!(importer(&ilm_path), ImportSettings::CharacterMap(MapSettings::Ilm(_))));

    // Skin's layer mask (M1 C7): its rim multiply texture's brightest channel, R, decoded from
    // sRGB, masks its matcap; nothing else is painted.
    let rim_input = extracted(&scratch, "maps/body_rim.png");
    assert!(rim_input.exists() && !meta::sidecar_path(&rim_input).exists());
    assert_eq!(fs::read(&rim_input).unwrap(), import.image(samples::image::RIM).unwrap().bytes);
    let layers_path = extracted(&scratch, "maps/body_rim.layers.ron");
    let layers: LayerMaskMap =
        sapphire_serialize::from_ron(&fs::read_to_string(&layers_path).unwrap()).unwrap();
    assert_eq!(
        layers,
        LayerMaskMap {
            size: None,
            matcap: ChannelInput::Image {
                path: "body_rim.png".into(),
                channel: ImageChannel::R,
                range: [0.0, 1.0],
                encoding: Encoding::Srgb,
            },
            metallic: layer_mask::unpainted(),
            emission: layer_mask::unpainted(),
            stocking: layer_mask::unpainted(),
        }
    );
    assert!(matches!(importer(&layers_path), ImportSettings::CharacterMap(MapSettings::LayerMask(_))));

    // The Skin document is the mapping with the extracted textures and maps bound.
    let skin_path = extracted(&scratch, "materials/Skin.material.ron");
    assert!(matches!(importer(&skin_path), ImportSettings::Material(_)));
    let skin = document(&skin_path);
    assert_eq!(skin.base_color_map, Some(id(&texture("body_albedo"))));
    assert_eq!(skin.normal_map, Some(id(&texture("body_normal"))));
    assert_eq!(skin.control_map, Some(id(&ilm_path)));
    assert_eq!(skin.matcap.map(|matcap: Matcap| matcap.map), Some(id(&texture("matcap"))));
    assert_eq!(skin.layer_mask, Some(id(&layers_path)));
    let mapped = import.slots[0].as_ref().unwrap().to_material(|binding| match binding {
        Binding::Texture(TextureUse::BaseColor) => Some(id(&texture("body_albedo"))),
        Binding::Texture(TextureUse::Normal) => Some(id(&texture("body_normal"))),
        Binding::Texture(TextureUse::Matcap) => Some(id(&texture("matcap"))),
        Binding::ControlMap => Some(id(&ilm_path)),
        Binding::LayerMask => Some(id(&layers_path)),
        Binding::Texture(_) => None,
    });
    assert_eq!(skin, mapped);
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
    assert_eq!(bound[AnimeCharacterMaterial::SURFACE_MASK], Some(id(&layers_path)));
    assert!(report.assets.iter().any(|asset| asset.source == ilm_path), "the ILM map cooks");
    assert!(report.assets.iter().any(|asset| asset.source == layers_path), "the layer mask cooks");
    for input in [input, rim_input] {
        assert!(!report.assets.iter().any(|asset| asset.source == input), "{input:?} is part of a map");
    }
}

#[test]
fn extraction_writes_the_glow_into_the_layer_mask() {
    let scratch = TempDir::new("extract_glow");
    // Skin glows through the outline image, (0, rising, 0): its G is the glow, and its hue
    // green. Hair glows through the same image at another strength; Lashes, unlit, doesn't.
    let mut sample = samples::vrm1();
    let materials = &mut sample.document["materials"];
    for (material, factor) in
        [(samples::material::SKIN, [1.0, 0.5, 0.25]), (samples::material::HAIR, [0.5; 3])]
    {
        materials[material]["emissiveFactor"] = serde_json::json!(factor);
        materials[material]["emissiveTexture"] = serde_json::json!({ "index": samples::image::OUTLINE });
    }
    let source = tree(&scratch, &sample);
    let extraction = extract_materials(&source, &ExtractOptions::default()).unwrap();
    let bound: Vec<&str> = extraction.bound.iter().map(|(slot, _)| slot.as_str()).collect();
    assert_eq!(bound, ["Skin", "Hair", "Lashes"], "Hair's glow is textured now");

    let read = |name: &str| -> LayerMaskMap {
        sapphire_serialize::from_ron(&fs::read_to_string(extracted(&scratch, name)).unwrap()).unwrap()
    };
    let glow = ChannelInput::Image {
        path: "body_outline.png".into(),
        channel: ImageChannel::G,
        range: [0.0, 1.0],
        encoding: Encoding::Srgb,
    };
    // Skin's mask holds its matcap's mask and its glow, named after both images; Hair's, the
    // glow alone (it has no matcap to mask).
    let skin_layers = read("maps/body_rim-body_outline.layers.ron");
    assert_eq!((&skin_layers.matcap, &skin_layers.emission), (&layers_of_rim(), &glow));
    let hair_layers = read("maps/body_outline.layers.ron");
    assert_eq!((&hair_layers.matcap, &hair_layers.emission), (&layer_mask::unpainted(), &glow));
    let skin = document(&extracted(&scratch, "materials/Skin.material.ron"));
    let hair = document(&extracted(&scratch, "materials/Hair.material.ron"));
    assert_eq!(skin.layer_mask, Some(id(&extracted(&scratch, "maps/body_rim-body_outline.layers.ron"))));
    assert_eq!(hair.layer_mask, Some(id(&extracted(&scratch, "maps/body_outline.layers.ron"))));
    // The glows take the rest of the texture's hue, steady.
    assert_eq!((skin.regions[0].emission, hair.regions[0].emission), ([0.0, 0.5, 0.0], [0.0, 0.5, 0.0]));
    assert_eq!((skin.emission.period, hair.emission.period), (0.0, 0.0));
    // Written out, for the artist to see and change.
    let text: String = fs::read_to_string(extracted(&scratch, "materials/Hair.material.ron"))
        .unwrap()
        .split_whitespace()
        .collect();
    assert!(text.contains("emission:(0.0,0.5,0.0)") && text.contains("period:0.0"), "{text}");
    // The emissive image is a map input only: no texture of its own.
    assert!(extracted(&scratch, "maps/body_outline.png").exists());
    assert!(!extracted(&scratch, "textures/body_outline-color.png").exists());

    let report = cook(&scratch);
    for layers in ["maps/body_rim-body_outline.layers.ron", "maps/body_outline.layers.ron"] {
        assert!(report.assets.iter().any(|asset| asset.source == extracted(&scratch, layers)), "{layers}");
    }
}

/// The layer mask input a sample material's rim multiply texture becomes: its R, sRGB.
fn layers_of_rim() -> ChannelInput {
    ChannelInput::Image {
        path: "body_rim.png".into(),
        channel: ImageChannel::R,
        range: [0.0, 1.0],
        encoding: Encoding::Srgb,
    }
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
fn extraction_writes_the_gi_weights_vroid_names_give() {
    use sapphire_assets::material::RegionGi;
    let scratch = TempDir::new("extract_gi");
    let mut sample = samples::vrm1();
    let materials = sample.document["materials"].as_array_mut().unwrap();
    for (material, name) in [
        (samples::material::SKIN, "Face_00_SKIN"),
        (samples::material::HAIR, "Hair_00_HAIR"),
        (samples::material::LASHES, "Body_00_SKIN"),
    ] {
        materials[material]["name"] = serde_json::json!(name);
    }
    let source = tree(&scratch, &sample);
    let extraction = extract_materials(&source, &ExtractOptions { all: true }).unwrap();
    assert_eq!(extraction.bound.len(), 4, "{:?}", extraction.bound);
    for (slot, sphere, skin) in [
        ("Face_00_SKIN", 1.0, 1.0),
        ("Hair_00_HAIR", 1.0, 0.0),
        ("Body_00_SKIN", 0.0, 1.0),
        ("Plain", 0.0, 0.0),
    ] {
        let path = extracted(&scratch, &format!("materials/{slot}.material.ron"));
        let [region] = &document(&path).regions[..] else { panic!("{slot}: one region") };
        assert_eq!(region.gi, RegionGi { sphere, skin }, "{slot}");
        // Written out, for the artist to see and change.
        let text: String = fs::read_to_string(&path).unwrap().split_whitespace().collect();
        assert!(text.contains(&format!("gi:(sphere:{sphere:?},skin:{skin:?},)")), "{slot}: {text}");
    }
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
    let meta = AssetMeta::new(meta::new_id().unwrap(), ImportSettings::Texture(TextureSettings::default()));
    meta::create(&meta::sidecar_path(&texture), &meta).unwrap();
    assert!(matches!(
        extract_materials(&texture, &ExtractOptions::default()),
        Err(ExtractError::NotAMesh(_))
    ));
}

/// A fixture `tools/blender/fbx_fixtures.py` made.
fn fbx_fixture(relative: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures/fbx").join(relative)
}

/// A content tree holding the textured FBX fixture `name` as `props/crate.fbx`, with the
/// fixtures' images beside it in `props/textures/` when `images`, cooked once.
fn fbx_tree(scratch: &TempDir, name: &str, images: bool) -> PathBuf {
    let source = scratch.path().join("content/props/crate.fbx");
    fs::create_dir_all(source.parent().unwrap().join("textures")).unwrap();
    fs::copy(fbx_fixture(name), &source).unwrap();
    if images {
        for image in ["checker.png", "bumps.png"] {
            fs::copy(
                fbx_fixture(&format!("textures/{image}")),
                source.parent().unwrap().join("textures").join(image),
            )
            .unwrap();
        }
    }
    cook(scratch);
    source
}

fn texture_class(path: &Path) -> TextureClass {
    match importer(path) {
        ImportSettings::Texture(settings) => settings.class,
        other => panic!("{}: {other:?}", path.display()),
    }
}

#[test]
fn fbx_images_beside_the_file_are_bound_where_they_lie() {
    let scratch = TempDir::new("extract_fbx_external");
    let source = fbx_tree(&scratch, "textured_external.fbx", true);
    let textures = source.parent().unwrap().join("textures");
    let (checker, bumps) = (textures.join("checker.png"), textures.join("bumps.png"));
    // The cook made both color textures.
    assert_eq!(texture_class(&bumps), TextureClass::Color(ColorSettings::default()));
    let bumps_id = id(&bumps);

    let extraction = extract_materials(&source, &ExtractOptions::default()).unwrap();
    assert_eq!(extraction.bound.iter().map(|(slot, _)| slot.as_str()).collect::<Vec<_>>(), ["Stone"]);
    assert!(extraction.conflicts.is_empty());
    let stone = document(&scratch.path().join("content/props/crate/materials/Stone.material.ron"));
    assert_eq!(stone.base_color_map, Some(id(&checker)));
    assert_eq!(stone.normal_map, Some(bumps_id));
    // The normal map's sidecar held the cook's defaults, so it is a normal map now, with its ID.
    assert_eq!(texture_class(&bumps), TextureClass::Normal(NormalSettings::default()));
    assert_eq!(id(&bumps), bumps_id);
    assert!(!scratch.path().join("content/props/crate/textures").exists(), "nothing copied");
    let generated = fbx::import(&source).unwrap().materials.remove(0).generated;
    assert_eq!(AnimeCharacterMaterial { base_color_map: None, normal_map: None, ..stone }, generated);

    // The tree cooks with the document bound.
    let report = cook(&scratch);
    let mesh_output = &report.assets.iter().find(|asset| asset.source == source).unwrap().output;
    let mesh = Mesh::from_cooked(&read_aligned(mesh_output)).unwrap();
    assert_eq!(mesh.materials[0].material, SlotMaterial::Asset(bindings(&source)["Stone"]));

    // A sidecar that says something else keeps its class, and the use is left unbound and
    // reported, so the tree still cooks.
    let scratch = TempDir::new("extract_fbx_external_mask");
    let source = fbx_tree(&scratch, "textured_external.fbx", true);
    let bumps = source.parent().unwrap().join("textures/bumps.png");
    let mask = AssetMeta::new(
        id(&bumps),
        ImportSettings::Texture(TextureSettings { class: TextureClass::Mask(MaskSettings::default()) }),
    );
    meta::rewrite(&meta::sidecar_path(&bumps), &mask).unwrap();
    let extraction = extract_materials(&source, &ExtractOptions::default()).unwrap();
    assert_eq!(extraction.conflicts, [conflict("Stone", &bumps, "normal", "mask")]);
    assert_eq!(texture_class(&bumps), TextureClass::Mask(MaskSettings::default()));
    let stone = document(&scratch.path().join("content/props/crate/materials/Stone.material.ron"));
    assert_eq!((stone.base_color_map, stone.normal_map), (Some(id(&checker_of(&source))), None));
    cook(&scratch);

    // A normal map sidecar with settings of its own is the normal map's, and stays as it is.
    let scratch = TempDir::new("extract_fbx_external_flipped");
    let source = fbx_tree(&scratch, "textured_external.fbx", true);
    let bumps = source.parent().unwrap().join("textures/bumps.png");
    let flipped = TextureClass::Normal(NormalSettings { flip_y: true, ..NormalSettings::default() });
    meta::rewrite(
        &meta::sidecar_path(&bumps),
        &AssetMeta::new(id(&bumps), ImportSettings::Texture(TextureSettings { class: flipped })),
    )
    .unwrap();
    let extraction = extract_materials(&source, &ExtractOptions::default()).unwrap();
    assert!(extraction.conflicts.is_empty(), "{:?}", extraction.conflicts);
    assert_eq!(texture_class(&bumps), flipped);
    let stone = document(&scratch.path().join("content/props/crate/materials/Stone.material.ron"));
    assert_eq!(stone.normal_map, Some(id(&bumps)));
}

/// `props/textures/checker.png` beside the FBX at `source`.
fn checker_of(source: &Path) -> PathBuf {
    source.parent().unwrap().join("textures/checker.png")
}

/// A conflict as extraction reports it: the image by its canonical path.
fn conflict(slot: &str, image: &Path, used_as: &'static str, keeps: &'static str) -> RoleConflict {
    RoleConflict { slot: slot.to_owned(), image: fs::canonicalize(image).unwrap(), used_as, keeps }
}

#[test]
fn embedded_fbx_images_are_written_out() {
    let scratch = TempDir::new("extract_fbx_embedded");
    let source = fbx_tree(&scratch, "textured_embedded.fbx", false);
    extract_materials(&source, &ExtractOptions::default()).unwrap();
    let written = scratch.path().join("content/props/crate/textures");
    let (checker, bumps) = (written.join("checker.png"), written.join("bumps.png"));
    // Byte for byte the PNGs Blender embedded.
    assert_eq!(fs::read(&checker).unwrap(), fs::read(fbx_fixture("textures/checker.png")).unwrap());
    assert_eq!(fs::read(&bumps).unwrap(), fs::read(fbx_fixture("textures/bumps.png")).unwrap());
    assert_eq!(texture_class(&checker), TextureClass::Color(ColorSettings::default()));
    assert_eq!(texture_class(&bumps), TextureClass::Normal(NormalSettings::default()));
    let stone = document(&scratch.path().join("content/props/crate/materials/Stone.material.ron"));
    assert_eq!((stone.base_color_map, stone.normal_map), (Some(id(&checker)), Some(id(&bumps))));
    cook(&scratch);
}

#[test]
fn an_fbx_image_found_nowhere_fails_extraction() {
    let scratch = TempDir::new("extract_fbx_missing");
    let source = fbx_tree(&scratch, "textured_external.fbx", false);
    let error = extract_materials(&source, &ExtractOptions::default()).unwrap_err();
    assert!(
        matches!(&error, ExtractError::MissingTexture { slot, texture } if slot == "Stone" && texture == "textures/checker.png"),
        "{error:?}"
    );
    assert!(bindings(&source).is_empty(), "nothing bound");
}

/// `fixture`'s bytes with each `(from, to)` replaced, keeping every string's length (FBX strings
/// carry their lengths), as the review's reproductions edited the textured fixture.
fn edited_fbx(fixture: &str, edits: &[(&[u8], &[u8])]) -> Vec<u8> {
    let mut bytes = fs::read(fbx_fixture(fixture)).unwrap();
    for (from, to) in edits {
        assert_eq!(from.len(), to.len());
        let mut found = 0;
        let mut at = 0;
        while let Some(offset) = bytes[at..].windows(from.len()).position(|window| window == *from) {
            bytes[at + offset..at + offset + to.len()].copy_from_slice(to);
            at += offset + to.len();
            found += 1;
        }
        assert!(found > 0, "{} names no {:?}", fixture, String::from_utf8_lossy(from));
    }
    bytes
}

/// A content tree with `fbx` at `content/<relative>`, cooked once.
fn fbx_at(scratch: &TempDir, relative: &str, fbx: &[u8]) -> PathBuf {
    let source = scratch.path().join("content").join(relative);
    fs::create_dir_all(source.parent().unwrap()).unwrap();
    fs::write(&source, fbx).unwrap();
    cook(scratch);
    source
}

/// Copies the fixtures' images into `dir`, as `names` (checker's first, then bumps').
fn copy_images(dir: &Path, names: [&str; 2]) {
    fs::create_dir_all(dir).unwrap();
    for (image, name) in ["checker.png", "bumps.png"].into_iter().zip(names) {
        fs::copy(fbx_fixture(&format!("textures/{image}")), dir.join(name)).unwrap();
    }
}

/// Every file under `dir`, relative to it.
fn listing(dir: &Path) -> BTreeSet<PathBuf> {
    let mut files = BTreeSet::new();
    let mut dirs = vec![dir.to_owned()];
    while let Some(next) = dirs.pop() {
        for entry in fs::read_dir(&next).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                dirs.push(path);
            } else {
                files.insert(path.strip_prefix(dir).unwrap().to_owned());
            }
        }
    }
    files
}

#[test]
fn fbx_images_outside_the_folder_fail_until_copied_beside_it() {
    // The fixture's images named one directory up, `../other/`: outside the content tree for an
    // FBX at its root, and inside it but outside the FBX's folder for one in `props/` (review
    // BR-003).
    let fbx = edited_fbx("textured_external.fbx", &[(b"textures/", b"../other/")]);
    for (relative, other) in [("crate.fbx", "other"), ("props/crate.fbx", "content/other")] {
        let scratch = TempDir::new("extract_fbx_outside");
        let other = scratch.path().join(other);
        copy_images(&other, ["checker.png", "bumps.png"]);
        let source = fbx_at(&scratch, relative, &fbx);
        let dir = source.parent().unwrap().to_owned();
        let (others, sidecar) = (listing(&other), fs::read(meta::sidecar_path(&source)).unwrap());

        let error = extract_materials(&source, &ExtractOptions::default()).unwrap_err();
        let ExtractError::OutsideFolder { slot, image, folder, copy } = &error else { panic!("{error:?}") };
        assert_eq!(
            (slot.as_str(), image.ends_with("other/checker.png"), folder, copy),
            ("Stone", true, &dir, &dir.join("checker.png")),
            "{relative}"
        );
        let message = error.to_string();
        assert!(message.contains("copy it to") && message.contains("MODIFIED.md"), "{message}");
        // Nothing written anywhere: beside the images, the extraction folder, the binding.
        assert_eq!(listing(&other), others, "{relative}");
        assert!(!dir.join("crate").exists(), "{relative}");
        assert_eq!(fs::read(meta::sidecar_path(&source)).unwrap(), sidecar, "{relative}");

        // Copied beside the FBX, as the error asks, they are bound there.
        copy_images(&dir, ["checker.png", "bumps.png"]);
        let extraction = extract_materials(&source, &ExtractOptions::default()).unwrap();
        assert!(extraction.conflicts.is_empty(), "{relative}");
        let stone = document(&dir.join("crate/materials/Stone.material.ron"));
        assert_eq!(stone.base_color_map, Some(id(&dir.join("checker.png"))), "{relative}");
        assert_eq!(stone.normal_map, Some(id(&dir.join("bumps.png"))), "{relative}");
        assert_eq!(texture_class(&dir.join("bumps.png")), TextureClass::Normal(NormalSettings::default()));
        assert_eq!(listing(&other), others, "{relative}");
        cook(&scratch);
    }
}

#[test]
fn fbx_images_are_bound_in_place_only_inside_the_folder() {
    let scratch = TempDir::new("extract_fbx_paths");
    let props = scratch.path().join("content/props");
    copy_images(&props.join("textures"), ["checker.png", "bumps.png"]);
    copy_images(&scratch.path().join("elsewhere"), ["checker.png", "bumps.png"]);
    let source = props.join("crate.fbx");
    let mut images = FbxImages::new(&source, BTreeMap::new()).unwrap();
    let texture = |file: &Path| FbxTexture {
        name: "checker".into(),
        file: file.to_string_lossy().replace('\\', "/"),
        embedded: None,
    };
    let inside = fs::canonicalize(props.join("textures/checker.png")).unwrap();
    // Absolute, and climbing out and back in: both inside.
    for file in [props.join("textures/checker.png"), PathBuf::from("textures/../textures/./checker.png")] {
        assert!(
            matches!(images.locate(&texture(&file)), Located::Inside(found) if found == inside),
            "{file:?}"
        );
    }
    // Absolute, and climbing out: both outside, so planning a slot that uses them fails.
    let outside = scratch.path().join("elsewhere/checker.png");
    for file in [outside.clone(), PathBuf::from("../../elsewhere/checker.png")] {
        let slot = Slot {
            name: "Stone".into(),
            generated: AnimeCharacterMaterial::default(),
            mtoon: None,
            textures: SlotTextures { base_color: Some(texture(&file)), normal: None },
        };
        let error = images.plan(&[slot], &[0]).unwrap_err();
        assert!(
            matches!(&error, ExtractError::OutsideFolder { image, .. } if image.ends_with("elsewhere/checker.png")),
            "{error:?}"
        );
        // An embedded copy is written out instead: the file isn't read.
        let embedded = FbxTexture { embedded: Some(vec![1]), ..texture(&file) };
        let slot = Slot {
            name: "Stone".into(),
            generated: AnimeCharacterMaterial::default(),
            mtoon: None,
            textures: SlotTextures { base_color: Some(embedded), normal: None },
        };
        images.plan(&[slot], &[0]).unwrap();
    }
    assert!(!meta::sidecar_path(&outside).exists());
}

#[test]
fn an_image_two_materials_use_in_two_roles_keeps_its_first() {
    // Stone's base color is Lacquer's normal map (review BR-002). The cook gave the image a
    // color sidecar: it stays one, and Lacquer's normal map is left unbound and reported.
    let scratch = TempDir::new("extract_fbx_shared");
    let source = fbx_tree(&scratch, "textured_shared.fbx", true);
    let (checker, bumps) = (checker_of(&source), source.parent().unwrap().join("textures/bumps.png"));
    let sidecar = fs::read(meta::sidecar_path(&checker)).unwrap();
    let extraction = extract_materials(&source, &ExtractOptions::default()).unwrap();
    assert_eq!(
        extraction.bound.iter().map(|(slot, _)| slot.as_str()).collect::<Vec<_>>(),
        ["Stone", "Lacquer"]
    );
    assert_eq!(extraction.conflicts, [conflict("Lacquer", &checker, "normal", "color")]);
    assert_eq!(fs::read(meta::sidecar_path(&checker)).unwrap(), sidecar, "the sidecar keeps its bytes");
    // Bumps is Stone's normal map only, so the cook's default sidecar is switched.
    assert_eq!(texture_class(&bumps), TextureClass::Normal(NormalSettings::default()));
    let materials = source.parent().unwrap().join("crate/materials");
    let stone = document(&materials.join("Stone.material.ron"));
    assert_eq!((stone.base_color_map, stone.normal_map), (Some(id(&checker)), Some(id(&bumps))));
    let lacquer = document(&materials.join("Lacquer.material.ron"));
    assert_eq!((lacquer.base_color_map, lacquer.normal_map), (None, None));
    cook(&scratch);
    // Again: nothing changes.
    let again = extract_materials(&source, &ExtractOptions::default()).unwrap();
    assert_eq!((again.written.len(), again.kept.len()), (0, 2));
    assert_eq!(fs::read(meta::sidecar_path(&checker)).unwrap(), sidecar);

    // Without sidecars, the image's first role is color, as its sidecar is created.
    let scratch = TempDir::new("extract_fbx_shared_fresh");
    let source = fbx_tree(&scratch, "textured_shared.fbx", true);
    let checker = checker_of(&source);
    for image in [&checker, &source.parent().unwrap().join("textures/bumps.png")] {
        fs::remove_file(meta::sidecar_path(image)).unwrap();
    }
    let extraction = extract_materials(&source, &ExtractOptions::default()).unwrap();
    assert_eq!(extraction.conflicts, [conflict("Lacquer", &checker, "normal", "color")]);
    assert_eq!(texture_class(&checker), TextureClass::Color(ColorSettings::default()));
    cook(&scratch);
}

#[test]
fn one_image_named_two_ways_is_one_image() {
    // One material's base color `textures/./image.png` and normal map `textures/image.png`.
    let scratch = TempDir::new("extract_fbx_alias");
    let fbx = edited_fbx(
        "textured_external.fbx",
        &[(b"checker.png", b"./image.png"), (b"bumps.png", b"image.png")],
    );
    let props = scratch.path().join("content/props");
    copy_images(&props.join("textures"), ["image.png", "unused.png"]);
    let source = fbx_at(&scratch, "props/crate.fbx", &fbx);
    let image = props.join("textures/image.png");
    let sidecar = fs::read(meta::sidecar_path(&image)).unwrap();
    let extraction = extract_materials(&source, &ExtractOptions::default()).unwrap();
    assert_eq!(extraction.conflicts, [conflict("Stone", &image, "normal", "color")]);
    assert_eq!(fs::read(meta::sidecar_path(&image)).unwrap(), sidecar);
    let stone = document(&props.join("crate/materials/Stone.material.ron"));
    assert_eq!((stone.base_color_map, stone.normal_map), (Some(id(&image)), None));
    cook(&scratch);
}

#[test]
fn an_image_another_material_binds_keeps_its_class() {
    // A material beside the FBX binds Stone's normal map image as its base color: the cook's
    // default sidecar isn't switched under it.
    let scratch = TempDir::new("extract_fbx_elsewhere");
    let source = fbx_tree(&scratch, "textured_external.fbx", true);
    let bumps = source.parent().unwrap().join("textures/bumps.png");
    let painted =
        AnimeCharacterMaterial { base_color_map: Some(id(&bumps)), ..AnimeCharacterMaterial::default() };
    fs::write(source.with_file_name("painted.material.ron"), sapphire_serialize::to_ron(&painted).unwrap())
        .unwrap();
    cook(&scratch);
    let sidecar = fs::read(meta::sidecar_path(&bumps)).unwrap();
    let extraction = extract_materials(&source, &ExtractOptions::default()).unwrap();
    assert_eq!(extraction.conflicts, [conflict("Stone", &bumps, "normal", "color")]);
    assert_eq!(fs::read(meta::sidecar_path(&bumps)).unwrap(), sidecar);
    cook(&scratch);
}

#[test]
fn embedded_images_used_in_two_roles_are_written_once_a_role() {
    let scratch = TempDir::new("extract_fbx_shared_embedded");
    let source = fbx_tree(&scratch, "textured_shared_embedded.fbx", false);
    let extraction = extract_materials(&source, &ExtractOptions::default()).unwrap();
    assert!(extraction.conflicts.is_empty());
    let written = scratch.path().join("content/props/crate/textures");
    let [color, normal, bumps] =
        ["checker-color.png", "checker-normal.png", "bumps.png"].map(|name| written.join(name));
    for copy in [&color, &normal] {
        assert_eq!(fs::read(copy).unwrap(), fs::read(fbx_fixture("textures/checker.png")).unwrap());
    }
    assert_eq!(texture_class(&color), TextureClass::Color(ColorSettings::default()));
    assert_eq!(texture_class(&normal), TextureClass::Normal(NormalSettings::default()));
    let materials = scratch.path().join("content/props/crate/materials");
    let stone = document(&materials.join("Stone.material.ron"));
    assert_eq!((stone.base_color_map, stone.normal_map), (Some(id(&color)), Some(id(&bumps))));
    assert_eq!(document(&materials.join("Lacquer.material.ron")).normal_map, Some(id(&normal)));
    cook(&scratch);
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
