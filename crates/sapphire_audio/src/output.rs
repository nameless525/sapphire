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
use cpal::{
    Device, ErrorKind, FromSample, I24, Sample, SampleFormat, SizedSample, Stream, SupportedStreamConfig, U24,
};
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

/// How many frames a device that doesn't take `f32` is mixed at a time, into a buffer made with its
/// stream. A multiple of the mixer's own block, so the mix comes out as it would into `f32`.
const CONVERTED_FRAMES: usize = 512;

/// The most a mix converts to, just under 1. `dasp_sample`'s conversions take -1 up to but not including
/// 1, while kira clips to exactly 1, which overflows 24-bit samples.
const BELOW_ONE: f32 = 1.0 - f32::EPSILON;

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
/// **It asks for the samples the device says it takes,** in its default config: `f32` on nearly every
/// device, and otherwise the mix is converted on the callback, with no allocation.
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
    opened: Option<(Device, SupportedStreamConfig)>,
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
fn open_default() -> Result<(Device, SupportedStreamConfig), String> {
    let host = cpal::default_host();
    let device = host.default_output_device().ok_or("the host has no default output device")?;
    let config = device.default_output_config().map_err(|error| format!("no output config: {error}"))?;
    Ok((device, config))
}

impl Backend for CpalOutput {
    type Settings = ();
    /// Never returned: see [`CpalOutput`].
    type Error = std::convert::Infallible;

    fn setup(_settings: (), _internal_buffer_size: usize) -> Result<(Self, u32), Self::Error> {
        DEVICE_OPENS.fetch_add(1, Ordering::Relaxed);
        let opened = open_default();
        let sample_rate = opened.as_ref().map_or(FALLBACK_SAMPLE_RATE, |(_, config)| config.sample_rate());
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
    fn run(mut self, mut opened: Option<(Device, SupportedStreamConfig)>) {
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
    fn run_stream(&mut self, opened: Option<(Device, SupportedStreamConfig)>) -> Option<Duration> {
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

    fn build(&mut self, device: &Device, config: SupportedStreamConfig) -> Result<Stream, String> {
        let shared = self.shared.clone();
        // No stream runs now, so this doesn't wait on a callback.
        let rate = config.sample_rate();
        if shared.sample_rate.swap(rate, Ordering::Relaxed) != rate
            && let Ok(mut renderer) = self.renderer.lock()
        {
            renderer.on_change_sample_rate(rate);
        }
        let channels = config.channels();
        shared.channels.store(channels, Ordering::Relaxed);
        let (format, buffer) = (config.sample_format(), config.config().buffer_size);
        let parts = StreamParts { device, config, renderer: self.renderer.clone(), shared: shared.clone() };
        let stream = open_stream(parts, thread::current())?;
        stream.play().map_err(|error| format!("can't start the stream: {error}"))?;
        let opened = shared.streams_opened.fetch_add(1, Ordering::Relaxed) + 1;
        tracing::info!(
            target: LOG_TARGET,
            sample_rate = rate,
            channels,
            %format,
            ?buffer,
            streams_opened = opened,
            "audio stream started"
        );
        Ok(stream)
    }
}

/// What a stream needs, whichever samples it takes.
struct StreamParts<'a, D> {
    device: &'a D,
    config: SupportedStreamConfig,
    renderer: Arc<Mutex<Renderer>>,
    shared: Arc<Shared>,
}

/// A stream on `parts.device`, in the samples its config says it takes: a device need not take `f32`,
/// and one that doesn't refuses a stream of them for good. A stream's errors wake `worker`.
fn open_stream<D: DeviceTrait>(parts: StreamParts<'_, D>, worker: Thread) -> Result<D::Stream, String> {
    match parts.config.sample_format() {
        SampleFormat::F32 => stream_of(parts, worker, |renderer, data: &mut [f32], channels| {
            renderer.on_start_processing();
            renderer.process(data, channels);
        }),
        SampleFormat::F64 => converting_stream::<D, f64>(parts, worker),
        SampleFormat::I8 => converting_stream::<D, i8>(parts, worker),
        SampleFormat::I16 => converting_stream::<D, i16>(parts, worker),
        SampleFormat::I24 => converting_stream::<D, I24>(parts, worker),
        SampleFormat::I32 => converting_stream::<D, i32>(parts, worker),
        SampleFormat::I64 => converting_stream::<D, i64>(parts, worker),
        SampleFormat::U8 => converting_stream::<D, u8>(parts, worker),
        SampleFormat::U16 => converting_stream::<D, u16>(parts, worker),
        SampleFormat::U24 => converting_stream::<D, U24>(parts, worker),
        SampleFormat::U32 => converting_stream::<D, u32>(parts, worker),
        SampleFormat::U64 => converting_stream::<D, u64>(parts, worker),
        other => Err(format!("can't open a stream: nothing here writes {other} samples")),
    }
}

/// A stream of `T`s, each block converted from the mix.
fn converting_stream<D, T>(parts: StreamParts<'_, D>, worker: Thread) -> Result<D::Stream, String>
where
    D: DeviceTrait,
    T: SizedSample + FromSample<f32> + 'static,
{
    let mut scratch = vec![0.0; CONVERTED_FRAMES * usize::from(parts.config.channels().max(1))];
    stream_of(parts, worker, move |renderer, data: &mut [T], channels| {
        mix_as(renderer, data, channels, &mut scratch);
    })
}

/// A stream of `T`s that `fill` writes, under the callback's timing and its counters.
fn stream_of<D, T>(
    parts: StreamParts<'_, D>,
    worker: Thread,
    mut fill: impl FnMut(&mut Renderer, &mut [T], u16) + Send + 'static,
) -> Result<D::Stream, String>
where
    D: DeviceTrait,
    T: SizedSample + 'static,
{
    let StreamParts { device, config, renderer, shared } = parts;
    let config = config.config();
    let channels = config.channels;
    let data_shared = shared.clone();
    let on_data = move |data: &mut [T], _: &cpal::OutputCallbackInfo| {
        let timer = Stopwatch::start();
        match renderer.try_lock() {
            Ok(mut renderer) => fill(&mut renderer, data, channels),
            Err(_) => data.fill(T::EQUILIBRIUM),
        }
        let nanos = timer.elapsed().as_nanos() as u64;
        let shared = &data_shared;
        shared.callbacks.fetch_add(1, Ordering::Relaxed);
        shared.frames.fetch_add((data.len() / usize::from(channels.max(1))) as u64, Ordering::Relaxed);
        shared.mix_nanos.fetch_add(nanos, Ordering::Relaxed);
        shared.mix_nanos_max.fetch_max(nanos, Ordering::Relaxed);
    };
    let on_error = move |error: cpal::Error| {
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
    device
        .build_output_stream(config, on_data, on_error, None)
        .map_err(|error| format!("can't open a stream: {error}"))
}

/// One callback's mix, into `data` as `T`s, through `scratch`, whole frames long.
fn mix_as<T: Sample + FromSample<f32>>(
    renderer: &mut Renderer,
    data: &mut [T],
    channels: u16,
    scratch: &mut [f32],
) {
    renderer.on_start_processing();
    for block in data.chunks_mut(scratch.len()) {
        let mixed = &mut scratch[..block.len()];
        renderer.process(mixed, channels);
        for (sample, &mixed) in block.iter_mut().zip(mixed.iter()) {
            *sample = device_sample(mixed);
        }
    }
}

/// A mixed sample as a device takes it: silence for a NaN, which would be the lowest value an unsigned
/// sample has.
fn device_sample<T: Sample + FromSample<f32>>(sample: f32) -> T {
    if sample.is_nan() { T::EQUILIBRIUM } else { T::from_sample(sample.clamp(-1.0, BELOW_ONE)) }
}

/// No device: a caller asks for samples and reads them, so tests hear the real mixer. Rendering
/// goes in the callback's shape, block by block, each a fresh `on_start_processing`.
pub struct OfflineOutput {
    renderer: Option<Renderer>,
    /// What [`Self::render_samples`] converts through, as a stream of them has.
    scratch: Vec<f32>,
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

    /// [`Self::render_into`], for a device that takes `T`s rather than `f32`s: what its callback does.
    pub fn render_samples<T: Sample + FromSample<f32>>(&mut self, out: &mut [T]) {
        if self.suspended {
            out.fill(T::EQUILIBRIUM);
            return;
        }
        let renderer = self.renderer.as_mut().expect("kira starts its backend on creation");
        for block in out.chunks_mut(OFFLINE_BLOCK * 2) {
            mix_as(renderer, block, 2, &mut self.scratch);
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
        let scratch = vec![0.0; CONVERTED_FRAMES * 2];
        Ok((
            Self { renderer: None, scratch, sample_rate, suspended: false, callbacks: 0, frames: 0 },
            sample_rate,
        ))
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

#[cfg(test)]
mod tests {
    use std::fmt;
    use std::hash::{Hash, Hasher};

    use cpal::{
        Data, DeviceDescription, DeviceId, InputCallbackInfo, OutputCallbackInfo, OutputStreamTimestamp,
        StreamConfig, StreamInstant, SupportedBufferSize, SupportedStreamConfigRange,
    };
    use sapphire_assets::AssetId;
    use sapphire_assets::cue::Bus;
    use sapphire_assets::sound::DecodedSound;

    use super::*;
    use crate::backend::{AudioBackend, VoiceParams, VoiceSlot};
    use crate::kira_backend::KiraBackend;

    const RATE: u32 = 48_000;
    const SOUND: AssetId = AssetId::from_text("4f6b2c1e-8a3d-4e5f-9b7c-0d1e2f3a4b5c");

    type RawCallback = Box<dyn FnMut(&mut Data, &OutputCallbackInfo) + Send>;

    /// A device that takes one format and refuses every other, as a device with no `f32` does, and
    /// whose stream's callback a test calls by hand.
    #[derive(Clone)]
    struct OneFormat {
        format: SampleFormat,
        channels: u16,
        callback: Arc<Mutex<Option<RawCallback>>>,
    }

    impl OneFormat {
        fn new(format: SampleFormat, channels: u16) -> Self {
            Self { format, channels, callback: Arc::default() }
        }
    }

    impl PartialEq for OneFormat {
        fn eq(&self, other: &Self) -> bool {
            self.format == other.format
        }
    }

    impl Eq for OneFormat {}

    impl Hash for OneFormat {
        fn hash<H: Hasher>(&self, state: &mut H) {
            self.channels.hash(state);
        }
    }

    impl fmt::Debug for OneFormat {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            write!(f, "a device taking only {}", self.format)
        }
    }

    impl fmt::Display for OneFormat {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            fmt::Debug::fmt(self, f)
        }
    }

    struct Idle;

    impl StreamTrait for Idle {
        fn play(&self) -> Result<(), cpal::Error> {
            Ok(())
        }

        fn pause(&self) -> Result<(), cpal::Error> {
            Ok(())
        }

        fn buffer_size(&self) -> Result<cpal::FrameCount, cpal::Error> {
            Err(cpal::Error::new(ErrorKind::UnsupportedOperation))
        }

        fn now(&self) -> StreamInstant {
            StreamInstant::new(0, 0)
        }
    }

    impl DeviceTrait for OneFormat {
        type SupportedInputConfigs = std::iter::Empty<SupportedStreamConfigRange>;
        type SupportedOutputConfigs = std::iter::Empty<SupportedStreamConfigRange>;
        type Stream = Idle;

        fn description(&self) -> Result<DeviceDescription, cpal::Error> {
            Err(cpal::Error::new(ErrorKind::UnsupportedOperation))
        }

        fn id(&self) -> Result<DeviceId, cpal::Error> {
            Err(cpal::Error::new(ErrorKind::UnsupportedOperation))
        }

        fn supported_input_configs(&self) -> Result<Self::SupportedInputConfigs, cpal::Error> {
            Ok(std::iter::empty())
        }

        fn supported_output_configs(&self) -> Result<Self::SupportedOutputConfigs, cpal::Error> {
            Ok(std::iter::empty())
        }

        fn default_input_config(&self) -> Result<SupportedStreamConfig, cpal::Error> {
            Err(cpal::Error::new(ErrorKind::UnsupportedOperation))
        }

        fn default_output_config(&self) -> Result<SupportedStreamConfig, cpal::Error> {
            Ok(SupportedStreamConfig::new(self.channels, RATE, SupportedBufferSize::Unknown, self.format))
        }

        fn build_input_stream_raw<D, E>(
            &self,
            _config: StreamConfig,
            _sample_format: SampleFormat,
            _data_callback: D,
            _error_callback: E,
            _timeout: Option<Duration>,
        ) -> Result<Self::Stream, cpal::Error>
        where
            D: FnMut(&Data, &InputCallbackInfo) + Send + 'static,
            E: FnMut(cpal::Error) + Send + 'static,
        {
            Err(cpal::Error::new(ErrorKind::UnsupportedOperation))
        }

        fn build_output_stream_raw<D, E>(
            &self,
            _config: StreamConfig,
            sample_format: SampleFormat,
            data_callback: D,
            _error_callback: E,
            _timeout: Option<Duration>,
        ) -> Result<Self::Stream, cpal::Error>
        where
            D: FnMut(&mut Data, &OutputCallbackInfo) + Send + 'static,
            E: FnMut(cpal::Error) + Send + 'static,
        {
            if sample_format != self.format {
                let refusal = format!("this device takes {}, not {sample_format}", self.format);
                return Err(cpal::Error::with_message(ErrorKind::UnsupportedConfig, refusal));
            }
            *self.callback.lock().expect("not poisoned") = Some(Box::new(data_callback));
            Ok(Idle)
        }
    }

    /// What pulls kira's audio here: nothing, until a test hands its renderer to a stream.
    struct Unstarted(Arc<Mutex<Option<Renderer>>>);

    impl Backend for Unstarted {
        type Settings = Arc<Mutex<Option<Renderer>>>;
        type Error = std::convert::Infallible;

        fn setup(slot: Self::Settings, _internal_buffer_size: usize) -> Result<(Self, u32), Self::Error> {
            Ok((Self(slot), RATE))
        }

        fn start(&mut self, renderer: Renderer) -> Result<(), Self::Error> {
            *self.0.lock().expect("not poisoned") = Some(renderer);
            Ok(())
        }
    }

    impl Output for Unstarted {
        fn suspend(&mut self) {}

        fn resume(&mut self) {}

        fn status(&self) -> DeviceStatus {
            DeviceStatus::default()
        }
    }

    fn shared() -> Arc<Shared> {
        Arc::new(Shared {
            want_running: AtomicBool::new(true),
            rebuild: AtomicBool::new(false),
            shutdown: AtomicBool::new(false),
            state: AtomicU8::new(0),
            sample_rate: AtomicU32::new(RATE),
            channels: AtomicU16::new(0),
            callbacks: AtomicU64::new(0),
            frames: AtomicU64::new(0),
            xruns: AtomicU64::new(0),
            mix_nanos: AtomicU64::new(0),
            mix_nanos_max: AtomicU64::new(0),
            streams_opened: AtomicU32::new(0),
            last_error: Mutex::new(None),
        })
    }

    /// `backend`, playing half a second of a loud tone.
    fn playing<O: Output>(mut backend: KiraBackend<O>) -> KiraBackend<O> {
        let samples = (0..RATE / 2).map(|frame| (frame as f32 * 0.05).sin() * 0.75).collect();
        backend.add_sound(SOUND, &DecodedSound { sample_rate: RATE, channels: 1, samples });
        let params = VoiceParams { gain_db: 0.0, pan: 0.0, pitch: 1.0 };
        backend.start_voice(VoiceSlot(0), SOUND, Bus::Ui, params, false).expect("a voice");
        backend
    }

    /// A stream on `device` in the samples its default config says, with the real mixer playing a tone.
    struct OnDevice {
        device: OneFormat,
        renderer: Arc<Mutex<Renderer>>,
        shared: Arc<Shared>,
        /// Holds the mixer's other end, which its handles write to.
        _backend: KiraBackend<Unstarted>,
    }

    impl OnDevice {
        fn open(device: OneFormat) -> Result<Self, String> {
            let slot = Arc::default();
            let backend = playing(KiraBackend::<Unstarted>::new(Arc::clone(&slot)).expect("a mixer"));
            let renderer = slot.lock().expect("not poisoned").take().expect("kira started its backend");
            let (renderer, shared) = (Arc::new(Mutex::new(renderer)), shared());
            let config = device.default_output_config().expect("it has one");
            let parts =
                StreamParts { device: &device, config, renderer: renderer.clone(), shared: shared.clone() };
            let Idle = open_stream(parts, thread::current())?;
            Ok(Self { device, renderer, shared, _backend: backend })
        }

        /// What the device would be given for `frames` frames of `T`s.
        fn callback<T: SizedSample>(&self, frames: usize) -> Vec<T> {
            let mut samples = vec![T::EQUILIBRIUM; frames * usize::from(self.device.channels)];
            let mut slot = self.device.callback.lock().expect("not poisoned");
            let callback = slot.as_mut().expect("the stream was built");
            // SAFETY: `samples` is `T`s, as `T::FORMAT` says, and outlives the call.
            let mut data = unsafe { Data::from_parts(samples.as_mut_ptr().cast(), samples.len(), T::FORMAT) };
            let now = StreamInstant::new(0, 0);
            callback(
                &mut data,
                &OutputCallbackInfo::new(OutputStreamTimestamp { callback: now, playback: now }),
            );
            samples
        }
    }

    /// The same tone through the same mixer, as floats, in the same blocks.
    fn tone_as_floats(frames: usize) -> Vec<f32> {
        let backend = KiraBackend::<OfflineOutput>::new(RATE).expect("an offline mixer");
        let mut backend = playing(backend);
        backend.output_mut().render(frames)
    }

    /// A device with no `f32` used to refuse every stream, and every retry 5 s later, for good.
    fn takes<T: SizedSample + FromSample<f32> + PartialEq + fmt::Debug>() {
        let stream = OnDevice::open(OneFormat::new(T::FORMAT, 2)).expect("it asks for what the device takes");
        let got = stream.callback::<T>(512);
        let expected: Vec<T> = tone_as_floats(512).iter().map(|&sample| device_sample(sample)).collect();
        assert_eq!(got, expected, "{}", T::FORMAT);
        assert_eq!(stream.shared.callbacks.load(Ordering::Relaxed), 1);
        assert_eq!(stream.shared.frames.load(Ordering::Relaxed), 512);
        assert!(got.iter().any(|&sample| sample != T::EQUILIBRIUM), "{}: it hears the tone", T::FORMAT);
    }

    #[test]
    fn a_device_gets_the_samples_it_takes() {
        takes::<f32>();
        takes::<f64>();
        takes::<i8>();
        takes::<i16>();
        takes::<I24>();
        takes::<i32>();
        takes::<i64>();
        takes::<u8>();
        takes::<u16>();
        takes::<U24>();
        takes::<u32>();
        takes::<u64>();
    }

    #[test]
    fn a_format_nothing_writes_says_so() {
        let Err(problem) = OnDevice::open(OneFormat::new(SampleFormat::DsdU8, 2)) else {
            panic!("a stream of DSD");
        };
        assert!(problem.contains("nothing here writes"), "{problem}");
    }

    /// Longer than the buffer it converts through, in whole frames, on any number of channels.
    #[test]
    fn a_long_callback_and_other_channel_counts_come_out_whole() {
        let frames = CONVERTED_FRAMES * 2 + 77;
        let floats = tone_as_floats(frames);
        let left: Vec<f32> = floats.iter().step_by(2).copied().collect();

        let stereo = OnDevice::open(OneFormat::new(SampleFormat::I16, 2)).expect("opens");
        let got = stereo.callback::<i16>(frames);
        assert_eq!(got.len(), frames * 2);
        assert_eq!(
            &got[1000..1010],
            &floats[1000..1010].iter().map(|&s| device_sample(s)).collect::<Vec<i16>>()[..]
        );

        let mono = OnDevice::open(OneFormat::new(SampleFormat::I16, 1)).expect("opens");
        let got = mono.callback::<i16>(frames);
        assert_eq!(got.len(), frames);
        assert!(got.iter().any(|&sample| sample != 0));

        // Past the mix's two, silence: an unsigned sample's middle, not its bottom.
        let surround = OnDevice::open(OneFormat::new(SampleFormat::U16, 6)).expect("opens");
        let got = surround.callback::<u16>(frames);
        assert_eq!(got.len(), frames * 6);
        for (frame, left) in got.as_chunks::<6>().0.iter().zip(&left) {
            assert_eq!(frame[0], device_sample::<u16>(*left));
            assert_eq!(frame[2..], [u16::EQUILIBRIUM; 4]);
        }
    }

    /// A worker holding the mixer: the callback doesn't wait, and writes silence in the device's own terms.
    #[test]
    fn with_the_mixer_held_the_callback_writes_silence() {
        let stream = OnDevice::open(OneFormat::new(SampleFormat::U8, 2)).expect("opens");
        let held = stream.renderer.lock().expect("not poisoned");
        assert_eq!(stream.callback::<u8>(64), [128; 128]);
        drop(held);
        assert_ne!(stream.callback::<u8>(64), [128; 128]);
    }

    #[test]
    fn a_mix_converts_to_the_ends_of_every_format_without_overflowing() {
        assert_eq!(device_sample::<f64>(-1.0), -1.0);
        for (sample, expected) in
            [(1.0, 127), (-1.0, -128), (0.0, 0), (f32::NAN, 0), (2.0, 127), (-2.0, -128)]
        {
            assert_eq!(device_sample::<i8>(sample), expected, "{sample}");
        }
        for (sample, expected) in [(1.0, 255), (-1.0, 0), (0.0, 128), (f32::NAN, 128)] {
            assert_eq!(device_sample::<u8>(sample), expected, "{sample}");
        }
        for (sample, expected) in [(1.0, 32_767), (-1.0, -32_768), (0.0, 0), (f32::NAN, 0)] {
            assert_eq!(device_sample::<i16>(sample), expected, "{sample}");
        }
        for (sample, expected) in [(1.0, 65_535), (-1.0, 0), (0.0, 32_768), (f32::NAN, 32_768)] {
            assert_eq!(device_sample::<u16>(sample), expected, "{sample}");
        }
        // These two hold their value in a wider type: exactly 1 would land outside their range.
        for (sample, expected) in [(1.0, 8_388_607), (-1.0, -8_388_608), (0.0, 0), (f32::NAN, 0)] {
            assert_eq!(device_sample::<I24>(sample).inner(), expected, "{sample}");
        }
        for (sample, expected) in [(1.0, 16_777_215), (-1.0, 0), (0.0, 8_388_608), (f32::NAN, 8_388_608)] {
            assert_eq!(device_sample::<U24>(sample).inner(), expected, "{sample}");
        }
        assert_eq!(device_sample::<i32>(-1.0), i32::MIN);
        assert!(device_sample::<i32>(1.0) > i32::MAX - 512);
        assert_eq!(device_sample::<u32>(-1.0), 0);
        assert!(device_sample::<u32>(1.0) > u32::MAX - 512);
        assert_eq!(device_sample::<i64>(-1.0), i64::MIN);
        assert!(device_sample::<i64>(1.0) > i64::MAX / 2);
        assert_eq!(device_sample::<u64>(-1.0), 0);
        assert!(device_sample::<u64>(1.0) > u64::MAX / 2);
    }
}
