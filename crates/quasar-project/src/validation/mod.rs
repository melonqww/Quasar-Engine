//! Validation for persistent project documents.

use std::collections::{HashMap, HashSet};

use crate::document::{PROJECT_DOCUMENT_VERSION, ProjectDocument, SCENE_DOCUMENT_VERSION};

pub fn validate_project(project: &ProjectDocument) -> Result<(), String> {
    if project.format_version != PROJECT_DOCUMENT_VERSION {
        return Err(format!(
            "unsupported project document version {}; supported version is {}",
            project.format_version, PROJECT_DOCUMENT_VERSION
        ));
    }
    if project.name.trim().is_empty() {
        return Err("project name must not be empty".to_owned());
    }
    if project.scenes.is_empty() {
        return Err("project must contain at least one scene".to_owned());
    }

    let mut scene_ids = HashSet::new();
    let mut object_ids = HashSet::new();
    let mut active_scene_found = false;
    for scene in &project.scenes {
        if scene.schema_version != SCENE_DOCUMENT_VERSION {
            return Err(format!(
                "scene '{}' uses unsupported schema version {}; supported version is {}",
                scene.name, scene.schema_version, SCENE_DOCUMENT_VERSION
            ));
        }
        if !scene_ids.insert(scene.id) {
            return Err(format!("duplicate scene id '{}'", scene.id.0));
        }
        if scene.id == project.active_scene_id {
            active_scene_found = true;
        }
        if scene.name.trim().is_empty() {
            return Err(format!("scene '{}' name must not be empty", scene.id.0));
        }

        let mut scene_objects = HashMap::new();
        for object in &scene.objects {
            if object.name.trim().is_empty() {
                return Err(format!("object '{}' name must not be empty", object.id.0));
            }
            if !object_ids.insert(object.id) {
                return Err(format!("duplicate project object id '{}'", object.id.0));
            }
            if object.parent_id == Some(object.id) {
                return Err(format!("object '{}' cannot be its own parent", object.id.0));
            }
            validate_transform(object.id, &object.local_transform)?;
            for component in &object.components {
                if component.type_id.trim().is_empty() || component.schema_version == 0 {
                    return Err(format!(
                        "object '{}' has a component with an empty type id or zero schema version",
                        object.id.0
                    ));
                }
            }
            scene_objects.insert(object.id, object.parent_id);
        }
        for object in &scene.objects {
            if let Some(parent_id) = object.parent_id {
                if !scene_objects.contains_key(&parent_id) {
                    return Err(format!(
                        "object '{}' references missing parent '{}' in scene '{}'",
                        object.id.0, parent_id.0, scene.name
                    ));
                }
                let mut ancestors = HashSet::new();
                let mut current = Some(object.id);
                while let Some(id) = current {
                    if !ancestors.insert(id) {
                        return Err(format!(
                            "scene '{}' contains a parent cycle at object '{}'",
                            scene.name, id.0
                        ));
                    }
                    current = scene_objects.get(&id).copied().flatten();
                }
            }
        }
    }
    if !active_scene_found {
        return Err(format!(
            "active scene '{}' does not exist in the project",
            project.active_scene_id.0
        ));
    }
    Ok(())
}

fn validate_transform(
    object_id: crate::document::ObjectId,
    transform: &crate::document::TransformDocument,
) -> Result<(), String> {
    if transform
        .translation
        .iter()
        .chain(transform.rotation_xyzw.iter())
        .chain(transform.scale.iter())
        .any(|value| !value.is_finite())
    {
        return Err(format!(
            "object '{}' transform contains a non-finite value",
            object_id.0
        ));
    }
    let norm_squared = transform
        .rotation_xyzw
        .iter()
        .map(|value| value * value)
        .sum::<f32>();
    if (norm_squared - 1.0).abs() > 0.001 {
        return Err(format!(
            "object '{}' rotation quaternion must be normalized",
            object_id.0
        ));
    }
    if transform.scale.iter().any(|value| value.abs() < 1e-6) {
        return Err(format!(
            "object '{}' scale components must be non-zero",
            object_id.0
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::validate_project;
    use crate::document::{ObjectId, ProjectDocument, SceneDocument, SceneObjectDocument};

    fn project() -> ProjectDocument {
        let mut scene = SceneDocument::new("Main");
        let root = SceneObjectDocument::new("Root", None);
        let mut child = SceneObjectDocument::new("Child", Some(root.id));
        child.local_transform.translation = [1.0, 2.0, 3.0];
        scene.objects = vec![root, child];
        ProjectDocument::new("Demo", scene)
    }

    #[test]
    fn valid_project_with_hierarchy_and_local_transforms_is_accepted() {
        assert!(validate_project(&project()).is_ok());
    }

    #[test]
    fn missing_active_scene_and_empty_names_are_rejected() {
        let mut document = project();
        document.active_scene_id = crate::document::SceneId::new();
        assert!(
            validate_project(&document)
                .unwrap_err()
                .contains("active scene")
        );

        let mut document = project();
        document.name = "  ".into();
        assert!(
            validate_project(&document)
                .unwrap_err()
                .contains("project name")
        );
    }

    #[test]
    fn missing_parent_and_parent_cycles_are_rejected() {
        let mut document = project();
        document.scenes[0].objects[1].parent_id = Some(ObjectId::new());
        assert!(
            validate_project(&document)
                .unwrap_err()
                .contains("missing parent")
        );

        let mut document = project();
        let first = document.scenes[0].objects[0].id;
        let second = document.scenes[0].objects[1].id;
        document.scenes[0].objects[0].parent_id = Some(second);
        document.scenes[0].objects[1].parent_id = Some(first);
        assert!(
            validate_project(&document)
                .unwrap_err()
                .contains("parent cycle")
        );
    }

    #[test]
    fn invalid_transform_and_duplicate_object_ids_are_rejected() {
        let mut document = project();
        document.scenes[0].objects[0].local_transform.rotation_xyzw = [0.0; 4];
        assert!(
            validate_project(&document)
                .unwrap_err()
                .contains("quaternion")
        );

        let mut document = project();
        let duplicate = document.scenes[0].objects[0].clone();
        document.scenes[0].objects.push(duplicate);
        assert!(
            validate_project(&document)
                .unwrap_err()
                .contains("duplicate project object id")
        );

        let mut document = project();
        document.scenes[0].objects[0].local_transform.scale[1] = 0.0;
        assert!(
            validate_project(&document)
                .unwrap_err()
                .contains("scale components must be non-zero")
        );
    }

    #[test]
    fn unknown_schema_versions_are_rejected() {
        let mut document = project();
        document.format_version += 1;
        assert!(
            validate_project(&document)
                .unwrap_err()
                .contains("unsupported project document")
        );

        let mut document = project();
        document.scenes[0].schema_version += 1;
        assert!(
            validate_project(&document)
                .unwrap_err()
                .contains("unsupported schema version")
        );
    }

    #[test]
    fn scene_ids_are_unique_and_parent_links_cannot_cross_scenes() {
        let mut document = project();
        let mut duplicate_scene = document.scenes[0].clone();
        duplicate_scene.name = "Duplicate ID".into();
        document.scenes.push(duplicate_scene);
        assert!(
            validate_project(&document)
                .unwrap_err()
                .contains("duplicate scene id")
        );

        let mut document = project();
        let first_scene_root = document.scenes[0].objects[0].id;
        let mut other_scene = crate::document::SceneDocument::new("Other");
        other_scene.objects.push(SceneObjectDocument::new(
            "Invalid cross-scene child",
            Some(first_scene_root),
        ));
        document.scenes.push(other_scene);
        assert!(
            validate_project(&document)
                .unwrap_err()
                .contains("missing parent")
        );
    }

    #[test]
    fn component_envelope_requires_a_type_id_and_schema_version() {
        let mut document = project();
        document.scenes[0].objects[0]
            .components
            .push(crate::document::ComponentDocument {
                type_id: " ".into(),
                schema_version: 0,
                data: serde_json::Value::Null,
            });
        assert!(
            validate_project(&document)
                .unwrap_err()
                .contains("component with an empty type id or zero schema version")
        );
    }
}
