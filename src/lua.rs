// src/lua.rs - Lua scripts for atomic operations
use redis::Script;

pub struct LuaScripts {
    pub move_delayed: Script,
    pub claim_job: Script,
    pub promote_repeat: Script,
}

impl LuaScripts {
    pub fn new() -> Self {
        let move_delayed = Script::new(include_str!("./lua/move_delayed.lua"));
        let claim_job = Script::new(include_str!("./lua/claim_job.lua"));
        let promote_repeat = Script::new(include_str!("./lua/promote_repeat.lua"));

        move_delayed.prepare_invoke();
        claim_job.prepare_invoke();
        promote_repeat.prepare_invoke();

        Self {
            move_delayed,
            claim_job,
            promote_repeat,
        }
    }
}
