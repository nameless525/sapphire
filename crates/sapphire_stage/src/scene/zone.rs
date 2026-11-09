//! A zone's cells as the scene's entities (M3 W2): what hosts [`sapphire_world::streaming`].
//!
//! A cell's mesh is an ordinary entity with a [`RenderMesh`] and a [`RenderTransform`], and a cell's
//! light an ordinary [`RenderLight`], so what activation changes reaches the renderer as the frame's
//! instance deltas and lights, and the packet doesn't know zones exist (R4). Nothing here runs in a fixed
//! step: a cell's activation is presentation only.
//!
//! The zone lies moved by `-stand` in the world, as the look-dev town's other pieces do, so its focus is
//! the camera's eye moved back.

use std::f32::consts::PI;

use sapphire_assets::{AssetId, MountError, UnmountReport};
use sapphire_ecs::prelude::*;
use sapphire_math::{Mat4, Vec3};
use sapphire_platform::AppInitError;
use sapphire_render::quality::TierId;
use sapphire_render::{LightKind, LocalLight, MeshHandle, Renderer};
use sapphire_rhi::GpuContext;
use sapphire_world::light_rig::{CharacterLightRig, LightRigLibrary};
use sapphire_world::streaming::{CellHost, StreamSettings, StreamStats, Streamer};
use sapphire_world::tod::TodTimeline;
use sapphire_zone::{ArchivedCookedLight, CookedLightShape};

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
    stand: Vec3,
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
        Ok(Self { streamer, pack, stand: focus })
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
        let ZoneShow { stem, at, walk, settings, pose, figure } = show;
        let handle = content.find_zone(stem)?;
        let id = handle.id();
        let zone = content.read(&handle).expect("a zone just loaded");
        let spawner = zone
            .simulation
            .spawner(sapphire_core::Name::new(at))
            .ok_or_else(|| AppInitError::msg(format!("zone {stem} has no spawner {at:?}")))?;
        let (position, yaw) = (Vec3::from(spawner.position()), spawner.yaw());
        let day = zone.presentation.day.as_ref().map(|text| TodTimeline::from_ron(text.as_str()));
        let rigs = zone.presentation.rigs.as_ref().map(|text| LightRigLibrary::from_ron(text.as_str()));
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
        if let Some(day) = day {
            self.time_of_day_mut()
                .set_timeline(day.map_err(AppInitError::msg)?)
                .map_err(AppInitError::msg)?;
        }
        self.figure = figure;
        self.extract_poses();
        self.wind = lookdev::WIND;
        self.zone_walk = walk;
        let world = self.ecs.world_mut();
        if let Some(rigs) = rigs {
            world.insert_resource(rigs.map_err(AppInitError::msg)?);
        }
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
        let opened = ZoneStage::open_in_place(content, id, stem, position, settings, renderer, gpu)?;
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
        let stand = self.town_stand.ok_or_else(|| AppInitError::msg("the town isn't spawned"))?;
        let zone =
            ZoneStage::open(content, lookdev::TOWN_ZONE, lookdev::TOWN_STEM, stand, settings, renderer, gpu)?;
        self.open_zone(zone, content).map_err(AppInitError::msg)
    }

    /// Takes the zone down, entities and handles at once, and unmounts its pack: a zone change, under a
    /// wipe (M3 W4). What's unused of its assets goes with the call, and what the app holds unloads as
    /// it lets go.
    pub fn close_zone(&mut self, content: &mut Content) -> Option<Result<UnmountReport, MountError>> {
        let ZoneStage { streamer, pack, stand: _ } = self.zone.take()?;
        let (server, _) = content.streaming_parts();
        let mut host = Host { world: self.ecs.world_mut(), find: &|_| None, to_world: Mat4::IDENTITY };
        streamer.close(&mut host);
        server.update();
        Some(server.unmount(&pack))
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

    /// Runs this frame's streaming, after the content's update. Allocates nothing while nothing changes.
    pub(super) fn stream_zone(&mut self, content: &mut Content) {
        let Some(zone) = &mut self.zone else { return };
        let eye = self.ecs.world().resource::<OrbitCamera>().eye();
        let focus = (eye + zone.stand).to_array();
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

    /// Draws what the zone has placed with its meshes on a new `renderer`: after device loss or for a new
    /// quality tier.
    pub fn set_zone_meshes(&mut self, content: &mut Content, renderer: &mut Renderer, gpu: &GpuContext) {
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
                CookedLightShape::Spot { direction, inner_angle, outer_angle } => {
                    LightKind::Spot { direction: Vec3::from(direction), inner_angle, outer_angle }
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
        let (place, facing) = walk.advance().expect("under way");
        assert!(place.distance(Vec3::new(0.0, 0.0, 0.75)) < 1e-5, "{place}");
        assert!(facing.distance(Vec3::Z) < 1e-5);
        let (place, facing) = walk.advance().expect("past the corner");
        assert!(place.distance(Vec3::new(0.5, 0.0, 1.0)) < 1e-5, "{place}");
        assert!(facing.distance(Vec3::X) < 1e-5);
        assert!(walk.advance().is_none(), "it's walked the rail's 2 m");
        assert!(walk.advance().is_none());
    }
}
