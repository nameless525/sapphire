//! The command palette (Cmd+P): every command the [`CommandRegistry`] has, found by typing words of its name or of
//! what it's for. A command that takes nothing runs when it's picked; one that takes arguments puts them in the
//! field to fill in, and Enter runs what the field says, as a script would spell it. Opening and closing it
//! changes nothing, so neither is a command.

use egui::{Align2, Color32, Key, RichText, Ui};

use super::Editor;
use crate::commands::{Command, CommandRegistry, Registered};

/// How many commands the palette lists at once.
const ROWS: usize = 12;

/// What the palette keeps while it's open.
#[derive(Debug, Default)]
pub(super) struct Palette {
    text: String,
    /// Whether the field has been given the keyboard: once, when it opens, and again when a pick fills it.
    focused: bool,
    /// Why the field's text didn't run, until it changes.
    refused: Option<String>,
}

impl Editor {
    /// Draws the palette over everything, if it's open, and runs what it's asked.
    pub(super) fn palette_ui(&mut self, ui: &Ui) {
        let Some(mut palette) = self.palette.take() else { return };
        let mut run = None;
        let mut open = true;
        egui::Window::new("Commands")
            .title_bar(false)
            .collapsible(false)
            .resizable(false)
            .anchor(Align2::CENTER_TOP, [0.0, 48.0])
            .default_width(520.0)
            .show(ui.ctx(), |ui| {
                let field = egui::TextEdit::singleline(&mut palette.text)
                    .hint_text("A command, or words of one")
                    .desired_width(f32::INFINITY);
                let response = ui.add(field);
                if !palette.focused {
                    response.request_focus();
                    palette.focused = true;
                }
                if response.changed() {
                    palette.refused = None;
                }
                let entered = response.lost_focus() && ui.input(|input| input.key_pressed(Key::Enter));
                if response.lost_focus() && ui.input(|input| input.key_pressed(Key::Escape)) {
                    open = false;
                }
                // The words before any arguments find it.
                let words = palette.text.split('(').next().unwrap_or_default();
                let found: Vec<&Registered> = CommandRegistry.matching(words).take(ROWS).collect();
                let mut picked = None;
                for registered in &found {
                    let key = registered.key.map(|key| format!("   {key}")).unwrap_or_default();
                    let row = RichText::new(format!("{}: {}{key}", registered.name, registered.help));
                    if ui.selectable_label(false, row).clicked() {
                        picked = Some(*registered);
                    }
                }
                if entered {
                    match Command::parse(&palette.text) {
                        Ok(command) => run = Some(command),
                        Err(why) => match found.first() {
                            Some(first) if !palette.text.contains('(') => picked = Some(*first),
                            _ => palette.refused = Some(why),
                        },
                    }
                }
                if let Some(picked) = picked {
                    if picked.takes_nothing() {
                        run = Command::parse(picked.name).ok();
                    } else {
                        palette.text = picked.template.to_owned();
                        palette.focused = false;
                    }
                }
                if let Some(why) = &palette.refused {
                    ui.label(RichText::new(why).small().color(Color32::from_rgb(255, 120, 110)));
                }
            });
        if let Some(command) = run {
            self.act(command);
        } else if open {
            self.palette = Some(palette);
        }
    }

    /// Whether the palette is open.
    #[must_use]
    pub fn palette_is_open(&self) -> bool {
        self.palette.is_some()
    }
}
