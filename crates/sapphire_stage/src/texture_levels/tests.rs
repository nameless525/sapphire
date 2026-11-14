//! The streamer's choices that need no GPU: the level a footprint reads, and the hysteresis.

use super::*;

/// Follows `reads`, one estimate each (`None` for one the views didn't read), from a fresh choice for a
/// texture `above` levels above its tail, and returns the tops held after each.
fn tops(reads: impl IntoIterator<Item = Option<f32>>, above: u32) -> Vec<u32> {
    let mut choice = Choice::default();
    reads
        .into_iter()
        .map(|finest| {
            choice.follow(finest, above);
            choice.target.unwrap_or(above)
        })
        .collect()
}

/// How many times the top held changed over `tops`, after the first.
fn changes(tops: &[u32]) -> usize {
    tops.windows(2).filter(|pair| pair[0] != pair[1]).count()
}

#[test]
fn a_footprint_reads_the_level_its_texels_per_pixel_say() {
    // A 1024-texel texture at 1/256 of its UV range a pixel: 4 texels a pixel, level 2.
    assert_eq!(finest_level(1.0 / 256.0, 1024), 2.0);
    assert_eq!(level(2.0 - MARGIN, 8), 1, "the margin keeps a level finer than the estimate");
    assert_eq!(level(-3.0, 8), 0, "magnified, it needs the whole texture");
    assert_eq!(level(20.0, 8), 8, "no deeper than the tail");
    assert_eq!(finest_level(0.0, 1024), f32::NEG_INFINITY, "a pixel no wider than nothing reads it whole");
}

/// m5-plan M5: a camera resting on a level's threshold, its estimate wobbling either side of it for 120
/// frames, changes the top held once, when the texture is first read, and never again.
#[test]
fn a_camera_resting_on_a_threshold_changes_nothing() {
    // The need's threshold between levels 1 and 2 is at a finest level of 2 + MARGIN.
    let threshold = 2.0 + MARGIN;
    let wobble = (0..120).map(|frame| Some(threshold + if frame % 2 == 0 { -0.01 } else { 0.01 }));
    let held = tops(wobble, 6);
    assert_eq!(held[0], 1, "first read, it rises to the finer need at once");
    assert_eq!(changes(&held), 0, "{held:?}");
    // A wider wobble, a third of a level either way, still never lowers it.
    let wobble = (0..120).map(|frame| Some(threshold - 0.33 * ((frame as f32) * 0.7).cos()));
    assert_eq!(changes(&tops(wobble, 6)), 0);
}

/// m5-plan M5: a pan across a street and back. A texture the views leave for a second and come back to
/// keeps its top; one left for longer is lowered once, after [`LOWER_AFTER`] estimates, and raised once
/// on the way back.
#[test]
fn a_pan_across_the_street_and_back_changes_the_top_at_most_twice() {
    let pan = |away: usize| {
        let read = std::iter::repeat_n(Some(2.7), 60);
        read.clone().chain(std::iter::repeat_n(None, away)).chain(read).collect::<Vec<_>>()
    };
    let held = tops(pan(60), 6);
    assert_eq!((held[0], changes(&held)), (2, 0), "away a second, it keeps its top: {held:?}");

    let held = tops(pan(200), 6);
    assert_eq!(changes(&held), 2, "lowered once and raised once: {held:?}");
    let lowered = held.iter().position(|&top| top == 6).expect("lowered to its tail");
    assert_eq!(lowered, 60 + LOWER_AFTER as usize - 1, "lowered only after {LOWER_AFTER} estimates");
    assert_eq!(held[60 + 200], 2, "raised the first frame it's read again");
}

/// A need that eases coarser is followed late, and to the finest need of the wait; one that sharpens, at
/// once.
#[test]
fn the_top_follows_a_finer_need_at_once_and_a_coarser_one_late() {
    let mut choice = Choice::default();
    choice.follow(Some(1.6), 6);
    assert_eq!((choice.need, choice.target), (1, Some(1)));
    // The camera backs away: the need is level 3 or 4 for a while, and the top waits.
    for frame in 0..LOWER_AFTER - 1 {
        choice.follow(Some(if frame == 10 { 4.0 } else { 5.0 }), 6);
        assert_eq!(choice.target, Some(1), "frame {frame}");
    }
    choice.follow(Some(5.0), 6);
    assert_eq!(choice.target, Some(3), "lowered to the finest slack-held need of the wait");
    // A need within the slack of the top held doesn't lower it.
    let mut choice = Choice { target: Some(2), ..Choice::default() };
    for _ in 0..3 * LOWER_AFTER {
        choice.follow(Some(3.9), 6);
    }
    assert_eq!((choice.need, choice.target), (3, Some(2)));
    // Closer, at once.
    choice.follow(Some(0.2), 6);
    assert_eq!(choice.target, Some(0));
}

/// A frame's levels raised and lowered and textures started are its passes' sums: its update's and each pass
/// a capture's or a counting bench's finishing adds, started over at the next frame's update.
#[test]
fn a_frames_levels_are_the_sum_of_its_passes() {
    let mut stats =
        TextureStreamingStats { levels_raised: 9, levels_lowered: 9, started: 9, ..Default::default() };
    add_pass(&mut stats, true, [2, 1, 3]);
    add_pass(&mut stats, false, [0, 0, 0]);
    add_pass(&mut stats, false, [5, 1, 0]);
    assert_eq!((stats.levels_raised, stats.levels_lowered, stats.started), (7, 2, 3));
    add_pass(&mut stats, true, [1, 0, 0]);
    assert_eq!((stats.levels_raised, stats.levels_lowered, stats.started), (1, 0, 0));
}
