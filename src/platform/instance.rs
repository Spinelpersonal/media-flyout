use windows::{
    Win32::{
        Foundation::{CloseHandle, ERROR_ALREADY_EXISTS, GetLastError, HANDLE, LPARAM, WPARAM},
        System::Threading::CreateMutexW,
        UI::WindowsAndMessaging::{FindWindowW, PostMessageW, WM_APP},
    },
    core::w,
};

pub const SHOW_MESSAGE: u32 = WM_APP + 20;
pub const EXIT_MESSAGE: u32 = WM_APP + 21;
pub const RELOAD_MESSAGE: u32 = WM_APP + 22;

pub struct Instance(HANDLE);

impl Instance {
    pub fn acquire() -> windows::core::Result<Option<Self>> {
        // SAFETY: The named local-session mutex has no owner and is released with
        // CloseHandle. No process-global or machine-wide settings are changed.
        unsafe {
            let handle = CreateMutexW(None, false, w!("Local\\MediaFlyout.Singleton"))?;
            if GetLastError() == ERROR_ALREADY_EXISTS {
                let _ = CloseHandle(handle);
                Ok(None)
            } else {
                Ok(Some(Self(handle)))
            }
        }
    }
}

pub fn activate_existing(manual_launch: bool) -> windows::core::Result<()> {
    // A simultaneous launch can hold the mutex before it has created its HWND.
    for _ in 0..100 {
        if request(SHOW_MESSAGE, WPARAM(usize::from(manual_launch)))? {
            return Ok(());
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    Err(windows::core::Error::new(
        windows::Win32::Foundation::E_FAIL,
        "Running flyout did not become ready",
    ))
}

impl Drop for Instance {
    fn drop(&mut self) {
        // SAFETY: This handle is owned and closed exactly once.
        unsafe {
            let _ = CloseHandle(self.0);
        }
    }
}

pub fn request_exit() -> windows::core::Result<bool> {
    request(EXIT_MESSAGE, WPARAM(0))
}

pub fn request_reload() -> windows::core::Result<bool> {
    request(RELOAD_MESSAGE, WPARAM(0))
}

fn request(message: u32, parameter: WPARAM) -> windows::core::Result<bool> {
    // SAFETY: Only an integer-valued application message is posted to the known
    // flyout class. Windows validates the returned HWND when posting.
    unsafe {
        let Ok(hwnd) = FindWindowW(w!("MediaFlyoutWindow"), w!("Media Flyout")) else {
            return Ok(false);
        };
        PostMessageW(Some(hwnd), message, parameter, LPARAM(0))?;
        Ok(true)
    }
}
