//! Walls: `#hall`, a building's four walls, and `#wall`, a freestanding wall under a tiled
//! coping, both with openings.
//!
//! Each side is a plaster face standing on a stone plinth that wraps the corners, proud of the
//! face. Openings are cut out of both: holes for windows, notches up from the ground for doors
//! and moon gates (the plinth stops at their jambs). Each opening's reveal runs into the wall: a
//! hall's to a backing set `recess` meters in (paper for windows, a door or a dark doorway),
//! with its lattice part way in; a freestanding wall's right through it, so a moon gate is a way
//! through and a lattice window is seen through. Rectangular openings take a timber frame proud
//! of the face, round ones a stone ring. Timber posts stand proud at the corners and between
//! bays, and timber beams wrap all four sides prouder still, so beams cross in front of posts.

use glam::{Vec2, Vec3};
use serde::{Deserialize, Serialize};

use super::roof::{Ridge, Roof, RoofKind, Tiles};
use super::{Detail, Frame, Mesh, Part, check, finite};

/// Margin kept between openings, and between them and their face's edges, in meters.
const MARGIN: f32 = 0.05;
/// Points round a full circle of an opening.
const ROUND: usize = 32;
/// Lattice bars' width and depth.
const BAR: [f32; 2] = [0.04, 0.035];
/// Vertical lattice bars stand this far in front of horizontal ones, so their fronts never
/// share a plane.
const BAR_STEP: f32 = 0.006;
/// Posts stop this far under the walls' top, so their tops never share a plane with a beam's or
/// a flat roof's.
const POST_TOP: f32 = 0.01;

/// A side of a building, by the way it faces: the front faces +Z.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Side {
    #[default]
    Front,
    Right,
    Back,
    Left,
}

impl Side {
    pub const ALL: [Self; 4] = [Self::Front, Self::Right, Self::Back, Self::Left];

    /// Its outward normal and the direction to the right seen from outside.
    fn axes(self) -> (Vec3, Vec3) {
        match self {
            Self::Front => (Vec3::Z, Vec3::X),
            Self::Right => (Vec3::X, -Vec3::Z),
            Self::Back => (-Vec3::Z, -Vec3::X),
            Self::Left => (-Vec3::X, Vec3::Z),
        }
    }
}

/// What an opening is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum OpeningKind {
    /// A rectangular window in a timber frame, its lattice before paper.
    Window,
    /// A round window in a stone ring.
    Moon,
    /// A rectangular doorway from the ground in a timber frame, a door leaf in it.
    Door,
    /// A round doorway from the ground (a moon gate) in a stone ring: `size` is its circle's
    /// width and the height of its top, the circle cut by the ground when that is less.
    Gate,
}

/// An opening in a side.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Opening {
    /// The side it's in (a freestanding wall's openings go through it from its front).
    #[serde(default)]
    pub side: Side,
    pub kind: OpeningKind,
    /// Its middle along the side (meters right of the side's middle, seen from outside) and its
    /// foot's height (doors and gates stand on the ground: 0).
    #[serde(default)]
    pub at: [f32; 2],
    /// Its width and height; a moon window's are its diameter.
    pub size: [f32; 2],
    /// Lattice bars, upright and across (windows and moon windows).
    #[serde(default)]
    pub lattice: [u32; 2],
}

/// The stone base walls stand on: its height and how far it stands proud of the plaster (no
/// plinth at height 0).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Plinth {
    pub height: f32,
    pub proud: f32,
}

impl Default for Plinth {
    fn default() -> Self {
        Self { height: 0.5, proud: 0.06 }
    }
}

/// Timber posts proud of the walls: at the corners and between `bays` (across the width and
/// the depth), `width` wide.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Posts {
    pub width: f32,
    pub proud: f32,
    pub bays: [u32; 2],
}

impl Default for Posts {
    fn default() -> Self {
        Self { width: 0.24, proud: 0.04, bays: [1, 1] }
    }
}

/// A timber beam round all four sides, its top `at` meters up.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Beam {
    pub at: f32,
    pub height: f32,
    pub proud: f32,
}

impl Default for Beam {
    fn default() -> Self {
        Self { at: 3.0, height: 0.22, proud: 0.07 }
    }
}

/// A building's walls (`#hall`): `size` meters wide (X), deep (Z) and high, standing on its
/// origin, the front facing +Z. A roof placed `size[2]` up sits on them.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Hall {
    pub size: [f32; 3],
    pub plinth: Plinth,
    pub posts: Option<Posts>,
    pub beams: Vec<Beam>,
    pub openings: Vec<Opening>,
    /// How far into the walls windows' paper and doors stand.
    pub recess: f32,
    /// The width of frames and rings round openings, and how proud they stand.
    pub frame: [f32; 2],
    /// A flat roof over it (none where a roof covers it).
    pub flat_top: bool,
}

impl Default for Hall {
    fn default() -> Self {
        Self {
            size: [6.0, 5.0, 3.6],
            plinth: Plinth::default(),
            posts: None,
            beams: Vec::new(),
            openings: Vec::new(),
            recess: 0.16,
            frame: [0.09, 0.03],
            flat_top: false,
        }
    }
}

/// A freestanding wall (`#wall`): `size` meters long (X), high and thick (Z), standing on its
/// origin, under a tiled coping. Its openings go through it, placed from its front.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Wall {
    pub size: [f32; 3],
    pub plinth: Plinth,
    pub coping: Coping,
    pub openings: Vec<Opening>,
    pub frame: [f32; 2],
    /// Whether its left and right ends are built (not where they run into a building).
    pub ends: [bool; 2],
}

impl Default for Wall {
    fn default() -> Self {
        Self {
            size: [6.0, 2.6, 0.32],
            plinth: Plinth { height: 0.4, proud: 0.05 },
            coping: Coping::default(),
            openings: Vec::new(),
            frame: [0.09, 0.03],
            ends: [true, true],
        }
    }
}

/// A freestanding wall's coping: a small gable roof along its top.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Coping {
    pub overhang: f32,
    pub barge: f32,
    pub rise: f32,
    pub thickness: f32,
    pub tiles: Tiles,
    pub ridge: Ridge,
}

impl Default for Coping {
    fn default() -> Self {
        Self {
            overhang: 0.16,
            barge: 0.1,
            rise: 0.22,
            thickness: 0.08,
            tiles: Tiles { period: 0.22, depth: 0.03 },
            ridge: Ridge { height: 0.12, width: 0.14, ends: 0.0 },
        }
    }
}

impl Coping {
    /// The coping as a gable roof over a wall `length` by `thickness`.
    fn roof(&self, length: f32, thickness: f32) -> Roof {
        Roof {
            size: [length, thickness],
            kind: RoofKind::Gable,
            overhang: self.overhang,
            barge: self.barge,
            rise: self.rise,
            curve: 0.2,
            sweep: 0.0,
            sweep_reach: 1.0,
            thickness: self.thickness,
            tiles: self.tiles,
            ridge: self.ridge,
        }
    }
}

/// The geometry a hall and a freestanding wall share.
struct Body<'a> {
    /// Half its width (X) and depth (Z).
    half: Vec2,
    height: f32,
    plinth: Plinth,
    posts: Option<Posts>,
    beams: &'a [Beam],
    frame: [f32; 2],
    /// A hall's recess; `None` for a wall, whose openings go through.
    recess: Option<f32>,
    /// The sides built.
    sides: [bool; 4],
}

/// One side's face: points on it are `origin + right a + up y + normal d`.
#[derive(Clone, Copy)]
struct Face {
    origin: Vec3,
    normal: Vec3,
    right: Vec3,
    /// Half its width along `right`.
    half: f32,
}

impl Face {
    fn of(side: Side, half: Vec2) -> Self {
        let (normal, right) = side.axes();
        let (out, across) = if normal.z != 0.0 { (half.y, half.x) } else { (half.x, half.y) };
        Self { origin: normal * out, normal, right, half: across }
    }

    fn point(&self, a: f32, y: f32, d: f32) -> Vec3 {
        self.origin + self.right * a + Vec3::Y * y + self.normal * d
    }

    /// Its plane, moved `d` out, as a 2D frame (right, up).
    fn frame(&self, d: f32) -> Frame {
        Frame { origin: self.origin + self.normal * d, right: self.right, up: Vec3::Y }
    }
}

/// An opening's outline on its face, in the face's (along, up) meters.
struct Outline {
    /// A window's closed outline; a grounded opening's path from its left foot on the ground up
    /// round to its right foot (its bottom open), through its jambs' points at the plinth's top.
    points: Vec<Vec2>,
    grounded: bool,
    /// A grounded opening's jambs, left and right, where it cuts the plinth.
    jambs: [f32; 2],
    center: Vec2,
}

impl Opening {
    fn radius(&self) -> f32 {
        0.5 * self.size[0]
    }

    fn grounded(&self) -> bool {
        matches!(self.kind, OpeningKind::Door | OpeningKind::Gate)
    }

    /// The gate's circle's center height.
    fn gate_center(&self) -> f32 {
        self.size[1] - self.radius()
    }

    /// Its extent on its face, frame or ring included: (left, right, bottom, top).
    fn extent(&self, frame: f32) -> [f32; 4] {
        let [a, foot] = self.at;
        let [width, height] = self.size;
        [a - 0.5 * width - frame, a + 0.5 * width + frame, foot, foot + height + frame]
    }

    /// Its outline on a face whose plinth is `plinth` high.
    fn outline(&self, plinth: f32) -> Outline {
        let [a, foot] = self.at;
        let [width, height] = self.size;
        let (l, r) = (a - 0.5 * width, a + 0.5 * width);
        match self.kind {
            OpeningKind::Window => Outline {
                points: vec![
                    Vec2::new(l, foot),
                    Vec2::new(r, foot),
                    Vec2::new(r, foot + height),
                    Vec2::new(l, foot + height),
                ],
                grounded: false,
                jambs: [l, r],
                center: Vec2::new(a, foot + 0.5 * height),
            },
            OpeningKind::Moon => {
                let center = Vec2::new(a, foot + self.radius());
                Outline {
                    points: super::circle(center, self.radius(), ROUND),
                    grounded: false,
                    jambs: [l, r],
                    center,
                }
            }
            OpeningKind::Door => {
                let mut points = vec![Vec2::new(l, 0.0)];
                if plinth > 0.0 {
                    points.push(Vec2::new(l, plinth));
                }
                points.extend([Vec2::new(l, height), Vec2::new(r, height)]);
                if plinth > 0.0 {
                    points.push(Vec2::new(r, plinth));
                }
                points.push(Vec2::new(r, 0.0));
                Outline { points, grounded: true, jambs: [l, r], center: Vec2::new(a, 0.5 * height) }
            }
            OpeningKind::Gate => {
                let radius = self.radius();
                let cy = self.gate_center();
                // The arc above the plinth (or the ground), from its left foot over the top.
                let base = plinth.max(cy - radius + 1e-4);
                let (from, to) = arc_span(radius, cy, base);
                let count = ((ROUND as f32) * (from - to) / std::f32::consts::TAU).ceil().max(4.0) as usize;
                let arc: Vec<Vec2> = (0..=count)
                    .map(|k| {
                        let angle = from + (to - from) * k as f32 / count as f32;
                        Vec2::new(a + radius * angle.cos(), cy + radius * angle.sin())
                    })
                    .collect();
                let (jl, jr) = (arc[0].x, arc[count].x);
                let mut points = Vec::new();
                if base > 0.0 {
                    points.push(Vec2::new(jl, 0.0));
                }
                points.extend(arc);
                if base > 0.0 {
                    points.push(Vec2::new(jr, 0.0));
                }
                Outline { points, grounded: true, jambs: [jl, jr], center: Vec2::new(a, cy) }
            }
        }
    }
}

/// The angles (radians from +X, counter-clockwise) of a circle's left and right points at
/// height `base`, its center `cy` high: from the left over the top to the right is from the
/// first down to the second.
fn arc_span(radius: f32, cy: f32, base: f32) -> (f32, f32) {
    let rise = ((base - cy) / radius).clamp(-1.0, 1.0).asin();
    (std::f32::consts::PI - rise, rise)
}

impl Outline {
    /// The part of a grounded opening's path above the plinth: its notch in the plaster.
    fn notch(&self, plinth: f32) -> &[Vec2] {
        let start = self.points.iter().position(|p| p.y >= plinth - 1e-6).unwrap_or(0);
        let end = self.points.iter().rposition(|p| p.y >= plinth - 1e-6).unwrap_or(self.points.len() - 1);
        &self.points[start..=end]
    }
}

impl Hall {
    pub fn validate(&self) -> Result<(), String> {
        let [width, depth, height] = self.size;
        check(finite(&self.size) && width > 0.0 && depth > 0.0 && height > 0.0, || {
            format!("a hall's size {:?} must be positive", self.size)
        })?;
        check(finite(&[self.recess]) && self.recess > 0.0, || "a hall's recess must be positive".to_owned())?;
        validate_body(&self.body(), &self.openings, false)
    }

    fn body(&self) -> Body<'_> {
        Body {
            half: Vec2::new(0.5 * self.size[0], 0.5 * self.size[1]),
            height: self.size[2],
            plinth: self.plinth,
            posts: self.posts,
            beams: &self.beams,
            frame: self.frame,
            recess: Some(self.recess),
            sides: [true; 4],
        }
    }

    pub(super) fn build(&self, mesh: &mut Mesh) -> Result<(), String> {
        let body = self.body();
        body.build(mesh, &self.openings)?;
        if self.flat_top {
            let [hx, hz] = body.half.to_array();
            let y = body.height;
            mesh.quad(
                Part::Plaster,
                [Vec3::new(-hx, y, -hz), Vec3::new(hx, y, -hz), Vec3::new(hx, y, hz), Vec3::new(-hx, y, hz)],
                Vec3::Y,
                [1.0; 4],
            );
        }
        Ok(())
    }
}

impl Wall {
    pub fn validate(&self) -> Result<(), String> {
        let [length, height, thickness] = self.size;
        check(finite(&self.size) && length > 0.0 && height > 0.0 && thickness > 0.0, || {
            format!("a wall's size {:?} must be positive", self.size)
        })?;
        for opening in &self.openings {
            check(opening.side == Side::Front, || {
                "a wall's openings go through it from its front: give them side Front".to_owned()
            })?;
        }
        check(thickness > 2.0 * self.frame[1] + 2.0 * BAR[1], || {
            format!("a wall {thickness} m thick is too thin for its frames and lattices")
        })?;
        self.coping.roof(length, thickness).validate()?;
        validate_body(&self.body(), &self.openings, true)
    }

    fn body(&self) -> Body<'_> {
        let [length, height, thickness] = self.size;
        Body {
            half: Vec2::new(0.5 * length, 0.5 * thickness),
            height,
            plinth: self.plinth,
            posts: None,
            beams: &[],
            frame: self.frame,
            recess: None,
            sides: [true, self.ends[1], true, self.ends[0]],
        }
    }

    pub(super) fn build(&self, mesh: &mut Mesh, detail: Detail) -> Result<(), String> {
        let [length, height, thickness] = self.size;
        // Each opening cuts the back too, mirrored.
        let mut openings = self.openings.clone();
        for opening in &self.openings {
            openings.push(Opening {
                side: Side::Back,
                at: [-opening.at[0], opening.at[1]],
                ..opening.clone()
            });
        }
        self.body().build(mesh, &openings)?;
        self.coping.roof(length, thickness).build(mesh, Vec3::Y * height, detail)
    }
}

/// Checks a body's parts and openings fit: plinth, posts and beams in order of how proud they
/// stand, and openings inside their faces, clear of the plinth and of each other.
fn validate_body(body: &Body<'_>, openings: &[Opening], through: bool) -> Result<(), String> {
    let Plinth { height: plinth, proud } = body.plinth;
    check(finite(&[plinth, proud]) && plinth >= 0.0 && proud >= 0.0 && plinth < body.height, || {
        format!("the plinth ({plinth} m high, {proud} m proud) must stand under the walls' top")
    })?;
    let [frame, frame_proud] = body.frame;
    check(finite(&body.frame) && frame > 0.0 && frame_proud > 0.0, || {
        "frames must have a width and stand proud".to_owned()
    })?;
    if let Some(posts) = body.posts {
        check(finite(&[posts.width, posts.proud]) && posts.width > 0.0 && posts.proud > 0.0, || {
            "posts must have a width and stand proud".to_owned()
        })?;
        check(posts.bays[0] >= 1 && posts.bays[1] >= 1, || "posts need at least one bay a side".to_owned())?;
        check(posts.width < body.half.min_element(), || "posts must be narrower than the walls".to_owned())?;
        check(posts.proud != frame_proud, || {
            "posts and frames must stand proud by different amounts, or their faces would share a plane"
                .to_owned()
        })?;
    }
    for beam in body.beams {
        check(finite(&[beam.at, beam.height, beam.proud]) && beam.height > 0.0 && beam.proud > 0.0, || {
            format!("beam {beam:?} needs a height and to stand proud")
        })?;
        check(beam.at <= body.height && beam.at - beam.height >= plinth, || {
            format!("beam {beam:?} must lie between the plinth and the walls' top")
        })?;
        if let Some(posts) = body.posts {
            check(beam.proud > posts.proud, || {
                "beams must stand prouder than posts, to cross in front of them".to_owned()
            })?;
        }
    }
    for (index, opening) in openings.iter().enumerate() {
        let what = || format!("opening {index} ({:?} on the {:?})", opening.kind, opening.side);
        check(
            finite(&opening.at) && finite(&opening.size) && opening.size[0] > 0.0 && opening.size[1] > 0.0,
            || format!("{} needs a finite place and a positive size", what()),
        )?;
        if opening.kind == OpeningKind::Moon {
            check((opening.size[0] - opening.size[1]).abs() < 1e-4, || {
                format!("{} is round: its width and height must agree", what())
            })?;
        }
        if opening.kind == OpeningKind::Gate {
            check(
                opening.size[1] >= opening.radius() + plinth + MARGIN && opening.size[1] <= opening.size[0],
                || {
                    format!(
                        "{}'s circle must reach the ground, its middle above the plinth: its height between {} and {} m",
                        what(),
                        opening.radius() + plinth + MARGIN,
                        opening.size[0]
                    )
                },
            )?;
        }
        if through {
            check(opening.lattice == [0, 0] || !opening.grounded(), || {
                format!("{} can't take a lattice", what())
            })?;
        }
        let face = Face::of(opening.side, body.half);
        let [left, right, bottom, top] = opening.extent(frame);
        check(left >= -face.half + MARGIN && right <= face.half - MARGIN, || {
            format!("{} reaches past its side's ends ({} m either side of its middle)", what(), face.half)
        })?;
        check(top <= body.height - MARGIN, || format!("{} reaches past the walls' top", what()))?;
        if let Some(posts) = body.posts {
            // Clear of the bays' posts and the corner posts' sides.
            let bays = if face.normal.z != 0.0 { posts.bays[0] } else { posts.bays[1] };
            let corner = face.half + posts.proud - posts.width;
            let clear = (1..bays).all(|k| {
                let a = -face.half + 2.0 * face.half * k as f32 / bays as f32;
                right <= a - 0.5 * posts.width - MARGIN || left >= a + 0.5 * posts.width + MARGIN
            }) && left >= -corner + MARGIN
                && right <= corner - MARGIN;
            check(clear, || format!("{} crosses a post", what()))?;
        }
        if opening.grounded() {
            check(opening.at[1] == 0.0, || {
                format!("{} stands on the ground: its foot must be at 0", what())
            })?;
        } else {
            check(bottom - frame >= plinth + MARGIN, || format!("{} must stand above the plinth", what()))?;
        }
        for (other_index, other) in openings.iter().enumerate().take(index) {
            if other.side != opening.side {
                continue;
            }
            let [ol, or, ob, ot] = other.extent(frame);
            let below = if other.grounded() { 0.0 } else { ob - frame };
            let own_below = if opening.grounded() { 0.0 } else { bottom - frame };
            let apart = left >= or + MARGIN
                || right <= ol - MARGIN
                || own_below >= ot + MARGIN
                || top <= below - MARGIN;
            check(apart, || format!("{} overlaps opening {other_index}", what()))?;
        }
    }
    Ok(())
}

impl Body<'_> {
    fn build(&self, mesh: &mut Mesh, openings: &[Opening]) -> Result<(), String> {
        for (index, side) in Side::ALL.into_iter().enumerate() {
            if !self.sides[index] {
                continue;
            }
            let face = Face::of(side, self.half);
            let own: Vec<&Opening> = openings.iter().filter(|opening| opening.side == side).collect();
            self.side(mesh, &face, &own)?;
            for opening in &own {
                // A wall's openings are cut through both faces; the front's builds the reveal
                // and lattice once.
                let through_once = self.recess.is_some() || side != Side::Back;
                self.opening(mesh, &face, opening, through_once)?;
            }
        }
        self.posts(mesh);
        self.beams(mesh);
        Ok(())
    }

    /// A side's plaster and plinth, cut by its openings.
    fn side(&self, mesh: &mut Mesh, face: &Face, openings: &[&Opening]) -> Result<(), String> {
        let plinth = self.plinth.height;
        let half = face.half;
        let mut outlines: Vec<Outline> = openings.iter().map(|opening| opening.outline(plinth)).collect();
        outlines.sort_by(|a, b| a.center.x.total_cmp(&b.center.x));
        // The plaster above the plinth, notched by grounded openings, holed by the others.
        let mut outer = vec![Vec2::new(-half, plinth)];
        let mut holes = Vec::new();
        for outline in &outlines {
            if outline.grounded {
                outer.extend_from_slice(outline.notch(plinth));
            } else {
                holes.push(outline.points.clone());
            }
        }
        outer.extend([Vec2::new(half, plinth), Vec2::new(half, self.height), Vec2::new(-half, self.height)]);
        outer.dedup_by(|a, b| a.distance_squared(*b) < 1e-10);
        mesh.fill(Part::Plaster, &face.frame(0.0), &outer, &holes, &|_| 1.0)?;

        if plinth <= 0.0 {
            return Ok(());
        }
        let proud = self.plinth.proud;
        // Spans of plinth between grounded openings' jambs.
        let mut spans = Vec::new();
        let mut from = None;
        for outline in outlines.iter().filter(|outline| outline.grounded) {
            spans.push((from, Some(outline.jambs[0])));
            from = Some(outline.jambs[1]);
        }
        spans.push((from, None));
        for (start, end) in spans {
            // The front, mitered past the corners where the plinth turns them.
            let (front_start, front_end) = (start.unwrap_or(-half - proud), end.unwrap_or(half + proud));
            let (back_start, back_end) = (start.unwrap_or(-half), end.unwrap_or(half));
            mesh.quad(
                Part::Plinth,
                [
                    face.point(front_start, 0.0, proud),
                    face.point(front_end, 0.0, proud),
                    face.point(front_end, plinth, proud),
                    face.point(front_start, plinth, proud),
                ],
                face.normal,
                [1.0; 4],
            );
            if proud > 0.0 {
                mesh.quad(
                    Part::Plinth,
                    [
                        face.point(back_start, plinth, 0.0),
                        face.point(back_end, plinth, 0.0),
                        face.point(front_end, plinth, proud),
                        face.point(front_start, plinth, proud),
                    ],
                    Vec3::Y,
                    [1.0; 4],
                );
                // Jambs where a grounded opening cuts it, facing into the opening.
                for (jamb, out) in [(start, face.right), (end, -face.right)] {
                    if let Some(a) = jamb {
                        mesh.quad(
                            Part::Plinth,
                            [
                                face.point(a, 0.0, 0.0),
                                face.point(a, 0.0, proud),
                                face.point(a, plinth, proud),
                                face.point(a, plinth, 0.0),
                            ],
                            out,
                            [0.8, 1.0, 1.0, 0.8],
                        );
                    }
                }
            }
        }
        Ok(())
    }

    /// An opening's reveal, back, frame or ring, and lattice.
    fn opening(&self, mesh: &mut Mesh, face: &Face, opening: &Opening, inside: bool) -> Result<(), String> {
        let plinth = self.plinth.height;
        let outline = opening.outline(plinth);
        let depth = self.recess.unwrap_or(2.0 * self.half.y);
        // The reveal: from the face to the back, facing into the opening; open along the
        // ground.
        let count = outline.points.len();
        let edges = if !inside {
            0
        } else if outline.grounded {
            count - 1
        } else {
            count
        };
        for k in 0..edges {
            let (a, b) = (outline.points[k], outline.points[(k + 1) % count]);
            let middle = 0.5 * (a + b);
            let toward = outline.center - middle;
            let out = (face.right * toward.x + Vec3::Y * toward.y).normalize_or_zero();
            // Edges along the ground leave nothing to see.
            if a.y <= 0.0 && b.y <= 0.0 {
                continue;
            }
            let back = if self.recess.is_some() { 0.6 } else { 1.0 };
            mesh.quad(
                Part::Plaster,
                [
                    face.point(a.x, a.y, 0.0),
                    face.point(b.x, b.y, 0.0),
                    face.point(b.x, b.y, -depth),
                    face.point(a.x, a.y, -depth),
                ],
                out,
                [0.95, 0.95, back, back],
            );
        }
        let [frame, frame_proud] = self.frame;
        match opening.kind {
            OpeningKind::Window | OpeningKind::Door => {
                self.rectangular_frame(mesh, face, opening, frame, frame_proud)?;
            }
            OpeningKind::Moon | OpeningKind::Gate => {
                ring(mesh, face, opening, &outline, plinth, frame, frame_proud);
            }
        }
        if let Some(recess) = self.recess {
            // The back: paper behind windows, a door leaf or a dark doorway.
            let part = if opening.grounded() { Part::Door } else { Part::Window };
            mesh.fill(part, &face.frame(-recess), &outline.points, &[], &|_| 1.0)?;
            if opening.kind == OpeningKind::Door {
                // The leaves' meeting stile, and a rail across both.
                let [a, _] = opening.at;
                let [width, height] = opening.size;
                let stile = Vec2::new(0.05, height);
                bar(
                    mesh,
                    Part::Timber,
                    face,
                    Vec2::new(a - 0.5 * stile.x, 0.0),
                    stile,
                    [-recess, -recess + 0.025],
                    true,
                    false,
                );
                let rail = Vec2::new(width, 0.07);
                bar(
                    mesh,
                    Part::Timber,
                    face,
                    Vec2::new(a - 0.5 * width, 0.36 * height),
                    rail,
                    [-recess, -recess + 0.018],
                    false,
                    false,
                );
            }
        }
        if inside && opening.lattice != [0, 0] && !opening.grounded() {
            let middle = match self.recess {
                Some(recess) => -0.45 * recess,
                None => -self.half.y,
            };
            lattice(mesh, face, opening, &outline, middle, self.recess.is_none());
        }
        Ok(())
    }

    /// A timber frame round a window (all round) or a door (from the plinth up).
    fn rectangular_frame(
        &self,
        mesh: &mut Mesh,
        face: &Face,
        opening: &Opening,
        width: f32,
        proud: f32,
    ) -> Result<(), String> {
        let [a, foot] = opening.at;
        let [w, h] = opening.size;
        let (l, r, top) = (a - 0.5 * w, a + 0.5 * w, foot + h);
        let (lo, ro, to) = (l - width, r + width, top + width);
        let door = opening.grounded();
        let bottom = if door { self.plinth.height } else { foot };
        let bo = if door { bottom } else { bottom - width };
        let front = face.frame(proud);
        if door {
            let outline = [
                Vec2::new(lo, bo),
                Vec2::new(l, bo),
                Vec2::new(l, top),
                Vec2::new(r, top),
                Vec2::new(r, bo),
                Vec2::new(ro, bo),
                Vec2::new(ro, to),
                Vec2::new(lo, to),
            ];
            mesh.fill(Part::Timber, &front, &outline, &[], &|_| 1.0)?;
        } else {
            let outer = [Vec2::new(lo, bo), Vec2::new(ro, bo), Vec2::new(ro, to), Vec2::new(lo, to)];
            let inner =
                vec![Vec2::new(l, bottom), Vec2::new(r, bottom), Vec2::new(r, top), Vec2::new(l, top)];
            mesh.fill(Part::Timber, &front, &outer, &[inner], &|_| 1.0)?;
        }
        let side = |mesh: &mut Mesh, a0: Vec2, a1: Vec2, out: Vec3| {
            mesh.quad(
                Part::Timber,
                [
                    face.point(a0.x, a0.y, 0.0),
                    face.point(a1.x, a1.y, 0.0),
                    face.point(a1.x, a1.y, proud),
                    face.point(a0.x, a0.y, proud),
                ],
                out,
                [1.0; 4],
            );
        };
        // Outer sides.
        side(mesh, Vec2::new(lo, bo), Vec2::new(lo, to), -face.right);
        side(mesh, Vec2::new(ro, bo), Vec2::new(ro, to), face.right);
        side(mesh, Vec2::new(lo, to), Vec2::new(ro, to), Vec3::Y);
        // Inner sides, running on from the reveal.
        side(mesh, Vec2::new(l, bottom), Vec2::new(l, top), face.right);
        side(mesh, Vec2::new(r, bottom), Vec2::new(r, top), -face.right);
        side(mesh, Vec2::new(l, top), Vec2::new(r, top), -Vec3::Y);
        if !door {
            side(mesh, Vec2::new(lo, bo), Vec2::new(ro, bo), -Vec3::Y);
            side(mesh, Vec2::new(l, bottom), Vec2::new(r, bottom), Vec3::Y);
        }
        Ok(())
    }

    /// Posts at the corners and between bays, from the plinth to just under the walls' top.
    fn posts(&self, mesh: &mut Mesh) {
        let Some(posts) = self.posts else { return };
        let (bottom, top) = (self.plinth.height, self.height - POST_TOP);
        let (w, q) = (posts.width, posts.proud);
        let [hx, hz] = self.half.to_array();
        // Corner posts: a square standing out of each corner by `q` both ways.
        for (sx, sz) in [(1.0, 1.0), (-1.0, 1.0), (-1.0, -1.0), (1.0, -1.0)] {
            let (outer_x, outer_z) = (sx * (hx + q), sz * (hz + q));
            let (inner_x, inner_z) = (sx * (hx + q - w), sz * (hz + q - w));
            let p = |x: f32, y: f32, z: f32| Vec3::new(x, y, z);
            let (nx, nz) = (Vec3::X * sx, Vec3::Z * sz);
            // The faces out along X and Z.
            mesh.quad(
                Part::Timber,
                [
                    p(outer_x, bottom, inner_z),
                    p(outer_x, bottom, outer_z),
                    p(outer_x, top, outer_z),
                    p(outer_x, top, inner_z),
                ],
                nx,
                [1.0; 4],
            );
            mesh.quad(
                Part::Timber,
                [
                    p(inner_x, bottom, outer_z),
                    p(outer_x, bottom, outer_z),
                    p(outer_x, top, outer_z),
                    p(inner_x, top, outer_z),
                ],
                nz,
                [1.0; 4],
            );
            // The sides beside the walls, where the post stands out of them.
            let (wall_x, wall_z) = (sx * hx, sz * hz);
            mesh.quad(
                Part::Timber,
                [
                    p(inner_x, bottom, wall_z),
                    p(inner_x, bottom, outer_z),
                    p(inner_x, top, outer_z),
                    p(inner_x, top, wall_z),
                ],
                -nx,
                [1.0; 4],
            );
            mesh.quad(
                Part::Timber,
                [
                    p(wall_x, bottom, inner_z),
                    p(outer_x, bottom, inner_z),
                    p(outer_x, top, inner_z),
                    p(wall_x, top, inner_z),
                ],
                -nz,
                [1.0; 4],
            );
            // Its top outside the walls: two strips.
            mesh.quad(
                Part::Timber,
                [
                    p(wall_x, top, inner_z),
                    p(outer_x, top, inner_z),
                    p(outer_x, top, outer_z),
                    p(wall_x, top, outer_z),
                ],
                Vec3::Y,
                [1.0; 4],
            );
            mesh.quad(
                Part::Timber,
                [
                    p(inner_x, top, wall_z),
                    p(wall_x, top, wall_z),
                    p(wall_x, top, outer_z),
                    p(inner_x, top, outer_z),
                ],
                Vec3::Y,
                [1.0; 4],
            );
        }
        // Posts between bays, on each side's face.
        for side in Side::ALL {
            let face = Face::of(side, self.half);
            let bays = if face.normal.z != 0.0 { posts.bays[0] } else { posts.bays[1] };
            for k in 1..bays {
                let a = -face.half + 2.0 * face.half * k as f32 / bays as f32;
                bar(
                    mesh,
                    Part::Timber,
                    &face,
                    Vec2::new(a - 0.5 * w, bottom),
                    Vec2::new(w, top - bottom),
                    [0.0, q],
                    true,
                    false,
                );
                mesh.quad(
                    Part::Timber,
                    [
                        face.point(a - 0.5 * w, top, 0.0),
                        face.point(a + 0.5 * w, top, 0.0),
                        face.point(a + 0.5 * w, top, q),
                        face.point(a - 0.5 * w, top, q),
                    ],
                    Vec3::Y,
                    [1.0; 4],
                );
            }
        }
    }

    /// Beams round all four sides, mitered at the corners.
    fn beams(&self, mesh: &mut Mesh) {
        for beam in self.beams {
            let (low, high, q) = (beam.at - beam.height, beam.at, beam.proud);
            for side in Side::ALL {
                let face = Face::of(side, self.half);
                let h = face.half;
                mesh.quad(
                    Part::Timber,
                    [
                        face.point(-h - q, low, q),
                        face.point(h + q, low, q),
                        face.point(h + q, high, q),
                        face.point(-h - q, high, q),
                    ],
                    face.normal,
                    [1.0; 4],
                );
                for (y, out) in [(high, Vec3::Y), (low, -Vec3::Y)] {
                    mesh.quad(
                        Part::Timber,
                        [
                            face.point(-h, y, 0.0),
                            face.point(h, y, 0.0),
                            face.point(h + q, y, q),
                            face.point(-h - q, y, q),
                        ],
                        out,
                        [1.0; 4],
                    );
                }
            }
        }
    }
}

/// A ring of stone round a moon window or over a moon gate (to the plinth's top), `width` wide
/// and standing `proud`.
fn ring(
    mesh: &mut Mesh,
    face: &Face,
    opening: &Opening,
    outline: &Outline,
    plinth: f32,
    width: f32,
    proud: f32,
) {
    let radius = opening.radius();
    let center = outline.center;
    let (inner, outer): (Vec<Vec2>, Vec<Vec2>) = match opening.kind {
        OpeningKind::Moon => {
            let inner = outline.points.clone();
            let outer = inner.iter().map(|p| center + (*p - center) * ((radius + width) / radius)).collect();
            (inner, outer)
        }
        _ => {
            // The arc above the plinth (or the ground); the outer arc's own feet are on it too.
            let inner: Vec<Vec2> =
                outline.points.iter().copied().filter(|p| p.y >= plinth.max(1e-6) - 1e-6).collect();
            let base = plinth.max(center.y - radius + 1e-4);
            let (from, to) = arc_span(radius + width, center.y, base);
            let count = inner.len() - 1;
            let outer = (0..=count)
                .map(|k| {
                    let angle = from + (to - from) * k as f32 / count as f32;
                    center + Vec2::new(angle.cos(), angle.sin()) * (radius + width)
                })
                .collect();
            (inner, outer)
        }
    };
    let closed = opening.kind == OpeningKind::Moon;
    let segments = if closed { inner.len() } else { inner.len() - 1 };
    let next = |k: usize| (k + 1) % inner.len();
    for k in 0..segments {
        let (i0, i1, o0, o1) = (inner[k], inner[next(k)], outer[k], outer[next(k)]);
        mesh.quad(
            Part::Trim,
            [
                face.point(i0.x, i0.y, proud),
                face.point(o0.x, o0.y, proud),
                face.point(o1.x, o1.y, proud),
                face.point(i1.x, i1.y, proud),
            ],
            face.normal,
            [1.0; 4],
        );
        let radial = |p: Vec2| {
            let d = (p - center).normalize_or_zero();
            face.right * d.x + Vec3::Y * d.y
        };
        mesh.quad(
            Part::Trim,
            [
                face.point(o0.x, o0.y, 0.0),
                face.point(o1.x, o1.y, 0.0),
                face.point(o1.x, o1.y, proud),
                face.point(o0.x, o0.y, proud),
            ],
            radial(0.5 * (o0 + o1)),
            [1.0; 4],
        );
        mesh.quad(
            Part::Trim,
            [
                face.point(i0.x, i0.y, 0.0),
                face.point(i1.x, i1.y, 0.0),
                face.point(i1.x, i1.y, proud),
                face.point(i0.x, i0.y, proud),
            ],
            -radial(0.5 * (i0 + i1)),
            [1.0; 4],
        );
    }
}

/// A timber bar on a face: from `corner` (its lower left, in the face's meters) `size` across and
/// up, between depths `depth[0]` and `depth[1]`: its front and the sides along it (left and right
/// for an `upright` bar, top and bottom for one across); `back` adds its back.
#[allow(clippy::too_many_arguments)]
fn bar(
    mesh: &mut Mesh,
    part: Part,
    face: &Face,
    corner: Vec2,
    size: Vec2,
    depth: [f32; 2],
    upright: bool,
    back: bool,
) {
    let (l, r, b, t) = (corner.x, corner.x + size.x, corner.y, corner.y + size.y);
    let [d0, d1] = depth;
    let p = |a: f32, y: f32, d: f32| face.point(a, y, d);
    mesh.quad(part, [p(l, b, d1), p(r, b, d1), p(r, t, d1), p(l, t, d1)], face.normal, [1.0; 4]);
    if back {
        mesh.quad(part, [p(l, b, d0), p(r, b, d0), p(r, t, d0), p(l, t, d0)], -face.normal, [1.0; 4]);
    }
    if upright {
        mesh.quad(part, [p(l, b, d0), p(l, b, d1), p(l, t, d1), p(l, t, d0)], -face.right, [1.0; 4]);
        mesh.quad(part, [p(r, b, d0), p(r, b, d1), p(r, t, d1), p(r, t, d0)], face.right, [1.0; 4]);
    } else {
        mesh.quad(part, [p(l, t, d0), p(r, t, d0), p(r, t, d1), p(l, t, d1)], Vec3::Y, [1.0; 4]);
        mesh.quad(part, [p(l, b, d0), p(r, b, d0), p(r, b, d1), p(l, b, d1)], -Vec3::Y, [1.0; 4]);
    }
}

/// A window's lattice: bars upright and across, evenly spaced and cut to its outline, their
/// middle `middle` deep; `through` gives them backs, for walls seen through.
fn lattice(mesh: &mut Mesh, face: &Face, opening: &Opening, outline: &Outline, middle: f32, through: bool) {
    let [ups, acrosses] = opening.lattice;
    let [width, depth] = BAR;
    let (min, max) =
        outline.points.iter().fold((Vec2::MAX, Vec2::MIN), |(lo, hi), p| (lo.min(*p), hi.max(*p)));
    let radius = opening.radius();
    let round = opening.kind == OpeningKind::Moon;
    // The opening's extent across a line through it, at `t` along the other axis.
    let span = |t: f32, along_x: bool| -> (f32, f32) {
        if round {
            let offset = if along_x { t - outline.center.y } else { t - outline.center.x };
            let half = (radius * radius - offset * offset).max(0.0).sqrt();
            let c = if along_x { outline.center.x } else { outline.center.y };
            (c - half, c + half)
        } else if along_x {
            (min.x, max.x)
        } else {
            (min.y, max.y)
        }
    };
    let layer = |front: bool| {
        let offset = if front { BAR_STEP } else { 0.0 };
        [middle - 0.5 * depth + offset, middle + 0.5 * depth + offset]
    };
    for k in 1..=ups {
        let a = min.x + (max.x - min.x) * k as f32 / (ups + 1) as f32;
        let (bottom, top) = span(a, false);
        bar(
            mesh,
            Part::Lattice,
            face,
            Vec2::new(a - 0.5 * width, bottom),
            Vec2::new(width, top - bottom),
            layer(true),
            true,
            through,
        );
    }
    for k in 1..=acrosses {
        let y = min.y + (max.y - min.y) * k as f32 / (acrosses + 1) as f32;
        let (left, right) = span(y, true);
        bar(
            mesh,
            Part::Lattice,
            face,
            Vec2::new(left, y - 0.5 * width),
            Vec2::new(right - left, width),
            layer(false),
            false,
            through,
        );
    }
}
