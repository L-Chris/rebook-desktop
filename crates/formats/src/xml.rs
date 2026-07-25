use encoding_rs::{Encoding, UTF_8};

use crate::{BookFormat, FormatError, conversion_error};

const MAX_XML_BYTES: usize = 64 * 1024 * 1024;

pub(crate) fn decode_xml(bytes: &[u8], format: BookFormat) -> Result<String, FormatError> {
    if bytes.len() > MAX_XML_BYTES {
        return Err(conversion_error(
            format,
            format_args!("XML 超过 {} MiB 限制", MAX_XML_BYTES / 1024 / 1024),
        ));
    }
    let encoding = declared_encoding(bytes)
        .and_then(Encoding::for_label)
        .unwrap_or(UTF_8);
    let (decoded, _, had_errors) = encoding.decode(bytes);
    if had_errors {
        return Err(conversion_error(format, "XML 字符编码无效"));
    }
    if contains_ascii_case_insensitive(decoded.as_bytes(), b"<!doctype") {
        return Err(conversion_error(format, "DOCTYPE 已禁用"));
    }
    Ok(decoded.trim_start_matches('\u{feff}').to_owned())
}

fn declared_encoding(bytes: &[u8]) -> Option<&[u8]> {
    let prefix = bytes.get(..bytes.len().min(256))?;
    let lower = prefix
        .iter()
        .map(u8::to_ascii_lowercase)
        .collect::<Vec<_>>();
    let start = lower.windows(8).position(|window| window == b"encoding")? + 8;
    let equals = lower[start..].iter().position(|byte| *byte == b'=')? + start + 1;
    let quote_index = lower[equals..]
        .iter()
        .position(|byte| *byte == b'\'' || *byte == b'\"')?
        + equals;
    let quote = lower[quote_index];
    let value_start = quote_index + 1;
    let value_end = lower[value_start..]
        .iter()
        .position(|byte| *byte == quote)?
        + value_start;
    prefix.get(value_start..value_end)
}

fn contains_ascii_case_insensitive(haystack: &[u8], needle: &[u8]) -> bool {
    haystack.windows(needle.len()).any(|window| {
        window
            .iter()
            .zip(needle)
            .all(|(left, right)| left.eq_ignore_ascii_case(right))
    })
}
