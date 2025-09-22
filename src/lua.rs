// src/lua.rs - Lua scripts for atomic operations
use crate::Result;
use redis::{Client as RedisClient, Script};

pub struct LuaScripts {
    pub move_delayed: Script,
    pub claim_job: Script,
    pub rate_limit: Script,
}

impl LuaScripts {
    pub async fn new(_client: &RedisClient) -> Result<Self> {
        let move_delayed = Script::new(include_str!("./lua/move_delayed.lua"));
        let claim_job = Script::new(include_str!("./lua/claim_job.lua"));
        let rate_limit = Script::new(include_str!("./lua/rate_limit.lua"));

        // Prepare invocations (optional hinting; no preloading required)
        // move_delayed.load(&mut client.get_connection()?)?;

        move_delayed.prepare_invoke();
        claim_job.prepare_invoke();
        rate_limit.prepare_invoke();

        Ok(Self {
            move_delayed,
            claim_job,
            rate_limit,
        })
    }
}
