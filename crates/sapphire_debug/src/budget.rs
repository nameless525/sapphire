//! The frame budgets of spec §3 (all Proposals until M2 measures them), and the per-frame
//! numbers the app measures against them.

use std::time::Duration;

use sapphire_core::target::FormFactor;

/// The tier whose budgets the stats panel compares against: Desktop-Low on desktop, Mobile-Low
/// on phones, the columns spec §3.2 and §3.3 give.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BudgetTier {
    /// 60 fps.
    DesktopLow,
    /// 30 fps.
    MobileLow,
}

impl BudgetTier {
    pub const CURRENT: Self = match FormFactor::CURRENT {
        FormFactor::Desktop => Self::DesktopLow,
        FormFactor::Mobile => Self::MobileLow,
    };

    pub fn name(self) -> &'static str {
        match self {
            Self::DesktopLow => "Desktop-Low @ 60 fps",
            Self::MobileLow => "Mobile-Low @ 30 fps",
        }
    }

    /// The whole frame at the tier's target rate.
    pub fn frame(self) -> Duration {
        match self {
            Self::DesktopLow => Duration::from_micros(16_667),
            Self::MobileLow => Duration::from_micros(33_333),
        }
    }

    /// The CPU critical path (§3.1).
    pub fn cpu_critical_path(self) -> Duration {
        self.pick(12.0, 22.0)
    }

    /// The GPU frame (§3.3's total).
    pub fn gpu_frame(self) -> Duration {
        self.pick(13.5, 23.5)
    }

    /// TAA and upscaling (§3.3's row): temporal AA on desktop, a spatial upscaler on phones.
    pub fn taa_upscaling(self) -> Duration {
        self.pick(0.8, 1.0)
    }

    /// Bloom, tonemap and LUT (§3.3's row): post's passes, one fused pass and its bloom chain on
    /// phones, the compositing layers included (M1 P2).
    pub fn post(self) -> Duration {
        self.pick(0.7, 2.5)
    }

    /// Shadows (§3.3's row): 4 cascades on desktop, 2 on phones.
    pub fn shadows(self) -> Duration {
        self.pick(2.0, 3.0)
    }

    /// The budget of a CPU stage (§3.2) for a frame that ran `steps` fixed steps.
    pub fn stage(self, stage: CpuStage, steps: u32) -> Duration {
        match stage {
            CpuStage::Input => self.pick(0.3, 0.6),
            CpuStage::Simulation => self.pick(1.0, 1.5) * steps,
            CpuStage::Extract => self.pick(0.7, 1.2),
            CpuStage::Render => self.pick(4.0, 8.0),
        }
    }

    /// §3.2's game-side total: every simulation stage, most of which arrive in M2.
    pub fn game_side(self) -> Duration {
        self.pick(6.0, 13.6)
    }

    fn pick(self, desktop_low_ms: f64, mobile_low_ms: f64) -> Duration {
        let ms = match self {
            Self::DesktopLow => desktop_low_ms,
            Self::MobileLow => mobile_low_ms,
        };
        Duration::from_secs_f64(ms / 1000.0)
    }
}

/// The CPU stages the app measures, as spec §3.2 names them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CpuStage {
    /// Platform events, input, orchestration.
    Input,
    /// Fixed-step simulation, budgeted per 60 Hz step.
    Simulation,
    /// Presentation and extraction into the frame packet.
    Extract,
    /// Render prepare and record, through the frame graph's submit.
    Render,
}

impl CpuStage {
    pub const ALL: [Self; 4] = [Self::Input, Self::Simulation, Self::Extract, Self::Render];

    pub fn name(self) -> &'static str {
        match self {
            Self::Input => "Events, input",
            Self::Simulation => "Fixed steps",
            Self::Extract => "Extract",
            Self::Render => "Render prepare + record",
        }
    }
}

/// One frame's CPU time per stage, measured by the app.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CpuStages {
    pub input: Duration,
    pub simulation: Duration,
    /// Fixed steps the simulation ran this frame.
    pub fixed_steps: u32,
    pub extract: Duration,
    pub render: Duration,
}

impl CpuStages {
    pub fn get(&self, stage: CpuStage) -> Duration {
        match stage {
            CpuStage::Input => self.input,
            CpuStage::Simulation => self.simulation,
            CpuStage::Extract => self.extract,
            CpuStage::Render => self.render,
        }
    }

    /// Every stage the game side owns: all but render.
    pub fn game_side(&self) -> Duration {
        self.input + self.simulation + self.extract
    }

    pub fn total(&self) -> Duration {
        self.game_side() + self.render
    }

    /// An exponential moving average toward `next`, so the panel reads steadily: `weight` of
    /// the new frame (fixed steps are taken as they are).
    pub fn smoothed(&self, next: &Self, weight: f32) -> Self {
        let mix = |old: Duration, new: Duration| old.mul_f32(1.0 - weight) + new.mul_f32(weight);
        Self {
            input: mix(self.input, next.input),
            simulation: mix(self.simulation, next.simulation),
            fixed_steps: next.fixed_steps,
            extract: mix(self.extract, next.extract),
            render: mix(self.render, next.render),
        }
    }
}

/// `duration` as milliseconds with a precision that suits it: `0.042 ms`, `1.25 ms`, `16.7 ms`.
pub fn format_ms(duration: Duration) -> String {
    let ms = duration.as_secs_f64() * 1000.0;
    if ms < 1.0 {
        format!("{ms:.3} ms")
    } else if ms < 10.0 {
        format!("{ms:.2} ms")
    } else {
        format!("{ms:.1} ms")
    }
}

/// A byte count in binary units: `512 B`, `1.5 KiB`, `12.0 MiB`.
pub fn format_bytes(bytes: u64) -> String {
    const UNITS: [&str; 4] = ["KiB", "MiB", "GiB", "TiB"];
    if bytes < 1024 {
        return format!("{bytes} B");
    }
    let mut value = bytes as f64 / 1024.0;
    let mut unit = 0;
    while value >= 1024.0 && unit + 1 < UNITS.len() {
        value /= 1024.0;
        unit += 1;
    }
    format!("{value:.1} {}", UNITS[unit])
}

/// How much of `budget` `used` takes, as a fraction (1 is exactly on budget).
pub fn budget_use(used: Duration, budget: Duration) -> f32 {
    if budget.is_zero() {
        if used.is_zero() { 0.0 } else { f32::INFINITY }
    } else {
        used.as_secs_f32() / budget.as_secs_f32()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn budgets_match_spec_section_3() {
        let ms = |ms: f64| Duration::from_secs_f64(ms / 1000.0);
        let desktop = BudgetTier::DesktopLow;
        assert_eq!(desktop.stage(CpuStage::Input, 1), ms(0.3));
        assert_eq!(desktop.stage(CpuStage::Simulation, 2), ms(2.0), "budgeted per step");
        assert_eq!(desktop.stage(CpuStage::Simulation, 0), Duration::ZERO);
        assert_eq!(desktop.stage(CpuStage::Render, 1), ms(4.0));
        let mobile = BudgetTier::MobileLow;
        assert_eq!(mobile.stage(CpuStage::Simulation, 2), ms(3.0), "1.5 ms × 2 steps at 30 fps");
        assert_eq!(mobile.stage(CpuStage::Extract, 2), ms(1.2));
        assert_eq!((mobile.game_side(), mobile.gpu_frame()), (ms(13.6), ms(23.5)));
        assert_eq!((desktop.taa_upscaling(), mobile.taa_upscaling()), (ms(0.8), ms(1.0)));
        assert_eq!((desktop.post(), mobile.post()), (ms(0.7), ms(2.5)));
        assert_eq!(desktop.frame(), Duration::from_micros(16_667));
        assert_eq!(BudgetTier::CURRENT == mobile, FormFactor::CURRENT == FormFactor::Mobile);
    }

    #[test]
    fn stages_sum_and_smooth() {
        let us = Duration::from_micros;
        let stages =
            CpuStages { input: us(10), simulation: us(40), fixed_steps: 2, extract: us(20), render: us(100) };
        assert_eq!(stages.game_side(), us(70));
        assert_eq!(stages.total(), us(170));
        assert_eq!(CpuStage::ALL.map(|stage| stages.get(stage)), [us(10), us(40), us(20), us(100)]);
        let half = CpuStages::default().smoothed(&stages, 0.5);
        assert_eq!((half.render, half.fixed_steps), (us(50), 2));
    }

    #[test]
    fn formats_durations_and_bytes_readably() {
        assert_eq!(format_ms(Duration::from_micros(42)), "0.042 ms");
        assert_eq!(format_ms(Duration::from_micros(1250)), "1.25 ms");
        assert_eq!(format_ms(Duration::from_micros(16_667)), "16.7 ms");
        assert_eq!(format_bytes(512), "512 B");
        assert_eq!(format_bytes(1536), "1.5 KiB");
        assert_eq!(format_bytes(12 * 1024 * 1024), "12.0 MiB");
        assert_eq!(format_bytes(u64::MAX), "16777216.0 TiB", "TiB is the largest unit");
        assert_eq!(budget_use(Duration::from_millis(3), Duration::from_millis(4)), 0.75);
        assert_eq!(budget_use(Duration::ZERO, Duration::ZERO), 0.0);
        assert!(budget_use(Duration::from_millis(1), Duration::ZERO).is_infinite());
    }
}
