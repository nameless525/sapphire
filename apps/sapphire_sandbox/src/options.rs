//! Command-line options.

use std::path::PathBuf;

use sapphire_platform::DeviceClass;
use sapphire_render::{LightPath, Msaa, RenderSettings};

/// The flags that select an offscreen mode; see [`Offscreen`].
pub(crate) const CAPTURE_FLAG: &str = "--capture";
pub(crate) const BENCH_FLAG: &str = "--bench";

/// Whether `arg` selects an offscreen mode, as `--capture`, `--bench` or their `--flag=value`
/// forms. `main` checks this before SDL starts, so the windowed app never opens for one.
pub(crate) fn is_offscreen_flag(arg: &str) -> bool {
    let name = arg.split_once('=').map_or(arg, |(name, _)| name);
    name == CAPTURE_FLAG || name == BENCH_FLAG
}

/// Options parsed from the process arguments.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Options {
    /// Exit after this many presented frames.
    pub frame_limit: Option<u64>,
    pub msaa: Msaa,
    /// `--lights`: place the demo lights around the character.
    pub lights: bool,
    /// `--light-path clustered|per-object`: override the device's default.
    pub light_path: Option<LightPath>,
    /// Render offscreen instead of opening a window.
    pub offscreen: Option<Offscreen>,
}

/// A mode that renders into an offscreen target, without creating a window or taking focus.
/// Desktop only.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Offscreen {
    pub mode: OffscreenMode,
    /// `--size WxH`, default 1920x1080.
    pub width: u32,
    pub height: u32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum OffscreenMode {
    /// `--capture <png> [--capture-ticks N]`: simulate `ticks` fixed steps, render one frame
    /// and write it to `path`.
    Capture { path: PathBuf, ticks: u32 },
    /// `--bench <frames>`: run the per-frame CPU path `frames` times and log its cost.
    Bench { frames: u32 },
}

impl Default for Options {
    fn default() -> Self {
        Self { frame_limit: None, msaa: Msaa::X4, lights: false, light_path: None, offscreen: None }
    }
}

impl Options {
    /// The renderer settings: the defaults for `class`, with the options' overrides.
    pub fn render_settings(&self, class: DeviceClass) -> RenderSettings {
        let defaults = RenderSettings::for_device(class);
        RenderSettings {
            msaa: self.msaa,
            light_path: self.light_path.unwrap_or(defaults.light_path),
            ..defaults
        }
    }

    /// Parses `args`, which includes the program name first. Unknown arguments are returned
    /// so the caller can warn about them; SDL and the OS may pass their own.
    pub fn parse(args: &[String]) -> Result<(Self, Vec<String>), String> {
        let mut options = Self::default();
        let mut unknown = Vec::new();
        let (mut capture, mut bench, mut capture_ticks, mut size) = (None, None, None, None);
        let mut args = args.iter().skip(1);
        while let Some(arg) = args.next() {
            // `--name=value` or `--name value`.
            let (name, mut inline) = match arg.split_once('=') {
                Some((name, value)) if name.starts_with("--") => (name, Some(value.to_owned())),
                _ => (arg.as_str(), None),
            };
            let mut value = |name: &str| {
                inline.take().or_else(|| args.next().cloned()).ok_or_else(|| format!("{name} needs a value"))
            };
            match name {
                "--frames" => options.frame_limit = Some(number(&value("--frames")?, "--frames")?),
                "--msaa" => {
                    options.msaa = match value("--msaa")?.as_str() {
                        "off" | "1" => Msaa::Off,
                        "2" => Msaa::X2,
                        "4" => Msaa::X4,
                        "8" => Msaa::X8,
                        other => return Err(format!("--msaa expects off, 2, 4 or 8, got {other:?}")),
                    };
                }
                "--lights" => options.lights = true,
                "--light-path" => {
                    options.light_path = Some(match value("--light-path")?.as_str() {
                        "clustered" => LightPath::Clustered,
                        "per-object" => LightPath::PerObject,
                        other => {
                            return Err(format!(
                                "--light-path expects clustered or per-object, got {other:?}"
                            ));
                        }
                    });
                }
                CAPTURE_FLAG => capture = Some(PathBuf::from(value(CAPTURE_FLAG)?)),
                BENCH_FLAG => bench = Some(number(&value(BENCH_FLAG)?, BENCH_FLAG)?),
                "--capture-ticks" => {
                    capture_ticks = Some(number(&value("--capture-ticks")?, "--capture-ticks")?)
                }
                "--size" => size = Some(parse_size(&value("--size")?)?),
                _ => unknown.push(arg.clone()),
            }
        }
        let mode = match (capture, bench) {
            (Some(_), Some(_)) => return Err(format!("{CAPTURE_FLAG} and {BENCH_FLAG} are exclusive")),
            (Some(path), None) => Some(OffscreenMode::Capture { path, ticks: capture_ticks.unwrap_or(90) }),
            (None, Some(frames)) => Some(OffscreenMode::Bench { frames }),
            (None, None) => None,
        };
        if capture_ticks.is_some() && !matches!(mode, Some(OffscreenMode::Capture { .. })) {
            return Err(format!("--capture-ticks needs {CAPTURE_FLAG}"));
        }
        options.offscreen = match mode {
            Some(mode) => {
                let (width, height) = size.unwrap_or((1920, 1080));
                Some(Offscreen { mode, width, height })
            }
            None if size.is_some() => return Err(format!("--size needs {CAPTURE_FLAG} or {BENCH_FLAG}")),
            None => None,
        };
        Ok((options, unknown))
    }
}

fn number<T: std::str::FromStr>(raw: &str, name: &str) -> Result<T, String> {
    raw.parse().map_err(|_| format!("{name} expects a number, got {raw:?}"))
}

fn parse_size(raw: &str) -> Result<(u32, u32), String> {
    let invalid = || format!("--size expects WIDTHxHEIGHT, got {raw:?}");
    let (width, height) = raw.split_once('x').ok_or_else(invalid)?;
    let (width, height) = (width.parse().map_err(|_| invalid())?, height.parse().map_err(|_| invalid())?);
    if width == 0 || height == 0 {
        return Err(invalid());
    }
    Ok((width, height))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(args: &[&str]) -> Result<(Options, Vec<String>), String> {
        let args: Vec<String> =
            std::iter::once("sandbox").chain(args.iter().copied()).map(String::from).collect();
        Options::parse(&args)
    }

    #[test]
    fn parses_known_options() {
        let (options, unknown) = parse(&["--frames", "120", "--msaa", "off"]).unwrap();
        assert_eq!(options, Options { frame_limit: Some(120), msaa: Msaa::Off, ..Options::default() });
        assert!(unknown.is_empty());
    }

    #[test]
    fn light_options_override_the_device_defaults() {
        let (options, _) = parse(&["--lights", "--light-path", "per-object", "--msaa", "2"]).unwrap();
        assert!(options.lights);
        let settings = options.render_settings(DeviceClass::Desktop);
        assert_eq!(settings.light_path, LightPath::PerObject);
        assert_eq!((settings.msaa, settings.max_lights), (Msaa::X2, 64), "the rest stay desktop defaults");

        let defaults = Options::default();
        assert!(!defaults.lights);
        assert_eq!(defaults.render_settings(DeviceClass::Desktop).light_path, LightPath::Clustered);
        let mobile = defaults.render_settings(DeviceClass::Mobile);
        assert_eq!((mobile.light_path, mobile.max_lights), (LightPath::PerObject, 8));
        let (clustered, _) = parse(&["--light-path=clustered"]).unwrap();
        assert_eq!(clustered.render_settings(DeviceClass::Mobile).light_path, LightPath::Clustered);
        assert!(parse(&["--light-path", "deferred"]).is_err());
    }

    #[test]
    fn parses_offscreen_modes_in_any_order() {
        let (options, _) =
            parse(&["--size", "640x360", "--capture", "out.png", "--capture-ticks", "5"]).unwrap();
        assert_eq!(
            options.offscreen,
            Some(Offscreen {
                mode: OffscreenMode::Capture { path: "out.png".into(), ticks: 5 },
                width: 640,
                height: 360
            })
        );
        let (options, _) = parse(&["--capture", "a.png"]).unwrap();
        assert_eq!(
            options.offscreen,
            Some(Offscreen {
                mode: OffscreenMode::Capture { path: "a.png".into(), ticks: 90 },
                width: 1920,
                height: 1080
            })
        );
        let (options, _) = parse(&["--bench", "300", "--msaa", "2"]).unwrap();
        assert_eq!(
            options.offscreen,
            Some(Offscreen { mode: OffscreenMode::Bench { frames: 300 }, width: 1920, height: 1080 })
        );
        assert_eq!(options.msaa, Msaa::X2);
    }

    #[test]
    fn keeps_unknown_arguments() {
        let (options, unknown) = parse(&["-NSDocumentRevisionsDebugMode", "YES"]).unwrap();
        assert_eq!(options, Options::default());
        assert_eq!(unknown, ["-NSDocumentRevisionsDebugMode", "YES"]);
    }

    #[test]
    fn rejects_bad_values() {
        assert!(parse(&["--frames"]).is_err());
        assert!(parse(&["--frames", "many"]).is_err());
        assert!(parse(&["--msaa", "3"]).is_err());
        assert!(parse(&["--capture-ticks", "4"]).is_err(), "needs --capture");
        assert!(parse(&["--bench", "10", "--capture-ticks", "4"]).is_err(), "needs --capture");
        assert!(parse(&["--size", "64x64"]).is_err(), "needs an offscreen mode");
        assert!(parse(&["--capture", "a.png", "--bench", "10"]).is_err(), "exclusive");
        assert!(parse(&["--capture", "a.png", "--size", "0x10"]).is_err());
        assert!(parse(&["--bench", "10", "--size", "wide"]).is_err());
    }

    #[test]
    fn accepts_inline_values() {
        let (options, unknown) = parse(&["--bench=60", "--size=320x200", "--msaa=off"]).unwrap();
        assert_eq!(
            options.offscreen,
            Some(Offscreen { mode: OffscreenMode::Bench { frames: 60 }, width: 320, height: 200 })
        );
        assert_eq!(options.msaa, Msaa::Off);
        assert!(unknown.is_empty());
        assert!(
            is_offscreen_flag("--bench=60")
                && is_offscreen_flag("--capture")
                && !is_offscreen_flag("--benchmark")
        );
    }
}
