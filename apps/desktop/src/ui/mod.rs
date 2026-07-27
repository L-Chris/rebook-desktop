mod button;
mod dialog;
mod feedback;
mod help_tooltip;
mod theme;

pub(crate) use button::button;
pub(crate) use dialog::confirmation_dialog;
pub(crate) use feedback::{NOTICE_WIDTH, NoticeTone, dismissible_notice, notice_card};
pub(crate) use help_tooltip::help_tooltip;
pub(crate) use theme::{
    CONTENT_GAP, CONTENT_PADDING_HORIZONTAL, CONTENT_PADDING_VERTICAL, CONTROL_HEIGHT,
    CONTROL_HEIGHT_COMPACT, DIALOG_FOOTER_HEIGHT, DIALOG_HEADER_HEIGHT, RADIUS_DIALOG,
    RADIUS_LARGE, RADIUS_MEDIUM, RADIUS_SMALL, SETTINGS_ROW_HEIGHT, UI_ACCENT, UI_ACCENT_BORDER,
    UI_ACCENT_SOFT, UI_BACKGROUND, UI_BORDER, UI_FONT_STACK, UI_MUTED, UI_SIDEBAR, UI_SURFACE,
    UI_SURFACE_MUTED, UI_TEXT, UI_TEXT_SOFT, ui_color,
};

use lucide_icons::Icon;
use xilem::masonry::peniko::{Blob, ImageAlphaType, ImageData, ImageFormat};
use xilem::masonry::properties::types::AsUnit;
use xilem::style::Style;
use xilem::view::{label, sized_box};
use xilem::{Color, WidgetView};

pub(crate) fn decode_image(bytes: &[u8]) -> Result<ImageData, ::image::ImageError> {
    let pixels = ::image::load_from_memory(bytes)?.into_rgba8();
    let width = pixels.width();
    let height = pixels.height();
    Ok(ImageData {
        data: Blob::new(std::sync::Arc::new(pixels.into_vec())),
        format: ImageFormat::Rgba8,
        alpha_type: ImageAlphaType::Alpha,
        width,
        height,
    })
}

pub(crate) fn icon_label<State: 'static>(
    icon: Icon,
    size: f32,
    color: Color,
) -> impl WidgetView<State> {
    label(char::from(icon).to_string())
        .font("lucide")
        .text_size(size)
        .color(color)
}

pub(crate) fn divider<State: 'static>() -> impl WidgetView<State> {
    sized_box(label(""))
        .height(1.px())
        .expand_width()
        .background_color(UI_BORDER)
}

pub(crate) fn ellipsize_display_text(text: &str, max_units: usize) -> String {
    let display_units = text.chars().map(display_character_units).sum::<usize>();
    if display_units <= max_units {
        return text.to_owned();
    }

    let mut used_units = 0;
    let mut end = 0;
    for (index, character) in text.char_indices() {
        let character_units = display_character_units(character);
        if used_units + character_units > max_units.saturating_sub(2) {
            break;
        }
        used_units += character_units;
        end = index + character.len_utf8();
    }
    format!("{}…", &text[..end])
}

pub(crate) fn wrap_display_text(text: &str, line_units: usize, max_lines: usize) -> String {
    if line_units == 0 || max_lines == 0 {
        return String::new();
    }

    let normalized = text.split_whitespace().collect::<Vec<_>>().join(" ");
    let text = ellipsize_display_text(&normalized, line_units.saturating_mul(max_lines));
    let mut lines = Vec::with_capacity(max_lines);
    let mut current = String::new();
    let mut used_units = 0;

    for character in text.chars() {
        let character_units = display_character_units(character);
        if !current.is_empty() && used_units + character_units > line_units {
            lines.push(current.trim_end().to_owned());
            if lines.len() == max_lines {
                break;
            }
            current.clear();
            used_units = 0;
            if character.is_whitespace() {
                continue;
            }
        }
        used_units += character_units;
        current.push(character);
    }

    if lines.len() < max_lines && !current.is_empty() {
        lines.push(current.trim_end().to_owned());
    }
    lines.join("\n")
}

pub(crate) fn display_character_units(character: char) -> usize {
    if character.is_ascii() { 1 } else { 2 }
}
