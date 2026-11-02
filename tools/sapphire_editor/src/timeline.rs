//! The timeline: a move's tracks in rows, in cells of one 60 Hz tick, and what dragging them does.
//!
//! A cell is a tick: tick `t` covers `x(t)` to `x(t + 1)`, so a window `7..11` fills cells 7 to 10, and the
//! playhead's column inside it means the window is open on that tick. What's drawn comes from the parsed
//! document, and what's dragged goes back through [`Action`]s the editor applies to the text. Nothing here
//! knows what a document may say: a window dragged past a move's end goes there, and the row shows the
//! document's own words about it ([`Problem`]).

use egui::{
    Align2, Color32, CornerRadius, FontId, Pos2, Rect, Response, Sense, Stroke, StrokeKind, Ui, Vec2,
    WidgetInfo,
};
use sapphire_moves::{MoveDocument, RootSample};

use crate::edits::{EntryRef, Phases, TickSpan, Track};

/// The width of the row titles.
const TITLES: f32 = 124.0;
const RULER: f32 = 24.0;
const LANE: f32 = 21.0;
const ROW_GAP: f32 = 3.0;
const PROBLEM_LINE: f32 = 15.0;
/// How far a window's handle reaches outside it and in, so a window of one tick still has two.
const HANDLE_OUT: f32 = 3.0;
const HANDLE_IN: f32 = 5.0;
/// The least a tick is wide, past which the row scrolls rather than squeezes.
const MIN_TICK: f32 = 3.0;
const ROOT_ROW: f32 = 30.0;

/// One thing wrong with the document, as the document or a device said it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Problem {
    pub in_move: Option<String>,
    pub track: Option<String>,
    pub message: String,
    /// A device refused it, and this is its answer rather than the editor's own check.
    pub from_device: bool,
}

impl Problem {
    /// How it reads anywhere it's not beside its track.
    #[must_use]
    pub fn describe(&self) -> String {
        match (&self.in_move, &self.track) {
            (Some(mv), Some(track)) => format!("move `{mv}`, track `{track}`: {}", self.message),
            (None, Some(track)) => format!("`{track}`: {}", self.message),
            _ => self.message.clone(),
        }
    }
}

/// Ticks to pixels.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TickScale {
    pub left: f32,
    pub per_tick: f32,
}

impl TickScale {
    /// Fits `ticks` into `width` pixels from `left`.
    #[must_use]
    pub fn fit(left: f32, width: f32, ticks: u32) -> Self {
        Self { left, per_tick: (width / ticks.max(1) as f32).max(MIN_TICK) }
    }

    /// Where tick `tick`'s cell starts.
    #[must_use]
    pub fn x(&self, tick: u32) -> f32 {
        self.left + tick as f32 * self.per_tick
    }

    /// The cell at `x`.
    #[must_use]
    pub fn cell(&self, x: f32) -> u32 {
        ((x - self.left) / self.per_tick).floor().max(0.0) as u32
    }

    /// The edge between cells nearest `x`: where a window's end lands.
    #[must_use]
    pub fn edge(&self, x: f32) -> u32 {
        ((x - self.left) / self.per_tick).round().max(0.0) as u32
    }
}

/// An entry on a row.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RowEntry {
    pub index: usize,
    pub span: TickSpan,
    pub lane: usize,
    pub label: String,
}

/// A row of the timeline.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Row {
    pub track: Track,
    pub entries: Vec<RowEntry>,
    pub lanes: usize,
}

/// A track's name on the timeline.
#[must_use]
pub const fn title(track: Track) -> &'static str {
    match track {
        Track::Hitboxes => "hit volumes",
        Track::Hurtboxes => "hurtboxes",
        Track::Invulnerable => "invulnerable",
        Track::PerfectDodge => "perfect dodge",
        Track::Parry => "parry",
        Track::Armour => "armour",
        Track::Cancels => "cancels",
        Track::Warps => "warp",
        Track::Cues => "cues",
        Track::RootKeys => "root keys",
    }
}

fn color(track: Track) -> Color32 {
    match track {
        Track::Hitboxes => Color32::from_rgb(214, 72, 72),
        Track::Hurtboxes => Color32::from_rgb(92, 170, 92),
        Track::Invulnerable => Color32::from_rgb(120, 128, 150),
        Track::PerfectDodge => Color32::from_rgb(190, 90, 190),
        Track::Parry => Color32::from_rgb(200, 180, 70),
        Track::Armour => Color32::from_rgb(205, 130, 60),
        Track::Cancels => Color32::from_rgb(58, 150, 150),
        Track::Warps => Color32::from_rgb(70, 130, 205),
        Track::Cues => Color32::from_rgb(225, 225, 225),
        Track::RootKeys => Color32::from_rgb(120, 170, 235),
    }
}

/// What an entry says of itself on its row.
fn label_of(mv: &MoveDocument, track: Track, index: usize) -> String {
    let allow = |list: &[sapphire_moves::AllowDocument]| {
        let names: Vec<String> = list
            .iter()
            .map(|allow| {
                if allow.intent == allow.target {
                    allow.target.clone()
                } else {
                    format!("{} -> {}", allow.intent, allow.target)
                }
            })
            .collect();
        names.join(", ")
    };
    match track {
        Track::Hitboxes => mv.hitboxes[index].name.clone(),
        Track::Hurtboxes => mv.hurtboxes[index].name.clone(),
        Track::Invulnerable | Track::Parry | Track::RootKeys => String::new(),
        Track::PerfectDodge => {
            let window = &mv.perfect_dodge[index];
            if window.volume.is_some() { "fires".to_owned() } else { String::new() }
        }
        Track::Armour => format!("below {}", mv.armour[index].threshold),
        Track::Cancels => allow(&mv.cancels[index].allow),
        Track::Warps => {
            let warp = &mv.warps[index];
            format!("{} m, {} rad", warp.max_distance, warp.max_turn)
        }
        Track::Cues => mv.cues[index].name.clone(),
    }
}

/// A move's rows: the plan's tracks always, in one place, and the rest where the move uses them.
#[must_use]
pub fn rows_of(mv: &MoveDocument, show_all: bool, per_tick: f32) -> Vec<Row> {
    let always = [Track::Hitboxes, Track::Invulnerable, Track::Cancels, Track::Warps, Track::Cues];
    Track::ALL
        .into_iter()
        .filter(|track| show_all || always.contains(track) || !track.spans(mv).is_empty())
        .filter(|track| *track != Track::RootKeys || !track.spans(mv).is_empty())
        .map(|track| {
            let spans = track.spans(mv);
            let labels: Vec<String> = (0..spans.len()).map(|index| label_of(mv, track, index)).collect();
            let (lanes_of, lanes) = lanes(&spans, &labels, track.is_instant(), per_tick);
            let entries = spans
                .into_iter()
                .zip(labels)
                .zip(lanes_of)
                .enumerate()
                .map(|(index, ((span, label), lane))| RowEntry { index, span, lane, label })
                .collect();
            Row { track, entries, lanes: lanes.max(1) }
        })
        .collect()
}

/// Puts overlapping entries in lanes of their own, first come, first down. An instant counts its label,
/// which runs off to its right.
fn lanes(spans: &[TickSpan], labels: &[String], instant: bool, per_tick: f32) -> (Vec<usize>, usize) {
    const CHAR: f32 = 7.0;
    let mut order: Vec<usize> = (0..spans.len()).collect();
    order.sort_by_key(|index| (spans[*index].from, *index));
    let mut ends: Vec<f32> = Vec::new();
    let mut lane_of = vec![0; spans.len()];
    for index in order {
        let span = spans[index];
        let from = span.from as f32 * per_tick;
        let mut to = span.to.max(span.from + 1) as f32 * per_tick;
        if instant {
            to += labels[index].chars().count() as f32 * CHAR + 6.0;
        }
        let lane = ends.iter().position(|end| *end <= from).unwrap_or(ends.len());
        if lane == ends.len() {
            ends.push(to);
        } else {
            ends[lane] = to;
        }
        lane_of[index] = lane;
    }
    (lane_of, ends.len())
}

/// Which end of a phase a drag has.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PhaseEdge {
    StartupEnd,
    ActiveEnd,
    MoveEnd,
}

/// What's held.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Part {
    Start,
    End,
    Body,
}

/// A drag in progress: what it holds, and how things stood when it began.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Drag {
    Window { entry: EntryRef, part: Part, span: TickSpan, grab_x: f32, grab_edge_x: f32 },
    Instant { entry: EntryRef, grab_offset: f32 },
    Phase { mv: usize, edge: PhaseEdge, phases: Phases, grab_offset: f32 },
}

impl Drag {
    /// What this drag's held thing is now, with the pointer at `x`.
    #[must_use]
    pub fn at(&self, x: f32, scale: &TickScale) -> Option<Action> {
        match *self {
            Self::Window { entry, part, span, grab_x, grab_edge_x } => {
                let edge = scale.edge(grab_edge_x + (x - grab_x));
                let (from, to) = match part {
                    Part::Start => (edge, span.to),
                    Part::End => (span.from, edge),
                    Part::Body => {
                        let delta = ((x - grab_x) / scale.per_tick).round() as i64;
                        let from = (i64::from(span.from) + delta).max(0) as u32;
                        (from, from + (span.to - span.from.min(span.to)))
                    }
                };
                Some(Action::SetWindow { entry, from, to, part })
            }
            Self::Instant { entry, grab_offset } => {
                Some(Action::SetInstant { entry, at: scale.cell(x + grab_offset) })
            }
            Self::Phase { mv, edge, phases, grab_offset } => {
                let tick = scale.edge(x + grab_offset);
                let Phases { startup, active, recovery } = phases;
                let next = match edge {
                    PhaseEdge::StartupEnd => {
                        let moved = tick.min(startup + active);
                        Phases { startup: moved, active: startup + active - moved, recovery }
                    }
                    PhaseEdge::ActiveEnd => {
                        let moved = tick.clamp(startup, startup + active + recovery);
                        Phases {
                            startup,
                            active: moved - startup,
                            recovery: startup + active + recovery - moved,
                        }
                    }
                    PhaseEdge::MoveEnd => {
                        Phases { startup, active, recovery: tick.saturating_sub(startup + active) }
                    }
                };
                Some(Action::SetPhases { mv, phases: next, edge })
            }
        }
    }
}

/// What the timeline asks of the editor, applied after it has drawn.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Action {
    Scrub(u32),
    Select(Option<EntryRef>),
    BeginGesture(String),
    EndGesture,
    SetWindow { entry: EntryRef, from: u32, to: u32, part: Part },
    SetInstant { entry: EntryRef, at: u32 },
    SetPhases { mv: usize, phases: Phases, edge: PhaseEdge },
    Add(Track),
}

/// What the timeline draws.
#[derive(Debug)]
pub struct TimelineView<'a> {
    pub mv: &'a MoveDocument,
    pub mv_index: usize,
    pub tick: u32,
    pub selected: Option<EntryRef>,
    pub problems: &'a [Problem],
    /// The table's root, tick by tick, where the document has compiled.
    pub root: Option<&'a [RootSample]>,
    /// The ticks a hit landed on the dummy, in the preview's run.
    pub hit_ticks: &'a [u32],
    pub editable: bool,
    pub show_all: bool,
}

/// Draws the timeline, and says what it was asked. `drag` lasts between frames.
pub fn show(ui: &mut Ui, view: &TimelineView<'_>, drag: &mut Option<Drag>) -> Vec<Action> {
    let mut actions = Vec::new();
    let mv = view.mv;
    let ticks = mv.ticks();
    let width = ui.available_width();
    let scale = TickScale::fit(ui.max_rect().left() + TITLES, (width - TITLES - 10.0).max(40.0), ticks);
    let rows = rows_of(mv, view.show_all, scale.per_tick);

    let problems_of = |track: &str| -> Vec<&Problem> {
        view.problems
            .iter()
            .filter(|problem| {
                problem.in_move.as_deref() == Some(mv.name.as_str())
                    && problem.track.as_deref() == Some(track)
            })
            .collect()
    };
    let phase_problems = problems_of("phases");
    let motion_problems = problems_of("motion");
    let height_of =
        |lanes: usize, problems: usize| lanes as f32 * LANE + problems as f32 * PROBLEM_LINE + ROW_GAP;
    let mut total = RULER + height_of(1, phase_problems.len());
    total += ROOT_ROW + ROW_GAP + motion_problems.len() as f32 * PROBLEM_LINE;
    for row in &rows {
        if row.track != Track::RootKeys {
            total += height_of(row.lanes, problems_of(row.track.key()).len());
        } else {
            total += height_of(row.lanes, 0);
        }
    }

    let tracks_width = (ticks.max(1) as f32 * scale.per_tick + TITLES + 10.0).max(width);
    let (rect, background) = ui.allocate_exact_size(Vec2::new(tracks_width, total), Sense::click_and_drag());
    background.widget_info(|| WidgetInfo::labeled(egui::Role::Slider, true, "timeline ruler"));
    let scale = TickScale { left: rect.left() + TITLES, per_tick: scale.per_tick };
    let painter = ui.painter_at(rect);
    let right = scale.x(ticks);

    // Background: alternate ticks, so a cell reads as a cell.
    painter.rect_filled(rect, CornerRadius::ZERO, Color32::from_gray(26));
    for tick in 0..ticks {
        if tick % 2 == 1 {
            let cell = Rect::from_x_y_ranges(scale.x(tick)..=scale.x(tick + 1), rect.y_range());
            painter.rect_filled(cell, CornerRadius::ZERO, Color32::from_gray(31));
        }
    }

    // The ruler.
    let mut y = rect.top();
    let ruler = Rect::from_min_max(Pos2::new(rect.left(), y), Pos2::new(rect.right(), y + RULER));
    painter.rect_filled(ruler, CornerRadius::ZERO, Color32::from_gray(38));
    let every = if scale.per_tick >= 12.0 {
        5
    } else if scale.per_tick >= 5.0 {
        10
    } else {
        20
    };
    for tick in (0..=ticks).step_by(every as usize) {
        painter.text(
            Pos2::new(scale.x(tick) + 2.0, y + 4.0),
            Align2::LEFT_TOP,
            tick.to_string(),
            FontId::monospace(11.0),
            Color32::from_gray(190),
        );
        painter.line_segment(
            [Pos2::new(scale.x(tick), y), Pos2::new(scale.x(tick), rect.bottom())],
            Stroke::new(1.0, Color32::from_gray(58)),
        );
    }
    painter.line_segment(
        [Pos2::new(right, y), Pos2::new(right, rect.bottom())],
        Stroke::new(1.5, Color32::from_gray(150)),
    );
    y += RULER;

    // Phases.
    let phases = Phases::of(mv);
    let phase_row = Rect::from_min_size(Pos2::new(scale.left, y), Vec2::new(right - scale.left, LANE - 1.0));
    title_at(&painter, rect, y, "phases", !phase_problems.is_empty());
    let segments = [
        ("startup", phases.startup, 0, Color32::from_rgb(92, 106, 66)),
        ("active", phases.active, phases.startup, Color32::from_rgb(168, 92, 58)),
        ("recovery", phases.recovery, phases.startup + phases.active, Color32::from_rgb(64, 76, 104)),
    ];
    for (name, length, start, fill) in segments {
        if length == 0 {
            continue;
        }
        let segment = Rect::from_x_y_ranges(scale.x(start)..=scale.x(start + length), phase_row.y_range());
        painter.rect_filled(segment.shrink2(Vec2::new(0.5, 0.0)), CornerRadius::same(2), fill);
        let text = format!("{name} {length}");
        let clip = painter.with_clip_rect(segment.intersect(rect));
        clip.text(
            segment.left_center() + Vec2::new(4.0, 0.0),
            Align2::LEFT_CENTER,
            text,
            FontId::proportional(12.0),
            Color32::WHITE,
        );
    }
    if view.editable {
        let edges = [
            (PhaseEdge::StartupEnd, phases.startup, "startup end"),
            (PhaseEdge::ActiveEnd, phases.startup + phases.active, "active end"),
            (PhaseEdge::MoveEnd, ticks, "move end"),
        ];
        for (edge, tick, label) in edges {
            let x = scale.x(tick);
            let handle = Rect::from_x_y_ranges(x - 4.0..=x + 4.0, phase_row.y_range());
            let id = ui.scope_id().with(("phase edge", view.mv_index, label));
            let response = ui.interact(handle, id, Sense::click_and_drag());
            response.widget_info(|| {
                WidgetInfo::labeled(egui::Role::Slider, true, format!("phases {label} {tick}"))
            });
            if response.hovered() || response.dragged() {
                painter.rect_filled(
                    handle.shrink2(Vec2::new(2.5, 0.0)),
                    CornerRadius::same(1),
                    Color32::WHITE,
                );
            }
            if response.drag_started()
                && let Some(pressed) = pressed_x(&response)
            {
                *drag = Some(Drag::Phase { mv: view.mv_index, edge, phases, grab_offset: x - pressed });
                actions.push(Action::BeginGesture(format!("move {label} of {}", mv.name)));
            }
            follow_drag(&response, drag, &scale, &mut actions);
        }
    }
    y += LANE;
    y = problem_lines(&painter, &phase_problems, scale.left, y);
    y += ROW_GAP;

    // Root motion: how far each tick travels, from the table.
    title_at(&painter, rect, y, "root motion", !motion_problems.is_empty());
    root_row(&painter, view.root, &scale, y);
    y += ROOT_ROW;
    y = problem_lines(&painter, &motion_problems, scale.left, y);
    y += ROW_GAP;

    // Tracks.
    for row in &rows {
        let track_problems =
            if row.track == Track::RootKeys { Vec::new() } else { problems_of(row.track.key()) };
        title_at(&painter, rect, y, title(row.track), !track_problems.is_empty());
        if view.editable && row.track != Track::RootKeys {
            let button = Rect::from_center_size(
                Pos2::new(rect.left() + TITLES - 11.0, y + LANE * 0.5),
                Vec2::splat(15.0),
            );
            let id = ui.scope_id().with(("add to track", row.track));
            let response = ui.interact(button, id, Sense::click());
            let label = format!("add to {}", title(row.track));
            response.widget_info(|| WidgetInfo::labeled(egui::Role::Button, true, label.as_str()));
            let ink = if response.hovered() { Color32::WHITE } else { Color32::from_gray(140) };
            painter.text(button.center(), Align2::CENTER_CENTER, "+", FontId::monospace(14.0), ink);
            let hint = format!("Add to {} at the playhead", title(row.track));
            if response.on_hover_text(hint).clicked() {
                actions.push(Action::Add(row.track));
            }
        }
        for entry in &row.entries {
            let entry_ref = EntryRef { mv: view.mv_index, track: row.track, index: entry.index };
            let top = y + entry.lane as f32 * LANE;
            let band = Rect::from_x_y_ranges(
                scale.x(entry.span.from)..=scale.x(entry.span.to.max(entry.span.from)),
                top + 1.0..=top + LANE - 2.0,
            );
            draw_entry(
                ui,
                &painter,
                view,
                entry,
                entry_ref,
                band,
                &scale,
                drag,
                &mut actions,
                &track_problems,
            );
        }
        y += row.lanes as f32 * LANE;
        y = problem_lines(&painter, &track_problems, scale.left, y);
        y += ROW_GAP;
    }

    // What the run found: a mark on the ruler where a hit landed.
    for tick in view.hit_ticks {
        let at = Pos2::new(scale.x(*tick) + scale.per_tick * 0.5, rect.top() + RULER - 4.0);
        painter.circle_filled(at, 3.5, Color32::from_rgb(255, 210, 90));
    }

    // The playhead's column, over everything.
    let column = Rect::from_x_y_ranges(scale.x(view.tick)..=scale.x(view.tick + 1), rect.y_range());
    painter.rect_filled(column, CornerRadius::ZERO, Color32::from_white_alpha(26));
    painter.rect_stroke(
        column,
        CornerRadius::ZERO,
        Stroke::new(1.0, Color32::from_rgb(255, 220, 120)),
        StrokeKind::Inside,
    );

    // Pressing or dragging anywhere else scrubs.
    if (background.clicked() || background.dragged())
        && let Some(pointer) = background.interact_pointer_pos()
        && pointer.x >= scale.left
    {
        actions.push(Action::Scrub(scale.cell(pointer.x).min(ticks.saturating_sub(1))));
    }
    if background.clicked() {
        actions.push(Action::Select(None));
    }
    actions
}

fn title_at(painter: &egui::Painter, rect: Rect, y: f32, text: &str, trouble: bool) {
    let ink = if trouble { Color32::from_rgb(255, 120, 110) } else { Color32::from_gray(200) };
    painter.text(
        Pos2::new(rect.left() + 6.0, y + LANE * 0.5),
        Align2::LEFT_CENTER,
        text,
        FontId::proportional(12.5),
        ink,
    );
    if trouble {
        let stripe = Rect::from_min_size(Pos2::new(rect.left(), y), Vec2::new(3.0, LANE));
        painter.rect_filled(stripe, CornerRadius::ZERO, Color32::from_rgb(235, 80, 70));
    }
}

fn problem_lines(painter: &egui::Painter, problems: &[&Problem], x: f32, mut y: f32) -> f32 {
    for problem in problems {
        let prefix = if problem.from_device { "device: " } else { "" };
        painter.text(
            Pos2::new(x + 2.0, y + 1.0),
            Align2::LEFT_TOP,
            format!("{prefix}{}", problem.message),
            FontId::proportional(12.0),
            Color32::from_rgb(255, 140, 125),
        );
        y += PROBLEM_LINE;
    }
    y
}

fn root_row(painter: &egui::Painter, root: Option<&[RootSample]>, scale: &TickScale, y: f32) {
    let Some(root) = root else {
        painter.text(
            Pos2::new(scale.left + 4.0, y + ROOT_ROW * 0.5),
            Align2::LEFT_CENTER,
            "no table yet: needs a document that compiles, and a cook for moves with clips",
            FontId::proportional(11.5),
            Color32::from_gray(130),
        );
        return;
    };
    let step = |pair: &[RootSample]| {
        let delta: f32 =
            (0..3).map(|axis| (pair[1].translation[axis] - pair[0].translation[axis]).powi(2)).sum();
        delta.sqrt()
    };
    let most = root.windows(2).map(step).fold(1e-4, f32::max);
    let bottom = y + ROOT_ROW - 3.0;
    for (tick, pair) in root.windows(2).enumerate() {
        let height = step(pair) / most * (ROOT_ROW - 6.0);
        if height < 0.5 {
            continue;
        }
        let bar = Rect::from_x_y_ranges(
            scale.x(tick as u32) + 0.5..=scale.x(tick as u32 + 1) - 0.5,
            bottom - height..=bottom,
        );
        painter.rect_filled(bar, CornerRadius::ZERO, Color32::from_rgb(72, 118, 170));
    }
    if let (Some(first), Some(last)) = (root.first(), root.last()) {
        let travel: f32 = (0..3).map(|axis| (last.translation[axis] - first.translation[axis]).powi(2)).sum();
        painter.text(
            Pos2::new(scale.x(root.len().saturating_sub(1) as u32) - 4.0, y + 2.0),
            Align2::RIGHT_TOP,
            format!("{:.2} m, {:.0}°", travel.sqrt(), (last.yaw - first.yaw).to_degrees()),
            FontId::monospace(10.5),
            Color32::from_gray(190),
        );
    }
}

#[allow(clippy::too_many_arguments)]
fn draw_entry(
    ui: &mut Ui,
    painter: &egui::Painter,
    view: &TimelineView<'_>,
    entry: &RowEntry,
    entry_ref: EntryRef,
    band: Rect,
    scale: &TickScale,
    drag: &mut Option<Drag>,
    actions: &mut Vec<Action>,
    problems: &[&Problem],
) {
    let track = entry_ref.track;
    let selected = view.selected == Some(entry_ref);
    let open = entry.span.from <= view.tick && view.tick < entry.span.to;
    let name = title(track);
    let label = if track.is_instant() {
        format!("{name} {} at {}", entry.label, entry.span.from)
    } else {
        format!("{name} {} {}..{}", entry.label, entry.span.from, entry.span.to)
    };
    let refused = problems.iter().any(|problem| {
        let span = format!("{}..{}", entry.span.from, entry.span.to);
        problem.message.contains(&span)
            || (!entry.label.is_empty() && problem.message.contains(&format!("`{}`", entry.label)))
    });
    let base = color(track);
    let base_id = ui.scope_id();
    let id = |part: &str| base_id.with(("timeline entry", entry_ref, part));

    if track.is_instant() {
        let cell =
            Rect::from_x_y_ranges(scale.x(entry.span.from)..=scale.x(entry.span.from + 1), band.y_range());
        let hit = cell.expand2(Vec2::new(2.0, 0.0));
        let response = ui.interact(hit, id("instant"), Sense::click_and_drag());
        response.widget_info(|| WidgetInfo::labeled(egui::Role::Button, true, label.as_str()));
        let center = cell.center();
        let radius = (cell.width() * 0.5).clamp(4.0, 7.0);
        let points = vec![
            center + Vec2::new(0.0, -radius),
            center + Vec2::new(radius, 0.0),
            center + Vec2::new(0.0, radius),
            center + Vec2::new(-radius, 0.0),
        ];
        let fill = if open || selected { Color32::WHITE } else { base };
        let stroke = if refused {
            Stroke::new(1.5, Color32::from_rgb(255, 90, 80))
        } else if selected {
            Stroke::new(1.5, Color32::from_rgb(255, 220, 120))
        } else {
            Stroke::NONE
        };
        painter.add(egui::Shape::convex_polygon(points, fill, stroke));
        painter.text(
            Pos2::new(cell.right() + 3.0, center.y),
            Align2::LEFT_CENTER,
            &entry.label,
            FontId::proportional(12.0),
            Color32::from_gray(220),
        );
        if response.drag_started()
            && let Some(pressed) = pressed_x(&response)
        {
            *drag = Some(Drag::Instant { entry: entry_ref, grab_offset: center.x - pressed });
            actions.push(Action::Select(Some(entry_ref)));
            actions.push(Action::BeginGesture(format!("move {name} {}", entry.label)));
        }
        follow_drag(&response, drag, scale, actions);
        if response.clicked() {
            actions.push(Action::Select(Some(entry_ref)));
        }
        return;
    }

    let fill = if open { brighten(base) } else { base.gamma_multiply(0.78) };
    painter.rect_filled(band, CornerRadius::same(3), fill);
    let outline = if refused {
        Stroke::new(1.5, Color32::from_rgb(255, 90, 80))
    } else if selected {
        Stroke::new(1.5, Color32::from_rgb(255, 220, 120))
    } else {
        Stroke::new(1.0, Color32::from_black_alpha(90))
    };
    painter.rect_stroke(band, CornerRadius::same(3), outline, StrokeKind::Inside);
    let text = if selected || band.width() > 60.0 {
        if entry.label.is_empty() {
            format!("{}..{}", entry.span.from, entry.span.to)
        } else {
            format!("{}  {}..{}", entry.label, entry.span.from, entry.span.to)
        }
    } else {
        entry.label.clone()
    };
    let clipped = painter.with_clip_rect(band.shrink(1.0).intersect(painter.clip_rect()));
    clipped.text(
        band.left_center() + Vec2::new(5.0, 0.0),
        Align2::LEFT_CENTER,
        text,
        FontId::proportional(12.0),
        Color32::WHITE,
    );

    let body = ui.interact(band, id("body"), Sense::click_and_drag());
    body.widget_info(|| WidgetInfo::labeled(egui::Role::Button, true, label.as_str()));
    let left = Rect::from_x_y_ranges(band.left() - HANDLE_OUT..=band.left() + HANDLE_IN, band.y_range());
    let right = Rect::from_x_y_ranges(band.right() - HANDLE_IN..=band.right() + HANDLE_OUT, band.y_range());
    let parts = [(left, Part::Start, "start"), (right, Part::End, "end")];
    if view.editable {
        begin_window_drag(&body, Part::Body, entry_ref, entry, band, drag, actions);
        follow_drag(&body, drag, scale, actions);
    }
    if body.clicked() {
        actions.push(Action::Select(Some(entry_ref)));
    }
    if view.editable {
        for (rect, part, word) in parts {
            let response = ui.interact(rect, id(word), Sense::click_and_drag());
            let handle = format!("{label} {word}");
            response.widget_info(|| WidgetInfo::labeled(egui::Role::Slider, true, handle.as_str()));
            if response.hovered() || response.dragged() {
                let x = if part == Part::Start { band.left() } else { band.right() };
                let bar = Rect::from_x_y_ranges(x - 1.5..=x + 1.5, band.y_range());
                painter.rect_filled(bar, CornerRadius::same(1), Color32::WHITE);
            }
            begin_window_drag(&response, part, entry_ref, entry, band, drag, actions);
            follow_drag(&response, drag, scale, actions);
            if response.clicked() {
                actions.push(Action::Select(Some(entry_ref)));
            }
        }
    }
}

fn begin_window_drag(
    response: &Response,
    part: Part,
    entry_ref: EntryRef,
    entry: &RowEntry,
    band: Rect,
    drag: &mut Option<Drag>,
    actions: &mut Vec<Action>,
) {
    if response.drag_started()
        && let Some(pressed) = pressed_x(response)
    {
        let grab_edge_x = match part {
            Part::Start | Part::Body => band.left(),
            Part::End => band.right(),
        };
        *drag = Some(Drag::Window { entry: entry_ref, part, span: entry.span, grab_x: pressed, grab_edge_x });
        actions.push(Action::Select(Some(entry_ref)));
        let word = match part {
            Part::Start => "start of",
            Part::End => "end of",
            Part::Body => "",
        };
        actions.push(Action::BeginGesture(format!("drag {word} {} {}", title(entry_ref.track), entry.label)));
    }
}

/// Where the press that became this drag was. egui says a drag has started only once the pointer has
/// travelled, and that travel counts: measured from there, a drag would come up short by it.
fn pressed_x(response: &Response) -> Option<f32> {
    let origin = response.ctx.input(|input| input.pointer.press_origin());
    origin.or_else(|| response.interact_pointer_pos()).map(|pointer| pointer.x)
}

fn follow_drag(response: &Response, drag: &mut Option<Drag>, scale: &TickScale, actions: &mut Vec<Action>) {
    // A release can arrive in one pass with the last motion, which then counts.
    if (response.dragged() || response.drag_stopped())
        && let (Some(held), Some(pointer)) = (&*drag, response.interact_pointer_pos())
        && let Some(action) = held.at(pointer.x, scale)
    {
        actions.push(action);
    }
    if response.drag_stopped() && drag.take().is_some() {
        actions.push(Action::EndGesture);
    }
}

fn brighten(color: Color32) -> Color32 {
    let lift = |channel: u8| channel.saturating_add(36);
    Color32::from_rgb(lift(color.r()), lift(color.g()), lift(color.b()))
}

#[cfg(test)]
mod tests {
    use sapphire_moves::MoveSetDocument;

    use super::*;
    use crate::edits::test_files::PLAYER;

    fn light_1() -> MoveDocument {
        let document = MoveSetDocument::parse(PLAYER).expect("parses");
        document.moves.into_iter().find(|mv| mv.name == "light_1").expect("the move")
    }

    #[test]
    fn cells_and_edges_are_ticks() {
        let scale = TickScale::fit(100.0, 340.0, 34);
        assert_eq!(scale.per_tick, 10.0);
        assert_eq!(scale.x(7), 170.0);
        assert_eq!(scale.cell(170.0), 7);
        assert_eq!(scale.cell(179.9), 7);
        assert_eq!(scale.edge(174.9), 7);
        assert_eq!(scale.edge(175.1), 8);
        assert_eq!(scale.cell(50.0), 0);
        assert_eq!(scale.edge(50.0), 0);
    }

    #[test]
    fn overlapping_windows_take_lanes_and_the_plans_rows_always_show() {
        let mv = light_1();
        let rows = rows_of(&mv, false, 10.0);
        let tracks: Vec<Track> = rows.iter().map(|row| row.track).collect();
        assert_eq!(tracks, [Track::Hitboxes, Track::Invulnerable, Track::Cancels, Track::Warps, Track::Cues]);
        let cancels = rows.iter().find(|row| row.track == Track::Cancels).expect("cancels");
        assert_eq!(cancels.lanes, 2, "2..34 and 14..28 overlap");
        assert_eq!(cancels.entries[0].lane, 0);
        assert_eq!(cancels.entries[1].lane, 1);
        assert_eq!(cancels.entries[0].label, "dodge");
        assert_eq!(cancels.entries[1].label, "light -> light_2, skill");
        let all = rows_of(&mv, true, 10.0);
        assert!(all.iter().any(|row| row.track == Track::Parry));
    }

    #[test]
    fn a_drag_lands_on_ticks_from_where_it_grabbed() {
        let scale = TickScale { left: 100.0, per_tick: 10.0 };
        let entry = EntryRef { mv: 0, track: Track::Hitboxes, index: 0 };
        let span = TickSpan { from: 7, to: 11 };
        // Grabbed 3 px inside the end, moved two ticks and a bit: the end goes two ticks.
        let drag = Drag::Window { entry, part: Part::End, span, grab_x: 207.0, grab_edge_x: 210.0 };
        assert_eq!(
            drag.at(207.0, &scale),
            Some(Action::SetWindow { entry, from: 7, to: 11, part: Part::End })
        );
        assert_eq!(
            drag.at(229.0, &scale),
            Some(Action::SetWindow { entry, from: 7, to: 13, part: Part::End })
        );
        // Past the far end it goes where it's put: the document says what it thinks of that.
        let drag = Drag::Window { entry, part: Part::Start, span, grab_x: 170.0, grab_edge_x: 170.0 };
        assert_eq!(
            drag.at(240.0, &scale),
            Some(Action::SetWindow { entry, from: 14, to: 11, part: Part::Start })
        );
        // Its body keeps its length, and stops at tick 0, which is as far as the text can say.
        let drag = Drag::Window { entry, part: Part::Body, span, grab_x: 180.0, grab_edge_x: 170.0 };
        assert_eq!(
            drag.at(195.0, &scale),
            Some(Action::SetWindow { entry, from: 9, to: 13, part: Part::Body })
        );
        assert_eq!(drag.at(0.0, &scale), Some(Action::SetWindow { entry, from: 0, to: 4, part: Part::Body }));
    }

    #[test]
    fn a_phase_edge_trades_ticks_with_its_neighbour_and_the_end_changes_the_length() {
        let scale = TickScale { left: 0.0, per_tick: 10.0 };
        let phases = Phases { startup: 7, active: 4, recovery: 23 };
        let edge = |edge, x| {
            let drag = Drag::Phase { mv: 0, edge, phases, grab_offset: 0.0 };
            let Some(Action::SetPhases { phases, .. }) = drag.at(x, &scale) else { panic!("a phase") };
            phases
        };
        assert_eq!(edge(PhaseEdge::StartupEnd, 90.0), Phases { startup: 9, active: 2, recovery: 23 });
        assert_eq!(edge(PhaseEdge::StartupEnd, 500.0), Phases { startup: 11, active: 0, recovery: 23 });
        assert_eq!(edge(PhaseEdge::ActiveEnd, 130.0), Phases { startup: 7, active: 6, recovery: 21 });
        assert_eq!(edge(PhaseEdge::ActiveEnd, 0.0), Phases { startup: 7, active: 0, recovery: 27 });
        assert_eq!(edge(PhaseEdge::MoveEnd, 360.0), Phases { startup: 7, active: 4, recovery: 25 });
    }
}
