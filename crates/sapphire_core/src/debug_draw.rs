//! Debug draw (spec §5.20, M2 T1): any code on any thread draws a line, a shape or a label into
//! the next frame, for the tuning views nothing can be tuned without.
//!
//! ```
//! use glam::{Quat, Vec3};
//! use sapphire_core::debug_draw::{self as dd, Color, Style};
//!
//! // Off by default: until something enables it, each of these is one relaxed atomic load.
//! dd::set_enabled(true);
//!
//! // One frame, hidden behind what the frame draws in front of it.
//! dd::line(Vec3::ZERO, Vec3::X, Color::RED);
//! // Readable inside a character, for two seconds on the app's clock.
//! dd::capsule(Vec3::ZERO, Vec3::Y, 0.3, Style::new(Color::CYAN).on_top().for_seconds(2.0));
//! dd::oriented_box(Vec3::Y, Vec3::splat(0.2), Quat::IDENTITY, Color::ORANGE);
//! dd::label(Vec3::Y, Color::WHITE, format_args!("hit {}", 12));
//!
//! // The app, once a frame: merge what every thread drew into the frame packet's list.
//! let mut collector = dd::Collector::new();
//! let mut list = dd::DebugDrawList::default();
//! let world_clock_seconds = 0.0;
//! collector.collect(world_clock_seconds, &mut list);
//! assert_eq!(list.stats.labels, 1);
//! # dd::set_enabled(false);
//! ```
//!
//! # The facade
//!
//! Spec §5.20 has shipping builds compile these calls to no-ops through a thin facade here, and
//! rule R8 keeps `dev` features off runtime crates, so this works as [cvars](crate::cvar) do: the
//! crate that draws depends on `sapphire_core` alone, so a simulation crate gains no edge to the
//! renderer, and each call starts with one relaxed load of one static. Nothing writes that static
//! in a shipping build ([`set_enabled`] is called only by development tools: the sandbox's `dev`
//! build and its `--debug-draw` flag), so every call there is that load and a branch, with no
//! allocation, however many arguments it names. Arguments the caller computes are still computed,
//! so wrap costly gathering in `if` [`enabled()`]. A [`label`]'s text isn't formatted unless
//! drawing is on.
//!
//! # From any thread, with no lock
//!
//! Each thread writes to a buffer of its own (the `buffer` module's ring of plain commands, one
//! writer and one reader), so a call takes no lock and never waits. A thread's first call
//! allocates its ring and registers it, under a lock; every call after allocates nothing, so a
//! call inside the fixed step keeps rule R6 once [warmed](warm_thread). [`set_enabled`] warms the
//! thread that calls it. A full ring drops the call and counts it ([`DebugDrawStats::dropped`]):
//! what fits draws.
//!
//! [`Collector::collect`] merges the rings at extract, each thread's calls in order, and turns
//! shapes into lines for the frame packet's [`DebugDrawList`], which is plain data (rule R4): the
//! renderer draws its lines and never sees this module's state. The list is empty on every frame
//! of a build that never enables drawing.
//!
//! # How long a draw lasts
//!
//! - **One frame**, the default ([`Style::new`]), drawn by the next [`Collector::collect`].
//! - **Inside a fixed step**, one frame means one step, because a frame may run no step at all (a
//!   120 Hz display) or several (a phone at 30 fps). The app brackets the steps with
//!   [`begin_step`] and [`end_step`], and what a step draws on that thread is held until a later
//!   step draws, or until a later step has run and the clock has moved on. So it neither flickers
//!   between steps nor doubles when two steps share a frame, and a paused simulation keeps its
//!   last step's shapes on screen to be looked at.
//! - **For a duration** ([`Style::for_seconds`]), on the clock the app gives
//!   [`Collector::collect`]. **Decision: the owner clock, not presentation.** The sandbox passes
//!   the world clock (spec §4.5), since these shapes describe the simulation and rule R2 has
//!   gameplay time read its owner's clock: pausing or single-stepping the world to inspect a hit
//!   volume keeps it there, and slow motion stretches it with what it marks. This module reads no
//!   clock itself (it sits below `sapphire_time`).
//!
//! # Not here
//!
//! The pass that draws lines is `sapphire_render::debug_draw`, and the labels are drawn by
//! `sapphire_debug`'s overlay (dev builds). The combat views built on this (hit and hurt volumes,
//! colliders, the intent buffer, the replay recorder) wait for the packages they show.

mod buffer;
mod collect;
mod list;

use std::fmt;
use std::sync::atomic::{AtomicBool, Ordering::Relaxed};

use glam::{Quat, Vec3};

pub use self::buffer::THREAD_BUFFER;
pub use self::collect::{Collector, MAX_HELD, MAX_PERSISTENT};
pub use self::list::{
    DebugDrawList, DebugDrawStats, DebugLabel, DebugLine, LABEL_BYTES, LabelText, MAX_LABELS, MAX_LINES,
    Shape,
};

/// Whether calls draw. Only development tools write it; see the [module docs](self).
static ENABLED: AtomicBool = AtomicBool::new(false);

/// Whether calls draw: one relaxed load, the whole cost of a call while they don't. For guarding
/// work done only to draw.
#[inline]
pub fn enabled() -> bool {
    ENABLED.load(Relaxed)
}

/// Turns drawing on or off, from any thread. Development tools alone call this, so a shipping
/// build's calls stay no-ops. Turning it on [warms](warm_thread) the calling thread; turning it
/// off drops what is waiting at the next [`Collector::collect`], and what it holds.
pub fn set_enabled(on: bool) {
    ENABLED.store(on, Relaxed);
    if on {
        warm_thread();
    }
}

/// Makes this thread's buffer now, so its first draw allocates nothing. Rule R6 asks it of a
/// thread that draws inside a fixed step, unless it is the one that called [`set_enabled`].
pub fn warm_thread() {
    buffer::warm();
}

/// Marks this thread as running a fixed step, until [`end_step`]: what it draws for one frame is
/// held for the step instead (see the [module docs](self)). Off, this is the one load; on, it
/// allocates nothing on a [warm](warm_thread) thread.
#[inline]
pub fn begin_step() {
    if enabled() {
        buffer::begin_step();
    }
}

/// Ends what [`begin_step`] began.
#[inline]
pub fn end_step() {
    if enabled() {
        buffer::end_step();
    }
}

/// A display color for debug draws: sRGB-encoded with straight alpha, drawn as is, past exposure
/// and grading.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Color {
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub a: u8,
}

impl Color {
    pub const RED: Self = Self::rgb(255, 64, 64);
    pub const GREEN: Self = Self::rgb(64, 230, 96);
    pub const BLUE: Self = Self::rgb(72, 128, 255);
    pub const YELLOW: Self = Self::rgb(255, 224, 64);
    pub const ORANGE: Self = Self::rgb(255, 150, 40);
    pub const CYAN: Self = Self::rgb(64, 224, 240);
    pub const MAGENTA: Self = Self::rgb(240, 80, 220);
    pub const WHITE: Self = Self::rgb(255, 255, 255);
    pub const BLACK: Self = Self::rgb(0, 0, 0);

    /// An opaque color.
    pub const fn rgb(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b, a: 255 }
    }

    pub const fn rgba(r: u8, g: u8, b: u8, a: u8) -> Self {
        Self { r, g, b, a }
    }

    /// The same color at `a`.
    pub const fn with_alpha(self, a: u8) -> Self {
        Self { a, ..self }
    }
}

/// Whether a draw hides behind what the frame drew in front of it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Depth {
    /// Hidden behind the surfaces in front of it.
    #[default]
    Tested,
    /// Over everything, for what must read through a surface, such as a hit volume inside a
    /// character.
    OnTop,
}

/// How long a draw lasts (see the [module docs](self)).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum Lifetime {
    /// The next frame, or the fixed step it is drawn in.
    #[default]
    Frame,
    /// This many seconds on the clock the app gives [`Collector::collect`]; never zero or less
    /// through [`Style::for_seconds`].
    Seconds(f32),
}

/// A draw's color, depth and lifetime. A [`Color`] alone converts to one that is depth-tested and
/// lasts one frame.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Style {
    pub color: Color,
    pub depth: Depth,
    pub lifetime: Lifetime,
}

impl Style {
    /// `color`, depth-tested, for one frame.
    pub const fn new(color: Color) -> Self {
        Self { color, depth: Depth::Tested, lifetime: Lifetime::Frame }
    }

    /// Over everything ([`Depth::OnTop`]).
    pub const fn on_top(self) -> Self {
        Self { depth: Depth::OnTop, ..self }
    }

    /// For `seconds` on the clock the app gives [`Collector::collect`]. Zero, negative or not a
    /// number means one frame.
    pub const fn for_seconds(self, seconds: f32) -> Self {
        let lifetime = if seconds > 0.0 { Lifetime::Seconds(seconds) } else { Lifetime::Frame };
        Self { lifetime, ..self }
    }
}

impl From<Color> for Style {
    fn from(color: Color) -> Self {
        Self::new(color)
    }
}

/// Draws `shape`: every function below is this for its own shape, each with its own check, so that
/// a disabled call doesn't even build its arguments' `Shape`.
#[inline]
pub fn shape(shape: Shape, style: impl Into<Style>) {
    if enabled() {
        buffer::write_shape(shape, style.into());
    }
}

/// A line from `from` to `to`.
#[inline]
pub fn line(from: Vec3, to: Vec3, style: impl Into<Style>) {
    if enabled() {
        buffer::write_shape(Shape::Line { from, to }, style.into());
    }
}

/// An arrow from `from` to its head at `to`.
#[inline]
pub fn arrow(from: Vec3, to: Vec3, style: impl Into<Style>) {
    if enabled() {
        buffer::write_shape(Shape::Arrow { from, to }, style.into());
    }
}

/// An axis-aligned box between its corners `min` and `max`.
#[inline]
pub fn aabb(min: Vec3, max: Vec3, style: impl Into<Style>) {
    if enabled() {
        buffer::write_shape(Shape::Aabb { min, max }, style.into());
    }
}

/// A box around `center`, `half_extents` along its own axes, turned by `rotation`.
#[inline]
pub fn oriented_box(center: Vec3, half_extents: Vec3, rotation: Quat, style: impl Into<Style>) {
    if enabled() {
        buffer::write_shape(Shape::OrientedBox { center, half_extents, rotation }, style.into());
    }
}

/// A sphere: three great circles.
#[inline]
pub fn sphere(center: Vec3, radius: f32, style: impl Into<Style>) {
    if enabled() {
        buffer::write_shape(Shape::Sphere { center, radius }, style.into());
    }
}

/// A capsule around the segment from `from` to `to`.
#[inline]
pub fn capsule(from: Vec3, to: Vec3, radius: f32, style: impl Into<Style>) {
    if enabled() {
        buffer::write_shape(Shape::Capsule { from, to, radius }, style.into());
    }
}

/// A circle around `center`, in the plane facing `normal`.
#[inline]
pub fn circle(center: Vec3, normal: Vec3, radius: f32, style: impl Into<Style>) {
    if enabled() {
        buffer::write_shape(Shape::Circle { center, normal, radius }, style.into());
    }
}

/// A frame's axes at `position`, turned by `rotation`, `length` long: its X red, Y green and Z
/// blue, whatever the style's color, of which only the alpha counts.
#[inline]
pub fn axes(position: Vec3, rotation: Quat, length: f32, style: impl Into<Style>) {
    if enabled() {
        buffer::write_shape(Shape::Axes { position, rotation, length }, style.into());
    }
}

/// Text beside `position`, cut at [`LABEL_BYTES`] bytes. Formatted into a fixed buffer, so it
/// allocates nothing unless a value's own formatting does; it isn't formatted at all while
/// drawing is off. Labels always draw on top, whatever the style's depth, and show only where
/// the debug overlay is linked (`sapphire_debug`, dev builds).
#[inline]
pub fn label(position: Vec3, style: impl Into<Style>, text: fmt::Arguments<'_>) {
    if enabled() {
        buffer::write_label(position, style.into(), text);
    }
}
