-- lua/claim_job.lua  
-- Atomically claim a job from waiting queue and move to active
-- KEYS[1] = wait_key
-- KEYS[2] = active_key  
-- ARGV[1] = worker_id
-- ARGV[2] = current_timestamp

local wait_key = KEYS[1]
local active_key = KEYS[2]
local worker_id = ARGV[1] 
local now = ARGV[2]

-- Pop job from waiting queue (right pop for FIFO)
local job_id = redis.call('rpop', wait_key)

if not job_id then
    return nil
end

-- Add to active set
redis.call('sadd', active_key, job_id)

-- Update job metadata
local job_key = "rbq:job:" .. job_id
redis.call('hmset', job_key,
    'state', '"Active"',
    'started_at', now,
    'worker_id', worker_id
)

return job_id