use std::collections::BTreeMap;
use std::fs;

use sapphire_assets::AssetId;
use sapphire_assets::material::{
    AlphaMode, AnimeCharacterMaterial, BandDiffuse, CharacterSurface, DiffuseMode, Emission, MatcapBlend,
    MatcapChannel, MaterialDocument, MaterialFlags, Rim,
};
use sapphire_assets::mesh::{Mesh, PositionFormat, SlotMaterial};
use sapphire_assets::rig::{
    Axis, CharacterRig, ColliderShape, ConstraintKind, ExpressionOverride, ExpressionPreset, HumanBone,
    LookAtKind, LookAtRange, MaterialColor, MorphTargetWeight, SpringTip,
};
use serde_json::{Value, json};

use super::mtoon::{Binding, Channel, LayerMask, Mtoon, OutlineWidth, ShadingShift, TextureRef, TextureUse};
use super::samples::{self, SampleVrm, image, material, node};
use super::*;
use crate::color::srgb_to_linear;
use crate::cook::{Cook, CookError, CookOptions, CookStatus, cook_dir};
use crate::meta::{self, AssetMeta, ImportSettings};
use crate::test_util::{TempDir, read_aligned};

const EPS: f32 = 1e-5;

/// The mean of the sample's shading shift image, `(x + y) × 16` over 8 × 8 texels: 112 / 255.
const SHIFT_MEAN: f32 = 112.0 / 255.0;

/// The linear mean color of the sample's rim image, `(255, x × 20, 0)` in sRGB.
const RIM_MEAN: [f32; 3] = [1.0, 0.091_391_19, 0.0];

fn close(a: f32, b: f32) -> bool {
    (a - b).abs() < EPS
}

fn close3(a: [f32; 3], b: [f32; 3]) -> bool {
    a.iter().zip(b).all(|(a, b)| close(*a, b))
}

fn import1() -> VrmImport {
    import(&samples::vrm1().to_glb(), Some(VrmVersion::V1)).expect("the 1.0 sample imports")
}

fn import0() -> VrmImport {
    import(&samples::vrm0().to_glb(), Some(VrmVersion::V0)).expect("the 0.x sample imports")
}

/// The index of the skeleton joint named after node `name`.
fn joint(import: &VrmImport, name: &str) -> u16 {
    let skin = import.mesh.skin.as_ref().expect("skinned");
    skin.joints.iter().position(|joint| joint.name == name).unwrap_or_else(|| panic!("no joint {name}"))
        as u16
}

fn target(import: &VrmImport, name: &str) -> u32 {
    import.mesh.morph_targets.iter().position(|target| target.name == name).expect("a morph target") as u32
}

fn slot(import: &VrmImport, name: &str) -> u32 {
    import.mesh.materials.iter().position(|slot| slot.name == name).expect("a slot") as u32
}

/// The error importing `sample` after `edit` changes its JSON.
fn error_after(sample: SampleVrm, edit: impl FnOnce(&mut Value)) -> VrmImportError {
    let mut sample = sample;
    edit(&mut sample.document);
    import(&sample.to_glb(), None).expect_err("the edit breaks the file")
}

fn assert_reference(error: VrmImportError, what: &str, problem: &str) {
    let text = error.to_string();
    assert!(matches!(error, VrmImportError::Reference { .. }), "{error:?}");
    assert!(text.contains(what) && text.contains(problem), "{text}");
}

#[test]
fn vrm_1_0_humanoid_expressions_look_at_springs_and_constraints() {
    let import = import1();
    assert_eq!(import.version, VrmVersion::V1);
    let rig = &import.rig;
    let skin = import.mesh.skin.as_ref().unwrap();
    assert_eq!(rig.joint_count as usize, skin.joints.len());
    // `blink` merges across the face and the lashes.
    assert_eq!(rig.morph_target_count, 2);
    let slots: Vec<&str> = import.mesh.materials.iter().map(|slot| slot.name.as_str()).collect();
    assert_eq!(slots, ["Skin", "Hair", "Plain", "Lashes"]);
    assert_eq!(rig.material_slot_count, 4);

    // Every node the data names is a joint, the hair's end node no skin uses included.
    assert!(skin.joints.iter().any(|joint| joint.name == "Hair3_end"));
    assert_eq!(rig.humanoid.len(), 17);
    for (bone, name) in
        [(HumanBone::Hips, "Hips"), (HumanBone::Head, "Head"), (HumanBone::LeftHand, "LeftHand")]
    {
        assert_eq!(rig.human_joint(bone), Some(joint(&import, name)), "{bone:?}");
    }
    assert!(rig.humanoid.windows(2).all(|pair| pair[0].bone < pair[1].bone), "sorted by bone");

    let names: Vec<&str> = rig.expressions.iter().map(|e| e.name.as_str()).collect();
    assert_eq!(names, ["happy", "aa", "blink", "sparkle"], "presets in order, then custom");
    let happy = rig.expression("happy").unwrap();
    assert_eq!(happy.preset, Some(ExpressionPreset::Happy));
    assert_eq!(happy.morph_targets, [MorphTargetWeight { target: target(&import, "smile"), weight: 1.0 }]);
    let blink = rig.expression("blink").unwrap();
    assert_eq!(blink.morph_targets, [MorphTargetWeight { target: target(&import, "blink"), weight: 1.0 }]);
    assert_eq!(
        (blink.override_blink, blink.override_look_at, blink.override_mouth),
        (ExpressionOverride::None, ExpressionOverride::None, ExpressionOverride::Block)
    );
    let aa = rig.expression("aa").unwrap();
    assert!(aa.binary);
    assert_eq!(aa.morph_targets[0].weight, 0.5);
    let sparkle = rig.expression("sparkle").unwrap();
    assert_eq!(sparkle.preset, None);
    assert_eq!(sparkle.override_blink, ExpressionOverride::Blend);
    // The bind of a material no primitive uses is dropped.
    assert_eq!(sparkle.material_colors.len(), 1);
    assert_eq!(sparkle.material_colors[0].slot, slot(&import, "Skin"));
    assert_eq!(sparkle.material_colors[0].color, MaterialColor::Shade);
    assert_eq!(sparkle.material_colors[0].value, [1.0, 0.5, 0.5, 1.0]);
    assert_eq!(sparkle.texture_transforms[0].slot, slot(&import, "Hair"));
    assert_eq!(
        (sparkle.texture_transforms[0].scale, sparkle.texture_transforms[0].offset),
        ([1.0; 2], [0.5, 0.0])
    );

    let look_at = rig.look_at.unwrap();
    assert_eq!(look_at.offset_from_head, [0.0, 0.06, 0.03]);
    assert_eq!(look_at.kind, LookAtKind::Bone);
    assert_eq!(look_at.horizontal_outer, LookAtRange { input_max_degrees: 90.0, output_scale: 12.0 });
    assert_eq!(look_at.vertical_down.input_max_degrees, 60.0);

    let [spring] = &rig.springs[..] else { panic!("one spring") };
    assert_eq!(spring.name, "hair");
    let chain: Vec<u16> = spring.joints.iter().map(|j| j.joint).collect();
    let expected: Vec<u16> =
        ["Hair1", "Hair2", "Hair3", "Hair3_end"].map(|name| joint(&import, name)).to_vec();
    assert_eq!(chain, expected);
    assert_eq!(spring.tip, SpringTip::Joint);
    let first = spring.joints[0];
    assert_eq!(
        (first.hit_radius, first.stiffness, first.gravity_power, first.gravity_dir, first.drag_force),
        (0.02, 1.5, 0.2, [0.0, -1.0, 0.0], 0.4)
    );
    let second = spring.joints[1];
    assert_eq!(
        (second.hit_radius, second.stiffness, second.gravity_power, second.gravity_dir, second.drag_force),
        (0.0, 1.0, 0.0, [0.0, -1.0, 0.0], 0.5),
        "VRMC_springBone's defaults"
    );
    assert_eq!((spring.collider_groups.as_slice(), spring.center), (&[0][..], Some(joint(&import, "Hips"))));
    assert_eq!(rig.colliders.len(), 2);
    assert_eq!(rig.colliders[0].joint, joint(&import, "Head"));
    assert_eq!(rig.colliders[0].shape, ColliderShape::Sphere { offset: [0.0, 0.05, 0.0], radius: 0.1 });
    assert_eq!(
        rig.colliders[1].shape,
        ColliderShape::Capsule { offset: [0.0; 3], radius: 0.12, tail: [0.0, 0.15, 0.0] }
    );
    assert_eq!(rig.collider_groups[0].name, "upper body");
    assert_eq!(rig.collider_groups[0].colliders, [0, 1]);

    let [constraint] = &rig.constraints[..] else { panic!("one constraint") };
    assert_eq!(constraint.joint, joint(&import, "LeftLowerArmTwist"));
    assert_eq!(constraint.source, joint(&import, "LeftHand"));
    assert_eq!((constraint.weight, constraint.kind), (0.5, ConstraintKind::Roll { axis: Axis::X }));
}

#[test]
fn mtoon_1_0_parses_every_field() {
    let import = import1();
    let skin = import.slots[0].clone().expect("Skin is MToon");
    let reference = |texture: usize, channel| Some(TextureRef { texture, channel });
    assert_eq!(skin.base_color, [0.8, 0.6, 1.0, 1.0]);
    assert_eq!(skin.base_color_texture, reference(image::ALBEDO, Channel::Rgba));
    assert_eq!(skin.alpha, AlphaMode::Opaque);
    assert_eq!(skin.normal_texture, reference(image::NORMAL, Channel::Rgba));
    assert_eq!(skin.shade_color, [0.4, 0.3, 0.5]);
    assert_eq!(skin.shade_multiply_texture, reference(image::ALBEDO, Channel::Rgba));
    assert_eq!(skin.shading_shift, -0.1);
    let shift = skin.shading_shift_texture.expect("a shading shift texture");
    assert_eq!(
        (shift.texture, shift.scale),
        (TextureRef { texture: image::SHIFT, channel: Channel::R }, 0.5)
    );
    assert!(close(shift.mean, SHIFT_MEAN), "measured on import: {}", shift.mean);
    assert_eq!(skin.shading_toony, 0.8);
    assert_eq!(skin.matcap_factor, [0.5, 0.4, 0.5]);
    assert_eq!(skin.matcap_texture, reference(image::MATCAP, Channel::Rgba));
    assert_eq!(
        (skin.rim_color, skin.rim_fresnel_power, skin.rim_lift, skin.rim_lighting_mix),
        ([0.4, 0.2, 0.2], 2.0, 0.1, 0.5)
    );
    // The rim multiply texture is measured as color: (1, rising G, 0), brightest in R.
    let rim = skin.rim_multiply_texture.expect("a rim multiply texture");
    assert_eq!(
        (rim.texture, rim.channel, rim.size),
        (TextureRef { texture: image::RIM, channel: Channel::Rgba }, Channel::R, [image::SIZE; 2])
    );
    assert!(close3(rim.mean, RIM_MEAN), "measured on import: {:?}", rim.mean);
    assert_eq!(rim.hue(), rim.mean, "its R is 1 throughout");
    assert_eq!(skin.outline_width, OutlineWidth::World(0.002));
    assert_eq!(skin.outline_width_multiply_texture, reference(image::OUTLINE, Channel::G));
    assert_eq!(skin.outline_color, [0.1, 0.05, 0.05]);
    assert_eq!((skin.emissive, skin.emissive_texture), ([0.0; 3], None), "glTF's default: black");
    assert_eq!(skin.uv_animation_mask_texture, reference(image::RIM, Channel::B));
    assert!(skin.is_textured());
    // The rim multiply texture masks the matcap, so the layer mask holds it rather than a
    // texture of its own.
    assert_eq!(skin.layer_mask(), Some(LayerMask { matcap: Some(rim), emission: None }));
    let uses: Vec<TextureUse> = skin.textures().into_iter().map(|(usage, _)| usage).collect();
    assert_eq!(
        uses,
        [
            TextureUse::BaseColor,
            TextureUse::Shade,
            TextureUse::Normal,
            TextureUse::ShadingShift,
            TextureUse::Matcap,
            TextureUse::OutlineWidth,
            TextureUse::UvAnimationMask,
        ]
    );
    let unmasked = Mtoon { matcap_texture: None, ..skin.clone() };
    assert_eq!(unmasked.layer_mask(), None, "no matcap for it to mask");
    assert!(unmasked.textures().contains(&(TextureUse::RimMultiply, rim.texture)), "kept as a texture");

    let hair = import.slots[1].clone().expect("Hair is MToon");
    assert_eq!(
        (hair.alpha, hair.outline_width),
        (AlphaMode::Mask { cutoff: 0.3 }, OutlineWidth::Screen(0.004))
    );
    assert!(!hair.is_textured());
    assert_eq!(import.slots[2], None, "Plain is neither MToon nor unlit");
    let lashes = import.slots[3].clone().expect("Lashes are unlit");
    assert_eq!(
        (lashes.shade_color, lashes.shade_multiply_texture, lashes.shading_toony),
        ([1.0; 3], lashes.base_color_texture, 1.0)
    );
    assert_eq!((lashes.rim_color, lashes.outline_width), ([0.0; 3], OutlineWidth::None));

    // MToon 1.0's defaults for everything left out.
    let defaults = &import.document.materials().nth(material::UNUSED).unwrap();
    let schema = serde_json::from_value(json!({ "specVersion": "1.0" })).unwrap();
    let check = TextureCheck { material: "Unused", images: import.images() };
    let unused = Mtoon::from_v1(defaults, &schema, &check).unwrap();
    assert_eq!(
        (unused.shade_color, unused.shading_shift, unused.shading_toony, unused.matcap_factor),
        ([0.0; 3], 0.0, 0.9, [1.0; 3])
    );
    assert_eq!(
        (unused.rim_color, unused.rim_fresnel_power, unused.rim_lift, unused.rim_lighting_mix),
        ([0.0; 3], 5.0, 0.0, 1.0)
    );
    assert_eq!((unused.outline_width, unused.outline_color), (OutlineWidth::None, [0.0; 3]));
}

/// A distinct asset ID per binding, for checking which slot binds which.
fn bind_id(binding: Binding) -> AssetId {
    let byte = match binding {
        Binding::Texture(usage) => usage as u8 + 1,
        Binding::ControlMap => 100,
        Binding::LayerMask => 101,
    };
    AssetId::from_bytes([byte; 16])
}

/// [`bind_id`] for a texture use.
fn use_id(usage: TextureUse) -> AssetId {
    bind_id(Binding::Texture(usage))
}

#[test]
fn mtoon_maps_onto_anime_character_materials_field_by_field() {
    let import = import1();
    let skin = import.slots[0].clone().unwrap();
    let material = skin.to_material(|binding| Some(bind_id(binding)));
    assert_eq!(material.surface, CharacterSurface::Body);
    assert_eq!(material.base_color, [0.8, 0.6, 1.0, 1.0]);
    assert_eq!(material.base_color_map, Some(use_id(TextureUse::BaseColor)));
    assert_eq!(material.normal_map, Some(use_id(TextureUse::Normal)));
    assert_eq!(material.control_map, Some(bind_id(Binding::ControlMap)));
    assert_eq!(material.alpha, AlphaMode::Opaque);
    // linearstep(−1 + 0.8, 1 − 0.8, N·L − 0.1): centered at N·L = 0.1, half-width 0.2.
    assert!(close(material.light_split.threshold, 0.1) && close(material.light_split.softness, 0.2));
    assert_eq!(material.diffuse, DiffuseMode::Bands(BandDiffuse { deep_threshold: None }));
    let [region] = &material.regions[..] else { panic!("one region") };
    assert!(close3(region.shade_color, [0.5, 0.5, 0.5]), "shade over base: {:?}", region.shade_color);
    assert_eq!((region.outline_color, region.outline_width), ([0.1, 0.05, 0.05], 1.0));
    // The rim color (0.4, 0.2, 0.2) × the rim multiply texture's mean color, which the
    // depth-offset rim can't take per texel: strength 0.4 in R, the tint the rest.
    assert!(close3(region.rim_color, [1.0, 0.5 * RIM_MEAN[1], 0.0]), "{:?}", region.rim_color);
    assert!(close(material.rim.strength, 0.4));
    // The Fresnel gate at half intensity, where (1 − N·V + 0.1)² = ½; the depth-offset rim's own
    // width (M1 C5), and the lighting mix as its light gate.
    assert!(close(material.rim.fresnel, 1.1 - 0.5f32.sqrt()), "{}", material.rim.fresnel);
    assert_eq!(material.rim.width, Rim::default().width);
    assert!(close(material.rim.light_gate, 0.5), "{}", material.rim.light_gate);
    let matcap = material.matcap.unwrap();
    assert_eq!(matcap.map, use_id(TextureUse::Matcap));
    assert!(close(matcap.strength, 0.5) && close(matcap.shadow_strength, 0.25));
    // C7's matcap, added as MToon's is, tinted by the factor (0.5, 0.4, 0.5) over its strength
    // and by the rim multiply texture's hue: its R masks the matcap through the layer mask.
    assert_eq!((region.matcap.channel, region.matcap.blend), (MatcapChannel::Rgb, MatcapBlend::Add));
    assert!(close3(region.matcap.color, [1.0, 0.8 * RIM_MEAN[1], 0.0]), "{:?}", region.matcap.color);
    assert_eq!(material.layer_mask, Some(bind_id(Binding::LayerMask)));
    assert_eq!(region.emission, [0.0; 3], "no glow");
    assert_eq!(material.emission, Emission::default(), "nothing to make steady");
    // 2 mm seen from 2 m through 45°, at 1080 pixels.
    let expected = 0.002 / (4.0 * 22.5f32.to_radians().tan()) * 1080.0;
    assert!(close(material.outline.width, expected), "{}", material.outline.width);
    assert_eq!(material.outline.z_offset, 0.0);
    // A world width holds in meters at every distance (outlines v2, M1 C4): the curve's three
    // knots at the 2 m the pixel width is measured from.
    let curve = material.outline.curve;
    assert_eq!((curve.near, curve.mid, curve.far), ((2.0, 1.0), 2.0, (2.0, 1.0)));
    for distance in [0.5, 2.0, 10.0] {
        assert!(close(curve.scale(distance) * distance, 2.0), "{distance} m");
    }
    // MToon's outlineLightingMixFactor, 1 here: the line × its surface's lit color, the albedo
    // and the light both.
    assert_eq!((material.outline.lighting, material.outline.albedo), (1.0, 1.0));
    assert_eq!(material.specular, AnimeCharacterMaterial::default().specular, "MToon has no specular");

    // The import binds no textures, so the generated material draws with factors alone.
    let generated = &import.mesh.materials[0].generated;
    assert_eq!(*generated, skin.to_material(|_| None));
    // Without its control map, the shading shift texture's mean offset joins the threshold.
    assert!(close(generated.light_split.threshold, 0.1 - 0.5 * SHIFT_MEAN), "{:?}", generated.light_split);
    assert!(
        generated.base_color_map.is_none()
            && generated.control_map.is_none()
            && generated.matcap.is_none()
            && generated.layer_mask.is_none()
    );
    // Uses the material has no texture for are never asked for.
    let hair = import.slots[1].clone().unwrap().to_material(|binding| Some(bind_id(binding)));
    assert_eq!((hair.base_color_map, hair.control_map, hair.matcap), (None, None, None));
    assert!(close(hair.outline.width, 0.004 * 1080.0));
    // A screen width holds in pixels from the camera out to the cutoff.
    let cutoff = hair.outline.curve.cutoff;
    assert_eq!((hair.outline.curve.near, hair.outline.curve.far), ((0.0, 1.0), (cutoff, 1.0)));
    assert!([0.05, 3.0, 50.0].iter().all(|&distance| close(hair.outline.curve.scale(distance), 1.0)));
    let unlit_lines = Mtoon { outline_lighting_mix: 0.25, ..import.slots[1].clone().unwrap() };
    let outline = unlit_lines.to_material(|_| None).outline;
    assert_eq!((outline.lighting, outline.albedo), (0.25, 0.25));
    assert!(close3(hair.regions[0].shade_color, [0.25, 0.25, 1.0]));
    assert_eq!(hair.alpha, AlphaMode::Mask { cutoff: 0.3 });
    // Unlit: both sides the same color, no rim, no outline, a hard split.
    let lashes = import.slots[3].clone().unwrap().to_material(|_| None);
    assert_eq!(
        (lashes.regions[0].shade_color, lashes.rim.strength, lashes.outline.width),
        ([1.0; 3], 0.0, 0.0)
    );
    assert_eq!(lashes.light_split.softness, 0.0);
    assert_eq!(lashes.alpha, AlphaMode::Mask { cutoff: 0.4 });

    // Edge cases.
    let edge = Mtoon {
        base_color: [0.0, 0.5, 1.0, 1.0],
        shade_color: [0.3, 0.25, 0.0],
        shading_toony: 1.5,
        rim_color: [0.0; 3],
        rim_fresnel_power: 0.0,
        outline_width: OutlineWidth::World(-1.0),
        ..skin.clone()
    };
    let mapped = edge.to_material(|_| None);
    assert!(close3(mapped.regions[0].shade_color, [0.3, 0.5, 0.0]), "a black base channel keeps the shade");
    assert_eq!(mapped.light_split.softness, 0.0, "toony clamps to 1");
    assert_eq!((mapped.rim.strength, mapped.regions[0].rim_color), (0.0, [1.0; 3]));
    assert_eq!(mapped.rim.fresnel, 1.0, "a zero power lights the whole silhouette");
    assert_eq!(mapped.outline.width, 0.0);
    assert!(mapped.matcap.is_none(), "no matcap without its texture's asset");
    mapped.validate().expect("mapped materials are valid documents");
    material.validate().expect("mapped materials are valid documents");
}

#[test]
fn shading_shift_textures_become_ilm_shadow_bias() {
    let range = |scale| {
        ShadingShift { texture: TextureRef { texture: 0, channel: Channel::R }, scale, mean: 0.0 }
            .shadow_bias_range()
    };
    assert_eq!(range(0.5), [0.5, 0.75]);
    assert_eq!(range(1.0), [0.5, 1.0]);
    assert_eq!(range(-1.0), [0.5, 0.0]);
    assert_eq!(range(3.0), [0.5, 1.0], "offsets past ±1 clamp");
}

#[test]
fn vrm_0_x_migrates_to_the_same_character() {
    let (v1, v0) = (import1(), import0());
    assert_eq!(v0.version, VrmVersion::V0);

    // Turned to face +Z, the 0.x scene is the 1.0 one: vertices (with their baked outline
    // widths: 0.x reads the width texture's R where 1.0 reads G), joints, binds, morph targets.
    assert_eq!(v0.mesh.vertices.len(), v1.mesh.vertices.len());
    for (a, b) in v0.mesh.vertices.iter().zip(&v1.mesh.vertices) {
        assert!(close3(a.position, b.position) && close3(a.normal, b.normal), "{a:?} != {b:?}");
        assert_eq!((a.uv, a.color), (b.uv, b.color));
    }
    let (s0, s1) = (v0.mesh.skin.as_ref().unwrap(), v1.mesh.skin.as_ref().unwrap());
    assert_eq!(s0.joints.len(), s1.joints.len());
    for (a, b) in s0.joints.iter().zip(&s1.joints) {
        assert_eq!((&a.name, a.parent), (&b.name, b.parent));
        assert!(close3(a.rest.translation, b.rest.translation), "{}: {a:?} != {b:?}", a.name);
    }
    for (a, b) in s0.binds.iter().zip(&s1.binds) {
        assert_eq!(a.joint, b.joint);
        let (a, b) = (a.inverse_bind_matrix.as_flattened(), b.inverse_bind_matrix.as_flattened());
        assert!(a.iter().zip(b).all(|(a, b)| close(*a, *b)), "{a:?} != {b:?}");
    }
    assert_eq!(v0.mesh.morph_targets, v1.mesh.morph_targets);

    let rig = &v0.rig;
    // The thumbs gained a metacarpal in 1.0.
    assert_eq!(rig.human_joint(HumanBone::LeftThumbMetacarpal), Some(joint(&v0, "LeftThumbA")));
    assert_eq!(rig.human_joint(HumanBone::LeftThumbProximal), Some(joint(&v0, "LeftThumbB")));
    assert_eq!(rig.humanoid.len(), 19);

    let names: Vec<&str> = rig.expressions.iter().map(|e| e.name.as_str()).collect();
    assert_eq!(names, ["happy", "blinkLeft", "Sparkle"]);
    assert_eq!(
        rig.expressions[0].morph_targets,
        [MorphTargetWeight { target: target(&v0, "smile"), weight: 1.0 }]
    );
    assert_eq!(
        rig.expressions[1].morph_targets,
        [MorphTargetWeight { target: target(&v0, "blink"), weight: 0.5 }]
    );
    let sparkle = &rig.expressions[2];
    assert!(sparkle.binary);
    assert_eq!(sparkle.material_colors.len(), 1, "only the properties 1.0 has");
    let shade = sparkle.material_colors[0];
    assert_eq!((shade.slot, shade.color), (slot(&v0, "Skin"), MaterialColor::Shade));
    assert!(close(shade.value[1], srgb_to_linear(0.5)), "sRGB decoded: {:?}", shade.value);
    let uv = sparkle.texture_transforms[0];
    assert_eq!(uv.slot, slot(&v0, "Hair"));
    assert!(close(uv.scale[1], 0.5) && close(uv.offset[0], 0.25) && close(uv.offset[1], 0.4), "{uv:?}");

    let look_at = rig.look_at.unwrap();
    assert!(close3(look_at.offset_from_head, [-0.01, 0.06, 0.03]), "{:?}", look_at.offset_from_head);
    assert_eq!(look_at.horizontal_inner, LookAtRange { input_max_degrees: 90.0, output_scale: 8.0 });

    // One spring per branch, each swinging its leaf toward a 7 cm tail.
    let chains: Vec<Vec<u16>> =
        rig.springs.iter().map(|s| s.joints.iter().map(|j| j.joint).collect()).collect();
    let names = |names: &[&str]| names.iter().map(|name| joint(&v0, name)).collect::<Vec<_>>();
    assert_eq!(chains, [names(&["Hair1", "Hair2", "Hair3", "Hair3_end"]), names(&["HairSide"])]);
    for spring in &rig.springs {
        assert_eq!(spring.name, "hair");
        assert_eq!(spring.tip, SpringTip::Extended { length: v0::LEAF_TAIL });
        assert_eq!((spring.collider_groups.as_slice(), spring.center), (&[0][..], None));
        let joint = spring.joints[0];
        assert_eq!(
            (joint.stiffness, joint.gravity_power, joint.drag_force, joint.hit_radius),
            (1.5, 0.2, 0.4, 0.02)
        );
        // Unity's (0.1, −1, 0.2), Z negated then turned: (−0.1, −1, 0.2), normalized.
        let length = (0.01f32 + 1.0 + 0.04).sqrt();
        assert!(
            close3(joint.gravity_dir, [-0.1 / length, -1.0 / length, 0.2 / length]),
            "{:?}",
            joint.gravity_dir
        );
    }
    assert_eq!(rig.colliders[0].joint, joint(&v0, "Head"));
    let ColliderShape::Sphere { offset, radius } = rig.colliders[0].shape else { panic!("a sphere") };
    assert!(close3(offset, [-0.01, 0.05, 0.02]) && radius == 0.1, "{offset:?}");
    assert!(rig.constraints.is_empty(), "VRM 0.x has no constraints");
}

#[test]
fn mtoon_0_x_properties_migrate() {
    let v0 = import0();
    let skin = v0.slots[0].clone().expect("MToon wins over the glTF material's unlit");
    let decode = |[r, g, b]: [f32; 3]| [srgb_to_linear(r), srgb_to_linear(g), srgb_to_linear(b)];
    assert_eq!(skin.base_color, [srgb_to_linear(0.9), srgb_to_linear(0.8), 1.0, 1.0]);
    assert_eq!(skin.shade_color, decode([0.6, 0.5, 0.7]));
    // toony = lerp(0.8, 1, ½ − ½·0.1) = 0.89; shift = 0.1 − (1 − 0.89), less the grade rate 0.5.
    assert!(close(skin.shading_toony, 0.89), "{}", skin.shading_toony);
    assert!(close(skin.shading_shift, 0.1 - 0.11 - 0.5), "{}", skin.shading_shift);
    let grade = skin.shading_shift_texture.expect("the grade texture");
    assert_eq!(
        (grade.texture, grade.scale),
        (TextureRef { texture: image::SHIFT, channel: Channel::R }, 0.5)
    );
    assert!(close(grade.mean, SHIFT_MEAN));
    // The generated material gives the grade's `−rate` back by the texture's mean, so it isn't
    // pushed into shadow wholesale (a white grade texture gives it all back).
    let generated = &v0.mesh.materials[0].generated;
    assert!(close(generated.light_split.threshold, 0.51 - 0.5 * SHIFT_MEAN), "{:?}", generated.light_split);
    assert_eq!(skin.outline_width, OutlineWidth::World(0.002), "0.2 cm");
    assert_eq!(
        skin.outline_width_multiply_texture,
        Some(TextureRef { texture: image::OUTLINE_R, channel: Channel::R })
    );
    assert_eq!(skin.outline_color, decode([0.2, 0.1, 0.1]));
    assert_eq!(skin.rim_color, decode([0.5, 0.25, 0.25]));
    assert_eq!((skin.rim_fresnel_power, skin.rim_lift, skin.rim_lighting_mix), (2.0, 0.1, 0.5));
    assert_eq!((skin.matcap_factor, skin.matcap_texture.map(|t| t.texture)), ([1.0; 3], Some(image::MATCAP)));
    // `_RimTexture` is 1.0's rim multiply texture, measured as color.
    let rim = skin.rim_multiply_texture.expect("`_RimTexture`");
    assert_eq!((rim.texture.texture, rim.channel), (image::RIM, Channel::R));
    assert!(close3(rim.mean, RIM_MEAN), "{:?}", rim.mean);
    assert_eq!(skin.normal_texture.map(|t| t.texture), Some(image::NORMAL));
    assert_eq!(skin.shade_multiply_texture.map(|t| t.texture), Some(image::ALBEDO));
    // `_OutlineColorMode` 1, mixed lighting: `_OutlineLightingMix` is 1.0's lighting mix.
    assert_eq!(skin.outline_lighting_mix, 1.0);
    assert_eq!((skin.emissive, skin.emissive_texture), ([0.0; 3], None), "0.x's default: black");

    // 0.x's ramp from `_ShadeShift` to lerp(1, shift, toony) is 1.0's split: N·L from −0.1 to
    // 0.08 (before the grade's shift).
    let (low, high) = (-0.1f32, -0.1 + (1.0 + 0.1) * (1.0 - 0.8));
    let ungraded = Mtoon { shading_shift: skin.shading_shift + 0.5, ..skin.clone() };
    let center = -ungraded.shading_shift;
    let half_width = 1.0 - ungraded.shading_toony;
    assert!(close(center - half_width, low) && close(center + half_width, high), "{low}..{high}");

    let hair = v0.slots[1].clone().unwrap();
    assert_eq!(hair.outline_width, OutlineWidth::Screen(0.004), "0.8 % of half the screen");
    assert_eq!(hair.alpha, AlphaMode::Mask { cutoff: 0.3 });
    // MToon 0.x's shader defaults fill in what a material leaves out.
    assert_eq!(hair.shading_toony, 0.9 + 0.1 * 0.5);
    // `_OutlineColorMode` 0, a fixed color, by default: the line unlit, whatever
    // `_OutlineLightingMix` says (1 by default).
    assert_eq!(hair.outline_lighting_mix, 0.0);
    let outline = v0.mesh.materials[1].generated.outline;
    assert_eq!((outline.lighting, outline.albedo), (0.0, 0.0));
    let lashes = v0.slots[3].clone().expect("VRM/UnlitCutout is unlit");
    assert_eq!((lashes.alpha, lashes.shading_toony), (AlphaMode::Mask { cutoff: 0.4 }, 1.0));
    assert_eq!(v0.slots[2], None);
}

/// The linear mean of the sample's outline image, `(0, x × 255 / 7, 0)` in sRGB: one hue,
/// green.
const OUTLINE_MEAN: [f32; 3] = [0.0, 0.335_925_7, 0.0];

/// Adds `extension` to the sample's material `material` with `value`, listing it as used.
fn add_material_extension(sample: &mut SampleVrm, material: usize, extension: &str, value: Value) {
    sample.document["materials"][material]["extensions"][extension] = value;
    let used = sample.document["extensionsUsed"].as_array_mut().expect("extensions used");
    if !used.iter().any(|name| name == extension) {
        used.push(json!(extension));
    }
}

#[test]
fn mtoon_emission_and_matcap_textures_map_onto_the_layers() {
    // Skin glows (0.5, 0.25, 1) at strength 4 through the outline image (one hue, green); Hair
    // glows without a texture.
    let mut sample = samples::vrm1();
    let materials = &mut sample.document["materials"];
    materials[material::SKIN]["emissiveFactor"] = json!([0.5, 0.25, 1.0]);
    materials[material::SKIN]["emissiveTexture"] = json!({ "index": image::OUTLINE });
    materials[material::HAIR]["emissiveFactor"] = json!([0.0, 0.5, 0.25]);
    add_material_extension(
        &mut sample,
        material::SKIN,
        "KHR_materials_emissive_strength",
        json!({ "emissiveStrength": 4.0 }),
    );
    let glowing = import(&sample.to_glb(), Some(VrmVersion::V1)).expect("the glowing sample imports");
    let skin = glowing.slots[0].clone().unwrap();
    assert_eq!(skin.emissive, [2.0, 1.0, 4.0], "the factor × the strength");
    let glow = skin.emissive_texture.expect("an emissive texture");
    assert_eq!(
        (glow.texture.texture, glow.channel, glow.size),
        (image::OUTLINE, Channel::G, [image::SIZE; 2])
    );
    assert!(close3(glow.mean, OUTLINE_MEAN), "{:?}", glow.mean);
    assert_eq!(glow.hue(), [0.0, 1.0, 0.0]);
    let rim = skin.rim_multiply_texture.unwrap();
    assert_eq!(skin.layer_mask(), Some(LayerMask { matcap: Some(rim), emission: Some(glow) }));
    assert!(!skin.textures().iter().any(|&(usage, _)| usage == TextureUse::Emissive), "the mask holds it");

    // Extracted, the layer mask's B paints the glow, and the region's color takes the rest of
    // the texture's hue: (2, 1, 4) × (0, 1, 0). Steady, blooming as the world's light.
    let bound = skin.to_material(|binding| Some(bind_id(binding)));
    assert_eq!(bound.layer_mask, Some(bind_id(Binding::LayerMask)));
    assert_eq!(bound.regions[0].emission, [0.0, 1.0, 0.0]);
    assert_eq!(bound.emission, Emission { period: 0.0, ..Emission::default() });
    assert!(bound.flags().contains(MaterialFlags::LAYERS));
    bound.validate().expect("a valid material");
    // Generated, it binds no mask, so it glows at the texture's mean everywhere.
    let generated = &glowing.mesh.materials[0].generated;
    assert_eq!(*generated, skin.to_material(|_| None));
    assert_eq!(generated.layer_mask, None);
    assert!(close3(generated.regions[0].emission, [0.0, OUTLINE_MEAN[1], 0.0]), "{:?}", generated.regions[0]);
    assert_eq!(generated.emission.period, 0.0);
    assert!(generated.flags().contains(MaterialFlags::LAYERS), "the glow is a layer (M1 C7)");
    generated.validate().expect("a valid material");
    // Without a texture, the factor glows as it is, and there's no mask to bind.
    let hair = glowing.slots[1].clone().unwrap();
    assert_eq!((hair.emissive, hair.layer_mask()), ([0.0, 0.5, 0.25], None));
    let hair = hair.to_material(|binding| Some(bind_id(binding)));
    assert_eq!((hair.regions[0].emission, hair.layer_mask), ([0.0, 0.5, 0.25], None));
    assert_eq!(hair.emission.period, 0.0);
    // Unlit materials shade their base color alone: no glow.
    let mut unlit = samples::vrm1();
    unlit.document["materials"][material::LASHES]["emissiveFactor"] = json!([1.0, 1.0, 1.0]);
    let unlit = import(&unlit.to_glb(), None).unwrap();
    assert_eq!(unlit.slots[3].as_ref().unwrap().emissive, [0.0; 3]);
    assert_eq!(unlit.mesh.materials[3].generated.regions[0].emission, [0.0; 3]);

    // An emissive texture of another size than the rim multiply texture's: the mask holds the
    // glow alone, the rim multiply texture stays a texture of its own, and the matcap takes its
    // mean color, as everything does that no bound mask holds. Here the rim multiply texture is
    // the green outline image too, so its hue and its mean differ.
    let mut sample = samples::vrm1();
    let big = sample.add_png_texture("big_glow", &samples::png(16, |_, _| [255, 255, 255, 255]));
    sample.document["materials"][material::SKIN]["emissiveFactor"] = json!([1.0, 1.0, 1.0]);
    sample.document["materials"][material::SKIN]["emissiveTexture"] = json!({ "index": big });
    let mtoon = &mut sample.document["materials"][material::SKIN]["extensions"]["VRMC_materials_mtoon"];
    mtoon["rimMultiplyTexture"] = json!({ "index": image::OUTLINE });
    let mismatched = import(&sample.to_glb(), None).unwrap();
    let skin = mismatched.slots[0].clone().unwrap();
    let mask = skin.layer_mask().expect("a layer mask");
    assert_eq!((mask.matcap, mask.emission.map(|glow| glow.size)), (None, Some([16, 16])));
    let rim = skin.rim_multiply_texture.unwrap();
    assert!(skin.textures().contains(&(TextureUse::RimMultiply, rim.texture)));
    let bound = skin.to_material(|binding| Some(bind_id(binding)));
    assert!(
        close3(bound.regions[0].matcap.color, [0.0, 0.8 * OUTLINE_MEAN[1], 0.0]),
        "{:?}",
        bound.regions[0].matcap
    );
    assert_eq!(bound.regions[0].emission, [1.0; 3], "a white texture's hue");
    // An eye binds its own mask in the layer mask's slot: its glow and matcap take the means.
    let eye = Mtoon { eye: Some(EyeShading::default()), ..skin.clone() };
    assert_eq!(eye.layer_mask(), None);
    let eye = eye.to_material(|binding| Some(bind_id(binding)));
    assert_eq!((eye.layer_mask, eye.regions[0].emission), (None, [1.0; 3]));
    eye.validate().expect("a valid eye");

    // VRM's older HDR multiplier where a file has no emissive strength; the strength wins.
    let mut sample = samples::vrm1();
    sample.document["materials"][material::HAIR]["emissiveFactor"] = json!([0.5, 0.5, 0.5]);
    add_material_extension(
        &mut sample,
        material::HAIR,
        "VRMC_materials_hdr_emissiveMultiplier",
        json!({ "emissiveMultiplier": 3.0 }),
    );
    let emissive =
        |sample: &SampleVrm| import(&sample.to_glb(), None).unwrap().slots[1].as_ref().unwrap().emissive;
    assert_eq!(emissive(&sample), [1.5; 3]);
    add_material_extension(&mut sample, material::HAIR, "KHR_materials_emissive_strength", json!({}));
    assert_eq!(emissive(&sample), [0.5; 3], "the strength's default, 1");
    add_material_extension(
        &mut sample,
        material::HAIR,
        "KHR_materials_emissive_strength",
        json!({ "emissiveStrength": "bright" }),
    );
    let error = import(&sample.to_glb(), None).unwrap_err();
    assert!(
        matches!(error, VrmImportError::Extension { extension: "KHR_materials_emissive_strength", .. }),
        "{error:?}"
    );

    // VRM 0.x: `_EmissionColor` (sRGB) and `_EmissionMap`.
    let mut sample = samples::vrm0();
    let skin = &mut sample.document["extensions"]["VRM"]["materialProperties"][0];
    skin["vectorProperties"]["_EmissionColor"] = json!([1.0, 0.5, 0.25, 1.0]);
    skin["textureProperties"]["_EmissionMap"] = json!(image::OUTLINE);
    let v0 = import(&sample.to_glb(), Some(VrmVersion::V0)).unwrap();
    let skin = v0.slots[0].as_ref().unwrap();
    assert_eq!(skin.emissive, [1.0, srgb_to_linear(0.5), srgb_to_linear(0.25)]);
    assert_eq!(
        skin.emissive_texture.map(|glow| (glow.texture.texture, glow.channel)),
        Some((image::OUTLINE, Channel::G))
    );
    let bound = skin.to_material(|binding| Some(bind_id(binding)));
    assert_eq!(bound.regions[0].emission, [0.0, srgb_to_linear(0.5), 0.0]);
    assert_eq!(bound.layer_mask, Some(bind_id(Binding::LayerMask)));
}

#[test]
fn materials_without_glow_or_matcap_keep_the_layers_off() {
    let import = import1();
    for slot in [0, 1, 3] {
        let generated = &import.mesh.materials[slot].generated;
        let [region] = &generated.regions[..] else { panic!("one region") };
        assert_eq!(region.emission, [0.0; 3], "slot {slot}");
        assert_eq!(region.matcap, Default::default(), "slot {slot}");
        assert_eq!((generated.layer_mask, generated.emission), (None, Emission::default()), "slot {slot}");
        assert!(!generated.flags().contains(MaterialFlags::LAYERS), "slot {slot}: no layer");
    }
    // Hair has no textures at all: extracted, it binds nothing either.
    let hair = import.slots[1].clone().unwrap();
    assert_eq!((hair.layer_mask(), hair.is_textured()), (None, false));
    assert_eq!(hair.to_material(|binding| Some(bind_id(binding))), import.mesh.materials[1].generated);
}

#[test]
fn outline_width_textures_bake_into_the_vertex_outline_width() {
    let import = import1();
    let skin = slot(&import, "Skin");
    let mut skin_vertices = 0;
    for submesh in &import.mesh.lods[0].submeshes {
        let range = submesh.first_index as usize..(submesh.first_index + submesh.index_count) as usize;
        for &index in &import.mesh.indices[range] {
            let vertex = import.mesh.vertices[index as usize];
            // MToon ignores COLOR_0: AO and the Z-offset scale stay neutral.
            assert_eq!([vertex.color[0], vertex.color[2]], [255; 2], "MToon ignores COLOR_0");
            if submesh.material == skin {
                skin_vertices += 1;
                // The texture rises from 0 at u = 0 to 1 at u = 1 (clamped at the edges).
                let expected = if vertex.uv[0] == 0.0 { 0 } else { 255 };
                assert_eq!(vertex.color[1], expected, "{vertex:?}");
            } else {
                assert_eq!(vertex.color[1], 255, "no width texture: full width");
            }
        }
    }
    assert!(skin_vertices > 0);

    let mask = Mask { width: 4, height: 1, values: vec![0.0, 1.0, 2.0, 3.0] };
    let sample = |u: f32, wrap| mask.sample([u, 0.5], (wrap, WrappingMode::ClampToEdge));
    assert_eq!(sample(0.375, WrappingMode::Repeat), 1.0, "a texel center");
    assert_eq!(sample(0.5, WrappingMode::Repeat), 1.5, "between centers");
    assert_eq!(sample(0.0, WrappingMode::ClampToEdge), 0.0);
    assert_eq!(sample(0.0, WrappingMode::Repeat), 1.5, "halfway to the far edge");
    assert_eq!(sample(1.0 + 0.125, WrappingMode::Repeat), 0.0);
    assert_eq!(sample(-0.125, WrappingMode::MirroredRepeat), 0.0);
    assert_eq!(sample(1.125, WrappingMode::MirroredRepeat), 3.0);
    assert_eq!(sample(f32::NAN, WrappingMode::Repeat), 0.0, "no panic");
    for wrap in [WrappingMode::Repeat, WrappingMode::ClampToEdge, WrappingMode::MirroredRepeat] {
        for u in [1e30, -1e30, f32::INFINITY, f32::MAX] {
            assert!(sample(u, wrap).is_finite(), "{u} with {wrap:?}");
        }
    }
}

#[test]
fn the_version_note_is_checked() {
    let (v1, v0) = (samples::vrm1().to_glb(), samples::vrm0().to_glb());
    assert_eq!(detect_version(&v1), Some(VrmVersion::V1));
    assert_eq!(detect_version(&v0), Some(VrmVersion::V0));
    assert_eq!(detect_version(&crate::samples::mannequin_glb()), None);
    assert_eq!(detect_version(b"not glTF"), None);
    let error = import(&v1, Some(VrmVersion::V0)).unwrap_err();
    assert!(matches!(
        error,
        VrmImportError::VersionMismatch { sidecar: VrmVersion::V0, file: VrmVersion::V1 }
    ));
    assert!(error.to_string().contains("correct `version`"), "{error}");
    assert!(import(&v0, None).is_ok(), "no note, no check");

    // A file with both extensions is read as 1.0.
    let mut both = samples::vrm1();
    let zero = samples::vrm0().document["extensions"]["VRM"].clone();
    both.document["extensions"]["VRM"] = zero;
    assert_eq!(detect_version(&both.to_glb()), Some(VrmVersion::V1));
}

#[test]
fn files_that_are_not_vrm_or_name_missing_things_are_errors() {
    let not_vrm = import(&crate::samples::mannequin_glb(), None).unwrap_err();
    assert!(matches!(not_vrm, VrmImportError::NotVrm), "{not_vrm:?}");
    assert!(matches!(import(b"garbage", None).unwrap_err(), VrmImportError::Parse(_)));

    fn vrm(document: &mut Value) -> &mut Value {
        &mut document["extensions"]["VRMC_vrm"]
    }
    let error = error_after(samples::vrm1(), |d| vrm(d)["specVersion"] = "1.0-beta".into());
    assert!(
        matches!(&error, VrmImportError::SpecVersion { extension: "VRMC_vrm", found } if found == "1.0-beta")
    );
    let error = error_after(samples::vrm1(), |d| {
        vrm(d).as_object_mut().unwrap().remove("meta");
    });
    assert!(matches!(error, VrmImportError::Extension { extension: "VRMC_vrm", .. }), "{error:?}");
    assert_reference(
        error_after(samples::vrm1(), |d| vrm(d)["humanoid"]["humanBones"]["hips"]["node"] = 99.into()),
        "humanoid bone hips",
        "names node 99",
    );
    assert_reference(
        error_after(samples::vrm1(), |d| {
            vrm(d)["expressions"]["preset"]["happy"]["morphTargetBinds"][0]["index"] = 7.into()
        }),
        "expression \"happy\"",
        "morph target 7",
    );
    assert_reference(
        error_after(samples::vrm1(), |d| {
            vrm(d)["expressions"]["preset"]["happy"]["morphTargetBinds"][0]["node"] = node::HIPS.into();
        }),
        "expression \"happy\"",
        "has no mesh",
    );
    assert_reference(
        error_after(samples::vrm1(), |d| {
            vrm(d)["expressions"]["custom"]["sparkle"]["materialColorBinds"][0]["material"] = 99.into();
        }),
        "expression \"sparkle\"",
        "names material 99",
    );
    assert_reference(
        error_after(samples::vrm1(), |d| vrm(d)["expressions"]["custom"]["blink"] = json!({})),
        "custom expression \"blink\"",
        "preset's name",
    );

    fn mtoon(document: &mut Value) -> &mut Value {
        &mut document["materials"][material::SKIN]["extensions"]["VRMC_materials_mtoon"]
    }
    assert_reference(
        error_after(samples::vrm1(), |d| mtoon(d)["matcapTexture"]["index"] = 99.into()),
        "material \"Skin\" matcapTexture",
        "names texture 99",
    );
    let error = error_after(samples::vrm1(), |d| mtoon(d)["specVersion"] = "0.9".into());
    assert!(
        matches!(error, VrmImportError::SpecVersion { extension: "VRMC_materials_mtoon", .. }),
        "{error:?}"
    );

    fn springs(document: &mut Value) -> &mut Value {
        &mut document["extensions"]["VRMC_springBone"]
    }
    assert_reference(
        error_after(samples::vrm1(), |d| springs(d)["springs"][0]["joints"][1]["node"] = node::HEAD.into()),
        "spring 0 (\"hair\") joint 1",
        "doesn't descend",
    );
    assert_reference(
        error_after(samples::vrm1(), |d| springs(d)["springs"][0]["colliderGroups"] = json!([5])),
        "spring 0 (\"hair\") collider group",
        "names group 5",
    );
    assert_reference(
        error_after(samples::vrm1(), |d| springs(d)["colliderGroups"][0]["colliders"] = json!([2])),
        "spring collider group 0",
        "names collider 2",
    );
    assert_reference(
        error_after(samples::vrm1(), |d| springs(d)["colliders"][0]["shape"] = json!({})),
        "spring collider 0",
        "either a sphere or a capsule",
    );
    assert_reference(
        error_after(samples::vrm1(), |d| springs(d)["springs"][0]["joints"] = json!([])),
        "spring 0 (\"hair\")",
        "has no joints",
    );

    fn constraint(document: &mut Value) -> &mut Value {
        &mut document["nodes"][node::TWIST]["extensions"]["VRMC_node_constraint"]["constraint"]
    }
    assert_reference(
        error_after(samples::vrm1(), |d| {
            constraint(d)["aim"] = json!({ "source": 3, "aimAxis": "PositiveX" })
        }),
        "node 18's constraint",
        "exactly one of",
    );
    assert_reference(
        error_after(samples::vrm1(), |d| constraint(d)["roll"]["source"] = node::TWIST.into()),
        "node 18's constraint",
        "its own node",
    );
    assert_reference(
        error_after(samples::vrm1(), |d| constraint(d)["roll"]["rollAxis"] = "W".into()),
        "node 18's constraint",
        "roll axis",
    );
    // Nodes outside the scene are neither joints nor meshes that morph targets come from.
    let error = error_after(samples::vrm1(), |d| {
        d["nodes"][node::HAIR3]["children"] = json!([]);
    });
    assert_reference(
        error,
        "spring 0 (\"hair\") joint 3",
        "names node 22, which isn't in the imported scene",
    );
    let error = error_after(samples::vrm1(), |d| {
        d["nodes"][node::ROOT]["children"].as_array_mut().unwrap().retain(|child| child != node::FACE);
    });
    assert_reference(error, "expression \"happy\"", "isn't in the imported scene");

    // Images the outline bake needs must be PNG or JPEG.
    let error =
        error_after(samples::vrm1(), |d| d["images"][image::OUTLINE]["mimeType"] = "image/webp".into());
    assert!(
        matches!(&error, VrmImportError::Image { index: 4, problem } if problem.contains("image/webp")),
        "{error:?}"
    );
}

#[test]
fn vrm_0_x_errors() {
    fn vrm(document: &mut Value) -> &mut Value {
        &mut document["extensions"]["VRM"]
    }
    assert_reference(
        error_after(samples::vrm0(), |d| {
            vrm(d)["humanoid"]["humanBones"]
                .as_array_mut()
                .unwrap()
                .push(json!({ "bone": "hips", "node": 2 }));
        }),
        "humanoid bone 19 (hips)",
        "appears twice",
    );
    assert_reference(
        error_after(samples::vrm0(), |d| {
            vrm(d)["blendShapeMaster"]["blendShapeGroups"][2]["materialValues"][0]["materialName"] =
                "Gone".into();
        }),
        "blend shape group 2",
        "\"Gone\", which doesn't exist",
    );
    assert_reference(
        error_after(samples::vrm0(), |d| {
            vrm(d)["blendShapeMaster"]["blendShapeGroups"][2]["name"] = "".into();
        }),
        "blend shape group 2",
        "without a name",
    );
    assert_reference(
        error_after(samples::vrm0(), |d| {
            vrm(d)["blendShapeMaster"]["blendShapeGroups"][1]["binds"][0]["mesh"] = 9.into();
        }),
        "blend shape group 1",
        "names mesh 9",
    );
    assert_reference(
        error_after(samples::vrm0(), |d| {
            vrm(d)["materialProperties"][0]["textureProperties"]["_BumpMap"] = 42.into();
        }),
        "material \"Skin\" _BumpMap",
        "names texture 42",
    );
}

#[test]
fn rigs_cook_read_back_and_pair_with_their_mesh() {
    let bytes = samples::vrm1().to_glb();
    let import = import(&bytes, None).unwrap();
    assert_eq!(import_rig(&bytes, None).unwrap(), import.rig, "the rig alone is the same rig");
    let cooked = import.rig.to_cooked().unwrap();
    assert_eq!(CharacterRig::from_cooked(&cooked).unwrap(), import.rig);
    let rig = CharacterRig::read(&cooked).unwrap();
    let mesh = import.mesh.pack(PositionFormat::Float32, &BTreeMap::new()).unwrap().to_cooked().unwrap();
    rig.check_mesh(Mesh::read(&mesh).unwrap()).unwrap();
    // A mesh with another material slot isn't this rig's.
    let mut other = import.mesh.clone();
    other.materials.push(crate::test_util::material("Extra"));
    let other = other.pack(PositionFormat::Float32, &BTreeMap::new()).unwrap().to_cooked().unwrap();
    assert!(rig.check_mesh(Mesh::read(&other).unwrap()).is_err());

    // A file whose data the format refuses (two bones on one node) fails the rig's read-back.
    let mut sample = samples::vrm1();
    sample.document["extensions"]["VRMC_vrm"]["humanoid"]["humanBones"]["chest"]["node"] = node::SPINE.into();
    let rig = import_rig(&sample.to_glb(), None).unwrap();
    let error = CharacterRig::read(&rig.to_cooked().unwrap()).unwrap_err();
    assert!(error.to_string().contains("shares its joint"), "{error}");
}

#[test]
fn imports_are_deterministic() {
    for sample in [samples::vrm1(), samples::vrm0()] {
        let bytes = sample.to_glb();
        let (a, b) = (import(&bytes, None).unwrap(), import(&bytes, None).unwrap());
        assert!(a.rig.to_cooked().unwrap().as_slice() == b.rig.to_cooked().unwrap().as_slice());
        assert_eq!(a.mesh, b.mesh);
        assert_eq!(a.slots, b.slots);
    }
}

/// A content tree in a scratch directory with `sample` at `name`.
fn tree(scratch: &TempDir, name: &str, sample: &SampleVrm) -> std::path::PathBuf {
    let path = scratch.path().join("content").join(name);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(&path, sample.to_glb()).unwrap();
    path
}

fn cook(scratch: &TempDir, cook: Cook, cache: bool) -> Result<crate::cook::CookReport, CookError> {
    let cache_dir = scratch.path().join("cache");
    let options = CookOptions { cook, cache: cache.then_some(cache_dir.as_path()), locked: false };
    cook_dir(&scratch.path().join("content"), &scratch.path().join("out").join(cook.name()), &options)
}

#[test]
fn vrm_sources_cook_to_a_mesh_and_a_rig() {
    let scratch = TempDir::new("vrm_cook");
    let source = tree(&scratch, "characters/sample.vrm", &samples::vrm1());
    let old = tree(&scratch, "characters/old.vrm", &samples::vrm0());
    let report = cook(&scratch, Cook::Desktop, true).unwrap();

    let sidecar = meta::read(&meta::sidecar_path(&source)).unwrap();
    let ImportSettings::Vrm(settings) = &sidecar.importer else { panic!("{sidecar:?}") };
    assert_eq!(settings.version, Some(VrmVersion::V1), "the note is taken from the file");
    let ImportSettings::Vrm(old_settings) = meta::read(&meta::sidecar_path(&old)).unwrap().importer else {
        panic!("a VRM sidecar")
    };
    assert_eq!(old_settings.version, Some(VrmVersion::V0));
    let text = fs::read_to_string(meta::sidecar_path(&old)).unwrap();
    assert!(text.contains("importer: Vrm((") && text.contains("version: Some(V0)"), "{text}");

    let assets: Vec<_> = report.assets.iter().filter(|asset| asset.source == source).collect();
    let [mesh, rig] = &assets[..] else { panic!("a mesh and a rig: {assets:?}") };
    assert_eq!(mesh.id, sidecar.id);
    assert_eq!(rig.id, meta::sub_asset_id(sidecar.id, "rig"));
    let out = scratch.path().join("out/desktop/characters");
    assert_eq!(
        (mesh.output.clone(), rig.output.clone()),
        (out.join("sample.smesh"), out.join("sample.srig"))
    );
    let mesh_bytes = read_aligned(&mesh.output);
    let cooked_mesh = Mesh::read(&mesh_bytes).unwrap();
    assert!(
        cooked_mesh
            .materials
            .iter()
            .all(|slot| matches!(slot.material, sapphire_assets::mesh::ArchivedSlotMaterial::Embedded(_)))
    );
    let rig_bytes = read_aligned(&rig.output);
    CharacterRig::read(&rig_bytes).unwrap().check_mesh(cooked_mesh).unwrap();
    let mesh = Mesh::from_cooked(&mesh_bytes).unwrap();
    assert!(mesh.materials.iter().all(|slot| matches!(slot.material, SlotMaterial::Embedded(_))));

    // Everything is up to date on a second cook, and the mobile cook pairs too.
    let again = cook(&scratch, Cook::Desktop, true).unwrap();
    assert!(again.assets.iter().all(|asset| asset.status == CookStatus::UpToDate));
    let mobile = cook(&scratch, Cook::Mobile, true).unwrap();
    let [mobile_mesh, mobile_rig] =
        &mobile.assets.iter().filter(|a| a.source == source).collect::<Vec<_>>()[..]
    else {
        panic!("two assets")
    };
    let mobile_mesh = read_aligned(&mobile_mesh.output);
    CharacterRig::read(&read_aligned(&mobile_rig.output))
        .unwrap()
        .check_mesh(Mesh::read(&mobile_mesh).unwrap())
        .unwrap();
}

#[test]
fn vrm_cache_keys_follow_the_file_and_its_settings() {
    let scratch = TempDir::new("vrm_keys");
    let source = tree(&scratch, "sample.vrm", &samples::vrm1());
    let first = cook(&scratch, Cook::Desktop, true).unwrap();
    let keys: Vec<_> = first.assets.iter().map(|asset| asset.key).collect();
    assert_ne!(keys[0], keys[1], "the mesh and the rig have keys of their own");

    // A spring parameter changes the file: both recook (each reads the file).
    let mut stiffer = samples::vrm1();
    stiffer.document["extensions"]["VRMC_springBone"]["springs"][0]["joints"][0]["stiffness"] = 2.0.into();
    fs::write(&source, stiffer.to_glb()).unwrap();
    let report = cook(&scratch, Cook::Desktop, true).unwrap();
    assert!(report.assets.iter().all(|asset| asset.status == CookStatus::Cooked));
    assert!(report.assets.iter().zip(&keys).all(|(asset, key)| asset.key != *key));
    let rig = CharacterRig::from_cooked(&read_aligned(&report.assets[1].output)).unwrap();
    assert_eq!(rig.springs[0].joints[0].stiffness, 2.0);

    // Back to the first file: both keys hit the cache. The mesh's bytes never changed (the
    // stiffness is the rig's), so only the rig is written.
    fs::write(&source, samples::vrm1().to_glb()).unwrap();
    let report = cook(&scratch, Cook::Desktop, true).unwrap();
    let statuses: Vec<CookStatus> = report.assets.iter().map(|asset| asset.status).collect();
    assert_eq!(statuses, [CookStatus::UpToDate, CookStatus::Cached]);

    // A wrong note fails both, cached or not.
    let sidecar = meta::sidecar_path(&source);
    let mut meta: AssetMeta = meta::read(&sidecar).unwrap();
    let ImportSettings::Vrm(settings) = &mut meta.importer else { panic!("a VRM sidecar") };
    settings.version = Some(VrmVersion::V0);
    fs::write(&sidecar, sapphire_serialize::to_ron(&meta).unwrap()).unwrap();
    let error = cook(&scratch, Cook::Desktop, false).unwrap_err();
    assert!(
        matches!(&error, CookError::Vrm { source, .. } if matches!(**source, VrmImportError::VersionMismatch { .. })),
        "{error:?}"
    );
}

#[test]
fn vrm_cooks_are_deterministic() {
    let scratch = TempDir::new("vrm_determinism");
    tree(&scratch, "sample.vrm", &samples::vrm1());
    tree(&scratch, "old.vrm", &samples::vrm0());
    let outputs = |report: &crate::cook::CookReport| -> Vec<Vec<u8>> {
        report.assets.iter().map(|asset| fs::read(&asset.output).unwrap()).collect()
    };
    let first = outputs(&cook(&scratch, Cook::Desktop, false).unwrap());
    fs::remove_dir_all(scratch.path().join("out")).unwrap();
    let second = outputs(&cook(&scratch, Cook::Desktop, false).unwrap());
    assert_eq!(first.len(), 4);
    assert!(first == second, "two cooks of one source write the same bytes");
}

#[test]
fn vroid_eye_materials_map_onto_eye_layers() {
    use sapphire_assets::material::{EyeLayer, HighlightMotion};
    const IRIS: &str = "N00_000_00_EyeIris_00_EYE (Instance)";
    const EYE_LINE: &str = "N00_000_00_FaceEyeline_00_FACE (Instance)";
    assert_eq!(vroid_eye_layer(IRIS), Some(EyeLayer::Iris));
    assert_eq!(vroid_eye_layer("N00_000_00_EyeWhite_00_EYE (Instance)"), Some(EyeLayer::Eye));
    assert_eq!(
        vroid_eye_layer("N00_000_00_EyeHighlight_00_EYE (Instance)"),
        Some(EyeLayer::Highlight(HighlightMotion::default()))
    );
    for eye in [EYE_LINE, "N00_000_00_FaceEyelash_00_FACE", "N00_000_00_FaceBrow_00_FACE (Instance)"] {
        assert_eq!(vroid_eye_layer(eye), Some(EyeLayer::Eye), "{eye}");
    }
    for other in [
        "N00_000_00_Face_00_SKIN (Instance)",
        "N00_000_00_FaceMouth_00_FACE (Instance)",
        "N00_000_00_HairBack_00_HAIR (Instance)",
        "Skin",
        "EyeIrisDark",
    ] {
        assert_eq!(vroid_eye_layer(other), None, "{other}");
    }

    // The 1.0 sample with its hair and lashes named as VRoid names an iris and an eye line.
    let mut sample = samples::vrm1();
    let materials = sample.document["materials"].as_array_mut().expect("materials");
    materials[material::HAIR]["name"] = json!(IRIS);
    materials[material::LASHES]["name"] = json!(EYE_LINE);
    let import = import(&sample.to_glb(), Some(VrmVersion::V1)).expect("the renamed sample imports");
    let generated = |name: &str| &import.mesh.materials[slot(&import, name) as usize].generated;
    let iris = generated(IRIS);
    let CharacterSurface::Eye(eye) = iris.surface else { panic!("an eye: {:?}", iris.surface) };
    assert_eq!(eye.layer, EyeLayer::Iris);
    let shape = eye.iris.expect("the ellipse its texture coordinates span");
    assert!(shape.radius.iter().all(|&r| r > 0.0) && shape.center.iter().all(|c| (0.0..=1.0).contains(c)));
    assert!(eye.parallax > 0.0 && eye.through_hair.is_some());
    assert_eq!(iris.alpha, AlphaMode::Blend, "a layer over the eye blends, though the hair was alpha-tested");
    assert_eq!(iris.outline.width, 0.0, "no outline");
    iris.validate().expect("a valid eye layer");
    let line = generated(EYE_LINE);
    let CharacterSurface::Eye(eye) = line.surface else { panic!("an eye: {:?}", line.surface) };
    assert_eq!((eye.layer, eye.iris, eye.parallax), (EyeLayer::Eye, None, 0.0));
    assert_eq!(line.alpha, AlphaMode::Mask { cutoff: 0.4 }, "the eye itself keeps its alpha test");
    line.validate().expect("a valid eye");
    assert_eq!(generated("Skin").surface, CharacterSurface::Body, "everything else stays a body");
    // Extraction writes the same mapping out (`Mtoon::to_material` with textures bound).
    let mtoon = import.slots[slot(&import, IRIS) as usize].as_ref().expect("MToon");
    assert_eq!(mtoon.to_material(|_| None), *iris);
}

#[test]
fn vroid_names_give_the_stylized_gis_region_weights() {
    use sapphire_assets::material::RegionGi;
    const FACE: RegionGi = RegionGi { sphere: 1.0, skin: 1.0 };
    const HAIR: RegionGi = RegionGi { sphere: 1.0, skin: 0.0 };
    const SKIN: RegionGi = RegionGi { sphere: 0.0, skin: 1.0 };
    const NONE: RegionGi = RegionGi { sphere: 0.0, skin: 0.0 };
    // X1's look-dev model names its 13 materials as VRoid Studio does without the model's
    // prefix and Unity's suffix; VRoid Studio's own exports carry both.
    let names = [
        ("Face_00_SKIN", FACE),
        ("N00_000_00_Face_00_SKIN (Instance)", FACE),
        ("Body_00_SKIN", SKIN),
        ("N00_000_00_Body_00_SKIN (Instance)", SKIN),
        ("Hair_00_HAIR", HAIR),
        ("HairBack_00_HAIR", HAIR),
        ("N00_000_Hair_00_HAIR_01 (Instance)", HAIR),
        ("N00_000_00_HairBack_00_HAIR (Instance)", HAIR),
        // Clothes, eyes, and what VRoid draws on or inside the face (its `FACE` group).
        ("Tops_01_CLOTH", NONE),
        ("Bottoms_01_CLOTH", NONE),
        ("Shoes_01_CLOTH", NONE),
        ("N00_010_01_Onepiece_00_CLOTH (Instance)", NONE),
        ("EyeWhite_00_EYE", NONE),
        ("EyeIris_00_EYE", NONE),
        ("EyeHighlight_00_EYE", NONE),
        ("FaceBrow_00_FACE", NONE),
        ("FaceEyeline_00_FACE", NONE),
        ("N00_000_00_FaceEyelash_00_FACE (Instance)", NONE),
        ("FaceMouth_00_FACE", NONE),
        // Names VRoid Studio didn't write place nothing.
        ("Skin", NONE),
        ("Hair", NONE),
        ("Face", NONE),
        ("Body_00_skin", NONE),
        ("FaceSKIN", NONE),
        ("HAIRCLIP_00_CLOTH", NONE),
        ("", NONE),
    ];
    for (name, gi) in names {
        assert_eq!(vroid_region_gi(name), gi, "{name:?}");
    }

    // The 1.0 sample's MToon and unlit materials renamed as VRoid names a body, hair and a face;
    // Plain isn't MToon, so its glTF material stays as the glTF importer generates it.
    const BODY_NAME: &str = "N00_000_00_Body_00_SKIN (Instance)";
    const HAIR_NAME: &str = "N00_000_Hair_00_HAIR_01 (Instance)";
    const FACE_NAME: &str = "N00_000_00_Face_00_SKIN (Instance)";
    const PLAIN_NAME: &str = "N00_000_00_HairBack_00_HAIR (Instance)";
    let mut sample = samples::vrm1();
    let materials = sample.document["materials"].as_array_mut().expect("materials");
    for (material, name) in [
        (material::SKIN, BODY_NAME),
        (material::HAIR, HAIR_NAME),
        (material::LASHES, FACE_NAME),
        (material::PLAIN, PLAIN_NAME),
    ] {
        materials[material]["name"] = json!(name);
    }
    let renamed = import(&sample.to_glb(), Some(VrmVersion::V1)).expect("the renamed sample imports");
    let gi_of = |import: &VrmImport, name: &str| {
        let slot = slot(import, name) as usize;
        let [region] = &import.mesh.materials[slot].generated.regions[..] else { panic!("one region") };
        (region.gi, import.slots[slot].as_ref().map(|mtoon| mtoon.gi))
    };
    assert_eq!(gi_of(&renamed, BODY_NAME), (SKIN, Some(SKIN)));
    assert_eq!(gi_of(&renamed, HAIR_NAME), (HAIR, Some(HAIR)));
    assert_eq!(gi_of(&renamed, FACE_NAME), (FACE, Some(FACE)), "unlit materials take it too");
    assert_eq!(gi_of(&renamed, PLAIN_NAME), (NONE, None), "only MToon and unlit materials map");
    // Nothing else about a material changes with its name (but for eyes, M1 C6).
    let original = import1();
    for (name, renamed_name) in [("Skin", BODY_NAME), ("Hair", HAIR_NAME), ("Lashes", FACE_NAME)] {
        let generated = |import: &VrmImport, name: &str| {
            import.mesh.materials[slot(import, name) as usize].generated.clone()
        };
        let mut material = generated(&renamed, renamed_name);
        assert_eq!(generated(&original, name).regions[0].gi, NONE, "{name} isn't a VRoid name");
        material.regions[0].gi = NONE;
        assert_eq!(material, generated(&original, name), "{renamed_name}");
        // Extraction writes the same weights out (`Mtoon::to_material` with textures bound).
        let mtoon = renamed.slots[slot(&renamed, renamed_name) as usize].as_ref().expect("MToon");
        let bound = mtoon.to_material(|binding| Some(bind_id(binding)));
        assert_eq!(bound.regions[0].gi, generated(&renamed, renamed_name).regions[0].gi);
        bound.validate().expect("a valid material");
    }

    // VRM 0.x files map the same names the same way (renamed in the expression that sets them
    // too).
    let mut sample = samples::vrm0();
    let renames = [("Skin", FACE_NAME), ("Hair", HAIR_NAME)];
    let materials = sample.document["materials"].as_array_mut().expect("materials");
    materials[material::SKIN]["name"] = json!(FACE_NAME);
    materials[material::HAIR]["name"] = json!(HAIR_NAME);
    let groups = &mut sample.document["extensions"]["VRM"]["blendShapeMaster"]["blendShapeGroups"];
    for group in groups.as_array_mut().expect("expressions") {
        for value in group["materialValues"].as_array_mut().into_iter().flatten() {
            let name = value["materialName"].as_str().expect("a material name");
            if let Some(&(_, renamed)) = renames.iter().find(|&&(old, _)| old == name) {
                value["materialName"] = json!(renamed);
            }
        }
    }
    let migrated = import(&sample.to_glb(), Some(VrmVersion::V0)).expect("the renamed 0.x sample imports");
    assert_eq!(gi_of(&migrated, FACE_NAME), (FACE, Some(FACE)));
    assert_eq!(gi_of(&migrated, HAIR_NAME), (HAIR, Some(HAIR)));
    assert_eq!(gi_of(&migrated, "Lashes"), (NONE, Some(NONE)));
}
