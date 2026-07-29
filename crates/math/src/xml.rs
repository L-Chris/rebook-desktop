/// XML 1.0 valid char ranges:
/// - 0x09, 0x0A, 0x0D
/// - 0x20..=0xD7FF
/// - 0xE000..=0xFFFD
/// - 0x10000..=0x10FFFF
fn is_valid_xml_char(c: char) -> bool {
    matches!(
        c as u32,
        0x09 | 0x0A | 0x0D | 0x20..=0xD7FF | 0xE000..=0xFFFD | 0x0001_0000..=0x0010_FFFF
    )
}

pub fn sanitize_xml_text(text: &str) -> String {
    text.chars().filter(|&c| is_valid_xml_char(c)).collect()
}

pub fn escape_xml(text: &str) -> String {
    let mut escaped = String::with_capacity(text.len());
    for c in text.chars() {
        if !is_valid_xml_char(c) {
            continue;
        }
        match c {
            '&' => escaped.push_str("&amp;"),
            '<' => escaped.push_str("&lt;"),
            '>' => escaped.push_str("&gt;"),
            '"' => escaped.push_str("&quot;"),
            '\'' => escaped.push_str("&apos;"),
            _ => escaped.push(c),
        }
    }
    escaped
}

#[cfg(test)]
mod tests {
    use super::{escape_xml, sanitize_xml_text};

    #[test]
    fn remove_invalid_control_chars() {
        let s = "A\u{0007}B\u{000C}C";
        assert_eq!(sanitize_xml_text(s), "ABC");
        assert_eq!(escape_xml(s), "ABC");
    }

    #[test]
    fn keep_valid_whitespace_controls() {
        let s = "a\tb\nc\rd";
        assert_eq!(sanitize_xml_text(s), s);
        assert_eq!(escape_xml(s), s);
    }

    #[test]
    fn escape_special_xml_chars() {
        let s = r#"<tag attr="x&y">'z'"#;
        assert_eq!(
            escape_xml(s),
            "&lt;tag attr=&quot;x&amp;y&quot;&gt;&apos;z&apos;"
        );
    }
}
