//! Validated domain commands for editing project documents.

use serde::{Deserialize, Serialize};

use crate::document::{
    ObjectId, ProjectDocument, SceneDocument, SceneId, SceneObjectDocument, TransformDocument,
};

/// A user-level scene edit. UI gestures and MCP tools map to these same commands.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum SceneCommand {
    CreateObject {
        scene_id: SceneId,
        object: SceneObjectDocument,
    },
    DeleteObject {
        scene_id: SceneId,
        object_id: ObjectId,
    },
    RenameObject {
        scene_id: SceneId,
        object_id: ObjectId,
        name: String,
    },
    SetTransform {
        scene_id: SceneId,
        object_id: ObjectId,
        transform: TransformDocument,
    },
    ReparentObject {
        scene_id: SceneId,
        object_id: ObjectId,
        parent_id: Option<ObjectId>,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CommandChange {
    pub label: &'static str,
    pub scene_id: SceneId,
    pub affected_objects: Vec<ObjectId>,
}

impl SceneCommand {
    pub fn scene_id(&self) -> SceneId {
        match self {
            Self::CreateObject { scene_id, .. }
            | Self::DeleteObject { scene_id, .. }
            | Self::RenameObject { scene_id, .. }
            | Self::SetTransform { scene_id, .. }
            | Self::ReparentObject { scene_id, .. } => *scene_id,
        }
    }

    pub fn label(&self) -> &'static str {
        match self {
            Self::CreateObject { .. } => "Create Object",
            Self::DeleteObject { .. } => "Delete Object",
            Self::RenameObject { .. } => "Rename Object",
            Self::SetTransform { .. } => "Set Transform",
            Self::ReparentObject { .. } => "Reparent Object",
        }
    }

    pub fn is_transform_edit(&self) -> bool {
        matches!(self, Self::SetTransform { .. })
    }

    /// Applies to an in-memory candidate. The caller validates the full project and only
    /// publishes the candidate after success, keeping a failed command atomic.
    pub fn apply(&self, document: &mut ProjectDocument) -> Result<CommandChange, String> {
        let scene_id = self.scene_id();
        let scene = document
            .scenes
            .iter_mut()
            .find(|scene| scene.id == scene_id)
            .ok_or_else(|| format!("scene '{}' does not exist", scene_id.0))?;
        let affected_objects = match self {
            Self::CreateObject { object, .. } => {
                if scene
                    .objects
                    .iter()
                    .any(|existing| existing.id == object.id)
                {
                    return Err(format!("object '{}' already exists", object.id.0));
                }
                let id = object.id;
                scene.objects.push(object.clone());
                vec![id]
            }
            Self::DeleteObject { object_id, .. } => delete_subtree(scene, *object_id)?,
            Self::RenameObject {
                object_id, name, ..
            } => {
                let object = find_object_mut(scene, *object_id)?;
                object.name.clone_from(name);
                vec![*object_id]
            }
            Self::SetTransform {
                object_id,
                transform,
                ..
            } => {
                let object = find_object_mut(scene, *object_id)?;
                object.local_transform = *transform;
                vec![*object_id]
            }
            Self::ReparentObject {
                object_id,
                parent_id,
                ..
            } => {
                let object = find_object_mut(scene, *object_id)?;
                object.parent_id = *parent_id;
                vec![*object_id]
            }
        };

        Ok(CommandChange {
            label: self.label(),
            scene_id,
            affected_objects,
        })
    }
}

fn find_object_mut(
    scene: &mut SceneDocument,
    object_id: ObjectId,
) -> Result<&mut SceneObjectDocument, String> {
    scene
        .objects
        .iter_mut()
        .find(|object| object.id == object_id)
        .ok_or_else(|| {
            format!(
                "object '{}' does not exist in scene '{}'",
                object_id.0, scene.id.0
            )
        })
}

fn delete_subtree(scene: &mut SceneDocument, root_id: ObjectId) -> Result<Vec<ObjectId>, String> {
    if !scene.objects.iter().any(|object| object.id == root_id) {
        return Err(format!(
            "object '{}' does not exist in scene '{}'",
            root_id.0, scene.id.0
        ));
    }
    let mut removed = vec![root_id];
    let mut index = 0;
    while index < removed.len() {
        let parent_id = removed[index];
        for object in &scene.objects {
            if object.parent_id == Some(parent_id) && !removed.contains(&object.id) {
                removed.push(object.id);
            }
        }
        index += 1;
    }
    scene.objects.retain(|object| !removed.contains(&object.id));
    Ok(removed)
}
