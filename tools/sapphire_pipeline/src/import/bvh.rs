//! BVH motion capture (`.bvh`, M6 A3): a hierarchy of joints with their offsets and channels, then
//! one row of channel values a frame, cooked to a skeleton and one clip by the same path as any other
//! animation source's ([`crate::clip`]): reduction, contacts, the report, mirroring and retargets.
//!
//! A `.bvh` gets `importer: Bvh((…))` ([`BvhSettings`]): the file's unit, the calibration pose, and
//! the [`ClipSettings`] every animation source has, under `clips`. The clip is named after the file
//! (`walk.bvh` cooks `walk`), and its map is the sidecar's `humanoid`, by joint name.
//!
//! ```ron
//! importer: Bvh((
//!     scale: 0.01,
//!     calibration: (pose: APose),
//!     clips: (
//!         root_motion: (bone: "Hips"),
//!         humanoid: { Hips: "Hips", Spine: "Spine", /* … */ RightToes: "RightToeBase" },
//!         retarget: [(target: "<a character's source ID>", names: ["walk"])],
//!     ),
//! )),
//! ```
//!
//! # What a file means
//!
//! The file is Y up and faces +Z, as the engine is, in units of [`BvhSettings::scale`] meters. A
//! joint's translation is its `OFFSET`, except along an axis it has a position channel for, which the
//! channel's value replaces: a root written with an offset of 0 and positions in the capture volume
//! (as most capture is) and one written with its rest offset and the same positions (as Blender
//! writes) mean the same motion. Its rotation is its rotation channels' turns in degrees, composed in
//! the order the file lists them (`Zrotation Xrotation Yrotation` is `Z · X · Y`). `End Site`s carry
//! no channels and become no joints. Every joint gets a track of each kind, sampled whether its
//! channels move or not, and reduction drops the ones that stay at rest.
//!
//! The capture is moved across the floor so that its first frame's root stands over the origin; its
//! height and facing are the file's.
//!
//! # Calibration
//!
//! A [retarget](crate::clip::retarget) reads each bone's turn from rest, so it needs the source's
//! rest to be the pose the targets rest in, a T-pose; a capture's hierarchy rarely is. So the sidecar
//! names the pose the skeleton rests in ([`Calibration`]): a frame of the capture, or the hierarchy
//! itself (every channel at zero); and what the actor stands in there ([`CalibrationPose`]): a T-pose,
//! taken as it is, or an A-pose, whose arms the cook raises to the horizontal: each upper arm, then
//! each lower arm, turned the shortest way until it points along the side's ±X, by the humanoid map.
//! Only the rest changes: every frame's channels are what they were, so a calibration moves a clip's
//! retargets and nothing of the motion on its own rig.
//!
//! The hierarchy's own pose stands where the first frame does: its root is raised or lowered until
//! its lowest joint is as high as the first frame's lowest, since a capture's root offset is often
//! the floor. A frame's pose stands where the actor did.
//!
//! # Time
//!
//! Frame `n` plays at `n` times the file's `Frame Time`. The clip is a whole number of ticks long, the
//! last rounded up, and each tick is sampled between the frames either side of it: rotations by a
//! slerp, translations by a line, held past the last frame.
//!
//! # Byte identity
//!
//! Numbers are read by Rust's own parser, which rounds correctly, and everything after them is
//! scalars and [`sapphire_math::det`], so a BVH cooks to the same bytes on every architecture, and a
//! move may bake its tables from one, as from a glTF.
//!
//! # Malformed files
//!
//! Are errors naming the line, never panics: a token where another belongs, a channel missing from
//! `CHANNELS` or from a frame's row, a frame count the rows don't match, a number that isn't one or
//! isn't finite. Nesting is walked without recursion, and the frames' values and the clip's samples
//! are counted against [`CLIP_BUDGET`] before either is held.

use std::path::Path;

use glam::{Quat, Vec3};
use sapphire_assets::clip::MAX_TICKS;
use sapphire_assets::rig::HumanBone;
use sapphire_assets::skeleton::{Joint, JointTransform, MAX_JOINTS, Skeleton};
use sapphire_math::det;
use serde::{Deserialize, Serialize};

use crate::cache::Stage;
use crate::clip::retarget::{HumanMap, HumanMapError};
use crate::clip::{ClipSettings, SampledClip};
use crate::import::gltf::clips::{CLIP_BUDGET, ClipImportError, lift_settings_root};

/// This importer, as cache keys know it. Bump the version whenever output changes for the same file
/// and settings.
pub const IMPORTER: Stage = Stage { name: "bvh", version: 1 };

/// How a `.bvh` cooks: see the [module docs](self).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct BvhSettings {
    /// Meters per unit of the file: 0.01 for centimeters, which most capture writes.
    pub scale: f32,
    /// The pose the skeleton rests in.
    pub calibration: Calibration,
    /// What every animation source's sidecar says: rate, reduction, root motion, humanoid map,
    /// retargets, mirrors, contacts and events. The file's one clip is named after it.
    pub clips: ClipSettings,
}

impl Default for BvhSettings {
    fn default() -> Self {
        Self { scale: 0.01, calibration: Calibration::default(), clips: ClipSettings::default() }
    }
}

/// Where the skeleton's rest comes from: see the [module docs](self)' "Calibration". The default is
/// the hierarchy's own pose, standing in a T-pose.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Calibration {
    /// The frame, from 0, whose pose is the calibration; `None` for the hierarchy's, every channel
    /// at zero.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub frame: Option<u32>,
    /// What the actor stands in there.
    pub pose: CalibrationPose,
}

/// What a capture's calibration pose is, by name.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum CalibrationPose {
    /// Arms out along ±X, legs straight down: the rest a VRM has. Taken as it is.
    #[default]
    TPose,
    /// Arms hanging down at the sides at any angle: raised to the horizontal by the humanoid map,
    /// which the sidecar must give.
    APose,
}

/// Why a `.bvh` can't be imported.
#[derive(Debug, thiserror::Error)]
pub enum BvhError {
    #[error("the file isn't UTF-8 text")]
    NotText,
    #[error("line {line}: {fault}")]
    Line { line: usize, fault: Fault },
    #[error("the sidecar's `scale` {0} isn't a finite number above 0")]
    Scale(f32),
    #[error("the sidecar calibrates on frame {frame}, but the file's frames run from 0 to {last}")]
    CalibrationFrame { frame: u32, last: usize },
    #[error(
        "an A-pose calibration raises the arms by the humanoid map: add `humanoid` to the sidecar's `clips`"
    )]
    NoArms,
    #[error("an A-pose calibration reads the sidecar's humanoid map")]
    Humanoid(#[source] HumanMapError),
    #[error("joint {joint:?} isn't finite on frame {frame}")]
    NonFinite { joint: String, frame: usize },
    #[error(
        "the capture is {seconds} s long, {ticks} ticks at {rate} a second; a clip holds at most {MAX_TICKS}"
    )]
    TooLong { seconds: f64, ticks: f64, rate: u16 },
    #[error("the capture needs {found} samples, over the budget of {budget}")]
    TooManySamples { found: usize, budget: usize },
}

/// What is wrong on a line of a `.bvh`.
#[derive(Debug, PartialEq, thiserror::Error)]
pub enum Fault {
    #[error("expected {expected}, found {found:?}")]
    Expected { expected: &'static str, found: String },
    #[error("the file ends inside the hierarchy, expecting {expected}")]
    EndsEarly { expected: &'static str },
    #[error("{found:?} isn't a finite number")]
    Number { found: String },
    #[error("CHANNELS says {said} channels, and the line names {found}")]
    ChannelCount { said: usize, found: usize },
    #[error("OFFSET needs 3 numbers on its line, and has {found} before the line's end or a `}}`")]
    OffsetCount { found: usize },
    #[error("{found:?} isn't a channel name (Xposition … Zrotation)")]
    Channel { found: String },
    #[error("joint {joint:?} names channel {channel} twice")]
    RepeatedChannel { joint: String, channel: String },
    #[error("joint {joint:?} has a second OFFSET")]
    RepeatedOffset { joint: String },
    #[error("joint {joint:?} has a second CHANNELS")]
    RepeatedChannels { joint: String },
    #[error("{joint:?} closes before its OFFSET")]
    NoOffset { joint: String },
    #[error("a second ROOT: a file holds one skeleton")]
    SecondRoot,
    #[error("more than {MAX_JOINTS} joints")]
    TooManyJoints,
    #[error("the hierarchy has no channels to animate")]
    NoChannels,
    #[error("{found:?} isn't a frame count")]
    FrameCount { found: String },
    #[error("a capture of no frames")]
    NoFrames,
    #[error("the frame time {found:?} isn't a finite number of seconds above 0")]
    FrameTime { found: String },
    #[error("{frames} frames of {channels} channels is over the budget of {budget} values")]
    TooManyValues { frames: usize, channels: usize, budget: usize },
    #[error("frame {frame} has {found} values, where the hierarchy's channels need {expected}")]
    FrameValues { frame: usize, found: usize, expected: usize },
    #[error("the header says {said} frames, but {found} follow")]
    FewerFrames { said: usize, found: usize },
    #[error("a frame past the {said} the header says")]
    MoreFrames { said: usize },
}

impl From<BvhError> for ClipImportError {
    fn from(error: BvhError) -> Self {
        Self::Bvh(Box::new(error))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Axis {
    X,
    Y,
    Z,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Channel {
    Position(Axis),
    Rotation(Axis),
}

impl Channel {
    fn parse(token: &str) -> Option<Self> {
        let channel = |name: &str| token.eq_ignore_ascii_case(name);
        Some(match () {
            () if channel("Xposition") => Self::Position(Axis::X),
            () if channel("Yposition") => Self::Position(Axis::Y),
            () if channel("Zposition") => Self::Position(Axis::Z),
            () if channel("Xrotation") => Self::Rotation(Axis::X),
            () if channel("Yrotation") => Self::Rotation(Axis::Y),
            () if channel("Zrotation") => Self::Rotation(Axis::Z),
            () => return None,
        })
    }
}

/// A joint of the hierarchy, as the file wrote it.
#[derive(Debug)]
struct Node {
    name: String,
    parent: Option<u16>,
    /// In the file's units.
    offset: Option<Vec3>,
    channels: Option<Vec<Channel>>,
    /// Where its channels start in a frame's row.
    first: usize,
}

/// A parsed `.bvh`: its hierarchy and its frames' values.
#[derive(Debug)]
struct Parsed {
    nodes: Vec<Node>,
    /// Channels in a frame's row.
    width: usize,
    frame_time: f64,
    /// Frame by frame, `width` each.
    values: Vec<f32>,
}

fn at(line: usize, fault: Fault) -> BvhError {
    BvhError::Line { line, fault }
}

fn number(line: usize, token: &str) -> Result<f32, BvhError> {
    token
        .parse::<f32>()
        .ok()
        .filter(|value| value.is_finite())
        .ok_or_else(|| at(line, Fault::Number { found: token.to_owned() }))
}

/// The hierarchy's tokens, each with its line from 1, up to the `MOTION` line.
struct Tokens<'a> {
    tokens: Vec<(usize, &'a str)>,
    next: usize,
    /// The last line read, for an error at the end.
    last_line: usize,
}

impl<'a> Tokens<'a> {
    fn next(&mut self, expected: &'static str) -> Result<(usize, &'a str), BvhError> {
        let token = self.tokens.get(self.next).copied();
        self.next += 1;
        token.ok_or(at(self.last_line, Fault::EndsEarly { expected }))
    }

    fn expect(&mut self, word: &'static str) -> Result<usize, BvhError> {
        let (line, token) = self.next(word)?;
        if token == word {
            Ok(line)
        } else {
            Err(at(line, Fault::Expected { expected: word, found: token.to_owned() }))
        }
    }

    fn vector(&mut self) -> Result<Vec3, BvhError> {
        let mut vector = [0.0; 3];
        for value in &mut vector {
            let (line, token) = self.next("a number")?;
            *value = number(line, token)?;
        }
        Ok(Vec3::from(vector))
    }
}

/// What is open in the hierarchy as it is read.
enum Open {
    Joint(usize),
    EndSite { offset: bool },
}

fn parse(bytes: &[u8]) -> Result<Parsed, BvhError> {
    let text = std::str::from_utf8(bytes).map_err(|_| BvhError::NotText)?;
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let mut lines = text.lines().enumerate().map(|(index, line)| (index + 1, line));

    // The hierarchy runs to the `MOTION` line.
    let mut tokens = Vec::new();
    let mut last_line = 0;
    for (number, line) in lines.by_ref() {
        last_line = number;
        let mut words = line.split_whitespace().peekable();
        if words.peek() == Some(&"MOTION") {
            tokens.extend(words.map(|word| (number, word)));
            break;
        }
        tokens.extend(words.map(|word| (number, word)));
    }
    let mut tokens = Tokens { tokens, next: 0, last_line };
    tokens.expect("HIERARCHY")?;
    tokens.expect("ROOT")?;
    let (_, name) = tokens.next("the root's name")?;
    let mut nodes =
        vec![Node { name: name.to_owned(), parent: None, offset: None, channels: None, first: 0 }];
    let mut open = vec![Open::Joint(0)];
    tokens.expect("{")?;
    let mut width = 0usize;
    while !open.is_empty() {
        let (line, token) = tokens.next("OFFSET, CHANNELS, JOINT, End Site or }")?;
        let joint_name = |open: &[Open], nodes: &[Node]| match open.last() {
            Some(Open::Joint(index)) => nodes[*index].name.clone(),
            _ => "End Site".to_owned(),
        };
        match token {
            "OFFSET" => {
                // Its numbers are on its own line, as CHANNELS' names are, so a short one is that line's;
                // a `}` may close a block after them, as in a one-line `End Site { OFFSET 0 0 0 }`.
                let numbers = tokens.tokens[tokens.next..]
                    .iter()
                    .take_while(|(on, token)| *on == line && *token != "}")
                    .count();
                if numbers != 3 {
                    return Err(at(line, Fault::OffsetCount { found: numbers }));
                }
                let offset = tokens.vector()?;
                match open.last_mut() {
                    Some(Open::Joint(index)) if nodes[*index].offset.is_none() => {
                        nodes[*index].offset = Some(offset)
                    }
                    Some(Open::EndSite { offset: seen }) if !*seen => *seen = true,
                    _ => return Err(at(line, Fault::RepeatedOffset { joint: joint_name(&open, &nodes) })),
                }
            }
            "CHANNELS" => {
                let Some(Open::Joint(index)) = open.last() else {
                    return Err(at(
                        line,
                        Fault::Expected { expected: "OFFSET or }", found: token.to_owned() },
                    ));
                };
                let index = *index;
                if nodes[index].channels.is_some() {
                    return Err(at(line, Fault::RepeatedChannels { joint: nodes[index].name.clone() }));
                }
                let (count_line, count) = tokens.next("a channel count")?;
                let count = count.parse::<usize>().ok().filter(|count| *count <= 6).ok_or_else(|| {
                    at(
                        count_line,
                        Fault::Expected { expected: "a channel count from 0 to 6", found: count.to_owned() },
                    )
                })?;
                // The names are on the count's line, so a missing one is that line's fault, not the next's.
                let named =
                    tokens.tokens[tokens.next..].iter().take_while(|(on, _)| *on == count_line).count();
                if named != count {
                    return Err(at(count_line, Fault::ChannelCount { said: count, found: named }));
                }
                let mut channels = Vec::with_capacity(count);
                for _ in 0..count {
                    let (line, name) = tokens.next("a channel name")?;
                    let channel = Channel::parse(name)
                        .ok_or_else(|| at(line, Fault::Channel { found: name.to_owned() }))?;
                    if channels.contains(&channel) {
                        let joint = nodes[index].name.clone();
                        return Err(at(line, Fault::RepeatedChannel { joint, channel: name.to_owned() }));
                    }
                    channels.push(channel);
                }
                nodes[index].first = width;
                width += count;
                nodes[index].channels = Some(channels);
            }
            "JOINT" => {
                let Some(Open::Joint(parent)) = open.last() else {
                    return Err(at(
                        line,
                        Fault::Expected { expected: "OFFSET or }", found: token.to_owned() },
                    ));
                };
                if nodes.len() >= MAX_JOINTS {
                    return Err(at(line, Fault::TooManyJoints));
                }
                let parent = Some(*parent as u16);
                let (_, name) = tokens.next("the joint's name")?;
                nodes.push(Node { name: name.to_owned(), parent, offset: None, channels: None, first: 0 });
                open.push(Open::Joint(nodes.len() - 1));
                tokens.expect("{")?;
            }
            "End" => {
                if !matches!(open.last(), Some(Open::Joint(_))) {
                    return Err(at(
                        line,
                        Fault::Expected { expected: "OFFSET or }", found: token.to_owned() },
                    ));
                }
                tokens.expect("Site")?;
                open.push(Open::EndSite { offset: false });
                tokens.expect("{")?;
            }
            "}" => {
                let closing = open.pop().expect("the loop runs while something is open");
                let closed_without_offset = match closing {
                    Open::Joint(index) => nodes[index].offset.is_none().then(|| nodes[index].name.clone()),
                    Open::EndSite { offset } => (!offset).then(|| "End Site".to_owned()),
                };
                if let Some(joint) = closed_without_offset {
                    return Err(at(line, Fault::NoOffset { joint }));
                }
            }
            _ => {
                return Err(at(
                    line,
                    Fault::Expected {
                        expected: "OFFSET, CHANNELS, JOINT, End Site or }",
                        found: token.to_owned(),
                    },
                ));
            }
        }
    }
    let motion_line = match tokens.next("MOTION")? {
        (line, "MOTION") => line,
        (line, "ROOT") => return Err(at(line, Fault::SecondRoot)),
        (line, found) => {
            return Err(at(line, Fault::Expected { expected: "MOTION", found: found.to_owned() }));
        }
    };
    if let Some(&(line, found)) = tokens.tokens.get(tokens.next) {
        return Err(at(
            line,
            Fault::Expected { expected: "the end of the MOTION line", found: found.to_owned() },
        ));
    }
    if width == 0 {
        return Err(at(motion_line, Fault::NoChannels));
    }

    // Then the header and a row a frame; blank lines are nothing.
    let mut rows = lines.filter(|(_, line)| !line.trim().is_empty());
    let mut header =
        |expected: &'static str| rows.next().ok_or(at(motion_line, Fault::EndsEarly { expected }));
    let (frames_line, line) = header("Frames:")?;
    let words: Vec<&str> = line.split_whitespace().collect();
    let said = match words.as_slice() {
        ["Frames:", count] => count
            .parse::<usize>()
            .map_err(|_| at(frames_line, Fault::FrameCount { found: (*count).to_owned() }))?,
        _ => {
            return Err(at(
                frames_line,
                Fault::Expected { expected: "Frames: <count>", found: line.trim().to_owned() },
            ));
        }
    };
    if said == 0 {
        return Err(at(frames_line, Fault::NoFrames));
    }
    if said.saturating_mul(width) > CLIP_BUDGET.values {
        return Err(at(
            frames_line,
            Fault::TooManyValues { frames: said, channels: width, budget: CLIP_BUDGET.values },
        ));
    }
    let (time_line, line) = header("Frame Time: <seconds>")?;
    let words: Vec<&str> = line.split_whitespace().collect();
    let frame_time = match words.as_slice() {
        ["Frame", "Time:", seconds] => seconds
            .parse::<f64>()
            .ok()
            .filter(|seconds| seconds.is_finite() && *seconds > 0.0)
            .ok_or_else(|| at(time_line, Fault::FrameTime { found: (*seconds).to_owned() }))?,
        _ => {
            return Err(at(
                time_line,
                Fault::Expected { expected: "Frame Time: <seconds>", found: line.trim().to_owned() },
            ));
        }
    };
    // No more than the text can hold, two bytes a value: a header may claim what isn't there.
    let mut values = Vec::with_capacity((said * width).min(text.len() / 2));
    let mut found = 0;
    for (line_number, line) in rows {
        if found == said {
            return Err(at(line_number, Fault::MoreFrames { said }));
        }
        let count = line.split_whitespace().count();
        if count != width {
            return Err(at(line_number, Fault::FrameValues { frame: found, found: count, expected: width }));
        }
        for token in line.split_whitespace() {
            values.push(number(line_number, token)?);
        }
        found += 1;
    }
    if found < said {
        return Err(at(frames_line, Fault::FewerFrames { said, found }));
    }
    Ok(Parsed { nodes, width, frame_time, values })
}

/// A rotation of `degrees` about `axis`, in `det`.
fn turn(axis: Axis, degrees: f32) -> Quat {
    let radians = degrees * (std::f32::consts::PI / 180.0);
    match axis {
        Axis::X => det::quat_from_rotation_x(radians),
        Axis::Y => det::quat_from_rotation_y(radians),
        Axis::Z => det::quat_from_rotation_z(radians),
    }
}

/// A joint's local transform on one frame: a translation in meters and a unit rotation.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Local {
    translation: Vec3,
    rotation: Quat,
}

/// A `.bvh`, imported: its joints, resting as its calibration says, and its frames, ready to
/// sample. See the [module docs](self).
#[derive(Debug)]
pub struct Capture {
    /// Its one clip's name: the file's.
    pub name: String,
    pub joints: Vec<Joint>,
    parsed: Parsed,
    scale: f32,
    /// What the root moves by, so the first frame stands over the origin.
    recenter: Vec3,
}

impl Capture {
    /// Reads the file at `path`, whose contents are `bytes`, as `settings` say.
    pub fn import(path: &Path, bytes: &[u8], settings: &BvhSettings) -> Result<Self, BvhError> {
        if !(settings.scale.is_finite() && settings.scale > 0.0) {
            return Err(BvhError::Scale(settings.scale));
        }
        let parsed = parse(bytes)?;
        let name = path.file_stem().map_or_else(String::new, |stem| stem.to_string_lossy().into_owned());
        let frames = parsed.values.len() / parsed.width;
        let mut capture =
            Self { name, joints: Vec::new(), parsed, scale: settings.scale, recenter: Vec3::ZERO };
        let first = capture.local_raw(0, 0);
        capture.recenter = Vec3::new(first.translation.x, 0.0, first.translation.z);
        for frame in 0..frames {
            for joint in 0..capture.parsed.nodes.len() {
                let local = capture.local(joint, frame);
                if !(local.translation.is_finite() && local.rotation.is_finite()) {
                    let joint = capture.parsed.nodes[joint].name.clone();
                    return Err(BvhError::NonFinite { joint, frame });
                }
            }
        }
        capture.joints = capture.rest(&settings.calibration, &settings.clips)?;
        Ok(capture)
    }

    fn frames(&self) -> usize {
        self.parsed.values.len() / self.parsed.width
    }

    /// Joint `joint` on frame `frame`, before the capture is moved over the origin.
    fn local_raw(&self, joint: usize, frame: usize) -> Local {
        let node = &self.parsed.nodes[joint];
        let channels = node.channels.as_deref().unwrap_or(&[]);
        let row = &self.parsed.values[frame * self.parsed.width + node.first..][..channels.len()];
        let mut translation = node.offset.expect("every joint closed with its offset");
        let mut rotation = Quat::IDENTITY;
        for (channel, &value) in channels.iter().zip(row) {
            match channel {
                Channel::Position(Axis::X) => translation.x = value,
                Channel::Position(Axis::Y) => translation.y = value,
                Channel::Position(Axis::Z) => translation.z = value,
                Channel::Rotation(axis) => rotation = det::quat_mul(rotation, turn(*axis, value)),
            }
        }
        Local { translation: translation * self.scale, rotation: det::quat_normalize(rotation) }
    }

    /// Joint `joint` on frame `frame`.
    fn local(&self, joint: usize, frame: usize) -> Local {
        let mut local = self.local_raw(joint, frame);
        if self.parsed.nodes[joint].parent.is_none() {
            local.translation -= self.recenter;
        }
        local
    }

    /// Joint `joint` with every channel at zero: its offset, and the root where the first frame's is,
    /// before [`Self::rest`] grounds it.
    fn hierarchy(&self, joint: usize) -> Local {
        let translation = match self.parsed.nodes[joint].parent {
            None => self.local(joint, 0).translation,
            Some(_) => {
                self.parsed.nodes[joint].offset.expect("every joint closed with its offset") * self.scale
            }
        };
        Local { translation, rotation: Quat::IDENTITY }
    }

    /// Each joint's model-space position and rotation, given their locals.
    fn model(&self, locals: &[Local]) -> Vec<Local> {
        let mut model: Vec<Local> = Vec::with_capacity(locals.len());
        for (joint, local) in locals.iter().enumerate() {
            model.push(match self.parsed.nodes[joint].parent {
                None => *local,
                Some(parent) => {
                    let parent = model[usize::from(parent)];
                    Local {
                        translation: parent.translation
                            + det::quat_rotate(parent.rotation, local.translation),
                        rotation: det::quat_normalize(det::quat_mul(parent.rotation, local.rotation)),
                    }
                }
            });
        }
        model
    }

    /// The skeleton's joints, resting as `calibration` says: see the [module docs](self).
    fn rest(&self, calibration: &Calibration, clips: &ClipSettings) -> Result<Vec<Joint>, BvhError> {
        let count = self.parsed.nodes.len();
        let mut locals: Vec<Local> = match calibration.frame {
            Some(frame) => {
                let index = usize::try_from(frame).unwrap_or(usize::MAX);
                if index >= self.frames() {
                    return Err(BvhError::CalibrationFrame { frame, last: self.frames() - 1 });
                }
                (0..count).map(|joint| self.local(joint, index)).collect()
            }
            None => {
                let mut locals: Vec<Local> = (0..count).map(|joint| self.hierarchy(joint)).collect();
                let lowest = |model: Vec<Local>| {
                    model.iter().map(|joint| joint.translation.y).fold(f32::INFINITY, f32::min)
                };
                let first: Vec<Local> = (0..count).map(|joint| self.local(joint, 0)).collect();
                let rise = lowest(self.model(&first)) - lowest(self.model(&locals));
                locals[0].translation.y += rise;
                locals
            }
        };
        if calibration.pose == CalibrationPose::APose {
            self.raise_arms(&mut locals, clips)?;
        }
        Ok(self
            .parsed
            .nodes
            .iter()
            .zip(&locals)
            .map(|(node, local)| Joint {
                name: node.name.clone(),
                parent: node.parent,
                rest: JointTransform {
                    translation: local.translation.to_array(),
                    rotation: local.rotation.to_array(),
                    scale: [1.0; 3],
                },
            })
            .collect())
    }

    /// Turns each upper arm, then each lower arm, of `locals` the shortest way until it points along
    /// its side's ±X, by the sidecar's humanoid map.
    fn raise_arms(&self, locals: &mut [Local], clips: &ClipSettings) -> Result<(), BvhError> {
        if clips.humanoid.is_empty() {
            return Err(BvhError::NoArms);
        }
        let joints: Vec<Joint> = self
            .parsed
            .nodes
            .iter()
            .map(|node| Joint {
                name: node.name.clone(),
                parent: node.parent,
                rest: JointTransform::IDENTITY,
            })
            .collect();
        let map =
            HumanMap::from_names(&clips.humanoid, &Skeleton::new(&joints)).map_err(BvhError::Humanoid)?;
        use HumanBone::{LeftHand, LeftLowerArm, LeftUpperArm, RightHand, RightLowerArm, RightUpperArm};
        let sides = [
            (LeftUpperArm, LeftLowerArm, LeftHand, Vec3::X),
            (RightUpperArm, RightLowerArm, RightHand, Vec3::NEG_X),
        ];
        for (upper, lower, hand, out) in sides {
            let joint = |bone| map.joint(bone).map(usize::from).ok_or(BvhError::NoArms);
            for (bone, end) in [(joint(upper)?, joint(lower)?), (joint(lower)?, joint(hand)?)] {
                let model = self.model(locals);
                let along = model[end].translation - model[bone].translation;
                let length = along.length();
                if length <= 0.0 {
                    continue;
                }
                let raise = det::quat_from_rotation_arc(along / length, out);
                let parent = self.parsed.nodes[bone]
                    .parent
                    .map_or(Quat::IDENTITY, |parent| model[usize::from(parent)].rotation);
                // The bone's model rotation turned by `raise`, back in its parent's frame.
                let turned = det::quat_mul(raise, model[bone].rotation);
                locals[bone].rotation =
                    det::quat_normalize(det::quat_mul(det::quat_conjugate(parent), turned));
            }
        }
        Ok(())
    }

    /// The capture as a clip at `settings`' rate, its root motion lifted as they say.
    pub fn sample(&self, settings: &ClipSettings) -> Result<SampledClip, ClipImportError> {
        let frames = self.frames();
        let seconds = (frames - 1) as f64 * self.parsed.frame_time;
        let rate = f64::from(settings.rate);
        // A hair over a whole tick is the file's rounding of its frame time, not a tick more.
        let ticks = (seconds * rate - 1e-3).ceil().max(0.0);
        if ticks > f64::from(MAX_TICKS) {
            return Err(BvhError::TooLong { seconds, ticks, rate: settings.rate }.into());
        }
        let ticks = ticks as u16;
        let count = usize::from(ticks) + 1;
        let joints = self.joints.len();
        // A rotation and a translation, each joint, each tick.
        let found = count.saturating_mul(joints).saturating_mul(7);
        if found > CLIP_BUDGET.samples {
            return Err(BvhError::TooManySamples { found, budget: CLIP_BUDGET.samples }.into());
        }
        let mut rotations = vec![Vec::with_capacity(count); joints];
        let mut translations = vec![Vec::with_capacity(count); joints];
        let last = frames - 1;
        for tick in 0..count {
            let at = tick as f64 / rate / self.parsed.frame_time;
            let frame = (at.floor() as usize).min(last);
            let (next, blend) =
                if frame == last { (last, 0.0) } else { (frame + 1, (at - frame as f64) as f32) };
            for joint in 0..joints {
                let (from, to) = (self.local(joint, frame), self.local(joint, next));
                let rotation = det::quat_slerp(from.rotation, to.rotation, blend);
                let translation = from.translation + (to.translation - from.translation) * blend;
                if !(rotation.is_finite() && translation.is_finite()) {
                    let joint = self.joints[joint].name.clone();
                    return Err(ClipImportError::NonFiniteSample {
                        animation: self.name.clone(),
                        joint,
                        tick,
                    });
                }
                rotations[joint].push(rotation);
                translations[joint].push(translation);
            }
        }
        fn numbered<T>(tracks: Vec<Vec<T>>) -> Vec<(u16, Vec<T>)> {
            tracks.into_iter().enumerate().map(|(joint, track)| (joint as u16, track)).collect()
        }
        let mut clip = SampledClip {
            name: self.name.clone(),
            rate: settings.rate,
            ticks,
            rotations: numbered(rotations),
            translations: numbered(translations),
            scales: Vec::new(),
            weights: Vec::new(),
            root: None,
        };
        lift_settings_root(&mut clip, &self.joints, settings)?;
        Ok(clip)
    }
}

#[cfg(test)]
pub(crate) mod tests;
