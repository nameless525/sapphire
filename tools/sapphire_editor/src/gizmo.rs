//! The translate gizmo (M3 E5): three arrows on what's selected, drawn by the stage's own debug draws, so the
//! viewport shows it through the renderer like anything else, and picked and dragged here, by plain math on the
//! camera the last frame drew with. None of it needs a GPU, so its tests run anywhere.
//!
//! A drag moves along one axis: the point on the axis nearest the pointer's ray, less where the press found it,
//! so the arrow stays under the pointer rather than jumping to it. Where the ray runs along the axis there's no
//! such point, and the value stays where it was.

use sapphire_core::debug_draw::{self as dd, Color, Style};
use sapphire_math::{Mat4, Vec3, Vec4};
use sapphire_render::CameraData;

/// An axis of the world.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
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

/// The gizmo on what's selected.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Gizmo {
    pub origin: Vec3,
    /// How long each arrow is, in metres.
    pub length: f32,
    /// The arrow under the pointer or being dragged, drawn brighter.
    pub hot: Option<Axis>,
}

impl Gizmo {
    /// How long the arrows are against the eye's distance: about the same share of the frame however near.
    const SHARE_OF_DISTANCE: f32 = 0.15;

    /// How near an arrow the pointer must be to take it, in points.
    pub const REACH: f32 = 8.0;

    /// A gizmo at `origin` as `camera` sees it.
    #[must_use]
    pub fn at(origin: Vec3, camera: &ViewCamera) -> Self {
        Self { origin, length: origin.distance(camera.eye()).max(0.5) * Self::SHARE_OF_DISTANCE, hot: None }
    }

    /// The arrow nearest `at` and within [`Self::REACH`] of it, in a frame shown in `rect`.
    #[must_use]
    pub fn pick(&self, camera: &ViewCamera, rect: egui::Rect, at: egui::Pos2) -> Option<Axis> {
        let from = camera.to_screen(self.origin, rect)?;
        Axis::ALL
            .into_iter()
            .filter_map(|axis| {
                let to = camera.to_screen(self.origin + axis.direction() * self.length, rect)?;
                Some((distance_to_segment(at, from, to), axis))
            })
            .filter(|(distance, _)| *distance <= Self::REACH)
            .min_by(|a, b| a.0.total_cmp(&b.0))
            .map(|(_, axis)| axis)
    }

    /// Draws it, for the frame: over everything, since what it moves may hide it.
    pub fn draw(&self) {
        for axis in Axis::ALL {
            let color = if self.hot == Some(axis) { Color::YELLOW } else { axis.color() };
            dd::arrow(self.origin, self.origin + axis.direction() * self.length, Style::new(color).on_top());
        }
    }
}

/// How far along the line through `origin` on `axis` its point nearest `ray` is, or `None` where the two run
/// together.
#[must_use]
pub fn along(origin: Vec3, axis: Axis, ray: Ray) -> Option<f32> {
    let direction = axis.direction();
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
            assert_eq!(gizmo.pick(&camera, rect(), at), Some(axis));
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
