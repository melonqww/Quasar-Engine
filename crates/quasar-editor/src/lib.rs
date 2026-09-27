//! Editor shell and UI integrations.

mod audio_probe;
mod gameplay_probe;
mod mcp_session;
mod project_probe;
mod viewport_probe;

use bevy::prelude::App;
use bevy_egui::EguiPlugin;

/// Adds egui to the shared Bevy/Rapier compatibility probe.
pub fn compatibility_probe_app() -> App {
    let project = project_probe::open_stage0_project()
        .unwrap_or_else(|error| panic!("cannot open Stage 0 animation project: {error}"));
    let benchmark = std::env::args().any(|argument| argument == "--benchmark-60s");
    let window_resolution = if benchmark { (1920, 1080) } else { (1440, 900) };
    let mut app = quasar_runtime::compatibility_probe_app_with_asset_root_and_resolution(
        project.asset_root.to_string_lossy().into_owned(),
        window_resolution,
    );
    app.insert_resource(quasar_runtime::animation::AnimationProbeData {
        animation: project.snapshot.scene.animation.clone(),
    })
    .insert_resource(project)
    .add_plugins(quasar_runtime::animation::AnimationProbePlugin);
    if let Some(path) =
        mcp_session::project_document_argument(&std::env::args().collect::<Vec<_>>())
            .unwrap_or_else(|error| panic!("invalid Editor project document argument: {error}"))
    {
        let open_session = mcp_session::open_project_session(&path)
            .unwrap_or_else(|error| panic!("cannot open Editor project document: {error}"));
        let mcp_server = mcp_session::start_mcp_session(open_session)
            .unwrap_or_else(|error| panic!("cannot start read-only MCP session: {error}"));
        app.insert_resource(mcp_server);
    }
    app.add_plugins(EguiPlugin::default());
    app.add_plugins(viewport_probe::ViewportProbePlugin);
    app.add_plugins(audio_probe::AudioProbePlugin);
    app.add_plugins(gameplay_probe::GameplayProbePlugin);
    app
}
