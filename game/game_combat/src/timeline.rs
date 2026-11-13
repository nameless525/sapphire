//! A timeline on a fight's stage (M3 Q1): the little that joins [`TimelinePlayer`]'s plain values to the
//! director. [`FightShow`](crate::fight::FightShow) adds the frame's post and cues, and an app's loop asks it whether
//! the simulation is held. The rest of it, and what a skip means, are in `sapphire_sequencer`'s docs. A track's
//! [`CharacterCue`] becomes what a fighter's graph plays in its combat slot ([`staged_row`], M3 Q3), for
//! fighters posed by their graphs, where it looks, and what its face shows (M5 C5), for the fighter its role
//! names ([`fighter_of_role`]).

use std::sync::Arc;

use sapphire_anim::CombatInput;
use sapphire_camera::{CameraDirector, CameraPose, ShotEye, TakeoverTerms};
use sapphire_core::Name;
use sapphire_gameplay::Simulation;
use sapphire_sequencer::{CharacterCue, Origin, StepClock, TimelinePlayer};
use sapphire_timeline::{EyeRule, Timeline};

use crate::arena::{ArenaActors, FighterKind};

/// The roles a fight's timelines name for its two fighters.
const HERO: Name = Name::new("hero");
const FOE: Name = Name::new("foe");

/// Set in a staged row's serial, which no simulation move's reaches.
const STAGED_SERIAL: u32 = 1 << 31;

/// The fighter a timeline's or a conversation's role names: `hero` is the fight's player, the fighter drawn as
/// its playable kind (M5 C7), and `foe` whoever they face, the second fighter, as since M3 Q3; and since M5 C5
/// any other role names a kind of enemy by its name (`enemy-warrior`, `boss-captain`), and binds the first
/// fighter of that kind.
pub(crate) fn fighter_of_role(role: Name, arena: &ArenaActors) -> Option<usize> {
    let count = arena.fighter_count();
    if role == HERO {
        return (0..count).find(|&fighter| arena.kind(fighter) == FighterKind::Playable);
    }
    if role == FOE {
        return (count > 1).then_some(1);
    }
    let kind = FighterKind::Enemy(arena.kind_named(role)?);
    (0..count).find(|&fighter| arena.kind(fighter) == kind)
}

/// What a fighter's graph plays in its combat slot for `cue`: its track's row there, if any.
pub(crate) fn staged_row(cue: &CharacterCue) -> Option<CombatInput> {
    Some(CombatInput {
        name: cue.row?,
        // Apart from every serial the simulation's moves count, so that a row's start, and the move after it,
        // are each a transition.
        serial: STAGED_SERIAL | cue.serial,
        position: cue.position_ticks,
    })
}

/// What [`TimelineShow::present`] did this frame.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct Shown {
    /// The real seconds it moved by.
    pub(crate) dt: f32,
    /// The view jumped between shots, so no frame's history is any use to the next.
    pub(crate) cut: bool,
}

/// One timeline playing on the stage.
#[derive(Debug)]
pub(crate) struct TimelineShow {
    player: TimelinePlayer,
    /// The real seconds since the last frame, unscaled, and held by a pause.
    dt: f32,
    /// The steps the simulation's run has taken, while it has one.
    steps_taken: Option<u32>,
    /// Whether a skip may let it go: not a move's cinematic (M3 Q3b), which is the move's own steps.
    skippable: bool,
}

impl TimelineShow {
    pub(crate) fn new(timeline: Arc<Timeline>, origin: Origin) -> Self {
        Self { player: TimelinePlayer::new(timeline, origin), dt: 0.0, steps_taken: None, skippable: true }
    }

    /// The same, for a move's cinematic: placed on its owner, and with no skip.
    pub(crate) fn of_move(timeline: Arc<Timeline>, origin: Origin) -> Self {
        Self { skippable: false, ..Self::new(timeline, origin) }
    }

    pub(crate) fn skippable(&self) -> bool {
        self.skippable
    }

    pub(crate) fn player(&self) -> &TimelinePlayer {
        &self.player
    }

    pub(crate) fn player_mut(&mut self) -> &mut TimelinePlayer {
        &mut self.player
    }

    pub(crate) fn holds_simulation(&self) -> bool {
        self.player.holds_simulation()
    }

    /// What the track whose role names `fighter` says at the playhead, if one does: the first in the
    /// document's order ([`fighter_of_role`]). Allocates nothing.
    pub(crate) fn cue_of_fighter(&self, fighter: usize, arena: &ArenaActors) -> Option<CharacterCue> {
        let role = self.player.roles().find(|&role| fighter_of_role(role, arena) == Some(fighter))?;
        self.player.character(role)
    }

    /// This much real time for the next frame.
    pub(crate) fn drive(&mut self, dt: f32) {
        self.dt += dt;
    }

    /// Where the simulation's run has got to, after a step.
    pub(crate) fn follow(&mut self, simulation: &Simulation) {
        self.steps_taken = simulation.timeline().map(|run| run.tick);
    }

    /// Moves the playhead to a frame drawn at `alpha`, and gives `director` its shot, or, over or skipped,
    /// lets it go on the blend out. The director's blend goes on from there with no one watching it.
    pub(crate) fn present(&mut self, alpha: f32, director: Option<&mut CameraDirector>) -> Shown {
        let dt = std::mem::take(&mut self.dt);
        let steps = self.steps_taken.map(|taken| StepClock { taken, alpha });
        self.player.advance(dt, steps);
        let mut shown = Shown { dt, cut: false };
        let (Some(director), Some(terms)) = (director, self.player.camera_terms()) else { return shown };
        let eye = match terms.eye {
            EyeRule::Exact => ShotEye::Exact,
            EyeRule::Clear => ShotEye::Clear,
        };
        match self.player.shot() {
            Some(shot) => {
                director.set_takeover_terms(TakeoverTerms { eye, blend_half_life: terms.blend_in });
                director.set_takeover(Some(CameraPose {
                    eye: shot.eye,
                    rotation: shot.rotation,
                    fov_y: shot.fov_y,
                }));
                if shot.cut {
                    director.cut_takeover();
                    shown.cut = true;
                }
            }
            None => {
                director.set_takeover_terms(TakeoverTerms { eye, blend_half_life: terms.blend_out });
                director.set_takeover(None);
            }
        }
        shown
    }
}
