//! Compact hover help used for secondary explanatory copy.

use std::marker::PhantomData;

use tracing::{Span, trace_span};
use xilem::core::{MessageContext, MessageResult, Mut, View, ViewMarker};
use xilem::masonry::accesskit::{Node, Role};
use xilem::masonry::core::{
    AccessCtx, AccessEvent, BoxConstraints, ChildrenIds, CursorIcon, EventCtx, LayoutCtx,
    NewWidget, NoAction, PaintCtx, PointerEvent, PointerUpdate, Properties, PropertiesMut,
    PropertiesRef, QueryCtx, RegisterCtx, TextEvent, Update, UpdateCtx, Widget, WidgetId,
    WidgetMut, WidgetPod,
};
use xilem::masonry::kurbo::{Point, Size};
use xilem::masonry::parley::{FontWeight, StyleProperty};
use xilem::masonry::properties::types::{AsUnit, UnitPoint};
use xilem::masonry::properties::{
    Background, BorderColor, BorderWidth, ContentColor, CornerRadius, Padding,
};
use xilem::masonry::vello::Scene;
use xilem::masonry::widgets::{Label, Prose, SizedBox, TextArea, ZStack};
use xilem::{Pod, TextAlign, ViewCtx};

use super::theme::{UI_BORDER, UI_MUTED, UI_SURFACE, UI_TEXT};

const HELP_ICON_SIZE: f64 = 16.0;
const TOOLTIP_WIDTH: f64 = 252.0;

pub(crate) fn help_tooltip<State: 'static>(text: impl Into<String>) -> HelpTooltipView<State> {
    HelpTooltipView {
        text: text.into(),
        state: PhantomData,
    }
}

pub(crate) struct HelpTooltipView<State> {
    text: String,
    state: PhantomData<fn() -> State>,
}

impl<State> ViewMarker for HelpTooltipView<State> {}

impl<State, Action> View<State, Action, ViewCtx> for HelpTooltipView<State>
where
    State: 'static,
    Action: 'static,
{
    type Element = Pod<HelpTooltip>;
    type ViewState = ();

    fn build(&self, ctx: &mut ViewCtx, _: &mut State) -> (Self::Element, Self::ViewState) {
        (ctx.create_pod(HelpTooltip::new(self.text.clone())), ())
    }

    fn rebuild(
        &self,
        prev: &Self,
        (): &mut Self::ViewState,
        _: &mut ViewCtx,
        mut element: Mut<'_, Self::Element>,
        _: &mut State,
    ) {
        if self.text != prev.text {
            HelpTooltip::set_text(&mut element, self.text.clone());
        }
    }

    fn teardown(
        &self,
        (): &mut Self::ViewState,
        _: &mut ViewCtx,
        mut element: Mut<'_, Self::Element>,
    ) {
        HelpTooltip::remove_layer(&mut element);
    }

    fn message(
        &self,
        (): &mut Self::ViewState,
        _: &mut MessageContext,
        _: Mut<'_, Self::Element>,
        _: &mut State,
    ) -> MessageResult<Action> {
        MessageResult::Stale
    }
}

pub(crate) struct HelpTooltip {
    text: String,
    icon: WidgetPod<SizedBox>,
    layer_root_id: Option<WidgetId>,
}

impl HelpTooltip {
    fn new(text: String) -> Self {
        let label = Label::new("?")
            .with_text_alignment(TextAlign::Center)
            .with_style(StyleProperty::FontSize(10.5))
            .with_style(StyleProperty::FontWeight(FontWeight::BOLD))
            .with_props(Properties::one(ContentColor::new(UI_MUTED)));
        let icon = SizedBox::new(label)
            .size(HELP_ICON_SIZE.px(), HELP_ICON_SIZE.px())
            .with_props(
                Properties::new()
                    .with(Background::Color(UI_SURFACE))
                    .with(BorderColor::new(UI_BORDER))
                    .with(BorderWidth::all(1.0))
                    .with(CornerRadius::all(HELP_ICON_SIZE / 2.0)),
            )
            .to_pod();
        Self {
            text,
            icon,
            layer_root_id: None,
        }
    }

    fn set_text(this: &mut WidgetMut<'_, Self>, text: String) {
        Self::remove_layer(this);
        this.widget.text = text;
    }

    fn remove_layer(this: &mut WidgetMut<'_, Self>) {
        if let Some(layer_root_id) = this.widget.layer_root_id.take() {
            this.ctx.remove_layer(layer_root_id);
        }
    }

    fn tooltip_layer(&self) -> NewWidget<ZStack> {
        let text_area = TextArea::new_immutable(&self.text)
            .with_word_wrap(true)
            .with_style(StyleProperty::FontSize(11.5))
            .with_props(Properties::one(ContentColor::new(UI_SURFACE)));
        let prose = Prose::from_text_area(text_area).with_auto_id();
        let tooltip = SizedBox::new(prose).width(TOOLTIP_WIDTH.px()).with_props(
            Properties::new()
                .with(Background::Color(UI_TEXT))
                .with(BorderColor::new(UI_TEXT))
                .with(BorderWidth::all(1.0))
                .with(CornerRadius::all(8.0))
                .with(Padding::from_vh(8.0, 10.0)),
        );

        // Layer roots receive the full window's minimum constraints. Keep that root
        // transparent and lay the actual tooltip out with loosened constraints so its
        // background hugs the content instead of covering the rest of the window.
        ZStack::new()
            .with_child(tooltip, UnitPoint::TOP_LEFT)
            .with_auto_id()
    }
}

impl Widget for HelpTooltip {
    type Action = NoAction;

    fn on_pointer_event(
        &mut self,
        ctx: &mut EventCtx<'_>,
        _props: &mut PropertiesMut<'_>,
        event: &PointerEvent,
    ) {
        if let PointerEvent::Move(PointerUpdate { current, .. }) = event
            && ctx.is_hovered()
        {
            let pointer = current.logical_point();
            let position = Point::new(pointer.x + 10.0, pointer.y + 14.0);
            if let Some(layer_root_id) = self.layer_root_id {
                ctx.reposition_layer(layer_root_id, position);
            } else {
                let layer = self.tooltip_layer();
                self.layer_root_id = Some(layer.id());
                ctx.create_layer(layer, position);
            }
        }
    }

    fn on_text_event(
        &mut self,
        _ctx: &mut EventCtx<'_>,
        _props: &mut PropertiesMut<'_>,
        _event: &TextEvent,
    ) {
    }

    fn on_access_event(
        &mut self,
        _ctx: &mut EventCtx<'_>,
        _props: &mut PropertiesMut<'_>,
        _event: &AccessEvent,
    ) {
    }

    fn register_children(&mut self, ctx: &mut RegisterCtx<'_>) {
        ctx.register_child(&mut self.icon);
    }

    fn update(&mut self, ctx: &mut UpdateCtx<'_>, _props: &mut PropertiesMut<'_>, event: &Update) {
        if let Update::HoveredChanged(false) = event
            && let Some(layer_root_id) = self.layer_root_id.take()
        {
            ctx.remove_layer(layer_root_id);
        }
    }

    fn layout(
        &mut self,
        ctx: &mut LayoutCtx<'_>,
        _props: &mut PropertiesMut<'_>,
        constraints: &BoxConstraints,
    ) -> Size {
        let size = ctx.run_layout(&mut self.icon, constraints);
        ctx.place_child(&mut self.icon, Point::ORIGIN);
        size
    }

    fn paint(&mut self, _ctx: &mut PaintCtx<'_>, _props: &PropertiesRef<'_>, _scene: &mut Scene) {}

    fn accessibility_role(&self) -> Role {
        Role::Button
    }

    fn get_cursor(&self, _ctx: &QueryCtx<'_>, _pos: Point) -> CursorIcon {
        CursorIcon::Help
    }

    fn accessibility(
        &mut self,
        _ctx: &mut AccessCtx<'_>,
        _props: &PropertiesRef<'_>,
        node: &mut Node,
    ) {
        node.set_label(self.text.clone());
    }

    fn children_ids(&self) -> ChildrenIds {
        ChildrenIds::from_slice(&[self.icon.id()])
    }

    fn make_trace_span(&self, id: WidgetId) -> Span {
        trace_span!("HelpTooltip", id = id.trace())
    }
}
