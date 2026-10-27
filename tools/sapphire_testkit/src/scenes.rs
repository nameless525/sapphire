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

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::golden::{self, Image, Thresholds, backend_key};

/// The environment variable the sandbox reads its cooked content from.
const CONTENT_DIR_ENV: &str = "SAPPHIRE_CONTENT_DIR";
/// Small enough for lavapipe and WARP to render each scene in a few seconds.
const SIZE: &str = "640x360";

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
pub fn scenes() -> [Scene; 78] {
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
        // and outlines, the market behind her in the low sun's warm light and blue-violet shade.
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
    /// Captures, actuals and diffs; emptied first.
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

/// Slice `index` of `count` (from 1): every `count`-th scene, starting at position `index - 1`.
/// Neighbouring scenes often share a feature and its cost on a CPU rasterizer (W5's six ambient
/// scenes take minutes each on WARP), so dealing them out in turn keeps the slices' times close,
/// where cutting the list into runs would give one slice all of them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Shard {
    pub index: usize,
    pub count: usize,
}

impl Shard {
    fn holds(self, position: usize) -> bool {
        position % self.count == self.index - 1
    }
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

/// Runs the selected scenes and returns one message per failure (empty: all passed).
pub fn run(paths: &Paths, mode: Mode, selection: Selection<'_>) -> Result<Vec<String>, String> {
    let scenes = scenes();
    let selected: Vec<&Scene> = scenes
        .iter()
        .enumerate()
        .filter(|&(position, scene)| match selection {
            Selection::All => true,
            Selection::Scene(name) => name == scene.name,
            Selection::Shard(shard) => shard.holds(position),
        })
        .map(|(_, scene)| scene)
        .collect();
    match selection {
        Selection::Scene(name) if selected.is_empty() => {
            let names: Vec<&str> = scenes.iter().map(|scene| scene.name).collect();
            return Err(format!("no golden scene named {name:?}; scenes: {}", names.join(", ")));
        }
        Selection::Shard(shard) if selected.is_empty() => {
            return Err(format!(
                "shard {}/{} holds none of the {} scenes",
                shard.index,
                shard.count,
                scenes.len()
            ));
        }
        Selection::Shard(shard) => {
            eprintln!("shard {}/{}: {} of {} scenes", shard.index, shard.count, selected.len(), scenes.len());
        }
        _ => {}
    }
    let io = |what: &str, path: &Path, error: std::io::Error| format!("{what} {}: {error}", path.display());
    if paths.out.exists() {
        fs::remove_dir_all(&paths.out).map_err(|e| io("failed to empty", &paths.out, e))?;
    }
    fs::create_dir_all(&paths.out).map_err(|e| io("failed to create", &paths.out, e))?;
    let in_ci = std::env::var_os("CI").is_some();

    let mut failures = Vec::new();
    for scene in selected {
        let capture = paths.out.join(format!("{}.png", scene.name));
        let output = Command::new(&paths.sandbox)
            .arg("--capture")
            .arg(&capture)
            .args(["--size", SIZE])
            .args(scene.args)
            .env(CONTENT_DIR_ENV, &paths.content)
            .output()
            .map_err(|e| io("failed to run", &paths.sandbox, e))?;
        if !output.status.success() {
            let log = String::from_utf8_lossy(&output.stderr);
            failures.push(format!("{}: the sandbox failed ({}):\n{log}", scene.name, output.status));
            continue;
        }
        let family = backend_family(&capture)?;
        let golden_path = paths.goldens.join(format!("{}.{family}.png", scene.name));
        let kept = paths.out.join(format!("{}.{family}.actual.png", scene.name));
        let keep = || fs::copy(&capture, &kept).map(drop).map_err(|e| io("failed to write", &kept, e));

        if mode == Mode::Update {
            fs::create_dir_all(&paths.goldens).map_err(|e| io("failed to create", &paths.goldens, e))?;
            fs::copy(&capture, &golden_path).map_err(|e| io("failed to write", &golden_path, e))?;
            eprintln!("updated  {}", golden_path.display());
            continue;
        }
        if !golden_path.is_file() {
            keep()?;
            let message = format!(
                "{} [{family}]: no golden yet; the capture is at {} (`cargo xtask golden --update` adds it)",
                scene.name,
                kept.display()
            );
            if in_ci {
                failures.push(message);
            } else {
                eprintln!("skipped  {message}");
            }
            continue;
        }

        let golden = Image::read_png(&golden_path).map_err(|e| e.to_string())?;
        let actual = Image::read_png(&capture).map_err(|e| e.to_string())?;
        let comparison = golden::compare(&golden, &actual, &scene.thresholds).map_err(|e| e.to_string())?;
        let summary = format!(
            "{} [{family}]: mean {:.4} (≤ {}), p{} {:.4} (≤ {}), max {:.3}",
            scene.name,
            comparison.mean,
            scene.thresholds.mean,
            comparison.percentile,
            comparison.percentile_value,
            scene.thresholds.percentile_max,
            comparison.max,
        );
        if comparison.passes(&scene.thresholds) {
            eprintln!("ok       {summary}");
        } else {
            keep()?;
            let diff = paths.out.join(format!("{}.{family}.diff.png", scene.name));
            comparison.error_map.write_png(&diff).map_err(|e| e.to_string())?;
            failures.push(format!("{summary}\n    see {} and {}", kept.display(), diff.display()));
        }
    }
    Ok(failures)
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

    #[test]
    fn shards_deal_out_every_scene_once_and_evenly() {
        let total = scenes().len();
        for count in 1..=6 {
            let mut taken = vec![0; total];
            let mut sizes = Vec::new();
            for index in 1..=count {
                let shard = Shard { index, count };
                let held: Vec<usize> = (0..total).filter(|&position| shard.holds(position)).collect();
                held.iter().for_each(|&position| taken[position] += 1);
                sizes.push(held.len());
            }
            assert!(taken.iter().all(|&times| times == 1), "{count} shards");
            assert!(
                sizes.iter().max().unwrap() - sizes.iter().min().unwrap() <= 1,
                "{count} shards: {sizes:?}"
            );
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
        assert!(
            run(&paths, Mode::Compare, Selection::Scene("no-such-scene")).unwrap_err().contains("turntable")
        );
        let past_the_end = Shard { index: 999, count: 999 };
        assert!(
            run(&paths, Mode::Compare, Selection::Shard(past_the_end)).unwrap_err().contains("holds none")
        );
    }
}
