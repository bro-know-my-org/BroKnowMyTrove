use eframe::egui::{self, Color32, Stroke};

pub const INK: Color32 = Color32::from_gray(242);
pub const MUTED: Color32 = Color32::from_rgba_premultiplied(179, 179, 179, 179);
pub const ACCENT: Color32 = Color32::from_rgb(10, 132, 255);
pub const BACKGROUND: Color32 = Color32::from_rgb(44, 44, 46);
pub const DEEP: Color32 = Color32::from_rgb(28, 28, 30);
pub const SURFACE: Color32 = Color32::from_rgb(58, 58, 60);
pub const BORDER: Color32 = Color32::from_rgba_premultiplied(26, 26, 26, 26);

pub fn clear_widget_memory(ctx: &egui::Context) {
    ctx.memory_mut(|memory| {
        let options = memory.options.clone();
        *memory = Default::default();
        memory.options = options; // Preserve appearance, discard editing and undo state.
    });
}

pub fn install(ctx: &egui::Context) {
    ctx.set_theme(egui::Theme::Dark);
    let mut style = egui::Style::default();
    for text_style in [egui::TextStyle::Body, egui::TextStyle::Button] {
        style
            .text_styles
            .insert(text_style, egui::FontId::proportional(14.0));
    }
    style.text_styles.insert(
        egui::TextStyle::Heading,
        egui::FontId::new(20.0, egui::FontFamily::Name("serif".into())),
    );
    style.spacing.item_spacing = egui::vec2(8.0, 6.0);
    style.spacing.button_padding = egui::vec2(10.0, 5.0);
    style.spacing.interact_size.y = 28.0;
    style.spacing.text_edit_width = 240.0;
    style.animation_time = 0.0;
    style.scroll_animation = egui::style::ScrollAnimation::none();
    style.visuals = egui::Visuals::dark();
    let visuals = &mut style.visuals;
    visuals.panel_fill = BACKGROUND;
    visuals.window_fill = BACKGROUND;
    visuals.extreme_bg_color = DEEP;
    visuals.text_edit_bg_color = Some(DEEP);
    visuals.code_bg_color = DEEP;
    visuals.faint_bg_color = SURFACE;
    visuals.weak_text_color = Some(MUTED);
    visuals.selection.bg_fill = SURFACE;
    visuals.selection.stroke = Stroke::new(1.0, ACCENT);
    visuals.hyperlink_color = ACCENT;
    visuals.window_shadow = egui::epaint::Shadow::NONE;
    visuals.popup_shadow = egui::epaint::Shadow::NONE;
    visuals.window_stroke = Stroke::new(1.0, BORDER);
    visuals.window_corner_radius = 12.into();
    visuals.menu_corner_radius = 8.into();
    visuals.warn_fg_color = INK;
    visuals.error_fg_color = INK;
    visuals.text_cursor.stroke = Stroke::new(1.0, ACCENT);
    visuals.text_cursor.blink = false;
    for widget in [
        &mut visuals.widgets.noninteractive,
        &mut visuals.widgets.inactive,
        &mut visuals.widgets.hovered,
        &mut visuals.widgets.active,
        &mut visuals.widgets.open,
    ] {
        widget.corner_radius = 4.into();
        widget.fg_stroke = Stroke::new(1.0, INK);
        widget.bg_stroke = Stroke::new(1.0, BORDER);
        widget.bg_fill = SURFACE;
        widget.weak_bg_fill = SURFACE;
        widget.expansion = 0.0;
    }
    visuals.widgets.noninteractive.bg_fill = BACKGROUND;
    visuals.widgets.noninteractive.weak_bg_fill = BACKGROUND;
    visuals.widgets.noninteractive.bg_stroke.color = Color32::from_white_alpha(20);
    visuals.widgets.hovered.bg_stroke.color = Color32::from_white_alpha(31);
    visuals.widgets.hovered.fg_stroke.color = ACCENT;
    visuals.widgets.active.fg_stroke.color = ACCENT;
    ctx.set_style_of(egui::Theme::Dark, style);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clearing_widget_state_preserves_appearance() {
        let ctx = egui::Context::default();
        install(&ctx);
        let id = egui::Id::new("editing-state");
        ctx.data_mut(|data| data.insert_temp(id, 42_u32));
        clear_widget_memory(&ctx);
        assert_eq!(ctx.data(|data| data.get_temp::<u32>(id)), None);
        let style = ctx.global_style();
        assert_eq!(style.visuals.panel_fill, BACKGROUND);
        assert_eq!(style.spacing.interact_size.y, 28.0);
        assert_eq!(ctx.theme(), egui::Theme::Dark);
    }
}
