//! `--vfx <show>`: plays `sapphire_vfx`'s shipped effects at fixed ticks around the character (M2
//! E2), for golden images, benches and looking at them. Everything it plays is on a schedule of
//! simulation ticks and seeded, so a capture is the same frame in every run. A swing here is a circle's
//! arc in front of the character, passed to [`VfxWorld::sample_trail`] at sub-tick rate. A fight's trails
//! leave its real blades: `trails.rs`.
//!
//! The attacker's and the victim's effects run on the character's own clock, so its hitstop (the
//! `hit` show's, or a key press's) freezes them through it, and world effects keep running.
//!
//! The `mesh-*` shows play the shipped effect meshes (M5 L5): one effect each, started again each second, for
//! their goldens; `mesh-one` and `mesh-eight` keep one or eight of the skill's arcs always playing round the
//! character, the bench's load.
//!
//! The `decal-*` shows play the shipped ground decals (M5 L7): `decal-slope` a telegraph filling toward its
//! strike every second and a scorch every three, on the slope `--ik-ground slope` builds, for its golden;
//! `decal-one` keeps one telegraph always filling in front of the character and `decal-full` as many
//! telegraphs and scorches as the tier's cap round it, the bench's load.
//!
//! The `distort-*` shows play the shipped bends (M5 L6), the scorch's shockwave and haze: `distort-shockwave` a
//! heavy's landing behind the character every second, its shockwave running out over the paving past the
//! character's feet; `distort-haze` one landing at the start, the haze over its burn; `distort-one` one landing
//! always playing and `distort-full` as many as the tier's bends' cap, the bench's load. A show's bends draw
//! unless `--distortion off` says not (`Scene::set_vfx_distortion`); the decal shows' scorches bend too.

use std::time::Duration;

use sapphire_math::{Quat, Vec3};
use sapphire_render::VfxFrame;
use sapphire_render::quality::TierId;
use sapphire_time::Stopwatch;
use sapphire_vfx::{
    EffectHandle, EffectId, EffectLibrary, OwnerId, OwnerSteps, Placement, VfxBudget, VfxWorld, on_ground,
};

use crate::ik::IkGround;

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
    /// A shockwave and a pillar of light on the ground every second, their streaks scrolling (M5 L5).
    MeshScroll,
    /// A shell of light burning away by an erosion every second (M5 L5).
    MeshErosion,
    /// A burst's flipbook on a card every second (M5 L5).
    MeshFlipbook,
    /// One of the skill's arcs, always playing in front of the character (the meshes' bench).
    MeshOne,
    /// Eight of them, always playing all round it.
    MeshEight,
    /// A telegraph's sector filling toward its strike every second, and a scorch every three, on the slope
    /// `--ik-ground slope` builds (M5 L7).
    DecalSlope,
    /// One telegraph, always filling in front of the character (the decals' bench).
    DecalOne,
    /// As many telegraphs and scorches as the tier's cap, always playing round it.
    DecalFull,
    /// A heavy's landing behind the character every second: the scorch, and its shockwave bending the paving as
    /// it runs out past the character's feet (M5 L6).
    DistortShockwave,
    /// One landing at the start: the haze shimmering over its burn.
    DistortHaze,
    /// One landing, always playing in front of the character (the bends' bench).
    DistortOne,
    /// As many as the tier's bends' cap, always playing round it.
    DistortFull,
}

impl VfxShow {
    pub const ALL: [Self; 16] = [
        Self::Arc,
        Self::Hit,
        Self::Dust,
        Self::Duel,
        Self::MeshScroll,
        Self::MeshErosion,
        Self::MeshFlipbook,
        Self::MeshOne,
        Self::MeshEight,
        Self::DecalSlope,
        Self::DecalOne,
        Self::DecalFull,
        Self::DistortShockwave,
        Self::DistortHaze,
        Self::DistortOne,
        Self::DistortFull,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Self::Arc => "arc",
            Self::Hit => "hit",
            Self::Dust => "dust",
            Self::Duel => "duel",
            Self::MeshScroll => "mesh-scroll",
            Self::MeshErosion => "mesh-erosion",
            Self::MeshFlipbook => "mesh-flipbook",
            Self::MeshOne => "mesh-one",
            Self::MeshEight => "mesh-eight",
            Self::DecalSlope => "decal-slope",
            Self::DecalOne => "decal-one",
            Self::DecalFull => "decal-full",
            Self::DistortShockwave => "distort-shockwave",
            Self::DistortHaze => "distort-haze",
            Self::DistortOne => "distort-one",
            Self::DistortFull => "distort-full",
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
/// Where the flipbook's card stands, beside the character at the height of its blade's hits.
const CARD_SPOT: Vec3 = Vec3::new(-0.1, 1.05, 0.4);

/// How long a show's telegraph fills before its strike, seconds: the shipped one's wind-up length.
const TELEGRAPH_STRIKE: f32 = 0.75;
/// The most decals `decal-full` keeps playing: Desktop-High's cap.
const MAX_SHOWN_DECALS: usize = 24;
/// Where the `distort-*` shows' landings are placed: behind the character on the paving, so its shockwave runs
/// out toward the camera past the character's feet and the haze rises beside it. The scorch lands its own
/// reach ahead of this, turned by [`LANDING_TURN`] away from the camera, which stands on +Z looking along -Z:
/// a bend round the camera would cover the whole screen.
const LANDING_SPOT: Vec3 = Vec3::new(0.6, 0.0, -0.9);
/// How the `distort-*` shows' landings are turned: facing -Z, away from the camera.
const LANDING_TURN: f32 = std::f32::consts::PI;
/// How often the bends' bench lands each landing again, in ticks: as long as the shipped scorch bends, its haze
/// ending 1.75 s in.
const LANDING_PERIOD: u32 = 105;

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
    /// The effect a `mesh-*` show plays, if it's one.
    mesh: Option<EffectId>,
    /// The arcs `mesh-one` and `mesh-eight` keep playing.
    arcs: [Option<EffectHandle>; 8],
    /// The telegraph's and the scorch's decals, for a `decal-*` show.
    decals: Option<(EffectId, EffectId)>,
    /// The scorch, whose shockwave and haze bend, for a `distort-*` show (M5 L6).
    landing: Option<EffectId>,
    /// Whether its bends draw (`--distortion`): when not, a frame has none.
    distortion: bool,
    /// The decals `decal-one` and `decal-full` keep playing, and how many: one, or the tier's cap.
    shown: [Option<EffectHandle>; MAX_SHOWN_DECALS],
    shown_count: usize,
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
        let mesh = match show {
            VfxShow::MeshScroll => Some("shock_ring"),
            VfxShow::MeshErosion => Some("ember_dome"),
            VfxShow::MeshFlipbook => Some("burst_card"),
            VfxShow::MeshOne | VfxShow::MeshEight => Some("skill_arc"),
            VfxShow::Arc
            | VfxShow::Hit
            | VfxShow::Dust
            | VfxShow::Duel
            | VfxShow::DecalSlope
            | VfxShow::DecalOne
            | VfxShow::DecalFull
            | VfxShow::DistortShockwave
            | VfxShow::DistortHaze
            | VfxShow::DistortOne
            | VfxShow::DistortFull => None,
        }
        .map(effect);
        let landing = matches!(
            show,
            VfxShow::DistortShockwave | VfxShow::DistortHaze | VfxShow::DistortOne | VfxShow::DistortFull
        )
        .then(|| effect("scorch_mark"));
        let decals = matches!(show, VfxShow::DecalSlope | VfxShow::DecalOne | VfxShow::DecalFull)
            .then(|| (effect("telegraph_ground"), effect("scorch_mark")));
        let shown_count = match show {
            VfxShow::DecalOne | VfxShow::DistortOne => 1,
            VfxShow::DecalFull => (budget.max_decals as usize).min(MAX_SHOWN_DECALS),
            // Each landing bends twice at its start, its haze alone after.
            VfxShow::DistortFull => (budget.max_distortions as usize).min(MAX_SHOWN_DECALS),
            _ => 0,
        };
        Self {
            show,
            world: VfxWorld::new(library, budget, SEED),
            spark,
            sword,
            dust,
            mesh,
            arcs: [None; 8],
            decals,
            landing,
            distortion: true,
            shown: [None; MAX_SHOWN_DECALS],
            shown_count,
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

    /// Whether its show plays effect meshes: what the scene declares (M5 L5).
    pub(crate) fn has_meshes(&self) -> bool {
        self.mesh.is_some()
    }

    /// Whether its show plays ground decals: what the scene declares (M5 L7).
    pub(crate) fn has_decals(&self) -> bool {
        self.decals.is_some() || self.landing.is_some()
    }

    /// Whether its show's effects bend the picture and they draw: what the scene declares (M5 L6). The decal
    /// shows' scorches bend.
    pub(crate) fn has_distortion(&self) -> bool {
        self.distortion && (self.landing.is_some() || self.decals.is_some())
    }

    /// Whether its bends draw: off, they play unseen (they take no random numbers) and a frame has none.
    pub(crate) fn set_distortion(&mut self, on: bool) {
        self.distortion = on;
    }

    /// One fixed step, with how long each clock moved by: the character's stops in hitstop. Returns
    /// a hitstop to start on the character's clock, if a hit landed. Allocates nothing.
    ///
    /// The order is what a game's would be: what starts this tick spawns before the clocks move, so
    /// it shows on this tick's frame, and sockets are sampled after, back from the clock's new time.
    pub(crate) fn step(&mut self, world_step: f32, character_step: f32) -> Option<u16> {
        let stopwatch = Stopwatch::start();
        let hitstop = self.start_what_starts(world_step, character_step);
        let owners = [(ATTACKER, character_step), (VICTIM, character_step)];
        self.world.advance(&OwnerSteps { world: world_step, owners: &owners });
        // After the step, which ended what had played out: so the frame always holds them all.
        if matches!(self.show, VfxShow::MeshOne | VfxShow::MeshEight) && world_step > 0.0 {
            self.keep_arcs_playing();
        }
        if matches!(self.show, VfxShow::DecalOne | VfxShow::DecalFull) && world_step > 0.0 {
            self.keep_decals_playing();
        }
        if matches!(self.show, VfxShow::DistortOne | VfxShow::DistortFull) && world_step > 0.0 {
            self.keep_landings_playing();
        }
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
    fn start_what_starts(&mut self, world_step: f32, character_step: f32) -> Option<u16> {
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
            VfxShow::Dust if world_step > 0.0 && self.world_ticks.is_multiple_of(30) => {
                self.world.spawn(self.dust, OwnerId::WORLD, Placement::at(DUST_SPOT));
                None
            }
            VfxShow::Duel if world_step > 0.0 => {
                self.start_duels_moment();
                None
            }
            VfxShow::MeshScroll | VfxShow::MeshErosion | VfxShow::MeshFlipbook
                if world_step > 0.0 && self.world_ticks.is_multiple_of(SWING_PERIOD) =>
            {
                let at = if self.show == VfxShow::MeshFlipbook { CARD_SPOT } else { DUST_SPOT };
                if let Some(mesh) = self.mesh {
                    self.world.spawn(mesh, OwnerId::WORLD, Placement::at(at));
                }
                None
            }
            VfxShow::DecalSlope if world_step > 0.0 && self.world_ticks.is_multiple_of(SWING_PERIOD) => {
                self.lay_on_the_slope();
                None
            }
            VfxShow::DistortShockwave
                if world_step > 0.0 && self.world_ticks.is_multiple_of(SWING_PERIOD) =>
            {
                self.land(LANDING_SPOT, LANDING_TURN);
                None
            }
            VfxShow::DistortHaze if world_step > 0.0 && self.world_ticks == 0 => {
                self.land(LANDING_SPOT, LANDING_TURN);
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

    /// Starts again each of the bench's arcs that has ended, so one or eight always play: turned an eighth of a
    /// turn apart round the character. After the step that ended them, so no frame is short of one.
    fn keep_arcs_playing(&mut self) {
        let Some(arc) = self.mesh else { return };
        let count = if self.show == VfxShow::MeshEight { self.arcs.len() } else { 1 };
        for (index, slot) in self.arcs.iter_mut().take(count).enumerate() {
            if slot.is_none_or(|handle| !self.world.is_playing(handle)) {
                let rotation =
                    sapphire_math::det::quat_from_rotation_y(index as f32 * std::f32::consts::FRAC_PI_4);
                *slot = self.world.spawn(arc, OwnerId::WORLD, Placement { position: Vec3::ZERO, rotation });
            }
        }
    }

    /// A telegraph on the slope, from behind the character's feet out across it to the camera's right, the
    /// character standing in it; and every third second a scorch inside its reach, nearer the camera.
    fn lay_on_the_slope(&mut self) {
        let Some((telegraph, scorch)) = self.decals else { return };
        let (at, normal) = IkGround::slope_surface(-0.2, -0.3);
        let placement = on_ground(at, 60_f32.to_radians(), normal);
        if let Some(handle) = self.world.spawn(telegraph, OwnerId::WORLD, placement) {
            self.world.set_strike(handle, TELEGRAPH_STRIKE);
        }
        if self.world_ticks.is_multiple_of(3 * SWING_PERIOD) {
            // The scorch lands its own reach ahead of where it's placed, toward the camera.
            let (at, normal) = IkGround::slope_surface(1.4, -0.45);
            self.world.spawn(scorch, OwnerId::WORLD, on_ground(at, 0.0, normal));
        }
    }

    /// Starts again each of the bench's decals that has ended, so one or the tier's cap always play: a
    /// telegraph in front of the character, then telegraphs and scorches by turns on rings round it.
    fn keep_decals_playing(&mut self) {
        let Some((telegraph, scorch)) = self.decals else { return };
        for (index, slot) in self.shown.iter_mut().take(self.shown_count).enumerate() {
            if slot.is_some_and(|handle| self.world.is_playing(handle)) {
                continue;
            }
            let turn = index as f32 * std::f32::consts::TAU / 8.0;
            let reach = if index == 0 { 0.0 } else { 1.5 + (index / 8) as f32 * 2.5 };
            let at = Vec3::new(turn.sin() * reach, 0.0, turn.cos() * reach);
            let effect = if index.is_multiple_of(2) { telegraph } else { scorch };
            *slot = self.world.spawn(effect, OwnerId::WORLD, on_ground(at, turn, Vec3::Y));
            if let Some(handle) = *slot
                && effect == telegraph
            {
                self.world.set_strike(handle, TELEGRAPH_STRIKE);
            }
        }
    }

    /// A heavy's landing at `at` on flat ground, turned `turn` radians: its scorch, shockwave and haze.
    fn land(&mut self, at: Vec3, turn: f32) -> Option<EffectHandle> {
        let landing = self.landing?;
        self.world.spawn(landing, OwnerId::WORLD, on_ground(at, turn, Vec3::Y))
    }

    /// Lands each of the bench's landings again as its bends end, a landing every [`LANDING_PERIOD`] in turn, so
    /// each always bends: its shockwave until its haze rises, then its haze. One in front of the character, then
    /// on rings round it; a scorch still burning when its landing comes round again plays on beside the new one.
    fn keep_landings_playing(&mut self) {
        for index in 0..self.shown_count {
            let started = self.shown[index].is_some();
            let due = (self.world_ticks + index as u32 * 7).is_multiple_of(LANDING_PERIOD);
            if started && !due {
                continue;
            }
            let turn = index as f32 * std::f32::consts::TAU / 8.0;
            let reach = if index == 0 { 0.0 } else { 1.5 + (index / 8) as f32 * 2.5 };
            let at = Vec3::new(turn.sin() * reach, 0.0, turn.cos() * reach);
            self.shown[index] = self.land(at, turn);
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
            VfxShow::Dust
            | VfxShow::MeshScroll
            | VfxShow::MeshErosion
            | VfxShow::MeshFlipbook
            | VfxShow::MeshOne
            | VfxShow::MeshEight
            | VfxShow::DecalSlope
            | VfxShow::DecalOne
            | VfxShow::DecalFull
            | VfxShow::DistortShockwave
            | VfxShow::DistortHaze
            | VfxShow::DistortOne
            | VfxShow::DistortFull => {}
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
        if !self.distortion {
            frame.distortions.clear();
        }
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
    use sapphire_render::VfxBend;

    use super::*;

    #[test]
    fn paused_world_does_not_repeat_scheduled_births() {
        for show in [VfxShow::Dust, VfxShow::Duel] {
            let mut player = VfxPlayer::new(show, VfxBudget::DESKTOP_HIGH);
            let mut frame = VfxFrame::default();
            for _ in 0..4 {
                player.step(0.0, 0.0);
            }
            player.write(&mut frame, 1.0);
            assert!(frame.is_empty(), "paused activation at tick zero: {show:?}");
            let ticks = if show == VfxShow::Dust { 30 } else { 36 };
            for _ in 0..ticks {
                player.step(1.0 / 60.0, 1.0 / 60.0);
            }
            player.write(&mut frame, 1.0);
            assert!(frame.stats.particles > 0, "positive activity control: {show:?}");
            let before = frame.stats;
            for _ in 0..20 {
                player.step(0.0, 0.0);
            }
            player.write(&mut frame, 1.0);
            assert_eq!(frame.stats.particles, before.particles, "{show:?}");
            assert_eq!(frame.stats.dropped_particles, before.dropped_particles, "{show:?}");
        }
    }

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

    /// Each mesh show draws its meshes: the goldens' at their schedule, and the bench's one or eight always.
    #[test]
    fn the_mesh_shows_draw_their_meshes() {
        for (show, meshes) in [
            (VfxShow::MeshScroll, 2),
            (VfxShow::MeshErosion, 1),
            (VfxShow::MeshFlipbook, 1),
            (VfxShow::MeshOne, 1),
        ] {
            let (_, frame) = frame_after(show, VfxBudget::DESKTOP_HIGH, 16);
            assert_eq!(frame.meshes.len(), meshes, "{show:?}");
            assert!(frame.sprites.is_empty() && frame.ribbon_points.is_empty(), "{show:?}");
        }
        let mut player = VfxPlayer::new(VfxShow::MeshEight, VfxBudget::DESKTOP_HIGH);
        let mut frame = VfxFrame::default();
        for _ in 0..200 {
            player.tick();
            player.write(&mut frame, 1.0);
            assert_eq!(frame.meshes.len(), 8);
        }
        // Mobile-Low's budget is eight: all of them still draw.
        let (_, frame) = frame_after(VfxShow::MeshEight, VfxBudget::MOBILE_LOW, 30);
        assert_eq!((frame.meshes.len(), frame.stats.dropped_meshes), (8, 0));
    }

    /// Each bends' show bends: the shockwave's landing its shockwave, then its haze too; the haze's its haze
    /// alone once the shockwave is gone; the bench's one landing one or two always, and the tier's cap always on
    /// every tier with bends, Mobile-Low's none. `--distortion off` draws none and declares none.
    #[test]
    fn the_distort_shows_bend() {
        let budget = VfxBudget::DESKTOP_HIGH;
        let bends = |frame: &VfxFrame| frame.distortions.iter().map(|bend| bend.bend).collect::<Vec<_>>();
        let (player, frame) = frame_after(VfxShow::DistortShockwave, budget, 4);
        assert_eq!(bends(&frame), [VfxBend::Outward]);
        assert!(player.has_distortion() && player.has_decals());
        let (_, frame) = frame_after(VfxShow::DistortShockwave, budget, 16);
        assert_eq!(bends(&frame), [VfxBend::Outward, VfxBend::Shimmer]);
        let (_, frame) = frame_after(VfxShow::DistortHaze, budget, 60);
        assert_eq!(bends(&frame), [VfxBend::Shimmer]);
        assert!(frame.meshes.is_empty() && frame.sprites.is_empty(), "the scorch bends and darkens alone");
        assert_eq!(frame.decals.len(), 1, "its scorch");

        let (mut player, mut frame) = frame_after(VfxShow::DistortHaze, budget, 60);
        player.set_distortion(false);
        player.write(&mut frame, 1.0);
        assert!(frame.distortions.is_empty() && !player.has_distortion());
        assert_eq!(frame.decals.len(), 1, "its scorch still darkens");

        for budget in
            [VfxBudget::DESKTOP_HIGH, VfxBudget::DESKTOP_LOW, VfxBudget::MOBILE_HIGH, VfxBudget::MOBILE_LOW]
        {
            let cap = budget.max_distortions as usize;
            for (show, least, most) in
                [(VfxShow::DistortOne, 1.min(cap), 2.min(cap)), (VfxShow::DistortFull, cap, cap)]
            {
                let mut player = VfxPlayer::new(show, budget);
                let mut frame = VfxFrame::default();
                for tick in 0..400 {
                    player.tick();
                    player.write(&mut frame, 1.0);
                    let count = frame.distortions.len();
                    assert!((least..=most).contains(&count), "{show:?} {budget:?} at {tick}: {count}");
                }
            }
        }
    }

    /// Each decal show draws its decals, and nothing else: the slope's telegraph filling and its scorch, and the
    /// bench's one or the tier's cap always, with none dropped.
    #[test]
    fn the_decal_shows_draw_their_decals() {
        let (_, frame) = frame_after(VfxShow::DecalSlope, VfxBudget::DESKTOP_HIGH, 31);
        assert_eq!(frame.decals.len(), 2);
        assert!(frame.sprites.is_empty() && frame.meshes.is_empty() && frame.ribbon_points.is_empty());
        let telegraph = frame.decals.iter().find(|decal| decal.fill < 1.0).expect("the telegraph fills");
        assert!((0.3..0.8).contains(&telegraph.fill), "{}", telegraph.fill);
        for budget in
            [VfxBudget::DESKTOP_HIGH, VfxBudget::DESKTOP_LOW, VfxBudget::MOBILE_HIGH, VfxBudget::MOBILE_LOW]
        {
            for (show, count) in [(VfxShow::DecalOne, 1), (VfxShow::DecalFull, budget.max_decals as usize)] {
                let mut player = VfxPlayer::new(show, budget);
                let mut frame = VfxFrame::default();
                for _ in 0..400 {
                    player.tick();
                    player.write(&mut frame, 1.0);
                    assert_eq!(frame.decals.len(), count, "{show:?} {budget:?}");
                }
                assert_eq!(frame.stats.dropped_decals, 0, "{show:?} {budget:?}");
            }
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
