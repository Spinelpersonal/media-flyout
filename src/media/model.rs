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

#[derive(Clone, Debug, Default)]
pub struct SessionInfo {
    pub id: u64,
    pub app_id: String,
    pub state: PlaybackState,
}

#[derive(Clone, Debug, Default)]
pub struct Snapshot {
    pub sessions: Vec<SessionInfo>,
    pub selected: Option<u64>,
    pub track: Track,
    pub state: PlaybackState,
    pub controls: Controls,
    pub status: Option<String>,
}

/// Playing sessions outrank a stale Windows "current" session.
pub fn select_session(sessions: &[SessionInfo], current: Option<u64>) -> Option<usize> {
    let current_index = sessions.iter().position(|s| Some(s.id) == current);
    current_index
        .filter(|&i| sessions[i].state == PlaybackState::Playing)
        .or_else(|| {
            sessions
                .iter()
                .position(|s| s.state == PlaybackState::Playing)
        })
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
            state,
        }
    }

    #[test]
    fn playing_player_outranks_paused_browser() {
        let sessions = [
            session(1, PlaybackState::Paused),
            session(2, PlaybackState::Playing),
        ];
        assert_eq!(select_session(&sessions, Some(1)), Some(1));
    }

    #[test]
    fn windows_preference_breaks_playing_tie() {
        let sessions = [
            session(1, PlaybackState::Playing),
            session(2, PlaybackState::Playing),
        ];
        assert_eq!(select_session(&sessions, Some(2)), Some(1));
    }

    #[test]
    fn handles_empty_and_disappearing_current() {
        assert_eq!(select_session(&[], Some(9)), None);
        assert_eq!(
            select_session(&[session(1, PlaybackState::Paused)], Some(9)),
            Some(0)
        );
    }
}
