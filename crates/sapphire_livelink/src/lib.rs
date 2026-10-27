//! Live link for development builds (spec §5.29, M1 A9): a dev build takes TCP connections from
//! the desktop, which pushes cooked packs, documents, shader sources and cvars to it; the device
//! applies each through the calls desktop hot reload uses (M1 A8) and answers how it went. The
//! desktop can also ask for the next frame the device draws (M1 X2's rehearsal), which is how it
//! sees what a phone shows.
//!
//! A `dev_only` crate (`xtask/layers.toml`): apps reach it through their `dev` feature (rule R8),
//! so shipping builds never link it. It runs on every platform, phones included: a phone has no
//! cook directory to watch, so live link is how an edit reaches it (the M1 plan's exit test 1).
//!
//! # Owns
//!
//! - The [`protocol`]: a versioned preamble, then length-prefixed postcard frames of requests and
//!   replies. A peer of another protocol version is refused, and both sides say which versions
//!   met.
//! - The device half. [`LiveLink`] listens (or dials, [`LinkMode`]) on a thread of its own and
//!   serves each connection on threads of its own, which read requests, receive and assemble
//!   pushed packs and write replies; the app takes what they pushed ([`Push`]) once a frame and
//!   applies it between frames, so the frame never waits on the network. [`LivePacks`] is the pack
//!   source the app's asset server reads through, serving pushed packs over the app's own, and
//!   [`ReloadTracker`] answers a pushed pack once its reloads land. [`set_cvar`] sets a pushed
//!   cvar through the app's `CvarRegistry`.
//! - The desktop half. [`Client`], and the `sapphire_livelink` tool built on it (`src/main.rs`),
//!   which `cargo xtask link` builds and runs: `status`, `cvar`, `push` (packs, documents,
//!   shaders), `capture` (the device's next frame, as a PNG) and `watch`, which pushes what
//!   changes: packs after `cargo xtask content`, edited documents and WGSL sources. Each command
//!   says how long the device took to answer.
//!
//! # Does not own
//!
//! - Drawing captures: the app draws the frame a capture asks for into a texture it reads back,
//!   and compresses it ([`protocol::Capture`]).
//! - Applying documents and shaders: the app hands a document to the owner of the type its
//!   envelope names (time of day's `TimeOfDay::set_timeline`, the character light rig presets'
//!   library, the screen effect presets' library), and shader sources to
//!   `Renderer::set_shader_sources`, as its hot reload does; each keeps what it has if the new
//!   one doesn't parse, validate or compile.
//! - Reloading assets: the asset server's remount (`sapphire_assets::AssetServer::remount`) and
//!   the app's uploader, which replaces each new version in place by its generation.
//! - Watching files: A8's `sapphire_hot_reload::FileWatcher`, which the tool's `watch` uses, with
//!   that crate's paths to the engine's sources.
//! - Cooking: `cargo xtask content --cook <cook>` writes the packs the tool pushes.
//!
//! # Packs
//!
//! A push sends only the assets the device doesn't already have. The tool sends the new pack's
//! header and table of contents; the device compares them with the version it serves, asks for
//! the byte ranges it lacks, and the tool streams those in chunks (at most
//! [`protocol::CHUNK_LEN`] each), which the connection's thread gathers off the frame. The app
//! then serves the new version (assembled from those bytes and the old version's) and remounts
//! it, and the asset server reloads exactly the assets whose blake3 hashes changed, behind their
//! handles: the path hot reload takes on desktop. `packs` has the details. A pack the device
//! refuses, or an asset whose new version fails, keeps the version it had.
//!
//! # Transports
//!
//! Plain TCP to an address. Devices listen at `127.0.0.1:7811` by default
//! ([`LinkMode::LOOPBACK`], [`protocol::DEFAULT_PORT`]):
//!
//! - **Desktop:** the tool connects to `127.0.0.1:7811`.
//! - **iOS Simulator:** simulated apps share the Mac's network stack, so the same.
//! - **iPhone over USB:** `iproxy 7811 7811` (libimobiledevice) forwards the Mac's port 7811 to
//!   the phone's, through usbmuxd; the tool connects to `127.0.0.1:7811`.
//! - **Android over USB:** either `adb forward tcp:7811 tcp:7811` with the app listening, the
//!   tool connecting to `127.0.0.1:7811`; or `adb reverse tcp:7811 tcp:7811` with the app
//!   dialing (`--live-link connect=127.0.0.1:7811`) and the tool listening
//!   (`--listen 127.0.0.1:7811`).
//! - **Wi-Fi:** the app listens on the LAN (`--live-link listen=0.0.0.0:7811`) and the tool
//!   connects to the device's address (`--addr 192.168.1.20:7811`). An iPhone may ask for local
//!   network access (`NSLocalNetworkUsageDescription`), which device builds' Info.plist will
//!   carry with signing (M4).
//!
//! # Security
//!
//! Anyone who can connect can replace the app's content, shaders and cvars: there is no
//! authentication or encryption. So the device listens on its loopback interface by default,
//! which only the machine itself and USB forwards reach; listening on the LAN is an explicit
//! opt-in, logged as a warning, for trusted networks only; and only development builds have any of
//! it. A malformed request can't make the device allocate unboundedly or hold its frame: frames
//! are capped ([`protocol::MAX_FRAME`]), pushes too ([`packs::MAX_PUSH`]), their buffers grow only
//! as bytes come, queues of pushes and replies are bounded, so are connections, the app applies a
//! few pushes a frame, and every pushed byte an asset is decoded from passes the pack's blake3
//! checks first.
//!
//! # Cost
//!
//! Idle, a listening link is one thread blocked in `accept`, and [`LiveLink::poll`] one empty
//! channel read a frame, allocating nothing. The sandbox's bench measures it (spec §5.29's M1 A9
//! note). Reads of packs go through [`LivePacks`]: one lock and a table lookup each.
//!
//! # Later milestones
//!
//! - Device to desktop: logs and Tracy (spec §5.29); a stats stream rather than the status the
//!   tool asks for; captures larger than a frame sent in pieces rather than halved.
//! - Pushing editor change records (spec §5.28), replay files, the quality tier table and input
//!   bindings.
//! - A pairing token before LAN use is routine, and taking a pushed pack back to the app's own.

pub mod client;
mod device;
pub mod packs;
pub mod protocol;

pub use client::Client;
pub use device::{LinkCounters, LinkMode, LiveLink, Push, ReloadTracker, Responder, set_cvar};
pub use packs::{LivePacks, PushedPack};
pub use protocol::{DEFAULT_PORT, LinkError, PROTOCOL_VERSION};

/// Log target for this crate.
pub(crate) const LOG_TARGET: &str = "sapphire::livelink";
