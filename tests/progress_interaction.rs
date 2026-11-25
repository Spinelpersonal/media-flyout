use std::{cell::RefCell, rc::Rc};

use media_flyout::ui::Flyout;
use slint::{
    ComponentHandle, LogicalPosition, PhysicalSize, PlatformError,
    platform::{
        Platform, PointerEventButton, WindowAdapter, WindowEvent,
        software_renderer::{MinimalSoftwareWindow, RepaintBufferType},
    },
};

struct Headless(Rc<MinimalSoftwareWindow>);

impl Platform for Headless {
    fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, PlatformError> {
        Ok(self.0.clone())
    }
}

#[test]
fn application_volume_supports_pointer_and_keyboard_and_skips_unmatched_apps() {
    use slint::platform::Key;
    use std::cell::Cell;
    let window = MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer);
    slint::platform::set_platform(Box::new(Headless(window.clone()))).unwrap();
    let flyout = Flyout::new().unwrap();
    flyout.window().set_size(PhysicalSize::new(452, 196));
    flyout.set_keyboard_mode(true);
    flyout.show().unwrap();
    let value = Rc::new(Cell::new(-1));
    let result = value.clone();
    flyout.on_set_volume(move |percent| result.set(percent));
    let result = value.clone();
    flyout.on_step_volume(move |delta| result.set(result.get() + delta));
    let muted = Rc::new(Cell::new(false));
    let result = muted.clone();
    flyout.on_toggle_mute(move || result.set(!result.get()));
    let pointer = |pressed, x| {
        let position = LogicalPosition::new(x, 121.0);
        let button = PointerEventButton::Left;
        window.dispatch_event(if pressed {
            WindowEvent::PointerPressed { position, button }
        } else {
            WindowEvent::PointerReleased { position, button }
        });
    };
    pointer(true, 245.0);
    pointer(false, 245.0);
    pointer(true, 224.0);
    pointer(false, 224.0);
    assert!(!muted.get() && value.get() == -1);
    flyout.set_volume_available(true);
    pointer(true, 245.0);
    assert!(flyout.get_adjusting_volume());
    pointer(false, 313.0);
    assert_eq!(value.get(), 50);
    assert!(!flyout.get_adjusting_volume());
    flyout.invoke_focus_source();
    let key = |k: Key| {
        window.dispatch_event(WindowEvent::KeyPressed { text: k.into() });
        window.dispatch_event(WindowEvent::KeyReleased { text: k.into() });
    };
    key(Key::Tab);
    key(Key::Return);
    assert!(muted.get());
    key(Key::Tab);
    key(Key::LeftArrow);
    assert_eq!(value.get(), 45);
    key(Key::RightArrow);
    assert_eq!(value.get(), 50);
    pointer(true, 245.0);
    flyout.set_adjusting_volume(false); // Controller invalidates a drag on target change.
    pointer(false, 381.0);
    assert_eq!(value.get(), 50);
}

#[test]
fn keyboard_traverses_available_controls_and_activates_the_highlighted_row() {
    use slint::{SharedString, VecModel, platform::Key};
    use std::cell::Cell;

    let window = MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer);
    slint::platform::set_platform(Box::new(Headless(window.clone()))).unwrap();
    let flyout = Flyout::new().unwrap();
    flyout.window().set_size(PhysicalSize::new(452, 196));
    flyout.set_keyboard_mode(true);
    flyout.set_can_toggle(true);
    flyout.set_has_timeline(true);
    flyout.set_can_seek(true);
    flyout
        .set_session_labels(Rc::new(VecModel::from(vec![SharedString::from("Player"); 8])).into());
    flyout.show().unwrap();
    flyout.invoke_focus_source();
    let press = |text: SharedString| {
        window.dispatch_event(WindowEvent::KeyPressed { text: text.clone() });
        window.dispatch_event(WindowEvent::KeyReleased { text });
    };
    let selected = Rc::new(Cell::new(-1));
    let result = selected.clone();
    flyout.on_select_session(move |index| result.set(index));
    press(Key::Return.into());
    assert!(flyout.get_session_menu());
    for _ in 0..7 {
        press(Key::DownArrow.into());
    }
    press(Key::UpArrow.into());
    press(" ".into());
    assert_eq!(selected.get(), 6);
    assert!(!flyout.get_session_menu());

    let toggled = Rc::new(Cell::new(false));
    let result = toggled.clone();
    flyout.on_toggle_playback(move || result.set(true));
    press(Key::Tab.into());
    press(Key::Return.into());
    assert!(
        toggled.get(),
        "skip unavailable lyrics and previous controls"
    );
    press(Key::Backtab.into());
    press(Key::Backtab.into());
    let step = Rc::new(Cell::new(0));
    let result = step.clone();
    flyout.on_step_seek(move |seconds| result.set(seconds));
    press(Key::LeftArrow.into());
    assert_eq!(step.get(), -5);
    press(Key::RightArrow.into());
    assert_eq!(step.get(), 5);

    let dismissed = Rc::new(Cell::new(false));
    let result = dismissed.clone();
    flyout.on_dismiss(move || result.set(true));
    flyout.invoke_focus_source();
    press(" ".into());
    flyout.invoke_escape();
    assert!(!flyout.get_session_menu() && !dismissed.get());
    flyout.invoke_escape();
    assert!(dismissed.get());
}

#[test]
fn progress_click_drag_edges_cancellation_and_disabled_player() {
    let window = MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer);
    slint::platform::set_platform(Box::new(Headless(window.clone()))).expect("headless platform");
    let flyout = Flyout::new().expect("flyout");
    flyout.window().set_size(PhysicalSize::new(452, 196));
    flyout.set_has_timeline(true);
    flyout.set_can_seek(true);
    flyout.set_progress(0.25);
    flyout.show().expect("show headless window");

    let previews = Rc::new(RefCell::new(Vec::new()));
    let values = previews.clone();
    flyout.on_seek_preview(move |fraction| values.borrow_mut().push(fraction));
    let commits = Rc::new(RefCell::new(0));
    let count = commits.clone();
    let weak = flyout.as_weak();
    flyout.on_seek_committed(move || {
        *count.borrow_mut() += 1;
        weak.unwrap().set_seeking(false);
    });
    let weak = flyout.as_weak();
    flyout.on_seek_cancelled(move || weak.unwrap().set_seeking(false));

    let press = |x, y| {
        window.dispatch_event(WindowEvent::PointerPressed {
            position: LogicalPosition::new(x, y),
            button: PointerEventButton::Left,
        })
    };
    let release = |x, y| {
        window.dispatch_event(WindowEvent::PointerReleased {
            position: LogicalPosition::new(x, y),
            button: PointerEventButton::Left,
        })
    };
    let move_to = |x, y| {
        window.dispatch_event(WindowEvent::PointerMoved {
            position: LogicalPosition::new(x, y),
        })
    };

    // The hit area extends above and below the thin visible track.
    press(400.0, 160.0);
    assert!(flyout.get_seeking());
    assert_eq!(previews.borrow().last(), Some(&0.75));
    assert_eq!(*commits.borrow(), 0, "preview does not seek yet");
    release(400.0, 160.0);
    assert_eq!(*commits.borrow(), 1, "click commits once");

    press(368.0, 168.0);
    move_to(336.0, 168.0);
    assert_eq!(previews.borrow().last(), Some(&0.25), "drag backwards");
    move_to(440.0, 168.0);
    assert_eq!(previews.borrow().last(), Some(&1.0), "clamp beyond end");
    move_to(290.0, 168.0);
    assert_eq!(previews.borrow().last(), Some(&0.0), "clamp before start");
    assert_eq!(*commits.borrow(), 1, "drag only previews");
    release(290.0, 168.0);
    assert_eq!(*commits.borrow(), 2, "release outside commits once");

    press(368.0, 168.0);
    flyout.set_seeking(false); // Controller cancels when the track/session changes.
    release(400.0, 168.0);
    assert_eq!(*commits.borrow(), 2, "cancelled drags do not seek");

    flyout.set_can_seek(false);
    let preview_count = previews.borrow().len();
    press(368.0, 168.0);
    release(400.0, 168.0);
    assert_eq!(previews.borrow().len(), preview_count);
    assert_eq!(*commits.borrow(), 2, "unsupported players stay read-only");

    // A rejected seek or a settings warning must not replace a valid timeline.
    flyout.set_can_seek(true);
    flyout.set_status("This player did not accept seeking".into());
    press(368.0, 168.0);
    assert!(
        flyout.get_seeking(),
        "status must leave progress interactive"
    );
    release(400.0, 168.0);
    assert_eq!(
        *commits.borrow(),
        3,
        "seeking can be retried after a rejection"
    );
    assert_eq!(previews.borrow().last(), Some(&0.75));
}
