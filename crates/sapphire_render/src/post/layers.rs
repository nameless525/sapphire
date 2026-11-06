//! The compositing layers' GPU side (M1 P2): how [`PostParams`]' diffusion glow, character bloom,
//! gradient layers, lens flare, vignette tint and sharpening become the fused pass's constants,
//! and the default lens the flare is drawn with. See the post module's docs for what each does.

use bytemuck::{Pod, Zeroable};
use glam::{Mat4, Vec2, Vec3};

use crate::CameraData;
use sapphire_look::post::{
    CharacterBloom, Diffusion, GradientBlend, GradientLayer, GradientShape, LensFlare, MAX_GRADIENT_LAYERS,
    PostParams,
};

/// The shortest ramp a gradient layer draws, in its positions: shorter ones are hard edges this
/// wide, so the ramp's slope stays finite.
const MIN_RAMP: f32 = 1e-4;

/// Visibility counts a disc pixel as fully visible when the frame there is at least this share of
/// the disc's own brightness: the disc's edge is soft, and the sky and the glow under it add.
pub(crate) const FLARE_VISIBLE_SHARE: f32 = 0.5;

/// What the frame shows of the sun's disc, for the lens flare: where it is on screen and how
/// bright a fully visible disc is, from the main view and the sky's disc ([`SunDisc`]).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct SunOnScreen {
    /// The disc's center in texture coordinates (0, 0 the top left).
    pub uv: Vec2,
    /// The disc's radius in frame heights.
    pub radius: f32,
    /// The luminance of the disc's radiance, before exposure.
    pub luminance: f32,
}

/// The sun's disc as the sky draws it this frame.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct SunDisc {
    /// Unit vector toward the sun.
    pub toward: Vec3,
    /// The disc's angular radius in radians; 0 for a disc the sky doesn't draw.
    pub angular_radius: f32,
    /// Linear RGB: the disc's radiance.
    pub radiance: Vec3,
}

impl SunOnScreen {
    /// Where `disc` lands in `camera`'s unjittered view, if the sky draws it and it is in front
    /// of the camera (it may still be off screen: the flare's visibility test finds that out).
    pub(crate) fn new(camera: &CameraData, disc: SunDisc) -> Option<Self> {
        let luminance = Vec3::new(0.2126, 0.7152, 0.0722).dot(disc.radiance);
        if !(disc.angular_radius > 0.0 && luminance > 0.0 && luminance.is_finite()) {
            return None;
        }
        // A direction is a point at infinity: w = 0.
        let clip = (camera.proj * camera.view) * disc.toward.extend(0.0);
        if !(clip.w > 1e-6 && clip.is_finite()) {
            return None;
        }
        let ndc = clip.truncate() / clip.w;
        let uv = Vec2::new(ndc.x * 0.5 + 0.5, 0.5 - ndc.y * 0.5);
        // Half the NDC height per frame height; the disc is small, so its size is the one it
        // has near the view's axis.
        let radius = disc.angular_radius.tan() * focal_y(&camera.proj) * 0.5;
        (uv.is_finite() && radius.is_finite()).then_some(Self { uv, radius, luminance })
    }
}

/// The projection's vertical focal length (`clip_from_view[1][1]`).
fn focal_y(proj: &Mat4) -> f32 {
    proj.y_axis.y
}

/// The width over the height of `proj`'s frame.
pub(crate) fn aspect(proj: &Mat4) -> f32 {
    let aspect = proj.y_axis.y / proj.x_axis.x;
    if aspect.is_finite() && aspect > 0.0 { aspect } else { 1.0 }
}

/// One element of a lens: a sprite placed along the axis from the sun to its mirror image
/// through the middle of the frame.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct FlareElement {
    /// Where along the axis: 0 on the sun, 0.5 in the middle of the frame, 1 at the sun's mirror.
    pub position: f32,
    /// In frame heights: a disc's or hexagon's radius, a ring's, a streak's half-length.
    pub radius: f32,
    /// A disc's or hexagon's soft edge as a fraction of its radius, a ring's width as one, a
    /// streak's thickness in frame heights.
    pub softness: f32,
    pub kind: FlareKind,
    /// Linear RGB, times the flare's color and intensity.
    pub color: Vec3,
}

/// The shape of a [`FlareElement`] (WGSL `FLARE_*`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum FlareKind {
    /// A ghost: a soft disc with a brighter rim, as an aperture's image.
    Disc = 0,
    /// A thin ring.
    Ring = 1,
    /// A ghost in the shape of a six-bladed aperture.
    Hexagon = 2,
    /// A horizontal, anamorphic streak through the element's center.
    Streak = 3,
}

/// The renderer's default lens (Proposals, tuned by eye on the golden scenes): a warm ring and a
/// streak on the sun, then colored ghosts down the axis, small and bright near the sun, wide and
/// faint past the middle, as anime photography paints them. Per-camera lens assets replace it
/// (post module docs, later milestones).
pub(crate) const DEFAULT_LENS: [FlareElement; FLARE_ELEMENTS] = [
    FlareElement {
        position: 0.0,
        radius: 0.11,
        softness: 0.12,
        kind: FlareKind::Ring,
        color: Vec3::new(1.0, 0.72, 0.42),
    },
    FlareElement {
        position: 0.0,
        radius: 0.55,
        softness: 0.004,
        kind: FlareKind::Streak,
        color: Vec3::new(1.0, 0.82, 0.62),
    },
    FlareElement {
        position: 0.22,
        radius: 0.035,
        softness: 0.35,
        kind: FlareKind::Hexagon,
        color: Vec3::new(1.0, 0.55, 0.25),
    },
    FlareElement {
        position: 0.38,
        radius: 0.016,
        softness: 0.4,
        kind: FlareKind::Disc,
        color: Vec3::new(0.55, 1.0, 0.65),
    },
    FlareElement {
        position: 0.62,
        radius: 0.06,
        softness: 0.6,
        kind: FlareKind::Disc,
        color: Vec3::new(0.35, 0.65, 1.0),
    },
    FlareElement {
        position: 0.85,
        radius: 0.1,
        softness: 0.3,
        kind: FlareKind::Hexagon,
        color: Vec3::new(0.7, 0.45, 1.0),
    },
    FlareElement {
        position: 1.05,
        radius: 0.045,
        softness: 0.5,
        kind: FlareKind::Disc,
        color: Vec3::new(1.0, 0.5, 0.75),
    },
    FlareElement {
        position: 1.3,
        radius: 0.2,
        softness: 0.1,
        kind: FlareKind::Ring,
        color: Vec3::new(0.25, 0.55, 0.65),
    },
];

/// Elements in [`DEFAULT_LENS`] (WGSL `FLARE_ELEMENTS`).
pub(crate) const FLARE_ELEMENTS: usize = 8;

/// One gradient layer as the fused pass reads it (WGSL `GradientLayer`).
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Pod, Zeroable)]
pub(crate) struct GpuGradient {
    /// rgb: the layer's color, sRGB-encoded; w: its opacity times the filter strength.
    pub color: [f32; 4],
    /// xy: a linear layer's direction into the frame, over the frame's extent along it, or a
    /// radial one's center; z: where it has faded out; w: 1 / (start − end).
    /// Exceptional modes store bounded normalized center/end/span coefficients instead.
    pub geometry: [f32; 4],
    /// x: the shape (0 linear, 1 radial); y: how it blends ([`BlendKind`]); z: [`GradientRamp`];
    /// w: the radial normalized span's sign. The unused scale/offset w hold its UV factors.
    pub mode: [u32; 4],
    /// rgb: the affine modes' scale and offset of the frame's value, `a × scale + offset`.
    pub scale: [f32; 4],
    pub offset: [f32; 4],
}

/// How the fused pass blends a gradient layer (WGSL `BLEND_*`). The modes affine in the frame's
/// value `a` are folded on the CPU into `a × scale + offset` (clamped to white), so the shader
/// keeps one case for four modes: normal (`0 · a + b`), multiply (`b · a`), screen
/// (`(1 − b) · a + b`) and add (`a + b`). Overlay and soft light are curves in `a`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum BlendKind {
    Affine = 0,
    Overlay = 1,
    SoftLight = 2,
}

/// How the shader evaluates a gradient's packed geometry (WGSL RAMP_*).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u32)]
enum GradientRamp {
    Direct = 0,
    Normalized = 1,
    Reversed = 2,
    Constant = 3,
    Radial = 4,
}

impl GpuGradient {
    /// `layer` with its opacity scaled by `strength`, if it shows at all.
    fn new(layer: &GradientLayer, strength: f32, aspect: f32) -> Option<Self> {
        let opacity = finite(layer.opacity, 0.0).clamp(0.0, 1.0) * strength;
        if opacity <= 0.0 {
            return None;
        }
        let (radial, xy) = match layer.shape {
            GradientShape::Linear { edge } => {
                let edge = finite(edge, 0.0);
                // From the edge into the frame, y down: the top's is (0, 1).
                let inward = Vec2::new(-edge.sin(), edge.cos());
                (false, inward / (inward.x.abs() + inward.y.abs()))
            }
            GradientShape::Radial { center } => {
                (true, if center.is_finite() { center } else { Vec2::splat(0.5) })
            }
        };
        let start = finite(layer.start, 0.0);
        let mut end = finite(layer.end, 1.0);
        if (start - end).abs() < MIN_RAMP {
            end = start - MIN_RAMP.copysign(start - end);
        }
        let span = start - end;
        // At large finite magnitudes subtracting MIN_RAMP can round back to start. Keep the
        // hard-edge slope finite; shape-specific packing below checks the viewport arithmetic.
        let inverse_span = if span == 0.0 { MIN_RAMP.copysign(span).recip() } else { span.recip() };
        // Wide spans use normalized coefficients. Their sign is explicit because the tiny
        // positive reciprocal may flush to zero on the GPU; ordinary packing stays exact.
        let exceptional = !span.is_finite() || (span != 0.0 && !inverse_span.is_normal());
        let (xy, end, inverse_span, ramp_mode) = if exceptional {
            let span64 = f64::from(start) - f64::from(end);
            let inverse = (1.0 / span64.abs()) as f32;
            let bias = (-f64::from(end) / span64) as f32;
            let xy = if radial {
                Vec2::new((f64::from(xy.x) / span64.abs()) as f32, (f64::from(xy.y) / span64.abs()) as f32)
            } else {
                xy
            };
            (xy, bias, inverse, if span64 < 0.0 { GradientRamp::Reversed } else { GradientRamp::Normalized })
        } else {
            (xy, end, inverse_span, GradientRamp::Direct)
        };
        let color = encoded(layer.color);
        let (kind, scale, offset) = match layer.blend {
            GradientBlend::Normal => (BlendKind::Affine, Vec3::ZERO, color),
            GradientBlend::Multiply => (BlendKind::Affine, color, Vec3::ZERO),
            GradientBlend::Screen => (BlendKind::Affine, Vec3::ONE - color, color),
            GradientBlend::Add => (BlendKind::Affine, Vec3::ONE, color),
            GradientBlend::Overlay => (BlendKind::Overlay, Vec3::ONE, Vec3::ZERO),
            GradientBlend::SoftLight => (BlendKind::SoftLight, Vec3::ONE, Vec3::ZERO),
        };
        let geometry = GradientGeometry {
            values: [xy.x, xy.y, end, inverse_span],
            ramp: ramp_mode,
            span_negative: false,
            uv_factors: [0.0; 2],
        };
        let geometry = if radial {
            pack_radial_gradient(geometry, start, aspect)
        } else {
            pack_linear_gradient(geometry)
        };
        Some(Self {
            color: color.extend(opacity).to_array(),
            geometry: geometry.values,
            mode: [u32::from(radial), kind as u32, geometry.ramp as u32, u32::from(geometry.span_negative)],
            scale: scale.extend(geometry.uv_factors[0]).to_array(),
            offset: offset.extend(geometry.uv_factors[1]).to_array(),
        })
    }
}

/// CPU geometry retains typed routing until the final GPU mode words are written.
struct GradientGeometry {
    values: [f32; 4],
    ramp: GradientRamp,
    span_negative: bool,
    uv_factors: [f32; 2],
}

/// Conservative radial direct-route bounds: 2^30 for aspect, center and endpoint, 2^16 for slope.
/// These select arithmetic, without restricting the accepted authored values.
const DIRECT_RADIAL_COMPONENT_BOUND: f32 = 1_073_741_824.0;
const DIRECT_RADIAL_INVERSE_SPAN_BOUND: f32 = 65_536.0;

/// A linear position lies in [-1,2] after L1 direction normalization, including f32 rounding.
/// Unsafe direct arithmetic therefore saturates throughout the viewport and can be packed once.
fn pack_linear_gradient(mut packed: GradientGeometry) -> GradientGeometry {
    if packed.ramp != GradientRamp::Direct {
        return packed;
    }
    let end = f64::from(packed.values[2]);
    let inverse = f64::from(packed.values[3]);
    let difference = 2.0 + end.abs();
    let safe = f64::from(f32::MAX) * 0.5;
    if difference > safe || difference * inverse.abs() > safe {
        let a = (-1.0 - end) * inverse;
        let b = (2.0 - end) * inverse;
        // Both operands came from finite f32 coefficients, so this f64 clamp is total and finite.
        let ramp = a.clamp(0.0, 1.0);
        debug_assert!(
            a.is_finite() && b.is_finite() && (ramp == 0.0 || ramp == 1.0) && b.clamp(0.0, 1.0) == ramp,
            "unsafe linear arithmetic must saturate throughout in-frame position [-1,2]"
        );
        packed.ramp = GradientRamp::Constant;
        packed.values = [0.0, 0.0, ramp as f32, 0.0];
    }
    packed
}

/// Keep ordinary radial arithmetic byte-exact; normalize only when its viewport intermediates
/// cannot stay finite. The same camera aspect is used for packing and the shader's frame uniform.
fn pack_radial_gradient(mut packed: GradientGeometry, start: f32, aspect: f32) -> GradientGeometry {
    let aspect = if aspect.is_finite() && aspect > 0.0 { aspect } else { 1.0 };
    if packed.ramp != GradientRamp::Direct {
        let diagonal = f64::from(aspect).hypot(1.0);
        packed.uv_factors[0] = (2.0 * f64::from(aspect) / diagonal) as f32;
        packed.uv_factors[1] = (2.0 / diagonal) as f32;
        return packed;
    }
    // Conservative ordinary route: comparisons only, no per-layer hypot/log/powi.
    // Position<=2^33, difference<=2^34, slope<=2^16, metric square<2^123.
    if !aspect.is_subnormal()
        && aspect <= DIRECT_RADIAL_COMPONENT_BOUND
        && packed.values[0].abs() <= DIRECT_RADIAL_COMPONENT_BOUND
        && packed.values[1].abs() <= DIRECT_RADIAL_COMPONENT_BOUND
        && packed.values[2].abs() <= DIRECT_RADIAL_COMPONENT_BOUND
        && packed.values[3].abs() <= DIRECT_RADIAL_INVERSE_SPAN_BOUND
    {
        return packed;
    }
    let x = (f64::from(packed.values[0]).abs() + 1.0) * f64::from(aspect);
    let y = f64::from(packed.values[1]).abs() + 1.0;
    let square = x * x + y * y;
    let diagonal = f64::from(aspect).hypot(1.0);
    let position = 2.0 * square.sqrt() / diagonal;
    let difference = position + f64::from(packed.values[2]).abs();
    let safe = f64::from(f32::MAX) * 0.5;
    if !aspect.is_subnormal()
        && square <= safe
        && f64::from(aspect) * f64::from(aspect) + 1.0 <= safe
        && difference <= safe
        && difference * f64::from(packed.values[3]).abs() <= safe
    {
        return packed;
    }
    let basis_x = 2.0 * f64::from(aspect) / diagonal;
    let basis_y = 2.0 / diagonal;
    let center_x = f64::from(packed.values[0]) * basis_x;
    let center_y = f64::from(packed.values[1]) * basis_y;
    let end = f64::from(packed.values[2]);
    let start = f64::from(start);
    let span = if start == end { f64::from(MIN_RAMP.copysign((start - end) as f32)) } else { start - end };
    let scale = center_x.abs().max(center_y.abs()).max(end.abs()).max(start.abs()).max(1.0);
    packed.values = [
        (center_x / scale) as f32,
        (center_y / scale) as f32,
        (end / scale) as f32,
        (span.abs() / scale) as f32,
    ];
    packed.uv_factors[0] = (basis_x / scale) as f32;
    packed.uv_factors[1] = (basis_y / scale) as f32;
    packed.ramp = GradientRamp::Radial;
    packed.span_negative = span < 0.0;
    packed
}

/// One lens flare element as the fused pass reads it (WGSL `FlareElement`).
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Pod, Zeroable)]
pub(crate) struct GpuFlareElement {
    /// x: position along the axis; y: radius; z: softness; w: the kind (`FlareKind`).
    pub shape: [f32; 4],
    /// rgb: linear color.
    pub color: [f32; 4],
    /// Where it draws anything, in frame heights from its center: xy a box's half-extents, z the
    /// radius of a hole in its middle.
    pub reach: [f32; 4],
}

impl GpuFlareElement {
    fn new(element: &FlareElement) -> Self {
        let FlareElement { position, radius, softness, kind, color } = *element;
        // Pixels outside skip the element, which keeps the flare cheap: a hexagon's corners, a
        // ring's soft edges (its Gaussian falls under 1/1000 three widths from its line), a
        // streak's length and its glow (under 1/1000 seven thicknesses from its line).
        let reach = match kind {
            FlareKind::Disc => [radius, radius, 0.0],
            FlareKind::Hexagon => [radius * 1.155, radius, 0.0],
            FlareKind::Ring => {
                let width = 3.0 * radius * softness;
                [radius + width, radius + width, (radius - width).max(0.0)]
            }
            FlareKind::Streak => [radius, 7.0 * softness, 0.0],
        };
        Self {
            shape: [position, radius, softness, kind as u32 as f32],
            color: color.extend(0.0).to_array(),
            reach: [reach[0], reach[1], reach[2], 0.0],
        }
    }
}

/// The compositing layers' share of the post constants, appended to R3's in `PostUniforms`.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct Layers {
    pub vignette_color: [f32; 4],
    pub character_bloom: [f32; 4],
    pub diffusion: [f32; 4],
    pub diffusion_threshold: [f32; 4],
    pub flare: [f32; 4],
    pub gradient_count: u32,
    pub gradients: [GpuGradient; MAX_GRADIENT_LAYERS],
    pub flare_elements: [GpuFlareElement; FLARE_ELEMENTS],
    /// The RCAS lobe's scale: 0 off, 1 full.
    pub sharpening: f32,
    /// Which of the fused pass's variants the frame needs.
    pub variant: Variant,
}

/// Which pipeline variants a frame draws with (post.wgsl's pipeline-overridable constants): the
/// plain ones draw post exactly as it was before the compositing layers.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Variant {
    /// `POST_LAYERS`: the diffusion glow, gradient layers, lens flare or a tinted vignette.
    pub layers: bool,
    /// `POST_SHARPEN`: RCAS.
    pub sharpen: bool,
    /// `POST_CHARACTERS`: the bloom prefilter tells characters apart.
    pub characters: bool,
    /// `POST_LENS_EFFECTS` (M1 P3): the radial blur or chromatic aberration, which read the
    /// scene in place of sharpening.
    pub lens_effects: bool,
    /// `POST_COLOR_EFFECTS` (M1 P3): the inversion, the duotone or speed lines.
    pub color_effects: bool,
}

impl Variant {
    /// Fused-pass variants, by [`Self::composite_index`]: with or without the layers and the
    /// color effects, reading the scene as it is, sharpened or through the lens effects.
    pub(crate) const COMPOSITES: usize = 12;

    pub(crate) fn composite_index(self) -> usize {
        let scene = if self.lens_effects { 2 } else { usize::from(self.sharpen) };
        usize::from(self.layers) | usize::from(self.color_effects) << 1 | scene << 2
    }

    /// The variant [`Self::composite_index`] gives `index`: its layers, sharpening, lens effects
    /// and color effects.
    pub(crate) fn composite(index: usize) -> (bool, bool, bool, bool) {
        let scene = index >> 2;
        (index & 1 != 0, scene == 1, scene == 2, index & 2 != 0)
    }
}

impl Layers {
    /// The layers of `params`, clamped to working ranges so no input can produce NaN.
    pub(crate) fn new(params: &PostParams, aspect: f32) -> Self {
        let vignette = finite_color(params.vignette.color);
        let tinted = vignette != Vec3::ZERO;
        let character = character_bloom(&params.character_bloom, params);
        let Diffusion { intensity, threshold, color } = params.diffusion;
        let diffusion_intensity = finite(intensity, 0.0).clamp(0.0, 64.0);
        let diffusion_threshold = finite(threshold, 0.0).clamp(0.0, 64.0);
        let LensFlare { intensity: flare_intensity, color: flare_color } = params.lens_flare;
        // Packing is the finite conversion boundary: authored HDR colors remain unrestricted,
        // but an unrepresentable color × intensity saturates rather than uploading infinity.
        let flare = (finite_color(flare_color) * finite(flare_intensity, 0.0).clamp(0.0, 64.0))
            .min(Vec3::splat(f32::MAX));
        let strength = finite(params.filter_strength, 1.0).clamp(0.0, 1.0);
        let mut gradients = [GpuGradient::default(); MAX_GRADIENT_LAYERS];
        let mut gradient_count = 0;
        for layer in params.gradients.iter().filter_map(|layer| GpuGradient::new(layer, strength, aspect)) {
            gradients[gradient_count] = layer;
            gradient_count += 1;
        }
        let sharpening = finite(params.sharpening, 0.0).clamp(0.0, 1.0);
        let diffusion_on = diffusion_intensity > 0.0 && finite_color(color) != Vec3::ZERO;
        Self {
            vignette_color: vignette.extend(0.0).to_array(),
            character_bloom: character.unwrap_or_default(),
            diffusion: finite_color(color)
                .extend(if diffusion_on { diffusion_intensity } else { 0.0 })
                .to_array(),
            // The soft-knee threshold's layout (`PostUniforms::bloom`), the knee half the
            // threshold, as bloom's default: below half the threshold nothing glows at all, which
            // the sRGB encoding would otherwise lift into view over black.
            diffusion_threshold: [
                diffusion_threshold,
                0.5 * diffusion_threshold,
                1.0 / (2.0 * diffusion_threshold + 1e-5),
                0.0,
            ],
            flare: flare.extend(0.0).to_array(),
            gradient_count: gradient_count as u32,
            gradients,
            flare_elements: DEFAULT_LENS.map(|element| GpuFlareElement::new(&element)),
            sharpening,
            variant: Variant {
                layers: tinted || diffusion_on || gradient_count > 0 || flare != Vec3::ZERO,
                sharpen: sharpening > 0.0,
                characters: character.is_some(),
                // The effects' own packing decides (`post::effects`).
                lens_effects: false,
                color_effects: false,
            },
        }
    }

    /// Whether the diffusion glow draws, which needs the bloom chain.
    pub(crate) fn diffusion(&self) -> bool {
        self.diffusion[3] > 0.0
    }

    /// Whether the lens flare draws, wherever the sun is.
    pub(crate) fn lens_flare(&self) -> bool {
        self.flare[..3] != [0.0; 3]
    }
}

/// The characters' bloom threshold, as `PostUniforms::character_bloom` holds it (x threshold,
/// y knee width, z 1 / (4 × knee width), w strength relative to the world's), or `None` when
/// characters bloom as the world does.
fn character_bloom(character: &CharacterBloom, params: &PostParams) -> Option<[f32; 4]> {
    let scale = finite(character.threshold, 1.0).clamp(0.0, 64.0);
    let strength = finite(character.strength, 1.0).clamp(0.0, 64.0);
    if scale == 1.0 && strength == 1.0 {
        return None;
    }
    // As `PostUniforms::new` clamps the world's.
    let world = finite(params.bloom.threshold, 1.0).clamp(0.0, super::lut::SHAPER_MAX);
    let threshold = (world * scale).min(super::lut::SHAPER_MAX);
    let knee = threshold * finite(params.bloom.knee, 0.0).clamp(0.0, 1.0);
    Some([threshold, knee, 1.0 / (4.0 * knee + 1e-5), strength])
}

pub(super) fn finite(value: f32, fallback: f32) -> f32 {
    if value.is_finite() { value } else { fallback }
}

/// Makes `color` nonnegative, falling back to black if the result is not finite.
pub(super) fn finite_color(color: Vec3) -> Vec3 {
    let color = color.max(Vec3::ZERO);
    if color.is_finite() { color } else { Vec3::ZERO }
}

/// Linear `color`, sRGB-encoded and clamped to display range.
pub(super) fn encoded(color: Vec3) -> Vec3 {
    let encode = |c: f32| {
        let c = finite(c, 0.0).clamp(0.0, 1.0);
        if c <= 0.003_130_8 { c * 12.92 } else { 1.055 * c.powf(1.0 / 2.4) - 0.055 }
    };
    Vec3::new(encode(color.x), encode(color.y), encode(color.z))
}

#[cfg(test)]
mod tests {
    use std::f32::consts::{FRAC_PI_2, PI};

    use glam::Vec3;

    use super::*;

    #[test]
    fn ordinary_gradient_block_remains_the_direct_shader_layout() {
        let layer = GradientLayer {
            shape: GradientShape::Linear { edge: 0.0 },
            blend: GradientBlend::Normal,
            color: Vec3::ONE,
            opacity: 0.75,
            start: 0.0,
            end: 1.0,
        };
        let packed = GpuGradient::new(&layer, 1.0, 16.0 / 9.0).expect("visible layer");
        // Frozen ordinary scalar packing, including its encoded-white rounding and -0.
        let white = f32::from_bits(0x3f7fffff);
        let expected = GpuGradient {
            color: [white, white, white, 0.75],
            geometry: [-0.0, 1.0, 1.0, -1.0],
            mode: [0, BlendKind::Affine as u32, GradientRamp::Direct as u32, 0],
            scale: [0.0; 4],
            offset: [white, white, white, 0.0],
        };
        assert_eq!(bytemuck::bytes_of(&packed), bytemuck::bytes_of(&expected));
    }

    #[test]
    fn finite_radial_center_and_subnormal_aspect_are_normalized_before_gpu_arithmetic() {
        let layer = GradientLayer {
            shape: GradientShape::Radial { center: Vec2::new(f32::MAX, 0.0) },
            blend: GradientBlend::Normal,
            color: Vec3::ONE,
            opacity: 1.0,
            start: MIN_RAMP,
            end: 0.0,
        };
        let packed = GpuGradient::new(&layer, 1.0, f32::from_bits(0x7fffff)).expect("visible layer");
        assert_eq!(packed.mode[2], GradientRamp::Radial as u32);
        assert!(packed.geometry[..3].iter().all(|value| value.abs() <= 1.0));
        assert!(packed.geometry[3].is_normal());
        assert!(bytemuck::cast_slice::<GpuGradient, f32>(&[packed]).iter().all(|value| value.is_finite()));
    }

    #[test]
    fn finite_linear_hard_edges_outside_the_frame_are_constant() {
        for (endpoint, ramp) in [(f32::MAX, 0.0), (-f32::MAX, 1.0)] {
            let layer = GradientLayer {
                shape: GradientShape::Linear { edge: 0.0 },
                blend: GradientBlend::Normal,
                color: Vec3::ONE,
                opacity: 1.0,
                start: endpoint,
                end: endpoint,
            };
            let packed = GpuGradient::new(&layer, 1.0, 1.0).expect("visible layer");
            assert_eq!(packed.mode[2], GradientRamp::Constant as u32);
            assert_eq!(packed.geometry[2], ramp);
        }
    }

    #[test]
    fn finite_derived_layer_values_are_representable_when_packed() {
        let params = PostParams {
            lens_flare: LensFlare { color: Vec3::splat(f32::MAX), intensity: 64.0 },
            ..PostParams::default()
        };
        let layers = Layers::new(&params, 1.0);
        assert!(layers.flare.iter().all(|x| x.is_finite()));
        assert_eq!(layers.flare[0], f32::MAX);
        for at in [f32::MAX, -f32::MAX, 1e20] {
            let layer = GradientLayer { start: at, end: at, opacity: 1.0, ..GradientLayer::NONE };
            let packed = GpuGradient::new(&layer, 1.0, 1.0).expect("visible layer");
            assert!(packed.geometry.iter().all(|x| x.is_finite()), "{at}: {packed:?}");
        }
    }
    use sapphire_look::post::{BloomParams, Vignette};

    #[test]
    fn plain_parameters_need_only_the_plain_variants() {
        let layers = Layers::new(&PostParams::default(), 1.0);
        assert_eq!(layers.variant, Variant::default());
        let plain = PostParams {
            vignette: Vignette { color: Vec3::new(0.0, 0.0, 0.3), ..Vignette::default() },
            diffusion: Diffusion { intensity: 2.0, threshold: 0.2, color: Vec3::ONE },
            sharpening: 0.5,
            ..PostParams::default()
        }
        .plain();
        assert_eq!(Layers::new(&plain, 1.0).variant, Variant::default());
        let tinted = PostParams {
            vignette: Vignette { color: Vec3::new(0.0, 0.0, 0.3), ..Vignette::default() },
            ..PostParams::default()
        };
        assert_eq!(Layers::new(&tinted, 1.0).variant, Variant { layers: true, ..Variant::default() });
        let sharp = PostParams { sharpening: 0.4, ..PostParams::default() };
        assert_eq!(Layers::new(&sharp, 1.0).variant, Variant { sharpen: true, ..Variant::default() });
        // A diffusion without color, or a flare without intensity, draws nothing.
        let dark = PostParams {
            diffusion: Diffusion { intensity: 3.0, threshold: 0.0, color: Vec3::ZERO },
            lens_flare: LensFlare { intensity: 0.0, color: Vec3::ONE },
            ..PostParams::default()
        };
        assert_eq!(Layers::new(&dark, 1.0).variant, Variant::default());
    }

    #[test]
    fn every_fused_pass_variant_has_its_own_pipeline_and_the_lens_effects_replace_sharpening() {
        let mut seen = [false; Variant::COMPOSITES];
        for bits in 0..16u8 {
            let [layers, sharpen, lens_effects, color_effects] = [0, 1, 2, 3].map(|bit| bits >> bit & 1 != 0);
            let variant = Variant { layers, sharpen, lens_effects, color_effects, characters: false };
            let index = variant.composite_index();
            seen[index] = true;
            assert_eq!(
                Variant::composite(index),
                (layers, sharpen && !lens_effects, lens_effects, color_effects)
            );
        }
        assert!(seen.iter().all(|&seen| seen), "every pipeline is drawn with");
        assert_eq!(Variant::default().composite_index(), 0, "the plain pass, R3's");
    }

    #[test]
    fn characters_bloom_with_their_threshold_and_strength_relative_to_the_world() {
        let params = PostParams {
            bloom: BloomParams { threshold: 0.8, knee: 0.5, ..BloomParams::default() },
            character_bloom: CharacterBloom { threshold: 1.5, strength: 0.25 },
            ..PostParams::default()
        };
        let layers = Layers::new(&params, 1.0);
        assert!(layers.variant.characters);
        let [threshold, knee, _, strength] = layers.character_bloom;
        assert!((threshold - 1.2).abs() < 1e-6 && (knee - 0.6).abs() < 1e-6 && strength == 0.25);
        let neutral = PostParams { character_bloom: CharacterBloom::NEUTRAL, ..params };
        assert!(!Layers::new(&neutral, 1.0).variant.characters);
        let broken = PostParams {
            character_bloom: CharacterBloom { threshold: f32::NAN, strength: f32::INFINITY },
            ..params
        };
        assert!(!Layers::new(&broken, 1.0).variant.characters, "non-finite values fall back to neutral");
    }

    #[test]
    fn gradient_layers_pack_in_order_and_skip_empty_slots() {
        let top = GradientLayer {
            shape: GradientShape::Linear { edge: 0.0 },
            blend: GradientBlend::Multiply,
            color: Vec3::new(0.2, 0.3, 0.8),
            opacity: 0.5,
            start: 0.0,
            end: 0.4,
        };
        let right = GradientLayer { shape: GradientShape::Linear { edge: FRAC_PI_2 }, ..top };
        let diagonal = GradientLayer { shape: GradientShape::Linear { edge: PI * 0.75 }, ..top };
        let glow = GradientLayer {
            shape: GradientShape::Radial { center: Vec2::new(0.5, 0.4) },
            blend: GradientBlend::Screen,
            start: 0.3,
            end: 0.3,
            ..top
        };
        let params = PostParams {
            gradients: [top, GradientLayer::NONE, right, diagonal],
            filter_strength: 0.5,
            ..PostParams::default()
        };
        let layers = Layers::new(&params, 1.0);
        assert_eq!(layers.gradient_count, 3);
        assert!(layers.variant.layers);
        let [top, right, diagonal, _] = layers.gradients;
        // Multiply is affine: the frame's value times the color.
        assert_eq!(top.mode, [0, BlendKind::Affine as u32, 0, 0]);
        assert_eq!((&top.scale[..3], &top.offset[..3]), (&top.color[..3], &[0.0; 3][..]));
        assert!(top.geometry[0].abs() < 1e-6 && (top.geometry[1] - 1.0).abs() < 1e-6, "{top:?}");
        assert_eq!(top.color[3], 0.25, "opacity × filter strength");
        assert!((right.geometry[0] + 1.0).abs() < 1e-6, "from the right, leftward: {right:?}");
        // Corner to corner: the direction over the frame's extent along it.
        let [x, y, ..] = diagonal.geometry;
        assert!((x.abs() + y.abs() - 1.0).abs() < 1e-6 && x < 0.0 && y < 0.0, "{diagonal:?}");
        let radial = Layers::new(&PostParams { gradients: [glow; MAX_GRADIENT_LAYERS], ..params }, 1.0);
        let ramp = radial.gradients[0].geometry;
        assert!(ramp[3].is_finite() && ramp[3].abs() >= 1.0 / MIN_RAMP * 0.99, "a hard edge: {ramp:?}");
        let none = PostParams { filter_strength: 0.0, ..params };
        assert_eq!(Layers::new(&none, 1.0).gradient_count, 0, "filter strength 0 leaves the frame untinted");
    }

    #[test]
    fn the_sun_lands_where_the_camera_sees_it() {
        let camera = CameraData::look_at(Vec3::ZERO, Vec3::NEG_Z, Vec3::Y, 1.0, 2.0, 0.1);
        let disc =
            SunDisc { toward: Vec3::NEG_Z, angular_radius: 0.01, radiance: Vec3::new(40.0, 38.0, 35.0) };
        let sun = SunOnScreen::new(&camera, disc).expect("in front");
        assert!(sun.uv.abs_diff_eq(Vec2::splat(0.5), 1e-6), "{sun:?}");
        // 0.01 rad at a vertical field of view of 1 rad.
        assert!((sun.radius - 0.01_f32.tan() / (0.5_f32.tan() * 2.0)).abs() < 1e-6);
        assert!((aspect(&camera.proj) - 2.0).abs() < 1e-5);
        let up = SunOnScreen::new(&camera, SunDisc { toward: Vec3::new(0.0, 0.2, -1.0).normalize(), ..disc })
            .expect("in front");
        assert!(up.uv.y < 0.5 && (up.uv.x - 0.5).abs() < 1e-6, "above the middle: {up:?}");
        assert_eq!(SunOnScreen::new(&camera, SunDisc { toward: Vec3::Z, ..disc }), None, "behind");
        assert_eq!(SunOnScreen::new(&camera, SunDisc { angular_radius: 0.0, ..disc }), None, "no disc");
        assert_eq!(SunOnScreen::new(&camera, SunDisc { radiance: Vec3::ZERO, ..disc }), None, "dark");
    }
}
