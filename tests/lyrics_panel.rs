use media_flyout::ui::Flyout;
use slint::{
    ComponentHandle, LogicalPosition, PhysicalSize, PlatformError,
    platform::{
        Platform, PointerEventButton, WindowAdapter, WindowEvent,
        software_renderer::{MinimalSoftwareWindow, RepaintBufferType},
    },
};
use std::rc::Rc;

struct Headless(Rc<MinimalSoftwareWindow>);
impl Platform for Headless {
    fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, PlatformError> {
        Ok(self.0.clone())
    }
}

#[test]
fn opens_from_the_flyout_highlights_individual_words_and_scrolls_plain_lyrics() {
    let window = MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer);
    slint::platform::set_platform(Box::new(Headless(window.clone()))).unwrap();
    let flyout = Flyout::new().unwrap();
    flyout.window().set_size(PhysicalSize::new(452, 196));
    flyout.set_lyrics_available(true);
    let weak = flyout.as_weak();
    flyout.on_toggle_lyrics(move || {
        let flyout = weak.unwrap();
        flyout.set_lyrics_open(!flyout.get_lyrics_open());
        flyout.window().set_size(PhysicalSize::new(
            452,
            if flyout.get_lyrics_open() { 340 } else { 196 },
        ));
    });
    flyout.show().unwrap();
    let click = || {
        let position = LogicalPosition::new(370., 24.);
        window.dispatch_event(WindowEvent::PointerPressed {
            position,
            button: PointerEventButton::Left,
        });
        window.dispatch_event(WindowEvent::PointerReleased {
            position,
            button: PointerEventButton::Left,
        });
    };
    click();
    assert!(flyout.get_lyrics_open());
    assert!(!flyout.get_session_menu());
    let panel = || {
        let image = flyout.window().take_snapshot().unwrap();
        assert_eq!(image.height(), 340);
        image.as_bytes()[196 * 452 * 4..].to_vec()
    };
    flyout.set_lyrics_timed(true);
    flyout.set_lyric_word("First".into());
    flyout.set_lyric_after(" second".into());
    flyout.set_lyric_next("Next line".into());
    let first = panel();
    flyout.set_lyric_before("First ".into());
    flyout.set_lyric_word("second".into());
    flyout.set_lyric_after("".into());
    assert!(
        first != panel(),
        "moving word emphasis must change the rendered line"
    );
    flyout.set_lyric_after(
        " carries on for long enough to wrap onto another row in the lyrics panel".into(),
    );
    let wrapped = flyout.window().take_snapshot().unwrap();
    let (rows, _) = wrapped.as_slice().as_chunks::<452>();
    for row in &rows[195..326] {
        let background = row[16];
        assert!(
            row[20..432].contains(&background),
            "a solid divider crosses the lyrics panel"
        );
    }
    flyout.set_lyrics_timed(false);
    flyout.set_lyrics_body(
        (0..40)
            .map(|i| format!("Plain lyric line {i}\n"))
            .collect::<String>()
            .into(),
    );
    let before = panel();
    window.dispatch_event(WindowEvent::PointerScrolled {
        position: LogicalPosition::new(100., 260.),
        delta_x: 0.,
        delta_y: -75.,
    });
    assert!(before != panel(), "the plain lyrics must scroll");
    click();
    assert!(!flyout.get_lyrics_open());
}
