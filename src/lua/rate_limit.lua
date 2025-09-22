-- lua/rate_limit.lua
-- Token bucket rate limiter
-- KEYS[1] = rate_key
-- ARGV[1] = max_tokens
-- ARGV[2] = refill_rate (tokens per second)
-- ARGV[3] = current_timestamp
-- ARGV[4] = tokens_requested

local rate_key = KEYS[1]
local max_tokens = tonumber(ARGV[1])
local refill_rate = tonumber(ARGV[2])
local now = tonumber(ARGV[3])
local requested = tonumber(ARGV[4])

-- Get current bucket state
local bucket = redis.call('hmget', rate_key, 'tokens', 'last_refill')
local current_tokens = tonumber(bucket[1]) or max_tokens
local last_refill = tonumber(bucket[2]) or now

-- Calculate tokens to add based on time passed
local time_passed = now - last_refill
local new_tokens = math.min(max_tokens, current_tokens + (time_passed * refill_rate))

-- Check if we have enough tokens
if new_tokens >= requested then
    -- Consume tokens
    new_tokens = new_tokens - requested
    
    -- Update bucket
    redis.call('hmset', rate_key,
        'tokens', new_tokens,
        'last_refill', now
    )
    
    -- Set expiration (cleanup old buckets)
    redis.call('expire', rate_key, 3600)
    
    return 1 -- Success
else
    -- Update bucket without consuming
    redis.call('hmset', rate_key,
        'tokens', new_tokens,
        'last_refill', now
    )
    redis.call('expire', rate_key, 3600)
    
    return 0 -- Rate limited
end