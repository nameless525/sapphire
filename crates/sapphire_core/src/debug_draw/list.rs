//! What the collector gives the frame packet: plain lines and labels, with shapes already turned
//! into lines.

use std::fmt;

use glam::{Quat, Vec3};

use super::{Color, Depth};

/// The most lines one frame's list holds; what doesn't fit is left out and counted.
pub const MAX_LINES: usize = 1 << 17;

/// The most labels one frame's list holds; what doesn't fit is left out and counted.
pub const MAX_LABELS: usize = 1 << 10;

/// The longest label, in bytes of UTF-8.
pub const LABEL_BYTES: usize = 47;

/// Segments in a full circle, a multiple of four so half circles land on their axes.
const CIRCLE_SEGMENTS: usize = 32;

/// A turn of `TAU / CIRCLE_SEGMENTS` as `(cos, sin)`: arcs step by it, a few multiplications a
/// point against a `sin_cos` (each point is a line in a frame's list).
const CIRCLE_STEP: (f32, f32) = (0.980_785_25, 0.195_090_32);

/// One debug line in world space, in a display color (see [`Color`]).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DebugLine {
    pub from: Vec3,
    pub to: Vec3,
    pub color: Color,
    pub depth: Depth,
}

/// Text beside a world position.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DebugLabel {
    pub position: Vec3,
    pub color: Color,
    pub text: LabelText,
}

/// A label's text, held inline so a call allocates nothing, cut at a character boundary to fit
/// [`LABEL_BYTES`].
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct LabelText {
    bytes: [u8; LABEL_BYTES],
    len: u8,
}

impl LabelText {
    /// `text`, formatted, as much as fits.
    pub fn format(text: fmt::Arguments<'_>) -> Self {
        let mut label = Self { bytes: [0; LABEL_BYTES], len: 0 };
        // A failing `Display`, or a cut (`write_str`), leaves what was written so far: it is only a
        // label.
        let _ = fmt::write(&mut label, text);
        label
    }

    pub fn as_str(&self) -> &str {
        // Only whole characters are ever copied in (`write_str`).
        std::str::from_utf8(&self.bytes[..usize::from(self.len)]).unwrap_or_default()
    }
}

impl fmt::Write for LabelText {
    /// Takes the whole characters of `text` that fit, and fails if that isn't all of it, which ends
    /// the formatting: the room a wide character left mustn't go to what comes after it.
    fn write_str(&mut self, text: &str) -> fmt::Result {
        let len = usize::from(self.len);
        let mut take = text.len().min(LABEL_BYTES - len);
        while !text.is_char_boundary(take) {
            take -= 1;
        }
        self.bytes[len..len + take].copy_from_slice(&text.as_bytes()[..take]);
        // At most `LABEL_BYTES`.
        self.len = (len + take) as u8;
        if take == text.len() { Ok(()) } else { Err(fmt::Error) }
    }
}

impl fmt::Debug for LabelText {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(self.as_str(), f)
    }
}

/// A shape to draw as lines ([`DebugDrawList::shape`]), in world space.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Shape {
    Line {
        from: Vec3,
        to: Vec3,
    },
    /// A line with a four-pronged head at `to`.
    Arrow {
        from: Vec3,
        to: Vec3,
    },
    /// An axis-aligned box between two corners.
    Aabb {
        min: Vec3,
        max: Vec3,
    },
    OrientedBox {
        center: Vec3,
        half_extents: Vec3,
        rotation: Quat,
    },
    /// Three great circles.
    Sphere {
        center: Vec3,
        radius: f32,
    },
    /// A circle at each end, four lines between them and two half circles over each end.
    Capsule {
        from: Vec3,
        to: Vec3,
        radius: f32,
    },
    Circle {
        center: Vec3,
        normal: Vec3,
        radius: f32,
    },
    /// X, Y and Z in red, green and blue, at the color's alpha.
    Axes {
        position: Vec3,
        rotation: Quat,
        length: f32,
    },
}

impl Shape {
    /// Whether every number it was given is finite.
    fn is_finite(&self) -> bool {
        match *self {
            Self::Line { from, to } | Self::Arrow { from, to } => from.is_finite() && to.is_finite(),
            Self::Aabb { min, max } => min.is_finite() && max.is_finite(),
            Self::OrientedBox { center, half_extents, rotation } => {
                center.is_finite() && half_extents.is_finite() && rotation.is_finite()
            }
            Self::Sphere { center, radius } => center.is_finite() && radius.is_finite(),
            Self::Capsule { from, to, radius } => from.is_finite() && to.is_finite() && radius.is_finite(),
            Self::Circle { center, normal, radius } => {
                center.is_finite() && normal.is_finite() && radius.is_finite()
            }
            Self::Axes { position, rotation, length } => {
                position.is_finite() && rotation.is_finite() && length.is_finite()
            }
        }
    }
}

/// What a frame's debug draws cost and lost (rule R9): `RenderStats` carries them to the overlay's
/// stats panel and the bench.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct DebugDrawStats {
    /// Lines in the list: every shape's.
    pub lines: u32,
    /// Shapes in the list, a lone line among them.
    pub shapes: u32,
    /// Labels in the list.
    pub labels: u32,
    /// Timed shapes and labels alive ([`Style::for_seconds`](super::Style::for_seconds)).
    pub persistent: u32,
    /// Shapes and labels a fixed step holds ([`begin_step`](super::begin_step)).
    pub held: u32,
    /// Calls lost this frame: a full thread buffer or store, a list past its limit (a shape that
    /// fits only in part counts, and what fits draws), or coordinates that aren't finite.
    pub dropped: u32,
}

/// A frame's debug draws, as plain data, in `FramePacket::debug`: empty in a build that never
/// enables drawing. The collector fills it, and tests and tools may fill one directly.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct DebugDrawList {
    pub lines: Vec<DebugLine>,
    pub labels: Vec<DebugLabel>,
    pub stats: DebugDrawStats,
}

impl DebugDrawList {
    /// Empties it, keeping its capacity.
    pub fn clear(&mut self) {
        self.lines.clear();
        self.labels.clear();
        self.stats = DebugDrawStats::default();
    }

    /// Adds `shape` as lines, as many as fit under [`MAX_LINES`]. A shape with numbers that aren't
    /// finite draws nothing. Either way it is counted ([`DebugDrawStats::dropped`]).
    pub fn shape(&mut self, shape: Shape, color: Color, depth: Depth) {
        if !shape.is_finite() {
            self.stats.dropped += 1;
            return;
        }
        let before = self.lines.len();
        let mut complete = true;
        tessellate(&shape, color, &mut |from, to, color| {
            // Finite numbers can still overflow as they turn into points.
            if self.lines.len() < MAX_LINES && from.is_finite() && to.is_finite() {
                self.lines.push(DebugLine { from, to, color, depth });
            } else {
                complete = false;
            }
        });
        self.stats.lines = self.lines.len() as u32;
        self.stats.shapes += u32::from(self.lines.len() > before);
        self.stats.dropped += u32::from(!complete);
    }

    /// Adds a label, if it fits under [`MAX_LABELS`] and its position is finite; counted if not.
    pub fn label(&mut self, position: Vec3, color: Color, text: LabelText) {
        if self.labels.len() < MAX_LABELS && position.is_finite() {
            self.labels.push(DebugLabel { position, color, text });
            self.stats.labels = self.labels.len() as u32;
        } else {
            self.stats.dropped += 1;
        }
    }
}

/// `shape`'s lines, each given to `emit` with its color.
fn tessellate(shape: &Shape, color: Color, emit: &mut dyn FnMut(Vec3, Vec3, Color)) {
    if let Shape::Axes { position, rotation, length } = *shape {
        let axes = [(Vec3::X, Color::RED), (Vec3::Y, Color::GREEN), (Vec3::Z, Color::BLUE)];
        for (axis, tone) in axes {
            emit(position, position + rotation * axis * length, tone.with_alpha(color.a));
        }
        return;
    }
    let mut line = |from, to| emit(from, to, color);
    match *shape {
        Shape::Line { from, to } => line(from, to),
        Shape::Arrow { from, to } => {
            line(from, to);
            let along = to - from;
            let (u, v) = along.normalize_or(Vec3::Y).any_orthonormal_pair();
            // The head is a fifth of the arrow, half as wide as it is long.
            let (back, wide) = (to - along * 0.2, along.length() * 0.05);
            for side in [u, -u, v, -v] {
                line(to, back + side * wide);
            }
        }
        Shape::Aabb { min, max } => {
            box_lines((min + max) * 0.5, (max - min) * 0.5, Quat::IDENTITY, &mut line);
        }
        Shape::OrientedBox { center, half_extents, rotation } => {
            box_lines(center, half_extents, rotation, &mut line);
        }
        Shape::Sphere { center, radius } => {
            arc(center, Vec3::X * radius, Vec3::Y * radius, CIRCLE_SEGMENTS, &mut line);
            arc(center, Vec3::Y * radius, Vec3::Z * radius, CIRCLE_SEGMENTS, &mut line);
            arc(center, Vec3::Z * radius, Vec3::X * radius, CIRCLE_SEGMENTS, &mut line);
        }
        Shape::Capsule { from, to, radius } => {
            let axis = (to - from).normalize_or(Vec3::Y);
            let (u, v) = axis.any_orthonormal_pair();
            let (u, v, axis) = (u * radius, v * radius, axis * radius);
            arc(from, u, v, CIRCLE_SEGMENTS, &mut line);
            arc(to, u, v, CIRCLE_SEGMENTS, &mut line);
            for side in [u, -u, v, -v] {
                line(from + side, to + side);
            }
            // Half circles from one side over the end to the other.
            for side in [u, v] {
                arc(to, side, axis, CIRCLE_SEGMENTS / 2, &mut line);
                arc(from, side, -axis, CIRCLE_SEGMENTS / 2, &mut line);
            }
        }
        Shape::Circle { center, normal, radius } => {
            let (u, v) = normal.normalize_or(Vec3::Y).any_orthonormal_pair();
            arc(center, u * radius, v * radius, CIRCLE_SEGMENTS, &mut line);
        }
        // Drawn above, in its own colors.
        Shape::Axes { .. } => {}
    }
}

/// The twelve edges of a box.
fn box_lines(center: Vec3, half: Vec3, rotation: Quat, line: &mut impl FnMut(Vec3, Vec3)) {
    // Corner `index`'s bits say which side of each axis it is on.
    let corner = |index: u32| {
        let side = |bit: u32| if index & (1 << bit) == 0 { -1.0 } else { 1.0 };
        center + rotation * (half * Vec3::new(side(0), side(1), side(2)))
    };
    for index in 0..8_u32 {
        for bit in 0..3 {
            // Each edge once, from its corner on the axis's low side.
            if index & (1 << bit) == 0 {
                line(corner(index), corner(index | (1 << bit)));
            }
        }
    }
}

/// `segments` of a circle around `center`, from `center + u` towards `center + v`, each a
/// [`CIRCLE_STEP`] turn. `u` and `v` are at right angles and of one length.
fn arc(center: Vec3, u: Vec3, v: Vec3, segments: usize, line: &mut impl FnMut(Vec3, Vec3)) {
    let (mut cos, mut sin) = (1.0_f32, 0.0_f32);
    let first = center + u;
    let mut previous = first;
    for segment in 1..=segments {
        (cos, sin) = (cos * CIRCLE_STEP.0 - sin * CIRCLE_STEP.1, sin * CIRCLE_STEP.0 + cos * CIRCLE_STEP.1);
        // A whole circle closes exactly.
        let point = if segment == CIRCLE_SEGMENTS { first } else { center + u * cos + v * sin };
        line(previous, point);
        previous = point;
    }
}

#[cfg(test)]
mod tests {
    use std::f32::consts::{FRAC_1_SQRT_2, FRAC_PI_2};

    use super::*;

    fn lines(shape: Shape) -> Vec<DebugLine> {
        let mut list = DebugDrawList::default();
        list.shape(shape, Color::RED, Depth::Tested);
        assert_eq!(list.stats.lines as usize, list.lines.len());
        assert_eq!(list.stats.dropped, 0, "{shape:?}");
        list.lines
    }

    fn assert_near(a: Vec3, b: Vec3) {
        assert!(a.distance(b) < 1e-4, "{a} against {b}");
    }

    #[test]
    fn each_shape_has_the_lines_it_should() {
        let (a, b) = (Vec3::ZERO, Vec3::new(0.0, 2.0, 0.0));
        assert_eq!(lines(Shape::Line { from: a, to: b }).len(), 1);
        assert_eq!(lines(Shape::Arrow { from: a, to: b }).len(), 5);
        assert_eq!(lines(Shape::Aabb { min: a, max: Vec3::ONE }).len(), 12);
        let rotation = Quat::from_rotation_y(1.0);
        assert_eq!(lines(Shape::OrientedBox { center: a, half_extents: b, rotation }).len(), 12);
        assert_eq!(lines(Shape::Sphere { center: a, radius: 1.0 }).len(), 96);
        assert_eq!(lines(Shape::Capsule { from: a, to: b, radius: 0.5 }).len(), 132);
        assert_eq!(lines(Shape::Circle { center: a, normal: Vec3::Y, radius: 1.0 }).len(), 32);
        assert_eq!(lines(Shape::Axes { position: a, rotation, length: 1.0 }).len(), 3);
    }

    #[test]
    fn a_box_has_each_edge_once_at_its_corners() {
        let (min, max) = (Vec3::new(-1.0, 0.0, 2.0), Vec3::new(1.0, 3.0, 2.5));
        let lines = lines(Shape::Aabb { min, max });
        for line in &lines {
            // An edge runs along one axis, so two of its coordinates agree.
            let same = (line.to - line.from).to_array().iter().filter(|&&delta| delta == 0.0).count();
            assert_eq!(same, 2, "{line:?}");
            for point in [line.from, line.to] {
                assert!(point.x == min.x || point.x == max.x);
                assert!(point.y == min.y || point.y == max.y);
                assert!(point.z == min.z || point.z == max.z);
            }
        }
        let mut edges: Vec<_> =
            lines.iter().map(|line| ((line.from + line.to) * 0.5).to_array().map(f32::to_bits)).collect();
        edges.sort_unstable();
        edges.dedup();
        assert_eq!(edges.len(), 12, "twelve different edges");
    }

    #[test]
    fn an_oriented_box_turns_about_its_center() {
        let center = Vec3::new(1.0, 2.0, 3.0);
        let half_extents = Vec3::new(1.0, 0.5, 0.25);
        let rotation = Quat::from_rotation_y(FRAC_PI_2);
        for line in lines(Shape::OrientedBox { center, half_extents, rotation }) {
            // A quarter turn about Y swaps the X and Z extents.
            for point in [line.from, line.to] {
                assert_near((point - center).abs(), Vec3::new(0.25, 0.5, 1.0));
            }
        }
    }

    #[test]
    fn round_shapes_stay_at_their_radius_and_close() {
        let center = Vec3::new(1.0, -2.0, 0.5);
        for line in lines(Shape::Sphere { center, radius: 2.0 }) {
            assert!((line.from.distance(center) - 2.0).abs() < 1e-4);
            assert!((line.to.distance(center) - 2.0).abs() < 1e-4);
        }
        let normal = Vec3::new(1.0, 1.0, 0.0);
        let circle = lines(Shape::Circle { center, normal, radius: 3.0 });
        for line in &circle {
            assert!((line.from.distance(center) - 3.0).abs() < 1e-4);
            assert!((line.from - center).dot(normal).abs() < 1e-4, "in the plane facing the normal");
        }
        assert_eq!(circle[0].from, circle[31].to, "a whole circle closes exactly");
        // An eighth of the way round, the arc's step is right.
        let (eighth, first) = (circle[3].to - center, circle[0].from - center);
        assert!((eighth.dot(first) / 9.0 - FRAC_1_SQRT_2).abs() < 1e-4);
    }

    #[test]
    fn a_capsule_reaches_past_its_ends_by_its_radius() {
        let (from, to) = (Vec3::new(0.0, 1.0, 0.0), Vec3::new(0.0, 3.0, 0.0));
        let lines = lines(Shape::Capsule { from, to, radius: 0.5 });
        let highest = lines.iter().map(|line| line.from.y.max(line.to.y)).fold(f32::MIN, f32::max);
        let lowest = lines.iter().map(|line| line.from.y.min(line.to.y)).fold(f32::MAX, f32::min);
        assert!((highest - 3.5).abs() < 1e-4, "{highest}");
        assert!((lowest - 0.5).abs() < 1e-4, "{lowest}");
        for line in &lines {
            for point in [line.from, line.to] {
                // Every point is a radius from the segment.
                let nearest = Vec3::new(0.0, point.y.clamp(1.0, 3.0), 0.0);
                assert!((point.distance(nearest) - 0.5).abs() < 1e-4, "{point}");
            }
        }
    }

    #[test]
    fn a_degenerate_capsule_or_arrow_still_draws() {
        let at = Vec3::new(1.0, 1.0, 1.0);
        assert_eq!(lines(Shape::Capsule { from: at, to: at, radius: 0.5 }).len(), 132);
        for line in lines(Shape::Arrow { from: at, to: at }) {
            assert_near(line.from, at);
            assert_near(line.to, at);
        }
    }

    #[test]
    fn an_arrows_head_is_at_its_end() {
        let (from, to) = (Vec3::ZERO, Vec3::new(0.0, 0.0, 2.0));
        let lines = lines(Shape::Arrow { from, to });
        for line in &lines[1..] {
            assert_eq!(line.from, to);
            assert!((line.to.z - 1.6).abs() < 1e-5, "a fifth of the way back: {}", line.to);
            assert!((line.to.truncate().length() - 0.1).abs() < 1e-5, "{}", line.to);
        }
    }

    #[test]
    fn axes_take_their_own_colors_and_the_styles_alpha() {
        let mut list = DebugDrawList::default();
        let rotation = Quat::from_rotation_z(FRAC_PI_2);
        let shape = Shape::Axes { position: Vec3::ONE, rotation, length: 2.0 };
        list.shape(shape, Color::WHITE.with_alpha(128), Depth::OnTop);
        let [x, y, z] = list.lines[..] else { panic!("three axes") };
        assert_eq!(x.color, Color::RED.with_alpha(128));
        assert_eq!(y.color, Color::GREEN.with_alpha(128));
        assert_eq!(z.color, Color::BLUE.with_alpha(128));
        assert_eq!(x.depth, Depth::OnTop);
        assert_near(x.to, Vec3::ONE + Vec3::new(0.0, 2.0, 0.0));
        assert_near(y.to, Vec3::ONE + Vec3::new(-2.0, 0.0, 0.0));
        assert_near(z.to, Vec3::ONE + Vec3::new(0.0, 0.0, 2.0));
    }

    #[test]
    fn numbers_that_arent_finite_draw_nothing_and_are_counted() {
        let bad = [
            Shape::Line { from: Vec3::NAN, to: Vec3::ONE },
            Shape::Line { from: Vec3::ONE, to: Vec3::INFINITY },
            Shape::Arrow { from: Vec3::NAN, to: Vec3::ONE },
            Shape::Sphere { center: Vec3::ONE, radius: f32::NAN },
            Shape::Sphere { center: Vec3::NAN, radius: 1.0 },
            Shape::Capsule { from: Vec3::ZERO, to: Vec3::NAN, radius: 1.0 },
            Shape::Circle { center: Vec3::ZERO, normal: Vec3::NAN, radius: 1.0 },
            Shape::OrientedBox { center: Vec3::ZERO, half_extents: Vec3::ONE, rotation: Quat::NAN },
            Shape::Axes { position: Vec3::ZERO, rotation: Quat::NAN, length: 1.0 },
            Shape::Aabb { min: Vec3::NAN, max: Vec3::ONE },
        ];
        for shape in bad {
            let mut list = DebugDrawList::default();
            list.shape(shape, Color::RED, Depth::Tested);
            assert!(list.lines.is_empty(), "{shape:?}");
            assert_eq!(list.stats.shapes, 0, "{shape:?}");
            assert_eq!(list.stats.dropped, 1, "{shape:?}");
        }
        // Finite numbers that overflow as they turn into points are left out line by line.
        let mut list = DebugDrawList::default();
        let (min, max) = (Vec3::splat(f32::MIN), Vec3::splat(f32::MAX));
        list.shape(Shape::Aabb { min, max }, Color::RED, Depth::Tested);
        assert!(list.lines.is_empty());
        assert_eq!(list.stats.dropped, 1);
    }

    #[test]
    fn a_full_list_keeps_what_fits_and_counts_the_rest() {
        let mut list = DebugDrawList::default();
        let line = Shape::Line { from: Vec3::ZERO, to: Vec3::ONE };
        for _ in 0..MAX_LINES - 10 {
            list.shape(line, Color::RED, Depth::Tested);
        }
        assert_eq!(list.stats.dropped, 0);
        // Only 10 of a sphere's 96 lines fit: they draw, and it counts as a drop.
        list.shape(Shape::Sphere { center: Vec3::ZERO, radius: 1.0 }, Color::RED, Depth::Tested);
        assert_eq!(list.lines.len(), MAX_LINES);
        assert_eq!(list.stats.lines as usize, MAX_LINES);
        assert_eq!(list.stats.dropped, 1);
        list.shape(line, Color::RED, Depth::Tested);
        assert_eq!(list.lines.len(), MAX_LINES);
        assert_eq!(list.stats.dropped, 2);
        for _ in 0..MAX_LABELS + 3 {
            list.label(Vec3::ZERO, Color::RED, LabelText::format(format_args!("x")));
        }
        assert_eq!(list.labels.len(), MAX_LABELS);
        assert_eq!(list.stats.dropped, 5);
        list.label(Vec3::NAN, Color::RED, LabelText::format(format_args!("x")));
        assert_eq!(list.stats.dropped, 6);
        list.clear();
        assert_eq!(list.stats, DebugDrawStats::default());
        assert!(list.lines.capacity() >= MAX_LINES, "keeps its capacity");
    }

    #[test]
    fn a_label_is_cut_at_a_character_boundary() {
        assert_eq!(LabelText::format(format_args!("hp {}", 12)).as_str(), "hp 12");
        let long = "x".repeat(LABEL_BYTES + 5);
        assert_eq!(LabelText::format(format_args!("{long}")).as_str().len(), LABEL_BYTES);
        // Three bytes each, so the last one straddles the limit (47 = 15 × 3 + 2).
        let wide = "字".repeat(20);
        assert_eq!(LabelText::format(format_args!("{wide}")).as_str(), "字".repeat(15));
        // Writes after the buffer is full, in several pieces, change nothing.
        let pieces = LabelText::format(format_args!("{long}{}{}", 1, "y"));
        assert_eq!(pieces.as_str(), "x".repeat(LABEL_BYTES));
    }

    /// A character too wide for the room left ends the label there, or the smaller writes after it
    /// would fill the room and the label would read as text nobody wrote.
    #[test]
    fn nothing_is_written_after_a_cut() {
        struct Pieces<'a>(&'a [&'a str]);
        impl fmt::Display for Pieces<'_> {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                self.0.iter().try_for_each(|piece| f.write_str(piece))
            }
        }
        let room_for_one = "a".repeat(LABEL_BYTES - 1);
        let wide = "字".repeat(15);
        let full = format!("{room_for_one}b");
        for (pieces, expected) in [
            // One byte left, for characters of four, three and two.
            (&[room_for_one.as_str(), "🗡", "Z"], room_for_one.as_str()),
            (&[room_for_one.as_str(), "⚔", "Z"], room_for_one.as_str()),
            (&[room_for_one.as_str(), "é", "Z"], room_for_one.as_str()),
            // A piece cut partway keeps the whole characters that fit, and nothing comes after it.
            (&[wide.as_str(), "字Z", "Y"], wide.as_str()),
            // Exactly full is no cut, and an empty write after it changes nothing.
            (&[room_for_one.as_str(), "b", ""], full.as_str()),
        ] {
            let label = LabelText::format(format_args!("{}", Pieces(pieces)));
            assert_eq!(label.as_str(), expected, "{pieces:?}");
            assert!(pieces.concat().starts_with(label.as_str()), "a label is a prefix of its text");
        }
    }
}
