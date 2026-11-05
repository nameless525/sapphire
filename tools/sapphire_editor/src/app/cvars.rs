//! The cvar panel: what a device has, drawn by what each cvar takes, and set over live link.
//!
//! The device is the only thing that knows its cvars, so with none connected there's nothing to show, and the
//! panel says so. The link asks only while this panel is drawn.

use egui::{Color32, RichText, Ui};
use sapphire_livelink::protocol::{CvarStatus, CvarStatusKind};

use super::Editor;

/// What the panel keeps between passes.
#[derive(Debug, Default)]
pub(super) struct CvarPanel {
    filter: String,
    /// Whether it's been drawn this pass.
    pub(super) drawn: bool,
}

impl Editor {
    pub(super) fn cvars_panel(&mut self, ui: &mut Ui) {
        self.cvar_panel.drawn = true;
        let Some(list) = self.link.cvars() else {
            let why = self.link.trouble().unwrap_or("looking");
            ui.label(
                RichText::new(if self.link.device().is_some() {
                    "Asking the device what it has."
                } else {
                    why
                })
                .small()
                .color(Color32::from_gray(150)),
            );
            return;
        };
        ui.horizontal(|ui| {
            ui.label("Find");
            ui.add(egui::TextEdit::singleline(&mut self.cvar_panel.filter).desired_width(f32::INFINITY));
        });
        if let Some((name, why)) = self.link.cvar_refusal() {
            ui.colored_label(Color32::from_rgb(255, 120, 110), format!("{name}: {why}"));
        }
        let filter = self.cvar_panel.filter.to_lowercase();
        let mut sets = Vec::new();
        egui::ScrollArea::vertical().id_salt("cvars").auto_shrink([false, false]).show(ui, |ui| {
            egui::Grid::new("cvar rows").num_columns(3).show(ui, |ui| {
                for cvar in list.iter().filter(|cvar| filter.is_empty() || cvar.name.contains(&filter)) {
                    ui.label(&cvar.name).on_hover_text(&cvar.help);
                    if let Some(value) = cvar_widget(ui, cvar) {
                        sets.push((cvar.name.clone(), value));
                    }
                    if cvar.value != cvar.default
                        && ui.small_button("reset").on_hover_text(format!("to {}", cvar.default)).clicked()
                    {
                        sets.push((cvar.name.clone(), cvar.default.clone()));
                    }
                    ui.end_row();
                }
            });
        });
        for (name, value) in sets {
            self.link.set_cvar(&name, value);
        }
    }
}

/// Draws what a cvar takes, and gives back the value it's been changed to, as text the console would take.
fn cvar_widget(ui: &mut Ui, cvar: &CvarStatus) -> Option<String> {
    match &cvar.kind {
        CvarStatusKind::Bool => {
            let mut on = cvar.value == "true" || cvar.value == "1" || cvar.value == "on";
            ui.checkbox(&mut on, "").changed().then(|| on.to_string())
        }
        CvarStatusKind::Choice(names) => {
            let mut chosen = None;
            egui::ComboBox::from_id_salt(&cvar.name).selected_text(&cvar.value).show_ui(ui, |ui| {
                for name in names {
                    if ui.selectable_label(*name == cvar.value, name).clicked() {
                        chosen = Some(name.clone());
                    }
                }
            });
            chosen
        }
        CvarStatusKind::Int | CvarStatusKind::UInt | CvarStatusKind::Float => {
            let whole = !matches!(cvar.kind, CvarStatusKind::Float);
            let Ok(mut number) = cvar.value.parse::<f64>() else {
                ui.label(&cvar.value);
                return None;
            };
            let mut drag = egui::DragValue::new(&mut number).speed(if whole { 0.25 } else { 0.01 });
            if whole {
                drag = drag.max_decimals(0);
            }
            if let Some((low, high)) = &cvar.range
                && let (Ok(low), Ok(high)) = (low.parse::<f64>(), high.parse::<f64>())
            {
                drag = drag.range(low..=high);
            } else if matches!(cvar.kind, CvarStatusKind::UInt) {
                drag = drag.range(0.0..=f64::from(u32::MAX));
            }
            ui.add(drag).changed().then(|| {
                if whole { format!("{}", number.round() as i64) } else { format!("{}", number as f32) }
            })
        }
    }
}
