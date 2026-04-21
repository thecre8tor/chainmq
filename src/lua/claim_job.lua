-- lua/claim_job.lua
-- Atomically claim a job from waiting queue and move to active.
-- Job metadata is updated in Rust (single JSON field) after this script returns.
-- KEYS[1] = wait_key
-- KEYS[2] = active_key

local wait_key = KEYS[1]
local active_key = KEYS[2]

local job_id = redis.call('rpop', wait_key)

if not job_id then
    return nil
end

redis.call('sadd', active_key, job_id)

return job_id
