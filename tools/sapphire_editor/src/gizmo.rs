//! The gizmo (M3 E5, its modes M6 T9b): the handles of what's selected, drawn by the stage's own debug draws, so
//! the viewport shows it through the renderer like anything else, and picked here, by plain math on the camera
//! the last frame drew with. What each handle changes is [`crate::handles`]'. None of it needs a GPU, so its tests
//! run anywhere, and it's the engine's own: arrows, bars, a ring and balls, each a line or two of debug draw.
//!
//! A drag along a line (an arrow, a bar, a ball on a face) goes by the point on the line nearest the pointer's ray,
//! less where the press found it, so the handle stays under the pointer rather than jumping to it; one about a
//! ring or over a level by where the ray meets its plane. Where the ray runs along the line, or in the plane,
//! there's no such point, and the value stays where it was.

use sapphire_core::debug_draw::{self as dd, Color, Shape, Style};
use sapphire_math::{Mat4, Quat, Vec3, Vec4};
use sapphire_render::CameraData;
use serde::{Deserialize, Serialize};

use crate::handles::{Axes, GizmoMode, Grip, Handle};

/// An axis of the world, or of an object's own.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Axis {
    X,
    Y,
    Z,
}

impl Axis {
    pub const ALL: [Self; 3] = [Self::X, Self::Y, Self::Z];

    #[must_use]
    pub const fn direction(self) -> Vec3 {
        match self {
            Self::X => Vec3::X,
            Self::Y => Vec3::Y,
            Self::Z => Vec3::Z,
        }
    }

    /// Its index in a position.
    #[must_use]
    pub const fn index(self) -> usize {
        match self {
            Self::X => 0,
            Self::Y => 1,
            Self::Z => 2,
        }
    }

    const fn color(self) -> Color {
        match self {
            Self::X => Color::RED,
            Self::Y => Color::GREEN,
            Self::Z => Color::BLUE,
        }
    }
}

/// A line from `origin` along `direction`, a unit vector.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Ray {
    pub origin: Vec3,
    pub direction: Vec3,
}

/// The camera a frame drew with, and where in the pane it drew: what turns a point of the pane into a ray of the
/// world, and back.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ViewCamera {
    view_projection: Mat4,
    eye: Vec3,
}

impl ViewCamera {
    #[must_use]
    pub fn of(camera: &CameraData) -> Self {
        Self { view_projection: camera.proj * camera.view, eye: camera.position }
    }

    #[must_use]
    pub const fn eye(&self) -> Vec3 {
        self.eye
    }

    /// The ray through `at`, in a frame shown in `rect`.
    #[must_use]
    pub fn ray(&self, at: egui::Pos2, rect: egui::Rect) -> Ray {
        let x = (at.x - rect.min.x) / rect.width() * 2.0 - 1.0;
        let y = 1.0 - (at.y - rect.min.y) / rect.height() * 2.0;
        // Any depth between the planes does: the projection is reverse-Z with no far plane.
        let point = self.view_projection.inverse() * Vec4::new(x, y, 0.5, 1.0);
        let direction = (point.truncate() / point.w - self.eye).normalize_or(Vec3::NEG_Z);
        Ray { origin: self.eye, direction }
    }

    /// What it projects by: the view, then the projection.
    #[must_use]
    pub const fn view_projection(&self) -> Mat4 {
        self.view_projection
    }

    /// Where `at`, in a frame shown in `rect`, is in normalized device coordinates.
    #[must_use]
    pub fn ndc(at: egui::Pos2, rect: egui::Rect) -> sapphire_math::Vec2 {
        sapphire_math::Vec2::new(
            (at.x - rect.min.x) / rect.width() * 2.0 - 1.0,
            1.0 - (at.y - rect.min.y) / rect.height() * 2.0,
        )
    }

    /// How far `points` of the view about `at` reach from the ray through it, as a share of how far along the ray:
    /// what a pick reaches ([`crate::pick::Pick::reach`]).
    #[must_use]
    pub fn reach(&self, at: egui::Pos2, rect: egui::Rect, points: f32) -> f32 {
        let (middle, aside) =
            (self.ray(at, rect).direction, self.ray(at + egui::vec2(points, 0.0), rect).direction);
        let cos = middle.dot(aside).clamp(-1.0, 1.0);
        (1.0 - cos * cos).max(0.0).sqrt() / cos.max(1e-3)
    }

    /// Where `point` shows in a frame shown in `rect`, if it's in front of the eye.
    #[must_use]
    pub fn to_screen(&self, point: Vec3, rect: egui::Rect) -> Option<egui::Pos2> {
        let clip = self.view_projection * point.extend(1.0);
        (clip.w > 1e-6).then(|| {
            let ndc = clip.truncate() / clip.w;
            egui::pos2(
                rect.min.x + (ndc.x + 1.0) * 0.5 * rect.width(),
                rect.min.y + (1.0 - ndc.y) * 0.5 * rect.height(),
            )
        })
    }
}

/// Where a handle is drawn and picked, from the gizmo's origin.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Spot {
    /// An arrow to here.
    Arrow(Vec3),
    /// A bar to here, with a box at its end turned as `Quat`.
    Bar(Vec3, Quat),
    /// A ring about +Y this wide.
    Ring(f32),
    /// A ball here.
    Ball(Vec3),
}

/// The gizmo on what's selected: its handles, in the mode it's in.
#[derive(Clone, Debug, PartialEq)]
pub struct Gizmo {
    pub origin: Vec3,
    /// How long each arrow is, in metres.
    pub length: f32,
    /// The handle under the pointer or being dragged, drawn brighter.
    pub hot: Option<Handle>,
    pub handles: Vec<(Handle, Spot)>,
}

impl Gizmo {
    /// How long the arrows are against the eye's distance: about the same share of the frame however near.
    const SHARE_OF_DISTANCE: f32 = 0.15;

    /// How near a handle the pointer must be to take it, in points.
    pub const REACH: f32 = 8.0;

    /// How wide a ring is, against the arrows' length.
    const RING: f32 = 0.8;

    /// How large a ball or a bar's box is, against the arrows' length.
    const BALL: f32 = 0.05;

    /// How long the arrows are at `origin` as `camera` sees it.
    #[must_use]
    pub fn length_at(origin: Vec3, camera: &ViewCamera) -> f32 {
        origin.distance(camera.eye()).max(0.5) * Self::SHARE_OF_DISTANCE
    }

    /// Three arrows along the zone's axes at `origin` as `camera` sees it: the gizmo of anything with a position,
    /// in [`GizmoMode::Move`] with [`Axes::World`].
    #[must_use]
    pub fn at(origin: Vec3, camera: &ViewCamera) -> Self {
        let length = Self::length_at(origin, camera);
        let handles = Axis::ALL
            .into_iter()
            .map(|axis| (Handle::Move(axis), Spot::Arrow(origin + axis.direction() * length)))
            .collect();
        Self { origin, length, hot: None, handles }
    }

    /// The gizmo on `grip` in `mode`, with its arrows along `axes`, as `camera` sees it.
    #[must_use]
    pub fn of(grip: &Grip, mode: GizmoMode, axes: Axes, camera: &ViewCamera) -> Self {
        let (origin, length) = (grip.origin, Self::length_at(grip.origin, camera));
        let handles = grip
            .handles(mode)
            .into_iter()
            .filter_map(|handle| {
                let spot = match handle {
                    Handle::Move(_) => Spot::Arrow(origin + grip.direction(handle, axes)? * length),
                    Handle::Scale(_) => {
                        Spot::Bar(origin + grip.direction(handle, axes)? * length, grip.frame)
                    }
                    Handle::Yaw => Spot::Ring(length * Self::RING),
                    _ => Spot::Ball(grip.ball(handle, length)?),
                };
                spot_is_finite(spot).then_some((handle, spot))
            })
            .collect();
        Self { origin, length, hot: None, handles }
    }

    /// The handle nearest `at` and within [`Self::REACH`] of it, in a frame shown in `rect`.
    #[must_use]
    pub fn pick(&self, camera: &ViewCamera, rect: egui::Rect, at: egui::Pos2) -> Option<Handle> {
        let from = camera.to_screen(self.origin, rect)?;
        let screen = |point: Vec3| camera.to_screen(point, rect);
        self.handles
            .iter()
            .filter_map(|&(handle, spot)| {
                let distance = match spot {
                    Spot::Arrow(to) | Spot::Bar(to, _) => distance_to_segment(at, from, screen(to)?),
                    Spot::Ball(center) => at.distance(screen(center)?),
                    Spot::Ring(radius) => {
                        let around = |step: usize| {
                            let angle = step as f32 / RING_SEGMENTS as f32 * std::f32::consts::TAU;
                            screen(self.origin + Vec3::new(angle.cos(), 0.0, -angle.sin()) * radius)
                        };
                        (0..RING_SEGMENTS)
                            .filter_map(|step| {
                                Some(distance_to_segment(at, around(step)?, around(step + 1)?))
                            })
                            .min_by(f32::total_cmp)?
                    }
                };
                Some((distance, handle))
            })
            .filter(|(distance, _)| *distance <= Self::REACH)
            .min_by(|a, b| a.0.total_cmp(&b.0))
            .map(|(_, handle)| handle)
    }

    /// Draws it, for the frame: over everything, since what it moves may hide it.
    pub fn draw(&self) {
        for &(handle, spot) in &self.handles {
            let color = if self.hot == Some(handle) { Color::YELLOW } else { color_of(handle) };
            let style = Style::new(color).on_top();
            let ball = self.length * Self::BALL;
            match spot {
                Spot::Arrow(to) => dd::arrow(self.origin, to, style),
                Spot::Bar(to, rotation) => {
                    dd::line(self.origin, to, style);
                    dd::shape(
                        Shape::OrientedBox { center: to, half_extents: Vec3::splat(ball), rotation },
                        style,
                    );
                }
                Spot::Ring(radius) => dd::circle(self.origin, Vec3::Y, radius, style),
                Spot::Ball(center) => {
                    if matches!(handle, Handle::Cone | Handle::InnerCone) {
                        dd::line(self.origin, center, style);
                    }
                    dd::sphere(center, ball, style);
                }
            }
        }
    }
}

/// How many segments a ring is picked by.
const RING_SEGMENTS: usize = 32;

fn spot_is_finite(spot: Spot) -> bool {
    match spot {
        Spot::Arrow(to) | Spot::Bar(to, _) | Spot::Ball(to) => to.is_finite(),
        Spot::Ring(radius) => radius.is_finite(),
    }
}

/// A handle's color, when it isn't under the pointer.
const fn color_of(handle: Handle) -> Color {
    match handle {
        Handle::Move(axis) | Handle::Scale(axis) | Handle::Size(axis) => axis.color(),
        Handle::Point(_) => Color::WHITE,
        Handle::Yaw => Color::MAGENTA,
        Handle::Cone => Color::ORANGE,
        Handle::InnerCone => Color::rgb(255, 220, 120),
        Handle::Radius | Handle::Range => Color::CYAN,
    }
}

/// How far along the line through `origin` on `axis` its point nearest `ray` is, or `None` where the two run
/// together.
#[must_use]
pub fn along(origin: Vec3, axis: Axis, ray: Ray) -> Option<f32> {
    along_line(origin, axis.direction(), ray)
}

/// How far along the line through `origin` along `direction`, a unit vector, its point nearest `ray` is, or
/// `None` where the two run together.
#[must_use]
pub fn along_line(origin: Vec3, direction: Vec3, ray: Ray) -> Option<f32> {
    let across = ray.direction.dot(direction);
    let denominator = 1.0 - across * across;
    if denominator < 1e-4 {
        return None;
    }
    let between = origin - ray.origin;
    let t = (across * ray.direction.dot(between) - direction.dot(between)) / denominator;
    t.is_finite().then_some(t)
}

fn distance_to_segment(point: egui::Pos2, from: egui::Pos2, to: egui::Pos2) -> f32 {
    let span = to - from;
    let length = span.length_sq();
    let t = if length > 0.0 { ((point - from).dot(span) / length).clamp(0.0, 1.0) } else { 0.0 };
    point.distance(from + span * t)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn camera() -> ViewCamera {
        ViewCamera::of(&CameraData::look_at(Vec3::new(4.0, 5.0, 10.0), Vec3::ZERO, Vec3::Y, 0.8, 1.5, 0.1))
    }

    fn rect() -> egui::Rect {
        egui::Rect::from_min_size(egui::pos2(100.0, 50.0), egui::vec2(600.0, 400.0))
    }

    /// A point goes to the pane and back along the ray through it.
    #[test]
    fn a_point_shows_where_its_ray_passes() {
        let camera = camera();
        let point = Vec3::new(1.0, 0.5, -2.0);
        let at = camera.to_screen(point, rect()).expect("in front");
        let ray = camera.ray(at, rect());
        let toward = (point - ray.origin).normalize();
        assert!(toward.dot(ray.direction) > 0.999_99, "{toward} against {}", ray.direction);
    }

    /// The pointer on an arrow's middle takes that arrow, and far from all of them takes none.
    #[test]
    fn the_pointer_takes_the_arrow_it_is_on() {
        let camera = camera();
        let gizmo = Gizmo::at(Vec3::new(1.0, 0.0, 1.0), &camera);
        for axis in Axis::ALL {
            let middle = gizmo.origin + axis.direction() * gizmo.length * 0.6;
            let at = camera.to_screen(middle, rect()).expect("in front");
            assert_eq!(gizmo.pick(&camera, rect(), at), Some(Handle::Move(axis)));
        }
        assert_eq!(gizmo.pick(&camera, rect(), rect().left_top()), None);
    }

    /// A ray through a point of the axis finds it there, wherever the pointer started.
    #[test]
    fn a_drag_follows_the_pointer_along_its_axis() {
        let camera = camera();
        let origin = Vec3::new(1.0, 0.0, 1.0);
        for axis in Axis::ALL {
            for distance in [-3.0, 0.0, 0.7, 5.0] {
                let at = camera.to_screen(origin + axis.direction() * distance, rect()).expect("in front");
                let found = along(origin, axis, camera.ray(at, rect())).expect("across the view");
                assert!((found - distance).abs() < 1e-3, "{axis:?} at {distance}: {found}");
            }
        }
        let down = Ray { origin: Vec3::new(1.0, 10.0, 1.0), direction: Vec3::NEG_Y };
        assert_eq!(along(origin, Axis::Y, down), None, "along the axis there's no nearest point");
    }
}
