//! The counts panel (M5 X1): the bench's count rows for the frame, each against its spec §3.4 budget on
//! the tier, if it has one, grouped as the bench logs them. The app evaluates the rows (the sandbox's
//! `COUNT_ROWS`, one source for the bench's log and this panel) and hands them over through
//! [`OverlayFrame::counts`](crate::OverlayFrame::counts), only while the panel shows
//! ([`DebugOverlay::shows_counts`](crate::DebugOverlay::shows_counts)).

use egui::{Color32, RichText, Sense, Ui, vec2};

use crate::ui::{GOOD, OVER};

/// One count row for the frame: its group and name, its value, and its budget on the tier.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CountRowView {
    pub group: &'static str,
    pub name: &'static str,
    pub value: u64,
    /// Spec §3.4's figure for the tier; `None` where the spec gives none.
    pub budget: Option<u64>,
}

impl CountRowView {
    /// How much of its budget it uses: `None` without one.
    pub fn budget_use(&self) -> Option<f32> {
        self.budget.map(|budget| if budget == 0 { f32::INFINITY } else { self.value as f32 / budget as f32 })
    }

    /// Whether it is past its budget.
    pub fn over(&self) -> bool {
        self.budget.is_some_and(|budget| self.value > budget)
    }

    /// The color its value is drawn in: the warning color past its budget, the good one within it, plain
    /// text without one.
    pub fn color(&self) -> Option<Color32> {
        self.budget.map(|_| if self.over() { OVER } else { GOOD })
    }
}

/// `rows` as runs of one group each, in their order: the group's name and its rows.
pub fn groups(rows: &[CountRowView]) -> impl Iterator<Item = (&'static str, &[CountRowView])> {
    rows.chunk_by(|a, b| a.group == b.group).map(|run| (run[0].group, run))
}

/// The panel: each group under its name, a row a line, its value, its budget and a bar of the share
/// of it used where it has one. Rows not exercised this frame (0, without a budget) are left out.
pub(crate) fn panel(ui: &mut Ui, rows: &[CountRowView]) {
    if rows.is_empty() {
        ui.label("The app hands over no count rows.");
        return;
    }
    let over = rows.iter().filter(|row| row.over()).count();
    ui.weak(format!("Count rows against spec §3.4's budgets: {over} over"));
    egui::ScrollArea::vertical().max_height(520.0).show(ui, |ui| {
        for (group, rows) in groups(rows) {
            let shown: Vec<&CountRowView> =
                rows.iter().filter(|row| row.value > 0 || row.budget.is_some()).collect();
            if shown.is_empty() {
                continue;
            }
            ui.separator();
            ui.strong(group);
            egui::Grid::new(group).num_columns(4).striped(true).show(ui, |ui| {
                for row in shown {
                    ui.label(row.name);
                    let value = RichText::new(row.value.to_string()).monospace();
                    ui.label(match row.color() {
                        Some(color) => value.color(color),
                        None => value,
                    });
                    ui.monospace(row.budget.map_or_else(|| "—".to_owned(), |budget| budget.to_string()));
                    match row.budget_use() {
                        Some(share) => bar(ui, share, row.color().unwrap_or(GOOD)),
                        None => {
                            ui.label("");
                        }
                    }
                    ui.end_row();
                }
            });
        }
    });
}

/// A bar 80 points wide filled to `share` of its budget (full past it), in `color`.
fn bar(ui: &mut Ui, share: f32, color: Color32) {
    let (response, painter) = ui.allocate_painter(vec2(80.0, 10.0), Sense::hover());
    let rect = response.rect;
    painter.rect_filled(rect, 0.0, Color32::from_gray(48));
    let filled = rect.with_max_x(rect.min.x + rect.width() * share.clamp(0.0, 1.0));
    painter.rect_filled(filled, 0.0, color);
}

#[cfg(test)]
mod tests {
    use super::*;

    const fn row(group: &'static str, name: &'static str, value: u64, budget: Option<u64>) -> CountRowView {
        CountRowView { group, name, value, budget }
    }

    #[test]
    fn a_row_past_its_budget_warns_and_one_within_it_doesnt() {
        let under = row("frame", "draw calls", 699, Some(700));
        let at = row("frame", "draw calls", 700, Some(700));
        let over = row("frame", "draw calls", 701, Some(700));
        let none = row("frame", "triangles", 9_000_000, None);
        assert!(!under.over() && !at.over() && over.over() && !none.over(), "the budget itself is within");
        assert_eq!(
            [under.color(), at.color(), over.color(), none.color()],
            [Some(GOOD), Some(GOOD), Some(OVER), None]
        );
        assert_eq!(at.budget_use(), Some(1.0));
        assert_eq!(none.budget_use(), None);
        assert_eq!(row("lights", "lights", 1, Some(0)).budget_use(), Some(f32::INFINITY));
    }

    #[test]
    fn rows_group_in_runs_in_their_order() {
        let rows = [
            row("frame", "draw calls", 1, Some(2_500)),
            row("frame", "triangles", 2, None),
            row("passes", "main draws", 3, None),
            row("lights", "lights", 4, Some(64)),
            row("lights", "lights dropped", 0, None),
        ];
        let groups: Vec<(&str, Vec<&str>)> =
            groups(&rows).map(|(group, rows)| (group, rows.iter().map(|row| row.name).collect())).collect();
        assert_eq!(
            groups,
            [
                ("frame", vec!["draw calls", "triangles"]),
                ("passes", vec!["main draws"]),
                ("lights", vec!["lights", "lights dropped"])
            ]
        );
        assert_eq!(super::groups(&[]).count(), 0);
    }
}
