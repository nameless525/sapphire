//! Presents the simulation’s constrained body and layers cosmetic springs over it.

use std::time::Duration;

use sapphire_anim::graph::BodyReadSpan;
use sapphire_anim::{BodySample, ClipSource, SpringFrame, SpringWork};
use sapphire_gameplay::{ActorId, Simulation};
use sapphire_math::{Affine3A, Mat4};
use sapphire_time::SUBTICKS_PER_TICK;
use sapphire_world::light_rig::CharacterLightRig;

use super::{ClipTable, GraphDriver};
use crate::animate::SpringView;
use crate::scene::Figure;
use crate::{RenderPose, RenderTransform};
/// A render consumer of an authoritative simulation body.
pub struct FixedBodyRenderer {
    sample: BodySample,
    source_revision: u64,
    character_versions: Option<(u32, u32, u32)>,
    ready: bool,
    last_pose: Option<((u64, u64, f32), f32)>,
    impact_release: Option<(f32, f64)>,
    last_generation: Option<u32>,
    reset_springs: bool,
    cosmetic_morph: Option<usize>,
}

impl FixedBodyRenderer {
    /// Duration of the impact release on each actor's own clock.
    pub const IMPACT_RELEASE_TICKS: u64 = 2;

    pub(super) fn bind_simulation(
        driver: &mut GraphDriver,
        clips: &ClipTable<'_>,
        versions: Option<(u32, u32, u32)>,
        sample: BodySample,
    ) -> Result<Self, String> {
        let cosmetic_morph = Self::check_cosmetics(driver, &sample, clips)?;
        if !sample.bind_presentation(driver.player.pose_mut()) {
            return Err("the simulation body's cooked layout or rest morphs differ from Stage".to_owned());
        }
        Ok(Self::new(sample, clips.version(), versions, cosmetic_morph))
    }

    fn check_cosmetics(
        driver: &GraphDriver,
        sample: &BodySample,
        clips: &impl ClipSource,
    ) -> Result<Option<usize>, String> {
        if let Some((writer, joint)) =
            driver.springs.as_ref().and_then(|springs| springs.protected_write(sample.protected_joints()))
        {
            let name = sample.joint_names().get(joint).map_or("out of range", String::as_str);
            return Err(format!("fixed body {writer} would change protected joint {name} ({joint})"));
        }
        let Some((name, slot)) = driver.springs.as_ref().and_then(sapphire_anim::SpringBones::underarm_morph)
        else {
            return Ok(None);
        };
        let (names, _) = sample.morphs();
        if names.and_then(|names| names.get(slot)).map(String::as_str) != Some(name)
            || driver.player.pose().rest_weights().get(slot).map(|value| value.to_bits()) != Some(0)
        {
            return Err(format!("the underarm clearance morph {name} ({slot}) is not a matching zero slot"));
        }
        for id in driver.player.clips_in_use() {
            let clip =
                clips.clip(id).ok_or_else(|| format!("missing clip {id} while binding underarm morph"))?;
            if clip.weights.iter().any(|track| track.target.to_native() as usize == slot) {
                return Err(format!("clip {id} authors the cosmetic underarm morph {name} ({slot})"));
            }
        }
        Ok(Some(slot))
    }

    fn new(
        sample: BodySample,
        source_revision: u64,
        character_versions: Option<(u32, u32, u32)>,
        cosmetic_morph: Option<usize>,
    ) -> Self {
        Self {
            sample,
            source_revision,
            character_versions,
            ready: false,
            last_pose: None,
            impact_release: None,
            last_generation: None,
            reset_springs: false,
            cosmetic_morph,
        }
    }

    pub fn read_simulation(&mut self, simulation: &Simulation, actor: ActorId, alpha: f32) -> bool {
        self.impact_release = None;
        self.ready = simulation.read_body(actor, alpha, &mut self.sample);
        self.ready
    }

    /// Publishes the exact resolved contact sample through the existing presentation path.
    pub fn read_impact(&mut self, sample: &BodySample) -> bool {
        self.impact_release = None;
        self.ready = self.sample.copy_from_sample(sample);
        self.ready
    }

    /// Applies the bounded two-owner-tick release on the live presentation sample.
    pub fn release_impact(&mut self, held: &BodySample, origin: &BodySample) {
        let (Some(live), Some(hold), Some(base)) = (self.sample.span(), held.span(), origin.span()) else {
            return;
        };
        let live_ticks = body_ticks(live);
        let start = base.before.owner_now.as_subticks() as f64 / f64::from(SUBTICKS_PER_TICK);
        let elapsed = ((live_ticks - start) / Self::IMPACT_RELEASE_TICKS as f64).clamp(0.0, 1.0);
        let retained = 1.0 - elapsed * elapsed * (3.0 - 2.0 * elapsed);
        if retained > 0.0 && self.sample.apply_impact_residual(held, origin, retained as f32) {
            let spring_ticks = live_ticks - retained * (start - body_ticks(hold));
            self.impact_release = Some((retained as f32, spring_ticks));
        }
    }

    pub fn discontinuity(&mut self) {
        self.ready = false;
        self.impact_release = None;
        self.last_pose = None;
        self.last_generation = None;
        self.reset_springs = true;
    }

    pub fn root(&self) -> Option<Mat4> {
        self.ready.then(|| Mat4::from(self.sample.root().affine()))
    }

    pub fn check_source(&self, clips: &ClipTable<'_>, versions: Option<(u32, u32, u32)>) {
        assert_eq!(clips.version(), self.source_revision, "fixed body clip binding changed");
        assert_eq!(versions, self.character_versions, "fixed body rig binding changed");
    }

    pub fn publish(
        &mut self,
        driver: &mut GraphDriver,
        render: &mut RenderPose,
        figure: &mut Figure,
        rig: &mut CharacterLightRig,
        view: &SpringView,
    ) -> (bool, SpringWork, Duration) {
        assert!(self.ready, "presenting requires a body read");
        let span = self.sample.span().expect("read a stamped body");
        let reset = std::mem::take(&mut self.reset_springs)
            || self.last_generation.is_some_and(|before| before != span.after.generation);
        if reset {
            self.last_pose = None;
            if let Some(springs) = &mut driver.springs {
                springs.reset();
            }
        }
        self.last_generation = Some(span.after.generation);
        let root = self.sample.root().affine();
        let active = driver.springs_run(view, &RenderTransform(Mat4::from(root)), figure);
        let retained = self.impact_release.map_or(0.0, |(retained, _)| retained);
        let key = self.sample.pose_key().map(|key| (key, retained));
        let pose_moved = key != self.last_pose;
        if pose_moved {
            let pose = driver.player.pose_mut();
            assert!(self.sample.publish_to(pose), "fixed body rig binding changed");
            *figure = driver.joints.moving(pose);
            rig.head = figure.head;
        }
        self.last_pose = key;
        let ticks = self.impact_release.map_or_else(|| body_ticks(span), |(_, ticks)| ticks);
        let frame = SpringFrame {
            world_from_model: Affine3A::from_mat4(Mat4::from(root)),
            ticks,
            pose_moved,
            active,
        };
        let (springs, elapsed) = driver.run_springs(&frame, self.sample.grounded(), figure);
        let pose = driver.player.pose();
        assert!(
            self.sample.protected_matches_with_morph(pose, self.cosmetic_morph),
            "cosmetics changed the authoritative contact body"
        );
        if pose_moved || springs.wrote_pose {
            pose.write_palette(&mut render.palette);
            render.morph_weights.copy_from_slice(pose.weights());
        }
        driver.posed = true;
        (pose_moved, springs, elapsed)
    }
}

fn body_ticks(span: BodyReadSpan) -> f64 {
    let after = span.after.owner_now.as_subticks();
    let before = span.before.owner_now.as_subticks();
    let behind = (1.0 - f64::from(span.fraction)) * (after - before) as f64;
    after as f64 / f64::from(SUBTICKS_PER_TICK) - behind / f64::from(SUBTICKS_PER_TICK)
}
