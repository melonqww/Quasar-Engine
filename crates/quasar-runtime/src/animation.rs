//! Shared Stage 0 animation playback for Editor and standalone Player.

use std::{collections::HashMap, time::Duration};

use bevy::{
    animation::{
        AnimationPlayer,
        graph::{AnimationGraph, AnimationGraphHandle, AnimationNodeIndex},
        transition::AnimationTransitions,
    },
    asset::LoadState,
    audio::Volume,
    gltf::{Gltf, GltfAssetLabel},
    prelude::*,
    world_serialization::WorldAssetRoot,
};
use bevy_rapier3d::prelude::Collider;
use quasar_project::{
    AnimatedTransformProperty, AnimationAssetKind, AnimationInterpolation, AnimationSnapshot,
    AnimationValueSnapshot, ObjectAnimationClipSnapshot,
};

/// Stable project-level identity attached to entities created from snapshot objects.
#[derive(Component, Clone, Debug, PartialEq, Eq)]
pub struct SnapshotObjectId(pub String);

/// Tag for sounds spawned by animation events so session Stop can find them.
#[derive(Component)]
pub struct AnimationSessionAudio;

/// Animation data currently loaded by the host application.
#[derive(Resource, Clone, Debug, Default)]
pub struct AnimationProbeData {
    pub animation: Option<AnimationSnapshot>,
}

/// Shared controls for the scene timeline and imported clip preview.
#[derive(Resource, Debug)]
pub struct AnimationWorkspaceState {
    pub door_clip_id: String,
    pub door_time_seconds: f32,
    pub door_playing: bool,
    pub door_completed: bool,
    pub door_opened: bool,
    pub door_request_revision: u64,
    pub door_reset_revision: u64,
    pub door_fired_events: Vec<String>,
    pub character_clip_source: String,
    pub character_preview_playing: bool,
    pub character_preview_time_seconds: f32,
    pub character_duration_seconds: f32,
    pub character_seek_revision: u64,
    pub diagnostics: Vec<String>,
}

impl Default for AnimationWorkspaceState {
    fn default() -> Self {
        Self {
            door_clip_id: "door_open".to_owned(),
            door_time_seconds: 0.0,
            door_playing: false,
            door_completed: false,
            door_opened: false,
            door_request_revision: 0,
            door_reset_revision: 0,
            door_fired_events: Vec::new(),
            character_clip_source: "Idle".to_owned(),
            character_preview_playing: true,
            character_preview_time_seconds: 0.0,
            character_duration_seconds: 3.33,
            character_seek_revision: 0,
            diagnostics: Vec::new(),
        }
    }
}

impl AnimationWorkspaceState {
    /// Starts the configured object clip from its first frame.
    pub fn play_door_clip(&mut self) {
        self.door_time_seconds = 0.0;
        self.door_playing = true;
        self.door_completed = false;
        self.door_opened = true;
        self.door_fired_events.clear();
        self.door_reset_revision = self.door_reset_revision.wrapping_add(1);
        self.door_request_revision = self.door_request_revision.wrapping_add(1);
    }

    /// Resets the object clip and restores the closed-door collision shape.
    pub fn reset_door_clip(&mut self) {
        self.door_time_seconds = 0.0;
        self.door_playing = false;
        self.door_completed = false;
        self.door_opened = false;
        self.door_fired_events.clear();
        self.door_reset_revision = self.door_reset_revision.wrapping_add(1);
    }

    /// Moves the timeline cursor without dispatching timed events.
    pub fn scrub_door_clip(&mut self, time_seconds: f32) {
        self.door_time_seconds = time_seconds.max(0.0);
        self.door_playing = false;
        self.door_completed = false;
        self.door_opened = false;
        self.door_fired_events.clear();
        self.door_reset_revision = self.door_reset_revision.wrapping_add(1);
    }

    /// Selects an imported source clip and restarts its preview cursor.
    pub fn select_character_clip(&mut self, source_clip: impl Into<String>) {
        let source_clip = source_clip.into();
        if self.character_clip_source != source_clip {
            self.character_clip_source = source_clip;
            self.character_preview_time_seconds = 0.0;
            self.character_seek_revision = self.character_seek_revision.wrapping_add(1);
        }
    }

    /// Updates the imported clip preview cursor after a user seek.
    pub fn seek_character_clip(&mut self, time_seconds: f32) {
        self.character_preview_time_seconds = time_seconds.max(0.0);
        self.character_seek_revision = self.character_seek_revision.wrapping_add(1);
    }
}

#[derive(Resource, Default)]
struct PreparedAnimationModels {
    models: HashMap<String, PreparedAnimationModel>,
    loading: HashMap<String, Handle<Gltf>>,
}

#[derive(Clone)]
struct PreparedAnimationModel {
    graph: Handle<AnimationGraph>,
    nodes: HashMap<String, AnimationNodeIndex>,
    durations: HashMap<String, f32>,
}

#[derive(Component)]
struct ImportedAnimationRig {
    object_id: String,
    asset_id: String,
    nodes: HashMap<String, AnimationNodeIndex>,
    active_source: String,
    last_seek_revision: u64,
    was_playing: bool,
}

/// Adds the same keyframe playback, event dispatch, and GLB clip handling to Editor and Player.
pub struct AnimationProbePlugin;

impl Plugin for AnimationProbePlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<AnimationProbeData>()
            .init_resource::<AnimationWorkspaceState>()
            .init_resource::<PreparedAnimationModels>()
            .add_systems(
                Update,
                (
                    prepare_imported_animation_models,
                    attach_imported_animation_rigs,
                    update_object_animation,
                    update_imported_animation_rigs,
                )
                    .chain(),
            );
    }
}

/// Samples transform tracks into an absolute local transform at `time_seconds`.
pub fn sample_object_clip(
    clip: &ObjectAnimationClipSnapshot,
    time_seconds: f32,
) -> (Option<Vec3>, Option<Quat>) {
    let time_seconds = time_seconds.clamp(0.0, clip.duration_seconds);
    let mut translation = None;
    let mut rotation = None;
    for track in &clip.tracks {
        let Some(value) = sample_track(&track.keys, track.interpolation, time_seconds) else {
            continue;
        };
        match (track.property, value) {
            (AnimatedTransformProperty::Translation, AnimationValueSnapshot::Vec3(value)) => {
                translation = Some(Vec3::from_array(value));
            }
            (AnimatedTransformProperty::Rotation, AnimationValueSnapshot::Quaternion(value)) => {
                let rotation_value = Quat::from_array(value).normalize();
                rotation = Some(rotation_value);
            }
            _ => {}
        }
    }
    (translation, rotation)
}

fn sample_track(
    keys: &[quasar_project::AnimationKeySnapshot],
    interpolation: AnimationInterpolation,
    time_seconds: f32,
) -> Option<AnimationValueSnapshot> {
    let first = keys.first()?;
    let last = keys.last()?;
    if time_seconds <= first.time_seconds {
        return Some(first.value.clone());
    }
    if time_seconds >= last.time_seconds {
        return Some(last.value.clone());
    }
    let right_index = keys.partition_point(|key| key.time_seconds < time_seconds);
    let left = keys.get(right_index.checked_sub(1)?)?;
    let right = keys.get(right_index)?;
    let span = right.time_seconds - left.time_seconds;
    if span <= 0.0 {
        return Some(right.value.clone());
    }
    let mut amount = (time_seconds - left.time_seconds) / span;
    amount = match interpolation {
        AnimationInterpolation::Step => 0.0,
        AnimationInterpolation::Linear => amount,
        AnimationInterpolation::SmoothStep => amount * amount * (3.0 - 2.0 * amount),
    };
    match (&left.value, &right.value) {
        (AnimationValueSnapshot::Vec3(a), AnimationValueSnapshot::Vec3(b)) => {
            Some(AnimationValueSnapshot::Vec3(
                (Vec3::from_array(*a).lerp(Vec3::from_array(*b), amount)).to_array(),
            ))
        }
        (AnimationValueSnapshot::Quaternion(a), AnimationValueSnapshot::Quaternion(b)) => {
            Some(AnimationValueSnapshot::Quaternion(
                Quat::from_array(*a)
                    .normalize()
                    .slerp(Quat::from_array(*b).normalize(), amount)
                    .normalize()
                    .to_array(),
            ))
        }
        _ => Some(left.value.clone()),
    }
}

fn update_object_animation(
    time: Res<Time>,
    data: Res<AnimationProbeData>,
    asset_server: Res<AssetServer>,
    mut workspace: ResMut<AnimationWorkspaceState>,
    mut commands: Commands,
    mut last_reset_revision: Local<u64>,
    mut doors: Query<(Entity, &SnapshotObjectId, &mut Transform)>,
) {
    let Some(animation) = data.animation.as_ref() else {
        return;
    };
    let Some(clip) = animation
        .object_clips
        .iter()
        .find(|clip| clip.id == workspace.door_clip_id)
    else {
        let message = format!(
            "Scene animation clip '{}' is missing.",
            workspace.door_clip_id
        );
        push_diagnostic(&mut workspace, message);
        return;
    };
    if !doors
        .iter()
        .any(|(_, object_id, _)| object_id.0 == clip.object_id)
    {
        push_diagnostic(
            &mut workspace,
            format!(
                "Scene animation clip '{}' targets missing runtime object '{}'.",
                clip.id, clip.object_id
            ),
        );
        return;
    }
    if workspace.door_reset_revision != *last_reset_revision {
        *last_reset_revision = workspace.door_reset_revision;
        for (entity, object_id, _) in &mut doors {
            if object_id.0 == clip.object_id {
                commands
                    .entity(entity)
                    .insert(Collider::cuboid(0.55, 1.05, 0.06));
            }
        }
    }
    if workspace.door_playing {
        let previous_time = workspace.door_time_seconds;
        workspace.door_time_seconds =
            (workspace.door_time_seconds + time.delta_secs()).min(clip.duration_seconds);
        for event in &clip.events {
            if animation_event_crossed(
                previous_time,
                workspace.door_time_seconds,
                event.time_seconds,
            ) && !workspace.door_fired_events.contains(&event.id)
            {
                if let quasar_project::AnimationEventKindSnapshot::PlaySound { asset_id } =
                    &event.kind
                    && let Some(asset) = animation.assets.iter().find(|asset| asset.id == *asset_id)
                    && asset.kind == AnimationAssetKind::Audio
                {
                    commands.spawn((
                        AudioPlayer::new(asset_server.load(asset.path.clone())),
                        PlaybackSettings::DESPAWN
                            .with_volume(Volume::Linear(0.6))
                            .with_spatial(true),
                        Transform::from_xyz(1.35, 1.0, -2.82),
                        AnimationSessionAudio,
                        Name::new(format!("Animation event {}", event.id)),
                    ));
                }
                workspace.door_fired_events.push(event.id.clone());
                info!(event = %event.id, "Animation event fired");
            }
        }
        if workspace.door_time_seconds >= clip.duration_seconds {
            workspace.door_playing = false;
            workspace.door_completed = true;
            info!(clip = %clip.id, "Scene animation clip completed");
            workspace
                .diagnostics
                .retain(|message| !message.contains("door collider"));
            for (entity, object_id, _) in &mut doors {
                if object_id.0 == clip.object_id {
                    commands.entity(entity).remove::<Collider>();
                }
            }
        }
    }

    let clip_time = workspace.door_time_seconds;
    for (_, object_id, mut transform) in &mut doors {
        if object_id.0 != clip.object_id {
            continue;
        }
        let (translation, rotation) = sample_object_clip(clip, clip_time);
        if let Some(translation) = translation {
            transform.translation = translation;
        }
        if let Some(rotation) = rotation {
            transform.rotation = rotation;
        }
    }
}

fn prepare_imported_animation_models(
    data: Res<AnimationProbeData>,
    asset_server: Res<AssetServer>,
    gltfs: Res<Assets<Gltf>>,
    clips_assets: Res<Assets<bevy::animation::AnimationClip>>,
    mut graphs: ResMut<Assets<AnimationGraph>>,
    mut prepared: ResMut<PreparedAnimationModels>,
    mut workspace: ResMut<AnimationWorkspaceState>,
) {
    let Some(animation) = data.animation.as_ref() else {
        return;
    };
    for binding in &animation.imported_bindings {
        if prepared.models.contains_key(&binding.asset_id) {
            continue;
        }
        let Some(asset) = animation
            .assets
            .iter()
            .find(|asset| asset.id == binding.asset_id)
        else {
            push_diagnostic(
                &mut workspace,
                format!(
                    "Animation binding references missing asset '{}'.",
                    binding.asset_id
                ),
            );
            continue;
        };
        if asset.kind != AnimationAssetKind::Model {
            push_diagnostic(
                &mut workspace,
                format!("Animation binding asset '{}' is not a model.", asset.id),
            );
            continue;
        }
        let gltf_handle = prepared
            .loading
            .entry(asset.id.clone())
            .or_insert_with(|| asset_server.load::<Gltf>(asset.path.clone()))
            .clone();
        if !asset_server.is_loaded_with_dependencies(&gltf_handle) {
            if let Some(LoadState::Failed(error)) = asset_server.get_load_state(&gltf_handle) {
                let message = format!("Cannot load animation model '{}': {error}", asset.id);
                push_diagnostic(&mut workspace, message);
            }
            continue;
        }
        let Some(gltf) = gltfs.get(&gltf_handle) else {
            continue;
        };
        let mut clips = Vec::with_capacity(binding.clips.len());
        let mut source_ids = Vec::with_capacity(binding.clips.len());
        let mut durations = HashMap::new();
        let available_names = gltf
            .named_animations
            .keys()
            .map(|name| name.as_ref())
            .collect::<Vec<_>>();
        let missing = missing_source_clip_names(
            &available_names,
            &binding
                .clips
                .iter()
                .map(|clip| clip.source_clip_id.clone())
                .collect::<Vec<_>>(),
        );
        if !missing.is_empty() {
            let message = format!(
                "Animation model '{}' is missing source clip(s): {}.",
                asset.id,
                missing.join(", ")
            );
            push_diagnostic(&mut workspace, message);
            continue;
        }
        for clip in &binding.clips {
            let handle = &gltf.named_animations[clip.source_clip_id.as_str()];
            clips.push(handle.clone());
            source_ids.push(clip.source_clip_id.clone());
            if let Some(clip_asset) = clips_assets.get(handle) {
                durations.insert(clip.source_clip_id.clone(), clip_asset.duration());
            }
        }
        let (graph, node_indices) = AnimationGraph::from_clips(clips);
        let prepared_source_ids = source_ids.clone();
        prepared.models.insert(
            asset.id.clone(),
            PreparedAnimationModel {
                graph: graphs.add(graph),
                nodes: source_ids.into_iter().zip(node_indices).collect(),
                durations,
            },
        );
        info!(
            asset_id = %asset.id,
            source_clips = ?prepared_source_ids,
            "Prepared imported GLB animation graph"
        );
        workspace
            .diagnostics
            .retain(|message| !message.contains(&format!("'{}'", asset.id)));
    }
}

fn attach_imported_animation_rigs(
    mut commands: Commands,
    data: Res<AnimationProbeData>,
    prepared: Res<PreparedAnimationModels>,
    mut players: Query<(Entity, &mut AnimationPlayer), Without<ImportedAnimationRig>>,
    object_ids: Query<&SnapshotObjectId>,
    parents: Query<&ChildOf>,
) {
    let Some(animation) = data.animation.as_ref() else {
        return;
    };
    for (player_entity, mut player) in &mut players {
        let mut ancestor = player_entity;
        let mut object_id = None;
        for _ in 0..128 {
            if let Ok(snapshot_id) = object_ids.get(ancestor) {
                object_id = Some(snapshot_id.0.as_str());
                break;
            }
            let Ok(parent) = parents.get(ancestor) else {
                break;
            };
            ancestor = parent.parent();
        }
        let Some(object_id) = object_id else {
            warn!(entity = ?player_entity, "GLB AnimationPlayer has no snapshot object ancestor");
            continue;
        };
        let Some(binding) = animation
            .imported_bindings
            .iter()
            .find(|binding| binding.object_id == object_id)
        else {
            warn!(entity = ?player_entity, object_id, "GLB AnimationPlayer has no imported clip binding");
            continue;
        };
        let Some(model) = prepared.models.get(&binding.asset_id) else {
            continue;
        };
        let initial_source = if model.nodes.contains_key("Idle") {
            "Idle"
        } else if let Some(clip) = binding.clips.first() {
            clip.source_clip_id.as_str()
        } else {
            continue;
        };
        let Some(node) = model.nodes.get(initial_source).copied() else {
            continue;
        };
        let mut transitions = AnimationTransitions::new();
        transitions.play(&mut player, node, Duration::ZERO).repeat();
        commands.entity(player_entity).insert((
            AnimationGraphHandle(model.graph.clone()),
            transitions,
            ImportedAnimationRig {
                object_id: object_id.to_owned(),
                asset_id: binding.asset_id.clone(),
                nodes: model.nodes.clone(),
                active_source: initial_source.to_owned(),
                last_seek_revision: u64::MAX,
                was_playing: true,
            },
        ));
        info!(
            object_id = %object_id,
            asset_id = %binding.asset_id,
            source_clip = initial_source,
            "Imported GLB animation player configured"
        );
    }
}

fn update_imported_animation_rigs(
    data: Res<AnimationProbeData>,
    time: Res<Time>,
    mut workspace: ResMut<AnimationWorkspaceState>,
    prepared: Res<PreparedAnimationModels>,
    mut rigs: Query<(
        &mut AnimationPlayer,
        &mut AnimationTransitions,
        &mut ImportedAnimationRig,
    )>,
) {
    let Some(animation) = data.animation.as_ref() else {
        return;
    };
    for (mut player, mut transitions, mut rig) in &mut rigs {
        let Some(binding) = animation
            .imported_bindings
            .iter()
            .find(|binding| binding.object_id == rig.object_id && binding.asset_id == rig.asset_id)
        else {
            continue;
        };
        let requested = if binding
            .clips
            .iter()
            .any(|clip| clip.source_clip_id == workspace.character_clip_source)
        {
            workspace.character_clip_source.as_str()
        } else if binding
            .clips
            .iter()
            .any(|clip| clip.source_clip_id == "Idle")
        {
            "Idle"
        } else if let Some(clip) = binding.clips.first() {
            clip.source_clip_id.as_str()
        } else {
            continue;
        };
        if requested != rig.active_source
            && let Some(node) = rig.nodes.get(requested).copied()
        {
            transitions
                .play(&mut player, node, Duration::from_millis(250))
                .repeat();
            info!(
                object_id = %rig.object_id,
                source_clip = requested,
                "Transitioned imported animation clip"
            );
            rig.active_source = requested.to_owned();
            rig.last_seek_revision = u64::MAX;
        }
        if let Some(duration) = prepared
            .models
            .get(&rig.asset_id)
            .and_then(|model| model.durations.get(rig.active_source.as_str()))
        {
            workspace.character_duration_seconds = *duration;
        }
        if rig.last_seek_revision != workspace.character_seek_revision
            && let Some(node) = rig.nodes.get(rig.active_source.as_str()).copied()
            && let Some(active) = player.animation_mut(node)
        {
            active.seek_to(workspace.character_preview_time_seconds);
            rig.last_seek_revision = workspace.character_seek_revision;
        }
        if rig.was_playing != workspace.character_preview_playing
            && let Some(node) = rig.nodes.get(rig.active_source.as_str()).copied()
            && let Some(active) = player.animation_mut(node)
        {
            if workspace.character_preview_playing {
                active.resume();
            } else {
                active.pause();
            }
            rig.was_playing = workspace.character_preview_playing;
        }
        if workspace.character_preview_playing {
            let duration = workspace.character_duration_seconds.max(f32::EPSILON);
            workspace.character_preview_time_seconds =
                (workspace.character_preview_time_seconds + time.delta_secs()) % duration;
        }
    }
}

/// Attaches the stable snapshot ID to a GLB instance root.
pub fn spawn_animated_model(
    commands: &mut Commands,
    asset_server: &AssetServer,
    asset_path: impl Into<String>,
    object_id: impl Into<String>,
    transform: Transform,
) -> Entity {
    commands
        .spawn((
            WorldAssetRoot(
                asset_server.load(GltfAssetLabel::Scene(0).from_asset(asset_path.into())),
            ),
            transform,
            SnapshotObjectId(object_id.into()),
        ))
        .id()
}

/// Calibrated GLB root transform shared by the Editor and Player Stage 0 probe.
pub fn stage0_character_model_transform() -> Transform {
    Transform::from_xyz(0.0, -1.12, 0.0).with_scale(Vec3::splat(0.5))
}

/// Returns the animation kind for a matching asset ID, if present.
pub fn animation_asset_kind(
    animation: &AnimationSnapshot,
    asset_id: &str,
) -> Option<AnimationAssetKind> {
    animation
        .assets
        .iter()
        .find(|asset| asset.id == asset_id)
        .map(|asset| asset.kind)
}

fn push_diagnostic(workspace: &mut AnimationWorkspaceState, message: String) {
    if !workspace.diagnostics.contains(&message) {
        warn!("{message}");
        workspace.diagnostics.push(message);
    }
}

fn animation_event_crossed(previous_time: f32, current_time: f32, event_time: f32) -> bool {
    previous_time < event_time && current_time >= event_time
}

fn missing_source_clip_names(available: &[&str], requested: &[String]) -> Vec<String> {
    requested
        .iter()
        .filter(|source| !available.contains(&source.as_str()))
        .cloned()
        .collect()
}

#[cfg(test)]
mod tests {
    use super::{
        AnimationWorkspaceState, animation_event_crossed, missing_source_clip_names,
        sample_object_clip,
    };
    use bevy::prelude::Vec3;
    use quasar_project::{
        AnimatedTransformProperty, AnimationEventSnapshot, AnimationInterpolation,
        AnimationKeySnapshot, AnimationValueSnapshot, ObjectAnimationClipSnapshot,
        TransformAnimationTrackSnapshot,
    };

    fn clip(interpolation: AnimationInterpolation) -> ObjectAnimationClipSnapshot {
        ObjectAnimationClipSnapshot {
            id: "test".into(),
            object_id: "door".into(),
            duration_seconds: 1.0,
            tracks: vec![
                TransformAnimationTrackSnapshot {
                    property: AnimatedTransformProperty::Translation,
                    interpolation,
                    keys: vec![
                        AnimationKeySnapshot {
                            time_seconds: 0.0,
                            value: AnimationValueSnapshot::Vec3([0.0, 0.0, 0.0]),
                        },
                        AnimationKeySnapshot {
                            time_seconds: 1.0,
                            value: AnimationValueSnapshot::Vec3([2.0, 0.0, 0.0]),
                        },
                    ],
                },
                TransformAnimationTrackSnapshot {
                    property: AnimatedTransformProperty::Rotation,
                    interpolation,
                    keys: vec![
                        AnimationKeySnapshot {
                            time_seconds: 0.0,
                            value: AnimationValueSnapshot::Quaternion([0.0, 0.0, 0.0, 1.0]),
                        },
                        AnimationKeySnapshot {
                            time_seconds: 1.0,
                            value: AnimationValueSnapshot::Quaternion([0.0, 1.0, 0.0, 0.0]),
                        },
                    ],
                },
            ],
            events: vec![AnimationEventSnapshot {
                id: "sound".into(),
                time_seconds: 0.5,
                kind: quasar_project::AnimationEventKindSnapshot::PlaySound {
                    asset_id: "latch".into(),
                },
            }],
        }
    }

    #[test]
    fn transform_clip_sampling_respects_interpolation_and_clamps_time() {
        let (start, _) = sample_object_clip(&clip(AnimationInterpolation::Linear), -1.0);
        let (middle, rotation) = sample_object_clip(&clip(AnimationInterpolation::Linear), 0.5);
        let (end, _) = sample_object_clip(&clip(AnimationInterpolation::Linear), 5.0);
        assert_eq!(start, Some(Vec3::ZERO));
        assert_eq!(middle, Some(Vec3::X));
        assert!(rotation.is_some_and(|value| (value.length() - 1.0).abs() < 1e-5));
        assert_eq!(end, Some(Vec3::new(2.0, 0.0, 0.0)));

        let (linear_quarter, _) = sample_object_clip(&clip(AnimationInterpolation::Linear), 0.25);
        let (smooth_quarter, _) =
            sample_object_clip(&clip(AnimationInterpolation::SmoothStep), 0.25);
        assert_eq!(linear_quarter, Some(Vec3::new(0.5, 0.0, 0.0)));
        assert_eq!(smooth_quarter, Some(Vec3::new(0.3125, 0.0, 0.0)));
        let (step_middle, _) = sample_object_clip(&clip(AnimationInterpolation::Step), 0.5);
        assert_eq!(step_middle, Some(Vec3::ZERO));
    }

    #[test]
    fn fixed_fixture_samples_expected_door_poses_at_key_times() {
        let fixture = include_str!("../../../tests/fixtures/stage0-animation/quasar.snapshot.json");
        let snapshot: quasar_project::ProjectSnapshot =
            serde_json::from_str(fixture).expect("animation fixture parses");
        let clip = snapshot
            .scene
            .animation
            .as_ref()
            .and_then(|animation| {
                animation
                    .object_clips
                    .iter()
                    .find(|clip| clip.id == "door_open")
            })
            .expect("door animation exists");
        let (start, _) = sample_object_clip(clip, 0.0);
        let (middle, rotation) = sample_object_clip(clip, 0.375);
        let (end, _) = sample_object_clip(clip, 0.75);
        assert_eq!(start, Some(Vec3::new(1.35, 1.02, -2.82)));
        assert_eq!(middle, Some(Vec3::new(1.575, 1.02, -2.82)));
        assert_eq!(end, Some(Vec3::new(1.8, 1.02, -2.82)));
        assert!(rotation.is_some_and(|value| {
            (value.length() - 1.0).abs() < 1e-5
                && (value.to_euler(bevy::prelude::EulerRot::YXZ).0 - 0.625).abs() < 0.01
        }));
    }

    #[test]
    fn imported_clip_names_must_match_the_glb_source_exactly() {
        let requested = vec!["Idle".to_owned(), "Walk".to_owned()];
        assert_eq!(
            missing_source_clip_names(&["Idle", "Walking"], &requested),
            vec!["Walk".to_owned()]
        );
        assert!(
            missing_source_clip_names(&["Idle", "Walking"], &["Idle".into(), "Walking".into()])
                .is_empty()
        );
    }

    #[test]
    fn event_crossing_is_one_shot_per_run_and_restart_clears_fired_events() {
        assert!(!animation_event_crossed(0.12, 0.13, 0.12));
        assert!(animation_event_crossed(0.11, 0.13, 0.12));
        assert!(!animation_event_crossed(0.0, 0.11, 0.12));

        let mut state = AnimationWorkspaceState {
            door_time_seconds: 0.5,
            door_completed: true,
            ..Default::default()
        };
        state
            .door_fired_events
            .push("door_latch_at_open".to_owned());
        state.play_door_clip();
        assert_eq!(state.door_time_seconds, 0.0);
        assert!(state.door_playing);
        assert!(!state.door_completed);
        assert!(state.door_fired_events.is_empty());
    }
}
