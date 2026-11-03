//! The party's looks (`--party`, M2 G7): one character in three palettes, and which of them draws.
//!
//! The party's second and third members are recolors of the character, with no new source. Each is
//! a copy of five of its materials, the hair, back hair, top, bottoms and shoes, with their
//! `base_color` multiplied by [`TINTS`], over the same maps, ramp and everything else, and a copy of its
//! mesh whose slots name them. They're made as the character loads, as `--hair-transmission`'s copies are,
//! so no file is added and nothing is cooked: `shared.spack` stays byte for byte what it was, and only
//! `--party` pays for them, in ten material blocks and two more of the mesh's buffers.
//!
//! A game's party would be three characters, each with a mesh of its own, so sharing those buffers
//! here would be work for nothing after M2.
//!
//! One entity draws whoever's on the field: [`Drawn`] gives it that member's mesh when a swap changes
//! it, which sends its pose again (`sapphire_stage`). `crate::arena`'s player reads the same member, for
//! its place and for what its graph plays, and cuts that graph at a swap, so no pose blends from one
//! member into another. All three share a skeleton, a graph and the character's blade, so nothing else knows
//! there's a party.

use std::collections::HashMap;

use sapphire_assets::material::{ArchivedMaterial, Material};
use sapphire_assets::mesh::ArchivedMesh;
use sapphire_assets::{AssetId, UploadError};
use sapphire_gameplay::{PARTY_SLOTS, Party};
use sapphire_render::texture::GpuTexture;
use sapphire_render::{MaterialHandle, MeshHandle, Renderer};

use sapphire_stage::content::UploadWatcher;

/// The looks beyond the character's own.
const LOOKS: usize = PARTY_SLOTS - 1;

/// The materials a look recolors, by their sidecars' IDs (`content/lookdev/character/materials/`).
const HAIR: AssetId = AssetId::from_text("ee5a0b89-c2d9-4e03-84eb-32b868f69f18");
const HAIR_BACK: AssetId = AssetId::from_text("fc60337f-24eb-4e2f-976f-7d8de565c77e");
const TOP: AssetId = AssetId::from_text("7a353fd4-8f93-46e3-b333-131fc05daf7c");
const BOTTOMS: AssetId = AssetId::from_text("973835c8-7707-41eb-afda-f50ee979b42f");
const SHOES: AssetId = AssetId::from_text("535adb27-24ff-4c18-a7ef-a704640efd71");

/// A look's multipliers on `base_color`. The shirt's map is white, so its tint is its colour. The hair's
/// map is a red-brown with little blue in it and the shorts' and shoes' are nearly black, so those
/// tints reach well past 1 to get anywhere.
struct Tint {
    hair: [f32; 3],
    top: [f32; 3],
    bottoms: [f32; 3],
    shoes: [f32; 3],
}

/// The second and third members. A combat frame shows a figure a couple of hundred pixels tall, where a
/// shirt and its hair are what read, so each takes a hue of its own in both: the character's own is a
/// white shirt under chestnut hair, the second a crimson shirt under silver hair, the third a jade shirt
/// under violet hair.
const TINTS: [Tint; LOOKS] = [
    Tint {
        hair: [1.6, 3.2, 6.0],
        top: [0.62, 0.075, 0.09],
        bottoms: [3.2, 3.0, 3.6],
        shoes: [5.0, 1.0, 1.0],
    },
    Tint {
        hair: [0.42, 0.42, 1.9],
        top: [0.07, 0.42, 0.34],
        bottoms: [1.4, 3.0, 2.6],
        shoes: [1.0, 3.6, 2.8],
    },
];

impl Tint {
    fn of(&self, material: AssetId) -> Option<[f32; 3]> {
        [
            (HAIR, self.hair),
            (HAIR_BACK, self.hair),
            (TOP, self.top),
            (BOTTOMS, self.bottoms),
            (SHOES, self.shoes),
        ]
        .into_iter()
        .find_map(|(id, tint)| (id == material).then_some(tint))
    }
}

/// What the renderer holds of one look, each with the generation it was made from.
#[derive(Default)]
struct Look {
    materials: HashMap<AssetId, (u32, MaterialHandle)>,
    mesh: Option<(u32, MeshHandle)>,
}

/// The looks' copies on one renderer. `sapphire_stage::content` shows it what it uploads.
#[derive(Default)]
pub struct PartyLooks {
    looks: [Look; LOOKS],
    prepared_material: Option<PreparedMaterial>,
    prepared_character: Option<PreparedCharacter>,
}

/// Only handles created for this upload are owned here; published copies remain untouched.
struct PreparedMaterial {
    id: AssetId,
    generation: u32,
    materials: [Option<MaterialHandle>; LOOKS],
}

/// Uploads stay private until the primary character and every copy can be adopted together.
struct PreparedCharacter {
    generation: u32,
    meshes: [Option<MeshHandle>; LOOKS],
}

impl UploadWatcher for PartyLooks {
    fn prepare_material<'t>(
        &mut self,
        id: AssetId,
        generation: u32,
        material: &ArchivedMaterial,
        textures: &dyn Fn(AssetId) -> Option<&'t GpuTexture>,
        renderer: &mut Renderer,
    ) -> Result<Vec<(MaterialHandle, MaterialHandle)>, UploadError> {
        self.discard_material(renderer);
        self.prepared_material = Some(PreparedMaterial { id, generation, materials: [None; LOOKS] });
        let prepared = self.prepared_material.as_mut().expect("preparation began above");
        let mut replacements = Vec::with_capacity(LOOKS);
        for ((look, tint), pending) in self.looks.iter().zip(&TINTS).zip(&mut prepared.materials) {
            let Some(tint) = tint.of(id) else { continue };
            let held = look.materials.get(&id);
            if held.is_some_and(|held| held.0 >= generation) {
                continue;
            }
            let mut copy = material.to_material()?;
            recolor(&mut copy, tint);
            let bytes = copy.to_cooked()?;
            let copy = Material::read(&bytes)?;
            let handle = renderer.upload_material(copy, textures)?;
            *pending = Some(handle);
            if let Some((_, existing)) = held {
                replacements.push((*existing, handle));
            }
        }
        Ok(replacements)
    }

    fn commit_material(&mut self) {
        let Some(prepared) = self.prepared_material.take() else { return };
        for (look, pending) in self.looks.iter_mut().zip(prepared.materials) {
            if let Some(handle) = pending {
                let stable = look.materials.get(&prepared.id).map_or(handle, |held| held.1);
                look.materials.insert(prepared.id, (prepared.generation, stable));
            }
        }
    }

    fn discard_material(&mut self, renderer: &mut Renderer) {
        if let Some(prepared) = self.prepared_material.take() {
            for handle in prepared.materials.into_iter().flatten() {
                renderer.free_material(handle);
            }
        }
    }

    fn prepare_character(
        &mut self,
        generation: u32,
        mesh: &ArchivedMesh,
        materials: &dyn Fn(AssetId) -> Option<MaterialHandle>,
        renderer: &mut Renderer,
    ) -> Result<Vec<(MeshHandle, MeshHandle)>, UploadError> {
        self.discard_character(renderer);
        self.prepared_character = Some(PreparedCharacter { generation, meshes: [None; LOOKS] });
        let prepared = self.prepared_character.as_mut().expect("preparation began above");
        let mut replacements = Vec::with_capacity(LOOKS);
        for (look, pending) in self.looks.iter().zip(&mut prepared.meshes) {
            if look.mesh.is_some_and(|held| held.0 >= generation) {
                continue;
            }
            let resolve =
                |material| look.materials.get(&material).map(|held| held.1).or_else(|| materials(material));
            let handle = renderer.upload_mesh_with_slots(mesh, resolve)?;
            *pending = Some(handle);
            if let Some((_, existing)) = look.mesh {
                replacements.push((existing, handle));
            }
        }
        Ok(replacements)
    }

    fn commit_character(&mut self) {
        let Some(prepared) = self.prepared_character.take() else { return };
        for (look, pending) in self.looks.iter_mut().zip(prepared.meshes) {
            if let Some(handle) = pending {
                let stable = look.mesh.map_or(handle, |held| held.1);
                look.mesh = Some((prepared.generation, stable));
            }
        }
    }

    fn discard_character(&mut self, renderer: &mut Renderer) {
        if let Some(prepared) = self.prepared_character.take() {
            for handle in prepared.meshes.into_iter().flatten() {
                renderer.free_mesh(handle);
            }
        }
    }

    /// A new renderer holds none of the looks' copies, and they come again as it uploads.
    fn for_new_renderer(&self) -> Box<dyn UploadWatcher> {
        Box::new(Self::default())
    }
}

impl PartyLooks {
    /// Each member's mesh, the first being `character`'s, once the character has loaded.
    pub fn meshes(&self, character: MeshHandle) -> Option<[MeshHandle; PARTY_SLOTS]> {
        let [second, third] = &self.looks;
        Some([character, second.mesh?.1, third.mesh?.1])
    }
}

/// Multiplies an anime character material's `base_color` by `tint`.
fn recolor(material: &mut Material, tint: [f32; 3]) {
    material.edit_anime_character(|params| {
        for (channel, by) in params.base_color.iter_mut().zip(tint) {
            *channel *= by;
        }
    });
}

/// Which of the members' meshes the character's entity draws.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Drawn {
    meshes: [MeshHandle; PARTY_SLOTS],
    slot: usize,
}

impl Drawn {
    /// Starts on the first member.
    pub(crate) fn new(meshes: [MeshHandle; PARTY_SLOTS]) -> Self {
        Self { meshes, slot: 0 }
    }

    /// The same member's mesh on a new renderer's `meshes`, for after device loss or a tier change.
    pub(crate) fn on_new_renderer(self, meshes: [MeshHandle; PARTY_SLOTS]) -> Self {
        Self { meshes, ..self }
    }

    /// The mesh drawing now.
    pub(crate) fn mesh(self) -> MeshHandle {
        self.meshes[self.slot]
    }

    /// The mesh to change to, if `party`'s member on the field has changed since last asked.
    pub(crate) fn follow(&mut self, party: &Party) -> Option<MeshHandle> {
        let slot = party.active_slot().filter(|&slot| slot != self.slot)?;
        self.slot = slot;
        Some(self.mesh())
    }
}

#[cfg(test)]
mod tests {
    use sapphire_gameplay::replay::{SCENARIOS, find_scenario};
    use sapphire_gameplay::{ActorActions, Buttons, Stick, TickActions};

    use super::*;

    /// A later copy's validation/upload failure leaves both published copies at the old version.
    #[test]
    #[ignore = "needs a GPU; cargo test -p game_combat prepared_party_copies_publish_together -- --ignored"]
    fn prepared_party_copies_publish_together() {
        use std::cell::Cell;

        use sapphire_assets::material::AnimeCharacterMaterial;
        use sapphire_assets::mesh::{
            MaterialSlot, Mesh, PositionFormat, SlotMaterial, Submesh, VERTEX_COLOR_NEUTRAL, Vertex,
            VertexFlags,
        };
        use sapphire_assets::texture::Swizzle;
        use sapphire_math::Vec3;
        use sapphire_render::RenderSettings;
        use sapphire_rhi::{GpuContext, GpuContextDesc, GpuInstance, GpuInstanceDesc, wgpu};

        let instance = GpuInstance::new(&GpuInstanceDesc::default()).expect("instance");
        let gpu = GpuContext::new(&instance, &GpuContextDesc::default(), None).expect("device");
        let mut renderer =
            Renderer::new(&gpu, wgpu::TextureFormat::Rgba8UnormSrgb, RenderSettings::default())
                .expect("renderer");
        let mut packets = sapphire_render::FramePacketRing::new();
        let target = gpu.device().create_texture(&wgpu::TextureDescriptor {
            label: None,
            size: wgpu::Extent3d { width: 16, height: 16, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8UnormSrgb,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        });
        let view = target.create_view(&Default::default());
        let mut executor = sapphire_render::graph::GraphExecutor::new();
        let mut settle = |renderer: &mut Renderer| {
            // Free requests retire through ordinary frame fences, including material uses
            // released by a discarded mesh; wait and prepare again before counting slots.
            for _ in 0..3 {
                let camera =
                    sapphire_render::CameraData::look_at(Vec3::Z, Vec3::ZERO, Vec3::Y, 0.9, 1.0, 0.05);
                packets.begin().views.push(sapphire_render::ViewData::new(camera));
                renderer.prepare(&gpu, packets.current());
                let mut graph = sapphire_render::graph::FrameGraph::new();
                let output = graph.import_texture("target", &view);
                renderer.record(&mut graph, output);
                executor.execute(&gpu, graph).expect("empty frame");
                gpu.device().poll(wgpu::PollType::wait_indefinitely()).expect("finished frame");
            }
        };
        let material = |green| {
            Material::from_document(
                &AnimeCharacterMaterial { base_color: [1.0, green, 1.0, 1.0], ..Default::default() },
                &[Swizzle::IDENTITY; AnimeCharacterMaterial::TEXTURE_SLOTS.len()],
            )
            .expect("finite source material")
            .to_cooked()
            .expect("cooked")
        };
        let bytes = material(1.0);
        let source = Material::read(&bytes).expect("valid");
        let mut looks = PartyLooks::default();
        assert!(
            looks.prepare_material(SHOES, 1, source, &|_| None, &mut renderer).expect("copies").is_empty()
        );
        assert!(looks.looks.iter().all(|look| look.materials.is_empty()), "preparation is private");
        looks.commit_material();
        let stable = looks.looks.each_ref().map(|look| look.materials[&SHOES].1);
        let materials = renderer.stats().materials;

        // The silver look's green multiplier is 1; the violet look's is 3.6.
        let overflowing = material(f32::MAX / 2.0);
        let overflowing = Material::read(&overflowing).expect("the original is valid");
        assert!(looks.prepare_material(SHOES, 2, overflowing, &|_| None, &mut renderer).is_err());
        let pending = &looks.prepared_material.as_ref().expect("pending cleanup").materials;
        assert!(pending[0].is_some() && pending[1].is_none(), "the second copy fails");
        assert_eq!(looks.looks.each_ref().map(|look| look.materials[&SHOES]), stable.map(|h| (1, h)));
        looks.discard_material(&mut renderer);
        settle(&mut renderer);
        assert_eq!(renderer.stats().materials, materials, "the temporary copy is freed");
        let replacements = looks.prepare_material(SHOES, 2, source, &|_| None, &mut renderer).expect("retry");
        renderer.adopt_materials(&replacements).expect("compatible prepared copies");
        looks.commit_material();
        settle(&mut renderer);
        assert_eq!(looks.looks.each_ref().map(|look| look.materials[&SHOES]), stable.map(|h| (2, h)));
        assert_eq!(renderer.stats().materials, materials);

        let external = AssetId::from_text("00000000-0000-4000-8000-000000000001");
        let vertex = |position| Vertex {
            position,
            normal: [0.0, 0.0, 1.0],
            outline_normal: [0.0, 0.0, 1.0],
            uv: [0.0; 2],
            color: VERTEX_COLOR_NEUTRAL,
            flags: VertexFlags::NONE,
        };
        let mesh = Mesh::from_vertices(
            &[vertex([0.0, 0.0, 0.0]), vertex([1.0, 0.0, 0.0]), vertex([0.0, 1.0, 0.0])],
            vec![0, 1, 2],
            vec![Submesh { first_index: 0, index_count: 3, material: 0 }],
            vec![MaterialSlot { name: "external".into(), material: SlotMaterial::Asset(external) }],
            PositionFormat::Unorm16,
        )
        .to_cooked()
        .expect("cooked mesh");
        let mesh = Mesh::read(&mesh).expect("valid mesh");
        looks.prepare_character(1, mesh, &|_| Some(stable[0]), &mut renderer).expect("initial meshes");
        assert!(looks.meshes(MeshHandle::from_raw(0)).is_none());
        looks.commit_character();
        let meshes = looks.looks.each_ref().map(|look| look.mesh.expect("published").1);
        let count = renderer.stats().meshes;
        let calls = Cell::new(0);
        let fail_second = |_| {
            let call = calls.get();
            calls.set(call + 1);
            (call == 0).then_some(stable[0])
        };
        assert!(looks.prepare_character(2, mesh, &fail_second, &mut renderer).is_err());
        let pending = &looks.prepared_character.as_ref().expect("pending cleanup").meshes;
        assert!(pending[0].is_some() && pending[1].is_none());
        assert_eq!(looks.looks.each_ref().map(|look| look.mesh), meshes.map(|h| Some((1, h))));
        looks.discard_character(&mut renderer);
        settle(&mut renderer);
        assert_eq!(renderer.stats().meshes, count);
        let replacements =
            looks.prepare_character(2, mesh, &|_| Some(stable[0]), &mut renderer).expect("retry");
        renderer.adopt_meshes(&replacements).expect("prepared meshes");
        looks.commit_character();
        settle(&mut renderer);
        assert_eq!(looks.looks.each_ref().map(|look| look.mesh), meshes.map(|h| Some((2, h))));
        assert_eq!(renderer.stats().meshes, count);
    }

    /// A material's sidecar moving its ID would leave a look's tint matching nothing, with no error.
    #[test]
    fn the_tints_name_the_characters_materials() {
        let sidecar = |name: &str| {
            let path = format!(
                "{}/../../content/lookdev/character/materials/{name}.material.ron.meta",
                env!("CARGO_MANIFEST_DIR"),
            );
            let text = std::fs::read_to_string(&path).expect("the sidecar reads");
            let id = text.split("id: \"").nth(1).and_then(|rest| rest.split('"').next());
            AssetId::from_text(id.expect("the sidecar names an ID"))
        };
        assert_eq!(sidecar("Hair_00_HAIR"), HAIR);
        assert_eq!(sidecar("HairBack_00_HAIR"), HAIR_BACK);
        assert_eq!(sidecar("Tops_01_CLOTH"), TOP);
        assert_eq!(sidecar("Bottoms_01_CLOTH"), BOTTOMS);
        assert_eq!(sidecar("Shoes_01_CLOTH"), SHOES);
    }

    /// A look takes a hue of its own on what reads in a fight, and leaves skin and eyes as they are.
    #[test]
    fn each_look_recolors_its_own_materials_only() {
        let (second, third) = (&TINTS[0], &TINTS[1]);
        for material in [HAIR, HAIR_BACK, TOP, BOTTOMS, SHOES] {
            assert!(second.of(material).is_some() && third.of(material).is_some());
            assert_ne!(second.of(material), third.of(material));
        }
        assert_eq!(
            second.of(AssetId::from_text("00000000-0000-4000-8000-000000000001")),
            None,
            "a skin, say"
        );

        let mut material = Material::from_document(
            &sapphire_assets::material::AnimeCharacterMaterial::default(),
            &[sapphire_assets::texture::Swizzle::IDENTITY;
                sapphire_assets::material::AnimeCharacterMaterial::TEXTURE_SLOTS.len()],
        )
        .expect("valid");
        let color = |material: &mut Material| {
            let mut color = [0.0; 4];
            assert!(material.edit_anime_character(|params| color = params.base_color));
            color
        };
        let [red, green, blue, alpha] = color(&mut material);
        recolor(&mut material, [0.5, 2.0, 4.0]);
        assert_eq!(color(&mut material), [red * 0.5, green * 2.0, blue * 4.0, alpha], "and keeps its alpha");
    }

    /// The party's own recording, with what draws following who's on the field and nothing in between.
    #[test]
    fn a_swap_changes_what_draws_and_only_a_swap_does() {
        let scenario = find_scenario(&SCENARIOS, crate::arena::PARTY_SCENARIO).expect("listed");
        let (mut sim, cast) = scenario.start(scenario.config).expect("sets up");
        let meshes = [0, 1, 2].map(MeshHandle::from_raw);
        let mut drawn = Drawn::new(meshes);

        assert_eq!(drawn.follow(sim.party()), None, "the first is on the field to start with");
        assert_eq!(drawn.mesh(), meshes[0]);
        let mut swap = TickActions::NEUTRAL;
        swap.set(cast[0], ActorActions { buttons: Buttons::SWAP, stick: Stick::CENTER }).expect("room");
        sim.step(&swap);
        assert_eq!(drawn.follow(sim.party()), Some(meshes[1]));
        assert_eq!(drawn.follow(sim.party()), None, "once");

        let drawn = drawn.on_new_renderer([3, 4, 5].map(MeshHandle::from_raw));
        assert_eq!(drawn.mesh(), MeshHandle::from_raw(4), "a new renderer draws the same member");
    }
}
