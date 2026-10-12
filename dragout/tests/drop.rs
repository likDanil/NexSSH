//! Drops a drag's files on a folder through the shell's own drop target, the one Explorer
//! uses for a folder, as a user's drop would ([`nexssh_dragout::drop_into`]).
#![cfg(windows)]

use std::io::{Cursor, ErrorKind, Read};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};
use std::thread::ThreadId;
use std::time::{Duration, Instant, UNIX_EPOCH};

use nexssh_dragout::{Ended, Item, Outcome, Source};
use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::System::Com::{COINIT_APARTMENTTHREADED, CoInitializeEx};
use windows::Win32::System::Ole::OleInitialize;
use windows::Win32::UI::WindowsAndMessaging::{
    DispatchMessageW, EnumWindows, GetClassNameW, GetWindowTextW, GetWindowThreadProcessId,
    IsWindowVisible, MSG, PM_REMOVE, PeekMessageW, PostMessageW, TranslateMessage, WM_CLOSE,
    WM_KEYDOWN,
};
use windows::core::BOOL;

/// Files in memory.
struct Files {
    items: Vec<Item>,
    contents: Vec<Vec<u8>>,
    ended: Mutex<Sender<Ended>>,
    /// The thread that drops, as a window's thread would.
    dropping: ThreadId,
    /// Whether a file was opened or read on it: waiting for the network there would freeze
    /// the window.
    on_dropping: Arc<AtomicBool>,
}

impl Source for Files {
    fn items(&self) -> Result<Vec<Item>, String> {
        Ok(self.items.clone())
    }

    fn open(&self, index: usize) -> std::io::Result<Box<dyn Read + Send>> {
        if std::thread::current().id() == self.dropping {
            self.on_dropping.store(true, Ordering::SeqCst);
        }
        Ok(Box::new(Trickle {
            data: Cursor::new(self.contents[index].clone()),
            dropping: self.dropping,
            on_dropping: Arc::clone(&self.on_dropping),
        }))
    }

    fn finished(&self, ended: Ended) {
        let _ = self.ended.lock().unwrap().send(ended);
    }
}

/// At most 50 000 bytes a read, as data comes from a network.
struct Trickle {
    data: Cursor<Vec<u8>>,
    dropping: ThreadId,
    on_dropping: Arc<AtomicBool>,
}

impl Read for Trickle {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        if std::thread::current().id() == self.dropping {
            self.on_dropping.store(true, Ordering::SeqCst);
        }
        let len = buffer.len().min(50_000);
        self.data.read(&mut buffer[..len])
    }
}

fn pattern(len: usize) -> Vec<u8> {
    (0..len).map(|i| (i * 7 % 251) as u8).collect()
}

fn folder(path: &str) -> (Item, Vec<u8>) {
    let item = Item {
        path: path.into(),
        folder: true,
        size: 0,
        modified: None,
    };
    (item, Vec::new())
}

fn file(path: &str, content: impl Into<Vec<u8>>, modified: u64) -> (Item, Vec<u8>) {
    let content = content.into();
    let item = Item {
        path: path.into(),
        folder: false,
        size: content.len() as u64,
        modified: Some(modified),
    };
    (item, content)
}

#[test]
fn files_and_folders_dropped_on_a_folder_are_copied_into_it() {
    // SAFETY: initializes OLE for this thread, as a window's thread has it.
    unsafe { OleInitialize(None) }.unwrap();
    let target: PathBuf =
        std::env::temp_dir().join(format!("nexssh-dragout-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&target);
    std::fs::create_dir_all(&target).unwrap();

    let (items, contents): (Vec<Item>, Vec<Vec<u8>>) = [
        folder("proj"),
        file(r"proj\a.txt", "hello", 1_700_000_000),
        folder(r"proj\deep"),
        folder(r"proj\deep\er"),
        file(r"proj\deep\er\b.bin", pattern(3_000_001), 1_600_000_000),
        folder(r"proj\nothing"),
        file(r"proj\empty.txt", "", 1_650_000_000),
        file("Привет, мир.txt", "по-русски", 1_700_000_001),
        file("日本語.txt", "テキスト", 1_700_000_002),
    ]
    .into_iter()
    .unzip();
    let (sender, ended) = channel();
    let source = Arc::new(Files {
        items,
        contents,
        ended: Mutex::new(sender),
        dropping: std::thread::current().id(),
        on_dropping: Arc::default(),
    });

    let outcome = nexssh_dragout::drop_into(&target, source.clone()).unwrap();
    assert_eq!(outcome, Outcome::Dropped);
    assert_eq!(wait(&ended, Duration::from_secs(60)), Ended::Done);
    assert!(
        !source.on_dropping.load(Ordering::SeqCst),
        "files were read on the thread that dropped them"
    );

    for (item, content) in source.items.iter().zip(&source.contents) {
        let path = target.join(&item.path);
        if item.folder {
            assert!(path.is_dir(), "{path:?}");
            continue;
        }
        assert!(std::fs::read(&path).unwrap() == *content, "{path:?}");
        let modified = std::fs::metadata(&path).unwrap().modified().unwrap();
        let expected = UNIX_EPOCH + Duration::from_secs(item.modified.unwrap());
        assert_eq!(modified, expected, "{path:?}");
    }
    let _ = std::fs::remove_dir_all(&target);
}

/// A file that stops being read part of the way, as when the transfer is cancelled in the app.
struct Stopping {
    ended: Mutex<Sender<Ended>>,
}

impl Source for Stopping {
    fn items(&self) -> Result<Vec<Item>, String> {
        let file = |path: &str| Item {
            path: path.into(),
            folder: false,
            size: 1_000_000,
            modified: Some(1_700_000_000),
        };
        Ok(vec![file("whole.bin"), file("stopped.bin")])
    }

    fn open(&self, index: usize) -> std::io::Result<Box<dyn Read + Send>> {
        Ok(if index == 0 {
            Box::new(Cursor::new(vec![1u8; 1_000_000]))
        } else {
            Box::new(Cursor::new(vec![2u8; 300_000]).chain(Cancelled))
        })
    }

    fn finished(&self, ended: Ended) {
        let _ = self.ended.lock().unwrap().send(ended);
    }
}

struct Cancelled;

impl Read for Cancelled {
    fn read(&mut self, _: &mut [u8]) -> std::io::Result<usize> {
        Err(std::io::Error::new(
            ErrorKind::ConnectionAborted,
            "cancelled",
        ))
    }
}

#[test]
fn a_copy_cancelled_in_the_app_stops_without_a_word_from_the_shell() {
    // SAFETY: initializes OLE for this thread, as a window's thread has it.
    unsafe { OleInitialize(None) }.unwrap();
    let target: PathBuf =
        std::env::temp_dir().join(format!("nexssh-dragout-cancel-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&target);
    std::fs::create_dir_all(&target).unwrap();
    let (sender, ended) = channel();
    let source = Arc::new(Stopping {
        ended: Mutex::new(sender),
    });

    // The shell shows a dialog for a file it cannot copy; a cancelled copy must not get one.
    let windows = Arc::new(Mutex::new(Vec::new()));
    let watching = Arc::new(AtomicBool::new(true));
    let watcher = {
        let (windows, watching) = (Arc::clone(&windows), Arc::clone(&watching));
        std::thread::spawn(move || {
            while watching.load(Ordering::SeqCst) {
                close_windows(&windows);
                std::thread::sleep(Duration::from_millis(20));
            }
        })
    };
    let outcome = nexssh_dragout::drop_into(&target, source.clone()).unwrap();
    let result = wait(&ended, Duration::from_secs(60));
    watching.store(false, Ordering::SeqCst);
    watcher.join().unwrap();

    assert_eq!(outcome, Outcome::Dropped);
    assert_eq!(result, Ended::Cancelled);
    assert_eq!(*windows.lock().unwrap(), Vec::<String>::new());
    // What was copied stays; the file stopped part of the way does not.
    assert_eq!(
        std::fs::read(target.join("whole.bin")).unwrap(),
        vec![1u8; 1_000_000]
    );
    assert!(!target.join("stopped.bin").exists());
    let _ = std::fs::remove_dir_all(&target);
}

/// Nothing to drag: a drag that ends at once never asks.
struct Nothing;

impl Source for Nothing {
    fn items(&self) -> Result<Vec<Item>, String> {
        Ok(Vec::new())
    }

    fn open(&self, _: usize) -> std::io::Result<Box<dyn Read + Send>> {
        Err(ErrorKind::NotFound.into())
    }

    fn finished(&self, _: Ended) {}
}

/// A drag on a thread with COM but not OLE, as the app's window thread is (its window takes no
/// drops itself), cancelled with Esc, which is waiting in the thread's queue.
#[test]
fn a_drag_runs_where_ole_is_not_set_up_and_esc_cancels_it() {
    let (done, finished) = channel();
    std::thread::spawn(move || {
        // SAFETY: initializes COM for this new thread.
        unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) }
            .ok()
            .unwrap();
        // SAFETY: posts to this thread's own queue (0x1B: VK_ESCAPE).
        unsafe { PostMessageW(None, WM_KEYDOWN, WPARAM(0x1B), LPARAM(0)) }.unwrap();
        let _ = done.send(nexssh_dragout::drag(HWND::default(), Arc::new(Nothing)));
    });
    // A drag that does not end holds the mouse: the test process goes rather than wait.
    let outcome = finished
        .recv_timeout(Duration::from_secs(10))
        .unwrap_or_else(|_| {
            eprintln!("the drag did not end");
            std::process::exit(1)
        });
    assert_eq!(outcome.unwrap(), Outcome::Cancelled);
}

/// Closes the visible windows of this process, noting their class and title.
fn close_windows(seen: &Mutex<Vec<String>>) {
    unsafe extern "system" fn each(window: HWND, seen: LPARAM) -> BOOL {
        // SAFETY: `seen` is the list passed to EnumWindows below, alive for the call.
        let seen = unsafe { &*(seen.0 as *const Mutex<Vec<String>>) };
        let mut process = 0;
        // SAFETY: plain Win32 calls on a window EnumWindows gave.
        unsafe {
            GetWindowThreadProcessId(window, Some(&mut process));
            if process == std::process::id() && IsWindowVisible(window).as_bool() {
                let (mut class, mut title) = ([0u16; 128], [0u16; 256]);
                let class_len = GetClassNameW(window, &mut class) as usize;
                let title_len = GetWindowTextW(window, &mut title) as usize;
                seen.lock().unwrap().push(format!(
                    "{} {:?}",
                    String::from_utf16_lossy(&class[..class_len]),
                    String::from_utf16_lossy(&title[..title_len])
                ));
                let _ = PostMessageW(Some(window), WM_CLOSE, WPARAM(0), LPARAM(0));
            }
        }
        true.into()
    }
    // SAFETY: the callback only reads `seen` while EnumWindows runs.
    let _ = unsafe { EnumWindows(Some(each), LPARAM(std::ptr::from_ref(seen) as isize)) };
}

/// Waits for the end of the copy, running this thread's messages meanwhile, as a window's
/// thread would (the shell may post some to it).
fn wait(ended: &Receiver<Ended>, limit: Duration) -> Ended {
    let start = Instant::now();
    loop {
        if let Ok(ended) = ended.try_recv() {
            return ended;
        }
        assert!(start.elapsed() < limit, "the copy did not end");
        let mut message = MSG::default();
        // SAFETY: the message loop of this thread's own queue.
        unsafe {
            while PeekMessageW(&mut message, None, 0, 0, PM_REMOVE).as_bool() {
                let _ = TranslateMessage(&message);
                DispatchMessageW(&message);
            }
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}
