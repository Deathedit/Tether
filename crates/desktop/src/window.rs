//! A plain window that shows the decoded video (winit + softbuffer).

use std::num::NonZeroU32;
use std::rc::Rc;
use std::sync::Arc;

use winit::application::ApplicationHandler;
use winit::dpi::LogicalSize;
use winit::event::WindowEvent;
use winit::event_loop::{ActiveEventLoop, EventLoop, EventLoopProxy};
use winit::window::{Window, WindowId};

use crate::video::Shared;

pub enum UserEvent {
    /// A new frame is in `Shared::latest`.
    Frame,
    /// The session ended; close the window.
    SessionEnded,
}

pub struct VideoWindow {
    event_loop: EventLoop<UserEvent>,
}

impl VideoWindow {
    pub fn new() -> anyhow::Result<Self> {
        Ok(Self {
            event_loop: EventLoop::<UserEvent>::with_user_event().build()?,
        })
    }

    pub fn proxy(&self) -> EventLoopProxy<UserEvent> {
        self.event_loop.create_proxy()
    }

    /// Runs until the window is closed or `SessionEnded` arrives. `on_close`
    /// runs when the user closes the window.
    pub fn run(
        self,
        title: String,
        shared: Arc<Shared>,
        on_close: impl FnMut() + 'static,
    ) -> anyhow::Result<()> {
        let mut app = App {
            title,
            shared,
            on_close: Box::new(on_close),
            window: None,
            surface: None,
        };
        self.event_loop.run_app(&mut app)?;
        Ok(())
    }
}

type Surface = softbuffer::Surface<Rc<Window>, Rc<Window>>;

struct App {
    title: String,
    shared: Arc<Shared>,
    on_close: Box<dyn FnMut()>,
    window: Option<Rc<Window>>,
    surface: Option<Surface>,
}

impl ApplicationHandler<UserEvent> for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }
        let attrs = Window::default_attributes()
            .with_title(&self.title)
            .with_inner_size(LogicalSize::new(1280.0, 720.0));
        let window = match event_loop.create_window(attrs) {
            Ok(w) => Rc::new(w),
            Err(e) => {
                tracing::error!("could not create window: {e}");
                event_loop.exit();
                return;
            }
        };
        let surface = softbuffer::Context::new(window.clone())
            .and_then(|ctx| Surface::new(&ctx, window.clone()));
        match surface {
            Ok(s) => self.surface = Some(s),
            Err(e) => {
                tracing::error!("could not create drawing surface: {e}");
                event_loop.exit();
                return;
            }
        }
        let size = window.inner_size();
        *self.shared.target.lock().unwrap() = (size.width, size.height);
        self.window = Some(window);
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        match event {
            WindowEvent::CloseRequested => {
                (self.on_close)();
                event_loop.exit();
            }
            WindowEvent::Resized(size) => {
                *self.shared.target.lock().unwrap() = (size.width, size.height);
                if let Some(w) = &self.window {
                    w.request_redraw();
                }
            }
            WindowEvent::RedrawRequested => self.draw(),
            _ => {}
        }
    }

    fn user_event(&mut self, event_loop: &ActiveEventLoop, event: UserEvent) {
        match event {
            UserEvent::Frame => {
                if let Some(w) = &self.window {
                    w.request_redraw();
                }
            }
            UserEvent::SessionEnded => event_loop.exit(),
        }
    }
}

impl App {
    fn draw(&mut self) {
        let (Some(window), Some(surface)) = (&self.window, &mut self.surface) else {
            return;
        };
        let size = window.inner_size();
        let (Some(w), Some(h)) = (NonZeroU32::new(size.width), NonZeroU32::new(size.height)) else {
            return;
        };
        if let Err(e) = surface.resize(w, h) {
            tracing::warn!("surface resize failed: {e}");
            return;
        }
        let mut buffer = match surface.buffer_mut() {
            Ok(b) => b,
            Err(e) => {
                tracing::warn!("no surface buffer: {e}");
                return;
            }
        };
        buffer.fill(0);
        if let Some(frame) = self.shared.latest.lock().unwrap().as_ref() {
            blit_centered(&mut buffer, size.width, size.height, frame);
        }
        if let Err(e) = buffer.present() {
            tracing::warn!("present failed: {e}");
        }
    }
}

/// Copies `frame` into the middle of a `bw`×`bh` buffer, clipping if needed.
fn blit_centered(buffer: &mut [u32], bw: u32, bh: u32, frame: &crate::video::RgbFrame) {
    let w = frame.width.min(bw) as usize;
    let h = frame.height.min(bh) as usize;
    let x0 = ((bw as usize) - w) / 2;
    let y0 = ((bh as usize) - h) / 2;
    for row in 0..h {
        let src = &frame.pixels[row * frame.width as usize..][..w];
        let dst_start = (y0 + row) * bw as usize + x0;
        buffer[dst_start..dst_start + w].copy_from_slice(src);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::video::RgbFrame;

    #[test]
    fn blit_centers_and_clips() {
        let frame = RgbFrame {
            width: 2,
            height: 1,
            pixels: vec![1, 2],
        };
        let mut buf = vec![0u32; 4 * 3];
        blit_centered(&mut buf, 4, 3, &frame);
        assert_eq!(buf, vec![0, 0, 0, 0, 0, 1, 2, 0, 0, 0, 0, 0]);

        let big = RgbFrame {
            width: 3,
            height: 2,
            pixels: vec![1, 2, 3, 4, 5, 6],
        };
        let mut small = vec![0u32; 2];
        blit_centered(&mut small, 2, 1, &big);
        assert_eq!(small, vec![1, 2]);
    }
}
