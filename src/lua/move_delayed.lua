-- lua/move_delayed.lua
-- Move delayed jobs from sorted set to waiting list when their time is due
-- ARGV[1] = queue_name
-- ARGV[2] = current_timestamp

local queue_name = ARGV[1]
local now = tonumber(ARGV[2])

local delayed_key = "rbq:queue:" .. queue_name .. ":delayed"
local wait_key = "rbq:queue:" .. queue_name .. ":wait"

-- Get jobs that are due (score <= now)
local jobs = redis.call('zrangebyscore', delayed_key, '-inf', now, 'LIMIT', 0, 100)

if #jobs == 0 then
    return 0
end

-- Move jobs from delayed to waiting
for i, job_id in ipairs(jobs) do
    -- Remove from delayed sorted set
    redis.call('zrem', delayed_key, job_id)
    
    -- Add to waiting list (left push for FIFO)
    redis.call('lpush', wait_key, job_id)
    
    -- Update job state
    local job_key = "rbq:job:" .. job_id
    redis.call('hset', job_key, 'state', '"Waiting"')
end

-- Optionally publish event for UI updates
redis.call('publish', 'rbq:events:' .. queue_name, 
    string.format('{"type":"delayed_moved","count":%d,"timestamp":%d}', #jobs, now))

return #jobs
