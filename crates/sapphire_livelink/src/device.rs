//! The device half: [`LiveLink`], which takes connections on threads of its own and hands what
//! they push to the app as [`Push`]es, once a frame; and [`ReloadTracker`], which answers pushed
//! packs once their reloads land.

use std::fmt;
use std::io::{self, BufReader, BufWriter, Write};
use std::net::{Ipv4Addr, Ipv6Addr, Shutdown, SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender, SyncSender, TrySendError};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use sapphire_assets::{AssetChange, AssetStats};
use sapphire_core::cvar::CvarRegistry;

use crate::LOG_TARGET;
use crate::packs::{LivePacks, PackPlan, PushedPack};
use crate::protocol::{
    AssetFailure, AssetGeneration, DEFAULT_PORT, LinkError, PackOutcome, Preamble, Reply, ReplyBody, Request,
    RequestBody, SourceFile, read_frame, write_frame,
};

/// How long a connection to a listening device has to send its preamble.
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(5);
/// How long a device that dials ([`LinkMode::Connect`]) waits before dialing again, and a
/// listening one before listening again after its socket failed.
const RETRY: Duration = Duration::from_secs(1);
/// Pushes waiting for the app. A full queue holds a connection's reader back, never the app.
const PUSH_QUEUE: usize = 16;
/// Replies waiting to be written to one connection. A tool that doesn't read its replies loses
/// those past this rather than growing the queue.
const REPLY_QUEUE: usize = 64;
/// Connections served at once; more are closed as they come.
const MAX_CONNECTIONS: u32 = 8;

/// How a device takes connections.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LinkMode {
    /// Listens at this address. The default, [`LinkMode::LOOPBACK`], takes connections from
    /// the same machine only: the desktop itself, the iOS Simulator's host, and USB forwards
    /// (`iproxy`, `adb forward`), which arrive on the device's loopback. An address on the LAN
    /// (`0.0.0.0:7811`) is an explicit opt-in: see the crate docs' security section.
    Listen(SocketAddr),
    /// Dials a tool listening at this address, and dials again whenever the connection drops:
    /// for `adb reverse`, where the device's loopback leads to the desktop.
    Connect(SocketAddr),
}

impl LinkMode {
    /// Listening on the loopback interface at [`DEFAULT_PORT`].
    pub const LOOPBACK: Self =
        Self::Listen(SocketAddr::new(std::net::IpAddr::V4(Ipv4Addr::LOCALHOST), DEFAULT_PORT));
}

impl Default for LinkMode {
    fn default() -> Self {
        Self::LOOPBACK
    }
}

/// Something a connection pushed, for the app to apply on its own thread, between frames, and
/// answer through its `reply`.
#[derive(Debug)]
pub enum Push {
    /// The app's state: answer with [`ReplyBody::Status`].
    Status { reply: Responder },
    /// A cvar to set: [`set_cvar`] applies it.
    Cvar { name: String, value: String, reply: Responder },
    /// A document whose envelope names `type_name`, for the app to hand to that type's owner.
    Document { origin: String, type_name: String, text: String, reply: Responder },
    /// Shader sources by file name, for the renderer.
    Shaders { files: Vec<SourceFile>, reply: Responder },
    /// A pack received whole: [`PushedPack::apply`] swaps it in, and a [`ReloadTracker`]
    /// answers once its reloads land.
    Pack { pack: PushedPack, reply: Responder },
    /// A frame to capture: the app reads back the next frame it draws and answers with
    /// [`ReplyBody::Captured`].
    Capture { reply: Responder },
}

/// Answers one request, on the connection it came from. Replying never blocks: the
/// connection's own thread writes it. A responder dropped without a reply answers nothing; the
/// tool waits until its timeout.
pub struct Responder {
    id: u32,
    replies: SyncSender<Reply>,
    shared: Arc<Shared>,
}

impl fmt::Debug for Responder {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Responder").field("id", &self.id).finish_non_exhaustive()
    }
}

impl Responder {
    pub fn reply(self, body: ReplyBody) {
        match &body {
            ReplyBody::Error(_) => self.shared.refused.fetch_add(1, Ordering::Relaxed),
            // Reads, which change nothing.
            ReplyBody::Status(_) | ReplyBody::PackNeed { .. } | ReplyBody::Captured(_) => 0,
            _ => self.shared.applied.fetch_add(1, Ordering::Relaxed),
        };
        // A closed connection no longer wants it; one whose tool reads none of its replies has it
        // dropped rather than queued without end.
        if let Err(TrySendError::Full(reply)) = self.replies.try_send(Reply { id: self.id, body }) {
            tracing::warn!(target: LOG_TARGET, id = reply.id, "live link reply dropped: the tool isn't reading");
        }
    }

    /// Replies with an error: the request changed nothing.
    pub fn refuse(self, message: impl Into<String>) {
        self.reply(ReplyBody::Error(message.into()));
    }
}

/// Sets cvar `name` from `value`, as the dev console does, and says how: its new value, or why it
/// kept the one it has (an unknown name, with a suggestion; a value of the wrong kind or out of
/// range).
pub fn set_cvar(cvars: &CvarRegistry, name: &str, value: &str) -> ReplyBody {
    match cvars.find(name).and_then(|cvar| cvar.set_text(value).map(|()| cvar)) {
        Ok(cvar) => ReplyBody::CvarSet { name: cvar.name().to_owned(), value: cvar.value().to_string() },
        Err(error) => ReplyBody::Error(error.to_string()),
    }
}

/// The link's counters, for the debug overlay and the status.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct LinkCounters {
    /// Tools connected now.
    pub connections: u32,
    /// Requests applied and refused.
    pub applied: u32,
    pub refused: u32,
    /// Pack bytes received.
    pub received: u64,
}

/// What every thread of a link shares.
#[derive(Default)]
struct Shared {
    closing: AtomicBool,
    /// Connections being served, handshakes included.
    serving: AtomicU32,
    /// Connections past their handshake.
    connections: AtomicU32,
    applied: AtomicU32,
    refused: AtomicU32,
    received: AtomicU64,
    /// The open connections, by number, to shut down when the link closes.
    streams: Mutex<Vec<(u64, TcpStream)>>,
    next_stream: AtomicU64,
}

/// The device half of live link (see the [crate docs](crate)): connections are served on
/// threads of its own, which receive and assemble pushes, and the app takes what they pushed with
/// [`Self::poll`] once a frame, applying it between frames.
pub struct LiveLink {
    mode: LinkMode,
    /// The address it listens at once bound (a port of 0 picks a free one), or dials.
    address: SocketAddr,
    /// Taken when the link closes, before its thread is joined: a connection's reader waiting
    /// to queue a push (16 wait already) then fails instead of waiting for a poll that won't
    /// come, which a dialing thread, serving its connection itself, would hold the join on.
    pushes: Option<Receiver<Push>>,
    shared: Arc<Shared>,
    thread: Option<JoinHandle<()>>,
    /// Dropped to wake a dialing thread between dials.
    wake: Option<Sender<()>>,
}

impl fmt::Debug for LiveLink {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("LiveLink")
            .field("mode", &self.mode)
            .field("address", &self.address)
            .finish_non_exhaustive()
    }
}

impl LiveLink {
    /// Starts taking connections as `mode` says, serving pushed packs through `packs`, the
    /// source the app's asset server reads. Fails if the address can't be listened at.
    pub fn start(mode: LinkMode, packs: Arc<LivePacks>) -> io::Result<Self> {
        let (sender, pushes) = mpsc::sync_channel(PUSH_QUEUE);
        let shared = Arc::new(Shared::default());
        let context = Context { shared: shared.clone(), packs, pushes: sender };
        let mut wake = None;
        let (address, thread) = match mode {
            LinkMode::Listen(address) => {
                let listener = TcpListener::bind(address)?;
                let address = listener.local_addr()?;
                let thread = thread::Builder::new()
                    .name("sapphire-live-link".into())
                    .spawn(move || accept(listener, address, &context))?;
                (address, thread)
            }
            LinkMode::Connect(address) => {
                let (sender, woken) = mpsc::channel();
                wake = Some(sender);
                let thread = thread::Builder::new()
                    .name("sapphire-live-link".into())
                    .spawn(move || dial(address, &context, &woken))?;
                (address, thread)
            }
        };
        match mode {
            LinkMode::Listen(_) if !address.ip().is_loopback() => tracing::warn!(
                target: LOG_TARGET,
                %address,
                "live link listening beyond this device: anyone who can reach it can push content and set cvars"
            ),
            LinkMode::Listen(_) => tracing::info!(target: LOG_TARGET, %address, "live link listening"),
            LinkMode::Connect(_) => tracing::info!(target: LOG_TARGET, %address, "live link dialing"),
        }
        Ok(Self { mode, address, pushes: Some(pushes), shared, thread: Some(thread), wake })
    }

    pub fn mode(&self) -> LinkMode {
        self.mode
    }

    /// The address it listens at (with the port it bound) or dials.
    pub fn address(&self) -> SocketAddr {
        self.address
    }

    /// The next push a connection received, if any. Call it once a frame until it returns `None`,
    /// or a few times (connections wait while 16 are queued): it allocates nothing while nothing
    /// came.
    pub fn poll(&self) -> Option<Push> {
        self.pushes.as_ref()?.try_recv().ok()
    }

    pub fn counters(&self) -> LinkCounters {
        LinkCounters {
            connections: self.shared.connections.load(Ordering::Relaxed),
            applied: self.shared.applied.load(Ordering::Relaxed),
            refused: self.shared.refused.load(Ordering::Relaxed),
            received: self.shared.received.load(Ordering::Relaxed),
        }
    }
}

impl Drop for LiveLink {
    fn drop(&mut self) {
        self.shared.closing.store(true, Ordering::Relaxed);
        let streams =
            std::mem::take(&mut *self.shared.streams.lock().unwrap_or_else(PoisonError::into_inner));
        for (_, stream) in streams {
            let _ = stream.shutdown(Shutdown::Both);
        }
        // Shutting a socket down wakes its reads and writes, not a reader waiting for room in
        // the push queue: dropping the queue does, and the pushes still in it go unanswered (the
        // tool times out), as a dropped `Responder` does.
        drop(self.pushes.take());
        let Some(thread) = self.thread.take() else { return };
        // A listener blocks in `accept`: a connection of our own wakes it. If none can be made,
        // the thread is left to end with the process rather than waited for.
        let woken = match self.mode {
            LinkMode::Listen(_) => {
                let ip = match self.address.ip() {
                    ip if !ip.is_unspecified() => ip,
                    std::net::IpAddr::V4(_) => Ipv4Addr::LOCALHOST.into(),
                    std::net::IpAddr::V6(_) => Ipv6Addr::LOCALHOST.into(),
                };
                TcpStream::connect_timeout(&SocketAddr::new(ip, self.address.port()), HANDSHAKE_TIMEOUT)
                    .is_ok()
            }
            LinkMode::Connect(_) => {
                drop(self.wake.take());
                true
            }
        };
        if woken {
            let _ = thread.join();
        }
    }
}

/// What a link's threads need.
#[derive(Clone)]
struct Context {
    shared: Arc<Shared>,
    packs: Arc<LivePacks>,
    pushes: SyncSender<Push>,
}

/// Serves each connection `listener` takes on a thread of its own, until the link closes. If the
/// socket fails, listens at `address` again: iOS reclaims a suspended app's listening sockets,
/// after which `accept` fails for good.
fn accept(mut listener: TcpListener, address: SocketAddr, context: &Context) {
    let shared = &context.shared;
    loop {
        let accepted = listener.accept();
        if shared.closing.load(Ordering::Relaxed) {
            return;
        }
        let stream = match accepted {
            Ok((stream, _)) => stream,
            // A connection reset before it was taken: nothing wrong with the socket.
            Err(error) if error.kind() == io::ErrorKind::ConnectionAborted => continue,
            Err(error) => {
                tracing::warn!(target: LOG_TARGET, %error, "live link's socket failed; listening again");
                drop(listener);
                listener = loop {
                    thread::sleep(RETRY);
                    if shared.closing.load(Ordering::Relaxed) {
                        return;
                    }
                    match TcpListener::bind(address) {
                        Ok(listener) => break listener,
                        Err(error) => {
                            tracing::debug!(target: LOG_TARGET, %error, %address, "live link can't listen yet")
                        }
                    }
                };
                continue;
            }
        };
        if shared.serving.load(Ordering::Relaxed) >= MAX_CONNECTIONS {
            tracing::warn!(target: LOG_TARGET, peer = ?stream.peer_addr().ok(), "live link serves {MAX_CONNECTIONS} connections already; closing one");
            continue;
        }
        shared.serving.fetch_add(1, Ordering::Relaxed);
        let context = context.clone();
        let spawned = thread::Builder::new().name("sapphire-live-link-connection".into()).spawn(move || {
            serve_logged(stream, &context, Some(HANDSHAKE_TIMEOUT));
            context.shared.serving.fetch_sub(1, Ordering::Relaxed);
        });
        if let Err(error) = spawned {
            shared.serving.fetch_sub(1, Ordering::Relaxed);
            tracing::warn!(target: LOG_TARGET, %error, "live link couldn't serve a connection");
        }
    }
}

/// Dials `address` and serves the connection, again whenever it drops, until the link closes,
/// which drops the sender of `wake`.
fn dial(address: SocketAddr, context: &Context, wake: &Receiver<()>) {
    while !context.shared.closing.load(Ordering::Relaxed) {
        // The handshake waits as long as the tool takes to accept: a dialed connection sits in
        // its backlog until then.
        if let Ok(stream) = TcpStream::connect_timeout(&address, RETRY) {
            serve_logged(stream, context, None);
        }
        // Whether the tool wasn't there or the connection ended, wait: behind `adb reverse` every
        // dial connects at once, to adbd, which closes it when no tool listens on the desktop.
        // A channel's timeout is the whole of it, where a thread's park may end early.
        match wake.recv_timeout(RETRY) {
            Err(RecvTimeoutError::Timeout) => {}
            Ok(()) | Err(RecvTimeoutError::Disconnected) => return,
        }
    }
}

fn serve_logged(stream: TcpStream, context: &Context, handshake: Option<Duration>) {
    let peer = stream.peer_addr().ok();
    match serve(stream, context, handshake) {
        Ok(()) => tracing::info!(target: LOG_TARGET, ?peer, "live link connection closed"),
        // Closed before its handshake: behind `adb reverse`, every dial while no tool listens.
        Err(LinkError::Closed) => tracing::debug!(target: LOG_TARGET, ?peer, "live link connection refused"),
        Err(error) => {
            tracing::warn!(target: LOG_TARGET, ?peer, error = error_chain(&error), "live link connection ended");
        }
    }
}

/// An open connection, which closing the link shuts down, and which is shut down when it is
/// dropped, so the peer sees it close however serving it ended.
struct Registered<'a> {
    shared: &'a Shared,
    number: u64,
    stream: TcpStream,
}

impl<'a> Registered<'a> {
    fn new(shared: &'a Shared, stream: &TcpStream) -> Result<Self, LinkError> {
        let number = shared.next_stream.fetch_add(1, Ordering::Relaxed);
        let registered = Self { shared, number, stream: stream.try_clone()? };
        shared.streams.lock().unwrap_or_else(PoisonError::into_inner).push((number, stream.try_clone()?));
        // Checked after registering: a link closing now either shut this one down or is seen here.
        if shared.closing.load(Ordering::Relaxed) {
            return Err(LinkError::Closed);
        }
        Ok(registered)
    }
}

impl Drop for Registered<'_> {
    fn drop(&mut self) {
        let _ = self.stream.shutdown(Shutdown::Both);
        self.shared
            .streams
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .retain(|(open, _)| *open != self.number);
    }
}

/// Counts a connection past its handshake while it lives.
struct Connected<'a>(&'a AtomicU32);

impl<'a> Connected<'a> {
    fn new(connections: &'a AtomicU32) -> Self {
        connections.fetch_add(1, Ordering::Relaxed);
        Self(connections)
    }
}

impl Drop for Connected<'_> {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::Relaxed);
    }
}

/// Serves one connection until it closes: the handshake, waiting at most `handshake` for the
/// peer's preamble, then requests, with replies written by a thread of the connection's own so
/// the app never waits on the network.
fn serve(stream: TcpStream, context: &Context, handshake: Option<Duration>) -> Result<(), LinkError> {
    let shared = &context.shared;
    let _registered = Registered::new(shared, &stream)?;
    stream.set_nodelay(true)?;
    // Each side sends its preamble first, so a tool of another version learns this one's even as
    // the connection is refused.
    (&stream).write_all(&Preamble::CURRENT.to_bytes())?;
    stream.set_read_timeout(handshake)?;
    Preamble::read(&mut &stream)?.check()?;
    stream.set_read_timeout(None)?;

    let (replies, outgoing) = mpsc::sync_channel(REPLY_QUEUE);
    let writer = stream.try_clone()?;
    thread::Builder::new()
        .name("sapphire-live-link-writer".into())
        .spawn(move || write_replies(writer, &outgoing))?;
    let _connected = Connected::new(&shared.connections);
    tracing::info!(target: LOG_TARGET, peer = ?stream.peer_addr().ok(), "live link connected");
    let mut session = Session { context, replies, push: None };
    let mut reader = BufReader::new(&stream);
    loop {
        if shared.closing.load(Ordering::Relaxed) {
            return Ok(());
        }
        match read_frame::<Request>(&mut reader) {
            Ok(request) => session.handle(request),
            Err(LinkError::Closed) => return Ok(()),
            Err(error) => return Err(error),
        }
    }
}

/// Writes replies as they come, until every responder of the connection is gone or it closes.
fn write_replies(stream: TcpStream, replies: &Receiver<Reply>) {
    let mut writer = BufWriter::new(&stream);
    while let Ok(reply) = replies.recv() {
        let written = write_frame(&mut writer, &reply).and_then(|()| Ok(writer.flush()?));
        if let Err(error) = written {
            tracing::debug!(target: LOG_TARGET, error = error_chain(&error), "live link reply not sent");
            return;
        }
    }
}

/// One connection's requests.
struct Session<'a> {
    context: &'a Context,
    replies: SyncSender<Reply>,
    /// The pack being pushed, or why its push failed.
    push: Option<Result<PackPlan, String>>,
}

impl Session<'_> {
    fn responder(&self, id: u32) -> Responder {
        Responder { id, replies: self.replies.clone(), shared: self.context.shared.clone() }
    }

    /// For the app's thread, waiting while it has [`PUSH_QUEUE`] pushes to apply.
    fn forward(&self, push: Push) {
        // The app dropped the link, and with it the receiver: it is closing.
        let _ = self.context.pushes.send(push);
    }

    fn handle(&mut self, request: Request) {
        let Request { id, body } = request;
        tracing::debug!(target: LOG_TARGET, id, ?body, "live link request");
        let reply = self.responder(id);
        match body {
            RequestBody::Status => self.forward(Push::Status { reply }),
            RequestBody::SetCvar { name, value } => self.forward(Push::Cvar { name, value, reply }),
            // The envelope names the type: a document that doesn't even have one is refused here.
            RequestBody::Document { origin, text } => match sapphire_serialize::document::type_name(&text) {
                Ok(type_name) => self.forward(Push::Document { origin, type_name, text, reply }),
                Err(error) => reply.refuse(format!("{origin}: {error}")),
            },
            RequestBody::Shaders { files } => self.forward(Push::Shaders { files, reply }),
            RequestBody::PackBegin { name, len, head } => {
                if let Some(Ok(plan)) = &self.push {
                    return reply.refuse(format!("pack {:?} is still being pushed", plan.name()));
                }
                match self.context.packs.plan(&name, len, head) {
                    Ok(plan) => {
                        reply.reply(ReplyBody::PackNeed { ranges: plan.needed().to_vec() });
                        self.push = Some(Ok(plan));
                    }
                    Err(error) => {
                        self.push = None;
                        reply.refuse(format!("pack {name:?}: {error}"));
                    }
                }
            }
            RequestBody::PackChunk { offset, bytes } => {
                self.context.shared.received.fetch_add(bytes.len() as u64, Ordering::Relaxed);
                match &mut self.push {
                    Some(Ok(plan)) => {
                        if let Err(error) = plan.receive(offset, bytes) {
                            self.push = Some(Err(format!("pack {:?}: {error}", plan.name())));
                        }
                    }
                    Some(Err(_)) => {}
                    None => self.push = Some(Err("a pack chunk came with no pack being pushed".to_owned())),
                }
            }
            RequestBody::PackEnd => match self.push.take() {
                Some(Ok(plan)) => {
                    let name = plan.name().to_owned();
                    match plan.finish() {
                        Ok(pack) => self.forward(Push::Pack { pack, reply }),
                        Err(error) => reply.refuse(format!("pack {name:?}: {error}")),
                    }
                }
                Some(Err(error)) => reply.refuse(error),
                None => reply.refuse("a pack end came with no pack being pushed"),
            },
            RequestBody::Capture => self.forward(Push::Capture { reply }),
        }
    }
}

/// Answers pushed packs once their reloads land, and keeps every reloaded asset's generation for
/// the status. The app feeds it the asset server's changes and counters once a frame.
///
/// A push is answered once the whole asset server is idle, so an app always loading answers late
/// (the tool gives up after `Client::PACK_TIMEOUT`, though the push applied), and a reload that
/// another trigger started meanwhile (desktop hot reload) is listed with the push's.
#[derive(Debug, Default)]
pub struct ReloadTracker {
    /// By asset ID.
    generations: Vec<AssetGeneration>,
    /// Applied pushes waiting for their reloads, with what they did so far.
    pending: Vec<(Responder, PackOutcome)>,
}

impl ReloadTracker {
    /// Waits for the reloads of a pack applied as `outcome` before answering through `reply`.
    pub fn track(&mut self, reply: Responder, outcome: PackOutcome) {
        self.pending.push((reply, outcome));
    }

    /// Takes the asset server's changes since the last frame (`AssetServer::take_changes`): new
    /// versions swapped in and those that failed go to every pack still waiting.
    pub fn record(&mut self, changes: &[AssetChange]) {
        for change in changes {
            match change {
                &AssetChange::Reloaded { id, kind, generation, .. } => {
                    let reloaded = AssetGeneration { id, kind, generation };
                    match self.generations.binary_search_by(|known| known.id.cmp(&id)) {
                        Ok(index) => self.generations[index] = reloaded,
                        Err(index) => self.generations.insert(index, reloaded),
                    }
                    for (_, outcome) in &mut self.pending {
                        outcome.reloaded.retain(|known| known.id != id);
                        outcome.reloaded.push(reloaded);
                    }
                }
                AssetChange::ReloadFailed { id, kind, error } => {
                    for (_, outcome) in &mut self.pending {
                        outcome.failed.push(AssetFailure { id: *id, kind: *kind, error: error_chain(error) });
                    }
                }
                // Counted in the remount's report.
                AssetChange::Removed { .. } => {}
            }
        }
    }

    /// Whether pushed packs wait for their reloads: [`Self::settle`] has work.
    pub fn waiting(&self) -> bool {
        !self.pending.is_empty()
    }

    /// Answers the waiting packs once the asset server has nothing loading or reloading: every
    /// new version they started is swapped in, or failed.
    pub fn settle(&mut self, stats: &AssetStats) {
        if self.pending.is_empty() || stats.loading + stats.waiting + stats.reloading > 0 {
            return;
        }
        for (reply, outcome) in self.pending.drain(..) {
            for asset in &outcome.reloaded {
                tracing::info!(target: LOG_TARGET, id = %asset.id, kind = %asset.kind, generation = asset.generation, "pushed asset swapped in");
            }
            for asset in &outcome.failed {
                tracing::warn!(target: LOG_TARGET, id = %asset.id, kind = %asset.kind, error = asset.error, "pushed asset kept its version");
            }
            reply.reply(ReplyBody::PackApplied(outcome));
        }
    }

    /// Every asset replaced since the app started, by ID, with the generation it now is.
    pub fn generations(&self) -> &[AssetGeneration] {
        &self.generations
    }
}

/// An error and its sources, as one line.
fn error_chain(error: &dyn std::error::Error) -> String {
    let mut message = error.to_string();
    let mut source = error.source();
    while let Some(cause) = source {
        message.push_str(": ");
        message.push_str(&cause.to_string());
        source = cause.source();
    }
    message
}
