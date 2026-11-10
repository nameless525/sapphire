//! The golden-image scenes and their runner, behind `cargo xtask golden` (spec §9 gate 3; M1
//! plan §3, open question Q6).
//!
//! Each scene is a sandbox `--capture` at 640×360 with fixed ticks: no window, no focus, and
//! fast enough on CPU rasterizers. The capture's adapter record names the backend family
//! ([`backend_key`]), and the capture is compared with `<golden dir>/<scene>.<family>.png`.
//!
//! - A failing comparison leaves `<scene>.<family>.actual.png` and `.diff.png` (the FLIP error
//!   map) in the output directory, which CI uploads.
//! - A scene with no golden for this family fails in CI (`CI` is set) and is skipped locally.
//!   Its capture is kept as `.actual.png`, ready to commit.
//! - A golden or capture that can't be read or compared (another size, a capture cut short) fails
//!   its scene as an error, naming it and keeping the capture, and the run goes on (review DV-8).
//! - [`Mode::Update`] writes this family's goldens from the captures instead of comparing.
//!
//! Each scene's sandbox writes its output to `<scene>.log` in the output directory and has a
//! deadline (`--scene-timeout`, [`DEFAULT_SCENE_TIMEOUT`]): one that runs past it is killed, fails
//! as timed out with its log's last lines, and the run goes on to the next scene (review BC-008).
//! Every scene's line gives its wall time, and `timings.json` in the output directory lists them
//! all (scene, family, seconds, status), rewritten after each scene so a cancelled run keeps what
//! it had. `--shard` splits the scenes by those times as measured in CI ([`Shard`], review
//! BC-107).

mod costs;

use std::ffi::OsString;
use std::fmt::Write as _;
use std::fs::{self, File};
use std::io::{self, Read as _, Seek as _, SeekFrom};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::time::Duration;

use sapphire_time::Stopwatch;

use self::costs::Family;
use crate::golden::{self, Image, Thresholds, backend_key};

/// The environment variable the sandbox reads its cooked content from.
const CONTENT_DIR_ENV: &str = "SAPPHIRE_CONTENT_DIR";
/// The environment variable that forces the sandbox's GPU backend (`sapphire_rhi`'s
/// `BACKEND_OVERRIDE_ENV`), which CI's golden jobs set: it says whose costs `--shard` splits by.
const BACKEND_ENV: &str = "SAPPHIRE_GPU_BACKEND";
/// Small enough for lavapipe and WARP to render each scene in a few seconds.
const SIZE: &str = "640x360";
/// How long a scene may take unless `--scene-timeout` says otherwise: about seven times the
/// slowest scene on a CPU rasterizer (WARP's `foliage-ground`, about 130 s in CI), since a slow
/// runner must not fail a scene that isn't hung.
pub const DEFAULT_SCENE_TIMEOUT: Duration = Duration::from_secs(900);
/// The highest mean error `turntable-taa` and `turntable-taa-upsampled` allow, where the default
/// allows 0.01 (review DV-2). They differ only in `--render-scale 0.667`, which the second exists to
/// show, and read as each other they stay inside every default gate on every family: mean 0.0085
/// to 0.0095, p99 0.066, worst tile 0.063. Their own noise leaves room for this one gate: at most
/// 0.0046 on lavapipe, 0.0005 on Metal and none on WARP. The mean separates them best: 0.0046 from
/// 0.0085, where the p99 and the worst tile have 0.044 from 0.062.
const TAA_RENDER_SCALE_MEAN: f32 = 0.0065;
/// How many of a failed scene's last log lines its failure quotes.
const LOG_TAIL_LINES: usize = 40;
/// Waiting on a sandbox polls with exponential backoff from [`FIRST_POLL`] to [`MAX_POLL`]: quick
/// scenes end within milliseconds of exiting, and slow ones cost no CPU.
const FIRST_POLL: Duration = Duration::from_millis(1);
const MAX_POLL: Duration = Duration::from_millis(50);

pub struct Scene {
    pub name: &'static str,
    /// Sandbox arguments on top of `--capture <png> --size 640x360`.
    pub args: &'static [&'static str],
    pub thresholds: Thresholds,
}

/// A game screen's golden (M3 U7): on a flat color, so it stays cheap and moves only for what it shows, at the
/// size and in the language named. Anything after that goes on its command line.
macro_rules! game_scene {
    ($name:literal, $screen:literal, $size:literal, $locale:literal $(, $more:literal)*) => {
        Scene {
            name: $name,
            args: &[
                "--size",
                $size,
                "--capture-ticks",
                "2",
                "--tier",
                "desktop-high",
                "--plain-sky",
                "--plain-post",
                "--aa",
                "msaa",
                "--ao",
                "off",
                "--shadows",
                "off",
                "--fog",
                "off",
                "--ui-plain",
                "--ui-game",
                $screen,
                "--ui-locale",
                $locale,
                $($more,)*
            ],
            thresholds: Thresholds::default(),
        }
    };
}

/// The court gate's cutscene in the game as it ships (M3 Q4), at a size in pixels and in a language: a continue
/// from `ward-gate.progress.ron` walks back through the gate, into the cutscene at frame 17, and the last frame,
/// 3.9 s on, falls mid-way through the warden's line (2.9 to 4.9 s). The 160 frames before 161 are drawn a
/// tenth as wide and high (`--game-draw-from`), so a software rasterizer draws 91 frames at full size, not 251.
/// The cutscene's cut at 2.5 s, frame 166, drops TAA's history as the size changing does, so the last frame is
/// byte for byte the one drawing them all would give; drawing full size from 167 on isn't.
macro_rules! gate_scene {
    ($name:literal, $size:literal, $language:literal) => {
        Scene {
            name: $name,
            args: &[
                "--size",
                $size,
                "--capture-ticks",
                "251",
                "--tier",
                "desktop-high",
                "--game",
                "continue",
                "--progress",
                "tests/saves/ward-gate.progress.ron",
                "--game-forward",
                "--game-language",
                $language,
                "--game-draw-from",
                "161",
            ],
            thresholds: Thresholds::default(),
        }
    };
}

/// The scenes, in the order they were added. Those from before the sky's layers (M1 W2) pin the
/// sky's gradient alone, `--plain-sky` (no discs, glows, stars or clouds), so they stay
/// pixel-identical to their goldens: they are about the character, and the sky has scenes of its
/// own. Those from before temporal AA (M1 R7) pin 4× MSAA (`--aa msaa`), which the desktop tiers
/// drew with until then, for the same reason. Those from before post's compositing layers (M1 P2)
/// pin post as it was, `--plain-post` (no diffusion glow, character bloom, gradient layers, lens
/// flare or vignette tint), which the post scenes show; the environment's (M1 W4) among them, which
/// draw the world as the desktop tiers do, under the whole sky and temporal AA. The outline scenes
/// (M1 C4) pin all three, so they show the lines alone. Those from before shadows (M1 R4), the
/// post and outline scenes included, pin them off (`--shadows off`, the player's quality setting),
/// so they stay pixel-identical too. The shadow scenes pin their tier, whose shadows they show
/// (the golden runners select Desktop-Low by device), and plain post, so they show shadows alone.
/// The screen effect scenes (M1 P3) push the sample presets held on one frame
/// (`--post-effect <preset>@<seconds>`) over the full look; no earlier scene pushes any, and an
/// empty effect stack draws exactly what it drew before, so nothing earlier needs pinning.
/// Every scene from before fog (M1 W3) pins it off (`--fog off`: no fog in any frame and no fog
/// cards in the courtyard), so it stays pixel-identical as well; the fog scenes pin plain post and
/// no shadows, so they show fog alone. The rim and hair scenes (M1 C5) pin the plain sky and post,
/// 4× MSAA, no shadows and no fog, so they show the character prepass's readers alone. The
/// foliage scenes (M1 W6) plant the foliage samples in the courtyard (`--foliage <view>`; no
/// earlier scene plants any, and the wind is calm without it) and pin plain post and no fog, so
/// they show foliage alone, its shadows on. Every scene from before ambient occlusion (M1 W5),
/// the screen effect, fog, rim, hair and foliage scenes included, pins it off (`--ao off`, the
/// player's quality setting), and none of them loads a probe grid, so the ambient hemisphere
/// lights them all as before. The ambient scenes (W5) pin their tier, plain post and no fog, so
/// they show the probe grid and ambient occlusion with the tier's shadows; the grid is the
/// desktop cook's, which the runner loads on every backend. The night scenes with characters
/// (`turntable-night`, `sky-night`, `environment-night`, `fog-night`, `ambient-night` and
/// `ambient-capsule`) changed with the night character light fix, which retuned the default day's
/// night character key; `night-character` draws a character at night under the whole look, which
/// no scene did before, with ambient occlusion off and no probe grid like every scene but W5's.
/// The look-dev scenes (M1 X1) pin Desktop-High, the tier whose look they judge, and ambient
/// occlusion off, as every scene but W5's: every other landed feature draws in them, as a game
/// would. They run on the stage's own day (`content/lookdev/lookdev_day.ron`, X1 part 2), which
/// only `--look-dev` loads, and plant its foliage, so no other scene sees either. The stylized GI scenes (M1 C2's follow-up) light the face rig by the character light
/// rig's GI alone, its key light off (`--face-layout gi` and `gi-clamped`), at hours whose keys the
/// night character light fix leaves as they were (17:30 and 10:00), and pin what the face scenes
/// pin, so they show the GI alone: each of its steps (the sphere and the camera bias on the GI's
/// normals, the HSV clamp's saturation and value, the skin tint, the occlusion) fails one of them
/// when broken. The eye scenes (M1 C6) pin what the rim and hair scenes pin, AO off among it, so
/// they show the eye layers alone, but for the highlight's, which draws post's glow and bloom
/// over its glint. The debug view scenes (M1 F5) alone set a view (`--debug-view <view>`; off,
/// every other scene's, builds and draws nothing, so nothing earlier needs pinning) and pin plain
/// post, no shadows and AO off, so the frame they draw over changes only with what they show;
/// the wireframe and overdraw keep fog for its cards.
/// The material layer scenes (M1 C7) place the layer samples beside the character
/// (`--materials`; no earlier scene places any, and every layer is off in every earlier material)
/// and pin 4× MSAA, no shadows, no fog and no AO, so they show the layers alone; the fade scenes
/// fade the character (`--fade`; no earlier scene fades one) in the courtyard under plain post,
/// AO off, through temporal AA and through MSAA.
/// The character shading follow-up scenes (M1 C8) turn one of its features on each, all off by
/// default, so no earlier scene draws any: the two-sided garments (`--two-sided`) pin what the
/// material layer scenes pin; the shadow hue scenes (`--shade-hue`) light the face rig at golden
/// hour, with and without the hue, and pin what the face scenes pin; the folded rim scenes light
/// the rim scenes' figures from behind through the gameplay rig's limits (`--rim-rig folded`),
/// with and without a camera rim (`--camera-rim`), and pin what the rim scenes pin; the soft
/// bangs' shadow scene softens the face rig's faces (`--hair-shadow-softness`) under the hair
/// scene's keys and pins what it pins; the back-lit hair scenes light the face rig from behind
/// (`--face-layout backlit`), without and with light through the hair (`--hair-transmission`),
/// and pin what the hair scenes pin.
///
/// The debug draw scene (M2 T1) alone turns debug draw on (`--debug-draw`; nothing else in these
/// builds draws any, so no earlier scene changes) and pins what the material layer scenes pin, with
/// plain post and a plain sky, so the frame under its lines changes only with what they show. Its
/// view of a fight (`debugdraw-volumes`) plays a committed replay, which the app checks against the
/// checksums beside it before it writes the picture.
///
/// The effects scenes (M2 E2) play `--vfx`'s shows at fixed ticks, whose simulation is seeded: the
/// sword arc at noon and at night, a hit at sunset and a duel's worst moment, on the look-dev stage
/// under temporal AA and at Mobile-Low, then alone with everything else pinned off.
///
/// The combat HUD scenes (M2 I2) alone draw the on-screen interface (`--hud fixed`, one frozen frame
/// on a made-up clock; no other scene has any, and a frame with no quads builds and draws nothing,
/// so nothing earlier needs pinning), over the look-dev stage, pinned as its scenes are but with 4×
/// MSAA in place of temporal AA, which needs no warm-up frames: the backdrop is a backdrop. Their
/// `--size`, after the runner's, sets each one's aspect ratio. `hud-energy-*` add the player's ultimate
/// (`--hud-energy`, M3 Q3c), its meter empty and full, on the wide frame and on the iPhone's.
///
/// The moving character's scenes (M2 N2) play a clip on the look-dev character (`--clip`; no earlier
/// scene plays one, and a character without a clip is never sampled) and pin what the look-dev
/// scenes pin.
///
/// The arena scenes (M2 O5) pin what the look-dev scenes pin: Desktop-High and ambient occlusion off,
/// and the plain camera (`--arena-shot follow`), so tuning the camera director moves none of them.
/// They alone run `--arena`, so no earlier scene sees its stage. `arena-replay` plays a replay on it,
/// and `arena-fight-telegraph`, `arena-fight-hit`, `arena-fight-cut` and `arena-fight-slash` a fight with
/// the enemy (M2 G6), side on, so the character isn't in front of what they're there to show.
/// `arena-party-second` and `arena-party-third` play a party's fight (M2 G7), and only they run `--party`.
/// `cutscene-gate-*` stand the arena's stage in the Lantern Ward's court for a replay that watches its gate
/// cutscene (M3 Q3), and only they play a cutscene from a fight. `arena-ultimate-chop` plays the player's
/// ultimate (M3 Q3b) on the arena's own stage, the only scene in a move's cinematic.
/// `arena-skirmish` plays eight fighters (M3 B1), on the director. `arena-boss` plays a boss (M3 B3), just
/// past the phase where it enrages, with its bar and phase marks. `arena-captain` draws two kinds of enemy at
/// once, each as itself (M3 B3b).
///
/// The tier scenes (M4 T0) draw three shots as each tier ships, AO and all, at the tier's own scale and
/// at its dynamic floor: the face's close-up on every tier, and the ward's street and a hit's post on the
/// low tiers. The phone tiers' set a `--size`, as the HUD scenes do, a phone's shape, and draw at the
/// scales a 1170-row phone draws, so their bilinear upscale is a phone's. They pin nothing else, and
/// keep their frames few, for the CPU rasterizers.
///
/// The display text scenes (M3 U8) draw a title in the display face at four sizes over the HUD scenes'
/// night street, pinned as those are, the smaller two from the glyph pages and the larger two from the
/// face's cooked field: Oxanium at one pixel to the point, and Dela Gothic One at three, as a phone's
/// capture is. They alone run `--display-text`, and they draw the interface as the HUD scenes do.
pub fn scenes() -> [Scene; 253] {
    [
        Scene {
            name: "turntable-start",
            args: &[
                "--capture-ticks",
                "0",
                "--plain-post",
                "--plain-sky",
                "--aa",
                "msaa",
                "--shadows",
                "off",
                "--fog",
                "off",
                "--ao",
                "off",
            ],
            thresholds: Thresholds::default(),
        },
        Scene {
            name: "turntable",
            args: &[
                "--capture-ticks",
                "90",
                "--plain-post",
                "--plain-sky",
                "--aa",
                "msaa",
                "--shadows",
                "off",
                "--fog",
                "off",
                "--ao",
                "off",
            ],
            thresholds: Thresholds::default(),
        },
        // Without MSAA or TAA every outline and terminator edge is a hard pixel step, the case
        // most sensitive to precision differences.
        Scene {
            name: "turntable-msaa-off",
            args: &[
                "--capture-ticks",
                "90",
                "--plain-post",
                "--aa",
                "off",
                "--plain-sky",
                "--shadows",
                "off",
                "--fog",
                "off",
                "--ao",
                "off",
            ],
            thresholds: Thresholds::default(),
        },
        // Three colored local lights through the clustered path (M1 R5): their splits on the
        // head and arm, and a spot's pool on the pedestal.
        Scene {
            name: "turntable-lights",
            args: &[
                "--capture-ticks",
                "90",
                "--plain-post",
                "--lights",
                "--plain-sky",
                "--aa",
                "msaa",
                "--shadows",
                "off",
                "--fog",
                "off",
                "--ao",
                "off",
            ],
            thresholds: Thresholds::default(),
        },
        // The scene at render scale 0.7 (448×252), upsampled by post's final pass (M1 R6).
        Scene {
            name: "turntable-render-scale",
            args: &[
                "--capture-ticks",
                "90",
                "--plain-post",
                "--render-scale",
                "0.7",
                "--plain-sky",
                "--aa",
                "msaa",
                "--shadows",
                "off",
                "--fog",
                "off",
                "--ao",
                "off",
            ],
            thresholds: Thresholds::default(),
        },
        // The procedural stalk beside the character (M1 R2): compute skinning bends it and its
        // two morph targets swell its top and narrow its middle, drawn from its skinning outputs
        // with shading and outline.
        Scene {
            name: "turntable-skinned",
            args: &[
                "--capture-ticks",
                "90",
                "--plain-post",
                "--skinned",
                "--plain-sky",
                "--aa",
                "msaa",
                "--shadows",
                "off",
                "--fog",
                "off",
                "--ao",
                "off",
            ],
            thresholds: Thresholds::default(),
        },
        // Sunset from the default day (M1 W1), between its golden-hour and sunset keys: a low
        // orange key light from the right, the sky's horizon band, violet-tinted shade and
        // post's two grading LUTs blended (80% toward sunset's).
        Scene {
            name: "turntable-sunset",
            args: &[
                "--capture-ticks",
                "90",
                "--plain-post",
                "--time-of-day",
                "18.5",
                "--plain-sky",
                "--aa",
                "msaa",
                "--shadows",
                "off",
                "--fog",
                "off",
                "--ao",
                "off",
            ],
            thresholds: Thresholds::default(),
        },
        // The anime character shader swatches beside the character (M1 C1): ramp mode over the
        // cooked swatch ramp and control map (per-ID rows, a striped shadow bias, a painted
        // shadow), the same with a subsurface mix, and three bands in per-ID colors from vertex
        // material IDs.
        Scene {
            name: "turntable-swatches",
            args: &[
                "--capture-ticks",
                "90",
                "--plain-post",
                "--swatches",
                "--plain-sky",
                "--aa",
                "msaa",
                "--shadows",
                "off",
                "--fog",
                "off",
                "--ao",
                "off",
            ],
            thresholds: Thresholds::default(),
        },
        // Night from the default day (M1 C2), halfway from dusk to midnight: the character lit by
        // its light rig's key (time of day's character key, brighter than the world's moon, from
        // the moon high on the camera's left) and stylized GI under a deep blue ambient, against
        // the night sky and two blended grading LUTs.
        Scene {
            name: "turntable-night",
            args: &[
                "--capture-ticks",
                "90",
                "--plain-post",
                "--time-of-day",
                "22",
                "--plain-sky",
                "--aa",
                "msaa",
                "--shadows",
                "off",
                "--fog",
                "off",
                "--ao",
                "off",
            ],
            thresholds: Thresholds::default(),
        },
        // The face look-dev rig (M1 C3): the cooked face test head in two rows, each lit by the
        // key light from its own angle in head space. A yaw sweep from 135° on the heads' right
        // round to 135° on their left (mirror-image SDF shadows, eyes and brows lit), then a top
        // light, light from above and to the side, a blush, and a gloom with a blush.
        Scene {
            name: "faces",
            args: &[
                "--capture-ticks",
                "90",
                "--plain-post",
                "--faces",
                "--plain-sky",
                "--aa",
                "msaa",
                "--shadows",
                "off",
                "--fog",
                "off",
                "--ao",
                "off",
            ],
            thresholds: Thresholds::default(),
        },
        // The sky by day (M1 W2), looking up past the character toward the morning sun: its
        // white-hot disc and wide glow, cirrus streaks overhead, and the card clouds' band on the
        // horizon, two-tone (white tops over lavender shade) with a rim, at 9:00's coverage.
        Scene {
            name: "sky-day",
            args: &[
                "--capture-ticks",
                "90",
                "--plain-post",
                "--time-of-day",
                "9",
                "--view",
                "99,-22",
                "--aa",
                "msaa",
                "--shadows",
                "off",
                "--fog",
                "off",
                "--ao",
                "off",
            ],
            thresholds: Thresholds::default(),
        },
        // Sunset (M1 W2): the sun low in a gap of the card clouds, its glow spread along the
        // horizon, gold-lit clouds over violet shade with bright rims toward the sun, and cirrus
        // catching the color.
        Scene {
            name: "sky-sunset",
            args: &[
                "--capture-ticks",
                "90",
                "--plain-post",
                "--time-of-day",
                "18.5",
                "--view",
                "-100,-6",
                "--aa",
                "msaa",
                "--shadows",
                "off",
                "--fog",
                "off",
                "--ao",
                "off",
            ],
            thresholds: Thresholds::default(),
        },
        // Night (M1 W2): the moon's disc and halo, procedural stars and the Milky Way on the
        // turning celestial sphere, and dark clouds with moonlit rims.
        Scene {
            name: "sky-night",
            args: &[
                "--capture-ticks",
                "90",
                "--plain-post",
                "--time-of-day",
                "21",
                "--view",
                "112,-20",
                "--aa",
                "msaa",
                "--shadows",
                "off",
                "--fog",
                "off",
                "--ao",
                "off",
            ],
            thresholds: Thresholds::default(),
        },
        // Temporal AA (M1 R7), the desktop tiers' antialiasing: the turntable held still while
        // 32 jittered frames converge, so outlines and light splits are antialiased as 4× MSAA
        // draws them.
        Scene {
            name: "turntable-taa",
            args: &[
                "--capture-ticks",
                "90",
                "--plain-post",
                "--aa",
                "taa",
                "--capture-frames",
                "32",
                "--plain-sky",
                "--shadows",
                "off",
                "--fog",
                "off",
                "--ao",
                "off",
            ],
            thresholds: Thresholds { mean: TAA_RENDER_SCALE_MEAN, ..Thresholds::default() },
        },
        // The turntable spinning at 3 rad/s, about 8 times its usual speed, for the 32 frames up
        // to the capture: outlines and two-tone edges must follow without ghost trails or smears.
        Scene {
            name: "turntable-taa-motion",
            args: &[
                "--capture-ticks",
                "90",
                "--plain-post",
                "--aa",
                "taa",
                "--spin",
                "3",
                "--capture-frames",
                "32",
                "--capture-step",
                "1",
                "--plain-sky",
                "--shadows",
                "off",
                "--fog",
                "off",
                "--ao",
                "off",
            ],
            thresholds: Thresholds::default(),
        },
        // Temporal upsampling: the scene at two thirds of the output's size (427×240), resolved
        // to 640×360 over 32 still frames.
        Scene {
            name: "turntable-taa-upsampled",
            args: &[
                "--capture-ticks",
                "90",
                "--plain-post",
                "--aa",
                "taa",
                "--render-scale",
                "0.667",
                "--capture-frames",
                "32",
                "--plain-sky",
                "--shadows",
                "off",
                "--fog",
                "off",
                "--ao",
                "off",
            ],
            thresholds: Thresholds { mean: TAA_RENDER_SCALE_MEAN, ..Thresholds::default() },
        },
        // The environment shader (M1 W4) by day: the character in the courtyard, under the morning
        // sun from the front left. Painted ground, a colonnade receding into rolling hills, a
        // plaster wall and boulders: soft wrapped light, lavender shadow sides with a warm band
        // along the pillars' terminators, terminators softening and colors greying with
        // distance, and the character reading on top.
        Scene {
            name: "environment-day",
            args: &[
                "--capture-ticks",
                "90",
                "--plain-post",
                "--environment",
                "--aa",
                "taa",
                "--capture-frames",
                "32",
                "--shadows",
                "off",
                "--fog",
                "off",
                "--ao",
                "off",
            ],
            thresholds: Thresholds::default(),
        },
        // Sunset (18:30): the low orange sun from the right warms every lit face, the shadow
        // sides keep time of day's violet tint, and the lanterns begin to glow.
        Scene {
            name: "environment-sunset",
            args: &[
                "--capture-ticks",
                "90",
                "--plain-post",
                "--environment",
                "--time-of-day",
                "18.5",
                "--aa",
                "taa",
                "--capture-frames",
                "32",
                "--shadows",
                "off",
                "--fog",
                "off",
                "--ao",
                "off",
            ],
            thresholds: Thresholds::default(),
        },
        // Night (22:00): the world under the moon and a deep blue ambient, the lanterns glowing
        // orange, and the demo lights' pools on the ground and the pedestal (local lights through
        // both families' shaders), the character lit by its own rig's brighter key.
        Scene {
            name: "environment-night",
            args: &[
                "--capture-ticks",
                "90",
                "--plain-post",
                "--environment",
                "--time-of-day",
                "22",
                "--lights",
                "--aa",
                "taa",
                "--capture-frames",
                "32",
                "--shadows",
                "off",
                "--fog",
                "off",
                "--ao",
                "off",
            ],
            thresholds: Thresholds::default(),
        },
        // Post's compositing layers by day (M1 P2), through temporal AA as the desktop tiers draw:
        // the diffusion glow softening the clouds, the top of the frame deepened toward the sky's
        // blue, the ground's shade toward the bottom, a soft pool of light on the character, whose
        // own bloom is held back (the character mask).
        Scene {
            name: "post-day",
            args: &[
                "--capture-ticks",
                "90",
                "--time-of-day",
                "10",
                "--aa",
                "taa",
                "--capture-frames",
                "32",
                "--shadows",
                "off",
                "--fog",
                "off",
                "--ao",
                "off",
            ],
            thresholds: Thresholds::default(),
        },
        // Sunset through Mobile-Low's fused pass (4× MSAA, four bloom levels): a warm, dreamy
        // glow over the clouds and along the horizon, violet from above, the ground in a deep
        // violet shade below, the character crisp in front of it.
        Scene {
            name: "post-sunset",
            args: &[
                "--capture-ticks",
                "90",
                "--time-of-day",
                "18.5",
                "--tier",
                "mobile-low",
                "--shadows",
                "off",
                "--fog",
                "off",
                "--ao",
                "off",
            ],
            thresholds: Thresholds::default(),
        },
        // The lens flare: the sunset sun in view beside the character, its warm ring and streak,
        // colored ghosts down the axis through the middle of the frame, and the diffusion glow
        // around the sun.
        Scene {
            name: "post-flare",
            args: &[
                "--capture-ticks",
                "90",
                "--time-of-day",
                "18.5",
                "--view",
                "-100,-6",
                "--aa",
                "msaa",
                "--shadows",
                "off",
                "--fog",
                "off",
                "--ao",
                "off",
            ],
            thresholds: Thresholds::default(),
        },
        // Outline width over distance (M1 C4): more of the character from 4 m to 90 m behind it,
        // seen from nearly level. Their lines thin along the width's two-segment curve, hold
        // their width in the world far away, and are gone on the last, past the LOD cutoff.
        Scene {
            name: "outlines-distance",
            args: &[
                "--capture-ticks",
                "90",
                "--outline-row",
                "--plain-post",
                "--plain-sky",
                "--aa",
                "msaa",
                "--shadows",
                "off",
                "--fog",
                "off",
                "--ao",
                "off",
            ],
            thresholds: Thresholds::default(),
        },
        // The same row through a 20° lens from farther away, the character framed as before:
        // its lines keep their width, and the row's, nearer in effective distance, widen.
        Scene {
            name: "outlines-telephoto",
            args: &[
                "--capture-ticks",
                "90",
                "--outline-row",
                "--fov",
                "20",
                "--plain-post",
                "--plain-sky",
                "--aa",
                "msaa",
                "--shadows",
                "off",
                "--fog",
                "off",
                "--ao",
                "off",
            ],
            thresholds: Thresholds::default(),
        },
        // The hard-edged sample (M1 C4): flat-shaded shapes with every edge split, whose cooked
        // smoothed outline normals keep each corner's hull closed.
        Scene {
            name: "outlines-hard-edges",
            args: &[
                "--capture-ticks",
                "90",
                "--hard-edges",
                "--plain-post",
                "--plain-sky",
                "--aa",
                "msaa",
                "--shadows",
                "off",
                "--fog",
                "off",
                "--ao",
                "off",
            ],
            thresholds: Thresholds::default(),
        },
        // A close-up of the character turning at 3 rad/s under temporal AA (M1 C4): its hulls
        // write their line's velocity in the prepass, the silhouette's (M1 P1), and moving lines
        // resolve from the current frame, so they stay as crisp as 4× MSAA draws them.
        Scene {
            name: "outlines-turning-taa",
            args: &[
                "--capture-ticks",
                "90",
                "--zoom",
                "0.6",
                "--aa",
                "taa",
                "--spin",
                "3",
                "--capture-frames",
                "32",
                "--capture-step",
                "1",
                "--plain-post",
                "--plain-sky",
                "--shadows",
                "off",
                "--fog",
                "off",
                "--ao",
                "off",
            ],
            thresholds: Thresholds::default(),
        },
        // The same close-up upsampled (M1 P1): the scene at two thirds of the output's size
        // (427×240 to 640×360), as Desktop-High draws 4K from 1440 rows. The turning lines keep
        // their width in output pixels and their darkness.
        Scene {
            name: "outlines-turning-taa-upsampled",
            args: &[
                "--capture-ticks",
                "90",
                "--zoom",
                "0.6",
                "--aa",
                "taa",
                "--render-scale",
                "0.667",
                "--spin",
                "3",
                "--capture-frames",
                "32",
                "--capture-step",
                "1",
                "--plain-post",
                "--plain-sky",
                "--shadows",
                "off",
                "--fog",
                "off",
                "--ao",
                "off",
            ],
            thresholds: Thresholds::default(),
        },
        // Shadows (M1 R4): the character on the stage's pedestal, split off as environment, on
        // Desktop-High at 10:00. Its own map's contact shadow falls on the pedestal around its
        // feet and its shadow runs on across the floor, the pedestal casts its own through the
        // cascades, and the figure shades without a mark of its own shadow: characters never
        // shadow themselves. Converged under temporal AA.
        Scene {
            name: "shadows-contact",
            args: &[
                "--capture-ticks",
                "90",
                "--plain-post",
                "--stage",
                "floor",
                "--tier",
                "desktop-high",
                "--aa",
                "taa",
                "--capture-frames",
                "32",
                "--plain-sky",
                "--fog",
                "off",
                "--ao",
                "off",
            ],
            thresholds: Thresholds::default(),
        },
        // A pillar between the sun and the character: its shadow covers the pedestal and the
        // character from the feet to the chest, a clean edge across the body in the character's
        // authored shade tones, with no acne on the pillar, the floor or the figure.
        Scene {
            name: "shadows-pillar",
            args: &[
                "--capture-ticks",
                "90",
                "--plain-post",
                "--stage",
                "pillar",
                "--tier",
                "desktop-high",
                "--aa",
                "taa",
                "--capture-frames",
                "32",
                "--plain-sky",
                "--fog",
                "off",
                "--ao",
                "off",
            ],
            thresholds: Thresholds::default(),
        },
        // The face rig (M1 C3) in a pillar's shadow: the faces it covers go into their authored
        // shadow tones whole, eyes and brows with them, while the lit ones keep their face-map
        // shadows; no shadow-map texels on any face.
        Scene {
            name: "shadows-faces",
            args: &[
                "--capture-ticks",
                "90",
                "--plain-post",
                "--faces",
                "--stage",
                "pillar",
                "--time-of-day",
                "14",
                "--tier",
                "desktop-high",
                "--aa",
                "taa",
                "--capture-frames",
                "32",
                "--plain-sky",
                "--fog",
                "off",
                "--ao",
                "off",
            ],
            thresholds: Thresholds::default(),
        },
        // The courtyard (W4) by day with shadows: the colonnade, the wall and the boulders cast
        // through the cascades onto the ground and each other, the character's own map casts on
        // the ground beyond its pedestal, and in every shadow the ground keeps time of day's
        // tint of the sun through the environment shader's terminator.
        Scene {
            name: "shadows-courtyard",
            args: &[
                "--capture-ticks",
                "90",
                "--plain-post",
                "--environment",
                "--tier",
                "desktop-high",
                "--aa",
                "taa",
                "--capture-frames",
                "32",
                "--fog",
                "off",
                "--ao",
                "off",
            ],
            thresholds: Thresholds::default(),
        },
        // Toward sunset (18:00) in the courtyard: the low sun from the right draws the pillars' and
        // the lanterns' shadows long across the ground into the far cascades, tinted, their edges
        // clean and still. (By 18:30 the wall's shadow already covers the colonnade.)
        Scene {
            name: "shadows-sunset",
            args: &[
                "--capture-ticks",
                "90",
                "--plain-post",
                "--environment",
                "--time-of-day",
                "18",
                "--tier",
                "desktop-high",
                "--aa",
                "taa",
                "--capture-frames",
                "32",
                "--fog",
                "off",
                "--ao",
                "off",
            ],
            thresholds: Thresholds::default(),
        },
        // Mobile-Low's shadows: two cascades, the 3 × 3 filter and 4× MSAA, and the character's
        // capsule instead of a map of its own, a soft blob around its feet on the pedestal running
        // on across the floor.
        Scene {
            name: "shadows-capsule",
            args: &[
                "--capture-ticks",
                "90",
                "--plain-post",
                "--stage",
                "floor",
                "--tier",
                "mobile-low",
                "--plain-sky",
                "--fog",
                "off",
                "--ao",
                "off",
            ],
            thresholds: Thresholds::default(),
        },
        // A dash's screen effects (M1 P3): the radial blur streaking the frame toward its middle
        // and white speed lines radiating from outside a clear middle, which the character stays
        // in front of. Through temporal AA, still, on the default day.
        Scene {
            name: "effects-blur-lines",
            args: &[
                "--capture-ticks",
                "90",
                "--post-effect",
                "radial_blur@0.05",
                "--post-effect",
                "speed_lines@0.3",
                "--aa",
                "taa",
                "--capture-frames",
                "32",
                "--fog",
                "off",
                "--ao",
                "off",
            ],
            thresholds: Thresholds::default(),
        },
        // The impact frame (M1 P3): the world in a dark-and-cream duotone with black speed lines
        // behind the character, which keeps its own colors through the character mask, under the
        // radial blur and chromatic aberration.
        Scene {
            name: "effects-impact-frame",
            args: &[
                "--capture-ticks",
                "90",
                "--post-effect",
                "impact_frame@0.03",
                "--aa",
                "taa",
                "--capture-frames",
                "32",
                "--fog",
                "off",
                "--ao",
                "off",
            ],
            thresholds: Thresholds::default(),
        },
        // Chromatic aberration and the inversion through Mobile-Low's settings (M1 P3): red and
        // blue fringes growing toward the corners of the inverted frame.
        Scene {
            name: "effects-chromatic-inversion",
            args: &[
                "--capture-ticks",
                "90",
                "--post-effect",
                "chromatic_aberration@0.02",
                "--post-effect",
                "inversion@0.02",
                "--tier",
                "mobile-low",
                "--fog",
                "off",
                "--ao",
                "off",
            ],
            thresholds: Thresholds::default(),
        },
        // Fog (M1 W3) in the courtyard by day: a light haze, the hills melting into the sky's
        // horizon without a seam where they meet it, the zenith clear, and wisps of mist (fog
        // cards) beyond the wall and among the far pillars, soft where they meet the ground.
        Scene {
            name: "fog-day",
            args: &[
                "--capture-ticks",
                "90",
                "--plain-post",
                "--environment",
                "--aa",
                "taa",
                "--capture-frames",
                "32",
                "--shadows",
                "off",
                "--ao",
                "off",
            ],
            thresholds: Thresholds::default(),
        },
        // At sunset: warm haze over violet near fog, the far layer taking the orange horizon, the
        // mist lit by the hour's fog.
        Scene {
            name: "fog-sunset",
            args: &[
                "--capture-ticks",
                "90",
                "--plain-post",
                "--environment",
                "--time-of-day",
                "18.5",
                "--aa",
                "taa",
                "--capture-frames",
                "32",
                "--shadows",
                "off",
                "--ao",
                "off",
            ],
            thresholds: Thresholds::default(),
        },
        // At night, with the lanterns' lights: a deep blue haze along the horizon that leaves the
        // stars and the clouds above it, the moon lighting it faintly.
        Scene {
            name: "fog-night",
            args: &[
                "--capture-ticks",
                "90",
                "--plain-post",
                "--environment",
                "--time-of-day",
                "22",
                "--lights",
                "--aa",
                "taa",
                "--capture-frames",
                "32",
                "--shadows",
                "off",
                "--ao",
                "off",
            ],
            thresholds: Thresholds::default(),
        },
        // Sunrise's morning mist, from higher and wider: height fog thickest on the ground,
        // swallowing the hills' feet while their tops rise out of it.
        Scene {
            name: "fog-valley",
            args: &[
                "--capture-ticks",
                "90",
                "--plain-post",
                "--environment",
                "--time-of-day",
                "6.5",
                "--view",
                "20,14",
                "--fov",
                "60",
                "--aa",
                "taa",
                "--capture-frames",
                "32",
                "--shadows",
                "off",
                "--ao",
                "off",
            ],
            thresholds: Thresholds::default(),
        },
        // The fog cards' vista in the early morning: the mist sheets across the valley and the
        // haze among the pillars, faded where they cut the ground (the desktop tiers' depth fade,
        // under temporal AA).
        Scene {
            name: "fog-cards",
            args: &[
                "--capture-ticks",
                "90",
                "--plain-post",
                "--environment",
                "--time-of-day",
                "7.5",
                "--view",
                "15,4",
                "--aa",
                "taa",
                "--capture-frames",
                "32",
                "--shadows",
                "off",
                "--ao",
                "off",
            ],
            thresholds: Thresholds::default(),
        },
        // The same on Mobile-Low (4× MSAA, no prepass): the cards without their depth fade, the
        // map's soft edges and the camera fade carrying them, and the fog as on desktop.
        Scene {
            name: "fog-cards-mobile",
            args: &[
                "--capture-ticks",
                "90",
                "--plain-post",
                "--environment",
                "--time-of-day",
                "7.5",
                "--view",
                "15,4",
                "--tier",
                "mobile-low",
                "--shadows",
                "off",
                "--ao",
                "off",
            ],
            thresholds: Thresholds::default(),
        },
        // Characters standing in fog (the outline row, from 4 m to 90 m, on the stage's floor)
        // in sunrise's mist: the floor melts into the horizon, while the characters, taking less
        // of the fog than the world, still read as figures far out.
        Scene {
            name: "fog-characters",
            args: &[
                "--capture-ticks",
                "90",
                "--plain-post",
                "--stage",
                "floor",
                "--outline-row",
                "--time-of-day",
                "6.5",
                "--aa",
                "taa",
                "--capture-frames",
                "32",
                "--shadows",
                "off",
                "--ao",
                "off",
            ],
            thresholds: Thresholds::default(),
        },
        // The courtyard's arcade lit by its probe grid (M1 W5) in the morning: the gateway on the
        // left, the loggia's roofed walk on the right, the character between. Sunlit grass
        // bounces green onto the faces turned to it, the terracotta walls warm the loggia's floor
        // and back wall in the roof's shade, and the sky seen past the roof and the arches lights
        // what it reaches; GTAO darkens the creases at the walls' feet.
        Scene {
            name: "ambient-day",
            args: &[
                "--capture-ticks",
                "90",
                "--plain-post",
                "--probes",
                "--tier",
                "desktop-high",
                "--aa",
                "taa",
                "--capture-frames",
                "32",
                "--fog",
                "off",
            ],
            thresholds: Thresholds::default(),
        },
        // Sunset (18:30): the bounce blends the grid's 17:30 and 18:45 keys, orange off the lit
        // walls into the shade, and the sky's part follows the evening sky every frame.
        Scene {
            name: "ambient-sunset",
            args: &[
                "--capture-ticks",
                "90",
                "--plain-post",
                "--probes",
                "--time-of-day",
                "18.5",
                "--tier",
                "desktop-high",
                "--aa",
                "taa",
                "--capture-frames",
                "32",
                "--fog",
                "off",
            ],
            thresholds: Thresholds::default(),
        },
        // Night (22:00): the moon's faint bounce, blended from the 20:00 and midnight keys, and
        // the night sky's blue through the arches and under the roof.
        Scene {
            name: "ambient-night",
            args: &[
                "--capture-ticks",
                "90",
                "--plain-post",
                "--probes",
                "--time-of-day",
                "22",
                "--tier",
                "desktop-high",
                "--aa",
                "taa",
                "--capture-frames",
                "32",
                "--fog",
                "off",
            ],
            thresholds: Thresholds::default(),
        },
        // GTAO close up, through a 12° lens in the afternoon (16:00): the corners where the
        // gateway, the loggia's plinth and its pillars meet the ground and each other, under the
        // arch and the roof, denoised and averaged by temporal AA into soft, still creases.
        Scene {
            name: "ambient-gtao",
            args: &[
                "--capture-ticks",
                "90",
                "--plain-post",
                "--probes",
                "--time-of-day",
                "16",
                "--fov",
                "12",
                "--tier",
                "desktop-high",
                "--aa",
                "taa",
                "--capture-frames",
                "32",
                "--fog",
                "off",
            ],
            thresholds: Thresholds::default(),
        },
        // Capsule AO under the character, seen from above: on a stone plinth (an environment
        // material) at night, where the ambient light is most of the light, the body's capsule
        // darkens the stone around the feet, softly and only nearby. GTAO off, so the capsule's
        // term shows alone.
        Scene {
            name: "ambient-capsule",
            args: &[
                "--capture-ticks",
                "90",
                "--plain-post",
                "--stage",
                "plinth",
                "--time-of-day",
                "22",
                "--view",
                "20,55",
                "--zoom",
                "0.7",
                "--tier",
                "desktop-high",
                "--aa",
                "taa",
                "--capture-frames",
                "32",
                "--plain-sky",
                "--ao",
                "capsule",
                "--fog",
                "off",
            ],
            thresholds: Thresholds::default(),
        },
        // Mobile-High's ambient: the arcade's grid with one GTAO slice, no temporal AA to average
        // its pattern (the denoise alone smooths it), 4× MSAA and the approximate specular.
        Scene {
            name: "ambient-mobile",
            args: &[
                "--capture-ticks",
                "90",
                "--plain-post",
                "--probes",
                "--tier",
                "mobile-high",
                "--fog",
                "off",
            ],
            thresholds: Thresholds::default(),
        },
        // A zone's own look (M3 C1): the hub and the last room, as a walk would come on them, with its
        // grid, its day and its rigs, and every feature on, as what a game would ship.
        Scene {
            name: "zone-ward-hub",
            args: &[
                "--capture-ticks",
                "90",
                "--zone",
                "lantern_ward:player_start",
                "--time-of-day",
                "14",
                "--tier",
                "desktop-high",
                "--capture-frames",
                "32",
            ],
            thresholds: Thresholds::default(),
        },
        // The second enemy (M3 C2) in the Lantern Ward's Market Court, whose encounter has brought a warrior and
        // a Lancer: the Lancer coming down out of its vault, spear first, each drawn from its own kind's look.
        Scene {
            name: "fight-ward-market",
            args: &[
                "--capture-ticks",
                "228",
                "--cues",
                "--hud",
                "live",
                "--arena",
                "--replay",
                "tests/replays/ward-market.sreplay",
                "--time-of-day",
                "10",
                "--tier",
                "desktop-high",
                "--ao",
                "off",
                "--blade",
                "on",
                "--capture-frames",
                "32",
            ],
            thresholds: Thresholds::default(),
        },
        // The ward's boss (M3 C2) on its terrace, a moment after its second wind's toll threw the character back,
        // its bar past the mark at half. A replay of a scenario names it, so nothing in code does.
        Scene {
            name: "fight-ward-terrace",
            args: &[
                "--capture-ticks",
                "1799",
                "--cues",
                "--hud",
                "live",
                "--arena",
                "--replay",
                "tests/replays/ward-terrace.sreplay",
                "--time-of-day",
                "17.5",
                "--tier",
                "desktop-high",
                "--ao",
                "off",
                "--blade",
                "on",
                "--capture-frames",
                "32",
            ],
            thresholds: Thresholds::default(),
        },
        Scene {
            name: "zone-ward-court",
            args: &[
                "--capture-ticks",
                "90",
                "--zone",
                "lantern_ward:checkpoint_court",
                "--time-of-day",
                "17.5",
                "--tier",
                "desktop-high",
                "--capture-frames",
                "32",
            ],
            thresholds: Thresholds::default(),
        },
        // The depth-offset rim (M1 C5): figures before dark and light panels, keys from the front,
        // the side, behind to the side and across, so the light gate shows on the shadow sides;
        // the arms keep theirs off the torso (their region's overlap).
        Scene {
            name: "rim-angles",
            args: &[
                "--capture-ticks",
                "90",
                "--rim-rig",
                "angles",
                "--plain-post",
                "--plain-sky",
                "--aa",
                "msaa",
                "--shadows",
                "off",
                "--fog",
                "off",
                "--ao",
                "off",
            ],
            thresholds: Thresholds::default(),
        },
        // Back-lit, where ZZZ's back-light weighting strengthens the outfit's rim.
        Scene {
            name: "rim-backlit",
            args: &[
                "--capture-ticks",
                "90",
                "--rim-rig",
                "backlit",
                "--plain-post",
                "--plain-sky",
                "--aa",
                "msaa",
                "--shadows",
                "off",
                "--fog",
                "off",
                "--ao",
                "off",
            ],
            thresholds: Thresholds::default(),
        },
        // The cap's angel ring holding its height on screen as heads nod, and the bangs' painted
        // band.
        Scene {
            name: "hair-highlights",
            args: &[
                "--capture-ticks",
                "90",
                "--faces",
                "--face-layout",
                "highlights",
                "--plain-post",
                "--plain-sky",
                "--aa",
                "msaa",
                "--shadows",
                "off",
                "--fog",
                "off",
                "--ao",
                "off",
            ],
            thresholds: Thresholds::default(),
        },
        // The bangs' shadow on faces, moving away from keys round the front and above.
        Scene {
            name: "hair-shadow",
            args: &[
                "--capture-ticks",
                "90",
                "--faces",
                "--face-layout",
                "hair",
                "--plain-post",
                "--plain-sky",
                "--aa",
                "msaa",
                "--shadows",
                "off",
                "--fog",
                "off",
                "--ao",
                "off",
            ],
            thresholds: Thresholds::default(),
        },
        // The same on Mobile-Low, whose character prepass is a render pass of its own before the
        // main pass's tile-memory MSAA.
        Scene {
            name: "hair-shadow-mobile",
            args: &[
                "--capture-ticks",
                "90",
                "--faces",
                "--face-layout",
                "hair",
                "--tier",
                "mobile-low",
                "--plain-post",
                "--plain-sky",
                "--aa",
                "msaa",
                "--shadows",
                "off",
                "--fog",
                "off",
                "--ao",
                "off",
            ],
            thresholds: Thresholds::default(),
        },
        // Eyes and brows through the bangs, front to three-quarter and past the fade, and nodding.
        Scene {
            name: "eyes-through-hair",
            args: &[
                "--capture-ticks",
                "90",
                "--faces",
                "--face-layout",
                "turn",
                "--plain-post",
                "--plain-sky",
                "--aa",
                "msaa",
                "--shadows",
                "off",
                "--fog",
                "off",
                "--ao",
                "off",
            ],
            thresholds: Thresholds::default(),
        },
        // Foliage (M1 W6) by day: two trees, bushes and grass in the courtyard around the
        // character, in a breeze held still. Canopies light as soft masses over their leaf cards,
        // deep teal inside to yellow-green on top, under the same lavender shade as the world;
        // the trees' shadows on the ground are leafy, their cards cut out; the grass lights like
        // the ground it stands on, its patches fading out at their edges.
        Scene {
            name: "foliage-day",
            args: &[
                "--capture-ticks",
                "90",
                "--plain-post",
                "--fog",
                "off",
                "--ao",
                "off",
                "--foliage",
                "grove",
                "--tier",
                "desktop-high",
                "--aa",
                "taa",
                "--capture-frames",
                "32",
            ],
            thresholds: Thresholds::default(),
        },
        // Sunset (18:30) through the big tree's crown, from its far side: the low sun behind the
        // canopy glows through its outer leaves (translucency), warm against the pink sky, while
        // leaves in the canopy's own shadow stay dark.
        Scene {
            name: "foliage-backlit",
            args: &[
                "--capture-ticks",
                "90",
                "--plain-post",
                "--fog",
                "off",
                "--ao",
                "off",
                "--foliage",
                "backlit",
                "--time-of-day",
                "18.5",
                "--tier",
                "desktop-high",
                "--aa",
                "taa",
                "--capture-frames",
                "32",
            ],
            thresholds: Thresholds::default(),
        },
        // Low over the grass beside a bush, the character beyond: blades lighting like the ground,
        // darker at their roots, bending at their two rows; the bush's cards a soft dome.
        Scene {
            name: "foliage-ground",
            args: &[
                "--capture-ticks",
                "90",
                "--plain-post",
                "--fog",
                "off",
                "--ao",
                "off",
                "--foliage",
                "ground",
                "--tier",
                "desktop-high",
                "--aa",
                "taa",
                "--capture-frames",
                "32",
            ],
            thresholds: Thresholds::default(),
        },
        // Mobile-Low's foliage: cards' edges through alpha-to-coverage under 4× MSAA, two wind
        // levels (no leaf flutter) and no translucency, two shadow cascades.
        Scene {
            name: "foliage-mobile",
            args: &[
                "--capture-ticks",
                "90",
                "--plain-post",
                "--fog",
                "off",
                "--ao",
                "off",
                "--foliage",
                "grove",
                "--tier",
                "mobile-low",
            ],
            thresholds: Thresholds::default(),
        },
        // A stiff wind (2.5 times the breeze) playing for the 32 frames up to tick 150: trunks
        // leaning, branches swinging and leaves fluttering, the frame resolved by temporal AA from
        // velocity that includes the wind, without ghosting or smears.
        Scene {
            name: "foliage-wind",
            args: &[
                "--capture-ticks",
                "150",
                "--plain-post",
                "--fog",
                "off",
                "--ao",
                "off",
                "--foliage",
                "grove",
                "--wind",
                "2.5",
                "--tier",
                "desktop-high",
                "--aa",
                "taa",
                "--capture-frames",
                "32",
                "--capture-step",
                "1",
            ],
            thresholds: Thresholds::default(),
        },
        // A character at night under the whole look (the night character light fix): the
        // courtyard at 23:00, nearer the mannequin, with the moon's shadows, the fog and post's
        // layers. Its key is time of day's character key, a little brighter than the moon (about
        // 1.25 times in luminance at this hour), so the mannequin reads a step above the pillars
        // and the wall in a cool, dim light, clearly at night, where a key set apart from the
        // world's kept it at its noon brightness.
        Scene {
            name: "night-character",
            args: &[
                "--capture-ticks",
                "90",
                "--environment",
                "--time-of-day",
                "23",
                "--ao",
                "off",
                "--zoom",
                "0.6",
                "--tier",
                "desktop-high",
                "--aa",
                "taa",
                "--capture-frames",
                "32",
            ],
            thresholds: Thresholds::default(),
        },
        // The look-dev town (M1 X1) at midday, the wide shot across the square: the town in its
        // environment materials and its plants under the whole sky, fog and fog cards, shadows,
        // post's layers and the character's outlines, the VRM character standing in the square;
        // the stage's own day's side-lit noon, cool shadows and blue-grey distance (X1 part 2).
        Scene {
            name: "lookdev-town-day",
            args: &[
                "--capture-ticks",
                "90",
                "--look-dev",
                "wide",
                "--time-of-day",
                "13",
                "--tier",
                "desktop-high",
                "--ao",
                "off",
                "--capture-frames",
                "32",
            ],
            thresholds: Thresholds::default(),
        },
        // The hero shot at golden hour: the character in three-quarter view with the SDF face map
        // and outlines, the pavilion behind the character with the low sun's glow round its roof, the gate
        // wall and the lantern-lit street through its moon gate (X1 part 2b).
        Scene {
            name: "lookdev-town-hero",
            args: &[
                "--capture-ticks",
                "90",
                "--look-dev",
                "hero",
                "--time-of-day",
                "18",
                "--tier",
                "desktop-high",
                "--ao",
                "off",
                "--capture-frames",
                "32",
            ],
            thresholds: Thresholds::default(),
        },
        // The wide shot at golden hour (X1 part 2): the low sun's huge glow over the rooftops on
        // the left, warm cloud sides over purple undersides, warm streaks across the square
        // between blue-violet shadows, the mountains in violet haze.
        Scene {
            name: "lookdev-town-golden",
            args: &[
                "--capture-ticks",
                "90",
                "--look-dev",
                "wide",
                "--time-of-day",
                "18.5",
                "--tier",
                "desktop-high",
                "--ao",
                "off",
                "--capture-frames",
                "32",
            ],
            thresholds: Thresholds::default(),
        },
        // The wide shot at the blue hour (X1 part 2): the sun gone, a cyan-blue sky and haze over
        // a blue town, the windows and lanterns coming on in amber, the character's skin warm.
        Scene {
            name: "lookdev-town-blue",
            args: &[
                "--capture-ticks",
                "90",
                "--look-dev",
                "wide",
                "--time-of-day",
                "19.75",
                "--tier",
                "desktop-high",
                "--ao",
                "off",
                "--capture-frames",
                "32",
            ],
            thresholds: Thresholds::default(),
        },
        // The sky shot at night (X1 part 2): up past the character to a navy sky with stars, the
        // teal Milky Way and moonlit lavender clouds over the rooftops, the windows and lanterns
        // amber and blooming, the mid-tones kept readable.
        Scene {
            name: "lookdev-town-night",
            args: &[
                "--capture-ticks",
                "90",
                "--look-dev",
                "sky",
                "--time-of-day",
                "23",
                "--tier",
                "desktop-high",
                "--ao",
                "off",
                "--capture-frames",
                "32",
            ],
            thresholds: Thresholds::default(),
        },
        // The street out of the square at night (X1 part 2b): Jinzhou's white walls and charcoal
        // tile roofs under a blue night, its parametric halls, gateway and tower, the paper
        // lanterns strung across it and on posts glowing amber (the posts' lamps lighting the
        // flagstones), the windows warm.
        Scene {
            name: "lookdev-street-night",
            args: &[
                "--capture-ticks",
                "90",
                "--look-dev",
                "street",
                "--time-of-day",
                "21",
                "--tier",
                "desktop-high",
                "--ao",
                "off",
                "--capture-frames",
                "32",
            ],
            thresholds: Thresholds::default(),
        },
        // The default light rig preset's stylized GI alone (M1 C2) at golden hour, the key light
        // off and the GI raised four-fold: the face rig turned and nodding every way under the
        // amber sky's light above and the violet ground's below, through normals rounded toward
        // the head's sphere (the jaw darkens with it) and bent toward the camera; the face in
        // the skin tint; the bangs' roots and two heads' gloom occluded.
        Scene {
            name: "gi-golden-hour",
            args: &[
                "--capture-ticks",
                "90",
                "--faces",
                "--face-layout",
                "gi",
                "--time-of-day",
                "17.5",
                "--plain-post",
                "--plain-sky",
                "--aa",
                "msaa",
                "--shadows",
                "off",
                "--fog",
                "off",
                "--ao",
                "off",
            ],
            thresholds: Thresholds::default(),
        },
        // The same heads by day, their GI clamped hard (saturation 0.2, value 0.3): the sky's
        // blue and the ground's violet greyed, the sky sides flattened to one value, and the
        // face warmed again by the skin tint.
        Scene {
            name: "gi-clamped",
            args: &[
                "--capture-ticks",
                "90",
                "--faces",
                "--face-layout",
                "gi-clamped",
                "--time-of-day",
                "10",
                "--plain-post",
                "--plain-sky",
                "--aa",
                "msaa",
                "--shadows",
                "off",
                "--fog",
                "off",
                "--ao",
                "off",
            ],
            thresholds: Thresholds::default(),
        },
        // Anime eyes close up and front on (M1 C6): the eye, its shadow, the iris and the highlights,
        // with the highlights at the low of their glint.
        Scene {
            name: "eyes-front",
            args: &[
                "--capture-ticks",
                "180",
                "--faces",
                "--face-layout",
                "eyes",
                "--plain-post",
                "--plain-sky",
                "--aa",
                "msaa",
                "--shadows",
                "off",
                "--fog",
                "off",
                "--ao",
                "off",
            ],
            thresholds: Thresholds::default(),
        },
        // The same turned three-quarter, the iris shifting behind the cornea.
        Scene {
            name: "eyes-three-quarter",
            args: &[
                "--capture-ticks",
                "90",
                "--faces",
                "--face-layout",
                "eyes-turned",
                "--plain-post",
                "--plain-sky",
                "--aa",
                "msaa",
                "--shadows",
                "off",
                "--fog",
                "off",
                "--ao",
                "off",
            ],
            thresholds: Thresholds::default(),
        },
        // Gaze morphs left, ahead and right, then up and down, carrying the iris and highlights.
        Scene {
            name: "eyes-gaze",
            args: &[
                "--capture-ticks",
                "90",
                "--faces",
                "--face-layout",
                "gaze",
                "--plain-post",
                "--plain-sky",
                "--aa",
                "msaa",
                "--shadows",
                "off",
                "--fog",
                "off",
                "--ao",
                "off",
            ],
            thresholds: Thresholds::default(),
        },
        // The highlights at the height of their glint, under post's glow and character bloom.
        Scene {
            name: "eyes-highlight",
            args: &[
                "--capture-ticks",
                "105",
                "--faces",
                "--face-layout",
                "eyes",
                "--plain-sky",
                "--aa",
                "msaa",
                "--shadows",
                "off",
                "--fog",
                "off",
                "--ao",
                "off",
            ],
            thresholds: Thresholds::default(),
        },
        // Lids half and fully closed, the layers going with the eye under them.
        Scene {
            name: "eyes-lids",
            args: &[
                "--capture-ticks",
                "90",
                "--faces",
                "--face-layout",
                "blink",
                "--plain-post",
                "--plain-sky",
                "--aa",
                "msaa",
                "--shadows",
                "off",
                "--fog",
                "off",
                "--ao",
                "off",
            ],
            thresholds: Thresholds::default(),
        },
        // The front close-up on Mobile-Low.
        Scene {
            name: "eyes-mobile",
            args: &[
                "--capture-ticks",
                "180",
                "--faces",
                "--face-layout",
                "eyes",
                "--tier",
                "mobile-low",
                "--plain-post",
                "--plain-sky",
                "--aa",
                "msaa",
                "--shadows",
                "off",
                "--fog",
                "off",
                "--ao",
                "off",
            ],
            thresholds: Thresholds::default(),
        },
        // The debug views (M1 F5), drawn over the frame after post. The wireframe low over the
        // grass beside a bush, the character beyond, under temporal AA: every shading family's
        // edges in its tone (the grass and bush green, the courtyard cyan, the character orange,
        // the morning's fog cards among the pillars violet), hidden ones left out.
        Scene {
            name: "debug-wireframe",
            args: &[
                "--capture-ticks",
                "90",
                "--plain-post",
                "--foliage",
                "ground",
                "--tier",
                "desktop-high",
                "--aa",
                "taa",
                "--capture-frames",
                "32",
                "--shadows",
                "off",
                "--ao",
                "off",
                "--debug-view",
                "wireframe",
            ],
            thresholds: Thresholds::default(),
        },
        // Overdraw in the grove: fragments shaded per pixel, the sky and the ground one each, the
        // trees' cards and the grass stacking up, the fog cards over the courtyard's back, and
        // the character's outline one more around it.
        Scene {
            name: "debug-overdraw",
            args: &[
                "--capture-ticks",
                "90",
                "--plain-post",
                "--foliage",
                "grove",
                "--tier",
                "desktop-high",
                "--aa",
                "taa",
                "--capture-frames",
                "32",
                "--shadows",
                "off",
                "--ao",
                "off",
                "--debug-view",
                "overdraw",
            ],
            thresholds: Thresholds::default(),
        },
        // The light lists (the light-cluster heatmap) with the three colored local lights around
        // the character in the courtyard: each pixel's cluster count, the clusters' tiles and
        // depth slices showing where the lights' ranges reach the ground, the pedestal and the
        // character.
        Scene {
            name: "debug-lights",
            args: &[
                "--capture-ticks",
                "90",
                "--plain-post",
                "--environment",
                "--lights",
                "--aa",
                "taa",
                "--capture-frames",
                "32",
                "--shadows",
                "off",
                "--fog",
                "off",
                "--ao",
                "off",
                "--debug-view",
                "lights",
            ],
            thresholds: Thresholds::default(),
        },
        // The same on Mobile-Low (4× MSAA), whose per-object lists have no clusters: each
        // instance's own list, 0 to 4 lights, over its pixels.
        Scene {
            name: "debug-lights-mobile",
            args: &[
                "--capture-ticks",
                "90",
                "--plain-post",
                "--environment",
                "--lights",
                "--tier",
                "mobile-low",
                "--shadows",
                "off",
                "--fog",
                "off",
                "--ao",
                "off",
                "--debug-view",
                "lights",
            ],
            thresholds: Thresholds::default(),
        },
        // The material IDs (F5's second phase) on the shader swatches: the regions each pixel
        // shades with, the ramp spheres' four latitude bands from their control map and the band
        // sphere's from its vertices, the mannequin's arms apart from the rest of it, and every
        // outline in its region's color, darker.
        Scene {
            name: "debug-material-ids",
            args: &[
                "--capture-ticks",
                "90",
                "--plain-post",
                "--swatches",
                "--plain-sky",
                "--aa",
                "msaa",
                "--shadows",
                "off",
                "--fog",
                "off",
                "--ao",
                "off",
                "--debug-view",
                "material-ids",
            ],
            thresholds: Thresholds::default(),
        },
        // The ramp rows on the same at 19:30 on Mobile-Low: the ramp spheres' day rows striped
        // with their night rows, time of day's blend of the two, and the band sphere's three
        // bands and the mannequin's two in their greys.
        Scene {
            name: "debug-ramp-rows",
            args: &[
                "--capture-ticks",
                "90",
                "--plain-post",
                "--swatches",
                "--plain-sky",
                "--time-of-day",
                "19.5",
                "--tier",
                "mobile-low",
                "--shadows",
                "off",
                "--fog",
                "off",
                "--ao",
                "off",
                "--debug-view",
                "ramp-rows",
            ],
            thresholds: Thresholds::default(),
        },
        // The outline widths along the row of mannequins under temporal AA: in pixels on a
        // 1080-row target, the nearest's widest, thinning with distance as the width curve says,
        // its pedestal's wider still, the farthest gone past the cutoff.
        Scene {
            name: "debug-outline-widths",
            args: &[
                "--capture-ticks",
                "90",
                "--outline-row",
                "--plain-post",
                "--plain-sky",
                "--aa",
                "taa",
                "--capture-frames",
                "32",
                "--shadows",
                "off",
                "--fog",
                "off",
                "--ao",
                "off",
                "--debug-view",
                "outline-widths",
            ],
            thresholds: Thresholds::default(),
        },
        // Debug draw through its whole path: one of each shape beside the mannequin, in its color;
        // a blue line hidden behind it and a red one over it; a blue one through it on top, and a
        // half-alpha red one; a magenta capsule inside its body, on top, where the green one
        // depth-tested inside it stays hidden but where it pokes out; the violet box lasting from
        // the first frame on the world's clock; the green arrow overhead held from the last fixed
        // step. No labels: only `dev` builds' overlay paints them.
        Scene {
            name: "debugdraw-shapes",
            args: &[
                "--capture-ticks",
                "90",
                "--plain-post",
                "--plain-sky",
                "--aa",
                "msaa",
                "--shadows",
                "off",
                "--fog",
                "off",
                "--ao",
                "off",
                "--debug-draw",
                "shapes",
            ],
            thresholds: Thresholds::default(),
        },
        // The combat views (M2 T1) on `melee-sweep` at tick 10, over the plain scene, whose mannequin
        // has nothing to do with the fight: the first actor, left, with its hurtbox in green and the
        // red sweep of its first hit's blade, half a metre this tick, faint where it began; the second,
        // nearest, mid-dodge and in its perfect window, magenta; the third, right, in its skill's parry
        // window, yellow. Each arrow is which way its actor faces.
        Scene {
            name: "debugdraw-volumes",
            args: &[
                "--capture-ticks",
                "10",
                "--replay",
                "tests/replays/melee-sweep.sreplay",
                "--zoom",
                "2.0",
                "--plain-post",
                "--plain-sky",
                "--aa",
                "msaa",
                "--shadows",
                "off",
                "--fog",
                "off",
                "--ao",
                "off",
                "--debug-draw",
                "volumes",
            ],
            thresholds: Thresholds::default(),
        },
        // A timeline with the camera and two post parameters (M3 Q1): the director's on a shot of the square
        // and the picture's darkened, with the fight held.
        Scene {
            name: "timeline-camera-post",
            args: &[
                "--capture-ticks",
                "120",
                "--arena",
                "--timeline",
                "455708fd-5c63-4cf4-98b6-1c0921bf8213",
                "--time-of-day",
                "13",
                "--tier",
                "desktop-high",
                "--ao",
                "off",
                "--capture-frames",
                "32",
            ],
            thresholds: Thresholds::default(),
        },
        // A zone trip's wipe (M3 W4), from `zone-trip`'s first entry, due after its 120th tick: 14 steps into
        // the slats coming in from the right over the arena, which the fight holds still behind. The wipe's
        // clock is the steps', so the frame is the same every run, and the load it covers comes later.
        Scene {
            name: "trip-wipe-mid",
            args: &[
                "--capture-ticks",
                "135",
                "--arena",
                "--replay",
                "tests/replays/zone-trip.sreplay",
                "--time-of-day",
                "13",
                "--tier",
                "desktop-high",
                "--ao",
                "off",
                "--capture-frames",
                "32",
            ],
            thresholds: Thresholds::default(),
        },
        // The same wipe covering the screen: however soon the hub comes in behind it, no step's time passes
        // over the frames drawn, so it can't uncover. Only the wipe draws.
        Scene {
            name: "trip-wipe-covered",
            args: &[
                "--capture-ticks",
                "154",
                "--arena",
                "--replay",
                "tests/replays/zone-trip.sreplay",
                "--time-of-day",
                "13",
                "--tier",
                "desktop-high",
                "--ao",
                "off",
                "--capture-frames",
                "32",
            ],
            thresholds: Thresholds::default(),
        },
        // The arena (M2 O5) at midday, by the plain camera behind the player: the fight's clear floor, the
        // enemy's stand-in past the player's shoulder, the pavilion on the left and the market on the
        // right, the street's moon gate between them. The frame the combat slice is judged in.
        Scene {
            name: "arena-day",
            args: &[
                "--capture-ticks",
                "90",
                "--arena",
                "--arena-shot",
                "follow",
                "--time-of-day",
                "13",
                "--tier",
                "desktop-high",
                "--ao",
                "off",
                "--capture-frames",
                "32",
            ],
            thresholds: Thresholds::default(),
        },
        // The same frame at golden hour, the low sun behind the pavilion's roof.
        Scene {
            name: "arena-golden",
            args: &[
                "--capture-ticks",
                "90",
                "--arena",
                "--arena-shot",
                "follow",
                "--time-of-day",
                "18",
                "--tier",
                "desktop-high",
                "--ao",
                "off",
                "--capture-frames",
                "32",
            ],
            thresholds: Thresholds::default(),
        },
        // The arena part way through a recorded walk, at the morning hour benches use: the character's
        // root and the camera both come from the replay's actor, on its run back from the lantern's
        // post at tick 280 of `arena-walk`, and the app
        // checks the replay reaches its checksums before this frame is written.
        Scene {
            name: "arena-replay",
            args: &[
                "--capture-ticks",
                "280",
                "--arena",
                "--arena-shot",
                "follow",
                "--replay",
                "tests/replays/arena-walk.sreplay",
                "--time-of-day",
                "10",
                "--tier",
                "desktop-high",
                "--ao",
                "off",
                "--capture-frames",
                "32",
            ],
            thresholds: Thresholds::default(),
        },
        // The enemy's overhead, 38 ticks into its 48 of wind-up with its arm up and its blade over it,
        // from `arena-fight-still`, whose player only stands there: what a dodge has to read.
        Scene {
            name: "arena-fight-telegraph",
            args: &[
                "--capture-ticks",
                "112",
                "--cues",
                "--arena",
                "--arena-shot",
                "follow",
                "--view",
                "75,8",
                "--replay",
                "tests/replays/arena-fight-still.sreplay",
                "--time-of-day",
                "10",
                "--tier",
                "desktop-high",
                "--ao",
                "off",
                "--blade",
                "on",
                "--capture-frames",
                "32",
            ],
            thresholds: Thresholds::default(),
        },
        // That overhead landed, three ticks on from its first contact: both clocks frozen, with the
        // blade across the player, who has started to fall.
        Scene {
            name: "arena-fight-hit",
            args: &[
                "--capture-ticks",
                "128",
                "--cues",
                "--hud",
                "live",
                "--arena",
                "--arena-shot",
                "follow",
                "--view",
                "75,8",
                "--replay",
                "tests/replays/arena-fight-still.sreplay",
                "--time-of-day",
                "10",
                "--tier",
                "desktop-high",
                "--ao",
                "off",
                "--blade",
                "on",
                "--capture-frames",
                "32",
            ],
            thresholds: Thresholds::default(),
        },
        // The player's first cut of `arena-fight-won`, its sword trail mid-arc: a strip that ends on the
        // drawn blade, the crescent narrowing back along what the tip passed.
        Scene {
            name: "arena-fight-slash",
            args: &[
                "--capture-ticks",
                "12",
                "--cues",
                "--arena",
                "--arena-shot",
                "follow",
                "--view",
                "75,8",
                "--replay",
                "tests/replays/arena-fight-won.sreplay",
                "--time-of-day",
                "10",
                "--tier",
                "desktop-high",
                "--ao",
                "off",
                "--blade",
                "on",
                "--capture-frames",
                "32",
            ],
            thresholds: Thresholds::default(),
        },
        // The player's turn, from `arena-fight-won`: a dash attack cutting across the warrior as it lands,
        // on O3's clip with the sword in the graph's hand, which is what the blade's hit sweeps along.
        Scene {
            name: "arena-fight-cut",
            args: &[
                "--capture-ticks",
                "104",
                "--cues",
                "--arena",
                "--arena-shot",
                "follow",
                "--view",
                "75,8",
                "--replay",
                "tests/replays/arena-fight-won.sreplay",
                "--time-of-day",
                "10",
                "--tier",
                "desktop-high",
                "--ao",
                "off",
                "--blade",
                "on",
                "--capture-frames",
                "32",
            ],
            thresholds: Thresholds::default(),
        },
        // A party's fight in the arena (M2 G7), by day: the second member on the field, in its recolor, in
        // the recorded `party-swap`, against the warrior. The app checks the replay reaches its checksums
        // before this frame is written.
        Scene {
            name: "arena-party-second",
            args: &[
                "--capture-ticks",
                "110",
                "--arena",
                "--arena-shot",
                "follow",
                "--party",
                "--replay",
                "tests/replays/party-swap.sreplay",
                "--time-of-day",
                "10",
                "--tier",
                "desktop-high",
                "--ao",
                "off",
                "--blade",
                "on",
                "--capture-frames",
                "32",
            ],
            thresholds: Thresholds::default(),
        },
        // The same fight later on: the third member, in its own.
        Scene {
            name: "arena-party-third",
            args: &[
                "--capture-ticks",
                "225",
                "--arena",
                "--arena-shot",
                "follow",
                "--party",
                "--replay",
                "tests/replays/party-swap.sreplay",
                "--time-of-day",
                "10",
                "--tier",
                "desktop-high",
                "--ao",
                "off",
                "--blade",
                "on",
                "--capture-frames",
                "32",
            ],
            thresholds: Thresholds::default(),
        },
        // The Lantern Ward's gate cutscene (M3 Q3), at fixed times of a replay that watches it, in the zone's
        // court at its evening hour. A capture's tick is a frame, and the gate holds the fight from frame 30,
        // so frame 30 + 60t is t seconds in. 3.5 s: the warden close, the exposure down, its overhead
        // coming down, the vignette in.
        Scene {
            name: "cutscene-gate-warden",
            args: &[
                "--capture-ticks",
                "240",
                "--arena",
                "--replay",
                "tests/replays/ward-court-watched.sreplay",
                "--time-of-day",
                "17.5",
                "--tier",
                "desktop-high",
                "--ao",
                "off",
                "--blade",
                "on",
                "--capture-frames",
                "32",
            ],
            thresholds: Thresholds::default(),
        },
        // 6.2 s: the character close, turning into guard, with the ward's street behind and the bloom up.
        Scene {
            name: "cutscene-gate-guard",
            args: &[
                "--capture-ticks",
                "402",
                "--arena",
                "--replay",
                "tests/replays/ward-court-watched.sreplay",
                "--time-of-day",
                "17.5",
                "--tier",
                "desktop-high",
                "--ao",
                "off",
                "--blade",
                "on",
                "--capture-frames",
                "32",
            ],
            thresholds: Thresholds::default(),
        },
        // 8.6 s: the two of them down the court as the view blends back toward the fight's camera, the
        // vignette going.
        Scene {
            name: "cutscene-gate-return",
            args: &[
                "--capture-ticks",
                "546",
                "--arena",
                "--replay",
                "tests/replays/ward-court-watched.sreplay",
                "--time-of-day",
                "17.5",
                "--tier",
                "desktop-high",
                "--ao",
                "off",
                "--blade",
                "on",
                "--capture-frames",
                "32",
            ],
            thresholds: Thresholds::default(),
        },
        // The player's ultimate (M3 Q3b) on `arena-ultimate`, which presses it on frame 60: 1.1 s into its
        // cinematic, the wide shot from the character's right as the chop lands on the warrior, the flash on
        // the landing and the bloom up, the vignette closed in, and the warrior held at a fifth of its speed.
        Scene {
            name: "arena-ultimate-chop",
            args: &[
                "--capture-ticks",
                "126",
                "--arena",
                "--replay",
                "tests/replays/arena-ultimate.sreplay",
                "--time-of-day",
                "10",
                "--tier",
                "desktop-high",
                "--ao",
                "off",
                "--blade",
                "on",
                "--capture-frames",
                "32",
            ],
            thresholds: Thresholds::default(),
        },
        // The material layer samples (M1 C7) by day, right of the character: crimson lacquer with
        // a crisp highlight; gold, its tinted matcap and highlight, the sky reflected on its
        // metallic stripes; leather, darker toward its rim, with a soft GGX highlight; a dark suit
        // with cyan light lines; and a leg in a thigh-high black stocking, sheer facing the
        // camera, near black at its silhouette, bare above its top.
        // Eight in one fight (M3 B1), in what a player sees: a warrior for each, and the HUD with the foe the
        // player has and a small bar for each other one. The only arena scene on the director, since how
        // it stands back for those near the player is what this is here to show.
        Scene {
            name: "arena-skirmish",
            args: &[
                "--capture-ticks",
                "240",
                "--cues",
                "--hud",
                "live",
                "--arena",
                "--replay",
                "tests/replays/arena-skirmish.sreplay",
                "--time-of-day",
                "10",
                "--tier",
                "desktop-high",
                "--ao",
                "off",
                "--blade",
                "on",
                "--capture-frames",
                "32",
            ],
            thresholds: Thresholds::default(),
        },
        // A boss (M3 B3) a few steps past the hit that took it under half its health, at tick 652 of its
        // replay: the HUD has its bar in the boss's place, wider than a foe's, with an inked mark at each of
        // its phases, and it fights on with its fiercer brain.
        Scene {
            name: "arena-boss",
            args: &[
                "--capture-ticks",
                "670",
                "--cues",
                "--hud",
                "live",
                "--arena",
                "--replay",
                "tests/replays/arena-boss.sreplay",
                "--time-of-day",
                "10",
                "--tier",
                "desktop-high",
                "--ao",
                "off",
                "--blade",
                "on",
                "--capture-frames",
                "32",
            ],
            thresholds: Thresholds::default(),
        },
        // Two kinds of enemy in one fight (M3 B3b): the captain, a head over the warrior beside it, each drawn
        // from its own kind's look, on the same moves and the same graph.
        Scene {
            name: "arena-captain",
            args: &[
                "--capture-ticks",
                "90",
                "--cues",
                "--hud",
                "live",
                "--arena",
                "--replay",
                "tests/replays/arena-captain.sreplay",
                "--time-of-day",
                "10",
                "--tier",
                "desktop-high",
                "--ao",
                "off",
                "--blade",
                "on",
                "--capture-frames",
                "32",
            ],
            thresholds: Thresholds::default(),
        },
        Scene {
            name: "materials-day",
            args: &[
                "--capture-ticks",
                "90",
                "--materials",
                "--plain-post",
                "--aa",
                "msaa",
                "--shadows",
                "off",
                "--fog",
                "off",
                "--ao",
                "off",
            ],
            thresholds: Thresholds::default(),
        },
        // The same at 21:00 under the full post look: the light lines' glow blooming as the
        // world's light, the gold reflecting the night sky, the highlights in the moonlit key.
        Scene {
            name: "materials-night",
            args: &[
                "--capture-ticks",
                "90",
                "--materials",
                "--time-of-day",
                "21",
                "--aa",
                "msaa",
                "--shadows",
                "off",
                "--fog",
                "off",
                "--ao",
                "off",
            ],
            thresholds: Thresholds::default(),
        },
        // The character at half opacity in the courtyard (M1 C7), temporal AA resolving the
        // proximity fade's dither, which turns through its 16-frame cycle, into a blend with what
        // lies behind it; its outlines fade with it and it takes no rim.
        Scene {
            name: "fade-taa",
            args: &[
                "--capture-ticks",
                "90",
                "--fade",
                "0.5",
                "--environment",
                "--plain-post",
                "--fog",
                "off",
                "--tier",
                "desktop-high",
                "--aa",
                "taa",
                "--ao",
                "off",
                "--capture-frames",
                "32",
            ],
            thresholds: Thresholds::default(),
        },
        // Mobile-Low's fade: 4× MSAA, each pixel covering half its samples, so the resolve blends.
        Scene {
            name: "fade-mobile",
            args: &[
                "--capture-ticks",
                "90",
                "--fade",
                "0.5",
                "--environment",
                "--plain-post",
                "--fog",
                "off",
                "--tier",
                "mobile-low",
                "--ao",
                "off",
            ],
            thresholds: Thresholds::default(),
        },
        // The look-dev character's face close-up (X1 part 2c) at midday, pinned as the other
        // look-dev scenes are: the face's SDF shadow in its peach ramp with a light blush, the
        // bangs' warm shadow on the forehead, the irises' gradient and sparkles under the
        // glowing highlights, the darkened lash line, the hair's crisp band and thin brown
        // outlines, the square behind the character.
        Scene {
            name: "lookdev-face",
            args: &[
                "--capture-ticks",
                "90",
                "--look-dev",
                "face",
                "--time-of-day",
                "13",
                "--tier",
                "desktop-high",
                "--ao",
                "off",
                "--capture-frames",
                "32",
            ],
            thresholds: Thresholds::default(),
        },
        // The first moving character (M2 N2): N1's key-pose clip on the look-dev character at the
        // hero shot's dusk, every landed feature on but spring bones, which came later, at fixed ticks
        // through the swing, each drawn playing up to its tick so temporal AA's history is a player's.
        // Here the anticipation's end: the character has sunk and drawn the right arm back, and turned
        // at the spine.
        Scene {
            name: "lookdev-strike-anticipation",
            args: &[
                "--capture-ticks",
                "14",
                "--look-dev",
                "hero",
                "--clip",
                "strike",
                "--springs",
                "off",
                "--time-of-day",
                "18",
                "--tier",
                "desktop-high",
                "--ao",
                "off",
                "--capture-frames",
                "15",
                "--capture-step",
                "1",
            ],
            thresholds: Thresholds::default(),
        },
        // Three ticks into the strike, its fastest stretch: the arm crosses about 100 px a tick
        // at 1080p and the root motion carries the character at the camera. Lines stay on the arm,
        // nothing trails on the wall behind it, and the sleeve folds as its weights make it.
        Scene {
            name: "lookdev-strike-fast",
            args: &[
                "--capture-ticks",
                "17",
                "--look-dev",
                "hero",
                "--clip",
                "strike",
                "--springs",
                "off",
                "--time-of-day",
                "18",
                "--tier",
                "desktop-high",
                "--ao",
                "off",
                "--capture-frames",
                "16",
                "--capture-step",
                "1",
            ],
            thresholds: Thresholds::default(),
        },
        // The strike's mark, the arm at full reach past its target and the hips forward.
        Scene {
            name: "lookdev-strike-mark",
            args: &[
                "--capture-ticks",
                "20",
                "--look-dev",
                "hero",
                "--clip",
                "strike",
                "--springs",
                "off",
                "--time-of-day",
                "18",
                "--tier",
                "desktop-high",
                "--ao",
                "off",
                "--capture-frames",
                "16",
                "--capture-step",
                "1",
            ],
            thresholds: Thresholds::default(),
        },
        // Settling back to the first pose, a slow arm: lines and shading hold under long history.
        Scene {
            name: "lookdev-strike-settle",
            args: &[
                "--capture-ticks",
                "40",
                "--look-dev",
                "hero",
                "--clip",
                "strike",
                "--springs",
                "off",
                "--time-of-day",
                "18",
                "--tier",
                "desktop-high",
                "--ao",
                "off",
                "--capture-frames",
                "16",
                "--capture-step",
                "1",
            ],
            thresholds: Thresholds::default(),
        },
        // The face in place (`--clip-in-place`) at midday at the strike's mark, the head turned 22°:
        // the face's SDF shadow, the bangs' shadow, the eye layers and the hair follow the head, as a
        // stale `CharacterFace` wouldn't.
        Scene {
            name: "lookdev-strike-face",
            args: &[
                "--capture-ticks",
                "20",
                "--look-dev",
                "face",
                "--clip",
                "strike",
                "--clip-in-place",
                "--springs",
                "off",
                "--time-of-day",
                "13",
                "--tier",
                "desktop-high",
                "--ao",
                "off",
                "--capture-frames",
                "16",
                "--capture-step",
                "1",
            ],
            thresholds: Thresholds::default(),
        },
        // IK (M2 N4) on a 30° slope across the character's stance, the clip's stance on flat ground
        // under it: the soles lie on the slope with no gap under the round foot, the hips have come
        // down for the lower foot and no bone stretches, and the head and eyes look over at a place
        // up the slope to the camera's right.
        Scene {
            name: "lookdev-ik-slope",
            args: &[
                "--springs",
                "off",
                "--capture-ticks",
                "14",
                "--look-dev",
                "full",
                "--clip",
                "strike",
                "--clip-in-place",
                "--ik",
                "on",
                "--ik-ground",
                "slope",
                "--ik-look",
                "2,1.5,2.5",
                "--time-of-day",
                "13",
                "--tier",
                "desktop-high",
                "--ao",
                "off",
                "--capture-frames",
                "15",
                "--capture-step",
                "1",
            ],
            thresholds: Thresholds::default(),
        },
        // Standing with one foot on an 18 cm step and the other on the paving below it, looking down
        // and away: that knee bends to take the height, the other leg keeps its pose.
        Scene {
            name: "lookdev-ik-step",
            args: &[
                "--springs",
                "off",
                "--capture-ticks",
                "14",
                "--look-dev",
                "full",
                "--clip",
                "strike",
                "--clip-in-place",
                "--ik",
                "on",
                "--ik-ground",
                "step",
                "--ik-look",
                "-2,1.2,2.5",
                "--time-of-day",
                "13",
                "--tier",
                "desktop-high",
                "--ao",
                "off",
                "--capture-frames",
                "15",
                "--capture-step",
                "1",
            ],
            thresholds: Thresholds::default(),
        },
        // A defeat (M2 N4) through the graph, after its move has ended: the character is on its back
        // where `death_back` has left it, eyes shut, and nothing's breathing. Its whole body is past the
        // cooked sphere, so this is posed bounds' frame: under a low sun its shadow is one piece on the
        // paving and its outline is whole, by the pose's own bounds. The first frame is inside the move.
        Scene {
            name: "lookdev-death-back",
            args: &[
                "--springs",
                "off",
                "--capture-ticks",
                "150",
                "--look-dev",
                "down",
                "--graph",
                "defeat",
                "--time-of-day",
                "17",
                "--tier",
                "desktop-high",
                "--ao",
                "off",
                "--capture-frames",
                "15",
                "--capture-step",
                "1",
            ],
            thresholds: Thresholds::default(),
        },
        // Spring bones under the strike (M2 N5): at its mark, the head turned and the body gone ahead,
        // the long hair trails the head, the bangs lag its turn and the hem and sleeves lift. Lines
        // stay on all of it, as hulls read the same palette.
        Scene {
            name: "lookdev-strike-springs-mark",
            args: &[
                "--capture-ticks",
                "20",
                "--look-dev",
                "hero",
                "--clip",
                "strike",
                "--time-of-day",
                "18",
                "--tier",
                "desktop-high",
                "--ao",
                "off",
                "--capture-frames",
                "16",
                "--capture-step",
                "1",
            ],
            thresholds: Thresholds::default(),
        },
        // Six ticks on, with the arm held, what the strike set moving is still going: the hair swinging
        // past the shoulder and the hem coming back.
        Scene {
            name: "lookdev-strike-springs-after",
            args: &[
                "--capture-ticks",
                "26",
                "--look-dev",
                "hero",
                "--clip",
                "strike",
                "--time-of-day",
                "18",
                "--tier",
                "desktop-high",
                "--ao",
                "off",
                "--capture-frames",
                "16",
                "--capture-step",
                "1",
            ],
            thresholds: Thresholds::default(),
        },
        Scene {
            name: "lookdev-clip-idle",
            args: &[
                "--springs",
                "off",
                "--capture-ticks",
                "70",
                "--look-dev",
                "hero",
                "--clip",
                "idle",
                "--time-of-day",
                "10",
                "--tier",
                "desktop-high",
                "--ao",
                "off",
                "--capture-frames",
                "15",
                "--capture-step",
                "1",
            ],
            thresholds: Thresholds::default(),
        },
        Scene {
            name: "lookdev-clip-run",
            args: &[
                "--springs",
                "off",
                "--capture-ticks",
                "31",
                "--look-dev",
                "hero",
                "--clip",
                "Running_B",
                "--time-of-day",
                "10",
                "--tier",
                "desktop-high",
                "--ao",
                "off",
                "--capture-frames",
                "15",
                "--capture-step",
                "1",
            ],
            thresholds: Thresholds::default(),
        },
        Scene {
            name: "lookdev-clip-hit",
            args: &[
                "--springs",
                "off",
                "--capture-ticks",
                "15",
                "--look-dev",
                "hero",
                "--clip",
                "Hit_B",
                "--time-of-day",
                "10",
                "--tier",
                "desktop-high",
                "--ao",
                "off",
                "--capture-frames",
                "15",
                "--capture-step",
                "1",
            ],
            thresholds: Thresholds::default(),
        },
        // The run with spring bones (M2 N5), the same frame as `lookdev-clip-run`: the hair lags the
        // body's bob and keeps its shape, where VRM's step alone flings it out in ropes round the head.
        Scene {
            name: "lookdev-clip-run-springs",
            args: &[
                "--capture-ticks",
                "31",
                "--look-dev",
                "hero",
                "--clip",
                "Running_B",
                "--time-of-day",
                "10",
                "--tier",
                "desktop-high",
                "--ao",
                "off",
                "--capture-frames",
                "15",
                "--capture-step",
                "1",
            ],
            thresholds: Thresholds::default(),
        },
        // The animation graph (M2 N3) half a second into the controller's run, every frame drawn: the
        // blend space past its walk and on its run with breathing added, the simulation placing the
        // character, a treadmill keeping it in shot, and the hair lagging the run it still feels.
        Scene {
            name: "lookdev-graph-run",
            args: &[
                "--capture-ticks",
                "30",
                "--look-dev",
                "hero",
                "--graph",
                "run",
                "--time-of-day",
                "10",
                "--tier",
                "desktop-high",
                "--ao",
                "off",
                "--capture-frames",
                "30",
                "--capture-step",
                "1",
            ],
            thresholds: Thresholds::default(),
        },
        // The frame after a strike cancels into a dodge: its first frame back on the new move's clip
        // and most of the way there, with the arm's swing carried into it rather than faded in.
        Scene {
            name: "lookdev-graph-cancel",
            args: &[
                "--capture-ticks",
                "28",
                "--look-dev",
                "hero",
                "--graph",
                "cancel",
                "--time-of-day",
                "10",
                "--tier",
                "desktop-high",
                "--ao",
                "off",
                "--capture-frames",
                "16",
                "--capture-step",
                "1",
            ],
            thresholds: Thresholds::default(),
        },
        // The enemy (M2 O4) squaring up to the character in the square at midday, pinned as the other
        // look-dev scenes are: bone in its lavender shade, dark plate with a painted sheen, a wine
        // mantle, a raised blade and red eyes that bloom.
        Scene {
            name: "lookdev-enemy",
            args: &[
                "--capture-ticks",
                "90",
                "--look-dev",
                "enemy",
                "--time-of-day",
                "13",
                "--tier",
                "desktop-high",
                "--ao",
                "off",
                "--capture-frames",
                "32",
            ],
            thresholds: Thresholds::default(),
        },
        // The same at night, where the threat is the glow: the eyes and the mantle's hem against the
        // blue night, as the world's light.
        Scene {
            name: "lookdev-enemy-night",
            args: &[
                "--capture-ticks",
                "90",
                "--look-dev",
                "enemy",
                "--time-of-day",
                "22",
                "--tier",
                "desktop-high",
                "--ao",
                "off",
                "--capture-frames",
                "32",
            ],
            thresholds: Thresholds::default(),
        },
        // The Lancer (M3 C2), the generated second enemy, standing where the warrior does by `--model`: its
        // thrust's wind-up held at the tick it ends, the spear drawn back. Indigo robes and lacquer under a
        // rain hat, a porcelain mask and jade glow lines on the hems, cuffs and hat band.
        Scene {
            name: "lookdev-lancer",
            args: &[
                "--capture-ticks",
                "90",
                "--look-dev",
                "enemy",
                "--model",
                "41e229ad-28a3-41f9-aeb8-ba214e06be6a:99cd4c5a-fdc8-527b-81fb-dcbec9db71df@16",
                "--time-of-day",
                "13",
                "--tier",
                "desktop-high",
                "--ao",
                "off",
                "--capture-frames",
                "32",
            ],
            thresholds: Thresholds::default(),
        },
        // The player's moves (M2 O3), in place, each drawn playing up to its tick, sword in hand. The first
        // cut coming through, seen as a fight sees it from behind the right shoulder: the hips ahead of the
        // arm and the right foot about to land. Springs are pinned off while N5 tunes the hair.
        Scene {
            name: "lookdev-move-cut",
            args: &[
                "--capture-ticks",
                "9",
                "--look-dev",
                "body",
                "--clip",
                "light_1",
                "--clip-in-place",
                "--springs",
                "off",
                "--view",
                "205,6",
                "--blade",
                "on",
                "--time-of-day",
                "10",
                "--tier",
                "desktop-high",
                "--ao",
                "off",
                "--capture-frames",
                "10",
                "--capture-step",
                "1",
            ],
            thresholds: Thresholds::default(),
        },
        // The finisher's top, from the side: up on the back toes and arched, the blade back over the head.
        Scene {
            name: "lookdev-move-windup",
            args: &[
                "--capture-ticks",
                "13",
                "--look-dev",
                "body",
                "--clip",
                "light_3",
                "--clip-in-place",
                "--springs",
                "off",
                "--view",
                "110,4",
                "--blade",
                "on",
                "--time-of-day",
                "10",
                "--tier",
                "desktop-high",
                "--ao",
                "off",
                "--capture-frames",
                "14",
                "--capture-step",
                "1",
            ],
            thresholds: Thresholds::default(),
        },
        // The finisher's landing, past its mark: a deep lunge folded over the blade. The shorts come closest
        // to the shirt's back here, and the back foot's toes bend on the ground.
        Scene {
            name: "lookdev-move-landing",
            args: &[
                "--capture-ticks",
                "23",
                "--look-dev",
                "body",
                "--clip",
                "light_3",
                "--clip-in-place",
                "--springs",
                "off",
                "--view",
                "110,4",
                "--blade",
                "on",
                "--time-of-day",
                "10",
                "--tier",
                "desktop-high",
                "--ao",
                "off",
                "--capture-frames",
                "16",
                "--capture-step",
                "1",
            ],
            thresholds: Thresholds::default(),
        },
        // The dash attack's cut starting, from behind: thrown forward low, the back foot trailing, the arm
        // and blade flung out to the right.
        Scene {
            name: "lookdev-move-dash",
            args: &[
                "--capture-ticks",
                "7",
                "--look-dev",
                "body",
                "--clip",
                "dash_attack",
                "--clip-in-place",
                "--springs",
                "off",
                "--view",
                "205,6",
                "--blade",
                "on",
                "--time-of-day",
                "10",
                "--tier",
                "desktop-high",
                "--ao",
                "off",
                "--capture-frames",
                "8",
                "--capture-step",
                "1",
            ],
            thresholds: Thresholds::default(),
        },
        // The dodge in the air, from behind, as a player sees it: low and wide, the feet split and the arms
        // and blade left out in front.
        Scene {
            name: "lookdev-move-dodge",
            args: &[
                "--capture-ticks",
                "6",
                "--look-dev",
                "body",
                "--clip",
                "dodge",
                "--clip-in-place",
                "--springs",
                "off",
                "--view",
                "205,6",
                "--blade",
                "on",
                "--time-of-day",
                "10",
                "--tier",
                "desktop-high",
                "--ao",
                "off",
                "--capture-frames",
                "7",
                "--capture-step",
                "1",
            ],
            thresholds: Thresholds::default(),
        },
        // The skill's coil, from the character's left: down and turned away, aiming along the left arm.
        Scene {
            name: "lookdev-move-coil",
            args: &[
                "--capture-ticks",
                "8",
                "--look-dev",
                "body",
                "--clip",
                "skill",
                "--clip-in-place",
                "--springs",
                "off",
                "--view",
                "60,4",
                "--blade",
                "on",
                "--time-of-day",
                "10",
                "--tier",
                "desktop-high",
                "--ao",
                "off",
                "--capture-frames",
                "9",
                "--capture-step",
                "1",
            ],
            thresholds: Thresholds::default(),
        },
        // The garment samples (M1 C8) left of the character: a navy collar, a crimson skirt and a
        // gold ribbon given half a twist, each one-sided (outer column: the insides culled, the
        // outline hull's inside showing through) beside two-sided (inner column: the insides
        // shaded as the side the camera sees, the ribbon's back as lit as its front).
        Scene {
            name: "two-sided",
            args: &[
                "--capture-ticks",
                "90",
                "--two-sided",
                "--plain-post",
                "--aa",
                "msaa",
                "--shadows",
                "off",
                "--fog",
                "off",
                "--ao",
                "off",
            ],
            thresholds: Thresholds::default(),
        },
        // The face rig's keys at 18:00 (M1 C8): the warm key lights the shadow sides' authored
        // tone, so they turn orange; the reference for the next scene.
        Scene {
            name: "shade-golden-hour",
            args: &[
                "--capture-ticks",
                "90",
                "--faces",
                "--face-layout",
                "lights",
                "--time-of-day",
                "18",
                "--plain-post",
                "--plain-sky",
                "--aa",
                "msaa",
                "--shadows",
                "off",
                "--fog",
                "off",
                "--ao",
                "off",
            ],
            thresholds: Thresholds::default(),
        },
        // The same with the shadow hue keyed (M1 C8): a cool near-white hue taking 80% of the
        // shadow sides' light, which keep the skin's rosy shade tone at the key's luminance while
        // the lit sides stay warm.
        Scene {
            name: "shade-hue-golden-hour",
            args: &[
                "--capture-ticks",
                "90",
                "--faces",
                "--face-layout",
                "lights",
                "--time-of-day",
                "18",
                "--shade-hue",
                "0.8,0.88,1,0.8",
                "--plain-post",
                "--plain-sky",
                "--aa",
                "msaa",
                "--shadows",
                "off",
                "--fog",
                "off",
                "--ao",
                "off",
            ],
            thresholds: Thresholds::default(),
        },
        // The rim figures keyed from behind through the gameplay rig's limits (M1 C8), which fold
        // each key round toward the camera's side: the rim follows the folded key, its back light
        // gone; the reference for the next scene.
        Scene {
            name: "rim-folded",
            args: &[
                "--capture-ticks",
                "90",
                "--rim-rig",
                "folded",
                "--plain-post",
                "--plain-sky",
                "--aa",
                "msaa",
                "--shadows",
                "off",
                "--fog",
                "off",
                "--ao",
                "off",
            ],
            thresholds: Thresholds::default(),
        },
        // The same with the rig's camera rim taking all the rim's gating (M1 C8): held behind the
        // figures on the camera's right and above, it keeps the rim on that silhouette and its
        // back-light weighting, whatever the folded key does.
        Scene {
            name: "rim-folded-camera",
            args: &[
                "--capture-ticks",
                "90",
                "--rim-rig",
                "folded",
                "--camera-rim",
                "1",
                "--plain-post",
                "--plain-sky",
                "--aa",
                "msaa",
                "--shadows",
                "off",
                "--fog",
                "off",
                "--ao",
                "off",
            ],
            thresholds: Thresholds::default(),
        },
        // The bangs' shadow of `hair-shadow` with a soft edge as wide as its reach (M1 C8): it
        // fades in along each key's direction below the strands instead of snapping to the face's
        // light split.
        Scene {
            name: "hair-shadow-soft",
            args: &[
                "--capture-ticks",
                "90",
                "--faces",
                "--face-layout",
                "hair",
                "--hair-shadow-softness",
                "0.03",
                "--plain-post",
                "--plain-sky",
                "--aa",
                "msaa",
                "--shadows",
                "off",
                "--fog",
                "off",
                "--ao",
                "off",
            ],
            thresholds: Thresholds::default(),
        },
        // The face rig lit from behind (M1 C8), round from the heads' right to their left and at
        // several heights: the hair in its shadow tone, the faces in their SDF shadow; the
        // reference for the next scene.
        Scene {
            name: "hair-backlit",
            args: &[
                "--capture-ticks",
                "90",
                "--faces",
                "--face-layout",
                "backlit",
                "--plain-post",
                "--plain-sky",
                "--aa",
                "msaa",
                "--shadows",
                "off",
                "--fog",
                "off",
                "--ao",
                "off",
            ],
            thresholds: Thresholds::default(),
        },
        // The same with light through the hair at strength 1 (M1 C8): the hair glows in its own
        // color, most where the key lies straight behind it.
        Scene {
            name: "hair-transmission",
            args: &[
                "--capture-ticks",
                "90",
                "--faces",
                "--face-layout",
                "backlit",
                "--hair-transmission",
                "1",
                "--plain-post",
                "--plain-sky",
                "--aa",
                "msaa",
                "--shadows",
                "off",
                "--fog",
                "off",
                "--ao",
                "off",
            ],
            thresholds: Thresholds::default(),
        },
        // A sword arc down across the look-dev character (M2 E2), mid-swing: the crisp core and its
        // soft glow over noon's bright walls, then over night's dark ones, where exposure lifts the
        // world and must not wash the core out.
        Scene {
            name: "vfx-arc-noon",
            args: &[
                "--capture-ticks",
                "66",
                "--look-dev",
                "hero",
                "--time-of-day",
                "12",
                "--tier",
                "desktop-high",
                "--ao",
                "off",
                "--capture-frames",
                "16",
                "--capture-step",
                "1",
                "--vfx",
                "arc",
            ],
            thresholds: Thresholds::default(),
        },
        Scene {
            name: "vfx-arc-night",
            args: &[
                "--capture-ticks",
                "66",
                "--look-dev",
                "hero",
                "--time-of-day",
                "23",
                "--tier",
                "desktop-high",
                "--ao",
                "off",
                "--capture-frames",
                "16",
                "--capture-step",
                "1",
                "--vfx",
                "arc",
            ],
            thresholds: Thresholds::default(),
        },
        // The arc's blade landing: frozen on the character's clock by its hitstop two ticks in, its
        // spark's flash at its height and its trail still whole, with the world's dust moving on.
        Scene {
            name: "vfx-hit-sunset",
            args: &[
                "--capture-ticks",
                "74",
                "--look-dev",
                "hero",
                "--time-of-day",
                "18",
                "--tier",
                "desktop-high",
                "--ao",
                "off",
                "--capture-frames",
                "16",
                "--capture-step",
                "1",
                "--vfx",
                "hit",
            ],
            thresholds: Thresholds::default(),
        },
        // A duel's worst moment, the bench's load: two trails, sparks and dust, over the character.
        Scene {
            name: "vfx-duel-noon",
            args: &[
                "--capture-ticks",
                "120",
                "--look-dev",
                "hero",
                "--time-of-day",
                "12",
                "--tier",
                "desktop-high",
                "--ao",
                "off",
                "--capture-frames",
                "16",
                "--capture-step",
                "1",
                "--vfx",
                "duel",
            ],
            thresholds: Thresholds::default(),
        },
        // The same on the phones' path, at night: drawn in the main pass, against its depth, without
        // soft fades, and with fewer points to a trail.
        Scene {
            name: "vfx-duel-mobile-low",
            args: &[
                "--capture-ticks",
                "120",
                "--look-dev",
                "hero",
                "--time-of-day",
                "23",
                "--tier",
                "mobile-low",
                "--ao",
                "off",
                "--capture-frames",
                "16",
                "--capture-step",
                "1",
                "--vfx",
                "duel",
            ],
            thresholds: Thresholds::default(),
        },
        // Effects with nothing else on, and neither post's bloom nor its tone curve over them.
        Scene {
            name: "vfx-plain",
            args: &[
                "--capture-ticks",
                "120",
                "--vfx",
                "duel",
                "--plain-post",
                "--plain-sky",
                "--aa",
                "msaa",
                "--shadows",
                "off",
                "--fog",
                "off",
                "--ao",
                "off",
            ],
            thresholds: Thresholds::default(),
        },
        // A fight's hits as the cue table fires them (M2 E3), from G3's recording, landing on the look-dev
        // character until fighters are drawn: a medium hit two ticks in, which a spark and a little
        // aberration carry; a guard breaking, in ink, with the ring opening round it; the last blow,
        // seven ticks in, against the dusk.
        Scene {
            name: "hit-medium-noon",
            args: &[
                "--capture-ticks",
                "71",
                "--look-dev",
                "hero",
                "--time-of-day",
                "12",
                "--tier",
                "desktop-high",
                "--ao",
                "off",
                "--capture-frames",
                "16",
                "--capture-step",
                "1",
                "--replay",
                "tests/replays/fight.sreplay",
                "--cues",
            ],
            thresholds: Thresholds::default(),
        },
        Scene {
            name: "hit-break-noon",
            args: &[
                "--capture-ticks",
                "132",
                "--look-dev",
                "hero",
                "--time-of-day",
                "12",
                "--tier",
                "desktop-high",
                "--ao",
                "off",
                "--capture-frames",
                "16",
                "--capture-step",
                "1",
                "--replay",
                "tests/replays/fight.sreplay",
                "--cues",
            ],
            thresholds: Thresholds::default(),
        },
        Scene {
            name: "hit-finish-dusk",
            args: &[
                "--capture-ticks",
                "1156",
                "--look-dev",
                "hero",
                "--time-of-day",
                "19",
                "--tier",
                "desktop-high",
                "--ao",
                "off",
                "--capture-frames",
                "16",
                "--capture-step",
                "1",
                "--replay",
                "tests/replays/fight.sreplay",
                "--cues",
            ],
            thresholds: Thresholds::default(),
        },
        // The game's screens (M3 U7) at the aspect ratios spec §5.17 names, in both languages, on their sample
        // models. The HUD's words stand over the HUD they belong to.
        game_scene!("game-title-tablet-en", "title", "1024x768", "en"),
        game_scene!("game-title-tablet-ja", "title", "1024x768", "ja"),
        game_scene!("game-title-wide-en", "title", "1280x720", "en"),
        game_scene!("game-title-wide-ja", "title", "1280x720", "ja"),
        game_scene!("game-title-ultrawide-en", "title", "1680x720", "en"),
        game_scene!("game-title-ultrawide-ja", "title", "1680x720", "ja"),
        game_scene!("game-pause-tablet-en", "pause", "1024x768", "en"),
        game_scene!("game-pause-tablet-ja", "pause", "1024x768", "ja"),
        game_scene!("game-pause-wide-en", "pause", "1280x720", "en"),
        game_scene!("game-pause-wide-ja", "pause", "1280x720", "ja"),
        game_scene!("game-pause-ultrawide-en", "pause", "1680x720", "en"),
        game_scene!("game-pause-ultrawide-ja", "pause", "1680x720", "ja"),
        game_scene!("game-settings-tablet-en", "settings", "1024x768", "en"),
        game_scene!("game-settings-tablet-ja", "settings", "1024x768", "ja"),
        game_scene!("game-settings-wide-en", "settings", "1280x720", "en"),
        game_scene!("game-settings-wide-ja", "settings", "1280x720", "ja"),
        game_scene!("game-settings-ultrawide-en", "settings", "1680x720", "en"),
        game_scene!("game-settings-ultrawide-ja", "settings", "1680x720", "ja"),
        // The settings' pages (M4 A1), each on its sample model: every setting at what it starts at.
        game_scene!("game-display-tablet-en", "display", "1024x768", "en"),
        game_scene!("game-display-tablet-ja", "display", "1024x768", "ja"),
        game_scene!("game-display-wide-en", "display", "1280x720", "en"),
        game_scene!("game-display-wide-ja", "display", "1280x720", "ja"),
        game_scene!("game-display-ultrawide-en", "display", "1680x720", "en"),
        game_scene!("game-display-ultrawide-ja", "display", "1680x720", "ja"),
        game_scene!("game-sound-tablet-en", "sound", "1024x768", "en"),
        game_scene!("game-sound-tablet-ja", "sound", "1024x768", "ja"),
        game_scene!("game-sound-wide-en", "sound", "1280x720", "en"),
        game_scene!("game-sound-wide-ja", "sound", "1280x720", "ja"),
        game_scene!("game-sound-ultrawide-en", "sound", "1680x720", "en"),
        game_scene!("game-sound-ultrawide-ja", "sound", "1680x720", "ja"),
        game_scene!("game-subtitles-tablet-en", "subtitles", "1024x768", "en"),
        game_scene!("game-subtitles-tablet-ja", "subtitles", "1024x768", "ja"),
        game_scene!("game-subtitles-wide-en", "subtitles", "1280x720", "en"),
        game_scene!("game-subtitles-wide-ja", "subtitles", "1280x720", "ja"),
        game_scene!("game-subtitles-ultrawide-en", "subtitles", "1680x720", "en"),
        game_scene!("game-subtitles-ultrawide-ja", "subtitles", "1680x720", "ja"),
        game_scene!("game-controls-tablet-en", "controls", "1024x768", "en"),
        game_scene!("game-controls-tablet-ja", "controls", "1024x768", "ja"),
        game_scene!("game-controls-wide-en", "controls", "1280x720", "en"),
        game_scene!("game-controls-wide-ja", "controls", "1280x720", "ja"),
        game_scene!("game-controls-ultrawide-en", "controls", "1680x720", "en"),
        game_scene!("game-controls-ultrawide-ja", "controls", "1680x720", "ja"),
        game_scene!("game-character-tablet-en", "character", "1024x768", "en"),
        game_scene!("game-character-tablet-ja", "character", "1024x768", "ja"),
        game_scene!("game-character-wide-en", "character", "1280x720", "en"),
        game_scene!("game-character-wide-ja", "character", "1280x720", "ja"),
        game_scene!("game-character-ultrawide-en", "character", "1680x720", "en"),
        game_scene!("game-character-ultrawide-ja", "character", "1680x720", "ja"),
        // The game's own show on its flow (M3 G1) in the Lantern Ward, through the sandbox's `--game`, which
        // stands the game as it ships, its look unpinned but for its tier, which a device would choose
        // (CI's Metal runner Desktop-Low, a Mac Desktop-High): its title over the run it would continue, and
        // a continue from a save at the court's checkpoint, a second and a half in.
        Scene {
            name: "game-ward-title",
            args: &[
                "--size",
                "640x360",
                "--capture-ticks",
                "30",
                "--tier",
                "desktop-high",
                "--game",
                "title",
            ],
            thresholds: Thresholds::default(),
        },
        Scene {
            name: "game-ward-checkpoint",
            args: &[
                "--size",
                "640x360",
                "--capture-ticks",
                "90",
                "--tier",
                "desktop-high",
                "--game",
                "continue",
                "--progress",
                "tests/saves/ward-court.progress.ron",
            ],
            thresholds: Thresholds::default(),
        },
        // The court gate's cutscene with its subtitles (M3 Q4, exit test 3): a continue at the court's
        // checkpoint walks back through the gate, which starts the court's encounter and the cutscene, and the
        // frame is mid-line, the warden speaking, in both languages at 4:3, 16:9 and 21:9.
        gate_scene!("game-ward-gate-tablet-en", "640x480", "en"),
        gate_scene!("game-ward-gate-tablet-ja", "640x480", "ja"),
        gate_scene!("game-ward-gate-wide-en", "854x480", "en"),
        gate_scene!("game-ward-gate-wide-ja", "854x480", "ja"),
        gate_scene!("game-ward-gate-ultrawide-en", "1120x480", "en"),
        gate_scene!("game-ward-gate-ultrawide-ja", "1120x480", "ja"),
        // The character screen's inset (M3 U6) on a phone's 19.5:9 at twice the points, on Mobile-Low, a
        // quarter turn round by a drag.
        Scene {
            name: "game-character-phone-en",
            args: &[
                "--size",
                "1688x780",
                "--capture-ticks",
                "2",
                "--tier",
                "mobile-low",
                "--plain-sky",
                "--plain-post",
                "--ao",
                "off",
                "--shadows",
                "off",
                "--fog",
                "off",
                "--ui-plain",
                "--ui-game",
                "character",
                "--ui-locale",
                "en",
                "--ui-scale",
                "2",
                "--ui-turn",
                "0.25",
            ],
            thresholds: Thresholds::default(),
        },
        game_scene!("game-dialogue-tablet-en", "dialogue", "1024x768", "en"),
        game_scene!("game-dialogue-tablet-ja", "dialogue", "1024x768", "ja"),
        game_scene!("game-dialogue-wide-en", "dialogue", "1280x720", "en"),
        game_scene!("game-dialogue-wide-ja", "dialogue", "1280x720", "ja"),
        game_scene!("game-dialogue-ultrawide-en", "dialogue", "1680x720", "en"),
        game_scene!("game-dialogue-ultrawide-ja", "dialogue", "1680x720", "ja"),
        // The dialogue panel on a real conversation (M3 Q2b): the Lantern Ward guide's question, cooked, its
        // answers in the panel's slots and its hidden one hidden, at U7's aspects.
        game_scene!("game-guide-tablet-en", "dialogue", "1024x768", "en", "--ui-dialogue", "1"),
        game_scene!("game-guide-tablet-ja", "dialogue", "1024x768", "ja", "--ui-dialogue", "1"),
        game_scene!("game-guide-wide-en", "dialogue", "1280x720", "en", "--ui-dialogue", "1"),
        game_scene!("game-guide-wide-ja", "dialogue", "1280x720", "ja", "--ui-dialogue", "1"),
        game_scene!("game-guide-ultrawide-en", "dialogue", "1680x720", "en", "--ui-dialogue", "1"),
        game_scene!("game-guide-ultrawide-ja", "dialogue", "1680x720", "ja", "--ui-dialogue", "1"),
        game_scene!("game-hud-tablet-en", "hud", "1024x768", "en", "--hud", "fixed"),
        game_scene!("game-hud-tablet-ja", "hud", "1024x768", "ja", "--hud", "fixed"),
        game_scene!("game-hud-wide-en", "hud", "1280x720", "en", "--hud", "fixed"),
        game_scene!("game-hud-wide-ja", "hud", "1280x720", "ja", "--hud", "fixed"),
        game_scene!("game-hud-ultrawide-en", "hud", "1680x720", "en", "--hud", "fixed"),
        game_scene!("game-hud-ultrawide-ja", "hud", "1680x720", "ja", "--hud", "fixed"),
        // Motion on a game's screen (M3 C3, in place of U5's test menu): the pause screen a tenth of a second
        // into its entrance, and the settings after a walk down their list, focus and its pulse moved with it.
        game_scene!("game-pause-entering-ja", "pause", "1280x720", "ja", "--ui-time", "0.1"),
        game_scene!(
            "game-settings-walked-ja",
            "settings",
            "1280x720",
            "ja",
            "--ui-walk",
            "down,down,down,down,down"
        ),
        // The combat HUD's fixed frame (M2 I2) at the aspect ratios spec §5.17 names, each a phone-
        // sized frame in points: bright midday behind it at 16:9 and with a notch, golden hour at
        // 4:3 and the lantern-lit street at 21:9, so its inks and digits read over all three.
        Scene {
            name: "hud-wide",
            args: &[
                "--capture-ticks",
                "90",
                "--look-dev",
                "wide",
                "--time-of-day",
                "13",
                "--tier",
                "desktop-high",
                "--aa",
                "msaa",
                "--ao",
                "off",
                "--hud",
                "fixed",
            ],
            thresholds: Thresholds::default(),
        },
        Scene {
            name: "hud-tablet",
            args: &[
                "--size",
                "480x360",
                "--capture-ticks",
                "90",
                "--look-dev",
                "hero",
                "--time-of-day",
                "18",
                "--tier",
                "desktop-high",
                "--aa",
                "msaa",
                "--ao",
                "off",
                "--hud",
                "fixed",
            ],
            thresholds: Thresholds::default(),
        },
        Scene {
            name: "hud-ultrawide",
            args: &[
                "--size",
                "840x360",
                "--capture-ticks",
                "90",
                "--look-dev",
                "street",
                "--time-of-day",
                "21",
                "--tier",
                "desktop-high",
                "--aa",
                "msaa",
                "--ao",
                "off",
                "--hud",
                "fixed",
            ],
            thresholds: Thresholds::default(),
        },
        // An iPhone's frame in points with its insets: the left and right notch bands and the home
        // indicator's strip stay clear, the controls just inside them.
        Scene {
            name: "hud-notch",
            args: &[
                "--size",
                "874x402",
                "--capture-ticks",
                "90",
                "--look-dev",
                "wide",
                "--time-of-day",
                "13",
                "--tier",
                "desktop-high",
                "--aa",
                "msaa",
                "--ao",
                "off",
                "--hud",
                "fixed",
                "--hud-insets",
                "62,0,62,21",
            ],
            thresholds: Thresholds::default(),
        },
        Scene {
            name: "display-text-latin",
            args: &[
                "--capture-ticks",
                "90",
                "--look-dev",
                "street",
                "--time-of-day",
                "21",
                "--tier",
                "desktop-high",
                "--aa",
                "msaa",
                "--ao",
                "off",
                "--display-text",
                "latin",
            ],
            thresholds: Thresholds::default(),
        },
        Scene {
            name: "display-text-japanese",
            args: &[
                "--size",
                "1920x1080",
                "--capture-ticks",
                "90",
                "--look-dev",
                "street",
                "--time-of-day",
                "21",
                "--tier",
                "desktop-high",
                "--aa",
                "msaa",
                "--ao",
                "off",
                "--display-text",
                "japanese",
            ],
            thresholds: Thresholds::default(),
        },
        // The fixed frame with the player's ultimate (M3 Q3c): its energy meter empty and its button
        // charging, then full with the button lit, on the wide frame and on the iPhone's.
        Scene {
            name: "hud-energy-empty-wide",
            args: &[
                "--capture-ticks",
                "90",
                "--look-dev",
                "wide",
                "--time-of-day",
                "13",
                "--tier",
                "desktop-high",
                "--aa",
                "msaa",
                "--ao",
                "off",
                "--hud",
                "fixed",
                "--hud-energy",
                "0",
            ],
            thresholds: Thresholds::default(),
        },
        Scene {
            name: "hud-energy-full-wide",
            args: &[
                "--capture-ticks",
                "90",
                "--look-dev",
                "wide",
                "--time-of-day",
                "13",
                "--tier",
                "desktop-high",
                "--aa",
                "msaa",
                "--ao",
                "off",
                "--hud",
                "fixed",
                "--hud-energy",
                "1",
            ],
            thresholds: Thresholds::default(),
        },
        Scene {
            name: "hud-energy-empty-notch",
            args: &[
                "--size",
                "874x402",
                "--capture-ticks",
                "90",
                "--look-dev",
                "wide",
                "--time-of-day",
                "13",
                "--tier",
                "desktop-high",
                "--aa",
                "msaa",
                "--ao",
                "off",
                "--hud",
                "fixed",
                "--hud-insets",
                "62,0,62,21",
                "--hud-energy",
                "0",
            ],
            thresholds: Thresholds::default(),
        },
        Scene {
            name: "hud-energy-full-notch",
            args: &[
                "--size",
                "874x402",
                "--capture-ticks",
                "90",
                "--look-dev",
                "wide",
                "--time-of-day",
                "13",
                "--tier",
                "desktop-high",
                "--aa",
                "msaa",
                "--ao",
                "off",
                "--hud",
                "fixed",
                "--hud-insets",
                "62,0,62,21",
                "--hud-energy",
                "1",
            ],
            thresholds: Thresholds::default(),
        },
        // The look at each tier and at its dynamic floor (M4 T0): the close-up of the character's face at noon, on
        // each tier as it ships, AO and all. The desktop tiers draw 640×360 at scale 1 and at 0.7; the phones'
        // draw 1280×592 at the scales a 1170-row phone draws, their own (740 or 590 rows) and 0.75 of it, so
        // the bilinear upscale is a phone's. The thermal ladder's rungs (M4 T2) compare against these.
        Scene {
            name: "tier-face-desktop-high",
            args: &[
                "--capture-ticks",
                "90",
                "--look-dev",
                "face",
                "--time-of-day",
                "13",
                "--tier",
                "desktop-high",
                "--capture-frames",
                "12",
            ],
            thresholds: Thresholds::default(),
        },
        Scene {
            name: "tier-face-desktop-high-floor",
            args: &[
                "--capture-ticks",
                "90",
                "--look-dev",
                "face",
                "--time-of-day",
                "13",
                "--tier",
                "desktop-high",
                "--render-scale",
                "0.7",
                "--capture-frames",
                "12",
            ],
            thresholds: Thresholds::default(),
        },
        Scene {
            name: "tier-face-desktop-low",
            args: &[
                "--capture-ticks",
                "90",
                "--look-dev",
                "face",
                "--time-of-day",
                "13",
                "--tier",
                "desktop-low",
                "--capture-frames",
                "12",
            ],
            thresholds: Thresholds::default(),
        },
        Scene {
            name: "tier-face-desktop-low-floor",
            args: &[
                "--capture-ticks",
                "90",
                "--look-dev",
                "face",
                "--time-of-day",
                "13",
                "--tier",
                "desktop-low",
                "--render-scale",
                "0.7",
                "--capture-frames",
                "12",
            ],
            thresholds: Thresholds::default(),
        },
        Scene {
            name: "tier-face-mobile-high",
            args: &[
                "--size",
                "1280x592",
                "--capture-ticks",
                "90",
                "--look-dev",
                "face",
                "--time-of-day",
                "13",
                "--tier",
                "mobile-high",
                "--render-scale",
                "0.632479",
            ],
            thresholds: Thresholds::default(),
        },
        Scene {
            name: "tier-face-mobile-high-floor",
            args: &[
                "--size",
                "1280x592",
                "--capture-ticks",
                "90",
                "--look-dev",
                "face",
                "--time-of-day",
                "13",
                "--tier",
                "mobile-high",
                "--render-scale",
                "0.474359",
            ],
            thresholds: Thresholds::default(),
        },
        Scene {
            name: "tier-face-mobile-low",
            args: &[
                "--size",
                "1280x592",
                "--capture-ticks",
                "90",
                "--look-dev",
                "face",
                "--time-of-day",
                "13",
                "--tier",
                "mobile-low",
                "--render-scale",
                "0.504274",
            ],
            thresholds: Thresholds::default(),
        },
        Scene {
            name: "tier-face-mobile-low-floor",
            args: &[
                "--size",
                "1280x592",
                "--capture-ticks",
                "90",
                "--look-dev",
                "face",
                "--time-of-day",
                "13",
                "--tier",
                "mobile-low",
                "--render-scale",
                "0.378205",
            ],
            thresholds: Thresholds::default(),
        },
        // The Lantern Ward's street from where a game starts, and a medium hit's spark and aberration on the
        // look-dev character, on the low tiers that the ladder's passes are measured on.
        Scene {
            name: "tier-ward-desktop-low",
            args: &[
                "--capture-ticks",
                "90",
                "--zone",
                "lantern_ward:player_start",
                "--time-of-day",
                "14",
                "--tier",
                "desktop-low",
                "--capture-frames",
                "12",
            ],
            thresholds: Thresholds::default(),
        },
        Scene {
            name: "tier-ward-mobile-low",
            args: &[
                "--size",
                "1280x592",
                "--capture-ticks",
                "90",
                "--zone",
                "lantern_ward:player_start",
                "--time-of-day",
                "14",
                "--tier",
                "mobile-low",
                "--render-scale",
                "0.504274",
            ],
            thresholds: Thresholds::default(),
        },
        Scene {
            name: "tier-hit-desktop-low",
            args: &[
                "--capture-ticks",
                "71",
                "--look-dev",
                "hero",
                "--time-of-day",
                "12",
                "--replay",
                "tests/replays/fight.sreplay",
                "--cues",
                "--tier",
                "desktop-low",
                "--capture-frames",
                "12",
                "--capture-step",
                "1",
            ],
            thresholds: Thresholds::default(),
        },
        Scene {
            name: "tier-hit-mobile-low",
            args: &[
                "--size",
                "1280x592",
                "--capture-ticks",
                "71",
                "--look-dev",
                "hero",
                "--time-of-day",
                "12",
                "--replay",
                "tests/replays/fight.sreplay",
                "--cues",
                "--tier",
                "mobile-low",
                "--render-scale",
                "0.504274",
            ],
            thresholds: Thresholds::default(),
        },
    ]
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Compare,
    Update,
}

/// Where the runner finds and puts things.
pub struct Paths {
    /// The sandbox executable.
    pub sandbox: PathBuf,
    /// Cooked desktop content.
    pub content: PathBuf,
    /// `tests/golden`.
    pub goldens: PathBuf,
    /// Captures, actuals, diffs, the scenes' logs and `timings.json`; emptied first. Its tree
    /// must be disjoint from all inputs, including existing symlink aliases.
    pub out: PathBuf,
}

/// Which scenes a run renders.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Selection<'a> {
    All,
    /// One scene, by name.
    Scene(&'a str),
    /// One of several slices of the list, so CI can split a backend's scenes across runners.
    Shard(Shard),
}

/// Slice `index` of `count` (from 1), split by what the scenes cost on the run's backend family
/// (review BC-107): costliest first, each scene joins the slice with the least so far, and each
/// slice keeps the list's order. On lavapipe and WARP a scene takes from under a second to two
/// minutes, so dealing the scenes out in turn, as the runner did before, left one of lavapipe's two
/// slices taking 10.4 minutes against the other's 3.7 to 7.
///
/// The costs are the CI times of the family that `SAPPHIRE_GPU_BACKEND` forces (`vulkan`
/// lavapipe's, `dx12` WARP's), kept in the `costs` module. Without them (another backend, or a
/// local run that forces none) every scene costs the same, which deals them out in turn. The split
/// depends on nothing else, so each of a family's runners computes the same one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Shard {
    pub index: usize,
    pub count: usize,
}

impl std::str::FromStr for Shard {
    type Err = String;

    /// Parses `<index>/<count>`, such as `2/4`.
    fn from_str(text: &str) -> Result<Self, String> {
        let invalid = || format!("invalid shard {text:?}: expected <index>/<count> from 1, such as 2/4");
        let (index, count) = text.split_once('/').ok_or_else(invalid)?;
        let index = index.parse::<usize>().map_err(|_| invalid())?;
        let count = count.parse::<usize>().map_err(|_| invalid())?;
        if index == 0 || index > count {
            return Err(invalid());
        }
        Ok(Shard { index, count })
    }
}

/// Which of `count` slices (from 0) each scene goes to, by position, given each scene's cost:
/// costliest first (equal costs in list order), each to the slice with the least cost so far (the
/// first of equals). Equal costs deal the scenes out in turn.
fn split(costs: &[u32], count: usize) -> Vec<usize> {
    let mut order: Vec<usize> = (0..costs.len()).collect();
    order.sort_by_key(|&position| (std::cmp::Reverse(costs[position]), position));
    let mut loads = vec![0_u64; count];
    let mut slices = vec![0; costs.len()];
    for position in order {
        let lightest = (0..count).min_by_key(|&slice| (loads[slice], slice)).expect("a shard count from 1");
        loads[lightest] += u64::from(costs[position]);
        slices[position] = lightest;
    }
    slices
}

/// Each scene's cost on `family`, in tenths of a second; all equal without a family.
fn costs(scenes: &[Scene], family: Option<Family>) -> Vec<u32> {
    scenes.iter().map(|scene| family.map_or(1, |family| family.cost(scene.name))).collect()
}

/// Runs the selected scenes and returns one message per failure (empty: all passed). Each scene
/// gets `timeout` (`--scene-timeout`; [`DEFAULT_SCENE_TIMEOUT`]).
pub fn run(
    paths: &Paths,
    mode: Mode,
    selection: Selection<'_>,
    timeout: Duration,
) -> Result<Vec<String>, String> {
    let scenes = scenes();
    let family = std::env::var(BACKEND_ENV).ok().and_then(|backend| Family::from_backend(&backend));
    let selected = select(&scenes, selection, family)?;
    let sandbox = |_: &Scene| {
        let mut command = Command::new(&paths.sandbox);
        command.env(CONTENT_DIR_ENV, &paths.content);
        command
    };
    run_scenes(&selected, paths, mode, timeout, &sandbox)
}

/// The scenes `selection` names, in their order.
fn select<'s>(
    scenes: &'s [Scene],
    selection: Selection<'_>,
    family: Option<Family>,
) -> Result<Vec<&'s Scene>, String> {
    match selection {
        Selection::All => Ok(scenes.iter().collect()),
        Selection::Scene(name) => match scenes.iter().find(|scene| scene.name == name) {
            Some(scene) => Ok(vec![scene]),
            None => {
                let names: Vec<&str> = scenes.iter().map(|scene| scene.name).collect();
                Err(format!("no golden scene named {name:?}; scenes: {}", names.join(", ")))
            }
        },
        Selection::Shard(shard) => {
            let costs = costs(scenes, family);
            let slices = split(&costs, shard.count);
            let held: Vec<usize> =
                (0..scenes.len()).filter(|&position| slices[position] == shard.index - 1).collect();
            if held.is_empty() {
                return Err(format!(
                    "shard {}/{} holds none of the {} scenes",
                    shard.index,
                    shard.count,
                    scenes.len()
                ));
            }
            let (index, count, total) = (shard.index, shard.count, scenes.len());
            match family {
                Some(family) => {
                    let minutes = |tenths: u64| tenths as f64 / 600.0;
                    let ours: u64 = held.iter().map(|&position| u64::from(costs[position])).sum();
                    let all: u64 = costs.iter().copied().map(u64::from).sum();
                    eprintln!(
                        "shard {index}/{count}: {} of {total} scenes, split by {}'s CI times: about {:.1} of their {:.1} \
                         minutes",
                        held.len(),
                        family.name(),
                        minutes(ours),
                        minutes(all)
                    );
                }
                None => eprintln!(
                    "shard {index}/{count}: {} of {total} scenes, dealt out in turn ({BACKEND_ENV} names no family with \
                     CI times)",
                    held.len()
                ),
            }
            Ok(held.into_iter().map(|position| &scenes[position]).collect())
        }
    }
}

/// How a scene ended, as `timings.json` records it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Status {
    /// Within its gates.
    Ok,
    /// Its golden written (`--update`).
    Updated,
    /// No golden for its family: a failure in CI, skipped locally.
    NoGolden,
    /// Outside its gates.
    Failed,
    /// The sandbox failed or left no adapter record, or its capture or golden can't be read or
    /// compared.
    Error,
    /// Killed at its deadline.
    TimedOut,
}

impl Status {
    fn name(self) -> &'static str {
        match self {
            Self::Ok => "ok",
            Self::Updated => "updated",
            Self::NoGolden => "no-golden",
            Self::Failed => "failed",
            Self::Error => "error",
            Self::TimedOut => "timed-out",
        }
    }
}

/// One scene's result.
struct Outcome {
    status: Status,
    /// Its backend family, when its sandbox wrote an adapter record.
    family: Option<String>,
    failure: Option<String>,
}

/// One scene's entry in `timings.json`.
struct Timing {
    scene: &'static str,
    family: Option<String>,
    seconds: f64,
    status: Status,
}

/// Runs `selected` with the sandbox `sandbox` starts for each (its program and environment; the
/// capture's arguments are added here).
fn run_scenes(
    selected: &[&Scene],
    paths: &Paths,
    mode: Mode,
    timeout: Duration,
    sandbox: &dyn Fn(&Scene) -> Command,
) -> Result<Vec<String>, String> {
    crate::output_paths::disjoint(&paths.out, &[&paths.goldens, &paths.content, &paths.sandbox])?;
    if paths.out.exists() {
        fs::remove_dir_all(&paths.out).map_err(|e| io_error("failed to empty", &paths.out, e))?;
    }
    fs::create_dir_all(&paths.out).map_err(|e| io_error("failed to create", &paths.out, e))?;
    let in_ci = std::env::var_os("CI").is_some();
    let timings_path = paths.out.join("timings.json");

    let run_clock = Stopwatch::start();
    let mut failures = Vec::new();
    let mut timings = Vec::with_capacity(selected.len());
    for &scene in selected {
        let clock = Stopwatch::start();
        let outcome = run_scene(scene, paths, mode, timeout, in_ci, sandbox, &clock)?;
        failures.extend(outcome.failure);
        timings.push(Timing {
            scene: scene.name,
            family: outcome.family,
            seconds: clock.elapsed().as_secs_f64(),
            status: outcome.status,
        });
        write_timings(&timings_path, &timings)?;
    }
    eprintln!(
        "{} scene(s) in {:.1} min; their times are in {}",
        timings.len(),
        run_clock.elapsed().as_secs_f64() / 60.0,
        timings_path.display()
    );
    Ok(failures)
}

/// What the sandbox is given for an argument of a scene. A path under `tests/`, such as a committed
/// replay's, is found from the goldens, which sit there too, wherever the run was started from.
fn scene_arg(arg: &str, paths: &Paths) -> OsString {
    match (arg.strip_prefix("tests/"), paths.goldens.parent()) {
        (Some(path), Some(tests)) => tests.join(path).into_os_string(),
        _ => arg.into(),
    }
}

/// Renders `scene` and checks it, printing one line with its time. `Err` stops the whole run: the
/// sandbox can't be started, or the output can't be written. A scene that fails is an [`Outcome`].
fn run_scene(
    scene: &Scene,
    paths: &Paths,
    mode: Mode,
    timeout: Duration,
    in_ci: bool,
    sandbox: &dyn Fn(&Scene) -> Command,
    clock: &Stopwatch,
) -> Result<Outcome, String> {
    let seconds = || clock.elapsed().as_secs_f64();
    let capture = paths.out.join(format!("{}.png", scene.name));
    let log = paths.out.join(format!("{}.log", scene.name));
    let mut command = sandbox(scene);
    command.arg("--capture").arg(&capture).args(["--size", SIZE]);
    command.args(scene.args.iter().map(|arg| scene_arg(arg, paths)));
    let exit = run_with_deadline(&mut command, &log, timeout)
        .map_err(|e| format!("failed to run the sandbox for {} ({}): {e}", scene.name, log.display()))?;
    let error = |status: Status, family: Option<String>, what: String| {
        eprintln!("{:<8} {}: {what} (log: {})", status.name(), scene.name, log.display());
        Outcome {
            status,
            family,
            failure: Some(format!(
                "{}: {what}; its log, {}, ends:\n{}",
                scene.name,
                log.display(),
                log_tail(&log)
            )),
        }
    };
    let Some(exit) = exit else {
        return Ok(error(Status::TimedOut, None, format!("timed out after {} s", timeout.as_secs())));
    };
    if !exit.success() {
        let family = backend_family(&capture).ok();
        return Ok(error(
            Status::Error,
            family,
            format!("the sandbox failed ({exit}) after {:.1} s", seconds()),
        ));
    }
    let family = match backend_family(&capture) {
        Ok(family) => family,
        Err(message) => return Ok(error(Status::Error, None, message)),
    };
    if let Some(builds) = fs::read_to_string(&log).ok().and_then(|text| frame_builds(&text, scene)) {
        return Ok(error(Status::Error, Some(family), builds));
    }
    let outcome =
        |status: Status, failure: Option<String>| Outcome { status, family: Some(family.clone()), failure };
    let golden_path = paths.goldens.join(format!("{}.{family}.png", scene.name));
    let kept = paths.out.join(format!("{}.{family}.actual.png", scene.name));
    let keep = || fs::copy(&capture, &kept).map(drop).map_err(|e| io_error("failed to write", &kept, e));
    // This scene's failure, not the run's: a golden of another size, a capture cut short.
    let unreadable = |why: golden::GoldenError| -> Result<Outcome, String> {
        let what = if capture.is_file() {
            keep()?;
            format!("{why}; the capture is at {}", kept.display())
        } else {
            why.to_string()
        };
        Ok(error(Status::Error, Some(family.clone()), what))
    };

    if mode == Mode::Update {
        // Read first: a capture cut short would pass for a golden until a run compared against it.
        if let Err(why) = Image::read_png(&capture) {
            return unreadable(why);
        }
        fs::create_dir_all(&paths.goldens).map_err(|e| io_error("failed to create", &paths.goldens, e))?;
        fs::copy(&capture, &golden_path).map_err(|e| io_error("failed to write", &golden_path, e))?;
        eprintln!("updated  {} {:.1} s", golden_path.display(), seconds());
        return Ok(outcome(Status::Updated, None));
    }
    if !golden_path.is_file() {
        keep()?;
        let message = format!(
            "{} [{family}] {:.1} s: no golden yet; the capture is at {} (`cargo xtask golden --update` adds it)",
            scene.name,
            seconds(),
            kept.display()
        );
        eprintln!("{:<8} {message}", if in_ci { "failed" } else { "skipped" });
        return Ok(outcome(Status::NoGolden, in_ci.then_some(message)));
    }

    let compared = Image::read_png(&golden_path).and_then(|golden| {
        let actual = Image::read_png(&capture)?;
        golden::compare(&golden, &actual, &scene.thresholds)
    });
    let comparison = match compared {
        Ok(comparison) => comparison,
        Err(why) => return unreadable(why),
    };
    let summary = format!(
        "{} [{family}] {:.1} s: mean {:.4} (≤ {}), p{} {:.4} (≤ {}), worst tile {:.4} (≤ {}), max {:.3}",
        scene.name,
        seconds(),
        comparison.mean,
        scene.thresholds.mean,
        comparison.percentile,
        comparison.percentile_value,
        scene.thresholds.percentile_max,
        comparison.worst_tile,
        scene.thresholds.tile_max,
        comparison.max,
    );
    if comparison.passes(&scene.thresholds) {
        eprintln!("ok       {summary}");
        return Ok(outcome(Status::Ok, None));
    }
    keep()?;
    let diff = paths.out.join(format!("{}.{family}.diff.png", scene.name));
    comparison.error_map.write_png(&diff).map_err(|e| e.to_string())?;
    eprintln!("failed   {summary}");
    Ok(outcome(Status::Failed, Some(format!("{summary}\n    see {} and {}", kept.display(), diff.display()))))
}

/// Runs `command` with its output going to a new file at `log`, for at most `timeout`: the exit
/// status, or `None` once a command that ran out of time has been killed and reaped.
///
/// A file, not a pipe: the sandbox's crash monitor inherits its output, so a pipe would stay open
/// after the sandbox had gone, and an undrained one stalls the sandbox once it fills (64 KiB on
/// Linux). Only the sandbox is killed: its crash monitor exits when its lifeline, a pipe only the
/// sandbox holds, closes (`sapphire_crash`). A kill is `SIGKILL` (`TerminateProcess` on Windows),
/// so a hung sandbox leaves no minidump; std has no portable way to send `SIGABRT` first.
fn run_with_deadline(command: &mut Command, log: &Path, timeout: Duration) -> io::Result<Option<ExitStatus>> {
    let stdout = File::create(log)?;
    let stderr = stdout.try_clone()?;
    let mut child = command.stdin(Stdio::null()).stdout(stdout).stderr(stderr).spawn()?;
    let waited = wait_with_deadline(&mut child, timeout);
    if waited.is_err() {
        // Never leave it running.
        let _ = child.kill();
        let _ = child.wait();
    }
    waited
}

fn wait_with_deadline(child: &mut Child, timeout: Duration) -> io::Result<Option<ExitStatus>> {
    let clock = Stopwatch::start();
    let mut delay = FIRST_POLL;
    loop {
        if let Some(status) = child.try_wait()? {
            return Ok(Some(status));
        }
        let elapsed = clock.elapsed();
        if elapsed >= timeout {
            // An error means it exited meanwhile; the wait reaps it either way.
            let _ = child.kill();
            child.wait()?;
            return Ok(None);
        }
        std::thread::sleep(delay.min(timeout - elapsed));
        delay = (delay * 2).min(MAX_POLL);
    }
}

/// The last [`LOG_TAIL_LINES`] lines of a scene's log, indented, for its failure.
/// What a scene's frames built that its load didn't declare (M3 K1), from its log's "captured frame" line:
/// `None` if nothing, or if the scene shows a debug view. A capture counts what its load builds as prewarmed,
/// so a first use there is a frame's, which a game's frame would stall on: the load's inventory, or what the
/// scene declares, should list it. A debug view alone builds in its frame: it copies what that frame draws
/// into its own target, and no load declares a view a tool turns on.
fn frame_builds(log: &str, scene: &Scene) -> Option<String> {
    if scene.args.contains(&"--debug-view") {
        return None;
    }
    let line = log.lines().rfind(|line| line.contains("captured frame"))?;
    let count: u32 = line.split_once(" first_use=")?.1.split(' ').next()?.parse().ok()?;
    (count > 0).then(|| {
        format!(
            "its frames built {count} pipelines its load didn't declare; its log's \"built on first use\" lines \
             name them"
        )
    })
}

fn log_tail(path: &Path) -> String {
    const MAX_BYTES: u64 = 16 * 1024;
    let read = || -> io::Result<String> {
        let mut file = File::open(path)?;
        let start = file.metadata()?.len().saturating_sub(MAX_BYTES);
        file.seek(SeekFrom::Start(start))?;
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes)?;
        let text = String::from_utf8_lossy(&bytes);
        // A read from the middle starts inside a line.
        let lines: Vec<&str> = text.lines().skip(usize::from(start > 0)).collect();
        let first = lines.len().saturating_sub(LOG_TAIL_LINES);
        Ok(lines[first..].iter().fold(String::new(), |mut tail, line| {
            let _ = writeln!(tail, "    {line}");
            tail
        }))
    };
    match read() {
        Ok(tail) if tail.is_empty() => "    (the log is empty)\n".to_owned(),
        Ok(tail) => tail,
        Err(error) => format!("    (the log can't be read: {error})\n"),
    }
}

/// Writes every scene's time so far as a JSON array of `{scene, family, seconds, status}`. A run
/// renders every scene on one backend, so a scene that left no adapter record (it timed out, say)
/// takes the family the others named.
fn write_timings(path: &Path, timings: &[Timing]) -> Result<(), String> {
    let run_family = timings.iter().find_map(|timing| timing.family.as_deref());
    let mut json = String::from("[\n");
    for (i, timing) in timings.iter().enumerate() {
        let family = timing.family.as_deref().or(run_family).map_or_else(|| "null".to_owned(), json_string);
        let comma = if i + 1 < timings.len() { "," } else { "" };
        let _ = writeln!(
            json,
            "  {{\"scene\": {}, \"family\": {family}, \"seconds\": {:.3}, \"status\": \"{}\"}}{comma}",
            json_string(timing.scene),
            timing.seconds,
            timing.status.name()
        );
    }
    json.push_str("]\n");
    fs::write(path, json).map_err(|e| io_error("failed to write", path, e))
}

/// `text` as a JSON string.
fn json_string(text: &str) -> String {
    let mut json = String::with_capacity(text.len() + 2);
    json.push('"');
    for c in text.chars() {
        match c {
            '"' => json.push_str("\\\""),
            '\\' => json.push_str("\\\\"),
            c if u32::from(c) < 0x20 => {
                let _ = write!(json, "\\u{:04x}", u32::from(c));
            }
            c => json.push(c),
        }
    }
    json.push('"');
    json
}

fn io_error(what: &str, path: &Path, error: io::Error) -> String {
    format!("{what} {}: {error}", path.display())
}

/// The backend family of a capture, from the `<capture>.adapter.txt` the sandbox writes.
fn backend_family(capture: &Path) -> Result<String, String> {
    let path = capture.with_extension("adapter.txt");
    let text = fs::read_to_string(&path).map_err(|e| format!("failed to read {}: {e}", path.display()))?;
    let field = |key: &str| {
        text.lines()
            .find_map(|line| line.strip_prefix(key)?.strip_prefix('='))
            .ok_or_else(|| format!("{} has no `{key}=` line", path.display()))
    };
    Ok(backend_key(field("backend")?, field("adapter")?))
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_frame_s_first_use_fails_a_scene_unless_it_shows_a_debug_view() {
        let line = |count: u32| {
            format!(
                "INFO sapphire::sandbox: captured frame path=x.png ticks=90 prewarmed=40 first_use={count}\n"
            )
        };
        let plain =
            Scene { name: "plain", args: &["--capture-ticks", "90"], thresholds: Thresholds::default() };
        let viewed =
            Scene { name: "viewed", args: &["--debug-view", "wireframe"], thresholds: Thresholds::default() };
        assert_eq!(frame_builds(&line(0), &plain), None);
        assert!(frame_builds(&line(3), &plain).is_some_and(|why| why.contains("built 3 pipelines")));
        assert_eq!(frame_builds(&line(3), &viewed), None, "a debug view builds in its frame");
        assert_eq!(frame_builds("INFO sapphire::sandbox: contact sheet written\n", &plain), None);
    }

    use std::io::Write as _;

    use super::*;

    #[test]
    fn scene_names_are_unique_file_stems() {
        let scenes = scenes();
        let mut names: Vec<&str> = scenes.iter().map(|scene| scene.name).collect();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), scenes.len());
        assert!(names.iter().all(|name| name.chars().all(|c| c.is_ascii_lowercase() || c == '-')));
    }

    #[test]
    fn shards_parse_from_one() {
        assert_eq!("2/4".parse(), Ok(Shard { index: 2, count: 4 }));
        assert_eq!("1/1".parse(), Ok(Shard { index: 1, count: 1 }));
        for invalid in ["0/4", "5/4", "1/0", "2", "a/4", "2/b", "", "/", "-1/4"] {
            assert!(invalid.parse::<Shard>().is_err(), "{invalid:?} parsed");
        }
    }

    /// The shard counts CI runs (lavapipe 2, WARP 4), one, and a few more.
    const SHARD_COUNTS: [usize; 6] = [1, 2, 3, 4, 5, 6];
    const FAMILIES: [Option<Family>; 3] = [None, Some(Family::Lavapipe), Some(Family::Warp)];

    /// Each shard's scenes, by name, as `select` picks them.
    fn shards(scenes: &[Scene], count: usize, family: Option<Family>) -> Vec<Vec<&'static str>> {
        (1..=count)
            .map(|index| {
                let held =
                    select(scenes, Selection::Shard(Shard { index, count }), family).unwrap_or_default();
                held.iter().map(|scene| scene.name).collect()
            })
            .collect()
    }

    #[test]
    fn shards_hold_every_scene_once_in_order() {
        let scenes = scenes();
        let names: Vec<&str> = scenes.iter().map(|scene| scene.name).collect();
        for family in FAMILIES {
            for count in SHARD_COUNTS {
                let shards = shards(&scenes, count, family);
                assert!(
                    shards.iter().all(|shard| !shard.is_empty()),
                    "{family:?}, {count} shards: an empty one"
                );
                for shard in &shards {
                    let positions: Vec<usize> =
                        shard.iter().map(|name| names.iter().position(|n| n == name).unwrap()).collect();
                    assert!(positions.is_sorted(), "{family:?}, {count} shards: out of order");
                }
                let mut held: Vec<&str> = shards.concat();
                held.sort_unstable();
                let mut all = names.clone();
                all.sort_unstable();
                assert_eq!(held, all, "{family:?}, {count} shards: every scene exactly once");
            }
        }
    }

    #[test]
    fn without_costs_shards_deal_the_scenes_out_in_turn() {
        // As the runner split them before it had costs.
        for count in SHARD_COUNTS {
            let slices = split(&[1; 95], count);
            assert!(slices.iter().enumerate().all(|(position, &slice)| slice == position % count), "{count}");
        }
        assert_eq!(Family::from_backend("metal"), None);
        assert_eq!(Family::from_backend("Vulkan"), Some(Family::Lavapipe));
        assert_eq!(Family::from_backend("dx12"), Some(Family::Warp));
    }

    #[test]
    fn a_scene_without_a_cost_still_lands_in_one_shard() {
        let mut scenes: Vec<Scene> = scenes().into_iter().collect();
        scenes.insert(3, Scene { name: "scene-added-since", args: &[], thresholds: Thresholds::default() });
        for family in [Family::Lavapipe, Family::Warp] {
            let costliest = costs::COSTS.iter().map(|cost| family.cost(cost.scene)).max().unwrap();
            assert_eq!(family.cost("scene-added-since"), costliest, "counts as the costliest");
            for count in SHARD_COUNTS {
                let shards = shards(&scenes, count, Some(family));
                let holding = shards.iter().filter(|shard| shard.contains(&"scene-added-since")).count();
                assert_eq!(holding, 1, "{family:?}, {count} shards");
                assert_eq!(shards.iter().map(Vec::len).sum::<usize>(), scenes.len());
            }
        }
    }

    #[test]
    fn the_costs_name_only_scenes_that_exist_each_once() {
        let names: Vec<&str> = scenes().iter().map(|scene| scene.name).collect();
        let mut listed: Vec<&str> = costs::COSTS.iter().map(|cost| cost.scene).collect();
        for scene in &listed {
            assert!(names.contains(scene), "the costs name {scene:?}, which is no longer a scene: remove it");
        }
        listed.sort_unstable();
        let before = listed.len();
        listed.dedup();
        assert_eq!(listed.len(), before, "a scene listed twice");
        assert!(costs::COSTS.iter().all(|cost| cost.lavapipe > 0.0 && cost.warp > 0.0));
    }

    #[test]
    fn splitting_by_cost_shortens_ci_s_slowest_shard() {
        // CI's split: lavapipe in 3, WARP in 6.
        let scenes = scenes();
        for (family, count) in [(Family::Lavapipe, 3), (Family::Warp, 6)] {
            let costs = costs(&scenes, Some(family));
            let slowest = |slices: &[usize]| {
                let mut loads = vec![0_u64; count];
                slices.iter().zip(&costs).for_each(|(&slice, &cost)| loads[slice] += u64::from(cost));
                loads.into_iter().max().unwrap()
            };
            let in_turn: Vec<usize> = (0..scenes.len()).map(|position| position % count).collect();
            let by_cost = slowest(&split(&costs, count));
            assert!(by_cost < slowest(&in_turn), "{family:?}");
            // Longest-first is within 4/3 of the best split, and an equal share bounds that below.
            let share = costs.iter().copied().map(u64::from).sum::<u64>().div_ceil(count as u64);
            assert!(by_cost * 3 <= share * 4, "{family:?}: {by_cost} against a share of {share}");
        }
    }

    #[test]
    fn only_the_ambient_scenes_take_ambient_occlusion_and_a_grid() {
        // Every scene from before M1 W5 pins AO off and loads no grid, so it draws as it did; the
        // ambient scenes pin fog off instead. A zone's brings its grid and pins nothing: it's its look.
        for scene in scenes() {
            let pinned = |flag: &str, value: &str| scene.args.windows(2).any(|pair| pair == [flag, value]);
            if scene.name.starts_with("ambient-") {
                assert!(pinned("--fog", "off"), "{}", scene.name);
            } else if scene.name.starts_with("zone-") {
                assert!(scene.args.contains(&"--zone"), "{}", scene.name);
            } else if scene.name.starts_with("game-ward-") {
                // The game as it ships, on the zone's own look, at a tier no device chooses for it.
                assert!(scene.args.contains(&"--game"), "{}", scene.name);
                assert!(pinned("--tier", "desktop-high"), "{}", scene.name);
            } else if scene.name.starts_with("tier-") {
                // A tier as it ships (M4 T0), whose GTAO the thermal ladder's rungs shed.
                assert!(scene.args.contains(&"--tier") && !scene.args.contains(&"--ao"), "{}", scene.name);
            } else {
                assert!(pinned("--ao", "off"), "{}", scene.name);
                assert!(!scene.args.contains(&"--probes"), "{}", scene.name);
            }
        }
    }

    #[test]
    fn only_the_hud_scenes_draw_the_interface_and_they_cover_spec_5_17_s_aspect_ratios() {
        let mut sizes = Vec::new();
        for scene in scenes() {
            let hud = scene.args.contains(&"--hud");
            // `arena-fight-hit`, `arena-skirmish`, `arena-boss`, `arena-captain` and the ward's fights show the
            // bars reading a real fight.
            let expected = scene.name.starts_with("hud-")
                || scene.name.starts_with("game-hud-")
                || ["arena-fight-hit", "arena-skirmish", "arena-boss", "arena-captain"].contains(&scene.name)
                || ["fight-ward-market", "fight-ward-terrace"].contains(&scene.name);
            assert_eq!(hud, expected, "{}", scene.name);
            let display_text = scene.args.contains(&"--display-text");
            assert_eq!(display_text, scene.name.starts_with("display-text-"), "{}", scene.name);
            if scene.name.starts_with("game-") {
                continue;
            }
            if hud {
                let size =
                    scene.args.windows(2).find(|pair| pair[0] == "--size").map_or(SIZE, |pair| pair[1]);
                sizes.push(size);
            } else if !display_text && !scene.name.starts_with("tier-") {
                // The tier scenes (M4 T0) draw a phone's frame at a phone's shape.
                assert!(!scene.args.iter().any(|arg| arg.starts_with("--size")), "{}", scene.name);
            }
        }
        for (ratio, size) in [("4:3", "480x360"), ("16:9", "640x360"), ("21:9", "840x360")] {
            assert!(sizes.contains(&size), "no HUD scene at {ratio}");
        }
    }

    /// Spec §5.17's aspect ratios again, for the game's screens: Japanese in all three, as exit test 4 wants.
    #[test]
    fn the_screens_scenes_cover_the_aspect_ratios_in_japanese() {
        let size_of = |name: &str| {
            let scene = scenes().into_iter().find(|scene| scene.name == name).expect("the scene");
            assert!(scene.args.windows(2).any(|pair| pair == ["--ui-locale", "ja"]), "{name}");
            let size = scene.args.windows(2).find(|pair| pair[0] == "--size").expect("a size")[1];
            let (width, height) = size.split_once('x').expect("a size");
            width.parse::<f32>().expect("a width") / height.parse::<f32>().expect("a height")
        };
        assert!((size_of("game-title-tablet-ja") - 4.0 / 3.0).abs() < 0.01);
        assert!((size_of("game-title-wide-ja") - 16.0 / 9.0).abs() < 0.01);
        assert!((size_of("game-title-ultrawide-ja") - 21.0 / 9.0).abs() < 0.01);
    }

    #[test]
    fn a_path_under_tests_is_found_from_the_goldens_and_exists() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let paths = Paths {
            sandbox: PathBuf::new(),
            content: PathBuf::new(),
            goldens: root.join("tests/golden"),
            out: PathBuf::new(),
        };
        assert_eq!(scene_arg("--replay", &paths), OsString::from("--replay"));
        let mut replays = 0;
        for scene in scenes() {
            for arg in scene.args.iter().filter(|arg| arg.starts_with("tests/")) {
                assert!(Path::new(&scene_arg(arg, &paths)).is_file(), "{}: {arg}", scene.name);
                replays += 1;
            }
        }
        assert!(replays > 0, "a scene plays a committed replay");
    }

    #[test]
    fn only_the_debug_draw_scene_turns_debug_draw_on() {
        for scene in scenes() {
            assert_eq!(
                scene.args.contains(&"--debug-draw"),
                scene.name.starts_with("debugdraw-"),
                "{}",
                scene.name
            );
        }
    }

    #[test]
    fn only_the_debug_scenes_show_a_debug_view_and_each_its_own() {
        for scene in scenes() {
            let view = scene.args.windows(2).find(|pair| pair[0] == "--debug-view").map(|pair| pair[1]);
            match scene.name.strip_prefix("debug-") {
                Some(name) => assert!(view.is_some_and(|view| name.starts_with(view)), "{}", scene.name),
                None => assert_eq!(view, None, "{}", scene.name),
            }
        }
    }

    #[test]
    fn only_the_taa_render_scale_pair_has_gates_of_its_own() {
        for scene in scenes() {
            let expected = match scene.name {
                "turntable-taa" | "turntable-taa-upsampled" => {
                    Thresholds { mean: TAA_RENDER_SCALE_MEAN, ..Thresholds::default() }
                }
                _ => Thresholds::default(),
            };
            assert_eq!(scene.thresholds, expected, "{}", scene.name);
        }
    }

    /// The families with committed goldens.
    const GOLDEN_FAMILIES: [&str; 3] = ["metal", "vulkan-lavapipe", "dx12-warp"];

    /// A committed golden, from `tests/golden`.
    fn committed(scene: &str, family: &str) -> Image {
        let name = format!("{scene}.{family}.png");
        Image::read_png(&Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/golden").join(name)).unwrap()
    }

    #[test]
    fn a_capture_that_lost_its_scene_s_feature_fails_the_golden() {
        // Review DV-2. Each pair's arguments differ in one thing a scene exists to show: the bangs'
        // soft shadow, MSAA, the render scale; so each golden read as the other is that feature
        // gone wrong. The tier pairs aren't here: `eyes-front` and `eyes-mobile`, and `hair-shadow`
        // and `hair-shadow-mobile`, stay within the gates (mean 0.0096, p99 0.054, worst tile 0.057
        // at most), moving a little everywhere and no more locally than lavapipe's noise.
        let scenes = scenes();
        let thresholds = |name: &str| scenes.iter().find(|scene| scene.name == name).unwrap().thresholds;
        for family in GOLDEN_FAMILIES {
            for (first, second) in [
                ("hair-shadow-soft", "hair-shadow"),
                ("turntable", "turntable-msaa-off"),
                ("turntable", "turntable-render-scale"),
                ("turntable-taa", "turntable-taa-upsampled"),
            ] {
                let first_image = committed(first, family);
                let second_image = committed(second, family);
                for (golden, actual, golden_image, actual_image) in [
                    (first, second, &first_image, &second_image),
                    (second, first, &second_image, &first_image),
                ] {
                    let thresholds = thresholds(golden);
                    let comparison = golden::compare(golden_image, actual_image, &thresholds).unwrap();
                    assert!(
                        !comparison.passes(&thresholds),
                        "{actual} passes as {golden} on {family}: mean {}, p{} {}, worst tile {}",
                        comparison.mean,
                        comparison.percentile,
                        comparison.percentile_value,
                        comparison.worst_tile,
                    );
                }
            }
        }
    }

    #[test]
    fn a_small_wrong_patch_fails_the_golden() {
        // Review DV-2: before the tile gate a centered 40 px patch passed these scenes; on
        // `lookdev-face` an eye is about 30 px wide, so a whole eye could go wrong.
        const SIDE: usize = 6;
        let scenes = scenes();
        for family in GOLDEN_FAMILIES {
            for name in ["eyes-front", "lookdev-face", "hair-shadow", "turntable", "lookdev-town-hero"] {
                let golden = committed(name, family);
                let mut actual = golden.clone();
                let (width, height) = (actual.width as usize, actual.height as usize);
                for y in (height - SIDE) / 2..(height + SIDE) / 2 {
                    for x in (width - SIDE) / 2..(width + SIDE) / 2 {
                        let start = (y * width + x) * 4;
                        actual.rgba[start..start + 3].fill(128);
                    }
                }
                let thresholds = scenes.iter().find(|scene| scene.name == name).unwrap().thresholds;
                let comparison = golden::compare(&golden, &actual, &thresholds).unwrap();
                assert!(
                    !comparison.passes(&thresholds),
                    "a {SIDE} px patch passes {name} on {family}: mean {}, p{} {}, worst tile {}",
                    comparison.mean,
                    comparison.percentile,
                    comparison.percentile_value,
                    comparison.worst_tile,
                );
            }
        }
    }

    #[test]
    fn reads_the_backend_family_from_the_adapter_record() {
        let dir = std::env::temp_dir().join(format!("sapphire_testkit_scenes_{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let capture = dir.join("scene.png");
        fs::write(dir.join("scene.adapter.txt"), "backend=vulkan\nadapter=llvmpipe (LLVM 19)\n").unwrap();
        assert_eq!(backend_family(&capture).unwrap(), "vulkan-lavapipe");
        fs::write(dir.join("scene.adapter.txt"), "backend=metal\n").unwrap();
        assert!(backend_family(&capture).is_err(), "no adapter line");
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn rejects_unknown_scenes_and_empty_shards() {
        let paths = Paths {
            sandbox: PathBuf::from("unused"),
            content: PathBuf::from("unused"),
            goldens: PathBuf::from("unused"),
            out: PathBuf::from("unused"),
        };
        let run = |selection| run(&paths, Mode::Compare, selection, DEFAULT_SCENE_TIMEOUT).unwrap_err();
        assert!(run(Selection::Scene("no-such-scene")).contains("turntable"));
        assert!(run(Selection::Shard(Shard { index: 999, count: 999 })).contains("holds none"));
    }

    /// The side of a fake sandbox's capture.
    const FAKE_SIDE: u32 = 8;

    fn fake_image(side: u32) -> Image {
        Image { width: side, height: side, rgba: [40, 80, 120, 255].repeat((side * side) as usize) }
    }

    /// In a fake sandbox's environment: what it does (see [`fake_sandbox`]).
    const FAKE_ENV: &str = "SAPPHIRE_TESTKIT_FAKE_SANDBOX";
    /// In a fake crash monitor's environment: the file it writes as it exits.
    const MONITOR_MARKER_ENV: &str = "SAPPHIRE_TESTKIT_FAKE_MONITOR_MARKER";

    /// This test binary started again as a fake sandbox doing `behaviour`, with only
    /// [`fake_sandbox`] selected. The runner adds the capture's arguments after the `--`, where the
    /// test harness takes them as test names to match exactly, which they never do.
    fn fake(behaviour: &'static str) -> Command {
        let mut command = Command::new(std::env::current_exe().expect("the test binary"));
        command
            .args(["--exact", "scenes::tests::fake_sandbox", "--nocapture", "--"])
            .env(FAKE_ENV, behaviour);
        command
    }

    /// Not a test of its own: the stand-in for the sandbox and its crash monitor that
    /// [`a_scene_past_its_deadline_is_stopped_and_the_run_goes_on`] starts (see [`fake`]). Without
    /// [`FAKE_ENV`] it does nothing.
    #[test]
    fn fake_sandbox() {
        let Ok(behaviour) = std::env::var(FAKE_ENV) else { return };
        if behaviour == "monitor" {
            // As `sapphire_crash`'s monitor does, it exits once the sandbox's end of its stdin closes.
            let _ = io::stdin().read_to_end(&mut Vec::new());
            fs::write(std::env::var(MONITOR_MARKER_ENV).expect("the marker's path"), "exited")
                .expect("the marker");
            return;
        }
        let args: Vec<String> = std::env::args().collect();
        let capture = args.windows(2).find(|pair| pair[0] == "--capture").map(|pair| PathBuf::from(&pair[1]));
        let capture = capture.expect("--capture <png>");
        // Like the real monitor, it inherits this process's output and holds a lifeline only this
        // process has, until this process is gone.
        let mut monitor = fake("monitor")
            .env(MONITOR_MARKER_ENV, capture.with_extension("monitor-exited"))
            .stdin(Stdio::piped())
            .spawn()
            .expect("the fake monitor");
        let lifeline = monitor.stdin.take();
        // More than a pipe holds (64 KiB on Linux) on each of stdout and stderr.
        let chatter = |lines: usize| {
            for line in 0..lines {
                writeln!(io::stdout(), "stdout line {line}: {}", "x".repeat(100)).expect("stdout");
                eprintln!("stderr line {line}: {}", "x".repeat(100));
            }
        };
        match behaviour.as_str() {
            "hang" => {}
            "stall" => {
                chatter(1000);
                eprintln!("stalling now");
            }
            "fail-one" | "fail-two" => {
                eprintln!("failing now");
                drop(lifeline);
                monitor.wait().expect("the fake monitor's exit");
                std::process::exit(42);
            }
            "chatty" | "wrong-size-golden" | "corrupt-golden" | "truncated" => {
                if behaviour == "chatty" {
                    chatter(2000);
                }
                fake_image(FAKE_SIDE).write_png(&capture).expect("the capture");
                if behaviour == "truncated" {
                    let bytes = fs::read(&capture).expect("the capture");
                    fs::write(&capture, &bytes[..bytes.len() / 2]).expect("half the capture");
                }
                let record = "backend=vulkan\nadapter=llvmpipe (fake)\n";
                fs::write(capture.with_extension("adapter.txt"), record).expect("the adapter record");
                // A clean exit closes the lifeline, as the real sandbox's does, and waits.
                drop(lifeline);
                monitor.wait().expect("the fake monitor's exit");
                return;
            }
            other => panic!("no fake sandbox does {other:?}"),
        }
        loop {
            std::thread::sleep(Duration::from_secs(3600));
        }
    }

    #[test]
    fn a_scene_past_its_deadline_is_stopped_and_the_run_goes_on() {
        // Both deadlines are real and unchanged. Independent output directories let these
        // process-lifecycle cases wait concurrently; each must continue to a successful capture.
        std::thread::scope(|scope| {
            let hang = scope.spawn(|| {
                let expected = [("fake-hang", "timed-out"), ("fake-chatty", "updated")];
                check_fake_run("deadline-hang", Mode::Update, &expected);
            });
            let stall = scope.spawn(|| {
                let expected = [("fake-stall", "timed-out"), ("fake-chatty", "updated")];
                check_fake_run("deadline-stall", Mode::Update, &expected);
            });
            hang.join().unwrap();
            stall.join().unwrap();
        });
    }

    #[test]
    fn consecutive_failures_are_collected_before_the_next_success() {
        // Failure accumulation still covers consecutive errors in one runner, without paying
        // two deadlines serially. Each child really exits unsuccessfully and reaps its monitor.
        check_fake_run(
            "failures",
            Mode::Update,
            &[("fake-fail-one", "error"), ("fake-fail-two", "error"), ("fake-chatty", "updated")],
        );
    }

    #[test]
    fn a_golden_or_capture_that_can_t_be_compared_fails_its_scene_and_the_run_goes_on() {
        // Review DV-8: each of these stopped the whole run, naming no scene.
        check_fake_run(
            "unreadable",
            Mode::Compare,
            &[
                ("fake-wrong-size-golden", "error"),
                ("fake-corrupt-golden", "error"),
                ("fake-truncated", "error"),
                ("fake-chatty", "ok"),
            ],
        );
    }

    #[test]
    fn update_keeps_a_capture_cut_short_out_of_the_goldens() {
        // It would pass for a golden until the next run compared against it.
        check_fake_run(
            "update-truncated",
            Mode::Update,
            &[("fake-truncated", "error"), ("fake-chatty", "updated")],
        );
    }

    fn check_fake_run(group: &str, mode: Mode, expected: &[(&'static str, &'static str)]) {
        let dir = std::env::temp_dir().join(format!("sapphire_testkit_{group}_{}", std::process::id()));
        let paths = Paths {
            sandbox: PathBuf::from("unused"),
            content: PathBuf::from("unused"),
            goldens: dir.join("goldens"),
            out: dir.join("out"),
        };
        let scenes: Vec<Scene> = expected
            .iter()
            .map(|&(name, _)| Scene { name, args: &["--plain-post"], thresholds: Thresholds::default() })
            .collect();
        let selected: Vec<&Scene> = scenes.iter().collect();
        let timeout = Duration::from_secs(3);
        let golden_of = |name: &str| paths.goldens.join(format!("{name}.vulkan-lavapipe.png"));
        if mode == Mode::Compare {
            fs::create_dir_all(&paths.goldens).unwrap();
            for &(name, _) in expected {
                match name {
                    "fake-wrong-size-golden" => {
                        fake_image(FAKE_SIDE * 2).write_png(&golden_of(name)).unwrap()
                    }
                    "fake-corrupt-golden" => fs::write(golden_of(name), "not a PNG").unwrap(),
                    _ => fake_image(FAKE_SIDE).write_png(&golden_of(name)).unwrap(),
                }
            }
        }
        let sandbox = |scene: &Scene| fake(scene.name.strip_prefix("fake-").expect("a fake"));
        let failures = run_scenes(&selected, &paths, mode, timeout, &sandbox).unwrap();
        let expected_failures: Vec<_> =
            expected.iter().filter(|(_, status)| !matches!(*status, "updated" | "ok")).collect();
        assert_eq!(failures.len(), expected_failures.len(), "{failures:#?}");
        for (failure, &&(name, status)) in failures.iter().zip(&expected_failures) {
            let unreadable = match name {
                "fake-wrong-size-golden" => Some("size mismatch: the golden is (16, 16), the capture (8, 8)"),
                "fake-corrupt-golden" => Some("not a PNG this harness reads"),
                "fake-truncated" => Some("not a PNG this harness reads"),
                _ => None,
            };
            if status == "timed-out" {
                assert!(failure.starts_with(&format!("{name}: timed out after 3 s")), "{failure}");
            } else if let Some(why) = unreadable {
                assert!(mode == Mode::Compare || !golden_of(name).exists(), "{name} became a golden");
                assert!(failure.starts_with(&format!("{name}: ")) && failure.contains(why), "{failure}");
                // The capture is kept, bytes as the sandbox wrote them, and the failure says where.
                let kept = paths.out.join(format!("{name}.vulkan-lavapipe.actual.png"));
                assert!(failure.contains(&kept.display().to_string()), "{failure}");
                assert_eq!(
                    fs::read(&kept).unwrap(),
                    fs::read(paths.out.join(format!("{name}.png"))).unwrap()
                );
            } else {
                assert!(failure.starts_with(&format!("{name}: the sandbox failed")), "{failure}");
                assert!(failure.contains("failing now"), "{failure}");
            }
            if name == "fake-stall" {
                assert!(failure.contains("stalling now"), "{failure}");
                // Its log is kept, all of it, including more output than a pipe holds.
                let log = fs::read_to_string(paths.out.join("fake-stall.log")).unwrap();
                assert!(
                    log.contains("stdout line 999")
                        && log.contains("stderr line 999")
                        && log.contains("stalling now")
                );
            }
        }
        // Every run goes on to a scene that writes far more than a pipe holds and finishes.
        assert!(paths.goldens.join("fake-chatty.vulkan-lavapipe.png").is_file());
        let log = fs::read_to_string(paths.out.join("fake-chatty.log")).unwrap();
        assert!(log.contains("stdout line 1999") && log.contains("stderr line 1999"));

        // Timings cover every scene in order, with the original real-time deadline and slack.
        let timings = fs::read_to_string(paths.out.join("timings.json")).unwrap();
        let entries: Vec<&str> = timings.lines().filter(|line| line.contains("\"scene\"")).collect();
        assert_eq!(entries.len(), expected.len(), "{timings}");
        for (entry, &(scene, status)) in entries.iter().zip(expected) {
            assert!(entry.contains(&format!("\"scene\": \"{scene}\"")), "{entry}");
            assert!(entry.contains(&format!("\"status\": \"{status}\"")), "{entry}");
            assert!(entry.contains("\"family\": \"vulkan-lavapipe\""), "{entry}");
            let seconds: f64 = entry
                .split("\"seconds\": ")
                .nth(1)
                .and_then(|rest| rest.split(',').next())
                .unwrap()
                .parse()
                .unwrap();
            if status == "timed-out" {
                assert!((3.0..8.0).contains(&seconds), "{entry}");
            }
        }

        // No crash monitor outlives its sandbox: each observed its lifeline closing.
        let clock = Stopwatch::start();
        for scene in &scenes {
            let marker = paths.out.join(format!("{}.monitor-exited", scene.name));
            while !marker.is_file() {
                assert!(clock.elapsed() < Duration::from_secs(30), "{}'s monitor still runs", scene.name);
                std::thread::sleep(Duration::from_millis(10));
            }
        }
        let _ = fs::remove_dir_all(&dir);
    }
}
