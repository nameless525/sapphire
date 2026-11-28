//! What the gizmo's handles change of a zone's objects (M6 T9b): a [`Grip`] is what an object has to take hold
//! of, read from the document when a drag starts, and a [`Drag`] turns where the pointer's ray goes into the
//! numbers it writes, snapped. None of it needs a GPU or a window, so its tests, and the commands a script drives a
//! drag by, run anywhere.
//!
//! **The handles,** by the gizmo's [mode](GizmoMode):
//! - **Move:** an arrow along each axis, the zone's or the object's own ([`Axes`]), which moves its position, or
//!   every point of a spline, a walk, a river or a lake's outline, or both corners of a reflection probe's box; and
//!   a ball on each point of a spline, a walk, a river or a lake, which moves that point over the level it stands
//!   at.
//! - **Turn:** a ring about +Y, for whatever has a yaw: a placement's or an instance's `turn` (or its `rotation`'s
//!   yaw where it has one), a spawner's, a checkpoint's or an ambient post's `yaw_degrees`, a yawed box's or an
//!   ellipsoid's `yaw`; and a spotlight's cone, its outer and inner half angles, each a ball on the cone's rim.
//! - **Scale:** a bar along each of a placement's or an instance's own axes, which scales it; a ball on each face
//!   of a box or an ellipsoid ([`sapphire_zone::ZoneShape`], or a trigger's, a checkpoint's or a room's volume),
//!   which moves that face; a ball on a sphere's side for its radius; and one at a light's range.
//!
//! **Snapping** ([`Steps`]): a move or a point lands on the grid's step along the zone's axes, or travels whole
//! steps along an object's own; a size, a radius or a range is a whole number of grid steps; a yaw or a cone's
//! angle a whole number of the yaw's step; a scale goes by hundredths. A step of 0 turns that snapping off. What
//! the zone's checks refuse stays as it was, and the drag goes on.

use sapphire_document::{PropertyPath, Step};
use sapphire_math::{Quat, Vec3, det};
use sapphire_zone::{LightShape, Volume, WaterShape, ZoneDocument, ZoneShape};
use serde::{Deserialize, Serialize};

use crate::gizmo::{Axis, Ray, along_line};
use crate::markers::{ObjectKind, ZoneObject};

/// What a scale is rounded to: hundredths, so the file reads as a person would write it.
pub const SCALE_STEP: f32 = 0.01;

/// Which of an object's handles the gizmo shows.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum GizmoMode {
    #[default]
    Move,
    Turn,
    Scale,
}

impl GizmoMode {
    pub const ALL: [Self; 3] = [Self::Move, Self::Turn, Self::Scale];

    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Move => "Move",
            Self::Turn => "Turn",
            Self::Scale => "Scale",
        }
    }
}

/// Which way the move arrows point: along the zone's axes, or the object's own as it's turned.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Axes {
    #[default]
    World,
    Local,
}

/// The steps a drag snaps to: `grid` in metres, `yaw` in degrees, 0 for none.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Steps {
    pub grid: f32,
    pub yaw: f32,
}

impl Default for Steps {
    /// A twentieth of a metre and a degree: what a person would write.
    fn default() -> Self {
        Self { grid: crate::app::SNAP, yaw: 1.0 }
    }
}

impl Steps {
    /// Whether each is a step: a number, 0 or more.
    #[must_use]
    pub fn are_steps(&self) -> bool {
        [self.grid, self.yaw].iter().all(|step| step.is_finite() && *step >= 0.0)
    }
}

/// `value` to the nearest whole number of `step`, or as it is for a step of 0. In f64, against the decimal the
/// step is written as (its shortest text), so a multiple of 0.05 is written `0.45`, never `0.45000002`.
#[must_use]
pub fn snap(value: f32, step: f32) -> f32 {
    if step <= 0.0 {
        return value;
    }
    let step = step.to_string().parse::<f64>().unwrap_or(f64::from(step));
    ((f64::from(value) / step).round() * step) as f32
}

/// One of the gizmo's handles (see the [module docs](self)).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Handle {
    /// An arrow: moves the object along an axis.
    Move(Axis),
    /// A point of a spline, a walk, a river or a lake, by its place in its list.
    Point(usize),
    /// The ring: turns the object about +Y.
    Yaw,
    /// A spotlight's outer half angle.
    Cone,
    /// A spotlight's inner half angle.
    InnerCone,
    /// A bar: scales a placement or an instance along its own axis.
    Scale(Axis),
    /// A face of a box or an ellipsoid, along its own axis: its half extent or its radius there.
    Size(Axis),
    /// A sphere's radius.
    Radius,
    /// A light's range.
    Range,
}

impl Handle {
    /// What a drag of it does, for the step it makes.
    #[must_use]
    pub const fn verb(self) -> &'static str {
        match self {
            Self::Move(_) => "move",
            Self::Point(_) => "move a point of",
            Self::Yaw => "turn",
            Self::Cone | Self::InnerCone => "widen the cone of",
            Self::Scale(_) => "scale",
            Self::Size(_) | Self::Radius => "resize",
            Self::Range => "set the range of",
        }
    }
}

/// A number of the document, by its path, as it was when the drag started.
#[derive(Clone, Debug, PartialEq)]
pub struct Number {
    pub path: PropertyPath,
    pub value: f32,
}

impl Number {
    fn at(path: PropertyPath, value: f32) -> Self {
        Self { path, value }
    }
}

/// Three numbers: a position, a scale, a box's half extents.
#[derive(Clone, Debug, PartialEq)]
pub struct Triple {
    pub path: PropertyPath,
    pub value: Vec3,
}

impl Triple {
    fn at(path: PropertyPath, value: [f32; 3]) -> Self {
        Self { path, value: Vec3::from(value) }
    }

    #[must_use]
    pub fn number(&self, axis: usize) -> PropertyPath {
        self.path.join(Step::Index(axis))
    }
}

/// A point of a spline, a walk, a river or a lake's outline, or a corner of a box.
#[derive(Clone, Debug, PartialEq)]
pub struct GripPoint {
    pub path: PropertyPath,
    pub at: Vec3,
    /// Written as `(x, z)`, at a height of its body's.
    pub flat: bool,
}

impl GripPoint {
    /// The paths of its numbers along X, Y and Z, those it has.
    fn numbers(&self) -> [Option<PropertyPath>; 3] {
        let at = |index| Some(self.path.join(Step::Index(index)));
        if self.flat { [at(0), None, at(1)] } else { [at(0), at(1), at(2)] }
    }
}

/// A box's half extents or an ellipsoid's radii, along its own axes.
#[derive(Clone, Debug, PartialEq)]
pub struct Size {
    pub extents: Triple,
    pub frame: Quat,
}

/// A spotlight's cone.
#[derive(Clone, Debug, PartialEq)]
pub struct Cone {
    /// Where its beam goes, unit length.
    pub along: Vec3,
    pub outer: Number,
    pub inner: Number,
}

/// What an object has for the gizmo to take hold of, as the document has it (see the [module docs](self)).
#[derive(Clone, Debug, PartialEq)]
pub struct Grip {
    /// Where the gizmo stands: its position, or its first point.
    pub origin: Vec3,
    /// Its own axes.
    pub frame: Quat,
    pub position: Option<Triple>,
    /// What moves with it where it has no position of its own.
    pub points: Vec<GripPoint>,
    /// Whether each point is a handle of its own.
    pub point_handles: bool,
    /// A lake's height, which its outline stands at.
    pub height: Option<Number>,
    pub yaw: Option<Number>,
    pub scale: Option<Triple>,
    pub size: Option<Size>,
    pub radius: Option<Number>,
    /// A light's range, and the way it's shown along.
    pub range: Option<(Number, Vec3)>,
    pub cone: Option<Cone>,
}

impl Grip {
    fn new(origin: Vec3) -> Self {
        Self {
            origin,
            frame: Quat::IDENTITY,
            position: None,
            points: Vec::new(),
            point_handles: false,
            height: None,
            yaw: None,
            scale: None,
            size: None,
            radius: None,
            range: None,
            cone: None,
        }
    }

    /// What `object` of `document` has to take hold of, if it has a place: an exit is its door's, and the probe
    /// a cook places over a water body is the cook's.
    #[must_use]
    pub fn of(document: &ZoneDocument, object: &ZoneObject) -> Option<Self> {
        let path = &object.path;
        let field = |name: &str| path.join(Step::Field(name.to_owned()));
        let (index, origin) = (object.index, object.at()?);
        let mut grip = Self::new(origin);
        let positioned = |grip: &mut Self, at: [f32; 3]| {
            grip.position =
                Some(Triple::at(field(if object.kind.is_placed() { "at" } else { "position" }), at));
        };
        match object.kind {
            ObjectKind::Placement | ObjectKind::Instance => {
                let (at, turn, rotation, scale, frame) = match object.kind {
                    ObjectKind::Placement => {
                        let placement = document.placements.get(index)?;
                        let frame = placement.orientation();
                        (placement.at, placement.turn, placement.rotation, placement.scale, frame)
                    }
                    _ => {
                        let instance = document.instances.get(index)?;
                        (
                            instance.at,
                            instance.turn,
                            instance.rotation,
                            instance.scale,
                            instance.orientation(),
                        )
                    }
                };
                positioned(&mut grip, at);
                grip.frame = frame;
                // A rotated entry turns by its rotation's yaw (T8a): one of `turn` and `rotation`, never both.
                grip.yaw = Some(match rotation {
                    Some([_, yaw, _]) => {
                        Number::at(field("rotation").join(Step::Some).join(Step::Index(1)), yaw)
                    }
                    None => Number::at(field("turn"), turn),
                });
                grip.scale = Some(Triple::at(field("scale"), scale));
            }
            ObjectKind::Light => {
                let light = document.lights.get(index)?;
                positioned(&mut grip, light.position);
                let range = Number::at(field("range"), light.range);
                match light.shape {
                    LightShape::Point => grip.range = Some((range, Vec3::X)),
                    LightShape::Spot { direction, inner_degrees, outer_degrees, .. } => {
                        let along = Vec3::from(direction).normalize_or(Vec3::NEG_Y);
                        let shape = field("shape");
                        grip.range = Some((range, along));
                        grip.cone = Some(Cone {
                            along,
                            outer: Number::at(
                                shape.join(Step::Field("outer_degrees".to_owned())),
                                outer_degrees,
                            ),
                            inner: Number::at(
                                shape.join(Step::Field("inner_degrees".to_owned())),
                                inner_degrees,
                            ),
                        });
                    }
                }
            }
            ObjectKind::ReflectionProbe => positioned(&mut grip, document.reflections.get(index)?.position),
            ObjectKind::ReflectionBox => {
                let parallax = &document.reflections.get(index)?.parallax;
                grip.points = [("min", parallax.min), ("max", parallax.max)]
                    .into_iter()
                    .map(|(corner, at)| GripPoint { path: field(corner), at: Vec3::from(at), flat: false })
                    .collect();
            }
            ObjectKind::Trigger => {
                let trigger = document.triggers.get(index)?;
                positioned(&mut grip, trigger.position);
                grip.volume(&field("volume"), &trigger.volume);
            }
            ObjectKind::Checkpoint => {
                let checkpoint = document.checkpoints.get(index)?;
                positioned(&mut grip, checkpoint.position);
                grip.yawed(field("yaw_degrees"), checkpoint.yaw_degrees);
                grip.volume(&field("volume"), &checkpoint.volume);
            }
            ObjectKind::Room => {
                let room = document.rooms.get(index)?;
                positioned(&mut grip, room.position);
                grip.volume(&field("volume"), &room.volume);
            }
            ObjectKind::Spawner => {
                let spawner = document.spawners.get(index)?;
                positioned(&mut grip, spawner.position);
                grip.yawed(field("yaw_degrees"), spawner.yaw_degrees);
            }
            ObjectKind::AmbientPost => {
                let post = document.ambient.as_ref()?.posts.get(index)?;
                positioned(&mut grip, post.position);
                grip.yawed(field("yaw_degrees"), post.yaw_degrees);
            }
            ObjectKind::Spline => grip.path_of(&field("points"), &document.splines.get(index)?.points),
            ObjectKind::AmbientWalk => {
                grip.path_of(&field("points"), &document.ambient.as_ref()?.walks.get(index)?.points);
            }
            ObjectKind::AtmosphereVolume => {
                let volume = document.volumes.get(index)?;
                positioned(&mut grip, volume.position);
                grip.shape(&field("shape"), &volume.shape);
            }
            ObjectKind::WeatherRegion => {
                let region = document.weather_plan.as_ref()?.regions.get(index)?;
                positioned(&mut grip, region.position);
                grip.shape(&field("shape"), &region.shape);
            }
            ObjectKind::WaterBody => {
                let shape = field("shape");
                match &document.water.get(index)?.shape {
                    WaterShape::Lake { outline, height } => {
                        let outline_path = shape.join(Step::Field("outline".to_owned()));
                        grip.points = outline
                            .iter()
                            .enumerate()
                            .map(|(at, &[x, z])| GripPoint {
                                path: outline_path.join(Step::Index(at)),
                                at: Vec3::new(x, *height, z),
                                flat: true,
                            })
                            .collect();
                        grip.point_handles = true;
                        grip.height = Some(Number::at(shape.join(Step::Field("height".to_owned())), *height));
                    }
                    WaterShape::River { points } => {
                        let list = shape.join(Step::Field("points".to_owned()));
                        grip.points = points
                            .iter()
                            .enumerate()
                            .map(|(at, point)| GripPoint {
                                path: list.join(Step::Index(at)).join(Step::Field("at".to_owned())),
                                at: Vec3::from(point.at),
                                flat: false,
                            })
                            .collect();
                        grip.point_handles = true;
                    }
                    WaterShape::Ocean { .. } => return None,
                }
            }
            ObjectKind::Exit | ObjectKind::WaterProbe => return None,
        }
        Some(grip)
    }

    /// A yaw about +Y, which turns its own axes too.
    fn yawed(&mut self, path: PropertyPath, yaw: f32) {
        self.frame = det::quat_from_axis_angle(Vec3::Y, yaw.to_radians());
        self.yaw = Some(Number::at(path, yaw));
    }

    /// A trigger's, a checkpoint's or a room's volume, along the zone's axes.
    fn volume(&mut self, path: &PropertyPath, volume: &Volume) {
        match *volume {
            Volume::Box { half_extents } => {
                let extents = Triple::at(path.join(Step::Field("half_extents".to_owned())), half_extents);
                self.size = Some(Size { extents, frame: Quat::IDENTITY });
            }
            Volume::Sphere { radius } => {
                self.radius = Some(Number::at(path.join(Step::Field("radius".to_owned())), radius));
            }
        }
    }

    /// A [`ZoneShape`]: a sphere's radius, or a box's or an ellipsoid's yaw and extents along its own axes.
    fn shape(&mut self, path: &PropertyPath, shape: &ZoneShape) {
        let at = |name: &str| path.join(Step::Field(name.to_owned()));
        let (extents, yaw) = match *shape {
            ZoneShape::Sphere { radius } => {
                self.radius = Some(Number::at(at("radius"), radius));
                return;
            }
            ZoneShape::Box { half_extents, yaw } => (Triple::at(at("half_extents"), half_extents), yaw),
            ZoneShape::Ellipsoid { radii, yaw } => (Triple::at(at("radii"), radii), yaw),
        };
        self.yawed(at("yaw"), yaw);
        self.size = Some(Size { extents, frame: self.frame });
    }

    /// The points of a spline or a walk.
    fn path_of(&mut self, list: &PropertyPath, points: &[[f32; 3]]) {
        self.points = points
            .iter()
            .enumerate()
            .map(|(at, &point)| GripPoint {
                path: list.join(Step::Index(at)),
                at: Vec3::from(point),
                flat: false,
            })
            .collect();
        self.point_handles = true;
    }

    /// The handles it has in `mode`.
    #[must_use]
    pub fn handles(&self, mode: GizmoMode) -> Vec<Handle> {
        let mut handles = Vec::new();
        match mode {
            GizmoMode::Move => {
                if self.position.is_some() || !self.points.is_empty() {
                    handles.extend(Axis::ALL.map(Handle::Move));
                }
                if self.point_handles {
                    handles.extend((0..self.points.len()).map(Handle::Point));
                }
            }
            GizmoMode::Turn => {
                if self.yaw.is_some() {
                    handles.push(Handle::Yaw);
                }
                if self.cone.is_some() {
                    handles.extend([Handle::Cone, Handle::InnerCone]);
                }
            }
            GizmoMode::Scale => {
                if self.scale.is_some() {
                    handles.extend(Axis::ALL.map(Handle::Scale));
                }
                if self.size.is_some() {
                    handles.extend(Axis::ALL.map(Handle::Size));
                }
                if self.radius.is_some() {
                    handles.push(Handle::Radius);
                }
                if self.range.is_some() {
                    handles.push(Handle::Range);
                }
            }
        }
        handles
    }

    /// The way the arrow or bar `axis` points with `axes`: a scale's always along the object's own.
    #[must_use]
    pub fn direction(&self, handle: Handle, axes: Axes) -> Option<Vec3> {
        let own = |axis: Axis| (self.frame * axis.direction()).normalize();
        Some(match handle {
            Handle::Move(axis) => match axes {
                Axes::World => axis.direction(),
                Axes::Local => own(axis),
            },
            Handle::Scale(axis) => own(axis),
            Handle::Size(axis) => (self.size.as_ref()?.frame * axis.direction()).normalize(),
            Handle::Radius => Vec3::X,
            Handle::Range => self.range.as_ref()?.1,
            Handle::Point(_) | Handle::Yaw | Handle::Cone | Handle::InnerCone => return None,
        })
    }

    /// Where a ball handle stands, in the zone's space, with the gizmo `length` metres long.
    #[must_use]
    pub fn ball(&self, handle: Handle, length: f32) -> Option<Vec3> {
        let out = |distance: f32| Some(self.origin + self.direction(handle, Axes::Local)? * distance);
        match handle {
            Handle::Point(at) => self.points.get(at).map(|point| point.at),
            Handle::Size(axis) => out(self.size.as_ref()?.extents.value[axis.index()]),
            Handle::Radius => out(self.radius.as_ref()?.value),
            Handle::Range => out(self.range.as_ref()?.0.value),
            Handle::Cone | Handle::InnerCone => {
                let cone = self.cone.as_ref()?;
                let degrees = if handle == Handle::Cone { cone.outer.value } else { cone.inner.value };
                let distance = length * 1.5;
                let (along, aside) = (cone.along, aside_of(cone.along));
                Some(self.origin + along * distance + aside * distance * degrees.to_radians().tan())
            }
            Handle::Move(_) | Handle::Scale(_) | Handle::Yaw => None,
        }
    }
}

/// A unit vector at right angles to `along`, a unit vector: the way a cone's handles stand off its beam.
#[must_use]
pub fn aside_of(along: Vec3) -> Vec3 {
    let across = along.cross(Vec3::Y);
    if across.length_squared() > 1e-6 { across.normalize() } else { along.cross(Vec3::X).normalize() }
}

/// Where a ray meets the plane through `point` facing `normal`, a unit vector.
fn on_plane(ray: Ray, point: Vec3, normal: Vec3) -> Option<Vec3> {
    let facing = ray.direction.dot(normal);
    if facing.abs() < 1e-4 {
        return None;
    }
    let t = (point - ray.origin).dot(normal) / facing;
    (t.is_finite() && t > 0.0).then(|| ray.origin + ray.direction * t)
}

/// The angle about +Y of `point` from `center`, in degrees, counter-clockwise seen from above from +X, as a
/// yaw turns: +X toward -Z.
fn yaw_of(point: Vec3, center: Vec3) -> f32 {
    let offset = point - center;
    det::atan2(-offset.z, offset.x).to_degrees()
}

/// `degrees` into -180 to 180.
fn wrapped(degrees: f32) -> f32 {
    (degrees + 180.0).rem_euclid(360.0) - 180.0
}

/// Where the press found the handle.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Pressed {
    /// How far along its line.
    Along(f32),
    /// Where on its plane.
    On(Vec3),
    /// At which angle, in degrees.
    Angle(f32),
}

/// A handle taken hold of: where the press found it, and what the object was then.
#[derive(Clone, Debug, PartialEq)]
pub struct Drag {
    pub handle: Handle,
    pub grip: Grip,
    axes: Axes,
    pressed: Pressed,
}

impl Drag {
    /// A press of `handle` of `grip` by `ray`, if the ray can say where on it: not along a line it runs with, nor
    /// across a plane it runs in.
    #[must_use]
    pub fn start(grip: Grip, handle: Handle, axes: Axes, ray: Ray) -> Option<Self> {
        let mut drag = Self { handle, grip, axes, pressed: Pressed::Along(0.0) };
        drag.pressed = drag.find(ray)?;
        Some(drag)
    }

    /// Where `ray` finds the handle, as a press does.
    fn find(&self, ray: Ray) -> Option<Pressed> {
        let grip = &self.grip;
        match self.handle {
            Handle::Move(_) | Handle::Scale(_) | Handle::Size(_) | Handle::Radius | Handle::Range => {
                let direction = grip.direction(self.handle, self.axes)?;
                along_line(grip.origin, direction, ray).map(Pressed::Along)
            }
            Handle::Point(at) => {
                let point = grip.points.get(at)?.at;
                on_plane(ray, point, Vec3::Y).map(Pressed::On)
            }
            Handle::Yaw => {
                on_plane(ray, grip.origin, Vec3::Y).map(|hit| Pressed::Angle(yaw_of(hit, grip.origin)))
            }
            Handle::Cone | Handle::InnerCone => {
                let cone = grip.cone.as_ref()?;
                let aside = aside_of(cone.along);
                let hit = on_plane(ray, grip.origin, cone.along.cross(aside).normalize())?;
                let offset = hit - grip.origin;
                Some(Pressed::Angle(det::atan2(offset.dot(aside), offset.dot(cone.along)).to_degrees()))
            }
        }
    }

    /// The numbers the drag puts where `ray` now points, snapped to `steps`: each it changes from what the object
    /// was when it started, and each it changed before and may now put back. `None` where the ray can't say.
    #[must_use]
    pub fn to(&self, ray: Ray, steps: Steps) -> Option<Vec<Number>> {
        let grip = &self.grip;
        let now = self.find(ray)?;
        let mut numbers = Vec::new();
        let mut put = |path: PropertyPath, value: f32| numbers.push(Number { path, value });
        match (self.handle, self.pressed, now) {
            (Handle::Move(axis), Pressed::Along(pressed), Pressed::Along(now)) => {
                let by = match self.axes {
                    // On the zone's grid, where the object's place lands.
                    Axes::World => {
                        let from = grip.origin[axis.index()];
                        axis.direction() * (snap(from + now - pressed, steps.grid) - from)
                    }
                    // Whole steps along its own axis, from where it was.
                    Axes::Local => grip.direction(self.handle, self.axes)? * snap(now - pressed, steps.grid),
                };
                if let Some(position) = &grip.position {
                    let mut moved = position.value + by;
                    // On the grid, the coordinate is the snapped number itself, not one f32 sum from it.
                    if self.axes == Axes::World && position.value == grip.origin {
                        let from = grip.origin[axis.index()];
                        moved[axis.index()] = snap(from + now - pressed, steps.grid);
                    }
                    for index in 0..3 {
                        put(position.number(index), moved[index]);
                    }
                }
                for point in &grip.points {
                    let moved = point.at + by;
                    for (index, path) in point.numbers().into_iter().enumerate() {
                        if let Some(path) = path {
                            put(path, moved[index]);
                        }
                    }
                }
                if let Some(height) = &grip.height {
                    put(height.path.clone(), height.value + by.y);
                }
            }
            (Handle::Point(at), Pressed::On(pressed), Pressed::On(now)) => {
                let point = grip.points.get(at)?;
                let moved = point.at + (now - pressed);
                let [x, _, z] = point.numbers();
                put(x?, snap(moved.x, steps.grid));
                put(z?, snap(moved.z, steps.grid));
            }
            (Handle::Yaw, Pressed::Angle(pressed), Pressed::Angle(now)) => {
                let yaw = grip.yaw.as_ref()?;
                put(yaw.path.clone(), snap(yaw.value + wrapped(now - pressed), steps.yaw));
            }
            (Handle::Cone | Handle::InnerCone, Pressed::Angle(pressed), Pressed::Angle(now)) => {
                let cone = grip.cone.as_ref()?;
                let angle = if self.handle == Handle::Cone { &cone.outer } else { &cone.inner };
                put(angle.path.clone(), snap(angle.value + (now - pressed), steps.yaw));
            }
            (Handle::Scale(axis), Pressed::Along(pressed), Pressed::Along(now)) => {
                let scale = grip.scale.as_ref()?;
                if pressed.abs() < 1e-4 {
                    return None;
                }
                // Never through 0 to a mirror: one step at least.
                let scaled = snap(scale.value[axis.index()] * now / pressed, SCALE_STEP).max(SCALE_STEP);
                put(scale.number(axis.index()), scaled);
            }
            (Handle::Size(axis), Pressed::Along(pressed), Pressed::Along(now)) => {
                let extents = &grip.size.as_ref()?.extents;
                put(
                    extents.number(axis.index()),
                    snap(extents.value[axis.index()] + now - pressed, steps.grid),
                );
            }
            (Handle::Radius, Pressed::Along(pressed), Pressed::Along(now)) => {
                let radius = grip.radius.as_ref()?;
                put(radius.path.clone(), snap(radius.value + now - pressed, steps.grid));
            }
            (Handle::Range, Pressed::Along(pressed), Pressed::Along(now)) => {
                let range = &grip.range.as_ref()?.0;
                put(range.path.clone(), snap(range.value + now - pressed, steps.grid));
            }
            _ => return None,
        }
        Some(numbers.into_iter().filter(|number| number.value.is_finite()).collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ray_down(at: Vec3) -> Ray {
        Ray { origin: at + Vec3::Y * 10.0, direction: Vec3::NEG_Y }
    }

    /// A ring's angle goes as a yaw does: a point toward -Z of the center is a quarter turn.
    #[test]
    fn a_yaw_turns_x_toward_minus_z() {
        assert!((yaw_of(Vec3::new(0.0, 0.0, -2.0), Vec3::ZERO) - 90.0).abs() < 1e-4);
        let turned = Quat::from_rotation_y(90f32.to_radians()) * Vec3::X;
        assert!((turned - Vec3::NEG_Z).length() < 1e-5, "{turned}");
        assert_eq!(wrapped(270.0), -90.0);
    }

    /// A snapped number is written with no more decimals than its step, over the first few hundred multiples of
    /// the grid's and a scale's, from either side.
    #[test]
    fn a_snap_writes_its_steps_decimals() {
        for (step, decimals) in [(0.05_f32, 2), (SCALE_STEP, 2), (0.5, 1)] {
            for multiple in -300_i16..300 {
                let near = f32::from(multiple) * step;
                for value in [near + step * 0.3, near - step * 0.3] {
                    let text = snap(value, step).to_string();
                    let written = text.split_once('.').map_or(0, |(_, fraction)| fraction.len());
                    assert!(written <= decimals, "{value} to {step} is written {text}");
                }
            }
        }
        assert_eq!(snap(0.46, 0.05).to_string(), "0.45");
        assert_eq!(snap(0.104, SCALE_STEP).to_string(), "0.1");
    }

    /// A point dragged on its level lands on the grid, its height as it was.
    #[test]
    fn a_point_lands_on_the_grid() {
        let path = PropertyPath::root().join(Step::Field("points".to_owned()));
        let mut grip = Grip::new(Vec3::new(1.0, 2.0, 3.0));
        grip.points =
            vec![GripPoint { path: path.join(Step::Index(0)), at: Vec3::new(1.0, 2.0, 3.0), flat: false }];
        grip.point_handles = true;
        let drag = Drag::start(grip, Handle::Point(0), Axes::World, ray_down(Vec3::new(1.0, 2.0, 3.0)))
            .expect("pressed");
        let numbers =
            drag.to(ray_down(Vec3::new(2.33, 2.0, 2.01)), Steps { grid: 0.5, yaw: 0.0 }).expect("moved");
        let values: Vec<f32> = numbers.iter().map(|number| number.value).collect();
        assert_eq!(values, [2.5, 2.0]);
        assert_eq!(numbers[1].path.to_string(), "points[0][2]");
    }
}
