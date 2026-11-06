//! Gate 4 with a VM in the step (spec §5.24): a real recorded fight under `CountingAllocator`, with hooks
//! firing inside its fixed steps. The VM's allocations go to their own count, a step with no hook reads
//! zero, and a Rust allocation in a binding fails the gate as any other allocation would.

mod support;

use std::alloc::System;
use std::ffi::c_int;
use std::sync::Mutex;

use mlua::lua_State;
use sapphire_core::alloc::{self, CountingAllocator, PER_THREAD_COUNTS};
use sapphire_gameplay::replay::{RecordedTrail, SCENARIOS, find_scenario};
use sapphire_script::bindings::{in_host, with_frame};
use sapphire_script::{Binding, Limits, ScriptHost, VmError, cook, host_of};
use support::{host_running, play, player_with, replay_file};

#[global_allocator]
static GLOBAL: CountingAllocator<System> = CountingAllocator::new(System);

/// A boss's phase change as a designer would write it: reads, a little arithmetic, one table of tuning,
/// a loop over adds, three commands.
const BOSS_PHASE: &str = r#"
local TUNING = {
    {slow = 500, steps = 12, rage = 1.0, adds = 2},
    {slow = 250, steps = 20, rage = 1.5, adds = 3},
    {slow = 200, steps = 30, rage = 2.0, adds = 5},
}

return {
    on_phase = function(actor, phase, health)
        local tuning = TUNING[phase]
        if tuning == nil then
            return
        end
        local rage = tuning.rage * (1 + (1 - health / (health + 100)) * 0.5)
        sapphire.set_board(actor * 4, phase)
        sapphire.set_board(actor * 4 + 1, rage)
        sapphire.slow_motion(tuning.slow, tuning.steps)
        for add = 1, tuning.adds do
            sapphire.mark(add * rage)
        end
    end,
}
"#;

/// The same, the way a hook that isn't careful reads: new tables and strings every call.
const BOSS_PHASE_CARELESS: &str = r#"
local function plan(phase)
    return {
        name = {phase, phase + 1},
        adds = {},
        slow = phase == 1 and 500 or 250,
    }
end

return {
    on_phase = function(actor, phase, health)
        local p = plan(phase)
        for i = 1, phase * 3 do
            p.adds[i] = {index = i, rage = health / i}
        end
        table.sort(p.adds, function(a, b) return a.rage < b.rage end)
        sapphire.set_board(actor * 4, #p.adds)
        sapphire.slow_motion(p.slow, 10)
        sapphire.mark(#p.name)
    end,
    on_beat = function(tick)
        local p = plan(tick % 3 + 1)
        sapphire.mark(#p.name)
    end,
}
"#;

#[allow(
    clippy::explicit_write,
    reason = "to the real stderr, past the test harness's capture, so CI's logs show it"
)]
// Tests here share a global allocator's thread counts only by thread, so they can run side by side, but
// the scenario's waiting host is one static.
static ONE_AT_A_TIME: Mutex<()> = Mutex::new(());

#[test]
fn a_hook_in_a_real_replay_passes_the_gate_and_the_vm_has_its_own_count() {
    if !PER_THREAD_COUNTS {
        return;
    }
    let _one = ONE_AT_A_TIME.lock().unwrap();
    assert!(alloc::is_installed());
    let mut host = host_running(BOSS_PHASE, Limits::default());
    host.beat_every(0);
    let finished = play(host, replay_file("arena-fight-won"));
    finished.played.allocation_gate().expect("a hook's VM isn't gate 4's, and nothing else allocated");
    let stats = finished.stats;
    assert!(stats.hooks_fired >= 3, "{stats:?}");
    assert_eq!((stats.hooks_failed, stats.hooks_over_budget), (0, 0), "{stats:?}");
    assert!(stats.commands_applied > stats.hooks_fired, "{stats:?}");
}

/// A hook that asks for nothing changes nothing: a fight with one on it reaches the recorded checksums.
#[test]
fn a_hook_that_commands_nothing_leaves_the_recording_alone() {
    let _one = ONE_AT_A_TIME.lock().unwrap();
    let plain = find_scenario(SCENARIOS, "arena-fight").expect("listed");
    let mut player =
        sapphire_gameplay::replay::Player::new(replay_file("arena-fight-won"), std::slice::from_ref(plain))
            .unwrap();
    player.run_to_end();
    let recorded = RecordedTrail::from_trail(&player.finish().trail);

    let mut host = host_running(
        "return { on_phase = function(actor, phase) sapphire.set_board(0, actor + phase) end, on_beat = function() local t = {} t.x = 1 end }",
        Limits::default(),
    );
    host.beat_every(7);
    let finished = play(host, replay_file("arena-fight-won"));
    assert_eq!(recorded.first_difference(&finished.played.trail), None);
    assert!(finished.stats.hooks_fired > 100);
}

/// Allocates and returns, as a careless generated binding could.
unsafe extern "C-unwind" fn allocating(_state: *mut lua_State) -> c_int {
    with_frame(|frame| {
        let kept = std::hint::black_box(vec![0u8; 64]);
        frame.board[63] += kept.len() as f64;
    });
    0
}

/// [`in_host`] with no frame: what a pure one does.
unsafe extern "C-unwind" fn allocating_with_no_frame(_state: *mut lua_State) -> c_int {
    in_host(|| drop(std::hint::black_box(vec![0u8; 64])));
    0
}

/// Allocates with the thread still on the VM's count, so gate 4 never sees it.
unsafe extern "C-unwind" fn unflipped(_state: *mut lua_State) -> c_int {
    drop(std::hint::black_box(vec![0u8; 64]));
    0
}

/// Touches nothing and goes nowhere: as unflipped as the one above, whatever it does.
unsafe extern "C-unwind" fn unflipped_empty(_state: *mut lua_State) -> c_int {
    0
}

fn host_with_binding(function: mlua::ffi::lua_CFunction, source: &str) -> ScriptHost {
    // SAFETY: these test callbacks validate access through with_frame/in_host, never raise, and
    // return zero results on the supplied live state; their deliberate allocation is the gate probe.
    let binding = unsafe { Binding::new("careless", function) };
    let mut host =
        ScriptHost::new_with_bindings(Limits::default(), &[binding]).expect("the binding goes through");
    host.bind(&cook(source).unwrap(), "=test").unwrap();
    host
}

/// The negative: gate 4 still bites. A binding that allocates fails the very replay above, at the tick
/// of the hook, whichever door it goes through.
#[test]
fn a_binding_that_allocates_fails_the_gate_at_its_tick() {
    if !PER_THREAD_COUNTS {
        return;
    }
    let _one = ONE_AT_A_TIME.lock().unwrap();
    for binding in [allocating as mlua::ffi::lua_CFunction, allocating_with_no_frame] {
        let host = host_with_binding(binding, "return { on_phase = function() sapphire.careless() end }");
        let finished = play(host, replay_file("arena-fight-won"));
        let failure = finished.played.allocation_gate().expect_err("the binding's allocation is gate 4's");
        assert!(failure.allocations >= 3, "{failure:?}");
    }
}

/// With the VM's count diverted, a binding that doesn't flip back could allocate past the gate. So one
/// can't start a VM at all, and a generated table with such a row fails every test that starts one.
#[test]
fn a_binding_that_doesnt_flip_back_cant_start_a_vm() {
    for function in [unflipped as mlua::ffi::lua_CFunction, unflipped_empty] {
        // SAFETY: both callbacks preserve the live stack, return no results and never raise. Their
        // missing host-attribution entry is deliberately rejected by the separate startup policy check.
        let binding = unsafe { Binding::new("careless", function) };
        let refused = ScriptHost::new_with_bindings(Limits::default(), &[binding]);
        assert!(
            matches!(refused, Err(VmError::UnflippedBinding("careless"))),
            "{:?}",
            refused.err().map(|error| error.to_string()),
        );
    }
}

/// A VM's allocations, a hook at a time, once warm: what a budget would be set from.
#[test]
fn what_hooks_allocate_once_warm() {
    if !PER_THREAD_COUNTS {
        return;
    }
    let _one = ONE_AT_A_TIME.lock().unwrap();
    for source in [BOSS_PHASE, BOSS_PHASE_CARELESS] {
        let mut host = host_running(source, Limits::default());
        host.beat_every(5);
        let mut player = player_with(host, replay_file("arena-fight-won"));
        let mut seen = Vec::new();
        let mut last = alloc::vm_counts();
        let mut last_fired = 0;
        let mut with_no_hook = 0u64;
        while player.step() {
            let counts = alloc::vm_counts();
            let host = host_of(player.simulation());
            let fired = host.stats().hooks_fired;
            let spent = counts.allocations - last.allocations;
            if fired == last_fired {
                with_no_hook += spent;
            } else {
                seen.push(spent);
            }
            last_fired = fired;
            host.gc_step().unwrap();
            last = alloc::vm_counts();
        }
        assert_eq!(with_no_hook, 0, "a step with no hook made VM allocations");
        let warm = &seen[seen.len() / 2..];
        assert_eq!(warm.iter().sum::<u64>(), 0, "a warm hook's garbage reuses the VM's pages: {seen:?}");
    }
}

/// A collector stepped once a tick keeps a hook that makes garbage every tick at one size, with the
/// stop in between: stopped isn't the same as never.
#[test]
fn a_step_a_tick_keeps_up_with_a_hook_a_tick() {
    let _one = ONE_AT_A_TIME.lock().unwrap();
    let mut host = host_running(BOSS_PHASE_CARELESS, Limits::default());
    host.beat_every(1);
    let mut player = player_with(host, replay_file("arena-fight-won"));
    let mut sizes = Vec::new();
    while player.step() {
        let host = host_of(player.simulation());
        host.gc_step().unwrap();
        assert!(host.vm().collector_stopped());
        sizes.push(host.vm().used_memory());
    }
    let second = sizes[sizes.len() / 2..].iter().copied().max().unwrap();
    let first = sizes[..sizes.len() / 2].iter().copied().max().unwrap();
    assert!(second <= first + first / 4, "it keeps growing: {first} then {second}");
    assert_eq!(host_of(player.simulation()).stats().hooks_failed, 0);
}
