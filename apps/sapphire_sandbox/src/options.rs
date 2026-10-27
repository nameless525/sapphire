//! Command-line options.

use std::net::SocketAddr;
use std::path::PathBuf;

use sapphire_render::quality::{OcclusionSetting, QualitySettings, Switch, TierId};
use sapphire_render::{DebugView, LightPath, MIN_RENDER_SCALE, Msaa, RenderSettings, Wind};

use crate::faces::FaceLayout;
use crate::foliage::{DEFAULT_WIND, FoliageView};
use crate::lookdev::{self, Shot};
use crate::rims::RimLayout;
use crate::scene::{self, Scene};
use crate::stage::StageLayout;

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
    /// `--shadows on|off`: the player's shadow setting; the tier's without it.
    pub shadows: Option<Switch>,
    /// `--ao off|capsule|full`: the player's ambient occlusion setting (M1 W5); the tier's
    /// without it.
    pub ambient_occlusion: Option<OcclusionSetting>,
    /// `--stage floor|pillar|plinth`: a floor under the character, and a pillar shadowing it or
    /// a stone plinth in place of its pedestal.
    pub stage: Option<StageLayout>,
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
    /// `--faces [--face-layout <layout>]`: show the face look-dev rig instead of the character,
    /// in this layout (`lights` without `--face-layout`; the layouts are `FaceLayout::NAMES`:
    /// lights, hair, highlights, turn, gi, gi-clamped, eyes, eyes-turned, gaze and blink).
    pub faces: Option<FaceLayout>,
    /// `--rim-rig angles|backlit`: show the rim look-dev rig instead of the character (M1 C5).
    pub rim_rig: Option<RimLayout>,
    /// `--environment`: place the character in the courtyard, the environment shader's test
    /// scene.
    pub environment: bool,
    /// `--foliage grove|backlit|ground`: plant the foliage samples in the courtyard (which it
    /// implies) and show them so (M1 W6).
    pub foliage: Option<FoliageView>,
    /// `--wind <strength>`: the wind's strength, 0 (calm) to 4, instead of `--foliage`'s breeze
    /// of 1.
    pub wind: Option<f32>,
    /// `--probes`: the courtyard with its arcade, lit by the ambient probe grid baked for both
    /// (M1 W5). Implies `--environment`.
    pub probes: bool,
    /// `--look-dev wide|hero|sky`: the look-dev town (M1 X1) with the VRM character standing in
    /// it, seen from that shot. A scene of its own rather than a `--stage` layout: it replaces the
    /// character and the ground the other scenes put under it, so it excludes `--stage`,
    /// `--environment` (and `--probes`, which implies it) and `--foliage`, and the scenes that
    /// frame more of the mannequin (`--outline-row`, `--faces`, `--rim-rig`, `--hard-edges`).
    pub look_dev: Option<Shot>,
    /// `--cloud-cover <0..1>`: both cloud layers' coverage instead of time of day's; 0 is a
    /// clear sky.
    pub cloud_cover: Option<f32>,
    /// `--plain-sky`: the sky's gradient alone, with no discs, glows, stars or clouds.
    pub plain_sky: bool,
    /// `--fog off`: no fog (M1 W3): time of day's fog left out of every frame, and no fog cards
    /// in the courtyard. `--fog on` is the default.
    pub fog_off: bool,
    /// `--plain-post`: post as it was before its compositing layers (M1 P2), with no diffusion
    /// glow, character bloom, gradient layers, lens flare, vignette tint or sharpening.
    pub plain_post: bool,
    /// `--sharpening <0..1>`: the player's sharpening instead of the `post.sharpening` cvar's.
    pub sharpening: Option<f32>,
    /// `--post-effect <preset>[@<seconds>]`, repeatable: screen effect presets pushed at the start
    /// (M1 P3), each playing from its start, or held that many seconds into it (for captures and
    /// benches). The scene checks the names against its presets.
    pub post_effects: Vec<(String, Option<f32>)>,
    /// `--view <yaw>,<pitch>`: the orbit camera's starting angles in degrees instead of the
    /// framing's (yaw orbits right from in front of the character; negative pitch looks up).
    pub view: Option<(f32, f32)>,
    /// `--spin <radians per second>`: the turntable's speed instead of its default.
    pub spin: Option<f32>,
    /// `--fov <degrees>`: the camera's vertical field of view instead of 43°, keeping the
    /// framing (a narrow lens from farther away).
    pub fov: Option<f32>,
    /// `--zoom <factor>`: the camera's starting distance as a factor of the framing's.
    pub zoom: Option<f32>,
    /// `--outline-row`: place more of the character behind it, out to past the outline cutoff
    /// (M1 C4).
    pub outline_row: bool,
    /// `--hard-edges`: show the hard-edged outline sample instead of the character (M1 C4).
    pub hard_edges: bool,
    /// `--outline-distance <scale>`: the scale on outlines' LOD cutoffs (`r.outline_distance`)
    /// instead of 1; 0 draws none, for profiling.
    pub outline_distance: Option<f32>,
    /// `--debug-view <view>`: the renderer's debug view (`r.debug_view`, M1 F5) instead of off.
    pub debug_view: Option<DebugView>,
    /// Render offscreen instead of opening a window.
    pub offscreen: Option<Offscreen>,
    /// `--serial-passes`: offscreen, submit each frame graph pass on its own and wait for the
    /// GPU after it, logging each at debug level, so a GPU crash, hang or device loss names its
    /// pass (`GraphExecutor::set_serial`). Slow; for debugging.
    pub serial_passes: bool,
    /// `--debug-overlay[=panels]`: the overlay starts shown, with these panels (empty: the
    /// default ones). Only `dev` builds have it.
    pub debug_overlay: Option<String>,
    /// `--no-hot-reload`: watch no files. Only `dev` builds watch any.
    pub no_hot_reload: bool,
    /// `--live-link <mode>`: how live link takes connections, instead of listening on the
    /// loopback interface. Only `dev` builds have it.
    pub live_link: Option<LiveLinkOption>,
}

/// How live link takes connections (`--live-link`), in `dev` builds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum LiveLinkOption {
    /// `off`.
    Off,
    /// `<addr>` or `listen=<addr>`: listen there. An address beyond loopback (`0.0.0.0:7811`)
    /// opts into taking pushes from the network.
    Listen(SocketAddr),
    /// `connect=<addr>`: dial a tool listening there (behind `adb reverse`).
    Connect(SocketAddr),
}

impl std::str::FromStr for LiveLinkOption {
    type Err = String;

    fn from_str(text: &str) -> Result<Self, String> {
        let address = |text: &str| {
            text.parse().map_err(|_| {
                format!(
                    "--live-link expects off, <addr>, listen=<addr> or connect=<addr> with an address and port \
                     such as 127.0.0.1:7811, got {text:?}"
                )
            })
        };
        match text.split_once('=') {
            _ if text == "off" => Ok(Self::Off),
            Some(("listen", rest)) => address(rest).map(Self::Listen),
            Some(("connect", rest)) => address(rest).map(Self::Connect),
            _ => address(text).map(Self::Listen),
        }
    }
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
    /// Sets `scene` up as the options ask: the hour and the day's length, the plants' framing at
    /// that hour and the wind, the cloud cover, the sky's and post's layers, sharpening, the
    /// screen effects, the camera's starting view and the turntable's speed. Fails if a screen
    /// effect preset is unknown.
    pub fn apply_to(&self, scene: &mut Scene) -> Result<(), String> {
        scene.set_day(self.time_of_day.unwrap_or(scene::DEFAULT_TIME_OF_DAY), self.day_length);
        scene.frame_foliage();
        if let Some(wind) = self.wind() {
            scene.set_wind(wind);
        }
        scene.set_cloud_cover(self.cloud_cover);
        scene.set_plain_sky(self.plain_sky);
        scene.set_fog(!self.fog_off);
        scene.set_plain_post(self.plain_post);
        scene.set_sharpening(self.sharpening);
        if let Some((yaw, pitch)) = self.view {
            scene.camera_mut().look_from(yaw.to_radians(), pitch.to_radians());
        }
        if let Some(speed) = self.spin {
            scene.set_spin(speed);
        }
        if let Some(fov) = self.fov {
            scene.camera_mut().set_fov(fov.to_radians());
        }
        if let Some(zoom) = self.zoom {
            scene.camera_mut().set_zoom(zoom);
        }
        if let Some(scale) = self.outline_distance
            && let Err(error) = sapphire_render::quality::OUTLINE_DISTANCE.set(scale)
        {
            tracing::warn!(target: crate::LOG_TARGET, %error, "--outline-distance");
        }
        if let Some(view) = self.debug_view
            && let Err(error) = sapphire_render::debug_view::DEBUG_VIEW.set(view)
        {
            tracing::warn!(target: crate::LOG_TARGET, %error, "--debug-view");
        }
        for (preset, at) in &self.post_effects {
            scene.push_effect(preset, *at)?;
        }
        Ok(())
    }

    /// The scene's wind: `--foliage`'s breeze at `--wind`'s strength, if either is set.
    pub fn wind(&self) -> Option<Wind> {
        (self.foliage.is_some() || self.wind.is_some())
            .then(|| Wind { strength: self.wind.unwrap_or(DEFAULT_WIND.strength), ..DEFAULT_WIND })
    }

    /// The character the scene draws: the look-dev VRM with `--look-dev`, the mannequin
    /// otherwise.
    pub fn character(&self) -> sapphire_assets::AssetId {
        match self.look_dev {
            Some(_) => lookdev::CHARACTER,
            None => crate::content::CHARACTER,
        }
    }

    /// Whether the content keeps a copy of the character's mesh: the stage and the rim rig draw
    /// the character from it without its pedestal, and the look-dev stage derives its held pose
    /// from it.
    pub fn keeps_character_mesh(&self) -> bool {
        self.stage.is_some() || self.rim_rig.is_some() || self.look_dev.is_some()
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
            shadows: self.shadows.unwrap_or_default(),
            ambient_occlusion: self.ambient_occlusion.unwrap_or_default(),
        }
    }

    /// Parses `args`, which includes the program name first. Unknown arguments are returned
    /// so the caller can warn about them; SDL and the OS may pass their own.
    pub fn parse(args: &[String]) -> Result<(Self, Vec<String>), String> {
        let mut options = Self::default();
        let mut unknown = Vec::new();
        let (mut capture, mut bench, mut capture_ticks, mut size) = (None, None, None, None);
        let (mut capture_frames, mut capture_step) = (None, None);
        let (mut faces, mut face_layout) = (false, None);
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
                "--shadows" => {
                    options.shadows = Some(match value("--shadows")?.as_str() {
                        "on" => Switch::On,
                        "off" => Switch::Off,
                        other => return Err(format!("--shadows expects on or off, got {other:?}")),
                    });
                }
                "--ao" => {
                    options.ambient_occlusion = Some(match value("--ao")?.as_str() {
                        "off" => OcclusionSetting::Off,
                        "capsule" => OcclusionSetting::Capsules,
                        "full" => OcclusionSetting::Full,
                        other => return Err(format!("--ao expects off, capsule or full, got {other:?}")),
                    });
                }
                "--stage" => {
                    let raw = value("--stage")?;
                    options.stage =
                        Some(StageLayout::from_name(&raw).ok_or_else(|| {
                            format!("--stage expects floor, pillar or plinth, got {raw:?}")
                        })?);
                }
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
                "--faces" => faces = true,
                "--face-layout" => {
                    let raw = value("--face-layout")?;
                    face_layout = Some(FaceLayout::from_name(&raw).ok_or_else(|| {
                        format!("--face-layout expects one of {}, got {raw:?}", FaceLayout::NAMES.join(", "))
                    })?);
                }
                "--rim-rig" => {
                    let raw = value("--rim-rig")?;
                    options.rim_rig = Some(
                        RimLayout::from_name(&raw)
                            .ok_or_else(|| format!("--rim-rig expects angles or backlit, got {raw:?}"))?,
                    );
                }
                "--environment" => options.environment = true,
                "--foliage" => {
                    let raw = value("--foliage")?;
                    options.foliage =
                        Some(FoliageView::from_name(&raw).ok_or_else(|| {
                            format!("--foliage expects grove, backlit or ground, got {raw:?}")
                        })?);
                    // The plants stand in the courtyard.
                    options.environment = true;
                }
                "--wind" => {
                    let strength: f32 = number(&value("--wind")?, "--wind")?;
                    if !(0.0..=4.0).contains(&strength) {
                        return Err(format!("--wind expects 0 to 4, got {strength}"));
                    }
                    options.wind = Some(strength);
                }
                "--probes" => (options.probes, options.environment) = (true, true),
                "--look-dev" => {
                    let raw = value("--look-dev")?;
                    options.look_dev = Some(Shot::from_name(&raw).ok_or_else(|| {
                        let names: Vec<&str> = Shot::ALL.iter().map(|shot| shot.name()).collect();
                        format!("--look-dev expects one of {}, got {raw:?}", names.join(", "))
                    })?);
                }
                "--cloud-cover" => {
                    let cover: f32 = number(&value("--cloud-cover")?, "--cloud-cover")?;
                    if !(0.0..=1.0).contains(&cover) {
                        return Err(format!("--cloud-cover expects 0 to 1, got {cover}"));
                    }
                    options.cloud_cover = Some(cover);
                }
                "--view" => options.view = Some(parse_view(&value("--view")?)?),
                "--fov" => {
                    let degrees: f32 = number(&value("--fov")?, "--fov")?;
                    if !(5.0..=120.0).contains(&degrees) {
                        return Err(format!("--fov expects 5 to 120 degrees, got {degrees}"));
                    }
                    options.fov = Some(degrees);
                }
                "--zoom" => {
                    let factor: f32 = number(&value("--zoom")?, "--zoom")?;
                    if !(factor.is_finite() && factor > 0.0) {
                        return Err(format!("--zoom expects a positive factor, got {factor}"));
                    }
                    options.zoom = Some(factor);
                }
                "--outline-row" => options.outline_row = true,
                "--hard-edges" => options.hard_edges = true,
                "--outline-distance" => {
                    let scale: f32 = number(&value("--outline-distance")?, "--outline-distance")?;
                    if !(0.0..=4.0).contains(&scale) {
                        return Err(format!("--outline-distance expects 0 to 4, got {scale}"));
                    }
                    options.outline_distance = Some(scale);
                }
                "--debug-view" => {
                    let name = value("--debug-view")?;
                    let view = DebugView::from_name(&name).ok_or_else(|| {
                        let names: Vec<&str> = DebugView::ALL.iter().map(|view| view.name()).collect();
                        format!("--debug-view expects one of {}, got {name:?}", names.join(", "))
                    })?;
                    options.debug_view = Some(view);
                }
                "--plain-sky" => options.plain_sky = true,
                "--fog" => {
                    options.fog_off = match value("--fog")?.as_str() {
                        "on" => false,
                        "off" => true,
                        other => return Err(format!("--fog expects on or off, got {other:?}")),
                    };
                }
                "--plain-post" => options.plain_post = true,
                "--sharpening" => {
                    let amount: f32 = number(&value("--sharpening")?, "--sharpening")?;
                    if !(0.0..=1.0).contains(&amount) {
                        return Err(format!("--sharpening expects 0 to 1, got {amount}"));
                    }
                    options.sharpening = Some(amount);
                }
                "--post-effect" => options.post_effects.push(parse_post_effect(&value("--post-effect")?)?),
                "--no-hot-reload" => options.no_hot_reload = true,
                "--serial-passes" => options.serial_passes = true,
                "--live-link" => options.live_link = Some(value("--live-link")?.parse()?),
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
        options.faces = match (faces, face_layout) {
            (true, layout) => Some(layout.unwrap_or_default()),
            (false, Some(_)) => return Err("--face-layout needs --faces".to_owned()),
            (false, None) => None,
        };
        if options.faces.is_some() && options.rim_rig.is_some() {
            return Err("--faces and --rim-rig are exclusive: each frames the camera on its rig".to_owned());
        }
        if options.look_dev.is_some()
            && (options.stage.is_some()
                || options.environment
                || options.foliage.is_some()
                || options.outline_row
                || options.faces.is_some()
                || options.rim_rig.is_some()
                || options.hard_edges)
        {
            return Err(
                "--look-dev is a scene of its own: drop --stage, --environment, --probes, --foliage, \
                 --outline-row, --faces, --rim-rig and --hard-edges"
                    .to_owned(),
            );
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

/// `<preset>` or `<preset>@<seconds>`.
fn parse_post_effect(raw: &str) -> Result<(String, Option<f32>), String> {
    let invalid = || format!("--post-effect expects PRESET or PRESET@SECONDS, got {raw:?}");
    let (name, at) = match raw.split_once('@') {
        Some((name, seconds)) => {
            let seconds: f32 = seconds.parse().map_err(|_| invalid())?;
            if !(seconds.is_finite() && seconds >= 0.0) {
                return Err(invalid());
            }
            (name, Some(seconds))
        }
        None => (raw, None),
    };
    if name.is_empty() {
        return Err(invalid());
    }
    Ok((name.to_owned(), at))
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
                dynamic_resolution: true,
                shadows: Switch::Auto,
                ambient_occlusion: OcclusionSetting::Auto,
            }
        );
        let (pinned, _) = parse(&["--ao", "off"]).unwrap();
        assert_eq!(pinned.quality_settings().ambient_occlusion, OcclusionSetting::Off, "a real setting");
        assert_eq!(parse(&["--ao=capsule"]).unwrap().0.ambient_occlusion, Some(OcclusionSetting::Capsules));
        assert_eq!(parse(&["--ao", "full"]).unwrap().0.ambient_occlusion, Some(OcclusionSetting::Full));
        assert!(parse(&["--ao", "ssao"]).is_err());
        let (off, _) = parse(&["--shadows", "off"]).unwrap();
        assert_eq!(off.quality_settings().shadows, Switch::Off, "a real quality setting");
        assert_eq!(parse(&["--shadows=on"]).unwrap().0.shadows, Some(Switch::On));
        assert!(parse(&["--shadows", "soft"]).is_err());
        let (staged, _) = parse(&["--stage", "pillar"]).unwrap();
        assert_eq!(staged.stage, Some(StageLayout::Pillar));
        assert!(parse(&["--stage", "arena"]).is_err());
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
    fn look_dev_takes_a_shot_and_its_own_character() {
        let (options, _) = parse(&["--look-dev", "hero"]).unwrap();
        assert_eq!(options.look_dev, Some(Shot::Hero));
        assert_eq!(options.character(), lookdev::CHARACTER);
        assert!(options.keeps_character_mesh());
        assert_eq!(parse(&["--look-dev=wide"]).unwrap().0.look_dev, Some(Shot::Wide));
        assert!(parse(&["--look-dev", "close"]).unwrap_err().contains("wide, hero, sky"));
        assert!(parse(&["--look-dev", "sky", "--stage", "floor"]).is_err());
        assert!(parse(&["--environment", "--look-dev", "sky"]).is_err());
        assert!(parse(&["--look-dev", "wide", "--outline-row"]).is_err());
        assert!(parse(&["--faces", "--look-dev", "hero"]).is_err());
        assert!(parse(&["--look-dev", "hero", "--rim-rig", "angles"]).is_err());
        assert!(parse(&["--look-dev", "wide", "--hard-edges"]).is_err());
        let foliage = parse(&["--look-dev", "wide", "--foliage", "grove"]).unwrap_err();
        assert!(foliage.contains("--foliage"), "{foliage}");
        let defaults = Options::default();
        assert_eq!(defaults.character(), crate::content::CHARACTER);
        assert!(!defaults.keeps_character_mesh());
    }

    #[test]
    fn the_environment_is_off_unless_asked_for() {
        assert!(!parse(&[]).unwrap().0.environment);
        let (options, unknown) = parse(&["--environment", "--time-of-day", "18.5"]).unwrap();
        assert!(options.environment && !options.probes && unknown.is_empty());
        let (options, _) = parse(&["--probes"]).unwrap();
        assert!(options.probes && options.environment, "the grid lights the courtyard");
    }

    #[test]
    fn foliage_brings_the_courtyard_and_a_breeze() {
        let defaults = parse(&[]).unwrap().0;
        assert_eq!((defaults.foliage, defaults.wind()), (None, None), "calm without foliage");
        let (options, unknown) = parse(&["--foliage", "backlit"]).unwrap();
        assert!(unknown.is_empty());
        assert_eq!(options.foliage, Some(FoliageView::Backlit));
        assert!(options.environment, "in the courtyard");
        assert_eq!(options.wind(), Some(DEFAULT_WIND));
        let (options, _) = parse(&["--foliage=grove", "--wind", "2.5"]).unwrap();
        assert_eq!(options.wind(), Some(Wind { strength: 2.5, ..DEFAULT_WIND }));
        assert_eq!(parse(&["--wind", "0"]).unwrap().0.wind().map(|wind| wind.strength), Some(0.0));
        assert!(parse(&["--foliage", "forest"]).is_err());
        assert!(parse(&["--foliage"]).is_err(), "needs a view");
        assert!(parse(&["--wind", "5"]).is_err());
        assert!(parse(&["--wind", "NaN"]).is_err());
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
    fn post_options_drop_the_layers_and_set_sharpening() {
        let (options, unknown) = parse(&["--plain-post", "--sharpening", "0.4"]).unwrap();
        assert!(unknown.is_empty());
        assert!(options.plain_post && !Options::default().plain_post);
        assert_eq!(options.sharpening, Some(0.4));
        assert_eq!(parse(&[]).unwrap().0.sharpening, None);
        assert!(parse(&["--sharpening", "1.5"]).is_err());
        assert!(parse(&["--sharpening", "NaN"]).is_err());
    }

    #[test]
    fn the_debug_view_is_named_on_the_command_line() {
        let (options, unknown) = parse(&["--debug-view", "overdraw"]).unwrap();
        assert!(unknown.is_empty());
        assert_eq!(options.debug_view, Some(DebugView::Overdraw));
        assert_eq!(parse(&["--debug-view=Wireframe"]).unwrap().0.debug_view, Some(DebugView::Wireframe));
        assert_eq!(parse(&["--debug-view", "off"]).unwrap().0.debug_view, Some(DebugView::Off));
        assert_eq!(Options::default().debug_view, None);
        let error = parse(&["--debug-view", "normals"]).unwrap_err();
        assert!(error.contains("off, wireframe, overdraw, lights"), "{error}");
    }

    #[test]
    fn outline_options_set_the_scenes_the_lens_and_the_cutoff_scale() {
        let args = ["--outline-row", "--hard-edges", "--fov", "20", "--zoom=0.6", "--outline-distance", "0"];
        let (options, _) = parse(&args).unwrap();
        assert!(options.outline_row && options.hard_edges);
        assert_eq!((options.fov, options.zoom, options.outline_distance), (Some(20.0), Some(0.6), Some(0.0)));
        let defaults = Options::default();
        assert!(!defaults.outline_row && !defaults.hard_edges);
        assert_eq!((defaults.fov, defaults.zoom, defaults.outline_distance), (None, None, None));
        assert!(parse(&["--fov", "2"]).is_err());
        assert!(parse(&["--fov", "150"]).is_err());
        assert!(parse(&["--zoom", "0"]).is_err());
        assert!(parse(&["--zoom", "inf"]).is_err());
        assert!(parse(&["--outline-distance", "-1"]).is_err());
        assert!(parse(&["--outline-distance", "5"]).is_err());
    }

    #[test]
    fn look_dev_rigs_take_their_layouts() {
        assert_eq!(parse(&["--faces"]).unwrap().0.faces, Some(FaceLayout::Lights));
        assert_eq!(parse(&["--faces", "--face-layout", "turn"]).unwrap().0.faces, Some(FaceLayout::Turn));
        assert_eq!(parse(&["--face-layout=hair", "--faces"]).unwrap().0.faces, Some(FaceLayout::Hair));
        let gi = parse(&["--faces", "--face-layout", "gi-clamped"]).unwrap().0.faces;
        assert_eq!(gi, Some(FaceLayout::GiClamped));
        assert!(parse(&["--face-layout", "hair"]).is_err(), "a layout needs the rig");
        assert!(parse(&["--faces", "--face-layout", "bangs"]).is_err());
        assert_eq!(parse(&["--rim-rig", "backlit"]).unwrap().0.rim_rig, Some(RimLayout::Backlit));
        assert!(parse(&["--rim-rig", "front"]).is_err());
        assert!(parse(&["--faces", "--rim-rig", "angles"]).is_err(), "one rig frames the camera");
        let defaults = Options::default();
        assert_eq!((defaults.faces, defaults.rim_rig), (None, None));
    }

    #[test]
    fn hot_reload_is_on_unless_turned_off() {
        assert!(!parse(&[]).unwrap().0.no_hot_reload);
        assert!(parse(&["--serial-passes"]).unwrap().0.serial_passes && !Options::default().serial_passes);
        let (options, unknown) = parse(&["--no-hot-reload", "--bench", "10"]).unwrap();
        assert!(options.no_hot_reload && unknown.is_empty());
    }

    #[test]
    fn live_link_listens_dials_or_is_off() {
        assert_eq!(parse(&[]).unwrap().0.live_link, None, "the default: loopback");
        let live_link = |arg: &str| parse(&["--live-link", arg]).map(|(options, _)| options.live_link);
        assert_eq!(live_link("off"), Ok(Some(LiveLinkOption::Off)));
        let lan: SocketAddr = "0.0.0.0:7811".parse().unwrap();
        assert_eq!(live_link("0.0.0.0:7811"), Ok(Some(LiveLinkOption::Listen(lan))));
        assert_eq!(live_link("listen=0.0.0.0:7811"), Ok(Some(LiveLinkOption::Listen(lan))));
        let host: SocketAddr = "127.0.0.1:7811".parse().unwrap();
        assert_eq!(live_link("connect=127.0.0.1:7811"), Ok(Some(LiveLinkOption::Connect(host))));
        assert_eq!(
            parse(&["--live-link=connect=[::1]:7811"]).unwrap().0.live_link,
            Some(LiveLinkOption::Connect("[::1]:7811".parse().unwrap()))
        );
        assert!(
            live_link("localhost:7811").unwrap_err().contains("127.0.0.1:7811"),
            "an address, not a name"
        );
        assert!(live_link("connect=7811").is_err());
        assert!(live_link("dial=127.0.0.1:7811").is_err());
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
    fn post_effects_repeat_and_may_be_held() {
        let (options, _) = parse(&[
            "--post-effect",
            "impact_frame@0.03",
            "--post-effect=speed_lines",
            "--post-effect",
            "a@0",
        ])
        .unwrap();
        assert_eq!(
            options.post_effects,
            [
                ("impact_frame".to_owned(), Some(0.03)),
                ("speed_lines".to_owned(), None),
                ("a".to_owned(), Some(0.0))
            ]
        );
        for bad in ["@0.1", "radial_blur@", "radial_blur@-1", "radial_blur@soon", "radial_blur@inf"] {
            assert!(parse(&["--post-effect", bad]).is_err(), "{bad}");
        }
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
