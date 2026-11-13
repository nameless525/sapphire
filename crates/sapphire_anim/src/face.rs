//! The face (M5 C5): a character's expression presets on its morph targets, as a layer after the pose.
//!
//! A [`FaceLayer`] is bound once to a character's rig ([`FaceLayer::new`]) and then shows what it's asked:
//! an expression preset at a weight, blended in from whatever it showed before ([`FaceLayer::show`]), and
//! back to the character's resting expression when nothing asks any more ([`FaceLayer::hand_back`]). It
//! writes nothing of its own into the pose: [`FaceLayer::apply`] takes the morph weights the pose made, the
//! graph's procedural blink and any clip's keys among them, and turns them into what's drawn. So it runs
//! after the graph, in presentation, and leaves a body the simulation solved exactly as it was.
//!
//! - **Its own clock.** [`FaceLayer::advance`] moves it by whatever its owner says, real seconds in a fight,
//!   so a face goes on blending while a cutscene or a conversation holds the simulation. A clock that
//!   doesn't move, with nothing asked, changes nothing: [`FaceLayer::take_moved`] says false, and the bits it
//!   would write are the bits it wrote.
//! - **Overrides, as VRM 1.0 has them.** An expression that overrides the blink, the look or the mouth turns
//!   that family down while it shows: `block` entirely once it shows at all, `blend` by its own weight, and
//!   several add up to all of it. The family's presets the layer shows go down by that much, and so do the
//!   family's morph targets as the pose made them, which is how the graph's procedural blink and an
//!   expression-driven look are turned down: the pose's weights on a family's targets are that family's
//!   doing as long as no clip keys them.
//! - **The resting expression** is in the pose's own rest weights (a character holds a soft face at rest), so
//!   the layer takes it out before it adds what it shows: an expression replaces the face rather than
//!   piling on it. At rest, with no mouth, the layer is idle, and [`FaceLayer::apply`] leaves the pose's
//!   weights to the bit.
//! - **The mouth** ([`Mouth`], M5 C6's seam): five viseme weights at a playhead, or an openness that drives
//!   `aa`. As much as it speaks, it replaces the mouth presets the expression shows, and the expression's
//!   mouth override turns it down as it does them.
//!
//! Binding allocates; nothing else does.

use sapphire_assets::rig::{ArchivedCharacterRig, ExpressionOverride, ExpressionPreset};
use sapphire_core::Name;

/// How many presets VRM 1.0 has: what a face's weights are held by, in [`ExpressionPreset::ALL`]'s order.
pub const PRESETS: usize = 18;
const _: () = assert!(ExpressionPreset::ALL.len() == PRESETS);

/// How many visemes a [`Mouth`] holds.
pub const VISEMES: usize = 5;

/// The visemes' presets, in a [`Mouth`]'s order.
pub const VISEME_PRESETS: [ExpressionPreset; VISEMES] = [
    ExpressionPreset::Aa,
    ExpressionPreset::Ih,
    ExpressionPreset::Ou,
    ExpressionPreset::Ee,
    ExpressionPreset::Oh,
];

/// The families an expression can override, in the order [`Bound::overrides`] holds them.
const BLINK: usize = 0;
const LOOK: usize = 1;
const MOUTH: usize = 2;
const FAMILIES: usize = 3;

/// The family `preset` belongs to, if any.
fn family_of(preset: ExpressionPreset) -> Option<usize> {
    use ExpressionPreset::{
        Aa, Blink, BlinkLeft, BlinkRight, Ee, Ih, LookDown, LookLeft, LookRight, LookUp, Oh, Ou,
    };
    match preset {
        Blink | BlinkLeft | BlinkRight => Some(BLINK),
        LookUp | LookDown | LookLeft | LookRight => Some(LOOK),
        Aa | Ih | Ou | Ee | Oh => Some(MOUTH),
        _ => None,
    }
}

/// The preset whose VRM name hashes to `name`.
#[must_use]
pub fn preset_named(name: Name) -> Option<ExpressionPreset> {
    ExpressionPreset::ALL.iter().copied().find(|preset| Name::new(preset.name()) == name)
}

/// What a mouth does at a moment, from outside the face: lip-sync's visemes, or a voice's envelope. Each
/// weight is 0 to 1.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Mouth {
    /// `aa`, `ih`, `ou`, `ee` and `oh`, as [`VISEME_PRESETS`] orders them.
    pub visemes: [f32; VISEMES],
}

impl Mouth {
    /// Saying nothing.
    pub const CLOSED: Self = Self { visemes: [0.0; VISEMES] };

    /// Open by `openness`, 0 to 1, with no shape: an envelope drives `aa`.
    #[must_use]
    pub fn open(openness: f32) -> Self {
        Self { visemes: [openness, 0.0, 0.0, 0.0, 0.0] }.checked()
    }

    /// Each weight within 0 to 1, and anything not a number nothing.
    #[must_use]
    pub fn checked(self) -> Self {
        Self { visemes: self.visemes.map(|weight| if weight > 0.0 { weight.min(1.0) } else { 0.0 }) }
    }

    /// How much it speaks: its weights together, at most 1.
    fn speaking(&self) -> f32 {
        self.visemes.iter().sum::<f32>().min(1.0)
    }
}

/// An expression for the face to show, from whatever decides it: a timeline's face key, a conversation's
/// line.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FaceOrder {
    /// Which asking this is: the same key again changes nothing, and a new one starts a blend.
    pub key: u64,
    /// The preset, by its VRM name's hash (`happy`, `blinkLeft`). A name that's no preset shows nothing new.
    pub expression: Name,
    /// 0 to 1.
    pub weight: f32,
    /// Seconds from what it showed before to this, smoothly: 0 is at once.
    pub blend: f32,
    /// Seconds since the order was given, as its giver counts: a key the playhead passed a frame ago starts
    /// its blend a frame in, so a face lands where its timeline says whatever the frame rate.
    pub since: f32,
}

/// One preset as the character's rig binds it.
#[derive(Clone, Debug)]
struct Bound {
    /// Morph targets and their weights at full.
    targets: Vec<(u16, f32)>,
    /// The weight snaps to 0 or 1, at 0.5.
    binary: bool,
    /// What it does to the blink, look and mouth families, by [`BLINK`], [`LOOK`] and [`MOUTH`].
    overrides: [ExpressionOverride; FAMILIES],
}

impl Default for Bound {
    /// A preset the rig lacks: no targets, overriding nothing.
    fn default() -> Self {
        Self { targets: Vec::new(), binary: false, overrides: [ExpressionOverride::None; FAMILIES] }
    }
}

/// See the [module docs](self).
#[derive(Clone, Debug)]
pub struct FaceLayer {
    /// By [`ExpressionPreset`], all [`PRESETS`] of them: a preset the rig lacks binds nothing.
    presets: Vec<Bound>,
    /// The morph targets each family's presets bind, each once.
    family_targets: [Vec<u16>; FAMILIES],
    /// Every morph target any preset binds, each once: what [`Self::apply`] keeps within 0 to 1.
    targets: Vec<u16>,
    /// The weights the pose's rest weights hold: the face nobody's asked for.
    rest: [f32; PRESETS],
    /// The blend's two ends, and when it started on this clock, over how long.
    from: [f32; PRESETS],
    to: [f32; PRESETS],
    started: f64,
    blend: f32,
    /// The layer's own clock, in seconds.
    now: f64,
    /// The order it shows: `None` once handed back.
    order: Option<u64>,
    mouth: Mouth,
    /// Whether what it would write has changed since [`Self::take_moved`] last said.
    moved: bool,
}

impl FaceLayer {
    /// The layer for a character whose rig is `rig` and whose mesh has `morph_targets`, resting with `rest`
    /// as the pose's rest weights hold it, if any. `None` for a rig with no presets: a face with nothing to
    /// show. Binds of targets past the mesh's are left out, as the graph's blink leaves them.
    #[must_use]
    pub fn new(
        rig: &ArchivedCharacterRig,
        morph_targets: usize,
        rest: Option<(ExpressionPreset, f32)>,
    ) -> Option<Self> {
        let mut presets = vec![Bound::default(); PRESETS];
        let mut any = false;
        for expression in rig.expressions.iter() {
            let Some(preset) = expression.preset.as_ref().copied() else { continue };
            any = true;
            presets[preset as usize] = Bound {
                targets: expression
                    .morph_targets
                    .iter()
                    .map(|bind| (bind.target.to_native() as usize, bind.weight.to_native()))
                    .filter(|&(target, _)| target < morph_targets)
                    .map(|(target, weight)| (target as u16, weight))
                    .collect(),
                binary: expression.binary,
                overrides: [
                    expression.override_blink,
                    expression.override_look_at,
                    expression.override_mouth,
                ],
            };
        }
        if !any {
            return None;
        }
        let mut family_targets: [Vec<u16>; FAMILIES] = Default::default();
        let mut targets = Vec::new();
        for (&preset, bound) in ExpressionPreset::ALL.iter().zip(&presets) {
            for &(target, _) in &bound.targets {
                if let Some(family) = family_of(preset) {
                    family_targets[family].push(target);
                }
                targets.push(target);
            }
        }
        for list in family_targets.iter_mut().chain([&mut targets]) {
            list.sort_unstable();
            list.dedup();
        }
        let mut resting = [0.0; PRESETS];
        if let Some((preset, weight)) = rest {
            resting[preset as usize] = weight.clamp(0.0, 1.0);
        }
        Some(Self {
            presets,
            family_targets,
            targets,
            rest: resting,
            from: resting,
            to: resting,
            started: 0.0,
            blend: 0.0,
            now: 0.0,
            order: None,
            mouth: Mouth::CLOSED,
            moved: false,
        })
    }

    /// Moves its clock on by `seconds`: real time in a fight, nothing in a pause. Nonsense moves nothing.
    pub fn advance(&mut self, seconds: f32) {
        if !(seconds > 0.0 && seconds.is_finite()) {
            return;
        }
        if !self.settled() {
            self.moved = true;
        }
        self.now += f64::from(seconds);
    }

    /// Shows `order`, blending from what it shows now, unless it's the order it shows already.
    pub fn show(&mut self, order: FaceOrder) {
        if self.order == Some(order.key) {
            return;
        }
        self.order = Some(order.key);
        let Some(preset) = preset_named(order.expression) else { return };
        let mut to = [0.0; PRESETS];
        to[preset as usize] = if order.weight > 0.0 { order.weight.min(1.0) } else { 0.0 };
        let since = if order.since > 0.0 { f64::from(order.since) } else { 0.0 };
        self.blend_to(to, order.blend, since);
    }

    /// Goes back to the resting expression over `blend` seconds, if anything was asked of it.
    pub fn hand_back(&mut self, blend: f32) {
        if self.order.take().is_some() {
            self.blend_to(self.rest, blend, 0.0);
        }
    }

    /// The mouth from outside the face from now on: [`Mouth::CLOSED`] for none.
    pub fn set_mouth(&mut self, mouth: Mouth) {
        let mouth = mouth.checked();
        if mouth.visemes.map(f32::to_bits) != self.mouth.visemes.map(f32::to_bits) {
            self.mouth = mouth;
            self.moved = true;
        }
    }

    /// Whether what it writes has changed since this was last asked, which it then forgets: what decides
    /// whether a frame with no new pose writes the morphs again.
    pub fn take_moved(&mut self) -> bool {
        std::mem::take(&mut self.moved)
    }

    /// Whether it leaves the pose's weights as they are: at rest, with no blend running and no mouth.
    #[must_use]
    pub fn is_idle(&self) -> bool {
        self.order.is_none() && self.settled() && self.mouth.speaking() == 0.0
    }

    /// Each preset's weight now, by [`ExpressionPreset`]: the blend's, before overrides and the mouth.
    #[must_use]
    pub fn weights(&self) -> [f32; PRESETS] {
        let share = self.share();
        if share >= 1.0 {
            return self.to;
        }
        std::array::from_fn(|preset| {
            let from = self.from[preset];
            from + (self.to[preset] - from) * share
        })
    }

    /// Turns `weights`, the morph weights the pose made, into what the face shows. Allocates nothing, and
    /// leaves them to the bit while [`Self::is_idle`].
    pub fn apply(&self, weights: &mut [f32]) {
        if self.is_idle() {
            return;
        }
        let mut shown = self.weights();
        for (weight, bound) in shown.iter_mut().zip(&self.presets) {
            if bound.binary {
                *weight = if *weight >= 0.5 { 1.0 } else { 0.0 };
            }
        }
        let speaking = self.mouth.speaking();
        if speaking > 0.0 {
            for (preset, &said) in VISEME_PRESETS.iter().zip(&self.mouth.visemes) {
                let weight = &mut shown[*preset as usize];
                *weight = (*weight * (1.0 - speaking) + said).min(1.0);
            }
        }
        let mut overridden = [0.0_f32; FAMILIES];
        for (&weight, bound) in shown.iter().zip(&self.presets) {
            if weight <= 0.0 {
                continue;
            }
            for (family, rule) in bound.overrides.iter().enumerate() {
                overridden[family] += match rule {
                    ExpressionOverride::None => 0.0,
                    ExpressionOverride::Block => 1.0,
                    ExpressionOverride::Blend => weight,
                };
            }
        }
        // The resting expression out, since what's shown replaces it.
        for (&rest, bound) in self.rest.iter().zip(&self.presets) {
            if rest > 0.0 {
                for &(target, share) in &bound.targets {
                    weights[usize::from(target)] -= rest * share;
                }
            }
        }
        for (family, overridden) in overridden.into_iter().enumerate() {
            let kept = 1.0 - overridden.min(1.0);
            if kept >= 1.0 {
                continue;
            }
            for &target in &self.family_targets[family] {
                weights[usize::from(target)] *= kept;
            }
            for (&preset, weight) in ExpressionPreset::ALL.iter().zip(&mut shown) {
                if family_of(preset) == Some(family) {
                    *weight *= kept;
                }
            }
        }
        for (&weight, bound) in shown.iter().zip(&self.presets) {
            if weight > 0.0 {
                for &(target, share) in &bound.targets {
                    weights[usize::from(target)] += weight * share;
                }
            }
        }
        for &target in &self.targets {
            let weight = &mut weights[usize::from(target)];
            *weight = weight.clamp(0.0, 1.0);
        }
    }

    fn blend_to(&mut self, to: [f32; PRESETS], blend: f32, since: f64) {
        self.from = self.weights();
        self.to = to;
        self.blend = if blend > 0.0 && blend.is_finite() { blend } else { 0.0 };
        self.started = self.now - since;
        self.moved = true;
    }

    /// How far through its blend, eased: 1 once it's over.
    fn share(&self) -> f32 {
        if self.blend <= 0.0 {
            return 1.0;
        }
        let through = ((self.now - self.started) / f64::from(self.blend)) as f32;
        if through >= 1.0 {
            return 1.0;
        }
        let through = through.max(0.0);
        through * through * (3.0 - 2.0 * through)
    }

    fn settled(&self) -> bool {
        self.share() >= 1.0
    }
}

#[cfg(test)]
mod tests {
    use sapphire_assets::rig::{CharacterRig, Expression, MorphTargetWeight};

    use super::*;

    /// The morph targets the test rig binds: one a preset, as the character's VRM has them.
    const NEUTRAL: u16 = 0;
    const FUN: u16 = 1;
    const JOY: u16 = 2;
    const ANGRY: u16 = 3;
    const EYE_CLOSE: u16 = 4;
    const MOUTH_A: u16 = 5;
    const MOUTH_O: u16 = 6;
    const SURPRISED: u16 = 7;
    const TARGETS: usize = 9;

    fn expression(preset: ExpressionPreset, target: u16, overrides: [ExpressionOverride; 3]) -> Expression {
        Expression {
            name: preset.name().to_owned(),
            preset: Some(preset),
            binary: false,
            override_blink: overrides[0],
            override_look_at: overrides[1],
            override_mouth: overrides[2],
            morph_targets: vec![MorphTargetWeight { target: u32::from(target), weight: 1.0 }],
            material_colors: Vec::new(),
            texture_transforms: Vec::new(),
        }
    }

    /// The character's VRM's presets, in miniature: `happy` blends the blink and the mouth down, `surprised`
    /// the mouth alone, and `angry` overrides nothing.
    fn rig() -> sapphire_serialize::cooked::AlignedVec {
        use ExpressionOverride::{Blend, Block, None as Free};
        let expressions = vec![
            expression(ExpressionPreset::Happy, JOY, [Blend, Free, Blend]),
            expression(ExpressionPreset::Angry, ANGRY, [Free; 3]),
            expression(ExpressionPreset::Relaxed, FUN, [Free; 3]),
            expression(ExpressionPreset::Surprised, SURPRISED, [Free, Free, Block]),
            expression(ExpressionPreset::Aa, MOUTH_A, [Free; 3]),
            expression(ExpressionPreset::Oh, MOUTH_O, [Free; 3]),
            expression(ExpressionPreset::Blink, EYE_CLOSE, [Free; 3]),
            expression(ExpressionPreset::Neutral, NEUTRAL, [Free; 3]),
        ];
        let rig = CharacterRig { morph_target_count: TARGETS as u32, expressions, ..CharacterRig::default() };
        rig.to_cooked().expect("it archives")
    }

    const REST: (ExpressionPreset, f32) = (ExpressionPreset::Relaxed, 0.35);

    fn layer() -> FaceLayer {
        let bytes = rig();
        let rig = CharacterRig::read(&bytes).expect("it reads");
        FaceLayer::new(rig, TARGETS, Some(REST)).expect("it has presets")
    }

    /// The pose's weights at rest: the resting expression, a blink half shut and a sleeve morph a clip keys.
    fn posed() -> [f32; TARGETS] {
        let mut weights = [0.0; TARGETS];
        weights[usize::from(FUN)] = 0.35;
        weights[usize::from(EYE_CLOSE)] = 0.5;
        weights[8] = 0.25;
        weights
    }

    fn order(key: u64, expression: &str, weight: f32, blend: f32) -> FaceOrder {
        FaceOrder { key, expression: Name::new(expression), weight, blend, since: 0.0 }
    }

    fn applied(layer: &FaceLayer) -> [f32; TARGETS] {
        let mut weights = posed();
        layer.apply(&mut weights);
        weights
    }

    fn bits(weights: [f32; TARGETS]) -> [u32; TARGETS] {
        weights.map(f32::to_bits)
    }

    #[test]
    fn with_nothing_asked_the_pose_is_drawn_to_the_bit() {
        let mut layer = layer();
        assert!(layer.is_idle());
        assert_eq!(bits(applied(&layer)), bits(posed()), "the blink and the rest as the graph made them");
        layer.advance(1.0);
        assert!(!layer.take_moved());
        assert_eq!(bits(applied(&layer)), bits(posed()));
        // Shown, then handed back and settled: idle again, so to the bit again.
        layer.show(order(1, "angry", 1.0, 0.2));
        layer.advance(0.5);
        layer.hand_back(0.2);
        layer.advance(0.5);
        assert!(layer.is_idle());
        assert_eq!(bits(applied(&layer)), bits(posed()));
    }

    #[test]
    fn an_expression_replaces_the_resting_face() {
        let mut layer = layer();
        layer.show(order(1, "angry", 1.0, 0.0));
        let weights = applied(&layer);
        assert_eq!(weights[usize::from(ANGRY)], 1.0);
        assert!(
            weights[usize::from(FUN)].abs() < 1e-7,
            "the rest's taken out: {}",
            weights[usize::from(FUN)]
        );
        assert_eq!(weights[usize::from(EYE_CLOSE)], 0.5, "angry overrides no blink");
        assert_eq!(weights[8], 0.25, "a target no preset binds is the pose's");
    }

    #[test]
    fn happy_blends_the_blink_and_the_mouth_away_by_its_weight() {
        let mut layer = layer();
        layer.show(order(1, "happy", 0.6, 0.0));
        layer.set_mouth(Mouth::open(1.0));
        let weights = applied(&layer);
        assert!((weights[usize::from(JOY)] - 0.6).abs() < 1e-6);
        assert!((weights[usize::from(EYE_CLOSE)] - 0.5 * 0.4).abs() < 1e-6, "{}", weights[4]);
        assert!((weights[usize::from(MOUTH_A)] - 0.4).abs() < 1e-6, "{}", weights[5]);
        layer.show(order(2, "happy", 1.0, 0.0));
        let weights = applied(&layer);
        assert_eq!((weights[usize::from(EYE_CLOSE)], weights[usize::from(MOUTH_A)]), (0.0, 0.0));
    }

    #[test]
    fn a_block_shuts_its_family_while_it_shows_at_all() {
        let mut layer = layer();
        layer.show(order(1, "surprised", 0.1, 0.0));
        layer.set_mouth(Mouth::open(1.0));
        let weights = applied(&layer);
        assert_eq!(weights[usize::from(MOUTH_A)], 0.0);
        assert_eq!(weights[usize::from(EYE_CLOSE)], 0.5, "the blink's free");
    }

    #[test]
    fn a_mouth_replaces_the_expressions_own_as_much_as_it_speaks() {
        let mut resting = layer();
        let mut layer = layer();
        layer.show(order(1, "oh", 1.0, 0.0));
        assert_eq!(applied(&layer)[usize::from(MOUTH_O)], 1.0);
        layer.set_mouth(Mouth { visemes: [0.25, 0.0, 0.0, 0.0, 0.0] });
        let weights = applied(&layer);
        assert!((weights[usize::from(MOUTH_O)] - 0.75).abs() < 1e-6);
        assert!((weights[usize::from(MOUTH_A)] - 0.25).abs() < 1e-6);
        // At rest a mouth alone moves only the mouth, and leaves the resting face as it was.
        resting.set_mouth(Mouth::open(0.5));
        assert!(!resting.is_idle());
        let weights = applied(&resting);
        assert!((weights[usize::from(MOUTH_A)] - 0.5).abs() < 1e-6);
        assert!((weights[usize::from(FUN)] - 0.35).abs() < 1e-6);
        assert!(Mouth::open(f32::NAN).visemes.iter().all(|&weight| weight == 0.0));
    }

    #[test]
    fn a_blend_eases_on_its_own_clock_and_starts_as_late_as_its_order() {
        let mut late = layer();
        let mut layer = layer();
        layer.show(order(1, "angry", 1.0, 0.4));
        assert!(layer.take_moved());
        assert_eq!(layer.weights()[ExpressionPreset::Angry as usize], 0.0);
        layer.advance(0.2);
        let half = layer.weights();
        assert!((half[ExpressionPreset::Angry as usize] - 0.5).abs() < 1e-6, "smooth: half at half time");
        assert!((half[ExpressionPreset::Relaxed as usize] - 0.175).abs() < 1e-6);
        assert!(layer.take_moved());
        layer.advance(0.3);
        assert_eq!(layer.weights()[ExpressionPreset::Angry as usize], 1.0);
        assert!(layer.take_moved(), "the frame that finished the blend moved");
        layer.advance(0.3);
        assert!(!layer.take_moved(), "settled");
        // The same order again is nothing new.
        layer.show(order(1, "angry", 1.0, 0.4));
        assert!(!layer.take_moved());
        // An order given 0.2 s ago is half through a 0.4 s blend already.
        late.show(FaceOrder { since: 0.2, ..order(1, "angry", 1.0, 0.4) });
        assert!((late.weights()[ExpressionPreset::Angry as usize] - 0.5).abs() < 1e-6);
    }

    #[test]
    fn a_frozen_clock_writes_the_same_bits_and_says_nothing_moved() {
        let mut layer = layer();
        layer.show(order(1, "happy", 1.0, 0.5));
        layer.advance(0.1);
        assert!(layer.take_moved());
        let held = bits(applied(&layer));
        for _ in 0..10 {
            layer.advance(0.0);
            layer.advance(f32::NAN);
            layer.advance(-1.0);
            layer.show(order(1, "happy", 1.0, 0.5));
            assert!(!layer.take_moved());
            assert_eq!(bits(applied(&layer)), held);
        }
    }

    #[test]
    fn a_name_thats_no_preset_shows_nothing_new() {
        let mut layer = layer();
        layer.show(order(1, "grin", 1.0, 0.0));
        assert_eq!(bits(applied(&layer)), bits(posed()));
        assert_eq!(preset_named(Name::new("blinkLeft")), Some(ExpressionPreset::BlinkLeft));
    }

    #[test]
    fn a_rig_with_no_presets_has_no_face() {
        let bytes = CharacterRig::default().to_cooked().unwrap();
        assert!(FaceLayer::new(CharacterRig::read(&bytes).unwrap(), 4, None).is_none());
    }
}
