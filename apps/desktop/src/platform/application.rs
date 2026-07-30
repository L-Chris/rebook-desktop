use std::sync::Arc;
use std::time::{Duration, Instant};

use winit::application::ApplicationHandler;
use winit::dpi::{LogicalSize, PhysicalSize};
use winit::event::{StartCause, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop, EventLoopProxy};
use winit::window::{Icon, Window, WindowId};

use super::UserEvent;
use super::gpu::GpuState;
use crate::app::DesktopApp;

const INITIAL_WIDTH: u32 = 1200;
const INITIAL_HEIGHT: u32 = 800;

fn app_icon() -> Option<Icon> {
    let image = image::load_from_memory(include_bytes!("../../../../assets/windows/torto-256.png"))
        .ok()?
        .into_rgba8();
    let (width, height) = image.dimensions();
    Icon::from_rgba(image.into_raw(), width, height).ok()
}

pub(crate) fn run(app: DesktopApp) -> Result<(), Box<dyn std::error::Error>> {
    let event_loop = EventLoop::<UserEvent>::with_user_event().build()?;
    let proxy = event_loop.create_proxy();
    let runtime = tokio::runtime::Runtime::new()?;
    let mut application = Application::new(app, proxy, runtime);
    event_loop.run_app(&mut application)?;
    if let Some(error) = application.fatal_error {
        return Err(error.into());
    }
    Ok(())
}

struct WindowState {
    window: Arc<Window>,
    gpu: GpuState,
    egui_state: egui_winit::State,
}

struct Application {
    app: DesktopApp,
    egui_ctx: egui::Context,
    window: Option<WindowState>,
    repaint_at: Option<Instant>,
    fatal_error: Option<String>,
    proxy: EventLoopProxy<UserEvent>,
    runtime: tokio::runtime::Runtime,
}

impl Application {
    fn new(
        app: DesktopApp,
        proxy: EventLoopProxy<UserEvent>,
        runtime: tokio::runtime::Runtime,
    ) -> Self {
        let egui_ctx = egui::Context::default();
        crate::ui::configure(&egui_ctx);
        let repaint_proxy = proxy.clone();
        egui_ctx.set_request_repaint_callback(move |request| {
            let _ = repaint_proxy.send_event(UserEvent::RepaintAfter(request.delay));
        });
        Self {
            app,
            egui_ctx,
            window: None,
            repaint_at: None,
            fatal_error: None,
            proxy,
            runtime,
        }
    }

    fn schedule_repaint(&mut self, event_loop: &ActiveEventLoop, delay: Duration) {
        let Some(window) = &self.window else {
            return;
        };
        if delay.is_zero() {
            window.window.request_redraw();
            return;
        }
        let deadline = Instant::now() + delay;
        if self.repaint_at.is_none_or(|current| deadline < current) {
            self.repaint_at = Some(deadline);
            event_loop.set_control_flow(ControlFlow::WaitUntil(deadline));
        }
    }
}

impl ApplicationHandler<UserEvent> for Application {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }
        let attributes = Window::default_attributes()
            .with_title("Torto · 小龟阅读")
            .with_window_icon(app_icon())
            .with_inner_size(LogicalSize::new(INITIAL_WIDTH, INITIAL_HEIGHT))
            .with_min_inner_size(LogicalSize::new(720_u32, 520_u32));
        let window = match event_loop.create_window(attributes) {
            Ok(window) => Arc::new(window),
            Err(error) => {
                self.fatal_error = Some(error.to_string());
                event_loop.exit();
                return;
            }
        };
        let egui_state = egui_winit::State::new(
            self.egui_ctx.clone(),
            egui::ViewportId::ROOT,
            window.as_ref(),
            None,
            window.theme(),
            None,
        );
        let gpu = match pollster::block_on(GpuState::new(Arc::clone(&window))) {
            Ok(gpu) => gpu,
            Err(error) => {
                self.fatal_error = Some(error);
                event_loop.exit();
                return;
            }
        };
        window.request_redraw();
        self.window = Some(WindowState {
            window,
            gpu,
            egui_state,
        });
    }

    fn new_events(&mut self, _event_loop: &ActiveEventLoop, cause: StartCause) {
        if matches!(cause, StartCause::ResumeTimeReached { .. })
            && self
                .repaint_at
                .is_some_and(|deadline| Instant::now() >= deadline)
        {
            self.repaint_at = None;
            if let Some(window) = &self.window {
                window.window.request_redraw();
            }
        }
    }

    fn user_event(&mut self, event_loop: &ActiveEventLoop, event: UserEvent) {
        match event {
            UserEvent::RepaintAfter(delay) => self.schedule_repaint(event_loop, delay),
            UserEvent::ShelfSync(message) => self.app.complete_shelf_sync(message),
            UserEvent::ReaderSearch(message) => self.app.complete_reader_search(message),
            UserEvent::ReaderChatStream(message) => self.app.update_reader_chat_stream(message),
            UserEvent::ReaderChat(message) => self.app.complete_reader_chat(message),
            UserEvent::ReaderTranslation(message) => self.app.complete_reader_translation(message),
            UserEvent::ReaderTocTranslation(message) => {
                self.app.complete_reader_toc_translation(message);
            }
        }
        if let Some(window) = &self.window {
            window.window.request_redraw();
        }
    }

    fn window_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        window_id: WindowId,
        event: WindowEvent,
    ) {
        let Some(state) = self.window.as_mut() else {
            return;
        };
        if state.window.id() != window_id {
            return;
        }
        let response = state.egui_state.on_window_event(&state.window, &event);
        if response.repaint {
            state.window.request_redraw();
        }
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::Focused(focused) => {
                let size = state.window.inner_size();
                crate::diagnostics::log(
                    "window.focus",
                    &[
                        crate::diagnostics::Field::Bool("focused", focused),
                        crate::diagnostics::Field::U64("width", u64::from(size.width)),
                        crate::diagnostics::Field::U64("height", u64::from(size.height)),
                    ],
                );
                self.app
                    .log_reader_diagnostics("window.focus.reader", Some(focused));
                if focused {
                    state.window.request_redraw();
                }
            }
            WindowEvent::Occluded(occluded) => {
                crate::diagnostics::log(
                    "window.occluded",
                    &[crate::diagnostics::Field::Bool("occluded", occluded)],
                );
                self.app
                    .log_reader_diagnostics("window.occluded.reader", None);
            }
            WindowEvent::Resized(size) => {
                state.gpu.resize(size);
                state.window.request_redraw();
            }
            WindowEvent::ScaleFactorChanged { .. } => {
                state.gpu.resize(state.window.inner_size());
                state.window.request_redraw();
            }
            WindowEvent::RedrawRequested => {
                if state.window.inner_size() == PhysicalSize::new(0, 0) {
                    return;
                }
                if let Err(error) = state.gpu.render(
                    &state.window,
                    &mut self.app,
                    &self.egui_ctx,
                    &mut state.egui_state,
                ) {
                    crate::diagnostics::log(
                        "render.fatal",
                        &[crate::diagnostics::Field::Usize(
                            "error_chars",
                            error.chars().count(),
                        )],
                    );
                    self.fatal_error = Some(error);
                    event_loop.exit();
                } else {
                    self.app.spawn_pending_tasks(&self.runtime, &self.proxy);
                }
            }
            _ => {}
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        if let Some(deadline) = self.repaint_at {
            if Instant::now() >= deadline {
                self.repaint_at = None;
                if let Some(window) = &self.window {
                    window.window.request_redraw();
                }
            } else {
                event_loop.set_control_flow(ControlFlow::WaitUntil(deadline));
            }
        } else {
            event_loop.set_control_flow(ControlFlow::Wait);
        }
    }
}
