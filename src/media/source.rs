/// Resolve once when a session is discovered, away from the UI thread. Keep the
/// original application ID separately for reconciliation and saved preferences.
pub(super) fn display_name(app_id: &str) -> String {
    resolve_with(app_id, crate::platform::app_display_name)
}

fn resolve_with(app_id: &str, lookup: impl FnOnce(&str) -> Option<String>) -> String {
    let lower = app_id.to_ascii_lowercase();
    let has_token = |token| {
        lower
            .split(|c: char| !c.is_ascii_alphanumeric())
            .any(|part| part == token)
    };
    for (token, name) in [
        ("spotify", "Spotify"),
        ("foobar2000", "foobar2000"),
        ("tidal", "TIDAL"),
    ] {
        if has_token(token) {
            return name.into();
        }
    }

    // Gecko browsers can publish installation-specific hashes. Windows can map
    // those IDs to names without guessing a browser from a particular hash.
    // Look up web apps before browser aliases so their own names are preserved.
    if let Some(name) = lookup(app_id) {
        let name = name.trim();
        if name != app_id && readable(name) {
            return name.into();
        }
    }
    for (token, name) in [
        ("firefox", "Firefox"),
        ("zen", "Zen Browser"),
        ("zenbrowser", "Zen Browser"),
        ("chrome", "Chrome"),
        ("msedge", "Edge"),
    ] {
        if has_token(token) {
            return name.into();
        }
    }

    let name = app_id.rsplit('!').next().unwrap_or(app_id);
    let name = name.rsplit(['\\', '/']).next().unwrap_or(name).trim();
    let name = if name.to_ascii_lowercase().ends_with(".exe") {
        &name[..name.len() - 4]
    } else {
        name
    };
    if readable(name) {
        name.into()
    } else {
        "Media player".into()
    }
}

fn readable(name: &str) -> bool {
    if name.is_empty() || name.contains(['<', '>']) || name.chars().any(char::is_control) {
        return false;
    }
    let hash = name.trim_matches(['{', '}']).replace('-', "");
    !(hash.len() >= 12 && hash.bytes().all(|c| c.is_ascii_hexdigit()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognizes_music_players_without_a_shell_registration() {
        for (id, expected) in [
            ("com.squirrel.TIDAL.TIDAL", "TIDAL"),
            ("Tidal.exe", "TIDAL"),
            ("TIDALMusicAS.TIDAL_abc123!TIDAL", "TIDAL"),
            ("Spotify.exe", "Spotify"),
            ("SpotifyAB.SpotifyMusic_zpdnekdrzrea0!Spotify", "Spotify"),
            ("foobar2000.exe", "foobar2000"),
        ] {
            assert_eq!(resolve_with(id, |_| None), expected, "{id}");
        }
    }

    #[test]
    fn resolves_opaque_browser_ids_using_windows_names() {
        // Hashes are installation-specific, not identifiers for YouTube itself.
        for id in ["F0DC299D809B9700", "308046B0AF4A39CB"] {
            assert_eq!(
                resolve_with(id, |requested| {
                    assert_eq!(requested, id);
                    Some("Zen Browser".into())
                }),
                "Zen Browser"
            );
        }
    }

    #[test]
    fn preserves_installed_web_app_names_before_browser_fallbacks() {
        assert_eq!(
            resolve_with("Chrome._crx_installed_web_app", |_| Some("YouTube".into())),
            "YouTube"
        );
    }

    #[test]
    fn browser_fallbacks_name_the_browser_not_the_website() {
        for (id, expected) in [
            ("firefox.exe", "Firefox"),
            ("zen.exe", "Zen Browser"),
            ("org.zen.ZenBrowser", "Zen Browser"),
            ("chrome.exe", "Chrome"),
            ("MSEdge.EXE", "Edge"),
        ] {
            assert_eq!(resolve_with(id, |_| None), expected);
        }
    }

    #[test]
    fn keeps_readable_fallbacks_and_unicode_display_names() {
        for id in [
            "CustomPlayer.exe",
            "Vendor.Package_hash!CustomPlayer",
            r"C:\Players\CustomPlayer.EXE",
        ] {
            assert_eq!(resolve_with(id, |_| None), "CustomPlayer");
        }
        assert_eq!(
            resolve_with("custom", |_| Some("  Lecteur étendu  ".into())),
            "Lecteur étendu"
        );
        assert_eq!(resolve_with("notspotify.exe", |_| None), "notspotify");
    }

    #[test]
    fn does_not_show_unresolved_hashes_or_invalid_names() {
        for id in [
            "F0DC299D809B9700",
            "FODC299D809B9700>",
            "{01234567-89ab-cdef-0123-456789abcdef}",
            "Package!F0DC299D809B9700",
            "",
            "player\0invalid",
        ] {
            for result in [
                None,
                Some(String::new()),
                Some(id.into()),
                Some("  ".into()),
            ] {
                assert_eq!(resolve_with(id, |_| result), "Media player", "{id}");
            }
        }
    }
}
