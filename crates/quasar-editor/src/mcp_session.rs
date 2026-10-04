//! Authenticated local bridge from the open Editor document session to the MCP adapter.

use std::{
    fs::{self, OpenOptions},
    io::{BufRead, BufReader, Read, Write},
    net::{TcpListener, TcpStream},
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread::{self, JoinHandle},
    time::Duration,
};

use bevy::prelude::Resource;
use quasar_project::{
    assets::{AssetCatalog, AssetKind, AssetStatus},
    commands::SceneCommand,
    document::SceneId,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use uuid::Uuid;

use crate::document_session::EditorDocumentSession;
use crate::import::jobs::AssetJobService;

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

pub(crate) fn start_mcp_session(
    session: EditorDocumentSession,
    jobs: AssetJobService,
) -> Result<EditorMcpSession, String> {
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
    let document = session.clone();
    let asset_jobs = jobs.clone();
    let worker_token = descriptor.token.clone();
    let worker = thread::Builder::new()
        .name("quasar-editor-mcp-session".to_owned())
        .spawn(move || {
            while !worker_shutdown.load(Ordering::Relaxed) {
                match listener.accept() {
                    Ok((stream, _)) => {
                        serve_internal_request(stream, &worker_token, &document, &asset_jobs)
                    }
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
        "Quasar Editor MCP session is available for '{}' (pid {})",
        session
            .snapshot()
            .map(|snapshot| snapshot.path.display().to_string())
            .unwrap_or_else(|_| "<unavailable>".to_owned()),
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
    session: &EditorDocumentSession,
    jobs: &AssetJobService,
) {
    // TcpStream instances accepted from a nonblocking listener may themselves
    // stay nonblocking on Windows. The MCP client completes connect() before
    // sending the request, so a read in that state can fail with WSAEWOULDBLOCK
    // (10035) instead of waiting for the request bytes.
    let response = match stream.set_nonblocking(false) {
        Err(error) => Err(format!("cannot configure local MCP stream: {error}")),
        Ok(()) => {
            let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
            let _ = stream.set_write_timeout(Some(Duration::from_secs(2)));
            let mut request_bytes = Vec::new();
            let read_result = BufReader::new(&mut stream)
                .take(MAX_REQUEST_BYTES)
                .read_until(b'\n', &mut request_bytes);
            match read_result {
                Ok(_) if request_bytes.len() as u64 >= MAX_REQUEST_BYTES => {
                    Err("local MCP request exceeds 1 MiB".to_owned())
                }
                Ok(_) => serde_json::from_slice::<Value>(&request_bytes)
                    .map_err(|error| format!("invalid local MCP request: {error}"))
                    .and_then(|request| {
                        handle_internal_request(request, expected_token, session, jobs)
                    }),
                Err(error) => Err(format!("cannot read local MCP request: {error}")),
            }
        }
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
    session: &EditorDocumentSession,
    jobs: &AssetJobService,
) -> Result<Value, String> {
    if request.get("token").and_then(Value::as_str) != Some(expected_token) {
        return Err("local MCP session authentication failed".to_owned());
    }
    let method = request
        .get("method")
        .and_then(Value::as_str)
        .ok_or_else(|| "local MCP method is missing".to_owned())?;
    let arguments = request
        .get("arguments")
        .cloned()
        .unwrap_or_else(|| json!({}));
    if matches!(method, "preview_scene_command" | "apply_scene_command") {
        let expected_revision = required_revision(&arguments)?;
        let command: SceneCommand = serde_json::from_value(
            arguments
                .get("command")
                .cloned()
                .ok_or_else(|| "command is required".to_owned())?,
        )
        .map_err(|error| format!("invalid scene command: {error}"))?;
        validate_command_asset_reference(&command, session)?;
        if method == "preview_scene_command" {
            let preview = session.preview_command(command, expected_revision)?;
            return Ok(json!({
                "revision": preview.revision,
                "label": preview.label,
                "affected_objects": preview.affected_objects,
                "changed": preview.changed,
                "document": preview.document
            }));
        }
        let receipt = session.apply_command(command, expected_revision)?;
        return Ok(json!({
            "revision": receipt.revision,
            "label": receipt.label,
            "affected_objects": receipt.affected_objects,
            "changed": receipt.changed
        }));
    }
    if matches!(method, "undo" | "redo") {
        let expected_revision = required_revision(&arguments)?;
        let receipt = if method == "undo" {
            session.undo(expected_revision)?
        } else {
            session.redo(expected_revision)?
        };
        return Ok(
            json!({ "revision": receipt.revision, "label": receipt.label, "affected_objects": receipt.affected_objects, "changed": receipt.changed }),
        );
    }
    if method == "save_project" {
        let saved = session.save(required_revision(&arguments)?)?;
        return Ok(json!({ "revision": saved.revision, "dirty": saved.dirty, "path": saved.path }));
    }
    let snapshot = session.snapshot()?;
    let document = &snapshot.document;
    let project_root = snapshot
        .path
        .parent()
        .ok_or_else(|| "open project document has no project root".to_owned())?;
    match method {
        "list_assets" => {
            let catalog = AssetCatalog::scan(project_root);
            return Ok(json!({
                "assets": catalog.assets.iter().map(|asset| json!({
                    "asset_id": asset.metadata.asset_id,
                    "kind": asset.metadata.kind,
                    "source_path": asset.metadata.source_path,
                    "importer_id": asset.metadata.importer_id,
                    "importer_version": asset.metadata.importer_version,
                    "source_url": asset.metadata.source_url,
                    "author": asset.metadata.author,
                    "license": asset.metadata.license,
                    "status": match asset.status {
                        AssetStatus::Ready => "ready",
                        AssetStatus::Missing => "missing",
                        AssetStatus::Conflict => "conflict",
                    }
                })).collect::<Vec<_>>(),
                "diagnostics": catalog.diagnostics.iter().map(|diagnostic| json!({
                    "path": diagnostic.path,
                    "message": diagnostic.message,
                })).collect::<Vec<_>>(),
            }));
        }
        "start_asset_import" => {
            let source_path = arguments
                .get("source_path")
                .and_then(Value::as_str)
                .ok_or_else(|| "start_asset_import requires source_path".to_owned())?;
            let job_id =
                jobs.start_local_import(project_root.to_path_buf(), PathBuf::from(source_path))?;
            return Ok(json!({ "job_id": job_id, "status": "queued" }));
        }
        "start_asset_import_url" => {
            let url = arguments
                .get("url")
                .and_then(Value::as_str)
                .ok_or_else(|| "start_asset_import_url requires url".to_owned())?;
            let job_id = jobs.start_url_import(
                project_root.to_path_buf(),
                url.to_owned(),
                arguments
                    .get("author")
                    .and_then(Value::as_str)
                    .map(str::to_owned),
                arguments
                    .get("license")
                    .and_then(Value::as_str)
                    .map(str::to_owned),
            )?;
            return Ok(json!({ "job_id": job_id, "status": "queued" }));
        }
        "start_asset_reimport" => {
            let asset_id = required_asset_id(&arguments, "asset_id")?;
            let source_path = arguments
                .get("source_path")
                .and_then(Value::as_str)
                .map(PathBuf::from);
            let job_id = jobs.start_reimport(project_root.to_path_buf(), asset_id, source_path)?;
            return Ok(json!({ "job_id": job_id, "status": "queued" }));
        }
        "get_asset_job" => {
            let job_id = required_uuid(&arguments, "job_id")?;
            return Ok(json!(jobs.get(job_id)?));
        }
        "cancel_asset_job" => {
            let job_id = required_uuid(&arguments, "job_id")?;
            return Ok(json!(jobs.cancel(job_id)?));
        }
        "assign_model_asset" => {
            let command = SceneCommand::AssignModelAsset {
                scene_id: SceneId(required_uuid(&arguments, "scene_id")?),
                object_id: quasar_project::document::ObjectId(required_uuid(
                    &arguments,
                    "object_id",
                )?),
                asset_id: match arguments.get("asset_id") {
                    Some(Value::Null) | None => None,
                    Some(Value::String(_)) => Some(required_asset_id(&arguments, "asset_id")?),
                    _ => return Err("asset_id must be a UUID or null".to_owned()),
                },
            };
            validate_command_asset_reference(&command, session)?;
            let receipt = session.apply_command(command, required_revision(&arguments)?)?;
            return Ok(json!({
                "revision": receipt.revision,
                "label": receipt.label,
                "affected_objects": receipt.affected_objects,
                "changed": receipt.changed,
            }));
        }
        _ => {}
    }
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
            "revision": snapshot.revision,
            "dirty": snapshot.dirty
        })),
        "list_scenes" => Ok(json!({
            "project_id": document.project_id,
            "revision": snapshot.revision,
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
                "revision": snapshot.revision,
                "active": scene.id == active_scene.id,
                "scene": scene
            }))
        }
        other => Err(format!("unsupported local MCP method '{other}'")),
    }
}

fn required_uuid(arguments: &Value, key: &str) -> Result<uuid::Uuid, String> {
    arguments
        .get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| format!("{key} is required as a UUID"))?
        .parse::<uuid::Uuid>()
        .map_err(|error| format!("{key} is not a valid UUID: {error}"))
}

fn required_asset_id(
    arguments: &Value,
    key: &str,
) -> Result<quasar_project::assets::AssetId, String> {
    required_uuid(arguments, key).map(quasar_project::assets::AssetId)
}

fn validate_command_asset_reference(
    command: &SceneCommand,
    session: &EditorDocumentSession,
) -> Result<(), String> {
    let SceneCommand::AssignModelAsset {
        asset_id: Some(asset_id),
        ..
    } = command
    else {
        return Ok(());
    };
    let snapshot = session.snapshot()?;
    let project_root = snapshot
        .path
        .parent()
        .ok_or_else(|| "open project document has no project root".to_owned())?;
    let catalog = AssetCatalog::scan(project_root);
    let asset = catalog
        .assets
        .iter()
        .find(|asset| asset.metadata.asset_id == *asset_id)
        .ok_or_else(|| format!("model asset '{}' is not in the project catalog", asset_id.0))?;
    if asset.metadata.kind != AssetKind::Model || asset.status != AssetStatus::Ready {
        return Err(format!("asset '{}' is not a ready model asset", asset_id.0));
    }
    Ok(())
}

fn required_revision(arguments: &Value) -> Result<u64, String> {
    arguments
        .get("expected_revision")
        .and_then(Value::as_u64)
        .ok_or_else(|| "expected_revision must be a non-negative integer".to_owned())
}

#[cfg(test)]
mod asset_tests {
    use super::*;
    use quasar_project::{
        assets::ModelAssetComponent,
        document::{ProjectDocument, SceneDocument, SceneObjectDocument},
    };
    use std::{thread, time::Instant};

    struct TestProject(PathBuf);

    impl TestProject {
        fn new() -> Self {
            let root = std::env::temp_dir().join(format!("quasar-mcp-assets-{}", Uuid::new_v4()));
            fs::create_dir_all(&root).expect("temporary project directory can be created");
            Self(root)
        }

        fn document_path(&self) -> PathBuf {
            self.0.join("quasar.project.json")
        }
    }

    impl Drop for TestProject {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn local_request(
        request: Value,
        expected_token: &str,
        session: &EditorDocumentSession,
        jobs: &AssetJobService,
    ) -> Result<Value, String> {
        let listener = TcpListener::bind(("127.0.0.1", 0)).expect("test bridge binds loopback");
        let address = listener.local_addr().expect("test bridge has an address");
        let token = expected_token.to_owned();
        let session = session.clone();
        let jobs = jobs.clone();
        let worker = thread::spawn(move || {
            let (stream, _) = listener.accept().expect("test bridge accepts request");
            serve_internal_request(stream, &token, &session, &jobs);
        });
        let mut client = TcpStream::connect(address).expect("test client connects to bridge");
        let mut bytes = serde_json::to_vec(&request).expect("test request serializes");
        bytes.push(b'\n');
        client
            .write_all(&bytes)
            .expect("test request reaches local bridge");
        let mut response_bytes = Vec::new();
        std::io::BufReader::new(client)
            .take(MAX_REQUEST_BYTES)
            .read_until(b'\n', &mut response_bytes)
            .expect("test bridge returns its reply");
        worker.join().expect("test bridge worker completes");
        let response: Value =
            serde_json::from_slice(&response_bytes).expect("test bridge response is valid JSON");
        if response["ok"] == true {
            Ok(response["result"].clone())
        } else {
            Err(response["error"]
                .as_str()
                .unwrap_or("test bridge returned an unspecified error")
                .to_owned())
        }
    }

    #[test]
    fn nonblocking_listener_accepts_repeated_mcp_polls_on_blocking_streams() {
        let project = TestProject::new();
        let scene = SceneDocument::new("Main");
        ProjectDocument::new("MCP listener test", scene)
            .save(&project.document_path())
            .expect("test project document saves");
        let session = EditorDocumentSession::open(&project.document_path())
            .expect("Editor document session opens");
        let jobs = AssetJobService::default();
        let editor_endpoint =
            start_mcp_session(session, jobs).expect("Editor starts its nonblocking MCP listener");
        let descriptor: SessionDescriptor = serde_json::from_slice(
            &fs::read(&editor_endpoint.manifest_path).expect("session manifest is published"),
        )
        .expect("session manifest is valid");
        let address = descriptor
            .address
            .parse::<std::net::SocketAddr>()
            .expect("session address is valid");

        for _ in 0..20 {
            let mut stream = TcpStream::connect_timeout(&address, Duration::from_secs(2))
                .expect("separate MCP client connects to Editor");
            stream
                .set_read_timeout(Some(Duration::from_secs(2)))
                .expect("test client has a bounded read");
            serde_json::to_writer(
                &mut stream,
                &json!({ "token": descriptor.token, "method": "list_assets" }),
            )
            .expect("MCP request serializes");
            stream.write_all(b"\n").expect("MCP request reaches Editor");
            let mut response_line = String::new();
            BufReader::new(stream)
                .take(1024 * 1024)
                .read_line(&mut response_line)
                .expect("Editor returns an MCP response");
            let response: Value =
                serde_json::from_str(&response_line).expect("Editor MCP response is valid JSON");
            assert_eq!(response["ok"], true, "response: {response}");
            assert_eq!(response["result"]["assets"].as_array().unwrap().len(), 0);
        }
    }

    #[test]
    fn local_mcp_import_assign_undo_redo_and_save_share_one_project_session() {
        let project = TestProject::new();
        let mut scene = SceneDocument::new("Main");
        let scene_id = scene.id;
        let object = SceneObjectDocument::new("Prop", None);
        let object_id = object.id;
        scene.objects.push(object);
        ProjectDocument::new("Asset MCP test", scene)
            .save(&project.document_path())
            .expect("test project document saves");
        let session = EditorDocumentSession::open(&project.document_path())
            .expect("Editor document session opens");
        let jobs = AssetJobService::default();
        let token = "test-session-token";

        let listed = local_request(
            json!({"token": token, "method": "list_assets"}),
            token,
            &session,
            &jobs,
        )
        .expect("MCP can list the empty asset catalog");
        assert_eq!(listed["assets"].as_array().unwrap().len(), 0);

        let source = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/fixtures/stage0-player/assets/Quasar/viewport-prop.glb");
        let started = local_request(
            json!({
                "token": token,
                "method": "start_asset_import",
                "arguments": {"source_path": source}
            }),
            token,
            &session,
            &jobs,
        )
        .expect("MCP starts a local import job");
        let job_id = started["job_id"].as_str().unwrap().parse::<Uuid>().unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        let job = loop {
            let value = local_request(
                json!({
                    "token": token,
                    "method": "get_asset_job",
                    "arguments": {"job_id": job_id}
                }),
                token,
                &session,
                &jobs,
            )
            .expect("MCP reads the import job");
            if value["status"] == "succeeded" || value["status"] == "failed" {
                break value;
            }
            assert!(Instant::now() < deadline, "asset import job timed out");
            thread::sleep(Duration::from_millis(10));
        };
        assert_eq!(job["status"], "succeeded", "job result: {job}");
        let asset_id = job["asset_id"].as_str().unwrap();

        let listed = local_request(
            json!({"token": token, "method": "list_assets"}),
            token,
            &session,
            &jobs,
        )
        .expect("MCP lists the imported model");
        assert_eq!(listed["assets"][0]["asset_id"], asset_id);
        assert_eq!(listed["assets"][0]["status"], "ready");
        let imported_path = project.0.join(
            listed["assets"][0]["source_path"]
                .as_str()
                .expect("asset source path is returned"),
        );
        let last_good_bytes =
            fs::read(&imported_path).expect("MCP-imported source exists in the project");
        let invalid_reimport = project.0.join("invalid-reimport.glb");
        fs::write(&invalid_reimport, b"not a GLB").unwrap();
        let reimport_started = local_request(
            json!({
                "token": token,
                "method": "start_asset_reimport",
                "arguments": { "asset_id": asset_id, "source_path": invalid_reimport }
            }),
            token,
            &session,
            &jobs,
        )
        .expect("MCP starts a reimport job");
        let reimport_job_id = reimport_started["job_id"]
            .as_str()
            .unwrap()
            .parse::<Uuid>()
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let reimport_job = local_request(
                json!({
                    "token": token,
                    "method": "get_asset_job",
                    "arguments": { "job_id": reimport_job_id }
                }),
                token,
                &session,
                &jobs,
            )
            .expect("MCP reads the reimport job");
            if reimport_job["status"] == "failed" {
                break;
            }
            assert_ne!(
                reimport_job["status"], "succeeded",
                "malformed reimport unexpectedly succeeded"
            );
            assert!(Instant::now() < deadline, "MCP reimport job timed out");
            thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(
            fs::read(&imported_path).unwrap(),
            last_good_bytes,
            "failed MCP reimport leaves the last good source in place"
        );

        let assigned = local_request(
            json!({
                "token": token,
                "method": "assign_model_asset",
                "arguments": {
                    "expected_revision": 0,
                    "scene_id": scene_id.0,
                    "object_id": object_id.0,
                    "asset_id": asset_id
                }
            }),
            token,
            &session,
            &jobs,
        )
        .expect("MCP assigns the ready model through a scene command");
        assert_eq!(assigned["revision"], 1);
        let snapshot = session.snapshot().unwrap();
        let model = ModelAssetComponent::from_components(
            &snapshot.document.scenes[0].objects[0].components,
        )
        .unwrap()
        .unwrap();
        assert_eq!(model.asset_id.0.to_string(), asset_id);

        local_request(
            json!({"token": token, "method": "undo", "arguments": {"expected_revision": 1}}),
            token,
            &session,
            &jobs,
        )
        .expect("MCP Undo clears the model assignment");
        assert!(
            ModelAssetComponent::from_components(
                &session.snapshot().unwrap().document.scenes[0].objects[0].components
            )
            .unwrap()
            .is_none()
        );
        local_request(
            json!({"token": token, "method": "redo", "arguments": {"expected_revision": 2}}),
            token,
            &session,
            &jobs,
        )
        .expect("MCP Redo restores the model assignment");
        local_request(
            json!({"token": token, "method": "save_project", "arguments": {"expected_revision": 3}}),
            token,
            &session,
            &jobs,
        )
        .expect("MCP saves the assigned model");
        let reopened = ProjectDocument::load(&project.document_path())
            .expect("saved project reopens with the model reference");
        assert!(
            ModelAssetComponent::from_components(&reopened.scenes[0].objects[0].components)
                .unwrap()
                .is_some()
        );

        assert!(
            local_request(
                json!({"token": "wrong", "method": "list_assets"}),
                token,
                &session,
                &jobs,
            )
            .unwrap_err()
            .contains("authentication failed")
        );
    }
}
