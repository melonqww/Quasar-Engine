//! Shared runtime integrations used by the editor and Player.

pub mod gameplay;
pub mod physics;

use bevy::{
    asset::AssetPlugin,
    prelude::{App, DefaultPlugins, PluginGroup, Window, WindowPlugin},
};
use bevy_rapier3d::prelude::{NoUserData, RapierPhysicsPlugin, TimestepMode};

/// Creates a minimal app to check that Bevy and the Rapier plugin compose.
pub fn compatibility_probe_app() -> App {
    compatibility_probe_app_with_asset_root("assets".to_owned())
}

/// Creates the probe app while resolving bundled/editor assets from an explicit root.
pub fn compatibility_probe_app_with_asset_root(asset_root: String) -> App {
    compatibility_probe_app_with_asset_root_and_resolution(asset_root, (1440, 900))
}

/// Creates the probe app with an explicit asset root and logical window size.
pub fn compatibility_probe_app_with_asset_root_and_resolution(
    asset_root: String,
    window_resolution: (u32, u32),
) -> App {
    let mut app = App::new();
    app.add_plugins(
        DefaultPlugins
            .set(WindowPlugin {
                primary_window: Some(Window {
                    title: "Quasar Engine — Stage 0 Viewport Probe".into(),
                    name: Some("quasar.viewport_probe".into()),
                    resolution: window_resolution.into(),
                    resizable: true,
                    ..Default::default()
                }),
                ..Default::default()
            })
            .set(AssetPlugin {
                file_path: asset_root,
                ..Default::default()
            }),
    )
    .insert_resource(TimestepMode::Fixed {
        dt: 1.0 / 60.0,
        substeps: 1,
    })
    .add_plugins(RapierPhysicsPlugin::<NoUserData>::default().in_fixed_schedule())
    .add_plugins(physics::CharacterControllerPlugin);
    app
}

/// Evaluates a small Lua 5.4 chunk using the selected vendored mlua backend.
pub fn compatibility_probe_lua() -> mlua::Result<i64> {
    let lua = mlua::Lua::new();
    lua.load("return 6 * 7").eval()
}
