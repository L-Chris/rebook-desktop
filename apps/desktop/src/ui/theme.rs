//! Shared sizing tokens for the desktop UI.

use rebook_publication::Rgba;
use xilem::Color;

pub(crate) const RADIUS_SMALL: f64 = 6.0;
pub(crate) const RADIUS_MEDIUM: f64 = 8.0;
pub(crate) const RADIUS_LARGE: f64 = 12.0;
pub(crate) const RADIUS_DIALOG: f64 = 14.0;

pub(crate) const CONTROL_HEIGHT_COMPACT: f64 = 28.0;
pub(crate) const CONTROL_HEIGHT: f64 = 32.0;
pub(crate) const SETTINGS_ROW_HEIGHT: f64 = 44.0;
pub(crate) const DIALOG_HEADER_HEIGHT: f64 = 48.0;
pub(crate) const DIALOG_FOOTER_HEIGHT: f64 = 48.0;

pub(crate) const CONTENT_GAP: f64 = 8.0;
pub(crate) const CONTENT_PADDING_HORIZONTAL: f64 = 18.0;
pub(crate) const CONTENT_PADDING_VERTICAL: f64 = 12.0;

// Keep these in sync with rebook-web's light reader tokens.
pub(crate) const UI_BACKGROUND: Color = Color::from_rgb8(0xff, 0xff, 0xff);
pub(crate) const UI_SURFACE: Color = Color::from_rgb8(0xff, 0xff, 0xff);
pub(crate) const UI_SIDEBAR: Color = Color::from_rgb8(0xfb, 0xfc, 0xfd);
pub(crate) const UI_SURFACE_MUTED: Color = Color::from_rgb8(0xf2, 0xf5, 0xf8);
pub(crate) const UI_TEXT: Color = Color::from_rgb8(0x1f, 0x2d, 0x3d);
pub(crate) const UI_TEXT_SOFT: Color = Color::from_rgb8(0x43, 0x55, 0x6b);
pub(crate) const UI_MUTED: Color = Color::from_rgb8(0x70, 0x82, 0x98);
pub(crate) const UI_BORDER: Color = Color::from_rgb8(0xdd, 0xe5, 0xee);
pub(crate) const UI_ACCENT: Color = Color::from_rgb8(0x0f, 0x76, 0x6e);
pub(crate) const UI_ACCENT_SOFT: Color = Color::from_rgb8(0xe2, 0xf3, 0xf1);
pub(crate) const UI_ACCENT_BORDER: Color = Color::from_rgb8(0xba, 0xe6, 0xe1);
pub(crate) const UI_FONT_STACK: &str = "'Microsoft YaHei UI', 'Microsoft YaHei', 'PingFang SC', 'Noto Sans CJK SC', 'Segoe UI Symbol', sans-serif";

pub(crate) fn ui_color(color: Rgba) -> Color {
    Color::from_rgba8(color.red, color.green, color.blue, color.alpha)
}
