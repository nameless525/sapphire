//! Facts about the compile target that code below the platform layer needs.
//!
//! The `cfg!(target_os = ...)` here is an accepted exception to rule R7 (ADR 0001): job-system
//! sizing and other foundation defaults depend on the target class, and they must not pull in
//! `sapphire_platform` (and SDL) to learn it. This is the single definition of the target
//! class; `sapphire_platform::DeviceClass` re-exports it.

/// Whether the target is a desktop or a phone (tablets count as phones).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum FormFactor {
    /// macOS, Windows, Linux (including Steam Deck).
    Desktop,
    /// iOS and Android: thermally limited, touch first.
    Mobile,
}

impl FormFactor {
    /// The form factor of the compile target.
    pub const CURRENT: Self =
        if cfg!(any(target_os = "ios", target_os = "android")) { Self::Mobile } else { Self::Desktop };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn current_matches_the_target() {
        let mobile = cfg!(any(target_os = "ios", target_os = "android"));
        assert_eq!(FormFactor::CURRENT == FormFactor::Mobile, mobile);
    }
}
