//! [`CueDispatcher`]: a table's names resolved, and each step's cues fired through it.

use sapphire_assets::AssetId;
use sapphire_audio::{CueRequest, CueTarget, Emitter, EmitterId, LineKey, LineRequest};
use sapphire_camera::{CameraDirector, CameraTuning, ShakeId};
use sapphire_core::Name;
use sapphire_ecs::prelude::{EntityClock, WorldClock};
use sapphire_gameplay::{ActorId, ActorTable, ActorView, CueEvent, HitCue, Simulation};
use sapphire_math::{Mat4, Quat, Vec2, Vec3};
use sapphire_moves::Weight;
use sapphire_render::PostVariants;
use sapphire_vfx::{Effect, EffectId, EffectLibrary as VfxLibrary, OwnerId, OwnerSteps, Placement, VfxWorld};
use sapphire_world::screen_effects::{
    EffectLibrary as ScreenLibrary, EffectPreset, EffectPresetId, EffectStack,
};

use crate::marks::{CROWN_CLEARANCE, Stand};
use crate::rumble::{Rumble, RumbleMixer, RumbleSink};
use crate::table::{
    CueRow, CueTable, CueTableError, EffectFire, EffectOwner, EffectPlace, MAX_ROW_EFFECTS, MAX_ROW_SOUNDS,
    ScreenCenter,
};

/// The most sounds a frame can start. [`sapphire_audio::MAX_VOICES`] is all that can play, and a frame
/// of two steps fills it only if every voice starts at once. Past it they're counted and dropped.
pub const MAX_FRAME_SOUNDS: usize = sapphire_audio::MAX_VOICES;

/// The most voice lines a frame can start: more at once is a crowd, not a voice. Past it they're
/// counted and dropped.
pub const MAX_FRAME_LINES: usize = 4;

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
    /// Where an actor stands ([`Stand::of`], for the simulation's). `None` for one that's gone, whose
    /// effects but a hit's don't play.
    pub stand_of: &'a dyn Fn(ActorId) -> Option<Stand>,
}

/// Who or where a cue happens.
#[derive(Clone, Copy)]
enum Site {
    Actor { actor: ActorId, hit: Option<HitCue> },
    Place(Vec3),
}

impl Site {
    fn hit(self) -> Option<HitCue> {
        match self {
            Self::Actor { hit, .. } => hit,
            Self::Place(_) => None,
        }
    }
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
    /// Voice lines asked for (M3 A1).
    pub lines: u64,
    /// Lines past [`MAX_FRAME_LINES`] in a frame.
    pub lines_dropped: u64,
    pub shakes: u64,
    pub screens: u64,
    /// Full-screen flashes held back (M4 A3): within a second of the last one shown.
    pub flashes_held: u64,
    pub rumbles: u64,
}

#[derive(Clone, Copy, Debug)]
struct RowEffect {
    effect: EffectId,
    at: EffectPlace,
    clock: EffectOwner,
}

/// A voice line a step asked for, by its key in the table, and who said it (M4 A2): what a caption shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SaidLine<'a> {
    /// The line's key, as the table's row names it.
    pub key: &'a str,
    /// The actor whose cue it was, or none for a cue at a place.
    pub actor: Option<ActorId>,
}

/// A line a row speaks: its key hashed for the voice route, and where its text is in the dispatcher's keys.
#[derive(Clone, Copy, Debug)]
struct RowLine {
    key: LineKey,
    text: u16,
}

/// A table's row with its names resolved. Plain data, so firing touches no heap.
#[derive(Clone, Copy, Debug)]
struct Row {
    cue: Name,
    weight: Option<Weight>,
    effects: [Option<RowEffect>; MAX_ROW_EFFECTS],
    sounds: [Option<AssetId>; MAX_ROW_SOUNDS],
    line: Option<RowLine>,
    shake: Option<(ShakeId, f32)>,
    screen: Option<(EffectPresetId, ScreenCenter)>,
    /// How long its screen effect changes the whole frame's light, if it's a full-screen flash (M4 A3).
    flash: Option<f32>,
    rumble: Option<Rumble>,
}

impl Row {
    fn resolve(
        row: &CueRow,
        effects: &VfxLibrary,
        presets: &ScreenLibrary,
        camera: &CameraTuning,
        line_keys: &mut Vec<Box<str>>,
    ) -> Result<Self, CueTableError> {
        let line = row.line.as_deref().map(|line| {
            let text = line_keys.iter().position(|key| **key == *line).unwrap_or_else(|| {
                line_keys.push(line.into());
                line_keys.len() - 1
            });
            RowLine { key: LineKey::of(line), text: text as u16 }
        });
        let mut resolved = Self {
            cue: Name::intern(&row.cue),
            weight: row.weight,
            effects: [None; MAX_ROW_EFFECTS],
            sounds: [None; MAX_ROW_SOUNDS],
            line,
            shake: None,
            screen: None,
            flash: None,
            rumble: row.rumble,
        };
        for (slot, EffectFire { effect, at, clock }) in resolved.effects.iter_mut().zip(&row.effects) {
            let found = effects
                .find(effect)
                .ok_or_else(|| CueTableError::UnknownEffect { row: row.label(), name: effect.clone() })?;
            // Firing keeps no handle, so nothing could stop one, and each would hold its place for good.
            if effects.effect(found).is_some_and(Effect::is_endless) {
                return Err(CueTableError::EndlessEffect { row: row.label(), name: effect.clone() });
            }
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
            resolved.flash = presets.preset(id).and_then(EffectPreset::flash_seconds);
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
    /// This frame's voice lines, the same.
    lines: Vec<LineRequest>,
    /// Who said each of them, and where its key is in `line_keys`.
    said: Vec<(Option<ActorId>, u16)>,
    /// The keys of the lines the rows speak, each once.
    line_keys: Vec<Box<str>>,
    mixer: RumbleMixer,
    /// The tick last read, so a step with nothing after it isn't read twice.
    read_tick: Option<u64>,
    /// Each actor's step, for [`Self::advance_effects`].
    owners: Vec<(OwnerId, f32)>,
    /// The full-screen flashes let through of late (M4 A3).
    flashes: FlashRate,
    /// Real seconds since it was made, from [`Self::end_frame`]'s.
    seconds: f64,
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
            lines: Vec::with_capacity(MAX_FRAME_LINES),
            said: Vec::with_capacity(MAX_FRAME_LINES),
            line_keys: Vec::new(),
            mixer: RumbleMixer::new(),
            read_tick: None,
            owners: Vec::new(),
            flashes: FlashRate::default(),
            seconds: 0.0,
            stats: CueStats::default(),
        };
        dispatcher.resolve(table, effects, presets, camera)?;
        Ok(dispatcher)
    }

    /// Reserves the simulation's complete actor pool before fixed stepping begins.
    pub fn reserve_actors(&mut self, capacity: usize) {
        self.owners.reserve(capacity.saturating_sub(self.owners.len()));
    }

    /// Begins another fight without discarding resolved rows or their allocated storage.
    /// Pending rumble is released at the next frame, where a pad is available. Lifetime
    /// statistics remain cumulative, so callers can count cues across restart boundaries. The flash rate
    /// starts over too: the restart clears the screen's effects, the last flash's with them.
    pub fn start_over(&mut self) {
        self.read_tick = None;
        self.flashes = FlashRate::default();
        self.owners.clear();
        self.sounds.clear();
        self.mixer.release();
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
        let mut line_keys = Vec::new();
        let rows = table.rows().iter().map(|row| Row::resolve(row, effects, presets, camera, &mut line_keys));
        self.rows = rows.collect::<Result<_, _>>()?;
        // What was said points into the old keys.
        self.said.clear();
        self.line_keys = line_keys;
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

    /// Fires a cue with no actor behind it, at `place`: what a timeline's cue track does (M3 Q1). Its row is
    /// looked up as a cue with no hit's would be, and its effects run on the world's clock. Allocates nothing.
    pub fn fire_at(&mut self, cue: Name, place: Vec3, step: &mut CueStep<'_>) {
        self.fire_on(cue, Site::Place(place), step);
    }

    /// Fires a cue on `actor` that the simulation didn't publish, as if it had with no hit: what
    /// [`CueMarks`](crate::CueMarks) finds (M4 A5). Allocates nothing.
    pub fn fire_on_actor(&mut self, cue: Name, actor: ActorId, step: &mut CueStep<'_>) {
        self.fire_on(cue, Site::Actor { actor, hit: None }, step);
    }

    fn fire(&mut self, event: &CueEvent, step: &mut CueStep<'_>) {
        self.fire_on(event.cue, Site::Actor { actor: event.actor, hit: event.hit }, step);
    }

    fn fire_on(&mut self, cue: Name, site: Site, step: &mut CueStep<'_>) {
        self.stats.cues += 1;
        let hit = site.hit();
        let weight = hit.map(|hit| hit.weight);
        let Some(row) = find_row(&self.rows, cue, weight).copied() else {
            self.stats.unmatched += 1;
            return;
        };
        for effect in row.effects.iter().flatten() {
            self.fire_effect(effect, site, step);
        }
        for sound in row.sounds.iter().flatten() {
            self.queue_sound(*sound, site);
        }
        if let Some(line) = row.line {
            let actor = match site {
                Site::Actor { actor, .. } => Some(actor),
                Site::Place(_) => None,
            };
            self.queue_line(line, actor);
        }
        if let Some((id, strength)) = row.shake {
            step.shakes.fire_shake(id, strength);
            self.stats.shakes += 1;
        }
        if let Some((id, center)) = row.screen
            && let Some(preset) = step.presets.preset(id)
        {
            // A full-screen flash within a second of another shows nothing, at every setting, unless it's the
            // bigger; the rest of its row still fires.
            let held = row.flash.is_some_and(|length| !self.flashes.admit(self.seconds, length));
            if held {
                self.stats.flashes_held += 1;
            } else {
                match center_of(center, hit, step.view_projection) {
                    Some(center) => step.screen.push_at(preset, center),
                    None => step.screen.push(preset),
                };
                self.stats.screens += 1;
            }
        }
        if let Some(rumble) = row.rumble {
            self.mixer.start(rumble);
            self.stats.rumbles += 1;
        }
    }

    fn fire_effect(&mut self, effect: &RowEffect, site: Site, step: &mut CueStep<'_>) {
        let (placement, owner) = match site {
            Site::Actor { actor, hit } => {
                let subject = hit.map_or(actor, |hit| hit.target);
                let stand = (step.stand_of)(subject);
                let placement = match (effect.at, hit) {
                    (EffectPlace::Hit, Some(hit)) => Some(along_normal(&hit)),
                    (EffectPlace::Crown, _) => {
                        stand.map(|stand| Placement::at(stand.crown + Vec3::Y * CROWN_CLEARANCE))
                    }
                    _ => stand.map(|stand| Placement::at(stand.feet)),
                };
                let owner = match effect.clock {
                    EffectOwner::World => OwnerId::WORLD,
                    EffectOwner::Subject => owner_of(subject),
                    EffectOwner::Actor => owner_of(actor),
                };
                (placement, owner)
            }
            Site::Place(place) => (Some(Placement::at(place)), OwnerId::WORLD),
        };
        let spawned = placement.and_then(|placement| step.vfx.spawn(effect.effect, owner, placement));
        match spawned {
            Some(_) => self.stats.effects += 1,
            None => self.stats.effects_dropped += 1,
        }
    }

    fn queue_sound(&mut self, cue: AssetId, site: Site) {
        if self.sounds.len() == MAX_FRAME_SOUNDS {
            self.stats.sounds_dropped += 1;
            return;
        }
        let target = match site {
            Site::Actor { hit: Some(hit), .. } => CueTarget::At(hit.point),
            Site::Actor { actor, hit: None } => CueTarget::On(emitter_of(actor)),
            Site::Place(place) => CueTarget::At(place),
        };
        self.sounds.push(CueRequest { cue, target });
        self.stats.sounds += 1;
    }

    /// A line is spoken flat, as the player's ear hears a voice: it names no emitter, so a fighter gone
    /// before the frame's audio still has it said.
    fn queue_line(&mut self, line: RowLine, actor: Option<ActorId>) {
        if self.lines.len() == MAX_FRAME_LINES {
            self.stats.lines_dropped += 1;
            return;
        }
        self.lines.push(LineRequest { line: line.key, target: CueTarget::Flat });
        self.said.push((actor, line.text));
        self.stats.lines += 1;
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

    /// The voice lines this frame's steps asked for: [`AudioFrame::lines`](sapphire_audio::AudioFrame::lines).
    #[must_use]
    pub fn lines(&self) -> &[LineRequest] {
        &self.lines
    }

    /// The same lines, in the same order, by their keys and who said them: what a caption shows (M4 A2),
    /// whether or not anything is heard. Allocates nothing.
    pub fn said(&self) -> impl ExactSizeIterator<Item = SaidLine<'_>> + '_ {
        self.said.iter().map(|&(actor, text)| SaidLine { key: &self.line_keys[usize::from(text)], actor })
    }

    /// Once a frame, after the frame's `AudioSystem::update` took [`Self::sounds`] and [`Self::lines`], `dt`
    /// real seconds after the last: forgets them, so a capture or a bench with no audio just never hears them,
    /// and tells `rumble` what changed. `()` is no rumble.
    pub fn end_frame(&mut self, dt: f32, rumble: &mut dyn RumbleSink) {
        self.seconds += f64::from(dt.max(0.0));
        self.sounds.clear();
        self.lines.clear();
        self.said.clear();
        self.mixer.update(dt, rumble);
    }

    /// Stops any rumble at once: the app lost focus or the foreground.
    pub fn stop_rumble(&mut self, rumble: &mut dyn RumbleSink) {
        self.mixer.stop(rumble);
    }

    /// Stops any rumble by the next [`Self::end_frame`], which has a pad to tell: a fight started over
    /// in a step, which doesn't.
    pub fn release_rumble(&mut self) {
        self.mixer.release();
    }

    /// Makes room for this many actors' effects to advance, so that the next ones' first steps allocate nothing.
    pub fn stand_by(&mut self, actors: usize) {
        self.owners.reserve(actors);
    }

    /// Forgets the sounds and lines this frame's steps asked for, before any of them started: a fight
    /// started over after they fired.
    pub fn forget_sounds(&mut self) {
        self.sounds.clear();
        self.lines.clear();
        self.said.clear();
    }

    /// The player's rumble setting, 0 (none, for accessibility) to 1 (as authored). What's playing takes it
    /// at the next [`Self::end_frame`], and 0 stops a pad that's running.
    pub fn set_rumble_setting(&mut self, setting: f32) {
        self.mixer.set_setting(setting);
    }

    #[must_use]
    pub fn stats(&self) -> CueStats {
        self.stats
    }
}

/// The last two full-screen flashes let through, oldest first, each by when it started and ended in real
/// seconds and how long it changed the frame's light (M4 A3). A flash is two transitions, its start and its end,
/// and WCAG 2.3.1 allows six in any second. One is near another when it starts within a second of the other's
/// start, or ends within a second of its end. A flash near none is let through; so is one longer than every
/// flash it's near, a guard break's after a heavy hit's or a finish's after both, so a fight's big moment is
/// never lost to a smaller one before it, and so is one starting while the last still shows, which blends into
/// it, a finish's on a perfect dodge's. Anything else near a flash is held: one flash a second, two
/// transitions, leaving four of the six to the fight itself, its camera, its fighters and their effects, and
/// more only as a rising run, which is short and rare. Measured on the stress fight, a camera passing a pillar takes four on its own; three flashes
/// a second, as WCAG would allow alone, failed it.
#[derive(Clone, Copy, Debug, Default)]
struct FlashRate {
    last: [Option<Flash>; 2],
}

/// A flash [`FlashRate`] let through.
#[derive(Clone, Copy, Debug)]
struct Flash {
    start: f64,
    end: f64,
    length: f32,
}

impl Flash {
    fn is_near(&self, start: f64, end: f64) -> bool {
        start - self.start < 1.0 || end - self.end < 1.0
    }
}

impl FlashRate {
    /// Whether a flash `length` seconds long may start at `now`, which it then does.
    fn admit(&mut self, now: f64, length: f32) -> bool {
        let end = now + f64::from(length);
        let near = |flash: &Option<Flash>| flash.is_some_and(|flash| flash.is_near(now, end));
        let longer = |flash: &Option<Flash>| !near(flash) || flash.is_some_and(|flash| length > flash.length);
        // One starting while the last still shows blends into it, one longer change on screen.
        let over = self.last[1].is_some_and(|last| now < last.end);
        let admitted = over || self.last.iter().all(longer);
        if admitted {
            self.last = [self.last[1], Some(Flash { start: now, end, length })];
        }
        admitted
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
    use sapphire_vfx::{EmitterDesc, Spawn, VfxBudget};

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
            // The attacker stands two metres tall, the target one.
            let stand_of = |actor: ActorId| match actor {
                ATTACKER => Some(Stand { feet: Vec3::new(0.0, 0.0, -1.0), crown: Vec3::new(0.0, 2.0, -1.0) }),
                TARGET => Some(Stand { feet: Vec3::new(0.0, 0.0, 1.0), crown: Vec3::new(0.0, 1.0, 1.0) }),
                _ => None,
            };
            let mut step = CueStep {
                vfx: &mut self.vfx,
                screen: &mut self.screen,
                presets: &self.presets,
                shakes: &mut self.shakes,
                view_projection: Mat4::IDENTITY,
                stand_of: &stand_of,
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
            line: None,
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

    /// A timeline's cue has no one behind it: its effect stands at the place, on the world's clock whatever
    /// the row says, its sound is at the place, and an unknown name fires nothing, as any other does.
    #[test]
    fn a_placed_cue_needs_no_actor() {
        let place = Vec3::new(4.0, 2.0, -1.0);
        let row =
            CueRow { sounds: vec![SOUND], ..with_effect("bell", EffectPlace::Feet, EffectOwner::Actor) };
        let mut dispatcher = dispatcher(vec![row]);
        let mut sinks = Sinks::new();
        let stand_of = |_| panic!("a placed cue asks for no actor");
        let mut step = CueStep {
            vfx: &mut sinks.vfx,
            screen: &mut sinks.screen,
            presets: &sinks.presets,
            shakes: &mut sinks.shakes,
            view_projection: Mat4::IDENTITY,
            stand_of: &stand_of,
        };
        dispatcher.fire_at(Name::new("bell"), place, &mut step);
        dispatcher.fire_at(Name::new("nothing"), place, &mut step);
        assert_eq!(dispatcher.sounds(), &[CueRequest { cue: SOUND, target: CueTarget::At(place) }]);
        assert_eq!(
            (dispatcher.stats().cues, dispatcher.stats().unmatched, dispatcher.stats().effects),
            (2, 1, 1)
        );
        // An owner in hitstop holds nothing of it.
        assert!(sinks.sprites_after(&[(owner_of(ATTACKER), 0.0)]) > 0);
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
    fn a_rows_line_is_asked_for_by_its_key_and_a_crowd_of_them_is_counted() {
        let mut dispatcher =
            dispatcher(vec![CueRow { line: Some("perfect-dodge".to_owned()), ..row("dodge") }]);
        let mut sinks = Sinks::new();
        sinks.read(&mut dispatcher, 1, &vec![event("dodge", None); MAX_FRAME_LINES + 2]);
        assert_eq!(dispatcher.lines().len(), MAX_FRAME_LINES);
        let said = LineRequest { line: LineKey::of("perfect-dodge"), target: CueTarget::Flat };
        assert!(dispatcher.lines().iter().all(|line| *line == said));
        let stats = dispatcher.stats();
        assert_eq!((stats.lines, stats.lines_dropped), (MAX_FRAME_LINES as u64, 2));
        dispatcher.end_frame(TICK, &mut ());
        assert!(dispatcher.lines().is_empty());
    }

    /// What a caption reads (M4 A2): each line by its key, and who said it, as long as the frame's lines last.
    #[test]
    fn what_was_said_names_its_line_and_who_said_it_until_the_frame_ends() {
        let rows = vec![
            CueRow { line: Some("perfect-dodge".to_owned()), ..row("dodge") },
            CueRow { line: Some("swap-in".to_owned()), ..row("swap") },
            CueRow { line: Some("perfect-dodge".to_owned()), ..row("dodge-again") },
        ];
        let mut dispatcher = dispatcher(rows);
        let mut sinks = Sinks::new();
        sinks.read(
            &mut dispatcher,
            1,
            &[event("dodge", None), event("swap", None), event("dodge-again", None)],
        );
        let keys: Vec<_> = dispatcher.said().map(|line| line.key).collect();
        assert_eq!(keys, ["perfect-dodge", "swap-in", "perfect-dodge"]);
        assert!(dispatcher.said().all(|line| line.actor == Some(ATTACKER)));
        assert_eq!(dispatcher.said().len(), dispatcher.lines().len());
        dispatcher.end_frame(TICK, &mut ());
        assert_eq!(dispatcher.said().len(), 0);
    }

    /// A fight starting over takes back what it hadn't started, and goes on from there.
    #[test]
    fn forgetting_sounds_leaves_the_steps_after_it_theirs() {
        let mut dispatcher = dispatcher(vec![with_sound("swing")]);
        let mut sinks = Sinks::new();
        sinks.read(&mut dispatcher, 1, &[event("swing", None)]);
        dispatcher.forget_sounds();
        assert!(dispatcher.sounds().is_empty());
        sinks.read(&mut dispatcher, 2, &[event("swing", None)]);
        assert_eq!(dispatcher.sounds().len(), 1);
    }

    #[test]
    fn effects_play_at_the_hit_or_at_the_subjects_feet_or_over_its_head() {
        let mut dispatcher = dispatcher(vec![
            with_effect("at_hit", EffectPlace::Hit, EffectOwner::World),
            with_effect("at_feet", EffectPlace::Feet, EffectOwner::World),
            with_effect("over", EffectPlace::Crown, EffectOwner::World),
        ]);
        // Dust rises and drifts, but not far on its first tick.
        let middle = |frame: &VfxFrame| {
            let total: Vec3 = frame.sprites.iter().map(|sprite| sprite.position).sum();
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
            middle(&frame)
        };
        let some_hit = Some(hit(Weight::Light));
        assert!(place("at_hit", some_hit).z.abs() < 0.3, "at the hit, z = 0");
        assert!((place("at_feet", some_hit).z - 1.0).abs() < 0.3, "at the target, z = 1");
        assert!((place("at_feet", None).z + 1.0).abs() < 0.3, "at the actor, z = -1");
        assert!((place("at_hit", None).z + 1.0).abs() < 0.3, "with no hit, at the actor");
        // Over the subject's crown: the target's one metre, the actor's two.
        let over = Vec3::new(0.0, 1.0 + CROWN_CLEARANCE, 1.0);
        assert!(place("over", some_hit).distance(over) < 0.3, "over the target");
        assert!(place("over", None).distance(over + Vec3::new(0.0, 1.0, -2.0)) < 0.3, "over the actor");
    }

    /// A cue the simulation didn't publish fires on its actor as one with no hit would (M4 A5).
    #[test]
    fn a_cue_fired_on_an_actor_plays_its_row_there() {
        let mut dispatcher =
            dispatcher(vec![with_effect("dodge_window", EffectPlace::Crown, EffectOwner::World)]);
        let mut sinks = Sinks::new();
        let stand_of = |_| Some(Stand { feet: Vec3::ZERO, crown: Vec3::Y });
        let mut step = CueStep {
            vfx: &mut sinks.vfx,
            screen: &mut sinks.screen,
            presets: &sinks.presets,
            shakes: &mut sinks.shakes,
            view_projection: Mat4::IDENTITY,
            stand_of: &stand_of,
        };
        dispatcher.fire_on_actor(Name::new("dodge_window"), ATTACKER, &mut step);
        dispatcher.fire_on_actor(Name::new("nothing"), ATTACKER, &mut step);
        let stats = dispatcher.stats();
        assert_eq!((stats.cues, stats.unmatched, stats.effects), (2, 1, 1));
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

    /// A library of the shipped dust's emitter, spawning each way named.
    fn effects_spawning(effects: &[(&str, &[Spawn])]) -> VfxLibrary {
        let shipped = VfxLibrary::default_effects();
        let dust = shipped.effect(shipped.find("dust_puff").expect("shipped")).expect("found");
        let effects = effects.iter().map(|(name, spawns)| Effect {
            name: (*name).to_owned(),
            emitters: spawns.iter().map(|&spawn| EmitterDesc { spawn, ..dust.emitters[0].clone() }).collect(),
            trail: None,
            mark: false,
            meshes: Vec::new(),
        });
        VfxLibrary::new(effects.collect()).expect("valid")
    }

    const ENDLESS: Spawn = Spawn::Rate { per_second: 1.0, duration: 0.0 };
    const ONE_SECOND: Spawn = Spawn::Rate { per_second: 1.0, duration: 1.0 };
    const ONCE: Spawn = Spawn::Burst { count: 1 };

    fn naming(effect: &str) -> CueRow {
        let fire = EffectFire { effect: effect.to_owned(), at: EffectPlace::Hit, clock: EffectOwner::World };
        CueRow { effects: vec![fire], ..row("hit") }
    }

    /// A cue keeps no handle, so nothing could ever stop an effect that plays until it's stopped: each
    /// one fired would hold its place in the budget for good.
    #[test]
    fn a_row_cant_fire_an_effect_that_plays_until_its_stopped() {
        let tuning = CameraTuning::default_tuning();
        let presets = ScreenLibrary::default_effects();
        let resolve = |effect: &str, library: &VfxLibrary| {
            let table = CueTable::new(vec![naming(effect)]).expect("valid");
            CueDispatcher::new(&table, library, &presets, &tuning).map(|_| ())
        };
        let refused =
            |name: &str| CueTableError::EndlessEffect { row: "hit".to_owned(), name: name.to_owned() };

        let shipped = VfxLibrary::default_effects();
        assert_eq!(resolve("sword_trail", &shipped), Err(refused("sword_trail")), "a trail");
        let error = resolve("sword_trail", &shipped).unwrap_err().to_string();
        assert!(error.contains("sword_trail") && error.contains("`hit`"), "{error}");

        let library = effects_spawning(&[
            ("endless", &[ENDLESS]),
            ("mixed", &[ONCE, ENDLESS]),
            ("finite", &[ONCE, ONE_SECOND]),
        ]);
        assert_eq!(resolve("endless", &library), Err(refused("endless")));
        assert_eq!(resolve("mixed", &library), Err(refused("mixed")), "one emitter is enough");
        assert_eq!(resolve("finite", &library), Ok(()));
    }

    /// A reload goes through the same check: a table naming one is refused, and so is a library that
    /// turns a name the table holds into one. The dispatcher keeps what it has.
    #[test]
    fn a_reload_cant_bring_in_an_effect_that_plays_until_its_stopped() {
        let tuning = CameraTuning::default_tuning();
        let presets = ScreenLibrary::default_effects();
        let finite = effects_spawning(&[("spark", &[ONCE]), ("endless", &[ENDLESS])]);
        let table = CueTable::new(vec![naming("spark")]).expect("valid");
        let mut dispatcher = CueDispatcher::new(&table, &finite, &presets, &tuning).expect("resolves");

        let bad_table = CueTable::new(vec![naming("endless")]).expect("valid");
        assert!(matches!(
            dispatcher.resolve(&bad_table, &finite, &presets, &tuning),
            Err(CueTableError::EndlessEffect { .. })
        ));
        let bad_library = effects_spawning(&[("spark", &[ENDLESS])]);
        assert!(matches!(
            dispatcher.resolve(&table, &bad_library, &presets, &tuning),
            Err(CueTableError::EndlessEffect { .. })
        ));

        let mut sinks = Sinks::new();
        sinks.vfx = VfxWorld::new(finite, VfxBudget::DESKTOP_LOW, 7);
        sinks.read(&mut dispatcher, 1, &[event("hit", Some(hit(Weight::Light)))]);
        assert_eq!(dispatcher.stats().effects, 1, "what it had still fires");
    }

    /// More one-shot cues than the budget holds: the ones that fit play out and give their places back.
    #[test]
    fn what_a_cue_fires_gives_its_place_back() {
        let mut dispatcher = dispatcher(vec![naming("dust_puff")]);
        let budget = VfxBudget::MOBILE_LOW;
        let mut sinks = Sinks::new();
        sinks.vfx = VfxWorld::new(VfxLibrary::default_effects(), budget, 7);
        let cues = vec![event("hit", Some(hit(Weight::Light))); budget.max_effects as usize + 12];
        sinks.read(&mut dispatcher, 1, &cues);
        let stats = dispatcher.stats();
        assert_eq!((stats.effects, stats.effects_dropped), (u64::from(budget.max_effects), 12));
        assert_eq!(sinks.vfx.stats().effects, budget.max_effects);

        for _ in 0..600 {
            sinks.vfx.advance(&OwnerSteps { world: TICK, owners: &[] });
        }
        assert_eq!(sinks.vfx.stats().effects, 0);
        assert_eq!(sinks.vfx.stats().particles, 0);

        sinks.read(&mut dispatcher, 2, &cues[..1]);
        assert_eq!(sinks.vfx.stats().effects, 1, "there's room again");
    }

    /// A full-screen flash comes at most once a second at every setting (M4 A3), its start and its end a
    /// second past the last one's, unless it's longer than that one, while what isn't a flash and the rest
    /// of a held flash's row fire as ever.
    #[test]
    fn a_full_screen_flash_a_second_comes_through() {
        let pushing = |cue: &str, preset: &str| CueRow {
            screen: Some(ScreenFire { preset: preset.to_owned(), center: ScreenCenter::Hit }),
            shake: Some(ShakeFire { preset: "hit_light".to_owned(), strength: 1.0 }),
            ..row(cue)
        };
        let rows =
            vec![pushing("heavy", "heavy_hit"), pushing("light", "light_hit"), pushing("finish", "finish")];
        // Fires `cue` at each frame of `frames`, 60 a second, from where the dispatcher's clock is, and says how
        // many shakes fired. Each firing is a step of its own: a dispatcher reads a step once.
        let mut ticks = 0;
        let mut fire = |dispatcher: &mut CueDispatcher, cue: &str, frames: &[u32]| {
            let mut sinks = Sinks::new();
            let mut frame = 0;
            for &at in frames {
                for _ in frame..at {
                    dispatcher.end_frame(TICK, &mut ());
                }
                frame = at;
                ticks += 1;
                sinks.read(dispatcher, ticks, &[event(cue, Some(hit(Weight::Heavy)))]);
            }
            sinks.shakes.0.len()
        };
        let mut apart = dispatcher(rows.clone());
        fire(&mut apart, "heavy", &[0, 61, 122]);
        assert_eq!((apart.stats().screens, apart.stats().flashes_held), (3, 0), "a second apart");

        let mut busy = dispatcher(rows.clone());
        // A heavy hit every sixth of a second: the next to show waits for a second after the first.
        let shaken = fire(&mut busy, "heavy", &[0, 10, 20, 30, 40, 50, 61, 71]);
        assert_eq!(shaken, 8, "each row's shake still fires");
        let stats = busy.stats();
        assert_eq!((stats.screens, stats.flashes_held), (2, 6), "frames 0 and 61");
        // A light hit's split isn't a flash.
        fire(&mut busy, "light", &[0, 1, 2, 3, 4, 5]);
        assert_eq!((busy.stats().screens, busy.stats().flashes_held), (8, 6));

        // A long flash's end counts too: a short one a second after a finish's start waits until it ends a second
        // after the finish's 0.7 s fall.
        let mut busy = dispatcher(rows.clone());
        fire(&mut busy, "finish", &[0]);
        fire(&mut busy, "heavy", &[61, 100]);
        let stats = busy.stats();
        assert_eq!((stats.screens, stats.flashes_held), (2, 1), "the one at 61 ends 0.37 s past the finish");

        // A finish after a heavy hit still shows, a longer flash than the one it's near, and one while it shows
        // blends into it; one after it ends, within a second of it, doesn't.
        let mut rising = dispatcher(rows);
        fire(&mut rising, "heavy", &[0]);
        fire(&mut rising, "finish", &[20]);
        fire(&mut rising, "heavy", &[10]);
        fire(&mut rising, "finish", &[35]);
        let stats = rising.stats();
        assert_eq!((stats.screens, stats.flashes_held), (3, 1), "all but the last finish");
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
