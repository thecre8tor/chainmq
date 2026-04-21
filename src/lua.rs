// src/lua.rs - Lua scripts for atomic operations
use crate::Result;
use redis::{Client as RedisClient, Script};

pub struct LuaScripts {
    pub move_delayed: Script,
    pub claim_job: Script,
}

impl LuaScripts {
    pub async fn new(_client: &RedisClient) -> Result<Self> {
        let move_delayed = Script::new(include_str!("./lua/move_delayed.lua"));
        let claim_job = Script::new(include_str!("./lua/claim_job.lua"));

        move_delayed.prepare_invoke();
        claim_job.prepare_invoke();

        Ok(Self {
            move_delayed,
            claim_job,
        })
    }
}
