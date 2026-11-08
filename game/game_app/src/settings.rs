//! The player's settings on disk: loaded at startup, written off the frame's path as they change.

use std::io;
use std::path::Path;
use std::sync::mpsc::{self, Receiver, TryRecvError};

use game_ui::menus::PlayerSettings;
use sapphire_save::{SettingsFile, SettingsOutcome, Snapshot};

use crate::LOG_TARGET;

/// The file's name, in the user's directory.
const NAME: &str = "settings";

/// The file, and the one write it has in flight: two could land out of order, and the older would win.
pub(crate) struct KeptSettings {
    file: SettingsFile<PlayerSettings>,
    written: Option<Receiver<io::Result<()>>>,
    /// What was asked for during a write.
    newest: Option<Snapshot<PlayerSettings>>,
}

impl KeptSettings {
    /// What's in `dir`, which may be nothing or a damaged file: a load always gives settings. `replace` writes
    /// a file whole.
    pub(crate) fn load(
        dir: &Path,
        replace: impl Fn(&Path, &[u8]) -> io::Result<()> + Send + Sync + 'static,
    ) -> (Self, PlayerSettings) {
        let file = SettingsFile::new(dir, NAME, replace);
        let loaded = file.load();
        match &loaded.outcome {
            SettingsOutcome::Missing | SettingsOutcome::Read => {}
            outcome => tracing::warn!(target: LOG_TARGET, ?outcome, "the settings file wasn't whole"),
        }
        (Self { file, written: None, newest: None }, loaded.value)
    }

    /// Keeps `settings`, on a job once any write in flight has reported back.
    pub(crate) fn keep(&mut self, settings: &PlayerSettings) {
        match self.file.snapshot(settings) {
            Ok(snapshot) => self.newest = Some(snapshot),
            Err(error) => tracing::error!(target: LOG_TARGET, %error, "the settings can't be written"),
        }
        self.poll();
    }

    /// Once a frame: hears how a write went, and starts the next.
    pub(crate) fn poll(&mut self) {
        if let Some(written) = &self.written {
            match written.try_recv() {
                Ok(result) => report(result),
                Err(TryRecvError::Empty) => return,
                // The job died, and its panic has been logged.
                Err(TryRecvError::Disconnected) => {}
            }
            self.written = None;
        }
        let Some(snapshot) = self.newest.take() else { return };
        let (report, written) = mpsc::channel();
        let file = self.file.clone();
        sapphire_jobs::spawn_async(move || {
            let _ = report.send(file.write(&snapshot));
        });
        self.written = Some(written);
    }

    /// Waits for what's in flight and writes what's left: the process is on its way out.
    pub(crate) fn finish(&mut self) {
        if let Some(written) = self.written.take()
            && let Ok(result) = written.recv()
        {
            report(result);
        }
        if let Some(snapshot) = self.newest.take() {
            report(self.file.write(&snapshot));
        }
    }
}

fn report(result: io::Result<()>) {
    if let Err(error) = result {
        tracing::error!(target: LOG_TARGET, %error, "the settings weren't written");
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use super::*;

    /// The newest write stays and an older can't follow it: `finish` is what a quit calls, and a write
    /// asked for during another waits for it.
    #[test]
    fn what_is_asked_for_last_is_what_stays() {
        let dir = std::env::temp_dir().join(format!("game-app-settings-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let writes = Arc::new(Mutex::new(0));
        let counted = Arc::clone(&writes);
        let replace = move |path: &Path, bytes: &[u8]| {
            *counted.lock().unwrap() += 1;
            std::fs::write(path, bytes)
        };
        let (mut kept, loaded) = KeptSettings::load(&dir, replace);
        assert_eq!(loaded, PlayerSettings::default(), "nothing there yet");

        let settings = |sound| PlayerSettings { sound, ..PlayerSettings::default() };
        kept.keep(&settings(10));
        kept.keep(&settings(20));
        kept.keep(&settings(30));
        kept.finish();
        assert!(*writes.lock().unwrap() <= 2, "what was overtaken isn't written");

        let (_, loaded) = KeptSettings::load(&dir, |_: &Path, _: &[u8]| Ok(()));
        assert_eq!(loaded, settings(30));
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
