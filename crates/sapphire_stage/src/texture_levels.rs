//! Bringing streamed textures to the top the stage wants of them, a level at a time (M5 M4, m5-plan §3
//! "Mip streaming").
//!
//! A large texture is cooked as a tail and a level asset for each level above it
//! (`sapphire_assets::texture`'s "Tails"). What loads a texture as a dependency, a material, loads its tail
//! alone, so a material is ready, and draws, once its textures' tails are. [`TextureLevels`] is what the
//! stage then does for each texture on the renderer:
//!
//! - **What it wants.** A top for each texture: the one asked for ([`crate::Content::want_texture_top`],
//!   M5's seam) or, until something asks, level 0, the whole texture, while a material on the renderer
//!   binds it; its tail's own top once none does, so its levels go back and its tail can go too.
//! - **Raising.** It loads the levels between the texture's top and the wanted one through the asset
//!   server, holding their handles, keeps each as it arrives ([`sapphire_assets::Upload::keep`]), and,
//!   once the levels just above the texture's top have come, raises it by them, several at once as one new
//!   texture ([`Renderer::raise_texture`]), as the renderer's budget for the frame allows
//!   ([`Renderer::may_raise`]). The renderer rebinds the materials that hold it.
//! - **Lowering.** A wanted top under the texture's lowers it at once ([`Renderer::lower_texture`]), and
//!   the levels it dropped go back to the server ([`AssetServer::unload`]): a level is uploaded once, so
//!   one the server still held as uploaded would never come again. The rule both ways is that a level is
//!   on the GPU, or kept on its way there, exactly while this holds a handle to it, and a level the server
//!   lets go of takes the texture under it.
//! - **Hot reload.** A level's new version is written over the one on the GPU
//!   ([`Renderer::rewrite_texture_level`]); a tail's of the same shape is written in place by the stage, and
//!   one of another shape makes a new texture, whose levels are asked for again ([`TextureLevels::reshaped`]).
//!
//! It runs at the end of [`crate::Content::update`], after the frame's uploads, within the renderer's
//! budget for raising, which is the renderer's own: the asset server's upload budget admits a level's bytes
//! to the frame, and this one its copy into a new texture. A capture, which must see every texture whole,
//! finishes them before each frame ([`crate::Content::finish_texture_levels`]).
//!
//! # The streamer (M5 M5)
//!
//! With streaming on ([`crate::Content::set_texture_streaming`]) the top wanted of each texture comes from
//! what the views draw rather than from its being bound:
//!
//! - **What the views read.** The renderer's estimate ([`Renderer::texture_coverage`]) bounds how finely
//!   the last frame read each texture; the finest level it can read, less [`MARGIN`], is the texture's
//!   need. With every need resident a frame is the frame with whole textures. A texture a material binds
//!   in a slot that never streams ([`sapphire_render::texture_streams`]: ramps, the face's shadow map,
//!   masks, matcaps, the sky's maps), or one held whole ([`crate::Content::keep_textures_whole`]: the
//!   party, an inset's or a timeline's framed characters), needs level 0. One the views didn't read needs
//!   only its tail.
//! - **Raise promptly, lower lazily.** The top held follows a finer need at once; it follows a coarser one
//!   only once the need, less a further [`LOWER_SLACK`] of a level, has stayed coarser for [`LOWER_AFTER`]
//!   estimates, and then to the finest of them. A camera resting on a level's threshold changes nothing.
//! - **The pool is hard.** What the textures hold, tails and levels, on the GPU and on their way, never
//!   passes the pool ([`TextureStreaming::pool`]). Where the tops held would, the streamer plans coarser
//!   ones, a level at a time, lowering first what's wanted least: a texture the views stopped reading, the
//!   longest unseen first; then a top held finer than its need, the widest surplus first, which costs nothing
//!   on screen, so no texture goes under its need while another holds more than its own (M5 Z2); then the one
//!   lowered least below its top held, so blur spreads a level at a time;
//!   then, of two alike, the one it lowered last frame; then the one read most coarsely (by its top held,
//!   never the need, which follows every estimate); then the largest. A texture held whole goes last. What
//!   it can't afford stays wanted, as debt.
//! - **The pool's plan remembers.** A texture the pool held coarser than its top stays at that plan: it
//!   rises only while the pool keeps room for another such level beside the plan, and only once every texture
//!   it ranks as more wanted has risen (one held whole goes last, also here), or once [`LOWER_AFTER`]
//!   estimates have passed and it tries its top again, losing ties to those it didn't lower. So a need
//!   wobbling across a level under a binding pool never swaps two textures' levels from frame to frame. Its
//!   levels still go back to the server as it lowers: one the server kept would be found ready and never
//!   handed over again, and keeping it here would take the pool's room as the GPU's level did.
//! - **A frame's work is bounded.** Each frame starts loading the levels of at most [`MAX_STARTS`]
//!   textures and the tier's raise budget in bytes ([`TextureStreaming::raise`], within the server's upload
//!   budget), the widest gap between top and need first, then the nearest; one held whole goes first and at
//!   a high priority, outside the count. It raises within the same budget (the renderer's
//!   [`Renderer::set_mip_budget`]) and lowers at most [`MAX_LOWERS`] textures, the greatest rank first; a
//!   level starts only if it fits the pool beside what the textures hold once the frame's lowers are done,
//!   so lowers that wait their turn never take the pool past its bound.
//! - **What it costs** is the streamer's work and the renderer's estimate, which the stage asks for at the
//!   top of its loading ([`Renderer::estimate_texture_coverage`]): both are timed
//!   ([`TextureStreamingStats::time`], [`TextureStreamingStats::coverage_time`]), against spec §3.2's
//!   streaming row.
//! - **Debt.** A texture is in debt while its top is coarser than its need; its debt is the frames it has
//!   been, which [`TextureStreamingStats`] reports with the levels raised and lowered and the bytes held.
//! - **Waits** ([`TextureWaits`], m5-plan exit test 2). A wait starts when a texture is needed finer than it
//!   held at the last update and ends when its debt is paid; its length in frames goes to a fixed histogram
//!   (0 to 31, and 32 or more) where the debt resets. The exit test reads every wait
//!   ([`TextureStreamingStats::all_waits`]): a surface the pool holds coarser is blurred all the same. Each
//!   wait has a kind that says why it was long: the pool held it at some frame
//!   ([`TextureStreamingStats::pool_waits`]), or it waited only for its levels
//!   ([`TextureStreamingStats::waits`]). Waits still open are counted at each update with the longest of
//!   them, so a texture starved for good shows. The histograms add up from the renderer's first update or
//!   the last [`crate::Content::start_texture_waits_over`] (the bench calls it at each phase's start); a wait
//!   open across it lands after it, with its whole length. They take no bytes a texture beyond a flag and
//!   the top last counted, and a branch a texture an update.
//! - **A capture that streams** ([`crate::CaptureTextures::Settled`]) brings each texture to the plan for the
//!   frame drawn before, unbudgeted, before it draws: what a player sees once streaming settles.

use std::collections::{BTreeMap, HashMap};

use sapphire_assets::texture::TextureDesc;
use sapphire_assets::texture_level::{ArchivedTextureLevel, TextureLevel};
use sapphire_assets::{
    AssetId, AssetServer, AssetSlot, CpuResident, Handle, LoadPriority, LoadState, Upload, UploadError,
};
use sapphire_render::texture::GpuTexture;
use sapphire_render::{DEFAULT_MIP_BUDGET, Renderer, StreamedTops, TextureCoverageFrame};
use sapphire_time::Stopwatch;

use crate::LOG_TARGET;
use crate::content::Held;

/// Levels taken off the estimate's finest level before it's a need: room for the GPU's own rounding of
/// the level it reads, so the picture is the whole textures' (the readback tests).
pub(crate) const MARGIN: f32 = 0.5;

/// A further share of a level a need must be coarser by before the top held is lowered to it.
pub(crate) const LOWER_SLACK: f32 = 0.5;

/// Estimates a need must stay coarser than the top held before it's lowered: a second and a half at 60
/// frames a second.
pub(crate) const LOWER_AFTER: u32 = 90;

/// Textures a frame may start loading levels for, at most.
pub(crate) const MAX_STARTS: usize = 16;

/// Textures a frame may lower, at most: a lower makes a texture and records its copies, about 3 µs on an M5,
/// so a frame's lowers stay near 0.1 ms, and the 300 a jump into a full pool asks for take ten frames. The
/// rest wait their turn, the most wanted lowered first, and what the pool can't hold yet doesn't start.
pub(crate) const MAX_LOWERS: usize = 32;

/// How the streamer chooses ([`crate::Content::set_texture_streaming`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TextureStreaming {
    /// Bytes the textures may hold on the GPU and on their way, tails and levels: the tier's zone texture
    /// pool (spec §3.4, `texture_pool_mib`), unless a run sets another.
    pub pool: u64,
    /// Bytes of levels a frame may write raising textures, and start loading: the tier's
    /// `texture_raise_kib`. The first raise of a frame goes whatever its size.
    pub raise: u64,
}

/// What the streamer did last frame and holds (rule R9), from [`crate::Content::texture_streaming_stats`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TextureStreamingStats {
    /// Streaming is on: off, every texture a material binds is wanted whole.
    pub streaming: bool,
    /// The pool, in bytes (0 with streaming off).
    pub pool: u64,
    /// Bytes the textures on the renderer hold, tails and levels, with the levels on their way: never more
    /// than the pool, while streaming, unless the tails alone are ([`Self::tails`]).
    pub resident: u64,
    /// The tails' share of [`Self::resident`]: what the streamer can't lower.
    pub tails: u64,
    /// Textures on the renderer, and those wanted whole whatever the views read.
    pub textures: u32,
    pub held_whole: u32,
    /// Textures whose top is coarser than what the views read, and the most frames one has been.
    pub in_debt: u32,
    pub worst_debt: u32,
    /// Textures the pool keeps coarser than their need: debt that waiting won't pay.
    pub over_pool: u32,
    /// Bytes the textures would hold at their needs, tails and levels: what the views would have the pool
    /// hold (0 with streaming off). Over the pool, the pool binds.
    pub wanted: u64,
    /// Levels raised and lowered this frame, over every texture: the frame's totals, its update's pass and
    /// every pass a capture's or a counting bench's finishing adds ([`crate::Content::finish_texture_levels`]),
    /// not the last pass's.
    pub levels_raised: u32,
    pub levels_lowered: u32,
    /// Textures whose levels this frame started loading, summed over its passes the same way.
    pub started: u32,
    /// Textures waiting to be lowered past [`MAX_LOWERS`].
    pub lowers_waiting: u32,
    /// What the last update's work on levels took on the calling thread: choosing, lowering, asking and
    /// raising, the renderer's copies and writes recorded. With [`Self::coverage_time`] it is what streaming
    /// adds to a frame's CPU (spec §3.2's streaming row).
    pub time: std::time::Duration,
    /// Of [`Self::time`], what the raises took on the renderer: making the textures, copying the levels
    /// into staging, recording the copies and rebinding.
    pub raising: std::time::Duration,
    /// What the renderer's estimate of the last frame's coverage took
    /// ([`Renderer::estimate_texture_coverage`]), at the top of the update, on the calling thread.
    pub coverage_time: std::time::Duration,
    /// Waits for a level that the pool never held: a texture needed finer than it held, waiting only for its
    /// levels to load and raise. m5-plan exit test 2's "within 30 frames" reads every wait, these and
    /// [`Self::pool_waits`] together ([`Self::all_waits`]); the kinds say why a wait was long.
    pub waits: TextureWaits,
    /// Waits the pool held at any frame: long because the pool kept the texture coarser, not the loads.
    pub pool_waits: TextureWaits,
}

impl TextureStreamingStats {
    /// Every wait, of both kinds: what exit test 2 reads.
    #[must_use]
    pub fn all_waits(&self) -> TextureWaits {
        let (waits, pool) = (self.waits, self.pool_waits);
        TextureWaits {
            frames: std::array::from_fn(|bin| waits.frames[bin].saturating_add(pool.frames[bin])),
            open: waits.open + pool.open,
            longest_open: waits.longest_open.max(pool.longest_open),
        }
    }
}

/// Bins of [`TextureWaits::frames`]: 0 to 31 frames, and the last for 32 or more.
pub const WAIT_BINS: usize = 33;

/// Waits of one kind for a texture's level (m5-plan exit test 2). A wait starts at the update a texture is
/// needed finer than it held at the last, and ends at the first its top reaches what it needs, its debt
/// paid; its length is the updates in between it was in debt: 0 when its levels came the same update.
/// Completed waits add up from the renderer's first update, or from the last
/// [`crate::Content::start_texture_waits_over`] (a new renderer starts over too); open ones are counted afresh
/// each update, so a wait open across a start over lands after it, with its whole length.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TextureWaits {
    /// Completed waits by their length in frames: bin `k` for `k`, the last for [`WAIT_BINS`] - 1 or more.
    pub frames: [u32; WAIT_BINS],
    /// Waits still open at the last update, and the longest of them, in frames: one that never ends shows
    /// here.
    pub open: u32,
    pub longest_open: u32,
}

impl Default for TextureWaits {
    fn default() -> Self {
        Self { frames: [0; WAIT_BINS], open: 0, longest_open: 0 }
    }
}

impl TextureWaits {
    /// Waits completed.
    #[must_use]
    pub fn completed(&self) -> u32 {
        self.frames.iter().sum()
    }

    /// The bin of the completed waits' `share` percentile (0.99 for the 99th; nearest rank), `None` with no
    /// waits completed. The last bin, [`WAIT_BINS`] - 1, reads as that many frames or more.
    #[must_use]
    pub fn percentile(&self, share: f64) -> Option<u32> {
        let completed = u64::from(self.completed());
        let rank = ((share.clamp(0.0, 1.0) * completed as f64).ceil() as u64).max(1);
        let mut below = 0;
        (0..WAIT_BINS as u32).find(|&bin| {
            below += u64::from(self.frames[bin as usize]);
            completed > 0 && below >= rank
        })
    }

    /// The bin of the longest completed wait, `None` with none.
    #[must_use]
    pub fn worst(&self) -> Option<u32> {
        (0..WAIT_BINS as u32).rev().find(|&bin| self.frames[bin as usize] > 0)
    }

    fn complete(&mut self, frames: u32) {
        let bin = &mut self.frames[(frames as usize).min(WAIT_BINS - 1)];
        *bin = bin.saturating_add(1);
    }

    fn still_open(&mut self, frames: u32) {
        self.open += 1;
        self.longest_open = self.longest_open.max(frames);
    }
}

/// One texture's state, for the scale views (m5-plan X1), from [`crate::Content::texture_levels`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TextureLevelState {
    /// The top the views want (0 is the whole texture), the one it has on the GPU, and its tail's.
    pub wanted: u32,
    pub resident: u32,
    pub tail: u32,
    /// Frames its top has been coarser than wanted.
    pub debt: u32,
}

/// What the stage wants of its streamed textures, and holds of their levels, on one renderer.
#[derive(Default)]
pub(crate) struct TextureLevels {
    textures: BTreeMap<AssetId, Wanted>,
    /// Tops asked for ([`crate::Content::want_texture_top`]), which outlast a renderer.
    asked: BTreeMap<AssetId, u32>,
    /// Levels let go of, which go back to the server once nothing here holds them again.
    give_back: Vec<(AssetId, u32)>,
    /// The streamer, while streaming is on; it outlasts a renderer.
    streaming: Option<TextureStreaming>,
    /// The coverage estimate last read ([`sapphire_render::TextureCoverageFrame::frame`]).
    seen: u64,
    stats: TextureStreamingStats,
    /// Scratch, kept so a steady frame allocates nothing.
    order: Vec<(Rank, AssetId)>,
    /// Levels the last [`Self::follow`] raised or lowered, and textures it started loading levels for: none,
    /// and with nothing on its way a capture's finishing is as settled as it gets.
    progress: u32,
}

/// One texture's levels.
#[derive(Default)]
struct Wanted {
    /// Materials on the renderer that bind the texture.
    users: u32,
    /// Of [`Self::users`], bindings in slots that never stream (M5 M5).
    whole_users: u32,
    /// Held whole this frame ([`crate::Content::keep_textures_whole`]).
    whole_now: bool,
    /// The streamer's choice for it.
    choice: Choice,
    /// Levels asked of the server, held while they're on their way or on the GPU.
    handles: BTreeMap<u32, Handle<TextureLevel>>,
    /// Levels the server handed over, waiting for the levels between them and the texture's top.
    arrived: BTreeMap<u32, Arrived>,
    /// The version of each level above the tail on the GPU.
    written: BTreeMap<u32, Version>,
    /// The deepest level that failed to load or raise: the texture stays under it until its tail changes.
    failed: Option<u32>,
    /// The deepest level the server let go of while it was on the GPU: the texture goes under it.
    lost: Option<u32>,
}

/// What the streamer chose for one texture.
#[derive(Clone, Copy, Debug, Default)]
struct Choice {
    /// The finest top the views read it at: 0 for one held whole, its tail's for one unseen.
    need: u32,
    /// The top held, by hysteresis; `None` before the first estimate, which is its tail.
    target: Option<u32>,
    /// Estimates in a row its need has been coarser than `target` by the slack, and the finest of them.
    coarser: u32,
    coarser_finest: u32,
    /// Estimates since the views last read it.
    unseen: u32,
    /// The top planned within the pool this frame.
    planned: u32,
    /// The pool's last plan for it, while that was coarser than its target.
    pool: Option<PoolLowered>,
    /// Frames its top has been coarser than its need.
    debt: u32,
    /// The pool held it coarser than its need at some frame of its debt ([`TextureStreamingStats::pool_waits`]).
    debt_held: bool,
    /// Its top at the last count: needed finer than that, it's waiting.
    counted_top: u32,
    /// Its missing levels may start loading this frame, and how urgently.
    start: Option<LoadPriority>,
    /// It may lower to its plan this frame ([`MAX_LOWERS`]).
    lower: bool,
}

/// A plan the pool held coarser than a texture's target: where the next frame's plan starts from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct PoolLowered {
    planned: u32,
    /// Estimates since it last tried its target: at [`LOWER_AFTER`] it tries again.
    estimates: u32,
}

impl Choice {
    /// The top it would plan with the pool's room: its target, under the level `floor` that failed, no deeper
    /// than `above`.
    fn base(&self, above: u32, floor: u32) -> u32 {
        self.target.unwrap_or(above).max(floor).min(above)
    }

    /// Follows a fresh estimate: the finest level the views read the texture at, or `None` if they didn't
    /// read it, with `above` levels above its tail. The need follows at once, and the top held by
    /// hysteresis: at once to a finer need, to a coarser one only after [`LOWER_AFTER`] estimates coarser by
    /// [`LOWER_SLACK`] too, and then to the finest of them.
    fn follow(&mut self, finest: Option<f32>, above: u32) {
        let (need, slack) = match finest {
            Some(finest) => {
                self.unseen = 0;
                (level(finest - MARGIN, above), level(finest - MARGIN - LOWER_SLACK, above))
            }
            None => {
                self.unseen = self.unseen.saturating_add(1);
                (above, above)
            }
        };
        self.need = need;
        let target = self.target.unwrap_or(above);
        if need < target {
            (self.target, self.coarser) = (Some(need), 0);
        } else if slack > target {
            self.coarser_finest = if self.coarser == 0 { slack } else { self.coarser_finest.min(slack) };
            self.coarser += 1;
            if self.coarser >= LOWER_AFTER {
                (self.target, self.coarser) = (Some(self.coarser_finest), 0);
            }
        } else {
            self.coarser = 0;
        }
    }
}

/// Which texture the pool lowers first: the greatest.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct Rank {
    /// Not held whole.
    streams: bool,
    /// Estimates the views haven't read it.
    unseen: u32,
    /// Levels its plan is finer than its need: a top hysteresis holds that the views don't read, which the pool
    /// takes before it puts any texture under its need, since lowering it costs nothing on screen (M5 Z2).
    surplus: u32,
    /// Levels its plan is below its target, reversed: the least lowered first.
    lowered: std::cmp::Reverse<u32>,
    /// The pool lowered it last frame: of two alike, the same one goes again, so they don't take turns.
    lowered_last: bool,
    /// The top held for it by hysteresis: the most coarsely read first. Never the need itself, which follows
    /// each estimate and would turn the order over with a wobble across a level.
    read: u32,
    /// Bytes its top planned level holds.
    bytes: u64,
}

/// What the streamer's plan reads of a texture on the renderer ([`TextureLevels::plan`]).
#[derive(Clone, Copy, Debug)]
struct Shape {
    /// The whole texture's shape.
    whole: TextureDesc,
    /// Its top on the GPU, and the levels above its tail.
    top: u32,
    above: u32,
}

impl Shape {
    fn of(texture: &GpuTexture) -> Self {
        Self { whole: texture.whole(), top: texture.top(), above: texture.above() }
    }
}

/// Which version of a level the server handed over.
#[derive(Clone, Copy, PartialEq, Eq)]
struct Version {
    slot: AssetSlot,
    generation: u32,
}

struct Arrived {
    version: Version,
    level: CpuResident<TextureLevel>,
}

impl Wanted {
    /// Held whole: by a slot that never streams, or this frame's [`TextureLevels::hold_whole_now`].
    fn held_whole(&self) -> bool {
        self.whole_users > 0 || self.whole_now
    }

    /// The top it stays under, below the deepest level that failed, no deeper than `above`.
    fn floor(&self, above: u32) -> u32 {
        self.failed.map_or(0, |failed| failed + 1).min(above)
    }

    /// Whether a level is held here: asked for, arrived or on the GPU.
    fn holds(&self, index: u32) -> bool {
        self.handles.contains_key(&index)
            || self.arrived.contains_key(&index)
            || self.written.contains_key(&index)
    }

    /// Lets go of every level it holds or has asked for that `keep` refuses, for the server to have back.
    fn let_go(&mut self, texture: AssetId, keep: impl Fn(u32) -> bool, give_back: &mut Vec<(AssetId, u32)>) {
        let dropped = |index: &u32| !keep(*index);
        give_back.extend(self.handles.keys().copied().filter(dropped).map(|index| (texture, index)));
        give_back.extend(self.arrived.keys().copied().filter(dropped).map(|index| (texture, index)));
        self.handles.retain(|index, _| keep(*index));
        self.arrived.retain(|index, _| keep(*index));
    }
}

impl TextureLevels {
    /// For a new renderer, which holds nothing yet: the tops asked for stay asked for, and the streamer as it
    /// was.
    pub(crate) fn for_new_renderer(&self) -> Self {
        Self { asked: self.asked.clone(), streaming: self.streaming, ..Self::default() }
    }

    /// A material on the renderer binds `texture`, in a slot that never streams if `whole`.
    pub(crate) fn bind(&mut self, texture: AssetId, whole: bool) {
        let wanted = self.textures.entry(texture).or_default();
        wanted.users += 1;
        wanted.whole_users += u32::from(whole);
    }

    /// A material that bound `texture` (in a slot that never streams if `whole`) left the renderer, or binds
    /// others now.
    pub(crate) fn unbind(&mut self, texture: AssetId, whole: bool) {
        if let Some(wanted) = self.textures.get_mut(&texture) {
            wanted.users = wanted.users.saturating_sub(1);
            wanted.whole_users = wanted.whole_users.saturating_sub(u32::from(whole));
        }
    }

    /// Streams textures by what the views draw within `streaming`'s pool, or with `None` wants every texture a
    /// material binds whole, as before the streamer.
    pub(crate) fn set_streaming(&mut self, streaming: Option<TextureStreaming>) {
        self.streaming = streaming;
    }

    pub(crate) fn streaming(&self) -> Option<TextureStreaming> {
        self.streaming
    }

    /// Holds `texture` whole this frame, or lets it stream again: [`crate::Content::keep_textures_whole`]'s,
    /// marked afresh before each [`Self::follow`].
    pub(crate) fn clear_whole_now(&mut self) {
        for wanted in self.textures.values_mut() {
            wanted.whole_now = false;
        }
    }

    pub(crate) fn hold_whole_now(&mut self, texture: AssetId) {
        if let Some(wanted) = self.textures.get_mut(&texture) {
            wanted.whole_now = true;
        }
    }

    pub(crate) fn stats(&self) -> TextureStreamingStats {
        self.stats
    }

    /// Starts the completed waits over ([`TextureWaits`]); open ones go on, to land when they end.
    pub(crate) fn start_waits_over(&mut self) {
        start_waits_over(&mut self.stats);
    }

    /// Has `renderer` estimate the coverage of the frame it recorded last, while streaming, and times it:
    /// at the top of an update, before uploads change what the frame's draws refer to.
    pub(crate) fn estimate(&mut self, renderer: &mut Renderer) {
        if self.streaming.is_none() {
            self.stats.coverage_time = std::time::Duration::ZERO;
            return;
        }
        let stopwatch = Stopwatch::start();
        renderer.estimate_texture_coverage();
        self.stats.coverage_time = stopwatch.elapsed();
    }

    /// Each texture on the renderer's state, in ID order (for the scale views, m5-plan X1).
    pub(crate) fn states<'a>(
        &'a self,
        textures: &'a HashMap<AssetId, Held<GpuTexture>>,
    ) -> impl Iterator<Item = (AssetId, TextureLevelState)> + 'a {
        self.textures.iter().filter_map(move |(&id, wanted)| {
            let texture = &textures.get(&id)?.object;
            let wanted_top = match self.streaming {
                Some(_) => wanted.choice.need.min(texture.above()),
                None => wanted_top(&self.asked, id, wanted, texture.above()),
            };
            Some((
                id,
                TextureLevelState {
                    wanted: wanted_top,
                    resident: texture.top(),
                    tail: texture.above(),
                    debt: wanted.choice.debt,
                },
            ))
        })
    }

    /// The streamed textures' tops for the renderer's texture view (M5 X1): while the streamer is on,
    /// each texture it streams this frame (not held whole, with levels above its tail, bound in a slot
    /// that streams), the top its views want and the one on the GPU; nothing with streaming off.
    pub(crate) fn streamed_tops<'a>(
        &'a self,
        textures: &'a HashMap<AssetId, Held<GpuTexture>>,
    ) -> impl Iterator<Item = (AssetId, StreamedTops)> + 'a {
        // As `update` holds a texture whole: any user that needs it whole, or a cut.
        let streams = |wanted: &Wanted| wanted.users > 0 && wanted.whole_users == 0 && !wanted.whole_now;
        self.states(textures)
            .filter(move |(id, state)| {
                self.streaming.is_some() && state.tail > 0 && self.textures.get(id).is_some_and(streams)
            })
            .map(|(id, state)| (id, StreamedTops { wanted: state.wanted, resident: state.resident }))
    }

    /// Asks for `texture` at `top`, or with `None` gives it back to the stage's choice.
    pub(crate) fn ask(&mut self, texture: AssetId, top: Option<u32>) {
        match top {
            Some(top) => self.asked.insert(texture, top),
            None => self.asked.remove(&texture),
        };
    }

    /// Takes `level`, which the server hands over in `upload`, for `texture`, the GPU texture it's a level
    /// of if that's on the renderer: a level the texture holds is written over if this is a new version,
    /// and any other is kept until the texture can rise to it.
    pub(crate) fn arrive(
        &mut self,
        upload: Upload<'_>,
        level: &ArchivedTextureLevel,
        texture: Option<&GpuTexture>,
        renderer: &mut Renderer,
    ) -> Result<(), UploadError> {
        let version = Version { slot: upload.slot(), generation: upload.generation() };
        let index = level.level();
        let wanted = self.textures.entry(level.texture()).or_default();
        if let Some(texture) = texture.filter(|texture| (texture.top()..texture.above()).contains(&index)) {
            // Sent again, for a renderer kept, it's the version written.
            if wanted.written.get(&index).is_some_and(|written| {
                written.slot == version.slot && written.generation >= version.generation
            }) {
                return Ok(());
            }
            renderer.rewrite_texture_level(texture, level)?;
            wanted.written.insert(index, version);
            return Ok(());
        }
        let level = upload.keep::<TextureLevel>().ok_or("a texture level that isn't one")?;
        wanted.arrived.insert(index, Arrived { version, level });
        Ok(())
    }

    /// The tail of `texture` was replaced by one of another shape (hot reload): the levels held for the
    /// old one go back, and are asked for again, as the new tail's levels.
    pub(crate) fn reshaped(&mut self, texture: AssetId) {
        if let Some(wanted) = self.textures.get_mut(&texture) {
            wanted.let_go(texture, |_| false, &mut self.give_back);
            wanted.written.clear();
            (wanted.failed, wanted.lost) = (None, None);
        }
    }

    /// The server unloaded asset `id` in `slot`: a texture's tail, which takes what's held of its levels
    /// with it, or a level, which takes the texture under it.
    pub(crate) fn unloaded(&mut self, id: AssetId, slot: AssetSlot) {
        if let Some(mut wanted) = self.textures.remove(&id) {
            wanted.let_go(id, |_| false, &mut self.give_back);
            // A material that binds it still counts, for when it comes again.
            if wanted.users > 0 {
                let users =
                    Wanted { users: wanted.users, whole_users: wanted.whole_users, ..Wanted::default() };
                self.textures.insert(id, users);
            }
            return;
        }
        for wanted in self.textures.values_mut() {
            wanted.handles.retain(|_, handle| handle.id() != id);
            wanted.arrived.retain(|_, arrived| arrived.version.slot != slot);
            let lost =
                wanted.written.iter().filter(|(_, written)| written.slot == slot).map(|(&index, _)| index);
            if let Some(index) = lost.max() {
                wanted.written.remove(&index);
                wanted.lost = wanted.lost.max(Some(index));
            }
        }
    }

    /// Keeps only what the server still holds, by `resident`: a renderer set aside while a new one was
    /// tried missed what the server unloaded meanwhile.
    pub(crate) fn take_stale(&mut self, resident: impl Fn(AssetId) -> bool) {
        for (&texture, wanted) in &mut self.textures {
            let gone: Vec<u32> = wanted
                .written
                .keys()
                .copied()
                .filter(|&index| !resident(TextureLevel::id(texture, index)))
                .collect();
            if let Some(&deepest) = gone.last() {
                wanted.lost = wanted.lost.max(Some(deepest));
            }
            for index in gone {
                wanted.written.remove(&index);
            }
            wanted.arrived.retain(|&index, _| resident(TextureLevel::id(texture, index)));
        }
    }

    /// Brings each texture in `textures` toward its wanted top: lowers it now, asks the server for the
    /// levels it lacks, and raises it by those that have come, within the renderer's budget for the frame
    /// unless `unlimited`. Gives back to the server what it let go of. Returns the bytes the textures hold
    /// now, less what they held.
    pub(crate) fn follow(
        &mut self,
        textures: &mut HashMap<AssetId, Held<GpuTexture>>,
        server: &mut AssetServer,
        renderer: &mut Renderer,
        unlimited: bool,
    ) -> i64 {
        let stopwatch = Stopwatch::start();
        // A capture's finishing goes to the plan, unbudgeted, while streaming (a capture that streams), and
        // whole otherwise (the stage turns a whole capture's streamer off).
        let streaming = self.streaming;
        renderer.set_mip_budget(streaming.map_or(DEFAULT_MIP_BUDGET, |streaming| streaming.raise));
        let started = match streaming {
            Some(streaming) => {
                let start_bytes =
                    if unlimited { u64::MAX } else { server.budgets().upload_per_frame.min(streaming.raise) };
                self.choose(textures, renderer, streaming, start_bytes)
            }
            None => 0,
        };
        let Self { textures: wants, asked, give_back, .. } = self;
        let (mut raised, mut lowered) = (0, 0);
        let mut raising = std::time::Duration::ZERO;
        let mut change = 0;
        wants.retain(|&id, wanted| {
            let Some(texture) = textures.get_mut(&id).map(|held| &mut held.object) else {
                // Its tail isn't on the renderer: what's held of its levels waits for it while a material
                // binds it.
                if wanted.users == 0 {
                    wanted.let_go(id, |_| false, give_back);
                }
                return wanted.users > 0;
            };
            let before = (texture.bytes(), texture.top());
            let above = texture.above();
            let (top, start, may_lower) = match streaming {
                Some(_) => (wanted.choice.planned, wanted.choice.start, wanted.choice.lower),
                None => (wanted_top(asked, id, wanted, above), Some(LoadPriority::Normal), true),
            };
            // A level the server let go of takes the texture under it now; the plan waits its turn.
            let lost = wanted.lost.take().map_or(0, |lost| lost + 1);
            let floor = if may_lower { top.max(lost) } else { lost }.min(above);
            if texture.top() < floor
                && let Err(error) = renderer.lower_texture(id, texture, floor)
            {
                tracing::warn!(target: LOG_TARGET, %id, %error, "a texture couldn't be lowered");
            }
            let on_gpu = texture.top();
            wanted.written.retain(|&index, _| index >= on_gpu);
            // What's on the GPU above the plan stays held until its lower.
            let kept = top.min(on_gpu);
            wanted.let_go(id, |index| (kept..above).contains(&index), give_back);
            ask_levels(id, wanted, top, on_gpu, start, server, give_back);
            let raising_from = Stopwatch::start();
            if raise(id, wanted, texture, top, renderer, unlimited, give_back) {
                raising += raising_from.elapsed();
            }
            change += texture.bytes() as i64 - before.0 as i64;
            let after = texture.top();
            raised += before.1.saturating_sub(after);
            lowered += after.saturating_sub(before.1);
            // Nothing binds it and nothing's held of it: it's at its tail, with nothing left to do.
            wanted.users > 0 || !wanted.handles.is_empty() || !wanted.arrived.is_empty()
        });
        self.progress = raised + lowered + started;
        let Self { textures: wants, asked, stats, .. } = self;
        // The budgeted pass is a frame's first, its update's; an unbudgeted one finishes the same frame.
        add_pass(stats, !unlimited, [raised, lowered, started]);
        count(wants, asked, textures, streaming, stats);
        if !unlimited {
            stats.raising = raising;
            stats.time = stopwatch.elapsed();
        }
        let Self { textures: wants, give_back, .. } = self;
        for (texture, index) in std::mem::take(give_back) {
            // Let go of and asked for again since, it stays. One the server can't unload yet (loading, or
            // held elsewhere) comes back as an arrival nothing wants, or is found ready and never handed over
            // when it's asked for again, and goes back then.
            if !wants.get(&texture).is_some_and(|wanted| wanted.holds(index)) {
                server.unload(TextureLevel::id(texture, index));
            }
        }
        change
    }

    /// The streamer's half of a frame (see the module docs), from the renderer's last estimate: [`Self::plan`].
    fn choose(
        &mut self,
        textures: &HashMap<AssetId, Held<GpuTexture>>,
        renderer: &Renderer,
        streaming: TextureStreaming,
        start_bytes: u64,
    ) -> u32 {
        let shape = |id: AssetId| textures.get(&id).map(|held| Shape::of(&held.object));
        self.plan(shape, renderer.texture_coverage(), streaming, start_bytes)
    }

    /// Each texture's need from `coverage`, the top held by hysteresis, the plan within `streaming`'s pool (with
    /// what the pool lowered last frame), and which textures may start loading levels, within `start_bytes`.
    /// `shape` says what each texture on the renderer is. Returns how many textures may start.
    fn plan(
        &mut self,
        shape: impl Fn(AssetId) -> Option<Shape>,
        coverage: TextureCoverageFrame<'_>,
        streaming: TextureStreaming,
        start_bytes: u64,
    ) -> u32 {
        let fresh = coverage.frame != self.seen;
        self.seen = coverage.frame;
        let mut total = 0u64;
        let mut tails = 0u64;
        let mut wanted_bytes = 0u64;
        self.order.clear();
        for (&id, wanted) in &mut self.textures {
            let Some(texture) = shape(id) else { continue };
            let (above, whole) = (texture.above, texture.whole);
            let floor = wanted.floor(above);
            let held_whole = wanted.held_whole();
            let choice = &mut wanted.choice;
            (choice.start, choice.lower) = (None, false);
            if wanted.users == 0 {
                (choice.need, choice.target, choice.coarser) = (above, Some(above), 0);
            } else if let Some(&top) = self.asked.get(&id) {
                (choice.need, choice.target, choice.coarser) = (top.min(above), Some(top.min(above)), 0);
            } else if held_whole {
                (choice.need, choice.target, choice.coarser) = (0, Some(0), 0);
            } else if fresh {
                let read = coverage.textures.binary_search_by(|entry| entry.texture.cmp(&id)).ok();
                let finest = read.map(|index| {
                    finest_level(coverage.textures[index].uv_per_pixel, whole.width.min(whole.height))
                });
                choice.follow(finest, above);
            } else if choice.target.is_none() {
                // Not estimated yet (a run's first update, before any frame is drawn): it needs its tail until
                // the views have read it, so it isn't waiting, nor held by the pool (M5 Z2).
                choice.need = above;
            }
            // One the pool lowered keeps last frame's plan until it may rise (below), or until it tries its
            // target again. One held whole doesn't: it goes last, so every frame's plan lowers each streamed
            // texture first, and a plan it kept would hold it down in their place.
            let base = choice.base(above, floor);
            choice.planned = match choice.pool {
                Some(pool) if pool.estimates < LOWER_AFTER && !held_whole => pool.planned.clamp(base, above),
                _ => base,
            };
            total += bytes_from(&whole, choice.planned);
            tails += bytes_from(&whole, above);
            wanted_bytes += bytes_from(&whole, choice.need.max(floor).min(above));
            if choice.planned < above {
                self.order.push((rank(&texture, choice, held_whole), id));
            }
        }
        // Over the pool: lower plans a level at a time, the greatest rank first.
        if total > streaming.pool {
            let mut heap = std::collections::BinaryHeap::from(std::mem::take(&mut self.order));
            while total > streaming.pool
                && let Some((_, id)) = heap.pop()
            {
                let (Some(wanted), Some(texture)) = (self.textures.get_mut(&id), shape(id)) else {
                    continue;
                };
                let held_whole = wanted.held_whole();
                let choice = &mut wanted.choice;
                total -= texture.whole.level_byte_len(choice.planned);
                choice.planned += 1;
                if choice.planned < texture.above {
                    heap.push((rank(&texture, choice, held_whole), id));
                }
            }
            self.order = heap.into_vec();
        }
        // Under it: what the pool lowered rises toward its target, the most wanted first, a level at a time while
        // the pool keeps room for one more such level beside the plan, so a rise never makes the next frame's
        // plan lower another texture in its place. One that can't rise holds back every one after it: a less
        // wanted texture never takes room a more wanted one waits for (one held whole goes last).
        self.order.clear();
        for (&id, wanted) in &self.textures {
            let Some(texture) = shape(id) else { continue };
            let choice = &wanted.choice;
            if choice.planned > choice.base(texture.above, wanted.floor(texture.above)) {
                self.order.push((rank(&texture, choice, wanted.held_whole()), id));
            }
        }
        self.order.sort_unstable();
        'rise: for &(_, id) in &self.order {
            let (Some(wanted), Some(texture)) = (self.textures.get_mut(&id), shape(id)) else { continue };
            let base = wanted.choice.base(texture.above, wanted.floor(texture.above));
            let choice = &mut wanted.choice;
            while choice.planned > base {
                let level = texture.whole.level_byte_len(choice.planned - 1);
                if total + 2 * level > streaming.pool {
                    break 'rise;
                }
                total += level;
                choice.planned -= 1;
            }
        }
        // What the pool lowered, for the next frame's plan, with the estimates since it last tried its target.
        for (&id, wanted) in &mut self.textures {
            let Some(texture) = shape(id) else { continue };
            let base = wanted.choice.base(texture.above, wanted.floor(texture.above));
            let choice = &mut wanted.choice;
            choice.pool = (choice.planned > base).then(|| {
                let estimates = match choice.pool {
                    Some(pool) if pool.estimates < LOWER_AFTER => pool.estimates + u32::from(fresh),
                    _ => 0,
                };
                PoolLowered { planned: choice.planned, estimates }
            });
        }
        // Which textures lower this frame: the greatest rank first, at most `MAX_LOWERS`. `held` is what the
        // textures hold, on the GPU and on their way, once those lowers are done.
        self.order.clear();
        let mut held = 0u64;
        for (&id, wanted) in &self.textures {
            let Some(texture) = shape(id) else { continue };
            let top = texture.top;
            let lowest = (0..top).find(|&index| wanted.holds(index)).unwrap_or(top);
            held += bytes_from(&texture.whole, lowest);
            if top < wanted.choice.planned {
                self.order.push((rank(&texture, &wanted.choice, wanted.held_whole()), id));
            }
        }
        self.order.sort_unstable_by(|a, b| b.cmp(a));
        let lowers = self.order.len().min(MAX_LOWERS);
        for &(_, id) in &self.order[..lowers] {
            let (Some(wanted), Some(texture)) = (self.textures.get_mut(&id), shape(id)) else {
                continue;
            };
            let top = texture.top;
            let lowest = (0..top).find(|&index| wanted.holds(index)).unwrap_or(top);
            held -= bytes_from(&texture.whole, lowest) - bytes_from(&texture.whole, wanted.choice.planned);
            wanted.choice.lower = true;
        }
        self.stats.lowers_waiting = (self.order.len() - lowers) as u32;
        // Which textures start loading levels: those held whole first, then the widest gap, then the nearest;
        // each only if its levels fit the pool beside what's held.
        self.order.clear();
        for (&id, wanted) in &self.textures {
            let Some(texture) = shape(id) else { continue };
            let planned = wanted.choice.planned;
            let missing: u64 = (planned..texture.top)
                .filter(|&index| !wanted.holds(index))
                .map(|index| texture.whole.level_byte_len(index))
                .sum();
            if missing > 0 {
                let gap = texture.top - planned;
                let rank = Rank {
                    streams: !wanted.held_whole(),
                    unseen: 0,
                    surplus: 0,
                    lowered: std::cmp::Reverse(gap),
                    lowered_last: false,
                    read: wanted.choice.target.unwrap_or(texture.above),
                    bytes: missing,
                };
                self.order.push((rank, id));
            }
        }
        self.order.sort_unstable();
        let (mut bounded, mut bytes, mut started) = (0, 0u64, 0);
        for &(rank, id) in &self.order {
            let urgent = !rank.streams;
            if !urgent && (bounded >= MAX_STARTS || (bounded > 0 && bytes + rank.bytes > start_bytes)) {
                continue;
            }
            if held + rank.bytes > streaming.pool {
                continue;
            }
            if !urgent {
                bounded += 1;
                bytes += rank.bytes;
            }
            if let Some(wanted) = self.textures.get_mut(&id) {
                wanted.choice.start = Some(if urgent { LoadPriority::High } else { LoadPriority::Normal });
                held += rank.bytes;
                started += 1;
            }
        }
        self.stats.pool = streaming.pool;
        self.stats.tails = tails;
        self.stats.wanted = wanted_bytes;
        started
    }

    /// The levels a capture waits for: asked for and not yet come.
    pub(crate) fn waiting(&self) -> Vec<Handle<TextureLevel>> {
        let mut waiting = Vec::new();
        for wanted in self.textures.values() {
            for (index, handle) in &wanted.handles {
                if !wanted.arrived.contains_key(index) && !wanted.written.contains_key(index) {
                    waiting.push(handle.clone());
                }
            }
        }
        waiting
    }

    /// Whether every texture in `textures` is at its wanted top, or as near as the levels that failed let it.
    pub(crate) fn settled(&self, textures: &HashMap<AssetId, Held<GpuTexture>>) -> bool {
        self.textures.iter().all(|(&id, wanted)| {
            textures.get(&id).is_none_or(|held| {
                let (top, above) = (held.object.top(), held.object.above());
                match self.streaming {
                    // At its plan: no lower waits its turn, and no level it plans for is missing.
                    Some(_) => top == wanted.choice.planned.min(above),
                    None => top <= wanted_top(&self.asked, id, wanted, above),
                }
            })
        })
    }

    /// Whether the last [`Self::follow`] raised, lowered or started anything.
    pub(crate) fn progressed(&self) -> bool {
        self.progress > 0
    }
}

/// The top wanted of `texture`, with `above` levels above its tail: the one asked for, or the whole
/// texture, while a material binds it; its tail's once none does. A level that failed keeps it under.
fn wanted_top(asked: &BTreeMap<AssetId, u32>, texture: AssetId, wanted: &Wanted, above: u32) -> u32 {
    if wanted.users == 0 {
        return above;
    }
    let floor = wanted.failed.map_or(0, |failed| failed + 1);
    asked.get(&texture).copied().unwrap_or(0).max(floor).min(above)
}

/// The finest level a texture `size` texels on its shorter side is read at, by the estimate's `uv_per_pixel`:
/// minus infinity for one read whole.
fn finest_level(uv_per_pixel: f32, size: u32) -> f32 {
    if uv_per_pixel > 0.0 { (uv_per_pixel * size as f32).log2() } else { f32::NEG_INFINITY }
}

/// The coarsest top that reads level `finest` or finer, no deeper than `above`.
fn level(finest: f32, above: u32) -> u32 {
    if finest >= 0.0 { (finest.floor() as u32).min(above) } else { 0 }
}

/// Bytes a texture of shape `whole` holds with its top at `top`.
fn bytes_from(whole: &TextureDesc, top: u32) -> u64 {
    (top..whole.mip_level_count).map(|level| whole.level_byte_len(level)).sum()
}

/// Where a texture with `choice` stands in the order the pool lowers plans in.
fn rank(texture: &Shape, choice: &Choice, held_whole: bool) -> Rank {
    let read = choice.target.unwrap_or(texture.above);
    Rank {
        streams: !held_whole,
        unseen: choice.unseen,
        surplus: choice.need.saturating_sub(choice.planned),
        lowered: std::cmp::Reverse(choice.planned.saturating_sub(read)),
        lowered_last: choice.pool.is_some(),
        read,
        bytes: texture.whole.level_byte_len(choice.planned),
    }
}

/// Adds a pass's levels raised and lowered and textures started to the frame's, `first` starting the frame's
/// over ([`TextureStreamingStats::levels_raised`]).
fn add_pass(stats: &mut TextureStreamingStats, first: bool, [raised, lowered, started]: [u32; 3]) {
    if first {
        (stats.levels_raised, stats.levels_lowered, stats.started) = (0, 0, 0);
    }
    stats.levels_raised += raised;
    stats.levels_lowered += lowered;
    stats.started += started;
}

/// The frame's counters: bytes held, debt and what's held whole. A texture's debt counts the frames its top
/// has been coarser than what's wanted of it: its need while streaming, its wanted top otherwise.
fn count(
    wants: &mut BTreeMap<AssetId, Wanted>,
    asked: &BTreeMap<AssetId, u32>,
    textures: &HashMap<AssetId, Held<GpuTexture>>,
    streaming: Option<TextureStreaming>,
    stats: &mut TextureStreamingStats,
) {
    let (mut resident, mut tails, mut count, mut held_whole) = (0, 0, 0, 0);
    let (mut in_debt, mut worst_debt, mut over_pool) = (0, 0, 0);
    for waits in [&mut stats.waits, &mut stats.pool_waits] {
        (waits.open, waits.longest_open) = (0, 0);
    }
    for (&id, wanted) in wants.iter_mut() {
        let Some(texture) = textures.get(&id).map(|held| &held.object) else { continue };
        let above = texture.above();
        count += 1;
        held_whole += u32::from(wanted.users > 0 && (wanted.whole_users > 0 || wanted.whole_now));
        tails += bytes_from(&texture.whole(), above);
        // On the GPU, and levels on their way down to the top planned.
        let top = texture.top();
        let lowest_held = (0..top).find(|&index| wanted.holds(index)).unwrap_or(top);
        resident += bytes_from(&texture.whole(), lowest_held);
        let need = match streaming {
            Some(_) => wanted.choice.need.min(above),
            None => wanted_top(asked, id, wanted, above),
        };
        let floor = wanted.failed.map_or(0, |failed| failed + 1).min(above);
        let required = need.max(floor);
        let pool_held = streaming.is_some() && wanted.choice.planned > required;
        if wait(&mut wanted.choice, top, required, pool_held, stats) {
            in_debt += 1;
            worst_debt = worst_debt.max(wanted.choice.debt);
            over_pool += u32::from(pool_held);
        }
    }
    *stats = TextureStreamingStats {
        streaming: streaming.is_some(),
        pool: streaming.map_or(0, |streaming| streaming.pool),
        resident,
        tails,
        textures: count,
        held_whole,
        in_debt,
        worst_debt,
        over_pool,
        ..*stats
    };
}

/// Empties the completed waits' histograms of both kinds, leaving the open waits, which each count recounts.
fn start_waits_over(stats: &mut TextureStreamingStats) {
    for waits in [&mut stats.waits, &mut stats.pool_waits] {
        waits.frames = [0; WAIT_BINS];
    }
}

/// Counts a texture's debt at an update, with its top at `top` and what it needs at `required`, and its wait
/// ([`TextureWaits`]): in debt, an open wait, of the pool's kind if `pool_held` this frame or any frame
/// before in the wait; out of it, a wait completed if it was in debt or is needed finer than it held at
/// the last count. Returns whether it's in debt.
fn wait(
    choice: &mut Choice,
    top: u32,
    required: u32,
    pool_held: bool,
    stats: &mut TextureStreamingStats,
) -> bool {
    let in_debt = top > required;
    if in_debt {
        choice.debt = choice.debt.saturating_add(1);
        choice.debt_held |= pool_held;
    }
    let waits = if choice.debt_held { &mut stats.pool_waits } else { &mut stats.waits };
    if in_debt {
        waits.still_open(choice.debt);
    } else {
        if choice.debt > 0 || required < choice.counted_top {
            waits.complete(choice.debt);
        }
        (choice.debt, choice.debt_held) = (0, false);
    }
    choice.counted_top = top;
    in_debt
}

/// Asks the server for `texture`'s levels from `top` to `on_gpu`, the texture's top, that aren't on their
/// way, the nearest first, at `start`'s priority; with `None`, it asks for none it hasn't already. A level
/// that failed stops the texture under it; one the server says is ready but never handed over here (it was
/// uploaded for another renderer) goes back, to be asked for again.
fn ask_levels(
    texture: AssetId,
    wanted: &mut Wanted,
    top: u32,
    on_gpu: u32,
    start: Option<LoadPriority>,
    server: &mut AssetServer,
    give_back: &mut Vec<(AssetId, u32)>,
) {
    for index in (top..on_gpu).rev() {
        if wanted.failed.is_some_and(|failed| index <= failed) {
            break;
        }
        if let Some(handle) = wanted.handles.get(&index) {
            match server.state(handle) {
                LoadState::Loading => {}
                LoadState::Ready if wanted.arrived.contains_key(&index) => {}
                LoadState::Ready => {
                    wanted.handles.remove(&index);
                    give_back.push((texture, index));
                }
                LoadState::Failed(error) => {
                    tracing::warn!(target: LOG_TARGET, %texture, level = index, %error, "a texture level failed");
                    wanted.handles.remove(&index);
                    wanted.failed = Some(index);
                    give_back.push((texture, index));
                }
            }
            continue;
        }
        let Some(priority) = start else { continue };
        match server.load_with::<TextureLevel>(TextureLevel::id(texture, index), priority) {
            Ok(handle) => {
                wanted.handles.insert(index, handle);
            }
            Err(error) => {
                tracing::warn!(target: LOG_TARGET, %texture, level = index, %error, "a texture level can't load");
                wanted.failed = Some(index);
            }
        }
    }
}

/// Raises `texture`, asset `id`, by the levels that have come just above its top, no higher than `top`, if
/// the frame's budget allows. Returns whether it tried.
fn raise(
    id: AssetId,
    wanted: &mut Wanted,
    texture: &mut GpuTexture,
    top: u32,
    renderer: &mut Renderer,
    unlimited: bool,
    give_back: &mut Vec<(AssetId, u32)>,
) -> bool {
    let on_gpu = texture.top();
    let Some(new_top) = (top..on_gpu).rev().take_while(|index| wanted.arrived.contains_key(index)).last()
    else {
        return false;
    };
    let levels: Vec<&ArchivedTextureLevel> =
        (new_top..on_gpu).map(|index| wanted.arrived[&index].level.view()).collect();
    let bytes = levels.iter().map(|level| level.data().len() as u64).sum();
    if !unlimited && !renderer.may_raise(bytes) {
        return false;
    }
    let raised = renderer.raise_texture(id, texture, &levels);
    drop(levels);
    match raised {
        Ok(()) => {
            for index in new_top..on_gpu {
                let arrived = wanted.arrived.remove(&index).expect("in the run");
                wanted.written.insert(index, arrived.version);
            }
        }
        Err(error) => {
            tracing::warn!(target: LOG_TARGET, %id, %error, "a texture couldn't be raised");
            wanted.failed = Some(on_gpu - 1);
            wanted.let_go(id, |index| index >= on_gpu, give_back);
        }
    }
    true
}

#[cfg(test)]
mod tests;
