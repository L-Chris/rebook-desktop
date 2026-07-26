//! Xilem button variant that uses the platform pointer cursor.

use std::any::{TypeId, type_name};

use tracing::{Span, trace, trace_span};
use xilem::core::{MessageContext, MessageResult, Mut, View, ViewId, ViewMarker, ViewPathTracker};
use xilem::masonry::accesskit::{Node, Role};
use xilem::masonry::core::keyboard::{Key, NamedKey};
use xilem::masonry::core::{
    AccessCtx, AccessEvent, BoxConstraints, ChildrenIds, CursorIcon, EventCtx, HasProperty,
    LayoutCtx, NewWidget, PaintCtx, PointerButton, PointerButtonEvent, PointerEvent, PropertiesMut,
    PropertiesRef, QueryCtx, RegisterCtx, TextEvent, Update, UpdateCtx, Widget, WidgetId,
    WidgetMut, WidgetPod,
};
use xilem::masonry::kurbo::Size;
use xilem::masonry::properties::{
    ActiveBackground, Background, BorderColor, BorderWidth, BoxShadow, CornerRadius,
    DisabledBackground, HoveredBorderColor, Padding,
};
use xilem::masonry::theme;
use xilem::masonry::util::{fill, stroke};
use xilem::masonry::vello::Scene;
use xilem::{Pod, ViewCtx, WidgetView};

/// Creates a standard button whose hover cursor is the platform pointing hand.
pub(crate) fn button<State, Action, V: WidgetView<State, Action>>(
    child: V,
    callback: impl Fn(&mut State) -> Action + Send + 'static,
) -> PointerButtonView<
    impl for<'a> Fn(&'a mut State, Option<PointerButton>) -> MessageResult<Action> + Send + 'static,
    V,
> {
    PointerButtonView {
        child,
        callback: move |state: &mut State, pointer| match pointer {
            None | Some(PointerButton::Primary) => MessageResult::Action(callback(state)),
            _ => MessageResult::Nop,
        },
        disabled: false,
        accessibility_label: None,
    }
}

/// Xilem view for [`PointerButtonWidget`].
#[must_use = "View values do nothing unless provided to Xilem."]
pub(crate) struct PointerButtonView<F, V> {
    child: V,
    callback: F,
    disabled: bool,
    accessibility_label: Option<String>,
}

impl<F, V> PointerButtonView<F, V> {
    /// Gives icon-only buttons a readable platform accessibility name.
    pub(crate) fn accessibility_label(mut self, label: impl Into<String>) -> Self {
        self.accessibility_label = Some(label.into());
        self
    }
}

const BUTTON_CONTENT_VIEW_ID: ViewId = ViewId::new(0);

impl<F, V> ViewMarker for PointerButtonView<F, V> {}

impl<F, V, State, Action> View<State, Action, ViewCtx> for PointerButtonView<F, V>
where
    V: WidgetView<State, Action>,
    F: Fn(&mut State, Option<PointerButton>) -> MessageResult<Action> + Send + Sync + 'static,
{
    type Element = Pod<PointerButtonWidget>;
    type ViewState = V::ViewState;

    fn build(&self, ctx: &mut ViewCtx, app_state: &mut State) -> (Self::Element, Self::ViewState) {
        let (child, child_state) = ctx.with_id(BUTTON_CONTENT_VIEW_ID, |ctx| {
            View::<State, Action, _>::build(&self.child, ctx, app_state)
        });
        let element = ctx.with_action_widget(|ctx| {
            let mut pod = ctx.create_pod(PointerButtonWidget::new(
                child.new_widget,
                self.accessibility_label.clone(),
            ));
            pod.new_widget.options.disabled = self.disabled;
            pod
        });
        (element, child_state)
    }

    fn rebuild(
        &self,
        prev: &Self,
        view_state: &mut Self::ViewState,
        ctx: &mut ViewCtx,
        mut element: Mut<'_, Self::Element>,
        app_state: &mut State,
    ) {
        if prev.disabled != self.disabled {
            element.ctx.set_disabled(self.disabled);
        }
        if prev.accessibility_label != self.accessibility_label {
            element
                .widget
                .accessibility_label
                .clone_from(&self.accessibility_label);
            element.ctx.request_accessibility_update();
        }
        ctx.with_id(BUTTON_CONTENT_VIEW_ID, |ctx| {
            View::<State, Action, _>::rebuild(
                &self.child,
                &prev.child,
                view_state,
                ctx,
                PointerButtonWidget::child_mut(&mut element).downcast(),
                app_state,
            );
        });
    }

    fn teardown(
        &self,
        view_state: &mut Self::ViewState,
        ctx: &mut ViewCtx,
        mut element: Mut<'_, Self::Element>,
    ) {
        ctx.with_id(BUTTON_CONTENT_VIEW_ID, |ctx| {
            View::<State, Action, _>::teardown(
                &self.child,
                view_state,
                ctx,
                PointerButtonWidget::child_mut(&mut element).downcast(),
            );
        });
        ctx.teardown_leaf(element);
    }

    fn message(
        &self,
        view_state: &mut Self::ViewState,
        message: &mut MessageContext,
        mut element: Mut<'_, Self::Element>,
        app_state: &mut State,
    ) -> MessageResult<Action> {
        match message.take_first() {
            Some(BUTTON_CONTENT_VIEW_ID) => self.child.message(
                view_state,
                message,
                PointerButtonWidget::child_mut(&mut element).downcast(),
                app_state,
            ),
            None => {
                if let Some(press) = message.take_message::<PointerButtonPress>() {
                    (self.callback)(app_state, press.button)
                } else {
                    tracing::error!(
                        "Wrong message type in PointerButtonView::message: {message:?} expected {}",
                        type_name::<PointerButtonPress>()
                    );
                    MessageResult::Stale
                }
            }
            _ => MessageResult::Stale,
        }
    }
}

pub(crate) struct PointerButtonWidget {
    child: WidgetPod<dyn Widget>,
    accessibility_label: Option<String>,
}

impl PointerButtonWidget {
    fn new(child: NewWidget<impl Widget + ?Sized>, accessibility_label: Option<String>) -> Self {
        Self {
            child: child.erased().to_pod(),
            accessibility_label,
        }
    }

    fn child_mut<'a>(this: &'a mut WidgetMut<'_, Self>) -> WidgetMut<'a, dyn Widget> {
        this.ctx.get_mut(&mut this.widget.child)
    }
}

#[derive(Debug, PartialEq)]
pub(crate) struct PointerButtonPress {
    button: Option<PointerButton>,
}

impl HasProperty<DisabledBackground> for PointerButtonWidget {}
impl HasProperty<ActiveBackground> for PointerButtonWidget {}
impl HasProperty<Background> for PointerButtonWidget {}
impl HasProperty<HoveredBorderColor> for PointerButtonWidget {}
impl HasProperty<BorderColor> for PointerButtonWidget {}
impl HasProperty<BorderWidth> for PointerButtonWidget {}
impl HasProperty<CornerRadius> for PointerButtonWidget {}
impl HasProperty<Padding> for PointerButtonWidget {}
impl HasProperty<BoxShadow> for PointerButtonWidget {}

impl Widget for PointerButtonWidget {
    type Action = PointerButtonPress;

    fn on_pointer_event(
        &mut self,
        ctx: &mut EventCtx<'_>,
        _props: &mut PropertiesMut<'_>,
        event: &PointerEvent,
    ) {
        match event {
            PointerEvent::Down(..) => {
                ctx.capture_pointer();
                ctx.request_paint_only();
                trace!("PointerButton {:?} pressed", ctx.widget_id());
            }
            PointerEvent::Up(PointerButtonEvent { button, .. }) => {
                if ctx.is_active() && ctx.is_hovered() {
                    ctx.submit_action::<Self::Action>(PointerButtonPress { button: *button });
                }
                ctx.request_paint_only();
            }
            _ => {}
        }
    }

    fn on_text_event(
        &mut self,
        ctx: &mut EventCtx<'_>,
        _props: &mut PropertiesMut<'_>,
        event: &TextEvent,
    ) {
        if let TextEvent::Keyboard(event) = event
            && event.state.is_up()
            && (matches!(&event.key, Key::Character(character) if character == " ")
                || event.key == Key::Named(NamedKey::Enter))
        {
            ctx.submit_action::<Self::Action>(PointerButtonPress { button: None });
        }
    }

    fn on_access_event(
        &mut self,
        ctx: &mut EventCtx<'_>,
        _props: &mut PropertiesMut<'_>,
        event: &AccessEvent,
    ) {
        if event.action == xilem::masonry::accesskit::Action::Click {
            ctx.submit_action::<Self::Action>(PointerButtonPress { button: None });
        }
    }

    fn update(&mut self, ctx: &mut UpdateCtx<'_>, _props: &mut PropertiesMut<'_>, event: &Update) {
        if matches!(
            event,
            Update::HoveredChanged(_)
                | Update::ActiveChanged(_)
                | Update::FocusChanged(_)
                | Update::DisabledChanged(_)
        ) {
            ctx.request_paint_only();
        }
    }

    fn register_children(&mut self, ctx: &mut RegisterCtx<'_>) {
        ctx.register_child(&mut self.child);
    }

    fn property_changed(&mut self, ctx: &mut UpdateCtx<'_>, property_type: TypeId) {
        DisabledBackground::prop_changed(ctx, property_type);
        ActiveBackground::prop_changed(ctx, property_type);
        Background::prop_changed(ctx, property_type);
        HoveredBorderColor::prop_changed(ctx, property_type);
        BorderColor::prop_changed(ctx, property_type);
        BorderWidth::prop_changed(ctx, property_type);
        CornerRadius::prop_changed(ctx, property_type);
        Padding::prop_changed(ctx, property_type);
        BoxShadow::prop_changed(ctx, property_type);
    }

    fn layout(
        &mut self,
        ctx: &mut LayoutCtx<'_>,
        props: &mut PropertiesMut<'_>,
        constraints: &BoxConstraints,
    ) -> Size {
        let border = props.get::<BorderWidth>();
        let padding = props.get::<Padding>();
        let shadow = props.get::<BoxShadow>();
        let child_constraints = padding.layout_down(border.layout_down(constraints.loosen()));
        let child_size = ctx.run_layout(&mut self.child, &child_constraints);
        let baseline = ctx.child_baseline_offset(&self.child);
        let (size, baseline) = padding.layout_up(child_size, baseline);
        let (mut size, baseline) = border.layout_up(size, baseline);
        size.height = size.height.max(theme::BORDERED_WIDGET_HEIGHT);
        let size = constraints.constrain(size);
        ctx.place_child(
            &mut self.child,
            ((size.to_vec2() - child_size.to_vec2()) / 2.0).to_point(),
        );
        if shadow.is_visible() {
            ctx.set_paint_insets(shadow.get_insets());
        }
        ctx.set_baseline_offset(baseline);
        size
    }

    fn paint(&mut self, ctx: &mut PaintCtx<'_>, props: &PropertiesRef<'_>, scene: &mut Scene) {
        let border_width = props.get::<BorderWidth>();
        let border_radius = props.get::<CornerRadius>();
        let background = if ctx.is_disabled() {
            &props.get::<DisabledBackground>().0
        } else if ctx.is_active() {
            &props.get::<ActiveBackground>().0
        } else {
            props.get::<Background>()
        };
        let border_color = if ctx.is_hovered() {
            &props.get::<HoveredBorderColor>().0
        } else {
            props.get::<BorderColor>()
        };
        let background_rect = border_width.bg_rect(ctx.size(), border_radius);
        let border_rect = border_width.border_rect(ctx.size(), border_radius);
        fill(
            scene,
            &background_rect,
            &background.get_peniko_brush_for_rect(background_rect.rect()),
        );
        stroke(scene, &border_rect, border_color.color, border_width.width);
    }

    fn post_paint(&mut self, ctx: &mut PaintCtx<'_>, props: &PropertiesRef<'_>, scene: &mut Scene) {
        let shadow = props.get::<BoxShadow>();
        shadow.paint(
            scene,
            xilem::masonry::kurbo::Affine::IDENTITY,
            shadow.shadow_rect(ctx.size(), props.get::<CornerRadius>()),
        );
    }

    fn accessibility_role(&self) -> Role {
        Role::Button
    }

    fn accessibility(
        &mut self,
        _ctx: &mut AccessCtx<'_>,
        _props: &PropertiesRef<'_>,
        node: &mut Node,
    ) {
        node.add_action(xilem::masonry::accesskit::Action::Click);
        if let Some(label) = &self.accessibility_label {
            node.set_label(label.clone());
        }
    }

    fn children_ids(&self) -> ChildrenIds {
        ChildrenIds::from_slice(&[self.child.id()])
    }

    fn accepts_focus(&self) -> bool {
        true
    }

    fn accepts_text_input(&self) -> bool {
        false
    }

    fn get_cursor(&self, _ctx: &QueryCtx<'_>, _pos: xilem::masonry::kurbo::Point) -> CursorIcon {
        CursorIcon::Pointer
    }

    fn make_trace_span(&self, id: WidgetId) -> Span {
        trace_span!("PointerButton", id = id.trace())
    }
}
