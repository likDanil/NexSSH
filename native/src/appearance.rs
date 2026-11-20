//! Read the 1.x visual preferences; keep preview changes in their own file.
use crate::AppWindow;
use std::path::Path;

pub fn load(ui: &AppWindow, dir: &Path) {
    let read = |name| {
        std::fs::read(dir.join(name))
            .ok()
            .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok())
    };
    let value = read("native-settings.json")
        .or_else(|| read("settings.json"))
        .unwrap_or_default();
    ui.set_theme(match value["theme"].as_str() {
        Some("graphite") => 1,
        Some("black") => 2,
        Some("navy") => 3,
        Some("system") => -1,
        _ => 0,
    });
    ui.set_sidebar_width(
        value["sidebarWidth"]
            .as_f64()
            .unwrap_or(240.0)
            .clamp(190.0, 380.0) as f32,
    );
    ui.set_font_size(value["fontSize"].as_f64().unwrap_or(13.0).clamp(8.0, 36.0) as f32);
    ui.set_line_height(value["lineHeight"].as_f64().unwrap_or(1.25).clamp(1.0, 2.0) as f32);
    ui.set_show_status(value["statusBar"].as_bool().unwrap_or(true));
    if let Some(font) = value["fontFamily"]
        .as_str()
        .filter(|s| !s.trim().is_empty())
    {
        ui.set_terminal_font(font.into());
    }
}

pub fn save(ui: &AppWindow, dir: &Path) -> std::io::Result<()> {
    let theme = if ui.get_theme() < 0 {
        "system"
    } else {
        ["light", "graphite", "black", "navy"][ui.get_theme().clamp(0, 3) as usize]
    };
    let value = serde_json::json!({
        "theme": theme,
        "sidebarWidth": ui.get_sidebar_width(), "fontSize": ui.get_font_size(),
        "lineHeight": ui.get_line_height(), "fontFamily": ui.get_terminal_font().as_str(),
        "statusBar": ui.get_show_status(),
    });
    let path = dir.join("native-settings.json");
    let temporary = path.with_extension("json.tmp");
    std::fs::write(&temporary, serde_json::to_vec_pretty(&value)?)?;
    std::fs::rename(temporary, path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inherits_legacy_preferences_and_keeps_preview_changes_separate() {
        i_slint_backend_testing::init_no_event_loop();
        let ui = AppWindow::new().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let legacy = br#"{"theme":"navy","sidebarWidth":280,"fontSize":17,"lineHeight":1.5,"statusBar":false}"#;
        std::fs::write(dir.path().join("settings.json"), legacy).unwrap();
        load(&ui, dir.path());
        assert_eq!(ui.get_theme(), 3);
        assert_eq!(ui.get_sidebar_width(), 280.0);
        assert_eq!(ui.get_font_size(), 17.0);
        assert!(!ui.get_show_status());
        ui.set_theme(-1);
        save(&ui, dir.path()).unwrap();
        ui.set_theme(0);
        load(&ui, dir.path());
        assert_eq!(ui.get_theme(), -1);
        assert_eq!(
            std::fs::read(dir.path().join("settings.json")).unwrap(),
            legacy
        );
    }
}
