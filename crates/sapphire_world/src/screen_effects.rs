//! Combat's screen effects (M1 P3; look pillar 8, M1 plan §2: "combat screen grammar"): the
//! radial blurs, impact frames and speed lines that sell a hit in Wuthering Waves and Zenless Zone
//! Zero, as a stack of effects that gameplay pushes and that play out on their own timelines.
//! M1 builds the hooks; M2 authors the effects for real combat.
//!
//! # Data
//!
//! - **[`ScreenEffect`]**: one effect at full weight: a radial blur, chromatic aberration, an
//!   inversion, a duotone or speed lines, each with its parameters (the renderer's
//!   [`PostEffects`](sapphire_render::PostEffects) as authored), and for those drawn over the
//!   frame an [`EffectMask`]: everywhere, the world alone (an impact frame that keeps characters
//!   colored, lines behind characters), or characters alone.
//! - **[`Envelope`]**: its weight over its own seconds: a pulse (attack, hold, release), a
//!   sustain held until released, or up to eight keys, linear between them, where two keys at one
//!   time cut, for impact frames that flash on exact frames.
//! - **[`EffectLayer`]**: an effect, its envelope, a delay after the push and the clock it plays
//!   on ([`EffectClock`]).
//! - **[`EffectPreset`]** / **[`EffectLibrary`]** (an `EffectLibrary` document, schema 1): named
//!   presets of one to eight layers that a push starts together, checked however they are made.
//!   [`EffectLibrary::default_effects`] (`default_screen_effects.ron`) is compiled in until packs
//!   carry it, and development builds hot reload it (A8): a sample preset per effect, and
//!   `impact_frame`, all of them at once.
//!
//! # The stack
//!
//! [`EffectStack`] (an ECS resource) holds the layers playing, in push order, each with its time
//! on its clock. Gameplay pushes a preset ([`EffectStack::push`], or [`EffectStack::push_at`] to
//! center it on a hit's point on screen) and gets a handle to release a sustained layer, stop or
//! hold the push. The fixed step advances it ([`tick_screen_effects`]) and drops the layers that
//! ended; once per frame [`EffectStack::resolve`] evaluates each layer's envelope at the frame's
//! time and blends the layers bottom to top, kind by kind, into the frame packet's
//! [`PostParams::effects`](sapphire_render::PostParams::effects): a layer's strength mixes from
//! what the layers under it resolved to by its weight, and its center and colors by its share of
//! the result, so the latest push shows over what was playing as it fades in, and an effect
//! fading in over nothing keeps its own center and colors. The renderer sees only those values
//! (rule R4); an empty stack resolves to none, which costs the renderer nothing.
//!
//! **Fixed capacity: [`STACK_CAPACITY`], 16 layers.** A heavy moment of combat is a few
//! overlapping hits of an impact preset (four layers each, their blurs fading over a few tenths
//! of a second as the next hit lands) on top of an ultimate's sustained layers; layers of a kind
//! blend toward the top one anyway, so more would add little. A full stack drops the layer with
//! the least time left (the one about to fade out by itself), or the new layer if it would end
//! first, and counts it. Nothing allocates: layers are `Copy` (an envelope's keys are a fixed
//! array), pushing copies them in, and resolving writes a `Copy` value.
//!
//! # Clocks, pause and hit-stop
//!
//! The stack's clock is a [`LocalClock`](sapphire_time::LocalClock) issued by the world clock
//! (spec §4.5), as time of day's is. Each layer plays on one of two clocks:
//!
//! - **[`EffectClock::World`]** (the default): the world's time. Pause holds it, and slow motion
//!   (a perfect dodge's 0.2×) and cinematic scales stretch it, so a blur fading over a slowed
//!   moment fades slowly with it.
//! - **[`EffectClock::Real`]**: fixed steps at full speed while the world runs, as hit-stop counts
//!   down: an impact frame authored as three frames lasts three frames even in slow motion. A
//!   stopped world (pause, the overlay's time controls) holds these too.
//!
//! **Hit-stop (M2)** freezes the attacker's and the victim's own clocks
//! ([`LocalClock::apply_hitstop`](sapphire_time::LocalClock::apply_hitstop)), never the world's,
//! so effects pushed with a hit play on through it, on either clock: the impact frame shows
//! while the two combatants hold still, which is the point. M2 pushes a hit's preset in the same
//! step it applies hit-stop, with its impact frames as [`EffectClock::Real`] keys as long as the
//! hit-stop's ticks. A finish-freeze that stops the whole world (a cinematic scale of 0) holds
//! both clocks: effects that must play through it need a world that isn't stopped (hit-stop on
//! every combatant instead), or a scale source of the stack's own, which M2 decides with the
//! freeze.
//!
//! Every layer's time is interpolated `alpha` of the way to the next fixed step when resolved,
//! as presentation interpolates everything it draws; held layers aren't.
//!
//! # Later milestones
//!
//! - **Gameplay's cues (M2):** skills' timelines push presets through the cue queue (spec §5.22,
//!   rule R3), centered on hit points projected through the camera; a layer anchored to a point
//!   in the world, re-projected each frame, for a hit point the camera moves past.
//! - **Hit-stop and impact timing (M2)**, above; cut-ins (an extra view of a character on a
//!   stage of its own) and comic panels are views, not post (the research notes' §10).
//! - **Curves on parameters:** envelopes scale an effect's strength; keyed centers (a blur
//!   sweeping across the frame) and keyed colors.
//! - **More effects:** halftone, a world desaturation that keeps characters colored for perfect
//!   dodges, CRT and glitch transitions (ZZZ's TV look), screen flashes.
//! - **Presets as content** loaded from packs (A4) and pushed by live link (A9) through
//!   [`EffectLibrary::from_ron`], as desktop hot reload already reloads the shipped document.
//! - **Cutscene tracks** (`sapphire_sequencer`) pushing and seeking presets on a shot's clock.
//! - **`sapphire_vfx` (spec §5.16)** publishes screen effects as `PostParams`: when it arrives
//!   it can own the stack, which then moves below the world crate.

mod envelope;
mod layer;
mod library;
mod stack;

#[cfg(test)]
mod tests;

pub use envelope::{Envelope, EnvelopeError, EnvelopeKeys, MAX_ENVELOPE_KEYS, MAX_ENVELOPE_SECONDS};
pub use layer::{
    ChromaticAberrationEffect, DuotoneEffect, EffectClock, EffectKind, EffectLayer, EffectMask,
    InversionEffect, LayerError, RadialBlurEffect, ScreenEffect, SpeedLinesEffect,
};
pub use library::{EffectLibrary, EffectLibraryError, EffectPreset, EffectPresetId, MAX_PRESET_LAYERS};
pub use stack::{
    EffectHandle, EffectStack, EffectStackStats, PlayingEffect, STACK_CAPACITY, tick_screen_effects,
};
