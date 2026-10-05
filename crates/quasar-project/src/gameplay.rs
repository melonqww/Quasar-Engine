//! Versioned, engine-independent gameplay component payloads.

use serde::{Serialize, de::DeserializeOwned};

use crate::document::{ComponentDocument, ObjectId};

pub const CAMERA_COMPONENT_TYPE_ID: &str = "quasar.camera";
pub const CHARACTER_CONTROLLER_COMPONENT_TYPE_ID: &str = "quasar.character_controller";
pub const COLLIDER_COMPONENT_TYPE_ID: &str = "quasar.collider";
pub const RIGID_BODY_COMPONENT_TYPE_ID: &str = "quasar.rigid_body";
pub const AUDIO_SOURCE_COMPONENT_TYPE_ID: &str = "quasar.audio_source";
pub const AUDIO_LISTENER_COMPONENT_TYPE_ID: &str = "quasar.audio_listener";
pub const DOOR_COMPONENT_TYPE_ID: &str = "quasar.door";
pub const GAMEPLAY_COMPONENT_SCHEMA_VERSION: u32 = 1;

/// Implemented by known, typed component envelopes. Unknown envelopes remain opaque.
pub trait TypedComponent: Serialize + DeserializeOwned + Sized {
    const TYPE_ID: &'static str;
    fn validate(&self) -> Result<(), String>;

    fn into_document(self) -> ComponentDocument {
        ComponentDocument {
            type_id: Self::TYPE_ID.to_owned(),
            schema_version: GAMEPLAY_COMPONENT_SCHEMA_VERSION,
            data: serde_json::to_value(self).expect("typed component is serializable"),
        }
    }

    fn from_components(components: &[ComponentDocument]) -> Result<Option<Self>, String> {
        let mut found = None;
        for component in components.iter().filter(|c| c.type_id == Self::TYPE_ID) {
            if component.schema_version != GAMEPLAY_COMPONENT_SCHEMA_VERSION {
                return Err(format!(
                    "unsupported {} component version {}; supported version is {}",
                    Self::TYPE_ID,
                    component.schema_version,
                    GAMEPLAY_COMPONENT_SCHEMA_VERSION
                ));
            }
            if found.is_some() {
                return Err(format!(
                    "object contains more than one {} component",
                    Self::TYPE_ID
                ));
            }
            let value: Self = serde_json::from_value(component.data.clone())
                .map_err(|error| format!("invalid {} component: {error}", Self::TYPE_ID))?;
            value.validate()?;
            found = Some(value);
        }
        Ok(found)
    }
}

macro_rules! typed_component {
    ($type:ty, $id:ident, $validate:expr) => {
        impl TypedComponent for $type {
            const TYPE_ID: &'static str = $id;
            fn validate(&self) -> Result<(), String> {
                ($validate)(self)
            }
        }
    };
}

#[derive(Clone, Copy, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CameraComponent {
    pub enabled: bool,
    /// Vertical field of view in degrees.
    pub vertical_fov_degrees: f32,
}

impl Default for CameraComponent {
    fn default() -> Self {
        Self {
            enabled: true,
            vertical_fov_degrees: 70.0,
        }
    }
}

typed_component!(
    CameraComponent,
    CAMERA_COMPONENT_TYPE_ID,
    |v: &CameraComponent| {
        if !v.vertical_fov_degrees.is_finite() || !(1.0..=179.0).contains(&v.vertical_fov_degrees) {
            return Err("camera vertical_fov_degrees must be finite and in 1..=179".into());
        }
        Ok(())
    }
);

#[derive(Clone, Copy, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CharacterControllerComponent {
    pub enabled: bool,
    pub camera_object: ObjectId,
    pub walk_speed: f32,
    pub jump_speed: f32,
    pub gravity: f32,
    pub radius: f32,
    /// Full capsule height, not Rapier's half-height.
    pub height: f32,
    pub eye_height: f32,
    pub max_slope_degrees: f32,
    pub step_height: f32,
    pub ground_snap: f32,
    pub mouse_sensitivity: f32,
}

impl CharacterControllerComponent {
    pub fn new(camera_object: ObjectId) -> Self {
        Self {
            enabled: true,
            camera_object,
            walk_speed: 4.0,
            jump_speed: 5.0,
            gravity: 9.81,
            radius: 0.3,
            height: 1.8,
            eye_height: 1.6,
            max_slope_degrees: 45.0,
            step_height: 0.25,
            ground_snap: 0.12,
            mouse_sensitivity: 0.0025,
        }
    }
}

typed_component!(
    CharacterControllerComponent,
    CHARACTER_CONTROLLER_COMPONENT_TYPE_ID,
    |v: &CharacterControllerComponent| {
        let non_negative = [
            v.walk_speed,
            v.jump_speed,
            v.gravity,
            v.step_height,
            v.ground_snap,
        ];
        if non_negative.iter().any(|n| !n.is_finite() || *n < 0.0) {
            return Err(
                "controller speed, gravity, step and snap values must be finite and non-negative"
                    .into(),
            );
        }
        if !v.radius.is_finite()
            || v.radius <= 0.0
            || !v.height.is_finite()
            || v.height < 2.0 * v.radius
        {
            return Err("controller capsule requires radius > 0 and height >= 2 * radius".into());
        }
        if !v.eye_height.is_finite() || v.eye_height < 0.0 || v.eye_height > v.height {
            return Err("controller eye_height must be within the capsule height".into());
        }
        if !v.max_slope_degrees.is_finite() || !(0.0..90.0).contains(&v.max_slope_degrees) {
            return Err("controller max_slope_degrees must be finite and in [0, 90)".into());
        }
        if !v.mouse_sensitivity.is_finite() || v.mouse_sensitivity <= 0.0 {
            return Err("controller mouse_sensitivity must be finite and positive".into());
        }
        Ok(())
    }
);

#[derive(Clone, Copy, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "shape", rename_all = "snake_case", deny_unknown_fields)]
pub enum ColliderShape {
    Box { size: [f32; 3] },
    Capsule { radius: f32, height: f32 },
}

#[derive(Clone, Copy, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ColliderComponent {
    pub enabled: bool,
    pub center: [f32; 3],
    pub shape: ColliderShape,
}

typed_component!(
    ColliderComponent,
    COLLIDER_COMPONENT_TYPE_ID,
    |v: &ColliderComponent| {
        if v.center.iter().any(|n| !n.is_finite()) {
            return Err("collider center must contain only finite values".into());
        }
        match v.shape {
            ColliderShape::Box { size } if size.iter().all(|n| n.is_finite() && *n > 0.0) => Ok(()),
            ColliderShape::Capsule { radius, height }
                if radius.is_finite()
                    && radius > 0.0
                    && height.is_finite()
                    && height >= 2.0 * radius =>
            {
                Ok(())
            }
            ColliderShape::Box { .. } => {
                Err("box collider size must contain three finite positive values".into())
            }
            ColliderShape::Capsule { .. } => {
                Err("capsule collider requires radius > 0 and height >= 2 * radius".into())
            }
        }
    }
);

#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RigidBodyKind {
    Static,
    Kinematic,
    Dynamic,
}

#[derive(Clone, Copy, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RigidBodyComponent {
    pub enabled: bool,
    pub kind: RigidBodyKind,
    pub mass: f32,
    pub linear_damping: f32,
}

impl Default for RigidBodyComponent {
    fn default() -> Self {
        Self {
            enabled: true,
            kind: RigidBodyKind::Static,
            mass: 1.0,
            linear_damping: 0.0,
        }
    }
}

typed_component!(
    RigidBodyComponent,
    RIGID_BODY_COMPONENT_TYPE_ID,
    |v: &RigidBodyComponent| {
        if !v.mass.is_finite()
            || v.mass <= 0.0
            || !v.linear_damping.is_finite()
            || v.linear_damping < 0.0
        {
            return Err("rigid body mass must be positive and damping must be non-negative".into());
        }
        Ok(())
    }
);

/// Runtime-openable hinged door settings. The authored transform remains the closed pose.
#[derive(Clone, Copy, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DoorComponent {
    pub enabled: bool,
    pub open_angle_degrees: f32,
    pub open_duration_seconds: f32,
}

impl Default for DoorComponent {
    fn default() -> Self {
        Self {
            enabled: true,
            open_angle_degrees: 90.0,
            open_duration_seconds: 0.75,
        }
    }
}

typed_component!(
    DoorComponent,
    DOOR_COMPONENT_TYPE_ID,
    |v: &DoorComponent| {
        if !v.open_angle_degrees.is_finite() || !(1.0..=180.0).contains(&v.open_angle_degrees) {
            return Err("door open_angle_degrees must be finite and in 1..=180".into());
        }
        if !v.open_duration_seconds.is_finite() || !(0.05..=10.0).contains(&v.open_duration_seconds)
        {
            return Err("door open_duration_seconds must be finite and in 0.05..=10".into());
        }
        Ok(())
    }
);

#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AudioCategory {
    Music,
    Sfx,
}

#[derive(Clone, Copy, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AudioSourceComponent {
    pub enabled: bool,
    pub asset_id: crate::assets::AssetId,
    pub volume: f32,
    pub looping: bool,
    pub autoplay: bool,
    pub spatial: bool,
    pub category: AudioCategory,
}

typed_component!(
    AudioSourceComponent,
    AUDIO_SOURCE_COMPONENT_TYPE_ID,
    |v: &AudioSourceComponent| {
        if !v.volume.is_finite() || !(0.0..=1.0).contains(&v.volume) {
            return Err("audio source volume must be finite and in 0..=1".into());
        }
        Ok(())
    }
);

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AudioListenerComponent {
    pub enabled: bool,
}

typed_component!(
    AudioListenerComponent,
    AUDIO_LISTENER_COMPONENT_TYPE_ID,
    |_v: &AudioListenerComponent| Ok(())
);

#[derive(Clone, Copy, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProjectAudioSettings {
    pub master: f32,
    pub music: f32,
    pub sfx: f32,
}

impl Default for ProjectAudioSettings {
    fn default() -> Self {
        Self {
            master: 1.0,
            music: 1.0,
            sfx: 1.0,
        }
    }
}

impl ProjectAudioSettings {
    pub fn validate(&self) -> Result<(), String> {
        if [self.master, self.music, self.sfx]
            .iter()
            .any(|n| !n.is_finite() || !(0.0..=1.0).contains(n))
        {
            return Err("project audio gains must be finite and in 0..=1".into());
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::document::SceneObjectDocument;

    #[test]
    fn typed_controller_roundtrips_through_component_envelope() {
        let camera = SceneObjectDocument::new("Camera", None);
        let controller = CharacterControllerComponent::new(camera.id);
        assert!(controller.validate().is_ok());
        let encoded = controller.into_document();
        assert_eq!(
            CharacterControllerComponent::from_components(&[encoded]).unwrap(),
            Some(controller)
        );
    }

    #[test]
    fn controller_rejects_invalid_capsule_and_non_finite_values() {
        let mut value = CharacterControllerComponent::new(ObjectId::new());
        value.radius = f32::NAN;
        assert!(value.validate().unwrap_err().contains("capsule"));
        value.radius = 0.3;
        value.height = 0.5;
        assert!(value.validate().unwrap_err().contains("height"));
    }

    #[test]
    fn collider_rejects_zero_size_and_audio_has_finite_gains() {
        let collider = ColliderComponent {
            enabled: true,
            center: [0.0; 3],
            shape: ColliderShape::Box {
                size: [1.0, 0.0, 1.0],
            },
        };
        assert!(collider.validate().is_err());
        assert!(
            ProjectAudioSettings {
                master: f32::INFINITY,
                ..ProjectAudioSettings::default()
            }
            .validate()
            .is_err()
        );
    }

    #[test]
    fn door_component_roundtrips_and_rejects_non_finite_motion_settings() {
        let door = DoorComponent::default();
        let encoded = door.into_document();
        assert_eq!(
            DoorComponent::from_components(&[encoded]).unwrap(),
            Some(door)
        );

        let mut invalid = door;
        invalid.open_duration_seconds = f32::NAN;
        assert!(invalid.validate().unwrap_err().contains("duration"));
        invalid = door;
        invalid.open_angle_degrees = 0.0;
        assert!(invalid.validate().unwrap_err().contains("angle"));
    }
}
