use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Arc;
use std::time::Duration;

use image::imageops::FilterType;
use image::{DynamicImage, Rgb, RgbImage};
use rebook_publication::BookSource;
use reqwest::Client;
use serde::Deserialize;
use serde_json::json;
use tokio::task::JoinSet;

#[cfg(test)]
use super::pdf_vision::is_retryable_vision_response_error;
use super::pdf_vision::{
    PAGE_IMAGE_MAX_DIMENSION, encode_jpeg_data_url, parse_json_value, render_page_data_url,
    render_page_image, request_vision_json,
};
use super::{AiProvider, PluginSettings};
use crate::generated_toc::{GeneratedTocDraft, GeneratedTocEntry};

const SCAN_BATCH_SIZE: usize = 8;
const SCAN_PAGE_LIMIT: usize = 20;
const SCAN_IMAGE_MAX_DIMENSION: u32 = 560;
const EXTRACTION_BATCH_SIZE: usize = 1;
const VISION_REQUEST_CONCURRENCY: usize = 4;

#[derive(Debug, Deserialize)]
struct ScanResponse {
    #[serde(default)]
    p: Vec<ScannedPage>,
}

#[derive(Debug, Deserialize)]
struct ScannedPage {
    i: usize,
    #[serde(default)]
    k: String,
    #[serde(default)]
    n: String,
}

#[derive(Debug, Deserialize)]
struct ExtractionResponse {
    #[serde(default)]
    e: Vec<ExtractedEntry>,
}

#[derive(Debug, Deserialize)]
struct ExtractedEntry {
    #[serde(default)]
    d: usize,
    #[serde(default)]
    t: String,
    #[serde(default)]
    n: String,
    #[serde(default)]
    c: Option<f32>,
}

#[derive(Clone)]
struct PageNumberAnchor {
    physical_page: usize,
    printed_page: String,
}

pub(crate) async fn generate_pdf_toc<F>(
    source: Arc<dyn BookSource>,
    settings: PluginSettings,
    mut on_progress: F,
) -> Result<GeneratedTocDraft, String>
where
    F: FnMut(String) + Send,
{
    let (provider, model) = settings.ocr_endpoint()?;
    let provider = provider.clone();
    let model = model.to_owned();
    let client = Client::builder()
        .timeout(Duration::from_secs(150))
        .build()
        .map_err(|error| format!("无法创建 AI 请求客户端：{error}"))?;
    let page_count = source.book().sections.len();
    if page_count == 0 {
        return Err("PDF 没有可识别的页面".into());
    }

    let (toc_pages, anchors) = locate_toc_pages(
        &client,
        &provider,
        &model,
        Arc::clone(&source),
        page_count,
        &mut on_progress,
    )
    .await?;
    if toc_pages.is_empty() {
        return Err("未在 PDF 前部识别到印刷目录页".into());
    }

    on_progress(format!("正在提取 {} 页目录…", toc_pages.len()));
    let extracted = extract_entries(
        &client,
        &provider,
        &model,
        source,
        &toc_pages,
        &mut on_progress,
    )
    .await?;
    let last_toc_page = toc_pages.iter().copied().max().unwrap_or(0);
    let (offset, offset_support) = infer_page_offset(&anchors, last_toc_page)
        .ok_or_else(|| "已识别目录，但无法建立印刷页码与 PDF 页码的映射".to_owned())?;
    let confidence_factor = if offset_support >= 3 { 1.0 } else { 0.88 };
    let mut seen = HashSet::new();
    let mut entries = Vec::new();
    for entry in extracted {
        let Some(printed_page) = parse_arabic_page_number(&entry.n) else {
            continue;
        };
        let physical_page = isize::try_from(printed_page)
            .ok()
            .and_then(|page| page.checked_add(offset))
            .and_then(|page| usize::try_from(page).ok())
            .filter(|page| (1..=page_count).contains(page));
        let Some(physical_page) = physical_page else {
            continue;
        };
        let title = entry.t.trim().to_owned();
        if title.is_empty() || !seen.insert((title.clone(), physical_page)) {
            continue;
        }
        entries.push(GeneratedTocEntry {
            depth: entry.d,
            title,
            printed_page: entry.n.trim().to_owned(),
            physical_page,
            confidence: entry.c.unwrap_or(0.9).clamp(0.0, 1.0) * confidence_factor,
        });
    }
    if entries.len() < 2 {
        return Err("目录条目过少，无法生成可靠的导航目录".into());
    }
    on_progress(format!("已生成 {} 个目录条目", entries.len()));
    Ok(GeneratedTocDraft {
        provider_name: provider.name,
        model,
        source_pages: toc_pages,
        entries,
    })
}

async fn locate_toc_pages<F>(
    client: &Client,
    provider: &AiProvider,
    model: &str,
    source: Arc<dyn BookSource>,
    page_count: usize,
    on_progress: &mut F,
) -> Result<(Vec<usize>, Vec<PageNumberAnchor>), String>
where
    F: FnMut(String),
{
    let scan_limit = page_count.min(SCAN_PAGE_LIMIT);
    let mut jobs = VecDeque::new();
    for batch_start in (0..scan_limit).step_by(SCAN_BATCH_SIZE) {
        let batch_end = (batch_start + SCAN_BATCH_SIZE).min(scan_limit);
        let page_indices = (batch_start..batch_end).collect::<Vec<_>>();
        let page_mapping = page_indices
            .iter()
            .enumerate()
            .map(|(slot, page)| format!("{slot}={}", page + 1))
            .collect::<Vec<_>>()
            .join(",");
        let content = vec![
            json!({
                "type": "text",
                "text": format!("The image is a 2-column contact sheet in row-major slot order. Slot-to-PDF-page mapping: {page_mapping}. Inspect every slot. A toc page is a printed table-of-contents page listing multiple headings with page numbers; covers, copyright pages, prefaces and chapter opening pages are other. Return compact JSON only: {{\"p\":[{{\"i\":0,\"k\":\"toc|other\",\"n\":\"visible printed page number or empty\"}}]}}. Include exactly one item for every slot. Do not infer n when it is not visibly printed."),
            }),
            json!({
                "type": "image_url",
                "image_url": {
                    "url": render_contact_sheet(source.as_ref(), &page_indices, SCAN_IMAGE_MAX_DIMENSION)?
                }
            }),
        ];
        jobs.push_back((batch_start, batch_end, content));
    }

    let total_batches = jobs.len();
    let mut tasks = JoinSet::new();
    while tasks.len() < VISION_REQUEST_CONCURRENCY
        && let Some((batch_start, batch_end, content)) = jobs.pop_front()
    {
        let client = client.clone();
        let provider = provider.clone();
        let model = model.to_owned();
        tasks.spawn(async move {
            let value = request_vision_json(&client, &provider, &model, content).await?;
            let response: ScanResponse = parse_json_value(&value)?;
            Ok::<_, String>((batch_start, batch_end, response))
        });
    }

    let mut toc_pages = Vec::new();
    let mut anchors = Vec::new();
    let mut completed_batches = 0;
    while let Some(result) = tasks.join_next().await {
        let (batch_start, batch_end, response) =
            result.map_err(|error| format!("目录页识别任务异常结束：{error}"))??;
        completed_batches += 1;
        on_progress(format!(
            "正在查找目录页：{completed_batches}/{total_batches} 批"
        ));
        for page in response.p {
            if page.i >= batch_end - batch_start {
                continue;
            }
            let physical_page = batch_start + page.i + 1;
            if page.k.eq_ignore_ascii_case("toc") {
                toc_pages.push(physical_page);
            }
            if !page.n.trim().is_empty() {
                anchors.push(PageNumberAnchor {
                    physical_page,
                    printed_page: page.n,
                });
            }
        }

        if let Some((batch_start, batch_end, content)) = jobs.pop_front() {
            let client = client.clone();
            let provider = provider.clone();
            let model = model.to_owned();
            tasks.spawn(async move {
                let value = request_vision_json(&client, &provider, &model, content).await?;
                let response: ScanResponse = parse_json_value(&value)?;
                Ok::<_, String>((batch_start, batch_end, response))
            });
        }
    }
    toc_pages.sort_unstable();
    toc_pages.dedup();
    Ok((toc_pages, anchors))
}

async fn extract_entries<F>(
    client: &Client,
    provider: &AiProvider,
    model: &str,
    source: Arc<dyn BookSource>,
    toc_pages: &[usize],
    on_progress: &mut F,
) -> Result<Vec<ExtractedEntry>, String>
where
    F: FnMut(String),
{
    let mut jobs = VecDeque::new();
    for (batch_index, pages) in toc_pages.chunks(EXTRACTION_BATCH_SIZE).enumerate() {
        let mut content = vec![json!({
            "type": "text",
            "text": "Extract every navigable entry from this printed table-of-contents page in visual reading order. Preserve the original title text. d is zero-based hierarchy depth, t is title without leaders or page number, n is the printed target page label, c is confidence 0..1. Ignore running headers and the page's own footer. Return compact JSON only: {\"e\":[{\"d\":0,\"t\":\"title\",\"n\":\"12\",\"c\":0.98}]} ."
        })];
        for physical_page in pages {
            content.push(json!({
                "type": "image_url",
                "image_url": {
                    "url": render_page_data_url(source.as_ref(), physical_page - 1, PAGE_IMAGE_MAX_DIMENSION)?
                }
            }));
        }
        jobs.push_back((batch_index, content));
    }

    let total_batches = jobs.len();
    let mut tasks = JoinSet::new();
    while tasks.len() < VISION_REQUEST_CONCURRENCY
        && let Some((batch_index, content)) = jobs.pop_front()
    {
        let client = client.clone();
        let provider = provider.clone();
        let model = model.to_owned();
        tasks.spawn(async move {
            let value = request_vision_json(&client, &provider, &model, content).await?;
            let response: ExtractionResponse = parse_json_value(&value)?;
            Ok::<_, String>((batch_index, response.e))
        });
    }

    let mut completed_batches = 0;
    let mut batches = Vec::with_capacity(total_batches);
    while let Some(result) = tasks.join_next().await {
        let batch = result.map_err(|error| format!("目录文字提取任务异常结束：{error}"))??;
        completed_batches += 1;
        on_progress(format!(
            "正在读取目录文字：{completed_batches}/{total_batches} 页"
        ));
        batches.push(batch);

        if let Some((batch_index, content)) = jobs.pop_front() {
            let client = client.clone();
            let provider = provider.clone();
            let model = model.to_owned();
            tasks.spawn(async move {
                let value = request_vision_json(&client, &provider, &model, content).await?;
                let response: ExtractionResponse = parse_json_value(&value)?;
                Ok::<_, String>((batch_index, response.e))
            });
        }
    }

    batches.sort_unstable_by_key(|(batch_index, _)| *batch_index);
    Ok(batches
        .into_iter()
        .flat_map(|(_, entries)| entries)
        .collect())
}

fn render_contact_sheet(
    source: &dyn BookSource,
    page_indices: &[usize],
    max_dimension: u32,
) -> Result<String, String> {
    let images = page_indices
        .iter()
        .map(|page_index| render_page_image(source, *page_index, max_dimension))
        .collect::<Result<Vec<_>, _>>()?;
    let columns = 2_u32;
    let rows = u32::try_from(images.len())
        .unwrap_or(u32::MAX)
        .div_ceil(columns);
    let cell_width = images.iter().map(DynamicImage::width).max().unwrap_or(1) + 8;
    let cell_height = images.iter().map(DynamicImage::height).max().unwrap_or(1) + 8;
    let mut sheet = RgbImage::from_pixel(
        cell_width * columns,
        cell_height * rows,
        Rgb([238, 238, 238]),
    );
    for (slot, image) in images.iter().enumerate() {
        let slot = u32::try_from(slot).unwrap_or(0);
        let x = (slot % columns) * cell_width + (cell_width - image.width()) / 2;
        let y = (slot / columns) * cell_height + (cell_height - image.height()) / 2;
        image::imageops::overlay(&mut sheet, &image.to_rgb8(), i64::from(x), i64::from(y));
    }
    let sheet = DynamicImage::ImageRgb8(sheet).resize(2_000, 2_000, FilterType::Triangle);
    encode_jpeg_data_url(&sheet, page_indices[0])
}

fn infer_page_offset(anchors: &[PageNumberAnchor], last_toc_page: usize) -> Option<(isize, usize)> {
    let mut counts = HashMap::<isize, usize>::new();
    for anchor in anchors {
        if anchor.physical_page <= last_toc_page {
            continue;
        }
        let Some(printed) = parse_arabic_page_number(&anchor.printed_page) else {
            continue;
        };
        let (Ok(physical), Ok(printed)) = (
            isize::try_from(anchor.physical_page),
            isize::try_from(printed),
        ) else {
            continue;
        };
        if printed <= 0 || printed > physical {
            continue;
        }
        *counts.entry(physical - printed).or_default() += 1;
    }
    counts
        .into_iter()
        .max_by_key(|(offset, count)| (*count, std::cmp::Reverse(offset.abs())))
}

fn parse_arabic_page_number(label: &str) -> Option<usize> {
    let digits = label
        .chars()
        .filter(char::is_ascii_digit)
        .collect::<String>();
    (!digits.is_empty()).then(|| digits.parse().ok()).flatten()
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use reqwest::Client;
    use serde_json::json;

    use super::{
        PageNumberAnchor, generate_pdf_toc, infer_page_offset, is_retryable_vision_response_error,
        parse_arabic_page_number, render_page_data_url, request_vision_json,
    };

    #[test]
    fn page_offset_uses_the_most_supported_mapping() {
        let anchors = vec![
            PageNumberAnchor {
                physical_page: 14,
                printed_page: "1".into(),
            },
            PageNumberAnchor {
                physical_page: 15,
                printed_page: "2".into(),
            },
            PageNumberAnchor {
                physical_page: 17,
                printed_page: "4".into(),
            },
            PageNumberAnchor {
                physical_page: 18,
                printed_page: "9".into(),
            },
        ];
        assert_eq!(infer_page_offset(&anchors, 10), Some((13, 3)));
        assert_eq!(parse_arabic_page_number("第 128 页"), Some(128));
    }

    #[test]
    fn retries_only_transient_vision_gateway_failures() {
        assert!(is_retryable_vision_response_error(
            "AI 响应缺少 choices[0].message"
        ));
        assert!(is_retryable_vision_response_error(
            "AI 服务返回 503 Service Unavailable"
        ));
        assert!(!is_retryable_vision_response_error(
            "AI 目录识别结果协议无效"
        ));
    }

    #[test]
    #[ignore = "uses the configured AI provider and a local PDF"]
    fn live_single_page_vision_probe() {
        let path = std::env::var_os("REBOOK_PDF_TOC_TEST_FILE")
            .expect("set REBOOK_PDF_TOC_TEST_FILE to a scanned PDF");
        let page_index = std::env::var("REBOOK_PDF_TOC_TEST_PAGE")
            .ok()
            .and_then(|value| value.parse::<usize>().ok())
            .unwrap_or(0);
        let opened = rebook_formats::open_file(std::path::PathBuf::from(path))
            .expect("test PDF should open");
        let source = opened.source();
        let settings = super::PluginSettings::load_default().expect("AI settings should load");
        let (provider, model) = settings
            .ocr_endpoint()
            .expect("OCR endpoint should be valid");
        let client = Client::builder()
            .timeout(Duration::from_secs(45))
            .build()
            .expect("HTTP client should build");
        let image_url =
            render_page_data_url(source.as_ref(), page_index, 1_200).expect("page should render");
        let content = vec![
            json!({
                "type": "text",
                "text": "Describe this scanned book page in one short sentence. Return JSON: {\"description\":\"...\"}."
            }),
            json!({
                "type": "image_url",
                "image_url": { "url": image_url }
            }),
        ];
        eprintln!("probing model {model} with PDF page {}", page_index + 1);
        let runtime = tokio::runtime::Runtime::new().expect("Tokio runtime should start");
        let response = runtime
            .block_on(request_vision_json(&client, provider, model, content))
            .expect("vision request should succeed");
        eprintln!("vision response: {response}");
    }

    #[test]
    #[ignore = "uses the configured AI provider and a local PDF"]
    fn live_scanned_pdf_toc_generation() {
        let path = std::env::var_os("REBOOK_PDF_TOC_TEST_FILE")
            .expect("set REBOOK_PDF_TOC_TEST_FILE to a scanned PDF");
        let opened = rebook_formats::open_file(std::path::PathBuf::from(path))
            .expect("test PDF should open");
        let source = opened.source();
        let page_count = source.book().sections.len();
        let text_layer_pages = (0..page_count.min(24))
            .filter(|index| {
                source.parse_section(*index).ok().is_some_and(|section| {
                    section.blocks.iter().any(|block| {
                        matches!(block, rebook_publication::Block::Image(image) if image.text_layer.as_ref().is_some_and(|layer| !layer.text.trim().is_empty()))
                    })
                })
            })
            .count();
        eprintln!("non-empty text layers in first 24 pages: {text_layer_pages}");
        let settings = super::PluginSettings::load_default().expect("AI settings should load");
        let runtime = tokio::runtime::Runtime::new().expect("Tokio runtime should start");
        let draft = runtime
            .block_on(generate_pdf_toc(source, settings, |message| {
                eprintln!("{message}");
            }))
            .expect("TOC generation should succeed");
        for entry in &draft.entries {
            eprintln!(
                "{}{} -> printed {}, PDF {} ({:.0}%)",
                "  ".repeat(entry.depth),
                entry.title,
                entry.printed_page,
                entry.physical_page,
                entry.confidence * 100.0
            );
        }
        assert!(draft.entries.len() >= 2);
        assert!(
            draft
                .entries
                .iter()
                .all(|entry| entry.physical_page <= page_count)
        );
    }
}
