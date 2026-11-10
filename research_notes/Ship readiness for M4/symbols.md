# Symbols: split debuginfo, Breakpad, symbolication, store uploads

A scout's read on 2026-11-10, web only, nothing built or run. Crate versions are from the crates.io API on 2026-11-10; the rest is from the linked pages or memory of them, marked where not re-read today.

## 1. Rust split debuginfo per target

- Cargo profile docs (https://doc.rust-lang.org/cargo/reference/profiles.html#split-debuginfo) and rustc's `-C split-debuginfo` (https://doc.rust-lang.org/rustc/codegen-options/index.html#split-debuginfo): `packed` on macOS/iOS runs dsymutil and produces a `.dSYM` bundle next to the binary; `unpacked` leaves the `.o` files. Windows MSVC always writes a `.pdb` (only `packed` is supported). On Linux, `packed` produces a `.dwp` via `thorin`/rust's packager and `unpacked` leaves `.dwo` files (stabilised for Linux in Rust 1.65).
- Android uses the Linux ELF path, so `packed` should give `.dwp` too. ours: Breakpad's `dump_syms` and Play Console both expect ordinary ELF with DWARF, not split DWARF; the safer Android recipe is `split-debuginfo = "off"` for Android, keep the unstripped `.so` as the symbol file, and ship a stripped copy. Check whether `dump_syms` reads `.dwp` (unverified).
- Strip: Cargo `strip = "debuginfo"` or `"symbols"` (https://doc.rust-lang.org/cargo/reference/profiles.html#strip). On macOS, rustc strips via `strip`; dSYM is produced before, so the dSYM keeps symbols (unverified that ordering holds with `packed` + `strip = "symbols"`; test once).
- Linux/Android manual route (https://sourceware.org/gdb/onlinedocs/gdb/Separate-Debug-Files.html): `objcopy --only-keep-debug app app.debug`, `objcopy --strip-debug app` (or `--strip-all`), `objcopy --add-gnu-debuglink=app.debug app`. `llvm-objcopy` supports the same flags and is in the NDK (`llvm-objcopy`, `llvm-strip`).
- ours: our shipping profile has `split-debuginfo = "packed"` everywhere; we likely need per-target overrides (`[profile.shipping]` cannot be target-specific, so set `-C split-debuginfo` via `.cargo/config.toml` `[target.<triple>] rustflags` or do the strip in xtask).

## 2. Breakpad symbol format and debug IDs

- Format (https://chromium.googlesource.com/breakpad/breakpad/+/master/docs/symbol_files.md): text lines `MODULE os arch debug_id name`, `INFO CODE_ID ...`, `FILE n path`, `INLINE_ORIGIN`, `FUNC [m] addr size param_size name`, line records `addr size line file`, `INLINE`, `PUBLIC`, and `STACK CFI INIT`/`STACK CFI` (DWARF CFI unwind rules) and `STACK WIN` (Windows frame data).
- Debug ID: a GUID plus an age, printed as 33 hex chars uppercase. Mach-O: the `LC_UUID` with age 0. PE: the CodeView RSDS GUID + age from the PDB reference. ELF: the GNU build ID note's first 16 bytes, with the first three GUID fields byte-swapped (little-endian GUID layout), age 0; without a build ID Breakpad falls back to hashing the first page of `.text` (fragile). Source: same doc plus rust-minidump's `debugid` crate docs (https://docs.rs/debugid).
- Symbol-server layout: `<debug_file>/<DEBUG_ID>/<debug_file>.sym`, e.g. `libgame.so/ABCD...0/libgame.so.sym`; for Windows the debug file is the `.pdb` name and the sym is `game.sym` (https://chromium.googlesource.com/breakpad/breakpad/+/master/docs/... and Mozilla's Tecken docs https://tecken.readthedocs.io/). Not re-read today.
- Microsoft symbol server (https://learn.microsoft.com/en-us/windows/win32/debug/symbol-server-and-symbol-stores): `<file.pdb>/<GUID><age-hex>/<file.pdb>`, and binaries by `<file.exe>/<TimeDateStamp><SizeOfImage>/<file.exe>`. `symstore.exe` builds it; a plain directory works.

## 3. Rust tools

- `dump_syms` (Mozilla, https://github.com/mozilla/dump_syms): 2.3.9 (2026-08-14), MIT/Apache-2.0. Reads PDB/PE, ELF (DWARF) and Mach-O/dSYM, writes Breakpad `.sym`; has a `--store` option writing the server layout. Primarily a binary, but the crate exposes a library (unverified how stable that API is).
- `rust-minidump` (https://github.com/rust-minidump/rust-minidump): `minidump-processor` and `minidump-unwind` 0.27.0 (2026-08-11), MIT. `minidump-stackwalk` is the CLI. Symbolicates offline from local Breakpad symbol directories (`--symbols-path`), or symbol servers via an http feature that can be turned off. Unwinds with CFI from `.sym` STACK records, frame pointers, and stack scanning as a last resort; reports which method found each frame ("trust"). ours: a test can link `minidump-processor` with a local `Symbolizer` over a temp dir and assert function names in our own crash dumps, no binaries downloaded.
- `symbolic` (Sentry, https://github.com/getsentry/symbolic): 13.9.0 (2026-11-03), MIT. Reads ELF/Mach-O/PE/PDB/Breakpad/WASM, symcache, demangling, debug IDs; can also write Breakpad (`symbolic-debuginfo`). Heavy but library-first.
- `wholesym` (samply's, https://github.com/mstange/samply): 0.8.1, last release 2025-03-07, MIT/Apache-2.0. Symbolicates addresses directly from binaries/dSYM/PDB, can fetch from symbol servers. ours: geared to profilers, not minidumps; second choice.
- ours: pick `dump_syms` (as a library or a cargo-installed tool in CI, built from source by cargo, not a downloaded binary) to produce `.sym` per target, and `minidump-processor` in tests and in an xtask `symbolicate` command.

## 4. Store-side symbol uploads

- Apple: bitcode was deprecated in Xcode 14 (https://developer.apple.com/documentation/xcode-release-notes/xcode-14-release-notes), so the dSYM you build is the one that matches. Xcode/`xcodebuild -exportArchive` with "Upload symbols" includes symbols so App Store Connect crash reports (Organizer) are symbolicated. Not re-read today. ours: for a hand-built `.app`, put our `.dSYM` into the `.xcarchive`'s `dSYMs/` folder before export, or keep dSYMs ourselves and symbolicate `.ips` crash logs locally with `atos`/our tooling.
- Google Play (https://developer.android.com/build/include-native-symbols): AGP's `android.defaultConfig.ndk.debugSymbolLevel = "SYMBOL_TABLE"` or `"FULL"` puts `BUNDLE-METADATA/com.android.tools.build.debugsymbols/<abi>/*.so.sym|.dbg` into the AAB. Or upload a zip in Play Console per app version (App bundle explorer, "Native debug symbols"); the zip is `<abi>/lib*.so` with symbols. ours: with a hand-built AAB we can add the BUNDLE-METADATA entries ourselves via bundletool's `--metadata-file`, or upload the zip manually; size limit around 800 MB (unverified).
- Sentry `sentry-cli debug-files upload` (https://docs.sentry.io/cli/dif/) accepts dSYM, PDB, ELF, Breakpad; it can be adopted later without changing what we keep.

## 5. Android native crashes without Sentry

- Tombstones are written by `debuggerd`/`crash_dump` to `/data/tombstones/` (not readable by apps) and logged to logcat (https://source.android.com/docs/core/tests/debug/native-crash).
- `ActivityManager.getHistoricalProcessExitReasons` (API 30, Android 11) returns `ApplicationExitInfo`; for `REASON_CRASH_NATIVE`, `getTraceInputStream()` returns the tombstone. Docs: "for native crashes... the tombstone in protobuf format" starting Android 12 (API 31); on 11 it is only ANR traces (https://developer.android.com/reference/android/app/ApplicationExitInfo#getTraceInputStream()). Proto schema: AOSP `system/core/debuggerd/proto/tombstone.proto`.
- ours: on next launch, from Kotlin/JNI, read exit reasons, copy the native-crash stream to app storage, and decode with a vendored copy of `tombstone.proto` (Apache-2.0) via `prost`. Keeps our own signal record for API < 31. Tombstone frames have build IDs and pc offsets, so the same `.sym` store symbolicates them.

## 6. Reproducible symbol identity

- Linux: GCC/clang builds often get `--build-id` from the distro's default linker config, but not guaranteed; `-C link-arg=-Wl,--build-id` makes it explicit. Rust has passed `--build-id` by default on some targets (unverified which). The NDK's clang passes `--build-id` by default (https://android.googlesource.com/platform/ndk/+/master/docs/BuildSystemMaintainers.md, unverified today); Play's symbol matching relies on it.
- macOS/iOS: ld64 writes `LC_UUID` from a hash of the output by default; `-no_uuid` disables it. Windows: link.exe writes the RSDS GUID; with `/Brepro` it is deterministic.
- ours: the rule is "symbols come from the same link as the shipped binary, never a rebuild". CI should, in one job: build, extract symbols (dSYM/pdb/unstripped .so), run `dump_syms`, strip, then assert the stripped binary's ID equals the `.sym` MODULE ID (a small xtask check using `symbolic` or `object`), and archive both keyed by ID.
