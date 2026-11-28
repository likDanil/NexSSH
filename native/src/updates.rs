//! Signed NSIS updates. Stable clients never discover prereleases; previews graduate to stable.
use base64::{Engine, engine::general_purpose::STANDARD};
use minisign_verify::{PublicKey, Signature};
use semver::Version;
use serde::Deserialize;
use std::{collections::BTreeMap, io::Write, sync::mpsc, time::Duration};

type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;
const STABLE: &str = "https://github.com/likDanil/NexSSH/releases/latest/download/latest.json";
const RELEASES: &str = "https://api.github.com/repos/likDanil/NexSSH/releases?per_page=100";
const MAX_MANIFEST: usize = 2 * 1024 * 1024;
const MAX_INSTALLER: usize = 256 * 1024 * 1024;

#[derive(Clone, Debug, Deserialize)]
pub struct Artifact {
    url: reqwest::Url,
    signature: String,
}
#[derive(Clone, Debug, Deserialize)]
pub struct Release {
    pub version: Version,
    platforms: BTreeMap<String, Artifact>,
}
impl Release {
    fn artifact(&self) -> Result<&Artifact> {
        self.platforms
            .get("windows-x86_64-nsis")
            .ok_or_else(|| "No Windows x64 NSIS installer in the update manifest".into())
    }
}

/// A preview stays in its release family until that version becomes stable.
fn eligible(current: &Version, candidate: &Version, stable_feed: bool) -> bool {
    candidate > current
        && candidate.build.is_empty()
        && if stable_feed {
            candidate.pre.is_empty()
        } else {
            !current.pre.is_empty()
                && !candidate.pre.is_empty()
                && (candidate.major, candidate.minor, candidate.patch)
                    == (current.major, current.minor, current.patch)
        }
}

fn safe_url(url: &reqwest::Url) -> Result<()> {
    let local = matches!(url.host_str(), Some("127.0.0.1" | "[::1]" | "localhost"));
    if (url.scheme() == "https" || (url.scheme() == "http" && local))
        && url.username().is_empty()
        && url.password().is_none()
    {
        Ok(())
    } else {
        Err("Update URLs must use HTTPS (HTTP is allowed only on loopback for tests)".into())
    }
}
fn client() -> Result<reqwest::Client> {
    Ok(reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(15))
        .timeout(Duration::from_secs(180))
        .user_agent(concat!("NexSSH/", env!("CARGO_PKG_VERSION")))
        .redirect(reqwest::redirect::Policy::custom(|attempt| {
            if attempt.previous().len() >= 8 {
                attempt.error("Too many redirects")
            } else if safe_url(attempt.url()).is_err() {
                attempt.error("Unsafe update redirect")
            } else {
                attempt.follow()
            }
        }))
        .build()?)
}
async fn body(
    client: &reqwest::Client,
    url: &reqwest::Url,
    limit: usize,
    mut progress: impl FnMut(u64, Option<u64>),
) -> Result<Option<Vec<u8>>> {
    safe_url(url)?;
    let mut response = client.get(url.clone()).send().await?;
    if response.status() == reqwest::StatusCode::NOT_FOUND
        || response.status() == reqwest::StatusCode::NO_CONTENT
    {
        return Ok(None);
    }
    response = response.error_for_status()?;
    let total = response.content_length();
    if total.is_some_and(|n| n > limit as u64) {
        return Err("Update response is too large".into());
    }
    let mut bytes = Vec::new();
    let mut last_percent = u64::MAX;
    while let Some(chunk) = response.chunk().await? {
        if bytes.len().saturating_add(chunk.len()) > limit {
            return Err("Update response is too large".into());
        }
        bytes.extend_from_slice(&chunk);
        let percent = total
            .filter(|n| *n > 0)
            .map(|n| bytes.len() as u64 * 100 / n)
            .unwrap_or(0);
        if percent != last_percent {
            progress(bytes.len() as u64, total);
            last_percent = percent;
        }
    }
    Ok(Some(bytes))
}
async fn manifest(client: &reqwest::Client, url: &reqwest::Url) -> Result<Option<Release>> {
    body(client, url, MAX_MANIFEST, |_, _| {})
        .await?
        .map(|bytes| serde_json::from_slice(&bytes).map_err(Into::into))
        .transpose()
}

#[derive(Deserialize)]
struct GithubRelease {
    tag_name: String,
    draft: bool,
    prerelease: bool,
    assets: Vec<GithubAsset>,
}
#[derive(Deserialize)]
struct GithubAsset {
    name: String,
    browser_download_url: reqwest::Url,
}

fn newest_preview(
    releases: Vec<GithubRelease>,
    current: &Version,
) -> Option<(Version, reqwest::Url)> {
    releases
        .into_iter()
        .filter_map(|r| {
            let version =
                Version::parse(r.tag_name.strip_prefix('v').unwrap_or(&r.tag_name)).ok()?;
            if r.draft || !r.prerelease || !eligible(current, &version, false) {
                return None;
            }
            let asset = r.assets.into_iter().find(|a| a.name == "preview.json")?;
            Some((version, asset.browser_download_url))
        })
        .max_by(|a, b| a.0.cmp(&b.0))
}

async fn discover(current: &Version) -> Result<Option<Release>> {
    let client = client()?;
    if let Ok(url) = std::env::var("NEXSSH_UPDATE_URL") {
        let release = manifest(&client, &url.parse()?).await?;
        return Ok(release.filter(|r| eligible(current, &r.version, r.version.pre.is_empty())));
    }
    // Prefer the final release as soon as it exists, even if there are newer preview tags.
    let stable_result = manifest(&client, &STABLE.parse()?).await;
    if let Ok(Some(release)) = &stable_result
        && eligible(current, &release.version, true)
    {
        return Ok(Some(release.clone()));
    }
    if current.pre.is_empty() {
        return stable_result.map(|_| None);
    }
    let bytes = body(&client, &RELEASES.parse()?, MAX_MANIFEST, |_, _| {})
        .await?
        .ok_or("No release listing")?;
    let releases: Vec<GithubRelease> = serde_json::from_slice(&bytes)?;
    if let Some((version, url)) = newest_preview(releases, current) {
        let release = manifest(&client, &url)
            .await?
            .ok_or("Preview manifest is missing")?;
        if release.version != version {
            return Err("Preview tag and manifest versions differ".into());
        }
        Ok(Some(release))
    } else {
        // Do not report 'up to date' if checking the stable channel failed.
        stable_result.map(|_| None)
    }
}

pub fn verify(bytes: &[u8], release: &Release, public_key: &str) -> Result<()> {
    let key = String::from_utf8(STANDARD.decode(public_key.trim())?)?;
    let signature = String::from_utf8(STANDARD.decode(release.artifact()?.signature.trim())?)?;
    let signature = Signature::decode(&signature)?;
    PublicKey::decode(&key)?.verify(bytes, &signature, false)?;
    let signed = signature
        .trusted_comment()
        .split('\t')
        .find_map(|f| f.strip_prefix("version:"))
        .ok_or("Update signature has no authenticated version")?;
    if Version::parse(signed.strip_prefix('v').unwrap_or(signed))? != release.version {
        return Err("Signed installer version differs from the update manifest".into());
    }
    Ok(())
}

struct Download {
    release: Release,
    dir: tempfile::TempDir,
}
enum Event {
    Checked(std::result::Result<Option<Release>, String>),
    Progress(u64, Option<u64>),
    Downloaded(std::result::Result<Download, String>),
}
#[derive(Default)]
enum State {
    #[default]
    Idle,
    Checking,
    Available(Release),
    Downloading(Option<u64>),
    Ready(Download),
    Failed(String),
}
pub struct Updater {
    state: State,
    sender: mpsc::Sender<Event>,
    receiver: mpsc::Receiver<Event>,
    checked: bool,
}
impl Default for Updater {
    fn default() -> Self {
        let (sender, receiver) = mpsc::channel();
        Self {
            state: State::Idle,
            sender,
            receiver,
            checked: false,
        }
    }
}
impl Updater {
    pub fn check(&mut self) {
        if matches!(
            self.state,
            State::Checking | State::Downloading(_) | State::Ready(_)
        ) {
            return;
        }
        self.state = State::Checking;
        let sender = self.sender.clone();
        std::thread::spawn(move || {
            let result = run(discover(
                &Version::parse(env!("CARGO_PKG_VERSION")).unwrap(),
            ))
            .map_err(|e| e.to_string());
            let _ = sender.send(Event::Checked(result));
        });
    }
    pub fn poll(&mut self) -> bool {
        let mut changed = false;
        while let Ok(event) = self.receiver.try_recv() {
            changed = true;
            match event {
                Event::Checked(Ok(Some(release))) => self.state = State::Available(release),
                Event::Checked(Ok(None)) => {
                    self.state = State::Idle;
                    self.checked = true;
                }
                Event::Checked(Err(error)) | Event::Downloaded(Err(error)) => {
                    self.state = State::Failed(error)
                }
                Event::Downloaded(Ok(download)) => self.state = State::Ready(download),
                Event::Progress(received, total) => {
                    if let State::Downloading(percent) = &mut self.state {
                        *percent = total.filter(|n| *n > 0).map(|n| received * 100 / n);
                    }
                }
            }
        }
        changed
    }
    pub fn ready(&self) -> bool {
        matches!(self.state, State::Ready(_))
    }
    pub fn offer(&self) -> bool {
        matches!(
            self.state,
            State::Available(_) | State::Downloading(_) | State::Ready(_)
        )
    }
    pub fn enabled(&self) -> bool {
        !matches!(self.state, State::Checking | State::Downloading(_))
    }
    pub fn label(&self, lang: &str) -> String {
        crate::i18n::tr(
            match self.state {
                State::Available(_) => "update.download",
                State::Ready(_) => "update.install",
                State::Checking => "update.checking",
                State::Downloading(_) => "update.downloadingUnknown",
                _ => "update.check",
            },
            lang,
        )
    }
    pub fn status(&self, lang: &str) -> String {
        let tr = |key| crate::i18n::tr(key, lang);
        match &self.state {
            State::Idle => tr(if self.checked {
                "update.upToDate"
            } else {
                "update.idle"
            }),
            State::Checking => tr("update.checking"),
            State::Available(r) => {
                tr("update.available").replace("{version}", &r.version.to_string())
            }
            State::Downloading(percent) => percent
                .map(|n| tr("update.downloading").replace("{percent}", &n.to_string()))
                .unwrap_or_else(|| tr("update.downloadingUnknown")),
            State::Ready(d) => {
                tr("update.ready").replace("{version}", &d.release.version.to_string())
            }
            State::Failed(error) => format!(
                "{}: {error}",
                if lang == "ru" {
                    "Ошибка обновления"
                } else {
                    "Update failed"
                }
            ),
        }
    }
    pub fn download_or_check(&mut self) {
        let State::Available(release) = &self.state else {
            self.check();
            return;
        };
        let release = release.clone();
        self.state = State::Downloading(None);
        let sender = self.sender.clone();
        std::thread::spawn(move || {
            let result = run(async {
                let artifact = release.artifact()?;
                let bytes = body(&client()?, &artifact.url, MAX_INSTALLER, |n, total| {
                    let _ = sender.send(Event::Progress(n, total));
                })
                .await?
                .ok_or("Installer was not found")?;
                verify(&bytes, &release, env!("NEXSSH_UPDATER_PUBLIC_KEY"))?;
                let dir = tempfile::Builder::new()
                    .prefix("nexssh-update-")
                    .tempdir()?;
                let mut file = std::fs::File::create(dir.path().join("update.exe"))?;
                file.write_all(&bytes)?;
                file.sync_all()?;
                Ok(Download { release, dir })
            })
            .map_err(|e| e.to_string());
            let _ = sender.send(Event::Downloaded(result));
        });
    }
    pub fn install(&mut self) -> Result<()> {
        let State::Ready(download) = &self.state else {
            return Err("No verified update is ready".into());
        };
        let path = download.dir.path().join("update.exe");
        verify(
            &std::fs::read(&path)?,
            &download.release,
            env!("NEXSSH_UPDATER_PUBLIC_KEY"),
        )?;
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            let exe = std::env::current_exe()?;
            // /D is the last NSIS parameter, with no surrounding quotes, even for spaces.
            std::process::Command::new(path)
                .args(["/S", "/UPDATE", "/R"])
                .arg(format!("/NEXSSH_WAIT_PID={}", std::process::id()))
                .raw_arg(format!(
                    "/D={}",
                    exe.parent().ok_or("No installation directory")?.display()
                ))
                .spawn()?;
            if let State::Ready(download) = std::mem::take(&mut self.state) {
                let _ = download.dir.keep();
            }
            Ok(())
        }
        #[cfg(not(windows))]
        {
            Err("Automatic installation is supported on Windows x64".into())
        }
    }
}
fn run<T>(future: impl std::future::Future<Output = Result<T>>) -> Result<T> {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?
        .block_on(future)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn preview_discovery_requires_channel_asset_and_matching_family() {
        let release = |version: &str, draft, prerelease, name: &str| {
            serde_json::json!({
                "tag_name":version,"draft":draft,"prerelease":prerelease,
                "assets":[{"name":name,"browser_download_url":"https://example.org/preview.json"}]
            })
        };
        let releases = serde_json::from_value(serde_json::json!([
            release("v2.0.0-alpha.2", false, true, "preview.json"),
            release("v2.0.0-beta.1", false, true, "preview.json"),
            release("v2.0.0-rc.1", true, true, "preview.json"),
            release("v2.0.0-rc.2", false, false, "preview.json"),
            release("v2.0.0-rc.3", false, true, "latest.json"),
            release("v2.1.0-alpha.1", false, true, "preview.json")
        ]))
        .unwrap();
        let current = Version::parse("2.0.0-alpha.1").unwrap();
        assert_eq!(
            newest_preview(releases, &current).unwrap().0.to_string(),
            "2.0.0-beta.1"
        );
    }
    fn fixture(signature: &str, version: &str) -> Release {
        serde_json::from_value(serde_json::json!({"version":version,"platforms":{
            "windows-x86_64-nsis":{"url":"https://example.org/update.exe","signature":signature}
        }}))
        .unwrap()
    }
    #[test]
    fn verifies_signed_artifacts_and_rejects_tampering_replay_and_legacy_signatures() {
        let payload = include_bytes!("../tests/fixtures/updater/payload.txt");
        let signature = include_str!("../tests/fixtures/updater/payload.txt.sig");
        let key = include_str!("../tests/fixtures/updater/test.key.pub");
        let release = fixture(signature.trim(), "9999.0.0");
        verify(payload, &release, key).unwrap();
        assert!(verify(b"modified installer", &release, key).is_err());
        // An unsigned manifest cannot replay an older, valid installer as a newer version.
        let replay = fixture(signature.trim(), "9999.0.1");
        assert!(
            verify(payload, &replay, key)
                .unwrap_err()
                .to_string()
                .contains("version differs")
        );
        let old = fixture(
            include_str!("../tests/fixtures/updater/unversioned.sig").trim(),
            "9999.0.0",
        );
        assert!(
            verify(payload, &old, key)
                .unwrap_err()
                .to_string()
                .contains("no authenticated version")
        );
        assert!(verify(payload, &release, "not a key").is_err());
    }
    #[test]
    fn http_download_is_bounded_and_reports_errors() {
        use std::{
            io::{Read, Write},
            net::TcpListener,
        };
        for (response, limit, expected) in [
            (
                "HTTP/1.1 200 OK\r\nContent-Length: 4\r\n\r\ntest",
                4,
                "test",
            ),
            (
                "HTTP/1.1 200 OK\r\nContent-Length: 100\r\n\r\n",
                4,
                "too large",
            ),
            (
                "HTTP/1.1 503 Unavailable\r\nContent-Length: 0\r\n\r\n",
                4,
                "503",
            ),
        ] {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let address = listener.local_addr().unwrap();
            let server = std::thread::spawn(move || {
                let (mut stream, _) = listener.accept().unwrap();
                assert!(stream.read(&mut [0; 4096]).unwrap() > 0);
                stream.write_all(response.as_bytes()).unwrap();
            });
            let url = format!("http://{address}/update").parse().unwrap();
            let result = run(async { body(&client()?, &url, limit, |_, _| {}).await });
            if expected == "test" {
                assert_eq!(result.unwrap().unwrap(), b"test");
            } else {
                assert!(result.unwrap_err().to_string().contains(expected));
            }
            server.join().unwrap();
        }
    }
    #[test]
    fn channels_and_graduation() {
        let v = |s| Version::parse(s).unwrap();
        for (current, next, stable, expected) in [
            ("1.5.1", "2.0.0-alpha.1", true, false),
            ("2.0.0-alpha.1", "2.0.0-alpha.2", false, true),
            ("2.0.0-beta.1", "2.0.0-alpha.9", false, false),
            ("2.0.0-alpha.1", "2.1.0-alpha.1", false, false),
            ("2.0.0-rc.1", "2.0.0", true, true),
            ("2.0.0-alpha.1", "1.5.2", true, false),
            ("2.0.0", "2.0.1-beta.1", true, false),
            ("2.0.0", "2.0.1", true, true),
        ] {
            assert_eq!(
                eligible(&v(current), &v(next), stable),
                expected,
                "{current} → {next}"
            );
        }
    }
    #[test]
    fn rejects_remote_http_credentials_and_bad_schemes() {
        for url in [
            "http://example.org/update",
            "file:///tmp/update",
            "https://user:secret@example.org/update",
        ] {
            assert!(safe_url(&url.parse().unwrap()).is_err());
        }
        for url in [
            STABLE,
            "http://127.0.0.1:8080/latest.json",
            "http://[::1]:8080/latest.json",
        ] {
            assert!(safe_url(&url.parse().unwrap()).is_ok());
        }
    }
}
