//! The look-dev town (`--look-dev <shot>`, M1 X1): a town square in the anime-aesthetic look, a
//! character standing in it, and authored cameras to judge the look by. Every landed feature is
//! on: time of day and its sky, fog and fog cards, shadows, the environment family, post's
//! compositing layers, the outlines, and the character prepass's readers (the rim, the hair's
//! highlight, the bangs' shadow and eyes through the bangs).
//!
//! - **The town** is content: `content/lookdev/town/town.assembly.ron`, an assembly of Kenney's
//!   Fantasy Town Kit (`sapphire_pipeline::assembly`) cooked into one static mesh whose slots its
//!   sidecar binds to the town's environment materials ([`TOWN`]). It never moves, so it is
//!   extracted once.
//! - **The character** is pixiv's VRM sample (A6, [`CHARACTER`]) in place of the mannequin. VRM
//!   characters rest in a T-pose, so the stage holds a standing pose ([`held_pose`]): humanoid
//!   bones turned from rest, the rig's twist and aim constraints followed ([`crate::pose`]), and a
//!   soft expression. Its figure (head, feet and body capsule, its face's basis) comes from the
//!   posed skeleton. Its look-dev materials (`content/lookdev/character/`, bound by the VRM's
//!   sidecar) say which parts are face, hair and eyes, as MToon doesn't, so the character
//!   prepass's IDs tell them apart.
//! - **Shots** ([`Shot`]): a wide establishing shot across the square, a three-quarter hero shot
//!   of the character and a low angle up past her toward the sky and the rooftops. `--view`,
//!   `--fov` and `--zoom` adjust them as they do the turntable's framing.
//! - **Fog cards**, where frames draw fog: the courtyard's mist sheets and haze cards
//!   ([`crate::environment::FOG_CARDS`]), placed across the valley beyond the town and down the
//!   street out of the square.
//! - **Lamps**: a warm point light at each lantern's glass, its intensity following time of day's
//!   night ([`LAMPS`]), so the square lights up at dusk.
//! - **Its own day** ([`day`]): `content/lookdev/lookdev_day.ron`, a time-of-day timeline aimed at
//!   Wuthering Waves' light (X1 part 2), replaces the default day while the stage runs, so the
//!   default day and every other scene stay as they are. Tests here hold it to the character light
//!   contract and keep the square clear of haze.
//! - **Its own sky** ([`SKY`]): the sandbox sky's maps with a teal Milky Way and a larger sun.
//! - **Plants** ([`PLANTINGS`]): W6's foliage meshes, each its own mesh rooted at its origin so the
//!   wind sways it about its root, set in the square's verges, in the town's planters and pots,
//!   and beyond the rows; the stage blows a light breeze ([`WIND`]).
//!
//! The town's lanterns are placed in its assembly; [`LAMPS`] repeats their places, and a test
//! here checks the two against each other.

use sapphire_assets::AssetId;
use sapphire_assets::mesh::Mesh;
use sapphire_assets::rig::{ArchivedCharacterRig, HumanBone};
use sapphire_math::{Mat4, Quat, Vec3};
use sapphire_platform::AppInitError;
use sapphire_render::{LightKind, LocalLight, MeshHandle, Renderer};
use sapphire_rhi::GpuContext;
use sapphire_world::tod::TodTimeline;

use crate::content::Content;
use crate::environment::{CardPlace, HAZE, MIST};
use crate::extract::RenderPose;
use crate::orbit::OrbitCamera;
use crate::pose::{self, RigJoints, Skeleton};
use crate::scene::Figure;

/// The town, by its sidecar's ID (`content/lookdev/town/town.assembly.ron.meta`).
pub(crate) const TOWN: AssetId = AssetId::from_text("1ff3290e-5fa9-4f61-b891-2ca6d76b285f");
/// The character: pixiv's `VRM1_Constraint_Twist_Sample`, by its sidecar's ID
/// (`content/third_party/vrm1_constraint_twist_sample/VRM1_Constraint_Twist_Sample.vrm.meta`).
pub(crate) const CHARACTER: AssetId = AssetId::from_text("6f8a81b6-29c4-485f-b161-847b001d561f");
/// The character's rig: the VRM's `rig` sub-asset, whose ID the pipeline derives from the VRM's
/// (`sapphire_pipeline::meta::sub_asset_id`; its look-dev tests pin this value).
pub(crate) const RIG: AssetId = AssetId::from_text("4b4073fd-44c5-8437-a712-1cccad486da3");
/// The town's sky: the sandbox sky's maps with a teal Milky Way and a larger sun
/// (`content/lookdev/sky/sky.material.ron`), by its sidecar's ID.
pub(crate) const SKY: AssetId = AssetId::from_text("8c0e3b8e-9f2a-4d55-b7c1-2f6a4e1d9a73");

/// A plant the stage places in the town (`content/lookdev/foliage/`): W6's foliage samples in the
/// look-dev's own foliage materials, and two of the kit's trees as foliage meshes (single-piece
/// assemblies, their roots at their origins, which the wind sways them about).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Plant {
    /// The card tree, green: teal-green inside its clumps to a sunny yellow-green.
    GreenTree,
    /// The card tree in pink blossom.
    BlossomTree,
    /// The card tree in orange autumn leaves.
    AutumnTree,
    /// The card bush, green, and potted.
    GreenBush,
    /// The card bush in lavender.
    Lavender,
    /// A grass bed: the grass patch, 4 m across at scale 1, thinning toward its edge.
    Grass,
    /// Kenney's `tree-high-round` as foliage, 8.8 m tall at scale 1.
    KitTreeRound,
    /// Kenney's `tree` as foliage, 8.7 m tall at scale 1.
    KitTree,
}

impl Plant {
    pub const ALL: [Self; 8] = [
        Self::GreenTree,
        Self::BlossomTree,
        Self::AutumnTree,
        Self::GreenBush,
        Self::Lavender,
        Self::Grass,
        Self::KitTreeRound,
        Self::KitTree,
    ];

    /// The plant's mesh, by its sidecar's ID (`content/lookdev/foliage/*.meta`).
    pub fn id(self) -> AssetId {
        AssetId::from_text(match self {
            Self::GreenTree => "0e0f2a4c-b0bb-4e0a-8688-79e9b18f7b69",
            Self::BlossomTree => "33168a54-0264-4e4c-8de5-88666f3f307a",
            Self::AutumnTree => "b25d6067-c5cb-48c1-bf33-a7bc047e4f57",
            Self::GreenBush => "4d78be88-2b53-4709-bda0-27d4690b6b79",
            Self::Lavender => "fde8ea98-378a-4ddb-aa2f-2a2cae49cbe4",
            Self::Grass => "b9addce7-3eb0-4cad-9dbc-e3877e8a18fe",
            Self::KitTreeRound => "6a687812-181f-48da-b548-aad1d91b8ccb",
            Self::KitTree => "57d170c9-421c-423c-b9d1-560c05c32bad",
        })
    }

    fn index(self) -> usize {
        self as usize
    }
}

/// The town's meshes on a renderer: the town and each plant's, in [`Plant::ALL`] order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct TownMeshes {
    pub town: MeshHandle,
    pub plants: [MeshHandle; Plant::ALL.len()],
}

impl TownMeshes {
    pub fn plant(&self, plant: Plant) -> MeshHandle {
        self.plants[plant.index()]
    }
}

/// The breeze the town's plants sway in.
pub(crate) const WIND: sapphire_render::Wind = crate::foliage::DEFAULT_WIND;

/// A plant in the town: where its root stands, its scale (x, y, z) and its turn about +Y in
/// degrees.
type Planting = (Plant, [f32; 3], [f32; 3], f32);

/// Uniform scale.
const fn even(scale: f32) -> [f32; 3] {
    [scale, scale, scale]
}

/// The top of the assembly's planters and pots, where their plants' roots stand.
const PLANTER: f32 = 0.4;
const POT: f32 = 0.555;

/// The town's plants: the trees that were the assembly's kit trees, now foliage (a green and a
/// blossom tree in the back corners, the wide shot's left and the hero shot's background; kit
/// trees and two card trees in blossom and autumn over the rooftops); an autumn tree in a planter
/// framing the wide shot's left, with lavender at its foot; the beds before the back row and
/// beside the left row, grass with lavender and green bushes; and bushes in the pots between the
/// back row's doors and by the stalls. Planters and pots are the assembly's (`@planter`, `@pot`).
pub(crate) const PLANTINGS: [Planting; 32] = [
    // The square's corners, and beyond.
    (Plant::GreenTree, [-17.0, 0.0, -17.0], even(1.35), 20.0),
    (Plant::BlossomTree, [16.8, 0.0, -16.8], even(1.45), 40.0),
    (Plant::KitTree, [13.0, 0.0, 5.0], even(0.85), 15.0),
    (Plant::KitTreeRound, [11.5, 0.0, 1.5], even(1.0), 70.0),
    // Over the rooftops.
    (Plant::KitTreeRound, [-17.0, 0.0, -29.0], even(1.25), 0.0),
    (Plant::KitTree, [-20.0, 0.0, -34.0], even(1.22), 60.0),
    (Plant::KitTreeRound, [16.0, 0.0, -31.0], even(1.31), 20.0),
    (Plant::KitTree, [22.0, 0.0, -36.0], even(1.28), 0.0),
    (Plant::AutumnTree, [-1.0, 0.0, -48.0], even(2.0), 0.0),
    (Plant::BlossomTree, [12.0, 0.0, -50.0], even(2.0), 30.0),
    // The planter framing the wide shot (3.4 m square).
    (Plant::AutumnTree, [-3.0, PLANTER, 5.2], even(1.25), 200.0),
    (Plant::Grass, [-3.0, PLANTER, 5.2], [0.75, 1.0, 0.75], 0.0),
    (Plant::Lavender, [-4.1, PLANTER, 6.2], even(0.55), 0.0),
    (Plant::Lavender, [-1.9, PLANTER, 4.2], even(0.5), 60.0),
    // The bed before the back row, right of the gate (4.6 × 1.7 m).
    (Plant::Grass, [10.2, PLANTER, -14.6], [1.05, 1.0, 0.38], 0.0),
    (Plant::Lavender, [8.9, PLANTER, -14.6], even(0.6), 10.0),
    (Plant::GreenBush, [10.2, PLANTER, -14.5], even(0.5), 80.0),
    (Plant::Lavender, [11.5, PLANTER, -14.6], even(0.55), 140.0),
    // The bed before the back row, left of the gate (5 × 1.9 m).
    (Plant::Grass, [-8.0, PLANTER, -15.2], [1.15, 1.0, 0.42], 0.0),
    (Plant::GreenBush, [-9.4, PLANTER, -15.2], even(0.55), 30.0),
    (Plant::Lavender, [-8.0, PLANTER, -15.1], even(0.6), 200.0),
    (Plant::GreenBush, [-6.6, PLANTER, -15.3], even(0.5), 110.0),
    // The bed beside the left row (2.2 × 6 m).
    (Plant::Grass, [-12.6, PLANTER, -4.0], [0.5, 1.0, 1.35], 0.0),
    (Plant::Lavender, [-12.6, PLANTER, -6.0], even(0.6), 0.0),
    (Plant::GreenBush, [-12.6, PLANTER, -4.0], even(0.5), 70.0),
    (Plant::Lavender, [-12.6, PLANTER, -2.0], even(0.55), 150.0),
    // Potted plants between the back row's doors and by the stalls.
    (Plant::GreenBush, [-12.8, POT, -16.5], even(0.42), 0.0),
    (Plant::GreenBush, [-3.2, POT, -16.5], even(0.42), 90.0),
    (Plant::GreenBush, [9.6, POT, -16.5], even(0.42), 45.0),
    (Plant::GreenBush, [12.8, POT, -16.9], even(0.42), 135.0),
    (Plant::Lavender, [4.4, POT, -9.6], even(0.42), 20.0),
    (Plant::GreenBush, [15.2, POT, -8.4], even(0.42), 160.0),
];

/// Every plant with its world transform, about its root.
pub(crate) fn plantings() -> impl Iterator<Item = (Plant, Mat4)> {
    PLANTINGS.into_iter().map(|(plant, at, scale, turn)| {
        let place = Mat4::from_scale_rotation_translation(
            Vec3::from(scale),
            Quat::from_rotation_y(turn.to_radians()),
            Vec3::from(at),
        );
        (plant, place)
    })
}

/// The look-dev day (`content/lookdev/lookdev_day.ron`), compiled in as the default day is: the
/// timeline `--look-dev` runs on instead of the default day, so every other scene keeps its look.
const DAY: &str = include_str!("../../../content/lookdev/lookdev_day.ron");

/// The look-dev day's timeline: time of day's base timeline while the look-dev stage runs.
pub(crate) fn day() -> TodTimeline {
    TodTimeline::from_ron(DAY).expect("the look-dev day is checked by this module's tests")
}

/// A look-dev camera.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Shot {
    /// Across the square from its open side: the fountain, the market, the rows of houses, the
    /// gate and the tower over the roofs, the character small in the middle ground.
    Wide,
    /// The character in three-quarter view from her right at chest height on a long lens, the
    /// market behind her; at golden hour the sun comes from behind her left shoulder.
    Hero,
    /// Low and in front of the character, looking up past her at the rooftops and the sky.
    Sky,
}

impl Shot {
    pub const ALL: [Self; 3] = [Self::Wide, Self::Hero, Self::Sky];

    pub fn name(self) -> &'static str {
        match self {
            Self::Wide => "wide",
            Self::Hero => "hero",
            Self::Sky => "sky",
        }
    }

    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|shot| shot.name() == name)
    }

    /// The shot's camera: its target, yaw and pitch (radians; yaw orbits right from in front of
    /// the character, negative pitch looks up), distance in meters and vertical field of view.
    pub fn camera(self) -> OrbitCamera {
        let (target, yaw, pitch, distance, fov): (Vec3, f32, f32, f32, f32) = match self {
            Self::Wide => (Vec3::new(-1.0, 2.8, -9.0), 14.0, 1.5, 20.0, 50.0),
            Self::Hero => (Vec3::new(0.0, 1.2, 0.0), -38.0, 6.0, 3.0, 32.0),
            Self::Sky => (Vec3::new(0.0, 1.45, 0.0), -28.0, -18.0, 3.2, 58.0),
        };
        OrbitCamera::shot(target, yaw.to_radians(), pitch.to_radians(), distance, fov.to_radians())
    }
}

/// The lanterns' glass, where their lights are: each lantern's place in the town (x, z) at the
/// glass's height. The same places as the `@lantern` placements in the town's assembly.
pub(crate) const LAMPS: [[f32; 2]; 7] =
    [[-8.0, -2.0], [-1.0, -13.0], [8.0, -2.5], [-14.0, -10.0], [14.5, -14.0], [0.6, -28.0], [5.8, -38.0]];
/// The glass's height: 1.39 of the lantern's 1.556 at its scale of 1.6.
const LAMP_HEIGHT: f32 = 2.22;
/// A lamp's light at full night: a warm candle color, its reach and intensity.
const LAMP_COLOR: Vec3 = Vec3::new(1.0, 0.55, 0.24);
const LAMP_RANGE: f32 = 6.0;
pub(crate) const LAMP_INTENSITY: f32 = 0.9;

/// The lamps' lights at full intensity, by night: scale them by time of day's `day_night`.
pub(crate) fn lamps() -> impl Iterator<Item = LocalLight> {
    LAMPS.iter().map(|&[x, z]| LocalLight {
        position: Vec3::new(x, LAMP_HEIGHT, z),
        color: LAMP_COLOR,
        intensity: LAMP_INTENSITY,
        range: LAMP_RANGE,
        kind: LightKind::Point,
    })
}

/// The town's fog cards: mist sheets over the fields and hills beyond the town, and haze down
/// the street out of the square and among the rooftops behind the back row.
pub(crate) const CARDS: [CardPlace; 7] = [
    CardPlace { mesh: MIST, center: [10.0, 4.0, -85.0], size: [90.0, 8.0], yaw: 0.05 },
    CardPlace { mesh: MIST, center: [-40.0, 6.0, -120.0], size: [110.0, 12.0], yaw: 0.2 },
    CardPlace { mesh: MIST, center: [50.0, 10.0, -170.0], size: [160.0, 18.0], yaw: -0.15 },
    CardPlace { mesh: MIST, center: [-20.0, 16.0, -240.0], size: [260.0, 28.0], yaw: 0.0 },
    CardPlace { mesh: HAZE, center: [3.2, 2.2, -44.0], size: [10.0, 4.0], yaw: 0.0 },
    CardPlace { mesh: HAZE, center: [-4.0, 9.0, -36.0], size: [22.0, 6.0], yaw: 0.0 },
    CardPlace { mesh: HAZE, center: [12.0, 8.0, -40.0], size: [20.0, 5.0], yaw: 0.0 },
];

/// The held pose's turns, in the model's rest space (VRM: facing +Z, the character's left
/// toward +X): arms down at her sides and a little forward, elbows softly bent, the head turned a
/// little toward the hero shot, on her right.
fn turns() -> [(HumanBone, Quat); 7] {
    let degrees = f32::to_radians;
    [
        (
            HumanBone::LeftUpperArm,
            Quat::from_rotation_z(degrees(-76.0)) * Quat::from_rotation_y(degrees(-4.0)),
        ),
        (
            HumanBone::RightUpperArm,
            Quat::from_rotation_z(degrees(76.0)) * Quat::from_rotation_y(degrees(4.0)),
        ),
        (HumanBone::LeftLowerArm, Quat::from_rotation_y(degrees(-10.0))),
        (HumanBone::RightLowerArm, Quat::from_rotation_y(degrees(10.0))),
        (HumanBone::LeftHand, Quat::from_rotation_x(degrees(10.0))),
        (HumanBone::RightHand, Quat::from_rotation_x(degrees(10.0))),
        (HumanBone::Head, Quat::from_rotation_y(degrees(-10.0)) * Quat::from_rotation_x(degrees(4.0))),
    ]
}

/// The expression the character holds, and its weight: a soft, relaxed face.
const EXPRESSION: (&str, f32) = ("relaxed", 0.35);

/// The radius of the character's body capsule (M1 R4), from her feet to her head.
const BODY_RADIUS: f32 = 0.16;
/// From the head joint (the top of the neck) to the middle of the head, in meters.
const HEAD_CENTER: Vec3 = Vec3::new(0.0, 0.09, 0.01);

/// Loads the character's rig and holds the character in its pose ([`held_pose`]), from the copy
/// of its mesh `content` kept ([`Content::keep_character_mesh`]).
pub(crate) fn hold(
    content: &mut Content,
    renderer: &mut Renderer,
    gpu: &GpuContext,
) -> Result<(RenderPose, Figure), AppInitError> {
    content.load_rig(RIG, renderer, gpu)?;
    let mesh =
        content.character_mesh().ok_or_else(|| AppInitError::msg("the look-dev character isn't loaded"))?;
    let rig = content.rig().ok_or_else(|| AppInitError::msg("the look-dev character's rig isn't loaded"))?;
    held_pose(mesh, rig).map_err(AppInitError::msg)
}

/// The character's held pose and the figure it makes: the skinning palette and morph weights
/// for its mesh, and its head, feet, body and face basis in its own space. Fails if the mesh has
/// no skin or the rig no head.
pub(crate) fn held_pose(mesh: &Mesh, rig: &ArchivedCharacterRig) -> Result<(RenderPose, Figure), String> {
    check_counts(rig, mesh)?;
    let skin = mesh.skin.as_ref().ok_or("the look-dev character has no skin")?;
    let skeleton = Skeleton::new(skin);
    let joints = RigJoints::new(rig);
    let posed = pose::pose(&skeleton, &joints, &turns());
    let head_joint = joints.joint(HumanBone::Head).ok_or("the look-dev character's rig has no head")?;
    let head = posed.model_from_joint[head_joint];
    let (_, head_rotation, head_position) = head.to_scale_rotation_translation();
    // The head's basis turned by the held pose, from its rest orientation.
    let rest_head = pose::rest_rotation(&skeleton, head_joint);
    let face = (head_rotation * rest_head.inverse()).normalize();
    let feet = Vec3::new(head_position.x, mesh.streams.bounds.min[1].max(0.0), head_position.z);
    let figure = Figure { head: head_position + face * HEAD_CENTER, feet, body_radius: BODY_RADIUS, face };
    let mut morph_weights = vec![0.0; mesh.morph_targets.len()];
    let (name, weight) = EXPRESSION;
    if let Some(expression) = rig.expressions.iter().find(|expression| expression.name.as_str() == name) {
        for target in expression.morph_targets.iter() {
            if let Some(slot) = morph_weights.get_mut(target.target.to_native() as usize) {
                *slot += weight * target.weight.to_native();
            }
        }
    }
    Ok((RenderPose { palette: posed.palette, morph_weights }, figure))
}

/// Whether `rig` was cooked with `mesh`: the same joints and morph targets, which the pose
/// indexes one through the other.
fn check_counts(rig: &ArchivedCharacterRig, mesh: &Mesh) -> Result<(), String> {
    let joints = mesh.skin.as_ref().map_or(0, |skin| skin.joints.len());
    let counts = (rig.joint_count.to_native() as usize, rig.morph_target_count.to_native() as usize);
    if counts != (joints, mesh.morph_targets.len()) {
        return Err(format!(
            "the look-dev character's rig has {counts:?} joints and morph targets, its mesh {:?}",
            (joints, mesh.morph_targets.len())
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The look-dev day is a complete base timeline with keys on the golden scenes' hours, so their
    /// frames don't move when the keys around them are retuned.
    #[test]
    fn the_look_dev_day_is_complete_and_keyed_on_the_golden_hours() {
        let day = day();
        assert!(day.is_complete());
        for hours in [13.0, 18.0] {
            assert!(day.keys().iter().any(|key| key.time == hours), "a key at {hours}");
        }
        assert_ne!(day, TodTimeline::default_day(), "the stage's own");
    }

    #[test]
    fn the_look_dev_day_never_lights_from_below() {
        let day = day();
        for minute in 0..24 * 60 {
            let key = day.evaluate(minute as f32 / 60.0).key_light();
            assert!(key.intensity == 0.0 || key.direction.y < 0.0, "{minute} min: {key:?} travels upward");
        }
    }

    /// CLAUDE.md's night character light contract, held as the default day's test holds it, at
    /// every quarter hour: the character key, as the screen shows it (luminance, exposed), is the
    /// world's key light by day and never dimmer; at sunset and from the blue hour after it
    /// through the night to the blue hour before dawn it is 1.2 to 1.6 times the key light, so
    /// characters read, and at most 70% of their noon brightness, so they read as night.
    #[test]
    fn the_look_dev_days_characters_stay_a_little_brighter_than_the_world_at_night() {
        use sapphire_math::color::luminance;
        let day = day();
        let shown = |look: &sapphire_world::tod::TodLook, color: Vec3, intensity: f32| {
            luminance(color) * intensity * look.exposure.exp2()
        };
        let noon = day.evaluate(13.0);
        let noon_character = shown(&noon, noon.character_light.color, noon.character_light.intensity);
        for quarter in 0..24 * 4 {
            let hours = quarter as f32 / 4.0;
            let look = day.evaluate(hours);
            let key = look.key_light();
            let character = shown(&look, look.character_light.color, look.character_light.intensity);
            let lift = character / shown(&look, key.color, key.intensity);
            assert!(lift >= 0.999, "{hours}: characters are never lit below the world ({lift})");
            if (9.0..=16.0).contains(&hours) {
                assert!((lift - 1.0).abs() < 1e-4, "{hours}: by day the same light ({lift})");
            }
            if hours >= 19.75 || hours <= 4.75 || hours == 19.0 {
                assert!((1.2..=1.6).contains(&lift), "{hours}: a little brighter than the world ({lift})");
                let share = character / noon_character;
                assert!(share <= 0.7, "{hours}: at night, at most 70% of noon's brightness ({share})");
            }
        }
    }

    /// Characters take part of the fog at every hour, so they read as figures where the world melts
    /// into the haze, and the square stays clear: no distance layer of the haze starts within 25 m.
    #[test]
    fn the_look_dev_day_keeps_the_square_clear_of_haze() {
        let day = day();
        for quarter in 0..24 * 4 {
            let hours = quarter as f32 / 4.0;
            let fog = day.evaluate(hours).fog;
            assert!((0.35..=0.5).contains(&fog.characters), "{hours}: characters take part of it");
            assert!(
                fog.near.start >= 25.0 && fog.far.start >= 45.0,
                "{hours}: the haze starts out of the square"
            );
        }
    }

    #[test]
    fn plantings_stand_on_the_ground_or_in_planters() {
        for (plant, at, scale, _) in PLANTINGS {
            assert!([0.0, PLANTER, POT].contains(&at[1]), "{plant:?} at {at:?}");
            assert!(scale.iter().all(|&s| s > 0.0), "{plant:?}");
        }
        assert_eq!(plantings().count(), PLANTINGS.len());
        for plant in Plant::ALL {
            assert!(PLANTINGS.iter().any(|planting| planting.0 == plant), "{plant:?} is planted");
        }
    }

    #[test]
    fn shots_parse_by_name() {
        for shot in Shot::ALL {
            assert_eq!(Shot::from_name(shot.name()), Some(shot));
        }
        assert_eq!(Shot::from_name("close"), None);
    }

    #[test]
    fn shots_see_the_character_from_the_front_and_the_sky_shot_looks_up() {
        for shot in Shot::ALL {
            let camera = shot.camera();
            let eye = camera.eye();
            assert!(eye.z > 0.0, "{shot:?} in front of her: {eye}");
            assert!(eye.x.atan2(eye.z).abs() < std::f32::consts::FRAC_PI_2, "{shot:?} sees her face");
        }
        assert!(Shot::Sky.camera().eye().y < 1.45, "below her chest, looking up");
        assert!(Shot::Wide.camera().eye().distance(Vec3::ZERO) > 10.0, "from across the square");
    }

    #[test]
    fn lamps_light_the_town_s_lanterns() {
        let town = include_str!("../../../content/lookdev/town/town.assembly.ron");
        let lanterns: Vec<[f32; 2]> = town
            .lines()
            .filter(|line| line.contains(r#"(piece: "@lantern""#))
            .map(|line| {
                let at = line.split("at: (").nth(1).expect("placed").split(')').next().expect("closed");
                let numbers: Vec<f32> = at.split(',').map(|n| n.trim().parse().expect("a number")).collect();
                [numbers[0], numbers[2]]
            })
            .collect();
        assert_eq!(lanterns, LAMPS);
        assert_eq!(lamps().count(), LAMPS.len());
    }
}
