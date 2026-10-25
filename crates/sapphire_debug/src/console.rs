//! The dev console's commands over a [`CvarRegistry`]: get, set, list and reset, with history and
//! name completion. The egui view is in `ui`.

use std::collections::VecDeque;
use std::fmt::Write as _;

use sapphire_core::cvar::{AnyCvar, CvarRegistry};

/// Output lines kept.
const SCROLLBACK: usize = 200;
/// Commands kept for recall.
const HISTORY: usize = 50;

const HELP: &str = "\
<name>                 show a cvar
<name> <value>         set a cvar (also: set <name> <value>)
list [prefix]          list cvars, with their values
reset <name> | all     back to the default
clear                  clear this output
help                   this text
Tab completes names; Up and Down recall commands.";

/// What an output line is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LineKind {
    /// An echoed command.
    Command,
    Info,
    Error,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Line {
    pub kind: LineKind,
    pub text: String,
}

/// What Tab does to the input.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Completion {
    /// No cvar starts with the word.
    None,
    /// The input, completed as far as every candidate agrees.
    Extended(String),
    /// Several cvars fit and the input is already their common prefix: list them.
    Candidates(Vec<&'static str>),
}

/// Console state: scrollback and command history.
#[derive(Debug, Default)]
pub struct Console {
    output: VecDeque<Line>,
    history: VecDeque<String>,
    /// While recalling: how far back in `history`.
    recall: Option<usize>,
}

impl Console {
    pub fn output(&self) -> impl Iterator<Item = &Line> {
        self.output.iter()
    }

    /// Runs one command line.
    pub fn execute(&mut self, line: &str, cvars: &CvarRegistry) {
        let line = line.trim();
        self.recall = None;
        if line.is_empty() {
            return;
        }
        if self.history.back().is_none_or(|last| last != line) {
            if self.history.len() == HISTORY {
                self.history.pop_front();
            }
            self.history.push_back(line.to_owned());
        }
        self.print(LineKind::Command, format!("> {line}"));

        let words: Vec<&str> = line.split_whitespace().collect();
        // Specific commands first: a lone word, or a word and a value, is a cvar.
        match words.as_slice() {
            ["help" | "?"] => {
                for help in HELP.lines() {
                    self.print(LineKind::Info, help.to_owned());
                }
            }
            ["clear"] => self.output.clear(),
            ["list"] => self.list(cvars, ""),
            ["list", prefix] => self.list(cvars, prefix),
            ["reset", "all"] => {
                cvars.reset_all();
                self.print(LineKind::Info, format!("reset {} cvars", cvars.len()));
            }
            ["reset", name] => match cvars.find(name) {
                Ok(cvar) => {
                    cvar.reset();
                    self.show(cvar);
                }
                Err(error) => self.print(LineKind::Error, error.to_string()),
            },
            ["get", name] | [name] => match cvars.find(name) {
                Ok(cvar) => self.show(cvar),
                Err(error) => self.print(LineKind::Error, error.to_string()),
            },
            ["set", name, value] | [name, value] => match cvars.find(name) {
                Ok(cvar) => match cvar.set_text(value) {
                    Ok(()) => self.show(cvar),
                    Err(error) => self.print(LineKind::Error, error.to_string()),
                },
                Err(error) => self.print(LineKind::Error, error.to_string()),
            },
            _ => self.print(LineKind::Error, format!("can't read `{line}`; try `help`")),
        }
    }

    /// The previous command (`older`, for Up) or the next one (for Down) from the one recalled
    /// last; `None` once past the newest, for a fresh line.
    pub fn recall(&mut self, older: bool) -> Option<&str> {
        let newest = self.history.len().checked_sub(1)?;
        let next = match (self.recall, older) {
            (None, true) => Some(newest),
            (None, false) => None,
            (Some(at), true) => Some(at.saturating_sub(1)),
            (Some(at), false) => (at < newest).then_some(at + 1),
        };
        self.recall = next;
        next.map(|at| self.history[at].as_str())
    }

    /// Completes the cvar name the input starts with (the first word, or the one after `set`,
    /// `get` or `reset`).
    pub fn complete(&self, input: &str, cvars: &CvarRegistry) -> Completion {
        let (head, word) = match input.rsplit_once(' ') {
            Some((head, word)) if matches!(head.trim(), "set" | "get" | "reset" | "list") => {
                (format!("{} ", head.trim()), word)
            }
            Some(_) => return Completion::None,
            None => (String::new(), input),
        };
        let candidates: Vec<&'static str> = cvars.with_prefix(word).map(|cvar| cvar.name()).collect();
        let Some(&first) = candidates.first() else { return Completion::None };
        if candidates.len() == 1 {
            return Completion::Extended(format!("{head}{first} "));
        }
        let common = candidates.iter().fold(first, |common, name| {
            let shared = common.bytes().zip(name.bytes()).take_while(|(a, b)| a == b).count();
            &common[..shared]
        });
        if common.len() > word.len() {
            Completion::Extended(format!("{head}{common}"))
        } else {
            Completion::Candidates(candidates)
        }
    }

    /// Prints the candidates of a [`Completion::Candidates`].
    pub fn print_candidates(&mut self, candidates: &[&str]) {
        self.print(LineKind::Info, candidates.join("  "));
    }

    fn list(&mut self, cvars: &CvarRegistry, prefix: &str) {
        let mut count = 0;
        for cvar in cvars.with_prefix(prefix) {
            count += 1;
            let mut line = format!("{} = {}", cvar.name(), cvar.value());
            if !cvar.is_default() {
                let _ = write!(line, " (default {})", cvar.default_value());
            }
            let _ = write!(line, "  {}", cvar.help());
            self.print(LineKind::Info, line);
        }
        if count == 0 {
            self.print(LineKind::Info, format!("no cvars start with `{prefix}`"));
        }
    }

    fn show(&mut self, cvar: &dyn AnyCvar) {
        let mut line = format!("{} = {}", cvar.name(), cvar.value());
        if !cvar.is_default() {
            let _ = write!(line, " (default {})", cvar.default_value());
        }
        if let Some((min, max)) = cvar.range() {
            let _ = write!(line, ", range {min}..={max}");
        }
        self.print(LineKind::Info, line);
    }

    fn print(&mut self, kind: LineKind, text: String) {
        if self.output.len() == SCROLLBACK {
            self.output.pop_front();
        }
        self.output.push_back(Line { kind, text });
    }
}

#[cfg(test)]
mod tests {
    use sapphire_core::cvar::Cvar;

    use super::*;

    static EXPOSURE: Cvar<f32> = Cvar::with_range("post.exposure", 0.0, -8.0, 8.0, "Exposure in stops.");
    static BLOOM: Cvar<f32> = Cvar::new("post.bloom", 0.25, "Bloom intensity.");
    static VIGNETTE: Cvar<f32> = Cvar::new("post.vignette", 0.18, "Vignette.");
    static STATS: Cvar<bool> = Cvar::new("debug.stats", true, "Stats panel.");

    /// The cvars are statics, so tests that set them take turns.
    fn setup() -> (std::sync::MutexGuard<'static, ()>, CvarRegistry, Console) {
        static SERIAL: std::sync::Mutex<()> = std::sync::Mutex::new(());
        let serial = SERIAL.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut cvars = CvarRegistry::new();
        cvars.register_all(&[&EXPOSURE, &BLOOM, &VIGNETTE, &STATS]).expect("valid");
        cvars.reset_all();
        (serial, cvars, Console::default())
    }

    fn last(console: &Console) -> &Line {
        console.output().last().expect("output")
    }

    #[test]
    fn sets_gets_and_resets_by_name() {
        let (_serial, cvars, mut console) = setup();
        console.execute("post.exposure 1.5", &cvars);
        assert_eq!(EXPOSURE.get(), 1.5);
        assert_eq!(last(&console).text, "post.exposure = 1.5 (default 0), range -8..=8");
        console.execute("set post.exposure -2", &cvars);
        assert_eq!(EXPOSURE.get(), -2.0);
        console.execute("get post.exposure", &cvars);
        assert!(last(&console).text.starts_with("post.exposure = -2"));
        console.execute("reset post.exposure", &cvars);
        assert_eq!((EXPOSURE.get(), last(&console).text.as_str()), (0.0, "post.exposure = 0, range -8..=8"));
        let commands: Vec<_> =
            console.output().filter(|line| line.kind == LineKind::Command).map(|line| &line.text).collect();
        assert_eq!(commands.len(), 4, "every command is echoed");
    }

    #[test]
    fn reports_errors_without_changing_anything() {
        let (_serial, cvars, mut console) = setup();
        console.execute("post.exposure 99", &cvars);
        assert_eq!(last(&console).kind, LineKind::Error);
        assert!(last(&console).text.contains("must be within"), "{:?}", last(&console));
        console.execute("debug.stats perhaps", &cvars);
        assert_eq!(last(&console).text, "`debug.stats` expects true or false, got `perhaps`");
        console.execute("post.exposur 1", &cvars);
        assert_eq!(last(&console).text, "no cvar is named `post.exposur` (did you mean `post.exposure`?)");
        console.execute("set a b c", &cvars);
        assert_eq!(last(&console).text, "can't read `set a b c`; try `help`");
        assert_eq!((EXPOSURE.get(), STATS.get()), (0.0, true));
    }

    #[test]
    fn lists_and_clears() {
        let (_serial, cvars, mut console) = setup();
        console.execute("list post.", &cvars);
        let listed: Vec<_> =
            console.output().skip(1).map(|line| line.text.split(' ').next().unwrap()).collect();
        assert_eq!(listed, ["post.bloom", "post.exposure", "post.vignette"]);
        console.execute("list r.", &cvars);
        assert_eq!(last(&console).text, "no cvars start with `r.`");
        console.execute("clear", &cvars);
        assert_eq!(console.output().count(), 0);
        console.execute("help", &cvars);
        assert_eq!(console.output().count(), 1 + HELP.lines().count());
    }

    #[test]
    fn completes_names_to_the_common_prefix() {
        let (_serial, cvars, console) = setup();
        assert_eq!(console.complete("po", &cvars), Completion::Extended("post.".into()));
        assert_eq!(
            console.complete("post.", &cvars),
            Completion::Candidates(vec!["post.bloom", "post.exposure", "post.vignette"])
        );
        assert_eq!(console.complete("post.e", &cvars), Completion::Extended("post.exposure ".into()));
        assert_eq!(console.complete("set post.v", &cvars), Completion::Extended("set post.vignette ".into()));
        assert_eq!(console.complete("render", &cvars), Completion::None);
        assert_eq!(console.complete("post.exposure 1", &cvars), Completion::None, "values aren't names");
    }

    #[test]
    fn recalls_history_newest_first() {
        let (_serial, cvars, mut console) = setup();
        assert_eq!(console.recall(true), None);
        for line in ["help", "list", "list", "clear"] {
            console.execute(line, &cvars);
        }
        assert_eq!(console.recall(true), Some("clear"));
        assert_eq!(console.recall(true), Some("list"), "repeats are kept once");
        assert_eq!(console.recall(true), Some("help"));
        assert_eq!(console.recall(true), Some("help"), "stops at the oldest");
        assert_eq!(console.recall(false), Some("list"));
        assert_eq!(console.recall(false), Some("clear"));
        assert_eq!(console.recall(false), None, "past the newest is a fresh line");
    }
}
