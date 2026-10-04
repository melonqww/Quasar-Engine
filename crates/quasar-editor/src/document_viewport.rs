//! The document scene renders to its own surface; editor panels never cover a window camera.

use bevy::{
    camera::RenderTarget,
    prelude::*,
    render::render_resource::{Extent3d, TextureFormat},
    window::PrimaryWindow,
};
use bevy_egui::{
    EguiGlobalSettings, EguiTextureHandle, EguiUserTextures, PrimaryEguiContext, egui,
};

#[derive(Resource)]
pub(super) struct DocumentViewport {
    pub texture_id: egui::TextureId,
    pub desired_points: Vec2,
    image: Handle<Image>,
    physical_size: UVec2,
}

pub(super) fn setup(
    mut commands: Commands,
    mut images: ResMut<Assets<Image>>,
    mut textures: ResMut<EguiUserTextures>,
    mut settings: ResMut<EguiGlobalSettings>,
) {
    settings.auto_create_primary_context = false;
    let size = UVec2::new(960, 640);
    let image = images.add(Image::new_target_texture(
        size.x,
        size.y,
        TextureFormat::Rgba8UnormSrgb,
        None,
    ));
    let texture_id = textures.add_image(EguiTextureHandle::Strong(image.clone()));
    commands.insert_resource(DocumentViewport {
        texture_id,
        desired_points: size.as_vec2(),
        image: image.clone(),
        physical_size: size,
    });
    commands.spawn((Camera2d, PrimaryEguiContext));
    commands.spawn((
        Camera3d::default(),
        Camera {
            order: -1,
            clear_color: ClearColorConfig::Custom(Color::srgb(0.11, 0.11, 0.12)),
            ..default()
        },
        RenderTarget::Image(image.into()),
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

pub(super) fn resize(
    mut viewport: ResMut<DocumentViewport>,
    window: Single<&Window, With<PrimaryWindow>>,
    mut images: ResMut<Assets<Image>>,
) {
    let size = requested_size(viewport.desired_points, window.scale_factor());
    if size != viewport.physical_size
        && let Some(mut image) = images.get_mut(&viewport.image)
    {
        image.resize(Extent3d {
            width: size.x,
            height: size.y,
            ..default()
        });
        viewport.physical_size = size;
    }
}

fn requested_size(points: Vec2, scale: f32) -> UVec2 {
    (points * scale)
        .round()
        .as_uvec2()
        .clamp(UVec2::splat(96), UVec2::splat(4096))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scene_camera_targets_image_and_ui_has_its_own_window_camera() {
        let mut app = App::new();
        app.insert_resource(Assets::<Image>::default())
            .init_resource::<EguiUserTextures>()
            .init_resource::<EguiGlobalSettings>()
            .add_systems(Startup, setup);
        app.update();
        assert!(
            !app.world()
                .resource::<EguiGlobalSettings>()
                .auto_create_primary_context
        );
        let mut scene_query = app
            .world_mut()
            .query_filtered::<(&Camera, &RenderTarget), With<Camera3d>>();
        let (camera, target) = scene_query.single(app.world()).unwrap();
        assert_eq!(camera.order, -1);
        assert!(matches!(target, RenderTarget::Image(_)));
        let mut ui_query = app
            .world_mut()
            .query_filtered::<Entity, (With<Camera2d>, With<PrimaryEguiContext>)>();
        assert_eq!(ui_query.iter(app.world()).count(), 1);
    }

    #[test]
    fn viewport_resizes_render_surface_for_panel_area_and_dpi() {
        let mut app = App::new();
        let mut images = Assets::<Image>::default();
        let image = images.add(Image::new_target_texture(
            320,
            240,
            TextureFormat::Rgba8UnormSrgb,
            None,
        ));
        app.insert_resource(images)
            .insert_resource(DocumentViewport {
                texture_id: egui::TextureId::Managed(0),
                image: image.clone(),
                physical_size: UVec2::new(320, 240),
                desired_points: Vec2::new(640.0, 480.0),
            });
        let mut window = Window::default();
        window.resolution.set_scale_factor_override(Some(1.5));
        app.world_mut().spawn((window, PrimaryWindow));
        app.add_systems(Update, resize);
        app.update();
        let images = app.world().resource::<Assets<Image>>();
        let surface = images.get(&image).unwrap();
        assert_eq!(surface.texture_descriptor.size.width, 960);
        assert_eq!(surface.texture_descriptor.size.height, 720);
        app.world_mut()
            .resource_mut::<DocumentViewport>()
            .desired_points = Vec2::new(800.0, 450.0);
        app.update();
        assert_eq!(
            app.world()
                .resource::<Assets<Image>>()
                .get(&image)
                .unwrap()
                .size(),
            UVec2::new(1200, 675)
        );
    }

    #[test]
    fn viewport_size_is_bounded_for_tiny_and_large_layouts() {
        assert_eq!(requested_size(Vec2::ZERO, 1.0), UVec2::splat(96));
        assert_eq!(
            requested_size(Vec2::splat(10000.0), 2.0),
            UVec2::splat(4096)
        );
    }
}
