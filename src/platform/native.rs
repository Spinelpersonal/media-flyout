use std::{
    cell::{Cell, RefCell},
    collections::VecDeque,
    rc::Rc,
    sync::{
        Arc, Mutex,
        atomic::{AtomicIsize, Ordering},
    },
    time::Duration,
};

use slint::{
    LogicalPosition, LogicalSize, PhysicalPosition, PhysicalSize, PlatformError, Window,
    WindowPosition, WindowSize,
    platform::{
        EventLoopProxy, Platform, PointerEventButton, Renderer, WindowAdapter, WindowEvent,
        software_renderer::{
            PremultipliedRgbaColor, RepaintBufferType, SoftwareRenderer, TargetPixel,
        },
    },
};
use windows::{
    Win32::{
        Foundation::{HWND, LPARAM, LRESULT, RECT, WPARAM},
        Graphics::Gdi::*,
        System::LibraryLoader::GetModuleHandleW,
        UI::{
            Controls::WM_MOUSELEAVE, HiDpi::*, Input::KeyboardAndMouse::*, WindowsAndMessaging::*,
        },
    },
    core::w,
};

const PROXY_MESSAGE: u32 = WM_APP + 1;
const QUIT_MESSAGE: u32 = WM_APP + 2;
const TIMER: usize = 1;
type Task = Box<dyn FnOnce() + Send>;

#[derive(Default)]
struct Shared {
    hwnd: AtomicIsize,
    queue: Mutex<VecDeque<Task>>,
}

impl Shared {
    fn post(&self, message: u32) -> Result<(), slint::EventLoopError> {
        let hwnd = HWND(self.hwnd.load(Ordering::Acquire) as *mut _);
        if hwnd.is_invalid() {
            return Err(slint::EventLoopError::EventLoopTerminated);
        }
        // SAFETY: The handle is published only while the UI window exists. A concurrent
        // shutdown makes PostMessage fail safely; no pointers are transferred in messages.
        unsafe { PostMessageW(Some(hwnd), message, WPARAM(0), LPARAM(0)) }
            .map_err(|_| slint::EventLoopError::EventLoopTerminated)
    }

    fn drain(&self) {
        loop {
            let task = self
                .queue
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .pop_front();
            match task {
                Some(task) => task(),
                None => break,
            }
        }
    }
}

struct Proxy(Arc<Shared>);

impl EventLoopProxy for Proxy {
    fn quit_event_loop(&self) -> Result<(), slint::EventLoopError> {
        self.0.post(QUIT_MESSAGE)
    }
    fn invoke_from_event_loop(&self, event: Task) -> Result<(), slint::EventLoopError> {
        self.0
            .queue
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .push_back(event);
        self.0.post(PROXY_MESSAGE)
    }
}

pub struct BackendState {
    window: RefCell<Option<Rc<NativeWindow>>>,
    shared: Arc<Shared>,
}

impl BackendState {
    pub fn window(&self) -> Option<Rc<NativeWindow>> {
        self.window.borrow().clone()
    }
}

struct Backend(Rc<BackendState>);

impl Platform for Backend {
    fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, PlatformError> {
        if self.0.window.borrow().is_some() {
            return Err("Only one flyout window is supported".into());
        }
        let window = NativeWindow::new(self.0.shared.clone())
            .map_err(|e| PlatformError::Other(e.to_string()))?;
        *self.0.window.borrow_mut() = Some(window.clone());
        Ok(window)
    }

    fn new_event_loop_proxy(&self) -> Option<Box<dyn EventLoopProxy>> {
        Some(Box::new(Proxy(self.0.shared.clone())))
    }

    fn run_event_loop(&self) -> Result<(), PlatformError> {
        let window = self
            .0
            .window
            .borrow()
            .clone()
            .ok_or_else(|| PlatformError::Other("No flyout window".into()))?;
        loop {
            slint::platform::update_timers_and_animations();
            let delay = slint::platform::duration_until_next_timer_update();
            let delay = if window.visible.get() && window.window.has_active_animations() {
                window.request_redraw();
                Some(
                    delay
                        .unwrap_or(Duration::from_millis(16))
                        .min(Duration::from_millis(16)),
                )
            } else {
                delay
            };
            // SAFETY: The message pump and its timer belong to the current UI thread.
            unsafe {
                let _ = KillTimer(Some(window.hwnd.get()), TIMER);
                if let Some(delay) = delay {
                    SetTimer(
                        Some(window.hwnd.get()),
                        TIMER,
                        delay.as_millis().clamp(1, u128::from(u32::MAX)) as u32,
                        None,
                    );
                }
                let mut message = MSG::default();
                let result = GetMessageW(&mut message, None, 0, 0).0;
                if result == -1 {
                    return Err(PlatformError::Other(
                        windows::core::Error::from_thread().to_string(),
                    ));
                }
                if result == 0 {
                    break;
                }
                let _ = TranslateMessage(&message);
                DispatchMessageW(&message);
            }
        }
        Ok(())
    }
}

pub fn initialize() -> Result<Rc<BackendState>, Box<dyn std::error::Error>> {
    // SAFETY: Called before creating any HWND on the main thread. Failure is benign
    // if a manifest or host has already selected per-monitor awareness.
    unsafe {
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
    }
    let state = Rc::new(BackendState {
        window: RefCell::new(None),
        shared: Arc::new(Shared::default()),
    });
    slint::platform::set_platform(Box::new(Backend(state.clone())))?;
    Ok(state)
}

/// Native 32-bit BGR pixels let GDI display Slint's retained buffer without a copy.
#[repr(transparent)]
#[derive(Clone, Copy, Default)]
struct Bgra(u32);

impl TargetPixel for Bgra {
    fn from_rgb(red: u8, green: u8, blue: u8) -> Self {
        Self(u32::from_le_bytes([blue, green, red, 0]))
    }
    fn blend(&mut self, color: PremultipliedRgbaColor) {
        let [blue, green, red, _] = self.0.to_le_bytes();
        let inverse = 255 - u32::from(color.alpha);
        let blend = |old: u8, new: u8| {
            (u32::from(new) + (u32::from(old) * inverse + 127) / 255).min(255) as u8
        };
        *self = Self::from_rgb(
            blend(red, color.red),
            blend(green, color.green),
            blend(blue, color.blue),
        );
    }
}

pub struct NativeWindow {
    window: Window,
    renderer: SoftwareRenderer,
    hwnd: Cell<HWND>,
    size: Cell<PhysicalSize>,
    visible: Cell<bool>,
    dirty: Cell<bool>,
    tracking_mouse: Cell<bool>,
    pixels: RefCell<Vec<Bgra>>,
    shared: Arc<Shared>,
}

impl NativeWindow {
    fn new(shared: Arc<Shared>) -> windows::core::Result<Rc<Self>> {
        let native = Rc::new_cyclic(|weak| Self {
            window: Window::new(weak.clone()),
            renderer: SoftwareRenderer::new_with_repaint_buffer_type(
                RepaintBufferType::ReusedBuffer,
            ),
            hwnd: Cell::new(HWND::default()),
            size: Cell::new(PhysicalSize::new(452, 196)),
            visible: Cell::new(false),
            dirty: Cell::new(true),
            tracking_mouse: Cell::new(false),
            pixels: RefCell::new(Vec::new()),
            shared,
        });
        // SAFETY: The registered procedure is valid for the process lifetime. The
        // stable Rc allocation outlives the HWND, and only its UI thread accesses it.
        unsafe {
            let instance = GetModuleHandleW(None)?;
            let class = WNDCLASSW {
                lpfnWndProc: Some(window_proc),
                hInstance: instance.into(),
                hCursor: LoadCursorW(None, IDC_ARROW)?,
                lpszClassName: w!("MediaFlyoutWindow"),
                ..Default::default()
            };
            if RegisterClassW(&class) == 0 {
                return Err(windows::core::Error::from_thread());
            }
            let hwnd = CreateWindowExW(
                WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
                w!("MediaFlyoutWindow"),
                w!("Media Flyout"),
                WS_POPUP,
                0,
                0,
                452,
                196,
                None,
                None,
                Some(instance.into()),
                Some(Rc::as_ptr(&native).cast()),
            )?;
            native.hwnd.set(hwnd);
            native.shared.hwnd.store(hwnd.0 as isize, Ordering::Release);
            let scale = GetDpiForWindow(hwnd).max(96) as f32 / 96.0;
            native.dispatch(WindowEvent::ScaleFactorChanged {
                scale_factor: scale,
            });
            native.resize(PhysicalSize::new(
                (452.0 * scale) as u32,
                (196.0 * scale) as u32,
            ));
        }
        Ok(native)
    }

    pub fn handle(&self) -> HWND {
        self.hwnd.get()
    }

    pub fn position_at_top(&self) {
        // SAFETY: All structures are initialized and pointers are valid for the call.
        unsafe {
            let foreground = GetForegroundWindow();
            let monitor = MonitorFromWindow(foreground, MONITOR_DEFAULTTOPRIMARY);
            let mut info = MONITORINFO {
                cbSize: std::mem::size_of::<MONITORINFO>() as u32,
                ..Default::default()
            };
            if GetMonitorInfoW(monitor, &mut info).as_bool() {
                let size = self.size.get();
                let left = info.rcWork.left
                    + ((info.rcWork.right - info.rcWork.left - size.width as i32) / 2).max(0);
                let top = info.rcWork.top + (20.0 * self.window.scale_factor()) as i32;
                let _ = SetWindowPos(
                    self.hwnd.get(),
                    Some(HWND_TOPMOST),
                    left,
                    top,
                    0,
                    0,
                    SWP_NOSIZE | SWP_NOACTIVATE,
                );
            }
        }
    }

    fn dispatch(&self, event: WindowEvent) {
        let _ = self.window.dispatch_event_with_result(event);
    }

    fn resize(&self, size: PhysicalSize) {
        let size = PhysicalSize::new(size.width.clamp(1, 4096), size.height.clamp(1, 4096));
        // SAFETY: The HWND belongs to this thread. No activation is requested.
        unsafe {
            let _ = SetWindowPos(
                self.hwnd.get(),
                None,
                0,
                0,
                size.width as i32,
                size.height as i32,
                SWP_NOMOVE | SWP_NOZORDER | SWP_NOACTIVATE,
            );
        }
        self.size.set(size);
        let scale = self.window.scale_factor();
        self.dispatch(WindowEvent::Resized {
            size: LogicalSize::new(size.width as f32 / scale, size.height as f32 / scale),
        });
        self.round_corners();
        self.request_redraw();
    }

    fn round_corners(&self) {
        let size = self.size.get();
        let radius = (32.0 * self.window.scale_factor()) as i32;
        // SAFETY: A successful SetWindowRgn transfers region ownership to Windows.
        unsafe {
            let region = CreateRoundRectRgn(
                0,
                0,
                size.width as i32 + 1,
                size.height as i32 + 1,
                radius,
                radius,
            );
            if !region.is_invalid() && SetWindowRgn(self.hwnd.get(), Some(region), true) == 0 {
                let _ = DeleteObject(region.into());
            }
        }
    }

    fn paint(&self) {
        // SAFETY: WM_PAINT is balanced with BeginPaint/EndPaint. The buffer is a
        // contiguous repr(transparent) array of 32-bit pixels and stays borrowed
        // until synchronous SetDIBitsToDevice returns. Negative height is top-down.
        unsafe {
            let mut paint = PAINTSTRUCT::default();
            let dc = BeginPaint(self.hwnd.get(), &mut paint);
            if self.visible.get() {
                let size = self.size.get();
                let mut pixels = self.pixels.borrow_mut();
                let length = size.width as usize * size.height as usize;
                if pixels.len() != length {
                    pixels.resize(length, Bgra::default());
                    self.dirty.set(true);
                }
                if self.dirty.replace(false) {
                    self.renderer.render(&mut pixels, size.width as usize);
                }
                let bitmap = BITMAPINFO {
                    bmiHeader: BITMAPINFOHEADER {
                        biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                        biWidth: size.width as i32,
                        biHeight: -(size.height as i32),
                        biPlanes: 1,
                        biBitCount: 32,
                        biCompression: BI_RGB.0,
                        ..Default::default()
                    },
                    ..Default::default()
                };
                SetDIBitsToDevice(
                    dc,
                    0,
                    0,
                    size.width,
                    size.height,
                    0,
                    0,
                    0,
                    size.height,
                    pixels.as_ptr().cast(),
                    &bitmap,
                    DIB_RGB_COLORS,
                );
            }
            let _ = EndPaint(self.hwnd.get(), &paint);
        }
    }

    fn pointer_position(&self, parameter: LPARAM) -> LogicalPosition {
        let x = (parameter.0 as u16) as i16;
        let y = ((parameter.0 >> 16) as u16) as i16;
        LogicalPosition::new(
            f32::from(x) / self.window.scale_factor(),
            f32::from(y) / self.window.scale_factor(),
        )
    }
}

impl WindowAdapter for NativeWindow {
    fn window(&self) -> &Window {
        &self.window
    }
    fn size(&self) -> PhysicalSize {
        self.size.get()
    }
    fn renderer(&self) -> &dyn Renderer {
        &self.renderer
    }
    fn set_visible(&self, visible: bool) -> Result<(), PlatformError> {
        self.visible.set(visible);
        if visible {
            self.position_at_top();
            self.request_redraw();
        }
        // SAFETY: The HWND is valid and the window is deliberately shown without focus.
        unsafe {
            let _ = ShowWindow(
                self.hwnd.get(),
                if visible { SW_SHOWNOACTIVATE } else { SW_HIDE },
            );
        }
        Ok(())
    }
    fn request_redraw(&self) {
        self.dirty.set(true);
        if self.visible.get() {
            // SAFETY: Invalidation posts a paint request; no Slint properties are read.
            unsafe {
                let _ = InvalidateRect(Some(self.hwnd.get()), None, false);
            }
        }
    }
    fn set_size(&self, size: WindowSize) {
        self.resize(size.to_physical(self.window.scale_factor()));
    }
    fn position(&self) -> Option<PhysicalPosition> {
        let mut rect = RECT::default();
        // SAFETY: rect is an initialized output structure and HWND is valid.
        unsafe {
            GetWindowRect(self.hwnd.get(), &mut rect).ok()?;
        }
        Some(PhysicalPosition::new(rect.left, rect.top))
    }
    fn set_position(&self, position: WindowPosition) {
        let position = position.to_physical(self.window.scale_factor());
        // SAFETY: This changes only the position of our window without activation.
        unsafe {
            let _ = SetWindowPos(
                self.hwnd.get(),
                None,
                position.x,
                position.y,
                0,
                0,
                SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE,
            );
        }
    }
}

impl Drop for NativeWindow {
    fn drop(&mut self) {
        self.shared.hwnd.store(0, Ordering::Release);
        // SAFETY: Clear the borrowed userdata pointer before destroying the window;
        // subsequent destroy messages fall through to DefWindowProc.
        unsafe {
            SetWindowLongPtrW(self.hwnd.get(), GWLP_USERDATA, 0);
            let _ = DestroyWindow(self.hwnd.get());
        }
    }
}

unsafe extern "system" fn window_proc(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    // SAFETY: WM_NCCREATE supplies CREATESTRUCTW. Its lpCreateParams points to a
    // stable NativeWindow Rc allocation alive until userdata is cleared in Drop.
    // Messages execute serially on its owning thread; all mutation uses Cell/RefCell.
    unsafe {
        if message == WM_NCCREATE {
            let create = &*(lparam.0 as *const CREATESTRUCTW);
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, create.lpCreateParams as isize);
        }
        let pointer = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *const NativeWindow;
        let Some(window) = pointer.as_ref() else {
            return DefWindowProcW(hwnd, message, wparam, lparam);
        };
        match message {
            WM_ERASEBKGND => return LRESULT(1),
            WM_PAINT => {
                window.paint();
                return LRESULT(0);
            }
            WM_CLOSE => {
                window.dispatch(WindowEvent::CloseRequested);
                return LRESULT(0);
            }
            WM_MOUSEACTIVATE => return LRESULT(MA_NOACTIVATE as isize),
            WM_MOUSEMOVE => {
                if !window.tracking_mouse.replace(true) {
                    let mut tracking = TRACKMOUSEEVENT {
                        cbSize: std::mem::size_of::<TRACKMOUSEEVENT>() as u32,
                        dwFlags: TME_LEAVE,
                        hwndTrack: hwnd,
                        ..Default::default()
                    };
                    let _ = TrackMouseEvent(&mut tracking);
                }
                window.dispatch(WindowEvent::PointerMoved {
                    position: window.pointer_position(lparam),
                });
            }
            WM_MOUSELEAVE => {
                window.tracking_mouse.set(false);
                window.dispatch(WindowEvent::PointerExited);
            }
            WM_LBUTTONDOWN => {
                SetCapture(hwnd);
                window.dispatch(WindowEvent::PointerPressed {
                    position: window.pointer_position(lparam),
                    button: PointerEventButton::Left,
                });
            }
            WM_LBUTTONUP => {
                let _ = ReleaseCapture();
                window.dispatch(WindowEvent::PointerReleased {
                    position: window.pointer_position(lparam),
                    button: PointerEventButton::Left,
                });
            }
            WM_DPICHANGED => {
                let scale = (wparam.0 as u16) as f32 / 96.0;
                window.dispatch(WindowEvent::ScaleFactorChanged {
                    scale_factor: scale,
                });
                window.resize(PhysicalSize::new(
                    (452.0 * scale) as u32,
                    (196.0 * scale) as u32,
                ));
            }
            WM_TIMER if wparam.0 == TIMER => {
                let _ = KillTimer(Some(hwnd), TIMER);
            }
            PROXY_MESSAGE => {
                window.shared.drain();
                return LRESULT(0);
            }
            QUIT_MESSAGE => {
                PostQuitMessage(0);
                return LRESULT(0);
            }
            _ => return DefWindowProcW(hwnd, message, wparam, lparam),
        }
        LRESULT(0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn native_pixel_blends_premultiplied_color() {
        let mut pixel = Bgra::from_rgb(100, 80, 60);
        pixel.blend(PremultipliedRgbaColor {
            red: 100,
            green: 0,
            blue: 0,
            alpha: 128,
        });
        assert_eq!(pixel.0.to_le_bytes(), [30, 40, 150, 0]);
        pixel.blend(PremultipliedRgbaColor {
            red: 1,
            green: 2,
            blue: 3,
            alpha: 255,
        });
        assert_eq!(pixel.0.to_le_bytes(), [3, 2, 1, 0]);
    }
}
