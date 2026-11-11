//! Crashes on purpose (M4 C2): what proves that a shipping build's crashes are captured and read back to
//! their function, file and line (the M4 plan's exit test 5). Every build has it, the store's included, since
//! that's the build whose crashes have to be read; nothing reaches it without a developer's tools.
//!
//! A run asks with `--crash <kind>` on its command line. An Android store build takes no launch arguments
//! (its activity drops them), so there the system property `debug.sapphire.crash` asks instead, which only
//! `adb shell setprop` can set, never another app.

/// A crash a run can ask for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ForcedCrash {
    /// A Rust panic, in `forced_panic`: the panic hook's report, and under the shipping profile's
    /// `panic = "abort"`, the abort's own crash after it.
    Panic,
    /// A write to an address no platform maps, in `forced_fault`: a native crash (`SIGSEGV`, or an access
    /// violation on Windows), which the panic hook never sees.
    Fault,
}

/// The command-line flag that asks for one: `--crash panic` or `--crash fault`.
pub const CRASH_FLAG: &str = "--crash";

/// Inside the first page, which no platform this ships on maps (macOS and iOS reserve the first 4 GiB).
const FAULT_ADDRESS: usize = 0x10;

impl ForcedCrash {
    const ALL: [Self; 2] = [Self::Panic, Self::Fault];

    /// What `--crash` calls it.
    pub fn name(self) -> &'static str {
        match self {
            Self::Panic => "panic",
            Self::Fault => "fault",
        }
    }

    /// The kind `name` names, or an error that lists them.
    pub fn from_name(name: &str) -> Result<Self, String> {
        Self::ALL.into_iter().find(|kind| kind.name() == name).ok_or_else(|| {
            let names: Vec<_> = Self::ALL.iter().map(|kind| kind.name()).collect();
            format!("{CRASH_FLAG} expects one of {}, got {name:?}", names.join(", "))
        })
    }

    /// The crash `args` ask for with `--crash <kind>`, if any. A kind that isn't one is an error.
    pub fn from_args<S: AsRef<str>>(args: &[S]) -> Result<Option<Self>, String> {
        let mut asked = None;
        let mut args = args.iter().map(AsRef::as_ref);
        while let Some(arg) = args.next() {
            if arg == CRASH_FLAG {
                let name = args.next().ok_or_else(|| format!("{CRASH_FLAG} needs a value"))?;
                asked = Some(Self::from_name(name)?);
            }
        }
        Ok(asked)
    }

    /// The crash this process was asked for: on Android by the system property `debug.sapphire.crash`, and
    /// elsewhere by its command line. A kind that isn't one is an error.
    pub fn requested() -> Result<Option<Self>, String> {
        #[cfg(target_os = "android")]
        {
            android_property().map(|name| Self::from_name(&name)).transpose()
        }
        #[cfg(not(target_os = "android"))]
        {
            let args: Vec<String> =
                std::env::args_os().map(|arg| arg.to_string_lossy().into_owned()).collect();
            Self::from_args(&args)
        }
    }

    /// Crashes the process. Install crash capture first, or nothing records it.
    pub fn now(self) -> ! {
        tracing::warn!(target: "sapphire::crash", kind = self.name(), "crashing on purpose, as asked");
        match self {
            Self::Panic => forced_panic(),
            Self::Fault => forced_fault(),
        }
    }
}

/// Panics. Never inlined, so a crash's frames name it.
#[inline(never)]
fn forced_panic() -> ! {
    panic!("a crash forced with {CRASH_FLAG} panic");
}

/// Writes to an unmapped address. Never inlined, so the faulting instruction is its own.
#[inline(never)]
fn forced_fault() -> ! {
    // Hidden from the optimizer, so the store is made as written: a plain store, not a call, so the
    // crash's first frame is this function at this line in every profile.
    let address = std::hint::black_box(std::ptr::without_provenance_mut::<u64>(FAULT_ADDRESS));
    // SAFETY: none, by design: the write is the crash. Nothing is mapped at `FAULT_ADDRESS`, so the store
    // faults before it can change anything, and the process ends in the signal (or exception) it raises.
    unsafe { *address = 0x5AFF_1BE5 };
    unreachable!("a write to {FAULT_ADDRESS:#x} returned");
}

/// The value of `debug.sapphire.crash`, if it's set and not empty.
#[cfg(target_os = "android")]
fn android_property() -> Option<String> {
    let mut value = [0 as libc::c_char; libc::PROP_VALUE_MAX as usize];
    // SAFETY: the name is a NUL-terminated string, and `value` has the PROP_VALUE_MAX bytes bionic writes
    // at most, the NUL included.
    let len = unsafe { libc::__system_property_get(c"debug.sapphire.crash".as_ptr(), value.as_mut_ptr()) };
    let len = usize::try_from(len).ok().filter(|&len| len > 0)?;
    let bytes: Vec<u8> = value[..len.min(value.len())].iter().map(|&c| c as u8).collect();
    Some(String::from_utf8_lossy(&bytes).trim().to_owned()).filter(|name| !name.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn args_name_the_crash() {
        assert_eq!(ForcedCrash::from_args(&["game_app", "--capture", "x.png"]), Ok(None));
        assert_eq!(ForcedCrash::from_args(&["game_app", "--crash", "fault"]), Ok(Some(ForcedCrash::Fault)));
        assert_eq!(ForcedCrash::from_args(&["--crash", "panic"]), Ok(Some(ForcedCrash::Panic)));
        assert_eq!(
            ForcedCrash::from_args(&["--crash", "soon"]),
            Err("--crash expects one of panic, fault, got \"soon\"".to_owned())
        );
        assert!(ForcedCrash::from_args(&["--crash"]).is_err());
    }
}
