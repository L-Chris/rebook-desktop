use std::sync::Arc;

use egui::TextureId;
use egui_wgpu::{Renderer, RendererOptions, ScreenDescriptor};
use kurbo::Affine;
use vello::{
    AaConfig, AaSupport, RenderParams, Renderer as VelloRenderer, RendererOptions as VelloOptions,
    Scene,
};
use wgpu::{TextureFormat, TextureUsages};
use winit::dpi::PhysicalSize;
use winit::window::Window;

use crate::app::DesktopApp;
use crate::reader::{ReaderFramePlan, ReaderPageTexture};

struct PageTarget {
    _texture: wgpu::Texture,
    view: wgpu::TextureView,
    texture_id: TextureId,
    size: [u32; 2],
    logical_size: egui::Vec2,
    rendered_revision: Option<u64>,
}

pub(super) struct GpuState {
    surface: wgpu::Surface<'static>,
    device: wgpu::Device,
    queue: wgpu::Queue,
    surface_config: wgpu::SurfaceConfiguration,
    egui_renderer: Renderer,
    vello_renderer: VelloRenderer,
    page_target: Option<PageTarget>,
    retired_page_textures: Vec<TextureId>,
}

impl GpuState {
    pub(super) async fn new(window: Arc<Window>) -> Result<Self, String> {
        let size = window.inner_size();
        let instance = wgpu::Instance::default();
        let surface = instance
            .create_surface(window)
            .map_err(|error| error.to_string())?;
        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::HighPerformance,
                compatible_surface: Some(&surface),
                force_fallback_adapter: false,
            })
            .await
            .map_err(|error| error.to_string())?;
        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor {
                label: Some("rebook-device"),
                ..Default::default()
            })
            .await
            .map_err(|error| error.to_string())?;
        let capabilities = surface.get_capabilities(&adapter);
        let format = capabilities
            .formats
            .iter()
            .copied()
            .find(TextureFormat::is_srgb)
            .unwrap_or(capabilities.formats[0]);
        let mut surface_config = surface
            .get_default_config(&adapter, size.width.max(1), size.height.max(1))
            .ok_or_else(|| "当前 GPU 不支持窗口 Surface".to_owned())?;
        surface_config.format = format;
        surface_config.usage = TextureUsages::RENDER_ATTACHMENT;
        surface_config.view_formats = vec![format];
        surface.configure(&device, &surface_config);

        let egui_renderer = Renderer::new(&device, format, RendererOptions::default());
        let vello_renderer = VelloRenderer::new(
            &device,
            VelloOptions {
                antialiasing_support: AaSupport::area_only(),
                ..Default::default()
            },
        )
        .map_err(|error| error.to_string())?;
        Ok(Self {
            surface,
            device,
            queue,
            surface_config,
            egui_renderer,
            vello_renderer,
            page_target: None,
            retired_page_textures: Vec::new(),
        })
    }

    pub(super) fn resize(&mut self, size: PhysicalSize<u32>) {
        if size.width == 0 || size.height == 0 {
            return;
        }
        self.surface_config.width = size.width;
        self.surface_config.height = size.height;
        self.surface.configure(&self.device, &self.surface_config);
    }

    pub(super) fn render(
        &mut self,
        window: &Window,
        app: &mut DesktopApp,
        egui_ctx: &egui::Context,
        egui_state: &mut egui_winit::State,
    ) -> Result<(), String> {
        let raw_input = egui_state.take_egui_input(window);
        let pixels_per_point = egui_ctx.pixels_per_point();
        let mut plan = None;
        let output = egui_ctx.run_ui(raw_input, |ui| {
            plan = app.ui(ui, self.page_texture());
        });
        egui_state.handle_platform_output(window, output.platform_output);

        let mut page_target_recreated = false;
        if let Some(plan) = plan {
            let recreated = self.ensure_page_target(plan, pixels_per_point);
            page_target_recreated = recreated;
            let needs_render = recreated
                || self
                    .page_target
                    .as_ref()
                    .is_some_and(|target| target.rendered_revision != Some(plan.scene_revision));
            if needs_render && let Some(scene) = app.reader_scene() {
                self.render_reader_scene(&scene, plan, pixels_per_point)?;
            }
        }
        if page_target_recreated {
            // The current egui frame still references the previous native texture.
            // Schedule a fresh frame that will pick up the newly rendered target.
            window.request_redraw();
        }

        let paint_jobs = egui_ctx.tessellate(output.shapes, pixels_per_point);
        let screen = ScreenDescriptor {
            size_in_pixels: [self.surface_config.width, self.surface_config.height],
            pixels_per_point,
        };
        for (id, delta) in &output.textures_delta.set {
            self.egui_renderer
                .update_texture(&self.device, &self.queue, *id, delta);
        }

        let frame = match self.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(frame) => frame,
            wgpu::CurrentSurfaceTexture::Suboptimal(frame) => {
                window.request_redraw();
                frame
            }
            wgpu::CurrentSurfaceTexture::Outdated | wgpu::CurrentSurfaceTexture::Lost => {
                self.surface.configure(&self.device, &self.surface_config);
                window.request_redraw();
                return Ok(());
            }
            wgpu::CurrentSurfaceTexture::Timeout | wgpu::CurrentSurfaceTexture::Occluded => {
                return Ok(());
            }
            wgpu::CurrentSurfaceTexture::Validation => {
                return Err("Surface validation failed".into());
            }
        };
        let view = frame
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("egui-encoder"),
            });
        let callback_commands = self.egui_renderer.update_buffers(
            &self.device,
            &self.queue,
            &mut encoder,
            &paint_jobs,
            &screen,
        );
        {
            let pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("egui-pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    resolve_target: None,
                    depth_slice: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color {
                            r: 0.965,
                            g: 0.957,
                            b: 0.937,
                            a: 1.0,
                        }),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            self.egui_renderer
                .render(&mut pass.forget_lifetime(), &paint_jobs, &screen);
        }
        self.queue
            .submit(callback_commands.into_iter().chain([encoder.finish()]));
        frame.present();
        for id in self.retired_page_textures.drain(..) {
            self.egui_renderer.free_texture(&id);
        }
        for id in &output.textures_delta.free {
            self.egui_renderer.free_texture(id);
        }
        Ok(())
    }

    fn page_texture(&self) -> Option<ReaderPageTexture> {
        self.page_target.as_ref().map(|target| ReaderPageTexture {
            id: target.texture_id,
            size: target.logical_size,
        })
    }

    fn ensure_page_target(&mut self, plan: ReaderFramePlan, pixels_per_point: f32) -> bool {
        let size = [
            physical_dimension(plan.rect.width(), pixels_per_point),
            physical_dimension(plan.rect.height(), pixels_per_point),
        ];
        if self
            .page_target
            .as_ref()
            .is_some_and(|target| target.size == size)
        {
            return false;
        }
        // Keep sampling the settled page texture while either side panel is sliding.
        // Replacing the native egui texture on every animation frame can expose a
        // partially updated GPU view; egui can safely scale the existing texture until
        // the panel reaches its target width, when we resize and redraw exactly once.
        if plan.defer_target_resize && self.page_target.is_some() {
            return false;
        }
        let texture = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("reader-vello-target"),
            size: wgpu::Extent3d {
                width: size[0],
                height: size[1],
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: TextureFormat::Rgba8Unorm,
            usage: TextureUsages::STORAGE_BINDING | TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        // Register a new native texture instead of rebinding the texture ID used by
        // the egui shapes already built for this frame. The previous ID stays alive
        // until those shapes have been submitted, so rapid panel motion can never
        // sample a half-swapped Vello target.
        let texture_id = self.egui_renderer.register_native_texture(
            &self.device,
            &view,
            wgpu::FilterMode::Linear,
        );
        let previous = self.page_target.replace(PageTarget {
            _texture: texture,
            view,
            texture_id,
            size,
            logical_size: plan.rect.size(),
            rendered_revision: None,
        });
        if let Some(previous) = previous {
            self.retired_page_textures.push(previous.texture_id);
        }
        true
    }

    fn render_reader_scene(
        &mut self,
        scene: &Scene,
        plan: ReaderFramePlan,
        pixels_per_point: f32,
    ) -> Result<(), String> {
        let Some(target) = self.page_target.as_mut() else {
            return Ok(());
        };
        let mut physical_scene = Scene::new();
        physical_scene.append(scene, Some(Affine::scale(f64::from(pixels_per_point))));
        self.vello_renderer
            .render_to_texture(
                &self.device,
                &self.queue,
                &physical_scene,
                &target.view,
                &RenderParams {
                    base_color: plan.background,
                    width: target.size[0],
                    height: target.size[1],
                    antialiasing_method: AaConfig::Area,
                },
            )
            .map_err(|error| error.to_string())?;
        target.rendered_revision = Some(plan.scene_revision);
        Ok(())
    }
}

#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_precision_loss
)]
fn physical_dimension(points: f32, pixels_per_point: f32) -> u32 {
    let pixels = (points * pixels_per_point).ceil();
    if !pixels.is_finite() || pixels <= 1.0 {
        return 1;
    }
    pixels.min(u32::MAX as f32) as u32
}
