#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum PlaybackState {
    Playing,
    Paused,
    #[default]
    Stopped,
}

#[derive(Clone, Debug, Default)]
pub struct Controls {
    pub play_pause: bool,
    pub previous: bool,
    pub next: bool,
    pub seek: bool,
}

#[derive(Clone, Debug, Default)]
pub struct Track {
    pub title: String,
    pub artist: String,
    pub album: String,
    pub artwork: Option<std::sync::Arc<Artwork>>,
}

pub struct Artwork {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

impl std::fmt::Debug for Artwork {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Artwork")
            .field("width", &self.width)
            .field("height", &self.height)
            .field("bytes", &self.rgba.len())
            .finish()
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SessionInfo {
    pub id: u64,
    /// Original Windows identity, used for reconciliation and preferences.
    pub app_id: String,
    /// Resolved on the media worker, for display only.
    pub display_name: String,
    pub state: PlaybackState,
}

#[derive(Clone, Debug, Default)]
pub struct Snapshot {
    pub sessions: Vec<SessionInfo>,
    pub selected: Option<u64>,
    pub manual: bool,
    pub track: Track,
    pub track_revision: u64,
    pub state: PlaybackState,
    pub controls: Controls,
    pub timeline: Option<super::Timeline>,
    pub status: Option<String>,
}

/// Playing sessions outrank a stale Windows "current" session.
pub fn select_session(
    sessions: &[SessionInfo],
    current: Option<u64>,
    preferred_app: Option<&str>,
    manual: Option<u64>,
) -> Option<usize> {
    if let Some(index) = sessions.iter().position(|s| Some(s.id) == manual) {
        return Some(index);
    }
    let current_index = sessions.iter().position(|s| Some(s.id) == current);
    let preferred = sessions
        .iter()
        .position(|s| preferred_app.is_some_and(|app| app.eq_ignore_ascii_case(&s.app_id)));
    sessions
        .iter()
        .position(|s| {
            s.state == PlaybackState::Playing
                && preferred_app.is_some_and(|app| app.eq_ignore_ascii_case(&s.app_id))
        })
        .or(current_index.filter(|&i| sessions[i].state == PlaybackState::Playing))
        .or_else(|| {
            sessions
                .iter()
                .position(|s| s.state == PlaybackState::Playing)
        })
        .or(preferred)
        .or(current_index)
        .or_else(|| (!sessions.is_empty()).then_some(0))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn session(id: u64, state: PlaybackState) -> SessionInfo {
        SessionInfo {
            id,
            app_id: format!("player-{id}"),
            display_name: format!("Player {id}"),
            state,
        }
    }

    #[test]
    fn playing_player_outranks_paused_browser() {
        let sessions = [
            session(1, PlaybackState::Paused),
            session(2, PlaybackState::Playing),
        ];
        assert_eq!(select_session(&sessions, Some(1), None, None), Some(1));
    }

    #[test]
    fn windows_preference_breaks_playing_tie() {
        let sessions = [
            session(1, PlaybackState::Playing),
            session(2, PlaybackState::Playing),
        ];
        assert_eq!(select_session(&sessions, Some(2), None, None), Some(1));
    }

    #[test]
    fn handles_empty_and_disappearing_current() {
        assert_eq!(select_session(&[], Some(9), None, None), None);
        assert_eq!(
            select_session(&[session(1, PlaybackState::Paused)], Some(9), None, None),
            Some(0)
        );
    }

    #[test]
    fn preferences_respect_playback_and_manual_choice() {
        let sessions = [
            session(1, PlaybackState::Paused),
            session(2, PlaybackState::Playing),
        ];
        assert_eq!(
            select_session(&sessions, Some(1), Some("player-1"), None),
            Some(1)
        );
        assert_eq!(select_session(&sessions, Some(2), None, Some(1)), Some(0));
        assert_eq!(select_session(&sessions, Some(2), None, Some(99)), Some(1));
        let sessions = [
            session(1, PlaybackState::Playing),
            session(2, PlaybackState::Playing),
        ];
        assert_eq!(
            select_session(&sessions, Some(2), Some("PLAYER-1"), None),
            Some(0)
        );
    }

    #[test]
    fn finds_playing_instance_of_preferred_application() {
        let sessions = [
            SessionInfo {
                id: 1,
                app_id: "player".into(),
                display_name: "Player".into(),
                state: PlaybackState::Paused,
            },
            SessionInfo {
                id: 2,
                app_id: "other".into(),
                display_name: "Other".into(),
                state: PlaybackState::Playing,
            },
            SessionInfo {
                id: 3,
                app_id: "player".into(),
                display_name: "Player".into(),
                state: PlaybackState::Playing,
            },
        ];
        assert_eq!(
            select_session(&sessions, Some(2), Some("player"), None),
            Some(2)
        );
    }

    #[test]
    fn preferences_use_original_ids_even_when_display_names_match() {
        let sessions = [
            SessionInfo {
                id: 1,
                app_id: "F0DC299D809B9700".into(),
                display_name: "Zen Browser".into(),
                state: PlaybackState::Playing,
            },
            SessionInfo {
                id: 2,
                app_id: "another-installation".into(),
                display_name: "Zen Browser".into(),
                state: PlaybackState::Playing,
            },
        ];
        assert_eq!(
            select_session(&sessions, Some(2), Some("f0dc299d809b9700"), None),
            Some(0)
        );
        assert_eq!(
            select_session(&sessions, Some(2), Some("Zen Browser"), None),
            Some(1)
        );
    }
}
