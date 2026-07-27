use std::borrow::Cow;
use std::collections::HashSet;

use lucide_icons::Icon;
use rebook_layout::{ReaderDefaultFont, ReaderTypography, SpreadMode};
use xilem::masonry::parley::style::FontStack;
use xilem::masonry::properties::PlaceholderColor;
use xilem::masonry::properties::types::{AsUnit, UnitPoint};
use xilem::style::{Padding, Style};
use xilem::view::{
    CrossAxisAlignment, FlexExt, FlexSpacer, MainAxisAlignment, ZStackExt, flex_col, flex_row,
    label, portal, prose, sized_box, text_input, zstack,
};
use xilem::{Affine, AnyWidgetView, Color, FontWeight, WidgetView};

use crate::plugins::{
    AiProvider, BUILTIN_PLUGINS, PluginSettings, TARGET_LANGUAGE_ENGLISH,
    TARGET_LANGUAGE_INTERFACE, TARGET_LANGUAGE_SIMPLIFIED_CHINESE, TranslationMode,
};
use crate::preferences::AppLanguage;
use crate::sync::{SyncSettingsCallbacks, sync_settings_content};
use crate::ui::{
    CONTENT_GAP, CONTENT_PADDING_HORIZONTAL, CONTENT_PADDING_VERTICAL, CONTROL_HEIGHT,
    CONTROL_HEIGHT_COMPACT, DIALOG_FOOTER_HEIGHT, DIALOG_HEADER_HEIGHT, RADIUS_DIALOG,
    RADIUS_LARGE, RADIUS_MEDIUM, RADIUS_SMALL, SETTINGS_ROW_HEIGHT, UI_ACCENT, UI_ACCENT_BORDER,
    UI_ACCENT_SOFT, UI_BORDER, UI_FONT_STACK, UI_MUTED, UI_SURFACE, UI_SURFACE_MUTED, UI_TEXT,
    UI_TEXT_SOFT, button, divider, icon_label,
};

use super::{FontPickerKind, SettingsFeature, SettingsTab};

const SETTINGS_WIDTH: f64 = 660.0;
const SETTINGS_HEIGHT: f64 = 500.0;
const MODAL_SCRIM_ALPHA: f32 = 0.35;

pub(super) fn settings_overlay(
    state: &SettingsFeature,
    progress: f32,
) -> impl WidgetView<SettingsFeature> + use<> {
    // Keep glyphs and one-pixel borders at their native scale throughout the
    // transition. Scaling the complete dialog makes text shimmer as Vello
    // resamples it on every frame, which reads as dropped frames on Windows.
    let offset = 8.0 * f64::from(1.0 - progress);
    let dialog_transform = Affine::translate((0.0, offset));
    sized_box(zstack((
        animated_scrim(modal_scrim_color(progress), SettingsFeature::close_overlay),
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

fn settings_dialog(state: &SettingsFeature) -> impl WidgetView<SettingsFeature> + use<> {
    settings_content(state)
}

fn settings_content(state: &SettingsFeature) -> impl WidgetView<SettingsFeature> + use<> {
    let language = state.draft_language;
    let spread = state.draft_spread;
    let typography = &state.draft_typography;
    let font_picker = state.font_picker;
    let tab = state.settings_tab;
    let title = match tab {
        SettingsTab::General => language.text("通用", "General"),
        SettingsTab::Reading => language.text("阅读", "Reading"),
        SettingsTab::Font => {
            font_picker.map_or(language.text("字体", "Font"), |kind| kind.title(language))
        }
        SettingsTab::Cloud => language.text("云盘", "Cloud drive"),
        SettingsTab::Ai => "AI",
        SettingsTab::AiChat => "AI Chat",
        SettingsTab::Translation => language.text("翻译", "Translation"),
        SettingsTab::Plugins => language.text("插件", "Plugins"),
    };
    let body: Box<AnyWidgetView<SettingsFeature>> = match tab {
        SettingsTab::General => general_settings_content(language).boxed(),
        SettingsTab::Reading => reading_settings_content(spread, language).boxed(),
        SettingsTab::Font => match font_picker {
            Some(kind) => {
                font_picker_content(kind, typography, &state.available_font_families, language)
                    .boxed()
            }
            None => font_settings_content(typography, language).boxed(),
        },
        SettingsTab::Cloud => sync_settings_content(
            &state.draft_sync_settings,
            state.draft_sync_password.clone(),
            !state.applied.sync_password.is_empty(),
            language,
            &SyncSettingsCallbacks {
                toggle_enabled: toggle_sync_enabled,
                set_base_url: set_sync_base_url,
                set_username: set_sync_username,
                set_password: set_sync_password,
                set_device_name: set_sync_device_name,
            },
        ),
        SettingsTab::Ai => {
            ai_settings_content(state.draft_plugin_settings.clone(), language).boxed()
        }
        SettingsTab::AiChat => {
            ai_chat_settings_content(&state.draft_plugin_settings, language).boxed()
        }
        SettingsTab::Translation => {
            translation_settings_content(&state.draft_plugin_settings, language).boxed()
        }
        SettingsTab::Plugins => plugin_settings_content(language).boxed(),
    };

    flex_row((
        settings_sidebar(language, tab),
        flex_col((
            settings_dialog_header(title),
            divider(),
            body.flex(1.0),
            divider(),
            settings_footer(language),
        ))
        .must_fill_major_axis(true)
        .flex(1.0),
    ))
}

fn settings_sidebar(language: AppLanguage, tab: SettingsTab) -> impl WidgetView<SettingsFeature> {
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
                    label(language.text("设置", "Settings"))
                        .font(UI_FONT_STACK)
                        .text_size(15.0)
                        .weight(FontWeight::BOLD)
                        .color(UI_TEXT),
                ))
                .gap(9.px())
                .cross_axis_alignment(CrossAxisAlignment::Center)
                .padding(Padding::from_vh(9.0, 8.0)),
                settings_tab_button(language.text("通用", "General"), SettingsTab::General, tab),
                settings_tab_button(language.text("阅读", "Reading"), SettingsTab::Reading, tab),
                settings_tab_button(language.text("字体", "Font"), SettingsTab::Font, tab),
                settings_tab_button(
                    language.text("云盘", "Cloud drive"),
                    SettingsTab::Cloud,
                    tab,
                ),
                settings_tab_button("AI", SettingsTab::Ai, tab),
                settings_tab_button("AI Chat", SettingsTab::AiChat, tab),
                settings_tab_button(
                    language.text("翻译", "Translation"),
                    SettingsTab::Translation,
                    tab,
                ),
                settings_tab_button(language.text("插件", "Plugins"), SettingsTab::Plugins, tab),
                FlexSpacer::Flex(1.0),
            ))
            .gap(3.px())
            .cross_axis_alignment(CrossAxisAlignment::Fill)
            .padding(8.0),
        )
        .expand(),
    )))
    .width(136.px())
    .expand_height()
}

fn settings_footer(language: AppLanguage) -> impl WidgetView<SettingsFeature> {
    sized_box(
        flex_row((
            FlexSpacer::Flex(1.0),
            secondary_action_button(
                language.text("取消", "Cancel"),
                SettingsFeature::close_overlay,
            ),
            primary_action_button(
                language.text("应用", "Apply"),
                SettingsFeature::apply_settings,
            ),
        ))
        .gap(8.px())
        .cross_axis_alignment(CrossAxisAlignment::Center),
    )
    .height(DIALOG_FOOTER_HEIGHT.px())
    .expand_width()
    .padding(Padding::horizontal(CONTENT_PADDING_HORIZONTAL))
}

fn settings_dialog_header(title: &'static str) -> impl WidgetView<SettingsFeature> {
    sized_box(
        flex_row((
            label(title)
                .font(UI_FONT_STACK)
                .text_size(15.0)
                .weight(FontWeight::BOLD)
                .color(UI_TEXT),
            FlexSpacer::Flex(1.0),
            icon_button(Icon::X, false, SettingsFeature::close_overlay),
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
) -> impl WidgetView<SettingsFeature> {
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
            move |state: &mut SettingsFeature| {
                state.settings_tab = value;
                state.font_picker = None;
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

fn general_settings_content(language: AppLanguage) -> impl WidgetView<SettingsFeature> {
    flex_col((
        settings_section_label(language.text("语言与地区", "Language & region")),
        flex_col((language_settings_row(language),))
            .cross_axis_alignment(CrossAxisAlignment::Fill)
            .background_color(UI_SURFACE)
            .border(UI_BORDER, 1.0)
            .corner_radius(RADIUS_MEDIUM),
        prose(language.text(
            "界面语言也会作为翻译目标语言的默认值。你仍可在“翻译”中选择固定语言。",
            "The interface language is also the default translation target. You can still choose a fixed language under Translation.",
        ))
        .text_size(10.5)
        .text_color(UI_MUTED),
    ))
    .gap(CONTENT_GAP.px())
    .cross_axis_alignment(CrossAxisAlignment::Fill)
    .main_axis_alignment(MainAxisAlignment::Start)
    .padding(Padding::from_vh(
        CONTENT_PADDING_VERTICAL,
        CONTENT_PADDING_HORIZONTAL,
    ))
}

fn language_settings_row(language: AppLanguage) -> impl WidgetView<SettingsFeature> {
    sized_box(
        flex_row((
            label(language.text("界面语言", "Interface language"))
                .text_size(13.0)
                .color(UI_TEXT_SOFT),
            FlexSpacer::Flex(1.0),
            language_choice("简体中文", AppLanguage::SimplifiedChinese, language),
            language_choice("English", AppLanguage::English, language),
        ))
        .gap(6.px())
        .cross_axis_alignment(CrossAxisAlignment::Center),
    )
    .height(SETTINGS_ROW_HEIGHT.px())
    .expand_width()
    .padding(Padding::horizontal(12.0))
}

fn language_choice(
    text: &'static str,
    value: AppLanguage,
    selected: AppLanguage,
) -> impl WidgetView<SettingsFeature> {
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
            move |state: &mut SettingsFeature| state.draft_language = value,
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

fn toggle_sync_enabled(state: &mut SettingsFeature) {
    state.draft_sync_settings.enabled = !state.draft_sync_settings.enabled;
}

fn set_sync_base_url(state: &mut SettingsFeature, value: String) {
    state.draft_sync_settings.base_url = value;
}

fn set_sync_username(state: &mut SettingsFeature, value: String) {
    state.draft_sync_settings.username = value;
}

fn set_sync_password(state: &mut SettingsFeature, value: String) {
    state.draft_sync_password = value;
}

fn set_sync_device_name(state: &mut SettingsFeature, value: String) {
    state.draft_sync_settings.device_name = value;
}

fn reading_settings_content(
    spread: SpreadMode,
    language: AppLanguage,
) -> impl WidgetView<SettingsFeature> {
    flex_col((
        label(language.text("页面布局", "Page layout"))
            .font(UI_FONT_STACK)
            .text_size(12.0)
            .weight(FontWeight::BOLD)
            .color(UI_MUTED),
        sized_box(flex_col((
            settings_value_row(
                language.text("阅读模式", "Reading mode"),
                language.text("分页", "Paginated"),
            ),
            divider(),
            spread_settings_row(spread, language),
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

fn font_settings_content(
    typography: &ReaderTypography,
    language: AppLanguage,
) -> impl WidgetView<SettingsFeature> + use<> {
    let preview_font = typography.default_stack();
    let preview_size = typography.font_size.min(24.0);
    let preview_weight = FontWeight::new(f32::from(typography.font_weight));
    let default_font = typography.default_font;
    let font_size = typography.font_size;
    let minimum_font_size = typography.minimum_font_size;
    let font_weight = typography.font_weight;

    portal(
        flex_col((
            settings_section_label(language.text("字号与字重", "Size & weight")),
            typography_metrics_card(font_size, minimum_font_size, font_weight, language),
            settings_section_label(language.text("字体", "Font")),
            flex_col((
                default_font_row(default_font, language),
                divider(),
                font_family_settings_row(
                    language.text("中文字体", "CJK font"),
                    typography.default_cjk_font.clone(),
                    FontPickerKind::Cjk,
                ),
            ))
            .cross_axis_alignment(CrossAxisAlignment::Fill)
            .background_color(UI_SURFACE)
            .border(UI_BORDER, 1.0)
            .corner_radius(RADIUS_MEDIUM),
            settings_section_label(language.text("字型", "Font families")),
            flex_col((
                font_family_settings_row(
                    language.text("衬线字体", "Serif font"),
                    typography.serif_font.clone(),
                    FontPickerKind::Serif,
                ),
                divider(),
                font_family_settings_row(
                    language.text("无衬线字体", "Sans-serif font"),
                    typography.sans_serif_font.clone(),
                    FontPickerKind::SansSerif,
                ),
                divider(),
                font_family_settings_row(
                    language.text("等宽字体", "Monospace font"),
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
                    label(language.text("字体预览", "Font preview"))
                        .font(UI_FONT_STACK)
                        .text_size(11.0)
                        .color(UI_MUTED),
                    label(language.text(
                        "阅读让思想抵达更远的地方 Reading 0123",
                        "Reading carries ideas farther 阅读 0123",
                    ))
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
    language: AppLanguage,
) -> impl WidgetView<SettingsFeature> {
    flex_col((
        typography_stepper_row(
            language.text("默认字号", "Default size"),
            format!("{font_size:.0} px"),
            |state: &mut SettingsFeature| {
                let minimum = state.draft_typography.minimum_font_size;
                state.draft_typography.font_size =
                    (state.draft_typography.font_size - 1.0).max(minimum);
            },
            |state: &mut SettingsFeature| {
                state.draft_typography.font_size =
                    (state.draft_typography.font_size + 1.0).min(120.0);
            },
        ),
        divider(),
        typography_stepper_row(
            language.text("最小字号", "Minimum size"),
            format!("{minimum_font_size:.0} px"),
            |state: &mut SettingsFeature| {
                state.draft_typography.minimum_font_size =
                    (state.draft_typography.minimum_font_size - 1.0).max(1.0);
            },
            |state: &mut SettingsFeature| {
                let typography = &mut state.draft_typography;
                typography.minimum_font_size = (typography.minimum_font_size + 1.0).min(120.0);
                typography.font_size = typography.font_size.max(typography.minimum_font_size);
            },
        ),
        divider(),
        typography_stepper_row(
            language.text("字体粗细", "Font weight"),
            font_weight.to_string(),
            |state: &mut SettingsFeature| {
                state.draft_typography.font_weight = state
                    .draft_typography
                    .font_weight
                    .saturating_sub(100)
                    .max(100);
            },
            |state: &mut SettingsFeature| {
                state.draft_typography.font_weight = state
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

fn settings_section_label(text: &'static str) -> impl WidgetView<SettingsFeature> {
    label(text)
        .font(UI_FONT_STACK)
        .text_size(12.0)
        .weight(FontWeight::BOLD)
        .color(UI_MUTED)
}

fn typography_stepper_row(
    name: &'static str,
    value: String,
    decrease: impl Fn(&mut SettingsFeature) + Send + Sync + 'static,
    increase: impl Fn(&mut SettingsFeature) + Send + Sync + 'static,
) -> impl WidgetView<SettingsFeature> {
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
    callback: impl Fn(&mut SettingsFeature) + Send + Sync + 'static,
) -> impl WidgetView<SettingsFeature> {
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

fn default_font_row(
    selected: ReaderDefaultFont,
    language: AppLanguage,
) -> impl WidgetView<SettingsFeature> {
    sized_box(
        flex_row((
            label(language.text("默认字体", "Default font"))
                .font(UI_FONT_STACK)
                .text_size(13.0)
                .color(UI_TEXT_SOFT),
            FlexSpacer::Flex(1.0),
            default_font_choice(
                language.text("衬线", "Serif"),
                ReaderDefaultFont::Serif,
                selected,
            ),
            default_font_choice(
                language.text("无衬线", "Sans serif"),
                ReaderDefaultFont::SansSerif,
                selected,
            ),
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
) -> impl WidgetView<SettingsFeature> {
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
            move |state: &mut SettingsFeature| state.draft_typography.default_font = value,
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
) -> impl WidgetView<SettingsFeature> {
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
            move |state: &mut SettingsFeature| state.font_picker = Some(picker),
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
    language: AppLanguage,
) -> impl WidgetView<SettingsFeature> + use<> {
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
                        label(language.text("返回字体设置", "Back to font settings"))
                            .font(UI_FONT_STACK)
                            .text_size(12.0)
                            .color(UI_TEXT_SOFT),
                    ))
                    .gap(6.px())
                    .cross_axis_alignment(CrossAxisAlignment::Center),
                    |state: &mut SettingsFeature| state.font_picker = None,
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
) -> impl WidgetView<SettingsFeature> + use<> {
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
            move |state: &mut SettingsFeature| {
                match kind {
                    FontPickerKind::Cjk => {
                        state.draft_typography.default_cjk_font.clone_from(&family);
                    }
                    FontPickerKind::Serif => {
                        state.draft_typography.serif_font.clone_from(&family);
                    }
                    FontPickerKind::SansSerif => {
                        state.draft_typography.sans_serif_font.clone_from(&family);
                    }
                    FontPickerKind::Monospace => {
                        state.draft_typography.monospace_font.clone_from(&family);
                    }
                }
                state.font_picker = None;
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
    Name(usize),
    BaseUrl(usize),
    ApiKey(usize),
    Model {
        provider_index: usize,
        model_index: usize,
    },
}

#[derive(Clone, Copy)]
enum AiFeature {
    Chat,
    Translation,
}

fn ai_settings_content(
    settings: PluginSettings,
    language: AppLanguage,
) -> impl WidgetView<SettingsFeature> + use<> {
    let provider_count = settings.providers.len();
    let provider_cards = settings
        .providers
        .into_iter()
        .enumerate()
        .map(|(index, provider)| ai_provider_card(index, provider, provider_count > 1, language))
        .collect::<Vec<_>>();
    portal(
        flex_col((
            settings_section_label("Providers"),
            flex_col(provider_cards)
                .gap(CONTENT_GAP.px())
                .cross_axis_alignment(CrossAxisAlignment::Fill),
            secondary_action_button(
                language.text("新增 Provider", "Add provider"),
                |state: &mut SettingsFeature| {
                    state.draft_plugin_settings.add_provider();
                },
            ),
            prose(language.text(
                "每个 Provider 可以维护多个模型。API Key 只保存在当前运行内存中，不会写入 plugins.json；默认 Provider 也可以通过 REBOOK_AI_API_KEY 环境变量提供密钥。",
                "Each provider can contain multiple models. API keys are kept only in memory and are never written to plugins.json. The default provider can also read REBOOK_AI_API_KEY.",
            ))
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
    language: AppLanguage,
) -> impl WidgetView<SettingsFeature> {
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
            ai_provider_model_row(index, model_index, model, model_count > 1, language)
        })
        .collect::<Vec<_>>();
    let remove_provider: Box<AnyWidgetView<SettingsFeature>> = if can_remove_provider {
        icon_button(Icon::Trash2, false, move |state: &mut SettingsFeature| {
            state.draft_plugin_settings.remove_provider(index);
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
            language.text("名称", "Name"),
            name,
            language.text("例如 OpenAI、Ollama", "For example OpenAI or Ollama"),
            AiSettingField::Name(index),
        ),
        divider(),
        ai_settings_input_row(
            language.text("API 地址", "API URL"),
            base_url,
            "https://api.openai.com/v1",
            AiSettingField::BaseUrl(index),
        ),
        divider(),
        ai_settings_input_row(
            language.text("API Key（仅本次会话）", "API key (this session only)"),
            api_key,
            "sk-…",
            AiSettingField::ApiKey(index),
        ),
        divider(),
        flex_col((
            label(language.text("模型", "Models"))
                .font(UI_FONT_STACK)
                .text_size(11.5)
                .weight(FontWeight::BOLD)
                .color(UI_MUTED),
            flex_col(model_rows).cross_axis_alignment(CrossAxisAlignment::Fill),
            secondary_action_button(
                language.text("新增模型", "Add model"),
                move |state: &mut SettingsFeature| {
                    if let Some(provider) = state.draft_plugin_settings.providers.get_mut(index) {
                        provider.models.push(String::new());
                    }
                },
            ),
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
    language: AppLanguage,
) -> impl WidgetView<SettingsFeature> {
    let remove: Box<AnyWidgetView<SettingsFeature>> = if can_remove {
        icon_button(Icon::X, false, move |state: &mut SettingsFeature| {
            state
                .draft_plugin_settings
                .remove_model(provider_index, model_index);
        })
        .boxed()
    } else {
        sized_box(label("")).width(32.px()).height(32.px()).boxed()
    };
    sized_box(
        flex_row((
            label(match language {
                AppLanguage::SimplifiedChinese => format!("模型 {}", model_index + 1),
                AppLanguage::English => format!("Model {}", model_index + 1),
            })
            .font(UI_FONT_STACK)
            .text_size(11.5)
            .color(UI_TEXT_SOFT),
            FlexSpacer::Flex(1.0),
            sized_box(
                text_input(value, move |state: &mut SettingsFeature, value| {
                    set_ai_setting(
                        state,
                        AiSettingField::Model {
                            provider_index,
                            model_index,
                        },
                        value,
                    );
                })
                .placeholder(language.text(
                    "模型 ID，例如 gpt-4o-mini",
                    "Model ID, for example gpt-4o-mini",
                ))
                .text_color(UI_TEXT)
                .caret_color(UI_ACCENT)
                .prop(PlaceholderColor::new(UI_MUTED))
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
) -> impl WidgetView<SettingsFeature> {
    sized_box(
        flex_row((
            label(label_text)
                .font(UI_FONT_STACK)
                .text_size(12.0)
                .color(UI_TEXT_SOFT),
            FlexSpacer::Flex(1.0),
            sized_box(
                text_input(value, move |state: &mut SettingsFeature, value| {
                    set_ai_setting(state, field, value);
                })
                .placeholder(placeholder)
                .text_color(UI_TEXT)
                .caret_color(UI_ACCENT)
                .prop(PlaceholderColor::new(UI_MUTED))
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

fn set_ai_setting(state: &mut SettingsFeature, field: AiSettingField, value: String) {
    match field {
        AiSettingField::Name(index) => {
            if let Some(provider) = state.draft_plugin_settings.providers.get_mut(index) {
                provider.name = value;
            }
        }
        AiSettingField::BaseUrl(index) => {
            if let Some(provider) = state.draft_plugin_settings.providers.get_mut(index) {
                provider.base_url = value;
            }
        }
        AiSettingField::ApiKey(index) => {
            if let Some(provider) = state.draft_plugin_settings.providers.get_mut(index) {
                provider.api_key = value;
            }
        }
        AiSettingField::Model {
            provider_index,
            model_index,
        } => {
            let updated = state
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
                if state.draft_plugin_settings.chat_provider == provider_id
                    && state.draft_plugin_settings.chat_model == previous
                {
                    state.draft_plugin_settings.chat_model.clone_from(&value);
                }
                if state.draft_plugin_settings.translation_provider == provider_id
                    && state.draft_plugin_settings.translation_model == previous
                {
                    state
                        .draft_plugin_settings
                        .translation_model
                        .clone_from(&value);
                }
            }
        }
    }
}

fn ai_chat_settings_content(
    settings: &PluginSettings,
    language: AppLanguage,
) -> impl WidgetView<SettingsFeature> + use<'_> {
    portal(
        flex_col((
            settings_section_label(language.text("AI Chat 模型", "AI Chat model")),
            ai_model_choices(settings, AiFeature::Chat, language),
            prose(language.text(
                "AI Chat 会使用这里选中的 Provider 和模型进行书籍问答、检索与解释。",
                "AI Chat uses the selected provider and model for book Q&A, search, and explanations.",
            ))
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
    language: AppLanguage,
) -> impl WidgetView<SettingsFeature> + use<'_> {
    let translation_mode = settings.translation_mode;
    portal(
        flex_col((
            settings_section_label(language.text("翻译模型", "Translation model")),
            ai_model_choices(settings, AiFeature::Translation, language),
            settings_section_label(language.text("输出", "Output")),
            flex_col((
                translation_target_settings_row(&settings.target_language, language),
                divider(),
                translation_mode_settings_row(translation_mode, language),
            ))
            .cross_axis_alignment(CrossAxisAlignment::Fill)
            .background_color(UI_SURFACE)
            .border(UI_BORDER, 1.0)
            .corner_radius(RADIUS_MEDIUM),
            prose(language.text(
                "点击阅读器顶部的翻译按钮后，会使用这里的模型、目标语言和显示方式翻译正文。",
                "Use the Translate button in the reader toolbar to translate the book with this model, target language, and display mode.",
            ))
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

fn translation_target_settings_row(
    target_language: &str,
    language: AppLanguage,
) -> impl WidgetView<SettingsFeature> {
    sized_box(
        flex_row((
            label(language.text("目标语言", "Target language"))
                .text_size(13.0)
                .color(UI_TEXT_SOFT),
            FlexSpacer::Flex(1.0),
            translation_target_choice(
                language.text("跟随界面", "Interface"),
                TARGET_LANGUAGE_INTERFACE,
                target_language,
            ),
            translation_target_choice(
                "简体中文",
                TARGET_LANGUAGE_SIMPLIFIED_CHINESE,
                target_language,
            ),
            translation_target_choice("English", TARGET_LANGUAGE_ENGLISH, target_language),
        ))
        .gap(6.px())
        .cross_axis_alignment(CrossAxisAlignment::Center),
    )
    .height(SETTINGS_ROW_HEIGHT.px())
    .expand_width()
    .padding(Padding::horizontal(12.0))
}

fn translation_target_choice(
    text: &'static str,
    value: &'static str,
    selected: &str,
) -> impl WidgetView<SettingsFeature> {
    let active = value == selected;
    sized_box(
        button(
            label(text)
                .text_size(11.5)
                .weight(if active {
                    FontWeight::BOLD
                } else {
                    FontWeight::NORMAL
                })
                .color(if active { UI_ACCENT } else { UI_TEXT_SOFT }),
            move |state: &mut SettingsFeature| {
                state.draft_plugin_settings.target_language = value.into();
            },
        )
        .background_color(if active { UI_ACCENT_SOFT } else { UI_SURFACE })
        .active_background_color(UI_ACCENT_SOFT)
        .border_color(if active { UI_ACCENT_BORDER } else { UI_BORDER })
        .hovered_border_color(UI_ACCENT_BORDER)
        .corner_radius(RADIUS_SMALL)
        .padding(Padding::from_vh(5.0, 8.0)),
    )
    .height(CONTROL_HEIGHT.px())
}

fn translation_mode_settings_row(
    mode: TranslationMode,
    language: AppLanguage,
) -> impl WidgetView<SettingsFeature> {
    sized_box(
        flex_row((
            label(language.text("显示方式", "Display mode"))
                .text_size(13.0)
                .color(UI_TEXT_SOFT),
            FlexSpacer::Flex(1.0),
            translation_mode_choice(
                language.text("替换", "Replace"),
                TranslationMode::Replace,
                mode,
            ),
            translation_mode_choice(
                language.text("双行翻译", "Bilingual"),
                TranslationMode::Bilingual,
                mode,
            ),
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
) -> impl WidgetView<SettingsFeature> {
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
            move |state: &mut SettingsFeature| {
                state.draft_plugin_settings.translation_mode = value;
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
    language: AppLanguage,
) -> Box<AnyWidgetView<SettingsFeature>> {
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
                        language,
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
            label(language.text(
                "请先在 AI 页面为 Provider 添加模型",
                "Add a model to a provider on the AI page first",
            ))
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
    language: AppLanguage,
    active: bool,
) -> impl WidgetView<SettingsFeature> {
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
                label(if active {
                    language.text("已选择", "Selected")
                } else {
                    language.text("选择", "Select")
                })
                .font(UI_FONT_STACK)
                .text_size(10.5)
                .color(if active { UI_ACCENT } else { UI_MUTED }),
            )),
            move |state: &mut SettingsFeature| match feature {
                AiFeature::Chat => {
                    state
                        .draft_plugin_settings
                        .chat_provider
                        .clone_from(&provider_id);
                    state.draft_plugin_settings.chat_model.clone_from(&model);
                }
                AiFeature::Translation => {
                    state
                        .draft_plugin_settings
                        .translation_provider
                        .clone_from(&provider_id);
                    state
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

fn plugin_settings_content(language: AppLanguage) -> impl WidgetView<SettingsFeature> + use<> {
    let plugin_cards = BUILTIN_PLUGINS
        .into_iter()
        .map(|plugin| {
            flex_row((
                icon_label(Icon::Blocks, 15.0, UI_ACCENT),
                flex_col((
                    label(match (language, plugin.id) {
                        (AppLanguage::English, "rebook.search") => "Full-text search",
                        (AppLanguage::English, "rebook.ai-chat") => "AI Chat",
                        (AppLanguage::English, "rebook.translation") => "Translation",
                        _ => plugin.name,
                    })
                    .font(UI_FONT_STACK)
                    .text_size(12.0)
                    .weight(FontWeight::BOLD)
                    .color(UI_TEXT_SOFT),
                    label(match (language, plugin.id) {
                        (AppLanguage::English, "rebook.search") => {
                            "Search book content and jump to the source"
                        }
                        (AppLanguage::English, "rebook.ai-chat") => {
                            "Search, explain, and answer questions about the book"
                        }
                        (AppLanguage::English, "rebook.translation") => {
                            "Translate book content while preserving source anchors"
                        }
                        _ => plugin.description,
                    })
                    .font(UI_FONT_STACK)
                    .text_size(10.5)
                    .color(UI_MUTED),
                ))
                .gap(2.px())
                .cross_axis_alignment(CrossAxisAlignment::Start)
                .flex(1.0),
                value_badge(language.text("已启用", "Enabled")),
            ))
            .gap(9.px())
            .cross_axis_alignment(CrossAxisAlignment::Center)
            .padding(Padding::from_vh(7.0, 10.0))
        })
        .collect::<Vec<_>>();
    portal(
        flex_col((
            label(language.text("内置插件", "Built-in plugins"))
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

fn settings_value_row(name: &'static str, value: &'static str) -> impl WidgetView<SettingsFeature> {
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

fn spread_settings_row(
    spread: SpreadMode,
    language: AppLanguage,
) -> impl WidgetView<SettingsFeature> {
    sized_box(
        flex_row((
            label(language.text("分页方式", "Page spread"))
                .text_size(13.0)
                .color(UI_TEXT_SOFT),
            FlexSpacer::Flex(1.0),
            spread_choice(language.text("单栏", "Single"), SpreadMode::Single, spread),
            spread_choice(language.text("双栏", "Double"), SpreadMode::Double, spread),
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
) -> impl WidgetView<SettingsFeature> {
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
            move |state: &mut SettingsFeature| state.draft_spread = value,
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

fn icon_button(
    icon: Icon,
    selected: bool,
    callback: impl Fn(&mut SettingsFeature) + Send + Sync + 'static,
) -> impl WidgetView<SettingsFeature> {
    let background = if selected {
        UI_SURFACE_MUTED
    } else {
        Color::TRANSPARENT
    };
    sized_box(
        button(
            icon_label(icon, 16.0, if selected { UI_TEXT } else { UI_MUTED }),
            callback,
        )
        .background_color(background)
        .active_background_color(UI_SURFACE_MUTED)
        .border_color(Color::TRANSPARENT)
        .hovered_border_color(Color::TRANSPARENT)
        .border_width(0.0)
        .corner_radius(8.0)
        .padding(0.0),
    )
    .width(32.px())
    .height(32.px())
}

fn value_badge(text: &'static str) -> impl WidgetView<SettingsFeature> {
    sized_box(label(text).text_size(12.0).color(UI_TEXT_SOFT))
        .height(CONTROL_HEIGHT.px())
        .background_color(UI_SURFACE)
        .border(UI_BORDER, 1.0)
        .corner_radius(RADIUS_SMALL)
        .padding(Padding::from_vh(5.0, 10.0))
}

fn primary_action_button(
    text: &'static str,
    callback: impl Fn(&mut SettingsFeature) + Send + Sync + 'static,
) -> impl WidgetView<SettingsFeature> {
    sized_box(
        button(
            label(text)
                .text_size(12.5)
                .weight(FontWeight::BOLD)
                .color(UI_SURFACE),
            callback,
        )
        .background_color(UI_ACCENT)
        .active_background_color(UI_TEXT)
        .border_color(UI_ACCENT)
        .corner_radius(RADIUS_SMALL)
        .padding(Padding::from_vh(5.0, 12.0)),
    )
    .height(CONTROL_HEIGHT.px())
}

fn secondary_action_button(
    text: &'static str,
    callback: impl Fn(&mut SettingsFeature) + Send + Sync + 'static,
) -> impl WidgetView<SettingsFeature> {
    sized_box(
        button(label(text).text_size(12.5).color(UI_TEXT_SOFT), callback)
            .background_color(UI_SURFACE)
            .active_background_color(UI_SURFACE_MUTED)
            .border_color(UI_SURFACE)
            .hovered_border_color(UI_BORDER)
            .corner_radius(RADIUS_SMALL)
            .padding(Padding::from_vh(5.0, 10.0)),
    )
    .height(CONTROL_HEIGHT.px())
}

fn animated_scrim(
    color: Color,
    callback: impl Fn(&mut SettingsFeature) + Send + Sync + 'static,
) -> impl WidgetView<SettingsFeature> {
    sized_box(
        button(label(""), callback)
            .background_color(Color::TRANSPARENT)
            .active_background_color(Color::TRANSPARENT)
            .border_color(Color::TRANSPARENT)
            .hovered_border_color(Color::TRANSPARENT)
            .border_width(0.0)
            .padding(0.0),
    )
    .expand()
    .background_color(color)
}

fn modal_scrim_color(progress: f32) -> Color {
    Color::from_rgb8(0x1f, 0x2d, 0x3d).with_alpha(MODAL_SCRIM_ALPHA * progress)
}
