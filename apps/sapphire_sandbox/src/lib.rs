//! Sapphire Sandbox, the engine test bed (milestone M0, spec §10).
//!
//! It runs the whole engine path on every platform: the SDL3 platform layer, the fixed 60 Hz
//! simulation on the engine clocks and ECS, time of day (`sapphire_world::tod`) resolving the
//! frame's look and the character's light rig, plain-data
//! [`FramePacket`](sapphire_render::FramePacket)s filled by
//! change-detecting extract systems, and the wgpu anime renderer's persistent scene, drawing the
//! cooked sample mannequin.
//!
//! What it draws is [`sapphire_stage`]'s, and its fights and their HUD `game_combat`'s and `game_ui`'s: a file
//! this page names that this crate doesn't have is one of theirs. This one is the bench and the goldens' host:
//! its command line (`options.rs`), its window loop (`app.rs`), captures, benches and dev panels, and what
//! mobile links.
//!
//! # Controls
//!
//! The default bindings are a `sapphire_input` document, `default_bindings.ron`, compiled into
//! the app (see `input.rs`), over `sapphire_controls`' combat bindings.
//!
//! | Input | Action |
//! |---|---|
//! | Left-drag, one-finger drag, right stick, arrow keys | Orbit |
//! | Wheel, pinch, triggers | Zoom |
//! | `Space`, gamepad South, double tap | Toggle world slow motion (perfect-dodge style, 0.2×) |
//! | `H`, gamepad East, long press (0.5 s) | 8-tick hitstop on the turntable |
//! | `P`, gamepad Start | Pause: the fixed steps stop, and the frame goes on |
//! | Android's Back, gamepad Back, `Backspace` | Pause, and while paused, leave |
//! | `P`, `Esc`, gamepad Start or East, a tap (while paused) | Resume |
//! | `Esc` | Quit (desktop) |
//! | `` ` ``, a still two-finger hold (1 s) | Show or hide the debug overlay (`dev` builds) |
//!
//! Combat's verbs (`sapphire_controls`' `combat_bindings.ron`) sit under these, so a source only
//! they bind reaches a tick: `J`, or gamepad West, is the light attack. Nothing plays ticks yet, so
//! `dev` builds log them as they change, as `tick actions`. `--combat-input` takes the table's
//! other rows away, leaving combat's bindings alone.
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
//! - `--shed gtao|cirrus|wind-level|sky-face-rate|bloom-level`, repeatable: an optional pass the
//!   tier goes without (`sapphire_render::quality::OptionalPass`), after `--ao`, built so from
//!   the start: one step of what the tier draws, and nothing where it draws none.
//! - `--rung <0..8>`: hold the tier's thermal ladder this many rungs down (`r.rung`, M4 T2;
//!   `tiers.ron` lists each tier's rungs): its scale, its passes shed in place and its frame-rate
//!   cap, whatever the heat. The golden scenes `tier-*-rung-*` draw each rung.
//! - `--spin <radians per second>`: the turntable's speed (default 0.35), for edges in motion.
//! - `--vfx arc|hit|dust|duel`: plays `sapphire_vfx`'s shipped effects at fixed ticks around the
//!   character (M2 E2; the `vfx` module): a sword arc each second, one that strikes with a spark
//!   and a hitstop, dust on the ground, or a duel's worst moment held still, as the bench's load.
//!   They play around the world's origin, where every stage but `--look-dev street` stands its
//!   character.
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
//! - `--face-layout lights|hair|highlights|turn|gi|gi-clamped|eyes|eyes-turned|gaze|blink|backlit` (with
//!   `--faces`): the face rig's heads (M1 C5): the lights above (the default); keys around the
//!   front and above, for the bangs' shadow on the face; heads nodding under one key, for the
//!   hair's highlights; heads turning and nodding from the camera, for the eyes and brows through
//!   the bangs. Then the GI layouts (M1 C2): heads turned and nodding every way, two in gloom, lit
//!   by the default light rig preset's stylized GI alone, the key light off and the GI raised
//!   four-fold (`gi`), or the same with its HSV clamp's ceilings pulled down into the GI's range
//!   (`gi-clamped`). Then the eyes' layouts (M1 C6), framed close on the eyes: one head facing the
//!   camera, one turned to three-quarter, heads looking around, and lids half closed and closed.
//!   Then keys from behind the heads (`backlit`, M1 C8), for light through thin hair.
//! - `--hair-shadow-softness <meters>`: soften every face material's bangs' shadow to an edge
//!   this wide (M1 C8), as it loads: the face rig's heads (`--face-layout hair` shows it) and the
//!   look-dev character.
//! - `--hair-transmission <strength>`: light every hair material through by this strength
//!   against a back light (M1 C8), as it loads: the face rig's heads (`--face-layout backlit`
//!   shows it) and the look-dev character.
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
//! - `--look-dev wide|hero|sky|street|face|body|full|down`: the look-dev town (M1 X1, `lookdev.rs`): pixiv's
//!   VRM sample in place of the mannequin, held standing in a Jinzhou-style town square of the
//!   assembly's parametric buildings in the environment family, with its fog cards and lamps,
//!   every landed feature on, seen from an authored shot: `wide` across the square, `hero` a
//!   three-quarter shot of the character, `sky` low and looking up past the character, `street`
//!   the character standing in the street out of the square, `face` a close-up of the face and
//!   shoulders, `body`
//!   the whole character for reading a move (M2 O3, the `lookdev-move-*` golden scenes). A
//!   scene of its own, not a `--stage` layout: it replaces the character and the ground under
//!   it, so `--stage`, `--environment`, `--probes`, `--foliage`, `--outline-row`, `--faces`,
//!   `--rim-rig` and `--hard-edges` can't join it. `--time-of-day` and `--day-length` run its
//!   day; `--view`, `--fov` and `--zoom` adjust the shot. The golden scenes
//!   `lookdev-town-day`, `-hero`, `-golden`, `-blue` and `-night`, `lookdev-street-night` and
//!   `lookdev-face` show it.
//! - `--ui-game <stem>`: one of the game's screens (M3 U7, `content/ui/<stem>.ui.ron`: `title`, `pause`,
//!   `settings`, `character`, `dialogue` or `hud`) on its sample model over whatever the scene draws, as the
//!   `game-*` goldens show them. Hot reload and live link's `push_document` take edits of its document.
//!   `--ui-locale <tag>` picks its language (`en`, by default, or `ja`), `--ui-scale <scale>` its pixels per
//!   point, and `--ui-pseudo` turns on pseudo-localization. It opens at the clock's start: `--ui-time
//!   <seconds>` is the one time it's shown at, a capture's being byte-stable, and `--ui-walk
//!   up,down,left,right,confirm` the focus steps taken as it opens. Input is `game_app`'s, so none reaches it
//!   here. With `--hud` both draw from one atlas, the HUD over the screen. `--ui-plain` stands a flat color
//!   behind it, so a capture doesn't move with the scene. `character` holds the 3D inset (M3 U6), which the
//!   sandbox stands a showcase of the character for: `--ui-turn <across>` turns it as a drag across that
//!   share of its width would, and `--ui-spin <across>` turns it that much more each frame of a bench, so it
//!   draws every frame. `--ui-dialogue [<name>:]<lines>`, with `--ui-game dialogue`, puts the panel on
//!   `content/dialogue/<name>.dialogue.ron`'s cooked conversation that many lines in, in place of its
//!   sample (M3 Q2b, Z5e); with no name, the Lantern Ward guide's. `--ui-text normal|large|larger` sets the
//!   player's text size, and `--ui-subtitle <size>:<background>` (`small` to `larger`, `off` to `solid`),
//!   with `--ui-game hud`, has the sample say a cutscene's line in subtitles of that look (M4 A2).
//! - `--hud fixed|live`: the combat HUD (`hud.rs`, M2 I2) over whatever the scene draws. `fixed` is
//!   one frozen frame on a made-up clock, a hit landing with both thumbs down, as the `hud-*` golden
//!   scenes capture it; `live` runs the touch zones on real touches, with a stand-in for gameplay,
//!   and logs what they do.
//! - `--hud-insets <left>,<top>,<right>,<bottom>`: what the device covers, in points, in place of
//!   the window's safe area: to try a notch on a frame that has none.
//! - `--hud-scale <scale>`: pixels per point in an offscreen frame (3 for a phone's capture), the
//!   player's UI scale on top of the display's in a window. 1 without it.
//! - `--hud-paused`: draws the pause's veil and mark, to look at them in a capture, which has no input
//!   to pause it.
//! - `--hud-energy <share>` (with `--hud fixed`): the fixed frame's player has an ultimate (M3 Q3c), its
//!   energy meter this full, 0 to 1, and ready at 1: the meter and the ultimate's button draw.
//! - `--display-text latin|japanese`: a title in the display chain's face at 12, 24, 48 and 96 points
//!   over the frame, 360 points tall (`display_text.rs`, M3 U8): the smaller two from the glyph pages,
//!   the larger two from the face's cooked field. The golden scenes `display-text-latin` and
//!   `display-text-japanese` show it, at one and three pixels to the point.
//! - `--game title|new-game|continue` (with `--capture`): the game's own show, as `game_app` stands it, on the
//!   flow the content names (M3 G1, `game_flow::capture`): its title, or past it as that row would, with no
//!   save kept. `--capture-ticks` frames, a tick each, and `--size`; the scene's flags don't reach it.
//! - `--clip <name>` (with `--look-dev` or `--arena`): plays that clip on the character instead of holding it,
//!   looping (M2 N2, `animate.rs`). A name is what a clip's source calls it among those the packs hold for the character (M3 S3), or any clip's
//!   asset ID. Each `--capture-ticks` is a fixed point of it, and `--bench` measures it.
//!   `--clip-in-place` leaves its root motion out, for a shot that frames the character close. The
//!   golden scenes `lookdev-strike-*` and `lookdev-clip-*` show it.
//! - `--blade on|off` (with `--look-dev` or `--arena`): the player's sword in the character's right
//!   hand (M2 O3, `blade.rs`), placed by the hand's joint each frame at the `blade` socket its hit
//!   volumes bake from. On for `--look-dev body` and `--fight`, off otherwise, so no older scene draws
//!   one.
//! - `--springs on|off` (with `--clip` or `--graph`): runs the character's spring bones or not, on by default
//!   (M2 N5). Scenes that don't show them pin them off; `lookdev-strike-springs-*` and
//!   `lookdev-clip-run-springs` do.
//! - `--contact-sheet <clip> <png>`: the clip at evenly spaced ticks, from the front and the side, as
//!   one image (M2 O2, `contact_sheet.rs`). A mode of its own, like `--capture`. `--sheet-cells <n>`
//!   and `--size <cell>` set the grid, `--sheet-ticks <a,b,..>` and `--sheet-views <yaw,yaw,..>` name
//!   what it draws, and `--zoom <factor>` pulls back for a clip that travels.
//! - `--party` (with `--arena` and a fight of the `party` scenario, or `--fight`): the player is whoever of
//!   the party is on the field, in that member's recolor (M2 G7, `party_looks.rs`). The golden scenes
//!   `arena-party-second` and `arena-party-third` show it.
//! - `--arena`: the combat stage (M2 O5, `arena.rs`): the look-dev town's square on its own day,
//!   as a place to fight: the character at the player's spawn point and the enemy at the enemy's,
//!   on the cooked colliders of `content/arena/square.colliders.ron`, seen by `sapphire_camera`'s
//!   director. `--arena-shot follow` is a plain fixed follow instead, which the golden scenes pin,
//!   and `over` looks straight down on the square. `--debug-draw colliders` draws the colliders over
//!   it all. With `--replay <arena replay>` the actors follow the replay, and `--record <file>
//!   --scenario arena` plays one by the bindings, in a window. It's a scene of its own: it can't join
//!   `--look-dev` or anything that can't. The golden scenes `arena-day`, `arena-golden` and
//!   `arena-replay` show it, and `arena-fight-telegraph`, `arena-fight-hit`, `arena-fight-cut` and
//!   `arena-fight-slash` a fight with the enemy.
//! - `--fight` (with `--arena`): player against brain with no file (M2 G6, `combat.rs`), each on its
//!   graph and its IK, the player with its sword. Add `--cues` and `--hud live` for the rest. In a window
//!   whoever's there plays it on combat's bindings alone, as with `--combat-input`, and it starts over
//!   4 s after someone falls, or on `R` or the d-pad's down. With `--script <name>` and `--capture`,
//!   `--bench` or `--offscreen` a script plays it, down the same path, for agents; each fight's log line
//!   says how it went.
//! - `--graph <script>` (with `--look-dev`, without `--clip`): plays the animation graph on the
//!   character from a scripted input (M2 N3, `anim_graph.rs`): `stand`, `walk`, `run`, `strafe`, `back`,
//!   `jump`, `cancel`, `hitstop`, `flinch` or `cut`, at fixed ticks, with a controller and a simulation
//!   beside it. The golden scenes `lookdev-graph-*` show it.
//! - `--ik on|off` (with `--clip` or `--graph`; off by default): solves IK over what plays (M2 N4,
//!   `ik.rs`): feet on the ground under them, by casts through `sapphire_physics`, and a look.
//!   `--ik-ground flat|step|slope` stands the character on a test bed of stone blocks, where the
//!   controller's capsule would rest; `--arena --clip <name> --ik on` stands it on the arena's own
//!   colliders. `--ik-look <x>,<y>,<z>|orbit` is where it looks: a place in the world, up from where it
//!   stands, or something circling it, crossing behind. The golden scenes `lookdev-ik-*` show them.
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
//! - `--color-vision off|protanopia|deuteranopia|tritanopia`: show the finished frame, the HUD
//!   included, as a dichromat sees it (`r.color_vision`, M4 A5, Machado et al. 2009), for checking
//!   that a fight's cues stay apart by shape. With any debug view, in every build.
//! - `--debug-draw shapes|lines:<count>|volumes|colliders|combat`: turn debug draw on and draw a
//!   fixed set of shapes around the character, one of each kind and each way of lasting and facing
//!   a surface (the golden scene `debugdraw-shapes`), or that many lines a frame, for benches; or,
//!   with `--replay` or `--record`, the views of that fight (`combat_views.rs`): its hit and hurt
//!   volumes (the golden scene `debugdraw-volumes`), its colliders, or both. `--arena` draws its
//!   colliders and spawn points with no fight too. `dev` builds have debug draw on anyway
//!   (`debug.draw`), and the overlay's combat panel switches the views; other builds only with this
//!   flag.
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
//! - `--zone <stem>[:<spawner>]`: the zone stage (M3 C1): the look-dev character held at a spawner of the zone
//!   cooked from `<stem>.zone.ron` (`player_start` unless named), under the zone's day, light rigs and probe
//!   grid, the camera behind it. A game mounts a zone like this, by its pack: no code names one. Not with
//!   `--look-dev` or `--arena`, whose town is a stage of its own; `--stream` says how it streams.
//! - `--zone-walk <spline>[:<metres a second>]`: the zone stage's character walks that spline of the zone, the
//!   camera behind it, each fixed step, at a run's speed unless told. Moves only what's drawn. A bench of it adds
//!   a `zone walk` row (streaming's and the uploads' times, what uploaded, the pools' peaks) and fails if any
//!   frame built a pipeline; `--inventory-without ambient` shows that failing, by leaving its grid undeclared.
//! - `--stream off|<entities>:<metres>`: how the town's zone streams (M3 W2): not at all, so the town has
//!   no cells, or with that many entities a frame and that activation radius, instead of the tier's.
//! - `--fog on|off`: draw time of day's fog (M1 W3), on by default. Off leaves it out of every
//!   frame and places no fog cards in the courtyard, so frames draw exactly as before fog, which
//!   the golden scenes from before it pin.
//! - `--plain-post`: draw post as it was before its compositing layers (`PostParams::plain`: no
//!   diffusion glow, character bloom, gradient layers, lens flare, vignette tint or sharpening),
//!   which the golden scenes from before them pin (M1 P2). Post then builds none of the layers'
//!   pipeline variants (`RenderSettings::post_variants`).
//! - `--view-culling on|off`: cull static world instances outside the main view (review RA-O01,
//!   `RenderSettings::view_culling`), on by default. Off draws them all, the same pixels, for
//!   benches that compare the two.
//! - `--sharpening <0..1>`: the player's sharpening (RCAS in post's fused pass) instead of the
//!   `post.sharpening` cvar's, 0 by default (M1 P2).
//! - `--post-effect <preset>[@<seconds>]`, repeatable: push a screen effect preset (M1 P3:
//!   `radial_blur`, `chromatic_aberration`, `inversion`, `duotone`, `speed_lines`,
//!   `impact_frame`) at the start, playing, or held that many seconds into it for captures and
//!   benches. An unknown name fails with the presets' names. The renderer prewarms the post
//!   variants of the presets pushed (`Renderer::prewarm_post`), or in `dev` builds of every
//!   preset, which the overlay fires, so a scene without effects builds none of theirs.
//! - `--effect-scale <flashes>:<motion>`: the player's flashes and camera motion (M4 A2), each 0 to 1, on
//!   every screen effect: flashes on the inversion, the duotone and speed lines, camera motion on the
//!   radial blur and chromatic aberration (`PostEffects::scaled`). `effects-impact-frame-calmer` shows it.
//! - `--view <yaw>,<pitch>`: start the orbit camera at these angles in degrees (yaw orbits right
//!   from in front of the character; a negative pitch looks up at the sky).
//! - `--no-hot-reload` (`dev` builds): watch nothing, so files edited during the run don't
//!   change it.
//! - `--combat-input`: the windowed app's input is combat's bindings alone: every verb on the pad
//!   and the keyboard, its camera on `look`, and `Esc` pausing rather than quitting.
//! - `--live-link off|<addr>|listen=<addr>|connect=<addr>` (`dev` builds): how live link takes
//!   connections instead of listening on 127.0.0.1:7811: listening at another address (one
//!   beyond loopback, such as `0.0.0.0:7811`, takes pushes from the network: an opt-in, logged as
//!   a warning), dialing a tool listening at `<addr>` (behind `adb reverse`), or not at all.
//! - `--debug-overlay[=panels]` (`dev` builds): show the overlay from the start, with its
//!   default panels or a comma-separated list (`stats`, `graph`, `time`, `day`, `effects`,
//!   `combat`, `console`, `cvars`, `all`). Offscreen captures draw it too.
//! - `--cue click|hit|swing`: play this cue (`content/audio/`) at the character on the real audio
//!   device (M2 E1, `audio.rs`); `--cue-repeat <seconds>` plays it again every so often. Slow motion
//!   bends and darkens it, and hitstop freezes `swing`. Only these flags open a device: no device is
//!   a logged state, and the offscreen modes refuse them.
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
//!   - `--replay <file>`, with `--capture` or `--bench`: play a recorded fight beside the scene,
//!     one tick per fixed step, and fail if it leaves the recorded checksums beside the file
//!     (`checksums.<architecture>.txt`) (M2 D2, `combat.rs`). A capture stops at the replay's end
//!     unless `--capture-ticks` picks an earlier tick. A bench goes on past the end, and says so.
//!   - `--record <file> --scenario <name> --script <name>`, with `--capture` or `--bench`: run a
//!     scenario on a script's actions for as many ticks as the mode steps, and write the replay.
//!     Without those two and without `--script`, in the windowed app, it records whoever's playing,
//!     through combat's bindings, and writes the file when the app quits (`combat.rs`).
//!   - `--cues`, with either: fire that simulation's cues as a game does (M2 E3, `cues.rs`), on the
//!     scene's character, or in `--arena` where its actors are: effects, screen effects and camera
//!     shakes, and in the windowed app its
//!     sounds and rumble, which load `audio.spack`. A capture and a bench stay silent.
//!   - `--serial-passes`: submit each frame graph pass on its own and wait for the GPU after
//!     it, logging each at debug level: a GPU crash or device loss then names its pass in the
//!     log, even on drivers that report nothing. Slow; for debugging.
//!
//!   `--capture` and `--bench` exit with an error on an argument they don't know, so a misspelled
//!   pin can't render another picture; the windowed app and `--offscreen` log a warning.
//!
//!   An uncaptured GPU error fails all of these, in every build, with a failing exit and the error's
//!   text, and a capture writes no picture; a shipping app in a window logs it and keeps running.
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
#[cfg(feature = "dev")]
mod combat_panel;
mod contact_sheet;
#[cfg(feature = "dev")]
mod dev;
mod display_text;
mod fight_bench;
mod input;
#[cfg(feature = "dev")]
mod link;
mod offscreen;
mod options;
#[cfg(feature = "dev")]
mod reload;
mod screen;
#[cfg(feature = "dev")]
mod serve;
#[cfg(feature = "dev")]
mod shader_edits;
mod stats;

use std::sync::{Mutex, Once};

use sapphire_core::alloc::EngineAllocator;
use sapphire_core::log::{LogConfig, LogSink};
use sapphire_crash::{CrashConfig, CrashGuard};
use sapphire_platform::{App, DeviceClass, Os, PlatformLogSink};

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
/// and from `SDL_main` on Android, where it must not panic. Each process runs `SDL_main` once:
/// the app leaves `SDL_HINT_ANDROID_ALLOW_RECREATE_ACTIVITY` off, so SDL exits the process when
/// the activity is recreated (which is why the manifest handles every configuration change).
/// Only the first call does anything all the same.
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

/// Command-line help requested by `--help` or `-h`. The desktop entry point prints this
/// before startup or platform initialization, so help needs neither content nor a display.
pub fn command_help() -> Option<&'static str> {
    std::env::args_os().any(|arg| arg.to_str().is_some_and(options::is_help_flag)).then_some(options::USAGE)
}

/// Whether the process's arguments ask for an offscreen mode (`--capture`, `--bench` or
/// `--offscreen`), which `main` runs as [`OffscreenApp`] in the background instead of opening the
/// windowed app.
pub fn offscreen_requested() -> bool {
    std::env::args_os().any(|arg| arg.to_str().is_some_and(options::is_offscreen_flag))
}

/// Whether `args` ask for an offscreen mode: [`offscreen_requested`] for arguments that aren't the
/// process's. Android's `SDL_main` asks it of the launching intent's, and runs [`OffscreenApp`] for
/// a yes (M3 Z2b), so a capture or a bench, and a replay with either, runs on the emulator too.
pub fn asks_offscreen(args: &[String]) -> bool {
    args.iter().any(|arg| options::is_offscreen_flag(arg))
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

sapphire_platform::export_android_main!(
    SandboxApp,
    init = startup,
    offscreen = OffscreenApp,
    when = asks_offscreen
);

#[cfg(test)]
mod tests {
    use super::asks_offscreen;

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|&arg| arg.to_owned()).collect()
    }

    /// Android's entry picks the app by this, so a replay launched by an intent with a capture or a
    /// bench runs offscreen and ends, and one without stays the windowed app.
    #[test]
    fn an_intent_asking_for_a_capture_or_a_bench_runs_offscreen() {
        for asked in [
            &["--capture", "end.png", "--arena", "--replay", "zone-trip.sreplay"][..],
            &["--arena", "--replay", "zone-trip.sreplay", "--bench=1"],
            &["--offscreen", "--arena", "--fight", "--script", "fight-back"],
            &["--contact-sheet", "sheet.png"],
        ] {
            assert!(asks_offscreen(&args(asked)), "{asked:?}");
        }
        for windowed in [
            &[][..],
            &["--arena", "--fight"],
            &["--arena", "--replay", "zone-trip.sreplay"],
            &["--benchmark"],
        ] {
            assert!(!asks_offscreen(&args(windowed)), "{windowed:?}");
        }
    }
}
