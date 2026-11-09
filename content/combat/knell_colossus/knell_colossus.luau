-- The Knell Colossus's script (M3 C2): its second wind. Its kind's document names this file, so a fight with
-- it in binds it, and nothing in code names either. Its phases are in the document, which says the phase's
-- name as the boss's cue for presentation (its bar's mark, the music); what the boss does there is here.

local TOLLING, PHASE_SHIFT = 1, 2

return {
    names = { "tolling", "phase_shift" },

    -- At half health, its first phase: it takes up its fiercer brain and tolls its bell at once, whatever
    -- it was doing. Anyone else crossing a phase has neither name, so nothing happens to them.
    on_phase = function(actor, phase, health)
        if phase == 1 and health > 0 then
            sapphire.set_brain(actor, TOLLING)
            sapphire.force_move(actor, PHASE_SHIFT)
        end
    end,
}
