use lucide_icons::Icon;
use xilem::masonry::properties::types::AsUnit;
use xilem::style::Style;
use xilem::view::{label, sized_box};
use xilem::{Color, WidgetView};

use crate::UI_BORDER;

pub(crate) fn icon_label<State: 'static>(
    icon: Icon,
    size: f32,
    color: Color,
) -> impl WidgetView<State> {
    label(char::from(icon).to_string())
        .font("lucide")
        .text_size(size)
        .color(color)
}

pub(crate) fn divider<State: 'static>() -> impl WidgetView<State> {
    sized_box(label(""))
        .height(1.px())
        .expand_width()
        .background_color(UI_BORDER)
}
