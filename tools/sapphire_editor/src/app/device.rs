//! The device pane: what a device on live link says of itself, of the last push and of its last attempts.

use super::*;

impl Editor {
    pub(super) fn device_panel(&mut self, ui: &mut Ui) {
        match self.link.device() {
            Some(status) => {
                ui.label(format!("{}, cook `{}`", status.platform, status.cook));
                let name = self.current().map(|file| file.file_name()).unwrap_or_default();
                let taken = status.documents.iter().find(|document| {
                    !name.is_empty()
                        && Path::new(&document.source)
                            .file_name()
                            .is_some_and(|source| source == name.as_str())
                });
                match taken {
                    Some(document) => {
                        ui.label(format!("takes this file: {} edits so far", document.edits));
                    }
                    None if !name.is_empty() => {
                        ui.colored_label(
                            Color32::from_rgb(235, 190, 110),
                            format!("no fight there plays {name}"),
                        );
                    }
                    None => {}
                }
            }
            None => {
                let why = self.link.trouble().unwrap_or("looking");
                ui.label(
                    RichText::new(format!("Not connected: {why}")).small().color(Color32::from_gray(150)),
                );
            }
        }
        match self.link.last_push() {
            _ if self.link.is_waiting() => {
                ui.label("Pushing...");
            }
            Some(Pushed::Took { owner, milliseconds }) => {
                ui.colored_label(
                    Color32::from_rgb(120, 214, 130),
                    format!("Took it in {milliseconds} ms: {owner}"),
                );
            }
            Some(Pushed::Refused(refusals)) => {
                ui.colored_label(
                    Color32::from_rgb(255, 130, 115),
                    format!("Refused, {} problems: it keeps what it had. See the timeline.", refusals.len()),
                );
            }
            Some(Pushed::Failed(why)) => {
                ui.colored_label(Color32::from_rgb(235, 190, 110), format!("Not pushed: {why}"));
            }
            None => {}
        }
        if let Some(earlier) = self.link.earlier_push() {
            let came_to = match &earlier.pushed {
                Pushed::Took { .. } => "taken".to_owned(),
                Pushed::Refused(refusals) => format!("refused, {} problems", refusals.len()),
                Pushed::Failed(why) => format!("not pushed: {why}"),
            };
            ui.label(
                RichText::new(format!("An earlier save of {}: {came_to}", earlier.origin))
                    .small()
                    .color(Color32::from_gray(150)),
            );
        }
        let attempts = self.link.attempts();
        if !attempts.is_empty() {
            ui.add_space(3.0);
            ui.label(RichText::new("Last attempts").small().strong());
            for attempt in attempts {
                self.attempt_row(ui, attempt);
            }
        }
    }

    pub(super) fn attempt_row(&self, ui: &mut Ui, attempt: &MoveAttempt) {
        let newest = attempt.revision == attempt.newest_revision;
        let word = if newest { "newest" } else { "older" };
        ui.label(
            RichText::new(format!(
                "actor {}: {} rev {} ({word})",
                attempt.actor, attempt.name, attempt.revision
            ))
            .small(),
        );
        let windows: Vec<String> =
            attempt.hit_windows.iter().map(|window| format!("{}..{}", window.from, window.to)).collect();
        if !windows.is_empty() {
            ui.label(
                RichText::new(format!("   hits {}", windows.join(", ")))
                    .small()
                    .color(Color32::from_gray(160)),
            );
        }
    }
}
