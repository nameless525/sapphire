//! A timeline document's keys as the sequencer edits them (M6 T16a): its lanes, a key by its place in its lane, and
//! what dragging, adding and removing one writes, worked out on the document's value and checked before a byte of
//! its text moves.
//!
//! A lane is one list of the document's keys: the camera's, its focus, a post parameter's, a character's places,
//! plays, faces and looks, the cues and the subtitles. A key is dragged to whole frames of
//! [`RATE`], as the playhead goes, and the drag writes its time alone (a span's two ends, moved
//! together). A key keeps its place in its lane: a drag onto or past a neighbour is refused, so the list stays in
//! the order the document reads it in, and the key stays where it last could go. Cues may share a time, so one may
//! land on its neighbour's, but not past it; a span may touch its neighbour, never overlap it.
//!
//! **Adding at the playhead.** A lane that interpolates (the camera, focus, post, places and looks) adds the value
//! it holds at that time, as [`sapphire_sequencer::TimelinePlayer`] samples it, so the shot at the playhead doesn't
//! change until the key is edited; the key arrives by the ease of the key after it, but a cut arrives as a line,
//! since the value held up to a cut is the one before it, and a cut here would throw the picture's history away.
//! Two focus keys around the playhead that aim by distance, or at points, give the plane between them; one that
//! aims at a role, or two kinds of aim, give the nearer's aim, which is the one approximation. Lanes of named things take the name and their defaults: a face its VRM 1.0 preset at
//! full weight and [`DEFAULT_FACE_BLEND`], a cue its row of the cue table at the timeline's origin, a play its row
//! of the graph and a subtitle its Fluent key, each of the two last for up to [`DEFAULT_SPAN`] seconds and no
//! further than the next one's start or the timeline's end. Focus where the shot has none is the lane's default
//! ([`default_focus`]), since sharp everywhere is no focus to hold, and a place where a character has none is the
//! timeline's origin.
//!
//! Every edit is checked as the document's own [`TimelineDocument::check`] checks it, and refused with what it says,
//! so the sequencer never writes a timeline that doesn't cook; one that doesn't check already takes no key edits
//! until Properties mends it. Which Fluent keys the string tables hold is the cook's to say (spec §5.17): it refuses
//! a subtitle naming one they don't, by its field.

use bevy_reflect::PartialReflect;
use sapphire_assets::rig::ExpressionPreset;
use sapphire_timeline::{
    CameraKey, CueKey, DEFAULT_FACE_BLEND, Ease, FaceKey, FocusKey, FocusOn, LookKey, PlaceKey, PlayRange,
    PostParameter, ScalarKey, SubtitleKey, TimelineDocument,
};
use serde::{Deserialize, Serialize};

use crate::app::RATE;

/// How long a play or a subtitle added at the playhead lasts, at most, in seconds.
pub const DEFAULT_SPAN: f32 = 2.0;

/// One list of a timeline's keys, as the sequencer shows it in a row.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Lane {
    /// The camera's keys.
    Camera,
    /// The camera's keys that say what the shot keeps sharp: focus is a camera key's, so dragging one drags its
    /// camera key, and removing one takes its focus away.
    Focus,
    /// A post parameter's track.
    Post(PostParameter),
    /// Where the character of this role stands.
    Places(String),
    /// The rows of its graph it plays, as spans.
    Plays(String),
    /// The expressions its face shows.
    Faces(String),
    /// Where it looks.
    Looks(String),
    Cues,
    /// As spans.
    Subtitles,
}

/// A key, by its lane and its place in the lane's list: for [`Lane::Focus`], its place among the camera's keys.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct KeyRef {
    pub lane: Lane,
    pub index: usize,
}

/// Where a key is in time, in seconds.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum When {
    At(f32),
    /// From, to.
    Span(f32, f32),
}

impl When {
    /// Where it starts.
    #[must_use]
    pub const fn start(self) -> f32 {
        match self {
            Self::At(at) | Self::Span(at, _) => at,
        }
    }
}

/// A frame of [`RATE`] in seconds, as a key's time is written.
#[must_use]
pub fn seconds_of(frame: u32) -> f32 {
    frame as f32 / RATE
}

/// The whole frame of [`RATE`] nearest `seconds`.
#[must_use]
pub fn frame_of(seconds: f32) -> u32 {
    (seconds * RATE).round().max(0.0) as u32
}

impl Lane {
    /// Every lane `document` has, in the order the sequencer shows them: the camera's and its focus if it has a
    /// camera, each post track, each character's four, then the cues and the subtitles, which are there to add to
    /// even while they're empty.
    #[must_use]
    pub fn of(document: &TimelineDocument) -> Vec<Self> {
        let mut lanes = Vec::new();
        if document.camera.is_some() {
            lanes.extend([Self::Camera, Self::Focus]);
        }
        lanes.extend(document.post.iter().map(|track| Self::Post(track.parameter)));
        for track in &document.characters {
            let role = &track.role;
            lanes.extend([
                Self::Places(role.clone()),
                Self::Plays(role.clone()),
                Self::Faces(role.clone()),
                Self::Looks(role.clone()),
            ]);
        }
        lanes.extend([Self::Cues, Self::Subtitles]);
        lanes
    }

    /// What its row is called.
    #[must_use]
    pub fn label(&self) -> String {
        match self {
            Self::Camera => "Camera".to_owned(),
            Self::Focus => "Focus".to_owned(),
            Self::Post(parameter) => format!("Post: {parameter:?}"),
            Self::Places(role) => format!("{role}: places"),
            Self::Plays(role) => format!("{role}: plays"),
            Self::Faces(role) => format!("{role}: faces"),
            Self::Looks(role) => format!("{role}: looks"),
            Self::Cues => "Cues".to_owned(),
            Self::Subtitles => "Subtitles".to_owned(),
        }
    }

    /// What a key added to it is named by, if it takes a name: a face's preset, a cue's row, a play's row or a
    /// subtitle's Fluent key.
    #[must_use]
    pub const fn takes(&self) -> Option<&'static str> {
        match self {
            Self::Faces(_) => Some("a VRM 1.0 expression preset"),
            Self::Cues => Some("a row of the cue table"),
            Self::Plays(_) => Some("a row of the character's graph"),
            Self::Subtitles => Some("a Fluent key"),
            _ => None,
        }
    }

    /// Whether its keys are spans.
    const fn spans(&self) -> bool {
        matches!(self, Self::Plays(_) | Self::Subtitles)
    }

    /// Whether two of its keys may share a time.
    const fn ties(&self) -> bool {
        matches!(self, Self::Cues)
    }

    /// The character track of `role`'s place.
    fn character(document: &TimelineDocument, role: &str) -> Result<usize, String> {
        document
            .characters
            .iter()
            .position(|track| track.role == role)
            .ok_or_else(|| format!("the timeline has no character `{role}`"))
    }

    /// The property path of its list.
    fn list(&self, document: &TimelineDocument) -> Result<String, String> {
        Ok(match self {
            Self::Camera | Self::Focus => {
                if document.camera.is_none() {
                    return Err("the timeline has no camera track: add one in Properties".to_owned());
                }
                "camera?.keys".to_owned()
            }
            Self::Post(parameter) => {
                let Some(index) = document.post.iter().position(|track| track.parameter == *parameter) else {
                    return Err(format!("the timeline has no {parameter:?} track: add one in Properties"));
                };
                format!("post[{index}].keys")
            }
            Self::Places(role) => format!("characters[{}].places", Self::character(document, role)?),
            Self::Plays(role) => format!("characters[{}].plays", Self::character(document, role)?),
            Self::Faces(role) => format!("characters[{}].faces", Self::character(document, role)?),
            Self::Looks(role) => format!("characters[{}].looks", Self::character(document, role)?),
            Self::Cues => "cues".to_owned(),
            Self::Subtitles => "subtitles".to_owned(),
        })
    }

    /// Every key of its list, in the list's order, with where it is: for [`Self::Focus`], every camera key, and
    /// whether it has focus.
    fn all(&self, document: &TimelineDocument) -> Vec<(When, bool)> {
        let at = |at: f32| (When::At(at), true);
        let role = |role: &str| document.characters.iter().find(|track| track.role == role);
        match self {
            Self::Camera | Self::Focus => document
                .camera
                .iter()
                .flat_map(|camera| &camera.keys)
                .map(|key| {
                    let shown = !matches!(self, Self::Focus) || key.focus.is_some();
                    (When::At(key.at), shown)
                })
                .collect(),
            Self::Post(parameter) => document
                .post
                .iter()
                .filter(|track| track.parameter == *parameter)
                .flat_map(|track| &track.keys)
                .map(|key| at(key.at))
                .collect(),
            Self::Places(name) => {
                role(name).iter().flat_map(|track| &track.places).map(|key| at(key.at)).collect()
            }
            Self::Plays(name) => role(name)
                .iter()
                .flat_map(|track| &track.plays)
                .map(|play| (When::Span(play.from, play.to), true))
                .collect(),
            Self::Faces(name) => {
                role(name).iter().flat_map(|track| &track.faces).map(|key| at(key.at)).collect()
            }
            Self::Looks(name) => {
                role(name).iter().flat_map(|track| &track.looks).map(|key| at(key.at)).collect()
            }
            Self::Cues => document.cues.iter().map(|key| at(key.at)).collect(),
            Self::Subtitles => {
                document.subtitles.iter().map(|line| (When::Span(line.from, line.to), true)).collect()
            }
        }
    }

    /// The keys it shows, by their places in its list.
    #[must_use]
    pub fn keys(&self, document: &TimelineDocument) -> Vec<(usize, When)> {
        let all = self.all(document);
        all.into_iter()
            .enumerate()
            .filter(|(_, (_, shown))| *shown)
            .map(|(index, (when, _))| (index, when))
            .collect()
    }
}

/// One write of an edit, by property path.
#[derive(Debug)]
pub enum Write {
    /// A number, spelled as it goes in the file: a key's time.
    Literal {
        path: String,
        literal: String,
    },
    /// A whole entry into a list, at its place.
    Insert {
        list: String,
        at: usize,
        value: Box<dyn PartialReflect>,
    },
    /// A whole value.
    Set {
        path: String,
        value: Box<dyn PartialReflect>,
    },
    Remove {
        path: String,
    },
}

/// An edit worked out: its writes, in order, and the value they leave, which has checked clean.
#[derive(Debug)]
pub struct Planned {
    pub writes: Vec<Write>,
    pub after: TimelineDocument,
}

/// A time as the file spells it: the shortest that reads back as the same `f32`, with its point.
fn spelled(seconds: f32) -> String {
    format!("{seconds:?}")
}

/// The edit, if the value it leaves checks: what the sequencer writes cooks.
fn checked(writes: Vec<Write>, after: TimelineDocument) -> Result<Planned, String> {
    after.check().map_err(|error| format!("refused, as the timeline would read: {error}"))?;
    Ok(Planned { writes, after })
}

/// That the timeline reads clean, so its lanes are in the order a key edit counts on.
fn ready(document: &TimelineDocument) -> Result<(), String> {
    document
        .check()
        .map_err(|error| format!("the timeline doesn't check, so its keys wait for Properties: {error}"))
}

/// The place of `role`'s character track, which [`Lane::list`] found.
fn role_at(document: &TimelineDocument, role: &str) -> usize {
    document.characters.iter().position(|track| track.role == role).expect("its lane's list was found")
}

/// Moves `key` to start at `frame`: its time, or a span's two ends by the same frames. Refused onto or past a
/// neighbour (see the [module docs](self)), past the timeline's end, or where it is already.
pub fn moved(document: &TimelineDocument, key: &KeyRef, frame: u32) -> Result<Planned, String> {
    ready(document)?;
    let list = key.lane.list(document)?;
    let all = key.lane.all(document);
    let Some(&(when, shown)) = all.get(key.index) else {
        return Err(format!("{} has no key {}", key.lane.label(), key.index));
    };
    if !shown {
        return Err(format!("camera key {} has no focus", key.index));
    }
    let before = key.index.checked_sub(1).and_then(|index| all.get(index)).map(|(when, _)| *when);
    let after = all.get(key.index + 1).map(|(when, _)| *when);
    let to = seconds_of(frame);
    let moved = match when {
        When::At(_) => When::At(to),
        When::Span(from, end) => {
            let by = i64::from(frame) - i64::from(frame_of(from));
            let end = u32::try_from(i64::from(frame_of(end)) + by).map_err(|_| "it would end before 0")?;
            When::Span(to, seconds_of(end))
        }
    };
    if moved == when {
        return Err("the key is at that frame already".to_owned());
    }
    let end = match moved {
        When::At(at) | When::Span(_, at) => at,
    };
    if end > document.length {
        return Err(format!("it would go past the timeline's end, frame {}", frame_of(document.length)));
    }
    let neighbour = |which: &str, at: f32| {
        format!(
            "frame {frame} is on or past the key {which} it, at frame {}: a key keeps its place in its lane",
            frame_of(at)
        )
    };
    match (moved, before, after) {
        (When::At(at), Some(When::At(before)), _) if at < before || (at == before && !key.lane.ties()) => {
            return Err(neighbour("before", before));
        }
        (When::At(at), _, Some(When::At(after))) if at > after || (at == after && !key.lane.ties()) => {
            return Err(neighbour("after", after));
        }
        (When::Span(from, _), Some(When::Span(_, before)), _) if from < before => {
            return Err(neighbour("before", before));
        }
        (When::Span(_, end), _, Some(When::Span(after, _))) if end > after => {
            return Err(neighbour("after", after));
        }
        _ => {}
    }
    let mut value = document.clone();
    set_time(&mut value, &key.lane, key.index, moved);
    let path = format!("{list}[{}]", key.index);
    let writes = match moved {
        When::At(at) => vec![Write::Literal { path: format!("{path}.at"), literal: spelled(at) }],
        When::Span(from, to) => vec![
            Write::Literal { path: format!("{path}.from"), literal: spelled(from) },
            Write::Literal { path: format!("{path}.to"), literal: spelled(to) },
        ],
    };
    checked(writes, value)
}

/// Sets the time of `lane`'s key `index` in `document`'s value, as its writes do in the text.
fn set_time(document: &mut TimelineDocument, lane: &Lane, index: usize, when: When) {
    let (at, end) = match when {
        When::At(at) => (at, at),
        When::Span(from, to) => (from, to),
    };
    match lane {
        Lane::Camera | Lane::Focus => {
            document.camera.as_mut().expect("its lane's list was found").keys[index].at = at;
        }
        Lane::Post(parameter) => {
            let track = document.post.iter_mut().find(|track| track.parameter == *parameter);
            track.expect("its lane's list was found").keys[index].at = at;
        }
        Lane::Places(role) => {
            let track = role_at(document, role);
            document.characters[track].places[index].at = at;
        }
        Lane::Plays(role) => {
            let track = role_at(document, role);
            let play = &mut document.characters[track].plays[index];
            (play.from, play.to) = (at, end);
        }
        Lane::Faces(role) => {
            let track = role_at(document, role);
            document.characters[track].faces[index].at = at;
        }
        Lane::Looks(role) => {
            let track = role_at(document, role);
            document.characters[track].looks[index].at = at;
        }
        Lane::Cues => document.cues[index].at = at,
        Lane::Subtitles => {
            let line = &mut document.subtitles[index];
            (line.from, line.to) = (at, end);
        }
    }
}

/// Takes `key` out of its lane: from [`Lane::Focus`], its camera key's focus.
pub fn removed(document: &TimelineDocument, key: &KeyRef) -> Result<Planned, String> {
    ready(document)?;
    let list = key.lane.list(document)?;
    match key.lane.all(document).get(key.index) {
        Some((_, true)) => {}
        Some((_, false)) => return Err(format!("camera key {} has no focus", key.index)),
        None => return Err(format!("{} has no key {}", key.lane.label(), key.index)),
    }
    let mut value = document.clone();
    let path = format!("{list}[{}]", key.index);
    let write = if key.lane == Lane::Focus {
        value.camera.as_mut().expect("its lane's list was found").keys[key.index].focus = None;
        Write::Set { path: format!("{path}.focus"), value: Box::new(None::<FocusKey>) }
    } else {
        remove_key(&mut value, &key.lane, key.index);
        Write::Remove { path }
    };
    checked(vec![write], value)
}

fn remove_key(document: &mut TimelineDocument, lane: &Lane, index: usize) {
    match lane {
        Lane::Camera | Lane::Focus => {
            document.camera.as_mut().expect("its lane's list was found").keys.remove(index);
        }
        Lane::Post(parameter) => {
            let track = document.post.iter_mut().find(|track| track.parameter == *parameter);
            track.expect("its lane's list was found").keys.remove(index);
        }
        Lane::Places(role) => {
            let track = role_at(document, role);
            document.characters[track].places.remove(index);
        }
        Lane::Plays(role) => {
            let track = role_at(document, role);
            document.characters[track].plays.remove(index);
        }
        Lane::Faces(role) => {
            let track = role_at(document, role);
            document.characters[track].faces.remove(index);
        }
        Lane::Looks(role) => {
            let track = role_at(document, role);
            document.characters[track].looks.remove(index);
        }
        Lane::Cues => {
            document.cues.remove(index);
        }
        Lane::Subtitles => {
            document.subtitles.remove(index);
        }
    }
}

/// The focus a key with none gets in [`Lane::Focus`]: sharp at the key's look-at, 0.3 m either side, softening over
/// 1 m in front and 6 m behind to 1% of the picture's height, as the document's own example keys it.
#[must_use]
pub fn default_focus(key: &CameraKey) -> FocusKey {
    let distance: f32 = (0..3).map(|axis| (key.look_at[axis] - key.eye[axis]).powi(2)).sum::<f32>().sqrt();
    FocusKey { on: FocusOn::Distance(distance), sharp: 0.3, near: 1.0, far: 6.0, blur: 0.01 }
}

/// Adds a key to `lane` at `frame`, named `name` if the lane's keys are named (see the [module docs](self)).
pub fn added(
    document: &TimelineDocument,
    lane: &Lane,
    frame: u32,
    name: Option<&str>,
) -> Result<Planned, String> {
    ready(document)?;
    let list = lane.list(document)?;
    let name = match (lane.takes(), name) {
        (Some(what), None) => return Err(format!("{} takes {what}", lane.label())),
        (None, Some(_)) => return Err(format!("{} takes no name: it adds the value it holds", lane.label())),
        (Some(_), Some(name)) => Some(name.to_owned()),
        (None, None) => None,
    };
    let at = seconds_of(frame);
    if at > document.length {
        return Err(format!("frame {frame} is past the timeline's end, frame {}", frame_of(document.length)));
    }
    let all = lane.all(document);
    // Where it goes in its list: after every key before it, and after those at its time where they may tie.
    let place = all.partition_point(|(when, _)| {
        let start = when.start();
        if lane.ties() { start <= at } else { start < at }
    });
    if !lane.ties() && !lane.spans() && all.get(place).is_some_and(|(when, _)| when.start() == at) {
        if *lane == Lane::Focus {
            return add_focus_at_key(document, &list, place);
        }
        return Err(format!("{} has a key at frame {frame} already", lane.label()));
    }
    let next = all.get(place).map(|(when, _)| when.start());
    let mut value = document.clone();
    let entry: Box<dyn PartialReflect> = match lane {
        Lane::Camera | Lane::Focus => {
            let camera = value.camera.as_mut().expect("its lane's list was found");
            let mut key = held_camera(&camera.keys, at, place);
            if *lane == Lane::Focus && key.focus.is_none() {
                key.focus = Some(default_focus(&key));
            }
            camera.keys.insert(place, key.clone());
            Box::new(key)
        }
        Lane::Post(parameter) => {
            let track = value
                .post
                .iter_mut()
                .find(|track| track.parameter == *parameter)
                .expect("its lane's list was found");
            let (from, to, share) = between(&track.keys, at, |key| key.at, |key| key.ease);
            let (from, to) = (&track.keys[from], &track.keys[to]);
            let key = ScalarKey {
                at,
                value: lerp(from.value, to.value, share),
                ease: arriving(&track.keys, place),
            };
            track.keys.insert(place, key);
            Box::new(key)
        }
        Lane::Places(role) => {
            let track = &mut value.characters[role_at(document, role)];
            let key = if track.places.is_empty() {
                PlaceKey { at, position: [0.0; 3], yaw_degrees: 0.0, ease: Ease::Linear }
            } else {
                let (from, to, share) = between(&track.places, at, |key| key.at, |key| key.ease);
                let (from, to) = (&track.places[from], &track.places[to]);
                PlaceKey {
                    at,
                    position: lerp3(from.position, to.position, share),
                    yaw_degrees: lerp(from.yaw_degrees, to.yaw_degrees, share),
                    ease: arriving(&track.places, place),
                }
            };
            track.places.insert(place, key);
            Box::new(key)
        }
        Lane::Looks(role) => {
            let track = &mut value.characters[role_at(document, role)];
            let key =
                LookKey { at, look_at: held_look(&track.looks, at), ease: arriving(&track.looks, place) };
            track.looks.insert(place, key);
            Box::new(key)
        }
        Lane::Faces(role) => {
            let expression = name.expect("named above");
            if ExpressionPreset::from_name(&expression).is_none() {
                return Err(format!(
                    "{expression:?} is no VRM 1.0 expression preset: {}",
                    ExpressionPreset::ALL.iter().map(|preset| preset.name()).collect::<Vec<_>>().join(", ")
                ));
            }
            let key = FaceKey { at, expression, weight: 1.0, blend: DEFAULT_FACE_BLEND };
            let track = &mut value.characters[role_at(document, role)];
            track.faces.insert(place, key.clone());
            Box::new(key)
        }
        Lane::Cues => {
            let key = CueKey { at, cue: name.expect("named above"), place: None };
            value.cues.insert(place, key.clone());
            Box::new(key)
        }
        Lane::Plays(_) | Lane::Subtitles => {
            let before_ends =
                place.checked_sub(1).and_then(|index| all.get(index)).map(|(when, _)| match when {
                    When::Span(_, end) => *end,
                    When::At(at) => *at,
                });
            if before_ends.is_some_and(|end| at < end) {
                return Err(format!("frame {frame} is inside the one before it: spans don't overlap"));
            }
            let end = (at + DEFAULT_SPAN).min(next.unwrap_or(f32::INFINITY)).min(document.length);
            // On a whole frame at or before it, as a drag leaves it.
            let mut whole = frame_of(end);
            if seconds_of(whole) > end {
                whole -= 1;
            }
            let end = seconds_of(whole);
            if end <= at {
                return Err(format!("no room at frame {frame}: the next one starts there"));
            }
            let name = name.expect("named above");
            if let Lane::Plays(role) = lane {
                let play = PlayRange { row: name, from: at, to: end };
                value.characters[role_at(document, role)].plays.insert(place, play.clone());
                Box::new(play)
            } else {
                let line = SubtitleKey { from: at, to: end, key: name, speaker: None };
                value.subtitles.insert(place, line.clone());
                Box::new(line)
            }
        }
    };
    checked(vec![Write::Insert { list, at: place, value: entry }], value)
}

/// Focus for the camera key at `index`, which has none yet: the lane's default.
fn add_focus_at_key(document: &TimelineDocument, list: &str, index: usize) -> Result<Planned, String> {
    let mut value = document.clone();
    let key = &mut value.camera.as_mut().expect("a camera").keys[index];
    if key.focus.is_some() {
        return Err(format!("camera key {index} has focus already"));
    }
    let focus = default_focus(key);
    key.focus = Some(focus.clone());
    checked(vec![Write::Set { path: format!("{list}[{index}].focus"), value: Box::new(Some(focus)) }], value)
}

/// How a key added at `place` of `keys` arrives: by the ease of the key after it, which the stretch it splits had,
/// but a cut as a line, since up to a cut the value is the key before's, which is what the new key holds.
fn arriving<K: Eased>(keys: &[K], place: usize) -> Ease {
    match keys.get(place) {
        Some(next) if place > 0 && next.ease() != Ease::Cut => next.ease(),
        _ => Ease::Linear,
    }
}

/// A key that says how it's arrived at.
trait Eased {
    fn ease(&self) -> Ease;
}

impl Eased for CameraKey {
    fn ease(&self) -> Ease {
        self.ease
    }
}

impl Eased for ScalarKey {
    fn ease(&self) -> Ease {
        self.ease
    }
}

impl Eased for PlaceKey {
    fn ease(&self) -> Ease {
        self.ease
    }
}

impl Eased for LookKey {
    fn ease(&self) -> Ease {
        self.ease
    }
}

// What a lane holds at a time, as `sapphire_sequencer`'s player samples it (`player.rs`'s `between`, `lerp` and
// `sample_*`, and `focus.rs`'s `ShotFocus::between`): the same arithmetic, so a key added holds the same value.

/// The keys around `at`, by their places, and how far from the first to the second, eased: past the last, that key
/// twice; before the first, the first.
fn between<K>(
    keys: &[K],
    at: f32,
    time: impl Fn(&K) -> f32,
    ease: impl Fn(&K) -> Ease,
) -> (usize, usize, f32) {
    let index = keys.partition_point(|key| time(key) <= at).saturating_sub(1);
    let Some(to) = keys.get(index + 1) else { return (index, index, 0.0) };
    let share = ((at - time(&keys[index])) / (time(to) - time(&keys[index]))).clamp(0.0, 1.0);
    (index, index + 1, ease(to).eased(share))
}

fn lerp(from: f32, to: f32, share: f32) -> f32 {
    from + (to - from) * share
}

fn lerp3(from: [f32; 3], to: [f32; 3], share: f32) -> [f32; 3] {
    [lerp(from[0], to[0], share), lerp(from[1], to[1], share), lerp(from[2], to[2], share)]
}

/// The camera key the track holds at `at`, to go at `place`: its shot and its focus.
fn held_camera(keys: &[CameraKey], at: f32, place: usize) -> CameraKey {
    let (from, to, share) = between(keys, at, |key| key.at, |key| key.ease);
    let (from, to) = (&keys[from], &keys[to]);
    CameraKey {
        at,
        eye: lerp3(from.eye, to.eye, share),
        look_at: lerp3(from.look_at, to.look_at, share),
        roll_degrees: lerp(from.roll_degrees, to.roll_degrees, share),
        fov_degrees: lerp(from.fov_degrees, to.fov_degrees, share),
        ease: arriving(keys, place),
        focus: held_focus(from.focus.as_ref(), to.focus.as_ref(), share),
    }
}

/// The focus between two camera keys `share` of the way, eased: the side without focus is the other's target and
/// ranges with no blur. Two distances or two points hold the plane between; other pairs aim at the nearer's; a blur
/// that comes to nothing is no focus.
fn held_focus(from: Option<&FocusKey>, to: Option<&FocusKey>, share: f32) -> Option<FocusKey> {
    let none = |key: &FocusKey| FocusKey { blur: 0.0, ..key.clone() };
    let (a, b) = match (from, to) {
        (None, None) => return None,
        (Some(a), Some(b)) => (a.clone(), b.clone()),
        (Some(a), None) => (a.clone(), none(a)),
        (None, Some(b)) => (none(b), b.clone()),
    };
    // Two distances, or two points, hold the plane between them; a role, or two kinds of aim, the nearer's.
    let on = match (&a.on, &b.on) {
        (FocusOn::Distance(from), FocusOn::Distance(to)) => FocusOn::Distance(lerp(*from, *to, share)),
        (FocusOn::Point(from), FocusOn::Point(to)) => FocusOn::Point(lerp3(*from, *to, share)),
        _ if share < 0.5 => a.on.clone(),
        _ => b.on.clone(),
    };
    let focus = FocusKey {
        on,
        sharp: lerp(a.sharp, b.sharp, share),
        near: lerp(a.near, b.near, share),
        far: lerp(a.far, b.far, share),
        blur: lerp(a.blur, b.blur, share),
    };
    (focus.blur > 0.0).then_some(focus)
}

/// Where a look track looks at `at`: nothing before its first key, or from a key that hands the look back; between
/// two that look somewhere, by the second's ease; up to one that hands back, where the one before looks.
fn held_look(keys: &[LookKey], at: f32) -> Option<[f32; 3]> {
    let index = keys.partition_point(|key| key.at <= at).checked_sub(1)?;
    let from = keys[index].look_at?;
    let Some(next) = keys.get(index + 1) else { return Some(from) };
    let Some(to) = next.look_at else { return Some(from) };
    let share = ((at - keys[index].at) / (next.at - keys[index].at)).clamp(0.0, 1.0);
    Some(lerp3(from, to, next.ease.eased(share)))
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use sapphire_core::Name;
    use sapphire_look::PostParams;
    use sapphire_math::Vec3;
    use sapphire_sequencer::{Origin, TimelinePlayer};

    use super::*;

    const CUTSCENE: &str =
        include_str!("../../../crates/sapphire_timeline/test_content/cutscene.timeline.ron");

    fn cutscene() -> TimelineDocument {
        TimelineDocument::from_ron(CUTSCENE).expect("it reads")
    }

    /// The test cutscene, its first two camera keys focused on the hero by different ranges.
    fn focused() -> TimelineDocument {
        let text = CUTSCENE
            .replacen(
                "fov_degrees: 35.0)",
                r#"fov_degrees: 35.0, focus: Some((on: Role("hero"), sharp: 0.3, near: 1.0, far: 6.0, blur: 0.01)))"#,
                1,
            )
            .replacen(
                "fov_degrees: 30.0, ease: Smooth)",
                r#"fov_degrees: 30.0, ease: Smooth, focus: Some((on: Role("hero"), sharp: 0.5, near: 2.0, far: 4.0, blur: 0.02)))"#,
                1,
            );
        TimelineDocument::from_ron(&text).expect("it reads")
    }

    /// What every lane that holds a value shows at `frame`, by a player's one advance there, so its playhead is the
    /// frame's time exactly: the shot (with its focus's target and ranges), post, and the hero's place and look.
    fn shown(document: &TimelineDocument, frame: u32) -> String {
        let mut player = TimelinePlayer::new(Arc::new(document.cook().expect("it cooks")), Origin::default());
        player.advance(seconds_of(frame), None);
        let shot = player.shot().expect("a shot");
        // The plane it focuses on, from both aims at its share, to a hundredth of a millimetre, a role's head where
        // the test's characters stand, and the ranges and blur exactly.
        let forward = shot.rotation * Vec3::NEG_Z;
        let head = |_| Some(Vec3::new(0.0, 1.5, 0.0));
        let focus = shot.focus.map(|focus| {
            let from = focus.from.distance(shot.eye, forward, &head).expect("a plane");
            let to = focus.to.distance(shot.eye, forward, &head).expect("a plane");
            (
                format!("{:.5}", from + (to - from) * focus.share),
                focus.sharp,
                focus.near,
                focus.far,
                focus.blur,
            )
        });
        let mut post = PostParams::default();
        player.apply_post(&mut post);
        let hero = player.character(Name::new("hero")).expect("the hero");
        format!(
            "{:?} {:?} {} {focus:?} {} {} {:?} {:?}",
            shot.eye,
            shot.rotation,
            shot.fov_y,
            post.exposure,
            post.vignette.intensity,
            hero.placed,
            hero.look
        )
    }

    /// Whether the shot at `frame` has focus.
    fn focus_shown(document: &TimelineDocument, frame: u32) -> bool {
        let mut player = TimelinePlayer::new(Arc::new(document.cook().expect("it cooks")), Origin::default());
        player.advance(seconds_of(frame), None);
        player.shot().expect("a shot").focus.is_some()
    }

    /// Adding a key to a lane that holds a value adds the value it holds: what the player shows at the playhead is
    /// the same to the bit, between linear and smooth keys, up to a cut, after the last and before the first.
    #[test]
    fn a_key_added_holds_what_its_lane_shows_at_the_playhead() {
        let document = focused();
        let lanes = [
            Lane::Camera,
            Lane::Focus,
            Lane::Post(PostParameter::Exposure),
            Lane::Post(PostParameter::Vignette),
            Lane::Places("hero".to_owned()),
            Lane::Looks("hero".to_owned()),
        ];
        for frame in [40, 100, 170, 290, 350] {
            for lane in &lanes {
                let planned = added(&document, lane, frame, None)
                    .unwrap_or_else(|why| panic!("{lane:?} takes a key at frame {frame}: {why}"));
                assert_eq!(planned.writes.len(), 1);
                if *lane == Lane::Focus && !focus_shown(&document, frame) {
                    // Where the shot has no focus there's none to hold: the key takes the lane's default.
                    let camera = planned.after.camera.as_ref().expect("a camera");
                    let key =
                        camera.keys.iter().find(|key| key.at == seconds_of(frame)).expect("the key added");
                    assert_eq!(key.focus, Some(default_focus(key)), "focus at frame {frame}");
                    continue;
                }
                assert_eq!(
                    shown(&planned.after, frame),
                    shown(&document, frame),
                    "{lane:?} at frame {frame}"
                );
            }
        }
        // Before the hero's first look, at 0.5 s: nothing, as a key that hands the look back says.
        let planned = added(&document, &Lane::Looks("hero".to_owned()), 12, None).expect("a look");
        assert_eq!(
            planned.after.characters[0].looks[0],
            LookKey { at: 0.2, look_at: None, ease: Ease::Linear }
        );
        assert_eq!(shown(&planned.after, 12), shown(&document, 12));
    }

    /// The test cutscene with its first two camera keys focused by `first` and `second`.
    fn aimed(first: &str, second: &str) -> TimelineDocument {
        let text = CUTSCENE
            .replacen(
                "fov_degrees: 35.0)",
                &format!("fov_degrees: 35.0, focus: Some((on: {first}, sharp: 0.3, near: 1.0, far: 6.0, blur: 0.01)))"),
                1,
            )
            .replacen(
                "fov_degrees: 30.0, ease: Smooth)",
                &format!(
                    "fov_degrees: 30.0, ease: Smooth, focus: Some((on: {second}, sharp: 0.5, near: 2.0, far: 4.0, blur: 0.02)))"
                ),
                1,
            );
        TimelineDocument::from_ron(&text).expect("it reads")
    }

    /// Two distances and two points hold the plane between them, so the focus shown is the same: the whole of it,
    /// its plane, ranges and blur. Aims of two kinds give the nearer's aim, the one approximation: its ranges and
    /// blur held, its plane the nearer key's.
    #[test]
    fn a_focus_added_between_two_aims_holds_their_plane_or_takes_the_nearers() {
        for (first, second) in
            [("Distance(2.0)", "Distance(5.0)"), ("Point((0.0, 1.0, 0.0))", "Point((1.0, 1.5, 2.0))")]
        {
            let document = aimed(first, second);
            for frame in [40, 100] {
                let planned = added(&document, &Lane::Focus, frame, None).expect("a focus");
                assert_eq!(
                    shown(&planned.after, frame),
                    shown(&document, frame),
                    "{first} to {second}, frame {frame}"
                );
            }
        }
        let document = aimed("Distance(2.0)", r#"Role("hero")"#);
        let nearer = |frame: u32| {
            let planned = added(&document, &Lane::Focus, frame, None).expect("a focus");
            let camera = planned.after.camera.expect("a camera");
            let key = camera.keys.into_iter().find(|key| key.at == seconds_of(frame)).expect("the key added");
            key.focus.expect("focus").on
        };
        // The keys are at 0 and 2.5 s, the second arriving smooth: 0.67 s in is short of half, 1.67 s past it.
        assert_eq!(nearer(40), FocusOn::Distance(2.0));
        assert_eq!(nearer(100), FocusOn::Role("hero".to_owned()));
    }

    /// A key is at its frame's time, the nearest f32 to `frame / 60`. The pane plays a timeline a frame of 1/60 s at
    /// a time, an f32 sum that can fall short of it: at frame 45 it's 0.74999994 s, so a cue dragged there fires
    /// a frame later in the pane, and at its own time, as one step there plays it, on the frame.
    #[test]
    fn a_key_is_at_its_frames_time_and_the_panes_summed_frames_can_reach_it_a_frame_late() {
        let key = KeyRef { lane: Lane::Cues, index: 0 };
        let planned = moved(&cutscene(), &key, 45).expect("a frame of its own");
        assert_eq!(planned.after.cues[0].at, 0.75);
        let timeline = Arc::new(planned.after.cook().expect("it cooks"));
        let mut summed = TimelinePlayer::new(Arc::clone(&timeline), Origin::default());
        let fired_at = (1..=60)
            .find(|_| {
                summed.advance(1.0 / RATE, None);
                summed.fired().len() > 0
            })
            .expect("it fires");
        assert_eq!(fired_at, 46, "the pane's frames sum to {} at 45", {
            let mut playhead = 0.0_f32;
            (0..45).for_each(|_| playhead += 1.0 / RATE);
            playhead
        });
        let mut stepped = TimelinePlayer::new(timeline, Origin::default());
        stepped.advance(seconds_of(45), None);
        assert_eq!(stepped.fired().len(), 1, "at its own time");
    }

    /// Up to a cut the value is the key before's, so the key added there arrives as a line and holds it: no cut of
    /// its own to throw the picture's history away. Elsewhere it arrives as the stretch it splits did.
    #[test]
    fn a_key_added_before_a_cut_holds_the_shot_before_it() {
        let document = focused();
        let planned = added(&document, &Lane::Camera, 170, None).expect("a key");
        let key = &planned.after.camera.as_ref().expect("a camera").keys[2];
        let before = &document.camera.as_ref().expect("a camera").keys[1];
        assert_eq!((key.ease, key.eye, key.fov_degrees), (Ease::Linear, before.eye, before.fov_degrees));
        let smooth = added(&document, &Lane::Camera, 100, None).expect("a key");
        assert_eq!(smooth.after.camera.as_ref().expect("a camera").keys[1].ease, Ease::Smooth);
    }

    /// Focus where the shot has none is the lane's default, on the camera key at the playhead or a new one, and a
    /// key that has focus takes no more. Removing a focus key takes its focus, not its camera key.
    #[test]
    fn focus_is_added_where_there_is_none_and_removed_from_its_key() {
        let document = cutscene();
        let on_key = added(&document, &Lane::Focus, 150, None).expect("focus on the key at 2.5 s");
        assert!(matches!(on_key.writes[..], [Write::Set { .. }]), "{:?}", on_key.writes);
        let camera = on_key.after.camera.as_ref().expect("a camera");
        assert_eq!(camera.keys.len(), 3);
        assert_eq!(camera.keys[1].focus, Some(default_focus(&camera.keys[1])));
        assert_eq!(Lane::Focus.keys(&on_key.after), [(1, When::At(2.5))]);
        let again = added(&on_key.after, &Lane::Focus, 150, None).unwrap_err();
        assert!(again.contains("has focus already"), "{again}");
        let between = added(&document, &Lane::Focus, 60, None).expect("a new key");
        assert!(matches!(between.writes[..], [Write::Insert { at: 1, .. }]), "{:?}", between.writes);

        let taken = removed(&on_key.after, &KeyRef { lane: Lane::Focus, index: 1 }).expect("removed");
        assert_eq!(taken.after, document, "the key stays, without its focus");
        let none = removed(&document, &KeyRef { lane: Lane::Focus, index: 0 }).unwrap_err();
        assert!(none.contains("has no focus"), "{none}");
    }

    /// A drag writes only the key's time, snapped to frames: a span's two ends by the same frames.
    #[test]
    fn a_drag_writes_only_the_time() {
        let document = cutscene();
        let literals = |planned: Planned| -> Vec<(String, String)> {
            planned
                .writes
                .into_iter()
                .map(|write| match write {
                    Write::Literal { path, literal } => (path, literal),
                    other => panic!("only times: {other:?}"),
                })
                .collect()
        };
        let camera = moved(&document, &KeyRef { lane: Lane::Camera, index: 1 }, 120).expect("moved");
        assert_eq!(literals(camera), [("camera?.keys[1].at".to_owned(), "2.0".to_owned())]);
        let play =
            moved(&document, &KeyRef { lane: Lane::Plays("hero".to_owned()), index: 0 }, 36).expect("moved");
        assert_eq!(
            literals(play),
            [
                ("characters[0].plays[0].from".to_owned(), "0.6".to_owned()),
                ("characters[0].plays[0].to".to_owned(), "2.1".to_owned()),
            ]
        );
        let line = moved(&document, &KeyRef { lane: Lane::Subtitles, index: 1 }, 210).expect("moved");
        assert_eq!((line.after.subtitles[1].from, line.after.subtitles[1].to), (3.5, 5.5));
    }

    /// A key keeps its place in its lane: onto or past a neighbour is refused, but a cue may land on its
    /// neighbour's time; a span may touch its neighbour, and nothing goes past the end.
    #[test]
    fn a_drag_onto_or_past_a_neighbour_is_refused() {
        let document = cutscene();
        let camera = KeyRef { lane: Lane::Camera, index: 1 };
        for frame in [0, 180, 200] {
            let why = moved(&document, &camera, frame).unwrap_err();
            assert!(why.contains("a key keeps its place in its lane"), "{why}");
        }
        assert!(moved(&document, &camera, 179).is_ok());
        assert!(moved(&document, &camera, 150).unwrap_err().contains("at that frame already"));

        let cue = KeyRef { lane: Lane::Cues, index: 1 };
        assert!(moved(&document, &cue, 60).is_ok(), "onto its neighbour's time");
        assert!(moved(&document, &cue, 59).is_err(), "past it");

        let line = KeyRef { lane: Lane::Subtitles, index: 0 };
        assert!(moved(&document, &line, 0).is_ok());
        assert!(moved(&document, &line, 61).unwrap_err().contains("a key keeps its place"));
        let last = KeyRef { lane: Lane::Subtitles, index: 1 };
        assert!(moved(&document, &last, 240).is_ok(), "to the end");
        assert!(moved(&document, &last, 241).unwrap_err().contains("past the timeline's end"));
    }

    /// Named lanes take their names: a face a VRM 1.0 preset, any of the 18, at full weight and the default blend;
    /// a subtitle its key, for up to two seconds and never into the next line or past the end; a cue its row, on
    /// another's time if it likes.
    #[test]
    fn a_named_key_takes_its_name_and_its_lanes_defaults() {
        let document = cutscene();
        let faces = Lane::Faces("hero".to_owned());
        assert_eq!(ExpressionPreset::ALL.len(), 18);
        for preset in ExpressionPreset::ALL {
            let planned = added(&document, &faces, 120, Some(preset.name())).expect("a preset");
            let key = &planned.after.characters[0].faces[1];
            assert_eq!(
                (key.at, key.expression.as_str(), key.weight, key.blend),
                (2.0, preset.name(), 1.0, DEFAULT_FACE_BLEND)
            );
        }
        let grin = added(&document, &faces, 120, Some("grin")).unwrap_err();
        assert!(grin.contains("no VRM 1.0 expression preset"), "{grin}");
        assert!(added(&document, &faces, 120, None).unwrap_err().contains("takes a VRM 1.0"));
        assert!(added(&document, &Lane::Camera, 120, Some("a")).unwrap_err().contains("takes no name"));

        let line = added(&document, &Lane::Subtitles, 330, Some("intro-third-line")).expect("a line");
        let third = SubtitleKey { from: 5.5, to: 6.0, key: "intro-third-line".to_owned(), speaker: None };
        assert_eq!(line.after.subtitles[2], third);
        let inside = added(&document, &Lane::Subtitles, 120, Some("intro-third-line")).unwrap_err();
        assert!(inside.contains("inside the one before"), "{inside}");
        let text = added(&document, &Lane::Subtitles, 330, Some("Hello!")).unwrap_err();
        assert!(text.contains("no localization key"), "{text}");
        let cue = added(&document, &Lane::Cues, 60, Some("bell")).expect("a cue on another's time");
        assert_eq!(cue.after.cues[1], CueKey { at: 1.0, cue: "bell".to_owned(), place: None });
    }

    /// Removing a key is refused where the document's own check would be (a post track's last key, which must end
    /// on 0), and a timeline that doesn't check takes no key edits.
    #[test]
    fn a_remove_the_document_refuses_is_refused() {
        let document = cutscene();
        let post = KeyRef { lane: Lane::Post(PostParameter::Exposure), index: 2 };
        let why = removed(&document, &post).unwrap_err();
        assert!(why.contains("post[0].keys[1].value"), "{why}");
        let face =
            removed(&document, &KeyRef { lane: Lane::Faces("hero".to_owned()), index: 0 }).expect("removed");
        assert!(matches!(&face.writes[..], [Write::Remove { path }] if path == "characters[0].faces[0]"));
        let mut broken = document;
        broken.length = -1.0;
        assert!(removed(&broken, &post).unwrap_err().contains("wait for Properties"));
    }
}
