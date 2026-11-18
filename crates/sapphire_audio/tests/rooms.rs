//! Rooms through the real mixer (M6 U5): the room's reverb is heard, and walking into a shelter crosses from the
//! street's reverb to the shelter's with no click, on two reverbs and on one. Which room plays and which voices
//! reach it are tested against the mock, in `system.rs`.

use sapphire_assets::AssetId;
use sapphire_assets::cue::{Bus, Cue};
use sapphire_assets::mix::Mix;
use sapphire_assets::sound::DecodedSound;
use sapphire_audio::{
    AudioFrame, AudioSystem, CueRequest, CueRng, CueTarget, Emitter, EmitterId, KiraBackend, Listener,
    OfflineOutput, Room, RoomShape, Rooms,
};
use sapphire_core::Name;
use sapphire_math::Vec3;

const RATE: u32 = 48_000;
/// A frame of the game at 60 fps, in audio frames.
const GAME_FRAME: usize = RATE as usize / 60;
/// The mixer's own buffer, in frames.
const BUFFER: usize = 128;

const HUM: AssetId = AssetId::from_text("5c0e0000-0000-4000-8000-0000000000e0");
const CLAP: AssetId = AssetId::from_text("5c0e0000-0000-4000-8000-0000000000e1");
const WALKER: EmitterId = EmitterId(1);

/// The shelter's fade, in seconds.
const SHELTER_FADE: f32 = 0.4;

/// A rising sweep, 200 Hz to 600 Hz, for this long, a quarter of full scale: it loops with no seam of its own.
fn sweep(seconds: f32) -> DecodedSound {
    let length = seconds * RATE as f32;
    let samples = (0..length as usize)
        .map(|frame| {
            let time = frame as f32 / RATE as f32;
            let phase = 200.0 * time + 400.0 * time * time / (2.0 * seconds);
            0.25 * (std::f32::consts::TAU * phase).sin()
        })
        .collect();
    DecodedSound { sample_rate: RATE, channels: 1, samples }
}

/// A 220 Hz tone, 0.2 s: 44 whole cycles, so it ends at nothing.
fn clap() -> DecodedSound {
    let samples = (0..RATE as usize / 5)
        .map(|frame| 0.25 * (std::f32::consts::TAU * 220.0 * frame as f32 / RATE as f32).sin())
        .collect();
    DecodedSound { sample_rate: RATE, channels: 1, samples }
}

fn left(stereo: &[f32]) -> Vec<f32> {
    stereo.as_chunks::<2>().0.iter().map(|frame| frame[0]).collect()
}

fn largest_step(samples: &[f32]) -> f32 {
    samples.windows(2).map(|pair| (pair[1] - pair[0]).abs()).fold(0.0, f32::max)
}

fn loudness(samples: &[f32]) -> f32 {
    (samples.iter().map(|sample| sample * sample).sum::<f32>() / samples.len() as f32).sqrt()
}

/// The shipped mix, with its `street` and `shelter` reverbs.
fn mixer(slots: u8) -> AudioSystem<KiraBackend<OfflineOutput>> {
    let mut system = AudioSystem::new(KiraBackend::<OfflineOutput>::new(RATE).expect("an offline mixer"));
    system
        .set_mix(&Mix::from_document(include_str!("../../../content/audio/game.mix.ron")).unwrap())
        .unwrap();
    system.set_reverb_slots(slots);
    system.add_sound(HUM, &sweep(0.5));
    system.add_sound(CLAP, &clap());
    let flat = Cue { sounds: vec![HUM], bus: Bus::Sfx, volume_db: [-6.0, -6.0], ..Cue::default() };
    system.add_cue(HUM, Cue { looped: true, ..flat.clone() });
    system.add_cue(CLAP, Cue { sounds: vec![CLAP], ..flat });
    system
}

/// A street open to the sky, and a shelter in it 20 m ahead.
fn rooms() -> [Room; 2] {
    [
        Room {
            position: Vec3::ZERO,
            shape: RoomShape::Box { half_extents: Vec3::new(50.0, 10.0, 50.0) },
            priority: 0,
            reverb: Name::new("street"),
            outdoor: true,
            fade: 0.5,
        },
        Room {
            position: Vec3::new(0.0, 0.0, -20.0),
            shape: RoomShape::Box { half_extents: Vec3::splat(3.0) },
            priority: 2,
            reverb: Name::new("shelter"),
            outdoor: false,
            fade: SHELTER_FADE,
        },
    ]
}

/// One game frame with the listener, and a walker beside it, at `at`: the update, then its audio's left side.
fn step(
    system: &mut AudioSystem<KiraBackend<OfflineOutput>>,
    at: Vec3,
    rooms: Rooms<'_>,
    cues: &[CueRequest],
    rng: &mut CueRng,
) -> Vec<f32> {
    let walker = [Emitter { id: WALKER, position: at, hitstop: false }];
    let frame = AudioFrame {
        dt: GAME_FRAME as f32 / RATE as f32,
        listener: Listener { position: at, ..Listener::default() },
        world_time_scale: 1.0,
        snapshots: &[],
        emitters: &walker,
        cues,
        lines: &[],
        speech: None,
        music: None,
        rooms,
    };
    system.update(&frame, rng);
    left(&system.backend_mut().output_mut().render(GAME_FRAME))
}

/// What the reverb adds to the hum walked along `path` for 120 frames, from the street: the walk less the same
/// walk with no rooms; and the frame the room last changed.
fn walk(slots: u8, path: impl Fn(usize) -> Vec3) -> (Vec<f32>, usize) {
    let rooms = rooms();
    let run = |rooms: Rooms<'_>| {
        let mut system = mixer(slots);
        let mut rng = CueRng::new(1);
        let hum = [CueRequest { cue: HUM, target: CueTarget::On(WALKER) }];
        let (mut out, mut changed) = (Vec::new(), 0);
        for frame in 0..120 {
            let changes = system.stats().room_changes;
            let cues: &[CueRequest] = if frame == 0 { &hum } else { &[] };
            out.extend(step(&mut system, path(frame), rooms, cues, &mut rng));
            if system.stats().room_changes > changes {
                changed = frame;
            }
        }
        assert_eq!(system.stats().voices, 1);
        (out, changed)
    };
    let (heard, changed) = run(Rooms { rooms: &rooms, origin: Vec3::ZERO });
    let (dry, _) = run(Rooms::NONE);
    (heard.iter().zip(&dry).map(|(heard, dry)| heard - dry).collect(), changed)
}

/// Walking from the street into the shelter crosses between their reverbs over the shelter's fade, with no step
/// in what the reverb adds bigger than either room's own, standing still; and past the fade it is the shelter's.
fn walking_in_crosses_without_a_click(slots: u8) {
    let (street, _) = walk(slots, |_| Vec3::ZERO);
    let (shelter, _) = walk(slots, |_| Vec3::new(0.0, 0.0, -20.0));
    let (walked, entered) = walk(slots, |frame| Vec3::new(0.0, 0.0, -0.5 * frame.min(40) as f32));
    assert_eq!(entered, 34, "the frame it's 17 m in, at the shelter's wall");
    let settled = GAME_FRAME * 40;
    let own = largest_step(&street[settled..]).max(largest_step(&shelter[settled..]));
    assert!(
        loudness(&street[settled..]) > 1e-3 && loudness(&shelter[settled..]) > 1e-3,
        "both rooms are heard"
    );
    let fade = (SHELTER_FADE * RATE as f32) as usize;
    let seam = &walked[entered * GAME_FRAME..entered * GAME_FRAME + fade + 2 * BUFFER];
    #[allow(clippy::print_stdout, reason = "the measurement the plan's note reports")]
    {
        println!(
            "{slots} reverb(s): the reverb's largest step across the room change {}, the rooms' own {own}",
            largest_step(seam)
        );
    }
    assert!(largest_step(seam) <= own * 1.05, "a click: {} against {own}", largest_step(seam));
    let after = (entered + 30) * GAME_FRAME;
    let off = |reference: &[f32]| {
        let apart: Vec<f32> =
            walked[after..after + fade].iter().zip(&reference[after..]).map(|(a, b)| a - b).collect();
        loudness(&apart)
    };
    assert!(
        off(&shelter) < off(&street) * 0.5,
        "{} off the shelter's, {} off the street's",
        off(&shelter),
        off(&street)
    );
}

#[test]
fn walking_into_a_shelter_crosses_its_reverb_in_without_a_click() {
    walking_in_crosses_without_a_click(2);
}

#[test]
fn on_one_reverb_walking_into_a_shelter_glides_without_a_click() {
    walking_in_crosses_without_a_click(1);
}

/// A clap in the shelter rings on after it ends; with no room, nothing does.
#[test]
fn the_shelter_rings_on_after_a_clap() {
    let rooms = rooms();
    let tail = |rooms: Rooms<'_>| {
        let mut system = mixer(2);
        let mut rng = CueRng::new(1);
        let clap = [CueRequest { cue: CLAP, target: CueTarget::On(WALKER) }];
        let mut out = Vec::new();
        for frame in 0..80 {
            let cues: &[CueRequest] = if frame == 40 { &clap } else { &[] };
            out.extend(step(&mut system, Vec3::new(0.0, 0.0, -20.0), rooms, cues, &mut rng));
        }
        // From a buffer past the clap's end.
        loudness(&out[40 * GAME_FRAME + RATE as usize / 5 + 2 * BUFFER..])
    };
    let (ringing, dry) = (tail(Rooms { rooms: &rooms, origin: Vec3::ZERO }), tail(Rooms::NONE));
    assert!(dry < 1e-6, "{dry}");
    assert!(ringing > 1e-3, "{ringing}");
}

/// A room entered in silence plays nothing of an earlier room's tail: a clap in the shelter, then `away` frames out
/// of every room, then 100 frames in the street with nothing playing. The loudest the street plays, all of it what
/// the reverbs kept, and the loudest of the shelter's own tail.
fn after_a_wait(slots: u8, away: usize) -> (f32, f32) {
    let rooms = rooms();
    let peak = |samples: &[f32]| samples.iter().fold(0.0_f32, |most, sample| most.max(sample.abs()));
    let mut system = mixer(slots);
    let mut rng = CueRng::new(1);
    let clap = [CueRequest { cue: CLAP, target: CueTarget::On(WALKER) }];
    let (shelter, nowhere, street) = (Vec3::new(0.0, 0.0, -20.0), Vec3::new(0.0, 0.0, 80.0), Vec3::ZERO);
    let entered = 40 + away;
    let mut out = Vec::new();
    for frame in 0..entered + 100 {
        let at = if frame < 40 {
            shelter
        } else if frame < entered {
            nowhere
        } else {
            street
        };
        let cues: &[CueRequest] = if frame == 5 { &clap } else { &[] };
        out.extend(step(&mut system, at, Rooms { rooms: &rooms, origin: Vec3::ZERO }, cues, &mut rng));
    }
    // The shelter's own tail, from a buffer past the clap's end until the walk out.
    let tail = peak(&out[5 * GAME_FRAME + RATE as usize / 5 + 2 * BUFFER..40 * GAME_FRAME]);
    (peak(&out[entered * GAME_FRAME..]), tail)
}

/// Out of every room long enough for the reverbs to ring out, the street, entered in silence, plays nothing of the
/// shelter's tail; a second out, what's left is far under the least step of 16 bits.
#[test]
fn a_room_entered_in_silence_brings_back_no_earlier_tail() {
    for slots in [2, 1] {
        let (stale, tail) = after_a_wait(slots, 300);
        let (soon, _) = after_a_wait(slots, 60);
        #[allow(clippy::print_stdout, reason = "the measurement the plan's note reports")]
        {
            println!(
                "{slots} reverb(s): the street entered in silence plays {stale} at most after 5 s away, {soon} after \
                 1 s; the shelter's tail {tail}"
            );
        }
        assert!(tail > 1e-3, "{slots}: the shelter rings: {tail}");
        assert!(
            stale <= tail * 1e-4,
            "{slots}: the street brought back the shelter's tail: {stale} against {tail}"
        );
        assert!(soon <= tail * 1e-3, "{slots}: a second away: {soon} against {tail}");
    }
}
