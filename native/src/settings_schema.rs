//! The sections and controls of SettingsDialog.svelte, backed by native preferences.
use crate::{Choice, SettingRow, i18n, settings::Settings};
use slint::{ModelRc, VecModel};

pub const SECTIONS: [&str; 7] = [
    "general", "terminal", "files", "keyboard", "agents", "backup", "about",
];
pub struct Context<'a> {
    pub fonts: Option<&'a [String]>,
    pub shells: &'a [nexssh_core::ShellProfile],
    pub editors: &'a [(String, String)],
    pub data_dir: &'a std::path::Path,
    pub agents_status: &'a str,
    pub servers: &'a [nexssh_core::Server],
    pub update_status: &'a str,
    pub keychain_status: &'a str,
}

pub fn choices(values: &[(&str, &str)], language: &str) -> ModelRc<Choice> {
    let rows: Vec<_> = values
        .iter()
        .map(|(value, label)| Choice {
            value: (*value).into(),
            label: if language == "ru" && label.parse::<f64>().is_ok() {
                label.replace('.', ",")
            } else if label.contains('.') {
                i18n::tr(label, language)
            } else {
                (*label).into()
            }
            .into(),
        })
        .collect();
    ModelRc::new(VecModel::from(rows))
}

pub fn rows(settings: &Settings, section: &str, context: &Context<'_>) -> ModelRc<SettingRow> {
    let language = i18n::resolve(settings.text("language"));
    let mut rows = vec![];
    let row = |key: &str, label: &str, hint: &str, kind: i32, values: &[(&str, &str)]| {
        let value = settings.get(key).cloned().unwrap_or_default();
        SettingRow {
            key: key.into(),
            label: i18n::tr(label, language).into(),
            hint: if hint.is_empty() {
                "".into()
            } else {
                i18n::tr(hint, language)
                    .replace("{key}", "Ctrl+Shift+B")
                    .into()
            },
            kind,
            value: value
                .as_str()
                .map(str::to_owned)
                .unwrap_or_else(|| {
                    if value.is_null() {
                        String::new()
                    } else {
                        value
                            .as_f64()
                            .map(|n| n.to_string())
                            .unwrap_or_else(|| value.to_string())
                    }
                })
                .into(),
            checked: value.as_bool().unwrap_or(false),
            choices: choices(values, language),
            enabled: true,
        }
    };
    let toggle = |key, label, hint| row(key, label, hint, 1, &[]);
    let select = |key: &str, label: &str, hint: &str, values: &[(&str, &str)]| {
        row(key, label, hint, 2, values)
    };
    let input = |key, label, hint| row(key, label, hint, 3, &[]);
    let action = |key, label, hint, button| {
        let mut r = row(key, label, hint, 6, &[]);
        r.value = i18n::tr(button, language).into();
        r
    };
    let info = |key, label, text: &str| {
        let mut r = row(key, label, "", 7, &[]);
        r.value = text.into();
        r
    };
    match section {
        "general" => {
            rows.push(select(
                "language",
                "settings.language",
                "",
                &[
                    ("system", "settings.languageSystem"),
                    ("en", "English"),
                    ("ru", "Русский"),
                ],
            ));
            let mut font = select("fontFamily", "settings.font", "", &[]);
            let mut fonts = vec![Choice {
                value: "".into(),
                label: i18n::tr("settings.fontPlaceholder", language).into(),
            }];
            fonts.extend(
                context
                    .fonts
                    .into_iter()
                    .flatten()
                    .filter(|name| name.as_str() != crate::fonts::BUNDLED)
                    .map(|name| Choice {
                        value: name.into(),
                        label: name.into(),
                    }),
            );
            if !font.value.is_empty() && !fonts.iter().any(|f| f.value == font.value) {
                fonts.push(Choice {
                    label: if context.fonts.is_none()
                        || font.value.split(',').any(|name| {
                            name.trim().trim_matches(['\'', '"']).eq_ignore_ascii_case(
                                crate::fonts::resolve(context.fonts, &font.value),
                            )
                        }) {
                        font.value.clone()
                    } else {
                        format!(
                            "{} ({})",
                            font.value,
                            i18n::tr("settings.fontUnavailable", language)
                        )
                        .into()
                    },
                    value: font.value.clone(),
                });
            }
            font.choices = ModelRc::new(VecModel::from(fonts));
            rows.push(font);
            rows.push(row("fontSize", "settings.fontSize", "", 4, &[]));
            rows.push(select(
                "lineHeight",
                "settings.lineHeight",
                "",
                &[
                    ("1", "1"),
                    ("1.1", "1.1"),
                    ("1.2", "1.2"),
                    ("1.25", "1.25"),
                    ("1.3", "1.3"),
                    ("1.4", "1.4"),
                    ("1.5", "1.5"),
                ],
            ));
            rows.push(toggle(
                "sidebarAutoHide",
                "settings.sidebarAutoHide",
                "settings.sidebarAutoHideHint",
            ));
            rows.push(toggle(
                "statusBar",
                "settings.statusBar",
                "settings.statusBarHint",
            ));
            rows.push(select(
                "closeAction",
                "settings.closeAction",
                "settings.closeActionHint",
                &[
                    ("ask", "settings.closeAsk"),
                    ("tray", "settings.closeTray"),
                    ("quit", "settings.closeQuit"),
                ],
            ));
            rows.push(select(
                "restoreTabs",
                "settings.restoreTabs",
                "settings.restoreTabsHint",
                &[
                    ("ask", "settings.restoreAsk"),
                    ("always", "settings.restoreAlways"),
                    ("never", "settings.restoreNever"),
                ],
            ));
        }
        "terminal" => {
            rows.push(select(
                "cursorStyle",
                "settings.cursor",
                "",
                &[
                    ("bar", "settings.cursorBar"),
                    ("block", "settings.cursorBlock"),
                    ("underline", "settings.cursorUnderline"),
                ],
            ));
            rows.push(toggle("cursorBlink", "settings.cursorBlink", ""));
            let mut history = select(
                "scrollback",
                "settings.scrollback",
                "",
                &[
                    (
                        "1000",
                        if language == "ru" {
                            "1 000 строк"
                        } else {
                            "1 000 lines"
                        },
                    ),
                    (
                        "5000",
                        if language == "ru" {
                            "5 000 строк"
                        } else {
                            "5 000 lines"
                        },
                    ),
                    (
                        "10000",
                        if language == "ru" {
                            "10 000 строк"
                        } else {
                            "10 000 lines"
                        },
                    ),
                    (
                        "50000",
                        if language == "ru" {
                            "50 000 строк"
                        } else {
                            "50 000 lines"
                        },
                    ),
                    (
                        "100000",
                        if language == "ru" {
                            "100 000 строк"
                        } else {
                            "100 000 lines"
                        },
                    ),
                ],
            );
            history.hint = if language == "ru" {
                "Размер истории применяется к новым вкладкам."
            } else {
                "The history size applies to new tabs."
            }
            .into();
            rows.push(history);
            rows.push(toggle("copyOnSelect", "settings.copyOnSelect", ""));
            rows.push(select(
                "rightClickPaste",
                "settings.rightClick",
                "",
                &[
                    ("false", "settings.rightClickMenu"),
                    ("true", "settings.rightClickCopyPaste"),
                ],
            ));
            rows.push(select(
                "pasteWarning",
                "settings.pasteWarning",
                "settings.pasteWarningHint",
                &[
                    ("auto", "settings.pasteAuto"),
                    ("always", "settings.pasteAlways"),
                    ("never", "settings.pasteNever"),
                ],
            ));
            let mut gpu = toggle("gpuAcceleration", "settings.gpu", "");
            gpu.hint = if language == "ru" {
                "Изменение отрисовки применяется после перезапуска приложения."
            } else {
                "Changing the renderer takes effect after restarting the application."
            }
            .into();
            rows.push(gpu);
            rows.push(toggle(
                "highlightKeywords",
                "settings.highlight",
                "settings.highlightHint",
            ));
            for (kind, label) in [
                ("error", "settings.highlightError"),
                ("warning", "settings.highlightWarning"),
                ("ok", "settings.highlightOk"),
                ("info", "settings.highlightInfo"),
                ("debug", "settings.highlightDebug"),
                ("address", "settings.highlightAddress"),
            ] {
                let mut color = row(&format!("highlightColors.{kind}"), label, "", 5, &[]);
                color.enabled = settings.boolean("highlightKeywords");
                rows.push(color);
            }
            let mut shell = select(
                "localShell",
                "settings.localShell",
                "settings.localShellHint",
                &[],
            );
            let default = context
                .shells
                .first()
                .map(|s| s.name.as_str())
                .unwrap_or("—");
            let mut options = vec![(
                String::new(),
                i18n::tr("settings.shellDefault", language).replace("{name}", default),
            )];
            options.extend(
                context
                    .shells
                    .iter()
                    .map(|s| (s.id.clone(), s.name.clone())),
            );
            if !settings.text("localShell").is_empty()
                && settings.text("localShell") != "custom"
                && !options
                    .iter()
                    .any(|(v, _)| v == settings.text("localShell"))
            {
                options.push((
                    settings.text("localShell").into(),
                    i18n::tr("settings.shellMissing", language)
                        .replace("{name}", settings.text("localShell")),
                ));
            }
            options.push(("custom".into(), i18n::tr("settings.shellCustom", language)));
            shell.choices = ModelRc::new(VecModel::from(
                options
                    .into_iter()
                    .map(|(value, label)| Choice {
                        value: value.into(),
                        label: label.into(),
                    })
                    .collect::<Vec<_>>(),
            ));
            rows.push(shell);
            if settings.text("localShell") == "custom" {
                rows.push(input(
                    "localShellCommand",
                    "settings.shellCommand",
                    "settings.shellCommandHint",
                ));
                rows.push(action(
                    "browse-shell",
                    "settings.shellPick",
                    "",
                    "editor.browse",
                ));
            }
            if cfg!(windows) {
                rows.push(toggle(
                    "explorerMenu",
                    "settings.explorerMenu",
                    "settings.explorerMenuHint",
                ));
            }
        }
        "files" => {
            let mut editor = select(
                "editor",
                "settings.editor",
                "settings.editorHint",
                &[
                    ("", "Автоматически"),
                    ("system", "settings.editorSystem"),
                    ("custom", "settings.editorCustom"),
                ],
            );
            let mut options = vec![(
                "".to_string(),
                if language == "ru" {
                    "Автоматически"
                } else {
                    "Automatic"
                }
                .to_string(),
            )];
            options.extend_from_slice(context.editors);
            options.extend([
                ("system".into(), i18n::tr("settings.editorSystem", language)),
                ("custom".into(), i18n::tr("settings.editorCustom", language)),
            ]);
            editor.choices = ModelRc::new(VecModel::from(
                options
                    .into_iter()
                    .map(|(v, l)| Choice {
                        value: v.into(),
                        label: l.into(),
                    })
                    .collect::<Vec<_>>(),
            ));
            rows.push(editor);
            if settings.text("editor") == "custom" {
                rows.push(input(
                    "editorCommand",
                    "settings.editorCommand",
                    "settings.editorCommandHint",
                ));
                rows.push(action(
                    "browse-editor",
                    "settings.editorPick",
                    "",
                    "editor.browse",
                ));
            }
            rows.push(select(
                "filesDoubleClick",
                "settings.doubleClick",
                "",
                &[
                    ("download", "settings.doubleClickDownload"),
                    ("edit", "settings.doubleClickEdit"),
                ],
            ));
            rows.push(toggle("filesShowHidden", "files.showHidden", ""));
            let mut sort = select(
                "filesSort",
                "files.name",
                "",
                &[
                    ("name", "files.name"),
                    ("size", "files.size"),
                    ("modified", "files.modified"),
                ],
            );
            sort.label = if language == "ru" {
                "Сортировка"
            } else {
                "Sort by"
            }
            .into();
            rows.push(sort);
            let mut descending = toggle("filesSortDesc", "files.name", "");
            descending.label = if language == "ru" {
                "По убыванию"
            } else {
                "Descending"
            }
            .into();
            rows.push(descending);
        }
        "keyboard" => {
            rows.push(toggle(
                "ctrlKInTerminal",
                "settings.ctrlK",
                "settings.ctrlKHint",
            ));
            for (label, keys) in [
                ("shortcut.palette", "Ctrl+K · Ctrl+Shift+K"),
                ("shortcut.newSession", "Ctrl+Shift+T"),
                ("shortcut.localTerminal", "Ctrl+Shift+`"),
                ("shortcut.closeTab", "Ctrl+Shift+W"),
                ("shortcut.nextPrevTab", "Ctrl+Tab · Ctrl+Shift+Tab"),
                ("shortcut.goToTab", "Ctrl+1…9"),
                ("shortcut.reconnect", "Ctrl+Shift+R"),
                ("shortcut.find", "Ctrl+Shift+F"),
                ("shortcut.files", "Ctrl+Shift+E"),
                ("shortcut.sidebar", "Ctrl+Shift+B"),
                ("shortcut.fullscreen", "F11"),
                ("shortcut.settings", "Ctrl+,"),
                ("shortcut.copyPaste", "Ctrl+Shift+C · Ctrl+Shift+V"),
                ("shortcut.fontSize", "Ctrl+= · Ctrl+− · Ctrl+0"),
            ] {
                rows.push(info(label, label, keys));
            }
        }
        "agents" => {
            rows.push(toggle(
                "agentsEnabled",
                "agents.enable",
                "agents.enableHint",
            ));
            rows.push(info(
                "agents-status",
                "agents.connect",
                context.agents_status,
            ));
            if settings.boolean("agentsEnabled") {
                rows.push(row("agentsPort", "agents.port", "agents.portHint", 4, &[]));
                for (key, label) in [
                    ("agents-copy-claude", "Claude Code"),
                    ("agents-copy-codex", "Codex"),
                    ("agents-copy-json", "JSON"),
                    ("agents-copy-http", "HTTP"),
                ] {
                    let mut r = action(key, "agents.connect", "", "agents.copy");
                    r.label = label.into();
                    rows.push(r);
                }
                rows.push(action(
                    "agents-token",
                    "agents.showToken",
                    "",
                    "agents.copy",
                ));
                rows.push(action(
                    "agents-renew",
                    "agents.newToken",
                    "",
                    "agents.newToken",
                ));
                rows.push(action(
                    "agents-clear",
                    "agents.activity",
                    "",
                    "agents.clear",
                ));
                for server in context.servers {
                    let mut r = select(
                        &format!("agent-server:{}", server.id),
                        "editor.agents",
                        "",
                        &[
                            ("off", "editor.agentsOff"),
                            ("ask", "editor.agentsAsk"),
                            ("allow", "editor.agentsAllow"),
                        ],
                    );
                    r.label = server.name.as_str().into();
                    r.value = match server.agents {
                        nexssh_core::AgentAccess::Off => "off",
                        nexssh_core::AgentAccess::Ask => "ask",
                        nexssh_core::AgentAccess::Allow => "allow",
                    }
                    .into();
                    rows.push(r);
                }
            }
        }
        "backup" => {
            rows.push(action(
                "backup-export",
                "backup.save",
                "backup.saveHint",
                "backup.saveButton",
            ));
            rows.push(action(
                "backup-import",
                "backup.restore",
                "backup.restoreHint",
                "backup.restoreButton",
            ));
        }
        _ => {
            rows.push(info(
                "version",
                "settings.about",
                &format!("NexSSH {} · Rust + Slint", env!("CARGO_PKG_VERSION")),
            ));
            rows.push(info(
                "data-path",
                "settings.dataFolder",
                &context.data_dir.display().to_string(),
            ));
            rows.push(action(
                "copy-data-path",
                "settings.copyPath",
                "",
                "terminal.copy",
            ));
            rows.push(action(
                "open-data-path",
                "settings.dataFolder",
                "settings.dataFolderHint",
                "logs.show",
            ));
            rows.push(action("logs-show", "logs.title", "logs.hint", "logs.show"));
            rows.push(action("logs-export", "logs.title", "", "logs.save"));
            rows.push(info("update-status", "update.title", context.update_status));
            rows.push(action("updates-check", "update.title", "", "update.check"));
            rows.push(toggle("autoUpdateCheck", "update.auto", ""));
            rows.push(info(
                "keychain-status",
                "settings.keychain",
                context.keychain_status,
            ));
            rows.push(action("project-home", "settings.about", "", "edit.open"));
        }
    }
    for row in &mut rows {
        if [
            "language",
            "closeAction",
            "restoreTabs",
            "cursorStyle",
            "rightClickPaste",
            "pasteWarning",
            "filesDoubleClick",
        ]
        .contains(&row.key.as_str())
        {
            row.kind = 8;
        }
        if row.key == "fontSize" {
            row.kind = 9;
        }
    }
    ModelRc::new(VecModel::from(rows))
}

#[cfg(test)]
mod tests {
    use super::*;
    use slint::Model;
    #[test]
    fn every_settings_section_uses_existing_translations_and_choices() {
        let dir = tempfile::tempdir().unwrap();
        let mut settings = Settings::memory(dir.path());
        settings.set("agentsEnabled", "true").unwrap();
        settings.set("localShell", "custom").unwrap();
        settings.set("editor", "custom").unwrap();
        settings.set("fontFamily", "Consolas").unwrap();
        let fonts = vec!["Consolas".into(), "Courier New".into()];
        let context = Context {
            fonts: Some(&fonts),
            shells: &[],
            editors: &[],
            servers: &[],
            data_dir: dir.path(),
            agents_status: "",
            update_status: "",
            keychain_status: "",
        };
        for language in ["ru", "en"] {
            settings.set("language", language).unwrap();
            let general = rows(&settings, "general", &context);
            let font = general.iter().find(|row| row.key == "fontFamily").unwrap();
            assert_eq!(font.kind, 2);
            assert!(font.choices.iter().any(|choice| choice.value.is_empty()));
            assert!(font.choices.iter().any(|choice| choice.value == "Consolas"));
            for section in SECTIONS {
                for row in rows(&settings, section, &context).iter() {
                    for text in [&row.label, &row.hint] {
                        assert!(
                            ![
                                "settings.",
                                "files.",
                                "agents.",
                                "backup.",
                                "update.",
                                "logs.",
                                "shortcut."
                            ]
                            .iter()
                            .any(|prefix| text.starts_with(prefix)),
                            "{section}: {text}"
                        );
                    }
                    if [2, 8].contains(&row.kind) && !row.value.is_empty() {
                        assert!(
                            row.choices.iter().any(|choice| choice.value == row.value),
                            "{section}: {} = {}",
                            row.key,
                            row.value
                        );
                    }
                }
            }
        }
    }
}
