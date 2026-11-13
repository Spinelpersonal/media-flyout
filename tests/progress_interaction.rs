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
