mod svg_loader;

use egui::{
    Align2, Color32, ColorImage, CornerRadius, FontData, FontDefinitions, FontFamily, FontId,
    Response, Sense, Stroke, TextStyle, Ui, Vec2, WidgetInfo, WidgetType,
};
use lucide_icons::Icon;

pub(crate) const BACKGROUND: Color32 = Color32::from_rgb(246, 244, 239);
pub(crate) const SURFACE: Color32 = Color32::from_rgb(255, 255, 255);
pub(crate) const SURFACE_MUTED: Color32 = Color32::from_rgb(240, 238, 233);
pub(crate) const TEXT: Color32 = Color32::from_rgb(38, 38, 36);
pub(crate) const MUTED: Color32 = Color32::from_rgb(118, 116, 109);
pub(crate) const BORDER: Color32 = Color32::from_rgb(218, 215, 207);
pub(crate) const ACCENT: Color32 = Color32::from_rgb(68, 137, 103);
pub(crate) const ACCENT_SOFT: Color32 = Color32::from_rgb(222, 237, 228);

pub(crate) fn configure(ctx: &egui::Context) {
    egui_extras::install_image_loaders(ctx);
    svg_loader::install(ctx);
    // Application state is mutated while building a frame. A single pass keeps
    // keyboard and pointer actions exactly-once; the retained reader layout
    // performs its own explicit invalidation when geometry changes.
    ctx.options_mut(|options| options.max_passes = 1.try_into().expect("one is non-zero"));
    // egui's one-pixel transparent feather produces a dark halo around rounded
    // fills with the current wgpu composition path. Disable that fringe and keep
    // rounded geometry snapped so control edges stay crisp at Windows scale factors.
    ctx.tessellation_options_mut(|options| {
        options.feathering = false;
        options.round_rects_to_pixels = true;
    });
    let mut fonts = FontDefinitions::default();
    fonts.font_data.insert(
        "reader-cjk".into(),
        FontData::from_static(crate::fonts::cjk_font_bytes()).into(),
    );
    fonts.font_data.insert(
        "lucide".into(),
        FontData::from_static(lucide_icons::LUCIDE_FONT_BYTES).into(),
    );
    fonts
        .families
        .insert(FontFamily::Name("lucide".into()), vec!["lucide".into()]);
    for family in [FontFamily::Proportional, FontFamily::Monospace] {
        fonts
            .families
            .entry(family)
            .or_default()
            .insert(0, "reader-cjk".into());
    }
    ctx.set_fonts(fonts);

    let mut visuals = egui::Visuals::light();
    visuals.panel_fill = BACKGROUND;
    visuals.window_fill = SURFACE;
    visuals.window_stroke = Stroke::new(1.0, BORDER);
    visuals.window_corner_radius = CornerRadius::same(10);
    visuals.text_edit_bg_color = Some(SURFACE);
    visuals.widgets.inactive.bg_fill = SURFACE_MUTED;
    visuals.widgets.inactive.weak_bg_fill = SURFACE_MUTED;
    visuals.widgets.inactive.bg_stroke = Stroke::new(1.0, BORDER);
    visuals.widgets.inactive.corner_radius = CornerRadius::same(6);
    visuals.widgets.inactive.fg_stroke = Stroke::new(1.0, TEXT);
    visuals.widgets.hovered.bg_fill = Color32::from_rgb(231, 235, 228);
    visuals.widgets.hovered.weak_bg_fill = Color32::from_rgb(237, 240, 235);
    visuals.widgets.hovered.bg_stroke = Stroke::new(1.0, Color32::from_rgb(171, 184, 174));
    visuals.widgets.hovered.corner_radius = CornerRadius::same(6);
    visuals.widgets.active.bg_fill = Color32::from_rgb(219, 229, 221);
    visuals.widgets.active.weak_bg_fill = Color32::from_rgb(228, 237, 230);
    visuals.widgets.active.bg_stroke = Stroke::new(1.5, ACCENT);
    visuals.widgets.active.corner_radius = CornerRadius::same(6);
    visuals.widgets.open.bg_fill = Color32::from_rgb(237, 240, 235);
    visuals.widgets.open.weak_bg_fill = Color32::from_rgb(237, 240, 235);
    visuals.widgets.open.bg_stroke = Stroke::new(1.5, ACCENT);
    visuals.widgets.open.corner_radius = CornerRadius::same(6);
    visuals.text_cursor.stroke = Stroke::new(1.5, ACCENT);
    visuals.selection.bg_fill = Color32::from_rgba_unmultiplied(68, 137, 103, 64);
    visuals.selection.stroke = Stroke::new(1.0, TEXT);
    ctx.set_visuals(visuals);
    ctx.all_styles_mut(|style| {
        // Application chrome should not behave like selectable document text.
        // Reader text selection is handled by the Vello-backed reader itself.
        style.interaction.selectable_labels = false;
        // egui 0.35's debug-only rect/id diagnostic has known false positives for
        // right-to-left child layouts and virtualized/animated regions (#8343,
        // #8092), painting bright red boxes into otherwise valid frames.
        #[cfg(debug_assertions)]
        {
            style.debug.warn_if_rect_changes_id = false;
        }
        // The default edge fades look like detached gray bars on a solid sidebar.
        style.spacing.scroll.fade.strength = 0.0;
        // Use the same soft surface/accent palette as the rest of the app. The
        // egui floating preset otherwise paints a dragged handle with the dark
        // foreground color, which looks almost black in the light theme.
        let scroll = &mut style.spacing.scroll;
        scroll.foreground_color = false;
        scroll.bar_width = 8.0;
        scroll.floating_width = 3.0;
        scroll.handle_min_length = 28.0;
        scroll.active_background_opacity = 0.18;
        scroll.interact_background_opacity = 0.32;
        scroll.active_handle_opacity = 0.72;
        scroll.interact_handle_opacity = 0.95;
    });
}

pub(crate) fn icon(icon: Icon) -> egui::RichText {
    egui::RichText::new(icon.unicode().to_string())
        .family(egui::FontFamily::Name("lucide".into()))
        .size(17.0)
}

/// A compact icon action painted as one borderless rounded layer.
///
/// Avoiding egui's native button frame prevents its global stroke from leaving
/// anti-aliased corner fragments around transparent and selected icon buttons.
pub(crate) fn icon_button(ui: &mut Ui, glyph: Icon) -> Response {
    painted_icon_button(ui, glyph, false)
}

/// Icon action used as a tab. The selected state uses a quiet accent surface
/// instead of the high-contrast fill intended for primary actions.
pub(crate) fn selectable_icon_button(ui: &mut Ui, glyph: Icon, selected: bool) -> Response {
    painted_icon_button(ui, glyph, selected)
}

/// Compact toolbar toggle with an explicit text state, so the inactive state is
/// understandable without relying on color or a hover tooltip.
pub(crate) fn toggle_icon_button(
    ui: &mut Ui,
    glyph: Icon,
    selected: bool,
    on_label: &str,
    off_label: &str,
) -> Response {
    let (rect, response) = ui.allocate_exact_size(Vec2::new(52.0, 32.0), Sense::click());
    let fill = if selected {
        ACCENT_SOFT
    } else if response.is_pointer_button_down_on() {
        ui.visuals().widgets.active.weak_bg_fill
    } else if response.hovered() {
        ui.visuals().widgets.hovered.weak_bg_fill
    } else {
        Color32::TRANSPARENT
    };
    let foreground = if selected { ACCENT } else { MUTED };
    let state_label = if selected { on_label } else { off_label };

    if ui.is_rect_visible(rect) {
        let painter = ui.painter();
        if fill != Color32::TRANSPARENT {
            painter.rect_filled(rect, 6.0, fill);
        }
        let icon_galley = painter.layout_no_wrap(
            glyph.unicode().to_string(),
            FontId::new(16.0, FontFamily::Name("lucide".into())),
            foreground,
        );
        let label_galley = painter.layout_no_wrap(
            state_label.to_owned(),
            FontId::proportional(11.0),
            foreground,
        );
        let gap = 4.0;
        let content_width = icon_galley.size().x + gap + label_galley.size().x;
        let start_x = rect.center().x - content_width / 2.0;
        painter.galley(
            egui::pos2(start_x, rect.center().y - icon_galley.size().y / 2.0),
            icon_galley,
            foreground,
        );
        painter.galley(
            egui::pos2(
                start_x + content_width - label_galley.size().x,
                rect.center().y - label_galley.size().y / 2.0,
            ),
            label_galley,
            foreground,
        );
    }
    response.widget_info(|| WidgetInfo::labeled(WidgetType::Button, ui.is_enabled(), state_label));
    response.on_hover_cursor(egui::CursorIcon::PointingHand)
}

fn painted_icon_button(ui: &mut Ui, glyph: Icon, selected: bool) -> Response {
    let (rect, response) = ui.allocate_exact_size(Vec2::splat(32.0), Sense::click());
    let fill = if selected {
        ACCENT_SOFT
    } else if response.is_pointer_button_down_on() {
        ui.visuals().widgets.active.weak_bg_fill
    } else if response.hovered() {
        ui.visuals().widgets.hovered.weak_bg_fill
    } else {
        Color32::TRANSPARENT
    };
    let foreground = if selected { ACCENT } else { TEXT };
    let label = glyph.unicode().to_string();

    if ui.is_rect_visible(rect) {
        if fill != Color32::TRANSPARENT {
            ui.painter().rect_filled(rect, 6.0, fill);
        }
        ui.painter().text(
            rect.center(),
            Align2::CENTER_CENTER,
            &label,
            FontId::new(17.0, FontFamily::Name("lucide".into())),
            foreground,
        );
    }
    response.widget_info(|| WidgetInfo::labeled(WidgetType::Button, ui.is_enabled(), &label));
    response.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// Full-width navigation/menu row with left-aligned icon and label.
pub(crate) fn navigation_button(ui: &mut Ui, glyph: Icon, label: &str, selected: bool) -> Response {
    let desired_size = Vec2::new(ui.available_width(), 36.0);
    let (rect, response) = ui.allocate_exact_size(desired_size, Sense::click());
    let fill = if selected {
        ACCENT_SOFT
    } else if response.is_pointer_button_down_on() {
        ui.visuals().widgets.active.bg_fill
    } else if response.hovered() {
        ui.visuals().widgets.hovered.bg_fill
    } else {
        Color32::TRANSPARENT
    };
    let foreground = if selected { ACCENT } else { TEXT };

    if ui.is_rect_visible(rect) {
        ui.painter().rect_filled(rect, 6.0, fill);
        ui.painter().text(
            egui::pos2(rect.left() + 10.0, rect.center().y),
            Align2::LEFT_CENTER,
            glyph.unicode().to_string(),
            FontId::new(17.0, FontFamily::Name("lucide".into())),
            foreground,
        );
        ui.painter().text(
            egui::pos2(rect.left() + 38.0, rect.center().y),
            Align2::LEFT_CENTER,
            label,
            TextStyle::Body.resolve(ui.style()),
            foreground,
        );
    }
    response.widget_info(|| WidgetInfo::labeled(WidgetType::Button, ui.is_enabled(), label));
    response
}

/// Full-width navigation/menu row with a left-aligned label and no icon slot.
pub(crate) fn navigation_text_button(ui: &mut Ui, label: &str, selected: bool) -> Response {
    let desired_size = Vec2::new(ui.available_width(), 36.0);
    let (rect, response) = ui.allocate_exact_size(desired_size, Sense::click());
    let fill = if selected {
        ACCENT_SOFT
    } else if response.is_pointer_button_down_on() {
        ui.visuals().widgets.active.bg_fill
    } else if response.hovered() {
        ui.visuals().widgets.hovered.bg_fill
    } else {
        Color32::TRANSPARENT
    };
    let foreground = if selected { ACCENT } else { TEXT };

    if ui.is_rect_visible(rect) {
        ui.painter().rect_filled(rect, 6.0, fill);
        ui.painter().text(
            egui::pos2(rect.left() + 10.0, rect.center().y),
            Align2::LEFT_CENTER,
            label,
            TextStyle::Body.resolve(ui.style()),
            foreground,
        );
    }
    response.widget_info(|| WidgetInfo::labeled(WidgetType::Button, ui.is_enabled(), label));
    response
}

pub(crate) fn decode_color_image(bytes: &[u8]) -> Result<ColorImage, image::ImageError> {
    let image = image::load_from_memory(bytes)?.to_rgba8();
    let size = [image.width() as usize, image.height() as usize];
    Ok(ColorImage::from_rgba_unmultiplied(size, image.as_raw()))
}
