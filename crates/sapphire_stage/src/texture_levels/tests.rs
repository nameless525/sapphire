//! The streamer's choices that need no GPU: the level a footprint reads, and the hysteresis.

use sapphire_render::TextureCoverage;

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

/// Two textures alike for [`TextureLevels::plan`]: 128 texels square, RGBA8, five levels above their 4 × 4 tails.
const A: AssetId = AssetId::from_bytes([0xa0; 16]);
const B: AssetId = AssetId::from_bytes([0xb0; 16]);
const ABOVE: u32 = 5;

fn whole() -> TextureDesc {
    use sapphire_assets::texture::{Swizzle, TextureFormat};
    let format = TextureFormat::Rgba8UnormSrgb;
    TextureDesc { format, width: 128, height: 128, mip_level_count: 8, swizzle: Swizzle::default() }
}

/// What the views read of a 128-texel texture whose finest level is `finest`.
fn read(texture: AssetId, finest: f32) -> TextureCoverage {
    TextureCoverage { texture, uv_per_pixel: finest.exp2() / 128.0 }
}

/// A and B, bound and streaming within a pool with room for one whole and the other a level down, and not for
/// both whole.
struct Pair {
    levels: TextureLevels,
    tops: [u32; 2],
    streaming: TextureStreaming,
    frame: u64,
}

impl Pair {
    fn new() -> Self {
        let mut levels = TextureLevels::default();
        levels.bind(A, false);
        levels.bind(B, false);
        let whole = whole();
        let pool = bytes_from(&whole, 0) + bytes_from(&whole, 1) + whole.level_byte_len(0) / 2;
        Self { levels, tops: [ABOVE; 2], streaming: TextureStreaming { pool, raise: u64::MAX }, frame: 0 }
    }

    /// One frame's plan from an estimate reading A and B at their finest levels, and the renderer's half as
    /// `follow` does it: a lower to the plan at once, and a rise to it as soon as its levels may start (they
    /// come at once here). Returns the tops after it.
    fn frame(&mut self, a: f32, b: f32) -> [u32; 2] {
        self.frame += 1;
        let reads = [read(A, a), read(B, b)];
        let coverage = TextureCoverageFrame { frame: self.frame, textures: &reads };
        let tops = self.tops;
        let shape =
            |id: AssetId| Some(Shape { whole: whole(), top: tops[usize::from(id == B)], above: ABOVE });
        self.levels.plan(shape, coverage, self.streaming, u64::MAX, 0);
        for (top, id) in self.tops.iter_mut().zip([A, B]) {
            let choice = self.levels.textures[&id].choice;
            if (choice.lower && *top < choice.planned) || (choice.start.is_some() && choice.planned < *top) {
                *top = choice.planned;
            }
        }
        // What the plan holds stays within the pool.
        let held: u64 = self.tops.iter().map(|&top| bytes_from(&whole(), top)).sum();
        assert!(held <= self.streaming.pool, "frame {}: {:?} holds {held}", self.frame, self.tops);
        self.tops
    }
}

/// m5-plan M5's hard pool, held still (Z3 review F2): A and B alike, the pool too small for both whole, and A's
/// need wobbling across a level every frame (an idle enemy's posed bounds) while B's stays whole. The pool's
/// plan lowers one of them once, and that one stays lowered: neither swaps its levels with the other each
/// frame, nor when the lowered one tries its target again after `LOWER_AFTER` estimates.
#[test]
fn a_need_wobbling_across_a_level_under_a_binding_pool_swaps_no_levels() {
    let mut pair = Pair::new();
    // A's finest level either side of 1 + MARGIN: a need of 0, then of 1. B's, well under: always 0.
    let wobble = |frame: u32| if frame.is_multiple_of(2) { 1.45 } else { 1.55 };
    for frame in 0..10 {
        pair.frame(wobble(frame), 0.2);
    }
    let settled = pair.tops;
    assert!(settled.contains(&0) && settled.contains(&1), "one whole and one a level down: {settled:?}");
    let tops: Vec<[u32; 2]> =
        (10..10 + 2 * LOWER_AFTER).map(|frame| pair.frame(wobble(frame), 0.2)).collect();
    for texture in 0..2 {
        let changes = tops.windows(2).filter(|pair| pair[0][texture] != pair[1][texture]).count();
        assert!(changes <= 1, "texture {texture}'s top changed {changes} times: {tops:?}");
    }
    // What the memory costs a texture.
    assert_eq!(size_of::<Option<PoolLowered>>(), 12);
}

/// What the pool lowered rises again once there's room: here once A leaves the renderer's materials, within
/// `LOWER_AFTER` estimates, as the memory holding it down lapses.
#[test]
fn a_texture_the_pool_lowered_rises_once_the_pool_has_room() {
    let mut pair = Pair::new();
    for _ in 0..10 {
        pair.frame(0.2, 0.2);
    }
    let lowered = usize::from(pair.tops[1] == 1);
    assert_eq!(pair.tops[lowered], 1, "one of them is a level down: {:?}", pair.tops);
    let other = [A, B][1 - lowered];
    pair.levels.unbind(other, false);
    let frames = (0..LOWER_AFTER + 2).position(|_| pair.frame(0.2, 0.2)[lowered] == 0);
    assert!(frames.is_some(), "it never rose: {:?}", pair.tops);
    assert_eq!(pair.tops[1 - lowered], ABOVE, "the one let go of is at its tail");
}

/// A texture held whole goes last, also as the pool's plan gives back what it lowered and remembers it: with a held
/// texture A that must go a level down and a streamed one B whose every level above its tail fits in the room that
/// leaves, B stays at its tail while A is down, also when the pool lowered A before B was first read. Before, B rose into that room past A, so the pool held a texture held
/// whole below its target in place of a streamed one, and the texture view (which shows only streamed
/// textures) showed no debt at all (`debug-textures-district`).
#[test]
fn a_streamed_texture_never_rises_past_one_held_whole_the_pool_lowered() {
    let mut levels = TextureLevels::default();
    levels.bind(A, true);
    levels.bind(B, false);
    let whole = whole();
    // A whole beside B at its tail doesn't fit; A a level down leaves room for B whole, twice over.
    let pool = bytes_from(&whole, 0) + bytes_from(&whole, ABOVE) - 1;
    let streaming = TextureStreaming { pool, raise: u64::MAX };
    let mut tops = [ABOVE; 2];
    for frame in 1..=2 * u64::from(LOWER_AFTER) {
        // The first estimate reads A alone, so B's plan is its tail and the pool lowers A: a plan it remembered
        // would keep A down once B is read, in B's place.
        let reads = [read(A, 0.2), read(B, 0.2)];
        let coverage = TextureCoverageFrame { frame, textures: &reads[..if frame == 1 { 1 } else { 2 }] };
        let now = tops;
        let shape = |id: AssetId| Some(Shape { whole, top: now[usize::from(id == B)], above: ABOVE });
        levels.plan(shape, coverage, streaming, u64::MAX, 0);
        for (top, id) in tops.iter_mut().zip([A, B]) {
            let choice = levels.textures[&id].choice;
            if (choice.lower && *top < choice.planned) || (choice.start.is_some() && choice.planned < *top) {
                *top = choice.planned;
            }
        }
        let planned = [A, B].map(|id| levels.textures[&id].choice.planned);
        if frame > 1 {
            assert!(planned[0] > 0, "frame {frame}: A can't be whole: {planned:?}");
            assert_eq!(
                planned[1], ABOVE,
                "frame {frame}: B is finer than its tail with A, held whole, lowered: {planned:?}"
            );
        }
    }
}

/// A surplus goes before a need (M5 Z2): A and B both held whole, the pool with room for both, then A's need a
/// level coarser (its top still held whole by hysteresis, within `LOWER_AFTER` estimates) and the pool cut to
/// one whole and one a level down. The plan takes A's surplus, which costs nothing on screen, rather than put B
/// under its need, though they tie on everything else and B loses ties.
#[test]
fn a_binding_pool_lowers_a_surplus_before_a_texture_under_its_need() {
    let mut pair = Pair::new();
    let whole = whole();
    pair.streaming.pool = 2 * bytes_from(&whole, 0);
    for _ in 0..10 {
        assert_eq!(pair.frame(0.2, 0.2), [0, 0], "both fit whole");
    }
    // A's finest level past 1 + MARGIN + LOWER_SLACK: a need of 1, which hysteresis doesn't follow yet.
    pair.frame(2.1, 0.2);
    assert_eq!(pair.levels.textures[&A].choice.target, Some(0), "A's top is still held whole");
    pair.streaming.pool = bytes_from(&whole, 0) + bytes_from(&whole, 1);
    let tops = pair.frame(2.1, 0.2);
    let planned = [A, B].map(|id| pair.levels.textures[&id].choice.planned);
    assert_eq!(planned, [1, 0], "A goes to its need, B stays at its own");
    assert_eq!(tops, [1, 0]);
}

/// A texture the stage hasn't estimated yet isn't waiting (M5 Z2): its need is its tail until the views have
/// read it, so the first update of a run, before any frame is drawn, opens no wait for it, of either kind.
#[test]
fn an_update_with_no_estimate_opens_no_wait() {
    let mut levels = TextureLevels::default();
    levels.bind(A, false);
    let shape = |_| Some(Shape { whole: whole(), top: ABOVE, above: ABOVE });
    // No frame recorded yet: the estimate's frame is the one already seen.
    let coverage = TextureCoverageFrame { frame: 0, textures: &[] };
    let streaming = TextureStreaming { pool: u64::MAX, raise: u64::MAX };
    levels.plan(shape, coverage, streaming, u64::MAX, 0);
    let choice = &mut levels.textures.get_mut(&A).expect("bound").choice;
    assert_eq!(choice.need, ABOVE, "unread, it needs its tail");
    let (need, pool_held) = (choice.need, choice.planned > choice.need);
    let mut stats = TextureStreamingStats::default();
    count_wait(choice, &mut stats, ABOVE, need, pool_held);
    assert_eq!(stats.all_waits(), TextureWaits::default(), "no wait opened");
}

/// The pool is the server's (M5 Z2): what the server's textures' pool holds that the streamer doesn't (a tail
/// still loading, a level let go of that the server can't unload yet) takes the room first, so a level starts
/// only if it fits beside everything the server holds; a lower frees room only for levels the server can give
/// back now; and a pool too small for what the streamer can't lower is counted, not exceeded silently.
#[test]
fn a_level_starts_only_with_room_beside_everything_the_server_holds() {
    let whole = whole();
    let start = |outside: u64, pool: u64| {
        let mut levels = TextureLevels::default();
        levels.bind(A, false);
        let shape = |_| Some(Shape { whole, top: ABOVE, above: ABOVE });
        let reads = [read(A, 0.2)];
        let coverage = TextureCoverageFrame { frame: 1, textures: &reads };
        let streaming = TextureStreaming { pool, raise: u64::MAX };
        // The server holds A's tail, as the streamer counts it, and `outside` it doesn't.
        levels.plan(shape, coverage, streaming, u64::MAX, bytes_from(&whole, ABOVE) + outside);
        (levels.textures[&A].choice.start.is_some(), levels.stats.tails_over_pool)
    };
    let pool = bytes_from(&whole, 0);
    assert_eq!(start(0, pool), (true, 0), "with nothing else held, A's levels fit");
    let level = whole.level_byte_len(ABOVE - 1);
    assert_eq!(start(pool - bytes_from(&whole, ABOVE) - level + 1, pool), (false, 0), "no room for a level");
    assert_eq!(start(pool, pool), (false, 1), "the tail alone is over what's left: counted");
}

/// A trip's wipe holds for the first view's textures (M5 Z2): while a streamed texture is short of its plan the
/// arrival isn't over; once it's there, or with streaming off, or a pool too small for the tails, it is. Under
/// the wipe the levels raise at the wipe's budget, and at the tier's once the zone has arrived.
#[test]
fn an_arrival_waits_for_the_first_views_textures_at_their_plan() {
    let whole = whole();
    let mut levels = TextureLevels::default();
    levels.bind(A, false);
    let streaming = TextureStreaming { pool: u64::MAX, raise: 4 << 20 };
    levels.set_streaming(Some(streaming));
    let reads = [read(A, 0.2)];
    let coverage = TextureCoverageFrame { frame: 1, textures: &reads };
    let at = |top| move |_| Some(Shape { whole, top, above: ABOVE });
    levels.plan(at(ABOVE), coverage, streaming, u64::MAX, 0);
    assert_eq!(levels.textures[&A].choice.planned, 0, "the view reads A whole");
    assert!(!levels.at_plan(at(ABOVE)), "at its tail, A is short of its plan: the wipe holds");
    assert!(!levels.at_plan(at(1)), "a level short still holds it");
    assert!(levels.at_plan(at(0)), "at its plan, the arrival may end");
    levels.stats.tails_over_pool = 1;
    assert!(
        levels.at_plan(at(ABOVE)),
        "a pool too small for the tails never settles: it doesn't hold the wipe"
    );
    levels.stats.tails_over_pool = 0;
    levels.set_streaming(None);
    assert!(levels.at_plan(at(ABOVE)), "streaming off holds nothing");
    assert_eq!(raise_budget(streaming, true), ARRIVAL_RAISE, "the wipe's budget");
    assert_eq!(raise_budget(streaming, false), 4 << 20, "play's");
}

/// One update's count of a texture for [`wait`], with the open waits started over as `count` does.
fn count_wait(
    choice: &mut Choice,
    stats: &mut TextureStreamingStats,
    top: u32,
    required: u32,
    pool_held: bool,
) {
    count_wait_arriving(choice, stats, top, required, pool_held, false);
}

/// [`count_wait`], while a zone arrives or not.
fn count_wait_arriving(
    choice: &mut Choice,
    stats: &mut TextureStreamingStats,
    top: u32,
    required: u32,
    pool_held: bool,
    arriving: bool,
) {
    for waits in [&mut stats.waits, &mut stats.pool_waits] {
        (waits.open, waits.longest_open) = (0, 0);
    }
    (stats.arrival.open, stats.arrival.longest_open) = (0, 0);
    wait(choice, top, required, pool_held, arriving, stats);
}

/// The opening is an arrival (M5 Z2): a wait opened while a zone arrives is counted apart, in neither kind's
/// histogram nor the reasons, however long it runs past the arrival's end; one opened after is judged as ever.
#[test]
fn a_wait_opened_before_the_zone_arrived_is_counted_apart() {
    let mut stats = TextureStreamingStats::default();
    let (mut before, mut after) = (Choice::default(), Choice::default());
    count_wait_arriving(&mut before, &mut stats, 5, 5, false, true);
    count_wait_arriving(&mut before, &mut stats, 5, 2, false, true);
    assert_eq!((stats.arrival.open, stats.all_waits().open), (1, 0), "open, and the arrival's");
    // The zone arrives; the wait opened before it goes on 40 frames and ends.
    for _ in 0..40 {
        count_wait_arriving(&mut before, &mut stats, 5, 2, true, false);
    }
    count_wait_arriving(&mut before, &mut stats, 2, 2, false, false);
    assert_eq!((stats.arrival.completed, stats.arrival.long), (1, 1));
    assert_eq!(stats.all_waits(), TextureWaits::default(), "in neither histogram");
    assert_eq!(stats.reasons, WaitReasons::default(), "nor the reasons");
    // One opened after the arrival is a surface newly in view.
    count_wait_arriving(&mut after, &mut stats, 5, 5, false, false);
    for _ in 0..3 {
        count_wait_arriving(&mut after, &mut stats, 5, 2, false, false);
    }
    count_wait_arriving(&mut after, &mut stats, 2, 2, false, false);
    assert_eq!((stats.waits.completed(), stats.waits.worst(), stats.arrival.completed), (1, Some(3), 1));
    // And the texture that waited for the arrival, needed finer again later, waits as any other.
    count_wait_arriving(&mut before, &mut stats, 2, 0, false, false);
    count_wait_arriving(&mut before, &mut stats, 0, 0, false, false);
    assert_eq!((stats.waits.completed(), stats.arrival.completed), (2, 1));
    start_waits_over(&mut stats);
    assert_eq!(stats.arrival, ArrivalWaits::default(), "they start over with the rest");
}

/// A bench's arrival ends at the first update the first view's textures are at their plan: before the views
/// have read a texture, it isn't (its tail is only for want of a reading), and once ended it stays ended.
#[test]
fn a_benchs_arrival_ends_once_the_first_views_textures_are_at_their_plan() {
    let whole = whole();
    let mut levels = TextureLevels::default();
    levels.bind(A, false);
    let streaming = TextureStreaming { pool: u64::MAX, raise: 4 << 20 };
    levels.set_streaming(Some(streaming));
    levels.open_as_arrival();
    let at = |top| move |_| Some(Shape { whole, top, above: ABOVE });
    levels.plan(at(ABOVE), TextureCoverageFrame { frame: 0, textures: &[] }, streaming, u64::MAX, 0);
    assert!(!levels.at_plan(at(ABOVE)), "not read yet: not at its plan");
    let reads = [read(A, 0.2)];
    levels.plan(at(ABOVE), TextureCoverageFrame { frame: 1, textures: &reads }, streaming, u64::MAX, 0);
    assert!(levels.opening() && levels.at_plan(at(0)));
    levels.end_arrival();
    assert!(!levels.opening());
    assert!(levels.stats.arrival.arrived_at > 0, "the update it arrived at");
}

/// M5 Z2 (a zone trip's wipe held for ever): a texture on the renderer that no view reads, a material's slot
/// nothing draws, is at its plan, its tail, from the views' first estimate, so it never holds an arrival.
#[test]
fn a_texture_no_view_reads_is_at_its_plan_once_the_views_are_estimated() {
    let whole = whole();
    let mut levels = TextureLevels::default();
    levels.bind(A, false);
    let streaming = TextureStreaming { pool: u64::MAX, raise: 4 << 20 };
    levels.set_streaming(Some(streaming));
    let at = |top| move |_| Some(Shape { whole, top, above: ABOVE });
    levels.plan(at(ABOVE), TextureCoverageFrame { frame: 0, textures: &[] }, streaming, u64::MAX, 0);
    assert!(!levels.at_plan(at(ABOVE)), "not estimated yet: not at its plan");
    for frame in 1..=3 {
        levels.plan(at(ABOVE), TextureCoverageFrame { frame, textures: &[] }, streaming, u64::MAX, 0);
        assert!(
            levels.at_plan(at(ABOVE)),
            "estimated at frame {frame} and read by no view: at its tail's plan"
        );
    }
    assert_eq!(levels.textures[&A].choice.need, ABOVE, "it needs its tail, so it waits for nothing");
}

/// M5 Z2: an arrival waits for its textures at most [`ARRIVAL_HOLD`] updates; one that hasn't reached its plan
/// by then is let go, counted once in [`ArrivalWaits::held_to_bound`]; one that reaches it first isn't counted.
#[test]
fn an_arrival_holds_its_wipe_no_longer_than_its_bound() {
    let whole = whole();
    let mut levels = TextureLevels::default();
    levels.bind(A, false);
    let streaming = TextureStreaming { pool: u64::MAX, raise: 4 << 20 };
    levels.set_streaming(Some(streaming));
    let at = |top| move |_| Some(Shape { whole, top, above: ABOVE });
    let reads = [read(A, 0.2)];
    levels.plan(at(ABOVE), TextureCoverageFrame { frame: 1, textures: &reads }, streaming, u64::MAX, 0);
    // Its levels never come: a fault, which the bound lets go.
    for update in 1..ARRIVAL_HOLD {
        levels.step_arrival(true, at(ABOVE));
        assert!(!levels.at_plan(at(ABOVE)), "held at update {update}");
    }
    levels.step_arrival(true, at(ABOVE));
    assert!(levels.at_plan(at(ABOVE)), "let go at the bound");
    assert_eq!(levels.stats.arrival.held_to_bound, 1);
    levels.step_arrival(true, at(ABOVE));
    assert_eq!(levels.stats.arrival.held_to_bound, 1, "counted once an arrival");
    // Arrived, the next arrival starts its count afresh, and one at its plan in time isn't counted.
    levels.step_arrival(false, at(ABOVE));
    assert!(!levels.at_plan(at(ABOVE)), "a new arrival holds again");
    for _ in 0..ARRIVAL_HOLD {
        levels.step_arrival(true, at(0));
    }
    assert!(levels.at_plan(at(0)));
    assert_eq!(levels.stats.arrival.held_to_bound, 1, "at its plan within the bound");
}

/// m5-plan exit test 2's waits: a texture settled at its tail is needed two levels finer, and its levels
/// come `k` updates later; the wait lands in bin `k`, the last bin for [`WAIT_BINS`] - 1 or more. One the pool
/// held at any update of its wait is the pool's kind; one that hasn't ended shows as open with its length.
#[test]
fn a_wait_lands_in_the_bin_of_the_frames_it_took() {
    let mut stats = TextureStreamingStats::default();
    let last = WAIT_BINS - 1;
    let after = last as u32 + 200;
    for (k, bin) in [(0, 0), (1, 1), (31, 31), (32, 32), (last as u32, last), (after, last)] {
        let mut choice = Choice::default();
        count_wait(&mut choice, &mut stats, 5, 5, false);
        for _ in 0..k {
            count_wait(&mut choice, &mut stats, 5, 2, false);
        }
        assert_eq!((stats.waits.open, stats.waits.longest_open), (u32::from(k > 0), k), "k = {k}");
        count_wait(&mut choice, &mut stats, 2, 2, false);
        assert_eq!(stats.waits.frames[bin], 1 + u32::from(k == after), "k = {k}");
        assert_eq!(stats.waits.open, 0);
        // Settled, it waits no more.
        count_wait(&mut choice, &mut stats, 2, 2, false);
    }
    assert_eq!(stats.waits.completed(), 6);
    assert_eq!(stats.pool_waits, TextureWaits::default(), "the pool held none");

    // The pool held it for one update of its wait: the pool's kind, open while it lasts.
    let mut choice = Choice::default();
    count_wait(&mut choice, &mut stats, 5, 5, false);
    count_wait(&mut choice, &mut stats, 5, 2, true);
    for _ in 0..499 {
        count_wait(&mut choice, &mut stats, 5, 2, false);
    }
    assert_eq!((stats.pool_waits.open, stats.pool_waits.longest_open), (1, 500), "a starved one shows");
    assert_eq!(stats.waits.open, 0);
    count_wait(&mut choice, &mut stats, 2, 2, false);
    assert_eq!((stats.pool_waits.completed(), stats.pool_waits.worst()), (1, Some(500)));
    assert_eq!(stats.waits.completed(), 6, "the other kind's unchanged");

    // Needed coarser, or as it holds, it isn't waiting.
    let mut choice = Choice::default();
    count_wait(&mut choice, &mut stats, 2, 2, false);
    count_wait(&mut choice, &mut stats, 2, 4, false);
    count_wait(&mut choice, &mut stats, 4, 4, false);
    assert_eq!(stats.waits.completed(), 6);
}

/// A completed wait's reason is what held it longest (M5 Z2): the pool at any frame makes it the pool's, of the
/// views-over kind if the views wanted more than the pool then; else the loads' reason with the most frames. A
/// wait over `LONG_WAIT` frames counts as long too.
#[test]
fn a_waits_reason_is_what_held_it_longest() {
    let mut stats = TextureStreamingStats::default();
    let mut waited = |held_by: [u16; 3], held: (bool, bool), frames: u32| {
        let mut choice = Choice::default();
        count_wait(&mut choice, &mut stats, 2, 2, false);
        for _ in 0..frames {
            count_wait(&mut choice, &mut stats, 5, 2, held.0);
        }
        (choice.held_by, choice.held_over) = (held_by, held.1);
        count_wait(&mut choice, &mut stats, 2, 2, false);
    };
    waited([3, 1, 0], (false, false), 4);
    waited([1, 5, 0], (false, false), 40);
    waited([0, 2, 2], (false, false), 4);
    waited([0, 0, 3], (false, false), 3);
    waited([9, 0, 0], (true, false), 9);
    waited([0, 0, 0], (true, true), 31);
    let [start, loading, raise, fit, over] = WaitReason::ALL.map(|reason| reason as usize);
    let reasons = stats.reasons;
    assert_eq!([reasons.waited[start], reasons.waited[loading], reasons.waited[raise]], [1, 2, 1]);
    assert_eq!([reasons.waited[fit], reasons.waited[over]], [1, 1]);
    assert_eq!(reasons.long, [0, 1, 0, 0, 1], "40 and 31 frames are over 30");
    start_waits_over(&mut stats);
    assert_eq!(stats.reasons, WaitReasons::default(), "they start over with the waits");
}

/// The percentile of completed waits at the bins' edges.
#[test]
fn a_percentile_of_waits_reads_its_bin() {
    let mut waits = TextureWaits::default();
    assert_eq!((waits.percentile(0.5), waits.worst()), (None, None), "no waits");
    waits.frames[0] = 98;
    waits.frames[31] = 1;
    waits.frames[WAIT_BINS - 1] = 1;
    let last = Some(WAIT_BINS as u32 - 1);
    assert_eq!(waits.percentile(0.5), Some(0));
    assert_eq!(waits.percentile(0.98), Some(0), "the 98th of 100 is the last in bin 0");
    assert_eq!(waits.percentile(0.99), Some(31));
    assert_eq!(waits.percentile(1.0), last, "the last bin's frames or more");
    assert_eq!(waits.percentile(0.0), Some(0), "the first");
    assert_eq!(waits.worst(), last);
    let one =
        TextureWaits { frames: std::array::from_fn(|bin| u32::from(bin == 7)), ..TextureWaits::default() };
    assert_eq!((one.percentile(0.01), one.percentile(0.99), one.worst()), (Some(7), Some(7), Some(7)));
}

/// Exit test 2 reads both kinds together: 98 quick waits for loads and two long ones the pool held put the
/// 99th percentile in the pool's.
#[test]
fn every_wait_counts_whichever_kind_it_is() {
    let mut stats = TextureStreamingStats::default();
    stats.waits.frames[2] = 98;
    stats.pool_waits.frames[WAIT_BINS - 1] = 2;
    (stats.waits.open, stats.waits.longest_open) = (3, 7);
    (stats.pool_waits.open, stats.pool_waits.longest_open) = (1, 90);
    let all = stats.all_waits();
    assert_eq!(all.completed(), 100);
    let last = Some(WAIT_BINS as u32 - 1);
    assert_eq!((all.percentile(0.5), all.percentile(0.99), all.worst()), (Some(2), last, last));
    assert_eq!(stats.waits.percentile(0.99), Some(2), "the loads' kind alone would flatter it");
    assert_eq!((all.open, all.longest_open), (4, 90));
}

/// Starting over empties what completed before it; a wait open across it lands after it, whole.
#[test]
fn a_wait_open_across_a_start_over_lands_after_it_whole() {
    let mut stats = TextureStreamingStats::default();
    let (mut done, mut open) = (Choice::default(), Choice::default());
    count_wait(&mut done, &mut stats, 5, 5, false);
    count_wait(&mut open, &mut stats, 5, 5, false);
    for _ in 0..3 {
        wait(&mut done, 5, 2, false, false, &mut stats);
        count_wait(&mut open, &mut stats, 5, 2, false);
    }
    wait(&mut done, 2, 2, false, false, &mut stats);
    count_wait(&mut open, &mut stats, 5, 2, true);
    assert_eq!((stats.waits.completed(), stats.pool_waits.open), (1, 1));
    start_waits_over(&mut stats);
    assert_eq!(stats.all_waits().completed(), 0, "what completed before is gone");
    assert_eq!((stats.pool_waits.open, stats.pool_waits.longest_open), (1, 4), "the open wait goes on");
    for _ in 0..6 {
        count_wait(&mut open, &mut stats, 5, 2, false);
    }
    count_wait(&mut open, &mut stats, 2, 2, false);
    assert_eq!(stats.all_waits().completed(), 1);
    assert_eq!(stats.pool_waits.frames[10], 1, "with its whole length: {:?}", stats.pool_waits.frames);
}
