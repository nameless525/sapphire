//! The screen effects panel (M1 P3): fire any preset of the world's effect library, release
//! sustained effects or clear the stack, and watch the layers playing with their weights.
//! [`EffectControls`] carries its requests to the world's
//! [`EffectStack`](sapphire_world::screen_effects::EffectStack), as
//! [`TodControls`](crate::TodControls) does to time of day.

use egui::{RichText, Ui};
use sapphire_world::screen_effects::{
    EffectClock, EffectHandle, EffectLibrary, EffectPresetId, EffectStack, STACK_CAPACITY,
};

/// Developer control of the screen effects: the panel records what it wants, and the app applies
/// it to the world's effect stack before each fixed step ([`Self::apply`]), the same hook as
/// [`TimeControls`](crate::TimeControls). It never allocates.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct EffectControls {
    fire: Option<EffectPresetId>,
    release: bool,
    clear: bool,
    /// The last push the panel made, to release.
    last: Option<EffectHandle>,
}

impl EffectControls {
    /// Pushes preset `id`.
    pub fn fire(&mut self, id: EffectPresetId) {
        self.fire = Some(id);
    }

    /// Releases the sustained layers of the last push the panel made.
    pub fn release(&mut self) {
        self.release = true;
    }

    /// Stops every effect.
    pub fn clear(&mut self) {
        self.clear = true;
    }

    /// Applies what the panel asked for since the last call.
    pub fn apply(&mut self, stack: &mut EffectStack, library: &EffectLibrary) {
        if std::mem::take(&mut self.clear) {
            stack.clear();
        }
        if std::mem::take(&mut self.release)
            && let Some(push) = self.last
        {
            stack.release(push);
        }
        if let Some(preset) = self.fire.take().and_then(|id| library.preset(id)) {
            self.last = Some(stack.push(preset));
        }
    }
}

/// Shows the panel for the world's effect stack and presets, recording what the player asks for
/// in `controls`.
pub(crate) fn panel(
    ui: &mut Ui,
    controls: &mut EffectControls,
    effects: Option<(&EffectStack, &EffectLibrary)>,
) {
    let Some((stack, library)) = effects else {
        ui.weak("The app runs no screen effects.");
        return;
    };
    ui.horizontal_wrapped(|ui| {
        for (index, name) in library.names().enumerate() {
            if ui.button(name).on_hover_text("Push this preset").clicked() {
                controls.fire(EffectPresetId(index as u16));
            }
        }
    });
    ui.horizontal(|ui| {
        if ui.button("Release").on_hover_text("Release the last push's sustained effects").clicked() {
            controls.release();
        }
        if ui.button("Clear").on_hover_text("Stop every effect").clicked() {
            controls.clear();
        }
    });
    let stats = stack.stats();
    ui.label(format!(
        "{} of {STACK_CAPACITY} layers, {} showing; {} pushed, {} dropped",
        stats.depth, stats.active, stats.pushed, stats.dropped
    ));
    if stack.is_empty() {
        ui.weak("Nothing playing.");
        return;
    }
    egui::Grid::new("screen effects").num_columns(4).striped(true).show(ui, |ui| {
        for heading in ["Effect", "Time", "Ends", "Weight"] {
            ui.weak(heading);
        }
        ui.end_row();
        for effect in stack.playing() {
            let clock = match effect.clock {
                EffectClock::World => "",
                EffectClock::Real => " (real time)",
            };
            ui.label(format!("{}{clock}", effect.kind.name()));
            let held = if effect.held { " held" } else { "" };
            ui.monospace(format!("{:.2} s{held}", effect.time));
            ui.monospace(if effect.end.is_finite() {
                format!("{:.2} s", effect.end)
            } else {
                "on release".to_owned()
            });
            ui.add(
                egui::ProgressBar::new(effect.weight)
                    .desired_width(80.0)
                    .text(RichText::new(format!("{:.2}", effect.weight)).monospace()),
            );
            ui.end_row();
        }
    });
}
