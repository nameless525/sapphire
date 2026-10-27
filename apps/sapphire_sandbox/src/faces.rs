//! The face look-dev rig (`--faces`, M1 C3 and C5): rows of the face test head, each lit by a key
//! light of its own from an angle in head space, the way an artist checks a face map and hair: a
//! yaw sweep across both sides, top light and expressions; the bangs' shadow; the hair's
//! highlights; eyes and brows through the bangs; and the character light rig's stylized GI alone
//! (M1 C2).
//!
//! - **The head** is content: the pipeline's face sample (`samples::face`), loaded by ID with
//!   its face, hair, bangs and brow materials and their maps through the asset server
//!   ([`Content::face_head`]). Its face region takes the GI's sphere and skin tint, its hair and
//!   bangs the sphere (`RegionGi`): the only sample content that takes either.
//! - **Each head is a character**, lit by the character light rig (M1 C2) like the turntable's:
//!   [`Scene::frame_packet`] lists every head with the default preset's lighting resolved for
//!   it, its key light turned to the head's [`FaceRig`] angles in head space, and its face, the
//!   head's pose and the rig's expression ([`character`]). The GI layouts switch the key light
//!   off instead ([`FaceLayout::light`]).
//! - **Layouts** (`--face-layout`, [`FaceLayout`]), each rows left to right as the camera sees
//!   them ([`FaceLayout::rows`]):
//!   - `lights` (the default, M1 C3): the heads face the camera, so the face map's shapes read
//!     as a light moving around still heads and no nose line shows (it appears only as a head
//!     turns from the camera). The sweep from light behind the heads' right (lighting the image's
//!     left) round to behind their left, then a top light, one from above and to the side, a
//!     blush, and a gloom with a blush.
//!   - `hair` (M1 C5): heads facing the camera under keys from their right round the front and
//!     above to their left, so the bangs' shadow on the face moves away from the light.
//!   - `highlights` (M1 C5): heads nodding from up to down under a key from the front and above:
//!     the cap's angel ring keeps its height on screen while the bangs' painted band follows the
//!     light and the view.
//!   - `turn` (M1 C5): heads turning from the camera, front to three-quarter to past the eyes'
//!     fade, then nodding up and down past it: eyes and brows show through the bangs, fading as
//!     the camera leaves the face.
//!   - `gi` (M1 C2): heads lit by the default preset's stylized GI alone, the key light off and
//!     the GI raised [`GI_EXPOSURE`] times, so the GI's own shading shows in the mid-tones: the
//!     ambient hemisphere's sky above and ground below, seen through normals rounded toward the
//!     head's sphere (the jaw, drawn out below it, takes the sphere's and darkens) and bent
//!     toward the camera, the skin tint on the face, and the hair mask's and gloom's occlusion.
//!     The first row faces the camera, turns 60° and nods down and up; the second turns either
//!     way nodding up at its ends, and between them has two heads in gloom, one facing the camera
//!     and one turned and nodding down. The preset's value ceiling stays just out of daylight's
//!     reach there, and its saturation ceiling well out of it.
//!   - `gi-clamped` (M1 C2): the same heads, their GI clamped hard ([`CLAMPED_GI`]): saturation
//!     held to 0.2 and value to 0.3, which greys their color and flattens their sky sides before
//!     the skin tint warms the face again.
//!   - `backlit` (M1 C8): heads facing the camera under keys from behind them, round from their
//!     right to their left and at several heights: the light through thin hair
//!     (`--hair-transmission`) and the rim against a back light.
//!   - The eyes' layouts (M1 C6), framed close on the eyes' line under a key from the front, a
//!     little above and to the side, with the heads closer together: `eyes`, one head facing
//!     the camera; `eyes-turned`, one turned to three-quarter; `gaze`, a row looking toward the
//!     image's left, ahead and to its right, then one up and one down; `blink`, lids half
//!     closed and closed.
//! - **Lids and gaze** (M1 C6): each head is posed by the head's morph targets
//!   (`samples::face::MORPH_TARGETS`) from its rig's blink and look ([`FaceRig::morph_weights`]),
//!   a [`RenderPose`] of its own.
//!
//! The heads never move, so their instances and poses are extracted once; their characters are
//! listed every frame, as every character's are.
//!
//! [`Content::face_head`]: crate::content::Content::face_head
//! [`Scene::frame_packet`]: crate::scene::Scene::frame_packet

use sapphire_assets::AssetId;
use sapphire_assets::material::{AnimeCharacterParams, HairTransmission, Material};
use sapphire_ecs::prelude::*;
use sapphire_math::{Mat3, Mat4, Quat, Vec3};
use sapphire_render::{CharacterFace, CharacterLighting, FaceExpression, HeadBasis};

use crate::extract::RenderPose;
use crate::orbit::OrbitCamera;

/// The face test head, by its sidecar's ID (`content/samples/face/face_head.glb.meta`, the
/// pipeline's `samples::face::HEAD_ID`).
pub(crate) const HEAD: AssetId = AssetId::from_text("cff436cc-9604-4a9f-8d3a-3bc8b5b6637e");

/// One head of the rig: the key light's angles in head space, in degrees (yaw toward the head's
/// right, negative to its left, and elevation above its horizontal plane), its expression, and
/// its pose: where the camera lies in head space, in degrees, `turn` around the head toward its
/// right (the head turned toward its left; negative the other way) and `nod` above its level
/// (the head nodding down; negative up). The eyes' fade through the bangs measures the same two
/// angles. Then its eyes (M1 C6): how far closed its lids are, 0 to 1, and where it looks, each
/// axis -1 to 1: toward the head's left (+X; the image's right, facing the camera) and up.
#[derive(Component, Clone, Copy, Debug, PartialEq)]
pub(crate) struct FaceRig {
    pub yaw: f32,
    pub elevation: f32,
    pub expression: FaceExpression,
    pub turn: f32,
    pub nod: f32,
    pub blink: f32,
    pub look: (f32, f32),
}

impl FaceRig {
    /// The unit vector toward the key light for a head facing `forward` with `up` (unit and
    /// perpendicular), whose right is `forward × up`.
    pub fn toward_key(&self, forward: Vec3, up: Vec3) -> Vec3 {
        let (yaw, elevation) = (self.yaw.to_radians(), self.elevation.to_radians());
        let right = forward.cross(up);
        right * (yaw.sin() * elevation.cos()) + up * elevation.sin() + forward * (yaw.cos() * elevation.cos())
    }

    /// The head's rotation from facing the camera, in its own space, that puts the camera at
    /// the pose's angles.
    fn pose(&self) -> Quat {
        Quat::from_rotation_x(self.nod.to_radians()) * Quat::from_rotation_y(self.turn.to_radians())
    }

    /// The head's morph weights for its blink and look (M1 C6), by the head's targets
    /// (`samples::face::MORPH_TARGETS`): the lids half closed, then the rest of the way, which
    /// apply one after the other; then the gaze toward the head's left, right, up and down.
    pub fn morph_weights(&self) -> [f32; MORPH_TARGETS] {
        let (blink, (x, y)) = (self.blink.clamp(0.0, 1.0), self.look);
        [
            (2.0 * blink).min(1.0),
            (2.0 * blink - 1.0).max(0.0),
            x.clamp(0.0, 1.0),
            (-x).clamp(0.0, 1.0),
            y.clamp(0.0, 1.0),
            (-y).clamp(0.0, 1.0),
        ]
    }

    /// The head's pose for the renderer: no joints, its morph weights.
    pub fn render_pose(&self) -> RenderPose {
        RenderPose { palette: Vec::new(), morph_weights: self.morph_weights().to_vec() }
    }
}

/// The head's morph targets (`samples::face::MORPH_TARGETS`, M1 C6).
const MORPH_TARGETS: usize = 6;

/// A head facing the camera, lit from `yaw` and `elevation`, with an expression.
const fn rig(yaw: f32, elevation: f32, blush: f32, gloom: f32) -> FaceRig {
    let expression = FaceExpression { blush, gloom };
    FaceRig { yaw, elevation, expression, turn: 0.0, nod: 0.0, blink: 0.0, look: (0.0, 0.0) }
}

/// A head turned and nodding from the camera, lit from `yaw` and `elevation` in its own space.
const fn posed(turn: f32, nod: f32, yaw: f32, elevation: f32) -> FaceRig {
    let expression = FaceExpression { blush: 0.0, gloom: 0.0 };
    FaceRig { yaw, elevation, expression, turn, nod, blink: 0.0, look: (0.0, 0.0) }
}

/// The eyes' key light (M1 C6), in head space: from the front, a little above and to the head's
/// right, as a portrait's.
const EYE_KEY: (f32, f32) = (30.0, 25.0);

/// A head for the eyes' layouts (M1 C6): turned `turn` degrees, under [`EYE_KEY`], its lids
/// `blink` closed and looking along `look`.
const fn eyes(turn: f32, blink: f32, look: (f32, f32)) -> FaceRig {
    FaceRig { blink, look, ..posed(turn, 0.0, EYE_KEY.0, EYE_KEY.1) }
}

/// A head of the GI layouts (M1 C2), turned and nodding from the camera, with a gloom: their key
/// light is off, so its angles are the front's.
const fn gi_head(turn: f32, nod: f32, gloom: f32) -> FaceRig {
    FaceRig { expression: FaceExpression { blush: 0.0, gloom }, ..posed(turn, nod, 0.0, 0.0) }
}

/// How a layout lights its heads.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum HeadLight {
    /// The default preset's lighting, its key light turned to each head's angles.
    Key,
    /// The default preset's stylized GI alone (M1 C2): the key light off and the GI raised
    /// [`GI_EXPOSURE`] times, under `ceilings` instead of the preset's where given.
    Gi { ceilings: Option<GiCeilings> },
}

/// The ceilings of the stylized GI's HSV clamp (M1 C2): saturation, 0 to 1, and value (the
/// brightest channel), linear light.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct GiCeilings {
    pub saturation: f32,
    pub value: f32,
}

/// How many times the GI layouts raise the default preset's GI. With the key light off, the GI
/// then reads in the mid-tones rather than near black: daylight's ambient sky (10:00 to 17:30)
/// takes it to a value of about 0.45 to 0.48, just under the preset's ceiling of 0.5.
pub(crate) const GI_EXPOSURE: f32 = 4.0;

/// `gi-clamped`'s ceilings, inside the raised GI's range so the clamp shows on every head:
/// daylight's ambient is about 0.2 to 0.4 saturated, and at [`GI_EXPOSURE`] its value runs from
/// about 0.1 facing the ground to 0.48 facing the sky, so the saturation ceiling greys all of it
/// and the value ceiling flattens the sky side.
pub(crate) const CLAMPED_GI: GiCeilings = GiCeilings { saturation: 0.2, value: 0.3 };

/// Which heads the rig shows (`--face-layout`): see the module docs.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum FaceLayout {
    #[default]
    Lights,
    Hair,
    Highlights,
    Turn,
    Gi,
    GiClamped,
    Eyes,
    EyesTurned,
    Gaze,
    Blink,
    Backlit,
}

impl FaceLayout {
    #[cfg(test)]
    pub const ALL: [Self; 11] = [
        Self::Lights,
        Self::Hair,
        Self::Highlights,
        Self::Turn,
        Self::Gi,
        Self::GiClamped,
        Self::Eyes,
        Self::EyesTurned,
        Self::Gaze,
        Self::Blink,
        Self::Backlit,
    ];

    /// The names `--face-layout` takes, one per layout, in the order they are declared.
    pub const NAMES: [&str; 11] = [
        "lights",
        "hair",
        "highlights",
        "turn",
        "gi",
        "gi-clamped",
        "eyes",
        "eyes-turned",
        "gaze",
        "blink",
        "backlit",
    ];

    /// The layout `--face-layout` names.
    pub fn from_name(name: &str) -> Option<Self> {
        match name {
            "lights" => Some(Self::Lights),
            "hair" => Some(Self::Hair),
            "highlights" => Some(Self::Highlights),
            "turn" => Some(Self::Turn),
            "gi" => Some(Self::Gi),
            "gi-clamped" => Some(Self::GiClamped),
            "eyes" => Some(Self::Eyes),
            "eyes-turned" => Some(Self::EyesTurned),
            "gaze" => Some(Self::Gaze),
            "blink" => Some(Self::Blink),
            "backlit" => Some(Self::Backlit),
            _ => None,
        }
    }

    /// The layout's rows, top to bottom, each left to right as the camera sees it.
    pub fn rows(self) -> &'static [&'static [FaceRig]] {
        match self {
            Self::Lights => &LIGHTS,
            Self::Hair => &HAIR,
            Self::Highlights => &HIGHLIGHTS,
            Self::Turn => &TURN,
            Self::Gi | Self::GiClamped => &GI,
            Self::Eyes => &EYES,
            Self::EyesTurned => &EYES_TURNED,
            Self::Gaze => &GAZE,
            Self::Blink => &BLINK,
            Self::Backlit => &BACKLIT,
        }
    }

    /// How the layout lights its heads: the GI layouts by their GI alone (M1 C2).
    pub fn light(self) -> HeadLight {
        match self {
            Self::Lights | Self::Hair | Self::Highlights | Self::Turn | Self::Backlit => HeadLight::Key,
            Self::Eyes | Self::EyesTurned | Self::Gaze | Self::Blink => HeadLight::Key,
            Self::Gi => HeadLight::Gi { ceilings: None },
            Self::GiClamped => HeadLight::Gi { ceilings: Some(CLAMPED_GI) },
        }
    }

    /// Whether the camera frames the eyes' line close (M1 C6) rather than whole heads.
    fn frames_eyes(self) -> bool {
        matches!(self, Self::Eyes | Self::EyesTurned | Self::Gaze | Self::Blink)
    }

    /// Meters between neighbors in a row, and between rows: closer for the eyes' layouts.
    fn spacing(self) -> (f32, f32) {
        if self.frames_eyes() { (0.3, 0.32) } else { (SPACING, ROW_SPACING) }
    }

    /// Heads in the layout.
    pub fn heads(self) -> usize {
        self.rows().iter().map(|row| row.len()).sum()
    }
}

const LIGHTS: [&[FaceRig]; 2] = [
    &[
        rig(135.0, 0.0, 0.0, 0.0),
        rig(90.0, 0.0, 0.0, 0.0),
        rig(45.0, 0.0, 0.0, 0.0),
        rig(-45.0, 0.0, 0.0, 0.0),
        rig(-90.0, 0.0, 0.0, 0.0),
        rig(-135.0, 0.0, 0.0, 0.0),
    ],
    &[
        rig(0.0, 65.0, 0.0, 0.0),
        rig(60.0, 40.0, 0.0, 0.0),
        rig(45.0, 10.0, 1.0, 0.0),
        rig(45.0, 10.0, 0.7, 1.0),
    ],
];

const HAIR: [&[FaceRig]; 1] = [&[
    rig(65.0, 20.0, 0.0, 0.0),
    rig(30.0, 40.0, 0.0, 0.0),
    rig(0.0, 60.0, 0.0, 0.0),
    rig(-30.0, 40.0, 0.0, 0.0),
    rig(-65.0, 20.0, 0.0, 0.0),
]];

const HIGHLIGHTS: [&[FaceRig]; 1] = [&[
    posed(0.0, -30.0, 20.0, 35.0),
    posed(0.0, -12.0, 20.0, 35.0),
    posed(0.0, 6.0, 20.0, 35.0),
    posed(0.0, 24.0, 20.0, 35.0),
]];

const TURN: [&[FaceRig]; 2] = [
    &[
        posed(0.0, 0.0, 30.0, 30.0),
        posed(25.0, 0.0, 30.0, 30.0),
        posed(45.0, 0.0, 30.0, 30.0),
        posed(-60.0, 0.0, 30.0, 30.0),
        posed(80.0, 0.0, 30.0, 30.0),
    ],
    &[
        posed(0.0, -25.0, 30.0, 30.0),
        posed(0.0, 20.0, 30.0, 30.0),
        posed(-35.0, 15.0, 30.0, 30.0),
        posed(0.0, 55.0, 30.0, 30.0),
    ],
];

// The GI layouts' heads (M1 C2): the poses that show the hemisphere's sky and ground on the
// face, the jaw and the hair from every side, then two in gloom.
const GI: [&[FaceRig]; 2] = [
    &[gi_head(0.0, 0.0, 0.0), gi_head(60.0, 0.0, 0.0), gi_head(0.0, 30.0, 0.0), gi_head(0.0, -35.0, 0.0)],
    &[
        gi_head(-40.0, -20.0, 0.0),
        gi_head(0.0, 0.0, 1.0),
        gi_head(35.0, 10.0, 1.0),
        gi_head(40.0, -20.0, 0.0),
    ],
];
// The eyes' layouts (M1 C6).
const EYES: [&[FaceRig]; 1] = [&[eyes(0.0, 0.0, (0.0, 0.0))]];
const EYES_TURNED: [&[FaceRig]; 1] = [&[eyes(35.0, 0.0, (0.0, 0.0))]];
const GAZE: [&[FaceRig]; 2] = [
    &[eyes(0.0, 0.0, (-1.0, 0.0)), eyes(0.0, 0.0, (0.0, 0.0)), eyes(0.0, 0.0, (1.0, 0.0))],
    &[eyes(0.0, 0.0, (0.0, 1.0)), eyes(0.0, 0.0, (0.0, -1.0))],
];
const BLINK: [&[FaceRig]; 1] = [&[eyes(0.0, 0.5, (0.0, 0.0)), eyes(0.0, 1.0, (0.0, 0.0))]];
// Back light (M1 C8): keys from behind the heads, round from their right to their left and at
// several heights, for the light through thin hair.
const BACKLIT: [&[FaceRig]; 1] = [&[
    rig(140.0, 20.0, 0.0, 0.0),
    rig(160.0, 40.0, 0.0, 0.0),
    rig(175.0, 10.0, 0.0, 0.0),
    rig(-160.0, 40.0, 0.0, 0.0),
    rig(-140.0, 20.0, 0.0, 0.0),
]];

/// Meters between neighbors in a row, and between rows.
const SPACING: f32 = 0.34;
const ROW_SPACING: f32 = 0.38;
/// Half the head's width and height, in meters, for framing.
const HEAD_HALF_EXTENTS: (f32, f32) = (0.14, 0.16);
/// The eyes' layouts' framing (M1 C6): half the width and height around each head's eyes, in
/// meters, and where the eyes lie in the head's model: their line's height and how far in front
/// of its center.
const EYE_HALF_EXTENTS: (f32, f32) = (0.13, 0.073);
const EYE_LINE: f32 = -0.012;
const EYE_DEPTH: f32 = 0.11;

/// Half the width and height of `layout`'s rows, in meters: of whole heads, or of the eyes'
/// line.
fn half_extents(layout: FaceLayout) -> (f32, f32) {
    let (spacing, row_spacing) = layout.spacing();
    let widest = layout.rows().iter().map(|row| row.len()).max().unwrap_or(1) as f32;
    let head = if layout.frames_eyes() { EYE_HALF_EXTENTS } else { HEAD_HALF_EXTENTS };
    (0.5 * (widest - 1.0) * spacing + head.0, 0.5 * (layout.rows().len() as f32 - 1.0) * row_spacing + head.1)
}

/// Half the height of `layout`'s rows, in meters.
fn half_height(layout: FaceLayout) -> f32 {
    half_extents(layout).1
}

/// The rig's center beside a character whose object-space bounds are `bounds`, and half its
/// height in `layout`: what the stage (`--stage`) shades it to.
pub(crate) fn rig_center(bounds: [[f32; 3]; 2], layout: FaceLayout) -> (Vec3, f32) {
    (center(bounds), half_height(layout))
}

/// Where the rig stands beside a character whose object-space bounds are `bounds`: well to its
/// left, at head height, so the camera framing the rig leaves the character out.
fn center(bounds: [[f32; 3]; 2]) -> Vec3 {
    let [min, max] = bounds.map(Vec3::from);
    let height = (max.y - min.y).max(0.1);
    Vec3::new(min.x - 1.6 * height, min.y + 0.55 * height, (min.z + max.z) * 0.5)
}

/// The camera framing the rig in `layout`: whole heads, or for the eyes' layouts (M1 C6) their
/// eyes' line, as near as the heads' eyes lie.
pub(crate) fn camera(bounds: [[f32; 3]; 2], layout: FaceLayout) -> OrbitCamera {
    let extents = half_extents(layout);
    let camera = OrbitCamera::fitting(center(bounds), extents);
    if !layout.frames_eyes() {
        return camera;
    }
    let view = camera.facing_rotation();
    let eyes = center(bounds) + view * Vec3::new(0.0, EYE_LINE, EYE_DEPTH);
    OrbitCamera::fitting(eyes, extents)
}

/// Every head's rig and placement in `layout`, in its rows' order: rows across the camera's view,
/// each head facing the camera, so the rig's angles (which take the camera as straight ahead)
/// hold for heads at the edges of the view too, and turned about that line so its eyes stay
/// level in the image; then turned and nodding by its pose.
pub(crate) fn heads(bounds: [[f32; 3]; 2], layout: FaceLayout, camera: &OrbitCamera) -> Vec<(FaceRig, Mat4)> {
    let view = camera.facing_rotation();
    let (right, up) = (view * Vec3::X, view * Vec3::Y);
    let rows = layout.rows();
    let (spacing, row_spacing) = layout.spacing();
    let mut heads = Vec::with_capacity(layout.heads());
    for (row, rigs) in rows.iter().enumerate() {
        let y = (0.5 * (rows.len() as f32 - 1.0) - row as f32) * row_spacing;
        for (column, &rig) in rigs.iter().enumerate() {
            let x = (column as f32 - 0.5 * (rigs.len() as f32 - 1.0)) * spacing;
            let at = center(bounds) + right * x + up * y;
            let forward = (camera.eye() - at).normalize();
            // A line through the head projects level when it lies in the plane of the line of
            // sight and the camera's right: the head's +X (its left) is that plane's direction
            // across the line of sight.
            let across = (right - forward * right.dot(forward)).normalize();
            let facing = Quat::from_mat3(&Mat3::from_cols(across, forward.cross(across), forward));
            heads.push((rig, Mat4::from_rotation_translation(facing * rig.pose(), at)));
        }
    }
    heads
}

/// A rig head's character data this frame in `layout`, from `lighting` resolved for it: the key
/// light turned to the rig's angles in the head's space, or in the GI layouts switched off and
/// the GI raised and clamped as [`FaceLayout::light`] says; and its face. The head is the mesh's,
/// placed by `transform` (its model faces +Z with +Y up, centered on its origin).
pub(crate) fn character(
    layout: FaceLayout,
    rig: &FaceRig,
    transform: Mat4,
    mut lighting: CharacterLighting,
) -> (CharacterLighting, CharacterFace) {
    let head =
        HeadBasis { forward: transform.transform_vector3(Vec3::Z), up: transform.transform_vector3(Vec3::Y) };
    let (forward, up) = head.orthonormalized().unwrap_or((Vec3::Z, Vec3::Y));
    lighting.key.direction = -rig.toward_key(forward, up);
    if let HeadLight::Gi { ceilings } = layout.light() {
        lighting.key.intensity = 0.0;
        lighting.gi.intensity *= GI_EXPOSURE;
        if let Some(GiCeilings { saturation, value }) = ceilings {
            (lighting.gi.max_saturation, lighting.gi.max_value) = (saturation, value);
        }
    }
    (lighting, CharacterFace { head, expression: rig.expression })
}

/// What `--hair-shadow-softness` and `--hair-transmission` change in the anime character
/// materials the sandbox uploads (M1 C8): a face's bangs' shadow's soft edge and hair's light
/// through it, each edited into a copy of the cooked material as it loads.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct MaterialOverrides {
    /// Faces' bangs' shadow's soft edge, in meters, for faces that cast one.
    pub hair_shadow_softness: Option<f32>,
    /// Hair's transmission strength, at its sharpness (the default's, where it had none).
    pub hair_transmission: Option<f32>,
}

impl MaterialOverrides {
    /// Whether they change nothing.
    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }

    /// Applies them to `material`, where it is a face or hair they change. Returns whether it
    /// changed.
    pub fn apply(&self, material: &mut Material) -> bool {
        let mut changed = false;
        material.edit_anime_character(|params| match params.modes[2] {
            AnimeCharacterParams::SURFACE_FACE => {
                if let Some(softness) = self.hair_shadow_softness
                    && params.hair_shadow[0] > 0.0
                {
                    params.hair_shadow[2] = softness;
                    changed = true;
                }
            }
            AnimeCharacterParams::SURFACE_HAIR => {
                if let Some(strength) = self.hair_transmission {
                    let sharpness = params.surface_color[3];
                    params.surface[3] = strength;
                    params.surface_color[3] =
                        if sharpness > 0.0 { sharpness } else { HairTransmission::default().sharpness };
                    changed = true;
                }
            }
            _ => {}
        });
        changed
    }
}

#[cfg(test)]
mod tests {
    use sapphire_assets::material::{AnimeCharacterMaterial, CharacterSurface, FaceShading, HairShading};
    use sapphire_assets::texture::Swizzle;
    use sapphire_render::{CharacterGi, DirectionalLight};

    use super::*;

    #[test]
    fn overrides_soften_faces_and_light_hair_through_and_leave_the_rest() {
        let swizzles = [Swizzle::IDENTITY; AnimeCharacterMaterial::TEXTURE_SLOTS.len()];
        let cook = |surface| {
            let document = AnimeCharacterMaterial { surface, ..AnimeCharacterMaterial::default() };
            Material::from_document(&document, &swizzles).expect("valid")
        };
        let read = |material: &mut Material| {
            let mut params = None;
            material.edit_anime_character(|block| params = Some(*block));
            params.expect("an anime character material")
        };
        let overrides = MaterialOverrides { hair_shadow_softness: Some(0.02), hair_transmission: Some(0.8) };
        assert!(MaterialOverrides::default().is_empty() && !overrides.is_empty());
        let face = FaceShading::new(AssetId::from_text("79309b71-b35f-4ce6-9d53-b629493934e7"));
        let mut material = cook(CharacterSurface::Face(face.clone()));
        assert!(overrides.apply(&mut material));
        assert_eq!(read(&mut material).hair_shadow, [0.03, 1.0, 0.02, 0.0], "the default face, softened");
        // Hair transmits at the default sharpness, or its own.
        let mut hair = cook(CharacterSurface::Hair(HairShading::default()));
        assert!(overrides.apply(&mut hair));
        let params = read(&mut hair);
        assert_eq!((params.surface[3], params.surface_color[3]), (0.8, 4.0));
        let own = HairShading {
            transmission: HairTransmission { strength: 0.3, sharpness: 2.0 },
            ..HairShading::default()
        };
        let mut own = cook(CharacterSurface::Hair(own));
        assert!(overrides.apply(&mut own));
        let params = read(&mut own);
        assert_eq!((params.surface[3], params.surface_color[3]), (0.8, 2.0));
        // A face that casts no bangs' shadow, a body, and hair with no transmission asked stay as
        // they are.
        let mut shadowless = face;
        shadowless.hair_shadow.distance = 0.0;
        let mut shadowless = cook(CharacterSurface::Face(shadowless));
        let before = shadowless.clone();
        assert!(!overrides.apply(&mut shadowless));
        assert_eq!(shadowless, before);
        assert!(!overrides.apply(&mut cook(CharacterSurface::Body)));
        let softening = MaterialOverrides { hair_transmission: None, ..overrides };
        assert!(!softening.apply(&mut cook(CharacterSurface::Hair(HairShading::default()))));
    }

    const BOUNDS: [[f32; 3]; 2] = [[-0.5, 0.0, -0.5], [0.5, 2.0, 0.5]];

    /// `light` in a head's basis: right, up, forward.
    fn in_head(face: CharacterFace, light: Vec3) -> Vec3 {
        let (forward, up) = face.head.orthonormalized().expect("a basis");
        Vec3::new(light.dot(forward.cross(up)), light.dot(up), light.dot(forward))
    }

    #[test]
    fn each_head_is_lit_from_its_angles_in_its_own_space() {
        let sun = DirectionalLight { direction: Vec3::new(0.55, -0.7, -0.45), ..DirectionalLight::default() };
        let resolved = CharacterLighting::unstyled(sun);
        for layout in FaceLayout::ALL {
            let camera = camera(BOUNDS, layout);
            for (rig, transform) in heads(BOUNDS, layout, &camera) {
                let (lighting, face) = character(layout, &rig, transform, resolved);
                let (yaw, elevation) = (rig.yaw.to_radians(), rig.elevation.to_radians());
                let expected =
                    Vec3::new(yaw.sin() * elevation.cos(), elevation.sin(), yaw.cos() * elevation.cos());
                let seen = in_head(face, -lighting.key.direction);
                assert!(seen.abs_diff_eq(expected, 1e-5), "{rig:?}: {seen}");
                assert_eq!(face.expression, rig.expression);
                assert_eq!(lighting.key.color, sun.color, "only the direction is the rig's");
            }
        }
    }

    #[test]
    fn the_gi_layouts_light_heads_by_their_gi_alone() {
        let sun = DirectionalLight { direction: Vec3::new(0.55, -0.7, -0.45), ..DirectionalLight::default() };
        // As the default preset resolves.
        let gi = CharacterGi {
            head: Some(Vec3::new(-3.0, 1.1, 0.0)),
            intensity: 1.0,
            camera_bias: 0.3,
            max_saturation: 0.6,
            max_value: 0.5,
            skin_tint: Vec3::new(1.12, 0.96, 0.86),
        };
        let resolved = CharacterLighting { gi, ..CharacterLighting::unstyled(sun) };
        for layout in FaceLayout::ALL {
            let camera = camera(BOUNDS, layout);
            for (rig, transform) in heads(BOUNDS, layout, &camera) {
                let (lighting, _) = character(layout, &rig, transform, resolved);
                let mut expected = CharacterLighting { key: lighting.key, ..resolved };
                match layout.light() {
                    HeadLight::Key => assert_eq!(lighting.key.intensity, sun.intensity),
                    HeadLight::Gi { ceilings } => {
                        assert_eq!(lighting.key.intensity, 0.0, "the key light is off");
                        expected.gi.intensity = GI_EXPOSURE;
                        if let Some(ceilings) = ceilings {
                            (expected.gi.max_saturation, expected.gi.max_value) =
                                (ceilings.saturation, ceilings.value);
                        }
                    }
                }
                assert_eq!(lighting, expected, "{layout:?}: the rest of the rig as resolved");
                assert!(lighting.is_valid());
            }
        }
        assert_eq!(FaceLayout::Gi.light(), HeadLight::Gi { ceilings: None }, "the preset's ceilings");
        assert_eq!(FaceLayout::GiClamped.light(), HeadLight::Gi { ceilings: Some(CLAMPED_GI) });
        assert_eq!(FaceLayout::Gi.rows(), FaceLayout::GiClamped.rows(), "the same heads");
        let gloomy = GI.iter().flat_map(|row| row.iter()).filter(|rig| rig.expression.gloom > 0.0);
        assert_eq!(gloomy.count(), 2, "gloom's occlusion shows in the GI");
    }

    #[test]
    fn heads_stand_in_rows_facing_the_camera_then_turn_and_nod() {
        for layout in FaceLayout::ALL {
            let camera = camera(BOUNDS, layout);
            let heads = heads(BOUNDS, layout, &camera);
            assert_eq!(heads.len(), layout.heads());
            for (rig, transform) in &heads {
                let at = transform.transform_point3(Vec3::ZERO);
                let toward_camera = (camera.eye() - at).normalize();
                let forward = transform.transform_vector3(Vec3::Z);
                // The camera's direction in the head's space: its yaw toward the head's right and
                // its pitch above the head's level.
                let up = transform.transform_vector3(Vec3::Y);
                let right = forward.cross(up);
                let level = (toward_camera - up * toward_camera.dot(up)).normalize();
                let turned = level.dot(right).atan2(level.dot(forward)).to_degrees();
                let raised = toward_camera.dot(up).asin().to_degrees();
                assert!((turned - rig.turn).abs() < 1e-3, "{layout:?} {rig:?}: turned {turned}");
                assert!((raised - rig.nod).abs() < 1e-3, "{layout:?} {rig:?}: raised {raised}");
                assert!(at.x < -2.5, "clear of the character: {at}");
            }
            // Neighbors don't overlap.
            let first_row: Vec<Vec3> =
                heads[..layout.rows()[0].len()].iter().map(|(_, t)| t.transform_point3(Vec3::ZERO)).collect();
            assert!(first_row.windows(2).all(|pair| pair[0].distance(pair[1]) > 2.0 * HEAD_HALF_EXTENTS.0));
        }
        // Facing the camera, a head's +X lies across the line of sight.
        let camera = camera(BOUNDS, FaceLayout::Lights);
        for (_, transform) in heads(BOUNDS, FaceLayout::Lights, &camera) {
            let across = transform.transform_vector3(Vec3::X);
            assert!(across.dot(camera.eye() - transform.transform_point3(Vec3::ZERO)).abs() < 1e-4);
        }
    }

    #[test]
    fn layouts_are_named_as_the_command_line_spells_them() {
        for (name, layout) in FaceLayout::NAMES.into_iter().zip(FaceLayout::ALL) {
            assert_eq!(FaceLayout::from_name(name), Some(layout));
        }
        assert_eq!(FaceLayout::from_name("bangs"), None);
        assert_eq!(FaceLayout::default(), FaceLayout::Lights, "C3's rig, as `--faces` alone draws it");
    }

    #[test]
    fn rigs_pose_their_lids_and_gaze_through_the_heads_morph_targets() {
        let weights = |blink, look| FaceRig { blink, look, ..rig(0.0, 0.0, 0.0, 0.0) }.morph_weights();
        assert_eq!(weights(0.0, (0.0, 0.0)), [0.0; 6], "at rest");
        assert_eq!(weights(0.5, (0.0, 0.0)), [1.0, 0.0, 0.0, 0.0, 0.0, 0.0], "half closed: the first step");
        assert_eq!(weights(1.0, (0.0, 0.0)), [1.0, 1.0, 0.0, 0.0, 0.0, 0.0], "closed: both steps");
        assert_eq!(weights(0.75, (0.0, 0.0)), [1.0, 0.5, 0.0, 0.0, 0.0, 0.0]);
        assert_eq!(weights(0.0, (1.0, -0.5)), [0.0, 0.0, 1.0, 0.0, 0.0, 0.5], "toward the head's left, down");
        assert_eq!(weights(0.0, (-1.0, 1.0)), [0.0, 0.0, 0.0, 1.0, 1.0, 0.0]);
        let pose = rig(0.0, 0.0, 0.0, 0.0).render_pose();
        assert!(pose.palette.is_empty(), "the head has no joints");
        // Only the eyes' layouts move the eyes.
        for layout in FaceLayout::ALL {
            let moved =
                layout.rows().iter().flat_map(|row| row.iter()).any(|rig| rig.morph_weights() != [0.0; 6]);
            assert_eq!(moved, matches!(layout, FaceLayout::Gaze | FaceLayout::Blink), "{layout:?}");
        }
    }

    #[test]
    fn the_eyes_layouts_frame_the_eyes_line_close() {
        for layout in [FaceLayout::Eyes, FaceLayout::EyesTurned, FaceLayout::Gaze, FaceLayout::Blink] {
            let camera = camera(BOUNDS, layout);
            let heads = heads(BOUNDS, layout, &camera);
            for (_, transform) in &heads {
                let eyes = transform.transform_point3(Vec3::new(0.0, EYE_LINE, EYE_DEPTH));
                let distance = camera.eye().distance(eyes);
                assert!(distance > 0.15 && distance < 1.0, "{layout:?}: {distance} m from the eyes");
            }
        }
        // A single head facing the camera: its eyes' line is what the camera looks at.
        let camera = camera(BOUNDS, FaceLayout::Eyes);
        let (_, transform) = heads(BOUNDS, FaceLayout::Eyes, &camera)[0];
        let eyes = transform.transform_point3(Vec3::new(0.0, EYE_LINE, EYE_DEPTH));
        let toward = (eyes - camera.eye()).normalize();
        let ahead = camera.facing_rotation() * Vec3::NEG_Z;
        assert!(toward.dot(ahead) > 0.999, "near the middle of the view: {toward} {ahead}");
    }
}
