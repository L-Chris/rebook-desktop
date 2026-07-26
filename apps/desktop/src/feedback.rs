//! Shared feedback surfaces for inline notices and transient toasts.

use lucide_icons::Icon;
use xilem::masonry::properties::types::AsUnit;
use xilem::style::{Padding, Style};
use xilem::view::{
    CrossAxisAlignment, FlexExt, FlexSpacer, flex_col, flex_row, label, sized_box, zstack,
};
use xilem::{AnyWidgetView, Color, FontWeight, WidgetView};

use crate::pointer_button::button;
use crate::{UI_BORDER, UI_FONT_STACK, UI_MUTED, UI_SURFACE, UI_SURFACE_MUTED, UI_TEXT};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum NoticeTone {
    Info,
    Success,
    Error,
}

#[derive(Clone, Copy)]
struct NoticePalette {
    icon: Icon,
    accent: Color,
    soft: Color,
}

impl NoticeTone {
    const fn palette(self) -> NoticePalette {
        match self {
            Self::Info => NoticePalette {
                icon: Icon::Info,
                accent: Color::from_rgb8(0x0f, 0x76, 0x6e),
                soft: Color::from_rgb8(0xe2, 0xf3, 0xf1),
            },
            Self::Success => NoticePalette {
                icon: Icon::CheckCircle2,
                accent: Color::from_rgb8(0x15, 0x80, 0x3d),
                soft: Color::from_rgb8(0xec, 0xfd, 0xf3),
            },
            Self::Error => NoticePalette {
                icon: Icon::AlertTriangle,
                accent: Color::from_rgb8(0xb4, 0x23, 0x18),
                soft: Color::from_rgb8(0xfe, 0xf3, 0xf2),
            },
        }
    }
}

pub(crate) fn notice_card<State: 'static>(
    tone: NoticeTone,
    title: impl Into<String>,
    message: impl Into<String>,
) -> impl WidgetView<State> {
    let title = title.into();
    let message = message.into();
    notice_layout(
        tone,
        title,
        &message,
        sized_box(label("")).width(0.px()).height(0.px()).boxed(),
    )
}

pub(crate) fn dismissible_notice<State: 'static>(
    tone: NoticeTone,
    title: impl Into<String>,
    message: impl Into<String>,
    on_dismiss: impl Fn(&mut State) + Send + Sync + 'static,
) -> impl WidgetView<State> {
    let title = title.into();
    let message = message.into();
    let dismiss = sized_box(
        button(
            centered_lucide_icon::<State>(Icon::X, 14.0, UI_MUTED),
            on_dismiss,
        )
        .accessibility_label("关闭提示")
        .background_color(Color::TRANSPARENT)
        .active_background_color(UI_SURFACE_MUTED)
        .border_color(Color::TRANSPARENT)
        .hovered_border_color(Color::TRANSPARENT)
        .border_width(0.0)
        .corner_radius(7.0)
        .padding(0.0),
    )
    .width(28.px())
    .height(28.px())
    .boxed();
    notice_layout(tone, title, &message, dismiss)
}

fn notice_layout<State: 'static>(
    tone: NoticeTone,
    title: String,
    message: &str,
    trailing: Box<AnyWidgetView<State>>,
) -> impl WidgetView<State> + use<State> {
    let palette = tone.palette();
    sized_box(
        flex_row((
            sized_box(centered_lucide_icon::<State>(
                palette.icon,
                16.0,
                palette.accent,
            ))
            .width(30.px())
            .height(30.px())
            .background_color(palette.soft)
            .corner_radius(8.0),
            flex_col((
                label(title)
                    .font(UI_FONT_STACK)
                    .text_size(12.0)
                    .weight(FontWeight::BOLD)
                    .color(UI_TEXT),
                wrapped_ui_text::<State>(message, 48, 11.5, UI_MUTED),
            ))
            .gap(2.px())
            .cross_axis_alignment(CrossAxisAlignment::Start)
            .flex(1.0),
            FlexSpacer::Fixed(2.px()),
            trailing,
        ))
        .gap(10.px())
        .cross_axis_alignment(CrossAxisAlignment::Start)
        .must_fill_major_axis(true),
    )
    .expand_width()
    .background_color(UI_SURFACE)
    .border(UI_BORDER, 1.0)
    .corner_radius(12.0)
    .padding(Padding::from_vh(10.0, 12.0))
}

pub(crate) fn centered_lucide_icon<State: 'static>(
    icon: Icon,
    size: f32,
    color: Color,
) -> impl WidgetView<State> {
    zstack((label(char::from(icon).to_string())
        .font("lucide")
        .text_size(size)
        .color(color),))
}

pub(crate) fn wrapped_ui_text<State: 'static>(
    text: &str,
    max_display_units: usize,
    size: f32,
    color: Color,
) -> impl WidgetView<State> + use<State> {
    let lines = wrap_ui_lines(text, max_display_units)
        .into_iter()
        .map(|line| label(line).font(UI_FONT_STACK).text_size(size).color(color))
        .collect::<Vec<_>>();
    flex_col(lines)
        .gap(2.px())
        .cross_axis_alignment(CrossAxisAlignment::Start)
}

fn wrap_ui_lines(text: &str, max_display_units: usize) -> Vec<String> {
    let max_display_units = max_display_units.max(1);
    let mut lines = Vec::new();
    for paragraph in text.split('\n') {
        if paragraph.is_empty() {
            lines.push(String::new());
            continue;
        }
        let mut line = String::new();
        let mut units = 0;
        for character in paragraph.chars() {
            let character_units = usize::from(!character.is_ascii()) + 1;
            if units + character_units > max_display_units && !line.is_empty() {
                lines.push(std::mem::take(&mut line));
                units = 0;
            }
            line.push(character);
            units += character_units;
        }
        if !line.is_empty() {
            lines.push(line);
        }
    }
    if lines.is_empty() {
        lines.push(String::new());
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::wrap_ui_lines;

    #[test]
    fn wraps_mixed_chinese_and_ascii_without_corrupting_text() {
        let text = "确定要移除《Structured Writing》吗？本地书架中的副本将被删除。";
        let lines = wrap_ui_lines(text, 24);

        assert!(lines.len() > 1);
        assert_eq!(lines.concat(), text);
        assert!(lines.iter().all(|line| !line.contains('\u{fffd}')));
    }
}
