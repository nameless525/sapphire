//! What a zone holds, drawn and picked (M6 T9a): each object of a zone document as an editor shows it, with the
//! debug draws that mark it in the viewport and the handles a click or a box picks it by.
//!
//! An object is one entry of one of the document's lists, named by a keyed path (M6 T8a): a placement, an
//! instance, a light or an atmosphere volume by its ID (`instances["3fa91c07"]`), every other by its name
//! (`spawners["player_start"]`). A part of an entry that stands somewhere of its own is an object too: a reflection
//! probe's parallax box (`reflections["hall"].parallax`) and the probe a cook places over a water body that names
//! none (`water["pond"].probe`). [`ZoneObjects::of`] makes them all from a document, in [`ObjectKind::ALL`]'s order
//! and each list's own, once a revision of it: what the viewport draws, what the [pick index](crate::pick) is
//! built over and what the zone pane lists.
//!
//! Markers are lines through the stage's debug draws, which the viewport's stage builds at its stand
//! ([`crate::viewport::stand_zone_stage`]), so marking a zone builds no pipeline. A placement is drawn as its
//! prefab's shape and has no marker of its own; the sea is everywhere and has no place, so it has neither marker
//! nor handle; an exit is marked over the trigger that is its door, and has no place without one. An object with
//! no place (the sea, an exit with no door, a spline or a lake with no points) is still an object, which the pane
//! lists and the Properties pane edits; only a click or a box can't find it.
//!
//! **A kind is added in one place:** a variant of [`ObjectKind`] (its name, color and list) and its arm in
//! [`ZoneObjects::of`], which says where its handles are and what it draws. The kinds whose packages haven't
//! landed (fog volumes, wind volumes, effect placements, ambience emitters) each become one of those. A zone's
//! navigation bounds and seeds are T11's to draw, with what the cook makes of them.

use std::f32::consts::TAU;

use sapphire_core::debug_draw::{self as dd, Color, Shape, Style};
use sapphire_document::{PropertyPath, Step};
use sapphire_math::{Quat, Vec3};
use sapphire_zone::{LightShape, Volume, WaterShape, ZoneDocument, ZoneShape};

/// How far over the top of its door's volume an exit's mark stands, in metres.
const EXIT_LIFT: f32 = 1.0;
/// How large a mark of a point is: an instance's box, a light's or a probe's ball, in metres.
const POINT_SIZE: f32 = 0.25;
/// How many lines an ellipse of an ellipsoid takes: debug draw has no ellipse, so each is a call of its own, and a
/// zone of many costs that many calls of its thread's buffer.
const ELLIPSE_SEGMENTS: usize = 12;

/// What an object of a zone is: which list it's of, and how it's marked.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ObjectKind {
    Placement,
    Instance,
    Light,
    ReflectionProbe,
    /// A reflection probe's parallax box.
    ReflectionBox,
    Trigger,
    Checkpoint,
    Spawner,
    Spline,
    Exit,
    AtmosphereVolume,
    WeatherRegion,
    WaterBody,
    /// The probe a cook places over a water body that names none.
    WaterProbe,
    Room,
    /// Someone of the zone's ambient characters standing in one place (M5 C10).
    AmbientPost,
    /// A line the zone's ambient characters walk (M5 C10).
    AmbientWalk,
}

impl ObjectKind {
    /// Every kind, in the order the pane lists them.
    pub const ALL: [Self; 17] = [
        Self::Placement,
        Self::Instance,
        Self::Light,
        Self::ReflectionProbe,
        Self::ReflectionBox,
        Self::Trigger,
        Self::Checkpoint,
        Self::Spawner,
        Self::Spline,
        Self::Exit,
        Self::AtmosphereVolume,
        Self::WeatherRegion,
        Self::WaterBody,
        Self::WaterProbe,
        Self::Room,
        Self::AmbientPost,
        Self::AmbientWalk,
    ];

    /// What the pane calls a list of them.
    #[must_use]
    pub const fn plural(self) -> &'static str {
        match self {
            Self::Placement => "Placements",
            Self::Instance => "Instances",
            Self::Light => "Lights",
            Self::ReflectionProbe => "Reflection probes",
            Self::ReflectionBox => "Reflection boxes",
            Self::Trigger => "Triggers",
            Self::Checkpoint => "Checkpoints",
            Self::Spawner => "Spawners",
            Self::Spline => "Splines",
            Self::Exit => "Exits",
            Self::AtmosphereVolume => "Atmosphere volumes",
            Self::WeatherRegion => "Weather regions",
            Self::WaterBody => "Water",
            Self::WaterProbe => "Water probes",
            Self::Room => "Rooms",
            Self::AmbientPost => "Ambient posts",
            Self::AmbientWalk => "Ambient walks",
        }
    }

    /// The document's list it's an entry of, or a part of an entry of.
    #[must_use]
    pub const fn list(self) -> &'static str {
        match self {
            Self::Placement => "placements",
            Self::Instance => "instances",
            Self::Light => "lights",
            Self::ReflectionProbe | Self::ReflectionBox => "reflections",
            Self::Trigger => "triggers",
            Self::Checkpoint => "checkpoints",
            Self::Spawner => "spawners",
            Self::Spline => "splines",
            Self::Exit => "exits",
            Self::AtmosphereVolume => "volumes",
            Self::WeatherRegion => "weather_plan",
            Self::WaterBody | Self::WaterProbe => "water",
            Self::Room => "rooms",
            Self::AmbientPost | Self::AmbientWalk => "ambient",
        }
    }

    /// The color its marks are drawn in, when it isn't selected.
    #[must_use]
    pub const fn color(self) -> Color {
        match self {
            Self::Placement => Color::WHITE,
            Self::Instance => Color::rgb(200, 200, 200),
            Self::Light => Color::YELLOW,
            Self::ReflectionProbe => Color::CYAN,
            Self::ReflectionBox => Color::rgb(40, 150, 160),
            Self::Trigger => Color::ORANGE,
            Self::Checkpoint => Color::GREEN,
            Self::Spawner => Color::RED,
            Self::Spline => Color::MAGENTA,
            Self::Exit => Color::rgb(255, 120, 200),
            Self::AtmosphereVolume => Color::rgb(150, 120, 255),
            Self::WeatherRegion => Color::rgb(170, 210, 255),
            Self::WaterBody => Color::BLUE,
            Self::WaterProbe => Color::rgb(90, 170, 255),
            Self::Room => Color::rgb(220, 170, 90),
            Self::AmbientPost => Color::rgb(120, 230, 180),
            Self::AmbientWalk => Color::rgb(70, 190, 140),
        }
    }
}

/// The color a selected object's marks are drawn in, over everything.
pub const SELECTED: Color = Color::WHITE;

/// One object of a zone (see the [module docs](self)).
#[derive(Clone, Debug, PartialEq)]
pub struct ZoneObject {
    pub kind: ObjectKind,
    /// Where it is in the document, by its key.
    pub path: PropertyPath,
    /// Its key in its list: its ID, or its name.
    pub key: String,
    /// Where it is in its list.
    pub index: usize,
    /// The points a pick finds it by, in the zone's space: none for an object with no place. The first is where
    /// it is.
    pub handles: Vec<Vec3>,
    /// What marks it.
    pub shapes: Vec<Shape>,
}

impl ZoneObject {
    /// Where it is: its first handle, if it has a place.
    #[must_use]
    pub fn at(&self) -> Option<Vec3> {
        self.handles.first().copied()
    }
}

/// Every object of a zone document (see the [module docs](self)).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ZoneObjects {
    pub objects: Vec<ZoneObject>,
    /// The zone's grid's side, in metres, which the pick index is cut along.
    pub cell_size: f32,
}

impl ZoneObjects {
    /// The objects of `document`, kind by kind in [`ObjectKind::ALL`]'s order.
    #[must_use]
    pub fn of(document: &ZoneDocument) -> Self {
        let mut objects = Vec::new();
        let mut add = |kind: ObjectKind, key: &str, index: usize, handles: Vec<Vec3>, shapes: Vec<Shape>| {
            // A place that isn't a number is no place: the zone's checks refuse it, and nothing can be shown there.
            let (handles, shapes) = if handles.iter().all(|handle| handle.is_finite()) {
                (handles, shapes)
            } else {
                Default::default()
            };
            objects.push(ZoneObject {
                kind,
                path: keyed(kind, key),
                key: key.to_owned(),
                index,
                handles,
                shapes,
            });
        };
        for (index, placement) in document.placements.iter().enumerate() {
            add(ObjectKind::Placement, &placement.id, index, vec![Vec3::from(placement.at)], Vec::new());
        }
        for (index, instance) in document.instances.iter().enumerate() {
            let at = Vec3::from(instance.at);
            let mark = Shape::OrientedBox {
                center: at,
                half_extents: Vec3::splat(POINT_SIZE),
                rotation: instance.orientation(),
            };
            add(ObjectKind::Instance, &instance.id, index, vec![at], vec![mark]);
        }
        for (index, light) in document.lights.iter().enumerate() {
            let at = Vec3::from(light.position);
            let mut shapes = vec![Shape::Sphere { center: at, radius: POINT_SIZE }];
            match light.shape {
                LightShape::Point => {
                    shapes.push(Shape::Circle { center: at, normal: Vec3::Y, radius: light.range });
                }
                LightShape::Spot { direction, .. } => {
                    let along = Vec3::from(direction).normalize_or(Vec3::NEG_Y);
                    shapes.push(Shape::Arrow { from: at, to: at + along * light.range.min(2.0) });
                }
            }
            add(ObjectKind::Light, &light.id, index, vec![at], shapes);
        }
        for (index, probe) in document.reflections.iter().enumerate() {
            let at = Vec3::from(probe.position);
            add(ObjectKind::ReflectionProbe, &probe.name, index, vec![at], vec![probe_mark(at)]);
        }
        for (index, probe) in document.reflections.iter().enumerate() {
            let (min, max) = (Vec3::from(probe.parallax.min), Vec3::from(probe.parallax.max));
            add(
                ObjectKind::ReflectionBox,
                &probe.name,
                index,
                vec![box_top(min, max)],
                vec![Shape::Aabb { min, max }],
            );
        }
        for (index, trigger) in document.triggers.iter().enumerate() {
            let at = Vec3::from(trigger.position);
            add(ObjectKind::Trigger, &trigger.name, index, vec![at], vec![volume(at, &trigger.volume)]);
        }
        for (index, checkpoint) in document.checkpoints.iter().enumerate() {
            let at = Vec3::from(checkpoint.position);
            let shapes = vec![volume(at, &checkpoint.volume), facing(at, checkpoint.yaw_degrees)];
            add(ObjectKind::Checkpoint, &checkpoint.name, index, vec![at], shapes);
        }
        for (index, spawner) in document.spawners.iter().enumerate() {
            let at = Vec3::from(spawner.position);
            let feet = Shape::Circle { center: at, normal: Vec3::Y, radius: 0.4 };
            add(
                ObjectKind::Spawner,
                &spawner.name,
                index,
                vec![at],
                vec![feet, facing(at, spawner.yaw_degrees)],
            );
        }
        for (index, spline) in document.splines.iter().enumerate() {
            let points: Vec<Vec3> = spline.points.iter().copied().map(Vec3::from).collect();
            let shapes = polyline(&points, spline.closed);
            add(ObjectKind::Spline, &spline.name, index, points, shapes);
        }
        for (index, exit) in document.exits.iter().enumerate() {
            // Its door: the trigger raising its name.
            let Some(door) = document.triggers.iter().find(|trigger| trigger.event == exit.name) else {
                add(ObjectKind::Exit, &exit.name, index, Vec::new(), Vec::new());
                continue;
            };
            let at = Vec3::from(door.position);
            let top = match door.volume {
                Volume::Box { half_extents } => half_extents[1],
                Volume::Sphere { radius } => radius,
            };
            let sign = at + Vec3::Y * (top + EXIT_LIFT);
            let shapes = vec![Shape::Arrow { from: at, to: sign }, probe_mark(sign)];
            add(ObjectKind::Exit, &exit.name, index, vec![sign], shapes);
        }
        for (index, placed) in document.volumes.iter().enumerate() {
            let at = Vec3::from(placed.position);
            add(ObjectKind::AtmosphereVolume, &placed.id, index, vec![at], zone_shape(at, &placed.shape));
        }
        let regions = document.weather_plan.iter().flat_map(|plan| &plan.regions);
        for (index, region) in regions.enumerate() {
            let at = Vec3::from(region.position);
            add(ObjectKind::WeatherRegion, &region.name, index, vec![at], zone_shape(at, &region.shape));
        }
        for (index, body) in document.water.iter().enumerate() {
            let (handles, closed) = match &body.shape {
                WaterShape::Lake { outline, height } => {
                    (outline.iter().map(|&[x, z]| Vec3::new(x, *height, z)).collect(), true)
                }
                WaterShape::River { points } => {
                    (points.iter().map(|point| Vec3::from(point.at)).collect(), false)
                }
                WaterShape::Ocean { .. } => (Vec::new(), false),
            };
            let shapes = polyline(&handles, closed);
            add(ObjectKind::WaterBody, &body.name, index, handles, shapes);
        }
        for (index, body) in document.water.iter().enumerate() {
            let Some(probe) = body.placed_probe() else { continue };
            let at = Vec3::from(probe.position);
            let (min, max) = (Vec3::from(probe.parallax.min), Vec3::from(probe.parallax.max));
            add(
                ObjectKind::WaterProbe,
                &body.name,
                index,
                vec![at],
                vec![probe_mark(at), Shape::Aabb { min, max }],
            );
        }
        for (index, room) in document.rooms.iter().enumerate() {
            let at = Vec3::from(room.position);
            add(ObjectKind::Room, &room.name, index, vec![at], vec![volume(at, &room.volume)]);
        }
        let posts = document.ambient.iter().flat_map(|ambient| &ambient.posts);
        for (index, post) in posts.enumerate() {
            let at = Vec3::from(post.position);
            let feet = Shape::Circle { center: at, normal: Vec3::Y, radius: 0.3 };
            add(
                ObjectKind::AmbientPost,
                &post.name,
                index,
                vec![at],
                vec![feet, facing(at, post.yaw_degrees)],
            );
        }
        let walks = document.ambient.iter().flat_map(|ambient| &ambient.walks);
        for (index, walk) in walks.enumerate() {
            let points: Vec<Vec3> = walk.points.iter().copied().map(Vec3::from).collect();
            let shapes = polyline(&points, walk.closed);
            add(ObjectKind::AmbientWalk, &walk.name, index, points, shapes);
        }
        Self { objects, cell_size: document.grid.cell_size }
    }

    /// The object at `path`, if it's one.
    #[must_use]
    pub fn find(&self, path: &PropertyPath) -> Option<usize> {
        self.objects.iter().position(|object| object.path == *path)
    }

    /// Draws every object's marks through debug draw: those in `selected`, indices sorted, first and over
    /// everything in [`SELECTED`], so a zone with more marks than debug draw keeps loses others' rather than theirs.
    pub fn draw(&self, selected: &[usize]) {
        if !dd::enabled() {
            return;
        }
        for object in selected.iter().filter_map(|&at| self.objects.get(at)) {
            for shape in &object.shapes {
                dd::shape(*shape, Style::new(SELECTED).on_top());
            }
        }
        for (index, object) in self.objects.iter().enumerate() {
            if selected.binary_search(&index).is_ok() {
                continue;
            }
            for shape in &object.shapes {
                dd::shape(*shape, Style::new(object.kind.color()));
            }
        }
    }

    /// How many marks it draws.
    #[must_use]
    pub fn shapes(&self) -> usize {
        self.objects.iter().map(|object| object.shapes.len()).sum()
    }
}

/// The path of the entry of `kind`'s list called `key`, or of its part that `kind` is.
#[must_use]
pub fn keyed(kind: ObjectKind, key: &str) -> PropertyPath {
    let field = |name: &str| Step::Field(name.to_owned());
    let within = |section: &str, list: &str| {
        PropertyPath::root().join(field(section)).join(Step::Some).join(field(list))
    };
    let list = match kind {
        ObjectKind::WeatherRegion => within("weather_plan", "regions"),
        ObjectKind::AmbientPost => within("ambient", "posts"),
        ObjectKind::AmbientWalk => within("ambient", "walks"),
        _ => PropertyPath::root().join(field(kind.list())),
    };
    let entry = list.join(Step::Key(key.to_owned()));
    match kind {
        ObjectKind::ReflectionBox => entry.join(field("parallax")),
        ObjectKind::WaterProbe => entry.join(field("probe")),
        _ => entry,
    }
}

/// A probe's mark: a ball.
const fn probe_mark(at: Vec3) -> Shape {
    Shape::Sphere { center: at, radius: POINT_SIZE }
}

/// The middle of a box's top, where a pick finds it: its middle is often its probe's place.
fn box_top(min: Vec3, max: Vec3) -> Vec3 {
    Vec3::new((min.x + max.x) * 0.5, max.y, (min.z + max.z) * 0.5)
}

/// An arrow along the ground from `at`, `yaw` degrees about +Y from +Z.
fn facing(at: Vec3, yaw: f32) -> Shape {
    let along = Quat::from_rotation_y(yaw.to_radians()) * Vec3::Z;
    Shape::Arrow { from: at, to: at + along }
}

/// A trigger's, a checkpoint's or a room's volume about `at`.
fn volume(at: Vec3, volume: &Volume) -> Shape {
    match *volume {
        Volume::Box { half_extents } => {
            let half = Vec3::from(half_extents);
            Shape::Aabb { min: at - half, max: at + half }
        }
        Volume::Sphere { radius } => Shape::Sphere { center: at, radius },
    }
}

/// A [`ZoneShape`] about `at`: an ellipsoid as its three principal ellipses.
fn zone_shape(at: Vec3, shape: &ZoneShape) -> Vec<Shape> {
    match *shape {
        ZoneShape::Sphere { radius } => vec![Shape::Sphere { center: at, radius }],
        ZoneShape::Box { half_extents, yaw } => vec![Shape::OrientedBox {
            center: at,
            half_extents: Vec3::from(half_extents),
            rotation: Quat::from_rotation_y(yaw.to_radians()),
        }],
        ZoneShape::Ellipsoid { radii: [x, y, z], yaw } => {
            let turn = Quat::from_rotation_y(yaw.to_radians());
            let (x, y, z) = (Vec3::X * x, Vec3::Y * y, Vec3::Z * z);
            [(x, y), (y, z), (z, x)]
                .into_iter()
                .flat_map(|(a, b)| {
                    let point = move |step: usize| {
                        let angle = step as f32 / ELLIPSE_SEGMENTS as f32 * TAU;
                        at + turn * (a * angle.cos() + b * angle.sin())
                    };
                    (0..ELLIPSE_SEGMENTS)
                        .map(move |step| Shape::Line { from: point(step), to: point(step + 1) })
                })
                .collect()
        }
    }
}

/// Lines from point to point, and back to the first if it's `closed`.
fn polyline(points: &[Vec3], closed: bool) -> Vec<Shape> {
    let mut shapes: Vec<Shape> =
        points.windows(2).map(|pair| Shape::Line { from: pair[0], to: pair[1] }).collect();
    if closed
        && let (Some(&first), Some(&last)) = (points.first(), points.last())
        && points.len() > 2
    {
        shapes.push(Shape::Line { from: last, to: first });
    }
    shapes
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One of each kind, apart from one another.
    const EVERY_KIND: &str = include_str!("../tests/zones/every_kind.zone.ron");

    fn every_kind() -> ZoneDocument {
        ZoneDocument::from_ron(EVERY_KIND).expect("reads")
    }

    #[test]
    fn every_kind_is_an_object_at_its_keyed_path() {
        let objects = ZoneObjects::of(&every_kind());
        let kinds: Vec<ObjectKind> = objects.objects.iter().map(|object| object.kind).collect();
        assert_eq!(kinds, ObjectKind::ALL, "one of each, in the pane's order");
        let paths: Vec<String> = objects.objects.iter().map(|object| object.path.to_string()).collect();
        assert_eq!(
            paths,
            [
                r#"placements["post"]"#,
                r#"instances["crate"]"#,
                r#"lights["lamp"]"#,
                r#"reflections["hall"]"#,
                r#"reflections["hall"].parallax"#,
                r#"triggers["door"]"#,
                r#"checkpoints["start"]"#,
                r#"spawners["player_start"]"#,
                r#"splines["rail"]"#,
                r#"exits["door"]"#,
                r#"volumes["haze"]"#,
                r#"weather_plan?.regions["drizzle"]"#,
                r#"water["pond"]"#,
                r#"water["pond"].probe"#,
                r#"rooms["cellar"]"#,
                r#"ambient?.posts["well"]"#,
                r#"ambient?.walks["lap"]"#,
            ]
        );
        for object in &objects.objects {
            let shapes = object.shapes.len();
            assert!(object.kind == ObjectKind::Placement || shapes > 0, "{:?} is marked", object.kind);
        }
    }

    #[test]
    fn a_lake_is_picked_by_its_outline_and_the_sea_has_no_place() {
        let mut document = every_kind();
        let pond = document.water[0].clone();
        document.water.push(sapphire_zone::WaterBody {
            name: "sea".to_owned(),
            shape: WaterShape::Ocean { height: -1.0 },
            probe: None,
            ..pond
        });
        let objects = ZoneObjects::of(&document);
        let lake = &objects.objects[objects.find(&keyed(ObjectKind::WaterBody, "pond")).expect("the pond")];
        assert_eq!(lake.handles.len(), 4, "each corner of its outline");
        assert_eq!(lake.shapes.len(), 4, "and the outline, closed");
        let sea = &objects.objects[objects.find(&keyed(ObjectKind::WaterBody, "sea")).expect("listed")];
        assert!(sea.handles.is_empty() && sea.shapes.is_empty(), "with no place or mark");
        assert!(objects.find(&keyed(ObjectKind::WaterProbe, "sea")).is_none(), "the sea reflects the sky");
    }

    #[test]
    fn an_exit_without_a_door_has_no_place() {
        let mut document = every_kind();
        document.triggers[0].event = "something else".to_owned();
        let objects = ZoneObjects::of(&document);
        let exit = &objects.objects[objects.find(&keyed(ObjectKind::Exit, "door")).expect("listed")];
        assert_eq!(exit.at(), None, "with no place");
        assert!(objects.find(&keyed(ObjectKind::Trigger, "door")).is_some());
    }
}
