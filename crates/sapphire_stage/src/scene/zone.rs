//! A zone's cells as the scene's entities (M3 W2): what hosts [`sapphire_world::streaming`].
//!
//! A cell's mesh is an ordinary entity with a [`RenderMesh`] and a [`RenderTransform`], and a cell's
//! light an ordinary [`RenderLight`], so what activation changes reaches the renderer as the frame's
//! instance deltas and lights, and the packet doesn't know zones exist (R4). Nothing here runs in a fixed
//! step: a cell's activation is presentation only.
//!
//! The zone lies moved by `-stand` in the world, as the look-dev town's other pieces do, so its focus is
//! the camera's eye moved back. A zone arrived in lies where it is, as its simulation does.
//!
//! # Zones coming next (M3 W4)
//!
//! [`Scene::prefetch_zone`] brings one in over frames, placing nothing, while another's on stage: its pack's
//! mount, then its pipelines, warmed up off the frame (M4 H1), then the zone and its hard references at
//! [`LoadPriority::Prefetch`], then the meshes near where it's arrived at, with
//! [`Streamer::prefetch`]'s limits. So no frame stalls on any of it, on a hub's way there or under a wipe,
//! and nothing's asked for before what draws it is built, so `first_use` stays 0 across a change, as a load
//! screen's [`Content::open_zone`] holds it. What the last zone had goes with [`Scene::arrive_in_zone`].
//!
//! # Zones bring their look (M5 M6)
//!
//! A zone's day, light rigs, probe grid, sky material, wind, weather and fog are its own, as it opens and as a
//! trip arrives in it ([`ZoneLook`]). A zone coming next brings its grid and its sky in with it, its grid's bytes
//! kept aside, since the renderer holds one grid, and what a grid draws through warmed with its pipelines; the
//! arrival puts them on. A zone that names none of something gets the stage's own back. And what the zone
//! arriving prefetches stays mounted, the zone left behind included, so a hub's next zone isn't mounted again
//! on every visit.
//!
//! The frame's weather (M5 L4) comes from three places, each over the last: the zone's (dry with none), then its
//! day's keyed weather by the keys' share (`TodLook::weather_over`: a day that keys none leaves the zone's), then a
//! tool's (`Scene::set_weather`, the sandbox's `--weather`). The simulation never reads it.

use std::f32::consts::PI;
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, TryRecvError};

use sapphire_assets::material::Material;
use sapphire_assets::probes::ProbeGrid;
use sapphire_assets::reflections::ReflectionProbes;
use sapphire_assets::{
    AssetId, AssetServer, Handle, LoadPriority, LoadState, MountError, MountState, UnmountReport,
};
use sapphire_audio::{Room, RoomShape};
use sapphire_core::Name;
use sapphire_ecs::prelude::*;
use sapphire_math::{Mat4, Vec3};
use sapphire_platform::AppInitError;
use sapphire_render::quality::TierId;
use sapphire_render::{
    Fog, FramePacket, LightKind, LocalLight, MaterialHandle, MeshHandle, Renderer, Weather, Wind,
};
use sapphire_rhi::GpuContext;
use sapphire_world::light_rig::{CharacterLightRig, LightRigLibrary, LightRigs};
use sapphire_world::streaming::{CellHost, StreamSettings, StreamStats, Streamer};
use sapphire_world::tod::{KeyedWeather, TimeOfDay, TodTimeline};
#[cfg(any(test, feature = "dev"))]
use sapphire_world::tod::{TodError, TodGroups};
use sapphire_zone::{
    ArchivedCookedLight, ArchivedZonePresentation, CookedLightShape, CookedVolume, Zone, ZoneFog,
};

use super::{Figure, LOG_TARGET, Lamp, Scene};
use crate::content::Content;
use crate::extract::{RenderLight, RenderMesh, RenderPose, RenderTransform};
use crate::lookdev;
use crate::orbit::OrbitCamera;
use crate::turntable::Turntable;

/// The zone stage's camera, as a third-person game's sits behind its character.
const ZONE_CAMERA_HEIGHT: f32 = 1.5;
const ZONE_CAMERA_PITCH: f32 = 10.0;
const ZONE_CAMERA_DISTANCE: f32 = 5.5;
const ZONE_CAMERA_FOV: f32 = 55.0;

/// How a zone streams on a tier: by the class of device it's for.
#[must_use]
pub fn stream_settings(tier: TierId) -> StreamSettings {
    match tier {
        TierId::DesktopHigh | TierId::DesktopLow => StreamSettings::DESKTOP,
        TierId::MobileHigh | TierId::MobileLow => StreamSettings::MOBILE,
    }
}

/// One thing a zone placed.
pub struct ZoneEntity {
    pub(super) entity: Entity,
    /// The mesh it draws, for finding on a new renderer. A light has none.
    pub(super) mesh: Option<AssetId>,
}

/// An open zone: its streamer, and where it lies.
pub struct ZoneStage {
    pub(super) streamer: Streamer<ZoneEntity>,
    /// Its pack, for closing it.
    pack: String,
    /// The zone's origin's place in the world is `-stand`.
    pub(super) stand: Vec3,
    /// Where it streams from, in its own space, until the camera's had a frame there:
    /// [`Scene::arrive_in_zone`] to [`Scene::end_arrival`].
    pub(super) held_focus: Option<[f32; 3]>,
}

/// Where a way out of a zone leads: an exit, as a show reads it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ZoneTarget {
    /// The zone asset.
    pub zone: AssetId,
    /// Its pack, which nothing need have mounted.
    pub pack: String,
    /// The ID of the spawner it arrives at, or of the checkpoint: a restarted run's trip home arrives at its
    /// checkpoint.
    pub arrive: u64,
}

/// How far a zone coming next has got ([`Scene::prefetch_zone`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ZoneLoad {
    /// On its way. Also: nobody asked.
    Waiting,
    /// The zone and its hard references are resident, and the meshes near where it's arrived at are on the
    /// renderer: [`Scene::arrive_in_zone`] draws from there.
    Ready,
    /// It can't come, and why. The scene keeps what it has on stage, and asking again tries again.
    Failed(String),
}

/// How far along a zone coming next is.
enum Coming {
    Mounting,
    /// Its pipelines.
    Prewarming,
    /// The zone, and what it depends on.
    Zone(Handle<Zone>),
    /// Its day and rigs, parsed on a job off the frame's thread (exit test 2), the zone in.
    Reading(Handle<Zone>, Receiver<ParsedLook>),
    /// The meshes around where it's arrived at, boxed: a streamer is most of what a zone coming holds.
    Near(Box<Streamer<ZoneEntity>>),
    Failed(String),
}

/// A zone on its way: see the [module docs](self).
pub(super) struct NextZone {
    target: ZoneTarget,
    settings: StreamSettings,
    coming: Coming,
    /// Someone's waiting under a wipe.
    hurried: bool,
    /// Where it's arrived at, once it can be read.
    focus: [f32; 3],
    /// Its probe grid, sky and reflection probes, coming with it.
    look: ComingLook,
}

/// What of a zone's look comes in with it, ahead of its arrival: its probe grid, whose bytes wait aside in the
/// server until the arrival puts them on the renderer; its sky material, on the renderer beside the app's; and
/// its reflection probes (M5 L3), on the renderer unshown until the arrival shows them. A hub that keeps an
/// exit's pack mounted has them from that pack. They leave with the look: the handles go with it.
#[derive(Default)]
struct ComingLook {
    grid: Option<Handle<ProbeGrid>>,
    sky: Option<Handle<Material>>,
    reflections: Option<Handle<ReflectionProbes>>,
    /// Its day, rigs and air, read as the zone came in: one that doesn't read can't come.
    read: Option<ZoneLook>,
    /// All are in, or the zone names none.
    ready: bool,
}

/// The assets of a zone's look its presentation names: its probe grid, sky material and reflection probes.
type LookNames = (Option<AssetId>, Option<AssetId>, Option<AssetId>);

impl ComingLook {
    /// Asks for the grid, sky and reflection probes a zone's presentation names, at `priority`.
    fn ask(
        server: &mut AssetServer,
        (grid, sky, reflections): LookNames,
        priority: LoadPriority,
    ) -> Result<Self, String> {
        let grid = grid.map(|grid| server.load_with(grid, priority)).transpose();
        let sky = sky.map(|sky| server.load_with(sky, priority)).transpose();
        let reflections = reflections.map(|probes| server.load_with(probes, priority)).transpose();
        Ok(Self {
            grid: grid.map_err(|error| error.to_string())?,
            sky: sky.map_err(|error| error.to_string())?,
            reflections: reflections.map_err(|error| error.to_string())?,
            read: None,
            ready: false,
        })
    }

    /// Whether it's all in yet, or why it can't come.
    fn follow(&mut self, server: &AssetServer) -> Result<(), String> {
        let grid = self.grid.as_ref().map_or(LoadState::Ready, |grid| server.state(grid));
        let sky = self.sky.as_ref().map_or(LoadState::Ready, |sky| server.state(sky));
        let reflections = self.reflections.as_ref().map_or(LoadState::Ready, |probes| server.state(probes));
        match (grid, sky, reflections) {
            (LoadState::Failed(error), _, _) => Err(format!("its probe grid didn't load: {error}")),
            (_, LoadState::Failed(error), _) => Err(format!("its sky material didn't load: {error}")),
            (_, _, LoadState::Failed(error)) => Err(format!("its reflection probes didn't load: {error}")),
            (LoadState::Ready, LoadState::Ready, LoadState::Ready) => {
                self.ready = true;
                Ok(())
            }
            _ => Ok(()),
        }
    }
}

impl NextZone {
    /// A frame of it. With `tool_weather`, its own weather never draws ([`Scene::set_weather`]).
    fn advance(&mut self, content: &mut Content, tool_weather: bool) {
        let pack = self.target.pack.as_str();
        match &self.coming {
            Coming::Mounting => {
                let (server, _) = content.streaming_parts();
                match server.mount_state(pack) {
                    MountState::Mounted => {
                        content.queue_zone_prewarm(pack);
                        self.coming = Coming::Prewarming;
                    }
                    MountState::Mounting => {}
                    MountState::Failed(error) => self.coming = Coming::Failed(error.to_string()),
                    MountState::Unmounted => {
                        if let Err(error) = server.mount(pack) {
                            self.coming = Coming::Failed(error.to_string());
                        }
                    }
                }
            }
            Coming::Prewarming => {
                if !content.prewarms_pack(pack) {
                    let priority = self.priority();
                    let (server, _) = content.streaming_parts();
                    self.coming = match server.load_with(self.target.zone, priority) {
                        Ok(zone) => Coming::Zone(zone),
                        Err(error) => Coming::Failed(error.to_string()),
                    };
                }
            }
            Coming::Zone(zone) => {
                let (server, _) = content.streaming_parts();
                match server.state(zone) {
                    LoadState::Ready => {
                        // Its day and rigs are text, parsed on a job: nothing loads on the frame's thread.
                        let presentation = &server.get(zone).expect("the zone is ready").presentation;
                        let day = presentation.day.as_ref().map(|text| text.as_str().to_owned());
                        let rigs = presentation.rigs.as_ref().map(|text| text.as_str().to_owned());
                        let (sender, receiver) = mpsc::sync_channel(1);
                        sapphire_jobs::spawn_async(move || {
                            let _ = sender.send(parse_look(day.as_deref(), rigs.as_deref()));
                        });
                        self.coming = Coming::Reading(zone.clone(), receiver);
                    }
                    LoadState::Failed(error) => self.coming = Coming::Failed(error.to_string()),
                    LoadState::Loading => {}
                }
            }
            Coming::Reading(zone, parsed) => {
                let parsed = match parsed.try_recv() {
                    Ok(parsed) => parsed,
                    Err(TryRecvError::Empty) => return,
                    Err(TryRecvError::Disconnected) => Err("its read stopped".to_owned()),
                };
                let reflects = content.reflects_probes();
                let (server, _) = content.streaming_parts();
                self.focus = arrival(server, zone, self.target.arrive);
                let presentation = &server.get(zone).expect("the zone is ready").presentation;
                let reflections = presentation.reflections.as_ref().copied().filter(|_| reflects);
                let names =
                    (presentation.probes.as_ref().copied(), presentation.sky.as_ref().copied(), reflections);
                // Its wet surfaces, off the frame, if it may go wet (M5 L4): the pack's prewarm
                // couldn't say, with the zone unread.
                let day_wet = matches!(&parsed, Ok((Some(day), _)) if day.may_be_wet());
                let wet = !tool_weather && (weather_is_wet(presentation) || day_wet);
                // Its day and rigs read before it's ready, so a look that doesn't read fails the trip
                // before the arrival switches anything (Z3 review F4).
                let read = parsed
                    .map(|(day, rigs)| ZoneLook::with_air(presentation, day, rigs))
                    .map_err(|error| format!("its look: {error}"));
                let priority = self.priority();
                self.coming = match read.and_then(|read| {
                    ComingLook::ask(server, names, priority)
                        .map(|look| ComingLook { read: Some(read), ..look })
                }) {
                    Ok(look) => {
                        self.look = look;
                        Coming::Near(Box::new(Streamer::new(zone.clone(), self.settings)))
                    }
                    Err(error) => Coming::Failed(error),
                };
                if wet {
                    content.queue_wet_prewarm(pack);
                }
            }
            Coming::Near(_) | Coming::Failed(_) => {}
        }
        if let Coming::Near(streamer) = &mut self.coming {
            let (server, _) = content.streaming_parts();
            streamer.prefetch(self.focus, server, self.hurried);
            if let Err(error) = self.look.follow(server) {
                self.coming = Coming::Failed(error);
            }
            // Its wet surfaces are part of its look: not ready until they're built.
            if content.prewarms_pack(pack) {
                self.look.ready = false;
            }
        }
    }

    /// Whether it's ready to arrive in: the meshes around where it's arrived at, its grid and its sky in.
    fn is_ready(&self) -> bool {
        matches!(&self.coming, Coming::Near(streamer) if streamer.near_is_loaded()) && self.look.ready
    }

    fn priority(&self) -> LoadPriority {
        if self.hurried { LoadPriority::Critical } else { LoadPriority::Prefetch }
    }

    /// Lets go of what it holds, and of its pack.
    fn drop_with_pack(self, content: &mut Content) {
        let Self { target, coming, .. } = self;
        drop(coming);
        content.drop_zone_prewarm(&target.pack);
        let (server, _) = content.streaming_parts();
        server.update();
        if let Err(error) = server.unmount(&target.pack) {
            tracing::warn!(target: LOG_TARGET, %error, pack = target.pack, "a zone that wasn't needed stays mounted");
        }
    }
}

/// A place to stand written `x,y,z,yaw`, in the zone's space and degrees, for a shot no spawner stands at (a
/// reflection probe's room, M5 L3), or `None` for anything else.
fn standing_place(at: &str) -> Option<(Vec3, f32)> {
    let mut numbers =
        at.split(',').map(|number| number.trim().parse::<f32>().ok().filter(|number| number.is_finite()));
    let place = [numbers.next()??, numbers.next()??, numbers.next()??, numbers.next()??];
    numbers.next().is_none().then(|| (Vec3::new(place[0], place[1], place[2]), place[3].to_radians()))
}

/// Where spawner or checkpoint `id` of `zone` stands: the zone's origin for one it hasn't got, as a cook holds
/// an exit to it.
fn arrival(server: &AssetServer, zone: &Handle<Zone>, id: u64) -> [f32; 3] {
    let simulation = &server.get(zone).expect("the zone is ready").simulation;
    let spawner = simulation.spawners.iter().find(|spawner| spawner.id.to_native() == id);
    let checkpoint = || simulation.checkpoints.iter().find(|checkpoint| checkpoint.id.to_native() == id);
    spawner
        .map(|spawner| spawner.position())
        .or_else(|| checkpoint().map(|checkpoint| checkpoint.position()))
        .unwrap_or([0.0; 3])
}

impl ZoneStage {
    /// Opens the zone `id`, whose source's stem is `stem`, with its first focus at `focus` in its own
    /// space and what's near that on `renderer`: the load screen's half. Its cells appear on the frame
    /// after this, all at once, when a scene given it first follows the content.
    pub fn open(
        content: &mut Content,
        id: AssetId,
        stem: &str,
        focus: Vec3,
        settings: StreamSettings,
        renderer: &mut Renderer,
        gpu: &GpuContext,
    ) -> Result<Self, AppInitError> {
        profiling::scope!("ZoneStage::open");
        let pack = sapphire_zone::pack_name(stem);
        let streamer = content.open_zone(id, &pack, focus.to_array(), settings, renderer, gpu)?;
        Ok(Self { streamer, pack, stand: focus, held_focus: None })
    }

    /// As [`Self::open`], with the zone lying where its document says, since its probe grid and what the
    /// simulation reads of it are in the zone's space.
    pub fn open_in_place(
        content: &mut Content,
        id: AssetId,
        stem: &str,
        focus: Vec3,
        settings: StreamSettings,
        renderer: &mut Renderer,
        gpu: &GpuContext,
    ) -> Result<Self, AppInitError> {
        Ok(Self { stand: Vec3::ZERO, ..Self::open(content, id, stem, focus, settings, renderer, gpu)? })
    }
}

/// What a zone says it looks like, read from its presentation (M5 M6): what [`Scene`] takes as it opens and as a
/// trip arrives in it. Its probe grid and its sky material are assets, which the content holds.
pub struct ZoneLook {
    day: Option<TodTimeline>,
    rigs: Option<LightRigLibrary>,
    wind: Option<Wind>,
    weather: Option<Weather>,
    fog: Option<ZoneFog>,
    /// Where its sound takes a reverb (M6 U5).
    rooms: Arc<[Room]>,
}

impl ZoneLook {
    /// Reads `presentation`'s: its day must be complete, as a stage's must, and its rigs a valid library. A cook
    /// checked the day; the rigs only by their type (a content test reads the shipped zones'), so a zone coming
    /// reads its look before it's ready, and can't come if it doesn't read (Z3 review F4).
    /// A zone coming parses its day and rigs on a job instead ([`parse_look`]).
    pub fn read(presentation: &ArchivedZonePresentation) -> Result<Self, String> {
        let day = presentation.day.as_ref().map(|text| text.as_str());
        let (day, rigs) = parse_look(day, presentation.rigs.as_ref().map(|text| text.as_str()))?;
        Ok(Self::with_air(presentation, day, rigs))
    }

    /// `day` and `rigs`, parsed, with `presentation`'s wind, weather and fog, which are plain numbers.
    fn with_air(
        presentation: &ArchivedZonePresentation,
        day: Option<TodTimeline>,
        rigs: Option<LightRigLibrary>,
    ) -> Self {
        let wind = presentation.wind.as_ref().map(|wind| {
            let wind = wind.to_native();
            Wind { direction: Vec3::from(wind.direction), strength: wind.strength, gustiness: wind.gustiness }
        });
        let weather = presentation.weather.as_ref().map(|weather| {
            let weather = weather.to_native();
            Weather { wetness: weather.wetness, puddles: weather.puddles }
        });
        let fog = presentation.fog.as_ref().map(|fog| fog.to_native());
        Self { day, rigs, wind, weather, fog, rooms: rooms_of(presentation) }
    }
}

/// A zone's rooms as the mixer takes them (M6 U5), in its space.
fn rooms_of(presentation: &ArchivedZonePresentation) -> Arc<[Room]> {
    presentation
        .rooms
        .iter()
        .map(|room| Room {
            position: Vec3::from(room.position()),
            shape: match room.volume.to_native() {
                CookedVolume::Box { half_extents } => {
                    RoomShape::Box { half_extents: Vec3::from(half_extents) }
                }
                CookedVolume::Sphere { radius } => RoomShape::Sphere { radius },
            },
            priority: room.priority,
            reverb: Name::new(room.reverb.as_str()),
            outdoor: room.outdoor,
            fade: room.fade.to_native(),
        })
        .collect()
}

/// A zone's day and rigs, parsed on a job, or why they don't read.
type ParsedLook = Result<(Option<TodTimeline>, Option<LightRigLibrary>), String>;

/// Parses a zone's `day` and `rigs` texts: its day must be complete, as a stage's must, and its rigs a valid
/// library. Building them from text is a load (exit test 2's count, `LoadWork::Build`), which a zone coming does
/// on a job.
fn parse_look(day: Option<&str>, rigs: Option<&str>) -> ParsedLook {
    if day.is_some() || rigs.is_some() {
        sapphire_core::frame_loads::count(sapphire_core::frame_loads::LoadWork::Build);
    }
    let day = match day.map(TodTimeline::from_ron).transpose().map_err(|error| format!("its day: {error}"))? {
        Some(day) if !day.is_complete() => {
            return Err("its day: it doesn't key every group a stage's day must".to_owned());
        }
        day => day,
    };
    let rigs = rigs.map(LightRigLibrary::from_ron).transpose();
    Ok((day, rigs.map_err(|error| format!("its light rigs: {error}"))?))
}

#[cfg(test)]
impl ZoneLook {
    /// A look naming `day` and `rigs`, and no air.
    pub(crate) fn of(day: Option<TodTimeline>, rigs: Option<LightRigLibrary>) -> Self {
        Self { day, rigs, wind: None, weather: None, fog: None, rooms: Arc::new([]) }
    }
}

/// Whether a zone's frames may draw wet (M5 L4): its own weather is, or its day keys any. What a load or a trip
/// declares for it (`PipelineInventory::wet_surfaces`), so its first wet frame builds nothing.
pub(crate) fn may_be_wet(presentation: &ArchivedZonePresentation) -> bool {
    let own = weather_is_wet(presentation);
    // A load's or a boot's: a zone coming parses its day on a job.
    let day = || {
        let day = presentation.day.as_ref().map(|text| {
            sapphire_core::frame_loads::count(sapphire_core::frame_loads::LoadWork::Build);
            TodTimeline::from_ron(text.as_str())
        });
        day.is_some_and(|day| day.is_ok_and(|day| day.may_be_wet()))
    };
    own || day()
}

/// Whether a zone's own weather is wet.
fn weather_is_wet(presentation: &ArchivedZonePresentation) -> bool {
    presentation.weather.as_ref().is_some_and(|weather| {
        let weather = weather.to_native();
        Weather { wetness: weather.wetness, puddles: weather.puddles }.is_wet()
    })
}

/// What a zone's look leaves on the scene beyond its day and rigs (M5 M6), which the frame's packet takes, and
/// what the stage had before a zone's look first replaced it.
#[derive(Default)]
pub(super) struct ZoneAir {
    /// The stage's own day, rigs and wind, for a zone that names none of them.
    own: Option<StageLook>,
    /// Whether the day live is the zone's own, and the rigs: a save of the stage's own file then waits in
    /// [`Self::own`] rather than going live (Z3 review F5, `Scene::take_stage_day` in `dev` builds).
    zone_day: bool,
    zone_rigs: bool,
    /// The zone on stage and the version of it whose look the stage took, for a recook to take its look again
    /// ([`Scene::follow_zone_look`]).
    #[cfg(feature = "dev")]
    look_version: Option<(AssetId, u32)>,
    /// The zone's weather: dry without one.
    pub(super) weather: Weather,
    /// Its fog over the day's.
    fog: Option<ZoneFog>,
    /// Its sky material on the renderer, once it's there (`Content::zone_sky_material`).
    pub(super) sky: Option<MaterialHandle>,
    /// Its rooms (M6 U5), which [`Scene::sound`] hands the mixer while a zone is on stage.
    pub(super) rooms: Arc<[Room]>,
}

/// What the stage looked like before a zone's: see [`ZoneAir::own`].
struct StageLook {
    day: TodTimeline,
    rigs: LightRigLibrary,
    wind: Wind,
}

impl ZoneAir {
    /// Writes the zone's weather under the day's `keyed` weather (M5 L4) and its fog over the day's into `packet`,
    /// after time of day has written it, the zone's origin lying at `origin` in the world's height; and its sky in
    /// place of the app's `sky`.
    pub(super) fn write(
        &self,
        packet: &mut FramePacket,
        sky: Option<MaterialHandle>,
        origin: f32,
        keyed: KeyedWeather,
    ) {
        packet.weather = keyed.over(self.weather);
        packet.sky_material = self.sky.or(sky);
        if let Some(zone) = self.fog {
            thicken(&mut packet.fog, zone, origin);
        }
    }
}

/// `fog`, the day's, as zone `zone` has it: each layer's density times the zone's, and the height fog on the
/// zone's floor, which lies at `origin` + its base in the world.
fn thicken(fog: &mut Fog, zone: ZoneFog, origin: f32) {
    fog.near.density *= zone.density;
    fog.far.density *= zone.density;
    fog.height.density *= zone.density;
    if let Some(base) = zone.height_base {
        fog.height.base = origin + base;
    }
}

/// A zone half stood ([`Scene::begin_stand_in_zone`], [`Scene::begin_town_zone`]): the zone, by its stem, and
/// where the character stands.
pub struct ZoneStanding {
    zone: Handle<Zone>,
    stem: String,
    position: Vec3,
    settings: StreamSettings,
}

/// How the zone stage stands and moves its character (`--zone`, M3 C1).
pub struct ZoneShow<'a> {
    /// The zone, by its source's stem.
    pub stem: &'a str,
    /// The spawner the character starts at, by its name.
    pub at: &'a str,
    /// A camera rail to walk along, by its name, and at how many metres a second.
    pub walk: Option<(&'a str, f32)>,
    pub settings: StreamSettings,
    pub pose: RenderPose,
    pub figure: Figure,
}

/// The character walking a zone's rail, with the camera behind it: a walk through the streaming with no
/// simulation in it.
pub(super) struct ZoneWalk {
    points: Vec<Vec3>,
    /// The leg it's on, and how far along that it is, in metres.
    leg: usize,
    along: f32,
    metres_per_step: f32,
}

impl ZoneWalk {
    fn new(points: Vec<Vec3>, metres_per_step: f32) -> Self {
        Self { points, leg: 0, along: 0.0, metres_per_step }
    }

    /// How far along the rail it's walked, in metres.
    fn walked(&self) -> f32 {
        let legs = self.points.windows(2).take(self.leg);
        legs.map(|leg| leg[0].distance(leg[1])).sum::<f32>() + self.along
    }

    /// Moves on a step, and says where that's reached and which way it's facing. It stops at the rail's end.
    fn advance(&mut self) -> Option<(Vec3, Vec3)> {
        let mut left = self.metres_per_step;
        while let [from, to] = *self.points.get(self.leg..self.leg + 2)? {
            let length = from.distance(to);
            if self.along + left < length {
                self.along += left;
                let direction = (to - from) / length;
                return Some((from + direction * self.along, direction));
            }
            left -= length - self.along;
            self.along = 0.0;
            self.leg += 1;
        }
        None
    }
}

impl Scene {
    /// Draws an already opened `zone` from the next frame on, moved so its opening focus is at
    /// the origin. Replacing a zone removes its entities and releases its pack; reopening the
    /// same pack keeps it mounted for the new streamer's preloaded dependencies.
    ///
    /// Open the replacement successfully before calling this. An unmount failure leaves the
    /// prior scene intact.
    pub fn open_zone(&mut self, zone: ZoneStage, content: &mut Content) -> Result<(), MountError> {
        let (server, _) = content.streaming_parts();
        if let Some(previous) = &self.zone
            && previous.pack != zone.pack
            && server.is_mounted(&previous.pack)
        {
            server.unmount(&previous.pack)?;
        }
        if let Some(previous) = self.zone.take() {
            let mut host = Host { world: self.ecs.world_mut(), find: &|_| None, to_world: Mat4::IDENTITY };
            previous.streamer.close(&mut host);
            server.update();
        }
        self.zone = Some(zone);
        Ok(())
    }

    /// The zone stage: the character held in its pose at one of a zone's spawners, under the zone's day and
    /// light rigs, its probe grid on `renderer`. No code names a zone: it's found by its stem, and all this
    /// reads of it is its document's.
    pub fn stand_in_zone(
        &mut self,
        show: ZoneShow<'_>,
        content: &mut Content,
        renderer: &mut Renderer,
        gpu: &GpuContext,
    ) -> Result<(), AppInitError> {
        let standing = self.begin_stand_in_zone(show, content)?;
        let standing = self.light_zone(standing, content, renderer, gpu)?;
        self.finish_stand_in_zone(standing, content, renderer, gpu)
    }

    /// [`Self::stand_in_zone`]'s first part, which a boot stands on a frame of its own (M4 H4): the character and
    /// the camera placed, the zone's day and light rigs read, and its probe grid, sky and reflections asked for
    /// ([`Content::ask_for_zone_look`], M5 X3). [`Self::light_zone`] is the next, on a frame of its own since M4
    /// H8: together they held a boot's frame 22 ms on an M5.
    pub fn begin_stand_in_zone(
        &mut self,
        show: ZoneShow<'_>,
        content: &mut Content,
    ) -> Result<ZoneStanding, AppInitError> {
        let ZoneShow { stem, at, walk, settings, pose, figure } = show;
        let handle = content.find_zone(stem)?;
        // Its grid, sky and reflections load while a boot screen draws, so lighting it waits for none (M5 X3).
        content.ask_for_zone_look(&handle)?;
        let zone = content.read(&handle).expect("a zone just loaded");
        let (position, yaw) = match zone.simulation.spawner(Name::new(at)) {
            Some(spawner) => (Vec3::from(spawner.position()), spawner.yaw()),
            None => standing_place(at)
                .ok_or_else(|| AppInitError::msg(format!("zone {stem} has no spawner {at:?}")))?,
        };
        let look = ZoneLook::read(&zone.presentation)
            .map_err(|error| AppInitError::msg(format!("zone {stem}'s look: {error}")))?;
        let walk = walk
            .map(|(name, metres_per_second)| {
                let spline = zone
                    .presentation
                    .splines
                    .iter()
                    .find(|spline| spline.name.as_str() == name)
                    .ok_or_else(|| AppInitError::msg(format!("zone {stem} has no spline {name:?}")))?;
                let points = (0..spline.points.len()).map(|at| Vec3::from(spline.point(at))).collect();
                let metres_per_step = metres_per_second / sapphire_time::TICK_HZ as f32;
                Ok::<_, AppInitError>(ZoneWalk::new(points, metres_per_step))
            })
            .transpose()?;
        self.figure = figure;
        self.extract_poses();
        // The zone stage's own breeze, for a zone that names no wind.
        self.wind = lookdev::WIND;
        self.take_zone_look(look).map_err(AppInitError::msg)?;
        self.zone_walk = walk;
        let world = self.ecs.world_mut();
        let place = Mat4::from_translation(position) * Mat4::from_rotation_y(yaw);
        let mut character = world.entity_mut(self.character);
        character.remove::<Turntable>();
        character.insert((pose, RenderTransform(place)));
        if let Some(mut rig) = character.get_mut::<CharacterLightRig>() {
            rig.head = figure.head;
        }
        // Behind it and a little over it, as a game's camera holds it.
        let target = position + Vec3::new(0.0, ZONE_CAMERA_HEIGHT, 0.0);
        world.insert_resource(OrbitCamera::shot(
            target,
            yaw + PI,
            ZONE_CAMERA_PITCH.to_radians(),
            ZONE_CAMERA_DISTANCE,
            ZONE_CAMERA_FOV.to_radians(),
        ));
        Ok(ZoneStanding { zone: handle, stem: stem.to_owned(), position, settings })
    }

    /// [`Self::stand_in_zone`]'s second part, after [`Self::begin_stand_in_zone`]: what the zone's meshes draw
    /// through built, and its probe grid on `renderer` (M4 H8), from what the first part asked for: once that has
    /// come, it reads nothing on the frame's thread (M5 X3).
    pub fn light_zone(
        &mut self,
        standing: ZoneStanding,
        content: &mut Content,
        renderer: &mut Renderer,
        gpu: &GpuContext,
    ) -> Result<ZoneStanding, AppInitError> {
        let pack = sapphire_zone::pack_name(&standing.stem);
        let zone = content.begin_zone(standing.zone.id(), &pack, renderer, gpu)?;
        Ok(ZoneStanding { zone, ..standing })
    }

    /// [`Self::stand_in_zone`]'s last part, after [`Self::light_zone`]: the cells near where it
    /// stands, loaded and on `renderer`, and the zone on stage.
    pub fn finish_stand_in_zone(
        &mut self,
        standing: ZoneStanding,
        content: &mut Content,
        renderer: &mut Renderer,
        gpu: &GpuContext,
    ) -> Result<(), AppInitError> {
        profiling::scope!("ZoneStage::open");
        let ZoneStanding { zone, stem, position, settings } = standing;
        let streamer = content.preload_zone(zone, position.to_array(), settings, renderer, gpu);
        let pack = sapphire_zone::pack_name(&stem);
        let opened = ZoneStage { streamer, pack, stand: Vec3::ZERO, held_focus: None };
        self.open_zone(opened, content).map_err(AppInitError::msg)
    }

    /// Takes the zone stage's walk a step on.
    pub(super) fn step_zone_walk(&mut self) {
        let Some((place, facing)) = self.zone_walk.as_mut().and_then(ZoneWalk::advance) else { return };
        let character = self.character;
        let world = self.ecs.world_mut();
        let yaw = facing.x.atan2(facing.z);
        let transform = Mat4::from_translation(place) * Mat4::from_rotation_y(yaw);
        world.entity_mut(character).insert(RenderTransform(transform));
        let mut camera = world.resource_mut::<OrbitCamera>();
        camera.set_target(place + Vec3::new(0.0, ZONE_CAMERA_HEIGHT, 0.0));
        camera.look_from(yaw + PI, ZONE_CAMERA_PITCH.to_radians());
    }

    /// Opens the look-dev town's zone around where the character stands in it: all of the town's cells
    /// on any tier, from anywhere in it, so its shots hold. Call after the look-dev stage or the arena is
    /// spawned.
    pub fn open_town_zone(
        &mut self,
        content: &mut Content,
        settings: StreamSettings,
        renderer: &mut Renderer,
        gpu: &GpuContext,
    ) -> Result<(), AppInitError> {
        let standing = self.begin_town_zone(content, settings, renderer, gpu)?;
        self.finish_town_zone(standing, content, renderer, gpu)
    }

    /// [`Self::open_town_zone`]'s first half, which a boot stands on a frame of its own (M4 Z3e): the town's pack
    /// mounted, what its meshes draw through built, and its zone loaded.
    pub fn begin_town_zone(
        &mut self,
        content: &mut Content,
        settings: StreamSettings,
        renderer: &mut Renderer,
        gpu: &GpuContext,
    ) -> Result<ZoneStanding, AppInitError> {
        let position = self.town_stand.ok_or_else(|| AppInitError::msg("the town isn't spawned"))?;
        let stem = lookdev::TOWN_STEM;
        let zone = content.begin_zone(lookdev::TOWN_ZONE, &sapphire_zone::pack_name(stem), renderer, gpu)?;
        Ok(ZoneStanding { zone, stem: stem.to_owned(), position, settings })
    }

    /// [`Self::open_town_zone`]'s second half, after [`Self::begin_town_zone`]: its cells loaded and on
    /// `renderer`, and the zone on stage.
    pub fn finish_town_zone(
        &mut self,
        standing: ZoneStanding,
        content: &mut Content,
        renderer: &mut Renderer,
        gpu: &GpuContext,
    ) -> Result<(), AppInitError> {
        profiling::scope!("ZoneStage::open");
        let ZoneStanding { zone, stem, position, settings } = standing;
        let streamer = content.preload_zone(zone, position.to_array(), settings, renderer, gpu);
        let pack = sapphire_zone::pack_name(&stem);
        let opened = ZoneStage { streamer, pack, stand: position, held_focus: None };
        self.open_zone(opened, content).map_err(AppInitError::msg)
    }

    /// Brings zone `target` in over frames, while what's on stage stays: see the [module docs](self). Asking
    /// for one on its way changes nothing, and asking for one that failed tries it again.
    pub fn prefetch_zone(&mut self, target: ZoneTarget, settings: StreamSettings) {
        let same = self.next_zones.iter().position(|next| next.target.zone == target.zone);
        if let Some(at) = same {
            if !matches!(self.next_zones[at].coming, Coming::Failed(_)) {
                return;
            }
            self.next_zones.remove(at);
        }
        self.next_zones.push(NextZone {
            target,
            settings,
            coming: Coming::Mounting,
            hurried: false,
            focus: [0.0; 3],
            look: ComingLook::default(),
        });
    }

    /// Someone's waiting for zone `zone`, under a wipe: from now on it goes as fast as it can.
    pub fn hurry_zone(&mut self, zone: AssetId) {
        if let Some(next) = self.next_zones.iter_mut().find(|next| next.target.zone == zone) {
            next.hurried = true;
        }
    }

    /// How far a zone asked for has got.
    #[must_use]
    pub fn zone_load(&self, zone: AssetId) -> ZoneLoad {
        let Some(next) = self.next_zones.iter().find(|next| next.target.zone == zone) else {
            return ZoneLoad::Waiting;
        };
        match &next.coming {
            _ if next.is_ready() => ZoneLoad::Ready,
            Coming::Failed(error) => ZoneLoad::Failed(error.clone()),
            _ => ZoneLoad::Waiting,
        }
    }

    /// Makes a zone that's [`ZoneLoad::Ready`] the one on stage, where it lies, with the focus at where it's
    /// arrived at, and its look the stage's ([`ZoneLook`]; its grid goes on the renderer with the next content
    /// update): the town's dressing goes, the last zone goes, entities and handles, and its pack unmounts, and
    /// any other zone that was coming goes the same way, since from here what's next is the new zone's to say
    /// ([`Self::prefetch_exits`]). What the new zone prefetches stays mounted, and coming if it was: a hub's
    /// next zone, the one just left among them, isn't mounted again on every visit (M5 M6). Under a wipe,
    /// because everything moves at once. The new zone's cells come from the next frame on, within its budget,
    /// until [`Self::zone_settled`], and the mesh pool gives back the last zone's room meanwhile: the wipe holds
    /// until [`Self::zone_arrived`], and [`Self::end_arrival`] lets it go. `None` for a zone that isn't ready.
    pub fn arrive_in_zone(&mut self, zone: AssetId, content: &mut Content) -> Option<ArrivedZone> {
        let at = self.next_zones.iter().position(|next| next.target.zone == zone && next.is_ready())?;
        // The stage takes the look first, read and checked as the zone came: should it not take, nothing
        // switches, the content's look included, and the zone can't come (Z3 review F4).
        let read = self.next_zones[at].look.read.take().expect("a ready zone's look was read");
        if let Err(error) = self.take_zone_look(read) {
            tracing::error!(target: LOG_TARGET, %zone, error, "a zone's look didn't take");
            self.next_zones[at].coming = Coming::Failed(format!("its look: {error}"));
            return None;
        }
        let NextZone { target, coming, focus, look, .. } = self.next_zones.remove(at);
        let Coming::Near(streamer) = coming else { unreachable!("it was just looked at") };
        let (server, _) = content.streaming_parts();
        let arrived = server.get(streamer.zone()).expect("a zone ready to arrive in is resident");
        let ahead: Vec<String> = arrived
            .simulation
            .exits
            .iter()
            .filter(|exit| exit.prefetch)
            .map(|exit| exit.pack.as_str().to_owned())
            .collect();
        content.arrive_in_zone(look.grid, look.sky, look.reflections);
        let left = self.close_zone_keeping(content, &ahead);
        self.leave_town();
        for mut next in std::mem::take(&mut self.next_zones) {
            if ahead.contains(&next.target.pack) {
                next.hurried = false;
                self.next_zones.push(next);
            } else {
                next.drop_with_pack(content);
            }
        }
        self.zone = Some(ZoneStage {
            streamer: *streamer,
            pack: target.pack,
            stand: Vec3::ZERO,
            held_focus: Some(focus),
        });
        Some(ArrivedZone { focus: Vec3::from(focus), left })
    }

    /// Takes `look` in place of what the stage has (M5 M6): its day, light rigs and wind, or the stage's own again
    /// for what it names none of; its weather, dry with none; its fog over the day's, its height base in the
    /// zone's space; and its rooms (M6 U5). Its probe grid and sky are the content's.
    pub fn take_zone_look(&mut self, look: ZoneLook) -> Result<(), String> {
        let ZoneLook { day, rigs, wind, weather, fog, rooms } = look;
        let (zone_day, zone_rigs) = (day.is_some(), rigs.is_some());
        let world = self.ecs.world();
        let own = self.zone_air.own.get_or_insert_with(|| StageLook {
            day: world.resource::<TimeOfDay>().timeline().clone(),
            rigs: world.resource::<LightRigs>().0.clone(),
            wind: self.wind,
        });
        let (day, rigs, wind) = (
            day.unwrap_or_else(|| own.day.clone()),
            rigs.unwrap_or_else(|| own.rigs.clone()),
            wind.unwrap_or(own.wind),
        );
        self.time_of_day_mut().set_timeline(day).map_err(|error| error.to_string())?;
        self.ecs.world_mut().insert_resource(LightRigs(rigs));
        (self.zone_air.zone_day, self.zone_air.zone_rigs) = (zone_day, zone_rigs);
        self.wind = wind;
        self.zone_air.weather = weather.unwrap_or_default();
        self.zone_air.fog = fog;
        self.zone_air.rooms = rooms;
        Ok(())
    }

    /// Takes a save of the stage's own day (Z3 review F5): live at once, unless a zone's own day is, when it waits
    /// in [`ZoneAir::own`] for a zone that names none. Either way a zone naming none gets it from then on, not the
    /// day the stage had before a zone's first replaced it. A day that isn't complete is refused either way.
    #[cfg(any(test, feature = "dev"))]
    pub(super) fn take_stage_day(&mut self, timeline: TodTimeline) -> Result<(), String> {
        if self.zone_air.zone_day {
            if let Some(group) = TodGroups::REQUIRED.first_missing_from(timeline.groups()) {
                return Err(TodError::Incomplete(group).to_string());
            }
        } else {
            self.time_of_day_mut().set_timeline(timeline.clone()).map_err(|error| error.to_string())?;
        }
        if let Some(own) = &mut self.zone_air.own {
            own.day = timeline;
        }
        Ok(())
    }

    /// Takes the look of the zone on stage again as it recooks (hot reload, Z3 review F5): a save of the day or rigs
    /// file a zone names recooks its `.szone` (the cook's key holds those files' bytes), whose new version this
    /// reads, as an arrival would. One that doesn't read leaves the look live as it is, logged. Only for a zone whose
    /// look the stage took, on a stand or an arrival; allocates nothing while nothing changed.
    #[cfg(feature = "dev")]
    pub(super) fn follow_zone_look(&mut self, content: &Content) {
        let Some(zone) = self.zone.as_ref().filter(|_| self.zone_air.own.is_some()) else {
            self.zone_air.look_version = None;
            return;
        };
        let handle = zone.streamer.zone();
        let now = (handle.id(), content.version_of(handle));
        match self.zone_air.look_version {
            Some(was) if was == now => return,
            Some(was) if was.0 == now.0 => {}
            // A zone new on stage: its arrival or stand took the look.
            _ => {
                self.zone_air.look_version = Some(now);
                return;
            }
        }
        let Some(read) = content.read(handle).map(|zone| ZoneLook::read(&zone.presentation)) else { return };
        self.zone_air.look_version = Some(now);
        let zone = now.0;
        match read.and_then(|look| self.take_zone_look(look)) {
            Ok(()) => tracing::info!(target: LOG_TARGET, %zone, "a recooked zone's look is on"),
            Err(error) => {
                tracing::error!(target: LOG_TARGET, %zone, error, "a recooked zone's look didn't take")
            }
        }
    }

    /// Takes a save of the stage's own light rigs, as [`Self::take_stage_day`] takes its day.
    #[cfg(any(test, feature = "dev"))]
    pub(super) fn take_stage_light_rigs(&mut self, rigs: LightRigLibrary) {
        if !self.zone_air.zone_rigs {
            self.ecs.world_mut().insert_resource(LightRigs(rigs.clone()));
        }
        if let Some(own) = &mut self.zone_air.own {
            own.rigs = rigs;
        }
    }

    /// The zone on stage, if there's one.
    #[must_use]
    pub fn zone_on_stage(&self) -> Option<AssetId> {
        self.zone.as_ref().map(|zone| zone.streamer.zone().id())
    }

    /// Whether the zone's first cells are in, by the focus [`Self::arrive_in_zone`] gave it.
    #[must_use]
    pub fn zone_settled(&self) -> bool {
        self.zone.as_ref().is_some_and(|zone| zone.streamer.first_cells_in())
    }

    /// Whether a trip's arrival can end, its wipe let go: the zone's first cells are in, the renderer's mesh
    /// pool has given back what the zone left held, down to its floor (M5 D9,
    /// [`Content::meshes_given_back`]), and the first view's streamed textures are at their plan (M5 Z2,
    /// [`Content::textures_at_plan`]).
    #[must_use]
    pub fn zone_arrived(&self, content: &Content) -> bool {
        self.zone_settled() && content.meshes_given_back() && content.textures_at_plan()
    }

    /// The arrival's over: streaming goes by the camera's eye from here on, and the mesh pool grows as the
    /// zone needs, no longer asked to compact.
    pub fn end_arrival(&mut self, content: &mut Content) {
        if let Some(zone) = &mut self.zone {
            zone.held_focus = None;
        }
        content.end_giving_back();
    }

    /// The zone on stage's exit called `name`, the hash of it, if there's a zone on stage with that exit.
    #[must_use]
    pub fn zone_exit(&self, name: u64, content: &mut Content) -> Option<ZoneTarget> {
        let zone = self.zone.as_ref()?;
        let (server, _) = content.streaming_parts();
        let zone = server.get(zone.streamer.zone())?;
        let exit = zone.simulation.exits.iter().find(|exit| exit.id.to_native() == name)?;
        Some(ZoneTarget {
            zone: exit.to,
            pack: exit.pack.as_str().to_owned(),
            arrive: exit.arrive.to_native(),
        })
    }

    /// Brings in what the zone on stage says is likely next: its exits with `prefetch`, which is what makes it
    /// a hub. Costs what those are worth for as long as it's there, and [`Self::arrive_in_zone`] gives it back.
    pub fn prefetch_exits(&mut self, content: &mut Content, settings: StreamSettings) {
        let Some(zone) = &self.zone else { return };
        let (server, _) = content.streaming_parts();
        let Some(zone) = server.get(zone.streamer.zone()) else { return };
        let targets: Vec<ZoneTarget> = zone
            .simulation
            .exits
            .iter()
            .filter(|exit| exit.prefetch)
            .map(|exit| ZoneTarget {
                zone: exit.to,
                pack: exit.pack.as_str().to_owned(),
                arrive: exit.arrive.to_native(),
            })
            .collect();
        for target in targets {
            self.prefetch_zone(target, settings);
        }
    }

    /// Takes the zone down, entities and handles at once, and unmounts its pack: a zone change, under a
    /// wipe (M3 W4). What's unused of its assets goes with the call, and what the app holds unloads as
    /// it lets go.
    pub fn close_zone(&mut self, content: &mut Content) -> Option<Result<UnmountReport, MountError>> {
        self.close_zone_keeping(content, &[])
    }

    /// [`Self::close_zone`], its pack left mounted if `kept` names it: then `None`, as nothing was given back.
    fn close_zone_keeping(
        &mut self,
        content: &mut Content,
        kept: &[String],
    ) -> Option<Result<UnmountReport, MountError>> {
        let ZoneStage { streamer, pack, stand: _, held_focus: _ } = self.zone.take()?;
        let (server, _) = content.streaming_parts();
        let mut host = Host { world: self.ecs.world_mut(), find: &|_| None, to_world: Mat4::IDENTITY };
        streamer.close(&mut host);
        server.update();
        (!kept.contains(&pack)).then(|| server.unmount(&pack))
    }

    /// What this frame's zones on their way took on the CPU, mounting, loading and streaming around where
    /// they're arrived at, beside `Content::zone_prewarm_time`: a bench's prefetch cost (M3 Z1).
    pub fn prefetch_time(&self) -> std::time::Duration {
        self.prefetch_time
    }

    /// How far the zone stage's walk has gone along its rail, in metres: where a bench's frame was on its
    /// tour (M5 B4). `None` without a walk.
    pub fn zone_walked(&self) -> Option<f32> {
        self.zone_walk.as_ref().map(ZoneWalk::walked)
    }

    /// Puts the zone stage's walk back at its rail's start, for a bench to walk the same tour again; the next
    /// step moves the character and the camera there. Nothing without a walk.
    pub fn restart_zone_walk(&mut self) {
        if let Some(walk) = &mut self.zone_walk {
            (walk.leg, walk.along) = (0, 0.0);
        }
    }

    /// What the zone's streaming did this frame and where it stands, with a zone open.
    pub fn stream_stats(&self) -> Option<StreamStats> {
        self.zone.as_ref().map(|zone| zone.streamer.stats())
    }

    /// Asks the zone to stream for these points, in the world, beside the view's eye, from the next frame on
    /// and until asked again: where a timeline's camera is about to cut, so that the cut lands on cells
    /// already placed (M3 Q3). Past [`AHEAD`](sapphire_world::streaming::AHEAD) of them, the rest are left out.
    pub fn stream_ahead(&mut self, points: impl IntoIterator<Item = Vec3>) {
        let (slots, count) = &mut self.stream_ahead;
        *count = 0;
        for (slot, point) in slots.iter_mut().zip(points) {
            *slot = point;
            *count += 1;
        }
    }

    /// What the zone streams by, with a zone open: its radii and the budget a frame's work shares.
    pub fn stream_settings(&self) -> Option<StreamSettings> {
        self.zone.as_ref().map(|zone| zone.streamer.settings())
    }

    /// Holds the zone's streaming where it is, or lets it go: a fight holds it while an encounter runs
    /// (M3 W5), so its frames neither ask for nor place a cell. Nothing without a zone open.
    pub fn hold_zone_streaming(&mut self, held: bool) {
        if let Some(zone) = &mut self.zone {
            zone.streamer.hold(held);
        }
    }

    /// Places what the zone's load screen brought in ([`Self::finish_stand_in_zone`], [`Self::finish_town_zone`]):
    /// the first streaming update after a preload places every cell it loaded, at once. A boot does it on a frame
    /// of its own under the boot screen, where it was the title's first frame (M4 H8); otherwise the next frame's
    /// [`Self::follow_content`] does. Nothing without a zone open.
    pub fn place_zone(&mut self, content: &mut Content) {
        self.stream_zone(content);
    }

    /// Runs this frame's streaming, after the content's update. Allocates nothing while nothing changes.
    pub(super) fn stream_zone(&mut self, content: &mut Content) {
        if let Some(zone) = &mut self.zone {
            let eye = self.ecs.world().resource::<OrbitCamera>().eye();
            let focus = zone.held_focus.unwrap_or((eye + zone.stand).to_array());
            let (points, count) = &self.stream_ahead;
            let mut ahead = [[0.0; 3]; sapphire_world::streaming::AHEAD];
            for (slot, point) in ahead.iter_mut().zip(&points[..*count]) {
                *slot = (*point + zone.stand).to_array();
            }
            let to_world = Mat4::from_translation(-zone.stand);
            let (server, meshes) = content.streaming_parts();
            let mut host = Host { world: self.ecs.world_mut(), find: &meshes, to_world };
            zone.streamer.update_ahead(focus, &ahead[..*count], server, &mut host);
        }
        self.follow_ambient(content);
        let stopwatch = sapphire_time::Stopwatch::start();
        // A tool's weather is the frame's whatever a zone says (M5 L4), declared with the scene's if wet.
        let tool_weather = self.weather.is_some();
        for next in &mut self.next_zones {
            next.advance(content, tool_weather);
        }
        self.prefetch_time = stopwatch.elapsed();
    }

    /// Draws what the zone has placed with its meshes on a new `renderer`: after device loss or for a new
    /// quality tier.
    pub fn set_zone_meshes(&mut self, content: &mut Content, renderer: &mut Renderer, gpu: &GpuContext) {
        self.set_ambient_meshes(content);
        for next in &mut self.next_zones {
            if let Coming::Near(streamer) = &mut next.coming {
                content.finish_zone_meshes(streamer.as_mut(), renderer, gpu);
            }
        }
        let Some(zone) = &mut self.zone else { return };
        content.finish_zone_meshes(&mut zone.streamer, renderer, gpu);
        let (_, meshes) = content.streaming_parts();
        let world = self.ecs.world_mut();
        for placed in zone.streamer.placed_mut() {
            if let Some(handle) = placed.mesh.and_then(&meshes) {
                world.entity_mut(placed.entity).insert(RenderMesh(handle));
            }
        }
    }
}

/// What [`Scene::arrive_in_zone`] did.
#[derive(Debug)]
pub struct ArrivedZone {
    /// Where it's arrived at, in the world.
    pub focus: Vec3,
    /// What closing the last zone came to, if there was one.
    pub left: Option<Result<UnmountReport, MountError>>,
}

/// Places what the streamer says in the scene's world.
struct Host<'a, F> {
    world: &'a mut World,
    /// A mesh's handle on the renderer. A mesh the server calls ready has one.
    find: &'a F,
    to_world: Mat4,
}

impl<F: Fn(AssetId) -> Option<MeshHandle>> CellHost for Host<'_, F> {
    type Placed = ZoneEntity;

    fn place_mesh(&mut self, mesh: AssetId, transform: Mat4) -> ZoneEntity {
        let mut entity = self.world.spawn(RenderTransform(self.to_world * transform));
        match (self.find)(mesh) {
            Some(handle) => {
                entity.insert(RenderMesh(handle));
            }
            None => tracing::warn!(target: LOG_TARGET, %mesh, "a zone's mesh isn't on the renderer"),
        }
        ZoneEntity { entity: entity.id(), mesh: Some(mesh) }
    }

    fn place_light(&mut self, light: &ArchivedCookedLight) -> ZoneEntity {
        let intensity = light.intensity.to_native();
        let render = RenderLight(LocalLight {
            position: self.to_world.transform_point3(Vec3::from(light.position())),
            color: Vec3::from(light.color()),
            intensity,
            range: light.range.to_native(),
            kind: match light.shape() {
                CookedLightShape::Point => LightKind::Point,
                CookedLightShape::Spot { direction, inner_angle, outer_angle, shadow } => {
                    LightKind::Spot { direction: Vec3::from(direction), inner_angle, outer_angle, shadow }
                }
            },
        });
        let entity = if light.at_night {
            self.world.spawn((Lamp { intensity }, render)).id()
        } else {
            self.world.spawn(render).id()
        };
        ZoneEntity { entity, mesh: None }
    }

    fn remove(&mut self, placed: ZoneEntity) {
        self.world.despawn(placed.entity);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The zone stage's walk goes the rail's length at its speed, faces along each leg, carries over a corner
    /// what it had left of a step, and ends at the rail's end.
    #[test]
    fn a_zone_walk_goes_round_its_corners_and_stops_at_the_end() {
        let points = vec![Vec3::ZERO, Vec3::new(0.0, 0.0, 1.0), Vec3::new(1.0, 0.0, 1.0)];
        let mut walk = ZoneWalk::new(points, 0.75);
        assert_eq!(walk.walked(), 0.0);
        let (place, facing) = walk.advance().expect("under way");
        assert!(place.distance(Vec3::new(0.0, 0.0, 0.75)) < 1e-5, "{place}");
        assert!(facing.distance(Vec3::Z) < 1e-5);
        let (place, facing) = walk.advance().expect("past the corner");
        assert!(place.distance(Vec3::new(0.5, 0.0, 1.0)) < 1e-5, "{place}");
        assert!(facing.distance(Vec3::X) < 1e-5);
        assert!((walk.walked() - 1.5).abs() < 1e-5, "{}", walk.walked());
        assert!(walk.advance().is_none(), "it's walked the rail's 2 m");
        assert!(walk.advance().is_none());
    }

    #[test]
    fn a_place_to_stand_is_four_numbers() {
        let (place, yaw) = standing_place("-24, 0, 30.5,90").expect("a place");
        assert_eq!(place, Vec3::new(-24.0, 0.0, 30.5));
        assert!((yaw - std::f32::consts::FRAC_PI_2).abs() < 1e-6);
        for not in ["player_start", "1,2,3", "1,2,3,4,5", "1,2,x,4", "1,2,inf,4", ""] {
            assert!(standing_place(not).is_none(), "{not:?}");
        }
    }
}
