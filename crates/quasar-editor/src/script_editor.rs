//! Revision-checked access to project Lua assets shared by the Editor and MCP.

use std::{fs, path::Path};

use quasar_project::assets::{AssetCatalog, AssetId, AssetKind, AssetStatus};
use serde::Serialize;
use uuid::Uuid;

use crate::document_session::EditorDocumentSession;

const MAX_SCRIPT_BYTES: usize = 64 * 1024;

#[derive(Clone, Debug, Serialize)]
pub(crate) struct ScriptSnapshot {
    pub asset_id: AssetId,
    pub source_path: String,
    pub source: String,
    pub revision: u64,
}

pub(crate) fn read_script(
    session: &EditorDocumentSession,
    asset_id: AssetId,
) -> Result<ScriptSnapshot, String> {
    let (asset, project_root) = locate_script(session, asset_id)?;
    let canonical = fs::canonicalize(&asset.source_file)
        .map_err(|error| format!("cannot resolve Lua script path: {error}"))?;
    if !canonical.starts_with(&project_root) {
        return Err("Lua asset path resolves outside the project root".into());
    }
    let bytes = fs::read(&asset.source_file).map_err(|error| {
        format!(
            "cannot read Lua script '{}': {error}",
            asset.metadata.source_path
        )
    })?;
    if bytes.len() > MAX_SCRIPT_BYTES {
        return Err(format!(
            "Lua script exceeds the {MAX_SCRIPT_BYTES}-byte editor limit"
        ));
    }
    let source = String::from_utf8(bytes)
        .map_err(|error| format!("Lua script is not valid UTF-8: {error}"))?;
    Ok(ScriptSnapshot {
        asset_id,
        source_path: asset.metadata.source_path.clone(),
        revision: revision(&source),
        source,
    })
}

pub(crate) fn write_script(
    session: &EditorDocumentSession,
    asset_id: AssetId,
    expected_revision: u64,
    source: &str,
) -> Result<ScriptSnapshot, String> {
    if source.len() > MAX_SCRIPT_BYTES {
        return Err(format!(
            "Lua script exceeds the {MAX_SCRIPT_BYTES}-byte editor limit"
        ));
    }
    quasar_runtime::gameplay::validate_project_script("project script", source)?;
    let (asset, project_root) = locate_script(session, asset_id)?;
    session.with_project_write(|| {
        let canonical = fs::canonicalize(&asset.source_file)
            .map_err(|error| format!("cannot resolve Lua script path: {error}"))?;
        if !canonical.starts_with(&project_root) {
            return Err("Lua asset path resolves outside the project root".into());
        }
        let current = fs::read_to_string(&canonical)
            .map_err(|error| format!("cannot read current Lua script: {error}"))?;
        if revision(&current) != expected_revision {
            return Err(format!(
                "script_revision_conflict: expected {expected_revision}, current revision is {}",
                revision(&current)
            ));
        }
        replace_file(&canonical, source.as_bytes())
    })?;
    read_script(session, asset_id)
}

fn locate_script(
    session: &EditorDocumentSession,
    asset_id: AssetId,
) -> Result<(quasar_project::assets::AssetRecord, std::path::PathBuf), String> {
    let snapshot = session.snapshot()?;
    let project_root = snapshot
        .path
        .parent()
        .ok_or_else(|| "open project document has no project root".to_owned())?;
    let project_root = fs::canonicalize(project_root)
        .map_err(|error| format!("cannot resolve project root: {error}"))?;
    let catalog = AssetCatalog::scan(&project_root);
    let asset = catalog
        .assets
        .into_iter()
        .find(|asset| asset.metadata.asset_id == asset_id)
        .ok_or_else(|| {
            format!(
                "Lua script asset '{}' is not in the project catalog",
                asset_id.0
            )
        })?;
    if asset.metadata.kind != AssetKind::Script {
        return Err("requested asset is not a Lua Script asset".into());
    }
    if asset.status != AssetStatus::Ready {
        return Err(format!("Lua script asset is not ready: {:?}", asset.status));
    }
    Ok((asset, project_root))
}

fn revision(source: &str) -> u64 {
    source
        .as_bytes()
        .iter()
        .fold(0xcbf29ce484222325, |hash, byte| {
            (hash ^ u64::from(*byte)).wrapping_mul(0x100000001b3)
        })
}

fn replace_file(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let parent = path
        .parent()
        .ok_or_else(|| "Lua script has no parent directory".to_owned())?;
    let nonce = Uuid::new_v4();
    let temporary = parent.join(format!(".quasar-script-{nonce}.tmp"));
    let backup = parent.join(format!(".quasar-script-{nonce}.bak"));
    fs::write(&temporary, bytes).map_err(|error| format!("cannot stage Lua script: {error}"))?;
    if let Err(error) = fs::rename(path, &backup) {
        let _ = fs::remove_file(&temporary);
        return Err(format!(
            "cannot stage current Lua script for replacement: {error}"
        ));
    }
    if let Err(error) = fs::rename(&temporary, path) {
        let rollback = fs::rename(&backup, path);
        let _ = fs::remove_file(&temporary);
        return Err(match rollback {
            Ok(()) => format!("cannot replace Lua script: {error}"),
            Err(rollback) => {
                format!("cannot replace Lua script: {error}; rollback failed: {rollback}")
            }
        });
    }
    fs::remove_file(backup)
        .map_err(|error| format!("script saved, but temporary backup cleanup failed: {error}"))?;
    Ok(())
}
