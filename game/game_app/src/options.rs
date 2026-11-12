//! What the game takes on its command line: only what a run with nobody playing needs (see the crate's docs).

use std::path::PathBuf;

use game_combat::combat::CombatOption;
use game_flow::boot::MakeShow;
use game_flow::capture::GameCapture;
use game_flow::flow::FlowCommand;
use game_flow::game::GameSetup;
use game_flow::progress::progress_store;
use game_flow::show::GameShow;
use game_flow::update::PatchSetup;
use game_ui::hud::OffscreenDisplay;
use game_ui::menus::PlayerSettings;
use sapphire_platform::{AppInitError, ContentFs, Platform, durable_replace};
use sapphire_render::quality::QualitySettings;

use crate::build_info;

const CAPTURE_FLAG: &str = "--capture";

/// How many more frames than ticks `--at-tick` may take: ten minutes of a cutscene's held steps.
const HELD_FRAMES: u32 = 10 * 60 * 60;

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
    /// A run's file each run plays, from its start, in place of anyone's input (M4 F2).
    pub replay: Option<PathBuf>,
    /// With `--capture` and `--replay`: the run's tick whose frame it draws.
    pub at_tick: Option<u64>,
    pub capture: Option<PathBuf>,
    pub size: (u32, u32),
    pub ticks: u32,
    /// Where patches install and stage, in place of the platform's directory for them (M4 P4).
    pub patch_dir: Option<PathBuf>,
    /// `--update-host <url>` (dev builds): where the boot looks for an update, in place of the build's host, so
    /// a patch served on this machine reaches a desktop, the Simulator and the emulator (M4 P4).
    #[cfg(feature = "dev")]
    pub update_host: Option<String>,
    /// `--lose-device <frames>` (dev builds): the window's GPU device is destroyed once, after that many frames
    /// were presented, so the game's rebuild runs where no device is lost when asked (M4 S6), as the sandbox's
    /// `debug.lose_device` does. The boot screen's frames count, so a small count loses it mid-boot (M4 H4).
    #[cfg(feature = "dev")]
    pub lose_device: Option<u64>,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            choose: None,
            script: None,
            save_dir: None,
            record: None,
            replay: None,
            at_tick: None,
            capture: None,
            size: (1280, 720),
            ticks: 180,
            patch_dir: None,
            #[cfg(feature = "dev")]
            update_host: None,
            #[cfg(feature = "dev")]
            lose_device: None,
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
        let mut ticks = false;
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
                "--patch-dir" => options.patch_dir = Some(PathBuf::from(value()?)),
                #[cfg(feature = "dev")]
                "--update-host" => options.update_host = Some(value()?.to_owned()),
                "--record" => options.record = Some(PathBuf::from(value()?)),
                "--replay" => options.replay = Some(PathBuf::from(value()?)),
                "--at-tick" => {
                    let raw = value()?;
                    let tick = raw.parse().ok().filter(|&tick| tick > 0);
                    options.at_tick =
                        Some(tick.ok_or_else(|| format!("--at-tick expects a tick from 1, got {raw:?}"))?);
                }
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
                    ticks = true;
                    let raw = value()?;
                    options.ticks =
                        raw.parse().map_err(|_| format!("--ticks expects a count, got {raw:?}"))?;
                }
                // `startup` crashes first where the process's own arguments ask (desktop and iOS); a debuggable
                // Android build's arrive here, from the launching intent.
                sapphire_crash::CRASH_FLAG => sapphire_crash::ForcedCrash::from_name(value()?)?.now(),
                #[cfg(feature = "dev")]
                "--lose-device" => {
                    let raw = value()?;
                    options.lose_device = Some(
                        raw.parse()
                            .map_err(|_| format!("--lose-device expects a count of frames, got {raw:?}"))?,
                    );
                }
                _ => return Err(format!("unknown argument {arg:?}")),
            }
        }
        if options.replay.is_some() {
            if options.script.is_some()
                || options.record.is_some()
                || options.choose == Some(FlowCommand::Continue)
            {
                return Err(
                    "--replay plays its file from its start, as recorded: drop --script, --record and \
                            --continue"
                        .to_owned(),
                );
            }
            // There's nothing to choose: the title's new game is the file's run.
            options.choose = Some(FlowCommand::NewGame);
        }
        if let Some(tick) = options.at_tick {
            if options.replay.is_none() || options.capture.is_none() {
                return Err(
                    "--at-tick draws a tick of a run's file: it needs --replay and --capture".to_owned()
                );
            }
            if ticks {
                return Err("--at-tick draws as many frames as the tick takes: drop --ticks".to_owned());
            }
            options.ticks = u32::try_from(tick)
                .ok()
                .and_then(|tick| tick.checked_add(HELD_FRAMES))
                .ok_or_else(|| format!("--at-tick {tick} is past any run's end"))?;
        }
        if options.choose == Some(FlowCommand::Continue) && !options.saves() {
            // It would draw the title, as a continue with nothing to continue does, and say nothing.
            return Err("--continue with --capture continues the save in --save-dir: a capture keeps none \
                        of its own"
                .to_owned());
        }
        Ok(options)
    }

    /// Whether the game keeps a save: always in a window, and in a capture only where `--save-dir` says, so
    /// a run nobody's watching leaves nobody's save changed.
    fn saves(&self) -> bool {
        self.capture.is_none() || self.save_dir.is_some()
    }

    /// Whether the game patches: always in a window, and in a capture only where `--patch-dir` says, so a run
    /// nobody's watching neither installs nor fetches into anybody's game.
    fn patches(&self) -> bool {
        self.capture.is_none() || self.patch_dir.is_some()
    }

    /// How this run patches (M4 P4), if it does: in `--patch-dir` or the player's directory's `patch/`, from the
    /// build's host (or `--update-host`'s, in a dev build), trusting the build's keys (and the test key, in a dev
    /// build), as the build `cargo xtask package` numbered, or 0 for one it didn't.
    pub(crate) fn patch_setup(&self, platform: &Platform) -> Option<PatchSetup> {
        if !self.patches() {
            return None;
        }
        let dir = self.patch_dir.clone().unwrap_or_else(|| platform.user_dir().join("patch"));
        let host = Some(build_info::PATCH_HOST).filter(|host| !host.is_empty()).map(str::to_owned);
        #[cfg(feature = "dev")]
        let host = self.update_host.clone().or(host);
        let mut root_keys = build_info::RELEASE_KEYS.to_vec();
        if cfg!(feature = "dev") {
            root_keys.push(sapphire_patch::TEST_KEY);
        }
        Some(PatchSetup { dir, host, root_keys, app_build: build_info::BUILD_NUMBER.unwrap_or(0) })
    }

    /// The show this run stands with `settings` on `content`, and the run it stands first: the game's flow over
    /// its runs where the content names one, with its save where `--save-dir` or the platform says, else the
    /// arena's fight alone.
    pub(crate) fn show(
        &self,
        platform: &Platform,
        content: &ContentFs,
        settings: PlayerSettings,
    ) -> Result<(GameShow, CombatOption), AppInitError> {
        (self.make_show(platform, settings))(content)
    }

    /// What makes [`Self::show`] on any content: a boot's, which makes it again on a release it installs.
    pub(crate) fn make_show(&self, platform: &Platform, settings: PlayerSettings) -> MakeShow {
        let dir = self.save_dir.clone().unwrap_or_else(|| platform.user_dir().to_owned());
        let (saves, script, record, replay) =
            (self.saves(), self.script.clone(), self.record.clone(), self.replay.clone());
        Box::new(move |content| {
            let setup = GameSetup {
                store: saves.then(|| progress_store(&dir, durable_replace)),
                script: script.clone(),
                record: record.clone(),
                replay: replay.clone(),
            };
            let made = GameShow::from_content(content, settings.clone(), setup);
            // A playtest build's show offers the mark and hands each run over, however often it's made again.
            #[cfg(feature = "playtest")]
            let made = made.map(|(show, first)| (crate::playtest::asked(show), first));
            made
        })
    }

    /// What `--capture` draws.
    pub(crate) fn capture(&self) -> GameCapture {
        GameCapture {
            choose: self.choose,
            size: self.size,
            ticks: self.ticks,
            at_tick: self.at_tick,
            quality: QualitySettings::default(),
            forward: false,
            draw_from: 0,
            boot: true,
            display: OffscreenDisplay::default(),
        }
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
            GameCapture {
                choose: Some(FlowCommand::NewGame),
                size: (640, 360),
                ticks: 9,
                at_tick: None,
                quality: QualitySettings::default(),
                forward: false,
                draw_from: 0,
                boot: true,
                display: OffscreenDisplay::default(),
            }
        );
        assert!(!options.saves());
        assert!(!options.patches(), "a capture patches nothing unless told where");
        assert!(is_offscreen_flag("--capture") && is_offscreen_flag("--capture=a.png"));
        assert!(!is_offscreen_flag("--script"));
        let options =
            parse(&["--capture", "a.png", "--save-dir", "saves", "--continue", "--record", "runs"]).unwrap();
        assert!(options.saves());
        assert!(!options.patches());
        assert_eq!(options.choose, Some(FlowCommand::Continue));
        assert_eq!(options.record, Some(PathBuf::from("runs")));
        let options = parse(&["--capture", "a.png", "--patch-dir", "patches"]).unwrap();
        assert_eq!(options.patch_dir, Some(PathBuf::from("patches")));
        assert!(options.patches());
        assert!(parse(&[]).unwrap().patches(), "a window's game patches");
        assert_eq!(parse(&["--open-fight"]).unwrap().choose, Some(FlowCommand::NewGame));
    }

    #[test]
    fn a_replay_plays_a_runs_file_and_can_draw_one_of_its_ticks() {
        let options = parse(&["--replay", "runs/1/ward-run-1.sreplay"]).unwrap();
        assert_eq!(options.replay, Some(PathBuf::from("runs/1/ward-run-1.sreplay")));
        assert_eq!(options.choose, Some(FlowCommand::NewGame), "past the title, to the file's run");
        let options =
            parse(&["--replay", "a.sreplay", "--at-tick", "1634", "--capture", "mark-1.png"]).unwrap();
        let capture = options.capture();
        assert_eq!(capture.at_tick, Some(1634));
        assert_eq!(capture.ticks, 1634 + HELD_FRAMES);
        for refused in [
            &["--replay", "a.sreplay", "--script", "fight-back"][..],
            &["--replay", "a.sreplay", "--record", "runs"],
            &["--replay", "a.sreplay", "--continue"],
            &["--at-tick", "5", "--capture", "a.png"],
            &["--replay", "a.sreplay", "--at-tick", "5"],
            &["--replay", "a.sreplay", "--at-tick", "5", "--capture", "a.png", "--ticks", "9"],
            &["--replay", "a.sreplay", "--at-tick", "0", "--capture", "a.png"],
            &["--replay", "a.sreplay", "--at-tick", "soon", "--capture", "a.png"],
        ] {
            assert!(parse(refused).is_err(), "{refused:?}");
        }
    }

    #[test]
    fn what_it_cant_read_it_says() {
        assert!(parse(&["--stage"]).is_err());
        assert!(parse(&["--size", "640"]).is_err());
        assert!(parse(&["--size", "0x10"]).is_err());
        assert!(parse(&["--ticks", "many"]).is_err());
        assert!(parse(&["--script"]).is_err());
        assert!(parse(&["--save-dir"]).is_err());
        assert!(parse(&["--patch-dir"]).is_err());
        let refused = parse(&["--capture", "a.png", "--continue"]).expect_err("nothing to continue");
        assert!(refused.contains("--save-dir"), "{refused}");
        assert!(parse(&["--continue"]).is_ok(), "a window's game keeps its save");
        // A kind that isn't one is refused, rather than crashing some other way.
        let refused = parse(&["--crash", "soon"]).expect_err("no such crash");
        assert!(refused.contains("panic, fault"), "{refused}");
    }

    #[cfg(feature = "dev")]
    #[test]
    fn a_dev_build_looks_for_updates_where_told() {
        let options = parse(&["--update-host", "http://127.0.0.1:8475/"]).unwrap();
        assert_eq!(options.update_host.as_deref(), Some("http://127.0.0.1:8475/"));
        assert!(parse(&["--update-host"]).is_err());
    }

    #[cfg(not(feature = "dev"))]
    #[test]
    fn only_a_dev_build_looks_elsewhere_for_updates() {
        assert!(parse(&["--update-host", "http://127.0.0.1:8475/"]).is_err());
    }

    #[cfg(feature = "dev")]
    #[test]
    fn a_dev_build_loses_its_device_when_told_to() {
        assert_eq!(parse(&["--lose-device", "120"]).unwrap().lose_device, Some(120));
        assert_eq!(parse(&[]).unwrap().lose_device, None);
        assert!(parse(&["--lose-device", "soon"]).is_err());
    }
}
