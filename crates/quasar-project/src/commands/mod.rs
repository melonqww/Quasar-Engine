//! Validated domain commands for editing project documents.

use serde::{Deserialize, Serialize};

use crate::assets::{
    AssetId, MODEL_COMPONENT_TYPE_ID, ModelAssetComponent, SCRIPT_COMPONENT_TYPE_ID,
    ScriptComponent,
};
use crate::document::{
    ComponentDocument, ObjectId, ProjectDocument, SceneDocument, SceneId, SceneObjectDocument,
    TransformDocument,
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
    AssignModelAsset {
        scene_id: SceneId,
        object_id: ObjectId,
        asset_id: Option<AssetId>,
    },
    AssignScriptAsset {
        scene_id: SceneId,
        object_id: ObjectId,
        asset_id: Option<AssetId>,
    },
    SetComponent {
        scene_id: SceneId,
        object_id: ObjectId,
        component: ComponentDocument,
    },
    RemoveComponent {
        scene_id: SceneId,
        object_id: ObjectId,
        type_id: String,
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
            | Self::ReparentObject { scene_id, .. }
            | Self::AssignModelAsset { scene_id, .. }
            | Self::AssignScriptAsset { scene_id, .. }
            | Self::SetComponent { scene_id, .. }
            | Self::RemoveComponent { scene_id, .. } => *scene_id,
        }
    }

    pub fn label(&self) -> &'static str {
        match self {
            Self::CreateObject { .. } => "Create Object",
            Self::DeleteObject { .. } => "Delete Object",
            Self::RenameObject { .. } => "Rename Object",
            Self::SetTransform { .. } => "Set Transform",
            Self::ReparentObject { .. } => "Reparent Object",
            Self::AssignModelAsset { .. } => "Assign Model Asset",
            Self::AssignScriptAsset { .. } => "Assign Script Asset",
            Self::SetComponent { .. } => "Set Component",
            Self::RemoveComponent { .. } => "Remove Component",
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
            Self::AssignModelAsset {
                object_id,
                asset_id,
                ..
            } => {
                let object = find_object_mut(scene, *object_id)?;
                object
                    .components
                    .retain(|component| component.type_id != MODEL_COMPONENT_TYPE_ID);
                if let Some(asset_id) = asset_id {
                    object.components.push(
                        ModelAssetComponent {
                            asset_id: *asset_id,
                        }
                        .into_component(),
                    );
                }
                vec![*object_id]
            }
            Self::AssignScriptAsset {
                object_id,
                asset_id,
                ..
            } => {
                let object = find_object_mut(scene, *object_id)?;
                object
                    .components
                    .retain(|component| component.type_id != SCRIPT_COMPONENT_TYPE_ID);
                if let Some(asset_id) = asset_id {
                    object.components.push(
                        ScriptComponent {
                            asset_id: *asset_id,
                            enabled: true,
                        }
                        .into_component(),
                    );
                }
                vec![*object_id]
            }
            Self::SetComponent {
                object_id,
                component,
                ..
            } => {
                if component.type_id.trim().is_empty() || component.schema_version == 0 {
                    return Err(
                        "component requires a non-empty type_id and non-zero schema_version".into(),
                    );
                }
                let object = find_object_mut(scene, *object_id)?;
                if let Some(existing) = object
                    .components
                    .iter_mut()
                    .find(|existing| existing.type_id == component.type_id)
                {
                    *existing = component.clone();
                } else {
                    object.components.push(component.clone());
                }
                vec![*object_id]
            }
            Self::RemoveComponent {
                object_id, type_id, ..
            } => {
                if type_id.trim().is_empty() {
                    return Err("component type_id cannot be empty".into());
                }
                let object = find_object_mut(scene, *object_id)?;
                let original_len = object.components.len();
                object
                    .components
                    .retain(|component| component.type_id != *type_id);
                if object.components.len() == original_len {
                    return Err(format!(
                        "object '{}' has no component '{}'",
                        object_id.0, type_id
                    ));
                }
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
