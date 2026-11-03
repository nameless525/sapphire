//! What a frame of a timeline costs: `cargo run --release -p sapphire_sequencer --example sequencer_bench`.
//!
//! A timeline well past any cutscene's size (a minute, 32 camera keys, 8 characters with 16 plays and places
//! each, 256 cues, 4 post tracks, 64 subtitles), a frame asking everything it can.

#![allow(clippy::print_stdout, reason = "a bench's output is what it's for")]

use std::fmt::Write as _;
use std::sync::Arc;
use std::time::Duration;

use glam::Vec3;
use sapphire_core::Name;
use sapphire_look::PostParams;
use sapphire_sequencer::{Origin, TimelinePlayer};
use sapphire_time::Stopwatch;
use sapphire_timeline::TimelineDocument;

const LENGTH: f32 = 60.0;
const FRAMES: u32 = 60 * 60;

fn big_document() -> String {
    let mut text = String::new();
    let _ = write!(
        text,
        r#"(type: "sapphire::timeline::TimelineDocument", version: 1, data: (length: {LENGTH:.1}, clock: Real, camera: Some((keys: ["#
    );
    for key in 0..32 {
        let at = key as f32 * LENGTH / 32.0;
        let _ = write!(
            text,
            "(at: {at:.3}, eye: ({at:.1}, 1.6, -4.0), look_at: (0.0, 1.4, 0.0), fov_degrees: 35.0, ease: Smooth),"
        );
    }
    text.push_str("])), characters: [");
    for character in 0..8 {
        let _ = write!(text, r#"(role: "role_{character}", plays: ["#);
        for play in 0..16 {
            let (from, to) = (play as f32 * LENGTH / 16.0, (play as f32 + 0.8) * LENGTH / 16.0);
            let _ = write!(text, r#"(row: "row_{play}", from: {from:.3}, to: {to:.3}),"#);
        }
        text.push_str("], places: [");
        for place in 0..16 {
            let at = place as f32 * LENGTH / 16.0;
            let _ = write!(text, "(at: {at:.3}, position: ({at:.1}, 0.0, 0.0)),");
        }
        text.push_str("]),");
    }
    text.push_str("], cues: [");
    for cue in 0..256 {
        let at = cue as f32 * LENGTH / 256.0;
        let _ = write!(text, r#"(at: {at:.3}, cue: "cue_{cue}"),"#);
    }
    text.push_str("], post: [");
    for parameter in ["Exposure", "Bloom", "Vignette", "Diffusion"] {
        let _ = write!(text, "(parameter: {parameter}, keys: [");
        for key in 0..32 {
            let (at, value) = (key as f32 * (LENGTH - 1.0) / 32.0, 0.25);
            let _ = write!(text, "(at: {at:.3}, value: {value}),");
        }
        let _ = write!(text, "(at: {LENGTH:.1}, value: 0.0)]),");
    }
    text.push_str("], subtitles: [");
    for line in 0..64 {
        let (from, to) = (line as f32 * LENGTH / 64.0, (line as f32 + 0.9) * LENGTH / 64.0);
        let _ = write!(text, r#"(from: {from:.3}, to: {to:.3}, key: "line-{line}"),"#);
    }
    text.push_str("]))");
    text
}

fn main() {
    let document = big_document();
    let timeline = Arc::new(TimelineDocument::from_ron(&document).expect("reads").cook().expect("cooks"));
    let cooked = timeline.to_cooked().expect("writes").len();
    let origin = Origin { position: Vec3::new(4.0, 0.0, 0.0), yaw: 1.0 };
    let roles: Vec<Name> = (0..8).map(|index| Name::new(&format!("role_{index}"))).collect();

    let mut frames = Vec::with_capacity(FRAMES as usize);
    let mut sink = 0.0_f32;
    for _ in 0..4 {
        let mut player = TimelinePlayer::new(timeline.clone(), origin);
        for _ in 0..FRAMES {
            let stopwatch = Stopwatch::start();
            player.advance(1.0 / 60.0, None);
            sink += player.shot().map_or(0.0, |shot| shot.eye.x);
            let mut post = PostParams::default();
            player.apply_post(&mut post);
            sink += post.exposure;
            sink += player.subtitle().map_or(0.0, |line| line.key.len() as f32);
            for &role in &roles {
                sink += player.character(role).map_or(0.0, |cue| cue.position_ticks as f32);
            }
            sink += player.fired().map(|cue| cue.place.x).sum::<f32>();
            frames.push(stopwatch.elapsed());
        }
    }
    frames.sort_unstable();
    let percentile = |share: f64| frames[((frames.len() - 1) as f64 * share) as usize];
    let mean = frames.iter().sum::<Duration>() / frames.len() as u32;
    println!(
        "a frame of a timeline ({} bytes cooked): mean {mean:?}, p50 {:?}, p99 {:?}, longest {:?} ({sink})",
        cooked,
        percentile(0.5),
        percentile(0.99),
        percentile(1.0),
    );
}
