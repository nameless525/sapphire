-- `dialogue-flag`'s chunk (M3 Q2): a fight that answers a story flag. Once the guide's choice has lit the
-- ward, the next beat wakes the first warrior with an overhead, and remembers it has by a flag of its own.
-- It leaves what it reads on the board, so a checksum holds it.

local WARD_LIT, WARD_ANSWERED, OVERHEAD = 1, 2, 3

local SEEN_WARD, ANSWERED_AT = 0, 1

-- The warrior the scenario spawns first, after the player.
local WARRIOR = 2

return {
    names = { "ward_lit", "ward_answered", "overhead" },

    on_beat = function(tick)
        local lit = sapphire.flag(WARD_LIT)
        sapphire.set_board(SEEN_WARD, lit)
        if lit >= 1 and sapphire.flag(WARD_ANSWERED) == 0 then
            sapphire.set_flag(WARD_ANSWERED, 1)
            sapphire.set_board(ANSWERED_AT, tick)
            sapphire.force_move(WARRIOR, OVERHEAD)
        end
    end,
}
