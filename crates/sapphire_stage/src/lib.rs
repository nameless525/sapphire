//! A stage: a GPU, cooked content and a scene, stepped and drawn. It's what turns a simulation and a
//! pack into a frame the renderer can draw, so an app, a game and the editor's viewport share one.
//!
//! It owns [`Scene`] (the ECS world, the fixed-step systems and what extracts a `FramePacket`), [`Content`]
//! (the asset server and what it uploaded) and [`Stage`], which holds them with a renderer on one device.
//! It owns no window, no command line, no main loop and no dev panel: those are the app's. What a frame draws
//! into is the caller's too.
//!
//! # A frame
//!
//! ```no_run
//! use sapphire_platform::{AppInitError, Platform};
//! use sapphire_render::quality::QualitySettings;
//! use sapphire_rhi::wgpu;
//! use sapphire_stage::{Readback, Stage, StageDesc, headless_gpu};
//!
//! fn one_frame(platform: &Platform) -> Result<Vec<u8>, AppInitError> {
//!     const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8UnormSrgb;
//!     let gpu = headless_gpu()?;
//!     let desc = StageDesc::new(platform.host(), &gpu, FORMAT, QualitySettings::default());
//!     let mut stage = Stage::new(platform.host(), gpu, desc)?;
//!     for _ in 0..60 {
//!         stage.step();
//!     }
//!     let target = stage.target(640, 360, wgpu::TextureUsages::COPY_SRC);
//!     let view = target.create_view(&wgpu::TextureViewDescriptor::default());
//!     let readback = Readback::new(&stage.gpu, 640, 360);
//!     stage.render(&view, 1.0, |graph, target| readback.record(graph, target))?;
//!     stage.check_gpu()?;
//!     readback.read(&stage.gpu).map_err(AppInitError::msg)
//! }
//! ```
//!
//! **A window:** make the instance, the surface and `GpuContext::new(&instance, &desc, Some(&surface))`
//! yourself, give [`StageDesc`] the surface's format, and pass [`Stage::render`] the acquired image's view.
//! Nothing here reads a clock or asks for a surface.
//!
//! **With no loop:** a tool that isn't on the platform's loop names its content with
//! [`Host::with_content_at`](sapphire_platform::Host::with_content_at), and can share its device: a stage takes an
//! `Arc<GpuContext>` as well as a context, and drawing into a view of any size is all it asks of a frame. A lost
//! device takes the stage, so it's made again on the next one. The editor's viewport is this.
//!
//! **What it shows** is placed on [`Stage::scene`], before the first step, and what plays there is a
//! [`SceneShow`] of the caller's, in [`Stage::show`] by [`Stage::with_show`]. `apps/sapphire_sandbox`'s
//! `offscreen.rs` is every scene it can hold, flag by flag.
//!
//! **Tools:** [`FrameHooks`] on [`Stage::with_hooks`], for what a development build draws over a frame or swaps
//! in before it loads. The `dev` feature adds live tuning (`tuning`), and the packs live link pushes to
//! [`Content`]: an app's own `dev` turns it on.
//!
//! # What a game holds
//!
//! The fight and its HUD are `game_combat`'s and `game_ui`'s (M3 S2), on these seams:
//!
//! - **What plays** is a [`SceneShow`], which the caller owns and passes in: the stage calls it at each of a
//!   frame's points. It knows nothing of who's fighting or how many.
//! - **What loads:** the caller holds its own assets by [`Content::hold`] and reads them by [`Content::read`], and
//!   [`Content::load_graph_of`] keys a graph by a [`content::GraphKey`] the game names. A recolor is an
//!   [`content::UploadWatcher`].
//!
//! Half the crate's public surface is what the sandbox's scene flags build, which a game never names.

// These docs are read with `--document-private-items`, as CI renders them: a module an app reaches into keeps
// its links to what stands behind it.
#![allow(rustdoc::private_intra_doc_links)]

pub mod anim_graph;
pub mod animate;
pub mod audio;
pub mod content;
pub mod cvars;
pub mod debug_shapes;
pub mod environment;
mod extract;
pub mod faces;
pub mod foliage;
mod frame;
pub mod garments;
pub mod ik;
pub mod layers;
pub mod lookdev;
pub mod orbit;
mod outlines;
pub mod pose;
mod readback;
pub mod rims;
pub mod scene;
mod show;
pub mod skinned;
pub mod studio;
pub mod swatches;
pub mod turntable;
pub mod vfx;

pub use content::Content;
pub use extract::{RenderMesh, RenderPose, RenderTransform};
pub use frame::{
    FRAME, FrameHooks, FrameLaps, LoadingFrame, Stage, StageDesc, StageTimes, ToolFrame, gpu_failure,
    headless_gpu,
};
pub use readback::Readback;
pub use scene::Scene;
pub use show::{ContentChanges, SceneShow};

/// Log target for the stage.
pub(crate) const LOG_TARGET: &str = "sapphire::stage";

/// mimalloc, counted, as an app's: tests hold allocations to what they should be.
#[cfg(test)]
#[global_allocator]
static GLOBAL: sapphire_core::alloc::EngineAllocator = sapphire_core::alloc::engine_allocator();
