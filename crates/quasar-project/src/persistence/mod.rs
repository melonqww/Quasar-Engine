//! Persistence primitives for project documents and temporary Stage 0 fixtures.

use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
};

use serde::de::DeserializeOwned;

pub fn read_json<T: DeserializeOwned>(path: &Path, label: &str) -> Result<T, String> {
    let source = fs::read_to_string(path)
        .map_err(|error| format!("cannot read {label} {}: {error}", path.display()))?;
    serde_json::from_str(&source)
        .map_err(|error| format!("invalid {label} {}: {error}", path.display()))
}

pub fn write_json_atomically(path: &Path, encoded: &[u8], label: &str) -> Result<(), String> {
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent).map_err(|error| {
        format!(
            "cannot create {label} directory {}: {error}",
            parent.display()
        )
    })?;
    let temporary_path = temporary_path(path, parent, label)?;
    let mut temporary_file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temporary_path)
        .map_err(|error| {
            format!(
                "cannot create temporary {label} {}: {error}",
                temporary_path.display()
            )
        })?;
    if let Err(error) = temporary_file
        .write_all(encoded)
        .and_then(|()| temporary_file.sync_all())
    {
        let _ = fs::remove_file(&temporary_path);
        return Err(format!(
            "cannot write temporary {label} {}: {error}",
            temporary_path.display()
        ));
    }
    drop(temporary_file);

    #[cfg(windows)]
    let replace_result = replace_file_windows(&temporary_path, path);
    #[cfg(not(windows))]
    let replace_result = fs::rename(&temporary_path, path);

    replace_result.map_err(|error| {
        let _ = fs::remove_file(&temporary_path);
        format!("cannot replace {label} {}: {error}", path.display())
    })
}

fn temporary_path(destination: &Path, parent: &Path, label: &str) -> Result<PathBuf, String> {
    let file_name = destination
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| {
            format!(
                "{label} path has no valid file name: {}",
                destination.display()
            )
        })?;
    let write_id = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|error| format!("system clock is before Unix epoch: {error}"))?
        .as_nanos();
    Ok(parent.join(format!(
        ".{file_name}.{}-{write_id}.tmp",
        std::process::id()
    )))
}

#[cfg(windows)]
fn replace_file_windows(source: &Path, destination: &Path) -> std::io::Result<()> {
    use std::{ffi::OsStr, os::windows::ffi::OsStrExt};

    const MOVEFILE_REPLACE_EXISTING: u32 = 0x1;
    const MOVEFILE_WRITE_THROUGH: u32 = 0x8;
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn MoveFileExW(existing: *const u16, new: *const u16, flags: u32) -> i32;
    }

    let source: Vec<u16> = OsStr::new(source).encode_wide().chain(Some(0)).collect();
    let destination: Vec<u16> = OsStr::new(destination)
        .encode_wide()
        .chain(Some(0))
        .collect();
    // SAFETY: both strings are nul-terminated UTF-16 buffers that live through
    // the call, and the flags request replacement of one file by another.
    let succeeded = unsafe {
        MoveFileExW(
            source.as_ptr(),
            destination.as_ptr(),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
    };
    if succeeded == 0 {
        Err(std::io::Error::last_os_error())
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use crate::document::{ProjectDocument, SceneDocument, SceneObjectDocument};

    fn temporary_path(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "quasar-project-document-{}-{}-{name}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ))
    }

    fn project() -> ProjectDocument {
        let mut scene = SceneDocument::new("Main");
        scene.objects.push(SceneObjectDocument::new("Root", None));
        ProjectDocument::new("Roundtrip", scene)
    }

    #[test]
    fn project_document_save_load_roundtrip_preserves_ids_and_relationships() {
        let path = temporary_path("project.qproj.json");
        let document = project();
        document.save(&path).expect("project saves");
        let loaded = ProjectDocument::load(&path).expect("project reloads");
        assert_eq!(loaded, document);
        std::fs::remove_file(path).expect("temporary project is removed");
    }

    #[test]
    fn checked_in_stage1_project_fixture_decodes_and_validates() {
        let source = include_str!("../../../../tests/fixtures/stage1-document/quasar.project.json");
        let document: ProjectDocument =
            serde_json::from_str(source).expect("Stage 1 project fixture decodes");
        document
            .validate()
            .expect("Stage 1 project fixture validates");
        assert_eq!(
            document.scenes[0].objects[1].parent_id,
            Some(document.scenes[0].objects[0].id)
        );
        assert_eq!(
            document.scenes[0].objects[1].components[0].data["nested"][1],
            serde_json::Value::Bool(true)
        );
    }

    #[test]
    fn invalid_document_does_not_replace_last_saved_project() {
        let path = temporary_path("preserved.qproj.json");
        let saved = project();
        saved.save(&path).expect("initial project saves");
        let original = std::fs::read(&path).expect("saved bytes can be read");
        let mut invalid = project();
        invalid.name.clear();
        assert!(invalid.save(&path).is_err());
        assert_eq!(std::fs::read(&path).expect("last save remains"), original);
        std::fs::remove_file(path).expect("temporary project is removed");
    }

    #[test]
    fn corrupt_or_newer_documents_fail_to_load_with_clear_errors() {
        let corrupt_path = temporary_path("corrupt.qproj.json");
        std::fs::write(&corrupt_path, "not-json").expect("corrupt fixture writes");
        assert!(
            ProjectDocument::load(&corrupt_path)
                .unwrap_err()
                .contains("invalid project document")
        );
        std::fs::remove_file(corrupt_path).expect("temporary project is removed");

        let newer_path = temporary_path("newer.qproj.json");
        let mut newer = project();
        newer.format_version += 1;
        std::fs::write(
            &newer_path,
            serde_json::to_vec(&newer).expect("newer document serializes"),
        )
        .expect("newer fixture writes");
        assert!(
            ProjectDocument::load(&newer_path)
                .unwrap_err()
                .contains("unsupported project document version")
        );
        std::fs::remove_file(newer_path).expect("temporary project is removed");
    }
}
