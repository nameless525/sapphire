//! Loads done on the frame's thread while the stage is in play (M5, exit test 2: "nothing loads on the frame's
//! thread"). The asset server's workers read and decode, so this holds by construction; this ledger is what
//! would move if a change read a pack, decompressed or checked an asset on the thread that runs the frame.
//!
//! - **The frame's thread** is the one that marks itself in play ([`InPlay`]), from a bench's first frame to its
//!   last: the walk's and the fight's (`sapphire_sandbox`). Nothing else is ever counted: not a boot, a loading
//!   screen or a tool, none of which mark themselves, nor any other thread. Within play, what the frame's thread
//!   does aside from a game's frame is set [`Aside`]: a capture's or a counting walk's bringing every texture to
//!   its top before a frame draws (`sapphire_stage`'s `CaptureTextures`), which waits for reads by design, as
//!   no game frame does.
//! - **A load** is the asset pipeline's own work, counted where every load passes, by [`LoadWork`]: a read of a
//!   pack (`sapphire_assets`' server, where a read's bytes land, or where its caller waits for them), a
//!   decompression (`Toc::decode_asset`), and an asset checked as its kind, which is all the fix-up a cooked
//!   asset gets (`AssetKind::references`). Handing decoded bytes to the GPU is the frame's bounded upload, and
//!   isn't one.
//!
//! The ledger is per thread, so only the frame's thread's own work counts, and a test's thread is its own. A
//! count is a thread-local add on a path that already reads or decodes: nothing per frame, nothing allocated.

use std::cell::Cell;

/// The asset pipeline's kinds of work, each counted apart, so a nonzero count says where.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LoadWork {
    /// A read of a pack's bytes: done here, or waited for here.
    Read,
    /// An asset's blocks decompressed and hashed.
    Decode,
    /// An asset checked as its kind: a cooked asset's whole fix-up, after which it's read in place.
    Build,
}

/// What the calling thread loaded while in play, by kind, since it started counting.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FrameLoads {
    pub reads: u64,
    pub decodes: u64,
    pub builds: u64,
}

impl FrameLoads {
    /// Every load, of any kind.
    #[must_use]
    pub fn total(&self) -> u64 {
        self.reads + self.decodes + self.builds
    }
}

thread_local! {
    static IN_PLAY: Cell<bool> = const { Cell::new(false) };
    static LOADS: Cell<FrameLoads> = const { Cell::new(FrameLoads { reads: 0, decodes: 0, builds: 0 }) };
}

/// The calling thread is the frame's, in play, until this is dropped: what it loads meanwhile counts.
#[must_use = "the thread is in play only while this lives"]
pub struct InPlay {
    was: bool,
}

impl InPlay {
    /// Marks the calling thread in play.
    pub fn begin() -> Self {
        Self { was: IN_PLAY.replace(true) }
    }
}

impl Drop for InPlay {
    fn drop(&mut self) {
        IN_PLAY.set(self.was);
    }
}

/// The calling thread's work is aside from play until this is dropped, if it was in play: what it loads
/// meanwhile doesn't count.
#[must_use = "the thread is aside only while this lives"]
pub struct Aside {
    was: bool,
}

impl Aside {
    /// Sets what the calling thread does from now on aside.
    pub fn begin() -> Self {
        Self { was: IN_PLAY.replace(false) }
    }
}

impl Drop for Aside {
    fn drop(&mut self) {
        IN_PLAY.set(self.was);
    }
}

/// Counts `work`, if the calling thread is in play.
#[inline]
pub fn count(work: LoadWork) {
    if IN_PLAY.get() {
        LOADS.with(|loads| {
            let mut counted = loads.get();
            match work {
                LoadWork::Read => counted.reads += 1,
                LoadWork::Decode => counted.decodes += 1,
                LoadWork::Build => counted.builds += 1,
            }
            loads.set(counted);
        });
    }
}

/// What the calling thread has loaded in play, since it started.
#[must_use]
pub fn loads() -> FrameLoads {
    LOADS.get()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_thread_in_play_counts_its_own_loads() {
        let start = loads();
        count(LoadWork::Decode);
        assert_eq!(loads(), start, "not in play");
        {
            let _play = InPlay::begin();
            count(LoadWork::Read);
            count(LoadWork::Decode);
            count(LoadWork::Build);
            count(LoadWork::Build);
            std::thread::spawn(|| count(LoadWork::Read)).join().unwrap();
            let aside = Aside::begin();
            count(LoadWork::Decode);
            drop(aside);
        }
        let counted = loads();
        let by_kind =
            (counted.reads - start.reads, counted.decodes - start.decodes, counted.builds - start.builds);
        assert_eq!(by_kind, (1, 1, 2), "another thread's own, and none aside or out of play");
        count(LoadWork::Read);
        assert_eq!(loads(), counted, "out of play again");
    }
}
