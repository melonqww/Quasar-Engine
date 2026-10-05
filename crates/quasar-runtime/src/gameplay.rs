//! Restricted Lua callback adapter used by the Stage 0 gameplay probe.

use std::collections::HashSet;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicU64, Ordering},
};

use mlua::{Function, HookTriggers, Lua, LuaOptions, StdLib, VmState};
use quasar_project::{assets::AssetId, document::ObjectId};

const MEMORY_LIMIT_BYTES: usize = 8 * 1024 * 1024;
const INSTRUCTION_BUDGET: u64 = 100_000;
const HOOK_INTERVAL: u64 = 1_000;
const MAX_SCRIPT_BYTES: usize = 64 * 1024;
pub const PROJECT_SCRIPT_MAX_BYTES: usize = MAX_SCRIPT_BYTES;

/// Validates project Lua without executing its top-level chunk or callbacks.
/// Runtime execution remains responsible for the instruction budget and host API.
pub fn validate_project_script(source_name: &str, source: &str) -> Result<(), String> {
    if source.len() > PROJECT_SCRIPT_MAX_BYTES {
        return Err(format!(
            "{source_name}: script exceeds the {PROJECT_SCRIPT_MAX_BYTES}-byte source limit"
        ));
    }
    if source.contains('\0') {
        return Err(format!("{source_name}: source contains a NUL byte"));
    }
    let lua = Lua::new_with(
        StdLib::TABLE | StdLib::STRING | StdLib::MATH | StdLib::UTF8,
        LuaOptions::default(),
    )
    .map_err(|error| format!("{source_name}: {error}"))?;
    lua.set_memory_limit(MEMORY_LIMIT_BYTES)
        .map_err(|error| format!("{source_name}: {error}"))?;
    lua.load(source)
        .set_name(source_name)
        .into_function()
        .map_err(|error| format!("{source_name}: {error}"))?;
    Ok(())
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GameplayCommand {
    OpenDoor,
    PlayDoorSound,
}

/// Commands exposed to Lua scripts in persistent project scenes.
#[derive(Clone, Debug, PartialEq)]
pub enum ProjectGameplayCommand {
    RotateObjectY { object_id: ObjectId, radians: f32 },
    OpenDoor { object_id: ObjectId },
    PlayAudio { asset_id: AssetId },
}

/// Runs one project's `on_interact(object_id)` callback in the restricted Lua VM.
/// Host commands are returned only after the whole callback succeeds.
pub fn run_project_interaction(
    source_name: &str,
    source: &str,
    object_id: ObjectId,
    valid_objects: &HashSet<ObjectId>,
    valid_doors: &HashSet<ObjectId>,
    valid_audio_assets: &HashSet<AssetId>,
) -> Result<Vec<ProjectGameplayCommand>, String> {
    if source.len() > MAX_SCRIPT_BYTES {
        return Err(format!(
            "{source_name}: script exceeds the {MAX_SCRIPT_BYTES}-byte source limit"
        ));
    }
    if source.contains('\0') {
        return Err(format!("{source_name}: source contains a NUL byte"));
    }

    let lua = Lua::new_with(
        StdLib::TABLE | StdLib::STRING | StdLib::MATH | StdLib::UTF8,
        LuaOptions::default(),
    )
    .map_err(|error| format!("{source_name}: {error}"))?;
    lua.set_memory_limit(MEMORY_LIMIT_BYTES)
        .map_err(|error| format!("{source_name}: {error}"))?;
    restrict_base_globals(&lua).map_err(|error| format!("{source_name}: {error}"))?;
    install_instruction_budget(&lua).map_err(|error| format!("{source_name}: {error}"))?;

    let pending = Arc::new(Mutex::new(Vec::<ProjectGameplayCommand>::new()));
    let scene_api = lua
        .create_table()
        .map_err(|error| format!("{source_name}: {error}"))?;
    let rotate_commands = Arc::clone(&pending);
    let valid_object_ids = valid_objects.clone();
    let rotate_y = lua
        .create_function(move |_, (object, radians): (String, f64)| {
            let object_id = object
                .parse::<uuid::Uuid>()
                .map(ObjectId)
                .map_err(|error| {
                    mlua::Error::RuntimeError(format!("invalid object id: {error}"))
                })?;
            if !valid_object_ids.contains(&object_id) {
                return Err(mlua::Error::RuntimeError(
                    "object id is not part of the active scene".into(),
                ));
            }
            if !radians.is_finite() || radians.abs() > std::f64::consts::TAU * 16.0 {
                return Err(mlua::Error::RuntimeError(
                    "rotation must be finite and within +/- 16 turns".into(),
                ));
            }
            push_project_command(
                &rotate_commands,
                ProjectGameplayCommand::RotateObjectY {
                    object_id,
                    radians: radians as f32,
                },
            )
        })
        .map_err(|error| format!("{source_name}: {error}"))?;
    scene_api
        .set("rotate_y", rotate_y)
        .map_err(|error| format!("{source_name}: {error}"))?;
    lua.globals()
        .set("scene", scene_api)
        .map_err(|error| format!("{source_name}: {error}"))?;

    let door_api = lua
        .create_table()
        .map_err(|error| format!("{source_name}: {error}"))?;
    let door_commands = Arc::clone(&pending);
    let valid_door_ids = valid_doors.clone();
    let open_door = lua
        .create_function(move |_, ()| {
            if !valid_door_ids.contains(&object_id) {
                return Err(mlua::Error::RuntimeError(
                    "interacting object is not an enabled, collidable kinematic door".into(),
                ));
            }
            push_project_command(
                &door_commands,
                ProjectGameplayCommand::OpenDoor { object_id },
            )
        })
        .map_err(|error| format!("{source_name}: {error}"))?;
    door_api
        .set("open", open_door)
        .map_err(|error| format!("{source_name}: {error}"))?;
    lua.globals()
        .set("door", door_api)
        .map_err(|error| format!("{source_name}: {error}"))?;

    let audio_api = lua
        .create_table()
        .map_err(|error| format!("{source_name}: {error}"))?;
    let audio_commands = Arc::clone(&pending);
    let valid_audio_ids = valid_audio_assets.clone();
    let play_audio = lua
        .create_function(move |_, asset: String| {
            let asset_id = asset.parse::<uuid::Uuid>().map(AssetId).map_err(|error| {
                mlua::Error::RuntimeError(format!("invalid audio asset id: {error}"))
            })?;
            if !valid_audio_ids.contains(&asset_id) {
                return Err(mlua::Error::RuntimeError(
                    "audio asset id is not a ready Audio asset in the project".into(),
                ));
            }
            push_project_command(
                &audio_commands,
                ProjectGameplayCommand::PlayAudio { asset_id },
            )
        })
        .map_err(|error| format!("{source_name}: {error}"))?;
    audio_api
        .set("play", play_audio)
        .map_err(|error| format!("{source_name}: {error}"))?;
    lua.globals()
        .set("audio", audio_api)
        .map_err(|error| format!("{source_name}: {error}"))?;

    let result = (|| -> mlua::Result<()> {
        lua.load(source).set_name(source_name).exec()?;
        let callback: Function = lua.globals().get("on_interact")?;
        callback.call(object_id.0.to_string())
    })();
    if let Err(error) = result {
        return Err(format!("{source_name}: {error}"));
    }
    pending
        .lock()
        .map(|mut commands| std::mem::take(&mut *commands))
        .map_err(|_| format!("{source_name}: host command queue is unavailable"))
}

fn push_project_command(
    pending: &Mutex<Vec<ProjectGameplayCommand>>,
    command: ProjectGameplayCommand,
) -> mlua::Result<()> {
    let mut commands = pending
        .lock()
        .map_err(|_| mlua::Error::RuntimeError("host command queue unavailable".into()))?;
    if commands.len() >= 128 {
        return Err(mlua::Error::RuntimeError(
            "host command limit exceeded (128 per callback)".into(),
        ));
    }
    commands.push(command);
    Ok(())
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
    use std::collections::HashSet;

    use quasar_project::{assets::AssetId, document::ObjectId};

    use super::{
        GameplayCommand, MAX_SCRIPT_BYTES, ProjectGameplayCommand, run_door_interaction,
        run_project_interaction, validate_project_script,
    };

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

    #[test]
    fn project_script_validation_compiles_without_executing_top_level_code() {
        validate_project_script("door.lua", "error('must not run during import')")
            .expect("valid chunk compiles and is not executed");
        let syntax_error = validate_project_script("door.lua", "function on_interact(\n");
        assert!(syntax_error.unwrap_err().contains("door.lua"));
        let oversized = " ".repeat(MAX_SCRIPT_BYTES + 1);
        assert!(
            validate_project_script("large.lua", &oversized)
                .unwrap_err()
                .contains("source limit")
        );
        assert!(
            validate_project_script("nul.lua", "return '\0'")
                .unwrap_err()
                .contains("NUL")
        );
    }

    #[test]
    fn project_interaction_opens_only_the_interacting_enabled_door_and_plays_allowlisted_audio() {
        let door_id = ObjectId::new();
        let audio_id = AssetId::new();
        let script = format!(
            "function on_interact() door.open(); audio.play('{}') end",
            audio_id.0
        );
        let commands = run_project_interaction(
            "door.lua",
            &script,
            door_id,
            &HashSet::from([door_id]),
            &HashSet::from([door_id]),
            &HashSet::from([audio_id]),
        )
        .expect("valid door interaction succeeds");
        assert_eq!(
            commands,
            [
                ProjectGameplayCommand::OpenDoor { object_id: door_id },
                ProjectGameplayCommand::PlayAudio { asset_id: audio_id },
            ]
        );
    }

    #[test]
    fn project_interaction_rejects_non_door_and_rolls_back_queued_host_commands() {
        let object_id = ObjectId::new();
        let script = "function on_interact() door.open() end";
        let error = run_project_interaction(
            "prop.lua",
            script,
            object_id,
            &HashSet::from([object_id]),
            &HashSet::new(),
            &HashSet::new(),
        )
        .expect_err("non-door object cannot open as a door");
        assert!(
            error.contains("not an enabled"),
            "unexpected error: {error}"
        );

        let error = run_project_interaction(
            "door.lua",
            "function on_interact() door.open(); error('abort') end",
            object_id,
            &HashSet::from([object_id]),
            &HashSet::from([object_id]),
            &HashSet::new(),
        )
        .expect_err("failed callback must discard the queued open command");
        assert!(error.contains("abort"), "unexpected error: {error}");
    }
}
