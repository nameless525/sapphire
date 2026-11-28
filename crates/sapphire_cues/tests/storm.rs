//! M6 U3's done-when through the cue table (fixed after its review, whose proof had called the audio system
//! directly): 199 footsteps' cues and then a boss's roar fired in one frame reach the mixer's ranking whole, so the
//! roar is mixed over the 63 footsteps that share the real voices with it, and the frame allocates nothing once
//! warmed. One test, since the counts are of this thread and a harness thread of its own keeps them honest.

use sapphire_assets::AssetId;
use sapphire_assets::cue::{Bus, Cue, CueNode};
use sapphire_assets::sound::DecodedSound;
use sapphire_audio::{
    AudioBackend, AudioFrame, AudioSystem, CueRng, KiraBackend, Listener, MAX_VOICES, MockBackend, MockCall,
    OfflineOutput,
};
use sapphire_camera::{CameraDirector, CameraTuning};
use sapphire_core::Name;
use sapphire_core::alloc::{CountingAllocator, PER_THREAD_COUNTS, count_in_scope};
use sapphire_cues::{CueDispatcher, CueRow, CueStep, CueTable};
use sapphire_math::{Mat4, Vec3};
use sapphire_time::ScaledClock;
use sapphire_vfx::{EffectLibrary as VfxLibrary, VfxBudget, VfxWorld};
use sapphire_world::screen_effects::{EffectLibrary as ScreenLibrary, EffectStack};

#[global_allocator]
static ALLOCATOR: CountingAllocator<std::alloc::System> = CountingAllocator::new(std::alloc::System);

const RATE: u32 = 48_000;
const FRAME: f32 = 1.0 / 60.0;
const STEP: AssetId = AssetId::from_text("5c0e0000-0000-4000-8000-0000000000f0");
const ROAR: AssetId = AssetId::from_text("5c0e0000-0000-4000-8000-0000000000f1");
const STEP_HZ: f32 = 2_000.0;
const ROAR_HZ: f32 = 220.0;
/// The footsteps a storm fires before its roar: with the roar, the plan's 200 requests.
const FOOTSTEPS: usize = 199;

fn row(cue: &str, sound: AssetId) -> CueRow {
    CueRow {
        cue: cue.to_owned(),
        weight: None,
        effects: Vec::new(),
        sounds: vec![sound],
        shake: None,
        screen: None,
        rumble: None,
        line: None,
        cutin: None,
    }
}

/// A sine at `hz`, a quarter of full scale, for this long.
fn tone(hz: f32, seconds: f32) -> DecodedSound {
    let samples = (0..(seconds * RATE as f32) as usize)
        .map(|frame| 0.25 * (std::f32::consts::TAU * hz * frame as f32 / RATE as f32).sin())
        .collect();
    DecodedSound { sample_rate: RATE, channels: 1, samples }
}

/// The footsteps' cue, priority 40 as the shipped `step` is but in no group, so every footstep competes for the
/// real voices; and the roar's, priority 200.
fn system<B: AudioBackend>(backend: B) -> AudioSystem<B> {
    let mut system = AudioSystem::new(backend);
    system.add_sound(STEP, &tone(STEP_HZ, 0.3));
    system.add_sound(ROAR, &tone(ROAR_HZ, 1.0));
    let flat = Cue { play: CueNode::one_of(vec![STEP]), bus: Bus::Sfx, ..Cue::default() };
    system.add_cue(STEP, Cue { volume_db: [-30.0, -30.0], priority: 40, ..flat.clone() });
    system.add_cue(ROAR, Cue { play: CueNode::one_of(vec![ROAR]), priority: 200, ..flat });
    system
}

/// The table's sinks, which fire nothing here but sound.
struct Sinks {
    vfx: VfxWorld,
    screen: EffectStack,
    presets: ScreenLibrary,
    camera: CameraDirector,
}

impl Sinks {
    fn new() -> Self {
        let mut world = ScaledClock::new();
        Self {
            vfx: VfxWorld::new(VfxLibrary::default_effects(), VfxBudget::DESKTOP_LOW, 7),
            screen: EffectStack::new(world.new_local_clock()),
            presets: ScreenLibrary::default_effects(),
            camera: CameraDirector::new(CameraTuning::default_tuning(), 7).expect("valid"),
        }
    }

    /// The storm's cues fired through the table, in order: the footsteps first, the roar last.
    fn storm(&mut self, cues: &mut CueDispatcher) {
        let stand_of = |_: sapphire_gameplay::ActorId| None;
        let mut step = CueStep {
            vfx: &mut self.vfx,
            screen: &mut self.screen,
            presets: &self.presets,
            shakes: &mut self.camera,
            view_projection: Mat4::IDENTITY,
            stand_of: &stand_of,
        };
        for _ in 0..FOOTSTEPS {
            cues.fire_at(Name::new("step"), Vec3::ZERO, &mut step);
        }
        cues.fire_at(Name::new("roar"), Vec3::ZERO, &mut step);
    }
}

/// The frame's sounds played, and the frame ended.
fn end_frame<B: AudioBackend>(cues: &mut CueDispatcher, audio: &mut AudioSystem<B>, random: &mut CueRng) {
    audio.update(
        &AudioFrame {
            dt: FRAME,
            listener: Listener::default(),
            world_time_scale: 1.0,
            snapshots: &[],
            emitters: &[],
            cues: cues.sounds(),
            lines: cues.lines(),
            speech: None,
            music: None,
            rooms: sapphire_audio::Rooms::NONE,
        },
        random,
    );
    cues.end_frame(FRAME, &mut ());
}

/// How much of `hz` is in `samples`: Goertzel's filter, as an amplitude.
fn energy_at(samples: &[f32], hz: f32) -> f32 {
    let coefficient = 2.0 * (std::f32::consts::TAU * hz / RATE as f32).cos();
    let (mut previous, mut before) = (0.0f32, 0.0f32);
    for &sample in samples {
        let next = sample + coefficient * previous - before;
        before = previous;
        previous = next;
    }
    let power = previous * previous + before * before - coefficient * previous * before;
    2.0 * power.max(0.0).sqrt() / samples.len() as f32
}

#[test]
fn a_storm_through_the_cue_table_mixes_the_roar_and_allocates_nothing() {
    let table = CueTable::new(vec![row("step", STEP), row("roar", ROAR)]).expect("valid");
    let new_cues = || {
        let (effects, presets) = (VfxLibrary::default_effects(), ScreenLibrary::default_effects());
        CueDispatcher::new(&table, &effects, &presets, &CameraTuning::default_tuning()).expect("resolves")
    };

    // Through the mock: the roar is among what starts, beside 63 footsteps, and nothing reached the mixer late.
    let (mut cues, mut sinks, mut random) = (new_cues(), Sinks::new(), CueRng::new(1));
    let mut audio = system(MockBackend::new());
    audio.backend_mut().reserve_calls(1 << 12);
    sinks.storm(&mut cues);
    assert_eq!(cues.sounds().len(), FOOTSTEPS + 1, "the table passed every request to the mixer");
    end_frame(&mut cues, &mut audio, &mut random);
    let started: Vec<AssetId> = audio
        .backend_mut()
        .take_calls()
        .into_iter()
        .filter_map(|call| match call {
            MockCall::StartVoice { sound, .. } => Some(sound),
            _ => None,
        })
        .collect();
    assert_eq!(started.len(), MAX_VOICES);
    assert!(started.contains(&ROAR), "the roar, fired last of 200, is mixed");
    assert_eq!(started.iter().filter(|&&sound| sound == STEP).count(), MAX_VOICES - 1);
    let stats = (cues.stats().sounds, cues.stats().sounds_dropped, audio.stats().dropped_voice_limit);
    assert_eq!(stats, (FOOTSTEPS as u64 + 1, 0, (FOOTSTEPS + 1 - MAX_VOICES) as u64));

    // A second storm, once the first has grown what grows once: the table and the mixer allocate nothing. The
    // mock's log was taken with its room, so it's given room again.
    audio.backend_mut().reserve_calls(1 << 12);
    if PER_THREAD_COUNTS {
        let ((), counts) = count_in_scope(|| {
            sinks.storm(&mut cues);
            end_frame(&mut cues, &mut audio, &mut random);
        });
        assert_eq!(counts.allocations, 0, "a storm's frame, through the table and a mock mixer");
    }

    // Through kira: the roar is heard over the footsteps.
    let (mut cues, mut sinks, mut random) = (new_cues(), Sinks::new(), CueRng::new(1));
    let mut audio = system(KiraBackend::<OfflineOutput>::new(RATE).expect("an offline mixer"));
    sinks.storm(&mut cues);
    end_frame(&mut cues, &mut audio, &mut random);
    let mut heard = Vec::new();
    for frame in 0..6 {
        if frame > 0 {
            end_frame(&mut cues, &mut audio, &mut random);
        }
        let out = audio.backend_mut().output_mut().render(RATE as usize / 60);
        heard.extend(out.as_chunks::<2>().0.iter().map(|frame| frame[0]));
    }
    assert!(energy_at(&heard, ROAR_HZ) > 0.1, "the roar: {}", energy_at(&heard, ROAR_HZ));
    assert!(energy_at(&heard, STEP_HZ) > 0.01, "the footsteps: {}", energy_at(&heard, STEP_HZ));
}
