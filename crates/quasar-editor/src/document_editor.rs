//! First editable ProjectDocument shell: hierarchy, inspector and a lightweight transform gizmo.

use std::collections::HashMap;

use bevy::{
    prelude::*,
    window::{Window, WindowPlugin},
};
use bevy_egui::{EguiContexts, EguiPlugin, EguiPrimaryContextPass, egui};
use quasar_project::{
    commands::SceneCommand,
    document::{
        ObjectId, ProjectDocument, SceneDocument, SceneId, SceneObjectDocument, TransformDocument,
    },
};

use crate::{
    document_session::{EditorDocumentSession, TransformGestureId},
    mcp_session::EditorMcpSession,
};

#[derive(Resource, Default)]
struct Selection(Option<ObjectId>);

#[derive(Resource, Default)]
struct UiState {
    name_edit: Option<(ObjectId, String)>,
    gesture: Option<(TransformGestureId, ObjectId)>,
    gizmo_mode: GizmoMode,
    status: String,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum GizmoMode {
    #[default]
    Move,
    Rotate,
    Scale,
}

#[derive(Component)]
struct DocumentObjectVisual;

#[derive(Resource, Default)]
struct RenderedRevision(Option<u64>);

pub(crate) fn document_editor_app(session: EditorDocumentSession, mcp: EditorMcpSession) -> App {
    let mut app = App::new();
    app.add_plugins(DefaultPlugins.set(WindowPlugin {
        primary_window: Some(Window {
            title: "Quasar Engine".into(),
            name: Some("quasar.editor".into()),
            resolution: (1440, 900).into(),
            resizable: true,
            ..default()
        }),
        ..default()
    }))
    .add_plugins(EguiPlugin::default())
    .insert_resource(session)
    .insert_resource(mcp)
    .init_resource::<Selection>()
    .init_resource::<UiState>()
    .init_resource::<RenderedRevision>()
    .add_systems(Startup, setup_document_viewport)
    .add_systems(Update, sync_document_scene)
    .add_systems(EguiPrimaryContextPass, draw_document_editor);
    app
}

fn setup_document_viewport(mut commands: Commands) {
    commands.spawn((
        Camera3d::default(),
        Transform::from_xyz(7.0, 6.0, 9.0).looking_at(Vec3::ZERO, Vec3::Y),
    ));
    commands.spawn((
        DirectionalLight {
            illuminance: 8500.0,
            shadow_maps_enabled: true,
            ..default()
        },
        Transform::from_rotation(Quat::from_euler(EulerRot::XYZ, -0.8, -0.6, 0.0)),
    ));
}

fn sync_document_scene(
    mut commands: Commands,
    session: Res<EditorDocumentSession>,
    mut rendered_revision: ResMut<RenderedRevision>,
    old_objects: Query<Entity, With<DocumentObjectVisual>>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    let Ok(snapshot) = session.snapshot() else {
        return;
    };
    if rendered_revision.0 == Some(snapshot.revision) {
        return;
    }
    for entity in &old_objects {
        commands.entity(entity).despawn();
    }
    let Some(scene) = active_scene(&snapshot.document) else {
        return;
    };
    let world = scene_world_transforms(scene);
    for object in &scene.objects {
        let transform = world.get(&object.id).copied().unwrap_or_default();
        let color = Color::srgb(0.3, 0.65, 0.95);
        commands.spawn((
            DocumentObjectVisual,
            Mesh3d(meshes.add(Cuboid::new(1.0, 1.0, 1.0))),
            MeshMaterial3d(materials.add(StandardMaterial {
                base_color: color,
                ..default()
            })),
            transform,
        ));
    }
    rendered_revision.0 = Some(snapshot.revision);
}

fn draw_document_editor(
    mut contexts: EguiContexts,
    session: Res<EditorDocumentSession>,
    mut selection: ResMut<Selection>,
    mut ui_state: ResMut<UiState>,
) -> Result {
    let Ok(snapshot) = session.snapshot() else {
        return Ok(());
    };
    let Some(scene) = active_scene(&snapshot.document) else {
        return Ok(());
    };
    let scene_id = scene.id;
    let objects = scene.objects.clone();
    let ctx = contexts.ctx_mut()?;
    let mut editor_ui = egui::Ui::new(
        ctx.clone(),
        "quasar_document_editor".into(),
        egui::UiBuilder::new()
            .layer_id(egui::LayerId::background())
            .max_rect(ctx.viewport_rect()),
    );

    egui::Panel::top("document_toolbar").show(&mut editor_ui, |ui| {
        ui.horizontal(|ui| {
            ui.heading(&snapshot.document.name);
            ui.separator();
            if ui.button("＋ Object").clicked() {
                let object = SceneObjectDocument::new(next_object_name(&objects), selection.0);
                match session.apply_command(
                    SceneCommand::CreateObject {
                        scene_id,
                        object: object.clone(),
                    },
                    snapshot.revision,
                ) {
                    Ok(_) => selection.0 = Some(object.id),
                    Err(error) => ui_state.status = error,
                }
            }
            if ui
                .add_enabled(selection.0.is_some(), egui::Button::new("Delete"))
                .clicked()
                && let Some(object_id) = selection.0
            {
                match session.apply_command(
                    SceneCommand::DeleteObject {
                        scene_id,
                        object_id,
                    },
                    snapshot.revision,
                ) {
                    Ok(_) => selection.0 = None,
                    Err(error) => ui_state.status = error,
                }
            }
            ui.separator();
            if ui
                .add_enabled(snapshot.undo_depth > 0, egui::Button::new("Undo"))
                .clicked()
            {
                match session.undo(snapshot.revision) {
                    Ok(_) => {}
                    Err(error) => ui_state.status = error,
                }
            }
            if ui
                .add_enabled(snapshot.redo_depth > 0, egui::Button::new("Redo"))
                .clicked()
            {
                match session.redo(snapshot.revision) {
                    Ok(_) => {}
                    Err(error) => ui_state.status = error,
                }
            }
            if ui.button("Save").clicked() {
                match session.save(snapshot.revision) {
                    Ok(_) => ui_state.status = "Saved".into(),
                    Err(error) => ui_state.status = error,
                }
            }
            ui.label(if snapshot.dirty {
                "● Unsaved"
            } else {
                "Saved"
            });
        });
    });

    egui::Panel::left("hierarchy")
        .resizable(true)
        .default_size(230.0)
        .show(&mut editor_ui, |ui| {
            ui.horizontal(|ui| {
                ui.heading("Hierarchy");
                if ui.button("＋").on_hover_text("Create object").clicked() {
                    let object = SceneObjectDocument::new(next_object_name(&objects), selection.0);
                    match session.apply_command(
                        SceneCommand::CreateObject {
                            scene_id,
                            object: object.clone(),
                        },
                        snapshot.revision,
                    ) {
                        Ok(_) => selection.0 = Some(object.id),
                        Err(error) => ui_state.status = error,
                    }
                }
            });
            ui.separator();
            for object in &objects {
                let depth = hierarchy_depth(object.id, &objects);
                let label = format!("{}{}", "　".repeat(depth), object.name);
                if ui
                    .selectable_label(selection.0 == Some(object.id), label)
                    .clicked()
                {
                    selection.0 = Some(object.id);
                }
            }
        });

    egui::Panel::right("inspector")
        .resizable(true)
        .default_size(290.0)
        .show(&mut editor_ui, |ui| {
            ui.heading("Inspector");
            ui.separator();
            if let Some(object) = selection
                .0
                .and_then(|id| objects.iter().find(|object| object.id == id))
            {
                draw_inspector(
                    ui,
                    &session,
                    &mut ui_state,
                    scene_id,
                    snapshot.revision,
                    object,
                );
            } else {
                ui.label("Select an object to inspect it.");
            }
        });

    egui::CentralPanel::default()
        .frame(egui::Frame::NONE)
        .show(&mut editor_ui, |ui| {
            ui.horizontal(|ui| {
                for (mode, label) in [
                    (GizmoMode::Move, "Move"),
                    (GizmoMode::Rotate, "Rotate"),
                    (GizmoMode::Scale, "Scale"),
                ] {
                    if ui
                        .selectable_label(ui_state.gizmo_mode == mode, label)
                        .clicked()
                    {
                        ui_state.gizmo_mode = mode;
                    }
                }
            });
            ui.separator();
            ui.label(format!("{} · {} objects", scene.name, objects.len()));
            ui.label("Viewport preview uses placeholder cubes for scene objects.");
            draw_gizmo(ui, &session, &mut selection, &mut ui_state, scene_id);
            ui.with_layout(egui::Layout::bottom_up(egui::Align::LEFT), |ui| {
                ui.label(&ui_state.status);
            });
        });
    Ok(())
}

#[allow(clippy::collapsible_if)]
fn draw_inspector(
    ui: &mut egui::Ui,
    session: &EditorDocumentSession,
    state: &mut UiState,
    scene_id: SceneId,
    revision: u64,
    object: &SceneObjectDocument,
) {
    let object_id = object.id;
    let current_name = state
        .name_edit
        .as_ref()
        .filter(|(id, _)| *id == object_id)
        .map(|(_, name)| name.clone())
        .unwrap_or_else(|| object.name.clone());
    let mut name = current_name;
    let response = ui.add(egui::TextEdit::singleline(&mut name).hint_text("Object name"));
    state.name_edit = Some((object_id, name.clone()));
    if response.lost_focus() && name != object.name {
        match session.apply_command(
            SceneCommand::RenameObject {
                scene_id,
                object_id,
                name,
            },
            revision,
        ) {
            Ok(_) => state.status = "Renamed object".into(),
            Err(error) => state.status = error,
        }
    }
    ui.label(format!("ID  {}", object_id.0));
    ui.separator();
    let mut transform = object.local_transform;
    let mut interaction = TransformEditInteraction::default();
    ui.label("Translation");
    interaction.merge(vector3_controls(ui, &mut transform.translation));
    ui.label("Rotation · quaternion");
    interaction.merge(vector4_controls(ui, &mut transform.rotation_xyzw));
    ui.label("Scale");
    interaction.merge(vector3_controls(ui, &mut transform.scale));
    if interaction.started {
        match session.begin_transform_gesture("Inspector transform", revision) {
            Ok(id) => state.gesture = Some((id, object_id)),
            Err(error) => state.status = error,
        }
    }
    if interaction.changed {
        let command = SceneCommand::SetTransform {
            scene_id,
            object_id,
            transform,
        };
        if interaction.dragging {
            if let Some((id, _active_object)) =
                state.gesture.filter(|(_, active)| *active == object_id)
            {
                match session.update_transform_gesture(
                    id,
                    command,
                    session
                        .snapshot()
                        .map(|snapshot| snapshot.revision)
                        .unwrap_or(revision),
                ) {
                    Ok(_) => {}
                    Err(error) => state.status = error,
                }
            }
        } else {
            match session.apply_command(command, revision) {
                Ok(_) => state.status = "Updated transform".into(),
                Err(error) => state.status = error,
            }
        }
    }
    if interaction.stopped {
        if let Some((id, _)) = state.gesture.take() {
            if let Err(error) = session.finish_transform_gesture(id) {
                state.status = error;
            }
        }
    }
}

#[derive(Default)]
struct TransformEditInteraction {
    changed: bool,
    dragging: bool,
    started: bool,
    stopped: bool,
}

impl TransformEditInteraction {
    fn merge(&mut self, other: Self) {
        self.changed |= other.changed;
        self.dragging |= other.dragging;
        self.started |= other.started;
        self.stopped |= other.stopped;
    }
}

fn vector3_controls(ui: &mut egui::Ui, value: &mut [f32; 3]) -> TransformEditInteraction {
    let mut interaction = TransformEditInteraction::default();
    ui.horizontal(|ui| {
        for index in 0..3 {
            ui.label(["X", "Y", "Z"][index]);
            let response = ui.add(
                egui::DragValue::new(&mut value[index])
                    .speed(0.05)
                    .range(-10000.0..=10000.0),
            );
            interaction.changed |= response.changed();
            interaction.dragging |= response.dragged();
            interaction.started |= response.drag_started();
            interaction.stopped |= response.drag_stopped();
        }
    });
    interaction
}

fn vector4_controls(ui: &mut egui::Ui, value: &mut [f32; 4]) -> TransformEditInteraction {
    let mut interaction = TransformEditInteraction::default();
    ui.horizontal(|ui| {
        for component in value.iter_mut() {
            let response = ui.add(
                egui::DragValue::new(component)
                    .speed(0.01)
                    .range(-1.0..=1.0),
            );
            interaction.changed |= response.changed();
            interaction.dragging |= response.dragged();
            interaction.started |= response.drag_started();
            interaction.stopped |= response.drag_stopped();
        }
    });
    interaction
}

#[allow(clippy::collapsible_if)]
fn draw_gizmo(
    ui: &mut egui::Ui,
    session: &EditorDocumentSession,
    selection: &mut Selection,
    state: &mut UiState,
    scene_id: SceneId,
) {
    ui.add_space(12.0);
    ui.label("Transform gizmo");
    ui.horizontal(|ui| {
        for (axis, color) in [
            (0, egui::Color32::from_rgb(220, 80, 80)),
            (1, egui::Color32::from_rgb(90, 200, 120)),
            (2, egui::Color32::from_rgb(90, 150, 240)),
        ] {
            let response = ui.add(
                egui::Button::new(["X axis", "Y axis", "Z axis"][axis])
                    .fill(color)
                    .sense(egui::Sense::drag()),
            );
            if response.drag_started() {
                if let Some(object_id) = selection.0 {
                    if let Ok(snapshot) = session.snapshot() {
                        if let Ok(id) = session.begin_transform_gesture(
                            format!(
                                "{:?} {}",
                                state.gizmo_mode,
                                "XYZ".chars().nth(axis).unwrap()
                            ),
                            snapshot.revision,
                        ) {
                            state.gesture = Some((id, object_id));
                        }
                    }
                }
            }
            if response.dragged() {
                if let (Some((gesture_id, object_id)), Ok(snapshot)) =
                    (state.gesture, session.snapshot())
                {
                    if object_id == selection.0.unwrap_or_default() {
                        if let Some(object) = active_scene(&snapshot.document).and_then(|scene| {
                            scene.objects.iter().find(|object| object.id == object_id)
                        }) {
                            let mut transform = object.local_transform;
                            let delta = (response.drag_delta().x - response.drag_delta().y) * 0.01;
                            match state.gizmo_mode {
                                GizmoMode::Move => transform.translation[axis] += delta,
                                GizmoMode::Scale => {
                                    transform.scale[axis] =
                                        (transform.scale[axis] + delta).max(0.01)
                                }
                                GizmoMode::Rotate => {
                                    let mut unit = [0.0; 3];
                                    unit[axis] = 1.0;
                                    let rotation = Quat::from_xyzw(
                                        transform.rotation_xyzw[0],
                                        transform.rotation_xyzw[1],
                                        transform.rotation_xyzw[2],
                                        transform.rotation_xyzw[3],
                                    );
                                    let next = Quat::from_axis_angle(Vec3::from_array(unit), delta)
                                        * rotation;
                                    transform.rotation_xyzw = [next.x, next.y, next.z, next.w];
                                }
                            }
                            if let Err(error) = session.update_transform_gesture(
                                gesture_id,
                                SceneCommand::SetTransform {
                                    scene_id,
                                    object_id,
                                    transform,
                                },
                                snapshot.revision,
                            ) {
                                state.status = error;
                            }
                        }
                    }
                }
            }
            if response.drag_stopped() {
                if let Some((gesture_id, _)) = state.gesture.take() {
                    if let Err(error) = session.finish_transform_gesture(gesture_id) {
                        state.status = error;
                    }
                }
            }
        }
    });
}

fn active_scene(document: &ProjectDocument) -> Option<&SceneDocument> {
    document
        .scenes
        .iter()
        .find(|scene| scene.id == document.active_scene_id)
}

fn next_object_name(objects: &[SceneObjectDocument]) -> String {
    let mut index = 1;
    loop {
        let name = format!("Object {index}");
        if !objects.iter().any(|object| object.name == name) {
            return name;
        }
        index += 1;
    }
}

fn hierarchy_depth(id: ObjectId, objects: &[SceneObjectDocument]) -> usize {
    let mut depth = 0;
    let mut cursor = objects
        .iter()
        .find(|object| object.id == id)
        .and_then(|object| object.parent_id);
    while let Some(parent) = cursor {
        depth += 1;
        cursor = objects
            .iter()
            .find(|object| object.id == parent)
            .and_then(|object| object.parent_id);
        if depth > objects.len() {
            break;
        }
    }
    depth
}

fn scene_world_transforms(scene: &SceneDocument) -> HashMap<ObjectId, Transform> {
    let mut result = HashMap::new();
    for object in &scene.objects {
        let _ = world_transform(object.id, scene, &mut result, 0);
    }
    result
}

fn world_transform(
    id: ObjectId,
    scene: &SceneDocument,
    cache: &mut HashMap<ObjectId, Transform>,
    depth: usize,
) -> Transform {
    if let Some(transform) = cache.get(&id) {
        return *transform;
    }
    let Some(object) = scene.objects.iter().find(|object| object.id == id) else {
        return Transform::IDENTITY;
    };
    let local = document_transform(object.local_transform);
    let world = if depth > scene.objects.len() {
        local
    } else if let Some(parent_id) = object.parent_id {
        let parent = world_transform(parent_id, scene, cache, depth + 1);
        parent.mul_transform(local)
    } else {
        local
    };
    cache.insert(id, world);
    world
}

fn document_transform(transform: TransformDocument) -> Transform {
    Transform {
        translation: Vec3::from_array(transform.translation),
        rotation: Quat::from_xyzw(
            transform.rotation_xyzw[0],
            transform.rotation_xyzw[1],
            transform.rotation_xyzw[2],
            transform.rotation_xyzw[3],
        ),
        scale: Vec3::from_array(transform.scale),
    }
}
