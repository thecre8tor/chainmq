-- lua/claim_job.lua
-- Atomically claim a job from waiting queues (priority + FIFO/LIFO buckets, then legacy wait) and move to active.
-- KEYS[1..n-1] = wait lists in claim order (RPOP from each), KEYS[n] = active_key

local n = #KEYS
local active_key = KEYS[n]

for i = 1, n - 1 do
    local job_id = redis.call('rpop', KEYS[i])
    if job_id then
        redis.call('sadd', active_key, job_id)
        return job_id
    end
end

return nil
