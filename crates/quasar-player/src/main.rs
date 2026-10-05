use std::{
    collections::HashSet,
    env, fs,
    path::{Path, PathBuf},
};

use bevy::{
    asset::AssetPlugin,
    audio::{GlobalVolume, SpatialListener, Volume},
    gltf::GltfAssetLabel,
    input::InputSystems,
    input::mouse::AccumulatedMouseMotion,
    prelude::*,
    render::view::screenshot::{Screenshot, save_to_disk},
    window::{CursorGrabMode, CursorOptions, PrimaryWindow, WindowPlugin},
    world_serialization::WorldAssetRoot,
};
use bevy_rapier3d::prelude::{
    AdditionalMassProperties, CharacterAutostep, CharacterLength, Collider, Damping,
    KinematicCharacterController, NoUserData, PhysicsSet, QueryFilter, RapierPhysicsPlugin,
    ReadRapierContext, RigidBody, TimestepMode,
};
use quasar_project::document::{ObjectId, ProjectDocument, SceneDocument};
use quasar_project::{
    ProjectSnapshot, SceneSnapshot, SnapshotObject, SnapshotObjectKind,
    assets::{AssetCatalog, AssetKind, AssetStatus, ModelAssetComponent, ScriptComponent},
    gameplay::{
        AudioListenerComponent, AudioSourceComponent, CameraComponent,
        CharacterControllerComponent, ColliderComponent, DoorComponent, RigidBodyComponent,
        TypedComponent,
    },
};
use quasar_runtime::{
    animation::{
        AnimationProbeData, AnimationProbePlugin, AnimationSessionAudio, AnimationWorkspaceState,
        SnapshotObjectId, stage0_character_model_transform,
    },
    gameplay::ProjectGameplayCommand,
    gameplay::{GameplayCommand, run_door_interaction},
    physics::{CharacterController, CharacterControllerInput},
    project_scene::{
        character_controller, collider_from_document, controller_collider, rigid_body,
        transform_from_document,
    },
};

const PROBE_TITLE: &str = "Quasar Player · Stage 0";
const BENCHMARK_WARMUP_SECONDS: f64 = 5.0;
const BENCHMARK_SAMPLE_SECONDS: f64 = 60.0;

#[derive(Resource, Default)]
struct BenchmarkRun {
    elapsed_seconds: f64,
    frame_times_ms: Vec<f64>,
    finished: bool,
}

#[derive(Clone, Debug, Resource)]
struct ResolvedSnapshot {
    project_root: PathBuf,
    asset_root: PathBuf,
    scene: SceneSnapshot,
    door_script: String,
}

#[derive(Clone, Debug, Resource)]
struct ResolvedDocument {
    project_path: PathBuf,
    asset_root: PathBuf,
    asset_catalog: AssetCatalog,
    document: ProjectDocument,
}

#[derive(Clone, Copy, Component)]
struct DocumentObjectId {
    _id: ObjectId,
}

#[derive(Component)]
struct DocumentAudioVoice;

#[derive(Component)]
struct DoorMotion {
    closed_rotation: Quat,
    open_angle_radians: f32,
    duration_seconds: f32,
    elapsed_seconds: Option<f32>,
}

impl DoorMotion {
    fn begin_open(&mut self) {
        if self.elapsed_seconds.is_none() {
            self.elapsed_seconds = Some(0.0);
        }
    }
}

#[derive(Component)]
struct PlayerCameraSettings {
    sensitivity: f32,
    pitch: f32,
}

#[derive(Component)]
struct ControllerMouseSensitivity(f32);

#[derive(Resource, Default)]
struct PendingDocumentJump(bool);

#[derive(Component)]
struct PlayerDoor;

#[derive(Component)]
struct PlayerSessionAudio;

type PlayerSessionAudioQuery<'w, 's> = Query<
    'w,
    's,
    (
        Entity,
        Option<&'static AudioSink>,
        Option<&'static SpatialAudioSink>,
    ),
    Or<(With<PlayerSessionAudio>, With<AnimationSessionAudio>)>,
>;

#[derive(Resource, Default)]
struct PlayerSessionState {
    ambience_active: bool,
    door_opened: bool,
}

#[derive(Resource)]
struct ScreenshotRequest(String);

fn main() {
    if let Err(error) = run() {
        eprintln!("quasar-player: {error}");
        std::process::exit(2);
    }
}

fn run() -> Result<(), String> {
    let args = env::args().skip(1).collect::<Vec<_>>();
    if args.iter().any(|argument| argument == "--lua-probe-only") {
        let value = quasar_runtime::compatibility_probe_lua()
            .map_err(|error| format!("Lua compatibility probe failed: {error}"))?;
        if value != 42 {
            return Err(format!(
                "Lua compatibility probe returned {value}, expected 42"
            ));
        }
        println!("Lua 5.4 compatibility probe passed: {value}");
        return Ok(());
    }

    if let Some(document_path) = parse_project_document_argument(&args)? {
        let asset_root = parse_asset_root_argument(&args)?.unwrap_or_else(|| {
            document_path
                .parent()
                .unwrap_or_else(|| Path::new("."))
                .to_path_buf()
        });
        let document = resolve_project_document_with_asset_root(&document_path, &asset_root)?;
        let screenshot = parse_screenshot_argument(&args)?;
        if args.iter().any(|argument| argument == "--validate-only") {
            let scene = active_document_scene(&document.document)?;
            println!(
                "Project document valid: project='{}', scene='{}', objects={}",
                document.document.name,
                scene.name,
                scene.objects.len()
            );
            return Ok(());
        }
        return launch_document_preview(document, screenshot);
    }

    let snapshot_arg = parse_snapshot_argument(&args)?;
    let screenshot = parse_screenshot_argument(&args)?;
    let snapshot = resolve_snapshot(&snapshot_arg)?;
    let benchmark = args.iter().any(|argument| argument == "--benchmark-60s");
    let animation_smoke = args.iter().any(|argument| argument == "--animation-smoke");
    if args.iter().any(|argument| argument == "--validate-only") {
        println!(
            "Snapshot valid: scene='{}', assets='{}'",
            snapshot.scene.name,
            snapshot.asset_root.display()
        );
        return Ok(());
    }
    if args.iter().any(|argument| argument == "--run-interaction") {
        let actions = run_door_interaction(&snapshot.door_script)
            .map_err(|error| format!("Lua interaction failed: {error}"))?;
        println!("Lua interaction actions: {actions:?}");
        return Ok(());
    }

    launch_player(snapshot, benchmark, screenshot, animation_smoke)
}

fn parse_project_document_argument(args: &[String]) -> Result<Option<PathBuf>, String> {
    let mut document = None;
    let mut index = 0;
    while index < args.len() {
        if args[index] == "--project-document" {
            if document.is_some() {
                return Err("--project-document may be supplied only once".to_owned());
            }
            index += 1;
            document =
                Some(PathBuf::from(args.get(index).ok_or_else(|| {
                    "--project-document requires a path".to_owned()
                })?));
        }
        index += 1;
    }
    Ok(document)
}

fn parse_asset_root_argument(args: &[String]) -> Result<Option<PathBuf>, String> {
    let mut root = None;
    let mut index = 0;
    while index < args.len() {
        if args[index] == "--asset-root" {
            if root.is_some() {
                return Err("--asset-root may be supplied only once".to_owned());
            }
            index += 1;
            root = Some(PathBuf::from(
                args.get(index)
                    .ok_or_else(|| "--asset-root requires a path".to_owned())?,
            ));
        }
        index += 1;
    }
    Ok(root)
}

fn resolve_project_document_with_asset_root(
    path: &Path,
    asset_root: &Path,
) -> Result<ResolvedDocument, String> {
    let project_path = fs::canonicalize(path).map_err(|error| {
        format!(
            "cannot resolve project document {}: {error}",
            path.display()
        )
    })?;
    let document = ProjectDocument::load(&project_path)?;
    let asset_root = fs::canonicalize(asset_root).map_err(|error| {
        format!(
            "cannot resolve asset root {}: {error}",
            asset_root.display()
        )
    })?;
    validate_document_model_assets(&asset_root, &document)?;
    validate_document_scene_audio(active_document_scene(&document)?, &document)?;
    let asset_catalog = AssetCatalog::scan(&asset_root);
    Ok(ResolvedDocument {
        project_path,
        asset_root,
        asset_catalog,
        document,
    })
}

fn validate_document_model_assets(
    project_root: &Path,
    document: &ProjectDocument,
) -> Result<(), String> {
    let catalog = AssetCatalog::scan(project_root);
    for object in document.scenes.iter().flat_map(|scene| &scene.objects) {
        if let Some(reference) = ModelAssetComponent::from_components(&object.components)? {
            let asset =
                find_ready_asset(project_root, &catalog, reference.asset_id, AssetKind::Model)
                    .map_err(|message| format!("object '{}': {message}", object.name))?;
            if !project_root.join(&asset.metadata.source_path).is_file() {
                return Err(format!(
                    "object '{}' references model asset '{}' whose source file is missing",
                    object.name, reference.asset_id.0
                ));
            }
        }
        if let Some(reference) = ScriptComponent::from_components(&object.components)?
            && reference.enabled
        {
            let asset = find_ready_asset(
                project_root,
                &catalog,
                reference.asset_id,
                AssetKind::Script,
            )
            .map_err(|message| format!("object '{}': {message}", object.name))?;
            let source_path = project_root.join(&asset.metadata.source_path);
            let source = fs::read_to_string(&source_path).map_err(|error| {
                format!(
                    "object '{}' Lua script '{}' cannot be read: {error}",
                    object.name, asset.metadata.source_path
                )
            })?;
            quasar_runtime::gameplay::validate_project_script(&asset.metadata.source_path, &source)
                .map_err(|error| format!("object '{}' Lua script error: {error}", object.name))?;
        }
        if let Some(reference) = AudioSourceComponent::from_components(&object.components)?
            && reference.enabled
        {
            let asset =
                find_ready_asset(project_root, &catalog, reference.asset_id, AssetKind::Audio)
                    .map_err(|message| format!("object '{}': {message}", object.name))?;
            if !project_root.join(&asset.metadata.source_path).is_file() {
                return Err(format!(
                    "object '{}' references audio asset '{}' whose source file is missing",
                    object.name, reference.asset_id.0
                ));
            }
            quasar_project::assets::validate_wav(
                &fs::read(project_root.join(&asset.metadata.source_path)).map_err(|error| {
                    format!("object '{}' WAV asset cannot be read: {error}", object.name)
                })?,
            )
            .map_err(|error| format!("object '{}' WAV asset is invalid: {error}", object.name))?;
        }
    }
    Ok(())
}

fn find_ready_asset<'a>(
    project_root: &Path,
    catalog: &'a AssetCatalog,
    asset_id: quasar_project::assets::AssetId,
    expected_kind: AssetKind,
) -> Result<&'a quasar_project::assets::AssetRecord, String> {
    let asset = catalog
        .assets
        .iter()
        .find(|asset| asset.metadata.asset_id == asset_id)
        .ok_or_else(|| {
            format!(
                "references missing {:?} asset '{}'",
                expected_kind, asset_id.0
            )
        })?;
    if asset.metadata.kind != expected_kind || asset.status != AssetStatus::Ready {
        return Err(format!(
            "references {:?} asset '{}' in kind/status {:?}/{:?}",
            expected_kind, asset_id.0, asset.metadata.kind, asset.status
        ));
    }
    let root = fs::canonicalize(project_root)
        .map_err(|error| format!("cannot resolve asset root: {error}"))?;
    let source = fs::canonicalize(&asset.source_file).map_err(|error| {
        format!(
            "asset '{}' source cannot be resolved: {error}",
            asset.metadata.source_path
        )
    })?;
    if !source.starts_with(root) {
        return Err(format!(
            "asset '{}' resolves outside the project root",
            asset.metadata.source_path
        ));
    }
    Ok(asset)
}

fn validate_document_scene_audio(
    scene: &SceneDocument,
    document: &ProjectDocument,
) -> Result<(), String> {
    let listeners = scene
        .objects
        .iter()
        .filter_map(|object| {
            AudioListenerComponent::from_components(&object.components)
                .ok()
                .flatten()
        })
        .filter(|listener| listener.enabled)
        .count();
    let mut spatial_sources = 0usize;
    let mut autoplay_sources = 0usize;
    for object in &scene.objects {
        if let Some(source) = AudioSourceComponent::from_components(&object.components)?
            && source.enabled
        {
            spatial_sources += usize::from(source.spatial);
            autoplay_sources += usize::from(source.autoplay);
        }
    }
    if listeners > 1 {
        return Err(format!(
            "active scene '{}' has {listeners} enabled audio listeners; exactly one is supported",
            scene.name
        ));
    }
    if spatial_sources > 0 && listeners != 1 {
        return Err(format!(
            "active scene '{}' has spatial audio sources but no enabled Audio Listener",
            scene.name
        ));
    }
    if autoplay_sources > 32 {
        return Err(format!(
            "active scene '{}' has {autoplay_sources} autoplay sources; the session budget is 32",
            scene.name
        ));
    }
    document.audio.validate()?;
    Ok(())
}

fn active_document_scene(document: &ProjectDocument) -> Result<&SceneDocument, String> {
    document
        .scenes
        .iter()
        .find(|scene| scene.id == document.active_scene_id)
        .ok_or_else(|| "active scene is missing from the project document".to_owned())
}

fn launch_document_preview(
    document: ResolvedDocument,
    screenshot: Option<String>,
) -> Result<(), String> {
    let scene = active_document_scene(&document.document)?;
    let mut app = App::new();
    let asset_root = document.asset_root.to_string_lossy().into_owned();
    app.add_plugins(
        DefaultPlugins
            .set(WindowPlugin {
                primary_window: Some(Window {
                    title: format!("{} — Quasar Player Preview", document.document.name),
                    name: Some("quasar.player.document_preview".to_owned()),
                    resolution: (1280, 800).into(),
                    resizable: true,
                    ..default()
                }),
                ..default()
            })
            .set(AssetPlugin {
                file_path: asset_root,
                ..default()
            }),
    )
    .insert_resource(document.clone())
    .insert_resource(GlobalVolume::new(Volume::Linear(
        document.document.audio.master,
    )))
    .insert_resource(TimestepMode::Fixed {
        dt: 1.0 / 60.0,
        substeps: 1,
    })
    .init_resource::<PendingDocumentJump>()
    .add_plugins(RapierPhysicsPlugin::<NoUserData>::default().in_fixed_schedule())
    .add_plugins(quasar_runtime::physics::CharacterControllerPlugin)
    .add_systems(Startup, setup_document_preview)
    .add_systems(
        PreUpdate,
        collect_document_controller_input.after(InputSystems),
    )
    .add_systems(
        FixedUpdate,
        animate_document_doors.before(PhysicsSet::SyncBackend),
    )
    .add_systems(
        FixedUpdate,
        consume_document_jump.after(PhysicsSet::SyncBackend),
    )
    .add_systems(Update, document_interact.after(TransformSystems::Propagate));
    if let Some(path) = screenshot {
        app.insert_resource(ScreenshotRequest(path))
            .add_systems(Update, capture_screenshot_after_render_warmup);
    }
    info!(
        project = %document.project_path.display(),
        scene = %scene.name,
        "Standalone Player opened a Stage 1 document preview"
    );
    app.run();
    Ok(())
}

fn setup_document_preview(
    mut commands: Commands,
    document: Res<ResolvedDocument>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    asset_server: Res<AssetServer>,
) {
    commands.spawn((
        DirectionalLight {
            illuminance: 10_000.0,
            shadow_maps_enabled: true,
            ..default()
        },
        Transform::from_rotation(Quat::from_euler(EulerRot::XYZ, -0.8, -0.6, 0.0)),
    ));
    let Ok(scene) = active_document_scene(&document.document) else {
        return;
    };
    let active_controller = scene.objects.iter().find_map(|object| {
        CharacterControllerComponent::from_components(&object.components)
            .ok()
            .flatten()
            .filter(|controller| controller.enabled)
            .map(|controller| (object.id, controller))
    });
    let active_camera_object = active_controller
        .map(|(_, controller)| controller.camera_object)
        .or_else(|| {
            scene.objects.iter().find_map(|object| {
                CameraComponent::from_components(&object.components)
                    .ok()
                    .flatten()
                    .filter(|camera| camera.enabled)
                    .map(|_| object.id)
            })
        });
    if active_camera_object.is_none() {
        commands.spawn((
            Camera3d::default(),
            Transform::from_xyz(5.0, 4.0, 7.0).looking_at(Vec3::ZERO, Vec3::Y),
        ));
    }
    let default_mesh = meshes.add(Cuboid::new(1.0, 1.0, 1.0));
    let material = materials.add(StandardMaterial {
        base_color: Color::srgb(0.36, 0.58, 0.76),
        perceptual_roughness: 0.78,
        ..default()
    });
    let entities = scene
        .objects
        .iter()
        .map(|object| (object.id, commands.spawn_empty().id()))
        .collect::<std::collections::HashMap<_, _>>();
    let asset_catalog = &document.asset_catalog;
    for object in &scene.objects {
        let Some(&entity) = entities.get(&object.id) else {
            continue;
        };
        let mut transform = transform_from_document(&object.local_transform);
        let object_mesh = ColliderComponent::from_components(&object.components)
            .ok()
            .flatten()
            .filter(|collider| collider.enabled)
            .and_then(|collider| match collider.shape {
                quasar_project::gameplay::ColliderShape::Box { size } => {
                    Some(meshes.add(Cuboid::from_size(Vec3::from_array(size))))
                }
                quasar_project::gameplay::ColliderShape::Capsule { .. } => None,
            })
            .unwrap_or_else(|| default_mesh.clone());
        let model_path = ModelAssetComponent::from_components(&object.components)
            .ok()
            .flatten()
            .and_then(|reference| {
                asset_catalog.assets.iter().find(|asset| {
                    asset.metadata.asset_id == reference.asset_id
                        && asset.metadata.kind == AssetKind::Model
                        && asset.status == AssetStatus::Ready
                })
            })
            .map(|asset| asset.metadata.source_path.clone());
        if let Some(model_path) = model_path {
            commands.entity(entity).insert((
                WorldAssetRoot(asset_server.load(GltfAssetLabel::Scene(0).from_asset(model_path))),
                transform,
                Name::new(object.name.clone()),
                DocumentObjectId { _id: object.id },
            ));
        } else {
            commands.entity(entity).insert((
                Mesh3d(object_mesh),
                MeshMaterial3d(material.clone()),
                transform,
                Name::new(object.name.clone()),
                DocumentObjectId { _id: object.id },
            ));
        }
        if let Ok(Some(controller)) =
            CharacterControllerComponent::from_components(&object.components)
            && controller.enabled
        {
            commands.entity(entity).insert((
                RigidBody::KinematicPositionBased,
                controller_collider(&controller),
                character_controller(&controller),
                ControllerMouseSensitivity(controller.mouse_sensitivity),
                CharacterController::with_tuning(
                    controller.walk_speed,
                    controller.jump_speed,
                    controller.gravity,
                ),
                CharacterControllerInput::default(),
            ));
        } else if let Ok(Some(collider)) = ColliderComponent::from_components(&object.components)
            && collider.enabled
        {
            commands.spawn((
                collider_from_document(&collider),
                Transform::from_translation(Vec3::from_array(collider.center)),
                DocumentObjectId { _id: object.id },
                ChildOf(entity),
            ));
            if let Ok(Some(body)) = RigidBodyComponent::from_components(&object.components)
                && body.enabled
            {
                commands.entity(entity).insert((
                    rigid_body(body.kind),
                    Damping {
                        linear_damping: body.linear_damping,
                        ..default()
                    },
                ));
                if body.kind == quasar_project::gameplay::RigidBodyKind::Dynamic {
                    commands
                        .entity(entity)
                        .insert(AdditionalMassProperties::Mass(body.mass));
                }
            } else {
                commands.entity(entity).insert(RigidBody::Fixed);
            }
        }
        if let Ok(Some(door)) = DoorComponent::from_components(&object.components)
            && door.enabled
        {
            commands.entity(entity).insert(DoorMotion {
                closed_rotation: transform.rotation,
                open_angle_radians: door.open_angle_degrees.to_radians(),
                duration_seconds: door.open_duration_seconds,
                elapsed_seconds: None,
            });
        }
        if let Ok(Some(camera)) = CameraComponent::from_components(&object.components)
            && camera.enabled
            && active_camera_object == Some(object.id)
        {
            let is_player_camera = active_controller.is_some();
            if is_player_camera && let Some((_, controller)) = active_controller {
                transform.translation.y = controller.eye_height;
                commands.entity(entity).insert(PlayerCameraSettings {
                    sensitivity: controller.mouse_sensitivity,
                    pitch: 0.0,
                });
            }
            commands.entity(entity).insert((
                Camera3d::default(),
                Projection::Perspective(PerspectiveProjection {
                    fov: camera.vertical_fov_degrees.to_radians(),
                    ..default()
                }),
            ));
        }
        if let Ok(Some(listener)) = AudioListenerComponent::from_components(&object.components)
            && listener.enabled
        {
            commands.entity(entity).insert(SpatialListener::new(0.2));
        }
        if let Ok(Some(source)) = AudioSourceComponent::from_components(&object.components)
            && source.enabled
            && source.autoplay
            && let Some(asset) = asset_catalog.assets.iter().find(|asset| {
                asset.metadata.asset_id == source.asset_id
                    && asset.metadata.kind == AssetKind::Audio
                    && asset.status == AssetStatus::Ready
            })
        {
            let category_gain = match source.category {
                quasar_project::gameplay::AudioCategory::Music => document.document.audio.music,
                quasar_project::gameplay::AudioCategory::Sfx => document.document.audio.sfx,
            };
            let settings = if source.looping {
                PlaybackSettings::LOOP
            } else {
                PlaybackSettings::DESPAWN
            }
            .with_volume(Volume::Linear(source.volume * category_gain))
            .with_spatial(source.spatial);
            commands.spawn((
                AudioPlayer::new(asset_server.load(asset.metadata.source_path.clone())),
                settings,
                Transform::default(),
                ChildOf(entity),
                DocumentAudioVoice,
            ));
        }
        if let Some(parent_id) = object.parent_id
            && let Some(&parent) = entities.get(&parent_id)
        {
            commands.entity(entity).insert(ChildOf(parent));
        }
    }
}

#[allow(clippy::too_many_arguments, clippy::type_complexity)]
fn collect_document_controller_input(
    keyboard: Res<ButtonInput<KeyCode>>,
    mut pending_jump: ResMut<PendingDocumentJump>,
    mouse: Res<AccumulatedMouseMotion>,
    mouse_buttons: Res<ButtonInput<MouseButton>>,
    window: Query<&Window, With<PrimaryWindow>>,
    mut cursor_options: Query<&mut CursorOptions, With<PrimaryWindow>>,
    mut controllers: Query<
        (
            Entity,
            &mut Transform,
            &mut CharacterControllerInput,
            &ControllerMouseSensitivity,
        ),
        (With<CharacterController>, Without<PlayerCameraSettings>),
    >,
    mut cameras: Query<
        (&mut Transform, &mut PlayerCameraSettings, &ChildOf),
        (With<PlayerCameraSettings>, Without<CharacterController>),
    >,
) {
    let mut focused = false;
    let mut cursor_locked = false;
    if let (Ok(window), Ok(mut cursor_options)) = (window.single(), cursor_options.single_mut()) {
        focused = window.focused;
        cursor_locked = cursor_options.grab_mode == CursorGrabMode::Locked;
        if keyboard.just_pressed(KeyCode::Escape) && cursor_locked {
            cursor_options.grab_mode = CursorGrabMode::None;
            cursor_options.visible = true;
            cursor_locked = false;
        } else if mouse_buttons.just_pressed(MouseButton::Left) && !cursor_locked {
            cursor_options.grab_mode = CursorGrabMode::Locked;
            cursor_options.visible = false;
            cursor_locked = true;
        }
    }
    if focused && keyboard.just_pressed(KeyCode::Space) {
        pending_jump.0 = true;
    }
    if !focused {
        pending_jump.0 = false;
    }
    let mut local = Vec2::ZERO;
    if keyboard.pressed(KeyCode::KeyA) || keyboard.pressed(KeyCode::ArrowLeft) {
        local.x -= 1.0;
    }
    if keyboard.pressed(KeyCode::KeyD) || keyboard.pressed(KeyCode::ArrowRight) {
        local.x += 1.0;
    }
    if keyboard.pressed(KeyCode::KeyW) || keyboard.pressed(KeyCode::ArrowUp) {
        local.y += 1.0;
    }
    if keyboard.pressed(KeyCode::KeyS) || keyboard.pressed(KeyCode::ArrowDown) {
        local.y -= 1.0;
    }
    for (entity, mut transform, mut input, sensitivity) in &mut controllers {
        if focused && cursor_locked {
            transform.rotate_y(-mouse.delta.x * sensitivity.0);
        }
        let right = transform.rotation * Vec3::X;
        let forward = transform.rotation * -Vec3::Z;
        let world = right * local.x + forward * local.y;
        input.movement = if focused {
            Vec2::new(world.x, -world.z)
        } else {
            Vec2::ZERO
        };
        input.jump_pressed = focused && cursor_locked && pending_jump.0;
        for (mut camera_transform, mut settings, parent) in &mut cameras {
            if parent.parent() == entity && focused && cursor_locked {
                settings.pitch = (settings.pitch - mouse.delta.y * settings.sensitivity)
                    .clamp(-1.48353, 1.48353);
                camera_transform.rotation = Quat::from_rotation_x(settings.pitch);
            }
        }
    }
}

fn consume_document_jump(mut pending_jump: ResMut<PendingDocumentJump>) {
    pending_jump.0 = false;
}

fn animate_document_doors(
    time: Res<Time<Fixed>>,
    mut doors: Query<(&mut Transform, &mut DoorMotion)>,
) {
    for (mut transform, mut door) in &mut doors {
        let Some(previous_elapsed) = door.elapsed_seconds else {
            continue;
        };
        let duration = door.duration_seconds;
        let elapsed = (previous_elapsed + time.delta_secs()).min(duration);
        let progress = (elapsed / duration).clamp(0.0, 1.0);
        transform.rotation = document_door_rotation(&door, progress);
        door.elapsed_seconds = (progress < 1.0).then_some(elapsed);
    }
}

fn document_door_rotation(door: &DoorMotion, progress: f32) -> Quat {
    let progress = progress.clamp(0.0, 1.0);
    let eased = progress * progress * (3.0 - 2.0 * progress);
    door.closed_rotation * Quat::from_rotation_y(door.open_angle_radians * eased)
}

#[allow(clippy::too_many_arguments, clippy::type_complexity)]
fn document_interact(
    keyboard: Res<ButtonInput<KeyCode>>,
    cursor_options: Query<&CursorOptions, With<PrimaryWindow>>,
    document: Res<ResolvedDocument>,
    asset_server: Res<AssetServer>,
    rapier_context: ReadRapierContext,
    controllers: Query<Entity, With<CharacterController>>,
    cameras: Query<(&GlobalTransform, &ChildOf), With<PlayerCameraSettings>>,
    object_ids: Query<&DocumentObjectId>,
    mut objects: Query<
        (Entity, &DocumentObjectId, &mut Transform, &GlobalTransform),
        Without<PlayerCameraSettings>,
    >,
    mut door_motions: Query<(&DocumentObjectId, &mut DoorMotion)>,
    mut commands: Commands,
    voices: Query<(), With<DocumentAudioVoice>>,
) {
    if !keyboard.just_pressed(KeyCode::KeyE)
        || !cursor_options
            .single()
            .is_ok_and(|options| options.grab_mode == CursorGrabMode::Locked)
    {
        return;
    }
    let Ok(player) = controllers.single() else {
        return;
    };
    let Some((camera_transform, _)) = cameras.iter().find(|(_, parent)| parent.parent() == player)
    else {
        return;
    };
    let origin = camera_transform.translation();
    let direction = (camera_transform.rotation() * Vec3::NEG_Z).normalize_or_zero();
    if direction == Vec3::ZERO {
        return;
    }
    let Ok(rapier) = rapier_context.single() else {
        return;
    };
    let Some((hit, _distance)) = rapier.cast_ray(
        origin,
        direction,
        2.0,
        true,
        QueryFilter::default().exclude_collider(player),
    ) else {
        return;
    };
    let Ok(hit_object_id) = object_ids.get(hit) else {
        return;
    };
    let object_id = hit_object_id._id;
    let Some(scene) = active_document_scene(&document.document).ok() else {
        return;
    };
    let Some(object) = scene.objects.iter().find(|object| object.id == object_id) else {
        return;
    };
    let Ok(Some(script)) = ScriptComponent::from_components(&object.components) else {
        return;
    };
    if !script.enabled {
        return;
    }
    let Some(script_asset) = document.asset_catalog.assets.iter().find(|asset| {
        asset.metadata.asset_id == script.asset_id
            && asset.metadata.kind == AssetKind::Script
            && asset.status == AssetStatus::Ready
    }) else {
        return;
    };
    let script_path = document.asset_root.join(&script_asset.metadata.source_path);
    let source = match fs::read_to_string(&script_path) {
        Ok(source) => source,
        Err(error) => {
            warn!(path = %script_path.display(), %error, "cannot read project interaction script");
            return;
        }
    };
    let valid_objects = scene
        .objects
        .iter()
        .map(|object| object.id)
        .collect::<HashSet<_>>();
    let valid_doors = enabled_kinematic_doors(scene);
    let valid_audio = document
        .asset_catalog
        .assets
        .iter()
        .filter_map(|asset| {
            (asset.metadata.kind == AssetKind::Audio && asset.status == AssetStatus::Ready)
                .then_some(asset.metadata.asset_id)
        })
        .collect::<HashSet<_>>();
    let gameplay = match quasar_runtime::gameplay::run_project_interaction(
        &script_asset.metadata.source_path,
        &source,
        object_id,
        &valid_objects,
        &valid_doors,
        &valid_audio,
    ) {
        Ok(commands) => commands,
        Err(error) => {
            warn!(%error, "project interaction callback failed; commands discarded");
            return;
        }
    };
    for action in gameplay {
        match action {
            ProjectGameplayCommand::RotateObjectY { object_id, radians } => {
                if let Some((_, _, mut transform, _)) =
                    objects.iter_mut().find(|(_, id, _, _)| id._id == object_id)
                {
                    transform.rotate_y(radians);
                }
            }
            ProjectGameplayCommand::OpenDoor { object_id } => {
                if let Some((_, mut motion)) =
                    door_motions.iter_mut().find(|(id, _)| id._id == object_id)
                {
                    motion.begin_open();
                }
            }
            ProjectGameplayCommand::PlayAudio { asset_id } => {
                if voices.iter().count() >= 32 {
                    warn!("project audio voice budget is full; one-shot was skipped");
                    continue;
                }
                let Some(asset) = document.asset_catalog.assets.iter().find(|asset| {
                    asset.metadata.asset_id == asset_id
                        && asset.metadata.kind == AssetKind::Audio
                        && asset.status == AssetStatus::Ready
                }) else {
                    continue;
                };
                let position = objects
                    .iter()
                    .find(|(_, id, _, _)| id._id == object_id)
                    .map(|(_, _, _, global)| global.translation())
                    .unwrap_or(origin);
                commands.spawn((
                    AudioPlayer::new(asset_server.load(asset.metadata.source_path.clone())),
                    PlaybackSettings::DESPAWN
                        .with_volume(Volume::Linear(document.document.audio.sfx))
                        .with_spatial(true),
                    Transform::from_translation(position),
                    DocumentAudioVoice,
                ));
            }
        }
    }
}

fn enabled_kinematic_doors(scene: &SceneDocument) -> HashSet<ObjectId> {
    scene
        .objects
        .iter()
        .filter_map(|object| {
            let door = DoorComponent::from_components(&object.components)
                .ok()
                .flatten()?;
            let body = RigidBodyComponent::from_components(&object.components)
                .ok()
                .flatten()?;
            let collider = ColliderComponent::from_components(&object.components)
                .ok()
                .flatten()?;
            (door.enabled
                && body.enabled
                && body.kind == quasar_project::gameplay::RigidBodyKind::Kinematic
                && collider.enabled)
                .then_some(object.id)
        })
        .collect()
}

fn parse_snapshot_argument(args: &[String]) -> Result<PathBuf, String> {
    let mut snapshot = None;
    let mut index = 0;
    while index < args.len() {
        if args[index] == "--snapshot" {
            if snapshot.is_some() {
                return Err("--snapshot may be supplied only once".to_owned());
            }
            index += 1;
            let value = args
                .get(index)
                .ok_or_else(|| "--snapshot requires a path".to_owned())?;
            snapshot = Some(PathBuf::from(value));
        } else if args[index] == "--screenshot" {
            index += 1;
            if index >= args.len() {
                return Err("--screenshot requires a path".to_owned());
            }
        } else if args[index] != "--validate-only"
            && args[index] != "--run-interaction"
            && args[index] != "--benchmark-60s"
            && args[index] != "--animation-smoke"
        {
            return Err(format!("unknown argument: {}", args[index]));
        }
        index += 1;
    }
    snapshot.ok_or_else(|| {
        "usage: quasar-player --snapshot <file> [--validate-only] [--screenshot <path>]".to_owned()
    })
}

fn parse_screenshot_argument(args: &[String]) -> Result<Option<String>, String> {
    let mut screenshot = None;
    let mut index = 0;
    while index < args.len() {
        if args[index] == "--screenshot" {
            if screenshot.is_some() {
                return Err("--screenshot may be supplied only once".to_owned());
            }
            index += 1;
            let path = args
                .get(index)
                .ok_or_else(|| "--screenshot requires a path".to_owned())?;
            if path.is_empty() {
                return Err("--screenshot path must not be empty".to_owned());
            }
            screenshot = Some(path.clone());
        }
        index += 1;
    }
    Ok(screenshot)
}

fn resolve_snapshot(path: &Path) -> Result<ResolvedSnapshot, String> {
    let snapshot_path = fs::canonicalize(path)
        .map_err(|error| format!("cannot resolve snapshot {}: {error}", path.display()))?;
    let document = ProjectSnapshot::read(&snapshot_path)?;
    let project_root = snapshot_path
        .parent()
        .ok_or_else(|| "snapshot has no parent directory".to_owned())?
        .to_path_buf();
    let asset_root =
        fs::canonicalize(project_root.join(&document.assets_directory)).map_err(|error| {
            format!(
                "cannot resolve assets directory '{}': {error}",
                document.assets_directory
            )
        })?;
    let mut resolved_paths = Vec::new();
    for (label, relative) in [
        ("prop model", document.scene.prop_model.as_str()),
        ("ambience audio", document.scene.ambience_audio.as_str()),
        ("door audio", document.scene.door_audio.as_str()),
        ("door script", document.scene.door_script.as_str()),
    ] {
        let resolved = fs::canonicalize(asset_root.join(relative))
            .map_err(|error| format!("cannot resolve snapshot {label} '{relative}': {error}"))?;
        if !resolved.starts_with(&asset_root) {
            return Err(format!(
                "snapshot {label} resolves outside the assets directory"
            ));
        }
        if !resolved.is_file() {
            return Err(format!(
                "snapshot {label} is not a file: {}",
                resolved.display()
            ));
        }
        resolved_paths.push(resolved);
    }
    let script_path = resolved_paths
        .get(3)
        .ok_or_else(|| "snapshot door script path is missing".to_owned())?;
    let door_script = fs::read_to_string(script_path)
        .map_err(|error| format!("cannot read door script {}: {error}", script_path.display()))?;
    if let Some(animation) = &document.scene.animation {
        for asset in &animation.assets {
            let resolved = fs::canonicalize(asset_root.join(&asset.path)).map_err(|error| {
                format!(
                    "cannot resolve snapshot animation asset '{}' at '{}': {error}",
                    asset.id, asset.path
                )
            })?;
            if !resolved.starts_with(&asset_root) {
                return Err(format!(
                    "snapshot animation asset '{}' resolves outside the assets directory",
                    asset.id
                ));
            }
            if !resolved.is_file() {
                return Err(format!(
                    "snapshot animation asset '{}' is not a file: {}",
                    asset.id,
                    resolved.display()
                ));
            }
        }
    }

    Ok(ResolvedSnapshot {
        project_root,
        asset_root,
        scene: document.scene,
        door_script,
    })
}

fn launch_player(
    snapshot: ResolvedSnapshot,
    benchmark: bool,
    screenshot: Option<String>,
    animation_smoke: bool,
) -> Result<(), String> {
    let asset_root = snapshot
        .asset_root
        .to_str()
        .ok_or_else(|| "assets directory path is not valid UTF-8".to_owned())?
        .to_owned();
    let mut app = App::new();
    let animation_data = AnimationProbeData {
        animation: snapshot.scene.animation.clone(),
    };
    let window_resolution = if benchmark { (1920, 1080) } else { (1280, 800) };
    app.add_plugins(
        DefaultPlugins
            .set(WindowPlugin {
                primary_window: Some(Window {
                    title: format!("{} — {PROBE_TITLE}", snapshot.scene.name),
                    name: Some("quasar.player_probe".to_owned()),
                    resolution: window_resolution.into(),
                    resizable: true,
                    ..default()
                }),
                ..default()
            })
            .set(AssetPlugin {
                file_path: asset_root,
                ..default()
            }),
    );
    app.insert_resource(TimestepMode::Fixed {
        dt: 1.0 / 60.0,
        substeps: 1,
    })
    .insert_resource(snapshot)
    .insert_resource(animation_data)
    .init_resource::<PlayerSessionState>()
    .add_plugins(RapierPhysicsPlugin::<NoUserData>::default().in_fixed_schedule())
    .add_plugins(quasar_runtime::physics::CharacterControllerPlugin)
    .add_plugins(AnimationProbePlugin)
    .add_systems(Startup, setup_player_scene)
    .add_systems(
        PreUpdate,
        (collect_player_input, interact_with_door).after(InputSystems),
    )
    .add_systems(Update, handle_session_audio_input);
    if benchmark {
        app.insert_resource(BenchmarkRun::default())
            .add_systems(PreUpdate, drive_benchmark_route.after(collect_player_input))
            .add_systems(Update, collect_benchmark_metrics);
    }
    if let Some(path) = screenshot {
        app.insert_resource(ScreenshotRequest(path))
            .add_systems(Update, capture_screenshot_after_render_warmup);
    }
    if animation_smoke {
        app.add_systems(Startup, start_animation_smoke.after(setup_player_scene))
            .add_systems(
                PreUpdate,
                keep_smoke_character_walking.after(collect_player_input),
            );
    }
    app.run();
    Ok(())
}

fn start_animation_smoke(mut animation: ResMut<AnimationWorkspaceState>) {
    animation.select_character_clip("Walking");
    animation.play_door_clip();
    info!("Started Stage 0 animation smoke: door_open + Walking");
}

fn keep_smoke_character_walking(mut animation: ResMut<AnimationWorkspaceState>) {
    animation.select_character_clip("Walking");
}

fn capture_screenshot_after_render_warmup(
    request: Res<ScreenshotRequest>,
    time: Res<Time>,
    mut commands: Commands,
    mut elapsed: Local<f32>,
    mut captured: Local<bool>,
) {
    if *captured {
        return;
    }
    *elapsed += time.delta_secs();
    if *elapsed >= 4.0 {
        commands
            .spawn(Screenshot::primary_window())
            .observe(save_to_disk(request.0.clone()));
        *captured = true;
    }
}

fn drive_benchmark_route(
    time: Res<Time>,
    mut benchmark: ResMut<BenchmarkRun>,
    mut controllers: Query<&mut CharacterControllerInput>,
) {
    benchmark.elapsed_seconds += f64::from(time.delta_secs());
    if benchmark.elapsed_seconds < BENCHMARK_WARMUP_SECONDS {
        return;
    }
    let route_time = benchmark.elapsed_seconds - BENCHMARK_WARMUP_SECONDS;
    let direction = benchmark_route_direction(route_time);
    for mut input in &mut controllers {
        input.movement = direction;
        input.jump_pressed = false;
    }
}

fn benchmark_route_direction(elapsed_seconds: f64) -> Vec2 {
    let route_time = elapsed_seconds.rem_euclid(8.0);
    let segment = (route_time / 2.0) as u8;
    if route_time % 2.0 >= 0.5 {
        Vec2::ZERO
    } else {
        match segment {
            0 => Vec2::Y,
            1 => Vec2::X,
            2 => Vec2::NEG_Y,
            _ => Vec2::NEG_X,
        }
    }
}

fn collect_benchmark_metrics(
    time: Res<Time>,
    mut benchmark: ResMut<BenchmarkRun>,
    mut exit: MessageWriter<AppExit>,
) {
    if benchmark.finished {
        return;
    }
    if benchmark.elapsed_seconds > BENCHMARK_WARMUP_SECONDS
        && benchmark.elapsed_seconds < BENCHMARK_WARMUP_SECONDS + BENCHMARK_SAMPLE_SECONDS
    {
        benchmark
            .frame_times_ms
            .push(f64::from(time.delta_secs()) * 1000.0);
    }
    if benchmark.elapsed_seconds >= BENCHMARK_WARMUP_SECONDS + BENCHMARK_SAMPLE_SECONDS {
        benchmark.finished = true;
        let mut sorted = benchmark.frame_times_ms.clone();
        sorted.sort_by(f64::total_cmp);
        let median = percentile(&sorted, 0.50);
        let p95 = percentile(&sorted, 0.95);
        println!(
            "BENCHMARK route=8s-W-D-S-A warmup={BENCHMARK_WARMUP_SECONDS:.0}s measured={BENCHMARK_SAMPLE_SECONDS:.0}s frames={} median_ms={median:.3} p95_ms={p95:.3}",
            sorted.len()
        );
        exit.write(AppExit::Success);
    }
}

fn percentile(sorted_samples: &[f64], percentile: f64) -> f64 {
    if sorted_samples.is_empty() {
        return f64::NAN;
    }
    let index = ((sorted_samples.len() - 1) as f64 * percentile).ceil() as usize;
    sorted_samples[index.min(sorted_samples.len() - 1)]
}

fn setup_player_scene(
    mut commands: Commands,
    snapshot: Res<ResolvedSnapshot>,
    asset_server: Res<AssetServer>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut state: ResMut<PlayerSessionState>,
) {
    commands.spawn((
        Camera3d::default(),
        Transform::from_xyz(4.5, 3.1, 6.5).looking_at(Vec3::new(0.0, 0.65, 0.0), Vec3::Y),
    ));
    for object in &snapshot.scene.objects {
        spawn_snapshot_object(
            &mut commands,
            object,
            &snapshot,
            &asset_server,
            &mut meshes,
            &mut materials,
        );
    }
    commands.spawn((
        DirectionalLight {
            illuminance: 11_000.0,
            shadow_maps_enabled: true,
            ..default()
        },
        Transform::from_rotation(Quat::from_euler(EulerRot::XYZ, -0.78, -0.62, 0.0)),
    ));
    commands.spawn((
        AudioPlayer::new(asset_server.load(&snapshot.scene.ambience_audio)),
        PlaybackSettings::LOOP.with_volume(Volume::Linear(0.12)),
        PlayerSessionAudio,
        Name::new("Room Ambience"),
    ));
    state.ambience_active = true;
    info!(
        scene = %snapshot.scene.name,
        project = %snapshot.project_root.display(),
        "Standalone Player started from snapshot"
    );
}

fn spawn_snapshot_object(
    commands: &mut Commands,
    object: &SnapshotObject,
    snapshot: &ResolvedSnapshot,
    asset_server: &AssetServer,
    meshes: &mut Assets<Mesh>,
    materials: &mut Assets<StandardMaterial>,
) {
    let transform = Transform::from_translation(Vec3::from_array(object.position))
        .with_scale(Vec3::from_array(object.scale));
    let name = Name::new(object.id.clone());
    match object.kind {
        SnapshotObjectKind::Floor => {
            commands.spawn((
                Mesh3d(meshes.add(Plane3d::default().mesh().size(12.0, 12.0))),
                MeshMaterial3d(materials.add(StandardMaterial {
                    base_color: Color::srgb(0.23, 0.28, 0.32),
                    perceptual_roughness: 0.92,
                    ..default()
                })),
                transform,
                Collider::cuboid(6.0, 0.1, 6.0),
                name,
            ));
        }
        SnapshotObjectKind::Wall => {
            commands.spawn((
                Mesh3d(meshes.add(Cuboid::new(8.0, 3.2, 0.18))),
                MeshMaterial3d(materials.add(StandardMaterial {
                    base_color: Color::srgb(0.30, 0.39, 0.49),
                    perceptual_roughness: 0.84,
                    ..default()
                })),
                transform,
                Collider::cuboid(4.0, 1.6, 0.09),
                name,
            ));
        }
        SnapshotObjectKind::Door => {
            commands.spawn((
                Mesh3d(meshes.add(Cuboid::new(1.1, 2.1, 0.12))),
                MeshMaterial3d(materials.add(StandardMaterial {
                    base_color: Color::srgb(0.45, 0.20, 0.08),
                    perceptual_roughness: 0.72,
                    ..default()
                })),
                transform,
                Collider::cuboid(0.55, 1.05, 0.06),
                PlayerDoor,
                SnapshotObjectId(object.id.clone()),
                name,
            ));
        }
        SnapshotObjectKind::Character => {
            let character = commands
                .spawn((
                    transform,
                    Visibility::default(),
                    Collider::capsule_y(0.8, 0.35),
                    SpatialListener::new(0.2),
                    KinematicCharacterController {
                        offset: CharacterLength::Absolute(0.01),
                        snap_to_ground: Some(CharacterLength::Absolute(0.12)),
                        autostep: Some(CharacterAutostep {
                            max_height: CharacterLength::Absolute(0.25),
                            min_width: CharacterLength::Absolute(0.2),
                            include_dynamic_bodies: false,
                        }),
                        max_slope_climb_angle: 45.0_f32.to_radians(),
                        min_slope_slide_angle: 30.0_f32.to_radians(),
                        ..default()
                    },
                    CharacterController::default(),
                    CharacterControllerInput::default(),
                    SnapshotObjectId(object.id.clone()),
                    name,
                ))
                .id();
            let animation_model = snapshot.scene.animation.as_ref().and_then(|animation| {
                let binding = animation
                    .imported_bindings
                    .iter()
                    .find(|binding| binding.object_id == object.id)?;
                animation
                    .assets
                    .iter()
                    .find(|asset| asset.id == binding.asset_id)
                    .map(|asset| asset.path.clone())
            });
            if let Some(path) = animation_model {
                commands.spawn((
                    WorldAssetRoot(asset_server.load(GltfAssetLabel::Scene(0).from_asset(path))),
                    stage0_character_model_transform(),
                    ChildOf(character),
                    Name::new("Imported Character Model"),
                ));
            } else {
                commands.entity(character).insert((
                    Mesh3d(meshes.add(Capsule3d::new(0.35, 1.6))),
                    MeshMaterial3d(materials.add(StandardMaterial {
                        base_color: Color::srgb(0.16, 0.68, 0.93),
                        perceptual_roughness: 0.72,
                        ..default()
                    })),
                ));
            }
        }
        SnapshotObjectKind::Prop => {
            commands.spawn((
                WorldAssetRoot(
                    asset_server.load(
                        GltfAssetLabel::Scene(0).from_asset(snapshot.scene.prop_model.clone()),
                    ),
                ),
                transform,
                name,
            ));
        }
    }
}

fn collect_player_input(
    keyboard: Res<ButtonInput<KeyCode>>,
    mut character_inputs: Query<&mut CharacterControllerInput>,
    mut animation_workspace: Option<ResMut<AnimationWorkspaceState>>,
) {
    let Ok(mut input) = character_inputs.single_mut() else {
        return;
    };
    *input = read_player_input(&keyboard);
    if let Some(animation_workspace) = animation_workspace.as_mut() {
        let source = if input.movement.length_squared() > 0.0 {
            "Walking"
        } else {
            "Idle"
        };
        animation_workspace.select_character_clip(source);
        animation_workspace.character_preview_playing = true;
    }
}

fn read_player_input(keyboard: &ButtonInput<KeyCode>) -> CharacterControllerInput {
    let mut movement = Vec2::ZERO;
    if keyboard.pressed(KeyCode::KeyD) {
        movement.x += 1.0;
    }
    if keyboard.pressed(KeyCode::KeyA) {
        movement.x -= 1.0;
    }
    if keyboard.pressed(KeyCode::KeyW) {
        movement.y += 1.0;
    }
    if keyboard.pressed(KeyCode::KeyS) {
        movement.y -= 1.0;
    }
    if movement.length_squared() > 1.0 {
        movement = movement.normalize();
    }
    CharacterControllerInput {
        movement,
        jump_pressed: keyboard.just_pressed(KeyCode::Space),
    }
}

#[allow(clippy::too_many_arguments)]
fn interact_with_door(
    keyboard: Res<ButtonInput<KeyCode>>,
    snapshot: Res<ResolvedSnapshot>,
    animation_data: Option<Res<AnimationProbeData>>,
    mut animation_workspace: Option<ResMut<AnimationWorkspaceState>>,
    asset_server: Res<AssetServer>,
    mut commands: Commands,
    mut state: ResMut<PlayerSessionState>,
    mut doors: Query<(Entity, &mut Transform), With<PlayerDoor>>,
) {
    if !keyboard.just_pressed(KeyCode::KeyE) || state.door_opened {
        return;
    }
    let actions = match run_door_interaction(&snapshot.door_script) {
        Ok(actions) => actions,
        Err(error) => {
            error!(%error, "Lua interaction failed; pending actions discarded");
            return;
        }
    };
    for action in actions {
        match action {
            GameplayCommand::OpenDoor => {
                if let Ok((entity, mut transform)) = doors.single_mut() {
                    if snapshot.scene.animation.is_some()
                        && let Some(workspace) = animation_workspace.as_mut()
                    {
                        workspace.play_door_clip();
                    } else {
                        transform.rotate_y(1.25);
                        commands.entity(entity).remove::<Collider>();
                    }
                    state.door_opened = true;
                }
            }
            GameplayCommand::PlayDoorSound => {
                if animation_data
                    .as_ref()
                    .is_some_and(|data| data.animation.is_some())
                {
                    continue;
                }
                commands.spawn((
                    AudioPlayer::new(asset_server.load(&snapshot.scene.door_audio)),
                    PlaybackSettings::DESPAWN
                        .with_volume(Volume::Linear(0.45))
                        .with_spatial(true),
                    Transform::from_xyz(1.35, 1.0, -2.82),
                    PlayerSessionAudio,
                    Name::new("Door Latch"),
                ));
            }
        }
    }
}

fn handle_session_audio_input(
    keyboard: Res<ButtonInput<KeyCode>>,
    snapshot: Res<ResolvedSnapshot>,
    asset_server: Res<AssetServer>,
    mut commands: Commands,
    mut state: ResMut<PlayerSessionState>,
    audio: PlayerSessionAudioQuery,
) {
    if keyboard.just_pressed(KeyCode::F10) {
        for (entity, sink, spatial_sink) in &audio {
            if let Some(sink) = sink {
                sink.stop();
            }
            if let Some(sink) = spatial_sink {
                sink.stop();
            }
            commands.entity(entity).despawn();
        }
        state.ambience_active = false;
    } else if keyboard.just_pressed(KeyCode::F9) && !state.ambience_active {
        commands.spawn((
            AudioPlayer::new(asset_server.load(&snapshot.scene.ambience_audio)),
            PlaybackSettings::LOOP.with_volume(Volume::Linear(0.12)),
            PlayerSessionAudio,
            Name::new("Room Ambience"),
        ));
        state.ambience_active = true;
    }
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use super::{
        DoorMotion, PlayerDoor, PlayerSessionAudio, PlayerSessionState, active_document_scene,
        benchmark_route_direction, document_door_rotation, enabled_kinematic_doors,
        handle_session_audio_input, interact_with_door, parse_screenshot_argument,
        parse_snapshot_argument, percentile, read_player_input,
        resolve_project_document_with_asset_root, resolve_snapshot, validate_document_model_assets,
    };
    use bevy::{
        asset::{AssetApp, AssetPlugin},
        audio::AudioSource,
        input::ButtonInput,
        math::Vec2,
        prelude::{App, KeyCode, MinimalPlugins, Name, PreUpdate, Quat, Transform, Update, With},
    };
    use bevy_rapier3d::prelude::Collider;
    use quasar_project::{
        assets::{ASSET_METADATA_VERSION, AssetId, AssetKind, AssetMetadata, ModelAssetComponent},
        document::{ProjectDocument, SceneDocument, SceneObjectDocument},
        gameplay::{
            AudioSourceComponent, CharacterControllerComponent, ColliderComponent, ColliderShape,
            DoorComponent, RigidBodyComponent, RigidBodyKind, TypedComponent,
        },
    };

    fn fixture_snapshot() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/fixtures/stage0-player/quasar.snapshot.json")
    }

    fn animation_fixture_snapshot() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/fixtures/stage0-animation/quasar.snapshot.json")
    }

    fn gameplay_fixture_root() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/stage4-gameplay")
    }

    struct TestProject(PathBuf);

    impl TestProject {
        fn new() -> Self {
            let nonce = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("system clock is after Unix epoch")
                .as_nanos();
            let root = std::env::temp_dir().join(format!(
                "quasar-player-assets-{}-{nonce}",
                std::process::id()
            ));
            std::fs::create_dir_all(&root).expect("temporary project directory can be created");
            Self(root)
        }
    }

    impl Drop for TestProject {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn document_with_model(asset_id: AssetId) -> ProjectDocument {
        let mut scene = SceneDocument::new("Main");
        let mut object = SceneObjectDocument::new("Model", None);
        object
            .components
            .push(ModelAssetComponent { asset_id }.into_component());
        scene.objects.push(object);
        ProjectDocument::new("Player asset test", scene)
    }

    fn write_model_metadata(project_root: &std::path::Path, asset_id: AssetId, source: &str) {
        let metadata = AssetMetadata {
            metadata_version: ASSET_METADATA_VERSION,
            asset_id,
            kind: AssetKind::Model,
            source_path: source.to_owned(),
            importer_id: "quasar.gltf.glb".to_owned(),
            importer_version: 1,
            import_settings: serde_json::json!({}),
            source_url: None,
            author: None,
            license: None,
            derived_files: Vec::new(),
        };
        let sidecar = project_root.join(source).with_file_name(format!(
            "{}.meta.json",
            Path::new(source).file_name().unwrap().to_string_lossy()
        ));
        std::fs::write(
            sidecar,
            serde_json::to_vec_pretty(&metadata).expect("asset metadata serializes"),
        )
        .expect("asset metadata can be written");
    }

    #[test]
    fn packaged_snapshot_resolves_all_resources_without_editor_assets() {
        let resolved = resolve_snapshot(&fixture_snapshot()).expect("fixture snapshot resolves");
        assert_eq!(resolved.scene.name, "Stage 0 Room");
        assert!(
            resolved
                .asset_root
                .join(&resolved.scene.prop_model)
                .is_file()
        );
        assert!(
            resolved
                .asset_root
                .join(&resolved.scene.ambience_audio)
                .is_file()
        );
        assert!(
            resolved
                .asset_root
                .join(&resolved.scene.door_audio)
                .is_file()
        );
        assert!(resolved.door_script.contains("door.open()"));
        assert!(
            resolved
                .scene
                .objects
                .iter()
                .any(|object| object.id == "character" && object.position == [-1.35, 1.12, 1.8])
        );
    }

    #[test]
    fn animation_fixture_resolves_declared_model_and_audio_assets() {
        let resolved = resolve_snapshot(&animation_fixture_snapshot())
            .expect("animation fixture resolves before Player launch");
        assert!(
            resolved
                .asset_root
                .join("Quasar/RobotExpressive.glb")
                .is_file()
        );
        assert!(
            resolved
                .asset_root
                .join("Quasar/audio/door-latch.wav")
                .is_file()
        );
    }

    #[test]
    fn persistent_gameplay_fixture_resolves_door_audio_and_full_physics_contract() {
        let root = gameplay_fixture_root();
        let document =
            resolve_project_document_with_asset_root(&root.join("quasar.project.json"), &root)
                .expect("Stage 4 gameplay fixture and its referenced assets resolve");
        let scene = active_document_scene(&document.document).unwrap();
        assert_eq!(scene.objects.len(), 8);
        assert_eq!(enabled_kinematic_doors(scene).len(), 1);
        assert!(scene.objects.iter().any(|object| {
            RigidBodyComponent::from_components(&object.components)
                .ok()
                .flatten()
                .is_some_and(|body| {
                    body.enabled && body.kind == quasar_project::gameplay::RigidBodyKind::Dynamic
                })
        }));
        assert!(scene.objects.iter().any(|object| {
            CharacterControllerComponent::from_components(&object.components)
                .ok()
                .flatten()
                .is_some_and(|controller| controller.enabled)
        }));
        assert!(scene.objects.iter().any(|object| {
            AudioSourceComponent::from_components(&object.components)
                .ok()
                .flatten()
                .is_some_and(|source| source.enabled && source.autoplay && source.looping)
        }));
        assert_eq!(document.asset_catalog.assets.len(), 3);
    }

    #[test]
    fn missing_animation_asset_fails_before_player_launch() {
        let source_path = animation_fixture_snapshot();
        let source = std::fs::read_to_string(&source_path).expect("animation fixture can be read");
        let mut document: quasar_project::ProjectSnapshot =
            serde_json::from_str(&source).expect("animation fixture JSON is valid");
        document
            .scene
            .animation
            .as_mut()
            .expect("animation extension exists")
            .assets[0]
            .path = "Quasar/not-present.glb".to_owned();
        let snapshot_path = source_path.with_file_name(format!(
            "missing-animation-{}.snapshot.json",
            std::process::id()
        ));
        std::fs::write(
            &snapshot_path,
            serde_json::to_vec(&document).expect("snapshot serializes"),
        )
        .expect("temporary snapshot can be written next to its fixture assets");

        let error =
            resolve_snapshot(&snapshot_path).expect_err("missing animation asset is rejected");
        assert!(
            error.contains("animation asset 'robot_expressive'"),
            "unexpected error: {error}"
        );
        std::fs::remove_file(snapshot_path).expect("temporary snapshot is removed");
    }

    #[test]
    fn snapshot_argument_is_required_and_cannot_be_duplicated() {
        assert!(parse_snapshot_argument(&[]).unwrap_err().contains("usage"));
        assert!(
            parse_snapshot_argument(&[
                "--snapshot".into(),
                "first.json".into(),
                "--snapshot".into(),
                "second.json".into(),
            ])
            .unwrap_err()
            .contains("only once")
        );
    }

    #[test]
    fn screenshot_argument_requires_one_nonempty_path() {
        assert_eq!(
            parse_screenshot_argument(&["--screenshot".into(), "frame.png".into()]).unwrap(),
            Some("frame.png".to_owned())
        );
        assert!(
            parse_screenshot_argument(&["--screenshot".into()])
                .unwrap_err()
                .contains("requires a path")
        );
        assert!(
            parse_screenshot_argument(&["--screenshot".into(), "".into()])
                .unwrap_err()
                .contains("must not be empty")
        );
        assert!(
            parse_screenshot_argument(&[
                "--screenshot".into(),
                "first.png".into(),
                "--screenshot".into(),
                "second.png".into(),
            ])
            .unwrap_err()
            .contains("only once")
        );
    }

    #[test]
    fn percentile_uses_sorted_samples_and_handles_empty_input() {
        assert_eq!(percentile(&[1.0, 2.0, 3.0, 4.0, 100.0], 0.50), 3.0);
        assert_eq!(percentile(&[1.0, 2.0, 3.0, 4.0, 100.0], 0.95), 100.0);
        assert!(percentile(&[], 0.95).is_nan());
    }

    #[test]
    fn benchmark_route_repeats_forward_right_back_left_with_rest_intervals() {
        assert_eq!(benchmark_route_direction(0.25), Vec2::Y);
        assert_eq!(benchmark_route_direction(0.75), Vec2::ZERO);
        assert_eq!(benchmark_route_direction(2.25), Vec2::X);
        assert_eq!(benchmark_route_direction(4.25), Vec2::NEG_Y);
        assert_eq!(benchmark_route_direction(6.25), Vec2::NEG_X);
        assert_eq!(benchmark_route_direction(8.25), Vec2::Y);
    }

    #[test]
    fn missing_snapshot_asset_fails_before_player_launch() {
        let unique = format!("quasar-player-invalid-{}", std::process::id());
        let temp_root = std::env::temp_dir().join(unique);
        let assets = temp_root.join("assets");
        std::fs::create_dir_all(&assets).expect("temporary assets directory exists");
        let source = std::fs::read_to_string(fixture_snapshot()).expect("fixture can be read");
        let mut document: quasar_project::ProjectSnapshot =
            serde_json::from_str(&source).expect("fixture JSON is valid");
        document.scene.prop_model = "Quasar/not-present.glb".to_owned();
        let snapshot_path = temp_root.join("missing.snapshot.json");
        std::fs::write(
            &snapshot_path,
            serde_json::to_vec(&document).expect("snapshot serializes"),
        )
        .expect("temporary snapshot can be written");

        let error = resolve_snapshot(&snapshot_path).expect_err("missing model is rejected");
        assert!(error.contains("prop model"), "unexpected error: {error}");
        std::fs::remove_dir_all(temp_root).expect("temporary files are removed");
    }

    #[test]
    fn player_accepts_ready_model_reference_and_rejects_missing_or_conflicting_assets() {
        let project = TestProject::new();
        let asset_id = AssetId::new();
        let document = document_with_model(asset_id);
        let document_path = project.0.join("quasar.project.json");
        document
            .save(&document_path)
            .expect("project document saves");
        std::fs::create_dir_all(project.0.join("Assets/Models"))
            .expect("asset folder can be created");

        let error = validate_document_model_assets(&project.0, &document)
            .expect_err("missing model reference prevents Player launch");
        assert!(
            error.contains("missing Model asset"),
            "unexpected error: {error}"
        );

        let source = "Assets/Models/model.glb";
        let fixture = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/fixtures/stage0-player/assets/Quasar/viewport-prop.glb");
        std::fs::copy(&fixture, project.0.join(source)).expect("valid GLB fixture copies");
        write_model_metadata(&project.0, asset_id, source);
        validate_document_model_assets(&project.0, &document)
            .expect("ready model reference is accepted");

        std::fs::remove_file(project.0.join(source)).expect("model source can be removed");
        let error = validate_document_model_assets(&project.0, &document)
            .expect_err("metadata with a missing model source prevents Player launch");
        assert!(error.contains("Missing"), "unexpected error: {error}");
        std::fs::copy(&fixture, project.0.join(source)).expect("model source can be restored");

        let second_source = "Assets/Models/model-copy.glb";
        std::fs::copy(&fixture, project.0.join(second_source)).expect("duplicate GLB copies");
        write_model_metadata(&project.0, asset_id, second_source);
        let error = validate_document_model_assets(&project.0, &document)
            .expect_err("duplicate AssetId prevents Player launch");
        assert!(error.contains("Conflict"), "unexpected error: {error}");
    }

    #[test]
    fn player_wasd_and_jump_input_are_mapped_and_diagonal_is_normalized() {
        let mut keys = ButtonInput::<KeyCode>::default();
        keys.press(KeyCode::KeyW);
        keys.press(KeyCode::KeyD);
        keys.press(KeyCode::Space);
        let input = read_player_input(&keys);

        assert!((input.movement.length() - 1.0).abs() < 1e-6);
        assert!(input.movement.x > 0.0 && input.movement.y > 0.0);
        assert!(input.jump_pressed);
    }

    #[test]
    fn authored_door_eases_from_closed_pose_to_configured_open_angle() {
        let door = DoorMotion {
            closed_rotation: Quat::from_rotation_y(0.2),
            open_angle_radians: 1.2,
            duration_seconds: 0.75,
            elapsed_seconds: None,
        };
        let closed = document_door_rotation(&door, 0.0);
        let midpoint = document_door_rotation(&door, 0.5);
        let open = document_door_rotation(&door, 1.0);
        assert!(closed.abs_diff_eq(door.closed_rotation, 1e-6));
        assert!(midpoint.abs_diff_eq(Quat::from_rotation_y(0.8), 1e-5));
        assert!(open.abs_diff_eq(Quat::from_rotation_y(1.4), 1e-5));
        assert!(document_door_rotation(&door, 2.0).abs_diff_eq(open, 1e-6));
    }

    #[test]
    fn only_enabled_doors_with_kinematic_bodies_and_colliders_are_interactable() {
        let mut scene = SceneDocument::new("Main");
        let mut door = SceneObjectDocument::new("Door", None);
        door.components
            .push(DoorComponent::default().into_document());
        door.components.push(
            RigidBodyComponent {
                kind: RigidBodyKind::Kinematic,
                ..RigidBodyComponent::default()
            }
            .into_document(),
        );
        door.components.push(
            ColliderComponent {
                enabled: true,
                center: [0.0; 3],
                shape: ColliderShape::Box { size: [1.0; 3] },
            }
            .into_document(),
        );
        let expected_id = door.id;
        scene.objects.push(door);

        let mut static_door = SceneObjectDocument::new("Static Door", None);
        static_door
            .components
            .push(DoorComponent::default().into_document());
        static_door
            .components
            .push(RigidBodyComponent::default().into_document());
        static_door.components.push(
            ColliderComponent {
                enabled: true,
                center: [0.0; 3],
                shape: ColliderShape::Box { size: [1.0; 3] },
            }
            .into_document(),
        );
        scene.objects.push(static_door);

        assert_eq!(
            enabled_kinematic_doors(&scene),
            std::collections::HashSet::from([expected_id])
        );
    }

    #[test]
    fn player_lua_interaction_opens_door_and_spawns_audio_only_on_success() {
        let mut snapshot = resolve_snapshot(&fixture_snapshot()).expect("fixture resolves");
        let mut app = App::new();
        app.add_plugins((MinimalPlugins, AssetPlugin::default()));
        app.init_asset::<AudioSource>();
        app.init_resource::<ButtonInput<KeyCode>>();
        app.insert_resource(snapshot.clone());
        app.init_resource::<PlayerSessionState>();
        app.add_systems(PreUpdate, interact_with_door);
        let door = app
            .world_mut()
            .spawn((
                PlayerDoor,
                Name::new("door"),
                Transform::default(),
                Collider::cuboid(0.5, 1.0, 0.1),
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
        assert!(door_entity.get::<Collider>().is_none());
        assert_ne!(
            door_entity.get::<Transform>().unwrap().rotation,
            Quat::IDENTITY
        );
        assert!(app.world().resource::<PlayerSessionState>().door_opened);
        let world = app.world_mut();
        let mut audio_query = world.query_filtered::<(), With<PlayerSessionAudio>>();
        assert_eq!(audio_query.iter(world).count(), 1);

        // A failing callback must leave both door state and audio unchanged.
        snapshot.door_script = "function on_interact() door.open(); error('failed') end".to_owned();
        app.insert_resource(snapshot);
        let second_door = app
            .world_mut()
            .spawn((
                PlayerDoor,
                Name::new("second door"),
                Transform::default(),
                Collider::cuboid(0.5, 1.0, 0.1),
            ))
            .id();
        app.world_mut()
            .resource_mut::<ButtonInput<KeyCode>>()
            .clear();
        app.world_mut()
            .resource_mut::<ButtonInput<KeyCode>>()
            .press(KeyCode::KeyE);
        app.world_mut()
            .resource_mut::<PlayerSessionState>()
            .door_opened = false;
        app.update();
        assert!(app.world().entity(second_door).get::<Collider>().is_some());
        let world = app.world_mut();
        let mut audio_query = world.query_filtered::<(), With<PlayerSessionAudio>>();
        assert_eq!(audio_query.iter(world).count(), 1);
    }

    #[test]
    fn player_stop_and_restart_changes_session_audio_count() {
        let snapshot = resolve_snapshot(&fixture_snapshot()).expect("fixture resolves");
        let mut app = App::new();
        app.add_plugins((MinimalPlugins, AssetPlugin::default()));
        app.init_asset::<AudioSource>();
        app.init_resource::<ButtonInput<KeyCode>>();
        app.insert_resource(snapshot);
        app.insert_resource(PlayerSessionState {
            ambience_active: true,
            door_opened: false,
        });
        app.add_systems(Update, handle_session_audio_input);
        app.world_mut().spawn(PlayerSessionAudio);
        app.finish();
        app.cleanup();

        app.world_mut()
            .resource_mut::<ButtonInput<KeyCode>>()
            .press(KeyCode::F10);
        app.update();
        assert!(!app.world().resource::<PlayerSessionState>().ambience_active);
        let world = app.world_mut();
        let mut audio_query = world.query_filtered::<(), With<PlayerSessionAudio>>();
        assert_eq!(audio_query.iter(world).count(), 0);

        app.world_mut()
            .resource_mut::<ButtonInput<KeyCode>>()
            .clear();
        app.world_mut()
            .resource_mut::<ButtonInput<KeyCode>>()
            .press(KeyCode::F9);
        app.update();
        assert!(app.world().resource::<PlayerSessionState>().ambience_active);
        let world = app.world_mut();
        let mut audio_query = world.query_filtered::<(), With<PlayerSessionAudio>>();
        assert_eq!(audio_query.iter(world).count(), 1);
    }
}
