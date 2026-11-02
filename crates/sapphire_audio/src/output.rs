//! What pulls kira's audio: [`CpalOutput`], the audio device, or [`OfflineOutput`], a test asking
//! for samples.
//!
//! Both are kira [`Backend`]s, the crate's public trait for connecting its renderer. Kira's own
//! `CpalBackend` isn't used, for three reasons found reading it: it has no way to stop the stream,
//! which spec §6.5 needs on background and iOS interruptions; its device watcher unwraps a stream
//! restart that fails, so a device lost can end in a panic where it must be a logged state; and it
//! polls the default device twice a second on phones, where the OS routes for us. Kira's
//! `MockBackend` isn't used either: it renders into a buffer nothing can read.

use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU16, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle, Thread};
use std::time::Duration;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{Device, ErrorKind, Stream, StreamConfig};
use kira::backend::{Backend, Renderer};
use sapphire_time::Stopwatch;

use crate::LOG_TARGET;
use crate::backend::{DeviceState, DeviceStatus};

/// A kira backend the audio system can stop and ask about.
pub trait Output: Backend {
    /// Stops pulling audio, keeping the mixer where it is.
    fn suspend(&mut self);
    /// Takes up where [`Self::suspend`] left off.
    fn resume(&mut self);
    /// What the output has done.
    fn status(&self) -> DeviceStatus;
}

/// Device outputs asked for in this process.
static DEVICE_OPENS: AtomicU64 = AtomicU64::new(0);

/// How many times this process has asked for an audio device. Offscreen runs hold this to 0, since
/// CI's runners have none and an app without audio must cost nothing.
pub fn device_opens() -> u64 {
    DEVICE_OPENS.load(Ordering::Relaxed)
}

/// The sample rate a mixer runs at until a device says otherwise.
const FALLBACK_SAMPLE_RATE: u32 = 48_000;

/// How long between tries at a device that can't be opened, in the background of a running app.
const RETRY: Duration = Duration::from_secs(5);

/// The output's atomics: the worker and the callback write, the frame reads.
struct Shared {
    want_running: AtomicBool,
    rebuild: AtomicBool,
    shutdown: AtomicBool,
    state: AtomicU8,
    sample_rate: AtomicU32,
    channels: AtomicU16,
    callbacks: AtomicU64,
    frames: AtomicU64,
    xruns: AtomicU64,
    mix_nanos: AtomicU64,
    mix_nanos_max: AtomicU64,
    streams_opened: AtomicU32,
    /// What the stream's error callback last said, for the worker to log: it may be called on the
    /// audio thread, so it only ever tries the lock.
    last_error: Mutex<Option<cpal::Error>>,
}

impl Shared {
    fn state(&self) -> DeviceState {
        match self.state.load(Ordering::Relaxed) {
            1 => DeviceState::Running,
            2 => DeviceState::Suspended,
            3 => DeviceState::NoDevice,
            _ => DeviceState::Starting,
        }
    }

    fn set_state(&self, state: DeviceState) {
        let code = match state {
            DeviceState::Starting => 0,
            DeviceState::Running => 1,
            DeviceState::Suspended => 2,
            DeviceState::NoDevice => 3,
        };
        self.state.store(code, Ordering::Relaxed);
    }
}

/// The audio device, through cpal: one stream on the host's default output, owned by a worker thread
/// of its own, so opening and stopping it never holds up a frame.
///
/// **No device is a state, not a failure.** With none at startup the mixer runs at 48 kHz with
/// nothing pulling it, reports [`DeviceState::NoDevice`], and the worker tries again every few
/// seconds. A stream the OS invalidates (a device unplugged, media services reset) is rebuilt on the
/// default device the same way, keeping every voice where it was.
///
/// On iOS and Android, `sapphire_platform::Platform::prepare_audio` must have run first: cpal reads
/// the audio session there, and panics opening a stream on Android without the activity it sets.
pub struct CpalOutput {
    shared: Arc<Shared>,
    /// What [`Backend::setup`] opened, for [`Backend::start`].
    opened: Option<(Device, StreamConfig)>,
    worker: Option<(JoinHandle<()>, Thread)>,
}

impl CpalOutput {
    fn wake(&self) {
        if let Some((_, thread)) = &self.worker {
            thread.unpark();
        }
    }
}

/// Why a stream didn't open. Logged by the worker: nothing else depends on it.
fn open_default() -> Result<(Device, StreamConfig), String> {
    let host = cpal::default_host();
    let device = host.default_output_device().ok_or("the host has no default output device")?;
    let config = device.default_output_config().map_err(|error| format!("no output config: {error}"))?;
    Ok((device, config.config()))
}

impl Backend for CpalOutput {
    type Settings = ();
    /// Never returned: see [`CpalOutput`].
    type Error = std::convert::Infallible;

    fn setup(_settings: (), _internal_buffer_size: usize) -> Result<(Self, u32), Self::Error> {
        DEVICE_OPENS.fetch_add(1, Ordering::Relaxed);
        let opened = open_default();
        let sample_rate = opened.as_ref().map_or(FALLBACK_SAMPLE_RATE, |(_, config)| config.sample_rate);
        if let Err(problem) = &opened {
            tracing::warn!(target: LOG_TARGET, problem, "no audio device yet");
        }
        let shared = Arc::new(Shared {
            want_running: AtomicBool::new(true),
            rebuild: AtomicBool::new(false),
            shutdown: AtomicBool::new(false),
            state: AtomicU8::new(0),
            sample_rate: AtomicU32::new(sample_rate),
            channels: AtomicU16::new(0),
            callbacks: AtomicU64::new(0),
            frames: AtomicU64::new(0),
            xruns: AtomicU64::new(0),
            mix_nanos: AtomicU64::new(0),
            mix_nanos_max: AtomicU64::new(0),
            streams_opened: AtomicU32::new(0),
            last_error: Mutex::new(None),
        });
        Ok((Self { shared, opened: opened.ok(), worker: None }, sample_rate))
    }

    fn start(&mut self, renderer: Renderer) -> Result<(), Self::Error> {
        let shared = self.shared.clone();
        let opened = self.opened.take();
        let worker = thread::Builder::new().name("sapphire-audio-device".to_owned()).spawn(move || {
            Worker { shared, renderer: Arc::new(Mutex::new(renderer)), stream: None, paused: false }
                .run(opened)
        });
        match worker {
            Ok(handle) => {
                let thread = handle.thread().clone();
                self.worker = Some((handle, thread));
            }
            Err(error) => {
                // The mixer stays silent, like a machine with no device; the app goes on.
                tracing::error!(target: LOG_TARGET, %error, "can't start the audio device's thread");
                self.shared.set_state(DeviceState::NoDevice);
            }
        }
        Ok(())
    }
}

impl Output for CpalOutput {
    fn suspend(&mut self) {
        self.shared.want_running.store(false, Ordering::Relaxed);
        self.wake();
    }

    fn resume(&mut self) {
        self.shared.want_running.store(true, Ordering::Relaxed);
        self.wake();
    }

    fn status(&self) -> DeviceStatus {
        let shared = &self.shared;
        DeviceStatus {
            state: shared.state(),
            sample_rate: shared.sample_rate.load(Ordering::Relaxed),
            channels: shared.channels.load(Ordering::Relaxed),
            callbacks: shared.callbacks.load(Ordering::Relaxed),
            frames: shared.frames.load(Ordering::Relaxed),
            xruns: shared.xruns.load(Ordering::Relaxed),
            mix_nanos: shared.mix_nanos.load(Ordering::Relaxed),
            mix_nanos_max: shared.mix_nanos_max.load(Ordering::Relaxed),
            streams_opened: shared.streams_opened.load(Ordering::Relaxed),
        }
    }
}

impl Drop for CpalOutput {
    fn drop(&mut self) {
        self.shared.shutdown.store(true, Ordering::Relaxed);
        if let Some((handle, thread)) = self.worker.take() {
            thread.unpark();
            let _ = handle.join();
        }
    }
}

/// The thread that owns the stream, so opening or stopping one never holds up a frame.
struct Worker {
    shared: Arc<Shared>,
    /// Behind a lock so a stream's end doesn't take the mixer with it: the callback only tries it,
    /// and the worker takes it only with no stream running.
    renderer: Arc<Mutex<Renderer>>,
    stream: Option<Stream>,
    paused: bool,
}

impl Worker {
    fn run(mut self, mut opened: Option<(Device, StreamConfig)>) {
        let shared = self.shared.clone();
        while !shared.shutdown.load(Ordering::Relaxed) {
            if let Ok(mut last) = shared.last_error.lock()
                && let Some(error) = last.take()
            {
                tracing::warn!(target: LOG_TARGET, %error, "audio stream error");
            }
            if shared.rebuild.swap(false, Ordering::Relaxed) && self.stream.take().is_some() {
                tracing::warn!(target: LOG_TARGET, "audio stream lost; opening another");
            }
            let wait = if shared.want_running.load(Ordering::Relaxed) {
                self.run_stream(opened.take())
            } else {
                self.pause_stream();
                None
            };
            match wait {
                Some(wait) => thread::park_timeout(wait),
                None => thread::park(),
            }
        }
        // Dropped here, on the thread that made it, before the renderer.
        drop(self.stream.take());
    }

    /// Has a stream running, if a device will have it. Returns how long to wait before trying
    /// again, if it can't.
    fn run_stream(&mut self, opened: Option<(Device, StreamConfig)>) -> Option<Duration> {
        if let Some(stream) = &self.stream {
            if self.paused {
                match stream.play() {
                    Ok(()) => self.paused = false,
                    // The stream can't come back: open another.
                    Err(error) => {
                        tracing::warn!(target: LOG_TARGET, %error, "can't restart the audio stream");
                        self.stream = None;
                        self.paused = false;
                        self.shared.set_state(DeviceState::NoDevice);
                        return Some(Duration::ZERO);
                    }
                }
            }
            self.shared.set_state(DeviceState::Running);
            return None;
        }
        let started =
            opened.map_or_else(open_default, Ok).and_then(|(device, config)| self.build(&device, config));
        match started {
            Ok(stream) => {
                self.stream = Some(stream);
                self.paused = false;
                self.shared.set_state(DeviceState::Running);
                None
            }
            Err(problem) => {
                if self.shared.state() != DeviceState::NoDevice {
                    tracing::warn!(target: LOG_TARGET, problem, "no audio device; retrying in the background");
                }
                self.shared.set_state(DeviceState::NoDevice);
                Some(RETRY)
            }
        }
    }

    fn pause_stream(&mut self) {
        if let Some(stream) = &self.stream
            && !self.paused
        {
            match stream.pause() {
                Ok(()) => self.paused = true,
                // Without a way to stop it, it goes.
                Err(error) => {
                    tracing::warn!(target: LOG_TARGET, %error, "can't pause the audio stream; closing it");
                    self.stream = None;
                }
            }
        }
        self.shared.set_state(DeviceState::Suspended);
    }

    fn build(&mut self, device: &Device, config: StreamConfig) -> Result<Stream, String> {
        let shared = self.shared.clone();
        // No stream runs now, so this doesn't wait on a callback.
        let rate = config.sample_rate;
        if shared.sample_rate.swap(rate, Ordering::Relaxed) != rate
            && let Ok(mut renderer) = self.renderer.lock()
        {
            renderer.on_change_sample_rate(rate);
        }
        let channels = config.channels;
        shared.channels.store(channels, Ordering::Relaxed);
        let (renderer, data_shared) = (self.renderer.clone(), shared.clone());
        let on_data = move |data: &mut [f32], _: &cpal::OutputCallbackInfo| {
            let timer = Stopwatch::start();
            match renderer.try_lock() {
                Ok(mut renderer) => {
                    renderer.on_start_processing();
                    renderer.process(data, channels);
                }
                Err(_) => data.fill(0.0),
            }
            let nanos = timer.elapsed().as_nanos() as u64;
            let shared = &data_shared;
            shared.callbacks.fetch_add(1, Ordering::Relaxed);
            shared.frames.fetch_add((data.len() / usize::from(channels.max(1))) as u64, Ordering::Relaxed);
            shared.mix_nanos.fetch_add(nanos, Ordering::Relaxed);
            shared.mix_nanos_max.fetch_max(nanos, Ordering::Relaxed);
        };
        let worker = thread::current();
        let error_shared = shared.clone();
        let on_error = move |error: cpal::Error| {
            let shared = &error_shared;
            match error.kind() {
                // These come from the audio thread itself, many times over: only count them.
                ErrorKind::Xrun => {
                    shared.xruns.fetch_add(1, Ordering::Relaxed);
                    return;
                }
                ErrorKind::DeviceNotAvailable | ErrorKind::StreamInvalidated | ErrorKind::DeviceChanged => {
                    shared.rebuild.store(true, Ordering::Relaxed);
                }
                _ => {}
            }
            if let Ok(mut last) = shared.last_error.try_lock() {
                *last = Some(error);
            }
            worker.unpark();
        };
        let stream = device
            .build_output_stream(config, on_data, on_error, None)
            .map_err(|error| format!("can't open a stream: {error}"))?;
        stream.play().map_err(|error| format!("can't start the stream: {error}"))?;
        let opened = shared.streams_opened.fetch_add(1, Ordering::Relaxed) + 1;
        tracing::info!(
            target: LOG_TARGET,
            sample_rate = rate,
            channels,
            buffer = ?config.buffer_size,
            streams_opened = opened,
            "audio stream started"
        );
        Ok(stream)
    }
}

/// No device: a caller asks for samples and reads them, so tests hear the real mixer. Rendering
/// goes in the callback's shape, block by block, each a fresh `on_start_processing`.
pub struct OfflineOutput {
    renderer: Option<Renderer>,
    sample_rate: u32,
    suspended: bool,
    callbacks: u64,
    frames: u64,
}

/// Frames per block of [`OfflineOutput::render`]: about what a device asks for.
const OFFLINE_BLOCK: usize = 512;

impl OfflineOutput {
    /// Renders `frames` frames of interleaved stereo. Suspended, it renders silence and the mixer
    /// doesn't move, as a paused stream's doesn't.
    pub fn render(&mut self, frames: usize) -> Vec<f32> {
        let mut out = vec![0.0; frames * 2];
        self.render_into(&mut out);
        out
    }

    /// [`Self::render`], into `out`: what the audio callback does, with no allocation of its own.
    pub fn render_into(&mut self, out: &mut [f32]) {
        if self.suspended {
            out.fill(0.0);
            return;
        }
        let renderer = self.renderer.as_mut().expect("kira starts its backend on creation");
        for block in out.chunks_mut(OFFLINE_BLOCK * 2) {
            renderer.on_start_processing();
            renderer.process(block, 2);
            self.callbacks += 1;
            self.frames += block.len() as u64 / 2;
        }
    }
}

impl Backend for OfflineOutput {
    /// The sample rate.
    type Settings = u32;
    type Error = std::convert::Infallible;

    fn setup(sample_rate: u32, _internal_buffer_size: usize) -> Result<(Self, u32), Self::Error> {
        Ok((Self { renderer: None, sample_rate, suspended: false, callbacks: 0, frames: 0 }, sample_rate))
    }

    fn start(&mut self, renderer: Renderer) -> Result<(), Self::Error> {
        self.renderer = Some(renderer);
        Ok(())
    }
}

impl Output for OfflineOutput {
    fn suspend(&mut self) {
        self.suspended = true;
    }

    fn resume(&mut self) {
        self.suspended = false;
    }

    fn status(&self) -> DeviceStatus {
        DeviceStatus {
            state: if self.suspended { DeviceState::Suspended } else { DeviceState::Running },
            sample_rate: self.sample_rate,
            channels: 2,
            callbacks: self.callbacks,
            frames: self.frames,
            ..DeviceStatus::default()
        }
    }
}
