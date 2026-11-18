//! [`TimeOfDay`]: the world's time-of-day state, resolved into a look once per frame.

use std::sync::Arc;
use std::time::Duration;

use bevy_ecs::prelude::{Res, ResMut, Resource};
use sapphire_ecs::WorldClock;
use sapphire_math::Vec3;
use sapphire_render::{CameraData, CharacterLighting, FramePacket, MAX_PROBE_KEYS, ProbeKeys};
use sapphire_time::{LocalClock, ScaledClock, Stopwatch};
use sapphire_tod::weather::{WeatherBlend, WeatherDocument};
use sapphire_tod::{
    DAY_HOURS, KeyedWeather, Segment, TodError, TodGroups, TodLook, TodTimeline, celestial_sphere, mix,
};

use super::clock::TodClock;
use super::cutscene::TodCutscene;
use super::volume::{TodVolume, TodVolumeId};
use crate::light_rig::{LightRig, ResolveRig};

/// Time of day for one world (an ECS resource): the base [`TodTimeline`], its clock, the zone
/// volumes in the world and a playing cutscene track, resolved each frame into a [`TodFrame`]
/// the app writes into the frame packet (see the [module docs](super)).
#[derive(Resource, Debug)]
pub struct TimeOfDay {
    timeline: TodTimeline,
    /// Counts timeline changes, so views of it (the overlay's day strip) know to refresh.
    revision: u64,
    clock: TodClock,
    /// In application order: by priority, then by when they were added.
    volumes: Vec<(TodVolumeId, TodVolume)>,
    next_volume: u32,
    cutscene: Option<Playback>,
    /// Seconds of world time the last fixed step took: how far a playing cutscene moved.
    world_step: f64,
    /// The weather's states (M6 W8a) and the blend between them the weather system chose for this
    /// frame, if any, which lie over the base timeline under the volumes.
    weather: Option<Arc<WeatherDocument>>,
    weather_blend: Option<WeatherBlend>,
    /// A cutscene's weather state and weight, laid over the world's blend.
    weather_shot: Option<(usize, f32)>,
    /// Whether the weather system's accumulated shares stand for the day's keyed weather (M6 Y2): a weather
    /// plan is live, so the base timeline's and the states' keyed weather give way to them.
    weather_shares: bool,
    frame: TodFrame,
    /// The hours whose looks the last resolve blended, with their weights: the clock's, and a
    /// playing cutscene's own hour by the track's weight (see [`Self::probe_keys`]).
    probe_hours: [(f32, f32); 2],
    stats: TodStats,
}

/// A cutscene track being played, and how far into the shot it is.
#[derive(Debug)]
struct Playback {
    track: Arc<TodCutscene>,
    seconds: f64,
}

/// Time of day resolved for one frame.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TodFrame {
    /// The clock's hour, `0 ≤ hours < 24`. A cutscene that sets its own time shows that hour's
    /// look while the clock keeps running underneath.
    pub hours: f32,
    pub look: TodLook,
    /// A playing cutscene's light rig, while its weight is above 0.
    pub shot_rig: Option<ShotRig>,
}

/// A cutscene's light rig (M1 C2) and the weight it lights characters with: the track's eased
/// weight, so the shot's rig blends in and out with its time-of-day values.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ShotRig {
    pub rig: LightRig,
    pub weight: f32,
}

impl TodFrame {
    /// Writes the look into `packet`'s scene constants: the key light, the sun, moon and
    /// celestial sphere, the sky and its layers, ambient, fog, shadow tint, character key (as it
    /// reaches characters, [`TodLook::character_key`]), day/night factor, emission scale and
    /// post-processing.
    pub fn write(&self, packet: &mut FramePacket) {
        let look = &self.look;
        packet.sun = look.key_light();
        packet.celestial = look.celestial();
        packet.sky = look.sky;
        packet.sky_layers = look.sky_layers();
        packet.ambient = look.ambient;
        packet.fog = look.fog;
        packet.shadow_tint = look.shadow_tint;
        packet.character_light = look.character_key();
        packet.day_night = look.day_night;
        packet.emission_scale = look.emission;
        packet.post = look.post();
    }

    /// The lighting of a character lit by `rig`, seen by `camera`, with its head at `head`
    /// (world space): the preset resolved under this frame's key light and character key
    /// ([`TodLook::key_light`], [`TodLook::character_key`], [`ResolveRig::resolve`]), with a
    /// playing cutscene's rig over it by the track's weight (the key's direction along the great
    /// circle, colors in OkLab), its shadow side in this frame's shadow hue (M1 C8). Allocates
    /// nothing.
    pub fn character_lighting(&self, rig: &LightRig, camera: &CameraData, head: Vec3) -> CharacterLighting {
        let key_light = self.look.key_light();
        let character_light = self.look.character_key();
        let own = rig.resolve(&key_light, &character_light, camera, head);
        let lighting = match &self.shot_rig {
            Some(shot) => {
                let shot_lighting = shot.rig.resolve(&key_light, &character_light, camera, head);
                mix(&own, &shot_lighting, shot.weight)
            }
            None => own,
        };
        CharacterLighting { shade: self.look.character_shade, ..lighting }
    }
}

/// What time of day did in its last resolve (rule R9).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct TodStats {
    /// CPU time of the last [`TimeOfDay::resolve`]. The budget is 0.05 ms (M1 plan, W1).
    pub resolve_time: Duration,
    /// Where the base timeline was: its keys and the blend between them.
    pub segment: Segment,
    pub volumes: u32,
    /// Volumes with a weight at the probe.
    pub active_volumes: u32,
    /// The playing cutscene track's weight, 0 without one.
    pub cutscene_weight: f32,
    /// The share of the picture whose grade didn't fit post's two LUTs (see
    /// [`GradeMix`](super::GradeMix)).
    pub dropped_grade_weight: f32,
    pub resolves: u64,
}

impl TimeOfDay {
    /// Time of day over the base `timeline`, which must be complete
    /// ([`TodTimeline::is_complete`]), on `clock` (issued by the world clock:
    /// [`ScaledClock::new_local_clock`]) at `hours`, with no cycle running.
    pub fn new(timeline: TodTimeline, clock: LocalClock, hours: f32) -> Result<Self, TodError> {
        check_complete(&timeline)?;
        let clock = TodClock::new(clock, hours);
        let hours = clock.hours(1.0);
        let frame = TodFrame { hours, look: timeline.evaluate(hours), shot_rig: None };
        Ok(Self {
            timeline,
            revision: 0,
            clock,
            volumes: Vec::new(),
            next_volume: 0,
            cutscene: None,
            world_step: 0.0,
            weather: None,
            weather_blend: None,
            weather_shot: None,
            weather_shares: false,
            frame,
            probe_hours: [(hours, 1.0), (0.0, 0.0)],
            stats: TodStats::default(),
        })
    }

    pub fn timeline(&self) -> &TodTimeline {
        &self.timeline
    }

    /// Replaces the base timeline (hot reload, live link), which must be complete.
    pub fn set_timeline(&mut self, timeline: TodTimeline) -> Result<(), TodError> {
        check_complete(&timeline)?;
        self.timeline = timeline;
        self.revision += 1;
        // The last resolve's segment named the old timeline's keys.
        self.stats.segment = Segment::default();
        Ok(())
    }

    /// Goes up by one whenever the base timeline is replaced.
    pub fn revision(&self) -> u64 {
        self.revision
    }

    pub fn clock(&self) -> &TodClock {
        &self.clock
    }

    /// The clock, to set the hour, run or stop the cycle, or pause it.
    pub fn clock_mut(&mut self) -> &mut TodClock {
        &mut self.clock
    }

    /// Adds a zone's override volume.
    pub fn add_volume(&mut self, volume: TodVolume) -> TodVolumeId {
        let id = TodVolumeId(self.next_volume);
        self.next_volume += 1;
        let at = self.volumes.partition_point(|(_, other)| other.priority <= volume.priority);
        self.volumes.insert(at, (id, volume));
        id
    }

    /// Removes a volume, returning it, or `None` if it was already removed.
    pub fn remove_volume(&mut self, id: TodVolumeId) -> Option<TodVolume> {
        let at = self.volumes.iter().position(|(other, _)| *other == id)?;
        Some(self.volumes.remove(at).1)
    }

    /// The volumes, in the order they apply.
    pub fn volumes(&self) -> impl ExactSizeIterator<Item = (TodVolumeId, &TodVolume)> {
        self.volumes.iter().map(|(id, volume)| (*id, volume))
    }

    /// Starts `track` at the beginning of its shot, replacing any playing track. It runs on the
    /// world's clock, not time of day's (so the overlay's pause of the cycle doesn't hold it),
    /// and ends by itself after its duration. A sequencer that drives it instead seeks it after
    /// each step's [`Self::tick`].
    pub fn play_cutscene(&mut self, track: Arc<TodCutscene>) {
        self.cutscene = Some(Playback { track, seconds: 0.0 });
    }

    /// Moves the playing track to `seconds` into its shot.
    pub fn seek_cutscene(&mut self, seconds: f32) {
        if let Some(playback) = &mut self.cutscene {
            playback.seconds = f64::from(seconds.max(0.0));
        }
    }

    pub fn stop_cutscene(&mut self) {
        self.cutscene = None;
    }

    /// The playing track and the seconds into its shot, as of the last fixed step.
    pub fn cutscene(&self) -> Option<(&TodCutscene, f32)> {
        self.cutscene.as_ref().map(|playback| (&*playback.track, playback.seconds as f32))
    }

    /// The playing track's weather state, by name, and the track's weight, `alpha` of the way through the
    /// latest step as [`Self::resolve`] reads it: what the weather system lays over the world's
    /// (`crate::weather::WeatherSystem`). `None` without a track, or a track that sets no weather.
    pub fn cutscene_weather(&self, alpha: f32) -> Option<(&str, f32)> {
        let playback = self.cutscene.as_ref()?;
        let state = playback.track.weather()?;
        Some((state, playback.track.weight(self.cutscene_seconds(playback, alpha))))
    }

    /// Sets the weather's states (M6 W8a): the document whose states' looks [`Self::set_weather_blend`]
    /// blends. `None` takes the weather away.
    pub fn set_weather(&mut self, document: Option<Arc<WeatherDocument>>) {
        self.weather = document;
    }

    /// The weather's states and how they blend this frame, over the base timeline and under the volumes
    /// (see [`Self::resolve`]): the weather system's choice. `None` shows none of them.
    pub fn set_weather_blend(&mut self, blend: Option<WeatherBlend>) {
        self.weather_blend = blend;
    }

    /// A cutscene's weather state and its weight, laid over [`Self::set_weather_blend`]'s whole blend: the
    /// weather system's (`crate::weather::WeatherSystem::shot`). `None` lays none.
    pub fn set_weather_shot(&mut self, shot: Option<(usize, f32)>) {
        self.weather_shot = shot;
    }

    /// Whether the weather's accumulated shares stand for the day's keyed weather (M6 Y2, m6-plan §3): with
    /// `true`, a look keys no weather out of the base timeline or the weather's states, so the shares lie over
    /// them, and only the volumes' and a cutscene track's keyed weather lie over the shares. The weather
    /// system's ([`crate::weather::WeatherSystem::write`]): `true` while a weather plan is live; a stage whose
    /// zone named no plan, the stage's own given back, sets it `false` after.
    pub fn set_weather_shares(&mut self, shares: bool) {
        self.weather_shares = shares;
    }

    /// The weather document [`Self::set_weather`] set, if any.
    pub fn weather(&self) -> Option<&Arc<WeatherDocument>> {
        self.weather.as_ref()
    }

    /// Advances one fixed step: the clock, and the playing cutscene track by the step's world
    /// time. Allocates nothing (rule R6).
    pub fn tick(&mut self, world: &ScaledClock) {
        self.clock.tick(world);
        self.world_step = f64::from(world.last_delta().as_secs_f32());
        if let Some(playback) = &mut self.cutscene {
            playback.seconds += self.world_step;
            if playback.seconds > f64::from(playback.track.duration()) {
                self.cutscene = None;
            }
        }
    }

    /// Resolves the look for a frame `alpha` of the way from the fixed step before the latest to the
    /// latest, where transforms draw, seen from `probe` (the camera, or the player), which zone volumes
    /// weigh themselves by:
    ///
    /// 1. the base timeline at the clock's hour;
    /// 2. the weather's states as [`Self::set_weather_blend`] blends them (M6 W8a), at the same hour,
    ///    over it ([`WeatherDocument::apply_looks`]); with [`Self::set_weather_shares`], the keyed weather
    ///    of these two steps cleared, the accumulated shares standing for it (M6 Y2);
    /// 3. each zone volume's timeline at the same hour, over them, by the volume's weight at the
    ///    probe, in priority order;
    /// 4. the playing cutscene track, by its weight: a crossfade to steps 1 to 3 at the shot's
    ///    hour, if it sets one, then its value keys over the result, and its light rig for
    ///    [`TodFrame::character_lighting`]. A track's weather state reaches step 2 through the weather
    ///    system ([`Self::cutscene_weather`]).
    ///
    /// The tool's own weather, and the tool's look, are the app's, over all of these.
    ///
    /// Allocates nothing, and costs under a microsecond (see the [module docs](super)).
    pub fn resolve(&mut self, probe: Vec3, alpha: f32) -> &TodFrame {
        profiling::scope!("TimeOfDay::resolve");
        let stopwatch = Stopwatch::start();
        let hours = self.clock.hours(alpha);
        let (mut look, active_volumes) = self.look_at(hours, probe);
        let mut cutscene_weight = 0.0;
        let mut shot_rig = None;
        let mut probe_hours = [(hours, 1.0), (0.0, 0.0)];
        if let Some(playback) = &self.cutscene {
            let seconds = self.cutscene_seconds(playback, alpha);
            let track = &playback.track;
            cutscene_weight = track.weight(seconds);
            if cutscene_weight > 0.0 {
                if let Some(shot_hours) = track.hours(seconds) {
                    let (shot, _) = self.look_at(shot_hours, probe);
                    look = mix(&look, &shot, cutscene_weight);
                    probe_hours = [(hours, 1.0 - cutscene_weight), (shot_hours, cutscene_weight)];
                }
                track.apply_values(&mut look, seconds, cutscene_weight);
                shot_rig = track.light_rig().map(|rig| ShotRig { rig: *rig, weight: cutscene_weight });
            }
        }
        self.frame = TodFrame { hours, look, shot_rig };
        self.probe_hours = probe_hours;
        self.stats = TodStats {
            resolve_time: stopwatch.elapsed(),
            segment: self.timeline.segment(hours),
            volumes: self.volumes.len() as u32,
            active_volumes,
            cutscene_weight,
            dropped_grade_weight: look.grades.dropped,
            resolves: self.stats.resolves + 1,
        };
        &self.frame
    }

    /// The last [`Self::resolve`]'s frame (before the first, the base timeline at the start
    /// hour).
    pub fn frame(&self) -> &TodFrame {
        &self.frame
    }

    pub fn stats(&self) -> TodStats {
        self.stats
    }

    /// Which of an ambient probe grid's baked keys the last resolve's bounce light blends, and by
    /// how much (rule R4: the renderer gets the keys and weights alone,
    /// [`FramePacket::probe_keys`]). `baked` is the hours the grid was baked at, ascending (the
    /// grid asset's key hours). For the clock's hour, and a playing cutscene's own hour by the
    /// track's weight, it takes the two baked keys the hour lies between: by the base timeline's
    /// own eased weight when the grid was baked at that timeline's keys (the cooker bakes every
    /// key), so the bounce moves with the rest of the look, and linearly by the hour otherwise.
    /// Zone volumes don't change it: the bake lit the base timeline. Allocates nothing.
    pub fn probe_keys(&self, baked: &[f32]) -> ProbeKeys {
        let mut keys = ProbeKeys::default();
        if baked.is_empty() {
            return keys;
        }
        let mut used = 0;
        for (hours, weight) in self.probe_hours {
            if weight <= 0.0 || weight.is_nan() {
                continue;
            }
            for (key, share) in self.baked_segment(hours, baked) {
                let blended = weight * share;
                if blended <= 0.0 {
                    continue;
                }
                match keys.keys[..used].iter().position(|&other| other == key) {
                    Some(slot) => keys.weights[slot] += blended,
                    None if used < MAX_PROBE_KEYS => {
                        keys.keys[used] = key;
                        keys.weights[used] = blended;
                        used += 1;
                    }
                    None => {}
                }
            }
        }
        keys
    }

    /// The two baked keys `hours` lies between, with their weights (see [`Self::probe_keys`]).
    fn baked_segment(&self, hours: f32, baked: &[f32]) -> [(u32, f32); 2] {
        let timeline = self.timeline.keys();
        let at_timeline_keys = timeline.len() == baked.len()
            && timeline.iter().zip(baked).all(|(key, &hours)| (key.time - hours).abs() < 1e-3);
        if at_timeline_keys {
            let Segment { from, to, weight } = self.timeline.segment(hours);
            return [(from as u32, 1.0 - weight), (to as u32, weight)];
        }
        let wrapped = if hours.is_finite() { hours.rem_euclid(DAY_HOURS) } else { 0.0 };
        let wrapped = if wrapped >= DAY_HOURS { 0.0 } else { wrapped };
        let last = baked.len() - 1;
        let next = baked.partition_point(|&key| key <= wrapped);
        let from = if next == 0 { last } else { next - 1 };
        let to = if next > last { 0 } else { next };
        let span = (baked[to] - baked[from]).rem_euclid(DAY_HOURS);
        let t = if from == to || span <= 0.0 {
            0.0
        } else {
            ((wrapped - baked[from]).rem_euclid(DAY_HOURS) / span).clamp(0.0, 1.0)
        };
        [(from as u32, 1.0 - t), (to as u32, t)]
    }

    /// Seconds into `playback`'s shot `alpha` of the way through the latest step: interpolated like the
    /// clock, and never from before the track's start.
    fn cutscene_seconds(&self, playback: &Playback, alpha: f32) -> f32 {
        let behind = f64::from(1.0 - alpha.clamp(0.0, 1.0)) * self.world_step;
        (playback.seconds - behind).max(0.0) as f32
    }

    /// The base timeline, the weather and zones at `hours`, and how many volumes weighed in.
    fn look_at(&self, hours: f32, probe: Vec3) -> (TodLook, u32) {
        let mut look = TodLook { sphere: celestial_sphere(hours), ..TodLook::default() };
        self.timeline.apply(&mut look, hours, 1.0);
        if let Some(weather) = &self.weather {
            if let Some(blend) = self.weather_blend {
                weather.apply_looks(blend, &mut look, hours);
            }
            // A cutscene's state over the world's whole blend, by the shot's weight.
            if let Some((to, weight)) = self.weather_shot {
                weather.apply_looks(WeatherBlend { from: None, to, weight }, &mut look, hours);
            }
        }
        if self.weather_shares {
            // The shares lie over the day's keyed weather; volumes and a cutscene's keys over them.
            look.weather = KeyedWeather::default();
        }
        let mut active = 0;
        for (_, volume) in &self.volumes {
            let weight = volume.weight_at(probe);
            if weight > 0.0 {
                volume.timeline.apply(&mut look, hours, weight);
                active += 1;
            }
        }
        (look, active)
    }
}

fn check_complete(timeline: &TodTimeline) -> Result<(), TodError> {
    match TodGroups::REQUIRED.first_missing_from(timeline.groups()) {
        Some(group) => Err(TodError::Incomplete(group)),
        None => Ok(()),
    }
}

/// Advances time of day by the step's world time. Add it to `FixedFirst` after
/// `SimSet::Clocks`, so it sees the step's world scales.
pub fn tick_time_of_day(world: Res<'_, WorldClock>, mut time_of_day: ResMut<'_, TimeOfDay>) {
    time_of_day.tick(&world.0);
}
