//! Durable working copy used by the Stage 0 animation Editor probe.

use std::{
    fs,
    path::{Path, PathBuf},
};

use bevy::prelude::Resource;
use quasar_project::ProjectSnapshot;

#[derive(Resource)]
pub(crate) struct EditorProject {
    pub(crate) snapshot: ProjectSnapshot,
    pub(crate) snapshot_path: PathBuf,
    pub(crate) asset_root: PathBuf,
    pub(crate) dirty: bool,
    pub(crate) status: String,
}

impl EditorProject {
    pub(crate) fn save(&mut self) -> Result<(), String> {
        self.snapshot.write(&self.snapshot_path)?;
        self.dirty = false;
        self.status = format!("Saved {}", self.snapshot_path.display());
        Ok(())
    }

    pub(crate) fn reload(&mut self) -> Result<(), String> {
        let snapshot = ProjectSnapshot::read(&self.snapshot_path)?;
        self.snapshot = snapshot;
        self.dirty = false;
        self.status = format!("Reloaded {}", self.snapshot_path.display());
        Ok(())
    }
}

pub(crate) fn open_stage0_project() -> Result<EditorProject, String> {
    let workspace_root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .map_err(|error| format!("cannot resolve workspace root: {error}"))?;
    if let Some(snapshot_arg) = parse_snapshot_argument(&std::env::args().collect::<Vec<_>>())? {
        return open_project_snapshot(&snapshot_arg);
    }

    let project_root = workspace_root.join("target/stage0-animation-editor-project");
    let snapshot_path = project_root.join("quasar.snapshot.json");
    if !snapshot_path.exists() {
        let fixture_root = workspace_root.join("tests/fixtures/stage0-animation");
        copy_directory(&fixture_root.join("assets"), &project_root.join("assets"))?;
        fs::create_dir_all(&project_root).map_err(|error| {
            format!(
                "cannot create editor project {}: {error}",
                project_root.display()
            )
        })?;
        fs::copy(fixture_root.join("quasar.snapshot.json"), &snapshot_path).map_err(|error| {
            format!(
                "cannot initialize editor snapshot {}: {error}",
                snapshot_path.display()
            )
        })?;
    }
    open_project_snapshot(&snapshot_path)
}

fn open_project_snapshot(path: &Path) -> Result<EditorProject, String> {
    let snapshot_path = path
        .canonicalize()
        .map_err(|error| format!("cannot resolve editor snapshot {}: {error}", path.display()))?;
    let snapshot = ProjectSnapshot::read(&snapshot_path)?;
    let project_root = snapshot_path
        .parent()
        .ok_or_else(|| "editor snapshot has no parent directory".to_owned())?;
    let asset_root = project_root
        .join(&snapshot.assets_directory)
        .canonicalize()
        .map_err(|error| format!("cannot resolve editor assets directory: {error}"))?;
    if !asset_root.is_dir() {
        return Err(format!(
            "editor assets path is not a directory: {}",
            asset_root.display()
        ));
    }
    let mut required_assets = vec![
        snapshot.scene.prop_model.as_str(),
        snapshot.scene.ambience_audio.as_str(),
        snapshot.scene.door_audio.as_str(),
        snapshot.scene.door_script.as_str(),
    ];
    if let Some(animation) = &snapshot.scene.animation {
        required_assets.extend(animation.assets.iter().map(|asset| asset.path.as_str()));
    }
    for relative_path in required_assets {
        let path = asset_root
            .join(relative_path)
            .canonicalize()
            .map_err(|error| format!("cannot resolve project asset '{relative_path}': {error}"))?;
        if !path.starts_with(&asset_root) || !path.is_file() {
            return Err(format!(
                "project asset '{relative_path}' is missing or resolves outside the assets directory"
            ));
        }
    }
    Ok(EditorProject {
        snapshot,
        snapshot_path,
        asset_root,
        dirty: false,
        status: "Project loaded".to_owned(),
    })
}

fn parse_snapshot_argument(args: &[String]) -> Result<Option<PathBuf>, String> {
    let mut snapshot = None;
    let mut index = 1;
    while index < args.len() {
        if args[index] == "--snapshot" {
            if snapshot.is_some() {
                return Err("--snapshot may be supplied only once".to_owned());
            }
            let path = args
                .get(index + 1)
                .ok_or_else(|| "--snapshot requires a path".to_owned())?;
            snapshot = Some(PathBuf::from(path));
            index += 1;
        }
        index += 1;
    }
    Ok(snapshot)
}

fn copy_directory(source: &Path, destination: &Path) -> Result<(), String> {
    fs::create_dir_all(destination).map_err(|error| {
        format!(
            "cannot create assets directory {}: {error}",
            destination.display()
        )
    })?;
    for entry in fs::read_dir(source)
        .map_err(|error| format!("cannot read fixture assets {}: {error}", source.display()))?
    {
        let entry = entry.map_err(|error| format!("cannot read fixture asset entry: {error}"))?;
        let source_path = entry.path();
        let destination_path = destination.join(entry.file_name());
        if source_path.is_dir() {
            copy_directory(&source_path, &destination_path)?;
        } else {
            fs::copy(&source_path, &destination_path).map_err(|error| {
                format!(
                    "cannot copy fixture asset {}: {error}",
                    source_path.display()
                )
            })?;
        }
    }
    Ok(())
}
