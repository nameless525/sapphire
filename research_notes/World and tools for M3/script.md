# Luau scripting for M3

A scout's read for [the M3 plan](../../docs/m3-plan.md), 2026-11-03: read-only, nothing built or run, on `main` at 1335f14d and crate sources from crates.io. Its claims are an agent's reading, and a package proves them before leaning on them.

Luau through mlua for M3: findings. Nothing was built or run, so all target and timing claims are inferred. Repository untouched.

**1. Versions and targets**
- Read (crates.io API): mlua 0.12.2 (2026-10-03, MSRV 1.88), mlua-sys 0.13.0, luau0-src 0.22.1+luau744 (2026-10-24; its `max_stable_version` field reports 0.22.0+luau740). All MIT. The spec's "0.12" row still holds.
- Read: S/src/lib.rs builds C++17 with `cc`, with no per-target gating apart from emscripten. The C++ stdlib is `c++` on Apple, `stdc++` on Linux, none on MSVC, and **`c++_shared` on Android** (lib.rs:336-358), overridable with `CXXSTDLIB_<target>`.
- Inferred: Android either ships libc++_shared.so in the APK or sets `CXXSTDLIB_aarch64_linux_android=c++_static`. xtask's NDK 29 `CXX_*` environment should already serve it. iOS and the simulator need only Xcode's clang; MSVC needs cl.exe.
- Read: C++ exceptions are required. `LUA_USE_LONGJMP` is off by default and mlua-sys never calls `use_longjmp` (grep of build/).
- **Risk, inferred from Rust's `C-unwind` rules:** mlua's callback path calls `lua_error` from a Rust frame (M/src/state/util.rs:174,181), and `[profile.shipping]` has `panic = "abort"` (Cargo.toml:322). A foreign exception reaching a `panic=abort` Rust frame aborts. So in shipping, a binding returning `Err` or an interrupt raising an error probably kills the process. A script's own `error()` stays in C++ frames up to `lua_pcall`, so it should be fine.
- Read: README claims CI only for x86-64 Windows, macOS and Linux plus aarch64 cross-compiles, so iOS and Android are unproven upstream.

**2. Allocation**
- Read: mlua hardcodes its allocator, which calls `std::alloc`, so **every VM allocation hits the Rust global allocator and is counted** (M/src/memory.rs:90-164; state/raw.rs:143). There's no public allocator option that I found. `init_from_ptr` is `pub(super)`, and `MemoryState::get` assumes the userdata is a `MemoryState` (memory.rs:24), so a custom `lua_newstate` allocator needs a fork or patch. `lua_newstate` itself takes one (VM/../include/lua.h:133).
- Read: small objects come from page pools, and `frealloc` is called only for a new or emptied page or a large block (VM/lmem.cpp header comment:15-76). So a hook that creates nothing shouldn't reach the allocator, but one that makes a table, string or closure can, on a page boundary.
- Read: `Function::call` is lock, stack guard, `lua_xpush`, `lua_pcall`, with no Rust allocation visible for primitive arguments (M/src/function.rs:193-217). Callbacks are boxed at creation (state.rs:1517), and failure slots are pooled (state/util.rs:42-52). Inferred: a stack or CallInfo grow can allocate until warm, and `String` or `Variadic` arguments and errors allocate.
- Read: the Compiler and Ast are C++ whose `new` wouldn't be counted (the VM has no raw `new`); that only matters at load.
- **Recommended honest gate:** keep zero on steps with no hook. Around a hook, set a thread-local that diverts counts to a separate `script_allocations` figure with its own budget and a replay report. Generated bindings flip it back, so Rust allocations inside a binding still fail gate 4. Stop the GC in-step (`LUA_GCSTOP` sets the threshold to `SIZE_MAX`, lapi.cpp:1433) so no assist runs. Pure-numeric hooks should reach zero and can be asserted so. A real arena comes later, if the bucket proves noisy.

**3. Determinism**
- Read: the string hash is **unseeded**, from content and length only (VM/lstring.cpp:10). `pairs` over string keys therefore follows hash, table size and insertion history. Inferred: repeatable given the same operations, but it's an implementation detail that a Luau upgrade can move, so it re-pins checksums.
- Read: table, function, thread and userdata keys **hash the low 32 address bits** (ltable.cpp:69-84, 225), so their `pairs` order follows ASLR and the allocator. Vector keys hash by float bits.
- Read: `math.sin/cos/tan/asin/acos/atan/atan2/exp/log/log10/pow/sinh/cosh/tanh` call the C library (lmathlib.cpp:47-174), and so does `vector.angle` (lveclib.cpp:99).
- **Replacing them isn't enough.** The compiler emits FASTCALL, and in a safe env `luauF_sin` and friends call libm directly (lbuiltins.cpp:60-385; Builtins.cpp:38-41 comment). Sandboxing turns safeenv on. Replacing needs `Compiler::set_disabled_builtins` (M/src/chunk.rs:390) as well.
- **Three more libm routes:**
  - The `^` operator calls `pow` directly (lvmexecute.cpp:2047, 2282; exact only for K = 2, 3, 0.5).
  - The compiler folds constants with the host's libm (Compiler/src/BuiltinFolding.cpp:198, 279; ConstantFolding `pow`, line 467).
  - `tonumber` uses `strtod` (lnumutils.h:133).
- Read: `tostring(number)` is Luau's own `luai_num2str` (lnumprint.cpp:273), so it should be OS-independent. `string.format("%f/%e/%g")` is libc `snprintf` (lstrlib.cpp:1140), so it isn't. `tostring(table)` prints an encoded address (laux.cpp:669).
- Read: `table.sort` is median-of-three quicksort with a heap fallback and no randomness (ltablib.cpp:480-529). It's repeatable but unstable, and it needs a total order.
- Read: vectors are float32 with 3 components by default. `vector.normalize` is sqrt-based, so should be exact IEEE; `math.noise` uses fmod and float arithmetic.
- Read: **GC pacing uses the wall clock** (lgc.cpp:1201, 1293-1311, 1326 feed `getheaptrigger`), so sweep timing, freed pages and used memory vary per run. This is harmless only while the GC is unobservable.
- Read: `math.random` auto-seeds from pointer, `time` and `clock` (lmathlib.cpp:519-521).

**Full list to remove or replace**
- `os.*`, `math.random/randomseed`, `gcinfo`, `collectgarbage` (mlua's restricted wrapper too, for checksummed scripts), `newproxy`.
- `getfenv/setfenv` (they turn off safeenv, lbaselib.cpp:134,143), `debug.*`, `print`, `require`.
- Transcendental `math.*` and `math.noise` (replace with `sapphire_math::det`, f64-to-f32 policy to be decided), `vector.angle`.
- `string.format` float conversions, `tonumber` on non-integer text.
- Weak tables (`__mode`); non-primitive table keys under `pairs` or `next`; `tostring` of reference types; `string.format("%p")`-style output if present (unchecked).
- `^` other than K = 2, 3, 0.5; memory-limit errors as behaviour.
- Coroutines only if yields can cross ticks.
- Scan cooked bytecode for `LOP_POW/POWK`. A lint for folded `^` and `pairs` needs AST access that I didn't find exposed in mlua.
- Inferred: FP contraction and MSVC vs Clang arithmetic in VM C++ between Linux and Windows is the residual risk, and only CI can prove it. Windows' cl.exe compiles the VM, and aarch64 has its own checksum file.

**4. Sandbox**
- Read: `Lua::sandbox(true)` runs `luaL_sandbox` and `luaL_sandboxthread`: libraries and built-in metatables go read-only, and globals become a proxy with safeenv (M/src/state.rs:680; VM/linit.cpp:66-128). Edit libraries **before** it. The spec's "`time` and `random` globals" aren't globals in Luau: they're `os.time` and `math.random`, so fix the wording.
- Read: `set_interrupt` fires at loop back-edges and calls (`VM_INTERRUPT`, 9 sites), Luau only (state.rs:843). Count interrupts for a deterministic budget, never wall time. Raising from it hits the shipping abort issue above. `VmState::Yield` calls `lua_yield`, which sets a status rather than throwing (ldo.cpp:860), so running hooks in a thread may give a kill path with no exception (unproven).
- Read: `set_memory_limit` is available (state.rs:1116). Treat hitting it as a failed run, since used memory is clock-dependent.
- Read: `gc_step` passes 0 KB (state.rs:1186-1195). The spec's time budget doesn't exist as an API: loop steps against a clock, or call `lua_gc(LUA_GCSTEP, kb)` through `exec_raw`, outside the step and outside the checksum. `gc_stop`, `gc_restart` and `gc_set_mode` exist. Inferred: an explicit step works under stop, because it sets the threshold temporarily (lapi.cpp:~1400), but confirm it restores the stopped state.

**5. Hot reload and bindings**
- Read: `lua.load(bytes).set_environment(t).into_function()` plus `Compiler::compile` returns bytecode, and binary chunks load (M/src/chunk.rs:398, 541, 652).
- Inferred design: a chunk returns a table of hooks; a name-to-`Function` table is swapped between steps; a chunk's hash goes into `tables_hash`. Durable state must live outside the VM, or a reload loses it.
- Read: `sapphire_reflect` registers explicitly and `bevy_reflect` 0.20's function reflection isn't enabled (no `function` in its Cargo.toml). I found no `.d.luau` generator in mlua.
- Recommendation: a declared script API table of commands and queries, with field shapes from the registry, and an `xtask` generating static Rust wrappers and the `.d.luau` from one source, with a test that the checked-in output matches. Inferred: runtime reflection dispatch would allocate and isn't worth it for hooks.

**6. Recommendation**
- **Placement:** `sapphire_script` joins `[simulation]` `crates`, with the shared `clippy.toml`. mlua's tree (`bstr`, `parking_lot`, `rustc-hash`, `num-traits`, `either`, `libc`, `cc`) hits nothing in `forbidden` (read M/Cargo.toml; `cargo tree` unrun). So a replay runner still builds with no GPU, and it's a C++ toolchain step only.
- **Edge:** gameplay is L4 and so is script (spec). Keep gameplay ignorant: gameplay defines the hook events and an intent queue, script depends on gameplay through an `[allow]` edge (or sits at L5), and it registers via `Simulation::add_combat_system` in a named `CombatSet` slot. `sapphire_replay` links it. Check-layers needs only that row.
- **Compile cost, estimated, not measured:** 60K C++ lines without CodeGen (VM 27.5K, Ast 13K, Compiler 11K, Bytecode 4.8K, the rest 3.6K). All libraries build unconditionally. I'd guess 10 to 30 s wall on a clean build and on each `LUAU_CXXFLAGS` change, then cached; the spike should time it.
- **Checksum:**
  - Scripts are pure decisions: a hook's only output is typed commands into a fixed-capacity queue, applied at one point in the step.
  - Hash each command (tick, hook ID, arguments by bits) in `state_checksum`, bumping its version.
  - Persistent script state lives in a simulation-owned, fixed-size, numeric, hashed blackboard rather than VM globals, so the heap never needs hashing and a reload or save survives.
  - RNG reads `SimRng`, the clock reads the actor or world clock.
  - Cook bytecode once at cook time with a pinned compiler and libm builtins disabled (bytes into `tables_hash`), so devices don't run the compiler or folding; live link pushes source compiled by the same settings.
- **First spike** (a worktree; no game work):
  1. `cargo check` on all five targets through xtask's environment, and link on the iOS simulator, Android emulator and Windows CI.
  2. Time the clean compile and record the binary-size delta.
  3. **Shipping `panic=abort`:** a binding `Err`, an interrupt kill and script `error()`, to see which abort, and whether a thread plus yield avoids them. If none works, the decision is `panic=unwind` for shipping vs a non-throwing binding convention.
  4. A boss-phase hook in a replay under `CountingAllocator`, showing zero on non-hook ticks and the bucket on hook ticks, with GC stopped in-step and stepped outside.
  5. Determinism attack across Linux and Windows CI: a script exercising every kept library, `pairs` over string keys, sort and vectors, emitting hashed commands; a negative test that banned routes are absent (including `math.sin` through FASTCALL and a folded constant); and several runs with ASLR to show unchanged checksums.
  6. Reload mid-replay rebinding hooks, with the blackboard surviving.
  7. Hook cost in µs against §3's 0.3/0.5 ms row (spec line 120).

**Couldn't confirm**
- Actual builds on any target, and compile time: nothing built.
- The `panic=abort` behaviour with mlua: inferred from Rust's unwinding rules, unrun.
- Whether mlua has any public route to a custom allocator: `init_from_ptr` and `Lua` constructors read partially.
- Whether `fmod` and other "exact" C functions match bit for bit on glibc and UCRT, and MSVC FP contraction: only CI proves these.
- `lua_gc` step behaviour under stop; whether optimization level alone turns off folding (Compiler.cpp's gates read only by grep); `%p` in `string.format`.
- Allocation-free hook calls are read from code, not measured.
