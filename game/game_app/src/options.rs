//! What the game takes on its command line: only what a run with nobody playing needs (see the crate's docs).

use std::path::PathBuf;

use game_combat::combat::CombatOption;
use game_flow::capture::GameCapture;
use game_flow::flow::FlowCommand;
use game_flow::game::GameSetup;
use game_flow::progress::progress_store;
use game_flow::show::GameShow;
use game_ui::menus::PlayerSettings;
use sapphire_platform::{AppInitError, Platform, durable_replace};

const CAPTURE_FLAG: &str = "--capture";

/// A run's arguments.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Options {
    /// What the title's chosen at once: a new game, or a continue.
    pub choose: Option<FlowCommand>,
    pub script: Option<String>,
    /// Where the game's save is kept, in place of the platform's directory for it.
    pub save_dir: Option<PathBuf>,
    /// Where each run's replay is kept.
    pub record: Option<PathBuf>,
    pub capture: Option<PathBuf>,
    pub size: (u32, u32),
    pub ticks: u32,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            choose: None,
            script: None,
            save_dir: None,
            record: None,
            capture: None,
            size: (1280, 720),
            ticks: 180,
        }
    }
}

/// Whether an argument asks for no window.
pub(crate) fn is_offscreen_flag(arg: &str) -> bool {
    arg.split_once('=').map_or(arg, |(name, _)| name) == CAPTURE_FLAG
}

impl Options {
    /// `args`, the program's name first. What it doesn't know it refuses: the platform's own arguments never
    /// reach here, and a typo in an unattended run should say so.
    pub(crate) fn parse(args: &[String]) -> Result<Self, String> {
        let mut options = Self::default();
        let mut args = args.iter().skip(1);
        while let Some(arg) = args.next() {
            let mut value =
                || args.next().ok_or_else(|| format!("{arg} needs a value")).map(|value| value.as_str());
            match arg.as_str() {
                // `--open-fight` is M3 S2's name for it, from before the flow.
                "--new-game" | "--open-fight" => options.choose = Some(FlowCommand::NewGame),
                "--continue" => options.choose = Some(FlowCommand::Continue),
                "--script" => options.script = Some(value()?.to_owned()),
                "--save-dir" => options.save_dir = Some(PathBuf::from(value()?)),
                "--record" => options.record = Some(PathBuf::from(value()?)),
                CAPTURE_FLAG => options.capture = Some(PathBuf::from(value()?)),
                "--size" => {
                    let raw = value()?;
                    let size = raw
                        .split_once('x')
                        .and_then(|(width, height)| Some((width.parse().ok()?, height.parse().ok()?)));
                    options.size = size
                        .filter(|&(width, height)| width > 0 && height > 0)
                        .ok_or_else(|| format!("--size expects <w>x<h>, got {raw:?}"))?;
                }
                "--ticks" => {
                    let raw = value()?;
                    options.ticks =
                        raw.parse().map_err(|_| format!("--ticks expects a count, got {raw:?}"))?;
                }
                _ => return Err(format!("unknown argument {arg:?}")),
            }
        }
        Ok(options)
    }

    /// Whether the game keeps a save: always in a window, and in a capture only where `--save-dir` says, so
    /// a run nobody's watching leaves nobody's save changed.
    fn saves(&self) -> bool {
        self.capture.is_none() || self.save_dir.is_some()
    }

    /// The show this run stands with `settings`, and the run it stands first: the game's flow over its runs
    /// where the content names one, with its save where `--save-dir` or the platform says, else the arena's
    /// fight alone.
    pub(crate) fn show(
        &self,
        platform: &Platform,
        settings: PlayerSettings,
    ) -> Result<(GameShow, CombatOption), AppInitError> {
        let dir = self.save_dir.clone().unwrap_or_else(|| platform.user_dir().to_owned());
        let setup = GameSetup {
            store: self.saves().then(|| progress_store(&dir, durable_replace)),
            script: self.script.clone(),
            record: self.record.clone(),
        };
        GameShow::from_content(platform.content(), settings, setup)
    }

    /// What `--capture` draws.
    pub(crate) fn capture(&self) -> GameCapture {
        GameCapture { choose: self.choose, size: self.size, ticks: self.ticks }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(args: &[&str]) -> Result<Options, String> {
        let args: Vec<String> = ["game_app"].iter().chain(args).map(ToString::to_string).collect();
        Options::parse(&args)
    }

    #[test]
    fn a_run_with_no_arguments_waits_at_the_title_for_someone_to_play() {
        let options = parse(&[]).unwrap();
        assert_eq!(options.choose, None);
        assert!(options.saves(), "a window's game keeps its save");
    }

    #[test]
    fn a_capture_says_what_it_draws_and_keeps_no_save_unless_told_where() {
        let options =
            parse(&["--capture", "a.png", "--size", "640x360", "--ticks", "9", "--new-game"]).unwrap();
        assert_eq!(options.capture, Some(PathBuf::from("a.png")));
        assert_eq!((options.size, options.ticks), ((640, 360), 9));
        assert_eq!(
            options.capture(),
            GameCapture { choose: Some(FlowCommand::NewGame), size: (640, 360), ticks: 9 }
        );
        assert!(!options.saves());
        assert!(is_offscreen_flag("--capture") && is_offscreen_flag("--capture=a.png"));
        assert!(!is_offscreen_flag("--script"));
        let options =
            parse(&["--capture", "a.png", "--save-dir", "saves", "--continue", "--record", "runs"]).unwrap();
        assert!(options.saves());
        assert_eq!(options.choose, Some(FlowCommand::Continue));
        assert_eq!(options.record, Some(PathBuf::from("runs")));
        assert_eq!(parse(&["--open-fight"]).unwrap().choose, Some(FlowCommand::NewGame));
    }

    #[test]
    fn what_it_cant_read_it_says() {
        assert!(parse(&["--stage"]).is_err());
        assert!(parse(&["--size", "640"]).is_err());
        assert!(parse(&["--size", "0x10"]).is_err());
        assert!(parse(&["--ticks", "many"]).is_err());
        assert!(parse(&["--script"]).is_err());
        assert!(parse(&["--save-dir"]).is_err());
    }
}
