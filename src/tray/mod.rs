use windows::{
    Win32::{
        Foundation::{HWND, LPARAM, POINT, WPARAM},
        UI::{Shell::*, WindowsAndMessaging::*},
    },
    core::{PCWSTR, w},
};

pub const CALLBACK_MESSAGE: u32 = WM_APP + 10;
// NIN_KEYSELECT is defined as (NIN_SELECT | NINF_KEY) in the Windows SDK.
pub const KEY_SELECT: u32 = NIN_SELECT | 1;

#[derive(Clone, Copy)]
pub enum MenuAction {
    Toggle,
    Exit,
    OpenConfig,
    Reload,
    Autostart,
    Remember,
    Automatic,
}

pub struct TrayIcon {
    hwnd: HWND,
    icon: HICON,
}

impl TrayIcon {
    pub fn new(hwnd: HWND) -> windows::core::Result<Self> {
        let mut pixels = [0u8; 32 * 32 * 4];
        let mut mask = [0xFFu8; 32 * 4];
        for y in 0..32usize {
            for x in 0..32usize {
                let inside = (x as i32 - 16).pow(2) + (y as i32 - 16).pow(2) <= 14 * 14;
                if inside {
                    let triangle = (12..=22).contains(&x) && y.abs_diff(16) * 2 <= 22 - x;
                    let rgba = if triangle {
                        [36, 42, 20, 255]
                    } else {
                        [205, 224, 173, 255]
                    };
                    pixels[(y * 32 + x) * 4..(y * 32 + x + 1) * 4].copy_from_slice(&rgba);
                    mask[y * 4 + x / 8] &= !(0x80 >> (x % 8));
                }
            }
        }
        // SAFETY: CreateIcon copies these correctly sized 32×32, 32-bit color and
        // word-aligned monochrome mask buffers. We own and destroy the returned icon.
        let icon = unsafe { CreateIcon(None, 32, 32, 1, 32, mask.as_ptr(), pixels.as_ptr())? };
        let tray = Self { hwnd, icon };
        tray.add()?;
        // SAFETY: The HWND and icon both remain valid until the tray is dropped.
        unsafe {
            SendMessageW(
                hwnd,
                WM_SETICON,
                Some(WPARAM(ICON_SMALL as usize)),
                Some(LPARAM(icon.0 as isize)),
            );
        }
        Ok(tray)
    }

    fn data(&self) -> NOTIFYICONDATAW {
        NOTIFYICONDATAW {
            cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32,
            hWnd: self.hwnd,
            uID: 1,
            ..Default::default()
        }
    }

    pub fn add(&self) -> windows::core::Result<()> {
        let mut data = self.data();
        data.uFlags = NIF_MESSAGE | NIF_ICON | NIF_TIP;
        data.uCallbackMessage = CALLBACK_MESSAGE;
        data.hIcon = self.icon;
        copy_text(&mut data.szTip, "Media Flyout");
        // SAFETY: The structure and icon are valid for the duration of the call.
        if !unsafe { Shell_NotifyIconW(NIM_ADD, &data) }.as_bool() {
            return Err(windows::core::Error::from_thread());
        }
        data.Anonymous.uVersion = NOTIFYICON_VERSION_4;
        // SAFETY: This is the same successfully registered notification icon.
        unsafe {
            let _ = Shell_NotifyIconW(NIM_SETVERSION, &data);
        }
        Ok(())
    }

    pub fn set_shortcut(&self, shortcut: &str) {
        let mut data = self.data();
        data.uFlags = NIF_TIP;
        copy_text(&mut data.szTip, &format!("Media Flyout • {shortcut}"));
        // SAFETY: The bounded, null-terminated tooltip belongs to our tray icon.
        unsafe {
            let _ = Shell_NotifyIconW(NIM_MODIFY, &data);
        }
    }

    pub fn notify(&self, message: &str) {
        let mut data = self.data();
        data.uFlags = NIF_INFO;
        data.dwInfoFlags = NIIF_WARNING;
        copy_text(&mut data.szInfoTitle, "Media Flyout");
        copy_text(&mut data.szInfo, message);
        // SAFETY: Fixed-size UTF-16 fields are bounded and null terminated.
        unsafe {
            let _ = Shell_NotifyIconW(NIM_MODIFY, &data);
        }
    }

    pub fn show_menu(&self, settings: &crate::config::Settings) -> Option<MenuAction> {
        // SAFETY: Menu handles are created, used, and destroyed on the UI thread.
        // TrackPopupMenu returns a command instead of dispatching arbitrary messages.
        unsafe {
            let menu = CreatePopupMenu().ok()?;
            let result = (|| {
                AppendMenuW(menu, MF_STRING, 1, w!("Show / hide flyout"))?;
                AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null())?;
                AppendMenuW(menu, MF_STRING, 3, w!("Open settings"))?;
                AppendMenuW(menu, MF_STRING, 4, w!("Reload settings"))?;
                AppendMenuW(
                    menu,
                    MF_STRING
                        | if settings.autostart {
                            MF_CHECKED
                        } else {
                            MF_UNCHECKED
                        },
                    5,
                    w!("Start with Windows"),
                )?;
                AppendMenuW(
                    menu,
                    MF_STRING
                        | if settings.remember_preferred_app {
                            MF_CHECKED
                        } else {
                            MF_UNCHECKED
                        },
                    6,
                    w!("Remember selected application"),
                )?;
                AppendMenuW(menu, MF_STRING, 7, w!("Clear application preference"))?;
                AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null())?;
                AppendMenuW(menu, MF_STRING, 2, w!("Exit"))?;
                let mut point = POINT::default();
                GetCursorPos(&mut point)?;
                let previous = GetForegroundWindow();
                let _ = SetForegroundWindow(self.hwnd);
                let command = TrackPopupMenu(
                    menu,
                    TPM_RETURNCMD | TPM_NONOTIFY | TPM_RIGHTBUTTON,
                    point.x,
                    point.y,
                    Some(0),
                    self.hwnd,
                    None,
                )
                .0;
                let _ = PostMessageW(Some(self.hwnd), WM_NULL, WPARAM(0), LPARAM(0));
                if previous != self.hwnd && !previous.is_invalid() {
                    let _ = SetForegroundWindow(previous);
                }
                Ok::<_, windows::core::Error>(match command {
                    1 => Some(MenuAction::Toggle),
                    2 => Some(MenuAction::Exit),
                    3 => Some(MenuAction::OpenConfig),
                    4 => Some(MenuAction::Reload),
                    5 => Some(MenuAction::Autostart),
                    6 => Some(MenuAction::Remember),
                    7 => Some(MenuAction::Automatic),
                    _ => None,
                })
            })();
            let _ = DestroyMenu(menu);
            result.ok().flatten()
        }
    }
}

impl Drop for TrayIcon {
    fn drop(&mut self) {
        // SAFETY: Remove our icon before releasing the HICON owned by this object.
        unsafe {
            let _ = Shell_NotifyIconW(NIM_DELETE, &self.data());
            let _ = DestroyIcon(self.icon);
        }
    }
}

fn copy_text<const N: usize>(destination: &mut [u16; N], text: &str) {
    for (destination, character) in destination
        .iter_mut()
        .take(N.saturating_sub(1))
        .zip(text.encode_utf16())
    {
        *destination = character;
    }
}
