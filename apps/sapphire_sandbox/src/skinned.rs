//! The skinned test subject (`--skinned`, M1 R2): a procedural stalk beside the character,
//! skinned to a chain of six bones and with two morph targets, that sways and breathes. It
//! exercises compute skinning, morph targets and their velocity until skinned sample characters
//! arrive (M1 A6's VRM import).
//!
//! - **Mesh** ([`stalk_mesh`]): a capsule [`HEIGHT`] tall, built as a surface of revolution
//!   (rings of 32 vertices, hemispherical caps). Each vertex blends the two bones nearest it:
//!   tent weights centered on each bone's middle, so the chain bends smoothly. The morph targets
//!   `bulb` (swells the top) and `pinch` (narrows the middle) list only the rings they move, with
//!   their exact normal deltas.
//! - **Rig**: bone `k` starts at joint `k`, `k` bone lengths up the stalk; the palette is one
//!   entry per bone, `model_from_joint × inverse_bind`, where the inverse bind undoes the joint's
//!   height.
//! - **Animation**: [`Sway`] advances a phase on the fixed step from its entity clock (so world
//!   slow motion and pauses apply), and [`pose`] builds the pose from it at the head of
//!   `Extract`, as the turntable's pose does (see [`crate::scene`]): each bone turns a little,
//!   out of phase along the chain, and both weights breathe. A pose is written only when it
//!   changes, so a paused stalk isn't extracted and costs nothing to draw.

use std::f32::consts::{PI, TAU};

use sapphire_assets::material::{AnimeCharacterMaterial, CharacterRegion, Material, Outline, Rim};
use sapphire_assets::mesh::{
    Influences, Joint, JointTransform, MaterialSlot, Mesh, MorphDelta, MorphTarget, PositionFormat, Skin,
    SkinBind, Submesh, Vertex, VertexInfluence, quantize_weights,
};
use sapphire_assets::texture::Swizzle;
use sapphire_ecs::prelude::*;
use sapphire_math::{Affine3A, Mat4, Quat, Vec3};
use sapphire_render::{MeshHandle, RenderError, Renderer};

use crate::extract::RenderPose;

/// The stalk's height and radius in its own space, in meters.
const HEIGHT: f32 = 1.5;
const RADIUS: f32 = 0.1;
/// Bones in the chain, each `HEIGHT / BONES` long.
const BONES: usize = 6;
const BONE_LENGTH: f32 = HEIGHT / BONES as f32;
/// Vertices around each ring, and rings in each cap and along the body.
const SEGMENTS: u32 = 32;
const CAP_RINGS: u32 = 8;
const BODY_RINGS: u32 = 48;

/// The stalk's height as a fraction of the character's.
const RELATIVE_HEIGHT: f32 = 0.7;

/// Radians per second of sway-phase time.
const SPEED: f32 = 1.6;

/// The morph targets: name, center height, half-width and change of radius.
const TARGETS: [(&str, f32, f32, f32); 2] = [("bulb", 1.15, 0.2, 0.07), ("pinch", 0.6, 0.18, -0.04)];

/// The stalk's sway phase, advanced on the fixed step like the turntable's angle.
#[derive(Component, Debug, Default)]
pub(crate) struct Sway {
    /// Phase after the latest fixed step, kept in [0, TAU × 10), a whole number of every
    /// motion's periods short of overflow (the slowest repeats every 10 turns of the phase).
    phase: f32,
    /// Phase after the step before, possibly negative after wrapping.
    previous: f32,
}

impl Sway {
    fn advance(&mut self, seconds: f32) {
        const WRAP: f32 = TAU * 10.0;
        self.previous = self.phase;
        self.phase += SPEED * seconds;
        if self.phase >= WRAP {
            self.phase -= WRAP;
            self.previous -= WRAP;
        }
    }

    /// The phase to draw, `alpha` of the way from the previous step to the latest.
    fn phase(&self, alpha: f32) -> f32 {
        self.previous + (self.phase - self.previous) * alpha
    }
}

/// Advances every stalk by its clock's advance this step. Runs in `FixedCombat`.
pub(crate) fn sway(mut stalks: Query<'_, '_, (&mut Sway, &EntityClock)>) {
    for (mut sway, clock) in &mut stalks {
        sway.advance(clock.0.last_delta().as_secs_f32());
    }
}

/// Poses each stalk between its last two steps. Runs at the head of `Extract`, before
/// extraction; writes only poses that changed.
pub(crate) fn pose(
    interpolation: Res<'_, Interpolation>,
    mut stalks: Query<'_, '_, (&Sway, &mut RenderPose)>,
) {
    for (sway, mut pose) in &mut stalks {
        let (palette, weights) = stalk_pose(sway.phase(interpolation.alpha));
        if pose.palette != palette || pose.morph_weights != weights {
            let pose = &mut *pose;
            pose.palette.copy_from_slice(&palette);
            pose.morph_weights.copy_from_slice(&weights);
        }
    }
}

/// A stalk's rest pose, the shape [`pose`] writes into: identity palette, weights 0.
pub(crate) fn rest_pose() -> RenderPose {
    RenderPose { palette: vec![Affine3A::IDENTITY; BONES], morph_weights: vec![0.0; TARGETS.len()] }
}

/// The stalk's pose at `phase`: each bone turns about Z and X, out of phase along the chain,
/// and the morph weights breathe between 0 and 1.
fn stalk_pose(phase: f32) -> ([Affine3A; BONES], [f32; TARGETS.len()]) {
    let mut palette = [Affine3A::IDENTITY; BONES];
    let mut model_from_joint = Affine3A::IDENTITY;
    for (bone, entry) in palette.iter_mut().enumerate() {
        let k = bone as f32;
        let turn = Quat::from_rotation_z(0.16 * (phase - 0.7 * k).sin())
            * Quat::from_rotation_x(0.07 * (0.8 * phase - 0.5 * k + 1.0).sin());
        let up = if bone == 0 { Vec3::ZERO } else { Vec3::new(0.0, BONE_LENGTH, 0.0) };
        model_from_joint *= Affine3A::from_rotation_translation(turn, up);
        *entry = model_from_joint * inverse_bind(bone);
    }
    let weights = [0.5 + 0.5 * (1.3 * phase).sin(), 0.5 + 0.5 * (0.9 * phase + 2.0).sin()];
    (palette, weights)
}

/// Undoes bone `bone`'s joint height: the bind-space position relative to the joint.
fn inverse_bind(bone: usize) -> Affine3A {
    Affine3A::from_translation(Vec3::new(0.0, -BONE_LENGTH * bone as f32, 0.0))
}

/// Where a stalk stands beside a character whose object-space bounds are `bounds`: right of
/// it, on the same floor, scaled to [`RELATIVE_HEIGHT`] of its height.
pub(crate) fn placement(bounds: [[f32; 3]; 2]) -> Mat4 {
    let [min, max] = bounds.map(Vec3::from);
    let height = (max.y - min.y).max(0.1);
    let scale = RELATIVE_HEIGHT * height / HEIGHT;
    let base = Vec3::new(max.x + 0.25 * height, min.y, (min.z + max.z) * 0.5);
    Mat4::from_scale_rotation_translation(Vec3::splat(scale), Quat::IDENTITY, base)
}

/// Builds, cooks and uploads the stalk, the way content is loaded: cooked bytes read in place,
/// with the anime character material it embeds.
pub(crate) fn upload_stalk(renderer: &mut Renderer) -> Result<MeshHandle, RenderError> {
    let bytes = stalk_mesh().to_cooked().expect("a procedural mesh cooks");
    let mesh = Mesh::read(&bytes)?;
    let materials = renderer.upload_embedded_materials(mesh)?;
    renderer.upload_mesh(mesh, &materials)
}

/// The stalk (see the module docs).
pub(crate) fn stalk_mesh() -> Mesh {
    // Rings bottom to top: height, radius, and the normal's radial and vertical parts.
    let mut rings: Vec<(f32, f32, f32, f32)> = Vec::new();
    for i in 0..=CAP_RINGS {
        let angle = -0.5 * PI + 0.5 * PI * i as f32 / CAP_RINGS as f32;
        rings.push((RADIUS + RADIUS * angle.sin(), RADIUS * angle.cos(), angle.cos(), angle.sin()));
    }
    for i in 1..BODY_RINGS {
        let y = RADIUS + (HEIGHT - 2.0 * RADIUS) * i as f32 / BODY_RINGS as f32;
        rings.push((y, RADIUS, 1.0, 0.0));
    }
    for i in 0..=CAP_RINGS {
        let angle = 0.5 * PI * i as f32 / CAP_RINGS as f32;
        rings.push((HEIGHT - RADIUS + RADIUS * angle.sin(), RADIUS * angle.cos(), angle.cos(), angle.sin()));
    }
    let mut vertices = Vec::new();
    for &(y, radius, radial, vertical) in &rings {
        // One more vertex than segments: the seam's UVs differ.
        for segment in 0..=SEGMENTS {
            let angle = TAU * segment as f32 / SEGMENTS as f32;
            let (sin, cos) = angle.sin_cos();
            let normal = [radial * cos, vertical, radial * sin];
            vertices.push(Vertex {
                position: [radius * cos, y, radius * sin],
                normal,
                outline_normal: normal,
                uv: [segment as f32 / SEGMENTS as f32, y / HEIGHT],
                color: [255; 4],
            });
        }
    }
    let stride = SEGMENTS + 1;
    let mut indices = Vec::new();
    for ring in 0..rings.len() as u32 - 1 {
        for segment in 0..SEGMENTS {
            let a = ring * stride + segment;
            let (b, c, d) = (a + stride, a + 1, a + stride + 1);
            // Counter-clockwise seen from outside.
            indices.extend([a, b, c, c, b, d]);
        }
    }
    let material = AnimeCharacterMaterial {
        base_color: [0.16, 0.62, 0.66, 1.0],
        regions: vec![CharacterRegion {
            shade_color: [0.55, 0.62, 0.85],
            outline_color: [0.04, 0.1, 0.12],
            ..CharacterRegion::default()
        }],
        rim: Rim { strength: 0.35, ..Rim::default() },
        outline: Outline { width: 4.0, ..Outline::default() },
        ..AnimeCharacterMaterial::default()
    };
    let swizzles = [Swizzle::IDENTITY; AnimeCharacterMaterial::TEXTURE_SLOTS.len()];
    let material = Material::from_document(&material, &swizzles).expect("the stalk's material is valid");
    let submeshes = vec![Submesh { first_index: 0, index_count: indices.len() as u32, material: 0 }];
    let slots = vec![MaterialSlot::embedded("stalk", material)];
    let mut mesh = Mesh::from_vertices(&vertices, indices, submeshes, slots, PositionFormat::Unorm16);

    let influences: Vec<VertexInfluence> = vertices
        .iter()
        .map(|vertex| {
            // In bones up the chain, held within the outer bones' middles.
            let along = (vertex.position[1] / BONE_LENGTH).clamp(0.5, BONES as f32 - 0.5);
            let lower = ((along - 0.5).floor() as usize).min(BONES - 2);
            let upper_weight = along - 0.5 - lower as f32;
            VertexInfluence {
                binds: [lower as u16, lower as u16 + 1, 0, 0],
                weights: quantize_weights([1.0 - upper_weight, upper_weight, 0.0, 0.0]),
            }
        })
        .collect();
    let joints = (0..BONES)
        .map(|bone| {
            let mut rest = JointTransform::IDENTITY;
            if bone > 0 {
                rest.translation = [0.0, BONE_LENGTH, 0.0];
            }
            Joint { name: format!("stalk_{bone}"), parent: bone.checked_sub(1).map(|p| p as u16), rest }
        })
        .collect();
    let binds = (0..BONES)
        .map(|bone| SkinBind {
            joint: bone as u16,
            inverse_bind_matrix: Mat4::from(inverse_bind(bone)).to_cols_array_2d(),
        })
        .collect();
    mesh.skin = Some(Skin { joints, binds, influences: Influences::pack(&influences, BONES) });

    for (name, center, half_width, change) in TARGETS {
        let first_delta = mesh.morph_deltas.len() as u32;
        for (index, vertex) in vertices.iter().enumerate() {
            let [x, y, z] = vertex.position;
            let offset = (y - center) / half_width;
            if offset.abs() >= 1.0 {
                continue;
            }
            // A raised cosine on the radius, and the normal of the surface it makes:
            // (cos φ, −dr/dy, sin φ) for a surface of revolution r(y).
            let grow = change * 0.5 * (1.0 + (PI * offset).cos());
            let slope = -change * 0.5 * PI / half_width * (PI * offset).sin();
            let outward = Vec3::new(x, 0.0, z).normalize();
            let normal = Vec3::new(outward.x, -slope, outward.z).normalize();
            let delta = MorphDelta::new(
                index as u32,
                (outward * grow).to_array(),
                (normal - Vec3::from(vertex.normal)).to_array(),
            );
            mesh.morph_deltas.push(delta);
        }
        let delta_count = mesh.morph_deltas.len() as u32 - first_delta;
        mesh.morph_targets.push(MorphTarget {
            name: name.into(),
            default_weight: 0.0,
            first_delta,
            delta_count,
        });
    }
    mesh
}

#[cfg(test)]
mod tests {
    use sapphire_time::{SimDuration, TICK_HZ};

    use super::*;

    #[test]
    fn the_stalk_is_a_valid_sparse_skinned_mesh() {
        let mesh = stalk_mesh();
        let bytes = mesh.to_cooked().expect("cook");
        let read = Mesh::read(&bytes).expect("a valid mesh");
        let skin = read.skin.as_ref().expect("skinned");
        assert_eq!((skin.joints.len(), skin.binds.len()), (BONES, BONES));
        let vertices = mesh.streams.len();
        assert_eq!(vertices, (2 * (CAP_RINGS + 1) + BODY_RINGS - 1) as usize * (SEGMENTS as usize + 1));
        for target in &mesh.morph_targets {
            assert!(
                target.delta_count > 0 && (target.delta_count as usize) < vertices / 3,
                "sparse: {target:?}"
            );
        }
    }

    #[test]
    fn poses_bend_the_chain_from_its_joints() {
        for phase in [0.0, 1.0, 4.0] {
            let (palette, weights) = stalk_pose(phase);
            assert!(weights.iter().all(|w| (0.0..=1.0).contains(w)), "{weights:?}");
            // Each bone carries its own joint to where its parent's bone ends.
            for bone in 1..BONES {
                let joint = Vec3::new(0.0, BONE_LENGTH * bone as f32, 0.0);
                let by_parent = palette[bone - 1].transform_point3(joint);
                let by_self = palette[bone].transform_point3(joint);
                assert!(
                    by_parent.distance(by_self) < 1e-5,
                    "phase {phase}, bone {bone}: the chain stays joined"
                );
            }
        }
        let tip = Vec3::new(0.0, HEIGHT, 0.0);
        let sway =
            (0..16).map(|step| stalk_pose(step as f32 * 0.4).0[BONES - 1].transform_point3(tip).x.abs());
        let widest = sway.fold(0.0, f32::max);
        assert!(widest > 0.1, "the tip sways: {widest} m at most");
    }

    #[test]
    fn sway_wraps_without_jumping() {
        let tick = SimDuration::from_ticks(1).as_secs_f32();
        let mut sway = Sway::default();
        for _ in 0..TICK_HZ {
            sway.advance(tick);
        }
        assert!((sway.phase(1.0) - SPEED).abs() < 1e-4);
        sway.phase = TAU * 10.0 - 0.001;
        sway.advance(tick);
        assert!(sway.phase < 1.0 && sway.phase(0.0) < sway.phase(1.0), "the short way round");
        let (before, _) = stalk_pose(sway.phase(0.0));
        let (after, _) = stalk_pose(sway.phase(0.0) + TAU * 10.0);
        let close = |a: &Affine3A, b: &Affine3A| a.abs_diff_eq(*b, 1e-3);
        assert!(before.iter().zip(&after).all(|(a, b)| close(a, b)), "the wrap is a whole number of periods");
    }
}
