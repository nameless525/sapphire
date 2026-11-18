//! The effects' pixels (M6 B5): how much of the frame the sprites and ribbons shade, counted as they're sorted, the
//! estimate a fight's fill is read by ([`VfxRenderStats::pixels`](super::VfxRenderStats::pixels)).
//!
//! Each quad is built on the CPU as `vfx.wgsl`'s vertex stage builds it, a sprite from its middle, size, turn and
//! axis and a ribbon's stretch from its two points' rails, taken through the view as two triangles of a strip,
//! cut at the near plane and the frame's edges, and measured. What a frame shades is their sum, every layer
//! counted: two sparks over one pixel shade it twice. It's an area, not a coverage: a quad shades every fragment
//! it covers, whatever its shape's map leaves of it, so a dot's corners cost what its middle does.
//!
//! - **A share of the target:** the sum is kept as a share of the frame (1 is the whole of it once), and the
//!   record turns it into pixels at the size the effects draw at, the output's after temporal AA's resolve and the
//!   scene's in the main pass, which a frame knows only then. A sample of MSAA is no pixel of its own here.
//! - **What it leaves out:** the mask draw, which shades the same quads again where post reads scene color's
//!   alpha; effect meshes, ground decals and bends, which are counted as draws, not pixels; and depth, so a quad
//!   behind a wall counts what it would cover in front of it: the after-resolve pass tests depth by hand, in the
//!   fragment, after shading has begun.
//!
//! Most quads lie wholly inside the view, and those are measured as they stand; only one that crosses an edge or
//! the near plane is cut. Nothing here allocates.

use glam::{Vec3, Vec4, Vec4Swizzles};

use super::{GpuVfxRibbonPoint, GpuVfxSprite};
use crate::CameraData;

/// The view's bounds in clip space, each a plane a point is inside when its dot with the point isn't negative: the
/// near plane (reverse-Z, so `w - z`, as `CameraData::proj` must be) and the four edges.
const PLANES: [Vec4; 5] = [
    Vec4::new(0.0, 0.0, -1.0, 1.0),
    Vec4::new(1.0, 0.0, 0.0, 1.0),
    Vec4::new(-1.0, 0.0, 0.0, 1.0),
    Vec4::new(0.0, 1.0, 0.0, 1.0),
    Vec4::new(0.0, -1.0, 0.0, 1.0),
];

/// The most corners a triangle has once cut: each plane adds at most one.
const MOST_CORNERS: usize = 3 + PLANES.len();

/// The clipper's room for corners: twice [`MOST_CORNERS`], since rounding can read a plane crossed four times where
/// it's crossed twice and add two corners, and past it a corner is dropped rather than written out of bounds.
const ROOM: usize = 2 * MOST_CORNERS;

/// The share of the frame `sprites` and the stretches between `points` shade, every layer counted, through
/// `camera`: what [`VfxRenderStats::pixels`](super::VfxRenderStats::pixels) is, over the target's pixels.
pub(super) fn screen_share(
    sprites: &[GpuVfxSprite],
    points: &[GpuVfxRibbonPoint],
    camera: &CameraData,
) -> f64 {
    let clip_from_world = camera.proj * camera.view;
    // The camera's axes in the world, the view matrix's rows, as the vertex stage takes them.
    let right = camera.view.row(0).xyz();
    let up = camera.view.row(1).xyz();
    let sprites = sprites.iter().map(|sprite| {
        let corners = sprite_corners(sprite, right, up, camera.position);
        quad_share(corners.map(|corner| clip_from_world * corner.extend(1.0)))
    });
    // A point marked as its ribbon's end joins nothing, as the vertex stage has it.
    let stretches = points.windows(2).filter(|pair| pair[0].ends == 0).map(|pair| {
        let [from, to] = [&pair[0], &pair[1]];
        let corners = [from.base, to.base, from.tip, to.tip].map(Vec3::from);
        quad_share(corners.map(|corner| clip_from_world * corner.extend(1.0)))
    });
    sprites.chain(stretches).sum()
}

/// A sprite's four corners in the world, in the strip's order ((0, 0), (1, 0), (0, 1), (1, 1)), as
/// `vfx_sprite_vertex` places them: facing the camera, turned by its rotation, or along its axis with its width
/// turned toward the eye.
fn sprite_corners(sprite: &GpuVfxSprite, right: Vec3, up: Vec3, eye: Vec3) -> [Vec3; 4] {
    let position = Vec3::from(sprite.position);
    let axis = Vec3::from(sprite.axis);
    let local = |corner: (f32, f32)| (corner.0 * 2.0 - 1.0) * sprite.half_width;
    let along_local = |corner: (f32, f32)| (corner.1 * 2.0 - 1.0) * sprite.half_length;
    [(0.0, 0.0), (1.0, 0.0), (0.0, 1.0), (1.0, 1.0)].map(|corner| {
        let (x, y) = (local(corner), along_local(corner));
        if axis == Vec3::ZERO {
            let (s, c) = sprite.rotation.sin_cos();
            position + right * (c * x - s * y) + up * (s * x + c * y)
        } else {
            let along = axis.normalize_or_zero();
            let side = along.cross(eye - position).normalize_or_zero();
            position + side * x + along * y
        }
    })
}

/// The share of the frame a strip of four corners in clip space shades: its two triangles', each cut to the view,
/// so a quad twisted into a bow counts what it draws.
fn quad_share([a, b, c, d]: [Vec4; 4]) -> f64 {
    f64::from(triangle_share([a, b, c])) + f64::from(triangle_share([b, d, c]))
}

/// The share of the frame a triangle in clip space covers once cut to the view (Sutherland–Hodgman, in place).
fn triangle_share(triangle: [Vec4; 3]) -> f32 {
    let mut corners = [Vec4::ZERO; ROOM];
    corners[..3].copy_from_slice(&triangle);
    let mut count = 3;
    let inside = |corner: &Vec4| PLANES.iter().all(|plane| plane.dot(*corner) >= 0.0);
    if !triangle.iter().all(inside) {
        for plane in PLANES {
            let mut kept = [Vec4::ZERO; ROOM];
            let mut kept_count = 0;
            let mut keep = |corner: Vec4| {
                if let Some(slot) = kept.get_mut(kept_count) {
                    *slot = corner;
                    kept_count += 1;
                }
            };
            for at in 0..count {
                let (from, to) = (corners[at], corners[(at + 1) % count]);
                let (from_side, to_side) = (plane.dot(from), plane.dot(to));
                if from_side >= 0.0 {
                    keep(from);
                }
                if (from_side >= 0.0) != (to_side >= 0.0) {
                    keep(from + (to - from) * (from_side / (from_side - to_side)));
                }
            }
            (corners, count) = (kept, kept_count);
            if count < 3 {
                return 0.0;
            }
        }
    }
    // Every corner left is at or past the near plane, so `w` is positive. The frame is 2 × 2 in NDC.
    let ndc = |corner: Vec4| corner.xy() / corner.w;
    let twice: f32 = (0..count)
        .map(|at| {
            let (from, to) = (ndc(corners[at]), ndc(corners[(at + 1) % count]));
            from.perp_dot(to)
        })
        .sum();
    (twice * 0.5).abs() / 4.0
}

#[cfg(test)]
mod tests {
    use sapphire_look::{VfxLight, VfxRibbonPoint, VfxShape, VfxSprite};

    use super::*;

    /// A camera at 5 m on +Z looking at the origin, 90° tall and square: at the origin the frame is 10 m across.
    fn camera() -> CameraData {
        CameraData::look_at(Vec3::Z * 5.0, Vec3::ZERO, Vec3::Y, std::f32::consts::FRAC_PI_2, 1.0, 0.1)
    }

    fn sprite(position: Vec3, half_width: f32, half_length: f32) -> GpuVfxSprite {
        GpuVfxSprite::new(&VfxSprite {
            position,
            axis: Vec3::ZERO,
            rotation: 0.0,
            half_width,
            half_length,
            color: Vec3::ONE,
            alpha: 1.0,
            additive: 1.0,
            soft: 0.0,
            shape: VfxShape::SoftDot,
            light: VfxLight::Emissive,
        })
        .expect("finite")
    }

    fn share(sprites: &[GpuVfxSprite]) -> f64 {
        screen_share(sprites, &[], &camera())
    }

    /// Within a hundredth of a percent, or a millionth of the frame: a quad cut at the frame's edges is measured in
    /// single precision.
    fn close(seen: f64, expected: f64) {
        assert!((seen - expected).abs() <= 1e-4 * expected + 1e-6, "{seen} against {expected}");
    }

    /// A known effect: a 1 × 2 m sprite facing the camera at the origin, whose 10 m frame it covers a fiftieth of,
    /// turned or not; two of them count twice; one twice as far covers a quarter as much.
    #[test]
    fn a_facing_sprite_shades_its_projected_area() {
        close(share(&[sprite(Vec3::ZERO, 0.5, 1.0)]), 0.02);
        let turned = GpuVfxSprite { rotation: 0.7, ..sprite(Vec3::ZERO, 0.5, 1.0) };
        close(share(&[turned]), 0.02);
        close(share(&[sprite(Vec3::ZERO, 0.5, 1.0); 2]), 0.04);
        close(share(&[sprite(Vec3::new(0.0, 0.0, -5.0), 0.5, 1.0)]), 0.005);
    }

    /// A sprite half off the frame counts the half inside; one behind the camera or wholly off the frame counts
    /// nothing, as does one nearer than the near plane; one just past it, wider than the frame, covers the whole
    /// frame once, and no more.
    #[test]
    fn what_the_frame_cuts_off_shades_nothing() {
        close(share(&[sprite(Vec3::new(5.0, 0.0, 0.0), 1.0, 1.0)]), 0.02);
        close(share(&[sprite(Vec3::new(0.0, 0.0, 10.0), 1.0, 1.0)]), 0.0);
        close(share(&[sprite(Vec3::new(40.0, 0.0, 0.0), 1.0, 1.0)]), 0.0);
        close(share(&[sprite(Vec3::new(0.0, 0.0, 4.95), 100.0, 100.0)]), 0.0);
        close(share(&[sprite(Vec3::new(0.0, 0.0, 4.8), 100.0, 100.0)]), 1.0);
    }

    /// A streak along the view, beside the eye and reaching past it, is cut where it crosses the near plane: the
    /// part in front counts and the part behind doesn't, so its share stays finite and at most one.
    #[test]
    fn a_quad_through_the_near_plane_counts_what_lies_past_it() {
        let through = GpuVfxSprite { axis: Vec3::Z.to_array(), ..sprite(Vec3::new(0.5, 0.0, 4.0), 0.5, 2.0) };
        let seen = share(&[through]);
        assert!(seen.is_finite() && seen > 0.0 && seen <= 1.0, "{seen}");
    }

    /// A streak lies along its axis with its width turned to the eye: across the view it shows its whole length,
    /// pointing at the eye it shows almost none.
    #[test]
    fn a_streak_shows_its_length_across_the_view_and_little_along_it() {
        let streak = |axis: Vec3| GpuVfxSprite { axis: axis.to_array(), ..sprite(Vec3::ZERO, 0.5, 1.0) };
        close(share(&[streak(Vec3::X)]), 0.02);
        assert!(share(&[streak(Vec3::Z)]) < 1e-4);
    }

    /// A ribbon's stretch joins two points' rails, and a point marked as its ribbon's end joins nothing: a 2 × 1 m
    /// band facing the camera, then a point after the end, which adds nothing.
    #[test]
    fn a_ribbon_shades_its_stretches_and_nothing_past_an_end() {
        let point = |x: f32, ends: bool| {
            GpuVfxRibbonPoint::new(&VfxRibbonPoint {
                base: Vec3::new(x, -0.5, 0.0),
                tip: Vec3::new(x, 0.5, 0.0),
                ends,
                core_position: 0.5,
                color: Vec3::ONE,
                alpha: 1.0,
                core: Vec3::ONE,
                core_half_width: 0.1,
                glow_reach: 0.5,
                additive: 1.0,
                soft: 0.0,
            })
            .expect("finite")
        };
        let ribbon = [point(-1.0, false), point(1.0, true), point(3.0, true)];
        close(screen_share(&[], &ribbon, &camera()), 0.02);
        close(screen_share(&[], &ribbon[..1], &camera()), 0.0);
    }

    /// Triangles thrown across the near plane and every edge, with corners exactly on `x = ±w`, `y = ±w` and
    /// `z = w`, never overrun the clipper and always shade between none of the frame and all of it (M6 B5).
    #[test]
    fn a_triangle_cut_by_every_plane_stays_in_the_frame() {
        let mut state = 0x9e37_79b9_7f4a_7c15_u64;
        let mut next = move || {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            (state >> 40) as f32 / (1u64 << 24) as f32
        };
        for _ in 0..20_000 {
            // As the reverse-Z infinite projection has it, clip z is the near distance at every point and `w` the
            // depth, here from 1 m behind the eye to 3 m past it.
            let mut corner = || {
                let w = next() * 4.0 - 1.0;
                let mut corner = Vec4::new(next() * 8.0 - 4.0, next() * 8.0 - 4.0, 0.1, w);
                // On an edge or the near plane exactly, to the bit, a third of the time.
                match (next() * 15.0) as u32 {
                    0 => corner.x = corner.w,
                    1 => corner.x = -corner.w,
                    2 => corner.y = corner.w,
                    3 => corner.y = -corner.w,
                    4 => corner.w = corner.z,
                    _ => {}
                }
                corner
            };
            let triangle = [corner(), corner(), corner()];
            let share = triangle_share(triangle);
            assert!(share.is_finite() && (0.0..=1.0 + 1e-4).contains(&share), "{triangle:?}: {share}");
        }
        // An edge lying on the frame's right edge, through the near plane: kept whole on one, cut on the other.
        let on_edge = [
            Vec4::new(1.0, -1.0, 0.1, 1.0),
            Vec4::new(-0.05, 0.05, 0.1, -0.05),
            Vec4::new(-3.0, 0.0, 0.1, 1.0),
        ];
        let share = triangle_share(on_edge);
        assert!(share > 0.0 && share <= 1.0, "{share}");
    }
}
