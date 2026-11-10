use std::{
    cell::RefCell,
    path::PathBuf,
    rc::Rc,
    sync::{Arc, Mutex},
    time::Duration,
};

use slint::{ComponentHandle, Model};

use crate::{
    config::{ConfigStore, Theme},
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
    pub config_path: Option<PathBuf>,
}

struct Controller {
    flyout: Flyout,
    native: Rc<NativeWindow>,
    service: RefCell<Option<media::MediaService>>,
    tray: TrayIcon,
    hotkey: RefCell<Option<RegisteredHotkey>>,
    dismiss_timer: slint::Timer,
    progress_timer: slint::Timer,
    latest: RefCell<Snapshot>,
    artwork: RefCell<Option<Arc<Artwork>>>,
    warnings: RefCell<Vec<String>>,
    config: RefCell<ConfigStore>,
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
        self.sync_progress();
    }

    fn hide(&self) {
        self.dismiss_timer.stop();
        self.progress_timer.stop();
        self.flyout.set_session_menu(false);
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
        let seconds = self.config.borrow().settings.dismiss_after_seconds;
        if seconds == 0 {
            self.dismiss_timer.stop();
            return;
        }
        let weak = Rc::downgrade(self);
        self.dismiss_timer.start(
            slint::TimerMode::SingleShot,
            Duration::from_secs(u64::from(seconds)),
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
                self.native.finish_transition();
                let _ = slint::quit_event_loop();
            }
            NativeEvent::Interaction => self.arm_dismiss(),
            NativeEvent::TrayCreated => {
                let _ = self.tray.add();
                self.update_tooltip();
            }
            NativeEvent::Resume => {
                if let Some(service) = &*self.service.borrow() {
                    service.reconnect();
                }
            }
            NativeEvent::TrayMenu => {
                self.dismiss_timer.stop();
                let settings = self.config.borrow().settings.clone();
                if let Some(action) = self.tray.show_menu(&settings) {
                    self.menu_action(action);
                }
                if self.flyout.window().is_visible() {
                    self.arm_dismiss();
                }
            }
            NativeEvent::SettingsChanged => {
                self.native.configure(&self.config.borrow().settings);
                self.apply_theme();
            }
            NativeEvent::Reload => self.reload(),
        }
    }

    fn apply_theme(&self) {
        let light = match self.config.borrow().settings.theme {
            Theme::Light => true,
            Theme::Dark => false,
            Theme::System => platform::system_light_theme(),
        };
        self.flyout.global::<Palette>().set_light(light);
    }

    fn update_tooltip(&self) {
        let shortcut = self
            .hotkey
            .borrow()
            .as_ref()
            .map(|h| h.shortcut().to_string())
            .unwrap_or_else(|| "shortcut unavailable".into());
        self.tray.set_shortcut(&shortcut);
    }

    fn apply_settings(self: &Rc<Self>) {
        let store = self.config.borrow();
        let settings = store.settings.clone();
        let path = store.path.clone();
        *self.warnings.borrow_mut() = store.warnings.clone();
        drop(store);
        if let Ok(shortcut) = settings.shortcut.parse() {
            let needs_registration = self
                .hotkey
                .borrow()
                .as_ref()
                .is_none_or(|old| old.shortcut() != shortcut);
            if needs_registration {
                let id = self
                    .hotkey
                    .borrow()
                    .as_ref()
                    .map_or(1, |old| if old.id() == 1 { 2 } else { 1 });
                match RegisteredHotkey::register(self.native.handle(), id, shortcut) {
                    Ok(hotkey) => {
                        *self.hotkey.borrow_mut() = Some(hotkey);
                    }
                    Err(error) => self
                        .warnings
                        .borrow_mut()
                        .push(format!("Shortcut unavailable; use the tray icon. {error}")),
                }
            }
        }
        self.update_tooltip();
        self.native.configure(&settings);
        self.apply_theme();
        if let Err(error) = platform::autostart(settings.autostart, &path) {
            self.warnings
                .borrow_mut()
                .push(format!("Startup setting failed: {error}"));
        }
        if let Some(service) = &*self.service.borrow() {
            service.prefer(settings.preferred_app);
            service.select(None);
        }
        let snapshot = self.latest.borrow().clone();
        self.update(snapshot);
        if let Some(warning) = self.warnings.borrow().first() {
            self.tray.notify(warning);
        }
        if self.flyout.window().is_visible() {
            self.arm_dismiss();
        }
    }

    fn reload(self: &Rc<Self>) {
        let result = self.config.borrow_mut().reload();
        match result {
            Ok(()) => self.apply_settings(),
            Err(error) => self.tray.notify(&format!("Settings unchanged: {error}")),
        }
    }

    fn menu_action(self: &Rc<Self>, action: MenuAction) {
        match action {
            MenuAction::Toggle => self.toggle(),
            MenuAction::Exit => self.native_event(NativeEvent::Exit),
            MenuAction::Reload => self.reload(),
            MenuAction::OpenConfig => {
                if let Err(error) = platform::open_config(&self.config.borrow().path) {
                    self.tray.notify(&error);
                }
            }
            MenuAction::Autostart | MenuAction::Remember => {
                let (key, enabled) = {
                    let store = self.config.borrow();
                    match action {
                        MenuAction::Autostart => ("autostart", !store.settings.autostart),
                        _ => (
                            "remember_preferred_app",
                            !store.settings.remember_preferred_app,
                        ),
                    }
                };
                let result = self.config.borrow_mut().set(key, enabled);
                match result {
                    Ok(()) => self.apply_settings(),
                    Err(error) => self.tray.notify(&format!("Cannot save settings: {error}")),
                }
            }
            MenuAction::Automatic => {
                let result = self.config.borrow_mut().set("preferred_app", "");
                match result {
                    Ok(()) => self.apply_settings(),
                    Err(error) => self
                        .tray
                        .notify(&format!("Cannot save preference: {error}")),
                }
            }
        }
    }

    fn select(self: &Rc<Self>, index: i32) {
        let selected = usize::try_from(index)
            .ok()
            .and_then(|i| i.checked_sub(1))
            .and_then(|i| self.latest.borrow().sessions.get(i).cloned());
        if let Some(service) = &*self.service.borrow() {
            service.select(selected.as_ref().map(|s| s.id));
        }
        if self.config.borrow().settings.remember_preferred_app
            && let Some(selected) = selected
        {
            let result = self
                .config
                .borrow_mut()
                .set("preferred_app", selected.app_id.clone());
            match result {
                Ok(()) => {
                    if let Some(service) = &*self.service.borrow() {
                        service.prefer(Some(selected.app_id));
                    }
                }
                Err(error) => self
                    .tray
                    .notify(&format!("Cannot remember application: {error}")),
            }
        }
        self.arm_dismiss();
    }

    fn update(self: &Rc<Self>, snapshot: Snapshot) {
        present(&self.flyout, &snapshot);
        if let Some(warning) = self.warnings.borrow().first() {
            self.flyout.set_status(warning.as_str().into());
        }
        if self.flyout.get_session_labels().row_count() == 0
            || self.latest.borrow().sessions != snapshot.sessions
        {
            let mut labels = vec![slint::SharedString::from(
                "Automatic · follow playing media",
            )];
            labels.extend(snapshot.sessions.iter().map(|s| {
                let name = app_name(&s.app_id);
                let name = if snapshot
                    .sessions
                    .iter()
                    .filter(|other| other.app_id == s.app_id)
                    .count()
                    > 1
                {
                    format!("{name} #{}", s.id)
                } else {
                    name
                };
                format!(
                    "{name} · {}",
                    if s.state == PlaybackState::Playing {
                        "Playing"
                    } else {
                        "Paused"
                    }
                )
                .into()
            }));
            self.flyout
                .set_session_labels(Rc::new(slint::VecModel::from(labels)).into());
        }
        self.flyout.set_selected_session(if snapshot.manual {
            snapshot
                .sessions
                .iter()
                .position(|s| Some(s.id) == snapshot.selected)
                .map_or(0, |i| i as i32 + 1)
        } else {
            0
        });
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
        self.sync_progress();
    }

    fn refresh_progress(&self) {
        let snapshot = self.latest.borrow();
        let timeline = snapshot
            .timeline
            .as_ref()
            .filter(|_| self.config.borrow().settings.show_progress);
        self.flyout.set_has_timeline(timeline.is_some());
        if let Some(timeline) = timeline {
            let position = timeline.current_position();
            self.flyout
                .set_elapsed(media::timestamp(position).as_str().into());
            self.flyout
                .set_duration(media::timestamp(timeline.duration).as_str().into());
            self.flyout.set_progress(
                (position.as_secs_f64() / timeline.duration.as_secs_f64()).clamp(0.0, 1.0) as f32,
            );
        }
    }

    fn sync_progress(self: &Rc<Self>) {
        self.refresh_progress();
        let active = self.flyout.window().is_visible()
            && self.config.borrow().settings.show_progress
            && self
                .latest
                .borrow()
                .timeline
                .as_ref()
                .is_some_and(|t| t.advancing);
        if active && !self.progress_timer.running() {
            let weak = Rc::downgrade(self);
            self.progress_timer.start(
                slint::TimerMode::Repeated,
                Duration::from_secs(1),
                move || {
                    if let Some(controller) = weak.upgrade() {
                        controller.refresh_progress();
                    }
                },
            );
        } else if !active {
            self.progress_timer.stop();
        }
    }
}

pub fn run(options: RunOptions) -> Result<(), Box<dyn std::error::Error>> {
    let Some(_instance) = platform::Instance::acquire()? else {
        if options.preview.is_some() {
            return Err("Close the running flyout before creating a preview".into());
        }
        return Ok(());
    };
    let backend = platform::initialize()?;
    let flyout = Flyout::new()?;
    let native = backend.window().ok_or("Native flyout was not created")?;
    let tray = TrayIcon::new(native.handle())?;
    let config = ConfigStore::load(options.config_path)?;
    let controller = Rc::new(Controller {
        flyout,
        native,
        service: RefCell::new(None),
        tray,
        hotkey: RefCell::new(None),
        dismiss_timer: slint::Timer::default(),
        progress_timer: slint::Timer::default(),
        latest: RefCell::new(Snapshot::default()),
        artwork: RefCell::new(None),
        warnings: RefCell::new(Vec::new()),
        config: RefCell::new(config),
    });
    controller.apply_settings();
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
    let weak = Rc::downgrade(&controller);
    controller.flyout.on_select_session(move |index| {
        if let Some(controller) = weak.upgrade() {
            controller.select(index);
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
    let preferred = controller.config.borrow().settings.preferred_app.clone();
    let service = media::MediaService::with_preference(preferred, move |snapshot| {
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
    controller.native.finish_transition();
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
        .map(|s| app_name(&s.app_id))
        .unwrap_or_else(|| "MEDIA FLYOUT".into());
    flyout.set_source(source.as_str().into());
    flyout.set_playing(snapshot.state == PlaybackState::Playing);
    flyout.set_can_toggle(snapshot.controls.play_pause);
    flyout.set_can_previous(snapshot.controls.previous);
    flyout.set_can_next(snapshot.controls.next);
    flyout.set_status(snapshot.status.as_deref().unwrap_or_default().into());
}

fn app_name(app_id: &str) -> String {
    let lower = app_id.to_ascii_lowercase();
    for (needle, name) in [
        ("spotify", "Spotify"),
        ("foobar2000", "foobar2000"),
        ("firefox", "Firefox"),
        ("chrome", "Chrome"),
        ("msedge", "Edge"),
    ] {
        if lower.contains(needle) {
            return name.into();
        }
    }
    let name = app_id.rsplit('!').next().unwrap_or(app_id);
    name.strip_suffix(".exe").unwrap_or(name).to_owned()
}
