//! The renderer's debug views (spec §5.12 "Debug views", M1 F5 part 2): the frame drawn as a
//! wireframe, as overdraw, as the light-cluster heatmap, or as the anime characters' material
//! IDs, ramp rows or outline widths instead of as it looks, for look-dev and profiling. The app
//! picks one with [`Renderer::set_debug_view`](crate::Renderer::set_debug_view) every frame, as
//! it passes `r.outline_distance`; its hook is the `r.debug_view` cvar
//! ([`DEBUG_VIEW`], declared here, so the debug overlay's cvar panel lists it as a choice). The
//! sandbox passes the cvar on and sets it from `--debug-view`, which is how the golden scenes of
//! the views take it.
//!
//! # Off costs nothing
//!
//! With the view off (the default, and every shipping frame, since nothing sets the cvar there)
//! the renderer builds nothing, adds no pass and allocates nothing: one comparison a frame, and
//! once a view was shown, a second that marks it hidden. The first frame that shows a view
//! builds what that view needs, on the calling thread, as a new material variant's pipelines
//! are built: a hitch of the main pass's compiles. Other views stay unbuilt; what a view built
//! is kept when it is turned off, so showing it again is instant.
//!
//! # How the views draw
//!
//! **Replays of the main pass.** Each view replays the main pass into a depth and stencil target
//! of the scene's size: the same draws in the same order, through the same function
//! (`renderer::draw_main_pass`), with copies of the main pass's own pipelines
//! (`Pipelines::build_replay`): the same vertex and fragment stages, constants, layouts and
//! vertex streams, single-sampled and writing no color, with depth, stencil and primitive state
//! of the view's (`pipelines::Replay`). A replay therefore draws exactly the fragments the main
//! pass draws: compute skinning, foliage's wind, the eye layers and their highlights' drift,
//! alpha tests, and every later change to a surface shader, with no shader of its own to keep in
//! step. It draws through the main view without temporal AA's jitter and tests its own depth, so
//! a view holds still under TAA. Copies are made for the variants the frame draws, as the main
//! pass's own are, and made again when those are (new shader sources, a probe grid's
//! constants); its 32-bit float depth is the main pass's precision where the device has
//! `DEPTH32FLOAT_STENCIL8` (24-bit elsewhere).
//!
//! **Over the frame, after post.** Each view ends in one full-screen pass at the target's size
//! (`shaders/debug_view.wgsl`) that reads what its replay gathered, texel by texel, and draws
//! over the finished frame. **Decision: views bypass post rather than survive it.** Exposure,
//! grading, bloom and TAA never touch their colors, so a pixel's color is its value's on the
//! view's scale, matching its legend (a bar of the scale's values along the bottom-left corner)
//! at any hour, under any antialiasing (MSAA, TAA or none) and render scale, with no change to
//! post. Where a view shows no value (the wireframe between its lines, the sky in the light
//! lists) it dims the frame, for bearings.
//!
//! - **Wireframe** ([`DebugView::Wireframe`]): every surface's triangles' edges as line lists
//!   (`Replay::Edges`), tested against the depth the opaque and alpha-tested surfaces wrote
//!   first, pushed back a little (`Replay::Hide`), so the edges they hide stay hidden while
//!   transparent surfaces' show with what is behind them (the far side's too: lines have no
//!   faces to cull). Each shading family's lines take their own tone ([`WIREFRAME_TONES`]: the
//!   anime characters' warm, the environment's cool, foliage's green, fog cards' violet), over
//!   the frame at a fifth of its light. Outline hulls and eyes through hair aren't drawn; their
//!   surfaces are. **Decision: line lists of the real vertex stages, on every device.** The
//!   line fill mode (`POLYGON_MODE_LINE`) is missing on Vulkan devices without
//!   `fillModeNonSolid` (many Android GPUs), and barycentric edges need `SHADER_BARYCENTRICS`
//!   (Vulkan alone) or vertex pulling (the mesh pool's streams decoded a second way, the wind
//!   and the eyes' drift written again). So a compute pass (`cs_debug_lines`) turns a copy of
//!   the mesh pool's indices into each triangle's three edges, two indices each at twice the
//!   triangle's position, and the replay draws every draw's range of them through copies of its
//!   own pipelines as line lists: the same vertex stages, cutouts included, alike on every
//!   backend. The lines are made again each frame the wireframe shows; a pool past what one
//!   storage binding holds of them (128 MiB at the default limits, about 5.6 million triangles)
//!   reports the view unsupported ([`DebugViewStats::unsupported`]) and draws the frame as it is.
//! - **Overdraw** ([`DebugView::Overdraw`]): everything the main pass draws, replayed into a
//!   stencil incremented per fragment shaded, with the sky counting one where no surface wrote
//!   depth: transparent surfaces, eye layers, eyes through hair, outlines and fog cards
//!   included, as the main pass's draw order lets early depth testing reject them. A fragment
//!   counts if the shader runs for it, whether it then keeps or discards it: opaque surfaces
//!   count what their own shaders keep (`Replay::Count`), and every shader that may discard
//!   (alpha-tested, transparent and fog card surfaces, alpha-tested outlines, eyes through hair)
//!   counts every fragment its depth test passes through a stand-in fragment stage that keeps
//!   them all (`Replay::Invocations`; for eyes through hair, which write `frag_depth`, every
//!   fragment), and those that write depth then draw again to write it as the main pass does
//!   (`Replay::Depth`), draw by draw. The ramp runs from 1 to [`OVERDRAW_SCALE`] fragments, log
//!   scaled; beyond is white.
//! - **Light lists**, the light-cluster heatmap ([`DebugView::Lights`]): per pixel, the local
//!   lights the nearest surface's shader walks there, which [`LightStats`](crate::LightStats)
//!   counts per frame. The ramp runs from 0 to
//!   [`LightStats::list_capacity`](crate::LightStats::list_capacity), the most a list holds, log
//!   scaled, so one light reads apart from none and red is a full list (where a cluster may have
//!   dropped lights); the overlay's stats panel shows the same scale with its numbers. Colors are
//!   flat, darkened along silhouettes and creases (where the replayed depth bends), so shapes
//!   stay readable. On the clustered path (the desktop tiers) each pixel shows the count of its
//!   cluster: its tile, at the depth slice of the nearest surface's depth there (the surfaces
//!   replayed for their depth, `Replay::Depth`, transparent ones included), read from the
//!   cluster buffer as the shaders read it. **The per-object path (the mobile tiers, Mobile-Low
//!   among them) has no clusters:** there the view shows each instance's own list, 0 to 4
//!   lights, over the pixels where it is the nearest surface: the surfaces replayed
//!   (`Replay::Tag`) once per count with each instance of that count tagging its pixels in the
//!   stencil. Fog cards walk no list, so neither path shows them.
//! - **The material views** ([`DebugView::MaterialIds`], [`DebugView::RampRows`],
//!   [`DebugView::OutlineWidths`]; F5's second phase, on C7's character shaders): what each
//!   anime character pixel shades with. One replay serves all three (`Replay::Values`): first
//!   the other families' surfaces that write depth, whatever their order in the main pass, then
//!   every anime character pipeline (surfaces of every coverage, outline hulls, eyes through
//!   hair), writing values into a color target (`Rgba16Float`, exact for these integers) where
//!   the main pass draws them: what drew the pixel (a surface or a line); the region it shades
//!   with (its control map's material ID or its vertices', clamped to the material's regions as
//!   the shader clamps it); its ramp rows (day and night in one value) or, for a material
//!   without a ramp, its band mode; and its material's slot or, on a line, the line's width in
//!   pixels on a 1080-row target (the material's width × its region's × the vertex color's G ×
//!   the model scale × the width curve at the line's effective distance; the cutoff's fade is
//!   left out). The character and outline shaders write them from debug entry points
//!   (`fs_anime_character_debug`, `fs_through_hair_debug`, `vs_outline_debug`,
//!   `fs_outline_debug`) that run the same shading functions to their single exit, so they cut
//!   the same pixels, alpha tests and fades included; transparent surfaces show where they cover
//!   at least half. **Decision: debug entry points rather than a debug override constant in the
//!   shared code.** The main pass's pipelines never name the entry points, so their count and
//!   constants are what they were, and nothing a view adds can multiply them; the three views
//!   share one set of copies, so switching between them builds only a view's pass over the
//!   frame. That pass turns the values into flat colors, darkened along silhouettes, creases and
//!   the borders between materials, with lines darker ([`LINE_SHADE`]) or in their width's
//!   color, and a legend:
//!   - Material IDs: each region in its color ([`region_color`]).
//!   - Ramp rows: each row in its material ID's color, a night row bluer ([`ramp_row_color`]),
//!     with time of day's blend of the two as diagonal stripes, the night row's share of them;
//!     band modes' two or three bands in greys ([`BAND_TONES`]).
//!   - Outline widths: each line's width on the heat ramp from 0 to [`OUTLINE_WIDTH_SCALE`]
//!     pixels at 1080 rows, white beyond, over the characters in one grey ([`CHARACTER_TONE`]).
//!
//!   Where no character drew, the frame shows, dimmed.
//!
//! # Later milestones
//!
//! - Probe spheres (W5), a face-SDF threshold view (C3) and a bloom-mask view, as the
//!   techniques they show ask for them; a view of the GPU's own cluster occupancy (overflowed
//!   lists) would need the cluster pass to count them.

use std::collections::HashMap;
use std::num::NonZeroU64;

use sapphire_assets::character_maps::{MATERIAL_IDS, ramp};
use sapphire_assets::mesh::PositionFormat;
use sapphire_core::cvar::{AnyCvar, Cvar, CvarKind, CvarType, CvarValue};
use sapphire_rhi::{TransientDesc, wgpu};

use crate::LOG_TARGET;
use crate::draw::{DrawRef, DrawSet, MeshDraws};
use crate::gpu_types::ViewUniforms;
use crate::graph::{FrameGraph, TextureId};
use crate::lights::{LightPath, Lights};
use crate::pipelines::{
    Coverage, Layouts, MainPipeline, Pipelines, REPLAY_COLOR_FORMAT, Replay, ReplayTarget, Shading,
    SurfaceVariant, VALUES_FORMAT, format_index, replay_depth_format,
};
use crate::renderer::{DrawThrough, PrepassTargets, draw_main_pass};
use crate::scene::DrawList;
use crate::shader_builds::Step;
use crate::shaders::{self, Program, ShaderSources};

/// A way to draw the frame other than as it looks (see the [module docs](self)).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Hash)]
pub enum DebugView {
    /// The frame as it looks.
    #[default]
    Off,
    /// Every surface's triangles as lines over the frame dimmed, in their shading family's tone
    /// ([`WIREFRAME_TONES`]).
    Wireframe,
    /// Fragments the main pass shades per pixel, from 1 to [`OVERDRAW_SCALE`] and beyond.
    Overdraw,
    /// The light-cluster heatmap: the local lights each pixel's surface walks.
    Lights,
    /// The anime characters' material IDs: the region each pixel shades with, in its color
    /// ([`region_color`]).
    MaterialIds,
    /// The ramp rows the anime characters' shadow sides read ([`ramp_row_color`]), or their
    /// band modes ([`BAND_TONES`]).
    RampRows,
    /// The anime characters' outline widths, in pixels on a 1080-row target, from 0 to
    /// [`OUTLINE_WIDTH_SCALE`] and beyond.
    OutlineWidths,
}

/// The views' names, as the cvar and the sandbox's `--debug-view` spell them.
const NAMES: [&str; 7] =
    ["off", "wireframe", "overdraw", "lights", "material-ids", "ramp-rows", "outline-widths"];

impl DebugView {
    pub const ALL: [Self; 7] = [
        Self::Off,
        Self::Wireframe,
        Self::Overdraw,
        Self::Lights,
        Self::MaterialIds,
        Self::RampRows,
        Self::OutlineWidths,
    ];

    /// How the cvar and the command line spell it: `overdraw`.
    pub fn name(self) -> &'static str {
        NAMES[self as usize]
    }

    /// The view called `name`, ignoring case.
    pub fn from_name(name: &str) -> Option<Self> {
        <Self as CvarType>::parse(name)
    }
}

impl CvarType for DebugView {
    const KIND: CvarKind = CvarKind::Choice(&NAMES);

    fn to_bits(self) -> u32 {
        self as u32
    }

    fn from_bits(bits: u32) -> Self {
        Self::ALL.get(bits as usize).copied().unwrap_or_default()
    }

    fn parse(text: &str) -> Option<Self> {
        let index = NAMES.iter().position(|name| name.eq_ignore_ascii_case(text))?;
        Some(Self::ALL[index])
    }

    fn to_value(self) -> CvarValue {
        CvarValue::Choice { index: self as u32, names: &NAMES }
    }

    fn from_value(value: CvarValue) -> Option<Self> {
        match value {
            CvarValue::Choice { index, .. } => Self::ALL.get(index as usize).copied(),
            _ => None,
        }
    }
}

/// The debug view the app shows ([`Renderer::set_debug_view`](crate::Renderer::set_debug_view)).
pub static DEBUG_VIEW: Cvar<DebugView> = Cvar::new(
    "r.debug_view",
    DebugView::Off,
    "Debug view (spec §5.12): wireframe, overdraw (fragments shaded per pixel), lights (the lights each pixel walks, \
     per cluster or per object), or the anime characters' material-ids, ramp-rows or outline-widths, drawn over the \
     frame after post. Off builds and draws nothing.",
);

/// This module's cvars, for the app's registry.
pub static CVARS: [&dyn AnyCvar; 1] = [&DEBUG_VIEW];

/// Fragments a pixel at the top of the overdraw view's scale (red); more show white.
pub const OVERDRAW_SCALE: u32 = 16;

/// The wireframe's tone per shading family, as display (sRGB-encoded) colors, in the order the
/// families' lines draw (a later family's line wins a pixel two share): the environment's, then
/// foliage's, fog cards' and the anime characters' (`tones` in `fs_debug_wireframe`).
pub const WIREFRAME_TONES: [(&str, [f32; 3]); 4] = [
    ("environment", [0.45, 0.85, 1.0]),
    ("foliage", [0.55, 1.0, 0.4]),
    ("fog cards", [0.8, 0.6, 1.0]),
    ("characters", [1.0, 0.72, 0.3]),
];

/// The stencil tag a `shading` family's lines take in the wireframe: its tone's index in
/// [`WIREFRAME_TONES`] + 1 (0 is no line).
fn wireframe_tag(shading: Shading) -> u32 {
    match shading {
        Shading::Environment => 1,
        Shading::Foliage => 2,
        Shading::FogCard => 3,
        Shading::Character(_) => 4,
    }
}

/// The views' heat ramp's stops, display (sRGB-encoded) colors evenly spaced from 0 to 1: deep
/// blue, blue, cyan, green, yellow, orange, red (`debug_heat` in `shaders/debug_view.wgsl`).
const HEAT: [[f32; 3]; 7] = [
    [0.06, 0.06, 0.30],
    [0.10, 0.30, 0.95],
    [0.10, 0.75, 0.90],
    [0.20, 0.85, 0.30],
    [0.95, 0.90, 0.15],
    [1.00, 0.50, 0.05],
    [0.85, 0.05, 0.05],
];

/// The light lists' color for a surface walking no light (display, sRGB-encoded): the legend's
/// first cell (`debug_lights` in `shaders/debug_view.wgsl`).
pub const NO_LIGHTS: [f32; 3] = [0.15; 3];

/// The material ID view's color per region (display, sRGB-encoded), apart from each other and
/// from the band modes' greys (`debug_region_color` in `shaders/debug_view.wgsl`).
const REGION_COLORS: [[f32; 3]; MATERIAL_IDS as usize] = [
    [0.90, 0.30, 0.30],
    [0.95, 0.62, 0.20],
    [0.92, 0.88, 0.30],
    [0.40, 0.80, 0.35],
    [0.30, 0.80, 0.85],
    [0.30, 0.50, 0.95],
    [0.65, 0.40, 0.90],
    [0.95, 0.50, 0.75],
];

/// The night rows' shade: a night row is its material ID's color halfway to this (display).
const NIGHT_ROW_SHADE: [f32; 3] = [0.10, 0.12, 0.35];

/// The ramp-row view's tones for materials in band mode, which read no ramp (display): two bands,
/// and three (a deep threshold).
pub const BAND_TONES: [[f32; 3]; 2] = [[0.75; 3], [0.5; 3]];

/// The outline-width view's top, in pixels on a 1080-row target (the unit materials author
/// widths in): red; wider shows white.
pub const OUTLINE_WIDTH_SCALE: u32 = 8;

/// The outline-width view's anime character surfaces, which its lines draw over (display).
pub const CHARACTER_TONE: [f32; 3] = [0.3; 3];

/// The share of its color an outline pixel keeps in the material ID and ramp-row views, so lines
/// read apart from the surfaces they outline.
pub const LINE_SHADE: f32 = 0.45;

/// Region `region`'s color in the material ID view (display), as it draws it; past the last
/// region's, the last's.
pub fn region_color(region: u32) -> [f32; 3] {
    REGION_COLORS[(region as usize).min(REGION_COLORS.len() - 1)]
}

/// Ramp row `row`'s color in the ramp-row view (display): a day row its material ID's
/// ([`region_color`]), a night row that halfway to a night blue (`character_maps::ramp`'s
/// layout: the day rows, then the night rows).
pub fn ramp_row_color(row: u32) -> [f32; 3] {
    let ids = u32::from(MATERIAL_IDS);
    let color = region_color(row % ids);
    if row < ids {
        color
    } else {
        std::array::from_fn(|channel| (color[channel] + NIGHT_ROW_SHADE[channel]) / 2.0)
    }
}

/// The outline-width view's color for a line `width` pixels wide on a 1080-row target (display):
/// the heat ramp from 0 to [`OUTLINE_WIDTH_SCALE`], white beyond.
pub fn outline_width_color(width: f32) -> [f32; 3] {
    if width > OUTLINE_WIDTH_SCALE as f32 { [1.0; 3] } else { heat(width / OUTLINE_WIDTH_SCALE as f32) }
}

/// The views' heat ramp at `t` (0 to 1, clamped) as a display (sRGB-encoded) color, as the views
/// draw it, for legends drawn elsewhere (the debug overlay's).
pub fn heat(t: f32) -> [f32; 3] {
    let x = if t.is_nan() { 0.0 } else { t.clamp(0.0, 1.0) * 6.0 };
    let index = (x as usize).min(5);
    let (from, to, along) = (HEAT[index], HEAT[index + 1], x - index as f32);
    // WGSL's `mix`.
    std::array::from_fn(|channel| from[channel] * (1.0 - along) + to[channel] * along)
}

/// Where `count` lights in a list of at most `capacity` sit on the light-list view's ramp: log
/// scaled, 0 for none and 1 for a full list.
pub fn light_position(count: u32, capacity: u32) -> f32 {
    (1.0 + count as f32).log2() / (1.0 + capacity.max(1) as f32).log2()
}

/// Where `fragments` a pixel sit on the overdraw view's ramp: log scaled, 0 for one and 1 at
/// [`OVERDRAW_SCALE`].
pub fn overdraw_position(fragments: u32) -> f32 {
    (fragments.max(1) as f32).log2() / (OVERDRAW_SCALE as f32).log2()
}

/// The debug views' counters (rule R9), in [`RenderStats::debug_view`](crate::RenderStats).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct DebugViewStats {
    /// The view the last prepared frame draws; `Off` while none is, or one can't be.
    pub view: DebugView,
    /// The view asked for can't draw this frame, which is drawn as it is: the wireframe of a
    /// mesh pool whose line indices would pass one storage binding.
    pub unsupported: bool,
    /// The top of the shown view's scale, which its legend ends at: [`OVERDRAW_SCALE`], the
    /// most lights a list holds
    /// ([`LightStats::list_capacity`](crate::LightStats::list_capacity)), the material IDs, the
    /// ramp rows, or [`OUTLINE_WIDTH_SCALE`]; 0 without one (the wireframe).
    pub scale: u32,
    /// Pipelines the views hold: copies of the main pass's and their own passes'.
    pub pipelines: u32,
    /// Copies of the main pass's pipelines made so far, those made again for new shaders
    /// included: the views' compile hitches.
    pub copies_made: u32,
}

/// The pass that makes the wireframe's line indices.
const LINES_PASS: &str = "debug view lines";
/// The replay's pass in the frame graph.
const REPLAY_PASS: &str = "debug view replay";
/// The pass that draws a view over the frame.
const VIEW_PASS: &str = "debug view";
/// The views' passes in the frame graph, in order, for GPU timings: a shown view's cost, which
/// the frame without it doesn't have.
pub const PASSES: [&str; 3] = [LINES_PASS, REPLAY_PASS, VIEW_PASS];

/// How a view draws: its replay of the main pass and its pass over the frame
/// (`debug_view.wgsl`'s fragment stage).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Plan {
    /// The opaque and alpha-tested surfaces' depth, then every surface's edges tagged with their
    /// family's tone.
    Wireframe,
    /// Every fragment shaded counted.
    Overdraw,
    /// The surfaces' depth, which the pass reads clusters at.
    ClusterLights,
    /// The surfaces tagged with their instances' lists' counts.
    ObjectLights,
    /// The anime characters' values (`Replay::Values`), shown as their regions.
    MaterialIds,
    /// The same, shown as their ramp rows.
    RampRows,
    /// The same, shown as their lines' widths.
    OutlineWidths,
}

impl Plan {
    const ALL: [Self; 7] = [
        Self::Wireframe,
        Self::Overdraw,
        Self::ClusterLights,
        Self::ObjectLights,
        Self::MaterialIds,
        Self::RampRows,
        Self::OutlineWidths,
    ];

    /// `view`'s, on `light_path`; none for `Off`.
    fn of(view: DebugView, light_path: LightPath) -> Option<Self> {
        match view {
            DebugView::Off => None,
            DebugView::Wireframe => Some(Self::Wireframe),
            DebugView::Overdraw => Some(Self::Overdraw),
            DebugView::Lights => match light_path {
                LightPath::Clustered => Some(Self::ClusterLights),
                LightPath::PerObject => Some(Self::ObjectLights),
            },
            DebugView::MaterialIds => Some(Self::MaterialIds),
            DebugView::RampRows => Some(Self::RampRows),
            DebugView::OutlineWidths => Some(Self::OutlineWidths),
        }
    }

    /// Whether it shows the anime characters' values (`Replay::Values`), which its pass over the
    /// frame reads with the depth.
    fn values(self) -> bool {
        matches!(self, Self::MaterialIds | Self::RampRows | Self::OutlineWidths)
    }

    fn index(self) -> usize {
        self as usize
    }

    /// The kinds of copies of the main pass's pipelines its replay draws with.
    fn replays(self) -> &'static [Replay] {
        match self {
            Self::Wireframe => &[Replay::Hide, Replay::Edges],
            Self::Overdraw => &[Replay::Count, Replay::Invocations, Replay::Depth],
            Self::ClusterLights => &[Replay::Depth],
            Self::ObjectLights => &[Replay::Tag],
            Self::MaterialIds | Self::RampRows | Self::OutlineWidths => &[Replay::Values],
        }
    }

    /// Whether its replay draws the main pass's `pipeline` through a `replay` copy (see the
    /// [module docs](self)): the wireframe hides behind the surfaces that write depth and draws
    /// every surface's edges; overdraw counts every pipeline, opaque ones as they keep fragments
    /// and the rest (which may discard) as they run, and writes the depth of those that write it
    /// after counting; the light lists take every surface but fog cards, which walk no list; the
    /// material views take every anime character pipeline, which writes its values, and the
    /// depth of every other surface that writes depth, which hides them.
    fn copies(self, replay: Replay, pipeline: MainPipeline) -> bool {
        use MainPipeline::{Outline, Surface, ThroughHair};
        let card = |variant: SurfaceVariant| variant == SurfaceVariant::FOG_CARD;
        if self.values() && replay == Replay::Values {
            return match pipeline {
                Surface(variant) => {
                    matches!(variant.shading, Shading::Character(_))
                        || variant.coverage != Coverage::Transparent
                }
                Outline(_) | ThroughHair(_) => true,
            };
        }
        match (self, replay, pipeline) {
            (Self::Wireframe, Replay::Hide, Surface(variant)) => variant.coverage != Coverage::Transparent,
            (Self::Wireframe, Replay::Edges, Surface(_)) => true,
            (Self::Overdraw, Replay::Count, Surface(variant)) => variant.coverage == Coverage::Opaque,
            (Self::Overdraw, Replay::Count, Outline(coverage)) => coverage == Coverage::Opaque,
            (Self::Overdraw, Replay::Invocations, Surface(variant)) => variant.coverage != Coverage::Opaque,
            (Self::Overdraw, Replay::Invocations, Outline(coverage)) => coverage != Coverage::Opaque,
            (Self::Overdraw, Replay::Invocations, ThroughHair(_)) => true,
            (Self::Overdraw, Replay::Depth, Surface(variant)) => variant.coverage == Coverage::AlphaTest,
            (Self::Overdraw, Replay::Depth, Outline(coverage)) => coverage == Coverage::AlphaTest,
            (Self::ClusterLights, Replay::Depth, Surface(variant))
            | (Self::ObjectLights, Replay::Tag, Surface(variant)) => !card(variant),
            _ => false,
        }
    }

    fn entry_point(self) -> &'static str {
        match self {
            Self::Wireframe => "fs_debug_wireframe",
            Self::Overdraw => "fs_debug_overdraw",
            Self::ClusterLights => "fs_debug_cluster_lights",
            Self::ObjectLights => "fs_debug_object_lights",
            Self::MaterialIds => "fs_debug_material_ids",
            Self::RampRows => "fs_debug_ramp_rows",
            Self::OutlineWidths => "fs_debug_outline_widths",
        }
    }

    /// The store ops of its replay's depth and stencil: what its pass over the frame reads is
    /// kept (the light lists' and material views' depth for their silhouettes, and the cluster
    /// lookup; the other views' stencil).
    fn stores(self) -> (wgpu::StoreOp, wgpu::StoreOp) {
        let keep = |kept: bool| if kept { wgpu::StoreOp::Store } else { wgpu::StoreOp::Discard };
        let depth = matches!(self, Self::ClusterLights | Self::ObjectLights) || self.values();
        let stencil = !matches!(self, Self::ClusterLights) && !self.values();
        (keep(depth), keep(stencil))
    }
}

/// `replay`'s index in [`DebugViews::replays`].
fn replay_index(replay: Replay) -> usize {
    match replay {
        Replay::Depth => 0,
        Replay::Hide => 1,
        Replay::Count => 2,
        Replay::Invocations => 3,
        Replay::Tag => 4,
        Replay::Edges => 5,
        Replay::Values => 6,
    }
}

/// One kind of copies of the main pass's pipelines, by position format and pipeline.
#[derive(Debug, Default)]
struct ReplayPipelines {
    /// The main pass's pipelines' generation they copy (`Pipelines::generation`).
    generation: u32,
    copies: HashMap<(PositionFormat, MainPipeline), wgpu::RenderPipeline>,
}

impl ReplayPipelines {
    /// Copies every pipeline of `pipelines` that `list` draws with and `wanted` takes, unless
    /// copied already, for `target`; copies of pipelines built since are made again. Returns how
    /// many it made.
    fn prepare(
        &mut self,
        device: &wgpu::Device,
        target: &ReplayTarget,
        pipelines: &Pipelines,
        list: &DrawList,
        wanted: impl Fn(MainPipeline) -> bool,
    ) -> u32 {
        if self.generation != pipelines.generation() {
            self.copies.clear();
            self.generation = pipelines.generation();
        }
        let mut made = 0;
        for format in PositionFormat::ALL {
            let Some(mesh) = pipelines.mesh(format) else { continue };
            for variant in SurfaceVariant::ALL {
                let drawn = !list.commands[format_index(format)][variant.index()].is_empty()
                    || (format == PositionFormat::Float32 && !list.deformed[variant.index()].is_empty());
                if !drawn {
                    continue;
                }
                for set in [DrawSet::Surfaces, DrawSet::Outlines, DrawSet::ThroughHair] {
                    let Some(pipeline) = MainPipeline::of(variant, set) else { continue };
                    let key = (format, pipeline);
                    if wanted(pipeline) && mesh.main(pipeline).is_some() && !self.copies.contains_key(&key) {
                        self.copies.insert(key, pipelines.build_replay(device, target, format, pipeline));
                        made += 1;
                    }
                }
            }
        }
        made
    }

    /// The copy of the pipeline the main pass draws `set` of `variant`'s draws in `format` with,
    /// if there is one.
    fn get(
        &self,
        format: PositionFormat,
        variant: SurfaceVariant,
        set: DrawSet,
    ) -> Option<&wgpu::RenderPipeline> {
        self.copies.get(&(format, MainPipeline::of(variant, set)?))
    }
}

/// The wireframe's line indices: each triangle of the mesh pool's indices as its three edges,
/// two indices each, at twice the triangle's position, so a draw's range of triangles is twice
/// its range of lines. A compute pass (`cs_debug_lines`) makes them again each frame the
/// wireframe shows, from a copy of the pool's indices (whose buffer isn't a storage buffer).
#[derive(Debug)]
struct LineIndices {
    /// The pool's indices in use, copied.
    triangles: wgpu::Buffer,
    /// The line indices: the compute pass's output and the replay's index buffer.
    lines: wgpu::Buffer,
    bind_group: wgpu::BindGroup,
}

impl LineIndices {
    /// Room for `indices` of the pool's indices and their lines, rounded up so a growing pool
    /// makes them again rarely.
    fn new(
        device: &wgpu::Device,
        layout: &wgpu::BindGroupLayout,
        indices: u64,
        max_lines_bytes: u64,
    ) -> Self {
        let triangle_bytes = (indices * 4).next_power_of_two().clamp(256, max_lines_bytes / 2);
        let buffer = |label, size, usage| {
            device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(label),
                size,
                usage,
                mapped_at_creation: false,
            })
        };
        let storage = wgpu::BufferUsages::STORAGE;
        let triangles =
            buffer("debug view triangles", triangle_bytes, storage | wgpu::BufferUsages::COPY_DST);
        let lines = buffer("debug view lines", 2 * triangle_bytes, storage | wgpu::BufferUsages::INDEX);
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some(LINES_PASS),
            layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: LINES_TRIANGLES_BINDING,
                    resource: triangles.as_entire_binding(),
                },
                wgpu::BindGroupEntry { binding: LINES_OUTPUT_BINDING, resource: lines.as_entire_binding() },
            ],
        });
        Self { triangles, lines, bind_group }
    }

    /// Pool indices it has room for.
    fn capacity(&self) -> u64 {
        self.triangles.size() / 4
    }
}

/// The material views' values in their passes' group 2 (`view_values` in `debug_view.wgsl`), beside
/// the depth at 1.
const VALUES_BINDING: u32 = 2;

/// `cs_debug_lines`' bindings in its group 0, past the frame group's, which `debug_view.wgsl`
/// declares too.
const LINES_TRIANGLES_BINDING: u32 = 20;
const LINES_OUTPUT_BINDING: u32 = 21;
/// `cs_debug_lines`' workgroup size: the most invocations a workgroup may have at the device
/// limits `sapphire_rhi` asks for.
const LINES_WORKGROUP: u32 = 128;

/// The layouts of the views' own passes.
#[derive(Clone, Debug)]
struct ViewLayouts {
    /// Group 2 of the passes over the frame: the replay's stencil (binding 0) and depth (1).
    input: wgpu::BindGroupLayout,
    /// The passes over the frame: groups 0 (the frame's), 1 (the view) and 2 (the input).
    pass: wgpu::PipelineLayout,
    /// The material views' group 2: the replay's depth (binding 1) and values (2).
    values_input: wgpu::BindGroupLayout,
    /// Their passes over the frame, as `pass` with `values_input`.
    values_pass: wgpu::PipelineLayout,
    /// No groups: the sky's stand-in in the overdraw replay.
    sky: wgpu::PipelineLayout,
    /// `cs_debug_lines`' group 0: the pool's indices copied, and the line indices.
    lines_group: wgpu::BindGroupLayout,
    lines: wgpu::PipelineLayout,
}

impl ViewLayouts {
    fn new(device: &wgpu::Device, layouts: &Layouts) -> Self {
        let texture = |binding, sample_type| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture {
                sample_type,
                view_dimension: wgpu::TextureViewDimension::D2,
                multisampled: false,
            },
            count: None,
        };
        let input = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("debug view input"),
            entries: &[texture(0, wgpu::TextureSampleType::Uint), texture(1, wgpu::TextureSampleType::Depth)],
        });
        let pass = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some(VIEW_PASS),
            bind_group_layouts: &[Some(&layouts.frame), Some(&layouts.view), Some(&input)],
            immediate_size: 0,
        });
        let values = wgpu::TextureSampleType::Float { filterable: false };
        let values_input = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("debug view values"),
            entries: &[texture(1, wgpu::TextureSampleType::Depth), texture(VALUES_BINDING, values)],
        });
        let values_pass = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("debug view values"),
            bind_group_layouts: &[Some(&layouts.frame), Some(&layouts.view), Some(&values_input)],
            immediate_size: 0,
        });
        let sky = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("debug view sky"),
            bind_group_layouts: &[],
            immediate_size: 0,
        });
        let storage = |binding, read_only| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::COMPUTE,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Storage { read_only },
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        };
        let lines_group = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some(LINES_PASS),
            entries: &[storage(LINES_TRIANGLES_BINDING, true), storage(LINES_OUTPUT_BINDING, false)],
        });
        let lines = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some(LINES_PASS),
            bind_group_layouts: &[Some(&lines_group)],
            immediate_size: 0,
        });
        Self { input, pass, values_input, values_pass, sky, lines_group, lines }
    }
}

/// The views' own pipelines, from the debug view program (`shaders/debug_view.wgsl`).
#[derive(Clone, Debug)]
struct ViewPipelines {
    /// Also the stand-in fragment stage's, which [`Replay::Invocations`] copies draw with.
    module: wgpu::ShaderModule,
    /// Per [`Plan`], built when a view first draws with it.
    passes: [Option<wgpu::RenderPipeline>; Plan::ALL.len()],
    /// The sky's stand-in in the overdraw replay, built with its first frame.
    sky: Option<wgpu::RenderPipeline>,
    /// The wireframe's line indices' compute pass, built with its first frame.
    lines: Option<wgpu::ComputePipeline>,
}

impl ViewPipelines {
    fn new(device: &wgpu::Device, sources: &ShaderSources) -> Self {
        let module = shaders::module(device, Program::DebugView, sources);
        Self { module, passes: Default::default(), sky: None, lines: None }
    }

    /// Builds what `plan` draws with, unless built already.
    fn prepare(&mut self, builder: &ViewBuilder<'_>, plan: Plan) {
        let pass = &mut self.passes[plan.index()];
        if pass.is_none() {
            *pass = Some(builder.pass(&self.module, plan));
        }
        if plan == Plan::Overdraw && self.sky.is_none() {
            self.sky = Some(builder.sky(&self.module));
        }
        if plan == Plan::Wireframe && self.lines.is_none() {
            self.lines = Some(builder.lines(&self.module));
        }
    }

    /// The same pipelines, built from `sources` instead.
    fn rebuilt(&self, builder: &ViewBuilder<'_>, sources: &ShaderSources) -> Self {
        let module = shaders::module(builder.device, Program::DebugView, sources);
        let passes =
            Plan::ALL.map(|plan| self.passes[plan.index()].as_ref().map(|_| builder.pass(&module, plan)));
        let sky = self.sky.as_ref().map(|_| builder.sky(&module));
        let lines = self.lines.as_ref().map(|_| builder.lines(&module));
        Self { module, passes, sky, lines }
    }

    fn count(&self) -> u32 {
        let others = usize::from(self.sky.is_some()) + usize::from(self.lines.is_some());
        (self.passes.iter().flatten().count() + others) as u32
    }
}

/// What the views' own pipelines are built with.
struct ViewBuilder<'a> {
    device: &'a wgpu::Device,
    layouts: &'a ViewLayouts,
    /// The target's format, which the passes over the frame draw into.
    color_format: wgpu::TextureFormat,
    /// The replay's depth and stencil format, for the sky's stand-in.
    depth_format: wgpu::TextureFormat,
    cache: Option<&'a wgpu::PipelineCache>,
}

impl ViewBuilder<'_> {
    /// `plan`'s pass over the frame: one triangle over the target, blended over the frame by its
    /// alpha.
    fn pass(&self, module: &wgpu::ShaderModule, plan: Plan) -> wgpu::RenderPipeline {
        self.device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some(VIEW_PASS),
            layout: Some(if plan.values() { &self.layouts.values_pass } else { &self.layouts.pass }),
            vertex: wgpu::VertexState {
                module,
                entry_point: Some("vs_debug_view"),
                compilation_options: Default::default(),
                buffers: &[],
            },
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            fragment: Some(wgpu::FragmentState {
                module,
                entry_point: Some(plan.entry_point()),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: self.color_format,
                    blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview_mask: None,
            cache: self.cache,
        })
    }

    /// The sky's stand-in in the overdraw replay: one triangle at the far plane, which passes
    /// the depth test as the sky does, where no surface wrote depth, and counts a fragment
    /// there. It writes no color.
    fn sky(&self, module: &wgpu::ShaderModule) -> wgpu::RenderPipeline {
        let count = wgpu::StencilFaceState {
            compare: wgpu::CompareFunction::Always,
            fail_op: wgpu::StencilOperation::Keep,
            depth_fail_op: wgpu::StencilOperation::Keep,
            pass_op: wgpu::StencilOperation::IncrementClamp,
        };
        self.device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("debug view sky"),
            layout: Some(&self.layouts.sky),
            vertex: wgpu::VertexState {
                module,
                entry_point: Some("vs_debug_view"),
                compilation_options: Default::default(),
                buffers: &[],
            },
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: Some(wgpu::DepthStencilState {
                format: self.depth_format,
                depth_write_enabled: Some(false),
                depth_compare: Some(wgpu::CompareFunction::Equal),
                stencil: wgpu::StencilState { front: count, back: count, read_mask: 0xff, write_mask: 0xff },
                bias: Default::default(),
            }),
            multisample: wgpu::MultisampleState::default(),
            fragment: Some(wgpu::FragmentState {
                module,
                entry_point: Some("fs_debug_stand_in"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: REPLAY_COLOR_FORMAT,
                    blend: None,
                    write_mask: wgpu::ColorWrites::empty(),
                })],
            }),
            multiview_mask: None,
            cache: self.cache,
        })
    }

    /// The wireframe's line indices' compute pass.
    fn lines(&self, module: &wgpu::ShaderModule) -> wgpu::ComputePipeline {
        self.device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some(LINES_PASS),
            layout: Some(&self.layouts.lines),
            module,
            entry_point: Some("cs_debug_lines"),
            compilation_options: Default::default(),
            cache: self.cache,
        })
    }
}

/// The views' pipelines built from new shader sources, for [`DebugViews::set_pipelines`].
#[derive(Debug)]
pub(crate) struct RebuiltViews(ViewPipelines);

impl RebuiltViews {
    pub(crate) fn count(&self) -> u32 {
        self.0.count()
    }
}

/// The debug views' state, made the first time a view is shown (see the [module docs](self)).
#[derive(Debug)]
pub(crate) struct DebugViews {
    device: wgpu::Device,
    cache: Option<wgpu::PipelineCache>,
    color_format: wgpu::TextureFormat,
    /// The replays' depth and stencil format ([`replay_depth_format`]).
    depth_format: wgpu::TextureFormat,
    /// The most bytes of line indices the wireframe may make: one storage binding, one buffer,
    /// and one dispatch's workgroups.
    max_lines_bytes: u64,
    /// What the last prepared frame draws: its view, `Off` for none, and how.
    shown: DebugView,
    plan: Option<Plan>,
    /// The view asked for can't draw this frame; warned of once.
    unsupported: bool,
    warned: bool,
    /// The main view without temporal AA's jitter, which the replays and the passes over the
    /// frame bind as group 1 (at offset 0), written by the frame's uploads.
    view_uniforms: wgpu::Buffer,
    view_bind_group: wgpu::BindGroup,
    layouts: ViewLayouts,
    pipelines: ViewPipelines,
    /// Copies of the main pass's pipelines, per kind (`replay_index`).
    replays: [ReplayPipelines; 7],
    copies_made: u32,
    /// The wireframe's line indices, made with its first frame.
    lines: Option<LineIndices>,
    /// The pool's indices in use in the last prepared frame, which the lines are made from.
    index_end: u32,
    /// Per instance slot, its per-object light list's count, which the light lists' replay tags
    /// its pixels with.
    object_counts: Vec<u32>,
    /// The most lights a list holds in the last prepared frame.
    list_capacity: u32,
}

impl DebugViews {
    /// The views' state for a renderer drawing into `color_format` targets, with no pipeline
    /// built yet: [`Self::prepare`] builds what each view needs when it is first shown.
    pub(crate) fn new(
        device: &wgpu::Device,
        layouts: &Layouts,
        color_format: wgpu::TextureFormat,
        sources: &ShaderSources,
        cache: Option<&wgpu::PipelineCache>,
    ) -> Self {
        let size = size_of::<ViewUniforms>() as u64;
        let view_uniforms = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("debug view"),
            size,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let view_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("debug view"),
            layout: &layouts.view,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                    buffer: &view_uniforms,
                    offset: 0,
                    size: NonZeroU64::new(size),
                }),
            }],
        });
        let limits = device.limits();
        let dispatch =
            u64::from(limits.max_compute_workgroups_per_dimension) * u64::from(LINES_WORKGROUP) * 24;
        let max_lines_bytes =
            limits.max_storage_buffer_binding_size.min(limits.max_buffer_size).min(dispatch);
        Self {
            device: device.clone(),
            cache: cache.cloned(),
            color_format,
            depth_format: replay_depth_format(device),
            max_lines_bytes,
            shown: DebugView::Off,
            plan: None,
            unsupported: false,
            warned: false,
            view_uniforms,
            view_bind_group,
            layouts: ViewLayouts::new(device, layouts),
            pipelines: ViewPipelines::new(device, sources),
            replays: Default::default(),
            copies_made: 0,
            lines: None,
            index_end: 0,
            object_counts: Vec::new(),
            list_capacity: 0,
        }
    }

    /// Readies `view` for the frame just prepared: builds the pipelines it needs that aren't
    /// built (copies of `pipelines`' for what `list` draws), sizes the wireframe's line indices
    /// for the pool's first `index_end` indices, and takes the frame's per-object light lists
    /// from `lights`.
    pub(crate) fn prepare(
        &mut self,
        view: DebugView,
        pipelines: &Pipelines,
        list: &DrawList,
        lights: &Lights,
        (light_path, index_end): (LightPath, u32),
    ) {
        profiling::scope!("DebugViews::prepare");
        let plan = Plan::of(view, light_path);
        self.index_end = index_end;
        // Each pool index makes two line indices of four bytes.
        let lines_bytes = u64::from(index_end) * 8;
        self.unsupported = plan == Some(Plan::Wireframe) && lines_bytes > self.max_lines_bytes;
        if self.unsupported && !std::mem::replace(&mut self.warned, true) {
            tracing::warn!(
                target: LOG_TARGET,
                lines_bytes,
                max = self.max_lines_bytes,
                "the mesh pool's line indices pass one storage binding, so the wireframe view draws the frame as it is"
            );
        }
        self.plan = plan.filter(|_| !self.unsupported);
        self.shown = if self.plan.is_some() { view } else { DebugView::Off };
        self.list_capacity = lights.stats().list_capacity;
        let Some(plan) = self.plan else { return };
        let builder = ViewBuilder {
            device: &self.device,
            layouts: &self.layouts,
            color_format: self.color_format,
            depth_format: self.depth_format,
            cache: self.cache.as_ref(),
        };
        self.pipelines.prepare(&builder, plan);
        for &replay in plan.replays() {
            let target = ReplayTarget {
                replay,
                depth_format: self.depth_format,
                stand_in: self.pipelines.module.clone(),
            };
            let wanted = |pipeline| plan.copies(replay, pipeline);
            let copies = &mut self.replays[replay_index(replay)];
            self.copies_made += copies.prepare(&self.device, &target, pipelines, list, wanted);
        }
        if plan == Plan::Wireframe
            && self.lines.as_ref().is_none_or(|lines| lines.capacity() < u64::from(index_end))
        {
            let layout = &self.layouts.lines_group;
            self.lines =
                Some(LineIndices::new(&self.device, layout, u64::from(index_end), self.max_lines_bytes));
        }
        if plan == Plan::ObjectLights {
            self.object_counts.clear();
            self.object_counts.extend(lights.object_counts());
        }
    }

    /// Shows no view from this frame on, keeping what the views built.
    pub(crate) fn hide(&mut self) {
        (self.plan, self.shown, self.unsupported) = (None, DebugView::Off, false);
    }

    /// The view the last prepared frame draws, if any.
    pub(crate) fn shown(&self) -> DebugView {
        self.shown
    }

    pub(crate) fn stats(&self) -> DebugViewStats {
        let scale = match self.plan {
            Some(Plan::Overdraw) => OVERDRAW_SCALE,
            Some(Plan::ClusterLights | Plan::ObjectLights) => self.list_capacity,
            Some(Plan::MaterialIds) => u32::from(MATERIAL_IDS),
            Some(Plan::RampRows) => ramp::ROWS,
            Some(Plan::OutlineWidths) => OUTLINE_WIDTH_SCALE,
            Some(Plan::Wireframe) | None => 0,
        };
        let copies: usize = self.replays.iter().map(|replay| replay.copies.len()).sum();
        DebugViewStats {
            view: self.shown,
            unsupported: self.unsupported,
            scale,
            pipelines: self.pipelines.count() + copies as u32,
            copies_made: self.copies_made,
        }
    }

    /// The uniform buffer the frame's uploads write the unjittered main view to, while a view
    /// shows.
    pub(crate) fn view_uniforms(&self) -> &wgpu::Buffer {
        &self.view_uniforms
    }

    /// The views' own pipelines, to build from new sources (shader hot reload, off the frame
    /// thread: `shader_builds`), for [`Self::set_pipelines`] once every changed program built.
    pub(crate) fn rebuild(&self) -> Step<RebuiltViews> {
        let (device, layouts, cache) = (self.device.clone(), self.layouts.clone(), self.cache.clone());
        let (color_format, depth_format) = (self.color_format, self.depth_format);
        let built = self.pipelines.clone();
        Box::new(move |sources, _| {
            let builder = ViewBuilder {
                device: &device,
                layouts: &layouts,
                color_format,
                depth_format,
                cache: cache.as_ref(),
            };
            RebuiltViews(built.rebuilt(&builder, sources))
        })
    }

    /// The views' own pipelines built so far (not the copies), which a build off the frame thread
    /// checks it rebuilt all of (`shader_builds`).
    pub(crate) fn pipelines_built(&self) -> u32 {
        self.pipelines.count()
    }

    /// Takes the views' own pipelines built from new sources; the copies that draw with their
    /// stand-in fragment stage are made again from them with the next frame.
    pub(crate) fn set_pipelines(&mut self, pipelines: RebuiltViews) {
        self.pipelines = pipelines.0;
        self.replays[replay_index(Replay::Invocations)].copies.clear();
    }

    /// Adds the shown view's passes to `graph`, after post: the wireframe's line indices, its
    /// replay of the main pass, then its pass over the frame into `frame.target`.
    pub(crate) fn record<'f>(&'f self, graph: &mut FrameGraph<'f>, frame: DebugFrame<'f>) {
        profiling::scope!("DebugViews::record");
        let Some(plan) = self.plan else { return };
        let lines = if plan == Plan::Wireframe { self.record_lines(graph, &frame) } else { None };
        let replayed = self.record_replay(graph, plan, &frame, lines);
        self.record_pass(graph, plan, replayed, &frame);
    }

    /// The wireframe's line indices made from the pool's, by a pass before the replay that reads
    /// them; the buffer it writes, if it has one.
    fn record_lines<'f>(
        &'f self,
        graph: &mut FrameGraph<'f>,
        frame: &DebugFrame<'f>,
    ) -> Option<&'f wgpu::Buffer> {
        let (lines, pipeline) = (self.lines.as_ref()?, self.pipelines.lines.as_ref()?);
        let indices = u64::from(frame.index_end).min(lines.capacity());
        let triangles = indices.div_ceil(3);
        if triangles == 0 {
            return Some(&lines.lines);
        }
        let (source, labels) = (frame.index_buffer, frame.labels);
        // Buffers aren't graph resources: a side effect, recorded before the replay that reads
        // what it writes.
        graph.add_pass(LINES_PASS).side_effect().record(move |ctx| {
            ctx.encoder.copy_buffer_to_buffer(source, 0, &lines.triangles, 0, indices * 4);
            let mut pass = ctx.encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: labels.then_some(LINES_PASS),
                timestamp_writes: ctx.compute_timestamp_writes(),
            });
            pass.set_pipeline(pipeline);
            pass.set_bind_group(0, &lines.bind_group, &[]);
            pass.dispatch_workgroups(triangles.div_ceil(u64::from(LINES_WORKGROUP)) as u32, 1, 1);
        });
        Some(&lines.lines)
    }

    /// The main pass replayed for `plan` into a depth and stencil of the scene's size, and for
    /// the material views their values, which it returns; the wireframe draws its edges from
    /// `lines`.
    fn record_replay<'f>(
        &'f self,
        graph: &mut FrameGraph<'f>,
        plan: Plan,
        frame: &DebugFrame<'f>,
        lines: Option<&'f wgpu::Buffer>,
    ) -> Replayed {
        let (width, height) = frame.size;
        let transient = |format, usage| TransientDesc { width, height, format, sample_count: 1, usage };
        let sampled = wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING;
        // The material views' copies write the values, which their pass reads; the others' write
        // no color, so their target stays in tile memory where the GPU has it.
        let (color, color_store) = if plan.values() {
            let values = graph.create_texture("debug view values", transient(VALUES_FORMAT, sampled));
            (values, wgpu::StoreOp::Store)
        } else {
            let usage = wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TRANSIENT_ATTACHMENT;
            let color = graph.create_texture("debug view color", transient(REPLAY_COLOR_FORMAT, usage));
            (color, wgpu::StoreOp::Discard)
        };
        let depth = graph.create_texture("debug view depth", transient(self.depth_format, sampled));
        let (depth_store, stencil_store) = plan.stores();
        let pass = graph
            .add_pass(REPLAY_PASS)
            .read(frame.prepass.character_depth)
            .read(frame.prepass.character_ids)
            .write(color)
            .write(depth);
        let replays = &self.replays;
        let sky = self.pipelines.sky.as_ref();
        let (object_counts, capacity) = (&self.object_counts[..], self.list_capacity);
        let view_bind_group = &self.view_bind_group;
        let DebugFrame { draws, frame_bind_group, scene_bind_groups, index_buffer, cards, labels, .. } =
            *frame;
        pass.record(move |ctx| {
            let mut pass = ctx.encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: labels.then_some(REPLAY_PASS),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: ctx.view(color),
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                        store: color_store,
                    },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: ctx.view(depth),
                    depth_ops: Some(wgpu::Operations { load: wgpu::LoadOp::Clear(0.0), store: depth_store }),
                    stencil_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(0),
                        store: stencil_store,
                    }),
                }),
                timestamp_writes: ctx.timestamp_writes(),
                ..Default::default()
            });
            pass.set_bind_group(0, frame_bind_group, &[]);
            pass.set_bind_group(1, view_bind_group, &[0]);
            pass.set_index_buffer(index_buffer.slice(..), wgpu::IndexFormat::Uint32);
            // The plan's own copies of a kind: those of the kind another view made aren't its.
            let copy = |replay, format, variant, set| {
                let pipeline = MainPipeline::of(variant, set)?;
                let copies = &replays[replay_index(replay)];
                plan.copies(replay, pipeline).then(|| copies.get(format, variant, set)).flatten()
            };
            let one = move |replay| {
                move |format, variant, set| copy(replay, format, variant, set).map(DrawThrough::One)
            };
            // Fog cards bind their family's scene group with its stand-in depth: their depth fade
            // changes neither their count nor their edges.
            let card_group = &scene_bind_groups[SurfaceVariant::FOG_CARD.shading.scene_group()];
            let everything = |_| true;
            // Each run of the draws starts with the anime character family's scene group bound.
            let group = |pass: &mut wgpu::RenderPass<'_>| pass.set_bind_group(2, &scene_bind_groups[0], &[]);
            match plan {
                Plan::Overdraw => {
                    // Opaque draws count what they keep; the others every fragment they run for,
                    // then, if they write depth, write it as the main pass does.
                    let through = |format, variant, set| {
                        let copy = |replay| copy(replay, format, variant, set);
                        match (copy(Replay::Count), copy(Replay::Invocations), copy(Replay::Depth)) {
                            (Some(count), _, _) => Some(DrawThrough::One(count)),
                            (None, Some(invocations), Some(depth)) => {
                                Some(DrawThrough::Both([invocations, depth]))
                            }
                            (None, Some(invocations), None) => Some(DrawThrough::One(invocations)),
                            (None, None, _) => None,
                        }
                    };
                    let sky = |pass: &mut wgpu::RenderPass<'_>| {
                        if let Some(sky) = sky {
                            pass.set_pipeline(sky);
                            pass.draw(0..3, 0..1);
                        }
                    };
                    let cards = Some((card_group, cards));
                    group(&mut pass);
                    draw_main_pass(
                        &mut pass,
                        &through,
                        &draws,
                        scene_bind_groups,
                        Some(&sky),
                        cards,
                        everything,
                    );
                }
                Plan::ClusterLights => {
                    group(&mut pass);
                    draw_main_pass(
                        &mut pass,
                        &one(Replay::Depth),
                        &draws,
                        scene_bind_groups,
                        None,
                        None,
                        everything,
                    );
                }
                Plan::Wireframe => {
                    let Some(lines) = lines else { return };
                    // What hides edges first: the surfaces that write depth in the main pass.
                    group(&mut pass);
                    draw_main_pass(
                        &mut pass,
                        &one(Replay::Hide),
                        &draws,
                        scene_bind_groups,
                        None,
                        None,
                        everything,
                    );
                    // Then every surface's edges, a family at a time, each tagged with its tone.
                    let (edges, draws) = (one(Replay::Edges), MeshDraws { lines: true, ..draws });
                    pass.set_index_buffer(lines.slice(..), wgpu::IndexFormat::Uint32);
                    for tag in 1..=WIREFRAME_TONES.len() as u32 {
                        let family = |format, variant: SurfaceVariant, set| {
                            let ours = wireframe_tag(variant.shading) == tag;
                            ours.then(|| edges(format, variant, set)).flatten()
                        };
                        let cards = (tag == wireframe_tag(Shading::FogCard)).then_some((card_group, cards));
                        pass.set_stencil_reference(tag);
                        group(&mut pass);
                        draw_main_pass(
                            &mut pass,
                            &family,
                            &draws,
                            scene_bind_groups,
                            None,
                            cards,
                            everything,
                        );
                    }
                }
                Plan::ObjectLights => {
                    // One run of the draws per count, its instances' pixels tagged count + 1 (0 is
                    // no surface); each writes depth, so each pixel keeps the nearest surface's.
                    let through = one(Replay::Tag);
                    for count in 0..=capacity {
                        let tagged =
                            |slot: u32| object_counts.get(slot as usize).copied().unwrap_or(0) == count;
                        pass.set_stencil_reference(count + 1);
                        group(&mut pass);
                        draw_main_pass(&mut pass, &through, &draws, scene_bind_groups, None, None, tagged);
                    }
                }
                Plan::MaterialIds | Plan::RampRows | Plan::OutlineWidths => {
                    // What hides the anime characters first, the other families' surfaces that
                    // write depth, whatever their order in the main pass; then the characters,
                    // which write their values where the main pass draws them.
                    let values = one(Replay::Values);
                    for characters in [false, true] {
                        let ours = |format, variant: SurfaceVariant, set| {
                            let character = matches!(variant.shading, Shading::Character(_));
                            (character == characters).then(|| values(format, variant, set)).flatten()
                        };
                        group(&mut pass);
                        draw_main_pass(&mut pass, &ours, &draws, scene_bind_groups, None, None, everything);
                    }
                }
            }
        });
        Replayed { depth, values: plan.values().then_some(color) }
    }

    /// `plan`'s pass over the frame, reading `replayed`, blended into `frame.target`.
    fn record_pass<'f>(
        &'f self,
        graph: &mut FrameGraph<'f>,
        plan: Plan,
        replayed: Replayed,
        frame: &DebugFrame<'f>,
    ) {
        let Some(pipeline) = self.pipelines.passes[plan.index()].as_ref() else { return };
        let target = frame.target;
        // It draws over the frame post finished, keeping it where it blends.
        let mut builder = graph.add_pass(VIEW_PASS).read(replayed.depth).read(target).write(target);
        if let Some(values) = replayed.values {
            builder = builder.read(values);
        }
        let (device, view_bind_group, layouts) = (&self.device, &self.view_bind_group, &self.layouts);
        let (frame_bind_group, labels) = (frame.frame_bind_group, frame.labels);
        builder.record(move |ctx| {
            let aspect = |aspect| {
                ctx.texture(replayed.depth)
                    .create_view(&wgpu::TextureViewDescriptor { aspect, ..Default::default() })
            };
            let depth = aspect(wgpu::TextureAspect::DepthOnly);
            let depth_entry =
                wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(&depth) };
            // The material views read the values beside the depth; the others the stencil.
            let inputs = match replayed.values {
                Some(values) => device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some(VIEW_PASS),
                    layout: &layouts.values_input,
                    entries: &[
                        depth_entry,
                        wgpu::BindGroupEntry {
                            binding: VALUES_BINDING,
                            resource: wgpu::BindingResource::TextureView(ctx.view(values)),
                        },
                    ],
                }),
                None => {
                    let stencil = aspect(wgpu::TextureAspect::StencilOnly);
                    device.create_bind_group(&wgpu::BindGroupDescriptor {
                        label: Some(VIEW_PASS),
                        layout: &layouts.input,
                        entries: &[
                            wgpu::BindGroupEntry {
                                binding: 0,
                                resource: wgpu::BindingResource::TextureView(&stencil),
                            },
                            depth_entry,
                        ],
                    })
                }
            };
            let mut pass = ctx.encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: labels.then_some(VIEW_PASS),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: ctx.view(target),
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations { load: wgpu::LoadOp::Load, store: wgpu::StoreOp::Store },
                })],
                timestamp_writes: ctx.timestamp_writes(),
                ..Default::default()
            });
            pass.set_pipeline(pipeline);
            pass.set_bind_group(0, frame_bind_group, &[]);
            pass.set_bind_group(1, view_bind_group, &[0]);
            pass.set_bind_group(2, &inputs, &[]);
            pass.draw(0..3, 0..1);
        });
    }
}

/// What a view's replay gathered: its depth and stencil, and the material views' values.
#[derive(Clone, Copy)]
struct Replayed {
    depth: TextureId,
    values: Option<TextureId>,
}

/// What a debug view draws with in a frame's graph, from [`Renderer::record`](crate::Renderer::record).
#[derive(Clone, Copy)]
pub(crate) struct DebugFrame<'f> {
    /// The frame's target, which post finished.
    pub target: TextureId,
    /// The scene's size: the target's at the render scale.
    pub size: (u32, u32),
    /// The character prepass's targets, which the frame's group 0 binds.
    pub prepass: PrepassTargets,
    /// The main pass's draws.
    pub draws: MeshDraws<'f>,
    pub frame_bind_group: &'f wgpu::BindGroup,
    pub scene_bind_groups: &'f [wgpu::BindGroup],
    /// The mesh pool's indices, and the end of those in use.
    pub index_buffer: &'f wgpu::Buffer,
    pub index_end: u32,
    /// The fog cards' draws, back to front.
    pub cards: &'f [DrawRef],
    pub labels: bool,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pipelines::Part;

    #[test]
    fn views_round_trip_through_their_names() {
        for view in DebugView::ALL {
            assert_eq!(DebugView::from_name(view.name()), Some(view));
            assert_eq!(DebugView::from_bits(view.to_bits()), view);
            assert_eq!(DebugView::from_value(view.to_value()), Some(view));
        }
        assert_eq!(DebugView::from_name("Overdraw"), Some(DebugView::Overdraw), "any case");
        assert_eq!(DebugView::from_name("normals"), None);
        assert_eq!(DebugView::from_bits(99), DebugView::Off);
        assert_eq!(DEBUG_VIEW.default(), DebugView::Off, "off unless asked");
    }

    #[test]
    fn views_plan_what_they_draw_with() {
        let clustered = LightPath::Clustered;
        assert_eq!(Plan::of(DebugView::Off, clustered), None, "off builds and draws nothing");
        assert_eq!(Plan::of(DebugView::Wireframe, clustered), Some(Plan::Wireframe));
        assert_eq!(Plan::of(DebugView::Overdraw, clustered), Some(Plan::Overdraw));
        // Clusters are read at the surfaces' depth; per-object lists need the instances' pixels.
        assert_eq!(Plan::of(DebugView::Lights, clustered), Some(Plan::ClusterLights));
        assert_eq!(Plan::of(DebugView::Lights, LightPath::PerObject), Some(Plan::ObjectLights));
        assert_eq!(Plan::Wireframe.replays(), [Replay::Hide, Replay::Edges], "what hides edges first");
        // The material views share one replay, whatever the light path.
        for (view, plan) in [
            (DebugView::MaterialIds, Plan::MaterialIds),
            (DebugView::RampRows, Plan::RampRows),
            (DebugView::OutlineWidths, Plan::OutlineWidths),
        ] {
            assert_eq!(Plan::of(view, clustered), Some(plan));
            assert_eq!(Plan::of(view, LightPath::PerObject), Some(plan));
            assert!(plan.values());
            assert_eq!(plan.replays(), [Replay::Values]);
        }
        for (index, plan) in Plan::ALL.into_iter().enumerate() {
            assert_eq!(plan.index(), index);
            // What each pass reads is kept: the stencil, the depth, or the depth and the values.
            let (depth, stencil) = plan.stores();
            let lights = matches!(plan, Plan::ClusterLights | Plan::ObjectLights);
            assert_eq!(depth == wgpu::StoreOp::Store, lights || plan.values(), "{plan:?}");
            let stencil_read = !matches!(plan, Plan::ClusterLights) && !plan.values();
            assert_eq!(stencil == wgpu::StoreOp::Store, stencil_read, "{plan:?}");
            let writes_values = plan.replays().iter().any(|replay| replay.color_format() == VALUES_FORMAT);
            assert_eq!(writes_values, plan.values(), "{plan:?}");
        }
        let mut indices = REPLAYS.map(replay_index);
        indices.sort_unstable();
        assert_eq!(indices, [0, 1, 2, 3, 4, 5, 6], "one set of copies per kind");
    }

    const REPLAYS: [Replay; 7] = [
        Replay::Depth,
        Replay::Hide,
        Replay::Count,
        Replay::Invocations,
        Replay::Tag,
        Replay::Edges,
        Replay::Values,
    ];

    #[test]
    fn replays_copy_what_their_view_draws() {
        let opaque = MainPipeline::Surface(SurfaceVariant::OPAQUE);
        let tested =
            MainPipeline::Surface(SurfaceVariant { coverage: Coverage::AlphaTest, ..SurfaceVariant::OPAQUE });
        let blended = MainPipeline::Surface(SurfaceVariant {
            coverage: Coverage::Transparent,
            ..SurfaceVariant::OPAQUE
        });
        let card = MainPipeline::Surface(SurfaceVariant::FOG_CARD);
        let (outline, tested_outline) =
            (MainPipeline::Outline(Coverage::Opaque), MainPipeline::Outline(Coverage::AlphaTest));
        let eyes = MainPipeline::ThroughHair(SurfaceVariant::EYE_LAYERS);
        let world = |coverage| MainPipeline::Surface(SurfaceVariant::environment(coverage));
        let (world, tested_world, glass) =
            (world(Coverage::Opaque), world(Coverage::AlphaTest), world(Coverage::Transparent));
        let leaves = MainPipeline::Surface(SurfaceVariant::foliage(Coverage::AlphaTest));
        let all = [
            opaque,
            tested,
            blended,
            card,
            outline,
            tested_outline,
            eyes,
            world,
            tested_world,
            glass,
            leaves,
        ];
        let copied = |plan: Plan, replay| {
            all.iter().filter(|&&pipeline| plan.copies(replay, pipeline)).copied().collect::<Vec<_>>()
        };
        // The wireframe hides behind what writes depth, and draws every surface's edges.
        assert_eq!(copied(Plan::Wireframe, Replay::Hide), [opaque, tested, world, tested_world, leaves]);
        assert_eq!(
            copied(Plan::Wireframe, Replay::Edges),
            [opaque, tested, blended, card, world, tested_world, glass, leaves]
        );
        // Overdraw counts every pipeline once: opaque ones as they keep fragments, the rest as
        // they run; those that write depth then write it.
        assert_eq!(copied(Plan::Overdraw, Replay::Count), [opaque, outline, world]);
        assert_eq!(
            copied(Plan::Overdraw, Replay::Invocations),
            [tested, blended, card, tested_outline, eyes, tested_world, glass, leaves]
        );
        assert_eq!(copied(Plan::Overdraw, Replay::Depth), [tested, tested_outline, tested_world, leaves]);
        // Fog cards walk no light list.
        let lit = [opaque, tested, blended, world, tested_world, glass, leaves];
        assert_eq!(copied(Plan::ClusterLights, Replay::Depth), lit);
        assert_eq!(copied(Plan::ObjectLights, Replay::Tag), lit);
        // The material views write every anime character pipeline's values, behind every other
        // surface that writes depth; glass and fog cards hide no character.
        for plan in [Plan::MaterialIds, Plan::RampRows, Plan::OutlineWidths] {
            assert_eq!(
                copied(plan, Replay::Values),
                [opaque, tested, blended, outline, tested_outline, eyes, world, tested_world, leaves]
            );
        }
        // Nothing else.
        for plan in Plan::ALL {
            for replay in REPLAYS {
                if !plan.replays().contains(&replay) {
                    assert!(copied(plan, replay).is_empty(), "{plan:?} {replay:?}");
                }
            }
        }
    }

    #[test]
    fn every_family_has_a_wireframe_tone() {
        let families =
            [Shading::Environment, Shading::Foliage, Shading::FogCard, Shading::Character(Part::Body)];
        assert_eq!(families.map(wireframe_tag), [1, 2, 3, 4], "in the tones' order");
        assert_eq!(
            WIREFRAME_TONES[wireframe_tag(Shading::Character(Part::Hair)) as usize - 1].0,
            "characters"
        );
    }

    #[test]
    fn scales_run_from_their_bottom_to_their_top() {
        assert_eq!(light_position(0, 64), 0.0);
        assert_eq!(light_position(64, 64), 1.0, "a full list is red");
        assert!(light_position(1, 64) > 0.15, "one light reads apart from none on a desktop scale");
        assert_eq!(light_position(4, 4), 1.0);
        assert_eq!(light_position(0, 0), 0.0, "no lights allowed");
        assert_eq!(overdraw_position(1), 0.0);
        assert_eq!(overdraw_position(OVERDRAW_SCALE), 1.0);
        assert_eq!(overdraw_position(4), 0.5);
        assert_eq!(heat(0.0), HEAT[0]);
        assert_eq!(heat(1.0), HEAT[6]);
        assert_eq!(heat(2.0), HEAT[6], "clamped");
        assert_eq!(heat(f32::NAN), HEAT[0]);
        let mid = heat(0.5 / 6.0);
        assert!(
            (0..3).all(|channel| (mid[channel] - (HEAT[0][channel] + HEAT[1][channel]) / 2.0).abs() < 1e-6)
        );
    }

    #[test]
    fn the_material_views_colors_tell_their_values_apart() {
        let regions: Vec<[f32; 3]> = (0..u32::from(MATERIAL_IDS)).map(region_color).collect();
        for (index, color) in regions.iter().enumerate() {
            assert!(!regions[..index].contains(color), "region {index}'s color is its own");
            assert!(!BAND_TONES.contains(color), "and no band mode's");
        }
        assert_eq!(region_color(99), region_color(7), "clamped");
        // A day row is its material ID's color; a night row a darker, bluer one of it.
        for row in 0..ramp::ROWS {
            let id = row % u32::from(MATERIAL_IDS);
            let color = ramp_row_color(row);
            if row < u32::from(MATERIAL_IDS) {
                assert_eq!(color, region_color(id));
            } else {
                assert_ne!(color, region_color(id));
                assert!(color.iter().sum::<f32>() < region_color(id).iter().sum::<f32>() + 0.05, "row {row}");
            }
        }
        assert_eq!(outline_width_color(0.0), HEAT[0]);
        assert_eq!(outline_width_color(OUTLINE_WIDTH_SCALE as f32), HEAT[6], "red at the top");
        assert_eq!(outline_width_color(OUTLINE_WIDTH_SCALE as f32 + 0.5), [1.0; 3], "white beyond");
        assert_eq!(outline_width_color(4.0), heat(0.5));
    }

    /// The `vec3<f32>(…)` rows after the line of `source` containing `start`.
    fn wgsl_rows(source: &str, start: &str, rows: usize) -> Vec<[f32; 3]> {
        source
            .lines()
            .skip_while(|line| !line.contains(start))
            .skip(1)
            .take(rows)
            .map(|line| {
                let values = line.trim().trim_start_matches("vec3<f32>(").trim_end_matches("),");
                let parsed: Vec<f32> =
                    values.split(',').map(|value| value.trim().parse().expect("a number")).collect();
                [parsed[0], parsed[1], parsed[2]]
            })
            .collect()
    }

    #[test]
    fn the_shader_mirrors_the_cpu_colors_and_constants() {
        let source = Program::DebugView.files().last().expect("its own file last").compiled_in();
        assert_eq!(wgsl_rows(source, "var stops = array<vec3<f32>, 7>(", HEAT.len()), HEAT);
        let tones: Vec<[f32; 3]> = WIREFRAME_TONES.iter().map(|&(_, tone)| tone).collect();
        assert_eq!(wgsl_rows(source, "var tones = array<vec3<f32>, 4>(", tones.len()), tones);
        assert_eq!(
            wgsl_rows(source, "var colors = array<vec3<f32>, 8>(", REGION_COLORS.len()),
            REGION_COLORS
        );
        let [two, three] = BAND_TONES.map(|tone| tone[0]);
        assert!(BAND_TONES.iter().all(|tone| tone.iter().all(|&channel| channel == tone[0])), "greys");
        let [r, g, b] = NIGHT_ROW_SHADE;
        for constant in [
            format!("const DEBUG_OVERDRAW_SCALE: u32 = {OVERDRAW_SCALE}u;"),
            format!("const DEBUG_NO_LIGHTS: f32 = {:?};", NO_LIGHTS[0]),
            format!("const DEBUG_MATERIAL_IDS: u32 = {MATERIAL_IDS}u;"),
            format!("const DEBUG_RAMP_ROWS: u32 = {}u;", ramp::ROWS),
            format!("const DEBUG_OUTLINE_WIDTH_SCALE: u32 = {OUTLINE_WIDTH_SCALE}u;"),
            format!("const DEBUG_CHARACTER_TONE: f32 = {:?};", CHARACTER_TONE[0]),
            format!("const DEBUG_LINE_SHADE: f32 = {LINE_SHADE:?};"),
            format!("let night = mix(color, vec3<f32>({r:?}, {g:?}, {b:?}), 0.5);"),
            format!("return select(vec3<f32>({three:?}), vec3<f32>({two:?}), code == DEBUG_TWO_BANDS);"),
            format!("@group(2) @binding({VALUES_BINDING}) var view_values: texture_2d<f32>;"),
            format!("@group(0) @binding({LINES_TRIANGLES_BINDING}) var<storage, read> debug_triangles"),
            format!("@group(0) @binding({LINES_OUTPUT_BINDING}) var<storage, read_write> debug_lines"),
            format!("@compute @workgroup_size({LINES_WORKGROUP})"),
        ] {
            assert!(source.contains(&constant), "{constant}");
        }
        assert!(NO_LIGHTS.iter().all(|&channel| channel == NO_LIGHTS[0]), "a grey");
        assert!(CHARACTER_TONE.iter().all(|&channel| channel == CHARACTER_TONE[0]), "a grey");
    }

    #[test]
    fn the_values_codes_match_between_the_shaders() {
        // `character_debug_values` writes them (anime_character.wgsl); `debug_view.wgsl`, which
        // doesn't include that file, reads them.
        let writer = Program::AnimeCharacter.files().last().expect("its own file last").compiled_in();
        let reader = Program::DebugView.files().last().expect("its own file last").compiled_in();
        for constant in [
            "const DEBUG_DRAWN_SURFACE: f32 = 1.0;",
            "const DEBUG_DRAWN_LINE: f32 = 2.0;",
            "const DEBUG_TWO_BANDS: f32 = -1.0;",
            "const DEBUG_THREE_BANDS: f32 = -2.0;",
        ] {
            assert!(writer.contains(constant) && reader.contains(constant), "{constant}");
        }
        assert!(writer.contains(&format!("const RAMP_ROWS: f32 = {:?};", ramp::ROWS as f32)));
    }
}
