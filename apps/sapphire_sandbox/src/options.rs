//! Command-line options.

use std::net::SocketAddr;
use std::num::NonZeroU32;
use std::path::PathBuf;

use sapphire_render::quality::{OcclusionSetting, QualitySettings, Switch, TierId};
use sapphire_render::{DebugView, LightPath, MIN_RENDER_SCALE, Msaa, PostVariants, RenderSettings, Wind};
use sapphire_ui::Insets;
use sapphire_world::streaming::{StreamBudget, StreamSettings};
use sapphire_world::tod::CharacterShadeKey;

use crate::screen::ScreenOptions;
use game_combat::arena::ArenaShot;
use game_combat::combat::{CaptureFrames, CombatOption};
use game_combat::combat_views::CombatViews;
use game_ui::hud::{HudMode, HudOptions};
use game_ui::menus::MenuScreen;
use sapphire_stage::anim_graph::GraphScript;
use sapphire_stage::audio::CueOption;
use sapphire_stage::debug_shapes::DebugDrawDemo;
use sapphire_stage::faces::{FaceLayout, MaterialOverrides};
use sapphire_stage::foliage::{DEFAULT_WIND, FoliageView};
use sapphire_stage::lookdev::{self, Shot};
use sapphire_stage::rims::RimLayout;
use sapphire_stage::scene::{self, Fade, Scene};
use sapphire_stage::studio::StudioLayout;
use sapphire_stage::vfx::VfxShow;

pub(crate) const USAGE: &str = "\
Usage: sapphire_sandbox [options]

Modes (offscreen modes never open a window):
  --capture <file.png>       Capture a scene and exit
  --bench <frames>           Benchmark a scene offscreen
  --offscreen               Run the scene without a window
  --contact-sheet <move> <file.png>  Capture animation samples

Common options:
  --help, -h                Print this help and exit before platform initialization
  --frames <count>          Exit after this many frames
  --size <width>x<height>    Offscreen image size
  --tier <tier>             desktop-high, desktop-low, mobile-high or mobile-low
  --aa off|msaa|taa         Antialiasing override
  --render-scale <0.25..1>   Render scale override
  --look-dev <scene>        Select a look-development scene
  --replay <file>           Play a recorded simulation
  --record <file>           Record a simulation
  --scenario <name>         Scenario for a recording or fight
  --script <name>           Script driving a recording or fight
  --fight                   Run a scripted fight

The sapphire_sandbox crate's Arguments documentation lists all scene, capture,
benchmark, animation, HUD and development options (cargo doc -p sapphire_sandbox).";

pub(crate) fn is_help_flag(arg: &str) -> bool {
    matches!(arg, "--help" | "-h")
}

/// The flags that select an offscreen mode; see [`Offscreen`].
pub(crate) const CAPTURE_FLAG: &str = "--capture";
pub(crate) const BENCH_FLAG: &str = "--bench";
pub(crate) const OFFSCREEN_FLAG: &str = "--offscreen";
pub(crate) const CONTACT_SHEET_FLAG: &str = "--contact-sheet";
/// Shows the debug overlay from the start (`dev` builds), with its default panels or those
/// listed: `--debug-overlay=stats,graph`.
const DEBUG_OVERLAY_FLAG: &str = "--debug-overlay";

/// Whether `arg` selects an offscreen mode, as `--capture`, `--bench`, `--offscreen`,
/// `--contact-sheet` or their `--flag=value` forms. `main` checks this before SDL starts, so the windowed app never opens
/// for one.
pub(crate) fn is_offscreen_flag(arg: &str) -> bool {
    let name = arg.split_once('=').map_or(arg, |(name, _)| name);
    name == CAPTURE_FLAG || name == BENCH_FLAG || name == OFFSCREEN_FLAG || name == CONTACT_SHEET_FLAG
}

/// What `--debug-draw` draws.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum DebugDraw {
    /// A fixed test of the debug draw path.
    Demo(DebugDrawDemo),
    /// These views of the fight `--replay` or `--record` runs, which the fight's `draw_views` draws.
    Views(CombatViews),
}

impl DebugDraw {
    fn parse(value: &str) -> Result<Self, String> {
        match value {
            "volumes" => Ok(Self::Views(CombatViews::VOLUMES)),
            "colliders" => Ok(Self::Views(CombatViews::COLLIDERS)),
            "combat" => Ok(Self::Views(CombatViews::ALL)),
            _ => DebugDrawDemo::parse(value).map(Self::Demo).ok_or_else(|| {
                format!(
                    "--debug-draw expects shapes, lines:<count>, volumes, colliders or combat, got {value:?}"
                )
            }),
        }
    }
}

/// Options parsed from the process arguments. The default is no options: everything from the
/// quality tier and the scene.
/// How the town's zone streams (`--stream`, M3 W2). Nothing it says reaches a simulation, which is what
/// `the_town_streaming_changes_nothing_the_checksums_see` holds.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum StreamChoice {
    /// As the tier's class of device does.
    #[default]
    Tier,
    /// `off`: the zone isn't opened, so the town has no cells.
    Off,
    /// `<entities>:<metres>`: that many entities a frame, and an activation radius.
    Limits { entities: u32, activate: f32 },
}

impl StreamChoice {
    fn parse(text: &str) -> Result<Self, String> {
        if text == "off" {
            return Ok(Self::Off);
        }
        let limits = text.split_once(':').and_then(|(entities, activate)| {
            let activate =
                activate.parse::<f32>().ok().filter(|metres| metres.is_finite() && *metres >= 0.0)?;
            Some(Self::Limits { entities: entities.parse().ok()?, activate })
        });
        limits.ok_or_else(|| format!("--stream expects off or <entities>:<metres>, got {text:?}"))
    }

    /// The settings on `tier`, if the zone opens.
    pub fn settings(self, tier: TierId) -> Option<StreamSettings> {
        let settings = scene::stream_settings(tier);
        match self {
            Self::Tier => Some(settings),
            Self::Off => None,
            Self::Limits { entities, activate } => Some(StreamSettings {
                activate,
                load: activate + 16.0,
                release: activate + 32.0,
                budget: StreamBudget { entities, ..settings.budget },
            }),
        }
    }
}

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
    pub stage: Option<StudioLayout>,
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
    /// `--shade-hue <r>,<g>,<b>,<share>`: key characters' shadow sides to keep this linear RGB
    /// hue by this share of their light, all day (M1 C8), into the day the scene runs on.
    pub shade_hue: Option<CharacterShadeKey>,
    /// `--camera-rim <share>`: give every light rig preset a camera rim taking this share of the
    /// rim's gating, 0 to 1 (M1 C8).
    pub camera_rim: Option<f32>,
    /// `--swatches`: place the anime character shader swatches beside the character.
    pub swatches: bool,
    /// `--materials`: place the material layer samples on the character's right (M1 C7).
    pub materials: bool,
    /// `--two-sided`: place the two-sided garment samples on the character's left (M1 C8).
    pub two_sided: bool,
    /// `--hair-shadow-softness <meters>`: soften every face material's bangs' shadow to an edge
    /// this wide (M1 C8).
    pub hair_shadow_softness: Option<f32>,
    /// `--hair-transmission <strength>`: light every hair material through by this strength
    /// against a back light (M1 C8).
    pub hair_transmission: Option<f32>,
    /// `--fade <0..1>`: draw the characters at this opacity, through the proximity fade's
    /// dither (M1 C7).
    pub fade: Option<f32>,
    /// `--proximity-fade`: fade the characters as the camera nears their bodies (M1 C7).
    pub proximity_fade: bool,
    /// `--faces [--face-layout <layout>]`: show the face look-dev rig instead of the character,
    /// in this layout (`lights` without `--face-layout`; the layouts are `FaceLayout::NAMES`:
    /// lights, hair, highlights, turn, gi, gi-clamped, eyes, eyes-turned, gaze and blink).
    pub faces: Option<FaceLayout>,
    /// `--rim-rig angles|backlit|folded`: show the rim look-dev rig instead of the character
    /// (M1 C5; `folded`, M1 C8).
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
    /// `--look-dev wide|hero|sky|street|face|enemy|body|full|down`: the look-dev town (M1 X1) with the VRM
    /// character standing in it, seen from that shot; `enemy` adds the enemy (M2 O4). A scene of its own rather
    /// than a `--stage` layout: it replaces the character and the ground the other scenes put under
    /// it, so it excludes `--stage`,
    /// `--environment` (and `--probes`, which implies it) and `--foliage`, and the scenes that
    /// frame more of the mannequin (`--outline-row`, `--faces`, `--rim-rig`, `--hard-edges`).
    pub look_dev: Option<Shot>,
    /// `--hud fixed|live`, `--hud-insets`, `--hud-scale`, `--hud-paused` and `--hud-energy`: the combat HUD over the scene
    /// (M2 I2).
    pub hud: Option<HudOptions>,
    /// `--ui-game`, `--ui-locale`, `--ui-scale`, `--ui-pseudo`, `--ui-plain`, `--ui-time`, `--ui-walk`,
    /// `--ui-turn` and `--ui-spin`: one of the game's screens over the scene (M3 U7, C3).
    pub ui_screen: Option<ScreenOptions>,
    /// `--display-text latin|japanese`: a title in the display face at four sizes over the frame
    /// (M3 U8, `display_text.rs`).
    pub display_text: Option<crate::display_text::DisplaySample>,
    /// `--clip <name>`: plays that clip on the look-dev character instead of holding it (M2 N2).
    /// Needs `--look-dev` or `--arena`, where it plays in place, since the simulation owns the root.
    pub clip: Option<lookdev::ClipName>,
    /// `--clip-in-place`: leaves the clip's root motion out, so the character stays where a close
    /// shot framed it. Needs `--clip`.
    pub clip_in_place: bool,
    /// `--model <mesh>[:<clip>[@<tick>]]`: a skinned model by its asset ID, standing where the enemy
    /// shot's warrior does and in its place, at scale 1, in its rest pose or the clip's pose at that
    /// tick (M3 C2, [`lookdev::model`]): a new enemy, looked at before anything plays it. Needs
    /// `--look-dev`.
    pub model: Option<lookdev::model::Model>,
    /// `--blade on|off`: the player's sword in the character's right hand (M2 O3). On for
    /// `--look-dev body` and `--fight`, off anywhere else unless it says, so no older scene draws one.
    /// Needs `--look-dev` or `--arena`. See [`Self::blade`].
    pub blade: Option<bool>,
    /// `--springs on|off`: whether the character's spring bones run under its clip or graph (M2 N5). On
    /// unless it says off, and a character holding still has none. Needs `--clip` or `--graph`.
    pub springs: Option<bool>,
    /// `--arena`: the combat stage (M2 O5, [`game_combat::arena`]), the look-dev town's square as a place
    /// to fight, framed by this shot (`--arena-shot`). A scene of its own, as `--look-dev` is: it
    /// excludes what that excludes, and `--look-dev` too.
    pub arena: Option<ArenaShot>,
    /// `--timeline <uuid>`: plays this timeline's presentation from the first frame (M3 Q1), its shots
    /// through the director and in the arena's own space. Needs `--arena`, on the director's shot.
    pub timeline: Option<sapphire_assets::AssetId>,
    /// `--party`: the arena's fight is a party's (M2 G7), so the player draws whoever's on the field, in
    /// that member's recolor (`game_combat::party_looks`). Needs the `party` scenario, which `--fight` then plays.
    pub party: bool,
    /// `--graph`: the animation graph on the look-dev character, from this script (M2 N3).
    pub graph: Option<GraphScript>,
    /// `--ik on|off` (off by default), `--ik-ground flat|step|slope`, `--ik-look x,y,z|orbit`: solves IK
    /// over the clip or the graph (M2 N4, [`sapphire_stage::ik`]), the character standing on that ground and
    /// looking there. Needs `--clip` or `--graph`. On `--arena` its ground is the arena's colliders.
    pub ik: Option<sapphire_stage::ik::IkOptions>,
    /// `--cloud-cover <0..1>`: both cloud layers' coverage instead of time of day's; 0 is a
    /// clear sky.
    pub cloud_cover: Option<f32>,
    /// `--plain-sky`: the sky's gradient alone, with no discs, glows, stars or clouds.
    pub plain_sky: bool,
    /// `--stream`: how the town's zone streams.
    pub stream: StreamChoice,
    /// `--fog off`: no fog (M1 W3): time of day's fog left out of every frame, and no fog cards
    /// in the courtyard. `--fog on` is the default.
    pub fog_off: bool,
    /// `--plain-post`: post as it was before its compositing layers (M1 P2), with no diffusion
    /// glow, character bloom, gradient layers, lens flare, vignette tint or sharpening.
    pub plain_post: bool,
    /// `--view-culling off`: draw every static world instance in the main view, culling none
    /// (review RA-O01), for comparisons. `--view-culling on` is the default.
    pub view_culling_off: bool,
    /// `--sharpening <0..1>`: the player's sharpening instead of the `post.sharpening` cvar's.
    pub sharpening: Option<f32>,
    /// `--post-effect <preset>[@<seconds>]`, repeatable: screen effect presets pushed at the start
    /// (M1 P3), each playing from its start, or held that many seconds into it (for captures and
    /// benches). The scene checks the names against its presets.
    pub post_effects: Vec<(String, Option<f32>)>,
    /// `--vfx arc|hit|dust|duel`: plays effects at fixed ticks (M2 E2).
    pub vfx: Option<VfxShow>,
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
    /// `--debug-draw <what>`: turn debug draw on and draw this (M2 T1).
    pub debug_draw: Option<DebugDraw>,
    /// `--cue <name> [--cue-repeat <seconds>]`: play this cue on the real audio device (M2 E1).
    pub cue: Option<CueOption>,
    /// Render offscreen instead of opening a window.
    pub offscreen: Option<Offscreen>,
    /// `--replay`, `--record` or `--fight`: a combat simulation beside the scene (M2 D2, G6).
    pub combat: Option<CombatOption>,
    /// `--cues`: fire that simulation's cues, as a game does (M2 E3).
    pub cues: bool,
    /// `--bench-trace <csv>`: a bench of a fight writes every frame here (M2 Z1, `fight_bench.rs`).
    pub bench_trace: Option<PathBuf>,
    /// `--draw-every <n>`: every frame steps, animates, prepares and records, and only every `n`th runs its
    /// graph, the first among them, losing the rest as a frame whose surface was lost. It's for a long run on a
    /// software GPU, where drawing is what costs: the renderer is still beside every frame, and nothing the
    /// simulation sees changes. With `--offscreen` or `--capture`, whose own frame always draws; or with
    /// `--bench` of a fight or a zone, which plays once or walks its frames, checks the load's inventory and
    /// measures nothing (M3 K1): every pipeline is built before a graph runs, so one that isn't counts.
    pub draw_every: Option<NonZeroU32>,
    /// `--zone <stem>[:<spawner>]`: the zone stage (M3 C1): the character at a spawner of the zone cooked from
    /// `<stem>.zone.ron`, `player_start` unless named, under the zone's day and light rigs.
    pub zone: Option<(String, String)>,
    /// `--zone-walk <spline>[:<metres a second>]`: the zone stage's character walks that rail of the zone's, at
    /// the game's run speed unless told.
    pub zone_walk: Option<(String, f32)>,
    /// `--inventory-without <entry>`: a bench leaves this entry out of what it prewarms, by the name a miss gives
    /// it: `mesh` and a key, `post`, `effects`, `skinning`, `ui`, `sky` or `ambient` (M3 K1). A bench of a fight
    /// then fails naming what it had to build, and a bench of the zone stage says so.
    pub inventory_without: Option<String>,
    /// `--watch-document <path>` (`dev` builds, repeatable): hot reload watches this file as well, as the
    /// document of its file's name: a tuner's copy of a fight's moves, say (M2 T2).
    pub watch_documents: Vec<PathBuf>,
    /// `--serial-passes`: offscreen, submit each frame graph pass on its own and wait for the
    /// GPU after it, logging each at debug level, so a GPU crash, hang or device loss names its
    /// pass (`GraphExecutor::set_serial`). Slow; for debugging.
    pub serial_passes: bool,
    /// `--debug-overlay[=panels]`: the overlay starts shown, with these panels (empty: the
    /// default ones). Only `dev` builds have it.
    pub debug_overlay: Option<String>,
    /// `--no-hot-reload`: watch no files. Only `dev` builds watch any.
    pub no_hot_reload: bool,
    /// `--combat-input`: the windowed app's input is combat's bindings alone.
    pub combat_input: bool,
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
    /// `--contact-sheet <clip> <png> [--sheet-cells N] [--sheet-ticks a,b,..] [--sheet-views a,b,..]
    /// [--size <cell>]`: the clip on the look-dev character at evenly spaced ticks or those named,
    /// seen from the front and the side or the yaws named, as one image (see
    /// [`crate::contact_sheet`]).
    ContactSheet { path: PathBuf, cells: u32, ticks: Vec<u32>, yaws: Vec<i32> },
    /// `--offscreen` (`dev` builds): draw the scene at up to 60 frames a second, one fixed step
    /// each, until the process is stopped or `--frames` have been drawn, applying hot reload's
    /// changes and live link's pushes, and answering live link's captures. The desktop's way to
    /// run what a device runs, without a window.
    Serve,
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
        if let Some(shade) = self.shade_hue {
            scene.set_shade_hue(shade)?;
        }
        if let Some(share) = self.camera_rim {
            scene.set_camera_rim(share)?;
        }
        scene.frame_foliage();
        if let Some(wind) = self.wind() {
            scene.set_wind(wind);
        }
        scene.set_cloud_cover(self.cloud_cover);
        scene.set_plain_sky(self.plain_sky);
        scene.set_fog(!self.fog_off);
        scene.set_plain_post(self.plain_post);
        scene.set_sharpening(self.sharpening);
        scene.set_fade(self.fade());
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
        match self.debug_draw {
            Some(DebugDraw::Demo(demo)) => scene.set_debug_draw_demo(demo),
            Some(DebugDraw::Views(views)) => {
                sapphire_core::debug_draw::set_enabled(true);
                views.turn_on();
            }
            None => {}
        }
        for (preset, at) in &self.post_effects {
            scene.push_effect(preset, *at)?;
        }
        Ok(())
    }

    /// How the characters fade: at `--fade`'s opacity, by `--proximity-fade`'s default
    /// distances, or not at all.
    pub fn fade(&self) -> Fade {
        match (self.fade, self.proximity_fade) {
            (Some(opacity), _) => Fade::Fixed(opacity),
            (None, true) => Fade::Proximity(Default::default()),
            (None, false) => Fade::Off,
        }
    }

    /// The scene's wind: `--foliage`'s breeze at `--wind`'s strength, if either is set.
    pub fn wind(&self) -> Option<Wind> {
        (self.foliage.is_some() || self.wind.is_some())
            .then(|| Wind { strength: self.wind.unwrap_or(DEFAULT_WIND.strength), ..DEFAULT_WIND })
    }

    /// Whether `sapphire_camera`'s director frames the scene: `--arena`, on its own shot.
    #[cfg(feature = "dev")]
    pub fn has_camera_director(&self) -> bool {
        self.arena == Some(ArenaShot::Director)
    }

    /// What live tuning follows in this run.
    #[cfg(feature = "dev")]
    pub fn tuned_run(&self) -> game_combat::tuning::TunedRun<'_> {
        game_combat::tuning::TunedRun {
            camera_director: self.has_camera_director(),
            cues: self.cues,
            graph_show: self.graph.is_some(),
            arena_fight: self.arena.is_some() && self.combat.is_some(),
            screen: self.ui_screen.as_ref().map(|shown| shown.screen.source()),
            hud: self.hud.is_some(),
            reads_files: !self.no_hot_reload,
            extra_documents: &self.watch_documents,
        }
    }

    /// Whether the scene is the look-dev town: `--look-dev`, or `--arena`, which fights in it.
    pub fn in_town(&self) -> bool {
        self.look_dev.is_some() || self.arena.is_some()
    }

    /// Whether the look-dev character, its pose and its sky are on stage: in the town or in a zone.
    pub fn look_dev_character(&self) -> bool {
        self.in_town() || self.zone.is_some()
    }

    /// Whether the character holds the player's sword: as `--blade` says, and on `--look-dev body`.
    pub fn blade(&self) -> bool {
        // A fight, or the arena's recording of someone playing one.
        let fighting = match self.combat {
            Some(CombatOption::Fight { .. }) => true,
            Some(CombatOption::Record { script: None, .. }) => self.arena.is_some(),
            _ => false,
        };
        self.blade.unwrap_or(self.look_dev == Some(Shot::Body) || fighting)
    }

    /// The character the scene draws: the look-dev VRM in the town or a zone, the mannequin otherwise.
    pub fn character(&self) -> sapphire_assets::AssetId {
        if self.look_dev_character() { lookdev::CHARACTER } else { sapphire_stage::content::CHARACTER }
    }

    /// The sky material the scene draws: the look-dev town's in the town, the sandbox's otherwise.
    pub fn sky(&self) -> sapphire_assets::AssetId {
        if self.look_dev_character() { lookdev::SKY } else { sapphire_stage::content::SKY }
    }

    /// Whether the content keeps a copy of the character's mesh: the stage and the rim rig draw
    /// the character from it without its pedestal, and the look-dev stage derives its held pose
    /// from it.
    pub fn keeps_character_mesh(&self) -> bool {
        self.stage.is_some() || self.rim_rig.is_some() || self.look_dev_character()
    }

    /// What `--hair-shadow-softness` and `--hair-transmission` change in the materials the
    /// sandbox loads (M1 C8).
    pub fn material_overrides(&self) -> MaterialOverrides {
        MaterialOverrides {
            hair_shadow_softness: self.hair_shadow_softness,
            hair_transmission: self.hair_transmission,
        }
    }

    /// The renderer settings: the quality tier's, with the options' overrides. `--msaa` wins
    /// over `--aa`'s sample count. Under `--plain-post` no frame draws a compositing layer, so
    /// post builds none of their variants.
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
            post_variants: if self.plain_post { PostVariants::NONE } else { tier.post_variants },
            view_culling: tier.view_culling && !self.view_culling_off,
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
    /// so the caller can warn about them; SDL and the OS may pass their own. A scripted run
    /// refuses them instead: see [`Options::refuse_unknown`].
    pub fn parse(args: &[String]) -> Result<(Self, Vec<String>), String> {
        let mut options = Self::default();
        let mut unknown = Vec::new();
        let (mut capture, mut bench, mut capture_ticks, mut size) = (None, None, None, None);
        let mut serve = false;
        let (mut cue, mut cue_repeat) = (None, None);
        let (mut capture_frames, mut capture_step) = (None, None);
        let (mut ik, mut ik_ground, mut ik_look) = (false, None, None);
        let (mut faces, mut face_layout) = (false, None);
        let (mut replay, mut record, mut scenario, mut script) = (None, None, None, None);
        let mut fight = false;
        let (mut hud_mode, mut hud_insets, mut hud_scale) = (None, None, None);
        let mut hud_paused = false;
        let mut hud_energy = None;
        let (mut ui_game, mut ui_time, mut ui_walk) = (None, None, Vec::new());
        let (mut ui_locale, mut ui_scale, mut ui_pseudo) = (None, None, false);
        let (mut ui_turn, mut ui_spin) = (None, None);
        let mut ui_plain = false;
        let (mut arena_flag, mut arena_shot) = (false, None);
        let (mut sheet, mut sheet_cells) = (None, None);
        let (mut sheet_ticks, mut sheet_views): (Vec<u32>, Vec<i32>) = (Vec::new(), Vec::new());
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
                        Some(StudioLayout::from_name(&raw).ok_or_else(|| {
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
                "--shade-hue" => options.shade_hue = Some(parse_shade_hue(&value("--shade-hue")?)?),
                "--camera-rim" => {
                    let share: f32 = number(&value("--camera-rim")?, "--camera-rim")?;
                    if !(0.0..=1.0).contains(&share) {
                        return Err(format!("--camera-rim expects a share from 0 to 1, got {share}"));
                    }
                    options.camera_rim = Some(share);
                }
                "--swatches" => options.swatches = true,
                "--materials" => options.materials = true,
                "--two-sided" => options.two_sided = true,
                "--hair-shadow-softness" => {
                    let softness: f32 = number(&value("--hair-shadow-softness")?, "--hair-shadow-softness")?;
                    if !(softness.is_finite() && softness >= 0.0) {
                        return Err(format!(
                            "--hair-shadow-softness expects meters, at least 0, got {softness}"
                        ));
                    }
                    options.hair_shadow_softness = Some(softness);
                }
                "--hair-transmission" => {
                    let strength: f32 = number(&value("--hair-transmission")?, "--hair-transmission")?;
                    if !(strength.is_finite() && strength >= 0.0) {
                        return Err(format!(
                            "--hair-transmission expects a strength, at least 0, got {strength}"
                        ));
                    }
                    options.hair_transmission = Some(strength);
                }
                "--fade" => {
                    let opacity: f32 = number(&value("--fade")?, "--fade")?;
                    if !(0.0..=1.0).contains(&opacity) {
                        return Err(format!("--fade expects an opacity from 0 to 1, got {opacity}"));
                    }
                    options.fade = Some(opacity);
                }
                "--proximity-fade" => options.proximity_fade = true,
                "--faces" => faces = true,
                "--face-layout" => {
                    let raw = value("--face-layout")?;
                    face_layout = Some(FaceLayout::from_name(&raw).ok_or_else(|| {
                        format!("--face-layout expects one of {}, got {raw:?}", FaceLayout::NAMES.join(", "))
                    })?);
                }
                "--rim-rig" => {
                    let raw = value("--rim-rig")?;
                    options.rim_rig = Some(RimLayout::from_name(&raw).ok_or_else(|| {
                        format!("--rim-rig expects angles, backlit or folded, got {raw:?}")
                    })?);
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
                "--hud" => {
                    let mode = match value("--hud")?.as_str() {
                        "fixed" => HudMode::Fixed,
                        "live" => HudMode::Live,
                        other => return Err(format!("--hud expects fixed or live, got {other:?}")),
                    };
                    hud_mode = Some(mode);
                }
                "--display-text" => {
                    options.display_text =
                        Some(crate::display_text::DisplaySample::parse(&value("--display-text")?)?);
                }
                "--draw-every" => options.draw_every = Some(number(&value("--draw-every")?, "--draw-every")?),
                "--inventory-without" => options.inventory_without = Some(value("--inventory-without")?),
                "--hud-insets" => hud_insets = Some(parse_insets(&value("--hud-insets")?)?),
                "--hud-paused" => hud_paused = true,
                "--hud-energy" => {
                    let energy: f32 = number(&value("--hud-energy")?, "--hud-energy")?;
                    if !(0.0..=1.0).contains(&energy) {
                        return Err(format!("--hud-energy expects 0 to 1, got {energy}"));
                    }
                    hud_energy = Some(energy);
                }
                "--ui-game" => {
                    let stem = value("--ui-game")?;
                    let names = || MenuScreen::ALL.map(MenuScreen::stem).join(", ");
                    ui_game = Some(MenuScreen::from_stem(&stem).ok_or_else(|| {
                        format!("--ui-game: `{stem}` isn't one of the game's screens: {}", names())
                    })?);
                }
                "--ui-turn" => {
                    let across: f32 = number(&value("--ui-turn")?, "--ui-turn")?;
                    if !across.is_finite() {
                        return Err(format!("--ui-turn expects a share of a drag across, got {across}"));
                    }
                    ui_turn = Some(across);
                }
                "--ui-spin" => {
                    let across: f32 = number(&value("--ui-spin")?, "--ui-spin")?;
                    if !across.is_finite() {
                        return Err(format!("--ui-spin expects a share of a drag across, got {across}"));
                    }
                    ui_spin = Some(across);
                }
                "--ui-time" => {
                    let seconds: f32 = number(&value("--ui-time")?, "--ui-time")?;
                    if !(seconds >= 0.0 && seconds.is_finite()) {
                        return Err(format!("--ui-time expects a time in seconds, got {seconds}"));
                    }
                    ui_time = Some(seconds);
                }
                "--ui-walk" => {
                    ui_walk = value("--ui-walk")?.split(',').map(str::to_owned).collect();
                }
                "--ui-locale" => ui_locale = Some(value("--ui-locale")?),
                "--ui-pseudo" => ui_pseudo = true,
                "--ui-plain" => ui_plain = true,
                "--ui-scale" => {
                    let scale: f32 = number(&value("--ui-scale")?, "--ui-scale")?;
                    if !(scale > 0.0 && scale.is_finite()) {
                        return Err(format!("--ui-scale expects a positive scale, got {scale}"));
                    }
                    ui_scale = Some(scale);
                }
                "--hud-scale" => {
                    let scale: f32 = number(&value("--hud-scale")?, "--hud-scale")?;
                    if !(scale.is_finite() && scale > 0.0) {
                        return Err(format!("--hud-scale expects a positive scale, got {scale}"));
                    }
                    hud_scale = Some(scale);
                }
                "--look-dev" => {
                    let raw = value("--look-dev")?;
                    options.look_dev = Some(Shot::from_name(&raw).ok_or_else(|| {
                        let names: Vec<&str> = Shot::ALL.iter().map(|shot| shot.name()).collect();
                        format!("--look-dev expects one of {}, got {raw:?}", names.join(", "))
                    })?);
                }
                "--clip" => options.clip = Some(lookdev::ClipName::parse(&value("--clip")?)),
                "--clip-in-place" => options.clip_in_place = true,
                "--model" => {
                    options.model =
                        Some(value("--model")?.parse().map_err(|error| format!("--model: {error}"))?)
                }
                "--arena" => arena_flag = true,
                "--party" => options.party = true,
                "--timeline" => {
                    let raw = value("--timeline")?;
                    let id = raw.parse().map_err(|error| format!("--timeline expects a UUID: {error}"))?;
                    options.timeline = Some(id);
                }
                "--arena-shot" => {
                    let raw = value("--arena-shot")?;
                    arena_shot = Some(ArenaShot::from_name(&raw).ok_or_else(|| {
                        let names: Vec<&str> = ArenaShot::ALL.iter().map(|shot| shot.name()).collect();
                        format!("--arena-shot expects one of {}, got {raw:?}", names.join(", "))
                    })?);
                }
                "--blade" => {
                    options.blade = Some(match value("--blade")?.as_str() {
                        "on" => true,
                        "off" => false,
                        other => return Err(format!("--blade expects on or off, got {other:?}")),
                    });
                }
                "--springs" => {
                    options.springs = Some(match value("--springs")?.as_str() {
                        "on" => true,
                        "off" => false,
                        other => return Err(format!("--springs expects on or off, got {other:?}")),
                    });
                }
                "--graph" => {
                    let raw = value("--graph")?;
                    options.graph = Some(GraphScript::from_name(&raw).ok_or_else(|| {
                        let names: Vec<&str> = GraphScript::ALL.iter().map(|script| script.name()).collect();
                        format!("--graph expects one of {}, got {raw:?}", names.join(", "))
                    })?);
                }
                "--ik" => {
                    ik = match value("--ik")?.as_str() {
                        "on" => true,
                        "off" => false,
                        other => return Err(format!("--ik expects on or off, got {other:?}")),
                    };
                }
                "--ik-ground" => {
                    let raw = value("--ik-ground")?;
                    ik_ground = Some(sapphire_stage::ik::IkGround::from_name(&raw).ok_or_else(|| {
                        let names: Vec<_> =
                            sapphire_stage::ik::IkGround::ALL.iter().map(|ground| ground.name()).collect();
                        format!("--ik-ground expects one of {}, got {raw:?}", names.join(", "))
                    })?);
                }
                "--ik-look" => {
                    ik_look = Some(sapphire_stage::ik::IkLook::parse(&value("--ik-look")?)?);
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
                "--debug-draw" => {
                    options.debug_draw = Some(DebugDraw::parse(&value("--debug-draw")?)?);
                }
                "--plain-sky" => options.plain_sky = true,
                "--stream" => options.stream = StreamChoice::parse(&value("--stream")?)?,
                "--zone" => {
                    let raw = value("--zone")?;
                    let (stem, at) = raw.split_once(':').unwrap_or((&raw, "player_start"));
                    options.zone = Some((stem.to_owned(), at.to_owned()));
                }
                "--zone-walk" => {
                    let raw = value("--zone-walk")?;
                    let (spline, speed) =
                        raw.split_once(':').map_or(Ok((raw.as_str(), None)), |(spline, speed)| {
                            number(speed, "--zone-walk").map(|speed: f32| (spline, Some(speed)))
                        })?;
                    let speed = speed.unwrap_or_else(|| sapphire_gameplay::arena::profile().run_speed);
                    options.zone_walk = Some((spline.to_owned(), speed));
                }
                "--fog" => {
                    options.fog_off = match value("--fog")?.as_str() {
                        "on" => false,
                        "off" => true,
                        other => return Err(format!("--fog expects on or off, got {other:?}")),
                    };
                }
                "--plain-post" => options.plain_post = true,
                "--view-culling" => {
                    options.view_culling_off = match value("--view-culling")?.as_str() {
                        "on" => false,
                        "off" => true,
                        other => return Err(format!("--view-culling expects on or off, got {other:?}")),
                    };
                }
                "--sharpening" => {
                    let amount: f32 = number(&value("--sharpening")?, "--sharpening")?;
                    if !(0.0..=1.0).contains(&amount) {
                        return Err(format!("--sharpening expects 0 to 1, got {amount}"));
                    }
                    options.sharpening = Some(amount);
                }
                "--post-effect" => options.post_effects.push(parse_post_effect(&value("--post-effect")?)?),
                "--cue" => cue = Some(sapphire_stage::audio::named_cue(&value("--cue")?)?),
                "--cue-repeat" => {
                    let seconds: f32 = number(&value("--cue-repeat")?, "--cue-repeat")?;
                    if !(seconds.is_finite() && seconds > 0.0) {
                        return Err(format!(
                            "--cue-repeat expects a positive number of seconds, got {seconds}"
                        ));
                    }
                    cue_repeat = Some(seconds);
                }
                "--cues" => options.cues = true,
                "--vfx" => {
                    let raw = value("--vfx")?;
                    options.vfx = Some(VfxShow::from_name(&raw).ok_or_else(|| {
                        let names: Vec<&str> = VfxShow::ALL.iter().map(|show| show.name()).collect();
                        format!("--vfx expects one of {}, got {raw:?}", names.join(", "))
                    })?);
                }
                "--no-hot-reload" => options.no_hot_reload = true,
                "--combat-input" => options.combat_input = true,
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
                "--bench-trace" => options.bench_trace = Some(PathBuf::from(value("--bench-trace")?)),
                OFFSCREEN_FLAG => serve = true,
                CONTACT_SHEET_FLAG => {
                    options.clip = Some(lookdev::ClipName::parse(&value(CONTACT_SHEET_FLAG)?));
                    sheet = Some(PathBuf::from(value(CONTACT_SHEET_FLAG)?));
                }
                "--sheet-cells" => {
                    let cells: u32 = number(&value("--sheet-cells")?, "--sheet-cells")?;
                    if !(1..=64).contains(&cells) {
                        return Err(format!("--sheet-cells expects 1 to 64, got {cells}"));
                    }
                    sheet_cells = Some(cells);
                }
                "--sheet-ticks" => sheet_ticks = number_list(&value("--sheet-ticks")?, "--sheet-ticks")?,
                "--sheet-views" => sheet_views = number_list(&value("--sheet-views")?, "--sheet-views")?,
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
                "--replay" => replay = Some(PathBuf::from(value("--replay")?)),
                "--record" => record = Some(PathBuf::from(value("--record")?)),
                "--scenario" => scenario = Some(value("--scenario")?),
                "--script" => script = Some(value("--script")?),
                "--watch-document" => {
                    let path = PathBuf::from(value("--watch-document")?);
                    if cfg!(not(feature = "dev")) {
                        return Err(
                            "--watch-document feeds hot reload, which only `dev` builds have (`--features dev`)"
                                .to_owned(),
                        );
                    }
                    options.watch_documents.push(path);
                }
                "--fight" => fight = true,
                _ => {
                    unknown.push(arg.clone());
                    continue;
                }
            }
            if let Some(value) = inline {
                return Err(format!("{name} takes no value, got {value:?}"));
            }
        }
        options.hud = match (hud_mode, hud_insets, hud_scale, hud_paused, hud_energy) {
            (None, None, None, false, None) => None,
            (None, ..) => {
                return Err("--hud-insets, --hud-scale, --hud-paused and --hud-energy need --hud".to_owned());
            }
            (Some(HudMode::Live), .., Some(_)) => {
                return Err("--hud-energy is the fixed frame's: a live HUD reads the fight's".to_owned());
            }
            (Some(mode), insets, scale, paused, energy) => {
                Some(HudOptions { mode, insets, scale: scale.unwrap_or(1.0), paused, energy })
            }
        };
        let companions = ui_locale.is_some()
            || ui_scale.is_some()
            || ui_pseudo
            || ui_plain
            || ui_time.is_some()
            || !ui_walk.is_empty()
            || ui_turn.is_some()
            || ui_spin.is_some();
        if ui_game.is_none() && companions {
            return Err(
                "--ui-locale, --ui-scale, --ui-pseudo, --ui-plain, --ui-time, --ui-walk, --ui-turn and --ui-spin need --ui-game"
                    .to_owned(),
            );
        }
        options.ui_screen = ui_game.map(|screen| {
            let shown = ScreenOptions::of(screen);
            ScreenOptions {
                screen,
                locale: ui_locale.unwrap_or(shown.locale),
                pseudo: ui_pseudo,
                scale: ui_scale.unwrap_or(shown.scale),
                plain: ui_plain,
                time: ui_time.unwrap_or(shown.time),
                walk: ui_walk,
                turn: ui_turn.unwrap_or(shown.turn),
                spin: ui_spin.unwrap_or(shown.spin),
            }
        });
        options.faces = match (faces, face_layout) {
            (true, layout) => Some(layout.unwrap_or_default()),
            (false, Some(_)) => return Err("--face-layout needs --faces".to_owned()),
            (false, None) => None,
        };
        if options.fade.is_some() && options.proximity_fade {
            return Err("--fade and --proximity-fade are exclusive: each sets the opacity".to_owned());
        }
        if options.two_sided && options.swatches {
            return Err(
                "--two-sided and --swatches are exclusive: both stand on the character's left".to_owned()
            );
        }
        if options.faces.is_some() && options.rim_rig.is_some() {
            return Err("--faces and --rim-rig are exclusive: each frames the camera on its rig".to_owned());
        }
        if (sheet_cells.is_some() || !sheet_ticks.is_empty() || !sheet_views.is_empty()) && sheet.is_none() {
            return Err(format!("--sheet-cells, --sheet-ticks and --sheet-views need {CONTACT_SHEET_FLAG}"));
        }
        if sheet.is_some() {
            if capture.is_some() || bench.is_some() || serve {
                return Err(format!(
                    "{CONTACT_SHEET_FLAG} is a mode of its own: drop {CAPTURE_FLAG}, {BENCH_FLAG} and {OFFSCREEN_FLAG}"
                ));
            }
            if options.look_dev.is_some() || options.view.is_some() {
                return Err(format!("{CONTACT_SHEET_FLAG} has its own cameras: drop --look-dev and --view"));
            }
            let range = crate::contact_sheet::ZOOM_RANGE;
            if let Some(zoom) = options.zoom.filter(|zoom| !range.contains(zoom)) {
                let (least, most) = range.into_inner();
                return Err(format!("{CONTACT_SHEET_FLAG}: --zoom expects {least} to {most}, got {zoom}"));
            }
            crate::contact_sheet::pin_defaults(&mut options);
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
        if cue_repeat.is_some() && cue.is_none() {
            return Err("--cue-repeat needs --cue".to_owned());
        }
        if cue.is_some() && (capture.is_some() || bench.is_some() || serve || sheet.is_some()) {
            return Err(format!(
                "--cue plays on the audio device, which {CAPTURE_FLAG}, {BENCH_FLAG}, {OFFSCREEN_FLAG} and {CONTACT_SHEET_FLAG} never open"
            ));
        }
        options.cue = cue.map(|id| CueOption { id, repeat: cue_repeat });
        if options.clip.is_some() && options.look_dev.is_none() && !arena_flag {
            return Err("--clip plays on the look-dev character: it needs --look-dev or --arena".to_owned());
        }
        if options.clip_in_place && options.clip.is_none() {
            return Err("--clip-in-place needs --clip".to_owned());
        }
        if options.model.is_some() && options.look_dev.is_none() {
            return Err("--model stands on the look-dev stage: it needs --look-dev".to_owned());
        }
        if options.graph.is_some() && options.look_dev.is_none() {
            return Err("--graph plays on the look-dev character: it needs --look-dev".to_owned());
        }
        if options.graph.is_some() && options.clip.is_some() {
            return Err("--graph and --clip are exclusive: the graph chooses what plays".to_owned());
        }
        options.arena = match (arena_flag, arena_shot) {
            (true, shot) => Some(shot.unwrap_or_default()),
            (false, Some(_)) => return Err("--arena-shot needs --arena".to_owned()),
            (false, None) => None,
        };
        if options.timeline.is_some() && options.arena != Some(ArenaShot::Director) {
            return Err(
                "--timeline's shots go to the director: it needs --arena, on the director's shot".to_owned()
            );
        }
        if options.arena.is_some()
            && (options.look_dev.is_some()
                || options.stage.is_some()
                || options.environment
                || options.foliage.is_some()
                || options.outline_row
                || options.faces.is_some()
                || options.rim_rig.is_some()
                || options.hard_edges)
        {
            return Err("--arena is a scene of its own: drop --look-dev, --stage, --environment, --probes, \
                 --foliage, --outline-row, --faces, --rim-rig and --hard-edges"
                .to_owned());
        }
        if options.party && options.arena.is_none() {
            return Err("--party needs --arena: it's who stands in the square".to_owned());
        }
        if options.blade.is_some() && !options.in_town() {
            return Err("--blade goes in the character's hand: it needs --look-dev or --arena".to_owned());
        }
        if options.springs.is_some()
            && options.clip.is_none()
            && options.graph.is_none()
            && options.arena.is_none()
        {
            return Err(
                "--springs runs under a clip or a graph: it needs --clip, --graph or --arena".to_owned()
            );
        }
        if ik && options.clip.is_none() && options.graph.is_none() {
            return Err("--ik solves over a clip or a graph: it needs --clip or --graph".to_owned());
        }
        if ik_ground.is_some() && options.arena.is_some() {
            return Err("--ik-ground is the look-dev stage's test bed: the arena has its own".to_owned());
        }
        if !ik && (ik_ground.is_some() || ik_look.is_some()) {
            return Err("--ik-ground and --ik-look need --ik on".to_owned());
        }
        options.ik = ik
            .then(|| sapphire_stage::ik::IkOptions { ground: ik_ground.unwrap_or_default(), look: ik_look });
        let frames = CaptureFrames { count: capture_frames.unwrap_or(1), step: capture_step.unwrap_or(0) };
        if usize::from(capture.is_some()) + usize::from(bench.is_some()) + usize::from(serve) > 1 {
            return Err(format!("{CAPTURE_FLAG}, {BENCH_FLAG} and {OFFSCREEN_FLAG} are exclusive"));
        }
        let mode = match (capture, bench) {
            _ if serve => Some(OffscreenMode::Serve),
            _ if sheet.is_some() => sheet.map(|path| OffscreenMode::ContactSheet {
                path,
                cells: sheet_cells.unwrap_or(crate::contact_sheet::DEFAULT_CELLS),
                ticks: std::mem::take(&mut sheet_ticks),
                yaws: std::mem::take(&mut sheet_views),
            }),
            (Some(path), _) => {
                let ticks = capture_ticks.unwrap_or(90);
                // A capture of a replay's end can't be checked until the replay is read.
                let at_replays_end = replay.is_some() && capture_ticks.is_none();
                if !at_replays_end && frames.first_tick(ticks).is_none() {
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
        if fight && options.arena.is_none() {
            return Err("--fight fights in the arena: it needs --arena".to_owned());
        }
        if fight && (replay.is_some() || record.is_some() || scenario.is_some()) {
            return Err("--fight is a fight of its own: drop --replay, --record and --scenario".to_owned());
        }
        options.combat = match (replay, record, scenario, script) {
            (None, None, None, script) if fight => Some(CombatOption::Fight { script, party: options.party }),
            (None, None, None, None) => None,
            (Some(path), None, None, None) => {
                Some(CombatOption::Replay { path, to_end: capture_ticks.is_none() })
            }
            (None, Some(path), Some(scenario), script) => {
                Some(CombatOption::Record { path, scenario, script })
            }
            (Some(_), Some(_), ..) => return Err("--replay and --record are exclusive".to_owned()),
            (None, Some(_), None, _) => return Err("--record needs --scenario".to_owned()),
            _ => return Err("--scenario and --script need --record".to_owned()),
        };
        // The arena has colliders to draw with no fight.
        let arena_colliders =
            options.arena.is_some() && options.debug_draw == Some(DebugDraw::Views(CombatViews::COLLIDERS));
        if matches!(options.debug_draw, Some(DebugDraw::Views(_)))
            && options.combat.is_none()
            && !arena_colliders
        {
            return Err("--debug-draw's views of a fight need --replay or --record".to_owned());
        }
        if options.party && options.combat.is_none() {
            return Err("--party needs --replay, --record or --fight: it draws a fight's party".to_owned());
        }
        if options.cues && options.combat.is_none() {
            return Err("--cues needs --replay or --record: it fires a fight's cues".to_owned());
        }
        if options.cues && options.vfx.is_some() {
            return Err("--cues and --vfx are exclusive: both play the frame's effects".to_owned());
        }
        if options.cues && options.cue.is_some() {
            return Err("--cues and --cue are exclusive: both play the frame's sound".to_owned());
        }
        if options.bench_trace.is_some()
            && !(matches!(mode, Some(OffscreenMode::Bench { .. }))
                && matches!(options.combat, Some(CombatOption::Replay { .. } | CombatOption::Fight { .. })))
        {
            return Err(format!(
                "--bench-trace needs {BENCH_FLAG} of a fight: --replay, or --fight with --script"
            ));
        }
        if options.zone_walk.is_some() && options.zone.is_none() {
            return Err("--zone-walk needs --zone".to_owned());
        }
        if options.zone.is_some() && options.in_town() {
            return Err("--zone is a stage of its own: not with --look-dev or --arena".to_owned());
        }
        if options.zone.is_some() && mode.is_none() {
            return Err("--zone draws offscreen: with --capture, --bench or --offscreen".to_owned());
        }
        let benching = matches!(mode, Some(OffscreenMode::Bench { .. }));
        let fight = matches!(options.combat, Some(CombatOption::Replay { .. } | CombatOption::Fight { .. }));
        let bench_of_a_load = benching && (fight || options.zone.is_some());
        if options.inventory_without.is_some() && !bench_of_a_load {
            return Err(format!(
                "--inventory-without needs {BENCH_FLAG} of a fight (--replay, or --fight with --script) or of a zone"
            ));
        }
        if options.draw_every.is_some() && !(capturing || serve || bench_of_a_load) {
            return Err(format!(
                "--draw-every needs {CAPTURE_FLAG}, {OFFSCREEN_FLAG}, or {BENCH_FLAG} of a fight (--replay, or \
                 --fight with --script) or of a zone"
            ));
        }
        let drawn_alone = matches!(mode, Some(OffscreenMode::Capture { .. } | OffscreenMode::Bench { .. }));
        match &options.combat {
            Some(CombatOption::Replay { .. }) if !drawn_alone => {
                return Err(format!("--replay needs {CAPTURE_FLAG} or {BENCH_FLAG}"));
            }
            Some(CombatOption::Record { script: Some(_), .. }) if mode.is_none() => {
                return Err(format!(
                    "--script needs {CAPTURE_FLAG}, {BENCH_FLAG} or {OFFSCREEN_FLAG}: the windowed app records \
                     who's playing"
                ));
            }
            Some(CombatOption::Record { script: None, .. }) if mode.is_some() => {
                return Err(
                    "--record needs --scenario and --script without a window: no one's there to play"
                        .to_owned(),
                );
            }
            Some(CombatOption::Fight { script: None, .. }) if mode.is_some() => {
                return Err("--fight needs --script without a window: no one's there to play".to_owned());
            }
            // The sandbox's own bindings would take the dodge and the heavy off a pad.
            // Someone's there to play it, or a script plays a phone's fight unattended (exit test 4), so
            // it's all on: the bars, the zones, the sparks and the sound. A recording in the arena is the
            // same fight, written down.
            Some(CombatOption::Fight { .. }) if mode.is_none() => options.play_in_a_window(),
            Some(CombatOption::Record { script: None, .. }) if options.arena.is_some() => {
                options.play_in_a_window();
            }
            _ => {}
        }
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
                let default_size = match mode {
                    OffscreenMode::ContactSheet { .. } => crate::contact_sheet::DEFAULT_CELL,
                    _ => (1920, 1080),
                };
                let (width, height) = size.unwrap_or(default_size);
                Some(Offscreen { mode, width, height })
            }
            None if size.is_some() => {
                return Err(format!("--size needs {CAPTURE_FLAG}, {BENCH_FLAG} or {OFFSCREEN_FLAG}"));
            }
            None => None,
        };
        Ok((options, unknown))
    }

    /// A `--record` path with no directory of its own goes under `dir`: a phone's cache.
    pub fn record_under(&mut self, dir: &std::path::Path) {
        if let Some(CombatOption::Record { path, .. }) = &mut self.combat
            && path.is_relative()
        {
            *path = dir.join(&*path);
        }
    }

    /// What a fight someone plays in a window takes without being asked: see where it's called.
    fn play_in_a_window(&mut self) {
        self.combat_input = true;
        self.cues = self.vfx.is_none() && self.cue.is_none();
        let live = HudOptions { mode: HudMode::Live, ..HudOptions::default() };
        self.hud.get_or_insert(live);
    }

    /// An error if a `--capture` or `--bench` run was given arguments it doesn't know. Scripts
    /// start these, so no argument of the OS's or SDL's is among them, and a misspelled pin would
    /// otherwise render another picture, which a golden bootstrapped from it would then hold
    /// forever (review DV-9). The windowed app and `--offscreen` only warn.
    pub fn refuse_unknown(&self, unknown: &[String]) -> Result<(), String> {
        let scripted = self.offscreen.as_ref().is_some_and(|offscreen| {
            matches!(
                offscreen.mode,
                OffscreenMode::Capture { .. }
                    | OffscreenMode::Bench { .. }
                    | OffscreenMode::ContactSheet { .. }
            )
        });
        if scripted && !unknown.is_empty() {
            return Err(format!(
                "{CAPTURE_FLAG} and {BENCH_FLAG} refuse arguments they don't know: {unknown:?}"
            ));
        }
        Ok(())
    }

    /// An error if the windowed app was given an offscreen mode, which only happens on Android:
    /// `main` picks the app from the process's arguments, and Android enters through `SDL_main`,
    /// which runs the windowed app with its arguments from the launching intent. It would draw
    /// until stopped, and never take the capture or the bench it was asked for.
    pub fn refuse_offscreen(&self) -> Result<(), String> {
        if self.offscreen.is_some() {
            return Err(format!(
                "{CAPTURE_FLAG}, {BENCH_FLAG} and {OFFSCREEN_FLAG} don't run on Android; \
                 a device's capture is `cargo xtask link capture`"
            ));
        }
        Ok(())
    }
}

fn number<T: std::str::FromStr>(raw: &str, name: &str) -> Result<T, String> {
    raw.parse().map_err(|_| format!("{name} expects a number, got {raw:?}"))
}

/// A list of numbers, comma-separated: 64 at most, as a sheet's cells are.
fn number_list<T: std::str::FromStr>(raw: &str, name: &str) -> Result<Vec<T>, String> {
    let values: Vec<T> = raw.split(',').map(|value| number(value.trim(), name)).collect::<Result<_, _>>()?;
    if values.len() > 64 {
        return Err(format!("{name} expects 64 values at most, got {}", values.len()));
    }
    Ok(values)
}

/// `<left>,<top>,<right>,<bottom>` in points, none negative.
fn parse_insets(raw: &str) -> Result<Insets, String> {
    let invalid = || format!("--hud-insets expects LEFT,TOP,RIGHT,BOTTOM, none negative, got {raw:?}");
    let values: Vec<f32> =
        raw.split(',').map(|value| value.trim().parse().map_err(|_| invalid())).collect::<Result<_, _>>()?;
    let [left, top, right, bottom] = values[..] else { return Err(invalid()) };
    if ![left, top, right, bottom].iter().all(|value| value.is_finite() && *value >= 0.0) {
        return Err(invalid());
    }
    Ok(Insets { left, top, right, bottom })
}

/// `<r>,<g>,<b>,<share>`: a hue that isn't black and a share within 0 to 1.
fn parse_shade_hue(raw: &str) -> Result<CharacterShadeKey, String> {
    let invalid = || {
        format!(
            "--shade-hue expects R,G,B,SHARE, a hue that isn't black and a share within 0..=1, got {raw:?}"
        )
    };
    let values: Vec<f32> =
        raw.split(',').map(|value| value.trim().parse().map_err(|_| invalid())).collect::<Result<_, _>>()?;
    let [r, g, b, share] = values[..] else { return Err(invalid()) };
    let hue = [r, g, b];
    let color = hue.iter().all(|c| c.is_finite() && *c >= 0.0) && hue.iter().any(|c| *c > 0.0);
    if !color || !(0.0..=1.0).contains(&share) {
        return Err(invalid());
    }
    Ok(CharacterShadeKey { hue, share })
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

    /// What live tuning follows comes from the flags: a fight needs the arena as well as a simulation, and
    /// `--no-hot-reload` reads no saved document.
    #[cfg(feature = "dev")]
    #[test]
    #[ignore = "runs with CI's dev tests, which take --ignored"]
    fn a_run_tells_live_tuning_what_it_holds() {
        let run = |args: &[&str]| {
            let (options, _) = parse(args).expect("good arguments");
            let run = options.tuned_run();
            (run.arena_fight, run.graph_show, run.cues, run.camera_director, run.reads_files)
        };
        assert_eq!(run(&["--arena", "--fight"]), (true, false, true, true, true));
        assert_eq!(run(&["--look-dev", "hero", "--graph", "walk"]), (false, true, false, false, true));
        assert_eq!(run(&["--look-dev", "hero"]), (false, false, false, false, true));
        assert_eq!(run(&["--arena"]), (false, false, false, true, true));
        assert_eq!(run(&["--no-hot-reload"]), (false, false, false, false, false));
        let cues = ["--offscreen", "--record", "a", "--scenario", "duel", "--script", "attempts", "--cues"];
        assert_eq!(run(&cues), (false, false, true, false, true));
    }

    /// Spec §5.15: a capture or a bench can't be asked to open an audio device.
    #[test]
    fn no_offscreen_run_takes_a_cue() {
        for mode in [&["--capture", "x.png"][..], &["--bench", "4"], &["--offscreen"]] {
            let args: Vec<&str> = mode.iter().copied().chain(["--cue", "hit"]).collect();
            let error = parse(&args).unwrap_err();
            assert!(error.contains("--cue") && error.contains(mode[0]), "{error}");
        }
    }

    #[test]
    fn a_cue_is_asked_for_by_name() {
        let (options, unknown) = parse(&["--cue", "swing", "--cue-repeat", "1.5"]).unwrap();
        assert!(unknown.is_empty());
        let id = sapphire_stage::audio::named_cue("swing").unwrap();
        assert_eq!(options.cue, Some(CueOption { id, repeat: Some(1.5) }));
        assert_eq!(parse(&["--cue", "hit"]).unwrap().0.cue.and_then(|cue| cue.repeat), None);
        assert_eq!(parse(&[]).unwrap().0.cue, None, "no sound unless asked");

        assert!(parse(&["--cue", "nope"]).unwrap_err().contains("click, hit, swing"));
        assert!(parse(&["--cue-repeat", "1"]).unwrap_err().contains("needs --cue"));
        for bad in ["0", "-1", "inf", "NaN"] {
            let error = parse(&["--cue", "hit", "--cue-repeat", bad]).unwrap_err();
            assert!(error.contains("--cue-repeat"), "{error}");
        }
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
    fn a_graph_plays_on_the_look_dev_character_alone() {
        assert_eq!(parse(&["--look-dev", "hero"]).unwrap().0.graph, None);
        let (options, _) = parse(&["--look-dev", "hero", "--graph", "cancel"]).unwrap();
        assert_eq!(options.graph, Some(GraphScript::Cancel));
        let error = parse(&["--graph", "cancel"]).unwrap_err();
        assert!(error.contains("--look-dev"), "{error}");
        let error = parse(&["--look-dev", "hero", "--graph", "dance"]).unwrap_err();
        assert!(error.contains("cancel") && error.contains("dance"), "{error}");
        let error = parse(&["--look-dev", "hero", "--graph", "run", "--clip", "strike"]).unwrap_err();
        assert!(error.contains("exclusive"), "{error}");
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
        assert_eq!(staged.stage, Some(StudioLayout::Pillar));
        assert!(parse(&["--stage", "arena"]).is_err());
        assert_eq!(Options::default().quality_settings(), QualitySettings::default());
        assert_eq!(parse(&["--tier=Desktop-High"]).unwrap().0.tier, Some(TierId::DesktopHigh));
        assert!(parse(&["--tier", "ultra"]).unwrap_err().contains("desktop-high"));
        assert!(parse(&["--render-scale", "0.2"]).is_err(), "below the minimum");
        assert!(parse(&["--render-scale", "1.5"]).is_err());
        assert!(parse(&["--render-scale", "NaN"]).is_err());
    }

    #[test]
    fn a_contact_sheet_names_its_clip_and_pins_what_it_implies() {
        let (options, _) = parse(&["--contact-sheet", "Hit_A", "sheet.png"]).unwrap();
        assert_eq!(options.clip, Some(lookdev::ClipName::parse("Hit_A")));
        assert_eq!(options.look_dev, Some(Shot::Hero));
        assert_eq!(options.aa, Some(Antialiasing::Msaa), "a cell has no history for temporal AA");
        assert!(options.fog_off);
        assert_eq!(options.time_of_day, Some(12.0));
        let offscreen = options.offscreen.expect("a mode of its own");
        assert_eq!((offscreen.width, offscreen.height), crate::contact_sheet::DEFAULT_CELL);
        let OffscreenMode::ContactSheet { path, cells, .. } = offscreen.mode else { panic!("a sheet") };
        assert_eq!((path, cells), (PathBuf::from("sheet.png"), crate::contact_sheet::DEFAULT_CELLS));

        // A camera past any use refuses, and says what it takes.
        let sheet_at =
            |zoom: &str| parse(&["--contact-sheet", "idle", "sheet.png", "--zoom", zoom]).map(drop);
        assert!(sheet_at("1.6").is_ok() && sheet_at("0.01").is_ok() && sheet_at("1000").is_ok());
        for zoom in ["1e30", "1000.5", "0.009"] {
            let refused = sheet_at(zoom).expect_err("past any use");
            assert!(refused.contains("--zoom expects 0.01 to 1000"), "{refused}");
        }
        // Only a sheet's: an orbit camera's zoom has its own limits.
        assert!(parse(&["--zoom", "1e30"]).is_ok());

        // Told otherwise, it does as it's told.
        let (options, _) = parse(&[
            "--contact-sheet",
            "idle",
            "sheet.png",
            "--sheet-cells",
            "3",
            "--size",
            "400x500",
            "--time-of-day",
            "18",
            "--aa",
            "taa",
        ])
        .unwrap();
        assert_eq!((options.time_of_day, options.aa), (Some(18.0), Some(Antialiasing::Taa)));
        let offscreen = options.offscreen.expect("a mode of its own");
        assert_eq!((offscreen.width, offscreen.height), (400, 500));
        assert!(matches!(offscreen.mode, OffscreenMode::ContactSheet { cells: 3, .. }));

        // Ticks and views by name, for reading one part of a move.
        let (options, _) = parse(&[
            "--contact-sheet",
            "idle",
            "sheet.png",
            "--sheet-ticks",
            "0, 7,9",
            "--sheet-views",
            "205,90",
        ])
        .unwrap();
        let mode = options.offscreen.expect("a mode of its own").mode;
        let OffscreenMode::ContactSheet { ticks, yaws, .. } = mode else { panic!("a sheet") };
        assert_eq!((ticks, yaws), (vec![0, 7, 9], vec![205, 90]));
        assert!(parse(&["--sheet-ticks", "3"]).unwrap_err().contains("--contact-sheet"));
        assert!(parse(&["--sheet-views", "3"]).unwrap_err().contains("--contact-sheet"));

        // Any clip by its ID. A name isn't looked for until the packs are mounted (`ClipName::resolve`).
        let id = "15af86bb-67b3-4208-bc54-1618aabf450b";
        assert!(parse(&["--contact-sheet", id, "sheet.png"]).is_ok());

        let refused = |args: &[&str], says: &str| {
            let error = parse(args).unwrap_err();
            assert!(error.contains(says), "{args:?}: {error}");
        };
        refused(&["--contact-sheet", "Hit_A"], "--contact-sheet");
        refused(&["--contact-sheet", "Hit_A", "a.png", "--capture", "b.png"], "--capture");
        refused(&["--contact-sheet", "Hit_A", "a.png", "--look-dev", "hero"], "--look-dev");
        refused(&["--contact-sheet", "Hit_A", "a.png", "--sheet-cells", "0"], "--sheet-cells");
        refused(&["--sheet-cells", "4"], "--contact-sheet");

        let (options, unknown) = parse(&["--contact-sheet", "Hit_A", "a.png", "--zoon", "2"]).unwrap();
        assert!(options.refuse_unknown(&unknown).is_err(), "a script's typo draws no other picture");
        assert!(is_offscreen_flag("--contact-sheet"), "never opens the window");
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
        assert_eq!(parse(&["--look-dev", "face"]).unwrap().0.look_dev, Some(Shot::Face));
        assert_eq!(parse(&["--look-dev", "enemy"]).unwrap().0.look_dev, Some(Shot::Enemy));
        assert!(parse(&["--look-dev", "close"]).unwrap_err().contains("wide, hero, sky"));
        assert_eq!(parse(&["--look-dev", "hero"]).unwrap().0.clip, None);
        assert_eq!(
            parse(&["--look-dev", "hero", "--clip", "strike"]).unwrap().0.clip,
            Some(lookdev::ClipName::parse("strike"))
        );
        assert!(parse(&["--clip", "strike"]).unwrap_err().contains("--look-dev"));
        assert_eq!(
            parse(&["--arena", "--clip", "idle"]).unwrap().0.clip,
            Some(lookdev::ClipName::parse("idle"))
        );
        let solving = ["--look-dev", "hero", "--clip", "strike", "--ik", "on"];
        assert_eq!(parse(&solving).unwrap().0.ik, Some(sapphire_stage::ik::IkOptions::default()));
        assert_eq!(parse(&["--look-dev", "hero", "--clip", "strike", "--ik", "off"]).unwrap().0.ik, None);
        assert_eq!(parse(&["--look-dev", "hero", "--clip", "strike"]).unwrap().0.ik, None, "off by default");
        let on_a_slope = [
            "--look-dev",
            "hero",
            "--clip",
            "strike",
            "--ik",
            "on",
            "--ik-ground",
            "slope",
            "--ik-look",
            "1,1.5,2",
        ];
        assert_eq!(
            parse(&on_a_slope).unwrap().0.ik,
            Some(sapphire_stage::ik::IkOptions {
                ground: sapphire_stage::ik::IkGround::Slope,
                look: Some(sapphire_stage::ik::IkLook::Point(sapphire_math::Vec3::new(1.0, 1.5, 2.0))),
            })
        );
        assert!(parse(&["--look-dev", "hero", "--ik", "on"]).unwrap_err().contains("--clip"));
        // Over a graph, as over a clip.
        let on_a_graph = ["--look-dev", "hero", "--graph", "walk", "--ik", "on"];
        assert_eq!(parse(&on_a_graph).unwrap().0.ik, Some(sapphire_stage::ik::IkOptions::default()));
        // On the arena's own colliders, which have no test bed to choose.
        let in_the_arena = ["--arena", "--clip", "idle", "--ik", "on"];
        assert_eq!(parse(&in_the_arena).unwrap().0.ik, Some(sapphire_stage::ik::IkOptions::default()));
        assert!(
            parse(&["--arena", "--clip", "idle", "--ik", "on", "--ik-ground", "step"])
                .unwrap_err()
                .contains("the arena has its own")
        );
        assert!(parse(&["--arena", "--ik", "on"]).unwrap_err().contains("--clip"));
        assert!(parse(&["--look-dev", "hero", "--clip", "strike", "--ik", "maybe"]).is_err());
        assert!(
            parse(&["--look-dev", "hero", "--clip", "strike", "--ik-ground", "step"])
                .unwrap_err()
                .contains("--ik on")
        );
        assert!(
            parse(&["--look-dev", "hero", "--clip", "strike", "--ik", "on", "--ik-ground", "hill"]).is_err()
        );
        assert!(
            parse(&["--look-dev", "hero", "--clip", "strike", "--ik", "on", "--ik-look", "1,2"]).is_err()
        );
        let orbit = ["--look-dev", "hero", "--clip", "strike", "--ik", "on", "--ik-look", "orbit"];
        assert_eq!(
            parse(&orbit).unwrap().0.ik.and_then(|ik| ik.look),
            Some(sapphire_stage::ik::IkLook::Orbit)
        );
        let in_place = ["--look-dev", "hero", "--clip", "strike", "--clip-in-place"];
        assert!(parse(&in_place).unwrap().0.clip_in_place);
        assert!(parse(&["--look-dev", "hero", "--clip-in-place"]).unwrap_err().contains("--clip"));
        // A model, by asset IDs, on the look-dev stage only.
        let model = "41e229ad-28a3-41f9-aeb8-ba214e06be6a:0853fc78-9a5d-4f9d-bcfe-1c8a485a0db4@12";
        let parsed = parse(&["--look-dev", "enemy", "--model", model]).unwrap().0.model.unwrap();
        assert_eq!((parsed.clip.is_some(), parsed.tick), (true, 12));
        assert!(parse(&["--model", model]).unwrap_err().contains("--look-dev"));
        assert!(parse(&["--look-dev", "enemy", "--model", "lancer"]).unwrap_err().contains("--model"));
        // The sword: where a scene asks, and on the shot made for reading moves.
        assert!(!parse(&["--look-dev", "hero"]).unwrap().0.blade());
        assert!(parse(&["--look-dev", "body"]).unwrap().0.blade());
        assert!(!parse(&["--look-dev", "body", "--blade", "off"]).unwrap().0.blade());
        assert!(parse(&["--look-dev", "hero", "--blade", "on"]).unwrap().0.blade());
        assert!(!parse(&["--arena"]).unwrap().0.blade());
        assert!(parse(&["--arena", "--fight"]).unwrap().0.blade(), "whoever fights has one");
        assert!(!parse(&["--arena", "--fight", "--blade", "off"]).unwrap().0.blade());
        assert!(parse(&["--arena", "--blade", "on"]).unwrap().0.blade());
        assert!(parse(&["--blade", "on"]).unwrap_err().contains("--look-dev"));
        assert!(parse(&["--look-dev", "hero", "--blade", "maybe"]).unwrap_err().contains("on or off"));
        let springs = ["--look-dev", "hero", "--clip", "strike", "--springs", "off"];
        assert_eq!(parse(&springs).unwrap().0.springs, Some(false));
        assert_eq!(parse(&["--look-dev", "hero", "--clip", "strike"]).unwrap().0.springs, None);
        assert!(parse(&["--look-dev", "hero", "--springs", "on"]).unwrap_err().contains("--clip"));
        let wrong = ["--look-dev", "hero", "--clip", "strike", "--springs", "maybe"];
        assert!(parse(&wrong).unwrap_err().contains("on or off"));
        assert!(parse(&["--look-dev", "sky", "--stage", "floor"]).is_err());
        assert!(parse(&["--environment", "--look-dev", "sky"]).is_err());
        assert!(parse(&["--look-dev", "wide", "--outline-row"]).is_err());
        assert!(parse(&["--faces", "--look-dev", "hero"]).is_err());
        assert!(parse(&["--look-dev", "hero", "--rim-rig", "angles"]).is_err());
        assert!(parse(&["--look-dev", "wide", "--hard-edges"]).is_err());
        let foliage = parse(&["--look-dev", "wide", "--foliage", "grove"]).unwrap_err();
        assert!(foliage.contains("--foliage"), "{foliage}");
        let defaults = Options::default();
        assert_eq!(defaults.character(), sapphire_stage::content::CHARACTER);
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
    fn the_zone_stage_names_a_zone_by_its_stem_and_draws_offscreen() {
        let bench = ["--bench", "1"];
        let (options, _) = parse(&[&bench[..], &["--zone", "ward"]].concat()).unwrap();
        assert_eq!(options.zone, Some(("ward".to_owned(), "player_start".to_owned())));
        assert!(options.look_dev_character() && options.keeps_character_mesh() && !options.in_town());
        assert_eq!(options.character(), lookdev::CHARACTER);
        let (options, _) =
            parse(&[&bench[..], &["--zone", "ward:boss", "--zone-walk", "tour:2.5"]].concat()).unwrap();
        assert_eq!(options.zone, Some(("ward".to_owned(), "boss".to_owned())));
        assert_eq!(options.zone_walk, Some(("tour".to_owned(), 2.5)));
        assert!(parse(&[&bench[..], &["--zone-walk", "tour"]].concat()).is_err(), "no zone to walk");
        assert!(parse(&[&bench[..], &["--zone", "ward", "--arena"]].concat()).is_err(), "a stage of its own");
        assert!(parse(&["--zone", "ward"]).is_err(), "it has no window");
        let without = ["--zone", "ward", "--inventory-without", "ambient"];
        assert!(parse(&[&bench[..], &without[..]].concat()).is_ok());
        assert!(parse(&[&["--capture", "x.png"][..], &without[..]].concat()).is_err(), "only a bench says");
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
    fn material_and_fade_options_place_the_samples_and_fade_the_characters() {
        let (options, unknown) = parse(&["--materials", "--fade", "0.25"]).unwrap();
        assert!(unknown.is_empty());
        assert!(options.materials && !Options::default().materials);
        assert_eq!(options.fade(), Fade::Fixed(0.25));
        assert_eq!(parse(&["--proximity-fade"]).unwrap().0.fade(), Fade::Proximity(Default::default()));
        assert_eq!(Options::default().fade(), Fade::Off);
        assert!(parse(&["--fade", "1.5"]).is_err());
        assert!(parse(&["--fade", "NaN"]).is_err());
        assert!(parse(&["--fade", "0.5", "--proximity-fade"]).is_err(), "exclusive");
    }

    #[test]
    fn two_sided_places_the_garments_on_the_swatches_side() {
        let (options, unknown) = parse(&["--two-sided", "--materials"]).unwrap();
        assert!(unknown.is_empty());
        assert!(options.two_sided && !Options::default().two_sided);
        assert!(parse(&["--two-sided", "--swatches"]).is_err(), "both stand on the character's left");
    }

    #[test]
    fn hair_overrides_parse_and_reach_the_materials() {
        let options = parse(&["--hair-shadow-softness", "0.02", "--hair-transmission", "0.8"]).unwrap().0;
        assert_eq!((options.hair_shadow_softness, options.hair_transmission), (Some(0.02), Some(0.8)));
        let overrides = MaterialOverrides { hair_shadow_softness: Some(0.02), hair_transmission: Some(0.8) };
        assert_eq!(options.material_overrides(), overrides);
        assert!(Options::default().material_overrides().is_empty());
        for flag in ["--hair-shadow-softness", "--hair-transmission"] {
            for wrong in ["-0.01", "NaN", "inf", "soft"] {
                assert!(parse(&[flag, wrong]).is_err(), "{flag} {wrong}");
            }
        }
    }

    #[test]
    fn camera_rim_parses_a_share() {
        assert_eq!(parse(&["--camera-rim", "0.75"]).unwrap().0.camera_rim, Some(0.75));
        assert_eq!(Options::default().camera_rim, None);
        for wrong in ["1.5", "-0.1", "NaN", "behind"] {
            assert!(parse(&["--camera-rim", wrong]).is_err(), "{wrong}");
        }
    }

    #[test]
    fn the_screen_s_companions_need_it() {
        assert_eq!(Options::default().ui_screen, None);
        let (options, _) = parse(&["--ui-game", "title"]).unwrap();
        assert_eq!(options.ui_screen, Some(ScreenOptions::of(MenuScreen::Title)));
        let (options, _) = parse(&[
            "--ui-game",
            "pause",
            "--ui-locale",
            "ja",
            "--ui-scale",
            "2",
            "--ui-pseudo",
            "--ui-plain",
        ])
        .unwrap();
        let shown = ScreenOptions {
            locale: "ja".to_owned(),
            pseudo: true,
            scale: 2.0,
            plain: true,
            ..ScreenOptions::of(MenuScreen::Pause)
        };
        assert_eq!(options.ui_screen, Some(shown));
        let (options, _) =
            parse(&["--ui-game", "settings", "--ui-time", "0.25", "--ui-walk", "down,confirm"]).unwrap();
        let shown = ScreenOptions {
            time: 0.25,
            walk: vec!["down".to_owned(), "confirm".to_owned()],
            ..ScreenOptions::of(MenuScreen::Settings)
        };
        assert_eq!(options.ui_screen, Some(shown));
        let refused = parse(&["--ui-game", "shop"]).map(drop).unwrap_err();
        assert!(refused.contains("shop") && refused.contains("settings"), "{refused}");
        let (options, _) = parse(&["--ui-game", "character", "--ui-turn", "0.25"]).unwrap();
        let shown = ScreenOptions { turn: 0.25, ..ScreenOptions::of(MenuScreen::Character) };
        assert_eq!(options.ui_screen, Some(shown));
        for args in [
            &["--ui-locale", "ja"][..],
            &["--ui-turn", "0.5"],
            &["--ui-game", "character", "--ui-turn", "inf"],
            &["--ui-scale", "2"],
            &["--ui-pseudo"],
            &["--ui-plain"],
            &["--ui-time", "1"],
            &["--ui-walk", "down"],
            &["--ui-game", "title", "--ui-time", "-1"],
            &["--ui-game", "title", "--ui-scale", "0"],
            &["--ui-game", "title", "--ui-scale", "many"],
        ] {
            assert!(parse(args).is_err(), "{args:?}");
        }
    }

    #[test]
    fn the_hud_takes_a_mode_and_its_companions_need_it() {
        assert_eq!(Options::default().hud, None);
        let (options, _) = parse(&["--hud", "fixed"]).unwrap();
        assert_eq!(options.hud, Some(HudOptions::default()));
        let (options, _) = parse(&["--hud-scale", "3", "--hud=live", "--hud-insets", "62, 0,62,21"]).unwrap();
        let insets = Insets { left: 62.0, top: 0.0, right: 62.0, bottom: 21.0 };
        let live =
            HudOptions { mode: HudMode::Live, insets: Some(insets), scale: 3.0, paused: false, energy: None };
        assert_eq!(options.hud, Some(live));
        let (options, _) = parse(&["--hud", "live", "--hud-paused"]).unwrap();
        assert!(options.hud.is_some_and(|hud| hud.paused));
        let (options, _) = parse(&["--hud", "fixed", "--hud-energy", "0.5"]).unwrap();
        assert_eq!(options.hud.and_then(|hud| hud.energy), Some(0.5));
        for wrong in [
            &["--hud", "maybe"][..],
            &["--hud-insets", "1,2,3,4"],
            &["--hud-scale", "2"],
            &["--hud-paused"],
            &["--hud-energy", "1"],
            &["--hud", "live", "--hud-energy", "1"],
            &["--hud", "fixed", "--hud-energy", "1.5"],
            &["--hud", "fixed", "--hud-energy", "full"],
            &["--hud", "fixed", "--hud-insets", "1,2,3"],
            &["--hud", "fixed", "--hud-insets", "1,2,3,-4"],
            &["--hud", "fixed", "--hud-scale", "0"],
        ] {
            assert!(parse(wrong).is_err(), "{wrong:?}");
        }
    }

    #[test]
    fn shade_hue_parses_a_hue_and_its_share() {
        let (options, unknown) = parse(&["--shade-hue", "0.5, 0.6,1,0.7"]).unwrap();
        assert!(unknown.is_empty());
        assert_eq!(options.shade_hue, Some(CharacterShadeKey { hue: [0.5, 0.6, 1.0], share: 0.7 }));
        assert_eq!(Options::default().shade_hue, None);
        for wrong in ["0.5,0.6,1", "0,0,0,0.5", "1,1,1,1.5", "1,-1,1,0.5", "1,1,NaN,0.5", "1,1,1,0.5,1"] {
            assert!(parse(&["--shade-hue", wrong]).is_err(), "{wrong}");
        }
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
        // No frame draws a layer, so post builds none of their variants.
        let tier = RenderSettings::default();
        assert_eq!(options.render_settings(tier).post_variants, PostVariants::NONE);
        assert_eq!(Options::default().render_settings(tier).post_variants, PostVariants::COMPOSITING);
    }

    #[test]
    fn view_culling_turns_off_for_comparisons() {
        let tier = RenderSettings::default();
        assert!(Options::default().render_settings(tier).view_culling, "on by default");
        let culling = |args: &[&str]| parse(args).unwrap().0.render_settings(tier).view_culling;
        assert!(!culling(&["--view-culling", "off"]));
        assert!(culling(&["--view-culling=on"]));
        assert!(parse(&["--view-culling", "some"]).is_err());
    }

    /// A phone's app runs from nowhere it can write: a path of its own stands, and any other goes under
    /// the cache, which is what can be fetched from a phone.
    #[test]
    fn a_recording_on_a_phone_goes_under_its_cache() {
        let recorded = |path: &str| {
            let (mut options, _) = parse(&["--arena", "--record", path, "--scenario", "s"]).unwrap();
            options.record_under(std::path::Path::new("/cache"));
            match options.combat {
                Some(CombatOption::Record { path, .. }) => path,
                _ => panic!("a recording"),
            }
        };
        assert_eq!(recorded("touch.sreplay"), PathBuf::from("/cache/touch.sreplay"));
        assert_eq!(recorded("/data/touch.sreplay"), PathBuf::from("/data/touch.sreplay"));
    }

    /// A recording in the arena is the same fight as `--fight`, written down: a phone has no other way to
    /// keep a session, and no keyboard to pass flags from. Elsewhere it stays as it was.
    #[test]
    fn a_recording_in_the_arena_is_a_fight_someone_plays() {
        let args = ["--arena", "--record", "a.sreplay", "--scenario", "arena-fight"];
        let (played, unknown) = parse(&args).unwrap();
        assert!(unknown.is_empty(), "{unknown:?}");
        assert!(played.combat_input && played.cues && played.blade());
        assert_eq!(played.hud.map(|hud| hud.mode), Some(HudMode::Live));
        played.refuse_offscreen().expect("Android's windowed app can record too");
        assert!(!parse(&[&args[..], &["--blade", "off"]].concat()).unwrap().0.blade());

        let (plain, _) = parse(&["--record", "a.sreplay", "--scenario", "melee"]).unwrap();
        assert!(!plain.combat_input && !plain.cues && !plain.blade());
        assert_eq!(plain.hud, None);

        // A script needs no one, and draws what it did.
        let scripted = ["--capture", "x", "--arena", "--record", "a", "--scenario", "s", "--script", "p"];
        let (scripted, _) = parse(&scripted).unwrap();
        assert!(!scripted.combat_input && !scripted.cues && !scripted.blade());
        assert_eq!(scripted.hud, None);
    }

    /// A fight has no file: in a window whoever's there plays it on combat's bindings alone, anywhere else
    /// a script does, down the same path, and a script can stand in for them in a window too, which is how
    /// a phone fights for 15 minutes with no one holding it.
    #[test]
    fn a_fight_is_played_in_a_window_or_scripted_anywhere_else() {
        let (windowed, unknown) = parse(&["--arena", "--fight"]).unwrap();
        assert!(unknown.is_empty());
        assert_eq!(windowed.combat, Some(CombatOption::Fight { script: None, party: false }));
        assert!(windowed.cues, "whoever's there gets the sparks and the sound");
        assert_eq!(windowed.hud.map(|hud| hud.mode), Some(HudMode::Live), "and the bars");
        assert!(windowed.offscreen.is_none());
        assert!(windowed.combat_input, "the sandbox's bindings would take the dodge off a pad");
        windowed.refuse_offscreen().expect("Android's windowed app can fight too");

        let scripted = Some(CombatOption::Fight { script: Some("idle".to_owned()), party: false });
        for offscreen in [&["--offscreen"][..], &["--capture", "x"], &["--bench", "10"]] {
            let args = [offscreen, &["--arena", "--fight", "--script", "idle"]].concat();
            let (options, _) = parse(&args).unwrap();
            assert_eq!(options.combat, scripted, "{offscreen:?}");
            assert!(!options.combat_input, "{offscreen:?}");
        }

        let refused = |args: &[&str], part: &str| {
            let error = parse(args).expect_err("refused");
            assert!(error.contains(part), "{args:?}: {error}");
        };
        let (unattended, unknown) = parse(&["--arena", "--fight", "--script", "fight-back"]).unwrap();
        assert!(unknown.is_empty());
        assert_eq!(
            unattended.combat,
            Some(CombatOption::Fight { script: Some("fight-back".to_owned()), party: false })
        );
        assert!(unattended.offscreen.is_none());
        assert!(unattended.cues, "the whole load");
        assert_eq!(unattended.hud.map(|hud| hud.mode), Some(HudMode::Live));
        unattended.refuse_offscreen().expect("Android's windowed app runs it");

        refused(&["--fight"], "--arena");
        refused(&["--arena", "--fight", "--capture", "x"], "no one's there");
        refused(&["--arena", "--fight", "--offscreen"], "no one's there");
        refused(&["--arena", "--fight", "--capture", "x", "--replay", "a"], "--replay");
        refused(&["--arena", "--fight", "--record", "a", "--scenario", "s"], "--record");
        refused(&["--arena", "--fight", "--scenario", "s"], "--scenario");
    }

    #[test]
    fn the_arena_takes_a_shot_and_the_town_s_character_and_sky() {
        assert_eq!(Options::default().arena, None);
        let (options, unknown) = parse(&["--arena"]).unwrap();
        assert!(unknown.is_empty());
        assert_eq!(options.arena, Some(ArenaShot::Director));
        // A fight in a window takes a recording, played by the bindings.
        let (live, unknown) =
            parse(&["--arena", "--record", "fight.sreplay", "--scenario", "arena"]).unwrap();
        assert!(unknown.is_empty());
        assert!(live.combat.is_some() && live.arena.is_some());
        assert_eq!(options.look_dev, None);
        assert_eq!(options.character(), lookdev::CHARACTER);
        assert_eq!(options.sky(), lookdev::SKY);
        assert!(options.keeps_character_mesh());
        assert_eq!(parse(&["--arena", "--arena-shot", "over"]).unwrap().0.arena, Some(ArenaShot::Over));
        assert_eq!(parse(&["--arena-shot=follow", "--arena"]).unwrap().0.arena, Some(ArenaShot::Follow));
    }

    #[test]
    fn the_arena_refuses_what_the_look_dev_stage_refuses() {
        for excluded in [
            &["--look-dev", "hero"][..],
            &["--stage", "floor"],
            &["--environment"],
            &["--probes"],
            &["--foliage", "grove"],
            &["--outline-row"],
            &["--faces"],
            &["--rim-rig", "angles"],
            &["--hard-edges"],
        ] {
            let args: Vec<&str> = ["--arena"].iter().chain(excluded).copied().collect();
            let error = parse(&args).expect_err(&format!("{args:?} isn't a scene"));
            assert!(error.contains("--arena is a scene of its own"), "{args:?}: {error}");
            assert!(error.contains(excluded[0]), "{args:?}: {error}");
        }
    }

    #[test]
    fn a_party_needs_the_arena_and_a_fight() {
        assert!(!Options::default().party);
        let fight = ["--record", "fight.sreplay", "--scenario", "party"];
        let (options, _) = parse(&["--arena", "--party", fight[0], fight[1], fight[2], fight[3]]).unwrap();
        assert!(options.party);
        let error = parse(&["--party", fight[0], fight[1], fight[2], fight[3]]).expect_err("no arena");
        assert!(error.contains("needs --arena"), "{error}");
        let error = parse(&["--arena", "--party"]).expect_err("no fight");
        assert!(error.contains("needs --replay, --record or --fight"), "{error}");
        let (fighting, _) = parse(&["--arena", "--fight", "--party"]).unwrap();
        assert_eq!(fighting.combat, Some(CombatOption::Fight { script: None, party: true }));
    }

    #[test]
    fn the_arena_s_other_flags_need_it() {
        const INTRO: &str = "--timeline=8f1c2a6e-5b7d-4e39-9a10-2c64d7e0b311";
        assert!(parse(&["--arena", INTRO]).unwrap().0.timeline.is_some());
        assert!(parse(&[INTRO]).unwrap_err().contains("--arena"));
        assert!(parse(&["--arena", "--arena-shot", "follow", INTRO]).unwrap_err().contains("director"));
        assert!(parse(&["--arena", "--timeline", "soon"]).unwrap_err().contains("UUID"));
        let error = parse(&["--arena-shot", "over"]).expect_err("no arena");
        assert!(error.contains("needs --arena"), "{error}");
        let error = parse(&["--arena", "--arena-shot", "near"]).expect_err("no such shot");
        assert!(error.contains("follow") && error.contains("over"), "{error}");
    }

    /// Only the arena's colliders draw with no fight: elsewhere there are none, and a volume is a fight's.
    #[test]
    fn the_arena_draws_its_colliders_with_no_fight() {
        assert_eq!(
            parse(&["--arena", "--debug-draw", "colliders"]).unwrap().0.debug_draw,
            Some(DebugDraw::Views(CombatViews::COLLIDERS)),
        );
        let refused: [&[&str]; 3] = [
            &["--debug-draw", "colliders"],
            &["--arena", "--debug-draw", "volumes"],
            &["--arena", "--debug-draw", "combat"],
        ];
        for args in refused {
            let error = parse(args).expect_err("no fight");
            assert!(error.contains("--replay or --record"), "{args:?}: {error}");
        }
    }

    #[test]
    fn debug_draw_is_named_on_the_command_line() {
        assert_eq!(Options::default().debug_draw, None);
        let (options, unknown) = parse(&["--debug-draw", "shapes"]).unwrap();
        assert!(unknown.is_empty());
        assert_eq!(options.debug_draw, Some(DebugDraw::Demo(DebugDrawDemo::Shapes)));
        assert_eq!(
            parse(&["--debug-draw=lines:2000"]).unwrap().0.debug_draw,
            Some(DebugDraw::Demo(DebugDrawDemo::Lines(2000)))
        );
        assert!(parse(&["--debug-draw", "circles"]).is_err());
        // The views of a fight have nothing to show without one.
        let error = parse(&["--debug-draw", "volumes"]).expect_err("no fight");
        assert!(error.contains("--replay or --record"), "{error}");
        let (options, _) = parse(&["--capture", "x", "--replay", "a", "--debug-draw", "combat"]).unwrap();
        assert!(matches!(options.debug_draw, Some(DebugDraw::Views(_))));
        assert!(parse(&["--record", "a", "--scenario", "s", "--debug-draw", "colliders"]).is_ok());
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
        let (options, _) = parse(&["--look-dev", "hero", "--offscreen", "--size", "1280x720"]).unwrap();
        assert_eq!(
            options.offscreen,
            Some(Offscreen { mode: OffscreenMode::Serve, width: 1280, height: 720 })
        );
        let (options, _) = parse(&["--offscreen", "--frames", "600"]).unwrap();
        assert_eq!(
            (options.offscreen, options.frame_limit),
            (Some(Offscreen { mode: OffscreenMode::Serve, width: 1920, height: 1080 }), Some(600)),
            "--frames ends it"
        );
        assert!(is_offscreen_flag("--offscreen"), "never opens the window");
        assert!(parse(&["--offscreen", "--bench", "10"]).is_err(), "exclusive");
        assert!(parse(&["--offscreen", "--capture", "a.png"]).is_err(), "exclusive");
        assert!(parse(&["--offscreen", "--capture-ticks", "4"]).is_err(), "needs --capture");
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
    fn boolean_flags_refuse_inline_values() {
        for flag in ["--lights=false", "--no-hot-reload=false", "--offscreen=false", "--fight=true"] {
            let error = parse(&[flag]).unwrap_err();
            assert!(error.contains("takes no value"), "{flag}: {error}");
        }
        let (_, unknown) = parse(&["--platform-option=false"]).unwrap();
        assert_eq!(unknown, ["--platform-option=false"]);
    }

    #[test]
    fn a_contact_sheet_cannot_play_a_device_cue() {
        let error = parse(&["--contact-sheet", "light_1", "sheet.png", "--cue", "hit"]).unwrap_err();
        assert!(error.contains("--cue") && error.contains("--contact-sheet"), "{error}");
    }

    #[test]
    fn keeps_unknown_arguments() {
        let (options, unknown) = parse(&["-NSDocumentRevisionsDebugMode", "YES"]).unwrap();
        assert_eq!(options, Options::default());
        assert_eq!(unknown, ["-NSDocumentRevisionsDebugMode", "YES"]);
    }

    #[test]
    fn replays_and_recordings_go_with_a_capture_or_a_bench() {
        let (options, _) = parse(&["--capture", "x.png", "--replay", "a.sreplay"]).unwrap();
        let replay = |to_end| CombatOption::Replay { path: PathBuf::from("a.sreplay"), to_end };
        assert_eq!(options.combat, Some(replay(true)));
        let (options, _) =
            parse(&["--capture", "x.png", "--replay=a.sreplay", "--capture-ticks", "30"]).unwrap();
        assert_eq!(options.combat, Some(replay(false)));
        let (options, _) = parse(&["--bench", "10", "--replay", "a.sreplay"]).unwrap();
        assert_eq!(options.combat, Some(replay(true)));
        let (options, _) =
            parse(&["--bench", "10", "--record", "a.sreplay", "--scenario", "s", "--script", "p"]).unwrap();
        assert_eq!(
            options.combat,
            Some(CombatOption::Record {
                path: PathBuf::from("a.sreplay"),
                scenario: "s".to_owned(),
                script: Some("p".to_owned()),
            })
        );
        // The windowed app records whoever's playing: it has no offscreen mode to refuse.
        let (options, _) = parse(&["--record", "a.sreplay", "--scenario", "s"]).unwrap();
        assert_eq!(
            options.combat,
            Some(CombatOption::Record {
                path: PathBuf::from("a.sreplay"),
                scenario: "s".to_owned(),
                script: None,
            })
        );
        assert!(options.offscreen.is_none());
        options.refuse_offscreen().expect("Android's windowed app can record too");

        let refused = |args: &[&str], part: &str| {
            let error = parse(args).expect_err("refused");
            assert!(error.contains(part), "{args:?}: {error}");
        };
        // A capture of its end draws as many frames as it likes: that's checked once it's read.
        assert!(
            parse(&["--capture", "x", "--replay", "a", "--capture-frames", "200", "--capture-step", "1"])
                .is_ok()
        );
        refused(
            &[
                "--capture",
                "x",
                "--replay",
                "a",
                "--capture-ticks",
                "90",
                "--capture-frames",
                "200",
                "--capture-step",
                "1",
            ],
            "before tick 0",
        );
        // A replay has nothing to show in a window yet, and a window's recording is of its player.
        refused(&["--replay", "a.sreplay"], "--capture");
        refused(&["--record", "a", "--scenario", "s", "--script", "p"], "--capture");
        refused(&["--offscreen", "--replay", "a.sreplay"], "--capture");
        // Nothing without a window has a player, so `--offscreen` plays a script (M2 T2).
        refused(&["--offscreen", "--record", "a", "--scenario", "s"], "no one's there");
        parse(&["--offscreen", "--record", "a", "--scenario", "s", "--script", "p"]).unwrap();
        refused(&["--capture", "x", "--record", "a", "--scenario", "s"], "--script");
        refused(&["--bench", "10", "--record", "a", "--scenario", "s"], "--script");
        refused(&["--record", "a"], "--scenario");
        refused(&["--capture", "x", "--record", "a"], "--scenario");
        refused(&["--capture", "x", "--scenario", "s"], "--record");
        refused(&["--capture", "x", "--replay", "a", "--scenario", "s"], "--record");
        refused(&["--capture", "x", "--replay", "a", "--record", "b"], "exclusive");
        refused(&["--capture", "x", "--replay"], "--replay needs a value");
    }

    #[test]
    fn a_bench_trace_goes_with_a_bench_of_a_fight() {
        for fight in [&["--replay", "x"][..], &["--fight", "--arena", "--script", "idle"]] {
            let args = [&["--bench", "1", "--bench-trace", "t.csv"], fight].concat();
            let (options, _) = parse(&args).unwrap();
            assert_eq!(options.bench_trace, Some(PathBuf::from("t.csv")));
        }
        for args in [
            &["--bench", "1", "--bench-trace", "t.csv"][..],
            &["--capture", "x.png", "--replay", "x", "--bench-trace", "t.csv"],
            &["--bench", "1", "--record", "x", "--scenario", "y", "--script", "z", "--bench-trace", "t.csv"],
        ] {
            assert!(parse(args).unwrap_err().contains("--bench-trace"), "{args:?}");
        }
    }

    #[test]
    fn drawing_one_frame_in_n_goes_with_a_capture_offscreen_or_a_bench_of_a_fight_or_a_zone() {
        for args in [
            &["--capture", "x.png", "--draw-every", "50"][..],
            &["--capture", "x.png", "--replay", "x", "--draw-every", "50"],
            &["--capture", "x.png", "--zone", "ward", "--draw-every", "50"],
            &["--offscreen", "--draw-every", "50"],
            &["--bench", "1", "--replay", "x", "--draw-every", "50"],
            &["--bench", "1", "--fight", "--arena", "--script", "x", "--draw-every", "50"],
            &["--bench", "1", "--zone", "ward", "--draw-every", "50"],
        ] {
            let (options, _) = parse(args).unwrap_or_else(|error| panic!("{args:?}: {error}"));
            assert_eq!(options.draw_every.map(NonZeroU32::get), Some(50), "{args:?}");
        }
        for args in [
            &["--draw-every", "50"][..],
            &["--bench", "1", "--draw-every", "50"],
            &["--bench", "1", "--arena", "--draw-every", "50"],
            &["--offscreen", "--draw-every", "0"],
            &["--bench", "1", "--replay", "x", "--draw-every", "0"],
        ] {
            assert!(parse(args).unwrap_err().contains("--draw-every"), "{args:?}");
        }
    }

    #[test]
    fn cues_go_with_a_fight() {
        let (options, _) = parse(&["--capture", "x.png", "--replay", "a.sreplay", "--cues"]).unwrap();
        assert!(options.cues);
        let (options, _) = parse(&["--capture", "x.png", "--replay", "a.sreplay"]).unwrap();
        assert!(!options.cues);
        let refused = |args: &[&str], with: &str| {
            let error = parse(args).map(|_| ()).unwrap_err();
            assert!(error.contains(with), "{args:?}: {error}");
        };
        refused(&["--capture", "x", "--cues"], "--replay or --record");
        let (options, _) = parse(&["--capture", "x", "--replay", "a", "--cues", "--arena"]).unwrap();
        assert!(options.cues, "a fight's cues land where its actors are");
        refused(&["--capture", "x", "--replay", "a", "--cues", "--vfx", "arc"], "exclusive");
        refused(&["--cues", "--record", "a", "--scenario", "s", "--cue", "hit"], "exclusive");
    }

    #[test]
    fn the_windowed_app_refuses_offscreen_modes() {
        for args in [
            &["--capture", "x.png"][..],
            &["--bench", "10"][..],
            &["--capture", "x.png", "--replay", "a.sreplay"][..],
            &["--bench", "10", "--replay", "a.sreplay"][..],
        ] {
            let (options, _) = parse(args).unwrap();
            assert!(options.refuse_offscreen().unwrap_err().contains("Android"), "{args:?}");
        }
        let (options, _) = parse(&["--look-dev", "wide"]).unwrap();
        assert_eq!(options.refuse_offscreen(), Ok(()));
    }

    #[test]
    fn scripted_runs_refuse_unknown_arguments() {
        for args in [
            &["--capture", "x.png", "--plain-psot", "--shadow", "off"][..],
            &["--plain-psot", "--bench", "10", "--shadow", "off"][..],
        ] {
            let (options, unknown) = parse(args).unwrap();
            assert_eq!(unknown, ["--plain-psot", "--shadow", "off"]);
            let error = options.refuse_unknown(&unknown).unwrap_err();
            assert!(error.contains("--plain-psot") && error.contains("--shadow"), "{error}");
        }
        let (options, unknown) = parse(&["--capture", "x.png", "--plain-post", "--shadows", "off"]).unwrap();
        assert_eq!(options.refuse_unknown(&unknown), Ok(()));
    }

    #[test]
    fn other_runs_keep_the_warning_for_unknown_arguments() {
        for args in [&["-NSDocumentRevisionsDebugMode", "YES"][..], &["--offscreen", "--plain-psot"][..]] {
            let (options, unknown) = parse(args).unwrap();
            assert!(!unknown.is_empty());
            assert_eq!(options.refuse_unknown(&unknown), Ok(()));
        }
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
    fn vfx_takes_a_show() {
        assert_eq!(parse(&["--vfx", "duel"]).unwrap().0.vfx, Some(VfxShow::Duel));
        assert_eq!(parse(&["--vfx=arc"]).unwrap().0.vfx, Some(VfxShow::Arc));
        assert_eq!(parse(&[]).unwrap().0.vfx, None);
        let error = parse(&["--vfx", "fireworks"]).unwrap_err();
        assert!(error.contains("arc, hit, dust, duel"), "{error}");
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
