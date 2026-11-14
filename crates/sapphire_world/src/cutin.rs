//! Cut-ins (M5 L10): a live close-up of a character cut across the frame when a big move lands, as a document.
//!
//! A cut-in is a view, not a picture: a stand of the character, playing a clip or holding a pose of its own,
//! drawn every frame from a camera framed on one of its joints through the renderer's inset path (M3 U6), under
//! a light rig of its own before a backdrop of one or two colors and speed lines. The interface composes it: a
//! panel over the game's view and under the HUD, the whole frame or a band with slanted sides, that comes in,
//! holds and goes. Nothing here draws or simulates: [`CutInLibrary`] is the document, and [`CutInPanel`] says
//! where the panel is at a moment of its own clock, as one quad of the interface's draw list.
//!
//! A cue's row names a cut-in by its name (`sapphire_cues`' `CueRow::cutin`); the game looks it up for the
//! kind that plays it ([`CutInLibrary::find`]), so a new playable kind brings its own close-up by adding an
//! entry with its kind and no Rust.
//!
//! **Units:** seconds of the presentation's clock, which hit stop and slow motion don't hold and a pause does;
//! degrees; meters in the stand's space (+X the picture's right seen from the character's front, +Y up, +Z
//! toward the front); shares of the frame's height; colors linear RGB but a border's, display-encoded bytes as
//! the interface's are.

use std::collections::BTreeSet;

use bevy_ecs::prelude::Resource;
use bevy_reflect::TypePath;
use sapphire_look::UiQuad;
use sapphire_reflect::Versioned;
use sapphire_serialize::{AssetId, DocumentError};
use serde::{Deserialize, Serialize};

use crate::tod::Rgb;

/// The shipped cut-ins (see [`CutInLibrary::default_cutins`]).
const DEFAULT_CUTINS: &str = include_str!("../default_cutins.ron");

/// The longest an enter or an exit takes, in seconds.
pub const MAX_MOTION_SECONDS: f32 = 2.0;
/// The longest a panel holds, in seconds.
pub const MAX_HOLD_SECONDS: f32 = 10.0;

/// One cut-in: whose it is, what the stand plays, how the camera frames it, its light and backdrop, its panel
/// and its sound.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CutIn {
    /// What a cue's row calls it.
    pub name: String,
    /// The playable kind it's for, by its document's ID; `None` is whoever plays it. A kind's own entry of a
    /// name wins over the one for anyone.
    #[serde(default)]
    pub kind: Option<AssetId>,
    pub clip: CutInClip,
    pub camera: CutInCamera,
    pub light: CutInLight,
    pub backdrop: CutInBackdrop,
    pub panel: CutInPanel,
    /// An audio `Cue` document to play flat as it starts, by its ID.
    #[serde(default)]
    pub sound: Option<AssetId>,
}

/// What the stand plays: a clip of the kind's graph, by its name, from `start` at `rate`; a rate of 0 holds the
/// pose at `start`. The stand plays it on its own, whatever the fighter does, and holds its last pose.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CutInClip {
    /// The clip's name, as its source calls it, among the clips the kind's graph holds.
    pub name: String,
    /// Seconds into the clip where it starts.
    #[serde(default)]
    pub start: f32,
    /// Its seconds per second of the cut-in's: 0 to 4.
    #[serde(default = "one")]
    pub rate: f32,
}

fn one() -> f32 {
    1.0
}

/// A joint of the stand the camera frames.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum CutInJoint {
    #[default]
    Head,
    Chest,
    Hips,
    RightHand,
    LeftHand,
}

/// The camera: from `offset` off the joint, looking at `look` off it, turned by `roll` about its view, across a
/// field of view of `fov`, both in degrees.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CutInCamera {
    pub joint: CutInJoint,
    /// Where the eye is from the joint, meters in the stand's space.
    pub offset: [f32; 3],
    /// Where it looks from the joint, likewise.
    #[serde(default)]
    pub look: [f32; 3],
    /// Degrees clockwise about the view: a Dutch angle.
    #[serde(default)]
    pub roll: f32,
    /// The vertical field of view, 5 to 120 degrees.
    pub fov: f32,
}

/// The stand's light, as a showcase's (`sapphire_assets::ui::UiInset`): a light rig by its name, a key light's
/// color and intensity, an ambient light, and the exposure.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CutInLight {
    /// A light rig of `default_light_rigs.ron`, by its name: the default's when it names none there.
    pub rig: String,
    pub light: Rgb,
    pub intensity: f32,
    pub ambient_sky: Rgb,
    pub ambient_ground: Rgb,
    pub ambient: f32,
    pub exposure: f32,
}

/// What's behind the stand: `color`, or a gradient from it to `to` along `angle` (degrees clockwise from
/// left-to-right), and speed lines over it.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CutInBackdrop {
    pub color: Rgb,
    #[serde(default)]
    pub to: Option<Rgb>,
    #[serde(default)]
    pub angle: f32,
    #[serde(default)]
    pub lines: Option<CutInLines>,
}

/// Speed lines over a cut-in's backdrop, as the screen effect draws them (`speed_lines` in
/// `default_screen_effects.ron`), in the picture's heights. With flashes reduced they hold one drawing.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CutInLines {
    pub opacity: f32,
    /// Their center in the picture's texture coordinates.
    #[serde(default = "middle")]
    pub center: [f32; 2],
    /// Lines around the center, 1 to 1024.
    pub count: u32,
    /// A line's width at its outer end, as a share of its slot.
    pub width: f32,
    pub inner_radius: f32,
    pub spread: f32,
    pub color: Rgb,
    /// Drawings a second, 0 for one.
    pub rate: f32,
}

fn middle() -> [f32; 2] {
    [0.5, 0.5]
}

/// The panel the interface shows the picture in, and how it comes and goes.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CutInPanel {
    pub shape: PanelShape,
    pub enter: PanelMotion,
    /// Seconds it holds between them.
    pub hold: f32,
    pub exit: PanelMotion,
    /// A line along a band's slanted sides.
    #[serde(default)]
    pub border: Option<PanelBorder>,
}

/// The panel's shape at its hold.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub enum PanelShape {
    /// The whole frame.
    Full,
    /// A band across the frame's width, `height` of the frame's height through its middle, its middle line
    /// `center` of the way down, its top and bottom sides each turned by an angle in degrees, positive rising
    /// to the right.
    Band { height: f32, center: f32, top_angle: f32, bottom_angle: f32 },
}

/// How a panel comes or goes: over `seconds`, by `curve`, sliding from or to a side or opening from or closing
/// to its middle line.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PanelMotion {
    pub seconds: f32,
    #[serde(default)]
    pub curve: PanelCurve,
    pub side: PanelSide,
}

/// A motion's ease.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum PanelCurve {
    Linear,
    EaseIn,
    #[default]
    EaseOut,
    Smooth,
}

impl PanelCurve {
    /// Its share at `t`, 0 to 1.
    #[must_use]
    pub fn at(self, t: f32) -> f32 {
        let t = t.clamp(0.0, 1.0);
        match self {
            Self::Linear => t,
            Self::EaseIn => t * t,
            Self::EaseOut => 1.0 - (1.0 - t) * (1.0 - t),
            Self::Smooth => t * t * (3.0 - 2.0 * t),
        }
    }
}

/// Where a panel comes from or goes to.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum PanelSide {
    Left,
    Right,
    Top,
    Bottom,
    /// Opening from its middle line, or closing to it.
    Centre,
}

/// A line along a band's sides: `width` UI points inside them, in `color`, display-encoded with straight alpha.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PanelBorder {
    pub width: f32,
    pub color: [u8; 4],
}

/// Where a panel is in its life at a moment of its clock.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum PanelPhase {
    /// Coming in, its motion's share done, 0 to 1.
    Enter(f32),
    Hold,
    /// Going, the share of it still showing, 1 to 0.
    Exit(f32),
}

impl CutInPanel {
    /// Seconds from its start to its end.
    #[must_use]
    pub fn seconds(&self) -> f32 {
        self.enter.seconds + self.hold + self.exit.seconds
    }

    /// Where it is `seconds` after it started: `None` once it's over.
    #[must_use]
    pub fn phase(&self, seconds: f32) -> Option<PanelPhase> {
        let (enter, hold) = (self.enter.seconds, self.hold);
        if seconds.is_nan() || seconds < 0.0 || seconds >= self.seconds() {
            return (seconds < 0.0).then_some(PanelPhase::Enter(0.0));
        }
        Some(if seconds < enter {
            PanelPhase::Enter(self.enter.curve.at(seconds / enter))
        } else if seconds < enter + hold {
            PanelPhase::Hold
        } else {
            let gone = (seconds - enter - hold) / self.exit.seconds.max(f32::MIN_POSITIVE);
            PanelPhase::Exit(1.0 - self.exit.curve.at(gone))
        })
    }

    /// The rectangle the panel covers at its hold, in UI points of a viewport `viewport` points across and
    /// down: `x0, y0, x1, y1`, inside the viewport. Its picture covers it, so this is the picture's shape.
    #[must_use]
    pub fn picture_rect(&self, viewport: [f32; 2]) -> [f32; 4] {
        let [width, height] = viewport;
        match self.shape {
            PanelShape::Full => [0.0, 0.0, width, height],
            PanelShape::Band { .. } => {
                let ([top, _], [bottom, _]) = self.sides(viewport, 1.0);
                let y = |line: Side, x: f32| line.point[1] - line.rise * (x - line.point[0]);
                let y0 = y(top, 0.0).min(y(top, width)).clamp(0.0, height);
                let y1 = y(bottom, 0.0).max(y(bottom, width)).clamp(y0, height);
                [0.0, y0, width, y1]
            }
        }
    }

    /// The band's top and bottom sides with their half-height scaled by `open`, each as a point and a rise,
    /// with its inward normal: `None` for the whole frame.
    fn sides(&self, [width, height]: [f32; 2], open: f32) -> ([Side; 2], [Side; 2]) {
        let (share, center, top_angle, bottom_angle) = match self.shape {
            PanelShape::Band { height, center, top_angle, bottom_angle } => {
                (height, center, top_angle, bottom_angle)
            }
            PanelShape::Full => (1.0, 0.5, 0.0, 0.0),
        };
        let (middle, half) = (center * height, 0.5 * share * height * open);
        let side = |y: f32, angle: f32, down: bool| {
            let radians = angle.to_radians();
            let (sin, cos) = radians.sin_cos();
            let normal = if down { [sin, cos] } else { [-sin, -cos] };
            Side { point: [0.5 * width, y], rise: radians.tan(), normal }
        };
        let top = side(middle - half, top_angle, true);
        let bottom = side(middle + half, bottom_angle, false);
        ([top, top], [bottom, bottom])
    }

    /// The quad showing a picture in it `seconds` after it started, over a viewport `viewport` points across and
    /// down: the picture sliding with it, or the panel opening over it. With `still` (flashes reduced) it comes
    /// and goes by fading where it holds. `None` once it's over, or while nothing of it shows.
    #[must_use]
    pub fn quad(&self, seconds: f32, still: bool, viewport: [f32; 2]) -> Option<UiQuad> {
        let phase = self.phase(seconds)?;
        let (share, side) = match phase {
            PanelPhase::Enter(share) => (share, self.enter.side),
            PanelPhase::Hold => (1.0, PanelSide::Centre),
            PanelPhase::Exit(share) => (share, self.exit.side),
        };
        if share <= 0.0 || viewport[0] <= 0.0 || viewport[1] <= 0.0 {
            return None;
        }
        let [x0, y0, x1, y1] = self.picture_rect(viewport);
        let (width, height) = (x1 - x0, y1 - y0);
        if width <= 0.0 || height <= 0.0 {
            return None;
        }
        let (mut shift, mut open, mut opacity) = ([0.0, 0.0], 1.0, 1.0);
        let mut slide = 0.0;
        if still {
            opacity = share;
        } else {
            match side {
                // In widths: the panel slides along the frame's width, its picture with it.
                PanelSide::Left => slide = share - 1.0,
                PanelSide::Right => slide = 1.0 - share,
                // Far enough that it starts and ends wholly off the frame.
                PanelSide::Top => shift[1] = (share - 1.0) * y1,
                PanelSide::Bottom => shift[1] = (1.0 - share) * (viewport[1] - y0),
                PanelSide::Centre => open = share,
            }
        }
        let origin = [x0, y0 + shift[1]];
        let mut quad = UiQuad::picture(origin, [width, 0.0], [0.0, height], (opacity * 255.0).round() as u8);
        if slide != 0.0 {
            quad.uv = [-slide, 0.0, 1.0 - slide, 1.0];
            quad.clip = [slide.max(0.0), (1.0 + slide).min(1.0)];
        }
        let banded = matches!(self.shape, PanelShape::Band { .. });
        if banded || open < 1.0 {
            let ([top, _], [bottom, _]) = self.sides(viewport, open);
            let moved = [slide * viewport[0], shift[1]];
            let edge = |line: Side| {
                let [nx, ny] = line.normal;
                let point = [line.point[0] + moved[0], line.point[1] + moved[1]];
                [nx * width, ny * height, nx * (origin[0] - point[0]) + ny * (origin[1] - point[1])]
            };
            quad.edges = [edge(top), edge(bottom)];
        }
        if let (true, Some(border)) = (banded, self.border) {
            quad.outline = border.width;
            quad.outline_color = border.color;
        }
        Some(quad)
    }
}

/// A side of a band: a point it passes through at the frame's middle, how much it rises per point to the right,
/// and its normal into the band.
#[derive(Clone, Copy, Debug)]
struct Side {
    point: [f32; 2],
    rise: f32,
    normal: [f32; 2],
}

/// Cut-ins by name and kind (a `CutInLibrary` document, schema 1). A library has at least one cut-in, no two of
/// one name for one kind, and every number in its range: it is valid however it was made, deserialization
/// included.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, TypePath, Resource)]
#[type_path = "sapphire::world"]
#[serde(try_from = "UncheckedLibrary")]
pub struct CutInLibrary {
    cutins: Vec<CutIn>,
}

impl Versioned for CutInLibrary {
    const VERSION: u32 = 1;
}

/// A library that isn't valid.
#[derive(Clone, Debug, PartialEq, thiserror::Error)]
pub enum CutInLibraryError {
    #[error("there are no cut-ins")]
    Empty,
    #[error("cut-in {0}: a name can't be empty")]
    EmptyName(usize),
    #[error("cut-in `{0}` appears twice for one kind")]
    Duplicate(String),
    #[error("cut-in `{name}`: `{field}` {problem}")]
    Invalid { name: String, field: &'static str, problem: &'static str },
}

/// A library as written, before [`CutInLibrary::new`] checks it.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct UncheckedLibrary {
    cutins: Vec<CutIn>,
}

impl TryFrom<UncheckedLibrary> for CutInLibrary {
    type Error = CutInLibraryError;

    fn try_from(library: UncheckedLibrary) -> Result<Self, CutInLibraryError> {
        Self::new(library.cutins)
    }
}

impl CutIn {
    /// Checks every number, returning the field and what's wrong.
    fn validate(&self) -> Result<(), (&'static str, &'static str)> {
        fn within(value: f32, range: std::ops::RangeInclusive<f32>) -> bool {
            value.is_finite() && range.contains(&value)
        }
        fn check(
            ok: bool,
            field: &'static str,
            problem: &'static str,
        ) -> Result<(), (&'static str, &'static str)> {
            if ok { Ok(()) } else { Err((field, problem)) }
        }
        let color = |rgb: Rgb| rgb.iter().all(|&channel| within(channel, 0.0..=64.0));
        let near = |offset: [f32; 3]| offset.iter().all(|&meters| within(meters, -10.0..=10.0));
        check(!self.clip.name.is_empty(), "clip.name", "can't be empty")?;
        check(within(self.clip.start, 0.0..=60.0), "clip.start", "must be within 0..=60 seconds")?;
        check(within(self.clip.rate, 0.0..=4.0), "clip.rate", "must be within 0..=4")?;
        let camera = &self.camera;
        check(near(camera.offset), "camera.offset", "must be within 10 m of the joint")?;
        check(near(camera.look), "camera.look", "must be within 10 m of the joint")?;
        check(
            (0..3).any(|axis| (camera.offset[axis] - camera.look[axis]).abs() > 1e-3),
            "camera.offset",
            "must be apart from where it looks",
        )?;
        check(within(camera.roll, -180.0..=180.0), "camera.roll", "must be within -180..=180 degrees")?;
        check(within(camera.fov, 5.0..=120.0), "camera.fov", "must be within 5..=120 degrees")?;
        let light = &self.light;
        check(color(light.light), "light.light", "must be within 0..=64")?;
        check(within(light.intensity, 0.0..=64.0), "light.intensity", "must be within 0..=64")?;
        check(color(light.ambient_sky), "light.ambient_sky", "must be within 0..=64")?;
        check(color(light.ambient_ground), "light.ambient_ground", "must be within 0..=64")?;
        check(within(light.ambient, 0.0..=16.0), "light.ambient", "must be within 0..=16")?;
        check(
            light.exposure > 0.0 && within(light.exposure, 0.0..=16.0),
            "light.exposure",
            "must be within (0, 16]",
        )?;
        let backdrop = &self.backdrop;
        check(color(backdrop.color), "backdrop.color", "must be within 0..=64")?;
        check(backdrop.to.is_none_or(color), "backdrop.to", "must be within 0..=64")?;
        check(within(backdrop.angle, -360.0..=360.0), "backdrop.angle", "must be within -360..=360 degrees")?;
        if let Some(lines) = &backdrop.lines {
            check(within(lines.opacity, 0.0..=1.0), "backdrop.lines.opacity", "must be within 0..=1")?;
            check(
                lines.center.iter().all(|&at| within(at, -1.0..=2.0)),
                "backdrop.lines.center",
                "must be within -1..=2",
            )?;
            check((1..=1024).contains(&lines.count), "backdrop.lines.count", "must be within 1..=1024")?;
            check(within(lines.width, 0.0..=1.0), "backdrop.lines.width", "must be within 0..=1")?;
            check(
                within(lines.inner_radius, 0.0..=4.0),
                "backdrop.lines.inner_radius",
                "must be within 0..=4",
            )?;
            check(within(lines.spread, 0.0..=4.0), "backdrop.lines.spread", "must be within 0..=4")?;
            check(color(lines.color), "backdrop.lines.color", "must be within 0..=64")?;
            check(within(lines.rate, 0.0..=60.0), "backdrop.lines.rate", "must be within 0..=60")?;
        }
        let panel = &self.panel;
        if let PanelShape::Band { height, center, top_angle, bottom_angle } = panel.shape {
            check(height > 0.0 && within(height, 0.0..=1.0), "panel.shape.height", "must be within (0, 1]")?;
            check(within(center, 0.0..=1.0), "panel.shape.center", "must be within 0..=1")?;
            check(
                within(top_angle, -45.0..=45.0),
                "panel.shape.top_angle",
                "must be within -45..=45 degrees",
            )?;
            check(
                within(bottom_angle, -45.0..=45.0),
                "panel.shape.bottom_angle",
                "must be within -45..=45 degrees",
            )?;
        }
        let seconds = 0.0..=MAX_MOTION_SECONDS;
        check(within(panel.enter.seconds, seconds.clone()), "panel.enter.seconds", "must be within 0..=2")?;
        check(within(panel.exit.seconds, seconds), "panel.exit.seconds", "must be within 0..=2")?;
        check(
            panel.hold > 0.0 && within(panel.hold, 0.0..=MAX_HOLD_SECONDS),
            "panel.hold",
            "must be within (0, 10]",
        )?;
        if let Some(border) = &panel.border {
            check(within(border.width, 0.0..=32.0), "panel.border.width", "must be within 0..=32 points")?;
        }
        Ok(())
    }
}

impl CutInLibrary {
    /// A library of `cutins`, checked.
    pub fn new(cutins: Vec<CutIn>) -> Result<Self, CutInLibraryError> {
        if cutins.is_empty() {
            return Err(CutInLibraryError::Empty);
        }
        let mut names = BTreeSet::new();
        for (index, cutin) in cutins.iter().enumerate() {
            if cutin.name.is_empty() {
                return Err(CutInLibraryError::EmptyName(index));
            }
            if !names.insert((cutin.name.as_str(), cutin.kind)) {
                return Err(CutInLibraryError::Duplicate(cutin.name.clone()));
            }
            cutin.validate().map_err(|(field, problem)| CutInLibraryError::Invalid {
                name: cutin.name.clone(),
                field,
                problem,
            })?;
        }
        Ok(Self { cutins })
    }

    /// Sapphire's cut-ins (`default_cutins.ron` beside this crate's manifest), compiled in: `ultimate`, the
    /// player's ultimate's band across the frame, for whoever plays it.
    pub fn default_cutins() -> Self {
        Self::from_ron(DEFAULT_CUTINS).expect("the shipped cut-ins are checked by this module's tests")
    }

    /// Reads a library document, upgrading older schema versions and checking it.
    pub fn from_ron(text: &str) -> Result<Self, DocumentError> {
        sapphire_serialize::from_ron(text)
    }

    pub fn to_ron(&self) -> Result<String, DocumentError> {
        sapphire_serialize::to_ron(self)
    }

    /// The cut-in called `name` for the playable kind `kind`: its own, or the one for whoever plays it.
    #[must_use]
    pub fn find(&self, name: &str, kind: Option<AssetId>) -> Option<&CutIn> {
        let named = || self.cutins.iter().filter(move |cutin| cutin.name == name);
        named()
            .find(|cutin| kind.is_some() && cutin.kind == kind)
            .or_else(|| named().find(|cutin| cutin.kind.is_none()))
    }

    /// Whether some cut-in is called `name`, for any kind.
    #[must_use]
    pub fn names(&self, name: &str) -> bool {
        self.cutins.iter().any(|cutin| cutin.name == name)
    }

    /// The cut-ins, in order.
    pub fn cutins(&self) -> &[CutIn] {
        &self.cutins
    }

    /// Every cut-in across the whole frame, its band set aside: how the sandbox's `--cutins full` shows that
    /// shape with the entries there are.
    #[must_use]
    pub fn full_frame(mut self) -> Self {
        for cutin in &mut self.cutins {
            cutin.panel.shape = PanelShape::Full;
        }
        self
    }

    /// The sounds its cut-ins play: what an app loads for them.
    pub fn sounds(&self) -> impl Iterator<Item = AssetId> + '_ {
        self.cutins.iter().filter_map(|cutin| cutin.sound)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn band() -> CutInPanel {
        CutInPanel {
            shape: PanelShape::Band { height: 0.4, center: 0.5, top_angle: 5.0, bottom_angle: -5.0 },
            enter: PanelMotion { seconds: 0.2, curve: PanelCurve::Linear, side: PanelSide::Left },
            hold: 1.0,
            exit: PanelMotion { seconds: 0.2, curve: PanelCurve::Linear, side: PanelSide::Right },
            border: Some(PanelBorder { width: 2.0, color: [255, 255, 255, 255] }),
        }
    }

    /// The distance inside line `edge` of `quad` at `point`, in points.
    fn inside(quad: &UiQuad, edge: usize, point: [f32; 2]) -> f32 {
        let local =
            [(point[0] - quad.origin[0]) / quad.axis_x[0], (point[1] - quad.origin[1]) / quad.axis_y[1]];
        let [x, y, offset] = quad.edges[edge];
        x * local[0] + y * local[1] + offset
    }

    #[test]
    fn the_shipped_cut_ins_read_and_write_back() {
        let library = CutInLibrary::default_cutins();
        let ultimate = library.find("ultimate", None).expect("the ultimate's");
        assert!(matches!(ultimate.panel.shape, PanelShape::Band { .. }));
        let written = library.to_ron().expect("writes");
        assert_eq!(CutInLibrary::from_ron(&written).expect("reads back"), library);
    }

    #[test]
    fn a_kind_s_own_wins_over_anyone_s() {
        let library = CutInLibrary::default_cutins();
        let anyone = library.cutins()[0].clone();
        let kind = AssetId::from_text("0a0a0a0a-0000-4000-8000-000000000001");
        let own =
            CutIn { kind: Some(kind), camera: CutInCamera { fov: 30.0, ..anyone.camera }, ..anyone.clone() };
        let library = CutInLibrary::new(vec![anyone.clone(), own.clone()]).expect("valid");
        assert_eq!(library.find(&anyone.name, Some(kind)), Some(&own));
        assert_eq!(
            library.find(&anyone.name, Some(AssetId::from_text("0a0a0a0a-0000-4000-8000-000000000002"))),
            Some(&anyone)
        );
        assert_eq!(library.find(&anyone.name, None), Some(&anyone));
        assert_eq!(library.find("nothing", None), None);
        assert!(library.names(&anyone.name) && !library.names("nothing"));
    }

    #[test]
    fn a_bad_library_is_refused_naming_the_cut_in() {
        let library = CutInLibrary::default_cutins();
        let good = library.cutins()[0].clone();
        assert_eq!(CutInLibrary::new(Vec::new()), Err(CutInLibraryError::Empty));
        let unnamed = CutIn { name: String::new(), ..good.clone() };
        assert_eq!(CutInLibrary::new(vec![unnamed]), Err(CutInLibraryError::EmptyName(0)));
        assert_eq!(
            CutInLibrary::new(vec![good.clone(), good.clone()]),
            Err(CutInLibraryError::Duplicate(good.name.clone()))
        );
        let wide = CutIn { camera: CutInCamera { fov: 170.0, ..good.camera }, ..good.clone() };
        assert_eq!(
            CutInLibrary::new(vec![wide]),
            Err(CutInLibraryError::Invalid {
                name: good.name.clone(),
                field: "camera.fov",
                problem: "must be within 5..=120 degrees"
            })
        );
        let nan = CutIn { panel: CutInPanel { hold: f32::NAN, ..good.panel }, ..good.clone() };
        assert!(matches!(
            CutInLibrary::new(vec![nan]),
            Err(CutInLibraryError::Invalid { field: "panel.hold", .. })
        ));
        // Fields nobody knows, and a newer schema, are refused as documents.
        let text = library.to_ron().expect("writes").replacen("hold:", "held: 1.0, hold:", 1);
        assert!(CutInLibrary::from_ron(&text).is_err());
        let text = library.to_ron().expect("writes").replacen("version: 1", "version: 9", 1);
        assert!(CutInLibrary::from_ron(&text).is_err());
    }

    #[test]
    fn a_panel_enters_holds_and_exits_on_its_clock() {
        let panel = band();
        assert!((panel.seconds() - 1.4).abs() < 1e-6);
        assert_eq!(panel.phase(0.1), Some(PanelPhase::Enter(0.5)));
        assert_eq!(panel.phase(0.7), Some(PanelPhase::Hold));
        assert!(matches!(panel.phase(1.3), Some(PanelPhase::Exit(left)) if (left - 0.5).abs() < 1e-4));
        assert_eq!(panel.phase(1.401), None);
        assert_eq!(panel.phase(f32::NAN), None);
        assert_eq!(PanelCurve::EaseOut.at(0.5), 0.75);
        assert_eq!(PanelCurve::EaseIn.at(0.5), 0.25);
        assert_eq!(PanelCurve::Smooth.at(0.5), 0.5);
    }

    #[test]
    fn a_band_s_quad_covers_its_slanted_sides() {
        let panel = band();
        let viewport = [1000.0, 500.0];
        let [x0, y0, x1, y1] = panel.picture_rect(viewport);
        // 0.4 of 500 through the middle, each side turned 5 degrees: 500 · tan 5° ≈ 43.7 more at its ends.
        assert_eq!((x0, x1), (0.0, 1000.0));
        assert!((y0 - (150.0 - 43.74)).abs() < 0.1 && (y1 - (350.0 + 43.74)).abs() < 0.1, "{y0} {y1}");
        let quad = panel.quad(0.7, false, viewport).expect("held");
        assert_eq!(quad.color[3], 255);
        assert_eq!((quad.clip, quad.uv), ([0.0, 1.0], [0.0, 0.0, 1.0, 1.0]));
        assert_eq!((quad.outline, quad.outline_color), (2.0, [255; 4]));
        // On its sides at the middle, and a point inside both by its distance.
        assert!(inside(&quad, 0, [500.0, 150.0]).abs() < 1e-3);
        assert!(inside(&quad, 1, [500.0, 350.0]).abs() < 1e-3);
        assert!((inside(&quad, 0, [500.0, 250.0]) - 100.0 * 5f32.to_radians().cos()).abs() < 1e-2);
        assert!(inside(&quad, 1, [500.0, 250.0]) > 99.0);
        // The top rises to the right: inside at the right end higher up than at the left.
        assert!(inside(&quad, 0, [990.0, 120.0]) > 0.0 && inside(&quad, 0, [10.0, 120.0]) < 0.0);
        // The bottom, turned the other way, falls to the right... by its angle, -5 degrees, rising leftward.
        assert!(inside(&quad, 1, [990.0, 380.0]) > 0.0 && inside(&quad, 1, [10.0, 380.0]) < 0.0);
    }

    #[test]
    fn a_panel_slides_in_with_its_picture_and_out_the_other_side() {
        let panel = band();
        let viewport = [1000.0, 500.0];
        let entering = panel.quad(0.05, false, viewport).expect("coming in");
        // A quarter in from the left: its leading quarter shows the picture's trailing quarter.
        assert_eq!(entering.clip, [0.0, 0.25]);
        assert_eq!(entering.uv, [0.75, 0.0, 1.75, 1.0]);
        // Its sides slid with it: on the top side at its middle, which is 750 points left of the frame's.
        let held = panel.quad(0.7, false, viewport).expect("held");
        let top = inside(&held, 0, [500.0, 200.0]);
        assert!((inside(&entering, 0, [-250.0, 200.0]) - top).abs() < 1e-2);
        let leaving = panel.quad(1.3, false, viewport).expect("going");
        assert!((leaving.clip[0] - 0.5).abs() < 1e-5 && leaving.clip[1] == 1.0, "half gone to the right");
        assert_eq!(panel.quad(1.401, false, viewport), None);
        // Flashes reduced: where it holds, fading.
        let fading = panel.quad(0.05, true, viewport).expect("fading in");
        assert_eq!((fading.clip, fading.uv, fading.edges), (held.clip, held.uv, held.edges));
        assert_eq!(fading.color[3], 64);
    }

    #[test]
    fn a_panel_opens_from_its_middle_and_a_whole_frame_slides_down() {
        let viewport = [1000.0, 500.0];
        let opening = CutInPanel {
            enter: PanelMotion { seconds: 0.2, curve: PanelCurve::Linear, side: PanelSide::Centre },
            ..band()
        };
        let quad = opening.quad(0.1, false, viewport).expect("opening");
        // Half open: its top side 50 points above the middle line, the picture where it holds.
        assert!(inside(&quad, 0, [500.0, 200.0]).abs() < 1e-3);
        assert_eq!(
            (quad.origin, quad.uv),
            (opening.quad(0.7, false, viewport).expect("held").origin, [0.0, 0.0, 1.0, 1.0])
        );
        let full = CutInPanel {
            shape: PanelShape::Full,
            enter: PanelMotion { seconds: 0.2, curve: PanelCurve::Linear, side: PanelSide::Top },
            border: None,
            ..band()
        };
        let held = full.quad(0.7, false, viewport).expect("held");
        assert_eq!(
            (held.origin, held.axis_x, held.axis_y, held.edges),
            ([0.0, 0.0], [1000.0, 0.0], [0.0, 500.0], [[0.0; 3]; 2])
        );
        let sliding = full.quad(0.1, false, viewport).expect("sliding");
        assert_eq!(sliding.origin, [0.0, -250.0]);
        assert_eq!(full.picture_rect(viewport), [0.0, 0.0, 1000.0, 500.0]);
    }
}
