//! A modeless Windows settings dialog. Dialog units scale with the system font.
use crate::{
    config::{Monitor, Position, Settings, Theme},
    hotkey::Shortcut,
};
use std::{
    cell::{Cell, RefCell},
    rc::Rc,
};
use windows::{
    Win32::{
        Foundation::{HWND, LPARAM, WPARAM},
        UI::WindowsAndMessaging::*,
    },
    core::PCWSTR,
};

type Changes = Vec<(&'static str, toml_edit::Value)>;
type Save = Box<dyn Fn(Changes) -> Result<(), String>>;
struct Dialog {
    hwnd: Cell<HWND>,
    fields: Vec<Field>,
    save: Save,
}
thread_local! { static OPEN: RefCell<Option<Rc<Dialog>>> = const { RefCell::new(None) }; }

enum Input {
    Text(String),
    Number(u16, u16),
    Choice(&'static [(&'static str, &'static str)], usize),
    Check(bool),
}
struct Field(&'static str, &'static str, Input);

fn fields(s: &Settings) -> Vec<Field> {
    use Input::*;
    vec![
        Field("shortcut", "Keyboard shortcut", Text(s.shortcut.clone())),
        Field(
            "position",
            "Flyout position",
            Choice(
                &[
                    ("Top center", "top-center"),
                    ("Top left", "top-left"),
                    ("Top right", "top-right"),
                    ("Bottom center", "bottom-center"),
                    ("Bottom left", "bottom-left"),
                    ("Bottom right", "bottom-right"),
                ],
                match s.position {
                    Position::TopCenter => 0,
                    Position::TopLeft => 1,
                    Position::TopRight => 2,
                    Position::BottomCenter => 3,
                    Position::BottomLeft => 4,
                    Position::BottomRight => 5,
                },
            ),
        ),
        Field(
            "monitor",
            "Monitor",
            Choice(
                &[
                    ("Active window", "active"),
                    ("Mouse pointer", "cursor"),
                    ("Primary display", "primary"),
                ],
                match s.monitor {
                    Monitor::Active => 0,
                    Monitor::Cursor => 1,
                    Monitor::Primary => 2,
                },
            ),
        ),
        Field("margin", "Edge margin (px)", Number(s.margin, 256)),
        Field(
            "theme",
            "Theme",
            Choice(
                &[
                    ("Dark", "dark"),
                    ("Light", "light"),
                    ("Use system setting", "system"),
                ],
                match s.theme {
                    Theme::Dark => 0,
                    Theme::Light => 1,
                    Theme::System => 2,
                },
            ),
        ),
        Field(
            "dismiss_after_seconds",
            "Hide after (s, 0 = never)",
            Number(s.dismiss_after_seconds, 600),
        ),
        Field(
            "preferred_app",
            "Preferred app ID",
            Text(s.preferred_app.clone().unwrap_or_default()),
        ),
        Field(
            "remember_preferred_app",
            "Remember selected application",
            Check(s.remember_preferred_app),
        ),
        Field(
            "dismiss_on_outside_click",
            "Dismiss on outside click",
            Check(s.dismiss_on_outside_click),
        ),
        Field(
            "show_progress",
            "Show playback progress",
            Check(s.show_progress),
        ),
        Field("animations", "Animate flyout", Check(s.animations)),
        Field(
            "show_visualizer",
            "Show audio spectrum",
            Check(s.show_visualizer),
        ),
        Field("autostart", "Start with Windows", Check(s.autostart)),
    ]
}

use super::dialog::{BUTTON, COMBO, EDIT, LABEL, Template};

fn template(fields: &[Field]) -> Vec<u32> {
    let mut t = Template::new("Media Flyout settings", 242);
    let mut y = 10;
    for (i, Field(_, label, input)) in fields.iter().enumerate() {
        let id = 100 + i as u16;
        if let Input::Check(_) = input {
            t.item(
                id,
                BUTTON,
                label,
                [12, y, 216, 12],
                WS_TABSTOP.0 | BS_AUTOCHECKBOX as u32,
            );
            y += 15;
        } else {
            t.item(0xffff, LABEL, label, [12, y + 2, 102, 12], 0);
            match input {
                Input::Choice(..) => t.item(
                    id,
                    COMBO,
                    "",
                    [119, y, 110, 100],
                    WS_TABSTOP.0 | WS_VSCROLL.0 | CBS_DROPDOWNLIST as u32,
                ),
                _ => {
                    let (text, style) = match input {
                        Input::Text(text) => (text.clone(), ES_AUTOHSCROLL),
                        Input::Number(n, _) => (n.to_string(), ES_NUMBER),
                        _ => unreachable!(),
                    };
                    t.item(
                        id,
                        EDIT,
                        &text,
                        [119, y, 110, 14],
                        WS_TABSTOP.0 | WS_BORDER.0 | style as u32,
                    );
                }
            }
            y += 20;
        }
    }
    y += 6;
    t.item(
        IDOK.0 as u16,
        BUTTON,
        "Save",
        [126, y, 50, 15],
        WS_TABSTOP.0 | BS_DEFPUSHBUTTON as u32,
    );
    t.item(
        IDCANCEL.0 as u16,
        BUTTON,
        "Cancel",
        [180, y, 50, 15],
        WS_TABSTOP.0,
    );
    t.finish(y + 25)
}

pub fn open_settings(
    settings: &Settings,
    save: impl Fn(Changes) -> Result<(), String> + 'static,
) -> windows::core::Result<()> {
    let hwnd = create_dialog(settings, save)?;
    // SAFETY: the returned dialog belongs to this UI thread.
    unsafe {
        let _ = ShowWindow(hwnd, SW_SHOW);
        let _ = SetForegroundWindow(hwnd);
    }
    Ok(())
}

fn create_dialog(
    settings: &Settings,
    save: impl Fn(Changes) -> Result<(), String> + 'static,
) -> windows::core::Result<HWND> {
    if let Some(dialog) = OPEN.with_borrow(Clone::clone) {
        return Ok(dialog.hwnd.get());
    }
    let fields = fields(settings);
    let layout = template(&fields);
    OPEN.set(Some(Rc::new(Dialog {
        hwnd: Cell::default(),
        fields,
        save: Box::new(save),
    })));
    // SAFETY: layout is DWORD aligned and remains valid during dialog creation.
    let hwnd = unsafe {
        CreateDialogIndirectParamW(
            None,
            layout.as_ptr().cast(),
            None,
            Some(dialog_proc),
            LPARAM(0),
        )
    };
    if hwnd.is_err() {
        OPEN.take();
    }
    hwnd
}

pub(super) fn handle_message(message: &MSG) -> bool {
    let dialog = OPEN.with_borrow(Clone::clone);
    dialog.is_some_and(|d| {
        // SAFETY: the HWND and queued message belong to this thread.
        unsafe { IsDialogMessageW(d.hwnd.get(), message).as_bool() }
    })
}

fn send(hwnd: HWND, id: i32, msg: u32, w: usize, l: isize) -> isize {
    // SAFETY: callers use standard dialog controls and synchronous messages.
    unsafe { SendDlgItemMessageW(hwnd, id, msg, WPARAM(w), LPARAM(l)).0 }
}

impl Dialog {
    fn initialize(&self, hwnd: HWND) {
        self.hwnd.set(hwnd);
        for (i, Field(_, _, input)) in self.fields.iter().enumerate() {
            let id = 100 + i as i32;
            match input {
                Input::Choice(items, selected) => {
                    for (name, _) in *items {
                        let name: Vec<_> = name.encode_utf16().chain(Some(0)).collect();
                        send(hwnd, id, CB_ADDSTRING, 0, name.as_ptr() as isize);
                    }
                    send(hwnd, id, CB_SETCURSEL, *selected, 0);
                }
                Input::Check(value) => {
                    send(hwnd, id, BM_SETCHECK, usize::from(*value), 0);
                }
                _ => {}
            }
        }
    }

    fn changes(&self) -> Result<Changes, String> {
        let hwnd = self.hwnd.get();
        let mut changes = Vec::new();
        for (i, Field(key, label, input)) in self.fields.iter().enumerate() {
            let id = 100 + i as i32;
            let value = match input {
                Input::Choice(items, old) => {
                    let index = send(hwnd, id, CB_GETCURSEL, 0, 0) as usize;
                    if index == *old {
                        continue;
                    }
                    items
                        .get(index)
                        .ok_or_else(|| format!("Choose a value for {label}."))?
                        .1
                        .into()
                }
                Input::Check(old) => {
                    let enabled = send(hwnd, id, BM_GETCHECK, 0, 0) != 0;
                    if enabled == *old {
                        continue;
                    }
                    enabled.into()
                }
                Input::Number(..) | Input::Text(_) => {
                    // SAFETY: id identifies our edit control; the API bounds the write to buf.
                    let text = unsafe {
                        let child = GetDlgItem(Some(hwnd), id).map_err(|e| e.to_string())?;
                        let mut buf = vec![0; GetWindowTextLengthW(child) as usize + 1];
                        let n = GetDlgItemTextW(hwnd, id, &mut buf) as usize;
                        String::from_utf16_lossy(&buf[..n]).trim().to_owned()
                    };
                    if let Input::Number(old, max) = input {
                        let value = text
                            .parse::<u16>()
                            .ok()
                            .filter(|n| n <= max)
                            .ok_or_else(|| format!("{label} must be between 0 and {max}."))?;
                        if value == *old {
                            continue;
                        }
                        i64::from(value).into()
                    } else {
                        if matches!(input,Input::Text(old) if old==&text) {
                            continue;
                        }
                        if *key == "shortcut" {
                            text.parse::<Shortcut>()?;
                        } else if text.len() > 512 || text.chars().any(char::is_control) {
                            return Err("Enter an application ID of up to 512 bytes.".into());
                        }
                        text.into()
                    }
                }
            };
            changes.push((*key, value));
        }
        Ok(changes)
    }
}

unsafe extern "system" fn dialog_proc(hwnd: HWND, msg: u32, w: WPARAM, _: LPARAM) -> isize {
    let Some(dialog) = OPEN.with_borrow(Clone::clone) else {
        return 0;
    };
    match msg {
        WM_INITDIALOG => {
            dialog.initialize(hwnd);
            1
        }
        WM_NCDESTROY => {
            OPEN.take();
            0
        }
        WM_COMMAND if w.0 as u16 == IDOK.0 as u16 => {
            let result = dialog.changes().and_then(|changes| {
                if changes.is_empty() {
                    Ok(())
                } else {
                    (dialog.save)(changes)
                }
            });
            // SAFETY: hwnd is live during this callback. MessageBox copies the bounded strings synchronously.
            unsafe {
                if let Err(e) = result {
                    let text: Vec<_> = e.encode_utf16().chain(Some(0)).collect();
                    MessageBoxW(
                        Some(hwnd),
                        PCWSTR(text.as_ptr()),
                        windows::core::w!("Media Flyout settings"),
                        MB_OK | MB_ICONWARNING,
                    );
                } else {
                    let _ = DestroyWindow(hwnd);
                }
            }
            1
        }
        WM_CLOSE | WM_COMMAND if msg == WM_CLOSE || w.0 as u16 == IDCANCEL.0 as u16 => {
            // SAFETY: destroying this modeless dialog also clears OPEN via WM_NCDESTROY.
            unsafe {
                let _ = DestroyWindow(hwnd);
            }
            1
        }
        _ => 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn native_form_loads_edits_saves_and_cancels() {
        let saved = Rc::new(RefCell::new(Vec::new()));
        let capture = saved.clone();
        let hwnd = create_dialog(&Settings::default(), move |changes| {
            *capture.borrow_mut() = changes;
            Ok(())
        })
        .expect("create hidden settings window");
        let dialog = OPEN.with_borrow(Clone::clone).unwrap();
        assert!(dialog.changes().unwrap().is_empty());
        assert_eq!(dialog.hwnd.get(), hwnd);
        assert_eq!(
            create_dialog(&Settings::default(), |_| unreachable!()).unwrap(),
            hwnd
        );
        // SAFETY: the hidden dialog and its edit controls are owned by this test thread.
        unsafe {
            SetDlgItemTextW(hwnd, 103, windows::core::w!("257")).unwrap();
            assert!(dialog.changes().is_err());
            SetDlgItemTextW(hwnd, 103, windows::core::w!("42")).unwrap();
            SendMessageW(hwnd, WM_COMMAND, Some(WPARAM(IDOK.0 as usize)), None);
        }
        assert_eq!(saved.borrow().len(), 1);
        assert_eq!(saved.borrow()[0].0, "margin");
        assert_eq!(saved.borrow()[0].1.as_integer(), Some(42));
        assert!(OPEN.with_borrow(Option::is_none));
        let hwnd = create_dialog(&Settings::default(), |_| panic!("cancel saved changes")).unwrap();
        // SAFETY: hwnd identifies the second dialog on this thread.
        unsafe {
            SetDlgItemTextW(hwnd, 103, windows::core::w!("24")).unwrap();
            SendMessageW(hwnd, WM_COMMAND, Some(WPARAM(IDCANCEL.0 as usize)), None);
        }
        assert!(OPEN.with_borrow(Option::is_none));
    }
}
