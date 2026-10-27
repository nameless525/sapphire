//! Sapphire Sandbox, the engine test bed (milestone M0, spec §10).
//!
//! It runs the whole engine path on every platform: the SDL3 platform layer, the fixed 60 Hz
//! simulation on the engine clocks and ECS, time of day (`sapphire_world::tod`) resolving the
//! frame's look and the character's light rig, plain-data
//! [`FramePacket`](sapphire_render::FramePacket)s filled by
//! change-detecting extract systems, and the wgpu anime renderer's persistent scene, drawing the
//! cooked sample mannequin.
//!
//! # Controls
//!
//! The default bindings are a `sapphire_input` document, `default_bindings.ron`, compiled into
//! the app (see `input.rs`).
//!
//! | Input | Action |
//! |---|---|
//! | Left-drag, one-finger drag, right stick, arrow keys | Orbit |
//! | Wheel, pinch, triggers | Zoom |
//! | `Space`, gamepad South, double tap | Toggle world slow motion (perfect-dodge style, 0.2×) |
//! | `H`, gamepad East, long press (0.5 s) | 8-tick hitstop on the turntable |
//! | `Esc` | Quit (desktop) |
//! | `` ` ``, a still two-finger hold (1 s) | Show or hide the debug overlay (`dev` builds) |
//!
//! # Development builds
//!
//! The `dev` feature links `sapphire_debug`, `sapphire_hot_reload` and `sapphire_livelink` (rule
//! R8), all of it behind `dev.rs`: the debug overlay, which sees platform events before the
//! game's input, applies its time and time-of-day controls before each fixed step and draws after
//! post; the cvar registry it edits, holding the sandbox's cvars (`cvars.rs`, compiled in every
//! build so they read their defaults), the renderer's quality cvars (`r.tier`,
//! `r.render_scale`, `r.dynamic_resolution`, `r.outline_distance`) and its debug view
//! (`r.debug_view`, M1 F5); hot reload (`reload.rs`, M1
//! A8), on by default on desktop: rewritten packs in the content root are remounted and their
//! changed assets replaced in place, edited WGSL rebuilds the renderer's pipelines, an edited
//! `default_day.ron` replaces time of day's timeline and an edited `default_light_rigs.ron` the
//! character light rig presets, all without a restart; and live link (`link.rs`, M1 A9), on by default on every
//! platform, listening on the loopback interface (127.0.0.1:7811): `cargo xtask link` pushes
//! the same edits from the desktop (packs, sending only the assets that changed; the two
//! documents; WGSL sources), sets cvars and captures the next frame drawn, on a phone as on the
//! desktop.
//! Without the feature none of it is compiled.
//!
//! # Arguments
//!
//! - `--frames N`: exit after N presented frames, for unattended runs; fails if they can't be
//!   presented in time.
//! - `--tier desktop-high|desktop-low|mobile-high|mobile-low`: the player's quality tier
//!   (`sapphire_render::quality`) instead of the one selected for the device. The `r.tier` cvar
//!   overrides it in `dev` builds, and a change makes a new renderer.
//! - `--render-scale <0.25..1>`: the player's base render scale instead of the tier's.
//! - `--dynamic-resolution`: let dynamic resolution lower the render scale to hold the tier's
//!   GPU budget (off by default; `r.dynamic_resolution` overrides it).
//! - `--aa off|msaa|taa`: the antialiasing technique instead of the tier's (temporal AA on the
//!   desktop tiers, 4× MSAA on mobile ones; M1 R7): `msaa` is the tier's sample count or 4×,
//!   `taa` turns MSAA off. For comparisons and golden images.
//! - `--msaa off|2|4|8`: override the sample count the tier or `--aa` gives.
//! - `--shadows on|off`: the player's shadow setting (M1 R4); the tier's shadows without it. Off
//!   draws no shadow maps and compiles the shaders as they were before shadows, which the golden
//!   scenes from before them pin.
//! - `--ao off|capsule|full`: the player's ambient occlusion setting (M1 W5); the tier's without
//!   it. `capsule` is capsule AO from the characters' bodies alone, `full` adds GTAO (the
//!   tier's, or the desktop tiers' on Mobile-Low), and `off`, which the golden scenes from before
//!   W5 pin, compiles the environment shader as it was before ambient occlusion.
//! - `--spin <radians per second>`: the turntable's speed (default 0.35), for edges in motion.
//! - `--lights`: place three colored local lights around the character.
//! - `--skinned`: place a procedural skinned and morphed stalk, swaying, beside the character
//!   (compute skinning, M1 R2).
//! - `--swatches`: place three spheres on the character's other side that show the anime
//!   character shader's diffuse modes over cooked maps (ramp mode, ramp mode with a subsurface
//!   mix, three bands; M1 C1).
//! - `--materials`: place the material layer samples on the character's right (M1 C7): spheres
//!   of lacquer (a crisp highlight), gold (a tinted matcap and the reflection add), leather (a
//!   multiplied matcap, a GGX highlight) and a suit with breathing light lines, and a leg in a
//!   thigh-high stocking.
//! - `--camera-rim <share>`: give every light rig preset a camera rim taking this share of the
//!   rim's gating, from behind the character on the camera's right and above (M1 C8), so the rim
//!   stays on the silhouette however the rig folds the key; `--rim-rig folded` shows it.
//! - `--shade-hue <r>,<g>,<b>,<share>`: key characters' shadow sides to keep this linear RGB hue
//!   by this share of their light, all day (M1 C8), into the day the scene runs on.
//! - `--two-sided`: place the two-sided garment samples on the character's left (M1 C8): a
//!   collar, a skirt and a twisted ribbon, each one-sided (the insides culled) beside two-sided
//!   (the insides shaded as the side the camera sees). Exclusive with `--swatches`.
//! - `--fade <0..1>`: draw the characters at this opacity through the proximity fade's dither
//!   (M1 C7), which temporal AA resolves into a blend and MSAA covers as a share of samples.
//! - `--proximity-fade`: fade the characters as the camera comes within half a meter of their
//!   bodies (M1 C7; `sapphire_world::fade::ProximityFade`'s defaults).
//! - `--faces`: show the face look-dev rig instead of the character: rows of the face test head,
//!   each a character lit by a key light of its own from an angle in its head space (a yaw sweep
//!   across both sides, top light), and two with blush and gloom (M1 C3).
//! - `--face-layout lights|hair|highlights|turn|gi|gi-clamped|eyes|eyes-turned|gaze|blink` (with
//!   `--faces`): the face rig's heads (M1 C5): the lights above (the default); keys around the
//!   front and above, for the bangs' shadow on the face; heads nodding under one key, for the
//!   hair's highlights; heads turning and nodding from the camera, for the eyes and brows through
//!   the bangs. Then the GI layouts (M1 C2): heads turned and nodding every way, two in gloom, lit
//!   by the default light rig preset's stylized GI alone, the key light off and the GI raised
//!   four-fold (`gi`), or the same with its HSV clamp's ceilings pulled down into the GI's range
//!   (`gi-clamped`). Then the eyes' layouts (M1 C6), framed close on the eyes: one head facing the
//!   camera, one turned to three-quarter, heads looking around, and lids half closed and closed.
//! - `--hair-shadow-softness <meters>`: soften every face material's bangs' shadow to an edge
//!   this wide (M1 C8), as it loads: the face rig's heads (`--face-layout hair` shows it) and the
//!   look-dev character.
//! - `--rim-rig angles|backlit|folded`: show the rim look-dev rig instead of the character: a row
//!   of it before dark and bright panels, each figure lit from its own angle around it, from the
//!   front round to behind (`angles`) or from behind at several heights (`backlit`; M1 C5), or
//!   from behind through the gameplay rig's limits, which fold the keys round toward the camera
//!   (`folded`; M1 C8).
//! - `--environment`: place the character in the courtyard, the environment shader's test scene
//!   (rolling ground, boulders, a colonnade, a wall and paper lanterns in environment materials;
//!   M1 W4), seen from farther back, with fog cards (mist across the valley, haze among the
//!   pillars; M1 W3) unless `--fog off`.
//! - `--foliage grove|backlit|ground`: plant two trees, three bushes and patches of grass in the
//!   courtyard (implying `--environment`), drawn by the foliage shader in a breeze (M1 W6), and
//!   show the grove whole, the big tree against the key light (light through its leaves), or the
//!   grass and a bush up close.
//! - `--wind <0..4>`: the wind's strength instead of `--foliage`'s breeze of 1; 0 is calm.
//! - `--probes`: the courtyard with its arcade (a gateway through the wall and a roofed loggia
//!   along it), lit by the ambient probe grid the cooker baked for both (M1 W5) in place of the
//!   ambient hemisphere, seen through a longer lens turned to the arcade. Implies
//!   `--environment`.
//! - `--look-dev wide|hero|sky|street|face`: the look-dev town (M1 X1, `lookdev.rs`): pixiv's
//!   VRM sample in place of the mannequin, held standing in a Jinzhou-style town square of the
//!   assembly's parametric buildings in the environment family, with its fog cards and lamps,
//!   every landed feature on, seen from an authored shot: `wide` across the square, `hero` a
//!   three-quarter shot of the character, `sky` low and looking up past her, `street` her
//!   standing in the street out of the square, `face` a close-up of her face and shoulders. A
//!   scene of its own, not a `--stage` layout: it replaces the character and the ground under
//!   it, so `--stage`, `--environment`, `--probes`, `--foliage`, `--outline-row`, `--faces`,
//!   `--rim-rig` and `--hard-edges` can't join it. `--time-of-day` and `--day-length` run its
//!   day; `--view`, `--fov` and `--zoom` adjust the shot. The golden scenes
//!   `lookdev-town-day`, `-hero`, `-golden`, `-blue` and `-night`, `lookdev-street-night` and
//!   `lookdev-face` show it.
//! - `--outline-row`: place more of the character behind it, from 4 m to 90 m further back, past
//!   the outline cutoff, showing the outline width's distance curve (M1 C4).
//! - `--hard-edges`: show the hard-edged outline sample instead of the character: flat-shaded
//!   shapes whose cooked outline normals keep every corner's hull closed (M1 C4).
//! - `--outline-distance <0..4>`: the scale on every material's outline LOD cutoff
//!   (`r.outline_distance`, 1 by default); 0 draws no outlines, for profiling.
//! - `--debug-view off|wireframe|overdraw|lights|material-ids|ramp-rows|outline-widths`: draw
//!   the frame as one of the renderer's debug views (`r.debug_view`, M1 F5): its triangles'
//!   edges, the fragments it shades per pixel, the local lights each pixel walks, or the anime
//!   characters' regions, ramp rows or outline widths, over the frame after post. In every
//!   build, so captures and golden scenes show them; the overlay's cvar panel switches them in
//!   `dev` builds.
//! - `--fov <degrees>`: the camera's vertical field of view (default 43°), keeping the framing:
//!   a narrow lens from farther away.
//! - `--zoom <factor>`: start the orbit camera this factor of the framing's distance away.
//! - `--stage floor|pillar|plinth`: stand the character on a floor, with its pedestal split off
//!   as environment, and with `pillar` a stone pillar toward the key light whose shadow crosses the
//!   character, or the face rig with `--faces` (M1 R4's shadow scenes); with `plinth` the pedestal
//!   is stone, an environment material, so capsule AO gathers around the feet (M1 W5).
//! - `--light-path clustered|per-object`: override how the tier lists local lights (clustered
//!   on desktop tiers, per-object on mobile ones).
//! - `--time-of-day <hours>`: start the day at this hour, `0 ≤ hours < 24` (default 10, the
//!   default day's key that is exactly the look from before time of day). Time stands still
//!   unless `--day-length` runs it.
//! - `--day-length <seconds>`: run the day, one every this many seconds of world time (so
//!   pause and slow motion reach it).
//! - `--character-rig <preset>`: light the character with this character light rig preset
//!   (`sapphire_world::light_rig`; `gameplay`, the default, or `portrait`).
//! - `--cloud-cover <0..1>`: hold both cloud layers at this coverage instead of time of day's;
//!   `0` is a clear sky.
//! - `--plain-sky`: draw the sky's gradient alone (no discs, glows, stars or clouds), a backdrop
//!   the same at any hour, which the golden scenes from before the sky's layers pin.
//! - `--fog on|off`: draw time of day's fog (M1 W3), on by default. Off leaves it out of every
//!   frame and places no fog cards in the courtyard, so frames draw exactly as before fog, which
//!   the golden scenes from before it pin.
//! - `--plain-post`: draw post as it was before its compositing layers (`PostParams::plain`: no
//!   diffusion glow, character bloom, gradient layers, lens flare, vignette tint or sharpening),
//!   which the golden scenes from before them pin (M1 P2). Post then builds none of the layers'
//!   pipeline variants (`RenderSettings::post_variants`).
//! - `--sharpening <0..1>`: the player's sharpening (RCAS in post's fused pass) instead of the
//!   `post.sharpening` cvar's, 0 by default (M1 P2).
//! - `--post-effect <preset>[@<seconds>]`, repeatable: push a screen effect preset (M1 P3:
//!   `radial_blur`, `chromatic_aberration`, `inversion`, `duotone`, `speed_lines`,
//!   `impact_frame`) at the start, playing, or held that many seconds into it for captures and
//!   benches. An unknown name fails with the presets' names. The renderer prewarms the post
//!   variants of the presets pushed (`Renderer::prewarm_post`), or in `dev` builds of every
//!   preset, which the overlay fires, so a scene without effects builds none of theirs.
//! - `--view <yaw>,<pitch>`: start the orbit camera at these angles in degrees (yaw orbits right
//!   from in front of the character; a negative pitch looks up at the sky).
//! - `--no-hot-reload` (`dev` builds): watch nothing, so files edited during the run don't
//!   change it.
//! - `--live-link off|<addr>|listen=<addr>|connect=<addr>` (`dev` builds): how live link takes
//!   connections instead of listening on 127.0.0.1:7811: listening at another address (one
//!   beyond loopback, such as `0.0.0.0:7811`, takes pushes from the network: an opt-in, logged as
//!   a warning), dialing a tool listening at `<addr>` (behind `adb reverse`), or not at all.
//! - `--debug-overlay[=panels]` (`dev` builds): show the overlay from the start, with its
//!   default panels or a comma-separated list (`stats`, `graph`, `time`, `day`, `effects`,
//!   `console`, `cvars`, `all`). Offscreen captures draw it too.
//! - Offscreen modes (desktop), which never open a window or take focus, at `--size WxH`
//!   (default 1920x1080):
//!   - `--capture <png> [--capture-ticks N]`: render the scene after N fixed ticks (default
//!     90), write a PNG and exit. `--capture-frames F` draws F frames and writes the last, for
//!     temporal AA's history; `--capture-step S` puts S ticks between them (0, the default:
//!     the scene holds still at tick N; 1: it plays at 60 fps up to tick N).
//!   - `--bench <frames>`: run the per-frame CPU path that many times and log CPU time and
//!     allocations per frame, then GPU pass times.
//!   - `--offscreen` (`dev` builds): keep drawing at up to 60 frames a second, one fixed step
//!     each, until the process is told to quit (Ctrl-C, `kill`) or `--frames N` have been drawn,
//!     with hot reload and live link on (`serve.rs`): what a device runs, without a window.
//!     `cargo xtask link capture <png>` writes what it draws.
//!   - `--serial-passes`: submit each frame graph pass on its own and wait for the GPU after
//!     it, logging each at debug level: a GPU crash or device loss then names its pass in the
//!     log, even on drivers that report nothing. Slow; for debugging.
//!
//! Desktop runs need cooked content: `cargo xtask run --platform macos` cooks and launches.
//!
//! # The sky
//!
//! The zone's sky is the sky material `samples/sky.material.ron` (`content::SKY`), loaded with its
//! maps at startup and after every renderer rebuild, and drawn with time of day's sky layers:
//! card clouds on the horizon, cirrus, the sun and moon with their glows, stars and the Milky Way
//! (spec §5.12, "Sky").

mod app;
mod bench;
mod capture;
mod content;
mod cvars;
#[cfg(feature = "dev")]
mod dev;
mod environment;
mod extract;
mod faces;
mod foliage;
mod garments;
mod input;
mod layers;
#[cfg(feature = "dev")]
mod link;
mod lookdev;
mod offscreen;
mod options;
mod orbit;
mod outlines;
mod pose;
#[cfg(feature = "dev")]
mod reload;
mod rims;
mod scene;
#[cfg(feature = "dev")]
mod serve;
mod skinned;
mod stage;
mod stats;
mod swatches;
mod turntable;

use std::cell::RefCell;
use std::fmt::Write as _;
use std::sync::{Mutex, Once};

use sapphire_core::alloc::EngineAllocator;
use sapphire_core::log::{LogConfig, LogLevel, LogRecord, LogSink};
use sapphire_crash::{CrashConfig, CrashGuard};
use sapphire_platform::{App, DeviceClass, Os};

pub use app::SandboxApp;
pub use offscreen::OffscreenApp;

/// mimalloc, counted so the frame stats and the R6 check can see allocations.
#[global_allocator]
static GLOBAL: EngineAllocator = sapphire_core::alloc::engine_allocator();

/// Log target for the sandbox.
pub(crate) const LOG_TARGET: &str = "sapphire::sandbox";

/// Crash capture for the whole process: installed by [`startup`], released by [`shutdown`].
static CRASH_GUARD: Mutex<Option<CrashGuard>> = Mutex::new(None);

/// Process setup that runs before SDL starts, in the order `sapphire_crash` requires:
/// logging, crash capture, then the job system (spec §5.2).
///
/// Called from `main` on desktop and iOS (after `sapphire_crash::run_monitor_if_requested`),
/// and from `SDL_main` on Android, where it must not panic. Android can start `SDL_main`
/// again in the same process after the activity is recreated, so only the first call does
/// anything.
pub fn startup() {
    static STARTUP: Once = Once::new();
    STARTUP.call_once(|| {
        // Mobile has no visible stderr: route log lines to logcat / the unified log.
        let mobile = Os::CURRENT.device_class() == DeviceClass::Mobile;
        let config = LogConfig { stderr: !mobile, ..LogConfig::default() };
        let sink = mobile.then(|| Box::new(PlatformLogSink) as Box<dyn LogSink>);
        if let Err(error) = sapphire_core::log::init(config, sink) {
            sapphire_platform::sdl_log(tracing::Level::WARN, &format!("logging setup failed: {error}"));
        }

        let guard = install_crash_capture();
        *CRASH_GUARD.lock().unwrap_or_else(|poisoned| poisoned.into_inner()) = guard;

        let jobs = sapphire_jobs::init(sapphire_jobs::JobConfig::default());
        tracing::info!(target: LOG_TARGET, topology = ?jobs.topology(), config = ?jobs.config(), "job system ready");
    });
}

/// Whether the arguments ask for an offscreen mode (`--capture`, `--bench` or `--offscreen`),
/// which `main` runs as [`OffscreenApp`] in the background instead of opening the windowed app.
pub fn offscreen_requested() -> bool {
    std::env::args_os().any(|arg| arg.to_str().is_some_and(options::is_offscreen_flag))
}

/// Releases process-wide resources once the platform loop has returned (desktop). iOS never
/// returns from `run`, and Android keeps the process alive, so neither calls it.
pub fn shutdown() {
    drop(CRASH_GUARD.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).take());
}

/// Installs crash capture into the app's cache directory, which is never backed up. Failure
/// leaves the app running without it.
fn install_crash_capture() -> Option<CrashGuard> {
    let metadata = SandboxApp::METADATA;
    let dir = match sapphire_platform::cache_dir(&metadata) {
        Ok(dir) => dir.join("crashes"),
        Err(error) => {
            tracing::warn!(target: LOG_TARGET, %error, "no cache directory; crash capture off");
            return None;
        }
    };
    if let Ok(pending) = sapphire_crash::pending_reports(&dir)
        && !pending.is_empty()
    {
        tracing::warn!(target: LOG_TARGET, count = pending.len(), dir = %dir.display(), "crash reports from earlier runs");
    }
    match sapphire_crash::install(CrashConfig::new(metadata.name, metadata.version, dir)) {
        Ok(guard) => {
            tracing::info!(target: LOG_TARGET, capture = ?guard.native_capture(), run = guard.run_id(), "crash capture on");
            Some(guard)
        }
        Err(error) => {
            tracing::warn!(target: LOG_TARGET, %error, "crash capture unavailable");
            None
        }
    }
}

/// Forwards log records to the platform logger (logcat, the Apple unified log), prefixed
/// with their target.
struct PlatformLogSink;

impl LogSink for PlatformLogSink {
    fn log(&self, record: &LogRecord<'_>) {
        thread_local! {
            // Reused per thread to keep line building cheap.
            static LINE: RefCell<String> = const { RefCell::new(String::new()) };
        }
        let level = tracing_level(record.level);
        // Never panic in a log call: if the buffer is unavailable (logging from inside this
        // sink, or during thread-local teardown), build the line on the spot.
        let written = LINE.try_with(|line| {
            let Ok(mut line) = line.try_borrow_mut() else { return false };
            line.clear();
            let _ = write!(line, "[{}] {}", record.target, record.message);
            sapphire_platform::sdl_log(level, &line);
            true
        });
        if written != Ok(true) {
            sapphire_platform::sdl_log(level, &format!("[{}] {}", record.target, record.message));
        }
    }
}

fn tracing_level(level: LogLevel) -> tracing::Level {
    match level {
        LogLevel::Error => tracing::Level::ERROR,
        LogLevel::Warn => tracing::Level::WARN,
        LogLevel::Info => tracing::Level::INFO,
        LogLevel::Debug => tracing::Level::DEBUG,
        LogLevel::Trace => tracing::Level::TRACE,
    }
}

sapphire_platform::export_android_main!(SandboxApp, init = startup);
