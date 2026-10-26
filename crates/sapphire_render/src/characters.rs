//! Per-character data on the GPU (spec §5.12 "Character light rig" and "Faces", M1 C2 and C3):
//! the light rig and the face of every character the frame lists ([`FramePacket::characters`]),
//! for the anime character shader.
//!
//! - **The buffer** (group 0 binding 3, fragment-visible) holds one [`GpuCharacter`] per
//!   character this frame, after entry 0: the fallback every other instance reads, the world's
//!   key light with unstyled GI ([`CharacterLighting::unstyled`]), a face at rest
//!   ([`CharacterFace::default`]) and no shadow probe. It is rewritten whole when any entry
//!   changes (while the hour runs, the camera moves for a rig whose key or GI depends on it, or a
//!   head, an expression or a body moves): 160 bytes a character. With shadows on, the face pass
//!   (M1 R4) writes each entry's `shadow.visibility` on the GPU every frame, after the upload.
//! - **Which entry an instance reads** is the `character` field of its instance data, which the
//!   vertex stage passes on as a flat varying. Entries follow the packet's order, so a character
//!   listed in the same place frame after frame keeps its entry and its instance data isn't
//!   uploaded again; one that moves in the list, joins it or leaves it is uploaded once.
//! - Frames without a main view draw nothing, so they leave every entry as it was.
//!
//! # Per-character data of later packages
//!
//! Each package's per-character data is a struct of its own inside [`GpuCharacter`] (WGSL
//! `CharacterData` in `frame.wgsl`): C2's is the rig, C3's the face beside it, R4's the shadow
//! (where the face pass samples the environment's shadow over the head, and what it found).
//! Later ones (C4's outline lighting) add theirs the same way. A character's data is then found
//! through one index and one binding: the fragment stage's fourth and last storage buffer (see
//! the crate docs, "Binding budget").

use bytemuck::{Pod, Zeroable};
use glam::Vec3;
use sapphire_rhi::wgpu;

use crate::buffer::{self, GrowableBuffer};
use crate::character_prepass::ID_ENTRY_MASK;
use crate::scene::InstanceSlots;
use crate::{CharacterBody, CharacterFace, CharacterLighting, FramePacket};

/// The radius of the sphere around a character's head that the face pass samples the
/// environment's shadow over (M1 R4), in meters: an anime head's.
pub(crate) const HEAD_PROBE_RADIUS: f32 = 0.12;

const ENTRY_BYTES: u64 = size_of::<GpuCharacter>() as u64;

/// One character's entry in the character buffer (WGSL `CharacterData`).
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Pod, Zeroable)]
pub(crate) struct GpuCharacter {
    pub rig: GpuRig,
    pub face: GpuFace,
    pub shadow: GpuCharacterShadow,
}

/// A character's light rig as the shader reads it (WGSL `CharacterRig`). 80 bytes.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Pod, Zeroable)]
pub(crate) struct GpuRig {
    /// xyz: unit vector toward the key light; w: the rim's scale.
    pub key_direction: [f32; 4],
    /// rgb: the key light's color × intensity; w: the local lights' luminance ceiling.
    pub key_radiance: [f32; 4],
    /// xyz: the head's center, world space; w: the GI's intensity.
    pub head: [f32; 4],
    /// x: the GI's camera bias; y: its saturation ceiling; z: its value ceiling; w: 1 where the
    /// head is known, so face and hair regions spherize, else 0.
    pub gi: [f32; 4],
    /// rgb: the GI's skin tint; w: 0.
    pub skin_tint: [f32; 4],
}

/// A character's face as the shader reads it (WGSL `CharacterFace`, M1 C3). 48 bytes.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Pod, Zeroable)]
pub(crate) struct GpuFace {
    /// xyz: unit head-forward, world space; w: 0.
    pub forward: [f32; 4],
    /// xyz: unit head-up, world space, perpendicular to forward; w: 0.
    pub up: [f32; 4],
    /// x: blush; y: gloom; zw: 0.
    pub expression: [f32; 4],
}

/// A character's shadow as the shaders read it (WGSL `CharacterShadow`, M1 R4). 32 bytes.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Pod, Zeroable)]
pub(crate) struct GpuCharacterShadow {
    /// xyz: the head's center, world space; w: the radius the face pass samples over around it,
    /// 0 for no head (the face stays lit).
    pub probe: [f32; 4],
    /// x: the key light's visibility there past environment casters, 1 as uploaded; the face pass
    /// writes it on the GPU. yzw: 0.
    pub visibility: [f32; 4],
}

impl GpuCharacterShadow {
    /// Where the face pass samples for a character with `lighting` and `body`: its rig's head if
    /// it has one, else the top of its body, else nowhere.
    fn new(lighting: &CharacterLighting, body: &CharacterBody) -> Self {
        let head = lighting.gi.head.or(body.casts().then_some(body.top));
        let probe = head.map_or([0.0; 4], |head| head.extend(HEAD_PROBE_RADIUS).to_array());
        Self { probe, visibility: [1.0, 0.0, 0.0, 0.0] }
    }
}

impl GpuFace {
    /// `face` packed, its head orthonormalized: at rest if it has no basis, which valid faces
    /// do.
    fn new(face: &CharacterFace) -> Self {
        let (forward, up) = face.head.orthonormalized().unwrap_or((Vec3::Z, Vec3::Y));
        let expression = face.expression;
        Self {
            forward: forward.extend(0.0).to_array(),
            up: up.extend(0.0).to_array(),
            expression: [expression.blush, expression.gloom, 0.0, 0.0],
        }
    }
}

impl GpuCharacter {
    /// The entry of a character lit by `lighting`, with `face` and `body`.
    fn new(lighting: &CharacterLighting, face: &CharacterFace, body: &CharacterBody) -> Self {
        let CharacterLighting { key, rim, local_light_ceiling, gi } = *lighting;
        let toward_key = (-key.direction).try_normalize().unwrap_or(Vec3::Y);
        Self {
            rig: GpuRig {
                key_direction: toward_key.extend(rim).to_array(),
                key_radiance: (key.color * key.intensity).extend(local_light_ceiling).to_array(),
                head: gi.head.unwrap_or(Vec3::ZERO).extend(gi.intensity).to_array(),
                // No ceiling is the largest finite one: the shader divides by the value, never
                // by the ceiling, so it stays finite.
                gi: [
                    gi.camera_bias,
                    gi.max_saturation,
                    gi.max_value.min(f32::MAX),
                    if gi.head.is_some() { 1.0 } else { 0.0 },
                ],
                skin_tint: gi.skin_tint.extend(0.0).to_array(),
            },
            face: GpuFace::new(face),
            shadow: GpuCharacterShadow::new(lighting, body),
        }
    }
}

/// The renderer's per-character data: this frame's entries and the buffer that holds them.
#[derive(Debug)]
pub(crate) struct Characters {
    list: CharacterList,
    /// The entries differ from what the buffer holds.
    unsent: bool,
    buffer: GrowableBuffer,
}

impl Characters {
    pub(crate) fn new(device: &wgpu::Device) -> Self {
        let limits = device.limits();
        let max = limits.max_storage_buffer_binding_size.min(limits.max_buffer_size);
        let usage = wgpu::BufferUsages::STORAGE;
        let buffer = GrowableBuffer::new(device, "characters", usage, 16 * ENTRY_BYTES, max);
        Self { list: CharacterList::default(), unsent: false, buffer }
    }

    /// Builds the frame's entries from `packet` and points the instances in `slots` (which
    /// already has the packet's instance deltas applied) at them; see [`CharacterList::update`].
    /// Returns `true` when the buffer was replaced, so bind groups that use it must be rebuilt.
    pub(crate) fn prepare(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        packet: &FramePacket,
        slots: &mut InstanceSlots,
    ) -> bool {
        profiling::scope!("Characters::prepare");
        if packet.main_view().is_none() {
            return false;
        }
        self.unsent |= self.list.update(packet, slots);
        // The entries are uploaded whole whenever they change, so nothing needs keeping.
        let bytes = self.list.entries.len() as u64 * ENTRY_BYTES;
        let replaced = self.buffer.reserve(device, queue, bytes, 0);
        self.unsent |= replaced;
        replaced
    }

    /// Group 0 binding 3.
    pub(crate) fn buffer(&self) -> &wgpu::Buffer {
        self.buffer.buffer()
    }

    /// Entries the last frame with a view wrote: the fallback, then one per character.
    pub(crate) fn entries(&self) -> u32 {
        u32::try_from(self.list.entries.len()).expect("fewer than 2³² characters")
    }

    /// Instances the last frame with a view lit with their own rigs.
    pub(crate) fn lit(&self) -> u32 {
        self.list.lit
    }

    /// Entries the last frame with a view ignored: for instances it doesn't draw, with lighting
    /// or a face that isn't valid, or past the 16,383 entries the character prepass's IDs hold.
    pub(crate) fn ignored(&self) -> u32 {
        self.list.ignored
    }

    /// Times the buffer grew.
    pub(crate) fn growths(&self) -> u32 {
        self.buffer.growths()
    }

    /// The entry the instance in `slot` reads this frame: 0 unless the frame lists it.
    pub(crate) fn entry(&self, slot: u32) -> u32 {
        self.list.next.get(slot as usize).copied().unwrap_or(0)
    }

    /// Splits the characters between a frame's passes: the uploads pass writes the entries, and
    /// the character prepass draws the instances with one, by slot (each slot's entry, 0 for
    /// none; slots past its end have none).
    pub(crate) fn split_for_record(&mut self) -> (CharacterUpload<'_>, &[u32]) {
        let upload = CharacterUpload {
            entries: &self.list.entries,
            unsent: &mut self.unsent,
            buffer: self.buffer.buffer(),
        };
        (upload, &self.list.next)
    }
}

/// The characters' share of a frame's uploads pass.
pub(crate) struct CharacterUpload<'a> {
    entries: &'a [GpuCharacter],
    unsent: &'a mut bool,
    buffer: &'a wgpu::Buffer,
}

impl CharacterUpload<'_> {
    /// Writes the entries if they changed since the buffer was last written, through `belt`.
    pub(crate) fn write(self, belt: &mut wgpu::util::StagingBelt, encoder: &mut wgpu::CommandEncoder) {
        if std::mem::take(self.unsent) {
            buffer::stage(belt, encoder, self.buffer, 0, bytemuck::cast_slice(self.entries));
        }
    }
}

/// The entries and which instance reads which, on the CPU. Its vectors are reused, so a warm
/// frame allocates nothing.
#[derive(Debug, Default)]
struct CharacterList {
    /// The last frame with a view: the fallback, then one entry per character it listed.
    entries: Vec<GpuCharacter>,
    /// The frame before's, to tell whether they changed.
    previous: Vec<GpuCharacter>,
    /// The instance slots the last frame gave an entry, and scratch for this frame's.
    listed: Vec<u32>,
    listing: Vec<u32>,
    /// Per instance slot: the entry this frame gives it, 0 for none.
    next: Vec<u32>,
    /// Instances the last frame lit with their own rigs, and the entries it ignored.
    lit: u32,
    ignored: u32,
}

impl CharacterList {
    /// Builds the entries for `packet`: the fallback from its key light, with a face at rest,
    /// then each valid character of an instance in `slots`, in packet order. Points each of
    /// those instances at its entry, and every instance the last frame listed but this one
    /// doesn't back at the fallback. Returns whether the entries changed.
    fn update(&mut self, packet: &FramePacket, slots: &mut InstanceSlots) -> bool {
        std::mem::swap(&mut self.entries, &mut self.previous);
        self.entries.clear();
        let fallback = CharacterLighting::unstyled(packet.sun);
        self.entries.push(GpuCharacter::new(&fallback, &CharacterFace::default(), &CharacterBody::default()));
        // Slots are never given back, so the last frame's are all below this.
        self.next.resize(slots.len(), 0);
        for &slot in &self.listed {
            self.next[slot as usize] = 0;
        }
        self.listing.clear();
        (self.lit, self.ignored) = (0, 0);
        for character in &packet.characters {
            // An entry past the character prepass's ID bits would alias an earlier character's
            // (or none), so the frame's characters stop there.
            let room = self.entries.len() <= ID_ENTRY_MASK as usize;
            let slot = slots.slot(character.id).filter(|_| character.is_valid() && room);
            let Some(slot) = slot else {
                self.ignored += 1;
                continue;
            };
            // Every slot is 0 here until this frame gives it an entry: an instance listed again
            // takes its last entry, and counts once.
            if self.next[slot as usize] == 0 {
                self.lit += 1;
                self.listing.push(slot);
            }
            self.next[slot as usize] = u32::try_from(self.entries.len()).expect("fewer than 2³² characters");
            self.entries.push(GpuCharacter::new(&character.lighting, &character.face, &character.body));
        }
        for &slot in self.listed.iter().chain(&self.listing) {
            slots.set_character(slot, self.next[slot as usize]);
        }
        std::mem::swap(&mut self.listed, &mut self.listing);
        self.entries != self.previous
    }
}

#[cfg(test)]
mod tests {
    use glam::Mat4;

    use super::*;
    use crate::{
        CameraData, CharacterGi, CharacterInstance, DirectionalLight, FaceExpression, HeadBasis,
        InstanceDeltas, InstanceId, MeshHandle, MeshInstance, ViewData,
    };

    fn lighting(intensity: f32) -> CharacterLighting {
        let key = DirectionalLight { intensity, ..DirectionalLight::default() };
        CharacterLighting::unstyled(key)
    }

    /// A packet with a view, listing `characters` as (instance, key intensity).
    fn packet(characters: &[(u64, f32)]) -> FramePacket {
        let camera = CameraData::look_at(Vec3::Z, Vec3::ZERO, Vec3::Y, 1.0, 1.0, 0.1);
        let characters = characters
            .iter()
            .map(|&(id, intensity)| CharacterInstance {
                id: InstanceId(id),
                lighting: lighting(intensity),
                face: CharacterFace::default(),
                body: CharacterBody::default(),
            })
            .collect();
        FramePacket { views: vec![ViewData::new(camera)], characters, ..FramePacket::default() }
    }

    fn slots(ids: std::ops::Range<u64>) -> InstanceSlots {
        let changed = ids
            .map(|id| MeshInstance {
                id: InstanceId(id),
                mesh: MeshHandle::from_raw(0),
                transform: Mat4::IDENTITY,
            })
            .collect();
        let mut slots = InstanceSlots::default();
        slots.apply(&InstanceDeltas { changed, removed: Vec::new() });
        slots
    }

    #[test]
    fn entries_pack_the_rig_for_the_shader() {
        let lighting = CharacterLighting {
            key: DirectionalLight {
                direction: Vec3::new(0.0, -2.0, 0.0),
                color: Vec3::new(1.0, 0.5, 0.25),
                intensity: 2.0,
            },
            rim: 0.5,
            local_light_ceiling: 1.5,
            gi: CharacterGi {
                head: Some(Vec3::new(1.0, 2.0, 3.0)),
                intensity: 0.8,
                camera_bias: 0.3,
                max_saturation: 0.6,
                max_value: f32::INFINITY,
                skin_tint: Vec3::new(1.1, 0.9, 0.8),
            },
        };
        let none = CharacterBody::default();
        let GpuCharacter { rig, face, shadow } =
            GpuCharacter::new(&lighting, &CharacterFace::default(), &none);
        assert_eq!(rig.key_direction, [0.0, 1.0, 0.0, 0.5], "toward the key, unit length; the rim");
        assert_eq!(rig.key_radiance, [2.0, 1.0, 0.5, 1.5]);
        assert_eq!(rig.head, [1.0, 2.0, 3.0, 0.8]);
        assert_eq!(rig.gi, [0.3, 0.6, f32::MAX, 1.0], "no value ceiling is the largest finite one");
        let headless = CharacterLighting { gi: CharacterGi::default(), ..lighting };
        let headless = GpuCharacter::new(&headless, &CharacterFace::default(), &none);
        assert_eq!((headless.rig.head, headless.rig.gi[3]), ([0.0, 0.0, 0.0, 1.0], 0.0), "nothing spherizes");
        assert_eq!(rig.skin_tint, [1.1, 0.9, 0.8, 0.0]);
        assert_eq!(face.forward, [0.0, 0.0, 1.0, 0.0], "a face at rest");
        assert_eq!((face.up, face.expression), ([0.0, 1.0, 0.0, 0.0], [0.0; 4]));
        // The face pass samples around the rig's head; without one, the body's top; else nowhere.
        assert_eq!(shadow.probe, [1.0, 2.0, 3.0, HEAD_PROBE_RADIUS]);
        assert_eq!(shadow.visibility, [1.0, 0.0, 0.0, 0.0], "lit until the face pass says otherwise");
        assert_eq!(headless.shadow.probe, [0.0; 4]);
        let body = CharacterBody::capsule(Vec3::ZERO, Vec3::new(0.0, 1.5, 0.0), 0.3);
        let bodied = GpuCharacter::new(&headless_lighting(), &CharacterFace::default(), &body);
        assert_eq!(bodied.shadow.probe, [0.0, 1.5, 0.0, HEAD_PROBE_RADIUS]);
    }

    fn headless_lighting() -> CharacterLighting {
        CharacterLighting { gi: CharacterGi::default(), ..lighting(1.0) }
    }

    #[test]
    fn entries_pack_the_face_orthonormalized() {
        let face = CharacterFace {
            head: HeadBasis { forward: Vec3::new(3.0, 0.0, 0.0), up: Vec3::new(1.0, 2.0, 0.0) },
            expression: FaceExpression { blush: 0.25, gloom: 2.0 },
        };
        let packed = GpuCharacter::new(&lighting(1.0), &face, &CharacterBody::default()).face;
        assert_eq!(packed.forward, [1.0, 0.0, 0.0, 0.0], "unit length");
        assert_eq!(packed.up, [0.0, 1.0, 0.0, 0.0], "square to forward");
        assert_eq!(packed.expression, [0.25, 2.0, 0.0, 0.0], "as given: the shader clamps");
    }

    #[test]
    fn only_valid_lighting_is_drawn() {
        let valid = lighting(1.0);
        assert!(valid.is_valid());
        let key = valid.key;
        let gi = valid.gi;
        let broken = [
            CharacterLighting { rim: f32::NAN, ..valid },
            CharacterLighting { local_light_ceiling: -1.0, ..valid },
            CharacterLighting { key: DirectionalLight { direction: Vec3::ZERO, ..key }, ..valid },
            CharacterLighting { key: DirectionalLight { intensity: f32::INFINITY, ..key }, ..valid },
            CharacterLighting { gi: CharacterGi { head: Some(Vec3::splat(f32::NAN)), ..gi }, ..valid },
            CharacterLighting { gi: CharacterGi { max_value: f32::NAN, ..gi }, ..valid },
            CharacterLighting { gi: CharacterGi { skin_tint: Vec3::new(1.0, -0.1, 1.0), ..gi }, ..valid },
        ];
        for lighting in broken {
            assert!(!lighting.is_valid(), "{lighting:?}");
        }
    }

    #[test]
    fn listed_instances_read_their_entries_and_the_rest_the_fallback() {
        let mut slots = slots(0..4);
        let mut list = CharacterList::default();
        let entry =
            |slots: &InstanceSlots, id: u64| slots.character(slots.slot(InstanceId(id)).expect("live"));
        // Instance 9 isn't drawn, and the second entry for 2 replaces the first.
        assert!(list.update(&packet(&[(1, 2.0), (9, 1.0), (2, 3.0), (2, 4.0)]), &mut slots));
        assert_eq!((list.lit, list.ignored), (2, 1), "instance 2 counts once");
        assert_eq!([0, 1, 2, 3].map(|id| entry(&slots, id)), [0, 1, 3, 0]);
        let intensity = |list: &CharacterList, index: usize| list.entries[index].rig.key_radiance[0];
        assert_eq!(intensity(&list, 0), 1.0, "the fallback is the packet's key light");
        assert_eq!(intensity(&list, 3), 4.0);

        // The same list again: nothing changes, so nothing is uploaded.
        assert!(!list.update(&packet(&[(1, 2.0), (9, 1.0), (2, 3.0), (2, 4.0)]), &mut slots));
        // Instance 1 leaves the list and 3 joins: 1 falls back, 2 and 3 move.
        assert!(list.update(&packet(&[(3, 1.0), (2, 1.0)]), &mut slots));
        assert_eq!([0, 1, 2, 3].map(|id| entry(&slots, id)), [0, 0, 2, 1]);
        assert_eq!(list.lit, 2);
        assert!(list.update(&packet(&[]), &mut slots));
        assert_eq!([0, 1, 2, 3].map(|id| entry(&slots, id)), [0; 4]);
        assert_eq!(list.entries.len(), 1, "the fallback alone");
    }

    #[test]
    fn characters_stop_at_the_prepass_ids_entries() {
        let count = ID_ENTRY_MASK as u64 + 2;
        let mut slots = slots(0..count);
        let mut list = CharacterList::default();
        let characters: Vec<(u64, f32)> = (0..count).map(|id| (id, 1.0)).collect();
        list.update(&packet(&characters), &mut slots);
        assert_eq!((list.lit, list.ignored), (ID_ENTRY_MASK, 2));
        assert_eq!(list.entries.len(), ID_ENTRY_MASK as usize + 1, "the fallback and the rest");
        assert_eq!(slots.character(slots.slot(InstanceId(count - 1)).expect("live")), 0, "unlit");
    }

    #[test]
    fn invalid_lighting_falls_back() {
        let mut slots = slots(0..1);
        let mut list = CharacterList::default();
        let mut frame = packet(&[(0, 1.0)]);
        frame.characters[0].lighting.rim = -1.0;
        list.update(&frame, &mut slots);
        assert_eq!((list.lit, list.ignored, slots.character(0)), (0, 1, 0));
    }

    #[test]
    fn an_invalid_face_falls_back_and_a_new_face_is_a_change() {
        let mut slots = slots(0..1);
        let mut list = CharacterList::default();
        let mut frame = packet(&[(0, 1.0)]);
        assert!(list.update(&frame, &mut slots));
        frame.characters[0].face.expression.blush = 0.5;
        assert!(list.update(&frame, &mut slots), "an expression moved");
        assert_eq!(list.entries[1].face.expression[0], 0.5);
        frame.characters[0].face.head = HeadBasis { forward: Vec3::ZERO, up: Vec3::Y };
        list.update(&frame, &mut slots);
        assert_eq!((list.lit, list.ignored, slots.character(0)), (0, 1, 0));
    }
}
