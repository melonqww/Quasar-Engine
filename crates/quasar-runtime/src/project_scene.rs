//! Adapters from authored project components to the shared Bevy/Rapier runtime.

use bevy::prelude::{Quat, Transform, Vec3};
use bevy_rapier3d::prelude::{
    CharacterAutostep, CharacterLength, Collider, KinematicCharacterController, RigidBody,
};
use quasar_project::{
    document::TransformDocument,
    gameplay::{CharacterControllerComponent, ColliderComponent, ColliderShape, RigidBodyKind},
};

pub fn transform_from_document(value: &TransformDocument) -> Transform {
    Transform {
        translation: Vec3::from_array(value.translation),
        rotation: Quat::from_xyzw(
            value.rotation_xyzw[0],
            value.rotation_xyzw[1],
            value.rotation_xyzw[2],
            value.rotation_xyzw[3],
        ),
        scale: Vec3::from_array(value.scale),
    }
}

/// Converts authored full height to Rapier's half-height cylinder plus capsule radius.
pub fn capsule_half_height(radius: f32, full_height: f32) -> f32 {
    ((full_height - 2.0 * radius) * 0.5).max(0.0)
}

pub fn controller_collider(component: &CharacterControllerComponent) -> Collider {
    Collider::capsule_y(
        capsule_half_height(component.radius, component.height),
        component.radius,
    )
}

pub fn collider_from_document(component: &ColliderComponent) -> Collider {
    match component.shape {
        ColliderShape::Box { size } => {
            Collider::cuboid(size[0] * 0.5, size[1] * 0.5, size[2] * 0.5)
        }
        ColliderShape::Capsule { radius, height } => {
            Collider::capsule_y(capsule_half_height(radius, height), radius)
        }
    }
}

pub fn character_controller(
    component: &CharacterControllerComponent,
) -> KinematicCharacterController {
    KinematicCharacterController {
        offset: CharacterLength::Absolute(0.01),
        snap_to_ground: (component.ground_snap > 0.0)
            .then_some(CharacterLength::Absolute(component.ground_snap)),
        autostep: (component.step_height > 0.0).then_some(CharacterAutostep {
            max_height: CharacterLength::Absolute(component.step_height),
            min_width: CharacterLength::Absolute(component.radius * 0.66),
            include_dynamic_bodies: false,
        }),
        max_slope_climb_angle: component.max_slope_degrees.to_radians(),
        min_slope_slide_angle: (component.max_slope_degrees - 15.0).max(0.0).to_radians(),
        ..Default::default()
    }
}

pub fn rigid_body(kind: RigidBodyKind) -> RigidBody {
    match kind {
        RigidBodyKind::Static => RigidBody::Fixed,
        RigidBodyKind::Kinematic => RigidBody::KinematicPositionBased,
        RigidBodyKind::Dynamic => RigidBody::Dynamic,
    }
}

#[cfg(test)]
mod tests {
    use super::capsule_half_height;

    #[test]
    fn authored_capsule_height_is_converted_once_to_rapier_half_height() {
        assert!((capsule_half_height(0.3, 1.8) - 0.6).abs() < f32::EPSILON);
        assert_eq!(capsule_half_height(0.4, 0.8), 0.0);
    }
}
