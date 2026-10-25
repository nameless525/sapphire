//! The world framework (spec §5.21, layer 4).
//!
//! # Owns
//!
//! - **Time of day** ([`tod`], M1 W1): the `TodTimeline` and `TodCutscene` documents, their
//!   evaluation in OkLab, zone override volumes, a cutscene track, its own clock domain, and
//!   the resolved look the app writes into the frame packet each frame.
//!
//! The rest of spec §5.21 (zones and their streaming cells, prefabs, spawning, zone transitions,
//! activation budgets and encounter volumes) arrives with M3; time of day's volumes become zone
//! data then.
//!
//! # Does not own
//!
//! - Rendering: the renderer sees only the plain scene constants time of day resolves into the
//!   `FramePacket` (rule R4: `sapphire_render` never depends on this crate). The sky, fog,
//!   ambient, shadow and character packages (W2–W5, R4, C1, C2) read those values.
//! - The frame loop and extraction: the app ticks time of day in the fixed step
//!   ([`tod::tick_time_of_day`]), resolves it once per frame and writes the packet.
//! - Cutscene playback: a track plays on the world's clock, or a sequencer
//!   (`sapphire_sequencer`, M3) seeks it.
//! - Loading timelines from packs (A4): the default day is compiled in for now.

pub mod tod;
