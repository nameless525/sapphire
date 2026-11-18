//! FBX animation evaluated in Rust from ufbx's raw keys (M6 A2), so a clip from an FBX is the same
//! bytes on every architecture and a move may bake its tables from one ([`crate::clip`]'s "Byte
//! identity").
//!
//! ufbx's own evaluation (`ufbx::evaluate_transform`) is C: it turns Euler angles into rotations
//! through its platform's libm, and a C compiler may fuse a multiply and an add on one architecture
//! and not another. This is the same evaluation, step for step, on IEEE arithmetic in `f64` (which
//! Rust never fuses) and the `libm` crate's `sin`, `cos`, `asin`, `acos`, `atan2` and `pow` (musl's,
//! in pure Rust, as [`sapphire_math::det`] wraps in `f32`), so it computes the same bits everywhere.
//! Only values cross from C: the keys (time, value, interpolation, tangents), each curve's pre- and
//! post-extrapolation, the properties' defaults and the conversion ufbx folds into a node on load.
//!
//! - **Curves:** a key's interpolation (constant, constant from the next key, linear, cubic Bézier
//!   by its tangents, solved for time by Newton's method as ufbx does), and each end's
//!   extrapolation (constant, slope, repeat, mirror, relative repeat, with a repeat count),
//!   counted in FBX's own time units so a repeat lands on its frames.
//! - **Layers:** the first layer sets a property; each later one adds to it, blends it by its
//!   weight (animated or not) or replaces it, rotations as rotations and scales by powers when the
//!   layer composes them, as ufbx does.
//! - **Transforms:** a node's scale, rotation and scale pivots and offsets, its pre- and
//!   post-rotations under its rotation order, the axis and unit conversion ufbx folded into it
//!   (`adjust_*`), and scale helpers, in ufbx's order.
//! - **Not here:** a property a connection drives from another element's (constraints, expressions),
//!   which ufbx follows: such a clip is refused, naming the property
//!   (`ClipImportError::FbxConnected`).
//!
//! **What the keys carry.** ufbx derives a key's tangents as it parses: in `f64`, rounded to `f32`.
//! Linear, constant and user (Maya's "fixed" or "broken") tangents are one product each, the same
//! bits everywhere. Auto and TCB tangents take a few multiply-adds first, which a C compiler may fuse
//! on aarch64; that moves a tangent only where the `f64` result lands within an `f64` rounding of
//! an `f32` boundary, which `moves`' pinned tables on CI's three operating systems watch for.

use std::collections::HashMap;

use ufbx::{Anim, AnimCurve, AnimLayer, AnimStack, AnimValue, ExtrapolationMode, Interpolation, Node};

/// `node`'s rest: its local transform from the properties the file holds, as ufbx's
/// `local_transform`, in Rust.
pub(crate) fn rest(node: &Node) -> ufbx::Transform {
    Evaluation::rest()
        .transform(node, 0.0)
        .expect("the rest reads a connected property as the value it holds")
}

/// A property a connection drives, which this doesn't evaluate: the clip is refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Connected {
    /// The property's name.
    pub property: &'static str,
}

/// The properties a node's transform reads.
const LCL_TRANSLATION: &str = "Lcl Translation";
const LCL_ROTATION: &str = "Lcl Rotation";
const LCL_SCALING: &str = "Lcl Scaling";
const PRE_ROTATION: &str = "PreRotation";
const POST_ROTATION: &str = "PostRotation";
const ROTATION_ORDER: &str = "RotationOrder";
const SCALING_PIVOT: &str = "ScalingPivot";
const ROTATION_PIVOT: &str = "RotationPivot";
const SCALING_OFFSET: &str = "ScalingOffset";
const ROTATION_OFFSET: &str = "RotationOffset";
const DEFORM_PERCENT: &str = "DeformPercent";

type V3 = [f64; 3];

/// A rotation as ufbx keeps one: x, y, z, w.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Q {
    x: f64,
    y: f64,
    z: f64,
    w: f64,
}

const IDENTITY: Q = Q { x: 0.0, y: 0.0, z: 0.0, w: 1.0 };

/// Translation, rotation and scale, composed as ufbx's `ufbx_transform` is.
#[derive(Clone, Copy, Debug)]
struct Composed {
    translation: V3,
    rotation: Q,
    scale: V3,
}

const IDENTITY_TRANSFORM: Composed = Composed { translation: [0.0; 3], rotation: IDENTITY, scale: [1.0; 3] };

/// One layer of a stack, with its animated properties by element and name.
struct Layer<'s> {
    layer: &'s AnimLayer,
    values: HashMap<(u32, &'s str), &'s AnimValue>,
    /// The curve of its own weight, where that is animated.
    weight: Option<&'s AnimValue>,
}

/// An animation's evaluation: one stack's layers, or none, for the rest pose.
pub(crate) struct Evaluation<'s> {
    layers: Vec<Layer<'s>>,
    /// FBX's time units a second, which repeats count in.
    ktime_second: f64,
    /// Whether a property a connection drives is refused (a stack's), or read as the value it holds
    /// (the rest, as ufbx reads it on load).
    refuse_connections: bool,
}

impl<'s> Evaluation<'s> {
    /// The rest pose: every property as the file holds it, as ufbx's `local_transform` is made. It
    /// evaluates no curve, so it needs no time units.
    fn rest() -> Self {
        Self { layers: Vec::new(), ktime_second: 1.0, refuse_connections: false }
    }

    /// `stack`'s animation.
    pub fn stack(scene: &ufbx::Scene, stack: &'s AnimStack) -> Self {
        Self::anim(scene, &stack.anim)
    }

    fn anim(scene: &ufbx::Scene, anim: &'s Anim) -> Self {
        let layers = anim
            .layers
            .iter()
            .map(|layer| {
                let layer: &'s AnimLayer = layer;
                let own = layer.element.element_id;
                let mut values = HashMap::new();
                let mut weight = None;
                for prop in layer.anim_props.iter() {
                    let id = prop.element.element_id;
                    // ufbx takes a layer's first property of its own as its weight's curve.
                    if id == own && weight.is_none() {
                        weight = Some(&*prop.anim_value);
                    }
                    values.entry((id, &*prop.prop_name)).or_insert(&*prop.anim_value);
                }
                Layer { layer, values, weight }
            })
            .collect();
        Self {
            layers,
            ktime_second: scene.metadata.ktime_second as f64,
            refuse_connections: !anim.ignore_connections,
        }
    }

    /// `node`'s local transform at `time`, as `ufbx::evaluate_transform` gives it.
    pub fn transform(&self, node: &Node, time: f64) -> Result<ufbx::Transform, Connected> {
        let mut translation_scale = None;
        let mut scale_factor = [1.0; 3];
        let mut use_scale_factor = false;
        if let Some(parent) = node.parent.as_deref() {
            if let Some(mut inherit) = parent.inherit_scale_node.as_deref() {
                use_scale_factor = node.is_scale_helper;
                while let Some(helper) = inherit.scale_helper.as_deref() {
                    // ufbx reads a missing property as zeros here.
                    let scale = self.prop(&helper.element, LCL_SCALING, time)?.unwrap_or([0.0; 3]);
                    scale_factor = mul3(scale_factor, scale);
                    match inherit.inherit_scale_node.as_deref() {
                        Some(next) => inherit = next,
                        None => break,
                    }
                }
            }
            if let Some(helper) = parent.scale_helper.as_deref() {
                let scale = self.prop(&helper.element, LCL_SCALING, time)?.unwrap_or([1.0; 3]);
                translation_scale = Some(mul3(scale, scale_factor));
            }
        }
        let order = self.rotation_order(&node.element, time)?;
        let mut transform = self.compose(node, order, translation_scale, time)?;
        if use_scale_factor {
            transform.scale = mul3(transform.scale, scale_factor);
        }
        let Composed { translation: t, rotation: r, scale: s } = transform;
        Ok(ufbx::Transform {
            translation: ufbx::Vec3 { x: t[0], y: t[1], z: t[2] },
            rotation: ufbx::Quat { x: r.x, y: r.y, z: r.z, w: r.w },
            scale: ufbx::Vec3 { x: s[0], y: s[1], z: s[2] },
        })
    }

    /// A blend channel's weight at `time`, as `ufbx::evaluate_blend_weight` gives it.
    pub fn blend_weight(&self, channel: &ufbx::BlendChannel, time: f64) -> Result<f64, Connected> {
        let percent =
            self.prop(&channel.element, DEFORM_PERCENT, time)?.map_or(channel.weight * 100.0, |v| v[0]);
        Ok(percent * 0.01)
    }

    /// ufbx's `ufbxi_get_transform`: `T · Roff · Rp · Rpre · R · Rpost⁻¹ · Rp⁻¹ · Soff · Sp · S · Sp⁻¹`,
    /// with the conversion ufbx folded into the node around it.
    fn compose(
        &self,
        node: &Node,
        order: ufbx::RotationOrder,
        translation_scale: Option<V3>,
        time: f64,
    ) -> Result<Composed, Connected> {
        let element = &node.element;
        let vec = |name: &'static str, default: V3| -> Result<V3, Connected> {
            Ok(self.prop(element, name, time)?.unwrap_or(default))
        };
        let scale_pivot = vec(SCALING_PIVOT, [0.0; 3])?;
        let rotation_pivot = vec(ROTATION_PIVOT, [0.0; 3])?;
        let scale_offset = vec(SCALING_OFFSET, [0.0; 3])?;
        let rotation_offset = vec(ROTATION_OFFSET, [0.0; 3])?;
        let mut translation = vec(LCL_TRANSLATION, [0.0; 3])?;
        let rotation = vec(LCL_ROTATION, [0.0; 3])?;
        let scaling = vec(LCL_SCALING, [1.0; 3])?;
        let pre_rotation = vec(PRE_ROTATION, [0.0; 3])?;
        let post_rotation = vec(POST_ROTATION, [0.0; 3])?;

        if let Some(scale) = translation_scale {
            translation = mul3(translation, scale);
        }
        let mut t = IDENTITY_TRANSFORM;
        if node.has_adjust_transform {
            t.rotate_by(quat(node.adjust_post_rotation));
            t.scale_by_real(node.adjust_post_scale);
        }
        t.translate(neg3(scale_pivot));
        t.scale_by(scaling);
        t.translate(scale_pivot);
        t.translate(scale_offset);
        t.translate(neg3(rotation_pivot));
        if node.use_rotation_space {
            t.rotate_euler(post_rotation, ufbx::RotationOrder::Xyz, true);
            t.rotate_euler(rotation, order, false);
            t.rotate_euler(pre_rotation, ufbx::RotationOrder::Xyz, false);
        } else {
            t.rotate_euler(rotation, ufbx::RotationOrder::Xyz, false);
        }
        t.translate(rotation_pivot);
        t.translate(rotation_offset);
        t.translate(translation);
        if node.has_adjust_transform {
            t.translate(vec3(node.adjust_pre_translation));
            t.rotate_by(quat(node.adjust_pre_rotation));
            t.scale_by_real(node.adjust_pre_scale);
            let scale = node.adjust_translation_scale;
            t.translation = mul3(t.translation, [scale; 3]);
        }
        let axis = node.adjust_mirror_axis as usize;
        if axis != 0 {
            t.translation[axis - 1] = -t.translation[axis - 1];
            let mut v = [t.rotation.x, t.rotation.y, t.rotation.z];
            v[axis % 3] = -v[axis % 3];
            v[(axis + 1) % 3] = -v[(axis + 1) % 3];
            t.rotation = Q { x: v[0], y: v[1], z: v[2], w: t.rotation.w };
        }
        Ok(t)
    }

    /// ufbx's `ufbxi_find_enum` on the rotation order: Euler XYZ unless the node says otherwise.
    fn rotation_order(&self, element: &ufbx::Element, time: f64) -> Result<ufbx::RotationOrder, Connected> {
        let Some(prop) = element.props.find_prop(ROTATION_ORDER) else { return Ok(ufbx::RotationOrder::Xyz) };
        let value = if self.animated(prop)? {
            f64_to_i64(self.prop(element, ROTATION_ORDER, time)?.map_or(0.0, |v| v[0]))
        } else {
            prop.value_int
        };
        Ok(match value {
            0 => ufbx::RotationOrder::Xyz,
            1 => ufbx::RotationOrder::Xzy,
            2 => ufbx::RotationOrder::Yzx,
            3 => ufbx::RotationOrder::Yxz,
            4 => ufbx::RotationOrder::Zxy,
            5 => ufbx::RotationOrder::Zyx,
            6 => ufbx::RotationOrder::Spheric,
            _ => ufbx::RotationOrder::Xyz,
        })
    }

    /// Whether a property's value comes from the layers: animated, or refused as connected.
    fn animated(&self, prop: &ufbx::Prop) -> Result<bool, Connected> {
        if prop.flags.has_any(ufbx::PropFlags::CONNECTED) && self.refuse_connections {
            let property = [
                LCL_TRANSLATION,
                LCL_ROTATION,
                LCL_SCALING,
                PRE_ROTATION,
                POST_ROTATION,
                ROTATION_ORDER,
                SCALING_PIVOT,
                ROTATION_PIVOT,
                SCALING_OFFSET,
                ROTATION_OFFSET,
                DEFORM_PERCENT,
            ]
            .into_iter()
            .find(|name| *name == &*prop.name)
            .unwrap_or("a property");
            return Err(Connected { property });
        }
        Ok(prop.flags.has_any(ufbx::PropFlags::ANIMATED))
    }

    /// Property `name` of `element` at `time`, or `None` where the element has no such property
    /// (ufbx's `ufbx_evaluate_prop` and `ufbxi_evaluate_props`).
    fn prop(&self, element: &ufbx::Element, name: &'static str, time: f64) -> Result<Option<V3>, Connected> {
        let Some(prop) = element.props.find_prop(name) else { return Ok(None) };
        let v = prop.value_vec4;
        let mut value = [v.x, v.y, v.z];
        if !self.animated(prop)? {
            return Ok(Some(value));
        }
        // Blending a rotation needs the element's order, read once, as ufbx does.
        let mut order = None;
        for (index, layer) in self.layers.iter().enumerate() {
            let mut weight = layer.layer.weight;
            if layer.layer.weight_is_animated
                && layer.layer.blended
                && let Some(curve) = layer.weight
            {
                weight = self.value(curve, time)[0] / 100.0;
                if weight < 0.0 {
                    weight = 0.0;
                }
                if weight > f64::from(0.99999f32) {
                    weight = 1.0;
                }
            }
            let Some(&curve) = layer.values.get(&(element.element_id, name)) else { continue };
            let v = self.value(curve, time);
            if index == 0 {
                value = v;
                continue;
            }
            let layer = layer.layer;
            if layer.compose_rotation && layer.blended && name == LCL_ROTATION && order.is_none() {
                order = Some(self.rotation_order(element, time)?);
            }
            let order = order.unwrap_or(ufbx::RotationOrder::Xyz);
            value = combine(layer, weight, name, order, value, v);
        }
        Ok(Some(value))
    }

    /// A curve node's value at `time`: its default where a component has no curve.
    fn value(&self, value: &AnimValue, time: f64) -> V3 {
        let d = value.default_value;
        let mut out = [d.x, d.y, d.z];
        for (axis, curve) in value.curves.iter().enumerate() {
            if let Some(curve) = curve {
                out[axis] = evaluate_curve(curve, time, out[axis], self.ktime_second, true);
            }
        }
        out
    }
}

/// One later layer's value `v` on top of `result` (ufbx's `ufbxi_combine_anim_layer`).
fn combine(layer: &AnimLayer, weight: f64, name: &str, order: ufbx::RotationOrder, result: V3, v: V3) -> V3 {
    let scaling = layer.compose_scale && name == LCL_SCALING;
    let rotating = layer.compose_rotation && name == LCL_ROTATION;
    if layer.additive {
        if scaling {
            std::array::from_fn(|i| result[i] * pow_abs(v[i], weight))
        } else if rotating {
            let a = euler_to_quat(result, order);
            let b = slerp(IDENTITY, euler_to_quat(v, order), weight);
            quat_to_euler(mul_quat(a, b), order)
        } else {
            std::array::from_fn(|i| result[i] + v[i] * weight)
        }
    } else if layer.blended {
        let rest = 1.0 - weight;
        if scaling {
            std::array::from_fn(|i| pow_abs(result[i], rest) * pow_abs(v[i], weight))
        } else if rotating {
            quat_to_euler(slerp(euler_to_quat(result, order), euler_to_quat(v, order), weight), order)
        } else {
            std::array::from_fn(|i| result[i] * rest + v[i] * weight)
        }
    } else {
        v
    }
}

/// `curve` at `time`, `default` where it has no keys (ufbx's `ufbx_evaluate_curve`).
pub(crate) fn evaluate_curve(
    curve: &AnimCurve,
    time: f64,
    default: f64,
    ktime_second: f64,
    extrapolate: bool,
) -> f64 {
    let keys: &[ufbx::Keyframe] = &curve.keyframes;
    match keys {
        [] => return default,
        [only] => return only.value,
        _ => {}
    }
    if extrapolate && (time < curve.min_time || time > curve.max_time) {
        return extrapolate_curve(curve, time, ktime_second);
    }
    // ufbx's search, kept as it is so that keys out of order find what they find there.
    let mut begin = 0;
    let mut end = keys.len();
    while end - begin >= 8 {
        let mid = (begin + end) >> 1;
        if keys[mid].time <= time {
            begin = mid + 1;
        } else {
            end = mid;
        }
    }
    for next_index in begin..keys.len() {
        let next = &keys[next_index];
        if next.time <= time {
            continue;
        }
        if next_index == 0 {
            return next.value;
        }
        let prev = &keys[next_index - 1];
        if prev.time == time {
            return prev.value;
        }
        let rcp_delta = 1.0 / (next.time - prev.time);
        let t = (time - prev.time) * rcp_delta;
        return match prev.interpolation {
            Interpolation::ConstantPrev => prev.value,
            Interpolation::ConstantNext => next.value,
            Interpolation::Linear => prev.value * (1.0 - t) + next.value * t,
            Interpolation::Cubic => {
                let x1 = f64::from(prev.right.dx) * rcp_delta;
                let x2 = 1.0 - f64::from(next.left.dx) * rcp_delta;
                let t = cubic_bezier_t(x1, x2, t);
                let (t2, u) = (t * t, 1.0 - t);
                let (t3, u2) = (t2 * t, u * u);
                let u3 = u2 * u;
                let y0 = prev.value;
                let y3 = next.value;
                let y1 = y0 + f64::from(prev.right.dy);
                let y2 = y3 - f64::from(next.left.dy);
                u3 * y0 + 3.0 * (u2 * t * y1 + u * t2 * y2) + t3 * y3
            }
        };
    }
    keys[keys.len() - 1].value
}

/// The `t` of a cubic Bézier from (0, 0) through x `p1` and `p2` to (1, 1) whose x is `x0`.
fn cubic_bezier_t(p1: f64, p2: f64, x0: f64) -> f64 {
    let (p1_3, p2_3) = (p1 * 3.0, p2 * 3.0);
    let a = p1_3 - p2_3 + 1.0;
    let b = p2_3 - p1_3 - p1_3;
    let c = p1_3;
    let (a_3, b_2) = (3.0 * a, 2.0 * b);
    let mut t = x0;
    let mut x1 = 0.0;
    let mut step = |t: &mut f64| {
        let t2 = *t * *t;
        let t3 = t2 * *t;
        x1 = a * t3 + b * t2 + c * *t - x0;
        *t -= x1 / (a_3 * t2 + b_2 * *t + c);
        x1
    };
    step(&mut t);
    step(&mut t);
    let mut residual = step(&mut t);
    // Four units in the last place from 1.0.
    const EPSILON: f64 = 8.881_784_197_001_252e-16;
    if residual.abs() <= EPSILON {
        return t;
    }
    for _ in 0..4 {
        step(&mut t);
        residual = step(&mut t);
        if residual.abs() <= EPSILON {
            return t;
        }
    }
    t
}

/// `curve` past its keys, by the end's extrapolation (ufbx's `ufbxi_extrapolate_curve`).
fn extrapolate_curve(curve: &AnimCurve, real_time: f64, ktime_second: f64) -> f64 {
    let pre = real_time < curve.min_time;
    let keys: &[ufbx::Keyframe] = &curve.keyframes;
    let (key, extrapolation) = if pre {
        (&keys[0], &curve.pre_extrapolation)
    } else {
        (&keys[keys.len() - 1], &curve.post_extrapolation)
    };
    match extrapolation.mode {
        ExtrapolationMode::Constant => return key.value,
        ExtrapolationMode::Slope => {
            let tangent = if pre { key.right } else { key.left };
            return key.value + f64::from(tangent.dy) * ((real_time - key.time) / f64::from(tangent.dx));
        }
        _ if extrapolation.repeat_count == 0 => return key.value,
        _ => {}
    }
    // In FBX's time units, so a repeat lands on its frames.
    let min_time = (curve.min_time * ktime_second).round_ties_even();
    let max_time = (curve.max_time * ktime_second).round_ties_even();
    let time = real_time * ktime_second;
    let delta = if pre { min_time - time } else { time - max_time };
    let duration = max_time - min_time;
    // At least one unit, and never a NaN.
    if duration < 1.0 || duration.is_nan() {
        return key.value;
    }
    let rep = delta / duration;
    let mut rep_n = rep.floor();
    let mut rep_d = delta - rep_n * duration;
    if extrapolation.repeat_count > 0 && rep_n >= f64::from(extrapolation.repeat_count) {
        rep_n = f64::from(extrapolation.repeat_count - 1);
        rep_d = duration;
    }
    if extrapolation.mode == ExtrapolationMode::Mirror {
        let parity = rep_n * 0.5 - (rep_n * 0.5).floor();
        if parity <= 0.25 {
            rep_d = duration - rep_d;
        }
    }
    if pre {
        rep_d = duration - rep_d;
    }
    let new_time = (min_time + rep_d) / ktime_second;
    let mut value = evaluate_curve(curve, new_time, key.value, ktime_second, false);
    if extrapolation.mode == ExtrapolationMode::RepeatRelative {
        let mut span = keys[keys.len() - 1].value - keys[0].value;
        if pre {
            span = -span;
        }
        value += span * (rep_n + 1.0);
    }
    value
}

impl Composed {
    fn translate(&mut self, v: V3) {
        self.translation = std::array::from_fn(|i| self.translation[i] + v[i]);
    }

    fn scale_by(&mut self, v: V3) {
        self.translation = mul3(self.translation, v);
        self.scale = mul3(self.scale, v);
    }

    fn scale_by_real(&mut self, v: f64) {
        self.scale_by([v; 3]);
    }

    /// ufbx's `ufbxi_mul_rotate_quat`: `q` after what's there.
    fn rotate_by(&mut self, q: Q) {
        if q == IDENTITY {
            return;
        }
        self.apply(q);
    }

    /// ufbx's `ufbxi_mul_rotate` and, `inverse`, `ufbxi_mul_inv_rotate`.
    fn rotate_euler(&mut self, euler: V3, order: ufbx::RotationOrder, inverse: bool) {
        if euler == [0.0; 3] {
            return;
        }
        let mut q = euler_to_quat(euler, order);
        if inverse {
            q = Q { x: -q.x, y: -q.y, z: -q.z, w: q.w };
        }
        self.apply(q);
    }

    fn apply(&mut self, q: Q) {
        self.rotation = if self.rotation.w != 1.0 { mul_quat(q, self.rotation) } else { q };
        if self.translation != [0.0; 3] {
            self.translation = rotate_vec3(q, self.translation);
        }
    }
}

fn mul3(a: V3, b: V3) -> V3 {
    [a[0] * b[0], a[1] * b[1], a[2] * b[2]]
}

fn neg3(a: V3) -> V3 {
    [-a[0], -a[1], -a[2]]
}

fn vec3(v: ufbx::Vec3) -> V3 {
    [v.x, v.y, v.z]
}

fn quat(q: ufbx::Quat) -> Q {
    Q { x: q.x, y: q.y, z: q.z, w: q.w }
}

fn mul_quat(a: Q, b: Q) -> Q {
    Q {
        x: a.w * b.x + a.x * b.w + a.y * b.z - a.z * b.y,
        y: a.w * b.y - a.x * b.z + a.y * b.w + a.z * b.x,
        z: a.w * b.z + a.x * b.y - a.y * b.x + a.z * b.w,
        w: a.w * b.w - a.x * b.x - a.y * b.y - a.z * b.z,
    }
}

/// ufbx's `ufbx_quat_rotate_vec3`.
fn rotate_vec3(q: Q, v: V3) -> V3 {
    let xy = q.x * v[1] - q.y * v[0];
    let xz = q.x * v[2] - q.z * v[0];
    let yz = q.y * v[2] - q.z * v[1];
    [
        2.0 * (q.w * yz + q.y * xy + q.z * xz) + v[0],
        2.0 * (-q.x * xy - q.w * xz + q.z * yz) + v[1],
        2.0 * (-q.x * xz - q.y * yz + q.w * xy) + v[2],
    ]
}

/// Degrees to radians, as ufbx's `UFBXI_DEG_TO_RAD_DOUBLE` rounds it.
const DEG_TO_RAD: f64 = std::f64::consts::PI / 180.0;
const RAD_TO_DEG: f64 = 180.0 / std::f64::consts::PI;

/// ufbx's `ufbx_euler_to_quat`: Euler angles in degrees, applied in `order`.
fn euler_to_quat(v: V3, order: ufbx::RotationOrder) -> Q {
    let half = DEG_TO_RAD * 0.5;
    let (vx, vy, vz) = (v[0] * half, v[1] * half, v[2] * half);
    let (cx, sx) = (libm::cos(vx), libm::sin(vx));
    let (cy, sy) = (libm::cos(vy), libm::sin(vy));
    let (cz, sz) = (libm::cos(vz), libm::sin(vz));
    use ufbx::RotationOrder as O;
    match order {
        O::Xyz => Q {
            x: -cx * sy * sz + cy * cz * sx,
            y: cx * cz * sy + cy * sx * sz,
            z: cx * cy * sz - cz * sx * sy,
            w: cx * cy * cz + sx * sy * sz,
        },
        O::Xzy => Q {
            x: cx * sy * sz + cy * cz * sx,
            y: cx * cz * sy + cy * sx * sz,
            z: cx * cy * sz - cz * sx * sy,
            w: cx * cy * cz - sx * sy * sz,
        },
        O::Yzx => Q {
            x: -cx * sy * sz + cy * cz * sx,
            y: cx * cz * sy - cy * sx * sz,
            z: cx * cy * sz + cz * sx * sy,
            w: cx * cy * cz + sx * sy * sz,
        },
        O::Yxz => Q {
            x: -cx * sy * sz + cy * cz * sx,
            y: cx * cz * sy + cy * sx * sz,
            z: cx * cy * sz + cz * sx * sy,
            w: cx * cy * cz - sx * sy * sz,
        },
        O::Zxy => Q {
            x: cx * sy * sz + cy * cz * sx,
            y: cx * cz * sy - cy * sx * sz,
            z: cx * cy * sz - cz * sx * sy,
            w: cx * cy * cz + sx * sy * sz,
        },
        O::Zyx => Q {
            x: cx * sy * sz + cy * cz * sx,
            y: cx * cz * sy - cy * sx * sz,
            z: cx * cy * sz + cz * sx * sy,
            w: cx * cy * cz - sx * sy * sz,
        },
        O::Spheric => IDENTITY,
    }
}

/// ufbx's `ufbx_quat_to_euler`, in degrees.
fn quat_to_euler(q: Q, order: ufbx::RotationOrder) -> V3 {
    const EPS: f64 = 0.999_999_999;
    let half_pi = std::f64::consts::PI * 0.5;
    let Q { x: qx, y: qy, z: qz, w: qw } = q;
    let (asin, atan2) = (libm::asin, libm::atan2);
    use ufbx::RotationOrder as O;
    let (vx, vy, vz);
    match order {
        O::Xyz => {
            let t = 2.0 * (qw * qy - qx * qz);
            if t.abs() < EPS {
                vy = asin(t);
                vz = atan2(2.0 * (qw * qz + qx * qy), 2.0 * (qw * qw + qx * qx) - 1.0);
                vx = -atan2(-2.0 * (qw * qx + qy * qz), 2.0 * (qw * qw + qz * qz) - 1.0);
            } else {
                vy = half_pi.copysign(t);
                vz = atan2(-2.0 * t * (qw * qx - qy * qz), t * (2.0 * qw * qy + 2.0 * qx * qz));
                vx = 0.0;
            }
        }
        O::Xzy => {
            let t = 2.0 * (qw * qz + qx * qy);
            if t.abs() < EPS {
                vz = asin(t);
                vy = atan2(2.0 * (qw * qy - qx * qz), 2.0 * (qw * qw + qx * qx) - 1.0);
                vx = -atan2(-2.0 * (qw * qx - qy * qz), 2.0 * (qw * qw + qy * qy) - 1.0);
            } else {
                vz = half_pi.copysign(t);
                vy = atan2(2.0 * t * (qw * qx + qy * qz), -t * (2.0 * qx * qy - 2.0 * qw * qz));
                vx = 0.0;
            }
        }
        O::Yzx => {
            let t = 2.0 * (qw * qz - qx * qy);
            if t.abs() < EPS {
                vz = asin(t);
                vx = atan2(2.0 * (qw * qx + qy * qz), 2.0 * (qw * qw + qy * qy) - 1.0);
                vy = -atan2(-2.0 * (qw * qy + qx * qz), 2.0 * (qw * qw + qx * qx) - 1.0);
            } else {
                vz = half_pi.copysign(t);
                vx = atan2(-2.0 * t * (qw * qy - qx * qz), t * (2.0 * qw * qz + 2.0 * qx * qy));
                vy = 0.0;
            }
        }
        O::Yxz => {
            let t = 2.0 * (qw * qx + qy * qz);
            if t.abs() < EPS {
                vx = asin(t);
                vz = atan2(2.0 * (qw * qz - qx * qy), 2.0 * (qw * qw + qy * qy) - 1.0);
                vy = -atan2(-2.0 * (qw * qy - qx * qz), 2.0 * (qw * qw + qz * qz) - 1.0);
            } else {
                vx = half_pi.copysign(t);
                vz = atan2(2.0 * t * (qw * qy + qx * qz), -t * (2.0 * qy * qz - 2.0 * qw * qx));
                vy = 0.0;
            }
        }
        O::Zxy => {
            let t = 2.0 * (qw * qx - qy * qz);
            if t.abs() < EPS {
                vx = asin(t);
                vy = atan2(2.0 * (qw * qy + qx * qz), 2.0 * (qw * qw + qz * qz) - 1.0);
                vz = -atan2(-2.0 * (qw * qz + qx * qy), 2.0 * (qw * qw + qy * qy) - 1.0);
            } else {
                vx = half_pi.copysign(t);
                vy = atan2(-2.0 * t * (qw * qz - qx * qy), t * (2.0 * qw * qx + 2.0 * qy * qz));
                vz = 0.0;
            }
        }
        O::Zyx => {
            let t = 2.0 * (qw * qy + qx * qz);
            if t.abs() < EPS {
                vy = asin(t);
                vx = atan2(2.0 * (qw * qx - qy * qz), 2.0 * (qw * qw + qz * qz) - 1.0);
                vz = -atan2(-2.0 * (qw * qz - qx * qy), 2.0 * (qw * qw + qx * qx) - 1.0);
            } else {
                vy = half_pi.copysign(t);
                vx = atan2(2.0 * t * (qw * qz + qx * qy), -t * (2.0 * qx * qz - 2.0 * qw * qy));
                vz = 0.0;
            }
        }
        O::Spheric => {
            (vx, vy, vz) = (0.0, 0.0, 0.0);
        }
    }
    [vx * RAD_TO_DEG, vy * RAD_TO_DEG, vz * RAD_TO_DEG]
}

/// ufbx's `ufbx_quat_slerp`.
fn slerp(a: Q, mut b: Q, t: f64) -> Q {
    let mut dot = a.x * b.x + a.y * b.y + a.z * b.z + a.w * b.w;
    if dot < 0.0 {
        dot = -dot;
        b = Q { x: -b.x, y: -b.y, z: -b.z, w: -b.w };
    }
    // `fmin(fmax(dot, 0), 1)`, which a NaN leaves at 0 as C's does.
    let clamped = if dot > 0.0 { if dot < 1.0 { dot } else { 1.0 } } else { 0.0 };
    let omega = libm::acos(clamped);
    if omega <= f64::from(f32::MIN_POSITIVE) {
        return a;
    }
    let rcp_so = 1.0 / libm::sin(omega);
    let af = libm::sin((1.0 - t) * omega) * rcp_so;
    let bf = libm::sin(t * omega) * rcp_so;
    let x = af * a.x + bf * b.x;
    let y = af * a.y + bf * b.y;
    let z = af * a.z + bf * b.z;
    let w = af * a.w + bf * b.w;
    let rcp_len = 1.0 / (x * x + y * y + z * z + w * w).sqrt();
    Q { x: x * rcp_len, y: y * rcp_len, z: z * rcp_len, w: w * rcp_len }
}

/// ufbx's `ufbxi_pow_abs`: a power that keeps the sign, at the ends of `e` exact.
fn pow_abs(v: f64, e: f64) -> f64 {
    if e <= 0.0 {
        return 1.0;
    }
    if e >= 1.0 {
        return v;
    }
    let sign = if v < 0.0 { -1.0 } else { 1.0 };
    sign * libm::pow(v * sign, e)
}

/// ufbx's `ufbxi_f64_to_i64`: saturating, with a NaN at the bottom.
fn f64_to_i64(value: f64) -> i64 {
    if value.abs() <= i64::MAX as f64 {
        value as i64
    } else if value >= 0.0 {
        i64::MAX
    } else {
        i64::MIN
    }
}

#[cfg(test)]
mod tests;
