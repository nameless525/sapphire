//! `frames.csv`: a second of play a row, with the OS's heat reading (the crate's docs have the format).

use std::fmt::Write as _;
use std::time::Duration;

use sapphire_core::thermal::{ThermalReading, ThermalState};

/// The file's first line: the columns, in order.
pub const FRAMES_HEADER: &str = "at,run,tick,frames,mean_ms,max_ms,late,budget_ms,thermal,headroom,low_power";

/// One second of play, or a little more: a row closes with the frame that takes it past a second.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FrameRow {
    /// When it closed, on the session's clock, in seconds.
    pub at: f64,
    /// The run standing as it closed, and its tick.
    pub run: u32,
    pub tick: u64,
    pub frames: u32,
    /// The frames' mean and longest times, in milliseconds.
    pub mean_ms: f32,
    pub max_ms: f32,
    /// Frames longer than one and a half of `budget_ms`: a vsync missed, where vsync paces the frames.
    pub late: u32,
    /// The interval the tier paces its frames to, in milliseconds.
    pub budget_ms: f32,
    /// The OS's heat reading as it closed. The hint session isn't kept.
    pub heat: ThermalReading,
}

/// A row of `frames.csv` that doesn't read.
#[derive(Debug, thiserror::Error)]
#[error("frames.csv line {line}: {reason}")]
pub struct FramesError {
    pub line: usize,
    pub reason: String,
}

impl FrameRow {
    /// Appends this row, with its line's end, to `out`.
    pub(crate) fn write(&self, out: &mut String) {
        let optional = |value: Option<String>| value.unwrap_or_default();
        let _ = writeln!(
            out,
            "{:.3},{},{},{},{:.2},{:.2},{},{:.2},{:?},{},{}",
            self.at,
            self.run,
            self.tick,
            self.frames,
            self.mean_ms,
            self.max_ms,
            self.late,
            self.budget_ms,
            self.heat.state,
            optional(self.heat.headroom.map(|headroom| format!("{headroom:.3}"))),
            optional(self.heat.low_power.map(|on| on.to_string())),
        );
    }

    /// Every row of a `frames.csv`'s text. A last line with no end is a write a power cut stopped, and is
    /// left out; any other line that doesn't read is an error.
    pub fn read_all(text: &str) -> Result<Vec<Self>, FramesError> {
        let mut lines = text.split_inclusive('\n').enumerate();
        match lines.next() {
            Some((_, header)) if header.trim_end() == FRAMES_HEADER => {}
            _ => return Err(FramesError { line: 1, reason: format!("the header isn't {FRAMES_HEADER:?}") }),
        }
        let mut rows = Vec::new();
        for (index, line) in lines {
            let Some(line) = line.strip_suffix('\n') else { break };
            let row = Self::parse(line)
                .map_err(|reason| FramesError { line: index + 1, reason: reason.to_owned() })?;
            rows.push(row);
        }
        Ok(rows)
    }

    fn parse(line: &str) -> Result<Self, &'static str> {
        let fields: Vec<&str> = line.split(',').collect();
        let [at, run, tick, frames, mean_ms, max_ms, late, budget_ms, state, headroom, low_power] =
            fields[..]
        else {
            return Err("it doesn't have 11 columns");
        };
        let number = |text: &str| text.parse::<f64>().map_err(|_| "a number doesn't read");
        let count = |text: &str| text.parse::<u64>().map_err(|_| "a count doesn't read");
        let state = ThermalState::ALL
            .into_iter()
            .find(|candidate| format!("{candidate:?}") == state)
            .ok_or("the thermal state isn't one of ThermalState's")?;
        let headroom = match headroom {
            "" => None,
            text => Some(number(text)? as f32),
        };
        let low_power = match low_power {
            "" => None,
            "true" => Some(true),
            "false" => Some(false),
            _ => return Err("low_power is neither true, false nor empty"),
        };
        Ok(Self {
            at: number(at)?,
            run: u32::try_from(count(run)?).map_err(|_| "the run's number is too big")?,
            tick: count(tick)?,
            frames: u32::try_from(count(frames)?).map_err(|_| "too many frames")?,
            mean_ms: number(mean_ms)? as f32,
            max_ms: number(max_ms)? as f32,
            late: u32::try_from(count(late)?).map_err(|_| "too many late frames")?,
            budget_ms: number(budget_ms)? as f32,
            heat: ThermalReading { state, headroom, hint_session: None, low_power },
        })
    }
}

/// The second a session is in the middle of: frames add to it until it's a second long, then it's a row.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct OpenSecond {
    /// On the session's clock.
    length: Duration,
    /// The frames' own times, summed.
    took: Duration,
    frames: u32,
    longest: Duration,
    late: u32,
}

impl OpenSecond {
    /// Adds a frame that stood for `stands` of the session's clock and took `frame` against `budget`: the same,
    /// but in a capture, which stands for a tick a frame however long it takes to draw. Returns the row it
    /// closes, if it does: at `at`, with `run`, `tick` and `heat` as they stand.
    pub(crate) fn add(
        &mut self,
        (stands, frame): (Duration, Duration),
        budget: Duration,
        heat: ThermalReading,
        at: f64,
        (run, tick): (u32, u64),
    ) -> Option<FrameRow> {
        self.length += stands;
        self.took += frame;
        self.frames += 1;
        self.longest = self.longest.max(frame);
        if frame.as_secs_f64() > budget.as_secs_f64() * 1.5 {
            self.late += 1;
        }
        if self.length < Duration::from_secs(1) {
            return None;
        }
        let ms = |duration: Duration| (duration.as_secs_f64() * 1e3) as f32;
        let row = FrameRow {
            at,
            run,
            tick,
            frames: self.frames,
            mean_ms: ms(self.took) / self.frames as f32,
            max_ms: ms(self.longest),
            late: self.late,
            budget_ms: ms(budget),
            heat,
        };
        *self = Self::default();
        Some(row)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn heat() -> ThermalReading {
        ThermalReading {
            state: ThermalState::Moderate,
            headroom: Some(0.9),
            hint_session: Some(true),
            low_power: None,
        }
    }

    #[test]
    fn a_second_of_frames_is_a_row_that_reads_back() {
        let mut second = OpenSecond::default();
        let budget = Duration::from_micros(16_667);
        let mut closed = None;
        for frame in 0..60 {
            // Every tenth frame misses a vsync.
            let time = if frame % 10 == 9 { budget * 2 } else { budget };
            closed = closed.or(second.add((time, time), budget, heat(), 1.0, (2, 300 + frame)));
        }
        let row = closed.expect("past a second");
        assert_eq!((row.frames, row.late, row.run), (55, 5, 2), "the row closes on the frame past a second");
        assert!((row.max_ms - 33.33).abs() < 0.01);

        let mut text = format!("{FRAMES_HEADER}\n");
        row.write(&mut text);
        let read = FrameRow::read_all(&text).unwrap();
        assert_eq!(read.len(), 1);
        assert_eq!(read[0].heat.state, ThermalState::Moderate);
        assert_eq!(read[0].heat.headroom, Some(0.9));
        assert_eq!((read[0].frames, read[0].late, read[0].tick), (55, 5, row.tick));
    }

    #[test]
    fn a_torn_last_line_is_left_out_and_a_bad_one_is_an_error() {
        let mut text = format!("{FRAMES_HEADER}\n");
        let row = FrameRow {
            at: 1.0,
            run: 1,
            tick: 60,
            frames: 60,
            mean_ms: 16.6,
            max_ms: 17.0,
            late: 0,
            budget_ms: 16.67,
            heat: ThermalReading::default(),
        };
        row.write(&mut text);
        text.push_str("2.0,1,120,6");
        assert_eq!(FrameRow::read_all(&text).unwrap().len(), 1);
        text.push('\n');
        assert!(FrameRow::read_all(&text).is_err());
        assert!(FrameRow::read_all("at,run\n").is_err());
    }
}
