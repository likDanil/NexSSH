//! Local terminals: the shells installed on this computer and the commands that start them.
//!
//! [`shells`] lists what can be started: PowerShell, Command Prompt, WSL distributions and
//! Git Bash on Windows; the login shell and the others in `/etc/shells` elsewhere. A
//! [`LocalCommand`] is what a local session runs ([`SessionManager::open_local`]).
//!
//! [`SessionManager::open_local`]: crate::SessionManager::open_local

use std::path::{Path, PathBuf};

use serde::{Serialize, Serializer};

use crate::error::{Error, Result};
use crate::i18n;

/// What a profile starts. The UI names profiles by kind (Command Prompt is "Командная
/// строка" in Russian), and WSL is taken to a folder its own way.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum ShellKind {
    /// PowerShell 7 or later (`pwsh.exe`).
    Pwsh,
    /// Windows PowerShell 5.1, part of Windows.
    WindowsPowerShell,
    /// Command Prompt (`cmd.exe`).
    Cmd,
    /// A WSL distribution; the profile's name is the distribution.
    Wsl,
    /// Git Bash from Git for Windows.
    GitBash,
    /// A Unix shell; the profile's name is its file name (`zsh`).
    Unix,
}

/// A shell that can run in a local terminal.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ShellProfile {
    /// What the settings keep: `pwsh`, `powershell`, `cmd`, `git-bash`,
    /// `wsl:<distribution>`, or the path of a Unix shell.
    pub id: String,
    pub kind: ShellKind,
    pub name: String,
    #[serde(serialize_with = "lossy")]
    pub program: PathBuf,
    pub args: Vec<String>,
    /// Started when the settings choose no other shell.
    pub default: bool,
}

fn lossy<S: Serializer>(path: &Path, s: S) -> std::result::Result<S::Ok, S::Error> {
    s.serialize_str(&path.to_string_lossy())
}

impl ShellProfile {
    fn new(id: &str, kind: ShellKind, name: &str, program: PathBuf, args: &[&str]) -> Self {
        ShellProfile {
            id: id.into(),
            kind,
            name: name.into(),
            program,
            args: args.iter().map(|a| a.to_string()).collect(),
            default: false,
        }
    }

    /// The command that starts this shell in `cwd` (the home folder when `None`).
    pub fn command(&self, cwd: Option<&Path>) -> LocalCommand {
        let mut command = LocalCommand::new(self.program.clone(), self.args.clone());
        match self.kind {
            // WSL starts in the Linux home folder unless told otherwise, and translates a
            // Windows folder itself (C:\Users → /mnt/c/Users).
            ShellKind::Wsl => {
                command.args.push("--cd".into());
                command
                    .args
                    .push(cwd.map_or_else(|| "~".into(), |d| d.display().to_string()));
                command.env.push(("WSLENV".into(), wslenv()));
            }
            ShellKind::GitBash => {
                // Keeps the login profile from changing to the home folder.
                command.env.push(("CHERE_INVOKING".into(), "1".into()));
                command.cwd = cwd.map(Path::to_path_buf);
            }
            _ => command.cwd = cwd.map(Path::to_path_buf),
        }
        command
    }
}

/// A program started in a local terminal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalCommand {
    pub program: PathBuf,
    pub args: Vec<String>,
    /// Added to the environment NexSSH was started with.
    pub env: Vec<(String, String)>,
    /// Where it starts; the home folder when `None` or not a folder.
    pub cwd: Option<PathBuf>,
}

impl LocalCommand {
    fn new(program: PathBuf, args: Vec<String>) -> Self {
        LocalCommand {
            program,
            args,
            env: terminal_env(),
            cwd: None,
        }
    }

    /// A command line from the settings: the program and its arguments separated by
    /// spaces, see [`split_command_line`]. `~/` at the start of the program means the home
    /// folder.
    pub fn parse(line: &str, cwd: Option<&Path>) -> Result<LocalCommand> {
        let mut words = split_command_line(line).into_iter();
        let program = words
            .next()
            .filter(|p| !p.is_empty())
            .ok_or_else(|| Error::invalid(i18n::command_empty()))?;
        let mut command = LocalCommand::new(crate::expand_tilde(&program), words.collect());
        command.cwd = cwd.map(Path::to_path_buf);
        Ok(command)
    }
}

/// Splits a command line into words. Spaces separate them; double or single quotes keep
/// spaces inside a word and are removed. Backslashes are ordinary characters, so Windows
/// paths need no escaping: `"C:\Program Files\Git\bin\bash.exe" --login`.
pub fn split_command_line(line: &str) -> Vec<String> {
    let mut words = Vec::new();
    let mut word = String::new();
    let mut quote = None;
    // A word has begun, possibly an empty quoted one.
    let mut started = false;
    for c in line.chars() {
        match quote {
            Some(q) if c == q => quote = None,
            Some(_) => word.push(c),
            None if c == '"' || c == '\'' => {
                quote = Some(c);
                started = true;
            }
            None if c.is_whitespace() => {
                if started {
                    words.push(std::mem::take(&mut word));
                    started = false;
                }
            }
            None => {
                word.push(c);
                started = true;
            }
        }
    }
    if started {
        words.push(word);
    }
    words
}

/// What programs learn about the terminal they run in.
fn terminal_env() -> Vec<(String, String)> {
    let mut env: Vec<(String, String)> = [
        ("TERM", "xterm-256color"),
        ("COLORTERM", "truecolor"),
        ("TERM_PROGRAM", "NexSSH"),
        ("TERM_PROGRAM_VERSION", env!("CARGO_PKG_VERSION")),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_string(), v.to_string()))
    .collect();
    // Apps started from the Finder get no locale, and shells would fall back to ASCII.
    if cfg!(target_os = "macos")
        && ["LC_ALL", "LC_CTYPE", "LANG"]
            .iter()
            .all(|v| std::env::var_os(v).is_none())
    {
        env.push(("LANG".into(), "en_US.UTF-8".into()));
    }
    env
}

/// `WSLENV` passing the terminal's variables into Linux as well.
fn wslenv() -> String {
    const SHARED: &str = "TERM_PROGRAM:TERM_PROGRAM_VERSION:COLORTERM";
    match std::env::var("WSLENV") {
        Ok(v) if !v.is_empty() => format!("{v}:{SHARED}"),
        _ => SHARED.into(),
    }
}

/// The shells found on this computer; exactly one of them is the default (the first).
pub fn shells() -> Vec<ShellProfile> {
    let mut found = detect();
    if let Some(first) = found.first_mut() {
        first.default = true;
    }
    found
}

/// Windows' build number (22631 for Windows 11 23H2); the terminal adapts to ConPTY by it.
pub fn windows_build() -> Option<u32> {
    #[cfg(windows)]
    return windows::build();
    #[cfg(not(windows))]
    None
}

/// Where the system would find `program`: a path as it is, a bare name in `PATH` (with the
/// `PATHEXT` extensions on Windows). `None` when there is no such program.
pub fn find_program(program: &Path) -> Option<PathBuf> {
    if program.is_absolute() || program.components().count() > 1 {
        return candidates(program).into_iter().find(|p| is_program(p));
    }
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .flat_map(|dir| candidates(&dir.join(program)))
        .find(|p| is_program(p))
}

/// `program`, and on Windows also with the extensions of `PATHEXT` when it has none.
fn candidates(program: &Path) -> Vec<PathBuf> {
    let mut out = vec![program.to_path_buf()];
    if cfg!(windows) && program.extension().is_none() {
        let exts = std::env::var("PATHEXT").unwrap_or_else(|_| ".COM;.EXE;.BAT;.CMD".into());
        for ext in exts.split(';').filter_map(|e| e.strip_prefix('.')) {
            out.push(program.with_extension(ext));
        }
    }
    out
}

#[cfg(unix)]
fn is_program(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    path.metadata()
        .is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
}

/// Also true for the Microsoft Store's app execution aliases (`WindowsApps\pwsh.exe`),
/// reparse points that cannot be opened as files but start like programs.
#[cfg(windows)]
fn is_program(path: &Path) -> bool {
    path.is_file() || path.symlink_metadata().is_ok_and(|m| !m.is_dir())
}

#[cfg(windows)]
fn detect() -> Vec<ShellProfile> {
    let system32 = windows::env_dir("SystemRoot")
        .unwrap_or_else(|| PathBuf::from(r"C:\Windows"))
        .join("System32");
    let mut found = Vec::new();
    // PowerShell 7 comes first, so it is the default when installed (like in Windows
    // Terminal); otherwise Windows PowerShell is.
    if let Some(pwsh) = windows::pwsh() {
        found.push(ShellProfile::new(
            "pwsh",
            ShellKind::Pwsh,
            "PowerShell",
            pwsh,
            &["-NoLogo"],
        ));
    }
    let powershell = system32.join(r"WindowsPowerShell\v1.0\powershell.exe");
    if is_program(&powershell) {
        found.push(ShellProfile::new(
            "powershell",
            ShellKind::WindowsPowerShell,
            "Windows PowerShell",
            powershell,
            &["-NoLogo"],
        ));
    }
    let cmd = windows::env_dir("ComSpec")
        .filter(|p| is_program(p))
        .unwrap_or_else(|| system32.join("cmd.exe"));
    if is_program(&cmd) {
        found.push(ShellProfile::new(
            "cmd",
            ShellKind::Cmd,
            "Command Prompt",
            cmd,
            &[],
        ));
    }
    let wsl = system32.join("wsl.exe");
    if is_program(&wsl) {
        for distro in windows::wsl_distributions() {
            found.push(ShellProfile::new(
                &format!("wsl:{distro}"),
                ShellKind::Wsl,
                &distro,
                wsl.clone(),
                &["-d", &distro],
            ));
        }
    }
    if let Some(bash) = windows::git_bash() {
        found.push(ShellProfile::new(
            "git-bash",
            ShellKind::GitBash,
            "Git Bash",
            bash,
            &["--login", "-i"],
        ));
    }
    found
}

/// The login shell first, then the other shells in `/etc/shells`.
#[cfg(unix)]
fn detect() -> Vec<ShellProfile> {
    let login = portable_pty::CommandBuilder::new_default_prog().get_shell();
    let listed = std::fs::read_to_string("/etc/shells").unwrap_or_default();
    let paths = std::iter::once(login.as_str())
        .chain(listed.lines().map(str::trim).filter(|l| l.starts_with('/')));
    // Like Terminal.app, a login shell on macOS: apps started from the Finder get a minimal
    // PATH, and the login profile completes it.
    let args: &[&str] = if cfg!(target_os = "macos") {
        &["-l"]
    } else {
        &[]
    };
    let mut seen = std::collections::HashSet::new();
    let mut found = Vec::new();
    for path in paths.map(Path::new) {
        let Some(name) = path.file_name().map(|n| n.to_string_lossy().into_owned()) else {
            continue;
        };
        // Not for working in: accounts that may not log in, and git's restricted shell.
        if matches!(name.as_str(), "nologin" | "false" | "true" | "git-shell") || !is_program(path)
        {
            continue;
        }
        // /bin/bash and /usr/bin/bash are one file on most systems.
        if !seen.insert(std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())) {
            continue;
        }
        found.push(ShellProfile::new(
            &path.to_string_lossy(),
            ShellKind::Unix,
            &name,
            path.to_path_buf(),
            args,
        ));
    }
    found
}

#[cfg(windows)]
mod windows {
    use std::path::PathBuf;

    use winreg::RegKey;
    use winreg::enums::{HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE};

    use super::is_program;

    /// A folder named by an environment variable.
    pub fn env_dir(var: &str) -> Option<PathBuf> {
        std::env::var_os(var)
            .filter(|v| !v.is_empty())
            .map(PathBuf::from)
    }

    /// PowerShell 7 or later: the newest stable one in Program Files, else the one `PATH`
    /// finds (the Microsoft Store's, a dotnet tool).
    pub fn pwsh() -> Option<PathBuf> {
        let mut best: Option<((bool, Vec<u32>), PathBuf)> = None;
        for base in ["ProgramFiles", "ProgramW6432"]
            .into_iter()
            .filter_map(env_dir)
        {
            let Ok(entries) = std::fs::read_dir(base.join("PowerShell")) else {
                continue;
            };
            for entry in entries.flatten() {
                let exe = entry.path().join("pwsh.exe");
                if !is_program(&exe) {
                    continue;
                }
                // Folders are named by version: `7`, `7-preview`.
                let name = entry.file_name().to_string_lossy().into_owned();
                let version: Vec<u32> = name
                    .split(|c: char| !c.is_ascii_digit())
                    .filter_map(|n| n.parse().ok())
                    .collect();
                let rank = (!name.contains("preview"), version);
                if best.as_ref().is_none_or(|(r, _)| *r < rank) {
                    best = Some((rank, exe));
                }
            }
        }
        best.map(|(_, exe)| exe)
            .or_else(|| super::find_program("pwsh.exe".as_ref()))
    }

    /// The WSL distributions registered for this user, the default one first. Docker's and
    /// Rancher's own distributions are not for working in.
    pub fn wsl_distributions() -> Vec<String> {
        let Ok(lxss) = RegKey::predef(HKEY_CURRENT_USER)
            .open_subkey(r"Software\Microsoft\Windows\CurrentVersion\Lxss")
        else {
            return Vec::new();
        };
        let default: String = lxss.get_value("DefaultDistribution").unwrap_or_default();
        let mut found: Vec<(bool, String)> = lxss
            .enum_keys()
            .flatten()
            .filter_map(|guid| {
                let key = lxss.open_subkey(&guid).ok()?;
                let name: String = key.get_value("DistributionName").ok()?;
                // 1 = installed; other states are installing or uninstalling.
                let state: u32 = key.get_value("State").unwrap_or(1);
                let internal =
                    name.starts_with("docker-desktop") || name.starts_with("rancher-desktop");
                (state == 1 && !internal && !name.is_empty())
                    .then(|| (!guid.eq_ignore_ascii_case(&default), name))
            })
            .collect();
        found.sort();
        found.into_iter().map(|(_, name)| name).collect()
    }

    /// Git Bash: where the Git for Windows installer says it is, else the usual folders.
    pub fn git_bash() -> Option<PathBuf> {
        let registered = [HKEY_LOCAL_MACHINE, HKEY_CURRENT_USER]
            .into_iter()
            .filter_map(|root| {
                let key = RegKey::predef(root)
                    .open_subkey(r"SOFTWARE\GitForWindows")
                    .ok()?;
                key.get_value::<String, _>("InstallPath")
                    .ok()
                    .map(PathBuf::from)
            });
        let usual = ["ProgramFiles", "ProgramW6432", "ProgramFiles(x86)"]
            .into_iter()
            .filter_map(env_dir)
            .map(|d| d.join("Git"))
            .chain(env_dir("LOCALAPPDATA").map(|d| d.join(r"Programs\Git")))
            .chain(env_dir("USERPROFILE").map(|d| d.join(r"scoop\apps\git\current")));
        registered
            .chain(usual)
            .map(|dir| dir.join(r"bin\bash.exe"))
            .find(|p| is_program(p))
    }

    pub fn build() -> Option<u32> {
        let key = RegKey::predef(HKEY_LOCAL_MACHINE)
            .open_subkey(r"SOFTWARE\Microsoft\Windows NT\CurrentVersion")
            .ok()?;
        key.get_value::<String, _>("CurrentBuildNumber")
            .ok()?
            .trim()
            .parse()
            .ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_command_lines_like_a_settings_field_expects() {
        assert_eq!(split_command_line("  pwsh   -NoLogo "), ["pwsh", "-NoLogo"]);
        assert_eq!(
            split_command_line(r#""C:\Program Files\Git\bin\bash.exe" --login -i"#),
            [r"C:\Program Files\Git\bin\bash.exe", "--login", "-i"]
        );
        assert_eq!(
            split_command_line(r#"bash -c 'echo "a b"'"#),
            ["bash", "-c", r#"echo "a b""#]
        );
        assert_eq!(
            split_command_line(r"C:\msys64\usr\bin\zsh.exe"),
            [r"C:\msys64\usr\bin\zsh.exe"]
        );
        assert_eq!(split_command_line(r#"prog "" x"#), ["prog", "", "x"]);
        assert_eq!(split_command_line(r#"a"b c"d"#), ["ab cd"]);
        assert!(split_command_line("   ").is_empty());
    }

    #[test]
    fn parses_a_custom_command() {
        let cwd = Path::new("/tmp");
        let c = LocalCommand::parse(" fish  -l ", Some(cwd)).unwrap();
        assert_eq!(c.program, PathBuf::from("fish"));
        assert_eq!(c.args, ["-l"]);
        assert_eq!(c.cwd.as_deref(), Some(cwd));
        assert!(
            c.env
                .iter()
                .any(|(k, v)| k == "TERM" && v == "xterm-256color")
        );
        assert!(
            c.env
                .iter()
                .any(|(k, v)| k == "TERM_PROGRAM" && v == "NexSSH")
        );

        if let Some(home) = crate::home_dir() {
            let c = LocalCommand::parse("~/bin/sh", None).unwrap();
            assert_eq!(c.program, home.join("bin/sh"));
        }
        assert!(LocalCommand::parse("  ", None).is_err());
        assert!(LocalCommand::parse(r#""""#, None).is_err());
    }

    #[test]
    fn a_profile_starts_in_the_folder_it_is_given() {
        let dir = Path::new("/work/project");
        let unix = ShellProfile::new("/bin/zsh", ShellKind::Unix, "zsh", "/bin/zsh".into(), &[]);
        assert_eq!(unix.command(Some(dir)).cwd.as_deref(), Some(dir));
        assert_eq!(unix.command(None).cwd, None);

        // WSL is told the folder (it translates Windows paths) or its home, not given a cwd.
        let wsl = ShellProfile::new(
            "wsl:Ubuntu",
            ShellKind::Wsl,
            "Ubuntu",
            r"C:\Windows\System32\wsl.exe".into(),
            &["-d", "Ubuntu"],
        );
        let c = wsl.command(Some(Path::new(r"C:\Users\me\src")));
        assert_eq!(c.args, ["-d", "Ubuntu", "--cd", r"C:\Users\me\src"]);
        assert_eq!(c.cwd, None);
        assert!(
            c.env.iter().any(|(k, v)| k == "WSLENV"
                && v.ends_with("TERM_PROGRAM:TERM_PROGRAM_VERSION:COLORTERM"))
        );
        assert_eq!(wsl.command(None).args, ["-d", "Ubuntu", "--cd", "~"]);

        let git = ShellProfile::new(
            "git-bash",
            ShellKind::GitBash,
            "Git Bash",
            r"C:\Program Files\Git\bin\bash.exe".into(),
            &["--login", "-i"],
        );
        let c = git.command(Some(dir));
        assert_eq!(c.cwd.as_deref(), Some(dir));
        assert!(c.env.iter().any(|(k, v)| k == "CHERE_INVOKING" && v == "1"));
    }

    #[cfg(windows)]
    #[test]
    fn finds_the_shells_of_windows() {
        let shells = shells();
        let ids: Vec<&str> = shells.iter().map(|s| s.id.as_str()).collect();
        assert!(
            ids.contains(&"cmd") && ids.contains(&"powershell"),
            "{shells:?}"
        );
        assert!(
            matches!(
                shells[0].kind,
                ShellKind::Pwsh | ShellKind::WindowsPowerShell
            ),
            "{shells:?}"
        );
        assert_eq!(shells.iter().filter(|s| s.default).count(), 1);
        assert!(shells.iter().all(|s| is_program(&s.program)), "{shells:?}");
        assert!(windows_build().is_some_and(|b| b >= 17763));
        // A bare name is found in PATH, with the extensions of PATHEXT.
        let cmd = find_program(Path::new("cmd")).expect("cmd in PATH");
        assert!(cmd.is_absolute(), "{cmd:?}");
        assert!(find_program(Path::new(r"C:\nonexistent\nexssh.exe")).is_none());
    }

    #[cfg(unix)]
    #[test]
    fn finds_shells_and_programs() {
        let shells = shells();
        assert!(!shells.is_empty(), "no shell found");
        assert!(shells[0].default);
        assert_eq!(shells.iter().filter(|s| s.default).count(), 1);
        assert!(shells.iter().all(|s| is_program(&s.program)));
        let ids: std::collections::HashSet<_> = shells.iter().map(|s| &s.id).collect();
        assert_eq!(ids.len(), shells.len(), "{shells:?}");

        let sh = find_program(Path::new("sh")).expect("sh in PATH");
        assert!(sh.is_absolute());
        assert_eq!(find_program(&sh), Some(sh.clone()));
        assert_eq!(find_program(Path::new("/nonexistent/nexssh-shell")), None);
        assert_eq!(find_program(Path::new("nexssh-no-such-program")), None);
        // A folder is not a program.
        assert_eq!(find_program(Path::new("/")), None);
    }
}
