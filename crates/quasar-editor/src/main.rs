#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]

use bevy::{
    prelude::*,
    render::view::screenshot::{Screenshot, save_to_disk},
};

#[derive(Resource)]
struct ScreenshotRequest(String);

fn main() {
    let mut app = quasar_editor::compatibility_probe_app();
    if let Some(path) = screenshot_path() {
        app.insert_resource(ScreenshotRequest(path));
        app.add_systems(Update, capture_screenshot_after_render_warmup);
    }
    app.run();
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

fn screenshot_path() -> Option<String> {
    let mut args = std::env::args().skip(1);
    while let Some(argument) = args.next() {
        if argument == "--screenshot" {
            return args.next();
        }
    }
    None
}
