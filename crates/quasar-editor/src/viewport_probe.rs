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
use quasar_project::{
    AnimatedTransformProperty, AnimationKeySnapshot, AnimationValueSnapshot,
    TransformAnimationTrackSnapshot,
};
use quasar_runtime::{
    animation::{
        AnimationProbeData, AnimationWorkspaceState, SnapshotObjectId,
        stage0_character_model_transform,
    },
    physics::{CharacterController, CharacterControllerInput},
};

use super::project_probe::EditorProject;
use crate::editor_style::{self as chrome, Icon};

const BENCHMARK_WARMUP_SECONDS: f64 = 5.0;
const BENCHMARK_SAMPLE_SECONDS: f64 = 60.0;

pub struct ViewportProbePlugin;

impl Plugin for ViewportProbePlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Startup, setup_viewport_probe)
            .init_resource::<AnimationWorkspaceUi>()
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

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum EditorWorkspaceTab {
    #[default]
    Scene,
    SceneTimeline,
    ClipPreview,
}

#[derive(Resource)]
struct AnimationWorkspaceUi {
    tab: EditorWorkspaceTab,
    selected_key_time: f32,
    key_translation: [f32; 3],
    key_yaw_degrees: f32,
    advanced: bool,
    console_open: bool,
}

impl Default for AnimationWorkspaceUi {
    fn default() -> Self {
        Self {
            tab: EditorWorkspaceTab::Scene,
            selected_key_time: 0.0,
            key_translation: [1.35, 1.02, -2.82],
            key_yaw_degrees: 0.0,
            advanced: false,
            console_open: false,
        }
    }
}

#[derive(Resource, Default)]
pub(super) struct ViewportInputFocus(pub(super) bool);

#[allow(clippy::too_many_arguments)]
fn setup_viewport_probe(
    mut commands: Commands,
    mut images: ResMut<Assets<Image>>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut egui_user_textures: ResMut<EguiUserTextures>,
    mut egui_global_settings: ResMut<EguiGlobalSettings>,
    asset_server: Res<AssetServer>,
    project: Res<EditorProject>,
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
            clear_color: ClearColorConfig::Custom(Color::srgb(0.11, 0.11, 0.12)),
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
        SnapshotObjectId("door".to_owned()),
        Name::new("Lua Door"),
    ));
    let character = commands
        .spawn((
            Transform::from_xyz(-1.35, 1.12, 1.8),
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
            SnapshotObjectId("character".to_owned()),
            Name::new("Physics Probe Character"),
        ))
        .id();
    if let Some(animation) = project.snapshot.scene.animation.as_ref()
        && let Some(binding) = animation
            .imported_bindings
            .iter()
            .find(|binding| binding.object_id == "character")
        && let Some(asset) = animation
            .assets
            .iter()
            .find(|asset| asset.id == binding.asset_id)
    {
        commands.spawn((
            WorldAssetRoot(
                asset_server.load(GltfAssetLabel::Scene(0).from_asset(asset.path.clone())),
            ),
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

#[allow(clippy::too_many_arguments)]
fn draw_editor_shell(
    mut contexts: EguiContexts,
    mut viewport: ResMut<ViewportSurface>,
    mut selection: ResMut<SelectionState>,
    mut viewport_focus: ResMut<ViewportInputFocus>,
    mut audio_controls: ResMut<AudioProbeControls>,
    audio_state: Res<AudioProbeState>,
    mut project: ResMut<EditorProject>,
    mut animation_data: ResMut<AnimationProbeData>,
    mut animation: ResMut<AnimationWorkspaceState>,
    mut workspace_ui: ResMut<AnimationWorkspaceUi>,
    window: Single<&Window, With<PrimaryWindow>>,
    scene_camera: Single<(&Camera, &GlobalTransform), With<SceneViewportCamera>>,
) -> Result {
    let ctx = contexts.ctx_mut()?;
    chrome::install(ctx);
    let mut viewport_ui = egui::Ui::new(
        ctx.clone(),
        "quasar_editor_viewport".into(),
        egui::UiBuilder::new()
            .layer_id(egui::LayerId::background())
            .max_rect(ctx.viewport_rect()),
    );

    egui::Panel::top("quasar_toolbar").frame(chrome::toolbar_frame()).show(&mut viewport_ui, |ui| {
        ui.horizontal(|ui| {
            if chrome::tab(ui, "Scene", workspace_ui.tab == EditorWorkspaceTab::Scene).clicked() {
                workspace_ui.tab = EditorWorkspaceTab::Scene;
            }
            ui.add_enabled(false, egui::Button::new("Code").min_size(egui::vec2(90.0, 34.0))).on_hover_text("Code workspace is planned.");
            ui.add_enabled(false, egui::Button::new("Assets").min_size(egui::vec2(90.0, 34.0))).on_hover_text("The asset library is available in ProjectDocument mode.");
            chrome::icon_button(ui, Icon::Plus, false, false, "Additional workspaces are planned.");
            if ui.button("Reload").on_hover_text("Reload the Stage 0 snapshot").clicked() {
                match project.reload() {
                    Ok(()) => {
                        animation_data.animation = project.snapshot.scene.animation.clone();
                        *animation = AnimationWorkspaceState::default();
                        animation.reset_door_clip();
                        workspace_ui.selected_key_time = 0.0;
                        workspace_ui.tab = EditorWorkspaceTab::Scene;
                    }
                    Err(error) => project.status = error,
                }
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.selectable_label(workspace_ui.advanced, "Advanced").clicked() { workspace_ui.advanced = true; }
                if ui.selectable_label(!workspace_ui.advanced, "Basic").clicked() { workspace_ui.advanced = false; }
                ui.separator();
                ui.add_enabled(false, egui::Button::new("Advisor")).on_hover_text("Advisor rules are planned.");
                chrome::icon_button(ui, Icon::Play, false, false, "This technical scene is already running; separate Play controls are planned.");
                ui.separator();
                chrome::icon_button(ui, Icon::Redo, false, false, "Undo/Redo is available in ProjectDocument mode.");
                chrome::icon_button(ui, Icon::Undo, false, false, "Undo/Redo is available in ProjectDocument mode.");
                if chrome::icon_button(ui, Icon::Save, false, true, "Save snapshot").clicked() && let Err(error) = project.save() { project.status = error; }
            });
        });
    });

    egui::Panel::top("quasar_workspace_tabs")
        .frame(chrome::toolbar_frame())
        .show(&mut viewport_ui, |ui| {
            ui.horizontal(|ui| {
                ui.selectable_value(&mut workspace_ui.tab, EditorWorkspaceTab::Scene, "Scene");
                ui.selectable_value(
                    &mut workspace_ui.tab,
                    EditorWorkspaceTab::SceneTimeline,
                    "Scene Timeline",
                );
                ui.selectable_value(
                    &mut workspace_ui.tab,
                    EditorWorkspaceTab::ClipPreview,
                    "Clip Preview",
                );
                ui.separator();
                ui.weak("Animation workspace");
            });
        });

    egui::Panel::bottom("quasar_console").frame(chrome::panel_frame()).show(&mut viewport_ui, |ui| {
        ui.horizontal(|ui| {
            if ui.selectable_label(workspace_ui.console_open, "Console").clicked() { workspace_ui.console_open = !workspace_ui.console_open; }
            ui.label(egui::RichText::new(&project.status).small().color(chrome::MUTED));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.label(egui::RichText::new(if project.dirty { "Unsaved changes" } else { "Saved" }).small().color(if project.dirty { chrome::ACCENT } else { chrome::MUTED }));
            });
        });
        if workspace_ui.console_open {
            ui.separator();
            ui.weak("Stage 0 · runtime and animation preview");
            ui.label("WASD / Space · move     E · interact     F10 · stop audio     F9 · restart audio");
        }
    });

    egui::Panel::left("quasar_hierarchy")
        .frame(chrome::panel_frame())
        .default_size(250.0)
        .min_size(190.0)
        .resizable(true)
        .show(&mut viewport_ui, |ui| {
            ui.heading("Hierarchy");
            ui.separator();
            ui.weak("Room.scene");
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
        .frame(chrome::panel_frame())
        .default_size(300.0)
        .min_size(240.0)
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
            if workspace_ui.advanced {
                ui.add_space(12.0);
                ui.label("Render target");
                ui.monospace(format!(
                    "{} × {} px",
                    viewport.physical_size.x, viewport.physical_size.y
                ));
                ui.weak(format!("DPI {:.0}%", window.scale_factor() * 100.0));
            }
        });

    if workspace_ui.tab == EditorWorkspaceTab::SceneTimeline {
        egui::Panel::bottom("quasar_animation_timeline")
            .frame(chrome::panel_frame())
            .default_size(230.0)
            .resizable(true)
            .show(&mut viewport_ui, |ui| {
                draw_scene_timeline(
                    ui,
                    &mut project,
                    &mut animation_data,
                    &mut animation,
                    &mut workspace_ui,
                );
            });
    } else if workspace_ui.tab == EditorWorkspaceTab::ClipPreview {
        egui::Panel::bottom("quasar_animation_clip_preview")
            .frame(chrome::panel_frame())
            .default_size(170.0)
            .resizable(true)
            .show(&mut viewport_ui, |ui| {
                draw_clip_preview(ui, &project, &mut animation, &mut workspace_ui);
            });
    }

    egui::CentralPanel::default()
        .frame(chrome::panel_frame())
        .show(&mut viewport_ui, |ui| {
            ui.horizontal(|ui| {
                chrome::tab(
                    ui,
                    if project.dirty {
                        "Room.scene *"
                    } else {
                        "Room.scene"
                    },
                    true,
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.weak("Stage 0");
                });
            });
            ui.separator();
            ui.horizontal(|ui| {
                ui.strong(match workspace_ui.tab {
                    EditorWorkspaceTab::Scene => "Perspective",
                    EditorWorkspaceTab::SceneTimeline => "Scene Timeline · Door animation",
                    EditorWorkspaceTab::ClipPreview => "Clip Preview · GLB character",
                });
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
            let response = ui.add(
                egui::Image::new((viewport.texture_id, available))
                    .corner_radius(7)
                    .sense(egui::Sense::click()),
            );
            viewport.desired_points = Vec2::new(response.rect.width(), response.rect.height());
            viewport_focus.0 = response.hovered();

            let rail = egui::Rect::from_min_size(
                response.rect.min + egui::vec2(10.0, 10.0),
                egui::vec2(42.0, 164.0),
            );
            let rail_response = ui.scope_builder(
                egui::UiBuilder::new()
                    .max_rect(rail)
                    .layout(egui::Layout::top_down(egui::Align::Center)),
                |ui| {
                    egui::Frame::new()
                        .fill(chrome::PANEL)
                        .stroke(egui::Stroke::new(1.0, chrome::BORDER))
                        .corner_radius(8)
                        .inner_margin(4)
                        .show(ui, |ui| {
                            chrome::icon_button(
                                ui,
                                Icon::Select,
                                true,
                                true,
                                "Select a prop in the viewport",
                            );
                            chrome::icon_button(
                                ui,
                                Icon::Move,
                                false,
                                false,
                                "Transform tools are available in ProjectDocument mode.",
                            );
                            chrome::icon_button(
                                ui,
                                Icon::Rotate,
                                false,
                                false,
                                "Transform tools are available in ProjectDocument mode.",
                            );
                            chrome::icon_button(
                                ui,
                                Icon::Scale,
                                false,
                                false,
                                "Transform tools are available in ProjectDocument mode.",
                            );
                        });
                },
            );
            if rail_response
                .response
                .rect
                .contains(ctx.pointer_hover_pos().unwrap_or_default())
            {
                viewport_focus.0 = false;
            }

            if response.clicked()
                && !rail.contains(response.interact_pointer_pos().unwrap_or_default())
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

fn draw_scene_timeline(
    ui: &mut egui::Ui,
    project: &mut EditorProject,
    animation_data: &mut AnimationProbeData,
    animation: &mut AnimationWorkspaceState,
    workspace_ui: &mut AnimationWorkspaceUi,
) {
    let Some(snapshot_animation) = project.snapshot.scene.animation.as_ref() else {
        ui.colored_label(
            egui::Color32::LIGHT_RED,
            "This project has no animation data.",
        );
        return;
    };
    let Some(clip) = snapshot_animation
        .object_clips
        .iter()
        .find(|clip| clip.id == animation.door_clip_id)
    else {
        ui.colored_label(
            egui::Color32::LIGHT_RED,
            "The selected scene clip is missing.",
        );
        return;
    };
    let clip_duration = clip.duration_seconds;
    let clip_id = clip.id.clone();
    ui.horizontal(|ui| {
        ui.label(format!("Clip: {} · target {}", clip.id, clip.object_id));
        if ui.small_button("Play").clicked() {
            animation.play_door_clip();
        }
        if ui.small_button("Pause").clicked() {
            animation.door_playing = false;
        }
        if ui.small_button("Stop / Reset").clicked() {
            animation.reset_door_clip();
        }
        ui.label(format!(
            "{:.3} / {:.3} s",
            animation.door_time_seconds, clip_duration
        ));
    });
    let mut timeline_time = animation.door_time_seconds;
    if ui
        .add(egui::Slider::new(&mut timeline_time, 0.0..=clip_duration).text("Time"))
        .changed()
    {
        animation.scrub_door_clip(timeline_time);
    }

    ui.horizontal_wrapped(|ui| {
        ui.label("Keys:");
        for track in &clip.tracks {
            for key in &track.keys {
                let value = match track.property {
                    AnimatedTransformProperty::Translation => "Position",
                    AnimatedTransformProperty::Rotation => "Rotation",
                };
                if ui
                    .small_button(format!("{value} · {:.3}s", key.time_seconds))
                    .clicked()
                {
                    workspace_ui.selected_key_time = key.time_seconds;
                    animation.scrub_door_clip(key.time_seconds);
                    if let AnimationValueSnapshot::Vec3(position) = &key.value {
                        workspace_ui.key_translation = *position;
                    }
                    if let AnimationValueSnapshot::Quaternion(rotation) = &key.value {
                        workspace_ui.key_yaw_degrees = Quat::from_array(*rotation)
                            .to_euler(EulerRot::YXZ)
                            .0
                            .to_degrees();
                    }
                }
            }
        }
    });
    ui.separator();
    ui.label("Set key values at the selected time");
    ui.horizontal(|ui| {
        ui.label("Position");
        for value in &mut workspace_ui.key_translation {
            ui.add(
                egui::DragValue::new(value)
                    .speed(0.01)
                    .range(-100.0..=100.0),
            );
        }
        ui.label("Yaw °");
        ui.add(egui::DragValue::new(&mut workspace_ui.key_yaw_degrees).speed(0.5));
        ui.add(
            egui::DragValue::new(&mut workspace_ui.selected_key_time)
                .speed(0.01)
                .range(0.0..=clip_duration)
                .suffix(" s"),
        );
        if ui.button("Add / Update Key").clicked() {
            let rotation = Quat::from_rotation_y(workspace_ui.key_yaw_degrees.to_radians());
            let changed = project
                .snapshot
                .scene
                .animation
                .as_mut()
                .and_then(|animation| {
                    animation
                        .object_clips
                        .iter_mut()
                        .find(|clip| clip.id == clip_id)
                })
                .is_some_and(|clip| {
                    upsert_animation_key(
                        clip,
                        AnimatedTransformProperty::Translation,
                        workspace_ui.selected_key_time,
                        AnimationValueSnapshot::Vec3(workspace_ui.key_translation),
                    );
                    upsert_animation_key(
                        clip,
                        AnimatedTransformProperty::Rotation,
                        workspace_ui.selected_key_time,
                        AnimationValueSnapshot::Quaternion(rotation.to_array()),
                    );
                    true
                });
            if changed {
                project.dirty = true;
                project.status = "Animation key edited — save to keep this change".to_owned();
                animation_data.animation = project.snapshot.scene.animation.clone();
                animation.scrub_door_clip(workspace_ui.selected_key_time);
            }
        }
    });
    if !animation.diagnostics.is_empty() {
        ui.separator();
        for diagnostic in &animation.diagnostics {
            ui.colored_label(egui::Color32::LIGHT_RED, diagnostic);
        }
    }
}

fn upsert_animation_key(
    clip: &mut quasar_project::ObjectAnimationClipSnapshot,
    property: AnimatedTransformProperty,
    time_seconds: f32,
    value: AnimationValueSnapshot,
) {
    let track = if let Some(track) = clip
        .tracks
        .iter_mut()
        .find(|track| track.property == property)
    {
        track
    } else {
        clip.tracks.push(TransformAnimationTrackSnapshot {
            property,
            interpolation: quasar_project::AnimationInterpolation::SmoothStep,
            keys: Vec::new(),
        });
        clip.tracks.last_mut().expect("the track was just inserted")
    };
    if let Some(existing) = track
        .keys
        .iter_mut()
        .find(|key| (key.time_seconds - time_seconds).abs() < 0.0005)
    {
        existing.value = value;
        existing.time_seconds = time_seconds;
    } else {
        track.keys.push(AnimationKeySnapshot {
            time_seconds,
            value,
        });
    }
    track
        .keys
        .sort_by(|left, right| left.time_seconds.total_cmp(&right.time_seconds));
}

fn draw_clip_preview(
    ui: &mut egui::Ui,
    project: &EditorProject,
    animation: &mut AnimationWorkspaceState,
    _workspace_ui: &mut AnimationWorkspaceUi,
) {
    let Some(snapshot_animation) = project.snapshot.scene.animation.as_ref() else {
        ui.colored_label(
            egui::Color32::LIGHT_RED,
            "This project has no imported animation data.",
        );
        return;
    };
    let Some(binding) = snapshot_animation
        .imported_bindings
        .iter()
        .find(|binding| binding.object_id == "character")
    else {
        ui.colored_label(
            egui::Color32::LIGHT_RED,
            "No imported clips are bound to the character.",
        );
        return;
    };
    ui.horizontal_wrapped(|ui| {
        ui.label("Character clips:");
        for clip in &binding.clips {
            let selected = animation.character_clip_source == clip.source_clip_id;
            if ui.selectable_label(selected, &clip.display_name).clicked() {
                animation.select_character_clip(clip.source_clip_id.clone());
            }
        }
        ui.separator();
        if ui
            .small_button(if animation.character_preview_playing {
                "Pause"
            } else {
                "Play"
            })
            .clicked()
        {
            animation.character_preview_playing = !animation.character_preview_playing;
        }
        if ui.small_button("Restart").clicked() {
            animation.seek_character_clip(0.0);
            animation.character_preview_playing = true;
        }
    });
    let mut preview_time = animation.character_preview_time_seconds;
    let clip_duration = animation.character_duration_seconds.max(0.001);
    let seek_changed = ui
        .add(
            egui::Slider::new(&mut preview_time, 0.0..=clip_duration).text(format!(
                "{} · preview time",
                animation.character_clip_source
            )),
        )
        .changed();
    let seek_requested = ui.button("Seek").clicked();
    if seek_changed || seek_requested {
        animation.character_preview_time_seconds = preview_time;
        animation.character_seek_revision = animation.character_seek_revision.wrapping_add(1);
        animation.character_preview_playing = false;
    }
    if !animation.diagnostics.is_empty() {
        ui.separator();
        for diagnostic in &animation.diagnostics {
            ui.colored_label(egui::Color32::LIGHT_RED, diagnostic);
        }
    }
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
        read_viewport_input, requested_viewport_size, resize_viewport_target, upsert_animation_key,
    };
    use bevy::{
        asset::Assets,
        math::{UVec2, Vec2, Vec3},
        prelude::{App, Image, Update, Window, default},
        render::render_resource::TextureFormat,
        window::{PrimaryWindow, WindowResolution},
    };
    use bevy_egui::egui;
    use quasar_project::{
        AnimatedTransformProperty, AnimationInterpolation, AnimationKeySnapshot,
        AnimationValueSnapshot, ObjectAnimationClipSnapshot, TransformAnimationTrackSnapshot,
    };
    use quasar_runtime::physics::CharacterControllerInput;

    const MIN: Vec3 = Vec3::splat(-1.0);
    const MAX: Vec3 = Vec3::splat(1.0);

    #[test]
    fn timeline_key_edits_insert_in_order_and_update_existing_time() {
        let mut clip = ObjectAnimationClipSnapshot {
            id: "door_open".to_owned(),
            object_id: "door".to_owned(),
            duration_seconds: 1.0,
            tracks: vec![TransformAnimationTrackSnapshot {
                property: AnimatedTransformProperty::Translation,
                interpolation: AnimationInterpolation::SmoothStep,
                keys: vec![
                    AnimationKeySnapshot {
                        time_seconds: 0.0,
                        value: AnimationValueSnapshot::Vec3([0.0; 3]),
                    },
                    AnimationKeySnapshot {
                        time_seconds: 1.0,
                        value: AnimationValueSnapshot::Vec3([1.0; 3]),
                    },
                ],
            }],
            events: Vec::new(),
        };
        upsert_animation_key(
            &mut clip,
            AnimatedTransformProperty::Translation,
            0.75,
            AnimationValueSnapshot::Vec3([0.75; 3]),
        );
        upsert_animation_key(
            &mut clip,
            AnimatedTransformProperty::Translation,
            0.75,
            AnimationValueSnapshot::Vec3([0.8; 3]),
        );

        let keys = &clip.tracks[0].keys;
        assert_eq!(keys.len(), 3);
        assert_eq!(keys[1].time_seconds, 0.75);
        assert_eq!(keys[1].value, AnimationValueSnapshot::Vec3([0.8; 3]));
    }

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
