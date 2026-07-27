//! Reusable in-app modal dialogs.

use std::sync::Arc;

use lucide_icons::Icon;
use xilem::masonry::properties::types::AsUnit;
use xilem::style::{Padding, Style};
use xilem::view::{
    CrossAxisAlignment, FlexExt, FlexSpacer, flex_col, flex_row, label, sized_box, zstack,
};
use xilem::{Color, FontWeight, WidgetView};

use super::button::button;
use super::feedback::{centered_lucide_icon, wrapped_ui_text};
use super::theme::{CONTROL_HEIGHT, RADIUS_DIALOG, RADIUS_MEDIUM, RADIUS_SMALL};
use super::theme::{UI_BORDER, UI_FONT_STACK, UI_MUTED, UI_SURFACE, UI_SURFACE_MUTED, UI_TEXT};

const DIALOG_WIDTH: f64 = 360.0;
const DANGER: Color = Color::from_rgb8(0xb4, 0x23, 0x18);
const DANGER_ACTIVE: Color = Color::from_rgb8(0x91, 0x20, 0x18);
const DANGER_SOFT: Color = Color::from_rgb8(0xfe, 0xf3, 0xf2);

pub(crate) fn confirmation_dialog<State: 'static>(
    title: impl Into<String>,
    message: impl Into<String>,
    cancel_label: impl Into<String>,
    confirm_label: impl Into<String>,
    on_cancel: impl Fn(&mut State) + Send + Sync + 'static,
    on_confirm: impl Fn(&mut State) + Send + Sync + 'static,
) -> impl WidgetView<State> {
    let on_cancel: Arc<dyn Fn(&mut State) + Send + Sync> = Arc::new(on_cancel);
    let on_confirm: Arc<dyn Fn(&mut State) + Send + Sync> = Arc::new(on_confirm);
    let scrim_cancel = Arc::clone(&on_cancel);
    let button_cancel = Arc::clone(&on_cancel);
    let confirm_label = confirm_label.into();
    let cancel_label = cancel_label.into();
    let message = message.into();

    let scrim = sized_box(
        button(label(""), move |state: &mut State| scrim_cancel(state))
            .background_color(Color::TRANSPARENT)
            .active_background_color(Color::TRANSPARENT)
            .border_color(Color::TRANSPARENT)
            .hovered_border_color(Color::TRANSPARENT)
            .border_width(0.0)
            .padding(0.0),
    )
    .expand()
    .background_color(Color::from_rgba8(15, 23, 42, 96));

    let close = sized_box(
        button(
            centered_lucide_icon::<State>(Icon::X, 14.0, UI_MUTED),
            move |state| {
                button_cancel(state);
            },
        )
        .accessibility_label("关闭确认弹窗")
        .background_color(Color::TRANSPARENT)
        .active_background_color(UI_SURFACE_MUTED)
        .border_color(Color::TRANSPARENT)
        .hovered_border_color(Color::TRANSPARENT)
        .border_width(0.0)
        .corner_radius(RADIUS_SMALL)
        .padding(0.0),
    )
    .width(CONTROL_HEIGHT.px())
    .height(CONTROL_HEIGHT.px());

    let cancel_action = Arc::clone(&on_cancel);
    let cancel = sized_box(
        button(
            label(cancel_label)
                .font(UI_FONT_STACK)
                .text_size(12.0)
                .color(UI_TEXT),
            move |state| cancel_action(state),
        )
        .background_color(UI_SURFACE)
        .active_background_color(UI_SURFACE_MUTED)
        .border_color(UI_BORDER)
        .hovered_border_color(UI_MUTED)
        .corner_radius(RADIUS_SMALL)
        .padding(Padding::from_vh(5.0, 12.0)),
    )
    .height(CONTROL_HEIGHT.px());

    let confirm = sized_box(
        button(
            label(confirm_label)
                .font(UI_FONT_STACK)
                .text_size(12.0)
                .weight(FontWeight::BOLD)
                .color(UI_SURFACE),
            move |state| on_confirm(state),
        )
        .background_color(DANGER)
        .active_background_color(DANGER_ACTIVE)
        .border_color(DANGER)
        .hovered_border_color(DANGER_ACTIVE)
        .corner_radius(RADIUS_SMALL)
        .padding(Padding::from_vh(5.0, 12.0)),
    )
    .height(CONTROL_HEIGHT.px());

    let dialog = sized_box(
        flex_col((
            flex_row((
                dialog_alert_icon::<State>(),
                label(title.into())
                    .font(UI_FONT_STACK)
                    .text_size(15.0)
                    .weight(FontWeight::BOLD)
                    .color(UI_TEXT)
                    .flex(1.0),
                close,
            ))
            .gap(10.px())
            .cross_axis_alignment(CrossAxisAlignment::Center),
            wrapped_ui_text::<State>(&message, 52, 12.0, UI_MUTED),
            flex_row((FlexSpacer::Flex(1.0), cancel, confirm))
                .gap(8.px())
                .cross_axis_alignment(CrossAxisAlignment::Center),
        ))
        .gap(16.px())
        .cross_axis_alignment(CrossAxisAlignment::Fill),
    )
    .width(DIALOG_WIDTH.px())
    .background_color(UI_SURFACE)
    .border(UI_BORDER, 1.0)
    .corner_radius(RADIUS_DIALOG)
    .padding(18.0);

    sized_box(zstack((scrim, dialog))).expand()
}

fn dialog_alert_icon<State: 'static>() -> impl WidgetView<State> {
    sized_box(centered_lucide_icon::<State>(
        Icon::AlertTriangle,
        16.0,
        DANGER,
    ))
    .width(34.px())
    .height(34.px())
    .background_color(DANGER_SOFT)
    .corner_radius(RADIUS_MEDIUM)
}
