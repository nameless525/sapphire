//! The preview: one frame of a [`Run`], drawn through egui from a long lens.
//!
//! **Why egui and not the renderer.** Drawing the character through `sapphire_render` needs the sandbox's
//! `Scene` and `Content`, some 5,700 lines that only an app can link today, and a GPU in every test. Driving
//! a sandbox over live link's capture needs a new way to hold a move at a tick, and a frame's round trip on
//! every scrub. What timing a move takes is the body and the volumes, exactly where the simulation has them,
//! at once: so a clay figure (the cooked mesh, posed and skinned here) and the volumes as shapes, through an
//! orthographic view so reach reads as reach. It runs headless like the rest, on any machine. How the frame
//! looks is what the sandbox beside it is for.

use std::sync::Arc;

use egui::epaint::Mesh;
use egui::{Align2, Color32, CornerRadius, FontId, Pos2, Rect, Shape, Stroke, StrokeKind, Vec2};
use sapphire_gameplay::ActorTransform;
use sapphire_math::{Vec3, det};
use sapphire_physics::{Pose, Shape as BodyShape};

use crate::figure::Figure;
use crate::run::{Frame, Run};

/// Where the view looks from.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct View {
    /// About +Y, from looking along +X with the character's forward to the right.
    pub yaw: f32,
    /// Above the horizon.
    pub pitch: f32,
    /// How close, where 1 fits a character on its feet in the frame's height.
    pub zoom: f32,
    /// What's in the middle of the frame.
    pub focus: Vec3,
}

impl Default for View {
    fn default() -> Self {
        Self::side(1.5)
    }
}

impl View {
    /// From the character's right, a little ahead and above, so the blade's hand is near the viewer and the
    /// dummy to the right of the character.
    #[must_use]
    pub fn side(dummy_distance: f32) -> Self {
        Self { yaw: 0.35, pitch: 0.2, zoom: 1.0, focus: Vec3::new(0.0, 1.0, dummy_distance * 0.5) }
    }

    /// From behind the dummy, looking back along the character's forward.
    #[must_use]
    pub fn front(dummy_distance: f32) -> Self {
        Self {
            yaw: std::f32::consts::FRAC_PI_2,
            pitch: 0.1,
            zoom: 1.0,
            focus: Vec3::new(0.0, 1.0, dummy_distance * 0.5),
        }
    }

    /// From above.
    #[must_use]
    pub fn top(dummy_distance: f32) -> Self {
        Self { yaw: 0.0, pitch: 1.5, zoom: 1.0, focus: Vec3::new(0.0, 0.0, dummy_distance * 0.5) }
    }

    /// The view's axes in the world: to the right of the frame, up it, and into it.
    #[must_use]
    pub fn axes(&self) -> (Vec3, Vec3, Vec3) {
        let (sin_yaw, cos_yaw) = self.yaw.sin_cos();
        let right = Vec3::new(sin_yaw, 0.0, cos_yaw);
        let level = Vec3::new(cos_yaw, 0.0, -sin_yaw);
        let (sin_pitch, cos_pitch) = self.pitch.sin_cos();
        let into = level * cos_pitch + Vec3::new(0.0, -sin_pitch, 0.0);
        let up = Vec3::new(0.0, cos_pitch, 0.0) + level * sin_pitch;
        (right, up, into)
    }

    /// Pixels per metre in `rect`.
    #[must_use]
    pub fn scale(&self, rect: Rect) -> f32 {
        rect.height() / SHOWN_HEIGHT * self.zoom
    }

    /// Where a point of the world is in `rect`.
    #[must_use]
    pub fn project(&self, point: Vec3, rect: Rect) -> Pos2 {
        let (right, up, _) = self.axes();
        let from_focus = point - self.focus;
        rect.center() + Vec2::new(from_focus.dot(right), -from_focus.dot(up)) * self.scale(rect)
    }

    /// How far into the frame a point is.
    #[must_use]
    pub fn depth(&self, point: Vec3) -> f32 {
        let (_, _, into) = self.axes();
        (point - self.focus).dot(into)
    }
}

/// A posed mesh's triangles on screen, kept while nothing about them changes.
#[derive(Debug, Default)]
pub struct FigureCache {
    key: Option<CacheKey>,
    mesh: Arc<Mesh>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct CacheKey {
    view: [f32; 6],
    rect: [f32; 4],
    root: [f32; 4],
    tick: u32,
    clip: Option<sapphire_assets::AssetId>,
    generation: u64,
}

/// What to draw over the frame's bodies.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Overlays {
    pub bones: bool,
    pub hurtboxes: bool,
}

impl Default for Overlays {
    fn default() -> Self {
        Self { bones: false, hurtboxes: true }
    }
}

/// A figure with the pose it takes at this frame.
#[derive(Clone, Copy, Debug)]
pub struct PosedFigure<'a> {
    pub figure: &'a Figure,
    pub clip: Option<sapphire_assets::AssetId>,
    /// Goes up when what it draws from changes.
    pub generation: u64,
}

/// How many metres a frame's height shows at a zoom of 1: a tall character with its arm up.
const SHOWN_HEIGHT: f32 = 2.7;

const GRID: Color32 = Color32::from_gray(52);
const GROUND_LINE: Color32 = Color32::from_gray(74);
const CLAY: [f32; 3] = [176.0, 186.0, 204.0];
const DUMMY: Color32 = Color32::from_rgb(112, 118, 128);
const HIT: Color32 = Color32::from_rgb(255, 76, 66);
const HURT: Color32 = Color32::from_rgb(96, 214, 110);
const OFF: Color32 = Color32::from_rgb(150, 150, 160);
const LANDED: Color32 = Color32::from_rgb(255, 214, 96);

/// Draws one frame of `run`.
#[allow(clippy::too_many_arguments)]
pub fn draw(
    painter: &egui::Painter,
    rect: Rect,
    view: &View,
    run: &Run,
    frame: &Frame,
    posed: Option<PosedFigure<'_>>,
    cache: &mut FigureCache,
    overlays: Overlays,
) {
    painter.rect_filled(rect, CornerRadius::ZERO, Color32::from_gray(22));
    ground(painter, rect, view, run);

    // The dummy, what a hit lands on.
    for body in &frame.dummy_bodies {
        filled_shape(
            painter,
            rect,
            view,
            body.shape,
            body.pose,
            DUMMY.gamma_multiply(0.55),
            Stroke::new(1.0, DUMMY),
        );
    }
    facing(painter, rect, view, frame.dummy_root, DUMMY);

    // The character.
    if let Some(posed) = posed {
        let model = posed.figure.model_at(posed.clip, frame.tick);
        if !posed.figure.triangles().is_empty() {
            let mesh = figure_mesh(posed, model, frame, view, rect, cache);
            painter.add(Shape::mesh(mesh));
        }
        if overlays.bones {
            let world = |point: Vec3| frame.root.position + det::rotate_y(point, frame.root.yaw);
            for (from, to) in posed.figure.bones(model) {
                painter.line_segment(
                    [view.project(world(from), rect), view.project(world(to), rect)],
                    Stroke::new(1.2, Color32::from_rgb(255, 196, 120)),
                );
            }
        }
    }
    facing(painter, rect, view, frame.root, Color32::from_rgb(230, 230, 240));

    if overlays.hurtboxes {
        let color = if frame.invulnerable { OFF } else { HURT };
        for body in &frame.bodies {
            outline_shape(painter, rect, view, body.shape, body.pose, Stroke::new(1.0, color));
        }
    }

    // Where the volumes will be, faintly, and where they are, plainly.
    for ghost in frame.ghosts.iter().filter(|ghost| !ghost.live) {
        outline_shape(
            painter,
            rect,
            view,
            ghost.shape,
            ghost.pose,
            Stroke::new(1.0, HIT.gamma_multiply(0.45)),
        );
    }
    trail(painter, rect, view, run);
    for volume in &frame.volumes {
        filled_shape(
            painter,
            rect,
            view,
            volume.shape,
            volume.pose,
            HIT.gamma_multiply(0.42),
            Stroke::new(1.8, HIT),
        );
    }
    for hit in &frame.hits {
        let at = view.project(hit.point, rect);
        painter.circle_filled(at, 7.0, LANDED);
        painter.circle_stroke(at, 11.0, Stroke::new(1.5, LANDED));
    }

    // In words, for what shape can't say.
    let mut lines = Vec::new();
    if frame.frozen {
        lines.push(("hitstop: the clock stands still".to_owned(), LANDED));
    }
    for hit in &frame.hits {
        lines.push((format!("{} lands", hit.name), LANDED));
    }
    if frame.invulnerable {
        lines.push(("invulnerable".to_owned(), OFF));
    }
    if frame.cancel_open {
        lines.push(("a cancel is open".to_owned(), Color32::from_rgb(120, 200, 200)));
    }
    let mut y = rect.top() + 8.0;
    for (line, color) in lines {
        painter.text(
            Pos2::new(rect.left() + 10.0, y),
            Align2::LEFT_TOP,
            line,
            FontId::proportional(13.0),
            color,
        );
        y += 17.0;
    }
    if run.hits().next().is_none() {
        painter.text(
            rect.right_top() + Vec2::new(-10.0, 8.0),
            Align2::RIGHT_TOP,
            format!("no hit lands at {:.2} m", run.setup.dummy_distance),
            FontId::proportional(13.0),
            OFF,
        );
    }
}

fn ground(painter: &egui::Painter, rect: Rect, view: &View, run: &Run) {
    let reach = run.setup.dummy_distance.max(1.0) + 3.0;
    let line = |from: Vec3, to: Vec3, color| {
        painter.line_segment([view.project(from, rect), view.project(to, rect)], Stroke::new(1.0, color));
    };
    let span = reach.ceil() as i32;
    for step in -3..=span {
        let along = step as f32;
        line(
            Vec3::new(-3.0, 0.0, along),
            Vec3::new(3.0, 0.0, along),
            if step == 0 { GROUND_LINE } else { GRID },
        );
    }
    for step in -3..=3 {
        let across = step as f32;
        line(
            Vec3::new(across, 0.0, -3.0),
            Vec3::new(across, 0.0, reach),
            if step == 0 { GROUND_LINE } else { GRID },
        );
    }
}

/// An arrow on the ground the way an actor faces.
fn facing(painter: &egui::Painter, rect: Rect, view: &View, root: ActorTransform, color: Color32) {
    let lift = Vec3::new(0.0, 0.02, 0.0);
    let from = root.position + lift;
    let to = root.point(Vec3::new(0.0, 0.0, 0.45)) + lift;
    painter.arrow(
        view.project(from, rect),
        view.project(to, rect) - view.project(from, rect),
        Stroke::new(1.5, color),
    );
}

/// The path of what the run's volumes sweep, each volume's middle from its first live step to its last.
fn trail(painter: &egui::Painter, rect: Rect, view: &View, run: &Run) {
    let mut by_name: Vec<(&str, Vec<Pos2>)> = Vec::new();
    for frame in &run.frames {
        for volume in &frame.volumes {
            let point = view.project(volume.pose.position, rect);
            match by_name.iter_mut().find(|(name, _)| *name == volume.name) {
                Some((_, points)) => points.push(point),
                None => by_name.push((volume.name.as_str(), vec![point])),
            }
        }
    }
    for (_, points) in by_name {
        painter.add(Shape::line(points, Stroke::new(1.0, HIT.gamma_multiply(0.6))));
    }
}

/// The figure as flat-shaded triangles, furthest first, since egui has no depth test.
fn figure_mesh(
    posed: PosedFigure<'_>,
    model: &[sapphire_math::Affine3A],
    frame: &Frame,
    view: &View,
    rect: Rect,
    cache: &mut FigureCache,
) -> Arc<Mesh> {
    let key = CacheKey {
        view: [view.yaw, view.pitch, view.zoom, view.focus.x, view.focus.y, view.focus.z],
        rect: [rect.min.x, rect.min.y, rect.max.x, rect.max.y],
        root: [frame.root.position.x, frame.root.position.y, frame.root.position.z, frame.root.yaw],
        tick: frame.tick,
        clip: posed.clip,
        generation: posed.generation,
    };
    if cache.key == Some(key) {
        return Arc::clone(&cache.mesh);
    }
    let world: Vec<Vec3> = posed
        .figure
        .skinned(model)
        .into_iter()
        .map(|point| frame.root.position + det::rotate_y(point, frame.root.yaw))
        .collect();
    let (right, up, into) = view.axes();
    let light = (up * 0.7 - into * 0.6 - right * 0.3).normalize();
    let mut order: Vec<(f32, [u32; 3])> = posed
        .figure
        .triangles()
        .iter()
        .map(|tri| {
            let depth = tri.iter().map(|index| view.depth(world[*index as usize])).sum::<f32>();
            (depth, *tri)
        })
        .collect();
    order.sort_by(|a, b| b.0.total_cmp(&a.0));
    let mut mesh = Mesh::default();
    mesh.reserve_triangles(order.len());
    mesh.reserve_vertices(order.len() * 3);
    for (_, [a, b, c]) in order {
        let (pa, pb, pc) = (world[a as usize], world[b as usize], world[c as usize]);
        let normal = (pb - pa).cross(pc - pa).normalize_or_zero();
        let lit = 0.34 + 0.66 * normal.dot(light).abs();
        let shade = |channel: f32| (channel * lit) as u8;
        let ink = Color32::from_rgb(shade(CLAY[0]), shade(CLAY[1]), shade(CLAY[2]));
        let first = mesh.vertices.len() as u32;
        for point in [pa, pb, pc] {
            mesh.colored_vertex(view.project(point, rect), ink);
        }
        mesh.add_triangle(first, first + 1, first + 2);
    }
    let mesh = Arc::new(mesh);
    cache.key = Some(key);
    cache.mesh = Arc::clone(&mesh);
    mesh
}

/// A shape's silhouette, as the view sees it: exact for spheres and capsules, whose outlines an
/// orthographic view keeps round, and a hull for boxes.
fn silhouette(view: &View, rect: Rect, shape: BodyShape, pose: Pose) -> Vec<Pos2> {
    let at = |local: Vec3| view.project(pose.position + pose.rotation * local, rect);
    match shape {
        BodyShape::Sphere { radius } => stadium(at(Vec3::ZERO), at(Vec3::ZERO), radius * view.scale(rect)),
        BodyShape::Capsule { half_height, radius } => {
            stadium(at(Vec3::Y * half_height), at(Vec3::NEG_Y * half_height), radius * view.scale(rect))
        }
        BodyShape::Box { half_extents } => {
            let mut corners = Vec::with_capacity(8);
            for x in [-1.0, 1.0] {
                for y in [-1.0, 1.0] {
                    for z in [-1.0, 1.0] {
                        corners.push(at(half_extents * Vec3::new(x, y, z)));
                    }
                }
            }
            hull(corners)
        }
    }
}

/// The outline of everything within `radius` of the segment `a`..`b`, around it clockwise on screen.
fn stadium(a: Pos2, b: Pos2, radius: f32) -> Vec<Pos2> {
    const STEPS: usize = 14;
    let along = b - a;
    let angle = if along.length_sq() > 1e-6 { along.y.atan2(along.x) } else { 0.0 };
    let half_turn = std::f32::consts::PI;
    let mut points = Vec::with_capacity(2 * STEPS + 2);
    for step in 0..=STEPS {
        let turn = angle - half_turn * 0.5 + half_turn * step as f32 / STEPS as f32;
        points.push(b + Vec2::angled(turn) * radius);
    }
    for step in 0..=STEPS {
        let turn = angle + half_turn * 0.5 + half_turn * step as f32 / STEPS as f32;
        points.push(a + Vec2::angled(turn) * radius);
    }
    points
}

/// The convex hull of a few points, in order round it.
fn hull(mut points: Vec<Pos2>) -> Vec<Pos2> {
    points.sort_by(|a, b| a.x.total_cmp(&b.x).then(a.y.total_cmp(&b.y)));
    let turn = |o: Pos2, a: Pos2, b: Pos2| (a.x - o.x) * (b.y - o.y) - (a.y - o.y) * (b.x - o.x);
    let mut lower: Vec<Pos2> = Vec::new();
    for &point in &points {
        while lower.len() >= 2 && turn(lower[lower.len() - 2], lower[lower.len() - 1], point) <= 0.0 {
            lower.pop();
        }
        lower.push(point);
    }
    let mut upper: Vec<Pos2> = Vec::new();
    for &point in points.iter().rev() {
        while upper.len() >= 2 && turn(upper[upper.len() - 2], upper[upper.len() - 1], point) <= 0.0 {
            upper.pop();
        }
        upper.push(point);
    }
    lower.pop();
    upper.pop();
    lower.extend(upper);
    lower
}

fn filled_shape(
    painter: &egui::Painter,
    rect: Rect,
    view: &View,
    shape: BodyShape,
    pose: Pose,
    fill: Color32,
    stroke: Stroke,
) {
    painter.add(Shape::convex_polygon(silhouette(view, rect, shape, pose), fill, stroke));
}

fn outline_shape(
    painter: &egui::Painter,
    rect: Rect,
    view: &View,
    shape: BodyShape,
    pose: Pose,
    stroke: Stroke,
) {
    painter.add(Shape::closed_line(silhouette(view, rect, shape, pose), stroke));
}

/// A readable frame on an empty stage, for when there's nothing to play.
pub fn empty(painter: &egui::Painter, rect: Rect, why: &str) {
    painter.rect_filled(rect, CornerRadius::ZERO, Color32::from_gray(22));
    painter.rect_stroke(
        rect,
        CornerRadius::ZERO,
        Stroke::new(1.0, Color32::from_gray(50)),
        StrokeKind::Inside,
    );
    painter.text(
        rect.center(),
        Align2::CENTER_CENTER,
        why,
        FontId::proportional(14.0),
        Color32::from_gray(170),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_side_view_has_the_dummy_to_the_right_and_up_up() {
        let view = View::side(1.5);
        let rect = Rect::from_min_size(Pos2::ZERO, Vec2::new(800.0, 600.0));
        let character = view.project(Vec3::ZERO, rect);
        let dummy = view.project(Vec3::new(0.0, 0.0, 1.5), rect);
        let head = view.project(Vec3::new(0.0, 1.6, 0.0), rect);
        assert!(dummy.x > character.x);
        assert!(head.y < character.y);
        // The character's right is nearer the viewer: into the frame is toward its left, +X.
        assert!(view.depth(Vec3::new(1.0, 0.0, 0.0)) > view.depth(Vec3::new(-1.0, 0.0, 0.0)));
    }

    #[test]
    fn a_view_keeps_lengths_along_its_own_axes() {
        let view = View { yaw: 0.7, pitch: 0.4, zoom: 1.0, focus: Vec3::ZERO };
        let (right, up, into) = view.axes();
        assert!((right.length() - 1.0).abs() < 1e-5);
        assert!((up.length() - 1.0).abs() < 1e-5);
        assert!((into.length() - 1.0).abs() < 1e-5);
        assert!(right.dot(up).abs() < 1e-5 && right.dot(into).abs() < 1e-5 && up.dot(into).abs() < 1e-5);
    }

    #[test]
    fn a_stadium_keeps_its_radius_all_round() {
        let (a, b) = (Pos2::new(0.0, 0.0), Pos2::new(50.0, 0.0));
        for point in stadium(a, b, 10.0) {
            let nearest = Pos2::new(point.x.clamp(0.0, 50.0), 0.0);
            assert!(((point - nearest).length() - 10.0).abs() < 1e-3, "{point:?}");
        }
    }

    #[test]
    fn a_hull_leaves_inner_points_out() {
        let hull = hull(vec![
            Pos2::new(0.0, 0.0),
            Pos2::new(10.0, 0.0),
            Pos2::new(10.0, 10.0),
            Pos2::new(0.0, 10.0),
            Pos2::new(5.0, 5.0),
        ]);
        assert_eq!(hull.len(), 4);
        assert!(!hull.contains(&Pos2::new(5.0, 5.0)));
    }
}
