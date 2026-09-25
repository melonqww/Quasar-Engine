//! Editor shell and UI integrations.

mod audio_probe;
mod gameplay_probe;
mod viewport_probe;

use bevy::prelude::App;
use bevy_egui::EguiPlugin;
use std::path::Path;

/// Adds egui to the shared Bevy/Rapier compatibility probe.
pub fn compatibility_probe_app() -> App {
    let asset_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("assets");
    let benchmark = std::env::args().any(|argument| argument == "--benchmark-60s");
    let window_resolution = if benchmark { (1920, 1080) } else { (1440, 900) };
    let mut app = quasar_runtime::compatibility_probe_app_with_asset_root_and_resolution(
        asset_root.to_string_lossy().into_owned(),
        window_resolution,
    );
    app.add_plugins(EguiPlugin::default());
    app.add_plugins(viewport_probe::ViewportProbePlugin);
    app.add_plugins(audio_probe::AudioProbePlugin);
    app.add_plugins(gameplay_probe::GameplayProbePlugin);
    app
}
