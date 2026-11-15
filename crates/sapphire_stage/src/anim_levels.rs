//! Animation levels (M5 C1): how often a character far off or out of sight is posed, and what of its
//! pose runs when it is.
//!
//! - **Bands by distance.** A tier's [`AnimLevels`] says, band by band from the eye out, how many whole
//!   ticks of a character's own clock pass between its poses, and whether its IK and its springs run.
//!   The nearest band poses every frame, at the frame's fraction of a tick, exactly as a character with
//!   no levels is posed. Past it a character is posed on its interval only, on the frame its clock
//!   crosses a tick boundary its interval and place in the roster pick, and at that tick, whole: never a
//!   tick it hasn't reached, so nothing's invented, and never between ticks. Between its poses it holds
//!   the last ([`AnimPace::Hold`]): its transform goes on moving as the simulation says, its pose doesn't,
//!   and nothing's skinned again. A band's edge has a margin either side, so a character standing on it
//!   doesn't flicker between two.
//! - **Off screen** ([`LevelView::shows`]): a character no view draws holds whatever band it's in,
//!   stepping nothing, its springs asleep. A view is the camera's, widened by a margin for a turn
//!   between frames, or its shadow's, swept from it along the sun's direction as far as a shadow of
//!   its height falls. Back in sight it's posed that frame, at once.
//! - **IK and springs** go out in a far band as they do in a jump: the feet and the look let go over
//!   their own fades ([`IkDriver::set_resting`](crate::ik::IkDriver::set_resting)), and springs sleep, by
//!   the rule after the pace in [`SpringView`](crate::animate::SpringView)'s decision, and wake from the
//!   pose. Neither pops coming back.
//! - **Who is never thinned** is the caller's to say: the simulation's own pair, the one on field, and
//!   whoever a timeline or a cut-in is staging take [`AnimPace::FULL`] without asking.
//!
//! Nothing here reads or writes what the simulation reads: a level only decides when presentation poses
//! a character, never what the simulation steps or when.

use sapphire_math::{Frustum, Mat4, Sphere, Vec3};
use sapphire_render::CameraData;
use sapphire_render::quality::TierId;

use crate::animate::CHARACTER_REACH;
use crate::scene::Figure;

/// How many bands a tier has.
pub const BANDS: usize = 3;

/// The angle the camera may turn between the frame the levels read and the one that draws, as a
/// tangent: about 10°, past which a turn is a cut. A character that close to the frame's edge counts
/// as on screen.
const TURN_MARGIN: f32 = 0.18;

/// The farthest a character's shadow is followed along the sun, in meters, for a sun low in the sky.
const LONGEST_SHADOW: f32 = 30.0;

/// The lowest the sun is taken to stand for a shadow's length, as the sine of its height.
const LOWEST_SUN: f32 = 0.2;

/// One band of a tier's levels: from how far out it starts, how often a character in it is posed, and
/// what of its pose runs.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AnimBand {
    /// From the eye to the middle of the character's body, in meters, where this band starts.
    pub from: f32,
    /// Ticks of the character's clock between its poses: 1 poses it every frame.
    pub interval: u32,
    /// Whether its feet and look follow the ground and its target.
    pub ik: bool,
    /// Whether its hair and cloth run.
    pub springs: bool,
}

impl AnimBand {
    /// Every frame, everything: what a character with no levels gets.
    pub const NEAR: Self = Self { from: 0.0, interval: 1, ik: true, springs: true };
}

/// A tier's bands, nearest first (see the [module docs](self)).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AnimLevels {
    /// From the eye out: the first starts at 0, and each starts farther than the last by more than the
    /// margin.
    pub bands: [AnimBand; BANDS],
    /// How far past a band's edge a character goes before it changes band, either way, in meters: half
    /// on each side.
    pub margin: f32,
}

impl AnimLevels {
    /// Desktop-High: every frame inside 20 m, every other tick to 40, every fourth past that with its
    /// IK and springs out, where a character is a few dozen pixels tall.
    pub const DESKTOP_HIGH: Self = Self {
        bands: [
            AnimBand::NEAR,
            AnimBand { from: 20.0, interval: 2, ik: true, springs: true },
            AnimBand { from: 40.0, interval: 4, ik: false, springs: false },
        ],
        margin: 2.0,
    };

    /// Desktop-Low: as the survey suggested, halved past 12 m and quartered past 24.
    pub const DESKTOP_LOW: Self = Self {
        bands: [
            AnimBand::NEAR,
            AnimBand { from: 12.0, interval: 2, ik: true, springs: true },
            AnimBand { from: 24.0, interval: 4, ik: false, springs: false },
        ],
        margin: 1.0,
    };

    /// Mobile: halved past 10 m with the feet and look out, quartered past 20 with the springs out too.
    pub const MOBILE: Self = Self {
        bands: [
            AnimBand::NEAR,
            AnimBand { from: 10.0, interval: 2, ik: false, springs: true },
            AnimBand { from: 20.0, interval: 4, ik: false, springs: false },
        ],
        margin: 1.0,
    };

    /// The band for a character `distance` from the eye that was in `current`: past each edge by half
    /// the margin to go out, inside it by half the margin to come back.
    fn band_at(&self, distance: f32, current: usize) -> usize {
        let half = self.margin * 0.5;
        (1..BANDS)
            .filter(|&band| {
                let edge = self.bands[band].from + if current < band { half } else { -half };
                distance >= edge
            })
            .count()
    }
}

/// How a tier thins animation.
pub fn anim_levels(tier: TierId) -> AnimLevels {
    match tier {
        TierId::DesktopHigh => AnimLevels::DESKTOP_HIGH,
        TierId::DesktopLow => AnimLevels::DESKTOP_LOW,
        TierId::MobileHigh | TierId::MobileLow => AnimLevels::MOBILE,
    }
}

/// What a character's pose runs on this frame: its band, and what that band says.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AnimLevel {
    /// Its band, 0 the nearest.
    pub band: u8,
    pub interval: u32,
    pub ik: bool,
    pub springs: bool,
}

impl AnimLevel {
    /// Every frame, everything.
    pub const FULL: Self = Self { band: 0, interval: 1, ik: true, springs: true };

    fn of(band: usize, settings: &AnimBand) -> Self {
        Self {
            band: band as u8,
            interval: settings.interval.max(1),
            ik: settings.ik,
            springs: settings.springs,
        }
    }

    /// Whether anything of the character's pose is thinned at this level.
    pub fn is_thinned(self) -> bool {
        self.interval > 1 || !self.ik || !self.springs
    }
}

impl Default for AnimLevel {
    fn default() -> Self {
        Self::FULL
    }
}

/// Whether a character is posed this frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AnimPace {
    /// Posed, at this level: every frame at the frame's fraction of a tick when its interval is 1, else
    /// at its clock's latest whole tick.
    Update(AnimLevel),
    /// Holding its last pose, touching nothing: off screen, or between the poses of its band.
    Hold { off_screen: bool },
}

impl AnimPace {
    pub const FULL: Self = Self::Update(AnimLevel::FULL);

    /// Whether it's posed between ticks, at the frame's fraction, as a character with no levels is.
    pub fn every_frame(self) -> bool {
        matches!(self, Self::Update(level) if level.interval <= 1)
    }
}

/// Where a character's clock stands, for its levels: the whole ticks it has reached, its place in the
/// roster, which spreads a band's characters over its interval, and whether it was just stood somewhere
/// new.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LevelClock {
    pub tick: u64,
    pub place: u32,
    pub cut: bool,
}

/// What a character's levels remember between frames. It lives with its driver.
#[derive(Clone, Copy, Debug, Default)]
pub struct LevelState {
    band: u8,
    /// The tick it was last posed at: `None` before the first, and after a frame posed without levels.
    posed: Option<u64>,
    /// Whether it was held off screen last frame.
    off_screen: bool,
    /// The level it was last posed at, which its springs and IK keep to until the next.
    current: AnimLevel,
}

impl LevelState {
    /// The level it was last posed at.
    pub fn current(&self) -> AnimLevel {
        self.current
    }

    /// The band it was last found in, on screen or held off it: 0 the nearest (M5 C11's counts).
    pub fn band(&self) -> u8 {
        self.band
    }

    /// Posed this frame with no levels at all: the next frame with levels poses it.
    pub fn pose_full(&mut self) {
        *self = Self::default();
    }
}

/// What decides each character's level this frame: the view, the sun its shadow falls from, and the
/// tier's bands. Plain data, so jobs can share it.
#[derive(Clone, Copy, Debug)]
pub struct LevelView {
    frustum: Frustum,
    eye: Vec3,
    /// Where the sun's light travels, normalized.
    sun: Vec3,
    levels: AnimLevels,
    on: bool,
}

impl LevelView {
    /// For `camera`, with the sun's light travelling along `sun`, at a tier's `levels`, or with every
    /// character posed every frame if `on` is false.
    pub fn new(camera: &CameraData, sun: Vec3, levels: AnimLevels, on: bool) -> Self {
        Self {
            frustum: Frustum::from_view_proj(&(camera.proj * camera.view)),
            eye: camera.position,
            sun: sun.try_normalize().unwrap_or(Vec3::NEG_Y),
            levels,
            on,
        }
    }

    /// Whether levels thin anything this frame.
    pub fn is_on(&self) -> bool {
        self.on
    }

    /// Whether a character whose middle is at `center` and whose head stands `height` over its feet shows
    /// in a view: the camera's, with [`TURN_MARGIN`] for a turn, or its shadow's, swept from it along the
    /// sun as far as a shadow its height casts falls.
    pub fn shows(&self, center: Vec3, height: f32) -> bool {
        let reach = CHARACTER_REACH + self.eye.distance(center) * TURN_MARGIN;
        if self.frustum.intersects_sphere(&Sphere::new(center, reach)) {
            return true;
        }
        let length = ((height + CHARACTER_REACH) / (-self.sun.y).max(LOWEST_SUN)).min(LONGEST_SHADOW);
        let tip = center + self.sun * length;
        // Conservative, as the sphere's test is: out only when both ends are out past one plane.
        self.frustum
            .planes()
            .iter()
            .all(|plane| plane.signed_distance(center).max(plane.signed_distance(tip)) >= -CHARACTER_REACH)
    }

    /// Whether the character `state` remembers, standing at `place` as `figure` says with its clock at
    /// `clock`, is posed this frame, and at what level. See the [module docs](self).
    pub fn pace(&self, state: &mut LevelState, place: Mat4, figure: &Figure, clock: LevelClock) -> AnimPace {
        if !self.on {
            state.pose_full();
            return AnimPace::FULL;
        }
        let head = place.transform_point3(figure.head);
        let feet = place.transform_point3(figure.feet);
        let center = (head + feet) * 0.5;
        if !self.shows(center, head.distance(feet)) {
            state.off_screen = true;
            return AnimPace::Hold { off_screen: true };
        }
        let band = self.levels.band_at(self.eye.distance(center), usize::from(state.band));
        let level = AnimLevel::of(band, &self.levels.bands[band]);
        state.band = level.band;
        let interval = u64::from(level.interval);
        let phase = u64::from(clock.place) % interval;
        let due = clock.cut
            || state.off_screen
            || interval == 1
            || state.posed.is_none_or(|posed| {
                clock.tick < posed || (clock.tick + phase) / interval != (posed + phase) / interval
            });
        state.off_screen = false;
        if !due {
            return AnimPace::Hold { off_screen: false };
        }
        state.posed = Some(clock.tick);
        state.current = level;
        AnimPace::Update(level)
    }
}

#[cfg(test)]
mod tests {
    use sapphire_math::Quat;

    use super::*;

    /// A camera at the origin looking down −Z, the sun straight down.
    fn view(levels: AnimLevels) -> LevelView {
        let camera = CameraData::look_at(Vec3::ZERO, Vec3::NEG_Z, Vec3::Y, 1.0, 16.0 / 9.0, 0.1);
        LevelView::new(&camera, Vec3::NEG_Y, levels, true)
    }

    fn figure() -> Figure {
        Figure { head: Vec3::new(0.0, 1.6, 0.0), feet: Vec3::ZERO, face: Quat::IDENTITY, ..Figure::MANNEQUIN }
    }

    /// Standing `distance` ahead of the camera, its middle on the camera's line.
    fn ahead(distance: f32) -> Mat4 {
        Mat4::from_translation(Vec3::new(0.0, -0.8, -distance))
    }

    fn clock(tick: u64, place: u32) -> LevelClock {
        LevelClock { tick, place, cut: false }
    }

    #[test]
    fn bands_change_past_a_margin_either_way() {
        let levels = AnimLevels::DESKTOP_LOW;
        assert_eq!(levels.band_at(11.0, 0), 0);
        assert_eq!(levels.band_at(12.4, 0), 0, "inside the margin going out");
        assert_eq!(levels.band_at(12.6, 0), 1);
        assert_eq!(levels.band_at(11.6, 1), 1, "inside the margin coming back");
        assert_eq!(levels.band_at(11.4, 1), 0);
        assert_eq!(levels.band_at(30.0, 0), 2);
        assert_eq!(levels.band_at(23.6, 2), 2);
        for tier in [AnimLevels::DESKTOP_HIGH, AnimLevels::DESKTOP_LOW, AnimLevels::MOBILE] {
            assert_eq!(tier.bands[0], AnimBand::NEAR, "the nearest band poses as before");
            for pair in tier.bands.windows(2) {
                assert!(pair[1].from - pair[0].from > tier.margin, "{tier:?}");
                assert!(pair[1].interval >= pair[0].interval, "{tier:?}");
            }
        }
    }

    /// Each band poses on its interval only, on the frames its clock crosses a boundary, whatever the
    /// frame rate; the nearest every frame, between ticks too.
    #[test]
    fn each_band_poses_on_its_interval_on_tick_boundaries() {
        let view = view(AnimLevels::DESKTOP_LOW);
        for (distance, interval) in [(5.0, 1), (18.0, 2), (30.0, 4)] {
            for place in 0..4 {
                // Two frames a tick, so half the frames cross no boundary at all.
                let mut state = LevelState::default();
                let mut posed = Vec::new();
                for frame in 0..48_u64 {
                    let tick = frame / 2;
                    let pace = view.pace(&mut state, ahead(distance), &figure(), clock(tick, place));
                    if let AnimPace::Update(level) = pace {
                        assert_eq!(level.interval, interval);
                        posed.push((frame, tick));
                    }
                }
                if interval == 1 {
                    assert_eq!(posed.len(), 48, "every frame near");
                    continue;
                }
                // The first frame poses; after it, once each interval, on the frame that reached the tick.
                let every = u64::from(interval);
                for &(frame, tick) in &posed[1..] {
                    assert_eq!(frame % 2, 0, "{distance} m: posed at frame {frame}, between ticks");
                    assert_eq!((tick + u64::from(place)) % every, 0, "{distance} m: posed at tick {tick}");
                }
                assert_eq!(posed.len() as u64, 1 + (24 - 1 + u64::from(place) % every) / every);
            }
        }
    }

    /// Off screen, a character holds; back on screen it's posed that frame, whatever its interval.
    #[test]
    fn off_screen_holds_and_back_on_screen_poses_at_once() {
        let view = view(AnimLevels::DESKTOP_LOW);
        let behind = Mat4::from_translation(Vec3::new(0.0, -0.8, 30.0));
        let mut state = LevelState::default();
        assert!(matches!(view.pace(&mut state, ahead(30.0), &figure(), clock(0, 0)), AnimPace::Update(_)));
        for tick in 1..20 {
            assert_eq!(
                view.pace(&mut state, behind, &figure(), clock(tick, 0)),
                AnimPace::Hold { off_screen: true }
            );
        }
        // Tick 21 is no boundary of a fourth for place 0, but it's back in sight.
        assert!(matches!(view.pace(&mut state, ahead(30.0), &figure(), clock(21, 0)), AnimPace::Update(_)));
        assert_eq!(
            view.pace(&mut state, ahead(30.0), &figure(), clock(22, 0)),
            AnimPace::Hold { off_screen: false }
        );
    }

    /// A character out of the camera's frame whose shadow falls into it shows; one whose shadow falls
    /// away doesn't. And levels off pose everyone every frame.
    #[test]
    fn a_shadow_in_the_frame_shows_its_character() {
        let eye = Vec3::new(0.0, 2.0, 0.0);
        let camera = CameraData::look_at(eye, eye + Vec3::NEG_Z, Vec3::Y, 1.0, 16.0 / 9.0, 0.1);
        // A low sun, its light travelling toward −Z: a shadow behind the camera reaches in front of it.
        let low = Vec3::new(0.0, -0.15, -1.0);
        let view = LevelView::new(&camera, low, AnimLevels::DESKTOP_LOW, true);
        let just_behind = Vec3::new(0.0, 0.8, 6.0);
        assert!(view.shows(just_behind, 1.6));
        let away = LevelView::new(&camera, Vec3::new(0.0, -0.15, 1.0), AnimLevels::DESKTOP_LOW, true);
        assert!(!away.shows(just_behind, 1.6));

        let off = LevelView::new(&camera, low, AnimLevels::DESKTOP_LOW, false);
        let mut state = LevelState::default();
        let far_behind = Mat4::from_translation(Vec3::new(0.0, 0.0, 50.0));
        for tick in 0..8 {
            assert_eq!(off.pace(&mut state, far_behind, &figure(), clock(tick, 0)), AnimPace::FULL);
        }
    }
}
