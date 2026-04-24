-- lua/promote_repeat.lua
-- Interval repeat promotion: tick SET NX, advance ZSET score, return lines "schedule_id,fire_unix".
-- ARGV[1] = queue_name
-- ARGV[2] = now (unix seconds)
-- ARGV[3] = key_prefix
-- ARGV[4] = tick_ttl_secs
-- Cron rows (cron_expr set, interval_secs empty) are left for Rust to handle.

local queue_name = ARGV[1]
local now = tonumber(ARGV[2])
local prefix = ARGV[3]
local tick_ttl = tonumber(ARGV[4])

local repeat_key = prefix .. ":queue:" .. queue_name .. ":repeat"
local raw = redis.call("ZRANGEBYSCORE", repeat_key, "-inf", now, "WITHSCORES", "LIMIT", 0, 100)

local out_parts = {}

for i = 1, #raw, 2 do
    local schedule_id = raw[i]
    local fire_unix = tonumber(raw[i + 1])
    if fire_unix == nil then
        fire_unix = 0
    end

    local hkey = prefix .. ":repeat:" .. schedule_id
    local enabled = redis.call("hget", hkey, "enabled")
    if enabled == "0" then
        redis.call("zrem", repeat_key, schedule_id)
    else
        local cron_expr = redis.call("hget", hkey, "cron_expr")
        if cron_expr and cron_expr ~= "" then
            -- Cron schedules are promoted from Rust (process_repeat_cron_due).
        else
            local interval_s = redis.call("hget", hkey, "interval_secs")
            if interval_s and interval_s ~= "" then
                local iv = tonumber(interval_s)
                if iv and iv > 0 then
                    local tick_key = prefix
                        .. ":repeat:tick:"
                        .. queue_name
                        .. ":"
                        .. schedule_id
                        .. ":"
                        .. tostring(math.floor(fire_unix))
                    if redis.call("set", tick_key, "1", "nx", "ex", tick_ttl) then
                        local next_unix = fire_unix + iv
                        redis.call("zadd", repeat_key, next_unix, schedule_id)
                        table.insert(out_parts, schedule_id .. "," .. tostring(math.floor(fire_unix)))
                    end
                end
            end
        end
    end
end

if #out_parts == 0 then
    return ""
end
return table.concat(out_parts, "\n")
