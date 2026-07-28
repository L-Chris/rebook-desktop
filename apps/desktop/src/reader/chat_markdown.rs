use std::collections::HashMap;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, Sender};
use std::time::Duration;

use egui::{Color32, FontFamily, FontId, ImageSource, RichText, Sense, TextStyle, Vec2};
use egui_commonmark::{CommonMarkCache, CommonMarkViewer};
use pulldown_cmark::{CodeBlockKind, Event, Options, Parser, Tag, TagEnd};

use crate::preferences::AppLanguage;
use crate::ui::{ACCENT, ACCENT_SOFT, BORDER, MUTED, SURFACE, SURFACE_MUTED, TEXT};

const MARKDOWN_FONT_SIZE: f32 = 12.5;
const PREVIEW_GAP: f32 = 8.0;
const ASSET_REPAINT_INTERVAL: Duration = Duration::from_millis(50);

pub(super) struct ChatMarkdownState {
    markdown_cache: CommonMarkCache,
    assets: HashMap<AssetKey, AssetStatus>,
    asset_sender: Sender<AssetRequest>,
    asset_receiver: Receiver<AssetResult>,
    preview_states: HashMap<u64, PreviewState>,
}

impl Default for ChatMarkdownState {
    fn default() -> Self {
        let (asset_sender, request_receiver) = mpsc::channel();
        let (result_sender, asset_receiver) = mpsc::channel();
        std::thread::Builder::new()
            .name("rebook-chat-assets".into())
            .spawn(move || asset_worker(&request_receiver, &result_sender))
            .expect("chat asset worker should start");
        Self {
            markdown_cache: CommonMarkCache::default(),
            assets: HashMap::new(),
            asset_sender,
            asset_receiver,
            preview_states: HashMap::new(),
        }
    }
}

impl ChatMarkdownState {
    pub(super) fn show(&mut self, ui: &mut egui::Ui, source: &str, language: AppLanguage) {
        self.drain_asset_results();
        let blocks = split_renderable_blocks(source);
        for (index, block) in blocks.iter().enumerate() {
            match block {
                RenderBlock::Markdown(markdown) => self.show_commonmark(ui, markdown),
                RenderBlock::Preview { kind, source } => {
                    self.show_code_preview(ui, *kind, source, language);
                }
            }
            if index + 1 < blocks.len() {
                ui.add_space(PREVIEW_GAP);
            }
        }
    }

    fn show_commonmark(&mut self, ui: &mut egui::Ui, markdown: &str) {
        if markdown.trim().is_empty() {
            return;
        }
        let formula_assets = self.prepare_markdown_math(markdown);

        let math_renderer = move |ui: &mut egui::Ui, tex: &str, inline: bool| {
            paint_asset(ui, &formula_assets, &AssetKey::formula(tex, inline), inline);
        };
        let html_renderer = |ui: &mut egui::Ui, html: &str| {
            if is_svg_markup(html) {
                paint_svg(ui, html.as_bytes(), "inline-svg", false);
            } else {
                ui.add(
                    egui::Label::new(RichText::new(html).monospace().size(MARKDOWN_FONT_SIZE))
                        .wrap()
                        .selectable(true),
                );
            }
        };
        let cache = &mut self.markdown_cache;
        ui.scope(|ui| {
            ui.style_mut().interaction.selectable_labels = true;
            ui.style_mut().text_styles.insert(
                TextStyle::Body,
                FontId::new(MARKDOWN_FONT_SIZE, FontFamily::Proportional),
            );
            ui.style_mut().text_styles.insert(
                TextStyle::Monospace,
                FontId::new(12.0, FontFamily::Monospace),
            );
            CommonMarkViewer::new()
                .indentation_spaces(2)
                .render_math_fn(Some(&math_renderer))
                .render_html_fn(Some(&html_renderer))
                .show(ui, cache, markdown);
        });
    }

    fn show_code_preview(
        &mut self,
        ui: &mut egui::Ui,
        kind: PreviewKind,
        source: &str,
        language: AppLanguage,
    ) {
        let preview_id = stable_hash(&(kind, source));
        let mut state = self
            .preview_states
            .get(&preview_id)
            .copied()
            .unwrap_or_default();

        egui::Frame::new()
            .fill(SURFACE_MUTED)
            .stroke(egui::Stroke::new(1.0, BORDER))
            .corner_radius(7)
            .inner_margin(egui::Margin::symmetric(8, 7))
            .show(ui, |ui| {
                ui.set_min_width(ui.available_width());
                ui.horizontal(|ui| {
                    ui.label(RichText::new(kind.label()).size(11.0).strong().color(MUTED));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let collapse_label = if state.expanded {
                            language.text("收起", "Collapse")
                        } else {
                            language.text("展开", "Expand")
                        };
                        if compact_text_button(ui, collapse_label, false).clicked() {
                            state.expanded = !state.expanded;
                        }
                        if compact_text_button(
                            ui,
                            language.text("代码", "Code"),
                            state.mode == PreviewMode::Code,
                        )
                        .clicked()
                        {
                            state.mode = PreviewMode::Code;
                            state.expanded = true;
                        }
                        if compact_text_button(
                            ui,
                            language.text("预览", "Preview"),
                            state.mode == PreviewMode::Preview,
                        )
                        .clicked()
                        {
                            state.mode = PreviewMode::Preview;
                            state.expanded = true;
                        }
                    });
                });

                if state.expanded {
                    ui.add_space(7.0);
                    match state.mode {
                        PreviewMode::Preview => {
                            self.show_preview_result(ui, kind, source, language);
                        }
                        PreviewMode::Code => show_source_code(ui, source),
                    }
                }
            });
        self.preview_states.insert(preview_id, state);
    }

    fn show_preview_result(
        &mut self,
        ui: &mut egui::Ui,
        kind: PreviewKind,
        source: &str,
        language: AppLanguage,
    ) {
        match kind {
            PreviewKind::Svg => {
                if is_svg_markup(source) {
                    paint_svg(ui, source.as_bytes(), "svg-preview", false);
                } else {
                    preview_error(ui, language.text("SVG 内容无效", "Invalid SVG content"));
                }
            }
            PreviewKind::Mermaid => {
                let key = AssetKey::Mermaid(source.to_owned());
                self.queue_asset(key.clone());
                paint_asset(ui, &self.assets, &key, false);
            }
        }
    }

    fn prepare_markdown_math(&mut self, markdown: &str) -> HashMap<AssetKey, AssetStatus> {
        let mut keys = Vec::new();
        for event in Parser::new_ext(markdown, markdown_options()) {
            let key = match event {
                Event::InlineMath(tex) => Some(AssetKey::formula(&tex, true)),
                Event::DisplayMath(tex) => Some(AssetKey::formula(&tex, false)),
                _ => None,
            };
            if let Some(key) = key {
                self.queue_asset(key.clone());
                keys.push(key);
            }
        }
        keys.into_iter()
            .filter_map(|key| self.assets.get(&key).cloned().map(|status| (key, status)))
            .collect()
    }

    fn queue_asset(&mut self, key: AssetKey) {
        if self.assets.contains_key(&key) {
            return;
        }
        self.assets.insert(key.clone(), AssetStatus::Pending);
        if self
            .asset_sender
            .send(AssetRequest { key: key.clone() })
            .is_err()
        {
            self.assets.insert(
                key,
                AssetStatus::Error("render worker is unavailable".to_owned()),
            );
        }
    }

    fn drain_asset_results(&mut self) {
        while let Ok(result) = self.asset_receiver.try_recv() {
            self.assets.insert(result.key, result.status);
        }
    }
}

#[derive(Clone, Debug, Hash, PartialEq, Eq)]
enum AssetKey {
    Formula { tex: String, inline: bool },
    Mermaid(String),
}

impl AssetKey {
    fn formula(tex: &str, inline: bool) -> Self {
        Self::Formula {
            tex: tex.to_owned(),
            inline,
        }
    }
}

#[derive(Clone)]
enum AssetStatus {
    Pending,
    Ready(Arc<[u8]>),
    Error(String),
}

struct AssetRequest {
    key: AssetKey,
}

struct AssetResult {
    key: AssetKey,
    status: AssetStatus,
}

fn asset_worker(requests: &Receiver<AssetRequest>, results: &Sender<AssetResult>) {
    while let Ok(request) = requests.recv() {
        let status = match &request.key {
            AssetKey::Formula { tex, inline } => match render_formula(tex, *inline) {
                Ok(svg) => AssetStatus::Ready(Arc::from(svg.into_bytes())),
                Err(error) => AssetStatus::Error(error),
            },
            AssetKey::Mermaid(source) => match mermaid_rs_renderer::render(source) {
                Ok(svg) => AssetStatus::Ready(Arc::from(svg.into_bytes())),
                Err(error) => AssetStatus::Error(error.to_string()),
            },
        };
        if results
            .send(AssetResult {
                key: request.key,
                status,
            })
            .is_err()
        {
            break;
        }
    }
}

fn render_formula(tex: &str, inline: bool) -> Result<String, String> {
    let font_size = if inline { 13.0 } else { 16.0 };
    let padding = if inline { 1.0 } else { 4.0 };
    let mut measure = markie::fonts::CosmicTextMeasure::new()?;
    let rendered = markie::math::render_math(tex, font_size, "#262624", &mut measure, !inline)?;
    let width = (rendered.width + padding * 2.0).max(1.0);
    let height = (rendered.ascent + rendered.descent + padding * 2.0).max(1.0);
    let view_y = -rendered.ascent - padding;
    Ok(format!(
        r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="{} {} {} {}" width="{}" height="{}"><g transform="translate({}, 0)">{}</g></svg>"#,
        -padding, view_y, width, height, width, height, padding, rendered.svg_fragment
    ))
}

fn paint_asset(
    ui: &mut egui::Ui,
    assets: &HashMap<AssetKey, AssetStatus>,
    key: &AssetKey,
    inline: bool,
) {
    match assets.get(key) {
        Some(AssetStatus::Ready(svg)) => {
            let uri = format!("bytes://rebook-chat-{}.svg", stable_hash(key));
            let mut image = egui::Image::new(ImageSource::Bytes {
                uri: uri.into(),
                bytes: egui::load::Bytes::Shared(Arc::clone(svg)),
            });
            if inline {
                image = image.max_height(22.0);
            } else {
                image = image
                    .max_width(ui.available_width())
                    .max_height(320.0)
                    .maintain_aspect_ratio(true);
            }
            ui.add(image);
        }
        Some(AssetStatus::Error(error)) => {
            preview_error(ui, error);
        }
        Some(AssetStatus::Pending) | None => {
            ui.ctx().request_repaint_after(ASSET_REPAINT_INTERVAL);
            if inline {
                ui.label(RichText::new("…").size(MARKDOWN_FONT_SIZE).color(MUTED));
            } else {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label(RichText::new("Rendering…").size(11.5).color(MUTED));
                });
            }
        }
    }
}

fn paint_svg(ui: &mut egui::Ui, bytes: &[u8], namespace: &str, inline: bool) {
    let uri = format!(
        "bytes://rebook-chat-{namespace}-{}.svg",
        stable_hash(&bytes)
    );
    let mut image = egui::Image::new(ImageSource::Bytes {
        uri: uri.into(),
        bytes: egui::load::Bytes::Shared(Arc::from(bytes)),
    });
    if inline {
        image = image.max_height(22.0);
    } else {
        image = image
            .max_width(ui.available_width())
            .max_height(320.0)
            .maintain_aspect_ratio(true);
    }
    ui.add(image);
}

fn preview_error(ui: &mut egui::Ui, error: &str) {
    egui::Frame::new()
        .fill(Color32::from_rgb(252, 239, 238))
        .corner_radius(5)
        .inner_margin(6)
        .show(ui, |ui| {
            ui.add(
                egui::Label::new(
                    RichText::new(error)
                        .size(11.5)
                        .color(Color32::from_rgb(151, 54, 50)),
                )
                .wrap()
                .selectable(true),
            );
        });
}

fn show_source_code(ui: &mut egui::Ui, source: &str) {
    egui::Frame::new()
        .fill(SURFACE)
        .corner_radius(5)
        .inner_margin(7)
        .show(ui, |ui| {
            ui.add(
                egui::Label::new(RichText::new(source).monospace().size(11.5).color(TEXT))
                    .wrap()
                    .selectable(true),
            );
        });
}

fn compact_text_button(ui: &mut egui::Ui, label: &str, selected: bool) -> egui::Response {
    let galley = ui.painter().layout_no_wrap(
        label.to_owned(),
        FontId::new(10.5, FontFamily::Proportional),
        if selected { ACCENT } else { MUTED },
    );
    let size = Vec2::new(galley.size().x + 12.0, 25.0);
    let (rect, response) = ui.allocate_exact_size(size, Sense::click());
    let fill = if selected {
        ACCENT_SOFT
    } else if response.hovered() {
        ui.visuals().widgets.hovered.weak_bg_fill
    } else {
        Color32::TRANSPARENT
    };
    if ui.is_rect_visible(rect) {
        ui.painter().rect_filled(rect, 5.0, fill);
        ui.painter().galley(
            egui::pos2(
                rect.center().x - galley.size().x * 0.5,
                rect.center().y - galley.size().y * 0.5,
            ),
            galley,
            if selected { ACCENT } else { MUTED },
        );
    }
    response.on_hover_cursor(egui::CursorIcon::PointingHand)
}

#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq)]
enum PreviewKind {
    Svg,
    Mermaid,
}

impl PreviewKind {
    const fn label(self) -> &'static str {
        match self {
            Self::Svg => "SVG",
            Self::Mermaid => "Mermaid",
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum PreviewMode {
    #[default]
    Preview,
    Code,
}

#[derive(Clone, Copy, Debug)]
struct PreviewState {
    mode: PreviewMode,
    expanded: bool,
}

impl Default for PreviewState {
    fn default() -> Self {
        Self {
            mode: PreviewMode::Preview,
            expanded: true,
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
enum RenderBlock<'a> {
    Markdown(&'a str),
    Preview { kind: PreviewKind, source: String },
}

struct OpenFence {
    start: usize,
    language: String,
    content: String,
}

fn split_renderable_blocks(source: &str) -> Vec<RenderBlock<'_>> {
    let mut blocks = Vec::new();
    let mut cursor = 0;
    let mut open_fence: Option<OpenFence> = None;

    for (event, range) in Parser::new_ext(source, markdown_options()).into_offset_iter() {
        match event {
            Event::Start(Tag::CodeBlock(CodeBlockKind::Fenced(language))) => {
                open_fence = Some(OpenFence {
                    start: range.start,
                    language: language.into_string(),
                    content: String::new(),
                });
            }
            Event::Text(text) if open_fence.is_some() => {
                if let Some(fence) = &mut open_fence {
                    fence.content.push_str(&text);
                }
            }
            Event::End(TagEnd::CodeBlock) => {
                if let Some(fence) = open_fence.take()
                    && let Some(kind) = classify_preview(&fence.language, &fence.content)
                {
                    if cursor < fence.start {
                        blocks.push(RenderBlock::Markdown(&source[cursor..fence.start]));
                    }
                    blocks.push(RenderBlock::Preview {
                        kind,
                        source: fence.content,
                    });
                    cursor = range.end;
                }
            }
            _ => {}
        }
    }

    if cursor < source.len() {
        blocks.push(RenderBlock::Markdown(&source[cursor..]));
    }
    if blocks.is_empty() {
        blocks.push(RenderBlock::Markdown(source));
    }
    blocks
}

fn classify_preview(language: &str, source: &str) -> Option<PreviewKind> {
    let language = language
        .split_ascii_whitespace()
        .next()
        .unwrap_or_default()
        .to_ascii_lowercase();
    match language.as_str() {
        "mermaid" | "mmd" if !source.trim().is_empty() => Some(PreviewKind::Mermaid),
        "svg" | "html" | "xml" if is_svg_markup(source) => Some(PreviewKind::Svg),
        _ => None,
    }
}

fn is_svg_markup(source: &str) -> bool {
    let normalized = source.trim().to_ascii_lowercase();
    normalized.contains("<svg") && normalized.contains("</svg>")
}

fn markdown_options() -> Options {
    Options::ENABLE_STRIKETHROUGH
        | Options::ENABLE_TABLES
        | Options::ENABLE_FOOTNOTES
        | Options::ENABLE_TASKLISTS
        | Options::ENABLE_MATH
}

fn stable_hash<T: Hash + ?Sized>(value: &T) -> u64 {
    let mut hasher = DefaultHasher::new();
    value.hash(&mut hasher);
    hasher.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_svg_and_mermaid_without_replacing_regular_code() {
        let source = r#"# Heading

**bold**

```rust
fn main() {}
```

```svg
<svg viewBox="0 0 10 10"><circle cx="5" cy="5" r="4" /></svg>
```

```mmd
flowchart LR
    A --> B
```
"#;
        let blocks = split_renderable_blocks(source);
        assert_eq!(blocks.len(), 5);
        assert!(
            matches!(blocks[0], RenderBlock::Markdown(markdown) if markdown.contains("```rust"))
        );
        assert!(matches!(
            blocks[1],
            RenderBlock::Preview {
                kind: PreviewKind::Svg,
                ..
            }
        ));
        assert!(matches!(blocks[2], RenderBlock::Markdown(markdown) if markdown.trim().is_empty()));
        assert!(matches!(
            blocks[3],
            RenderBlock::Preview {
                kind: PreviewKind::Mermaid,
                ..
            }
        ));
        assert!(matches!(blocks[4], RenderBlock::Markdown(markdown) if markdown.trim().is_empty()));
    }

    #[test]
    fn recognizes_html_svg_but_not_regular_html_or_code() {
        assert_eq!(
            classify_preview("html", "<svg><path /></svg>"),
            Some(PreviewKind::Svg)
        );
        assert_eq!(classify_preview("html", "<div>hello</div>"), None);
        assert_eq!(classify_preview("rust", "let x = 1;"), None);
    }

    #[test]
    fn markdown_parser_detects_inline_and_display_math() {
        let events = Parser::new_ext(
            "Inline $x^2$ and display $$\\frac{a}{b}$$",
            markdown_options(),
        )
        .collect::<Vec<_>>();
        assert!(
            events
                .iter()
                .any(|event| matches!(event, Event::InlineMath(_)))
        );
        assert!(
            events
                .iter()
                .any(|event| matches!(event, Event::DisplayMath(_)))
        );
    }

    #[test]
    fn mermaid_renderer_produces_an_svg() {
        let svg = mermaid_rs_renderer::render("flowchart LR; A-->B").unwrap();
        assert!(svg.contains("<svg"));
        assert!(svg.contains("</svg>"));
    }

    #[test]
    fn formula_renderer_produces_a_standalone_svg() {
        let svg = render_formula(r"\frac{a}{b}", false).unwrap();
        assert!(svg.contains("<svg"));
        assert!(svg.contains("<text"));
        assert!(svg.contains("<line"));
        assert!(svg.contains("</svg>"));
    }
}
