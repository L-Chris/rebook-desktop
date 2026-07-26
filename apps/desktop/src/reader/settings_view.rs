use std::borrow::Cow;
use std::collections::HashSet;

use lucide_icons::Icon;
use rebook_layout::{ReaderDefaultFont, ReaderTypography, SpreadMode};
use xilem::masonry::parley::style::FontStack;
use xilem::masonry::properties::types::{AsUnit, UnitPoint};
use xilem::style::{Padding, Style};
use xilem::view::{
    CrossAxisAlignment, FlexExt, FlexSpacer, ZStackExt, flex_col, flex_row, label, portal, prose,
    sized_box, text_input, zstack,
};
use xilem::{Affine, AnyWidgetView, Color, FontWeight, WidgetView};

use crate::plugins::{AiProvider, BUILTIN_PLUGINS, PluginSettings, TranslationMode};
use crate::ui::{
    CONTENT_GAP, CONTENT_PADDING_HORIZONTAL, CONTENT_PADDING_VERTICAL, CONTROL_HEIGHT,
    CONTROL_HEIGHT_COMPACT, DIALOG_FOOTER_HEIGHT, DIALOG_HEADER_HEIGHT, RADIUS_DIALOG,
    RADIUS_LARGE, RADIUS_MEDIUM, RADIUS_SMALL, SETTINGS_ROW_HEIGHT, UI_ACCENT, UI_ACCENT_BORDER,
    UI_ACCENT_SOFT, UI_BORDER, UI_FONT_STACK, UI_MUTED, UI_SURFACE, UI_SURFACE_MUTED, UI_TEXT,
    UI_TEXT_SOFT, button, divider, icon_label,
};

use super::view::{
    animated_scrim, icon_button, modal_scrim_color, primary_action_button, secondary_action_button,
    value_badge,
};
use super::{DesktopReader, FontPickerKind, SettingsTab};

const SETTINGS_WIDTH: f64 = 660.0;
const SETTINGS_HEIGHT: f64 = 500.0;

pub(super) fn settings_overlay(
    state: &DesktopReader,
    progress: f32,
) -> impl WidgetView<DesktopReader> + use<> {
    // Keep glyphs and one-pixel borders at their native scale throughout the
    // transition. Scaling the complete dialog makes text shimmer as Vello
    // resamples it on every frame, which reads as dropped frames on Windows.
    let offset = 8.0 * f64::from(1.0 - progress);
    let dialog_transform = Affine::translate((0.0, offset));
    sized_box(zstack((
        animated_scrim(modal_scrim_color(progress), DesktopReader::close_overlay),
        sized_box(settings_dialog(state))
            .width(SETTINGS_WIDTH.px())
            .height(SETTINGS_HEIGHT.px())
            .background_color(UI_SURFACE)
            .border(UI_BORDER, 1.0)
            .corner_radius(RADIUS_DIALOG)
            .transform(dialog_transform),
    )))
    .expand()
}

fn settings_dialog(state: &DesktopReader) -> impl WidgetView<DesktopReader> + use<> {
    settings_content(state)
}

fn settings_content(state: &DesktopReader) -> impl WidgetView<DesktopReader> + use<> {
    let spread = state.ui.draft_spread;
    let typography = &state.ui.draft_typography;
    let font_picker = state.ui.font_picker;
    let tab = state.ui.settings_tab;
    let title = match tab {
        SettingsTab::Reading => "阅读",
        SettingsTab::Font => font_picker.map_or("字体", FontPickerKind::title),
        SettingsTab::Ai => "AI",
        SettingsTab::AiChat => "AI Chat",
        SettingsTab::Translation => "翻译",
        SettingsTab::Plugins => "插件",
    };
    let body: Box<AnyWidgetView<DesktopReader>> = match tab {
        SettingsTab::Reading => reading_settings_content(spread).boxed(),
        SettingsTab::Font => match font_picker {
            Some(kind) => {
                font_picker_content(kind, typography, &state.available_font_families).boxed()
            }
            None => font_settings_content(typography).boxed(),
        },
        SettingsTab::Ai => ai_settings_content(state.ui.draft_plugin_settings.clone()).boxed(),
        SettingsTab::AiChat => ai_chat_settings_content(&state.ui.draft_plugin_settings).boxed(),
        SettingsTab::Translation => {
            translation_settings_content(&state.ui.draft_plugin_settings).boxed()
        }
        SettingsTab::Plugins => plugin_settings_content().boxed(),
    };

    flex_row((
        sized_box(zstack((
            sized_box(label(""))
                .expand()
                .background_color(UI_SURFACE_MUTED)
                .corner_radius(RADIUS_LARGE),
            sized_box(label(""))
                .width(RADIUS_DIALOG.px())
                .expand_height()
                .background_color(UI_SURFACE_MUTED)
                .alignment(UnitPoint::RIGHT),
            sized_box(
                flex_col((
                    flex_row((
                        icon_label(Icon::Settings, 17.0, UI_MUTED),
                        label("设置")
                            .font(UI_FONT_STACK)
                            .text_size(15.0)
                            .weight(FontWeight::BOLD)
                            .color(UI_TEXT),
                    ))
                    .gap(9.px())
                    .cross_axis_alignment(CrossAxisAlignment::Center)
                    .padding(Padding::from_vh(9.0, 8.0)),
                    settings_tab_button("阅读", SettingsTab::Reading, tab),
                    settings_tab_button("字体", SettingsTab::Font, tab),
                    settings_tab_button("AI", SettingsTab::Ai, tab),
                    settings_tab_button("AI Chat", SettingsTab::AiChat, tab),
                    settings_tab_button("翻译", SettingsTab::Translation, tab),
                    settings_tab_button("插件", SettingsTab::Plugins, tab),
                    FlexSpacer::Flex(1.0),
                ))
                .gap(3.px())
                .cross_axis_alignment(CrossAxisAlignment::Fill)
                .padding(8.0),
            )
            .expand(),
        )))
        .width(136.px())
        .expand_height(),
        flex_col((
            settings_dialog_header(title),
            divider(),
            body.flex(1.0),
            divider(),
            sized_box(
                flex_row((
                    FlexSpacer::Flex(1.0),
                    secondary_action_button("取消", DesktopReader::close_overlay),
                    primary_action_button("应用", DesktopReader::apply_settings),
                ))
                .gap(8.px())
                .cross_axis_alignment(CrossAxisAlignment::Center),
            )
            .height(DIALOG_FOOTER_HEIGHT.px())
            .expand_width()
            .padding(Padding::horizontal(CONTENT_PADDING_HORIZONTAL)),
        ))
        .must_fill_major_axis(true)
        .flex(1.0),
    ))
}

fn settings_dialog_header(title: &'static str) -> impl WidgetView<DesktopReader> {
    sized_box(
        flex_row((
            label(title)
                .font(UI_FONT_STACK)
                .text_size(15.0)
                .weight(FontWeight::BOLD)
                .color(UI_TEXT),
            FlexSpacer::Flex(1.0),
            icon_button(Icon::X, false, DesktopReader::close_overlay),
        ))
        .cross_axis_alignment(CrossAxisAlignment::Center),
    )
    .height(DIALOG_HEADER_HEIGHT.px())
    .padding(Padding::horizontal(CONTENT_PADDING_HORIZONTAL))
}

fn settings_tab_button(
    text: &'static str,
    value: SettingsTab,
    selected: SettingsTab,
) -> impl WidgetView<DesktopReader> {
    let active = value == selected;
    sized_box(
        button(
            flex_row((
                label(text)
                    .font(UI_FONT_STACK)
                    .text_size(13.0)
                    .weight(if active {
                        FontWeight::BOLD
                    } else {
                        FontWeight::NORMAL
                    })
                    .color(if active { UI_TEXT } else { UI_TEXT_SOFT }),
                FlexSpacer::Flex(1.0),
            )),
            move |state: &mut DesktopReader| {
                state.ui.settings_tab = value;
                state.ui.font_picker = None;
            },
        )
        .background_color(if active {
            UI_SURFACE
        } else {
            Color::TRANSPARENT
        })
        .active_background_color(UI_ACCENT_SOFT)
        .border_color(if active {
            UI_BORDER
        } else {
            Color::TRANSPARENT
        })
        .hovered_border_color(UI_BORDER)
        .corner_radius(RADIUS_SMALL)
        .padding(Padding::horizontal(10.0)),
    )
    .height(CONTROL_HEIGHT.px())
    .expand_width()
}

fn reading_settings_content(spread: SpreadMode) -> impl WidgetView<DesktopReader> {
    flex_col((
        label("页面布局")
            .font(UI_FONT_STACK)
            .text_size(12.0)
            .weight(FontWeight::BOLD)
            .color(UI_MUTED),
        sized_box(flex_col((
            settings_value_row("阅读模式", "分页"),
            divider(),
            spread_settings_row(spread),
        )))
        .background_color(UI_SURFACE)
        .border(UI_BORDER, 1.0)
        .corner_radius(RADIUS_MEDIUM),
    ))
    .gap(CONTENT_GAP.px())
    .cross_axis_alignment(CrossAxisAlignment::Fill)
    .padding(Padding::from_vh(
        CONTENT_PADDING_VERTICAL,
        CONTENT_PADDING_HORIZONTAL,
    ))
}

fn font_settings_content(typography: &ReaderTypography) -> impl WidgetView<DesktopReader> + use<> {
    let preview_font = typography.default_stack();
    let preview_size = typography.font_size.min(24.0);
    let preview_weight = FontWeight::new(f32::from(typography.font_weight));
    let default_font = typography.default_font;
    let font_size = typography.font_size;
    let minimum_font_size = typography.minimum_font_size;
    let font_weight = typography.font_weight;

    portal(
        flex_col((
            settings_section_label("字号与字重"),
            typography_metrics_card(font_size, minimum_font_size, font_weight),
            settings_section_label("字体"),
            flex_col((
                default_font_row(default_font),
                divider(),
                font_family_settings_row(
                    "中文字体",
                    typography.default_cjk_font.clone(),
                    FontPickerKind::Cjk,
                ),
            ))
            .cross_axis_alignment(CrossAxisAlignment::Fill)
            .background_color(UI_SURFACE)
            .border(UI_BORDER, 1.0)
            .corner_radius(RADIUS_MEDIUM),
            settings_section_label("字型"),
            flex_col((
                font_family_settings_row(
                    "衬线字体",
                    typography.serif_font.clone(),
                    FontPickerKind::Serif,
                ),
                divider(),
                font_family_settings_row(
                    "无衬线字体",
                    typography.sans_serif_font.clone(),
                    FontPickerKind::SansSerif,
                ),
                divider(),
                font_family_settings_row(
                    "等宽字体",
                    typography.monospace_font.clone(),
                    FontPickerKind::Monospace,
                ),
            ))
            .cross_axis_alignment(CrossAxisAlignment::Fill)
            .background_color(UI_SURFACE)
            .border(UI_BORDER, 1.0)
            .corner_radius(RADIUS_MEDIUM),
            sized_box(
                flex_col((
                    label("字体预览")
                        .font(UI_FONT_STACK)
                        .text_size(11.0)
                        .color(UI_MUTED),
                    label("阅读让思想抵达更远的地方 Reading 0123")
                        .font(ui_font_stack(preview_font))
                        .text_size(preview_size)
                        .weight(preview_weight)
                        .color(UI_TEXT),
                ))
                .gap(6.px())
                .cross_axis_alignment(CrossAxisAlignment::Start),
            )
            .background_color(UI_SURFACE_MUTED)
            .border(UI_BORDER, 1.0)
            .corner_radius(RADIUS_MEDIUM)
            .padding(Padding::from_vh(10.0, 12.0)),
        ))
        .gap(CONTENT_GAP.px())
        .cross_axis_alignment(CrossAxisAlignment::Fill)
        .padding(Padding::from_vh(
            CONTENT_PADDING_VERTICAL,
            CONTENT_PADDING_HORIZONTAL,
        )),
    )
}

fn typography_metrics_card(
    font_size: f32,
    minimum_font_size: f32,
    font_weight: u16,
) -> impl WidgetView<DesktopReader> {
    flex_col((
        typography_stepper_row(
            "默认字号",
            format!("{font_size:.0} px"),
            |state: &mut DesktopReader| {
                let minimum = state.ui.draft_typography.minimum_font_size;
                state.ui.draft_typography.font_size =
                    (state.ui.draft_typography.font_size - 1.0).max(minimum);
            },
            |state: &mut DesktopReader| {
                state.ui.draft_typography.font_size =
                    (state.ui.draft_typography.font_size + 1.0).min(120.0);
            },
        ),
        divider(),
        typography_stepper_row(
            "最小字号",
            format!("{minimum_font_size:.0} px"),
            |state: &mut DesktopReader| {
                state.ui.draft_typography.minimum_font_size =
                    (state.ui.draft_typography.minimum_font_size - 1.0).max(1.0);
            },
            |state: &mut DesktopReader| {
                let typography = &mut state.ui.draft_typography;
                typography.minimum_font_size = (typography.minimum_font_size + 1.0).min(120.0);
                typography.font_size = typography.font_size.max(typography.minimum_font_size);
            },
        ),
        divider(),
        typography_stepper_row(
            "字体粗细",
            font_weight.to_string(),
            |state: &mut DesktopReader| {
                state.ui.draft_typography.font_weight = state
                    .ui
                    .draft_typography
                    .font_weight
                    .saturating_sub(100)
                    .max(100);
            },
            |state: &mut DesktopReader| {
                state.ui.draft_typography.font_weight = state
                    .ui
                    .draft_typography
                    .font_weight
                    .saturating_add(100)
                    .min(900);
            },
        ),
    ))
    .cross_axis_alignment(CrossAxisAlignment::Fill)
    .background_color(UI_SURFACE)
    .border(UI_BORDER, 1.0)
    .corner_radius(RADIUS_MEDIUM)
}

fn settings_section_label(text: &'static str) -> impl WidgetView<DesktopReader> {
    label(text)
        .font(UI_FONT_STACK)
        .text_size(12.0)
        .weight(FontWeight::BOLD)
        .color(UI_MUTED)
}

fn typography_stepper_row(
    name: &'static str,
    value: String,
    decrease: impl Fn(&mut DesktopReader) + Send + Sync + 'static,
    increase: impl Fn(&mut DesktopReader) + Send + Sync + 'static,
) -> impl WidgetView<DesktopReader> {
    sized_box(
        flex_row((
            label(name)
                .font(UI_FONT_STACK)
                .text_size(13.0)
                .color(UI_TEXT_SOFT),
            FlexSpacer::Flex(1.0),
            stepper_button(Icon::Minus, decrease),
            sized_box(
                label(value)
                    .font(UI_FONT_STACK)
                    .text_size(12.0)
                    .color(UI_TEXT),
            )
            .width(62.px()),
            stepper_button(Icon::Plus, increase),
        ))
        .gap(5.px())
        .cross_axis_alignment(CrossAxisAlignment::Center),
    )
    .height(SETTINGS_ROW_HEIGHT.px())
    .expand_width()
    .padding(Padding::horizontal(12.0))
}

fn stepper_button(
    icon: Icon,
    callback: impl Fn(&mut DesktopReader) + Send + Sync + 'static,
) -> impl WidgetView<DesktopReader> {
    sized_box(
        button(icon_label(icon, 13.0, UI_TEXT_SOFT), callback)
            .background_color(UI_SURFACE_MUTED)
            .active_background_color(UI_ACCENT_SOFT)
            .border_color(UI_BORDER)
            .hovered_border_color(UI_ACCENT_BORDER)
            .corner_radius(RADIUS_SMALL)
            .padding(0.0),
    )
    .width(CONTROL_HEIGHT_COMPACT.px())
    .height(CONTROL_HEIGHT_COMPACT.px())
}

fn default_font_row(selected: ReaderDefaultFont) -> impl WidgetView<DesktopReader> {
    sized_box(
        flex_row((
            label("默认字体")
                .font(UI_FONT_STACK)
                .text_size(13.0)
                .color(UI_TEXT_SOFT),
            FlexSpacer::Flex(1.0),
            default_font_choice("衬线", ReaderDefaultFont::Serif, selected),
            default_font_choice("无衬线", ReaderDefaultFont::SansSerif, selected),
        ))
        .gap(6.px())
        .cross_axis_alignment(CrossAxisAlignment::Center),
    )
    .height(SETTINGS_ROW_HEIGHT.px())
    .expand_width()
    .padding(Padding::horizontal(12.0))
}

fn default_font_choice(
    text: &'static str,
    value: ReaderDefaultFont,
    selected: ReaderDefaultFont,
) -> impl WidgetView<DesktopReader> {
    let active = value == selected;
    sized_box(
        button(
            label(text)
                .font(UI_FONT_STACK)
                .text_size(12.0)
                .weight(if active {
                    FontWeight::BOLD
                } else {
                    FontWeight::NORMAL
                })
                .color(if active { UI_ACCENT } else { UI_TEXT_SOFT }),
            move |state: &mut DesktopReader| state.ui.draft_typography.default_font = value,
        )
        .background_color(if active { UI_ACCENT_SOFT } else { UI_SURFACE })
        .active_background_color(UI_ACCENT_SOFT)
        .border_color(if active { UI_ACCENT_BORDER } else { UI_BORDER })
        .hovered_border_color(UI_ACCENT_BORDER)
        .corner_radius(RADIUS_SMALL)
        .padding(Padding::from_vh(5.0, 9.0)),
    )
    .height(CONTROL_HEIGHT_COMPACT.px())
}

fn font_family_settings_row(
    name: &'static str,
    value: String,
    picker: FontPickerKind,
) -> impl WidgetView<DesktopReader> {
    let display_value = value.clone();
    sized_box(
        button(
            flex_row((
                label(name)
                    .font(UI_FONT_STACK)
                    .text_size(13.0)
                    .color(UI_TEXT_SOFT),
                FlexSpacer::Flex(1.0),
                label(display_value)
                    .font(ui_font_stack(value))
                    .text_size(12.0)
                    .color(UI_TEXT),
                icon_label(Icon::ChevronRight, 14.0, UI_MUTED),
            ))
            .gap(8.px())
            .cross_axis_alignment(CrossAxisAlignment::Center),
            move |state: &mut DesktopReader| state.ui.font_picker = Some(picker),
        )
        .background_color(UI_SURFACE)
        .active_background_color(UI_SURFACE_MUTED)
        .border_color(Color::TRANSPARENT)
        .hovered_border_color(Color::TRANSPARENT)
        .border_width(0.0)
        .padding(Padding::horizontal(12.0)),
    )
    .height(SETTINGS_ROW_HEIGHT.px())
    .expand_width()
}

fn font_picker_content(
    kind: FontPickerKind,
    typography: &ReaderTypography,
    available_families: &[String],
) -> impl WidgetView<DesktopReader> + use<> {
    let selected = selected_font_family(typography, kind).to_owned();
    let rows = font_candidates(kind, available_families)
        .into_iter()
        .map(|family| font_picker_row(family, &selected, kind))
        .collect::<Vec<_>>();

    portal(
        flex_col((
            sized_box(
                button(
                    flex_row((
                        icon_label(Icon::ChevronLeft, 14.0, UI_MUTED),
                        label("返回字体设置")
                            .font(UI_FONT_STACK)
                            .text_size(12.0)
                            .color(UI_TEXT_SOFT),
                    ))
                    .gap(6.px())
                    .cross_axis_alignment(CrossAxisAlignment::Center),
                    |state: &mut DesktopReader| state.ui.font_picker = None,
                )
                .background_color(Color::TRANSPARENT)
                .active_background_color(UI_SURFACE_MUTED)
                .border_color(Color::TRANSPARENT)
                .hovered_border_color(Color::TRANSPARENT)
                .border_width(0.0)
                .padding(Padding::from_vh(6.0, 8.0)),
            )
            .height(CONTROL_HEIGHT.px()),
            flex_col(rows)
                .cross_axis_alignment(CrossAxisAlignment::Fill)
                .background_color(UI_SURFACE)
                .border(UI_BORDER, 1.0)
                .corner_radius(RADIUS_MEDIUM),
        ))
        .gap(CONTENT_GAP.px())
        .cross_axis_alignment(CrossAxisAlignment::Fill)
        .padding(Padding::from_vh(
            CONTENT_PADDING_VERTICAL,
            CONTENT_PADDING_HORIZONTAL,
        )),
    )
}

fn selected_font_family(typography: &ReaderTypography, kind: FontPickerKind) -> &str {
    match kind {
        FontPickerKind::Cjk => &typography.default_cjk_font,
        FontPickerKind::Serif => &typography.serif_font,
        FontPickerKind::SansSerif => &typography.sans_serif_font,
        FontPickerKind::Monospace => &typography.monospace_font,
    }
}

fn font_picker_row(
    family: String,
    selected: &str,
    kind: FontPickerKind,
) -> impl WidgetView<DesktopReader> + use<> {
    let active = family.eq_ignore_ascii_case(selected);
    let label_text = family.clone();
    let label_font = family.clone();
    sized_box(
        button(
            flex_row((
                label(label_text)
                    .font(ui_font_stack(label_font))
                    .text_size(13.0)
                    .weight(if active {
                        FontWeight::BOLD
                    } else {
                        FontWeight::NORMAL
                    })
                    .color(if active { UI_ACCENT } else { UI_TEXT }),
                FlexSpacer::Flex(1.0),
                icon_label(
                    Icon::Check,
                    14.0,
                    if active {
                        UI_ACCENT
                    } else {
                        Color::TRANSPARENT
                    },
                ),
            ))
            .cross_axis_alignment(CrossAxisAlignment::Center),
            move |state: &mut DesktopReader| {
                match kind {
                    FontPickerKind::Cjk => {
                        state
                            .ui
                            .draft_typography
                            .default_cjk_font
                            .clone_from(&family);
                    }
                    FontPickerKind::Serif => {
                        state.ui.draft_typography.serif_font.clone_from(&family);
                    }
                    FontPickerKind::SansSerif => {
                        state
                            .ui
                            .draft_typography
                            .sans_serif_font
                            .clone_from(&family);
                    }
                    FontPickerKind::Monospace => {
                        state.ui.draft_typography.monospace_font.clone_from(&family);
                    }
                }
                state.ui.font_picker = None;
            },
        )
        .background_color(if active { UI_ACCENT_SOFT } else { UI_SURFACE })
        .active_background_color(UI_ACCENT_SOFT)
        .border_color(Color::TRANSPARENT)
        .hovered_border_color(Color::TRANSPARENT)
        .border_width(0.0)
        .padding(Padding::horizontal(12.0)),
    )
    .height(42.px())
    .expand_width()
}

pub(super) fn font_candidates(kind: FontPickerKind, available_families: &[String]) -> Vec<String> {
    let curated: &[&str] = match kind {
        FontPickerKind::Cjk => &[
            "LXGW WenKai GB Screen",
            "LXGW WenKai",
            "Noto Serif SC",
            "Noto Sans SC",
            "Microsoft YaHei",
            "SimSun",
            "KaiTi",
        ],
        FontPickerKind::Serif => &[
            "Bitter",
            "Literata",
            "Merriweather",
            "Noto Serif",
            "Georgia",
        ],
        FontPickerKind::SansSerif => &[
            "Roboto",
            "Noto Sans",
            "Open Sans",
            "Inter",
            "Microsoft YaHei",
        ],
        FontPickerKind::Monospace => &["Consolas", "Fira Code", "Roboto Mono", "IBM Plex Mono"],
    };
    let available = available_families
        .iter()
        .map(|family| family.to_lowercase())
        .collect::<HashSet<_>>();
    let mut seen = HashSet::new();
    let mut candidates = Vec::new();
    for family in curated
        .iter()
        .filter(|family| {
            **family == "LXGW WenKai GB Screen" || available.contains(&family.to_lowercase())
        })
        .map(|family| (*family).to_owned())
        .chain(available_families.iter().filter_map(|family| {
            if kind != FontPickerKind::Cjk || looks_like_cjk_font(family) {
                Some(family.clone())
            } else {
                None
            }
        }))
    {
        if seen.insert(family.to_lowercase()) {
            candidates.push(family);
        }
    }
    candidates
}

fn looks_like_cjk_font(family: &str) -> bool {
    let name = family.to_lowercase();
    [
        "cjk", "han", "song", "ming", "hei", "kai", "yahei", "wenkai", "gothic", "meiryo",
        "malgun", "pingfang", "fangsong", "simsun", "simhei",
    ]
    .iter()
    .any(|keyword| name.contains(keyword))
}

fn ui_font_stack(source: String) -> FontStack<'static> {
    FontStack::Source(Cow::Owned(source))
}

#[derive(Clone, Copy)]
enum AiSettingField {
    ProviderName(usize),
    ProviderBaseUrl(usize),
    ProviderApiKey(usize),
    ProviderModel {
        provider_index: usize,
        model_index: usize,
    },
    TargetLanguage,
}

#[derive(Clone, Copy)]
enum AiFeature {
    Chat,
    Translation,
}

fn ai_settings_content(settings: PluginSettings) -> impl WidgetView<DesktopReader> + use<> {
    let provider_count = settings.providers.len();
    let provider_cards = settings
        .providers
        .into_iter()
        .enumerate()
        .map(|(index, provider)| ai_provider_card(index, provider, provider_count > 1))
        .collect::<Vec<_>>();
    portal(
        flex_col((
            settings_section_label("Providers"),
            flex_col(provider_cards)
                .gap(CONTENT_GAP.px())
                .cross_axis_alignment(CrossAxisAlignment::Fill),
            secondary_action_button("新增 Provider", |state: &mut DesktopReader| {
                state.ui.draft_plugin_settings.add_provider();
            }),
            prose(
                "每个 Provider 可以维护多个模型。API Key 只保存在当前运行内存中，不会写入 plugins.json；默认 Provider 也可以通过 REBOOK_AI_API_KEY 环境变量提供密钥。",
            )
            .text_size(10.5)
            .text_color(UI_MUTED),
        ))
        .gap(CONTENT_GAP.px())
        .cross_axis_alignment(CrossAxisAlignment::Fill)
        .padding(Padding::from_vh(
            CONTENT_PADDING_VERTICAL,
            CONTENT_PADDING_HORIZONTAL,
        )),
    )
}

fn ai_provider_card(
    index: usize,
    provider: AiProvider,
    can_remove_provider: bool,
) -> impl WidgetView<DesktopReader> {
    let AiProvider {
        name,
        base_url,
        models,
        api_key,
        ..
    } = provider;
    let title = if name.trim().is_empty() {
        format!("Provider {}", index + 1)
    } else {
        name.clone()
    };
    let model_count = models.len();
    let model_rows = models
        .into_iter()
        .enumerate()
        .map(|(model_index, model)| {
            ai_provider_model_row(index, model_index, model, model_count > 1)
        })
        .collect::<Vec<_>>();
    let remove_provider: Box<AnyWidgetView<DesktopReader>> = if can_remove_provider {
        icon_button(Icon::Trash2, false, move |state: &mut DesktopReader| {
            state.ui.draft_plugin_settings.remove_provider(index);
        })
        .boxed()
    } else {
        sized_box(label("")).width(32.px()).height(32.px()).boxed()
    };

    flex_col((
        flex_row((
            icon_label(Icon::Bot, 16.0, UI_ACCENT),
            label(title)
                .font(UI_FONT_STACK)
                .text_size(12.5)
                .weight(FontWeight::BOLD)
                .color(UI_TEXT),
            FlexSpacer::Flex(1.0),
            remove_provider,
        ))
        .gap(8.px())
        .cross_axis_alignment(CrossAxisAlignment::Center)
        .padding(Padding::from_vh(7.0, 10.0)),
        divider(),
        ai_settings_input_row(
            "名称",
            name,
            "例如 OpenAI、Ollama",
            AiSettingField::ProviderName(index),
        ),
        divider(),
        ai_settings_input_row(
            "API 地址",
            base_url,
            "https://api.openai.com/v1",
            AiSettingField::ProviderBaseUrl(index),
        ),
        divider(),
        ai_settings_input_row(
            "API Key（仅本次会话）",
            api_key,
            "sk-…",
            AiSettingField::ProviderApiKey(index),
        ),
        divider(),
        flex_col((
            label("模型")
                .font(UI_FONT_STACK)
                .text_size(11.5)
                .weight(FontWeight::BOLD)
                .color(UI_MUTED),
            flex_col(model_rows).cross_axis_alignment(CrossAxisAlignment::Fill),
            secondary_action_button("新增模型", move |state: &mut DesktopReader| {
                if let Some(provider) = state.ui.draft_plugin_settings.providers.get_mut(index) {
                    provider.models.push(String::new());
                }
            }),
        ))
        .gap(6.px())
        .cross_axis_alignment(CrossAxisAlignment::Fill)
        .padding(Padding::from_vh(8.0, 10.0)),
    ))
    .cross_axis_alignment(CrossAxisAlignment::Fill)
    .background_color(UI_SURFACE)
    .border(UI_BORDER, 1.0)
    .corner_radius(RADIUS_MEDIUM)
}

fn ai_provider_model_row(
    provider_index: usize,
    model_index: usize,
    value: String,
    can_remove: bool,
) -> impl WidgetView<DesktopReader> {
    let remove: Box<AnyWidgetView<DesktopReader>> = if can_remove {
        icon_button(Icon::X, false, move |state: &mut DesktopReader| {
            state
                .ui
                .draft_plugin_settings
                .remove_model(provider_index, model_index);
        })
        .boxed()
    } else {
        sized_box(label("")).width(32.px()).height(32.px()).boxed()
    };
    sized_box(
        flex_row((
            label(format!("模型 {}", model_index + 1))
                .font(UI_FONT_STACK)
                .text_size(11.5)
                .color(UI_TEXT_SOFT),
            FlexSpacer::Flex(1.0),
            sized_box(
                text_input(value, move |state: &mut DesktopReader, value| {
                    set_ai_setting(
                        state,
                        AiSettingField::ProviderModel {
                            provider_index,
                            model_index,
                        },
                        value,
                    );
                })
                .placeholder("模型 ID，例如 gpt-4o-mini")
                .text_color(UI_TEXT)
                .caret_color(UI_ACCENT)
                .background_color(UI_SURFACE_MUTED)
                .border_color(UI_BORDER)
                .border_width(1.0)
                .corner_radius(RADIUS_SMALL)
                .padding(Padding::from_vh(4.0, 8.0)),
            )
            .width(250.px())
            .height(CONTROL_HEIGHT.px()),
            remove,
        ))
        .gap(6.px())
        .cross_axis_alignment(CrossAxisAlignment::Center),
    )
    .height(SETTINGS_ROW_HEIGHT.px())
    .expand_width()
}

fn ai_settings_input_row(
    label_text: &'static str,
    value: String,
    placeholder: &'static str,
    field: AiSettingField,
) -> impl WidgetView<DesktopReader> {
    sized_box(
        flex_row((
            label(label_text)
                .font(UI_FONT_STACK)
                .text_size(12.0)
                .color(UI_TEXT_SOFT),
            FlexSpacer::Flex(1.0),
            sized_box(
                text_input(value, move |state: &mut DesktopReader, value| {
                    set_ai_setting(state, field, value);
                })
                .placeholder(placeholder)
                .text_color(UI_TEXT)
                .caret_color(UI_ACCENT)
                .background_color(UI_SURFACE_MUTED)
                .border_color(UI_BORDER)
                .border_width(1.0)
                .corner_radius(RADIUS_SMALL)
                .padding(Padding::from_vh(4.0, 8.0)),
            )
            .width(276.px())
            .height(CONTROL_HEIGHT.px()),
        ))
        .gap(10.px())
        .cross_axis_alignment(CrossAxisAlignment::Center),
    )
    .height(SETTINGS_ROW_HEIGHT.px())
    .expand_width()
    .padding(Padding::horizontal(10.0))
}

fn set_ai_setting(state: &mut DesktopReader, field: AiSettingField, value: String) {
    match field {
        AiSettingField::ProviderName(index) => {
            if let Some(provider) = state.ui.draft_plugin_settings.providers.get_mut(index) {
                provider.name = value;
            }
        }
        AiSettingField::ProviderBaseUrl(index) => {
            if let Some(provider) = state.ui.draft_plugin_settings.providers.get_mut(index) {
                provider.base_url = value;
            }
        }
        AiSettingField::ProviderApiKey(index) => {
            if let Some(provider) = state.ui.draft_plugin_settings.providers.get_mut(index) {
                provider.api_key = value;
            }
        }
        AiSettingField::ProviderModel {
            provider_index,
            model_index,
        } => {
            let updated = state
                .ui
                .draft_plugin_settings
                .providers
                .get_mut(provider_index)
                .and_then(|provider| {
                    let provider_id = provider.id.clone();
                    let model = provider.models.get_mut(model_index)?;
                    let previous = std::mem::replace(model, value.clone());
                    Some((provider_id, previous))
                });
            if let Some((provider_id, previous)) = updated {
                if state.ui.draft_plugin_settings.chat_provider == provider_id
                    && state.ui.draft_plugin_settings.chat_model == previous
                {
                    state.ui.draft_plugin_settings.chat_model.clone_from(&value);
                }
                if state.ui.draft_plugin_settings.translation_provider == provider_id
                    && state.ui.draft_plugin_settings.translation_model == previous
                {
                    state
                        .ui
                        .draft_plugin_settings
                        .translation_model
                        .clone_from(&value);
                }
            }
        }
        AiSettingField::TargetLanguage => {
            state.ui.draft_plugin_settings.target_language = value;
        }
    }
}

fn ai_chat_settings_content(settings: &PluginSettings) -> impl WidgetView<DesktopReader> + use<> {
    portal(
        flex_col((
            settings_section_label("AI Chat 模型"),
            ai_model_choices(settings, AiFeature::Chat),
            prose("AI Chat 会使用这里选中的 Provider 和模型进行书籍问答、检索与解释。")
                .text_size(10.5)
                .text_color(UI_MUTED),
        ))
        .gap(CONTENT_GAP.px())
        .cross_axis_alignment(CrossAxisAlignment::Fill)
        .padding(Padding::from_vh(
            CONTENT_PADDING_VERTICAL,
            CONTENT_PADDING_HORIZONTAL,
        )),
    )
}

fn translation_settings_content(
    settings: &PluginSettings,
) -> impl WidgetView<DesktopReader> + use<> {
    let target_language = settings.target_language.clone();
    let translation_mode = settings.translation_mode;
    portal(
        flex_col((
            settings_section_label("翻译模型"),
            ai_model_choices(settings, AiFeature::Translation),
            settings_section_label("输出"),
            flex_col((
                ai_settings_input_row(
                    "目标语言",
                    target_language,
                    "简体中文",
                    AiSettingField::TargetLanguage,
                ),
                divider(),
                translation_mode_settings_row(translation_mode),
            ))
            .cross_axis_alignment(CrossAxisAlignment::Fill)
            .background_color(UI_SURFACE)
            .border(UI_BORDER, 1.0)
            .corner_radius(RADIUS_MEDIUM),
            prose("点击阅读器顶部的翻译按钮后，会使用这里的模型、目标语言和显示方式翻译正文。")
                .text_size(10.5)
                .text_color(UI_MUTED),
        ))
        .gap(CONTENT_GAP.px())
        .cross_axis_alignment(CrossAxisAlignment::Fill)
        .padding(Padding::from_vh(
            CONTENT_PADDING_VERTICAL,
            CONTENT_PADDING_HORIZONTAL,
        )),
    )
}

fn translation_mode_settings_row(mode: TranslationMode) -> impl WidgetView<DesktopReader> {
    sized_box(
        flex_row((
            label("显示方式").text_size(13.0).color(UI_TEXT_SOFT),
            FlexSpacer::Flex(1.0),
            translation_mode_choice("替换", TranslationMode::Replace, mode),
            translation_mode_choice("双行翻译", TranslationMode::Bilingual, mode),
        ))
        .gap(6.px())
        .cross_axis_alignment(CrossAxisAlignment::Center),
    )
    .height(SETTINGS_ROW_HEIGHT.px())
    .expand_width()
    .padding(Padding::horizontal(12.0))
}

fn translation_mode_choice(
    text: &'static str,
    value: TranslationMode,
    selected: TranslationMode,
) -> impl WidgetView<DesktopReader> {
    let active = value == selected;
    sized_box(
        button(
            label(text)
                .text_size(12.0)
                .weight(if active {
                    FontWeight::BOLD
                } else {
                    FontWeight::NORMAL
                })
                .color(if active { UI_ACCENT } else { UI_TEXT_SOFT }),
            move |state: &mut DesktopReader| {
                state.ui.draft_plugin_settings.translation_mode = value;
            },
        )
        .background_color(if active { UI_ACCENT_SOFT } else { UI_SURFACE })
        .active_background_color(UI_ACCENT_SOFT)
        .border_color(if active { UI_ACCENT_BORDER } else { UI_BORDER })
        .hovered_border_color(UI_ACCENT_BORDER)
        .corner_radius(RADIUS_SMALL)
        .padding(Padding::from_vh(5.0, 9.0)),
    )
    .height(CONTROL_HEIGHT.px())
}

fn ai_model_choices(
    settings: &PluginSettings,
    feature: AiFeature,
) -> Box<AnyWidgetView<DesktopReader>> {
    let choices = settings
        .providers
        .iter()
        .flat_map(|provider| {
            provider.models.iter().filter_map(|model| {
                let model = model.trim();
                (!model.is_empty()).then(|| {
                    ai_model_choice_button(
                        provider.id.clone(),
                        &provider.name,
                        model.to_owned(),
                        feature,
                        match feature {
                            AiFeature::Chat => {
                                settings.chat_provider == provider.id
                                    && settings.chat_model.trim() == model
                            }
                            AiFeature::Translation => {
                                settings.translation_provider == provider.id
                                    && settings.translation_model.trim() == model
                            }
                        },
                    )
                })
            })
        })
        .collect::<Vec<_>>();
    if choices.is_empty() {
        sized_box(
            label("请先在 AI 页面为 Provider 添加模型")
                .font(UI_FONT_STACK)
                .text_size(12.0)
                .color(UI_MUTED),
        )
        .height(SETTINGS_ROW_HEIGHT.px())
        .expand_width()
        .background_color(UI_SURFACE)
        .border(UI_BORDER, 1.0)
        .corner_radius(RADIUS_MEDIUM)
        .padding(Padding::horizontal(12.0))
        .boxed()
    } else {
        flex_col(choices)
            .gap(6.px())
            .cross_axis_alignment(CrossAxisAlignment::Fill)
            .boxed()
    }
}

fn ai_model_choice_button(
    provider_id: String,
    provider_name: &str,
    model: String,
    feature: AiFeature,
    active: bool,
) -> impl WidgetView<DesktopReader> {
    let display = format!(
        "{}  /  {}",
        if provider_name.trim().is_empty() {
            "Provider"
        } else {
            provider_name.trim()
        },
        model
    );
    sized_box(
        button(
            flex_row((
                label(display)
                    .font(UI_FONT_STACK)
                    .text_size(12.0)
                    .weight(if active {
                        FontWeight::BOLD
                    } else {
                        FontWeight::NORMAL
                    })
                    .color(if active { UI_ACCENT } else { UI_TEXT_SOFT }),
                FlexSpacer::Flex(1.0),
                label(if active { "已选择" } else { "选择" })
                    .font(UI_FONT_STACK)
                    .text_size(10.5)
                    .color(if active { UI_ACCENT } else { UI_MUTED }),
            )),
            move |state: &mut DesktopReader| match feature {
                AiFeature::Chat => {
                    state
                        .ui
                        .draft_plugin_settings
                        .chat_provider
                        .clone_from(&provider_id);
                    state.ui.draft_plugin_settings.chat_model.clone_from(&model);
                }
                AiFeature::Translation => {
                    state
                        .ui
                        .draft_plugin_settings
                        .translation_provider
                        .clone_from(&provider_id);
                    state
                        .ui
                        .draft_plugin_settings
                        .translation_model
                        .clone_from(&model);
                }
            },
        )
        .background_color(if active { UI_ACCENT_SOFT } else { UI_SURFACE })
        .active_background_color(UI_ACCENT_SOFT)
        .border_color(if active { UI_ACCENT_BORDER } else { UI_BORDER })
        .hovered_border_color(UI_ACCENT_BORDER)
        .corner_radius(RADIUS_MEDIUM)
        .padding(Padding::horizontal(12.0)),
    )
    .height(40.px())
    .expand_width()
}

fn plugin_settings_content() -> impl WidgetView<DesktopReader> + use<> {
    let plugin_cards = BUILTIN_PLUGINS
        .into_iter()
        .map(|plugin| {
            flex_row((
                icon_label(Icon::Blocks, 15.0, UI_ACCENT),
                flex_col((
                    label(plugin.name)
                        .font(UI_FONT_STACK)
                        .text_size(12.0)
                        .weight(FontWeight::BOLD)
                        .color(UI_TEXT_SOFT),
                    label(plugin.description)
                        .font(UI_FONT_STACK)
                        .text_size(10.5)
                        .color(UI_MUTED),
                ))
                .gap(2.px())
                .cross_axis_alignment(CrossAxisAlignment::Start)
                .flex(1.0),
                value_badge("已启用"),
            ))
            .gap(9.px())
            .cross_axis_alignment(CrossAxisAlignment::Center)
            .padding(Padding::from_vh(7.0, 10.0))
        })
        .collect::<Vec<_>>();
    portal(
        flex_col((
            label("内置插件")
                .font(UI_FONT_STACK)
                .text_size(12.0)
                .weight(FontWeight::BOLD)
                .color(UI_MUTED),
            flex_col(plugin_cards)
                .cross_axis_alignment(CrossAxisAlignment::Fill)
                .background_color(UI_SURFACE)
                .border(UI_BORDER, 1.0)
                .corner_radius(RADIUS_MEDIUM),
        ))
        .gap(CONTENT_GAP.px())
        .cross_axis_alignment(CrossAxisAlignment::Fill)
        .padding(Padding::from_vh(
            CONTENT_PADDING_VERTICAL,
            CONTENT_PADDING_HORIZONTAL,
        )),
    )
}

fn settings_value_row(name: &'static str, value: &'static str) -> impl WidgetView<DesktopReader> {
    sized_box(
        flex_row((
            label(name).text_size(13.0).color(UI_TEXT_SOFT),
            FlexSpacer::Flex(1.0),
            value_badge(value),
        ))
        .cross_axis_alignment(CrossAxisAlignment::Center),
    )
    .height(SETTINGS_ROW_HEIGHT.px())
    .expand_width()
    .padding(Padding::horizontal(12.0))
}

fn spread_settings_row(spread: SpreadMode) -> impl WidgetView<DesktopReader> {
    sized_box(
        flex_row((
            label("分页方式").text_size(13.0).color(UI_TEXT_SOFT),
            FlexSpacer::Flex(1.0),
            spread_choice("单栏", SpreadMode::Single, spread),
            spread_choice("双栏", SpreadMode::Double, spread),
        ))
        .gap(6.px())
        .cross_axis_alignment(CrossAxisAlignment::Center),
    )
    .height(SETTINGS_ROW_HEIGHT.px())
    .expand_width()
    .padding(Padding::horizontal(12.0))
}

fn spread_choice(
    text: &'static str,
    value: SpreadMode,
    selected: SpreadMode,
) -> impl WidgetView<DesktopReader> {
    let active = value == selected;
    sized_box(
        button(
            label(text)
                .text_size(12.0)
                .weight(if active {
                    FontWeight::BOLD
                } else {
                    FontWeight::NORMAL
                })
                .color(if active { UI_ACCENT } else { UI_TEXT_SOFT }),
            move |state: &mut DesktopReader| state.ui.draft_spread = value,
        )
        .background_color(if active { UI_ACCENT_SOFT } else { UI_SURFACE })
        .active_background_color(UI_ACCENT_SOFT)
        .border_color(if active { UI_ACCENT_BORDER } else { UI_BORDER })
        .hovered_border_color(UI_ACCENT_BORDER)
        .corner_radius(RADIUS_SMALL)
        .padding(Padding::from_vh(5.0, 9.0)),
    )
    .width(58.px())
    .height(CONTROL_HEIGHT.px())
}
