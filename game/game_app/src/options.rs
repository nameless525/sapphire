//! What the game takes on its command line: only what a run with nobody playing needs (see the crate's docs).

use std::path::PathBuf;

use game_combat::combat::CombatOption;

const CAPTURE_FLAG: &str = "--capture";

/// A run's arguments.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Options {
    pub open_fight: bool,
    pub script: Option<String>,
    pub capture: Option<PathBuf>,
    pub size: (u32, u32),
    pub ticks: u32,
}

impl Default for Options {
    fn default() -> Self {
        Self { open_fight: false, script: None, capture: None, size: (1280, 720), ticks: 180 }
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
                "--open-fight" => options.open_fight = true,
                "--script" => options.script = Some(value()?.to_owned()),
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

    /// The fight this run stands: the arena's, as whoever's there plays it or as `--script` does.
    pub(crate) fn fight(&self) -> CombatOption {
        CombatOption::Fight { script: self.script.clone(), party: false }
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
        assert!(!options.open_fight);
        assert_eq!(options.fight(), CombatOption::Fight { script: None, party: false });
    }

    #[test]
    fn a_capture_says_what_it_draws() {
        let options =
            parse(&["--capture", "a.png", "--size", "640x360", "--ticks", "9", "--open-fight"]).unwrap();
        assert_eq!(options.capture, Some(PathBuf::from("a.png")));
        assert_eq!((options.size, options.ticks), ((640, 360), 9));
        assert!(options.open_fight);
        assert!(is_offscreen_flag("--capture") && is_offscreen_flag("--capture=a.png"));
        assert!(!is_offscreen_flag("--script"));
    }

    #[test]
    fn what_it_cant_read_it_says() {
        assert!(parse(&["--stage"]).is_err());
        assert!(parse(&["--size", "640"]).is_err());
        assert!(parse(&["--size", "0x10"]).is_err());
        assert!(parse(&["--ticks", "many"]).is_err());
        assert!(parse(&["--script"]).is_err());
    }
}
