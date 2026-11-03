//! Saving's other half: a thread of its own that talks to a device over live link, so the interface never
//! waits on a network. It keeps a [`Client`] connected, pushes what it's given, and asks the device what
//! it's playing twice a second, which is how the editor knows an edit reached an attempt rather than
//! being told it should have.
//!
//! A reply can take seconds, and the text and the address can change meanwhile. So every push has an ID and
//! every report says which address it's about: [`Link::last_push`] only ever speaks for the text and the
//! device in front of the person, [`Link::earlier_push`] holds what came back too late to, and nothing an
//! old device says lands on a new one.
//!
//! Nothing piles up behind a slow device or a missing one, where each push can take the connection's whole
//! deadline: the thread holds one push of each file at most, the newest save, and a new address sends back
//! what was waiting for the old one. So the newest text waits for what's already out and no more, and what
//! never went says so in [`Link::earlier_push`].

use std::net::SocketAddr;
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use sapphire_livelink::protocol::{CvarStatus, DeviceStatus, MoveAttempt};
use sapphire_livelink::{Client, LinkError};
use sapphire_time::Stopwatch;

use crate::edits::error_chain;

/// How often a connected device is asked what it's doing. Slow enough to cost it nothing and quick enough
/// that a move of half a second shows.
const STATUS_EVERY: Duration = Duration::from_millis(250);
/// How often a panel that's showing cvars has them looked at again: values change on the device too.
const CVARS_EVERY: Duration = Duration::from_secs(1);
/// How often to try for a device that isn't there.
const RECONNECT_EVERY: Duration = Duration::from_secs(1);
/// What a save that never went came to, when a newer save of its file took its place.
const REPLACED: &str = "a newer save went in its place";
/// What a save that never went came to, when the link moved to another device first.
const ADDRESS_MOVED: &str = "the device changed before it went";

/// One thing wrong with a pushed document, by the words of its refusal.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Refusal {
    pub in_move: Option<String>,
    pub track: Option<String>,
    pub message: String,
}

/// Splits what a device said into what's wrong, naming move and track where it did. A refusal of moves is
/// `sapphire_moves::Issue`s, one a line, and a device may put the file's name before them.
#[must_use]
pub fn refusals_of(text: &str) -> Vec<Refusal> {
    text.lines().filter(|line| !line.trim().is_empty()).map(refusal_of_line).collect()
}

fn refusal_of_line(line: &str) -> Refusal {
    let in_quotes = |text: &str| -> Option<(String, String)> {
        let text = text.strip_prefix('`')?;
        let (inside, rest) = text.split_once('`')?;
        Some((inside.to_owned(), rest.to_owned()))
    };
    if let Some(at) = line.find("move `") {
        let named = &line[at + "move ".len()..];
        if let Some((name, rest)) = in_quotes(named)
            && let Some(rest) = rest.strip_prefix(", track ")
            && let Some((track, rest)) = in_quotes(rest)
            && let Some(message) = rest.strip_prefix(": ")
        {
            return Refusal { in_move: Some(name), track: Some(track), message: message.to_owned() };
        }
    }
    for (at, _) in line.match_indices('`') {
        if at == 0 || line[..at].ends_with(": ") {
            if let Some((track, rest)) = in_quotes(&line[at..])
                && let Some(message) = rest.strip_prefix(": ")
            {
                return Refusal { in_move: None, track: Some(track), message: message.to_owned() };
            }
            break;
        }
    }
    Refusal { in_move: None, track: None, message: line.to_owned() }
}

/// What became of a push.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Pushed {
    /// The device took it: what it replaced, in its words, and how long it took to say so.
    Took { owner: String, milliseconds: u32 },
    /// The device kept what it had, and said why.
    Refused(Vec<Refusal>),
    /// It never got that far, or the answer didn't come back.
    Failed(String),
}

/// What an older push came to, once what it pushed had stopped being what's here.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EarlierPush {
    /// The file it was of.
    pub origin: String,
    pub pushed: Pushed,
}

/// A push the thread hasn't begun.
#[derive(Debug)]
struct WaitingPush {
    id: u64,
    origin: String,
    text: String,
}

/// What waits for the thread, and all that can.
#[derive(Debug, Default)]
struct Waiting {
    /// The address to move to, with the count of addresses so far, which the reports made after this repeat.
    address: Option<(u64, Option<SocketAddr>)>,
    /// To the newest address, oldest first, one of each file.
    pushes: Vec<WaitingPush>,
    /// Cvars to set, oldest first, one of each name: a value dragged sends the newest and no more.
    cvars: Vec<(String, String)>,
    /// A panel has just begun to show cvars, and wants them now.
    list_cvars: bool,
    /// A panel is showing cvars, so they're worth asking for.
    wants_cvars: bool,
    /// The link has gone.
    closed: bool,
}

impl Waiting {
    fn has_work(&self) -> bool {
        self.closed
            || self.address.is_some()
            || !self.pushes.is_empty()
            || !self.cvars.is_empty()
            || self.list_cvars
    }
}

/// What the thread does next.
enum Next {
    Move {
        target: u64,
        address: Option<SocketAddr>,
    },
    Push(WaitingPush),
    SetCvar(String, String),
    ListCvars,
    /// Nothing came in time.
    Quiet,
    Stop,
}

/// Where the interface leaves work for the thread: held only to put something in or take it out, so the
/// interface never waits on it for longer than that, whatever the thread is waiting on.
#[derive(Debug, Default)]
struct Mailbox {
    waiting: Mutex<Waiting>,
    wake: Condvar,
}

impl Mailbox {
    fn lock(&self) -> MutexGuard<'_, Waiting> {
        self.waiting.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Leaves `push`, returning the push of its file it takes the place of, if one was waiting.
    fn put_push(&self, push: WaitingPush) -> Option<WaitingPush> {
        let mut waiting = self.lock();
        let replaced = waiting
            .pushes
            .iter()
            .position(|other| other.origin == push.origin)
            .map(|index| waiting.pushes.remove(index));
        waiting.pushes.push(push);
        self.wake.notify_one();
        replaced
    }

    /// Leaves a new address, returning the pushes that were waiting for the old one.
    fn put_address(&self, target: u64, address: Option<SocketAddr>) -> Vec<WaitingPush> {
        let mut waiting = self.lock();
        waiting.address = Some((target, address));
        self.wake.notify_one();
        std::mem::take(&mut waiting.pushes)
    }

    /// Leaves a cvar to set, in place of any set of that name still waiting.
    fn put_cvar(&self, name: String, value: String) {
        let mut waiting = self.lock();
        waiting.cvars.retain(|(other, _)| *other != name);
        waiting.cvars.push((name, value));
        self.wake.notify_one();
    }

    /// Says whether a panel is showing cvars. One that's just begun gets them now.
    fn want_cvars(&self, wanted: bool) {
        let mut waiting = self.lock();
        if wanted && !waiting.wants_cvars {
            waiting.list_cvars = true;
            self.wake.notify_one();
        }
        waiting.wants_cvars = wanted;
    }

    fn close(&self) {
        self.lock().closed = true;
        self.wake.notify_one();
    }

    /// The next thing to do, waiting up to `wait` for one. Going to another device comes first.
    fn next(&self, wait: Duration) -> Next {
        let waiting = self.lock();
        let (mut waiting, _) = self
            .wake
            .wait_timeout_while(waiting, wait, |waiting| !waiting.has_work())
            .unwrap_or_else(PoisonError::into_inner);
        if waiting.closed {
            Next::Stop
        } else if let Some((target, address)) = waiting.address.take() {
            Next::Move { target, address }
        } else if !waiting.pushes.is_empty() {
            Next::Push(waiting.pushes.remove(0))
        } else if !waiting.cvars.is_empty() {
            let (name, value) = waiting.cvars.remove(0);
            Next::SetCvar(name, value)
        } else if std::mem::take(&mut waiting.list_cvars) {
            Next::ListCvars
        } else {
            Next::Quiet
        }
    }
}

enum Report {
    /// A new connection, which may be to a device that has started again since the last.
    Connected {
        target: u64,
    },
    Connection {
        target: u64,
        status: Result<Box<DeviceStatus>, String>,
    },
    Pushed {
        id: u64,
        origin: String,
        pushed: Pushed,
    },
    /// A cvar's value as the device has it now, or why it kept the one it had.
    CvarSet {
        target: u64,
        name: String,
        value: Result<String, String>,
    },
    Cvars {
        target: u64,
        list: Result<Vec<CvarStatus>, String>,
    },
}

/// A connection to a device, as the interface sees it: it never blocks.
#[derive(Debug)]
pub struct Link {
    mailbox: Arc<Mailbox>,
    reports: Receiver<Report>,
    worker: Option<JoinHandle<()>>,
    address: Option<SocketAddr>,
    /// Which address this is: a report of any other is about a device no longer looked at.
    target: u64,
    /// The device's last status, while it's connected.
    device: Option<DeviceStatus>,
    /// Why there's no device, if there isn't.
    trouble: Option<String>,
    /// Pushes with no answer yet: the one the thread is on, and those waiting for it.
    waiting: u32,
    /// The ID the next push takes.
    next_push: u64,
    /// Pushes from this one on are of the text and to the device now in front of the person.
    current_from: u64,
    last_push: Option<Pushed>,
    earlier_push: Option<EarlierPush>,
    /// The last attempt seen of each actor on this connection: a move can end between two statuses.
    attempts: Vec<MoveAttempt>,
    /// What the device last said it has, while a panel asks.
    cvars: Option<Vec<CvarStatus>>,
    /// The last cvar the device wouldn't take, and why.
    cvar_refusal: Option<(String, String)>,
}

impl Link {
    /// A link talking to `address`, or to nothing yet.
    #[must_use]
    pub fn new(address: Option<SocketAddr>) -> Self {
        let mailbox = Arc::new(Mailbox::default());
        let (reports_out, reports) = mpsc::channel();
        let worker = thread::Builder::new()
            .name("live link".to_owned())
            .spawn({
                let mailbox = mailbox.clone();
                move || work(address, &mailbox, &reports_out)
            })
            .ok();
        Self {
            mailbox,
            reports,
            worker,
            address,
            target: 0,
            device: None,
            trouble: None,
            waiting: 0,
            next_push: 0,
            current_from: 0,
            last_push: None,
            earlier_push: None,
            attempts: Vec::new(),
            cvars: None,
            cvar_refusal: None,
        }
    }

    /// Points it at another device, if this one isn't it. Nothing said so far speaks for the new one.
    pub fn set_address(&mut self, address: Option<SocketAddr>) {
        if address == self.address {
            return;
        }
        self.address = address;
        self.target += 1;
        self.device = None;
        self.attempts.clear();
        self.cvars = None;
        self.cvar_refusal = None;
        self.clear_push();
        if !self.thread_runs() {
            return;
        }
        for push in self.mailbox.put_address(self.target, address) {
            self.waiting = self.waiting.saturating_sub(1);
            self.never_went(push.origin, ADDRESS_MOVED);
        }
    }

    /// Pushes a document, by its file's name, in place of any save of that file still waiting for the
    /// thread. [`Self::last_push`] says what became of it, as long as nothing has changed by the time it's
    /// known.
    pub fn push(&mut self, origin: String, text: String) {
        let id = self.next_push;
        self.next_push += 1;
        if !self.thread_runs() {
            return;
        }
        self.waiting += 1;
        if let Some(replaced) = self.mailbox.put_push(WaitingPush { id, origin, text }) {
            self.waiting = self.waiting.saturating_sub(1);
            // The same text needs no word: this push answers for it.
            if replaced.id < self.current_from {
                self.never_went(replaced.origin, REPLACED);
            }
        }
    }

    /// Sets a cvar on the device, in place of any set of that name still waiting. What's shown takes the value
    /// at once, and [`Self::cvar_refusal`] says if the device wouldn't have it.
    pub fn set_cvar(&mut self, name: &str, value: String) {
        if let Some(cvar) = self.cvars.iter_mut().flatten().find(|cvar| cvar.name == name) {
            cvar.value.clone_from(&value);
        }
        self.cvar_refusal = None;
        if self.thread_runs() {
            self.mailbox.put_cvar(name.to_owned(), value);
        }
    }

    /// Says whether a panel is showing cvars: the device is asked for them only then.
    pub fn want_cvars(&self, wanted: bool) {
        self.mailbox.want_cvars(wanted);
    }

    /// What the device last said it has, if it's been asked.
    #[must_use]
    pub fn cvars(&self) -> Option<&[CvarStatus]> {
        self.cvars.as_deref()
    }

    /// The last cvar the device wouldn't take, and why.
    #[must_use]
    pub fn cvar_refusal(&self) -> Option<&(String, String)> {
        self.cvar_refusal.as_ref()
    }

    fn never_went(&mut self, origin: String, why: &str) {
        self.earlier_push = Some(EarlierPush { origin, pushed: Pushed::Failed(why.to_owned()) });
    }

    /// Whether the thread is there to take work. If it isn't, nothing out will be answered, and this says so.
    fn thread_runs(&mut self) -> bool {
        let runs = self.worker.as_ref().is_some_and(|worker| !worker.is_finished());
        if !runs {
            self.waiting = 0;
            self.last_push = Some(Pushed::Failed("live link's thread has stopped".to_owned()));
        }
        runs
    }

    /// Takes what the thread has reported. Call it once a frame.
    pub fn poll(&mut self) {
        loop {
            match self.reports.try_recv() {
                Ok(Report::Connected { target }) if target == self.target => self.attempts.clear(),
                Ok(Report::Connection { target, status: Ok(status) }) if target == self.target => {
                    self.trouble = None;
                    for attempt in &status.attempts {
                        match self.attempts.iter_mut().find(|seen| seen.actor == attempt.actor) {
                            Some(seen) => *seen = attempt.clone(),
                            None => self.attempts.push(attempt.clone()),
                        }
                    }
                    self.device = Some(*status);
                }
                Ok(Report::Connection { target, status: Err(why) }) if target == self.target => {
                    self.device = None;
                    self.trouble = Some(why);
                }
                Ok(Report::CvarSet { target, name, value }) if target == self.target => match value {
                    Ok(value) => {
                        if let Some(cvar) = self.cvars.iter_mut().flatten().find(|cvar| cvar.name == name) {
                            cvar.value = value;
                        }
                    }
                    Err(why) => self.cvar_refusal = Some((name, why)),
                },
                Ok(Report::Cvars { target, list }) if target == self.target => match list {
                    Ok(list) => self.cvars = Some(list),
                    Err(_) => self.cvars = None,
                },
                Ok(
                    Report::Connected { .. }
                    | Report::Connection { .. }
                    | Report::CvarSet { .. }
                    | Report::Cvars { .. },
                ) => {}
                Ok(Report::Pushed { id, origin, pushed }) => {
                    self.waiting = self.waiting.saturating_sub(1);
                    if id >= self.current_from {
                        self.last_push = Some(pushed);
                        self.earlier_push = None;
                    } else {
                        self.earlier_push = Some(EarlierPush { origin, pushed });
                    }
                }
                Err(TryRecvError::Empty | TryRecvError::Disconnected) => return,
            }
        }
    }

    #[must_use]
    pub fn device(&self) -> Option<&DeviceStatus> {
        self.device.as_ref()
    }

    #[must_use]
    pub fn trouble(&self) -> Option<&str> {
        self.trouble.as_deref()
    }

    #[must_use]
    pub fn is_waiting(&self) -> bool {
        self.waiting > 0
    }

    /// What became of the last push of the text and to the device there are now.
    #[must_use]
    pub fn last_push(&self) -> Option<&Pushed> {
        self.last_push.as_ref()
    }

    /// What became of a push that was overtaken while it was out: true of what it pushed, and of nothing
    /// here now. It stays until a current answer replaces it.
    #[must_use]
    pub fn earlier_push(&self) -> Option<&EarlierPush> {
        self.earlier_push.as_ref()
    }

    /// The last attempt seen of each actor.
    #[must_use]
    pub fn attempts(&self) -> &[MoveAttempt] {
        &self.attempts
    }

    /// Forgets what the last push said: the document has changed since, and it no longer describes it. Nor
    /// does the answer to any push still out.
    pub fn clear_push(&mut self) {
        self.last_push = None;
        self.current_from = self.next_push;
    }
}

impl Drop for Link {
    fn drop(&mut self) {
        self.mailbox.close();
        // Its longest wait is a connection's deadline, so it isn't waited for.
        drop(self.worker.take());
    }
}

fn work(mut address: Option<SocketAddr>, mailbox: &Mailbox, reports: &Sender<Report>) {
    let mut client: Option<Client> = None;
    let mut target = 0;
    let mut since_try = Stopwatch::start();
    let mut since_cvars = Stopwatch::start();
    loop {
        match mailbox.next(STATUS_EVERY) {
            Next::Move { target: next_target, address: next } => {
                target = next_target;
                address = next;
                client = None;
                since_try = Stopwatch::start();
                if address.is_none() {
                    let status = Err("no address".to_owned());
                    let _ = reports.send(Report::Connection { target, status });
                }
            }
            Next::Push(WaitingPush { id, origin, text }) => {
                let pushed = push(address, target, &mut client, reports, &origin, text);
                if reports.send(Report::Pushed { id, origin, pushed }).is_err() {
                    return;
                }
            }
            Next::SetCvar(name, value) => {
                let value = set_cvar(address, target, &mut client, reports, &name, &value);
                if reports.send(Report::CvarSet { target, name, value }).is_err() {
                    return;
                }
            }
            Next::ListCvars => {
                since_cvars = Stopwatch::start();
                let list = list_cvars(address, target, &mut client, reports);
                if reports.send(Report::Cvars { target, list }).is_err() {
                    return;
                }
            }
            Next::Quiet => {
                if client.is_some() && mailbox.lock().wants_cvars && since_cvars.elapsed() >= CVARS_EVERY {
                    since_cvars = Stopwatch::start();
                    let list = list_cvars(address, target, &mut client, reports);
                    if reports.send(Report::Cvars { target, list }).is_err() {
                        return;
                    }
                }
                if client.is_none() && since_try.elapsed() < RECONNECT_EVERY {
                    continue;
                }
                since_try = Stopwatch::start();
                let Some(address) = address else { continue };
                let status = connected(address, target, &mut client, reports)
                    .and_then(|client| client.status().map(Box::new).map_err(|error| error_chain(&error)));
                if client.as_ref().is_some_and(Client::is_closed) || status.is_err() {
                    client = None;
                }
                if reports.send(Report::Connection { target, status }).is_err() {
                    return;
                }
            }
            Next::Stop => return,
        }
    }
}

fn connected<'c>(
    address: SocketAddr,
    target: u64,
    client: &'c mut Option<Client>,
    reports: &Sender<Report>,
) -> Result<&'c mut Client, String> {
    if client.is_none() {
        *client = Some(Client::connect(address).map_err(|error| error_chain(&error))?);
        let _ = reports.send(Report::Connected { target });
    }
    client.as_mut().ok_or_else(|| "no connection".to_owned())
}

fn set_cvar(
    address: Option<SocketAddr>,
    target: u64,
    client: &mut Option<Client>,
    reports: &Sender<Report>,
    name: &str,
    value: &str,
) -> Result<String, String> {
    let address = address.ok_or_else(|| "no device address".to_owned())?;
    let connection = connected(address, target, client, reports)?;
    match connection.set_cvar(name, value) {
        Ok((_, value)) => Ok(value),
        Err(LinkError::Refused(why)) => Err(why),
        Err(error) => {
            *client = None;
            Err(error_chain(&error))
        }
    }
}

fn list_cvars(
    address: Option<SocketAddr>,
    target: u64,
    client: &mut Option<Client>,
    reports: &Sender<Report>,
) -> Result<Vec<CvarStatus>, String> {
    let address = address.ok_or_else(|| "no device address".to_owned())?;
    let connection = connected(address, target, client, reports)?;
    match connection.cvars() {
        Ok(list) => Ok(list),
        Err(LinkError::Refused(why)) => Err(why),
        Err(error) => {
            *client = None;
            Err(error_chain(&error))
        }
    }
}

fn push(
    address: Option<SocketAddr>,
    target: u64,
    client: &mut Option<Client>,
    reports: &Sender<Report>,
    origin: &str,
    text: String,
) -> Pushed {
    let Some(address) = address else {
        return Pushed::Failed("no device address to push to".to_owned());
    };
    let stopwatch = Stopwatch::start();
    let connection = match connected(address, target, client, reports) {
        Ok(connection) => connection,
        Err(why) => return Pushed::Failed(format!("no device at {address}: {why}")),
    };
    match connection.push_document(origin, text) {
        Ok(owner) => Pushed::Took { owner, milliseconds: stopwatch.elapsed().as_millis() as u32 },
        Err(LinkError::Refused(why)) => Pushed::Refused(refusals_of(&why)),
        Err(error) => {
            *client = None;
            Pushed::Failed(error_chain(&error))
        }
    }
}

#[cfg(test)]
mod tests {
    use std::io::Write as _;
    use std::net::TcpListener;

    use sapphire_livelink::protocol::{
        Preamble, Reply, ReplyBody, Request, RequestBody, read_frame, write_frame,
    };
    use sapphire_moves::{Issue, MoveSetError};

    use super::*;

    /// A device of a test's own: each document it's sent goes to `seen`, and it answers once told to.
    fn device() -> (SocketAddr, Receiver<String>, Sender<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").expect("a port");
        let address = listener.local_addr().expect("its address");
        let (seen_out, seen) = mpsc::channel();
        let (answer, answers) = mpsc::channel();
        thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("a connection");
            stream.write_all(&Preamble::CURRENT.to_bytes()).expect("a preamble");
            Preamble::read(&mut stream).expect("its preamble").check().expect("the same version");
            while let Ok(request) = read_frame::<Request>(&mut stream) {
                let body = match request.body {
                    RequestBody::Document { text, .. } => {
                        let _ = seen_out.send(text);
                        let _ = answers.recv_timeout(Duration::from_secs(5));
                        ReplyBody::DocumentApplied { owner: "moves".to_owned() }
                    }
                    _ => ReplyBody::Status(DeviceStatus::default()),
                };
                if write_frame(&mut stream, &Reply { id: request.id, body }).is_err() {
                    return;
                }
            }
        });
        (address, seen, answer)
    }

    fn document_at(seen: &Receiver<String>) -> String {
        seen.recv_timeout(Duration::from_secs(5)).expect("the device was sent a document")
    }

    fn settle(link: &mut Link) {
        let since = Stopwatch::start();
        loop {
            link.poll();
            if !link.is_waiting() {
                return;
            }
            assert!(since.elapsed() < Duration::from_secs(5), "a push never came back");
            thread::sleep(Duration::from_millis(1));
        }
    }

    /// A dragged number sets on every frame, and a device behind it gets the newest of each, in the order they
    /// were last touched.
    #[test]
    fn a_cvars_newest_value_goes_in_place_of_those_waiting() {
        let mailbox = Mailbox { waiting: Mutex::new(Waiting::default()), wake: Condvar::new() };
        mailbox.put_cvar("a".to_owned(), "1".to_owned());
        mailbox.put_cvar("b".to_owned(), "1".to_owned());
        mailbox.put_cvar("a".to_owned(), "2".to_owned());
        let mut sets = Vec::new();
        while let Next::SetCvar(name, value) = mailbox.next(Duration::ZERO) {
            sets.push((name, value));
        }
        let owned = |name: &str, value: &str| (name.to_owned(), value.to_owned());
        assert_eq!(sets, [owned("b", "1"), owned("a", "2")]);
    }

    /// 30 saves behind a device busy with the first: it was sent all 30 in turn, 6.1 s before the newest
    /// took at 200 ms a document, where now it's sent the first and the newest.
    #[test]
    fn saves_behind_a_slow_device_wait_as_the_newest_alone() {
        let (address, seen, answer) = device();
        let mut link = Link::new(Some(address));
        link.push("a.moves.ron".to_owned(), "save 0".to_owned());
        assert_eq!(document_at(&seen), "save 0");
        for save in 1..30 {
            link.push("a.moves.ron".to_owned(), format!("save {save}"));
        }
        assert!(link.is_waiting());
        answer.send(()).expect("the device");
        assert_eq!(document_at(&seen), "save 29");
        answer.send(()).expect("the device");
        settle(&mut link);
        assert!(matches!(link.last_push(), Some(Pushed::Took { owner, .. }) if owner == "moves"));
        assert_eq!(link.earlier_push(), None, "these were all of the text there still is");
        assert!(seen.try_recv().is_err(), "nothing else went");
    }

    #[test]
    fn a_save_that_never_went_says_what_took_its_place() {
        let (address, seen, answer) = device();
        let mut link = Link::new(Some(address));
        link.push("a.moves.ron".to_owned(), "out".to_owned());
        assert_eq!(document_at(&seen), "out");
        link.clear_push();
        link.push("a.moves.ron".to_owned(), "waits".to_owned());
        link.clear_push();
        link.push("a.moves.ron".to_owned(), "newest".to_owned());
        let earlier = link.earlier_push().expect("the one that never went");
        assert_eq!(
            (earlier.origin.as_str(), &earlier.pushed),
            ("a.moves.ron", &Pushed::Failed(REPLACED.to_owned()))
        );
        answer.send(()).expect("the device");
        assert_eq!(document_at(&seen), "newest");
        answer.send(()).expect("the device");
        settle(&mut link);
        assert!(matches!(link.last_push(), Some(Pushed::Took { .. })));
    }

    #[test]
    fn each_file_keeps_its_newest_save() {
        let (address, seen, answer) = device();
        let mut link = Link::new(Some(address));
        link.push("a.moves.ron".to_owned(), "a 0".to_owned());
        assert_eq!(document_at(&seen), "a 0");
        link.push("a.moves.ron".to_owned(), "a 1".to_owned());
        link.push("b.moves.ron".to_owned(), "b 0".to_owned());
        link.push("a.moves.ron".to_owned(), "a 2".to_owned());
        for expected in ["b 0", "a 2"] {
            answer.send(()).expect("the device");
            assert_eq!(document_at(&seen), expected);
        }
        answer.send(()).expect("the device");
        settle(&mut link);
    }

    /// What was waiting for the old device doesn't go there, and doesn't hold up the new one.
    #[test]
    fn a_new_address_goes_ahead_of_what_waited_for_the_old_one() {
        let (old, seen_old, answer_old) = device();
        let (new, seen_new, answer_new) = device();
        let mut link = Link::new(Some(old));
        link.push("a.moves.ron".to_owned(), "out".to_owned());
        assert_eq!(document_at(&seen_old), "out");
        link.push("b.moves.ron".to_owned(), "waits".to_owned());
        link.set_address(Some(new));
        let earlier = link.earlier_push().expect("the one that never went");
        assert_eq!(
            (earlier.origin.as_str(), &earlier.pushed),
            ("b.moves.ron", &Pushed::Failed(ADDRESS_MOVED.to_owned()))
        );
        link.push("a.moves.ron".to_owned(), "to the new one".to_owned());
        answer_old.send(()).expect("the device");
        assert_eq!(document_at(&seen_new), "to the new one");
        answer_new.send(()).expect("the device");
        settle(&mut link);
        assert!(matches!(link.last_push(), Some(Pushed::Took { .. })));
        assert!(seen_old.try_recv().is_err(), "nothing else went to the old one");
    }

    /// The words a refusal comes in are `Issue`'s own, held here to what it writes.
    #[test]
    fn a_refusal_comes_apart_into_move_track_and_message() {
        let issues = vec![
            Issue {
                in_move: Some("light_1".to_owned()),
                track: "hitboxes",
                message: "window 12..11 is empty".to_owned(),
            },
            Issue { in_move: None, track: "intents", message: "`light` appears twice".to_owned() },
        ];
        let text = format!("player.moves.ron: {}", MoveSetError::Invalid(issues));
        assert_eq!(
            refusals_of(&text),
            [
                Refusal {
                    in_move: Some("light_1".to_owned()),
                    track: Some("hitboxes".to_owned()),
                    message: "window 12..11 is empty".to_owned(),
                },
                Refusal {
                    in_move: None,
                    track: Some("intents".to_owned()),
                    message: "`light` appears twice".to_owned(),
                },
            ]
        );
    }

    #[test]
    fn a_refusal_that_names_nothing_keeps_its_words() {
        let text = "no fight here plays `other.moves.ron`: it takes [\"player.moves.ron\"]";
        let refusals = refusals_of(text);
        assert_eq!(refusals.len(), 1);
        assert_eq!(refusals[0].track, None);
        assert_eq!(refusals[0].message, text);
    }
}
