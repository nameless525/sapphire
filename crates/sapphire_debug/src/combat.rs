//! The combat panel (M2 T1): each actor's intent buffer, current move, phase, elapsed tick and live
//! windows, a timeline of the move with the current tick and each track's window, switches for the
//! sandbox's combat views, and the replay recorder's controls.
//!
//! **The seam is [`CombatSnapshot`],** plain data the app builds while the panel is open. This crate
//! may depend on `sapphire_gameplay` (`xtask/layers.toml` lets a dev-only crate depend on any runtime
//! crate), and doesn't, on purpose. What the panel needs is mostly work against a simulation's
//! internals: a move's text and tracks through the library by revision, a window's state at a tick, an
//! intent's age on its owner's clock. The app does that once, in one function, and the G packages can
//! change what a simulation holds without touching the overlay. It also lets a test show the panel
//! something exact, including states no simulation is in yet. The cost is one allocation-heavy
//! rebuild a frame, paid only while the panel shows.
//!
//! [`RecorderControls`] carries what the panel asks for back to the app, as
//! [`EffectControls`](crate::EffectControls) does for screen effects.

use egui::{Color32, FontId, Sense, Ui, pos2, vec2};
use sapphire_core::cvar::CvarRegistry;
use sapphire_core::debug_draw::Color;

use crate::labels;
use crate::ui::cvar_widget;

/// The prefix of the cvars this panel shows switches for: the combat views' (M2 T1), which the app
/// registers.
pub const COMBAT_VIEW_CVARS: &str = "combat.view";

/// Points a track's name takes in the timeline.
const TRACK_NAMES: f32 = 96.0;

/// A row of the timeline, in points.
const ROW: f32 = 11.0;

/// The fight the app is running, as the panel shows it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct CombatSnapshot {
    /// Fixed steps the simulation has run.
    pub tick: u64,
    pub actors: Vec<ActorSnapshot>,
    pub recorder: RecorderSnapshot,
}

/// One actor's moves and intents.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ActorSnapshot {
    /// Its stable ID.
    pub id: u32,
    /// Its own clock's reading, in ticks.
    pub clock_ticks: f32,
    /// Ticks of hitstop left on that clock.
    pub hitstop_ticks: u32,
    /// What its hurtbox is doing, with the combat views' colour for it.
    pub body: (&'static str, Color),
    /// Which windows count this step, each with its colour.
    pub windows: Vec<(&'static str, Color)>,
    /// Its intent buffer, oldest first.
    pub intents: Vec<IntentSnapshot>,
    /// The intents held down.
    pub held: Vec<String>,
    /// The move it's playing.
    pub current: Option<MoveSnapshot>,
}

/// A buffered press.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct IntentSnapshot {
    pub name: String,
    pub priority: u8,
    /// Ticks on its owner's clock since it was pressed.
    pub age_ticks: f32,
    /// Ticks on its owner's clock it can still fire in.
    pub left_ticks: f32,
}

/// A move playing, with its tracks.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct MoveSnapshot {
    pub name: String,
    /// Which playing this is, of all the actor's.
    pub serial: u32,
    pub phase: &'static str,
    /// How far into the move, in ticks of its owner's clock.
    pub elapsed_ticks: f32,
    pub startup: u32,
    pub active: u32,
    pub recovery: u32,
    pub tracks: Vec<TrackSnapshot>,
}

impl MoveSnapshot {
    fn ticks(&self) -> u32 {
        self.startup + self.active + self.recovery
    }
}

/// One window of a move: `from` up to but not including `to`, in ticks.
#[derive(Clone, Debug, PartialEq)]
pub struct TrackSnapshot {
    pub name: String,
    pub from: u32,
    pub to: u32,
    /// Whether it's open at the move's current tick.
    pub live: bool,
    pub color: Color,
}

/// The replay recorder, as the panel shows it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct RecorderSnapshot {
    pub state: RecorderState,
    /// The scenarios a recording can start from.
    pub scenarios: Vec<String>,
    /// Whether someone is there to play: the panel offers to record only then.
    pub can_record: bool,
    /// How the last save went, or why the last request failed.
    pub note: Option<String>,
}

/// What the app's fight is.
#[derive(Clone, Debug, Default, PartialEq)]
pub enum RecorderState {
    /// No fight is running.
    #[default]
    Idle,
    /// A recording, of the player or of a script.
    Recording {
        scenario: String,
        path: String,
        ticks: u64,
        /// It reached the file's limit on a run's length, and has stopped there.
        at_limit: bool,
        /// A script plays it, so there's nothing for the panel to do but say so.
        scripted: bool,
    },
    /// A replay playing.
    Replaying { name: String, tick: u64, ticks: u32 },
    /// A fight with no file (`--fight`), of the many it's had, which starts over by itself.
    Fighting { fight: u32, tick: u64 },
}

/// What the panel asks of the recorder.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RecorderRequest {
    /// Start `scenario` over and record the player from its first tick.
    Start { scenario: String },
    /// End the recording and write it.
    Save,
    /// End the recording and write nothing.
    Discard,
}

/// Developer control of the replay recorder: the panel records what it wants, and the app acts on it
/// between steps ([`Self::take`]), the same hook as [`TimeControls`](crate::TimeControls).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RecorderControls {
    request: Option<RecorderRequest>,
}

impl RecorderControls {
    /// Asks for `request`, in place of anything not yet taken.
    pub fn ask(&mut self, request: RecorderRequest) {
        self.request = Some(request);
    }

    /// What the panel asked for since the last call.
    pub fn take(&mut self) -> Option<RecorderRequest> {
        self.request.take()
    }
}

/// What the panel keeps between frames.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct CombatPanel {
    /// Which scenario the recorder would start.
    scenario: usize,
}

/// Shows the panel, recording what the player asks for in `controls`.
pub(crate) fn panel(
    ui: &mut Ui,
    memory: &mut CombatPanel,
    controls: &mut RecorderControls,
    cvars: &CvarRegistry,
    snapshot: Option<&CombatSnapshot>,
) {
    ui.horizontal(|ui| {
        for cvar in cvars.with_prefix(COMBAT_VIEW_CVARS) {
            ui.label(cvar.name().trim_start_matches("combat.")).on_hover_text(cvar.help());
            cvar_widget(ui, cvar);
        }
    });
    let Some(snapshot) = snapshot else {
        ui.weak("The app runs no fight.");
        return;
    };
    ui.separator();
    recorder(ui, memory, controls, &snapshot.recorder);
    ui.separator();
    if snapshot.actors.is_empty() {
        ui.weak("No fight is running: record one, or start with --replay.");
        return;
    }
    ui.label(format!("Tick {}", snapshot.tick));
    egui::ScrollArea::vertical().max_height(520.0).auto_shrink([false, true]).show(ui, |ui| {
        for actor in &snapshot.actors {
            actor_section(ui, actor);
            ui.separator();
        }
    });
}

fn recorder(
    ui: &mut Ui,
    memory: &mut CombatPanel,
    controls: &mut RecorderControls,
    recorder: &RecorderSnapshot,
) {
    match &recorder.state {
        RecorderState::Idle if recorder.can_record => {
            ui.horizontal(|ui| {
                let scenario = recorder.scenarios.get(memory.scenario);
                egui::ComboBox::from_id_salt("combat recorder scenario")
                    .selected_text(scenario.map_or("", String::as_str))
                    .show_ui(ui, |ui| {
                        for (index, name) in recorder.scenarios.iter().enumerate() {
                            ui.selectable_value(&mut memory.scenario, index, name);
                        }
                    });
                if let Some(scenario) = scenario
                    && ui
                        .button("Record")
                        .on_hover_text(
                            "Starts the scenario over and records from its first tick: a replay is a \
                             scenario and its actions, with no state to begin from anywhere else.",
                        )
                        .clicked()
                {
                    controls.ask(RecorderRequest::Start { scenario: scenario.clone() });
                }
            });
        }
        RecorderState::Idle => {
            ui.weak("Recording needs someone to play: the windowed app, in a `dev` build.");
        }
        RecorderState::Recording { scenario, path, ticks, at_limit, scripted } => {
            ui.label(format!("Recording {scenario}: {ticks} ticks, to {path}"));
            if *at_limit {
                ui.colored_label(
                    Color32::from_rgb(255, 180, 80),
                    "It reached a file's limit and has stopped.",
                );
            }
            if recorder.can_record && !*scripted {
                ui.horizontal(|ui| {
                    if ui.button("Stop and save").clicked() {
                        controls.ask(RecorderRequest::Save);
                    }
                    if ui.button("Discard").clicked() {
                        controls.ask(RecorderRequest::Discard);
                    }
                });
            }
        }
        RecorderState::Replaying { name, tick, ticks } => {
            ui.label(format!("Replaying {name}: tick {tick} of {ticks}"));
        }
        RecorderState::Fighting { fight, tick } => {
            ui.label(format!("Fight {fight}, tick {tick}. Nothing's written, and R starts it over."));
        }
    }
    if let Some(note) = &recorder.note {
        ui.weak(note);
    }
}

fn actor_section(ui: &mut Ui, actor: &ActorSnapshot) {
    ui.horizontal_wrapped(|ui| {
        ui.strong(format!("#{}", actor.id));
        let (body, color) = actor.body;
        ui.colored_label(labels::color(color), format!("hurtbox {body}"));
        ui.weak(format!("clock {:.2}", actor.clock_ticks));
        if actor.hitstop_ticks > 0 {
            ui.colored_label(Color32::from_rgb(255, 180, 80), format!("hitstop {}", actor.hitstop_ticks));
        }
        for &(window, color) in &actor.windows {
            ui.colored_label(labels::color(color), window);
        }
    });
    match &actor.current {
        Some(current) => {
            ui.label(format!(
                "{} (#{}) · {} · tick {:.2} of {}",
                current.name,
                current.serial,
                current.phase,
                current.elapsed_ticks,
                current.ticks(),
            ));
            timeline(ui, current);
        }
        None => {
            ui.weak("No move.");
        }
    }
    ui.horizontal_wrapped(|ui| {
        if actor.intents.is_empty() && actor.held.is_empty() {
            ui.weak("Buffer empty.");
        }
        for intent in &actor.intents {
            ui.label(format!(
                "{} p{} · {:.1} old, {:.1} left",
                intent.name, intent.priority, intent.age_ticks, intent.left_ticks
            ));
        }
        if !actor.held.is_empty() {
            ui.weak(format!("held: {}", actor.held.join(", ")));
        }
    });
}

/// The move's phases on top, a row for each track under them, and a line at the current tick.
fn timeline(ui: &mut Ui, current: &MoveSnapshot) {
    let rows = 1 + current.tracks.len();
    let width = ui.available_width().clamp(240.0, 460.0);
    let (rect, _) = ui.allocate_exact_size(vec2(width, ROW * rows as f32), Sense::hover());
    let painter = ui.painter_at(rect);
    let left = rect.left() + TRACK_NAMES;
    let scale = (rect.right() - left) / current.ticks().max(1) as f32;
    let x = |tick: f32| left + tick * scale;
    let row = |index: usize| rect.top() + ROW * index as f32;
    let font = FontId::monospace(9.0);
    let label = |index: usize, text: &str, color: Color32| {
        painter.text(pos2(rect.left(), row(index)), egui::Align2::LEFT_TOP, text, font.clone(), color);
    };

    let bar = |index: usize, from: f32, to: f32, color: Color32| {
        let bar =
            egui::Rect::from_min_max(pos2(x(from), row(index) + 1.0), pos2(x(to), row(index) + ROW - 1.0));
        painter.rect_filled(bar, 1.0, color);
    };
    label(0, "phases", Color32::from_gray(190));
    let (startup, active) = (current.startup as f32, current.active as f32);
    bar(0, 0.0, startup, Color32::from_gray(70));
    bar(0, startup, startup + active, Color32::from_gray(170));
    bar(0, startup + active, current.ticks() as f32, Color32::from_gray(100));

    for (index, track) in current.tracks.iter().enumerate() {
        let color = labels::color(track.color);
        // One tick at the least, so a cue at one instant shows.
        let to = track.to.max(track.from + 1) as f32;
        let fill = if track.live { color } else { color.gamma_multiply(0.45) };
        label(index + 1, &track.name, if track.live { color } else { Color32::from_gray(150) });
        bar(index + 1, track.from as f32, to, fill);
    }
    let now = x(current.elapsed_ticks.min(current.ticks() as f32));
    painter.vline(now, rect.y_range(), egui::Stroke::new(1.0, Color32::WHITE));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn context() -> egui::Context {
        let ctx = egui::Context::default();
        ctx.set_fonts(egui::FontDefinitions::default());
        ctx
    }

    fn input(events: Vec<egui::Event>) -> egui::RawInput {
        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, vec2(800.0, 600.0));
        egui::RawInput { screen_rect: Some(screen), events, ..egui::RawInput::default() }
    }

    /// Shows the panel for a frame with `events` in it.
    fn frame(
        ctx: &egui::Context,
        memory: &mut CombatPanel,
        controls: &mut RecorderControls,
        cvars: &CvarRegistry,
        snapshot: Option<&CombatSnapshot>,
        events: Vec<egui::Event>,
    ) -> egui::FullOutput {
        let mut output = ctx.run_ui(input(events), |ui| panel(ui, memory, controls, cvars, snapshot));
        output.textures_delta.clear();
        output
    }

    fn fight() -> CombatSnapshot {
        let track = |name: &str, from, to, live, color| TrackSnapshot {
            name: name.to_owned(),
            from,
            to,
            live,
            color,
        };
        CombatSnapshot {
            tick: 42,
            actors: vec![
                ActorSnapshot {
                    id: 1,
                    clock_ticks: 42.0,
                    hitstop_ticks: 3,
                    body: ("perfect dodge", Color::MAGENTA),
                    windows: vec![("perfect dodge", Color::MAGENTA), ("cancel", Color::BLUE)],
                    intents: vec![IntentSnapshot {
                        name: "light".to_owned(),
                        priority: 1,
                        age_ticks: 3.0,
                        left_ticks: 9.0,
                    }],
                    held: vec!["dodge".to_owned()],
                    current: Some(MoveSnapshot {
                        name: "dodge".to_owned(),
                        serial: 4,
                        phase: "active",
                        elapsed_ticks: 6.5,
                        startup: 1,
                        active: 13,
                        recovery: 8,
                        tracks: vec![
                            track("invulnerable", 1, 14, true, Color::rgb(150, 150, 160)),
                            track("perfect dodge", 1, 7, true, Color::MAGENTA),
                            track("cue", 2, 2, false, Color::WHITE),
                        ],
                    }),
                },
                ActorSnapshot { id: 2, ..ActorSnapshot::default() },
            ],
            recorder: RecorderSnapshot {
                state: RecorderState::Replaying { name: "dodge-dash".to_owned(), tick: 42, ticks: 720 },
                ..RecorderSnapshot::default()
            },
        }
    }

    /// The text a panel laid out, from its painted shapes.
    fn text_of(output: &egui::FullOutput) -> String {
        fn visit(shape: &egui::epaint::Shape, text: &mut String) {
            match shape {
                egui::epaint::Shape::Text(shape) => {
                    text.push_str(shape.galley.text());
                    text.push('\n');
                }
                egui::epaint::Shape::Vec(shapes) => shapes.iter().for_each(|shape| visit(shape, text)),
                _ => {}
            }
        }
        let mut text = String::new();
        for clipped in &output.shapes {
            visit(&clipped.shape, &mut text);
        }
        text
    }

    #[test]
    fn it_shows_each_actors_move_windows_and_buffer() {
        let (ctx, mut memory, mut controls, cvars) =
            (context(), CombatPanel::default(), RecorderControls::default(), CvarRegistry::new());
        let snapshot = fight();
        let mut output = frame(&ctx, &mut memory, &mut controls, &cvars, Some(&snapshot), vec![]);
        // egui lays a new window's first frame out twice.
        if text_of(&output).is_empty() {
            output = frame(&ctx, &mut memory, &mut controls, &cvars, Some(&snapshot), vec![]);
        }
        let text = text_of(&output);
        for part in [
            "Tick 42",
            "#1",
            "hurtbox perfect dodge",
            "hitstop 3",
            "dodge (#4) · active · tick 6.50 of 22",
            "invulnerable",
            "cue",
            "light p1 · 3.0 old, 9.0 left",
            "held: dodge",
            "#2",
            "No move.",
            "Replaying dodge-dash: tick 42 of 720",
        ] {
            assert!(text.contains(part), "{part:?} isn't in:\n{text}");
        }
    }

    #[test]
    fn without_a_fight_it_says_so() {
        let (ctx, mut memory, mut controls, cvars) =
            (context(), CombatPanel::default(), RecorderControls::default(), CvarRegistry::new());
        let text = text_of(&frame(&ctx, &mut memory, &mut controls, &cvars, None, vec![]));
        assert!(text.contains("no fight"), "{text}");
        let idle = CombatSnapshot::default();
        let text = text_of(&frame(&ctx, &mut memory, &mut controls, &cvars, Some(&idle), vec![]));
        assert!(text.contains("No fight is running"), "{text}");
        assert!(text.contains("needs someone to play"), "{text}");
    }

    /// Clicks at each point of a grid over the panel until something's asked for, and returns it.
    fn click_through(snapshot: &CombatSnapshot) -> Option<RecorderRequest> {
        let (ctx, mut memory, mut controls, cvars) =
            (context(), CombatPanel::default(), RecorderControls::default(), CvarRegistry::new());
        frame(&ctx, &mut memory, &mut controls, &cvars, Some(snapshot), vec![]);
        for y in (0..200).step_by(6) {
            for x in (0..400).step_by(16) {
                let at = pos2(x as f32, y as f32);
                let press = |pressed| egui::Event::PointerButton {
                    pos: at,
                    button: egui::PointerButton::Primary,
                    pressed,
                    modifiers: egui::Modifiers::NONE,
                };
                for events in
                    [vec![egui::Event::PointerMoved(at)], vec![press(true)], vec![press(false)], vec![]]
                {
                    frame(&ctx, &mut memory, &mut controls, &cvars, Some(snapshot), events);
                    if let Some(request) = controls.take() {
                        return Some(request);
                    }
                }
            }
        }
        None
    }

    #[test]
    fn its_record_button_asks_to_start_the_scenario_shown() {
        let snapshot = CombatSnapshot {
            recorder: RecorderSnapshot {
                scenarios: vec!["melee".to_owned(), "duel".to_owned()],
                can_record: true,
                ..RecorderSnapshot::default()
            },
            ..CombatSnapshot::default()
        };
        assert_eq!(click_through(&snapshot), Some(RecorderRequest::Start { scenario: "melee".to_owned() }));
    }

    #[test]
    fn a_recording_offers_save_and_discard_only_with_a_player() {
        let recording = |can_record, scripted| CombatSnapshot {
            recorder: RecorderSnapshot {
                state: RecorderState::Recording {
                    scenario: "duel".to_owned(),
                    path: "a.sreplay".to_owned(),
                    ticks: 3,
                    at_limit: false,
                    scripted,
                },
                can_record,
                ..RecorderSnapshot::default()
            },
            ..CombatSnapshot::default()
        };
        assert_eq!(click_through(&recording(true, false)), Some(RecorderRequest::Save));
        assert_eq!(click_through(&recording(false, false)), None, "a capture has no one to click");
        assert_eq!(click_through(&recording(true, true)), None, "a script's isn't the player's to stop");
    }

    #[test]
    fn a_request_waits_to_be_taken_once() {
        let mut controls = RecorderControls::default();
        assert_eq!(controls.take(), None);
        controls.ask(RecorderRequest::Discard);
        controls.ask(RecorderRequest::Save);
        assert_eq!(controls.take(), Some(RecorderRequest::Save), "the latest");
        assert_eq!(controls.take(), None);
    }
}
