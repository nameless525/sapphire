//! Each frame's shadows, planned on the CPU: which cascades are drawn and where, who casts into
//! what, which characters draw maps and which cast capsules, and what receivers read. Its vectors
//! are reused, so a warm frame allocates nothing.

use glam::Vec3;

use super::fit::{self, CascadeFit, Frustum, LightSpace};
use super::{
    ATLAS_COLUMNS, GpuCapsule, GpuCascade, GpuCharacterMap, MAX_CAPSULES, MAX_CASCADES, MAX_CHARACTER_MAPS,
    ShadowSettings, ShadowStats, ShadowUniforms,
};
use crate::bounds::Sphere;
use crate::characters::CharacterTable;
use crate::mesh_pool::{MeshRecord, MeshStreams};
use crate::pipelines::SurfaceVariant;
use crate::scene::{DrawList, InstanceSlots};
use crate::{CameraData, CharacterBody, FramePacket};

/// How far the frame's wind can carry a mesh's vertices past its cooked bounds, in meters:
/// foliage's margin (`foliage::wind_margin`), 0 for every other mesh.
pub(crate) type WindMargin<'a> = dyn Fn(&MeshRecord) -> f32 + 'a;

/// What each instance slot casts into this frame.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct SlotCaster {
    /// Bit `i`: the instance is drawn into cascade `i`.
    pub cascades: u8,
    /// The character map it is drawn into, if any.
    pub map: Option<u8>,
}

/// Whether an instance slot is a character this frame (it has an entry in the character
/// buffer), and the character group it belongs to if that casts a shadow of its own.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum SlotCharacter {
    #[default]
    Environment,
    Character(Option<u32>),
}

/// A character casting a shadow of its own this frame: its instances share `body`.
#[derive(Clone, Copy, Debug)]
struct CharacterGroup {
    body: CharacterBody,
    /// Around its body and its instances' bounds, world space.
    sphere: Sphere,
    /// From the camera to the sphere's nearest point, for choosing who gets maps.
    distance: f32,
    cast: Cast,
}

/// What a character casts its own shadow with this frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Cast {
    /// A tile of the character atlas (desktop).
    Map(u8),
    /// Its body's capsule.
    Capsule,
    /// Nothing: past the shadow distance, or past the budgets.
    Nothing,
}

/// A frame's shadows (see the module docs) and the cascades as last drawn, which later frames
/// reuse.
#[derive(Debug, Default)]
pub(crate) struct ShadowPlan {
    /// Per cascade: how it was last drawn, while still valid.
    cached: [Option<CascadeFit>; MAX_CASCADES],
    /// The quantized key light the cached cascades were drawn along.
    toward: Option<Vec3>,
    /// Per cascade: its map holds nothing, since nothing cast into it when it was last drawn.
    empty: [bool; MAX_CASCADES],
    /// Frames planned since the renderer was created: whose turn it is.
    turn: u64,
    /// Per cascade: drawn this frame.
    pub drawn: [Option<CascadeFit>; MAX_CASCADES],
    /// This frame's character maps, by index.
    pub maps: Vec<CascadeFit>,
    /// Per instance slot: what it casts into.
    pub casters: Vec<SlotCaster>,
    /// Per instance slot: a character or not.
    characters: Vec<SlotCharacter>,
    groups: Vec<CharacterGroup>,
    /// The groups' indices, nearest the camera first.
    order: Vec<u32>,
    /// What receivers read.
    pub uniforms: ShadowUniforms,
    /// The frame has a main view: there is something to draw.
    pub active: bool,
    pub stats: ShadowStats,
}

impl ShadowPlan {
    /// Plans the frame of `packet` with `settings`: the quantized key light, the cascades, the
    /// casters and the receivers' uniforms. `slots` has the packet's instance deltas applied,
    /// `meshes` are the pool's records, `wind_margin` how far the frame's wind can carry each
    /// mesh's vertices past its bounds (foliage's; 0 for the rest), `list` the frame's draw list
    /// (for the stats' counts), `characters` the frame's characters as the character buffer
    /// resolved them, and `atlas` the character atlas's size in texels.
    #[expect(clippy::too_many_arguments, reason = "the frame's inputs, each read once")]
    pub(crate) fn update(
        &mut self,
        settings: &ShadowSettings,
        packet: &FramePacket,
        slots: &InstanceSlots,
        meshes: &[MeshRecord],
        wind_margin: &WindMargin<'_>,
        list: &DrawList,
        characters: CharacterTable<'_>,
        atlas: (u32, u32),
    ) {
        profiling::scope!("ShadowPlan::update");
        self.drawn = [None; MAX_CASCADES];
        self.maps.clear();
        let Some(view) = packet.main_view() else {
            // Nothing draws, and the frames before and after may be far apart: the next frame
            // with a view draws every cascade.
            self.active = false;
            self.cached = [None; MAX_CASCADES];
            return;
        };
        self.active = true;
        let camera = &view.camera;
        let toward = fit::quantize(-packet.sun.direction);
        let light = LightSpace::new(toward);
        if self.toward != Some(toward) || view.cut {
            if self.toward.is_some_and(|before| before != toward) {
                self.stats.light_steps += 1;
            }
            self.cached = [None; MAX_CASCADES];
            self.toward = Some(toward);
        }
        self.plan_cascades(settings, camera, light);
        self.classify(characters, slots, (meshes, wind_margin), settings, camera, light);
        self.skip_empty();
        self.uniforms = self.pack(settings, camera, light, characters.count, atlas);
        self.count_draws(list, settings);
    }

    /// Fits the cascades for `camera` and picks the ones drawn this frame: the fresh ones
    /// always, a cached one on its turn, when it was never drawn, or once the view has moved an
    /// eighth of its width (a quarter of its radius) since it was ([`Self::skip_empty`] leaves some
    /// out again).
    fn plan_cascades(&mut self, settings: &ShadowSettings, camera: &CameraData, light: LightSpace) {
        let cascades = settings.cascades as usize;
        let frustum = Frustum::of(camera);
        let splits = fit::split_distances(frustum.near, settings.distance as f32, cascades);
        let fresh = cascades - settings.cached_cascades as usize;
        let period = u64::from(settings.cached_cascades).max(2);
        let turn = self.turn;
        self.turn += 1;
        let mut from = frustum.near;
        for (index, &to) in splits.iter().enumerate().take(cascades) {
            let sphere = fit::slice_sphere(camera, &frustum, from, to);
            from = to;
            let desired = CascadeFit::new(light, sphere, settings.resolution);
            let draw = match self.cached[index] {
                None => true,
                Some(_) if index < fresh => true,
                Some(cached) => {
                    let moved = (cached.center.truncate() - desired.center.truncate()).abs().max_element();
                    turn % period == (index - fresh) as u64 || moved > 0.25 * cached.radius
                }
            };
            if draw {
                self.cached[index] = Some(desired);
                self.drawn[index] = Some(desired);
            }
        }
    }

    /// Classifies every instance as character or environment by `characters`, groups characters
    /// by body, gives the nearest groups maps (desktop) and the next capsules, and culls
    /// environment casters per cascade drawn.
    fn classify(
        &mut self,
        characters: CharacterTable<'_>,
        slots: &InstanceSlots,
        (meshes, wind_margin): (&[MeshRecord], &WindMargin<'_>),
        settings: &ShadowSettings,
        camera: &CameraData,
        light: LightSpace,
    ) {
        self.casters.clear();
        self.casters.resize(slots.len(), SlotCaster::default());
        self.characters.clear();
        self.characters.resize(slots.len(), SlotCharacter::Environment);
        // Characters: the instances with an entry in the character buffer, grouped by the body
        // their entry casts with (M1 review RS-005), so each casts as the character, or the
        // world, it shades as. Its bodies come each once, in entry order.
        self.groups.clear();
        self.groups.extend(characters.bodies.iter().map(|&body| CharacterGroup {
            body,
            sphere: capsule_sphere(&body),
            distance: 0.0,
            cast: Cast::Nothing,
        }));
        for &slot in characters.listed {
            let group = characters.body(slot);
            let instance = slots
                .mesh(slot)
                .and_then(|mesh| meshes.get(mesh.index()))
                .zip(slots.transform(slot))
                .map(|(record, transform)| record.bounds.transformed(transform));
            if let Some((group, instance)) = group.zip(instance) {
                let group = &mut self.groups[group as usize];
                group.sphere = group.sphere.union(instance);
            }
            self.characters[slot as usize] = SlotCharacter::Character(group);
        }
        // The nearest get maps (desktop), the next capsules; those past the shadow distance, or
        // past both budgets, cast nothing.
        self.order.clear();
        for (index, group) in self.groups.iter_mut().enumerate() {
            group.distance = (group.sphere.center.distance(camera.position) - group.sphere.radius).max(0.0);
            self.order.push(index as u32);
        }
        let groups = &self.groups;
        // In place (a stable sort allocates); equal distances keep the packet's order.
        self.order.sort_unstable_by(|&a, &b| {
            groups[a as usize].distance.total_cmp(&groups[b as usize].distance).then(a.cmp(&b))
        });
        let reach = settings.distance as f32;
        let (mut capsules, mut uncast) = (0u32, 0u32);
        for &index in &self.order {
            let group = &mut self.groups[index as usize];
            group.cast = if group.distance > reach {
                Cast::Nothing
            } else if settings.character_shadow && self.maps.len() < MAX_CHARACTER_MAPS {
                self.maps.push(CascadeFit::character(light, group.sphere, settings.character_resolution));
                Cast::Map(self.maps.len() as u8 - 1)
            } else if (capsules as usize) < MAX_CAPSULES {
                capsules += 1;
                Cast::Capsule
            } else {
                Cast::Nothing
            };
            uncast += u32::from(group.cast == Cast::Nothing);
        }
        // Characters into their maps; environment casters into the cascades drawn that they can
        // cast into.
        for (slot, mesh, transform) in slots.live_transforms() {
            let caster = &mut self.casters[slot as usize];
            match self.characters[slot as usize] {
                SlotCharacter::Character(group) => {
                    caster.map = group.and_then(|group| match self.groups[group as usize].cast {
                        Cast::Map(map) => Some(map),
                        Cast::Capsule | Cast::Nothing => None,
                    });
                }
                SlotCharacter::Environment => {
                    let sphere = match meshes.get(mesh.index()) {
                        Some(record) if matches!(record.streams, MeshStreams::Slab { .. }) => {
                            // Foliage reaches past its cooked bounds by as far as the wind
                            // carries it (M1 W6), in world meters whatever the transform.
                            let mut sphere = record.bounds.transformed(transform);
                            sphere.radius += wind_margin(record);
                            Some(sphere)
                        }
                        // Posed bounds aren't known yet: every cascade.
                        _ => None,
                    };
                    for (index, fit) in self.drawn.iter().enumerate() {
                        if let Some(fit) = fit
                            && sphere.is_none_or(|sphere| fit.holds(&sphere))
                        {
                            caster.cascades |= 1 << index;
                        }
                    }
                }
            }
        }
        self.stats.character_maps = self.maps.len() as u32;
        self.stats.capsules = capsules;
        self.stats.uncast_characters = uncast;
    }

    /// Leaves out the cascades picked this frame that nothing casts into while their maps hold
    /// nothing already: an empty map reads the same under any fit, so the new fit is kept as if
    /// it had been drawn. The others remember whether they hold anything.
    fn skip_empty(&mut self) {
        let cast = self.casters.iter().fold(0u8, |bits, caster| bits | caster.cascades);
        for (index, drawn) in self.drawn.iter_mut().enumerate() {
            if drawn.is_none() {
                continue;
            }
            let holds = cast & 1 << index != 0;
            if !holds && self.empty[index] {
                *drawn = None;
            }
            self.empty[index] = !holds;
        }
    }

    /// The receivers' uniforms for the frame planned.
    fn pack(
        &self,
        settings: &ShadowSettings,
        camera: &CameraData,
        light: LightSpace,
        entries: u32,
        (width, height): (u32, u32),
    ) -> ShadowUniforms {
        let mut uniforms = ShadowUniforms {
            light_x: light.x.extend(0.0).to_array(),
            light_y: light.y.extend(0.0).to_array(),
            light_z: light.z.extend(0.0).to_array(),
            camera: camera.position.extend(settings.distance as f32).to_array(),
            ..ShadowUniforms::default()
        };
        let cascades = settings.cascades as usize;
        // Every cascade has been drawn by now: the cache keeps those not drawn this frame.
        for (gpu, fit) in uniforms.cascades.iter_mut().zip(&self.cached[..cascades]) {
            if let Some(fit) = fit {
                *gpu = GpuCascade { area: area(fit), depth: depth(fit, fit.resolution as f32, 0.0) };
            }
        }
        let tile = settings.character_resolution as f32;
        for (index, (gpu, fit)) in uniforms.characters.iter_mut().zip(&self.maps).enumerate() {
            let (column, row) = (index as u32 % ATLAS_COLUMNS, index as u32 / ATLAS_COLUMNS);
            let fade = 0.25 * fit::CHARACTER_REACH;
            *gpu = GpuCharacterMap {
                area: area(fit),
                depth: depth(fit, fit.far, 1.0 / fade),
                tile: [
                    column as f32 * tile / width as f32,
                    row as f32 * tile / height as f32,
                    tile / width as f32,
                    tile / height as f32,
                ],
            };
        }
        let capsules = self.order.iter().map(|&index| &self.groups[index as usize]);
        let capsules = capsules.filter(|group| group.cast == Cast::Capsule).take(MAX_CAPSULES);
        let mut count = 0;
        for (gpu, group) in uniforms.capsules.iter_mut().zip(capsules) {
            let body = group.body;
            *gpu = GpuCapsule {
                base: body.base.extend(body.radius).to_array(),
                top: body.top.extend(0.0).to_array(),
            };
            count += 1;
        }
        uniforms.counts = [settings.cascades, self.maps.len() as u32, count, entries];
        uniforms.atlas = [1.0 / width as f32, 1.0 / height as f32, tile, 0.0];
        uniforms
    }

    /// Counts the frame's caster draws, and the environment draws culled from cascades drawn.
    fn count_draws(&mut self, list: &DrawList, settings: &ShadowSettings) {
        let drawn = self.drawn_mask();
        let (mut draws, mut culled) = (0, 0);
        let mut count = |slot: u32| {
            let caster = self.casters.get(slot as usize).copied().unwrap_or_default();
            let cascades = (caster.cascades & drawn).count_ones();
            draws += cascades + u32::from(caster.map.is_some());
            if self.characters.get(slot as usize) == Some(&SlotCharacter::Environment) {
                culled += drawn.count_ones() - cascades;
            }
        };
        for variant in SurfaceVariant::DEPTH_WRITING {
            for commands in &list.commands {
                for command in &commands[variant.index()] {
                    count(command.slot);
                }
            }
            for draw in &list.deformed[variant.index()] {
                count(draw.slot);
            }
        }
        self.stats = ShadowStats {
            enabled: true,
            cascades: settings.cascades,
            resolution: settings.resolution,
            cascades_drawn: drawn.count_ones(),
            cascades_cached: settings.cascades - drawn.count_ones(),
            caster_draws: draws,
            culled_draws: culled,
            ..self.stats
        };
    }

    /// Bit `i`: cascade `i` is drawn this frame.
    pub(crate) fn drawn_mask(&self) -> u8 {
        self.drawn
            .iter()
            .enumerate()
            .filter(|(_, fit)| fit.is_some())
            .fold(0, |bits, (index, _)| bits | 1 << index)
    }

    /// Whether the instance in `slot` is a character this frame.
    #[cfg(test)]
    pub(crate) fn is_character(&self, slot: u32) -> bool {
        matches!(self.characters.get(slot as usize), Some(SlotCharacter::Character(_)))
    }
}

/// The sphere around a body's capsule.
fn capsule_sphere(body: &CharacterBody) -> Sphere {
    Sphere { center: (body.base + body.top) * 0.5, radius: body.base.distance(body.top) * 0.5 + body.radius }
}

/// A cascade's or map's area as receivers read it.
fn area(fit: &CascadeFit) -> [f32; 4] {
    [fit.center.x, fit.center.y, 0.5 / fit.radius, fit.texel()]
}

/// A cascade's or map's depth transform, with `z` and `w` as its receivers read them.
fn depth(fit: &CascadeFit, z: f32, w: f32) -> [f32; 4] {
    let (scale, offset) = fit.depth_transform();
    [scale, offset, z, w]
}

#[cfg(test)]
mod tests {
    use glam::{Mat4, Vec3};

    use super::*;
    use crate::character_prepass::ID_ENTRY_MASK;
    use crate::characters::CharacterList;
    use crate::gpu_types::Dequantize;
    use crate::mesh_pool::{DeformSource, Placement};
    use crate::{
        CharacterFace, CharacterInstance, CharacterLighting, DirectionalLight, InstanceDeltas, InstanceId,
        MeshHandle, MeshInstance, ViewData,
    };

    const SETTINGS: ShadowSettings = ShadowSettings {
        cascades: 4,
        resolution: 2048,
        distance: 120,
        cached_cascades: 2,
        character_shadow: true,
        character_resolution: 1024,
        filter: super::super::ShadowFilter::Tent5,
    };

    /// A static unit cube (mesh 0) and a deformable one (mesh 1).
    fn meshes() -> Vec<MeshRecord> {
        let record = |streams| MeshRecord {
            streams,
            dequantize: Dequantize::default(),
            lods: Vec::new(),
            lod_vertices: Vec::new(),
            placement: Placement::default(),
            slots: Vec::new(),
            bounds: Sphere::around([[-0.5; 3], [0.5; 3]]),
            foliage: None,
        };
        vec![
            record(MeshStreams::Slab {
                format: sapphire_assets::mesh::PositionFormat::Unorm16,
                first_vertex: 0,
            }),
            record(MeshStreams::Deformable(DeformSource::default())),
        ]
    }

    fn instance(id: u64, mesh: u32, at: Vec3) -> MeshInstance {
        MeshInstance {
            id: InstanceId(id),
            mesh: MeshHandle::from_raw(mesh),
            transform: Mat4::from_translation(at),
        }
    }

    fn character(id: u64, body: CharacterBody) -> CharacterInstance {
        CharacterInstance {
            id: InstanceId(id),
            lighting: CharacterLighting::unstyled(DirectionalLight::default()),
            face: CharacterFace::default(),
            body,
            opacity: 1.0,
        }
    }

    fn packet(characters: Vec<CharacterInstance>) -> FramePacket {
        let camera =
            CameraData::look_at(Vec3::new(0.0, 1.5, 4.0), Vec3::new(0.0, 1.0, 0.0), Vec3::Y, 0.75, 1.5, 0.05);
        FramePacket {
            views: vec![ViewData::new(camera)],
            sun: DirectionalLight { direction: Vec3::new(0.55, -0.7, -0.45), ..DirectionalLight::default() },
            characters,
            ..FramePacket::default()
        }
    }

    fn slots(instances: &[MeshInstance]) -> InstanceSlots {
        let mut slots = InstanceSlots::default();
        slots.apply(&InstanceDeltas { changed: instances.to_vec(), removed: Vec::new() });
        slots
    }

    /// Plans `packet`'s frame with `settings`, its characters resolved into `slots` by the
    /// character buffer's own list, as the renderer does.
    fn update(
        plan: &mut ShadowPlan,
        settings: &ShadowSettings,
        packet: &FramePacket,
        slots: &mut InstanceSlots,
    ) {
        let mut list = CharacterList::default();
        list.update(packet, slots, false);
        let (meshes, list_of_draws) = (meshes(), DrawList::default());
        plan.update(settings, packet, slots, &meshes, &|_| 0.0, &list_of_draws, list.table(), (4096, 2048));
    }

    #[test]
    fn characters_cast_only_into_their_maps_and_the_environment_into_cascades() {
        let body = CharacterBody::capsule(Vec3::new(0.0, 0.25, 0.0), Vec3::new(0.0, 1.6, 0.0), 0.25);
        let instances =
            [instance(1, 0, Vec3::ZERO), instance(2, 1, Vec3::Y), instance(3, 0, Vec3::new(2.0, 0.0, 0.0))];
        let mut slots = slots(&instances);
        // Instances 1 and 2 are one character (the same body); 3 is environment.
        let packet = packet(vec![character(1, body), character(2, body)]);
        let mut plan = ShadowPlan::default();
        update(&mut plan, &SETTINGS, &packet, &mut slots);
        assert!(plan.active);
        assert_eq!(plan.drawn_mask(), 0b1111, "every cascade on the first frame");
        assert_eq!(plan.maps.len(), 1, "one character, one map");
        for slot in [0, 1] {
            assert!(plan.is_character(slot));
            assert_eq!(
                plan.casters[slot as usize],
                SlotCaster { cascades: 0, map: Some(0) },
                "never a cascade"
            );
        }
        assert!(!plan.is_character(2));
        assert_eq!(plan.casters[2].map, None);
        assert_eq!(plan.casters[2].cascades & 1, 1, "the environment casts into the first cascade");
        // The entries: the fallback and the character's, which its instances share.
        assert_eq!(plan.uniforms.counts, [4, 1, 0, 2]);
        // Its map holds the whole body and the deformable instance's bounds.
        let fit = plan.maps[0];
        for point in [body.base, body.top, Vec3::new(0.5, 1.5, 0.5)] {
            assert!(fit.holds(&Sphere { center: point, radius: 0.0 }), "{point}");
        }
    }

    #[test]
    fn phones_cast_capsules_and_bodiless_characters_cast_nothing() {
        let mobile = ShadowSettings { cascades: 2, cached_cascades: 1, character_shadow: false, ..SETTINGS };
        let body = CharacterBody::capsule(Vec3::ZERO, Vec3::Y, 0.3);
        let instances = [instance(1, 0, Vec3::ZERO), instance(2, 0, Vec3::X)];
        let packet = packet(vec![character(1, body), character(2, CharacterBody::default())]);
        let mut plan = ShadowPlan::default();
        update(&mut plan, &mobile, &packet, &mut slots(&instances));
        assert!(plan.maps.is_empty());
        assert_eq!(plan.uniforms.counts[..3], [2, 0, 1]);
        assert_eq!(plan.uniforms.capsules[0].base, [0.0, 0.0, 0.0, 0.3]);
        assert_eq!(plan.uniforms.capsules[0].top, [0.0, 1.0, 0.0, 0.0]);
        assert!(plan.is_character(1), "a character without a body still never casts into the cascades");
        assert_eq!(plan.casters[1], SlotCaster::default());
        assert_eq!((plan.stats.capsules, plan.stats.character_maps), (1, 0));
    }

    #[test]
    fn the_nearest_characters_get_maps_and_the_rest_capsules() {
        let instances: Vec<MeshInstance> =
            (0..12).map(|index| instance(index, 0, Vec3::new(0.0, 0.0, -(index as f32) * 3.0))).collect();
        let characters = instances
            .iter()
            .map(|instance| {
                let base = instance.transform.transform_point3(Vec3::ZERO);
                character(instance.id.0, CharacterBody::capsule(base, base + Vec3::Y, 0.25))
            })
            .collect();
        let mut plan = ShadowPlan::default();
        update(&mut plan, &SETTINGS, &packet(characters), &mut slots(&instances));
        assert_eq!(plan.maps.len(), MAX_CHARACTER_MAPS);
        assert_eq!(plan.uniforms.counts[2], 12 - MAX_CHARACTER_MAPS as u32, "the rest cast capsules");
        for slot in 0..MAX_CHARACTER_MAPS as u32 {
            assert_eq!(plan.casters[slot as usize].map, Some(slot as u8), "nearest first");
        }
        assert_eq!(plan.casters[MAX_CHARACTER_MAPS].map, None);
        // The capsules go nearest first too.
        let first = plan.uniforms.capsules[0].base;
        assert!((first[2] + 3.0 * MAX_CHARACTER_MAPS as f32).abs() < 1e-5, "{first:?}");
    }

    /// A posed instance by the camera: it casts into every cascade.
    fn everywhere() -> InstanceSlots {
        slots(&[instance(1, 1, Vec3::ZERO)])
    }

    #[test]
    fn cached_cascades_take_turns_and_everything_redraws_when_the_light_steps() {
        let mut slots = everywhere();
        let mut packet = packet(Vec::new());
        let mut plan = ShadowPlan::default();
        update(&mut plan, &SETTINGS, &packet, &mut slots);
        assert_eq!(plan.drawn_mask(), 0b1111);
        let mut masks = Vec::new();
        for _ in 0..4 {
            update(&mut plan, &SETTINGS, &packet, &mut slots);
            masks.push(plan.drawn_mask());
        }
        assert_eq!(masks, [0b1011, 0b0111, 0b1011, 0b0111], "the far two alternate");
        assert_eq!((plan.stats.cascades_drawn, plan.stats.cascades_cached), (3, 1));
        // A sub-step turn of the sun keeps the cache; a step draws everything again.
        packet.sun.direction = Vec3::new(0.5501, -0.7, -0.45);
        update(&mut plan, &SETTINGS, &packet, &mut slots);
        assert_ne!(plan.drawn_mask(), 0b1111);
        packet.sun.direction = Vec3::new(0.6, -0.7, -0.45);
        update(&mut plan, &SETTINGS, &packet, &mut slots);
        assert_eq!(plan.drawn_mask(), 0b1111);
        assert_eq!(plan.stats.light_steps, 1);
        // One cached cascade is drawn every other frame; a cut draws them all.
        let mobile = ShadowSettings { cascades: 2, cached_cascades: 1, ..SETTINGS };
        let mut plan = ShadowPlan::default();
        let masks: Vec<u8> = (0..5)
            .map(|_| {
                update(&mut plan, &mobile, &packet, &mut slots);
                plan.drawn_mask()
            })
            .collect();
        assert_eq!(masks, [0b11, 0b01, 0b11, 0b01, 0b11]);
        packet.views[0].cut = true;
        update(&mut plan, &mobile, &packet, &mut slots);
        assert_eq!(plan.drawn_mask(), 0b11, "a cut");
    }

    #[test]
    fn a_cached_cascade_left_behind_by_the_view_is_drawn_early() {
        let mut slots = everywhere();
        let mut packet = packet(Vec::new());
        let mut plan = ShadowPlan::default();
        update(&mut plan, &SETTINGS, &packet, &mut slots);
        update(&mut plan, &SETTINGS, &packet, &mut slots);
        assert_eq!(plan.drawn_mask(), 0b1011);
        // The camera turns around: cascade 3 is cached, but its square is now behind the view.
        packet.views[0].camera = CameraData::look_at(
            Vec3::new(0.0, 1.5, 4.0),
            Vec3::new(0.0, 1.0, 20.0),
            Vec3::Y,
            0.75,
            1.5,
            0.05,
        );
        update(&mut plan, &SETTINGS, &packet, &mut slots);
        assert_eq!(plan.drawn_mask(), 0b1111);
    }

    #[test]
    fn environment_casters_are_culled_per_cascade() {
        // One cube by the camera and one far away: the far one misses the first cascade.
        let instances = [
            instance(1, 0, Vec3::ZERO),
            instance(2, 0, Vec3::new(0.0, 0.0, -80.0)),
            instance(3, 1, Vec3::new(500.0, 0.0, 0.0)),
        ];
        let mut plan = ShadowPlan::default();
        update(&mut plan, &SETTINGS, &packet(Vec::new()), &mut slots(&instances));
        assert_eq!(plan.casters[0].cascades & 1, 1);
        assert_eq!(plan.casters[1].cascades & 1, 0, "past the first cascade");
        assert_ne!(plan.casters[1].cascades & 0b1000, 0, "in the last");
        assert_eq!(plan.casters[2].cascades, 0b1111, "posed bounds aren't known: every cascade");
    }

    #[test]
    fn the_wind_keeps_swaying_foliage_in_the_cascades_it_reaches() {
        // The first place, walking away from the camera, where a cube falls out of the first
        // cascade; there, a margin of a meter (foliage's wind) keeps it in.
        let first_cascade = |at: Vec3, margin: f32| {
            let mut plan = ShadowPlan::default();
            let slots = slots(&[instance(1, 0, at)]);
            plan.update(
                &SETTINGS,
                &packet(Vec::new()),
                &slots,
                &meshes(),
                &|_| margin,
                &DrawList::default(),
                CharacterList::default().table(),
                (4096, 2048),
            );
            plan.casters[0].cascades & 1 == 1
        };
        let outside = (0..400)
            .map(|step| Vec3::new(0.0, 0.0, -0.25 * step as f32))
            .find(|&at| !first_cascade(at, 0.0))
            .expect("past the first cascade within 100 m");
        assert!(first_cascade(outside, 1.0), "the wind's margin reaches back in at {outside}");
    }

    #[test]
    fn frames_without_a_view_plan_nothing_and_start_the_cache_over() {
        let mut slots = everywhere();
        let mut plan = ShadowPlan::default();
        update(&mut plan, &SETTINGS, &packet(Vec::new()), &mut slots);
        update(&mut plan, &SETTINGS, &FramePacket::default(), &mut slots);
        assert!(!plan.active && plan.drawn_mask() == 0);
        update(&mut plan, &SETTINGS, &packet(Vec::new()), &mut slots);
        assert_eq!(plan.drawn_mask(), 0b1111);
    }

    #[test]
    fn a_cascade_nothing_casts_into_is_drawn_once_and_left_empty() {
        // A cube by the camera casts into the near two cascades; the far two are drawn empty
        // once, then left as they are, cached or not.
        let mut instances = vec![instance(1, 0, Vec3::ZERO)];
        let mut plan = ShadowPlan::default();
        update(&mut plan, &SETTINGS, &packet(Vec::new()), &mut slots(&instances));
        assert_eq!(plan.drawn_mask(), 0b1111, "the first frame clears every map");
        update(&mut plan, &SETTINGS, &packet(Vec::new()), &mut slots(&instances));
        assert_eq!(plan.drawn_mask(), 0b0011);
        assert_eq!((plan.stats.cascades_drawn, plan.stats.cascades_cached), (2, 2));
        // Their fits are kept for the receivers all the same.
        assert!(plan.uniforms.cascades.iter().all(|cascade| cascade.area[2] > 0.0));
        // A caster far away: the far cascade is drawn on its turn, and again while it holds it.
        instances.push(instance(2, 0, Vec3::new(0.0, 0.0, -80.0)));
        let masks: Vec<u8> = (0..3)
            .map(|_| {
                update(&mut plan, &SETTINGS, &packet(Vec::new()), &mut slots(&instances));
                plan.drawn_mask()
            })
            .collect();
        assert_eq!(masks.iter().filter(|&&mask| mask & 0b1000 != 0).count(), 1, "{masks:?}");
        // A step of the light draws again only the cascades that hold something.
        let mut stepped = packet(Vec::new());
        stepped.sun.direction = Vec3::new(0.6, -0.7, -0.45);
        update(&mut plan, &SETTINGS, &stepped, &mut slots(&instances[..1]));
        assert_eq!(plan.drawn_mask(), 0b1011, "cascade 3 is cleared of the cube that left");
        update(&mut plan, &SETTINGS, &stepped, &mut slots(&instances[..1]));
        assert_eq!(plan.drawn_mask() & 0b1100, 0, "and then left empty");
    }

    /// `count` instances in a row, each listed as a character with a body of its own.
    fn crowd(count: u64) -> (Vec<MeshInstance>, Vec<CharacterInstance>) {
        let instances: Vec<MeshInstance> =
            (0..count).map(|index| instance(index, 0, Vec3::new(index as f32, 0.0, -2.0))).collect();
        let characters = instances
            .iter()
            .map(|instance| {
                let base = instance.transform.transform_point3(Vec3::ZERO);
                character(instance.id.0, CharacterBody::capsule(base, base + Vec3::Y, 0.25))
            })
            .collect();
        (instances, characters)
    }

    #[test]
    fn characters_past_the_entries_are_environment() {
        // The character buffer stops at the prepass IDs' 16,383 entries: the instances past them
        // shade as the world does, so they cast as the world does (M1 review RS-005).
        let cap = ID_ENTRY_MASK as u64;
        for count in [cap, cap + 1, cap + 117] {
            let (instances, characters) = crowd(count);
            let mut slots = slots(&instances);
            let mut plan = ShadowPlan::default();
            update(&mut plan, &SETTINGS, &packet(characters), &mut slots);
            let listed = (0..count as u32).filter(|&slot| slots.character(slot) != 0).count();
            assert_eq!(listed as u64, count.min(cap), "{count}: the entries there are");
            for slot in 0..count as u32 {
                assert_eq!(plan.is_character(slot), slots.character(slot) != 0, "{count}: slot {slot}");
            }
            assert_eq!(plan.groups.len() as u64, count.min(cap), "{count}: a group per character");
        }
    }

    #[test]
    fn an_instance_listed_again_casts_only_the_body_its_entry_has() {
        // Listed twice in two states, the instance reads its second entry: its first body is no
        // one's, and casts nothing.
        let body = CharacterBody::capsule(Vec3::ZERO, Vec3::Y, 0.25);
        let moved = CharacterBody::capsule(Vec3::X, Vec3::X + Vec3::Y, 0.25);
        let twice = packet(vec![character(1, body), character(1, moved)]);
        let mut one = slots(&[instance(1, 0, Vec3::ZERO)]);
        let mut plan = ShadowPlan::default();
        update(&mut plan, &SETTINGS, &twice, &mut one);
        assert_eq!(plan.maps.len(), 1, "one character, one map");
        let mobile = ShadowSettings { character_shadow: false, ..SETTINGS };
        update(&mut plan, &mobile, &twice, &mut one);
        assert_eq!(plan.uniforms.counts[2], 1, "one capsule");
        assert_eq!(plan.uniforms.capsules[0].base, [1.0, 0.0, 0.0, 0.25], "the last body's");
        // At the last entry there is, the second listing finds no room: the instance keeps its
        // first entry, and casts with that body. A part listed later in the first listing's
        // state shares the entry, room or not.
        let cap = ID_ENTRY_MASK as u64;
        let (mut instances, mut characters) = crowd(cap - 1);
        instances.extend([instance(cap, 0, Vec3::ZERO), instance(cap + 1, 0, Vec3::Y)]);
        characters.extend([character(cap, body), character(cap, moved), character(cap + 1, body)]);
        let mut slots = slots(&instances);
        update(&mut plan, &mobile, &packet(characters), &mut slots);
        let [last, part] = [cap, cap + 1].map(|id| slots.slot(InstanceId(id)).expect("live"));
        assert_eq!([last, part].map(|slot| slots.character(slot)), [ID_ENTRY_MASK; 2], "the last entry");
        let SlotCharacter::Character(Some(group)) = plan.characters[last as usize] else {
            panic!("a character with a body: {:?}", plan.characters[last as usize]);
        };
        assert_eq!(plan.groups[group as usize].body, body, "the body of the entry it reads");
        assert_eq!(plan.characters[part as usize], SlotCharacter::Character(Some(group)), "one character");
        assert!(plan.groups.iter().all(|group| group.body != moved), "its second body is no one's");
    }

    #[test]
    fn more_bodies_than_a_frame_holds_plan_without_panicking() {
        // 70,000 distinct bodies, past the 2¹⁶ groups a 16-bit index held: the frame's
        // characters stop at the entries there are, and so do its groups.
        let (instances, characters) = crowd(70_000);
        let mut slots = slots(&instances);
        let mut plan = ShadowPlan::default();
        update(&mut plan, &SETTINGS, &packet(characters), &mut slots);
        assert_eq!(plan.groups.len(), ID_ENTRY_MASK as usize);
        assert_eq!(plan.maps.len(), MAX_CHARACTER_MAPS);
    }

    #[test]
    fn the_plan_classifies_and_groups_by_the_character_buffers_entries() {
        // The cross-check: a frame lists one character in two parts (one state), one instance
        // twice in two states, one character with no body, then one first listed before them
        // all, an instance that isn't drawn and lighting that isn't valid. Each instance casts
        // as the entry it reads makes it, and the groups come in entry order, which is the order
        // the packet first lists their bodies in.
        let at = |x: f32| CharacterBody::capsule(Vec3::new(x, 0.0, -2.0), Vec3::new(x, 1.0, -2.0), 0.25);
        let instances: Vec<MeshInstance> =
            (0..6).map(|id| instance(id, 0, Vec3::new(id as f32, 0.0, -2.0))).collect();
        let mut invalid = character(5, at(5.0));
        invalid.lighting.rim = f32::NAN;
        let characters = vec![
            character(3, at(3.0)),
            character(0, at(0.0)),
            character(1, at(0.0)),
            character(2, at(2.0)),
            character(2, at(7.0)),
            character(4, CharacterBody::default()),
            character(9, at(9.0)),
            invalid,
        ];
        let mut slots = slots(&instances);
        let mut plan = ShadowPlan::default();
        update(&mut plan, &SETTINGS, &packet(characters), &mut slots);
        let bodies: Vec<CharacterBody> = plan.groups.iter().map(|group| group.body).collect();
        assert_eq!(bodies, [at(3.0), at(0.0), at(7.0)]);
        assert_eq!((0..6).map(|slot| slots.character(slot)).collect::<Vec<_>>(), [2, 2, 4, 1, 5, 0]);
        use SlotCharacter::{Character, Environment};
        let expected = [Character(Some(1)), Character(Some(1)), Character(Some(2)), Character(Some(0))];
        assert_eq!(plan.characters[..4], expected);
        assert_eq!(plan.characters[4..6], [Character(None), Environment]);
        for slot in 0..6 {
            assert_eq!(plan.is_character(slot), slots.character(slot) != 0, "slot {slot}");
        }
        assert_eq!(plan.maps.len(), 3, "a map each");
    }
}
