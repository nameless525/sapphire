//! The world framework (spec §5.21, layer 4).
//!
//! # Owns
//!
//! - **Time of day** ([`tod`], M1 W1): the `TodTimeline` and `TodCutscene` documents, their
//!   evaluation in OkLab, zone override volumes, a cutscene track, its own clock domain, and
//!   the resolved look the app writes into the frame packet each frame.
//! - **The character light rig** ([`light_rig`], M1 C2): presets for the key light and stylized
//!   GI characters are lit by (the `LightRigLibrary` document), each character's choice of
//!   preset, and their resolution under time of day and the camera into the lighting the app
//!   writes into the frame packet per character.
//! - **The proximity fade** ([`fade`], M1 C7): how a character fades out as the camera comes
//!   close to its body, resolved per character into the opacity the app writes into the frame
//!   packet.
//! - **Combat's screen effects** ([`screen_effects`], M1 P3): the effect presets (the
//!   `EffectLibrary` document) and the stack of effects gameplay pushes, each on its own timeline
//!   and clock, resolved once per frame into the post effects the app writes into the frame
//!   packet.
//!
//! The rest of spec §5.21 (zones and their streaming cells, prefabs, spawning, zone transitions,
//! activation budgets and encounter volumes) arrives with M3; time of day's volumes become zone
//! data then.
//!
//! # Does not own
//!
//! - Rendering: the renderer sees only the plain scene constants time of day resolves into the
//!   `FramePacket`, and each character's resolved lighting (rule R4: `sapphire_render` never
//!   depends on this crate). The sky, fog, ambient, shadow and character packages (W2–W5, R4,
//!   C1, C2) read those values. The zone's sky material (its maps and the shapes of discs and
//!   clouds) is content the app hands the renderer beside them.
//! - The frame loop and extraction: the app ticks time of day and the screen effects in the
//!   fixed step ([`tod::tick_time_of_day`], [`screen_effects::tick_screen_effects`]), resolves
//!   them once per frame and writes the packet.
//! - Deciding when effects play: gameplay pushes them (M2's cues; the sandbox and the overlay
//!   for now).
//! - Cutscene playback: a track plays on the world's clock, or a sequencer
//!   (`sapphire_sequencer`, M3) seeks it.
//! - Loading timelines and presets from packs (A4): the default day, the default light rig
//!   presets and the sample effect presets are compiled in for now.

pub mod fade;
pub mod light_rig;
pub mod screen_effects;
pub mod tod;
