//! `--debug-draw`: fixed debug draws through the real path, from the facade in `sapphire_core`
//! through the collector and the frame packet to the renderer's pass (spec §5.20, M2 T1), for the
//! golden scene `debugdraw-shapes` and for measuring the pass at a stated load.
//!
//! Drawing stays off unless this flag or a `dev` build turns it on, so no other scene or shipping
//! frame sees any of it.

use sapphire_core::debug_draw::{self as dd, Color, Style};
use sapphire_math::{Quat, Vec3};

/// Timed draws last this long on the world clock: longer than any capture.
const LONG: f32 = 3600.0;

/// What `--debug-draw` draws.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DebugDrawDemo {
    /// One of each shape, for each way a draw can last and face a surface, around the character.
    Shapes,
    /// This many lines a frame, depth-tested, across the frame, for benches.
    Lines(u32),
}

impl DebugDrawDemo {
    /// Parses `shapes` or `lines:<count>`, or says nothing if it's neither.
    pub fn parse(value: &str) -> Option<Self> {
        if value == "shapes" {
            return Some(Self::Shapes);
        }
        value.strip_prefix("lines:")?.parse().ok().map(Self::Lines)
    }

    /// Once, as the scene starts: what stays.
    pub(crate) fn draw_once(self) {
        match self {
            Self::Shapes => {
                let timed = Style::new(Color::rgb(160, 120, 255)).for_seconds(LONG);
                let tilt = Quat::from_rotation_y(0.6);
                dd::oriented_box(Vec3::new(0.0, 0.02, 0.0), Vec3::new(0.7, 0.02, 0.7), tilt, timed);
            }
            Self::Lines(_) => {}
        }
    }

    /// Inside each fixed step: held until the next, whatever the frame rate.
    pub(crate) fn draw_in_step(self) {
        if self == Self::Shapes {
            dd::arrow(Vec3::new(-0.5, 2.25, 0.0), Vec3::new(0.5, 2.25, 0.0), Color::GREEN);
        }
    }

    /// Each frame.
    pub(crate) fn draw_in_frame(self) {
        match self {
            Self::Shapes => shapes(),
            Self::Lines(count) => lines(count),
        }
    }
}

/// One of each shape beside the character, depth-tested; then what a surface does to a line: one
/// behind the character and one in front, and a volume inside it, hidden and on top.
fn shapes() {
    let turned = Quat::from_rotation_y(0.7) * Quat::from_rotation_z(0.4);
    dd::aabb(Vec3::new(-1.5, 0.05, -0.3), Vec3::new(-0.95, 0.6, 0.3), Color::YELLOW);
    dd::oriented_box(Vec3::new(-1.25, 1.1, 0.0), Vec3::new(0.3, 0.18, 0.18), turned, Color::ORANGE);
    dd::sphere(Vec3::new(-1.25, 1.75, 0.0), 0.28, Color::CYAN);
    dd::capsule(Vec3::new(1.05, 0.2, 0.0), Vec3::new(1.45, 0.75, 0.0), 0.16, Color::GREEN);
    dd::circle(Vec3::new(1.25, 1.2, 0.0), Vec3::new(0.3, 1.0, 0.6), 0.3, Color::MAGENTA);
    dd::arrow(Vec3::new(1.0, 1.6, 0.0), Vec3::new(1.55, 1.85, 0.2), Color::WHITE);
    dd::axes(Vec3::new(1.25, 2.0, 0.0), turned, 0.3, Color::WHITE);

    let across = |height: f32, z: f32| (Vec3::new(-0.9, height, z), Vec3::new(0.9, height, z));
    let (from, to) = across(1.0, -0.6);
    dd::line(from, to, Color::BLUE);
    let (from, to) = across(0.8, 0.6);
    dd::line(from, to, Color::RED);
    let (from, to) = across(0.6, -0.6);
    dd::line(from, to, Style::new(Color::BLUE).on_top());
    let (from, to) = across(0.4, 0.6);
    dd::line(from, to, Color::RED.with_alpha(128));

    let (feet, chest) = (Vec3::new(0.0, 0.5, 0.0), Vec3::new(0.0, 1.3, 0.0));
    dd::capsule(feet, chest, 0.05, Color::GREEN);
    dd::capsule(feet, chest, 0.1, Style::new(Color::MAGENTA).on_top());
    dd::label(Vec3::new(1.05, 0.2, 0.0), Color::WHITE, format_args!("capsule r {:.2}", 0.16));
}

/// `count` lines over the frame, as a bench's load: a fan through the character's height, so most
/// are hidden or crossing what's in front of them, each about a quarter of the frame long.
fn lines(count: u32) {
    for index in 0..count {
        let t = index as f32 / count.max(1) as f32;
        let (sin, cos) = (t * std::f32::consts::TAU).sin_cos();
        let center = Vec3::new(cos * 0.8, 1.0 + sin * 0.8, 0.5 * sin);
        dd::line(center, center + Vec3::new(sin * 0.6, cos * 0.6, 0.0), Color::CYAN);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_flags_value_is_parsed() {
        assert_eq!(DebugDrawDemo::parse("shapes"), Some(DebugDrawDemo::Shapes));
        assert_eq!(DebugDrawDemo::parse("lines:2000"), Some(DebugDrawDemo::Lines(2000)));
        for bad in ["", "lines", "lines:", "lines:many", "circles"] {
            assert_eq!(DebugDrawDemo::parse(bad), None, "{bad}");
        }
    }
}
