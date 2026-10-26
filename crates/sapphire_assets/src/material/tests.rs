use sapphire_serialize::{from_ron, to_ron};

use super::*;
use crate::texture::Component;

const TEXTURE: &str = "7c4b0a9e-3f1d-4c2a-9b8e-5d6f7a8b9c0d";
const RAMP: &str = "0e1f2a3b-4c5d-4e6f-8a9b-0c1d2e3f4a5b";

fn id(text: &str) -> AssetId {
    text.parse().unwrap()
}

/// ASTC's two-channel swizzle.
const RA01: Swizzle = Swizzle([Component::R, Component::A, Component::Zero, Component::One]);

/// One document of every family, each binding at least one texture and setting fields away
/// from their defaults.
fn documents() -> Vec<AnyMaterialDocument> {
    let character = AnimeCharacterMaterial {
        surface: CharacterSurface::Hair(HairShading {
            mask_map: Some(id(TEXTURE)),
            highlight: HairHighlight {
                style: HighlightStyle::AngelRing,
                strength: 0.6,
                ..HairHighlight::default()
            },
        }),
        base_color_map: Some(id(TEXTURE)),
        alpha: AlphaMode::Mask { cutoff: 0.4 },
        diffuse: DiffuseMode::Ramp(RampDiffuse { map: id(RAMP), mix: 0.3 }),
        regions: vec![
            CharacterRegion { ramp_row: Some(2), night_ramp_row: Some(7), ..CharacterRegion::default() },
            CharacterRegion {
                shade_color: [0.9, 0.5, 0.4],
                gi: RegionGi { sphere: 1.0, skin: 0.75 },
                rim_strength: 0.5,
                rim_width: 1.5,
                rim_overlap: 0.0,
                ..CharacterRegion::default()
            },
        ],
        rim: Rim {
            width: 3.0,
            strength: 0.6,
            threshold: 0.05,
            fresnel: 0.8,
            light_gate: 0.25,
            back_light: 1.0,
        },
        matcap: Some(Matcap { map: id(TEXTURE), strength: 0.8, shadow_strength: 0.2 }),
        outline: Outline {
            curve: OutlineCurve { near: (0.25, 1.5), mid: 2.0, far: (12.0, 0.5), cutoff: 60.0 },
            model_scale: 0.6,
            flatten: 0.05,
            albedo: 0.75,
            lighting: 0.5,
            ..Outline::default()
        },
        ..AnimeCharacterMaterial::default()
    };
    let environment = EnvironmentMaterial {
        normal_map: Some(id(TEXTURE)),
        terminator: Terminator {
            growth: 0.002,
            band: Some(TerminatorBand { map: id(RAMP), strength: 0.5 }),
            ..Terminator::default()
        },
        wrap: 0.4,
        ..EnvironmentMaterial::default()
    };
    let foliage =
        FoliageMaterial { mask_map: Some(id(TEXTURE)), canopy_occlusion: 0.7, ..FoliageMaterial::default() };
    let sky = SkyMaterial {
        clouds: CloudLayer { map: Some(id(TEXTURE)), scale: 3.0, ..CloudLayer::default() },
        ..SkyMaterial::default()
    };
    let fog = FogCardMaterial { map: Some(id(TEXTURE)), scroll: [0.01, 0.0], ..FogCardMaterial::default() };
    vec![
        AnyMaterialDocument::AnimeCharacter(character),
        AnyMaterialDocument::Environment(environment),
        AnyMaterialDocument::Foliage(foliage),
        AnyMaterialDocument::Sky(sky),
        AnyMaterialDocument::FogCard(fog),
    ]
}

/// `RA01` for every bound slot.
fn swizzles(document: &AnyMaterialDocument) -> Vec<Swizzle> {
    document
        .textures()
        .iter()
        .map(|texture| if texture.is_some() { RA01 } else { Swizzle::IDENTITY })
        .collect()
}

#[test]
fn every_family_round_trips_as_a_document() {
    let mut registry = TypeRegistry::new();
    register_types(&mut registry);
    for document in documents() {
        let text = document.to_ron().unwrap();
        let family = document.family();
        assert!(text.contains("type: \"sapphire::render::"), "{family}: {text}");
        assert_eq!(AnyMaterialDocument::from_ron(&text).unwrap(), document, "{family}");
        // Registered for loading by name, as hot reload and the editor load documents.
        let dynamic = sapphire_serialize::from_ron_dynamic(&registry, &text).unwrap();
        assert_eq!(dynamic.reflect_type_path(), text.split('"').nth(1).unwrap(), "{family}");
        assert_eq!(document.textures().len(), family.texture_slots().len(), "{family}");
    }
    let text = to_ron(&AnimeCharacterMaterial::default()).unwrap();
    assert_eq!(from_ron::<AnimeCharacterMaterial>(&text).unwrap(), AnimeCharacterMaterial::default());
}

#[test]
fn documents_may_leave_fields_out() {
    let text = format!(
        r#"(type: "sapphire::render::AnimeCharacterMaterial", version: 2, data: (
            base_color: (1.0, 0.5, 0.25, 1.0),
            base_color_map: "{TEXTURE}",
            diffuse: Bands((deep_threshold: -0.4)),
            regions: [(shade_color: (0.8, 0.4, 0.4), gi: (sphere: 1.0))],
            surface: Face((shadow_map: "{RAMP}")),
            outline: (width: 4.0),
        ))"#
    );
    let AnyMaterialDocument::AnimeCharacter(material) = AnyMaterialDocument::from_ron(&text).unwrap() else {
        panic!("an anime character material");
    };
    assert_eq!(material.base_color_map, Some(id(TEXTURE)), "`Some` may be left out");
    assert_eq!(material.diffuse, DiffuseMode::Bands(BandDiffuse { deep_threshold: Some(-0.4) }));
    assert_eq!(material.regions[0].outline_color, CharacterRegion::default().outline_color);
    assert_eq!(material.regions[0].gi, RegionGi { sphere: 1.0, skin: 0.0 }, "a face region's GI");
    assert_eq!(material.rim, Rim::default());
    assert_eq!(material.outline, Outline { width: 4.0, ..Outline::default() }, "outlines v2's settings too");
    let CharacterSurface::Face(face) = &material.surface else { panic!("a face") };
    assert_eq!(face, &FaceShading::new(id(RAMP)), "a face's settings may be left out too");
    assert_eq!((face.mask_map, face.expression_map, face.softness), (None, None, 0.02));
    assert_eq!((face.hair_shadow, face.through_hair), (HairShadow::default(), None));
    let region = material.regions[0];
    assert_eq!((region.rim_strength, region.rim_width, region.rim_overlap), (1.0, 1.0, 1.0));

    let empty = r#"(type: "sapphire::render::SkyMaterial", version: 1, data: ())"#;
    assert_eq!(
        AnyMaterialDocument::from_ron(empty).unwrap(),
        AnyMaterialDocument::Sky(SkyMaterial::default())
    );
}

#[test]
fn documents_of_other_types_and_bad_ids_are_errors() {
    let other = r#"(type: "sapphire::input::InputBindings", version: 1, data: ())"#;
    let error = AnyMaterialDocument::from_ron(other).unwrap_err();
    assert!(matches!(error, DocumentError::WrongType { .. }), "{error}");
    let too_new = r#"(type: "sapphire::render::FogCardMaterial", version: 2, data: ())"#;
    assert!(matches!(AnyMaterialDocument::from_ron(too_new), Err(DocumentError::TooNew { .. })));
    let path = r#"(type: "sapphire::render::FogCardMaterial", version: 1, data: (map: "textures/fog.png"))"#;
    let error = AnyMaterialDocument::from_ron(path).unwrap_err();
    assert!(error.to_string().contains("asset ID"), "textures are referenced by ID, never path: {error}");
}

#[test]
fn validation_names_the_field() {
    let error = |document: AnyMaterialDocument| document.validate().unwrap_err().to_string();
    let character = |edit: fn(&mut AnimeCharacterMaterial)| {
        let mut material = AnimeCharacterMaterial::default();
        edit(&mut material);
        AnyMaterialDocument::AnimeCharacter(material)
    };
    assert!(error(character(|m| m.base_color[1] = f32::NAN)).starts_with("base_color "));
    assert!(error(character(|m| m.regions.clear())).starts_with("regions "));
    assert!(error(character(|m| m.regions = vec![CharacterRegion::default(); 9])).starts_with("regions "));
    assert!(
        error(character(|m| m.regions[0].outline_color = [-1.0; 3])).starts_with("regions[0].outline_color")
    );
    assert!(error(character(|m| m.alpha = AlphaMode::Mask { cutoff: 2.0 })).starts_with("alpha.cutoff"));
    assert!(
        error(character(|m| m.diffuse = DiffuseMode::Ramp(RampDiffuse { map: id(RAMP), mix: 1.5 })))
            .starts_with("diffuse.mix")
    );
    assert!(error(character(|m| m.outline.width = -1.0)).starts_with("outline.width"));
    assert!(error(character(|m| m.outline.curve.mid = 30.0)).starts_with("outline.curve "));
    assert!(error(character(|m| m.outline.curve.far.0 = f32::INFINITY)).starts_with("outline.curve "));
    assert!(error(character(|m| m.outline.curve.cutoff = 0.0)).starts_with("outline.curve.cutoff"));
    assert!(error(character(|m| m.outline.curve.near.1 = -1.0)).starts_with("outline.curve.near scale"));
    assert!(error(character(|m| m.outline.model_scale = f32::NAN)).starts_with("outline.model_scale"));
    assert!(error(character(|m| m.outline.flatten = -0.1)).starts_with("outline.flatten"));
    assert!(error(character(|m| m.outline.albedo = 1.5)).starts_with("outline.albedo"));
    assert!(error(character(|m| m.outline.lighting = -0.5)).starts_with("outline.lighting"));
    assert!(
        error(character(|m| m.regions[0].night_ramp_row = Some(16))).starts_with("regions[0].night_ramp_row")
    );
    assert!(error(character(|m| m.regions[0].gi.sphere = 1.5)).starts_with("regions[0].gi.sphere"));
    assert!(error(character(|m| m.regions[0].gi.skin = f32::NAN)).starts_with("regions[0].gi.skin"));
    let environment = EnvironmentMaterial { roughness_range: [0.6, 0.2], ..EnvironmentMaterial::default() };
    assert!(error(AnyMaterialDocument::Environment(environment)).starts_with("roughness_range"));
    let foliage = FoliageMaterial { wrap: 3.0, ..FoliageMaterial::default() };
    assert!(error(AnyMaterialDocument::Foliage(foliage)).starts_with("wrap"));
    let sky = SkyMaterial { sun: Disc { size: f32::INFINITY, softness: 0.1 }, ..SkyMaterial::default() };
    assert!(error(AnyMaterialDocument::Sky(sky)).starts_with("sun.size"));
    let fog = FogCardMaterial { camera_fade: [3.0, 1.0], ..FogCardMaterial::default() };
    assert!(error(AnyMaterialDocument::FogCard(fog)).starts_with("camera_fade"));
    // Cooking validates too.
    let invalid = character(|m| m.light_split.softness = -0.1);
    assert!(matches!(invalid.cook(&swizzles(&invalid)), Err(MaterialError::Invalid { .. })));
}

#[test]
fn cooked_materials_round_trip_and_are_read_in_place() {
    for document in documents() {
        let family = document.family();
        let material = document.cook(&swizzles(&document)).unwrap();
        assert_eq!((material.family, material.params.len()), (family, family.params_size()));
        let bytes = material.to_cooked().unwrap();
        assert_eq!(&bytes[..4], b"SPCK");
        let archived = Material::read(&bytes).unwrap();
        assert_eq!(archived.family, family);
        assert_eq!(archived.params(), &material.params[..]);
        for (binding, texture) in archived.textures.iter().zip(document.textures()) {
            assert_eq!(binding.texture(), texture, "{family}");
            let expected = if texture.is_some() { RA01 } else { Swizzle::IDENTITY };
            assert_eq!(binding.swizzle(), expected, "{family}");
        }
        assert_eq!(Material::from_cooked(&bytes).unwrap(), material);
        // Deterministic: the same document cooks to the same bytes.
        let again = document.cook(&swizzles(&document)).unwrap().to_cooked().unwrap();
        assert!(again.as_slice() == bytes.as_slice(), "{family}");
    }
}

#[test]
fn unbound_slots_take_the_identity_swizzle() {
    let document = AnyMaterialDocument::FogCard(FogCardMaterial::default());
    let material = document.cook(&[RA01]).unwrap();
    assert_eq!(material.textures, [TextureBinding::UNBOUND]);
    let params: FogCardParams = bytemuck::pod_read_unaligned(&material.params);
    assert_eq!(params.swizzles[0], 0x688);
    assert_eq!(params.modes[0], 0, "no slot is bound");
    assert!(matches!(document.cook(&[]), Err(MaterialError::SlotCount { expected: 1, found: 0, .. })));
}

#[test]
fn cooked_materials_that_break_the_format_are_rejected() {
    let document = &documents()[0];
    let good = document.cook(&swizzles(document)).unwrap();
    let check = |edit: fn(&mut Material), expected: fn(&MaterialError) -> bool| {
        let mut material = good.clone();
        edit(&mut material);
        let error = Material::read(&material.to_cooked().unwrap()).unwrap_err();
        assert!(expected(&error), "{error}");
    };
    check(|m| m.params.push(0), |e| matches!(e, MaterialError::ParamsSize { found: 1121, .. }));
    check(
        |m| m.family = MaterialFamily::Sky,
        |e| matches!(e, MaterialError::ParamsSize { expected: 96, .. }),
    );
    check(|m| m.textures.pop().map_or((), drop), |e| matches!(e, MaterialError::SlotCount { found: 7, .. }));
    check(
        |m| m.textures[0].swizzle = *b"rgbx",
        |e| matches!(e, MaterialError::InvalidSwizzle { slot: 0, .. }),
    );
    check(
        |m| m.textures[2].swizzle = *b"ra01",
        |e| matches!(e, MaterialError::UnboundSwizzle { slot: 2, .. }),
    );
    check(|m| m.flags = 1 << 20, |e| matches!(e, MaterialError::UnknownFlags(_)));
    // The block itself: finite floats, and the bindings' bound slots and swizzles.
    check(
        |m| m.params[..4].copy_from_slice(&f32::NAN.to_le_bytes()),
        |e| matches!(e, MaterialError::NonFinite),
    );
    check(
        |m| m.textures[0] = TextureBinding::UNBOUND,
        |e| matches!(e, MaterialError::BlockMismatch("bound slots")),
    );
    check(|m| m.textures[0].swizzle = *b"rgba", |e| matches!(e, MaterialError::BlockMismatch("swizzles")));

    // bytecheck rejects an archive no material could have written: an unknown family.
    // rkyv puts the root at the end, and `family` is its first field.
    let mut bytes = good.to_cooked().unwrap();
    let at = bytes.len() - size_of::<ArchivedMaterial>();
    assert_eq!(bytes[at], MaterialFamily::AnimeCharacter as u8);
    bytes[at] = 9;
    assert!(matches!(Material::read(&bytes), Err(MaterialError::Cooked(_))));
    // A mesh is not a material.
    assert!(matches!(Material::read(&[0; 64]), Err(MaterialError::Cooked(_))));
}

#[test]
fn swizzles_pack_two_slots_to_a_word() {
    assert_eq!(swizzle_code(Swizzle::IDENTITY), 0x688);
    // r a 0 1: 0 | 3 << 3 | 4 << 6 | 5 << 9.
    assert_eq!(swizzle_code(RA01), 3 << 3 | 4 << 6 | 5 << 9);
    let words = pack_swizzles(&[Swizzle::IDENTITY, RA01, RA01]);
    assert_eq!(words, [0x688 | swizzle_code(RA01) << SWIZZLE_BITS, swizzle_code(RA01), 0, 0]);
}

#[test]
fn gpu_blocks_have_their_documented_sizes() {
    let sizes = MaterialFamily::ALL.map(MaterialFamily::params_size);
    assert_eq!(sizes, [1120, 144, 160, 96, 80]);
    assert!(sizes.iter().all(|size| size % 16 == 0), "WGSL storage arrays stride by 16");
    assert_eq!(size_of::<AnimeCharacterRegion>(), 96);
    for family in MaterialFamily::ALL {
        assert!(family.texture_slots().len() <= 8, "{family}: at most 8 slots fit the swizzle words");
        assert_eq!(MaterialFamily::ALL[family.index()], family);
    }
}

#[test]
fn the_anime_character_block_carries_every_field() {
    let AnyMaterialDocument::AnimeCharacter(document) = &documents()[0] else { unreachable!() };
    let material = Material::from_document(document, &swizzles(&documents()[0])).unwrap();
    let params: AnimeCharacterParams = bytemuck::pod_read_unaligned(&material.params);
    let bound = 1 << AnimeCharacterMaterial::BASE_COLOR
        | 1 << AnimeCharacterMaterial::RAMP
        | 1 << AnimeCharacterMaterial::SURFACE_MASK
        | 1 << AnimeCharacterMaterial::MATCAP;
    let flags = bound | AnimeCharacterParams::FLAG_RAMP | AnimeCharacterParams::FLAG_ALPHA_TEST;
    assert_eq!(
        params.modes,
        [flags, 2, AnimeCharacterParams::SURFACE_HAIR, AnimeCharacterParams::HIGHLIGHT_ANGEL_RING << 8]
    );
    assert_eq!(params.split, [0.0, 0.02, 0.0, 0.4]);
    assert_eq!(params.edges, [3.0, 0.6, 5.5, 0.0], "the rim's width in pixels and strength");
    assert_eq!(params.terms, [0.3, 0.05, 0.8, 0.2]);
    assert_eq!(params.surface[0], 0.6);
    assert_eq!((params.regions[0].shade[3], params.regions[0].deep_shade[3]), (2.0, 7.0), "ramp rows");
    // Region 1 keeps A7's rows for material ID 1: day row 1, night row 9.
    assert_eq!(params.regions[1].shade, [0.9, 0.5, 0.4, 1.0]);
    assert_eq!(params.regions[1].deep_shade[3], 9.0);
    // The GI weights ride in the subsurface and rim vectors' fourth components.
    assert_eq!((params.regions[1].subsurface[3], params.regions[1].rim[3]), (1.0, 0.75), "GI weights");
    assert_eq!((params.regions[0].subsurface[3], params.regions[0].rim[3]), (0.0, 0.0), "none by default");
    assert_eq!(params.regions[2], AnimeCharacterRegion::default(), "unused regions stay zero");
    // Outlines v2's settings follow the regions.
    assert_eq!(params.outline_curve, [0.25, 2.0, 12.0, 60.0]);
    assert_eq!(params.outline_shape, [1.5, 0.5, 0.6, 0.05]);
    assert_eq!(params.outline_color, [0.75, 0.5, 0.0, 0.0]);
    assert_eq!(std::mem::offset_of!(AnimeCharacterParams, outline_curve), 128 + 8 * 96);
    // C5's settings follow the outline's; a hair material has no hair shadow or eyes through it.
    assert_eq!(params.rim, [0.05, 0.8, 0.25, 1.0]);
    assert_eq!((params.hair_shadow, params.through_hair), ([0.0; 4], [0.0; 4]));
    assert_eq!(params.rim_regions[0], [1.0, 1.0, 1.0, 0.0], "a region's rim by default");
    assert_eq!(params.rim_regions[1], [0.5, 1.5, 0.0, 0.0]);
    assert_eq!(params.rim_regions[2], [0.0; 4], "unused regions stay zero");
    assert_eq!(std::mem::offset_of!(AnimeCharacterParams, rim), 128 + 8 * 96 + 3 * 16);
    assert_eq!(std::mem::offset_of!(AnimeCharacterParams, rim_regions), 128 + 8 * 96 + 6 * 16);
    assert_eq!(params.swizzles[0] & 0xfff, swizzle_code(RA01), "slot 0 is bound");
    assert_eq!(params.swizzles[0] >> SWIZZLE_BITS, 0x688, "slot 1 is not");
    let expected = MaterialFlags::ALPHA_TEST | MaterialFlags::OUTLINE | MaterialFlags::HAIR;
    assert_eq!(material.material_flags(), expected);
    assert!(expected.contains(MaterialFlags::HAIR) && !expected.contains(MaterialFlags::FACE));
}

#[test]
fn the_outline_curve_holds_world_width_outside_its_two_segments() {
    let curve = OutlineCurve { near: (0.5, 1.6), mid: 3.0, far: (20.0, 0.4), cutoff: 80.0 };
    let close = |a: f32, b: f32| (a - b).abs() < 1e-5;
    // The knots, and halfway along each segment.
    assert!(close(curve.scale(0.5), 1.6) && close(curve.scale(3.0), 1.0) && close(curve.scale(20.0), 0.4));
    assert!(close(curve.scale(1.75), 1.3) && close(curve.scale(11.5), 0.7));
    // Outside them the width in the world holds: pixels × distance is constant.
    assert!(close(curve.scale(0.25) * 0.25, 1.6 * 0.5) && close(curve.scale(0.05) * 0.05, 0.8));
    assert!(close(curve.scale(40.0) * 40.0, 0.4 * 20.0) && close(curve.scale(75.0) * 75.0, 8.0));
    // The cutoff's fade over its last quarter.
    assert_eq!(
        (curve.fade(10.0), curve.fade(60.0), curve.fade(80.0), curve.fade(500.0)),
        (1.0, 1.0, 0.0, 0.0)
    );
    assert!(close(curve.fade(70.0), 0.5));
    // Empty segments: a pixel width kept close up with `near` at 0, a world width everywhere
    // with all three equal (MToon's world-space outlines).
    let pixels = OutlineCurve { near: (0.0, 1.0), ..curve };
    assert!(close(pixels.scale(0.01), 1.0) && close(pixels.scale(0.0), 1.0));
    let world = OutlineCurve { near: (2.0, 1.0), mid: 2.0, far: (2.0, 1.0), cutoff: 80.0 };
    for distance in [0.25, 1.0, 2.0, 7.0, 60.0] {
        assert!(close(world.scale(distance) * distance, 2.0), "{distance}");
    }
    // The default keeps C1's constant pixel width from half a meter to the mid distance, where
    // the sandbox's character stands.
    let default = OutlineCurve::default();
    assert!([0.5, 1.0, 2.5, 3.0].iter().all(|&d| close(default.scale(d), 1.0)));
    assert!(default.scale(10.0) < 1.0 && default.scale(40.0) < default.scale(20.0));
}

#[test]
fn face_materials_bind_their_maps_and_pack_their_settings() {
    let face = FaceShading {
        mask_map: Some(id(TEXTURE)),
        expression_map: Some(id(TEXTURE)),
        softness: 0.03,
        nose_line: 0.5,
        highlight: 0.25,
        highlight_reveal: 0.1,
        blush_color: [1.0, 0.5, 0.6],
        hair_shadow: HairShadow { distance: 0.02, strength: 0.75 },
        through_hair: Some(ThroughHair { opacity: 0.6, fade_yaw: 60.0, fade_pitch: 90.0, distance: 0.1 }),
        ..FaceShading::new(id(RAMP))
    };
    let document =
        AnimeCharacterMaterial { surface: CharacterSurface::Face(face.clone()), ..Default::default() };
    let textures = document.textures();
    assert_eq!(textures[AnimeCharacterMaterial::FACE_SHADOW], Some(id(RAMP)));
    assert_eq!(textures[AnimeCharacterMaterial::SURFACE_MASK], Some(id(TEXTURE)));
    assert_eq!(textures[AnimeCharacterMaterial::EXPRESSION], Some(id(TEXTURE)));
    assert_eq!(
        AnimeCharacterMaterial::TEXTURE_SLOTS[AnimeCharacterMaterial::EXPRESSION].usage,
        TextureUsage::Mask
    );
    let any = AnyMaterialDocument::AnimeCharacter(document.clone());
    let material = Material::from_document(&document, &swizzles(&any)).unwrap();
    let params: AnimeCharacterParams = bytemuck::pod_read_unaligned(&material.params);
    assert_eq!(params.modes[2], AnimeCharacterParams::SURFACE_FACE);
    assert_eq!(params.surface, [0.03, 0.5, 0.25, 0.1]);
    assert_eq!(params.surface_color, [1.0, 0.5, 0.6, 0.0]);
    let bound = 1 << AnimeCharacterMaterial::FACE_SHADOW
        | 1 << AnimeCharacterMaterial::SURFACE_MASK
        | 1 << AnimeCharacterMaterial::EXPRESSION;
    assert_eq!(params.modes[0], bound);
    assert_eq!(params.swizzles[3] >> SWIZZLE_BITS, swizzle_code(RA01), "slot 7, the expression map's");
    assert!(material.material_flags().contains(MaterialFlags::FACE | MaterialFlags::THROUGH_HAIR));
    assert_eq!(params.hair_shadow, [0.02, 0.75, 0.0, 0.0]);
    let [opacity, yaw, pitch, distance] = params.through_hair;
    assert_eq!((opacity, distance), (0.6, 0.1));
    assert!((yaw - 0.5).abs() < 1e-6 && pitch.abs() < 1e-6, "the fades' cosines: {yaw}, {pitch}");

    // Eyes through hair needs the mask, whose B says which parts of a face draw through.
    let unmasked = FaceShading { mask_map: None, ..face };
    let document = AnimeCharacterMaterial { surface: CharacterSurface::Face(unmasked), ..Default::default() };
    assert!(!document.flags().contains(MaterialFlags::THROUGH_HAIR));
    // An eye draws all of itself through, unless its opacity is 0.
    let eye = |opacity| AnimeCharacterMaterial {
        surface: CharacterSurface::Eye(EyeShading {
            through_hair: Some(ThroughHair { opacity, ..ThroughHair::default() }),
            ..EyeShading::default()
        }),
        ..Default::default()
    };
    assert_eq!(eye(0.5).flags(), MaterialFlags::OUTLINE | MaterialFlags::EYE | MaterialFlags::THROUGH_HAIR);
    assert!(!eye(0.0).flags().contains(MaterialFlags::THROUGH_HAIR));
    let params = eye(0.5).gpu_params(&[Swizzle::IDENTITY; 8]);
    assert_eq!((params.through_hair[0], params.hair_shadow), (0.5, [0.0; 4]));

    let invalid = |edit: fn(&mut FaceShading)| {
        let mut face = FaceShading::new(id(RAMP));
        edit(&mut face);
        let document = AnimeCharacterMaterial { surface: CharacterSurface::Face(face), ..Default::default() };
        document.validate().unwrap_err().to_string()
    };
    assert!(invalid(|f| f.nose_line = -0.1).starts_with("surface.nose_line"));
    assert!(invalid(|f| f.highlight_reveal = f32::NAN).starts_with("surface.highlight_reveal"));
    assert!(invalid(|f| f.blush_color = [2.0, -1.0, 0.0]).starts_with("surface.blush_color"));
    assert!(invalid(|f| f.hair_shadow.strength = 1.5).starts_with("surface.hair_shadow.strength"));
    assert!(invalid(|f| f.hair_shadow.distance = -0.1).starts_with("surface.hair_shadow.distance"));
    let through = |edit: fn(&mut ThroughHair)| {
        let mut through = ThroughHair::default();
        edit(&mut through);
        invalid_through(through)
    };
    assert!(through(|t| t.opacity = 1.5).starts_with("surface.through_hair.opacity"));
    assert!(through(|t| t.fade_yaw = 0.0).starts_with("surface.through_hair.fade_yaw"));
    assert!(through(|t| t.fade_pitch = 200.0).starts_with("surface.through_hair.fade_pitch"));
    assert!(through(|t| t.distance = f32::NAN).starts_with("surface.through_hair.distance"));
}

fn invalid_through(through: ThroughHair) -> String {
    let face = FaceShading { through_hair: Some(through), ..FaceShading::new(id(RAMP)) };
    let document = AnimeCharacterMaterial { surface: CharacterSurface::Face(face), ..Default::default() };
    document.validate().unwrap_err().to_string()
}

#[test]
fn rims_and_regions_validate_their_ranges() {
    let invalid = |edit: fn(&mut AnimeCharacterMaterial)| {
        let mut document = AnimeCharacterMaterial::default();
        edit(&mut document);
        document.validate().unwrap_err().to_string()
    };
    assert!(invalid(|m| m.rim.width = -1.0).starts_with("rim.width"));
    assert!(invalid(|m| m.rim.threshold = f32::INFINITY).starts_with("rim.threshold"));
    assert!(invalid(|m| m.rim.fresnel = 1.5).starts_with("rim.fresnel"));
    assert!(invalid(|m| m.rim.light_gate = -0.5).starts_with("rim.light_gate"));
    assert!(invalid(|m| m.rim.back_light = 2.0).starts_with("rim.back_light"));
    assert!(invalid(|m| m.regions[0].rim_strength = -1.0).starts_with("regions[0].rim_strength"));
    assert!(invalid(|m| m.regions[0].rim_width = f32::NAN).starts_with("regions[0].rim_width"));
    assert!(invalid(|m| m.regions[0].rim_overlap = 1.5).starts_with("regions[0].rim_overlap"));
}

/// Version 1 documents (M1 A3 to C4) load: their rim's width, the share of N·V it covered,
/// becomes the Fresnel gate, and everything new takes its default.
#[test]
fn version_1_documents_upgrade() {
    let text = format!(
        r#"(type: "sapphire::render::AnimeCharacterMaterial", version: 1, data: (
            base_color: (1.0, 0.5, 0.25, 1.0),
            regions: [(shade_color: (0.8, 0.4, 0.4), rim_color: (1.0, 0.9, 0.8))],
            surface: Face((shadow_map: "{RAMP}", softness: 0.03)),
            rim: (width: 0.3, strength: 0.6),
            outline: (width: 4.0),
        ))"#
    );
    let AnyMaterialDocument::AnimeCharacter(material) = AnyMaterialDocument::from_ron(&text).unwrap() else {
        panic!("an anime character material");
    };
    assert_eq!(material.rim, Rim { fresnel: 0.3, strength: 0.6, ..Rim::default() });
    assert_eq!(material.base_color, [1.0, 0.5, 0.25, 1.0]);
    assert_eq!(material.regions[0].rim_color, [1.0, 0.9, 0.8]);
    assert_eq!(material.regions[0].rim_overlap, 1.0);
    assert_eq!(material.outline.width, 4.0);
    let CharacterSurface::Face(face) = &material.surface else { panic!("a face") };
    assert_eq!(face, &FaceShading { softness: 0.03, ..FaceShading::new(id(RAMP)) });

    // A version 1 document that leaves the rim out keeps version 1's default extent.
    let bare = r#"(type: "sapphire::render::AnimeCharacterMaterial", version: 1, data: ())"#;
    let AnyMaterialDocument::AnimeCharacter(material) = AnyMaterialDocument::from_ron(bare).unwrap() else {
        panic!("an anime character material");
    };
    assert_eq!(
        material,
        AnimeCharacterMaterial { rim: Rim { fresnel: 0.22, ..Rim::default() }, ..Default::default() }
    );
    // What's written is version 2.
    let written = AnyMaterialDocument::AnimeCharacter(material).to_ron().unwrap();
    assert!(written.contains("version: 2"), "{written}");
}
