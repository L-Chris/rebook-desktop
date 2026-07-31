use winit::window::{Theme, Window};

use crate::preferences::AppTheme;

pub(super) fn apply(window: &Window, theme: AppTheme) {
    #[cfg(target_os = "windows")]
    apply_windows_backdrop(window, theme);

    window.set_theme(Some(native_theme(theme)));
}

const fn native_theme(theme: AppTheme) -> Theme {
    match theme {
        AppTheme::Dark => Theme::Dark,
        AppTheme::Light | AppTheme::Glass => Theme::Light,
    }
}

#[cfg(target_os = "windows")]
fn apply_windows_backdrop(window: &Window, theme: AppTheme) {
    match theme {
        AppTheme::Glass => {
            // Mica is the Windows 11 backdrop intended for persistent app surfaces.
            // Unsupported Windows versions simply keep the ordinary title bar.
            let _ = window_vibrancy::apply_mica(window, Some(false));
        }
        AppTheme::Light | AppTheme::Dark => {
            let _ = window_vibrancy::clear_mica(window);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_application_themes_to_native_caption_themes() {
        assert_eq!(native_theme(AppTheme::Light), Theme::Light);
        assert_eq!(native_theme(AppTheme::Dark), Theme::Dark);
        assert_eq!(native_theme(AppTheme::Glass), Theme::Light);
    }
}
