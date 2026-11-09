//! The wipe that hides a zone's load (M3 W4): a retained screen (`content/ui/zone_wipe.ui.ron`) over everything,
//! its machine driven by where the fight's trip has got (`game_combat::fight::trip`).
//!
//! A game's flow asks the same of it for a restart (M3 G1): cover, stand the new run, clear ([`WipeAsk`]).
//!
//! The trip asks it to cover as it starts, and waits for its `covered` state before the swap; once the new
//! zone's first cells are in, it asks it to uncover, and the trip ends when it's back to `clear`. Its clock is
//! the real time the fight's camera was given ([`FightShow::take_trip_time`](game_combat::fight::FightShow::take_trip_time)),
//! which in a capture is the steps' sixtieths, so a frame at a tick shows the same wipe every run. The tree
//! takes what it's told on a frame it draws, dated when the trip moved, and says it's covered a frame after it is.
//!
//! With no trip and nothing on screen, it draws nothing and costs a branch. One look for every trip, and
//! that look's a first draft: the document's to change, not this.

use game_combat::fight::trip::TripPhase;
use sapphire_assets::AssetId;
use sapphire_look::{UiDrawList, UiPageState};
use sapphire_platform::{AppInitError, Timestamp};
use sapphire_render::quality::TierId;
use sapphire_render::{PipelineInventory, Renderer};
use sapphire_rhi::GpuContext;
use sapphire_stage::content::{Content, ScreenAssets};
use sapphire_stage::scene::stream_settings;
use sapphire_ui::Atlas;

use crate::hud::ShownFight;
use crate::screen::{ScreenStack, StackSetup};

/// The wipe's document: `content/ui/zone_wipe.ui.ron`, by its sidecar's ID.
pub const ZONE_WIPE: AssetId = AssetId::from_text("6b1d4e9a-2f73-4c58-9e06-d47a83b5c912");

/// How many points high the wipe lays out on any target, so its slats' offsets clear any screen's width.
const POINTS_HIGH: f32 = 720.0;

/// Loads the wipe and covers `shown`'s zone trips with it, if its fight goes between zones
/// ([`Fight::travels`](game_combat::fight::Fight::travels)), streaming zones as `tier` does: once the fight's
/// set, before its first frame. It draws from `atlas`, the one the HUD's and any screen's frame has. What it
/// draws through is built here, so its first frame builds nothing. Whether it did.
pub fn cover_trips(
    shown: &mut ShownFight,
    atlas: Atlas,
    content: &mut Content,
    renderer: &mut Renderer,
    gpu: &GpuContext,
    tier: TierId,
) -> Result<bool, AppInitError> {
    shown.fight.show.set_zone_streaming(stream_settings(tier));
    if !shown.fight.travels() {
        return Ok(false);
    }
    shown.wipe_trips(ZoneWipe::load(atlas, content, renderer, gpu)?);
    Ok(true)
}

/// What a wipe's asked to do: cover the screen, or clear it. A trip asks by its phase ([`Self::of_trip`]),
/// and a game's flow as it restarts a run (M3 G1).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WipeAsk {
    Cover,
    Clear,
}

impl WipeAsk {
    /// What a trip in `phase` asks: to cover until its new zone's in, then to clear.
    #[must_use]
    pub fn of_trip(phase: TripPhase) -> Self {
        if phase == TripPhase::Uncovering { Self::Clear } else { Self::Cover }
    }
}

/// A wipe over a fight's stage: see the [module docs](self).
#[derive(Debug)]
pub struct ZoneWipe {
    /// Its one screen.
    layer: ScreenStack,
    /// Its clock, in seconds since it was made.
    now: f64,
    /// What it was last asked.
    asked: Option<WipeAsk>,
    /// What its machine's to be told on the next frame drawn, and when it happened on its clock.
    told: Option<(&'static str, f64)>,
    /// It has something on screen, or is about to.
    showing: bool,
}

impl ZoneWipe {
    /// The wipe from what `Content::load_screens` gave for [`ZONE_WIPE`], over `atlas`.
    pub fn new(assets: ScreenAssets, atlas: Atlas) -> Result<Self, String> {
        let setup = StackSetup { locale: "en".to_owned(), pseudo: false, scale: 1.0 };
        let document = assets.documents.first().ok_or("no wipe document")?;
        let mut layer = ScreenStack::new(assets.tables, assets.fonts, atlas, setup)?;
        layer.push(document, Box::new(()), Timestamp::from_nanos(0), Vec::new())?;
        Ok(Self { layer, now: 0.0, asked: None, told: None, showing: false })
    }

    /// [`ZONE_WIPE`], loaded, over `atlas`, with what it draws through built, so its first frame builds
    /// nothing.
    pub fn load(
        atlas: Atlas,
        content: &mut Content,
        renderer: &mut Renderer,
        gpu: &GpuContext,
    ) -> Result<Self, AppInitError> {
        let assets = content.load_screens(&[ZONE_WIPE], renderer, gpu)?;
        let wipe = Self::new(assets, atlas).map_err(AppInitError::msg)?;
        renderer.prewarm(&PipelineInventory { ui: true, ..PipelineInventory::default() });
        Ok(wipe)
    }

    /// Takes what it's asked, by a trip or a game's flow, and the real time since the last call, before the
    /// frame. What it's told starts when the ask changed, however many steps went by with no frame drawn: a
    /// capture's frame shows the same wipe whichever frames came before it.
    pub fn follow(&mut self, ask: Option<WipeAsk>, dt: f32) {
        let at = self.now;
        if self.showing || ask.is_some() {
            self.now += f64::from(dt.max(0.0));
        }
        match (self.asked, ask) {
            (None | Some(WipeAsk::Clear), Some(WipeAsk::Cover)) => {
                self.told = Some(("cover", at));
                self.showing = true;
            }
            // An ask that ended at once, with nothing drawn between, too.
            (Some(WipeAsk::Cover), Some(WipeAsk::Clear) | None) => {
                self.told = Some(("uncover", at));
            }
            _ => {}
        }
        self.asked = ask;
    }

    /// Whether it covers the screen, as of its last frame.
    #[must_use]
    pub fn covered(&self) -> bool {
        self.told.is_none() && self.layer.tree(0).state() == Some("covered")
    }

    /// Whether the screen's clear of it, as of its last frame.
    #[must_use]
    pub fn clear(&self) -> bool {
        self.told.is_none() && self.layer.tree(0).state().is_none_or(|state| state == "clear")
    }

    /// What the renderer's glyph pages hold, for the next draw.
    pub fn follow_pages(&mut self, pages: UiPageState) {
        self.layer.follow_pages(pages);
    }

    /// Writes the wipe into `list`, over whatever's there, for a target of `size` pixels: nothing while it's
    /// clear and no trip asks for it.
    pub fn draw(&mut self, size: (u32, u32), list: &mut UiDrawList) {
        if !self.showing {
            return;
        }
        let now = Timestamp::from_nanos((self.now * 1e9) as u64);
        self.layer.set_now(now);
        self.layer.set_scale((size.1 as f32 / POINTS_HIGH).max(f32::MIN_POSITIVE));
        self.layer.draw(size, list);
        if let Some((event, at)) = self.told.take() {
            self.layer.tree_mut(0).fire(event, Timestamp::from_nanos((at * 1e9) as u64));
        }
        if self.asked.is_none() && self.clear() {
            self.showing = false;
        }
    }

    /// The tree, for a test of where it's got.
    #[must_use]
    pub fn tree(&self) -> &sapphire_ui::UiTree {
        self.layer.tree(0)
    }
}

#[cfg(test)]
mod tests {
    use sapphire_assets::ui::UiDocument;

    use super::*;

    const DOCUMENT: &str = include_str!("../../../content/ui/zone_wipe.ui.ron");

    #[test]
    fn the_wipe_is_named_by_its_sidecar_and_reads() {
        let sidecar = include_str!("../../../content/ui/zone_wipe.ui.ron.meta");
        assert!(sidecar.contains(&ZONE_WIPE.to_string()), "{ZONE_WIPE} isn't in {sidecar}");
        UiDocument::from_document(DOCUMENT).expect("it reads");
    }

    fn wipe() -> ZoneWipe {
        let document = UiDocument::from_document(DOCUMENT).expect("it reads");
        let table =
            sapphire_loc::StringTable { locale: "en".to_owned(), ..sapphire_loc::StringTable::default() };
        let assets = ScreenAssets { documents: vec![document], tables: vec![table], fonts: Vec::new() };
        ZoneWipe::new(assets, Atlas::paint()).expect("a wipe")
    }

    /// Frames of 1/60 s, following `phase`, until `until` or a second's gone.
    fn frames(wipe: &mut ZoneWipe, phase: Option<TripPhase>, until: impl Fn(&ZoneWipe) -> bool) -> u32 {
        let mut list = UiDrawList::default();
        for frame in 0..60 {
            wipe.follow(phase.map(WipeAsk::of_trip), 1.0 / 60.0);
            if until(wipe) {
                return frame;
            }
            list.reset();
            wipe.draw((320, 180), &mut list);
        }
        panic!("it never got there: {:?}", wipe.tree().state());
    }

    #[test]
    fn a_trip_covers_then_clears_and_an_idle_wipe_draws_nothing() {
        let mut wipe = wipe();
        let mut list = UiDrawList::default();
        wipe.follow(None, 1.0 / 60.0);
        wipe.draw((320, 180), &mut list);
        assert!(list.quads.is_empty(), "nothing with no trip");
        assert!(wipe.clear());

        let covering = frames(&mut wipe, Some(TripPhase::Covering), ZoneWipe::covered);
        // Its slats take 0.32 s, staggered by 0.04 s: 0.52 s, and a frame to say so.
        assert!((30..=34).contains(&covering), "{covering} frames to cover");
        assert!(!wipe.clear());
        frames(&mut wipe, Some(TripPhase::Swapping), ZoneWipe::covered);
        let uncovering = frames(&mut wipe, Some(TripPhase::Uncovering), ZoneWipe::clear);
        assert!((30..=34).contains(&uncovering), "{uncovering} frames to clear");
        wipe.follow(None, 1.0 / 60.0);
        list.reset();
        wipe.draw((320, 180), &mut list);
        list.reset();
        wipe.follow(None, 1.0 / 60.0);
        wipe.draw((320, 180), &mut list);
        assert!(list.quads.is_empty(), "nothing once it's clear again");
    }

    #[test]
    fn a_trip_begun_while_it_uncovers_covers_again() {
        let mut wipe = wipe();
        frames(&mut wipe, Some(TripPhase::Covering), ZoneWipe::covered);
        let mut list = UiDrawList::default();
        for _ in 0..5 {
            wipe.follow(Some(WipeAsk::Clear), 1.0 / 60.0);
            wipe.draw((320, 180), &mut list);
        }
        assert_eq!(wipe.tree().state(), Some("uncovering"));
        frames(&mut wipe, Some(TripPhase::Covering), ZoneWipe::covered);
    }
}
