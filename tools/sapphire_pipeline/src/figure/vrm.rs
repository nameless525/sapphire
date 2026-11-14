//! A figure with a `character` as a VRM 1.0 file (M5 B3), so its model cooks a rig beside its mesh
//! (`import::vrm`): the glTF scene `Built::scene` writes, with `VRMC_vrm` (the humanoid, the
//! expressions bound to the face's morph targets, and a look-at by expression) and
//! `VRMC_springBone` (each chain copy a spring, with every collider in one group).
//!
//! Its materials carry no MToon: the figure's sidecar binds every slot to an anime character
//! material document, as the look-dev character's does.

use glam::Vec3;
use sapphire_assets::rig::{ExpressionOverride, HumanBone};
use serde_json::{Map, Value, json};

use super::build::{Built, chain_copies};
use super::{CharacterSpec, Figure, FigureError};
use crate::gltf_writer::GltfWriter;

/// How far, in degrees, the look-at expressions reach at full weight: their range maps' input.
const LOOK_RANGE_DEGREES: f32 = 30.0;

/// The model as a VRM 1.0 file.
pub(crate) fn to_vrm(
    built: &Built,
    figure: &Figure,
    character: &CharacterSpec,
) -> Result<Vec<u8>, FigureError> {
    let rig = &built.rig;
    let joint = |name: &str, what: &dyn Fn() -> String| {
        rig.find(name).ok_or_else(|| FigureError::Spec(format!("{}: no joint {name:?}", what())))
    };
    let mut humanoid = Map::new();
    for (bone, name) in &character.humanoid {
        if HumanBone::from_name(bone).is_none() {
            return Err(FigureError::Spec(format!("{bone:?} isn't one of VRM 1.0's humanoid bones")));
        }
        humanoid.insert(bone.clone(), json!({ "node": joint(name, &|| format!("humanoid bone {bone:?}"))? }));
    }
    for bone in HumanBone::REQUIRED {
        if !humanoid.contains_key(bone.name()) {
            return Err(FigureError::Spec(format!("the humanoid needs {:?}", bone.name())));
        }
    }

    let mut writer = GltfWriter::default();
    let face = built.scene(&mut writer);

    let mut presets = Map::new();
    for (index, expression) in figure.expressions.iter().enumerate() {
        let face = face.expect("a figure with expressions has a face node");
        let mut value = json!({ "morphTargetBinds": [{ "node": face, "index": index, "weight": 1.0 }] });
        if expression.binary {
            value["isBinary"] = true.into();
        }
        for (key, override_) in [
            ("overrideBlink", &expression.override_blink),
            ("overrideLookAt", &expression.override_look_at),
            ("overrideMouth", &expression.override_mouth),
        ] {
            if let Some(name) = override_ {
                if ExpressionOverride::from_name(name).is_none() {
                    return Err(FigureError::Spec(format!(
                        "expression {:?}: {key} is none, block or blend, not {name:?}",
                        expression.preset
                    )));
                }
                value[key] = name.as_str().into();
            }
        }
        presets.insert(expression.preset.clone(), value);
    }
    let range = json!({ "inputMaxValue": LOOK_RANGE_DEGREES, "outputScale": 1.0 });
    writer.set_extension(
        "VRMC_vrm",
        json!({
            "specVersion": "1.0",
            "meta": {
                "name": figure.name,
                "authors": ["Sapphire's figure generator"],
                "licenseUrl": "https://vrm.dev/licenses/1.0/",
            },
            "humanoid": { "humanBones": humanoid },
            "expressions": { "preset": presets },
            "lookAt": {
                "offsetFromHeadBone": character.look_from,
                "type": "expression",
                "rangeMapHorizontalInner": range,
                "rangeMapHorizontalOuter": range,
                "rangeMapVerticalDown": range,
                "rangeMapVerticalUp": range,
            },
        }),
    );

    // Colliders sit on their joints, in the joint's frame at rest.
    let local = |joint: usize, point: [f32; 3]| {
        (rig.frames[joint].transpose() * (Vec3::from(point) - rig.at[joint])).to_array()
    };
    let mut colliders = Vec::with_capacity(character.colliders.len());
    for (index, collider) in character.colliders.iter().enumerate() {
        let at = joint(&collider.joint, &|| format!("collider {index}"))?;
        if collider.radius.is_nan() || collider.radius <= 0.0 {
            return Err(FigureError::Spec(format!("collider {index}'s radius must be more than 0")));
        }
        let shape = match collider.to {
            None => json!({ "sphere": { "offset": local(at, collider.at), "radius": collider.radius } }),
            Some(to) => json!({
                "capsule": { "offset": local(at, collider.at), "radius": collider.radius, "tail": local(at, to) }
            }),
        };
        colliders.push(json!({ "node": at, "shape": shape }));
    }
    let groups: Vec<Value> = if colliders.is_empty() {
        Vec::new()
    } else {
        vec![json!({ "name": "body", "colliders": (0..colliders.len()).collect::<Vec<_>>() })]
    };
    let copies = chain_copies(figure)?;
    let mut springs = Vec::new();
    for spec in &character.springs {
        let mut found = false;
        for (copy, _) in copies.iter().filter(|(copy, _)| copy.chain == spec.chain) {
            found = true;
            let joints: Vec<Value> = copy
                .joints
                .iter()
                .map(|name| {
                    let node = rig.find(name).expect("a chain's joints are in the skeleton");
                    json!({
                        "node": node,
                        "hitRadius": spec.hit_radius,
                        "stiffness": spec.stiffness,
                        "gravityPower": spec.gravity_power,
                        "gravityDir": [0.0, -1.0, 0.0],
                        "dragForce": spec.drag_force,
                    })
                })
                .collect();
            let mut spring = json!({ "name": copy.name, "joints": joints });
            if !groups.is_empty() {
                spring["colliderGroups"] = json!([0]);
            }
            springs.push(spring);
        }
        if !found {
            return Err(FigureError::Spec(format!("spring {:?} names no chain", spec.chain)));
        }
    }
    if !springs.is_empty() {
        writer.set_extension(
            "VRMC_springBone",
            json!({ "specVersion": "1.0", "colliders": colliders, "colliderGroups": groups, "springs": springs }),
        );
    }
    Ok(writer.to_glb())
}
