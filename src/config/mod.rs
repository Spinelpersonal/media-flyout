use std::{
    fs::{self, OpenOptions},
    io::{self, Read, Write},
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};
use toml_edit::{DocumentMut, Item};

use crate::hotkey::Shortcut;

const EXAMPLE: &str = include_str!("../../config.example.toml");
const MAX_CONFIG_BYTES: u64 = 64 * 1024;
static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Position {
    #[default]
    TopCenter,
    TopLeft,
    TopRight,
    BottomCenter,
    BottomLeft,
    BottomRight,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Monitor {
    #[default]
    Active,
    Cursor,
    Primary,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Theme {
    #[default]
    Dark,
    Light,
    System,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Settings {
    pub shortcut: String,
    pub position: Position,
    pub monitor: Monitor,
    pub margin: u16,
    pub preferred_app: Option<String>,
    pub remember_preferred_app: bool,
    pub theme: Theme,
    pub autostart: bool,
    pub animations: bool,
    pub show_progress: bool,
    pub dismiss_after_seconds: u16,
    pub dismiss_on_outside_click: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            shortcut: "Ctrl+Space".into(),
            position: Position::TopCenter,
            monitor: Monitor::Active,
            margin: 20,
            preferred_app: None,
            remember_preferred_app: false,
            theme: Theme::Dark,
            autostart: false,
            animations: true,
            show_progress: true,
            dismiss_after_seconds: 10,
            dismiss_on_outside_click: true,
        }
    }
}

pub struct ConfigStore {
    pub path: PathBuf,
    pub settings: Settings,
    pub warnings: Vec<String>,
}

impl ConfigStore {
    pub fn load(path: Option<PathBuf>) -> io::Result<Self> {
        let path = match path {
            Some(path) => path,
            None => default_path()?,
        };
        let path = std::path::absolute(path)?;
        let mut store = Self {
            path,
            settings: Settings::default(),
            warnings: Vec::new(),
        };
        if !store.path.exists()
            && let Err(error) = store.create()
        {
            store
                .warnings
                .push(format!("Cannot create settings: {error}"));
        }
        if let Err(error) = store.reload() {
            store
                .warnings
                .push(format!("Cannot read settings; using defaults: {error}"));
        }
        Ok(store)
    }

    fn create(&self) -> io::Result<()> {
        if let Some(parent) = self.path.parent().filter(|p| !p.as_os_str().is_empty()) {
            fs::create_dir_all(parent)?;
        }
        match OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&self.path)
        {
            Ok(mut file) => {
                file.write_all(EXAMPLE.as_bytes())?;
                file.sync_all()
            }
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => Ok(()),
            Err(error) => Err(error),
        }
    }

    pub fn reload(&mut self) -> io::Result<()> {
        let document = read_document(&self.path)?;
        let (settings, warnings) = parse(&document);
        self.settings = settings;
        self.warnings = warnings;
        Ok(())
    }

    /// Re-read before editing so unrelated manual edits and comments survive.
    /// Malformed files are never replaced with defaults.
    pub fn set(&mut self, key: &str, value: impl Into<toml_edit::Value>) -> io::Result<()> {
        let mut document = read_document(&self.path)?;
        let decor = document
            .get(key)
            .and_then(Item::as_value)
            .map(|value| value.decor().clone());
        document[key] = toml_edit::value(value.into());
        if let (Some(decor), Some(value)) = (decor, document[key].as_value_mut()) {
            *value.decor_mut() = decor;
        }
        atomic_write(&self.path, document.to_string().as_bytes())?;
        let (settings, warnings) = parse(&document);
        self.settings = settings;
        self.warnings = warnings;
        Ok(())
    }
}

pub fn default_path() -> io::Result<PathBuf> {
    std::env::var_os("APPDATA")
        .filter(|value| !value.is_empty())
        .map(|path| PathBuf::from(path).join("MediaFlyout").join("config.toml"))
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::NotFound,
                "APPDATA is unavailable; provide --config <path>",
            )
        })
}

fn read_document(path: &Path) -> io::Result<DocumentMut> {
    let mut text = String::new();
    fs::File::open(path)?
        .take(MAX_CONFIG_BYTES + 1)
        .read_to_string(&mut text)?;
    if text.len() as u64 > MAX_CONFIG_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "Settings file exceeds 64 KiB",
        ));
    }
    text.trim_start_matches('\u{feff}')
        .parse()
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))
}

fn atomic_write(path: &Path, contents: &[u8]) -> io::Result<()> {
    let sequence = NEXT_TEMP.fetch_add(1, Ordering::Relaxed);
    let temp = path.with_extension(format!("toml.{}.{sequence}.tmp", std::process::id()));
    let result = (|| {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temp)?;
        file.write_all(contents)?;
        file.sync_all()?;
        drop(file);
        fs::rename(&temp, path)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temp);
    }
    result
}

fn parse(document: &DocumentMut) -> (Settings, Vec<String>) {
    let mut settings = Settings::default();
    let mut warnings = Vec::new();
    for (key, value) in document.iter() {
        let valid = match key {
            "shortcut" => value
                .as_str()
                .filter(|s| s.parse::<Shortcut>().is_ok())
                .map(|s| settings.shortcut = s.into())
                .is_some(),
            "position" => value
                .as_str()
                .and_then(|s| {
                    Some(match s {
                        "top-center" => Position::TopCenter,
                        "top-left" => Position::TopLeft,
                        "top-right" => Position::TopRight,
                        "bottom-center" => Position::BottomCenter,
                        "bottom-left" => Position::BottomLeft,
                        "bottom-right" => Position::BottomRight,
                        _ => return None,
                    })
                })
                .map(|p| settings.position = p)
                .is_some(),
            "monitor" => value
                .as_str()
                .and_then(|s| {
                    Some(match s {
                        "active" => Monitor::Active,
                        "cursor" => Monitor::Cursor,
                        "primary" => Monitor::Primary,
                        _ => return None,
                    })
                })
                .map(|m| settings.monitor = m)
                .is_some(),
            "theme" => value
                .as_str()
                .and_then(|s| {
                    Some(match s {
                        "dark" => Theme::Dark,
                        "light" => Theme::Light,
                        "system" => Theme::System,
                        _ => return None,
                    })
                })
                .map(|t| settings.theme = t)
                .is_some(),
            "preferred_app" => value
                .as_str()
                .filter(|s| s.len() <= 512 && !s.chars().any(char::is_control))
                .map(|s| {
                    settings.preferred_app = (!s.trim().is_empty()).then(|| s.trim().to_owned())
                })
                .is_some(),
            "margin" => value
                .as_integer()
                .filter(|&n| (0..=256).contains(&n))
                .map(|n| settings.margin = n as u16)
                .is_some(),
            "dismiss_after_seconds" => value
                .as_integer()
                .filter(|&n| (0..=600).contains(&n))
                .map(|n| settings.dismiss_after_seconds = n as u16)
                .is_some(),
            "remember_preferred_app" => value
                .as_bool()
                .map(|b| settings.remember_preferred_app = b)
                .is_some(),
            "autostart" => value.as_bool().map(|b| settings.autostart = b).is_some(),
            "animations" => value.as_bool().map(|b| settings.animations = b).is_some(),
            "show_progress" => value
                .as_bool()
                .map(|b| settings.show_progress = b)
                .is_some(),
            "dismiss_on_outside_click" => value
                .as_bool()
                .map(|b| settings.dismiss_on_outside_click = b)
                .is_some(),
            _ => {
                warnings.push(format!("Unknown setting: {key}"));
                continue;
            }
        };
        if !valid {
            warnings.push(format!("Invalid {key}; using its default"));
        }
    }
    (settings, warnings)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn example_matches_defaults_and_autostart_is_opt_in() {
        let (settings, warnings) = parse(&EXAMPLE.parse().expect("example TOML"));
        assert_eq!(settings, Settings::default());
        assert!(warnings.is_empty());
        assert!(!settings.autostart);
    }
    #[test]
    fn invalid_fields_do_not_discard_valid_preferences() {
        let document = "shortcut = 'F12'\nmargin = -1\ndismiss_after_seconds = 99999\ntheme = 'light'\npreferred_app = 'Spotify.exe'\nautostart = 'yes'\nunrecognized = true".parse().expect("TOML");
        let (settings, warnings) = parse(&document);
        assert_eq!(settings.shortcut, "Ctrl+Space");
        assert_eq!(settings.theme, Theme::Light);
        assert_eq!(settings.preferred_app.as_deref(), Some("Spotify.exe"));
        assert!(!settings.autostart);
        assert_eq!(warnings.len(), 5);
    }
    #[test]
    fn edits_preserve_comments_and_reject_malformed_files() {
        let path = std::env::temp_dir().join(format!(
            "media-flyout-config-test-{}.toml",
            std::process::id()
        ));
        fs::write(
            &path,
            "# My settings\npreferred_app = '' # keep this\ntheme = 'light'\n",
        )
        .expect("test file");
        let mut store = ConfigStore::load(Some(path.clone())).expect("load");
        store.set("preferred_app", "Spotify.exe").expect("save");
        let text = fs::read_to_string(&path).expect("read");
        assert!(text.contains("# keep this"));
        assert!(text.contains("# My settings"));
        assert_eq!(store.settings.theme, Theme::Light);
        fs::write(&path, "theme = [").expect("malformed fixture");
        assert!(store.set("autostart", true).is_err());
        assert_eq!(fs::read_to_string(&path).expect("read"), "theme = [");
        fs::remove_file(path).expect("remove fixture");
    }
}
