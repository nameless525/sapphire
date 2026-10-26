//! Deterministic noise for the generated samples: a lattice hash, value noise and its fractal
//! sum, and a small random generator. The same seed always paints the same texel, so every
//! sample regenerates byte for byte.

use glam::Vec2;

/// A 32-bit integer hash (Wellons' lowbias32): the noise lattices' random values.
pub(crate) fn hash(mut x: u32) -> u32 {
    x ^= x >> 16;
    x = x.wrapping_mul(0x7feb_352d);
    x ^= x >> 15;
    x = x.wrapping_mul(0x846c_a68b);
    x ^= x >> 16;
    x
}

/// A random value in [0, 1) at lattice point `(x, y)` of noise `seed`.
fn lattice(x: i32, y: i32, seed: u32) -> f32 {
    hash(x as u32 ^ hash(y as u32 ^ hash(seed))) as f32 / 4_294_967_296.0
}

/// Smooth value noise in [0, 1] at `p` (in lattice cells), repeating every `period` cells.
pub(crate) fn value_noise(p: Vec2, period: (i32, i32), seed: u32) -> f32 {
    let cell = p.floor();
    let t = p - cell;
    let fade = t * t * t * (t * (t * 6.0 - 15.0) + 10.0);
    let (x, y) = (cell.x as i32, cell.y as i32);
    let at = |dx: i32, dy: i32| lattice((x + dx).rem_euclid(period.0), (y + dy).rem_euclid(period.1), seed);
    let top = at(0, 0) + (at(1, 0) - at(0, 0)) * fade.x;
    let bottom = at(0, 1) + (at(1, 1) - at(0, 1)) * fade.x;
    top + (bottom - top) * fade.y
}

/// Fractal value noise in [0, 1]: `octaves` octaves, each at twice the frequency and half the
/// amplitude, repeating every `period` cells of the first.
pub(crate) fn fbm(p: Vec2, period: (i32, i32), octaves: u32, seed: u32) -> f32 {
    let (mut sum, mut amplitude, mut total, mut scale) = (0.0, 1.0, 0.0, 1);
    for octave in 0..octaves {
        let at = p * scale as f32;
        sum += amplitude * value_noise(at, (period.0 * scale, period.1 * scale), seed.wrapping_add(octave));
        total += amplitude;
        amplitude *= 0.5;
        scale *= 2;
    }
    sum / total
}

/// A period long enough that noise never repeats within a map.
pub(crate) const NO_PERIOD: i32 = 1 << 20;

pub(crate) fn smoothstep(edge0: f32, edge1: f32, x: f32) -> f32 {
    let t = ((x - edge0) / (edge1 - edge0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// A deterministic generator for placing things.
pub(crate) struct Rng(pub u32);

impl Rng {
    /// Uniform in [0, 1).
    pub(crate) fn next(&mut self) -> f32 {
        self.0 = hash(self.0.wrapping_add(0x9e37_79b9));
        self.0 as f32 / 4_294_967_296.0
    }

    pub(crate) fn range(&mut self, low: f32, high: f32) -> f32 {
        low + (high - low) * self.next()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn noise_stays_in_range_and_repeats_by_its_period() {
        for i in 0..200 {
            let p = Vec2::new(i as f32 * 0.37, i as f32 * 0.91);
            let value = fbm(p, (8, 8), 4, 7);
            assert!((0.0..=1.0).contains(&value), "{value}");
            assert!((value - fbm(p + Vec2::splat(8.0), (8, 8), 4, 7)).abs() < 1e-5, "periodic");
        }
        let mut rng = Rng(3);
        assert!((0..100).map(|_| rng.range(2.0, 4.0)).all(|x| (2.0..4.0).contains(&x)));
    }
}
