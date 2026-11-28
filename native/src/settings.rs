//! Complete 1.x preference schema with native overrides and atomic, debounced persistence.
use crate::{AppWindow, i18n};
use serde_json::{Map, Value};
use std::{
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

pub fn defaults() -> Value {
    serde_json::from_str(include_str!(concat!(
        env!("OUT_DIR"),
        "/settings_defaults.json"
    )))
    .expect("generated defaults")
}

pub struct Settings {
    pub value: Value,
    path: PathBuf,
    dirty: Option<Instant>,
    pub recovery: Option<String>,
}

fn merge(destination: &mut Value, patch: Value) {
    if let (Some(destination), Some(patch)) = (destination.as_object_mut(), patch.as_object()) {
        for (key, value) in patch {
            if destination.get(key).is_some_and(Value::is_object) && value.is_object() {
                merge(destination.get_mut(key).unwrap(), value.clone());
            } else {
                destination.insert(key.clone(), value.clone());
            }
        }
    }
}

impl Settings {
    pub fn load(dir: &Path) -> std::io::Result<Self> {
        let mut value = defaults();
        let mut recovery = None;
        for name in ["settings.json", "native-settings.json"] {
            let path = dir.join(name);
            match std::fs::read(&path) {
                Ok(bytes) => match serde_json::from_slice::<Value>(&bytes)
                    .ok()
                    .filter(Value::is_object)
                {
                    Some(patch) => merge(&mut value, patch),
                    None => {
                        // Preserve the original file. Only the native file is ours to repair.
                        if name == "native-settings.json" {
                            let stamp = std::time::SystemTime::now()
                                .duration_since(std::time::UNIX_EPOCH)
                                .unwrap_or_default()
                                .as_nanos();
                            let preserved = path.with_extension(format!("json.broken-{stamp}"));
                            std::fs::rename(&path, &preserved)?;
                            recovery = Some(preserved.display().to_string());
                        }
                    }
                },
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(error),
            }
        }
        // Invalid individual fields fall back independently; newer/unknown fields survive.
        let baseline = defaults();
        for (key, fallback) in baseline.as_object().unwrap() {
            if !same_type(&value[key], fallback) {
                value[key] = fallback.clone();
            }
        }
        let mut settings = Self {
            value,
            path: dir.join("native-settings.json"),
            dirty: None,
            recovery,
        };
        settings.normalize();
        Ok(settings)
    }

    #[cfg(test)]
    pub fn memory(dir: &Path) -> Self {
        Self {
            value: defaults(),
            path: dir.join("native-settings.json"),
            dirty: None,
            recovery: None,
        }
    }
    pub fn text(&self, key: &str) -> &str {
        self.get(key).and_then(Value::as_str).unwrap_or("")
    }
    pub fn boolean(&self, key: &str) -> bool {
        self.get(key).and_then(Value::as_bool).unwrap_or(false)
    }
    pub fn number(&self, key: &str) -> f64 {
        self.get(key).and_then(Value::as_f64).unwrap_or(0.0)
    }
    pub fn get(&self, key: &str) -> Option<&Value> {
        match key.split_once('.') {
            Some((outer, inner)) => self.value.get(outer)?.get(inner),
            None => self.value.get(key),
        }
    }
    pub fn replace(&mut self, value: Value) {
        let mut next = defaults();
        merge(&mut next, value);
        self.value = next;
        self.normalize();
        self.dirty = Some(Instant::now());
    }
    pub fn set_value(&mut self, key: &str, value: Value) {
        if self.value.get(key) != Some(&value) {
            self.value[key] = value;
            self.dirty = Some(Instant::now());
        }
    }
    pub fn set(&mut self, key: &str, raw: &str) -> Result<(), String> {
        let baseline = defaults();
        let fallback = match key.split_once('.') {
            Some((outer, inner)) => baseline.get(outer).and_then(|v| v.get(inner)),
            None => baseline.get(key),
        }
        .ok_or_else(|| format!("Unknown preference: {key}"))?;
        let value = match fallback {
            Value::String(_) => Value::String(raw.into()),
            Value::Bool(_) => match raw {
                "true" => Value::Bool(true),
                "false" => Value::Bool(false),
                _ => return Err("Expected true or false".into()),
            },
            Value::Number(_) => serde_json::from_str::<Value>(raw)
                .ok()
                .filter(Value::is_number)
                .ok_or("Enter a number")?,
            _ => serde_json::from_str(raw).map_err(|e| e.to_string())?,
        };
        if key.starts_with("highlightColors.") && !valid_color(raw) {
            return Err("Use #RRGGBB".into());
        }
        if key == "localShellCommand" && !raw.trim().is_empty() {
            nexssh_core::LocalCommand::parse(raw, None).map_err(|e| e.to_string())?;
        }
        if self.get(key) == Some(&value) {
            return Ok(());
        }
        match key.split_once('.') {
            Some((outer, inner)) => {
                if !self.value[outer].is_object() {
                    self.value[outer] = Value::Object(Map::new());
                }
                self.value[outer][inner] = value;
            }
            None => self.value[key] = value,
        }
        self.normalize();
        self.dirty = Some(Instant::now());
        Ok(())
    }
    fn normalize(&mut self) {
        let baseline = defaults();
        for (key, fallback) in baseline.as_object().unwrap() {
            if !same_type(&self.value[key], fallback) {
                self.value[key] = fallback.clone();
            }
        }
        for (key, low, high) in [
            ("fontSize", 9.0, 28.0),
            ("lineHeight", 1.0, 1.5),
            ("sidebarWidth", 190.0, 380.0),
            ("filesWidth", 300.0, 640.0),
            ("scrollback", 1000.0, 100000.0),
            ("agentsPort", 1.0, 65535.0),
        ] {
            let number = self.number(key).clamp(low, high);
            self.value[key] = if number.fract() == 0.0 {
                Value::from(number as u64)
            } else {
                Value::from(number)
            };
        }
        for (key, choices, fallback) in [
            ("language", &["system", "ru", "en"][..], "system"),
            (
                "theme",
                &["system", "light", "graphite", "black", "navy"][..],
                "light",
            ),
            ("cursorStyle", &["bar", "block", "underline"][..], "bar"),
            ("pasteWarning", &["auto", "always", "never"][..], "auto"),
            ("closeAction", &["ask", "tray", "quit"][..], "ask"),
            ("restoreTabs", &["ask", "always", "never"][..], "ask"),
            ("filesDoubleClick", &["download", "edit"][..], "download"),
            ("filesSort", &["name", "size", "modified"][..], "name"),
        ] {
            if !choices.contains(&self.text(key)) {
                self.value[key] = fallback.into();
            }
        }
        let fallback = defaults();
        for kind in ["error", "warning", "ok", "info", "debug", "address"] {
            if !self.value["highlightColors"][kind]
                .as_str()
                .is_some_and(valid_color)
            {
                self.value["highlightColors"][kind] = fallback["highlightColors"][kind].clone();
            }
        }
    }
    pub fn apply(&self, ui: &AppWindow) {
        ui.set_theme(match self.text("theme") {
            "system" => -1,
            "graphite" => 1,
            "black" => 2,
            "navy" => 3,
            _ => 0,
        });
        ui.set_sidebar_width(self.number("sidebarWidth") as f32);
        ui.set_font_size(self.number("fontSize") as f32);
        ui.set_line_height(self.number("lineHeight") as f32);
        ui.set_terminal_font(self.text("fontFamily").into());
        ui.set_show_status(self.boolean("statusBar"));
        ui.set_sidebar_hidden(
            self.boolean("sidebarHidden")
                || (self.boolean("sidebarAutoHide") && ui.get_active_tab() >= 0),
        );
        ui.set_ctrl_k_in_terminal(self.boolean("ctrlKInTerminal"));
        ui.set_cursor_style(self.text("cursorStyle").into());
        ui.set_cursor_blink(self.boolean("cursorBlink"));
        ui.set_files_open(self.boolean("filesOpen"));
        ui.set_files_width(self.number("filesWidth") as f32);
        ui.set_files_hidden(self.boolean("filesShowHidden"));
        ui.set_files_sort(self.text("filesSort").into());
        ui.set_files_sort_descending(self.boolean("filesSortDesc"));
        i18n::apply(ui, self.text("language"));
    }
    pub fn capture(&mut self, ui: &AppWindow) {
        for (key, value) in [
            (
                "theme",
                Value::from(if ui.get_theme() < 0 {
                    "system"
                } else {
                    ["light", "graphite", "black", "navy"][ui.get_theme().clamp(0, 3) as usize]
                }),
            ),
            ("sidebarWidth", Value::from(ui.get_sidebar_width())),
            ("fontSize", Value::from(ui.get_font_size())),
            (
                "lineHeight",
                Value::from((f64::from(ui.get_line_height()) * 100.0).round() / 100.0),
            ),
            ("fontFamily", Value::from(ui.get_terminal_font().as_str())),
            ("statusBar", Value::from(ui.get_show_status())),
            ("filesWidth", Value::from(ui.get_files_width())),
        ] {
            if self.value[key] != value {
                self.set_value(key, value);
            }
        }
        // Automatic hiding is temporary, and must not overwrite the manual preference.
        if !self.boolean("sidebarAutoHide") {
            self.set_value("sidebarHidden", ui.get_sidebar_hidden().into());
        }
    }
    pub fn flush_if_due(&mut self) -> std::io::Result<bool> {
        if self
            .dirty
            .is_some_and(|at| at.elapsed() >= Duration::from_millis(350))
        {
            self.flush()?;
            return Ok(true);
        }
        Ok(false)
    }
    pub fn flush(&mut self) -> std::io::Result<()> {
        if self.dirty.is_none() {
            return Ok(());
        }
        if let Some(dir) = self.path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let tmp = self.path.with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_vec_pretty(&self.value)?)?;
        std::fs::rename(tmp, &self.path)?;
        self.dirty = None;
        Ok(())
    }
}

fn same_type(value: &Value, fallback: &Value) -> bool {
    matches!(
        (value, fallback),
        (Value::String(_), Value::String(_))
            | (Value::Bool(_), Value::Bool(_))
            | (Value::Number(_), Value::Number(_))
            | (Value::Array(_), Value::Array(_))
            | (Value::Object(_), Value::Object(_))
    ) || fallback.is_null()
}
pub fn valid_color(value: &str) -> bool {
    value.len() == 7 && value.starts_with('#') && value[1..].bytes().all(|c| c.is_ascii_hexdigit())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn legacy_and_native_merge_without_losing_unknown_fields_or_secrets_policy() {
        let dir = tempfile::tempdir().unwrap();
        let legacy=br##"{"theme":"navy","copyOnSelect":true,"highlightColors":{"error":"#123456"},"futureField":{"n":42}}"##;
        std::fs::write(dir.path().join("settings.json"), legacy).unwrap();
        std::fs::write(
            dir.path().join("native-settings.json"),
            br#"{"theme":"light","fontSize":17}"#,
        )
        .unwrap();
        let mut settings = Settings::load(dir.path()).unwrap();
        assert!(settings.boolean("copyOnSelect"));
        assert_eq!(settings.number("fontSize"), 17.0);
        assert_eq!(settings.text("highlightColors.error"), "#123456");
        assert_eq!(settings.value["futureField"]["n"], 42);
        settings.set("scrollback", "50000").unwrap();
        settings.flush().unwrap();
        let reloaded = Settings::load(dir.path()).unwrap();
        assert_eq!(reloaded.number("scrollback"), 50000.0);
        assert_eq!(
            std::fs::read(dir.path().join("settings.json")).unwrap(),
            legacy
        );
    }
    #[test]
    fn damaged_native_file_is_preserved_and_values_are_validated() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("native-settings.json"), b"damaged").unwrap();
        let mut settings = Settings::load(dir.path()).unwrap();
        assert!(Path::new(settings.recovery.as_ref().unwrap()).exists());
        settings.set("fontSize", "99").unwrap();
        assert_eq!(settings.number("fontSize"), 28.0);
        assert!(settings.set("highlightColors.error", "red").is_err());
        assert!(settings.set("cursorBlink", "maybe").is_err());
        settings.flush().unwrap();
        assert_eq!(
            std::fs::read(settings.recovery.unwrap()).unwrap(),
            b"damaged"
        );
    }
}
