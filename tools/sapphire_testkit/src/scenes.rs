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
/// with and without a camera rim (`--camera-rim`), and pin what the rim scenes pin.
/// with and without a camera rim (`--camera-rim`), and pin what the rim scenes pin; the soft
/// bangs' shadow scene softens the face rig's faces (`--hair-shadow-softness`) under the hair
/// scene's keys and pins what it pins; the back-lit hair scenes light the face rig from behind
/// (`--face-layout backlit`), without and with light through the hair (`--hair-transmission`),
/// and pin what the hair scenes pin.
pub fn scenes() -> [Scene; 95] {
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
            thresholds: Thresholds::default(),
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
            thresholds: Thresholds::default(),
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
        // The hero shot at golden hour: the character in three-quarter view with her SDF face map
        // and outlines, the pavilion behind her with the low sun's glow round its roof, the gate
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
        // The material layer samples (M1 C7) by day, right of the character: crimson lacquer with
        // a crisp highlight; gold, its tinted matcap and highlight, the sky reflected on its
        // metallic stripes; leather, darker toward its rim, with a soft GGX highlight; a dark suit
        // with cyan light lines; and a leg in a thigh-high black stocking, sheer facing the
        // camera, near black at its silhouette, bare above its top.
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
        // look-dev scenes are: her face's SDF shadow in its peach ramp with a light blush, the
        // bangs' warm shadow on her forehead, the irises' gradient and sparkles under the
        // glowing highlights, the darkened lash line, the hair's crisp band and thin brown
        // outlines, the square behind her.
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
    /// Captures, actuals, diffs, the scenes' logs and `timings.json`; emptied first.
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

/// Explicit control for before/after measurements and cache-related driver triage. This
/// optimizes pipeline compilation only; every scene still starts a separate sandbox process.
const PIPELINE_CACHE_ENV: &str = "SAPPHIRE_GOLDEN_PIPELINE_CACHE";
const CAPTURE_PIPELINE_CACHE_ENV: &str = "SAPPHIRE_CAPTURE_PIPELINE_CACHE";

fn pipeline_cache_enabled(value: Option<&str>) -> Result<bool, String> {
    match value {
        None | Some("on") => Ok(true),
        Some("off") => Ok(false),
        Some(value) => Err(format!("{PIPELINE_CACHE_ENV} expects on or off, got {value:?}")),
    }
}

/// The cache lives under the output directory that `run_scenes` empties before starting its
/// serial children. Sharing the application's user cache, or running these children in parallel
/// against one cache, would race its crash sentinel and atomic temporary file. Metal and DX12
/// ignore this optional cache in the RHI and never create its directory.
fn sandbox_command(paths: &Paths, pipeline_cache: bool) -> Command {
    let mut command = Command::new(&paths.sandbox);
    command.env(CONTENT_DIR_ENV, &paths.content);
    // Off must override an inherited capture cache as well, for a real uncached control.
    command.env_remove(CAPTURE_PIPELINE_CACHE_ENV);
    if pipeline_cache {
        command.env(CAPTURE_PIPELINE_CACHE_ENV, paths.out.join("pipeline-cache"));
    }
    command
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
    let setting = match std::env::var(PIPELINE_CACHE_ENV) {
        Ok(value) => Some(value),
        Err(std::env::VarError::NotPresent) => None,
        Err(error) => return Err(format!("{PIPELINE_CACHE_ENV}: {error}")),
    };
    let pipeline_cache = pipeline_cache_enabled(setting.as_deref())?;
    let sandbox = |_: &Scene| sandbox_command(paths, pipeline_cache);
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
    /// The sandbox failed or left no adapter record.
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
    command.arg("--capture").arg(&capture).args(["--size", SIZE]).args(scene.args);
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
    let outcome =
        |status: Status, failure: Option<String>| Outcome { status, family: Some(family.clone()), failure };
    let golden_path = paths.goldens.join(format!("{}.{family}.png", scene.name));
    let kept = paths.out.join(format!("{}.{family}.actual.png", scene.name));
    let keep = || fs::copy(&capture, &kept).map(drop).map_err(|e| io_error("failed to write", &kept, e));

    if mode == Mode::Update {
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

    let golden = Image::read_png(&golden_path).map_err(|e| e.to_string())?;
    let actual = Image::read_png(&capture).map_err(|e| e.to_string())?;
    let comparison = golden::compare(&golden, &actual, &scene.thresholds).map_err(|e| e.to_string())?;
    let summary = format!(
        "{} [{family}] {:.1} s: mean {:.4} (≤ {}), p{} {:.4} (≤ {}), max {:.3}",
        scene.name,
        seconds(),
        comparison.mean,
        scene.thresholds.mean,
        comparison.percentile,
        comparison.percentile_value,
        scene.thresholds.percentile_max,
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
    use std::io::Write as _;

    use super::*;

    #[test]
    fn capture_pipeline_cache_is_private_to_the_run_and_can_be_disabled() {
        let paths = Paths {
            sandbox: "sandbox".into(),
            content: "cooked".into(),
            goldens: "references".into(),
            out: "test-run".into(),
        };
        let command = sandbox_command(&paths, true);
        let cache = command.get_envs().find(|(key, _)| *key == CAPTURE_PIPELINE_CACHE_ENV);
        assert_eq!(cache.unwrap().1, Some(paths.out.join("pipeline-cache").as_os_str()));
        let command = sandbox_command(&paths, false);
        let cache = command.get_envs().find(|(key, _)| *key == CAPTURE_PIPELINE_CACHE_ENV);
        assert_eq!(cache.unwrap().1, None, "disabled also removes an inherited directory");
        assert_eq!(pipeline_cache_enabled(None), Ok(true));
        assert_eq!(pipeline_cache_enabled(Some("on")), Ok(true));
        assert_eq!(pipeline_cache_enabled(Some("off")), Ok(false));
        assert!(pipeline_cache_enabled(Some("true")).is_err());
    }

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
        // CI's split: lavapipe in 2, WARP in 4.
        let scenes = scenes();
        for (family, count) in [(Family::Lavapipe, 2), (Family::Warp, 4)] {
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
        // ambient scenes pin fog off instead.
        for scene in scenes() {
            let pinned = |flag: &str, value: &str| scene.args.windows(2).any(|pair| pair == [flag, value]);
            if scene.name.starts_with("ambient-") {
                assert!(pinned("--fog", "off"), "{}", scene.name);
            } else {
                assert!(pinned("--ao", "off"), "{}", scene.name);
                assert!(!scene.args.contains(&"--probes"), "{}", scene.name);
            }
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
            "chatty" => {
                chatter(2000);
                fs::write(&capture, "unread by --update").expect("the capture");
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
                check_fake_run("deadline-hang", &[("fake-hang", "timed-out"), ("fake-chatty", "updated")]);
            });
            let stall = scope.spawn(|| {
                check_fake_run("deadline-stall", &[("fake-stall", "timed-out"), ("fake-chatty", "updated")]);
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
            &[("fake-fail-one", "error"), ("fake-fail-two", "error"), ("fake-chatty", "updated")],
        );
    }

    fn check_fake_run(group: &str, expected: &[(&'static str, &'static str)]) {
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
        let sandbox = |scene: &Scene| fake(scene.name.strip_prefix("fake-").expect("a fake"));
        let failures = run_scenes(&selected, &paths, Mode::Update, timeout, &sandbox).unwrap();
        let expected_failures: Vec<_> = expected.iter().filter(|(_, status)| *status != "updated").collect();
        assert_eq!(failures.len(), expected_failures.len(), "{failures:#?}");
        for (failure, &&(name, status)) in failures.iter().zip(&expected_failures) {
            if status == "timed-out" {
                assert!(failure.starts_with(&format!("{name}: timed out after 3 s")), "{failure}");
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
