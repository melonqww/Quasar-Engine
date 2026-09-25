//! Small, input-agnostic character motor layered over Rapier's kinematic controller.

use bevy::prelude::*;
use bevy_rapier3d::prelude::{
    KinematicCharacterController, KinematicCharacterControllerOutput, PhysicsSet,
};

pub struct CharacterControllerPlugin;

impl Plugin for CharacterControllerPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(
            FixedUpdate,
            advance_character_controllers.before(PhysicsSet::SyncBackend),
        );
    }
}

/// Per-instance tuning and vertical motor state. Distances use meters and time uses seconds.
#[derive(Component, Debug, Clone)]
pub struct CharacterController {
    pub walk_speed: f32,
    pub jump_speed: f32,
    pub gravity: f32,
    vertical_speed: f32,
}

impl Default for CharacterController {
    fn default() -> Self {
        Self {
            walk_speed: 4.0,
            jump_speed: 5.0,
            gravity: 9.81,
            vertical_speed: 0.0,
        }
    }
}

/// Input supplied by the host (editor, Player, or tests); positive Y means forward.
#[derive(Component, Debug, Default, Clone, Copy, PartialEq)]
pub struct CharacterControllerInput {
    pub movement: Vec2,
    pub jump_pressed: bool,
}

fn advance_character_controllers(
    fixed_time: Res<Time<Fixed>>,
    mut controllers: Query<(
        &mut CharacterController,
        &mut CharacterControllerInput,
        &mut KinematicCharacterController,
        Option<&KinematicCharacterControllerOutput>,
    )>,
) {
    let delta_secs = fixed_time.delta_secs();
    for (mut motor, mut input, mut rapier_controller, output) in &mut controllers {
        let grounded = output.is_some_and(|state| state.grounded);
        rapier_controller.translation = Some(motor_step(&mut motor, *input, grounded, delta_secs));
        input.jump_pressed = false;
    }
}

fn motor_step(
    motor: &mut CharacterController,
    input: CharacterControllerInput,
    grounded: bool,
    delta_secs: f32,
) -> Vec3 {
    let delta_secs = delta_secs.max(0.0);
    let mut movement = input.movement;
    if movement.length_squared() > 1.0 {
        movement = movement.normalize();
    }

    if grounded {
        motor.vertical_speed = 0.0;
    }
    if grounded && input.jump_pressed {
        motor.vertical_speed = motor.jump_speed.max(0.0);
    }

    let vertical_delta = motor.vertical_speed * delta_secs;
    motor.vertical_speed -= motor.gravity.max(0.0) * delta_secs;

    Vec3::new(
        movement.x * motor.walk_speed.max(0.0) * delta_secs,
        vertical_delta,
        -movement.y * motor.walk_speed.max(0.0) * delta_secs,
    )
}

#[cfg(test)]
mod tests {
    use super::{CharacterController, CharacterControllerInput, motor_step};
    use bevy::{prelude::*, time::TimeUpdateStrategy};
    use bevy_rapier3d::prelude::{
        Collider, KinematicCharacterController, KinematicCharacterControllerOutput, NoUserData,
        RapierPhysicsPlugin, TimestepMode,
    };
    use std::time::Duration;

    #[test]
    fn diagonal_input_is_normalized_to_walking_speed() {
        let mut motor = CharacterController::default();
        let step = motor_step(
            &mut motor,
            CharacterControllerInput {
                movement: Vec2::ONE,
                jump_pressed: false,
            },
            true,
            0.25,
        );

        assert!((Vec2::new(step.x, -step.z).length() - 1.0).abs() < 1e-6);
    }

    #[test]
    fn gravity_accelerates_motor_downward_while_airborne() {
        let mut motor = CharacterController::default();
        let first = motor_step(&mut motor, CharacterControllerInput::default(), false, 0.1);
        let second = motor_step(&mut motor, CharacterControllerInput::default(), false, 0.1);

        assert_eq!(first.y, 0.0);
        assert!(second.y < 0.0);
    }

    #[test]
    fn jump_is_applied_only_when_grounded() {
        let input = CharacterControllerInput {
            movement: Vec2::ZERO,
            jump_pressed: true,
        };
        let mut grounded_motor = CharacterController::default();
        let jump = motor_step(&mut grounded_motor, input, true, 0.1);
        assert!(jump.y > 0.0);

        let mut airborne_motor = CharacterController::default();
        let no_jump = motor_step(&mut airborne_motor, input, false, 0.1);
        assert_eq!(no_jump, Vec3::ZERO);
    }

    #[test]
    fn zero_or_negative_delta_does_not_move_the_motor() {
        let mut motor = CharacterController::default();
        let input = CharacterControllerInput {
            movement: Vec2::new(1.0, 0.0),
            jump_pressed: true,
        };
        assert_eq!(motor_step(&mut motor, input, true, -0.1), Vec3::ZERO);
    }

    #[test]
    fn rapier_character_stops_at_static_wall_and_remains_grounded() {
        let mut app = App::new();
        app.insert_resource(TimestepMode::Fixed {
            dt: 1.0 / 60.0,
            substeps: 1,
        });
        app.add_plugins((
            MinimalPlugins,
            bevy::transform::TransformPlugin,
            RapierPhysicsPlugin::<NoUserData>::default().in_fixed_schedule(),
            super::CharacterControllerPlugin,
        ));
        app.finish();
        app.cleanup();
        app.insert_resource(TimeUpdateStrategy::ManualDuration(Duration::from_secs_f32(
            1.0 / 60.0,
        )));
        app.world_mut().spawn((
            Transform::from_xyz(0.0, -0.13, 0.0),
            Collider::cuboid(6.0, 0.1, 6.0),
        ));
        app.world_mut().spawn((
            Transform::from_xyz(0.0, 1.5, -2.0),
            Collider::cuboid(2.0, 1.5, 0.1),
        ));
        let player = app
            .world_mut()
            .spawn((
                Transform::from_xyz(0.0, 1.16, 0.0),
                Collider::capsule_y(0.8, 0.35),
                KinematicCharacterController::default(),
                CharacterController::default(),
                CharacterControllerInput::default(),
            ))
            .id();

        for _ in 0..90 {
            app.update();
        }
        app.world_mut()
            .entity_mut(player)
            .get_mut::<CharacterControllerInput>()
            .expect("player input component exists")
            .movement = Vec2::new(0.0, 1.0);
        for _ in 0..180 {
            app.update();
        }

        let entity = app.world().entity(player);
        let z = entity
            .get::<Transform>()
            .expect("player transform exists")
            .translation
            .z;
        let grounded = entity
            .get::<KinematicCharacterControllerOutput>()
            .is_some_and(|output| output.grounded);
        assert!(z > -1.75, "capsule crossed the wall: final z={z}");
        assert!(grounded, "capsule should settle on the floor");
    }
}
