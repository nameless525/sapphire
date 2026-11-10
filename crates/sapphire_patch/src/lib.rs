//! Patches' download (M4 P2, m4-plan §3 "What a patch moves", "What's signed" and "HTTP"): what a new
//! release lacks, fetched by hash from a static host, checked before it's kept, and resumed after a kill.
//!
//! # What a download does
//!
//! A [`Download`] runs on a thread of its own (`patcher`) and goes through three phases ([`Phase`]):
//!
//! 1. **Checking:** it fetches the host's `release.smanifest`, capped at the most a manifest may be, and
//!    asks the installed release to take it as its successor ([`Manifest::accept_successor`]): signed by
//!    a key the installed release names, newer, unexpired at the caller's clock, for this app build and
//!    cook. One that isn't is refused before anything else is fetched. The same version as the installed one is
//!    [`Outcome::UpToDate`]. The new release must leave every file a patch may not change as it was
//!    ([`Plan::new`]).
//! 2. **Reading:** each pack the install doesn't hold by header hash is a changed pack, and its tables (the
//!    header and table of contents, the bytes before its data) come by range request, each checked against
//!    the hash the manifest names.
//! 3. **Fetching:** each changed file outside the packs, whole, and each block of a changed pack whose hash
//!    no installed pack holds, once per hash ([`Plan::blocks`]). Blocks that lie next to each other in one
//!    pack go in one range request of up to 1 MiB, the alignment padding between them included (at most 15
//!    bytes a block); each block's blake3 is checked before it's written.
//!
//! So a patch moves exactly the new manifest, the changed packs' tables, the blocks whose hashes are new
//! (and the padding between neighbours), and the changed files: exit test 2's count. Every read is capped
//! at the size the manifest or a table gives, so a host can't make the patcher read or keep more.
//!
//! # A host that misbehaves
//!
//! A dropped connection, a short or long body, wrong bytes, an ignored `Range` and a stall
//! ([`Timeouts`]) each fail the request they hit ([`FetchError`]). Failed requests are tried again, with a
//! wait that doubles ([`Retries`]); blocks that arrived whole and right before a failure are kept. An item
//! still failing after its attempts stops the download with [`DownloadError::Host`], naming it, and
//! what was kept stays for the next run. Nothing the host sends is trusted without its hash or signature,
//! and nothing it sends can panic the patcher.
//!
//! # Staging, and resuming
//!
//! A download stages what it fetches in a directory of its own ([`Staging`], which has the layout): the
//! accepted manifest, written durably (`sapphire_platform::durable_replace`), and every table, block and
//! file under its hash. That is the whole of its state. Nothing there is trusted for being there: the
//! manifest is accepted again on every run, from the host, and each staged file is hashed before it
//! counts, so a kill at any moment, mid-write included, leaves at worst a file that's fetched again. A
//! killed or cancelled download started again fetches nothing it had already checked and written.
//!
//! # Progress and cancelling
//!
//! [`Download::progress`] reads atomics, never a lock, so a boot screen can ask every frame:
//! the phase, the bytes checked and kept so far and the bytes the download moves in all, staged ones
//! included, so a resumed download starts part way. [`Download::cancel`] stops it at its next read; a
//! stalled read ends at its timeout. Dropping a [`Download`] cancels it without waiting.
//!
//! # Transport
//!
//! [`Transport`] is the one thing that speaks HTTP, so another client can slot in (m4-plan §3's undo:
//! `reqwest`). [`HttpTransport`] is `ureq` 3, blocking, on `rustls` with the `ring` provider. It trusts
//! the OS's verifier everywhere but Android, where it trusts Mozilla's roots, compiled in ([`Roots`]).
//! Integrity never rests on TLS: the signed manifest and every block's hash carry it.
//!
//! # Not here
//!
//! Installing what was staged is M4 P3's: assembling the new packs from kept and fetched blocks
//! ([`Installed::block`] says where a kept one lies), checking them whole, and switching to them. When to
//! check for a patch, the clock, the app build and the host are the game's (M4 P4).

//!
//! [`Manifest::accept_successor`]: sapphire_assets::manifest::Manifest::accept_successor

mod download;
mod http;
mod plan;
mod staging;
mod transport;

pub use download::{Download, DownloadError, Outcome, Phase, Progress, Request, Retries, Staged};
pub use http::{HttpError, HttpTransport, Roots, Timeouts};
pub use plan::{Installed, InstalledBlock, InstalledError, Plan, PlanError, PlannedBlock};
pub use staging::{Staging, StagingError};
pub use transport::{Body, FetchError, Transport};

/// The host's path of a release's current manifest: `release.smanifest`, which a release writes last.
pub const MANIFEST_PATH: &str = sapphire_assets::manifest::FILE_NAME;

/// The host's path of the pack whose header hash is `header_hash`: `packs/<hash in hex>.spack`.
pub fn pack_path(header_hash: &[u8; 32]) -> String {
    format!("packs/{}.{}", hex(header_hash), sapphire_assets::pack::PACK_EXTENSION)
}

/// The host's path of the file outside the packs whose blake3 is `hash`: `files/<hash in hex>`.
pub fn file_path(hash: &[u8; 32]) -> String {
    format!("files/{}", hex(hash))
}

fn hex(hash: &[u8; 32]) -> String {
    blake3::Hash::from_bytes(*hash).to_hex().to_string()
}
