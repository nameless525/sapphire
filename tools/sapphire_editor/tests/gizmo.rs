//! The gizmo's modes and snapping (M6 T9b), driven as a script drives them, with no window and no GPU: each
//! handle taken hold of by a ray from an eye, dragged by another, and let go, writes exactly the numbers expected,
//! as one step of undo; a snap to the ground lands on the colliders; a duplicate and a delete keep the colliders in
//! step.

use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};

use sapphire_colliders::{ColliderDocument, Rotation};
use sapphire_editor::commands::{Command, SCRIPT_SUFFIX, Script};
use sapphire_editor::gizmo::{Spot, ViewCamera};
use sapphire_editor::handles::{Handle, aside_of, snap};
use sapphire_editor::{Editor, Options};
use sapphire_math::{Quat, Vec3, det};
use sapphire_render::CameraData;
use sapphire_zone::{LightShape, Volume, WaterShape, ZoneDocument, ZoneShape};

const ZONE: &str = r#"// What the gizmo's tests take hold of.
(
    type: "sapphire::zone::ZoneDocument",
    version: 2,
    data: (
        grid: (cell_size: 16.0),
        placements: [
            (id: "post", prefab: "2c0ce8b4-10c6-4cdd-b546-4249a9601188", at: (0.0, 3.0, 0.0), colliders: ["post"]),
            (id: "gate", prefab: "2c0ce8b4-10c6-4cdd-b546-4249a9601188", at: (6.0, 0.0, 0.0), turn: 30.0, colliders: ["gate"]),
            (id: "left", prefab: "2c0ce8b4-10c6-4cdd-b546-4249a9601188", at: (-6.0, 0.0, 0.0), colliders: ["wall"]),
            (id: "right", prefab: "2c0ce8b4-10c6-4cdd-b546-4249a9601188", at: (-6.0, 0.0, 0.0), colliders: ["wall"]),
        ],
        instances: [(id: "crate", mesh: "f6dcae8b-5937-4299-ba5e-e2f026fc2762", at: (6.1, 0.0, 6.0))],
        lights: [
            (id: "lamp", position: (0.0, 2.0, 6.0), color: (1.0, 1.0, 1.0), intensity: 4.0, range: 3.0),
            (id: "beam", position: (6.0, 4.0, 12.0), color: (1.0, 1.0, 1.0), intensity: 4.0, range: 5.0, shape: Spot(direction: (0.0, -1.0, 0.0), inner_degrees: 20.0, outer_degrees: 40.0)),
        ],
        colliders: Some("6a1f2e3d-4c5b-4a69-8778-9f0e1d2c3b4a"),
        spawners: [(name: "player_start", position: (12.0, 0.0, 0.0), spawns: "player")],
        triggers: [(name: "door", position: (12.0, 1.0, 6.0), volume: Box(half_extents: (1.0, 1.0, 1.0)), event: "door")],
        checkpoints: [(name: "start", position: (12.0, 0.0, 12.0), volume: Sphere(radius: 1.0))],
        splines: [(name: "rail", points: [(0.0, 0.0, 12.0), (2.0, 0.0, 12.0)])],
        volumes: [(id: "haze", name: "haze", position: (0.0, 1.0, 18.0), shape: Ellipsoid(radii: (2.0, 1.0, 2.0)), look: "haze.ron", blend: 1.0)],
        weather_plan: Some((
            states: "presets.weather.ron",
            seed: 1,
            schedule: [(from: 0.0, states: [(state: "clear")])],
            regions: [(name: "drizzle", position: (6.0, 1.0, 18.0), shape: Box(half_extents: (2.0, 1.0, 2.0), yaw: 30.0), state: "rain")],
        )),
        water: [
            (name: "pond", shape: Lake(outline: [(10.0, 14.0), (14.0, 14.0), (14.0, 18.0)], height: 0.0)),
            (name: "brook", shape: River(points: [(at: (18.0, 0.0, 0.0), width: 2.0), (at: (18.0, 0.0, 6.0), width: 2.0)])),
        ],
    ),
)
"#;

const COLLIDERS: &str = r#"(
    type: "sapphire::colliders::ColliderDocument",
    version: 1,
    data: (
        colliders: [
            // The ground, its top at 0.25.
            (name: "ground", shape: Box(half_extents: (40.0, 0.5, 40.0)), position: (0.0, -0.25, 0.0)),
            (name: "post", shape: Box(half_extents: (0.5, 2.0, 0.5)), position: (0.0, 5.0, 0.0)),
            (name: "gate", shape: Box(half_extents: (1.0, 1.0, 0.25)), position: (8.0, 1.0, 0.0)),
            (name: "wall", shape: Box(half_extents: (0.5, 1.0, 2.0)), position: (-6.0, 1.0, 0.0)),
        ],
    ),
)
"#;

const COLLIDERS_META: &str = r#"(
    type: "sapphire::assets::AssetMeta",
    version: 1,
    data: (
        id: "6a1f2e3d-4c5b-4a69-8778-9f0e1d2c3b4a",
        importer: Colliders(()),
    ),
)
"#;

/// Where every ray of these tests starts: above and aside of all of it.
const EYE: Vec3 = Vec3::new(3.0, 25.0, -10.0);

/// A directory of this test's own, gone when it ends.
struct TestDir(PathBuf);

impl TestDir {
    fn new(name: &str) -> Self {
        static NEXT: AtomicU32 = AtomicU32::new(0);
        let unique = NEXT.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir()
            .join(format!("sapphire_editor_gizmo_{name}_{}_{unique}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("a temp dir");
        Self(dir)
    }

    fn write(&self, name: &str, text: &str) -> PathBuf {
        let path = self.0.join(name);
        fs::create_dir_all(path.parent().expect("a parent")).expect("its folder");
        fs::write(&path, text).expect("a test file");
        path
    }
}

impl Drop for TestDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// The zone and its colliders in a folder of their own, and an editor on the zone as `--run` starts one.
struct Zone {
    dir: TestDir,
    zone: PathBuf,
    colliders: PathBuf,
    editor: Editor,
}

impl Zone {
    fn new(name: &str) -> Self {
        Self::of(name, ZONE)
    }

    /// One on a zone that reads `text`.
    fn of(name: &str, text: &str) -> Self {
        let dir = TestDir::new(name);
        let zone = dir.write("z/handles.zone.ron", text);
        let colliders = dir.write("z/handles.colliders.ron", COLLIDERS);
        dir.write("z/handles.colliders.ron.meta", COLLIDERS_META);
        let editor = Editor::new(Options {
            file: Some(zone.clone()),
            content: Some(dir.0.join("no cook here")),
            link: Some("127.0.0.1:9".parse().expect("an address")),
            pipeline: None,
            game: None,
        });
        Self { dir, zone, colliders, editor }
    }

    /// Runs `commands` from a script in the folder, as `--run` does.
    #[track_caller]
    fn run(&mut self, commands: &str) {
        let path = self.dir.0.join(format!("twin{SCRIPT_SUFFIX}"));
        fs::write(&path, format!("(version: 1, commands: [{commands}])")).expect("a script");
        let script = Script::read(&path).unwrap_or_else(|error| panic!("{error}"));
        if let Err(error) = self.editor.run_script(&script) {
            panic!("{error}");
        }
    }

    /// Selects the object of the zone at `path`.
    #[track_caller]
    fn select(&mut self, path: &str) {
        self.run(&format!(r#"SelectObjects([(document: "z/handles.zone.ron", path: {path:?})])"#));
    }

    /// Takes hold of `handle` by a ray from the eye through `from`, drags it by one through `to`, and lets go.
    #[track_caller]
    fn drag(&mut self, handle: &str, from: Vec3, to: Vec3) {
        self.drag_through(handle, from, &[to]);
    }

    /// Takes hold of `handle` through `from`, drags it through each of `through` in turn, a frame each, and lets
    /// go.
    #[track_caller]
    fn drag_through(&mut self, handle: &str, from: Vec3, through: &[Vec3]) {
        let frames: Vec<String> =
            through.iter().map(|to| format!("DragTo({}), ", ray_through(*to))).collect();
        self.run(&format!("Grab(handle: {handle}, {}), {}End", ray_through(from), frames.concat()));
    }

    fn text(&self) -> String {
        self.editor.current().expect("the zone").text().to_owned()
    }

    fn value(&self) -> ZoneDocument {
        ZoneDocument::from_ron(&self.text()).expect("the zone reads")
    }

    /// The colliders as the editor has them.
    fn colliders(&mut self) -> ColliderDocument {
        self.run("SaveAll");
        ColliderDocument::from_ron(&fs::read_to_string(&self.colliders).expect("saved")).expect("reads")
    }

    /// Undoes once, and holds both files to their bytes.
    #[track_caller]
    fn undo_is_the_files(&mut self) {
        self.run("Undo, SaveAll");
        assert_eq!(fs::read_to_string(&self.zone).expect("saved"), ZONE, "one undo is the zone's bytes");
        assert_eq!(fs::read_to_string(&self.colliders).expect("saved"), COLLIDERS, "and the colliders'");
    }
}

/// The ray from the eye through `point`, as a command spells it.
fn ray_through(point: Vec3) -> String {
    let [x, y, z] = EYE.to_array();
    let [dx, dy, dz] = (point - EYE).normalize().to_array();
    format!("from: ({x:?}, {y:?}, {z:?}), toward: ({dx:?}, {dy:?}, {dz:?})")
}

/// `turn` degrees about +Y of `offset`, as a yaw turns.
fn turned(offset: Vec3, degrees: f32) -> Vec3 {
    Quat::from_rotation_y(degrees.to_radians()) * offset
}

fn collider<'d>(colliders: &'d ColliderDocument, name: &str) -> &'d sapphire_colliders::ColliderEntry {
    colliders
        .colliders
        .iter()
        .find(|entry| entry.name == name)
        .unwrap_or_else(|| panic!("no collider {name}"))
}

// Move (and the world's and an object's own axes, and the grid).

/// An arrow along the zone's X moves the post one metre, the one number it moves and its collider with it.
#[test]
fn a_move_along_the_worlds_axis_writes_its_one_number() {
    let mut zone = Zone::new("move_world");
    zone.select(r#"placements["post"]"#);
    zone.drag("Move(X)", Vec3::new(0.5, 3.0, 0.0), Vec3::new(1.5, 3.0, 0.0));
    let text = zone.text();
    assert_eq!(text, ZONE.replacen("at: (0.0, 3.0, 0.0)", "at: (1.0, 3.0, 0.0)", 1), "one number's bytes");
    assert_eq!(zone.editor.current().expect("open").undo_label(), Some("move a placement"));
    assert_eq!(collider(&zone.colliders(), "post").position, [1.0, 5.0, 0.0], "its collider went with it");
    zone.undo_is_the_files();
}

/// With the object's own axes, the gate turned 30 degrees moves along its own X: whole grid steps along it, every
/// number it changes exactly where its turn puts it.
#[test]
fn a_move_along_its_own_axis_follows_its_turn() {
    let mut zone = Zone::new("move_local");
    zone.select(r#"placements["gate"]"#);
    zone.run("GizmoAxes(Local)");
    let own = (sapphire_zone::document::orientation(30.0, None) * Vec3::X).normalize();
    let at = Vec3::new(6.0, 0.0, 0.0);
    zone.drag("Move(X)", at + own * 0.5, at + own * 2.52);
    let expected = at + own * snap(2.02, 0.05);
    let gate = &zone.value().placements[1];
    assert_eq!(gate.at, expected.to_array(), "along its own X, two metres, to the bit");
    assert!(gate.at[2] < -0.5, "toward -Z as it's turned: {:?}", gate.at);
    zone.undo_is_the_files();
}

/// On a grid of half a metre the crate at x 6.1 dragged 0.6 lands on 6.5, and a grid of 0 leaves the drag as it
/// went.
#[test]
fn a_move_snaps_to_the_grid_it_is_given() {
    let mut zone = Zone::new("grid");
    zone.select(r#"instances["crate"]"#);
    zone.run("Snap(grid: 0.5, yaw: 1.0)");
    let at = Vec3::new(6.1, 0.0, 6.0);
    zone.drag("Move(X)", at, at + Vec3::X * 0.6);
    assert_eq!(zone.value().instances[0].at, [6.5, 0.0, 6.0]);
    assert_eq!(zone.editor.current().expect("open").undo_label(), Some("move an instance"));
    zone.run("Undo, Snap(grid: 0.0, yaw: 1.0)");
    zone.drag("Move(Z)", at, at + Vec3::Z * 0.23);
    let moved = zone.value().instances[0].at[2];
    assert!((moved - 6.23).abs() < 1e-3, "off the grid, as dragged: {moved}");
    assert!(zone.editor.run(Command::Snap { grid: -1.0, yaw: 1.0 }).is_err(), "a step is 0 or more");
}

/// A spline's point is a ball of its own, moved over its level onto the grid; the spline's arrows move every point.
#[test]
fn a_splines_point_moves_on_its_level() {
    let mut zone = Zone::new("spline_point");
    zone.select(r#"splines["rail"]"#);
    let point = Vec3::new(2.0, 0.0, 12.0);
    zone.drag("Point(1)", point, point + Vec3::new(1.52, 0.0, -0.49));
    assert_eq!(zone.value().splines[0].points, [[0.0, 0.0, 12.0], [3.5, 0.0, 11.5]]);
    assert_eq!(zone.editor.current().expect("open").undo_label(), Some("move a point of a spline"));
    zone.undo_is_the_files();
    zone.drag("Move(Y)", Vec3::new(0.0, 0.0, 12.0), Vec3::new(0.0, 1.0, 12.0));
    assert_eq!(zone.value().splines[0].points, [[0.0, 1.0, 12.0], [2.0, 1.0, 12.0]], "the whole spline");
}

/// A river's point moves the same way, and a lake's corner, which is written flat, at the lake's height.
#[test]
fn a_rivers_and_a_lakes_points_move_on_their_level() {
    let mut zone = Zone::new("water_points");
    zone.select(r#"water["brook"]"#);
    let point = Vec3::new(18.0, 0.0, 6.0);
    zone.drag("Point(1)", point, point + Vec3::new(-1.0, 0.0, 1.0));
    let WaterShape::River { points } = &zone.value().water[1].shape else { panic!("a river") };
    assert_eq!(points[1].at, [17.0, 0.0, 7.0]);
    assert_eq!(points[1].width, 2.0, "its width stays");
    zone.undo_is_the_files();
    zone.select(r#"water["pond"]"#);
    let corner = Vec3::new(14.0, 0.0, 18.0);
    zone.drag("Point(2)", corner, corner + Vec3::new(0.5, 0.0, 1.0));
    let WaterShape::Lake { outline, height } = &zone.value().water[0].shape else { panic!("a lake") };
    assert_eq!((outline[2], *height), ([14.5, 19.0], 0.0));
    zone.undo_is_the_files();
}

// Turn.

/// The ring turns the post about +Y by where the pointer goes round it, to the degree: its collider standing over
/// it turns in place.
#[test]
fn the_ring_turns_by_the_degree() {
    let mut zone = Zone::new("yaw");
    zone.select(r#"placements["post"]"#);
    zone.run("GizmoMode(Turn)");
    let center = Vec3::new(0.0, 3.0, 0.0);
    zone.drag("Yaw", center + Vec3::X * 2.0, center + turned(Vec3::X * 2.0, 30.3));
    assert_eq!(zone.value().placements[0].turn, 30.0);
    assert_eq!(zone.editor.current().expect("open").undo_label(), Some("turn a placement"));
    let post = collider(&zone.colliders(), "post").clone();
    assert!(
        Vec3::from(post.position).distance(Vec3::new(0.0, 5.0, 0.0)) < 1e-5,
        "straight over it, in place"
    );
    assert_eq!(post.rotation, Rotation::AxisAngle { axis: [0.0, 1.0, 0.0], degrees: 30.0 });
    zone.undo_is_the_files();
}

/// With a yaw step of 15 degrees a turn of 52 lands on 45: and the gate, turned from 30, takes its collider round
/// with it, about where it stands.
#[test]
fn a_turn_snaps_to_the_yaw_step_and_takes_its_colliders_round() {
    let mut zone = Zone::new("yaw_snap");
    zone.select(r#"placements["gate"]"#);
    zone.run("GizmoMode(Turn), Snap(grid: 0.05, yaw: 15.0)");
    let center = Vec3::new(6.0, 0.0, 0.0);
    zone.drag("Yaw", center + Vec3::X * 2.0, center + turned(Vec3::X * 2.0, 52.0));
    assert_eq!(zone.value().placements[1].turn, 75.0, "30 and 45");
    let gate = collider(&zone.colliders(), "gate").clone();
    assert_eq!(gate.position, carried(Vec3::new(2.0, 1.0, 0.0), 45.0), "about where it stands, to the bit");
    assert_eq!(gate.rotation, Rotation::AxisAngle { axis: [0.0, 1.0, 0.0], degrees: 45.0 });
    zone.undo_is_the_files();
}

/// Where the gate's collider, `offset` from where the gate stands, goes when the gate turns `degrees`, as the
/// document crate turns it.
fn carried(offset: Vec3, degrees: f32) -> [f32; 3] {
    let by = det::quat_from_axis_angle(Vec3::Y, degrees.to_radians());
    (Vec3::new(6.0, 0.0, 0.0) + det::quat_rotate(by, offset)).to_array()
}

/// A turn's colliders are written from where the drag began: a drag out to 30 and back to 20 leaves the gate's
/// collider turned 20, beside it where a turn of 20 puts it, and one drag to the same end in 1 frame or in 7
/// writes the same bytes, in one step.
#[test]
fn a_turns_colliders_go_by_where_the_drag_ends() {
    let center = Vec3::new(6.0, 0.0, 0.0);
    let ring = |degrees: f32| center + turned(Vec3::X * 2.0, degrees);
    let mut back = Zone::new("yaw_back");
    back.select(r#"placements["gate"]"#);
    back.run("GizmoMode(Turn)");
    back.drag_through("Yaw", ring(0.0), &[ring(30.0), ring(20.0)]);
    assert_eq!(back.value().placements[1].turn, 50.0);
    let gate = collider(&back.colliders(), "gate").clone();
    assert_eq!(gate.position, carried(Vec3::new(2.0, 1.0, 0.0), 20.0));
    assert_eq!(gate.rotation, Rotation::AxisAngle { axis: [0.0, 1.0, 0.0], degrees: 20.0 });
    back.undo_is_the_files();

    let mut written = Vec::new();
    for frames in [1_u8, 7] {
        let mut zone = Zone::new(&format!("yaw_frames_{frames}"));
        zone.select(r#"placements["gate"]"#);
        zone.run("GizmoMode(Turn)");
        let through: Vec<Vec3> =
            (1..=frames).map(|frame| ring(f32::from(frame) * 160.0 / f32::from(frames))).collect();
        zone.drag_through("Yaw", ring(0.0), &through);
        zone.run("SaveAll");
        let read = |path: &PathBuf| fs::read_to_string(path).expect("saved");
        written.push((read(&zone.zone), read(&zone.colliders)));
        zone.undo_is_the_files();
    }
    assert_eq!(written[0], written[1], "1 frame and 7 write the same bytes");
    let colliders = ColliderDocument::from_ron(&written[0].1).expect("reads");
    let gate = collider(&colliders, "gate");
    assert_eq!(gate.rotation, Rotation::AxisAngle { axis: [0.0, 1.0, 0.0], degrees: 160.0 });
    assert!(written[0].0.contains("turn: 190.0"), "on past 180");
}

/// A spawner's ring turns its `yaw_degrees`, and a yawed box's its shape's `yaw`.
#[test]
fn the_ring_turns_a_spawner_and_a_yawed_box() {
    let mut zone = Zone::new("yaw_kinds");
    zone.run("GizmoMode(Turn)");
    zone.select(r#"spawners["player_start"]"#);
    let center = Vec3::new(12.0, 0.0, 0.0);
    zone.drag("Yaw", center + Vec3::X, center + turned(Vec3::X, -90.0));
    assert_eq!(zone.value().spawners[0].yaw_degrees, -90.0);
    zone.undo_is_the_files();
    zone.select(r#"weather_plan?.regions["drizzle"]"#);
    let center = Vec3::new(6.0, 1.0, 18.0);
    zone.drag("Yaw", center + Vec3::X * 2.0, center + turned(Vec3::X * 2.0, 10.0));
    let ZoneShape::Box { yaw, .. } = zone.value().weather_plan.expect("a plan").regions[0].shape else {
        panic!("a box")
    };
    assert_eq!(yaw, 40.0);
    zone.undo_is_the_files();
}

/// A spotlight's cone opens by its rim's ball, outer and inner each, to the degree.
#[test]
fn a_spotlights_cone_opens_by_its_rim() {
    let mut zone = Zone::new("cone");
    zone.select(r#"lights["beam"]"#);
    zone.run("GizmoMode(Turn)");
    let (origin, along) = (Vec3::new(6.0, 4.0, 12.0), Vec3::NEG_Y);
    let aside = aside_of(along);
    let rim = |degrees: f32| {
        let angle = degrees.to_radians();
        origin + (along * angle.cos() + aside * angle.sin()) * 3.0
    };
    zone.drag("Cone", rim(40.0), rim(50.2));
    zone.drag("InnerCone", rim(20.0), rim(25.0));
    let LightShape::Spot { outer_degrees, inner_degrees, .. } = zone.value().lights[1].shape else {
        panic!("a spot")
    };
    assert_eq!((outer_degrees, inner_degrees), (50.0, 25.0));
    zone.run("Undo");
    zone.undo_is_the_files();
}

// Scale and sizes.

/// A bar scales the post along its own X by how far the pointer goes along it: from 1 metre out to 2, twice.
#[test]
fn a_bar_scales_by_the_pointers_reach() {
    let mut zone = Zone::new("scale");
    zone.select(r#"placements["post"]"#);
    zone.run("GizmoMode(Scale)");
    let at = Vec3::new(0.0, 3.0, 0.0);
    zone.drag("Scale(X)", at + Vec3::X, at + Vec3::X * 2.0);
    assert_eq!(zone.value().placements[0].scale, [2.0, 1.0, 1.0]);
    assert_eq!(zone.editor.current().expect("open").undo_label(), Some("scale a placement"));
    zone.undo_is_the_files();
}

/// A yawed box's face moves along the box's own axis, an ellipsoid's along its own, a trigger's box along the
/// zone's, each a whole number of grid steps.
#[test]
fn a_face_of_a_box_or_an_ellipsoid_moves_out() {
    let mut zone = Zone::new("size");
    zone.run("GizmoMode(Scale)");
    zone.select(r#"weather_plan?.regions["drizzle"]"#);
    let center = Vec3::new(6.0, 1.0, 18.0);
    let own = turned(Vec3::X, 30.0);
    zone.drag("Size(X)", center + own * 2.0, center + own * 3.01);
    let ZoneShape::Box { half_extents, .. } = zone.value().weather_plan.expect("a plan").regions[0].shape
    else {
        panic!("a box")
    };
    assert_eq!(half_extents, [3.0, 1.0, 2.0]);
    zone.undo_is_the_files();
    zone.select(r#"volumes["haze"]"#);
    let center = Vec3::new(0.0, 1.0, 18.0);
    zone.drag("Size(Y)", center + Vec3::Y, center + Vec3::Y * 1.5);
    let ZoneShape::Ellipsoid { radii, .. } = zone.value().volumes[0].shape else { panic!("an ellipsoid") };
    assert_eq!(radii, [2.0, 1.5, 2.0]);
    zone.undo_is_the_files();
    zone.select(r#"triggers["door"]"#);
    let center = Vec3::new(12.0, 1.0, 6.0);
    zone.drag("Size(Z)", center + Vec3::Z, center + Vec3::Z * 0.5);
    let Volume::Box { half_extents } = zone.value().triggers[0].volume else { panic!("a box") };
    assert_eq!(half_extents, [1.0, 1.0, 0.5]);
    zone.undo_is_the_files();
}

/// A sphere's ball sets its radius, and a light's its range.
#[test]
fn a_radius_and_a_range_move_out() {
    let mut zone = Zone::new("radius");
    zone.run("GizmoMode(Scale)");
    zone.select(r#"checkpoints["start"]"#);
    let center = Vec3::new(12.0, 0.0, 12.0);
    zone.drag("Radius", center + Vec3::X, center + Vec3::X * 1.5);
    assert_eq!(zone.value().checkpoints[0].volume, Volume::Sphere { radius: 1.5 });
    zone.undo_is_the_files();
    zone.select(r#"lights["lamp"]"#);
    let center = Vec3::new(0.0, 2.0, 6.0);
    zone.drag("Range", center + Vec3::X * 3.0, center + Vec3::X * 4.0);
    assert_eq!(zone.value().lights[0].range, 4.0);
    assert_eq!(zone.editor.current().expect("open").undo_label(), Some("set the range of a light"));
    zone.undo_is_the_files();
}

/// What the gizmo shows is what it can take: a handle the mode doesn't have is refused, and a press shown on the
/// cone's ball picks the cone.
#[test]
fn the_gizmo_shows_what_its_mode_has() {
    let mut zone = Zone::new("modes");
    zone.select(r#"lights["beam"]"#);
    let said = zone
        .editor
        .run(
            Command::parse("Grab(handle: Cone, from: (0.0, 9.0, 0.0), toward: (0.0, -1.0, 0.0))")
                .expect("a command"),
        )
        .expect_err("not in Move");
    assert!(said.contains("Cone") && said.contains("Move"), "{said}");
    zone.run("GizmoMode(Turn)");
    let camera = ViewCamera::of(&CameraData::look_at(EYE, Vec3::new(6.0, 2.0, 12.0), Vec3::Y, 0.8, 1.6, 0.1));
    let rect = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(1120.0, 700.0));
    let gizmo = zone.editor.gizmo_seen(&camera).expect("on the beam");
    let handles: Vec<Handle> = gizmo.handles.iter().map(|(handle, _)| *handle).collect();
    assert_eq!(handles, [Handle::Cone, Handle::InnerCone], "no ring: a light has no yaw");
    let Some(&(_, Spot::Ball(ball))) = gizmo.handles.first() else { panic!("a ball") };
    let at = camera.to_screen(ball, rect).expect("in view");
    assert_eq!(gizmo.pick(&camera, rect, at), Some(Handle::Cone));
    zone.run("GizmoMode(Scale)");
    zone.select(r#"placements["post"]"#);
    let gizmo = zone.editor.gizmo_seen(&camera).expect("on the post");
    assert_eq!(gizmo.handles.len(), 3, "three bars");
    assert!(
        zone.editor.run(Command::DragTo { from: [0.0; 3], toward: [0.0, -1.0, 0.0] }).is_err(),
        "nothing held"
    );
    assert_eq!(zone.text(), ZONE, "and nothing written");
}

// The ground.

/// The post, three metres up, snaps down onto the ground's top, its own collider left out of the ray and taken
/// down with it, as one step.
#[test]
fn a_ground_snapped_placement_sits_on_the_collider() {
    let mut zone = Zone::new("ground");
    zone.select(r#"placements["post"]"#);
    zone.run("SnapToGround");
    let post = zone.value().placements[0].at;
    assert!((post[1] - 0.25).abs() <= 0.001, "within a millimetre of the ground's top: {post:?}");
    assert_eq!((post[0], post[2]), (0.0, 0.0));
    let collider = collider(&zone.colliders(), "post").position;
    assert!(
        (collider[1] - (5.0 - (3.0 - post[1]))).abs() < 1e-5,
        "its collider came down as far: {collider:?}"
    );
    zone.undo_is_the_files();
    zone.select(r#"splines["rail"]"#);
    let said = zone.editor.run(Command::SnapToGround).expect_err("a spline has no position");
    assert!(said.contains("position"), "{said}");
}

/// A placement with no ground under it is refused, and nothing is written or made a step.
#[test]
fn nothing_below_is_no_snap() {
    let mut zone = Zone::of("ground_none", &ZONE.replacen("at: (0.0, 3.0, 0.0)", "at: (90.0, 3.0, 0.0)", 1));
    zone.select(r#"placements["post"]"#);
    let said = zone.editor.run(Command::SnapToGround).expect_err("off the ground");
    assert!(said.contains("below"), "{said}");
    assert_eq!(zone.text(), ZONE.replacen("at: (0.0, 3.0, 0.0)", "at: (90.0, 3.0, 0.0)", 1));
    assert_eq!(zone.editor.current().expect("open").undo_label(), None, "no step");
}

// Duplicate and delete.

/// A duplicate of the post takes the ID given and names a copy of its collider, standing where its does; one undo
/// takes both away; and moving the copy moves its own collider, not the post's.
#[test]
fn a_duplicate_keeps_its_colliders_in_step() {
    let mut zone = Zone::new("duplicate");
    zone.select(r#"placements["post"]"#);
    zone.run(r#"DuplicateAs(["post-copy"])"#);
    let value = zone.value();
    let copy = value.placements.last().expect("the copy");
    assert_eq!(
        (copy.id.as_str(), copy.at, &copy.colliders[..]),
        ("post-copy", [0.0, 3.0, 0.0], &["post-2".to_owned()][..])
    );
    let selected: Vec<String> =
        zone.editor.selection().iter().map(|object| object.path.to_string()).collect();
    assert_eq!(selected, [r#"placements["post-copy"]"#], "the copy is selected");
    let colliders = zone.colliders();
    assert_eq!(collider(&colliders, "post-2").position, collider(&colliders, "post").position);
    assert_eq!(colliders.colliders.len(), 5);
    zone.undo_is_the_files();
    zone.run("Redo");
    zone.select(r#"placements["post-copy"]"#);
    zone.drag("Move(X)", Vec3::new(0.5, 3.0, 0.0), Vec3::new(2.5, 3.0, 0.0));
    let colliders = zone.colliders();
    assert_eq!(collider(&colliders, "post-2").position, [2.0, 5.0, 0.0], "the copy's own");
    assert_eq!(collider(&colliders, "post").position, [0.0, 5.0, 0.0], "the post's stays");
    zone.select(r#"spawners["player_start"]"#);
    zone.run("Duplicate");
    let names: Vec<String> = zone.value().spawners.iter().map(|spawner| spawner.name.clone()).collect();
    assert_eq!(names, ["player_start", "player_start-2"], "a fresh name");
    let said = zone.editor.run(Command::parse(r#"DuplicateAs(["gate"])"#).expect("a command"));
    assert!(said.is_ok(), "a spawner takes no ID: {said:?}");
    zone.select(r#"placements["gate"]"#);
    let taken =
        zone.editor.run(Command::parse(r#"DuplicateAs(["post"])"#).expect("a command")).expect_err("taken");
    assert!(taken.contains("post"), "{taken}");
}

/// A delete takes out what it's asked and the colliders only it named: a collider another placement still names
/// stays; one undo puts both files back.
#[test]
fn a_delete_keeps_its_colliders_in_step() {
    let mut zone = Zone::new("delete");
    zone.select(r#"placements["left"]"#);
    zone.run("DeleteObjects");
    assert!(zone.value().placements.iter().all(|placement| placement.id != "left"));
    assert!(zone.editor.selection().is_empty(), "nothing gone is selected");
    let colliders = zone.colliders();
    assert!(colliders.colliders.iter().any(|entry| entry.name == "wall"), "the right still names it");
    zone.undo_is_the_files();
    zone.select(r#"placements["post"]"#);
    zone.run("DeleteObjects");
    let colliders = zone.colliders();
    assert!(colliders.colliders.iter().all(|entry| entry.name != "post"), "only the post named it");
    assert_eq!(zone.editor.current().expect("open").undo_label(), Some("delete"));
    zone.undo_is_the_files();
}

// A problem's row (T9a's owed item).

/// Going to a field, as a problem's row does, selects the entry it's of, by its key, so the gizmo is on it.
#[test]
fn going_to_a_field_selects_its_entry() {
    let mut zone = Zone::new("field");
    zone.run(r#"SelectField((document: "z/handles.zone.ron", path: "spawners[0].position"))"#);
    let selected: Vec<String> =
        zone.editor.selection().iter().map(|object| object.path.to_string()).collect();
    assert_eq!(selected, [r#"spawners["player_start"]"#]);
    zone.run(r#"SelectField((document: "z/handles.zone.ron", path: "grid.cell_size"))"#);
    let still: Vec<String> = zone.editor.selection().iter().map(|object| object.path.to_string()).collect();
    assert_eq!(still, selected, "a field of the zone itself selects nothing new");
    zone.run(r#"SelectField((document: "z/handles.zone.ron", path: "weather_plan.regions[0].shape"))"#);
    let region: Vec<String> = zone.editor.selection().iter().map(|object| object.path.to_string()).collect();
    assert_eq!(region, [r#"weather_plan?.regions["drizzle"]"#], "the entry, by its key");
}
