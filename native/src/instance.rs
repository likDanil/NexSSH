//! One GUI per data directory. Helper commands bypass this coordinator in main.rs.
//! An OS file lock survives neither a normal exit nor a crash. Launches travel over
//! an authenticated loopback connection; only the GUI thread changes windows/tabs.
use serde::{Deserialize, Serialize};
use std::{
    fs::{self, File, OpenOptions, TryLockError},
    io::{self, Read, Write},
    net::{Ipv4Addr, SocketAddr, TcpListener, TcpStream},
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

const LOCK_FILE: &str = ".native-instance.lock";
const ENDPOINT_FILE: &str = ".native-instance.json";
const GREETING: &[u8; 8] = b"NexSSH2\0";
const MAX_MESSAGE: usize = 64 * 1024;
const MAX_FOLDERS: usize = 32;
const IO_TIMEOUT: Duration = Duration::from_millis(750);
const STARTUP_TIMEOUT: Duration = Duration::from_secs(6);

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Launch {
    pub folders: Vec<PathBuf>,
}

impl Launch {
    /// Args exclude the executable; relative Explorer paths belong to the sender's cwd.
    pub fn from_args(args: &[String], cwd: &Path) -> io::Result<Self> {
        let mut folders = vec![];
        let mut args = args.iter();
        while let Some(arg) = args.next() {
            let value = if arg == "--cwd" {
                args.next().map(String::as_str)
            } else {
                arg.strip_prefix("--cwd=")
            };
            let Some(value) = value.map(str::trim).filter(|v| !v.is_empty()) else {
                continue;
            };
            let mut value = value.to_owned();
            // Explorer's quoted drive root "D:\" arrives as D:" (same fix as 1.x).
            if cfg!(windows) && value.ends_with('"') {
                value.pop();
                value.push('\\');
            }
            let path = nexssh_core::expand_tilde(&value);
            let path = if path.is_absolute() {
                path
            } else {
                cwd.join(path)
            };
            folders.push(path);
        }
        let launch = Self { folders };
        launch.validate()?;
        Ok(launch)
    }

    fn validate(&self) -> io::Result<()> {
        if self.folders.len() > MAX_FOLDERS || self.folders.iter().any(|p| !p.is_absolute()) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "Invalid NexSSH launch folders",
            ));
        }
        Ok(())
    }
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Endpoint {
    version: u32,
    pid: u32,
    port: u16,
    token: String,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Message {
    token: String,
    launch: Launch,
}

pub enum Start {
    Primary(Primary),
    Forwarded,
}

pub struct Primary {
    _lock: File,
    endpoint: PathBuf,
    address: SocketAddr,
    stop: Arc<AtomicBool>,
    worker: Option<JoinHandle<()>>,
    requests: Option<mpsc::Receiver<Launch>>,
}

/// Called before opening settings, credentials, the tray or any GUI window.
pub fn start(data_dir: &Path, launch: &Launch) -> io::Result<Start> {
    launch.validate()?;
    fs::create_dir_all(data_dir)?;
    let mut options = OpenOptions::new();
    options.read(true).write(true).create(true).truncate(false);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    // Never unlink this file: a different inode would permit two concurrent owners.
    let lock = options.open(data_dir.join(LOCK_FILE))?;
    let deadline = Instant::now() + STARTUP_TIMEOUT;
    loop {
        match lock.try_lock() {
            Ok(()) => return Primary::new(data_dir, lock).map(Start::Primary),
            Err(TryLockError::Error(error)) => return Err(error),
            Err(TryLockError::WouldBlock) => {}
        }
        if let Ok(endpoint) = read_endpoint(data_dir)
            && forward(&endpoint, launch).is_ok()
        {
            return Ok(Start::Forwarded);
        }
        if Instant::now() >= deadline {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "NexSSH is already running, but its window did not respond. Please retry.",
            ));
        }
        // Includes startup/shutdown races: retry the lock, not just the old endpoint.
        thread::sleep(Duration::from_millis(30));
    }
}

impl Primary {
    fn new(data_dir: &Path, lock: File) -> io::Result<Self> {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))?;
        let address = listener.local_addr()?;
        let mut random = [0u8; 32];
        getrandom::fill(&mut random).map_err(io::Error::other)?;
        let token: String = random.iter().map(|b| format!("{b:02x}")).collect();
        let endpoint = Endpoint {
            version: 1,
            pid: std::process::id(),
            port: address.port(),
            token: token.clone(),
        };
        let endpoint_path = data_dir.join(ENDPOINT_FILE);
        // NamedTempFile uses owner-only Unix permissions; Windows inherits the user's data-dir ACL.
        let mut metadata = tempfile::NamedTempFile::new_in(data_dir)?;
        metadata.write_all(&serde_json::to_vec(&endpoint)?)?;
        metadata.persist(&endpoint_path).map_err(|e| e.error)?;
        let (sender, receiver) = mpsc::sync_channel(32);
        let stop = Arc::new(AtomicBool::new(false));
        let worker_stop = Arc::clone(&stop);
        let worker = match thread::Builder::new()
            .name("nexssh-launches".into())
            .spawn(move || {
                // Blocking accept sleeps in the OS; no polling timer or extra process.
                for connection in listener.incoming() {
                    if worker_stop.load(Ordering::Acquire) {
                        break;
                    }
                    match connection {
                        Ok(mut stream) => {
                            let _ = serve(&mut stream, &token, &sender);
                        }
                        Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                        Err(_) => break,
                    }
                }
            }) {
            Ok(worker) => worker,
            Err(error) => {
                let _ = fs::remove_file(&endpoint_path);
                return Err(error);
            }
        };
        Ok(Self {
            _lock: lock,
            endpoint: endpoint_path,
            address,
            stop,
            worker: Some(worker),
            requests: Some(receiver),
        })
    }

    pub fn take_requests(&mut self) -> mpsc::Receiver<Launch> {
        self.requests.take().expect("launch receiver is taken once")
    }

    /// Stop accepting before slow session cleanup, while retaining the lock until exit.
    pub fn stop_accepting(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(worker) = self.worker.take() {
            let _ = TcpStream::connect_timeout(&self.address, IO_TIMEOUT);
            let _ = worker.join();
            let _ = fs::remove_file(&self.endpoint);
        }
    }
}

impl Drop for Primary {
    fn drop(&mut self) {
        self.stop_accepting();
    }
}

fn read_endpoint(data_dir: &Path) -> io::Result<Endpoint> {
    let mut bytes = vec![];
    File::open(data_dir.join(ENDPOINT_FILE))?
        .take(4097)
        .read_to_end(&mut bytes)?;
    if bytes.len() > 4096 {
        return Err(io::Error::other("Invalid NexSSH endpoint"));
    }
    let endpoint: Endpoint = serde_json::from_slice(&bytes)?;
    if endpoint.version != 1
        || endpoint.pid == 0
        || endpoint.port == 0
        || endpoint.token.len() != 64
    {
        return Err(io::Error::other("Invalid NexSSH endpoint"));
    }
    Ok(endpoint)
}

fn configure(stream: &TcpStream) -> io::Result<()> {
    stream.set_read_timeout(Some(IO_TIMEOUT))?;
    stream.set_write_timeout(Some(IO_TIMEOUT))?;
    stream.set_nodelay(true)
}

fn forward(endpoint: &Endpoint, launch: &Launch) -> io::Result<()> {
    let bytes = serde_json::to_vec(&Message {
        token: endpoint.token.clone(),
        launch: launch.clone(),
    })?;
    if bytes.len() > MAX_MESSAGE {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "NexSSH launch is too large",
        ));
    }
    let mut stream =
        TcpStream::connect_timeout(&(Ipv4Addr::LOCALHOST, endpoint.port).into(), IO_TIMEOUT)?;
    configure(&stream)?;
    let mut greeting = [0u8; 12];
    stream.read_exact(&mut greeting)?;
    if &greeting[..8] != GREETING
        || u32::from_be_bytes(greeting[8..].try_into().unwrap()) != endpoint.pid
    {
        return Err(io::Error::other("Unexpected NexSSH endpoint"));
    }
    #[cfg(windows)]
    {
        // A launch from Explorer passes its foreground permission to the existing GUI.
        use windows::Win32::UI::WindowsAndMessaging::AllowSetForegroundWindow;
        let _ = unsafe { AllowSetForegroundWindow(endpoint.pid) };
    }
    stream.write_all(&(bytes.len() as u32).to_be_bytes())?;
    stream.write_all(&bytes)?;
    let mut accepted = [0];
    stream.read_exact(&mut accepted)?;
    if accepted != [1] {
        return Err(io::Error::other("NexSSH launch was not accepted"));
    }
    Ok(())
}

fn serve(stream: &mut TcpStream, token: &str, sender: &mpsc::SyncSender<Launch>) -> io::Result<()> {
    configure(stream)?;
    stream.write_all(GREETING)?;
    stream.write_all(&std::process::id().to_be_bytes())?;
    let mut header = [0; 4];
    stream.read_exact(&mut header)?;
    let length = u32::from_be_bytes(header) as usize;
    if length == 0 || length > MAX_MESSAGE {
        return Err(io::Error::other("Invalid NexSSH launch size"));
    }
    let mut bytes = vec![0; length];
    stream.read_exact(&mut bytes)?;
    let message: Message = serde_json::from_slice(&bytes)?;
    message.launch.validate()?;
    let accepted = same_token(&message.token, token) && sender.try_send(message.launch).is_ok();
    stream.write_all(&[u8::from(accepted)])
}

fn same_token(given: &str, expected: &str) -> bool {
    given.len() == expected.len()
        && given
            .bytes()
            .zip(expected.bytes())
            .fold(0u8, |difference, (a, b)| difference | (a ^ b))
            == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn forwards_requests_instead_of_creating_another_primary() {
        let directory = tempfile::tempdir().unwrap();
        let Start::Primary(mut primary) = start(directory.path(), &Launch::default()).unwrap()
        else {
            panic!("first launch");
        };
        let receiver = primary.take_requests();
        let launch = Launch {
            folders: vec![directory.path().join("Папка с пробелами")],
        };
        assert!(matches!(
            start(directory.path(), &launch).unwrap(),
            Start::Forwarded
        ));
        assert_eq!(
            receiver.recv_timeout(Duration::from_secs(2)).unwrap(),
            launch
        );
        assert!(matches!(
            start(directory.path(), &Launch::default()).unwrap(),
            Start::Forwarded
        ));
        assert_eq!(
            receiver.recv_timeout(Duration::from_secs(2)).unwrap(),
            Launch::default()
        );
        drop(primary);
        assert!(matches!(
            start(directory.path(), &Launch::default()).unwrap(),
            Start::Primary(_)
        ));
    }

    #[test]
    fn stale_metadata_is_replaced_and_separate_profiles_are_independent() {
        let first = tempfile::tempdir().unwrap();
        let second = tempfile::tempdir().unwrap();
        fs::write(first.path().join(ENDPOINT_FILE), b"stale metadata").unwrap();
        let Start::Primary(_first) = start(first.path(), &Launch::default()).unwrap() else {
            panic!();
        };
        let Start::Primary(_second) = start(second.path(), &Launch::default()).unwrap() else {
            panic!();
        };
        let endpoint = read_endpoint(first.path()).unwrap();
        assert_eq!(endpoint.pid, std::process::id());
        assert_ne!(endpoint.port, read_endpoint(second.path()).unwrap().port);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(first.path().join(ENDPOINT_FILE))
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o600
            );
        }
    }

    #[test]
    fn untrusted_or_oversized_requests_do_not_reach_the_ui() {
        let directory = tempfile::tempdir().unwrap();
        let Start::Primary(mut primary) = start(directory.path(), &Launch::default()).unwrap()
        else {
            panic!();
        };
        let receiver = primary.take_requests();
        let mut endpoint = read_endpoint(directory.path()).unwrap();
        endpoint.token = "0".repeat(64);
        assert!(forward(&endpoint, &Launch::default()).is_err());
        assert!(receiver.try_recv().is_err());
        let endpoint = read_endpoint(directory.path()).unwrap();
        let mut stream = TcpStream::connect((Ipv4Addr::LOCALHOST, endpoint.port)).unwrap();
        configure(&stream).unwrap();
        stream.read_exact(&mut [0; 12]).unwrap();
        stream
            .write_all(&((MAX_MESSAGE + 1) as u32).to_be_bytes())
            .unwrap();
        drop(stream);
        assert!(forward(&endpoint, &Launch::default()).is_ok());
        assert_eq!(
            receiver.recv_timeout(Duration::from_secs(2)).unwrap(),
            Launch::default()
        );
    }

    #[test]
    fn reads_launch_folders_and_keeps_bare_launches_empty() {
        let directory = tempfile::tempdir().unwrap();
        assert!(
            Launch::from_args(&[], directory.path())
                .unwrap()
                .folders
                .is_empty()
        );
        let args = [
            "--cwd",
            "Проект с пробелами",
            "--cwd=second",
            "--cwdx",
            "ignored",
        ]
        .map(String::from);
        assert_eq!(
            Launch::from_args(&args, directory.path()).unwrap().folders,
            [
                directory.path().join("Проект с пробелами"),
                directory.path().join("second")
            ]
        );
        assert!(
            Launch::from_args(&["--cwd".into()], directory.path())
                .unwrap()
                .folders
                .is_empty()
        );
        #[cfg(windows)]
        assert_eq!(
            Launch::from_args(&["--cwd".into(), "D:\"".into()], directory.path())
                .unwrap()
                .folders,
            [PathBuf::from(r"D:\")]
        );
    }
}
