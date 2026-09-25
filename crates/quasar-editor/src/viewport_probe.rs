use super::audio_probe::{AudioProbeCommand, AudioProbeControls, AudioProbeState};
use crate::gameplay_probe::ProbeDoor;
use bevy::{
    camera::RenderTarget,
    gltf::GltfAssetLabel,
    input::InputSystems,
    prelude::*,
    render::render_resource::{Extent3d, TextureFormat},
    window::PrimaryWindow,
    world_serialization::WorldAssetRoot,
};
use bevy_egui::{
    EguiContexts, EguiGlobalSettings, EguiPrimaryContextPass, EguiTextureHandle, EguiUserTextures,
    PrimaryEguiContext, egui,
};
use bevy_rapier3d::prelude::{
    CharacterAutostep, CharacterLength, Collider, KinematicCharacterController,
};
use quasar_runtime::physics::{CharacterController, CharacterControllerInput};

const BENCHMARK_WARMUP_SECONDS: f64 = 5.0;
const BENCHMARK_SAMPLE_SECONDS: f64 = 60.0;

pub struct ViewportProbePlugin;

impl Plugin for ViewportProbePlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Startup, setup_viewport_probe)
            .add_systems(EguiPrimaryContextPass, draw_editor_shell)
            .add_systems(
                PreUpdate,
                collect_viewport_character_input.after(InputSystems),
            )
            .add_systems(Update, resize_viewport_target);
        if std::env::args().any(|argument| argument == "--benchmark-60s") {
            app.init_resource::<EditorBenchmarkRun>()
                .add_systems(
                    PreUpdate,
                    drive_editor_benchmark_route.after(collect_viewport_character_input),
                )
                .add_systems(Update, collect_editor_benchmark_metrics);
        }
    }
}

#[derive(Component)]
struct SceneViewportCamera;

#[derive(Resource)]
struct ViewportSurface {
    image: Handle<Image>,
    texture_id: egui::TextureId,
    physical_size: UVec2,
    desired_points: Vec2,
}

#[derive(Resource, Default)]
struct SelectionState {
    prop_selected: bool,
}

#[derive(Resource, Default)]
pub(super) struct ViewportInputFocus(pub(super) bool);

fn setup_viewport_probe(
    mut commands: Commands,
    mut images: ResMut<Assets<Image>>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut egui_user_textures: ResMut<EguiUserTextures>,
    mut egui_global_settings: ResMut<EguiGlobalSettings>,
    asset_server: Res<AssetServer>,
) {
    egui_global_settings.auto_create_primary_context = false;

    let physical_size = UVec2::new(1280, 720);
    let image = images.add(Image::new_target_texture(
        physical_size.x,
        physical_size.y,
        TextureFormat::Rgba8UnormSrgb,
        None,
    ));
    let texture_id = egui_user_textures.add_image(EguiTextureHandle::Strong(image.clone()));
    commands.insert_resource(ViewportSurface {
        image: image.clone(),
        texture_id,
        physical_size,
        desired_points: Vec2::new(960.0, 640.0),
    });
    commands.init_resource::<SelectionState>();
    commands.init_resource::<ViewportInputFocus>();

    // The window camera hosts the editor UI; the scene camera renders only to the viewport texture.
    commands.spawn((Camera2d, PrimaryEguiContext));

    commands.spawn((
        Camera3d::default(),
        Camera {
            order: -1,
            clear_color: ClearColorConfig::Custom(Color::srgb(0.055, 0.075, 0.11)),
            ..default()
        },
        RenderTarget::Image(image.into()),
        SceneViewportCamera,
        Transform::from_xyz(4.5, 3.1, 6.5).looking_at(Vec3::new(0.0, 0.65, 0.0), Vec3::Y),
    ));

    commands.spawn((
        Mesh3d(meshes.add(Plane3d::default().mesh().size(12.0, 12.0))),
        MeshMaterial3d(materials.add(StandardMaterial {
            base_color: Color::srgb(0.23, 0.28, 0.32),
            perceptual_roughness: 0.92,
            ..default()
        })),
        Transform::from_xyz(0.0, -0.03, 0.0),
        Collider::cuboid(6.0, 0.1, 6.0),
    ));
    commands.spawn((
        Mesh3d(meshes.add(Cuboid::new(8.0, 3.2, 0.18))),
        MeshMaterial3d(materials.add(StandardMaterial {
            base_color: Color::srgb(0.30, 0.39, 0.49),
            perceptual_roughness: 0.84,
            ..default()
        })),
        Transform::from_xyz(0.0, 1.55, -3.0),
        Collider::cuboid(4.0, 1.6, 0.09),
    ));
    commands.spawn((
        Mesh3d(meshes.add(Cuboid::new(1.1, 2.1, 0.12))),
        MeshMaterial3d(materials.add(StandardMaterial {
            base_color: Color::srgb(0.45, 0.20, 0.08),
            perceptual_roughness: 0.72,
            ..default()
        })),
        Transform::from_xyz(1.35, 1.02, -2.82),
        Collider::cuboid(0.55, 1.05, 0.06),
        ProbeDoor,
        Name::new("Lua Door"),
    ));
    commands.spawn((
        Mesh3d(meshes.add(Capsule3d::new(0.35, 1.6))),
        MeshMaterial3d(materials.add(StandardMaterial {
            base_color: Color::srgb(0.16, 0.68, 0.93),
            perceptual_roughness: 0.72,
            ..default()
        })),
        Transform::from_xyz(-1.35, 1.12, 1.8),
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
        Name::new("Physics Probe Character"),
    ));
    commands.spawn((
        DirectionalLight {
            illuminance: 11_000.0,
            shadow_maps_enabled: true,
            ..default()
        },
        Transform::from_rotation(Quat::from_euler(EulerRot::XYZ, -0.78, -0.62, 0.0)),
    ));
    commands.spawn((
        WorldAssetRoot(
            asset_server.load(GltfAssetLabel::Scene(0).from_asset("Quasar/viewport-prop.glb")),
        ),
        Transform::from_xyz(0.0, 0.72, 0.0).with_scale(Vec3::splat(1.35)),
        Name::new("GLB Viewport Prop"),
    ));
}

fn draw_editor_shell(
    mut contexts: EguiContexts,
    mut viewport: ResMut<ViewportSurface>,
    mut selection: ResMut<SelectionState>,
    mut viewport_focus: ResMut<ViewportInputFocus>,
    mut audio_controls: ResMut<AudioProbeControls>,
    audio_state: Res<AudioProbeState>,
    window: Single<&Window, With<PrimaryWindow>>,
    scene_camera: Single<(&Camera, &GlobalTransform), With<SceneViewportCamera>>,
) -> Result {
    let ctx = contexts.ctx_mut()?;
    let mut viewport_ui = egui::Ui::new(
        ctx.clone(),
        "quasar_editor_viewport".into(),
        egui::UiBuilder::new()
            .layer_id(egui::LayerId::background())
            .max_rect(ctx.viewport_rect()),
    );

    egui::Panel::top("quasar_toolbar").show(&mut viewport_ui, |ui| {
        ui.horizontal(|ui| {
            ui.heading("Quasar Engine");
            ui.separator();
            ui.label("Stage 0 · Viewport / Physics");
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.label(format!("DPI {:.0}%", window.scale_factor() * 100.0));
            });
        });
    });

    egui::Panel::left("quasar_hierarchy")
        .default_size(205.0)
        .resizable(true)
        .show(&mut viewport_ui, |ui| {
            ui.heading("Hierarchy");
            ui.separator();
            ui.label("Scene");
            ui.indent("scene_root", |ui| {
                ui.label("Room");
                ui.indent("room_children", |ui| {
                    ui.label("Floor");
                    ui.label("Back wall");
                    let selected = selection.prop_selected;
                    if ui.selectable_label(selected, "GLB Viewport Prop").clicked() {
                        selection.prop_selected = true;
                    }
                });
            });
        });

    egui::Panel::right("quasar_inspector")
        .default_size(235.0)
        .resizable(true)
        .show(&mut viewport_ui, |ui| {
            ui.heading("Inspector");
            ui.separator();
            if selection.prop_selected {
                ui.label("GLB Viewport Prop");
                ui.label("Selected by viewport picking");
                ui.monospace("Scene0 · static mesh");
            } else {
                ui.label("Nothing selected");
                ui.label("Click the orange prop in the viewport.");
            }
            ui.add_space(12.0);
            ui.label("Render target");
            ui.monospace(format!(
                "{} × {} px",
                viewport.physical_size.x, viewport.physical_size.y
            ));
        });

    egui::CentralPanel::default().show(&mut viewport_ui, |ui| {
        ui.horizontal(|ui| {
            ui.strong("Viewport");
            ui.separator();
            ui.label("WASD / Space · E door");
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.small_button("Stop").clicked() {
                    audio_controls.pending = Some(AudioProbeCommand::Stop);
                }
                if ui.small_button("Restart ambience").clicked() {
                    audio_controls.pending = Some(AudioProbeCommand::Restart);
                }
                ui.label(if audio_state.ambience_active {
                    "Ambience: on"
                } else {
                    "Ambience: off"
                });
                ui.separator();
                ui.label("F10 Stop · F9 Restart");
            });
        });
        ui.add_space(6.0);
        let available = ui.available_size().max(egui::vec2(96.0, 96.0));
        let response =
            ui.add(egui::Image::new((viewport.texture_id, available)).sense(egui::Sense::click()));
        viewport.desired_points = Vec2::new(response.rect.width(), response.rect.height());
        viewport_focus.0 = response.hovered();

        if response.clicked()
            && let Some(pointer) = response.interact_pointer_pos()
        {
            let uv = (pointer - response.rect.min) / response.rect.size();
            let physical_position = Vec2::new(
                uv.x * viewport.physical_size.x as f32,
                uv.y * viewport.physical_size.y as f32,
            );
            let (camera, transform) = *scene_camera;
            selection.prop_selected = camera
                .viewport_to_world(transform, physical_position)
                .is_ok_and(|ray| {
                    ray_intersects_aabb(
                        ray.origin,
                        ray.direction.as_vec3(),
                        Vec3::new(-0.75, 0.10, -0.58),
                        Vec3::new(0.75, 1.95, 0.58),
                    )
                });
        }
    });

    Ok(())
}

fn collect_viewport_character_input(
    keyboard: Res<ButtonInput<KeyCode>>,
    viewport_focus: Res<ViewportInputFocus>,
    mut character_inputs: Query<&mut CharacterControllerInput>,
) {
    let Ok(mut input) = character_inputs.single_mut() else {
        return;
    };

    *input = read_viewport_input(&keyboard, viewport_focus.0);
}

fn read_viewport_input(
    keyboard: &ButtonInput<KeyCode>,
    viewport_focused: bool,
) -> CharacterControllerInput {
    if !viewport_focused {
        return CharacterControllerInput::default();
    }

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

fn resize_viewport_target(
    mut viewport: ResMut<ViewportSurface>,
    window: Single<&Window, With<PrimaryWindow>>,
    mut images: ResMut<Assets<Image>>,
) {
    let scale = window.scale_factor();
    let requested = requested_viewport_size(viewport.desired_points, scale);
    if requested == viewport.physical_size {
        return;
    }

    if let Some(mut image) = images.get_mut(&viewport.image) {
        image.resize(Extent3d {
            width: requested.x,
            height: requested.y,
            ..default()
        });
        viewport.physical_size = requested;
    }
}

fn requested_viewport_size(desired_points: Vec2, scale_factor: f32) -> UVec2 {
    (desired_points * scale_factor)
        .round()
        .as_uvec2()
        .clamp(UVec2::splat(96), UVec2::splat(4096))
}

#[derive(Resource, Default)]
struct EditorBenchmarkRun {
    elapsed_seconds: f64,
    frame_times_ms: Vec<f64>,
    finished: bool,
}

fn drive_editor_benchmark_route(
    time: Res<Time>,
    mut benchmark: ResMut<EditorBenchmarkRun>,
    mut controllers: Query<&mut CharacterControllerInput>,
) {
    benchmark.elapsed_seconds += f64::from(time.delta_secs());
    if benchmark.elapsed_seconds < BENCHMARK_WARMUP_SECONDS {
        return;
    }
    let direction =
        editor_benchmark_route_direction(benchmark.elapsed_seconds - BENCHMARK_WARMUP_SECONDS);
    for mut input in &mut controllers {
        input.movement = direction;
        input.jump_pressed = false;
    }
}

fn editor_benchmark_route_direction(elapsed_seconds: f64) -> Vec2 {
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

fn collect_editor_benchmark_metrics(
    time: Res<Time>,
    mut benchmark: ResMut<EditorBenchmarkRun>,
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
        println!(
            "BENCHMARK app=editor route=8s-W-D-S-A warmup={BENCHMARK_WARMUP_SECONDS:.0}s measured={BENCHMARK_SAMPLE_SECONDS:.0}s frames={} median_ms={:.3} p95_ms={:.3}",
            sorted.len(),
            percentile(&sorted, 0.50),
            percentile(&sorted, 0.95),
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

fn ray_intersects_aabb(origin: Vec3, direction: Vec3, min: Vec3, max: Vec3) -> bool {
    let mut near = 0.0_f32;
    let mut far = f32::INFINITY;
    for axis in 0..3 {
        let origin_axis = origin[axis];
        let direction_axis = direction[axis];
        if direction_axis.abs() < 1e-6 {
            if origin_axis < min[axis] || origin_axis > max[axis] {
                return false;
            }
            continue;
        }
        let inverse = direction_axis.recip();
        let mut first = (min[axis] - origin_axis) * inverse;
        let mut second = (max[axis] - origin_axis) * inverse;
        if first > second {
            std::mem::swap(&mut first, &mut second);
        }
        near = near.max(first);
        far = far.min(second);
        if near > far {
            return false;
        }
    }
    far >= 0.0
}

#[cfg(test)]
mod tests {
    use super::{
        ViewportSurface, editor_benchmark_route_direction, percentile, ray_intersects_aabb,
        read_viewport_input, requested_viewport_size, resize_viewport_target,
    };
    use bevy::{
        asset::Assets,
        math::{UVec2, Vec2, Vec3},
        prelude::{App, Image, Update, Window, default},
        render::render_resource::TextureFormat,
        window::{PrimaryWindow, WindowResolution},
    };
    use bevy_egui::egui;
    use quasar_runtime::physics::CharacterControllerInput;

    const MIN: Vec3 = Vec3::splat(-1.0);
    const MAX: Vec3 = Vec3::splat(1.0);

    #[test]
    fn ray_hits_box_from_outside() {
        assert!(ray_intersects_aabb(
            Vec3::new(0.0, 0.0, 3.0),
            Vec3::NEG_Z,
            MIN,
            MAX,
        ));
    }

    #[test]
    fn ray_parallel_to_box_and_outside_misses() {
        assert!(!ray_intersects_aabb(
            Vec3::new(2.0, 0.0, 3.0),
            Vec3::NEG_Z,
            MIN,
            MAX,
        ));
    }

    #[test]
    fn ray_starting_inside_box_hits() {
        assert!(ray_intersects_aabb(Vec3::ZERO, Vec3::X, MIN, MAX,));
    }

    #[test]
    fn viewport_target_tracks_dpi_and_clamps_oversized_targets() {
        assert_eq!(
            requested_viewport_size(Vec2::new(640.0, 480.0), 1.5),
            UVec2::new(960, 720)
        );
        assert_eq!(
            requested_viewport_size(Vec2::new(10_000.0, 10_000.0), 2.0),
            UVec2::splat(4096)
        );
        assert_eq!(
            requested_viewport_size(Vec2::new(1.0, 1.0), 1.0),
            UVec2::splat(96)
        );
    }

    #[test]
    fn resize_system_reallocates_viewport_image_when_available_area_changes() {
        let mut app = App::new();
        app.insert_resource(Assets::<Image>::default());
        let image = app
            .world_mut()
            .resource_mut::<Assets<Image>>()
            .add(Image::new_target_texture(
                320,
                240,
                TextureFormat::Rgba8UnormSrgb,
                None,
            ));
        app.insert_resource(ViewportSurface {
            image: image.clone(),
            texture_id: egui::TextureId::default(),
            physical_size: UVec2::new(320, 240),
            desired_points: Vec2::new(640.0, 480.0),
        });
        app.world_mut().spawn((
            Window {
                resolution: WindowResolution::new(1280, 720).with_scale_factor_override(1.5),
                ..default()
            },
            PrimaryWindow,
        ));
        app.add_systems(Update, resize_viewport_target);

        app.update();
        let resized = app
            .world()
            .resource::<Assets<Image>>()
            .get(&image)
            .expect("render target remains in image assets");
        assert_eq!(
            (
                resized.texture_descriptor.size.width,
                resized.texture_descriptor.size.height
            ),
            (960, 720)
        );

        app.world_mut()
            .resource_mut::<ViewportSurface>()
            .desired_points = Vec2::new(800.0, 450.0);
        app.update();
        let resized_again = app
            .world()
            .resource::<Assets<Image>>()
            .get(&image)
            .expect("render target remains in image assets");
        assert_eq!(
            (
                resized_again.texture_descriptor.size.width,
                resized_again.texture_descriptor.size.height
            ),
            (1200, 675)
        );
    }

    #[test]
    fn editor_benchmark_uses_the_same_repeatable_route_and_percentiles() {
        assert_eq!(editor_benchmark_route_direction(0.25), Vec2::Y);
        assert_eq!(editor_benchmark_route_direction(0.75), Vec2::ZERO);
        assert_eq!(editor_benchmark_route_direction(2.25), Vec2::X);
        assert_eq!(editor_benchmark_route_direction(4.25), Vec2::NEG_Y);
        assert_eq!(editor_benchmark_route_direction(6.25), Vec2::NEG_X);
        assert_eq!(editor_benchmark_route_direction(8.25), Vec2::Y);
        assert_eq!(percentile(&[1.0, 2.0, 3.0, 4.0, 5.0], 0.95), 5.0);
    }

    #[test]
    fn viewport_keyboard_input_is_gated_by_focus() {
        use bevy::{input::ButtonInput, prelude::KeyCode};

        let mut keys = ButtonInput::<KeyCode>::default();
        keys.press(KeyCode::KeyW);
        keys.press(KeyCode::Space);
        let unfocused = read_viewport_input(&keys, false);
        assert_eq!(unfocused, CharacterControllerInput::default());

        let focused = read_viewport_input(&keys, true);
        assert_eq!(focused.movement, Vec2::Y);
        assert!(focused.jump_pressed);
    }
}
