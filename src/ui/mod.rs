use std::{
    cell::RefCell,
    path::PathBuf,
    rc::Rc,
    sync::{Arc, Mutex},
    time::Duration,
};

use slint::ComponentHandle;

use crate::{
    hotkey::RegisteredHotkey,
    media::{self, Action, Artwork, PlaybackState, Snapshot},
    platform::{self, NativeEvent, NativeWindow},
    tray::{MenuAction, TrayIcon},
};

slint::include_modules!();

#[derive(Default)]
pub struct RunOptions {
    pub show: bool,
    pub preview: Option<PathBuf>,
}

struct Controller {
    flyout: Flyout,
    native: Rc<NativeWindow>,
    service: RefCell<Option<media::MediaService>>,
    tray: TrayIcon,
    _hotkey: Option<RegisteredHotkey>,
    dismiss_timer: slint::Timer,
    latest: RefCell<Snapshot>,
    artwork: RefCell<Option<Arc<Artwork>>>,
    warning: Option<String>,
}

impl Controller {
    fn show(self: &Rc<Self>) {
        self.native.position_at_top();
        if let Err(error) = self.flyout.show() {
            self.tray.notify(&error.to_string());
            return;
        }
        if let Some(service) = &*self.service.borrow() {
            service.refresh();
        }
        self.arm_dismiss();
    }

    fn hide(&self) {
        self.dismiss_timer.stop();
        let _ = self.flyout.hide();
    }

    fn toggle(self: &Rc<Self>) {
        if self.flyout.window().is_visible() {
            self.hide();
        } else {
            self.show();
        }
    }

    fn arm_dismiss(self: &Rc<Self>) {
        let weak = Rc::downgrade(self);
        self.dismiss_timer.start(
            slint::TimerMode::SingleShot,
            Duration::from_secs(10),
            move || {
                if let Some(controller) = weak.upgrade() {
                    controller.hide();
                }
            },
        );
    }

    fn control(self: &Rc<Self>, action: Action) {
        if let Some(service) = &*self.service.borrow() {
            service.control(action);
        }
        self.arm_dismiss();
    }

    fn native_event(self: &Rc<Self>, event: NativeEvent) {
        match event {
            NativeEvent::Toggle => self.toggle(),
            NativeEvent::Show => self.show(),
            NativeEvent::Dismiss => self.hide(),
            NativeEvent::Exit => {
                self.hide();
                let _ = slint::quit_event_loop();
            }
            NativeEvent::Interaction => self.arm_dismiss(),
            NativeEvent::TrayCreated => {
                let _ = self.tray.add();
            }
            NativeEvent::Resume => {
                if let Some(service) = &*self.service.borrow() {
                    service.reconnect();
                }
            }
            NativeEvent::TrayMenu => {
                self.dismiss_timer.stop();
                match self.tray.show_menu() {
                    Some(MenuAction::Toggle) => self.toggle(),
                    Some(MenuAction::Exit) => self.native_event(NativeEvent::Exit),
                    None => {}
                }
                if self.flyout.window().is_visible() {
                    self.arm_dismiss();
                }
            }
        }
    }

    fn update(&self, snapshot: Snapshot) {
        present(&self.flyout, &snapshot);
        if let Some(warning) = &self.warning {
            self.flyout.set_status(warning.as_str().into());
        }
        let changed = match (&*self.artwork.borrow(), &snapshot.track.artwork) {
            (Some(old), Some(new)) => !Arc::ptr_eq(old, new),
            (None, None) => false,
            _ => true,
        };
        if changed {
            let image = snapshot
                .track
                .artwork
                .as_ref()
                .map(|artwork| {
                    slint::Image::from_rgba8(
                        slint::SharedPixelBuffer::<slint::Rgba8Pixel>::clone_from_slice(
                            &artwork.rgba,
                            artwork.width,
                            artwork.height,
                        ),
                    )
                })
                .unwrap_or_default();
            self.flyout.set_artwork(image);
            self.flyout
                .set_has_artwork(snapshot.track.artwork.is_some());
            *self.artwork.borrow_mut() = snapshot.track.artwork.clone();
        }
        *self.latest.borrow_mut() = snapshot;
    }
}

pub fn run(options: RunOptions) -> Result<(), Box<dyn std::error::Error>> {
    let Some(_instance) = platform::Instance::acquire()? else {
        return Ok(());
    };
    let backend = platform::initialize()?;
    let flyout = Flyout::new()?;
    let native = backend.window().ok_or("Native flyout was not created")?;
    let tray = TrayIcon::new(native.handle())?;
    let shortcut = "Ctrl+Space".parse()?;
    let (hotkey, warning) = match RegisteredHotkey::register(native.handle(), 1, shortcut) {
        Ok(hotkey) => (Some(hotkey), None),
        Err(error) => {
            tray.notify(&format!(
                "Ctrl + Space is unavailable: {error}. Use the tray icon to open the flyout."
            ));
            (
                None,
                Some("Shortcut unavailable. Use the tray icon.".into()),
            )
        }
    };
    let controller = Rc::new(Controller {
        flyout,
        native,
        service: RefCell::new(None),
        tray,
        _hotkey: hotkey,
        dismiss_timer: slint::Timer::default(),
        latest: RefCell::new(Snapshot::default()),
        artwork: RefCell::new(None),
        warning,
    });
    let weak = Rc::downgrade(&controller);
    controller.native.set_event_handler(move |event| {
        if let Some(controller) = weak.upgrade() {
            controller.native_event(event);
        }
    });
    let weak = Rc::downgrade(&controller);
    controller.flyout.on_dismiss(move || {
        if let Some(controller) = weak.upgrade() {
            controller.hide();
        }
    });
    let weak = Rc::downgrade(&controller);
    controller.flyout.window().on_close_requested(move || {
        if let Some(controller) = weak.upgrade() {
            controller.hide();
        }
        slint::CloseRequestResponse::KeepWindowShown
    });
    let weak = Rc::downgrade(&controller);
    controller.flyout.on_toggle_playback(move || {
        if let Some(controller) = weak.upgrade() {
            controller.control(Action::PlayPause);
        }
    });
    let weak = Rc::downgrade(&controller);
    controller.flyout.on_previous(move || {
        if let Some(controller) = weak.upgrade() {
            controller.control(Action::Previous);
        }
    });
    let weak = Rc::downgrade(&controller);
    controller.flyout.on_next(move || {
        if let Some(controller) = weak.upgrade() {
            controller.control(Action::Next);
        }
    });
    let mailbox = Arc::new(Mutex::new(None::<Snapshot>));
    let pending = mailbox.clone();
    let weak = Rc::downgrade(&controller);
    controller.flyout.on_media_updated(move || {
        let snapshot = pending.lock().ok().and_then(|mut slot| slot.take());
        if let (Some(controller), Some(snapshot)) = (weak.upgrade(), snapshot) {
            controller.update(snapshot);
        }
    });
    let weak = controller.flyout.as_weak();
    let service = media::MediaService::start(move |snapshot| {
        let should_schedule = mailbox
            .lock()
            .map(|mut slot| slot.replace(snapshot).is_none())
            .unwrap_or(false);
        if should_schedule {
            let weak = weak.clone();
            let _ = slint::invoke_from_event_loop(move || {
                if let Some(flyout) = weak.upgrade() {
                    flyout.invoke_media_updated();
                }
            });
        }
    })?;
    *controller.service.borrow_mut() = Some(service);
    slint::platform::update_timers_and_animations();
    if options.show || options.preview.is_some() {
        controller.show();
    }
    let preview_result = Rc::new(RefCell::new(Ok(())));
    let preview_timer = slint::Timer::default();
    if let Some(path) = options.preview {
        let weak = controller.flyout.as_weak();
        let result = preview_result.clone();
        preview_timer.start(
            slint::TimerMode::SingleShot,
            Duration::from_secs(2),
            move || {
                if let Some(flyout) = weak.upgrade() {
                    *result.borrow_mut() = flyout
                        .window()
                        .take_snapshot()
                        .map_err(std::io::Error::other)
                        .and_then(|pixels| save_bitmap(&path, &pixels));
                }
                let _ = slint::quit_event_loop();
            },
        );
    }
    slint::run_event_loop_until_quit()?;
    controller.hide();
    controller.service.borrow_mut().take();
    std::mem::replace(&mut *preview_result.borrow_mut(), Ok(()))?;
    Ok(())
}

fn save_bitmap(
    path: &std::path::Path,
    pixels: &slint::SharedPixelBuffer<slint::Rgba8Pixel>,
) -> std::io::Result<()> {
    use std::io::Write;
    let mut file = std::io::BufWriter::new(std::fs::File::create(path)?);
    let length = pixels.width() * pixels.height() * 4;
    file.write_all(b"BM")?;
    file.write_all(&(54 + length).to_le_bytes())?;
    file.write_all(&[0; 4])?;
    file.write_all(&54u32.to_le_bytes())?;
    file.write_all(&40u32.to_le_bytes())?;
    file.write_all(&pixels.width().to_le_bytes())?;
    file.write_all(&pixels.height().to_le_bytes())?;
    file.write_all(&1u16.to_le_bytes())?;
    file.write_all(&32u16.to_le_bytes())?;
    file.write_all(&[0; 24])?;
    for row in pixels.as_slice().chunks(pixels.width() as usize).rev() {
        for pixel in row {
            file.write_all(&[pixel.b, pixel.g, pixel.r, 0])?;
        }
    }
    file.flush()
}

fn present(flyout: &Flyout, snapshot: &Snapshot) {
    let has_session = snapshot.selected.is_some();
    flyout.set_track_title(if snapshot.track.title.is_empty() {
        if has_session {
            "Untitled media"
        } else {
            "Nothing playing"
        }
        .into()
    } else {
        snapshot.track.title.as_str().into()
    });
    flyout.set_artist(if snapshot.track.artist.is_empty() {
        if has_session {
            "Unknown artist"
        } else {
            "Open a media player to get started"
        }
        .into()
    } else {
        snapshot.track.artist.as_str().into()
    });
    flyout.set_album(snapshot.track.album.as_str().into());
    let source = snapshot
        .sessions
        .iter()
        .find(|s| Some(s.id) == snapshot.selected)
        .map(|s| s.app_id.strip_suffix(".exe").unwrap_or(&s.app_id))
        .unwrap_or("MEDIA FLYOUT");
    flyout.set_source(source.into());
    flyout.set_playing(snapshot.state == PlaybackState::Playing);
    flyout.set_can_toggle(snapshot.controls.play_pause);
    flyout.set_can_previous(snapshot.controls.previous);
    flyout.set_can_next(snapshot.controls.next);
    flyout.set_status(snapshot.status.as_deref().unwrap_or_default().into());
}
