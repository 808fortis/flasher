#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]

mod app;
mod fastboot;
mod scatter;

use std::sync::Arc;
use std::time::Instant;
use winit::{
    application::ApplicationHandler,
    dpi::{LogicalSize, PhysicalPosition, PhysicalSize},
    event::{ElementState, Event, MouseButton, WindowEvent},
    event_loop::{ActiveEventLoop, EventLoop},
    window::{Window, WindowId},
};
use imgui_winit_support::{WinitPlatform, HiDpiMode};

use crate::app::App;

const EDGE_THICKNESS: f64 = 6.0;
const MIN_W: u32 = 640;
const MIN_H: u32 = 480;

#[derive(Default)]
struct DragState {
    active: bool,
    start_x: f64,
    start_y: f64,
    win_start_w: u32,
    win_start_h: u32,
    win_start_x: i32,
    win_start_y: i32,
    resize_left: bool,
    resize_right: bool,
    resize_top: bool,
    resize_bottom: bool,
}

impl DragState {
    fn is_resize(&self) -> bool {
        self.resize_left || self.resize_right || self.resize_top || self.resize_bottom
    }

    fn cursor_for_edge(window_w: f64, window_h: f64, cx: f64, cy: f64) -> Option<DragState> {
        let on_left = cx < EDGE_THICKNESS;
        let on_right = cx > window_w - EDGE_THICKNESS;
        let on_top = cy < EDGE_THICKNESS;
        let on_bottom = cy > window_h - EDGE_THICKNESS;

        if !on_left && !on_right && !on_top && !on_bottom {
            return None;
        }
        Some(DragState {
            resize_left: on_left, resize_right: on_right,
            resize_top: on_top, resize_bottom: on_bottom,
            ..Default::default()
        })
    }
}

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
            force_fallback_adapter: true,
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
    drag: DragState,
    last_frame: Instant,
    cursor_pos: (f64, f64),
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
            drag: DragState::default(),
            last_frame: Instant::now(),
            cursor_pos: (0.0, 0.0),
        }
    }
}

impl ApplicationHandler for FlasherApp {
    fn resumed(&mut self, el: &ActiveEventLoop) {
        let win = el.create_window(
            Window::default_attributes()
                .with_decorations(false)
                .with_resizable(true)
                .with_inner_size(LogicalSize::new(980.0, 680.0))
                .with_min_inner_size(LogicalSize::new(MIN_W as f64, MIN_H as f64))
                .with_title("flasher")
        ).unwrap();

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

    fn window_event(&mut self, el: &ActiveEventLoop, window_id: WindowId, event: WindowEvent) {
        let Some(ref window) = self.window else { return };
        let Some(ref mut platform) = self.platform else { return };

        let ev: Event<()> = Event::WindowEvent { window_id, event: event.clone() };
        platform.handle_event(self.imgui.io_mut(), window, &ev);

        match event {
            WindowEvent::CloseRequested => el.exit(),
            WindowEvent::DroppedFile(path) => self.app_state.load_scatter(&path),

            WindowEvent::CursorMoved { position, .. } => {
                self.cursor_pos = (position.x, position.y);

                if self.drag.active {
                    let dx = self.cursor_pos.0 - self.drag.start_x;
                    let dy = self.cursor_pos.1 - self.drag.start_y;

                    if self.drag.is_resize() {
                        let mut new_w = self.drag.win_start_w as f64;
                        let mut new_h = self.drag.win_start_h as f64;
                        let mut new_x = self.drag.win_start_x as f64;
                        let mut new_y = self.drag.win_start_y as f64;

                        if self.drag.resize_left {
                            new_w -= dx;
                            new_x += dx;
                        } else if self.drag.resize_right {
                            new_w += dx;
                        }
                        if self.drag.resize_top {
                            new_h -= dy;
                            new_y += dy;
                        } else if self.drag.resize_bottom {
                            new_h += dy;
                        }

                        let new_w = new_w.max(MIN_W as f64) as u32;
                        let new_h = new_h.max(MIN_H as f64) as u32;
                        let _ = window.request_inner_size(PhysicalSize::new(new_w, new_h));
                        window.set_outer_position(PhysicalPosition::new(new_x as i32, new_y as i32));
                    } else {
                        let pos = window.outer_position().unwrap();
                        window.set_outer_position(PhysicalPosition::new(
                            pos.x + dx as i32, pos.y + dy as i32,
                        ));
                        self.drag.start_x = self.cursor_pos.0;
                        self.drag.start_y = self.cursor_pos.1;
                    }
                }
            }

            WindowEvent::MouseInput { state, button: MouseButton::Left, .. } => {
                match state {
                    ElementState::Pressed => {
                        let win_w = window.inner_size().width as f64;
                        let win_h = window.inner_size().height as f64;
                        if let Some(edge) = DragState::cursor_for_edge(win_w, win_h, self.cursor_pos.0, self.cursor_pos.1) {
                            self.drag = edge;
                        } else {
                            self.drag = DragState {
                                active: true,
                                start_x: self.cursor_pos.0,
                                start_y: self.cursor_pos.1,
                                win_start_w: window.inner_size().width,
                                win_start_h: window.inner_size().height,
                                win_start_x: window.outer_position().unwrap_or(PhysicalPosition::new(0, 0)).x,
                                win_start_y: window.outer_position().unwrap_or(PhysicalPosition::new(0, 0)).y,
                                ..Default::default()
                            };
                        }
                    }
                    ElementState::Released => {
                        self.drag = DragState::default();
                    }
                }
            }

            WindowEvent::RedrawRequested => {
                let Some(ref state) = self.wgpu_state else { return };
                let Some(ref mut platform) = self.platform else { return };
                let Some(ref mut renderer) = self.renderer else { return };

                self.imgui.io_mut().update_delta_time(self.last_frame.elapsed());
                self.last_frame = Instant::now();

                let current = state.surface.get_current_texture();
                let surface_texture = match current {
                    wgpu::CurrentSurfaceTexture::Success(t) => t,
                    wgpu::CurrentSurfaceTexture::Suboptimal(t) => t,
                    _ => return,
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

    fn about_to_wait(&mut self, _el: &ActiveEventLoop) {
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
