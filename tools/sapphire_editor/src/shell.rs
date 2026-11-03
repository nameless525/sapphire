//! The docked panes: what's where, which tab shows, and a layout that lasts.
//!
//! `egui_tiles` does the docking. Every pane has a tab, even one alone, so it has something to drag and close
//! by. A pane closed here isn't gone: [`Shell::open`] brings it back. What a pane draws is [`PaneUi`]'s, so the
//! layout is tested without an editor behind it.
//!
//! A layout is RON and isn't a document: `Tiles` keeps a hash map, so its bytes vary, and nothing commits one.
//! One that doesn't read is refused, and leaves the layout there.

use egui_tiles::{Container, LinearDir, Tile, TileId, Tiles, Tree, UiResponse};
use serde::{Deserialize, Serialize};

/// What a pane shows. There's one of each.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Pane {
    /// Every document and asset under the roots.
    Browser,
    /// The moves of the document on the timeline.
    Moves,
    /// T3's timeline.
    Timeline,
    /// The simulation's run of the selected move.
    Preview,
    /// The entry selected on the timeline.
    Inspector,
    /// Every property of the current document, whatever its kind.
    Properties,
    /// A device's cvars.
    Cvars,
    /// What a device says.
    Device,
}

impl Pane {
    pub const ALL: [Self; 8] = [
        Self::Browser,
        Self::Moves,
        Self::Timeline,
        Self::Preview,
        Self::Inspector,
        Self::Properties,
        Self::Cvars,
        Self::Device,
    ];

    /// Its tab's title.
    #[must_use]
    pub const fn title(self) -> &'static str {
        match self {
            Self::Browser => "Assets",
            Self::Moves => "Moves",
            Self::Timeline => "Timeline",
            Self::Preview => "Preview",
            Self::Inspector => "Inspector",
            Self::Properties => "Properties",
            Self::Cvars => "Cvars",
            Self::Device => "Device",
        }
    }
}

/// What draws the panes.
pub trait PaneUi {
    fn pane_ui(&mut self, ui: &mut egui::Ui, pane: Pane);
}

/// A layout that can't be read or written.
#[derive(Debug, thiserror::Error)]
pub enum LayoutError {
    #[error("writing the layout: {0}")]
    Write(#[from] ron::Error),
    #[error("reading the layout: {0}")]
    Read(#[from] ron::error::SpannedError),
}

/// The docked panes.
#[derive(Debug)]
pub struct Shell {
    pub tree: Tree<Pane>,
}

impl Shell {
    /// The first layout: documents and moves on the left, the preview over the timeline, and what's selected
    /// and what the device says on the right.
    #[must_use]
    pub fn new() -> Self {
        const LEFT: f32 = 0.16;
        const RIGHT: f32 = 0.22;
        const TIMELINE: f32 = 0.42;
        let mut tiles = Tiles::default();
        let browser = tiles.insert_pane(Pane::Browser);
        let moves = tiles.insert_pane(Pane::Moves);
        let left = tiles.insert_vertical_tile(vec![browser, moves]);
        let preview = tiles.insert_pane(Pane::Preview);
        let timeline = tiles.insert_pane(Pane::Timeline);
        let middle = tiles.insert_vertical_tile(vec![preview, timeline]);
        share(&mut tiles, middle, &[(preview, 1.0 - TIMELINE), (timeline, TIMELINE)]);
        let side = vec![
            tiles.insert_pane(Pane::Inspector),
            tiles.insert_pane(Pane::Properties),
            tiles.insert_pane(Pane::Cvars),
        ];
        let side = tiles.insert_tab_tile(side);
        let device = tiles.insert_pane(Pane::Device);
        let right = tiles.insert_vertical_tile(vec![side, device]);
        share(&mut tiles, right, &[(side, 0.65), (device, 0.35)]);
        let root = tiles.insert_horizontal_tile(vec![left, middle, right]);
        share(&mut tiles, root, &[(left, LEFT), (middle, 1.0 - LEFT - RIGHT), (right, RIGHT)]);
        Self { tree: Tree::new(egui::Id::unique("shell"), root, tiles) }
    }

    /// Nothing at all, standing in while a pass has the real one out: see [`Self::ui`]'s callers.
    #[must_use]
    pub(crate) fn taken() -> Self {
        Self { tree: Tree::empty(egui::Id::unique("no shell")) }
    }

    /// A pane's tile, if it's open.
    #[must_use]
    pub fn find(&self, pane: Pane) -> Option<TileId> {
        self.tree.tiles.find_pane(&pane)
    }

    /// Whether it's showing or a tab away.
    #[must_use]
    pub fn is_open(&self, pane: Pane) -> bool {
        self.find(pane).is_some()
    }

    /// Splits `target`'s tile in two along `direction`, with `pane` as its second half, closing it wherever it
    /// was. Nothing happens if that's `target` itself.
    pub fn split(&mut self, target: TileId, pane: Pane, direction: LinearDir) {
        if self.find(pane) == Some(target) {
            return;
        }
        self.close(pane);
        let Some(tile) = self.tree.tiles.remove(target) else { return };
        let moved = self.tree.tiles.insert_new(tile);
        let added = self.tree.tiles.insert_pane(pane);
        let container = Container::new_linear(direction, vec![moved, added]);
        // The tile keeps its ID, so whatever holds it, the root included, holds the split now.
        self.tree.tiles.insert(target, Tile::Container(container));
    }

    /// Brings a pane back, or shows it if it's a tab away. A new one goes at the end of what's at the root.
    pub fn open(&mut self, pane: Pane) {
        if let Some(tile) = self.find(pane) {
            let _ = self.tree.make_active(|id, _| id == tile);
            return;
        }
        let added = self.tree.tiles.insert_pane(pane);
        match self.tree.root() {
            None => self.tree.root = Some(added),
            Some(root) => {
                if let Some(Tile::Container(container)) = self.tree.tiles.get_mut(root) {
                    container.add_child(added);
                } else {
                    let holder = self.tree.tiles.insert_horizontal_tile(vec![root, added]);
                    self.tree.root = Some(holder);
                }
            }
        }
    }

    /// Closes a pane.
    pub fn close(&mut self, pane: Pane) {
        if let Some(tile) = self.find(pane) {
            self.tree.remove_recursively(tile);
        }
    }

    /// The layout as RON.
    pub fn save_layout(&self) -> Result<String, LayoutError> {
        Ok(ron::ser::to_string_pretty(&self.tree, ron::ser::PrettyConfig::new())?)
    }

    /// Takes a layout saved by [`Self::save_layout`]. One that doesn't read leaves this one.
    pub fn restore_layout(&mut self, layout: &str) -> Result<(), LayoutError> {
        self.tree = ron::from_str(layout)?;
        Ok(())
    }

    /// Draws the panes.
    pub fn ui(&mut self, ui: &mut egui::Ui, panes: &mut dyn PaneUi) {
        let mut behavior = Behavior { panes };
        self.tree.ui(&mut behavior, ui);
    }
}

impl Default for Shell {
    fn default() -> Self {
        Self::new()
    }
}

/// Gives a linear container's children their shares.
fn share(tiles: &mut Tiles<Pane>, container: TileId, shares: &[(TileId, f32)]) {
    if let Some(Tile::Container(Container::Linear(linear))) = tiles.get_mut(container) {
        for (child, share) in shares {
            linear.shares.set_share(*child, *share);
        }
    }
}

struct Behavior<'a> {
    panes: &'a mut dyn PaneUi,
}

impl egui_tiles::Behavior<Pane> for Behavior<'_> {
    fn pane_ui(&mut self, ui: &mut egui::Ui, _tile: TileId, pane: &mut Pane) -> UiResponse {
        self.panes.pane_ui(ui, *pane);
        UiResponse::None
    }

    fn tab_title_for_pane(&mut self, pane: &Pane) -> egui::WidgetText {
        pane.title().into()
    }

    fn is_tab_closable(&self, _tiles: &Tiles<Pane>, _tile: TileId) -> bool {
        true
    }

    /// Tabs over a single pane too, so every pane has something to drag and close by.
    fn simplification_options(&self) -> egui_tiles::SimplificationOptions {
        egui_tiles::SimplificationOptions {
            all_panes_must_have_tabs: true,
            ..egui_tiles::SimplificationOptions::default()
        }
    }
}
