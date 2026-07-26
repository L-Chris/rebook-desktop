//! Invisible layout probe used to make the bookshelf grid responsive.

use std::marker::PhantomData;

use tracing::{Span, trace_span};
use xilem::core::{MessageContext, MessageResult, Mut, View, ViewMarker};
use xilem::masonry::accesskit::{Node, Role};
use xilem::masonry::core::{
    AccessCtx, AccessEvent, BoxConstraints, ChildrenIds, CursorIcon, EventCtx, LayoutCtx, PaintCtx,
    PointerEvent, PropertiesMut, PropertiesRef, QueryCtx, RegisterCtx, TextEvent, Widget, WidgetId,
};
use xilem::masonry::kurbo::{Point, Size};
use xilem::masonry::vello::Scene;
use xilem::{Pod, ViewCtx};

pub(crate) fn shelf_width_probe<State, F>(on_width_changed: F) -> ShelfWidthProbeView<State, F>
where
    State: 'static,
    F: Fn(&mut State, f64) + Send + Sync + 'static,
{
    ShelfWidthProbeView {
        on_width_changed,
        state: PhantomData,
    }
}

pub(crate) struct ShelfWidthProbeView<State, F> {
    on_width_changed: F,
    state: PhantomData<fn() -> State>,
}

impl<State, F> ViewMarker for ShelfWidthProbeView<State, F> {}

impl<State, Action, F> View<State, Action, ViewCtx> for ShelfWidthProbeView<State, F>
where
    State: 'static,
    Action: 'static,
    F: Fn(&mut State, f64) + Send + Sync + 'static,
{
    type Element = Pod<ShelfWidthProbe>;
    type ViewState = ();

    fn build(&self, ctx: &mut ViewCtx, _: &mut State) -> (Self::Element, Self::ViewState) {
        (
            ctx.with_action_widget(|ctx| ctx.create_pod(ShelfWidthProbe::default())),
            (),
        )
    }

    fn rebuild(
        &self,
        _prev: &Self,
        (): &mut Self::ViewState,
        _ctx: &mut ViewCtx,
        _element: Mut<'_, Self::Element>,
        _state: &mut State,
    ) {
    }

    fn teardown(&self, (): &mut Self::ViewState, _: &mut ViewCtx, _: Mut<'_, Self::Element>) {}

    fn message(
        &self,
        (): &mut Self::ViewState,
        message: &mut MessageContext,
        _: Mut<'_, Self::Element>,
        state: &mut State,
    ) -> MessageResult<Action> {
        let Some(width) = message.take_message::<ShelfWidthChanged>() else {
            return MessageResult::Stale;
        };
        (self.on_width_changed)(state, width.0);
        MessageResult::RequestRebuild
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct ShelfWidthProbe {
    width: f64,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct ShelfWidthChanged(f64);

impl Widget for ShelfWidthProbe {
    type Action = ShelfWidthChanged;

    fn on_pointer_event(
        &mut self,
        _ctx: &mut EventCtx<'_>,
        _props: &mut PropertiesMut<'_>,
        _event: &PointerEvent,
    ) {
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

    fn register_children(&mut self, _ctx: &mut RegisterCtx<'_>) {}

    fn layout(
        &mut self,
        ctx: &mut LayoutCtx<'_>,
        _props: &mut PropertiesMut<'_>,
        constraints: &BoxConstraints,
    ) -> Size {
        let width = if constraints.is_width_bounded() {
            constraints.max().width
        } else {
            constraints.min().width
        };
        if (self.width - width).abs() > f64::EPSILON {
            self.width = width;
            ctx.submit_action::<Self::Action>(ShelfWidthChanged(width));
        }
        constraints.constrain(Size::new(width, 0.0))
    }

    fn paint(&mut self, _ctx: &mut PaintCtx<'_>, _props: &PropertiesRef<'_>, _scene: &mut Scene) {}

    fn accessibility_role(&self) -> Role {
        Role::GenericContainer
    }

    fn get_cursor(&self, _ctx: &QueryCtx<'_>, _pos: Point) -> CursorIcon {
        CursorIcon::Default
    }

    fn accessibility(
        &mut self,
        _ctx: &mut AccessCtx<'_>,
        _props: &PropertiesRef<'_>,
        _node: &mut Node,
    ) {
    }

    fn children_ids(&self) -> ChildrenIds {
        ChildrenIds::new()
    }

    fn make_trace_span(&self, id: WidgetId) -> Span {
        trace_span!("ShelfWidthProbe", id = id.trace())
    }
}
