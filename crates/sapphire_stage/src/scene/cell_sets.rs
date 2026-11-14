//! Visible sets by cell on the open zone (M5 D0's prototype, `dev` builds): which of the zone's placed meshes
//! the main view may skip, from the cell its camera stands in.
//!
//! The sets come from a file beside a cook (`sapphire_zone::visibility`), checked against the zone once it's
//! ready. Each frame the main camera's eye, moved into the zone's space, finds its cell, and every placed mesh
//! that cell's set leaves out goes into [`FramePacket::main_view_hidden`]. A camera outside the space the
//! sets sampled ([`Outside`]) hides nothing: the frame is the frustum's alone. Shadows, insets and anything
//! but the main view and its prepass draw as they would.

use sapphire_render::FramePacket;
use sapphire_world::streaming::Streamer;
use sapphire_zone::ArchivedZonePresentation;
use sapphire_zone::visibility::CellVisibility;
pub use sapphire_zone::visibility::Outside;

use super::LOG_TARGET;
use super::zone::ZoneEntity;
use crate::extract::instance_id;

/// Where the main camera stood last frame, as the sets read it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CellSetsUse {
    /// The sets don't fit the zone, or no zone is ready yet: nothing is hidden.
    Unchecked,
    /// In cell `index`'s sampled space: its set hid what it leaves out.
    Cell(usize),
    /// Outside the space sampled: the frustum alone.
    Outside(Outside),
}

/// The sets, and what each cell's first object is numbered.
pub struct CellSets {
    sets: CellVisibility,
    firsts: Vec<usize>,
    /// Whether they fit the zone open, once it's been asked: `None` until then.
    fits: Option<bool>,
    last: CellSetsUse,
}

impl CellSets {
    /// Sets to hide by.
    #[must_use]
    pub fn new(sets: CellVisibility) -> Self {
        let firsts = sets.firsts();
        Self { sets, firsts, fits: None, last: CellSetsUse::Unchecked }
    }

    /// Sets from a file's `bytes`, by object, or with `by_cells` a whole cell at a time.
    pub fn read(bytes: &[u8], by_cells: bool) -> Result<Self, String> {
        let sets = CellVisibility::from_bytes(bytes).map_err(|error| error.to_string())?;
        Ok(Self::new(if by_cells { sets.by_cells() } else { sets }))
    }

    /// What the last frame did.
    #[must_use]
    pub fn last(&self) -> CellSetsUse {
        self.last
    }

    /// Checks them against the zone once, logging a mismatch, after which they hide nothing.
    pub(super) fn check(&mut self, zone: &ArchivedZonePresentation) {
        if self.fits.is_none() {
            let fits = self.sets.check_fits(zone);
            if let Err(error) = &fits {
                tracing::error!(target: LOG_TARGET, %error, "the visible sets hide nothing");
            }
            self.fits = Some(fits.is_ok());
        }
    }

    /// Names what the main view may skip in `packet`, from `eye` in the zone's space.
    pub(super) fn hide(&mut self, eye: [f32; 3], streamer: &Streamer<ZoneEntity>, packet: &mut FramePacket) {
        packet.main_view_hidden.clear();
        if self.fits != Some(true) {
            self.last = CellSetsUse::Unchecked;
            return;
        }
        let from = match self.sets.cell_for(eye) {
            Ok(from) => from,
            Err(outside) => {
                self.last = CellSetsUse::Outside(outside);
                return;
            }
        };
        self.last = CellSetsUse::Cell(from);
        for (cell, placed) in streamer.placed_by_cell() {
            let first = self.firsts[cell];
            let objects = self.sets.cells[cell].objects.len();
            // A cell's lights come after its objects and are never hidden.
            for (item, entity) in placed.iter().enumerate().take(objects) {
                if !self.sets.sees(from, first + item) {
                    packet.main_view_hidden.push(instance_id(entity.entity));
                }
            }
        }
    }
}
