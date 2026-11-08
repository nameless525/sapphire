//! Fixed-step graph history, isolated from presentation sampling and gameplay.

use std::sync::Arc;

use sapphire_time::{SUBTICKS_PER_TICK, SimTime};

use super::canonical::Canonical;
use super::{ClipSource, GraphBinding, GraphInput, GraphPlayer};
use crate::CharacterPose;
use crate::pose_math::PoseMath;

/// The two clocks of one actor's graph at a completed fixed step.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GraphStepStamp {
    /// The simulation's fixed-step index, including steps during hitstop.
    pub step: u64,
    /// This actor's own integer clock, independent of other actors' rates.
    pub owner_now: SimTime,
    /// Advances on actor resets, teleports or incompatible bindings.
    pub generation: u32,
}

impl GraphStepStamp {
    /// Feeds the stamp as little-endian integers. The graph and IK state need separate hashing.
    pub fn write_canonical(self, sink: &mut impl FnMut(&[u8])) {
        sink(&self.step.to_le_bytes());
        sink(&self.owner_now.as_subticks().to_le_bytes());
        sink(&self.generation.to_le_bytes());
    }
}

/// A fixed-step sequence was inconsistent before evaluation.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum GraphHistoryError {
    #[error("expected the next fixed step")]
    StepOrder,
    #[error("the graph generation moved backwards")]
    GenerationRewind,
    #[error("the actor's clock moved backwards in the same generation")]
    OwnerRewind,
}

/// An experimental fixed-step owner for the shared graph kernel. A read cannot evaluate a frame
/// or consume a controller event. The clip binding is immutable for the lifetime of the owner.
/// Body IK, publication/interpolation and the remaining deterministic math are separate stages.
pub struct FixedGraphHistory<C: ClipSource> {
    pub(super) player: GraphPlayer,
    pub(super) clips: C,
    stamp: Option<GraphStepStamp>,
}

#[derive(Clone, Copy)]
pub(super) struct PreparedStep {
    stamp: GraphStepStamp,
    pub(super) input: GraphInput,
}

impl<C: ClipSource> FixedGraphHistory<C> {
    /// Creates a fresh owner. The actor's first step may be later than the world's first step.
    #[must_use]
    pub fn new(graph: Arc<GraphBinding>, pose: CharacterPose, seed: u32, clips: C) -> Self {
        let mut player = GraphPlayer::new(graph, pose.into_fixed(), seed);
        player.layers.math = PoseMath::Pinned;
        player.inertia.set_math(PoseMath::Pinned);
        Self { player, clips, stamp: None }
    }

    /// The fixed rest pose and immutable joint layout, available for binding the same rig to IK.
    #[must_use]
    pub fn pose(&self) -> &CharacterPose {
        self.player.pose()
    }

    /// Evaluates one controller sample. The stamp supplies `ticks` and `cut`; the caller
    /// supplies combat position from this actor's same completed step. Equal owner times still
    /// accept discrete events. No earlier generation may be replayed into a live owner.
    pub fn advance(
        &mut self,
        stamp: GraphStepStamp,
        input: GraphInput,
        turn_support: Option<usize>,
    ) -> Result<bool, GraphHistoryError> {
        let prepared = self.prepare(stamp, input, false)?;
        Ok(self.advance_prepared(prepared, turn_support, None, false))
    }

    pub(super) fn prepare(
        &self,
        stamp: GraphStepStamp,
        mut input: GraphInput,
        discontinuity: bool,
    ) -> Result<PreparedStep, GraphHistoryError> {
        let mut reset = discontinuity && self.stamp.is_some();
        if let Some(previous) = self.stamp.filter(|_| !discontinuity) {
            if previous.step.checked_add(1) != Some(stamp.step) {
                return Err(GraphHistoryError::StepOrder);
            }
            if stamp.generation < previous.generation {
                return Err(GraphHistoryError::GenerationRewind);
            }
            reset = stamp.generation != previous.generation;
            if !reset && stamp.owner_now < previous.owner_now {
                return Err(GraphHistoryError::OwnerRewind);
            }
        }
        input.ticks = stamp.owner_now.as_subticks() as f64 / f64::from(SUBTICKS_PER_TICK);
        input.cut = reset;
        Ok(PreparedStep { stamp, input: input.sanitized(self.player.drawn.unwrap_or(0.0)) })
    }

    pub(super) fn advance_prepared(
        &mut self,
        step: PreparedStep,
        turn_support: Option<usize>,
        ground_support: Option<usize>,
        movement_released: bool,
    ) -> bool {
        let PreparedStep { stamp, input } = step;
        let reset = input.cut;
        if reset {
            // Legacy camera cuts can keep gait phase and the defeated pose. A body-generation
            // cut must start like a newly bound actor, even when its owner time is unchanged.
            self.player.layers.state = self.player.graph.start;
            self.player.layers.phase = 0.0;
            self.player.layers.blend.fill(0.0);
            self.player.layers.combat = None;
            // The graph uses this ordinal to distinguish authored landing contacts.
            self.player.stats.transitions = 0;
            self.player.key = None;
        }
        self.player.request_grounded_gather(movement_released);
        let changed = self.player.evaluate_with_support(&input, &self.clips, 0, turn_support, ground_support);
        if reset && let Some(key) = &mut self.player.key {
            key.input.cut = false;
        }
        self.stamp = Some(stamp);
        changed
    }

    /// Feeds the fixed graph's history to a canonical byte sink. Bind the immutable graph, rig
    /// and clip revision externally. This layout covers graph history only, including the pose
    /// pair and transition offsets. A body owner must still write root, IK, gaze and turn state.
    pub fn write_canonical(&self, sink: &mut impl FnMut(&[u8])) {
        // Graph history layout, independent of the eventual gameplay checksum layout.
        sink(&5_u32.to_le_bytes());
        sink(&[u8::from(self.stamp.is_some())]);
        if let Some(stamp) = self.stamp {
            stamp.write_canonical(sink);
        }
        Canonical(sink).player(&self.player);
    }

    /// The most recent accepted stamp and read-only graph, if this owner has stepped.
    #[must_use]
    pub fn current(&self) -> Option<(GraphStepStamp, &GraphPlayer)> {
        Some((self.stamp?, &self.player))
    }
}
