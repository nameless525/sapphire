-- What the sandbox's library has to be in Luau rather than Rust, run once before the sandbox closes.
--
-- An error raised here comes from Luau's own `error`, all C++ frames, so it's caught by the hook's
-- resume in every build. The same error from a Rust binding is a C++ exception crossing a Rust frame,
-- which ends the process under `panic = "abort"`: that's why these check here.

local next, type, error, select, raw_tostring, raw_tonumber = next, type, error, select, tostring, tonumber
local byte, len = string.byte, string.len

-- `pairs` only over keys whose hash comes from their value: a table, function or thread hashes by its
-- address, so its place in the walk changes from run to run.
--
-- The loop's third value is `START` and not nil for a reason: `for k in pairs(t)` compiles to
-- FORGPREP_NEXT, and in a safe environment that instruction looks only for a table and a nil, and then
-- walks the table itself. The function `pairs` returned never runs. Luau 0.740, `lvmexecute.cpp`.
local START = table.freeze({})

local function checked_next(t, key)
	if key == START then
		key = nil
	end
	local next_key, value = next(t, key)
	if next_key ~= nil then
		local kind = type(next_key)
		if kind ~= "string" and kind ~= "number" and kind ~= "boolean" then
			error("pairs over a table with a " .. kind .. " for a key: its order follows addresses", 2)
		end
	end
	return next_key, value
end

function pairs(t)
	if type(t) ~= "table" then
		error("pairs takes a table", 2)
	end
	return checked_next, t, START
end

-- A reference prints its address.
function tostring(value)
	local kind = type(value)
	if kind == "string" or kind == "number" or kind == "boolean" or kind == "nil" then
		return raw_tostring(value)
	end
	error("tostring of a " .. kind .. " prints its address", 2)
end

-- Text goes through the C library's `strtod`, whose rounding and spellings are its own, so only whole
-- numbers in plain digits are read.
function tonumber(value)
	local kind = type(value)
	if kind == "number" then
		return value
	end
	if kind ~= "string" then
		return nil
	end
	local size = len(value)
	local first = 1
	if size > 0 and byte(value, 1) == 45 then
		first = 2
	end
	if first > size or size - first >= 15 then
		return nil
	end
	for i = first, size do
		local digit = byte(value, i)
		if digit < 48 or digit > 57 then
			return nil
		end
	end
	return raw_tonumber(value)
end

-- These walk a table in Luau for the budget's sake: an interrupt fires on each back-edge, where C's walk
-- 90,000 slots on one, which at 10,000 interrupts was 970 ms of `table.find`.
-- The sandbox's table walks accept array positions, never fractional/hash keys. Refuse an invalid
-- position before shifting elements; allowing one could silently change which values survive.
local function array_position(position)
	if type(position) ~= "number" or position % 1 ~= 0 then
		error("table position must be a finite integer", 3)
	end
	return position
end

function table.find(t, value, from)
	if type(t) ~= "table" then
		error("table.find takes a table", 2)
	end
	if from == nil then
		from = 1
	end
	from = array_position(from)
	if from < 1 then
		error("'find' position out of bounds", 2)
	end
	for i = from, #t do
		if t[i] == value then
			return i
		end
	end
	return nil
end

-- Two arguments never get here unless the table's frozen: they compile to FASTCALL's append.
function table.insert(t, ...)
	if type(t) ~= "table" then
		error("table.insert takes a table", 2)
	end
	local count = select("#", ...)
	if count == 1 then
		t[#t + 1] = (...)
		return
	end
	if count ~= 2 then
		error("wrong number of arguments to 'insert'", 2)
	end
	local position, value = ...
	position = array_position(position)
	local size = #t
	if position < 1 or position > size + 1 then
		error("'insert' position out of bounds", 2)
	end
	for i = size, position, -1 do
		t[i + 1] = t[i]
	end
	t[position] = value
end

function table.remove(t, position)
	if type(t) ~= "table" then
		error("table.remove takes a table", 2)
	end
	local size = #t
	if position == nil then
		position = size
	end
	position = array_position(position)
	if size == 0 and position == 0 then
		return nil
	end
	if position < 1 or position > size + 1 then
		error("'remove' position out of bounds", 2)
	end
	local value = t[position]
	for i = position, size - 1 do
		t[i] = t[i + 1]
	end
	if position <= size then
		t[size] = nil
	end
	return value
end

-- A sort in Luau: the C one calls its comparison across a C boundary, where a hook's budget can't end
-- it, and its quicksort's choices are Luau's to change. Heapsort, so it needs nothing but the array.
local function sift(t, less, first, last)
	local root = first
	while root * 2 <= last do
		local child = root * 2
		if child < last and less(t[child], t[child + 1]) then
			child += 1
		end
		if not less(t[root], t[child]) then
			return
		end
		t[root], t[child] = t[child], t[root]
		root = child
	end
end

local function ascending(a, b)
	return a < b
end

function table.sort(t, less)
	if type(t) ~= "table" then
		error("table.sort takes a table", 2)
	end
	less = less or ascending
	local size = #t
	for first = size // 2, 1, -1 do
		sift(t, less, first, size)
	end
	for last = size, 2, -1 do
		t[1], t[last] = t[last], t[1]
		sift(t, less, 1, last - 1)
	end
end
