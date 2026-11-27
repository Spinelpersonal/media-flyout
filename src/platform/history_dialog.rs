use super::dialog::{BUTTON, COMBO, EDIT, LABEL, Template};
use crate::{
    history::{Entry, Filter, History, Result},
    media::timestamp,
};
use chrono::{Local, TimeDelta, Utc};
use std::{
    cell::{Cell, RefCell},
    ffi::OsString,
    os::windows::ffi::OsStringExt,
    path::PathBuf,
    rc::Rc,
    sync::Arc,
    time::Duration,
};
use windows::{
    Win32::{
        Foundation::{HWND, LPARAM, RECT, WPARAM},
        UI::{
            Controls::{Dialogs::*, *},
            Input::KeyboardAndMouse::EnableWindow,
            WindowsAndMessaging::*,
        },
    },
    core::{HSTRING, PWSTR, w},
};

const SEARCH: u16 = 100;
const PERIOD: u16 = 101;
const LIST: u16 = 102;
const STATUS: u16 = 103;
const PREVIOUS: u16 = 104;
const NEXT: u16 = 105;
const EXPORT: u16 = 106;
const CLEAR: u16 = 107;
const REFRESH: u16 = 108;
const PAGE: i64 = 50;

struct Panel {
    hwnd: HWND,
    history: Arc<History>,
    offset: Cell<i64>,
    revision: Cell<u64>,
}
thread_local! { static OPEN: RefCell<Option<Rc<Panel>>> = const { RefCell::new(None) }; }

pub(crate) fn open_history(history: Arc<History>) -> Result<()> {
    let panel = match OPEN.with_borrow(Clone::clone) {
        Some(panel) => panel,
        None => create_panel(history)?,
    };
    panel.refresh();
    // SAFETY: OPEN holds a live modeless dialog on the calling UI thread.
    unsafe {
        let _ = ShowWindow(panel.hwnd, SW_SHOW);
        let _ = SetForegroundWindow(panel.hwnd);
    }
    Ok(())
}

fn create_panel(history: Arc<History>) -> Result<Rc<Panel>> {
    let mut t = Template::new("Listening history", 492);
    t.item(
        0xffff,
        LABEL,
        "&Search tracks, artists or players",
        [10, 8, 290, 10],
        0,
    );
    t.item(
        SEARCH,
        EDIT,
        "",
        [10, 21, 302, 15],
        WS_TABSTOP.0 | WS_BORDER.0 | ES_AUTOHSCROLL as u32,
    );
    t.item(0xffff, LABEL, "Time &period", [320, 8, 162, 10], 0);
    t.item(
        PERIOD,
        COMBO,
        "",
        [320, 21, 162, 80],
        WS_TABSTOP.0 | CBS_DROPDOWNLIST as u32,
    );
    t.item_with_class(
        LIST,
        "SysListView32".encode_utf16().chain(Some(0)),
        "",
        [10, 44, 472, 158],
        WS_TABSTOP.0 | WS_BORDER.0 | LVS_REPORT | LVS_SINGLESEL | LVS_SHOWSELALWAYS,
    );
    t.item(STATUS, LABEL, "", [10, 209, 334, 20], 0);
    t.item(
        PREVIOUS,
        BUTTON,
        "Previous",
        [365, 205, 56, 16],
        WS_TABSTOP.0,
    );
    t.item(NEXT, BUTTON, "Next", [426, 205, 56, 16], WS_TABSTOP.0);
    t.item(
        EXPORT,
        BUTTON,
        "Export CSV…",
        [10, 236, 72, 17],
        WS_TABSTOP.0,
    );
    t.item(
        CLEAR,
        BUTTON,
        "Clear history…",
        [87, 236, 75, 17],
        WS_TABSTOP.0,
    );
    t.item(REFRESH, BUTTON, "Refresh", [167, 236, 55, 17], WS_TABSTOP.0);
    t.item(
        IDCANCEL.0 as u16,
        BUTTON,
        "Close",
        [426, 236, 56, 17],
        WS_TABSTOP.0,
    );
    let layout = t.finish(262);
    // SAFETY: The common-control descriptor and aligned template live through creation.
    let hwnd = unsafe {
        InitCommonControlsEx(&INITCOMMONCONTROLSEX {
            dwSize: size_of::<INITCOMMONCONTROLSEX>() as u32,
            dwICC: ICC_LISTVIEW_CLASSES,
        })
        .ok()?;
        CreateDialogIndirectParamW(
            None,
            layout.as_ptr().cast(),
            None,
            Some(dialog_proc),
            LPARAM(0),
        )?
    };
    let panel = Rc::new(Panel {
        hwnd,
        history,
        offset: Cell::new(0),
        revision: Cell::new(0),
    });
    panel.initialize();
    OPEN.set(Some(panel.clone()));
    Ok(panel)
}

pub(crate) fn refresh_history() {
    if let Some(panel) = OPEN.with_borrow(Clone::clone)
        && panel.revision.get() != panel.history.revision()
    {
        panel.refresh();
    }
}

pub(super) fn handle_message(message: &MSG) -> bool {
    let panel = OPEN.with_borrow(Clone::clone);
    // SAFETY: The registered HWND and queued message belong to this thread.
    panel.is_some_and(|p| unsafe { IsDialogMessageW(p.hwnd, message).as_bool() })
}

impl Panel {
    fn send(&self, id: u16, msg: u32, w: usize, l: isize) -> isize {
        // SAFETY: Callers use synchronous messages to this dialog's controls.
        unsafe { SendDlgItemMessageW(self.hwnd, id.into(), msg, WPARAM(w), LPARAM(l)).0 }
    }
    fn initialize(&self) {
        for period in ["All time", "Last 24 hours", "Last 7 days", "Last 30 days"] {
            self.send(
                PERIOD,
                CB_ADDSTRING,
                0,
                HSTRING::from(period).as_ptr() as isize,
            );
        }
        self.send(PERIOD, CB_SETCURSEL, 0, 0);
        self.send(
            LIST,
            LVM_SETEXTENDEDLISTVIEWSTYLE,
            0,
            (LVS_EX_FULLROWSELECT | LVS_EX_LABELTIP | LVS_EX_DOUBLEBUFFER) as isize,
        );
        let mut rect = RECT::default();
        // SAFETY: LIST belongs to this dialog and rect is writable.
        unsafe {
            if let Ok(list) = GetDlgItem(Some(self.hwnd), LIST.into()) {
                let _ = GetClientRect(list, &mut rect);
            }
        }
        for (index, (label, percentage)) in [
            ("Started (local)", 21),
            ("Track", 30),
            ("Artist", 20),
            ("Player", 18),
            ("Listened", 11),
        ]
        .into_iter()
        .enumerate()
        {
            let label = HSTRING::from(label);
            let column = LVCOLUMNW {
                mask: LVCF_TEXT | LVCF_WIDTH,
                cx: (rect.right - 20) * percentage / 100,
                pszText: PWSTR(label.as_ptr().cast_mut()),
                ..Default::default()
            };
            self.send(LIST, LVM_INSERTCOLUMNW, index, &column as *const _ as isize);
        }
    }

    fn filter(&self) -> Filter {
        let days = match self.send(PERIOD, CB_GETCURSEL, 0, 0) {
            1 => 1,
            2 => 7,
            3 => 30,
            _ => 0,
        };
        // SAFETY: The edit control belongs to hwnd and the API bounds writes to the buffer.
        let search = unsafe {
            let length = self.send(SEARCH, WM_GETTEXTLENGTH, 0, 0) as usize;
            let mut buffer = vec![0; length + 1];
            let n = GetDlgItemTextW(self.hwnd, SEARCH.into(), &mut buffer);
            String::from_utf16_lossy(&buffer[..n as usize])
        };
        Filter {
            search,
            since: if days == 0 {
                i64::MIN
            } else {
                (Utc::now() - TimeDelta::days(days)).timestamp_millis()
            },
        }
    }

    fn add_row(&self, index: i32, entry: Entry) {
        for (column, text) in [
            entry
                .started
                .with_timezone(&Local)
                .format("%Y-%m-%d %H:%M:%S")
                .to_string(),
            entry.title,
            entry.artist,
            entry.player,
            timestamp(Duration::from_millis(entry.duration_ms)),
        ]
        .into_iter()
        .enumerate()
        {
            let text = HSTRING::from(text);
            let item = LVITEMW {
                mask: LVIF_TEXT,
                iItem: index,
                iSubItem: column as i32,
                pszText: PWSTR(text.as_ptr().cast_mut()),
                ..Default::default()
            };
            self.send(
                LIST,
                if column == 0 {
                    LVM_INSERTITEMW
                } else {
                    LVM_SETITEMTEXTW
                },
                index as usize,
                &item as *const _ as isize,
            );
        }
    }

    fn status(&self, message: &str) {
        // SAFETY: SetDlgItemTextW copies the string synchronously.
        unsafe {
            let _ = SetDlgItemTextW(self.hwnd, STATUS.into(), &HSTRING::from(message));
        }
    }
    fn enable(&self, id: u16, enabled: bool) {
        // SAFETY: The child belongs to this live dialog.
        unsafe {
            if let Ok(child) = GetDlgItem(Some(self.hwnd), id.into()) {
                let _ = EnableWindow(child, enabled);
            }
        }
    }

    fn refresh(&self) {
        self.revision.set(self.history.revision());
        let offset = self.offset.get();
        self.send(LIST, LVM_DELETEALLITEMS, 0, 0);
        let mut count = 0;
        let result = self.history.view().and_then(|view| {
            view.rows(&self.filter(), offset, PAGE + 1, |entry| {
                if count < PAGE {
                    self.add_row(count as i32, entry);
                }
                count += 1;
                Ok(())
            })
        });
        self.enable(PREVIOUS, offset > 0);
        self.enable(NEXT, count > PAGE);
        self.enable(EXPORT, result.is_ok() && count > 0);
        let recording = if self.history.enabled() {
            "Recording on"
        } else {
            "Recording off · Enable in settings"
        };
        let status = match result {
            Err(error) => format!("Cannot read history: {error}"),
            Ok(()) if count == 0 => format!("{recording}   |   No matching listens"),
            Ok(()) => format!(
                "{recording}   |   Showing {}–{}",
                offset + 1,
                offset + count.min(PAGE)
            ),
        };
        self.status(&status);
    }

    fn export(&self) -> Result<()> {
        let mut name = vec![0u16; 32768];
        for (to, from) in name.iter_mut().zip("listening-history.csv".encode_utf16()) {
            *to = from;
        }
        let mut dialog = OPENFILENAMEW {
            lStructSize: size_of::<OPENFILENAMEW>() as u32,
            hwndOwner: self.hwnd,
            lpstrFilter: w!("CSV files (*.csv)\0*.csv\0"),
            lpstrFile: PWSTR(name.as_mut_ptr()),
            nMaxFile: name.len() as u32,
            lpstrDefExt: w!("csv"),
            Flags: OFN_OVERWRITEPROMPT | OFN_PATHMUSTEXIST | OFN_NOCHANGEDIR | OFN_EXPLORER,
            ..Default::default()
        };
        // SAFETY: All pointers remain valid throughout the modal save dialog.
        unsafe {
            if !GetSaveFileNameW(&mut dialog).as_bool() {
                let code = CommDlgExtendedError().0;
                if code == 0 {
                    return Ok(());
                }
                return Err(format!("Save dialog error {code:#x}").into());
            }
        }
        let end = name.iter().position(|&c| c == 0).unwrap_or(name.len());
        let path = PathBuf::from(OsString::from_wide(&name[..end]));
        self.history.view()?.export(&self.filter(), &path)?;
        self.status("Exported matching listens to CSV.");
        Ok(())
    }
}

unsafe extern "system" fn dialog_proc(hwnd: HWND, msg: u32, w: WPARAM, _: LPARAM) -> isize {
    if msg == WM_INITDIALOG {
        return 1;
    }
    let Some(panel) = OPEN.with_borrow(Clone::clone) else {
        return 0;
    };
    let id = w.0 as u16;
    let notification = (w.0 >> 16) as u32;
    match msg {
        WM_NCDESTROY => {
            OPEN.take();
            0
        }
        WM_CLOSE | WM_COMMAND if msg == WM_CLOSE || id == IDCANCEL.0 as u16 => {
            // SAFETY: Destroy the modeless window on its owning thread.
            unsafe {
                let _ = DestroyWindow(hwnd);
            }
            1
        }
        WM_COMMAND => {
            match id {
                SEARCH if notification == EN_CHANGE => panel.offset.set(0),
                PERIOD if notification == CBN_SELCHANGE => panel.offset.set(0),
                PREVIOUS => panel.offset.set((panel.offset.get() - PAGE).max(0)),
                NEXT => panel.offset.set(panel.offset.get() + PAGE),
                REFRESH => {}
                CLEAR => {
                    // SAFETY: Modal confirmation is owned by the live panel.
                    if unsafe {
                        MessageBoxW(
                            Some(hwnd),
                            w!(
                                "Delete all listening history, including entries outside the current filter?"
                            ),
                            w!("Clear listening history"),
                            MB_YESNO | MB_ICONWARNING | MB_DEFBUTTON2,
                        )
                    } != IDYES
                    {
                        return 1;
                    }
                    if let Err(error) = panel.history.clear() {
                        panel.status(&format!("Cannot clear history: {error}"));
                        return 1;
                    }
                    panel.offset.set(0);
                }
                EXPORT => {
                    if let Err(error) = panel.export() {
                        panel.status(&format!("Cannot export CSV: {error}"));
                    }
                    return 1;
                }
                _ => return 0,
            }
            panel.refresh();
            1
        }
        _ => 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::media::{PlaybackState, SessionInfo, Track};

    #[test]
    fn native_list_shows_and_filters_recorded_tracks() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let history = Arc::new(History::new(Some(dir.path().join("history.sqlite3"))));
        history.set_enabled(true)?;
        let session = SessionInfo {
            id: 1,
            display_name: "Fixture player".into(),
            state: PlaybackState::Playing,
            ..Default::default()
        };
        for n in 0..=PAGE {
            let track = Track {
                title: format!("Song #{n}"),
                ..Default::default()
            };
            history.observe(std::iter::once((&session, &track)))?;
        }
        let panel = create_panel(history.clone())?;
        panel.refresh();
        assert_eq!(panel.send(LIST, LVM_GETITEMCOUNT, 0, 0), PAGE as isize);
        panel.send(NEXT, BM_CLICK, 0, 0);
        assert_eq!(panel.send(LIST, LVM_GETITEMCOUNT, 0, 0), 1);
        let path = dir.path().join("all.csv");
        history.view()?.export(&panel.filter(), &path)?;
        assert_eq!(
            csv::Reader::from_path(path)?.records().count(),
            PAGE as usize + 1
        );
        // SAFETY: The hidden dialog and edit control belong to this test thread.
        unsafe {
            SetDlgItemTextW(panel.hwnd, SEARCH.into(), w!("song #22"))?;
        }
        assert_eq!(panel.send(LIST, LVM_GETITEMCOUNT, 0, 0), 1);
        history.clear()?;
        refresh_history();
        assert_eq!(panel.send(LIST, LVM_GETITEMCOUNT, 0, 0), 0);
        // SAFETY: Destroy the hidden modeless dialog on its owning test thread.
        unsafe {
            DestroyWindow(panel.hwnd)?;
        }
        assert!(OPEN.with_borrow(|p| p.is_none()));
        Ok(())
    }
}
