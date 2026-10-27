//! Per-object light lists (spec §5.12 "Lighting", the mobile path): the CPU picks each
//! instance's most influential lights, at most [`LIGHTS_PER_OBJECT`], and the instance's pixels
//! walk only those. No compute pass and no per-pixel cluster lookup, which suits phones.
//!
//! - **Influence** of a light on an instance is the light's luminance × its range window × its
//!   cone, at the point of the instance's bounding sphere nearest the light: the most it can add
//!   anywhere on the instance. A light that can't reach the sphere has none.
//! - **Choice:** the four with the most influence. Equal influence goes to the lower index, so
//!   the choice is deterministic. A list holds its lights in index order, as cluster lists do,
//!   so both paths sum a pixel's lights in the same order.
//! - **When:** an instance's list is chosen again when the instance is added, moved or
//!   re-meshed, and every list when the frame's lights changed (or after a frame without a
//!   view). Lists that come out the same aren't uploaded; the rest go up in runs of consecutive
//!   slots.
//!
//! The list buffer holds one list per instance slot in the cluster lists' layout at capacity 4:
//! a count, then four 16-bit light indices in two words. Bounding spheres come from the meshes'
//! cooked bounds, which their records in the mesh pool carry (M1 R4 moved them there, for
//! shadow casters' culling too).

use bytemuck::{Pod, Zeroable};
use glam::{Mat4, Vec3};

use super::{GpuLight, luminance};
use crate::bounds::Sphere;
use crate::scene::InstanceSlots;
use crate::{MeshHandle, MeshInstance};

/// The most lights one instance is lit by on the per-object path (spec §5.12).
pub(crate) const LIGHTS_PER_OBJECT: u32 = 4;

/// One instance's list as the GPU reads it.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Pod, Zeroable)]
pub(crate) struct ObjectLightList {
    count: u32,
    /// Two 16-bit light indices per word, low half first.
    indices: [u32; 2],
}

/// Bytes per list in the list buffer.
pub(crate) const LIST_BYTES: u64 = size_of::<ObjectLightList>() as u64;

impl ObjectLightList {
    /// A list of `lights`, at most four indices below 2¹⁶.
    fn new(lights: &[u32]) -> Self {
        debug_assert!(lights.len() <= LIGHTS_PER_OBJECT as usize);
        let mut indices = [0; 2];
        for (position, &light) in lights.iter().enumerate() {
            debug_assert!(light <= u32::from(u16::MAX));
            indices[position / 2] |= light << (16 * (position % 2));
        }
        Self { count: lights.len() as u32, indices }
    }

    /// The listed light indices.
    #[cfg(test)]
    pub(crate) fn lights(&self) -> Vec<u32> {
        (0..self.count as usize)
            .map(|position| (self.indices[position / 2] >> (16 * (position % 2))) & 0xffff)
            .collect()
    }
}

/// How much `light` can add anywhere within `sphere`: its luminance × range window × cone at the
/// sphere's point nearest the light (the shader's `light_window` and `light_cone`). Zero when it
/// can't reach the sphere.
pub(crate) fn influence(light: &GpuLight, sphere: &Sphere) -> f32 {
    let offset = sphere.center - Vec3::from_slice(&light.position[..3]);
    let distance = offset.length();
    let gap = (distance - sphere.radius).max(0.0);
    let fade = (1.0 - gap * gap * light.radiance[3]).clamp(0.0, 1.0);
    if fade <= 0.0 {
        return 0.0;
    }
    // A point light's cone is 1 wherever the sphere is: it packs a cone scale of 0, which no
    // spot does (`GpuLight::new`), so the angular path below would compute `t` = 0 × cos + 1, whose
    // smoothstep is exactly 1. Skipping its acos, asin and cos gives the same bits (review
    // RA-O04). A NaN distance keeps the angular path, so a degenerate sphere's influence is what it
    // was too.
    let cone = if distance <= sphere.radius || (light.cone[0] == 0.0 && !distance.is_nan()) {
        1.0
    } else {
        // The smallest angle off the spot's axis that reaches the sphere: the angle to its
        // center less its angular radius.
        let axis = Vec3::from_slice(&light.spot[..3]);
        let angle = (offset / distance).dot(axis).clamp(-1.0, 1.0).acos();
        let nearest = (angle - (sphere.radius / distance).asin()).max(0.0);
        let t = (nearest.cos() * light.cone[0] + light.cone[1]).clamp(0.0, 1.0);
        t * t * (3.0 - 2.0 * t)
    };
    luminance(Vec3::from_slice(&light.radiance[..3])) * fade * fade * cone
}

/// The lights with the most influence on `sphere`, at most four, ties to the lower index, listed
/// in index order.
pub(crate) fn choose(lights: &[GpuLight], sphere: &Sphere) -> ObjectLightList {
    const SLOTS: usize = LIGHTS_PER_OBJECT as usize;
    // A degenerate transform's sphere isn't finite, and its instance draws nothing. `influence`
    // can't tell: `max` ignores NaN, so a NaN sphere's gap comes out 0 and every light in range
    // of nothing would reach it at full strength.
    if !(sphere.center.is_finite() && sphere.radius.is_finite()) {
        return ObjectLightList::default();
    }
    // The best so far, most influential first.
    let mut best = [(0.0f32, 0u32); SLOTS];
    let mut len = 0;
    for (index, light) in lights.iter().enumerate() {
        let influence = influence(light, sphere);
        // And NaN, though a finite sphere and a valid light give none.
        if influence.is_nan() || influence <= 0.0 {
            continue;
        }
        // After every light at least as influential: earlier lights win ties.
        let at = best[..len].iter().position(|&(other, _)| influence > other).unwrap_or(len);
        if at == SLOTS {
            continue;
        }
        best.copy_within(at..SLOTS - 1, at + 1);
        best[at] = (influence, index as u32);
        len = (len + 1).min(SLOTS);
    }
    let mut chosen = best.map(|(_, index)| index);
    chosen[..len].sort_unstable();
    ObjectLightList::new(&chosen[..len])
}

/// Every instance slot's list.
#[derive(Debug, Default)]
pub(crate) struct ObjectLists {
    /// Per instance slot: the list the GPU holds, or will after the next upload.
    lists: Vec<ObjectLightList>,
    /// Per slot: listed in `dirty`.
    queued: Vec<bool>,
    /// Slots whose list changed since the last upload, each once.
    dirty: Vec<u32>,
    /// The next update chooses every live instance's list.
    stale: bool,
    /// Lists chosen by the last update.
    chosen: u32,
}

impl ObjectLists {
    /// Makes the next update choose every instance's list, as after a frame whose lights it
    /// never saw.
    pub(crate) fn invalidate(&mut self) {
        self.stale = true;
    }

    /// Lists chosen by the last update.
    pub(crate) fn chosen(&self) -> u32 {
        self.chosen
    }

    /// The lights each instance slot's list holds, by slot.
    pub(crate) fn counts(&self) -> impl Iterator<Item = u32> + '_ {
        self.lists.iter().map(|list| list.count)
    }

    /// Chooses lists from `lights`: every live instance's if the lights changed since the last
    /// update (or it was invalidated), otherwise only the `changed` instances' (the packet's
    /// additions and changes, already applied to `slots`). `bounds` gives each mesh's
    /// object-space bounding sphere. Only slots below `limit`, the list buffer's, get lists
    /// (review RA-004).
    pub(crate) fn update(
        &mut self,
        lights: &[GpuLight],
        lights_changed: bool,
        slots: &InstanceSlots,
        changed: &[MeshInstance],
        limit: u32,
        bounds: impl Fn(MeshHandle) -> Option<Sphere>,
    ) {
        profiling::scope!("ObjectLists::update");
        let listed = slots.len().min(limit as usize);
        self.lists.resize(listed, ObjectLightList::default());
        self.queued.resize(listed, false);
        self.chosen = 0;
        if std::mem::take(&mut self.stale) || lights_changed {
            for (slot, mesh, transform) in slots.live_transforms().take_while(|&(slot, ..)| slot < limit) {
                self.choose(lights, slot, bounds(mesh), transform);
            }
        } else {
            for instance in changed {
                if let Some(slot) = slots.slot(instance.id).filter(|&slot| slot < limit) {
                    self.choose(lights, slot, bounds(instance.mesh), instance.transform);
                }
            }
        }
    }

    fn choose(&mut self, lights: &[GpuLight], slot: u32, sphere: Option<Sphere>, transform: Mat4) {
        debug_assert!(sphere.is_some(), "a mesh that was not uploaded to this renderer");
        let Some(sphere) = sphere else { return };
        self.chosen += 1;
        let list = choose(lights, &sphere.transformed(transform));
        let slot = slot as usize;
        if self.lists[slot] != list {
            self.lists[slot] = list;
            if !std::mem::replace(&mut self.queued[slot], true) {
                self.dirty.push(slot as u32);
            }
        }
    }

    /// Hands each run of consecutive changed lists to `write` (first slot, lists), then marks
    /// them uploaded.
    pub(crate) fn flush(&mut self, mut write: impl FnMut(u32, &[ObjectLightList])) {
        self.dirty.sort_unstable();
        for run in self.dirty.chunk_by(|&a, &b| b == a + 1) {
            let (first, last) = (run[0], run[run.len() - 1]);
            write(first, &self.lists[first as usize..=last as usize]);
        }
        for &slot in &self.dirty {
            self.queued[slot as usize] = false;
        }
        self.dirty.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lights::Rng;
    use crate::{InstanceDeltas, InstanceId, LightKind, LocalLight};

    fn point(position: Vec3, intensity: f32, range: f32) -> GpuLight {
        GpuLight::new(&LocalLight { position, color: Vec3::ONE, intensity, range, kind: LightKind::Point })
            .expect("valid")
    }

    const UNIT: Sphere = Sphere { center: Vec3::ZERO, radius: 1.0 };

    #[test]
    fn influence_peaks_inside_and_ends_at_the_range() {
        let inside = point(Vec3::new(0.5, 0.0, 0.0), 2.0, 3.0);
        assert!((influence(&inside, &UNIT) - 2.0).abs() < 1e-5, "full strength inside the sphere");
        // 2 m from the center is 1 m from the surface: a third of the range.
        let near = point(Vec3::new(2.0, 0.0, 0.0), 1.0, 3.0);
        let expected = (1.0f32 - 1.0 / 9.0).powi(2);
        assert!((influence(&near, &UNIT) - expected).abs() < 1e-5);
        assert_eq!(influence(&point(Vec3::new(4.0, 0.0, 0.0), 1.0, 3.0), &UNIT), 0.0, "just out of reach");
        // Brighter and nearer both count.
        assert!(influence(&point(Vec3::new(2.0, 0.0, 0.0), 2.0, 3.0), &UNIT) > influence(&near, &UNIT));
        assert!(influence(&point(Vec3::new(1.5, 0.0, 0.0), 1.0, 3.0), &UNIT) > influence(&near, &UNIT));
    }

    #[test]
    fn spots_influence_only_what_their_cone_reaches() {
        let spot = |direction: Vec3| {
            GpuLight::new(&LocalLight {
                position: Vec3::new(3.0, 0.0, 0.0),
                color: Vec3::ONE,
                intensity: 1.0,
                range: 5.0,
                kind: LightKind::Spot { direction, inner_angle: 0.1, outer_angle: 0.3 },
            })
            .expect("valid")
        };
        let aimed = influence(&spot(Vec3::NEG_X), &UNIT);
        assert!(
            (aimed - (1.0f32 - 4.0 / 25.0).powi(2)).abs() < 1e-5,
            "aimed at the sphere: the window alone"
        );
        assert_eq!(influence(&spot(Vec3::X), &UNIT), 0.0, "aimed away");
        // Aimed past its center: the sphere's angular radius (asin 1/3, about 0.34 rad) brings
        // its edge into the cone's soft edge at 0.55 rad, but not at 0.7.
        let past = |angle: f32| spot(Vec3::new(-angle.cos(), angle.sin(), 0.0));
        let grazing = influence(&past(0.55), &UNIT);
        assert!(grazing > 0.1 && grazing < aimed * 0.9, "{grazing}");
        assert_eq!(influence(&past(0.7), &UNIT), 0.0);
    }

    #[test]
    fn chooses_the_four_most_influential_in_index_order() {
        // Influence falls with distance; lights 1 and 4 are out of reach.
        let lights: Vec<GpuLight> = [2.0, 9.0, 1.5, 1.2, 9.5, 1.8, 2.5, 1.1]
            .into_iter()
            .map(|x| point(Vec3::new(x, 0.0, 0.0), 1.0, 3.0))
            .collect();
        assert_eq!(choose(&lights, &UNIT).lights(), [2, 3, 5, 7]);
        assert_eq!(choose(&lights[..3], &UNIT).lights(), [0, 2], "fewer than four in reach");
        assert_eq!(choose(&[], &UNIT).lights(), Vec::<u32>::new());
    }

    #[test]
    fn ties_go_to_the_lower_index() {
        // Eight equally influential lights around the sphere.
        let lights: Vec<GpuLight> = (0..8)
            .map(|i| {
                let angle = i as f32 * std::f32::consts::TAU / 8.0;
                point(Vec3::new(angle.cos(), angle.sin(), 0.0) * 2.0, 1.0, 3.0)
            })
            .collect();
        let influences: Vec<f32> = lights.iter().map(|light| influence(light, &UNIT)).collect();
        assert!(influences.windows(2).all(|pair| (pair[0] - pair[1]).abs() < 1e-5));
        // Equal to the last bit or not, the choice is the same on every run; exactly equal lights
        // (all at one spot) keep the first four.
        let same: Vec<GpuLight> = (0..8).map(|_| lights[0]).collect();
        assert_eq!(choose(&same, &UNIT).lights(), [0, 1, 2, 3]);
        assert_eq!(choose(&lights, &UNIT), choose(&lights, &UNIT));
        // A brighter late light displaces the last of the equals.
        let mut brighter = same.clone();
        brighter[6] = point(Vec3::new(2.0, 0.0, 0.0), 1.5, 3.0);
        assert_eq!(choose(&brighter, &UNIT).lights(), [0, 1, 2, 6]);
    }

    #[test]
    fn matches_a_brute_force_sort() {
        let mut rng = Rng(3);
        for _ in 0..200 {
            let lights: Vec<GpuLight> = (0..rng.range(0.0, 12.0) as usize)
                .map(|_| {
                    let position =
                        Vec3::new(rng.range(-5.0, 5.0), rng.range(-5.0, 5.0), rng.range(-5.0, 5.0));
                    point(position, rng.range(0.1, 3.0), rng.range(0.5, 6.0))
                })
                .collect();
            let mut ranked: Vec<(f32, u32)> = lights
                .iter()
                .enumerate()
                .map(|(index, light)| (influence(light, &UNIT), index as u32))
                .filter(|&(influence, _)| influence > 0.0)
                .collect();
            ranked.sort_by(|a, b| b.0.total_cmp(&a.0).then(a.1.cmp(&b.1)));
            let mut expected: Vec<u32> = ranked.iter().take(4).map(|&(_, index)| index).collect();
            expected.sort_unstable();
            assert_eq!(choose(&lights, &UNIT).lights(), expected);
        }
    }

    /// `influence` as it was before point lights skipped the cone (review RA-O04), operation for
    /// operation: what the fast path must match bit for bit.
    fn influence_before(light: &GpuLight, sphere: &Sphere) -> f32 {
        let offset = sphere.center - Vec3::from_slice(&light.position[..3]);
        let distance = offset.length();
        let gap = (distance - sphere.radius).max(0.0);
        let fade = (1.0 - gap * gap * light.radiance[3]).clamp(0.0, 1.0);
        if fade <= 0.0 {
            return 0.0;
        }
        let cone = if distance <= sphere.radius {
            1.0
        } else {
            let axis = Vec3::from_slice(&light.spot[..3]);
            let angle = (offset / distance).dot(axis).clamp(-1.0, 1.0).acos();
            let nearest = (angle - (sphere.radius / distance).asin()).max(0.0);
            let t = (nearest.cos() * light.cone[0] + light.cone[1]).clamp(0.0, 1.0);
            t * t * (3.0 - 2.0 * t)
        };
        luminance(Vec3::from_slice(&light.radiance[..3])) * fade * fade * cone
    }

    /// Seeded lights: points, spots of every width (full-angle ones among them, whose outer
    /// cosine is −1 as a point's is) and spots with a hard edge.
    fn seeded_lights(rng: &mut Rng, count: usize) -> Vec<GpuLight> {
        (0..count)
            .map(|index| {
                let position = Vec3::new(rng.range(-6.0, 6.0), rng.range(-6.0, 6.0), rng.range(-6.0, 6.0));
                let direction = Vec3::new(rng.range(-1.0, 1.0), rng.range(-1.0, 1.0), rng.range(-1.0, 1.0));
                let outer = match index % 4 {
                    0 | 1 => None,
                    2 => Some(rng.range(0.0, std::f32::consts::PI)),
                    _ => Some(std::f32::consts::PI),
                };
                let kind = match outer {
                    None => LightKind::Point,
                    Some(outer_angle) => {
                        // Every few a hard edge: the inner angle at the outer one.
                        let inner_angle =
                            if index % 3 == 0 { outer_angle } else { rng.range(0.0, outer_angle) };
                        LightKind::Spot { direction, inner_angle, outer_angle }
                    }
                };
                let light = LocalLight {
                    position,
                    color: Vec3::new(rng.range(0.1, 1.0), rng.range(0.1, 1.0), rng.range(0.1, 1.0)),
                    intensity: rng.range(0.1, 4.0),
                    range: rng.range(0.5, 8.0),
                    kind,
                };
                GpuLight::new(&light).expect("valid")
            })
            .collect()
    }

    /// Seeded spheres: around and away from the lights, from points to wider than a light's reach.
    fn seeded_spheres(rng: &mut Rng, count: usize) -> Vec<Sphere> {
        (0..count)
            .map(|index| {
                let center = Vec3::new(rng.range(-8.0, 8.0), rng.range(-8.0, 8.0), rng.range(-8.0, 8.0));
                let radius = match index % 5 {
                    0 => 0.0,
                    1 => rng.range(0.0, 0.1),
                    _ => rng.range(0.0, 4.0),
                };
                Sphere { center, radius }
            })
            .collect()
    }

    /// Spheres a degenerate transform makes: not finite.
    const DEGENERATE: [Sphere; 6] = [
        Sphere { center: Vec3::NAN, radius: 1.0 },
        Sphere { center: Vec3::new(1.0, f32::NAN, 0.0), radius: 1.0 },
        Sphere { center: Vec3::ZERO, radius: f32::NAN },
        Sphere { center: Vec3::NAN, radius: f32::NAN },
        Sphere { center: Vec3::new(f32::INFINITY, 0.0, 0.0), radius: 1.0 },
        Sphere { center: Vec3::ZERO, radius: f32::INFINITY },
    ];

    #[test]
    fn point_lights_skip_the_cone_bit_for_bit() {
        let mut rng = Rng(0x5eed_0004);
        let lights = seeded_lights(&mut rng, 64);
        let mut spheres = seeded_spheres(&mut rng, 512);
        spheres.extend(DEGENERATE);
        let mut points_outside = 0;
        for light in &lights {
            for sphere in &spheres {
                let (now, before) = (influence(light, sphere), influence_before(light, sphere));
                assert_eq!(
                    now.to_bits(),
                    before.to_bits(),
                    "{light:?} on {sphere:?}: {now} against {before}"
                );
                let distance = sphere.center.distance(Vec3::from_slice(&light.position[..3]));
                points_outside += u32::from(light.cone[0] == 0.0 && distance > sphere.radius && now > 0.0);
            }
        }
        assert!(points_outside > 1000, "the fast path ran: {points_outside} point-light pairs");
        // And so the same lists, for every finite sphere.
        for sphere in &spheres[..512] {
            let mut ranked: Vec<(f32, u32)> = lights
                .iter()
                .enumerate()
                .map(|(index, light)| (influence_before(light, sphere), index as u32))
                .filter(|&(influence, _)| influence > 0.0)
                .collect();
            ranked.sort_by(|a, b| b.0.total_cmp(&a.0).then(a.1.cmp(&b.1)));
            let mut expected: Vec<u32> = ranked.iter().take(4).map(|&(_, index)| index).collect();
            expected.sort_unstable();
            assert_eq!(choose(&lights, sphere).lights(), expected, "{sphere:?}");
        }
    }

    #[test]
    fn spheres_that_are_not_finite_get_no_lights() {
        let mut rng = Rng(0x5eed_0005);
        let lights = seeded_lights(&mut rng, 16);
        for sphere in DEGENERATE {
            assert_eq!(choose(&lights, &sphere).lights(), Vec::<u32>::new(), "{sphere:?}");
        }
        // Their instances keep empty lists through updates too.
        let mut lists = ObjectLists::default();
        let mut slots = InstanceSlots::default();
        let nan = MeshInstance { transform: Mat4::from_scale(Vec3::NAN), ..instance(1, 0.0) };
        apply(&mut slots, &[nan, instance(2, 0.0)]);
        let lights = [point(Vec3::new(0.0, 1.0, 0.0), 1.0, 2.0)];
        lists.update(&lights, true, &slots, &[], u32::MAX, |_| Some(UNIT));
        assert_eq!((lists.lists[0].lights(), lists.lists[1].lights()), (vec![], vec![0]));
    }

    #[test]
    fn slots_past_the_limit_get_no_list() {
        // Review RA-004: the list buffer holds two lists.
        let mut lists = ObjectLists::default();
        let mut slots = InstanceSlots::default();
        let three = [instance(1, 0.0), instance(2, 0.0), instance(3, 0.0)];
        apply(&mut slots, &three);
        let lights = [point(Vec3::new(0.0, 1.0, 0.0), 1.0, 2.0)];
        lists.update(&lights, true, &slots, &three, 2, |_| Some(UNIT));
        assert_eq!((lists.chosen(), lists.lists.len()), (2, 2));
        assert_eq!(flushed(&mut lists), [(0, 2)]);
        let moved = [instance(1, 0.1), instance(3, 0.1)];
        apply(&mut slots, &moved);
        lists.update(&lights, false, &slots, &moved, 2, |_| Some(UNIT));
        assert_eq!(lists.chosen(), 1, "only the moved instance within the limit");
    }

    #[test]
    fn only_point_lights_pack_a_zero_cone_scale() {
        let mut rng = Rng(0x5eed_0006);
        for light in seeded_lights(&mut rng, 4096) {
            let point = light.spot[..3] == [0.0; 3];
            assert_eq!(light.cone[0] == 0.0, point, "{light:?}");
        }
        // The widest spots: a full angle with a hard edge, and the hardest edge at no angle.
        for (inner_angle, outer_angle) in
            [(std::f32::consts::PI, std::f32::consts::PI), (0.0, 0.0), (0.0, 7.0)]
        {
            let kind = LightKind::Spot { direction: Vec3::Z, inner_angle, outer_angle };
            let spot =
                LocalLight { position: Vec3::ZERO, color: Vec3::ONE, intensity: 1.0, range: 1.0, kind };
            let light = GpuLight::new(&spot).expect("valid");
            assert!(light.cone[0] > 0.0, "{light:?}");
        }
    }

    #[test]
    fn packs_sixteen_bit_indices() {
        let list = ObjectLightList::new(&[0, 7, 300, 65_535]);
        assert_eq!(list.count, 4);
        assert_eq!(list.indices, [7 << 16, 300 | (65_535 << 16)]);
        assert_eq!(list.lights(), [0, 7, 300, 65_535]);
        assert_eq!(LIST_BYTES, 12);
    }

    fn instance(id: u64, x: f32) -> MeshInstance {
        MeshInstance {
            id: InstanceId(id),
            mesh: MeshHandle::from_raw(0),
            transform: Mat4::from_translation(Vec3::new(x, 0.0, 0.0)),
        }
    }

    fn apply(slots: &mut InstanceSlots, changed: &[MeshInstance]) {
        slots.apply(&InstanceDeltas { changed: changed.to_vec(), removed: Vec::new() });
    }

    /// Flushes, returning the runs written (first slot, list count).
    fn flushed(lists: &mut ObjectLists) -> Vec<(u32, usize)> {
        let mut runs = Vec::new();
        lists.flush(|first, written| runs.push((first, written.len())));
        runs
    }

    #[test]
    fn updates_choose_only_what_changed() {
        let mut lists = ObjectLists::default();
        let bounds = |mesh: MeshHandle| (mesh.index() == 0).then(|| Sphere::around([[-0.5; 3], [0.5; 3]]));
        let mut slots = InstanceSlots::default();
        let three = [instance(1, 0.0), instance(2, 10.0), instance(3, 20.0)];
        apply(&mut slots, &three);
        let lights = [point(Vec3::new(0.0, 1.0, 0.0), 1.0, 2.0), point(Vec3::new(20.0, 1.0, 0.0), 1.0, 2.0)];

        lists.update(&lights, true, &slots, &three, u32::MAX, bounds);
        assert_eq!(lists.chosen(), 3, "new lights: every instance");
        assert_eq!(lists.lists[0].lights(), [0]);
        assert_eq!(lists.lists[1].lights(), Vec::<u32>::new());
        assert_eq!(lists.lists[2].lights(), [1]);
        // Slot 1 is lit by nothing, which the GPU's zeroed buffer already says.
        assert_eq!(flushed(&mut lists), [(0, 1), (2, 1)]);

        // Same lights: only the instance that moved is chosen again.
        let moved = [instance(2, 0.5)];
        apply(&mut slots, &moved);
        lists.update(&lights, false, &slots, &moved, u32::MAX, bounds);
        assert_eq!(lists.chosen(), 1);
        assert_eq!(lists.lists[1].lights(), [0]);
        assert_eq!(flushed(&mut lists), [(1, 1)]);

        // Unchanged lists aren't uploaded again.
        lists.update(&lights, true, &slots, &[], u32::MAX, bounds);
        assert_eq!(lists.chosen(), 3);
        assert_eq!(flushed(&mut lists), []);

        // After a frame without a view, everything is chosen again.
        lists.invalidate();
        lists.update(&lights, false, &slots, &[], u32::MAX, bounds);
        assert_eq!(lists.chosen(), 3);
    }
}
