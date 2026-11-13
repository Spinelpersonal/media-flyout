#![cfg(windows)]

use std::os::windows::process::CommandExt;
use std::{
    path::Path,
    process::{Child, Command, Stdio},
    thread,
    time::{Duration, Instant},
};
use windows::{
    Win32::{
        Foundation::{HWND, RECT},
        UI::WindowsAndMessaging::{
            FindWindowW, GWL_EXSTYLE, GetWindowLongPtrW, GetWindowRect, IsWindowVisible,
            WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW,
        },
    },
    core::w,
};

struct App(Child);
impl Drop for App {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

struct HiddenApp(windows::Win32::Foundation::HANDLE);

impl HiddenApp {
    fn start(config: &Path) -> Self {
        use windows::{
            Win32::System::Threading::{
                CREATE_NO_WINDOW, CreateProcessW, PROCESS_INFORMATION, STARTF_USESHOWWINDOW,
                STARTUPINFOW,
            },
            core::{PCWSTR, PWSTR},
        };
        let mut command: Vec<u16> = format!(
            "\"{}\" --config \"{}\"",
            env!("CARGO_BIN_EXE_media-flyout"),
            config.display()
        )
        .encode_utf16()
        .chain(Some(0))
        .collect();
        let startup = STARTUPINFOW {
            cb: std::mem::size_of::<STARTUPINFOW>() as u32,
            dwFlags: STARTF_USESHOWWINDOW,
            wShowWindow: windows::Win32::UI::WindowsAndMessaging::SW_HIDE.0 as u16,
            ..Default::default()
        };
        let mut process = PROCESS_INFORMATION::default();
        // SAFETY: The writable, null-terminated command and initialized structures
        // live through CreateProcess. We own both returned handles.
        unsafe {
            CreateProcessW(
                PCWSTR::null(),
                Some(PWSTR(command.as_mut_ptr())),
                None,
                None,
                false,
                CREATE_NO_WINDOW,
                None,
                PCWSTR::null(),
                &startup,
                &mut process,
            )
            .expect("start with hidden-window startup info");
            windows::Win32::Foundation::CloseHandle(process.hThread).expect("close thread handle");
        }
        Self(process.hProcess)
    }

    fn exited(&self) -> bool {
        // SAFETY: We own the process handle, and a zero timeout never blocks.
        unsafe {
            windows::Win32::System::Threading::WaitForSingleObject(self.0, 0)
                == windows::Win32::Foundation::WAIT_OBJECT_0
        }
    }
}

impl Drop for HiddenApp {
    fn drop(&mut self) {
        // SAFETY: These are our own child process and handle. Ensure a failed test
        // cannot leave a background instance holding the singleton/hotkey.
        unsafe {
            if !self.exited() {
                let _ = windows::Win32::System::Threading::TerminateProcess(self.0, 1);
                windows::Win32::System::Threading::WaitForSingleObject(self.0, 5000);
            }
            let _ = windows::Win32::Foundation::CloseHandle(self.0);
        }
    }
}

#[test]
#[ignore = "Requires the interactive Windows desktop, shell, and no running flyout; sends a test shortcut"]
fn shortcut_works_before_first_show() {
    use media_flyout::hotkey::RegisteredHotkey;
    use windows::Win32::UI::Input::KeyboardAndMouse::{
        GetAsyncKeyState, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYEVENTF_KEYUP, SendInput,
        VIRTUAL_KEY, VK_CONTROL, VK_LWIN, VK_MENU, VK_RWIN, VK_SHIFT,
    };

    assert!(
        window().is_none(),
        "Close the existing flyout before this test"
    );
    // Never release modifiers physically held by the person running this test.
    for key in [VK_CONTROL, VK_MENU, VK_SHIFT, VK_LWIN, VK_RWIN] {
        // SAFETY: Read-only query of the interactive desktop's key state.
        let state = unsafe { GetAsyncKeyState(i32::from(key.0)) };
        assert!(state >= 0, "Release modifier keys before running this test");
    }
    let shortcut = "Ctrl+Alt+Shift+F24".parse().expect("shortcut");
    let available = RegisteredHotkey::register(HWND::default(), 99, shortcut)
        .expect("test shortcut must be free");
    drop(available);
    let config = std::env::temp_dir().join(format!(
        "media-flyout-first-shortcut-{}.toml",
        std::process::id()
    ));
    for animations in [true, false] {
        std::fs::write(&config, format!("shortcut = 'Ctrl+Alt+Shift+F24'\ndismiss_after_seconds = 0\ndismiss_on_outside_click = false\nanimations = {animations}\n")).expect("isolated settings");
        let app = HiddenApp::start(&config);
        wait_until(Duration::from_secs(10), || {
            assert!(
                !app.exited(),
                "flyout exited before registering the shortcut"
            );
            window().is_some() && RegisteredHotkey::register(HWND::default(), 99, shortcut).is_err()
        });
        let hwnd = window().expect("window");
        // SAFETY: Read-only query of the test application's window.
        assert!(!unsafe { IsWindowVisible(hwnd) }.as_bool(), "starts hidden");
        // Wait past initialization without warming the window via show or reload.
        thread::sleep(Duration::from_secs(1));
        let keys = [VK_CONTROL, VK_MENU, VK_SHIFT, VIRTUAL_KEY(0x87)];
        let inputs: Vec<INPUT> = keys
            .iter()
            .copied()
            .map(|key| (key, false))
            .chain(keys.iter().rev().copied().map(|key| (key, true)))
            .map(|(key, up)| INPUT {
                r#type: INPUT_KEYBOARD,
                Anonymous: INPUT_0 {
                    ki: KEYBDINPUT {
                        wVk: key,
                        dwFlags: if up {
                            KEYEVENTF_KEYUP
                        } else {
                            Default::default()
                        },
                        ..Default::default()
                    },
                },
            })
            .collect();
        // SAFETY: All INPUTs are initialized keyboard events, using an isolated
        // chord reserved by the test app, with balanced modifier/key releases.
        let sent = unsafe { SendInput(&inputs, std::mem::size_of::<INPUT>() as i32) };
        assert_eq!(sent as usize, inputs.len());
        // SAFETY: Read-only visibility query; no tray interaction or reload.
        wait_until(Duration::from_secs(5), || unsafe {
            IsWindowVisible(hwnd).as_bool()
        });
        assert!(
            command(&config)
                .arg("--quit")
                .status()
                .expect("exit")
                .success()
        );
        wait_until(Duration::from_secs(10), || app.exited());
    }
    std::fs::remove_file(config).expect("remove settings");
}

#[test]
#[ignore = "Requires the interactive Windows desktop, shell, and no running flyout"]
fn shortcut_conflict_and_reload_preserve_working_registration() {
    use media_flyout::hotkey::{RegisteredHotkey, Shortcut};
    assert!(
        window().is_none(),
        "Close the existing flyout before this test"
    );
    let first: Shortcut = "Ctrl+Alt+Shift+F23".parse().expect("shortcut");
    let second: Shortcut = "Ctrl+Alt+Shift+F22".parse().expect("shortcut");
    let blocker =
        RegisteredHotkey::register(HWND::default(), 97, first).expect("reserve test shortcut");
    let config =
        std::env::temp_dir().join(format!("media-flyout-reload-{}.toml", std::process::id()));
    std::fs::write(&config, "shortcut = 'Ctrl+Alt+Shift+F23'\ndismiss_after_seconds = 0\ndismiss_on_outside_click = false\nanimations = false\n").expect("settings");
    let mut app = App(command(&config).arg("--show").spawn().expect("start"));
    wait_until(Duration::from_secs(10), || {
        window().is_some_and(|hwnd| {
            // SAFETY: Read-only visibility query on the test application's HWND.
            unsafe { IsWindowVisible(hwnd).as_bool() }
        })
    });
    assert!(
        app.0.try_wait().expect("status").is_none(),
        "shortcut conflict does not terminate the app"
    );
    drop(blocker);
    assert!(
        command(&config)
            .arg("--reload")
            .status()
            .expect("reload")
            .success()
    );
    wait_until(Duration::from_secs(5), || {
        RegisteredHotkey::register(HWND::default(), 99, first).is_err()
    });
    let blocker =
        RegisteredHotkey::register(HWND::default(), 98, second).expect("reserve new shortcut");
    let hwnd = window().expect("window");
    let mut before = RECT::default();
    // SAFETY: Initialized RECT output and live test HWND.
    unsafe {
        GetWindowRect(hwnd, &mut before).expect("window rectangle");
    }
    std::fs::write(&config, "shortcut = 'Ctrl+Alt+Shift+F22'\nposition = 'bottom-right'\ndismiss_after_seconds = 0\ndismiss_on_outside_click = false\nanimations = false\n").expect("settings");
    assert!(
        command(&config)
            .arg("--reload")
            .status()
            .expect("reload")
            .success()
    );
    wait_until(Duration::from_secs(5), || {
        let mut after = RECT::default();
        // SAFETY: Read-only geometry query proves the reload message was processed.
        unsafe {
            GetWindowRect(hwnd, &mut after).is_ok()
                && (after.left != before.left || after.top != before.top)
        }
    });
    assert!(
        RegisteredHotkey::register(HWND::default(), 99, first).is_err(),
        "old shortcut retained when replacement conflicts"
    );
    drop(blocker);
    assert!(
        command(&config)
            .arg("--reload")
            .status()
            .expect("reload")
            .success()
    );
    wait_until(Duration::from_secs(5), || {
        RegisteredHotkey::register(HWND::default(), 99, first).is_ok()
    });
    assert!(
        RegisteredHotkey::register(HWND::default(), 99, second).is_err(),
        "replacement shortcut registered"
    );
    assert!(
        command(&config)
            .arg("--quit")
            .status()
            .expect("exit")
            .success()
    );
    wait_until(Duration::from_secs(10), || {
        app.0.try_wait().expect("status").is_some()
    });
    assert!(
        RegisteredHotkey::register(HWND::default(), 99, second).is_ok(),
        "replacement released on exit"
    );
    std::fs::remove_file(config).expect("remove settings");
}

fn command(config: &Path) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_media-flyout"));
    command
        .arg("--config")
        .arg(config)
        .creation_flags(0x08000000)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    command
}

fn window() -> Option<HWND> {
    // SAFETY: FindWindow performs a read-only lookup by class and title.
    unsafe { FindWindowW(w!("MediaFlyoutWindow"), w!("Media Flyout")).ok() }
}

fn wait_until(timeout: Duration, mut condition: impl FnMut() -> bool) {
    let deadline = Instant::now() + timeout;
    while !condition() {
        assert!(
            Instant::now() < deadline,
            "Timed out waiting for flyout state"
        );
        thread::sleep(Duration::from_millis(20));
    }
}

#[test]
#[ignore = "Requires the interactive Windows desktop, shell, and no running flyout"]
fn background_visibility_single_instance_and_clean_exit() {
    assert!(
        window().is_none(),
        "Close the existing flyout before this test"
    );
    let config = std::env::temp_dir().join(format!(
        "media-flyout-lifecycle-{}.toml",
        std::process::id()
    ));
    std::fs::write(&config, "shortcut = 'Ctrl+Alt+Shift+F24'\ndismiss_after_seconds = 10\ndismiss_on_outside_click = false\nanimations = false\n").expect("isolated settings");
    let shortcut = "Ctrl+Alt+Shift+F24".parse().expect("shortcut");
    let initial = media_flyout::hotkey::RegisteredHotkey::register(HWND::default(), 99, shortcut)
        .expect("test shortcut must be free");
    drop(initial);
    let mut app = App(command(&config).spawn().expect("start flyout"));
    wait_until(Duration::from_secs(10), || window().is_some());
    let hwnd = window().expect("native window");
    // SAFETY: These read-only queries inspect the test process's live HWND.
    unsafe {
        assert!(!IsWindowVisible(hwnd).as_bool(), "starts hidden");
        let style = GetWindowLongPtrW(hwnd, GWL_EXSTYLE) as u32;
        assert_ne!(
            style & WS_EX_TOOLWINDOW.0,
            0,
            "excluded from taskbar/Alt-Tab"
        );
        assert_ne!(style & WS_EX_NOACTIVATE.0, 0, "does not take focus");
    }
    assert!(
        command(&config)
            .arg("--show")
            .status()
            .expect("show command")
            .success()
    );
    // SAFETY: This queries visibility only; user input is never synthesized.
    wait_until(Duration::from_secs(5), || unsafe {
        IsWindowVisible(hwnd).as_bool()
    });
    let shown_at = Instant::now();
    assert!(
        media_flyout::hotkey::RegisteredHotkey::register(HWND::default(), 99, shortcut).is_err(),
        "global shortcut is registered by the app"
    );
    assert!(
        app.0.try_wait().expect("process status").is_none(),
        "original process reused"
    );
    // SAFETY: This queries visibility only while waiting for the real dismiss timer.
    wait_until(Duration::from_secs(12), || unsafe {
        !IsWindowVisible(hwnd).as_bool()
    });
    assert!(
        shown_at.elapsed() >= Duration::from_secs(9),
        "full dismiss interval with outside clicks disabled"
    );
    assert!(
        app.0.try_wait().expect("process status").is_none(),
        "dismiss keeps background process alive"
    );
    assert!(
        command(&config)
            .arg("--quit")
            .status()
            .expect("exit command")
            .success()
    );
    wait_until(Duration::from_secs(10), || {
        app.0.try_wait().expect("process status").is_some()
    });
    assert!(window().is_none());
    let released = media_flyout::hotkey::RegisteredHotkey::register(HWND::default(), 99, shortcut)
        .expect("shortcut released on exit");
    drop(released);
    std::fs::remove_file(config).expect("remove isolated settings");
}
