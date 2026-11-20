//! Remote files edited in a program of this computer (the syncing is
//! `nexssh_core::edit`): the editors found here, starting the one the settings choose, and
//! the IPC commands. The page gets the list of edited files on every change (`edits` event).
//!
//! A file opens in the program the settings choose for its extension
//! (`editorAssociations`), else in the default editor (`editor`). A program is a found
//! editor's id, `system` for the file's default program, or `custom` for a command line
//! (`command` of the association, `editorCommand` of the default). The default editor `""`
//! is the first one found. Like a local terminal's custom shell, command lines come from the
//! settings, not from what the page asks for.

use std::path::Path;

use nexssh_core::i18n;
use nexssh_core::local::{find_program, split_command_line};
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// An editor found on this computer.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EditorInfo {
    pub id: String,
    pub name: String,
    pub program: String,
}

/// The editors found here, the preferred first (the one used when the settings name none).
pub fn editors() -> Vec<EditorInfo> {
    platform::KNOWN
        .iter()
        .filter_map(|(id, name, places)| {
            places
                .iter()
                .find_map(|place| platform::locate(place))
                .map(|program| EditorInfo {
                    id: (*id).to_string(),
                    name: (*name).to_string(),
                    program: program.display().to_string(),
                })
        })
        .collect()
}

/// A program for files with some extensions (`editorAssociations` in the settings).
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct Association {
    /// Without the dot: `log`, `tar.gz`.
    extensions: Vec<String>,
    /// A found editor's id, `system` or `custom`.
    editor: String,
    /// For `custom`.
    command: String,
}

/// What opens a file: a found editor's id (`""`: the first one found), `system`, or `custom`
/// with its command line.
#[derive(Debug, PartialEq)]
struct Program {
    editor: String,
    command: String,
}

impl Program {
    /// Whether it can open files here: a custom one needs its command, an editor must still
    /// be found.
    fn usable(&self, found: &[EditorInfo]) -> bool {
        match self.editor.as_str() {
            "custom" => !self.command.trim().is_empty(),
            "system" => true,
            id => found.iter().any(|e| e.id == id),
        }
    }
}

/// The program the settings choose for the file `name`: the one associated with its
/// extension (the longest that matches, so `tar.gz` before `gz`; `.env` has the extension
/// `env`), else the default editor. An association whose program cannot open files here is
/// passed over.
fn program_for(settings: &Value, name: &str, found: &[EditorInfo]) -> Program {
    let name = name.to_lowercase();
    let associations = settings
        .get("editorAssociations")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default();
    let associated = associations
        .iter()
        // One that does not read as an association is left out, not the others with it.
        .filter_map(|item| serde_json::from_value::<Association>(item.clone()).ok())
        .filter_map(|association| {
            let longest = association
                .extensions
                .iter()
                .map(|ext| ext.trim().trim_start_matches('.').to_lowercase())
                .filter(|ext| !ext.is_empty() && name.ends_with(&format!(".{ext}")))
                .map(|ext| ext.len())
                .max()?;
            let program = Program {
                editor: association.editor,
                command: association.command,
            };
            Some((longest, program))
        })
        .filter(|(_, program)| program.usable(found))
        // The first of the longest.
        .fold(
            None::<(usize, Program)>,
            |best, (len, program)| match best {
                Some((best_len, _)) if best_len >= len => best,
                _ => Some((len, program)),
            },
        );
    if let Some((_, program)) = associated {
        return program;
    }
    let text = |key: &str| {
        settings
            .get(key)
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string()
    };
    Program {
        editor: text("editor"),
        command: text("editorCommand"),
    }
}

/// Opens `file` in the program the settings choose for it.
pub fn launch(settings: &Value, file: &Path) -> std::io::Result<()> {
    let found = editors();
    let name = file
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let program = program_for(settings, &name, &found);
    let command = program.command.trim();
    match program.editor.as_str() {
        "custom" => log::info!("{name} opens with `{command}`"),
        "" => log::info!("{name} opens in the first editor found"),
        editor => log::info!("{name} opens in {editor}"),
    }
    match program.editor.as_str() {
        "custom" if !command.is_empty() => custom(command, file),
        "system" => open::that(file).map_err(std::io::Error::other),
        choice => {
            let editor = found
                .iter()
                .find(|e| e.id == choice)
                .or_else(|| found.first());
            match editor {
                Some(editor) => platform::start(Path::new(&editor.program), file),
                None => platform::fallback(file),
            }
        }
    }
}

/// A command line from the settings: `{file}` stands for the file, else it goes last.
fn custom(command: &str, file: &Path) -> std::io::Result<()> {
    let file = file.display().to_string();
    let mut words = split_command_line(command);
    if words.is_empty() {
        return Err(std::io::Error::other(i18n::command_empty()));
    }
    let program = words.remove(0);
    let mut args: Vec<String> = words.iter().map(|w| w.replace("{file}", &file)).collect();
    if !words.iter().any(|w| w.contains("{file}")) {
        args.push(file);
    }
    let program = nexssh_core::expand_tilde(&program);
    let program = find_program(&program).unwrap_or(program);
    spawn(std::process::Command::new(program).args(args))
}

/// Starts a program on its own; it is reaped in the background (an editor may run long).
fn spawn(command: &mut std::process::Command) -> std::io::Result<()> {
    use std::process::Stdio;
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x08000000);
    }
    let mut child = command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;
    std::thread::spawn(move || child.wait());
    Ok(())
}

#[cfg(windows)]
mod platform {
    use std::path::{Path, PathBuf};

    pub const KNOWN: &[(&str, &str, &[&str])] = &[
        (
            "vscode",
            "Visual Studio Code",
            &[
                r"%LOCALAPPDATA%\Programs\Microsoft VS Code\Code.exe",
                r"%ProgramFiles%\Microsoft VS Code\Code.exe",
            ],
        ),
        (
            "cursor",
            "Cursor",
            &[r"%LOCALAPPDATA%\Programs\cursor\Cursor.exe"],
        ),
        (
            "windsurf",
            "Windsurf",
            &[r"%LOCALAPPDATA%\Programs\Windsurf\Windsurf.exe"],
        ),
        ("zed", "Zed", &[r"%LOCALAPPDATA%\Programs\Zed\Zed.exe"]),
        (
            "sublime",
            "Sublime Text",
            &[
                r"%ProgramFiles%\Sublime Text\sublime_text.exe",
                r"%ProgramFiles%\Sublime Text 3\sublime_text.exe",
            ],
        ),
        (
            "notepad++",
            "Notepad++",
            &[
                r"%ProgramFiles%\Notepad++\notepad++.exe",
                r"%ProgramFiles(x86)%\Notepad++\notepad++.exe",
            ],
        ),
        (
            "notepad",
            "Notepad",
            &[r"%SystemRoot%\System32\notepad.exe"],
        ),
    ];

    /// `%VAR%\…` made a path, if that file exists.
    pub fn locate(place: &str) -> Option<PathBuf> {
        let mut path = String::new();
        for (i, part) in place.split('%').enumerate() {
            if i % 2 == 1 {
                path.push_str(&std::env::var(part).ok()?);
            } else {
                path.push_str(part);
            }
        }
        let path = PathBuf::from(path);
        path.is_file().then_some(path)
    }

    pub fn start(program: &Path, file: &Path) -> std::io::Result<()> {
        super::spawn(std::process::Command::new(program).arg(file))
    }

    pub fn fallback(file: &Path) -> std::io::Result<()> {
        super::spawn(std::process::Command::new("notepad.exe").arg(file))
    }
}

#[cfg(target_os = "macos")]
mod platform {
    use std::path::{Path, PathBuf};

    pub const KNOWN: &[(&str, &str, &[&str])] = &[
        ("vscode", "Visual Studio Code", &["Visual Studio Code.app"]),
        ("cursor", "Cursor", &["Cursor.app"]),
        ("windsurf", "Windsurf", &["Windsurf.app"]),
        ("zed", "Zed", &["Zed.app"]),
        ("sublime", "Sublime Text", &["Sublime Text.app"]),
        ("bbedit", "BBEdit", &["BBEdit.app"]),
        (
            "textedit",
            "TextEdit",
            &["/System/Applications/TextEdit.app"],
        ),
    ];

    /// An application in /Applications or ~/Applications (or at an absolute path).
    pub fn locate(place: &str) -> Option<PathBuf> {
        let candidates = if place.starts_with('/') {
            vec![PathBuf::from(place)]
        } else {
            let mut list = vec![Path::new("/Applications").join(place)];
            if let Some(home) = nexssh_core::home_dir() {
                list.push(home.join("Applications").join(place));
            }
            list
        };
        candidates.into_iter().find(|p| p.is_dir())
    }

    pub fn start(program: &Path, file: &Path) -> std::io::Result<()> {
        super::spawn(
            std::process::Command::new("open")
                .arg("-a")
                .arg(program)
                .arg(file),
        )
    }

    /// The default text editor (`open -t`), whatever the file's type.
    pub fn fallback(file: &Path) -> std::io::Result<()> {
        super::spawn(std::process::Command::new("open").arg("-t").arg(file))
    }
}

#[cfg(not(any(windows, target_os = "macos")))]
mod platform {
    use std::path::{Path, PathBuf};

    pub const KNOWN: &[(&str, &str, &[&str])] = &[
        ("vscode", "Visual Studio Code", &["code"]),
        ("cursor", "Cursor", &["cursor"]),
        ("windsurf", "Windsurf", &["windsurf"]),
        ("zed", "Zed", &["zeditor", "zed"]),
        ("sublime", "Sublime Text", &["subl"]),
        ("kate", "Kate", &["kate"]),
        ("gnome-text-editor", "Text Editor", &["gnome-text-editor"]),
        ("gedit", "gedit", &["gedit"]),
        ("mousepad", "Mousepad", &["mousepad"]),
        ("xed", "Xed", &["xed"]),
    ];

    pub fn locate(place: &str) -> Option<PathBuf> {
        nexssh_core::local::find_program(Path::new(place))
    }

    pub fn start(program: &Path, file: &Path) -> std::io::Result<()> {
        super::spawn(std::process::Command::new(program).arg(file))
    }

    pub fn fallback(file: &Path) -> std::io::Result<()> {
        super::spawn(std::process::Command::new("xdg-open").arg(file))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_last_editor_is_always_there() {
        // Notepad on Windows, TextEdit on macOS; on Linux there may be none (xdg-open then).
        let found = editors();
        if cfg!(any(windows, target_os = "macos")) {
            assert!(!found.is_empty());
        }
        for editor in &found {
            assert!(Path::new(&editor.program).exists(), "{editor:?}");
        }
    }

    fn found(ids: &[&str]) -> Vec<EditorInfo> {
        ids.iter()
            .map(|id| EditorInfo {
                id: id.to_string(),
                name: id.to_string(),
                program: String::new(),
            })
            .collect()
    }

    fn program(editor: &str, command: &str) -> Program {
        Program {
            editor: editor.into(),
            command: command.into(),
        }
    }

    #[test]
    fn files_open_in_the_program_of_their_extension() {
        let settings = serde_json::json!({
            "editor": "vscode",
            "editorCommand": "",
            "editorAssociations": [
                { "extensions": ["log", "TXT"], "editor": "notepad++" },
                { "extensions": ["gz"], "editor": "system" },
                { "extensions": ["tar.gz"], "editor": "custom", "command": "7z x {file}" },
                { "extensions": [".env"], "editor": "custom", "command": "vim" },
                { "extensions": ["md"], "editor": "typora" },
                { "extensions": ["ini"], "editor": "custom", "command": "  " },
                "not an association",
                { "extensions": ["log"], "editor": "zed" },
            ],
        });
        let found = found(&["vscode", "notepad++", "zed"]);
        let open = |name: &str| program_for(&settings, name, &found);
        assert_eq!(
            open("syslog.log"),
            program("notepad++", ""),
            "the first that matches"
        );
        assert_eq!(open("README.TXT"), program("notepad++", ""), "in any case");
        assert_eq!(
            open("backup.tar.gz"),
            program("custom", "7z x {file}"),
            "the longest"
        );
        assert_eq!(open("data.gz"), program("system", ""));
        assert_eq!(open(".env"), program("custom", "vim"), "a dotfile");
        assert_eq!(open("app.env"), program("custom", "vim"));
        // What cannot open files here gives way to the default editor.
        assert_eq!(
            open("notes.md"),
            program("vscode", ""),
            "an editor not found"
        );
        assert_eq!(open("php.ini"), program("vscode", ""), "no command");
        // No association.
        assert_eq!(open("main.rs"), program("vscode", ""));
        assert_eq!(open("log"), program("vscode", ""), "no dot");
        assert_eq!(open("catalog"), program("vscode", ""));
    }

    #[test]
    fn without_associations_the_default_editor() {
        let found = found(&["vscode"]);
        let custom = serde_json::json!({ "editor": "custom", "editorCommand": "subl {file}" });
        let open = |settings: &Value| program_for(settings, "a.log", &found);
        assert_eq!(open(&custom), program("custom", "subl {file}"));
        let broken = serde_json::json!({ "editorAssociations": { "log": "zed" } });
        assert_eq!(open(&broken), program("", ""));
        assert_eq!(open(&Value::Null), program("", ""));
    }

    #[cfg(windows)]
    #[test]
    fn expands_variables_in_places() {
        let found = platform::locate(r"%SystemRoot%\System32\notepad.exe");
        assert!(found.is_some_and(|p| p.ends_with("notepad.exe")));
        assert!(platform::locate(r"%NEXSSH_NO_SUCH_VARIABLE%\x.exe").is_none());
    }
}
