use super::{Controller, Flyout};
use crate::lyrics::{Client, Lyrics, TrackKey};
use slint::{ComponentHandle, LogicalSize};
use std::{
    collections::VecDeque,
    rc::Rc,
    sync::{Arc, mpsc},
    thread,
    time::{Duration, Instant},
};

type Found = Option<Arc<Lyrics>>;
type Reply = (TrackKey, Result<Found, String>);

struct Worker {
    request: mpsc::Sender<Option<TrackKey>>,
    replies: mpsc::Receiver<Reply>,
}

impl Worker {
    fn start(window: slint::Weak<Flyout>) -> std::io::Result<Self> {
        let (request, jobs) = mpsc::channel::<Option<TrackKey>>();
        let (done, replies) = mpsc::channel();
        thread::Builder::new()
            .name("lrclib".into())
            .spawn(move || {
                let mut client = Client::new();
                let mut cache: VecDeque<(TrackKey, Found, Instant)> = VecDeque::new();
                while let Ok(first) = jobs.recv() {
                    // Discard intermediate track changes and respect a closed panel.
                    let Some(key) = jobs.try_iter().last().unwrap_or(first) else {
                        continue;
                    };
                    let result = if let Some((_, lyrics, _)) = cache
                        .iter()
                        .find(|(k, _, at)| k == &key && at.elapsed() < Duration::from_secs(600))
                    {
                        Ok(lyrics.clone())
                    } else {
                        client.lookup(&key).map(|lyrics| {
                            let lyrics = lyrics.map(Arc::new);
                            cache.push_front((key.clone(), lyrics.clone(), Instant::now()));
                            cache.truncate(8);
                            lyrics
                        })
                    };
                    if done.send((key, result)).is_err() {
                        break;
                    }
                    if window
                        .upgrade_in_event_loop(|w| w.invoke_lyrics_ready())
                        .is_err()
                    {
                        break;
                    }
                }
            })?;
        Ok(Self { request, replies })
    }
}

type Cursor = (Option<usize>, Option<usize>, Option<usize>);

#[derive(Default)]
pub(super) struct Panel {
    worker: Option<Worker>,
    key: Option<TrackKey>,
    song: Found,
    cursor: Option<Cursor>,
}

pub(super) fn connect(controller: &Rc<Controller>) {
    let weak = Rc::downgrade(controller);
    controller.flyout.on_toggle_lyrics(move || {
        let Some(c) = weak.upgrade() else { return };
        if c.flyout.get_lyrics_open() {
            c.close_lyrics();
        } else {
            c.flyout.set_session_menu(false);
            c.size_for_lyrics(true);
            c.sync_lyrics();
        }
        c.native.position_at_top();
        c.arm_dismiss();
    });
    let weak = Rc::downgrade(controller);
    controller.flyout.on_lyrics_ready(move || {
        if let Some(c) = weak.upgrade() {
            c.receive_lyrics();
        }
    });
}

impl Controller {
    fn size_for_lyrics(&self, open: bool) {
        self.flyout.set_lyrics_open(open);
        self.flyout
            .window()
            .set_size(LogicalSize::new(452., if open { 340. } else { 196. }));
    }

    pub(super) fn close_lyrics(&self) {
        if !self.flyout.get_lyrics_open() {
            return;
        }
        self.size_for_lyrics(false);
        self.lyric_timer.stop();
        let mut panel = self.lyrics.borrow_mut();
        if let Some(w) = &panel.worker {
            let _ = w.request.send(None);
        }
        panel.key = None;
        panel.song = None;
        panel.cursor = None;
    }

    fn lyrics_message(&self, text: &str) {
        self.flyout.set_lyrics_timed(false);
        self.flyout.set_lyrics_body(text.into());
    }

    pub(super) fn sync_lyrics(self: &Rc<Self>) {
        let snapshot = self.latest.borrow();
        let track = &snapshot.track;
        let available = snapshot.selected.is_some()
            && !track.artist.trim().is_empty()
            && !track.title.trim().is_empty();
        self.flyout.set_lyrics_available(available);
        if !self.flyout.get_lyrics_open() {
            return;
        }
        let key = available.then(|| TrackKey {
            title: track.title.trim().into(),
            artist: track.artist.trim().into(),
            album: track.album.trim().into(),
            duration: snapshot
                .timeline
                .as_ref()
                .map(|t| t.duration.as_secs_f64().round() as u64),
        });
        drop(snapshot);
        let mut panel = self.lyrics.borrow_mut();
        if panel.key != key {
            panel.song = None;
            panel.cursor = None;
            panel.key = key.clone();
            if key.is_some() && panel.worker.is_none() {
                match Worker::start(self.flyout.as_weak()) {
                    Ok(worker) => panel.worker = Some(worker),
                    Err(_) => {
                        self.lyrics_message(
                            "Couldn’t start lyrics lookup. Reopen Lyrics to retry.",
                        );
                        return;
                    }
                }
            }
            if key.is_some() {
                self.lyrics_message("Looking up lyrics…");
            }
            if let Some(w) = &panel.worker
                && w.request.send(key.clone()).is_err()
            {
                panel.worker = None;
                self.lyrics_message("Lyrics lookup stopped. Reopen Lyrics to retry.");
            }
        }
        if key.is_none() {
            self.lyrics_message("Waiting for a track title and artist.");
        }
        drop(panel);
        self.sync_lyric_clock();
    }

    fn receive_lyrics(self: &Rc<Self>) {
        let mut panel = self.lyrics.borrow_mut();
        let Some(w) = &panel.worker else {
            return;
        };
        let reply = w
            .replies
            .try_iter()
            .filter(|(key, _)| panel.key.as_ref() == Some(key))
            .last();
        let Some((_, result)) = reply else {
            return;
        };
        panel.cursor = None;
        match result {
            Ok(Some(song)) if song.instrumental => self.lyrics_message("Instrumental"),
            Ok(Some(song)) => {
                self.lyrics_message(&song.plain);
                panel.song = Some(song);
            }
            Ok(None) => self.lyrics_message("No matching lyrics on LRCLIB."),
            Err(error) => self.lyrics_message(&error),
        }
        drop(panel);
        self.sync_lyric_clock();
    }

    fn sync_lyric_clock(self: &Rc<Self>) {
        let advancing = self.render_lyrics();
        if advancing && !self.lyric_timer.running() {
            let weak = Rc::downgrade(self);
            self.lyric_timer.start(
                slint::TimerMode::Repeated,
                Duration::from_millis(50),
                move || {
                    if let Some(c) = weak.upgrade() {
                        c.render_lyrics();
                    }
                },
            );
        } else if !advancing {
            self.lyric_timer.stop();
        }
    }

    fn render_lyrics(&self) -> bool {
        let snapshot = self.latest.borrow();
        let mut panel = self.lyrics.borrow_mut();
        let Some(timeline) = &snapshot.timeline else {
            self.flyout.set_lyrics_timed(false);
            panel.cursor = None;
            return false;
        };
        let Some(song) = panel.song.as_ref().filter(|s| !s.lines.is_empty()) else {
            return false;
        };
        let cursor = song.cursor(timeline.current_position().as_millis() as u64);
        if panel.cursor != Some(cursor) {
            let (line, word, next) = cursor;
            let mut before = String::new();
            let mut active = "";
            let mut after = String::new();
            if let Some(line) = line.map(|i| &song.lines[i]) {
                if let Some(i) = word {
                    before.extend(line.words[..i].iter().map(|w| w.text.as_str()));
                    active = &line.words[i].text;
                    after.extend(line.words[i + 1..].iter().map(|w| w.text.as_str()));
                } else {
                    before.clone_from(&line.text);
                }
            }
            self.flyout.set_lyric_before(before.into());
            self.flyout.set_lyric_word(active.into());
            self.flyout.set_lyric_after(after.into());
            self.flyout
                .set_lyric_next(next.map_or("", |i| song.lines[i].text.as_str()).into());
            self.flyout.set_lyrics_timed(true);
            panel.cursor = Some(cursor);
        }
        timeline.advancing
    }
}
