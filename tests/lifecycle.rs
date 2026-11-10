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
        Foundation::HWND,
        UI::WindowsAndMessaging::{
            FindWindowW, GWL_EXSTYLE, GetWindowLongPtrW, IsWindowVisible, WS_EX_NOACTIVATE,
            WS_EX_TOOLWINDOW,
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
