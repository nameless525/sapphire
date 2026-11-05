//! A zone's cells as the scene's entities (M3 W2): what hosts [`sapphire_world::streaming`].
//!
//! A cell's mesh is an ordinary entity with a [`RenderMesh`] and a [`RenderTransform`], and a cell's
//! light an ordinary [`RenderLight`], so what activation changes reaches the renderer as the frame's
//! instance deltas and lights, and the packet doesn't know zones exist (R4). Nothing here runs in a fixed
//! step: a cell's activation is presentation only.
//!
//! The zone lies moved by `-stand` in the world, as the look-dev town's other pieces do, so its focus is
//! the camera's eye moved back.

use sapphire_assets::{AssetId, MountError, UnmountReport};
use sapphire_ecs::prelude::*;
use sapphire_math::{Mat4, Vec3};
use sapphire_platform::AppInitError;
use sapphire_render::quality::TierId;
use sapphire_render::{LightKind, LocalLight, MeshHandle, Renderer};
use sapphire_rhi::GpuContext;
use sapphire_world::streaming::{CellHost, StreamSettings, StreamStats, Streamer};
use sapphire_zone::{ArchivedCookedLight, CookedLightShape};

use super::{LOG_TARGET, Lamp, Scene};
use crate::content::Content;
use crate::extract::{RenderLight, RenderMesh, RenderTransform};
use crate::lookdev;
use crate::orbit::OrbitCamera;

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

    /// Runs this frame's streaming, after the content's update. Allocates nothing while nothing changes.
    pub(super) fn stream_zone(&mut self, content: &mut Content) {
        let Some(zone) = &mut self.zone else { return };
        let eye = self.ecs.world().resource::<OrbitCamera>().eye();
        let focus = (eye + zone.stand).to_array();
        let to_world = Mat4::from_translation(-zone.stand);
        let (server, meshes) = content.streaming_parts();
        let mut host = Host { world: self.ecs.world_mut(), find: &meshes, to_world };
        zone.streamer.update(focus, server, &mut host);
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
