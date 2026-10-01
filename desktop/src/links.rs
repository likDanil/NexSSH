//! Links in the terminal (Ctrl+click in the page): only web links, opened by the system's
//! default browser.
//!
//! Whatever a server prints can be a link, and a hyperlink (OSC 8) can show one text while
//! it points elsewhere, so the page is not trusted with what it asks to open: anything but an
//! `http` or `https` URL with a host is refused (no `file:`, no protocol handlers of other
//! programs), and the browser gets the URL as the parser writes it back, percent-encoded.

use std::io;

use nexssh_core::i18n;
use tauri::Url;

use crate::commands::{CmdError, CmdResult, blocking};

/// Longer links are refused (browsers stop somewhere around there too).
const MAX_LEN: usize = 8192;

/// Opens a web link from the terminal in the default browser.
#[tauri::command]
pub async fn open_link(url: String) -> CmdResult<()> {
    let link = web_link(&url).ok_or_else(|| CmdError::from(i18n::link_not_web(&url)))?;
    blocking(move || open(&link))
        .await?
        .map_err(|e| CmdError::from(i18n::open_link_failed(e)))
}

/// `text` as an `http` or `https` URL with a host, normalized; `None` for anything else.
fn web_link(text: &str) -> Option<String> {
    if text.len() > MAX_LEN {
        return None;
    }
    let url = Url::parse(text.trim()).ok()?;
    if !matches!(url.scheme(), "http" | "https") || url.host_str().is_none_or(str::is_empty) {
        return None;
    }
    Some(url.into())
}

#[cfg(windows)]
fn open(url: &str) -> io::Result<()> {
    use windows::Win32::System::Com::{
        COINIT_APARTMENTTHREADED, COINIT_DISABLE_OLE1DDE, CoInitializeEx, CoUninitialize,
    };
    use windows::Win32::UI::Shell::{
        SEE_MASK_FLAG_NO_UI, SEE_MASK_NOASYNC, SHELLEXECUTEINFOW, ShellExecuteExW,
    };
    use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
    use windows::core::{HSTRING, PCWSTR, w};

    let file = HSTRING::from(url);
    let mut info = SHELLEXECUTEINFOW {
        cbSize: size_of::<SHELLEXECUTEINFOW>() as u32,
        // Errors come back to the page instead of a dialog.
        fMask: SEE_MASK_NOASYNC | SEE_MASK_FLAG_NO_UI,
        lpVerb: w!("open"),
        lpFile: PCWSTR(file.as_ptr()),
        nShow: SW_SHOWNORMAL.0,
        ..Default::default()
    };
    // SAFETY: COM is set up for this thread (ShellExecute may hand the link to a shell
    // extension) and torn down after; the strings outlive the call.
    unsafe {
        let com = CoInitializeEx(None, COINIT_APARTMENTTHREADED | COINIT_DISABLE_OLE1DDE);
        let result = ShellExecuteExW(&mut info);
        if com.is_ok() {
            CoUninitialize();
        }
        result.map_err(|e| io::Error::other(e.message()))
    }
}

#[cfg(target_os = "macos")]
fn open(url: &str) -> io::Result<()> {
    spawn(std::process::Command::new("open").arg(url))
}

#[cfg(not(any(windows, target_os = "macos")))]
fn open(url: &str) -> io::Result<()> {
    spawn(std::process::Command::new("xdg-open").arg(url))
}

/// Starts the opener and reaps it in the background: it may stay until the browser it
/// started closes.
#[cfg(not(windows))]
fn spawn(command: &mut std::process::Command) -> io::Result<()> {
    let mut child = command.spawn()?;
    std::thread::spawn(move || child.wait());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn web_links_open_as_the_parser_writes_them() {
        assert_eq!(
            web_link("https://example.com/path?q=1#top").as_deref(),
            Some("https://example.com/path?q=1#top")
        );
        assert_eq!(
            web_link(" HTTP://Example.COM:8080 ").as_deref(),
            Some("http://example.com:8080/")
        );
        assert_eq!(
            web_link("https://example.com/a b\"c").as_deref(),
            Some("https://example.com/a%20b%22c")
        );
        assert_eq!(
            web_link("https://пример.рф/").as_deref(),
            Some("https://xn--e1afmkfd.xn--p1ai/")
        );
    }

    #[test]
    fn nothing_but_web_links_opens() {
        for text in [
            "file:///etc/passwd",
            "file://server/share/x.exe",
            "javascript:alert(1)",
            "ms-msdt:/id PCWDiagnostic",
            "search-ms:query=x",
            "mailto:someone@example.com",
            "ftp://example.com/",
            "https://",
            "example.com",
            "",
        ] {
            assert_eq!(web_link(text), None, "{text}");
        }
        let long = format!("https://example.com/{}", "a".repeat(MAX_LEN));
        assert_eq!(web_link(&long), None);
    }
}
