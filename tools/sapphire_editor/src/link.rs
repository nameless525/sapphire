//! Saving's other half: a thread of its own that talks to a device over live link, so the interface never
//! waits on a network. It keeps a [`Client`] connected, pushes what it's given, and asks the device what
//! it's playing twice a second, which is how the editor knows an edit reached an attempt rather than
//! being told it should have.
//!
//! A reply can take seconds, and the text and the address can change meanwhile. So every push has an ID and
//! every report says which address it's about: [`Link::last_push`] only ever speaks for the text and the
//! device in front of the person, [`Link::earlier_push`] holds what came back too late to, and nothing an
//! old device says lands on a new one.

use std::net::SocketAddr;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender, TryRecvError};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use sapphire_livelink::protocol::{DeviceStatus, MoveAttempt};
use sapphire_livelink::{Client, LinkError};
use sapphire_time::Stopwatch;

use crate::edits::error_chain;

/// How often a connected device is asked what it's doing. Slow enough to cost it nothing and quick enough
/// that a move of half a second shows.
const STATUS_EVERY: Duration = Duration::from_millis(250);
/// How often to try for a device that isn't there.
const RECONNECT_EVERY: Duration = Duration::from_secs(1);

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

enum Command {
    /// `target` is the count of addresses so far, which the reports made after this repeat.
    Address {
        target: u64,
        address: Option<SocketAddr>,
    },
    Push {
        id: u64,
        origin: String,
        text: String,
    },
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
}

/// A connection to a device, as the interface sees it: it never blocks.
#[derive(Debug)]
pub struct Link {
    commands: Option<Sender<Command>>,
    reports: Receiver<Report>,
    worker: Option<JoinHandle<()>>,
    address: Option<SocketAddr>,
    /// Which address this is: a report of any other is about a device no longer looked at.
    target: u64,
    /// The device's last status, while it's connected.
    device: Option<DeviceStatus>,
    /// Why there's no device, if there isn't.
    trouble: Option<String>,
    /// Pushes with no answer yet. The thread takes them one at a time, so there can be several.
    waiting: u32,
    /// The ID the next push takes.
    next_push: u64,
    /// Pushes from this one on are of the text and to the device now in front of the person.
    current_from: u64,
    last_push: Option<Pushed>,
    earlier_push: Option<EarlierPush>,
    /// The last attempt seen of each actor on this connection: a move can end between two statuses.
    attempts: Vec<MoveAttempt>,
}

impl Link {
    /// A link talking to `address`, or to nothing yet.
    #[must_use]
    pub fn new(address: Option<SocketAddr>) -> Self {
        let (commands, commands_in) = mpsc::channel();
        let (reports_out, reports) = mpsc::channel();
        let worker = thread::Builder::new()
            .name("live link".to_owned())
            .spawn(move || work(address, &commands_in, &reports_out))
            .ok();
        Self {
            commands: Some(commands),
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
        self.clear_push();
        self.send(Command::Address { target: self.target, address });
    }

    /// Pushes a document, by its file's name. [`Self::last_push`] says what became of it, as long as nothing
    /// has changed by the time it's known.
    pub fn push(&mut self, origin: String, text: String) {
        self.waiting += 1;
        let id = self.next_push;
        self.next_push += 1;
        self.send(Command::Push { id, origin, text });
    }

    fn send(&mut self, command: Command) {
        let sent = self.commands.as_ref().is_some_and(|commands| commands.send(command).is_ok());
        if !sent {
            self.waiting = 0;
            self.last_push = Some(Pushed::Failed("live link's thread has stopped".to_owned()));
        }
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
                Ok(Report::Connected { .. } | Report::Connection { .. }) => {}
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
        self.commands = None;
        // Its longest wait is a connection's deadline, so it isn't waited for.
        drop(self.worker.take());
    }
}

fn work(mut address: Option<SocketAddr>, commands: &Receiver<Command>, reports: &Sender<Report>) {
    let mut client: Option<Client> = None;
    let mut target = 0;
    let mut since_try = Stopwatch::start();
    loop {
        match commands.recv_timeout(STATUS_EVERY) {
            Ok(Command::Address { target: next_target, address: next }) => {
                target = next_target;
                address = next;
                client = None;
                since_try = Stopwatch::start();
                if address.is_none() {
                    let status = Err("no address".to_owned());
                    let _ = reports.send(Report::Connection { target, status });
                }
            }
            Ok(Command::Push { id, origin, text }) => {
                let pushed = push(address, target, &mut client, reports, &origin, text);
                if reports.send(Report::Pushed { id, origin, pushed }).is_err() {
                    return;
                }
            }
            Err(RecvTimeoutError::Timeout) => {
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
            Err(RecvTimeoutError::Disconnected) => return,
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
    use sapphire_moves::{Issue, MoveSetError};

    use super::*;

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
