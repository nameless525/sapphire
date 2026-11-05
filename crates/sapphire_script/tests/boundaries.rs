//! Public callback and initialization boundaries. These regressions run only against the repaired VM.

use std::alloc::System;

use mlua::Table;
use sapphire_core::alloc::{self, CountingAllocator, VmAllocations};
use sapphire_script::bindings::{in_host, with_frame};
use sapphire_script::{Frame, Hook, HookArgs, HookEnd, Limits, ScriptVm, VmError, cook};

#[global_allocator]
static GLOBAL: CountingAllocator<System> = CountingAllocator::new(System);

fn hook(vm: &ScriptVm, source: &str) -> Hook {
    vm.load(&cook(source).unwrap(), "=boundary").unwrap().hook("hook").unwrap().unwrap()
}

#[test]
fn initializer_loops_are_bounded_and_failed_loads_leave_hooks_usable() {
    let vm = ScriptVm::new(Limits { interrupts: 20, ..Limits::default() }).unwrap();
    let finite =
        hook(&vm, "local x = 0 for i = 1, 3 do x += i end return {hook = function() sapphire.mark(x) end}");
    for source in
        ["while true do end return {}", "local function spin() while true do end end spin() return {}"]
    {
        let bytes = cook(source).unwrap();
        for _ in 0..8 {
            assert!(matches!(vm.load(&bytes, "=loop"), Err(VmError::OverBudget)));
            let mut frame = Frame::new();
            assert_eq!(vm.fire(&mut frame, &finite, ()), HookEnd::Returned);
            assert_eq!(frame.queue[0], sapphire_script::Command::Mark(6.0));
            while !vm.gc_step().unwrap() {}
        }
    }
}

#[test]
fn initialization_requires_enough_budget_and_does_not_retain_a_failed_stack() {
    let source = cook("return {hook = function() end}").unwrap();
    let vm = ScriptVm::new(Limits { interrupts: 0, ..Limits::default() }).unwrap();
    assert!(matches!(vm.load(&source, "=zero"), Err(VmError::OverBudget)));
    let vm = ScriptVm::new(Limits::default()).unwrap();
    assert!(matches!(vm.load(&cook("return 1").unwrap(), "=value"), Err(VmError::NotAHookTable)));
    assert!(vm.load(&cook("error('initializer')").unwrap(), "=error").is_err());
    assert!(vm.load(&source, "=good").is_ok());
}

struct Nested<'a> {
    vm: &'a ScriptVm,
    hook: &'a Hook,
}

impl From<Nested<'_>> for HookArgs {
    fn from(arguments: Nested<'_>) -> Self {
        with_frame(|outer| {
            outer.board[0] = 5.0;
            assert!(with_frame(|_| panic!("outer borrow is live")).is_none());
            let mut inner = Frame::new();
            assert_eq!(arguments.vm.fire(&mut inner, arguments.hook, ()), HookEnd::Returned);
            assert_eq!(inner.board[0], 9.0);
            assert!(with_frame(|_| panic!("outer borrow is still live")).is_none());
            outer.board[0] += 1.0;
            // A nested initializer must not see or mutate the calling hook's frame.
            arguments.vm.load(&cook("sapphire.set_board(0, 99) return {}").unwrap(), "=nested").unwrap();
            assert_eq!(outer.board[0], 6.0);
        })
        .expect("outer frame installed during conversion");
        HookArgs::None
    }
}

#[test]
fn nested_vms_restore_frame_access_and_the_callers_budget() {
    let outer = ScriptVm::new(Limits { interrupts: 20, ..Limits::default() }).unwrap();
    let inner = ScriptVm::new(Limits { interrupts: 2_000, ..Limits::default() }).unwrap();
    let spinning = hook(&outer, "return {hook = function() while true do end end}");
    let marking = hook(&inner, "return {hook = function() sapphire.set_board(0, 9) end}");
    let mut frame = Frame::new();
    let before = ScriptVm::interrupts();
    assert_eq!(outer.fire(&mut frame, &spinning, ()), HookEnd::OverBudget);
    let ordinary = ScriptVm::interrupts() - before;
    let before = ScriptVm::interrupts();
    assert_eq!(inner.fire(&mut Frame::new(), &marking, ()), HookEnd::Returned);
    // Include the initializer used by Nested in the inner cost, outside the outer hook.
    inner.load(&cook("sapphire.set_board(0, 99) return {}").unwrap(), "=nested").unwrap();
    let nested_cost = ScriptVm::interrupts() - before;
    let before = ScriptVm::interrupts();
    assert_eq!(outer.fire(&mut frame, &spinning, Nested { vm: &inner, hook: &marking }), HookEnd::OverBudget);
    assert_eq!(ScriptVm::interrupts() - before, ordinary + nested_cost);
    assert_eq!(frame.board[0], 6.0);
    assert!(with_frame(|_| ()).is_none());
}

#[test]
fn graph_freezing_covers_cycles_keys_shared_children_and_function_upvalues() {
    let vm = ScriptVm::new(Limits::default()).unwrap();
    let hooks = vm
        .load(
            &cook(
                r#"
        local key = {}
        local child = {}
        local root = {[key] = child, child = child}
        child.parent = root
        return {root = root, hook = function() child.changed = 1 end}
    "#,
            )
            .unwrap(),
            "=graph",
        )
        .unwrap();
    // SAFETY: this diagnostic only inspects compiler-created tables and executes no bytecode.
    let raw = unsafe { hooks.table_for_tests() };
    let root: Table = raw.get("root").unwrap();
    let child: Table = root.get("child").unwrap();
    assert!(hooks.is_readonly());
    assert!(root.is_readonly());
    assert!(child.is_readonly());
    for pair in root.pairs::<mlua::Value, mlua::Value>() {
        let (key, _) = pair.unwrap();
        if let mlua::Value::Table(key) = key {
            assert!(key.is_readonly());
        }
    }
    let hook = hooks.hook("hook").unwrap().unwrap();
    assert!(matches!(vm.fire(&mut Frame::new(), &hook, ()), HookEnd::Failed(_)));
}

#[test]
fn deep_graphs_use_vm_stack_capacity_without_native_recursion() {
    let vm = ScriptVm::new(Limits { memory: 8 << 20, interrupts: 100_000, ..Limits::default() }).unwrap();
    let hooks = vm.load(&cook("local root = {} local t = root for i = 1, 4000 do local child = {} t.child = child t = child end return {root = root}").unwrap(), "=deep").unwrap();
    // SAFETY: this diagnostic only inspects compiler-created tables and executes no bytecode.
    let mut table: Table = unsafe { hooks.table_for_tests() }.get("root").unwrap();
    for _ in 0..4000 {
        assert!(table.is_readonly());
        table = table.get("child").unwrap();
    }
    assert!(table.is_readonly());
}

struct Reentry<'a> {
    vm: &'a ScriptVm,
    hook: &'a Hook,
}

impl From<Reentry<'_>> for HookArgs {
    fn from(arguments: Reentry<'_>) -> Self {
        with_frame(|frame| {
            assert!(matches!(arguments.vm.fire(frame, arguments.hook, ()), HookEnd::Failed(_)));
        })
        .unwrap();
        HookArgs::None
    }
}

#[test]
fn reentering_one_vms_hook_does_not_replace_its_running_coroutine() {
    let vm = ScriptVm::new(Limits { interrupts: 20, ..Limits::default() }).unwrap();
    let spinning = hook(&vm, "return {hook = function() while true do end end}");
    let marking = hook(&vm, "return {hook = function() sapphire.mark(1) end}");
    let mut frame = Frame::new();
    assert_eq!(vm.fire(&mut frame, &spinning, Reentry { vm: &vm, hook: &marking }), HookEnd::OverBudget);
    assert_eq!(frame.queued, 0);
    assert_eq!(vm.fire(&mut frame, &marking, ()), HookEnd::Returned);
    assert_eq!(frame.queued, 1);
}

#[test]
fn nested_host_and_vm_entries_restore_allocation_attribution() {
    if !alloc::PER_THREAD_COUNTS {
        return;
    }
    let vm = ScriptVm::new(Limits::default()).unwrap();
    let allocating = hook(&vm, "return {hook = function() local t = {} for i = 1, 32 do t[i] = {i} end end}");
    let mut frame = Frame::new();
    let _outer_vm = VmAllocations::enter();
    let host_before = alloc::thread_counts().allocations;
    let vm_before = alloc::vm_counts().allocations;
    in_host(|| {
        drop(std::hint::black_box(vec![0u8; 64]));
        assert_eq!(vm.fire(&mut frame, &allocating, ()), HookEnd::Returned);
        drop(std::hint::black_box(vec![0u8; 64]));
    });
    assert_eq!(alloc::thread_counts().allocations - host_before, 2);
    let vm_after_hook = alloc::vm_counts().allocations;
    assert!(vm_after_hook > vm_before, "the hook allocated in the VM");
    drop(std::hint::black_box(vec![0u8; 64]));
    assert_eq!(alloc::vm_counts().allocations - vm_after_hook, 1);
}

struct Panicking;

impl From<Panicking> for HookArgs {
    fn from(_: Panicking) -> Self {
        with_frame(|frame| {
            frame.board[0] = 23.0;
            panic!("argument conversion unwinds before the VM resumes");
        });
        unreachable!()
    }
}

#[test]
fn a_panicking_argument_conversion_releases_frame_budget_and_coroutine_guards() {
    let vm = ScriptVm::new(Limits::default()).unwrap();
    let marking = hook(&vm, "return {hook = function() sapphire.mark(7) end}");
    let mut frame = Frame::new();
    let caught =
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| vm.fire(&mut frame, &marking, Panicking)));
    assert!(caught.is_err());
    assert!(with_frame(|_| ()).is_none());
    assert_eq!(frame.board[0], 23.0);
    assert_eq!(vm.fire(&mut frame, &marking, ()), HookEnd::Returned);
    assert_eq!(frame.queue[0], sapphire_script::Command::Mark(7.0));
}

struct Initializing<'a>(&'a ScriptVm);

impl From<Initializing<'_>> for HookArgs {
    fn from(arguments: Initializing<'_>) -> Self {
        assert_eq!(with_frame(|frame| frame.board[0]), Some(0.0));
        // This conversion has not borrowed the frame while either initialization runs.
        arguments.0.load(&cook("sapphire.set_board(0, 99) return {}").unwrap(), "=initializer").unwrap();
        let _new_vm = ScriptVm::new(Limits::default()).unwrap();
        assert_eq!(with_frame(|frame| frame.board[0]), Some(0.0));
        HookArgs::None
    }
}

#[test]
fn initialization_has_no_frame_even_when_a_calling_hooks_frame_is_available() {
    let vm = ScriptVm::new(Limits::default()).unwrap();
    let noop = hook(&vm, "return {hook = function() end}");
    let mut frame = Frame::new();
    assert_eq!(vm.fire(&mut frame, &noop, Initializing(&vm)), HookEnd::Returned);
    assert_eq!(frame.board[0], 0.0);
    assert!(!frame.fault);
}

#[test]
fn invalid_numeric_indices_cannot_alias_slot_zero_or_another_actor() {
    let vm = ScriptVm::new(Limits::default()).unwrap();
    for expression in ["-1", "0 / 0", "0.5", "math.huge", "64"] {
        let writing =
            hook(&vm, &format!("return {{hook = function() sapphire.set_board({expression}, 99) end}}"));
        let mut frame = Frame::new();
        frame.board[0] = 7.0;
        assert_eq!(vm.fire(&mut frame, &writing, ()), HookEnd::Returned);
        assert!(frame.fault, "{expression}");
        assert_eq!(frame.board[0], 7.0);
        let reading = hook(
            &vm,
            &format!("return {{hook = function() sapphire.mark(sapphire.board({expression})) end}}"),
        );
        frame.fault = false;
        assert_eq!(vm.fire(&mut frame, &reading, ()), HookEnd::Returned);
        assert!(frame.fault, "{expression}");
        assert_eq!(frame.queue[0], sapphire_script::Command::Mark(0.0));
    }
    for expression in ["-1", "0 / 0", "0.5", "math.huge", "16"] {
        let reading = hook(
            &vm,
            &format!("return {{hook = function() sapphire.mark(sapphire.health({expression})) end}}"),
        );
        let mut frame = Frame::new();
        frame.health[0] = 123.0;
        assert_eq!(vm.fire(&mut frame, &reading, ()), HookEnd::Returned);
        assert_eq!(frame.queue[0], sapphire_script::Command::Mark(0.0));
    }
}

#[test]
fn a_preseeded_full_drop_counter_stays_saturated() {
    let vm = ScriptVm::new(Limits::default()).unwrap();
    let marking = hook(&vm, "return {hook = function() sapphire.mark(1) end}");
    let mut frame = Frame::new();
    frame.queued = sapphire_script::bindings::QUEUE_LEN;
    frame.dropped = u32::MAX;
    assert_eq!(vm.fire(&mut frame, &marking, ()), HookEnd::Returned);
    assert_eq!(frame.dropped, u32::MAX);
}

#[test]
fn table_walk_positions_are_finite_whole_array_indices() {
    let vm = ScriptVm::new(Limits::default()).unwrap();
    for position in ["1.5", "0 / 0", "math.huge", "false"] {
        for operation in [
            format!("table.insert(t, {position}, 9)"),
            format!("table.remove(t, {position})"),
            format!("table.find(t, 2, {position})"),
        ] {
            let body =
                hook(&vm, &format!("return {{hook = function() local t = {{1, 2, 3}} {operation} end}}"));
            assert!(matches!(vm.fire(&mut Frame::new(), &body, ()), HookEnd::Failed(_)), "{operation}");
        }
    }
}

#[test]
fn budgeted_table_walks_reject_non_tables_even_when_they_would_do_no_work() {
    let vm = ScriptVm::new(Limits::default()).unwrap();
    for operation in ["table.find('', 1)", "table.remove('')", "table.sort('')"] {
        let body = hook(&vm, &format!("return {{hook = function() {operation} end}}"));
        assert!(matches!(vm.fire(&mut Frame::new(), &body, ()), HookEnd::Failed(_)), "{operation}");
    }
}

#[test]
fn trusted_compiler_output_round_trips_without_changing_bytes() {
    let original = cook("return {hook = function() sapphire.mark(17) end}").unwrap();
    let bytes = original.as_ref().to_vec();
    // SAFETY: this is an exact copy of the immutable output just produced by the pinned compiler.
    let restored = unsafe { sapphire_script::CookedScript::from_trusted_bytes(bytes) };
    assert_eq!(restored, original);
    let vm = ScriptVm::new(Limits::default()).unwrap();
    let hooks = vm.load(&restored, "=restored").unwrap();
    let mut frame = Frame::new();
    assert_eq!(vm.fire(&mut frame, &hooks.hook("hook").unwrap().unwrap(), ()), HookEnd::Returned);
    assert_eq!(frame.queue[0], sapphire_script::Command::Mark(17.0));
}

struct AllocatingArgs<'a> {
    inner: &'a ScriptVm,
    hook: &'a Hook,
}

impl From<AllocatingArgs<'_>> for HookArgs {
    fn from(arguments: AllocatingArgs<'_>) -> Self {
        drop(std::hint::black_box(vec![0u8; 64]));
        assert_eq!(arguments.inner.fire(&mut Frame::new(), arguments.hook, ()), HookEnd::Returned);
        drop(std::hint::black_box(vec![0u8; 64]));
        HookArgs::Three(2.0, 3.0, 5.0)
    }
}

#[test]
fn custom_argument_allocations_are_host_work_while_lua_allocations_remain_vm_work() {
    if !alloc::PER_THREAD_COUNTS {
        return;
    }
    let vm = ScriptVm::new(Limits::default()).unwrap();
    let allocating = hook(
        &vm,
        "return {hook = function(a, b, c) local t = {} for i = 1, 32 do t[i] = {i} end sapphire.set_board(0, a + b + c + #t) end}",
    );
    let inner = ScriptVm::new(Limits::default()).unwrap();
    let inner_allocating =
        hook(&inner, "return {hook = function() local t = {} for i = 1, 32 do t[i] = {i} end end}");
    let mut frame = Frame::new();
    // Even a caller already inside VM attribution must charge its custom Rust conversion to host.
    let _outer_vm = VmAllocations::enter();
    let host_before = alloc::thread_counts().allocations;
    let vm_before = alloc::vm_counts().allocations;
    assert_eq!(
        vm.fire(&mut frame, &allocating, AllocatingArgs { inner: &inner, hook: &inner_allocating }),
        HookEnd::Returned
    );
    assert_eq!(alloc::thread_counts().allocations - host_before, 2);
    assert!(alloc::vm_counts().allocations > vm_before, "the nested and outer Lua tables allocate in the VM");
    assert_eq!(frame.board[0], 42.0);
    let host_before = alloc::thread_counts().allocations;
    let vm_before = alloc::vm_counts().allocations;
    assert_eq!(vm.fire(&mut frame, &allocating, (2.0, 3.0, 5.0)), HookEnd::Returned);
    assert_eq!(alloc::thread_counts().allocations, host_before, "numeric conversion does not allocate");
    assert!(alloc::vm_counts().allocations > vm_before);
    let vm_after_hook = alloc::vm_counts().allocations;
    drop(std::hint::black_box(vec![0u8; 64]));
    assert_eq!(alloc::vm_counts().allocations - vm_after_hook, 1, "caller's attribution was restored");
}

#[test]
fn numeric_event_argument_shapes_preserve_arity_and_values() {
    let vm = ScriptVm::new(Limits::default()).unwrap();
    let recording = hook(
        &vm,
        "return {hook = function(...) sapphire.set_board(0, select('#', ...)) local a, b, c = ... sapphire.set_board(1, a or 0) sapphire.set_board(2, b or 0) sapphire.set_board(3, c or 0) end}",
    );
    for (args, expected) in [
        (HookArgs::from(()), [0.0, 0.0, 0.0, 0.0]),
        (HookArgs::from(2.0), [1.0, 2.0, 0.0, 0.0]),
        (HookArgs::from((2.0,)), [1.0, 2.0, 0.0, 0.0]),
        (HookArgs::from((2.0, 3.0)), [2.0, 2.0, 3.0, 0.0]),
        (HookArgs::from((2.0, 3.0, 5.0)), [3.0, 2.0, 3.0, 5.0]),
    ] {
        let mut frame = Frame::new();
        assert_eq!(vm.fire(&mut frame, &recording, args), HookEnd::Returned);
        assert_eq!(&frame.board[..4], &expected);
        assert!(!frame.fault);
    }
}

#[test]
fn hook_lookup_is_optional_typed_and_foreign_vm_rejection_does_not_change_either_vm() {
    let first = ScriptVm::new(Limits::default()).unwrap();
    let second = ScriptVm::new(Limits::default()).unwrap();
    let hooks = first
        .load(&cook("return {hook = function() sapphire.mark(17) end, invalid = 42}").unwrap(), "=lookup")
        .unwrap();
    assert!(hooks.hook("absent").unwrap().is_none());
    assert!(hooks.hook("invalid").is_err());
    let foreign = hooks.hook("hook").unwrap().unwrap();
    let local = hook(&second, "return {hook = function() sapphire.mark(23) end}");
    let mut frame = Frame::new();
    let refused = second.fire(&mut frame, &foreign, ());
    assert!(matches!(refused, HookEnd::Failed(_)));
    assert_eq!(frame.queued, 0);
    assert!(with_frame(|_| ()).is_none());
    assert_eq!(second.fire(&mut frame, &local, ()), HookEnd::Returned);
    assert_eq!(frame.queue[0], sapphire_script::Command::Mark(23.0));
    frame.queued = 0;
    assert_eq!(first.fire(&mut frame, &foreign, ()), HookEnd::Returned);
    assert_eq!(frame.queue[0], sapphire_script::Command::Mark(17.0));
}

struct TrustedTableArgs;

impl mlua::IntoLuaMulti for TrustedTableArgs {
    fn into_lua_multi(self, lua: &mlua::Lua) -> mlua::Result<mlua::MultiValue> {
        let number = in_host(|| {
            let host = std::hint::black_box(vec![17.0f64]);
            host[0]
        });
        let table = lua.create_table()?;
        table.raw_set("number", number)?;
        mlua::IntoLuaMulti::into_lua_multi(table, lua)
    }
}

#[test]
fn explicit_trusted_integration_retains_arbitrary_lua_arguments_with_honest_attribution() {
    let vm = ScriptVm::new(Limits::default()).unwrap();
    let reading = hook(&vm, "return {hook = function(value) sapphire.set_board(0, value.number) end}");
    let mut frame = Frame::new();
    let host_before = alloc::thread_counts().allocations;
    let vm_before = alloc::vm_counts().allocations;
    // SAFETY: the hook was cooked by the pinned compiler. The converter only creates ordinary Lua
    // data, explicitly counts its Rust allocation as host work, and executes/retains no new code.
    let end = unsafe { vm.fire_trusted(&mut frame, reading.function_for_tests(), TrustedTableArgs) };
    assert_eq!(end, HookEnd::Returned);
    assert_eq!(frame.board[0], 17.0);
    if alloc::PER_THREAD_COUNTS {
        assert_eq!(alloc::thread_counts().allocations - host_before, 1);
        assert!(alloc::vm_counts().allocations > vm_before);
    }
}

struct DroppingArgs<'a>(&'a std::cell::Cell<bool>);

impl Drop for DroppingArgs<'_> {
    fn drop(&mut self) {
        self.0.set(true);
        drop(std::hint::black_box(vec![0u8; 64]));
    }
}

impl From<DroppingArgs<'_>> for HookArgs {
    fn from(_: DroppingArgs<'_>) -> Self {
        panic!("foreign hook rejection must happen before argument conversion");
    }
}

#[test]
fn rejected_numeric_arguments_drop_as_host_work_before_restoring_an_outer_vm_scope() {
    let first = ScriptVm::new(Limits::default()).unwrap();
    let foreign = hook(&first, "return {hook = function() end}");
    let second = ScriptVm::new(Limits::default()).unwrap();
    let dropped = std::cell::Cell::new(false);
    let _outer_vm = VmAllocations::enter();
    let host_before = alloc::thread_counts().allocations;
    let vm_before = alloc::vm_counts().allocations;
    assert!(matches!(second.fire(&mut Frame::new(), &foreign, DroppingArgs(&dropped)), HookEnd::Failed(_)));
    assert!(dropped.get());
    if alloc::PER_THREAD_COUNTS {
        assert_eq!(alloc::vm_counts().allocations, vm_before);
        assert!(
            alloc::thread_counts().allocations - host_before >= 2,
            "argument drop and rejection diagnostic are host work"
        );
        drop(std::hint::black_box(vec![0u8; 64]));
        assert_eq!(alloc::vm_counts().allocations - vm_before, 1);
    }
}
