//! The one route every pipeline is built through, and what it counts (M3 K1, exit test 7).
//!
//! A pipeline built while frames draw stalls the frame that wants it: a third of a second cold on an M5,
//! more on a phone. So every `create_render_pipeline` and `create_compute_pipeline` in the renderer goes
//! through [`PipelineBuilds`], which owns the pipeline cache they share and sorts what it builds into three:
//!
//! - **Prewarmed:** built while a renderer is made, or inside a prewarm
//!   ([`Renderer::prewarm`](crate::Renderer::prewarm) and its kin), or under a load's
//!   [`Renderer::warm_load`](crate::Renderer::warm_load), when nothing draws.
//! - **Rebuilt:** an edit's new pipelines, built by `shader_builds`' job from what was already built, off the
//!   frame thread or with the app asking for that stall.
//! - **First use:** every other one, wherever it comes from: an upload of a mesh whose key nothing prewarmed,
//!   a post variant, the sky's, the interface's, a pipeline a probe grid's ambient constants asked for again, if nothing declared the grid.
//!   Exit test 7 asks for none of these after a load.
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

/// What a renderer has built, by why (rule R9, [`RenderStats::pipelines`](crate::RenderStats::pipelines); the
/// module docs say what each means). Exit test 7's budget for [`Self::first_use_builds`] after a load is 0.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PipelineStats {
    pub prewarmed: u32,
    pub first_use_builds: u32,
    pub rebuilt: u32,
}

impl PipelineStats {
    /// Every pipeline it has built, whichever it replaced.
    pub const fn built(self) -> u32 {
        self.prewarmed + self.first_use_builds + self.rebuilt
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

/// Every first-use build a renderer has made, and the mesh keys they were for, which no prewarm had listed.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FirstUseReport {
    pub builds: Vec<FirstUseBuild>,
    pub keys: Vec<PipelineKey>,
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
}

#[derive(Default)]
struct Ledger {
    /// Open prewarms. While there are any, what's built is prewarmed, on whichever thread builds it.
    warming: AtomicU32,
    prewarmed: AtomicU32,
    first_use_builds: AtomicU32,
    rebuilt: AtomicU32,
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
        if REBUILDING.get() {
            ledger.rebuilt.fetch_add(1, Ordering::Relaxed);
        } else if ledger.warming.load(Ordering::Relaxed) > 0 {
            ledger.prewarmed.fetch_add(1, Ordering::Relaxed);
        } else {
            ledger.first_use_builds.fetch_add(1, Ordering::Relaxed);
            let pipeline = name();
            tracing::warn!(
                target: LOG_TARGET,
                %family,
                %pipeline,
                "a pipeline built on first use, stalling what wanted it: list it in the load's inventory"
            );
            self.report().builds.push(FirstUseBuild { family, pipeline });
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
        assert_eq!(builds.stats(), PipelineStats { prewarmed: 2, first_use_builds: 0, rebuilt: 0 });
        builds.count(PipelineFamily::Ui, || "ui".to_owned());
        assert_eq!(builds.stats(), PipelineStats { prewarmed: 2, first_use_builds: 1, rebuilt: 0 });
        assert_eq!(builds.stats().built(), 3);
    }

    #[test]
    fn an_unlisted_key_is_named_once() {
        let builds = PipelineBuilds::new(None);
        let key = PipelineKey {
            family: sapphire_assets::material::MaterialFamily::Environment,
            flags: sapphire_assets::material::MaterialFlags::empty(),
            format: sapphire_assets::mesh::PositionFormat::Unorm16,
        };
        builds.unlisted([key, key]);
        builds.unlisted([key]);
        assert_eq!(builds.first_use_report().keys, [key]);
        assert!(builds.first_use_report().to_string().contains("mesh environment"));
    }
}
