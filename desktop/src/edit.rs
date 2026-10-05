//! Remote files edited in a program of this computer (the syncing is
//! `nexssh_core::edit`): the editors found here, starting the one the settings choose, and
//! the IPC commands. The page gets the list of edited files on every change (`edits` event).
//!
//! The editor is the one the settings name (`editor`: a found editor's id, `system` for the
//! file's default program, `custom` for the command line in `editorCommand`), else the first
//! one found. Like a local terminal's custom shell, the command line comes from the
//! settings, not from what the page asks for.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use nexssh_core::edit::{EditId, EditInfo, Edits, Refusal, Resolution};
use nexssh_core::local::{find_program, split_command_line};
use nexssh_core::{SessionId, i18n};
use serde::Serialize;
use serde_json::Value;
use tauri::{AppHandle, Emitter, State};

use crate::commands::{AppState, CmdError, CmdResult, blocking};

/// The files being edited.
pub struct Editing(pub Arc<Edits>);

impl Editing {
    /// Copies go under the temporary folder; the page hears of every change.
    pub fn new(app: &AppHandle, sessions: nexssh_core::SessionManager) -> Editing {
        let app = app.clone();
        let root = std::env::temp_dir().join("NexSSH-edit");
        Editing(Edits::new(
            sessions,
            root,
            Box::new(move |list| {
                let _ = app.emit("edits", list);
            }),
        ))
    }
}

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

/// Opens `file` in the editor the settings choose.
fn launch(settings: &Value, file: &Path) -> std::io::Result<()> {
    let choice = settings.get("editor").and_then(Value::as_str).unwrap_or("");
    let command = settings
        .get("editorCommand")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim();
    match choice {
        "custom" if !command.is_empty() => custom(command, file),
        "system" => crate::links::open(&file.display().to_string()),
        _ => {
            let found = editors();
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

fn settings(state: &AppState) -> Value {
    state
        .settings
        .lock()
        .map(|s| s.value().clone())
        .unwrap_or(Value::Null)
}

/// Starts the editor on an edited file; a failure is reported as one opening it.
fn open_in_editor(state: &AppState, info: &EditInfo) -> Result<(), Refusal> {
    launch(&settings(state), Path::new(&info.local_path)).map_err(|e| Refusal::Failed {
        message: i18n::editor_failed(e),
    })
}

// ---- IPC ------------------------------------------------------------------------------

/// The editors found on this computer, for the settings.
#[tauri::command]
pub async fn edit_editors() -> CmdResult<Vec<EditorInfo>> {
    blocking(editors).await
}

/// Opens a remote file in the editor of the settings; `sudo` reads it with sudo (with
/// `password`, or one typed earlier, or none). A file edited already opens its copy again.
#[tauri::command]
pub async fn edit_open(
    state: State<'_, AppState>,
    editing: State<'_, Editing>,
    session_id: SessionId,
    path: String,
    sudo: Option<bool>,
    password: Option<String>,
) -> Result<EditInfo, Refusal> {
    let info = if sudo == Some(true) {
        editing.0.open_sudo(session_id, &path, password).await?
    } else {
        editing.0.open(session_id, &path).await?
    };
    open_in_editor(&state, &info)?;
    Ok(info)
}

#[tauri::command]
pub fn edit_list(editing: State<'_, Editing>) -> Vec<EditInfo> {
    editing.0.list()
}

/// Settles a file that changed on the server: write ours over it, or take theirs.
#[tauri::command]
pub async fn edit_resolve(
    editing: State<'_, Editing>,
    id: EditId,
    resolution: Resolution,
) -> CmdResult<Option<EditInfo>> {
    Ok(editing.0.resolve(id, resolution).await)
}

/// Writes the file with sudo from now on (`password`: sudo's), and sends it.
#[tauri::command]
pub async fn edit_sudo(
    editing: State<'_, Editing>,
    id: EditId,
    password: Option<String>,
) -> CmdResult<Option<EditInfo>> {
    Ok(editing.0.use_sudo(id, password).await)
}

#[tauri::command]
pub fn edit_stop(editing: State<'_, Editing>, id: EditId) {
    editing.0.stop(id);
}

/// Shows an edited file's local copy in the system file manager.
#[tauri::command]
pub fn edit_reveal(editing: State<'_, Editing>, id: EditId) -> CmdResult<()> {
    let info = editing
        .0
        .get(id)
        .ok_or_else(|| CmdError::from(i18n::edit_not_found()))?;
    let path = PathBuf::from(info.local_path);
    crate::sftp::reveal(&path).map_err(|e| CmdError::from(i18n::reveal_failed(e)))
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

    #[cfg(windows)]
    #[test]
    fn expands_variables_in_places() {
        let found = platform::locate(r"%SystemRoot%\System32\notepad.exe");
        assert!(found.is_some_and(|p| p.ends_with("notepad.exe")));
        assert!(platform::locate(r"%NEXSSH_NO_SUCH_VARIABLE%\x.exe").is_none());
    }
}
