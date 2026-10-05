//! First editable ProjectDocument shell: hierarchy, inspector and a lightweight transform gizmo.

use std::{collections::HashMap, time::Instant};

use bevy::{
    asset::AssetPlugin,
    gltf::GltfAssetLabel,
    prelude::*,
    window::{Window, WindowPlugin},
    world_serialization::WorldAssetRoot,
};
use bevy_egui::{EguiContexts, EguiPlugin, EguiPrimaryContextPass, egui};
use quasar_project::{
    assets::{
        AssetCatalog, AssetId, AssetKind, AssetRecord, AssetStatus, ModelAssetComponent,
        ScriptComponent,
    },
    commands::SceneCommand,
    document::{
        ObjectId, ProjectDocument, SceneDocument, SceneId, SceneObjectDocument, TransformDocument,
    },
    gameplay::{
        AUDIO_SOURCE_COMPONENT_TYPE_ID, AudioCategory, AudioListenerComponent,
        AudioSourceComponent, CameraComponent, CharacterControllerComponent, ColliderComponent,
        ColliderShape, DoorComponent, RigidBodyComponent, RigidBodyKind, TypedComponent,
    },
};

use crate::{
    document_session::{EditorDocumentSession, TransformGestureId},
    import::jobs::{AssetJobService, AssetJobSnapshot, AssetJobStatus},
    mcp_session::EditorMcpSession,
    play::EditorPlayService,
    script_editor,
};
use crate::{
    document_viewport::{self, DocumentViewport},
    editor_style::{self as chrome, Icon},
};
use uuid::Uuid;

#[derive(Resource, Default)]
struct Selection(Option<ObjectId>);

#[derive(Resource, Default)]
struct UiState {
    name_edit: Option<(ObjectId, String)>,
    gesture: Option<(TransformGestureId, ObjectId)>,
    gizmo_mode: GizmoMode,
    status: String,
    workspace: WorkspaceTab,
    advanced: bool,
    console_open: bool,
    hierarchy_search: String,
    script_edit: Option<(AssetId, String, u64)>,
    script_dirty_since: Option<Instant>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum WorkspaceTab {
    #[default]
    Scene,
    Code,
    Assets,
}

#[derive(Resource, Default)]
struct AssetBrowserState {
    project_root: Option<std::path::PathBuf>,
    records: Vec<AssetRecord>,
    diagnostics: Vec<(std::path::PathBuf, String)>,
    search: String,
    import_path: String,
    reimport_path: String,
    message: String,
    selected: Option<AssetId>,
    last_dropped_path: Option<std::path::PathBuf>,
    url_input: String,
    author_input: String,
    license_input: String,
    seen_jobs: std::collections::HashSet<Uuid>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum GizmoMode {
    #[default]
    Select,
    Move,
    Rotate,
    Scale,
}

#[derive(Component)]
struct DocumentObjectVisual;

#[derive(Resource, Default)]
struct RenderedRevision(Option<u64>);

pub(crate) fn document_editor_app(
    session: EditorDocumentSession,
    mcp: EditorMcpSession,
    jobs: AssetJobService,
    play: EditorPlayService,
) -> App {
    let mut app = App::new();
    let asset_root = session
        .snapshot()
        .ok()
        .and_then(|snapshot| snapshot.path.parent().map(|path| path.to_path_buf()))
        .unwrap_or_else(|| std::path::PathBuf::from("."));
    app.add_plugins(
        DefaultPlugins
            .set(WindowPlugin {
                primary_window: Some(Window {
                    title: "Quasar Engine".into(),
                    name: Some("quasar.editor".into()),
                    resolution: (1440, 900).into(),
                    resizable: true,
                    ..default()
                }),
                ..default()
            })
            .set(AssetPlugin {
                file_path: asset_root.to_string_lossy().into_owned(),
                ..default()
            }),
    )
    .add_plugins(EguiPlugin::default())
    .insert_resource(session)
    .insert_resource(mcp)
    .insert_resource(jobs)
    .insert_resource(play)
    .init_resource::<Selection>()
    .init_resource::<UiState>()
    .init_resource::<AssetBrowserState>()
    .init_resource::<RenderedRevision>()
    .add_systems(Startup, document_viewport::setup)
    .add_systems(Update, (sync_document_scene, document_viewport::resize))
    .add_systems(EguiPrimaryContextPass, draw_document_editor);
    app
}

fn sync_document_scene(
    mut commands: Commands,
    session: Res<EditorDocumentSession>,
    mut rendered_revision: ResMut<RenderedRevision>,
    old_objects: Query<Entity, With<DocumentObjectVisual>>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    asset_server: Res<AssetServer>,
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
    let asset_catalog = AssetCatalog::scan(
        snapshot
            .path
            .parent()
            .unwrap_or_else(|| std::path::Path::new(".")),
    );
    for object in &scene.objects {
        let transform = world.get(&object.id).copied().unwrap_or_default();
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
            commands.spawn((
                DocumentObjectVisual,
                WorldAssetRoot(asset_server.load(GltfAssetLabel::Scene(0).from_asset(model_path))),
                transform,
            ));
            continue;
        }
        let color = Color::srgb(0.58, 0.56, 0.52);
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

#[allow(clippy::too_many_arguments)]
fn draw_document_editor(
    mut contexts: EguiContexts,
    session: Res<EditorDocumentSession>,
    mut selection: ResMut<Selection>,
    mut ui_state: ResMut<UiState>,
    mut asset_browser: ResMut<AssetBrowserState>,
    jobs: Res<AssetJobService>,
    play: Res<EditorPlayService>,
    mut viewport: ResMut<DocumentViewport>,
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
    chrome::install(ctx);
    let project_root = snapshot
        .path
        .parent()
        .unwrap_or_else(|| std::path::Path::new("."))
        .to_path_buf();
    if asset_browser.project_root.as_ref() != Some(&project_root) {
        refresh_asset_catalog(&mut asset_browser, &project_root);
        asset_browser.project_root = Some(project_root.clone());
    }
    let recent_jobs = jobs.recent(8).unwrap_or_default();
    let mut refresh_after_jobs = false;
    for job in &recent_jobs {
        if job.status == AssetJobStatus::Succeeded && asset_browser.seen_jobs.insert(job.job_id) {
            refresh_after_jobs = true;
            if let Some(asset_id) = job.asset_id {
                asset_browser.selected = Some(asset_id);
            }
        }
    }
    if refresh_after_jobs {
        refresh_asset_catalog(&mut asset_browser, &project_root);
    }
    let dropped_paths = ctx.input(|input| {
        input
            .raw
            .dropped_files
            .iter()
            .filter_map(|file| {
                let path = file.path();
                (!path.as_os_str().is_empty()).then(|| path.to_path_buf())
            })
            .collect::<Vec<_>>()
    });
    if dropped_paths.is_empty() {
        asset_browser.last_dropped_path = None;
    } else if let Some(path) = dropped_paths.first()
        && asset_browser.last_dropped_path.as_ref() != Some(path)
    {
        asset_browser.last_dropped_path = Some(path.clone());
        start_local_asset_job(&mut asset_browser, &jobs, &project_root, path);
    }
    let mut editor_ui = egui::Ui::new(
        ctx.clone(),
        "quasar_document_editor".into(),
        egui::UiBuilder::new()
            .layer_id(egui::LayerId::background())
            .max_rect(ctx.viewport_rect()),
    );

    egui::Panel::top("document_toolbar")
        .frame(chrome::toolbar_frame())
        .show(&mut editor_ui, |ui| {
            ui.horizontal(|ui| {
                if chrome::tab(ui, "Scene", ui_state.workspace == WorkspaceTab::Scene).clicked() {
                    ui_state.workspace = WorkspaceTab::Scene;
                }
                if chrome::tab(ui, "Code", ui_state.workspace == WorkspaceTab::Code).clicked() {
                    ui_state.workspace = WorkspaceTab::Code;
                }
                if chrome::tab(ui, "Assets", ui_state.workspace == WorkspaceTab::Assets).clicked() {
                    ui_state.workspace = WorkspaceTab::Assets;
                }
                chrome::icon_button(
                    ui,
                    Icon::Plus,
                    false,
                    false,
                    "Additional workspaces will be available in a later version.",
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.selectable_label(ui_state.advanced, "Advanced").clicked() {
                        ui_state.advanced = true;
                    }
                    if ui.selectable_label(!ui_state.advanced, "Basic").clicked() {
                        ui_state.advanced = false;
                    }
                    ui.separator();
                    ui.add_enabled(false, egui::Button::new("Advisor"))
                        .on_hover_text("Advisor rules are planned.");
                    let play_status = play.status(&session);
                    if ui
                        .button(if play_status.state == "Running" {
                            "Stop"
                        } else {
                            "Play"
                        })
                        .on_hover_text(if play_status.state == "Running" {
                            "Stop standalone Player"
                        } else {
                            "Launch current project in Player"
                        })
                        .clicked()
                    {
                        let result = if play_status.state == "Running" {
                            play.stop(&session)
                        } else {
                            play.start(&session)
                        };
                        match result {
                            Ok(status) => {
                                ui_state.status = format!("Play {}", status.state.to_lowercase())
                            }
                            Err(error) => ui_state.status = error,
                        }
                    }
                    ui.separator();
                    if chrome::icon_button(ui, Icon::Redo, false, snapshot.redo_depth > 0, "Redo")
                        .clicked()
                        && let Err(error) = session.redo(snapshot.revision)
                    {
                        ui_state.status = error;
                    }
                    if chrome::icon_button(ui, Icon::Undo, false, snapshot.undo_depth > 0, "Undo")
                        .clicked()
                        && let Err(error) = session.undo(snapshot.revision)
                    {
                        ui_state.status = error;
                    }
                    if chrome::icon_button(ui, Icon::Save, false, true, "Save project").clicked() {
                        match session.save(snapshot.revision) {
                            Ok(_) => ui_state.status = "Project saved".into(),
                            Err(error) => ui_state.status = error,
                        }
                    }
                });
            });
        });

    egui::Panel::bottom("document_console")
        .frame(chrome::panel_frame())
        .show(&mut editor_ui, |ui| {
            ui.horizontal(|ui| {
                if ui
                    .selectable_label(ui_state.console_open, "Console")
                    .clicked()
                {
                    ui_state.console_open = !ui_state.console_open;
                }
                ui.label(
                    egui::RichText::new(if ui_state.status.is_empty() {
                        "Ready"
                    } else {
                        &ui_state.status
                    })
                    .small()
                    .color(chrome::MUTED),
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(
                        egui::RichText::new("MCP · available")
                            .small()
                            .color(chrome::MUTED),
                    )
                    .on_hover_text("Local MCP bridge is available for this Editor session.");
                    ui.label(
                        egui::RichText::new(if snapshot.dirty {
                            "Unsaved changes"
                        } else {
                            "Saved"
                        })
                        .small()
                        .color(if snapshot.dirty {
                            chrome::ACCENT
                        } else {
                            chrome::MUTED
                        }),
                    );
                });
            });
            if ui_state.console_open {
                egui::ScrollArea::vertical()
                    .max_height(120.0)
                    .show(ui, |ui| {
                        ui.separator();
                        ui.label(format!(
                            "{} · revision {}",
                            snapshot.document.name, snapshot.revision
                        ));
                        for job in &recent_jobs {
                            ui.label(format!(
                                "{:?} · {} bytes · {}",
                                job.status, job.progress_bytes, job.job_id
                            ));
                        }
                        for (path, message) in &asset_browser.diagnostics {
                            ui.label(format!("{}: {message}", path.display()));
                        }
                    });
            }
        });

    egui::Panel::left("hierarchy_or_assets")
        .frame(chrome::panel_frame())
        .resizable(true)
        .default_size(if ui_state.workspace == WorkspaceTab::Assets {
            330.0
        } else {
            250.0
        })
        .min_size(190.0)
        .show(&mut editor_ui, |ui| {
            if ui_state.workspace == WorkspaceTab::Scene {
                ui.horizontal(|ui| {
                    ui.heading("Hierarchy");
                    if chrome::icon_button(ui, Icon::Plus, false, true, "Create object").clicked() {
                        let object =
                            SceneObjectDocument::new(next_object_name(&objects), selection.0);
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
                ui.add(
                    egui::TextEdit::singleline(&mut ui_state.hierarchy_search)
                        .hint_text("Search objects")
                        .desired_width(f32::INFINITY),
                );
                ui.add_space(4.0);
                ui.label(
                    egui::RichText::new(&scene.name)
                        .small()
                        .color(chrome::MUTED),
                );
                egui::ScrollArea::vertical().show(ui, |ui| {
                    for object in &objects {
                        if !object
                            .name
                            .to_lowercase()
                            .contains(&ui_state.hierarchy_search.to_lowercase())
                        {
                            continue;
                        }
                        let depth = hierarchy_depth(object.id, &objects);
                        if chrome::object_row(
                            ui,
                            &object.name,
                            depth,
                            selection.0 == Some(object.id),
                        )
                        .clicked()
                        {
                            selection.0 = Some(object.id);
                        }
                    }
                });
                ui.separator();
                if ui
                    .add_enabled(selection.0.is_some(), egui::Button::new("Delete selected"))
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
            } else if ui_state.workspace == WorkspaceTab::Assets {
                egui::ScrollArea::vertical()
                    .id_salt("asset-panel-body")
                    .show(ui, |ui| {
                        draw_asset_browser(
                            ui,
                            &mut asset_browser,
                            &jobs,
                            &recent_jobs,
                            &project_root,
                        );
                    });
            } else {
                ui.heading("Project scripts");
                ui.separator();
                let scripts = asset_browser
                    .records
                    .iter()
                    .filter(|record| record.metadata.kind == AssetKind::Script)
                    .map(|record| {
                        (
                            record.metadata.asset_id,
                            record.metadata.source_path.clone(),
                        )
                    })
                    .collect::<Vec<_>>();
                egui::ScrollArea::vertical().show(ui, |ui| {
                    for (_, source_path) in &scripts {
                        ui.label(source_path);
                    }
                });
            }
        });

    egui::Panel::right("inspector")
        .frame(chrome::panel_frame())
        .resizable(true)
        .default_size(300.0)
        .min_size(260.0)
        .show(&mut editor_ui, |ui| {
            ui.heading("Inspector");
            ui.separator();
            if let Some(object) = selection
                .0
                .and_then(|id| objects.iter().find(|object| object.id == id))
            {
                egui::ScrollArea::vertical().show(ui, |ui| {
                    draw_inspector(
                        ui,
                        &session,
                        &mut ui_state,
                        scene_id,
                        snapshot.revision,
                        object,
                        &objects,
                        &asset_browser.records,
                    );
                });
            } else {
                ui.add_space(24.0);
                ui.label("No object selected");
                ui.weak("Choose an object in the hierarchy to edit its properties.");
            }
        });

    egui::CentralPanel::default()
        .frame(chrome::panel_frame())
        .show(&mut editor_ui, |ui| {
            if ui_state.workspace == WorkspaceTab::Code {
                draw_code_workspace(ui, &session, &asset_browser.records, &mut ui_state);
                return;
            }
            ui.horizontal(|ui| {
                chrome::tab(
                    ui,
                    &format!(
                        "{}.scene{}",
                        scene.name,
                        if snapshot.dirty { " *" } else { "" }
                    ),
                    true,
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.weak(format!("{} objects", objects.len()));
                });
            });
            ui.separator();
            ui.horizontal(|ui| {
                ui.label("Perspective");
                ui.separator();
                ui.weak("Scene view");
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.weak("Y up");
                });
            });
            let available = ui.available_size().max(egui::vec2(96.0, 96.0));
            let response =
                ui.add(egui::Image::new((viewport.texture_id, available)).corner_radius(7));
            viewport.desired_points = Vec2::new(response.rect.width(), response.rect.height());
            let rail = egui::Rect::from_min_size(
                response.rect.min + egui::vec2(10.0, 10.0),
                egui::vec2(42.0, 164.0),
            );
            ui.scope_builder(
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
                            for (mode, icon, hint) in [
                                (GizmoMode::Select, Icon::Select, "Select"),
                                (GizmoMode::Move, Icon::Move, "Move"),
                                (GizmoMode::Rotate, Icon::Rotate, "Rotate"),
                                (GizmoMode::Scale, Icon::Scale, "Scale"),
                            ] {
                                if chrome::icon_button(
                                    ui,
                                    icon,
                                    ui_state.gizmo_mode == mode,
                                    true,
                                    hint,
                                )
                                .clicked()
                                {
                                    ui_state.gizmo_mode = mode;
                                }
                            }
                        });
                },
            );
            if selection.0.is_some() && ui_state.gizmo_mode != GizmoMode::Select {
                let controls = egui::Rect::from_min_size(
                    response.rect.left_bottom() + egui::vec2(62.0, -70.0),
                    egui::vec2(260.0, 60.0),
                );
                ui.scope_builder(egui::UiBuilder::new().max_rect(controls), |ui| {
                    egui::Frame::new()
                        .fill(chrome::PANEL)
                        .corner_radius(8)
                        .inner_margin(8)
                        .show(ui, |ui| {
                            draw_gizmo(ui, &session, &mut selection, &mut ui_state, scene_id);
                        });
                });
            }
        });
    Ok(())
}

fn draw_code_workspace(
    ui: &mut egui::Ui,
    session: &EditorDocumentSession,
    assets: &[AssetRecord],
    state: &mut UiState,
) {
    let scripts = assets
        .iter()
        .filter(|record| record.metadata.kind == AssetKind::Script)
        .collect::<Vec<_>>();
    ui.horizontal(|ui| {
        ui.heading("Code");
        ui.weak("Project Lua scripts");
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let Some((asset_id, source, revision)) = state.script_edit.as_ref() else {
                return;
            };
            let dirty = script_editor::read_script(session, *asset_id)
                .is_ok_and(|current| current.revision != *revision || current.source != *source);
            if ui
                .add_enabled(dirty, egui::Button::new("Save script"))
                .clicked()
            {
                let (id, text, expected) = (*asset_id, source.clone(), *revision);
                match script_editor::write_script(session, id, expected, &text) {
                    Ok(saved) => {
                        state.script_edit = Some((saved.asset_id, saved.source, saved.revision));
                        state.script_dirty_since = None;
                        state.status = "Lua script saved".into();
                    }
                    Err(error) => state.status = error,
                }
            }
        });
    });
    ui.separator();
    if scripts.is_empty() {
        ui.label("No project Lua scripts yet. Import a .lua file in Assets.");
        return;
    }
    if state
        .script_edit
        .as_ref()
        .is_none_or(|(id, _, _)| !scripts.iter().any(|r| r.metadata.asset_id == *id))
        && let Some(script) = scripts.first()
        && let Ok(script) = script_editor::read_script(session, script.metadata.asset_id)
    {
        state.script_edit = Some((script.asset_id, script.source, script.revision));
    }
    if let Some((asset_id, source, revision)) = &mut state.script_edit {
        let current_name = scripts
            .iter()
            .find(|r| r.metadata.asset_id == *asset_id)
            .map(|r| r.metadata.source_path.as_str())
            .unwrap_or("Script");
        let mut requested_script = None;
        egui::ComboBox::from_id_salt("project-script-selector")
            .selected_text(current_name)
            .show_ui(ui, |ui| {
                for record in &scripts {
                    if ui
                        .selectable_label(
                            record.metadata.asset_id == *asset_id,
                            &record.metadata.source_path,
                        )
                        .clicked()
                    {
                        requested_script = Some(record.metadata.asset_id);
                    }
                }
            });
        if let Some(next_id) = requested_script
            && next_id != *asset_id
        {
            let can_switch = match script_editor::read_script(session, *asset_id) {
                Ok(current) if current.revision != *revision || current.source != *source => {
                    match script_editor::write_script(session, *asset_id, *revision, source) {
                        Ok(_) => true,
                        Err(error) => {
                            state.status = error;
                            false
                        }
                    }
                }
                Ok(_) => true,
                Err(error) => {
                    state.status = error;
                    false
                }
            };
            if can_switch {
                match script_editor::read_script(session, next_id) {
                    Ok(script) => {
                        *asset_id = script.asset_id;
                        *source = script.source;
                        *revision = script.revision;
                        state.script_dirty_since = None;
                    }
                    Err(error) => state.status = error,
                }
            }
        }
        ui.add_space(6.0);
        let response = ui.add(
            egui::TextEdit::multiline(source)
                .code_editor()
                .desired_width(f32::INFINITY)
                .desired_rows(24),
        );
        if response.changed() {
            state.script_dirty_since = Some(Instant::now());
        }
        if state
            .script_dirty_since
            .is_some_and(|since| since.elapsed() >= std::time::Duration::from_millis(700))
        {
            let (id, text, expected) = (*asset_id, source.clone(), *revision);
            match script_editor::write_script(session, id, expected, &text) {
                Ok(saved) => {
                    *asset_id = saved.asset_id;
                    *source = saved.source;
                    *revision = saved.revision;
                    state.script_dirty_since = None;
                    state.status = "Lua script auto-saved".into();
                }
                Err(error) => {
                    state.status = error;
                    state.script_dirty_since = None;
                }
            }
        }
        ui.small("Lua syntax is validated on save. Stale revisions are rejected.");
    }
}

fn draw_asset_browser(
    ui: &mut egui::Ui,
    browser: &mut AssetBrowserState,
    jobs: &AssetJobService,
    recent_jobs: &[AssetJobSnapshot],
    project_root: &std::path::Path,
) {
    ui.horizontal(|ui| {
        ui.heading("Asset Library");
        if ui.button("↻").on_hover_text("Refresh asset list").clicked() {
            refresh_asset_catalog(browser, project_root);
        }
    });
    ui.add(
        egui::TextEdit::singleline(&mut browser.import_path)
            .hint_text("File path: GLB, PNG, JPEG or WAV"),
    );
    let import_clicked = ui.button("Import file").clicked();
    if import_clicked {
        let path = std::path::PathBuf::from(browser.import_path.trim());
        if path.as_os_str().is_empty() {
            browser.message = "Enter a source file path or drop a file onto the editor.".into();
        } else {
            start_local_asset_job(browser, jobs, project_root, &path);
        }
    }
    ui.collapsing("Import from URL", |ui| {
        ui.add(
            egui::TextEdit::singleline(&mut browser.url_input).hint_text("https://.../asset.glb"),
        );
        ui.add(
            egui::TextEdit::singleline(&mut browser.author_input).hint_text("Author (optional)"),
        );
        ui.add(
            egui::TextEdit::singleline(&mut browser.license_input).hint_text("License (optional)"),
        );
        if ui.button("Download and import").clicked() {
            let url = browser.url_input.trim().to_owned();
            let author = nonempty_option(&browser.author_input);
            let license = nonempty_option(&browser.license_input);
            match jobs.start_url_import(project_root.to_path_buf(), url, author, license) {
                Ok(job_id) => browser.message = format!("URL import job queued: {job_id}"),
                Err(error) => browser.message = format!("Could not start URL import: {error}"),
            }
        }
    });
    ui.separator();
    ui.add(egui::TextEdit::singleline(&mut browser.search).hint_text("Search assets"));
    let filtered = browser
        .records
        .iter()
        .filter(|record| {
            record
                .metadata
                .source_path
                .to_ascii_lowercase()
                .contains(&browser.search.to_ascii_lowercase())
        })
        .cloned()
        .collect::<Vec<_>>();
    ui.label(format!("{} assets", filtered.len()));
    egui::ScrollArea::vertical()
        .max_height(220.0)
        .show(ui, |ui| {
            for record in &filtered {
                let name = std::path::Path::new(&record.metadata.source_path)
                    .file_name()
                    .and_then(|name| name.to_str())
                    .unwrap_or(&record.metadata.source_path);
                let label = format!(
                    "{}  {}  ·  {}",
                    asset_status_label(record.status),
                    name,
                    asset_kind_label(record.metadata.kind)
                );
                if ui
                    .selectable_label(browser.selected == Some(record.metadata.asset_id), label)
                    .clicked()
                {
                    browser.selected = Some(record.metadata.asset_id);
                }
            }
        });
    if let Some(selected) = browser
        .selected
        .and_then(|id| {
            browser
                .records
                .iter()
                .find(|record| record.metadata.asset_id == id)
        })
        .cloned()
    {
        ui.separator();
        ui.label(format!("Asset ID  {}", selected.metadata.asset_id.0));
        ui.label(format!("Source  {}", selected.metadata.source_path));
        ui.label(format!(
            "Importer  {} v{}",
            selected.metadata.importer_id, selected.metadata.importer_version
        ));
        ui.label(format!(
            "License  {}",
            selected.metadata.license.as_deref().unwrap_or("Unknown")
        ));
        ui.add(
            egui::TextEdit::singleline(&mut browser.reimport_path)
                .hint_text("Optional replacement source path"),
        );
        if ui
            .add_enabled(
                selected.status == AssetStatus::Ready,
                egui::Button::new("Reimport from project source"),
            )
            .clicked()
        {
            let source_path = nonempty_option(&browser.reimport_path).map(std::path::PathBuf::from);
            match jobs.start_reimport(
                project_root.to_path_buf(),
                selected.metadata.asset_id,
                source_path,
            ) {
                Ok(job_id) => browser.message = format!("Reimport job queued: {job_id}"),
                Err(error) => browser.message = format!("Could not start reimport: {error}"),
            }
        }
    }
    if !recent_jobs.is_empty() {
        ui.separator();
        ui.label("Asset jobs");
        for job in recent_jobs {
            ui.horizontal(|ui| {
                ui.label(format!(
                    "{} · {} · {} B",
                    job.operation,
                    asset_job_status_label(job.status),
                    job.progress_bytes
                ));
                if matches!(job.status, AssetJobStatus::Queued | AssetJobStatus::Running)
                    && ui.small_button("Cancel").clicked()
                    && let Err(error) = jobs.cancel(job.job_id)
                {
                    browser.message = error;
                }
            });
            if let Some(message) = &job.message {
                ui.small(format!("{}: {message}", job.job_id));
            }
        }
    }
    if !browser.message.is_empty() {
        ui.separator();
        ui.label(&browser.message);
    }
    for (path, message) in browser.diagnostics.iter().take(5) {
        ui.colored_label(
            egui::Color32::LIGHT_RED,
            format!("{}: {message}", path.display()),
        );
    }
}

fn refresh_asset_catalog(browser: &mut AssetBrowserState, project_root: &std::path::Path) {
    let catalog = AssetCatalog::scan(project_root);
    browser.records = catalog.assets;
    browser.diagnostics = catalog
        .diagnostics
        .into_iter()
        .map(|diagnostic| (diagnostic.path, diagnostic.message))
        .collect();
    if browser.selected.is_some_and(|id| {
        !browser
            .records
            .iter()
            .any(|record| record.metadata.asset_id == id)
    }) {
        browser.selected = None;
    }
}

fn start_local_asset_job(
    browser: &mut AssetBrowserState,
    jobs: &AssetJobService,
    project_root: &std::path::Path,
    source_path: &std::path::Path,
) {
    match jobs.start_local_import(project_root.to_path_buf(), source_path.to_path_buf()) {
        Ok(job_id) => browser.message = format!("Import job queued: {job_id}"),
        Err(error) => browser.message = format!("Could not start import: {error}"),
    }
}

fn nonempty_option(value: &str) -> Option<String> {
    let value = value.trim();
    (!value.is_empty()).then(|| value.to_owned())
}

fn asset_job_status_label(status: AssetJobStatus) -> &'static str {
    match status {
        AssetJobStatus::Queued => "Queued",
        AssetJobStatus::Running => "Running",
        AssetJobStatus::Succeeded => "Succeeded",
        AssetJobStatus::Failed => "Failed",
        AssetJobStatus::Cancelled => "Cancelled",
    }
}

fn asset_status_label(status: AssetStatus) -> &'static str {
    match status {
        AssetStatus::Ready => "● Ready",
        AssetStatus::Missing => "! Missing",
        AssetStatus::Conflict => "! ID conflict",
    }
}

fn asset_kind_label(kind: AssetKind) -> &'static str {
    match kind {
        AssetKind::Model => "Model",
        AssetKind::Texture => "Texture",
        AssetKind::Audio => "Audio",
        AssetKind::Script => "Script",
    }
}

#[cfg(test)]
mod asset_ui_tests {
    use super::*;
    use std::{thread, time::Instant};

    struct TestProject(std::path::PathBuf);

    impl TestProject {
        fn new() -> Self {
            let root =
                std::env::temp_dir().join(format!("quasar-editor-assets-{}", Uuid::new_v4()));
            std::fs::create_dir_all(&root).expect("temporary project directory can be created");
            Self(root)
        }
    }

    impl Drop for TestProject {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn asset_panel_import_action_publishes_and_refreshes_catalog() {
        let project = TestProject::new();
        let source = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/fixtures/stage0-player/assets/Quasar/viewport-prop.glb");
        let jobs = AssetJobService::default();
        let mut browser = AssetBrowserState::default();
        start_local_asset_job(&mut browser, &jobs, &project.0, &source);
        assert!(browser.message.starts_with("Import job queued:"));
        let job_id = browser
            .message
            .split_whitespace()
            .last()
            .unwrap()
            .parse::<Uuid>()
            .unwrap();

        let deadline = Instant::now() + std::time::Duration::from_secs(5);
        loop {
            let job = jobs.get(job_id).expect("UI import job is visible");
            match job.status {
                AssetJobStatus::Succeeded => break,
                AssetJobStatus::Failed | AssetJobStatus::Cancelled => {
                    panic!("UI import job did not succeed: {job:?}")
                }
                AssetJobStatus::Queued | AssetJobStatus::Running => {}
            }
            assert!(Instant::now() < deadline, "UI import job timed out");
            thread::sleep(std::time::Duration::from_millis(10));
        }

        refresh_asset_catalog(&mut browser, &project.0);
        assert_eq!(browser.records.len(), 1);
        assert_eq!(browser.records[0].status, AssetStatus::Ready);
        assert_eq!(browser.records[0].metadata.kind, AssetKind::Model);
    }
}

#[allow(clippy::collapsible_if, clippy::too_many_arguments)]
fn draw_inspector(
    ui: &mut egui::Ui,
    session: &EditorDocumentSession,
    state: &mut UiState,
    scene_id: SceneId,
    revision: u64,
    object: &SceneObjectDocument,
    objects: &[SceneObjectDocument],
    assets: &[AssetRecord],
) {
    let object_id = object.id;
    let current_name = state
        .name_edit
        .as_ref()
        .filter(|(id, _)| *id == object_id)
        .map(|(_, name)| name.clone())
        .unwrap_or_else(|| object.name.clone());
    let mut name = current_name;
    let response = ui.add(
        egui::TextEdit::singleline(&mut name)
            .hint_text("Object name")
            .desired_width(f32::INFINITY),
    );
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
    if state.advanced {
        ui.label(
            egui::RichText::new(format!("ID  {}", object_id.0))
                .small()
                .color(chrome::MUTED),
        );
    }
    ui.separator();
    ui.strong("Model");
    let assigned_model = ModelAssetComponent::from_components(&object.components)
        .ok()
        .flatten()
        .map(|reference| reference.asset_id);
    let mut next_model = assigned_model;
    egui::ComboBox::from_id_salt(("model-asset", object_id))
        .selected_text(
            assigned_model
                .and_then(|asset_id| {
                    assets.iter().find(|asset| {
                        asset.metadata.asset_id == asset_id
                            && asset.metadata.kind == AssetKind::Model
                    })
                })
                .and_then(|asset| {
                    std::path::Path::new(&asset.metadata.source_path)
                        .file_name()
                        .and_then(|name| name.to_str())
                })
                .unwrap_or("None"),
        )
        .show_ui(ui, |ui| {
            ui.selectable_value(&mut next_model, None, "None");
            for asset in assets.iter().filter(|asset| {
                asset.metadata.kind == AssetKind::Model && asset.status == AssetStatus::Ready
            }) {
                ui.selectable_value(
                    &mut next_model,
                    Some(asset.metadata.asset_id),
                    &asset.metadata.source_path,
                );
            }
        });
    if next_model != assigned_model {
        match session.apply_command(
            SceneCommand::AssignModelAsset {
                scene_id,
                object_id,
                asset_id: next_model,
            },
            revision,
        ) {
            Ok(_) => state.status = "Updated model asset".into(),
            Err(error) => state.status = error,
        }
        return;
    }
    let assigned_script = ScriptComponent::from_components(&object.components)
        .ok()
        .flatten()
        .filter(|script| script.enabled)
        .map(|script| script.asset_id);
    let mut next_script = assigned_script;
    ui.strong("Gameplay Script");
    egui::ComboBox::from_id_salt(("script-asset", object_id))
        .selected_text(
            assigned_script
                .and_then(|asset_id| {
                    assets.iter().find(|asset| {
                        asset.metadata.asset_id == asset_id
                            && asset.metadata.kind == AssetKind::Script
                    })
                })
                .and_then(|asset| std::path::Path::new(&asset.metadata.source_path).file_name())
                .and_then(|name| name.to_str())
                .unwrap_or("None"),
        )
        .show_ui(ui, |ui| {
            ui.selectable_value(&mut next_script, None, "None");
            for asset in assets.iter().filter(|asset| {
                asset.metadata.kind == AssetKind::Script && asset.status == AssetStatus::Ready
            }) {
                ui.selectable_value(
                    &mut next_script,
                    Some(asset.metadata.asset_id),
                    &asset.metadata.source_path,
                );
            }
        });
    if next_script != assigned_script {
        match session.apply_command(
            SceneCommand::AssignScriptAsset {
                scene_id,
                object_id,
                asset_id: next_script,
            },
            revision,
        ) {
            Ok(_) => state.status = "Updated gameplay script".into(),
            Err(error) => state.status = error,
        }
        return;
    }

    ui.separator();
    ui.strong("Gameplay");
    let camera = CameraComponent::from_components(&object.components)
        .ok()
        .flatten();
    if let Some(mut camera) = camera {
        let mut changed = ui.checkbox(&mut camera.enabled, "Camera enabled").changed();
        if ui.small_button("Remove camera").clicked() {
            remove_component(
                session,
                scene_id,
                revision,
                object_id,
                CameraComponent::TYPE_ID,
                state,
            );
            return;
        }
        ui.horizontal(|ui| {
            ui.label("Vertical FOV");
            changed |= ui
                .add(egui::DragValue::new(&mut camera.vertical_fov_degrees).range(1.0..=179.0))
                .changed();
        });
        if changed {
            if let Err(error) = set_typed_component(session, scene_id, revision, object_id, camera)
            {
                state.status = error;
            }
            return;
        }
    } else if ui.button("+ Add Camera").clicked() {
        if let Err(error) = set_typed_component(
            session,
            scene_id,
            revision,
            object_id,
            CameraComponent::default(),
        ) {
            state.status = error;
        }
        return;
    }

    if let Some(mut listener) = AudioListenerComponent::from_components(&object.components)
        .ok()
        .flatten()
    {
        let changed = ui
            .checkbox(&mut listener.enabled, "Audio listener enabled")
            .changed();
        if ui.small_button("Remove audio listener").clicked() {
            remove_component(
                session,
                scene_id,
                revision,
                object_id,
                AudioListenerComponent::TYPE_ID,
                state,
            );
            return;
        }
        if changed {
            if let Err(error) =
                set_typed_component(session, scene_id, revision, object_id, listener)
            {
                state.status = error;
            }
            return;
        }
    } else if ui.button("+ Add Audio Listener").clicked() {
        if let Err(error) = set_typed_component(
            session,
            scene_id,
            revision,
            object_id,
            AudioListenerComponent { enabled: true },
        ) {
            state.status = error;
        }
        return;
    }

    let collider = ColliderComponent::from_components(&object.components)
        .ok()
        .flatten();
    if let Some(mut collider) = collider {
        let mut changed = ui
            .checkbox(&mut collider.enabled, "Collider enabled")
            .changed();
        if ui.small_button("Remove collider").clicked() {
            remove_component(
                session,
                scene_id,
                revision,
                object_id,
                ColliderComponent::TYPE_ID,
                state,
            );
            return;
        }
        match &mut collider.shape {
            ColliderShape::Box { size } => {
                ui.label("Box size");
                changed |= vector3_controls(ui, size).changed;
                ui.label("Center");
                changed |= vector3_controls(ui, &mut collider.center).changed;
            }
            ColliderShape::Capsule { radius, height } => {
                ui.horizontal(|ui| {
                    ui.label("Radius");
                    changed |= ui
                        .add(egui::DragValue::new(radius).range(0.01..=1000.0))
                        .changed();
                    ui.label("Height");
                    changed |= ui
                        .add(egui::DragValue::new(height).range(0.02..=2000.0))
                        .changed();
                });
                ui.label("Center");
                changed |= vector3_controls(ui, &mut collider.center).changed;
            }
        }
        if changed {
            if let Err(error) =
                set_typed_component(session, scene_id, revision, object_id, collider)
            {
                state.status = error;
            }
            return;
        }
    } else if ui.button("+ Add Box Collider").clicked() {
        if let Err(error) = set_typed_component(
            session,
            scene_id,
            revision,
            object_id,
            ColliderComponent {
                enabled: true,
                center: [0.0; 3],
                shape: ColliderShape::Box { size: [1.0; 3] },
            },
        ) {
            state.status = error;
        }
        return;
    }

    let body = RigidBodyComponent::from_components(&object.components)
        .ok()
        .flatten();
    if let Some(mut body) = body {
        let mut changed = ui
            .checkbox(&mut body.enabled, "Rigid body enabled")
            .changed();
        if ui.small_button("Remove rigid body").clicked() {
            remove_component(
                session,
                scene_id,
                revision,
                object_id,
                RigidBodyComponent::TYPE_ID,
                state,
            );
            return;
        }
        egui::ComboBox::from_id_salt(("rigid-body-kind", object_id))
            .selected_text(format!("{:?}", body.kind))
            .show_ui(ui, |ui| {
                changed |= ui
                    .selectable_value(&mut body.kind, RigidBodyKind::Static, "Static")
                    .changed();
                changed |= ui
                    .selectable_value(&mut body.kind, RigidBodyKind::Kinematic, "Kinematic")
                    .changed();
                changed |= ui
                    .selectable_value(&mut body.kind, RigidBodyKind::Dynamic, "Dynamic")
                    .changed();
            });
        ui.horizontal(|ui| {
            ui.label("Mass");
            changed |= ui
                .add(egui::DragValue::new(&mut body.mass).range(0.001..=1_000_000.0))
                .changed();
            ui.label("Damping");
            changed |= ui
                .add(egui::DragValue::new(&mut body.linear_damping).range(0.0..=1000.0))
                .changed();
        });
        if changed {
            if let Err(error) = set_typed_component(session, scene_id, revision, object_id, body) {
                state.status = error;
            }
            return;
        }
    } else if collider.is_some_and(|collider| collider.enabled)
        && ui.button("+ Add Static Body").clicked()
    {
        if let Err(error) = set_typed_component(
            session,
            scene_id,
            revision,
            object_id,
            RigidBodyComponent::default(),
        ) {
            state.status = error;
        }
        return;
    }

    let door = DoorComponent::from_components(&object.components)
        .ok()
        .flatten();
    if let Some(mut door) = door {
        let mut changed = ui.checkbox(&mut door.enabled, "Door enabled").changed();
        if ui.small_button("Remove door").clicked() {
            remove_component(
                session,
                scene_id,
                revision,
                object_id,
                DoorComponent::TYPE_ID,
                state,
            );
            return;
        }
        ui.horizontal(|ui| {
            ui.label("Open angle");
            changed |= ui
                .add(egui::DragValue::new(&mut door.open_angle_degrees).range(1.0..=180.0))
                .changed();
            ui.label("Duration (s)");
            changed |= ui
                .add(egui::DragValue::new(&mut door.open_duration_seconds).range(0.05..=10.0))
                .changed();
        });
        if changed {
            if let Err(error) = set_typed_component(session, scene_id, revision, object_id, door) {
                state.status = error;
            }
            return;
        }
    } else if ui.button("+ Add Door").clicked() {
        if let Err(error) = set_typed_component(
            session,
            scene_id,
            revision,
            object_id,
            DoorComponent::default(),
        ) {
            state.status = error;
        }
        return;
    }

    let controller = CharacterControllerComponent::from_components(&object.components)
        .ok()
        .flatten();
    if let Some(mut controller) = controller {
        let mut changed = ui
            .checkbox(&mut controller.enabled, "Character controller enabled")
            .changed();
        if ui.small_button("Remove character controller").clicked() {
            remove_component(
                session,
                scene_id,
                revision,
                object_id,
                CharacterControllerComponent::TYPE_ID,
                state,
            );
            return;
        }
        for (label, value, range) in [
            ("Walk speed", &mut controller.walk_speed, 0.0..=1000.0),
            ("Jump speed", &mut controller.jump_speed, 0.0..=1000.0),
            ("Gravity", &mut controller.gravity, 0.0..=1000.0),
            ("Capsule radius", &mut controller.radius, 0.01..=1000.0),
            ("Capsule height", &mut controller.height, 0.02..=2000.0),
            ("Eye height", &mut controller.eye_height, 0.0..=2000.0),
            (
                "Max slope °",
                &mut controller.max_slope_degrees,
                0.0..=89.99,
            ),
            ("Step height", &mut controller.step_height, 0.0..=1000.0),
            ("Ground snap", &mut controller.ground_snap, 0.0..=1000.0),
            (
                "Mouse sensitivity",
                &mut controller.mouse_sensitivity,
                0.0001..=1.0,
            ),
        ] {
            ui.horizontal(|ui| {
                ui.label(label);
                changed |= ui
                    .add(egui::DragValue::new(value).speed(0.05).range(range))
                    .changed();
            });
        }
        let camera_options = objects
            .iter()
            .filter_map(|candidate| {
                (candidate.parent_id == Some(object_id)
                    && CameraComponent::from_components(&candidate.components)
                        .ok()
                        .flatten()
                        .is_some_and(|camera| camera.enabled))
                .then_some((candidate.id, candidate.name.as_str()))
            })
            .collect::<Vec<_>>();
        egui::ComboBox::from_id_salt(("controller-camera", object_id))
            .selected_text(
                camera_options
                    .iter()
                    .find(|(id, _)| *id == controller.camera_object)
                    .map(|(_, name)| *name)
                    .unwrap_or("Missing camera"),
            )
            .show_ui(ui, |ui| {
                for (camera_id, camera_name) in &camera_options {
                    changed |= ui
                        .selectable_value(&mut controller.camera_object, *camera_id, *camera_name)
                        .changed();
                }
            });
        if changed {
            if let Err(error) =
                set_typed_component(session, scene_id, revision, object_id, controller)
            {
                state.status = error;
            }
            return;
        }
    } else if object.parent_id.is_none() {
        let camera_options = objects
            .iter()
            .filter_map(|candidate| {
                (candidate.parent_id == Some(object_id)
                    && CameraComponent::from_components(&candidate.components)
                        .ok()
                        .flatten()
                        .is_some_and(|camera| camera.enabled))
                .then_some((candidate.id, candidate.name.as_str()))
            })
            .collect::<Vec<_>>();
        if !camera_options.is_empty() {
            let mut selected_camera = camera_options.first().map(|(id, _)| *id);
            egui::ComboBox::from_id_salt(("new-controller-camera", object_id))
                .selected_text(
                    camera_options
                        .first()
                        .map(|(_, name)| *name)
                        .unwrap_or("Select camera"),
                )
                .show_ui(ui, |ui| {
                    for (camera_id, camera_name) in &camera_options {
                        ui.selectable_value(&mut selected_camera, Some(*camera_id), *camera_name);
                    }
                });
            if ui.button("+ Add Character Controller").clicked()
                && let Some(camera_id) = selected_camera
            {
                if let Err(error) = set_typed_component(
                    session,
                    scene_id,
                    revision,
                    object_id,
                    CharacterControllerComponent::new(camera_id),
                ) {
                    state.status = error;
                }
                return;
            }
        }
    }

    let audio_source = AudioSourceComponent::from_components(&object.components)
        .ok()
        .flatten();
    let assigned_audio = audio_source.map(|source| source.asset_id);
    let mut next_audio = assigned_audio;
    egui::ComboBox::from_id_salt(("audio-asset", object_id))
        .selected_text(
            assigned_audio
                .and_then(|id| {
                    assets.iter().find(|asset| {
                        asset.metadata.asset_id == id && asset.metadata.kind == AssetKind::Audio
                    })
                })
                .and_then(|asset| std::path::Path::new(&asset.metadata.source_path).file_name())
                .and_then(|name| name.to_str())
                .unwrap_or("No Audio Source"),
        )
        .show_ui(ui, |ui| {
            ui.selectable_value(&mut next_audio, None, "None");
            for asset in assets.iter().filter(|asset| {
                asset.metadata.kind == AssetKind::Audio && asset.status == AssetStatus::Ready
            }) {
                ui.selectable_value(
                    &mut next_audio,
                    Some(asset.metadata.asset_id),
                    &asset.metadata.source_path,
                );
            }
        });
    if next_audio != assigned_audio {
        let result = if let Some(asset_id) = next_audio {
            set_typed_component(
                session,
                scene_id,
                revision,
                object_id,
                AudioSourceComponent {
                    enabled: true,
                    asset_id,
                    volume: 1.0,
                    looping: false,
                    autoplay: false,
                    spatial: true,
                    category: AudioCategory::Sfx,
                },
            )
        } else {
            session
                .apply_command(
                    SceneCommand::RemoveComponent {
                        scene_id,
                        object_id,
                        type_id: AUDIO_SOURCE_COMPONENT_TYPE_ID.into(),
                    },
                    revision,
                )
                .map(|_| ())
        };
        if let Err(error) = result {
            state.status = error;
        }
        return;
    }
    if let Some(mut source) = audio_source {
        let mut changed = ui
            .checkbox(&mut source.enabled, "Audio source enabled")
            .changed();
        ui.horizontal(|ui| {
            ui.label("Volume");
            changed |= ui
                .add(egui::Slider::new(&mut source.volume, 0.0..=1.0))
                .changed();
        });
        changed |= ui.checkbox(&mut source.looping, "Loop").changed();
        changed |= ui.checkbox(&mut source.autoplay, "Autoplay").changed();
        changed |= ui.checkbox(&mut source.spatial, "Spatial").changed();
        egui::ComboBox::from_id_salt(("audio-category", object_id))
            .selected_text(format!("{:?}", source.category))
            .show_ui(ui, |ui| {
                changed |= ui
                    .selectable_value(&mut source.category, AudioCategory::Music, "Music")
                    .changed();
                changed |= ui
                    .selectable_value(&mut source.category, AudioCategory::Sfx, "SFX")
                    .changed();
            });
        if changed {
            if let Err(error) = set_typed_component(session, scene_id, revision, object_id, source)
            {
                state.status = error;
            }
            return;
        }
    }
    let mut transform = object.local_transform;
    let mut interaction = TransformEditInteraction::default();
    ui.add_space(12.0);
    ui.separator();
    ui.strong("Transform");
    ui.label("Position");
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

fn set_typed_component<T: TypedComponent>(
    session: &EditorDocumentSession,
    scene_id: SceneId,
    revision: u64,
    object_id: ObjectId,
    component: T,
) -> Result<(), String> {
    session
        .apply_command(
            SceneCommand::SetComponent {
                scene_id,
                object_id,
                component: component.into_document(),
            },
            revision,
        )
        .map(|_| ())
}

fn remove_component(
    session: &EditorDocumentSession,
    scene_id: SceneId,
    revision: u64,
    object_id: ObjectId,
    type_id: &str,
    state: &mut UiState,
) {
    match session.apply_command(
        SceneCommand::RemoveComponent {
            scene_id,
            object_id,
            type_id: type_id.to_owned(),
        },
        revision,
    ) {
        Ok(_) => state.status = format!("Removed {type_id}"),
        Err(error) => state.status = error,
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
            ui.label(egui::RichText::new(["X", "Y", "Z"][index]).color(
                [
                    egui::Color32::from_rgb(215, 116, 107),
                    egui::Color32::from_rgb(137, 183, 125),
                    egui::Color32::from_rgb(121, 161, 209),
                ][index],
            ));
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
    ui.label(
        egui::RichText::new(format!("{:?} · drag an axis", state.gizmo_mode))
            .small()
            .color(chrome::MUTED),
    );
    ui.horizontal(|ui| {
        for (axis, color) in [
            (0, egui::Color32::from_rgb(220, 80, 80)),
            (1, egui::Color32::from_rgb(90, 200, 120)),
            (2, egui::Color32::from_rgb(90, 150, 240)),
        ] {
            let response = ui.add(
                egui::Button::new(egui::RichText::new(["X", "Y", "Z"][axis]).color(color))
                    .fill(chrome::FIELD)
                    .min_size(egui::vec2(50.0, 28.0))
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
                                GizmoMode::Select => {}
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
