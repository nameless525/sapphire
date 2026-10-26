//! Command-line options.

use std::path::PathBuf;

use sapphire_render::quality::{QualitySettings, TierId};
use sapphire_render::{LightPath, MIN_RENDER_SCALE, Msaa, RenderSettings};

use crate::scene::{self, Scene};

/// The flags that select an offscreen mode; see [`Offscreen`].
pub(crate) const CAPTURE_FLAG: &str = "--capture";
pub(crate) const BENCH_FLAG: &str = "--bench";
/// Shows the debug overlay from the start (`dev` builds), with its default panels or those
/// listed: `--debug-overlay=stats,graph`.
const DEBUG_OVERLAY_FLAG: &str = "--debug-overlay";

/// Whether `arg` selects an offscreen mode, as `--capture`, `--bench` or their `--flag=value`
/// forms. `main` checks this before SDL starts, so the windowed app never opens for one.
pub(crate) fn is_offscreen_flag(arg: &str) -> bool {
    let name = arg.split_once('=').map_or(arg, |(name, _)| name);
    name == CAPTURE_FLAG || name == BENCH_FLAG
}

/// Options parsed from the process arguments. The default is no options: everything from the
/// quality tier and the scene.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Options {
    /// Exit after this many presented frames.
    pub frame_limit: Option<u64>,
    /// `--aa off|msaa|taa`: the antialiasing technique instead of the quality tier's, for
    /// comparisons and golden images.
    pub aa: Option<Antialiasing>,
    /// `--msaa off|2|4|8`: override the quality tier's (or `--aa`'s) sample count.
    pub msaa: Option<Msaa>,
    /// `--lights`: place the demo lights around the character.
    pub lights: bool,
    /// `--light-path clustered|per-object`: override the quality tier's.
    pub light_path: Option<LightPath>,
    /// `--tier <tier>`: the player's quality tier instead of the device's.
    pub tier: Option<TierId>,
    /// `--render-scale <0.25..1>`: the player's base render scale instead of the tier's.
    pub render_scale: Option<f32>,
    /// `--dynamic-resolution`: let dynamic resolution lower the render scale.
    pub dynamic_resolution: bool,
    /// `--skinned`: place the skinned stalk beside the character.
    pub skinned: bool,
    /// `--time-of-day <hours>`: the hour the scene starts at, `0 ≤ hours < 24`, instead of the
    /// default day's 10:00.
    pub time_of_day: Option<f32>,
    /// `--day-length <seconds>`: run the day, one every this many seconds of world time.
    pub day_length: Option<f32>,
    /// `--character-rig <preset>`: the character light rig preset the character is lit by,
    /// instead of the default (`gameplay`). The scene checks the name against its presets.
    pub character_rig: Option<String>,
    /// `--swatches`: place the anime character shader swatches beside the character.
    pub swatches: bool,
    /// `--faces`: show the face look-dev rig instead of the character.
    pub faces: bool,
    /// `--environment`: place the character in the courtyard, the environment shader's test
    /// scene.
    pub environment: bool,
    /// `--cloud-cover <0..1>`: both cloud layers' coverage instead of time of day's; 0 is a
    /// clear sky.
    pub cloud_cover: Option<f32>,
    /// `--plain-sky`: the sky's gradient alone, with no discs, glows, stars or clouds.
    pub plain_sky: bool,
    /// `--view <yaw>,<pitch>`: the orbit camera's starting angles in degrees instead of the
    /// framing's (yaw orbits right from in front of the character; negative pitch looks up).
    pub view: Option<(f32, f32)>,
    /// `--spin <radians per second>`: the turntable's speed instead of its default.
    pub spin: Option<f32>,
    /// Render offscreen instead of opening a window.
    pub offscreen: Option<Offscreen>,
    /// `--debug-overlay[=panels]`: the overlay starts shown, with these panels (empty: the
    /// default ones). Only `dev` builds have it.
    pub debug_overlay: Option<String>,
    /// `--no-hot-reload`: watch no files. Only `dev` builds watch any.
    pub no_hot_reload: bool,
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
    /// `--capture <png> [--capture-ticks N] [--capture-frames F [--capture-step S]]`: simulate
    /// to tick `ticks`, render `frames` frames and write the last to `path` (see
    /// [`CaptureFrames`]).
    Capture { path: PathBuf, ticks: u32, frames: CaptureFrames },
    /// `--bench <frames>`: run the per-frame CPU path `frames` times and log its cost.
    Bench { frames: u32 },
}

/// The frames a capture draws, the last of which it writes: one by default. Temporal AA needs
/// earlier frames to build its history from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct CaptureFrames {
    /// `--capture-frames`: how many.
    pub count: u32,
    /// `--capture-step`: fixed steps between consecutive frames. 0 draws them all at the
    /// capture's tick (the scene holds still while TAA converges); 1 plays the scene at 60 fps
    /// up to it.
    pub step: u32,
}

impl Default for CaptureFrames {
    fn default() -> Self {
        Self { count: 1, step: 0 }
    }
}

impl CaptureFrames {
    /// The ticks to simulate before the first frame, so the last lands on tick `ticks`.
    pub fn first_tick(&self, ticks: u32) -> Option<u32> {
        ticks.checked_sub(self.step * (self.count - 1))
    }
}

/// An antialiasing technique (`--aa`), over the quality tier's.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Antialiasing {
    /// Neither: every edge a hard pixel step.
    Off,
    /// Multisampling alone: the tier's sample count, or 4× where the tier has none.
    Msaa,
    /// Temporal AA alone (the desktop tiers').
    Taa,
}

impl Options {
    /// Sets `scene` up as the options ask: the hour and the day's length, the cloud cover, the
    /// camera's starting view and the turntable's speed.
    pub fn apply_to(&self, scene: &mut Scene) {
        scene.set_day(self.time_of_day.unwrap_or(scene::DEFAULT_TIME_OF_DAY), self.day_length);
        scene.set_cloud_cover(self.cloud_cover);
        scene.set_plain_sky(self.plain_sky);
        if let Some((yaw, pitch)) = self.view {
            scene.camera_mut().look_from(yaw.to_radians(), pitch.to_radians());
        }
        if let Some(speed) = self.spin {
            scene.set_spin(speed);
        }
    }

    /// The renderer settings: the quality tier's, with the options' overrides. `--msaa` wins
    /// over `--aa`'s sample count.
    pub fn render_settings(&self, tier: RenderSettings) -> RenderSettings {
        let (msaa, temporal) = match self.aa {
            None => (tier.msaa, tier.temporal),
            Some(Antialiasing::Off) => (Msaa::Off, false),
            Some(Antialiasing::Msaa) if tier.msaa == Msaa::Off => (Msaa::X4, false),
            Some(Antialiasing::Msaa) => (tier.msaa, false),
            Some(Antialiasing::Taa) => (Msaa::Off, true),
        };
        RenderSettings {
            msaa: self.msaa.unwrap_or(msaa),
            temporal,
            light_path: self.light_path.unwrap_or(tier.light_path),
            ..tier
        }
    }

    /// The player's quality settings: the command line's, until a settings screen (M3).
    pub fn quality_settings(&self) -> QualitySettings {
        QualitySettings {
            tier: self.tier,
            render_scale: self.render_scale,
            dynamic_resolution: self.dynamic_resolution,
        }
    }

    /// Parses `args`, which includes the program name first. Unknown arguments are returned
    /// so the caller can warn about them; SDL and the OS may pass their own.
    pub fn parse(args: &[String]) -> Result<(Self, Vec<String>), String> {
        let mut options = Self::default();
        let mut unknown = Vec::new();
        let (mut capture, mut bench, mut capture_ticks, mut size) = (None, None, None, None);
        let (mut capture_frames, mut capture_step) = (None, None);
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
                "--aa" => {
                    options.aa = Some(match value("--aa")?.as_str() {
                        "off" => Antialiasing::Off,
                        "msaa" => Antialiasing::Msaa,
                        "taa" => Antialiasing::Taa,
                        other => return Err(format!("--aa expects off, msaa or taa, got {other:?}")),
                    });
                }
                "--msaa" => {
                    options.msaa = Some(match value("--msaa")?.as_str() {
                        "off" | "1" => Msaa::Off,
                        "2" => Msaa::X2,
                        "4" => Msaa::X4,
                        "8" => Msaa::X8,
                        other => return Err(format!("--msaa expects off, 2, 4 or 8, got {other:?}")),
                    });
                }
                "--tier" => {
                    let raw = value("--tier")?;
                    options.tier = Some(TierId::from_key(&raw).ok_or_else(|| {
                        let keys: Vec<&str> = TierId::ALL.iter().map(|tier| tier.key()).collect();
                        format!("--tier expects one of {}, got {raw:?}", keys.join(", "))
                    })?);
                }
                "--render-scale" => {
                    let scale: f32 = number(&value("--render-scale")?, "--render-scale")?;
                    if !(MIN_RENDER_SCALE..=1.0).contains(&scale) {
                        return Err(format!("--render-scale expects {MIN_RENDER_SCALE} to 1, got {scale}"));
                    }
                    options.render_scale = Some(scale);
                }
                "--dynamic-resolution" => options.dynamic_resolution = true,
                "--lights" => options.lights = true,
                "--skinned" => options.skinned = true,
                "--time-of-day" => {
                    let hours: f32 = number(&value("--time-of-day")?, "--time-of-day")?;
                    if !(0.0..24.0).contains(&hours) {
                        return Err(format!("--time-of-day expects hours from 0 to below 24, got {hours}"));
                    }
                    options.time_of_day = Some(hours);
                }
                "--day-length" => {
                    let seconds: f32 = number(&value("--day-length")?, "--day-length")?;
                    if !(seconds.is_finite() && seconds > 0.0) {
                        return Err(format!(
                            "--day-length expects a positive number of seconds, got {seconds}"
                        ));
                    }
                    options.day_length = Some(seconds);
                }
                "--character-rig" => options.character_rig = Some(value("--character-rig")?),
                "--swatches" => options.swatches = true,
                "--faces" => options.faces = true,
                "--environment" => options.environment = true,
                "--cloud-cover" => {
                    let cover: f32 = number(&value("--cloud-cover")?, "--cloud-cover")?;
                    if !(0.0..=1.0).contains(&cover) {
                        return Err(format!("--cloud-cover expects 0 to 1, got {cover}"));
                    }
                    options.cloud_cover = Some(cover);
                }
                "--view" => options.view = Some(parse_view(&value("--view")?)?),
                "--plain-sky" => options.plain_sky = true,
                "--no-hot-reload" => options.no_hot_reload = true,
                "--spin" => {
                    let speed: f32 = number(&value("--spin")?, "--spin")?;
                    if !speed.is_finite() {
                        return Err(format!("--spin expects radians per second, got {speed}"));
                    }
                    options.spin = Some(speed);
                }
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
                // Its panel list is optional, so it only comes inline: `--debug-overlay=all`.
                DEBUG_OVERLAY_FLAG => options.debug_overlay = Some(inline.take().unwrap_or_default()),
                CAPTURE_FLAG => capture = Some(PathBuf::from(value(CAPTURE_FLAG)?)),
                BENCH_FLAG => bench = Some(number(&value(BENCH_FLAG)?, BENCH_FLAG)?),
                "--capture-ticks" => {
                    capture_ticks = Some(number(&value("--capture-ticks")?, "--capture-ticks")?)
                }
                "--capture-frames" => {
                    let count: u32 = number(&value("--capture-frames")?, "--capture-frames")?;
                    if count == 0 {
                        return Err("--capture-frames expects at least 1".to_owned());
                    }
                    capture_frames = Some(count);
                }
                "--capture-step" => capture_step = Some(number(&value("--capture-step")?, "--capture-step")?),
                "--size" => size = Some(parse_size(&value("--size")?)?),
                _ => unknown.push(arg.clone()),
            }
        }
        let frames = CaptureFrames { count: capture_frames.unwrap_or(1), step: capture_step.unwrap_or(0) };
        let mode = match (capture, bench) {
            (Some(_), Some(_)) => return Err(format!("{CAPTURE_FLAG} and {BENCH_FLAG} are exclusive")),
            (Some(path), None) => {
                let ticks = capture_ticks.unwrap_or(90);
                if frames.first_tick(ticks).is_none() {
                    return Err(format!(
                        "--capture-frames {} --capture-step {} start before tick 0 for --capture-ticks {ticks}",
                        frames.count, frames.step
                    ));
                }
                Some(OffscreenMode::Capture { path, ticks, frames })
            }
            (None, Some(frames)) => Some(OffscreenMode::Bench { frames }),
            (None, None) => None,
        };
        let capturing = matches!(mode, Some(OffscreenMode::Capture { .. }));
        for (given, flag) in [
            (capture_ticks.is_some(), "--capture-ticks"),
            (capture_frames.is_some(), "--capture-frames"),
            (capture_step.is_some(), "--capture-step"),
        ] {
            if given && !capturing {
                return Err(format!("{flag} needs {CAPTURE_FLAG}"));
            }
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

fn parse_view(raw: &str) -> Result<(f32, f32), String> {
    let invalid = || format!("--view expects YAW,PITCH in degrees, pitch within -85..=85, got {raw:?}");
    let (yaw, pitch) = raw.split_once(',').ok_or_else(invalid)?;
    let (yaw, pitch): (f32, f32) =
        (yaw.trim().parse().map_err(|_| invalid())?, pitch.trim().parse().map_err(|_| invalid())?);
    if !yaw.is_finite() || !(-85.0..=85.0).contains(&pitch) {
        return Err(invalid());
    }
    Ok((yaw, pitch))
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
        let (options, unknown) = parse(&["--frames", "120", "--msaa", "off", "--skinned"]).unwrap();
        assert_eq!(
            options,
            Options { frame_limit: Some(120), msaa: Some(Msaa::Off), skinned: true, ..Options::default() }
        );
        assert!(unknown.is_empty());
    }

    #[test]
    fn render_options_override_the_tier() {
        let mobile =
            RenderSettings { light_path: LightPath::PerObject, max_lights: 8, ..RenderSettings::default() };
        let (options, _) = parse(&["--lights", "--light-path", "clustered", "--msaa", "2"]).unwrap();
        assert!(options.lights);
        let settings = options.render_settings(mobile);
        assert_eq!((settings.light_path, settings.msaa), (LightPath::Clustered, Msaa::X2));
        assert_eq!((settings.max_lights, settings.bloom_levels), (8, 5), "the rest stay the tier's");

        let defaults = Options::default();
        assert!(!defaults.lights);
        assert_eq!(defaults.render_settings(mobile), mobile, "no overrides: the tier's settings");
        let (per_object, _) = parse(&["--light-path=per-object"]).unwrap();
        assert_eq!(per_object.render_settings(RenderSettings::default()).light_path, LightPath::PerObject);
        assert!(parse(&["--light-path", "deferred"]).is_err());
    }

    #[test]
    fn quality_options_are_the_players_settings() {
        let (options, _) =
            parse(&["--tier", "mobile-low", "--render-scale", "0.7", "--dynamic-resolution"]).unwrap();
        assert_eq!(
            options.quality_settings(),
            QualitySettings {
                tier: Some(TierId::MobileLow),
                render_scale: Some(0.7),
                dynamic_resolution: true
            }
        );
        assert_eq!(Options::default().quality_settings(), QualitySettings::default());
        assert_eq!(parse(&["--tier=Desktop-High"]).unwrap().0.tier, Some(TierId::DesktopHigh));
        assert!(parse(&["--tier", "ultra"]).unwrap_err().contains("desktop-high"));
        assert!(parse(&["--render-scale", "0.2"]).is_err(), "below the minimum");
        assert!(parse(&["--render-scale", "1.5"]).is_err());
        assert!(parse(&["--render-scale", "NaN"]).is_err());
    }

    #[test]
    fn time_of_day_options_set_the_hour_and_the_cycle() {
        let (options, _) = parse(&["--time-of-day", "18.5", "--day-length=1440"]).unwrap();
        assert_eq!((options.time_of_day, options.day_length), (Some(18.5), Some(1440.0)));
        assert_eq!(parse(&[]).unwrap().0.time_of_day, None);
        assert!(parse(&["--time-of-day", "24"]).is_err());
        assert!(parse(&["--time-of-day", "-1"]).is_err());
        assert!(parse(&["--time-of-day", "noon"]).is_err());
        assert!(parse(&["--day-length", "0"]).is_err());
        assert!(parse(&["--day-length", "inf"]).is_err());
    }

    #[test]
    fn the_character_rig_takes_a_preset_name() {
        assert_eq!(
            parse(&["--character-rig", "portrait"]).unwrap().0.character_rig.as_deref(),
            Some("portrait")
        );
        assert_eq!(
            parse(&["--character-rig=gameplay"]).unwrap().0.character_rig.as_deref(),
            Some("gameplay")
        );
        assert_eq!(parse(&[]).unwrap().0.character_rig, None);
        assert!(parse(&["--character-rig"]).is_err(), "needs a name");
    }

    #[test]
    fn the_environment_is_off_unless_asked_for() {
        assert!(!parse(&[]).unwrap().0.environment);
        let (options, unknown) = parse(&["--environment", "--time-of-day", "18.5"]).unwrap();
        assert!(options.environment && unknown.is_empty());
    }

    #[test]
    fn sky_options_set_the_cloud_cover_and_the_view() {
        let (options, _) = parse(&["--cloud-cover", "0", "--view=120,-15", "--plain-sky"]).unwrap();
        assert_eq!((options.cloud_cover, options.view), (Some(0.0), Some((120.0, -15.0))));
        assert!(options.plain_sky && !Options::default().plain_sky);
        assert_eq!(parse(&["--view", " -40.5 , 10 "]).unwrap().0.view, Some((-40.5, 10.0)));
        assert_eq!(parse(&[]).unwrap().0.cloud_cover, None);
        assert!(parse(&["--cloud-cover", "1.5"]).is_err());
        assert!(parse(&["--cloud-cover", "NaN"]).is_err());
        assert!(parse(&["--view", "120"]).is_err());
        assert!(parse(&["--view", "0,90"]).is_err(), "the orbit camera stops short of straight up");
        assert!(parse(&["--view", "inf,0"]).is_err());
    }

    #[test]
    fn hot_reload_is_on_unless_turned_off() {
        assert!(!parse(&[]).unwrap().0.no_hot_reload);
        let (options, unknown) = parse(&["--no-hot-reload", "--bench", "10"]).unwrap();
        assert!(options.no_hot_reload && unknown.is_empty());
    }

    #[test]
    fn the_debug_overlay_takes_an_optional_inline_panel_list() {
        let (options, unknown) = parse(&["--debug-overlay", "--frames", "5"]).unwrap();
        assert_eq!((options.debug_overlay.as_deref(), options.frame_limit), (Some(""), Some(5)));
        assert!(unknown.is_empty(), "the next argument is not its value");
        let (options, _) = parse(&["--capture", "a.png", "--debug-overlay=stats,graph"]).unwrap();
        assert_eq!(options.debug_overlay.as_deref(), Some("stats,graph"));
        assert_eq!(parse(&[]).unwrap().0.debug_overlay, None);
    }

    #[test]
    fn parses_offscreen_modes_in_any_order() {
        let (options, _) =
            parse(&["--size", "640x360", "--capture", "out.png", "--capture-ticks", "5"]).unwrap();
        assert_eq!(
            options.offscreen,
            Some(Offscreen {
                mode: OffscreenMode::Capture {
                    path: "out.png".into(),
                    ticks: 5,
                    frames: CaptureFrames::default()
                },
                width: 640,
                height: 360
            })
        );
        let (options, _) = parse(&["--capture", "a.png"]).unwrap();
        assert_eq!(
            options.offscreen,
            Some(Offscreen {
                mode: OffscreenMode::Capture {
                    path: "a.png".into(),
                    ticks: 90,
                    frames: CaptureFrames::default()
                },
                width: 1920,
                height: 1080
            })
        );
        let (options, _) = parse(&["--bench", "300", "--msaa", "2"]).unwrap();
        assert_eq!(
            options.offscreen,
            Some(Offscreen { mode: OffscreenMode::Bench { frames: 300 }, width: 1920, height: 1080 })
        );
        assert_eq!(options.msaa, Some(Msaa::X2));
    }

    #[test]
    fn captures_draw_frames_up_to_their_tick() {
        let (options, _) = parse(&[
            "--capture",
            "a.png",
            "--capture-frames",
            "32",
            "--capture-step=1",
            "--capture-ticks",
            "90",
        ])
        .unwrap();
        let Some(Offscreen { mode: OffscreenMode::Capture { ticks, frames, .. }, .. }) = options.offscreen
        else {
            panic!("a capture");
        };
        assert_eq!((ticks, frames), (90, CaptureFrames { count: 32, step: 1 }));
        assert_eq!(frames.first_tick(ticks), Some(59), "the last of 32 frames a tick apart is tick 90");
        assert_eq!(CaptureFrames { count: 32, step: 0 }.first_tick(90), Some(90), "still frames");
        assert_eq!(CaptureFrames::default().first_tick(0), Some(0));
        assert!(parse(&["--capture", "a.png", "--capture-frames", "0"]).is_err());
        assert!(
            parse(&[
                "--capture",
                "a.png",
                "--capture-ticks",
                "10",
                "--capture-frames",
                "12",
                "--capture-step",
                "1"
            ])
            .is_err(),
            "would start before tick 0"
        );
        assert!(parse(&["--bench", "10", "--capture-frames", "4"]).is_err(), "needs --capture");
        assert!(parse(&["--capture-step", "1"]).is_err(), "needs --capture");
    }

    #[test]
    fn antialiasing_options_pick_the_technique() {
        let taa_tier = RenderSettings { msaa: Msaa::Off, temporal: true, ..RenderSettings::default() };
        let mobile = RenderSettings { msaa: Msaa::X4, ..RenderSettings::default() };
        let settings = |args: &[&str], tier| parse(args).unwrap().0.render_settings(tier);
        let aa = |settings: RenderSettings| (settings.msaa, settings.temporal);
        assert_eq!(aa(settings(&[], taa_tier)), (Msaa::Off, true), "the tier's");
        assert_eq!(
            aa(settings(&["--aa", "msaa"], taa_tier)),
            (Msaa::X4, false),
            "4× where the tier has none"
        );
        assert_eq!(aa(settings(&["--aa=msaa", "--msaa", "8"], taa_tier)), (Msaa::X8, false));
        assert_eq!(aa(settings(&["--aa", "taa"], mobile)), (Msaa::Off, true));
        assert_eq!(aa(settings(&["--aa", "taa", "--msaa", "4"], mobile)), (Msaa::X4, true), "both");
        assert_eq!(aa(settings(&["--aa", "off"], mobile)), (Msaa::Off, false));
        assert_eq!(
            aa(settings(&["--msaa", "2"], taa_tier)),
            (Msaa::X2, true),
            "--msaa alone keeps the tier's TAA"
        );
        assert!(parse(&["--aa", "fxaa"]).is_err());

        let (options, _) = parse(&["--spin", "3"]).unwrap();
        assert_eq!(options.spin, Some(3.0));
        assert!(parse(&["--spin", "inf"]).is_err());
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
        assert_eq!(options.msaa, Some(Msaa::Off));
        assert!(unknown.is_empty());
        assert!(
            is_offscreen_flag("--bench=60")
                && is_offscreen_flag("--capture")
                && !is_offscreen_flag("--benchmark")
        );
    }
}
