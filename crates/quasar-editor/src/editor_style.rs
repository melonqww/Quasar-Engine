//! Shared neutral editor chrome. Colors and spacing live here, not in document commands.

use bevy_egui::egui::{self, Color32, Pos2, Response, Stroke, Vec2};

pub const BACKGROUND: Color32 = Color32::from_rgb(24, 24, 26);
pub const PANEL: Color32 = Color32::from_rgb(32, 32, 35);
pub const FIELD: Color32 = Color32::from_rgb(27, 27, 30);
pub const BORDER: Color32 = Color32::from_rgb(57, 57, 62);
pub const TEXT: Color32 = Color32::from_rgb(224, 224, 228);
pub const MUTED: Color32 = Color32::from_rgb(145, 145, 154);
pub const ACCENT: Color32 = Color32::from_rgb(219, 169, 91);
pub const SELECTED: Color32 = Color32::from_rgb(74, 62, 43);

pub fn install(ctx: &egui::Context) {
    let id = egui::Id::new("quasar-neutral-theme");
    if ctx.data_mut(|data| data.get_temp::<bool>(id).unwrap_or(false)) {
        return;
    }
    ctx.set_theme(egui::Theme::Dark);
    let mut style = (*ctx.style_of(egui::Theme::Dark)).clone();
    style.spacing.item_spacing = Vec2::new(8.0, 8.0);
    style.spacing.button_padding = Vec2::new(10.0, 7.0);
    style.spacing.interact_size = Vec2::new(32.0, 30.0);
    style.spacing.indent = 18.0;
    style
        .text_styles
        .insert(egui::TextStyle::Body, egui::FontId::proportional(14.0));
    style
        .text_styles
        .insert(egui::TextStyle::Button, egui::FontId::proportional(14.0));
    style
        .text_styles
        .insert(egui::TextStyle::Heading, egui::FontId::proportional(16.0));
    style.visuals = egui::Visuals::dark();
    style.visuals.override_text_color = Some(TEXT);
    style.visuals.weak_text_color = Some(MUTED);
    style.visuals.panel_fill = BACKGROUND;
    style.visuals.window_fill = PANEL;
    style.visuals.extreme_bg_color = FIELD;
    style.visuals.faint_bg_color = PANEL;
    style.visuals.selection.bg_fill = SELECTED;
    style.visuals.selection.stroke = Stroke::new(1.0, ACCENT);
    for widget in [
        &mut style.visuals.widgets.noninteractive,
        &mut style.visuals.widgets.inactive,
        &mut style.visuals.widgets.hovered,
        &mut style.visuals.widgets.active,
        &mut style.visuals.widgets.open,
    ] {
        widget.corner_radius = 6.into();
        widget.bg_stroke = Stroke::new(1.0, BORDER);
        widget.fg_stroke = Stroke::new(1.0, TEXT);
    }
    style.visuals.widgets.inactive.weak_bg_fill = FIELD;
    style.visuals.widgets.inactive.bg_fill = FIELD;
    style.visuals.widgets.hovered.weak_bg_fill = Color32::from_gray(54);
    style.visuals.widgets.hovered.bg_fill = Color32::from_gray(54);
    style.visuals.widgets.active.weak_bg_fill = SELECTED;
    style.visuals.widgets.active.bg_fill = SELECTED;
    ctx.set_style_of(egui::Theme::Dark, style);
    ctx.data_mut(|data| data.insert_temp(id, true));
}

pub fn panel_frame() -> egui::Frame {
    egui::Frame::new()
        .fill(PANEL)
        .stroke(Stroke::new(1.0, BORDER))
        .corner_radius(10)
        .inner_margin(14)
        .outer_margin(egui::Margin::symmetric(6, 8))
}

pub fn toolbar_frame() -> egui::Frame {
    egui::Frame::new()
        .fill(BACKGROUND)
        .inner_margin(egui::Margin::symmetric(14, 10))
}

pub fn tab(ui: &mut egui::Ui, label: &str, selected: bool) -> Response {
    ui.add(
        egui::Button::new(egui::RichText::new(label).color(if selected { ACCENT } else { TEXT }))
            .fill(if selected { SELECTED } else { PANEL })
            .stroke(Stroke::new(
                1.0,
                if selected {
                    Color32::from_rgb(108, 87, 56)
                } else {
                    BORDER
                },
            ))
            .corner_radius(8)
            .min_size(Vec2::new(90.0, 34.0)),
    )
}

#[derive(Clone, Copy)]
pub enum Icon {
    Cube,
    Select,
    Move,
    Rotate,
    Scale,
    Save,
    Undo,
    Redo,
    Play,
    Plus,
}

pub fn icon_button(
    ui: &mut egui::Ui,
    icon: Icon,
    selected: bool,
    enabled: bool,
    hint: &str,
) -> Response {
    let response = ui.add_enabled(
        enabled,
        egui::Button::new("")
            .fill(if selected {
                SELECTED
            } else {
                Color32::TRANSPARENT
            })
            .stroke(if selected {
                Stroke::new(1.0, ACCENT.gamma_multiply(0.6))
            } else {
                Stroke::NONE
            })
            .min_size(Vec2::splat(34.0)),
    );
    let color = if !enabled {
        MUTED.gamma_multiply(0.45)
    } else if selected {
        ACCENT
    } else {
        TEXT
    };
    paint_icon(ui.painter(), response.rect.center(), icon, color);
    response.on_hover_text(hint)
}

fn paint_icon(painter: &egui::Painter, center: Pos2, icon: Icon, color: Color32) {
    let stroke = Stroke::new(1.5, color);
    let p = |x, y| center + Vec2::new(x, y);
    match icon {
        Icon::Cube => {
            painter.add(egui::Shape::closed_line(
                vec![
                    p(0.0, -7.0),
                    p(6.0, -3.0),
                    p(6.0, 4.0),
                    p(0.0, 8.0),
                    p(-6.0, 4.0),
                    p(-6.0, -3.0),
                ],
                stroke,
            ));
            painter.line_segment([p(-6.0, -3.0), p(0.0, 1.0)], stroke);
            painter.line_segment([p(6.0, -3.0), p(0.0, 1.0)], stroke);
            painter.line_segment([p(0.0, 1.0), p(0.0, 8.0)], stroke);
        }
        Icon::Plus => {
            painter.line_segment([p(-6.0, 0.0), p(6.0, 0.0)], stroke);
            painter.line_segment([p(0.0, -6.0), p(0.0, 6.0)], stroke);
        }
        Icon::Play | Icon::Select => {
            let points = if matches!(icon, Icon::Play) {
                vec![p(-4.0, -7.0), p(7.0, 0.0), p(-4.0, 7.0)]
            } else {
                vec![p(-6.0, -8.0), p(7.0, 1.0), p(0.0, 3.0), p(-3.0, 8.0)]
            };
            painter.add(egui::Shape::convex_polygon(points, color, Stroke::NONE));
        }
        Icon::Save => {
            painter.rect_stroke(
                egui::Rect::from_center_size(center, Vec2::splat(15.0)),
                2,
                stroke,
                egui::StrokeKind::Inside,
            );
            painter.rect_stroke(
                egui::Rect::from_min_max(p(-4.0, 1.0), p(4.0, 6.0)),
                0,
                stroke,
                egui::StrokeKind::Inside,
            );
            painter.line_segment([p(-3.0, -6.0), p(-3.0, -1.0)], stroke);
        }
        Icon::Move => {
            for direction in [Vec2::X, Vec2::Y, -Vec2::X, -Vec2::Y] {
                let tip = center + direction * 8.0;
                let base = center + direction * 4.0;
                let normal = Vec2::new(-direction.y, direction.x) * 3.0;
                painter.line_segment([center, tip], stroke);
                painter.line_segment([base + normal, tip], stroke);
                painter.line_segment([base - normal, tip], stroke);
            }
        }
        Icon::Scale => {
            painter.rect_stroke(
                egui::Rect::from_center_size(center, Vec2::splat(13.0)),
                1,
                stroke,
                egui::StrokeKind::Inside,
            );
            painter.line_segment([p(-3.0, 3.0), p(8.0, -8.0)], stroke);
            painter.line_segment([p(2.0, -8.0), p(8.0, -8.0)], stroke);
            painter.line_segment([p(8.0, -8.0), p(8.0, -2.0)], stroke);
        }
        Icon::Rotate => {
            painter.circle_stroke(center, 7.0, stroke);
            painter.line_segment([p(0.0, -10.0), p(0.0, -3.0)], stroke);
            painter.line_segment([p(0.0, -3.0), p(5.0, -5.0)], stroke);
        }
        Icon::Undo | Icon::Redo => {
            let sign = if matches!(icon, Icon::Undo) {
                1.0
            } else {
                -1.0
            };
            painter.add(egui::Shape::line(
                vec![
                    p(5.0 * sign, 6.0),
                    p(7.0 * sign, 1.0),
                    p(4.0 * sign, -4.0),
                    p(-7.0 * sign, -4.0),
                ],
                stroke,
            ));
            painter.line_segment([p(-7.0 * sign, -4.0), p(-2.0 * sign, -8.0)], stroke);
            painter.line_segment([p(-7.0 * sign, -4.0), p(-2.0 * sign, 1.0)], stroke);
        }
    }
}

pub fn object_row(ui: &mut egui::Ui, name: &str, depth: usize, selected: bool) -> Response {
    let indent = depth.min(8);
    let response = ui.add_sized(
        [ui.available_width(), 30.0],
        egui::Button::new(format!("{}     {name}", "   ".repeat(indent)))
            .right_text("")
            .truncate()
            .selected(selected)
            .frame_when_inactive(false),
    );
    paint_icon(
        ui.painter(),
        egui::pos2(
            response.rect.left() + 16.0 + indent as f32 * 12.0,
            response.rect.center().y,
        ),
        Icon::Cube,
        if selected { ACCENT } else { MUTED },
    );
    response.on_hover_text(name)
}
