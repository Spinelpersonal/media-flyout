use std::{
    cell::RefCell,
    rc::Rc,
    sync::{Arc, Mutex},
};

use slint::ComponentHandle;

use crate::{
    media::{self, Action, PlaybackState, Snapshot},
    platform,
};

slint::include_modules!();

pub fn run(preview: Option<std::path::PathBuf>) -> Result<(), Box<dyn std::error::Error>> {
    let native = platform::initialize()?;
    let flyout = Flyout::new()?;
    let weak = flyout.as_weak();
    let pending = Arc::new(Mutex::new(None::<Snapshot>));
    let mailbox = pending.clone();
    let service = Rc::new(media::MediaService::start(move |snapshot| {
        let should_schedule = mailbox
            .lock()
            .map(|mut slot| slot.replace(snapshot).is_none())
            .unwrap_or(false);
        if should_schedule {
            let mailbox = mailbox.clone();
            let weak = weak.clone();
            let _ = slint::invoke_from_event_loop(move || {
                let snapshot = mailbox.lock().ok().and_then(|mut slot| slot.take());
                if let (Some(flyout), Some(snapshot)) = (weak.upgrade(), snapshot) {
                    present(&flyout, &snapshot);
                }
            });
        }
    })?);
    let controls = service.clone();
    flyout.on_toggle_playback(move || controls.control(Action::PlayPause));
    let controls = service.clone();
    flyout.on_previous(move || controls.control(Action::Previous));
    let controls = service.clone();
    flyout.on_next(move || controls.control(Action::Next));
    flyout.on_dismiss(|| {
        let _ = slint::quit_event_loop();
    });
    flyout.window().on_close_requested(|| {
        let _ = slint::quit_event_loop();
        slint::CloseRequestResponse::HideWindow
    });
    // Keep the shared native state and media worker alive throughout the event loop.
    let _lifetime = (native, service, RefCell::new(pending));
    flyout.show()?;
    let preview_result = Rc::new(RefCell::new(Ok(())));
    let preview_timer = slint::Timer::default();
    if let Some(path) = preview {
        let weak = flyout.as_weak();
        let result = preview_result.clone();
        preview_timer.start(
            slint::TimerMode::SingleShot,
            std::time::Duration::from_secs(2),
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
    slint::run_event_loop()?;
    flyout.hide()?;
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
    if let Some(artwork) = &snapshot.track.artwork {
        let pixels = slint::SharedPixelBuffer::<slint::Rgba8Pixel>::clone_from_slice(
            &artwork.rgba,
            artwork.width,
            artwork.height,
        );
        flyout.set_artwork(slint::Image::from_rgba8(pixels));
        flyout.set_has_artwork(true);
    } else {
        flyout.set_artwork(slint::Image::default());
        flyout.set_has_artwork(false);
    }
}
