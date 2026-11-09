//! A zone laid out for an editor (M3 E5): each placement drawn on its own, as its shape's mesh, where the
//! document has it now.
//!
//! A game draws a zone's cells, which hold its placements merged ([`super::zone`]). An editor moves one
//! placement at a time, so it lays the zone out from the meshes a cook makes of each shape a zone places
//! (`sapphire_zone::Placement::shape_name`), each an ordinary entity with a [`RenderMesh`] and a
//! [`RenderTransform`]: a drag is a new transform on one entity, and reaches the renderer as one instance
//! delta, with nothing cooked, loaded or built. What it lays out is plain (a mesh and a transform each),
//! so nothing here knows documents. It lies at the world's origin, in the zone's own space.

use std::collections::BTreeMap;
use std::collections::btree_map::Entry;

use sapphire_assets::mesh::Mesh;
use sapphire_assets::{AssetId, Handle};
use sapphire_ecs::prelude::*;
use sapphire_math::Mat4;
use sapphire_platform::AppInitError;
use sapphire_render::Renderer;
use sapphire_rhi::GpuContext;

use super::{LOG_TARGET, Scene};
use crate::content::Content;
use crate::extract::{RenderMesh, RenderTransform};

/// One thing to lay out: a mesh of the zone's pack, and where it goes in the zone.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LaidOut {
    pub mesh: AssetId,
    pub transform: Mat4,
}

/// What laying a zone out drew.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Layout {
    /// The things laid out with nothing to draw, by their place in the list: their meshes aren't in the pack,
    /// since nothing has cooked them yet.
    pub missing: Vec<usize>,
    /// How many were spawned or given another mesh, rather than moved or left as they were.
    pub spawned: usize,
}

/// A zone laid out.
pub(super) struct ZoneLayout {
    pack: String,
    /// Each thing's entity, if its mesh loaded, with what it was laid out as.
    placed: Vec<(Option<Entity>, LaidOut)>,
    /// What keeps each mesh loaded.
    held: BTreeMap<AssetId, Handle<Mesh>>,
}

impl Scene {
    /// Lays out `things` from zone pack `pack`, mounting it and building what its meshes draw through if
    /// that's not done: what's there already of the same mesh, at the same place in the list, moves rather
    /// than spawning again, so a drag allocates and loads nothing. Laying out another pack takes the last
    /// down first.
    pub fn lay_out_zone(
        &mut self,
        content: &mut Content,
        pack: &str,
        things: &[LaidOut],
        renderer: &mut Renderer,
        gpu: &GpuContext,
    ) -> Result<Layout, AppInitError> {
        profiling::scope!("Scene::lay_out_zone");
        if self.layout.as_ref().is_some_and(|layout| layout.pack != pack) {
            self.close_layout();
        }
        if self.layout.is_none() {
            content.mount_zone_pack(pack, false, renderer)?;
            self.layout =
                Some(ZoneLayout { pack: pack.to_owned(), placed: Vec::new(), held: BTreeMap::new() });
        }
        let layout = self.layout.as_mut().expect("laid out above");
        let world = self.ecs.world_mut();
        let mut report = Layout::default();
        for gone in layout.placed.drain(things.len().min(layout.placed.len())..) {
            if let (Some(entity), _) = gone {
                world.despawn(entity);
            }
        }
        for (index, thing) in things.iter().enumerate() {
            let kept = layout.placed.get_mut(index).filter(|(_, was)| was.mesh == thing.mesh);
            if let Some((entity, was)) = kept {
                if let Some(entity) = *entity
                    && was.transform != thing.transform
                {
                    world.entity_mut(entity).insert(RenderTransform(thing.transform));
                }
                was.transform = thing.transform;
                if entity.is_none() {
                    report.missing.push(index);
                }
                continue;
            }
            report.spawned += 1;
            let held = match layout.held.entry(thing.mesh) {
                Entry::Occupied(_) => Ok(()),
                Entry::Vacant(entry) => content.hold::<Mesh>(thing.mesh, renderer, gpu).map(|handle| {
                    entry.insert(handle);
                }),
            };
            let entity = match held {
                Ok(()) => {
                    let (_, meshes) = content.streaming_parts();
                    let mesh = meshes(thing.mesh).expect("a mesh held is on the renderer");
                    Some(world.spawn((RenderTransform(thing.transform), RenderMesh(mesh))).id())
                }
                Err(error) => {
                    tracing::debug!(target: LOG_TARGET, mesh = %thing.mesh, %error, "nothing to lay out");
                    report.missing.push(index);
                    None
                }
            };
            match layout.placed.get_mut(index) {
                Some(slot) => {
                    if let (Some(old), _) = std::mem::replace(slot, (entity, *thing)) {
                        world.despawn(old);
                    }
                }
                None => layout.placed.push((entity, *thing)),
            }
        }
        Ok(report)
    }

    /// Takes the laid-out zone down. Its pack stays mounted, and its meshes go as nothing holds them.
    pub fn close_layout(&mut self) {
        let Some(layout) = self.layout.take() else { return };
        let world = self.ecs.world_mut();
        for (entity, _) in layout.placed {
            if let Some(entity) = entity {
                world.despawn(entity);
            }
        }
    }
}
