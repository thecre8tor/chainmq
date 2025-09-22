-- lua/fail_job.lua
-- Handle job failure with retry logic
-- KEYS[1] = active_key
-- KEYS[2] = delayed_key (for retries)
-- KEYS[3] = failed_key (for final failures)
-- ARGV[1] = job_id
-- ARGV[2] = error_message
-- ARGV[3] = current_timestamp
-- ARGV[4] = retry_timestamp (if retrying)
-- ARGV[5] = is_final_failure (0 or 1)

local active_key = KEYS[1]
local delayed_key = KEYS[2]
local failed_key = KEYS[3]
local job_id = ARGV[1]
local error_msg = ARGV[2]
local now = ARGV[3]
local retry_at = ARGV[4]
local is_final = tonumber(ARGV[5])

-- Remove from active set
redis.call('srem', active_key, job_id)

local job_key = "rbq:job:" .. job_id

if is_final == 1 then
    -- Final failure - move to failed queue
    redis.call('lpush', failed_key, job_id)
    redis.call('hmset', job_key,
        'state', '"Failed"',
        'failed_at', now,
        'last_error', error_msg
    )
else
    -- Retry - schedule for later
    redis.call('zadd', delayed_key, retry_at, job_id)
    redis.call('hmset', job_key,
        'state', '"Delayed"',
        'last_error', error_msg,
        'failed_at', now
    )
end

return 1