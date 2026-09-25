use std::{
    env, fs,
    path::{Path, PathBuf},
};

use bevy::{
    asset::AssetPlugin,
    audio::Volume,
    gltf::GltfAssetLabel,
    input::InputSystems,
    prelude::*,
    render::view::screenshot::{Screenshot, save_to_disk},
    window::WindowPlugin,
    world_serialization::WorldAssetRoot,
};
use bevy_rapier3d::prelude::{
    CharacterAutostep, CharacterLength, Collider, KinematicCharacterController, NoUserData,
    RapierPhysicsPlugin, TimestepMode,
};
use quasar_project::{ProjectSnapshot, SceneSnapshot, SnapshotObject, SnapshotObjectKind};
use quasar_runtime::{
    gameplay::{GameplayCommand, run_door_interaction},
    physics::{CharacterController, CharacterControllerInput},
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
    With<PlayerSessionAudio>,
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

    let snapshot_arg = parse_snapshot_argument(&args)?;
    let screenshot = parse_screenshot_argument(&args)?;
    let snapshot = resolve_snapshot(&snapshot_arg)?;
    let benchmark = args.iter().any(|argument| argument == "--benchmark-60s");
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

    launch_player(snapshot, benchmark, screenshot)
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
) -> Result<(), String> {
    let asset_root = snapshot
        .asset_root
        .to_str()
        .ok_or_else(|| "assets directory path is not valid UTF-8".to_owned())?
        .to_owned();
    let mut app = App::new();
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
    .init_resource::<PlayerSessionState>()
    .add_plugins(RapierPhysicsPlugin::<NoUserData>::default().in_fixed_schedule())
    .add_plugins(quasar_runtime::physics::CharacterControllerPlugin)
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
    app.run();
    Ok(())
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
                name,
            ));
        }
        SnapshotObjectKind::Character => {
            commands.spawn((
                Mesh3d(meshes.add(Capsule3d::new(0.35, 1.6))),
                MeshMaterial3d(materials.add(StandardMaterial {
                    base_color: Color::srgb(0.16, 0.68, 0.93),
                    perceptual_roughness: 0.72,
                    ..default()
                })),
                transform,
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
                name,
            ));
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
) {
    let Ok(mut input) = character_inputs.single_mut() else {
        return;
    };
    *input = read_player_input(&keyboard);
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

fn interact_with_door(
    keyboard: Res<ButtonInput<KeyCode>>,
    snapshot: Res<ResolvedSnapshot>,
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
                    transform.rotate_y(1.25);
                    commands.entity(entity).remove::<Collider>();
                    state.door_opened = true;
                }
            }
            GameplayCommand::PlayDoorSound => {
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
    use std::path::PathBuf;

    use super::{
        PlayerDoor, PlayerSessionAudio, PlayerSessionState, benchmark_route_direction,
        handle_session_audio_input, interact_with_door, parse_screenshot_argument,
        parse_snapshot_argument, percentile, read_player_input, resolve_snapshot,
    };
    use bevy::{
        asset::{AssetApp, AssetPlugin},
        audio::AudioSource,
        input::ButtonInput,
        math::Vec2,
        prelude::{App, KeyCode, MinimalPlugins, Name, PreUpdate, Quat, Transform, Update, With},
    };
    use bevy_rapier3d::prelude::Collider;

    fn fixture_snapshot() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/fixtures/stage0-player/quasar.snapshot.json")
    }

    fn animation_fixture_snapshot() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/fixtures/stage0-animation/quasar.snapshot.json")
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
