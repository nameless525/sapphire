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
//! What a game loads beside these, it holds itself ([`Content::hold`]): `game_combat`'s enemy and blade are
//! that, the enemy's mesh copied as it uploads since its held pose derives from it ([`Content::keep_mesh_copy`]).
//!
//! With `--look-dev` it loads the town's plants ([`lookdev::Plant`]), whose
//! sidecars bind theirs to the look-dev foliage materials, and its far mountains
//! ([`lookdev::MOUNTAINS`], whose sidecars bind them to the mountains' environment material;
//! [`Content::town`]), the courtyard's fog cards, and the character's rig ([`lookdev::RIG`],
//! [`Content::rig`]) and skeleton ([`Content::skeleton`]), which stay on the CPU. The town itself is a
//! zone ([`lookdev::TOWN_ZONE`]), which the scene streams ([`crate::scene::ZoneStage`]): this holds
//! none of its cells' handles, only the means to load them ([`Content::open_zone`]).
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
//! streams under its handle, which starts its instances over in their bind pose, so the scene
//! sends their poses again in the frame that happens ([`Content::take_meshes_replaced`],
//! `Scene::follow_content`). A new version of the look-dev character's mesh, rig or skeleton holds
//! the character anew ([`Content::take_character_changed`]). What the stage and the rim rig made from
//! the character's mesh are replaced with it, so they're as new as it is
//! ([`crate::studio::CharacterCopies`]).
//!
//! In `dev` builds the server reads packs through live link's source (M1 A9,
//! `sapphire_livelink::LivePacks`), which serves packs pushed from the desktop in place of the
//! platform's; a pushed pack is remounted the same way (`Content::apply_pack`).

use std::collections::{HashMap, HashSet};
use std::io;
#[cfg(feature = "dev")]
use std::path::PathBuf;
#[cfg(feature = "dev")]
use std::sync::Arc;

use sapphire_anim::{AnimGraph, ArchivedClip, ArchivedSkeleton, Clip, Skeleton};
#[cfg(feature = "dev")]
use sapphire_anim::{ClipSource, GraphBindError, GraphBinding};
use sapphire_assets::cue::Cue;
use sapphire_assets::material::{AlignedVec, ArchivedMaterial, Material};
use sapphire_assets::mesh::{ArchivedMesh, Mesh};
use sapphire_assets::pack;
use sapphire_assets::probes::ProbeGrid;
use sapphire_assets::rig::{ArchivedCharacterRig, CharacterRig};
use sapphire_assets::sound::{AUDIO_PACK, DecodedSound, Sound};
use sapphire_timeline::Timeline;
use sapphire_assets::{
    Asset, AssetBudgets, AssetId, AssetServer, AssetSlot, AssetStats, Handle, LoadError, LoadPriority,
    PackFile, PackSource, ReadDone, TextureAsset, Unloaded, Upload, UploadError,
};
#[cfg(feature = "dev")]
use sapphire_assets::{AssetChange, MountError, RemountReport};
#[cfg(feature = "dev")]
use sapphire_livelink::protocol::PackOutcome;
#[cfg(feature = "dev")]
use sapphire_livelink::{LivePacks, PushedPack};
use sapphire_platform::{AppInitError, IoFile, IoPriority, IoThread, Platform, ReadError};
use sapphire_render::texture::GpuTexture;
use sapphire_render::{MaterialHandle, MeshHandle, PipelineInventory, Renderer};
use sapphire_rhi::GpuContext;
use sapphire_time::Stopwatch;

use crate::LOG_TARGET;
use crate::anim_graph::ClipTable;
use crate::environment;
use crate::faces::{self, MaterialOverrides};
use crate::foliage::{self, FoliageMeshes};
use crate::layers;
use crate::lookdev::{self, Plant, TownMeshes};
use crate::outlines;
use crate::rims::{self, RimRigMeshes};
use crate::scene;
use crate::studio::{self, CharacterCopies, StudioMeshes};
use crate::swatches::{self, SWATCHES};
use sapphire_world::streaming::{StreamSettings, Streamer};

/// The pack the sandbox's content ships in.
const PACK: &str = "shared";
/// The sample character: the cooked mannequin (`samples/mannequin.glb`), by its sidecar's ID.
pub const CHARACTER: AssetId = AssetId::from_text("8659a15c-d588-4cb6-b757-deb86adf26a1");
/// The sandbox's sky: the sky material `samples/sky.material.ron`, by its sidecar's ID.
pub const SKY: AssetId = AssetId::from_text("22d0ccf1-d03c-4387-a00f-42891c86866f");

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
    /// The bytes of [`Self::textures`], against the zone texture pool's budget (spec §3.4).
    texture_bytes: u64,
    materials: HashMap<AssetId, Held<MaterialHandle>>,
    /// Meshes, with their bounds.
    meshes: HashMap<AssetId, Held<GpuMesh>>,
    /// The character's ID, whose mesh [`Self::character_mesh`] copies.
    character: AssetId,
    /// A copy of the character's mesh as last uploaded, while one is wanted.
    character_mesh: Option<Mesh>,
    keep_character: bool,
    /// A copy of each mesh a caller asked to keep ([`Content::keep_mesh_copy`]) as last uploaded, which what
    /// it derives is made from: the server keeps no bytes of what it has uploaded.
    kept_meshes: Vec<(AssetId, Option<Mesh>)>,
    /// A mesh was replaced in place since [`Content::take_meshes_replaced`] last asked.
    meshes_replaced: bool,
    /// What the materials it uploads change (`--hair-shadow-softness`, `--hair-transmission`).
    overrides: MaterialOverrides,
    /// What makes copies of its own of what it uploads ([`UploadWatcher`]).
    watcher: Option<Box<dyn UploadWatcher>>,
    /// What the stage and the rim rig made of the character on this renderer.
    copies: CharacterCopies,
    /// A new version of the character has replaced the stage's copies, since the scene last asked.
    studio_replaced: bool,
    /// The probe grid the renderer holds (it holds one), as its key hours.
    probe_grid: Option<Held<Vec<f32>>>,
    /// Meshes and materials of assets unloaded, for the next [`Self::free_stale`].
    stale_meshes: Vec<MeshHandle>,
    stale_materials: Vec<MaterialHandle>,
}

impl GpuAssets {
    /// Nothing uploaded yet, for `character`, whose mesh is copied while one is wanted.
    fn new(character: AssetId, keep_character: bool) -> Self {
        Self {
            textures: HashMap::new(),
            texture_bytes: 0,
            materials: HashMap::new(),
            meshes: HashMap::new(),
            character,
            character_mesh: None,
            keep_character,
            kept_meshes: Vec::new(),
            meshes_replaced: false,
            overrides: MaterialOverrides::default(),
            watcher: None,
            copies: CharacterCopies::default(),
            studio_replaced: false,
            probe_grid: None,
            stale_meshes: Vec::new(),
            stale_materials: Vec::new(),
        }
    }

    /// Lets go of what's held of any asset `resident` no longer says is: this renderer was set aside
    /// while a new one was tried, and only the new one heard of what the server unloaded meanwhile.
    fn take_stale(&mut self, resident: impl Fn(AssetId) -> bool) {
        let mut texture_bytes = self.texture_bytes;
        self.textures.retain(|&id, held| {
            let kept = resident(id);
            if !kept {
                texture_bytes -= held.object.bytes();
            }
            kept
        });
        self.texture_bytes = texture_bytes;
        let stale_materials = &mut self.stale_materials;
        self.materials.retain(|&id, held| {
            let kept = resident(id);
            if !kept {
                stale_materials.push(held.object);
            }
            kept
        });
        let stale_meshes = &mut self.stale_meshes;
        self.meshes.retain(|&id, held| {
            let kept = resident(id);
            if !kept {
                stale_meshes.push(held.object.handle);
            }
            kept
        });
    }

    /// Frees what's been let go of, on the renderer it's held on.
    fn free_stale(&mut self, renderer: &mut Renderer) {
        for mesh in self.stale_meshes.drain(..) {
            renderer.free_mesh(mesh);
        }
        for material in self.stale_materials.drain(..) {
            renderer.free_material(material);
        }
    }

    /// Nothing uploaded, for a new renderer, which is told what this one was: the server sends it
    /// every asset again, and a material changed by the command line has to be changed again.
    fn for_new_renderer(&self) -> Self {
        Self {
            overrides: self.overrides,
            watcher: self.watcher.as_ref().map(|watcher| watcher.for_new_renderer()),
            kept_meshes: self.kept_meshes.iter().map(|(id, _)| (*id, None)).collect(),
            ..Self::new(self.character, self.keep_character)
        }
    }

    /// A face or hair changed by `--hair-shadow-softness` or `--hair-transmission` (M1 C8), as a
    /// copy, edited and read back. `None` for a material they leave as it is.
    fn overridden(&self, material: &ArchivedMaterial) -> Result<Option<AlignedVec>, UploadError> {
        if self.overrides.is_empty() {
            return Ok(None);
        }
        let mut copy = material.to_material()?;
        Ok(self.overrides.apply(&mut copy).then(|| copy.to_cooked()).transpose()?)
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
            self.texture_bytes += object.bytes();
            if let Some(old) = self.textures.insert(id, Held { slot, generation, object }) {
                self.texture_bytes -= old.object.bytes();
            }
        } else if let Some(material) = upload.get::<Material>() {
            let overridden = self.overridden(material)?;
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
                    // Another slot's: an asset that was unloaded while this renderer was set aside.
                    if let Some(old) = self.materials.insert(id, Held { slot, generation, object }) {
                        renderer.free_material(old.object);
                    }
                }
            }
            if let Some(watcher) = &mut self.watcher {
                watcher.take_material(id, generation, material, &resolve, renderer)?;
            }
        } else if let Some(mesh) = upload.get::<Mesh>() {
            let copy = (id == self.character && self.keep_character).then(|| mesh.to_mesh()).transpose()?;
            if let Some((_, kept)) = self.kept_meshes.iter_mut().find(|(kept, _)| *kept == id) {
                *kept = Some(mesh.to_mesh()?);
            }
            let materials = &self.materials;
            let resolve = |material| materials.get(&material).map(|held| held.object);
            let bounds = scene::mesh_bounds(mesh);
            match self.meshes.get_mut(&id).filter(|held| held.slot == slot) {
                Some(held) if held.generation >= generation => {}
                Some(held) => {
                    renderer.update_mesh(held.object.handle, mesh, resolve)?;
                    *held = Held { slot, generation, object: GpuMesh { handle: held.object.handle, bounds } };
                    self.meshes_replaced = true;
                    if let Some(character) = &copy {
                        self.copies.take_character(character, resolve, renderer)?;
                        self.studio_replaced = self.copies.studio.is_some();
                    }
                }
                None => {
                    // All or nothing, so a mesh the renderer rejects leaves no materials behind.
                    let handle = renderer.upload_mesh_with_slots(mesh, resolve)?;
                    let held = Held { slot, generation, object: GpuMesh { handle, bounds } };
                    // Another slot's: an asset that was unloaded while this renderer was set aside.
                    if let Some(old) = self.meshes.insert(id, held) {
                        renderer.free_mesh(old.object.handle);
                    }
                }
            }
            if id == self.character
                && let Some(watcher) = &mut self.watcher
            {
                watcher.take_character(generation, mesh, &resolve, renderer)?;
            }
            // Only what the renderer took: the held pose is derived from it.
            if copy.is_some() {
                self.character_mesh = copy;
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

    /// Gives an unloaded asset's GPU objects back. A texture's is let go of here, and goes when the last
    /// material bound to it has (its `wgpu::Texture` is shared with their bind groups). A mesh or
    /// material goes to the next [`Self::free_stale`], and from there when nothing draws it and no frame
    /// can be reading it, which the renderer sees to.
    fn unload(&mut self, unloaded: Unloaded) {
        let Unloaded { id, slot, .. } = unloaded;
        if self.textures.get(&id).is_some_and(|held| held.slot == slot)
            && let Some(held) = self.textures.remove(&id)
        {
            self.texture_bytes -= held.object.bytes();
        }
        if self.materials.get(&id).is_some_and(|held| held.slot == slot)
            && let Some(held) = self.materials.remove(&id)
        {
            self.stale_materials.push(held.object);
        }
        if self.meshes.get(&id).is_some_and(|held| held.slot == slot)
            && let Some(held) = self.meshes.remove(&id)
        {
            self.stale_meshes.push(held.object.handle);
        }
        // The renderer keeps the grid until another replaces it: it holds one.
        if self.probe_grid.as_ref().is_some_and(|held| held.slot == slot) {
            self.probe_grid = None;
        }
    }
}

/// What sees each upload as the renderer takes it, and makes copies of its own on that renderer: a game's
/// recolors of its character. Given to [`crate::StageDesc`], or to [`Content::watch_uploads`] before the character
/// loads.
pub trait UploadWatcher: std::any::Any {
    /// A material the renderer has just taken, with what it found its maps through.
    fn take_material<'t>(
        &mut self,
        id: AssetId,
        generation: u32,
        material: &ArchivedMaterial,
        textures: &dyn Fn(AssetId) -> Option<&'t GpuTexture>,
        renderer: &mut Renderer,
    ) -> Result<(), UploadError>;

    /// The character's mesh, which the renderer has just taken, with its materials (the server uploads
    /// dependencies first).
    fn take_character(
        &mut self,
        generation: u32,
        mesh: &ArchivedMesh,
        materials: &dyn Fn(AssetId) -> Option<MaterialHandle>,
        renderer: &mut Renderer,
    ) -> Result<(), UploadError>;

    /// One with nothing made yet, for a new renderer.
    fn for_new_renderer(&self) -> Box<dyn UploadWatcher>;
}

/// What one renderer holds of loaded assets, set aside while a new renderer is tried
/// ([`Content::renderer_rebuilt`]).
pub struct RendererAssets(GpuAssets);

/// `named`, and those of `held` that `in_use` lists: a graph still playing reads them until its next
/// state's start, one waiting reads them from then, and each draws the rest pose without them
/// ([`sapphire_anim::GraphPlayer::clips_in_use`]). The rest go, so what's held is at most
/// [`crate::anim_graph::MAX_HELD_CLIPS`], however many edits come before a state starts.
fn keeping_held(
    mut named: Vec<(AssetId, Handle<Clip>)>,
    held: &[(AssetId, Handle<Clip>)],
    in_use: &[AssetId],
) -> Vec<(AssetId, Handle<Clip>)> {
    for old in held {
        if in_use.contains(&old.0) && !named.iter().any(|(id, _)| *id == old.0) {
            named.push(old.clone());
        }
    }
    named
}

/// A graph and the clips it names, kept resident.
struct HeldGraph {
    graph: Handle<AnimGraph>,
    clips: Vec<(AssetId, Handle<Clip>)>,
    /// What it binds against, for anyone but the character, who has a rig and a mesh besides.
    #[cfg_attr(not(feature = "dev"), allow(dead_code))]
    skeleton: Option<Handle<Skeleton>>,
    /// The graph live tuning took as a document, over the cooked one until a cook lands
    /// ([`newest_saved`]).
    #[cfg(feature = "dev")]
    live: Option<AnimGraph>,
    /// The cooked graph's generation, as [`Content::take_graph_changed`] last saw it.
    #[cfg(feature = "dev")]
    cooked: u32,
}

impl HeldGraph {
    /// Takes handles on the clips it names now, [keeping](keeping_held) those of its own that are `in_use`.
    fn hold_clips(&mut self, server: &mut AssetServer, in_use: &[AssetId]) -> Result<(), LoadError> {
        let Some(ids) = self.clip_ids(server) else { return Ok(()) };
        let clips = ids.into_iter().map(|id| Ok((id, server.load_with(id, LoadPriority::Critical)?)));
        self.clips = keeping_held(clips.collect::<Result<_, LoadError>>()?, &self.clips, in_use);
        Ok(())
    }

    /// The clips it names now, once it's resident.
    fn clip_ids(&self, server: &AssetServer) -> Option<Vec<AssetId>> {
        #[cfg(feature = "dev")]
        if let Some(live) = &self.live {
            return Some(live.clips.iter().map(|clip| clip.id).collect());
        }
        Some(server.get(&self.graph)?.clip_ids().collect())
    }

    /// The graph a character binds: the one live tuning took, or the cooked one.
    fn document(&self, server: &AssetServer) -> Result<AnimGraph, String> {
        #[cfg(feature = "dev")]
        if let Some(live) = &self.live {
            return Ok(live.clone());
        }
        let graph = server.get(&self.graph).ok_or("its cooked graph isn't resident")?;
        graph.to_owned_graph().map_err(|error| error.to_string())
    }

    /// Its clips, as a frame's evaluation reads them. Allocates nothing.
    fn table<'a>(&self, server: &'a AssetServer) -> ClipTable<'a> {
        ClipTable::new(self.clips.iter().map(|(id, clip)| (*id, server.get(clip), server.generation(clip))))
    }

    /// A number that moves with any new version of it or its clips, and as a clip first lands: a graph
    /// that named one before it was resident can bind now.
    fn version(&self, server: &AssetServer) -> u32 {
        self.clips.iter().fold(server.generation(&self.graph), |versions, (_, clip)| {
            let landed = u32::from(server.get(clip).is_some());
            versions.wrapping_add(server.generation(clip)).wrapping_add(landed)
        })
    }
}

/// The asset server, what it has uploaded, and the character it keeps loaded.
pub struct Content {
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
    /// The look-dev town's plants and mountains, once asked for (`--look-dev`). Its cells are the zone's.
    town: Vec<Handle<Mesh>>,
    /// The entry an opening zone's inventory leaves out ([`Self::leave_out_of_zones`]).
    left_out_of_zones: Option<String>,
    /// A character rig, once asked for (`--look-dev`'s).
    rig: Option<Handle<CharacterRig>>,
    /// The skeleton of the character's mesh, with the rig.
    skeleton: Option<Handle<Skeleton>>,
    /// The versions of the character's mesh, rig and skeleton the scene last knew of
    /// ([`Self::take_character_changed`]), once there is a rig.
    character_versions: Option<(u32, u32, u32)>,
    /// The clip the character plays, once asked for (`--clip`), and the version the scene last
    /// knew of ([`Self::take_clip_changed`]).
    clip: Option<Handle<Clip>>,
    clip_version: u32,
    /// The graphs loaded, by whose they are: the character's once asked for (`--graph`, `--arena`), and
    /// whatever a game named. And the versions the scene last knew of ([`Self::take_graph_changed`]).
    graphs: Vec<(GraphKey, HeldGraph)>,
    graph_version: u32,
    /// The sky material the scene draws: [`SKY`] unless the scene wants another.
    sky_id: AssetId,
    /// The sky material, once asked for.
    sky: Option<Handle<Material>>,
    server: AssetServer,
    /// What the server reads through in `dev` builds: packs pushed over live link, over the
    /// platform's.
    #[cfg(feature = "dev")]
    live_packs: Arc<LivePacks>,
    /// The files hot reload reads each fighter's graph from, on a machine that has them.
    #[cfg(feature = "dev")]
    graph_files: Vec<(GraphKey, PathBuf)>,
}

/// Whose graph it is, by a name its owner gives (M2 T2): each binds against its own skeleton.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GraphKey(pub &'static str);

impl GraphKey {
    /// The scene's character's, which [`Content::load_graph`] loads.
    pub const CHARACTER: Self = Self("character");
}

/// What the newest of `owner`'s `files` says its graph is, if there's one and it reads: what binds, once a
/// cook has landed. A cook reads a document when it starts, so one that started before the newest save
/// brings what the file said then, and taking it for the newer because it arrived later would undo the save
/// with nothing to say so, while the file went on saying otherwise. So where there's a file it's the word,
/// and what a cook read is the same text. With no file, on a phone, or one that doesn't read yet or names a
/// clip no pack holds, which live tuning refuses too and can't bind, the cook is all there is. Moves do the
/// same (`LiveTuning::newest_of`).
#[cfg(feature = "dev")]
fn newest_saved(files: &[(GraphKey, PathBuf)], owner: GraphKey, server: &AssetServer) -> Option<AnimGraph> {
    let named = files.iter().filter(|(of, _)| *of == owner).map(|(_, path)| path);
    let mut graph = AnimGraph::from_document(&newest_text(named)?).ok()?;
    graph.resolve(sapphire_assets::graph::clip_id);
    graph.clips.iter().all(|clip| server.contains(clip.id)).then_some(graph)
}

impl Content {
    /// What the mounted packs' meshes draw through, and the skinning that deforms theirs (M3 K1): for a renderer
    /// to prewarm before any of them is uploaded.
    pub fn pipeline_inventory(&self) -> PipelineInventory {
        PipelineInventory {
            meshes: self.server.pipeline_keys(),
            skinning: true,
            ..PipelineInventory::default()
        }
    }

    /// Leaves `entry` of [`PipelineInventory::entries`] out of what a zone builds as it opens, as a load's
    /// `--inventory-without` leaves it out of the packs': what proves a miss fails, whichever pack lists it.
    pub fn leave_out_of_zones(&mut self, entry: Option<String>) {
        self.left_out_of_zones = entry;
    }

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
        server.mount_now(PACK).map_err(|error| missing(&error))?;
        let character_id = character;
        let character = server.load_with(character_id, LoadPriority::Critical).map_err(AppInitError::msg)?;
        tracing::info!(target: LOG_TARGET, pack = PACK, elapsed = ?stopwatch.elapsed(), ?budgets, "content mounted");
        Ok(Self::over_server(
            server,
            character_id,
            character,
            sky,
            #[cfg(feature = "dev")]
            live_packs,
        ))
    }

    /// With nothing uploaded yet, over a `server` that's loading the character.
    pub fn over_server(
        server: AssetServer,
        character_id: AssetId,
        character: Handle<Mesh>,
        sky_id: AssetId,
        #[cfg(feature = "dev")] live_packs: Arc<LivePacks>,
    ) -> Self {
        Self {
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
            left_out_of_zones: None,
            rig: None,
            skeleton: None,
            character_versions: None,
            clip: None,
            clip_version: 0,
            graphs: Vec::new(),
            graph_version: 0,
            sky_id,
            sky: None,
            server,
            #[cfg(feature = "dev")]
            live_packs,
            #[cfg(feature = "dev")]
            graph_files: Vec::new(),
        }
    }

    /// One frame of loading: applies finished reads and decodes, frees what the server unloaded,
    /// and uploads what is ready within the frame's budget.
    pub fn update(&mut self, renderer: &mut Renderer, gpu: &GpuContext) {
        self.server.update();
        for unloaded in self.server.take_unloaded() {
            self.gpu.unload(unloaded);
        }
        self.gpu.free_stale(renderer);
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

    /// Shows `watcher` what's uploaded from now on. Call before [`Self::character`].
    pub fn watch_uploads(&mut self, watcher: Box<dyn UploadWatcher>) {
        self.gpu.watcher = Some(watcher);
    }

    /// The watcher [`Self::watch_uploads`] was given, if it's a `W`.
    pub fn upload_watcher<W: UploadWatcher>(&self) -> Option<&W> {
        let watcher: &dyn std::any::Any = self.gpu.watcher.as_deref()?;
        watcher.downcast_ref()
    }

    /// Changes the anime character materials it uploads from now on by `overrides`
    /// (`--hair-shadow-softness`, `--hair-transmission`, M1 C8): the face test head's and the
    /// look-dev character's. Call before loading them.
    pub fn override_materials(&mut self, overrides: MaterialOverrides) {
        self.gpu.overrides = overrides;
    }

    /// Makes the stage's meshes from the character, on `renderer`: a new version of it replaces them, and
    /// the scene hears of that ([`Self::take_new_studio`]).
    pub fn studio_meshes(&mut self, renderer: &mut Renderer) -> Result<StudioMeshes, AppInitError> {
        let character =
            self.character_mesh().ok_or_else(|| AppInitError::msg("the character isn't loaded"))?;
        let meshes = studio::upload_studio(renderer, character)?;
        self.gpu.copies.studio = Some(meshes);
        Ok(meshes)
    }

    /// Makes the rim rig's meshes from the character, on `renderer`: a new version of it replaces its
    /// figure. The row stays where it stands, for the camera framing it.
    pub fn rim_rig_meshes(&mut self, renderer: &mut Renderer) -> Result<RimRigMeshes, AppInitError> {
        let character =
            self.character_mesh().ok_or_else(|| AppInitError::msg("the character isn't loaded"))?;
        let meshes = rims::upload_rig(renderer, character)?;
        self.gpu.copies.rim_figure = Some(meshes.figure);
        Ok(meshes)
    }

    /// The stage's meshes and the character's bounds, if a new version of the character has replaced them
    /// since the last call, which the stage's pieces stand by.
    pub(crate) fn take_new_studio(&mut self) -> Option<(StudioMeshes, [[f32; 3]; 2])> {
        std::mem::take(&mut self.gpu.studio_replaced).then_some(())?;
        let bounds = self.gpu.meshes.get(&self.character_id)?.object.bounds;
        Some((self.gpu.copies.studio?, bounds))
    }

    /// The character's mesh as last uploaded, if [`Self::keep_character_mesh`] asked for it.
    pub(crate) fn character_mesh(&self) -> Option<&Mesh> {
        self.gpu.character_mesh.as_ref()
    }

    /// Whether a mesh was replaced in place since the last call (hot reload, live link, or a new
    /// version sent to a renderer that was kept): the renderer starts its instances over in their
    /// bind pose. A new renderer's meshes aren't: its scene is sent everything anyway.
    pub(crate) fn take_meshes_replaced(&mut self) -> bool {
        std::mem::take(&mut self.gpu.meshes_replaced)
    }

    /// Whether the character's mesh, rig or skeleton has a new version since the last call or since
    /// [`Self::load_rig`], whichever way it came; they may land frames apart. Always false without
    /// a rig.
    pub(crate) fn take_character_changed(&mut self) -> bool {
        let versions = self.current_character_versions();
        std::mem::replace(&mut self.character_versions, versions) != versions
    }

    /// The versions the character's mesh, rig and skeleton are at, if it has a rig.
    pub fn current_character_versions(&self) -> Option<(u32, u32, u32)> {
        let rig = self.rig.as_ref()?;
        let skeleton = self.skeleton.as_ref().map_or(0, |skeleton| self.server.generation(skeleton));
        Some((self.server.generation(&self.character), self.server.generation(rig), skeleton))
    }

    /// Loads the shader swatches' materials, with the textures they name, until they are on
    /// `renderer`, and returns their handles, top to bottom: the load screen of `--swatches`.
    /// They stay loaded from then on.
    pub(crate) fn swatch_materials(
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
    pub(crate) fn layer_materials(
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
        // Asking again tries a card that failed before, as the character does. Every card is
        // asked for before the first is waited on, so their reads and decodes overlap, as the
        // foliage's do (review PA-009); the server still uploads each card's map and material
        // before its mesh.
        let cards = environment::FOG_CARDS
            .into_iter()
            .map(|id| self.server.load_with(id, LoadPriority::Critical).map_err(AppInitError::msg))
            .collect::<Result<Vec<_>, _>>()?;
        let assets = &mut self.gpu;
        let loaded: Result<(), _> = cards
            .iter()
            .try_for_each(|card| self.server.finish(card, |upload| assets.upload(upload, renderer, gpu)));
        self.fog_cards = cards;
        for unloaded in self.server.take_unloaded() {
            self.gpu.unload(unloaded);
        }
        loaded.map_err(|error| AppInitError::msg(format!("a fog card didn't load: {error}")))?;
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

    /// Mounts zone `id`'s pack, builds what its meshes draw through, and loads until the zone is ready and
    /// everything near `focus`, in the zone's space, is on `renderer`: a zone's load screen. Returns the
    /// streamer that has it from there, whose cells then build nothing as they arrive. A capture pays for
    /// this too, as it would for the uploads that follow, which build what's unbuilt.
    pub fn open_zone<P>(
        &mut self,
        id: AssetId,
        pack: &str,
        focus: [f32; 3],
        settings: StreamSettings,
        renderer: &mut Renderer,
        gpu: &GpuContext,
    ) -> Result<Streamer<P>, AppInitError> {
        if !self.server.is_mounted(pack) {
            self.server.mount_now(pack).map_err(AppInitError::msg)?;
        }
        let mut inventory =
            PipelineInventory { meshes: self.server.pipeline_keys_of(pack), ..PipelineInventory::default() };
        if let Some(entry) = &self.left_out_of_zones {
            inventory.remove(entry);
        }
        let built = renderer.prewarm(&inventory);
        tracing::info!(target: LOG_TARGET, pack, keys = inventory.meshes.len(), built, "a zone's pipelines prewarmed");
        let zone = self.server.load_with(id, LoadPriority::Critical).map_err(AppInitError::msg)?;
        let assets = &mut self.gpu;
        self.server
            .finish(&zone, |upload| assets.upload(upload, renderer, gpu))
            .map_err(|error| AppInitError::msg(format!("zone {id} didn't load: {error}")))?;
        let mut streamer = Streamer::new(zone, settings);
        streamer.preload(focus, &mut self.server, |upload| assets.upload(upload, renderer, gpu));
        Ok(streamer)
    }

    /// Brings what `streamer` holds to `renderer`, a new one.
    pub(crate) fn finish_zone_meshes<P>(
        &mut self,
        streamer: &mut Streamer<P>,
        renderer: &mut Renderer,
        gpu: &GpuContext,
    ) {
        let assets = &mut self.gpu;
        streamer.finish_held(&mut self.server, |upload| assets.upload(upload, renderer, gpu));
    }

    /// What a zone's streaming reads: the server its meshes load through, and each mesh's handle on the
    /// renderer, if it's there.
    pub(crate) fn streaming_parts(
        &mut self,
    ) -> (&mut AssetServer, impl Fn(AssetId) -> Option<MeshHandle> + use<'_>) {
        let meshes = &self.gpu.meshes;
        (&mut self.server, move |id| meshes.get(&id).map(|held| held.object.handle))
    }

    /// Loads until the look-dev town's plants and its mountains are on `renderer`, with their
    /// materials and the maps they name, and returns their meshes: the load screen of
    /// `--look-dev`. They stay loaded from then on. The town itself is a zone ([`Self::open_zone`]).
    pub fn town(&mut self, renderer: &mut Renderer, gpu: &GpuContext) -> Result<TownMeshes, AppInitError> {
        // Asking again tries a town that failed before, as the character does.
        let ids = Plant::ALL.map(Plant::id).into_iter().chain(lookdev::MOUNTAINS);
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
            plants: Plant::ALL.map(|plant| mesh(plant.id())),
            mountains: lookdev::MOUNTAINS.map(mesh),
        })
    }

    /// Loads the asset `id` until it's resident and on `renderer`, with whatever it names, and returns its
    /// handle, which keeps it loaded for as long as it's held. Asking again tries what failed before, as the
    /// character does.
    pub fn hold<T: Asset>(
        &mut self,
        id: AssetId,
        renderer: &mut Renderer,
        gpu: &GpuContext,
    ) -> Result<Handle<T>, AppInitError> {
        let handle = self.server.load_with(id, LoadPriority::Critical).map_err(AppInitError::msg)?;
        let assets = &mut self.gpu;
        let loaded = self.server.finish(&handle, |upload| assets.upload(upload, renderer, gpu));
        for unloaded in self.server.take_unloaded() {
            self.gpu.unload(unloaded);
        }
        loaded.map_err(AppInitError::msg)?;
        Ok(handle)
    }

    /// Reads a held asset, while it's resident.
    pub fn read<T: Asset>(&self, handle: &Handle<T>) -> Option<T::View<'_>> {
        self.server.get(handle)
    }

    /// The version a held asset is at: a new one means hot reload replaced it. Allocates nothing.
    pub fn version_of<T: Asset>(&self, handle: &Handle<T>) -> u32 {
        self.server.generation(handle)
    }

    /// The mesh `id` on the renderer, once it's there.
    pub fn mesh_on_renderer(&self, id: AssetId) -> Option<MeshHandle> {
        self.gpu.meshes.get(&id).map(|held| held.object.handle)
    }

    /// Keeps a copy of the mesh `id` from its next upload on, for what's derived from it, as
    /// [`Self::keep_character_mesh`] does for the character's. Call before loading it.
    pub fn keep_mesh_copy(&mut self, id: AssetId) {
        if !self.gpu.kept_meshes.iter().any(|(kept, _)| *kept == id) {
            self.gpu.kept_meshes.push((id, None));
        }
    }

    /// The copy [`Self::keep_mesh_copy`] kept of the mesh `id`, as last uploaded.
    pub fn mesh_copy(&self, id: AssetId) -> Option<&Mesh> {
        self.gpu.kept_meshes.iter().find(|(kept, _)| *kept == id)?.1.as_ref()
    }

    /// Loads the character rig `id` until it is resident, and the skeleton the character's kept
    /// mesh names ([`Self::keep_character_mesh`]): CPU data, which nothing uploads ([`Self::rig`]
    /// and [`Self::skeleton`] read them). They stay loaded from then on.
    pub(crate) fn load_rig(
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
        let skeleton = self.character_mesh().and_then(|mesh| mesh.skin.as_ref()?.skeleton);
        let skeleton_loaded = match skeleton {
            Some(id) => {
                let skeleton =
                    self.server.load_with(id, LoadPriority::Critical).map_err(AppInitError::msg)?;
                let assets = &mut self.gpu;
                let loaded = self.server.finish(&skeleton, |upload| assets.upload(upload, renderer, gpu));
                self.skeleton = Some(skeleton);
                loaded
            }
            None => Ok(()),
        };
        for unloaded in self.server.take_unloaded() {
            self.gpu.unload(unloaded);
        }
        self.character_versions = self.current_character_versions();
        loaded.map_err(|error| AppInitError::msg(format!("the character rig didn't load: {error}")))?;
        skeleton_loaded
            .map_err(|error| AppInitError::msg(format!("the character's skeleton didn't load: {error}")))
    }

    /// Loads the timeline `id` until it's resident, and hands it back to be played: it holds nothing of the
    /// server's from then on.
    pub fn load_timeline(
        &mut self,
        id: AssetId,
        renderer: &mut Renderer,
        gpu: &GpuContext,
    ) -> Result<std::sync::Arc<Timeline>, AppInitError> {
        let failed = |error: &dyn std::fmt::Display| AppInitError::msg(format!("the timeline {id}: {error}"));
        let assets = &mut self.gpu;
        let handle =
            self.server.load_with::<Timeline>(id, LoadPriority::Critical).map_err(|error| failed(&error))?;
        self.server
            .finish(&handle, |upload| assets.upload(upload, renderer, gpu))
            .map_err(|error| failed(&error))?;
        let timeline = self.server.get(&handle).ok_or_else(|| failed(&"not loaded"))?;
        Ok(std::sync::Arc::new(Timeline::from_archived(timeline)))
    }

    /// Mounts the audio pack, which nothing else reads, and loads the cue `id` with its sounds until
    /// they are resident, then hands back the cue and each sound decoded, but for those already in
    /// `decoded`, as cues sharing a sound would be, which it adds the rest to. Neither stays loaded: a
    /// mixer holds the decoded sound from then on.
    pub(crate) fn load_cue(
        &mut self,
        id: AssetId,
        decoded: &mut HashSet<AssetId>,
        renderer: &mut Renderer,
        gpu: &GpuContext,
    ) -> Result<(Cue, Vec<(AssetId, DecodedSound)>), AppInitError> {
        let failed = |error: &dyn std::fmt::Display| AppInitError::msg(format!("the cue {id}: {error}"));
        if !self.server.is_mounted(AUDIO_PACK) {
            self.server.mount_now(AUDIO_PACK).map_err(|error| failed(&error))?;
        }
        let assets = &mut self.gpu;
        let handle =
            self.server.load_with::<Cue>(id, LoadPriority::Critical).map_err(|error| failed(&error))?;
        self.server
            .finish(&handle, |upload| assets.upload(upload, renderer, gpu))
            .map_err(|error| failed(&error))?;
        let cue = self.server.get(&handle).ok_or_else(|| failed(&"not loaded"))?.to_owned_cue();
        let mut sounds = Vec::with_capacity(cue.sounds.len());
        for &sound in cue.sounds.iter().filter(|&&sound| decoded.insert(sound)) {
            let handle = self
                .server
                .load_with::<Sound>(sound, LoadPriority::Critical)
                .map_err(|error| failed(&error))?;
            self.server
                .finish(&handle, |upload| assets.upload(upload, renderer, gpu))
                .map_err(|error| failed(&error))?;
            let cooked = self.server.get(&handle).ok_or_else(|| failed(&"a sound isn't loaded"))?;
            sounds.push((sound, cooked.decode().map_err(|error| failed(&error))?));
        }
        Ok((cue, sounds))
    }

    /// Loads the clip `id` until it is resident: CPU data, which nothing uploads ([`Self::clip`] reads
    /// it). It stays loaded from then on.
    pub fn load_clip(
        &mut self,
        id: AssetId,
        renderer: &mut Renderer,
        gpu: &GpuContext,
    ) -> Result<(), AppInitError> {
        let clip = self.server.load_with(id, LoadPriority::Critical).map_err(AppInitError::msg)?;
        let assets = &mut self.gpu;
        let loaded = self.server.finish(&clip, |upload| assets.upload(upload, renderer, gpu));
        self.clip_version = self.server.generation(&clip);
        self.clip = Some(clip);
        for unloaded in self.server.take_unloaded() {
            self.gpu.unload(unloaded);
        }
        loaded.map_err(|error| AppInitError::msg(format!("the clip didn't load: {error}")))
    }

    /// The clip [`Self::load_clip`] loaded, and which version of it this is: a new version lands
    /// behind the same handle.
    pub fn clip(&self) -> Option<(&ArchivedClip, u32)> {
        let clip = self.clip.as_ref()?;
        Some((self.server.get(clip)?, self.server.generation(clip)))
    }

    /// Whether the clip has a new version since the last call or since [`Self::load_clip`], which a
    /// character's player may have to be made again for. Always false without a clip.
    pub(crate) fn take_clip_changed(&mut self) -> bool {
        let version = self.clip.as_ref().map_or(0, |clip| self.server.generation(clip));
        std::mem::replace(&mut self.clip_version, version) != version
    }

    /// Loads the graph `id` until it and the clips it names are resident: CPU data, which nothing
    /// uploads ([`Self::graph_document`] and [`Self::graph_clips`] read them). They stay loaded from then on.
    pub fn load_graph(
        &mut self,
        id: AssetId,
        renderer: &mut Renderer,
        gpu: &GpuContext,
    ) -> Result<(), AppInitError> {
        self.load_graph_of(GraphKey::CHARACTER, None, id, renderer, gpu)
    }

    /// Loads the graph `id` as [`Self::load_graph`] does the character's, for `key`, who binds it against
    /// `skeleton` ([`Self::graph_clips_of`] reads it).
    pub fn load_graph_of(
        &mut self,
        key: GraphKey,
        skeleton: Option<Handle<Skeleton>>,
        id: AssetId,
        renderer: &mut Renderer,
        gpu: &GpuContext,
    ) -> Result<(), AppInitError> {
        let mut held = self.load_held_graph(id, renderer, gpu)?;
        held.skeleton = skeleton;
        match self.graphs.iter_mut().find(|(owner, _)| *owner == key) {
            Some((_, graph)) => *graph = held,
            None => self.graphs.push((key, held)),
        }
        self.graph_version = self.graph_versions();
        Ok(())
    }

    /// `key`'s graph, if it's loaded.
    fn held_graph(&self, key: GraphKey) -> Option<&HeldGraph> {
        self.graphs.iter().find(|(owner, _)| *owner == key).map(|(_, held)| held)
    }

    fn load_held_graph(
        &mut self,
        id: AssetId,
        renderer: &mut Renderer,
        gpu: &GpuContext,
    ) -> Result<HeldGraph, AppInitError> {
        let graph = self.server.load_with(id, LoadPriority::Critical).map_err(AppInitError::msg)?;
        let assets = &mut self.gpu;
        let loaded = self.server.finish(&graph, |upload| assets.upload(upload, renderer, gpu));
        let mut held = HeldGraph {
            #[cfg(feature = "dev")]
            cooked: self.server.generation(&graph),
            graph,
            clips: Vec::new(),
            skeleton: None,
            #[cfg(feature = "dev")]
            live: None,
        };
        for unloaded in self.server.take_unloaded() {
            self.gpu.unload(unloaded);
        }
        loaded.map_err(|error| AppInitError::msg(format!("the graph {id} didn't load: {error}")))?;
        held.hold_clips(&mut self.server, &[]).map_err(AppInitError::msg)?;
        Ok(held)
    }

    /// Takes handles on the clips the graphs named here name now, which they keep loaded as their
    /// dependencies, with those their players have `in_use`: a new version may name others. A clip one can't
    /// hold leaves the others' held all the same, and the first failure is what returns. A graph live tuning
    /// took that can never bind gives way to the cook's here, once its clips have said so. Allocates, so only
    /// when [`Self::take_graph_changed`] says so.
    pub fn hold_graph_clips(&mut self, graphs: &[(GraphKey, &[AssetId])]) -> Result<(), LoadError> {
        let held = self.hold_clips_of_graphs(graphs);
        #[cfg(feature = "dev")]
        if self.let_go_of_graphs_that_never_fit() {
            return self.hold_clips_of_graphs(graphs);
        }
        held
    }

    fn hold_clips_of_graphs(&mut self, graphs: &[(GraphKey, &[AssetId])]) -> Result<(), LoadError> {
        let mut held_all = Ok(());
        for (key, in_use) in graphs {
            if let Some((_, held)) = self.graphs.iter_mut().find(|(owner, _)| owner == key) {
                held_all = held_all.and(held.hold_clips(&mut self.server, in_use));
            }
        }
        held_all
    }

    /// The graph a character binds: the one live tuning took, or the one [`Self::load_graph`] loaded.
    pub fn graph_document(&self) -> Result<AnimGraph, String> {
        self.graph_document_of(GraphKey::CHARACTER)
    }

    /// The graph `key` binds: the one live tuning took, or the one it loaded.
    pub fn graph_document_of(&self, key: GraphKey) -> Result<AnimGraph, String> {
        let held = self.held_graph(key).ok_or_else(|| format!("the graph of `{}` isn't loaded", key.0))?;
        held.document(&self.server)
    }

    /// Loads the clips an edited graph of `owner`'s names, until they're resident, keeping those its player
    /// has `in_use`: it goes on until its next state's start. Fails on a clip nothing cooked, keeping
    /// what's held. They wait for the edit to bind or fail, and the next call lets go of what nothing reads.
    #[cfg(feature = "dev")]
    pub fn hold_clips_of(
        &mut self,
        owner: GraphKey,
        graph: &AnimGraph,
        in_use: &[AssetId],
        renderer: &mut Renderer,
        gpu: &GpuContext,
    ) -> Result<(), String> {
        let current = self.held_graph(owner).map_or(Vec::new(), |held| held.clips.clone());
        let mut held = Vec::with_capacity(graph.clips.len());
        for clip in &graph.clips {
            if let Some(found) = current.iter().find(|(id, _)| *id == clip.id) {
                held.push(found.clone());
                continue;
            }
            let failed = |error: &dyn std::fmt::Display| format!("clip `{}`: {error}", clip.name);
            let handle = self.server.load_with(clip.id, LoadPriority::Critical).map_err(|e| failed(&e))?;
            let assets = &mut self.gpu;
            let loaded = self.server.finish(&handle, |upload| assets.upload(upload, renderer, gpu));
            loaded.map_err(|e| failed(&e))?;
            held.push((clip.id, handle));
        }
        let held = keeping_held(held, &current, in_use);
        if let Some((_, current)) = self.graphs.iter_mut().find(|(key, _)| *key == owner) {
            current.clips = held;
        }
        self.graph_version = self.graph_versions();
        Ok(())
    }

    /// Where each fighter's graph documents are, on a machine that has them: what a cook that lands is
    /// weighed against ([`newest_saved`]).
    #[cfg(feature = "dev")]
    pub fn read_graphs_from(&mut self, files: Vec<(GraphKey, PathBuf)>) {
        self.graph_files = files;
    }

    /// Binds `owner` to `graph` from now on, until a cook lands, or to the cook's with none, and returns
    /// what it replaces.
    #[cfg(feature = "dev")]
    pub fn set_live_graph(&mut self, owner: GraphKey, graph: Option<AnimGraph>) -> Option<AnimGraph> {
        let (_, owned) = self.graphs.iter_mut().find(|(key, _)| *key == owner)?;
        std::mem::replace(&mut owned.live, graph)
    }

    /// Why `graph` can't bind to `owner` as it stands, if no landing could change that. A clip still to
    /// land could, and so could one that doesn't fit its own skeleton, because the parts of a new version
    /// land apart. A clip cooked for another skeleton stays that, and so does what the rig doesn't have.
    /// A new rig that brings what a graph names can land after the graph, and then it counts as never
    /// fitting: the cook binds until the next save.
    #[cfg(feature = "dev")]
    fn never_fits(&self, owner: GraphKey, graph: &AnimGraph) -> Option<GraphBindError> {
        let clips = self.graph_clips_of(owner);
        let (skeleton, rig, morph_targets) = match self.held_graph(owner)?.skeleton.as_ref() {
            None => {
                let morph_targets = self.character_mesh()?.morph_targets.len();
                (self.skeleton.as_ref()?, Some(self.rig()?), morph_targets)
            }
            Some(skeleton) => (skeleton, None, 0),
        };
        let error = GraphBinding::new(graph, self.server.get(skeleton)?, rig, morph_targets, &clips).err()?;
        let could_land = match &error {
            GraphBindError::MissingClip { .. } => true,
            GraphBindError::WrongSkeleton { id, .. } => {
                let cooked_for = clips.clip(*id).and_then(|clip| clip.skeleton.as_ref().copied());
                cooked_for == Some(skeleton.id())
            }
            _ => false,
        };
        (!could_land).then_some(error)
    }

    /// Lets go of a graph live tuning took that never fits (`Self::never_fits`), which only shows once its
    /// clips are held, so that the cook's binds in its place: a cook that lands reads the file, which may
    /// say what live tuning refused. Returns whether it did, and then the graph names other clips, for
    /// [`Self::hold_graph_clips`] to hold.
    #[cfg(feature = "dev")]
    fn let_go_of_graphs_that_never_fit(&mut self) -> bool {
        let mut let_go = false;
        for index in 0..self.graphs.len() {
            let (owner, held) = &self.graphs[index];
            let owner = *owner;
            let Some(error) = held.live.as_ref().and_then(|graph| self.never_fits(owner, graph)) else {
                continue;
            };
            tracing::warn!(target: LOG_TARGET, ?owner, %error, "the saved graph can't bind: the cook's does until the next save");
            self.set_live_graph(owner, None);
            let_go = true;
        }
        let_go
    }

    /// The clips the graph names, as a frame's evaluation reads them. Allocates nothing.
    pub fn graph_clips(&self) -> ClipTable<'_> {
        self.graph_clips_of(GraphKey::CHARACTER)
    }

    /// The clips `key`'s graph names, likewise.
    pub fn graph_clips_of(&self, key: GraphKey) -> ClipTable<'_> {
        self.held_graph(key).map_or_else(ClipTable::empty, |held| held.table(&self.server))
    }

    /// Whether `key`'s graph holds the clip `id`, landed or not.
    #[cfg(feature = "dev")]
    pub fn holds_graph_clip(&self, key: GraphKey, id: AssetId) -> bool {
        self.held_graph(key).is_some_and(|held| held.clips.iter().any(|(clip, _)| *clip == id))
    }

    /// A number that moves with any new version of a graph or its clips.
    pub fn graph_versions(&self) -> u32 {
        self.graphs.iter().fold(0, |versions, (_, held)| versions.wrapping_add(held.version(&self.server)))
    }

    /// An edit for one that waits for the rest of a new version of itself. This edit may be all it waits
    /// for: a cook reads what was saved, so a graph that never fits can be the only one there is, and no
    /// landing would end that. So the edit gets the try a landing gets, `try_again`, which says if it still
    /// waits, and it starts over on it. If its other parts still don't fit the edit is refused, and it goes
    /// back to waiting for what it was, with the clips the edit's took the place of: the next landing needs
    /// them held to be seen.
    #[cfg(feature = "dev")]
    pub fn end_wait_on(
        &mut self,
        owner: GraphKey,
        graph: AnimGraph,
        bound: Result<(), String>,
        mut try_again: impl FnMut(&mut Self) -> bool,
    ) -> Result<(), String> {
        let refusal = match bound {
            Ok(()) => {
                let before = self.set_live_graph(owner, Some(graph));
                if !try_again(self) {
                    return Ok(());
                }
                self.set_live_graph(owner, before);
                "a new version of this character is still landing: push again once it has".to_owned()
            }
            Err(error) => format!("a new version of this character waits for a graph that binds: {error}"),
        };
        try_again(self);
        Err(refusal)
    }

    /// Whether a graph or a clip one names has a new version since the last call or since
    /// [`Self::load_graph`], which a character's graph has to be bound again for. Always false without a
    /// graph. A cook that has landed reads its graph's file, if it has one (`newest_saved`), and otherwise
    /// it allocates nothing.
    pub fn take_graph_changed(&mut self) -> bool {
        let version = self.graph_versions();
        #[cfg(feature = "dev")]
        {
            for (owner, held) in &mut self.graphs {
                let cooked = self.server.generation(&held.graph);
                if std::mem::replace(&mut held.cooked, cooked) != cooked {
                    held.live = newest_saved(&self.graph_files, *owner, &self.server);
                    if held.live.is_some() {
                        tracing::info!(target: LOG_TARGET, ?owner, "a cook landed: the graph is what its file says");
                    }
                }
            }
        }
        std::mem::replace(&mut self.graph_version, version) != version
    }

    /// The rig [`Self::load_rig`] loaded.
    pub(crate) fn rig(&self) -> Option<&ArchivedCharacterRig> {
        self.rig.as_ref().and_then(|rig| self.server.get(rig))
    }

    /// The skeleton [`Self::load_rig`] loaded.
    pub(crate) fn skeleton(&self) -> Option<&ArchivedSkeleton> {
        self.skeleton.as_ref().and_then(|skeleton| self.server.get(skeleton))
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
        let fresh = self.gpu.for_new_renderer();
        let previous = std::mem::replace(&mut self.gpu, fresh);
        RendererAssets(previous)
    }

    /// Goes back to the renderer whose objects `previous` holds, after a new one failed. Every
    /// asset is sent again, and the old renderer takes only what it lacks, so nothing it holds is
    /// uploaded twice and the handles the scene draws with stay valid.
    pub fn keep_renderer(&mut self, previous: RendererAssets) {
        self.gpu = previous.0;
        let server = &self.server;
        self.gpu.take_stale(|id| server.is_resident(id));
        self.server.reupload_all();
    }

    /// The bytes of textures on the renderer, against the zone texture pool's budget (spec §3.4).
    #[must_use]
    pub fn texture_bytes(&self) -> u64 {
        self.gpu.texture_bytes
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

    /// Takes a handle on a cooked set of moves, whose new versions then reach
    /// [`AssetChange::Reloaded`](sapphire_assets::AssetChange) (M2 T2). It loads as anything does, and
    /// stays. Fails for an asset no pack holds.
    #[cfg(feature = "dev")]
    pub fn hold_move_set(&mut self, id: AssetId) -> Result<Handle<sapphire_moves::MoveSet>, LoadError> {
        self.server.load_with(id, LoadPriority::Normal)
    }

    /// A cooked set of moves [`Self::hold_move_set`] holds, if it's loaded.
    #[cfg(feature = "dev")]
    pub fn move_set(
        &self,
        set: &Handle<sapphire_moves::MoveSet>,
    ) -> Option<&sapphire_moves::ArchivedMoveSet> {
        self.server.get(set)
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
    pub fn stats(&self) -> AssetStats {
        self.server.stats()
    }

    /// Unloads every asset nothing holds or pins and frees the textures uploaded for them at once,
    /// rather than at the next [`Self::update`]: the response to a system memory warning (review
    /// PA-004, spec §6.3). What the scene draws stays, since the sandbox holds a handle to all of
    /// it; what goes is what reloads and rebuilds left behind. Their meshes and materials go from the
    /// next [`Self::update`] (see `GpuAssets::unload`). Returns the server's counters before and after.
    pub fn trim(&mut self) -> (AssetStats, AssetStats) {
        let before = self.server.stats();
        self.server.unload_unused();
        for unloaded in self.server.take_unloaded() {
            self.gpu.unload(unloaded);
        }
        (before, self.server.stats())
    }
}

/// Review RC-1, through the asset server, the uploader and the scene: what a re-cook does to a
/// posed character while the app runs. Hot reload and a live-link push land the same way, as a
/// remount of the pack, and differ only in what tells the app to remount.
/// The text of the last modified of `files`, if one reads.
#[cfg(feature = "dev")]
pub fn newest_text<'a>(files: impl Iterator<Item = &'a PathBuf>) -> Option<String> {
    let modified = |path: &&PathBuf| std::fs::metadata(path).and_then(|file| file.modified()).ok();
    std::fs::read_to_string(files.max_by_key(modified)?).ok()
}

#[cfg(test)]
mod new_renderer_tests {
    use sapphire_assets::material::{AnimeCharacterMaterial, CharacterSurface, HairShading};
    use sapphire_assets::mesh::{
        MaterialSlot, PositionFormat, Submesh, VERTEX_COLOR_NEUTRAL, Vertex, VertexFlags,
    };
    use sapphire_assets::pack::{Codec, PackAsset, write_pack};
    use sapphire_assets::texture::Swizzle;
    use sapphire_assets::{AssetKind, MemoryPackSource};

    use super::*;

    const CHARACTER: AssetId = AssetId::from_bytes([0x71; 16]);
    const OVERRIDES: MaterialOverrides =
        MaterialOverrides { hair_shadow_softness: None, hair_transmission: Some(0.8) };

    /// A material as a cook makes it, with `surface`.
    fn material(surface: CharacterSurface) -> Material {
        let swizzles = [Swizzle::IDENTITY; AnimeCharacterMaterial::TEXTURE_SLOTS.len()];
        let document = AnimeCharacterMaterial { surface, ..AnimeCharacterMaterial::default() };
        Material::from_document(&document, &swizzles).expect("a valid material")
    }

    /// Content over a pack in memory, with no platform and no renderer: all that deciding what a
    /// material uploads as takes.
    fn content() -> Content {
        let vertex = |position: [f32; 3]| Vertex {
            position,
            normal: [0.0, 0.0, 1.0],
            outline_normal: [0.0, 0.0, 1.0],
            uv: [0.0, 0.0],
            color: VERTEX_COLOR_NEUTRAL,
            flags: VertexFlags::NONE,
        };
        let mesh = Mesh::from_vertices(
            &[vertex([0.0, 0.0, 0.0]), vertex([1.0, 0.0, 0.0]), vertex([0.0, 1.0, 0.0])],
            vec![0, 1, 2],
            vec![Submesh { first_index: 0, index_count: 3, material: 0 }],
            vec![MaterialSlot::embedded("skin", material(CharacterSurface::Body))],
            PositionFormat::Unorm16,
        );
        let bytes = mesh.to_cooked().expect("cooked");
        let asset = PackAsset::encode(CHARACTER, AssetKind::Mesh, Codec::Lz4, &bytes, Vec::new(), 3);
        let source = MemoryPackSource::new();
        source.insert(PACK, write_pack(vec![asset.expect("encoded")]).expect("a pack"));
        #[cfg(feature = "dev")]
        let live_packs = Arc::new(LivePacks::new(source));
        #[cfg(feature = "dev")]
        let source = live_packs.clone();
        let budgets = AssetBudgets { upload_per_frame: u64::MAX, resident: [u64::MAX; AssetKind::ALL.len()] };
        let mut server = AssetServer::new(source, budgets);
        server.mount_now(PACK).expect("the pack");
        let character = server.load_with(CHARACTER, LoadPriority::Critical).expect("loading");
        Content::over_server(
            server,
            CHARACTER,
            character,
            SKY,
            #[cfg(feature = "dev")]
            live_packs,
        )
    }

    /// The hair transmission `content` would upload `hair` with.
    fn transmission(content: &Content, hair: &Material) -> Option<f32> {
        let bytes = hair.to_cooked().expect("cooked");
        let cooked = Material::read(&bytes).expect("a valid material");
        let edited = content.gpu.overridden(cooked).expect("reads")?;
        let mut edited = Material::read(&edited).expect("a valid material").to_material().expect("reads");
        let mut strength = None;
        edited.edit_anime_character(|params| strength = Some(params.surface[3]));
        strength
    }

    /// The windowed app sets the overrides and then makes its renderer, and a lost device or a new tier
    /// makes another: every one of them starts with `renderer_rebuilt`, which had dropped the overrides,
    /// so `--hair-transmission` and `--hair-shadow-softness` only ever reached a capture.
    #[test]
    fn a_new_renderer_uploads_what_the_command_line_changed() {
        let hair = material(CharacterSurface::Hair(HairShading::default()));
        let mut content = content();
        assert_eq!(transmission(&content, &hair), None, "nothing's asked yet");
        content.override_materials(OVERRIDES);
        assert_eq!(transmission(&content, &hair), Some(0.8));

        let previous = content.renderer_rebuilt();
        assert_eq!(transmission(&content, &hair), Some(0.8), "the renderer the app starts with");
        let _ = content.renderer_rebuilt();
        assert_eq!(transmission(&content, &hair), Some(0.8), "and one after a device loss or a tier change");
        content.keep_renderer(previous);
        assert_eq!(transmission(&content, &hair), Some(0.8), "and going back to one that was kept");
    }
}

#[cfg(all(test, feature = "dev"))]
mod tests {
    use sapphire_anim::{ClipSource, Joint, JointTransform};
    use sapphire_assets::material::AnimeCharacterMaterial;
    use sapphire_assets::mesh::{
        Influences, MaterialSlot, PositionFormat, Skin, SkinBind, Submesh, VERTEX_COLOR_NEUTRAL, Vertex,
        VertexFlags, VertexInfluence, quantize_weights,
    };
    use sapphire_assets::pack::{Codec, PackAsset, write_pack};
    use sapphire_assets::rig::{HumanBone, HumanoidJoint};
    use sapphire_assets::texture::Swizzle;
    use sapphire_assets::{AssetKind, MemoryPackSource};
    use sapphire_math::{Mat4, Vec3};
    use sapphire_render::graph::{FrameGraph, GraphExecutor};
    use sapphire_render::{RenderSettings, SkinningStats};
    use sapphire_rhi::{GpuContextDesc, GpuInstance, GpuInstanceDesc, wgpu};

    use super::*;
    use crate::anim_graph::{GraphScript, GraphShow};
    use crate::lookdev::Shot;
    use crate::scene::{LookDev, Scene};
    use crate::studio::StudioLayout;

    const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8UnormSrgb;
    const SIDE: u32 = 32;

    fn triangle(x: f32) -> Mesh {
        let vertex = |position: [f32; 3]| Vertex {
            position,
            normal: [0.0, 0.0, 1.0],
            outline_normal: [0.0, 0.0, 1.0],
            uv: [0.0, 0.0],
            color: VERTEX_COLOR_NEUTRAL,
            flags: VertexFlags::NONE,
        };
        let swizzles = [Swizzle::IDENTITY; AnimeCharacterMaterial::TEXTURE_SLOTS.len()];
        let material =
            Material::from_document(&AnimeCharacterMaterial::default(), &swizzles).expect("a valid material");
        Mesh::from_vertices(
            &[vertex([x, 0.0, 0.0]), vertex([1.0, 0.0, 0.0]), vertex([0.0, 2.0, 0.0])],
            vec![0, 1, 2],
            vec![Submesh { first_index: 0, index_count: 3, material: 0 }],
            vec![MaterialSlot::embedded("skin", material)],
            PositionFormat::Unorm16,
        )
    }

    /// `bytes` as a pack holds them.
    fn packed(id: AssetId, kind: AssetKind, bytes: &[u8]) -> PackAsset {
        packed_with(id, kind, bytes, Vec::new())
    }

    /// `bytes` as a pack holds them, naming `dependencies`.
    fn packed_with(id: AssetId, kind: AssetKind, bytes: &[u8], dependencies: Vec<AssetId>) -> PackAsset {
        PackAsset::encode(id, kind, Codec::Lz4, bytes, dependencies, 3).expect("encoded")
    }

    /// The test character's skeleton.
    const SKELETON: AssetId = AssetId::from_bytes([0x5e; 16]);
    /// A clip for the character: its root swings 1.5 m from one side to the other over [`SWING_TICKS`].
    const SWING: AssetId = AssetId::from_bytes([0x5f; 16]);
    const SWING_TICKS: u16 = 30;
    /// A graph for the character that plays [`SWING`].
    const GRAPH: AssetId = AssetId::from_bytes([0x60; 16]);
    /// A cooked set of moves, as the graph show plays.
    const MOVES: AssetId = AssetId::from_bytes([0x61; 16]);
    /// A second clip, as [`SWING`], that a new version of the graph can play instead.
    const OTHER_SWING: AssetId = AssetId::from_bytes([0x62; 16]);
    /// Another rig's skeleton.
    const OTHER_RIG: AssetId = AssetId::from_bytes([0x63; 16]);
    /// [`SWING`], for another rig.
    const OTHER_RIGS_SWING: AssetId = AssetId::from_bytes([0x64; 16]);

    /// A pack with the look-dev character, its first vertex at `x`, its rig and its skeleton: every
    /// required humanoid bone a joint of its own under the root, and `extra` joints more.
    fn pack(x: f32, extra: usize) -> Vec<u8> {
        pack_with_decay(x, extra, 5)
    }

    fn pack_with_decay(x: f32, extra: usize, decay: u32) -> Vec<u8> {
        pack_of(x, extra, decay, 0)
    }

    fn pack_of(x: f32, extra: usize, decay: u32, recovery: u32) -> Vec<u8> {
        pack_of_graph(x, extra, swing_graph(decay), SWING, recovery)
    }

    /// The first pack, with `graph` naming `clip`.
    fn pack_of_graph(x: f32, extra: usize, graph: AnimGraph, clip: AssetId, recovery: u32) -> Vec<u8> {
        write_pack(pack_assets(x, extra, graph, clip, recovery)).expect("a pack")
    }

    /// What [`pack_of_graph`] packs.
    fn pack_assets(x: f32, extra: usize, graph: AnimGraph, clip: AssetId, recovery: u32) -> Vec<PackAsset> {
        let count = HumanBone::REQUIRED.len() + extra;
        let at = |joint: usize| [joint as f32 * 0.1, 1.0, 0.0];
        let joints: Vec<Joint> = (0..count)
            .map(|joint| Joint {
                name: format!("joint {joint}"),
                parent: (joint > 0).then_some(0),
                rest: JointTransform {
                    translation: if joint > 0 { [joint as f32 * 0.1, 0.0, 0.0] } else { at(0) },
                    ..JointTransform::IDENTITY
                },
            })
            .collect();
        let binds = (0..count)
            .map(|joint| SkinBind {
                joint: joint as u16,
                inverse_bind_matrix: Mat4::from_translation(-Vec3::from(at(joint))).to_cols_array_2d(),
            })
            .collect();
        let held = VertexInfluence { binds: [0; 4], weights: quantize_weights([1.0, 0.0, 0.0, 0.0]) };
        let mut mesh = triangle(x);
        mesh.skin = Some(Skin {
            skeleton: Some(SKELETON),
            joint_count: count as u32,
            binds,
            influences: Influences::pack(&[held; 3], count),
        });
        let mut humanoid: Vec<_> = HumanBone::REQUIRED
            .iter()
            .enumerate()
            .map(|(joint, &bone)| HumanoidJoint { bone, joint: joint as u16 })
            .collect();
        humanoid.sort_by_key(|entry| entry.bone);
        let rig = CharacterRig {
            joint_count: count as u32,
            material_slot_count: 1,
            humanoid,
            ..CharacterRig::default()
        };
        vec![
            packed_with(
                lookdev::CHARACTER,
                AssetKind::Mesh,
                &mesh.to_cooked().expect("cooked"),
                vec![SKELETON],
            ),
            packed(lookdev::RIG, AssetKind::Rig, &rig.to_cooked().expect("cooked")),
            packed(SKELETON, AssetKind::Skeleton, &Skeleton::new(&joints).to_cooked().expect("cooked")),
            packed_with(SWING, AssetKind::Clip, &swing(count).to_cooked().expect("cooked"), vec![SKELETON]),
            packed_with(
                OTHER_SWING,
                AssetKind::Clip,
                &swing(count).to_cooked().expect("cooked"),
                vec![SKELETON],
            ),
            packed_with(GRAPH, AssetKind::AnimGraph, &graph.to_cooked().expect("cooked"), vec![clip]),
            packed(MOVES, AssetKind::MoveSet, &show_moves(recovery).to_cooked().expect("cooked")),
            // A swing that binds to none of this character's joints.
            packed(
                OTHER_RIG,
                AssetKind::Skeleton,
                &Skeleton::new(&[Joint {
                    name: "not on this rig".to_owned(),
                    parent: None,
                    rest: JointTransform::IDENTITY,
                }])
                .to_cooked()
                .expect("cooked"),
            ),
            packed_with(
                OTHER_RIGS_SWING,
                AssetKind::Clip,
                &Clip { skeleton: Some(OTHER_RIG), ..swing(1) }.to_cooked().expect("cooked"),
                vec![OTHER_RIG],
            ),
        ]
    }

    /// The graph show's moves as a cook makes them, with `strike` so many ticks longer.
    fn show_moves(longer_by: u32) -> sapphire_moves::MoveSet {
        let text = include_str!("../../../content/lookdev/character/show.moves.ron");
        let mut document = sapphire_moves::MoveSetDocument::parse(text).expect("parses");
        document.moves[0].recovery += longer_by;
        document.compile().expect("compiles")
    }

    /// The swing's document, looping. Its decay is all that tells one version from another. Its source is
    /// the clip's own ID, so what a file of it resolves to is what a cook of it does.
    fn swing_document(decay: u32) -> String {
        format!(
            r#"(type: "sapphire::assets::AnimGraph", version: 1, data: (
                clips: [(name: "swing", source: "{SWING}")],
                locomotion: (
                    start: "ground",
                    states: [(name: "ground", plays: Clip(clip: "swing", looping: true))],
                ),
                combat: (decay: {decay}, exit: {decay}, moves: []),
            ))"#
        )
    }

    /// [`swing_document`], as a cook makes it.
    fn swing_graph(decay: u32) -> AnimGraph {
        let mut graph = AnimGraph::from_document(&swing_document(decay)).expect("a good document");
        graph.resolve(sapphire_assets::graph::clip_id);
        graph
    }

    /// The root from 0.75 m to the character's right to 0.75 m to its left, a clip for `joints` joints.
    fn swing(joints: usize) -> Clip {
        let from = [-0.75, 1.0, 0.0];
        let to = [0.75, 1.0, 0.0];
        let range = sapphire_anim::VectorRange::enclosing([from, to]);
        Clip {
            name: "swing".into(),
            skeleton: Some(SKELETON),
            joint_count: joints as u32,
            rate: sapphire_assets::clip::DEFAULT_RATE,
            ticks: SWING_TICKS,
            translations: vec![sapphire_anim::VectorTrack { joint: 0, count: 2, first: 0, range }],
            vector_ticks: vec![0, SWING_TICKS],
            vector_keys: vec![range.encode(from), range.encode(to)],
            ..Clip::default()
        }
    }

    /// The look-dev stage over a pack in memory, run frame by frame as the app runs it.
    struct Stage {
        // Drop order: what lives on the GPU before the device.
        scene: Scene,
        content: Content,
        graph: GraphExecutor,
        renderer: Renderer,
        target: wgpu::Texture,
        gpu: GpuContext,
        source: Arc<MemoryPackSource>,
    }

    impl Stage {
        fn new() -> Self {
            Self::over_pack(pack(0.0, 0))
        }

        fn over_pack(pack: Vec<u8>) -> Self {
            Self::over_pack_with(pack, Self::in_town)
        }

        /// The mannequin's kind of scene: the character alone, with no rig, as `--stage` and `--rim-rig`
        /// draw it.
        fn mannequin_of(mesh: &Mesh) -> Self {
            let bytes = mesh.to_cooked().expect("cooked");
            let pack = write_pack(vec![packed(lookdev::CHARACTER, AssetKind::Mesh, &bytes)]).expect("a pack");
            Self::over_pack_with(pack, |_, _, _, mesh, bounds| Scene::with_character(mesh, bounds, false))
        }

        /// The look-dev stage around the character.
        fn in_town(
            content: &mut Content,
            renderer: &mut Renderer,
            gpu: &GpuContext,
            mesh: MeshHandle,
            bounds: [[f32; 3]; 2],
        ) -> Scene {
            let (pose, figure) = lookdev::hold(content, renderer, gpu).expect("the character's pose");
            // What the town draws doesn't matter here.
            let bytes = triangle(0.0).to_cooked().expect("cooked");
            let plain = Mesh::read(&bytes).expect("a valid mesh");
            let materials = renderer.upload_embedded_materials(plain).expect("materials");
            let plain = renderer.upload_mesh(plain, &materials).expect("a mesh");
            let town = TownMeshes {
                plants: Plant::ALL.map(|_| plain),
                mountains: lookdev::MOUNTAINS.map(|_| plain),
            };
            let mut scene = Scene::with_character(mesh, bounds, false);
            scene.spawn_look_dev(Shot::Hero, LookDev { town, fog_cards: None, pose, figure });
            scene
        }

        fn over_pack_with(
            pack: Vec<u8>,
            scene: impl FnOnce(&mut Content, &mut Renderer, &GpuContext, MeshHandle, [[f32; 3]; 2]) -> Scene,
        ) -> Self {
            let source = Arc::new(MemoryPackSource::new());
            source.insert(PACK, pack);
            let live_packs = Arc::new(LivePacks::new(source.clone()));
            let budgets =
                AssetBudgets { upload_per_frame: u64::MAX, resident: [u64::MAX; AssetKind::ALL.len()] };
            let mut server = AssetServer::new(live_packs.clone(), budgets);
            server.mount_now(PACK).expect("the pack");
            let character = server.load_with(lookdev::CHARACTER, LoadPriority::Critical).expect("loading");
            let mut content = Content::over_server(server, lookdev::CHARACTER, character, SKY, live_packs);
            content.keep_character_mesh();
            let instance = GpuInstance::new(&GpuInstanceDesc::default()).expect("instance");
            let gpu = GpuContext::new(&instance, &GpuContextDesc::default(), None).expect("device");
            let mut renderer = Renderer::new(&gpu, FORMAT, RenderSettings::default()).expect("renderer");
            let (mesh, bounds) = content.character(&mut renderer, &gpu).expect("the character");
            let scene = scene(&mut content, &mut renderer, &gpu, mesh, bounds);
            let target = gpu.device().create_texture(&wgpu::TextureDescriptor {
                label: None,
                size: wgpu::Extent3d { width: SIDE, height: SIDE, depth_or_array_layers: 1 },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: FORMAT,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
                view_formats: &[],
            });
            Self { scene, content, graph: GraphExecutor::new(), renderer, target, gpu, source }
        }

        /// One frame, as the app runs it: what the frame's loading uploaded, then extraction, and
        /// what the frame skins.
        fn frame(&mut self) -> SkinningStats {
            self.content.update(&mut self.renderer, &self.gpu);
            self.scene.follow_content(&mut self.content, &mut ());
            self.scene.step();
            self.scene.animate(&self.content, 1.0, &mut ());
            let packet = self.scene.frame_packet(1.0, Some((SIDE, SIDE)), &mut ());
            self.renderer.prepare(&self.gpu, packet);
            let stats = self.renderer.stats().skinning;
            let view = self.target.create_view(&Default::default());
            let mut graph = FrameGraph::new();
            let target = graph.import_texture("target", &view);
            self.renderer.record(&mut graph, target).expect("a view and a target");
            self.graph.execute(&self.gpu, graph).expect("a valid graph");
            self.scene.end_frame();
            stats
        }

        /// Plays the swing on the character, framed so its triangle is in view whichever way it swings.
        fn play_swing(&mut self) {
            self.content.load_clip(SWING, &mut self.renderer, &self.gpu).expect("the clip");
            self.scene.play_clip(lookdev::player(&self.content).expect("a player"));
            self.frame_on_character();
        }

        /// Plays the graph on the character, from a script that stands still.
        fn play_graph(&mut self) {
            self.content.load_graph(GRAPH, &mut self.renderer, &self.gpu).expect("the graph");
            let driver = lookdev::graph_driver(&self.content).expect("a driver");
            self.scene.play_graph(driver, GraphShow::new(GraphScript::Stand));
            self.frame_on_character();
        }

        /// Frames the character from the front, close enough for its triangle to fill much of the target.
        fn frame_on_character(&mut self) {
            *self.scene.camera_mut() =
                crate::orbit::OrbitCamera::framing([[-1.0, 0.0, -1.0], [1.0, 2.0, 1.0]]);
        }

        /// One frame as [`Self::frame`] draws it, without a fixed step, read back.
        fn pixels(&mut self) -> Vec<u8> {
            let target = self.gpu.device().create_texture(&wgpu::TextureDescriptor {
                label: None,
                size: wgpu::Extent3d { width: SIDE, height: SIDE, depth_or_array_layers: 1 },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: FORMAT,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
                view_formats: &[],
            });
            let readback = crate::readback::Readback::new(&self.gpu, SIDE, SIDE);
            self.content.update(&mut self.renderer, &self.gpu);
            self.scene.follow_content(&mut self.content, &mut ());
            self.scene.animate(&self.content, 1.0, &mut ());
            let packet = self.scene.frame_packet(1.0, Some((SIDE, SIDE)), &mut ());
            self.renderer.prepare(&self.gpu, packet);
            let view = target.create_view(&Default::default());
            let mut graph = FrameGraph::new();
            let imported = graph.import_texture("target", &view);
            self.renderer.record(&mut graph, imported).expect("a view and a target");
            readback.record(&mut graph, imported);
            self.graph.execute(&self.gpu, graph).expect("a valid graph");
            self.scene.end_frame();
            readback.read(&self.gpu).expect("its pixels")
        }

        /// The pack re-cooked, as a watched file or a push hands it over.
        fn recook(&mut self, x: f32, extra: usize) {
            self.source.insert(PACK, pack(x, extra));
            self.content.remount(PACK).expect("the new pack");
        }

        /// The pack re-cooked with `extra` joints more and a graph of `decay`, with `late` still as the
        /// first pack has them, as a cook's pack lands in parts. Bytes that don't change make no new
        /// version, so what's late lands when a later pack brings it.
        fn recook_all_but(&mut self, late: &[AssetId], extra: usize, decay: u32) {
            let first = pack_assets(0.0, 0, swing_graph(5), SWING, 0);
            let mut assets = pack_assets(0.0, extra, swing_graph(decay), SWING, 0);
            assets.retain(|asset| !late.contains(&asset.id));
            assets.extend(first.into_iter().filter(|asset| late.contains(&asset.id)));
            self.source.insert(PACK, write_pack(assets).expect("a pack"));
            self.content.remount(PACK).expect("the new pack");
        }

        /// Frames until `landed` says so.
        fn frames_until(&mut self, landed: impl Fn(&Self) -> bool) {
            for _ in 0..10_000 {
                self.frame();
                if landed(self) {
                    return;
                }
                std::thread::sleep(std::time::Duration::from_millis(1));
            }
            panic!("it never landed");
        }

        /// How many joints the pose the character's graph plays has.
        fn joints(&self) -> usize {
            self.scene.graph_player().expect("playing").pose().joint_count()
        }

        /// The versions the character's mesh, rig and skeleton are at.
        fn versions(&self) -> (u32, u32, u32) {
            self.content.current_character_versions().expect("a rig")
        }

        /// Frames until the character's mesh, rig and skeleton are at `versions`, and what the last
        /// skinned.
        fn until(&mut self, versions: (u32, u32, u32)) -> SkinningStats {
            for _ in 0..10_000 {
                let stats = self.frame();
                if self.versions() == versions {
                    return stats;
                }
                std::thread::sleep(std::time::Duration::from_millis(1));
            }
            panic!("still at {:?}", self.versions());
        }
    }

    #[test]
    #[ignore = "needs a GPU"]
    fn a_mesh_replaced_in_place_is_posed_in_the_frame_that_replaces_it() {
        let mut stage = Stage::new();
        let posed = stage.frame();
        assert!(posed.palette_bytes > 0, "the character starts posed: {posed:?}");
        assert_eq!(stage.frame().palette_bytes, 0, "a held pose is sent once");
        assert!(!stage.content.take_meshes_replaced(), "nothing replaced yet");

        // The character's mesh alone changes, as a sidecar's LOD edit changes it.
        stage.recook(0.1, 0);
        let replaced = stage.until((1, 0, 0));
        // No frame draws the new mesh in its bind pose: this frame's skin is the held pose, not the
        // bind pose a new instance would start from.
        assert_eq!(replaced.ignored_poses, 0);
        assert_eq!(replaced.palette_bytes, posed.palette_bytes, "posed: {replaced:?}");
        let after = stage.frame();
        assert_eq!((after.skinned, after.palette_bytes), (0, 0), "and sent once");
    }

    #[test]
    #[ignore = "needs a GPU"]
    fn a_new_skeleton_holds_the_character_anew() {
        let mut stage = Stage::new();
        let joints = HumanBone::REQUIRED.len() as u32;
        let per_joint = stage.frame().palette_bytes / joints;
        assert!(per_joint > 0);
        stage.frame();

        // One joint more, in the character's mesh, rig and skeleton: the palette it was held with no longer
        // fits, whichever of them lands first.
        stage.recook(0.0, 1);
        let settled = stage.until((1, 1, 1));
        assert_eq!(settled.ignored_poses, 0, "{settled:?}");
        assert_eq!(settled.palette_bytes, (joints + 1) * per_joint, "the new palette: {settled:?}");
        assert_eq!(stage.frame().palette_bytes, 0, "held again");
    }

    /// A new graph binds at once and takes effect at the next state's start (T2's seam), with the
    /// character drawn on every frame between. A state's start here is the swing's own: nothing in the
    /// script changes state, so the new graph waits.
    #[test]
    #[ignore = "needs a GPU"]
    fn a_new_graph_waits_for_the_next_states_start() {
        let mut stage = Stage::new();
        stage.play_graph();
        for _ in 0..3 {
            stage.frame();
        }
        assert!(!stage.content.take_graph_changed(), "nothing's new");

        stage.source.insert(PACK, pack_with_decay(0.0, 0, 9));
        stage.content.remount(PACK).expect("the new pack");
        let mut waiting = false;
        for _ in 0..1000 {
            stage.frame();
            waiting = stage.scene.graph_player().expect("playing").has_replacement();
            if waiting {
                break;
            }
        }
        assert!(waiting, "the new version reached the character");
        let stats = stage.frame();
        assert_eq!(stats.ignored_poses, 0, "{stats:?}");
    }

    /// A new graph that plays another clip takes effect at the next state's start, and a character
    /// standing in its one state may not reach one for a long time. Until then the old graph goes on
    /// playing, which needs the clip it names: without it, it draws the rest pose.
    #[test]
    #[ignore = "needs a GPU"]
    fn a_new_graph_naming_another_clip_leaves_the_old_one_its_clip() {
        let mut stage = Stage::new();
        stage.play_graph();
        for _ in 0..3 {
            stage.frame();
        }

        let mut other = swing_graph(5);
        other.resolve(|_| OTHER_SWING);
        stage.source.insert(PACK, pack_of_graph(0.0, 0, other, OTHER_SWING, 0));
        stage.content.remount(PACK).expect("the new pack");
        let mut waiting = false;
        for _ in 0..1000 {
            stage.frame();
            waiting = stage.scene.graph_player().expect("playing").has_replacement();
            if waiting {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        assert!(waiting, "the new version reached the character");

        // Its root swings from one side to the other, and stands at x 0 in the rest pose.
        let mut furthest = 0.0f32;
        for _ in 0..SWING_TICKS {
            stage.frame();
            let root = stage.scene.graph_player().expect("playing").pose().model()[0].translation;
            furthest = furthest.max(root[0].abs());
        }
        assert!(stage.scene.graph_player().expect("playing").has_replacement(), "and it's still waiting");
        assert!(furthest > 0.5, "the character went on swinging: {furthest} m at most");
    }

    /// The mannequin's kind of character: a triangle in `color`, and with `pedestal` one at its feet in a
    /// slot of its own, the size given, which a stage splits off.
    fn mannequin(x: f32, color: [f32; 4], pedestal: Option<f32>) -> Mesh {
        let vertex = |position: [f32; 3]| Vertex {
            position,
            normal: [0.0, 0.0, 1.0],
            outline_normal: [0.0, 0.0, 1.0],
            uv: [0.0, 0.0],
            color: VERTEX_COLOR_NEUTRAL,
            flags: VertexFlags::NONE,
        };
        let swizzles = [Swizzle::IDENTITY; AnimeCharacterMaterial::TEXTURE_SLOTS.len()];
        let material = |color| {
            let document = AnimeCharacterMaterial { base_color: color, ..AnimeCharacterMaterial::default() };
            Material::from_document(&document, &swizzles).expect("a valid material")
        };
        let mut vertices = vec![vertex([x, 0.25, 0.0]), vertex([1.0, 0.25, 0.0]), vertex([0.0, 2.0, 0.0])];
        let mut indices = vec![0, 1, 2];
        let mut submeshes = vec![Submesh { first_index: 0, index_count: 3, material: 0 }];
        let mut slots = vec![MaterialSlot::embedded("skin", material(color))];
        if let Some(size) = pedestal {
            vertices.extend([vertex([-size, 0.0, 0.0]), vertex([size, 0.0, 0.0]), vertex([0.0, 0.25, 0.0])]);
            indices.extend([3, 4, 5]);
            submeshes.push(Submesh { first_index: 3, index_count: 3, material: 1 });
            slots.push(MaterialSlot::embedded("Pedestal", material([0.1, 0.8, 0.2, 1.0])));
        }
        Mesh::from_vertices(&vertices, indices, submeshes, slots, PositionFormat::Unorm16)
    }

    const PALE: [f32; 4] = [0.9, 0.9, 0.9, 1.0];

    /// The character's mesh on a new version, drawn until it has landed, with no steps between.
    fn land_mannequin(stage: &mut Stage, mesh: &Mesh) {
        let generation = stage.content.server.generation(&stage.content.character);
        let bytes = mesh.to_cooked().expect("cooked");
        let pack = write_pack(vec![packed(lookdev::CHARACTER, AssetKind::Mesh, &bytes)]).expect("a pack");
        stage.source.insert(PACK, pack);
        stage.content.remount(PACK).expect("the new pack");
        for _ in 0..10_000 {
            stage.pixels();
            if stage.content.server.generation(&stage.content.character) != generation {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        panic!("it never landed");
    }

    /// `mesh` on a stage of `layout`, framed.
    fn on_a_studio_floor(mesh: &Mesh, layout: StudioLayout) -> Stage {
        let mut stage = Stage::mannequin_of(mesh);
        let meshes = stage.content.studio_meshes(&mut stage.renderer).expect("the stage's meshes");
        stage.scene.spawn_studio(layout, meshes);
        stage.frame_on_character();
        stage
    }

    /// `mesh` in the rim rig, framed.
    fn in_a_rim_rig(mesh: &Mesh) -> Stage {
        let mut stage = Stage::mannequin_of(mesh);
        let meshes = stage.content.rim_rig_meshes(&mut stage.renderer).expect("the rig's meshes");
        stage.scene.spawn_rim_rig(rims::RimLayout::Angles, meshes);
        stage
    }

    /// Whether two frames show the same thing: a fresh renderer and a replaced one have drawn different
    /// numbers of frames, so a code value or two is nothing.
    fn same_picture(a: &[u8], b: &[u8]) -> bool {
        difference(a, b).0 == 0
    }

    /// A stage draws a copy of the character, split from its pedestal when it was made, so a new version
    /// of the character has to reach that as well, with no rig to say so: its geometry and its embedded
    /// material, under the same handles, as a stage made from it would draw.
    #[test]
    #[ignore = "needs a GPU"]
    fn a_new_character_reaches_the_figure_on_a_stage() {
        let first = mannequin(-1.0, PALE, Some(1.0));
        let moved = mannequin(0.5, PALE, Some(1.0));
        let recolored = mannequin(0.5, [0.8, 0.1, 0.1, 1.0], Some(1.0));
        let mut stage = on_a_studio_floor(&first, StudioLayout::Floor);
        let before = stage.pixels();

        land_mannequin(&mut stage, &moved);
        let after = stage.pixels();
        assert!(!same_picture(&before, &after), "its new shape shows");
        assert!(same_picture(&after, &on_a_studio_floor(&moved, StudioLayout::Floor).pixels()));

        land_mannequin(&mut stage, &recolored);
        let colored = stage.pixels();
        assert!(!same_picture(&after, &colored), "its new color shows");
        assert!(same_picture(&colored, &on_a_studio_floor(&recolored, StudioLayout::Floor).pixels()));
    }

    /// The rim rig's figures are a copy of their own.
    #[test]
    #[ignore = "needs a GPU"]
    fn a_new_character_reaches_the_figures_of_a_rim_rig() {
        let first = mannequin(-1.0, PALE, Some(1.0));
        let new = mannequin(0.5, [0.8, 0.1, 0.1, 1.0], Some(1.0));
        let mut stage = in_a_rim_rig(&first);
        let before = stage.pixels();

        land_mannequin(&mut stage, &new);
        let after = stage.pixels();
        assert!(!same_picture(&before, &after), "the new version shows");
        assert!(same_picture(&after, &in_a_rim_rig(&new).pixels()));
    }

    /// A pedestal can come and go with a new version, and a plinth stands where the pedestal's bounds say.
    #[test]
    #[ignore = "needs a GPU"]
    fn a_new_character_brings_its_pedestal_or_leaves_it_out() {
        let with = mannequin(-1.0, PALE, Some(1.0));
        let without = mannequin(-1.0, PALE, None);
        let mut stage = on_a_studio_floor(&with, StudioLayout::Floor);
        let before = stage.pixels();
        land_mannequin(&mut stage, &without);
        let gone = stage.pixels();
        assert!(!same_picture(&before, &gone), "the pedestal's gone");
        assert!(same_picture(&gone, &on_a_studio_floor(&without, StudioLayout::Floor).pixels()));
        land_mannequin(&mut stage, &with);
        assert!(
            same_picture(&stage.pixels(), &on_a_studio_floor(&with, StudioLayout::Floor).pixels()),
            "and it's back"
        );

        let mut stage = on_a_studio_floor(&with, StudioLayout::Plinth);
        let narrow = mannequin(-1.0, PALE, Some(0.25));
        let before = stage.pixels();
        land_mannequin(&mut stage, &narrow);
        let after = stage.pixels();
        assert!(!same_picture(&before, &after), "the plinth takes the pedestal's size");
        assert!(same_picture(&after, &on_a_studio_floor(&narrow, StudioLayout::Plinth).pixels()));
    }

    /// `ids`, each a clip like [`SWING`] for the character, as a pack holds them.
    fn more_swings(ids: &[AssetId]) -> Vec<PackAsset> {
        let clip = swing(HumanBone::REQUIRED.len());
        let bytes = clip.to_cooked().expect("cooked");
        ids.iter().map(|&id| packed_with(id, AssetKind::Clip, &bytes, vec![SKELETON])).collect()
    }

    /// The IDs of a whole graph's worth of clips that aren't [`SWING`].
    fn a_graphs_worth_of_other_clips() -> Vec<AssetId> {
        (0..sapphire_assets::graph::MAX_GRAPH_CLIPS)
            .map(|index| AssetId::from_bytes([0x80 + index as u8; 16]))
            .collect()
    }

    /// A graph naming `ids`, looping the first in its one state.
    fn graph_naming(ids: &[AssetId]) -> AnimGraph {
        let clips: Vec<String> = ids
            .iter()
            .enumerate()
            .map(|(index, id)| format!(r#"(name: "clip {index}", source: "{id}")"#))
            .collect();
        let text = format!(
            r#"(type: "sapphire::assets::AnimGraph", version: 1, data: (
                clips: [{clips}],
                locomotion: (
                    start: "ground",
                    states: [(name: "ground", plays: Clip(clip: "clip 0", looping: true))],
                ),
                combat: (decay: 5, exit: 5, moves: []),
            ))"#,
            clips = clips.join(", "),
        );
        let mut graph = AnimGraph::from_document(&text).expect("a good document");
        graph.resolve(sapphire_assets::graph::clip_id);
        graph
    }

    /// [`SWING`] in two states that take turns as it ends, so a state starts every [`SWING_TICKS`].
    fn turns_graph() -> AnimGraph {
        let text = format!(
            r#"(type: "sapphire::assets::AnimGraph", version: 1, data: (
                clips: [(name: "swing", source: "{SWING}")],
                locomotion: (
                    start: "ground",
                    states: [
                        (name: "ground", plays: Clip(clip: "swing", looping: false)),
                        (name: "again", plays: Clip(clip: "swing", looping: false)),
                    ],
                    transitions: [
                        (from: "ground", to: "again", when: Finished, decay: 1),
                        (from: "again", to: "ground", when: Finished, decay: 1),
                    ],
                ),
                combat: (decay: 5, exit: 5, moves: []),
            ))"#
        );
        let mut graph = AnimGraph::from_document(&text).expect("a good document");
        graph.resolve(sapphire_assets::graph::clip_id);
        graph
    }

    /// A frame of a character still swinging: how far its root stands from the rest pose's x 0, and that
    /// no clip it wanted was missing.
    fn swinging(stage: &mut Stage) -> f32 {
        stage.frame();
        let player = stage.scene.graph_player().expect("playing");
        assert_eq!(player.stats().missing_clips, 0, "it read every clip it wanted");
        player.pose().model()[0].translation[0].abs()
    }

    /// A graph can name a whole table's worth of clips, none of them the old graph's, which goes on until
    /// its next state's start and needs its own. Past that it lets them go.
    #[cfg(feature = "dev")]
    #[test]
    #[ignore = "needs a GPU"]
    fn a_whole_new_graph_leaves_the_old_one_its_clips_until_it_comes_in() {
        let others = a_graphs_worth_of_other_clips();
        let mut assets = pack_assets(0.0, 0, turns_graph(), SWING, 0);
        assets.extend(more_swings(&others));
        let mut stage = Stage::over_pack(write_pack(assets).expect("a pack"));
        stage.play_graph();
        stage.frame();

        let Stage { scene, content, renderer, gpu, .. } = &mut stage;
        scene.replace_graph_document(graph_naming(&others), content, renderer, gpu).expect("it binds");
        assert!(stage.scene.graph_player().expect("playing").has_replacement());

        let mut furthest = 0.0f32;
        let mut frames = 0;
        while stage.scene.graph_player().expect("playing").has_replacement() {
            furthest = furthest.max(swinging(&mut stage));
            frames += 1;
            assert!(frames <= 2 * u32::from(SWING_TICKS), "its clip ends, and the next state starts");
        }
        assert!(furthest > 0.5, "it went on swinging: {furthest} m at most");

        let mut furthest = 0.0f32;
        for _ in 0..SWING_TICKS {
            furthest = furthest.max(swinging(&mut stage));
        }
        assert!(furthest > 0.5, "and the new one plays its own: {furthest} m at most");
        let playing: Vec<AssetId> = stage.scene.graph_player().expect("playing").clips_in_use().collect();
        assert_eq!(playing, others, "the new graph's, and nothing waits");
    }

    /// The same from a pack, which names the new clips before anything has bound them.
    #[test]
    #[ignore = "needs a GPU"]
    fn a_whole_new_graph_from_a_pack_leaves_the_old_one_its_clip() {
        let others = a_graphs_worth_of_other_clips();
        let mut stage = Stage::new();
        stage.play_graph();
        stage.frame();

        let graph = graph_naming(&others);
        let mut assets = pack_assets(0.0, 0, graph.clone(), others[0], 0);
        assets.retain(|asset| asset.id != GRAPH);
        // As a cook's does, it names them all as what it depends on.
        let cooked = graph.to_cooked().expect("cooked");
        assets.push(packed_with(GRAPH, AssetKind::AnimGraph, &cooked, others.clone()));
        assets.extend(more_swings(&others));
        stage.source.insert(PACK, write_pack(assets).expect("a pack"));
        stage.content.remount(PACK).expect("the new pack");
        stage.frames_until(|stage| stage.scene.graph_player().expect("playing").has_replacement());

        let mut furthest = 0.0f32;
        for _ in 0..SWING_TICKS {
            furthest = furthest.max(swinging(&mut stage));
        }
        assert!(stage.scene.graph_player().expect("playing").has_replacement(), "and it's still waiting");
        assert!(furthest > 0.5, "the character went on swinging: {furthest} m at most");
    }

    /// An edit that takes every clip it names and then doesn't bind changes nothing: the graph playing and
    /// the one waiting both still have theirs, and a later edit drops what nothing reads.
    #[cfg(feature = "dev")]
    #[test]
    #[ignore = "needs a GPU"]
    fn a_new_graph_that_wont_bind_leaves_every_graph_its_clips() {
        let others = a_graphs_worth_of_other_clips();
        let mut assets = pack_assets(0.0, 0, swing_graph(5), SWING, 0);
        assets.extend(more_swings(&others));
        let mut stage = Stage::over_pack(write_pack(assets).expect("a pack"));
        stage.play_graph();
        stage.frame();
        let Stage { scene, content, renderer, gpu, .. } = &mut stage;

        let waiting = graph_naming(&[OTHER_SWING]);
        scene.replace_graph_document(waiting, content, renderer, gpu).expect("it binds");

        // The last it names is another skeleton's, which only binding finds.
        let mut named = others.clone();
        *named.last_mut().expect("some") = OTHER_RIGS_SWING;
        let error = scene
            .replace_graph_document(graph_naming(&named), content, renderer, gpu)
            .expect_err("another skeleton's clip");
        assert!(error.contains("clip 63"), "{error}");

        let table = content.graph_clips();
        assert!(table.clip(SWING).is_some(), "the graph playing's");
        assert!(table.clip(OTHER_SWING).is_some(), "the graph waiting's");
        let mut furthest = 0.0f32;
        for _ in 0..SWING_TICKS {
            furthest = furthest.max(swinging(&mut stage));
        }
        assert!(furthest > 0.5, "the character went on swinging: {furthest} m at most");
        assert!(stage.scene.graph_player().expect("playing").has_replacement(), "and the other's waiting");

        // A good edit takes the waiting graph's place, and nothing reads what the bad one named.
        let Stage { scene, content, renderer, gpu, .. } = &mut stage;
        scene.replace_graph_document(graph_naming(&others[..1]), content, renderer, gpu).expect("it binds");
        let table = content.graph_clips();
        assert!(table.clip(SWING).is_some(), "the graph playing's");
        assert!(table.clip(others[0]).is_some(), "the new one's");
        assert!(table.clip(others[1]).is_none(), "nothing reads what the bad edit named");

        // Nor, by the next edit, the graph that one took the place of. However many come before a state
        // starts, what's held is what three graphs name.
        scene.replace_graph_document(graph_naming(&others[1..2]), content, renderer, gpu).expect("it binds");
        let table = content.graph_clips();
        assert!(table.clip(SWING).is_some(), "the graph playing's");
        assert!(table.clip(others[1]).is_some(), "the new one's");
        assert!(table.clip(OTHER_SWING).is_none(), "no one's");
    }

    /// A cook that lands under a save naming a clip nothing has loaded yet: the graph binds when it lands,
    /// which a first load's generation says nothing of.
    #[cfg(feature = "dev")]
    #[test]
    #[ignore = "needs a GPU"]
    fn a_saved_graph_binds_once_a_clip_it_names_has_landed() {
        let mut stage = Stage::new();
        stage.play_graph();
        stage.frame();
        let folder =
            std::env::temp_dir().join(format!("sapphire_sandbox_landing_graph_{}", std::process::id()));
        std::fs::create_dir_all(&folder).expect("a folder");
        let file = folder.join("character.animgraph.ron");
        stage.content.read_graphs_from(vec![(GraphKey::CHARACTER, file.clone())]);

        let text = swing_document(4).replace(&SWING.to_string(), &OTHER_SWING.to_string());
        std::fs::write(&file, text).expect("the save");
        stage.source.insert(PACK, pack_with_decay(0.0, 0, 7));
        stage.content.remount(PACK).expect("the new pack");
        stage.frames_until(|stage| stage.scene.graph_player().expect("playing").has_replacement());
        assert_eq!(stage.content.graph_document().expect("a graph").combat.decay, 4);
        let wanted: Vec<AssetId> = stage.scene.graph_player().expect("playing").clips_in_use().collect();
        assert_eq!(wanted, [SWING, OTHER_SWING], "the graph playing's, and the save's");
        std::fs::remove_dir_all(&folder).expect("cleaned up");
    }

    /// An edited graph over live link (M2 T2): it binds at once and waits for the next state's start like
    /// a new version from a pack, it stays what binds until one comes, and one that can't bind leaves
    /// everything as it was.
    #[cfg(feature = "dev")]
    #[test]
    #[ignore = "needs a GPU"]
    fn an_edited_graph_waits_for_the_next_states_start_until_a_pack_replaces_it() {
        let mut stage = Stage::new();
        stage.play_graph();
        stage.frame();
        let Stage { scene, content, renderer, gpu, .. } = &mut stage;

        let mut missing = swing_graph(4);
        missing.resolve(|_| AssetId::from_bytes([9; 16]));
        let error = scene.replace_graph_document(missing, content, renderer, gpu).expect_err("no such clip");
        assert!(error.contains("swing"), "{error}");
        assert!(!scene.graph_player().expect("playing").has_replacement());

        scene.replace_graph_document(swing_graph(4), content, renderer, gpu).expect("it binds");
        assert!(scene.graph_player().expect("playing").has_replacement());
        assert_eq!(content.graph_document().expect("a graph").combat.decay, 4);
        assert!(scene.graph_player().expect("playing").has_replacement());
        assert!(!content.take_graph_changed(), "its own clips are nothing new");

        stage.source.insert(PACK, pack_with_decay(0.0, 0, 9));
        stage.content.remount(PACK).expect("the new pack");
        for _ in 0..1000 {
            stage.frame();
            if stage.content.graph_document().expect("a graph").combat.decay == 9 {
                return;
            }
        }
        panic!("the cook of the document should win over the edit it came after");
    }

    /// A cook reads a document when it starts, so one that started before the newest save brings what the
    /// file said then, which mustn't undo the save with nothing to say so. Arriving later doesn't make it
    /// newer: the file is the word, and a cook of what it says, or a file that says nothing, leaves the
    /// cook's.
    #[cfg(feature = "dev")]
    #[test]
    #[ignore = "needs a GPU"]
    fn a_cook_older_than_the_newest_save_leaves_the_saved_graph_playing() {
        let mut stage = Stage::new();
        stage.play_graph();
        stage.frame();
        let folder =
            std::env::temp_dir().join(format!("sapphire_sandbox_older_graph_{}", std::process::id()));
        std::fs::create_dir_all(&folder).expect("a folder");
        let file = folder.join("character.animgraph.ron");
        stage.content.read_graphs_from(vec![(GraphKey::CHARACTER, file.clone())]);
        let bound = |stage: &Stage| stage.content.graph_document().expect("a graph").combat.decay;
        let land = |stage: &mut Stage, decay: u32| {
            let versions = stage.content.graph_versions();
            stage.source.insert(PACK, pack_with_decay(0.0, 0, decay));
            stage.content.remount(PACK).expect("the new pack");
            stage.frames_until(|stage| stage.content.graph_versions() != versions);
        };

        // Saved as a cook of an earlier save ran: 4, where it read 7.
        std::fs::write(&file, swing_document(4)).expect("the save");
        let Stage { scene, content, renderer, gpu, .. } = &mut stage;
        scene.replace_graph_document(swing_graph(4), content, renderer, gpu).expect("it binds");
        land(&mut stage, 7);
        assert_eq!(bound(&stage), 4, "the save stands");
        assert!(stage.scene.graph_player().expect("playing").has_replacement(), "and it's what comes next");
        stage.frame();
        assert_eq!(bound(&stage), 4, "from then on");

        // The next cook reads the save, and is no edit.
        land(&mut stage, 4);
        assert_eq!(bound(&stage), 4);

        // A newer save, with no edit taken of it, and a cook of it: what they say is the graph.
        std::fs::write(&file, swing_document(9)).expect("the save");
        land(&mut stage, 9);
        assert_eq!(bound(&stage), 9);

        // A file that doesn't read says nothing against a cook, and nor does none.
        std::fs::write(&file, "half a docu").expect("half a save");
        land(&mut stage, 11);
        assert_eq!(bound(&stage), 11);
        std::fs::remove_dir_all(&folder).expect("cleaned up");
        land(&mut stage, 12);
        assert_eq!(bound(&stage), 12);
    }

    /// A save can read and still not bind, naming a clip nothing has cooked. Live tuning refuses it, and a
    /// cook that lands mustn't take it up in the cook's place: moves fall back on what a cook made, and so
    /// does this, or the graph playing goes on being older than both, with nothing that can bind.
    #[cfg(feature = "dev")]
    #[test]
    #[ignore = "needs a GPU"]
    fn a_saved_graph_that_cant_bind_doesnt_keep_a_cook_out() {
        let mut stage = Stage::new();
        stage.play_graph();
        stage.frame();
        let folder =
            std::env::temp_dir().join(format!("sapphire_sandbox_unbound_graph_{}", std::process::id()));
        std::fs::create_dir_all(&folder).expect("a folder");
        let file = folder.join("character.animgraph.ron");
        stage.content.read_graphs_from(vec![(GraphKey::CHARACTER, file.clone())]);

        let nothing_cooked = AssetId::from_bytes([0x33; 16]);
        let text = swing_document(4).replace(&SWING.to_string(), &nothing_cooked.to_string());
        std::fs::write(&file, text).expect("the save");
        let versions = stage.content.graph_versions();
        stage.source.insert(PACK, pack_with_decay(0.0, 0, 7));
        stage.content.remount(PACK).expect("the new pack");
        stage.frames_until(|stage| stage.content.graph_versions() != versions);
        std::fs::remove_dir_all(&folder).expect("cleaned up");
        assert_eq!(stage.content.graph_document().expect("a graph").combat.decay, 7, "the cook's");
        assert!(stage.scene.graph_player().expect("playing").has_replacement(), "and it's what comes next");
    }

    /// [`swing_document`] with a blink no character here can bind, though it reads and its clip is in the
    /// pack: the test rig has no expressions, and the enemy binds with no rig at all.
    #[cfg(feature = "dev")]
    fn blinking_document(decay: u32) -> String {
        swing_document(decay).replace(
            "combat:",
            r#"blink: Some((expression: "wink", every: (150, 360), close: 3, hold: 1, open: 7)), combat:"#,
        )
    }

    /// [`blinking_document`], as a cook makes it: a cook reads the same file a save writes.
    #[cfg(feature = "dev")]
    fn blinking_graph(decay: u32) -> AnimGraph {
        let mut graph = AnimGraph::from_document(&blinking_document(decay)).expect("a good document");
        graph.resolve(sapphire_assets::graph::clip_id);
        graph
    }

    /// A save can read, name clips the pack holds, and still not fit the character, in a way no landing
    /// changes. Live tuning refuses it, and a cook that lands reads the file again: it mustn't stay in the
    /// cook's place with nothing that can bind.
    #[cfg(feature = "dev")]
    #[test]
    #[ignore = "needs a GPU"]
    fn a_saved_graph_that_never_fits_gives_way_to_the_cook() {
        let other_rigs = swing_document(4).replace(&SWING.to_string(), &OTHER_RIGS_SWING.to_string());
        for (why, save) in
            [("an expression the rig lacks", blinking_document(4)), ("another rig's clip", other_rigs)]
        {
            let mut stage = Stage::new();
            stage.play_graph();
            stage.frame();
            let folder =
                std::env::temp_dir().join(format!("sapphire_sandbox_unfit_graph_{}", std::process::id()));
            std::fs::create_dir_all(&folder).expect("a folder");
            let file = folder.join("character.animgraph.ron");
            stage.content.read_graphs_from(vec![(GraphKey::CHARACTER, file.clone())]);
            std::fs::write(&file, save).expect("the save");

            stage.source.insert(PACK, pack_with_decay(0.0, 0, 7));
            stage.content.remount(PACK).expect("the new pack");
            stage.frames_until(|stage| stage.scene.graph_player().expect("playing").has_replacement());
            std::fs::remove_dir_all(&folder).expect("cleaned up");
            assert_eq!(stage.content.graph_document().expect("a graph").combat.decay, 7, "{why}");
        }
    }

    /// A new character waits for a graph that binds. A save that never will mustn't keep it waiting when
    /// the cook's does, nor have the next save refused as if something were still landing.
    #[cfg(feature = "dev")]
    #[test]
    #[ignore = "needs a GPU"]
    fn a_saved_graph_that_never_fits_doesnt_keep_a_new_character_waiting() {
        let mut stage = Stage::new();
        stage.play_graph();
        stage.frame();
        let joints = stage.joints();
        let folder =
            std::env::temp_dir().join(format!("sapphire_sandbox_unfit_waiting_{}", std::process::id()));
        std::fs::create_dir_all(&folder).expect("a folder");
        let file = folder.join("character.animgraph.ron");
        stage.content.read_graphs_from(vec![(GraphKey::CHARACTER, file.clone())]);
        std::fs::write(&file, blinking_document(4)).expect("the save");

        stage.source.insert(PACK, pack_with_decay(0.0, 1, 7));
        stage.content.remount(PACK).expect("the new pack");
        stage.frames_until(|stage| stage.joints() != joints);
        std::fs::remove_dir_all(&folder).expect("cleaned up");
        assert_eq!(stage.joints(), joints + 1);
        assert_eq!(stage.content.graph_document().expect("a graph").combat.decay, 7, "the cook's");

        let Stage { scene, content, renderer, gpu, .. } = &mut stage;
        scene.replace_graph_document(swing_graph(4), content, renderer, gpu).expect("nothing's landing");
        assert!(scene.graph_player().expect("playing").has_replacement());
    }

    /// A cook reads what was saved, so a graph that never fits can be the cook's too, and then a new
    /// character has nothing to wait for but the correction. That has to reach it.
    #[cfg(feature = "dev")]
    #[test]
    #[ignore = "needs a GPU"]
    fn a_corrected_save_ends_a_new_characters_wait_for_a_graph_that_fits() {
        let mut stage = Stage::new();
        stage.play_graph();
        stage.frame();
        let joints = stage.joints();
        stage.source.insert(PACK, pack_of_graph(0.0, 1, blinking_graph(7), SWING, 0));
        stage.content.remount(PACK).expect("the new pack");
        stage.frames_until(|stage| {
            stage.versions() == (1, 1, 1) && stage.content.graph_document().is_ok_and(|g| g.blink.is_some())
        });
        for _ in 0..30 {
            stage.frame();
        }
        assert_eq!(stage.joints(), joints, "it keeps what it has");

        let Stage { scene, content, renderer, gpu, .. } = &mut stage;
        let error = scene
            .replace_graph_document(blinking_graph(4), content, renderer, gpu)
            .expect_err("that one doesn't fit either");
        assert!(error.contains("wink"), "it says why: {error}");
        assert_eq!(stage.joints(), joints, "it still keeps what it has");

        let Stage { scene, content, renderer, gpu, .. } = &mut stage;
        scene.replace_graph_document(swing_graph(4), content, renderer, gpu).expect("that one does");
        assert_eq!(stage.joints(), joints + 1, "it started over on the new joints");
        assert_eq!(stage.content.graph_document().expect("a graph").combat.decay, 4);
        stage.frame();
        assert_eq!(stage.joints(), joints + 1);
    }

    /// A new character's parts land apart, so a save its clips don't fit yet is only early, and it stays
    /// the word: a save can be newer than what the cook read.
    #[cfg(feature = "dev")]
    #[test]
    #[ignore = "needs a GPU"]
    fn a_saved_graph_outlasts_a_new_character_landing_in_parts() {
        let mut stage = Stage::new();
        stage.play_graph();
        stage.frame();
        let joints = stage.joints();
        let folder =
            std::env::temp_dir().join(format!("sapphire_sandbox_early_graph_{}", std::process::id()));
        std::fs::create_dir_all(&folder).expect("a folder");
        let file = folder.join("character.animgraph.ron");
        stage.content.read_graphs_from(vec![(GraphKey::CHARACTER, file.clone())]);
        std::fs::write(&file, swing_document(4)).expect("the save");

        stage.recook_all_but(&[SWING, OTHER_SWING], 1, 7);
        stage.until((1, 1, 1));
        stage.frames_until(|stage| stage.content.graph_document().is_ok_and(|g| g.combat.decay == 4));
        for _ in 0..30 {
            stage.frame();
        }
        assert_eq!(stage.joints(), joints, "its clips aren't here yet");
        assert_eq!(stage.content.graph_document().expect("a graph").combat.decay, 4, "still the save");

        stage.source.insert(PACK, pack_with_decay(0.0, 1, 7));
        stage.content.remount(PACK).expect("the rest of it");
        stage.frames_until(|stage| stage.joints() != joints);
        std::fs::remove_dir_all(&folder).expect("cleaned up");
        assert_eq!(stage.content.graph_document().expect("a graph").combat.decay, 4, "and it binds");
    }

    /// A fight's player plays a graph, so an edit of it takes as the graph show's does.
    #[cfg(feature = "dev")]
    #[test]
    #[ignore = "needs a GPU"]
    fn a_fighters_graph_takes_an_edit() {
        let mut stage = Stage::new();
        stage.content.load_graph(GRAPH, &mut stage.renderer, &stage.gpu).expect("the graph");
        let driver = lookdev::graph_driver(&stage.content).expect("a driver");
        stage.scene.pose_character_by_graph(driver);
        let Stage { scene, content, renderer, gpu, .. } = &mut stage;
        scene
            .replace_graph_document(swing_graph(4), content, renderer, gpu)
            .expect("a fighter plays a graph");
        assert!(scene.graph_player().expect("playing").has_replacement());
        assert_eq!(content.graph_document().expect("a graph").combat.decay, 4);
    }

    /// A new version of the character under a graph: the graph plays on, on the new joints, posed on the
    /// first frame that draws them.
    #[test]
    #[ignore = "needs a GPU"]
    fn a_new_skeleton_under_a_graph_plays_on() {
        let mut stage = Stage::new();
        stage.play_graph();
        let joints = HumanBone::REQUIRED.len() as u32;
        let per_joint = stage.frame().palette_bytes / joints;
        assert!(per_joint > 0);
        stage.frame();

        // The clip gets the new joint too, and the graph can't bind without it: it may land after the rest.
        stage.recook(0.0, 1);
        let settled = stage.until((1, 1, 1));
        assert_eq!(settled.ignored_poses, 0, "{settled:?}");
        stage.frames_until(|stage| stage.joints() == joints as usize + 1);
        let mut palette = settled.palette_bytes;
        for _ in 0..4 {
            let stats = stage.frame();
            assert_eq!(stats.ignored_poses, 0, "{stats:?}");
            palette = stats.palette_bytes;
        }
        assert_eq!(palette, (joints + 1) * per_joint, "posed on its new palette, every frame");
    }

    /// A new version of the character whose clips land after the rest of it, as a cook's pack can: the
    /// graph can't bind to the new skeleton until they do, so it keeps its pose, and the clips' landing
    /// starts its driver on the new joints rather than handing a binding to the old ones.
    #[test]
    #[ignore = "needs a GPU"]
    fn a_graph_waits_for_clips_that_land_after_the_new_character() {
        let mut stage = Stage::new();
        stage.play_graph();
        let joints = HumanBone::REQUIRED.len();
        let per_joint = stage.frame().palette_bytes as usize / joints;
        assert!(per_joint > 0);
        stage.frame();

        stage.recook_all_but(&[SWING, OTHER_SWING], 1, 5);
        stage.until((1, 1, 1));
        for _ in 0..20 {
            stage.frame();
        }
        assert_eq!(stage.joints(), joints, "it keeps what it has");
        let waiting = stage.scene.graph_player().expect("playing");
        assert!(!waiting.has_replacement(), "nothing bound to the new joints yet");

        stage.recook(0.0, 1);
        stage.frames_until(|stage| stage.joints() != joints);
        assert_eq!(stage.joints(), joints + 1, "the clips landed, and it's on the new joints");
        for _ in 0..4 {
            let stats = stage.frame();
            assert_eq!(stats.ignored_poses, 0, "{stats:?}");
            assert_eq!(stats.palette_bytes as usize, (joints + 1) * per_joint, "and it plays");
        }
    }

    /// A rig and a skeleton with a joint more, then clips for them, with the mesh still to come: the clips
    /// bind to the new skeleton, but the character keeps the pose of the mesh it has, and neither their
    /// landing nor an edit hands that pose a binding to other joints.
    #[test]
    #[ignore = "needs a GPU"]
    fn a_character_waits_for_the_rest_of_a_new_version_to_land() {
        let mut stage = Stage::new();
        stage.play_graph();
        stage.frame();
        let joints = stage.joints();

        stage.recook_all_but(&[lookdev::CHARACTER, SWING, OTHER_SWING], 1, 5);
        stage.until((0, 1, 1));
        let versions = stage.content.graph_versions();
        stage.recook_all_but(&[lookdev::CHARACTER], 1, 5);
        stage.frames_until(|stage| stage.content.graph_versions() != versions);
        assert_eq!(stage.joints(), joints, "it keeps what it has");
        let waiting = stage.scene.graph_player().expect("playing");
        assert!(!waiting.has_replacement(), "a graph bound to other joints waits with it");

        let Stage { scene, content, renderer, gpu, .. } = &mut stage;
        let error = scene
            .replace_graph_document(swing_graph(4), content, renderer, gpu)
            .expect_err("an edit waits with it");
        assert!(error.contains("still landing"), "{error}");
        assert!(!scene.graph_player().expect("playing").has_replacement());

        stage.recook(0.0, 1);
        stage.frames_until(|stage| stage.joints() != joints);
        assert_eq!(stage.joints(), joints + 1, "the rest landed, and so did it");

        // It's over: an edit takes as it did before.
        let Stage { scene, content, renderer, gpu, .. } = &mut stage;
        scene.replace_graph_document(swing_graph(4), content, renderer, gpu).expect("nothing's landing");
        assert!(scene.graph_player().expect("playing").has_replacement());
    }

    /// The pixels of `a` and `b` that differ by more than a little, and their center along x.
    fn difference(a: &[u8], b: &[u8]) -> (usize, f32) {
        let (mut count, mut sum) = (0, 0.0);
        for (index, (a, b)) in a.as_chunks::<4>().0.iter().zip(b.as_chunks::<4>().0).enumerate() {
            if a.iter().zip(b).take(3).any(|(a, b)| a.abs_diff(*b) > 8) {
                count += 1;
                sum += (index % SIDE as usize) as f32;
            }
        }
        (count, if count > 0 { sum / count as f32 } else { 0.0 })
    }

    /// Review RC-1 says why a first frame matters: a renderer starts a new instance in its bind pose,
    /// so a pose that reaches it a frame late shows as a pop to the T-pose. A moved joint moves
    /// pixels, and the first frame of a clip already draws the clip.
    #[test]
    #[ignore = "needs a GPU"]
    fn a_moved_joint_moves_pixels_from_the_first_frame() {
        // The bind pose, held (the triangle hangs from the root, which holding doesn't turn).
        let mut held = Stage::new();
        held.frame_on_character();
        let bind = held.pixels();
        assert_eq!(bind, held.pixels(), "a held pose draws the same");

        // A clip's first frame, with no step before it: 0.75 m to the character's right already.
        let mut stage = Stage::new();
        stage.play_swing();
        let first = stage.pixels();
        let (moved, _) = difference(&bind, &first);
        assert!(moved > 16, "the first frame isn't the bind pose: {moved} pixels differ");

        // Half the swing later, back where the bind pose has it: the clip's position, not the
        // first frame's, and through the skinning that drew it.
        for _ in 0..SWING_TICKS / 2 {
            stage.scene.step();
        }
        let centered = stage.pixels();
        let (off, _) = difference(&bind, &centered);
        assert!(off < moved / 4, "at its bind position by now: {off} pixels off, {moved} before");

        // The swing's far side moved the other way across the target from the first frame.
        for _ in 0..SWING_TICKS / 2 - 1 {
            stage.scene.step();
        }
        let far = stage.pixels();
        let (_, toward_first) = difference(&centered, &first);
        let (_, toward_far) = difference(&centered, &far);
        assert!(difference(&first, &far).0 > 16, "moved");
        assert!(
            (toward_first - toward_far).abs() > 1.0,
            "the two sides differ across the target: {toward_first} and {toward_far}"
        );

        // A step with a stopped clock draws the same and skins nothing.
        stage.scene.world_clock_mut().0.push_scale(sapphire_time::ScaleSource::Debug, 0).expect("room");
        stage.frame();
        let frozen = stage.frame();
        assert_eq!((frozen.skinned, frozen.palette_bytes), (0, 0), "{frozen:?}");
    }

    /// A clip plays on across a new skeleton, with no frame drawing a palette that doesn't fit.
    #[test]
    #[ignore = "needs a GPU"]
    fn a_clip_plays_on_across_a_new_skeleton() {
        let mut stage = Stage::new();
        stage.play_swing();
        let joints = HumanBone::REQUIRED.len() as u32;
        let per_joint = stage.frame().palette_bytes / joints;
        assert!(per_joint > 0);
        assert_eq!(stage.frame().palette_bytes, joints * per_joint, "moving, so sent each frame");

        stage.recook(0.0, 1);
        let settled = stage.until((1, 1, 1));
        assert_eq!(settled.ignored_poses, 0, "{settled:?}");
        // Its clip came with it, so the character goes on moving with its new palette.
        for _ in 0..5 {
            let frame = stage.frame();
            assert_eq!(frame.ignored_poses, 0, "{frame:?}");
            assert_eq!(frame.palette_bytes, (joints + 1) * per_joint, "the new palette: {frame:?}");
        }
    }
}
