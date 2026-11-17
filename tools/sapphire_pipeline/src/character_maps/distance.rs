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
    transform(width, height, is_feature, None)
}

/// The nearest pixel for which `is_feature` holds to each pixel of a `width × height` grid
/// (row-major), by its index: the feature transform beside [`squared_distances`], from the same
/// envelopes, so ties go the same way on every run. `None` everywhere if no pixel is a feature.
pub(super) fn nearest_features(
    width: usize,
    height: usize,
    is_feature: impl Fn(usize) -> bool,
) -> Vec<Option<usize>> {
    let mut nearest = vec![0; width * height];
    let squared = transform(width, height, is_feature, Some(&mut nearest));
    squared.iter().zip(nearest).map(|(squared, index)| squared.is_finite().then_some(index)).collect()
}

/// [`squared_distances`], and where `nearest` is given, each pixel's nearest feature's index in
/// it.
fn transform(
    width: usize,
    height: usize,
    is_feature: impl Fn(usize) -> bool,
    mut nearest: Option<&mut [usize]>,
) -> Vec<f64> {
    let mut grid: Vec<f64> = (0..width * height).map(|i| if is_feature(i) { 0.0 } else { FAR }).collect();
    let mut envelope = Envelope::new(width.max(height));
    let (mut line, mut out) = (vec![0.0; height], vec![0.0; height]);
    // The column pass's nearest feature row for each pixel, and a line's sources.
    let mut rows = if nearest.is_some() { vec![0; width * height] } else { Vec::new() };
    let mut sources = vec![0; width.max(height)];
    for x in 0..width {
        for (y, value) in line.iter_mut().enumerate() {
            *value = grid[y * width + x];
        }
        let track = nearest.is_some().then_some(&mut sources[..height]);
        envelope.transform(&line, &mut out, track);
        for (y, &value) in out.iter().enumerate() {
            grid[y * width + x] = value;
            if nearest.is_some() {
                rows[y * width + x] = sources[y];
            }
        }
    }
    let mut line = vec![0.0; width];
    for (y, row) in grid.chunks_exact_mut(width.max(1)).enumerate() {
        line.copy_from_slice(row);
        let track = nearest.is_some().then_some(&mut sources[..width]);
        envelope.transform(&line, row, track);
        if let Some(nearest) = nearest.as_deref_mut() {
            for (x, &column) in sources[..width].iter().enumerate() {
                nearest[y * width + x] = rows[y * width + column] * width + column;
            }
        }
    }
    for value in &mut grid {
        if *value >= FAR {
            *value = f64::INFINITY;
        }
    }
    grid
}

/// The distance, in pixels, from each pixel's center to the edge of an anti-aliased shape whose
/// coverage of each pixel of a `width × height` grid (row-major) is `coverage`, 0 to 1, after
/// Gustavson and Strand, "Anti-aliasing Euclidean distance transform" (Pattern Recognition
/// Letters, 2011): the edge is where coverage crosses one half, and a gray pixel's value says
/// how far across it the edge lies. 0 for pixels the shape covers at least half of, and
/// `f64::INFINITY` everywhere if it covers no pixel at all.
///
/// The edge's distance from a pixel `p` is the least, over the pixels `q` the shape touches
/// near `p`'s nearest one, of the distance from `p`'s center to `q`'s plus the signed distance
/// from `q`'s center to the edge across it ([`edge_offset`]), whose direction is the way from
/// `q` to `p` (the local gradient where `q` is `p`). Gustavson and Strand propagate their
/// nearest pixel by that measure itself, approximately; this takes the exact nearest pixel by
/// center distance ([`nearest_features`]) and searches its neighbors, which holds the same pixel
/// wherever the edge isn't folded within a pixel. Lines across gray pixels normal to the local
/// gradient measured worse on a face map's edges (M6 C6: 0.069 texels against 0.020), the
/// gradient's direction being a few degrees off. Every step is a fixed sequence of `f64`
/// operations on the inputs, so the result is the same on every run and machine.
pub(super) fn anti_aliased_distances(width: usize, height: usize, coverage: &[f32]) -> Vec<f64> {
    let cover = |i: usize| f64::from(coverage[i]);
    let nearest = nearest_features(width, height, |i| cover(i) > 0.0);
    (0..width * height)
        .map(|p| {
            if cover(p) >= 0.5 {
                return 0.0;
            }
            let Some(closest) = nearest[p] else { return f64::INFINITY };
            let (px, py) = ((p % width) as i64, (p / width) as i64);
            let (cx, cy) = ((closest % width) as i64, (closest / width) as i64);
            let mut best = f64::INFINITY;
            for (qx, qy) in (-1..=1).flat_map(|dy| (-1..=1).map(move |dx| (cx + dx, cy + dy))) {
                if !(0..width as i64).contains(&qx) || !(0..height as i64).contains(&qy) {
                    continue;
                }
                let q = qy as usize * width + qx as usize;
                let a = cover(q);
                if a <= 0.0 {
                    continue;
                }
                let (dx, dy) = ((px - qx) as f64, (py - qy) as f64);
                let distance = if q == p {
                    let (gx, gy) = gradient(width, height, &cover, q);
                    edge_offset(gx, gy, a)
                } else {
                    (dx * dx + dy * dy).sqrt() + edge_offset(dx, dy, a)
                };
                best = best.min(distance);
            }
            // Never negative where the shape covers under half; held for rounding.
            best.max(0.0)
        })
        .collect()
}

/// The coverage's gradient at pixel `i`, by Gustavson and Strand's isotropic 3 × 3 operator,
/// with pixels past the grid's border taking the nearest pixel's value.
fn gradient(width: usize, height: usize, cover: &impl Fn(usize) -> f64, i: usize) -> (f64, f64) {
    let (x, y) = ((i % width) as i64, (i / width) as i64);
    let at = |dx: i64, dy: i64| {
        let (sx, sy) = ((x + dx).clamp(0, width as i64 - 1), (y + dy).clamp(0, height as i64 - 1));
        cover(sy as usize * width + sx as usize)
    };
    let root2 = std::f64::consts::SQRT_2;
    let gx = at(1, -1) + root2 * at(1, 0) + at(1, 1) - at(-1, -1) - root2 * at(-1, 0) - at(-1, 1);
    let gy = at(-1, 1) + root2 * at(0, 1) + at(1, 1) - at(-1, -1) - root2 * at(0, -1) - at(1, -1);
    (gx, gy)
}

/// The signed distance from a pixel's center to a straight edge across it, normal to `(gx, gy)`
/// (either way along it), that leaves `a` of the unit pixel on the shape's side: positive when
/// the center lies outside the shape (`a` under one half), 0 for half. Gustavson and Strand's
/// exact area-to-distance relation for a line across a square, by octant.
fn edge_offset(gx: f64, gy: f64, a: f64) -> f64 {
    if gx == 0.0 || gy == 0.0 {
        // Along an axis the relation is linear, and with no direction it's a fair guess.
        return 0.5 - a;
    }
    // Written out: `hypot` isn't pinned across platforms, and `sqrt` is exactly rounded.
    let length = (gx * gx + gy * gy).sqrt();
    let (gx, gy) = ((gx / length).abs(), (gy / length).abs());
    let (gx, gy) = if gx < gy { (gy, gx) } else { (gx, gy) };
    let a1 = 0.5 * gy / gx;
    if a < a1 {
        // The edge clips a corner of the pixel.
        0.5 * (gx + gy) - (2.0 * gx * gy * a).sqrt()
    } else if a < 1.0 - a1 {
        (0.5 - a) * gx
    } else {
        -0.5 * (gx + gy) + (2.0 * gx * gy * (1.0 - a)).sqrt()
    }
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

    /// `out[q] = min over p of (q − p)² + f[p]`, and where `sources` is given, `sources[q]` that
    /// `p`.
    fn transform(&mut self, f: &[f64], out: &mut [f64], mut sources: Option<&mut [usize]>) {
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
            if let Some(sources) = sources.as_deref_mut() {
                sources[q] = vertices[k];
            }
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
    fn nearest_features_are_at_the_transforms_distance() {
        for (width, height, density) in [(17, 9, 10), (9, 17, 3), (32, 32, 1), (1, 12, 20), (12, 1, 20)] {
            for seed in 0..4 {
                let features = scatter(width, height, seed, density);
                let squared = squared_distances(width, height, |i| features[i]);
                let nearest = nearest_features(width, height, |i| features[i]);
                for (i, (nearest, squared)) in nearest.iter().zip(&squared).enumerate() {
                    let Some(j) = *nearest else {
                        assert_eq!(*squared, f64::INFINITY);
                        continue;
                    };
                    assert!(features[j], "{width}x{height}, seed {seed}: {i} -> {j}");
                    let (dx, dy) = ((i % width).abs_diff(j % width), (i / width).abs_diff(j / width));
                    assert_eq!((dx * dx + dy * dy) as f64, *squared);
                }
            }
        }
        assert!(nearest_features(4, 4, |_| false).iter().all(Option::is_none));
    }

    /// A straight edge's coverage of each pixel of a `size`² grid: the share of a fine grid of
    /// points inside `inside`.
    fn coverage(size: usize, inside: impl Fn(f64, f64) -> bool) -> Vec<f32> {
        const POINTS: usize = 64;
        (0..size * size)
            .map(|i| {
                let (x, y) = ((i % size) as f64, (i / size) as f64);
                let points = (0..POINTS * POINTS).filter(|&k| {
                    let offset = |p: usize| (p as f64 + 0.5) / POINTS as f64 - 0.5;
                    inside(x + offset(k % POINTS), y + offset(k / POINTS))
                });
                (points.count() as f64 / (POINTS * POINTS) as f64) as f32
            })
            .collect()
    }

    #[test]
    fn anti_aliased_distances_find_an_edge_inside_a_pixel() {
        // A vertical edge at x = 9.3 (pixel centers at whole numbers): every pixel right of it is
        // its distance from the edge, which binary distances would put at 9.5.
        let size = 24;
        let distances = anti_aliased_distances(size, size, &coverage(size, |x, _| x < 9.3));
        for x in 10..size {
            let error = (distances[8 * size + x] - (x as f64 - 9.3)).abs();
            assert!(error < 0.01, "x {x}: {}", distances[8 * size + x]);
        }
        assert_eq!(distances[8 * size + 9], 0.0, "covered at least half");
        // A sloped edge, through (12, 12) at 30° from vertical: a few pixels out, on average
        // within a twentieth of a pixel of the exact distance and never a tenth off, where
        // binary distances (to the centers, less half a pixel) are off by a fifth on average.
        let (sin, cos) = 30f64.to_radians().sin_cos();
        let signed = |x: f64, y: f64| (x - 12.0) * cos - (y - 12.0) * sin;
        let cover = coverage(size, |x, y| signed(x, y) < 0.0);
        let distances = anti_aliased_distances(size, size, &cover);
        let binary = squared_distances(size, size, |i| cover[i] >= 0.5);
        let (mut count, mut error, mut binary_error) = (0.0, 0.0, 0.0);
        for i in 0..size * size {
            let (x, y) = ((i % size) as f64, (i / size) as f64);
            let exact = signed(x, y);
            if (1.0..6.0).contains(&exact) && (4.0..20.0).contains(&y) {
                assert!((distances[i] - exact).abs() < 0.1, "({x}, {y}): {} vs {exact}", distances[i]);
                count += 1.0;
                error += (distances[i] - exact).abs();
                binary_error += (binary[i].sqrt() - 0.5 - exact).abs();
            }
        }
        let (error, binary_error) = (error / count, binary_error / count);
        assert!(error < 0.05 && binary_error > 0.15, "{error} against binary's {binary_error}");
        assert!(anti_aliased_distances(3, 3, &[0.0; 9]).iter().all(|d| *d == f64::INFINITY));
    }

    #[test]
    fn edge_offsets_follow_the_covered_area() {
        // Along an axis, the edge sits `0.5 - a` from the center; half covered, on it.
        assert_eq!(edge_offset(1.0, 0.0, 0.25), 0.25);
        assert_eq!(edge_offset(0.0, -1.0, 0.75), -0.25);
        assert_eq!(edge_offset(1.0, 1.0, 0.5), 0.0);
        // On the diagonal, a corner's triangle of area `a` puts the edge
        // `sqrt(2)/2 - sqrt(a)` from the center.
        let a: f64 = 0.125;
        assert!((edge_offset(1.0, 1.0, a) - (0.5f64.sqrt() - a.sqrt())).abs() < 1e-12);
        assert!((edge_offset(-1.0, 1.0, 1.0 - a) + (0.5f64.sqrt() - a.sqrt())).abs() < 1e-12);
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
