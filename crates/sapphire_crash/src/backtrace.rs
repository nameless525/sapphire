//! Raw frame instruction pointers and the loaded-module table for panic reports.
//!
//! A stripped shipping build (spec §7.2) prints `std::backtrace::Backtrace` with names only and
//! no addresses, so it cannot be symbolicated after the fact. Instead the panic hook records each
//! frame's raw instruction pointer plus every loaded module's load base and debug id (Mach-O
//! UUID, ELF GNU build-id, or PE PDB GUID). CI resolves those offline, for example with
//! `atos -o <binary> -l <base> <ip>`, or Sentry's symbolicator does it server-side.
//!
//! This uses `libc` directly rather than adding the `backtrace`/`findshlibs` crates: platform
//! `cfg` is allowed here (rule R7), `libc::backtrace` already gives frame IPs on Apple and glibc
//! Linux, and the module walk is a short, well-trodden use of `dyld` / `dl_iterate_phdr`. Windows
//! records no module table here; its panic dump (requested by the hook) carries the module list
//! instead. The capture runs on the panicking thread in a normal (not signal) context, so it may
//! allocate.

use crate::report::ModuleId;

/// The deepest stacks are truncated to this many frames, which is plenty for a panic.
const MAX_FRAMES: usize = 128;

/// Frame IPs (innermost first) and loaded modules at the moment of capture.
pub(crate) struct Capture {
    pub frames: Vec<u64>,
    pub modules: Vec<ModuleId>,
}

/// Captures the current thread's frame IPs and the process's loaded modules.
pub(crate) fn capture() -> Capture {
    Capture { frames: imp::frames(MAX_FRAMES), modules: imp::modules() }
}

#[cfg(target_vendor = "apple")]
mod imp {
    use std::ffi::{CStr, c_char, c_void};
    use std::fmt::Write as _;

    use crate::report::ModuleId;

    // The `<mach-o/dyld.h>` image functions. `libc` marks its copies deprecated and points at the
    // `mach2` crate, which does not expose them, so they are declared here instead.
    unsafe extern "C" {
        fn _dyld_image_count() -> u32;
        fn _dyld_get_image_header(image_index: u32) -> *const MachHeader64;
        fn _dyld_get_image_name(image_index: u32) -> *const c_char;
    }

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

    pub(super) fn modules() -> Vec<ModuleId> {
        let mut out = Vec::new();
        // SAFETY: the dyld image functions return, for indices below `_dyld_image_count`, pointers
        // to Mach-O headers currently mapped into this process. We only read them during this
        // call, on this thread, so the images cannot be unloaded underneath us.
        unsafe {
            for i in 0.._dyld_image_count() {
                let header = _dyld_get_image_header(i);
                if header.is_null() {
                    continue;
                }
                let name = _dyld_get_image_name(i);
                let name = if name.is_null() {
                    String::new()
                } else {
                    CStr::from_ptr(name).to_string_lossy().into_owned()
                };
                out.push(ModuleId {
                    base: header.addr() as u64,
                    debug_id: image_uuid(header).unwrap_or_default(),
                    name,
                });
            }
        }
        out
    }

    /// Reads the `LC_UUID` of a mapped 64-bit Mach-O image, formatted like `dwarfdump --uuid`.
    ///
    /// # Safety
    ///
    /// `header` must point at a mapped Mach-O header with valid `ncmds`/`cmdsize` fields, as dyld
    /// guarantees for a loaded image.
    unsafe fn image_uuid(header: *const MachHeader64) -> Option<String> {
        // SAFETY: by this function's contract `header` is a mapped Mach-O header, so reading its
        // fields and walking `ncmds` load commands (each bounded by `cmdsize`) stays within the
        // image's mapped load-command region.
        unsafe {
            let ncmds = (*header).ncmds;
            let lc_size = size_of::<LoadCommand>();
            // Load commands follow the 64-bit header.
            let mut cmd = header.cast::<u8>().add(size_of::<MachHeader64>());
            for _ in 0..ncmds {
                let lc = cmd.cast::<LoadCommand>().read_unaligned();
                if (lc.cmdsize as usize) < lc_size {
                    return None;
                }
                if lc.cmd == LC_UUID {
                    let mut uuid = [0u8; 16];
                    std::ptr::copy_nonoverlapping(cmd.add(lc_size), uuid.as_mut_ptr(), uuid.len());
                    return Some(format_uuid(&uuid));
                }
                cmd = cmd.add(lc.cmdsize as usize);
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

    pub(super) fn modules() -> Vec<ModuleId> {
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

    pub(super) fn modules() -> Vec<ModuleId> {
        Vec::new()
    }
}
