//! The overlay's panels, in egui: a bar of toggles, then stats and budgets with the frame-time
//! graph, the frame graph, time controls, time of day, the screen effects, the console and the
//! cvar list.

use std::time::Duration;

use egui::{Align2, Color32, FontId, Id, Order, RichText, Sense, Stroke, Ui, pos2, vec2};
use sapphire_assets::AssetKind;
use sapphire_core::cvar::{AnyCvar, CvarRegistry, CvarValue};
use sapphire_input::{DeviceKind, GamepadFamily};
use sapphire_platform::PacingSource;
use sapphire_render::debug_view::{
    self, BAND_TONES, CHARACTER_TONE, NO_LIGHTS, OUTLINE_WIDTH_SCALE, OVERDRAW_SCALE, WIREFRAME_TONES, heat,
    light_position, outline_width_color, overdraw_position, ramp_row_color, region_color,
};
use sapphire_render::graph::{GraphReport, TextureReport};
use sapphire_render::quality::{ResolutionSignal, TierSource};
use sapphire_render::{DebugView, DebugViewStats, MAIN_PASS, PREPASSES, ambient, post, shadows, temporal};
use sapphire_time::{NORMAL_SPEED, ScaledClock};

use crate::budget::{self, BudgetTier, CpuStage, CpuStages, format_bytes, format_ms};
use crate::console::{Completion, Console, LineKind};
use crate::effects::{self, EffectControls};
use crate::history::{FrameTimes, HISTORY};
use crate::overlay::{HotReloadStatus, LiveLinkStatus, OverlayCost, OverlayFrame};
use crate::time::{SPEEDS, TimeControls};
use crate::time_of_day::{self, TodControls, TodPanel};
use crate::{GPU_TIMING, route};

pub(crate) const GOOD: Color32 = Color32::from_rgb(120, 200, 130);
const WARN: Color32 = Color32::from_rgb(230, 190, 70);
pub(crate) const OVER: Color32 = Color32::from_rgb(240, 95, 85);

/// Which panels are open.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Panels {
    pub stats: bool,
    pub graph: bool,
    pub time: bool,
    /// Time of day.
    pub day: bool,
    /// The screen effects (M1 P3).
    pub effects: bool,
    pub console: bool,
    pub cvars: bool,
}

impl Default for Panels {
    /// Stats and the frame graph.
    fn default() -> Self {
        Self { stats: true, graph: true, ..Self::NONE }
    }
}

impl Panels {
    pub const NAMES: [&'static str; 7] = ["stats", "graph", "time", "day", "effects", "console", "cvars"];

    const NONE: Self = Self {
        stats: false,
        graph: false,
        time: false,
        day: false,
        effects: false,
        console: false,
        cvars: false,
    };
    const ALL: Self =
        Self { stats: true, graph: true, time: true, day: true, effects: true, console: true, cvars: true };

    /// A comma-separated list of [`Self::NAMES`], or `all`.
    pub fn parse(list: &str) -> Result<Self, String> {
        let mut panels = Self::NONE;
        for name in list.split(',').map(str::trim) {
            match name {
                "all" => panels = Self::ALL,
                "stats" => panels.stats = true,
                "graph" => panels.graph = true,
                "time" => panels.time = true,
                "day" => panels.day = true,
                "effects" => panels.effects = true,
                "console" => panels.console = true,
                "cvars" => panels.cvars = true,
                _ => {
                    return Err(format!(
                        "unknown overlay panel `{name}`; panels: all, {}",
                        Self::NAMES.join(", ")
                    ));
                }
            }
        }
        Ok(panels)
    }
}

/// UI state that lives across frames.
pub(crate) struct UiMemory {
    pub panels: Panels,
    console: Console,
    console_input: String,
    focus_console: bool,
    day: TodPanel,
}

impl UiMemory {
    pub fn new(panels: Panels) -> Self {
        Self {
            panels,
            console: Console::default(),
            console_input: String::new(),
            focus_console: true,
            day: TodPanel::default(),
        }
    }
}

/// What a frame's UI reads.
pub(crate) struct View<'a> {
    pub frame: &'a OverlayFrame<'a>,
    pub history: &'a FrameTimes,
    pub stages: Option<&'a CpuStages>,
    pub cost: OverlayCost,
    /// The safe area, in points.
    pub safe: egui::Rect,
    /// GPU timing is on.
    pub gpu_timing: bool,
}

/// The overlay's look: dark, opaque windows, and labels that don't take drags (so a finger
/// dragging on a stat moves the window).
///
/// egui blends in sRGB space, but the overlay draws into the frame's sRGB target, which blends
/// in linear space, so translucent colors come out fainter than egui intends: windows are
/// opaque, and weak text, which egui draws translucent, is an opaque grey.
pub(crate) fn style(ctx: &egui::Context) {
    ctx.set_theme(egui::Theme::Dark);
    ctx.all_styles_mut(|style| {
        style.visuals.window_fill = Color32::from_rgb(16, 18, 22);
        style.visuals.weak_text_color = Some(Color32::from_gray(150));
        style.interaction.selectable_labels = false;
        style.spacing.item_spacing = vec2(6.0, 3.0);
    });
}

/// Shows every open panel; returns whether the player asked to hide the overlay.
pub(crate) fn show(
    ctx: &egui::Context,
    memory: &mut UiMemory,
    time: &mut TimeControls,
    day: &mut TodControls,
    effects: &mut EffectControls,
    view: &View<'_>,
) -> bool {
    let hide = bar(ctx, &mut memory.panels, view);
    // First placement, laid out for screens 1280 points wide or more (windows remember where
    // they were moved): stats on the left, the frame graph on the right, time controls between
    // them with time of day below and the console under them, and the cvars under the frame
    // graph.
    let safe = view.safe;
    let at = |x: f32, y: f32| safe.min + vec2(x, y);
    let right = |width: f32, y: f32| pos2(safe.max.x - width - 6.0, safe.min.y + y).max(at(440.0, y));
    let UiMemory { panels, console, console_input, focus_console, day: day_panel } = memory;
    panel("Stats", &mut panels.stats, at(6.0, 44.0), safe)
        .default_width(340.0)
        .resizable(false)
        .show(ctx, |ui| stats(ui, view));
    panel("Frame graph", &mut panels.graph, right(440.0, 44.0), safe)
        .default_width(440.0)
        .show(ctx, |ui| graph(ui, view));
    panel("Time", &mut panels.time, at(440.0, 44.0), safe)
        .resizable(false)
        .show(ctx, |ui| time_controls(ui, time, view.frame.world_clock));
    panel("Time of day", &mut panels.day, at(440.0, 180.0), safe)
        .default_width(560.0)
        .resizable(false)
        .show(ctx, |ui| {
            time_of_day::panel(ui, day_panel, day, view.frame.time_of_day, view.frame.characters)
        });
    panel("Screen effects", &mut panels.effects, at(1010.0, 180.0), safe)
        .default_width(360.0)
        .show(ctx, |ui| effects::panel(ui, effects, view.frame.screen_effects));
    panel("Cvars", &mut panels.cvars, right(380.0, (safe.height() - 250.0).max(44.0)), safe)
        .default_width(380.0)
        .show(ctx, |ui| cvars(ui, view.frame.cvars));
    let console_open = panels.console;
    panel("Console", &mut panels.console, at(440.0, (safe.height() - 230.0).max(200.0)), safe)
        .default_size(vec2(440.0, 200.0))
        .show(ctx, |ui| console_panel(ui, console, console_input, focus_console, view.frame.cvars));
    if !console_open {
        *focus_console = true;
    }
    hide
}

/// A panel's window: placed at `pos` the first time, kept inside the safe area, and shown at
/// once rather than faded in, so a capture a few frames in shows it whole.
fn panel<'a>(title: &'static str, open: &'a mut bool, pos: egui::Pos2, safe: egui::Rect) -> egui::Window<'a> {
    egui::Window::new(title).open(open).default_pos(pos).constrain_to(safe).fade_in(false)
}

/// The bar of panel toggles, top left of the safe area.
fn bar(ctx: &egui::Context, panels: &mut Panels, view: &View<'_>) -> bool {
    let mut hide = false;
    egui::Area::new(Id::unique("sapphire_debug.bar"))
        .fixed_pos(view.safe.min + vec2(6.0, 6.0))
        .order(Order::Foreground)
        .show(ctx, |ui| {
            egui::Frame::popup(ui.style()).show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.strong("Sapphire");
                    let summary = view.history.summary();
                    let frame = Duration::from_secs_f32(summary.mean);
                    let color = budget_color(budget::budget_use(frame, BudgetTier::CURRENT.frame()));
                    ui.label(RichText::new(format!("{:.0} fps", summary.fps())).monospace().color(color));
                    ui.separator();
                    ui.toggle_value(&mut panels.stats, "Stats");
                    ui.toggle_value(&mut panels.graph, "Graph");
                    ui.toggle_value(&mut panels.time, "Time");
                    ui.toggle_value(&mut panels.day, "Day");
                    ui.toggle_value(&mut panels.effects, "Effects");
                    ui.toggle_value(&mut panels.console, "Console");
                    ui.toggle_value(&mut panels.cvars, "Cvars");
                    ui.separator();
                    let hint = format!(
                        "Hide the overlay. ` toggles it, as does holding two fingers still for {} s.",
                        route::HOLD_TIME.as_secs()
                    );
                    hide = ui.button("Hide").on_hover_text(hint).clicked();
                });
            });
        });
    hide
}

fn stats(ui: &mut Ui, view: &View<'_>) {
    let tier = BudgetTier::CURRENT;
    let frame = view.frame;
    ui.weak(format!("Budgets: {} (spec §3)", tier.name()));

    let summary = view.history.summary();
    let seconds = |s: f32| Duration::from_secs_f32(s);
    let frame_budget = tier.frame();
    egui::Grid::new("frame").num_columns(5).show(ui, |ui| {
        for heading in ["Frame", "last", "mean", "p99", "max"] {
            ui.weak(heading);
        }
        ui.end_row();
        ui.label(format!("{} frames", summary.frames));
        let last = view.history.last().unwrap_or_default();
        for value in [last, summary.mean, summary.p99, summary.max] {
            let used = seconds(value);
            ui.label(
                RichText::new(format_ms(used))
                    .monospace()
                    .color(budget_color(budget::budget_use(used, frame_budget))),
            );
        }
        ui.end_row();
    });
    frame_time_graph(ui, view.history, frame_budget);
    pacing(ui, frame);

    ui.separator();
    egui::Grid::new("cpu").num_columns(3).striped(true).show(ui, |ui| {
        ui.weak("CPU, averaged");
        ui.weak("time");
        ui.weak("budget");
        ui.end_row();
        let Some(stages) = view.stages else {
            ui.label("The app reports no stage times.");
            ui.end_row();
            return;
        };
        for stage in CpuStage::ALL {
            let name = match stage {
                CpuStage::Simulation => format!("{} ({})", stage.name(), stages.fixed_steps),
                _ => stage.name().to_owned(),
            };
            budget_row(ui, &name, stages.get(stage), tier.stage(stage, stages.fixed_steps));
        }
        budget_row(ui, "Game side (of §3.2's total)", stages.game_side(), tier.game_side());
        budget_row(ui, "CPU critical path", stages.total(), tier.cpu_critical_path());
        let gpu = frame.graph.gpu_timings();
        match gpu {
            Some(timings) => {
                budget_row(ui, "GPU frame (pass span)", timings.span(), tier.gpu_frame());
                // The prepasses: the character prepass on every tier (M1 C5), the scene prepass
                // under TAA, what each added to the frame. Then the main pass, foliage's draws
                // (M1 W6) among every family's.
                let prepasses =
                    PREPASSES.iter().filter_map(|name| timings.get(name)).map(|pass| pass.exclusive);
                budget_row(ui, "  Culling + prepass", prepasses.sum(), tier.prepass());
                if let Some(main) = timings.get(MAIN_PASS) {
                    budget_row(ui, "  Forward (main pass)", main.exclusive, tier.forward_opaque());
                }
                // Temporal AA's two passes (the renderer's `responsive mask` and `taa`), what
                // each added to the frame.
                if frame.render.temporal.enabled {
                    let taa = temporal::PASSES
                        .iter()
                        .filter_map(|name| timings.get(name))
                        .map(|pass| pass.exclusive);
                    budget_row(ui, "  TAA / upsampling", taa.sum(), tier.taa_upscaling());
                }
                // Post's passes (the bloom chain and the fused pass, M1 P2).
                let post =
                    post::PASSES.iter().filter_map(|name| timings.get(name)).map(|pass| pass.exclusive);
                budget_row(ui, "  Bloom + tonemap + LUT", post.sum(), tier.post());
                // The shadow passes (M1 R4): the cascades drawn, the character maps and the face
                // pass, what each added to the frame.
                if frame.render.shadows.enabled {
                    let passes = shadows::PASSES.iter().filter_map(|name| timings.get(name));
                    budget_row(ui, "  Shadows", passes.map(|pass| pass.exclusive).sum(), tier.shadows());
                }
                // The light-cluster pass and the ambient pass (M1 W5): the probe grid's relight,
                // the sky's projection and prefilter, the characters' ambient and GTAO.
                let mut clustering_ao = std::iter::once(&CLUSTER_PASS)
                    .chain(&ambient::PASSES)
                    .filter_map(|name| timings.get(name))
                    .peekable();
                if clustering_ao.peek().is_some() {
                    let used = clustering_ao.map(|pass| pass.exclusive).sum();
                    match tier.clustering_ao() {
                        Some(budget) => budget_row(ui, "  Light clustering, AO", used, budget),
                        None => unbudgeted_row(ui, "  Light clustering, AO", used),
                    }
                }
                // A debug view's replay and its pass over the frame (M1 F5): what showing it
                // adds, which the frame without it doesn't have.
                let mut views = debug_view::PASSES.iter().filter_map(|name| timings.get(name)).peekable();
                if views.peek().is_some() {
                    unbudgeted_row(ui, "  Debug view", views.map(|pass| pass.exclusive).sum());
                }
            }
            None => {
                ui.label("GPU frame");
                ui.weak(gpu_timing_state(view));
                ui.end_row();
            }
        }
    });

    ui.separator();
    quality(ui, frame);

    ui.separator();
    egui::Grid::new("counters").num_columns(2).show(ui, |ui| {
        let row = |ui: &mut Ui, name: &str, value: String| {
            ui.label(name);
            ui.monospace(value);
            ui.end_row();
        };
        row(
            ui,
            "Allocations / frame",
            format!("{} ({})", frame.allocations.allocations, format_bytes(frame.allocations.bytes)),
        );
        row(
            ui,
            "  of which overlay",
            format!(
                "{} ({}, {} vertices)",
                view.cost.allocations,
                format_ms(view.cost.cpu),
                view.cost.vertices
            ),
        );
        let render = &frame.render;
        row(
            ui,
            "Draws",
            format!(
                "{} surfaces ({} alpha-tested, {} transparent, {} faces, {} hair, {} eyes, {} environment, {} \
                 foliage on {} plants), {} outlines ({} past their cutoff), {} instances",
                render.draws,
                render.alpha_tested_draws,
                render.transparent_draws,
                render.face_draws,
                render.hair_draws,
                render.eye_draws,
                render.environment_draws,
                render.foliage_draws,
                render.foliage_instances,
                render.outline_draws,
                render.outlines_culled,
                render.instances
            ),
        );
        let prepass = &render.character_prepass;
        row(
            ui,
            "Character prepass",
            format!(
                "{} draws, {} faces under bangs, {} eye draws through hair, {} eye layers",
                prepass.draws, prepass.hair_shadow_characters, prepass.through_hair_draws, prepass.eye_layer_draws
            ),
        );
        row(
            ui,
            "Fading characters",
            format!("{} ({} draws through the dither)", render.faded_characters, render.faded_draws),
        );
        row(ui, "Character pipelines", render.character_pipelines.to_string());
        row(ui, "Environment pipelines", render.environment_pipelines.to_string());
        row(ui, "Foliage pipelines", render.foliage_pipelines.to_string());
        row(ui, "Changed instances", render.changed_instances.to_string());
        row(ui, "Draw-list builds", render.draw_list_builds.to_string());
        row(ui, "Mesh pool", format_bytes(render.mesh_pool_bytes));
        let skinning = &render.skinning;
        let skinned = if skinning.instances == 0 {
            "no skinned or morphed instances".to_owned()
        } else {
            format!(
                "{} of {} instances, {} vertices, {} of palettes",
                skinning.skinned,
                skinning.instances,
                skinning.vertices,
                format_bytes(u64::from(skinning.palette_bytes))
            )
        };
        row(ui, "Skinning", skinned);
        row(ui, "Sky", sky_text(&render.sky));
        row(ui, "Post layers", post_text(&render.post));
        row(ui, "Screen effects", effects_text(&render.post.effects, view.frame.screen_effects));
        row(ui, "Fog", fog_text(&render.fog));
        let taa = &render.temporal;
        let taa = if taa.enabled {
            let ((input_w, input_h), (output_w, output_h)) = (taa.input_size, taa.output_size);
            let [jx, jy] = taa.jitter();
            let reset = taa.reset.map_or(String::new(), |reason| format!(" ({reason:?} now)"));
            format!(
                "{input_w}×{input_h} → {output_w}×{output_h}, jitter {}/{} ({jx:+.2}, {jy:+.2}), {} reset{}{reset}",
                taa.jitter_index + 1,
                taa.jitter_phases,
                taa.resets,
                if taa.resets == 1 { "" } else { "s" }
            )
        } else {
            "off".to_owned()
        };
        row(ui, "Temporal AA", taa);
        row(ui, "Buffer growths", render.buffer_growths.to_string());
        row(ui, "LUT bakes", render.lut_bakes.to_string());
        if let Some(tod) = frame.time_of_day {
            let stats = tod.stats();
            row(
                ui,
                "Time of day",
                format!(
                    "{}, {} of {} zones",
                    format_ms(stats.resolve_time),
                    stats.active_volumes,
                    stats.volumes
                ),
            );
        }
        // The light lists' debug view (`r.debug_view lights`) shows these lists' counts per pixel.
        let lights = &render.lights;
        row(
            ui,
            "Local lights",
            format!("{} drawn, {} culled, {} dropped", lights.drawn, lights.culled, lights.dropped),
        );
        let lists = if lights.drawn == 0 {
            "none: no lights drawn".to_owned()
        } else if lights.clusters > 0 {
            format!("{} clusters, up to {} lights each", lights.clusters, lights.list_capacity)
        } else {
            format!("per object, up to {} each, {} chosen again", lights.list_capacity, lights.object_lists)
        };
        row(ui, "Light lists", lists);
        debug_view(ui, &render.debug_view);
        row(
            ui,
            "Character rigs",
            format!("{} characters lit, {} ignored", render.characters, render.ignored_characters),
        );
        row(ui, "Shadows", shadow_text(&render.shadows));
        row(ui, "Ambient", ambient_text(&render.ambient));
        let graph = frame.graph.stats();
        row(
            ui,
            "Graph",
            format!(
                "{} passes, {} culled, {} command buffers",
                graph.passes, graph.culled, graph.command_buffers
            ),
        );
        row(ui, "Transients", format!("{} in {} pooled textures", graph.transients, graph.transient_slots));
        row(
            ui,
            "Texture pool",
            format!(
                "{} resident, {} created, {} evicted",
                graph.pool.textures, graph.pool.created, graph.pool.evicted
            ),
        );
        row(ui, "Input", device_name(frame.input_device, frame.gamepads));
        let adapter = &frame.gpu.caps().adapter;
        row(ui, "GPU", format!("{} ({})", adapter.name, adapter.backend));
    });

    ui.separator();
    assets(ui, frame);
    if let Some(status) = frame.hot_reload {
        ui.separator();
        hot_reload(ui, frame, &status);
    }
    if let Some(status) = frame.live_link {
        ui.separator();
        live_link(ui, &status);
    }
}

/// Live link's state and its last refusals (M1 A9).
fn live_link(ui: &mut Ui, status: &LiveLinkStatus<'_>) {
    egui::Grid::new("live link").num_columns(2).show(ui, |ui| {
        ui.label("Live link");
        ui.monospace(status.state);
        ui.end_row();
        ui.label("Pushes");
        ui.monospace(format!(
            "{} applied, {} refused, {} received, {} connected",
            status.applied,
            status.refused,
            format_bytes(status.received),
            status.connections
        ));
        ui.end_row();
    });
    for error in status.errors {
        ui.label(RichText::new(error).monospace().color(OVER));
    }
}

/// Hot reload's state and the last change's errors (M1 A8).
fn hot_reload(ui: &mut Ui, frame: &OverlayFrame<'_>, status: &HotReloadStatus<'_>) {
    egui::Grid::new("hot reload").num_columns(2).show(ui, |ui| {
        ui.label("Hot reload");
        ui.monospace(status.watching);
        ui.end_row();
        let (reloads, failures) =
            frame.assets.map_or((0, 0), |assets| (assets.reloads, assets.reload_failures));
        ui.label("Reloaded");
        ui.monospace(format!(
            "{reloads} assets ({failures} failed), {} shader rebuilds, {} documents",
            frame.render.shader_rebuilds, status.documents
        ));
        ui.end_row();
    });
    for error in status.errors {
        ui.label(RichText::new(error).monospace().color(OVER));
    }
}

/// The asset server's counters and budgets (M1 A4).
fn assets(ui: &mut Ui, frame: &OverlayFrame<'_>) {
    egui::Grid::new("assets").num_columns(2).show(ui, |ui| {
        let row = |ui: &mut Ui, name: &str, value: String| {
            ui.label(name);
            ui.monospace(value);
            ui.end_row();
        };
        let Some(assets) = frame.assets else {
            row(ui, "Assets", "none: the app runs no asset server".to_owned());
            return;
        };
        row(
            ui,
            "Assets",
            format!(
                "{} ({} ready, {} loading, {} waiting, {} failed), {} unused, {} pinned",
                assets.assets,
                assets.ready,
                assets.loading,
                assets.waiting,
                assets.failed,
                assets.unused,
                assets.pinned
            ),
        );
        for kind in AssetKind::ALL {
            let (resident, budget) = (assets.resident[kind.index()], assets.budgets.resident(kind));
            let over = if resident > budget { ", over budget" } else { "" };
            row(
                ui,
                &format!("  {kind}s"),
                format!("{} of {}{over}", format_bytes(resident), format_bytes(budget)),
            );
        }
        let upload = assets.last_upload;
        row(
            ui,
            "Uploads",
            format!(
                "{} ({}) of {} a frame, {} queued, {} reads",
                upload.assets,
                format_bytes(upload.bytes),
                format_bytes(assets.budgets.upload_per_frame),
                upload.pending,
                assets.reads_in_flight
            ),
        );
        row(
            ui,
            "Since start",
            format!("{} loads, {} unloads, {} failures", assets.loads, assets.unloads, assets.failures),
        );
    });
}

/// The quality tier, the render scale and dynamic resolution (M1 R6).
fn quality(ui: &mut Ui, frame: &OverlayFrame<'_>) {
    egui::Grid::new("quality").num_columns(2).show(ui, |ui| {
        let row = |ui: &mut Ui, name: &str, value: String| {
            ui.label(name);
            ui.monospace(value);
            ui.end_row();
        };
        let Some(quality) = frame.quality else {
            row(ui, "Quality tier", "none: the app runs no tiers".to_owned());
            return;
        };
        let source = match quality.tier.source {
            TierSource::Device => "the device's",
            TierSource::Setting => "the player's",
            TierSource::Cvar => "r.tier",
        };
        row(ui, "Quality tier", format!("{} ({source})", quality.tier.id));
        if quality.tier.source != TierSource::Device {
            row(ui, "  device's tier", quality.device_tier.to_string());
        }
        let (width, height) = frame.render.scene_size;
        row(
            ui,
            "Render scale",
            format!("{:.2} of base {:.2}: {width}×{height}", quality.render_scale, quality.base_scale),
        );
        let dynamic = quality.dynamic;
        let state = match (dynamic.enabled, dynamic.signal, dynamic.estimate) {
            (false, ..) => "off (r.dynamic_resolution)".to_owned(),
            (true, Some(signal), Some(estimate)) => {
                let signal = match signal {
                    ResolutionSignal::GpuTime => "GPU",
                    ResolutionSignal::FrameTime => "frame",
                };
                format!(
                    "{signal} {} in {}–{}, ×{:.2} (floor {:.2}), {} changes",
                    format_ms(estimate),
                    format_ms(dynamic.low),
                    format_ms(dynamic.high),
                    dynamic.factor,
                    dynamic.floor,
                    dynamic.changes
                )
            }
            (true, ..) => format!("on, waiting for timings; ×{:.2}", dynamic.factor),
        };
        row(ui, "Dynamic resolution", state);
    });
}

/// How the platform paces frames against the tier's rate, and how evenly they have come (M1 F7).
fn pacing(ui: &mut Ui, frame: &OverlayFrame<'_>) {
    let Some(pacing) = frame.pacing else { return };
    egui::Grid::new("pacing").num_columns(2).show(ui, |ui| {
        let row = |ui: &mut Ui, name: &str, value: String| {
            ui.label(name);
            ui.monospace(value);
            ui.end_row();
        };
        let display = pacing.refresh_hz.map_or_else(String::new, |hz| format!(" on a {hz:.0} Hz display"));
        let how = match (pacing.source, pacing.target) {
            (PacingSource::DisplayLink, Some(rate)) => format!("display link at {rate} fps{display}"),
            (PacingSource::DisplayLink, None) => format!("display link at the display's rate{display}"),
            (PacingSource::Presentation, Some(rate)) => {
                format!("presentation (FIFO), against {rate} fps{display}")
            }
            (PacingSource::Presentation, None) => format!("presentation (FIFO){display}"),
        };
        row(ui, "Frame pacing", how);
        let stats = pacing.stats;
        let held = match pacing.source {
            PacingSource::DisplayLink => format!(", {} ticks held", stats.held),
            PacingSource::Presentation => String::new(),
        };
        row(
            ui,
            "  frames",
            format!(
                "{}: {} on time, {} missed, {} doubled{held}",
                stats.frames, stats.on_time, stats.missed, stats.doubled
            ),
        );
        row(ui, "  last interval", format_ms(pacing.interval));
    });
}

/// The sky's layers drawn this frame and its cube (M1 W2).
/// The shadows' counters (M1 R4) on one line.
fn shadow_text(shadows: &sapphire_render::ShadowStats) -> String {
    if !shadows.enabled {
        return "off".to_owned();
    }
    format!(
        "{} cascades of {}² ({} drawn, {} cached), {} character maps, {} capsules{}, {} caster draws ({} culled), {} light steps",
        shadows.cascades,
        shadows.resolution,
        shadows.cascades_drawn,
        shadows.cascades_cached,
        shadows.character_maps,
        shadows.capsules,
        if shadows.uncast_characters > 0 {
            format!(", {} uncast", shadows.uncast_characters)
        } else {
            String::new()
        },
        shadows.caster_draws,
        shadows.culled_draws,
        shadows.light_steps,
    )
}

/// The ambient probe grid, ambient occlusion and the reflection cube (M1 W5). The grid itself
/// is a readout here; drawing its probes waits for a probe debug view (`r.debug_view`'s later
/// views).
fn ambient_text(ambient: &sapphire_render::AmbientStats) -> String {
    let grid = if ambient.grid {
        format!(
            "grid: {} probes in {} of {} bricks, {}, {} keys ({} blended), {} bricks relit",
            ambient.probes,
            ambient.bricks,
            ambient.brick_positions,
            format_bytes(ambient.grid_bytes),
            ambient.keys,
            ambient.blended_keys,
            ambient.relit_bricks
        )
    } else {
        "no grid: the ambient hemisphere".to_owned()
    };
    let mut occlusion = Vec::new();
    if ambient.capsule_ao {
        occlusion.push(format!("{} capsules", ambient.capsules));
    }
    if ambient.gtao {
        let (width, height) = ambient.gtao_size;
        occlusion.push(format!("GTAO {width}×{height}"));
    }
    let occlusion =
        if occlusion.is_empty() { "AO off".to_owned() } else { format!("AO: {}", occlusion.join(", ")) };
    let reflections = if ambient.reflection_size > 0 {
        format!(
            "; reflections {}² in {} mips ({} faces filtered)",
            ambient.reflection_size, ambient.reflection_mips, ambient.prefiltered_faces
        )
    } else {
        String::new()
    };
    // The pass runs only with a grid or GTAO, and builds its pipelines only for them.
    let pass = if ambient.pipelines > 0 {
        format!("; pass: {} dispatches, {} pipelines", ambient.dispatches, ambient.pipelines)
    } else {
        "; no pass".to_owned()
    };
    format!("{grid}; {occlusion}{reflections}{pass}")
}

fn sky_text(sky: &sapphire_render::SkyStats) -> String {
    let layers: Vec<&str> = [(sky.stars, "stars"), (sky.clouds, "clouds"), (sky.cirrus, "cirrus")]
        .into_iter()
        .filter_map(|(drawn, name)| drawn.then_some(name))
        .collect();
    let layers = if layers.is_empty() { "gradient and discs".to_owned() } else { layers.join(", ") };
    let face = sky.last_face.map_or_else(|| "none".to_owned(), |face| face.to_string());
    format!("{layers}; cube {}², face {face} ({} drawn)", sky.cube_size, sky.cube_faces)
}

/// The fog's layers and its cards this frame (M1 W3).
fn fog_text(fog: &sapphire_render::FogStats) -> String {
    let layers = match fog.layers {
        0 => "off".to_owned(),
        1 => "1 layer".to_owned(),
        n => format!("{n} layers"),
    };
    let plural = |n: u32| if n == 1 { "" } else { "s" };
    format!(
        "{layers}; {} card{} ({} pipeline{})",
        fog.cards,
        plural(fog.cards),
        fog.card_pipelines,
        plural(fog.card_pipelines)
    )
}

/// The compositing layers post drew this frame (M1 P2), and its pipelines: those built so far,
/// and those frames built on first use, each a stall a prewarm would have saved.
fn post_text(post: &sapphire_render::PostStats) -> String {
    let gradients = match post.gradient_layers {
        0 => None,
        1 => Some("1 gradient".to_owned()),
        n => Some(format!("{n} gradients")),
    };
    let layers: Vec<String> = [
        (post.diffusion, "diffusion"),
        (post.character_bloom, "character bloom"),
        (post.lens_flare, "lens flare"),
        (post.vignette_tint, "vignette tint"),
        (post.sharpening, "sharpening"),
    ]
    .into_iter()
    .filter(|&(drawn, _)| drawn)
    .map(|(_, name)| name.to_owned())
    .chain(gradients)
    .collect();
    let bloom = if post.bloom_chain { "bloom chain" } else { "no bloom chain" };
    let pipelines = match post.first_use_builds {
        0 => format!("{} pipelines", post.pipelines),
        late => format!("{} pipelines, {late} built on first use", post.pipelines),
    };
    if layers.is_empty() {
        format!("none (plain); {bloom}; {pipelines}")
    } else {
        format!("{}; {bloom}; {pipelines}", layers.join(", "))
    }
}

/// The screen effects post drew this frame and the effect stack's depth (M1 P3).
fn effects_text(
    drawn: &sapphire_render::DrawnEffects,
    stack: Option<(
        &sapphire_world::screen_effects::EffectStack,
        &sapphire_world::screen_effects::EffectLibrary,
    )>,
) -> String {
    let effects: Vec<&str> = [
        (drawn.radial_blur, "radial blur"),
        (drawn.chromatic_aberration, "chromatic aberration"),
        (drawn.inversion, "inversion"),
        (drawn.duotone, "duotone"),
        (drawn.speed_lines, "speed lines"),
    ]
    .into_iter()
    .filter_map(|(on, name)| on.then_some(name))
    .collect();
    let drawn_text = if effects.is_empty() {
        "none".to_owned()
    } else {
        let mask = if drawn.character_mask { ", by the character mask" } else { "" };
        format!("{}{mask}", effects.join(", "))
    };
    match stack {
        Some((stack, _)) => format!("{drawn_text}; {} layers playing", stack.stats().depth),
        None => drawn_text,
    }
}

/// The renderer's debug view (`r.debug_view`, M1 F5): the one shown with its legend's values
/// (the bar the view draws in the frame's corner has no numbers), or none, with the pipelines
/// the views built.
fn debug_view(ui: &mut Ui, stats: &DebugViewStats) {
    // A display (sRGB-encoded) color, as egui takes it.
    let swatch = |ui: &mut Ui, color: [f32; 3]| {
        let (rect, _) = ui.allocate_exact_size(vec2(10.0, 10.0), Sense::hover());
        let [r, g, b] = color.map(|channel| (channel.clamp(0.0, 1.0) * 255.0).round() as u8);
        ui.painter().rect_filled(rect, 1.0, Color32::from_rgb(r, g, b));
    };
    ui.label("Debug view");
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 3.0;
        match stats.view {
            DebugView::Off if stats.unsupported => {
                ui.label(
                    RichText::new("wireframe unsupported: its line indices pass one storage binding")
                        .monospace()
                        .color(WARN),
                );
            }
            DebugView::Off if stats.pipelines == 0 => {
                ui.monospace("off");
            }
            DebugView::Off => {
                ui.monospace(format!("off ({} pipelines kept)", stats.pipelines));
            }
            DebugView::Wireframe => {
                ui.monospace("wireframe:");
                for (family, tone) in WIREFRAME_TONES {
                    swatch(ui, tone);
                    ui.monospace(family);
                }
            }
            DebugView::Overdraw => {
                ui.monospace("fragments per pixel:");
                for fragments in [1, 2, 4, 8, OVERDRAW_SCALE] {
                    swatch(ui, heat(overdraw_position(fragments)));
                    ui.monospace(fragments.to_string());
                }
                swatch(ui, [1.0; 3]);
                ui.monospace("more");
            }
            DebugView::Lights => {
                ui.monospace("lights per pixel:");
                // None (grey, as the view's legend starts), then powers of two up to a full list.
                swatch(ui, NO_LIGHTS);
                ui.monospace("0");
                let counts = std::iter::successors(Some(1), |count| Some(count * 2))
                    .take_while(|&count| count < stats.scale);
                for count in counts.chain([stats.scale]).filter(|&count| count > 0) {
                    swatch(ui, heat(light_position(count, stats.scale)));
                    ui.monospace(count.to_string());
                }
            }
            DebugView::MaterialIds => {
                ui.monospace("regions:");
                for region in 0..stats.scale {
                    swatch(ui, region_color(region));
                    ui.monospace(region.to_string());
                }
            }
            DebugView::RampRows => {
                // The day rows, then the night rows (the material IDs' own by default), as the
                // view's legend runs; time of day's blend of the two shows as stripes.
                let ids = stats.scale / 2;
                for (half, rows) in [("day rows:", 0..ids), ("night:", ids..stats.scale)] {
                    ui.monospace(half);
                    for row in rows {
                        swatch(ui, ramp_row_color(row));
                    }
                }
                for (tone, bands) in BAND_TONES.into_iter().zip(["2 bands", "3 bands"]) {
                    swatch(ui, tone);
                    ui.monospace(bands);
                }
            }
            DebugView::OutlineWidths => {
                ui.monospace("outline px at 1080 rows:");
                swatch(ui, CHARACTER_TONE);
                ui.monospace("surface");
                for width in [0, 1, 2, 4, OUTLINE_WIDTH_SCALE] {
                    swatch(ui, outline_width_color(width as f32));
                    ui.monospace(width.to_string());
                }
                swatch(ui, [1.0; 3]);
                ui.monospace("wider");
            }
        }
    });
    ui.end_row();
}

/// A row whose budget spec §3 leaves without a figure on this tier ("—"): measured, never
/// colored.
fn unbudgeted_row(ui: &mut Ui, name: &str, used: Duration) {
    ui.label(name);
    ui.monospace(format_ms(used));
    ui.monospace("—");
    ui.end_row();
}

fn budget_row(ui: &mut Ui, name: &str, used: Duration, budget: Duration) {
    ui.label(name);
    let color = budget_color(budget::budget_use(used, budget));
    ui.label(RichText::new(format_ms(used)).monospace().color(color));
    ui.monospace(if budget.is_zero() { "—".to_owned() } else { format_ms(budget) });
    ui.end_row();
}

/// The light-cluster pass's name in the frame graph (`sapphire_render`'s renderer).
const CLUSTER_PASS: &str = "light clusters";

/// Green within three quarters of the budget, amber up to it, red over.
fn budget_color(fraction: f32) -> Color32 {
    // A 1% grace: a vsync-paced frame lands on its budget give or take timer jitter.
    if fraction > 1.01 {
        OVER
    } else if fraction > 0.75 {
        WARN
    } else {
        GOOD
    }
}

/// The last [`HISTORY`] frames as bars, newest on the right, against the frame budget.
fn frame_time_graph(ui: &mut Ui, history: &FrameTimes, budget: Duration) {
    let (response, painter) = ui.allocate_painter(vec2(ui.available_width(), 56.0), Sense::hover());
    let rect = response.rect;
    painter.rect_filled(rect, 2.0, Color32::from_black_alpha(140));
    let budget = budget.as_secs_f32();
    // Twice the budget fits; slower frames clip at the top.
    let top = 2.0 * budget;
    let y = |seconds: f32| rect.bottom() - (seconds / top).min(1.0) * rect.height();
    let width = rect.width() / HISTORY as f32;
    let start = rect.right() - history.len() as f32 * width;
    for (index, seconds) in history.iter().enumerate() {
        let x = start + (index as f32 + 0.5) * width;
        let color = if seconds > budget { OVER } else { GOOD.gamma_multiply(0.8) };
        painter
            .line_segment([pos2(x, rect.bottom()), pos2(x, y(seconds))], Stroke::new(width.max(1.0), color));
    }
    painter.hline(rect.x_range(), y(budget), Stroke::new(1.0, WARN.gamma_multiply(0.8)));
    painter.text(
        rect.left_top() + vec2(4.0, 2.0),
        Align2::LEFT_TOP,
        format!("{} budget", format_ms(Duration::from_secs_f32(budget))),
        FontId::monospace(10.0),
        ui.visuals().weak_text_color(),
    );
}

fn gpu_timing_state(view: &View<'_>) -> &'static str {
    if !view.frame.gpu.caps().timestamp_query {
        "unavailable: the device has no timestamp queries"
    } else if !GPU_TIMING.get() {
        "off (debug.gpu_timing)"
    } else if view.gpu_timing {
        "waiting for timestamps"
    } else {
        "unavailable"
    }
}

fn device_name(device: Option<DeviceKind>, gamepads: usize) -> String {
    let device = match device {
        None => "none yet",
        Some(DeviceKind::KeyboardMouse) => "keyboard and mouse",
        Some(DeviceKind::Touch) => "touch",
        Some(DeviceKind::Gamepad(GamepadFamily::Xbox)) => "gamepad (Xbox)",
        Some(DeviceKind::Gamepad(GamepadFamily::PlayStation)) => "gamepad (PlayStation)",
        Some(DeviceKind::Gamepad(GamepadFamily::Nintendo)) => "gamepad (Nintendo)",
        Some(DeviceKind::Gamepad(GamepadFamily::Generic)) => "gamepad",
    };
    format!("last used: {device}; {gamepads} gamepads")
}

fn graph(ui: &mut Ui, view: &View<'_>) {
    let executor = view.frame.graph;
    let stats = executor.stats();
    ui.label(format!(
        "{} passes ran, {} culled, recorded into {} command buffers.",
        stats.passes, stats.culled, stats.command_buffers
    ));
    let timings = executor.gpu_timings();
    match timings {
        Some(timings) => {
            let age = executor.frames().saturating_sub(timings.frame);
            let frames = if age == 1 { "frame" } else { "frames" };
            let how =
                if timings.every_pass { "every pass" } else { "passes that take render-pass timestamps" };
            ui.weak(format!("GPU times from {age} {frames} ago, timing {how}."));
        }
        None => {
            ui.weak(format!("GPU times: {}.", gpu_timing_state(view)));
        }
    }
    let Some(report) = executor.report() else {
        ui.weak("The report starts with the next frame.");
        return;
    };
    egui::ScrollArea::vertical().max_height(360.0).auto_shrink([false, true]).show(ui, |ui| {
        egui::Grid::new("passes").num_columns(5).striped(true).show(ui, |ui| {
            for heading in ["#", "Pass", "Buffer"] {
                ui.weak(heading);
            }
            ui.weak("GPU").on_hover_text(
                "What the pass added to the frame: its time after the passes before it ended. Tile-based \
                 GPUs start a pass while earlier ones still run, so its own start-to-end time would \
                 include waiting on them.",
            );
            ui.weak("Uses");
            ui.end_row();
            for pass in &report.passes {
                let Some(order) = pass.order else {
                    ui.weak("—");
                    ui.weak(RichText::new(pass.name).strikethrough());
                    ui.weak("culled");
                    ui.label("");
                    ui.weak(uses(report, pass));
                    ui.end_row();
                    continue;
                };
                ui.monospace(order.to_string());
                ui.label(if pass.side_effect {
                    format!("{} (side effect)", pass.name)
                } else {
                    pass.name.to_owned()
                });
                ui.monospace(pass.command_buffer.map_or_else(String::new, |buffer| buffer.to_string()));
                // What the pass added to the frame: on tile-based GPUs a pass's own start and end
                // also cover waiting on the passes before it.
                let gpu = timings.and_then(|timings| timings.get(pass.name)).map(|time| time.exclusive);
                ui.monospace(gpu.map_or_else(|| "—".to_owned(), format_ms));
                ui.weak(uses(report, pass));
                ui.end_row();
            }
        });
        egui::CollapsingHeader::new(format!("Textures ({})", report.textures.len())).default_open(true).show(
            ui,
            |ui| {
                egui::Grid::new("textures").num_columns(4).striped(true).show(ui, |ui| {
                    for heading in ["Texture", "Size, format", "Slot", "Passes"] {
                        ui.weak(heading);
                    }
                    ui.end_row();
                    for texture in &report.textures {
                        ui.label(texture.name);
                        ui.monospace(texture_format(texture));
                        ui.monospace(texture.slot.map_or_else(|| "—".to_owned(), |slot| slot.to_string()));
                        let lifetime = texture.lifetime.map(|(first, last)| format!("{first}–{last}"));
                        ui.monospace(lifetime.unwrap_or_else(|| "unused".to_owned()));
                        ui.end_row();
                    }
                });
            },
        );
    });
}

/// What a pass reads, then what it writes: `a, b -> c`. (egui's default fonts have no arrow.)
fn uses(report: &GraphReport, pass: &sapphire_render::graph::PassReport) -> String {
    let names = |write: bool| {
        let names: Vec<&str> = report
            .accesses(pass)
            .iter()
            .filter(|access| access.write == write)
            .map(|access| report.textures[access.texture as usize].name)
            .collect();
        names.join(", ")
    };
    match (names(false), names(true)) {
        // Side effects on buffers only, such as uploads: buffers aren't graph resources yet.
        (reads, writes) if reads.is_empty() && writes.is_empty() => String::new(),
        (reads, writes) if reads.is_empty() => format!("-> {writes}"),
        (reads, writes) if writes.is_empty() => format!("{reads} ->"),
        (reads, writes) => format!("{reads} -> {writes}"),
    }
}

fn texture_format(texture: &TextureReport) -> String {
    let (width, height) = texture.size;
    match texture.transient {
        None => format!("{width}×{height} imported"),
        Some(desc) if desc.sample_count > 1 => {
            format!("{width}×{height} {:?} ×{}", desc.format, desc.sample_count)
        }
        Some(desc) => format!("{width}×{height} {:?}", desc.format),
    }
}

fn time_controls(ui: &mut Ui, time: &mut TimeControls, clock: &ScaledClock) {
    ui.label(format!(
        "World {:.2} s, tick {}, speed {:.3}×",
        clock.now().as_secs_f64(),
        clock.ticks(),
        clock.effective_scale()
    ));
    ui.horizontal(|ui| {
        let paused = time.is_paused();
        if ui.button(if paused { "Resume" } else { "Pause" }).clicked() {
            time.set_paused(!paused);
        }
        if ui.button("Step").on_hover_text("Run one fixed step of world time").clicked() {
            time.step(1);
        }
        if ui.button("Step 10").clicked() {
            time.step(10);
        }
    });
    ui.horizontal_wrapped(|ui| {
        ui.label("Speed");
        for speed in SPEEDS {
            let label = format!("{}×", f64::from(speed) / f64::from(NORMAL_SPEED));
            if ui.selectable_label(time.speed() == speed, label).clicked() {
                time.set_speed(speed);
            }
        }
    });
    let scales: Vec<String> = clock
        .scales()
        .map(|scale| format!("{:?} {}×", scale.source, f64::from(scale.per_mille) / f64::from(NORMAL_SPEED)))
        .collect();
    ui.weak(if scales.is_empty() {
        "No time scales pushed.".to_owned()
    } else {
        format!("Scales: {}", scales.join(", "))
    });
    if time.stack_full() {
        ui.colored_label(OVER, "The world clock's scale stack is full: a scale was pushed and never popped.");
    }
}

fn cvars(ui: &mut Ui, cvars: &CvarRegistry) {
    if cvars.is_empty() {
        ui.weak("No cvars are registered.");
        return;
    }
    egui::ScrollArea::vertical().max_height(420.0).show(ui, |ui| {
        egui::Grid::new("cvars").num_columns(3).striped(true).show(ui, |ui| {
            for cvar in cvars.iter() {
                ui.label(cvar.name()).on_hover_text(cvar.help());
                cvar_widget(ui, cvar);
                if cvar.is_default() {
                    ui.label("");
                } else if ui
                    .small_button("reset")
                    .on_hover_text(format!("Default: {}", cvar.default_value()))
                    .clicked()
                {
                    cvar.reset();
                }
                ui.end_row();
            }
        });
    });
}

/// The widget that edits `cvar`: a checkbox, a slider for a ranged number, a drag value, or a
/// list of choices.
fn cvar_widget(ui: &mut Ui, cvar: &dyn AnyCvar) {
    let range = cvar.range();
    let changed = match cvar.value() {
        CvarValue::Bool(mut value) => ui.checkbox(&mut value, "").changed().then_some(CvarValue::Bool(value)),
        CvarValue::Int(mut value) => {
            let response = match range {
                Some((CvarValue::Int(min), CvarValue::Int(max))) => {
                    ui.add(egui::Slider::new(&mut value, min..=max))
                }
                _ => ui.add(egui::DragValue::new(&mut value)),
            };
            response.changed().then_some(CvarValue::Int(value))
        }
        CvarValue::UInt(mut value) => {
            let response = match range {
                Some((CvarValue::UInt(min), CvarValue::UInt(max))) => {
                    ui.add(egui::Slider::new(&mut value, min..=max))
                }
                _ => ui.add(egui::DragValue::new(&mut value)),
            };
            response.changed().then_some(CvarValue::UInt(value))
        }
        CvarValue::Float(mut value) => {
            let response = match range {
                Some((CvarValue::Float(min), CvarValue::Float(max))) => {
                    ui.add(egui::Slider::new(&mut value, min..=max))
                }
                _ => ui.add(egui::DragValue::new(&mut value).speed(0.01)),
            };
            response.changed().then_some(CvarValue::Float(value))
        }
        CvarValue::Choice { index, names } => {
            let mut chosen = index;
            egui::ComboBox::from_id_salt(cvar.name())
                .selected_text(names.get(index as usize).copied().unwrap_or("?"))
                .show_ui(ui, |ui| {
                    for (choice, name) in (0u32..).zip(names) {
                        ui.selectable_value(&mut chosen, choice, *name);
                    }
                });
            (chosen != index).then_some(CvarValue::Choice { index: chosen, names })
        }
    };
    if let Some(value) = changed
        && let Err(error) = cvar.set_value(value)
    {
        tracing::warn!(target: crate::LOG_TARGET, %error, "cvar not set");
    }
}

fn console_panel(
    ui: &mut Ui,
    console: &mut Console,
    input: &mut String,
    focus: &mut bool,
    cvars: &CvarRegistry,
) {
    let line_height = ui.text_style_height(&egui::TextStyle::Monospace);
    egui::ScrollArea::vertical()
        .max_height((ui.available_height() - 2.0 * line_height).max(4.0 * line_height))
        .auto_shrink([false, false])
        .stick_to_bottom(true)
        .show(ui, |ui| {
            for line in console.output() {
                let color = match line.kind {
                    LineKind::Command => ui.visuals().weak_text_color(),
                    LineKind::Info => ui.visuals().text_color(),
                    LineKind::Error => OVER,
                };
                ui.label(RichText::new(&line.text).monospace().color(color));
            }
        });
    let id = Id::unique("sapphire_debug.console.input");
    // Enter, Tab, Up and Down belong to the console while it has focus, not to egui's focus
    // moves. The field keeps its focus on Enter, so text input (and a phone's keyboard) stays
    // on between commands.
    let mut submit = false;
    if ui.memory(|memory| memory.has_focus(id)) {
        let (enter, tab, up, down) = ui.input_mut(|input| {
            (
                input.consume_key(egui::Modifiers::NONE, egui::Key::Enter),
                input.consume_key(egui::Modifiers::NONE, egui::Key::Tab),
                input.consume_key(egui::Modifiers::NONE, egui::Key::ArrowUp),
                input.consume_key(egui::Modifiers::NONE, egui::Key::ArrowDown),
            )
        });
        submit = enter;
        if tab {
            match console.complete(input, cvars) {
                Completion::Extended(completed) => *input = completed,
                Completion::Candidates(candidates) => console.print_candidates(&candidates),
                Completion::None => {}
            }
        }
        if up || down {
            *input = console.recall(up).unwrap_or_default().to_owned();
        }
    }
    let response = ui.add(
        egui::TextEdit::singleline(input)
            .id(id)
            .font(egui::TextStyle::Monospace)
            .hint_text("help, list, <cvar> <value>")
            .desired_width(f32::INFINITY)
            .return_key(None)
            .lock_focus(true),
    );
    if submit {
        console.execute(input, cvars);
        input.clear();
    }
    if std::mem::take(focus) {
        response.request_focus();
    }
}
