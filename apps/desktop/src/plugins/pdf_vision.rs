use std::io::Cursor;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;
use image::codecs::jpeg::JpegEncoder;
use image::imageops::FilterType;
use image::{DynamicImage, ImageBuffer, RgbaImage};
use rebook_publication::{Block, BookSource};
use reqwest::Client;
use serde::Deserialize;
use serde_json::{Value, json};

use super::AiProvider;
use super::ai::{message_content, request_completion};

pub(super) const PAGE_IMAGE_MAX_DIMENSION: u32 = 1_600;
const VISION_RESPONSE_RETRIES: usize = 1;

pub(super) async fn request_vision_json(
    client: &Client,
    provider: &AiProvider,
    model: &str,
    content: Vec<Value>,
) -> Result<Value, String> {
    let messages = vec![json!({ "role": "user", "content": content })];
    let mut extra_body = json!({
        "response_format": { "type": "json_object" }
    });
    if model.to_ascii_lowercase().contains("qwen") {
        extra_body["enable_thinking"] = Value::Bool(false);
    }
    for attempt in 0..=VISION_RESPONSE_RETRIES {
        let result = request_completion(
            client,
            provider,
            model,
            &messages,
            None,
            None,
            Some(&extra_body),
        )
        .await
        .and_then(|message| {
            message_content(&message)
                .filter(|content| !content.trim().is_empty())
                .map(Value::String)
                .ok_or_else(|| {
                    if message
                        .get("reasoning_content")
                        .and_then(Value::as_str)
                        .is_some_and(|content| !content.trim().is_empty())
                    {
                        "AI 视觉识别只返回了思考过程，没有返回 JSON 正文".into()
                    } else {
                        "AI 视觉识别响应缺少消息正文".into()
                    }
                })
        });
        match result {
            Ok(value) => return Ok(value),
            Err(error)
                if attempt < VISION_RESPONSE_RETRIES
                    && is_retryable_vision_response_error(&error) =>
            {
                tokio::time::sleep(std::time::Duration::from_millis(800)).await;
            }
            Err(error) => return Err(error),
        }
    }
    unreachable!("vision response retry loop always returns")
}

pub(super) fn is_retryable_vision_response_error(error: &str) -> bool {
    [
        "AI 响应缺少 choices[0].message",
        "AI 视觉识别响应缺少消息正文",
        "AI 视觉识别只返回了思考过程",
        "AI 请求失败",
        "429",
        "502",
        "503",
        "504",
    ]
    .iter()
    .any(|fragment| error.contains(fragment))
}

pub(super) fn parse_json_value<T>(value: &Value) -> Result<T, String>
where
    T: for<'de> Deserialize<'de>,
{
    let text = if let Some(text) = value.as_str() {
        text.to_owned()
    } else if let Some(parts) = value.as_array() {
        parts
            .iter()
            .filter_map(|part| part.get("text").and_then(Value::as_str))
            .collect::<Vec<_>>()
            .join("")
    } else {
        return Err("AI 视觉识别响应内容为空".into());
    };
    let trimmed = text.trim();
    let candidate = trimmed
        .strip_prefix("```json")
        .or_else(|| trimmed.strip_prefix("```"))
        .and_then(|value| value.strip_suffix("```"))
        .map(str::trim)
        .or_else(|| {
            let start = trimmed.find('{')?;
            let end = trimmed.rfind('}')?;
            (start <= end).then(|| &trimmed[start..=end])
        })
        .unwrap_or(trimmed);
    serde_json::from_str(candidate).map_err(|error| format!("AI 视觉识别结果协议无效：{error}"))
}

pub(super) fn render_page_data_url(
    source: &dyn BookSource,
    page_index: usize,
    max_dimension: u32,
) -> Result<String, String> {
    render_page_data_url_with_quality(source, page_index, max_dimension, 82)
}

pub(super) fn render_page_data_url_with_quality(
    source: &dyn BookSource,
    page_index: usize,
    max_dimension: u32,
    quality: u8,
) -> Result<String, String> {
    let image = render_page_image(source, page_index, max_dimension)?;
    encode_jpeg_data_url_with_quality(&image, page_index, quality)
}

pub(super) fn render_page_image(
    source: &dyn BookSource,
    page_index: usize,
    max_dimension: u32,
) -> Result<DynamicImage, String> {
    let section = source
        .parse_section(page_index)
        .map_err(|error| format!("读取 PDF 第 {} 页失败：{error}", page_index + 1))?;
    let href = section
        .blocks
        .iter()
        .find_map(|block| match block {
            Block::Image(image) => Some(&image.href),
            _ => None,
        })
        .ok_or_else(|| format!("PDF 第 {} 页没有可识别图像", page_index + 1))?;
    let image = if let Some(raster) = source
        .raster_resource(href)
        .map_err(|error| format!("渲染 PDF 第 {} 页失败：{error}", page_index + 1))?
    {
        let rgba: RgbaImage =
            ImageBuffer::from_raw(raster.width, raster.height, raster.pixels.to_vec())
                .ok_or_else(|| format!("PDF 第 {} 页的像素数据无效", page_index + 1))?;
        DynamicImage::ImageRgba8(rgba)
    } else {
        let resource = source
            .resource(href)
            .map_err(|error| format!("读取 PDF 第 {} 页图像失败：{error}", page_index + 1))?;
        image::load_from_memory(&resource.bytes)
            .map_err(|error| format!("解码 PDF 第 {} 页图像失败：{error}", page_index + 1))?
    };
    Ok(image.resize(max_dimension, max_dimension, FilterType::Triangle))
}

pub(super) fn encode_jpeg_data_url(
    image: &DynamicImage,
    page_index: usize,
) -> Result<String, String> {
    encode_jpeg_data_url_with_quality(image, page_index, 82)
}

fn encode_jpeg_data_url_with_quality(
    image: &DynamicImage,
    page_index: usize,
    quality: u8,
) -> Result<String, String> {
    let mut bytes = Cursor::new(Vec::new());
    JpegEncoder::new_with_quality(&mut bytes, quality)
        .encode_image(image)
        .map_err(|error| format!("压缩 PDF 第 {} 页图像失败：{error}", page_index + 1))?;
    Ok(format!(
        "data:image/jpeg;base64,{}",
        BASE64.encode(bytes.into_inner())
    ))
}
