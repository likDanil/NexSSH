//! Windows 11's snap layouts on the page's maximize button. Windows offers them when the mouse
//! rests on what a window says is its maximize button (`HTMAXBUTTON` for `WM_NCHITTEST`). The
//! title bar is the page's, and WebView2 covers the window, so Windows never asks about it. A
//! window of our own lies over the page's button and answers for it, as Windows Terminal does
//! for its title bar: Windows shows the layouts there, a click maximizes or restores, and the
//! page hears how to show its button (`snap-button`: `hover`, `press` or `none`). That window
//! never paints, so the page's button shows through it, like Tauri's resize border, which keeps
//! the top rows while the window is not maximized.
//!
//! The page says where its button is (`window_snap_button`), whenever that may have changed.

use serde::Deserialize;

use crate::commands::CmdResult;

/// Where the page's maximize button is, in the window's pixels.
#[derive(Debug, Clone, Copy, Deserialize)]
#[cfg_attr(not(windows), allow(dead_code))]
pub struct ButtonRect {
    x: i32,
    y: i32,
    width: i32,
    height: i32,
}

/// Puts the snap layouts on the page's maximize button at `rect`; `None` takes them away (no
/// such button, as in full screen). Only Windows 11 has them.
#[tauri::command]
pub fn window_snap_button(window: tauri::WebviewWindow, rect: Option<ButtonRect>) -> CmdResult<()> {
    #[cfg(windows)]
    {
        if nexssh_core::local::windows_build().is_none_or(|build| build < 22000) {
            return Ok(());
        }
        use crate::commands::CmdError;
        use tauri::Manager;
        let parent = window.hwnd().map_err(|e| CmdError::from(e.to_string()))?.0 as isize;
        let app = window.app_handle().clone();
        window
            .run_on_main_thread(move || win::place(&app, parent, rect))
            .map_err(|e| CmdError::from(e.to_string()))?;
    }
    #[cfg(not(windows))]
    let _ = (window, rect);
    Ok(())
}

#[cfg(windows)]
mod win {
    use std::sync::OnceLock;
    use std::sync::atomic::{AtomicBool, AtomicIsize, AtomicU8, Ordering};

    use tauri::{AppHandle, Emitter};
    use windows::Win32::Foundation::{HINSTANCE, HWND, LPARAM, LRESULT, WPARAM};
    use windows::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows::Win32::UI::HiDpi::{GetDpiForWindow, GetSystemMetricsForDpi};
    use windows::Win32::UI::Input::KeyboardAndMouse::{
        TME_LEAVE, TME_NONCLIENT, TRACKMOUSEEVENT, TrackMouseEvent,
    };
    use windows::Win32::UI::WindowsAndMessaging::{
        CreateWindowExW, DefWindowProcW, GetParent, HTMAXBUTTON, HWND_TOP, IDC_ARROW, IsZoomed,
        LoadCursorW, RegisterClassExW, SM_CYFRAME, SW_HIDE, SW_MAXIMIZE, SW_RESTORE,
        SWP_NOACTIVATE, SWP_SHOWWINDOW, SetWindowPos, ShowWindow, WINDOW_EX_STYLE, WM_NCHITTEST,
        WM_NCLBUTTONDBLCLK, WM_NCLBUTTONDOWN, WM_NCLBUTTONUP, WM_NCMOUSELEAVE, WM_NCMOUSEMOVE,
        WM_NCRBUTTONDOWN, WM_NCRBUTTONUP, WNDCLASSEXW, WS_CHILD, WS_CLIPSIBLINGS,
    };
    use windows::core::w;

    use super::ButtonRect;

    const NONE: u8 = 0;
    const HOVER: u8 = 1;
    const PRESS: u8 = 2;

    static APP: OnceLock<AppHandle> = OnceLock::new();
    /// The window over the button (0 until it is made).
    static SINK: AtomicIsize = AtomicIsize::new(0);
    /// Whether Windows tells the window when the mouse leaves it.
    static TRACKING: AtomicBool = AtomicBool::new(false);
    /// A press on the button that has not been let go.
    static PRESSED: AtomicBool = AtomicBool::new(false);
    /// How the page was last told to show its button.
    static SHOWN: AtomicU8 = AtomicU8::new(NONE);

    /// Runs on the main thread: makes the window over the button, puts it at `rect`, or hides
    /// it.
    pub fn place(app: &AppHandle, parent: isize, rect: Option<ButtonRect>) {
        let _ = APP.set(app.clone());
        let parent = HWND(parent as _);
        let sink = match SINK.load(Ordering::Relaxed) {
            0 => match create(parent) {
                Some(sink) => sink,
                None => return,
            },
            sink => HWND(sink as _),
        };
        let Some(rect) = rect.filter(|r| r.width > 0 && r.height > 0) else {
            unsafe {
                let _ = ShowWindow(sink, SW_HIDE);
            }
            TRACKING.store(false, Ordering::Relaxed);
            PRESSED.store(false, Ordering::Relaxed);
            show(NONE);
            return;
        };
        unsafe {
            // The rows of the window's resize border stay Tauri's, so that the window is
            // still resized at its top edge above the button (as Windows' own windows are).
            let border = if IsZoomed(parent).as_bool() {
                0
            } else {
                GetSystemMetricsForDpi(SM_CYFRAME, GetDpiForWindow(parent))
            };
            let top = (border - rect.y).clamp(0, rect.height - 1);
            let _ = SetWindowPos(
                sink,
                Some(HWND_TOP),
                rect.x,
                rect.y + top,
                rect.width,
                rect.height - top,
                SWP_NOACTIVATE | SWP_SHOWWINDOW,
            );
        }
    }

    fn create(parent: HWND) -> Option<HWND> {
        let class_name = w!("NexSSHSnapLayouts");
        unsafe {
            let instance = HINSTANCE(GetModuleHandleW(None).ok()?.0);
            let class = WNDCLASSEXW {
                cbSize: size_of::<WNDCLASSEXW>() as u32,
                lpfnWndProc: Some(window_proc),
                hInstance: instance,
                hCursor: LoadCursorW(None, IDC_ARROW).ok()?,
                lpszClassName: class_name,
                ..Default::default()
            };
            RegisterClassExW(&class);
            // No background and nothing painted: the page's button shows through.
            let sink = CreateWindowExW(
                WINDOW_EX_STYLE::default(),
                class_name,
                w!(""),
                WS_CHILD | WS_CLIPSIBLINGS,
                0,
                0,
                0,
                0,
                Some(parent),
                None,
                Some(instance),
                None,
            )
            .ok()?;
            SINK.store(sink.0 as isize, Ordering::Relaxed);
            Some(sink)
        }
    }

    /// Tells the page how to show its button, when that changes.
    fn show(state: u8) {
        if SHOWN.swap(state, Ordering::Relaxed) == state {
            return;
        }
        if let Some(app) = APP.get() {
            let name = match state {
                HOVER => "hover",
                PRESS => "press",
                _ => "none",
            };
            let _ = app.emit("snap-button", name);
        }
    }

    unsafe extern "system" fn window_proc(
        hwnd: HWND,
        msg: u32,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> LRESULT {
        match msg {
            WM_NCHITTEST => return LRESULT(HTMAXBUTTON as isize),
            WM_NCMOUSEMOVE => {
                if !TRACKING.swap(true, Ordering::Relaxed) {
                    let mut leave = TRACKMOUSEEVENT {
                        cbSize: size_of::<TRACKMOUSEEVENT>() as u32,
                        dwFlags: TME_LEAVE | TME_NONCLIENT,
                        hwndTrack: hwnd,
                        dwHoverTime: 0,
                    };
                    let _ = unsafe { TrackMouseEvent(&mut leave) };
                }
                show(if PRESSED.load(Ordering::Relaxed) {
                    PRESS
                } else {
                    HOVER
                });
                // On to Windows, which offers the snap layouts.
            }
            WM_NCMOUSELEAVE => {
                TRACKING.store(false, Ordering::Relaxed);
                PRESSED.store(false, Ordering::Relaxed);
                show(NONE);
            }
            // The button is the page's: Windows would look for one of its own.
            WM_NCLBUTTONDOWN | WM_NCLBUTTONDBLCLK => {
                PRESSED.store(true, Ordering::Relaxed);
                show(PRESS);
                return LRESULT(0);
            }
            WM_NCLBUTTONUP => {
                if PRESSED.swap(false, Ordering::Relaxed)
                    && let Ok(parent) = unsafe { GetParent(hwnd) }
                {
                    let maximized = unsafe { IsZoomed(parent) }.as_bool();
                    let _ = unsafe {
                        ShowWindow(parent, if maximized { SW_RESTORE } else { SW_MAXIMIZE })
                    };
                    // The button moved with the window's edge; it looks hovered again once the
                    // mouse is on it.
                    show(NONE);
                } else {
                    show(HOVER);
                }
                return LRESULT(0);
            }
            WM_NCRBUTTONDOWN | WM_NCRBUTTONUP => return LRESULT(0),
            _ => {}
        }
        unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
    }
}
