//! The room's reverb on kira (M6 U5): what each voice sends, gathered, and two Freeverbs crossfaded on a change of
//! room.
//!
//! kira sends a track's whole output, so a voice of its own can't choose its level to a reverb; and a voice moved to
//! another track would have to start again. So each voice adds its own share of what it plays into a [`RoomTap`] as
//! it's mixed, and [`RoomReverb`], the first effect of one send track, takes it in. kira mixes every sub-track before
//! any send track, so what the voices gave in a buffer is heard through the reverb in the same buffer. Both run on
//! the audio thread alone, one after the other, so the tap's atomics are never contended: they only keep its sharing
//! safe.
//!
//! With two reverbs ([`RoomHandle::set_slots`]), a new room's goes on the one not heard, retuned there, and the two
//! cross over the room's fade by equal power, a frame at a time, so nothing clicks; both take the same input all
//! along, so the new one comes in already ringing. With one, it glides to the new room's settings over the fade
//! (kira's tweens, once a buffer). Both reverbs run whether they're heard or not: that is the cost the two-reverb
//! row measures, and why one is Mobile-Low's choice if it costs too much. With no room playing, once its fade is
//! done, both ring out on silence, unheard, and then neither runs: so the next room never fades in an earlier
//! one's tail (kira's reverb has no reset).

use std::f32::consts::FRAC_PI_2;
use std::sync::Arc;
use std::sync::atomic::{AtomicU8, AtomicU32, AtomicU64, Ordering};
use std::time::Duration;

use kira::effect::reverb::{ReverbBuilder, ReverbHandle};
use kira::effect::{Effect, EffectBuilder};
use kira::info::Info;
use kira::{Decibels, Frame, Mix as Share, Tween};
use sapphire_assets::mix::Reverb;

/// What the voices send the room in one buffer: each frame's two sides, summed as each voice is mixed. Made with
/// room for kira's whole internal buffer.
pub(crate) struct RoomTap {
    frames: Box<[AtomicU64]>,
}

impl RoomTap {
    pub(crate) fn new(frames: usize) -> Self {
        Self { frames: (0..frames).map(|_| AtomicU64::new(pack(Frame::ZERO))).collect() }
    }

    /// Adds `frames`, each by a gain going in a straight line from `from` to `to` across them, as a voice's do.
    pub(crate) fn add(&self, frames: &[Frame], from: f32, to: f32) {
        let step = (to - from) / frames.len().max(1) as f32;
        let mut gain = from;
        for (frame, sum) in frames.iter().zip(self.frames.iter()) {
            gain += step;
            let was = unpack(sum.load(Ordering::Relaxed));
            sum.store(pack(was + *frame * gain), Ordering::Relaxed);
        }
    }

    /// Adds what was sent into `input`, and clears it for the next buffer.
    pub(crate) fn take_into(&self, input: &mut [Frame]) {
        for (frame, sum) in input.iter_mut().zip(self.frames.iter()) {
            *frame += unpack(sum.swap(pack(Frame::ZERO), Ordering::Relaxed));
        }
    }
}

fn pack(frame: Frame) -> u64 {
    u64::from(frame.left.to_bits()) | (u64::from(frame.right.to_bits()) << 32)
}

fn unpack(bits: u64) -> Frame {
    Frame::new(f32::from_bits(bits as u32), f32::from_bits((bits >> 32) as u32))
}

/// What the calling thread asks of the effect, read once a buffer.
struct Control {
    /// The reverb to be heard, 0 or 1.
    heard: AtomicU8,
    /// How many run, 1 or 2.
    slots: AtomicU8,
    /// Each reverb's level to come, an amplitude.
    levels: [AtomicU32; 2],
    /// Seconds a crossover or a level takes.
    fade: AtomicU32,
}

/// The calling thread's half of the room's reverb.
pub(crate) struct RoomHandle {
    reverbs: [ReverbHandle; 2],
    control: Arc<Control>,
    /// Which reverb is heard, and what each was last given.
    heard: usize,
    slots: u8,
    playing: [Option<Tuning>; 2],
    /// The room's, as last asked.
    room: Option<Tuning>,
}

/// A reverb's settings without its name, so keeping them allocates nothing.
#[derive(Clone, Copy, PartialEq)]
struct Tuning {
    feedback: f32,
    damping: f32,
    stereo_width: f32,
    volume_db: f32,
}

impl Tuning {
    fn of(reverb: &Reverb) -> Self {
        let Reverb { feedback, damping, stereo_width, volume_db, .. } = *reverb;
        Self { feedback, damping, stereo_width, volume_db }
    }
}

impl RoomHandle {
    /// Plays `reverb` as the room's, crossfaded over `fade` (see the [module docs](self)); none fades it out. A
    /// room that sounds as the one heard changes nothing.
    pub(crate) fn set_room(&mut self, reverb: Option<&Reverb>, fade: Duration) {
        self.play(reverb.map(Tuning::of), fade);
    }

    fn play(&mut self, reverb: Option<Tuning>, fade: Duration) {
        self.room = reverb;
        self.control.fade.store(fade.as_secs_f32().to_bits(), Ordering::Relaxed);
        let Some(reverb) = reverb else {
            self.control.levels[self.heard].store(0.0_f32.to_bits(), Ordering::Relaxed);
            self.playing[self.heard] = None;
            return;
        };
        if self.playing[self.heard] == Some(reverb) {
            return;
        }
        // Two cross over to the one not heard, retuned at once while it's silent; one glides there.
        let (slot, tween) = if self.slots == 2 {
            (1 - self.heard, Tween { duration: Duration::ZERO, ..Tween::default() })
        } else {
            (0, Tween { duration: fade, ..Tween::default() })
        };
        let handle = &mut self.reverbs[slot];
        handle.set_feedback(f64::from(reverb.feedback), tween);
        handle.set_damping(f64::from(reverb.damping), tween);
        handle.set_stereo_width(f64::from(reverb.stereo_width), tween);
        self.control.levels[slot]
            .store(Decibels(reverb.volume_db).as_amplitude().to_bits(), Ordering::Relaxed);
        self.playing[slot] = Some(reverb);
        self.heard = slot;
        self.control.heard.store(slot as u8, Ordering::Relaxed);
    }

    /// Runs one reverb or two from now on, the room playing as it was, crossing to the new arrangement over
    /// [`SLOTS_FADE`].
    pub(crate) fn set_slots(&mut self, slots: u8) {
        let slots = slots.clamp(1, 2);
        if slots == self.slots {
            return;
        }
        self.slots = slots;
        self.control.slots.store(slots, Ordering::Relaxed);
        // Played again on the arrangement it's now on.
        let room = self.room.take();
        self.playing = [None, None];
        if slots == 1 && self.heard == 1 {
            self.control.levels[1].store(0.0_f32.to_bits(), Ordering::Relaxed);
        }
        self.play(room, SLOTS_FADE);
    }
}

/// How long a change between one reverb and two takes to cross.
const SLOTS_FADE: Duration = Duration::from_millis(100);

/// Builds a [`RoomReverb`] that takes `tap` in, with two reverbs, silent until a room is asked.
pub(crate) struct RoomReverbBuilder {
    pub tap: Arc<RoomTap>,
}

impl EffectBuilder for RoomReverbBuilder {
    type Handle = RoomHandle;

    fn build(self) -> (Box<dyn Effect>, RoomHandle) {
        let (effect, handle) = self.parts();
        (Box::new(effect), handle)
    }
}

impl RoomReverbBuilder {
    fn parts(self) -> (RoomReverb, RoomHandle) {
        let reverb = || ReverbBuilder::new().mix(Share::WET).build();
        let ((first, first_handle), (second, second_handle)) = (reverb(), reverb());
        let control = Arc::new(Control {
            heard: AtomicU8::new(0),
            slots: AtomicU8::new(2),
            levels: [AtomicU32::new(0.0_f32.to_bits()), AtomicU32::new(0.0_f32.to_bits())],
            fade: AtomicU32::new(0.0_f32.to_bits()),
        });
        let effect = RoomReverb {
            tap: self.tap,
            control: Arc::clone(&control),
            reverbs: [first, second],
            wet: [Vec::new(), Vec::new()],
            share: 0.0,
            levels: [0.0, 0.0],
            targets: [0.0, 0.0],
            rates: [0.0, 0.0],
            quiet: [QUIET_FRAMES; 2],
            idle: true,
        };
        let handle = RoomHandle {
            reverbs: [first_handle, second_handle],
            control,
            heard: 0,
            slots: 2,
            playing: [None, None],
            room: None,
        };
        (effect, handle)
    }
}

/// The room's reverb, on the audio thread: the tap's input through one reverb or two, crossed by equal power.
pub(crate) struct RoomReverb {
    tap: Arc<RoomTap>,
    control: Arc<Control>,
    reverbs: [Box<dyn Effect>; 2],
    /// Each reverb's output, made with the mixer's buffer.
    wet: [Vec<Frame>; 2],
    /// How far over to the second reverb, 0 to 1.
    share: f32,
    /// Each reverb's level now, ...
    levels: [f32; 2],
    /// ... the one it's going to, ...
    targets: [f32; 2],
    /// ... and how far it moves a frame on the way.
    rates: [f32; 2],
    /// How many frames each reverb's output has stayed under [`RUNG_OUT`], up to [`QUIET_FRAMES`].
    quiet: [usize; 2],
    /// Whether the last buffer ran no reverb, no room playing.
    idle: bool,
}

/// A reverb's output under this is rung out: about -120 dB, below the least step any output has.
const RUNG_OUT: f32 = 1e-6;
/// How long a reverb stays under [`RUNG_OUT`] before it's taken as silent: past Freeverb's longest delay line at
/// 48 kHz, so nothing is still on its way through.
const QUIET_FRAMES: usize = 4096;

impl Effect for RoomReverb {
    fn init(&mut self, sample_rate: u32, internal_buffer_size: usize) {
        for (reverb, wet) in self.reverbs.iter_mut().zip(self.wet.iter_mut()) {
            reverb.init(sample_rate, internal_buffer_size);
            *wet = vec![Frame::ZERO; internal_buffer_size];
        }
    }

    fn on_change_sample_rate(&mut self, sample_rate: u32) {
        for reverb in &mut self.reverbs {
            reverb.on_change_sample_rate(sample_rate);
        }
    }

    fn on_start_processing(&mut self) {
        for reverb in &mut self.reverbs {
            reverb.on_start_processing();
        }
    }

    fn process(&mut self, input: &mut [Frame], dt: f64, info: &Info<'_>) {
        self.tap.take_into(input);
        let frames = input.len();
        if frames > self.wet[0].len() {
            // A buffer longer than the one it was made for, which kira never gives.
            input.fill(Frame::ZERO);
            return;
        }
        let control = &self.control;
        let two = control.slots.load(Ordering::Relaxed) == 2;
        let share_to = if two { f32::from(control.heard.load(Ordering::Relaxed).min(1)) } else { 0.0 };
        let levels_to = control.levels.each_ref().map(|level| f32::from_bits(level.load(Ordering::Relaxed)));
        let fade = f32::from_bits(control.fade.load(Ordering::Relaxed));
        // A share of the way a frame: the whole way at once for no fade.
        let step = if fade > 0.0 { dt as f32 / fade } else { 1.0 };

        // A new level is reached in a straight line over the fade, from wherever it was.
        let slots = self.targets.iter_mut().zip(&mut self.rates).zip(levels_to.iter().zip(&self.levels));
        for ((target, rate), (&to, &now)) in slots {
            if to != *target {
                *target = to;
                *rate = (to - now).abs() * step.min(1.0);
            }
        }
        let at_rest = self.share == share_to && self.levels == levels_to;
        // At rest only the reverb heard has a gain: the other keeps the level it last played at, never ramped,
        // which would bend a crossover. So no room plays once the heard one's level is none.
        let heard = usize::from(share_to > 0.0);
        let silent = at_rest && self.levels[heard] == 0.0;
        let rung_out = self.quiet.map(|quiet| quiet >= QUIET_FRAMES);
        // Nothing runs once both have rung out, so the next room never fades in an earlier one's tail.
        self.idle = silent && rung_out == [true, true];
        if silent {
            // No room is heard: the reverbs ring out on silence.
            input.fill(Frame::ZERO);
        }
        if self.idle {
            return;
        }
        // The second plays while it's heard at all, on its way out after a change to one; out of play, it rings
        // out on silence and stops.
        let both = two || self.share > 0.0;
        let reverbs =
            self.reverbs.iter_mut().zip(self.wet.iter_mut()).zip(self.quiet.iter_mut().zip(rung_out));
        for (slot, ((reverb, wet), (quiet, rung_out))) in reverbs.enumerate() {
            let plays = slot == 0 || both;
            if !plays && rung_out {
                continue;
            }
            let wet = &mut wet[..frames];
            if plays {
                wet.copy_from_slice(input);
            } else {
                wet.fill(Frame::ZERO);
            }
            reverb.process(wet, dt, info);
            let loudest =
                wet.iter().fold(0.0_f32, |most, frame| most.max(frame.left.abs()).max(frame.right.abs()));
            *quiet = if loudest > RUNG_OUT { 0 } else { (*quiet + frames).min(QUIET_FRAMES) };
        }
        if silent {
            // What rings out is heard by no room.
            return;
        }
        let (mut first, mut second) = crossover(self.share);
        for (n, frame) in input.iter_mut().enumerate() {
            if !at_rest {
                self.share = toward(self.share, share_to, step);
                for ((level, &to), &rate) in self.levels.iter_mut().zip(&levels_to).zip(&self.rates) {
                    *level = toward(*level, to, rate);
                }
                (first, second) = crossover(self.share);
            }
            let heard = self.wet[0][n] * (first * self.levels[0]);
            *frame = if both { heard + self.wet[1][n] * (second * self.levels[1]) } else { heard };
        }
    }
}

/// Each reverb's gain at `share` of the way to the second: equal power, since the two are unlike.
fn crossover(share: f32) -> (f32, f32) {
    let angle = share * FRAC_PI_2;
    (angle.cos(), angle.sin())
}

fn toward(now: f32, to: f32, step: f32) -> f32 {
    if to > now { (now + step).min(to) } else { (now - step).max(to) }
}

#[cfg(test)]
mod tests {
    use kira::info::MockInfoBuilder;

    use super::*;

    const RATE: u32 = 48_000;
    const BUFFER: usize = 128;

    fn reverb(name: &str, feedback: f32) -> Reverb {
        Reverb { name: name.to_owned(), feedback, damping: 0.5, stereo_width: 1.0, volume_db: -3.0 }
    }

    struct Rig {
        effect: RoomReverb,
        handle: RoomHandle,
        tap: Arc<RoomTap>,
    }

    impl Rig {
        fn new(slots: u8) -> Self {
            let tap = Arc::new(RoomTap::new(BUFFER));
            let (mut effect, mut handle) = RoomReverbBuilder { tap: Arc::clone(&tap) }.parts();
            effect.init(RATE, BUFFER);
            handle.set_slots(slots);
            Self { effect, handle, tap }
        }

        /// One buffer with a tone sent in: whether it ran no reverb, and its loudest sample.
        fn buffer(&mut self) -> (bool, f32) {
            let tone: Vec<Frame> =
                (0..BUFFER).map(|n| Frame::from_mono((n as f32 * 0.3).sin() * 0.25)).collect();
            self.tap.add(&tone, 1.0, 1.0);
            let mut out = vec![Frame::ZERO; BUFFER];
            self.effect.on_start_processing();
            self.effect.process(&mut out, 1.0 / f64::from(RATE), &MockInfoBuilder::new().build());
            (
                self.effect.idle,
                out.iter().map(|frame| frame.left.abs().max(frame.right.abs())).fold(0.0, f32::max),
            )
        }

        /// Buffers until a fade of `seconds` is surely done: what the last says.
        fn settle(&mut self, seconds: f32) -> (bool, f32) {
            let buffers = (seconds * RATE as f32 / BUFFER as f32) as usize + 2;
            (0..buffers).map(|_| self.buffer()).last().expect("at least one")
        }

        /// After a change to no room: the fade, then buffers until no reverb runs, each silent, with a tone still
        /// sent in. How long the reverbs took to ring out, in seconds.
        fn rings_out(&mut self, fade: Duration) -> f32 {
            assert_eq!(self.settle(fade.as_secs_f32()).1, 0.0, "nothing is heard once the fade is done");
            for buffer in 0..(10 * RATE as usize / BUFFER) {
                let (idle, loudest) = self.buffer();
                assert_eq!(loudest, 0.0, "nothing is heard while the reverbs ring out");
                if idle {
                    assert_eq!(self.buffer(), (true, 0.0), "and once rung out, it stays idle");
                    let seconds = (buffer * BUFFER) as f32 / RATE as f32;
                    #[allow(clippy::print_stdout, reason = "the measurement the hand-back reports")]
                    {
                        println!("rung out {seconds} s past the fade");
                    }
                    return seconds;
                }
            }
            panic!("the reverbs never rang out");
        }
    }

    /// M6 U5's review: whatever rooms came before, none playing runs no reverb once its fade is done and the
    /// reverbs have rung out unheard, on two reverbs, on one, and after a change from two to one.
    #[test]
    fn no_room_runs_no_reverb_once_rung_out_whatever_came_before() {
        let fade = Duration::from_millis(50);
        let (a, b) = (reverb("a", 0.8), reverb("b", 0.4));
        for slots in [2, 1] {
            let mut rig = Rig::new(slots);
            assert!(rig.buffer().0, "{slots}: idle before any room");
            rig.handle.set_room(Some(&a), fade);
            assert!(!rig.settle(0.05).0, "{slots}: A plays");
            rig.handle.set_room(Some(&b), fade);
            let (idle, loudest) = rig.settle(0.05);
            assert!(!idle && loudest > 0.0, "{slots}: B plays");
            rig.handle.set_room(None, fade);
            let seconds = rig.rings_out(fade);
            assert!(seconds < 5.0, "{slots}: A, B, then none, rung out in {seconds} s");
        }
        // Two reverbs, A on the second and B on the first, then down to one while the first is heard: the second
        // rings out unheard.
        let mut rig = Rig::new(2);
        rig.handle.set_room(Some(&a), fade);
        rig.settle(0.05);
        rig.handle.set_room(Some(&b), fade);
        rig.settle(0.05);
        rig.handle.set_slots(1);
        assert!(!rig.settle(0.2).0, "B plays on one");
        rig.handle.set_room(None, fade);
        let seconds = rig.rings_out(fade);
        assert!(seconds < 5.0, "two, then one, then none, rung out in {seconds} s");
    }
}
