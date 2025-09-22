-- lua/complete_job.lua
-- Complete a job successfully
-- KEYS[1] = active_key
-- ARGV[1] = job_id
-- ARGV[2] = completion_timestamp

local active_key = KEYS[1]
local job_id = ARGV[1]
local completed_at = ARGV[2]

-- Remove from active set
redis.call('srem', active_key, job_id)

-- Update job metadata
local job_key = "rbq:job:" .. job_id
redis.call('hmset', job_key,
    'state', '"Completed"',
    'completed_at', completed_at
)

return 1