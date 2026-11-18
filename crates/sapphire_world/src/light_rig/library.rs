//! The shipped [`LightRigLibrary`], compiled in. The library's type is `sapphire_tod::light_rig`'s
//! (M6 T4), where an editor reflects it without the renderer.

use super::LightRigLibrary;

/// The shipped presets (see [`default_rigs`]).
const DEFAULT_RIGS: &str = include_str!("../../default_light_rigs.ron");

/// Sapphire's presets (`default_light_rigs.ron` beside this crate's manifest), compiled in:
/// `gameplay`, the default, which follows the sun and moon within limits that keep faces lit
/// from the camera's side of the head and from above the horizon, and `portrait`, a key
/// fixed to the camera for dialogue, menus and photo shots.
#[must_use]
pub fn default_rigs() -> LightRigLibrary {
    LightRigLibrary::from_ron(DEFAULT_RIGS).expect("the shipped presets are checked by this module's tests")
}
