//! An effect's ground decals (M5 L7): a signed distance shape lying flat on the ground where the effect plays,
//! its outline showing its reach, its fill growing over its life, all from the document. See
//! [`DecalLayerDesc`].
//!
//! A decal takes no random numbers, as a mesh layer takes none, so adding one moves no spark. It resolves on
//! its owner's clock: frozen in its owner's hitstop, slowed with it, read back along the owner's last step.
//!
//! **A telegraph's time** is its actor's: a layer with `until_strike` lives exactly as long as the actor has
//! left before its running move's next strike lands, when its cue says so
//! ([`VfxWorld::set_strike`](crate::VfxWorld::set_strike)), so its fill completes on the step the blow lands,
//! through any hitstop and slow motion on the way, since both clocks are the actor's.

use bevy_reflect::{Reflect, ReflectDeserialize, ReflectSerialize};
use glam::{Quat, Vec3};
use sapphire_look::{VfxDecal, VfxDecalBlend, VfxDecalShape};
use serde::{Deserialize, Serialize};

use crate::budget::VfxBudget;
use crate::curve::Curve;
use crate::document::{Problem, curve_ok, one, one_share, require, share_ok};
use crate::world::Placement;

/// Decal layers an effect may have.
pub const MAX_DECAL_LAYERS: usize = 2;

/// Seconds short of its life's end within which a decal's curves stand at their ends: a tenth of a
/// millisecond, far under a step.
const STRIKE_SLACK: f32 = 1e-4;

/// The largest a decal may be across, metres: an arena's worth.
pub const MAX_DECAL_SIZE: f32 = 64.0;

/// A decal's shape (`VfxDecalShape`), in metres, lying in the ground's plane round where its effect plays;
/// a cone and a lane reach the way the placement faces.
#[derive(Reflect, Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[reflect(Serialize, Deserialize)]
pub enum DecalShape {
    /// A disc of `radius`.
    Disc { radius: f32 },
    /// A band from `inner` to `outer`, the ground inside it spared.
    Ring { inner: f32, outer: f32 },
    /// A cone `angle` degrees across in all, out to `radius`, from `inner` if it's given.
    Sector {
        radius: f32,
        angle: f32,
        #[serde(default)]
        inner: f32,
    },
    /// A lane `width` across, reaching `length` ahead.
    Lane { width: f32, length: f32 },
}

impl From<DecalShape> for VfxDecalShape {
    fn from(shape: DecalShape) -> Self {
        match shape {
            DecalShape::Disc { radius } => Self::Disc { radius },
            DecalShape::Ring { inner, outer } => Self::Ring { inner, outer },
            DecalShape::Sector { radius, angle, inner } => Self::Sector { radius, angle, inner },
            DecalShape::Lane { width, length } => Self::Lane { width, length },
        }
    }
}

/// How a decal meets the ground (`VfxDecalBlend`).
#[derive(Reflect, Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[reflect(Serialize, Deserialize)]
pub enum DecalBlend {
    /// Light over the ground, relative to the display, bright enough for bloom: a telegraph.
    #[default]
    Light,
    /// The ground darkened toward its color: a scorch.
    Darken,
}

impl From<DecalBlend> for VfxDecalBlend {
    fn from(blend: DecalBlend) -> Self {
        match blend {
            DecalBlend::Light => Self::Light,
            DecalBlend::Darken => Self::Darken,
        }
    }
}

/// A ground decal of an effect: what [`VfxDecal`] it resolves to at each moment of its life. Every field but
/// `shape`, `lifetime` and `color` has a default, so a plain one is three lines.
#[derive(Reflect, Clone, Debug, PartialEq, Serialize, Deserialize)]
#[reflect(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DecalLayerDesc {
    /// What errors call it.
    #[serde(default)]
    pub name: String,
    pub shape: DecalShape,
    #[serde(default)]
    pub blend: DecalBlend,
    /// Metres from where its effect plays, across (`x`) and ahead (`z`) along the ground: a blow's scorch where
    /// its blade lands, in front of the fighter.
    #[serde(default)]
    pub offset: [f32; 2],
    /// Seconds on the owner's clock from the effect's start before it shows.
    #[serde(default)]
    pub delay: f32,
    /// Seconds its life lasts: what its curves run over. A tier may shorten it (`VfxBudget::decal_life`)
    /// unless it's `until_strike`.
    pub lifetime: f32,
    /// Live until its actor's next strike lands instead, when the cue that played it knows one (a telegraph):
    /// its life is then the time left on the actor's clock to that strike, and `lifetime` only where there's
    /// none. Never shortened by a tier: it says when the blow lands.
    #[serde(default)]
    pub until_strike: bool,
    /// Seconds it stays after its life, at its curves' ends, fading out: so the whole fill shows on the
    /// frame the blow lands, and goes after.
    #[serde(default)]
    pub linger: f32,
    /// How far the fill has grown over its reach, over its life: 0 none, 1 the whole.
    #[serde(default = "one")]
    pub fill: Curve<f32>,
    /// Linear color over its life, relative to the display: the light it adds, or what it darkens toward.
    pub color: Curve<[f32; 3]>,
    /// Its opacity over its life.
    #[serde(default = "one")]
    pub alpha: Curve<f32>,
    /// The outline's width inside its edge, metres: 0 has none.
    #[serde(default)]
    pub outline: f32,
    /// The share of its opacity the body shows where the fill hasn't reached, 0 to 1.
    #[serde(default = "one_share")]
    pub body: f32,
    /// How far its edge wanders either way, metres, burnt rather than drawn: 0 is a clean edge. A tier may
    /// draw it clean (`VfxBudget::decal_noise`).
    #[serde(default)]
    pub noise: f32,
    /// Which decals draw when more want to than the tier's budget holds: the highest first, and of equal
    /// priority those of the effects spawned first.
    #[serde(default)]
    pub priority: u8,
}

fn size_ok(value: f32) -> bool {
    value.is_finite() && value > 0.0 && value <= MAX_DECAL_SIZE
}

impl DecalLayerDesc {
    /// Seconds its life lasts, from its delay, for a strike `strike` seconds after its effect's start.
    fn life(&self, strike: Option<f32>, budget: &VfxBudget) -> f32 {
        match strike.filter(|_| self.until_strike).map(|strike| strike - self.delay) {
            Some(left) if left.is_finite() && left > 0.0 => left,
            _ if self.until_strike => self.lifetime,
            _ => self.lifetime * budget.decal_life.clamp(0.05, 1.0),
        }
    }

    /// When it ends, seconds from its effect's start.
    pub(crate) fn end(&self, strike: Option<f32>, budget: &VfxBudget) -> f32 {
        self.delay + self.life(strike, budget) + self.linger
    }

    /// Whether it shows `age` seconds into its effect's life.
    pub(crate) fn shows(&self, age: f32, strike: Option<f32>, budget: &VfxBudget) -> bool {
        (self.delay..self.end(strike, budget)).contains(&age)
    }

    pub(crate) fn problem(&self) -> Result<(), Problem> {
        match self.shape {
            DecalShape::Disc { radius } => {
                require(size_ok(radius), "shape", "a radius must be above 0 and at most 64")?;
            }
            DecalShape::Ring { inner, outer } => {
                require(size_ok(outer), "shape", "`outer` must be above 0 and at most 64")?;
                require(
                    inner.is_finite() && (0.0..outer).contains(&inner),
                    "shape",
                    "`inner` must be 0 to `outer`",
                )?;
            }
            DecalShape::Sector { radius, angle, inner } => {
                require(size_ok(radius), "shape", "a radius must be above 0 and at most 64")?;
                require(
                    angle.is_finite() && angle > 0.0 && angle <= 360.0,
                    "shape",
                    "`angle` must be above 0 and at most 360",
                )?;
                require(
                    inner.is_finite() && (0.0..radius).contains(&inner),
                    "shape",
                    "`inner` must be 0 to `radius`",
                )?;
            }
            DecalShape::Lane { width, length } => {
                require(
                    size_ok(width) && size_ok(length),
                    "shape",
                    "`width` and `length` must be above 0 and at most 64",
                )?;
            }
        }
        require(
            self.offset.iter().all(|c| c.is_finite() && c.abs() <= MAX_DECAL_SIZE),
            "offset",
            "must be finite and at most 64",
        )?;
        require(self.delay.is_finite() && self.delay >= 0.0, "delay", "can't be negative")?;
        require(self.lifetime.is_finite() && self.lifetime > 0.0, "lifetime", "must be above 0")?;
        require(self.linger.is_finite() && self.linger >= 0.0, "linger", "can't be negative")?;
        curve_ok(&self.fill, "fill")?;
        require(self.fill.never_negative() && self.fill.at_most_one(), "fill", "must be 0 to 1")?;
        curve_ok(&self.color, "color")?;
        require(self.color.never_negative(), "color", "can't be negative")?;
        curve_ok(&self.alpha, "alpha")?;
        require(self.alpha.never_negative() && self.alpha.at_most_one(), "alpha", "must be 0 to 1")?;
        let size = VfxDecalShape::from(self.shape).size();
        require(
            self.outline.is_finite() && self.outline >= 0.0 && self.outline <= size * 0.5,
            "outline",
            "must be 0 to half the shape's size",
        )?;
        require(share_ok(self.body), "body", "must be 0 to 1")?;
        require(
            self.noise.is_finite() && self.noise >= 0.0 && self.noise <= size * 0.25,
            "noise",
            "must be 0 to a quarter of the shape's size",
        )?;
        Ok(())
    }

    /// What it shows `age` seconds into its effect's life at `placement`, for a strike `strike` seconds after
    /// the effect's start, by `budget`'s tier: `None` before its delay and after its linger.
    pub(crate) fn resolve(
        &self,
        placement: &Placement,
        age: f32,
        strike: Option<f32>,
        budget: &VfxBudget,
    ) -> Option<VfxDecal> {
        let life = self.life(strike, budget);
        let local = age - self.delay;
        if !(0.0..life + self.linger).contains(&local) {
            return None;
        }
        // The owner's clock sums f32 steps, which drift from the strike's own sum by far less than this: within
        // it the fill is whole, so it is on the strike's step.
        let t = if local >= life - STRIKE_SLACK { 1.0 } else { local / life };
        let fade = if local > life && self.linger > 0.0 { 1.0 - (local - life) / self.linger } else { 1.0 };
        let rotation = placement.rotation.normalize();
        let rotation = if rotation.is_finite() { rotation } else { Quat::IDENTITY };
        let [across, ahead] = self.offset;
        Some(VfxDecal {
            shape: self.shape.into(),
            position: placement.position
                + sapphire_math::det::quat_rotate(rotation, Vec3::new(across, 0.0, ahead)),
            rotation,
            color: self.color.sample_vec3(t),
            alpha: (self.alpha.sample(t) * fade).clamp(0.0, 1.0),
            blend: self.blend.into(),
            fill: self.fill.sample(t).clamp(0.0, 1.0),
            outline: self.outline,
            body: self.body,
            noise: if budget.decal_noise { self.noise } else { 0.0 },
        })
    }
}

/// The ground under a placement: its rotation turned so its +Y meets `normal`, keeping the way it faces as
/// near as the slope lets it. What a cue's ground placement is.
#[must_use]
pub fn on_ground(position: Vec3, yaw: f32, normal: Vec3) -> Placement {
    let facing = sapphire_math::det::quat_from_rotation_y(yaw);
    let normal = normal.normalize_or(Vec3::Y);
    let tilt = Quat::from_rotation_arc(Vec3::Y, if normal.y > 0.0 { normal } else { Vec3::Y });
    Placement { position, rotation: (tilt * facing).normalize() }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{EffectId, EffectLibrary, OwnerId, OwnerSteps, VfxWorld};
    use sapphire_look::VfxFrame;

    /// A version 3 document of one effect whose decal layers are `layers`.
    fn document(layers: &str) -> String {
        format!(
            r#"(
                type: "sapphire::vfx::EffectLibrary",
                version: 3,
                data: (effects: [(name: "warning", decals: [{layers}])]),
            )"#
        )
    }

    const WHITE: &str = "color: [(0.0, (1.0, 1.0, 1.0))]";

    fn library(layers: &str) -> EffectLibrary {
        EffectLibrary::from_ron(&document(layers)).expect("valid")
    }

    fn layer(library: &EffectLibrary) -> &DecalLayerDesc {
        &library.effect(EffectId(0)).expect("an effect").decals[0]
    }

    fn error(layers: &str) -> String {
        EffectLibrary::from_ron(&document(layers)).expect_err("invalid").to_string()
    }

    #[test]
    fn a_plain_decal_is_three_lines() {
        let library = library(&format!("(shape: Disc(radius: 2.0), lifetime: 1.0, {WHITE})"));
        let disc = layer(&library);
        assert_eq!(disc.blend, DecalBlend::Light);
        assert_eq!((disc.delay, disc.linger, disc.outline, disc.noise, disc.body), (0.0, 0.0, 0.0, 0.0, 1.0));
        assert!(!disc.until_strike && disc.fill == one() && disc.priority == 0);
        assert!(library.has_decals() && !library.has_meshes());
        let sector = library_of_one(
            "(shape: Sector(radius: 3.0, angle: 90.0), lifetime: 1.0, color: [(0.0, (1.0, 0.0, 0.0))])",
        );
        assert_eq!(layer(&sector).shape, DecalShape::Sector { radius: 3.0, angle: 90.0, inner: 0.0 });
    }

    fn library_of_one(layer: &str) -> EffectLibrary {
        library(layer)
    }

    #[test]
    fn every_bad_decal_field_is_named_with_its_reason() {
        let cases = [
            ("shape: Disc(radius: 0.0), lifetime: 1.0", "`shape` a radius must be above 0"),
            ("shape: Disc(radius: 100.0), lifetime: 1.0", "`shape` a radius must be above 0 and at most 64"),
            ("shape: Ring(inner: 2.0, outer: 1.0), lifetime: 1.0", "`shape` `inner` must be 0 to `outer`"),
            ("shape: Sector(radius: 2.0, angle: 400.0), lifetime: 1.0", "`shape` `angle` must be above 0"),
            (
                "shape: Sector(radius: 2.0, angle: 90.0, inner: 3.0), lifetime: 1.0",
                "`inner` must be 0 to `radius`",
            ),
            ("shape: Lane(width: 1.0, length: -1.0), lifetime: 1.0", "`width` and `length` must be above 0"),
            ("shape: Disc(radius: 1.0), lifetime: 0.0", "`lifetime` must be above 0"),
            ("shape: Disc(radius: 1.0), lifetime: 1.0, delay: -1.0", "`delay` can't be negative"),
            ("shape: Disc(radius: 1.0), lifetime: 1.0, offset: (0.0, 100.0)", "`offset` must be finite"),
            ("shape: Disc(radius: 1.0), lifetime: 1.0, linger: -0.1", "`linger` can't be negative"),
            ("shape: Disc(radius: 1.0), lifetime: 1.0, fill: [(0.0, 1.5)]", "`fill` must be 0 to 1"),
            ("shape: Disc(radius: 1.0), lifetime: 1.0, alpha: [(0.0, -0.5)]", "`alpha` must be 0 to 1"),
            ("shape: Disc(radius: 1.0), lifetime: 1.0, outline: 2.0", "`outline` must be 0 to half"),
            ("shape: Disc(radius: 1.0), lifetime: 1.0, body: 1.5", "`body` must be 0 to 1"),
            ("shape: Disc(radius: 1.0), lifetime: 1.0, noise: 1.0", "`noise` must be 0 to a quarter"),
        ];
        for (fields, reason) in cases {
            let text = format!(r#"(name: "mark", {WHITE}, {fields})"#);
            let error = error(&text);
            assert!(
                error.contains("effect `warning`, decal layer `mark`") && error.contains(reason),
                "{fields}: {error}"
            );
        }
        let unnamed = error(&format!("(shape: Disc(radius: 1.0), lifetime: -1.0, {WHITE})"));
        assert!(unnamed.contains("decal layer 0"), "{unnamed}");
        let misspelled = error(&format!("(shape: Disc(radius: 1.0), lifetime: 1.0, {WHITE}, outlin: 0.1)"));
        assert!(misspelled.contains("outlin"), "{misspelled}");
        let one = format!("(shape: Disc(radius: 1.0), lifetime: 1.0, {WHITE})");
        let many = [one.as_str(); MAX_DECAL_LAYERS + 1].join(", ");
        assert!(error(&many).contains("more than 2 decal layers"));
    }

    /// Version 1 and 2 documents load as they did, and a library with no decals writes none.
    #[test]
    fn older_documents_load_unchanged_and_write_no_decals() {
        for version in [1, 2] {
            let text = format!(
                r#"(
                    type: "sapphire::vfx::EffectLibrary",
                    version: {version},
                    data: (effects: [(name: "dots", emitters: [(name: "a", spawn: Burst(count: 2), lifetime: (0.5, 0.5),
                        size: [(0.0, 0.05)], {WHITE})])]),
                )"#
            );
            let library = EffectLibrary::from_ron(&text).expect("an older document");
            assert!(!library.has_decals());
            let written = library.to_ron().expect("written");
            assert!(!written.contains("decals"), "{written}");
            assert_eq!(EffectLibrary::from_ron(&written).expect("read back"), library);
        }
        let shipped = EffectLibrary::default_effects();
        assert_eq!(EffectLibrary::from_ron(&shipped.to_ron().expect("written")).expect("read"), shipped);
    }

    /// The fill and the color follow its life; after it, it holds its ends and fades over its linger.
    #[test]
    fn fill_runs_over_its_life_and_it_fades_over_its_linger() {
        let library = library(&format!(
            "(shape: Lane(width: 1.0, length: 4.0), delay: 0.1, lifetime: 1.0, linger: 0.2, offset: (0.0, 2.0), \
             fill: [(0.0, 0.0), (1.0, 1.0)], outline: 0.05, body: 0.3, {WHITE})"
        ));
        let lane = layer(&library);
        let budget = VfxBudget::DESKTOP_HIGH;
        let place = on_ground(Vec3::new(1.0, 0.5, 0.0), std::f32::consts::FRAC_PI_2, Vec3::Y);
        assert!(lane.resolve(&place, 0.05, None, &budget).is_none(), "not before its delay");
        let half = lane.resolve(&place, 0.6, None, &budget).expect("halfway");
        assert!((half.fill - 0.5).abs() < 1e-5 && half.alpha == 1.0);
        assert_eq!(half.shape, VfxDecalShape::Lane { width: 1.0, length: 4.0 });
        assert!((half.rotation * Vec3::Z - Vec3::X).length() < 1e-5, "its lane reaches the way it faces");
        assert!(
            (half.position - Vec3::new(3.0, 0.5, 0.0)).length() < 1e-5,
            "two metres ahead: {}",
            half.position
        );
        let lingering = lane.resolve(&place, 1.2, None, &budget).expect("lingering");
        assert!(lingering.fill == 1.0 && (lingering.alpha - 0.5).abs() < 1e-4, "{lingering:?}");
        assert!(lane.resolve(&place, 1.31, None, &budget).is_none(), "gone after its linger");
    }

    /// A telegraph lives until its strike, whatever its lifetime says, and a tier never shortens it; a scorch's
    /// fixed life is the tier's to shorten, and its noise the tier's to drop.
    #[test]
    fn a_strike_sets_a_telegraphs_life_and_a_tier_a_scorchs() {
        let library = library(&format!(
            "(shape: Disc(radius: 1.0), lifetime: 5.0, until_strike: true, fill: [(0.0, 0.0), (1.0, 1.0)], {WHITE}), \
             (shape: Disc(radius: 1.0), blend: Darken, lifetime: 2.0, noise: 0.1, {WHITE})"
        ));
        let effect = library.effect(EffectId(0)).expect("an effect");
        let (telegraph, scorch) = (&effect.decals[0], &effect.decals[1]);
        let low = VfxBudget::MOBILE_LOW;
        let place = Placement::default();
        let fill = |age, strike| telegraph.resolve(&place, age, strike, &low).map(|decal| decal.fill);
        assert_eq!(fill(0.375, Some(0.75)), Some(0.5));
        assert_eq!(fill(0.75, Some(0.75)), None, "its life ends with the strike, with no linger");
        assert_eq!(fill(2.5, None), Some(0.5), "with no strike known, its lifetime");
        assert!(telegraph.shows(4.9, None, &low));
        assert!((scorch.end(None, &low) - 2.0 * low.decal_life).abs() < 1e-6);
        assert_eq!(scorch.end(None, &VfxBudget::DESKTOP_HIGH), 2.0);
        assert_eq!(scorch.resolve(&place, 0.1, None, &low).map(|decal| decal.noise), Some(0.0));
        assert_eq!(scorch.resolve(&place, 0.1, None, &VfxBudget::DESKTOP_HIGH).map(|d| d.noise), Some(0.1));
    }

    /// On a slope, a decal's +Y meets the ground's normal and its +Z still faces as near the way it did.
    #[test]
    fn on_a_slope_it_lies_on_the_ground_facing_its_way() {
        let normal = Vec3::new(-0.5, 3_f32.sqrt() * 0.5, 0.0);
        let place = on_ground(Vec3::ZERO, 0.0, normal);
        assert!((place.rotation * Vec3::Y - normal).length() < 1e-5);
        assert!((place.rotation * Vec3::Z - Vec3::Z).length() < 1e-5, "facing along the slope's level line");
        let flat = on_ground(Vec3::ZERO, 1.0, Vec3::new(0.0, -1.0, 0.0));
        assert!((flat.rotation * Vec3::Y - Vec3::Y).length() < 1e-5, "a ceiling's normal lies flat");
    }

    fn step(world: &mut VfxWorld, owners: &[(OwnerId, f32)], seconds: f32) -> VfxFrame {
        world.advance(&OwnerSteps { world: seconds, owners });
        let mut frame = VfxFrame::default();
        world.write_frame(&mut frame, 1.0);
        frame
    }

    /// A strike set on the spawn sets its telegraph's life, on its owner's clock: a hitstop on the way holds
    /// the fill and slow motion slows it, and it fills on the step its owner's clock reaches the strike.
    #[test]
    fn a_telegraph_fills_on_its_strikes_step_through_hitstop_and_slow_motion() {
        let library = library(&format!(
            "(shape: Disc(radius: 1.0), lifetime: 9.0, until_strike: true, linger: 0.1, \
             fill: [(0.0, 0.0), (1.0, 1.0)], {WHITE})"
        ));
        let mut world = VfxWorld::new(library, VfxBudget::DESKTOP_LOW, 3);
        let foe = OwnerId(5);
        let handle = world.spawn(EffectId(0), foe, Placement::default()).expect("spawned");
        let tick = 1.0 / 60.0;
        world.set_strike(handle, 30.0 * tick);
        let mut fills = Vec::new();
        // Ten ticks, five frozen, ten at half speed, then on.
        let steps = [(10, tick), (5, 0.0), (10, tick * 0.5), (40, tick)];
        let mut owner_time = 0.0_f32;
        for (count, own) in steps {
            for _ in 0..count {
                owner_time += own;
                let frame = step(&mut world, &[(foe, own)], tick);
                fills.push((owner_time, frame.decals.first().map(|decal| decal.fill)));
            }
        }
        let landing =
            fills.iter().position(|(time, _)| *time >= 30.0 * tick - 1e-5).expect("the strike lands");
        assert_eq!(fills[landing].1, Some(1.0), "full on the strike's step: {fills:?}");
        assert!(fills[landing - 1].1.is_some_and(|fill| fill < 1.0), "{fills:?}");
        // Frozen, the fill holds.
        assert_eq!(fills[10].1, fills[14].1);
        assert!(fills[10..15].windows(2).all(|pair| pair[0].1 == pair[1].1));
        assert_eq!(landing, 10 + 5 + 10 + 15 - 1);
    }
}
