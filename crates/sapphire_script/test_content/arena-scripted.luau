-- `arena-scripted`'s chunk: every hook, query and command of the API on a real fight, as a boss's might
-- use them. It leaves what it sees on the board, so a checksum holds it.

local LISTED_MOVES, HITS, DAMAGE, DEFEATS, LAST_SIDE, SPREAD, LAST_TARGET, BEATS = 0, 1, 2, 3, 4, 5, 6, 7

local SWIPE, OVERHEAD, LUNGE, STEADY = 1, 2, 3, 4

local function add(slot, value)
    sapphire.set_board(slot, sapphire.board(slot) + value)
end

return {
    names = { "swipe", "overhead", "lunge", "steady" },

    on_phase = function(actor, phase, health)
        sapphire.mark(actor * 100 + phase)
        if phase == 2 and sapphire.max_health(actor) > 0 then
            sapphire.slow_motion(500, 30)
            -- A boss would change to a fiercer brain here. The warrior has one, so this asks for it again.
            sapphire.set_brain(actor, STEADY)
        end
        if phase == 3 and health > 0 then
            sapphire.force_move(actor, OVERHEAD)
        end
    end,

    on_move = function(actor, name)
        if name ~= 0 then
            add(LISTED_MOVES, 1)
        end
    end,

    on_hit = function(attacker, target, damage)
        add(HITS, 1)
        add(DAMAGE, damage)
        -- A brisk answer now and then, from whoever was hit, if they're still up.
        if sapphire.board(HITS) % 7 == 0 and sapphire.is_defeated(target) == 0 then
            sapphire.force_move(target, if sapphire.random() < 0.5 then SWIPE else LUNGE)
        end
    end,

    on_defeat = function(actor)
        add(DEFEATS, 1)
        sapphire.mark(sapphire.tick())
        sapphire.set_board(LAST_SIDE, sapphire.faction(actor))
        -- Whoever they'd picked as a foe wakes up to it.
        local target = sapphire.brain_target(actor)
        if target >= 0 then
            sapphire.give_soft_lock(target)
            sapphire.give_brain(target)
        end
    end,

    on_beat = function(tick)
        add(BEATS, 1)
        local x, y, z = sapphire.position(1)
        local far = 0
        for other = 2, 8 do
            local ox, oy, oz = sapphire.position(other)
            local dx, dz = ox - x, oz - z
            far = math.max(far, math.sqrt(dx * dx + dz * dz) + oy - y)
        end
        sapphire.set_board(SPREAD, far)
        sapphire.set_board(LAST_TARGET, sapphire.soft_lock_target(1))
        sapphire.mark(sapphire.health(1) + z)
    end,
}
