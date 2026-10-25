//! Packed vertex elements and the encodings behind them.
//!
//! Every element is a little-endian byte array, the layout GPUs read vertex and storage data
//! in. Each archives as itself with alignment 1 (as [`AssetId`](crate::AssetId) does), so a
//! cooked stream is a `&[T]` where it lies and uploads without conversion.

use bytemuck::{Pod, Zeroable};
use half::f16;

use super::{Bounds, UvBounds};

/// Largest unorm16 value.
const UNORM16_MAX: f32 = 65535.0;
/// Largest snorm16 magnitude. -32768 also decodes to -1, so it is never written.
const SNORM16_MAX: f32 = 32767.0;

/// Declares a packed element: `$len` little-endian bytes with the derives every element shares.
macro_rules! packed {
    ($(#[$meta:meta])* $name:ident, $len:literal) => {
        $(#[$meta])*
        #[derive(
            Clone,
            Copy,
            Debug,
            Default,
            PartialEq,
            Eq,
            Hash,
            Pod,
            Zeroable,
            rkyv::Archive,
            rkyv::Serialize,
            rkyv::Deserialize,
            rkyv::Portable,
            rkyv::bytecheck::CheckBytes,
        )]
        #[rkyv(as = $name)]
        #[bytecheck(crate = rkyv::bytecheck)]
        #[repr(transparent)]
        pub struct $name([u8; $len]);

        impl $name {
            /// The element as cooked data and the GPU hold it.
            pub const fn from_bytes(bytes: [u8; $len]) -> Self {
                Self(bytes)
            }

            pub const fn to_bytes(self) -> [u8; $len] {
                self.0
            }
        }
    };
}

pub(crate) use packed;

packed!(
    /// A position quantized across its mesh's [`Bounds`]: x, y and z as unorm16 (0 at the
    /// minimum, 65 535 at the maximum), then a reserved zero. 8 bytes, read as `Unorm16x4`.
    ///
    /// The error is at most half a step, `extent / 131 070` per axis: 16 µm across a 2.1 m
    /// character, far below a pixel in any close-up.
    QuantizedPosition,
    8
);

impl QuantizedPosition {
    pub fn new([x, y, z]: [u16; 3]) -> Self {
        Self(pack_u16([x, y, z, 0]))
    }

    pub fn get(self) -> [u16; 3] {
        let [x, y, z, _] = unpack_u16(self.0);
        [x, y, z]
    }

    /// The fourth component, which must be zero.
    pub fn reserved(self) -> u16 {
        unpack_u16::<4, 8>(self.0)[3]
    }

    /// The grid point nearest `position`, which should lie inside `bounds` (it is clamped).
    pub fn encode(position: [f32; 3], bounds: &Bounds) -> Self {
        Self::new(std::array::from_fn(|axis| unorm16(position[axis], bounds.min[axis], bounds.max[axis])))
    }

    /// The position this encodes, as the vertex shader computes it.
    pub fn decode(self, bounds: &Bounds) -> [f32; 3] {
        let q = self.get();
        std::array::from_fn(|axis| from_unorm16(q[axis], bounds.min[axis], bounds.max[axis]))
    }
}

packed!(
    /// A position at full precision: f32 x, y and z. 12 bytes, read as `Float32x3`. For meshes
    /// too large for 16 bits across their bounds, or whose edges must meet a neighbor's exactly.
    FloatPosition,
    12
);

impl FloatPosition {
    pub fn new(position: [f32; 3]) -> Self {
        Self(bytemuck::cast(position.map(f32::to_le_bytes)))
    }

    pub fn get(self) -> [f32; 3] {
        let words: [[u8; 4]; 3] = bytemuck::cast(self.0);
        words.map(f32::from_le_bytes)
    }
}

packed!(
    /// The shading normal and the outline normal, each octahedral-encoded as snorm16 × 2
    /// ([`oct_encode`]). 8 bytes, read as two `Snorm16x2`.
    ///
    /// Both directions are within 0.008° of the source. The toon light split is a hard
    /// threshold on N·L, so normal error moves the terminator line directly: 8-bit octahedral
    /// (about 1°) makes it wobble across a smooth face. The outline normal sets the hull's
    /// extrusion and must stay as smooth as it was baked; 8 bits would save nothing anyway,
    /// since the stream would be padded to 8 bytes.
    VertexShading,
    8
);

impl VertexShading {
    pub fn new(normal: [f32; 3], outline_normal: [f32; 3]) -> Self {
        let [a, b] = oct_encode(normal);
        let [c, d] = oct_encode(outline_normal);
        Self(pack_u16([a, b, c, d].map(|v| v as u16)))
    }

    /// The unit shading normal.
    pub fn normal(self) -> [f32; 3] {
        let [a, b, _, _] = unpack_u16(self.0);
        oct_decode([a as i16, b as i16])
    }

    /// The unit outline normal.
    pub fn outline_normal(self) -> [f32; 3] {
        let [_, _, c, d] = unpack_u16(self.0);
        oct_decode([c as i16, d as i16])
    }
}

packed!(
    /// Texture coordinates quantized across the mesh's [`UvBounds`] as unorm16 × 2, then the
    /// RGBA8 vertex color. 8 bytes, read as `Unorm16x2` and `Unorm8x4`.
    ///
    /// Color RGB is an sRGB albedo tint and alpha the outline-width scale (255 = 1.0). For UVs
    /// in [0, 1] the error is 1/131 070, 0.016 texels of a 2K map. Float16 would be the same
    /// size with 1/2048 steps near 1.0, half a texel, enough to shift the hard edges of ILM
    /// masks and SDF face maps.
    VertexSurface,
    8
);

impl VertexSurface {
    pub fn new(uv: [f32; 2], bounds: &UvBounds, color: [u8; 4]) -> Self {
        let [u, v] = std::array::from_fn(|axis| unorm16(uv[axis], bounds.min[axis], bounds.max[axis]));
        let mut bytes = [0; 8];
        bytes[..4].copy_from_slice(&pack_u16::<2, 4>([u, v]));
        bytes[4..].copy_from_slice(&color);
        Self(bytes)
    }

    pub fn uv(self, bounds: &UvBounds) -> [f32; 2] {
        let q: [u16; 2] = unpack_u16(*self.0.first_chunk::<4>().expect("8 bytes hold 4"));
        std::array::from_fn(|axis| from_unorm16(q[axis], bounds.min[axis], bounds.max[axis]))
    }

    pub fn color(self) -> [u8; 4] {
        *self.0.last_chunk::<4>().expect("8 bytes hold 4")
    }
}

/// Octahedral encoding of a direction as snorm16 × 2 (Cigolle et al., "A Survey of Efficient
/// Representations for Independent Unit Vectors", JCGT 2014): the sphere folded onto a square.
///
/// Encoding is "precise": of the four grid points around the exact projection, it keeps the
/// one whose decoded direction is closest, which halves the worst-case error of rounding.
/// `n` need not be unit length; a zero or non-finite vector encodes +Z.
pub fn oct_encode(n: [f32; 3]) -> [i16; 2] {
    let [x, y, z] = n;
    let l1 = x.abs() + y.abs() + z.abs();
    if !(l1.is_finite() && l1 > 0.0) {
        return [0, 0];
    }
    let (u, v) = (x / l1, y / l1);
    let (u, v) = if z < 0.0 { ((1.0 - v.abs()) * sign(u), (1.0 - u.abs()) * sign(v)) } else { (u, v) };
    let (base_u, base_v) = ((u * SNORM16_MAX).floor(), (v * SNORM16_MAX).floor());
    let snorm = |value: f32| value.clamp(-SNORM16_MAX, SNORM16_MAX) as i16;
    let mut best = ([0; 2], f32::NEG_INFINITY);
    for (du, dv) in [(0.0, 0.0), (1.0, 0.0), (0.0, 1.0), (1.0, 1.0)] {
        let candidate = [snorm(base_u + du), snorm(base_v + dv)];
        let [a, b, c] = oct_decode(candidate);
        // Comparing dot products against an unnormalized `n` ranks candidates the same way.
        let dot = a * x + b * y + c * z;
        if dot > best.1 {
            best = (candidate, dot);
        }
    }
    best.0
}

/// The unit vector [`oct_encode`] encoded, computed as the vertex shaders do (`oct_decode` in
/// `frame.wgsl`).
pub fn oct_decode([a, b]: [i16; 2]) -> [f32; 3] {
    // GPUs decode snorm16 as max(q / 32767, -1).
    let (u, v) = ((f32::from(a) / SNORM16_MAX).max(-1.0), (f32::from(b) / SNORM16_MAX).max(-1.0));
    let z = 1.0 - u.abs() - v.abs();
    // Unfolds the lower hemisphere; the result's L1 norm is 1, so it is never zero.
    let t = (-z).max(0.0);
    let x = u + if u >= 0.0 { -t } else { t };
    let y = v + if v >= 0.0 { -t } else { t };
    let length = (x * x + y * y + z * z).sqrt();
    [x / length, y / length, z / length]
}

/// IEEE binary16 bits of `value`, rounded to nearest even.
pub(crate) fn f16_bits(value: f32) -> u16 {
    f16::from_f32(value).to_bits()
}

pub(crate) fn from_f16_bits(bits: u16) -> f32 {
    f16::from_bits(bits).to_f32()
}

/// `value` across `min..=max` as unorm16, clamped. A zero-width range encodes 0.
fn unorm16(value: f32, min: f32, max: f32) -> u16 {
    let extent = max - min;
    if extent.is_nan() || extent <= 0.0 {
        return 0;
    }
    ((value - min) / extent * UNORM16_MAX).round().clamp(0.0, UNORM16_MAX) as u16
}

/// The inverse of [`unorm16`], as the GPU's unorm conversion and the shaders' affine decode
/// compute it.
fn from_unorm16(q: u16, min: f32, max: f32) -> f32 {
    min + f32::from(q) / UNORM16_MAX * (max - min)
}

/// +1 for zero and up, -1 below: the octahedral fold must not map 0 to 0.
fn sign(value: f32) -> f32 {
    if value >= 0.0 { 1.0 } else { -1.0 }
}

/// `N` u16 values as `B` = 2N little-endian bytes.
pub(crate) fn pack_u16<const N: usize, const B: usize>(values: [u16; N]) -> [u8; B] {
    debug_assert_eq!(B, 2 * N);
    let mut bytes = [0; B];
    for (chunk, value) in bytes.as_chunks_mut::<2>().0.iter_mut().zip(values) {
        *chunk = value.to_le_bytes();
    }
    bytes
}

/// The inverse of [`pack_u16`].
pub(crate) fn unpack_u16<const N: usize, const B: usize>(bytes: [u8; B]) -> [u16; N] {
    debug_assert_eq!(B, 2 * N);
    let chunks = bytes.as_chunks::<2>().0;
    std::array::from_fn(|i| u16::from_le_bytes(chunks[i]))
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A deterministic generator for test inputs (64-bit LCG, top 24 bits).
    pub(crate) struct Random(u64);

    impl Random {
        pub fn new(seed: u64) -> Self {
            Self(seed)
        }

        /// Uniform in [0, 1).
        pub fn unit(&mut self) -> f32 {
            self.0 = self.0.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1_442_695_040_888_963_407);
            (self.0 >> 40) as f32 / (1u64 << 24) as f32
        }

        /// Uniform in [min, max).
        pub fn range(&mut self, min: f32, max: f32) -> f32 {
            min + (max - min) * self.unit()
        }

        /// A random unit vector, uniform on the sphere.
        pub fn direction(&mut self) -> [f32; 3] {
            let z = self.range(-1.0, 1.0);
            let phi = self.range(0.0, std::f32::consts::TAU);
            let r = (1.0 - z * z).max(0.0).sqrt();
            [r * phi.cos(), r * phi.sin(), z]
        }
    }

    fn angle_degrees(a: [f32; 3], b: [f32; 3]) -> f64 {
        // In f64, so the measurement isn't limited by f32 rounding near 1.
        let dot: f64 = a.iter().zip(&b).map(|(&a, &b)| f64::from(a) * f64::from(b)).sum();
        let length = |v: [f32; 3]| v.iter().map(|&c| f64::from(c).powi(2)).sum::<f64>().sqrt();
        (dot / (length(a) * length(b))).clamp(-1.0, 1.0).acos().to_degrees()
    }

    #[test]
    fn positions_round_trip_within_half_a_step() {
        let mut random = Random::new(7);
        for _ in 0..200 {
            let min: [f32; 3] = std::array::from_fn(|_| random.range(-50.0, 50.0));
            let extent: [f32; 3] = std::array::from_fn(|_| random.range(0.01, 20.0));
            let bounds = Bounds { min, max: std::array::from_fn(|i| min[i] + extent[i]) };
            for _ in 0..50 {
                let p: [f32; 3] = std::array::from_fn(|i| random.range(bounds.min[i], bounds.max[i]));
                let decoded = QuantizedPosition::encode(p, &bounds).decode(&bounds);
                for axis in 0..3 {
                    // Half a step, plus f32 rounding of the coordinates themselves.
                    let bound =
                        extent[axis] / (2.0 * UNORM16_MAX) + 4.0 * f32::EPSILON * p[axis].abs().max(1.0);
                    let error = (decoded[axis] - p[axis]).abs();
                    assert!(error <= bound, "axis {axis}: error {error} > {bound} for {p:?} in {bounds:?}");
                }
            }
        }
    }

    #[test]
    fn position_bounds_decode_exactly_and_flat_axes_are_zero() {
        let bounds = Bounds { min: [-0.55, 0.0, 2.0], max: [0.55, 2.1, 2.0] };
        assert_eq!(QuantizedPosition::encode(bounds.min, &bounds).get(), [0, 0, 0]);
        assert_eq!(QuantizedPosition::encode(bounds.max, &bounds).get(), [65535, 65535, 0]);
        assert_eq!(QuantizedPosition::encode(bounds.min, &bounds).decode(&bounds), bounds.min);
        assert_eq!(QuantizedPosition::encode(bounds.max, &bounds).decode(&bounds), bounds.max);
        // Outside the bounds clamps; the reserved component stays zero.
        let outside = QuantizedPosition::encode([9.0, -9.0, 5.0], &bounds);
        assert_eq!((outside.get(), outside.reserved()), ([65535, 0, 0], 0));
        assert_eq!(QuantizedPosition::new([1, 2, 3]).to_bytes(), [1, 0, 2, 0, 3, 0, 0, 0]);
        let float = FloatPosition::new([1.5, -2.25, 1e-7]);
        assert_eq!(float.get(), [1.5, -2.25, 1e-7]);
        assert_eq!(&float.to_bytes()[..4], &1.5f32.to_le_bytes());
    }

    #[test]
    fn octahedral_normals_are_within_eight_thousandths_of_a_degree() {
        let mut random = Random::new(11);
        let mut worst = 0.0f64;
        let mut directions: Vec<[f32; 3]> = (0..200_000).map(|_| random.direction()).collect();
        // The fold's seams and the poles.
        for axis in [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0], [1.0, 1.0, 1.0], [1.0, -1.0, 0.0]] {
            for sign in [1.0, -1.0] {
                directions.push(axis.map(|c: f32| c * sign));
            }
        }
        for n in directions {
            worst = worst.max(angle_degrees(n, oct_decode(oct_encode(n))));
        }
        assert!(worst < 0.008, "worst octahedral error {worst:.5}°");
        for axis in [[1.0, 0.0, 0.0], [0.0, -1.0, 0.0], [0.0, 0.0, 1.0], [0.0, 0.0, -1.0]] {
            assert_eq!(oct_decode(oct_encode(axis)), axis, "axes are exact");
        }
        // Not unit length, zero or not finite.
        assert!(angle_degrees(oct_decode(oct_encode([0.0, 3.0, 4.0])), [0.0, 0.6, 0.8]) < 0.008);
        assert_eq!(oct_encode([0.0; 3]), [0, 0]);
        assert_eq!(oct_encode([f32::NAN, 0.0, 1.0]), [0, 0]);
        assert_eq!(oct_decode([0, 0]), [0.0, 0.0, 1.0]);
        // -32768 can't be written but must still decode.
        assert!(oct_decode([i16::MIN, 0]).iter().all(|c| c.is_finite()));
    }

    #[test]
    fn shading_and_surface_keep_their_parts() {
        let shading = VertexShading::new([0.0, 1.0, 0.0], [0.6, 0.0, -0.8]);
        assert_eq!(shading.normal(), [0.0, 1.0, 0.0]);
        assert!(angle_degrees(shading.outline_normal(), [0.6, 0.0, -0.8]) < 0.008);

        let bounds = UvBounds { min: [0.0, -1.0], max: [1.0, 3.0] };
        let surface = VertexSurface::new([0.25, 2.0], &bounds, [10, 20, 30, 140]);
        assert_eq!(surface.color(), [10, 20, 30, 140]);
        let [u, v] = surface.uv(&bounds);
        assert!((u - 0.25).abs() <= 1.0 / 131_070.0 && (v - 2.0).abs() <= 4.0 / 131_070.0, "{u} {v}");
        let flat = UvBounds { min: [0.5, 0.5], max: [0.5, 0.5] };
        assert_eq!(VertexSurface::new([0.5, 0.5], &flat, [0; 4]).uv(&flat), [0.5, 0.5]);
    }

    #[test]
    fn uvs_round_trip_within_half_a_step() {
        let mut random = Random::new(3);
        let bounds = UvBounds { min: [0.0, 0.0], max: [1.0, 1.0] };
        let mut worst = 0.0f32;
        for _ in 0..100_000 {
            let uv = [random.unit(), random.unit()];
            let decoded = VertexSurface::new(uv, &bounds, [0; 4]).uv(&bounds);
            worst = worst.max((decoded[0] - uv[0]).abs()).max((decoded[1] - uv[1]).abs());
        }
        // 0.016 texels of a 2048-texel map.
        assert!(worst * 2048.0 < 0.016, "worst UV error {} texels at 2K", worst * 2048.0);
    }

    #[test]
    fn half_floats_keep_eleven_bits() {
        let mut random = Random::new(5);
        for _ in 0..10_000 {
            let value = random.range(-2.0, 2.0) * 10f32.powf(random.range(-5.0, 1.0));
            let decoded = from_f16_bits(f16_bits(value));
            // binary16 keeps 11 significant bits; below 2^-14 it is subnormal, with steps of 2^-24.
            let bound = (value.abs() * 2f32.powi(-11)).max(2f32.powi(-25));
            assert!((decoded - value).abs() <= bound, "{value} -> {decoded}");
        }
    }
}
