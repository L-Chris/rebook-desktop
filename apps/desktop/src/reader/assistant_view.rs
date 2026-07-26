use lucide_icons::Icon;
use xilem::masonry::properties::types::{AsUnit, UnitPoint};
use xilem::style::{Padding, Style};
use xilem::view::{
    CrossAxisAlignment, FlexExt, FlexSpacer, MainAxisAlignment, ZStackExt, flex_col, flex_row,
    label, portal, prose, sized_box, text_input, zstack,
};
use xilem::{AnyWidgetView, Color, FontWeight, WidgetView};

use crate::plugins::{ChatCommand, ChatRole, ChatTurn, chat_command_suggestions};
use crate::ui::{
    NoticeTone, RADIUS_MEDIUM, UI_ACCENT, UI_ACCENT_BORDER, UI_ACCENT_SOFT, UI_BORDER,
    UI_FONT_STACK, UI_MUTED, UI_SIDEBAR, UI_SURFACE, UI_TEXT, UI_TEXT_SOFT, button, divider,
    icon_label, notice_card,
};

use super::DesktopReader;
use super::view::icon_button;

const ASSISTANT_PANEL_WIDTH: f64 = 340.0;

pub(super) fn assistant_panel(state: &DesktopReader) -> impl WidgetView<DesktopReader> + use<> {
    chat_panel(state)
}

fn chat_panel(state: &DesktopReader) -> impl WidgetView<DesktopReader> + use<> {
    let input = state.chat.input.clone();
    let busy = state.chat.task.is_pending();
    let mut rows = state
        .chat
        .messages
        .iter()
        .cloned()
        .map(|turn| chat_message_row(turn).boxed())
        .collect::<Vec<_>>();
    if busy {
        rows.push(
            chat_message_row(ChatTurn {
                role: ChatRole::Assistant,
                content: "正在阅读和检索书籍…".into(),
                display_content: None,
            })
            .boxed(),
        );
    }
    let conversation: Box<AnyWidgetView<DesktopReader>> = if rows.is_empty() {
        flex_col((
            icon_label(Icon::MessageCircle, 28.0, UI_MUTED),
            label("围绕当前书籍提问")
                .font(UI_FONT_STACK)
                .text_size(13.0)
                .weight(FontWeight::BOLD)
                .color(UI_TEXT_SOFT),
            prose("可以总结章节、解释选中的段落，或让 AI 搜索书中的概念。")
                .text_size(12.0)
                .text_color(UI_MUTED),
        ))
        .gap(8.px())
        .cross_axis_alignment(CrossAxisAlignment::Center)
        .main_axis_alignment(MainAxisAlignment::Center)
        .padding(24.0)
        .boxed()
    } else {
        portal(
            flex_col(rows)
                .gap(10.px())
                .cross_axis_alignment(CrossAxisAlignment::Fill)
                .padding(12.0),
        )
        .boxed()
    };
    let error: Box<AnyWidgetView<DesktopReader>> = state.chat.error.as_ref().map_or_else(
        || sized_box(label("")).width(0.px()).height(0.px()).boxed(),
        |error| notice_card(NoticeTone::Error, "AI 请求失败", error.clone()).boxed(),
    );
    let command_menu = chat_command_menu(&input, busy);
    let composer = chat_composer(input, busy);
    let conversation_layer = flex_col((
        assistant_panel_header(Icon::MessageCircle, "AI 对话", true),
        divider(),
        conversation.flex(1.0),
        sized_box(label("")).height(48.px()),
    ))
    .must_fill_major_axis(true)
    .gap(8.px())
    .cross_axis_alignment(CrossAxisAlignment::Fill)
    .alignment(UnitPoint::TOP_LEFT);
    let composer_layer = sized_box(
        flex_col((error, command_menu, composer))
            .gap(8.px())
            .cross_axis_alignment(CrossAxisAlignment::Fill),
    )
    .expand_width()
    .alignment(UnitPoint::BOTTOM);

    sized_box(zstack((
        sized_box(label("")).expand(),
        conversation_layer,
        composer_layer,
    )))
    .width(ASSISTANT_PANEL_WIDTH.px())
    .expand_height()
    .background_color(UI_SIDEBAR)
    .border(UI_BORDER, 1.0)
    .padding(Padding::from_vh(6.0, 10.0))
}

fn chat_command_menu(input: &str, busy: bool) -> Box<AnyWidgetView<DesktopReader>> {
    let command_suggestions = chat_command_suggestions(input);
    if command_suggestions.is_empty() || busy {
        return sized_box(label("")).width(0.px()).height(0.px()).boxed();
    }
    let rows = command_suggestions
        .into_iter()
        .map(|command| chat_command_suggestion_row(command).boxed())
        .collect::<Vec<_>>();
    flex_col(rows)
        .cross_axis_alignment(CrossAxisAlignment::Fill)
        .background_color(UI_SURFACE)
        .border(UI_BORDER, 1.0)
        .corner_radius(RADIUS_MEDIUM)
        .padding(4.0)
        .boxed()
}

fn chat_composer(input: String, busy: bool) -> impl WidgetView<DesktopReader> + use<> {
    sized_box(
        flex_row((
            text_input(input, |state: &mut DesktopReader, value| {
                state.chat.input = value;
            })
            .on_enter(|state: &mut DesktopReader, value| {
                state.chat.input = value;
                state.send_chat();
            })
            .placeholder("询问这本书，或输入 / 使用技能…")
            .text_color(UI_TEXT)
            .caret_color(UI_ACCENT)
            .background_color(Color::TRANSPARENT)
            .border_color(Color::TRANSPARENT)
            .border_width(0.0)
            .padding(0.0)
            .flex(1.0),
            icon_button(Icon::Send, busy, DesktopReader::send_chat),
        ))
        .gap(6.px())
        .cross_axis_alignment(CrossAxisAlignment::Center),
    )
    .height(40.px())
    .background_color(UI_SURFACE)
    .border(UI_BORDER, 1.0)
    .corner_radius(RADIUS_MEDIUM)
    .padding(Padding::horizontal(8.0))
}

fn chat_message_row(turn: ChatTurn) -> impl WidgetView<DesktopReader> + use<> {
    let (role, background, border, label_color) = match turn.role {
        ChatRole::User => ("你", UI_ACCENT_SOFT, UI_ACCENT_BORDER, UI_ACCENT),
        ChatRole::Assistant => ("Rebook AI", UI_SURFACE, UI_BORDER, UI_MUTED),
    };
    let content = turn.display_content.unwrap_or(turn.content);
    sized_box(
        flex_col((
            label(role)
                .font(UI_FONT_STACK)
                .text_size(10.5)
                .weight(FontWeight::BOLD)
                .color(label_color),
            prose(content).text_size(12.5).text_color(UI_TEXT_SOFT),
        ))
        .gap(5.px())
        .cross_axis_alignment(CrossAxisAlignment::Start),
    )
    .expand_width()
    .background_color(background)
    .border(border, 1.0)
    .corner_radius(RADIUS_MEDIUM)
    .padding(Padding::from_vh(9.0, 10.0))
}

fn chat_command_suggestion_row(command: ChatCommand) -> impl WidgetView<DesktopReader> + use<> {
    sized_box(
        button(
            flex_row((
                label(command.name)
                    .font(UI_FONT_STACK)
                    .text_size(12.0)
                    .weight(FontWeight::BOLD)
                    .color(UI_ACCENT),
                label(command.description)
                    .font(UI_FONT_STACK)
                    .text_size(11.0)
                    .color(UI_MUTED)
                    .flex(1.0),
            ))
            .gap(9.px())
            .cross_axis_alignment(CrossAxisAlignment::Center),
            move |state: &mut DesktopReader| state.select_chat_command(command),
        )
        .background_color(Color::TRANSPARENT)
        .active_background_color(UI_ACCENT_SOFT)
        .border_color(Color::TRANSPARENT)
        .hovered_border_color(UI_ACCENT_BORDER)
        .border_width(1.0)
        .corner_radius(7.0)
        .padding(Padding::horizontal(8.0)),
    )
    .height(34.px())
    .expand_width()
}

fn assistant_panel_header(
    icon: Icon,
    title: &'static str,
    clearable: bool,
) -> impl WidgetView<DesktopReader> {
    let clear: Box<AnyWidgetView<DesktopReader>> = if clearable {
        icon_button(Icon::Trash2, false, DesktopReader::clear_chat).boxed()
    } else {
        sized_box(label("")).width(0.px()).height(0.px()).boxed()
    };
    flex_row((
        icon_label(icon, 16.0, UI_MUTED),
        label(title)
            .font(UI_FONT_STACK)
            .text_size(13.5)
            .weight(FontWeight::BOLD)
            .color(UI_TEXT),
        FlexSpacer::Flex(1.0),
        clear,
        icon_button(Icon::X, false, DesktopReader::close_assistant_panel),
    ))
    .gap(6.px())
    .cross_axis_alignment(CrossAxisAlignment::Center)
}
