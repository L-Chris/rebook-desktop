mod svg_loader;

use std::sync::atomic::{AtomicU8, Ordering};

use egui::{
    Align2, Color32, ColorImage, CornerRadius, FontData, FontDefinitions, FontFamily, FontId,
    Response, Sense, Stroke, TextStyle, Ui, Vec2, WidgetInfo, WidgetType,
};
use lucide_icons::Icon;

use crate::preferences::AppTheme;

/// Theme-dependent color set. Chrome reads colors through `palette()` so a
/// saved theme switch recolors the whole app without threading state through
/// every view.
#[derive(Clone, Copy)]
pub(crate) struct Palette {
    pub(crate) dark: bool,
    pub(crate) background: Color32,
    pub(crate) surface: Color32,
    pub(crate) surface_muted: Color32,
    pub(crate) text: Color32,
    pub(crate) muted: Color32,
    pub(crate) border: Color32,
    pub(crate) accent: Color32,
    pub(crate) accent_soft: Color32,
    pub(crate) hovered_fill: Color32,
    pub(crate) hovered_weak_fill: Color32,
    pub(crate) hovered_stroke: Color32,
    pub(crate) active_fill: Color32,
    pub(crate) active_weak_fill: Color32,
    pub(crate) open_fill: Color32,
    pub(crate) selection_fill: Color32,
    pub(crate) error: Color32,
    pub(crate) error_fill: Color32,
    pub(crate) error_stroke: Color32,
    pub(crate) error_text: Color32,
    pub(crate) toast_error_fill: Color32,
    pub(crate) card_fill: Color32,
    pub(crate) accent_border: Color32,
    pub(crate) pill_fill: Color32,
    pub(crate) pill_stroke: Color32,
}

impl Palette {
    fn light() -> Self {
        Self {
            dark: false,
            background: Color32::from_rgb(246, 244, 239),
            surface: Color32::from_rgb(255, 255, 255),
            surface_muted: Color32::from_rgb(240, 238, 233),
            text: Color32::from_rgb(38, 38, 36),
            muted: Color32::from_rgb(118, 116, 109),
            border: Color32::from_rgb(218, 215, 207),
            accent: Color32::from_rgb(68, 137, 103),
            accent_soft: Color32::from_rgb(222, 237, 228),
            hovered_fill: Color32::from_rgb(231, 235, 228),
            hovered_weak_fill: Color32::from_rgb(237, 240, 235),
            hovered_stroke: Color32::from_rgb(171, 184, 174),
            active_fill: Color32::from_rgb(219, 229, 221),
            active_weak_fill: Color32::from_rgb(228, 237, 230),
            open_fill: Color32::from_rgb(237, 240, 235),
            selection_fill: Color32::from_rgba_unmultiplied(68, 137, 103, 64),
            error: Color32::from_rgb(180, 55, 55),
            error_fill: Color32::from_rgb(252, 239, 238),
            error_stroke: Color32::from_rgb(226, 180, 176),
            error_text: Color32::from_rgb(151, 54, 50),
            toast_error_fill: Color32::from_rgb(78, 39, 39),
            card_fill: Color32::from_rgb(251, 250, 247),
            accent_border: Color32::from_rgb(177, 209, 190),
            pill_fill: Color32::from_rgb(231, 235, 242),
            pill_stroke: Color32::from_rgb(220, 223, 228),
        }
    }

    fn dark() -> Self {
        Self {
            dark: true,
            background: Color32::from_rgb(32, 31, 28),
            surface: Color32::from_rgb(42, 41, 37),
            surface_muted: Color32::from_rgb(52, 50, 45),
            text: Color32::from_rgb(232, 230, 225),
            muted: Color32::from_rgb(150, 147, 138),
            border: Color32::from_rgb(64, 62, 56),
            accent: Color32::from_rgb(88, 148, 114),
            accent_soft: Color32::from_rgb(44, 62, 52),
            hovered_fill: Color32::from_rgb(52, 57, 51),
            hovered_weak_fill: Color32::from_rgb(46, 49, 44),
            hovered_stroke: Color32::from_rgb(90, 102, 92),
            active_fill: Color32::from_rgb(48, 58, 51),
            active_weak_fill: Color32::from_rgb(44, 54, 48),
            open_fill: Color32::from_rgb(46, 49, 44),
            selection_fill: Color32::from_rgba_unmultiplied(88, 148, 114, 72),
            error: Color32::from_rgb(219, 120, 111),
            error_fill: Color32::from_rgb(64, 40, 38),
            error_stroke: Color32::from_rgb(110, 64, 60),
            error_text: Color32::from_rgb(224, 138, 130),
            toast_error_fill: Color32::from_rgb(96, 46, 44),
            card_fill: Color32::from_rgb(47, 46, 42),
            accent_border: Color32::from_rgb(62, 94, 78),
            pill_fill: Color32::from_rgb(54, 53, 48),
            pill_stroke: Color32::from_rgb(72, 70, 64),
        }
    }

    // Glassmorphism approximation: egui has no backdrop blur, so frosted
    // surfaces are translucent white layers over a cool slate background.
    fn glass() -> Self {
        Self {
            dark: false,
            background: Color32::from_rgb(219, 227, 238),
            surface: Color32::from_rgba_unmultiplied(255, 255, 255, 168),
            surface_muted: Color32::from_rgba_unmultiplied(255, 255, 255, 108),
            text: Color32::from_rgb(30, 41, 59),
            muted: Color32::from_rgb(100, 116, 139),
            border: Color32::from_rgba_unmultiplied(255, 255, 255, 150),
            accent: Color32::from_rgb(79, 70, 229),
            accent_soft: Color32::from_rgba_unmultiplied(99, 102, 241, 42),
            hovered_fill: Color32::from_rgba_unmultiplied(255, 255, 255, 150),
            hovered_weak_fill: Color32::from_rgba_unmultiplied(255, 255, 255, 96),
            hovered_stroke: Color32::from_rgba_unmultiplied(148, 163, 184, 140),
            active_fill: Color32::from_rgba_unmultiplied(255, 255, 255, 176),
            active_weak_fill: Color32::from_rgba_unmultiplied(255, 255, 255, 128),
            open_fill: Color32::from_rgba_unmultiplied(255, 255, 255, 96),
            selection_fill: Color32::from_rgba_unmultiplied(99, 102, 241, 72),
            error: Color32::from_rgb(220, 38, 38),
            error_fill: Color32::from_rgba_unmultiplied(254, 226, 226, 200),
            error_stroke: Color32::from_rgba_unmultiplied(252, 165, 165, 200),
            error_text: Color32::from_rgb(185, 28, 28),
            toast_error_fill: Color32::from_rgb(78, 39, 39),
            card_fill: Color32::from_rgba_unmultiplied(255, 255, 255, 140),
            accent_border: Color32::from_rgba_unmultiplied(99, 102, 241, 128),
            pill_fill: Color32::from_rgba_unmultiplied(255, 255, 255, 120),
            pill_stroke: Color32::from_rgba_unmultiplied(148, 163, 184, 110),
        }
    }
}

static CURRENT_THEME: AtomicU8 = AtomicU8::new(0);

pub(crate) fn set_theme(theme: AppTheme) {
    let value = match theme {
        AppTheme::Light => 0,
        AppTheme::Dark => 1,
        AppTheme::Glass => 2,
    };
    CURRENT_THEME.store(value, Ordering::Relaxed);
}

pub(crate) fn theme() -> AppTheme {
    match CURRENT_THEME.load(Ordering::Relaxed) {
        1 => AppTheme::Dark,
        2 => AppTheme::Glass,
        _ => AppTheme::Light,
    }
}

pub(crate) fn palette() -> Palette {
    match theme() {
        AppTheme::Light => Palette::light(),
        AppTheme::Dark => Palette::dark(),
        AppTheme::Glass => Palette::glass(),
    }
}

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

    apply_visuals(ctx, &Palette::light());
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

// Rebuild egui visuals from a palette. Startup applies the light palette;
// a saved theme change re-applies the matching palette and repaints.
pub(crate) fn apply_visuals(ctx: &egui::Context, palette: &Palette) {
    let mut visuals = if palette.dark {
        egui::Visuals::dark()
    } else {
        egui::Visuals::light()
    };
    visuals.panel_fill = palette.background;
    visuals.window_fill = palette.surface;
    visuals.window_stroke = Stroke::new(1.0, palette.border);
    visuals.window_corner_radius = CornerRadius::same(10);
    visuals.text_edit_bg_color = Some(palette.surface);
    visuals.widgets.inactive.bg_fill = palette.surface_muted;
    visuals.widgets.inactive.weak_bg_fill = palette.surface_muted;
    visuals.widgets.inactive.bg_stroke = Stroke::new(1.0, palette.border);
    visuals.widgets.inactive.corner_radius = CornerRadius::same(6);
    visuals.widgets.inactive.fg_stroke = Stroke::new(1.0, palette.text);
    visuals.widgets.hovered.bg_fill = palette.hovered_fill;
    visuals.widgets.hovered.weak_bg_fill = palette.hovered_weak_fill;
    visuals.widgets.hovered.bg_stroke = Stroke::new(1.0, palette.hovered_stroke);
    visuals.widgets.hovered.corner_radius = CornerRadius::same(6);
    visuals.widgets.active.bg_fill = palette.active_fill;
    visuals.widgets.active.weak_bg_fill = palette.active_weak_fill;
    visuals.widgets.active.bg_stroke = Stroke::new(1.5, palette.accent);
    visuals.widgets.active.corner_radius = CornerRadius::same(6);
    visuals.widgets.open.bg_fill = palette.open_fill;
    visuals.widgets.open.weak_bg_fill = palette.open_fill;
    visuals.widgets.open.bg_stroke = Stroke::new(1.5, palette.accent);
    visuals.widgets.open.corner_radius = CornerRadius::same(6);
    visuals.text_cursor.stroke = Stroke::new(1.5, palette.accent);
    visuals.selection.bg_fill = palette.selection_fill;
    visuals.selection.stroke = Stroke::new(1.0, palette.text);
    ctx.set_visuals(visuals);
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
    let palette = palette();
    let fill = if selected {
        palette.accent_soft
    } else if response.is_pointer_button_down_on() {
        ui.visuals().widgets.active.weak_bg_fill
    } else if response.hovered() {
        ui.visuals().widgets.hovered.weak_bg_fill
    } else {
        Color32::TRANSPARENT
    };
    let foreground = if selected {
        palette.accent
    } else {
        palette.muted
    };
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
    let palette = palette();
    let fill = if selected {
        palette.accent_soft
    } else if response.is_pointer_button_down_on() {
        ui.visuals().widgets.active.weak_bg_fill
    } else if response.hovered() {
        ui.visuals().widgets.hovered.weak_bg_fill
    } else {
        Color32::TRANSPARENT
    };
    let foreground = if selected {
        palette.accent
    } else {
        palette.text
    };
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
    let palette = palette();
    let fill = if selected {
        palette.accent_soft
    } else if response.is_pointer_button_down_on() {
        ui.visuals().widgets.active.bg_fill
    } else if response.hovered() {
        ui.visuals().widgets.hovered.bg_fill
    } else {
        Color32::TRANSPARENT
    };
    let foreground = if selected {
        palette.accent
    } else {
        palette.text
    };

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
    let palette = palette();
    let fill = if selected {
        palette.accent_soft
    } else if response.is_pointer_button_down_on() {
        ui.visuals().widgets.active.bg_fill
    } else if response.hovered() {
        ui.visuals().widgets.hovered.bg_fill
    } else {
        Color32::TRANSPARENT
    };
    let foreground = if selected {
        palette.accent
    } else {
        palette.text
    };

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
