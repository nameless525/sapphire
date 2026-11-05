//! [`TimelinePlayer`]: one playhead over a timeline, and what its tracks say there.

use std::sync::Arc;
use std::time::Duration;

use glam::{Mat3, Quat, Vec3};
use sapphire_core::Name;
use sapphire_look::PostParams;
use sapphire_time::{Stopwatch, TICK_HZ};
use sapphire_timeline::{
    CameraKey, CameraTrack, CookedCharacter, CookedCue, Ease, EyeRule, PlaceKey, PostParameter, ScalarKey,
    Timeline, TimelineClock,
};

/// Where a timeline's own space sits in the world: a cutscene's is the zone's own, and a cinematic's sits on
/// who cast it.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Origin {
    pub position: Vec3,
    /// About +Y, from +Z toward +X.
    pub yaw: f32,
}

impl Origin {
    fn point(&self, local: [f32; 3]) -> Vec3 {
        self.position + Quat::from_rotation_y(self.yaw) * Vec3::from(local)
    }
}

/// Where the simulation is, for a timeline on its steps: from `Simulation::timeline`, with the frame's
/// interpolation.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct StepClock {
    /// Steps the run has taken.
    pub taken: u32,
    /// How far this frame draws between the last step and the next, as poses do.
    /// Clamped to 0–1; a nonfinite value draws the previous step (0).
    pub alpha: f32,
}

/// How far along a player is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Playing {
    /// Its tracks are in charge, and if it's on [`TimelineClock::Real`] the simulation is held.
    Yes,
    /// It reached its end, and has let go of everything. The director's blend out goes on without it.
    Finished,
    /// As [`Self::Finished`], by a skip.
    Skipped,
}

/// The shot the camera track has at the playhead, in the world, in the terms of the director's takeover.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Shot {
    pub eye: Vec3,
    /// Turns +Z to where it looks and +Y to its up.
    pub rotation: Quat,
    /// The full vertical field of view, in radians.
    pub fov_y: f32,
    /// This frame crossed a [`Ease::Cut`] key: the renderer's history is no use to it, nor the eye's last
    /// place.
    pub cut: bool,
}

/// How a camera track blends and treats the eye, for the director's takeover terms.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CameraTerms {
    pub blend_in: f32,
    pub blend_out: f32,
    pub eye: EyeRule,
}

/// The line up at the playhead, as keys for `sapphire_loc`. It's never text, so a language change is
/// nothing to do with a timeline.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Subtitle<'a> {
    pub key: &'a str,
    pub speaker: Option<&'a str>,
}

/// Where a track has a character stand.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Placed {
    /// Its feet.
    pub position: Vec3,
    /// About +Y; 0 faces +Z.
    pub yaw: f32,
}

/// What a character's track says at the playhead: what goes in the combat slot of its `GraphInput`, as the
/// animation graph's own row, and where it stands. With no row, the slot is empty, which is how a character
/// is handed back: the graph's exit decay blends it out of the last row.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CharacterCue {
    /// The row's move name in its graph.
    pub row: Option<Name>,
    /// Which playing this is, of this timeline's for it: what makes two plays of one row a transition.
    pub serial: u32,
    /// How far into the row, in ticks of 1/60 s: the units the slot takes.
    pub position_ticks: f64,
    /// `None` if its track has no places, so it stands where it is.
    pub placed: Option<Placed>,
}

/// A cue the playhead reached since the last advance.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FiredCue {
    pub cue: Name,
    /// Where it happens.
    pub place: Vec3,
}

/// What a player has done, for the frame's stats (rule R9). No row of §3.2 budgets a sequencer: it costs
/// binary searches, and `sequencer_bench` has what they come to.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SequencerStats {
    pub advances: u64,
    /// Cues the playhead reached.
    pub cues_fired: u64,
    /// Cues a skip jumped, which are transient and never fire.
    pub cues_skipped: u64,
    pub last_advance: Duration,
    pub longest_advance: Duration,
}

/// Plays a [`Timeline`]'s presentation half. See the [crate docs](crate). Allocates only in [`Self::new`].
#[derive(Clone, Debug)]
pub struct TimelinePlayer {
    timeline: Arc<Timeline>,
    origin: Origin,
    /// Seconds: never goes back.
    playhead: f32,
    /// Where it was at the last advance's start.
    before: f32,
    state: Playing,
    /// The next cue the playhead hasn't reached, and the first of those the last advance fired.
    next_cue: usize,
    fired_from: usize,
    stats: SequencerStats,
}

impl TimelinePlayer {
    /// Ready at its first moment. The timeline's tracks were checked when it was read.
    #[must_use]
    pub fn new(timeline: Arc<Timeline>, origin: Origin) -> Self {
        Self {
            timeline,
            origin,
            playhead: 0.0,
            before: 0.0,
            state: Playing::Yes,
            next_cue: 0,
            fired_from: 0,
            stats: SequencerStats::default(),
        }
    }

    /// Moves the playhead for this frame: `real_dt` is the frame's unscaled seconds, 0 while paused, for a
    /// timeline on [`TimelineClock::Real`]. `steps` is the run's progress for one on
    /// [`TimelineClock::Steps`], `None` once it's over. So a time scale, or a stop, slows the world under a
    /// timeline and not the timeline itself.
    pub fn advance(&mut self, real_dt: f32, steps: Option<StepClock>) {
        profiling::scope!("TimelinePlayer::advance");
        let stopwatch = Stopwatch::start();
        self.before = self.playhead;
        self.fired_from = self.next_cue;
        if self.state == Playing::Yes {
            let length = self.length();
            let at = match self.presentation_clock() {
                // Past a frame's worth of nothing: a NaN or negative step moves nothing.
                TimelineClock::Real => self.playhead + if real_dt > 0.0 { real_dt } else { 0.0 },
                // What's drawn is between the last two steps, as poses are.
                TimelineClock::Steps => match steps {
                    Some(steps) => {
                        let alpha = if steps.alpha.is_finite() { steps.alpha.clamp(0.0, 1.0) } else { 0.0 };
                        (steps.taken as f32 - 1.0 + alpha) / TICK_HZ as f32
                    }
                    None => length,
                },
            };
            self.playhead = at.clamp(self.playhead, length);
            let cues = &self.timeline.presentation.cues;
            self.next_cue += cues[self.next_cue..].partition_point(|cue| cue.at <= self.playhead);
            self.stats.cues_fired += (self.next_cue - self.fired_from) as u64;
            if self.playhead >= length {
                self.state = Playing::Finished;
            }
        }
        let elapsed = stopwatch.elapsed();
        self.stats.advances += 1;
        self.stats.last_advance = elapsed;
        self.stats.longest_advance = self.stats.longest_advance.max(elapsed);
    }

    /// Lets go of everything at once, and says how far it had got, in milliseconds, for a replay to record.
    /// Cues it jumps never fire: they're transient, and there's nothing they could reach that's kept. A post
    /// offset drops to nothing, as a cut would. For a timeline on the steps, the app then runs the steps
    /// the simulation still holds; see `sapphire_gameplay`'s `timeline` module.
    pub fn skip(&mut self) -> u32 {
        let at_ms = (self.playhead * 1000.0) as u32;
        if self.state == Playing::Yes {
            let cues = self.timeline.presentation.cues.len();
            self.stats.cues_skipped += (cues - self.next_cue) as u64;
            self.next_cue = cues;
            self.fired_from = cues;
            self.playhead = self.length();
            self.before = self.playhead;
            self.state = Playing::Skipped;
        }
        at_ms
    }

    #[must_use]
    pub fn state(&self) -> Playing {
        self.state
    }

    /// Whether the app should run no fixed steps this frame: a cutscene, until it ends or is skipped.
    #[must_use]
    pub fn holds_simulation(&self) -> bool {
        self.state == Playing::Yes && self.presentation_clock() == TimelineClock::Real
    }

    /// The playhead, in seconds.
    #[must_use]
    pub fn playhead(&self) -> f32 {
        self.playhead
    }

    #[must_use]
    pub fn length(&self) -> f32 {
        self.timeline.presentation.length
    }

    fn presentation_clock(&self) -> TimelineClock {
        self.timeline.presentation.clock
    }

    /// How its camera track blends, if it has one.
    #[must_use]
    pub fn camera_terms(&self) -> Option<CameraTerms> {
        let camera = self.timeline.presentation.camera.as_ref()?;
        Some(CameraTerms { blend_in: camera.blend_in, blend_out: camera.blend_out, eye: camera.eye })
    }

    /// The shot at the playhead. `None` with no camera track, or once it's over: the director blends back.
    #[must_use]
    pub fn shot(&self) -> Option<Shot> {
        if self.state != Playing::Yes {
            return None;
        }
        let camera = self.timeline.presentation.camera.as_ref()?;
        let (eye, look_at, roll, fov) = sample_camera(camera, self.playhead);
        let eye = self.origin.point(eye);
        let look_at = self.origin.point(look_at);
        let from = camera.keys.partition_point(|key| key.at <= self.before);
        let to = camera.keys.partition_point(|key| key.at <= self.playhead);
        let cut = camera.keys[from..to].iter().any(|key| key.ease == Ease::Cut);
        Some(Shot { eye, rotation: looking(eye, look_at, roll), fov_y: fov.to_radians(), cut })
    }

    /// Adds the post tracks' offsets at the playhead to what the look resolved. Nothing here bakes a LUT, so
    /// it costs what writing four numbers does, and every track ends on 0, so handing back moves nothing.
    pub fn apply_post(&self, post: &mut PostParams) {
        if self.state != Playing::Yes {
            return;
        }
        for track in &self.timeline.presentation.post {
            let offset = sample_scalar(&track.keys, self.playhead);
            match track.parameter {
                PostParameter::Exposure => post.exposure += offset,
                PostParameter::Bloom => post.bloom.intensity = (post.bloom.intensity + offset).max(0.0),
                PostParameter::Vignette => {
                    post.vignette.intensity = (post.vignette.intensity + offset).max(0.0);
                }
                PostParameter::Diffusion => {
                    post.diffusion.intensity = (post.diffusion.intensity + offset).max(0.0);
                }
            }
        }
    }

    /// The line up at the playhead.
    #[must_use]
    pub fn subtitle(&self) -> Option<Subtitle<'_>> {
        if self.state != Playing::Yes {
            return None;
        }
        let lines = &self.timeline.presentation.subtitles;
        let index = lines.partition_point(|line| line.from <= self.playhead).checked_sub(1)?;
        let line = &lines[index];
        (self.playhead < line.to).then(|| Subtitle { key: &line.key, speaker: line.speaker.as_deref() })
    }

    /// The roles it has a part for, as their names' hashes.
    pub fn roles(&self) -> impl Iterator<Item = Name> + '_ {
        self.timeline.presentation.characters.iter().map(|character| Name::from_u64(character.role))
    }

    /// What `role`'s track says at the playhead, and `None` once there's no track to say it: the character's
    /// back with whoever had it.
    #[must_use]
    pub fn character(&self, role: Name) -> Option<CharacterCue> {
        if self.state != Playing::Yes {
            return None;
        }
        let characters = &self.timeline.presentation.characters;
        let character = characters.iter().find(|character| character.role == role.as_u64())?;
        Some(self.cue_of(character))
    }

    fn cue_of(&self, character: &CookedCharacter) -> CharacterCue {
        let at = self.playhead;
        let index = character.plays.partition_point(|play| play.from <= at);
        let playing = index
            .checked_sub(1)
            .map(|index| (index, &character.plays[index]))
            .filter(|(_, play)| at < play.to);
        let placed = (!character.places.is_empty()).then(|| {
            let (position, yaw) = sample_place(&character.places, at);
            Placed { position: self.origin.point(position), yaw: self.origin.yaw + yaw.to_radians() }
        });
        match playing {
            Some((index, play)) => CharacterCue {
                row: Some(Name::from_u64(play.row)),
                serial: index as u32 + 1,
                position_ticks: f64::from(at - play.from) * f64::from(TICK_HZ),
                placed,
            },
            None => CharacterCue { row: None, serial: 0, position_ticks: 0.0, placed },
        }
    }

    /// The cues the last advance reached. Each frame's are gone at the next, and a skip fires none.
    pub fn fired(&self) -> impl ExactSizeIterator<Item = FiredCue> + '_ {
        let cues: &[CookedCue] = &self.timeline.presentation.cues[self.fired_from..self.next_cue];
        cues.iter().map(|cue| FiredCue { cue: Name::from_u64(cue.cue), place: self.origin.point(cue.place) })
    }

    #[must_use]
    pub fn stats(&self) -> SequencerStats {
        self.stats
    }
}

/// The index of the last of `keys` at or before `at`: the first key holds before any.
fn key_before<K>(keys: &[K], at: f32, time: impl Fn(&K) -> f32) -> usize {
    keys.partition_point(|key| time(key) <= at).saturating_sub(1)
}

/// The keys around `at` and how far from the first to the second, eased: past the last, that key twice.
fn between<K>(keys: &[K], at: f32, time: impl Fn(&K) -> f32, ease: impl Fn(&K) -> Ease) -> (&K, &K, f32) {
    let index = key_before(keys, at, &time);
    let from = &keys[index];
    let Some(to) = keys.get(index + 1) else { return (from, from, 0.0) };
    let share = ((at - time(from)) / (time(to) - time(from))).clamp(0.0, 1.0);
    (from, to, ease(to).eased(share))
}

fn lerp(from: f32, to: f32, share: f32) -> f32 {
    from + (to - from) * share
}

fn lerp3(from: [f32; 3], to: [f32; 3], share: f32) -> [f32; 3] {
    [lerp(from[0], to[0], share), lerp(from[1], to[1], share), lerp(from[2], to[2], share)]
}

fn sample_camera(camera: &CameraTrack, at: f32) -> ([f32; 3], [f32; 3], f32, f32) {
    let (from, to, share): (&CameraKey, &CameraKey, f32) =
        between(&camera.keys, at, |key| key.at, |key| key.ease);
    (
        lerp3(from.eye, to.eye, share),
        lerp3(from.look_at, to.look_at, share),
        lerp(from.roll_degrees, to.roll_degrees, share),
        lerp(from.fov_degrees, to.fov_degrees, share),
    )
}

fn sample_scalar(keys: &[ScalarKey], at: f32) -> f32 {
    let (from, to, share) = between(keys, at, |key| key.at, |key| key.ease);
    lerp(from.value, to.value, share)
}

fn sample_place(keys: &[PlaceKey], at: f32) -> ([f32; 3], f32) {
    let (from, to, share) = between(keys, at, |key| key.at, |key| key.ease);
    (lerp3(from.position, to.position, share), lerp(from.yaw_degrees, to.yaw_degrees, share))
}

/// The rotation that turns +Z to `eye`'s way to `at`, with `roll` degrees about it, as a camera pose's does.
fn looking(eye: Vec3, at: Vec3, roll: f32) -> Quat {
    let forward = (at - eye).normalize_or(Vec3::Z);
    // Straight up or down, world up says nothing about the side: use the view's nearest.
    let up = if forward.y.abs() > 0.999 { Vec3::Z } else { Vec3::Y };
    let side = up.cross(forward).normalize();
    let upward = forward.cross(side);
    let unrolled = Quat::from_mat3(&Mat3::from_cols(side, upward, forward));
    (unrolled * Quat::from_rotation_z(roll.to_radians())).normalize()
}

#[cfg(test)]
mod tests {
    use sapphire_core::alloc::{self, CountingAllocator, PER_THREAD_COUNTS};
    use sapphire_timeline::TimelineDocument;

    use super::*;

    #[global_allocator]
    static GLOBAL: CountingAllocator<std::alloc::System> = CountingAllocator::new(std::alloc::System);

    const CUTSCENE: &str = include_str!("../../sapphire_timeline/test_content/cutscene.timeline.ron");
    const CINEMATIC: &str = include_str!("../../sapphire_timeline/test_content/cinematic.timeline.ron");
    const DT: f32 = 1.0 / 60.0;

    fn timeline(text: &str) -> Arc<Timeline> {
        Arc::new(TimelineDocument::from_ron(text).unwrap().cook().unwrap())
    }

    fn player() -> TimelinePlayer {
        TimelinePlayer::new(timeline(CUTSCENE), Origin::default())
    }

    fn at(seconds: f32) -> TimelinePlayer {
        let mut player = player();
        player.advance(seconds, None);
        player
    }

    fn close(a: Vec3, b: Vec3) -> bool {
        a.distance(b) < 1e-4
    }

    #[test]
    fn a_shot_runs_through_its_keys_and_holds_up_to_a_cut() {
        let shot = |seconds| at(seconds).shot().unwrap();
        assert!(close(shot(0.0).eye, Vec3::new(0.0, 1.6, -4.0)));
        assert!((shot(0.0).fov_y - 35_f32.to_radians()).abs() < 1e-5);
        // `Smooth`: at half-time, half-way.
        assert!(close(shot(1.25).eye, Vec3::new(0.75, 1.6, -3.5)));
        assert!(shot(0.25).eye.x < 0.15, "and slow away from the key: {}", shot(0.25).eye);
        assert!(close(shot(2.5).eye, Vec3::new(1.5, 1.6, -3.0)));
        // The key before holds until the cut's own time.
        assert!(close(shot(2.99).eye, Vec3::new(1.5, 1.6, -3.0)));
        assert!(close(shot(3.0).eye, Vec3::new(-2.0, 1.2, 2.0)));
        assert!(close(shot(5.9).eye, Vec3::new(-2.0, 1.2, 2.0)), "the last holds to the end");
    }

    #[test]
    fn a_shot_looks_where_it_says_with_its_up_up_and_its_roll() {
        let shot = at(0.0).shot().unwrap();
        let forward = shot.rotation * Vec3::Z;
        assert!(close(forward, (Vec3::new(0.0, 1.4, 0.0) - shot.eye).normalize()), "{forward}");
        assert!((shot.rotation * Vec3::Y).y > 0.99);
        // Right-handed: +X is the look's own left, turned from +Y to +Z's far side.
        assert!(close(shot.rotation * Vec3::X, (Vec3::Y.cross(forward)).normalize()));
        let rolled = at(3.0).shot().unwrap();
        let unrolled_side = Vec3::Y.cross(rolled.rotation * Vec3::Z).normalize();
        assert!((rolled.rotation * Vec3::X).dot(unrolled_side) < 0.999, "4 degrees of roll");
        // Straight down has no world up, and it's still a rotation.
        let down = looking(Vec3::Y, Vec3::ZERO, 0.0);
        assert!(close(down * Vec3::Z, Vec3::NEG_Y));
    }

    #[test]
    fn a_cut_is_marked_on_the_one_frame_that_crosses_it() {
        let mut player = player();
        player.advance(2.9, None);
        assert!(!player.shot().unwrap().cut);
        player.advance(0.2, None);
        assert!(player.shot().unwrap().cut);
        player.advance(0.1, None);
        assert!(!player.shot().unwrap().cut);
    }

    #[test]
    fn a_cut_is_retained_when_the_frame_also_crosses_a_smooth_key() {
        let mut document = TimelineDocument::from_ron(CUTSCENE).unwrap();
        let keys = &mut document.camera.as_mut().unwrap().keys;
        let mut smooth = *keys.last().unwrap();
        smooth.at = 3.5;
        smooth.ease = Ease::Smooth;
        keys.push(smooth);
        let mut cut = *keys.last().unwrap();
        cut.at = 4.0;
        cut.ease = Ease::Cut;
        keys.push(cut);
        let mut smooth = *keys.last().unwrap();
        smooth.at = 4.5;
        smooth.ease = Ease::Smooth;
        keys.push(smooth);
        let timeline = Arc::new(document.cook().unwrap());
        for delta in [0.1, 0.8, 1.8] {
            let mut player = TimelinePlayer::new(timeline.clone(), Origin::default());
            player.advance(2.9, None);
            assert!(!player.shot().unwrap().cut);
            player.advance(delta, None);
            assert!(player.shot().unwrap().cut);
            player.advance(0.0, None);
            assert!(!player.shot().unwrap().cut);
            player.advance(10.0, None);
            assert_eq!(player.shot(), None, "completion hands back the shot");
        }
    }

    #[test]
    fn invalid_step_interpolation_cannot_poison_the_playhead() {
        for (alpha, expected) in
            [(f32::NAN, 0.0), (f32::INFINITY, 0.0), (f32::NEG_INFINITY, 0.0), (-1.0, 0.0), (2.0, 1.0)]
        {
            let mut player = TimelinePlayer::new(timeline(CINEMATIC), Origin::default());
            player.advance(0.0, Some(StepClock { taken: 0, alpha }));
            assert_eq!(player.playhead(), 0.0);
            player.advance(0.0, Some(StepClock { taken: 31, alpha }));
            assert!((player.playhead() - (30.0 + expected) / 60.0).abs() < 1e-5);
            player.advance(0.0, Some(StepClock { taken: 32, alpha: 0.5 }));
            assert!((player.playhead() - 31.5 / 60.0).abs() < 1e-5);
        }
    }

    #[test]
    fn an_origin_places_the_shot_and_everything_else() {
        let origin = Origin { position: Vec3::new(10.0, 0.0, 0.0), yaw: std::f32::consts::FRAC_PI_2 };
        let mut player = TimelinePlayer::new(timeline(CUTSCENE), origin);
        player.advance(1.0, None);
        // +Z locally is +X in the world, a quarter turn on.
        let eye = player.shot().unwrap().eye;
        let plain = at(1.0).shot().unwrap().eye;
        assert!(close(eye, Vec3::new(10.0 + plain.z, plain.y, -plain.x)), "{eye} from {plain}");
        let forward = player.shot().unwrap().rotation * Vec3::Z;
        let plain_forward = at(1.0).shot().unwrap().rotation * Vec3::Z;
        assert!(close(forward, Vec3::new(plain_forward.z, plain_forward.y, -plain_forward.x)));
        let fired: Vec<FiredCue> = player.fired().collect();
        assert!(close(fired[0].place, Vec3::new(10.0, 2.0, 0.0)), "{fired:?}");
    }

    #[test]
    fn post_offsets_add_to_what_the_look_resolved_and_end_on_nothing() {
        let resolved = PostParams::default();
        let post = |seconds| {
            let mut post = resolved;
            at(seconds).apply_post(&mut post);
            post
        };
        assert!((post(1.0).exposure - (resolved.exposure - 0.5)).abs() < 1e-5);
        assert!((post(3.0).exposure - (resolved.exposure - 0.25)).abs() < 1e-5);
        assert!((post(0.0).vignette.intensity - (resolved.vignette.intensity + 0.25)).abs() < 1e-5);
        assert_eq!(post(6.0).exposure, resolved.exposure);
        assert_eq!(post(6.0).vignette.intensity, resolved.vignette.intensity);
        // A skip drops them at once, and a negative one can't take a look below nothing.
        let mut player = at(1.0);
        player.skip();
        let mut after = resolved;
        player.apply_post(&mut after);
        assert_eq!(after.exposure, resolved.exposure);
    }

    #[test]
    fn a_subtitle_is_a_key_for_its_time_and_never_after() {
        let first = Subtitle { key: "intro-first-line", speaker: Some("hero-name") };
        assert_eq!(at(0.5).subtitle(), None);
        assert_eq!(at(1.0).subtitle(), Some(first));
        assert_eq!(at(2.99).subtitle(), Some(first));
        assert_eq!(at(3.0).subtitle(), Some(Subtitle { key: "intro-second-line.short", speaker: None }));
        assert_eq!(at(5.0).subtitle(), None);
    }

    /// One clock: a subtitle and a cue written at one time come up on the same frame, whatever the frame rate.
    #[test]
    fn a_subtitle_and_a_cue_at_one_time_come_up_together() {
        for hz in [24.0, 30.0, 60.0, 144.0] {
            let mut player = player();
            let mut bell_on_screen = None;
            for frame in 0..(hz as u32 * 6) {
                player.advance(1.0 / hz, None);
                if player.fired().count() > 0 && bell_on_screen.is_none() {
                    bell_on_screen = Some((frame, player.subtitle().map(|line| line.key.to_owned())));
                }
            }
            let (_, line) = bell_on_screen.expect("it rang");
            assert_eq!(line.as_deref(), Some("intro-first-line"), "at {hz} Hz");
        }
    }

    #[test]
    fn a_characters_cue_is_its_row_serial_and_place() {
        let hero = Name::new("hero");
        let cue = |seconds| at(seconds).character(hero).unwrap();
        assert_eq!(cue(0.1).row, None);
        assert_eq!(cue(1.0).row, Some(Name::new("bow")));
        assert_eq!(cue(1.0).serial, 1);
        assert!((cue(1.0).position_ticks - 30.0).abs() < 1e-3);
        assert_eq!((cue(2.2).row, cue(2.2).serial), (None, 0), "between plays the slot's empty");
        assert_eq!((cue(3.0).row, cue(3.0).serial), (Some(Name::new("wave")), 2));
        let placed = cue(0.0).placed.unwrap();
        assert!(close(placed.position, Vec3::ZERO) && (placed.yaw - std::f32::consts::PI).abs() < 1e-5);
        let placed = cue(4.0).placed.unwrap();
        assert!(close(placed.position, Vec3::X));
        let villain = at(1.0).character(Name::new("villain")).unwrap();
        assert_eq!((villain.row, villain.placed), (Some(Name::new("stand")), None));
        assert_eq!(at(1.0).character(Name::new("nobody")), None);
        assert_eq!(at(1.0).roles().collect::<Vec<_>>(), [hero, Name::new("villain")]);
    }

    #[test]
    fn its_over_at_its_end_and_hands_everything_back() {
        let mut player = at(5.99);
        assert_eq!(player.state(), Playing::Yes);
        assert!(player.holds_simulation());
        player.advance(0.1, None);
        assert_eq!(player.state(), Playing::Finished);
        assert_eq!(player.playhead(), 6.0, "held to its length");
        assert!(!player.holds_simulation());
        assert_eq!(player.shot(), None);
        assert_eq!(player.subtitle(), None);
        assert_eq!(player.character(Name::new("hero")), None);
    }

    #[test]
    fn a_cue_fires_once_and_nothing_goes_backward() {
        let mut player = player();
        let mut fired = 0;
        for _ in 0..(60 * 7) {
            player.advance(DT, None);
            fired += player.fired().count();
        }
        assert_eq!(fired, 2);
        assert_eq!(player.stats().cues_fired, 2);
        assert_eq!(player.fired().count(), 0, "its last advance fired none");

        // Nonsense for a step doesn't move it, nor can a step go back.
        let mut player = at(1.0);
        for bad in [f32::NAN, -1.0, 0.0] {
            player.advance(bad, None);
            assert_eq!(player.playhead(), 1.0);
        }
    }

    #[test]
    fn a_skip_lets_go_at_once_and_drops_what_it_jumped() {
        let mut player = at(1.5);
        assert_eq!(player.fired().count(), 1);
        assert_eq!(player.skip(), 1500);
        assert_eq!(player.state(), Playing::Skipped);
        assert!(!player.holds_simulation());
        assert_eq!((player.shot(), player.subtitle()), (None, None));
        assert_eq!(player.fired().count(), 0, "not even what the frame had");
        assert_eq!((player.stats().cues_fired, player.stats().cues_skipped), (1, 1));
        player.advance(DT, None);
        assert_eq!(player.fired().count(), 0, "and nothing later");
        assert_eq!(player.state(), Playing::Skipped);
    }

    #[test]
    fn on_the_steps_it_follows_the_run_and_a_slow_world_doesnt_slow_it() {
        let mut player = TimelinePlayer::new(timeline(CINEMATIC), Origin::default());
        assert!(!player.holds_simulation(), "the simulation runs under it");
        player.advance(5.0, Some(StepClock { taken: 0, alpha: 0.5 }));
        assert_eq!(player.playhead(), 0.0, "the real clock's nothing to it, and it can't start early");
        player.advance(0.0, Some(StepClock { taken: 31, alpha: 0.5 }));
        assert!((player.playhead() - 30.5 / 60.0).abs() < 1e-5);
        player.advance(0.0, Some(StepClock { taken: 30, alpha: 0.0 }));
        assert!((player.playhead() - 30.5 / 60.0).abs() < 1e-5, "never back");
        player.advance(0.0, None);
        assert_eq!(player.state(), Playing::Finished);
    }

    #[test]
    fn a_frame_allocates_nothing() {
        if !PER_THREAD_COUNTS {
            return;
        }
        let mut player = player();
        let hero = Name::new("hero");
        let mut post = PostParams::default();
        let mut sink = 0.0;
        let ((), counts) = alloc::count_in_scope(|| {
            for _ in 0..(60 * 7) {
                player.advance(DT, None);
                sink += player.shot().map_or(0.0, |shot| shot.fov_y);
                player.apply_post(&mut post);
                sink += player.subtitle().map_or(0.0, |line| line.key.len() as f32);
                sink += player.character(hero).map_or(0.0, |cue| cue.position_ticks as f32);
                sink += player.fired().map(|cue| cue.place.x).sum::<f32>();
            }
        });
        assert!(sink.is_finite());
        assert_eq!(counts.allocations, 0);
    }
}
