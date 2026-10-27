//! The rim look-dev rig (`--rim-rig angles|backlit`, M1 C5): a row of the character before a
//! backdrop of dark and bright panels, each figure lit by a key light of its own from an angle
//! around it, the way an artist checks a rim: on the lit side and the shadow side, against dark
//! and bright, and back-lit.
//!
//! - **The figures** are the character without its pedestal (the stage's figure), each a
//!   character lit by the default light rig preset with its key light turned to its [`RimKey`]
//!   around it, as the camera sees it ([`RimKey::toward`]). They face the camera, upright, so the
//!   angles read the same across the row.
//! - **The backdrop** stands behind them: panels of a dark and a light environment material in
//!   turn, a seam behind each figure's middle as the camera sees it, so each figure's rim shows
//!   against both. The light panels glow a little (their emission), so they read the same at
//!   any hour.
//! - **Layouts** ([`RimLayout`]): `angles` lights the figures from the front, the side, behind
//!   and to the side, and across from the other side, so the rim's light gate shows; `backlit`
//!   lights them from behind at several heights, where ZZZ's back-light weighting (the sample
//!   mannequin's outfit has it) strengthens the rim; `folded` lights them from behind too, but
//!   through the gameplay rig's limits, which fold each key round toward the camera's side, as
//!   play does (M1 C8): the rim follows the folded key, unless the rig's camera rim
//!   (`--camera-rim`) holds it behind.
//!
//! Nothing moves, so the instances are extracted once; the figures' characters are listed every
//! frame, as every character's are.

use sapphire_assets::material::{EnvironmentMaterial, Material, Terminator};
use sapphire_assets::mesh::{
    MaterialSlot, Mesh, PositionFormat, Submesh, VERTEX_COLOR_NEUTRAL, Vertex, VertexFlags,
};
use sapphire_assets::texture::Swizzle;
use sapphire_ecs::prelude::*;
use sapphire_math::{Mat4, Quat, Vec3};
use sapphire_platform::AppInitError;
use sapphire_render::{MeshHandle, Renderer};

use crate::orbit::OrbitCamera;
use crate::stage;

/// Figures in the row.
pub(crate) const FIGURES: usize = 4;
/// Panels in the backdrop: one either side of each seam.
pub(crate) const PANELS: usize = FIGURES + 1;

/// Meters between neighboring figures: their pedestals clear each other.
const SPACING: f32 = 1.2;
/// How far behind the figures the backdrop stands, in meters, across the camera's view.
const BACKDROP_DISTANCE: f32 = 1.6;
/// The outer panels' width, past the outer figures' seams, and every panel's height, in meters:
/// enough to fill the view.
const OUTER_PANEL_WIDTH: f32 = 4.0;
const PANEL_HEIGHT: f32 = 7.0;

/// A figure's key light: its angles around the figure as the camera sees it, in degrees. Yaw 0
/// lights it from the camera, 90 from the image's right, 180 from behind it, negative from the
/// image's left; elevation is above the horizon.
#[derive(Component, Clone, Copy, Debug, PartialEq)]
pub(crate) struct RimKey {
    pub yaw: f32,
    pub elevation: f32,
    /// Through the figure's light rig's limits (M1 C8), as play lights a character, rather than
    /// exactly at its angles.
    pub folded: bool,
}

impl RimKey {
    /// The unit vector toward the key light for a figure at `at` seen from a camera at `eye`.
    pub fn toward(&self, eye: Vec3, at: Vec3) -> Vec3 {
        let back = Vec3::new(at.x - eye.x, 0.0, at.z - eye.z).normalize_or(Vec3::NEG_Z);
        let right = back.cross(Vec3::Y);
        let (yaw, elevation) = (self.yaw.to_radians(), self.elevation.to_radians());
        (-back * yaw.cos() + right * yaw.sin()) * elevation.cos() + Vec3::Y * elevation.sin()
    }
}

/// Which keys the figures take (`--rim-rig`): see the module docs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RimLayout {
    Angles,
    Backlit,
    Folded,
}

impl RimLayout {
    /// The layout `--rim-rig` names.
    pub fn from_name(name: &str) -> Option<Self> {
        match name {
            "angles" => Some(Self::Angles),
            "backlit" => Some(Self::Backlit),
            "folded" => Some(Self::Folded),
            _ => None,
        }
    }

    /// The figures' keys, left to right as the camera sees them.
    pub fn keys(self) -> [RimKey; FIGURES] {
        let key = |yaw, elevation| RimKey { yaw, elevation, folded: false };
        let folded = |yaw, elevation| RimKey { yaw, elevation, folded: true };
        match self {
            Self::Angles => [key(0.0, 30.0), key(75.0, 20.0), key(140.0, 25.0), key(-110.0, 15.0)],
            Self::Backlit => [key(180.0, 15.0), key(155.0, 35.0), key(-150.0, 10.0), key(180.0, 55.0)],
            Self::Folded => {
                [folded(170.0, 20.0), folded(145.0, 35.0), folded(-155.0, 15.0), folded(-130.0, 45.0)]
            }
        }
    }
}

/// A backdrop panel's material.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Panel {
    Dark,
    Bright,
}

/// The rig's meshes on a renderer: the figure and the backdrop's panels.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct RimRigMeshes {
    pub figure: MeshHandle,
    pub dark: MeshHandle,
    pub bright: MeshHandle,
}

impl RimRigMeshes {
    /// The mesh a backdrop panel draws.
    pub fn panel(&self, panel: Panel) -> MeshHandle {
        match panel {
            Panel::Dark => self.dark,
            Panel::Bright => self.bright,
        }
    }
}

/// Where the rig stands beside a character whose object-space bounds are `bounds`: well to its
/// right, on the same ground, so the camera framing the rig leaves the character out, and the
/// backdrop hides it.
fn center(bounds: [[f32; 3]; 2]) -> Vec3 {
    let [min, max] = bounds.map(Vec3::from);
    let height = (max.y - min.y).max(0.1);
    Vec3::new(max.x + 3.5 * height, min.y + 0.5 * height, (min.z + max.z) * 0.5)
}

/// The camera framing the rig of figures whose object-space bounds are `bounds`.
pub(crate) fn camera(bounds: [[f32; 3]; 2]) -> OrbitCamera {
    let [min, max] = bounds.map(Vec3::from);
    let half_width = 0.5 * (FIGURES as f32 - 1.0) * SPACING + 0.5 * (max.x - min.x);
    OrbitCamera::fitting(center(bounds), (half_width, 0.5 * (max.y - min.y)))
}

/// The camera's level directions at the rig: across to its right, and away from it.
fn axes(camera: &OrbitCamera, bounds: [[f32; 3]; 2]) -> (Vec3, Vec3) {
    let at = center(bounds);
    let eye = camera.eye();
    let back = Vec3::new(at.x - eye.x, 0.0, at.z - eye.z).normalize_or(Vec3::NEG_Z);
    (back.cross(Vec3::Y), back)
}

/// Each figure's key and placement, left to right as `camera` sees them: in a row across its
/// view on the character's ground, each upright and facing it (the model faces +Z).
pub(crate) fn figures(
    bounds: [[f32; 3]; 2],
    layout: RimLayout,
    camera: &OrbitCamera,
) -> [(RimKey, Mat4); FIGURES] {
    let (right, _) = axes(camera, bounds);
    let center = center(bounds);
    let base = Vec3::new(center.x, 0.0, center.z);
    let keys = layout.keys();
    std::array::from_fn(|index| {
        let at = base + right * offset(index);
        let toward_camera = camera.eye() - at;
        let facing = Quat::from_rotation_y(toward_camera.x.atan2(toward_camera.z));
        (keys[index], Mat4::from_rotation_translation(facing, at))
    })
}

/// Figure `index`'s offset across the camera's view from the rig's middle, in meters.
fn offset(index: usize) -> f32 {
    (index as f32 - 0.5 * (FIGURES as f32 - 1.0)) * SPACING
}

/// The backdrop's panels and placements: left to right as `camera` sees them, dark and bright in
/// turn, each a vertical quad facing the camera, with a seam behind each figure's middle as the
/// camera sees it.
pub(crate) fn panels(bounds: [[f32; 3]; 2], camera: &OrbitCamera) -> [(Panel, Mat4); PANELS] {
    let (right, back) = axes(camera, bounds);
    let center = center(bounds);
    let base = Vec3::new(center.x, 0.0, center.z);
    // The camera across the view and in front of the row, and where its ray through each figure
    // meets the backdrop.
    let eye = camera.eye() - base;
    let (eye_across, eye_depth) = (eye.dot(right), -eye.dot(back));
    let reach = 1.0 + BACKDROP_DISTANCE / eye_depth.max(0.1);
    let seam = |index: usize| eye_across + (offset(index) - eye_across) * reach;
    let wall = base + back * BACKDROP_DISTANCE + Vec3::Y * center.y;
    let facing = Quat::from_rotation_y((-back.x).atan2(-back.z));
    std::array::from_fn(|index| {
        // Panel `index` spans from seam `index − 1` to seam `index`; the outer ones run on.
        let (left, right_edge) = match index {
            0 => (seam(0) - OUTER_PANEL_WIDTH, seam(0)),
            _ if index == FIGURES => (seam(FIGURES - 1), seam(FIGURES - 1) + OUTER_PANEL_WIDTH),
            _ => (seam(index - 1), seam(index)),
        };
        let middle = wall + right * (0.5 * (left + right_edge));
        let panel = if index % 2 == 0 { Panel::Dark } else { Panel::Bright };
        let size = Vec3::new(right_edge - left, PANEL_HEIGHT, 1.0);
        (panel, Mat4::from_scale_rotation_translation(size, facing, middle))
    })
}

/// Uploads the figure, the `character` without its pedestal, and the backdrop's two panels, each
/// with its embedded material.
pub(crate) fn upload_rig(renderer: &mut Renderer, character: &Mesh) -> Result<RimRigMeshes, AppInitError> {
    Ok(RimRigMeshes {
        figure: stage::upload_figure(renderer, character)?,
        dark: stage::upload(renderer, &panel_mesh(Panel::Dark))?,
        bright: stage::upload(renderer, &panel_mesh(Panel::Bright))?,
    })
}

/// A unit quad standing on its middle, facing +Z: a panel before its placement's scale.
fn panel_geometry() -> (Vec<Vertex>, Vec<u32>) {
    let vertices = [(-0.5, -0.5), (0.5, -0.5), (0.5, 0.5), (-0.5, 0.5)]
        .into_iter()
        .map(|(x, y)| Vertex {
            position: [x, y, 0.0],
            normal: [0.0, 0.0, 1.0],
            outline_normal: [0.0, 0.0, 1.0],
            uv: [x + 0.5, 0.5 - y],
            color: VERTEX_COLOR_NEUTRAL,
            flags: VertexFlags::NONE,
        })
        .collect();
    (vertices, vec![0, 1, 2, 0, 2, 3])
}

/// A panel's material: a near-black matte, or a warm light grey that glows a little.
fn panel_material(panel: Panel) -> EnvironmentMaterial {
    let (base, emission) = match panel {
        Panel::Dark => (0.02, [0.0; 3]),
        Panel::Bright => (0.3, [0.42, 0.4, 0.36]),
    };
    EnvironmentMaterial {
        base_color: [base, base, base * 1.1, 1.0],
        roughness: 0.9,
        specular_clamp: 0.0,
        wrap: 0.3,
        terminator: Terminator { width: 0.04, growth: 0.006, band: None },
        emission,
        ..EnvironmentMaterial::default()
    }
}

fn panel_mesh(panel: Panel) -> Mesh {
    let (vertices, indices) = panel_geometry();
    let swizzles = [Swizzle::IDENTITY; EnvironmentMaterial::TEXTURE_SLOTS.len()];
    let material =
        Material::from_document(&panel_material(panel), &swizzles).expect("the panels' materials are valid");
    let submeshes = vec![Submesh { first_index: 0, index_count: indices.len() as u32, material: 0 }];
    let name = match panel {
        Panel::Dark => "dark panel",
        Panel::Bright => "bright panel",
    };
    Mesh::from_vertices(
        &vertices,
        indices,
        submeshes,
        vec![MaterialSlot::embedded(name, material)],
        PositionFormat::Unorm16,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    const BOUNDS: [[f32; 3]; 2] = [[-0.55, 0.0, -0.55], [0.55, 2.1, 0.55]];

    #[test]
    fn keys_turn_around_each_figure_as_the_camera_sees_it() {
        let (eye, at) = (Vec3::new(0.0, 1.0, 5.0), Vec3::new(0.0, 1.0, 0.0));
        let toward = |yaw, elevation| RimKey { yaw, elevation, folded: false }.toward(eye, at);
        assert!(toward(0.0, 0.0).abs_diff_eq(Vec3::Z, 1e-6), "from the camera");
        assert!(toward(90.0, 0.0).abs_diff_eq(Vec3::X, 1e-6), "from the image's right");
        assert!(toward(180.0, 0.0).abs_diff_eq(Vec3::NEG_Z, 1e-6), "from behind");
        assert!(toward(-90.0, 90.0).abs_diff_eq(Vec3::Y, 1e-6), "from above");
    }

    #[test]
    fn figures_face_the_camera_in_a_row_before_the_seams() {
        let camera = camera(BOUNDS);
        let figures = figures(BOUNDS, RimLayout::Angles, &camera);
        let panels = panels(BOUNDS, &camera);
        let eye = camera.eye();
        for (index, (key, place)) in figures.iter().enumerate() {
            assert_eq!(*key, RimLayout::Angles.keys()[index]);
            let at = place.transform_point3(Vec3::ZERO);
            let forward = place.transform_vector3(Vec3::Z);
            let level = Vec3::new(eye.x - at.x, 0.0, eye.z - at.z).normalize();
            assert!(forward.abs_diff_eq(level, 1e-5), "facing the camera: {forward} {level}");
            assert_eq!(at.y, 0.0, "on the character's ground");
            assert!(at.x > 5.0, "clear of the character: {at}");
            // A seam behind the figure as the camera sees it: the panels either side meet there.
            let (_, left) = panels[index];
            let (_, right) = panels[index + 1];
            let edge = |panel: Mat4, side: f32| panel.transform_point3(Vec3::new(side * 0.5, 0.0, 0.0));
            assert!(edge(left, 1.0).abs_diff_eq(edge(right, -1.0), 1e-4), "panels meet");
            let (seam, figure) = (edge(left, 1.0) - eye, at - eye);
            let level = |v: Vec3| Vec3::new(v.x, 0.0, v.z).normalize();
            assert!(level(seam).cross(level(figure)).y.abs() < 1e-4, "the seam lies behind the figure");
            assert_ne!(panels[index].0, panels[index + 1].0, "dark and bright in turn");
        }
        // The panels stand in one plane facing the camera across the row's middle.
        let middle = panels[PANELS / 2].1.transform_point3(Vec3::ZERO);
        let toward_camera = Vec3::new(eye.x - middle.x, 0.0, eye.z - middle.z).normalize();
        for (_, place) in panels {
            let normal = place.transform_vector3(Vec3::Z).normalize();
            assert!(normal.abs_diff_eq(toward_camera, 1e-3), "{normal} {toward_camera}");
            assert!((place.transform_point3(Vec3::ZERO) - middle).dot(normal).abs() < 1e-3);
        }
    }

    #[test]
    fn panels_cook_and_face_their_normal() {
        for panel in [Panel::Dark, Panel::Bright] {
            let bytes = panel_mesh(panel).to_cooked().expect("cook");
            Mesh::read(&bytes).expect("a valid mesh");
        }
        let (vertices, indices) = panel_geometry();
        for triangle in indices.as_chunks::<3>().0 {
            let [a, b, c] = triangle.map(|index| Vec3::from(vertices[index as usize].position));
            assert!((b - a).cross(c - a).normalize().abs_diff_eq(Vec3::Z, 1e-6));
        }
    }

    #[test]
    fn layouts_are_named_as_the_command_line_spells_them() {
        assert_eq!(RimLayout::from_name("angles"), Some(RimLayout::Angles));
        assert_eq!(RimLayout::from_name("backlit"), Some(RimLayout::Backlit));
        assert_eq!(RimLayout::from_name("folded"), Some(RimLayout::Folded));
        assert!(
            RimLayout::Folded.keys().iter().all(|key| key.folded && key.yaw.abs() > 120.0),
            "from behind"
        );
        assert_eq!(RimLayout::from_name("front"), None);
    }
}
