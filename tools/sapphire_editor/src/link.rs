//! Saving's other half: a thread of its own that talks to a device over live link, so the interface never
//! waits on a network. It keeps a [`Client`] connected, pushes what it's given, and asks the device what
//! it's playing twice a second, which is how the editor knows an edit reached an attempt rather than
//! being told it should have.

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

enum Command {
    Address(Option<SocketAddr>),
    Push { origin: String, text: String },
}

enum Report {
    Connection(Result<Box<DeviceStatus>, String>),
    Pushed(Pushed),
}

/// A connection to a device, as the interface sees it: it never blocks.
#[derive(Debug)]
pub struct Link {
    commands: Option<Sender<Command>>,
    reports: Receiver<Report>,
    worker: Option<JoinHandle<()>>,
    /// The device's last status, while it's connected.
    device: Option<DeviceStatus>,
    /// Why there's no device, if there isn't.
    trouble: Option<String>,
    /// A push is on its way.
    waiting: bool,
    last_push: Option<Pushed>,
    /// The last attempt seen of each actor, newest revision first: a move can end between two statuses.
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
            device: None,
            trouble: None,
            waiting: false,
            last_push: None,
            attempts: Vec::new(),
        }
    }

    /// Points it at another device.
    pub fn set_address(&mut self, address: Option<SocketAddr>) {
        self.device = None;
        self.attempts.clear();
        self.send(Command::Address(address));
    }

    /// Pushes a document, by its file's name. [`Self::last_push`] says what became of it.
    pub fn push(&mut self, origin: String, text: String) {
        self.waiting = true;
        self.send(Command::Push { origin, text });
    }

    fn send(&mut self, command: Command) {
        let sent = self.commands.as_ref().is_some_and(|commands| commands.send(command).is_ok());
        if !sent {
            self.waiting = false;
            self.last_push = Some(Pushed::Failed("live link's thread has stopped".to_owned()));
        }
    }

    /// Takes what the thread has reported. Call it once a frame.
    pub fn poll(&mut self) {
        loop {
            match self.reports.try_recv() {
                Ok(Report::Connection(Ok(status))) => {
                    self.trouble = None;
                    for attempt in &status.attempts {
                        match self.attempts.iter_mut().find(|seen| seen.actor == attempt.actor) {
                            Some(seen) => *seen = attempt.clone(),
                            None => self.attempts.push(attempt.clone()),
                        }
                    }
                    self.device = Some(*status);
                }
                Ok(Report::Connection(Err(why))) => {
                    self.device = None;
                    self.trouble = Some(why);
                }
                Ok(Report::Pushed(pushed)) => {
                    self.waiting = false;
                    self.last_push = Some(pushed);
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
        self.waiting
    }

    #[must_use]
    pub fn last_push(&self) -> Option<&Pushed> {
        self.last_push.as_ref()
    }

    /// The last attempt seen of each actor.
    #[must_use]
    pub fn attempts(&self) -> &[MoveAttempt] {
        &self.attempts
    }

    /// Forgets what the last push said: the document has changed since, and it no longer describes it.
    pub fn clear_push(&mut self) {
        self.last_push = None;
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
    let mut since_try = Stopwatch::start();
    loop {
        match commands.recv_timeout(STATUS_EVERY) {
            Ok(Command::Address(next)) => {
                address = next;
                client = None;
                since_try = Stopwatch::start();
                if address.is_none() {
                    let _ = reports.send(Report::Connection(Err("no address".to_owned())));
                }
            }
            Ok(Command::Push { origin, text }) => {
                let report = push(address, &mut client, &origin, text);
                if reports.send(Report::Pushed(report)).is_err() {
                    return;
                }
            }
            Err(RecvTimeoutError::Timeout) => {
                if client.is_none() && since_try.elapsed() < RECONNECT_EVERY {
                    continue;
                }
                since_try = Stopwatch::start();
                let Some(address) = address else { continue };
                let status = connected(address, &mut client)
                    .and_then(|client| client.status().map(Box::new).map_err(|error| error_chain(&error)));
                if client.as_ref().is_some_and(Client::is_closed) || status.is_err() {
                    client = None;
                }
                if reports.send(Report::Connection(status)).is_err() {
                    return;
                }
            }
            Err(RecvTimeoutError::Disconnected) => return,
        }
    }
}

fn connected(address: SocketAddr, client: &mut Option<Client>) -> Result<&mut Client, String> {
    if client.is_none() {
        *client = Some(Client::connect(address).map_err(|error| error_chain(&error))?);
    }
    client.as_mut().ok_or_else(|| "no connection".to_owned())
}

fn push(address: Option<SocketAddr>, client: &mut Option<Client>, origin: &str, text: String) -> Pushed {
    let Some(address) = address else {
        return Pushed::Failed("no device address to push to".to_owned());
    };
    let stopwatch = Stopwatch::start();
    let connection = match connected(address, client) {
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
