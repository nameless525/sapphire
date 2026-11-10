//! The flow on the game's screens (M3 G1): what a menu's row asks of it, and what the title and the HUD show of
//! it. The screens know nothing of the flow: [`flow_command`] reads their [`MenuCommand`]s as its own
//! [`FlowCommand`]s, and [`show_title`] and [`show_hud`] fill their models from it.

use game_ui::menus::{GameMenus, HudModel, MenuCommand};

use crate::flow::{Banner, Flow, FlowCommand};

/// What a menu's command asks of the flow: `None` for what's the interface's own (a setting moved) or the
/// dialogue runner's (a line read, an answer chosen, which it ends with [`FlowCommand::DialogueEnded`]).
#[must_use]
pub const fn flow_command(command: MenuCommand) -> Option<FlowCommand> {
    Some(match command {
        MenuCommand::NewGame => FlowCommand::NewGame,
        MenuCommand::Continue => FlowCommand::Continue,
        MenuCommand::ToTitle => FlowCommand::ToTitle,
        MenuCommand::Skip => FlowCommand::Skip,
        MenuCommand::Pause => FlowCommand::Pause,
        MenuCommand::Resume => FlowCommand::Resume,
        MenuCommand::Leave => FlowCommand::Leave,
        MenuCommand::SettingsChanged | MenuCommand::DialogueNext | MenuCommand::DialogueChose(_) => {
            return None;
        }
    })
}

/// A banner's key in the HUD's strings.
#[must_use]
pub const fn banner_key(banner: Banner) -> &'static str {
    match banner {
        Banner::Checkpoint => "hud-banner-checkpoint",
        Banner::Victory => "hud-banner-victory",
        Banner::Defeat => "hud-banner-defeat",
    }
}

/// The menus as the flow has them: whether there's a run to continue, which the title shows. It allocates
/// nothing.
pub fn show_title(flow: &Flow, menus: &mut GameMenus) {
    menus.set_can_continue(flow.can_continue());
}

/// The HUD as the flow has it: its banner, and whether what's playing can be skipped. A frame's call allocates
/// nothing: a banner's key fits in what the model's string already holds.
pub fn show_hud(flow: &Flow, model: &mut HudModel) {
    model.can_skip = flow.can_skip();
    model.has_banner = flow.banner().is_some();
    if let Some(banner) = flow.banner() {
        let key = banner_key(banner);
        if model.banner != key {
            model.banner.clear();
            model.banner.push_str(key);
        }
    }
}

#[cfg(test)]
mod tests {
    use game_ui::menus::{MenuDocument, MenuScreen, RowAction};

    use super::*;

    #[test]
    fn every_row_that_asks_the_flow_reaches_it() {
        let screens = MenuDocument::from_ron(include_str!("../../../content/ui/menus.ron")).expect("reads");
        let rows = [
            (MenuScreen::TITLE, "continue", FlowCommand::Continue),
            (MenuScreen::TITLE, "new", FlowCommand::NewGame),
            (MenuScreen::TITLE, "leave", FlowCommand::Leave),
            (MenuScreen::PAUSE, "resume", FlowCommand::Resume),
            (MenuScreen::PAUSE, "to-title", FlowCommand::ToTitle),
            (MenuScreen::PAUSE, "leave", FlowCommand::Leave),
            (MenuScreen::HUD, "pause", FlowCommand::Pause),
            (MenuScreen::HUD, "skip", FlowCommand::Skip),
        ];
        for (screen, row, wanted) in rows {
            let Some(RowAction::Command(command)) = screens.entry(screen).and_then(|entry| entry.action(row))
            else {
                panic!("{screen:?}'s {row} asks nothing");
            };
            assert_eq!(flow_command(*command), Some(wanted), "{screen:?}'s {row}");
        }
    }

    #[test]
    fn what_the_interface_and_the_dialogue_keep_never_reaches_the_flow() {
        for command in
            [MenuCommand::SettingsChanged, MenuCommand::DialogueNext, MenuCommand::DialogueChose(1)]
        {
            assert_eq!(flow_command(command), None, "{command:?}");
        }
    }

    #[test]
    fn every_banners_key_fits_in_the_models_first() {
        let held = HudModel::default().banner.capacity();
        for banner in [Banner::Checkpoint, Banner::Victory, Banner::Defeat] {
            assert!(banner_key(banner).len() <= held, "{banner:?} would grow the HUD's string");
        }
    }
}
