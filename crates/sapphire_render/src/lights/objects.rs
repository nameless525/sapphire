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
//! cooked bounds, kept here per mesh until meshes carry them for culling.

use bytemuck::{Pod, Zeroable};
use glam::{Mat4, Vec3};

use super::{GpuLight, luminance};
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

/// A bounding sphere.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Sphere {
    pub center: Vec3,
    pub radius: f32,
}

impl Sphere {
    /// The sphere around an object-space box (`[min, max]`, as meshes are cooked with).
    pub(crate) fn around([min, max]: [[f32; 3]; 2]) -> Self {
        let (min, max) = (Vec3::from(min), Vec3::from(max));
        Self { center: (min + max) * 0.5, radius: (max - min).length() * 0.5 }
    }

    /// This sphere after `transform`, grown by its largest scale so it still holds the mesh.
    pub(crate) fn transformed(self, transform: Mat4) -> Self {
        let scale = transform
            .x_axis
            .truncate()
            .length()
            .max(transform.y_axis.truncate().length())
            .max(transform.z_axis.truncate().length());
        Self { center: transform.transform_point3(self.center), radius: self.radius * scale }
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
    let cone = if distance <= sphere.radius {
        1.0
    } else {
        // The smallest angle off the spot's axis that reaches the sphere: the angle to its
        // center less its angular radius. Point lights have no axis, and a cone factor of 1.
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
    // The best so far, most influential first.
    let mut best = [(0.0f32, 0u32); SLOTS];
    let mut len = 0;
    for (index, light) in lights.iter().enumerate() {
        let influence = influence(light, sphere);
        // Also skips NaN, from a degenerate transform.
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

/// Every instance slot's list and the bounding spheres they are chosen with.
#[derive(Debug, Default)]
pub(crate) struct ObjectLists {
    /// Object-space bounding sphere per mesh, by handle index.
    meshes: Vec<Sphere>,
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
    pub(crate) fn mesh_uploaded(&mut self, mesh: MeshHandle, bounds: [[f32; 3]; 2]) {
        let index = mesh.index();
        if self.meshes.len() <= index {
            self.meshes.resize(index + 1, Sphere { center: Vec3::ZERO, radius: 0.0 });
        }
        self.meshes[index] = Sphere::around(bounds);
    }

    /// Makes the next update choose every instance's list, as after a frame whose lights it
    /// never saw.
    pub(crate) fn invalidate(&mut self) {
        self.stale = true;
    }

    /// Lists chosen by the last update.
    pub(crate) fn chosen(&self) -> u32 {
        self.chosen
    }

    /// Chooses lists from `lights`: every live instance's if the lights changed since the last
    /// update (or it was invalidated), otherwise only the `changed` instances' (the packet's
    /// additions and changes, already applied to `slots`).
    pub(crate) fn update(
        &mut self,
        lights: &[GpuLight],
        lights_changed: bool,
        slots: &InstanceSlots,
        changed: &[MeshInstance],
    ) {
        profiling::scope!("ObjectLists::update");
        self.lists.resize(slots.len(), ObjectLightList::default());
        self.queued.resize(slots.len(), false);
        self.chosen = 0;
        if std::mem::take(&mut self.stale) || lights_changed {
            for (slot, mesh, transform) in slots.live_transforms() {
                self.choose(lights, slot, mesh, transform);
            }
        } else {
            for instance in changed {
                if let Some(slot) = slots.slot(instance.id) {
                    self.choose(lights, slot, instance.mesh, instance.transform);
                }
            }
        }
    }

    fn choose(&mut self, lights: &[GpuLight], slot: u32, mesh: MeshHandle, transform: Mat4) {
        let sphere = self.meshes.get(mesh.index());
        debug_assert!(sphere.is_some(), "{mesh:?} was not uploaded to this renderer");
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

    #[test]
    fn packs_sixteen_bit_indices() {
        let list = ObjectLightList::new(&[0, 7, 300, 65_535]);
        assert_eq!(list.count, 4);
        assert_eq!(list.indices, [7 << 16, 300 | (65_535 << 16)]);
        assert_eq!(list.lights(), [0, 7, 300, 65_535]);
        assert_eq!(LIST_BYTES, 12);
    }

    #[test]
    fn spheres_follow_their_instances() {
        let sphere = Sphere::around([[-1.0, 0.0, -1.0], [1.0, 2.0, 1.0]]);
        assert_eq!(sphere.center, Vec3::Y);
        assert!((sphere.radius - 3f32.sqrt()).abs() < 1e-6);
        let moved = sphere.transformed(Mat4::from_scale_rotation_translation(
            Vec3::new(1.0, 3.0, 1.0),
            glam::Quat::from_rotation_y(1.0),
            Vec3::new(5.0, 0.0, 0.0),
        ));
        assert!((moved.center - Vec3::new(5.0, 3.0, 0.0)).length() < 1e-5);
        assert!((moved.radius - 3.0 * 3f32.sqrt()).abs() < 1e-5, "the largest scale");
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
        lists.mesh_uploaded(MeshHandle::from_raw(0), [[-0.5; 3], [0.5; 3]]);
        let mut slots = InstanceSlots::default();
        let three = [instance(1, 0.0), instance(2, 10.0), instance(3, 20.0)];
        apply(&mut slots, &three);
        let lights = [point(Vec3::new(0.0, 1.0, 0.0), 1.0, 2.0), point(Vec3::new(20.0, 1.0, 0.0), 1.0, 2.0)];

        lists.update(&lights, true, &slots, &three);
        assert_eq!(lists.chosen(), 3, "new lights: every instance");
        assert_eq!(lists.lists[0].lights(), [0]);
        assert_eq!(lists.lists[1].lights(), Vec::<u32>::new());
        assert_eq!(lists.lists[2].lights(), [1]);
        // Slot 1 is lit by nothing, which the GPU's zeroed buffer already says.
        assert_eq!(flushed(&mut lists), [(0, 1), (2, 1)]);

        // Same lights: only the instance that moved is chosen again.
        let moved = [instance(2, 0.5)];
        apply(&mut slots, &moved);
        lists.update(&lights, false, &slots, &moved);
        assert_eq!(lists.chosen(), 1);
        assert_eq!(lists.lists[1].lights(), [0]);
        assert_eq!(flushed(&mut lists), [(1, 1)]);

        // Unchanged lists aren't uploaded again.
        lists.update(&lights, true, &slots, &[]);
        assert_eq!(lists.chosen(), 3);
        assert_eq!(flushed(&mut lists), []);

        // After a frame without a view, everything is chosen again.
        lists.invalidate();
        lists.update(&lights, false, &slots, &[]);
        assert_eq!(lists.chosen(), 3);
    }
}
