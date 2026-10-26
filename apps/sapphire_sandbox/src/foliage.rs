//! The foliage scene (`--foliage`, M1 W6): the pipeline's foliage samples planted in the
//! courtyard around the character, drawn by the foliage shader beside the environment's and the
//! character's, under the same time of day and a breeze.
//!
//! - **The plants** are content: a tree, a bush and a patch of grass, each one mesh with its
//!   origin at its root and its slots bound to foliage materials, loaded by ID through the asset
//!   server ([`Content::foliage`]). [`placements`] plants two trees, three bushes and grass
//!   patches on the courtyard's flat middle, behind and beside the character, clear of its
//!   rocks, pillars and lantern posts. They never move but for the wind, which the shader adds,
//!   so their instances are extracted once.
//! - **The wind** is the scene's (`--wind`, a breeze by default): plain data in every frame's
//!   packet ([`DEFAULT_WIND`]).
//! - **The camera** depends on the view [`FoliageView`] names: the grove from in front, with the
//!   character in it; the big tree against the key light, for light through its leaves; or low
//!   over the grass and a bush.
//!
//! [`Content::foliage`]: crate::content::Content::foliage

use sapphire_assets::AssetId;
use sapphire_math::{Mat4, Quat, Vec3};
use sapphire_render::{MeshHandle, Wind};

use crate::orbit::OrbitCamera;

/// The plants, by their sidecars' IDs (`content/samples/foliage/*.glb.meta`, the pipeline's
/// `samples::foliage` IDs).
pub(crate) const TREE: AssetId = AssetId::from_text("2feef8a7-6b16-4c41-ae6d-44b73cdabb44");
pub(crate) const BUSH: AssetId = AssetId::from_text("a24406d9-3d40-4468-b165-d8eaa06d10bf");
pub(crate) const GRASS: AssetId = AssetId::from_text("14d8adcb-6683-4cdd-babd-a3433d3dac33");

/// The breeze `--foliage` blows unless `--wind` says otherwise: across the courtyard, slightly
/// toward the camera, at the strength the materials are authored at, gusting a little.
pub(crate) const DEFAULT_WIND: Wind =
    Wind { direction: Vec3::new(0.944, 0.0, 0.33), strength: 1.0, gustiness: 0.3 };

/// How the camera shows the plants (`--foliage`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum FoliageView {
    /// The grove from in front of the character, the trees whole.
    Grove,
    /// The big tree from its far side, against the key light: light through its leaves.
    Backlit,
    /// Low over the grass in front of a bush.
    Ground,
}

impl FoliageView {
    /// The view `--foliage` names.
    pub fn from_name(name: &str) -> Option<Self> {
        match name {
            "grove" => Some(Self::Grove),
            "backlit" => Some(Self::Backlit),
            "ground" => Some(Self::Ground),
            _ => None,
        }
    }
}

/// A plant's mesh.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Plant {
    Tree,
    Bush,
    Grass,
}

/// The plants' meshes on a renderer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct FoliageMeshes {
    pub tree: MeshHandle,
    pub bush: MeshHandle,
    pub grass: MeshHandle,
}

impl FoliageMeshes {
    pub fn get(&self, plant: Plant) -> MeshHandle {
        match plant {
            Plant::Tree => self.tree,
            Plant::Bush => self.bush,
            Plant::Grass => self.grass,
        }
    }
}

/// The big tree's root, behind the character to its right.
const BIG_TREE: Vec3 = Vec3::new(4.9, 0.0, -3.9);
/// Its crown's middle, above its root: what the backlit view looks at.
const CROWN: Vec3 = Vec3::new(0.0, 3.4, 0.0);

/// Every plant with its placement: `(plant, x, z, scale, turn in radians)`. All stand on the
/// courtyard's flat middle (7 m around the character), clear of its rocks, colonnade and lantern
/// posts.
const PLANTS: [(Plant, f32, f32, f32, f32); 14] = [
    (Plant::Tree, BIG_TREE.x, BIG_TREE.z, 1.0, 0.4),
    (Plant::Tree, -3.8, 0.6, 0.72, 2.1),
    (Plant::Bush, 1.7, -2.3, 1.0, 0.0),
    (Plant::Bush, -1.6, -3.4, 1.2, 1.3),
    (Plant::Bush, 3.1, -0.2, 0.85, 2.6),
    (Plant::Grass, 0.4, -3.6, 1.0, 0.3),
    (Plant::Grass, 3.4, -4.6, 1.0, 1.1),
    (Plant::Grass, 5.4, -2.6, 0.9, 2.0),
    (Plant::Grass, -1.6, -5.2, 1.0, 0.7),
    (Plant::Grass, 2.4, -0.6, 0.9, 1.6),
    (Plant::Grass, 2.6, 2.0, 1.0, 0.2),
    (Plant::Grass, 4.8, 0.6, 1.0, 2.4),
    (Plant::Grass, -2.4, -1.0, 0.85, 1.9),
    (Plant::Grass, 0.2, 2.6, 0.8, 0.9),
];

/// Where the plants stand: each plant with its world transform, about its root.
pub(crate) fn placements() -> impl Iterator<Item = (Plant, Mat4)> {
    PLANTS.into_iter().map(|(plant, x, z, scale, turn)| {
        let place = Mat4::from_scale_rotation_translation(
            Vec3::splat(scale),
            Quat::from_rotation_y(turn),
            Vec3::new(x, 0.0, z),
        );
        (plant, place)
    })
}

/// The camera for `view`, with the key light shining toward `-toward_light` (the backlit view
/// faces it).
pub(crate) fn camera(view: FoliageView, toward_light: Vec3) -> OrbitCamera {
    match view {
        FoliageView::Grove => {
            let mut camera = OrbitCamera::fitting(Vec3::new(1.0, 2.6, -2.4), (7.6, 3.5));
            camera.look_from(0.25, 0.1);
            camera
        }
        FoliageView::Backlit => {
            // From the tree's far side, looking a little up through its crown toward the light.
            let toward =
                Vec3::new(toward_light.x, 0.0, toward_light.z).try_normalize().unwrap_or(Vec3::NEG_Z);
            let mut camera = OrbitCamera::fitting(BIG_TREE + CROWN, (3.6, 2.6));
            camera.look_from((-toward.x).atan2(-toward.z), -0.12);
            camera
        }
        FoliageView::Ground => {
            let mut camera = OrbitCamera::fitting(Vec3::new(2.9, 0.5, -0.6), (1.9, 1.05));
            camera.look_from(0.6, 0.18);
            camera
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn views_have_names() {
        for (name, view) in [
            ("grove", FoliageView::Grove),
            ("backlit", FoliageView::Backlit),
            ("ground", FoliageView::Ground),
        ] {
            assert_eq!(FoliageView::from_name(name), Some(view));
        }
        assert_eq!(FoliageView::from_name("forest"), None);
    }

    #[test]
    fn plants_stand_on_the_flat_ground_clear_of_the_character() {
        for (plant, place) in placements() {
            let root = place.transform_point3(Vec3::ZERO);
            assert_eq!(root.y, 0.0);
            let distance = root.length();
            assert!(distance < 7.0, "{plant:?} at {root} is past the courtyard's flat middle");
            if plant != Plant::Grass {
                assert!(distance > 2.0, "{plant:?} at {root} crowds the character");
            }
        }
    }

    #[test]
    fn the_backlit_view_faces_the_light_through_the_crown() {
        let toward_light = Vec3::new(-0.6, 0.2, -0.8);
        let camera = camera(FoliageView::Backlit, toward_light);
        let crown = BIG_TREE + CROWN;
        let looking = (crown - camera.eye()).normalize();
        let horizontal = Vec3::new(toward_light.x, 0.0, toward_light.z).normalize();
        assert!(looking.dot(horizontal) > 0.95, "toward the light: {looking}");
        assert!(looking.y > 0.0, "a little up into the crown");
    }
}
