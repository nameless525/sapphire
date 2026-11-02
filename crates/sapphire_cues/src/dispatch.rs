//! [`CueDispatcher`]: a table's names resolved, and each step's cues fired through it.

use sapphire_assets::AssetId;
use sapphire_audio::{CueRequest, CueTarget, Emitter, EmitterId};
use sapphire_camera::{CameraDirector, CameraTuning, ShakeId};
use sapphire_core::Name;
use sapphire_ecs::prelude::{EntityClock, WorldClock};
use sapphire_gameplay::{ActorId, ActorTable, ActorView, CueEvent, HitCue, Simulation};
use sapphire_math::{Mat4, Quat, Vec2, Vec3};
use sapphire_moves::Weight;
use sapphire_render::PostVariants;
use sapphire_vfx::{EffectId, EffectLibrary as VfxLibrary, OwnerId, OwnerSteps, Placement, VfxWorld};
use sapphire_world::screen_effects::{EffectLibrary as ScreenLibrary, EffectPresetId, EffectStack};

use crate::rumble::{Rumble, RumbleMixer, RumbleSink};
use crate::table::{
    CueRow, CueTable, CueTableError, EffectFire, EffectOwner, EffectPlace, MAX_ROW_EFFECTS, MAX_ROW_SOUNDS,
    ScreenCenter,
};

/// The most sounds a frame can start. [`sapphire_audio::MAX_VOICES`] is all that can play, and a frame
/// of two steps fills it only if every voice starts at once. Past it they're counted and dropped.
pub const MAX_FRAME_SOUNDS: usize = sapphire_audio::MAX_VOICES;

/// Whose clock an actor's effects run on.
#[must_use]
pub fn owner_of(actor: ActorId) -> OwnerId {
    OwnerId(u64::from(actor.raw()))
}

/// The emitter an actor's sounds follow. An ID never reads 0, so [`EmitterId`] 0 stays an app's own.
#[must_use]
pub fn emitter_of(actor: ActorId) -> EmitterId {
    EmitterId(actor.raw())
}

/// An actor as `sapphire_audio` hears it: what an app adds to each frame's
/// [`AudioFrame::emitters`](sapphire_audio::AudioFrame::emitters).
#[must_use]
pub fn actor_emitter(actor: &ActorView) -> Emitter {
    Emitter { id: emitter_of(actor.id), position: actor.position, hitstop: actor.hitstop_ticks > 0 }
}

/// What fires a camera shake: `sapphire_camera`'s director, or whatever else an app's view plays them
/// on.
pub trait ShakeSink {
    fn fire_shake(&mut self, id: ShakeId, strength: f32);
}

impl ShakeSink for CameraDirector {
    fn fire_shake(&mut self, id: ShakeId, strength: f32) {
        CameraDirector::fire_shake(self, id, strength);
    }
}

/// What a step's cues fire into. Everything here takes a cue in a step without allocating; what can't,
/// sound and SDL's rumble, waits for [`CueDispatcher::end_frame`].
pub struct CueStep<'a> {
    pub vfx: &'a mut VfxWorld,
    pub screen: &'a mut EffectStack,
    /// The presets [`CueDispatcher::new`] resolved.
    pub presets: &'a ScreenLibrary,
    pub shakes: &'a mut dyn ShakeSink,
    /// The view's world-to-clip, which puts a hit's point on screen.
    pub view_projection: Mat4,
    /// Where an actor stands. `None` for one that's gone, whose [`EffectPlace::Feet`] effects don't play.
    pub root_of: &'a dyn Fn(ActorId) -> Option<Vec3>,
}

/// What's been fired so far (rule R9). Nothing here reads a clock: whoever runs it times it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CueStats {
    /// Cues read.
    pub cues: u64,
    /// Cues with no row, which fire nothing.
    pub unmatched: u64,
    pub effects: u64,
    /// Effects the budget refused, or whose actor is gone.
    pub effects_dropped: u64,
    pub sounds: u64,
    /// Sounds past [`MAX_FRAME_SOUNDS`] in a frame.
    pub sounds_dropped: u64,
    pub shakes: u64,
    pub screens: u64,
    pub rumbles: u64,
}

#[derive(Clone, Copy, Debug)]
struct RowEffect {
    effect: EffectId,
    at: EffectPlace,
    clock: EffectOwner,
}

/// A table's row with its names resolved. Plain data, so firing touches no heap.
#[derive(Clone, Copy, Debug)]
struct Row {
    cue: Name,
    weight: Option<Weight>,
    effects: [Option<RowEffect>; MAX_ROW_EFFECTS],
    sounds: [Option<AssetId>; MAX_ROW_SOUNDS],
    shake: Option<(ShakeId, f32)>,
    screen: Option<(EffectPresetId, ScreenCenter)>,
    rumble: Option<Rumble>,
}

impl Row {
    fn resolve(
        row: &CueRow,
        effects: &VfxLibrary,
        presets: &ScreenLibrary,
        camera: &CameraTuning,
    ) -> Result<Self, CueTableError> {
        let mut resolved = Self {
            cue: Name::intern(&row.cue),
            weight: row.weight,
            effects: [None; MAX_ROW_EFFECTS],
            sounds: [None; MAX_ROW_SOUNDS],
            shake: None,
            screen: None,
            rumble: row.rumble,
        };
        for (slot, EffectFire { effect, at, clock }) in resolved.effects.iter_mut().zip(&row.effects) {
            let found = effects
                .find(effect)
                .ok_or_else(|| CueTableError::UnknownEffect { row: row.label(), name: effect.clone() })?;
            *slot = Some(RowEffect { effect: found, at: *at, clock: *clock });
        }
        for (slot, sound) in resolved.sounds.iter_mut().zip(&row.sounds) {
            *slot = Some(*sound);
        }
        if let Some(shake) = &row.shake {
            let id = camera.shake_id(&shake.preset).ok_or_else(|| CueTableError::UnknownShake {
                row: row.label(),
                name: shake.preset.clone(),
            })?;
            resolved.shake = Some((id, shake.strength));
        }
        if let Some(screen) = &row.screen {
            let id = presets.find(&screen.preset).ok_or_else(|| CueTableError::UnknownPreset {
                row: row.label(),
                name: screen.preset.clone(),
            })?;
            resolved.screen = Some((id, screen.center));
        }
        Ok(resolved)
    }
}

/// Fires a table's rows for the cues a simulation publishes. See the [crate docs](crate).
#[derive(Clone, Debug)]
pub struct CueDispatcher {
    rows: Vec<Row>,
    /// This frame's sounds. Reserved, and never grown past that.
    sounds: Vec<CueRequest>,
    mixer: RumbleMixer,
    /// The tick last read, so a step with nothing after it isn't read twice.
    read_tick: Option<u64>,
    /// Each actor's step, for [`Self::advance_effects`].
    owners: Vec<(OwnerId, f32)>,
    stats: CueStats,
}

impl CueDispatcher {
    /// `table`, with every name it holds resolved, or an error naming the first one that isn't there.
    pub fn new(
        table: &CueTable,
        effects: &VfxLibrary,
        presets: &ScreenLibrary,
        camera: &CameraTuning,
    ) -> Result<Self, CueTableError> {
        let mut dispatcher = Self {
            rows: Vec::new(),
            sounds: Vec::with_capacity(MAX_FRAME_SOUNDS),
            mixer: RumbleMixer::new(),
            read_tick: None,
            owners: Vec::new(),
            stats: CueStats::default(),
        };
        dispatcher.resolve(table, effects, presets, camera)?;
        Ok(dispatcher)
    }

    /// Takes a new table, or new libraries for this one: IDs hold for the libraries they came from, so
    /// a reload of either comes through here. On an error it keeps what it has, which holds only for
    /// the libraries it had.
    pub fn resolve(
        &mut self,
        table: &CueTable,
        effects: &VfxLibrary,
        presets: &ScreenLibrary,
        camera: &CameraTuning,
    ) -> Result<(), CueTableError> {
        let rows = table.rows().iter().map(|row| Row::resolve(row, effects, presets, camera));
        self.rows = rows.collect::<Result<_, _>>()?;
        Ok(())
    }

    /// The post variants the table's screen effects draw through: what a game prewarms at load
    /// ([`Renderer::prewarm_post`](sapphire_render::Renderer::prewarm_post)), so a hit's first effect
    /// shows on its frame rather than after a compile stall. Fewer than the whole library's, which
    /// holds presets nothing here pushes.
    #[must_use]
    pub fn post_variants(&self, presets: &ScreenLibrary) -> PostVariants {
        let pushed = self.rows.iter().filter_map(|row| row.screen).filter_map(|(id, _)| presets.preset(id));
        pushed.fold(PostVariants::NONE, |variants, preset| variants | preset.post_variants())
    }

    /// Fires one step's cues: call it after each `Simulation::step`, since the next step's start clears
    /// them, and before [`Self::advance_effects`], so what a hit starts shows on this tick's frame.
    /// Allocates nothing.
    ///
    /// `tick` is `Simulation::tick`: the same tick read again fires nothing, as when a replay has run out
    /// and its last step's cues are still there to read.
    pub fn read_step(&mut self, tick: u64, cues: impl IntoIterator<Item = CueEvent>, step: &mut CueStep<'_>) {
        profiling::scope!("CueDispatcher::read_step");
        if self.read_tick == Some(tick) {
            return;
        }
        self.read_tick = Some(tick);
        for event in cues {
            self.fire(&event, step);
        }
    }

    fn fire(&mut self, event: &CueEvent, step: &mut CueStep<'_>) {
        self.stats.cues += 1;
        let weight = event.hit.map(|hit| hit.weight);
        let Some(row) = find_row(&self.rows, event.cue, weight).copied() else {
            self.stats.unmatched += 1;
            return;
        };
        let subject = event.hit.map_or(event.actor, |hit| hit.target);
        for effect in row.effects.iter().flatten() {
            self.fire_effect(effect, event, subject, step);
        }
        for sound in row.sounds.iter().flatten() {
            self.queue_sound(*sound, event);
        }
        if let Some((id, strength)) = row.shake {
            step.shakes.fire_shake(id, strength);
            self.stats.shakes += 1;
        }
        if let Some((id, center)) = row.screen
            && let Some(preset) = step.presets.preset(id)
        {
            match center_of(center, event.hit, step.view_projection) {
                Some(center) => step.screen.push_at(preset, center),
                None => step.screen.push(preset),
            };
            self.stats.screens += 1;
        }
        if let Some(rumble) = row.rumble {
            self.mixer.start(rumble);
            self.stats.rumbles += 1;
        }
    }

    fn fire_effect(
        &mut self,
        effect: &RowEffect,
        event: &CueEvent,
        subject: ActorId,
        step: &mut CueStep<'_>,
    ) {
        let root = (step.root_of)(subject);
        let placement = match (effect.at, event.hit) {
            (EffectPlace::Hit, Some(hit)) => Some(along_normal(&hit)),
            _ => root.map(Placement::at),
        };
        let owner = match effect.clock {
            EffectOwner::World => OwnerId::WORLD,
            EffectOwner::Subject => owner_of(subject),
            EffectOwner::Actor => owner_of(event.actor),
        };
        let spawned = placement.and_then(|placement| step.vfx.spawn(effect.effect, owner, placement));
        match spawned {
            Some(_) => self.stats.effects += 1,
            None => self.stats.effects_dropped += 1,
        }
    }

    fn queue_sound(&mut self, cue: AssetId, event: &CueEvent) {
        if self.sounds.len() == MAX_FRAME_SOUNDS {
            self.stats.sounds_dropped += 1;
            return;
        }
        let target = match event.hit {
            Some(hit) => CueTarget::At(hit.point),
            None => CueTarget::On(emitter_of(event.actor)),
        };
        self.sounds.push(CueRequest { cue, target });
        self.stats.sounds += 1;
    }

    /// Moves `vfx` by the step `sim` just took: the world's effects by the world clock, and each
    /// actor's by its own, so its hitstop holds them. Allocates nothing once it has seen every actor.
    pub fn advance_effects(&mut self, sim: &Simulation, vfx: &mut VfxWorld) {
        let world = sim.world();
        self.owners.clear();
        for (id, entity) in world.resource::<ActorTable>().iter() {
            if let Some(clock) = world.get::<EntityClock>(entity) {
                self.owners.push((owner_of(id), clock.0.last_delta().as_secs_f32()));
            }
        }
        let world_step = world.resource::<WorldClock>().0.last_delta().as_secs_f32();
        vfx.advance(&OwnerSteps { world: world_step, owners: &self.owners });
    }

    /// The sounds this frame's steps asked for: [`AudioFrame::cues`](sapphire_audio::AudioFrame::cues).
    #[must_use]
    pub fn sounds(&self) -> &[CueRequest] {
        &self.sounds
    }

    /// Once a frame, after the frame's `AudioSystem::update` took [`Self::sounds`], `dt` real seconds
    /// after the last: forgets the sounds, so a capture or a bench with no audio just never hears them,
    /// and tells `rumble` what changed. `()` is no rumble.
    pub fn end_frame(&mut self, dt: f32, rumble: &mut dyn RumbleSink) {
        self.sounds.clear();
        self.mixer.update(dt, rumble);
    }

    /// Stops any rumble at once: the app lost focus or the foreground.
    pub fn stop_rumble(&mut self, rumble: &mut dyn RumbleSink) {
        self.mixer.stop(rumble);
    }

    /// The player's rumble setting, 0 (none, for accessibility) to 1 (as authored).
    pub fn set_rumble_setting(&mut self, setting: f32) {
        self.mixer.set_setting(setting);
    }

    #[must_use]
    pub fn stats(&self) -> CueStats {
        self.stats
    }
}

/// A row with the weight wins over a row with none. Rows are few, and a frame's cues fewer.
fn find_row(rows: &[Row], cue: Name, weight: Option<Weight>) -> Option<&Row> {
    let named = || rows.iter().filter(|row| row.cue == cue);
    weight
        .and_then(|weight| named().find(|row| row.weight == Some(weight)))
        .or_else(|| named().find(|row| row.weight.is_none()))
}

/// A hit's effect, at its point and with +Y out of the surface. A normal with no direction leaves it upright.
fn along_normal(hit: &HitCue) -> Placement {
    let normal = hit.normal.normalize_or_zero();
    let rotation =
        if normal == Vec3::ZERO { Quat::IDENTITY } else { Quat::from_rotation_arc(Vec3::Y, normal) };
    Placement { position: hit.point, rotation }
}

/// Where a screen effect centers, in texture coordinates, or `None` for the preset's own.
fn center_of(center: ScreenCenter, hit: Option<HitCue>, view_projection: Mat4) -> Option<Vec2> {
    match center {
        ScreenCenter::Preset => None,
        ScreenCenter::Hit => screen_point(view_projection, hit?.point),
    }
}

/// `point` on screen, in texture coordinates, held to the frame. `None` behind the camera.
fn screen_point(view_projection: Mat4, point: Vec3) -> Option<Vec2> {
    let clip = view_projection * point.extend(1.0);
    if clip.w <= 0.0 {
        return None;
    }
    let (x, y) = (clip.x / clip.w, clip.y / clip.w);
    Some(Vec2::new(x * 0.5 + 0.5, 0.5 - y * 0.5).clamp(Vec2::ZERO, Vec2::ONE))
}

#[cfg(test)]
mod tests {
    use sapphire_gameplay::HitOutcome;
    use sapphire_look::VfxFrame;
    use sapphire_math::Vec4;
    use sapphire_time::{ScaledClock, SimDuration, SimTime};
    use sapphire_vfx::VfxBudget;

    use super::*;
    use crate::table::{ScreenFire, ShakeFire};

    const ATTACKER: ActorId = ActorId::from_raw(1).expect("not zero");
    const TARGET: ActorId = ActorId::from_raw(2).expect("not zero");
    const SOUND: AssetId = AssetId::from_text("11111111-2222-4333-8444-555555555555");
    const TICK: f32 = 1.0 / 60.0;

    /// Writes down what it's told.
    #[derive(Default)]
    struct Shakes(Vec<(ShakeId, f32)>);

    impl ShakeSink for Shakes {
        fn fire_shake(&mut self, id: ShakeId, strength: f32) {
            self.0.push((id, strength));
        }
    }

    /// Everything a step fires into.
    struct Sinks {
        vfx: VfxWorld,
        screen: EffectStack,
        presets: ScreenLibrary,
        shakes: Shakes,
        _world: ScaledClock,
    }

    impl Sinks {
        fn new() -> Self {
            let mut world = ScaledClock::new();
            Self {
                vfx: VfxWorld::new(VfxLibrary::default_effects(), VfxBudget::DESKTOP_LOW, 7),
                screen: EffectStack::new(world.new_local_clock()),
                presets: ScreenLibrary::default_effects(),
                shakes: Shakes::default(),
                _world: world,
            }
        }

        fn read(&mut self, dispatcher: &mut CueDispatcher, tick: u64, cues: &[CueEvent]) {
            let root_of = |actor: ActorId| match actor {
                ATTACKER => Some(Vec3::new(0.0, 0.0, -1.0)),
                TARGET => Some(Vec3::new(0.0, 0.0, 1.0)),
                _ => None,
            };
            let mut step = CueStep {
                vfx: &mut self.vfx,
                screen: &mut self.screen,
                presets: &self.presets,
                shakes: &mut self.shakes,
                view_projection: Mat4::IDENTITY,
                root_of: &root_of,
            };
            dispatcher.read_step(tick, cues.iter().copied(), &mut step);
        }

        /// Sprites after a tick on the world's clock, and on `owner`'s.
        fn sprites_after(&mut self, owners: &[(OwnerId, f32)]) -> usize {
            self.vfx.advance(&OwnerSteps { world: TICK, owners });
            let mut frame = VfxFrame::default();
            self.vfx.write_frame(&mut frame, 1.0);
            frame.sprites.len()
        }
    }

    fn row(cue: &str) -> CueRow {
        CueRow {
            cue: cue.to_owned(),
            weight: None,
            effects: Vec::new(),
            sounds: Vec::new(),
            shake: None,
            screen: None,
            rumble: None,
        }
    }

    fn dispatcher(rows: Vec<CueRow>) -> CueDispatcher {
        let table = CueTable::new(rows).expect("valid");
        let (effects, presets) = (VfxLibrary::default_effects(), ScreenLibrary::default_effects());
        CueDispatcher::new(&table, &effects, &presets, &CameraTuning::default_tuning()).expect("resolves")
    }

    fn event(cue: &str, hit: Option<HitCue>) -> CueEvent {
        CueEvent {
            actor: ATTACKER,
            cue: Name::new(cue),
            tick: 0,
            local_time: SimTime::ZERO,
            move_time: SimDuration::ZERO,
            hit,
        }
    }

    fn hit(weight: Weight) -> HitCue {
        HitCue {
            target: TARGET,
            hurtbox: 0,
            point: Vec3::new(0.25, 0.5, 0.0),
            normal: Vec3::X,
            weight,
            damage: 1,
            poise: 1,
            hitstop: 3,
            outcome: HitOutcome::Flinched,
        }
    }

    fn with_sound(cue: &str) -> CueRow {
        CueRow { sounds: vec![SOUND], ..row(cue) }
    }

    fn with_effect(cue: &str, at: EffectPlace, clock: EffectOwner) -> CueRow {
        CueRow { effects: vec![EffectFire { effect: "dust_puff".to_owned(), at, clock }], ..row(cue) }
    }

    #[test]
    fn a_row_with_the_weight_wins_over_one_without() {
        let shaking = |weight, strength| CueRow {
            weight,
            shake: Some(ShakeFire { preset: "hit_light".to_owned(), strength }),
            ..row("hit")
        };
        let strength_of = |shakes: &Shakes| shakes.0.last().map(|shake| shake.1);
        let mut dispatcher = dispatcher(vec![shaking(None, 1.0), shaking(Some(Weight::Heavy), 2.0)]);
        let mut sinks = Sinks::new();
        sinks.read(&mut dispatcher, 1, &[event("hit", Some(hit(Weight::Heavy)))]);
        assert_eq!(strength_of(&sinks.shakes), Some(2.0), "the weight's row");
        sinks.read(&mut dispatcher, 2, &[event("hit", Some(hit(Weight::Light)))]);
        assert_eq!(strength_of(&sinks.shakes), Some(1.0), "the row with none");
        sinks.read(&mut dispatcher, 3, &[event("hit", None)]);
        assert_eq!(strength_of(&sinks.shakes), Some(1.0), "a cue with no hit has no weight");
    }

    #[test]
    fn a_weighted_row_alone_takes_only_its_weight() {
        let heavy = CueRow { weight: Some(Weight::Heavy), ..with_sound("hit") };
        let mut dispatcher = dispatcher(vec![heavy]);
        let mut sinks = Sinks::new();
        sinks.read(&mut dispatcher, 1, &[event("hit", Some(hit(Weight::Light)))]);
        assert!(dispatcher.sounds().is_empty());
        assert_eq!(dispatcher.stats().unmatched, 1);
    }

    #[test]
    fn a_cue_with_no_row_is_counted_and_fires_nothing() {
        let mut dispatcher = dispatcher(vec![with_sound("known")]);
        let mut sinks = Sinks::new();
        sinks.read(&mut dispatcher, 1, &[event("unknown", None)]);
        let stats = dispatcher.stats();
        assert_eq!((stats.cues, stats.unmatched), (1, 1));
        assert!(dispatcher.sounds().is_empty());
        assert_eq!(sinks.screen.len(), 0);
        assert!(sinks.shakes.0.is_empty());
    }

    /// A replay that has run out leaves its last step's cues there to read.
    #[test]
    fn the_same_tick_read_again_fires_nothing() {
        let mut dispatcher = dispatcher(vec![with_sound("swing")]);
        let mut sinks = Sinks::new();
        let cues = [event("swing", None)];
        sinks.read(&mut dispatcher, 5, &cues);
        sinks.read(&mut dispatcher, 5, &cues);
        assert_eq!(dispatcher.sounds().len(), 1);
        assert_eq!(dispatcher.stats().cues, 1);
        sinks.read(&mut dispatcher, 6, &cues);
        assert_eq!(dispatcher.sounds().len(), 2, "both steps' sounds wait for the frame");
    }

    /// A hit is heard where it lands. Anything else follows whoever made it, so hitstop can freeze it.
    #[test]
    fn sounds_play_at_a_hit_or_on_the_actor() {
        let mut dispatcher = dispatcher(vec![with_sound("hit"), with_sound("swing")]);
        let mut sinks = Sinks::new();
        sinks.read(&mut dispatcher, 1, &[event("hit", Some(hit(Weight::Light))), event("swing", None)]);
        let targets: Vec<_> = dispatcher.sounds().iter().map(|request| request.target).collect();
        assert_eq!(targets, [CueTarget::At(hit(Weight::Light).point), CueTarget::On(emitter_of(ATTACKER))]);
    }

    #[test]
    fn a_frame_keeps_its_sounds_until_it_ends_and_counts_what_doesnt_fit() {
        let mut dispatcher = dispatcher(vec![with_sound("swing")]);
        let mut sinks = Sinks::new();
        let cues = vec![event("swing", None); MAX_FRAME_SOUNDS + 3];
        sinks.read(&mut dispatcher, 1, &cues);
        assert_eq!(dispatcher.sounds().len(), MAX_FRAME_SOUNDS);
        let stats = dispatcher.stats();
        assert_eq!((stats.sounds, stats.sounds_dropped), (MAX_FRAME_SOUNDS as u64, 3));
        dispatcher.end_frame(TICK, &mut ());
        assert!(dispatcher.sounds().is_empty(), "with no audio, quietly gone");
    }

    #[test]
    fn effects_play_at_the_hit_or_at_the_subjects_feet() {
        let mut dispatcher = dispatcher(vec![
            with_effect("at_hit", EffectPlace::Hit, EffectOwner::World),
            with_effect("at_feet", EffectPlace::Feet, EffectOwner::World),
        ]);
        // Dust rises and drifts, but not far on its first tick.
        let middle_z = |frame: &VfxFrame| {
            let total: f32 = frame.sprites.iter().map(|sprite| sprite.position.z).sum();
            total / frame.sprites.len() as f32
        };
        let mut frame = VfxFrame::default();
        let mut place = |name: &str, hit| {
            let mut sinks = Sinks::new();
            dispatcher.read_tick = None;
            sinks.read(&mut dispatcher, 1, &[event(name, hit)]);
            sinks.vfx.advance(&OwnerSteps { world: TICK, owners: &[] });
            sinks.vfx.write_frame(&mut frame, 1.0);
            assert!(!frame.sprites.is_empty(), "{name}");
            middle_z(&frame)
        };
        let some_hit = Some(hit(Weight::Light));
        assert!(place("at_hit", some_hit).abs() < 0.3, "at the hit, z = 0");
        assert!((place("at_feet", some_hit) - 1.0).abs() < 0.3, "at the target, z = 1");
        assert!((place("at_feet", None) + 1.0).abs() < 0.3, "at the actor, z = -1");
        assert!((place("at_hit", None) + 1.0).abs() < 0.3, "with no hit, at the actor");
    }

    #[test]
    fn an_effect_with_nowhere_to_play_is_counted() {
        let mut dispatcher = dispatcher(vec![with_effect("gone", EffectPlace::Feet, EffectOwner::World)]);
        let mut sinks = Sinks::new();
        let stranger = ActorId::from_raw(9).expect("not zero");
        sinks.read(&mut dispatcher, 1, &[CueEvent { actor: stranger, ..event("gone", None) }]);
        assert_eq!(dispatcher.stats().effects_dropped, 1);
        assert_eq!(dispatcher.stats().effects, 0);
    }

    /// Hitstop holds what's on an actor's clock and leaves the world's alone.
    #[test]
    fn an_effect_runs_on_the_clock_its_row_names() {
        let frozen = |clock: EffectOwner, owner: ActorId| {
            let mut dispatcher = dispatcher(vec![with_effect("x", EffectPlace::Feet, clock)]);
            let mut sinks = Sinks::new();
            sinks.read(&mut dispatcher, 1, &[event("x", Some(hit(Weight::Light)))]);
            sinks.sprites_after(&[(owner_of(owner), 0.0)])
        };
        assert_eq!(frozen(EffectOwner::Subject, TARGET), 0, "the target's frozen: nothing's emitted");
        assert_eq!(frozen(EffectOwner::Actor, ATTACKER), 0, "the attacker's frozen");
        assert!(frozen(EffectOwner::Subject, ATTACKER) > 0, "not the target's own clock");
        assert!(frozen(EffectOwner::Actor, TARGET) > 0, "not the attacker's own clock");
        assert!(frozen(EffectOwner::World, TARGET) > 0, "nobody's");
    }

    /// A screen effect pushed on a hit centers on it, unless the row says otherwise.
    #[test]
    fn a_screen_effect_centers_on_the_hit() {
        // A preset with no attack, so it shows at once.
        let split = |center| CueRow {
            screen: Some(ScreenFire { preset: "chromatic_aberration".to_owned(), center }),
            ..row("hit")
        };
        let center_after = |row: CueRow, hit: Option<HitCue>| {
            let mut dispatcher = dispatcher(vec![row]);
            let mut sinks = Sinks::new();
            sinks.read(&mut dispatcher, 1, &[event("hit", hit)]);
            assert_eq!(sinks.screen.len(), 1);
            sinks.screen.resolve(0.0).chromatic_aberration.center
        };
        // Under an identity view, (0.25, 0.5) in clip space is at (0.625, 0.25) on screen.
        let some_hit = Some(hit(Weight::Light));
        assert_eq!(center_after(split(ScreenCenter::Hit), some_hit), Vec2::new(0.625, 0.25));
        assert_eq!(center_after(split(ScreenCenter::Preset), some_hit), Vec2::splat(0.5));
        assert_eq!(center_after(split(ScreenCenter::Hit), None), Vec2::splat(0.5), "no hit");
    }

    #[test]
    fn a_point_behind_the_camera_has_no_place_on_screen_and_an_edge_holds_it() {
        let turned = Mat4::from_cols(Vec4::X, Vec4::Y, Vec4::Z, Vec4::NEG_W);
        assert_eq!(screen_point(turned, Vec3::ZERO), None);
        assert_eq!(screen_point(Mat4::IDENTITY, Vec3::new(5.0, -5.0, 0.0)), Some(Vec2::new(1.0, 1.0)));
    }

    /// A table that names what isn't there fails at load, naming the row.
    #[test]
    fn an_unknown_name_fails_at_load_and_keeps_what_it_had() {
        let tuning = CameraTuning::default_tuning();
        let (effects, presets) = (VfxLibrary::default_effects(), ScreenLibrary::default_effects());
        let resolve = |row: CueRow| {
            let table = CueTable::new(vec![row]).expect("valid");
            CueDispatcher::new(&table, &effects, &presets, &tuning).expect_err("refused")
        };
        let effect = with_effect("a", EffectPlace::Hit, EffectOwner::World);
        let effect = CueRow {
            effects: vec![EffectFire { effect: "nope".to_owned(), ..effect.effects[0].clone() }],
            ..effect
        };
        assert!(matches!(resolve(effect), CueTableError::UnknownEffect { .. }));
        let screen = ScreenFire { preset: "nope".to_owned(), center: ScreenCenter::Hit };
        assert!(matches!(
            resolve(CueRow { screen: Some(screen), ..row("a") }),
            CueTableError::UnknownPreset { .. }
        ));
        let shake = ShakeFire { preset: "nope".to_owned(), strength: 1.0 };
        let error = resolve(CueRow { shake: Some(shake), ..row("a") });
        assert!(matches!(error, CueTableError::UnknownShake { .. }), "{error:?}");
        assert!(error.to_string().contains("nope") && error.to_string().contains("`a`"), "{error}");

        let mut kept = dispatcher(vec![with_sound("swing")]);
        let bad = CueTable::new(vec![CueRow {
            shake: Some(ShakeFire { preset: "nope".to_owned(), strength: 1.0 }),
            ..row("swing")
        }])
        .expect("valid");
        assert!(kept.resolve(&bad, &effects, &presets, &tuning).is_err());
        let mut sinks = Sinks::new();
        sinks.read(&mut kept, 1, &[event("swing", None)]);
        assert_eq!(kept.sounds().len(), 1, "what it had still fires");
    }

    /// The table's screen presets draw through these, and a preset nothing pushes adds nothing.
    #[test]
    fn post_variants_cover_what_the_table_pushes_and_no_more() {
        let presets = ScreenLibrary::default_effects();
        let pushing = |preset: &str| {
            dispatcher(vec![CueRow {
                screen: Some(ScreenFire { preset: preset.to_owned(), center: ScreenCenter::Hit }),
                ..row("hit")
            }])
            .post_variants(&presets)
        };
        let of = |name: &str| {
            presets.preset(presets.find(name).expect("a preset")).expect("found").post_variants()
        };
        assert_eq!(pushing("radial_blur"), of("radial_blur"));
        assert_eq!(pushing("duotone"), of("duotone"));
        assert_eq!(dispatcher(vec![row("hit")]).post_variants(&presets), PostVariants::NONE);
    }

    /// Every preset the shipped table pushes is covered, so nothing it fires compiles on the frame it
    /// shows.
    #[test]
    fn the_shipped_table_resolves_against_the_shipped_libraries() {
        let table = CueTable::default_table();
        let (effects, presets) = (VfxLibrary::default_effects(), ScreenLibrary::default_effects());
        let dispatcher = CueDispatcher::new(&table, &effects, &presets, &CameraTuning::default_tuning())
            .expect("every name it holds is there");
        let variants = dispatcher.post_variants(&presets);
        for row in table.rows() {
            if let Some(screen) = &row.screen {
                let preset = presets.preset(presets.find(&screen.preset).expect("found")).expect("found");
                assert_eq!(variants | preset.post_variants(), variants, "{}", screen.preset);
            }
        }
    }
}
