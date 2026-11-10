//! The one route every pipeline is built through, and what it counts (M3 K1, exit test 7).
//!
//! A pipeline built while frames draw stalls the frame that wants it: a third of a second cold on an M5,
//! more on a phone. So every `create_render_pipeline` and `create_compute_pipeline` in the renderer goes
//! through [`PipelineBuilds`], which owns the pipeline cache they share and sorts what it builds into three:
//!
//! - **Prewarmed:** built while a renderer is made, by the warm-up ([`Renderer::warm`](crate::Renderer::warm),
//!   [`Renderer::prewarm`](crate::Renderer::prewarm) and its kin, the `warm_up` module), or under a load's
//!   [`Renderer::warm_load`](crate::Renderer::warm_load), when nothing draws.
//! - **Rebuilt:** an edit's new pipelines, built by `shader_builds`' job from what was already built, off the
//!   frame thread or with the app asking for that stall.
//! - **First use:** every other one, wherever it comes from: an upload of a mesh whose key nothing prewarmed,
//!   a post variant, the sky's, the interface's, a pipeline a probe grid's ambient constants asked for again, if nothing declared the grid.
//!   Exit test 7 asks for none of these after a load.
//!
//! And apart from why, where (M4 H1): a build on a warm-up thread (`sapphire_jobs::spawn_warm_up`) stalls no
//! frame, and any other, on the frame's thread or on a worker it waits on, stalls the frame that built it. Once a
//! renderer has prepared its first frame, every build of the second kind that isn't an edit's rebuild is a hitch
//! ([`PipelineStats::frame_thread_builds`]), named in the report as a first use is, and a bench fails on one as it
//! does on a first use (exit test 1).
//!
//! The count can't be escaped by a new owner: a test scans the workspace's sources for any other route to a
//! pipeline (`every_pipeline_goes_through_the_route`), and the GPU tests compare the ledger with what wgpu says
//! it holds, so a build on some path the scan can't read shows too.
//!
//! The ledger is one renderer's. A new tier's or a recovered device's renderer starts its own.

use std::cell::Cell;
use std::collections::BTreeSet;
use std::fmt;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use sapphire_assets::PipelineKey;
use sapphire_rhi::wgpu;

use crate::LOG_TARGET;
use crate::materials::MaterialHandle;
use crate::post::PostVariants;

/// Who a pipeline is for: what a first-use build is named by, and a family of its own for each thing an app
/// or a pack can declare. A pack's meshes draw through [`Self::Mesh`], whichever pass they draw in: surfaces,
/// outlines, and the depth prepasses' pipelines.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PipelineFamily {
    Mesh,
    Shadow,
    Skinning,
    Post,
    Effects,
    Sky,
    Ui,
    /// An inset's resolve (M3 U6): its meshes draw through [`Self::Mesh`].
    Inset,
    Ambient,
    Temporal,
    Lights,
    Debug,
}

impl PipelineFamily {
    /// The name used in messages.
    pub const fn name(self) -> &'static str {
        match self {
            Self::Mesh => "mesh",
            Self::Shadow => "shadow",
            Self::Skinning => "skinning",
            Self::Post => "post",
            Self::Effects => "effects",
            Self::Sky => "sky",
            Self::Ui => "ui",
            Self::Inset => "inset",
            Self::Ambient => "ambient",
            Self::Temporal => "temporal",
            Self::Lights => "lights",
            Self::Debug => "debug",
        }
    }
}

impl fmt::Display for PipelineFamily {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

/// What a renderer has built, by why, and what stalled a frame (rule R9,
/// [`RenderStats::pipelines`](crate::RenderStats::pipelines); the module docs say what each means). Exit test 7's
/// budget for [`Self::first_use_builds`] after a load is 0, and exit test 1's for [`Self::hitches`] after the first
/// frame.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PipelineStats {
    pub prewarmed: u32,
    pub first_use_builds: u32,
    pub rebuilt: u32,
    /// Of all of them, those the warm-up's threads built, which no frame waited on.
    pub warmed_off_frame: u32,
    /// Builds after the renderer's first frame on the frame's thread, or on a worker it waited on: whatever they
    /// were, prewarmed or first use, each stalled a frame. An edit's rebuild isn't counted.
    pub frame_thread_builds: u32,
    /// Times the frame's thread waited for the warm-up after its first frame
    /// ([`Renderer::prewarm`](crate::Renderer::prewarm) while one builds).
    pub frame_waits: u32,
}

impl PipelineStats {
    /// Every pipeline it has built, whichever it replaced.
    pub const fn built(self) -> u32 {
        self.prewarmed + self.first_use_builds + self.rebuilt
    }

    /// The frames that stalled on a pipeline after the first: a build on the frame's thread, or a wait for one.
    pub const fn hitches(self) -> u32 {
        self.frame_thread_builds + self.frame_waits
    }
}

/// One pipeline built on first use: who for, and which, by its label and the constants that make it what it is.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FirstUseBuild {
    pub family: PipelineFamily,
    pub pipeline: String,
}

impl fmt::Display for FirstUseBuild {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.family, self.pipeline)
    }
}

/// Every first-use build a renderer has made, and the mesh keys they were for, which no prewarm had listed; and
/// every build a frame stalled on after the first ([`PipelineStats::frame_thread_builds`]).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FirstUseReport {
    pub builds: Vec<FirstUseBuild>,
    pub keys: Vec<PipelineKey>,
    /// Built on the frame's thread, or a worker it waited on, after the first frame: first uses among them.
    pub on_frame: Vec<FirstUseBuild>,
}

impl fmt::Display for FirstUseReport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} first-use pipeline builds", self.builds.len())?;
        if !self.keys.is_empty() {
            f.write_str(", for mesh keys no inventory listed:")?;
            for key in &self.keys {
                write!(f, "\n  mesh {key}")?;
            }
        }
        for build in &self.builds {
            write!(f, "\n  {build}")?;
        }
        if !self.on_frame.is_empty() {
            write!(f, "\n{} built on a frame's thread after the first frame:", self.on_frame.len())?;
            for build in &self.on_frame {
                write!(f, "\n  {build}")?;
            }
        }
        Ok(())
    }
}

/// What a load declares its frames may draw through, for [`Renderer::prewarm`](crate::Renderer::prewarm). The
/// packs' tables say what their meshes do; the rest no table can say, so the app declares it. A renderer's
/// settings carry the rest: the shadow casters come with a key's format, and its passes come with the renderer.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PipelineInventory {
    /// What meshes draw through ([`sapphire_assets::AssetServer::pipeline_keys`]), with any of the app's
    /// own making.
    pub meshes: BTreeSet<PipelineKey>,
    /// The post variants its screen effects may switch on, beyond the settings'.
    pub post: PostVariants,
    /// It plays particle effects.
    pub effects: bool,
    /// It deforms meshes, with skinning or morphs.
    pub skinning: bool,
    /// It draws an interface: atlas quads and glyph pages go through the one pipeline this builds.
    pub ui: bool,
    /// It shows 3D insets (M3 U6): their resolve. Their characters draw through `meshes`' keys.
    pub insets: bool,
    /// The sky materials it draws, uploaded by the time they're listed.
    pub sky: Vec<MaterialHandle>,
    /// A probe grid arrives after this (a zone's, M3 C1): the ambient pass's pipelines, and the environment
    /// and foliage surfaces with the grid's constants on, so that its arrival builds nothing.
    pub probe_grid: bool,
    /// Its characters may fade (M1 C7, `--fade` or the proximity fade): the alpha-tested variants of the anime
    /// character surfaces built by then and of [`Self::meshes`]' own, with their outlines, eyes through hair and
    /// the prepasses' masked pipelines, which a fading character's opaque draws move to.
    pub fades: bool,
    /// It draws debug lines (M2 T1): their pass's pipeline, and the scene prepass's, whose depth it reads.
    pub debug_draw: bool,
}

impl PipelineInventory {
    /// The name of each entry: `mesh` and a key, `post`, `effects`, `skinning`, `ui`, `inset`, `sky` and `ambient`
    /// as a miss names them, and `fades` and `debug`.
    pub fn entries(&self) -> Vec<String> {
        let mut entries: Vec<String> = self.meshes.iter().map(|key| format!("mesh {key}")).collect();
        let others = [
            (PipelineFamily::Post.name(), self.post != PostVariants::NONE),
            (PipelineFamily::Effects.name(), self.effects),
            (PipelineFamily::Skinning.name(), self.skinning),
            (PipelineFamily::Ui.name(), self.ui),
            (PipelineFamily::Inset.name(), self.insets),
            (PipelineFamily::Sky.name(), !self.sky.is_empty()),
            (PipelineFamily::Ambient.name(), self.probe_grid),
            ("fades", self.fades),
            (PipelineFamily::Debug.name(), self.debug_draw),
        ];
        entries.extend(others.into_iter().filter(|&(_, listed)| listed).map(|(name, _)| name.to_owned()));
        entries
    }

    /// Takes out the entry called `name` ([`Self::entries`]), saying whether it was there: what proves a miss
    /// fails.
    pub fn remove(&mut self, name: &str) -> bool {
        let listed = self.entries().iter().any(|entry| entry == name);
        match name {
            "post" => self.post = PostVariants::NONE,
            "effects" => self.effects = false,
            "skinning" => self.skinning = false,
            "ui" => self.ui = false,
            "inset" => self.insets = false,
            "sky" => self.sky.clear(),
            "ambient" => self.probe_grid = false,
            "fades" => self.fades = false,
            "debug" => self.debug_draw = false,
            _ => self.meshes.retain(|key| format!("mesh {key}") != name),
        }
        listed
    }

    /// Adds what `other` lists: a warm-up asked for while another builds waits with every request since
    /// (`warm_up`), as a prewarm only adds.
    pub fn add(&mut self, other: &Self) {
        let Self { meshes, post, effects, skinning, ui, insets, sky, probe_grid, fades, debug_draw } = other;
        self.meshes.extend(meshes.iter().copied());
        self.post |= *post;
        self.effects |= effects;
        self.skinning |= skinning;
        self.ui |= ui;
        self.insets |= insets;
        for material in sky {
            if !self.sky.contains(material) {
                self.sky.push(*material);
            }
        }
        self.probe_grid |= probe_grid;
        self.fades |= fades;
        self.debug_draw |= debug_draw;
    }
}

#[derive(Default)]
struct Ledger {
    /// Open prewarms. While there are any, what's built is prewarmed, on whichever thread builds it.
    warming: AtomicU32,
    prewarmed: AtomicU32,
    first_use_builds: AtomicU32,
    rebuilt: AtomicU32,
    warmed_off_frame: AtomicU32,
    frame_thread_builds: AtomicU32,
    frame_waits: AtomicU32,
    /// Frames the renderer has started preparing, up to 2: from the second on, a build a frame waits on is a
    /// hitch.
    frames: AtomicU32,
    report: Mutex<FirstUseReport>,
}

thread_local! {
    /// Whether this thread is running a shader build's steps (`shader_builds`), which build one after another on
    /// one thread, inside its error scopes.
    static REBUILDING: Cell<bool> = const { Cell::new(false) };
}

/// While this lives, what its thread builds is an edit's rebuild rather than a first use.
pub(crate) struct Rebuilding(());

impl Rebuilding {
    pub(crate) fn begin() -> Self {
        REBUILDING.set(true);
        Self(())
    }
}

impl Drop for Rebuilding {
    fn drop(&mut self) {
        REBUILDING.set(false);
    }
}

/// While this lives, what's built is prewarmed, on whichever thread builds it.
#[must_use = "it counts builds as prewarmed only while it lives"]
pub struct PipelineWarmup(Arc<Ledger>);

impl fmt::Debug for PipelineWarmup {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("PipelineWarmup")
    }
}

impl Drop for PipelineWarmup {
    fn drop(&mut self) {
        self.0.warming.fetch_sub(1, Ordering::Relaxed);
    }
}

/// The route to a pipeline. See the [module docs](self). Clones share one ledger and one cache.
#[derive(Clone)]
pub(crate) struct PipelineBuilds {
    cache: Option<wgpu::PipelineCache>,
    ledger: Arc<Ledger>,
}

impl fmt::Debug for PipelineBuilds {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PipelineBuilds").field("stats", &self.stats()).finish_non_exhaustive()
    }
}

impl PipelineBuilds {
    pub(crate) fn new(cache: Option<&wgpu::PipelineCache>) -> Self {
        Self { cache: cache.cloned(), ledger: Arc::default() }
    }

    /// The pipeline cache, for the descriptor's `cache`.
    pub(crate) fn cache(&self) -> Option<&wgpu::PipelineCache> {
        self.cache.as_ref()
    }

    /// Counts what's built as prewarmed until what it returns drops.
    pub(crate) fn warming(&self) -> PipelineWarmup {
        self.ledger.warming.fetch_add(1, Ordering::Relaxed);
        PipelineWarmup(self.ledger.clone())
    }

    /// Builds a render pipeline for `family`.
    pub(crate) fn render(
        &self,
        device: &wgpu::Device,
        family: PipelineFamily,
        desc: &wgpu::RenderPipelineDescriptor<'_>,
    ) -> wgpu::RenderPipeline {
        self.count(family, || {
            let fragment =
                desc.fragment.as_ref().map_or(&[][..], |state| state.compilation_options.constants);
            // A mesh's positions are the first per-vertex buffer's first attribute: its draw data is per instance.
            let per_vertex =
                |buffer: &&wgpu::VertexBufferLayout<'_>| buffer.step_mode == wgpu::VertexStepMode::Vertex;
            let positions = desc.vertex.buffers.iter().flatten().find(per_vertex);
            let format =
                positions.and_then(|buffer| buffer.attributes.first()).map(|attribute| attribute.format);
            name_of(desc.label, [desc.vertex.compilation_options.constants, fragment], format)
        });
        device.create_render_pipeline(desc)
    }

    /// Builds a compute pipeline for `family`.
    pub(crate) fn compute(
        &self,
        device: &wgpu::Device,
        family: PipelineFamily,
        desc: &wgpu::ComputePipelineDescriptor<'_>,
    ) -> wgpu::ComputePipeline {
        self.count(family, || name_of(desc.label, [desc.compilation_options.constants, &[]], None));
        device.create_compute_pipeline(desc)
    }

    fn count(&self, family: PipelineFamily, name: impl FnOnce() -> String) {
        let ledger = &self.ledger;
        // On the warm-up's threads, no frame waits.
        let off_frame = sapphire_jobs::on_warm_up_thread();
        let rebuilding = REBUILDING.get();
        let mut name = Some(name);
        let mut named = None;
        if rebuilding {
            ledger.rebuilt.fetch_add(1, Ordering::Relaxed);
        } else if off_frame || ledger.warming.load(Ordering::Relaxed) > 0 {
            ledger.prewarmed.fetch_add(1, Ordering::Relaxed);
        } else {
            ledger.first_use_builds.fetch_add(1, Ordering::Relaxed);
            let pipeline = name.take().expect("named once")();
            tracing::warn!(
                target: LOG_TARGET,
                %family,
                %pipeline,
                "a pipeline built on first use, stalling what wanted it: list it in the load's inventory"
            );
            named = Some(pipeline.clone());
            self.report().builds.push(FirstUseBuild { family, pipeline });
        }
        if off_frame {
            ledger.warmed_off_frame.fetch_add(1, Ordering::Relaxed);
        } else if !rebuilding && ledger.frames.load(Ordering::Relaxed) > 1 {
            ledger.frame_thread_builds.fetch_add(1, Ordering::Relaxed);
            let pipeline = named.or_else(|| name.map(|name| name())).expect("named once");
            tracing::warn!(
                target: LOG_TARGET,
                %family,
                %pipeline,
                "a pipeline built on a frame's thread after the first frame: warm it up off the frame"
            );
            self.report().on_frame.push(FirstUseBuild { family, pipeline });
        }
    }

    /// The renderer starts preparing a frame: from the second on, a build a frame waits on is a hitch.
    pub(crate) fn frame_started(&self) {
        let frames = &self.ledger.frames;
        if frames.load(Ordering::Relaxed) < 2 {
            frames.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// The frame's thread is about to wait for the warm-up: a hitch past the first frame.
    pub(crate) fn frame_waits(&self) {
        if self.ledger.frames.load(Ordering::Relaxed) > 1 {
            self.ledger.frame_waits.fetch_add(1, Ordering::Relaxed);
            tracing::warn!(target: LOG_TARGET, "a frame waited for the pipeline warm-up after the first frame");
        }
    }

    /// Says `keys` drew first-use builds with no inventory having listed them, once each.
    pub(crate) fn unlisted(&self, keys: impl IntoIterator<Item = PipelineKey>) {
        let mut report = self.report();
        for key in keys {
            if !report.keys.contains(&key) {
                report.keys.push(key);
            }
        }
    }

    pub(crate) fn stats(&self) -> PipelineStats {
        let ledger = &self.ledger;
        PipelineStats {
            prewarmed: ledger.prewarmed.load(Ordering::Relaxed),
            first_use_builds: ledger.first_use_builds.load(Ordering::Relaxed),
            rebuilt: ledger.rebuilt.load(Ordering::Relaxed),
            warmed_off_frame: ledger.warmed_off_frame.load(Ordering::Relaxed),
            frame_thread_builds: ledger.frame_thread_builds.load(Ordering::Relaxed),
            frame_waits: ledger.frame_waits.load(Ordering::Relaxed),
        }
    }

    pub(crate) fn first_use_report(&self) -> FirstUseReport {
        self.report().clone()
    }

    fn report(&self) -> std::sync::MutexGuard<'_, FirstUseReport> {
        self.ledger.report.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// A pipeline's name in a report: its label, the constants it sets, and the format of its first per-vertex
/// attribute, which is a mesh pipeline's position format.
fn name_of(
    label: Option<&str>,
    constants: [&[(&str, f64)]; 2],
    format: Option<wgpu::VertexFormat>,
) -> String {
    let mut name = label.unwrap_or("unlabeled").to_owned();
    let mut seen: Vec<(&str, f64)> = Vec::new();
    for &(constant, value) in constants.into_iter().flatten() {
        if value != 0.0 && !seen.iter().any(|&(other, _)| other == constant) {
            seen.push((constant, value));
        }
    }
    for (constant, value) in seen {
        name.push_str(&format!(" {constant}={value}"));
    }
    if let Some(format) = format {
        name.push_str(&format!(" {format:?}"));
    }
    name
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::{Path, PathBuf};

    use super::*;

    /// Test code that makes a pipeline of its own, none of which a renderer ever draws through.
    const OTHER_ROUTES: [&str; 5] = [
        // The route itself.
        "crates/sapphire_render/src/pipeline_builds.rs",
        // Runs a shader's own functions on values only the GPU sees: `#[cfg(test)]`, with no renderer.
        "crates/sapphire_render/src/shaders.rs",
        // Draws the sky's shader into a target of its own, with no renderer.
        "crates/sapphire_render/src/sky/gpu_tests.rs",
        // Samples textures to prove they uploaded, with no renderer.
        "crates/sapphire_render/tests/gpu/texture.rs",
        // Draws a ramp into a texture for a pane to show, with no renderer.
        "tools/sapphire_editor/tests/viewport.rs",
    ];

    fn rust_files(dir: &Path, files: &mut Vec<PathBuf>) {
        for entry in fs::read_dir(dir).expect("a source directory") {
            let path = entry.expect("an entry").path();
            if path.is_dir() {
                rust_files(&path, files);
            } else if path.extension().is_some_and(|extension| extension == "rs") {
                files.push(path);
            }
        }
    }

    /// Fails when a new route to a pipeline appears anywhere in the workspace: it would build past the count.
    /// Give it a family and go through [`PipelineBuilds`] (the module docs).
    #[test]
    fn every_pipeline_goes_through_the_route() {
        let workspace = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let mut files = Vec::new();
        for dir in ["apps", "crates", "tools", "xtask"] {
            rust_files(&workspace.join(dir), &mut files);
        }
        assert!(files.len() > 500, "the scan reads the workspace: {} files", files.len());
        let mut routes = Vec::new();
        for path in files {
            let relative = path.strip_prefix(&workspace).expect("under the workspace");
            let relative = relative.to_str().expect("a UTF-8 path").replace('\\', "/");
            let text = fs::read_to_string(&path).expect("a source file");
            // Spelled apart, so this file doesn't find itself saying them.
            for method in [concat!("create_render", "_pipeline"), concat!("create_compute", "_pipeline")] {
                if text.contains(method) {
                    routes.push(relative.clone());
                }
            }
        }
        routes.retain(|route| !OTHER_ROUTES.contains(&route.as_str()));
        assert_eq!(routes, Vec::<String>::new(), "these build pipelines past `PipelineBuilds`");
    }

    #[test]
    fn a_name_says_the_label_the_constants_on_and_the_format() {
        let name = name_of(
            Some("surface"),
            [&[("A", 1.0), ("B", 0.0)], &[("A", 1.0), ("C", 2.0)]],
            Some(wgpu::VertexFormat::Float32x3),
        );
        assert_eq!(name, "surface A=1 C=2 Float32x3");
        assert_eq!(name_of(None, [&[], &[]], None), "unlabeled");
    }

    #[test]
    fn a_rebuild_on_its_thread_isnt_a_first_use_on_another() {
        let builds = PipelineBuilds::new(None);
        {
            let _rebuilding = Rebuilding::begin();
            builds.count(PipelineFamily::Post, || panic!("a rebuild needs no name"));
            std::thread::scope(|scope| {
                scope.spawn(|| builds.count(PipelineFamily::Sky, || "sky".to_owned()));
            });
        }
        let stats = builds.stats();
        assert_eq!((stats.rebuilt, stats.first_use_builds), (1, 1));
        assert_eq!(
            builds.first_use_report().builds,
            [FirstUseBuild { family: PipelineFamily::Sky, pipeline: "sky".to_owned() }]
        );
    }

    #[test]
    fn what_a_prewarm_builds_on_any_thread_is_prewarmed_and_nothing_after() {
        let builds = PipelineBuilds::new(None);
        let warming = builds.warming();
        let nested = builds.warming();
        drop(nested);
        builds.count(PipelineFamily::Post, || panic!("no name"));
        std::thread::scope(|scope| {
            scope.spawn(|| builds.count(PipelineFamily::Post, || panic!("no name")));
        });
        drop(warming);
        assert_eq!(builds.stats(), PipelineStats { prewarmed: 2, ..PipelineStats::default() });
        builds.count(PipelineFamily::Ui, || "ui".to_owned());
        assert_eq!(
            builds.stats(),
            PipelineStats { prewarmed: 2, first_use_builds: 1, ..PipelineStats::default() }
        );
        assert_eq!(builds.stats().built(), 3);
    }

    /// Error scopes are per thread in wgpu 30, so a build on the job pool scopes itself on the thread that runs it.
    /// What wgpu refuses then reaches that build's scope alone, whichever thread the pool gave it (the waiting thread
    /// runs some), and neither the waiting thread's own scope nor the device's uncaptured handler sees it. M4's
    /// warm-up builds on this (H0).
    #[test]
    #[ignore = "needs a GPU"]
    fn a_build_refused_on_the_pool_reaches_only_its_own_scope() {
        use sapphire_rhi::{GpuContext, GpuContextDesc, GpuInstance, GpuInstanceDesc};

        use crate::shader_builds::scope_error;

        /// The one build whose fragment entry point doesn't exist.
        const REFUSED: usize = 5;
        let instance = GpuInstance::new(&GpuInstanceDesc::default()).expect("instance");
        let gpu = GpuContext::new(&instance, &GpuContextDesc::default(), None).expect("device");
        let device = gpu.device().clone();
        let uncaptured = Arc::new(AtomicU32::new(0));
        let counted = uncaptured.clone();
        device.on_uncaptured_error(Arc::new(move |_| {
            counted.fetch_add(1, Ordering::Relaxed);
        }));
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: None,
            source: wgpu::ShaderSource::Wgsl(
                "@vertex fn vs() -> @builtin(position) vec4<f32> { return vec4<f32>(0.0); }
                 @fragment fn fs() -> @location(0) vec4<f32> { return vec4<f32>(1.0); }"
                    .into(),
            ),
        });
        let builds = PipelineBuilds::new(None);
        let _warming = builds.warming();
        let waiting = device.push_error_scope(wgpu::ErrorFilter::Validation);
        let caught = sapphire_jobs::scope(|scope| {
            for build in 0..16 {
                let (device, module, builds) = (device.clone(), module.clone(), builds.clone());
                scope.spawn(move || {
                    let fragment = if build == REFUSED { "missing" } else { "fs" };
                    let validation = device.push_error_scope(wgpu::ErrorFilter::Validation);
                    let internal = device.push_error_scope(wgpu::ErrorFilter::Internal);
                    builds.render(
                        &device,
                        PipelineFamily::Debug,
                        &wgpu::RenderPipelineDescriptor {
                            label: None,
                            layout: None,
                            vertex: wgpu::VertexState {
                                module: &module,
                                entry_point: Some("vs"),
                                compilation_options: Default::default(),
                                buffers: &[],
                            },
                            primitive: Default::default(),
                            depth_stencil: None,
                            multisample: Default::default(),
                            fragment: Some(wgpu::FragmentState {
                                module: &module,
                                entry_point: Some(fragment),
                                compilation_options: Default::default(),
                                targets: &[Some(wgpu::TextureFormat::Rgba8Unorm.into())],
                            }),
                            multiview_mask: None,
                            cache: None,
                        },
                    );
                    let internal = scope_error(internal.pop());
                    let validation = scope_error(validation.pop());
                    internal.or(validation).is_some()
                });
            }
        });
        assert!(
            scope_error(waiting.pop()).is_none(),
            "the waiting thread's own scope sees no worker's refusal"
        );
        let refused: Vec<usize> =
            caught.iter().enumerate().filter(|&(_, &refused)| refused).map(|(build, _)| build).collect();
        assert_eq!(refused, [REFUSED], "only the refused build's scope caught it");
        assert_eq!(uncaptured.load(Ordering::Relaxed), 0, "nothing reached the uncaptured handler");
        assert_eq!(builds.stats().prewarmed, 16);
    }

    #[test]
    fn past_the_first_frame_a_build_off_the_warm_up_is_a_hitch_and_one_on_it_isnt() {
        let builds = PipelineBuilds::new(None);
        let warming = builds.warming();
        builds.frame_started();
        builds.count(PipelineFamily::Post, || panic!("the first frame's builds are no hitch"));
        builds.frame_waits();
        builds.frame_started();
        builds.count(PipelineFamily::Sky, || "sky".to_owned());
        let (sender, receiver) = std::sync::mpsc::channel();
        let off = builds.clone();
        sapphire_jobs::spawn_warm_up(move || {
            off.count(PipelineFamily::Mesh, || panic!("a warm-up's build is no hitch"));
            sender.send(()).expect("the test waits");
        });
        receiver.recv().expect("the warm-up ran");
        {
            let _rebuilding = Rebuilding::begin();
            builds.count(PipelineFamily::Post, || panic!("an edit's rebuild is no hitch"));
        }
        builds.frame_waits();
        drop(warming);
        let stats = builds.stats();
        assert_eq!((stats.prewarmed, stats.rebuilt, stats.first_use_builds), (3, 1, 0));
        assert_eq!((stats.warmed_off_frame, stats.frame_thread_builds, stats.frame_waits), (1, 1, 1));
        assert_eq!(stats.hitches(), 2);
        let report = builds.first_use_report();
        assert_eq!(
            report.on_frame,
            [FirstUseBuild { family: PipelineFamily::Sky, pipeline: "sky".to_owned() }]
        );
        assert!(
            report.to_string().contains("1 built on a frame's thread after the first frame:\n  sky: sky")
        );
    }

    #[test]
    fn an_unlisted_key_is_named_once() {
        let builds = PipelineBuilds::new(None);
        let key = PipelineKey {
            family: sapphire_assets::material::MaterialFamily::Environment,
            flags: sapphire_assets::material::MaterialFlags::empty(),
            format: sapphire_assets::mesh::PositionFormat::Unorm16,
            reflection: false,
        };
        builds.unlisted([key, key]);
        builds.unlisted([key]);
        assert_eq!(builds.first_use_report().keys, [key]);
        assert!(builds.first_use_report().to_string().contains("mesh environment"));
    }
}
