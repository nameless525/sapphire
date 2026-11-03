//! `--vfx <show>`: plays `sapphire_vfx`'s shipped effects at fixed ticks around the character (M2
//! E2), for golden images, benches and looking at them. Everything it plays is on a schedule of
//! simulation ticks and seeded, so a capture is the same frame in every run. A swing here is a circle's
//! arc in front of the character, passed to [`VfxWorld::sample_trail`] at sub-tick rate. A fight's trails
//! leave its real blades: `trails.rs`.
//!
//! The attacker's and the victim's effects run on the character's own clock, so its hitstop (the
//! `hit` show's, or a key press's) freezes them through it, and world effects keep running.

use std::time::Duration;

use sapphire_math::{Quat, Vec3};
use sapphire_render::VfxFrame;
use sapphire_render::quality::TierId;
use sapphire_time::Stopwatch;
use sapphire_vfx::{
    EffectHandle, EffectId, EffectLibrary, OwnerId, OwnerSteps, Placement, VfxBudget, VfxWorld,
};

/// Whose clock the attacker's effects run on, and the victim's.
const ATTACKER: OwnerId = OwnerId(1);
const VICTIM: OwnerId = OwnerId(2);

/// Seeded, so every run's particles land alike.
const SEED: u64 = 0x5EED;

/// What `--vfx` plays.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VfxShow {
    /// A diagonal sword arc every second, over the character.
    Arc,
    /// The same arc, whose blade strikes something halfway: a spark and a hitstop on the
    /// character's clock, which holds both still, and dust on the world's, which doesn't stop.
    Hit,
    /// A puff of dust on the ground every half second.
    Dust,
    /// A duel's worst moment held still (the bench's load): two sword trails always swinging, about
    /// 200 sparks and 50 dust particles alive at once.
    Duel,
}

impl VfxShow {
    pub const ALL: [Self; 4] = [Self::Arc, Self::Hit, Self::Dust, Self::Duel];

    pub fn name(self) -> &'static str {
        match self {
            Self::Arc => "arc",
            Self::Hit => "hit",
            Self::Dust => "dust",
            Self::Duel => "duel",
        }
    }

    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|show| show.name() == name)
    }
}

/// The plain table's row for a quality tier.
pub fn budget(tier: TierId) -> VfxBudget {
    match tier {
        TierId::DesktopHigh => VfxBudget::DESKTOP_HIGH,
        TierId::DesktopLow => VfxBudget::DESKTOP_LOW,
        TierId::MobileHigh => VfxBudget::MOBILE_HIGH,
        TierId::MobileLow => VfxBudget::MOBILE_LOW,
    }
}

/// Where dust rises: paving the look-dev shots can see, as a close shot can't the character's feet.
const DUST_SPOT: Vec3 = Vec3::new(0.7, 0.02, -2.3);

/// The ticks a swing takes: a sixth of a second, as a light attack's strike.
const SWING_TICKS: f32 = 10.0;
/// A swing starts each time the attacker's clock reaches a multiple of this.
const SWING_PERIOD: u32 = 60;
/// Where in its swing a blade lands a hit.
const HIT_AT: u32 = 6;
/// A hit's hitstop.
pub(crate) const HIT_STOP_TICKS: u16 = 6;
/// The trail's socket samples per tick: 240 a second, or half that on Mobile-Low.
const SAMPLES_PER_TICK: u32 = 4;

/// A blade's circle: where it pivots and how far its base and tip are from there, and which way it
/// sweeps, from one angle to the other. The circle's plane leans toward the camera.
#[derive(Clone, Copy, Debug)]
struct Blade {
    pivot: Vec3,
    base_reach: f32,
    tip_reach: f32,
    /// Degrees, in the plane: 0 toward +X, 90 up.
    from: f32,
    to: f32,
    /// The plane's lean toward +Z, degrees.
    lean: f32,
}

impl Blade {
    /// The character's: from high on one side, down across its body, from over the other shoulder.
    const ATTACKER: Self = Self {
        pivot: Vec3::new(0.45, 1.55, 0.35),
        base_reach: 0.45,
        tip_reach: 1.1,
        from: 165.0,
        to: 305.0,
        lean: 20.0,
    };
    /// Its opponent's, beside it, the other way across.
    const VICTIM: Self = Self {
        pivot: Vec3::new(-0.9, 1.0, -0.3),
        base_reach: 0.25,
        tip_reach: 0.9,
        from: 20.0,
        to: 200.0,
        lean: 15.0,
    };

    /// The base's and tip's sockets, `share` of the way through the swing.
    fn sockets(&self, share: f32) -> (Vec3, Vec3) {
        let angle = (self.from + (self.to - self.from) * share).to_radians();
        let lean = self.lean.to_radians();
        let out = Vec3::new(angle.cos(), angle.sin() * lean.cos(), angle.sin() * lean.sin());
        (self.pivot + out * self.base_reach, self.pivot + out * self.tip_reach)
    }
}

/// Eased through the swing: a blade starts and ends slower than it passes through its middle.
fn eased(share: f32) -> f32 {
    let t = share.clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// What a show has running, and its schedule.
#[derive(Clone, Debug)]
pub(crate) struct VfxPlayer {
    show: VfxShow,
    world: VfxWorld,
    spark: EffectId,
    sword: EffectId,
    dust: EffectId,
    /// The blades' trails, while they're swinging.
    blades: [Option<EffectHandle>; 2],
    /// Ticks on the world's clock, and on the character's, which stops through a hitstop.
    world_ticks: u32,
    character_ticks: u32,
    /// Sub-tick samples a tick: the budget's, and how many a blade's trail keeps.
    samples_per_tick: u32,
    /// Spawns so far, which scatters where the duel's land.
    scatter: u32,
    /// The time spent in steps since the last frame was written.
    stepping: Duration,
}

impl VfxPlayer {
    /// A player of `show` within `budget`.
    pub(crate) fn new(show: VfxShow, budget: VfxBudget) -> Self {
        let library = EffectLibrary::default_effects();
        let effect = |name: &str| library.find(name).expect("a shipped effect");
        let (spark, sword, dust) = (effect("hit_spark"), effect("sword_trail"), effect("dust_puff"));
        Self {
            show,
            world: VfxWorld::new(library, budget, SEED),
            spark,
            sword,
            dust,
            blades: [None, None],
            world_ticks: 0,
            character_ticks: 0,
            // Half the samples where the budget smooths the least, as a phone would take them.
            samples_per_tick: if budget.trail_subdivisions <= 2 {
                SAMPLES_PER_TICK / 2
            } else {
                SAMPLES_PER_TICK
            },
            scatter: 0,
            stepping: Duration::ZERO,
        }
    }

    /// One fixed step, with how long each clock moved by: the character's stops in hitstop. Returns
    /// a hitstop to start on the character's clock, if a hit landed. Allocates nothing.
    ///
    /// The order is what a game's would be: what starts this tick spawns before the clocks move, so
    /// it shows on this tick's frame, and sockets are sampled after, back from the clock's new time.
    pub(crate) fn step(&mut self, world_step: f32, character_step: f32) -> Option<u16> {
        let stopwatch = Stopwatch::start();
        let hitstop = self.start_what_starts(character_step);
        let owners = [(ATTACKER, character_step), (VICTIM, character_step)];
        self.world.advance(&OwnerSteps { world: world_step, owners: &owners });
        if character_step > 0.0 {
            self.sample_blades(character_step);
            self.character_ticks += 1;
        }
        if world_step > 0.0 {
            self.world_ticks += 1;
        }
        self.stepping += stopwatch.elapsed();
        hitstop
    }

    /// This tick's spawns, by each clock's schedule.
    fn start_what_starts(&mut self, character_step: f32) -> Option<u16> {
        let swing_tick = self.character_ticks % SWING_PERIOD;
        let moving = character_step > 0.0;
        match self.show {
            VfxShow::Arc | VfxShow::Hit if moving && swing_tick == 0 => {
                self.blades[0] = self.world.spawn(self.sword, ATTACKER, Placement::default());
                None
            }
            VfxShow::Hit if moving && swing_tick == HIT_AT => {
                let (_, tip) = Blade::ATTACKER.sockets(eased(HIT_AT as f32 / SWING_TICKS));
                self.world.spawn(self.spark, VICTIM, facing_camera(tip));
                // The blow's kick on the ground, on the world's clock: it goes on through the hitstop.
                self.world.spawn(self.dust, OwnerId::WORLD, Placement::at(DUST_SPOT));
                Some(HIT_STOP_TICKS)
            }
            VfxShow::Dust if self.world_ticks.is_multiple_of(30) => {
                self.world.spawn(self.dust, OwnerId::WORLD, Placement::at(DUST_SPOT));
                None
            }
            VfxShow::Duel => {
                self.start_duels_moment();
                None
            }
            _ => None,
        }
    }

    /// Steady spawns that keep about 200 sparks and 50 dust particles alive, and two trails.
    fn start_duels_moment(&mut self) {
        // Two hits of 26 particles every 4 ticks, which live about 15 ticks on average.
        if self.world_ticks.is_multiple_of(4) {
            for _ in 0..2 {
                let at = self.scattered(Vec3::new(-0.1, 1.05, 0.4), Vec3::new(0.7, 0.45, 0.3));
                self.world.spawn(self.spark, OwnerId::WORLD, facing_camera(at));
            }
        }
        // 10 dust particles every 9 ticks, which live about 43 ticks on average.
        if self.world_ticks.is_multiple_of(9) {
            let at = self.scattered(DUST_SPOT, Vec3::new(1.4, 0.0, 0.5));
            self.world.spawn(self.dust, OwnerId::WORLD, Placement::at(at));
        }
        for (index, owner) in [ATTACKER, VICTIM].into_iter().enumerate() {
            if self.blades[index].is_none() {
                self.blades[index] = self.world.spawn(self.sword, owner, Placement::default());
            }
        }
    }

    /// This tick's samples of the blades' sockets, oldest first, at sub-tick rate.
    fn sample_blades(&mut self, step: f32) {
        match self.show {
            VfxShow::Arc | VfxShow::Hit => {
                let Some(handle) = self.blades[0] else { return };
                let tick = self.character_ticks % SWING_PERIOD;
                for sample in 1..=self.samples_per_tick {
                    let share = sample as f32 / self.samples_per_tick as f32;
                    let swing = (tick as f32 - 1.0 + share) / SWING_TICKS;
                    let (base, tip) = Blade::ATTACKER.sockets(eased(swing));
                    self.world.sample_trail(handle, base, tip, step * (1.0 - share));
                }
                if tick as f32 >= SWING_TICKS {
                    self.world.stop(handle);
                    self.blades[0] = None;
                }
            }
            VfxShow::Duel => {
                for (index, blade) in [Blade::ATTACKER, Blade::VICTIM].into_iter().enumerate() {
                    let Some(handle) = self.blades[index] else { continue };
                    // Back and forth, so neither trail ever rests or retires, and out of step.
                    let period = 2.0 * SWING_TICKS;
                    let clock = self.character_ticks as f32 + index as f32 * 3.5;
                    for sample in 1..=self.samples_per_tick {
                        let share = sample as f32 / self.samples_per_tick as f32;
                        let phase = (clock - 1.0 + share).rem_euclid(period) / SWING_TICKS;
                        let swing = if phase <= 1.0 { phase } else { 2.0 - phase };
                        let (base, tip) = blade.sockets(eased(swing));
                        self.world.sample_trail(handle, base, tip, step * (1.0 - share));
                    }
                }
            }
            VfxShow::Dust => {}
        }
    }

    /// A point within `reach` of `center`, by a spawn's turn: a low-discrepancy scatter, not a random
    /// stream, so the duel stays the same however the effects' seed changes.
    fn scattered(&mut self, center: Vec3, reach: Vec3) -> Vec3 {
        self.scatter += 1;
        let fraction = |multiplier: f32| (self.scatter as f32 * multiplier).fract() * 2.0 - 1.0;
        center + reach * Vec3::new(fraction(0.754_877_7), fraction(0.569_840_3), fraction(0.438_596_6))
    }

    /// Writes the frame's sprites and ribbon points to `frame`, `alpha` of the way from the step before
    /// the latest to the latest, with the time this frame's steps and the write itself took: what spec
    /// §3.2 budgets. Allocates nothing once `frame` is warm.
    pub(crate) fn write(&mut self, frame: &mut VfxFrame, alpha: f32) {
        let stopwatch = Stopwatch::start();
        self.world.write_frame(frame, alpha);
        frame.stats.simulation = self.stepping + stopwatch.elapsed();
        self.stepping = Duration::ZERO;
    }
}

/// An effect at `at`, its +Y turned toward the character's side of the camera: sparks fly out of the
/// frame's plane rather than along it.
fn facing_camera(at: Vec3) -> Placement {
    let rotation = Quat::from_rotation_arc(Vec3::Y, Vec3::new(0.2, 0.55, 0.8).normalize());
    Placement { position: at, rotation }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A simulation tick, seconds.
    const TICK: f32 = 1.0 / 60.0;

    impl VfxPlayer {
        /// One tick of both clocks.
        fn tick(&mut self) -> Option<u16> {
            self.step(TICK, TICK)
        }
    }

    fn frame_after(show: VfxShow, budget: VfxBudget, ticks: u32) -> (VfxPlayer, VfxFrame) {
        let mut player = VfxPlayer::new(show, budget);
        let mut frame = VfxFrame::default();
        for _ in 0..ticks {
            player.tick();
            player.write(&mut frame, 1.0);
        }
        (player, frame)
    }

    #[test]
    fn names_round_trip() {
        for show in VfxShow::ALL {
            assert_eq!(VfxShow::from_name(show.name()), Some(show));
        }
        assert_eq!(VfxShow::from_name("nothing"), None);
    }

    #[test]
    fn an_arc_draws_mid_swing_and_ends() {
        let budget = VfxBudget::DESKTOP_HIGH;
        let (_, frame) = frame_after(VfxShow::Arc, budget, 6);
        assert_eq!(frame.stats.ribbons, 1);
        assert!(frame.ribbon_points.len() > 40, "{}", frame.ribbon_points.len());
        let (_, frame) = frame_after(VfxShow::Arc, budget, 40);
        assert!(frame.is_empty(), "the trail fades before the next swing");
        let (_, frame) = frame_after(VfxShow::Arc, budget, 66);
        assert_eq!(frame.stats.ribbons, 1, "and swings again");
    }

    #[test]
    fn a_hit_starts_a_hitstop_with_its_spark() {
        let mut player = VfxPlayer::new(VfxShow::Hit, VfxBudget::DESKTOP_HIGH);
        let mut frame = VfxFrame::default();
        let mut stopped = None;
        for tick in 0..20 {
            if let Some(ticks) = player.tick() {
                stopped = Some((tick, ticks));
                break;
            }
        }
        assert_eq!(stopped, Some((HIT_AT, HIT_STOP_TICKS)));
        player.write(&mut frame, 1.0);
        assert!(frame.sprites.len() > 26, "the spark, and the dust: {}", frame.sprites.len());
        // On the character's clock the blade and the spark hold; the world's dust moves on.
        let trail = frame.ribbon_points.clone();
        let dust = |frame: &VfxFrame| frame.sprites.iter().filter(|s| s.position.y < 0.5).count();
        let sparks: Vec<_> = frame.sprites.iter().filter(|s| s.position.y >= 0.5).copied().collect();
        assert!(dust(&frame) > 0);
        for _ in 0..HIT_STOP_TICKS {
            assert_eq!(player.step(TICK, 0.0), None);
        }
        player.write(&mut frame, 1.0);
        assert_eq!(frame.ribbon_points, trail, "the blade's trail");
        let held: Vec<_> = frame.sprites.iter().filter(|s| s.position.y >= 0.5).copied().collect();
        assert_eq!(held, sparks, "the spark");
    }

    /// The bench's load is what it says, on every tier, with nothing dropped.
    #[test]
    fn the_duel_holds_its_load_on_every_tier() {
        for budget in
            [VfxBudget::DESKTOP_HIGH, VfxBudget::DESKTOP_LOW, VfxBudget::MOBILE_HIGH, VfxBudget::MOBILE_LOW]
        {
            let mut player = VfxPlayer::new(VfxShow::Duel, budget);
            let mut frame = VfxFrame::default();
            let (mut particles, mut frames) = (0_u32, 0_u32);
            for tick in 0..300 {
                player.tick();
                player.write(&mut frame, 1.0);
                if tick >= 120 {
                    particles += frame.stats.particles;
                    frames += 1;
                }
            }
            let mean = particles / frames;
            assert!((220..=320).contains(&mean), "{budget:?}: {mean} particles on average");
            assert_eq!(frame.stats.ribbons, 2, "{budget:?}");
            assert_eq!(frame.stats.dropped_particles, 0, "{budget:?}");
            assert_eq!(frame.stats.dropped_ribbon_points, 0, "{budget:?}");
            assert_eq!(frame.stats.dropped_effects, 0, "{budget:?}");
        }
    }

    #[test]
    fn a_show_allocates_nothing_once_warmed() {
        for show in VfxShow::ALL {
            let (mut player, mut frame) = frame_after(show, VfxBudget::DESKTOP_LOW, 240);
            let ((), counts) = sapphire_core::alloc::count_in_scope(|| {
                for _ in 0..240 {
                    player.tick();
                    player.write(&mut frame, 1.0);
                }
            });
            assert_eq!(counts.allocations, 0, "{show:?}");
        }
    }
}
