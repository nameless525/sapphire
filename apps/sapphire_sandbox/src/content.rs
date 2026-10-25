//! The sandbox's content (M1 A4): the shared pack, read through the platform's IO thread, and
//! what the asset server loads from it, uploaded to the renderer.
//!
//! Assets are named by ID only (rule R5): the character is [`CHARACTER`], the mannequin's ID
//! from its sidecar (`content/samples/mannequin.glb.meta`). The asset server loads it with the
//! materials its slots name and their textures, dependencies first, and hands each to
//! [`GpuAssets`], which uploads it with the renderer's own calls and keeps the renderer's handles
//! by asset ID: a material's textures and a mesh's materials resolve through what is already
//! uploaded. The mannequin's materials are embedded today; a sidecar that binds a slot to a
//! material document draws with that material and its textures the same way.
//!
//! With `--swatches` it loads the shader swatches' three materials too, by the IDs in
//! [`swatches::MATERIALS`], each with the maps it names ([`Content::swatch_materials`]); the
//! spheres they draw on are procedural.
//!
//! A new renderer (device loss, a quality-tier change) gets everything again through
//! [`Content::renderer_rebuilt`]: the server sends every asset back through its upload queue,
//! reading meshes and textures from the pack again.

use std::collections::HashMap;
use std::io;

use sapphire_assets::material::Material;
use sapphire_assets::mesh::Mesh;
use sapphire_assets::pack;
use sapphire_assets::{
    AssetBudgets, AssetId, AssetServer, AssetSlot, AssetStats, Handle, LoadPriority, PackFile, PackSource,
    ReadDone, TextureAsset, Unloaded, Upload, UploadError,
};
use sapphire_platform::{AppInitError, IoFile, IoPriority, IoThread, Platform, ReadError};
use sapphire_render::texture::GpuTexture;
use sapphire_render::{MaterialHandle, MeshHandle, Renderer};
use sapphire_rhi::GpuContext;
use sapphire_time::Stopwatch;

use crate::LOG_TARGET;
use crate::scene;
use crate::swatches::{self, SWATCHES};

/// The pack the sandbox's content ships in.
const PACK: &str = "shared";
/// The sample character: the cooked mannequin (`samples/mannequin.glb`), by its sidecar's ID.
pub(crate) const CHARACTER: AssetId = AssetId::from_text("8659a15c-d588-4cb6-b757-deb86adf26a1");

/// The platform's IO thread, serving the asset server's pack reads.
struct PlatformPacks(IoThread);

impl PackSource for PlatformPacks {
    fn open(&self, name: &str) -> io::Result<(PackFile, u64)> {
        let (file, len) = self.0.open(&pack::file_name(name)).map_err(io::Error::other)?;
        Ok((PackFile(u64::from(file.to_raw())), len))
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

/// What the renderer holds of loaded assets, by asset ID, with the slot each was uploaded for.
#[derive(Default)]
struct GpuAssets {
    textures: HashMap<AssetId, (AssetSlot, GpuTexture)>,
    materials: HashMap<AssetId, (AssetSlot, MaterialHandle)>,
    meshes: HashMap<AssetId, (AssetSlot, MeshHandle, [[f32; 3]; 2])>,
}

impl GpuAssets {
    /// Uploads one asset the server hands over, resolving its references through what is
    /// already uploaded (the server uploads dependencies first). An asset this renderer already
    /// holds for the same slot is kept as it is: the server re-sends everything after a renderer
    /// change, which [`Content::keep_renderer`] undoes.
    fn upload(
        &mut self,
        upload: Upload<'_>,
        renderer: &mut Renderer,
        gpu: &GpuContext,
    ) -> Result<(), UploadError> {
        let (id, slot) = (upload.id(), upload.slot());
        if self.holds(id, slot) {
            return Ok(());
        }
        if let Some(texture) = upload.get::<TextureAsset>() {
            let texture = GpuTexture::upload(gpu, &texture, "texture asset")?;
            self.textures.insert(id, (slot, texture));
        } else if let Some(material) = upload.get::<Material>() {
            let handle =
                renderer.upload_material(material, |texture| self.textures.get(&texture).map(|(_, t)| t))?;
            self.materials.insert(id, (slot, handle));
        } else if let Some(mesh) = upload.get::<Mesh>() {
            let materials = renderer.upload_slot_materials(mesh, |material| {
                self.materials.get(&material).map(|&(_, handle)| handle)
            })?;
            let handle = renderer.upload_mesh(mesh, &materials)?;
            self.meshes.insert(id, (slot, handle, scene::mesh_bounds(mesh)));
        }
        Ok(())
    }

    /// Forgets an unloaded asset. Textures are freed; the renderer keeps mesh and material slots
    /// for its lifetime until it gains residency of its own (`sapphire_render`'s later
    /// milestones).
    fn unload(&mut self, unloaded: Unloaded) {
        let Unloaded { id, slot, .. } = unloaded;
        if self.textures.get(&id).is_some_and(|(uploaded, _)| *uploaded == slot) {
            self.textures.remove(&id);
        }
        if self.materials.get(&id).is_some_and(|(uploaded, _)| *uploaded == slot) {
            self.materials.remove(&id);
        }
        if self.meshes.get(&id).is_some_and(|(uploaded, ..)| *uploaded == slot) {
            self.meshes.remove(&id);
        }
    }

    /// Whether this renderer holds asset `id` as uploaded for `slot`.
    fn holds(&self, id: AssetId, slot: AssetSlot) -> bool {
        self.textures.get(&id).is_some_and(|(uploaded, _)| *uploaded == slot)
            || self.materials.get(&id).is_some_and(|(uploaded, _)| *uploaded == slot)
            || self.meshes.get(&id).is_some_and(|(uploaded, ..)| *uploaded == slot)
    }
}

/// What one renderer holds of loaded assets, set aside while a new renderer is tried
/// ([`Content::renderer_rebuilt`]).
pub(crate) struct RendererAssets(GpuAssets);

/// The asset server, what it has uploaded, and the character it keeps loaded.
pub(crate) struct Content {
    // Drop order: the uploaded GPU objects, then the server, whose IO thread joins last.
    gpu: GpuAssets,
    character: Handle<Mesh>,
    /// The shader swatches' materials, once asked for (`--swatches`).
    swatches: Vec<Handle<Material>>,
    server: AssetServer,
}

impl Content {
    /// Starts the IO thread on the app's content root, mounts the shared pack and starts loading
    /// the character, within the budgets of the device's class.
    pub fn new(platform: &Platform) -> Result<Self, AppInitError> {
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
        let mut server = AssetServer::new(PlatformPacks(io), budgets);
        server.mount(PACK).map_err(|error| missing(&error))?;
        let character = server.load_with(CHARACTER, LoadPriority::Critical).map_err(AppInitError::msg)?;
        tracing::info!(target: LOG_TARGET, pack = PACK, elapsed = ?stopwatch.elapsed(), ?budgets, "content mounted");
        Ok(Self { gpu: GpuAssets::default(), character, swatches: Vec::new(), server })
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
            self.server.load_with(CHARACTER, LoadPriority::Critical).map_err(AppInitError::msg)?;
        let assets = &mut self.gpu;
        let loaded = self.server.finish(&self.character, |upload| assets.upload(upload, renderer, gpu));
        // Unloads during the wait free only the copies they name: newer uploads have new slots.
        for unloaded in self.server.take_unloaded() {
            self.gpu.unload(unloaded);
        }
        loaded.map_err(|error| AppInitError::msg(format!("the character didn't load: {error}")))?;
        let &(_, mesh, bounds) = self.gpu.meshes.get(&CHARACTER).expect("a ready mesh was uploaded here");
        let stats = self.server.stats();
        tracing::info!(
            target: LOG_TARGET,
            id = %CHARACTER,
            elapsed = ?stopwatch.elapsed(),
            assets = stats.assets,
            resident = ?stats.resident,
            "character ready"
        );
        Ok((mesh, bounds))
    }

    /// Loads the shader swatches' materials, with the textures they name, until they are on
    /// `renderer`, and returns their handles, top to bottom: the load screen of `--swatches`.
    /// They stay loaded from then on.
    pub fn swatch_materials(
        &mut self,
        renderer: &mut Renderer,
        gpu: &GpuContext,
    ) -> Result<[MaterialHandle; SWATCHES], AppInitError> {
        // Asking again tries materials that failed before, as the character does.
        self.swatches = swatches::MATERIALS
            .iter()
            .map(|&id| self.server.load_with(id, LoadPriority::Critical))
            .collect::<Result<_, _>>()
            .map_err(AppInitError::msg)?;
        let mut loaded = Ok(());
        for handle in &self.swatches {
            let assets = &mut self.gpu;
            loaded = loaded.and(self.server.finish(handle, |upload| assets.upload(upload, renderer, gpu)));
        }
        for unloaded in self.server.take_unloaded() {
            self.gpu.unload(unloaded);
        }
        loaded.map_err(|error| AppInitError::msg(format!("the swatch materials didn't load: {error}")))?;
        Ok(swatches::MATERIALS.map(|id| {
            let &(_, handle) = self.gpu.materials.get(&id).expect("a ready material was uploaded here");
            handle
        }))
    }

    /// For a new renderer, which starts empty: sets the old one's objects aside and sends every
    /// asset through the upload queue again. Returns the old objects, for [`Self::keep_renderer`]
    /// if the new renderer is given up.
    pub fn renderer_rebuilt(&mut self) -> RendererAssets {
        self.server.reupload_all();
        RendererAssets(std::mem::take(&mut self.gpu))
    }

    /// Goes back to the renderer whose objects `previous` holds, after a new one failed. Every
    /// asset is sent again, and the old renderer takes only what it lacks, so nothing it holds is
    /// uploaded twice and the handles the scene draws with stay valid.
    pub fn keep_renderer(&mut self, previous: RendererAssets) {
        self.gpu = previous.0;
        self.server.reupload_all();
    }

    /// The asset server's counters (rule R9).
    #[cfg_attr(not(feature = "dev"), expect(dead_code, reason = "the debug overlay shows them"))]
    pub fn stats(&self) -> AssetStats {
        self.server.stats()
    }
}
