use crate::config::Monitor;
use windows::Win32::{
    Foundation::{POINT, RECT},
    Graphics::{
        Dwm::{DWMWA_EXTENDED_FRAME_BOUNDS, DwmGetWindowAttribute},
        Gdi::*,
    },
    UI::WindowsAndMessaging::*,
};

pub(crate) fn target(monitor: Monitor) -> HMONITOR {
    // SAFETY: All window and monitor queries use live system handles or local buffers.
    unsafe {
        match monitor {
            Monitor::Active => MonitorFromWindow(GetForegroundWindow(), MONITOR_DEFAULTTOPRIMARY),
            Monitor::Cursor => {
                let mut point = POINT::default();
                let _ = GetCursorPos(&mut point);
                MonitorFromPoint(point, MONITOR_DEFAULTTOPRIMARY)
            }
            Monitor::Primary => MonitorFromPoint(POINT::default(), MONITOR_DEFAULTTOPRIMARY),
        }
    }
}

pub(crate) fn foreground_fullscreen(monitor: HMONITOR) -> bool {
    // SAFETY: Read-only queries on the foreground HWND and initialized output buffers.
    unsafe {
        let window = GetForegroundWindow();
        if window.is_invalid()
            || window == GetDesktopWindow()
            || window == GetShellWindow()
            || !IsWindowVisible(window).as_bool()
            || IsIconic(window).as_bool()
            || (IsZoomed(window).as_bool()
                && GetWindowLongW(window, GWL_STYLE) as u32 & WS_CAPTION.0 == WS_CAPTION.0)
        {
            return false;
        }
        let mut class = [0u16; 32];
        let length = GetClassNameW(window, &mut class) as usize;
        if matches!(
            String::from_utf16_lossy(&class[..length]).as_str(),
            "WorkerW" | "Progman" | "Shell_TrayWnd"
        ) {
            return false;
        }
        let mut info = MONITORINFO {
            cbSize: size_of::<MONITORINFO>() as u32,
            ..Default::default()
        };
        if !GetMonitorInfoW(monitor, &mut info).as_bool() {
            return false;
        }
        let mut bounds = RECT::default();
        if DwmGetWindowAttribute(
            window,
            DWMWA_EXTENDED_FRAME_BOUNDS,
            (&mut bounds as *mut RECT).cast(),
            size_of::<RECT>() as u32,
        )
        .is_err()
            && GetWindowRect(window, &mut bounds).is_err()
        {
            return false;
        }
        covers_monitor(bounds, info.rcMonitor)
    }
}

fn covers_monitor(window: RECT, monitor: RECT) -> bool {
    // Allow a one-pixel rounding difference on scaled monitors.
    window.left <= monitor.left + 1
        && window.top <= monitor.top + 1
        && window.right >= monitor.right - 1
        && window.bottom >= monitor.bottom - 1
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fullscreen_uses_monitor_bounds_instead_of_work_area() {
        let monitor = RECT {
            left: -1920,
            top: 0,
            right: 0,
            bottom: 1080,
        };
        assert!(covers_monitor(monitor, monitor));
        assert!(covers_monitor(
            RECT {
                right: 1920,
                ..monitor
            },
            monitor
        ));
        assert!(!covers_monitor(
            RECT {
                bottom: 1040,
                ..monitor
            },
            monitor
        ));
        assert!(!covers_monitor(
            RECT {
                left: 0,
                right: 1920,
                ..monitor
            },
            monitor
        ));
    }
}
