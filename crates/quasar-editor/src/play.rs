//! Owns an immutable Player snapshot and the child process launched for Editor Play.

use std::{
    collections::{HashMap, HashSet},
    ffi::OsString,
    fs,
    path::{Path, PathBuf},
    process::{Child, Command, ExitStatus, Stdio},
    sync::{Arc, Mutex},
};

use bevy::prelude::Resource;
use quasar_project::{
    assets::{
        AssetCatalog, AssetId, AssetKind, AssetRecord, AssetStatus, ModelAssetComponent,
        ScriptComponent, sidecar_path,
    },
    document::{ProjectDocument, SceneDocument},
    gameplay::{AudioListenerComponent, AudioSourceComponent, TypedComponent},
};
use serde::Serialize;
use uuid::Uuid;

use crate::document_session::{DocumentSessionSnapshot, EditorDocumentSession};

#[derive(Clone, Debug, Serialize)]
pub(crate) struct PlayStatus {
    pub state: &'static str,
    pub run_id: Option<Uuid>,
    pub player_pid: Option<u32>,
    pub diagnostic: Option<String>,
}

impl Default for PlayStatus {
    fn default() -> Self {
        Self {
            state: "Stopped",
            run_id: None,
            player_pid: None,
            diagnostic: None,
        }
    }
}

#[derive(Clone, Resource, Default)]
pub(crate) struct EditorPlayService {
    inner: Arc<Mutex<PlayServiceState>>,
}

#[derive(Default)]
struct PlayServiceState {
    status: PlayStatus,
    active: Option<ActivePlay>,
}

struct ActivePlay {
    run_id: Uuid,
    child: Child,
    snapshot_dir: PathBuf,
    log_path: PathBuf,
}

impl EditorPlayService {
    pub(crate) fn status(&self, session: &EditorDocumentSession) -> PlayStatus {
        self.poll(session);
        self.inner
            .lock()
            .map(|inner| inner.status.clone())
            .unwrap_or_else(|_| PlayStatus {
                state: "Failed",
                diagnostic: Some("Play service lock is unavailable".into()),
                ..PlayStatus::default()
            })
    }

    pub(crate) fn start(&self, session: &EditorDocumentSession) -> Result<PlayStatus, String> {
        self.start_with_command(session, None)
    }

    fn start_with_command(
        &self,
        session: &EditorDocumentSession,
        override_command: Option<(PathBuf, Vec<OsString>)>,
    ) -> Result<PlayStatus, String> {
        let mut inner = self
            .inner
            .lock()
            .map_err(|_| "Play service is unavailable".to_owned())?;
        if inner.active.is_some() {
            return Err("Play is already running for this Editor session".into());
        }
        let run_id = Uuid::new_v4();
        session.begin_play(run_id)?;
        let result: Result<(PathBuf, PathBuf, Child), String> = (|| {
            let snapshot = session.snapshot()?;
            let snapshot_dir = prepare_snapshot(&snapshot, run_id)?;
            let (player, prefix_args) = match override_command {
                Some((path, args)) => (path, args),
                None => (
                    player_executable().inspect_err(|_| {
                        let _ = fs::remove_dir_all(&snapshot_dir);
                    })?,
                    Vec::new(),
                ),
            };
            let log_path = snapshot_dir.join("player.log");
            let log_file = fs::File::create(&log_path).map_err(|error| {
                let _ = fs::remove_dir_all(&snapshot_dir);
                format!("cannot create Player log: {error}")
            })?;
            let stderr = log_file.try_clone().map_err(|error| {
                let _ = fs::remove_dir_all(&snapshot_dir);
                format!("cannot duplicate Player log handle: {error}")
            })?;
            let child = Command::new(&player)
                .args(prefix_args)
                .arg("--project-document")
                .arg(snapshot_dir.join("project.json"))
                .arg("--asset-root")
                .arg(&snapshot_dir)
                .current_dir(&snapshot_dir)
                .stdin(Stdio::null())
                .stdout(Stdio::from(log_file))
                .stderr(Stdio::from(stderr))
                .spawn()
                .map_err(|error| {
                    let _ = fs::remove_dir_all(&snapshot_dir);
                    format!("cannot start Player {}: {error}", player.display())
                })?;
            Ok((snapshot_dir, log_path, child))
        })();
        match result {
            Ok((snapshot_dir, log_path, child)) => {
                inner.status = PlayStatus {
                    state: "Running",
                    run_id: Some(run_id),
                    player_pid: Some(child.id()),
                    diagnostic: None,
                };
                inner.active = Some(ActivePlay {
                    run_id,
                    child,
                    snapshot_dir,
                    log_path,
                });
                Ok(inner.status.clone())
            }
            Err(error) => {
                session.end_play(run_id);
                inner.status = PlayStatus {
                    state: "Failed",
                    run_id: Some(run_id),
                    diagnostic: Some(error.clone()),
                    ..PlayStatus::default()
                };
                Err(error)
            }
        }
    }

    #[cfg(test)]
    fn start_with_test_command(
        &self,
        session: &EditorDocumentSession,
        path: PathBuf,
        args: Vec<OsString>,
    ) -> Result<PlayStatus, String> {
        self.start_with_command(session, Some((path, args)))
    }

    pub(crate) fn stop(&self, session: &EditorDocumentSession) -> Result<PlayStatus, String> {
        let mut inner = self
            .inner
            .lock()
            .map_err(|_| "Play service is unavailable".to_owned())?;
        if inner.active.is_none() {
            return Ok(inner.status.clone());
        }
        inner.status.state = "Stopping";
        let Some(active) = inner.active.as_mut() else {
            unreachable!()
        };
        if let Err(error) = active.child.kill()
            && active.child.try_wait().ok().flatten().is_none()
        {
            inner.status.state = "Failed";
            inner.status.diagnostic = Some(format!("failed to stop Player: {error}"));
            return Err(error.to_string());
        }
        let Some(mut active) = inner.active.take() else {
            unreachable!()
        };
        let _ = active.child.wait();
        session.end_play(active.run_id);
        let _ = fs::remove_dir_all(&active.snapshot_dir);
        inner.status = PlayStatus {
            state: "Stopped",
            run_id: Some(active.run_id),
            player_pid: None,
            diagnostic: None,
        };
        Ok(inner.status.clone())
    }

    fn poll(&self, session: &EditorDocumentSession) {
        let Ok(mut inner) = self.inner.lock() else {
            return;
        };
        let Some(active) = inner.active.as_mut() else {
            return;
        };
        let exit = match active.child.try_wait() {
            Ok(status) => status,
            Err(error) => {
                inner.status.state = "Failed";
                inner.status.diagnostic = Some(format!("cannot inspect Player process: {error}"));
                return;
            }
        };
        let Some(exit) = exit else {
            return;
        };
        let active = inner.active.take().expect("active run checked above");
        session.end_play(active.run_id);
        let log = fs::read_to_string(&active.log_path).unwrap_or_default();
        let diagnostic = exit_diagnostic(exit, &log);
        let _ = fs::remove_dir_all(&active.snapshot_dir);
        inner.status = PlayStatus {
            state: if exit.success() { "Stopped" } else { "Failed" },
            run_id: Some(active.run_id),
            player_pid: None,
            diagnostic,
        };
    }
}

impl Drop for EditorPlayService {
    fn drop(&mut self) {
        if Arc::strong_count(&self.inner) != 1 {
            return;
        }
        let Ok(mut inner) = self.inner.lock() else {
            return;
        };
        if let Some(mut active) = inner.active.take() {
            let _ = active.child.kill();
            let _ = active.child.wait();
            let _ = fs::remove_dir_all(active.snapshot_dir);
        }
    }
}

fn exit_diagnostic(exit: ExitStatus, log: &str) -> Option<String> {
    if exit.success() {
        return None;
    }
    let tail = log.lines().rev().take(12).collect::<Vec<_>>();
    let tail = tail.into_iter().rev().collect::<Vec<_>>().join("\n");
    Some(if tail.is_empty() {
        format!("Player exited unexpectedly with {exit}")
    } else {
        format!("Player exited unexpectedly with {exit}:\n{tail}")
    })
}

fn player_executable() -> Result<PathBuf, String> {
    if let Some(path) = std::env::var_os("QUASAR_PLAYER_BIN") {
        let path = PathBuf::from(path);
        if path.is_file() {
            return Ok(path);
        }
        return Err(format!(
            "QUASAR_PLAYER_BIN does not exist: {}",
            path.display()
        ));
    }
    let executable = std::env::current_exe()
        .map_err(|error| format!("cannot locate Quasar Editor binary: {error}"))?;
    let file_name = if cfg!(windows) {
        "quasar-player.exe"
    } else {
        "quasar-player"
    };
    let sibling = executable
        .parent()
        .ok_or_else(|| "Quasar Editor binary has no parent directory".to_owned())?
        .join(file_name);
    if sibling.is_file() {
        Ok(sibling)
    } else {
        Err(format!(
            "cannot find Player beside Editor at {}; set QUASAR_PLAYER_BIN",
            sibling.display()
        ))
    }
}

fn prepare_snapshot(snapshot: &DocumentSessionSnapshot, run_id: Uuid) -> Result<PathBuf, String> {
    let project_root = snapshot
        .path
        .parent()
        .ok_or_else(|| "project document has no project root".to_owned())?;
    let snapshot_dir = std::env::temp_dir()
        .join("QuasarEngine")
        .join("Play")
        .join(run_id.to_string());
    fs::create_dir_all(&snapshot_dir).map_err(|error| {
        format!(
            "cannot create immutable Play snapshot {}: {error}",
            snapshot_dir.display()
        )
    })?;
    let result = (|| {
        let catalog = AssetCatalog::scan(project_root);
        let asset_ids = referenced_assets(&snapshot.document)?;
        let mut asset_ids = asset_ids;
        let has_enabled_script = snapshot
            .document
            .scenes
            .iter()
            .flat_map(|scene| &scene.objects)
            .filter_map(|object| {
                ScriptComponent::from_components(&object.components)
                    .ok()
                    .flatten()
            })
            .any(|script| script.enabled);
        if has_enabled_script {
            // Lua's audio.play(asset_id) can address any ready project Audio asset, including
            // sounds referenced only inside script source and not by an AudioSource component.
            asset_ids.extend(
                catalog
                    .assets
                    .iter()
                    .filter(|asset| {
                        asset.metadata.kind == AssetKind::Audio
                            && asset.status == AssetStatus::Ready
                    })
                    .map(|asset| asset.metadata.asset_id),
            );
        }
        validate_play_inputs(&snapshot.document, project_root, &catalog, &asset_ids)?;
        for asset_id in asset_ids {
            let asset = catalog
                .assets
                .iter()
                .find(|asset| {
                    asset.metadata.asset_id == asset_id && asset.status == AssetStatus::Ready
                })
                .ok_or_else(|| {
                    format!(
                        "Play snapshot references unavailable asset '{}'",
                        asset_id.0
                    )
                })?;
            let source = fs::canonicalize(&asset.source_file).map_err(|error| {
                format!(
                    "cannot resolve asset '{}': {error}",
                    asset.metadata.source_path
                )
            })?;
            let canonical_root = fs::canonicalize(project_root)
                .map_err(|error| format!("cannot resolve project root: {error}"))?;
            if !source.starts_with(canonical_root) {
                return Err(format!(
                    "asset '{}' resolves outside the project root",
                    asset.metadata.source_path
                ));
            }
            copy_asset(asset, &snapshot_dir)?;
        }
        snapshot.document.save(&snapshot_dir.join("project.json"))?;
        Ok(())
    })();
    if let Err(error) = result {
        let _ = fs::remove_dir_all(&snapshot_dir);
        return Err(error);
    }
    Ok(snapshot_dir)
}

fn referenced_assets(document: &ProjectDocument) -> Result<HashSet<AssetId>, String> {
    let mut ids = HashSet::new();
    for object in document.scenes.iter().flat_map(|scene| &scene.objects) {
        if let Some(model) = ModelAssetComponent::from_components(&object.components)? {
            ids.insert(model.asset_id);
        }
        if let Some(script) = ScriptComponent::from_components(&object.components)? {
            ids.insert(script.asset_id);
        }
        if let Some(audio) = AudioSourceComponent::from_components(&object.components)? {
            ids.insert(audio.asset_id);
        }
    }
    Ok(ids)
}

fn expected_asset_kinds(document: &ProjectDocument) -> Result<HashMap<AssetId, AssetKind>, String> {
    let mut kinds = HashMap::new();
    for object in document.scenes.iter().flat_map(|scene| &scene.objects) {
        if let Some(model) = ModelAssetComponent::from_components(&object.components)? {
            kinds.insert(model.asset_id, AssetKind::Model);
        }
        if let Some(script) = ScriptComponent::from_components(&object.components)? {
            kinds.insert(script.asset_id, AssetKind::Script);
        }
        if let Some(audio) = AudioSourceComponent::from_components(&object.components)? {
            kinds.insert(audio.asset_id, AssetKind::Audio);
        }
    }
    Ok(kinds)
}

fn validate_play_inputs(
    document: &ProjectDocument,
    project_root: &Path,
    catalog: &AssetCatalog,
    asset_ids: &HashSet<AssetId>,
) -> Result<(), String> {
    let scene = document
        .scenes
        .iter()
        .find(|scene| scene.id == document.active_scene_id)
        .ok_or_else(|| "active scene is missing".to_owned())?;
    validate_scene_audio(scene, document)?;
    let expected_kinds = expected_asset_kinds(document)?;
    for asset_id in asset_ids {
        let asset = catalog
            .assets
            .iter()
            .find(|asset| asset.metadata.asset_id == *asset_id)
            .ok_or_else(|| {
                format!(
                    "referenced asset '{}' is missing from the catalog",
                    asset_id.0
                )
            })?;
        if asset.status != AssetStatus::Ready {
            return Err(format!(
                "asset '{}' has status {:?}",
                asset_id.0, asset.status
            ));
        }
        let expected_kind = expected_kinds.get(asset_id);
        let script_audio_dependency = expected_kind.is_none()
            && asset.metadata.kind == AssetKind::Audio
            && asset.status == AssetStatus::Ready;
        if expected_kind.is_some_and(|kind| kind != &asset.metadata.kind)
            || (expected_kind.is_none() && !script_audio_dependency)
        {
            return Err(format!(
                "asset '{}' has kind {:?}, incompatible with its scene component",
                asset_id.0, asset.metadata.kind
            ));
        }
        let source_path = fs::canonicalize(&asset.source_file).map_err(|error| {
            format!(
                "cannot resolve asset '{}': {error}",
                asset.metadata.source_path
            )
        })?;
        if !source_path.starts_with(project_root) {
            return Err(format!(
                "asset '{}' resolves outside the project root",
                asset.metadata.source_path
            ));
        }
        match asset.metadata.kind {
            AssetKind::Audio => {
                let bytes = fs::read(&asset.source_file).map_err(|error| {
                    format!(
                        "cannot read WAV asset '{}': {error}",
                        asset.metadata.source_path
                    )
                })?;
                quasar_project::assets::validate_wav(&bytes).map_err(|error| {
                    format!(
                        "invalid WAV asset '{}': {error}",
                        asset.metadata.source_path
                    )
                })?;
            }
            AssetKind::Script => {
                let source = fs::read_to_string(&asset.source_file).map_err(|error| {
                    format!(
                        "cannot read Lua asset '{}': {error}",
                        asset.metadata.source_path
                    )
                })?;
                quasar_runtime::gameplay::validate_project_script(
                    &asset.metadata.source_path,
                    &source,
                )?;
            }
            AssetKind::Model | AssetKind::Texture => {}
        }
    }
    Ok(())
}

fn validate_scene_audio(scene: &SceneDocument, document: &ProjectDocument) -> Result<(), String> {
    let mut listeners = 0usize;
    let mut spatial_sources = 0usize;
    let mut autoplay_sources = 0usize;
    for object in &scene.objects {
        listeners += usize::from(
            AudioListenerComponent::from_components(&object.components)?
                .is_some_and(|listener| listener.enabled),
        );
        if let Some(source) = AudioSourceComponent::from_components(&object.components)?
            && source.enabled
        {
            spatial_sources += usize::from(source.spatial);
            autoplay_sources += usize::from(source.autoplay);
        }
    }
    if listeners > 1 || (spatial_sources > 0 && listeners != 1) {
        return Err(format!(
            "scene '{}' requires exactly one enabled Audio Listener for spatial sound",
            scene.name
        ));
    }
    if autoplay_sources > 32 {
        return Err(format!(
            "scene '{}' exceeds the 32 autoplay voice budget",
            scene.name
        ));
    }
    document.audio.validate()
}

fn copy_asset(asset: &AssetRecord, root: &Path) -> Result<(), String> {
    let relative_source = Path::new(&asset.metadata.source_path);
    let destination = root.join(relative_source);
    let destination_root = destination
        .parent()
        .ok_or_else(|| "asset source has no parent directory".to_owned())?;
    fs::create_dir_all(destination_root)
        .map_err(|error| format!("cannot create Play asset directory: {error}"))?;
    fs::copy(&asset.source_file, &destination).map_err(|error| {
        format!(
            "cannot snapshot asset '{}': {error}",
            asset.metadata.source_path
        )
    })?;
    let metadata_destination = sidecar_path(&destination);
    let bytes = serde_json::to_vec_pretty(&asset.metadata)
        .map_err(|error| format!("cannot encode asset metadata: {error}"))?;
    fs::write(metadata_destination, bytes)
        .map_err(|error| format!("cannot snapshot asset metadata: {error}"))?;
    Ok(())
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;
    use crate::document_session::EditorDocumentSession;
    use quasar_project::commands::SceneCommand;

    #[test]
    fn play_snapshots_fixture_blocks_edits_and_stop_is_idempotent() {
        let fixture = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/fixtures/stage4-gameplay/quasar.project.json");
        let session = EditorDocumentSession::open(&fixture)
            .expect("Stage 4 acceptance fixture opens in the Editor session");
        let service = EditorPlayService::default();
        let powershell = PathBuf::from(std::env::var_os("SystemRoot").expect("Windows SystemRoot"))
            .join("System32/WindowsPowerShell/v1.0/powershell.exe");
        let started = service
            .start_with_test_command(
                &session,
                powershell,
                vec![
                    OsString::from("-NoProfile"),
                    OsString::from("-Command"),
                    OsString::from("Start-Sleep -Seconds 60"),
                ],
            )
            .expect("Play snapshots assets and starts a child process");
        assert_eq!(started.state, "Running");
        let run_id = started.run_id.expect("running session has a run id");
        let snapshot_dir = std::env::temp_dir()
            .join("QuasarEngine")
            .join("Play")
            .join(run_id.to_string());
        assert!(snapshot_dir.join("project.json").is_file());
        assert!(
            snapshot_dir
                .join("Assets/Audio/room-ambience.wav")
                .is_file()
        );
        assert!(snapshot_dir.join("Assets/Audio/door-latch.wav").is_file());
        assert!(snapshot_dir.join("Assets/Scripts/door.lua").is_file());
        let document = session.snapshot().expect("session snapshot is available");
        let scene = &document.document.scenes[0];
        let object = &scene.objects[0];
        assert!(
            session
                .apply_command(
                    SceneCommand::RenameObject {
                        scene_id: scene.id,
                        object_id: object.id,
                        name: "Blocked while playing".into(),
                    },
                    0,
                )
                .is_err(),
            "authoring changes stay blocked while Player is running"
        );

        let stopped = service
            .stop(&session)
            .expect("Play can stop the child process");
        assert_eq!(stopped.state, "Stopped");
        assert!(!snapshot_dir.exists(), "temporary snapshot is cleaned up");
        assert_eq!(
            service.stop(&session).expect("second Stop is safe").state,
            "Stopped"
        );
    }
}
