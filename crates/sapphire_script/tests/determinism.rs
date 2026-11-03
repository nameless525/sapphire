//! The determinism attack (`docs/m3-plan.md` §3, "Luau's determinism"): each hazard tried by a script, and
//! whether the allowlist removes it, the cook's scan refuses it, or it's on record as a hole. A hazard
//! that's real is shown to be real on a plain VM first, so a passing test isn't passing because nothing
//! was ever there.

mod support;

use std::io::Write as _;
use std::process::Command as Process;

use mlua::Lua;
use sapphire_gameplay::StateHasher;
use sapphire_gameplay::replay::ARCHITECTURE;
use sapphire_math::det;
use sapphire_script::compile::{self, CookError, DISABLED_BUILTINS};
use sapphire_script::scan::scan;
use std::ffi::c_int;

use mlua::{ffi, lua_State};
use sapphire_script::bindings::{BOARD_SLOTS, with_frame};
use sapphire_script::{Binding, Frame, HookEnd, Limits, ScriptHost, ScriptVm, cook};
use support::{marks, play, replay_file, run_hook};

fn refused(source: &str) -> String {
    match cook(source) {
        Err(CookError::Refused(rule)) => rule,
        other => panic!("cooked what should have been refused: {source}: {other:?}"),
    }
}

fn hook_fails(body: &str) -> String {
    let source = format!("return {{ hook = function() {body} end }}");
    match run_hook(&source).expect("cooks and loads").0 {
        HookEnd::Failed(error) => error,
        end => panic!("{body}: ended as {end:?}"),
    }
}

fn marked(body: &str) -> Vec<f64> {
    let source = format!("return {{ hook = function() {body} end }}");
    let (end, frame) = run_hook(&source).expect("cooks and loads");
    assert_eq!(end, HookEnd::Returned, "{body}");
    assert!(!frame.fault, "{body}");
    marks(&frame)
}

// The allowlist.

#[test]
fn what_the_plan_removes_isnt_there() {
    for name in [
        "os",
        "gcinfo",
        "collectgarbage",
        "newproxy",
        "getfenv",
        "setfenv",
        "debug",
        "print",
        "require",
        "coroutine",
        "utf8",
        "buffer",
        "integer",
        "setmetatable",
        "getmetatable",
        "next",
        "pcall",
        "xpcall",
        "load",
        "loadstring",
        "_G",
        "_VERSION",
        "warn",
    ] {
        assert_eq!(marked(&format!("sapphire.mark({name} == nil and 1 or 0)")), [1.0], "{name}");
    }
    for name in [
        "math.random",
        "math.randomseed",
        "math.noise",
        "math.fmod",
        "math.cosh",
        "math.sinh",
        "math.tanh",
        "math.log10",
        "math.frexp",
        "math.ldexp",
        "math.modf",
        "vector.angle",
        "string.rep",
        "string.char",
        "string.byte",
        "string.reverse",
        "table.concat",
        "table.clone",
        "table.clear",
        "table.move",
        "table.unpack",
        "unpack",
        "string.format",
        "string.gsub",
        "string.find",
        "string.match",
        "string.gmatch",
        "string.lower",
        "string.upper",
        "string.pack",
        "table.foreach",
        "table.getn",
    ] {
        assert_eq!(marked(&format!("sapphire.mark({name} == nil and 1 or 0)")), [1.0], "{name}");
    }
}

#[test]
fn every_global_a_script_can_reach_is_on_the_list() {
    // The list is one of what stays, so a Luau upgrade's new global is gone by default. Walking what the
    // sandbox's proxy reads through says nothing else got in.
    let vm = ScriptVm::new(Limits::default()).unwrap();
    let globals = vm.lua_for_tests().globals();
    let behind: mlua::Table = globals.metatable().expect("the sandbox's proxy").get("__index").unwrap();
    let mut names = Vec::new();
    behind
        .for_each(|name: String, _: mlua::Value| {
            names.push(name);
            Ok(())
        })
        .unwrap();
    names.sort_unstable();
    assert_eq!(
        names,
        [
            "assert", "bit32", "error", "ipairs", "math", "pairs", "rawequal", "rawget", "rawlen", "rawset",
            "sapphire", "select", "string", "table", "tonumber", "tostring", "type", "typeof", "vector",
        ],
    );
}

#[test]
fn a_global_write_is_refused_twice() {
    assert!(refused("x = 1 return {}").contains("global write"));
    // And past the scan, the table's frozen: a chunk from elsewhere that's been scanned can't write it
    // by name either, and `rawset` raises in Luau's frames.
    assert!(hook_fails("rawset(table, 'x', 1)").contains("readonly"));
}

// Keys and iteration.

#[test]
fn pairs_over_table_keys_really_does_follow_addresses_on_a_plain_vm() {
    // The negative of `the_same_hook_reads_one_checksum_under_aslr`: `child` prints this.
    let lua = Lua::new();
    let order: String = lua
        .load(
            "local keys, index = {}, {} for i = 1, 16 do keys[i] = {} index[keys[i]] = i end
             local out = '' for k, v in pairs(index) do out ..= v .. ',' end return out",
        )
        .eval()
        .unwrap();
    assert!(order.split(',').count() > 16);
}

#[test]
fn pairs_over_a_reference_key_raises_and_over_a_string_key_runs() {
    assert!(hook_fails("local t = {[{}] = 1} for k in pairs(t) do end").contains("order follows addresses"));
    assert!(hook_fails("local t = {[function() end] = 1} for k in pairs(t) do end").contains("addresses"));
    assert!(hook_fails("local f = pairs({}); f({[{}] = 1})").contains("addresses"));
    let walked = marked(
        "local n = 0 for k, v in pairs({a = 1, b = 2, c = 3, [10] = 4, [true] = 5}) do n += v end sapphire.mark(n)",
    );
    assert_eq!(walked, [15.0]);
}

#[test]
fn pairs_over_string_keys_goes_in_one_pinned_order() {
    // Hash, table size and insertion history, with no address in it. A Luau upgrade can move this, which
    // re-pins checksums: a row in the upgrade's review, not a hole.
    let order = marked(
        "local t = {} for i, name in ipairs({'phase', 'rage', 'adds', 'timer', 'enrage', 'wave', 'door'}) do t[name] = i end
         t.rage = nil t.boss = 8
         for k, v in pairs(t) do sapphire.mark(v) end",
    );
    assert_eq!(order.len(), 7);
    assert_eq!(order, marked(
        "local t = {} for i, name in ipairs({'phase', 'rage', 'adds', 'timer', 'enrage', 'wave', 'door'}) do t[name] = i end
         t.rage = nil t.boss = 8
         for k, v in pairs(t) do sapphire.mark(v) end",
    ));
}

#[test]
fn a_bare_table_in_a_for_is_refused_by_the_scan() {
    // Generalized iteration walks the hash with no `pairs` to check it: removing `next` doesn't stop it.
    assert!(refused("return { hook = function() for k in {} do end end }").contains("for .. in"));
    let f = "local f, t = pairs({}) for k in f, t do end";
    assert!(refused(&format!("return {{ hook = function() {f} end }}")).contains("for .. in"));
}

#[test]
fn ipairs_and_numeric_loops_run() {
    assert_eq!(marked("for i, v in ipairs({5, 6}) do sapphire.mark(i * v) end"), [5.0, 12.0]);
    assert_eq!(marked("for i = 3, 1, -1 do sapphire.mark(i) end"), [3.0, 2.0, 1.0]);
}

// Text.

#[test]
fn tostring_of_a_reference_raises_and_of_a_value_prints() {
    assert!(hook_fails("return tostring({})").contains("address"));
    assert!(hook_fails("return tostring(sapphire.mark)").contains("address"));
    // What's left is Luau's own printer, with no C library in it.
    assert_eq!(marked("sapphire.mark(string.len(tostring(0.1)))"), [3.0]);
    assert_eq!(marked("sapphire.mark(string.len(tostring(1e300)))"), [6.0]);
}

#[test]
fn string_format_is_gone_by_every_road() {
    assert!(hook_fails("return string.format('%f', 1)").contains("nil"));
    assert!(hook_fails("return ('%f'):format(1)").contains("format"));
}

#[test]
fn tonumber_reads_whole_numbers_and_nothing_a_c_library_would_round() {
    assert_eq!(
        marked("sapphire.mark(tonumber('12')) sapphire.mark(tonumber('-3')) sapphire.mark(tonumber(4.5))"),
        [12.0, -3.0, 4.5]
    );
    for text in ["0.1", "1e3", "0x10", " 1", "1 ", "inf", "nan", "", "-", "1234567890123456"] {
        assert_eq!(marked(&format!("sapphire.mark(tonumber('{text}') == nil and 1 or 0)")), [1.0], "{text}");
    }
}

/// On record as a hole, the one the allowlist can't close: Luau's arithmetic coerces text by itself,
/// through `strtod`, with no function to take away and no instruction to scan for. It takes a string a
/// script made on purpose, and each of the C libraries here rounds correctly, so it's low: this reads
/// what each OS does with the awkward spellings, for CI to compare.
#[test]
fn text_in_arithmetic_goes_through_the_c_library() {
    assert_eq!(marked("local text = '0.1' sapphire.mark(text + 0)"), [0.1]);
    let mut read = Vec::new();
    for text in ["0.1", "1e-320", "0x1p4", "0x10", "inf", "nan", "1e400", "1.7976931348623157e308", " 7 "] {
        let source =
            format!("return {{ hook = function() local text = '{text}' sapphire.mark(text + 0) end }}");
        let (end, frame) = run_hook(&source).unwrap();
        read.push(match end {
            HookEnd::Returned => format!("{text}={:016x}", marks(&frame)[0].to_bits()),
            _ => format!("{text}=refused"),
        });
    }
    say(&format!("coercions {}", read.join(" ")));
}

#[test]
fn bindings_take_only_numbers() {
    // `lua_tonumberx` alone reads text.
    assert!(marked("sapphire.mark(math.sin('0.5') ~= math.sin('0.5') and 1 or 0)") == [1.0]);
    let (_, frame) = run_hook("return { hook = function() sapphire.mark('0.5') end }").unwrap();
    assert!(frame.fault);
}

// Math.

#[test]
fn a_replaced_function_is_the_pinned_one_at_every_level() {
    for level in [0, 1, 2] {
        let bytes = compile::compile_unchecked(
            "return { hook = function(x) sapphire.mark(math.sin(x)) sapphire.mark(math.pow(x, 2.5)) sapphire.mark(math.exp(x)) end }",
            level,
        )
        .unwrap();
        assert!(scan(&bytes).unwrap().fastcalls_all.is_empty(), "level {level}");
        let vm = ScriptVm::new(Limits::default()).unwrap();
        let hooks = vm.load(&bytes, "=test").unwrap();
        let hook: mlua::Function = hooks.get("hook").unwrap();
        let mut frame = Frame::new();
        assert_eq!(vm.fire(&mut frame, &hook, 0.7), HookEnd::Returned);
        let x = 0.7_f64 as f32;
        assert_eq!(
            marks(&frame).iter().map(|v| v.to_bits()).collect::<Vec<_>>(),
            [det::sin(x), det::pow(x, 2.5), det::exp(x)].map(|v| f64::from(v).to_bits()),
            "level {level}",
        );
    }
}

#[test]
fn fastcall_reaches_libm_with_the_global_replaced_unless_it_is_disabled() {
    // The compiler with no list: `math.sin` is FASTCALL 24, and it calls C whatever `math.sin` now holds.
    let plain = mlua::chunk::Compiler::new().set_optimization_level(2);
    let source = "return { hook = function(x) sapphire.mark(math.sin(x)) end }";
    let bytes = plain.compile(source).unwrap();
    let findings = scan(&bytes).unwrap();
    assert_eq!(findings.fastcalls, [24], "{findings:?}");
    assert!(compile::check(&bytes).unwrap_err().to_string().contains("builtin 24"));

    // Past the scan, it does what the scout read: the sandbox's safe env runs the C function, not ours.
    let vm = ScriptVm::new(Limits::default()).unwrap();
    let hooks: mlua::Table =
        vm.lua_for_tests().load(&bytes).set_mode(mlua::chunk::ChunkMode::Binary).eval().unwrap();
    let hook: mlua::Function = hooks.get("hook").unwrap();
    let mut frame = Frame::new();
    assert_eq!(vm.fire(&mut frame, &hook, 0.7), HookEnd::Returned);
    #[allow(clippy::disallowed_methods, reason = "what the C library says is what's being shown")]
    let libm = 0.7_f64.sin();
    assert_eq!(marks(&frame), [libm], "a FASTCALL reached the C library");

    // And with the list, no FASTCALL at any level.
    assert!(DISABLED_BUILTINS.contains(&"math.sin"));
}

#[test]
fn fastcall_reaches_a_removed_function_too() {
    // `setmetatable` is gone from the globals, and a chunk with no list still gets it, weak tables and
    // all. The scan's what stops that, and it can't be walked around: the VM scans what it loads.
    let plain = mlua::chunk::Compiler::new().set_optimization_level(2);
    let bytes =
        plain.compile("return { hook = function() return setmetatable({}, {__mode = 'k'}) end }").unwrap();
    assert_eq!(scan(&bytes).unwrap().fastcalls, [61]);
    let vm = ScriptVm::new(Limits::default()).unwrap();
    assert!(vm.load(&bytes, "=test").is_err());
}

#[test]
fn weak_tables_have_no_road_in() {
    assert!(hook_fails("return setmetatable({}, {__mode = 'k'})").contains("nil"));
    assert!(hook_fails("return getmetatable('').__index").contains("nil"));
}

#[test]
fn a_power_reaches_c_only_where_the_scan_says() {
    // Exact in the VM: x * x, x * x * x, sqrt.
    assert_eq!(
        marked("local x = 1.5 sapphire.mark(x ^ 2) sapphire.mark(x ^ 3) sapphire.mark(x ^ 0.5)")[..2],
        [2.25, 3.375]
    );
    // Everything else.
    assert!(refused("return { hook = function(x) return x ^ 2.5 end }").contains("2.5"));
    assert!(refused("return { hook = function(x) return x ^ 1 end }").contains("^ 1"));
    assert!(refused("return { hook = function(x, y) return x ^ y end }").contains("`^`"));
    assert!(refused("local k = 2.5 return { hook = function(x) return x ^ k end }").contains("^"));
    // Folded above level 0: no POW left in the bytes to find, and C's answer in the constants.
    let folded = compile::compile_unchecked("return { hook = function() return 2 ^ 2.5 end }", 2).unwrap();
    let findings = scan(&folded).unwrap();
    assert_eq!((findings.pow, findings.pow_constants.len()), (0, 0), "nothing for a scan of level 2 to see");
    assert!(findings.numbers.contains(&5.656_854_249_492_381_f64.to_bits()), "{:?}", findings.numbers);
    // Which is why the cook reads level 0.
    assert!(refused("return { hook = function() return 2 ^ 2.5 end }").contains("two literals"));
    assert!(refused("return { hook = function() return 5.3 % 1.1 end }").contains("two literals"));
    // On record as a hole: a constant carried in a local. Level 0 can't see through it, and level 1 folds
    // it with the host's `pow`. It's bytes, so the run's still one run; `tables_hash` is what would show
    // a cook on another host differing.
    let carried = cook("local base = 1.5 return { hook = function() return base ^ 0.5 end }").unwrap();
    assert_eq!(scan(&carried).unwrap().pow, 0);
}

#[test]
fn a_script_cant_choose_its_own_level() {
    assert!(refused("--!optimize 2\nreturn {}").contains("compile options"));
    assert!(refused("--!native\nreturn {}").contains("compile options"));
}

// State between hooks.

#[test]
fn nothing_written_outlasts_a_hook_but_the_board() {
    // An outer local.
    assert!(refused("local n = 0 return { hook = function() n += 1 end }").contains("outer local"));
    // A table an outer local holds, however deep.
    let source = "local state = {deep = {n = 0}} return { hook = function() state.deep.n += 1 end }";
    let (end, _) = run_hook(source).unwrap();
    assert!(matches!(&end, HookEnd::Failed(error) if error.contains("readonly")), "{end:?}");
    // The hook table.
    let source = "local hooks = {} hooks.hook = function() hooks.n = 1 end return hooks";
    let (end, _) = run_hook(source).unwrap();
    assert!(matches!(&end, HookEnd::Failed(error) if error.contains("readonly")), "{end:?}");
    // Reachable only through a helper's upvalue.
    let source = "local t = {} local function helper() t.n = 1 end return { hook = function() helper() end }";
    let (end, _) = run_hook(source).unwrap();
    assert!(matches!(&end, HookEnd::Failed(error) if error.contains("readonly")), "{end:?}");
    // The board does.
    let mut host = ScriptHost::new(Limits::default()).unwrap();
    host.bind(&cook("return { on_phase = function() end }").unwrap(), "=test").unwrap();
    assert_eq!(host.board()[0].to_bits(), 0);
}

#[test]
fn a_coroutine_has_no_road_in() {
    assert!(hook_fails("return coroutine.wrap(function() end)").contains("nil"));
}

// The budget.

#[test]
fn a_budget_is_a_count_that_lands_on_one_instruction() {
    // Runs to an over-budget end twice; the board-free mark count a script reached can't differ, because
    // a hook over budget leaves nothing. So read the interrupts themselves.
    let spin = "return { hook = function() while true do end end }";
    let spent = |limit: u64| {
        let vm = ScriptVm::new(Limits { interrupts: limit, ..Limits::default() }).unwrap();
        let hooks = vm.load(&cook(spin).unwrap(), "=test").unwrap();
        let hook: mlua::Function = hooks.get("hook").unwrap();
        let mut frame = Frame::new();
        let before = ScriptVm::interrupts();
        let end = vm.fire(&mut frame, &hook, ());
        assert_eq!(end, HookEnd::OverBudget);
        ScriptVm::interrupts() - before
    };
    assert_eq!(spent(1000), spent(1000));
    assert_eq!(spent(1000), 1001, "the budget, then the interrupt that ends it");
    // A loop's cost is its shape, and level 0 is what ships.
    let counted = "return { hook = function() for i = 1, 500 do end local function f() end for i = 1, 100 do f() end end }";
    let vm = ScriptVm::new(Limits::default()).unwrap();
    let hooks = vm.load(&cook(counted).unwrap(), "=test").unwrap();
    let hook: mlua::Function = hooks.get("hook").unwrap();
    let mut frame = Frame::new();
    let before = ScriptVm::interrupts();
    assert_eq!(vm.fire(&mut frame, &hook, ()), HookEnd::Returned);
    let first = ScriptVm::interrupts() - before;
    let before = ScriptVm::interrupts();
    assert_eq!(vm.fire(&mut frame, &hook, ()), HookEnd::Returned);
    assert_eq!(ScriptVm::interrupts() - before, first);
}

#[test]
fn a_budget_ends_a_hook_wherever_it_spins() {
    for body in [
        "while true do end",
        "local function f() return f() end f()",
        "table.sort({3, 2, 1}, function(a, b) while true do end end)",
        "for k in pairs({a = 1}) do while true do end end",
        "for i, v in ipairs({1}) do while true do end end",
        "local function f(n) for i = 1, 1e9 do n += 1 end end f(1)",
    ] {
        let source = format!("return {{ hook = function() {body} end }}");
        let (end, _) = run_hook(&source).unwrap();
        assert_eq!(end, HookEnd::OverBudget, "{body}");
    }
}

/// A library walk spends the budget as it goes: C's `table.find` took one interrupt for 90,000 slots, so
/// a loop of them ran 970 ms under a budget meant for 25 µs.
#[test]
fn a_walk_of_a_table_spends_the_budget() {
    for body in ["table.find(t, 1)", "table.insert(t, 1, 0)", "table.remove(t, 1)"] {
        let source = format!("return {{ hook = function() local t = table.create(9000, 0) {body} end }}");
        let vm = ScriptVm::new(Limits { interrupts: 1_000_000, ..Limits::default() }).unwrap();
        let hook: mlua::Function = vm.load(&cook(&source).unwrap(), "=test").unwrap().get("hook").unwrap();
        let before = ScriptVm::interrupts();
        assert_eq!(vm.fire(&mut Frame::new(), &hook, ()), HookEnd::Returned, "{body}");
        assert!(ScriptVm::interrupts() - before >= 9000, "{body}: {}", ScriptVm::interrupts() - before);
    }
    assert_eq!(
        marked(
            "local t = {1, 2} table.insert(t, 3) table.insert(t, 1, 0) sapphire.mark(t[1]) sapphire.mark(t[4])"
        ),
        [0.0, 3.0]
    );
    assert_eq!(
        marked(
            "local t = {1, 2, 3} sapphire.mark(table.remove(t, 1)) sapphire.mark(table.remove(t)) sapphire.mark(#t)"
        ),
        [1.0, 3.0, 1.0]
    );
    assert_eq!(
        marked(
            "local t = {4, 5} sapphire.mark(table.find(t, 5)) sapphire.mark(table.find(t, 6) == nil and 1 or 0)"
        ),
        [2.0, 1.0]
    );
    assert_eq!(marked("local t = {} sapphire.mark(table.remove(t) == nil and 1 or 0)"), [1.0]);
    assert!(hook_fails("table.insert({}, 9, 1)").contains("out of bounds"));
    assert!(hook_fails("table.freeze({1}) table.insert(table.freeze({1}), 2)").contains("readonly"));
}

/// A string's only as long as a chunk spells it, so a loop over one is bounded: `..` built one of 256 KB
/// that a loop of `string.sub` then spent 230 ms copying and comparing under one budget.
#[test]
fn a_string_cant_grow() {
    assert!(refused("return { hook = function() local s = tostring(1) return s .. s end }").contains("`..`"));
    let long = "x".repeat(sapphire_script::scan::MAX_STRING + 1);
    assert!(refused(&format!("return {{ hook = function() return '{long}' end }}")).contains("bytes"));
    // Interpolation is `string.format`, which is gone.
    assert!(hook_fails("local x = 1 local s = `{x}`").contains("format"));
    assert_eq!(
        marked("local s = 'phase' sapphire.mark(#s) sapphire.mark(s:len()) sapphire.mark(#s:sub(2))"),
        [5.0, 5.0, 4.0]
    );
}

// One checksum.

/// Every library a script keeps, with its answers in the marks: what Linux and Windows have to agree on.
/// A number that differs in its last bit between C libraries or compilers reaches the checksum.
const ATTACK: &str = r#"
local WORDS = {'phase', 'rage', 'adds', 'timer', 'enrage', 'wave', 'door', 'seal'}
local DIGITS = {['0'] = 0, ['1'] = 1, ['2'] = 2, ['3'] = 3, ['4'] = 4, ['5'] = 5, ['6'] = 6, ['7'] = 7, ['8'] = 8, ['9'] = 9, ['.'] = 10}

local function fold(a, b)
    sapphire.note(b)
    return (a * 31 + b) % 4294967296
end

local function attack(seed)
    local h = seed
    seed = seed % 97

    -- Arithmetic the VM does itself, and %, which it does as a - floor(a / b) * b.
    local x = seed * 0.37 + 0.1
    h = fold(h, (x * 1.7 + x / 3.3 - x % 0.77) * 1000)
    h = fold(h, (x ^ 2 + x ^ 3 + x ^ 0.5) * 1000)
    h = fold(h, x // 0.3)
    local mark = x % 0.77
    sapphire.note(mark)

    -- Pinned math, through f32.
    for _, f in ipairs({math.sin, math.cos, math.tan, math.atan, math.exp, math.log}) do
        local y = f(x)
        sapphire.note(y)
        h = fold(h, y * 1000)
    end
    sapphire.note(math.atan2(x, 2.5))
    sapphire.note(math.pow(x, 2.5))
    sapphire.note(math.asin(0.25))
    sapphire.note(math.acos(0.25))

    -- What's left of math.
    h = fold(h, math.floor(x) + math.ceil(x) + math.round(x) + math.abs(-x) + math.sign(x))
    h = fold(h, math.max(x, 1) + math.min(x, 1) + math.clamp(x, 0, 1) + math.sqrt(x))
    sapphire.note(math.sqrt(x))
    sapphire.note(math.lerp(x, 3.7, 0.3))
    sapphire.note(math.deg(x))
    sapphire.note(math.rad(x))

    -- Vectors, which are float32 in the VM's C++, including the sums of products.
    local v = vector.create(x, x * 0.5, x * 0.25)
    local w = vector.create(0.3, x, 1.1)
    sapphire.note(vector.dot(v, w))
    sapphire.note(vector.magnitude(v))
    local n = vector.normalize(v)
    sapphire.note(n.x) sapphire.note(n.y) sapphire.note(n.z)
    local c = vector.cross(v, w)
    sapphire.note(c.x) sapphire.note(c.y) sapphire.note(c.z)
    local l = vector.lerp(v, w, 0.3)
    sapphire.note(l.x)
    local p = v * w + v / 3 - w
    sapphire.note(p.x) sapphire.note(p.y) sapphire.note(p.z)

    -- Bits.
    local whole = math.floor(x * 1000)
    h = fold(h, bit32.bxor(whole, 0xA5A5) + bit32.lrotate(whole, 7) + bit32.countlz(whole) + bit32.extract(whole, 3, 5))

    -- Numbers through Luau's own printer, read back a character at a time.
    for _, number in ipairs({x, x * 1e10, x / 1e10, 1e300, x ^ 2}) do
        local printed = tostring(number)
        for i = 1, #printed do
            h = fold(h, DIGITS[string.sub(printed, i, i)] or 11)
        end
    end
    h = fold(h, tonumber('1234'))

    -- A table's walk over string keys, its size and history changing, and a sort.
    local t = {}
    for i = 1, #WORDS do
        t[WORDS[(i * seed) % #WORDS + 1]] = i
    end
    t[WORDS[seed % #WORDS + 1]] = nil
    for k, value in pairs(t) do
        h = fold(h, value)
        h = fold(h, #k)
    end
    local list = {}
    for i = 1, 40 do
        list[i] = (i * 7919 + seed) % 101
    end
    table.sort(list)
    table.sort(list, function(a, b) return a > b end)
    h = fold(h, list[1] + list[20] * 2 + list[40] * 3)
    h = fold(h, table.find(list, list[7]))
    table.insert(list, 1, h % 7)
    h = fold(h, table.remove(list, 3) + table.remove(list))

    -- The simulation's randomness: it moves the simulation's checksum too.
    h = fold(h, sapphire.random() * 1000)
    h = fold(h, sapphire.health(0) + sapphire.health(1))

    sapphire.set_board(0, h)
    sapphire.set_board(1, sapphire.board(1) + 1)
    sapphire.note(h)
    sapphire.mark(h)
end

return {
    on_beat = function(tick) attack(tick) end,
    on_phase = function(actor, phase, health)
        attack(actor * 100 + phase)
        sapphire.slow_motion(500, 12)
    end,
}
"#;

/// A corpus for bytes: what a cook does to folding, at every level. What's in `numbers` at level 2 is
/// what the compiler worked out by itself on this host.
const CORPUS: &str = r#"
local a = 5.3 % 1.1
local b = 0.1 + 0.2
local c = 1 / 3
local d = 7 // 0.3
local e = 2 ^ 0.5
local g = 2 ^ 2.5
local h = 1.1 ^ 1.7
local i = 0.3 % 0.07
local j = 1234.567 % 0.0123
return { hook = function()
    sapphire.mark(a) sapphire.mark(b) sapphire.mark(c) sapphire.mark(d) sapphire.mark(e)
    sapphire.mark(g) sapphire.mark(h) sapphire.mark(i) sapphire.mark(j)
end }
"#;

/// Where a run's whole result is one line. Direct to stderr: `eprintln!` is captured on a pass, and a
/// CI log has to show this on a pass.
#[allow(
    clippy::explicit_write,
    reason = "to the real stderr, past the test harness's capture, so CI's logs show it"
)]
fn say(line: &str) {
    writeln!(std::io::stderr(), "script-digest|{ARCHITECTURE}|{line}").unwrap();
}

/// A run's scenario takes its host from one static.
fn attack_lock() -> std::sync::MutexGuard<'static, ()> {
    static ONE_AT_A_TIME: std::sync::Mutex<()> = std::sync::Mutex::new(());
    ONE_AT_A_TIME.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// `sapphire.note(x)`, for the attack alone: a number's bits, chained into the board's last slot, so the
/// digest sees every number a hook works out and not only the commands that fit its queue.
unsafe extern "C-unwind" fn note(state: *mut lua_State) -> c_int {
    // SAFETY: Luau passes a live state, and neither call raises.
    let value = unsafe {
        if ffi::lua_type(state, 1) == ffi::LUA_TNUMBER {
            ffi::lua_tonumberx(state, 1, std::ptr::null_mut())
        } else {
            f64::NAN
        }
    };
    with_frame(|frame| {
        let bits = if value.is_nan() { f64::NAN.to_bits() } else { value.to_bits() };
        let chain = frame.board[BOARD_SLOTS - 1].to_bits();
        let mixed = (chain ^ bits ^ (bits >> 52)).wrapping_mul(0x0000_0100_0000_01b3);
        // Kept below NaN's exponent, so that the digest's reading of a NaN can't flatten it.
        frame.board[BOARD_SLOTS - 1] = f64::from_bits(mixed & 0x000f_ffff_ffff_ffff);
    });
    0
}

fn attack_run_of(source: &str) -> (u64, u64, u64) {
    let limits = Limits { interrupts: 1_000_000, ..Limits::default() };
    let mut host =
        ScriptHost::new_with_bindings(limits, &[Binding { name: "note", function: note }]).unwrap();
    host.bind(&cook(source).unwrap(), "=attack").unwrap();
    host.beat_every(30);
    let finished = play(host, replay_file("arena-fight-won"));
    assert_eq!(finished.stats.hooks_failed, 0, "{:?}", finished.last_error);
    assert_eq!(finished.stats.hooks_over_budget, 0);
    assert_eq!(finished.stats.commands_dropped, 0, "a dropped command is a number the digest didn't see");
    assert!(finished.stats.hooks_fired > 50, "{:?}", finished.stats);
    assert!(finished.stats.commands_applied >= finished.stats.hooks_fired, "{:?}", finished.stats);
    assert!(finished.board[0].is_finite(), "a NaN would hide everything folded into it");
    assert_ne!(finished.board[BOARD_SLOTS - 1], 0.0);
    (finished.played.trail.end(), finished.digest, finished.used_memory as u64)
}

fn attack_run() -> (u64, u64, u64) {
    attack_run_of(ATTACK)
}

/// The digest has to see what the attack does, all the way to its last line, or one checksum across
/// machines says less than it seems to.
#[test]
fn the_attack_s_digest_sees_every_section() {
    let _one = attack_lock();
    let plain = attack_run().1;
    for (from, to) in [
        ("x % 0.77) * 1000", "x % 0.78) * 1000"),
        ("sapphire.note(vector.magnitude(v))", "sapphire.note(vector.magnitude(w))"),
        ("sapphire.note(p.z)", "sapphire.note(p.y)"),
        ("x / 1e10, 1e300", "x / 1e10, 1e299"),
        ("h = fold(h, value)", "h = fold(h, value + 1)"),
        ("table.sort(list, function(a, b) return a > b end)", "table.sort(list)"),
        ("sapphire.note(h)", "sapphire.note(h + 1)"),
    ] {
        assert!(ATTACK.contains(from), "{from}");
        assert_ne!(attack_run_of(&ATTACK.replace(from, to)).1, plain, "{from}");
    }
}

fn hash_bytes(bytes: &[u8]) -> u64 {
    let mut hash = StateHasher::new();
    hash.bytes(bytes);
    hash.finish()
}

/// Not a test of its own: what a child process prints for the parent to compare. It reads as passing
/// when nobody asks it for anything.
#[test]
#[allow(clippy::print_stdout, reason = "the parent process reads this line")]
fn child() {
    if std::env::var_os("SAPPHIRE_SCRIPT_CHILD").is_none() {
        return;
    }
    let (sim, digest, used) = attack_run();
    let lua = Lua::new();
    let printed: String = lua.load("return tostring({})").eval().unwrap();
    let order: String = lua
        .load(
            "local keys, index = {}, {} for i = 1, 32 do keys[i] = {} index[keys[i]] = i end
             local out = '' for k, v in pairs(index) do out ..= v .. ',' end return out",
        )
        .eval()
        .unwrap();
    let printed = printed.replace(' ', "_");
    let cooked = hash_bytes(&cook(ATTACK).unwrap());
    println!("SCRIPT-CHILD {sim:016x} {digest:016x} {used} {cooked:016x} {printed} {order}");
}

/// One hook-driven fight many times, each a process of its own, so each has its own addresses.
#[test]
fn the_same_hook_reads_one_checksum_under_aslr() {
    // CI's few are enough to read a checksum; `SAPPHIRE_SCRIPT_RUNS` asks for more.
    let runs: usize =
        std::env::var("SAPPHIRE_SCRIPT_RUNS").ok().and_then(|runs| runs.parse().ok()).unwrap_or(12);
    let me = std::env::current_exe().unwrap();
    let mut seen = Vec::new();
    for _ in 0..runs {
        let output = Process::new(&me)
            .args(["--exact", "child", "--nocapture"])
            .env("SAPPHIRE_SCRIPT_CHILD", "1")
            .output()
            .unwrap();
        assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
        let text = String::from_utf8_lossy(&output.stdout).into_owned();
        let line = text
            .lines()
            .find_map(|line| line.strip_prefix("SCRIPT-CHILD "))
            .expect("a child's line")
            .to_owned();
        seen.push(line.split(' ').map(str::to_owned).collect::<Vec<_>>());
    }
    let distinct = |column: usize| {
        let mut values: Vec<&str> = seen.iter().map(|row| row[column].as_str()).collect();
        values.sort_unstable();
        values.dedup();
        values.len()
    };
    say(&format!(
        "ASLR runs {runs}: sim {} digest {} used-memory {} cooked bytes {} | plain VM tostring(table) {} distinct, pairs over table keys {} distinct",
        distinct(0),
        distinct(1),
        distinct(2),
        distinct(3),
        distinct(4),
        distinct(5),
    ));
    assert_eq!(distinct(0), 1, "the simulation's checksum moved between runs: {seen:?}");
    assert_eq!(distinct(1), 1, "the script's digest moved between runs: {seen:?}");
    assert_eq!(distinct(2), 1, "the VM's memory moved between runs: {seen:?}");
    assert_eq!(distinct(3), 1, "a cook's bytes moved between runs: {seen:?}");
    // The negative: this machine's addresses do move, and a plain VM shows it, so one checksum above
    // means something.
    assert!(distinct(4) > 1, "tostring(table) never moved: no ASLR to attack here");
    assert!(distinct(5) > 1, "pairs over table keys never moved");
}

/// What the attack reads on every machine, a Luau upgrade or a change to `ATTACK` moving them on purpose.
const ATTACK_SIM: u64 = 0x4bdf_798c_11df_c65e;
const ATTACK_DIGEST: u64 = 0x02b7_0e41_88ba_5ceb;
const ATTACK_COOKED: u64 = 0xaeb4_3be9_564b_eb34;
const CORPUS_COOKED: [u64; 3] = [0x5242_6308_ea97_c17b, 0x4e2e_a36b_d34f_f76d, 0x4e2e_a36b_d34f_f76d];

/// What one cook means on every machine CI has: one line per OS for reading, and the same numbers held
/// here, so a machine that differs fails rather than waits to be read.
#[test]
fn the_checksums_say_what_they_are() {
    let (sim, digest, used) = {
        let _one = attack_lock();
        attack_run()
    };
    say(&format!("attack sim {sim:016x} digest {digest:016x} used-memory {used}"));
    assert_eq!((sim, digest), (ATTACK_SIM, ATTACK_DIGEST), "this machine's arithmetic differs");

    let cooked = cook(ATTACK).unwrap();
    assert_eq!(cooked, cook(ATTACK).unwrap(), "a cook is the same bytes twice");
    say(&format!("attack cooked len {} hash {:016x}", cooked.len(), hash_bytes(&cooked)));
    assert_eq!(hash_bytes(&cooked), ATTACK_COOKED, "this machine's cook differs");
    for level in [0, 1, 2] {
        let bytes = compile::compile_unchecked(CORPUS, level).unwrap();
        let numbers: Vec<String> =
            scan(&bytes).unwrap().numbers.iter().map(|bits| format!("{bits:016x}")).collect();
        say(&format!("corpus level {level} hash {:016x} numbers {}", hash_bytes(&bytes), numbers.join(",")));
        assert_eq!(
            hash_bytes(&bytes),
            CORPUS_COOKED[level as usize],
            "level {level}: this host folds differently"
        );
    }

    // Whether this build's VM and compiler fused a multiply and an add: `%` is a - floor(a / b) * b in one
    // C expression, and clang's default contracts that on aarch64, where a fused multiply-add exists.
    let pair = (1..100_000)
        .map(|i| (f64::from(i) * 0.37, 1.1 + f64::from(i) * 0.001))
        .find(|(a, b)| {
            let plain = a - (a / b).floor() * b;
            let fused = (-(a / b).floor()).mul_add(*b, *a);
            plain.to_bits() != fused.to_bits()
        })
        .expect("some numbers round differently fused");
    let (a, b) = pair;
    let plain = a - (a / b).floor() * b;
    let fused = (-(a / b).floor()).mul_add(b, a);
    let vm = marked(&format!(
        "local a, b = sapphire.board(0) + {a:?}, sapphire.board(0) + {b:?} sapphire.mark(a % b)"
    ))[0];
    let folded =
        scan(&compile::compile_unchecked(&format!("return {{ x = {a:?} % {b:?} }}"), 1).unwrap()).unwrap();
    let read = |bits: u64| match bits {
        bits if bits == plain.to_bits() => "plain",
        bits if bits == fused.to_bits() => "fused",
        _ => "neither",
    };
    say(&format!(
        "mod {a:?} % {b:?}: plain {:016x} fused {:016x}; the VM's is {}, the compiler's fold is {}",
        plain.to_bits(),
        fused.to_bits(),
        read(vm.to_bits()),
        folded.numbers.iter().map(|bits| read(*bits)).collect::<Vec<_>>().join("/"),
    ));
    let dot = marked(
        "local x, y = sapphire.board(0) + 0.1, sapphire.board(0) + 0.7
         sapphire.mark(math.lerp(x, y, 0.3)) sapphire.mark(vector.dot(vector.create(x, y, x), vector.create(y, x, y)))",
    );
    say(&format!("lerp {:016x} dot {:016x}", dot[0].to_bits(), dot[1].to_bits()));
}
