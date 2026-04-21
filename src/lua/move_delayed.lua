-- lua/move_delayed.lua
-- Move delayed jobs from sorted set to waiting list when due.
-- ARGV[1] = queue_name
-- ARGV[2] = current_timestamp (unix seconds)
-- ARGV[3] = key_prefix (e.g. rbq)
-- Returns comma-separated job IDs moved (empty string if none).

local queue_name = ARGV[1]
local now = tonumber(ARGV[2])
local prefix = ARGV[3]

local delayed_key = prefix .. ":queue:" .. queue_name .. ":delayed"
local wait_key = prefix .. ":queue:" .. queue_name .. ":wait"

local jobs = redis.call('zrangebyscore', delayed_key, '-inf', now, 'LIMIT', 0, 100)

if #jobs == 0 then
    return ""
end

for i, job_id in ipairs(jobs) do
    redis.call('zrem', delayed_key, job_id)
    redis.call('lpush', wait_key, job_id)
end

redis.call('publish', prefix .. ':events:' .. queue_name,
    string.format('{"type":"delayed_moved","count":%d,"timestamp":%d}', #jobs, now))

return table.concat(jobs, ",")
