use super::{Controller, Palette, TrackNotification, accent, artwork_image};
use crate::{
    config::Settings,
    media::{Artwork, Snapshot, notifications::TrackStart},
    platform::{NativeWindow, monitor},
};
use slint::ComponentHandle;
use std::{rc::Rc, sync::Arc, time::Duration};

pub(super) struct Popup {
    component: TrackNotification,
    native: Rc<NativeWindow>,
    current: Option<TrackStart>,
}

impl Popup {
    pub fn set_theme(&self, light: bool, settings: &Settings) {
        let palette = self.component.global::<Palette>();
        palette.set_light(light);
        accent::apply(&palette, settings, None);
    }

    fn set_artwork(&self, artwork: Option<&Artwork>) {
        self.component.set_artwork(artwork_image(artwork));
        self.component.set_has_artwork(artwork.is_some());
    }
}

impl Controller {
    pub(super) fn notify_track(self: &Rc<Self>, notice: TrackStart, snapshot: &Snapshot) {
        if notice.generation != self.notifications.generation() {
            return;
        }

        let store = self.config.borrow();
        let settings = &store.settings;
        let target = monitor::target(settings.monitor);
        if settings.notification_hide_fullscreen && monitor::foreground_fullscreen(target) {
            self.hide_notification();
            return;
        }
        let mut slot = self.notification.borrow_mut();
        let popup = match slot.as_mut() {
            Some(popup) => popup,
            None => {
                let result = TrackNotification::new().and_then(|component| {
                    let _ = component.window();
                    let native = self
                        .backend
                        .notification()
                        .ok_or("Notification window was not created")?;
                    Ok(Popup {
                        component,
                        native,
                        current: None,
                    })
                });
                match result {
                    Ok(popup) => slot.insert(popup),
                    Err(error) => {
                        self.tray
                            .notify(&format!("Cannot show track notification: {error}"));
                        return;
                    }
                }
            }
        };
        popup.set_theme(self.flyout.global::<Palette>().get_light(), settings);
        popup
            .component
            .set_track_title(notice.track.title.as_str().into());
        popup.component.set_player(notice.player.as_str().into());
        popup
            .component
            .set_artist(notice.track.artist.as_str().into());
        popup.set_artwork(notice.track.artwork.as_deref());
        popup.current = Some(notice);
        popup.native.position_notification(target, settings);
        if let Err(error) = popup.component.show() {
            self.tray
                .notify(&format!("Cannot show track notification: {error}"));
            return;
        }
        drop(slot);
        self.update_notification_artwork(snapshot);
        let controller = Rc::downgrade(self);
        self.notification_timer.start(
            slint::TimerMode::SingleShot,
            Duration::from_secs(settings.notification_seconds.into()),
            move || {
                if let Some(controller) = controller.upgrade() {
                    controller.hide_notification();
                }
            },
        );
        self.fullscreen_timer.stop();
        if settings.notification_hide_fullscreen {
            let controller = Rc::downgrade(self);
            self.fullscreen_timer.start(
                slint::TimerMode::Repeated,
                Duration::from_millis(200),
                move || {
                    if let Some(controller) = controller.upgrade() {
                        let monitor = controller
                            .notification
                            .borrow()
                            .as_ref()
                            .map(|popup| popup.native.monitor());
                        if monitor.is_some_and(monitor::foreground_fullscreen) {
                            controller.hide_notification();
                        }
                    }
                },
            );
        }
    }

    pub(super) fn update_notification_artwork(&self, snapshot: &Snapshot) {
        if let Some(popup) = self.notification.borrow_mut().as_mut()
            && let Some(current) = popup.current.as_mut()
            && current.matches(snapshot.selected, &snapshot.track)
        {
            let track = &snapshot.track;
            if track.artwork.as_ref().is_some_and(|new| {
                current
                    .track
                    .artwork
                    .as_ref()
                    .is_none_or(|old| !Arc::ptr_eq(old, new))
            }) {
                current.track.artwork.clone_from(&track.artwork);
                popup.set_artwork(track.artwork.as_deref());
            }
            if !track.artist.is_empty() && popup.component.get_artist() != track.artist.as_str() {
                popup.component.set_artist(track.artist.as_str().into());
            }
        }
    }

    pub(super) fn hide_notification(&self) {
        self.notification_timer.stop();
        self.fullscreen_timer.stop();
        if let Some(popup) = self.notification.borrow_mut().as_mut()
            && popup.current.take().is_some()
        {
            let _ = popup.component.hide();
            popup.set_artwork(None);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        config::{Monitor, Position},
        platform,
        ui::Flyout,
    };
    use windows::Win32::UI::WindowsAndMessaging::{
        GWL_EXSTYLE, GetForegroundWindow, GetWindowLongW, IsWindowVisible, WS_EX_NOACTIVATE,
        WS_EX_TRANSPARENT,
    };

    #[test]
    fn compact_window_uses_independent_position_and_passive_styles()
    -> Result<(), Box<dyn std::error::Error>> {
        let backend = platform::initialize()?;
        let main = Flyout::new()?;
        let notification = TrackNotification::new()?;
        let _ = notification.window();
        let native = backend.notification().expect("compact native window");
        assert_ne!(native.handle(), backend.window().unwrap().handle());
        notification.set_track_title("Test track".into());
        notification.set_artist("Test artist".into());
        notification.set_player("Test player".into());
        let settings = Settings {
            notification_position: Position::BottomRight,
            ..Settings::default()
        };
        native.position_notification(monitor::target(Monitor::Primary), &settings);
        let bottom_right = notification.window().position();
        native.position_notification(
            monitor::target(Monitor::Primary),
            &Settings {
                notification_position: Position::TopLeft,
                ..settings
            },
        );
        let top_left = notification.window().position();
        assert!(top_left.x < bottom_right.x && top_left.y < bottom_right.y);
        notification.show()?;
        // SAFETY: These read-only calls use the live HWND created on this thread.
        unsafe {
            let style = GetWindowLongW(native.handle(), GWL_EXSTYLE) as u32;
            let expected = WS_EX_NOACTIVATE | WS_EX_TRANSPARENT;
            assert_eq!(style & expected.0, expected.0);
            assert!(IsWindowVisible(native.handle()).as_bool());
            assert_ne!(GetForegroundWindow(), native.handle());
        }
        assert!(!main.window().is_visible());
        let pixels = notification.window().take_snapshot()?;
        assert!(pixels.width() > pixels.height());
        assert!(pixels.as_slice().windows(2).any(|pair| pair[0] != pair[1]));
        notification.hide()?;
        // SAFETY: The window is hidden and still owned by this thread.
        assert!(!unsafe { IsWindowVisible(native.handle()) }.as_bool());
        Ok(())
    }
}
