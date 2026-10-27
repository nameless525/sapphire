use sapphire_serialize::{from_ron, to_ron};

use super::*;
use crate::character_maps::layer_mask;
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
            transmission: HairTransmission { strength: 0.5, sharpness: 6.0 },
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
        emission: [1.2, 0.6, 0.2],
        emission_steady: 0.25,
        ..EnvironmentMaterial::default()
    };
    let foliage =
        FoliageMaterial { mask_map: Some(id(TEXTURE)), canopy_occlusion: 0.7, ..FoliageMaterial::default() };
    let sky = SkyMaterial {
        stars: Stars { nebula: Some(id(TEXTURE)), ..Stars::default() },
        clouds: CloudLayer {
            map: Some(id(TEXTURE)),
            scale: 3.0,
            shading: CloudShading::Lit,
            ..CloudLayer::default()
        },
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

/// The nebula's slot and the card clouds' shading (M1 X1 part 2e) reach the sky's block, and a
/// sky that uses neither writes its document as before them.
#[test]
fn the_sky_block_packs_its_nebula_and_lit_clouds() {
    let swizzles = [Swizzle::IDENTITY; SkyMaterial::TEXTURE_SLOTS.len()];
    let painted = SkyMaterial::default();
    let text = to_ron(&painted).unwrap();
    assert!(!text.contains("nebula") && !text.contains("shading"), "{text}");
    assert_eq!(painted.gpu_params(&swizzles).modes[..2], [0, 0]);
    let lit = SkyMaterial {
        stars: Stars { nebula: Some(id(TEXTURE)), ..Stars::default() },
        clouds: CloudLayer { shading: CloudShading::Lit, ..CloudLayer::default() },
        ..SkyMaterial::default()
    };
    assert_eq!(SkyMaterial::TEXTURE_SLOTS[SkyMaterial::NEBULA].name, "nebula");
    let modes = lit.gpu_params(&swizzles).modes;
    assert_eq!(modes[..2], [1 << SkyMaterial::NEBULA, SkyParams::LIT_CLOUDS]);
    assert!(to_ron(&lit).unwrap().contains("shading: Lit"));
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
    for steady in [-0.1, 1.5, f32::NAN] {
        let environment = EnvironmentMaterial { emission_steady: steady, ..EnvironmentMaterial::default() };
        assert!(error(AnyMaterialDocument::Environment(environment)).starts_with("emission_steady"));
    }
    let foliage = FoliageMaterial { wrap: 3.0, ..FoliageMaterial::default() };
    assert!(error(AnyMaterialDocument::Foliage(foliage)).starts_with("wrap"));
    let sky = SkyMaterial { sun: Disc { size: f32::INFINITY, softness: 0.1 }, ..SkyMaterial::default() };
    assert!(error(AnyMaterialDocument::Sky(sky)).starts_with("sun.size"));
    let lit = CloudLayer { shading: CloudShading::Lit, ..CloudLayer::default() };
    let sky = SkyMaterial { cirrus: lit, ..SkyMaterial::default() };
    assert!(error(AnyMaterialDocument::Sky(sky)).starts_with("cirrus.shading"), "the cirrus is painted");
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
    check(|m| m.params.push(0), |e| matches!(e, MaterialError::ParamsSize { found: 1585, .. }));
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
    assert_eq!(sizes, [1584, 144, 160, 96, 80]);
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
    // The matcap's regions take it as MToon does by default: its layer is on (M1 C7).
    let flags = bound
        | AnimeCharacterParams::FLAG_RAMP
        | AnimeCharacterParams::FLAG_ALPHA_TEST
        | AnimeCharacterParams::FLAG_MATCAP;
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
    // Its regions' highlight colors turn the layers on (M1 C7).
    let expected =
        MaterialFlags::ALPHA_TEST | MaterialFlags::OUTLINE | MaterialFlags::HAIR | MaterialFlags::LAYERS;
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
        hair_shadow: HairShadow { distance: 0.02, strength: 0.75, softness: 0.01 },
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
    assert_eq!(params.hair_shadow, [0.02, 0.75, 0.01, 0.0]);
    // A block edited after packing, through a copy out of the archive, as the sandbox derives
    // softened faces.
    let cooked = material.to_cooked().unwrap();
    let mut copy = Material::read(&cooked).unwrap().to_material().unwrap();
    assert_eq!(copy, material);
    assert!(copy.edit_anime_character(|params| params.hair_shadow[2] = 0.05));
    let edited: AnimeCharacterParams = bytemuck::pod_read_unaligned(&copy.params);
    assert_eq!(edited, AnimeCharacterParams { hair_shadow: [0.02, 0.75, 0.05, 0.0], ..params });
    let mut other = Material { family: MaterialFamily::Environment, ..copy.clone() };
    assert!(!other.edit_anime_character(|params| params.hair_shadow[2] = 1.0), "another family");
    assert_eq!(other.params, copy.params, "left as it was");
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
    assert!(invalid(|f| f.hair_shadow.softness = f32::NAN).starts_with("surface.hair_shadow.softness"));
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

/// Eye layers (M1 C6): the layer and its settings pack into the block's eye fields, overlays
/// blend and the eye doesn't, and an eye document from before the layers cooks to the same
/// block it did.
#[test]
fn eye_layers_pack_their_settings_and_blend_over_the_eye() {
    let eye = |layer, alpha| AnimeCharacterMaterial {
        surface: CharacterSurface::Eye(EyeShading {
            layer,
            parallax: 0.002,
            iris: Some(IrisShape { center: [0.4, 0.55], radius: [0.3, 0.35] }),
            through_hair: Some(ThroughHair::default()),
            ..EyeShading::default()
        }),
        alpha,
        outline: Outline { width: 0.0, ..Outline::default() },
        ..Default::default()
    };
    let identity = [Swizzle::IDENTITY; 8];
    let params = eye(EyeLayer::Iris, AlphaMode::Blend).gpu_params(&identity);
    assert_eq!(params.surface, [0.002, 0.0, 0.0, 0.0]);
    assert_eq!(params.surface_color, [0.4, 0.55, 0.3, 0.35]);
    assert_eq!(AnimeCharacterParams::eye_layer(params.modes), AnimeCharacterParams::EYE_LAYER_IRIS);
    let motion = HighlightMotion { drift: 0.001, drift_rate: 0.5, sparkle: 0.25, sparkle_rate: 2.0 };
    let highlight = eye(EyeLayer::Highlight(motion), AlphaMode::Blend);
    let params = highlight.gpu_params(&identity);
    assert_eq!(params.surface_color, [0.001, 0.5, 0.25, 2.0], "a highlight's motion, not the iris");
    assert_eq!(params.modes[3], AnimeCharacterParams::EYE_LAYER_HIGHLIGHT << 16);
    // Overlays are transparent eyes that draw through hair over their eye; the eye isn't.
    assert_eq!(
        highlight.flags(),
        MaterialFlags::EYE | MaterialFlags::TRANSPARENT | MaterialFlags::THROUGH_HAIR
    );
    let shadow = eye(EyeLayer::Shadow, AlphaMode::Blend).gpu_params(&identity);
    assert_eq!((shadow.surface_color, AnimeCharacterParams::eye_layer(shadow.modes)), ([0.0; 4], 1));
    // Drawing order is the layers' order.
    let order =
        [EyeLayer::Eye, EyeLayer::Shadow, EyeLayer::Iris, EyeLayer::Highlight(motion)].map(EyeLayer::index);
    assert_eq!(order, [0, 1, 2, 3]);
    // Another surface reads as the eye itself, whatever its high bits hold.
    let hair = AnimeCharacterMaterial {
        surface: CharacterSurface::Hair(HairShading {
            highlight: HairHighlight { style: HighlightStyle::KajiyaKay, ..HairHighlight::default() },
            ..HairShading::default()
        }),
        ..Default::default()
    };
    let params = hair.gpu_params(&identity);
    assert_eq!((params.modes[3] >> 8, AnimeCharacterParams::eye_layer(params.modes)), (2, 0));

    // Hair's transmission (M1 C8) packs into the surface's and its color's w, and off packs
    // nothing, whatever its sharpness, so hair from before it packs as it did.
    assert_eq!((params.surface[3], params.surface_color[3]), (0.0, 0.0), "off by default");
    let transmitting = |transmission| AnimeCharacterMaterial {
        surface: CharacterSurface::Hair(HairShading { transmission, ..HairShading::default() }),
        ..Default::default()
    };
    let on = transmitting(HairTransmission { strength: 0.75, sharpness: 3.0 }).gpu_params(&identity);
    assert_eq!((on.surface[3], on.surface_color[3]), (0.75, 3.0));
    let off = transmitting(HairTransmission { strength: 0.0, sharpness: 3.0 }).gpu_params(&identity);
    assert_eq!((off.surface[3], off.surface_color[3]), (0.0, 0.0));
    let written = to_ron(&transmitting(HairTransmission::default())).unwrap();
    assert!(!written.contains("transmission"), "{written}");
    for (transmission, field) in [
        (HairTransmission { strength: -1.0, sharpness: 4.0 }, "surface.transmission.strength"),
        (HairTransmission { strength: 1.0, sharpness: 0.0 }, "surface.transmission.sharpness"),
        (HairTransmission { strength: 1.0, sharpness: f32::INFINITY }, "surface.transmission.sharpness"),
    ] {
        let error = transmitting(transmission).validate().unwrap_err().to_string();
        assert!(error.starts_with(field), "{error}");
    }

    // An eye material from before C6 (the eye layer, no iris) packs as it did: nothing in the
    // eye's new fields, so its cooked block is unchanged.
    let before = AnimeCharacterMaterial {
        surface: CharacterSurface::Eye(EyeShading { parallax: 0.0, ..EyeShading::default() }),
        ..Default::default()
    };
    let params = before.gpu_params(&identity);
    assert_eq!((params.surface, params.surface_color, params.modes[3]), ([0.0; 4], [0.0; 4], 0));
    let text = r#"(type: "sapphire::render::AnimeCharacterMaterial", version: 2, data: (
        surface: Eye((mask_map: None, parallax: 0.0, through_hair: None)),
    ))"#;
    let AnyMaterialDocument::AnimeCharacter(read) = AnyMaterialDocument::from_ron(text).unwrap() else {
        panic!("an anime character material");
    };
    assert_eq!(read.surface, before.surface, "C5's eye documents read as the eye layer");

    // Overlays must blend and the eye mustn't; the iris needs a positive size and the depth
    // and motion can't be negative.
    let error = |material: AnimeCharacterMaterial| material.validate().unwrap_err().to_string();
    assert!(error(eye(EyeLayer::Iris, AlphaMode::Opaque)).starts_with("alpha"));
    assert!(error(eye(EyeLayer::Eye, AlphaMode::Blend)).starts_with("alpha"));
    assert!(eye(EyeLayer::Eye, AlphaMode::Mask { cutoff: 0.5 }).validate().is_ok());
    let edited = |edit: fn(&mut EyeShading)| {
        let mut material = eye(EyeLayer::Iris, AlphaMode::Blend);
        let CharacterSurface::Eye(shading) = &mut material.surface else { unreachable!() };
        edit(shading);
        error(material)
    };
    assert!(edited(|e| e.parallax = -0.001).starts_with("surface.parallax"));
    assert!(
        edited(|e| e.iris = Some(IrisShape { radius: [0.0, 0.2], ..IrisShape::default() }))
            .starts_with("surface.iris.radius")
    );
    assert!(
        edited(|e| e.iris = Some(IrisShape { center: [f32::NAN, 0.5], ..IrisShape::default() }))
            .starts_with("surface.iris.center")
    );
    assert!(
        edited(|e| e.layer = EyeLayer::Highlight(HighlightMotion { sparkle: -1.0, ..Default::default() }))
            .starts_with("surface.layer.sparkle")
    );
}

/// A body material wearing every layer (M1 C7): a layer mask, a highlight, a tinted grayscale
/// matcap channel multiplying the albedo, a reflective region, stockings with a weave, and a
/// breathing glow.
fn layered() -> AnimeCharacterMaterial {
    let metal_matcap =
        RegionMatcap { channel: MatcapChannel::G, blend: MatcapBlend::Multiply, color: [1.5, 1.2, 0.8] };
    AnimeCharacterMaterial {
        layer_mask: Some(id(TEXTURE)),
        matcap: Some(Matcap { map: id(RAMP), strength: 1.2, shadow_strength: 0.4 }),
        specular: Specular { model: SpecularModel::Ggx, softness: 0.02, albedo: 0.75 },
        regions: vec![
            CharacterRegion {
                specular_color: [0.5, 0.4, 0.3],
                specular_size: 0.25,
                matcap: metal_matcap,
                metallic: 0.95,
                roughness: 0.2,
                ..CharacterRegion::default()
            },
            CharacterRegion {
                matcap: RegionMatcap { channel: MatcapChannel::Off, ..RegionMatcap::default() },
                stocking: 0.8,
                emission: [2.0, 0.5, 0.25],
                ..CharacterRegion::default()
            },
        ],
        stockings: Stockings {
            color: [0.02, 0.01, 0.03],
            edge_opacity: 0.9,
            facing_opacity: 0.3,
            range: [0.2, 0.7],
            sheen: [0.3, 0.3, 0.35],
            sheen_sharpness: 6.0,
            direction: FabricDirection::U,
            weave_scale: 400.0,
            weave_strength: 0.4,
        },
        emission: Emission { period: 2.5, depth: 0.5, bloom: 0.75 },
        ..AnimeCharacterMaterial::default()
    }
}

#[test]
fn layers_pack_after_the_rim_settings_and_set_their_flags() {
    let document = layered();
    let swizzles = vec![Swizzle::IDENTITY; AnimeCharacterMaterial::TEXTURE_SLOTS.len()];
    let material = Material::from_document(&document, &swizzles).unwrap();
    let params: AnimeCharacterParams = bytemuck::pod_read_unaligned(&material.params);
    // A body binds the layer mask in the surface mask slot.
    let bound = 1 << AnimeCharacterMaterial::SURFACE_MASK | 1 << AnimeCharacterMaterial::MATCAP;
    assert_eq!(material.textures[AnimeCharacterMaterial::SURFACE_MASK].texture, Some(id(TEXTURE)));
    assert_eq!(params.modes[0], bound | AnimeCharacterParams::FLAG_LAYERS);
    assert_eq!(params.modes[3], AnimeCharacterParams::SPECULAR_GGX);
    assert_eq!(params.terms[1], 0.02, "the highlight's softness");
    assert_eq!(params.layers, [0.75, 0.4, 0.5, 0.75], "albedo share, breaths a second, depth, bloom");
    assert_eq!(params.stocking_color, [0.02, 0.01, 0.03, 0.9]);
    assert_eq!(params.stocking_shape, [0.2, 0.7, 0.3, 400.0]);
    assert_eq!(params.stocking_sheen, [0.3, 0.3, 0.35, 6.0]);
    assert_eq!(params.stocking_weave, [0.4, 0.0, 0.0, 0.0], "fabric along u");
    let [metal, legs] = [params.layer_regions[0], params.layer_regions[1]];
    assert_eq!(metal.matcap, [1.5, 1.2, 0.8, 2.0], "channel G");
    assert_eq!(metal.surface, [0.95, 0.2, 0.0, 1.0], "metallic, roughness, no stockings, multiplying");
    assert_eq!(legs.matcap[3], -1.0, "no matcap");
    assert_eq!(legs.surface, [0.0, 0.3, 0.8, 0.0]);
    assert_eq!(legs.emission, [2.0, 0.5, 0.25, 0.0]);
    assert_eq!(params.layer_regions[2], CharacterLayerRegion::default(), "unused regions stay zero");
    assert_eq!(params.regions[0].specular, [0.5, 0.4, 0.3, 0.25]);
    // They follow C5's settings, so every earlier offset holds.
    let rim_regions = 128 + 8 * 96 + 6 * 16;
    assert_eq!(std::mem::offset_of!(AnimeCharacterParams, layers), rim_regions + 8 * 16);
    assert_eq!(std::mem::offset_of!(AnimeCharacterParams, layer_regions), rim_regions + 8 * 16 + 5 * 16);
    assert_eq!(size_of::<CharacterLayerRegion>(), 48);
    assert_eq!(material.material_flags(), MaterialFlags::OUTLINE | MaterialFlags::LAYERS, "the layers' bit");
}

#[test]
fn each_layer_turns_on_alone_and_none_by_default() {
    let flags = |document: &AnimeCharacterMaterial| {
        document.gpu_params(&[Swizzle::IDENTITY; 8]).modes[0] & AnimeCharacterParams::FLAG_LAYERS
    };
    let with_region = |region: CharacterRegion| AnimeCharacterMaterial {
        regions: vec![region],
        ..AnimeCharacterMaterial::default()
    };
    // Materials from before the layers set none of their flags, whatever their stockings and
    // emission settings say, until a region takes them.
    assert_eq!(flags(&AnimeCharacterMaterial::default()), 0);
    let mut stockings = AnimeCharacterMaterial::default();
    stockings.stockings.weave_scale = 300.0;
    assert_eq!(flags(&stockings), 0);
    let region = CharacterRegion::default();
    let turned_on = [
        (CharacterRegion { specular_color: [0.1; 3], ..region }, AnimeCharacterParams::FLAG_SPECULAR),
        (CharacterRegion { metallic: 0.91, ..region }, AnimeCharacterParams::FLAG_REFLECTION),
        (CharacterRegion { stocking: 0.1, ..region }, AnimeCharacterParams::FLAG_STOCKINGS),
        (CharacterRegion { emission: [0.0, 0.0, 0.5], ..region }, AnimeCharacterParams::FLAG_EMISSION),
    ];
    for (region, flag) in turned_on {
        let document = with_region(region);
        assert_eq!(flags(&document), flag, "{region:?}");
        assert!(document.flags().contains(MaterialFlags::LAYERS), "the permutation bit: {region:?}");
    }
    assert!(!AnimeCharacterMaterial::default().flags().contains(MaterialFlags::LAYERS));
    // Metallic at the threshold doesn't reflect: only texels above it do.
    let at = with_region(CharacterRegion { metallic: layer_mask::REFLECTS, ..region });
    assert_eq!(flags(&at), 0);
    // The matcap needs a map, some strength and a region that takes it.
    let matcap = Matcap { map: id(TEXTURE), strength: 1.0, shadow_strength: 0.0 };
    let with = AnimeCharacterMaterial { matcap: Some(matcap), ..AnimeCharacterMaterial::default() };
    assert_eq!(flags(&with), AnimeCharacterParams::FLAG_MATCAP);
    let dark = AnimeCharacterMaterial { matcap: Some(Matcap { strength: 0.0, ..matcap }), ..with.clone() };
    assert_eq!(flags(&dark), 0);
    let off_region = CharacterRegion {
        matcap: RegionMatcap { channel: MatcapChannel::Off, ..RegionMatcap::default() },
        ..region
    };
    let off = AnimeCharacterMaterial { regions: vec![off_region], ..with };
    assert_eq!(flags(&off), 0);
}

#[test]
fn layers_validate_their_ranges() {
    let invalid = |edit: fn(&mut AnimeCharacterMaterial)| {
        let mut document = layered();
        edit(&mut document);
        document.validate().unwrap_err().to_string()
    };
    assert!(layered().validate().is_ok());
    assert!(invalid(|m| m.specular.albedo = 1.5).starts_with("specular.albedo"));
    assert!(invalid(|m| m.regions[0].metallic = -0.1).starts_with("regions[0].metallic"));
    assert!(invalid(|m| m.regions[0].roughness = 2.0).starts_with("regions[0].roughness"));
    assert!(invalid(|m| m.regions[1].stocking = f32::NAN).starts_with("regions[1].stocking"));
    assert!(invalid(|m| m.regions[1].emission = [-1.0, 0.0, 0.0]).starts_with("regions[1].emission"));
    assert!(
        invalid(|m| m.regions[0].matcap.color = [f32::INFINITY; 3]).starts_with("regions[0].matcap.color")
    );
    assert!(invalid(|m| m.stockings.range = [0.8, 0.2]).starts_with("stockings.range"));
    assert!(invalid(|m| m.stockings.facing_opacity = 1.5).starts_with("stockings.facing_opacity"));
    assert!(invalid(|m| m.stockings.sheen_sharpness = 0.5).starts_with("stockings.sheen_sharpness"));
    assert!(invalid(|m| m.stockings.weave_scale = -1.0).starts_with("stockings.weave_scale"));
    assert!(invalid(|m| m.emission.depth = 2.0).starts_with("emission.depth"));
    assert!(invalid(|m| m.emission.period = -1.0).starts_with("emission.period"));
    assert!(invalid(|m| m.emission.bloom = 1.5).starts_with("emission.bloom"));
    // Faces, hair and eyes bind their own masks in the layer mask's slot.
    let face =
        |m: &mut AnimeCharacterMaterial| m.surface = CharacterSurface::Face(FaceShading::new(id(RAMP)));
    assert!(invalid(face).starts_with("layer_mask"));
}

#[test]
fn layer_references_match_their_shapes() {
    // Both highlight models peak at 1 facing the half vector and fall off monotonically, GGX's
    // tail longer than a Blinn lobe of about its width.
    for (model, size) in [(SpecularModel::Blinn, 64.0), (SpecularModel::Ggx, 0.2)] {
        assert!((model.shape(1.0, size) - 1.0).abs() < 1e-6, "{model:?}");
        let samples: Vec<f32> = (0..=20).map(|i| model.shape(1.0 - i as f32 * 0.05, size)).collect();
        assert!(samples.windows(2).all(|pair| pair[1] <= pair[0]), "{model:?}: {samples:?}");
    }
    assert!(SpecularModel::Ggx.shape(0.8, 0.2) > SpecularModel::Blinn.shape(0.8, 64.0));
    let floor = SpecularModel::MIN_GGX_ROUGHNESS;
    assert_eq!(SpecularModel::Ggx.shape(0.9, 0.0), SpecularModel::Ggx.shape(0.9, floor));
    // The denier: the edge's opacity at and below the range, facing's at and above it.
    let stockings =
        Stockings { edge_opacity: 0.9, facing_opacity: 0.3, range: [0.2, 0.7], ..Stockings::default() };
    assert_eq!(stockings.opacity(0.0), 0.9);
    assert_eq!(stockings.opacity(1.0), 0.3);
    assert!((stockings.opacity(0.45) - 0.6).abs() < 1e-6, "halfway");
    // A breath: full at its top, dimmed by the depth half a period later, steady without one.
    let emission = Emission { period: 4.0, depth: 0.4, bloom: 1.0 };
    assert!((emission.level(0.0, 0.0) - 1.0).abs() < 1e-6);
    assert!((emission.level(2.0, 0.0) - 0.6).abs() < 1e-6);
    let shifted = emission.level(1.0, std::f32::consts::PI);
    assert!((shifted - emission.level(3.0, 0.0)).abs() < 1e-6, "the phase");
    assert_eq!(Emission { period: 0.0, ..emission }.level(1.7, 0.3), 1.0);
}

/// Documents from before the layers (M1 C7) load unchanged and draw as they did: no layer flag,
/// and a body's surface mask slot unbound.
#[test]
fn documents_from_before_the_layers_take_none() {
    let text = r#"(type: "sapphire::render::AnimeCharacterMaterial", version: 2, data: (
        regions: [(shade_color: (0.8, 0.4, 0.4), specular_color: (0.0, 0.0, 0.0), specular_size: 32.0)],
        specular: (model: Blinn, softness: 0.05),
        matcap: None,
    ))"#;
    let AnyMaterialDocument::AnimeCharacter(material) = AnyMaterialDocument::from_ron(text).unwrap() else {
        panic!("an anime character material");
    };
    assert_eq!(material.layer_mask, None);
    assert_eq!((material.stockings, material.emission), (Stockings::default(), Emission::default()));
    assert_eq!(material.regions[0].matcap, RegionMatcap::default());
    let params = material.gpu_params(&[Swizzle::IDENTITY; 8]);
    assert_eq!(params.modes[0] & AnimeCharacterParams::FLAG_LAYERS, 0);
    assert_eq!(material.textures()[AnimeCharacterMaterial::SURFACE_MASK], None);
}

/// The share of an environment material's emission kept steady at every hour (M1 W1's keyed
/// emission) packs into the emission vector's alpha, which blocks cooked before it hold as 0, its
/// default; documents write it only when set, so those from before it read and write as they
/// did.
#[test]
fn environment_emission_keeps_its_steady_share_in_the_blocks_alpha() {
    let swizzles = [Swizzle::IDENTITY; EnvironmentMaterial::TEXTURE_SLOTS.len()];
    let lantern = EnvironmentMaterial { emission: [2.4, 1.4, 0.55], ..EnvironmentMaterial::default() };
    assert_eq!(lantern.gpu_params(&swizzles).emission, [2.4, 1.4, 0.55, 0.0], "all of it keyed");
    let sign = EnvironmentMaterial { emission_steady: 1.0, ..lantern.clone() };
    assert_eq!(sign.gpu_params(&swizzles).emission, [2.4, 1.4, 0.55, 1.0], "all of it steady");

    // A document without the field, as every one before it: read and written as it was.
    let written = to_ron(&lantern).unwrap();
    assert!(!written.contains("emission_steady"), "left out at its default: {written}");
    assert_eq!(from_ron::<EnvironmentMaterial>(&written).unwrap(), lantern);
    let text = to_ron(&sign).unwrap();
    assert!(text.contains("emission_steady: 1.0"), "{text}");
    assert_eq!(from_ron::<EnvironmentMaterial>(&text).unwrap(), sign);
}

#[test]
fn double_sided_materials_set_their_flag_unless_they_blend() {
    // Off by default, so every document from before it draws culled as it did (M1 C8).
    assert!(!AnimeCharacterMaterial::default().flags().contains(MaterialFlags::DOUBLE_SIDED));
    let sided =
        |alpha| AnimeCharacterMaterial { double_sided: true, alpha, ..AnimeCharacterMaterial::default() };
    let swizzles = vec![Swizzle::IDENTITY; AnimeCharacterMaterial::TEXTURE_SLOTS.len()];
    for alpha in [AlphaMode::Opaque, AlphaMode::Mask { cutoff: 0.5 }] {
        let document = sided(alpha);
        assert!(document.flags().contains(MaterialFlags::DOUBLE_SIDED), "{alpha:?}");
        let material = Material::from_document(&document, &swizzles).unwrap();
        assert!(material.material_flags().contains(MaterialFlags::DOUBLE_SIDED), "{alpha:?}");
        // The block is the same: the sides are the pipeline's render state.
        let one_sided = AnimeCharacterMaterial { double_sided: false, ..document.clone() };
        assert_eq!(material.params, Material::from_document(&one_sided, &swizzles).unwrap().params);
    }
    // Blended surfaces draw unsorted: both faces would blend in the mesh's order, so they cull.
    assert!(!sided(AlphaMode::Blend).flags().contains(MaterialFlags::DOUBLE_SIDED));
    // Written only when set, so documents from before it read and write as they did.
    let text = to_ron(&sided(AlphaMode::Opaque)).unwrap();
    assert!(text.contains("double_sided: true"), "{text}");
    assert!(from_ron::<AnimeCharacterMaterial>(&text).unwrap().double_sided);
    let plain = to_ron(&AnimeCharacterMaterial::default()).unwrap();
    assert!(!plain.contains("double_sided"), "left out at its default: {plain}");
    assert!(!from_ron::<AnimeCharacterMaterial>(&plain).unwrap().double_sided);
    // Cooked materials' checks know the flag.
    let bits = MaterialFlags::DOUBLE_SIDED.bits();
    assert_eq!(MaterialFlags::from_bits(bits), Some(MaterialFlags::DOUBLE_SIDED));
    assert_eq!(MaterialFlags::from_bits(bits << 1), None);
}
