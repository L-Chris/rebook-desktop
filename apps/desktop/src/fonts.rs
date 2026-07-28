use std::sync::Arc;

use peniko::Blob;

const BITTER_REGULAR: &[u8] = include_bytes!("../../../assets/fonts/Bitter-wght.ttf");
const BITTER_ITALIC: &[u8] = include_bytes!("../../../assets/fonts/Bitter-Italic-wght.ttf");
const ROBOTO_REGULAR: &[u8] = include_bytes!("../../../assets/fonts/Roboto-wdth-wght.ttf");
const ROBOTO_ITALIC: &[u8] = include_bytes!("../../../assets/fonts/Roboto-Italic-wdth-wght.ttf");
const FIRA_CODE: &[u8] = include_bytes!("../../../assets/fonts/FiraCode-wght.ttf");
const LXGW_WENKAI: &[u8] = include_bytes!("../../../assets/fonts/LXGWWenKai-Regular.ttf");

pub fn embedded_reader_fonts() -> Arc<[Blob<u8>]> {
    [
        font_blob(BITTER_REGULAR),
        font_blob(BITTER_ITALIC),
        font_blob(ROBOTO_REGULAR),
        font_blob(ROBOTO_ITALIC),
        font_blob(FIRA_CODE),
        font_blob(LXGW_WENKAI),
    ]
    .into()
}

fn font_blob(bytes: &'static [u8]) -> Blob<u8> {
    Blob::new(Arc::new(bytes))
}

#[cfg(test)]
mod tests {
    use super::*;
    use rebook_layout::LayoutEngine;

    #[test]
    fn embedded_font_assets_are_non_empty() {
        let fonts = embedded_reader_fonts();
        assert_eq!(fonts.len(), 6);
        assert!(fonts.iter().all(|font| !font.is_empty()));
    }

    #[test]
    fn embedded_fonts_register_with_reader_family_names() {
        let fonts = embedded_reader_fonts();
        let mut engine = LayoutEngine::with_fonts(fonts.iter().cloned());
        let families = engine.available_font_families();

        for expected in ["Bitter", "Roboto", "Fira Code", "LXGW WenKai"] {
            assert!(
                families.iter().any(|family| family == expected),
                "missing embedded font family {expected:?}; registered: {families:?}"
            );
        }
    }
}
