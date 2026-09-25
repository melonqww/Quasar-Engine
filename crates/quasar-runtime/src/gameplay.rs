//! Restricted Lua callback adapter used by the Stage 0 gameplay probe.

use std::sync::{
    Arc, Mutex,
    atomic::{AtomicU64, Ordering},
};

use mlua::{Function, HookTriggers, Lua, LuaOptions, StdLib, VmState};

const MEMORY_LIMIT_BYTES: usize = 8 * 1024 * 1024;
const INSTRUCTION_BUDGET: u64 = 100_000;
const HOOK_INTERVAL: u64 = 1_000;
const MAX_SCRIPT_BYTES: usize = 64 * 1024;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GameplayCommand {
    OpenDoor,
    PlayDoorSound,
}

/// Runs `on_interact()` with only the host APIs needed by the door probe.
/// Commands are returned only if the whole callback succeeds.
pub fn run_door_interaction(source: &str) -> Result<Vec<GameplayCommand>, String> {
    if source.len() > MAX_SCRIPT_BYTES {
        return Err(format!(
            "door.lua: script exceeds the {MAX_SCRIPT_BYTES}-byte source limit"
        ));
    }

    let lua = Lua::new_with(
        StdLib::TABLE | StdLib::STRING | StdLib::MATH | StdLib::UTF8,
        LuaOptions::default(),
    )
    .map_err(|error| format!("door.lua: {error}"))?;
    lua.set_memory_limit(MEMORY_LIMIT_BYTES)
        .map_err(|error| format!("door.lua: {error}"))?;
    restrict_base_globals(&lua).map_err(|error| format!("door.lua: {error}"))?;
    install_instruction_budget(&lua).map_err(|error| format!("door.lua: {error}"))?;

    let commands = Arc::new(Mutex::new(Vec::new()));
    install_host_api(&lua, Arc::clone(&commands)).map_err(|error| format!("door.lua: {error}"))?;

    let callback_result = (|| -> mlua::Result<()> {
        lua.load(source)
            .set_name("essentials/scripts/door.lua")
            .exec()?;
        let callback: Function = lua.globals().get("on_interact")?;
        callback.call(())
    })();

    if let Err(error) = callback_result {
        return Err(format!("door.lua: {error}"));
    }

    commands
        .lock()
        .map(|mut pending| std::mem::take(&mut *pending))
        .map_err(|_| "door.lua: host command queue is unavailable".to_owned())
}

fn restrict_base_globals(lua: &Lua) -> mlua::Result<()> {
    for name in [
        "collectgarbage",
        "dofile",
        "load",
        "loadfile",
        "pcall",
        "print",
        "require",
        "xpcall",
    ] {
        lua.globals().set(name, mlua::Nil)?;
    }
    Ok(())
}

fn install_host_api(lua: &Lua, commands: Arc<Mutex<Vec<GameplayCommand>>>) -> mlua::Result<()> {
    let door = lua.create_table()?;
    let open_commands = Arc::clone(&commands);
    door.set(
        "open",
        lua.create_function(move |_, ()| {
            let mut pending = open_commands
                .lock()
                .map_err(|_| mlua::Error::RuntimeError("host command queue unavailable".into()))?;
            pending.push(GameplayCommand::OpenDoor);
            Ok(())
        })?,
    )?;
    lua.globals().set("door", door)?;

    let audio = lua.create_table()?;
    audio.set(
        "play",
        lua.create_function(move |_, sound_id: String| {
            if sound_id != "door" {
                return Err(mlua::Error::RuntimeError(format!(
                    "unknown sound id: {sound_id}"
                )));
            }
            let mut pending = commands
                .lock()
                .map_err(|_| mlua::Error::RuntimeError("host command queue unavailable".into()))?;
            pending.push(GameplayCommand::PlayDoorSound);
            Ok(())
        })?,
    )?;
    lua.globals().set("audio", audio)?;
    Ok(())
}

fn install_instruction_budget(lua: &Lua) -> mlua::Result<()> {
    let executed = Arc::new(AtomicU64::new(0));
    lua.set_hook(
        HookTriggers::new().every_nth_instruction(HOOK_INTERVAL as u32),
        move |_, _| {
            let next = executed.fetch_add(HOOK_INTERVAL, Ordering::Relaxed) + HOOK_INTERVAL;
            if next >= INSTRUCTION_BUDGET {
                return Err(mlua::Error::RuntimeError(
                    "instruction budget exceeded".to_owned(),
                ));
            }
            Ok(VmState::Continue)
        },
    )
}

#[cfg(test)]
mod tests {
    use super::{GameplayCommand, MAX_SCRIPT_BYTES, run_door_interaction};

    #[test]
    fn door_callback_returns_open_and_sound_commands() {
        let commands =
            run_door_interaction("function on_interact() door.open(); audio.play('door') end")
                .expect("valid door callback succeeds");

        assert_eq!(
            commands,
            [GameplayCommand::OpenDoor, GameplayCommand::PlayDoorSound]
        );
    }

    #[test]
    fn callback_error_discards_commands_queued_before_the_error() {
        let error = run_door_interaction(
            "function on_interact() door.open(); error('broken callback') end",
        )
        .expect_err("broken callback should return an error");

        assert!(
            error.contains("broken callback"),
            "unexpected error: {error}"
        );
    }

    #[test]
    fn infinite_loop_is_stopped_by_instruction_budget() {
        let error = run_door_interaction("function on_interact() while true do end end")
            .expect_err("infinite callback should be interrupted");

        assert!(
            error.contains("instruction budget exceeded"),
            "unexpected error: {error}"
        );
    }

    #[test]
    fn oversized_script_is_rejected_before_compilation() {
        let oversized = " ".repeat(MAX_SCRIPT_BYTES + 1);
        let error = run_door_interaction(&oversized).expect_err("oversized source is rejected");

        assert!(error.contains("source limit"), "unexpected error: {error}");
    }

    #[test]
    fn excessive_lua_allocation_is_limited() {
        let error = run_door_interaction(
            "function on_interact() local payload = string.rep('x', 10000000) end",
        )
        .expect_err("VM allocation should stay below its memory limit");

        assert!(error.contains("memory"), "unexpected error: {error}");
    }

    #[test]
    fn filesystem_and_package_libraries_are_not_available() {
        let commands = run_door_interaction(
            "function on_interact() if io ~= nil or package ~= nil or os ~= nil or debug ~= nil or dofile ~= nil or loadfile ~= nil or pcall ~= nil then error('unsafe library loaded') end end",
        )
        .expect("filesystem, package, debug, and protected-call APIs should be absent");

        assert!(commands.is_empty());
    }
}
