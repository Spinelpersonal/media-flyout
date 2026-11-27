use super::{PlaybackState, SessionInfo, Track};
use std::{
    collections::{HashMap, hash_map::Entry},
    sync::atomic::{AtomicUsize, Ordering},
};

#[derive(Default)]
pub(crate) struct Notifications(AtomicUsize);

impl Notifications {
    pub fn set_enabled(&self, enabled: bool) {
        let current = self.generation();
        if !current.is_multiple_of(2) != enabled {
            self.0.fetch_add(1, Ordering::Release);
        }
    }

    pub fn generation(&self) -> usize {
        self.0.load(Ordering::Acquire)
    }
}

pub(crate) struct TrackStart {
    pub generation: usize,
    pub session_id: u64,
    pub player: String,
    pub track: Track,
}

impl TrackStart {
    pub fn matches(&self, session: Option<u64>, track: &Track) -> bool {
        session == Some(self.session_id) && same_track(&self.track, track)
    }
}

fn same_track(old: &Track, new: &Track) -> bool {
    // Players sometimes fill in missing fields after publishing the title.
    fn compatible(old: &str, new: &str) -> bool {
        old == new || old.is_empty() || new.is_empty()
    }

    old.title == new.title
        && compatible(&old.artist, &new.artist)
        && compatible(&old.album, &new.album)
}

#[derive(Default)]
pub(super) struct TrackStarts {
    generation: usize,
    playing_tracks: HashMap<u64, Track>,
}

impl TrackStarts {
    pub fn observe<'a>(
        &mut self,
        generation: usize,
        selected: Option<u64>,
        sessions: impl Iterator<Item = (&'a SessionInfo, &'a Track)> + Clone,
    ) -> Option<TrackStart> {
        let baseline = self.generation != generation;
        if baseline {
            self.generation = generation;
            self.playing_tracks.clear();
        }
        if generation.is_multiple_of(2) {
            return None;
        }
        self.playing_tracks.retain(|id, _| {
            sessions
                .clone()
                .any(|(info, _)| info.id == *id && info.state != PlaybackState::Stopped)
        });

        let mut notice = None;
        for (info, track) in sessions {
            if info.state != PlaybackState::Playing || track.title.trim().is_empty() {
                continue;
            }
            let changed = match self.playing_tracks.entry(info.id) {
                Entry::Occupied(mut entry) => {
                    let old = entry.get_mut();
                    if same_track(old, track) {
                        if !track.artist.is_empty() && old.artist.is_empty() {
                            old.artist.clone_from(&track.artist);
                        }
                        if !track.album.is_empty() && old.album.is_empty() {
                            old.album.clone_from(&track.album);
                        }
                        continue;
                    }
                    entry.into_mut()
                }
                Entry::Vacant(entry) => entry.insert(Track::default()),
            };
            *changed = Track {
                artwork: None,
                ..track.clone()
            };
            if !baseline && (notice.is_none() || selected == Some(info.id)) {
                notice = Some(TrackStart {
                    generation,
                    session_id: info.id,
                    player: info.display_name.clone(),
                    track: track.clone(),
                });
            }
        }
        notice
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use PlaybackState::{Paused, Playing, Stopped};

    #[test]
    fn announces_playback_changes_across_sessions_without_repeating_metadata_updates() {
        let mut starts = TrackStarts::default();
        let mut first = SessionInfo {
            id: 1,
            state: Playing,
            ..Default::default()
        };
        let mut second = SessionInfo {
            id: 2,
            state: Paused,
            ..Default::default()
        };
        let mut song = Track {
            title: "Opening".into(),
            ..Default::default()
        };
        let other = Track {
            title: "Background track".into(),
            ..Default::default()
        };
        assert!(
            starts
                .observe(1, Some(1), [(&first, &song), (&second, &other)].into_iter())
                .is_none()
        );
        song.artist = "Artist".into();
        assert!(
            starts
                .observe(1, Some(2), [(&first, &song), (&second, &other)].into_iter())
                .is_none()
        );
        second.state = Playing;
        let notice = starts
            .observe(1, Some(1), [(&first, &song), (&second, &other)].into_iter())
            .unwrap();
        assert_eq!(notice.session_id, 2);
        first.state = Paused;
        song.title = "Next track".into();
        assert!(
            starts
                .observe(1, Some(1), [(&first, &song), (&second, &other)].into_iter())
                .is_none()
        );
        first.state = Playing;
        assert_eq!(
            starts
                .observe(1, Some(1), [(&first, &song), (&second, &other)].into_iter())
                .unwrap()
                .track
                .title,
            "Next track"
        );
        for state in [Paused, Playing, Stopped] {
            first.state = state;
            assert!(
                starts
                    .observe(1, Some(1), [(&first, &song), (&second, &other)].into_iter())
                    .is_none()
            );
        }
        first.state = Playing;
        assert!(
            starts
                .observe(1, Some(1), [(&first, &song), (&second, &other)].into_iter())
                .is_some()
        );
        song.title = "While disabled".into();
        assert!(
            starts
                .observe(2, Some(1), [(&first, &song)].into_iter())
                .is_none()
        );
        assert!(
            starts
                .observe(3, Some(1), [(&first, &song)].into_iter())
                .is_none()
        );
    }
}
