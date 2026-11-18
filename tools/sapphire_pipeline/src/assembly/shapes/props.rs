//! Turned and boxed props: `#column` and `#lantern`.

use glam::{Vec2, Vec3};
use sapphire_math::det;
use sapphire_zone::prefab::shapes::*;

use super::{Detail, Mesh, Part, check, finite};
use crate::assembly::ShapeUv;

pub(super) trait ColumnGeometry {
    fn validate(&self) -> Result<(), String>;
    fn build(&self, mesh: &mut Mesh, detail: Detail);
}

impl ColumnGeometry for Column {
    fn validate(&self) -> Result<(), String> {
        let [base_radius, base_height] = self.base;
        let [cap_width, cap_height] = self.cap;
        check(finite(&[self.height, self.radius, base_radius, base_height, cap_width, cap_height]), || {
            "a column's numbers must be finite".to_owned()
        })?;
        check(self.height > 0.0 && self.radius > 0.0 && (3..=64).contains(&self.sides), || {
            "a column needs a height, a radius and 3 to 64 sides".to_owned()
        })?;
        check(base_height == 0.0 || base_radius > self.radius, || {
            "a column's drum must be wider than it".to_owned()
        })?;
        check(cap_height == 0.0 || cap_width > 2.0 * self.radius, || {
            "a column's block must be wider than it".to_owned()
        })?;
        check(base_height.max(0.0) + cap_height.max(0.0) < self.height, || {
            "a column's drum and block must fit its height".to_owned()
        })
    }

    fn build(&self, mesh: &mut Mesh, detail: Detail) {
        let sides = if detail == Detail::Light { self.sides.min(8) } else { self.sides };
        let [base_radius, base_height] = self.base;
        let [cap_width, cap_height] = self.cap;
        let r = self.radius;
        if base_height > 0.0 {
            let profile = [
                (base_radius, 0.0),
                (base_radius, 0.55 * base_height),
                (0.5 * (base_radius + r) + 0.02, base_height),
                (r, base_height),
            ];
            mesh.lathe(Part::Plinth, Vec3::ZERO, &profile, sides, [false, false], false);
        }
        let shaft_top = self.height - cap_height;
        mesh.lathe(
            Part::Timber,
            Vec3::ZERO,
            &[(r, base_height), (r, shaft_top)],
            sides,
            [false, cap_height <= 0.0],
            false,
        );
        if cap_height > 0.0 {
            let half = 0.5 * cap_width;
            mesh.cuboid(
                Part::Timber,
                Vec3::new(-half, shaft_top, -half),
                Vec3::new(half, self.height, half),
                &[],
            );
        }
    }
}

pub(super) trait LanternGeometry {
    fn validate(&self) -> Result<(), String>;
    /// The caps' height.
    fn cap(&self) -> f32;
    fn build(&self, mesh: &mut Mesh, detail: Detail);
}

impl LanternGeometry for Lantern {
    fn validate(&self) -> Result<(), String> {
        let [width, height] = self.size;
        check(finite(&self.size) && width > 0.0 && height > 0.0, || {
            format!("a lantern's size {:?} must be positive", self.size)
        })?;
        check((3..=32).contains(&self.sides), || "a lantern needs 3 to 32 sides".to_owned())?;
        let reach = match self.mount {
            Mount::Hang(length) | Mount::Post(length) => length,
            Mount::Bare => 0.0,
        };
        check(reach.is_finite() && reach >= 0.0, || "a lantern's cord or post can't be negative".to_owned())
    }

    fn cap(&self) -> f32 {
        (0.1 * self.size[1]).max(0.03)
    }

    fn build(&self, mesh: &mut Mesh, detail: Detail) {
        let sides = if detail == Detail::Light { self.sides.min(6) } else { self.sides };
        let [width, height] = self.size;
        let cap = self.cap();
        let total = height + 2.0 * cap;
        // The lantern's foot, from its mount.
        let foot = match self.mount {
            Mount::Hang(length) => -(length + total),
            Mount::Post(post) => post,
            Mount::Bare => 0.0,
        };
        let base = Vec3::Y * foot;
        let radius = 0.5 * width;
        match self.kind {
            LanternKind::Round | LanternKind::Drum => {
                let (fractions, swell): (&[f32], fn(f32) -> f32) = match self.kind {
                    LanternKind::Round => (&[0.0, 0.1, 0.25, 0.5, 0.75, 0.9, 1.0], |t| {
                        0.42 + 0.58 * det::pow(det::sin(std::f32::consts::PI * t).max(0.0), 0.7)
                    }),
                    _ => (&[0.0, 0.25, 0.5, 0.75, 1.0], |t| 0.9 + 0.1 * det::sin(std::f32::consts::PI * t)),
                };
                let paper: Vec<(f32, f32)> =
                    fractions.iter().map(|&t| (radius * swell(t), cap + t * height)).collect();
                mesh.lathe(Part::Paper, base, &paper, sides, [false, false], true);
                // Caps over the paper's ends, a ledge round each.
                let end = paper[0].0;
                let rim = end * 1.12 + 0.01;
                mesh.lathe(
                    Part::Frame,
                    base,
                    &[(0.8 * rim, 0.0), (rim, 0.3 * cap), (rim, cap), (end, cap)],
                    sides,
                    [true, false],
                    false,
                );
                let top = cap + height;
                mesh.lathe(
                    Part::Frame,
                    base,
                    &[(end, top), (rim, top), (rim, top + 0.7 * cap), (0.8 * rim, total)],
                    sides,
                    [false, true],
                    false,
                );
                if matches!(self.mount, Mount::Hang(_)) {
                    // A tassel under it.
                    let tassel = [
                        (0.25 * rim, 0.0),
                        (0.18 * rim, -0.12 * height),
                        (0.3 * rim, -0.32 * height),
                        (0.0, -0.34 * height),
                    ];
                    mesh.lathe(Part::Frame, base, &tassel, 6, [false, false], false);
                }
            }
            LanternKind::Square => {
                let half = radius;
                let post = 0.06 * width.max(0.3);
                // Paper panels, one tile each, inside the corner posts.
                let corners = [
                    Vec2::new(half, half),
                    Vec2::new(-half, half),
                    Vec2::new(-half, -half),
                    Vec2::new(half, -half),
                ];
                for k in 0..4 {
                    let (a, b) = (corners[k], corners[(k + 1) % 4]);
                    let out = Vec3::new(a.x + b.x, 0.0, a.y + b.y).normalize();
                    let p = |c: Vec2, y: f32| base + Vec3::new(c.x, y, c.y);
                    let quad = [p(a, cap), p(b, cap), p(b, cap + height), p(a, cap + height)];
                    let uv =
                        [Vec2::new(0.0, 1.0), Vec2::new(1.0, 1.0), Vec2::new(1.0, 0.0), Vec2::new(0.0, 0.0)];
                    for tri in [[0, 1, 2], [0, 2, 3]] {
                        let mut tri = tri;
                        let ps = tri.map(|i| quad[i]);
                        if (ps[1] - ps[0]).cross(ps[2] - ps[0]).dot(out) < 0.0 {
                            tri.swap(1, 2);
                        }
                        mesh.triangle(
                            Part::Paper,
                            tri.map(|i| quad[i]),
                            [Vec3::ZERO; 3],
                            ShapeUv::Tiles(tri.map(|i| uv[i])),
                            [1.0; 3],
                        );
                    }
                }
                for c in corners {
                    let o = Vec3::new(c.x, 0.0, c.y) * (1.0 + 0.5 * post / half);
                    let h = Vec3::new(post, 0.0, post) * 0.5;
                    mesh.cuboid(
                        Part::Frame,
                        base + o - h + Vec3::Y * cap,
                        base + o + h + Vec3::Y * (cap + height),
                        &[-Vec3::Y, Vec3::Y],
                    );
                }
                let rim = half + post;
                mesh.cuboid(
                    Part::Frame,
                    base + Vec3::new(-rim, 0.0, -rim),
                    base + Vec3::new(rim, cap, rim),
                    &[],
                );
                mesh.cuboid(
                    Part::Frame,
                    base + Vec3::new(-rim, cap + height, -rim),
                    base + Vec3::new(rim, total, rim),
                    &[],
                );
                // A little pyramid roof over it, its corners over the posts.
                let eave = 1.4 * rim;
                let apex = base + Vec3::Y * (total + 0.45 * width);
                let at = |c: Vec2| base + Vec3::new(c.x * eave / half, total, c.y * eave / half);
                for k in 0..4 {
                    let (a, b) = (at(corners[k]), at(corners[(k + 1) % 4]));
                    let out = Vec3::new(a.x + b.x - 2.0 * base.x, 0.0, a.z + b.z - 2.0 * base.z).normalize()
                        + Vec3::Y;
                    mesh.flat(Part::Frame, [a, b, apex], out, [1.0; 3]);
                }
                mesh.quad(Part::Frame, corners.map(at), -Vec3::Y, [0.8; 4]);
            }
        }
        match self.mount {
            Mount::Hang(length) if length > 0.0 => {
                let cord = 0.012 + 0.02 * width;
                mesh.cuboid(
                    Part::Frame,
                    Vec3::new(-cord, -length - 0.01, -cord),
                    Vec3::new(cord, 0.0, cord),
                    &[-Vec3::Y],
                );
            }
            Mount::Post(post) if post > 0.0 => {
                let w = (0.18 * width).max(0.08);
                mesh.cuboid(
                    Part::Timber,
                    Vec3::new(-w, 0.0, -w),
                    Vec3::new(w, post, w),
                    &[-Vec3::Y, Vec3::Y],
                );
                let foot = 1.6 * w;
                mesh.cuboid(
                    Part::Plinth,
                    Vec3::new(-foot, 0.0, -foot),
                    Vec3::new(foot, 0.12, foot),
                    &[-Vec3::Y],
                );
            }
            _ => {}
        }
    }
}
