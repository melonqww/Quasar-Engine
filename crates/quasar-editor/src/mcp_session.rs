//! Read-only bridge from an open Editor document to the local MCP adapter.

use std::{
    fs::{self, OpenOptions},
    io::{BufRead, BufReader, Read, Write},
    net::{TcpListener, TcpStream},
    path::{Path, PathBuf},
    sync::{
        Arc, RwLock,
        atomic::{AtomicBool, Ordering},
    },
    thread::{self, JoinHandle},
    time::Duration,
};

use bevy::prelude::Resource;
use quasar_project::document::{ProjectDocument, SceneId};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use uuid::Uuid;

const SESSION_FILE: &str = "editor-session.json";
const MAX_REQUEST_BYTES: u64 = 1024 * 1024;

#[derive(Clone, Debug, Serialize, Deserialize)]
struct SessionDescriptor {
    address: String,
    token: String,
    editor_pid: u32,
}

#[derive(Resource)]
pub(crate) struct EditorMcpSession {
    _shutdown: Arc<AtomicBool>,
    _worker: Option<JoinHandle<()>>,
    manifest_path: PathBuf,
    token: String,
}

impl Drop for EditorMcpSession {
    fn drop(&mut self) {
        self._shutdown.store(true, Ordering::Relaxed);
        if let Some(worker) = self._worker.take() {
            let _ = worker.join();
        }
        if fs::read_to_string(&self.manifest_path)
            .ok()
            .and_then(|text| serde_json::from_str::<SessionDescriptor>(&text).ok())
            .is_some_and(|descriptor| descriptor.token == self.token)
        {
            let _ = fs::remove_file(&self.manifest_path);
        }
    }
}

pub(crate) struct OpenProjectSession {
    document: Arc<RwLock<ProjectDocument>>,
    path: PathBuf,
}

pub(crate) fn project_document_argument(args: &[String]) -> Result<Option<PathBuf>, String> {
    let mut path = None;
    let mut index = 0;
    while index < args.len() {
        if args[index] == "--project-document" {
            if path.is_some() {
                return Err("--project-document may be supplied only once".to_owned());
            }
            index += 1;
            path =
                Some(PathBuf::from(args.get(index).ok_or_else(|| {
                    "--project-document requires a path".to_owned()
                })?));
        }
        index += 1;
    }
    Ok(path)
}

pub(crate) fn open_project_session(path: &Path) -> Result<OpenProjectSession, String> {
    let path = fs::canonicalize(path).map_err(|error| {
        format!(
            "cannot resolve project document {}: {error}",
            path.display()
        )
    })?;
    let document = ProjectDocument::load(&path)?;
    Ok(OpenProjectSession {
        document: Arc::new(RwLock::new(document)),
        path,
    })
}

pub(crate) fn start_mcp_session(session: OpenProjectSession) -> Result<EditorMcpSession, String> {
    let directory = std::env::temp_dir().join("QuasarEngine");
    fs::create_dir_all(&directory).map_err(|error| {
        format!(
            "cannot create local MCP session directory {}: {error}",
            directory.display()
        )
    })?;
    let manifest_path = directory.join(SESSION_FILE);
    clear_stale_manifest(&manifest_path)?;

    let listener = TcpListener::bind(("127.0.0.1", 0))
        .map_err(|error| format!("cannot bind local MCP session: {error}"))?;
    listener
        .set_nonblocking(true)
        .map_err(|error| format!("cannot configure local MCP session: {error}"))?;
    let descriptor = SessionDescriptor {
        address: listener
            .local_addr()
            .map_err(|error| format!("cannot read local MCP address: {error}"))?
            .to_string(),
        token: Uuid::new_v4().to_string(),
        editor_pid: std::process::id(),
    };
    publish_manifest(&manifest_path, &descriptor)?;

    let shutdown = Arc::new(AtomicBool::new(false));
    let worker_shutdown = Arc::clone(&shutdown);
    let document = Arc::clone(&session.document);
    let worker_token = descriptor.token.clone();
    let worker = thread::Builder::new()
        .name("quasar-editor-mcp-session".to_owned())
        .spawn(move || {
            while !worker_shutdown.load(Ordering::Relaxed) {
                match listener.accept() {
                    Ok((stream, _)) => serve_internal_request(stream, &worker_token, &document),
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(20));
                    }
                    Err(error) => {
                        eprintln!("quasar-editor MCP session stopped: {error}");
                        break;
                    }
                }
            }
        })
        .map_err(|error| format!("cannot start local MCP worker: {error}"))?;

    eprintln!(
        "Quasar Editor read-only MCP session is available for '{}' (pid {})",
        session.path.display(),
        descriptor.editor_pid
    );
    Ok(EditorMcpSession {
        _shutdown: shutdown,
        _worker: Some(worker),
        manifest_path,
        token: descriptor.token,
    })
}

fn clear_stale_manifest(path: &Path) -> Result<(), String> {
    let Ok(text) = fs::read_to_string(path) else {
        return Ok(());
    };
    if let Ok(descriptor) = serde_json::from_str::<SessionDescriptor>(&text) {
        let address = descriptor
            .address
            .parse::<std::net::SocketAddr>()
            .map_err(|error| {
                format!(
                    "invalid existing MCP session address in {}: {error}",
                    path.display()
                )
            })?;
        if !address.ip().is_loopback() {
            return fs::remove_file(path).map_err(|error| {
                format!(
                    "cannot remove stale MCP session file {}: {error}",
                    path.display()
                )
            });
        }
        if TcpStream::connect_timeout(&address, Duration::from_millis(150)).is_ok() {
            return Err(format!(
                "another Quasar Editor session already owns the local MCP endpoint (pid {})",
                descriptor.editor_pid
            ));
        }
    }
    fs::remove_file(path).map_err(|error| {
        format!(
            "cannot remove stale MCP session file {}: {error}",
            path.display()
        )
    })
}

fn publish_manifest(path: &Path, descriptor: &SessionDescriptor) -> Result<(), String> {
    let bytes = serde_json::to_vec(descriptor)
        .map_err(|error| format!("cannot encode MCP session descriptor: {error}"))?;
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|error| format!("cannot publish MCP session {}: {error}", path.display()))?;
    if let Err(error) = file.write_all(&bytes).and_then(|()| file.sync_all()) {
        let _ = fs::remove_file(path);
        return Err(format!(
            "cannot write MCP session {}: {error}",
            path.display()
        ));
    }
    Ok(())
}

fn serve_internal_request(
    mut stream: TcpStream,
    expected_token: &str,
    document: &RwLock<ProjectDocument>,
) {
    let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
    let _ = stream.set_write_timeout(Some(Duration::from_secs(2)));
    let mut request_bytes = Vec::new();
    let read_result = BufReader::new(&mut stream)
        .take(MAX_REQUEST_BYTES)
        .read_until(b'\n', &mut request_bytes);
    let response = match read_result {
        Ok(_) if request_bytes.len() as u64 >= MAX_REQUEST_BYTES => {
            Err("local MCP request exceeds 1 MiB".to_owned())
        }
        Ok(_) => serde_json::from_slice::<Value>(&request_bytes)
            .map_err(|error| format!("invalid local MCP request: {error}"))
            .and_then(|request| handle_internal_request(request, expected_token, document)),
        Err(error) => Err(format!("cannot read local MCP request: {error}")),
    };
    let response = match response {
        Ok(value) => json!({ "ok": true, "result": value }),
        Err(error) => json!({ "ok": false, "error": error }),
    };
    let _ = serde_json::to_writer(&mut stream, &response);
    let _ = stream.write_all(b"\n");
}

fn handle_internal_request(
    request: Value,
    expected_token: &str,
    document: &RwLock<ProjectDocument>,
) -> Result<Value, String> {
    if request.get("token").and_then(Value::as_str) != Some(expected_token) {
        return Err("local MCP session authentication failed".to_owned());
    }
    let method = request
        .get("method")
        .and_then(Value::as_str)
        .ok_or_else(|| "local MCP method is missing".to_owned())?;
    let document = document
        .read()
        .map_err(|_| "open Editor project state is unavailable".to_owned())?;
    let active_scene = document
        .scenes
        .iter()
        .find(|scene| scene.id == document.active_scene_id)
        .ok_or_else(|| "active scene is missing from the open project".to_owned())?;
    match method {
        "get_project_state" => Ok(json!({
            "project_id": document.project_id,
            "name": document.name,
            "active_scene_id": document.active_scene_id,
            "scene_count": document.scenes.len(),
            "revision": 0
        })),
        "list_scenes" => Ok(json!({
            "project_id": document.project_id,
            "revision": 0,
            "scenes": document.scenes.iter().map(|scene| json!({
                "id": scene.id,
                "name": scene.name,
                "object_count": scene.objects.len(),
                "active": scene.id == document.active_scene_id
            })).collect::<Vec<_>>()
        })),
        "get_scene" => {
            let scene_id = request
                .get("arguments")
                .and_then(|arguments| arguments.get("scene_id"))
                .and_then(Value::as_str)
                .ok_or_else(|| "get_scene requires a UUID scene_id".to_owned())?
                .parse::<uuid::Uuid>()
                .map_err(|error| format!("scene_id is not a valid UUID: {error}"))?;
            let scene_id = SceneId(scene_id);
            let scene = document
                .scenes
                .iter()
                .find(|scene| scene.id == scene_id)
                .ok_or_else(|| format!("scene '{scene_id:?}' is not in the open project"))?;
            Ok(json!({
                "project_id": document.project_id,
                "revision": 0,
                "active": scene.id == active_scene.id,
                "scene": scene
            }))
        }
        other => Err(format!("unsupported local MCP method '{other}'")),
    }
}
