//! Work that must not block the Slint event loop.
use nexssh_core::{Core, ShellProfile, backup};
use std::{
    collections::BTreeMap,
    path::PathBuf,
    sync::{Arc, mpsc},
};

pub enum ServiceEvent {
    Metadata {
        shells: Vec<ShellProfile>,
        editors: Vec<(String, String)>,
        keychain: Option<String>,
    },
    Notice(String),
    Chosen(Box<backup::Backup>),
    RestorePrepared {
        backup: Box<backup::Backup>,
        passwords: Result<BTreeMap<String, String>, String>,
    },
    Restored {
        settings: serde_json::Value,
        failed: usize,
    },
    Preference(&'static str, String),
    Clipboard(String),
    AgentToken(String),
}

pub struct Services {
    pub shells: Vec<ShellProfile>,
    pub editors: Vec<(String, String)>,
    pub keychain: String,
    pub keychain_available: bool,
    pub receiver: mpsc::Receiver<ServiceEvent>,
    sender: mpsc::Sender<ServiceEvent>,
    pub updater: crate::updates::Updater,
}

impl Services {
    pub fn new(core: Arc<Core>) -> Self {
        let (sender, receiver) = mpsc::channel();
        let tx = sender.clone();
        std::thread::spawn(move || {
            let shells = nexssh_core::local::shells();
            let editors = discover_editors();
            let keychain = core.secrets.unavailable_reason();
            let _ = tx.send(ServiceEvent::Metadata {
                shells,
                editors,
                keychain,
            });
        });
        Self {
            shells: vec![],
            editors: vec![],
            keychain: "…".into(),
            keychain_available: false,
            receiver,
            sender,
            updater: crate::updates::Updater::default(),
        }
    }

    pub fn job<F>(&self, job: F)
    where
        F: FnOnce() -> Result<Option<ServiceEvent>, Box<dyn std::error::Error + Send + Sync>>
            + Send
            + 'static,
    {
        let sender = self.sender.clone();
        std::thread::spawn(move || match job() {
            Ok(Some(event)) => {
                let _ = sender.send(event);
            }
            Ok(None) => {}
            Err(error) => {
                log::warn!("{error}");
                let _ = sender.send(ServiceEvent::Notice(error.to_string()));
            }
        });
    }

    pub fn export(&self, core: Arc<Core>, settings: serde_json::Value, password: String) {
        self.job(move || {
            let Some(path) = rfd::FileDialog::new()
                .add_filter("NexSSH backup", &["json"])
                .set_file_name("nexssh-backup.json")
                .save_file()
            else {
                return Ok(None);
            };
            let created = backup::create(
                &core,
                settings,
                (!password.is_empty()).then_some(password.as_str()),
            )?;
            // The temporary file lives alongside the destination; rename never leaves a partial backup.
            let temporary = path.with_extension("json.tmp");
            let mut value: serde_json::Value = serde_json::from_slice(&created.json)?;
            value["appVersion"] = env!("CARGO_PKG_VERSION").into();
            std::fs::write(&temporary, serde_json::to_vec_pretty(&value)?)?;
            std::fs::rename(temporary, &path)?;
            Ok(Some(ServiceEvent::Notice(format!(
                "{} · {}",
                path.display(),
                created.passwords
            ))))
        });
    }
    pub fn import(&self) {
        self.job(|| {
            let Some(path) = rfd::FileDialog::new()
                .add_filter("NexSSH backup", &["json"])
                .pick_file()
            else {
                return Ok(None);
            };
            if std::fs::metadata(&path)?.len() > 16 * 1024 * 1024 {
                return Err("Backup exceeds 16 MiB".into());
            }
            Ok(Some(ServiceEvent::Chosen(Box::new(backup::parse(
                &std::fs::read(path)?,
            )?))))
        });
    }
    pub fn restore(
        &self,
        core: Arc<Core>,
        backup: backup::Backup,
        passwords: BTreeMap<String, String>,
    ) {
        self.job(move || {
            let report = backup::restore(&core, &backup, &passwords)?;
            log::info!(
                "Restored {} servers, {} passwords, {} password errors",
                report.servers,
                report.passwords,
                report.passwords_failed
            );
            Ok(Some(ServiceEvent::Restored {
                settings: backup.settings,
                failed: report.passwords_failed,
            }))
        });
    }
    pub fn prepare_restore(&self, backup: Box<backup::Backup>, password: String) {
        self.job(move || {
            let passwords = if password.is_empty() {
                Ok(BTreeMap::new())
            } else {
                backup.open_passwords(&password).map_err(|e| e.to_string())
            };
            Ok(Some(ServiceEvent::RestorePrepared { backup, passwords }))
        });
    }
    pub fn browse(&self, key: &'static str) {
        self.job(move || {
            Ok(rfd::FileDialog::new()
                .pick_file()
                .map(|p| ServiceEvent::Preference(key, format!("\"{}\"", p.display()))))
        });
    }
    pub fn open(&self, path: PathBuf) {
        self.job(move || {
            open::that(path)?;
            Ok(None)
        });
    }
    pub fn url(&self, url: &'static str) {
        self.job(move || {
            open::that(url)?;
            Ok(None)
        });
    }
    pub fn logs_export(&self) {
        self.job(|| {
            let Some(dir) = crate::logger::dir() else {
                return Err("No log file".into());
            };
            let Some(path) = rfd::FileDialog::new()
                .set_file_name("nexssh.log")
                .save_file()
            else {
                return Ok(None);
            };
            let text = format!(
                "NexSSH {} · {} {}\n{}",
                env!("CARGO_PKG_VERSION"),
                std::env::consts::OS,
                std::env::consts::ARCH,
                crate::logger::read(&dir)?
            );
            std::fs::write(&path, text)?;
            Ok(Some(ServiceEvent::Notice(path.display().to_string())))
        });
    }
    pub fn check_updates(&mut self) {
        self.updater.check();
    }
}

fn discover_editors() -> Vec<(String, String)> {
    crate::editor::editors()
        .into_iter()
        .map(|e| (e.id, e.name))
        .collect()
}
