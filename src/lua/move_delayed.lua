-- lua/move_delayed.lua
-- Move delayed jobs to priority wait buckets when due.
-- ARGV[1] = queue_name
-- ARGV[2] = current_timestamp (unix seconds)
-- ARGV[3] = key_prefix (e.g. rbq)
-- Returns comma-separated job IDs moved (empty string if none).

local queue_name = ARGV[1]
local now = tonumber(ARGV[2])
local prefix = ARGV[3]

local delayed_key = prefix .. ":queue:" .. queue_name .. ":delayed"

local jobs = redis.call('zrangebyscore', delayed_key, '-inf', now, 'LIMIT', 0, 100)

if #jobs == 0 then
    return ""
end

for _, job_id in ipairs(jobs) do
    redis.call('zrem', delayed_key, job_id)
    local job_key = prefix .. ":job:" .. job_id
    local pri = redis.call('hget', job_key, 'priority')
    if not pri then
        pri = '5'
    end
    local lifo = redis.call('hget', job_key, 'enqueue_lifo')
    if not lifo then
        lifo = '0'
    end
    local wait_key
    if lifo == '1' then
        wait_key = prefix .. ":queue:" .. queue_name .. ":waitl:p" .. pri
        redis.call('rpush', wait_key, job_id)
    else
        wait_key = prefix .. ":queue:" .. queue_name .. ":wait:p" .. pri
        redis.call('lpush', wait_key, job_id)
    end
end

return table.concat(jobs, ",")
