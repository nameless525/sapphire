//! What a program knows of where it runs without the platform's loop.

use std::path::Path;

use crate::content::ContentFs;
use crate::system::SystemInfo;

/// Where content comes from on this device, and the device's facts: all [`Platform`](crate::Platform) gives
/// that doesn't need its loop, its window or `SDL_Init`. An app on the loop reads
/// [`Platform::host`](crate::Platform::host). A tool or a test with no loop, such as an editor's viewport,
/// names its own content with [`Self::with_content_at`], on any thread.
#[derive(Clone, Debug)]
pub struct Host {
    content: ContentFs,
    system_info: SystemInfo,
}

impl Host {
    pub(crate) fn new(content: ContentFs, system_info: SystemInfo) -> Self {
        Self { content, system_info }
    }

    /// This device, with content read from the directory `root`. SDL opens the files and answers for the device,
    /// neither of which needs it initialized on a desktop. A phone's content is in its package, and comes
    /// through `Platform`.
    #[must_use]
    pub fn with_content_at(root: &Path) -> Self {
        Self::new(ContentFs::new(root.to_string_lossy().into_owned()), SystemInfo::query())
    }

    /// The read-only content root.
    #[must_use]
    pub fn content(&self) -> &ContentFs {
        &self.content
    }

    /// This device with `content` in place of its own: the same content with a patch's files placed over it
    /// ([`ContentFs::with_files_over`], M4 P4).
    #[must_use]
    pub fn with_content(&self, content: ContentFs) -> Self {
        Self { content, system_info: self.system_info }
    }

    /// Static facts about the device.
    #[must_use]
    pub fn system_info(&self) -> &SystemInfo {
        &self.system_info
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// With nothing initialized, as a test's own thread is: it reads a file, and knows the device.
    #[test]
    fn a_host_needs_no_loop() {
        let dir = std::env::temp_dir().join(format!("sapphire_host_{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("made");
        std::fs::write(dir.join("a.txt"), "read").expect("written");
        let host = Host::with_content_at(&dir);
        assert_eq!(host.content().read("a.txt").expect("reads"), b"read");
        assert!(host.content().read("b.txt").is_err());
        assert!(host.system_info().logical_cores >= 1);
        std::fs::remove_dir_all(&dir).expect("cleaned");
    }
}
