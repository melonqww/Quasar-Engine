//! Small Lua-driven door interaction for the Stage 0 probe.

use std::{fs, path::Path};

use bevy::{audio::Volume, input::InputSystems, prelude::*};
use bevy_rapier3d::prelude::Collider;
use quasar_runtime::gameplay::{GameplayCommand, run_door_interaction};

use super::{audio_probe::ProbeSessionAudio, viewport_probe::ViewportInputFocus};

const DOOR_SCRIPT_PATH: &str = "../../essentials/scripts/door.lua";

pub struct GameplayProbePlugin;

impl Plugin for GameplayProbePlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<DoorProbeState>()
            .add_systems(Startup, load_door_script)
            .add_systems(PreUpdate, interact_with_lua_door.after(InputSystems));
    }
}

#[derive(Component)]
pub(super) struct ProbeDoor;

#[derive(Resource, Default)]
struct DoorProbeState {
    opened: bool,
}

#[derive(Resource)]
struct DoorScriptSource(String);

fn load_door_script(mut commands: Commands) {
    let script_path = Path::new(env!("CARGO_MANIFEST_DIR")).join(DOOR_SCRIPT_PATH);
    match fs::read_to_string(&script_path) {
        Ok(source) => {
            info!(path = %script_path.display(), "Loaded Lua door probe");
            commands.insert_resource(DoorScriptSource(source));
        }
        Err(error) => {
            error!(path = %script_path.display(), %error, "Failed to load Lua door probe");
            commands.insert_resource(DoorScriptSource(String::new()));
        }
    }
}

fn interact_with_lua_door(
    keyboard: Res<ButtonInput<KeyCode>>,
    focus: Res<ViewportInputFocus>,
    script: Res<DoorScriptSource>,
    asset_server: Res<AssetServer>,
    mut commands: Commands,
    mut state: ResMut<DoorProbeState>,
    mut doors: Query<(Entity, &mut Transform), With<ProbeDoor>>,
) {
    if !focus.0 || !keyboard.just_pressed(KeyCode::KeyE) || state.opened {
        return;
    }

    let actions = match run_door_interaction(&script.0) {
        Ok(actions) => actions,
        Err(error) => {
            error!(%error, "Lua door interaction failed; pending actions discarded");
            return;
        }
    };

    for action in actions {
        match action {
            GameplayCommand::OpenDoor => {
                if let Ok((entity, mut transform)) = doors.single_mut() {
                    transform.rotate_y(1.25);
                    commands.entity(entity).remove::<Collider>();
                    state.opened = true;
                    info!("Lua interaction opened the probe door");
                }
            }
            GameplayCommand::PlayDoorSound => {
                commands.spawn((
                    AudioPlayer::new(asset_server.load("Quasar/audio/door-latch.wav")),
                    PlaybackSettings::DESPAWN
                        .with_volume(Volume::Linear(0.9))
                        .with_spatial(true),
                    Transform::from_xyz(1.35, 1.0, -2.82),
                    ProbeSessionAudio,
                    Name::new("Lua Door Sound"),
                ));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{DoorProbeState, GameplayProbePlugin, ProbeDoor};
    use crate::{
        audio_probe::{AudioProbePlugin, ProbeSessionAudio},
        viewport_probe::ViewportInputFocus,
    };
    use bevy::{
        asset::AssetApp,
        asset::AssetPlugin,
        audio::AudioSource,
        input::ButtonInput,
        prelude::{App, KeyCode, MinimalPlugins, Transform, With},
    };
    use bevy_rapier3d::prelude::Collider as RapierCollider;

    fn session_audio_count(app: &mut App) -> usize {
        let world = app.world_mut();
        let mut query = world.query_filtered::<(), With<ProbeSessionAudio>>();
        query.iter(world).count()
    }

    #[test]
    fn interact_runs_project_lua_opens_door_and_spawns_spatial_sound() {
        let mut app = App::new();
        app.add_plugins((
            MinimalPlugins,
            AssetPlugin::default(),
            AudioProbePlugin,
            GameplayProbePlugin,
        ));
        app.init_asset::<AudioSource>();
        app.init_resource::<ButtonInput<KeyCode>>();
        app.insert_resource(ViewportInputFocus(true));
        let door = app
            .world_mut()
            .spawn((
                ProbeDoor,
                Transform::default(),
                RapierCollider::cuboid(0.5, 1.0, 0.1),
            ))
            .id();
        app.finish();
        app.cleanup();
        app.update();

        app.world_mut()
            .resource_mut::<ButtonInput<KeyCode>>()
            .press(KeyCode::KeyE);
        app.update();

        let door_entity = app.world().entity(door);
        assert!(door_entity.get::<RapierCollider>().is_none());
        assert_ne!(
            door_entity.get::<Transform>().unwrap().rotation,
            bevy::math::Quat::IDENTITY
        );
        assert!(app.world().resource::<DoorProbeState>().opened);
        assert_eq!(session_audio_count(&mut app), 2, "ambient + Lua effect");
    }

    #[test]
    fn stop_removes_the_lua_spawned_effect_together_with_session_audio() {
        let mut app = App::new();
        app.add_plugins((
            MinimalPlugins,
            AssetPlugin::default(),
            AudioProbePlugin,
            GameplayProbePlugin,
        ));
        app.init_asset::<AudioSource>();
        app.init_resource::<ButtonInput<KeyCode>>();
        app.insert_resource(ViewportInputFocus(true));
        app.world_mut().spawn((
            ProbeDoor,
            Transform::default(),
            RapierCollider::cuboid(0.5, 1.0, 0.1),
        ));
        app.finish();
        app.cleanup();
        app.update();

        app.world_mut()
            .resource_mut::<ButtonInput<KeyCode>>()
            .press(KeyCode::KeyE);
        app.update();
        assert_eq!(session_audio_count(&mut app), 2);

        app.world_mut()
            .resource_mut::<ButtonInput<KeyCode>>()
            .press(KeyCode::F10);
        app.update();
        assert_eq!(session_audio_count(&mut app), 0);
    }
}
