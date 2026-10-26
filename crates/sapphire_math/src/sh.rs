//! Real spherical harmonics, bands 0–2 (M1 W5): what the ambient probe grid stores sky visibility
//! and bounce light in, shared by the cooker's bake and the renderer's relight so both read the
//! coefficients the same way.
//!
//! # Conventions
//!
//! - **Basis:** the real SH of Sloan's "Stupid Spherical Harmonics Tricks" (2008), coefficients in
//!   the order `Y₀₀, Y₁₋₁, Y₁₀, Y₁₁, Y₂₋₂, Y₂₋₁, Y₂₀, Y₂₁, Y₂₂`, as functions of a unit direction's
//!   world-space `(x, y, z)`: band 1 is `(y, z, x)` scaled. The world is Y-up, so `y` is up; the
//!   basis doesn't care which axis is up, only that every reader uses the same one.
//!   L1 is the first 4 coefficients, L2 all 9.
//! - **Irradiance** follows the renderer's Lambert convention (no 1/π: a uniform radiance of 1
//!   gives an irradiance of 1, as `ambient_light()` does in `frame.wgsl`), so convolving radiance
//!   with the clamped cosine scales band `l` by Âₗ/π: 1, 2/3 and 1/4 ([`IRRADIANCE_BANDS`],
//!   Ramamoorthi and Hanrahan 2001).
//! - **Products** of two functions given as SH are evaluated exactly by quadrature on the 12
//!   vertices of the icosahedron ([`ICOSAHEDRON`]), a spherical 5-design: it integrates every
//!   polynomial of degree 5 or less on the sphere exactly, and the product of two L2 functions
//!   projected onto L1 is a polynomial of degree 2 + 2 + 1. This is how the relight multiplies
//!   the sky by a probe's visibility ([`SkyTransfer`]).
//!
//! Everything here is plain scalar `f32` arithmetic and `sqrt`, so the cooker's bake reproduces
//! the same bits on every target (see [`det`](crate::det)).

use glam::Vec3;

/// Coefficients of an L1 function.
pub const L1: usize = 4;
/// Coefficients of an L2 function.
pub const L2: usize = 9;

/// 1 / (2√π).
const C0: f32 = 0.282_094_8;
/// √3 / (2√π).
const C1: f32 = 0.488_602_52;
/// √15 / (2√π).
const C2: f32 = 1.092_548_4;
/// √5 / (4√π).
const C20: f32 = 0.315_391_57;
/// √15 / (4√π).
const C22: f32 = 0.546_274_2;

/// The scale convolving radiance with the clamped cosine gives each band, in the renderer's
/// normalization (Âₗ/π).
pub const IRRADIANCE_BANDS: [f32; 3] = [1.0, 2.0 / 3.0, 0.25];

/// The band coefficient `index` belongs to.
pub const fn band(index: usize) -> usize {
    match index {
        0 => 0,
        1..=3 => 1,
        _ => 2,
    }
}

/// The 9 basis functions at unit direction `d`.
pub fn basis(d: Vec3) -> [f32; L2] {
    let (x, y, z) = (d.x, d.y, d.z);
    [
        C0,
        C1 * y,
        C1 * z,
        C1 * x,
        C2 * x * y,
        C2 * y * z,
        C20 * (3.0 * z * z - 1.0),
        C2 * x * z,
        C22 * (x * x - y * y),
    ]
}

/// The projection of the constant function `value`: full visibility, or a uniform sky.
pub fn project_constant(value: f32) -> [f32; L2] {
    let mut coefficients = [0.0; L2];
    // ∫ value · Y₀₀ dω = value · 4π / (2√π).
    coefficients[0] = value * 4.0 * std::f32::consts::PI * C0;
    coefficients
}

/// The first 4 basis functions (L1) at unit direction `d`.
pub fn basis_l1(d: Vec3) -> [f32; L1] {
    [C0, C1 * d.y, C1 * d.z, C1 * d.x]
}

/// The function `coefficients` (L1 or L2, by its length) holds, at unit direction `d`.
pub fn evaluate(coefficients: &[f32], d: Vec3) -> f32 {
    debug_assert!(matches!(coefficients.len(), L1 | L2), "{} coefficients", coefficients.len());
    basis(d).iter().zip(coefficients).map(|(y, c)| y * c).sum()
}

/// The irradiance (in the renderer's normalization) of the radiance `coefficients` hold: each
/// band scaled by [`IRRADIANCE_BANDS`].
pub fn irradiance<const N: usize>(coefficients: [f32; N]) -> [f32; N] {
    let mut out = coefficients;
    for (index, value) in out.iter_mut().enumerate() {
        *value *= IRRADIANCE_BANDS[band(index)];
    }
    out
}

/// Accumulates a projection onto SH from samples of a function over the sphere: each sample adds
/// its value times the basis there, times the solid angle it stands for.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Projection<const N: usize> {
    pub coefficients: [f32; N],
}

impl<const N: usize> Default for Projection<N> {
    fn default() -> Self {
        Self { coefficients: [0.0; N] }
    }
}

impl<const N: usize> Projection<N> {
    /// Adds `value` at unit direction `d`, standing for `solid_angle` steradians.
    pub fn add(&mut self, d: Vec3, value: f32, solid_angle: f32) {
        debug_assert!(N == L1 || N == L2, "{N} coefficients");
        let y = basis(d);
        for (c, y) in self.coefficients.iter_mut().zip(y) {
            *c += value * solid_angle * y;
        }
    }
}

/// The icosahedron's 12 vertices, unit length: a spherical 5-design (see the [module docs](self)).
pub const ICOSAHEDRON: [Vec3; 12] = {
    // (0, ±1, ±φ) and its cyclic permutations, divided by √(1 + φ²).
    const A: f32 = 0.525_731_1;
    const B: f32 = 0.850_650_8;
    [
        Vec3::new(0.0, A, B),
        Vec3::new(0.0, A, -B),
        Vec3::new(0.0, -A, B),
        Vec3::new(0.0, -A, -B),
        Vec3::new(A, B, 0.0),
        Vec3::new(A, -B, 0.0),
        Vec3::new(-A, B, 0.0),
        Vec3::new(-A, -B, 0.0),
        Vec3::new(B, 0.0, A),
        Vec3::new(-B, 0.0, A),
        Vec3::new(B, 0.0, -A),
        Vec3::new(-B, 0.0, -A),
    ]
};

/// The solid angle each [`ICOSAHEDRON`] vertex stands for: 4π / 12.
pub const ICOSAHEDRON_WEIGHT: f32 = 4.0 * std::f32::consts::PI / 12.0;

/// The L1 projection of the product of two functions given as SH (each L1 or L2), exact (see the
/// [module docs](self)).
pub fn product_l1(a: &[f32], b: &[f32]) -> [f32; L1] {
    let mut out = [0.0; L1];
    for d in ICOSAHEDRON {
        let value = evaluate(a, d) * evaluate(b, d) * ICOSAHEDRON_WEIGHT;
        for (o, y) in out.iter_mut().zip(basis_l1(d)) {
            *o += value * y;
        }
    }
    out
}

/// One color channel of the sky as the relight multiplies it by a probe's visibility: the 4 × 9
/// matrix that takes visibility coefficients (L2, or L1 padded with zeros) to the irradiance (L1,
/// renderer normalization) of the sky light that visibility lets through. Row `i`, column `k`:
/// `Âᵢ/π · Σ_d w L(d) Yᵢ(d) Yₖ(d)` over the icosahedron's vertices `d`, exact for an L2 sky.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SkyTransfer {
    pub rows: [[f32; L2]; L1],
}

impl SkyTransfer {
    /// The transfer of the sky radiance `sky` (L2 coefficients).
    pub fn new(sky: &[f32; L2]) -> Self {
        let mut rows = [[0.0; L2]; L1];
        for d in ICOSAHEDRON {
            let radiance = evaluate(sky, d) * ICOSAHEDRON_WEIGHT;
            let y = basis(d);
            for (i, row) in rows.iter_mut().enumerate() {
                let scale = radiance * y[i] * IRRADIANCE_BANDS[band(i)];
                for (value, yk) in row.iter_mut().zip(y) {
                    *value += scale * yk;
                }
            }
        }
        Self { rows }
    }

    /// The irradiance (L1) of the sky light `visibility` (L1 or L2) lets through.
    pub fn apply(&self, visibility: &[f32]) -> [f32; L1] {
        self.rows.map(|row| row.iter().zip(visibility).map(|(m, v)| m * v).sum())
    }
}

/// Unit directions spread evenly over the sphere: `count` points of the spherical Fibonacci
/// lattice (Marques et al. 2013), deterministic on every target. Each stands for 4π / `count`
/// steradians.
pub fn fibonacci_directions(count: u32) -> Vec<Vec3> {
    // The golden angle, 2π(1 − 1/φ).
    const GOLDEN_ANGLE: f32 = 2.399_963_3;
    (0..count)
        .map(|i| {
            let y = 1.0 - (2.0 * i as f32 + 1.0) / count as f32;
            let radius = (1.0 - y * y).max(0.0).sqrt();
            let (sin, cos) = crate::det::sin_cos(GOLDEN_ANGLE * i as f32);
            Vec3::new(radius * cos, y, radius * sin)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use std::f32::consts::PI;

    use super::*;

    /// A fine, exact-enough integral over the sphere of `f`: a latitude–longitude grid in
    /// (cos θ, φ), uniform in solid angle.
    fn integrate(f: impl Fn(Vec3) -> f32) -> f32 {
        const ROWS: u32 = 400;
        const COLUMNS: u32 = 800;
        let weight = 4.0 * PI / (ROWS * COLUMNS) as f32;
        let mut sum = 0.0f64;
        for row in 0..ROWS {
            let y = 1.0 - 2.0 * (row as f32 + 0.5) / ROWS as f32;
            let radius = (1.0 - y * y).sqrt();
            for column in 0..COLUMNS {
                let phi = 2.0 * PI * (column as f32 + 0.5) / COLUMNS as f32;
                sum += f64::from(f(Vec3::new(radius * phi.cos(), y, radius * phi.sin())) * weight);
            }
        }
        sum as f32
    }

    fn project(f: impl Fn(Vec3) -> f32) -> [f32; L2] {
        std::array::from_fn(|index| integrate(|d| f(d) * basis(d)[index]))
    }

    fn close(a: f32, b: f32, tolerance: f32) -> bool {
        (a - b).abs() <= tolerance
    }

    #[test]
    fn the_basis_is_orthonormal() {
        for i in 0..L2 {
            for j in 0..L2 {
                let dot = integrate(|d| basis(d)[i] * basis(d)[j]);
                let expected = if i == j { 1.0 } else { 0.0 };
                assert!(close(dot, expected, 1e-3), "<Y{i}, Y{j}> = {dot}");
            }
        }
        assert_eq!(basis_l1(Vec3::Y), basis(Vec3::Y)[..L1]);
    }

    #[test]
    fn projection_and_evaluation_reproduce_polynomials_up_to_band_two() {
        // A constant, a linear and a quadratic function are exactly L2.
        let f = |d: Vec3| 0.7 + 0.3 * d.y - 0.2 * d.x + 0.5 * d.x * d.z + 0.25 * d.y * d.y;
        let coefficients = project(f);
        for d in fibonacci_directions(64) {
            assert!(close(evaluate(&coefficients, d), f(d), 2e-3), "at {d}");
        }
        // Sampling through `Projection` with the Fibonacci lattice agrees.
        let mut projection = Projection::<L2>::default();
        let directions = fibonacci_directions(4096);
        let weight = 4.0 * PI / directions.len() as f32;
        for &d in &directions {
            projection.add(d, f(d), weight);
        }
        for (sampled, exact) in projection.coefficients.iter().zip(coefficients) {
            assert!(close(*sampled, exact, 2e-3), "{sampled} vs {exact}");
        }
    }

    #[test]
    fn irradiance_of_known_skies() {
        // A uniform sky of radiance 1 gives an irradiance of 1 in every direction.
        let uniform = irradiance(project(|_| 1.0));
        for d in [Vec3::Y, Vec3::NEG_Y, Vec3::X, Vec3::new(0.6, 0.0, 0.8)] {
            assert!(close(evaluate(&uniform, d), 1.0, 1e-3));
        }
        // Two uniform hemispheres, sky above and ground below: the irradiance is exactly the
        // hemisphere blend `ambient_light()` computes, and lies entirely in L1.
        let (sky, ground) = (0.9, 0.2);
        let hemispheres = irradiance(project(|d| if d.y >= 0.0 { sky } else { ground }));
        for d in fibonacci_directions(32) {
            let expected = ground + (sky - ground) * (0.5 + 0.5 * d.y);
            assert!(close(evaluate(&hemispheres[..L1], d), expected, 2e-3), "at {d}");
        }
        assert!(hemispheres[L1..].iter().all(|c| c.abs() < 2e-3), "no band 2: {hemispheres:?}");
        // The clamped cosine itself, irradiance from a sky that is bright only straight up: its
        // band scales are Ramamoorthi and Hanrahan's.
        let lobe = project(|d| d.y.max(0.0));
        let convolved = irradiance(lobe);
        for (index, (c, l)) in convolved.iter().zip(lobe).enumerate() {
            assert!(close(*c, l * IRRADIANCE_BANDS[band(index)], 1e-6));
        }
    }

    #[test]
    fn icosahedron_products_are_exact() {
        assert!(ICOSAHEDRON.iter().all(|d| close(d.length(), 1.0, 1e-6)));
        let a = project(|d| 0.5 + d.y + 0.3 * d.x * d.y - 0.4 * d.z * d.z);
        let b = project(|d| 0.8 - 0.6 * d.y + 0.2 * d.x + 0.5 * d.y * d.z);
        let exact: [f32; L1] =
            std::array::from_fn(|index| integrate(|d| evaluate(&a, d) * evaluate(&b, d) * basis(d)[index]));
        let quadrature = product_l1(&a, &b);
        for (q, e) in quadrature.iter().zip(exact) {
            assert!(close(*q, e, 2e-3), "{quadrature:?} vs {exact:?}");
        }
        // L1 visibility works the same way, padded or not.
        let b_l1 = &b[..L1];
        let mut padded = [0.0; L2];
        padded[..L1].copy_from_slice(b_l1);
        assert_eq!(product_l1(&a, b_l1), product_l1(&a, &padded));
    }

    #[test]
    fn the_sky_transfer_multiplies_sky_by_visibility() {
        let sky = project(|d| 0.3 + 0.6 * d.y.max(0.0) + 0.1 * d.x);
        let transfer = SkyTransfer::new(&sky);
        // Full visibility (a constant 1) passes the whole sky: its irradiance's L1 part.
        let open = project(|_| 1.0);
        let full = transfer.apply(&open);
        let expected = irradiance(product_l1(&sky, &open));
        for (f, e) in full.iter().zip(expected) {
            assert!(close(*f, e, 1e-5));
        }
        let sky_irradiance = irradiance(sky);
        for (f, e) in full.iter().zip(&sky_irradiance[..L1]) {
            assert!(close(*f, *e, 2e-3), "{full:?} vs {sky_irradiance:?}");
        }
        // Visibility of the upper hemisphere alone lets the sky above through: facing up, a
        // point sees almost all of it, facing down almost none.
        let upper = project(|d| if d.y > 0.0 { 1.0 } else { 0.0 });
        let lit = transfer.apply(&upper);
        assert!(evaluate(&lit, Vec3::Y) > 4.0 * evaluate(&lit, Vec3::NEG_Y).max(0.0));
        // Zero visibility lets nothing through.
        assert_eq!(transfer.apply(&[0.0; L2]), [0.0; L1]);
    }

    #[test]
    fn fibonacci_directions_cover_the_sphere_evenly() {
        let directions = fibonacci_directions(256);
        assert_eq!(directions.len(), 256);
        assert!(directions.iter().all(|d| close(d.length(), 1.0, 1e-5)));
        let mean: Vec3 = directions.iter().sum::<Vec3>() / 256.0;
        assert!(mean.length() < 0.01, "balanced: {mean}");
        assert_eq!(fibonacci_directions(256), directions, "deterministic");
    }
}
