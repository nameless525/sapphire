//! The simulation's side of the extract boundary (spec §4.6): render proxies as ECS data, and
//! the `Extract` systems that copy what changed into the frame's [`FramePacket`].
//!
//! - **Render proxies:** an entity draws while it has a [`RenderMesh`], and lights while it has
//!   a [`RenderLight`]. Its [`RenderTransform`] (required, identity by default) is where it draws
//!   or lights from this frame; whatever poses the entity writes it, only when it moves. An
//!   entity drawing a skinned or morphed mesh also has a [`RenderPose`], its skinning palette and
//!   morph weights, which animation writes only when they change.
//! - **[`RenderExtract`]** holds the packet ring, and whether the next extraction resends
//!   everything.
//! - **[`extract_instances`]** writes instance deltas with bevy change detection: `Changed`
//!   (which includes additions) for added, re-meshed or moved instances, and
//!   `RemovedComponents` for ones that stopped drawing. An entity that didn't change adds
//!   nothing to the packet.
//! - **[`extract_lights`]** writes every light, every frame: a frame has a few dozen at most, and
//!   the renderer culls and lists them per frame anyway, so deltas would buy nothing.
//! - **[`extract_poses`]** writes the poses that changed, and those of entities that were added
//!   or changed mesh (whose outputs the renderer starts over in the bind pose). Their palettes
//!   and weights are copied into the packet's flat arrays, whose capacity the packet ring keeps,
//!   so a warm frame allocates nothing for them.
//!
//! Extraction must run every frame, drawn or not: `RemovedComponents` keeps removals only until
//! the second `Cleanup` after them, and the renderer must see every packet's deltas.

use sapphire_ecs::prelude::*;
use sapphire_math::{Affine3A, Mat4};
use sapphire_render::{
    FramePacket, FramePacketRing, InstanceId, LightKind, LocalLight, MeshHandle, MeshInstance,
};

/// The mesh an entity draws. Adding it starts drawing; removing it (or despawning the entity)
/// stops.
#[derive(Component, Clone, Copy, Debug, PartialEq, Eq)]
#[require(RenderTransform)]
pub(crate) struct RenderMesh(pub MeshHandle);

/// A light an entity casts. Its position and spot direction are in the entity's space, placed by
/// its [`RenderTransform`]; its range is in world meters, whatever the transform's scale.
#[derive(Component, Clone, Copy, Debug, PartialEq)]
#[require(RenderTransform)]
pub(crate) struct RenderLight(pub LocalLight);

/// Where an entity draws this frame: object to world, interpolated between the last two fixed
/// steps. Written before [`extract_instances`] runs, through `set_if_neq` so an entity that
/// holds still isn't extracted again.
#[derive(Component, Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct RenderTransform(pub Mat4);

/// The pose a skinned or morphed [`RenderMesh`] draws in this frame: its skinning palette (one
/// matrix per palette entry of the mesh, in the mesh's object space) and its morph weights (see
/// [`sapphire_render::SkinnedInstance`]). Written in place, and only when it changes, so its
/// vectors never reallocate and a pose that holds still isn't extracted again.
#[derive(Component, Clone, Debug, Default, PartialEq)]
pub(crate) struct RenderPose {
    pub palette: Vec<Affine3A>,
    pub morph_weights: Vec<f32>,
}

/// The packets extraction fills, and whether the next one resends everything.
#[derive(Resource, Debug, Default)]
pub(crate) struct RenderExtract {
    packets: FramePacketRing,
    /// Send every instance and pose on the next extraction, changed or not.
    resend: bool,
    /// The packet being extracted resends everything.
    resending: bool,
}

impl RenderExtract {
    /// Starts the next frame's packet, empty.
    pub fn begin(&mut self) -> &mut FramePacket {
        self.resending = std::mem::take(&mut self.resend);
        self.packets.begin()
    }

    /// The packet being extracted (after [`Self::begin`]) or last extracted.
    pub fn packet(&self) -> &FramePacket {
        self.packets.current()
    }

    pub fn packet_mut(&mut self) -> &mut FramePacket {
        self.packets.current_mut()
    }

    /// Makes the next extraction send every instance: for a renderer that starts empty, such
    /// as one rebuilt after GPU device loss.
    pub fn resend_all(&mut self) {
        self.resend = true;
    }
}

/// The packet's instance ID for an entity: its bits, unique among live entities.
fn instance_id(entity: Entity) -> InstanceId {
    InstanceId(entity.to_bits())
}

/// A render proxy as extraction reads it.
type Proxy<'a> = (Entity, &'a RenderMesh, &'a RenderTransform);

/// Proxies added since extraction last ran, or whose mesh or transform changed.
type Touched = Or<(Changed<RenderMesh>, Changed<RenderTransform>)>;

/// Writes this frame's instance deltas. Runs in `Extract`.
pub(crate) fn extract_instances(
    mut extract: ResMut<'_, RenderExtract>,
    changed: Query<'_, '_, Proxy<'_>, Touched>,
    all: Query<'_, '_, Proxy<'_>>,
    meshes: Query<'_, '_, (), With<RenderMesh>>,
    mut removed_meshes: RemovedComponents<'_, '_, RenderMesh>,
    mut removed_transforms: RemovedComponents<'_, '_, RenderTransform>,
) {
    let resend = extract.resending;
    let deltas = &mut extract.packet_mut().instances;
    // An entity that lost only its transform stops drawing too. Despawns remove both
    // components, so the mesh's removal alone reports them.
    let lost_transform = removed_transforms.read().filter(|&entity| meshes.contains(entity));
    deltas.removed.extend(removed_meshes.read().chain(lost_transform).map(instance_id));

    let instance = |(entity, mesh, transform): Proxy<'_>| MeshInstance {
        id: instance_id(entity),
        mesh: mesh.0,
        transform: transform.0,
    };
    if resend {
        deltas.changed.extend(all.iter().map(instance));
    } else {
        deltas.changed.extend(changed.iter().map(instance));
    }
}

/// Writes this frame's lights, in query order. Runs in `Extract`.
pub(crate) fn extract_lights(
    mut extract: ResMut<'_, RenderExtract>,
    lights: Query<'_, '_, (&RenderLight, &RenderTransform)>,
) {
    let packet = extract.packet_mut();
    packet.lights.extend(lights.iter().map(|(light, transform)| place(light.0, transform.0)));
}

/// A posed render proxy as extraction reads it.
type Posed<'a> = (Entity, &'a RenderPose);

/// Poses that changed, or whose entity was added, changed mesh or got its transform back: the
/// renderer starts a new instance, or a new mesh, in its bind pose.
type Reposed = (With<RenderMesh>, Or<(Changed<RenderPose>, Changed<RenderMesh>, Added<RenderTransform>)>);

/// Writes this frame's poses. Runs in `Extract`, after the poses are written.
pub(crate) fn extract_poses(
    mut extract: ResMut<'_, RenderExtract>,
    changed: Query<'_, '_, Posed<'_>, Reposed>,
    all: Query<'_, '_, Posed<'_>, With<RenderMesh>>,
) {
    let resend = extract.resending;
    let packet = extract.packet_mut();
    let mut push = |(entity, pose): Posed<'_>| {
        packet.push_skinned(
            instance_id(entity),
            pose.palette.iter().copied(),
            pose.morph_weights.iter().copied(),
        );
    };
    if resend {
        all.iter().for_each(&mut push);
    } else {
        changed.iter().for_each(&mut push);
    }
}

/// `light` from an entity at `transform`.
fn place(light: LocalLight, transform: Mat4) -> LocalLight {
    let kind = match light.kind {
        LightKind::Point => LightKind::Point,
        LightKind::Spot { direction, inner_angle, outer_angle } => {
            LightKind::Spot { direction: transform.transform_vector3(direction), inner_angle, outer_angle }
        }
    };
    LocalLight { position: transform.transform_point3(light.position), kind, ..light }
}

#[cfg(test)]
mod tests {
    use sapphire_math::Vec3;
    use sapphire_render::InstanceDeltas;

    use super::*;

    fn mesh(index: u32) -> RenderMesh {
        RenderMesh(MeshHandle::from_raw(index))
    }

    fn at(x: f32) -> RenderTransform {
        RenderTransform(Mat4::from_translation(Vec3::new(x, 0.0, 0.0)))
    }

    /// An ECS with just extraction, as the scene runs it each frame.
    fn ecs() -> Ecs {
        let mut ecs = Ecs::new();
        ecs.world_mut().insert_resource(RenderExtract::default());
        ecs.add_systems(Extract, (extract_instances, extract_lights, extract_poses).chain());
        ecs.startup();
        ecs
    }

    /// Runs one frame's extraction and returns its deltas.
    fn frame(ecs: &mut Ecs) -> InstanceDeltas {
        ecs.world_mut().resource_mut::<RenderExtract>().begin();
        ecs.extract();
        let deltas = ecs.world().resource::<RenderExtract>().packet().instances.clone();
        ecs.cleanup();
        deltas
    }

    /// The changed instances' IDs, sorted.
    fn ids(deltas: &InstanceDeltas) -> Vec<InstanceId> {
        sorted(deltas.changed.iter().map(|instance| instance.id))
    }

    fn sorted(ids: impl IntoIterator<Item = InstanceId>) -> Vec<InstanceId> {
        let mut ids: Vec<_> = ids.into_iter().collect();
        ids.sort();
        ids
    }

    fn find(deltas: &InstanceDeltas, id: InstanceId) -> MeshInstance {
        *deltas.changed.iter().find(|instance| instance.id == id).expect("extracted")
    }

    #[test]
    fn change_detection_extracts_only_what_changed() {
        let mut ecs = ecs();
        let world = ecs.world_mut();
        let a = world.spawn((mesh(0), at(1.0))).id();
        let b = world.spawn(mesh(0)).id();
        let c = world.spawn((mesh(1), at(3.0))).id();
        // Never drawn: a transform alone is not a render proxy.
        world.spawn(at(9.0));
        let [a_id, b_id, c_id] = [a, b, c].map(instance_id);

        let added = frame(&mut ecs);
        assert_eq!(ids(&added), sorted([a_id, b_id, c_id]));
        assert_eq!(
            find(&added, b_id).transform,
            Mat4::IDENTITY,
            "the transform is required, identity by default"
        );
        assert_eq!(find(&added, c_id).mesh, MeshHandle::from_raw(1));
        assert!(added.removed.is_empty());

        assert!(frame(&mut ecs).is_empty(), "unchanged entities produce nothing");

        // Moving one entity extracts only it; rewriting an equal value through set_if_neq
        // doesn't count as a change.
        ecs.world_mut().get_mut::<RenderTransform>(a).expect("a").set_if_neq(at(2.0));
        ecs.world_mut().get_mut::<RenderTransform>(c).expect("c").set_if_neq(at(3.0));
        let moved = frame(&mut ecs);
        assert_eq!(ids(&moved), [a_id]);
        assert_eq!(moved.changed[0].transform, at(2.0).0);

        // A new mesh is a change too.
        ecs.world_mut().entity_mut(c).insert(mesh(2));
        let remeshed = frame(&mut ecs);
        assert_eq!(ids(&remeshed), [c_id]);
        assert_eq!(remeshed.changed[0].mesh, MeshHandle::from_raw(2));

        // Despawning reports one removal; so does losing either proxy component.
        ecs.world_mut().despawn(b);
        ecs.world_mut().entity_mut(c).remove::<RenderTransform>();
        let removed = frame(&mut ecs);
        assert!(removed.changed.is_empty());
        assert_eq!(removed.removed, [b_id, c_id]);
        ecs.world_mut().entity_mut(a).remove::<RenderMesh>();
        assert_eq!(frame(&mut ecs).removed, [a_id]);
        assert!(frame(&mut ecs).is_empty(), "removals are reported once");

        // Drawing again is an addition.
        ecs.world_mut().entity_mut(a).insert(mesh(0));
        assert_eq!(ids(&frame(&mut ecs)), [a_id]);
    }

    #[test]
    fn lights_are_placed_by_their_transform_every_frame() {
        let mut ecs = ecs();
        let spot = LocalLight {
            position: Vec3::new(0.0, 1.0, 0.0),
            color: Vec3::new(1.0, 0.5, 0.2),
            intensity: 2.0,
            range: 3.0,
            kind: LightKind::Spot { direction: Vec3::NEG_Y, inner_angle: 0.2, outer_angle: 0.4 },
        };
        let point = LocalLight { kind: LightKind::Point, ..spot };
        let world = ecs.world_mut();
        // Turned a quarter around Z and moved: the spot then points along +X.
        let turned = Mat4::from_rotation_translation(
            sapphire_math::Quat::from_rotation_z(std::f32::consts::FRAC_PI_2),
            Vec3::new(5.0, 0.0, 0.0),
        );
        world.spawn((RenderLight(spot), RenderTransform(turned)));
        world.spawn(RenderLight(point));

        let lights = |ecs: &mut Ecs| {
            ecs.world_mut().resource_mut::<RenderExtract>().begin();
            ecs.extract();
            let lights = ecs.world().resource::<RenderExtract>().packet().lights.clone();
            ecs.cleanup();
            lights
        };
        let first = lights(&mut ecs);
        assert_eq!(first.len(), 2);
        assert_eq!(first[1], point, "an identity transform by default");
        let LightKind::Spot { direction, .. } = first[0].kind else { panic!("a spot") };
        assert!((first[0].position - Vec3::new(4.0, 0.0, 0.0)).length() < 1e-5);
        assert!((direction - Vec3::X).length() < 1e-5);
        assert_eq!((first[0].range, first[0].intensity, first[0].color), (3.0, 2.0, spot.color));
        assert_eq!(lights(&mut ecs), first, "unchanged lights are still sent every frame");
    }

    /// Runs one frame's extraction and returns its poses: instance, palette and weights.
    fn poses(ecs: &mut Ecs) -> Vec<(InstanceId, Vec<Affine3A>, Vec<f32>)> {
        ecs.world_mut().resource_mut::<RenderExtract>().begin();
        ecs.extract();
        let packet = ecs.world().resource::<RenderExtract>().packet();
        let poses = packet
            .skinned
            .iter()
            .map(|skinned| {
                let palette = packet.palette(skinned).expect("in range").to_vec();
                (skinned.id, palette, packet.morph_weights(skinned).expect("in range").to_vec())
            })
            .collect();
        ecs.cleanup();
        poses
    }

    #[test]
    fn poses_are_extracted_when_they_change_or_their_instance_starts_over() {
        let mut ecs = ecs();
        let lifted = Affine3A::from_translation(Vec3::Y);
        let pose = RenderPose { palette: vec![Affine3A::IDENTITY, lifted], morph_weights: vec![0.25] };
        let posed = ecs.world_mut().spawn((mesh(0), pose.clone())).id();
        // Not drawn: a pose alone is not a render proxy.
        ecs.world_mut().spawn(pose.clone());
        let id = instance_id(posed);

        assert_eq!(poses(&mut ecs), [(id, pose.palette.clone(), vec![0.25])], "added");
        assert!(poses(&mut ecs).is_empty(), "a pose that holds still is sent once");

        ecs.world_mut().get_mut::<RenderPose>(posed).expect("posed").morph_weights[0] = 0.5;
        assert_eq!(poses(&mut ecs)[0].2, [0.5]);
        // A new mesh starts its outputs over, so the pose is sent again.
        ecs.world_mut().entity_mut(posed).insert(mesh(1));
        assert_eq!(poses(&mut ecs).len(), 1);
        assert!(poses(&mut ecs).is_empty());

        ecs.world_mut().resource_mut::<RenderExtract>().resend_all();
        assert_eq!(poses(&mut ecs).len(), 1, "resent to a renderer that starts over");
        assert!(poses(&mut ecs).is_empty());

        // Losing the transform stops the instance; getting it back starts it over.
        ecs.world_mut().entity_mut(posed).remove::<RenderTransform>();
        assert!(poses(&mut ecs).is_empty());
        ecs.world_mut().entity_mut(posed).insert(RenderTransform::default());
        assert_eq!(poses(&mut ecs).len(), 1);
    }

    #[test]
    fn resend_extracts_every_instance_once() {
        let mut ecs = ecs();
        let world = ecs.world_mut();
        let all: Vec<_> = (0..3).map(|i| instance_id(world.spawn((mesh(0), at(i as f32))).id())).collect();
        frame(&mut ecs);
        assert!(frame(&mut ecs).is_empty());

        ecs.world_mut().resource_mut::<RenderExtract>().resend_all();
        assert_eq!(ids(&frame(&mut ecs)), sorted(all));
        assert!(frame(&mut ecs).is_empty(), "then back to changes only");
    }
}
