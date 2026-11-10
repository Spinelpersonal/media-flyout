use windows::Win32::{
    Foundation::HWND,
    UI::Input::KeyboardAndMouse::{
        HOT_KEY_MODIFIERS, MOD_ALT, MOD_CONTROL, MOD_NOREPEAT, MOD_SHIFT, MOD_WIN, RegisterHotKey,
        UnregisterHotKey,
    },
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Shortcut {
    pub modifiers: u32,
    pub key: u32,
}

impl std::str::FromStr for Shortcut {
    type Err = String;
    fn from_str(text: &str) -> Result<Self, Self::Err> {
        let mut modifiers = 0;
        let mut key = None;
        for part in text.split('+').map(str::trim) {
            let part = part.to_ascii_uppercase();
            let modifier = match part.as_str() {
                "CTRL" | "CONTROL" => Some(MOD_CONTROL.0),
                "ALT" => Some(MOD_ALT.0),
                "SHIFT" => Some(MOD_SHIFT.0),
                "WIN" | "WINDOWS" | "SUPER" => Some(MOD_WIN.0),
                _ => None,
            };
            if let Some(modifier) = modifier {
                if modifiers & modifier != 0 {
                    return Err("Duplicate shortcut modifier".into());
                }
                modifiers |= modifier;
            } else {
                if key.is_some() {
                    return Err("A shortcut must contain exactly one key".into());
                }
                key = Some(match part.as_str() {
                    "SPACE" => 0x20,
                    "TAB" => 0x09,
                    "ENTER" | "RETURN" => 0x0D,
                    "INSERT" => 0x2D,
                    "DELETE" => 0x2E,
                    "HOME" => 0x24,
                    "END" => 0x23,
                    "PAGEUP" => 0x21,
                    "PAGEDOWN" => 0x22,
                    "UP" => 0x26,
                    "DOWN" => 0x28,
                    "LEFT" => 0x25,
                    "RIGHT" => 0x27,
                    "PAUSE" => 0x13,
                    _ if part.len() == 1 && part.as_bytes()[0].is_ascii_alphanumeric() => {
                        u32::from(part.as_bytes()[0])
                    }
                    _ if part.starts_with('F') => {
                        let number = part[1..]
                            .parse::<u32>()
                            .map_err(|_| "Invalid function key")?;
                        if !(1..=24).contains(&number) || number == 12 {
                            return Err("Use F1–F24 except F12, which Windows reserves".into());
                        }
                        0x6F + number
                    }
                    _ => return Err(format!("Unsupported shortcut key: {part}")),
                });
            }
        }
        let key = key.ok_or("Shortcut is missing its key")?;
        if modifiers == 0 && !(0x70..=0x87).contains(&key) {
            return Err("A global shortcut requires a modifier or function key".into());
        }
        Ok(Self { modifiers, key })
    }
}

pub struct RegisteredHotkey {
    hwnd: HWND,
    id: i32,
    shortcut: Shortcut,
}

impl RegisteredHotkey {
    pub fn register(hwnd: HWND, id: i32, shortcut: Shortcut) -> windows::core::Result<Self> {
        // SAFETY: Registration is performed on the HWND's owning UI thread and is
        // balanced by Drop. MOD_NOREPEAT prevents key-repeat visibility flicker.
        unsafe {
            RegisterHotKey(
                Some(hwnd),
                id,
                HOT_KEY_MODIFIERS(shortcut.modifiers | MOD_NOREPEAT.0),
                shortcut.key,
            )?;
        }
        Ok(Self { hwnd, id, shortcut })
    }
    pub fn shortcut(&self) -> Shortcut {
        self.shortcut
    }
    pub fn id(&self) -> i32 {
        self.id
    }
}

impl Drop for RegisteredHotkey {
    fn drop(&mut self) {
        // SAFETY: The owned registration is removed before its HWND is destroyed.
        unsafe {
            let _ = UnregisterHotKey(Some(self.hwnd), self.id);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn parses_shortcuts_and_aliases() {
        assert_eq!(
            "Ctrl + Space".parse::<Shortcut>(),
            Ok(Shortcut {
                modifiers: MOD_CONTROL.0,
                key: 0x20
            })
        );
        assert_eq!(
            "Alt+Shift+F9".parse::<Shortcut>(),
            Ok(Shortcut {
                modifiers: MOD_ALT.0 | MOD_SHIFT.0,
                key: 0x78
            })
        );
        assert_eq!("control+q".parse::<Shortcut>(), "CTRL+Q".parse());
    }
    #[test]
    fn rejects_ambiguous_and_reserved_shortcuts() {
        for text in [
            "",
            "Ctrl",
            "Ctrl+",
            "Ctrl+Ctrl+A",
            "Ctrl+A+B",
            "Space",
            "F12",
            "Alt+F25",
            "Ctrl+Escape",
        ] {
            assert!(text.parse::<Shortcut>().is_err(), "{text}");
        }
    }
}
