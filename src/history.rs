use crate::media::{PlaybackState, SessionInfo, Track};
use chrono::{DateTime, SecondsFormat, Utc};
use rusqlite::{Connection, OpenFlags, functions::FunctionFlags, params};
use std::{
    collections::HashMap,
    fs, io,
    path::{Path, PathBuf},
    sync::{
        Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
};

pub(crate) type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

// Windows' unbiased clock excludes sleep and hibernation and survives wall-clock edits.
fn awake_ms() -> u64 {
    let mut ticks = 0;
    // SAFETY: The output points to a writable u64 for the duration of the call.
    let _ = unsafe {
        windows::Win32::System::WindowsProgramming::QueryUnbiasedInterruptTime(&mut ticks)
    };
    ticks / 10_000
}

struct Listen {
    // None suppresses a cleared listen until playback pauses or changes track.
    row: Option<i64>,
    title: String,
    artist: String,
    album: String,
    elapsed: u64,
    since: Option<u64>,
}

impl Listen {
    fn duration(&self, now: u64) -> u64 {
        self.elapsed + self.since.map_or(0, |start| now.saturating_sub(start))
    }

    fn checkpoint(&mut self, db: &Connection, now: u64, playing: bool) -> Result<()> {
        let Some(row) = self.row else { return Ok(()) };
        let elapsed = self.duration(now);
        db.execute(
            "UPDATE listens SET duration_ms=? WHERE id=?",
            params![elapsed, row],
        )?;
        self.elapsed = elapsed;
        self.since = playing.then_some(now);
        Ok(())
    }
}

#[derive(Default)]
struct Recorder {
    db: Option<Connection>,
    sessions: HashMap<u64, Listen>,
}

pub(crate) struct History {
    path: Option<PathBuf>,
    enabled: AtomicBool,
    revision: AtomicU64,
    recorder: Mutex<Recorder>,
}

impl History {
    pub fn local() -> Self {
        Self::new(
            std::env::var_os("LOCALAPPDATA")
                .filter(|p| !p.is_empty())
                .map(|p| PathBuf::from(p).join("MediaFlyout").join("history.sqlite3")),
        )
    }

    pub(crate) fn new(path: Option<PathBuf>) -> Self {
        Self {
            path,
            enabled: false.into(),
            revision: 0.into(),
            recorder: Mutex::default(),
        }
    }

    fn path(&self) -> Result<&Path> {
        self.path
            .as_deref()
            .ok_or_else(|| "LOCALAPPDATA is unavailable".into())
    }

    pub fn enabled(&self) -> bool {
        self.enabled.load(Ordering::Acquire)
    }

    pub fn revision(&self) -> u64 {
        self.revision.load(Ordering::Acquire)
    }

    fn changed(&self) {
        self.revision.fetch_add(1, Ordering::Release);
    }

    pub fn set_enabled(&self, enabled: bool) -> Result<()> {
        let mut recorder = self.recorder.lock().unwrap();
        if self.enabled.swap(enabled, Ordering::AcqRel) != enabled {
            self.changed();
        }
        if !enabled {
            // End all intervals even if the disk is unavailable at opt-out.
            let Recorder { db, sessions } = std::mem::take(&mut *recorder);
            if let Some(mut db) = db {
                let now = awake_ms();
                let transaction = db.transaction()?;
                for mut listen in sessions.into_values().filter(|l| l.since.is_some()) {
                    listen.checkpoint(&transaction, now, false)?;
                }
                transaction.commit()?;
            }
        }
        Ok(())
    }

    fn open_writer(&self) -> Result<Connection> {
        let path = self.path()?;
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let db = Connection::open(path)?;
        db.execute_batch(
            "PRAGMA journal_mode=WAL;
            PRAGMA secure_delete=ON;
            CREATE TABLE IF NOT EXISTS listens (
                id INTEGER PRIMARY KEY, started INTEGER NOT NULL, title TEXT NOT NULL,
                artist TEXT NOT NULL, player TEXT NOT NULL, duration_ms INTEGER NOT NULL DEFAULT 0
            );
            CREATE INDEX IF NOT EXISTS listens_started ON listens(started DESC, id DESC);",
        )?;
        Ok(db)
    }

    pub fn observe<'a>(
        &self,
        sessions: impl Iterator<Item = (&'a SessionInfo, &'a Track)> + Clone,
    ) -> Result<()> {
        self.observe_at(sessions, awake_ms(), Utc::now().timestamp_millis())
    }

    fn observe_at<'a>(
        &self,
        observed: impl Iterator<Item = (&'a SessionInfo, &'a Track)> + Clone,
        now: u64,
        started: i64,
    ) -> Result<()> {
        let mut recorder = self.recorder.lock().unwrap();
        if !self.enabled() {
            return Ok(());
        }
        let Recorder { db, sessions } = &mut *recorder;
        let mut finished = Vec::new();
        for (&id, listen) in sessions.iter_mut() {
            let current = observed.clone().find(|(info, _)| info.id == id);
            let same = current.filter(|(info, track)| {
                info.state != PlaybackState::Stopped
                    && listen.title == track.title
                    && listen.artist == track.artist
                    && listen.album == track.album
            });
            let playing = same.is_some_and(|(info, _)| info.state == PlaybackState::Playing);
            if listen.row.is_none() {
                if !playing {
                    finished.push(id);
                }
                continue;
            }
            if playing != listen.since.is_some()
                || listen
                    .since
                    .is_some_and(|t| now.saturating_sub(t) >= 60_000)
            {
                listen.checkpoint(
                    db.as_ref().expect("active listen has a database"),
                    now,
                    playing,
                )?;
                self.changed();
            }
            if same.is_none() {
                finished.push(id);
            }
        }
        for id in finished {
            sessions.remove(&id);
        }
        for (info, track) in observed {
            if info.state != PlaybackState::Playing
                || track.title.trim().is_empty()
                || sessions.contains_key(&info.id)
            {
                continue;
            }
            if db.is_none() {
                *db = Some(self.open_writer()?);
            }
            let db = db.as_ref().unwrap();
            db.execute(
                "INSERT INTO listens(started,title,artist,player) VALUES (?,?,?,?)",
                params![started, track.title, track.artist, info.display_name],
            )?;
            sessions.insert(
                info.id,
                Listen {
                    row: Some(db.last_insert_rowid()),
                    title: track.title.clone(),
                    artist: track.artist.clone(),
                    album: track.album.clone(),
                    elapsed: 0,
                    since: Some(now),
                },
            );
            self.changed();
        }
        Ok(())
    }

    pub fn clear(&self) -> Result<()> {
        let mut recorder = self.recorder.lock().unwrap();
        if self.path()?.try_exists()? {
            let db = self.open_writer()?;
            db.execute("DELETE FROM listens", [])?;
        }
        recorder.sessions.retain(|_, listen| {
            listen.row = None;
            listen.since.is_some()
        });
        self.changed();
        Ok(())
    }

    // Pin a read snapshot before releasing the recorder. Browsing and export can
    // stream from it while the media worker continues to record subsequent events.
    pub fn view(&self) -> Result<View> {
        let recorder = self.recorder.lock().unwrap();
        let db = if self.path()?.try_exists()? {
            let db = Connection::open_with_flags(self.path()?, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
            db.execute_batch("BEGIN; SELECT id FROM listens LIMIT 1;")?;
            db.create_scalar_function(
                "fold",
                1,
                FunctionFlags::SQLITE_DETERMINISTIC | FunctionFlags::SQLITE_UTF8,
                |ctx| Ok(ctx.get::<String>(0)?.to_lowercase()),
            )?;
            Some(db)
        } else {
            None
        };
        let now = awake_ms();
        let durations = recorder
            .sessions
            .values()
            .filter_map(|l| Some((l.row?, l.duration(now))))
            .collect();
        Ok(View { db, durations })
    }
}

pub(crate) struct View {
    db: Option<Connection>,
    durations: HashMap<i64, u64>,
}

pub(crate) struct Filter {
    pub search: String,
    pub since: i64,
}

pub(crate) struct Entry {
    pub started: DateTime<Utc>,
    pub title: String,
    pub artist: String,
    pub player: String,
    pub duration_ms: u64,
}

impl View {
    pub fn rows(
        &self,
        filter: &Filter,
        offset: i64,
        limit: i64,
        mut visit: impl FnMut(Entry) -> Result<()>,
    ) -> Result<()> {
        let Some(db) = &self.db else { return Ok(()) };
        let mut statement = db.prepare("SELECT id, started, title, artist, player, duration_ms FROM listens
            WHERE started >= ?1 AND (?2 = '' OR instr(fold(title), ?2) OR instr(fold(artist), ?2) OR instr(fold(player), ?2))
            ORDER BY started DESC, id DESC LIMIT ?3 OFFSET ?4")?;
        let mut rows = statement.query(params![
            filter.since,
            filter.search.trim().to_lowercase(),
            limit,
            offset
        ])?;
        while let Some(row) = rows.next()? {
            let id: i64 = row.get(0)?;
            visit(Entry {
                started: DateTime::from_timestamp_millis(row.get(1)?)
                    .ok_or("Invalid history timestamp")?,
                title: row.get(2)?,
                artist: row.get(3)?,
                player: row.get(4)?,
                duration_ms: self.durations.get(&id).copied().unwrap_or(row.get(5)?),
            })?;
        }
        Ok(())
    }

    pub fn export(&self, filter: &Filter, path: &Path) -> Result<()> {
        if !path
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("csv"))
        {
            return Err("Choose a file with a .csv extension".into());
        }
        let mut file = tempfile::NamedTempFile::new_in(path.parent().unwrap_or(Path::new(".")))?;
        self.write_csv(filter, file.as_file_mut())?;
        file.as_file().sync_all()?;
        file.persist(path)?;
        Ok(())
    }

    fn write_csv(&self, filter: &Filter, output: impl io::Write) -> Result<()> {
        let mut writer = csv::WriterBuilder::new()
            .terminator(csv::Terminator::CRLF)
            .from_writer(output);
        writer.write_record([
            "Started (UTC)",
            "Track",
            "Artist",
            "Player",
            "Listening seconds",
        ])?;
        self.rows(filter, 0, -1, |entry| {
            writer.write_record([
                entry.started.to_rfc3339_opts(SecondsFormat::Secs, true),
                csv_text(entry.title),
                csv_text(entry.artist),
                csv_text(entry.player),
                format!(
                    "{}.{:03}",
                    entry.duration_ms / 1000,
                    entry.duration_ms % 1000
                ),
            ])?;
            Ok(())
        })?;
        writer.flush()?;
        Ok(())
    }
}

fn csv_text(mut text: String) -> String {
    if text.trim_start().starts_with(['=', '+', '-', '@']) || text.starts_with(['\t', '\r', '\n']) {
        text.insert(0, '\'');
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;
    use PlaybackState::{Paused, Playing, Stopped};

    fn observe(history: &History, ms: u64, sessions: &[(u64, PlaybackState, &str)]) {
        let tracks: Vec<_> = sessions
            .iter()
            .map(|(id, state, title)| {
                (
                    SessionInfo {
                        id: *id,
                        display_name: format!("Player {id}"),
                        state: *state,
                        ..Default::default()
                    },
                    Track {
                        title: title.to_string(),
                        artist: "Ártist".into(),
                        ..Default::default()
                    },
                )
            })
            .collect();
        history
            .observe_at(tracks.iter().map(|(s, t)| (s, t)), ms, ms as i64)
            .unwrap();
    }

    fn entries(history: &History, search: &str, since: i64) -> Vec<Entry> {
        let mut entries = Vec::new();
        history
            .view()
            .unwrap()
            .rows(
                &Filter {
                    search: search.into(),
                    since,
                },
                0,
                -1,
                |e| {
                    entries.push(e);
                    Ok(())
                },
            )
            .unwrap();
        entries
    }

    #[test]
    fn durations_survive_restarts_and_exclude_pauses() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("history.sqlite3");
        let history = History::new(Some(path.clone()));
        observe(&history, 0, &[(1, Playing, "Before opt-in")]);
        assert!(entries(&history, "", 0).is_empty() && !path.exists());
        history.set_enabled(true).unwrap();
        observe(
            &history,
            1000,
            &[(1, Playing, "First"), (2, Playing, "Other")],
        );
        let rev = history.revision();
        observe(
            &history,
            1100,
            &[(1, Playing, "First"), (2, Playing, "Other")],
        );
        assert_eq!(history.revision(), rev);
        observe(
            &history,
            3000,
            &[(1, Paused, "First"), (2, Playing, "Other")],
        );
        observe(&history, 8000, &[(1, Playing, "First")]);
        observe(&history, 9500, &[(1, Playing, "Next")]);
        observe(&history, 10000, &[(1, Stopped, "Next")]);
        drop(history);

        let reopened = History::new(Some(path));
        let rows = entries(&reopened, "árTIST", 0);
        let summary: Vec<_> = rows
            .iter()
            .map(|e| (e.title.as_str(), e.duration_ms))
            .collect();
        assert_eq!(summary, [("Next", 500), ("Other", 7000), ("First", 3500)]);
        assert_eq!(
            entries(&reopened, "next", 2000)[0]
                .started
                .timestamp_millis(),
            9500
        );
        assert_eq!(entries(&reopened, "Player 2", 0).len(), 1);
        assert!(entries(&reopened, "First", 2000).is_empty());
    }

    #[test]
    fn clear_during_playback_and_disable_never_restore_deleted_rows() {
        let dir = tempfile::tempdir().unwrap();
        let history = History::new(Some(dir.path().join("history.sqlite3")));
        history.set_enabled(true).unwrap();
        observe(&history, 0, &[(1, Playing, "A")]);
        history.clear().unwrap();
        observe(&history, 2000, &[(1, Playing, "A")]);
        assert!(entries(&history, "", 0).is_empty());
        observe(&history, 3000, &[(1, Paused, "A")]);
        observe(&history, 4000, &[(1, Playing, "A")]);
        history.set_enabled(false).unwrap();
        observe(&history, 5000, &[(1, Playing, "B")]);
        assert_eq!(entries(&history, "", 0).len(), 1);
        history.clear().unwrap();
        assert!(entries(&history, "", 0).is_empty());
    }

    #[test]
    fn csv_streams_all_matches_from_a_consistent_snapshot() {
        let dir = tempfile::tempdir().unwrap();
        let history = History::new(Some(dir.path().join("history.sqlite3")));
        history.set_enabled(true).unwrap();
        let title = "=Title, \"live\"\nβ";
        observe(&history, 0, &[(1, Playing, title), (2, Playing, "Other")]);
        observe(&history, 1234, &[]);
        let view = history.view().unwrap();
        history.clear().unwrap();
        let path = dir.path().join("listens.csv");
        let filter = Filter {
            search: "title".into(),
            since: 0,
        };
        view.export(&filter, &path).unwrap();
        let mut csv = csv::Reader::from_path(path).unwrap();
        let records: Vec<_> = csv
            .records()
            .collect::<std::result::Result<_, _>>()
            .unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!(
            records[0].iter().collect::<Vec<_>>(),
            [
                "1970-01-01T00:00:00Z",
                &format!("'{title}"),
                "Ártist",
                "Player 1",
                "1.234"
            ]
        );
        assert!(view.export(&filter, history.path().unwrap()).is_err());
    }
}
