//! Xilem view backed by a retained Vello scene.

use std::marker::PhantomData;
use std::sync::Arc;

use tracing::{Span, trace_span};
use xilem::core::{MessageContext, MessageResult, Mut, View, ViewMarker};
use xilem::masonry::accesskit::{Node, Role};
use xilem::masonry::core::keyboard::{Key, KeyState, Modifiers, NamedKey};
use xilem::masonry::core::{
    AccessCtx, AccessEvent, BoxConstraints, ChildrenIds, CursorIcon, EventCtx, LayoutCtx, PaintCtx,
    PointerButton, PointerButtonEvent, PointerEvent, PointerScrollEvent, PointerUpdate,
    PropertiesMut, PropertiesRef, QueryCtx, RegisterCtx, ScrollDelta, TextEvent, Widget, WidgetId,
    WidgetMut,
};
use xilem::masonry::kurbo::{Point, Size};
use xilem::masonry::vello::Scene;
use xilem::{Pod, ViewCtx};

const TOOLBAR_REVEAL_HEIGHT: f64 = 56.0;

// Masonry exposes pointer positions as f64, while reader hit testing uses f32
// logical pixels. Window-local coordinates are finite and viewport-bounded.
#[allow(clippy::cast_possible_truncation)]
fn pointer_coordinate(value: f64) -> f32 {
    debug_assert!(value.is_finite());
    debug_assert!((f64::from(f32::MIN)..=f64::from(f32::MAX)).contains(&value));
    value as f32
}

pub fn reader_canvas<State, F, G>(
    scene_revision: u64,
    draw: F,
    on_action: G,
) -> ReaderCanvasView<State, F, G>
where
    State: 'static,
    F: Fn(&mut State, Size) -> Arc<Scene> + Send + Sync + 'static,
{
    ReaderCanvasView {
        scene_revision,
        draw,
        on_action,
        state: PhantomData,
    }
}

pub struct ReaderCanvasView<State, F, G> {
    scene_revision: u64,
    draw: F,
    on_action: G,
    state: PhantomData<fn() -> State>,
}

pub struct ReaderCanvasViewState {
    scene_revision: u64,
    size: Size,
}

impl<State, F, G> ViewMarker for ReaderCanvasView<State, F, G> {}

impl<State, Action, F, G> View<State, Action, ViewCtx> for ReaderCanvasView<State, F, G>
where
    State: 'static,
    F: Fn(&mut State, Size) -> Arc<Scene> + Send + Sync + 'static,
    G: Fn(&mut State, ReaderCanvasAction) -> Action + Send + Sync + 'static,
{
    type Element = Pod<ReaderCanvas>;
    type ViewState = ReaderCanvasViewState;

    fn build(&self, ctx: &mut ViewCtx, _: &mut State) -> (Self::Element, Self::ViewState) {
        (
            ctx.with_action_widget(|ctx| ctx.create_pod(ReaderCanvas::default())),
            ReaderCanvasViewState {
                scene_revision: self.scene_revision,
                size: Size::ZERO,
            },
        )
    }

    fn rebuild(
        &self,
        _prev: &Self,
        view_state: &mut Self::ViewState,
        _ctx: &mut ViewCtx,
        mut element: Mut<'_, Self::Element>,
        state: &mut State,
    ) {
        let size = element.widget.size();
        if view_state.scene_revision == self.scene_revision && view_state.size == size {
            return;
        }
        let scene = (self.draw)(state, size);
        ReaderCanvas::set_scene(&mut element, scene);
        view_state.scene_revision = self.scene_revision;
        view_state.size = size;
    }

    fn teardown(&self, _: &mut Self::ViewState, _: &mut ViewCtx, _: Mut<'_, Self::Element>) {}

    fn message(
        &self,
        _: &mut Self::ViewState,
        message: &mut MessageContext,
        _: Mut<'_, Self::Element>,
        state: &mut State,
    ) -> MessageResult<Action> {
        let Some(action) = message.take_message::<ReaderCanvasAction>() else {
            return MessageResult::Stale;
        };
        let action = *action;
        if action == ReaderCanvasAction::SizeChanged {
            MessageResult::RequestRebuild
        } else {
            MessageResult::Action((self.on_action)(state, action))
        }
    }
}

#[derive(Clone)]
pub struct ReaderCanvas {
    scene: Arc<Scene>,
    size: Size,
    toolbar_visible: bool,
    selection_active: bool,
    selection_start: Point,
    selection_moved: bool,
}

impl Default for ReaderCanvas {
    fn default() -> Self {
        Self {
            scene: Arc::new(Scene::new()),
            size: Size::ZERO,
            toolbar_visible: false,
            selection_active: false,
            selection_start: Point::ZERO,
            selection_moved: false,
        }
    }
}

impl std::fmt::Debug for ReaderCanvas {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ReaderCanvas")
            .field("size", &self.size)
            .finish_non_exhaustive()
    }
}

impl ReaderCanvas {
    fn size(&self) -> Size {
        self.size
    }

    fn set_scene(this: &mut WidgetMut<'_, Self>, scene: Arc<Scene>) {
        this.widget.scene = scene;
        this.ctx.request_render();
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ReaderCanvasAction {
    SizeChanged,
    ToolbarVisibility(bool),
    OpenSearch,
    PreviousPage,
    NextPage,
    SelectionStart { x: f32, y: f32 },
    SelectionUpdate { x: f32, y: f32 },
    SelectionFinish { x: f32, y: f32, moved: bool },
    SelectionCancel,
}

impl Widget for ReaderCanvas {
    type Action = ReaderCanvasAction;

    fn accepts_focus(&self) -> bool {
        true
    }

    fn on_pointer_event(
        &mut self,
        ctx: &mut EventCtx<'_>,
        _props: &mut PropertiesMut<'_>,
        event: &PointerEvent,
    ) {
        match event {
            PointerEvent::Down(PointerButtonEvent {
                button: None | Some(PointerButton::Primary),
                state,
                ..
            }) => {
                ctx.request_focus();
                ctx.capture_pointer();
                let position = ctx.local_position(state.position);
                self.selection_active = true;
                self.selection_start = position;
                self.selection_moved = false;
                ctx.submit_action::<Self::Action>(ReaderCanvasAction::SelectionStart {
                    x: pointer_coordinate(position.x),
                    y: pointer_coordinate(position.y),
                });
            }
            PointerEvent::Move(PointerUpdate { current, .. }) => {
                if !ctx.is_focus_target() {
                    ctx.request_focus();
                }
                let position = ctx.local_position(current.position);
                let visible = toolbar_visible_at_y(position.y);
                if self.toolbar_visible != visible {
                    self.toolbar_visible = visible;
                    ctx.submit_action::<Self::Action>(ReaderCanvasAction::ToolbarVisibility(
                        visible,
                    ));
                }
                if self.selection_active && ctx.is_active() {
                    let distance = position - self.selection_start;
                    self.selection_moved |= distance.hypot() >= 3.0;
                    if self.selection_moved {
                        ctx.submit_action::<Self::Action>(ReaderCanvasAction::SelectionUpdate {
                            x: pointer_coordinate(position.x),
                            y: pointer_coordinate(position.y),
                        });
                    }
                }
            }
            PointerEvent::Up(PointerButtonEvent {
                button: None | Some(PointerButton::Primary),
                state,
                ..
            }) => {
                if self.selection_active {
                    let position = ctx.local_position(state.position);
                    ctx.submit_action::<Self::Action>(ReaderCanvasAction::SelectionFinish {
                        x: pointer_coordinate(position.x),
                        y: pointer_coordinate(position.y),
                        moved: self.selection_moved,
                    });
                    self.selection_active = false;
                    self.selection_moved = false;
                    ctx.release_pointer();
                }
            }
            PointerEvent::Cancel(_) => {
                if self.selection_active {
                    self.selection_active = false;
                    self.selection_moved = false;
                    ctx.submit_action::<Self::Action>(ReaderCanvasAction::SelectionCancel);
                }
            }
            PointerEvent::Scroll(PointerScrollEvent { delta, .. }) => {
                if let Some(action) = page_action_for_scroll_delta(*delta) {
                    ctx.submit_action::<Self::Action>(action);
                    ctx.set_handled();
                }
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
        let TextEvent::Keyboard(event) = event else {
            return;
        };
        if event.state != KeyState::Down || event.repeat {
            return;
        }
        let action = reader_action_for_key(&event.key, event.modifiers);
        if let Some(action) = action {
            ctx.submit_action::<Self::Action>(action);
            ctx.set_handled();
        }
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
        let size = if constraints.is_width_bounded() && constraints.is_height_bounded() {
            constraints.max()
        } else {
            constraints.constrain(Size::new(640.0, 480.0))
        };
        if self.size != size {
            self.size = size;
            ctx.submit_action::<Self::Action>(ReaderCanvasAction::SizeChanged);
        }
        size
    }

    fn paint(&mut self, _ctx: &mut PaintCtx<'_>, _props: &PropertiesRef<'_>, scene: &mut Scene) {
        scene.append(&self.scene, None);
    }

    fn accessibility_role(&self) -> Role {
        Role::Document
    }

    fn get_cursor(&self, _ctx: &QueryCtx<'_>, _pos: Point) -> CursorIcon {
        CursorIcon::Text
    }

    fn accessibility(
        &mut self,
        _ctx: &mut AccessCtx<'_>,
        _props: &PropertiesRef<'_>,
        node: &mut Node,
    ) {
        node.set_label("电子书阅读页面");
    }

    fn children_ids(&self) -> ChildrenIds {
        ChildrenIds::new()
    }

    fn make_trace_span(&self, id: WidgetId) -> Span {
        trace_span!("ReaderCanvas", id = id.trace())
    }
}

fn toolbar_visible_at_y(y: f64) -> bool {
    y.is_finite() && (0.0..=TOOLBAR_REVEAL_HEIGHT).contains(&y)
}

fn reader_action_for_key(key: &Key, modifiers: Modifiers) -> Option<ReaderCanvasAction> {
    if modifiers.ctrl()
        && matches!(key, Key::Character(character) if character.eq_ignore_ascii_case("f"))
    {
        return Some(ReaderCanvasAction::OpenSearch);
    }
    match key {
        Key::Named(NamedKey::ArrowLeft) => Some(ReaderCanvasAction::PreviousPage),
        Key::Named(NamedKey::ArrowRight) => Some(ReaderCanvasAction::NextPage),
        _ => None,
    }
}

fn page_action_for_scroll_delta(delta: ScrollDelta) -> Option<ReaderCanvasAction> {
    let vertical = match delta {
        ScrollDelta::PageDelta(_, y) | ScrollDelta::LineDelta(_, y) => f64::from(y),
        ScrollDelta::PixelDelta(position) => position.y,
    };
    match vertical.partial_cmp(&0.0) {
        Some(std::cmp::Ordering::Greater) => Some(ReaderCanvasAction::PreviousPage),
        Some(std::cmp::Ordering::Less) => Some(ReaderCanvasAction::NextPage),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn toolbar_is_revealed_only_in_the_reader_header_zone() {
        assert!(toolbar_visible_at_y(0.0));
        assert!(toolbar_visible_at_y(TOOLBAR_REVEAL_HEIGHT));
        assert!(!toolbar_visible_at_y(TOOLBAR_REVEAL_HEIGHT + 0.1));
        assert!(!toolbar_visible_at_y(-1.0));
        assert!(!toolbar_visible_at_y(f64::NAN));
    }

    #[test]
    fn horizontal_arrows_map_to_page_turns() {
        assert_eq!(
            reader_action_for_key(&Key::Named(NamedKey::ArrowLeft), Modifiers::empty()),
            Some(ReaderCanvasAction::PreviousPage)
        );
        assert_eq!(
            reader_action_for_key(&Key::Named(NamedKey::ArrowRight), Modifiers::empty()),
            Some(ReaderCanvasAction::NextPage)
        );
        assert_eq!(
            reader_action_for_key(&Key::Named(NamedKey::ArrowUp), Modifiers::empty()),
            None
        );
    }

    #[test]
    fn control_f_opens_reader_search() {
        assert_eq!(
            reader_action_for_key(&Key::Character("f".into()), Modifiers::CONTROL),
            Some(ReaderCanvasAction::OpenSearch)
        );
        assert_eq!(
            reader_action_for_key(&Key::Character("F".into()), Modifiers::CONTROL),
            Some(ReaderCanvasAction::OpenSearch)
        );
        assert_eq!(
            reader_action_for_key(&Key::Character("f".into()), Modifiers::empty()),
            None
        );
    }

    #[test]
    fn vertical_scroll_maps_to_page_turns() {
        assert_eq!(
            page_action_for_scroll_delta(ScrollDelta::LineDelta(0.0, 1.0)),
            Some(ReaderCanvasAction::PreviousPage)
        );
        assert_eq!(
            page_action_for_scroll_delta(ScrollDelta::LineDelta(0.0, -1.0)),
            Some(ReaderCanvasAction::NextPage)
        );
        assert_eq!(
            page_action_for_scroll_delta(ScrollDelta::LineDelta(1.0, 0.0)),
            None
        );
    }
}
