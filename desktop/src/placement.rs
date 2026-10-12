//! Where the window was, how big, and whether it was maximized: kept in `window.json` next to
//! `settings.json`, so that the window comes back as it was left. (The page writes
//! `settings.json` whole, so this has its own file.) It follows the window as it moves and
//! resizes, noted and written once the window keeps still, and as NexSSH quits. Minimized,
//! hidden in the tray or in full screen, the window keeps the place it would come back to.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, MutexGuard};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tauri::{Manager, PhysicalPosition, PhysicalSize, WebviewWindow, Window};

use crate::commands::AppState;

/// How long the window keeps still before where it is gets noted.
const SETTLE: Duration = Duration::from_millis(500);
/// How far down from the window's top the point is that has to be on a screen for the window
/// to come back where it was: the title bar is there, which moves the window.
const TITLE_BAR: i32 = 16;

/// In physical pixels: the frame's top left corner, and the size of what is inside it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Placement {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
    pub maximized: bool,
}

/// A screen's work area (without the taskbar), in physical pixels.
#[derive(Debug, Clone, Copy)]
struct Screen {
    x: i32,
    y: i32,
    width: u32,
    height: u32,
}

impl Placement {
    /// Whether the middle of the title bar is on one of `screens`: if not (a screen was
    /// unplugged, or set up otherwise), the window is put in the middle instead.
    fn reachable(&self, screens: &[Screen]) -> bool {
        let x = i64::from(self.x) + i64::from(self.width / 2);
        let y = i64::from(self.y) + i64::from(TITLE_BAR);
        screens.iter().any(|s| {
            let (left, top) = (i64::from(s.x), i64::from(s.y));
            x >= left && x < left + i64::from(s.width) && y >= top && y < top + i64::from(s.height)
        })
    }
}

pub struct Placements {
    path: PathBuf,
    /// As last seen, or as read at start-up.
    latest: Mutex<Option<Placement>>,
    /// Counts the changes: a write waits until the window has kept still.
    changes: AtomicU64,
}

impl Placements {
    pub fn load(data_dir: &Path) -> Placements {
        let path = data_dir.join("window.json");
        let latest = std::fs::read(&path)
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok());
        Placements {
            path,
            latest: Mutex::new(latest),
            changes: AtomicU64::new(0),
        }
    }

    /// Puts the window, not shown yet, where it was left.
    pub fn restore(&self, window: &WebviewWindow) {
        let Some(saved) = *lock(&self.latest) else {
            return;
        };
        let screens: Vec<Screen> = window
            .available_monitors()
            .unwrap_or_default()
            .iter()
            .map(|m| {
                let area = m.work_area();
                Screen {
                    x: area.position.x,
                    y: area.position.y,
                    width: area.size.width,
                    height: area.size.height,
                }
            })
            .collect();
        if saved.reachable(&screens) {
            // The position first: on a screen of another scale, the size changes as the window
            // moves there.
            let _ = window.set_position(PhysicalPosition::new(saved.x, saved.y));
            // A window without the system's title bar (Windows) may first get the height of one
            // it does not have: what comes out is set right, a few times at most.
            let wanted = PhysicalSize::new(saved.width, saved.height);
            let mut ask = wanted;
            for _ in 0..4 {
                let _ = window.set_size(ask);
                match window.inner_size() {
                    Ok(got) if got != wanted => {
                        ask = PhysicalSize::new(
                            corrected(ask.width, wanted.width, got.width),
                            corrected(ask.height, wanted.height, got.height),
                        );
                    }
                    _ => break,
                }
            }
        }
        if saved.maximized {
            let _ = window.maximize();
        }
    }

    /// The window moved or resized (or was just shown): what it is gets noted once it keeps
    /// still. Not before: while the window maximizes, its moves come before it says it is
    /// maximized, and would be taken for where it goes back to.
    pub fn changed(&self, window: &Window) {
        let change = self.changes.fetch_add(1, Ordering::SeqCst) + 1;
        let window = window.clone();
        tauri::async_runtime::spawn(async move {
            tokio::time::sleep(SETTLE).await;
            if let Some(placements) = window.try_state::<Placements>()
                && placements.changes.load(Ordering::SeqCst) == change
            {
                placements.note(&window);
            }
        });
    }

    /// Notes where the window is now, and writes it if that changed.
    pub fn note(&self, window: &Window) {
        if !window.is_visible().unwrap_or(false) || window.is_minimized().unwrap_or(true) {
            return;
        }
        let mut latest = lock(&self.latest);
        let Some(mut next) = latest.or_else(|| bounds(window)) else {
            return;
        };
        if window.is_fullscreen().unwrap_or(false) {
            // Full screen ends as the window was before it (see `commands::window_set_fullscreen`).
            next.maximized = window
                .try_state::<AppState>()
                .is_some_and(|state| state.restore_maximized.load(Ordering::Relaxed));
        } else if window.is_maximized().unwrap_or(false) {
            // Kept as it was before: where the window goes back to when it is restored.
            next.maximized = true;
        } else if let Some(bounds) = bounds(window) {
            next = bounds;
        }
        if *latest == Some(next) {
            return;
        }
        *latest = Some(next);
        drop(latest);
        self.write();
    }

    /// Writes where the window was last seen; also as NexSSH quits.
    pub fn write(&self) {
        let latest = lock(&self.latest);
        let Some(placement) = *latest else {
            return;
        };
        let written = serde_json::to_vec_pretty(&placement)
            .map_err(std::io::Error::other)
            .and_then(|json| {
                let tmp = self.path.with_extension("json.tmp");
                std::fs::write(&tmp, json)?;
                std::fs::rename(&tmp, &self.path)
            });
        if let Err(e) = written {
            log::warn!("cannot keep where the window is: {e}");
        }
    }
}

/// What to ask for next to get `wanted`, where asking for `asked` gave `got`.
fn corrected(asked: u32, wanted: u32, got: u32) -> u32 {
    (i64::from(asked) + i64::from(wanted) - i64::from(got)).clamp(1, i64::from(u32::MAX)) as u32
}

/// Where the window is and its size, as it is not maximized.
fn bounds(window: &Window) -> Option<Placement> {
    let position = window.outer_position().ok()?;
    let size = window.inner_size().ok()?;
    Some(Placement {
        x: position.x,
        y: position.y,
        width: size.width,
        height: size.height,
        maximized: false,
    })
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|e| e.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn placement(x: i32, y: i32) -> Placement {
        Placement {
            x,
            y,
            width: 1000,
            height: 700,
            maximized: false,
        }
    }

    #[test]
    fn a_window_comes_back_only_where_its_title_bar_can_be_reached() {
        let screens = [
            Screen {
                x: 0,
                y: 0,
                width: 1920,
                height: 1040,
            },
            // A second screen to the left, higher up.
            Screen {
                x: -1280,
                y: -200,
                width: 1280,
                height: 984,
            },
        ];
        assert!(placement(100, 100).reachable(&screens));
        assert!(
            placement(-1200, -150).reachable(&screens),
            "on the second screen"
        );
        // Partly off a screen, but with the middle of its title bar on one.
        assert!(placement(1400, 900).reachable(&screens));
        // The second screen is gone, or the title bar would be above the screens.
        assert!(!placement(-1200, -150).reachable(&screens[..1]));
        assert!(!placement(100, -40).reachable(&screens));
        assert!(!placement(3000, 100).reachable(&screens));
        assert!(!placement(100, 100).reachable(&[]));
    }

    #[test]
    fn a_size_that_comes_out_off_is_asked_for_off_the_other_way() {
        assert_eq!(
            corrected(641, 641, 671),
            611,
            "30 px too high: 30 px less asked for"
        );
        // The next time it came out as asked: back to what is wanted.
        assert_eq!(corrected(611, 641, 611), 641);
        assert_eq!(corrected(500, 500, 480), 520);
        assert_eq!(corrected(10, 10, 100), 1, "never zero");
    }

    #[test]
    fn placements_are_read_back_as_written() {
        let dir = std::env::temp_dir().join(format!("nexssh-placement-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let placements = Placements::load(&dir);
        assert_eq!(*lock(&placements.latest), None, "nothing kept yet");
        let kept = Placement {
            maximized: true,
            ..placement(-1200, -150)
        };
        *lock(&placements.latest) = Some(kept);
        placements.write();
        assert_eq!(*lock(&Placements::load(&dir).latest), Some(kept));
        std::fs::write(dir.join("window.json"), b"not json").unwrap();
        assert_eq!(
            *lock(&Placements::load(&dir).latest),
            None,
            "a broken file is ignored"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
