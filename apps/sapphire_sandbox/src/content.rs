//! The sandbox's content (M1 A4): the shared pack, read through the platform's IO thread, and
//! what the asset server loads from it, uploaded to the renderer.
//!
//! Assets are named by ID only (rule R5): the character is [`CHARACTER`], the mannequin's ID
//! from its sidecar (`content/samples/mannequin.glb.meta`), or the one the scene asks for
//! (`--look-dev`'s VRM, [`lookdev::CHARACTER`]). The asset server loads it with the
//! materials its slots name and their textures, dependencies first, and hands each to
//! [`GpuAssets`], which uploads it with the renderer's own calls and keeps the renderer's handles
//! by asset ID: a material's textures and a mesh's materials resolve through what is already
//! uploaded. The mannequin's materials are embedded today; a sidecar that binds a slot to a
//! material document draws with that material and its textures the same way.
//!
//! With `--swatches` it loads the shader swatches' three materials too, by the IDs in
//! [`swatches::MATERIALS`], each with the maps it names ([`Content::swatch_materials`]); the
//! spheres they draw on are procedural. With `--materials` it loads the material layer samples'
//! five materials (M1 C7), by the IDs in [`layers::MATERIALS`], with the matcap and layer mask
//! they name ([`Content::layer_materials`]). With `--faces` it loads the face test head
//! ([`faces::HEAD`]), whose face slot the head's sidecar binds to a face material, which names an
//! SDF face map, a face mask and an expression map: all of it cooked content
//! ([`Content::face_head`]); with `--hair-shadow-softness` or `--hair-transmission`, face and
//! hair materials upload with their bangs' shadow softened or their hair transmitting (M1 C8,
//! [`Content::override_materials`]), a copy of the cooked material edited. With `--hard-edges`
//! it loads the hard-edged outline sample
//! ([`outlines::HARD_EDGES`], [`Content::hard_edges`]).
//!
//! The zone's sky is the sky material [`SKY`] (`samples/sky.material.ron`), or the one the scene
//! asks for (`--look-dev`'s, [`lookdev::SKY`]), loaded with its cloud, cirrus, moon, star and
//! Milky Way maps ([`Content::sky_material`]).
//!
//! With `--environment` it loads the courtyard ([`environment::COURTYARD`]), whose slots its sidecar
//! binds to environment materials, which name a terminator band and a painted ground map
//! ([`Content::courtyard`]), and, where frames draw fog, its fog cards
//! ([`environment::FOG_CARDS`]), whose sidecars bind their slots to fog card materials, which
//! name the mist map ([`Content::fog_cards`]). With `--foliage` it loads the foliage samples, a
//! tree, a bush and a patch of grass ([`foliage::TREE`], [`foliage::BUSH`], [`foliage::GRASS`]),
//! whose slots their sidecars bind to foliage materials, which name the cards' maps
//! ([`Content::foliage`]). With `--probes` it loads the arcade ([`environment::ARCADE`]) and the
//! courtyard's ambient probe grid ([`environment::PROBES`], M1 W5), which the renderer holds one
//! of: a new version replaces it there, and the scene takes its key hours ([`Content::probes`],
//! [`Content::probe_hours`]).
//!
//! With `--look-dev` it loads the look-dev town ([`lookdev::TOWN`]), whose slots its sidecar
//! binds to the town's environment materials, the town's plants ([`lookdev::Plant`]), whose
//! sidecars bind theirs to the look-dev foliage materials, and its far mountains
//! ([`lookdev::MOUNTAINS`], whose sidecars bind them to the mountains' environment material;
//! [`Content::town`]), the courtyard's fog cards, and the character's rig ([`lookdev::RIG`],
//! [`Content::rig`]), which stays on the CPU.
//!
//! With `--stage` or `--look-dev` it keeps a copy of the character's mesh, from which the stage
//! derives the character without its pedestal and the look-dev stage its held pose
//! ([`Content::keep_character_mesh`]): the server keeps no mesh's bytes once uploaded.
//!
//! A new renderer (device loss, a quality-tier change) gets everything again through
//! [`Content::renderer_rebuilt`]: the server sends every asset back through its upload queue,
//! reading meshes and textures from the pack again.
//!
//! Hot reload (M1 A8; `dev` builds remount the pack when a cook rewrites it,
//! `Content::remount`) sends a new version of a loaded asset through the same queue, for the
//! same slot at a higher generation: [`GpuAssets`] replaces what the renderer holds in place, so
//! the handles the scene draws with stay valid. A texture is uploaded anew and the materials
//! that bind it come again to be rebound; a material rewrites its slot; a mesh replaces its
//! streams under its handle.
//!
//! In `dev` builds the server reads packs through live link's source (M1 A9,
//! `sapphire_livelink::LivePacks`), which serves packs pushed from the desktop in place of the
//! platform's; a pushed pack is remounted the same way (`Content::apply_pack`).

use std::collections::HashMap;
use std::io;
#[cfg(feature = "dev")]
use std::sync::Arc;

use sapphire_assets::material::Material;
use sapphire_assets::mesh::Mesh;
use sapphire_assets::pack;
use sapphire_assets::probes::ProbeGrid;
use sapphire_assets::rig::{ArchivedCharacterRig, CharacterRig};
use sapphire_assets::{
    AssetBudgets, AssetId, AssetServer, AssetSlot, AssetStats, Handle, LoadPriority, PackFile, PackSource,
    ReadDone, TextureAsset, Unloaded, Upload, UploadError,
};
#[cfg(feature = "dev")]
use sapphire_assets::{AssetChange, MountError, RemountReport};
#[cfg(feature = "dev")]
use sapphire_livelink::protocol::PackOutcome;
#[cfg(feature = "dev")]
use sapphire_livelink::{LivePacks, PushedPack};
use sapphire_platform::{AppInitError, IoFile, IoPriority, IoThread, Platform, ReadError};
use sapphire_render::texture::GpuTexture;
use sapphire_render::{MaterialHandle, MeshHandle, Renderer};
use sapphire_rhi::GpuContext;
use sapphire_time::Stopwatch;

use crate::LOG_TARGET;
use crate::environment;
use crate::faces::{self, MaterialOverrides};
use crate::foliage::{self, FoliageMeshes};
use crate::layers;
use crate::lookdev::{self, Plant, TownMeshes};
use crate::outlines;
use crate::scene;
use crate::swatches::{self, SWATCHES};

/// The pack the sandbox's content ships in.
const PACK: &str = "shared";
/// The sample character: the cooked mannequin (`samples/mannequin.glb`), by its sidecar's ID.
pub(crate) const CHARACTER: AssetId = AssetId::from_text("8659a15c-d588-4cb6-b757-deb86adf26a1");
/// The sandbox's sky: the sky material `samples/sky.material.ron`, by its sidecar's ID.
pub(crate) const SKY: AssetId = AssetId::from_text("22d0ccf1-d03c-4387-a00f-42891c86866f");

/// The platform's IO thread, serving the asset server's pack reads.
struct PlatformPacks(IoThread);

impl PackSource for PlatformPacks {
    fn open(&self, name: &str) -> io::Result<(PackFile, u64)> {
        let (file, len) = self.0.open(&pack::file_name(name)).map_err(io::Error::other)?;
        Ok((PackFile(u64::from(file.to_raw())), len))
    }

    fn close(&self, file: PackFile) {
        if let Ok(raw) = u32::try_from(file.0) {
            self.0.close(IoFile::from_raw(raw));
        }
    }

    fn read(&self, file: PackFile, offset: u64, len: usize, priority: LoadPriority, done: ReadDone) {
        let priority = match priority {
            LoadPriority::Critical => IoPriority::Critical,
            LoadPriority::High => IoPriority::High,
            LoadPriority::Normal => IoPriority::Normal,
            LoadPriority::Prefetch => IoPriority::Prefetch,
        };
        let Ok(raw) = u32::try_from(file.0) else {
            return done(Err(io::Error::other("not a file this source opened")));
        };
        self.0.read(
            IoFile::from_raw(raw),
            offset,
            len,
            priority,
            Box::new(move |result| {
                done(result.map_err(|error| match error {
                    ReadError::Io(error) => error,
                    other => io::Error::other(other),
                }))
            }),
        );
    }
}

/// What the renderer holds of an uploaded asset: the slot and generation it was uploaded for,
/// and the renderer's object.
struct Held<T> {
    slot: AssetSlot,
    generation: u32,
    object: T,
}

/// A mesh on the renderer, with its bounds.
#[derive(Clone, Copy)]
struct GpuMesh {
    handle: MeshHandle,
    bounds: [[f32; 3]; 2],
}

/// What the renderer holds of loaded assets, by asset ID.
struct GpuAssets {
    textures: HashMap<AssetId, Held<GpuTexture>>,
    materials: HashMap<AssetId, Held<MaterialHandle>>,
    /// Meshes, with their bounds.
    meshes: HashMap<AssetId, Held<GpuMesh>>,
    /// The character's ID, whose mesh [`Self::character_mesh`] copies.
    character: AssetId,
    /// A copy of the character's mesh as last uploaded, while one is wanted.
    character_mesh: Option<Mesh>,
    keep_character: bool,
    /// What the materials it uploads change (`--hair-shadow-softness`, `--hair-transmission`).
    overrides: MaterialOverrides,
    /// The probe grid the renderer holds (it holds one), as its key hours.
    probe_grid: Option<Held<Vec<f32>>>,
}

impl GpuAssets {
    /// Nothing uploaded yet, for `character`, whose mesh is copied while one is wanted.
    fn new(character: AssetId, keep_character: bool) -> Self {
        Self {
            textures: HashMap::new(),
            materials: HashMap::new(),
            meshes: HashMap::new(),
            character,
            character_mesh: None,
            keep_character,
            overrides: MaterialOverrides::default(),
            probe_grid: None,
        }
    }

    /// Uploads one asset the server hands over, resolving its references through what is
    /// already uploaded (the server uploads dependencies first). An asset this renderer already
    /// holds for the same slot at the same generation is kept as it is: the server re-sends
    /// everything after a renderer change, which [`Content::keep_renderer`] undoes. One it holds
    /// at an older generation is a new version (hot reload), replaced in place.
    fn upload(
        &mut self,
        upload: Upload<'_>,
        renderer: &mut Renderer,
        gpu: &GpuContext,
    ) -> Result<(), UploadError> {
        let (id, slot, generation) = (upload.id(), upload.slot(), upload.generation());
        if let Some(texture) = upload.get::<TextureAsset>() {
            if self.textures.get(&id).is_some_and(|held| held.slot == slot && held.generation >= generation) {
                return Ok(());
            }
            let object = GpuTexture::upload(gpu, &texture, "texture asset")?;
            self.textures.insert(id, Held { slot, generation, object });
        } else if let Some(material) = upload.get::<Material>() {
            // A face or hair changed by `--hair-shadow-softness` or `--hair-transmission` (M1
            // C8): a copy, edited and read back.
            let overridden = if self.overrides.is_empty() {
                None
            } else {
                let mut copy = material.to_material()?;
                self.overrides.apply(&mut copy).then(|| copy.to_cooked()).transpose()?
            };
            let material = match &overridden {
                Some(bytes) => Material::read(bytes)?,
                None => material,
            };
            let textures = &self.textures;
            let resolve = |texture| textures.get(&texture).map(|held| &held.object);
            match self.materials.get_mut(&id).filter(|held| held.slot == slot) {
                Some(held) if held.generation >= generation => {}
                Some(held) => {
                    renderer.update_material(held.object, material, resolve)?;
                    held.generation = generation;
                }
                None => {
                    let object = renderer.upload_material(material, resolve)?;
                    self.materials.insert(id, Held { slot, generation, object });
                }
            }
        } else if let Some(mesh) = upload.get::<Mesh>() {
            if id == self.character && self.keep_character {
                self.character_mesh = Some(mesh.to_mesh()?);
            }
            let materials = &self.materials;
            let resolve = |material| materials.get(&material).map(|held| held.object);
            let bounds = scene::mesh_bounds(mesh);
            match self.meshes.get_mut(&id).filter(|held| held.slot == slot) {
                Some(held) if held.generation >= generation => {}
                Some(held) => {
                    renderer.update_mesh(held.object.handle, mesh, resolve)?;
                    *held = Held { slot, generation, object: GpuMesh { handle: held.object.handle, bounds } };
                }
                None => {
                    // All or nothing, so a mesh the renderer rejects leaves no materials behind.
                    let handle = renderer.upload_mesh_with_slots(mesh, resolve)?;
                    self.meshes.insert(id, Held { slot, generation, object: GpuMesh { handle, bounds } });
                }
            }
        } else if let Some(grid) = upload.get::<ProbeGrid>() {
            // The renderer holds one grid: a new version (hot reload) replaces it there.
            if self.probe_grid.as_ref().is_some_and(|held| held.slot == slot && held.generation >= generation)
            {
                return Ok(());
            }
            renderer.set_probe_grid(Some(grid))?;
            self.probe_grid = Some(Held { slot, generation, object: grid.key_hours().collect() });
        }
        Ok(())
    }

    /// Forgets an unloaded asset. Textures are freed; the renderer keeps mesh and material slots
    /// for its lifetime until it gains residency of its own (`sapphire_render`'s later
    /// milestones).
    fn unload(&mut self, unloaded: Unloaded) {
        let Unloaded { id, slot, .. } = unloaded;
        if self.textures.get(&id).is_some_and(|held| held.slot == slot) {
            self.textures.remove(&id);
        }
        if self.materials.get(&id).is_some_and(|held| held.slot == slot) {
            self.materials.remove(&id);
        }
        if self.meshes.get(&id).is_some_and(|held| held.slot == slot) {
            self.meshes.remove(&id);
        }
        // The renderer keeps the grid, as it keeps meshes, until another replaces it.
        if self.probe_grid.as_ref().is_some_and(|held| held.slot == slot) {
            self.probe_grid = None;
        }
    }
}

/// What one renderer holds of loaded assets, set aside while a new renderer is tried
/// ([`Content::renderer_rebuilt`]).
pub(crate) struct RendererAssets(GpuAssets);

/// The asset server, what it has uploaded, and the character it keeps loaded.
pub(crate) struct Content {
    // Drop order: the uploaded GPU objects, then the server, whose IO thread joins last.
    gpu: GpuAssets,
    /// The character's ID: the mannequin's unless the scene asked for another.
    character_id: AssetId,
    character: Handle<Mesh>,
    /// The shader swatches' materials, once asked for (`--swatches`).
    swatches: Vec<Handle<Material>>,
    /// The material layer samples' materials, once asked for (`--materials`).
    layer_samples: Vec<Handle<Material>>,
    /// The face test head, once asked for (`--faces`).
    face_head: Option<Handle<Mesh>>,
    /// The courtyard, once asked for (`--environment`).
    courtyard: Option<Handle<Mesh>>,
    /// The courtyard's fog cards, once asked for (`--environment` with fog).
    fog_cards: Vec<Handle<Mesh>>,
    /// The foliage samples, once asked for (`--foliage`).
    foliage: Vec<Handle<Mesh>>,
    /// The hard-edged outline sample, once asked for (`--hard-edges`).
    hard_edges: Option<Handle<Mesh>>,
    /// The arcade and the courtyard's probe grid, once asked for (`--probes`).
    probes: Option<(Handle<Mesh>, Handle<ProbeGrid>)>,
    /// The look-dev town, its plants and its mountains, once asked for (`--look-dev`).
    town: Vec<Handle<Mesh>>,
    /// A character rig, once asked for (`--look-dev`'s).
    rig: Option<Handle<CharacterRig>>,
    /// The sky material the scene draws: [`SKY`] unless the scene wants another.
    sky_id: AssetId,
    /// The sky material, once asked for.
    sky: Option<Handle<Material>>,
    server: AssetServer,
    /// What the server reads through in `dev` builds: packs pushed over live link, over the
    /// platform's.
    #[cfg(feature = "dev")]
    live_packs: Arc<LivePacks>,
}

impl Content {
    /// Starts the IO thread on the app's content root, mounts the shared pack and starts loading
    /// the character (`character`, a mesh's ID: [`CHARACTER`] unless the scene wants another),
    /// within the budgets of the device's class. The scene's sky is the sky material `sky`
    /// ([`SKY`] unless the scene wants another), loaded when asked for.
    pub fn new(platform: &Platform, character: AssetId, sky: AssetId) -> Result<Self, AppInitError> {
        let stopwatch = Stopwatch::start();
        let content = platform.content();
        let missing = |error: &dyn std::fmt::Display| {
            AppInitError::msg(format!(
                "{error} (content root {:?}; cook it with `cargo xtask content`)",
                content.root()
            ))
        };
        let io = IoThread::start(content.clone()).map_err(|error| missing(&error))?;
        let budgets = AssetBudgets::for_form_factor(platform.system_info().device_class);
        let packs = PlatformPacks(io);
        #[cfg(feature = "dev")]
        let live_packs = Arc::new(LivePacks::new(packs));
        #[cfg(feature = "dev")]
        let mut server = AssetServer::new(live_packs.clone(), budgets);
        #[cfg(not(feature = "dev"))]
        let mut server = AssetServer::new(packs, budgets);
        server.mount(PACK).map_err(|error| missing(&error))?;
        let character_id = character;
        let character = server.load_with(character_id, LoadPriority::Critical).map_err(AppInitError::msg)?;
        tracing::info!(target: LOG_TARGET, pack = PACK, elapsed = ?stopwatch.elapsed(), ?budgets, "content mounted");
        Ok(Self {
            gpu: GpuAssets::new(character_id, false),
            character_id,
            character,
            swatches: Vec::new(),
            layer_samples: Vec::new(),
            face_head: None,
            courtyard: None,
            fog_cards: Vec::new(),
            foliage: Vec::new(),
            hard_edges: None,
            probes: None,
            town: Vec::new(),
            rig: None,
            sky_id: sky,
            sky: None,
            server,
            #[cfg(feature = "dev")]
            live_packs,
        })
    }

    /// One frame of loading: applies finished reads and decodes, frees what the server unloaded,
    /// and uploads what is ready within the frame's budget.
    pub fn update(&mut self, renderer: &mut Renderer, gpu: &GpuContext) {
        self.server.update();
        for unloaded in self.server.take_unloaded() {
            self.gpu.unload(unloaded);
        }
        let assets = &mut self.gpu;
        self.server.upload(|upload| assets.upload(upload, renderer, gpu));
    }

    /// Loads until the character is on `renderer`, then returns its mesh and bounds: the load
    /// screen the sandbox has at startup and after a renderer rebuild.
    pub fn character(
        &mut self,
        renderer: &mut Renderer,
        gpu: &GpuContext,
    ) -> Result<(MeshHandle, [[f32; 3]; 2]), AppInitError> {
        let stopwatch = Stopwatch::start();
        // Asking again tries a character that failed before (a read or upload that failed once).
        self.character =
            self.server.load_with(self.character_id, LoadPriority::Critical).map_err(AppInitError::msg)?;
        let assets = &mut self.gpu;
        let loaded = self.server.finish(&self.character, |upload| assets.upload(upload, renderer, gpu));
        // Unloads during the wait free only the copies they name: newer uploads have new slots.
        for unloaded in self.server.take_unloaded() {
            self.gpu.unload(unloaded);
        }
        loaded.map_err(|error| AppInitError::msg(format!("the character didn't load: {error}")))?;
        let GpuMesh { handle: mesh, bounds } =
            self.gpu.meshes.get(&self.character_id).expect("a ready mesh was uploaded here").object;
        let stats = self.server.stats();
        tracing::info!(
            target: LOG_TARGET,
            id = %self.character_id,
            elapsed = ?stopwatch.elapsed(),
            assets = stats.assets,
            resident = ?stats.resident,
            "character ready"
        );
        Ok((mesh, bounds))
    }

    /// Keeps a copy of the character's mesh from its next upload on, for meshes derived from it
    /// (the stage's figure and pedestal). Call before [`Self::character`].
    pub fn keep_character_mesh(&mut self) {
        self.gpu.keep_character = true;
    }

    /// Changes the anime character materials it uploads from now on by `overrides`
    /// (`--hair-shadow-softness`, `--hair-transmission`, M1 C8): the face test head's and the
    /// look-dev character's. Call before loading them.
    pub fn override_materials(&mut self, overrides: MaterialOverrides) {
        self.gpu.overrides = overrides;
    }

    /// The character's mesh as last uploaded, if [`Self::keep_character_mesh`] asked for it.
    pub fn character_mesh(&self) -> Option<&Mesh> {
        self.gpu.character_mesh.as_ref()
    }

    /// Loads the shader swatches' materials, with the textures they name, until they are on
    /// `renderer`, and returns their handles, top to bottom: the load screen of `--swatches`.
    /// They stay loaded from then on.
    pub fn swatch_materials(
        &mut self,
        renderer: &mut Renderer,
        gpu: &GpuContext,
    ) -> Result<[MaterialHandle; SWATCHES], AppInitError> {
        let mut kept = std::mem::take(&mut self.swatches);
        let materials = self.materials(swatches::MATERIALS, &mut kept, renderer, gpu, "swatch");
        self.swatches = kept;
        materials
    }

    /// Loads the material layer samples' materials (M1 C7), with the textures they name, until
    /// they are on `renderer`, and returns their handles in [`layers::MATERIALS`] order: the load
    /// screen of `--materials`. They stay loaded from then on.
    pub fn layer_materials(
        &mut self,
        renderer: &mut Renderer,
        gpu: &GpuContext,
    ) -> Result<[MaterialHandle; layers::SAMPLES], AppInitError> {
        let mut kept = std::mem::take(&mut self.layer_samples);
        let materials = self.materials(layers::MATERIALS, &mut kept, renderer, gpu, "material sample");
        self.layer_samples = kept;
        materials
    }

    /// Loads the materials `ids`, with the textures they name, until they are on `renderer`,
    /// keeping their handles in `kept`, and returns the renderer's handles in order. `what` names
    /// them in the error.
    fn materials<const N: usize>(
        &mut self,
        ids: [AssetId; N],
        kept: &mut Vec<Handle<Material>>,
        renderer: &mut Renderer,
        gpu: &GpuContext,
        what: &str,
    ) -> Result<[MaterialHandle; N], AppInitError> {
        // Asking again tries materials that failed before, as the character does.
        *kept = ids
            .iter()
            .map(|&id| self.server.load_with(id, LoadPriority::Critical))
            .collect::<Result<_, _>>()
            .map_err(AppInitError::msg)?;
        let mut loaded = Ok(());
        for handle in kept.iter() {
            let assets = &mut self.gpu;
            loaded = loaded.and(self.server.finish(handle, |upload| assets.upload(upload, renderer, gpu)));
        }
        for unloaded in self.server.take_unloaded() {
            self.gpu.unload(unloaded);
        }
        loaded.map_err(|error| AppInitError::msg(format!("the {what} materials didn't load: {error}")))?;
        Ok(ids.map(|id| self.gpu.materials.get(&id).expect("a ready material was uploaded here").object))
    }

    /// Loads until the face test head is on `renderer`, with its materials and the maps they
    /// name, and returns its mesh: the load screen of `--faces`. It stays loaded from then on.
    pub fn face_head(
        &mut self,
        renderer: &mut Renderer,
        gpu: &GpuContext,
    ) -> Result<MeshHandle, AppInitError> {
        // Asking again tries a head that failed before, as the character does.
        let head = self.server.load_with(faces::HEAD, LoadPriority::Critical).map_err(AppInitError::msg)?;
        let assets = &mut self.gpu;
        let loaded = self.server.finish(&head, |upload| assets.upload(upload, renderer, gpu));
        self.face_head = Some(head);
        for unloaded in self.server.take_unloaded() {
            self.gpu.unload(unloaded);
        }
        loaded.map_err(|error| AppInitError::msg(format!("the face head didn't load: {error}")))?;
        Ok(self.gpu.meshes.get(&faces::HEAD).expect("a ready mesh was uploaded here").object.handle)
    }

    /// Loads until the courtyard is on `renderer`, with its environment materials and the maps they
    /// name, and returns its mesh: the load screen of `--environment`. It stays loaded from then
    /// on.
    pub fn courtyard(
        &mut self,
        renderer: &mut Renderer,
        gpu: &GpuContext,
    ) -> Result<MeshHandle, AppInitError> {
        // Asking again tries a courtyard that failed before, as the character does.
        let courtyard = self
            .server
            .load_with(environment::COURTYARD, LoadPriority::Critical)
            .map_err(AppInitError::msg)?;
        let assets = &mut self.gpu;
        let loaded = self.server.finish(&courtyard, |upload| assets.upload(upload, renderer, gpu));
        self.courtyard = Some(courtyard);
        for unloaded in self.server.take_unloaded() {
            self.gpu.unload(unloaded);
        }
        loaded.map_err(|error| AppInitError::msg(format!("the courtyard didn't load: {error}")))?;
        Ok(self
            .gpu
            .meshes
            .get(&environment::COURTYARD)
            .expect("a ready mesh was uploaded here")
            .object
            .handle)
    }

    /// Loads until the courtyard's fog cards are on `renderer`, with their fog card materials and
    /// the map they read, and returns their meshes in [`environment::FOG_CARDS`] order. They stay
    /// loaded from then on.
    pub fn fog_cards(
        &mut self,
        renderer: &mut Renderer,
        gpu: &GpuContext,
    ) -> Result<[MeshHandle; environment::FOG_CARDS.len()], AppInitError> {
        let mut cards = Vec::with_capacity(environment::FOG_CARDS.len());
        for id in environment::FOG_CARDS {
            // Asking again tries a card that failed before, as the character does.
            let card = self.server.load_with(id, LoadPriority::Critical).map_err(AppInitError::msg)?;
            let assets = &mut self.gpu;
            let loaded = self.server.finish(&card, |upload| assets.upload(upload, renderer, gpu));
            cards.push(card);
            for unloaded in self.server.take_unloaded() {
                self.gpu.unload(unloaded);
            }
            loaded.map_err(|error| AppInitError::msg(format!("a fog card didn't load: {error}")))?;
        }
        self.fog_cards = cards;
        Ok(environment::FOG_CARDS
            .map(|id| self.gpu.meshes.get(&id).expect("a ready mesh was uploaded here").object.handle))
    }

    /// Loads until the foliage samples are on `renderer`, with their foliage materials and the
    /// cards' maps, and returns their meshes: the load screen of `--foliage`. They stay loaded
    /// from then on.
    pub fn foliage(
        &mut self,
        renderer: &mut Renderer,
        gpu: &GpuContext,
    ) -> Result<FoliageMeshes, AppInitError> {
        // Asking again tries plants that failed before, as the character does.
        let plants = [foliage::TREE, foliage::BUSH, foliage::GRASS];
        let handles = plants
            .into_iter()
            .map(|id| self.server.load_with(id, LoadPriority::Critical).map_err(AppInitError::msg))
            .collect::<Result<Vec<_>, _>>()?;
        let assets = &mut self.gpu;
        let loaded: Result<(), _> = handles
            .iter()
            .try_for_each(|handle| self.server.finish(handle, |upload| assets.upload(upload, renderer, gpu)));
        self.foliage = handles;
        for unloaded in self.server.take_unloaded() {
            self.gpu.unload(unloaded);
        }
        loaded.map_err(|error| AppInitError::msg(format!("the foliage didn't load: {error}")))?;
        let mesh = |id| self.gpu.meshes.get(&id).expect("a ready mesh was uploaded here").object.handle;
        Ok(FoliageMeshes {
            tree: mesh(foliage::TREE),
            bush: mesh(foliage::BUSH),
            grass: mesh(foliage::GRASS),
        })
    }

    /// Loads until the arcade and the courtyard's probe grid are on `renderer`, the arcade with
    /// its materials, and returns the arcade's mesh: the load screen of `--probes`. Both stay
    /// loaded from then on; the grid's key hours are [`Self::probe_hours`].
    pub fn probes(&mut self, renderer: &mut Renderer, gpu: &GpuContext) -> Result<MeshHandle, AppInitError> {
        // Asking again tries what failed before, as the character does.
        let arcade =
            self.server.load_with(environment::ARCADE, LoadPriority::Critical).map_err(AppInitError::msg)?;
        let grid =
            self.server.load_with(environment::PROBES, LoadPriority::Critical).map_err(AppInitError::msg)?;
        let assets = &mut self.gpu;
        let loaded = self
            .server
            .finish(&arcade, |upload| assets.upload(upload, renderer, gpu))
            .and_then(|()| self.server.finish(&grid, |upload| assets.upload(upload, renderer, gpu)));
        self.probes = Some((arcade, grid));
        for unloaded in self.server.take_unloaded() {
            self.gpu.unload(unloaded);
        }
        loaded
            .map_err(|error| AppInitError::msg(format!("the arcade and probe grid didn't load: {error}")))?;
        Ok(self.gpu.meshes.get(&environment::ARCADE).expect("a ready mesh was uploaded here").object.handle)
    }

    /// The hours of the time-of-day keys the uploaded probe grid was baked at, for time of day
    /// to pick the keys a frame blends (`TimeOfDay::probe_keys`); empty without a grid.
    pub fn probe_hours(&self) -> &[f32] {
        self.gpu.probe_grid.as_ref().map_or(&[], |held| &held.object)
    }

    /// Loads until the hard-edged outline sample is on `renderer`, with the materials it embeds,
    /// and returns its mesh: the load screen of `--hard-edges`. It stays loaded from then on.
    pub fn hard_edges(
        &mut self,
        renderer: &mut Renderer,
        gpu: &GpuContext,
    ) -> Result<MeshHandle, AppInitError> {
        // Asking again tries a sample that failed before, as the character does.
        let mesh =
            self.server.load_with(outlines::HARD_EDGES, LoadPriority::Critical).map_err(AppInitError::msg)?;
        let assets = &mut self.gpu;
        let loaded = self.server.finish(&mesh, |upload| assets.upload(upload, renderer, gpu));
        self.hard_edges = Some(mesh);
        for unloaded in self.server.take_unloaded() {
            self.gpu.unload(unloaded);
        }
        loaded.map_err(|error| AppInitError::msg(format!("the hard-edged sample didn't load: {error}")))?;
        Ok(self.gpu.meshes.get(&outlines::HARD_EDGES).expect("a ready mesh was uploaded here").object.handle)
    }

    /// Loads until the look-dev town, its plants and its mountains are on `renderer`, with their
    /// materials and the maps they name, and returns their meshes: the load screen of
    /// `--look-dev`. They stay loaded from then on.
    pub fn town(&mut self, renderer: &mut Renderer, gpu: &GpuContext) -> Result<TownMeshes, AppInitError> {
        // Asking again tries a town that failed before, as the character does.
        let ids = std::iter::once(lookdev::TOWN).chain(Plant::ALL.map(Plant::id)).chain(lookdev::MOUNTAINS);
        let handles = ids
            .map(|id| self.server.load_with(id, LoadPriority::Critical).map_err(AppInitError::msg))
            .collect::<Result<Vec<_>, _>>()?;
        let assets = &mut self.gpu;
        let loaded: Result<(), _> = handles
            .iter()
            .try_for_each(|handle| self.server.finish(handle, |upload| assets.upload(upload, renderer, gpu)));
        self.town = handles;
        for unloaded in self.server.take_unloaded() {
            self.gpu.unload(unloaded);
        }
        loaded.map_err(|error| AppInitError::msg(format!("the look-dev town didn't load: {error}")))?;
        let mesh = |id| self.gpu.meshes.get(&id).expect("a ready mesh was uploaded here").object.handle;
        Ok(TownMeshes {
            town: mesh(lookdev::TOWN),
            plants: Plant::ALL.map(|plant| mesh(plant.id())),
            mountains: lookdev::MOUNTAINS.map(mesh),
        })
    }

    /// Loads the character rig `id` until it is resident: CPU data, which nothing uploads
    /// ([`Self::rig`] reads it). It stays loaded from then on.
    pub fn load_rig(
        &mut self,
        id: AssetId,
        renderer: &mut Renderer,
        gpu: &GpuContext,
    ) -> Result<(), AppInitError> {
        // Asking again tries a rig that failed before, as the character does.
        let rig = self.server.load_with(id, LoadPriority::Critical).map_err(AppInitError::msg)?;
        let assets = &mut self.gpu;
        let loaded = self.server.finish(&rig, |upload| assets.upload(upload, renderer, gpu));
        self.rig = Some(rig);
        for unloaded in self.server.take_unloaded() {
            self.gpu.unload(unloaded);
        }
        loaded.map_err(|error| AppInitError::msg(format!("the character rig didn't load: {error}")))
    }

    /// The rig [`Self::load_rig`] loaded.
    pub fn rig(&self) -> Option<&ArchivedCharacterRig> {
        self.rig.as_ref().and_then(|rig| self.server.get(rig))
    }

    /// Loads the sky material, with the maps it names, until it is on `renderer`, and returns its
    /// handle: the sky's part of the load screen. It stays loaded from then on.
    pub fn sky_material(
        &mut self,
        renderer: &mut Renderer,
        gpu: &GpuContext,
    ) -> Result<MaterialHandle, AppInitError> {
        // Asking again tries a material that failed before, as the character does.
        let handle = self.server.load_with(self.sky_id, LoadPriority::Critical).map_err(AppInitError::msg)?;
        let assets = &mut self.gpu;
        let loaded = self.server.finish(&handle, |upload| assets.upload(upload, renderer, gpu));
        self.sky = Some(handle);
        for unloaded in self.server.take_unloaded() {
            self.gpu.unload(unloaded);
        }
        loaded.map_err(|error| AppInitError::msg(format!("the sky material didn't load: {error}")))?;
        Ok(self.gpu.materials.get(&self.sky_id).expect("a ready material was uploaded here").object)
    }

    /// For a new renderer, which starts empty: sets the old one's objects aside and sends every
    /// asset through the upload queue again. Returns the old objects, for [`Self::keep_renderer`]
    /// if the new renderer is given up.
    pub fn renderer_rebuilt(&mut self) -> RendererAssets {
        self.server.reupload_all();
        let fresh = GpuAssets::new(self.gpu.character, self.gpu.keep_character);
        let previous = std::mem::replace(&mut self.gpu, fresh);
        RendererAssets(previous)
    }

    /// Goes back to the renderer whose objects `previous` holds, after a new one failed. Every
    /// asset is sent again, and the old renderer takes only what it lacks, so nothing it holds is
    /// uploaded twice and the handles the scene draws with stay valid.
    pub fn keep_renderer(&mut self, previous: RendererAssets) {
        self.gpu = previous.0;
        self.server.reupload_all();
    }

    /// Reads pack `name` again after its file changed and reloads what changed (hot reload,
    /// `dev` builds): new versions reach the renderer through [`Self::update`].
    #[cfg(feature = "dev")]
    pub fn remount(&mut self, name: &str) -> Result<RemountReport, MountError> {
        self.server.remount(name)
    }

    /// Whether pack `name` is mounted.
    #[cfg(feature = "dev")]
    pub fn is_mounted(&self, name: &str) -> bool {
        self.server.is_mounted(name)
    }

    /// Hot reload's changes to loaded assets since the last call.
    #[cfg(feature = "dev")]
    pub fn take_changes(&mut self) -> Vec<AssetChange> {
        self.server.take_changes()
    }

    /// The pack source the server reads through, which live link's connections plan pushes
    /// against (`dev` builds).
    #[cfg(feature = "dev")]
    pub fn live_packs(&self) -> Arc<LivePacks> {
        self.live_packs.clone()
    }

    /// Serves a pack pushed over live link from now on, and remounts it if it is mounted: what
    /// changed reaches the renderer through [`Self::update`], as a hot reload does. A pack the
    /// server refuses is served no more.
    #[cfg(feature = "dev")]
    pub fn apply_pack(&mut self, pack: PushedPack) -> Result<PackOutcome, MountError> {
        pack.apply(&self.live_packs, &mut self.server)
    }

    /// The asset server's counters (rule R9).
    #[cfg_attr(not(feature = "dev"), expect(dead_code, reason = "the debug overlay shows them"))]
    pub fn stats(&self) -> AssetStats {
        self.server.stats()
    }

    /// Unloads every asset nothing holds or pins and frees the textures uploaded for them at once,
    /// rather than at the next [`Self::update`]: the response to a system memory warning (review
    /// PA-004, spec §6.3). What the scene draws stays, since the sandbox holds a handle to all of
    /// it; what goes is what reloads and rebuilds left behind. The renderer keeps mesh and
    /// material slots for its lifetime (see `GpuAssets::unload`). Returns the server's counters
    /// before and after.
    pub fn trim(&mut self) -> (AssetStats, AssetStats) {
        let before = self.server.stats();
        self.server.unload_unused();
        for unloaded in self.server.take_unloaded() {
            self.gpu.unload(unloaded);
        }
        (before, self.server.stats())
    }
}
