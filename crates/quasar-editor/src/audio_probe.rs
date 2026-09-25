//! Reproducible loop, spatial effect, and stop/restart probe for Bevy Audio.

use bevy::{audio::Volume, prelude::*};

pub struct AudioProbePlugin;

impl Plugin for AudioProbePlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<AudioProbeState>()
            .init_resource::<AudioProbeControls>()
            .add_systems(Startup, start_ambient_loop)
            .add_systems(Update, handle_audio_probe_input);
    }
}

#[derive(Resource, Default)]
pub(super) struct AudioProbeState {
    pub(super) ambience_active: bool,
}

#[derive(Resource, Default)]
pub(super) struct AudioProbeControls {
    pub(super) pending: Option<AudioProbeCommand>,
}

#[derive(Clone, Copy)]
pub(super) enum AudioProbeCommand {
    Restart,
    Stop,
}

#[derive(Component)]
pub(super) struct ProbeSessionAudio;

#[derive(Component)]
struct ProbeAmbientAudio;

type ProbeSessionAudioQuery<'w, 's> = Query<
    'w,
    's,
    (
        Entity,
        Option<&'static AudioSink>,
        Option<&'static SpatialAudioSink>,
    ),
    With<ProbeSessionAudio>,
>;

fn start_ambient_loop(
    mut commands: Commands,
    asset_server: Res<AssetServer>,
    mut state: ResMut<AudioProbeState>,
) {
    spawn_ambient_loop(&mut commands, &asset_server);
    state.ambience_active = true;
}

fn spawn_ambient_loop(commands: &mut Commands, asset_server: &AssetServer) {
    commands.spawn((
        AudioPlayer::new(asset_server.load("Quasar/audio/room-ambience.wav")),
        PlaybackSettings::LOOP.with_volume(Volume::Linear(0.7)),
        ProbeSessionAudio,
        ProbeAmbientAudio,
        Name::new("Probe Room Ambience"),
    ));
}

fn handle_audio_probe_input(
    keyboard: Res<ButtonInput<KeyCode>>,
    asset_server: Res<AssetServer>,
    mut commands: Commands,
    mut state: ResMut<AudioProbeState>,
    mut controls: ResMut<AudioProbeControls>,
    mut session_audio: ProbeSessionAudioQuery,
    mut ambient_audio: Query<(Entity, Option<&AudioSink>), With<ProbeAmbientAudio>>,
) {
    let command = controls.pending.take().or_else(|| {
        if keyboard.just_pressed(KeyCode::F10) {
            Some(AudioProbeCommand::Stop)
        } else if keyboard.just_pressed(KeyCode::F9) {
            Some(AudioProbeCommand::Restart)
        } else {
            None
        }
    });

    match command {
        Some(AudioProbeCommand::Stop) => {
            for (entity, sink, spatial_sink) in &mut session_audio {
                if let Some(sink) = sink {
                    sink.stop();
                }
                if let Some(sink) = spatial_sink {
                    sink.stop();
                }
                commands.entity(entity).despawn();
            }
            state.ambience_active = false;
        }
        Some(AudioProbeCommand::Restart) => {
            for (entity, sink) in &mut ambient_audio {
                if let Some(sink) = sink {
                    sink.stop();
                }
                commands.entity(entity).despawn();
            }
            spawn_ambient_loop(&mut commands, &asset_server);
            state.ambience_active = true;
        }
        None => {}
    }
}

#[cfg(test)]
mod tests {
    use super::{AudioProbePlugin, AudioProbeState, ProbeSessionAudio};
    use crate::viewport_probe::ViewportInputFocus;
    use bevy::{
        asset::{AssetApp, AssetPlugin},
        audio::AudioSource,
        input::ButtonInput,
        prelude::{App, KeyCode, MinimalPlugins, With},
    };

    fn session_audio_count(app: &mut App) -> usize {
        let world = app.world_mut();
        let mut query = world.query_filtered::<(), With<ProbeSessionAudio>>();
        query.iter(world).count()
    }

    #[test]
    fn generated_wav_fixtures_are_mono_44100_hz_pcm16() {
        for bytes in [
            include_bytes!("../assets/Quasar/audio/room-ambience.wav").as_slice(),
            include_bytes!("../assets/Quasar/audio/door-latch.wav").as_slice(),
        ] {
            assert_eq!(&bytes[0..4], b"RIFF");
            assert_eq!(&bytes[8..12], b"WAVE");
            assert_eq!(&bytes[20..22], &1_u16.to_le_bytes());
            assert_eq!(&bytes[22..24], &1_u16.to_le_bytes());
            assert_eq!(&bytes[24..28], &44_100_u32.to_le_bytes());
            assert_eq!(&bytes[34..36], &16_u16.to_le_bytes());
        }
    }

    #[test]
    fn stop_despawns_loop_then_restart_creates_one_loop() {
        let mut app = App::new();
        app.add_plugins((MinimalPlugins, AssetPlugin::default(), AudioProbePlugin));
        app.init_asset::<AudioSource>();
        app.init_resource::<ButtonInput<KeyCode>>();
        app.insert_resource(ViewportInputFocus(true));
        app.finish();
        app.cleanup();
        app.update();

        assert_eq!(session_audio_count(&mut app), 1);
        assert!(app.world().resource::<AudioProbeState>().ambience_active);

        app.world_mut()
            .resource_mut::<ButtonInput<KeyCode>>()
            .press(KeyCode::F10);
        app.update();
        assert_eq!(session_audio_count(&mut app), 0);
        assert!(!app.world().resource::<AudioProbeState>().ambience_active);

        app.world_mut()
            .resource_mut::<ButtonInput<KeyCode>>()
            .clear();
        app.world_mut()
            .resource_mut::<ButtonInput<KeyCode>>()
            .press(KeyCode::F9);
        app.update();
        assert_eq!(session_audio_count(&mut app), 1);
        assert!(app.world().resource::<AudioProbeState>().ambience_active);
    }
}
