use xilem::masonry::properties::PlaceholderColor;
use xilem::masonry::properties::types::{AsUnit, UnitPoint};
use xilem::style::{Padding, Style};
use xilem::view::{
    CrossAxisAlignment, FlexSpacer, ZStackExt, flex_col, flex_row, label, prose, sized_box,
    text_input, zstack,
};
use xilem::{AnyWidgetView, FontWeight, WidgetView};

use crate::preferences::AppLanguage;
use crate::ui::{
    CONTENT_GAP, CONTENT_PADDING_HORIZONTAL, CONTENT_PADDING_VERTICAL, CONTROL_HEIGHT,
    RADIUS_MEDIUM, RADIUS_SMALL, SETTINGS_ROW_HEIGHT, UI_ACCENT, UI_ACCENT_BORDER, UI_BORDER,
    UI_FONT_STACK, UI_MUTED, UI_SURFACE, UI_SURFACE_MUTED, UI_TEXT, UI_TEXT_SOFT, button,
};

use super::SyncSettings;

#[derive(Clone, Copy)]
pub(crate) struct SyncSettingsCallbacks<State> {
    pub(crate) toggle_enabled: fn(&mut State),
    pub(crate) set_base_url: fn(&mut State, String),
    pub(crate) set_username: fn(&mut State, String),
    pub(crate) set_password: fn(&mut State, String),
    pub(crate) set_device_name: fn(&mut State, String),
}

pub(crate) fn sync_settings_content<State: 'static>(
    settings: &SyncSettings,
    password: String,
    has_saved_password: bool,
    language: AppLanguage,
    callbacks: &SyncSettingsCallbacks<State>,
) -> Box<AnyWidgetView<State>> {
    let enabled = settings.enabled;
    let toggle = sized_box(
        button(
            flex_row((
                label(language.text("已启用", "Enabled"))
                    .font(UI_FONT_STACK)
                    .text_size(12.0)
                    .color(if enabled { UI_ACCENT } else { UI_TEXT_SOFT }),
                FlexSpacer::Flex(1.0),
                sized_box(zstack((
                    sized_box(label(""))
                        .width(32.px())
                        .height(18.px())
                        .background_color(if enabled { UI_ACCENT } else { UI_BORDER })
                        .corner_radius(9.0),
                    sized_box(label(""))
                        .width(14.px())
                        .height(14.px())
                        .background_color(UI_SURFACE)
                        .corner_radius(7.0)
                        .alignment(if enabled {
                            UnitPoint::RIGHT
                        } else {
                            UnitPoint::LEFT
                        }),
                )))
                .width(32.px())
                .height(18.px()),
            )),
            callbacks.toggle_enabled,
        )
        .background_color(UI_SURFACE)
        .active_background_color(UI_SURFACE_MUTED)
        .border_color(UI_BORDER)
        .hovered_border_color(UI_ACCENT_BORDER)
        .corner_radius(RADIUS_SMALL)
        .padding(Padding::from_vh(6.0, 10.0)),
    )
    .height(CONTROL_HEIGHT.px())
    .width(276.px());

    flex_col((
        settings_section_label::<State>(language.text("同步", "Sync")),
        prose(language.text(
            "桌面端会直接连接 WebDAV；密码只保存到 Windows 凭据管理器。",
            "Rebook Desktop connects directly to WebDAV. The password is stored only in Windows Credential Manager.",
        ))
        .text_size(10.5)
        .text_color(UI_MUTED),
        flex_col((
            settings_row(language.text("自动同步", "Automatic sync"), toggle.boxed()),
            settings_divider::<State>(),
            settings_input_row(
                language.text("WebDAV 地址", "WebDAV URL"),
                settings.base_url.clone(),
                "https://dav.example.com/path",
                callbacks.set_base_url,
            ),
            settings_divider::<State>(),
            settings_input_row(
                language.text("用户名", "Username"),
                settings.username.clone(),
                language.text("WebDAV 用户名", "WebDAV username"),
                callbacks.set_username,
            ),
            settings_divider::<State>(),
            settings_input_row(
                language.text("密码", "Password"),
                password,
                if has_saved_password {
                    language.text("已保存；留空不修改", "Saved; leave blank to keep it")
                } else {
                    language.text("应用专用密码", "App password")
                },
                callbacks.set_password,
            ),
            settings_divider::<State>(),
            settings_input_row(
                language.text("设备名称", "Device name"),
                settings.device_name.clone(),
                language.text("这台电脑", "This computer"),
                callbacks.set_device_name,
            ),
        ))
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
    ))
    .boxed()
}

fn settings_section_label<State: 'static>(text: &'static str) -> impl WidgetView<State> {
    label(text)
        .font(UI_FONT_STACK)
        .text_size(12.0)
        .weight(FontWeight::BOLD)
        .color(UI_MUTED)
}

fn settings_divider<State: 'static>() -> impl WidgetView<State> {
    sized_box(label(""))
        .height(1.px())
        .expand_width()
        .background_color(UI_BORDER)
}

fn settings_row<State: 'static>(
    label_text: &'static str,
    control: Box<AnyWidgetView<State>>,
) -> impl WidgetView<State> {
    sized_box(
        flex_row((
            label(label_text)
                .font(UI_FONT_STACK)
                .text_size(12.0)
                .color(UI_TEXT_SOFT),
            FlexSpacer::Flex(1.0),
            control,
        ))
        .gap(10.px())
        .cross_axis_alignment(CrossAxisAlignment::Center),
    )
    .height(SETTINGS_ROW_HEIGHT.px())
    .expand_width()
    .padding(Padding::horizontal(10.0))
}

fn settings_input_row<State: 'static>(
    label_text: &'static str,
    value: String,
    placeholder: &'static str,
    callback: fn(&mut State, String),
) -> impl WidgetView<State> {
    let input = sized_box(
        text_input(value, callback)
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
    .height(CONTROL_HEIGHT.px());
    settings_row(label_text, input.boxed())
}
