//! Writable ProjectDocument session shared by the Editor UI and local adapters.

use std::{
    path::{Path, PathBuf},
    sync::{Arc, RwLock},
};

use bevy::prelude::Resource;
use quasar_project::{
    commands::SceneCommand,
    document::{ProjectDocument, SceneDocument, SceneId},
};
use uuid::Uuid;

const MAX_UNDO_ENTRIES: usize = 128;

#[derive(Clone, Resource)]
pub struct EditorDocumentSession {
    state: Arc<RwLock<SessionState>>,
}

#[derive(Clone, Debug)]
pub struct DocumentSessionSnapshot {
    pub document: ProjectDocument,
    pub path: PathBuf,
    pub revision: u64,
    pub dirty: bool,
    pub undo_depth: usize,
    pub redo_depth: usize,
    pub active_gesture: bool,
}

#[derive(Clone, Debug)]
pub struct CommandReceipt {
    pub revision: u64,
    pub label: String,
    pub affected_objects: Vec<quasar_project::document::ObjectId>,
    pub changed: bool,
}

#[derive(Clone, Debug)]
pub struct CommandPreview {
    pub revision: u64,
    pub label: String,
    pub affected_objects: Vec<quasar_project::document::ObjectId>,
    pub changed: bool,
    pub document: ProjectDocument,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TransformGestureId(Uuid);

struct SessionState {
    document: ProjectDocument,
    saved_document: ProjectDocument,
    path: PathBuf,
    revision: u64,
    undo: Vec<HistoryEntry>,
    redo: Vec<HistoryEntry>,
    active_gesture: Option<ActiveGesture>,
    play_run_id: Option<Uuid>,
}

struct HistoryEntry {
    scene_id: SceneId,
    before: SceneDocument,
    after: SceneDocument,
    label: String,
}

struct ActiveGesture {
    id: TransformGestureId,
    label: String,
    scene_id: Option<SceneId>,
    before: Option<SceneDocument>,
}

impl EditorDocumentSession {
    pub fn open(path: &Path) -> Result<Self, String> {
        let path = std::fs::canonicalize(path).map_err(|error| {
            format!(
                "cannot resolve project document {}: {error}",
                path.display()
            )
        })?;
        let document = ProjectDocument::load(&path)?;
        Ok(Self {
            state: Arc::new(RwLock::new(SessionState {
                saved_document: document.clone(),
                document,
                path,
                revision: 0,
                undo: Vec::new(),
                redo: Vec::new(),
                active_gesture: None,
                play_run_id: None,
            })),
        })
    }

    pub fn snapshot(&self) -> Result<DocumentSessionSnapshot, String> {
        let state = self
            .state
            .read()
            .map_err(|_| "Editor document session is unavailable".to_owned())?;
        Ok(snapshot_of(&state))
    }

    pub fn apply_command(
        &self,
        command: SceneCommand,
        expected_revision: u64,
    ) -> Result<CommandReceipt, String> {
        let mut state = self
            .state
            .write()
            .map_err(|_| "Editor document session is unavailable".to_owned())?;
        check_revision(&state, expected_revision)?;
        ensure_not_playing(&state)?;
        if state.active_gesture.is_some() {
            return Err(
                "finish or cancel the active transform gesture before another command".to_owned(),
            );
        }

        let scene_id = command.scene_id();
        let before = scene_snapshot(&state.document, scene_id)?;
        let mut candidate = state.document.clone();
        let change = command.apply(&mut candidate)?;
        candidate.validate()?;
        if candidate == state.document {
            return Ok(CommandReceipt {
                revision: state.revision,
                label: change.label.to_owned(),
                affected_objects: change.affected_objects,
                changed: false,
            });
        }
        let after = scene_snapshot(&candidate, scene_id)?;
        state.document = candidate;
        state.revision += 1;
        push_undo(
            &mut state,
            HistoryEntry {
                scene_id,
                before,
                after,
                label: change.label.to_owned(),
            },
        );
        Ok(CommandReceipt {
            revision: state.revision,
            label: change.label.to_owned(),
            affected_objects: change.affected_objects,
            changed: true,
        })
    }

    /// Validates a command against a private candidate and returns the resulting document
    /// without changing revision, dirty state, history, or the open project.
    pub fn preview_command(
        &self,
        command: SceneCommand,
        expected_revision: u64,
    ) -> Result<CommandPreview, String> {
        let state = self
            .state
            .read()
            .map_err(|_| "Editor document session is unavailable".to_owned())?;
        check_revision(&state, expected_revision)?;
        ensure_not_playing(&state)?;
        if state.active_gesture.is_some() {
            return Err(
                "finish or cancel the active transform gesture before previewing a command"
                    .to_owned(),
            );
        }
        let mut candidate = state.document.clone();
        let change = command.apply(&mut candidate)?;
        candidate.validate()?;
        Ok(CommandPreview {
            revision: state.revision,
            label: change.label.to_owned(),
            affected_objects: change.affected_objects,
            changed: candidate != state.document,
            document: candidate,
        })
    }

    pub fn begin_transform_gesture(
        &self,
        label: impl Into<String>,
        expected_revision: u64,
    ) -> Result<TransformGestureId, String> {
        let mut state = self
            .state
            .write()
            .map_err(|_| "Editor document session is unavailable".to_owned())?;
        check_revision(&state, expected_revision)?;
        ensure_not_playing(&state)?;
        if state.active_gesture.is_some() {
            return Err("a transform gesture is already active".to_owned());
        }
        let id = TransformGestureId(Uuid::new_v4());
        state.active_gesture = Some(ActiveGesture {
            id,
            label: label.into(),
            scene_id: None,
            before: None,
        });
        Ok(id)
    }

    pub fn update_transform_gesture(
        &self,
        id: TransformGestureId,
        command: SceneCommand,
        expected_revision: u64,
    ) -> Result<CommandReceipt, String> {
        if !command.is_transform_edit() {
            return Err("a transform gesture accepts only SetTransform commands".to_owned());
        }
        let mut state = self
            .state
            .write()
            .map_err(|_| "Editor document session is unavailable".to_owned())?;
        check_revision(&state, expected_revision)?;
        ensure_not_playing(&state)?;
        let scene_id = command.scene_id();
        let gesture = state
            .active_gesture
            .as_ref()
            .filter(|gesture| gesture.id == id)
            .ok_or_else(|| "transform gesture is missing or expired".to_owned())?;
        if gesture
            .scene_id
            .is_some_and(|active_scene| active_scene != scene_id)
        {
            return Err("one transform gesture cannot span multiple scenes".to_owned());
        }

        let before = scene_snapshot(&state.document, scene_id)?;
        let mut candidate = state.document.clone();
        let change = command.apply(&mut candidate)?;
        candidate.validate()?;
        if candidate == state.document {
            return Ok(CommandReceipt {
                revision: state.revision,
                label: change.label.to_owned(),
                affected_objects: change.affected_objects,
                changed: false,
            });
        }
        if let Some(gesture) = state.active_gesture.as_mut() {
            gesture.scene_id = Some(scene_id);
            gesture.before.get_or_insert(before);
        }
        state.document = candidate;
        state.revision += 1;
        Ok(CommandReceipt {
            revision: state.revision,
            label: change.label.to_owned(),
            affected_objects: change.affected_objects,
            changed: true,
        })
    }

    pub fn finish_transform_gesture(
        &self,
        id: TransformGestureId,
    ) -> Result<Option<CommandReceipt>, String> {
        let mut state = self
            .state
            .write()
            .map_err(|_| "Editor document session is unavailable".to_owned())?;
        if !state
            .active_gesture
            .as_ref()
            .is_some_and(|gesture| gesture.id == id)
        {
            return Err("transform gesture is missing or expired".to_owned());
        }
        let gesture = state
            .active_gesture
            .take()
            .expect("matching active gesture was just checked");
        let (Some(scene_id), Some(before)) = (gesture.scene_id, gesture.before) else {
            return Ok(None);
        };
        let after = scene_snapshot(&state.document, scene_id)?;
        if before == after {
            return Ok(None);
        }
        let affected_objects = changed_object_ids(&before, &after);
        push_undo(
            &mut state,
            HistoryEntry {
                scene_id,
                before,
                after,
                label: gesture.label.clone(),
            },
        );
        Ok(Some(CommandReceipt {
            revision: state.revision,
            label: gesture.label,
            affected_objects,
            changed: true,
        }))
    }

    pub fn cancel_transform_gesture(&self, id: TransformGestureId) -> Result<bool, String> {
        let mut state = self
            .state
            .write()
            .map_err(|_| "Editor document session is unavailable".to_owned())?;
        if !state
            .active_gesture
            .as_ref()
            .is_some_and(|gesture| gesture.id == id)
        {
            return Err("transform gesture is missing or expired".to_owned());
        }
        let gesture = state
            .active_gesture
            .take()
            .expect("matching active gesture was just checked");
        let (Some(scene_id), Some(before)) = (gesture.scene_id, gesture.before) else {
            return Ok(false);
        };
        let scene = find_scene_mut(&mut state.document, scene_id)?;
        if *scene == before {
            return Ok(false);
        }
        *scene = before;
        state.revision += 1;
        Ok(true)
    }

    pub fn undo(&self, expected_revision: u64) -> Result<CommandReceipt, String> {
        let mut state = self
            .state
            .write()
            .map_err(|_| "Editor document session is unavailable".to_owned())?;
        check_revision(&state, expected_revision)?;
        ensure_not_playing(&state)?;
        ensure_no_active_gesture(&state)?;
        let entry = state
            .undo
            .pop()
            .ok_or_else(|| "nothing to undo".to_owned())?;
        *find_scene_mut(&mut state.document, entry.scene_id)? = entry.before.clone();
        state.document.validate()?;
        state.revision += 1;
        let receipt = CommandReceipt {
            revision: state.revision,
            label: format!("Undo {}", entry.label),
            affected_objects: changed_object_ids(&entry.before, &entry.after),
            changed: true,
        };
        state.redo.push(entry);
        Ok(receipt)
    }

    pub fn redo(&self, expected_revision: u64) -> Result<CommandReceipt, String> {
        let mut state = self
            .state
            .write()
            .map_err(|_| "Editor document session is unavailable".to_owned())?;
        check_revision(&state, expected_revision)?;
        ensure_not_playing(&state)?;
        ensure_no_active_gesture(&state)?;
        let entry = state
            .redo
            .pop()
            .ok_or_else(|| "nothing to redo".to_owned())?;
        *find_scene_mut(&mut state.document, entry.scene_id)? = entry.after.clone();
        state.document.validate()?;
        state.revision += 1;
        let receipt = CommandReceipt {
            revision: state.revision,
            label: format!("Redo {}", entry.label),
            affected_objects: changed_object_ids(&entry.before, &entry.after),
            changed: true,
        };
        state.undo.push(entry);
        trim_history(&mut state.undo);
        Ok(receipt)
    }

    pub fn save(&self, expected_revision: u64) -> Result<DocumentSessionSnapshot, String> {
        let mut state = self
            .state
            .write()
            .map_err(|_| "Editor document session is unavailable".to_owned())?;
        check_revision(&state, expected_revision)?;
        ensure_not_playing(&state)?;
        ensure_no_active_gesture(&state)?;
        state.document.save(&state.path)?;
        state.saved_document = state.document.clone();
        Ok(snapshot_of(&state))
    }

    pub(crate) fn begin_play(&self, run_id: Uuid) -> Result<(), String> {
        let mut state = self
            .state
            .write()
            .map_err(|_| "Editor document session is unavailable".to_owned())?;
        if state.play_run_id.is_some() {
            return Err("a Play session is already active".to_owned());
        }
        if state.active_gesture.is_some() {
            return Err("finish or cancel the active edit gesture before starting Play".to_owned());
        }
        state.play_run_id = Some(run_id);
        Ok(())
    }

    pub(crate) fn end_play(&self, run_id: Uuid) {
        if let Ok(mut state) = self.state.write()
            && state.play_run_id == Some(run_id)
        {
            state.play_run_id = None;
        }
    }

    pub(crate) fn with_project_write<T>(
        &self,
        action: impl FnOnce() -> Result<T, String>,
    ) -> Result<T, String> {
        let state = self
            .state
            .write()
            .map_err(|_| "Editor document session is unavailable".to_owned())?;
        ensure_not_playing(&state)?;
        action()
    }
}

fn ensure_not_playing(state: &SessionState) -> Result<(), String> {
    if state.play_run_id.is_some() {
        Err("project editing is locked while Play is running".to_owned())
    } else {
        Ok(())
    }
}

fn snapshot_of(state: &SessionState) -> DocumentSessionSnapshot {
    DocumentSessionSnapshot {
        document: state.document.clone(),
        path: state.path.clone(),
        revision: state.revision,
        dirty: state.document != state.saved_document,
        undo_depth: state.undo.len(),
        redo_depth: state.redo.len(),
        active_gesture: state.active_gesture.is_some(),
    }
}

fn check_revision(state: &SessionState, expected: u64) -> Result<(), String> {
    if expected != state.revision {
        return Err(format!(
            "revision_conflict: expected {expected}, current revision is {}",
            state.revision
        ));
    }
    Ok(())
}

fn ensure_no_active_gesture(state: &SessionState) -> Result<(), String> {
    if state.active_gesture.is_some() {
        Err("finish or cancel the active transform gesture first".to_owned())
    } else {
        Ok(())
    }
}

fn scene_snapshot(document: &ProjectDocument, scene_id: SceneId) -> Result<SceneDocument, String> {
    document
        .scenes
        .iter()
        .find(|scene| scene.id == scene_id)
        .cloned()
        .ok_or_else(|| format!("scene '{}' does not exist", scene_id.0))
}

fn find_scene_mut(
    document: &mut ProjectDocument,
    scene_id: SceneId,
) -> Result<&mut SceneDocument, String> {
    document
        .scenes
        .iter_mut()
        .find(|scene| scene.id == scene_id)
        .ok_or_else(|| format!("scene '{}' does not exist", scene_id.0))
}

fn push_undo(state: &mut SessionState, entry: HistoryEntry) {
    state.undo.push(entry);
    trim_history(&mut state.undo);
    state.redo.clear();
}

fn trim_history(history: &mut Vec<HistoryEntry>) {
    if history.len() > MAX_UNDO_ENTRIES {
        let remove_count = history.len() - MAX_UNDO_ENTRIES;
        history.drain(0..remove_count);
    }
}

fn changed_object_ids(
    before: &SceneDocument,
    after: &SceneDocument,
) -> Vec<quasar_project::document::ObjectId> {
    let before_by_id = before
        .objects
        .iter()
        .map(|object| (object.id, object))
        .collect::<std::collections::HashMap<_, _>>();
    let after_by_id = after
        .objects
        .iter()
        .map(|object| (object.id, object))
        .collect::<std::collections::HashMap<_, _>>();
    let mut ids = before_by_id
        .iter()
        .filter(|(id, object)| after_by_id.get(id).is_none_or(|newer| *newer != **object))
        .map(|(id, _)| *id)
        .chain(
            after_by_id
                .keys()
                .filter(|id| !before_by_id.contains_key(id))
                .copied(),
        )
        .collect::<Vec<_>>();
    ids.sort_unstable();
    ids
}

#[cfg(test)]
mod tests {
    use super::*;
    use quasar_project::document::{
        ProjectDocument, SceneDocument, SceneObjectDocument, TransformDocument,
    };

    struct TemporaryProject(PathBuf);

    impl TemporaryProject {
        fn new() -> Self {
            let directory =
                std::env::temp_dir().join(format!("quasar-session-test-{}", Uuid::new_v4()));
            std::fs::create_dir_all(&directory).expect("create isolated temporary directory");
            Self(directory.join("project.json"))
        }

        fn initialize(&self) {
            ProjectDocument::new("Session test", SceneDocument::new("Scene"))
                .save(&self.0)
                .expect("write initial project document");
        }
    }

    impl Drop for TemporaryProject {
        fn drop(&mut self) {
            if let Some(directory) = self.0.parent() {
                let _ = std::fs::remove_dir_all(directory);
            }
        }
    }

    #[test]
    fn preview_is_read_only_and_stale_revision_is_rejected() {
        let file = TemporaryProject::new();
        file.initialize();
        let session = EditorDocumentSession::open(&file.0).expect("open project session");
        let initial = session.snapshot().expect("read initial snapshot");
        let scene_id = initial.document.active_scene_id;
        let object = SceneObjectDocument::new("Cube", None);
        let command = SceneCommand::CreateObject {
            scene_id,
            object: object.clone(),
        };

        let preview = session
            .preview_command(command.clone(), 0)
            .expect("preview command");
        assert!(preview.changed);
        assert_eq!(preview.revision, 0);
        assert_eq!(preview.document.scenes[0].objects.len(), 1);
        let unchanged = session.snapshot().expect("read session after preview");
        assert_eq!(unchanged.revision, 0);
        assert!(!unchanged.dirty);
        assert!(unchanged.document.scenes[0].objects.is_empty());

        let applied = session.apply_command(command, 0).expect("apply command");
        assert_eq!(applied.revision, 1);
        assert!(session.undo(0).unwrap_err().contains("revision_conflict"));
    }

    #[test]
    fn transform_gesture_is_one_undo_and_save_reopens_document() {
        let file = TemporaryProject::new();
        file.initialize();
        let session = EditorDocumentSession::open(&file.0).expect("open project session");
        let snapshot = session.snapshot().expect("read initial snapshot");
        let scene_id = snapshot.document.active_scene_id;
        let object = SceneObjectDocument::new("Cube", None);
        let object_id = object.id;
        session
            .apply_command(SceneCommand::CreateObject { scene_id, object }, 0)
            .expect("create object");

        let gesture = session
            .begin_transform_gesture("Move cube", 1)
            .expect("begin gesture");
        for (revision, x) in [(1, 1.0), (2, 2.0)] {
            session
                .update_transform_gesture(
                    gesture,
                    SceneCommand::SetTransform {
                        scene_id,
                        object_id,
                        transform: TransformDocument {
                            translation: [x, 0.0, 0.0],
                            ..TransformDocument::default()
                        },
                    },
                    revision,
                )
                .expect("update transform");
        }
        session
            .finish_transform_gesture(gesture)
            .expect("finish gesture");
        let current = session.snapshot().expect("read changed snapshot");
        assert_eq!(current.revision, 3);
        assert_eq!(
            current.undo_depth, 2,
            "create plus one grouped transform entry"
        );

        session.undo(3).expect("undo grouped transform");
        let undone = session.snapshot().expect("read undone snapshot");
        assert_eq!(
            undone.document.scenes[0].objects[0].local_transform,
            TransformDocument::default()
        );
        session.redo(4).expect("redo grouped transform");
        let saved = session.save(5).expect("save project");
        assert!(!saved.dirty);

        let reopened = ProjectDocument::load(&file.0).expect("reopen saved project");
        assert_eq!(
            reopened.scenes[0].objects[0].local_transform.translation,
            [2.0, 0.0, 0.0]
        );
    }

    #[test]
    fn cancel_transform_gesture_restores_document_without_adding_history() {
        let file = TemporaryProject::new();
        file.initialize();
        let session = EditorDocumentSession::open(&file.0).expect("open project session");
        let initial = session.snapshot().expect("read initial snapshot");
        let scene_id = initial.document.active_scene_id;
        let object = SceneObjectDocument::new("Cube", None);
        let object_id = object.id;
        session
            .apply_command(SceneCommand::CreateObject { scene_id, object }, 0)
            .expect("create object");

        let gesture = session
            .begin_transform_gesture("Cancelled move", 1)
            .expect("begin gesture");
        session
            .update_transform_gesture(
                gesture,
                SceneCommand::SetTransform {
                    scene_id,
                    object_id,
                    transform: TransformDocument {
                        translation: [9.0, 4.0, -2.0],
                        ..TransformDocument::default()
                    },
                },
                1,
            )
            .expect("update transform");
        assert!(
            session
                .cancel_transform_gesture(gesture)
                .expect("cancel gesture")
        );

        let cancelled = session.snapshot().expect("read cancelled snapshot");
        assert_eq!(cancelled.revision, 3);
        assert_eq!(
            cancelled.undo_depth, 1,
            "cancel adds no transform history entry"
        );
        assert!(!cancelled.active_gesture);
        assert_eq!(
            cancelled.document.scenes[0].objects[0].local_transform,
            TransformDocument::default()
        );
    }
}
