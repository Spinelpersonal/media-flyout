use super::dialog::{BUTTON, EDIT, LABEL, Template};
use std::{
    cell::RefCell,
    fs, io,
    path::{Path, PathBuf},
};
use windows::Win32::{
    Foundation::{HWND, LPARAM, WPARAM},
    UI::{Input::KeyboardAndMouse::SetFocus, WindowsAndMessaging::*},
};

struct Welcome {
    hwnd: HWND,
    open_settings: Box<dyn FnOnce()>,
}
thread_local! { static OPEN: RefCell<Option<Welcome>> = const { RefCell::new(None) }; }

const SETTINGS: u16 = 100;

pub(super) fn handle_message(message: &MSG) -> bool {
    let hwnd = OPEN.with_borrow(|open| open.as_ref().map(|w| w.hwnd));
    // SAFETY: The modeless dialog and queued message belong to this thread.
    hwnd.is_some_and(|hwnd| unsafe { IsDialogMessageW(hwnd, message).as_bool() })
}

pub(crate) fn welcome_on_first_launch(
    shortcut: Option<&str>,
    config: &Path,
    open_settings: impl FnOnce() + 'static,
) -> io::Result<bool> {
    if let Some(hwnd) = OPEN.with_borrow(|open| open.as_ref().map(|w| w.hwnd)) {
        // SAFETY: OPEN is cleared when this thread destroys the dialog.
        unsafe {
            let _ = SetForegroundWindow(hwnd);
        }
        return Ok(true);
    }
    let local = std::env::var_os("LOCALAPPDATA")
        .filter(|p| !p.is_empty())
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "LOCALAPPDATA is unavailable"))?;
    let directory = PathBuf::from(local).join("MediaFlyout");
    fs::create_dir_all(&directory)?;
    present_once(&directory.join("welcome-seen"), || {
        let hwnd = create_dialog(shortcut, config, open_settings).map_err(io::Error::other)?;
        // SAFETY: The dialog was just created on this thread.
        unsafe {
            let _ = ShowWindow(hwnd, SW_SHOW);
            let _ = SetForegroundWindow(hwnd);
        }
        Ok(())
    })
}

fn present_once(marker: &Path, present: impl FnOnce() -> io::Result<()>) -> io::Result<bool> {
    match fs::File::create_new(marker) {
        Ok(_) => match present() {
            Ok(()) => Ok(true),
            Err(error) => {
                fs::remove_file(marker)?;
                Err(error)
            }
        },
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => Ok(false),
        Err(error) => Err(error),
    }
}

fn create_dialog(
    shortcut: Option<&str>,
    config: &Path,
    open_settings: impl FnOnce() + 'static,
) -> windows::core::Result<HWND> {
    let mut t = Template::new("Welcome to Media Flyout", 292);
    let toggle = match shortcut {
        Some(keys) => format!("Press {keys} or click the tray icon to show or hide the flyout. Pin it to keep it on top."),
        None => "Click the tray icon to open the flyout. Pin it to keep it on top. You can choose a shortcut in settings.".into(),
    };
    t.item(
        0xffff, LABEL,
        "Media Flyout stays in the system tray near the clock. If you don’t see its play icon, open Show hidden icons.",
        [12, 9, 268, 20], 0,
    );
    t.item(
        0xffff,
        BUTTON,
        "Control your media",
        [10, 33, 272, 73],
        BS_GROUPBOX as u32,
    );
    t.item(0xffff, LABEL, &toggle, [20, 45, 252, 18], 0);
    t.item(
        0xffff, LABEL,
        "Play music or a video in a compatible player. The flyout follows playback automatically. Click the player name to switch sources. You can pause, skip, seek, adjust app volume or open Lyrics when available.",
        [20, 65, 252, 35], 0,
    );
    t.item(
        0xffff,
        BUTTON,
        "Change your settings",
        [10, 112, 272, 83],
        BS_GROUPBOX as u32,
    );
    t.item(
        0xffff, LABEL,
        "Right-click the tray icon and choose Open settings. You can change the shortcut, position, theme and more. Choose Save to apply your changes.",
        [20, 125, 252, 28], 0,
    );
    t.item(
        0xffff,
        LABEL,
        "If you edit this file, choose Reload settings from the tray menu.",
        [20, 155, 252, 18],
        0,
    );
    t.item(
        101,
        EDIT,
        &config.to_string_lossy(),
        [20, 175, 252, 14],
        WS_TABSTOP.0 | WS_BORDER.0 | (ES_READONLY | ES_AUTOHSCROLL) as u32,
    );
    t.item(
        SETTINGS,
        BUTTON,
        "Open settings",
        [130, 203, 78, 16],
        WS_TABSTOP.0,
    );
    t.item(
        IDOK.0 as u16,
        BUTTON,
        "Got it",
        [214, 203, 66, 16],
        WS_TABSTOP.0 | BS_DEFPUSHBUTTON as u32,
    );
    let layout = t.finish(229);
    // SAFETY: The aligned template and its strings live through creation.
    let hwnd = unsafe {
        CreateDialogIndirectParamW(
            None,
            layout.as_ptr().cast(),
            None,
            Some(dialog_proc),
            LPARAM(0),
        )?
    };
    OPEN.set(Some(Welcome {
        hwnd,
        open_settings: Box::new(open_settings),
    }));
    Ok(hwnd)
}

unsafe extern "system" fn dialog_proc(hwnd: HWND, msg: u32, w: WPARAM, _: LPARAM) -> isize {
    match msg {
        WM_INITDIALOG => {
            // SAFETY: The default button exists in the dialog template.
            unsafe {
                if let Ok(button) = GetDlgItem(Some(hwnd), IDOK.0) {
                    let _ = SetFocus(Some(button));
                }
            }
            0
        }
        WM_CLOSE | WM_COMMAND
            if msg == WM_CLOSE
                || [SETTINGS, IDOK.0 as u16, IDCANCEL.0 as u16].contains(&(w.0 as u16)) =>
        {
            let welcome = OPEN.take();
            // SAFETY: This is a close action on our modeless dialog.
            unsafe {
                let _ = DestroyWindow(hwnd);
            }
            if msg == WM_COMMAND
                && w.0 as u16 == SETTINGS
                && let Some(welcome) = welcome
            {
                (welcome.open_settings)();
            }
            1
        }
        WM_NCDESTROY => {
            OPEN.take();
            0
        }
        _ => 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;
    use std::rc::Rc;

    #[test]
    fn only_a_successful_presentation_consumes_the_first_launch() {
        let marker =
            std::env::temp_dir().join(format!("media-flyout-welcome-test-{}", std::process::id()));
        assert!(present_once(&marker, || Err(io::Error::other("dialog failed"))).is_err());
        assert!(present_once(&marker, || Ok(())).unwrap());
        assert!(!present_once(&marker, || panic!("repeated welcome")).unwrap());
        fs::remove_file(marker).unwrap();
    }

    #[test]
    fn welcome_buttons_dismiss_or_open_settings() {
        for button in [IDOK.0 as u16, IDCANCEL.0 as u16, SETTINGS] {
            let opened = Rc::new(Cell::new(false));
            let capture = opened.clone();
            let hwnd = create_dialog(Some("Ctrl+Space"), Path::new("settings.toml"), move || {
                capture.set(true)
            })
            .unwrap();
            // SAFETY: The hidden dialog belongs to the test thread.
            unsafe {
                SendMessageW(hwnd, WM_COMMAND, Some(WPARAM(button.into())), None);
            }
            assert!(OPEN.with_borrow(Option::is_none));
            assert_eq!(opened.get(), button == SETTINGS);
        }
    }
}
