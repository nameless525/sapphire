//! A minimal argument parser; xtask's handful of flags don't justify clap's build time.

use std::str::FromStr;

use anyhow::{Context as _, bail, ensure};

/// Arguments after the command name, with anything after `--` held apart for the app.
#[derive(Debug)]
pub struct Args {
    args: Vec<String>,
    passthrough: Option<Vec<String>>,
}

impl Args {
    pub fn new(args: impl IntoIterator<Item = String>) -> Self {
        let mut args: Vec<String> = args.into_iter().collect();
        let passthrough = args.iter().position(|a| a == "--").map(|i| args.split_off(i).split_off(1));
        Self { args, passthrough }
    }

    /// Takes the leading command name, if any.
    pub fn command(&mut self) -> Option<String> {
        (!self.args.is_empty()).then(|| self.args.remove(0))
    }

    /// Takes a boolean `--flag`.
    pub fn flag(&mut self, name: &str) -> bool {
        let found = self.args.iter().position(|a| a == name);
        found.map(|i| self.args.remove(i)).is_some()
    }

    /// Takes `--name value` or `--name=value` and parses the value.
    pub fn value<T>(&mut self, name: &str) -> anyhow::Result<Option<T>>
    where
        T: FromStr,
        anyhow::Error: From<T::Err>,
    {
        let prefix = format!("{name}=");
        let Some(i) = self.args.iter().position(|a| a == name || a.starts_with(&prefix)) else {
            return Ok(None);
        };
        let arg = self.args.remove(i);
        let value = match arg.strip_prefix(&prefix) {
            Some(value) => value.to_owned(),
            None => {
                ensure!(i < self.args.len() && !self.args[i].starts_with("--"), "{name} needs a value");
                self.args.remove(i)
            }
        };
        let parsed = value.parse::<T>().map_err(anyhow::Error::from);
        parsed.map(Some).with_context(|| format!("invalid {name}"))
    }

    /// Like [`value`](Self::value), but the option must be present.
    pub fn required<T>(&mut self, name: &str) -> anyhow::Result<T>
    where
        T: FromStr,
        anyhow::Error: From<T::Err>,
    {
        self.value(name)?.with_context(|| format!("missing required option {name}"))
    }

    /// Fails on leftover arguments; returns what followed `--`.
    pub fn finish(self, allow_passthrough: bool) -> anyhow::Result<Vec<String>> {
        if let Some(unexpected) = self.args.first() {
            bail!("unexpected argument `{unexpected}`");
        }
        ensure!(
            allow_passthrough || self.passthrough.is_none(),
            "this command takes no arguments after `--`"
        );
        Ok(self.passthrough.unwrap_or_default())
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    fn args(line: &str) -> Args {
        Args::new(line.split_whitespace().map(str::to_owned))
    }

    #[test]
    fn parses_flags_values_and_passthrough() {
        let mut a = args("run --release --platform=ios-sim --exe /tmp/app -- --fullscreen 1");
        assert_eq!(a.command().as_deref(), Some("run"));
        assert!(a.flag("--release"));
        assert!(!a.flag("--verbose"));
        assert_eq!(a.required::<String>("--platform").unwrap(), "ios-sim");
        assert_eq!(a.value::<PathBuf>("--exe").unwrap(), Some(PathBuf::from("/tmp/app")));
        assert_eq!(a.finish(true).unwrap(), ["--fullscreen", "1"]);
    }

    #[test]
    fn reports_mistakes() {
        let mut a = args("bundle --platform");
        a.command();
        assert_eq!(a.required::<String>("--platform").unwrap_err().to_string(), "--platform needs a value");

        let mut a = args("content --cook");
        a.command();
        assert!(a.value::<String>("--cook").is_err());

        let mut a = args("bundle");
        a.command();
        assert_eq!(
            a.required::<String>("--platform").unwrap_err().to_string(),
            "missing required option --platform"
        );

        let a = args("check-layers --strict");
        assert_eq!(a.finish(false).unwrap_err().to_string(), "unexpected argument `check-layers`");

        let mut a = args("content -- extra");
        a.command();
        assert!(a.finish(false).is_err());
        let mut a = args("run -- --fullscreen 1");
        a.command();
        assert_eq!(a.finish(true).unwrap(), ["--fullscreen", "1"]);
    }
}
