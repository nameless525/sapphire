//! The dialogue panel on a conversation (M3 Q2b): U7's screen, reading a [`DialogueRunner`].
//!
//! [`DialoguePanel::follow`] puts the panel up when a conversation opens, fills its [`DialogueModel`] from
//! the runner's view each time the walk moves, and takes it down when the conversation ends. The model holds
//! keys, which the panel's document draws through the string tables, so a line shows in whatever language
//! the player chose. [`DialoguePanel::hear`] takes the panel's two commands, `DialogueNext` and
//! `DialogueChose`, to the runner, which applies what a choice sets through the run's command route.
//!
//! A game calls [`DialoguePanel::hear`] for each command the menus give, the runner's `follow` after each
//! step, and [`DialoguePanel::follow`] before the menus draw: `game_app`'s show does.

use game_combat::dialogue::{DialogueHost, DialogueRunner};
use sapphire_dialogue::DialogueView;

use crate::LOG_TARGET;
use crate::menus::{DialogueChoice, DialogueModel, GameMenus, MenuCommand, MenuScreen};

/// See the [module docs](self).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct DialoguePanel {
    /// The runner's line count the model shows, while the panel's up.
    shown: Option<u64>,
}

impl DialoguePanel {
    /// Takes `command` to `runner`, between steps, if it's the panel's: says whether it was.
    pub fn hear(
        &mut self,
        command: MenuCommand,
        runner: &mut DialogueRunner,
        host: &mut (impl DialogueHost + ?Sized),
    ) -> bool {
        let went = match command {
            MenuCommand::DialogueNext => runner.next(host),
            MenuCommand::DialogueChose(slot) => runner.choose(slot, host),
            _ => return false,
        };
        if let Err(error) = went {
            tracing::warn!(target: LOG_TARGET, %error, "the conversation stopped");
        }
        true
    }

    /// Puts the panel up, fills it or takes it down as `runner` says, before the menus draw.
    pub fn follow(&mut self, menus: &mut GameMenus, runner: &DialogueRunner) {
        match runner.view() {
            Some(view) => {
                let lines = runner.lines_shown();
                if self.shown == Some(lines) {
                    return;
                }
                if !menus.is_open(MenuScreen::DIALOGUE) {
                    menus.open(MenuScreen::DIALOGUE);
                }
                if let Some(model) = menus.model_mut::<DialogueModel>(MenuScreen::DIALOGUE) {
                    model.show(&view);
                }
                self.shown = Some(lines);
            }
            None if self.shown.take().is_some() => menus.close(MenuScreen::DIALOGUE),
            None => {}
        }
    }

    /// Whether the panel's up on a conversation.
    #[must_use]
    pub const fn is_up(&self) -> bool {
        self.shown.is_some()
    }
}

impl DialogueModel {
    /// Shows `view`: its keys, its answers in their slots and whether it goes on by "next". Writes into the
    /// strings it has, so a line no longer than the last allocates nothing.
    pub fn show(&mut self, view: &DialogueView<'_>) {
        set(&mut self.speaker, view.speaker);
        set(&mut self.line, view.line);
        self.next = view.next;
        let slots = [&mut self.first, &mut self.second, &mut self.third, &mut self.fourth];
        for (slot, choice) in slots.into_iter().zip(view.choices) {
            show_choice(slot, choice.map(|choice| choice.line));
        }
    }
}

fn show_choice(slot: &mut DialogueChoice, line: Option<&str>) {
    slot.shown = line.is_some();
    set(&mut slot.line, line.unwrap_or_default());
}

fn set(held: &mut String, key: &str) {
    if held != key {
        held.clear();
        held.push_str(key);
    }
}
