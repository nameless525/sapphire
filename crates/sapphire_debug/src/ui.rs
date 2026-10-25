//! The overlay's panels, in egui: a bar of toggles, then stats and budgets with the frame-time
//! graph, the frame graph, time controls, the console and the cvar list.

use std::time::Duration;

use egui::{Align2, Color32, FontId, Id, Order, RichText, Sense, Stroke, Ui, pos2, vec2};
use sapphire_core::cvar::{AnyCvar, CvarRegistry, CvarValue};
use sapphire_input::{DeviceKind, GamepadFamily};
use sapphire_render::graph::{GraphReport, TextureReport};
use sapphire_render::quality::{ResolutionSignal, TierSource};
use sapphire_time::{NORMAL_SPEED, ScaledClock};

use crate::budget::{self, BudgetTier, CpuStage, CpuStages, format_bytes, format_ms};
use crate::console::{Completion, Console, LineKind};
use crate::history::{FrameTimes, HISTORY};
use crate::overlay::{OverlayCost, OverlayFrame};
use crate::time::{SPEEDS, TimeControls};
use crate::{GPU_TIMING, route};

const GOOD: Color32 = Color32::from_rgb(120, 200, 130);
const WARN: Color32 = Color32::from_rgb(230, 190, 70);
const OVER: Color32 = Color32::from_rgb(240, 95, 85);

/// Which panels are open.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Panels {
    pub stats: bool,
    pub graph: bool,
    pub time: bool,
    pub console: bool,
    pub cvars: bool,
}

impl Default for Panels {
    /// Stats and the frame graph.
    fn default() -> Self {
        Self { stats: true, graph: true, time: false, console: false, cvars: false }
    }
}

impl Panels {
    pub const NAMES: [&'static str; 5] = ["stats", "graph", "time", "console", "cvars"];

    /// A comma-separated list of [`Self::NAMES`], or `all`.
    pub fn parse(list: &str) -> Result<Self, String> {
        let mut panels = Self { stats: false, graph: false, time: false, console: false, cvars: false };
        for name in list.split(',').map(str::trim) {
            match name {
                "all" => {
                    panels = Self { stats: true, graph: true, time: true, console: true, cvars: true };
                }
                "stats" => panels.stats = true,
                "graph" => panels.graph = true,
                "time" => panels.time = true,
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
}

impl UiMemory {
    pub fn new(panels: Panels) -> Self {
        Self { panels, console: Console::default(), console_input: String::new(), focus_console: true }
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
    view: &View<'_>,
) -> bool {
    let hide = bar(ctx, &mut memory.panels, view);
    // First placement, laid out for screens 1280 points wide or more (windows remember where
    // they were moved): stats on the left, the frame graph on the right, time controls between
    // them with the console under them, and the cvars under the frame graph.
    let safe = view.safe;
    let at = |x: f32, y: f32| safe.min + vec2(x, y);
    let right = |width: f32, y: f32| pos2(safe.max.x - width - 6.0, safe.min.y + y).max(at(440.0, y));
    let UiMemory { panels, console, console_input, focus_console } = memory;
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
            Some(timings) => budget_row(ui, "GPU frame (pass span)", timings.span(), tier.gpu_frame()),
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
        row(ui, "Draws", format!("{} per mesh pass, {} instances", render.draws, render.instances));
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
        row(ui, "Buffer growths", render.buffer_growths.to_string());
        row(ui, "LUT bakes", render.lut_bakes.to_string());
        // The cluster counts here are what the light-cluster heatmap view will show per pixel.
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

fn budget_row(ui: &mut Ui, name: &str, used: Duration, budget: Duration) {
    ui.label(name);
    let color = budget_color(budget::budget_use(used, budget));
    ui.label(RichText::new(format_ms(used)).monospace().color(color));
    ui.monospace(if budget.is_zero() { "—".to_owned() } else { format_ms(budget) });
    ui.end_row();
}

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
