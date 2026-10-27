//! Raw frame instruction pointers and the module table for panic reports.
//!
//! A stripped shipping build (spec §7.2) prints `std::backtrace::Backtrace` with names only and
//! no addresses, so it cannot be symbolicated after the fact. Instead the panic hook records each
//! frame's raw instruction pointer plus the load base and debug id (Mach-O UUID, ELF GNU
//! build-id, or PE PDB GUID) of the modules they are in. CI resolves those offline, for example
//! with `atos -o <binary> -l <base> <ip>`, or Sentry's symbolicator does it server-side.
//!
//! This uses `libc` directly rather than adding the `backtrace`/`findshlibs` crates: platform
//! `cfg` is allowed here (rule R7), `libc::backtrace` already gives frame IPs on Apple and glibc
//! Linux, and the module lookup is a short, well-trodden use of `dladdr` / `dl_iterate_phdr`.
//! Windows records no module table here; its panic dump (requested by the hook) carries the
//! module list instead. The capture runs on the panicking thread in a normal (not signal)
//! context, so it may allocate.
//!
//! Module lookups must hold up while other threads load and unload libraries:
//! `dl_iterate_phdr` holds the loader's lock while it walks, and on Apple platforms only the
//! images that contain the captured frames are read, found through `dladdr`, which takes dyld's
//! lock (review PA-002, see the Apple `modules`).

use crate::report::ModuleId;

/// The deepest stacks are truncated to this many frames, which is plenty for a panic.
const MAX_FRAMES: usize = 128;

/// Frame IPs (innermost first) and the modules needed to symbolicate them.
pub(crate) struct Capture {
    pub frames: Vec<u64>,
    pub modules: Vec<ModuleId>,
}

/// Captures the current thread's frame IPs and the modules they are in (on Linux, every loaded
/// module).
pub(crate) fn capture() -> Capture {
    let frames = imp::frames(MAX_FRAMES);
    let modules = imp::modules(&frames);
    Capture { frames, modules }
}

#[cfg(target_vendor = "apple")]
mod imp {
    use std::ffi::{CStr, c_void};
    use std::fmt::Write as _;

    use crate::report::ModuleId;

    /// A 64-bit Mach-O header; load commands follow it.
    #[repr(C)]
    struct MachHeader64 {
        magic: u32,
        cputype: i32,
        cpusubtype: i32,
        filetype: u32,
        ncmds: u32,
        sizeofcmds: u32,
        flags: u32,
        reserved: u32,
    }

    /// The common prefix of every Mach-O load command.
    #[repr(C)]
    struct LoadCommand {
        cmd: u32,
        cmdsize: u32,
    }

    /// `MH_MAGIC_64`: a 64-bit Mach-O header in the host's byte order.
    const MH_MAGIC_64: u32 = 0xfeed_facf;

    /// `LC_UUID`: carries the 16-byte Mach-O UUID after the load-command header.
    const LC_UUID: u32 = 0x1b;

    pub(super) fn frames(max: usize) -> Vec<u64> {
        let mut buf = vec![std::ptr::null_mut::<c_void>(); max];
        // SAFETY: `backtrace` writes up to `max` frame pointers into `buf` and returns how many it
        // wrote. `buf` has `max` slots, so the returned count is in bounds.
        let count = unsafe { libc::backtrace(buf.as_mut_ptr(), max as libc::c_int) };
        let count = usize::try_from(count).unwrap_or(0).min(max);
        buf[..count].iter().map(|frame| frame.addr() as u64).collect()
    }

    /// The images that contain `frames`, each once, in the order the frames first reach them.
    ///
    /// Walking dyld's image list (`_dyld_image_count`, `_dyld_get_image_header`,
    /// `_dyld_get_image_name`) is not safe while another thread loads or unloads an image: the
    /// indices shift and a header can be unmapped while it is read, which faulted under a test
    /// that unloads a library in a loop (review PA-002). Instead each frame's image comes from
    /// `dladdr`, which holds dyld's lock while it looks. Only images with a frame on this thread's
    /// stack are read, and those stay loaded while the thread runs (returning into an unloaded
    /// image would crash the program anyway), so their headers and paths stay valid here.
    pub(super) fn modules(frames: &[u64]) -> Vec<ModuleId> {
        let mut out: Vec<ModuleId> = Vec::new();
        for &ip in frames {
            // Frames above the innermost hold return addresses, which can point one past the
            // call at the very end of an image; the byte before is always inside the caller.
            let address = std::ptr::with_exposed_provenance::<c_void>(ip.saturating_sub(1) as usize);
            // SAFETY: `dladdr` only looks the address up in dyld's tables and fills `info`, a
            // plain C struct for which all zeroes is a valid value.
            let info = unsafe {
                let mut info: libc::Dl_info = std::mem::zeroed();
                (libc::dladdr(address, &mut info) != 0).then_some(info)
            };
            let Some(info) = info.filter(|info| !info.dli_fbase.is_null()) else {
                continue;
            };
            let base = info.dli_fbase.addr() as u64;
            if out.iter().any(|module| module.base == base) {
                continue;
            }
            // SAFETY: `dli_fbase` is the Mach-O header and `dli_fname` the path of an image that
            // holds one of this thread's frames, so both stay mapped while we read them (above).
            let (debug_id, name) = unsafe {
                let name = if info.dli_fname.is_null() {
                    String::new()
                } else {
                    CStr::from_ptr(info.dli_fname).to_string_lossy().into_owned()
                };
                (image_uuid(info.dli_fbase.cast()).unwrap_or_default(), name)
            };
            out.push(ModuleId { base, debug_id, name });
        }
        out
    }

    /// Reads the `LC_UUID` of a mapped 64-bit Mach-O image, formatted like `dwarfdump --uuid`.
    ///
    /// # Safety
    ///
    /// `header` must point at a mapped Mach-O image's header, whose `sizeofcmds` bytes of load
    /// commands follow it, as dyld guarantees for a loaded image.
    unsafe fn image_uuid(header: *const MachHeader64) -> Option<String> {
        // SAFETY: by this function's contract `header` is a mapped Mach-O header, and every read
        // below stays within the `sizeofcmds` bytes of load commands that follow it.
        unsafe {
            let header_value = header.read_unaligned();
            if header_value.magic != MH_MAGIC_64 {
                return None;
            }
            let commands = header.cast::<u8>().add(size_of::<MachHeader64>());
            let limit = header_value.sizeofcmds as usize;
            let lc_size = size_of::<LoadCommand>();
            let mut offset = 0;
            for _ in 0..header_value.ncmds {
                if offset + lc_size > limit {
                    return None;
                }
                let lc = commands.add(offset).cast::<LoadCommand>().read_unaligned();
                let size = lc.cmdsize as usize;
                if size < lc_size || offset + size > limit {
                    return None;
                }
                if lc.cmd == LC_UUID {
                    let mut uuid = [0u8; 16];
                    if size < lc_size + uuid.len() {
                        return None;
                    }
                    std::ptr::copy_nonoverlapping(
                        commands.add(offset + lc_size),
                        uuid.as_mut_ptr(),
                        uuid.len(),
                    );
                    return Some(format_uuid(&uuid));
                }
                offset += size;
            }
        }
        None
    }

    fn format_uuid(bytes: &[u8; 16]) -> String {
        let mut out = String::with_capacity(36);
        for (i, byte) in bytes.iter().enumerate() {
            if matches!(i, 4 | 6 | 8 | 10) {
                out.push('-');
            }
            // Writing to a String cannot fail.
            let _ = write!(out, "{byte:02X}");
        }
        out
    }
}

#[cfg(any(target_os = "linux", target_os = "android"))]
mod imp {
    use std::ffi::{CStr, c_void};

    use crate::report::ModuleId;

    // glibc provides `backtrace`; bionic (Android) and musl do not, so there frames stay empty
    // and the module table alone locates the crash.
    #[cfg(all(target_os = "linux", target_env = "gnu"))]
    pub(super) fn frames(max: usize) -> Vec<u64> {
        let mut buf = vec![std::ptr::null_mut::<c_void>(); max];
        // SAFETY: `backtrace` writes up to `max` frame pointers into `buf` and returns the count,
        // which is therefore within `buf`.
        let count = unsafe { libc::backtrace(buf.as_mut_ptr(), max as libc::c_int) };
        let count = usize::try_from(count).unwrap_or(0).min(max);
        buf[..count].iter().map(|frame| frame.addr() as u64).collect()
    }

    #[cfg(not(all(target_os = "linux", target_env = "gnu")))]
    pub(super) fn frames(_max: usize) -> Vec<u64> {
        Vec::new()
    }

    /// Every loaded object. `dl_iterate_phdr` holds the loader's lock while it walks, so a library
    /// another thread unloads can't vanish underneath, and the table covers the frames whether
    /// or not `backtrace` found any (bionic and musl have none).
    pub(super) fn modules(_frames: &[u64]) -> Vec<ModuleId> {
        let mut out: Vec<ModuleId> = Vec::new();
        // SAFETY: `dl_iterate_phdr` invokes `collect` once per loaded object with a valid info
        // pointer, passing our `&mut Vec` back as the opaque data argument. It runs synchronously
        // on this thread, so the exclusive borrow is sound.
        unsafe {
            libc::dl_iterate_phdr(Some(collect), std::ptr::from_mut(&mut out).cast());
        }
        out
    }

    const NT_GNU_BUILD_ID: u32 = 3;

    unsafe extern "C" fn collect(
        info: *mut libc::dl_phdr_info,
        _size: libc::size_t,
        data: *mut c_void,
    ) -> libc::c_int {
        // SAFETY: `info` is a valid record for one loaded object, and `data` is the `&mut Vec`
        // passed to `dl_iterate_phdr`, used only here on the iterating thread.
        unsafe {
            let info = &*info;
            let out = &mut *data.cast::<Vec<ModuleId>>();
            let name = if info.dlpi_name.is_null() {
                String::new()
            } else {
                CStr::from_ptr(info.dlpi_name).to_string_lossy().into_owned()
            };
            out.push(ModuleId {
                // `dlpi_addr` is `Elf64_Addr` (u64); Sapphire targets only 64-bit (spec §2).
                base: info.dlpi_addr,
                debug_id: build_id(info).unwrap_or_default(),
                name,
            });
        }
        0
    }

    /// Reads the GNU build-id from a loaded object's `PT_NOTE` segment, as lowercase hex.
    ///
    /// # Safety
    ///
    /// `info` must be a valid `dl_phdr_info` as delivered by `dl_iterate_phdr`.
    unsafe fn build_id(info: &libc::dl_phdr_info) -> Option<String> {
        // SAFETY: by this function's contract `info` is valid, so `dlpi_phdr`/`dlpi_phnum`
        // describe the object's program-header array, and each `PT_NOTE` segment is mapped at
        // `dlpi_addr + p_vaddr` for `p_memsz` bytes.
        unsafe {
            let phdrs = std::slice::from_raw_parts(info.dlpi_phdr, info.dlpi_phnum as usize);
            for phdr in phdrs {
                if phdr.p_type != libc::PT_NOTE {
                    continue;
                }
                let addr = (info.dlpi_addr as usize).wrapping_add(phdr.p_vaddr as usize);
                if let Some(id) = scan_notes(addr as *const u8, phdr.p_memsz as usize) {
                    return Some(id);
                }
            }
        }
        None
    }

    /// An ELF note header. Each note is this header, then a 4-byte-aligned name, then a
    /// 4-byte-aligned descriptor.
    #[repr(C)]
    struct NoteHeader {
        name_size: u32,
        desc_size: u32,
        kind: u32,
    }

    /// Walks the notes in `[ptr, ptr + len)` and returns the GNU build-id if present.
    ///
    /// # Safety
    ///
    /// `ptr` must point at `len` readable bytes (the mapped `PT_NOTE` segment).
    unsafe fn scan_notes(mut ptr: *const u8, mut remaining: usize) -> Option<String> {
        // SAFETY: by this function's contract `[ptr, ptr + remaining)` is readable. Every read is
        // bounded by `remaining`, which is decremented by each note's total size, so the cursor
        // never moves past the segment.
        unsafe {
            let header_size = size_of::<NoteHeader>();
            while remaining >= header_size {
                let note = ptr.cast::<NoteHeader>().read_unaligned();
                let name_padded = (note.name_size as usize).div_ceil(4) * 4;
                let desc_padded = (note.desc_size as usize).div_ceil(4) * 4;
                let total = header_size.checked_add(name_padded)?.checked_add(desc_padded)?;
                if total > remaining {
                    return None;
                }
                if note.kind == NT_GNU_BUILD_ID && note.name_size == 4 {
                    let name = std::slice::from_raw_parts(ptr.add(header_size), 3);
                    if name == b"GNU" {
                        let desc = ptr.add(header_size + name_padded);
                        let desc = std::slice::from_raw_parts(desc, note.desc_size as usize);
                        return Some(hex_lower(desc));
                    }
                }
                ptr = ptr.add(total);
                remaining -= total;
            }
        }
        None
    }

    fn hex_lower(bytes: &[u8]) -> String {
        use std::fmt::Write as _;
        let mut out = String::with_capacity(bytes.len() * 2);
        for byte in bytes {
            // Writing to a String cannot fail.
            let _ = write!(out, "{byte:02x}");
        }
        out
    }
}

// Windows and any other target: the panic dump carries the module list instead (see the hook).
#[cfg(not(any(target_vendor = "apple", target_os = "linux", target_os = "android")))]
mod imp {
    use crate::report::ModuleId;

    pub(super) fn frames(_max: usize) -> Vec<u64> {
        Vec::new()
    }

    pub(super) fn modules(_frames: &[u64]) -> Vec<ModuleId> {
        Vec::new()
    }
}

#[cfg(all(test, target_vendor = "apple"))]
mod apple_tests {
    use std::ffi::{CStr, CString, c_void};
    use std::process::Command;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

    use super::capture;
    use crate::report::ModuleId;
    use crate::test_support::ScratchDir;

    /// What `dladdr` says about `address`: its image's header address and path.
    fn dladdr_image(address: u64) -> Option<(u64, String)> {
        // SAFETY: `dladdr` only looks the address up and fills `info`, a plain C struct for which
        // all zeroes is valid. Callers pass addresses in images that stay loaded meanwhile, so the
        // path it points at stays valid while it is copied.
        unsafe {
            let mut info: libc::Dl_info = std::mem::zeroed();
            if libc::dladdr(std::ptr::with_exposed_provenance::<c_void>(address as usize), &mut info) == 0
                || info.dli_fname.is_null()
            {
                return None;
            }
            let name = CStr::from_ptr(info.dli_fname).to_string_lossy().into_owned();
            Some((info.dli_fbase.addr() as u64, name))
        }
    }

    /// Modules whose base and name don't describe one loaded image.
    fn inconsistent(modules: &[ModuleId]) -> Vec<String> {
        modules
            .iter()
            .filter(|module| dladdr_image(module.base) != Some((module.base, module.name.clone())))
            .map(|module| format!("{:#x} {:?}", module.base, module.name))
            .collect()
    }

    #[test]
    fn modules_are_the_frames_images() {
        let capture = capture();
        assert!(!capture.frames.is_empty());
        assert_eq!(inconsistent(&capture.modules), Vec::<String>::new());
        // This test's own code is in the capture, and so is its image, with its UUID.
        let (this_image, _) =
            dladdr_image((modules_are_the_frames_images as *const ()).addr() as u64).expect("own image");
        let own = capture.modules.iter().find(|module| module.base == this_image).expect("own image listed");
        assert_eq!(own.debug_id.len(), 36, "a formatted UUID: {own:?}");
        // Each image once.
        let mut bases: Vec<_> = capture.modules.iter().map(|module| module.base).collect();
        bases.dedup();
        assert_eq!(bases.len(), capture.modules.len());
    }

    /// PA-002: capturing while another thread loads and unloads a library must not fault.
    /// Walking dyld's image list, as the capture did, faulted here within 300,000 captures,
    /// reading the header of a library that was unloaded mid-walk (and 20,000 captures listed it
    /// 337 times); only images with frames on the capturing thread may be read.
    ///
    /// Needs Xcode (`xcrun cc`) to build the library it unloads, and runs for a few seconds:
    /// `cargo test -p sapphire_crash --lib -- --ignored modules_survive`.
    #[test]
    #[ignore = "builds a dylib with Xcode's cc and stresses dyld for seconds"]
    fn modules_survive_concurrent_dlclose() {
        let dir = ScratchDir::new("dlclose");
        let source = dir.path().join("stress.c");
        std::fs::write(&source, "int sapphire_stress(void) { return 42; }\n").expect("write source");
        let library = dir.path().join("libsapphire_stress.dylib");
        // Xcode's compiler and macOS SDK; the workspace's cargo config exports an iOS deployment
        // target, which would make clang build for iOS.
        let status = Command::new("xcrun")
            .args(["--sdk", "macosx", "cc", "-dynamiclib", "-o"])
            .arg(&library)
            .arg(&source)
            .env_remove("IPHONEOS_DEPLOYMENT_TARGET")
            .status()
            .expect("run xcrun cc");
        assert!(status.success(), "cc failed");
        let library_name = library.to_string_lossy().into_owned();
        let library = CString::new(library.into_os_string().into_encoded_bytes()).expect("path");

        let stop = Arc::new(AtomicBool::new(false));
        let cycles = Arc::new(AtomicUsize::new(0));
        let loader = {
            let (stop, cycles) = (Arc::clone(&stop), Arc::clone(&cycles));
            std::thread::spawn(move || {
                while !stop.load(Ordering::Relaxed) {
                    // SAFETY: loads and unloads a library without initializers; nothing keeps a
                    // pointer into it between the two calls.
                    unsafe {
                        let handle = libc::dlopen(library.as_ptr(), libc::RTLD_NOW | libc::RTLD_LOCAL);
                        assert!(!handle.is_null(), "dlopen failed");
                        libc::dlclose(handle);
                    }
                    cycles.fetch_add(1, Ordering::Relaxed);
                }
            })
        };

        let mut listed_library = 0;
        let mut bad = Vec::new();
        for _ in 0..30_000 {
            let capture = capture();
            // Compared by name only: the library may be gone already, so nothing of it is read.
            let (library, pinned): (Vec<_>, Vec<_>) =
                capture.modules.into_iter().partition(|module| module.name.ends_with(&library_name));
            listed_library += library.len();
            bad.extend(inconsistent(&pinned));
        }
        stop.store(true, Ordering::Relaxed);
        loader.join().expect("loader thread");
        assert!(cycles.load(Ordering::Relaxed) > 100, "the library must have been unloaded repeatedly");
        assert_eq!(listed_library, 0, "captures read a library no frame was in");
        assert_eq!(bad, Vec::<String>::new(), "modules that describe no loaded image");
    }
}
