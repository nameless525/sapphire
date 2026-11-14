//! A fight's cut-ins (M5 L10, `sapphire_world::cutin`): the close-up a big move cuts across the frame, asked
//! for by a cue's row, from a move's frame or a timeline's cue track, and played on the presentation's clock.
//!
//! - **Asked for:** a row of the fight's cue table names a cut-in (`cutin:`); a step's or a timeline's cue that
//!   fires it asks for it, and [`CutIns::ask`] hears it at the frame's end. The player's own cue, or one at a
//!   place (a timeline's), plays it for the kind the character draws as; an enemy's plays nothing and is counted.
//!   A cut-in asked for while one plays cuts that one.
//! - **Stood:** at the next frame's animation, which has the content: the scene stands a stand of the character
//!   as it is then, playing the entry's clip for its kind ([`Scene::stand_cutin`]), and the entry's sound is
//!   queued flat.
//! - **Its clock:** the presentation's, from the frame that asked: [`CutIns::advance`] takes the frame's view
//!   time, which hit stop and slow motion don't hold and a pause, which takes no steps, does. Steps taken with
//!   no frame drawn (a capture's way to its tick) move it as frames would, so a cut-in stood late stands as far
//!   in as it would be, and one already over stands not at all. The simulation never hears of it.
//! - **Leaving:** a cut-in finishes on its own clock when its fighter leaves (a swap, a defeat), since its stand
//!   is a copy; a fight that starts over, or a show that stops, cuts it at once ([`CutIns::cut`]).
//! - **Drawn:** under the HUD, over the game's view ([`CutIns::draw`]): its panel is one quad of the interface's
//!   list, put under what the HUD drew, so the bars and the subtitles stay readable over it. With flashes reduced
//!   it fades in place and its backdrop's lines hold still.
//!
//! Nothing is stood, drawn or allocated while none plays; a table that can play one declares the inset path to
//! the pipeline inventory, so the first draws with nothing to build, and readies its stand's springs on the
//! warm-up pool from the fight's first frames, so the first stands without fitting them.
//!
//! [`Scene::stand_cutin`]: sapphire_stage::Scene::stand_cutin

use sapphire_assets::AssetId;
use sapphire_gameplay::ActorId;
use sapphire_render::UiDrawList;
use sapphire_stage::cutin::{CUTIN_INSET, CutInShot};
use sapphire_stage::{Content, Scene};
use sapphire_world::cutin::{CutIn, CutInLibrary};

use crate::LOG_TARGET;

/// What a fight's cut-ins did (rule R9).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CutInStats {
    /// Cut-ins stood.
    pub played: u32,
    /// Cut short: by another, or a fight starting over.
    pub cut: u32,
    /// Asked for by a cue no cut-in plays for: an enemy's, or one no entry has for the kind.
    pub unplayable: u32,
}

/// One playing.
#[derive(Clone, Copy, Debug)]
struct Playing {
    /// Its entry, by its place in the library.
    entry: usize,
    seconds: f32,
}

/// A fight's cut-ins. See the [module docs](self).
#[derive(Debug)]
pub struct CutIns {
    library: CutInLibrary,
    /// The names the cue table asks for, by their place there.
    names: Vec<Box<str>>,
    /// False with `--cutins off`, which an old golden pins.
    on: bool,
    /// Asked for since the last animation: a name's place, and seconds since.
    asked: Option<(u16, f32)>,
    playing: Option<Playing>,
    /// The picture's size in pixels as the interface last drew it.
    picture: Option<(u32, u32)>,
    /// Flashes reduced: it fades in place, its lines still.
    still: bool,
    stats: CutInStats,
}

impl CutIns {
    /// `library`'s cut-ins, for a cue table whose rows ask for `names`: an error names one it hasn't.
    pub fn new(library: CutInLibrary, names: &[Box<str>]) -> Result<Self, String> {
        if let Some(missing) = names.iter().find(|name| !library.names(name)) {
            return Err(format!(
                "the cue table asks for the cut-in `{missing}`, which the cut-in library hasn't"
            ));
        }
        Ok(Self {
            library,
            names: names.to_vec(),
            on: true,
            asked: None,
            playing: None,
            picture: None,
            still: false,
            stats: CutInStats::default(),
        })
    }

    /// Whether its table can play a cut-in: what declares the inset path to the inventory.
    #[must_use]
    pub fn can_play(&self) -> bool {
        self.on && !self.names.is_empty()
    }

    /// Plays cut-ins, or not (`--cutins off`); off cuts one playing at the next animation.
    pub fn set_on(&mut self, on: bool) {
        self.on = on;
    }

    /// Fades in place with its lines still (flashes reduced), or slides as authored.
    pub fn set_still(&mut self, still: bool) {
        self.still = still;
    }

    /// Hears a row's ask for the cut-in named `name` (its place in the table's names), from `actor`'s cue or a
    /// place's: the player's, `player`, or a place's plays; another's is counted.
    pub fn ask(&mut self, name: u16, actor: Option<ActorId>, player: Option<ActorId>) {
        if !self.on {
            return;
        }
        if actor.is_some() && player.is_some() && actor != player {
            self.stats.unplayable += 1;
            return;
        }
        self.asked = Some((name, 0.0));
    }

    /// Moves a playing cut-in, and one asked for and not yet stood, on by `view_dt` seconds of the presentation's
    /// clock.
    pub fn advance(&mut self, view_dt: f32) {
        let view_dt = view_dt.max(0.0);
        if let Some(playing) = &mut self.playing {
            playing.seconds += view_dt;
        }
        if let Some((_, seconds)) = &mut self.asked {
            *seconds += view_dt;
        }
    }

    /// Stands an asked-for cut-in, poses a playing one, and takes away one that's over, on `scene` with
    /// `content`: what the frame's animation does. The sound of one it stood, to play flat this frame.
    pub fn animate(&mut self, scene: &mut Scene, content: &Content) -> Option<AssetId> {
        let mut sound = None;
        // Its stand's springs readied off the frame, so the first cut-in doesn't hitch.
        if let (true, Some(kind)) = (self.can_play(), content.character_kind()) {
            scene.prepare_cutin(content, kind);
        }
        if !self.on && self.playing.is_some() {
            self.cut(scene);
        }
        if let Some((name, seconds)) = self.asked.take() {
            sound = self.stand(scene, content, name, seconds);
        }
        let Some(playing) = self.playing else { return sound };
        let entry = &self.library.cutins()[playing.entry];
        if playing.seconds >= entry.panel.seconds() {
            scene.end_cutin();
            self.playing = None;
            self.picture = None;
            return sound;
        }
        scene.pose_cutin(content, playing.seconds);
        sound
    }

    fn stand(&mut self, scene: &mut Scene, content: &Content, name: u16, seconds: f32) -> Option<AssetId> {
        let name = self.names.get(usize::from(name))?;
        let kind = content.character_kind();
        let found = kind.and_then(|kind| {
            let entry = self.library.find(name, Some(kind))?;
            Some((kind, entry, self.library.cutins().iter().position(|cutin| std::ptr::eq(cutin, entry))?))
        });
        let Some((kind, entry, place)) = found else {
            self.stats.unplayable += 1;
            return None;
        };
        if self.playing.take().is_some() {
            self.stats.cut += 1;
            scene.end_cutin();
        }
        if seconds >= entry.panel.seconds() {
            return None;
        }
        match scene.stand_cutin(content, scene.character(), kind, entry) {
            Ok(()) => {
                self.playing = Some(Playing { entry: place, seconds });
                self.stats.played += 1;
                entry.sound
            }
            Err(error) => {
                tracing::warn!(target: LOG_TARGET, %error, cutin = %name, "a cut-in couldn't stand");
                self.stats.unplayable += 1;
                None
            }
        }
    }

    /// Cuts a playing cut-in at once: a fight starting over, or a show that stops.
    pub fn cut(&mut self, scene: &mut Scene) {
        self.asked = None;
        if self.playing.take().is_some() {
            self.stats.cut += 1;
        }
        self.picture = None;
        scene.end_cutin();
    }

    /// Draws a playing cut-in's panel for a target of `size` pixels into `list`, under the quads from `under` on:
    /// what was drawn after the game's view, the HUD's. Allocates nothing once the list has grown.
    pub fn draw(&mut self, size: (u32, u32), list: &mut UiDrawList, under: u32) {
        let Some(playing) = self.playing else { return };
        if list.viewport[0] <= 0.0 || list.viewport[1] <= 0.0 {
            list.viewport = [size.0 as f32, size.1 as f32];
        }
        let viewport = list.viewport;
        let panel = &self.library.cutins()[playing.entry].panel;
        let [x0, y0, x1, y1] = panel.picture_rect(viewport);
        let pixels = |points: f32, side: u32, across: f32| {
            ((points * side as f32 / across).round() as u32).clamp(1, side.max(1))
        };
        self.picture = Some((pixels(x1 - x0, size.0, viewport[0]), pixels(y1 - y0, size.1, viewport[1])));
        if let Some(quad) = panel.quad(playing.seconds, self.still, viewport) {
            list.insert_inset_quads(under, CUTIN_INSET, &[quad]);
        }
    }

    /// What its interface drew this frame, for the scene to shoot: `None` while none plays.
    #[must_use]
    pub fn shot(&self) -> Option<CutInShot<'_>> {
        let playing = self.playing?;
        let size = self.picture?;
        let cutin: &CutIn = &self.library.cutins()[playing.entry];
        Some(CutInShot { cutin, size, seconds: playing.seconds, still: self.still })
    }

    /// Seconds into the one playing.
    #[must_use]
    pub fn seconds(&self) -> Option<f32> {
        self.playing.map(|playing| playing.seconds)
    }

    /// The sounds its entries play: what an app loads for them.
    pub fn sounds(&self) -> impl Iterator<Item = AssetId> + '_ {
        self.library.sounds()
    }

    #[must_use]
    pub fn stats(&self) -> CutInStats {
        self.stats
    }
}

#[cfg(test)]
mod tests {
    use sapphire_render::UiQuad;

    use super::*;

    fn actor(raw: u32) -> ActorId {
        ActorId::from_raw(raw).expect("not 0")
    }

    fn cutins() -> CutIns {
        CutIns::new(CutInLibrary::default_cutins(), &["ultimate".into()]).expect("the shipped table's")
    }

    /// One playing `seconds` in, as if stood: standing needs a character's content, which these leave out.
    fn playing(seconds: f32) -> CutIns {
        let mut cutins = cutins();
        cutins.playing = Some(Playing { entry: 0, seconds });
        cutins
    }

    #[test]
    fn a_table_asking_for_a_cut_in_the_library_hasn_t_is_refused() {
        let error = CutIns::new(CutInLibrary::default_cutins(), &["nowhere".into()]).expect_err("refused");
        assert!(error.contains("`nowhere`"), "{error}");
        assert!(cutins().can_play());
        assert!(!CutIns::new(CutInLibrary::default_cutins(), &[]).expect("none asked").can_play());
    }

    #[test]
    fn the_player_s_cue_or_a_place_s_asks_and_an_enemy_s_is_counted() {
        let (player, enemy) = (Some(actor(1)), Some(actor(2)));
        let mut cutins = cutins();
        cutins.ask(0, enemy, player);
        assert_eq!((cutins.asked, cutins.stats().unplayable), (None, 1));
        cutins.ask(0, player, player);
        assert_eq!(cutins.asked, Some((0, 0.0)));
        cutins.asked = None;
        cutins.ask(0, None, player);
        assert_eq!(cutins.asked, Some((0, 0.0)), "a timeline's, at a place");
        cutins.asked = None;
        cutins.set_on(false);
        assert!(!cutins.can_play());
        cutins.ask(0, None, player);
        assert_eq!(cutins.asked, None, "pinned off");
    }

    /// The presentation's clock: a frame's view time moves it, whatever hit stop or slow motion do to the
    /// world's, and a frame that gives the view none (a pause) holds it.
    #[test]
    fn its_clock_is_the_view_s_and_a_pause_holds_it() {
        let mut played = playing(0.0);
        for _ in 0..30 {
            played.advance(1.0 / 60.0);
        }
        assert!((played.seconds().expect("playing") - 0.5).abs() < 1e-5);
        for _ in 0..30 {
            played.advance(0.0);
        }
        assert!((played.seconds().expect("playing") - 0.5).abs() < 1e-5);
        played.advance(-1.0);
        assert!((played.seconds().expect("playing") - 0.5).abs() < 1e-5, "never back");
        // None playing, nothing moves; one asked for and not yet stood counts from its ask, as a capture's
        // undrawn steps reach a cut-in it stands at its first drawn frame.
        let mut idle = cutins();
        idle.advance(1.0);
        assert_eq!(idle.seconds(), None);
        idle.ask(0, None, None);
        for _ in 0..12 {
            idle.advance(1.0 / 60.0);
        }
        let (_, waited) = idle.asked.expect("asked");
        assert!((waited - 0.2).abs() < 1e-5);
    }

    #[test]
    fn none_playing_draws_nothing_and_shoots_nothing() {
        let mut cutins = cutins();
        let mut list = UiDrawList { viewport: [640.0, 360.0], ..UiDrawList::default() };
        cutins.draw((1280, 720), &mut list, 0);
        assert!(list.quads.is_empty() && list.inset_runs.is_empty());
        assert!(cutins.shot().is_none());
    }

    #[test]
    fn a_playing_one_draws_under_the_hud_and_sizes_its_picture_in_pixels() {
        let mut cutins = playing(0.4);
        // The HUD drew one quad over the game's view, in points at half the pixels.
        let hud = UiQuad::glyph([0.0; 2], [1.0; 2], [0.0; 4], [255; 4]);
        let mut list = UiDrawList { viewport: [640.0, 360.0], ..UiDrawList::default() };
        list.quads.push(hud);
        cutins.draw((1280, 720), &mut list, 0);
        assert_eq!(list.quads.len(), 2);
        assert_eq!(list.quads[1], hud, "the HUD over it");
        assert_eq!(list.inset_runs[0].inset, CUTIN_INSET);
        let shot = cutins.shot().expect("drawn");
        let [_, y0, _, y1] = shot.cutin.panel.picture_rect([640.0, 360.0]);
        assert_eq!(shot.size, (1280, ((y1 - y0) * 2.0).round() as u32));
        assert!((shot.seconds - 0.4).abs() < 1e-6 && !shot.still);
        // With flashes reduced, early on, it fades where it holds.
        let mut still = playing(0.05);
        still.set_still(true);
        let mut list = UiDrawList { viewport: [640.0, 360.0], ..UiDrawList::default() };
        still.draw((1280, 720), &mut list, 0);
        let quad = list.quads[0];
        assert!(quad.color[3] < 255 && quad.clip == [0.0, 1.0] && quad.uv == [0.0, 0.0, 1.0, 1.0]);
        assert!(still.shot().expect("drawn").still);
        // A list no interface has drawn in yet takes the target's pixels as its points.
        let mut bare = playing(0.4);
        let mut list = UiDrawList::default();
        bare.draw((1280, 720), &mut list, 0);
        assert_eq!(list.viewport, [1280.0, 720.0]);
    }
}
