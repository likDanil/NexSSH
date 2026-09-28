//! UI settings persisted as `settings.json` next to `servers.json`.
//!
//! The schema belongs to the UI; the backend stores the object as-is and only reads
//! the theme to paint the window background before the page loads (no white flash).

use std::path::{Path, PathBuf};

use serde_json::{Map, Value};

pub struct Settings {
    path: PathBuf,
    value: Value,
}

impl Settings {
    pub fn load(data_dir: &Path) -> Settings {
        let path = data_dir.join("settings.json");
        let value = std::fs::read(&path)
            .ok()
            .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
            .filter(Value::is_object)
            .unwrap_or_else(|| Value::Object(Map::new()));
        Settings { path, value }
    }

    pub fn value(&self) -> &Value {
        &self.value
    }

    pub fn save(&mut self, value: Value) -> std::io::Result<()> {
        if !value.is_object() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "settings must be an object",
            ));
        }
        let json = serde_json::to_vec_pretty(&value)?;
        let tmp = self.path.with_extension("json.tmp");
        std::fs::write(&tmp, json)?;
        std::fs::rename(&tmp, &self.path)?;
        self.value = value;
        Ok(())
    }

    pub fn theme(&self) -> &str {
        self.value
            .get("theme")
            .and_then(Value::as_str)
            .unwrap_or("light")
    }
}

/// Window background per theme (must match `--bg-frame` in the UI's theme CSS).
pub fn theme_background(theme: &str, system_dark: bool) -> (u8, u8, u8) {
    let theme = match theme {
        "system" if system_dark => "graphite",
        "system" => "light",
        other => other,
    };
    match theme {
        "graphite" => (0x22, 0x24, 0x28),
        "black" => (0x0c, 0x0d, 0x0f),
        "navy" => (0x17, 0x1c, 0x29),
        _ => (0xec, 0xee, 0xf1),
    }
}
