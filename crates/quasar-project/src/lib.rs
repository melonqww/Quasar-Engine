//! Authoring data and project persistence for Quasar Engine.

use std::{fs, path::Path};

use serde::{Deserialize, Serialize};

pub const SNAPSHOT_SCHEMA_VERSION: u32 = 1;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ProjectSnapshot {
    pub schema_version: u32,
    pub scene: SceneSnapshot,
    pub assets_directory: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SceneSnapshot {
    pub name: String,
    pub objects: Vec<SnapshotObject>,
    pub prop_model: String,
    pub ambience_audio: String,
    pub door_audio: String,
    pub door_script: String,
    /// Optional Stage 0 authoring probe. Older snapshots remain valid without it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub animation: Option<AnimationSnapshot>,
}

/// Temporary, versioned data for validating animation authoring and imported clips.
/// This is not the final SceneDocument schema.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AnimationSnapshot {
    pub assets: Vec<AnimationAssetSnapshot>,
    pub object_clips: Vec<ObjectAnimationClipSnapshot>,
    pub imported_bindings: Vec<ImportedAnimationBindingSnapshot>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AnimationAssetSnapshot {
    pub id: String,
    pub kind: AnimationAssetKind,
    pub path: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AnimationAssetKind {
    Model,
    Audio,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ObjectAnimationClipSnapshot {
    pub id: String,
    pub object_id: String,
    pub duration_seconds: f32,
    pub tracks: Vec<TransformAnimationTrackSnapshot>,
    pub events: Vec<AnimationEventSnapshot>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TransformAnimationTrackSnapshot {
    pub property: AnimatedTransformProperty,
    pub interpolation: AnimationInterpolation,
    pub keys: Vec<AnimationKeySnapshot>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AnimatedTransformProperty {
    Translation,
    Rotation,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AnimationInterpolation {
    Step,
    Linear,
    SmoothStep,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AnimationKeySnapshot {
    pub time_seconds: f32,
    pub value: AnimationValueSnapshot,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", content = "components", rename_all = "snake_case")]
pub enum AnimationValueSnapshot {
    Vec3([f32; 3]),
    Quaternion([f32; 4]),
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AnimationEventSnapshot {
    pub id: String,
    pub time_seconds: f32,
    #[serde(flatten)]
    pub kind: AnimationEventKindSnapshot,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AnimationEventKindSnapshot {
    PlaySound { asset_id: String },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImportedAnimationBindingSnapshot {
    pub object_id: String,
    pub asset_id: String,
    pub clips: Vec<ImportedAnimationClipSnapshot>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImportedAnimationClipSnapshot {
    /// Stable within this asset; for this fixture it is the exact glTF animation name.
    pub source_clip_id: String,
    pub display_name: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SnapshotObjectKind {
    Floor,
    Wall,
    Door,
    Character,
    Prop,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SnapshotObject {
    pub id: String,
    pub kind: SnapshotObjectKind,
    pub position: [f32; 3],
    pub scale: [f32; 3],
}

impl ProjectSnapshot {
    pub fn read(path: &Path) -> Result<Self, String> {
        let source = fs::read_to_string(path)
            .map_err(|error| format!("cannot read snapshot {}: {error}", path.display()))?;
        let snapshot: Self = serde_json::from_str(&source)
            .map_err(|error| format!("invalid snapshot {}: {error}", path.display()))?;
        snapshot.validate()?;
        Ok(snapshot)
    }

    pub fn validate(&self) -> Result<(), String> {
        if self.schema_version != SNAPSHOT_SCHEMA_VERSION {
            return Err(format!(
                "unsupported snapshot schema version {}; supported version is {}",
                self.schema_version, SNAPSHOT_SCHEMA_VERSION
            ));
        }
        if self.scene.name.trim().is_empty() {
            return Err("snapshot scene name must not be empty".to_owned());
        }
        if self.scene.objects.is_empty() {
            return Err("snapshot scene must contain at least one object".to_owned());
        }
        let mut object_ids = std::collections::HashSet::new();
        let mut has_door = false;
        let mut has_character = false;
        let mut has_prop = false;
        for object in &self.scene.objects {
            if object.id.trim().is_empty() || !object_ids.insert(&object.id) {
                return Err(format!(
                    "snapshot object id '{}' must be non-empty and unique",
                    object.id
                ));
            }
            if object
                .position
                .iter()
                .chain(object.scale.iter())
                .any(|value| !value.is_finite())
            {
                return Err(format!(
                    "snapshot object '{}' has a non-finite transform",
                    object.id
                ));
            }
            if object.scale.iter().any(|value| *value <= 0.0) {
                return Err(format!(
                    "snapshot object '{}' scale must be positive",
                    object.id
                ));
            }
            has_door |= object.kind == SnapshotObjectKind::Door;
            has_character |= object.kind == SnapshotObjectKind::Character;
            has_prop |= object.kind == SnapshotObjectKind::Prop;
        }
        if !has_door || !has_character || !has_prop {
            return Err("stage 0 snapshot requires a door, character, and GLB prop".to_owned());
        }
        if let Some(animation) = &self.scene.animation {
            animation.validate(&self.scene.objects)?;
        }
        for (label, value) in [
            ("assets directory", self.assets_directory.as_str()),
            ("prop model", self.scene.prop_model.as_str()),
            ("ambience audio", self.scene.ambience_audio.as_str()),
            ("door audio", self.scene.door_audio.as_str()),
            ("door script", self.scene.door_script.as_str()),
        ] {
            validate_relative_path(label, value)?;
        }
        Ok(())
    }
}

impl AnimationSnapshot {
    fn validate(&self, objects: &[SnapshotObject]) -> Result<(), String> {
        let mut assets = std::collections::HashMap::new();
        for asset in &self.assets {
            if asset.id.trim().is_empty() || assets.insert(asset.id.as_str(), asset).is_some() {
                return Err(format!(
                    "animation asset id '{}' must be non-empty and unique",
                    asset.id
                ));
            }
            validate_relative_path("animation asset", &asset.path)?;
        }

        let object_by_id: std::collections::HashMap<_, _> = objects
            .iter()
            .map(|object| (object.id.as_str(), object))
            .collect();
        let mut clip_ids = std::collections::HashSet::new();
        for clip in &self.object_clips {
            if clip.id.trim().is_empty() || !clip_ids.insert(clip.id.as_str()) {
                return Err(format!(
                    "animation clip id '{}' must be non-empty and unique",
                    clip.id
                ));
            }
            if !object_by_id.contains_key(clip.object_id.as_str()) {
                return Err(format!(
                    "animation clip '{}' references missing object '{}'",
                    clip.id, clip.object_id
                ));
            }
            if !clip.duration_seconds.is_finite() || clip.duration_seconds <= 0.0 {
                return Err(format!(
                    "animation clip '{}' duration must be finite and positive",
                    clip.id
                ));
            }
            if clip.tracks.is_empty() {
                return Err(format!(
                    "animation clip '{}' must contain at least one track",
                    clip.id
                ));
            }

            let mut properties = std::collections::HashSet::new();
            for track in &clip.tracks {
                if !properties.insert(track.property) {
                    return Err(format!(
                        "animation clip '{}' contains duplicate {:?} tracks",
                        clip.id, track.property
                    ));
                }
                if track.keys.len() < 2 {
                    return Err(format!(
                        "animation clip '{}' {:?} track requires at least two keys",
                        clip.id, track.property
                    ));
                }
                let mut previous_time = None;
                for key in &track.keys {
                    if !key.time_seconds.is_finite()
                        || key.time_seconds < 0.0
                        || key.time_seconds > clip.duration_seconds
                        || previous_time.is_some_and(|time| key.time_seconds <= time)
                    {
                        return Err(format!(
                            "animation clip '{}' key times must be finite, strictly increasing, and within the clip duration",
                            clip.id
                        ));
                    }
                    previous_time = Some(key.time_seconds);
                    let value_matches = match (&track.property, &key.value) {
                        (
                            AnimatedTransformProperty::Translation,
                            AnimationValueSnapshot::Vec3(values),
                        ) => values.iter().all(|value| value.is_finite()),
                        (
                            AnimatedTransformProperty::Rotation,
                            AnimationValueSnapshot::Quaternion(values),
                        ) => {
                            values.iter().all(|value| value.is_finite())
                                && values.iter().map(|value| value * value).sum::<f32>() > 1.0e-8
                        }
                        _ => false,
                    };
                    if !value_matches {
                        return Err(format!(
                            "animation clip '{}' {:?} key has an incompatible or invalid value",
                            clip.id, track.property
                        ));
                    }
                }
            }

            let mut event_ids = std::collections::HashSet::new();
            for event in &clip.events {
                if event.id.trim().is_empty() || !event_ids.insert(event.id.as_str()) {
                    return Err(format!(
                        "animation clip '{}' event id '{}' must be non-empty and unique",
                        clip.id, event.id
                    ));
                }
                if !event.time_seconds.is_finite()
                    || event.time_seconds < 0.0
                    || event.time_seconds > clip.duration_seconds
                {
                    return Err(format!(
                        "animation clip '{}' event '{}' time must be within the clip duration",
                        clip.id, event.id
                    ));
                }
                match &event.kind {
                    AnimationEventKindSnapshot::PlaySound { asset_id } => {
                        let Some(asset) = assets.get(asset_id.as_str()) else {
                            return Err(format!(
                                "animation clip '{}' event '{}' references missing asset '{}'",
                                clip.id, event.id, asset_id
                            ));
                        };
                        if asset.kind != AnimationAssetKind::Audio {
                            return Err(format!(
                                "animation clip '{}' event '{}' asset '{}' is not audio",
                                clip.id, event.id, asset_id
                            ));
                        }
                    }
                }
            }
        }

        let mut bound_objects = std::collections::HashSet::new();
        for binding in &self.imported_bindings {
            if !bound_objects.insert(binding.object_id.as_str()) {
                return Err(format!(
                    "imported animation binding for object '{}' must be unique",
                    binding.object_id
                ));
            }
            let Some(object) = object_by_id.get(binding.object_id.as_str()) else {
                return Err(format!(
                    "imported animation binding references missing object '{}'",
                    binding.object_id
                ));
            };
            if object.kind != SnapshotObjectKind::Character {
                return Err(format!(
                    "imported animation binding object '{}' must be a character",
                    binding.object_id
                ));
            }
            let Some(asset) = assets.get(binding.asset_id.as_str()) else {
                return Err(format!(
                    "imported animation binding for object '{}' references missing asset '{}'",
                    binding.object_id, binding.asset_id
                ));
            };
            if asset.kind != AnimationAssetKind::Model {
                return Err(format!(
                    "imported animation binding asset '{}' is not a model",
                    binding.asset_id
                ));
            }
            if binding.clips.is_empty() {
                return Err(format!(
                    "imported animation binding for object '{}' must list at least one clip",
                    binding.object_id
                ));
            }
            let mut source_clip_ids = std::collections::HashSet::new();
            let mut display_names = std::collections::HashSet::new();
            for clip in &binding.clips {
                if clip.source_clip_id.trim().is_empty()
                    || !source_clip_ids.insert(clip.source_clip_id.as_str())
                    || clip.display_name.trim().is_empty()
                    || !display_names.insert(clip.display_name.as_str())
                {
                    return Err(format!(
                        "imported animation clips for object '{}' need unique non-empty source ids and display names",
                        binding.object_id
                    ));
                }
            }
        }
        Ok(())
    }
}

pub fn validate_relative_path(label: &str, value: &str) -> Result<(), String> {
    let path = Path::new(value);
    if value.trim().is_empty() || path.is_absolute() {
        return Err(format!(
            "snapshot {label} must be a non-empty relative path"
        ));
    }
    if path.components().any(|component| {
        matches!(
            component,
            std::path::Component::ParentDir
                | std::path::Component::RootDir
                | std::path::Component::Prefix(_)
        )
    }) {
        return Err(format!(
            "snapshot {label} must not escape its project directory"
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{
        AnimationEventKindSnapshot, ProjectSnapshot, SNAPSHOT_SCHEMA_VERSION, SceneSnapshot,
        SnapshotObject, SnapshotObjectKind,
    };

    fn snapshot() -> ProjectSnapshot {
        ProjectSnapshot {
            schema_version: SNAPSHOT_SCHEMA_VERSION,
            scene: SceneSnapshot {
                name: "Stage 0 Room".to_owned(),
                objects: vec![
                    SnapshotObject {
                        id: "floor".to_owned(),
                        kind: SnapshotObjectKind::Floor,
                        position: [0.0, -0.03, 0.0],
                        scale: [1.0; 3],
                    },
                    SnapshotObject {
                        id: "wall".to_owned(),
                        kind: SnapshotObjectKind::Wall,
                        position: [0.0, 1.55, -3.0],
                        scale: [1.0; 3],
                    },
                    SnapshotObject {
                        id: "door".to_owned(),
                        kind: SnapshotObjectKind::Door,
                        position: [1.35, 1.02, -2.82],
                        scale: [1.0; 3],
                    },
                    SnapshotObject {
                        id: "character".to_owned(),
                        kind: SnapshotObjectKind::Character,
                        position: [-1.35, 1.12, 1.8],
                        scale: [1.0; 3],
                    },
                    SnapshotObject {
                        id: "prop".to_owned(),
                        kind: SnapshotObjectKind::Prop,
                        position: [0.0, 0.72, 0.0],
                        scale: [1.35; 3],
                    },
                ],
                prop_model: "Quasar/viewport-prop.glb".to_owned(),
                ambience_audio: "Quasar/audio/room-ambience.wav".to_owned(),
                door_audio: "Quasar/audio/door-latch.wav".to_owned(),
                door_script: "scripts/door.lua".to_owned(),
                animation: None,
            },
            assets_directory: "assets".to_owned(),
        }
    }

    #[test]
    fn snapshot_json_roundtrips() {
        let expected = snapshot();
        let encoded = serde_json::to_string(&expected).expect("snapshot encodes");
        let decoded: ProjectSnapshot = serde_json::from_str(&encoded).expect("snapshot decodes");

        assert_eq!(decoded, expected);
        decoded.validate().expect("snapshot structure is valid");
    }

    #[test]
    fn legacy_stage0_fixture_without_animation_remains_valid() {
        let source = include_str!("../../../tests/fixtures/stage0-player/quasar.snapshot.json");
        let decoded: ProjectSnapshot =
            serde_json::from_str(source).expect("legacy snapshot decodes");

        assert_eq!(decoded.schema_version, SNAPSHOT_SCHEMA_VERSION);
        assert!(decoded.scene.animation.is_none());
        decoded.validate().expect("legacy snapshot remains valid");
    }

    #[test]
    fn stage0_animation_fixture_roundtrips_and_validates_references() {
        let source = include_str!("../../../tests/fixtures/stage0-animation/quasar.snapshot.json");
        let decoded: ProjectSnapshot =
            serde_json::from_str(source).expect("animation snapshot decodes");

        decoded.validate().expect("animation snapshot is valid");
        let animation = decoded
            .scene
            .animation
            .expect("animation extension is present");
        assert_eq!(animation.object_clips[0].id, "door_open");
        assert_eq!(
            animation.imported_bindings[0].clips[1].source_clip_id,
            "Walking"
        );
        assert_eq!(animation.imported_bindings[0].clips[1].display_name, "Walk");
    }

    #[test]
    fn animation_snapshot_rejects_missing_targets_assets_and_bad_key_times() {
        let source = include_str!("../../../tests/fixtures/stage0-animation/quasar.snapshot.json");
        let mut document: ProjectSnapshot = serde_json::from_str(source).expect("fixture decodes");

        document.scene.animation.as_mut().unwrap().object_clips[0].object_id =
            "removed-door".to_owned();
        assert!(document.validate().unwrap_err().contains("missing object"));

        let mut document: ProjectSnapshot = serde_json::from_str(source).expect("fixture decodes");
        document.scene.animation.as_mut().unwrap().object_clips[0].events[0].kind =
            AnimationEventKindSnapshot::PlaySound {
                asset_id: "missing-audio".to_owned(),
            };
        assert!(document.validate().unwrap_err().contains("missing asset"));

        let mut document: ProjectSnapshot = serde_json::from_str(source).expect("fixture decodes");
        let keys = &mut document.scene.animation.as_mut().unwrap().object_clips[0].tracks[0].keys;
        keys[1].time_seconds = keys[0].time_seconds;
        assert!(
            document
                .validate()
                .unwrap_err()
                .contains("strictly increasing")
        );
    }

    #[test]
    fn unknown_schema_version_is_rejected() {
        let mut document = snapshot();
        document.schema_version += 1;

        assert!(
            document
                .validate()
                .unwrap_err()
                .contains("unsupported snapshot schema")
        );
    }

    #[test]
    fn paths_that_escape_the_project_are_rejected() {
        let mut document = snapshot();
        document.scene.door_script = "../../outside.lua".to_owned();

        assert!(document.validate().unwrap_err().contains("must not escape"));
    }

    #[test]
    fn empty_scene_name_is_rejected() {
        let mut document = snapshot();
        document.scene.name = "  ".to_owned();

        assert!(document.validate().unwrap_err().contains("scene name"));
    }

    #[test]
    fn duplicate_ids_and_invalid_transforms_are_rejected() {
        let mut document = snapshot();
        document.scene.objects[1].id = document.scene.objects[0].id.clone();
        assert!(document.validate().unwrap_err().contains("unique"));

        let mut document = snapshot();
        document.scene.objects[0].position[0] = f32::NAN;
        assert!(document.validate().unwrap_err().contains("non-finite"));
    }
}
