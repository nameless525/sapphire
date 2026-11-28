//! Picking a zone's objects (M6 T9a): a [`PickIndex`] of their handles on the zone's grid, made from a
//! [`ZoneObjects`] once a revision of the document, never a frame, and the two questions it answers, which object a
//! ray through the pointer picks ([`PickIndex::pick`]) and which a box drawn on the view holds
//! ([`PickIndex::in_box`]). Each answers by objects' places in the [`ZoneObjects`], which name them by keyed paths.
//!
//! A cell is a column of the grid, so the index keeps each handle in the one its X and Z fall in. A pick reaches
//! as far about its ray as a few points of the view do ([`Pick::reach`], as a share of how far along the ray) or a
//! handle's own size ([`HANDLE_RADIUS`]), whichever is more, and finds the handle it misses least by that measure,
//! the nearer of two it misses alike: it walks the ray's cells across the grid, and those within its reach of each,
//! only where they cross the box every handle lies in (grown by a cell), so the cells it walks are bound by the
//! zone's extent however far the eye is or however wide a reach a command asks for ([`MAX_REACH`]).
//! A box holds an object when every handle of it shows inside the box; it looks only in the cells some of which
//! may show there. Both count what they visit ([`Visits`]), which is how their cost is measured at a district's
//! scale (T12 times them).

use std::collections::{HashMap, HashSet};

use sapphire_math::{Mat4, Vec2, Vec3, Vec4};

use crate::gizmo::Ray;
use crate::markers::ZoneObjects;

/// How far about it a handle can be picked however far it is, in metres.
pub const HANDLE_RADIUS: f32 = 0.3;

/// The widest a pick reaches, as a share of how far along its ray: 45 degrees about it. A click reaches a few points
/// of the view, a small share of this; a command that asks for more gets this.
pub const MAX_REACH: f32 = 1.0;

/// How far along a ray a pick looks at most, in metres: four times as far as anything in a zone may lie.
const FAR: f32 = 4.0 * sapphire_zone::grid::MAX_COORDINATE;

/// A cell of the grid, by its X and Z.
type Cell = (i32, i32);

/// What a pick or a box looked at to answer.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Visits {
    /// Cells of the grid: for a pick, every one it walked, whether any handle lies in it or none.
    pub cells: usize,
    /// Handles in them.
    pub handles: usize,
}

/// A ray to pick by, and how far about it it reaches.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Pick {
    pub ray: Ray,
    /// How far from the ray a handle may be, as a share of how far along it the handle is: a few points of the
    /// view's ([`crate::gizmo::ViewCamera::reach`]).
    pub reach: f32,
}

/// One handle: where it is, and whose.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Handle {
    at: Vec3,
    object: u32,
}

/// The handles of a zone's objects on its grid (see the [module docs](self)).
#[derive(Clone, Debug, Default)]
pub struct PickIndex {
    cell_size: f32,
    /// Each cell's handles, by their places in `handles`, and the box they lie in.
    cells: HashMap<Cell, (Vec<u32>, Vec3, Vec3)>,
    handles: Vec<Handle>,
    /// Each object's handles, by their places in `handles`: from `objects[i]` to `objects[i + 1]`.
    objects: Vec<u32>,
    /// The box every handle lies in.
    bounds: Option<(Vec3, Vec3)>,
}

impl PickIndex {
    /// The index of `objects`' handles, and how many it put in.
    #[must_use]
    pub fn build(objects: &ZoneObjects) -> (Self, Visits) {
        let cell_size = objects.cell_size.max(sapphire_zone::grid::MIN_CELL_SIZE);
        let mut index = Self { cell_size, ..Self::default() };
        for (object, zone_object) in objects.objects.iter().enumerate() {
            index.objects.push(index.handles.len() as u32);
            for &at in &zone_object.handles {
                let place = index.handles.len() as u32;
                index.handles.push(Handle { at, object: object as u32 });
                let (handles, min, max) =
                    index.cells.entry(index.cell_of(at)).or_insert_with(|| (Vec::new(), at, at));
                handles.push(place);
                (*min, *max) = (min.min(at), max.max(at));
                index.bounds = Some(index.bounds.map_or((at, at), |(min, max)| (min.min(at), max.max(at))));
            }
        }
        index.objects.push(index.handles.len() as u32);
        let visits = Visits { cells: index.cells.len(), handles: index.handles.len() };
        (index, visits)
    }

    /// How many objects it holds.
    #[must_use]
    pub fn len(&self) -> usize {
        self.objects.len().saturating_sub(1)
    }

    /// Whether it holds none.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    fn cell_of(&self, at: Vec3) -> Cell {
        self.cell_of2(Vec2::new(at.x, at.z))
    }

    /// The cell a point in X and Z falls in.
    fn cell_of2(&self, at: Vec2) -> Cell {
        ((at.x / self.cell_size).floor() as i32, (at.y / self.cell_size).floor() as i32)
    }

    /// The handles of object `object`.
    fn handles_of(&self, object: usize) -> &[Handle] {
        &self.handles[self.objects[object] as usize..self.objects[object + 1] as usize]
    }

    /// The object `pick` finds, if any, and what it visited to find it.
    #[must_use]
    pub fn pick(&self, pick: &Pick) -> (Option<usize>, Visits) {
        let mut visits = Visits::default();
        let Some((min, max)) = self.bounds else { return (None, visits) };
        let Ray { origin, direction } = pick.ray;
        if !origin.is_finite() || !direction.is_finite() || direction.length_squared() < 1e-12 {
            return (None, visits);
        }
        if !pick.reach.is_finite() {
            return (None, visits);
        }
        let direction = direction.normalize();
        let reach = pick.reach.clamp(0.0, MAX_REACH);
        let widest = |along: f32| HANDLE_RADIUS.max(reach * along);
        // No handle lies farther along the ray than the farthest corner of their box.
        let far = (0..8)
            .map(|corner| {
                let pick_of = |bit: usize, low: f32, high: f32| if corner & bit == 0 { low } else { high };
                let at =
                    Vec3::new(pick_of(1, min.x, max.x), pick_of(2, min.y, max.y), pick_of(4, min.z, max.z));
                (at - origin).dot(direction)
            })
            .fold(0.0_f32, f32::max)
            .min(FAR)
            + HANDLE_RADIUS;
        let mut best: Option<(f32, f32, usize)> = None;
        let mut seen = HashSet::new();
        let mut visit = |cell: Cell, visits: &mut Visits| {
            if !seen.insert(cell) {
                return;
            }
            visits.cells += 1;
            let Some((handles, low, high)) = self.cells.get(&cell) else { return };
            // A cell whose handles all lie out of reach of the ray, which passes over or under them, has none to give.
            let wide = Vec3::splat(widest(far));
            if !meets(origin, direction, *low - wide, *high + wide, far) {
                return;
            }
            for &place in handles {
                visits.handles += 1;
                let handle = self.handles[place as usize];
                let along = (handle.at - origin).dot(direction);
                if along <= 0.0 {
                    continue;
                }
                let off = (origin + direction * along).distance(handle.at);
                let share = off / widest(along);
                if share > 1.0 {
                    continue;
                }
                let better = best.is_none_or(|(least, nearest, object)| {
                    (share, along, handle.object as usize) < (least, nearest, object)
                });
                if better {
                    best = Some((share, along, handle.object as usize));
                }
            }
        };
        // Across the grid in X and Z: the cells a band as wide as the farthest reach crosses, from the eye to as
        // far as anything is, as parallel lines too close together for a cell to fit between two of them, each only
        // where it crosses the box every handle lies in grown by a cell: a cell that holds one lies inside that,
        // and so does where a line of the band crosses it.
        let wide = widest(far);
        let edge = Vec2::splat(self.cell_size);
        let (low, high) = (Vec2::new(min.x, min.z) - edge, Vec2::new(max.x, max.z) + edge);
        let flat = Vec2::new(direction.x, direction.z);
        let across = flat.length();
        let from = Vec2::new(origin.x, origin.z);
        if across < 1e-6 {
            // Straight up or down: its own column and those within reach, within the box.
            let ring = Vec2::splat(wide);
            let (first, last) =
                (self.cell_of2((from - ring).max(low)), self.cell_of2((from + ring).min(high)));
            for x in first.0..=last.0 {
                for z in first.1..=last.1 {
                    visit((x, z), &mut visits);
                }
            }
        } else {
            let along = flat / across;
            let side = along.perp();
            let lines = (2.0 * wide / self.cell_size).ceil().max(1.0) as i32;
            let (start, length) = (from - along * wide, far * across + 2.0 * wide);
            for line in -lines..=lines {
                let offset = side * (wide * line as f32 / lines as f32);
                let Some((enter, leave)) = clip(start + offset, along, length, low, high) else { continue };
                self.walk(start + offset + along * enter, along, leave - enter, |cell| {
                    visit(cell, &mut visits)
                });
            }
        }
        (best.map(|(_, _, object)| object), visits)
    }

    /// Calls `visit` with each cell a line crosses in X and Z, from `start` along `along`, a unit vector, for
    /// `length` metres.
    fn walk(&self, start: Vec2, along: Vec2, length: f32, mut visit: impl FnMut(Cell)) {
        let mut cell = ((start.x / self.cell_size).floor() as i32, (start.y / self.cell_size).floor() as i32);
        let step = |d: f32| if d > 0.0 { 1 } else { -1 };
        let (step_x, step_z) = (step(along.x), step(along.y));
        // How far along it the next line of the grid is on each axis, and how far apart the lines are.
        let next = |at: f32, index: i32, d: f32, stepping: i32| {
            if d.abs() < 1e-9 {
                return f32::INFINITY;
            }
            ((index + i32::from(stepping > 0)) as f32 * self.cell_size - at) / d
        };
        let (mut to_x, mut to_z) =
            (next(start.x, cell.0, along.x, step_x), next(start.y, cell.1, along.y, step_z));
        let (each_x, each_z) = (self.cell_size / along.x.abs(), self.cell_size / along.y.abs());
        loop {
            visit(cell);
            if to_x.min(to_z) >= length {
                return;
            }
            if to_x < to_z {
                cell.0 += step_x;
                to_x += each_x;
            } else {
                cell.1 += step_z;
                to_z += each_z;
            }
        }
    }
    /// The objects every handle of which shows inside the box from `from` to `to` (corners in normalized device
    /// coordinates, either way round) of a view by `view_projection`, in their order, and what it visited.
    #[must_use]
    pub fn in_box(&self, view_projection: Mat4, from: Vec2, to: Vec2) -> (Vec<usize>, Visits) {
        let mut visits = Visits::default();
        let (low, high) = (from.min(to), from.max(to));
        let shown = |at: Vec3| -> Option<Vec2> {
            let clip = view_projection * at.extend(1.0);
            (clip.w > 1e-6).then(|| clip.truncate().truncate() / clip.w)
        };
        let inside = |at: Vec3| shown(at).is_some_and(|ndc| ndc.cmpge(low).all() && ndc.cmple(high).all());
        let mut found = HashSet::new();
        for (handles, min, max) in self.cells.values() {
            if !box_may_show(view_projection, *min, *max, low, high) {
                continue;
            }
            visits.cells += 1;
            for &place in handles {
                visits.handles += 1;
                let handle = self.handles[place as usize];
                if inside(handle.at) {
                    found.insert(handle.object as usize);
                }
            }
        }
        let mut objects: Vec<usize> = found
            .into_iter()
            .filter(|&object| {
                let handles = self.handles_of(object);
                visits.handles += handles.len();
                handles.iter().all(|handle| inside(handle.at))
            })
            .collect();
        objects.sort_unstable();
        (objects, visits)
    }
}

/// Where the line from `start` along `along`, a unit vector, for `length` metres, is inside the rectangle from `low`
/// to `high`: how far along it enters and leaves, or `None` if it misses.
fn clip(start: Vec2, along: Vec2, length: f32, low: Vec2, high: Vec2) -> Option<(f32, f32)> {
    let (mut enter, mut leave) = (0.0_f32, length);
    for axis in 0..2 {
        let (from, d) = (start[axis], along[axis]);
        if d.abs() < 1e-9 {
            if from < low[axis] || from > high[axis] {
                return None;
            }
            continue;
        }
        let (a, b) = ((low[axis] - from) / d, (high[axis] - from) / d);
        (enter, leave) = (enter.max(a.min(b)), leave.min(a.max(b)));
    }
    (enter <= leave).then_some((enter, leave))
}

/// Whether the ray from `origin` along `direction`, from its start to `far` along it, passes through the box from
/// `min` to `max`.
fn meets(origin: Vec3, direction: Vec3, min: Vec3, max: Vec3, far: f32) -> bool {
    let (mut near, mut out) = (0.0_f32, far);
    for axis in 0..3 {
        let (from, d) = (origin[axis], direction[axis]);
        if d.abs() < 1e-9 {
            if from < min[axis] || from > max[axis] {
                return false;
            }
            continue;
        }
        let (a, b) = ((min[axis] - from) / d, (max[axis] - from) / d);
        (near, out) = (near.max(a.min(b)), out.min(a.max(b)));
        if near > out {
            return false;
        }
    }
    true
}

/// Whether any of the box from `min` to `max` may show in the part of the view from `low` to `high`: its corners'
/// span on the screen meets it, or a corner is behind the eye, which a span can't say.
fn box_may_show(view_projection: Mat4, min: Vec3, max: Vec3, low: Vec2, high: Vec2) -> bool {
    let (mut least, mut most) = (Vec2::splat(f32::INFINITY), Vec2::splat(f32::NEG_INFINITY));
    for corner in 0..8 {
        let pick = |bit: usize, a: f32, b: f32| if corner & bit == 0 { a } else { b };
        let at = Vec4::new(pick(1, min.x, max.x), pick(2, min.y, max.y), pick(4, min.z, max.z), 1.0);
        let clip = view_projection * at;
        if clip.w <= 1e-6 {
            return true;
        }
        let ndc = clip.truncate().truncate() / clip.w;
        (least, most) = (least.min(ndc), most.max(ndc));
    }
    least.cmple(high).all() && most.cmpge(low).all()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::markers::{ObjectKind, ZoneObject};

    fn point(kind: ObjectKind, at: Vec3) -> ZoneObject {
        ZoneObject {
            kind,
            path: crate::markers::keyed(kind, "x"),
            key: "x".to_owned(),
            index: 0,
            handles: vec![at],
            shapes: Vec::new(),
        }
    }

    /// A field of lamps a metre apart, in 16 m cells, `side` a side.
    fn field(side: usize) -> ZoneObjects {
        let objects = (0..side * side)
            .map(|at| point(ObjectKind::Light, Vec3::new((at % side) as f32, 0.0, (at / side) as f32)))
            .collect();
        ZoneObjects { objects, cell_size: 16.0 }
    }

    fn ray_at(target: Vec3, from: Vec3) -> Pick {
        Pick { ray: Ray { origin: from, direction: (target - from).normalize() }, reach: 0.01 }
    }

    #[test]
    fn a_ray_picks_what_it_passes_through_and_the_nearer_of_two() {
        let objects = field(40);
        let (index, built) = PickIndex::build(&objects);
        assert_eq!(built, Visits { cells: 9, handles: 1600 });
        let eye = Vec3::new(-10.0, 12.0, -10.0);
        for target in [0, 41, 1599, 820] {
            let at = objects.objects[target].at().expect("a place");
            assert_eq!(index.pick(&ray_at(at, eye)).0, Some(target), "{at}");
        }
        // Straight down onto one, and across to nothing.
        let down =
            Pick { ray: Ray { origin: Vec3::new(5.0, 30.0, 7.0), direction: Vec3::NEG_Y }, reach: 0.01 };
        assert_eq!(index.pick(&down).0, Some(7 * 40 + 5));
        let away = Pick { ray: Ray { origin: eye, direction: Vec3::new(-1.0, 0.0, 0.0) }, reach: 0.01 };
        assert_eq!(index.pick(&away).0, None);
        // Two in line: the nearer.
        let both = ZoneObjects {
            objects: vec![
                point(ObjectKind::Light, Vec3::new(0.0, 0.0, 9.0)),
                point(ObjectKind::Spawner, Vec3::new(0.0, 0.0, 3.0)),
            ],
            cell_size: 4.0,
        };
        let (index, _) = PickIndex::build(&both);
        let along = Pick { ray: Ray { origin: Vec3::ZERO, direction: Vec3::Z }, reach: 0.01 };
        assert_eq!(index.pick(&along).0, Some(1));
    }

    #[test]
    fn a_pick_reaches_further_the_further_it_looks() {
        let far = ZoneObjects {
            objects: vec![point(ObjectKind::Light, Vec3::new(1.0, 0.0, 100.0))],
            cell_size: 16.0,
        };
        let (index, _) = PickIndex::build(&far);
        let ray = Ray { origin: Vec3::ZERO, direction: Vec3::Z };
        assert_eq!(index.pick(&Pick { ray, reach: 0.02 }).0, Some(0), "a metre off at 100 m is within 2%");
        assert_eq!(index.pick(&Pick { ray, reach: 0.005 }).0, None, "and not within half of one");
    }

    /// A reach a command asks for that isn't a number picks nothing, one wider than [`MAX_REACH`] reaches that,
    /// and a pick from far outside the zone walks only the cells about what it holds, each counted.
    #[test]
    fn a_pick_walks_only_the_cells_about_the_zone_however_wide_or_far() {
        let objects = field(40);
        let (index, _) = PickIndex::build(&objects);
        let target = objects.objects[820].at().expect("a place");
        let eye = Vec3::new(-10.0, 12.0, -10.0);
        for reach in [f32::INFINITY, f32::NAN, f32::NEG_INFINITY] {
            let pick = Pick { reach, ..ray_at(target, eye) };
            assert_eq!(index.pick(&pick), (None, Visits::default()), "{reach}");
        }
        let (picked, wide) = index.pick(&Pick { reach: 1e30, ..ray_at(target, eye) });
        assert!(picked.is_some());
        let (_, widest) = index.pick(&Pick { reach: MAX_REACH, ..ray_at(target, eye) });
        assert_eq!(wide, widest, "as wide as the widest");
        assert!(wide.cells <= 25, "the field's 3 by 3 cells and those about them at most: {wide:?}");

        let away = Vec3::new(-10_000.0, 50.0, -10_000.0);
        let (picked, visits) = index.pick(&ray_at(target, away));
        assert_eq!(picked, Some(820), "picked from 14 km");
        assert!(visits.cells <= 25, "the field's cells and those about them, not 14 km of them: {visits:?}");
        let down =
            Pick { ray: Ray { origin: Vec3::new(5.0, 9_000.0, 7.0), direction: Vec3::NEG_Y }, reach: 0.5 };
        let (picked, visits) = index.pick(&down);
        assert_eq!(picked, Some(7 * 40 + 5));
        assert!(visits.cells <= 25, "straight down, only the field's cells and those about them: {visits:?}");
    }

    #[test]
    fn a_box_holds_exactly_what_shows_inside_it() {
        let objects = field(10);
        let (index, _) = PickIndex::build(&objects);
        let above = Vec3::new(4.5, 20.0, 4.5);
        let camera =
            sapphire_render::CameraData::look_at(above, Vec3::new(4.5, 0.0, 4.5), Vec3::Z, 1.0, 1.0, 0.1);
        let view_projection = crate::gizmo::ViewCamera::of(&camera).view_projection();
        let ndc = |at: Vec3| {
            let clip = view_projection * at.extend(1.0);
            clip.truncate().truncate() / clip.w
        };
        // Around the lamps from (2, 3) to (5, 6), and no further.
        let (from, to) = (ndc(Vec3::new(1.5, 0.0, 2.5)), ndc(Vec3::new(5.5, 0.0, 6.5)));
        let (held, _) = index.in_box(view_projection, from, to);
        let expected: Vec<usize> = (3..=6).flat_map(|z| (2..=5).map(move |x| z * 10 + x)).collect();
        assert_eq!(held, expected);
        // A spline half in and half out isn't held.
        let mut objects = field(10);
        objects.objects.push(ZoneObject {
            handles: vec![Vec3::new(3.0, 0.0, 4.2), Vec3::new(9.0, 0.0, 4.2)],
            ..point(ObjectKind::Spline, Vec3::ZERO)
        });
        let (index, _) = PickIndex::build(&objects);
        assert!(!index.in_box(view_projection, from, to).0.contains(&100));
    }

    /// What making the index, a pick and a box visit, at the `budget` district's scale: its 6,943 props and 518
    /// lights over its 8 × 8 cells of 32 m (`bench/README.md`), here as placements on a grid 3 m apart and lamps
    /// 11 m apart over them, seen as the editor's view sees a zone, from above and aside. Counted, not timed: T12
    /// times them.
    #[test]
    #[allow(clippy::print_stdout, reason = "with --nocapture, the counts for the report")]
    fn a_pick_and_a_box_at_the_districts_scale_visit_a_share_of_it() {
        let mut objects: Vec<ZoneObject> = (0..6943)
            .map(|at| {
                point(ObjectKind::Placement, Vec3::new((at % 84) as f32 * 3.05, 0.0, (at / 84) as f32 * 3.05))
            })
            .collect();
        objects.extend((0..518).map(|at| {
            point(ObjectKind::Light, Vec3::new((at % 23) as f32 * 11.1, 4.0, (at / 23) as f32 * 11.1))
        }));
        let objects = ZoneObjects { objects, cell_size: 32.0 };
        let (index, built) = PickIndex::build(&objects);
        assert_eq!(built, Visits { cells: 64, handles: 7461 });
        let camera = sapphire_render::CameraData::look_at(
            Vec3::new(128.0, 160.0, -120.0),
            Vec3::new(128.0, 0.0, 128.0),
            Vec3::Y,
            0.8,
            16.0 / 9.0,
            0.1,
        );
        let view = crate::gizmo::ViewCamera::of(&camera);
        let rect = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1280.0, 720.0));
        let mut most = Visits::default();
        for target in [0, 3000, 6942, 7000, 7460] {
            let at = view.to_screen(objects.objects[target].at().expect("a place"), rect).expect("in view");
            let pick = Pick { ray: view.ray(at, rect), reach: view.reach(at, rect, 16.0) };
            let (picked, visits) = index.pick(&pick);
            assert_eq!(picked, Some(target));
            most = Visits { cells: most.cells.max(visits.cells), handles: most.handles.max(visits.handles) };
        }
        let middle = rect.center();
        let (held, boxed) = index.in_box(
            view.view_projection(),
            crate::gizmo::ViewCamera::ndc(middle - egui::vec2(60.0, 40.0), rect),
            crate::gizmo::ViewCamera::ndc(middle + egui::vec2(60.0, 40.0), rect),
        );
        println!(
            "{} objects: built {built:?}; a pick at most {most:?}; a box of 120 × 80 points {boxed:?}, held {}",
            7461,
            held.len()
        );
        assert!(most.handles < 7461 / 4, "a pick visits a share of the handles: {most:?}");
        assert!(boxed.handles < 7461 / 4, "and so does a box: {boxed:?}");
        assert!(!held.is_empty());
    }
}
