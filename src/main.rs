#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]

mod app;
mod fastboot;
mod scatter;

use std::sync::Arc;
use std::time::Instant;
use winit::{
    application::ApplicationHandler,
    dpi::LogicalSize,
    event::{Event, WindowEvent},
    event_loop::{ActiveEventLoop, EventLoop},
    window::{Window, WindowId},
};
use imgui_winit_support::{WinitPlatform, HiDpiMode};

use crate::app::App;

const MIN_W: u32 = 800;
const MIN_H: u32 = 500;

struct WgpuState {
    device: wgpu::Device,
    queue: wgpu::Queue,
    surface: wgpu::Surface<'static>,
    config: wgpu::SurfaceConfiguration,
}

impl WgpuState {
    async fn new(instance: &wgpu::Instance, arc_win: &Arc<Window>) -> Option<Self> {
        let size = arc_win.inner_size();
        let surface = instance.create_surface(arc_win.clone()).ok()?;
        let adapter = instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            compatible_surface: Some(&surface),
            force_fallback_adapter: false,
        }).await.ok()?;
        let (device, queue) = adapter.request_device(&wgpu::DeviceDescriptor::default()).await.ok()?;
        let mut config = surface.get_default_config(&adapter, size.width, size.height)?;
        config.view_formats.push(config.format);
        surface.configure(&device, &config);
        Some(WgpuState { device, queue, surface, config })
    }

    fn resize(&mut self, width: u32, height: u32) {
        self.config.width = width.max(1);
        self.config.height = height.max(1);
        self.surface.configure(&self.device, &self.config);
    }
}

struct FlasherApp {
    window: Option<Arc<Window>>,
    wgpu_state: Option<WgpuState>,
    imgui: imgui::Context,
    platform: Option<WinitPlatform>,
    renderer: Option<imgui_wgpu::Renderer>,
    app_state: App,
    last_frame: Instant,
    init_attempted: bool,
}

impl FlasherApp {
    fn new() -> Self {
        FlasherApp {
            window: None,
            wgpu_state: None,
            imgui: imgui::Context::create(),
            platform: None,
            renderer: None,
            app_state: App::new(),
            last_frame: Instant::now(),
            init_attempted: false,
        }
    }
}

impl FlasherApp {
    fn init_wgpu(&mut self, el: &ActiveEventLoop) {
        let win = match el.create_window(
            Window::default_attributes()
                .with_resizable(true)
                .with_inner_size(LogicalSize::new(980.0, 680.0))
                .with_min_inner_size(LogicalSize::new(MIN_W as f64, MIN_H as f64))
                .with_title("flasher")
        ) {
            Ok(w) => w,
            Err(e) => { eprintln!("create_window: {e}"); return; }
        };

        let arc_win = Arc::new(win);
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
        let Some(state) = pollster::block_on(WgpuState::new(&instance, &arc_win)) else {
            eprintln!("failed to initialize wgpu");
            el.exit();
            return;
        };

        let mut platform = WinitPlatform::new(&mut self.imgui);
        platform.attach_window(self.imgui.io_mut(), &arc_win, HiDpiMode::Default);

        let renderer = imgui_wgpu::Renderer::new(
            &mut self.imgui, &state.device, &state.queue,
            imgui_wgpu::RendererConfig::default(),
        );

        self.window = Some(arc_win);
        self.wgpu_state = Some(state);
        self.platform = Some(platform);
        self.renderer = Some(renderer);
    }
}

impl ApplicationHandler for FlasherApp {
    fn resumed(&mut self, el: &ActiveEventLoop) {
        if !self.init_attempted {
            self.init_attempted = true;
            self.init_wgpu(el);
        }
    }

    fn window_event(&mut self, el: &ActiveEventLoop, window_id: WindowId, event: WindowEvent) {
        let Some(ref window) = self.window else { return };
        let Some(ref mut platform) = self.platform else { return };

        let ev: Event<()> = Event::WindowEvent { window_id, event: event.clone() };
        platform.handle_event(self.imgui.io_mut(), window, &ev);

        match event {
            WindowEvent::CloseRequested => el.exit(),
            WindowEvent::DroppedFile(path) => self.app_state.load_scatter(&path),

            WindowEvent::RedrawRequested => {
                if self.wgpu_state.is_none() {
                    window.request_redraw();
                    return;
                }
                let state = self.wgpu_state.as_mut().unwrap();
                let platform = self.platform.as_mut().unwrap();
                let renderer = self.renderer.as_mut().unwrap();

                self.imgui.io_mut().update_delta_time(self.last_frame.elapsed());
                self.last_frame = Instant::now();

                let current = state.surface.get_current_texture();
                let surface_texture = match current {
                    wgpu::CurrentSurfaceTexture::Success(t) => t,
                    wgpu::CurrentSurfaceTexture::Suboptimal(t) => t,
                    wgpu::CurrentSurfaceTexture::Timeout | wgpu::CurrentSurfaceTexture::Occluded => {
                        window.request_redraw();
                        return;
                    }
                    wgpu::CurrentSurfaceTexture::Outdated => {
                        let size = window.inner_size();
                        state.resize(size.width, size.height);
                        window.request_redraw();
                        return;
                    }
                    wgpu::CurrentSurfaceTexture::Lost => {
                        let size = window.inner_size();
                        state.resize(size.width, size.height);
                        window.request_redraw();
                        return;
                    }
                    wgpu::CurrentSurfaceTexture::Validation => {
                        let size = window.inner_size();
                        state.resize(size.width, size.height);
                        window.request_redraw();
                        return;
                    }
                };

                let view = surface_texture.texture.create_view(&wgpu::TextureViewDescriptor::default());

                let size = window.inner_size();
                let ui = self.imgui.new_frame();
                self.app_state.render(ui, size.width as f32, size.height as f32);

                let mut encoder = state.device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
                    label: Some("encoder"),
                });

                let mut rpass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("pass"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: &view,
                        depth_slice: None,
                        resolve_target: None,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Clear(wgpu::Color { r: 0.08, g: 0.08, b: 0.10, a: 1.0 }),
                            store: wgpu::StoreOp::Store,
                        },
                    })],
                    depth_stencil_attachment: None,
                    timestamp_writes: None,
                    occlusion_query_set: None,
                    multiview_mask: None,
                });

                platform.prepare_render(ui, window);
                let _ = renderer.render(
                    self.imgui.render(), &state.queue, &state.device, &mut rpass,
                );

                drop(rpass);
                state.queue.submit(std::iter::once(encoder.finish()));
                surface_texture.present();
            }

            WindowEvent::Resized(size) => {
                if let Some(ref mut state) = self.wgpu_state {
                    state.resize(size.width, size.height);
                }
            }

            _ => {}
        }
    }

    fn about_to_wait(&mut self, el: &ActiveEventLoop) {
        if !self.init_attempted {
            self.init_attempted = true;
            self.init_wgpu(el);
        }
        if let Some(ref window) = self.window {
            window.request_redraw();
        }
    }
}

fn main() {
    std::panic::set_hook(Box::new(|info| {
        let msg = info.to_string();
        eprintln!("panic: {msg}");
        let _ = rfd::MessageDialog::new()
            .set_level(rfd::MessageLevel::Error)
            .set_title("flasher error")
            .set_description(&msg)
            .show();
    }));

    let event_loop = match EventLoop::new() {
        Ok(el) => el,
        Err(e) => {
            eprintln!("failed to create event loop: {e}");
            let _ = rfd::MessageDialog::new()
                .set_level(rfd::MessageLevel::Error)
                .set_title("flasher error")
                .set_description(format!("failed to create event loop: {e}"))
                .show();
            return;
        }
    };
    let mut app = FlasherApp::new();
    if let Err(e) = event_loop.run_app(&mut app) {
        eprintln!("event loop error: {e}");
    }
}
