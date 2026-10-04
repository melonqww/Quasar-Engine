//! stdio MCP adapter for the active Quasar Editor document session.

use std::{
    env,
    io::{self, BufRead, BufReader, BufWriter, Read, Write},
    net::{SocketAddr, TcpStream},
    path::PathBuf,
    time::Duration,
};

use serde::Deserialize;
use serde_json::{Value, json};

const PROTOCOL_VERSION: &str = "2025-11-25";
const MAX_LINE_BYTES: usize = 2 * 1024 * 1024;
const SERVER_NAME: &str = "quasar-engine";
const SERVER_VERSION: &str = env!("CARGO_PKG_VERSION");

#[derive(Debug, Deserialize)]
struct SessionDescriptor {
    address: String,
    token: String,
}

fn main() {
    if let Err(error) = serve_stdio() {
        eprintln!("quasar-mcp: {error}");
        std::process::exit(2);
    }
}

fn serve_stdio() -> Result<(), String> {
    let manifest_path = parse_session_argument(&env::args().skip(1).collect::<Vec<_>>())?;
    let stdin = io::stdin();
    let stdout = io::stdout();
    let mut reader = BufReader::new(stdin.lock());
    let mut writer = BufWriter::new(stdout.lock());
    let mut initialized = false;
    loop {
        let mut line = String::new();
        let bytes = reader
            .by_ref()
            .take((MAX_LINE_BYTES + 1) as u64)
            .read_line(&mut line)
            .map_err(|error| format!("cannot read MCP stdin: {error}"))?;
        if bytes == 0 {
            return Ok(());
        }
        if bytes > MAX_LINE_BYTES {
            return Err("MCP message exceeds 2 MiB".to_owned());
        }
        let request: Value = match serde_json::from_str(&line) {
            Ok(request) => request,
            Err(error) => {
                write_message(
                    &mut writer,
                    &json!({
                        "jsonrpc": "2.0",
                        "id": Value::Null,
                        "error": { "code": -32700, "message": format!("Parse error: {error}") }
                    }),
                )?;
                continue;
            }
        };
        let has_id = request.get("id").is_some();
        let method = request.get("method").and_then(Value::as_str).unwrap_or("");
        if !has_id && method.starts_with("notifications/") {
            continue;
        }
        let id = request.get("id").cloned().unwrap_or(Value::Null);
        let result = match method {
            "initialize" => {
                let requested = request
                    .pointer("/params/protocolVersion")
                    .and_then(Value::as_str)
                    .unwrap_or(PROTOCOL_VERSION);
                initialized = true;
                Ok(json!({
                    "protocolVersion": if supported_protocol_version(requested) { requested } else { PROTOCOL_VERSION },
                    "capabilities": { "tools": { "listChanged": false } },
                    "serverInfo": { "name": SERVER_NAME, "version": SERVER_VERSION },
                    "instructions": "Controlled access to the ProjectDocument and asset catalog currently open in Quasar Editor. Asset imports and reimports return job IDs; poll get_asset_job and cancel with cancel_asset_job."
                }))
            }
            "notifications/initialized" => continue,
            "ping" if initialized => Ok(json!({})),
            "tools/list" if initialized => Ok(tools_list()),
            "tools/call" if initialized => call_tool(&request, &manifest_path),
            "resources/list" if initialized => Ok(json!({ "resources": [] })),
            "prompts/list" if initialized => Ok(json!({ "prompts": [] })),
            _ if !initialized => Err((-32002, "Server not initialized".to_owned())),
            _ => Err((-32601, format!("Method not found: {method}"))),
        };
        if !has_id {
            continue;
        }
        let response = match result {
            Ok(value) => json!({ "jsonrpc": "2.0", "id": id, "result": value }),
            Err((code, message)) => json!({
                "jsonrpc": "2.0",
                "id": id,
                "error": { "code": code, "message": message }
            }),
        };
        write_message(&mut writer, &response)?;
    }
}

fn supported_protocol_version(requested: &str) -> bool {
    matches!(
        requested,
        "2024-11-05" | "2025-03-26" | "2025-06-18" | "2025-11-25"
    )
}

fn tools_list() -> Value {
    json!({
        "tools": [
            {
                "name": "get_project_state",
                "description": "Read the identity, active scene, scene count and session revision of the project open in Quasar Editor.",
                "inputSchema": { "type": "object", "properties": {}, "additionalProperties": false }
            },
            {
                "name": "list_scenes",
                "description": "List scenes in the project currently open in Quasar Editor.",
                "inputSchema": { "type": "object", "properties": {}, "additionalProperties": false }
            },
            {
                "name": "get_scene",
                "description": "Read a scene and its stable object IDs from the project currently open in Quasar Editor.",
                "inputSchema": {
                    "type": "object",
                    "properties": { "scene_id": { "type": "string", "format": "uuid" } },
                    "required": ["scene_id"],
                    "additionalProperties": false
                }
            },
            {
                "name": "preview_scene_command",
                "description": "Validate a scene command on a private candidate and return its result without changing the open project.",
                "inputSchema": {
                    "type": "object",
                    "properties": { "expected_revision": { "type": "integer", "minimum": 0 }, "command": { "type": "object", "description": "SceneCommand tagged object, such as {kind: set_transform, scene_id, object_id, transform}." } },
                    "required": ["expected_revision", "command"],
                    "additionalProperties": false
                }
            },
            {
                "name": "apply_scene_command",
                "description": "Apply one validated scene command to the open project. Use the current expected_revision; stale edits are rejected.",
                "inputSchema": {
                    "type": "object",
                    "properties": { "expected_revision": { "type": "integer", "minimum": 0 }, "command": { "type": "object", "description": "SceneCommand tagged object." } },
                    "required": ["expected_revision", "command"],
                    "additionalProperties": false
                }
            },
            {
                "name": "undo",
                "description": "Undo the most recent scene edit in the open Editor session.",
                "inputSchema": { "type": "object", "properties": { "expected_revision": { "type": "integer", "minimum": 0 } }, "required": ["expected_revision"], "additionalProperties": false }
            },
            {
                "name": "redo",
                "description": "Redo the most recently undone scene edit in the open Editor session.",
                "inputSchema": { "type": "object", "properties": { "expected_revision": { "type": "integer", "minimum": 0 } }, "required": ["expected_revision"], "additionalProperties": false }
            },
            {
                "name": "save_project",
                "description": "Save the open ProjectDocument after checking expected_revision.",
                "inputSchema": { "type": "object", "properties": { "expected_revision": { "type": "integer", "minimum": 0 } }, "required": ["expected_revision"], "additionalProperties": false }
            },
            {
                "name": "list_assets",
                "description": "List imported assets and catalog diagnostics for the project open in Quasar Editor.",
                "inputSchema": { "type": "object", "properties": {}, "additionalProperties": false }
            },
            {
                "name": "start_asset_import",
                "description": "Start a cancellable local asset import job for a GLB, PNG, JPEG or WAV file.",
                "inputSchema": { "type": "object", "properties": { "source_path": { "type": "string" } }, "required": ["source_path"], "additionalProperties": false }
            },
            {
                "name": "start_asset_import_url",
                "description": "Start a direct HTTP(S) asset download and import job. Private or local network hosts are rejected.",
                "inputSchema": { "type": "object", "properties": { "url": { "type": "string", "format": "uri" }, "author": { "type": "string" }, "license": { "type": "string" } }, "required": ["url"], "additionalProperties": false }
            },
            {
                "name": "start_asset_reimport",
                "description": "Revalidate and reimport an asset by stable AssetId while preserving the last successful file on failure.",
                "inputSchema": { "type": "object", "properties": { "asset_id": { "type": "string", "format": "uuid" }, "source_path": { "type": "string", "description": "Optional replacement source file. Required for assets without a stored source URL." } }, "required": ["asset_id"], "additionalProperties": false }
            },
            {
                "name": "get_asset_job",
                "description": "Read progress and final status for an asset import or reimport job.",
                "inputSchema": { "type": "object", "properties": { "job_id": { "type": "string", "format": "uuid" } }, "required": ["job_id"], "additionalProperties": false }
            },
            {
                "name": "cancel_asset_job",
                "description": "Cancel a queued or running asset operation.",
                "inputSchema": { "type": "object", "properties": { "job_id": { "type": "string", "format": "uuid" } }, "required": ["job_id"], "additionalProperties": false }
            },
            {
                "name": "assign_model_asset",
                "description": "Assign a ready model asset to an object or clear its model assignment. The change is undoable and requires the current revision.",
                "inputSchema": { "type": "object", "properties": { "expected_revision": { "type": "integer", "minimum": 0 }, "scene_id": { "type": "string", "format": "uuid" }, "object_id": { "type": "string", "format": "uuid" }, "asset_id": { "type": ["string", "null"], "format": "uuid" } }, "required": ["expected_revision", "scene_id", "object_id", "asset_id"], "additionalProperties": false }
            }
        ]
    })
}

fn call_tool(request: &Value, manifest_path: &PathBuf) -> Result<Value, (i64, String)> {
    let params = request
        .get("params")
        .ok_or((-32602, "Missing params".to_owned()))?;
    let name = params
        .get("name")
        .and_then(Value::as_str)
        .ok_or((-32602, "Missing tool name".to_owned()))?;
    let arguments = params
        .get("arguments")
        .cloned()
        .unwrap_or_else(|| json!({}));
    let result = match name {
        "get_project_state"
        | "list_scenes"
        | "preview_scene_command"
        | "apply_scene_command"
        | "undo"
        | "redo"
        | "save_project"
        | "list_assets"
        | "start_asset_import"
        | "start_asset_import_url"
        | "start_asset_reimport"
        | "get_asset_job"
        | "cancel_asset_job"
        | "assign_model_asset" => query_editor(manifest_path, name, &arguments),
        "get_scene" => {
            let scene_id = arguments
                .get("scene_id")
                .and_then(Value::as_str)
                .ok_or((-32602, "get_scene requires a UUID scene_id".to_owned()))?;
            if uuid::Uuid::parse_str(scene_id).is_err() {
                return Err((-32602, "scene_id is not a valid UUID".to_owned()));
            }
            query_editor(manifest_path, name, &arguments)
        }
        _ => return Err((-32602, format!("Unknown tool '{name}'"))),
    };
    match result {
        Ok(value) => Ok(json!({
            "content": [{ "type": "text", "text": serde_json::to_string_pretty(&value).unwrap_or_else(|_| "{}".to_owned()) }],
            "isError": false
        })),
        Err(error) => Ok(json!({
            "content": [{ "type": "text", "text": error }],
            "isError": true
        })),
    }
}

fn query_editor(manifest_path: &PathBuf, method: &str, arguments: &Value) -> Result<Value, String> {
    let descriptor: SessionDescriptor =
        serde_json::from_slice(&std::fs::read(manifest_path).map_err(|error| {
            format!(
                "no Quasar Editor session is available at {}: {error}",
                manifest_path.display()
            )
        })?)
        .map_err(|error| format!("invalid Quasar Editor session descriptor: {error}"))?;
    let address = descriptor
        .address
        .parse::<SocketAddr>()
        .map_err(|error| format!("invalid local Editor address: {error}"))?;
    if !address.ip().is_loopback() {
        return Err("Editor session address is not loopback".to_owned());
    }
    let mut stream = TcpStream::connect_timeout(&address, Duration::from_secs(2))
        .map_err(|error| format!("cannot reach the open Quasar Editor session: {error}"))?;
    stream
        .set_read_timeout(Some(Duration::from_secs(2)))
        .map_err(|error| format!("cannot configure local Editor connection: {error}"))?;
    stream
        .set_write_timeout(Some(Duration::from_secs(2)))
        .map_err(|error| format!("cannot configure local Editor connection: {error}"))?;
    let request = json!({
        "token": descriptor.token,
        "method": method,
        "arguments": arguments
    });
    serde_json::to_writer(&mut stream, &request)
        .map_err(|error| format!("cannot encode local Editor request: {error}"))?;
    stream
        .write_all(b"\n")
        .map_err(|error| format!("cannot send local Editor request: {error}"))?;
    let mut response_line = String::new();
    BufReader::new(stream)
        .take((MAX_LINE_BYTES + 1) as u64)
        .read_line(&mut response_line)
        .map_err(|error| format!("cannot read local Editor response: {error}"))?;
    if response_line.len() > MAX_LINE_BYTES {
        return Err("local Editor response exceeds 2 MiB".to_owned());
    }
    let envelope: Value = serde_json::from_str(&response_line)
        .map_err(|error| format!("invalid local Editor response: {error}"))?;
    if envelope.get("ok").and_then(Value::as_bool) != Some(true) {
        return Err(envelope
            .get("error")
            .and_then(Value::as_str)
            .unwrap_or("open Editor query failed")
            .to_owned());
    }
    envelope
        .get("result")
        .cloned()
        .ok_or_else(|| "open Editor response is missing result".to_owned())
}

fn parse_session_argument(args: &[String]) -> Result<PathBuf, String> {
    let mut path = None;
    let mut index = 0;
    while index < args.len() {
        if args[index] == "--session-file" {
            if path.is_some() {
                return Err("--session-file may be supplied only once".to_owned());
            }
            index += 1;
            path =
                Some(PathBuf::from(args.get(index).ok_or_else(|| {
                    "--session-file requires a path".to_owned()
                })?));
        } else {
            return Err(format!("unknown argument: {}", args[index]));
        }
        index += 1;
    }
    Ok(path.unwrap_or_else(|| {
        env::temp_dir()
            .join("QuasarEngine")
            .join("editor-session.json")
    }))
}

fn write_message(writer: &mut impl Write, message: &Value) -> Result<(), String> {
    serde_json::to_writer(&mut *writer, message)
        .map_err(|error| format!("cannot encode MCP response: {error}"))?;
    writer
        .write_all(b"\n")
        .and_then(|()| writer.flush())
        .map_err(|error| format!("cannot write MCP stdout: {error}"))
}
