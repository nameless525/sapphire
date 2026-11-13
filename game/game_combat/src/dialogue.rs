//! The game's dialogue runner (M3 Q2b): which conversation is open, walking it as a panel says, and what it
//! does to the simulation.
//!
//! A [`DialogueRunner`] holds the cooked dialogues whose simulation halves the run loaded, in the order it
//! loaded them, so a [`DialogueId`] is a place in both (`Fight::hold_dialogues` finds them in the packs by
//! the digests the simulation pins). It opens one when a zone's trigger fires the event the dialogue's
//! `opens_on` names, or when something asks for it by ID ([`DialogueRunner::open`]), and from then on a
//! panel reads [`DialogueRunner::view`] and drives [`DialogueRunner::next`] and [`DialogueRunner::choose`].
//!
//! **What reaches the simulation:** a condition is asked of the run's script VM on a read-only frame, and an
//! effect goes through [`DialogueHost::apply`], which for a recording records it as
//! `ReplayCommand::Dialogue`: the app's one route for commands, so what the panel chose replays. A replay
//! opens nothing, since it applies what it recorded by itself. All of this happens between steps, and the
//! fight holds its steps while a conversation's open (`Fight::holds_step`), so a choice lands between the
//! same two ticks it was made between.
//!
//! **What ends one:** [`DialogueRunner::take_ended`] says once that the conversation closed, with a
//! [`DialogueEnded`]: what a game's flow listens for to go on (G1).
//!
//! **Voice (A1's seam, M5 C6's voice):** each line a conversation enters moves [`DialogueRunner::lines_shown`]
//! on, and the view then has its `vo`, a key of the voice lines' document. [`DialogueRunner::speech`] asks the
//! audio system for it by that count, every frame the line's shown, so a new line, even the same line again,
//! cuts the one before and starts from its beginning, and a conversation's end cuts it. Nothing here plays it.

use std::sync::Arc;

use sapphire_audio::{CueTarget, LineKey, SpeechRequest};
use sapphire_dialogue::{ConditionId, Dialogue, DialogueView, DialogueWalk, DialogueWorld, EffectId};
use sapphire_gameplay::replay::Recording;
use sapphire_gameplay::zone::ZoneId;
use sapphire_gameplay::{DialogueEffect, DialogueId, Simulation};
use sapphire_platform::AppInitError;
use sapphire_render::Renderer;
use sapphire_rhi::GpuContext;
use sapphire_script::{ConditionSet, ScriptHost, ask};
use sapphire_stage::content::Content;

use crate::LOG_TARGET;
use crate::combat::Combat;

/// What a conversation runs against: a run whose simulation loaded the dialogue's effects, and whose VM, if
/// it has one, asks the conditions. [`Combat`] is the app's; a [`Recording`] is a test's or a tool's.
pub trait DialogueHost {
    /// The simulation as it stands.
    fn simulation(&self) -> &Simulation;

    /// Asks one of a bound set's conditions, between steps, read-only: false where nothing can ask it.
    fn ask(&mut self, conditions: &mut ConditionSet, id: ConditionId) -> bool;

    /// Applies an effect between steps, through whatever this run records commands by.
    ///
    /// # Errors
    ///
    /// If this run can't take it, saying why.
    fn apply(&mut self, effect: DialogueEffect) -> Result<(), String>;

    /// Whether a conversation may open on this run: not on a replay, which applies what it recorded.
    fn takes_dialogue(&self) -> bool {
        true
    }
}

impl DialogueHost for Recording {
    fn simulation(&self) -> &Simulation {
        Recording::simulation(self)
    }

    fn ask(&mut self, conditions: &mut ConditionSet, id: ConditionId) -> bool {
        ask(self, conditions, id)
    }

    fn apply(&mut self, effect: DialogueEffect) -> Result<(), String> {
        self.apply_dialogue_effect(effect).map_err(|error| error.to_string())
    }
}

impl DialogueHost for Combat {
    fn simulation(&self) -> &Simulation {
        Combat::simulation(self)
    }

    fn ask(&mut self, conditions: &mut ConditionSet, id: ConditionId) -> bool {
        self.ask_condition(conditions, id)
    }

    fn apply(&mut self, effect: DialogueEffect) -> Result<(), String> {
        self.apply_dialogue_effect(effect)
    }

    fn takes_dialogue(&self) -> bool {
        !self.is_replay()
    }
}

/// A conversation that closed: see the [module docs](self).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DialogueEnded {
    /// Which of the run's dialogues it was.
    pub dialogue: DialogueId,
    /// The tick it closed before.
    pub tick: u64,
}

/// Why a conversation didn't open, or stopped.
#[derive(Debug, thiserror::Error)]
pub enum DialogueRunError {
    #[error("the run loaded no dialogue {}", .0.raw())]
    Unknown(DialogueId),
    #[error("dialogue {} asks conditions, and this run has no script VM to ask them", .0.raw())]
    NoScripts(DialogueId),
    #[error("dialogue {}'s conditions: {problem}", .dialogue.raw())]
    Conditions { dialogue: DialogueId, problem: String },
    #[error("dialogue {}'s effect didn't take: {problem}", .dialogue.raw())]
    Effect { dialogue: DialogueId, problem: String },
    #[error("a replay opens no conversation: it applies what it recorded")]
    Replay,
}

/// The conversation that's open: its conditions are the runner's, by its ID.
struct Open {
    id: DialogueId,
    dialogue: Arc<Dialogue>,
    walk: DialogueWalk,
}

/// How far each dialogue's trigger had fired when last looked at, and on what.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Watched {
    zone: Option<ZoneId>,
    tick: u64,
    fires: u32,
}

/// See the [module docs](self).
#[derive(Default)]
pub struct DialogueRunner {
    /// By [`DialogueId`].
    dialogues: Vec<Arc<Dialogue>>,
    /// By [`DialogueId`], for those with an `opens_on`.
    watched: Vec<Watched>,
    /// By [`DialogueId`]: each one's conditions, bound in the run's VM by [`Self::bind`] before its first
    /// step, so an open compiles nothing. Empty for one with none, or not bound yet.
    conditions: Vec<ConditionSet>,
    open: Option<Open>,
    ended: Option<DialogueEnded>,
    lines: u64,
}

/// A walk's view of the run, for as long as one call into the walk lasts.
struct Asking<'a, H: DialogueHost + ?Sized> {
    host: &'a mut H,
    conditions: &'a mut ConditionSet,
    id: DialogueId,
    /// The first effect that didn't take.
    failed: Option<String>,
}

impl<H: DialogueHost + ?Sized> DialogueWorld for Asking<'_, H> {
    fn ask(&mut self, condition: ConditionId) -> bool {
        self.host.ask(self.conditions, condition)
    }

    fn apply(&mut self, effect: EffectId) {
        if let Err(problem) = self.host.apply(DialogueEffect { dialogue: self.id, effect }) {
            self.failed.get_or_insert(problem);
        }
    }
}

impl DialogueRunner {
    /// A runner over the run's dialogues, by [`DialogueId`]: the cooked dialogue whose simulation half each
    /// `add_dialogue` loaded. Nothing's open.
    #[must_use]
    pub fn new(dialogues: Vec<Arc<Dialogue>>) -> Self {
        let watched = vec![Watched::default(); dialogues.len()];
        let conditions = dialogues.iter().map(|_| ConditionSet::default()).collect();
        Self { dialogues, watched, conditions, open: None, ended: None, lines: 0 }
    }

    /// Binds every dialogue's conditions in `host`'s VM, once a run's attached and before its first step, so
    /// a conversation that opens later, a game's beat's included, binds and compiles nothing and takes
    /// nothing from the heap. A replay, which opens none, binds none. A dialogue whose conditions don't bind
    /// is left to [`Self::open`], which tries again and says why.
    pub fn bind(&mut self, host: &(impl DialogueHost + ?Sized)) {
        if !host.takes_dialogue() {
            return;
        }
        for (raw, (dialogue, conditions)) in self.dialogues.iter().zip(&mut self.conditions).enumerate() {
            let id = DialogueId::from_raw(raw as u16);
            match bind(dialogue, id, host) {
                Ok(bound) => *conditions = bound,
                Err(error) => {
                    tracing::warn!(target: LOG_TARGET, %error, "a dialogue's conditions didn't bind")
                }
            }
        }
    }

    /// A runner over every dialogue `simulation` loaded, found among the shared pack's and `zone`'s by the
    /// digest the simulation pins, as a fight's timelines are: the production load. Loads nothing for a
    /// simulation with none.
    ///
    /// # Errors
    ///
    /// If a digest has no cooked dialogue: a cook older than the build.
    pub fn hold(
        simulation: &Simulation,
        zone: Option<&str>,
        content: &mut Content,
        renderer: &mut Renderer,
        gpu: &GpuContext,
    ) -> Result<Self, AppInitError> {
        let digests: Vec<u64> =
            (0..).map_while(|raw| simulation.dialogue_digest(DialogueId::from_raw(raw))).collect();
        if digests.is_empty() {
            return Ok(Self::default());
        }
        Ok(Self::new(content.find_dialogues(&digests, zone, renderer, gpu)?))
    }

    /// The dialogues it holds, by [`DialogueId`].
    #[must_use]
    pub fn dialogues(&self) -> &[Arc<Dialogue>] {
        &self.dialogues
    }

    /// Whether a conversation's open: the fight holds its steps while one is.
    #[must_use]
    pub const fn is_open(&self) -> bool {
        self.open.is_some()
    }

    /// Which dialogue's open, if one is.
    #[must_use]
    pub fn open_dialogue(&self) -> Option<DialogueId> {
        self.open.as_ref().map(|open| open.id)
    }

    /// What a panel shows now: `None` with nothing open.
    #[must_use]
    pub fn view(&self) -> Option<DialogueView<'_>> {
        let open = self.open.as_ref()?;
        open.walk.view(&open.dialogue)
    }

    /// How many lines conversations have shown, the open one's included: a voice player starts the view's
    /// `vo` when this moves on.
    #[must_use]
    pub const fn lines_shown(&self) -> u64 {
        self.lines
    }

    /// The voice of the line shown (M5 C6), if it has a `vo`, numbered by [`Self::lines_shown`], and `held`
    /// while a pause holds the conversation: what the audio system's [`SpeechRequest`] takes each frame.
    #[must_use]
    pub fn speech(&self, held: bool) -> Option<SpeechRequest> {
        let vo = self.view()?.vo?;
        Some(SpeechRequest { line: LineKey::of(vo), serial: self.lines, target: CueTarget::Flat, held })
    }

    /// The conversation that closed since this was last asked, once.
    pub fn take_ended(&mut self) -> Option<DialogueEnded> {
        self.ended.take()
    }

    /// Between steps: opens the conversation whose trigger has fired since this was last called, if none is
    /// open, and says what opened. A run that started over or changed zone starts counting again, so only
    /// someone walking in opens one.
    ///
    /// # Errors
    ///
    /// As [`Self::open`]: the trigger fired and the conversation couldn't open.
    pub fn follow(
        &mut self,
        host: &mut (impl DialogueHost + ?Sized),
    ) -> Result<Option<DialogueId>, DialogueRunError> {
        let simulation = host.simulation();
        let (tick, zone) = (simulation.tick(), simulation.current_zone());
        let mut fired = None;
        for (index, (dialogue, watched)) in self.dialogues.iter().zip(&mut self.watched).enumerate() {
            let Some(event) = dialogue.presentation.opens_on else { continue };
            let fires = simulation.zone().map_or(0, |held| held.fires(sapphire_core::Name::from_u64(event)));
            let again = zone != watched.zone || tick < watched.tick || fires < watched.fires;
            if !again && fires > watched.fires && fired.is_none() {
                fired = Some(DialogueId::from_raw(index as u16));
            }
            *watched = Watched { zone, tick, fires };
        }
        match fired {
            Some(id) if self.open.is_none() && host.takes_dialogue() => {
                self.open(id, host).map(|()| Some(id))
            }
            _ => Ok(None),
        }
    }

    /// Opens dialogue `id` between steps: runs its branches to the first line and applies what showing that
    /// sets. Whatever was open closes first, as it would have ended. Its conditions are those [`Self::bind`]
    /// bound in the run's VM; if that VM didn't bind them, as when nothing called it, it binds them here,
    /// compiling them, which allocates.
    ///
    /// # Errors
    ///
    /// If the run loaded no such dialogue, or it asks conditions of a run with no VM, or they don't bind, or an
    /// effect it applied didn't take, or it's a replay's. Nothing's left open.
    pub fn open(
        &mut self,
        id: DialogueId,
        host: &mut (impl DialogueHost + ?Sized),
    ) -> Result<(), DialogueRunError> {
        if !host.takes_dialogue() {
            return Err(DialogueRunError::Replay);
        }
        let index = usize::from(id.raw());
        let dialogue = Arc::clone(self.dialogues.get(index).ok_or(DialogueRunError::Unknown(id))?);
        self.close(host.simulation().tick());
        let conditions = &mut self.conditions[index];
        let bound_here =
            host.simulation().world().get_resource::<ScriptHost>().is_some_and(|vm| vm.holds(conditions));
        if dialogue.needs_scripts() && (conditions.is_empty() || !bound_here) {
            *conditions = bind(&dialogue, id, host)?;
        }
        let mut world = Asking { host, conditions, id, failed: None };
        let walk = DialogueWalk::begin(&dialogue, &mut world);
        let failed = world.failed.take();
        let tick = world.host.simulation().tick();
        tracing::info!(target: LOG_TARGET, dialogue = id.raw(), tick, "a conversation opens");
        self.open = Some(Open { id, dialogue, walk });
        self.after_move(tick, failed)
    }

    /// The panel's "next", between steps: goes on from a line with no choices. Says whether it went anywhere.
    ///
    /// # Errors
    ///
    /// If an effect a line it reached sets didn't take. The conversation closes.
    pub fn next(&mut self, host: &mut (impl DialogueHost + ?Sized)) -> Result<bool, DialogueRunError> {
        self.step_walk(host, |walk, dialogue, world| walk.next(dialogue, world))
    }

    /// The panel's answer in `slot`, between steps: applies what the choice sets, through the run's command
    /// route, and goes where it leads. Says whether there was such a choice.
    ///
    /// # Errors
    ///
    /// As [`Self::next`].
    pub fn choose(
        &mut self,
        slot: u8,
        host: &mut (impl DialogueHost + ?Sized),
    ) -> Result<bool, DialogueRunError> {
        self.step_walk(host, |walk, dialogue, world| walk.choose(dialogue, slot, world))
    }

    fn step_walk<H: DialogueHost + ?Sized>(
        &mut self,
        host: &mut H,
        go: impl FnOnce(&mut DialogueWalk, &Dialogue, &mut Asking<'_, H>) -> bool,
    ) -> Result<bool, DialogueRunError> {
        let Some(open) = &mut self.open else { return Ok(false) };
        let conditions = &mut self.conditions[usize::from(open.id.raw())];
        let mut world = Asking { host, conditions, id: open.id, failed: None };
        let went = go(&mut open.walk, &open.dialogue, &mut world);
        let failed = world.failed.take();
        let tick = world.host.simulation().tick();
        if went {
            self.after_move(tick, failed)?;
        }
        Ok(went)
    }

    /// After the walk moved: counts the line it reached, or closes a conversation that's over or failed.
    fn after_move(&mut self, tick: u64, failed: Option<String>) -> Result<(), DialogueRunError> {
        let Some(open) = &self.open else { return Ok(()) };
        if let Some(problem) = failed {
            let dialogue = open.id;
            self.close(tick);
            return Err(DialogueRunError::Effect { dialogue, problem });
        }
        if open.walk.is_over() {
            self.close(tick);
        } else {
            self.lines += 1;
        }
        Ok(())
    }

    /// Closes what's open, saying so once through [`Self::take_ended`].
    fn close(&mut self, tick: u64) {
        if let Some(open) = self.open.take() {
            tracing::info!(target: LOG_TARGET, dialogue = open.id.raw(), tick, "a conversation ends");
            self.ended = Some(DialogueEnded { dialogue: open.id, tick });
        }
    }
}

/// Binds dialogue `id`'s conditions in the VM of `host`'s run: nothing for one with none.
fn bind(
    dialogue: &Dialogue,
    id: DialogueId,
    host: &(impl DialogueHost + ?Sized),
) -> Result<ConditionSet, DialogueRunError> {
    if !dialogue.needs_scripts() {
        return Ok(ConditionSet::default());
    }
    let vm = host.simulation().world().get_resource::<ScriptHost>().ok_or(DialogueRunError::NoScripts(id))?;
    vm.bind_conditions(dialogue, &format!("=dialogue {}", id.raw()))
        .map_err(|error| DialogueRunError::Conditions { dialogue: id, problem: error.to_string() })
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use sapphire_core::Name;
    use sapphire_gameplay::replay::RunStart;
    use sapphire_script::scenarios::ward_guide;

    use super::*;
    use crate::combat::CombatOption;

    const GUIDE: DialogueId = DialogueId::from_raw(0);

    /// Walks the open conversation to its end, taking the first answer wherever there's a choice.
    fn walk(runner: &mut DialogueRunner, host: &mut Combat) {
        for _ in 0..16 {
            if !runner.is_open() {
                return;
            }
            if !runner.next(host).expect("goes on") {
                assert!(runner.choose(0, host).expect("answers"), "a line with no way on");
            }
        }
        panic!("the conversation never ended");
    }

    /// A game's run is a recording whether it keeps its file or not (M3 G1), so a conversation's conditions
    /// are asked of its VM and its choices land in it, as they do in a recorded fight. Only a fight with no
    /// file at all refuses one.
    #[test]
    fn a_games_run_takes_a_conversation_with_no_file_kept() {
        let option = CombatOption::Run {
            scenario: "dialogue-flag".to_owned(),
            start: RunStart::default(),
            script: None,
            keep: None,
        };
        let mut run = Combat::new(&option).expect("sets up");
        let mut runner = DialogueRunner::new(vec![Arc::new(ward_guide())]);
        let ward_lit = Name::new("ward_lit");
        assert_eq!(run.simulation().flags().get(ward_lit), 0);
        runner.open(GUIDE, &mut run).expect("a run takes a conversation");
        walk(&mut runner, &mut run);
        assert_eq!(run.simulation().flags().get(ward_lit), 1, "the first answer lit the ward");
        let ended = runner.take_ended().expect("it ended");
        assert_eq!((ended.dialogue, ended.tick), (GUIDE, 0));
        run.finish().expect("nothing kept");

        let mut fight = Combat::new(&CombatOption::Fight { script: None, party: false }).expect("sets up");
        let refused = fight.apply_dialogue_effect(DialogueEffect::from_raw(0));
        assert!(refused.expect_err("no file").contains("no dialogue"));
    }

    fn guide_run() -> Combat {
        let option = CombatOption::Run {
            scenario: "dialogue-flag".to_owned(),
            start: RunStart::default(),
            script: None,
            keep: None,
        };
        Combat::new(&option).expect("sets up")
    }

    /// Conditions bound as the run attaches leave an open, a game's beat's included, nothing to compile: it
    /// takes nothing from the heap, the VM's own counted apart.
    #[test]
    fn a_conversation_bound_before_the_run_opens_with_no_heap() {
        let mut run = guide_run();
        let mut runner = DialogueRunner::new(vec![Arc::new(ward_guide())]);
        runner.bind(&run);
        assert!(!runner.conditions[0].is_empty(), "the guide asks conditions");
        let (opened, counts) = sapphire_core::alloc::count_in_scope(|| runner.open(GUIDE, &mut run));
        opened.expect("it opens");
        if sapphire_core::alloc::PER_THREAD_COUNTS {
            assert_eq!(counts.allocations, 0, "{counts:?}");
        }
        walk(&mut runner, &mut run);
        assert_eq!(run.simulation().flags().get(Name::new("ward_lit")), 1, "the first answer lit the ward");
    }

    /// M5 C6: a voiced line asks for its voice by the count of lines shown, held as the caller says, and a
    /// line with none, or no conversation, asks for nothing.
    #[test]
    fn a_voiced_line_asks_for_its_voice_by_its_showing() {
        let mut run = guide_run();
        let mut runner = DialogueRunner::new(vec![Arc::new(ward_guide())]);
        assert_eq!(runner.speech(false), None);
        runner.open(GUIDE, &mut run).expect("it opens");
        assert_eq!(runner.view().and_then(|view| view.vo), Some("guide_greet"), "the guide greets first");
        let asked = runner.speech(true).expect("a voiced line");
        assert_eq!(
            (asked.line, asked.serial, asked.target, asked.held),
            (LineKey::of("guide_greet"), runner.lines_shown(), CueTarget::Flat, true)
        );
        assert!(!runner.speech(false).unwrap().held);
        runner.next(&mut run).expect("goes on");
        assert_eq!(runner.view().and_then(|view| view.vo), None);
        assert_eq!(runner.speech(false), None);
        walk(&mut runner, &mut run);
        assert_eq!(runner.speech(false), None);
    }

    /// Conditions another run's VM bound, as a runner kept for the next run holds, are bound again in this
    /// run's as it opens, so they ask this run's flags rather than reading false.
    #[test]
    fn a_conversation_bound_for_another_run_binds_again_in_this_one() {
        let first = guide_run();
        let mut runner = DialogueRunner::new(vec![Arc::new(ward_guide())]);
        runner.bind(&first);
        let mut next = guide_run();
        fn vm(run: &Combat) -> &ScriptHost {
            run.simulation().world().get_resource::<ScriptHost>().expect("a VM")
        }
        assert!(!vm(&next).holds(&runner.conditions[0]), "the first run's VM bound them");
        runner.open(GUIDE, &mut next).expect("it opens");
        assert!(vm(&next).holds(&runner.conditions[0]), "bound again in this run's");
        walk(&mut runner, &mut next);
        assert_eq!(next.simulation().flags().get(Name::new("ward_lit")), 1);
    }
}
