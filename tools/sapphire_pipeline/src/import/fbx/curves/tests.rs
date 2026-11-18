//! The evaluation against ufbx's own, tick by tick, on every FBX fixture with curves and on
//! hand-written files for what Blender doesn't write: each interpolation, tangent and
//! extrapolation, rotation orders, pivots, pre- and post-rotations, other axes and units, layers of
//! every blend mode, and a blend channel.

use std::fmt::Write as _;
use std::path::Path;

use glam::{DQuat, DVec3};

use super::*;
use crate::import::fbx::load;
use crate::import::fbx::tests::{Y_UP, ascii_fbx};

/// FBX's time units a second, as files write them.
const KTIME: f64 = 46_186_158_000.0;

/// How far a channel may move what it drives before FBX stays presentation-only (the plan's kill
/// line, M6 A2): a quarter of a millimeter.
const KILL_MM: f64 = 0.25;

/// What a rotation or a scale is measured at: the end of a bone this far off, longer than any in
/// these fixtures (the tallest rig stands 2 m).
const REACH_M: f64 = 2.0;

fn fixture(name: &str) -> Vec<u8> {
    std::fs::read(Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures/fbx").join(name)).unwrap()
}

/// One key: seconds, value, FBX's `KeyAttrFlags` and its four `KeyAttrDataFloat`s.
#[derive(Clone, Copy)]
struct Key(f64, f64, u32, [f32; 4]);

const CONSTANT: u32 = 0x2;
const CONSTANT_NEXT: u32 = 0x2 | 0x100;
const LINEAR: u32 = 0x4;
const CUBIC_AUTO: u32 = 0x8 | 0x100;
const CUBIC_AUTO_CLAMPED: u32 = 0x8 | 0x100 | 0x1000;
const CUBIC_USER: u32 = 0x8 | 0x400;
const CUBIC_BROKEN: u32 = 0x8 | 0x400 | 0x800;
const CUBIC_TCB: u32 = 0x8 | 0x200;

fn key(time: f64, value: f64, flags: u32) -> Key {
    Key(time, value, flags, [0.0; 4])
}

/// Extrapolation as FBX writes it: `C`onstant, `K` (slope), `R`epeat, `M`irror or `A` (relative
/// repeat), with a repeat count (−1 forever).
type Extrapolation = Option<(char, i32)>;

/// A hand-written scene: models, their curves, layers and stacks.
#[derive(Default)]
struct Scene {
    objects: String,
    connections: String,
    next_id: u64,
}

impl Scene {
    fn id(&mut self) -> u64 {
        self.next_id += 1;
        1000 + self.next_id
    }

    /// A limb node under `parent` (0, the scene's root) with `properties` (`Properties70` lines).
    fn model(&mut self, name: &str, parent: u64, properties: &[&str]) -> u64 {
        let id = self.id();
        let properties: String = properties.iter().map(|line| format!("\t\t\tP: {line}\n")).collect();
        let _ = write!(
            self.objects,
            "\tModel: {id}, \"Model::{name}\", \"LimbNode\" {{\n\t\tVersion: 232\n\t\tProperties70:  {{\n{properties}\t\t}}\n\t}}\n"
        );
        let _ = writeln!(self.connections, "\tC: \"OO\",{id},{parent}");
        id
    }

    /// A stack over `range` seconds, if given.
    fn stack(&mut self, name: &str, range: Option<(f64, f64)>) -> u64 {
        let id = self.id();
        let local = range.map_or(String::new(), |(begin, end)| {
            format!(
                "\t\t\tP: \"LocalStart\", \"KTime\", \"Time\", \"\",{}\n\t\t\tP: \"LocalStop\", \"KTime\", \"Time\", \"\",{}\n",
                (begin * KTIME) as i64,
                (end * KTIME) as i64
            )
        });
        let _ = write!(
            self.objects,
            "\tAnimationStack: {id}, \"AnimStack::{name}\", \"\" {{\n\t\tProperties70:  {{\n{local}\t\t}}\n\t}}\n"
        );
        id
    }

    /// A layer of `stack`, after those before it, with `properties` (weight, blend mode, …).
    fn layer(&mut self, stack: u64, properties: &[&str]) -> u64 {
        let id = self.id();
        let properties: String = properties.iter().map(|line| format!("\t\t\tP: {line}\n")).collect();
        let _ = write!(
            self.objects,
            "\tAnimationLayer: {id}, \"AnimLayer::L{id}\", \"\" {{\n\t\tProperties70:  {{\n{properties}\t\t}}\n\t}}\n"
        );
        let _ = writeln!(self.connections, "\tC: \"OO\",{id},{stack}");
        id
    }

    /// A curve node on `property` of `owner` in `layer`, its channels (`d|X`, …) each with its
    /// default and, where given, its curve.
    fn curves(
        &mut self,
        layer: u64,
        owner: u64,
        property: &str,
        channels: &[(&str, f64, Option<Curve>)],
    ) -> u64 {
        let id = self.id();
        let defaults: String = channels
            .iter()
            .map(|(name, default, _)| format!("\t\t\tP: \"{name}\", \"Number\", \"\", \"A\",{default}\n"))
            .collect();
        let _ = write!(
            self.objects,
            "\tAnimationCurveNode: {id}, \"AnimCurveNode::{property}\", \"\" {{\n\t\tProperties70:  {{\n{defaults}\t\t}}\n\t}}\n"
        );
        let _ =
            writeln!(self.connections, "\tC: \"OO\",{id},{layer}\n\tC: \"OP\",{id},{owner}, \"{property}\"");
        for (name, _, curve) in channels {
            if let Some(curve) = curve {
                let curve_id = self.curve(curve);
                let _ = writeln!(self.connections, "\tC: \"OP\",{curve_id},{id}, \"{name}\"");
            }
        }
        id
    }

    fn curve(&mut self, curve: &Curve) -> u64 {
        let id = self.id();
        let n = curve.keys.len();
        let join = |values: Vec<String>| values.join(",");
        let times = join(curve.keys.iter().map(|key| ((key.0 * KTIME) as i64).to_string()).collect());
        let values = join(curve.keys.iter().map(|key| key.1.to_string()).collect());
        let flags = join(curve.keys.iter().map(|key| key.2.to_string()).collect());
        // An ASCII file from FBX 7.2 on writes these as the floats' bits, as integers.
        let data = join(
            curve
                .keys
                .iter()
                .flat_map(|key| key.3)
                .map(|value| (value.to_bits() as i32).to_string())
                .collect(),
        );
        let extrapolation = |name: &str, extrapolation: Extrapolation| {
            extrapolation.map_or(String::new(), |(mode, count)| {
                format!("\t\t{name}:  {{\n\t\t\tType: {}\n\t\t\tRepetition: {count}\n\t\t}}\n", mode as u32)
            })
        };
        let _ = write!(
            self.objects,
            "\tAnimationCurve: {id}, \"AnimCurve::\", \"\" {{\n\t\tDefault: 0\n\t\tKeyVer: 4008\n\
             \t\tKeyTime: *{n} {{\n\t\t\ta: {times}\n\t\t}}\n\t\tKeyValueFloat: *{n} {{\n\t\t\ta: {values}\n\t\t}}\n\
             \t\tKeyAttrFlags: *{n} {{\n\t\t\ta: {flags}\n\t\t}}\n\t\tKeyAttrDataFloat: *{} {{\n\t\t\ta: {data}\n\t\t}}\n\
             \t\tKeyAttrRefCount: *{n} {{\n\t\t\ta: {}\n\t\t}}\n{}{}\t}}\n",
            n * 4,
            join(vec!["1".to_owned(); n]),
            extrapolation("Pre-Extrapolation", curve.pre),
            extrapolation("Post-Extrapolation", curve.post),
        );
        id
    }

    fn bytes(&self, axes: [i32; 6], unit_cm: f64) -> Vec<u8> {
        ascii_fbx(axes, unit_cm, &self.objects, &self.connections)
    }
}

#[derive(Clone)]
struct Curve {
    keys: Vec<Key>,
    pre: Extrapolation,
    post: Extrapolation,
}

fn curve(keys: &[Key]) -> Option<Curve> {
    Some(Curve { keys: keys.to_vec(), pre: None, post: None })
}

fn extrapolated(keys: &[Key], pre: Extrapolation, post: Extrapolation) -> Option<Curve> {
    Some(Curve { keys: keys.to_vec(), pre, post })
}

/// Every interpolation and tangent ufbx reads, and every extrapolation, on one node's channels;
/// a stack whose range runs past the keys on both sides, so the extrapolations play.
fn curves_scene() -> Vec<u8> {
    let mut scene = Scene::default();
    let stack = scene.stack("Curves", Some((-1.5, 3.5)));
    let layer = scene.layer(stack, &[]);
    let mover = scene.model(
        "Mover",
        0,
        &[
            "\"Lcl Translation\", \"Lcl Translation\", \"\", \"A\",1,2,3",
            "\"Lcl Rotation\", \"Lcl Rotation\", \"\", \"A\",10,20,30",
        ],
    );
    let user = [0.4f32, -1.5, 0.0, 0.0];
    let wavy = |flags: u32| {
        vec![
            key(0.0, 0.0, flags),
            key(0.3, 12.0, flags),
            key(0.55, -4.0, flags),
            key(1.0, 25.0, flags),
            key(1.4, 3.0, flags),
        ]
    };
    let mut tcb = wavy(CUBIC_TCB);
    tcb[1].3 = [0.3, -0.2, 0.5, 0.0];
    tcb[2].3 = [-0.4, 0.6, -0.1, 0.0];
    let mut user_keys = wavy(CUBIC_USER);
    for key in &mut user_keys {
        key.3 = user;
    }
    let mut broken = wavy(CUBIC_BROKEN);
    broken[2].3 = [3.0, -2.0, 0.0, 0.0];
    let mixed = [
        key(0.0, 1.0, CONSTANT),
        key(0.4, 2.0, CONSTANT_NEXT),
        key(0.8, 0.5, LINEAR),
        key(1.2, 3.0, CUBIC_AUTO),
        key(1.6, -1.0, LINEAR),
        key(2.0, 0.0, LINEAR),
    ];
    scene.curves(
        layer,
        mover,
        "Lcl Translation",
        &[
            ("d|X", 0.0, extrapolated(&wavy(CUBIC_AUTO), Some(('K', -1)), Some(('R', -1)))),
            ("d|Y", 0.0, extrapolated(&mixed, Some(('M', -1)), Some(('A', -1)))),
            ("d|Z", 0.0, extrapolated(&wavy(CUBIC_AUTO_CLAMPED), Some(('R', 1)), Some(('M', 2)))),
        ],
    );
    scene.curves(
        layer,
        mover,
        "Lcl Rotation",
        &[
            ("d|X", 0.0, extrapolated(&user_keys, Some(('A', 2)), Some(('K', -1)))),
            ("d|Y", 0.0, extrapolated(&broken, None, Some(('C', -1)))),
            ("d|Z", 0.0, extrapolated(&tcb, Some(('R', -1)), Some(('A', -1)))),
        ],
    );
    // A scale curve on one channel, its others the curve node's defaults.
    scene.curves(
        layer,
        mover,
        "Lcl Scaling",
        &[
            ("d|X", 1.0, curve(&[key(0.0, 1.0, LINEAR), key(1.0, 1.5, CUBIC_AUTO), key(2.0, 0.8, LINEAR)])),
            ("d|Y", 1.2, None),
            ("d|Z", 0.9, None),
        ],
    );
    scene.bytes(Y_UP, 100.0)
}

/// Pivots, offsets, pre- and post-rotations and every rotation order, under a Z-up file in
/// centimeters.
fn transforms_scene() -> Vec<u8> {
    let mut scene = Scene::default();
    let stack = scene.stack("Transforms", Some((0.0, 1.0)));
    let layer = scene.layer(stack, &[]);
    let top = scene.model(
        "Top",
        0,
        &[
            "\"Lcl Translation\", \"Lcl Translation\", \"\", \"A\",100,0,0",
            "\"Lcl Scaling\", \"Lcl Scaling\", \"\", \"A\",2,2,2",
        ],
    );
    let mut parent = top;
    let turn = curve(&[key(0.0, -30.0, CUBIC_AUTO), key(0.5, 70.0, CUBIC_AUTO), key(1.0, 150.0, CUBIC_AUTO)]);
    let order_lines: Vec<String> =
        (0..7).map(|order| format!("\"RotationOrder\", \"enum\", \"\", \"\",{order}")).collect();
    for (order, line) in order_lines.iter().enumerate() {
        let name = format!("Order{order}");
        let node = scene.model(
            &name,
            parent,
            &[
                line,
                "\"PreRotation\", \"Vector3D\", \"Vector\", \"\",0,15,90",
                "\"PostRotation\", \"Vector3D\", \"Vector\", \"\",5,30,0",
                "\"RotationActive\", \"bool\", \"\", \"\",1",
                "\"RotationPivot\", \"Vector3D\", \"Vector\", \"\",1,2,3",
                "\"ScalingPivot\", \"Vector3D\", \"Vector\", \"\",-1,0.5,2",
                "\"RotationOffset\", \"Vector3D\", \"Vector\", \"\",0.25,0,-1",
                "\"ScalingOffset\", \"Vector3D\", \"Vector\", \"\",0,0.75,0",
                "\"Lcl Translation\", \"Lcl Translation\", \"\", \"A\",10,20,30",
            ],
        );
        scene.curves(
            layer,
            node,
            "Lcl Rotation",
            &[("d|X", 20.0, turn.clone()), ("d|Y", -35.0, turn.clone()), ("d|Z", 50.0, turn.clone())],
        );
        scene.curves(
            layer,
            node,
            "Lcl Scaling",
            &[
                ("d|X", 1.0, None),
                ("d|Y", 1.0, curve(&[key(0.0, 1.0, LINEAR), key(1.0, 1.3, LINEAR)])),
                ("d|Z", 1.0, None),
            ],
        );
        parent = node;
    }
    // A node whose rotation space is off, which takes its rotation alone, in XYZ.
    let plain = scene.model(
        "Plain",
        parent,
        &[
            "\"RotationOrder\", \"enum\", \"\", \"\",5",
            "\"PreRotation\", \"Vector3D\", \"Vector\", \"\",0,15,90",
            "\"RotationActive\", \"bool\", \"\", \"\",0",
            // A geometric transform, which moves its mesh and not its children.
            "\"GeometricTranslation\", \"Vector3D\", \"Vector\", \"\",1,-2,3",
            "\"GeometricRotation\", \"Vector3D\", \"Vector\", \"\",10,-25,40",
            "\"GeometricScaling\", \"Vector3D\", \"Vector\", \"\",1,2,0.5",
        ],
    );
    scene.curves(
        layer,
        plain,
        "Lcl Rotation",
        &[("d|X", 0.0, turn.clone()), ("d|Y", 0.0, None), ("d|Z", 0.0, turn)],
    );
    let z_up = [2, 1, 1, -1, 0, 1];
    scene.bytes(z_up, 1.0)
}

/// Children that don't take their parent's scale as FBX's default does (`InheritType` 2, Maya's
/// segment scale compensate, and 0, by component), under a parent whose scale is animated and not
/// uniform, which the load's `Compensate` handling gives scale helpers, and under one whose
/// constant uniform scale it compensates in the children's conversion instead.
fn inherit_scene() -> Vec<u8> {
    let mut scene = Scene::default();
    let stack = scene.stack("Inherit", Some((0.0, 1.0)));
    let layer = scene.layer(stack, &[]);
    let stretch = scene.model(
        "Stretch",
        0,
        &[
            "\"Lcl Translation\", \"Lcl Translation\", \"\", \"A\",5,10,0",
            "\"Lcl Scaling\", \"Lcl Scaling\", \"\", \"A\",1.5,0.8,1.2",
        ],
    );
    let grow = |from: f64, to: f64| {
        curve(&[key(0.0, from, CUBIC_AUTO), key(0.6, to, CUBIC_AUTO), key(1.0, from, LINEAR)])
    };
    scene.curves(
        layer,
        stretch,
        "Lcl Scaling",
        &[("d|X", 1.5, grow(1.5, 2.5)), ("d|Y", 0.8, grow(0.8, 0.5)), ("d|Z", 1.2, None)],
    );
    let turn = curve(&[key(0.0, 0.0, CUBIC_AUTO), key(1.0, 80.0, CUBIC_AUTO)]);
    let add_child = |scene: &mut Scene, name: &str, parent: u64, inherit: &str| {
        let child = scene.model(
            name,
            parent,
            &[
                inherit,
                "\"Lcl Translation\", \"Lcl Translation\", \"\", \"A\",0,20,5",
                "\"Lcl Scaling\", \"Lcl Scaling\", \"\", \"A\",1,1.4,0.7",
            ],
        );
        scene.curves(
            layer,
            child,
            "Lcl Rotation",
            &[("d|X", 10.0, None), ("d|Y", 0.0, turn.clone()), ("d|Z", 0.0, turn.clone())],
        );
        child
    };
    let ignores = add_child(&mut scene, "Ignores", stretch, "\"InheritType\", \"enum\", \"\", \"\",2");
    add_child(&mut scene, "UnderIgnores", ignores, "\"InheritType\", \"enum\", \"\", \"\",2");
    add_child(&mut scene, "ByComponent", stretch, "\"InheritType\", \"enum\", \"\", \"\",0");
    add_child(&mut scene, "Normal", stretch, "\"InheritType\", \"enum\", \"\", \"\",1");
    // A constant uniform scale, which `Compensate` takes out of the child's conversion.
    let uniform = scene.model(
        "Uniform",
        0,
        &[
            "\"Lcl Translation\", \"Lcl Translation\", \"\", \"A\",-5,0,0",
            "\"Lcl Scaling\", \"Lcl Scaling\", \"\", \"A\",2,2,2",
        ],
    );
    add_child(&mut scene, "Compensated", uniform, "\"InheritType\", \"enum\", \"\", \"\",2");
    scene.bytes(Y_UP, 100.0)
}

/// The inheritance file loads as it says: scale helpers, a chain of them, and a compensated child,
/// so the spike measures each.
#[test]
fn the_inherit_file_has_scale_helpers_and_a_compensated_child() {
    let scene = load(&inherit_scene(), false, true).unwrap();
    let named = |name: &str| scene.nodes.iter().find(|node| &*node.element.name == name).unwrap();
    assert!(named("Stretch").scale_helper.is_some(), "an animated non-uniform scale gets a helper");
    // A helper under a node that takes its parent's scale by component: the chain of helpers'
    // scales a transform multiplies through.
    assert!(scene.nodes.iter().any(|node| node.is_scale_helper
        && node.parent.as_deref().is_some_and(|parent| parent.inherit_scale_node.is_some())));
    assert!(named("Uniform").scale_helper.is_none() && named("Compensated").has_adjust_transform);
}

/// Layers of every blend mode and accumulation, one with an animated weight, over two stacks.
fn layers_scene() -> Vec<u8> {
    let mut scene = Scene::default();
    let mover = scene.model("Mover", 0, &[]);
    let spin = |scale: f64| {
        curve(&[
            key(0.0, 0.0, CUBIC_AUTO),
            key(0.5, 60.0 * scale, CUBIC_AUTO),
            key(1.0, 170.0 * scale, CUBIC_AUTO),
        ])
    };
    let stretch = |to: f64| curve(&[key(0.0, 1.0, LINEAR), key(1.0, to, LINEAR)]);
    let base = |scene: &mut Scene, layer: u64| {
        scene.curves(
            layer,
            mover,
            "Lcl Translation",
            &[("d|X", 0.0, stretch(3.0)), ("d|Y", 2.0, None), ("d|Z", 0.0, None)],
        );
        scene.curves(
            layer,
            mover,
            "Lcl Rotation",
            &[("d|X", 0.0, spin(1.0)), ("d|Y", 10.0, spin(-0.5)), ("d|Z", 0.0, None)],
        );
        scene.curves(
            layer,
            mover,
            "Lcl Scaling",
            &[("d|X", 1.0, stretch(2.0)), ("d|Y", 1.0, None), ("d|Z", 1.0, stretch(0.5))],
        );
    };
    let others = |scene: &mut Scene, layer: u64| {
        scene.curves(
            layer,
            mover,
            "Lcl Translation",
            &[("d|X", 1.0, None), ("d|Y", 0.0, stretch(-2.0)), ("d|Z", 0.5, None)],
        );
        scene.curves(
            layer,
            mover,
            "Lcl Rotation",
            &[("d|X", 25.0, None), ("d|Y", 0.0, spin(0.3)), ("d|Z", -40.0, spin(0.7))],
        );
        scene.curves(
            layer,
            mover,
            "Lcl Scaling",
            &[("d|X", 1.5, None), ("d|Y", 1.0, stretch(1.8)), ("d|Z", 1.0, None)],
        );
    };
    // Additive, composing rotations and scales (FBX's default accumulation), then the same by
    // component, at a fixed weight.
    let stack = scene.stack("Additive", Some((0.0, 1.0)));
    let layer = scene.layer(stack, &[]);
    base(&mut scene, layer);
    let add = scene
        .layer(stack, &["\"Weight\", \"Number\", \"\", \"A\",60", "\"BlendMode\", \"enum\", \"\", \"\",0"]);
    others(&mut scene, add);
    let by_component = scene.layer(
        stack,
        &[
            "\"Weight\", \"Number\", \"\", \"A\",40",
            "\"BlendMode\", \"enum\", \"\", \"\",0",
            "\"RotationAccumulationMode\", \"enum\", \"\", \"\",1",
            "\"ScaleAccumulationMode\", \"enum\", \"\", \"\",1",
        ],
    );
    others(&mut scene, by_component);
    // A blend (override passthrough) whose weight is animated, then an override.
    let stack = scene.stack("Blended", Some((0.0, 1.0)));
    let layer = scene.layer(stack, &[]);
    base(&mut scene, layer);
    let blend = scene
        .layer(stack, &["\"Weight\", \"Number\", \"\", \"A\",100", "\"BlendMode\", \"enum\", \"\", \"\",2"]);
    others(&mut scene, blend);
    scene.curves(
        blend,
        blend,
        "Weight",
        &[("d|Weight", 100.0, curve(&[key(0.0, 0.0, LINEAR), key(1.0, 100.0, CUBIC_AUTO)]))],
    );
    let blend_by_component = scene.layer(
        stack,
        &[
            "\"Weight\", \"Number\", \"\", \"A\",35",
            "\"BlendMode\", \"enum\", \"\", \"\",2",
            "\"RotationAccumulationMode\", \"enum\", \"\", \"\",1",
            "\"ScaleAccumulationMode\", \"enum\", \"\", \"\",1",
        ],
    );
    others(&mut scene, blend_by_component);
    let over = scene
        .layer(stack, &["\"Weight\", \"Number\", \"\", \"A\",100", "\"BlendMode\", \"enum\", \"\", \"\",1"]);
    scene.curves(
        over,
        mover,
        "Lcl Translation",
        &[("d|X", 0.0, None), ("d|Y", 0.0, None), ("d|Z", 4.0, stretch(5.0))],
    );
    scene.bytes(Y_UP, 100.0)
}

/// A blend channel with a cubic curve on its `DeformPercent`.
fn morph_scene() -> Vec<u8> {
    let mut scene = Scene::default();
    scene.objects += "\tGeometry: 101, \"Geometry::Plate\", \"Mesh\" {\n\
         \t\tVertices: *9 { a: 0,0,0,1,0,0,0,1,0 }\n\
         \t\tPolygonVertexIndex: *3 { a: 0,1,-3 }\n\t}\n\
         \tModel: 100, \"Model::Plate\", \"Mesh\" {\n\t\tVersion: 232\n\t}\n\
         \tDeformer: 300, \"Deformer::Shapes\", \"BlendShape\" {\n\t\tVersion: 100\n\t}\n\
         \tDeformer: 301, \"SubDeformer::Smile\", \"BlendShapeChannel\" {\n\t\tVersion: 100\n\t\tDeformPercent: 0\n\t\tFullWeights: *1 { a: 100 }\n\t}\n\
         \tGeometry: 302, \"Geometry::Smile\", \"Shape\" {\n\t\tVersion: 100\n\
         \t\tIndexes: *1 { a: 0 }\n\t\tVertices: *3 { a: 0,0,0.5 }\n\t}\n";
    scene.connections += "\tC: \"OO\",100,0\n\tC: \"OO\",101,100\n\tC: \"OO\",300,101\n\tC: \"OO\",301,300\n\tC: \"OO\",302,301\n";
    let stack = scene.stack("Smile", Some((0.0, 1.0)));
    let layer = scene.layer(stack, &[]);
    let percent = curve(&[key(0.0, 0.0, CUBIC_AUTO), key(0.4, 80.0, CUBIC_AUTO), key(1.0, 35.0, CUBIC_AUTO)]);
    scene.curves(layer, 301, "DeformPercent", &[("d|DeformPercent", 0.0, percent)]);
    scene.bytes(Y_UP, 100.0)
}

/// The worst a channel came to: millimeters, and how many ticks were the same `f32` bits.
#[derive(Default)]
struct Worst {
    mm: f64,
    same: usize,
    ticks: usize,
}

impl Worst {
    fn add(&mut self, mm: f64, same: bool) {
        self.mm = self.mm.max(mm);
        self.same += usize::from(same);
        self.ticks += 1;
    }
}

/// The largest scale of `node`'s parent in model space at rest: a translation's meters per unit.
fn parent_scale(node: &Node) -> f64 {
    let Some(parent) = node.parent.as_deref() else { return 1.0 };
    let m = &parent.node_to_world;
    [DVec3::new(m.m00, m.m10, m.m20), DVec3::new(m.m01, m.m11, m.m21), DVec3::new(m.m02, m.m12, m.m22)]
        .iter()
        .map(|column| column.length())
        .fold(0.0, f64::max)
}

fn as_f32_bits(transform: &ufbx::Transform) -> Vec<u32> {
    let joint = crate::import::fbx::joint_transform(transform);
    joint.translation.iter().chain(&joint.rotation).chain(&joint.scale).map(|value| value.to_bits()).collect()
}

/// Each channel's worst difference from ufbx's evaluation over `scene`'s stacks, a row a channel,
/// at 30 and 60 ticks a second over each stack's range and half a second either side of it, and
/// the rest of every node.
#[allow(clippy::print_stdout, reason = "the spike's measurement, read with --nocapture")]
fn measure(name: &str, bytes: &[u8], rows: &mut Vec<(String, Worst)>) {
    let scene = load(bytes, false, true).unwrap();
    let mut by_interpolation = [0usize; 4];
    for key in scene.anim_curves.iter().flat_map(|curve| curve.keyframes.iter()) {
        by_interpolation[key.interpolation as usize] += 1;
    }
    println!("{name}: keys constant, constant from the next, linear, cubic: {by_interpolation:?}");
    // The rest: the evaluation with no animation against ufbx's `local_transform`.
    let mut worst = Worst::default();
    for node in scene.nodes.iter().filter(|node| !node.is_root) {
        let own = rest(node);
        worst.add(
            difference_mm(node, &own, &node.local_transform),
            as_f32_bits(&own) == as_f32_bits(&node.local_transform),
        );
    }
    rows.push((format!("{name} rest"), worst));

    for stack in scene.anim_stacks.iter() {
        let evaluation = Evaluation::stack(&scene, stack);
        let (begin, end) = (stack.time_begin, stack.time_end);
        let times: Vec<f64> = [30.0, 60.0]
            .into_iter()
            .flat_map(|rate: f64| {
                let ticks = ((end - begin + 1.0) * rate).ceil() as usize;
                (0..=ticks).map(move |tick| begin - 0.5 + tick as f64 / rate)
            })
            .collect();
        for node in scene.nodes.iter().filter(|node| !node.is_root) {
            let mut channels = [Worst::default(), Worst::default(), Worst::default()];
            for &time in &times {
                let own = evaluation.transform(node, time).unwrap();
                let theirs = ufbx::evaluate_transform(&stack.anim, node, time);
                let scale = parent_scale(node);
                let (t, r, s) = channel_differences(&own, &theirs);
                let (own_bits, their_bits) = (as_f32_bits(&own), as_f32_bits(&theirs));
                channels[0].add(t * scale * 1000.0, own_bits[0..3] == their_bits[0..3]);
                channels[1].add(r * REACH_M * 1000.0, own_bits[3..7] == their_bits[3..7]);
                channels[2].add(s * REACH_M * 1000.0, own_bits[7..10] == their_bits[7..10]);
            }
            for (channel, worst) in ["translation", "rotation", "scale"].into_iter().zip(channels) {
                rows.push((
                    format!("{name} {} {} {channel}", &*stack.element.name, &*node.element.name),
                    worst,
                ));
            }
        }
        for channel in scene.blend_channels.iter() {
            let mut worst = Worst::default();
            for &time in &times {
                let own = evaluation.blend_weight(channel, time).unwrap();
                let theirs = ufbx::evaluate_blend_weight(&stack.anim, channel, time);
                // A weight of 1 moves its target's whole offset, 0.5 m here.
                let mm = (own - theirs).abs() * 0.5 * 1000.0;
                let mm = if mm.is_finite() { mm } else { f64::INFINITY };
                worst.add(mm, (own as f32).to_bits() == (theirs as f32).to_bits());
            }
            rows.push((format!("{name} {} {} weight", &*stack.element.name, &*channel.element.name), worst));
        }
    }
}

/// Translation (units), rotation (radians) and scale (ratio) differences.
fn channel_differences(a: &ufbx::Transform, b: &ufbx::Transform) -> (f64, f64, f64) {
    let v = |v: ufbx::Vec3| DVec3::new(v.x, v.y, v.z);
    let q = |q: ufbx::Quat| DQuat::from_xyzw(q.x, q.y, q.z, q.w);
    let (qa, qb) = (q(a.rotation), q(b.rotation));
    // Neither is normalized: compare the turns, and how far each is from a unit. Two unit rotations a
    // chord `c` apart turn `4 asin(c / 2)` apart, about `2c`, which keeps its precision near zero.
    let (na, nb) = (qa.normalize(), qb.normalize());
    let nb = if na.dot(nb) < 0.0 { -nb } else { nb };
    let turn = 4.0 * ((na - nb).length() / 2.0).min(1.0).asin();
    let length = (qa.length() - qb.length()).abs();
    let differences = (
        v(a.translation).distance(v(b.translation)),
        turn + length,
        (v(a.scale) - v(b.scale)).abs().max_element(),
    );
    // Anything that isn't a number is no agreement, even where both are NaN: a clip refuses it.
    let finite = |d: f64| if d.is_finite() { d } else { f64::INFINITY };
    (finite(differences.0), finite(differences.1), finite(differences.2))
}

fn difference_mm(node: &Node, a: &ufbx::Transform, b: &ufbx::Transform) -> f64 {
    let (t, r, s) = channel_differences(a, b);
    (t * parent_scale(node)).max(r * REACH_M).max(s * REACH_M) * 1000.0
}

/// The spike (M6 A2's "measure first"): every channel of every fixture with curves, evaluated here
/// against ufbx, tick by tick. Prints a row a channel; `--nocapture` shows them.
#[test]
#[allow(clippy::print_stdout, reason = "the spike's table, read with --nocapture")]
fn every_channel_evaluates_within_the_kill_line_of_ufbx() {
    let mut rows = Vec::new();
    measure("animated_clips.fbx", &fixture("animated_clips.fbx"), &mut rows);
    measure("curves", &curves_scene(), &mut rows);
    measure("transforms", &transforms_scene(), &mut rows);
    measure("layers", &layers_scene(), &mut rows);
    measure("morph", &morph_scene(), &mut rows);
    measure("inherit", &inherit_scene(), &mut rows);

    let width = rows.iter().map(|(name, _)| name.len()).max().unwrap_or(0);
    let mut table = format!("{:width$}  {:>12}  same f32 bits\n", "channel", "worst mm");
    for (name, worst) in &rows {
        let _ = writeln!(table, "{name:width$}  {:>12.3e}  {}/{}", worst.mm, worst.same, worst.ticks);
    }
    println!("{table}");
    let (name, worst) = rows.iter().max_by(|a, b| a.1.mm.total_cmp(&b.1.mm)).unwrap();
    assert!(worst.mm <= KILL_MM, "{name}: {} mm, over the kill line of {KILL_MM} mm\n{table}", worst.mm);
    assert!(rows.len() > 60, "every fixture's channels are measured: {}", rows.len());
}

/// The hand-written files hold what they say: each interpolation, extrapolation, order and blend
/// mode, as ufbx reads them, so the spike measures each.
#[test]
fn the_hand_written_files_hold_every_mode() {
    let scene = load(&curves_scene(), false, true).unwrap();
    let curves: Vec<&AnimCurve> = scene.anim_curves.iter().map(|curve| &**curve).collect();
    let interpolations: Vec<Interpolation> =
        curves.iter().flat_map(|curve| curve.keyframes.iter().map(|key| key.interpolation)).collect();
    for mode in [
        Interpolation::ConstantPrev,
        Interpolation::ConstantNext,
        Interpolation::Linear,
        Interpolation::Cubic,
    ] {
        assert!(interpolations.contains(&mode), "{mode:?}");
    }
    let extrapolations: Vec<(ExtrapolationMode, i32)> = curves
        .iter()
        .flat_map(|curve| [&curve.pre_extrapolation, &curve.post_extrapolation])
        .map(|e| (e.mode, e.repeat_count))
        .collect();
    for mode in [
        ExtrapolationMode::Constant,
        ExtrapolationMode::Slope,
        ExtrapolationMode::Repeat,
        ExtrapolationMode::Mirror,
        ExtrapolationMode::RepeatRelative,
    ] {
        assert!(extrapolations.iter().any(|(m, _)| *m == mode), "{mode:?}: {extrapolations:?}");
    }
    assert!(extrapolations.iter().any(|(_, count)| *count > 0), "a repeat count");

    let scene = load(&transforms_scene(), false, true).unwrap();
    let orders: Vec<ufbx::RotationOrder> = scene.nodes.iter().map(|node| node.rotation_order).collect();
    for order in 0..7 {
        assert!(orders.iter().any(|o| *o as u32 == order), "order {order}: {orders:?}");
    }

    let scene = load(&layers_scene(), false, true).unwrap();
    let layers: Vec<(bool, bool, bool, bool, bool)> = scene
        .anim_layers
        .iter()
        .map(|l| (l.additive, l.blended, l.compose_rotation, l.compose_scale, l.weight_is_animated))
        .collect();
    assert!(layers.contains(&(true, true, true, true, false)), "{layers:?}");
    assert!(layers.contains(&(true, true, false, false, false)), "{layers:?}");
    assert!(layers.contains(&(false, true, true, true, true)), "{layers:?}");
    assert!(layers.contains(&(false, true, false, false, false)), "{layers:?}");
    assert!(layers.iter().any(|l| !l.0 && !l.1), "an override: {layers:?}");
}

/// A property a connection drives is one ufbx would follow, so a clip names it and is refused, and a
/// rest reads the value it holds, as ufbx's own rest does.
#[test]
fn a_connected_property_is_refused_by_name() {
    let mut scene = Scene::default();
    let stack = scene.stack("Driven", Some((0.0, 1.0)));
    let layer = scene.layer(stack, &[]);
    let driver = scene.model("Driver", 0, &["\"Lcl Translation\", \"Lcl Translation\", \"\", \"A\",1,2,3"]);
    let driven = scene.model("Driven", 0, &["\"Lcl Translation\", \"Lcl Translation\", \"\", \"A\",4,5,6"]);
    let turn = curve(&[key(0.0, 0.0, LINEAR), key(1.0, 90.0, LINEAR)]);
    scene.curves(
        layer,
        driven,
        "Lcl Rotation",
        &[("d|X", 0.0, turn), ("d|Y", 0.0, None), ("d|Z", 0.0, None)],
    );
    let _ = writeln!(
        scene.connections,
        "\tC: \"PP\",{driver}, \"Lcl Translation\",{driven}, \"Lcl Translation\""
    );
    let bytes = scene.bytes(Y_UP, 100.0);

    let loaded = load(&bytes, false, true).unwrap();
    let node = loaded.nodes.iter().find(|node| &*node.element.name == "Driven").unwrap();
    let prop = node.element.props.find_prop(LCL_TRANSLATION).unwrap();
    assert!(prop.flags.has_any(ufbx::PropFlags::CONNECTED), "the file connects it");
    let own = rest(node);
    assert_eq!(as_f32_bits(&own), as_f32_bits(&node.local_transform));

    let clips = crate::import::fbx::clips::FbxClips::import(&bytes, false).unwrap();
    let error = clips.sample(0, &crate::clip::ClipSettings::default()).unwrap_err();
    let said = error.to_string();
    assert!(
        said.contains("Driven") && said.contains("Lcl Translation") && said.contains("connection"),
        "{said}"
    );
}

/// The hand-written files' clips, every stack sampled, are one set of bits on every architecture:
/// CI's `Test` job holds this on Linux and Windows as well as macOS. Unlike the Blender fixture, whose
/// keys are all linear, these hold auto and TCB tangents, which ufbx derives in C as it parses, so this
/// is where a fused multiply-add there would show. If only some machines fail, that's it; if all do,
/// a file here or the evaluation changed on purpose: update the constant.
#[test]
fn the_hand_written_clips_sample_to_one_set_of_bits() {
    let mut hasher = blake3::Hasher::new();
    for (bytes, with_mesh) in [
        (curves_scene(), false),
        (transforms_scene(), false),
        (layers_scene(), false),
        (morph_scene(), true),
        (inherit_scene(), false),
    ] {
        let clips = crate::import::fbx::clips::FbxClips::import(&bytes, with_mesh).unwrap();
        let (names, _) = crate::import::fbx::clips::plan(&bytes).unwrap();
        for index in 0..names.len() {
            let clip = clips.sample(index, &crate::clip::ClipSettings::default()).unwrap();
            let mut add = |values: &[f32]| {
                for value in values {
                    hasher.update(&value.to_bits().to_le_bytes());
                }
            };
            for (_, track) in &clip.rotations {
                track.iter().for_each(|q| add(&q.to_array()));
            }
            for (_, track) in clip.translations.iter().chain(&clip.scales) {
                track.iter().for_each(|v| add(&v.to_array()));
            }
            for (_, track) in &clip.weights {
                add(track);
            }
        }
        for joint in clips.joints() {
            let rest = &joint.rest;
            hasher.update(bytemuck::cast_slice(&[rest.translation, rest.scale]));
            hasher.update(bytemuck::cast_slice(&rest.rotation));
        }
    }
    assert_eq!(hasher.finalize().to_hex().as_str(), HAND_WRITTEN_CLIPS);
}

const HAND_WRITTEN_CLIPS: &str = "7495d7e35b0f3469bbf303a9636fc4cdb921526028195fa170b5ecd1183bf4cf";

/// How far apart two of ufbx's affine matrices put what they carry, in millimeters: the translation
/// in meters (the load's unit), and the rest at the end of a bone [`REACH_M`] long.
fn matrix_mm(a: &ufbx::Matrix, b: &ufbx::Matrix) -> f64 {
    let linear = [
        a.m00 - b.m00,
        a.m10 - b.m10,
        a.m20 - b.m20,
        a.m01 - b.m01,
        a.m11 - b.m11,
        a.m21 - b.m21,
        a.m02 - b.m02,
        a.m12 - b.m12,
        a.m22 - b.m22,
    ];
    let translation = DVec3::new(a.m03 - b.m03, a.m13 - b.m13, a.m23 - b.m23).length();
    let linear = linear.iter().fold(0.0f64, |worst, d| worst.max(d.abs()));
    let mm = translation.max(linear * REACH_M) * 1000.0;
    if mm.is_finite() { mm } else { f64::INFINITY }
}

fn matrix_bits(m: &ufbx::Matrix) -> [u64; 12] {
    [m.m00, m.m10, m.m20, m.m01, m.m11, m.m21, m.m02, m.m12, m.m22, m.m03, m.m13, m.m23].map(f64::to_bits)
}

/// The importer's matrices against ufbx's: each node's world rest ([`rest_world`] against
/// `node_to_world`), its geometry's (against `geometry_to_world`) and each skin cluster's
/// mesh-to-bone matrix (against `geometry_to_bone`), on every FBX fixture, the pavilion and the
/// hand-written files (the transforms' carries a geometric transform, the inheritance's scale
/// helpers). Prints a row a file; `--nocapture` shows them.
#[test]
#[allow(clippy::print_stdout, reason = "the measurement's table, read with --nocapture")]
fn every_rest_and_geometry_matrix_is_within_the_kill_line_of_ufbx() {
    let pavilion = std::fs::read(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../content/third_party/vvaytoyek_chinese_pavilion/chinese_four_corner_pavilion.fbx"),
    )
    .unwrap();
    let files = [
        ("animated_clips.fbx", fixture("animated_clips.fbx")),
        ("skinned_morph.fbx", fixture("skinned_morph.fbx")),
        ("static_multi.fbx", fixture("static_multi.fbx")),
        ("pavilion", pavilion),
        ("transforms", transforms_scene()),
        ("inherit", inherit_scene()),
    ];
    let mut table = format!(
        "{:20}  {:>12}  {:>12}  {:>12}  same f64 bits\n",
        "file", "rest mm", "geometry mm", "bind mm"
    );
    let mut worst_of_all = 0.0f64;
    let mut geometric = 0;
    for (name, bytes) in &files {
        let scene = load(bytes, false, false).unwrap();
        let (mut rest_mm, mut geometry_mm, mut bind_mm) = (0.0f64, 0.0f64, 0.0f64);
        let (mut same, mut count) = (0, 0);
        let mut add = |worst: &mut f64, own: &ufbx::Matrix, theirs: &ufbx::Matrix| {
            *worst = worst.max(matrix_mm(own, theirs));
            same += usize::from(matrix_bits(own) == matrix_bits(theirs));
            count += 1;
        };
        for node in scene.nodes.iter().filter(|node| !node.is_root) {
            add(&mut rest_mm, &rest_world(node), &node.node_to_world);
            add(&mut geometry_mm, &crate::import::fbx::geometry_to_world(node), &node.geometry_to_world);
            geometric += usize::from(geometry_to_node(node).is_some());
            assert_eq!(
                geometry_to_node(node).is_some(),
                node.has_geometry_transform,
                "{}",
                &*node.element.name
            );
        }
        for mesh in scene.meshes.iter() {
            for cluster in mesh.skin_deformers.iter().flat_map(|skin| skin.clusters.iter()) {
                let own = crate::import::fbx::geometry_to_bone(mesh, &mesh.element.instances[0], cluster);
                add(&mut bind_mm, &own, &cluster.geometry_to_bone);
            }
        }
        let _ = writeln!(
            table,
            "{name:20}  {rest_mm:>12.3e}  {geometry_mm:>12.3e}  {bind_mm:>12.3e}  {same}/{count}"
        );
        worst_of_all = worst_of_all.max(rest_mm).max(geometry_mm).max(bind_mm);
    }
    println!("{table}");
    assert!(worst_of_all <= KILL_MM, "{worst_of_all} mm, over the kill line of {KILL_MM} mm\n{table}");
    assert!(geometric > 0, "a geometric transform is measured");
}

/// An FBX mesh imports to one set of bits on every architecture: the skinned fixture with blend
/// shapes, whose vertices, palette, rests and deltas all come through the Rust rests above. CI's
/// `Test` job holds this on Linux and Windows as well as macOS. If only some machines fail, a
/// matrix here still comes from C; if all do, the importer changed on purpose: update the constant.
#[test]
fn an_fbx_mesh_imports_to_one_set_of_bits() {
    let mut hasher = blake3::Hasher::new();
    for name in ["skinned_morph.fbx", "animated_clips.fbx"] {
        let mesh = crate::import::fbx::import_slice(&fixture(name)).unwrap();
        assert!(mesh.skin.is_some(), "{name} is skinned");
        // `Debug` writes every float in the fewest digits that read back to its bits.
        hasher.update(format!("{mesh:?}").as_bytes());
    }
    assert_eq!(hasher.finalize().to_hex().as_str(), FBX_MESHES);
}

const FBX_MESHES: &str = "f56ebe94c4ab2d3d5aa967de74fa981f151d6cffa3df74835342bc95d4711654";
