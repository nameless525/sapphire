//! Exact Euclidean distance transforms of binary masks, after Felzenszwalb and Huttenlocher,
//! "Distance Transforms of Sampled Functions" (Theory of Computing, 2012).
//!
//! The squared distance from every pixel center to the nearest feature pixel's center, in
//! time linear in the pixel count: the 2D transform separates into a 1D transform of every
//! column, then of every row, and each 1D transform is the lower envelope of one parabola
//! per pixel. The result is exact, unlike chamfer, 8SSEDT or jump-flooding approximations,
//! and computed in a fixed order in `f64` on integer inputs, so it is the same on every run
//! and machine.

/// Stands in for infinity inside the envelope: finite, so differences between two such
/// values stay finite, and far above any squared distance in a texture (2 · 16384² ≈ 5.4e8).
const FAR: f64 = 1e20;

/// The squared distance, in pixels², from each pixel of a `width × height` grid (row-major) to
/// the nearest pixel for which `is_feature` holds. `f64::INFINITY` everywhere if none does.
pub(super) fn squared_distances(width: usize, height: usize, is_feature: impl Fn(usize) -> bool) -> Vec<f64> {
    let mut grid: Vec<f64> = (0..width * height).map(|i| if is_feature(i) { 0.0 } else { FAR }).collect();
    let mut envelope = Envelope::new(width.max(height));
    let (mut line, mut out) = (vec![0.0; height], vec![0.0; height]);
    for x in 0..width {
        for (y, value) in line.iter_mut().enumerate() {
            *value = grid[y * width + x];
        }
        envelope.transform(&line, &mut out);
        for (y, &value) in out.iter().enumerate() {
            grid[y * width + x] = value;
        }
    }
    let mut line = vec![0.0; width];
    for row in grid.chunks_exact_mut(width.max(1)) {
        line.copy_from_slice(row);
        envelope.transform(&line, row);
    }
    for value in &mut grid {
        if *value >= FAR {
            *value = f64::INFINITY;
        }
    }
    grid
}

/// Scratch space for the 1D transform: the parabolas of the lower envelope and the
/// boundaries between them.
struct Envelope {
    /// Positions of the parabolas in the envelope, left to right.
    vertices: Vec<usize>,
    /// `boundaries[k]..boundaries[k + 1]` is where parabola `k` is lowest.
    boundaries: Vec<f64>,
}

impl Envelope {
    fn new(len: usize) -> Self {
        Self { vertices: vec![0; len.max(1)], boundaries: vec![0.0; len + 1] }
    }

    /// `out[q] = min over p of (q − p)² + f[p]`.
    fn transform(&mut self, f: &[f64], out: &mut [f64]) {
        if f.is_empty() {
            return;
        }
        let (vertices, boundaries) = (&mut self.vertices, &mut self.boundaries);
        // Where the parabolas rooted at `q` and `p` intersect.
        let intersection = |q: usize, p: usize| {
            let (qf, pf) = (q as f64, p as f64);
            ((f[q] + qf * qf) - (f[p] + pf * pf)) / (2.0 * (qf - pf))
        };
        let mut k = 0;
        vertices[0] = 0;
        boundaries[0] = f64::NEG_INFINITY;
        boundaries[1] = f64::INFINITY;
        for q in 1..f.len() {
            let mut s = intersection(q, vertices[k]);
            // `boundaries[0]` is −∞ and `s` is finite, so this stops at the first parabola.
            while s <= boundaries[k] {
                k -= 1;
                s = intersection(q, vertices[k]);
            }
            k += 1;
            vertices[k] = q;
            boundaries[k] = s;
            boundaries[k + 1] = f64::INFINITY;
        }
        k = 0;
        for (q, value) in out.iter_mut().enumerate() {
            while boundaries[k + 1] < q as f64 {
                k += 1;
            }
            let offset = q.abs_diff(vertices[k]) as f64;
            *value = offset * offset + f[vertices[k]];
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A deterministic scatter of features over a `width × height` grid.
    fn scatter(width: usize, height: usize, seed: u64, density: u64) -> Vec<bool> {
        let mut state = seed;
        (0..width * height)
            .map(|_| {
                state = state.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1_442_695_040_888_963_407);
                (state >> 33) % 100 < density
            })
            .collect()
    }

    fn brute_force(width: usize, height: usize, features: &[bool]) -> Vec<f64> {
        (0..width * height)
            .map(|i| {
                let (x, y) = ((i % width) as f64, (i / width) as f64);
                (0..width * height)
                    .filter(|&j| features[j])
                    .map(|j| {
                        let (fx, fy) = ((j % width) as f64, (j / width) as f64);
                        (x - fx).powi(2) + (y - fy).powi(2)
                    })
                    .fold(f64::INFINITY, f64::min)
            })
            .collect()
    }

    #[test]
    fn matches_brute_force_exactly() {
        for (width, height, density) in
            [(17, 9, 10), (9, 17, 3), (32, 32, 1), (1, 12, 20), (12, 1, 20), (1, 1, 100)]
        {
            for seed in 0..4 {
                let features = scatter(width, height, seed, density);
                let fast = squared_distances(width, height, |i| features[i]);
                assert_eq!(fast, brute_force(width, height, &features), "{width}x{height}, seed {seed}");
            }
        }
    }

    #[test]
    fn no_features_is_infinitely_far() {
        assert!(squared_distances(5, 3, |_| false).iter().all(|d| *d == f64::INFINITY));
        let one = squared_distances(5, 3, |i| i == 7);
        assert_eq!(one[7], 0.0);
        assert_eq!(one[0], 4.0 + 1.0);
        assert!(squared_distances(0, 0, |_| true).is_empty());
    }
}
