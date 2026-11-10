use std::{ffi::OsStr, os::windows::ffi::OsStrExt, path::Path};
use windows::{
    Win32::{
        Foundation::{ERROR_FILE_NOT_FOUND, ERROR_SUCCESS, HWND},
        System::Registry::*,
        UI::{
            Shell::ShellExecuteW,
            WindowsAndMessaging::{MB_ICONERROR, MB_OK, MessageBoxW, SW_SHOWNORMAL},
        },
    },
    core::{PCWSTR, w},
};

struct Key(HKEY);
impl Drop for Key {
    fn drop(&mut self) {
        // SAFETY: The registry handle was opened by this module and is closed once.
        unsafe {
            let _ = RegCloseKey(self.0);
        }
    }
}

fn wide(text: &OsStr) -> Vec<u16> {
    text.encode_wide().chain(Some(0)).collect()
}

pub fn autostart(enabled: bool, config_path: &Path) -> Result<(), Box<dyn std::error::Error>> {
    let executable = std::env::current_exe()?;
    let command = if enabled {
        Some(startup_command(&executable, config_path)?)
    } else {
        None
    };
    // SAFETY: Only the current user's MediaFlyout value is written or deleted.
    // Buffers are UTF-16 with terminating nulls and sizes are supplied in bytes.
    unsafe {
        if !enabled {
            // A custom test/profile configuration must not remove another
            // installation's startup entry. Only manage the entry for this file.
            let mut existing = [0u16; 1024];
            let mut bytes = (existing.len() * 2) as u32;
            let status = RegGetValueW(
                HKEY_CURRENT_USER,
                w!("Software\\Microsoft\\Windows\\CurrentVersion\\Run"),
                w!("MediaFlyout"),
                RRF_RT_REG_SZ,
                None,
                Some(existing.as_mut_ptr().cast()),
                Some(&mut bytes),
            );
            if status != ERROR_SUCCESS {
                return Ok(());
            }
            let length = existing
                .iter()
                .position(|&c| c == 0)
                .unwrap_or(existing.len());
            let existing = String::from_utf16_lossy(&existing[..length]);
            let suffix = format!(" --config \"{}\"", config_path.display());
            if !existing.to_lowercase().ends_with(&suffix.to_lowercase()) {
                return Ok(());
            }
        }
        let mut raw = HKEY::default();
        let status = RegCreateKeyExW(
            HKEY_CURRENT_USER,
            w!("Software\\Microsoft\\Windows\\CurrentVersion\\Run"),
            Some(0),
            None,
            REG_OPTION_NON_VOLATILE,
            KEY_SET_VALUE,
            None,
            &mut raw,
            None,
        );
        status.ok()?;
        let key = Key(raw);
        if let Some(command) = command {
            let data = wide(OsStr::new(&command));
            let bytes = std::slice::from_raw_parts(data.as_ptr().cast::<u8>(), data.len() * 2);
            RegSetValueExW(key.0, w!("MediaFlyout"), Some(0), REG_SZ, Some(bytes)).ok()?;
        } else {
            let status = RegDeleteValueW(key.0, w!("MediaFlyout"));
            if status != ERROR_SUCCESS && status != ERROR_FILE_NOT_FOUND {
                status.ok()?;
            }
        }
    }
    Ok(())
}

fn startup_command(executable: &Path, config: &Path) -> Result<String, Box<dyn std::error::Error>> {
    let executable = executable
        .to_str()
        .ok_or("Executable path is not valid Unicode")?;
    let config = config
        .to_str()
        .ok_or("Settings path is not valid Unicode")?;
    if executable.contains(['"', '\r', '\n']) || config.contains(['"', '\r', '\n']) {
        return Err("Invalid startup path".into());
    }
    let command = format!("\"{executable}\" --config \"{config}\"");
    if command.encode_utf16().count() > 259 {
        return Err("Startup command exceeds the Windows Run key's 260-character limit".into());
    }
    Ok(command)
}

pub fn system_light_theme() -> bool {
    let mut value = 0u32;
    let mut length = 4u32;
    // SAFETY: The DWORD output buffer is initialized and its byte length is exact.
    let result = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            w!("Software\\Microsoft\\Windows\\CurrentVersion\\Themes\\Personalize"),
            w!("AppsUseLightTheme"),
            RRF_RT_REG_DWORD,
            None,
            Some((&mut value as *mut u32).cast()),
            Some(&mut length),
        )
    };
    result == ERROR_SUCCESS && value != 0
}

pub fn open_config(path: &Path) -> Result<(), String> {
    let argument = format!("\"{}\"", path.display());
    let argument = wide(OsStr::new(&argument));
    // SAFETY: The null-terminated buffers live through this call. The editor
    // executable is fixed; only the selected configuration path is an argument.
    let result = unsafe {
        ShellExecuteW(
            None,
            w!("open"),
            w!("notepad.exe"),
            PCWSTR(argument.as_ptr()),
            None,
            SW_SHOWNORMAL,
        )
    };
    if result.0 as isize <= 32 {
        return Err("Could not open settings in Notepad".into());
    }
    Ok(())
}

pub fn show_error(message: &str) {
    let message = wide(OsStr::new(message));
    // SAFETY: Both strings are null terminated and valid through the modal call.
    unsafe {
        MessageBoxW(
            Some(HWND::default()),
            PCWSTR(message.as_ptr()),
            w!("Media Flyout"),
            MB_ICONERROR | MB_OK,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn startup_paths_are_quoted_and_bounded() {
        assert_eq!(
            startup_command(
                Path::new("C:\\Program Files\\Flyout.exe"),
                Path::new("C:\\My Settings\\config.toml")
            )
            .expect("command"),
            "\"C:\\Program Files\\Flyout.exe\" --config \"C:\\My Settings\\config.toml\""
        );
        assert!(startup_command(Path::new("bad\"path.exe"), Path::new("config.toml")).is_err());
        assert!(startup_command(Path::new(&"x".repeat(260)), Path::new("config.toml")).is_err());
    }
}
