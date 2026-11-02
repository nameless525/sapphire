//! The real device, or none: whatever this machine has, the output must run or say it can't, and
//! never fail. CI runs this on each desktop OS's runner, where there may be no sound hardware at all.
//!
//! `#[ignore]`d so a plain `cargo test` on a machine someone's using opens no device. It plays
//! silence's neighbor (-60 dB), so nobody hears it:
//! `cargo test -p sapphire_audio --test device -- --ignored`.

use std::thread::sleep;
use std::time::Duration;

use sapphire_assets::AssetId;
use sapphire_assets::cue::Bus;
use sapphire_assets::sound::DecodedSound;
use sapphire_audio::{
    AudioBackend, BusParams, CpalOutput, DeviceState, DeviceStatus, KiraBackend, VoiceParams, VoiceSlot,
    device_opens,
};
use sapphire_time::Stopwatch;

const SOUND: AssetId = AssetId::from_text("00000000-0000-4000-8000-00000000d00d");

/// One second of a quiet tone.
fn tone() -> DecodedSound {
    let samples = (0..48_000).map(|frame| (frame as f32 * 0.05).sin() * 0.1).collect();
    DecodedSound { sample_rate: 48_000, channels: 1, samples }
}

/// Polls until `done` says so, or `limit` passes.
fn wait_for(
    backend: &mut KiraBackend<CpalOutput>,
    limit: Duration,
    done: impl Fn(&DeviceStatus) -> bool,
) -> DeviceStatus {
    let start = Stopwatch::start();
    loop {
        let status = backend.status();
        if done(&status) || start.elapsed() > limit {
            return status;
        }
        sleep(Duration::from_millis(10));
    }
}

#[test]
#[ignore = "opens the audio device"]
fn whatever_the_device_the_output_runs_or_says_why_not() {
    let opens = device_opens();
    let Ok(mut backend) = KiraBackend::<CpalOutput>::new(());
    assert_eq!(device_opens(), opens + 1, "one output, one count");
    backend.add_sound(SOUND, &tone());
    backend.set_bus(Bus::Sfx, BusParams::OPEN);
    let quiet = VoiceParams { gain_db: -60.0, pan: 0.0, pitch: 1.0 };
    backend.start_voice(VoiceSlot(0), SOUND, Bus::Sfx, quiet, false).expect("a voice starts");

    let settled = |status: &DeviceStatus| status.state != DeviceState::Starting;
    let status = wait_for(&mut backend, Duration::from_secs(10), settled);
    eprintln!("after starting: {status:?}");
    match status.state {
        DeviceState::NoDevice => {
            // A runner with no sound hardware: a logged state, and everything still answers.
            eprintln!("no audio device here: nothing more to test");
            backend.suspend();
            backend.resume();
            return;
        }
        DeviceState::Running => {}
        state => panic!("the output should run, or say there's no device: {state}"),
    }
    assert!(status.sample_rate > 0 && status.channels > 0, "{status:?}");

    let at_start = status.callbacks;
    let status = wait_for(&mut backend, Duration::from_secs(10), |status| status.callbacks > at_start + 2);
    if status.callbacks == 0 {
        // A virtual machine's device may open and never ask for anything. Nothing has failed.
        eprintln!("the device opened and never called: nothing more to test");
        return;
    }
    assert!(status.callbacks > at_start + 2, "the device stopped calling: {status:?}");
    assert!(status.frames > 0, "{status:?}");

    backend.suspend();
    let stopped =
        wait_for(&mut backend, Duration::from_secs(5), |status| status.state == DeviceState::Suspended);
    assert_eq!(stopped.state, DeviceState::Suspended, "{stopped:?}");
    // A callback already running may still finish.
    sleep(Duration::from_millis(100));
    let before = backend.status().callbacks;
    sleep(Duration::from_millis(200));
    let after = backend.status().callbacks;
    assert_eq!(before, after, "a suspended output asks for no audio");

    backend.resume();
    let status = wait_for(&mut backend, Duration::from_secs(10), |status| {
        status.state == DeviceState::Running && status.callbacks > after + 2
    });
    assert!(status.callbacks > after + 2, "it didn't come back: {status:?}");

    drop(backend);
}
