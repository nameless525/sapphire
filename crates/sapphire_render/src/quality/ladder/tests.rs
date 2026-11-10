//! The ladder on modelled traces: heat rising, heat that comes and goes, recovery, and a GPU that
//! can't hold its budget, the last in a loop with dynamic resolution.

use std::time::Duration;

use sapphire_core::thermal::{ThermalReading, ThermalState};

use super::*;
use crate::quality::dynamic::{DynamicResolution, FrameTiming};
use crate::quality::{QualityTier, TierTable};

fn tier(id: TierId) -> QualityTier {
    TierTable::shipped().get(id).clone()
}

fn reading(state: ThermalState) -> ThermalReading {
    ThermalReading { state, ..ThermalReading::default() }
}

fn secs(seconds: f64) -> Duration {
    Duration::from_secs_f64(seconds)
}

/// Every move of a run: when, and to what depth.
type Moves = Vec<(Duration, usize, Cause)>;

/// Runs `ladder` from `from` to `to` seconds, 10 updates a second, with heat and pressure from
/// `trace(seconds)`, and returns its moves.
fn run(ladder: &mut Ladder, from: f64, to: f64, trace: impl Fn(f64) -> (ThermalState, Pressure)) -> Moves {
    let mut moves = Vec::new();
    let mut tenths = (from * 10.0).round() as u64;
    while (tenths as f64) < to * 10.0 {
        let now = Duration::from_millis(tenths * 100);
        let (state, pressure) = trace(tenths as f64 / 10.0);
        if let Some(step) = ladder.update(now, &reading(state), pressure, None) {
            assert_eq!(step.to.depth, ladder.state().depth);
            moves.push((now, step.to.depth, step.cause));
        }
        tenths += 1;
    }
    moves
}

fn depths(moves: &Moves) -> Vec<usize> {
    moves.iter().map(|&(_, depth, _)| depth).collect()
}

#[test]
fn the_shipped_ladders_spend_what_the_look_can_spare_first() {
    use OptionalPass::{BloomLevel, Gtao, SkyFaceRate};
    // Desktops: 0.85, the optional passes, the floor, 30 fps, and Desktop-High a tier.
    let desktop = [Rung::Scale(0.85), Rung::Shed(vec![SkyFaceRate, Gtao, BloomLevel]), Rung::Scale(0.7)];
    let high = tier(TierId::DesktopHigh);
    assert_eq!(high.ladder[..3], desktop);
    assert_eq!(high.ladder[3..], [Rung::FrameRate(30), Rung::Tier(TierId::DesktopLow)]);
    assert_eq!(tier(TierId::DesktopLow).ladder[..3], desktop);
    assert_eq!(tier(TierId::DesktopLow).ladder[3..], [Rung::FrameRate(30)]);
    // Phones: the passes first, then 0.85 and 0.75. Mobile-Low has no GTAO, already runs at 30 and
    // is the floor, and its dynamic resolution stops at 0.85, leaving 0.75 to the ladder.
    let mobile_high = tier(TierId::MobileHigh);
    assert_eq!(
        mobile_high.ladder,
        [
            Rung::Shed(vec![SkyFaceRate, BloomLevel, Gtao]),
            Rung::Scale(0.85),
            Rung::Scale(0.75),
            Rung::FrameRate(30),
            Rung::Tier(TierId::MobileLow),
        ]
    );
    let mobile_low = tier(TierId::MobileLow);
    assert_eq!(
        mobile_low.ladder,
        [Rung::Shed(vec![SkyFaceRate, BloomLevel]), Rung::Scale(0.85), Rung::Scale(0.75)]
    );
    assert_eq!(mobile_low.resolution.dynamic_floor, 0.85);
    assert_eq!(mobile_high.resolution.dynamic_floor, 0.75);

    for tier in TierTable::shipped().tiers() {
        let bottom = LadderState::of(&tier.ladder);
        // Each rung holds the ones above it.
        for depth in 1..=tier.ladder.len() {
            let (above, here) =
                (LadderState::of(&tier.ladder[..depth - 1]), LadderState::of(&tier.ladder[..depth]));
            assert!(
                here.scale <= above.scale && here.shed.iter().count() >= above.shed.iter().count(),
                "{}",
                tier.id
            );
        }
        // Every pass sheds in place, and shedding them all touches nothing but what may be shed:
        // `optional_passes_shed_one_step_and_nothing_else` holds the rest of the settings.
        assert!(bottom.shed.iter().all(OptionalPass::in_place), "{}", tier.id);
        assert!(bottom.scale >= 0.7, "{}: no rung below T0's floors", tier.id);
    }
}

/// A change to one tier's ladder, and what its check says.
type BadLadder = (TierId, fn(&mut QualityTier), &'static str);

#[test]
fn bad_ladders_are_refused() {
    use TierId::*;
    let cases: [BadLadder; 8] = [
        (DesktopLow, |tier| tier.ladder[2] = Rung::Scale(0.9), "scales must fall"),
        (DesktopLow, |tier| tier.ladder[0] = Rung::Scale(0.1), "scales must fall"),
        (DesktopLow, |tier| tier.ladder[1] = Rung::Shed(vec![OptionalPass::Cirrus]), "only a new renderer"),
        (MobileLow, |tier| tier.ladder.push(Rung::Shed(vec![OptionalPass::BloomLevel])), "twice"),
        (MobileLow, |tier| tier.ladder.push(Rung::FrameRate(30)), "frame rate"),
        (DesktopLow, |tier| tier.ladder.push(Rung::Tier(MobileLow)), "a tier must be lower"),
        (MobileHigh, |tier| tier.ladder.swap(3, 4), "a tier must be lower"),
        (MobileHigh, |tier| tier.ladder.insert(1, Rung::Shed(vec![])), "sheds nothing"),
    ];
    for (id, edit, problem) in cases {
        let mut tier = tier(id);
        edit(&mut tier);
        let error = validate(&tier).expect_err(problem);
        assert!(error.contains(problem), "{id}: {error}");
        assert!(tier.validate().is_err());
    }
    let mut long = tier(MobileLow);
    long.ladder = (0..9).map(|step| Rung::Scale(0.9 - step as f32 * 0.05)).collect();
    assert!(validate(&long).is_err());
}

/// Google's thresholds (developer.android.com/games/optimize/adpf/thermal, read 2026-11-10): 0.85
/// "reduce workload if possible", 0.95 "reduce workload immediately", 1.0 throttling at SEVERE.
#[test]
fn heat_reads_the_state_the_forecast_and_low_power() {
    use ThermalState::*;
    let with = |state, headroom: Option<f32>, low_power| ThermalReading {
        state,
        headroom,
        hint_session: None,
        low_power,
    };
    assert_eq!(heat(&ThermalReading::default()), Nominal, "a desktop");
    assert_eq!(heat(&with(Moderate, None, None)), Moderate);
    assert_eq!(heat(&with(Nominal, Some(0.84), None)), Nominal);
    assert_eq!(heat(&with(Nominal, Some(0.85), None)), Light);
    assert_eq!(heat(&with(Nominal, Some(0.90), None)), Light, "the emulator's constant forecast");
    assert_eq!(heat(&with(Nominal, Some(0.95), None)), Moderate);
    assert_eq!(heat(&with(Light, Some(1.2), None)), Severe);
    assert_eq!(heat(&with(Critical, Some(0.5), None)), Critical, "the state when it says more");
    assert_eq!(heat(&with(Nominal, None, Some(true))), Severe, "Low Power Mode");
    assert_eq!(heat(&with(Nominal, None, Some(false))), Nominal);
}

/// A phone heating over five minutes: each state takes the ladder straight to its depth, and
/// `Critical` asks for the lower tier.
#[test]
fn heating_takes_each_state_straight_to_its_depth() {
    use ThermalState::*;
    let mut ladder = Ladder::new(&tier(TierId::MobileHigh));
    let states = [Nominal, Light, Moderate, Severe, Critical];
    let moves = run(&mut ladder, 0.0, 300.0, |t| (states[(t / 60.0) as usize], Pressure::Unknown));
    assert_eq!(
        moves,
        [
            (secs(60.0), 1, Cause::Heat(Light)),
            (secs(120.0), 2, Cause::Heat(Moderate)),
            (secs(180.0), 4, Cause::Heat(Severe)),
            (secs(240.0), 5, Cause::Heat(Critical)),
        ]
    );
    let bottom = ladder.state();
    assert_eq!((bottom.scale, bottom.frame_rate, bottom.tier), (0.75, Some(30), Some(TierId::MobileLow)));
    assert_eq!(ladder.tier_request(), Some(TierId::MobileLow));
    // Severe stops short of the tier: it caps the frame rate and keeps the renderer.
    let mut severe = Ladder::new(&tier(TierId::MobileHigh));
    run(&mut severe, 0.0, 10.0, |_| (Severe, Pressure::Unknown));
    assert_eq!((severe.state().depth, severe.tier_request()), (4, None));
}

/// Cooling from the bottom: a rung at a time, the tier after 60 s, the frame-rate cap after 60 s
/// more, each other rung after 10 s.
#[test]
fn recovery_takes_each_rung_back_after_its_hold() {
    use ThermalState::*;
    let mut ladder = Ladder::new(&tier(TierId::MobileHigh));
    run(&mut ladder, 0.0, 30.0, |_| (Critical, Pressure::Unknown));
    let moves = run(&mut ladder, 30.0, 300.0, |_| (Nominal, Pressure::Unknown));
    assert_eq!(
        moves,
        [
            (secs(90.0), 4, Cause::Calm),
            (secs(150.0), 3, Cause::Calm),
            (secs(160.0), 2, Cause::Calm),
            (secs(170.0), 1, Cause::Calm),
            (secs(180.0), 0, Cause::Calm),
        ]
    );
    assert_eq!(ladder.state(), LadderState::TOP);
    // A GPU without slack holds every rung, however cool the device.
    let mut full = Ladder::new(&tier(TierId::DesktopLow));
    run(&mut full, 0.0, 5.0, |_| (Light, Pressure::Unknown));
    assert_eq!(full.state().depth, 1);
    assert!(run(&mut full, 5.0, 600.0, |_| (Nominal, Pressure::Within(0.9))).is_empty());
}

/// Heat that comes and goes doesn't make the ladder flap: one too quick to outlast a hold never
/// brings a rung back, and one that does stretches the holds until it can't.
#[test]
fn heat_that_comes_and_goes_does_not_flap() {
    use ThermalState::*;
    // Light 2 s, Nominal 2 s, for ten minutes: down once and held.
    let mut ladder = Ladder::new(&tier(TierId::MobileLow));
    let flicker = run(&mut ladder, 0.0, 600.0, |t| {
        (if ((t / 2.0) as u64).is_multiple_of(2) { Light } else { Nominal }, Pressure::Unknown)
    });
    assert_eq!(depths(&flicker), [1]);

    // Light 5 s in every 23: the first cool spell outlasts the 10 s hold, the next can't outlast 20.
    let mut ladder = Ladder::new(&tier(TierId::MobileLow));
    let waves =
        run(&mut ladder, 0.0, 600.0, |t| (if t % 23.0 < 5.0 { Light } else { Nominal }, Pressure::Unknown));
    assert_eq!(depths(&waves), [1, 0, 1], "{waves:?}");
    assert_eq!(ladder.stats().backoff, 2);
    // Five minutes at the top once the heat stops: the holds are their own length again.
    run(&mut ladder, 600.0, 950.0, |_| (Nominal, Pressure::Unknown));
    assert_eq!((ladder.state().depth, ladder.stats().backoff), (0, 1));

    // The device swinging between Moderate and Severe holds the cap for as long as it does.
    let mut ladder = Ladder::new(&tier(TierId::DesktopHigh));
    let swing = run(&mut ladder, 0.0, 600.0, |t| {
        (if ((t / 20.0) as u64).is_multiple_of(2) { Severe } else { Moderate }, Pressure::Unknown)
    });
    assert_eq!(depths(&swing), [4], "the cap's 60 s hold outlasts every cool spell");
}

/// A GPU in a loop with dynamic resolution: time per frame is a fixed part plus a part per scene
/// pixel, GTAO shed saves 13% of the latter (T0: 12–14% of Desktop-Low's frame), and the frame-rate
/// cap stretches the budget. The ladder reads only what the controller says.
struct Gpu {
    controller: DynamicResolution,
    tier: QualityTier,
    frame: u64,
    timing: FrameTiming,
    times: Vec<Duration>,
}

impl Gpu {
    const PIXELS: f32 = 1920.0 * 1080.0;

    fn new(tier: QualityTier) -> Self {
        let mut controller = DynamicResolution::new(tier.resolution_budget(), true);
        controller.set_enabled(true);
        Self {
            controller,
            tier,
            frame: 1,
            timing: FrameTiming { frame: 1, ..FrameTiming::default() },
            times: vec![Duration::ZERO],
        }
    }

    /// Runs frames at the tier's capped rate from `from` to `to` seconds under `load` (a multiple
    /// of a frame of 1 ms fixed and 13 ms of pixels at scale 1), and returns the ladder's moves.
    fn run(&mut self, ladder: &mut Ladder, from: f64, to: f64, load: impl Fn(f64) -> f32) -> Moves {
        let mut moves = Vec::new();
        let mut now = from;
        while now < to {
            let state = ladder.state();
            let rate = state.frame_rate.unwrap_or(self.tier.frame_rate);
            let pressure = self.controller.pressure();
            if let Some(step) = ladder.update(secs(now), &ThermalReading::default(), pressure, None) {
                moves.push((secs(now), step.to.depth, step.cause));
                if step.to.frame_rate != step.from.frame_rate {
                    let rate = step.to.frame_rate.unwrap_or(self.tier.frame_rate);
                    self.controller.set_budget(self.tier.resolution_budget_at(rate));
                }
            }
            let state = ladder.state();
            self.controller.set_ceiling(state.scale);
            let factor = self.controller.update(&self.timing, Self::PIXELS);
            let shed = if state.shed.contains(OptionalPass::Gtao) { 0.87 } else { 1.0 };
            let ms = load(now) * (1.0 + 13.0 * factor * factor * shed);
            self.times.push(Duration::from_secs_f32(ms / 1000.0));
            // GPU timings come back two frames late.
            let measured = self.frame.checked_sub(1).filter(|&frame| frame >= 1);
            self.frame += 1;
            self.timing = FrameTiming {
                frame: self.frame,
                gpu: measured.map(|frame| (frame, self.times[frame as usize])),
                frame_time: Duration::ZERO,
            };
            now += 1.0 / f64::from(rate);
        }
        moves
    }
}

/// Dynamic resolution at Desktop-Low's floor and over budget presses the ladder a rung at a time,
/// skipping the scales it already renders under; once the load falls the rungs come back.
#[test]
fn pressure_takes_the_rungs_dynamic_resolution_cannot() {
    let tier = tier(TierId::DesktopLow);
    let mut ladder = Ladder::new(&tier);
    let mut gpu = Gpu::new(tier);
    // 11.2 ms at scale 1, inside the 14 ms budget: nothing moves.
    assert!(gpu.run(&mut ladder, 0.0, 60.0, |_| 0.8).is_empty());
    assert_eq!(gpu.controller.factor(), 1.0);
    // Twice that: 16.2 ms at the 0.7 floor. The passes go (skipping 0.85), still 14.2 ms, then the
    // cap (skipping 0.7), whose 28 ms frame holds it.
    let overload = gpu.run(&mut ladder, 60.0, 300.0, |_| 2.2);
    assert_eq!(depths(&overload), [2, 4], "{overload:?}");
    assert!(overload.iter().all(|&(_, _, cause)| cause == Cause::Pressure));
    assert!(overload[1].0 - overload[0].0 >= PRESSURE_DWELL);
    assert!(overload[1].0 < secs(70.0), "within ten seconds of the load: {overload:?}");
    assert_eq!(ladder.state().frame_rate, Some(30));
    // The load falls back: the cap after 60 s, then a rung each 10 s, back to the top.
    let calm = gpu.run(&mut ladder, 300.0, 600.0, |_| 0.8);
    assert_eq!(depths(&calm), [3, 2, 1, 0], "{calm:?}");
    assert!(calm[0].0 >= secs(360.0) && calm[3].0 < secs(420.0), "{calm:?}");
    assert_eq!(ladder.state(), LadderState::TOP);
    assert_eq!(gpu.controller.factor(), 1.0);
}

/// A load that fits only with the passes shed: the ladder sheds them and stays, since a GPU at its
/// floor without slack holds every rung, rather than bringing them back to press again.
#[test]
fn a_load_that_fits_one_rung_down_stays_there() {
    let tier = tier(TierId::DesktopLow);
    let mut ladder = Ladder::new(&tier);
    let mut gpu = Gpu::new(tier);
    // 14.7 ms at the 0.7 floor, 13.4 ms with GTAO shed.
    let moves = gpu.run(&mut ladder, 0.0, 600.0, |_| 2.0);
    assert_eq!(depths(&moves), [2], "{moves:?}");
    let Pressure::Within(share) = gpu.controller.pressure() else {
        panic!("{:?}", gpu.controller.pressure())
    };
    assert!(share > LOW_WATER, "no room for GTAO back: {share}");
}

/// Mobile-Low's 0.75 is a rung under its dynamic floor: heat holds the scale there, with dynamic
/// resolution on or off.
#[test]
fn a_scale_under_the_floor_holds_the_scale_there() {
    let tier = tier(TierId::MobileLow);
    let mut controller = DynamicResolution::new(tier.resolution_budget(), true);
    let timing = FrameTiming { frame: 1, ..FrameTiming::default() };
    controller.set_ceiling(0.75);
    assert_eq!(controller.update(&timing, 1.0e6), 0.75, "off, at the ceiling");
    controller.set_enabled(true);
    assert_eq!(controller.update(&timing, 1.0e6), 0.75, "on, held under its 0.85 floor");
    controller.set_ceiling(1.0);
    controller.set_enabled(false);
    assert_eq!(controller.update(&timing, 1.0e6), 1.0);
}

/// A lowered tier starts where the one it left stopped, climbs back, and once calm at its top for a
/// minute asks for the tier it came from.
#[test]
fn a_lowered_tier_climbs_back_and_asks_for_the_tier_it_left() {
    use ThermalState::*;
    let mut high = Ladder::new(&tier(TierId::MobileHigh));
    run(&mut high, 0.0, 1.0, |_| (Critical, Pressure::Unknown));
    assert_eq!(high.tier_request(), Some(TierId::MobileLow));
    // The game takes it at a pause, still hot.
    let mut low = Ladder::entered(&tier(TierId::MobileLow), &high, Some(TierId::MobileHigh), secs(5.0));
    assert_eq!(low.state(), LadderState::of(&tier(TierId::MobileLow).ladder));
    assert_eq!(low.tier_request(), None);
    assert!(run(&mut low, 5.0, 20.0, |_| (Critical, Pressure::Unknown)).is_empty());
    // Cool: each rung after 10 s, then a minute at the top before it asks.
    let moves = run(&mut low, 20.0, 200.0, |_| (Nominal, Pressure::Unknown));
    assert_eq!(depths(&moves), [2, 1, 0]);
    assert_eq!(moves[2].0, secs(50.0));
    assert_eq!(low.tier_request(), Some(TierId::MobileHigh));
    // Warm again and it stops asking.
    run(&mut low, 200.0, 201.0, |_| (Light, Pressure::Unknown));
    assert_eq!(low.tier_request(), None);
}

#[test]
fn forced_depths_hold_whatever_the_heat() {
    let mut ladder = Ladder::new(&tier(TierId::DesktopHigh));
    let step = ladder.update(secs(0.0), &reading(ThermalState::Critical), Pressure::Unknown, Some(2));
    assert_eq!(step.map(|step| (step.to.depth, step.cause)), Some((2, Cause::Forced)));
    assert_eq!(ladder.update(secs(1.0), &reading(ThermalState::Critical), Pressure::Unknown, Some(2)), None);
    // Past the bottom is the bottom.
    ladder.update(secs(2.0), &ThermalReading::default(), Pressure::Unknown, Some(99));
    assert_eq!(ladder.state().depth, 5);
    // Let go, a cool device climbs as usual.
    let moves = run(&mut ladder, 3.0, 300.0, |_| (ThermalState::Nominal, Pressure::Unknown));
    assert_eq!(depths(&moves), [4, 3, 2, 1, 0]);
}
