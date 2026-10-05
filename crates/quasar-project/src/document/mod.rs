//! Stable, engine-independent authoring documents for Quasar projects.

use std::path::Path;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{gameplay::ProjectAudioSettings, persistence, validation};

pub const PROJECT_DOCUMENT_VERSION: u32 = 1;
pub const SCENE_DOCUMENT_VERSION: u32 = 1;

macro_rules! stable_id {
    ($name:ident) => {
        #[derive(
            Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize, Deserialize,
        )]
        #[serde(transparent)]
        pub struct $name(pub Uuid);

        impl $name {
            pub fn new() -> Self {
                Self(Uuid::new_v4())
            }
        }

        impl Default for $name {
            fn default() -> Self {
                Self::new()
            }
        }
    };
}

stable_id!(ProjectId);
stable_id!(SceneId);
stable_id!(ObjectId);

/// Complete authored project state. Session revision and editor UI state are not persisted here.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProjectDocument {
    pub format_version: u32,
    pub project_id: ProjectId,
    pub name: String,
    pub active_scene_id: SceneId,
    pub scenes: Vec<SceneDocument>,
    /// Project-wide authoring settings; absent in older version-1 documents means defaults.
    #[serde(default, skip_serializing_if = "is_default_audio_settings")]
    pub audio: ProjectAudioSettings,
}

fn is_default_audio_settings(settings: &ProjectAudioSettings) -> bool {
    *settings == ProjectAudioSettings::default()
}

impl ProjectDocument {
    pub fn new(name: impl Into<String>, scene: SceneDocument) -> Self {
        let active_scene_id = scene.id;
        Self {
            format_version: PROJECT_DOCUMENT_VERSION,
            project_id: ProjectId::new(),
            name: name.into(),
            active_scene_id,
            scenes: vec![scene],
            audio: ProjectAudioSettings::default(),
        }
    }

    pub fn validate(&self) -> Result<(), String> {
        validation::validate_project(self)
    }

    pub fn load(path: &Path) -> Result<Self, String> {
        let document: Self = persistence::read_json(path, "project document")?;
        document.validate()?;
        Ok(document)
    }

    pub fn save(&self, path: &Path) -> Result<(), String> {
        self.validate()?;
        let encoded = serde_json::to_vec_pretty(self).map_err(|error| {
            format!("cannot encode project document {}: {error}", path.display())
        })?;
        persistence::write_json_atomically(path, &encoded, "project document")
    }
}

/// A scene is authored data; runtime entities and handles are generated from it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SceneDocument {
    pub schema_version: u32,
    pub id: SceneId,
    pub name: String,
    pub objects: Vec<SceneObjectDocument>,
}

impl SceneDocument {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            schema_version: SCENE_DOCUMENT_VERSION,
            id: SceneId::new(),
            name: name.into(),
            objects: Vec::new(),
        }
    }
}

/// Object identity is stable across saves; `parent_id` always refers within this scene.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SceneObjectDocument {
    pub id: ObjectId,
    pub name: String,
    pub parent_id: Option<ObjectId>,
    pub local_transform: TransformDocument,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub components: Vec<ComponentDocument>,
}

impl SceneObjectDocument {
    pub fn new(name: impl Into<String>, parent_id: Option<ObjectId>) -> Self {
        Self {
            id: ObjectId::new(),
            name: name.into(),
            parent_id,
            local_transform: TransformDocument::default(),
            components: Vec::new(),
        }
    }
}

/// Versioned component envelope lets future stages add typed component payloads without tying
/// documents to Bevy components. Unknown payloads remain round-trippable at this boundary.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ComponentDocument {
    pub type_id: String,
    pub schema_version: u32,
    pub data: serde_json::Value,
}

/// Local transform in meters, with quaternion components serialized as `[x, y, z, w]`.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TransformDocument {
    pub translation: [f32; 3],
    pub rotation_xyzw: [f32; 4],
    pub scale: [f32; 3],
}

impl Default for TransformDocument {
    fn default() -> Self {
        Self {
            translation: [0.0; 3],
            rotation_xyzw: [0.0, 0.0, 0.0, 1.0],
            scale: [1.0; 3],
        }
    }
}
