//! SFTP and external editing on the existing session connection.
use nexssh_core::{
    Core, SessionId,
    edit::{EditInfo, Edits, Resolution},
    sftp::{self, Entry},
};
use std::{
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    time::Instant,
};

pub enum Event {
    Listing {
        generation: u64,
        path: String,
        result: Result<Vec<Entry>, String>,
    },
    Message(String),
    Transfer {
        done: u64,
        total: u64,
    },
    TransferStarted(String),
    Finished {
        session: SessionId,
        result: Result<String, String>,
        cancelled: bool,
    },
    Changed {
        session: SessionId,
        result: Result<(), String>,
    },
    Edits(Vec<EditInfo>),
}
pub struct Files {
    pub session: SessionId,
    pub path: String,
    pub entries: Vec<Entry>,
    pub generation: u64,
    pub receiver: mpsc::Receiver<Event>,
    sender: mpsc::Sender<Event>,
    pub edits: Arc<Edits>,
    pub editing: Vec<EditInfo>,
    cancel: Arc<AtomicBool>,
    pub transferring: bool,
    pub transfer_started: Instant,
    pub transfer_local: Option<PathBuf>,
    pub initial_path: String,
    pub initial_source: String,
}
impl Files {
    pub fn new(core: &Core) -> Self {
        let (sender, receiver) = mpsc::channel();
        let tx = sender.clone();
        let edits = Edits::new(
            core.sessions.clone(),
            std::env::temp_dir().join("NexSSH-native-edit"),
            Box::new(move |list| {
                let _ = tx.send(Event::Edits(list));
            }),
        );
        Self {
            session: 0,
            path: String::new(),
            entries: vec![],
            generation: 0,
            receiver,
            sender,
            edits,
            editing: vec![],
            cancel: Arc::new(AtomicBool::new(false)),
            transferring: false,
            transfer_started: Instant::now(),
            transfer_local: None,
            initial_path: String::new(),
            initial_source: String::new(),
        }
    }
    pub fn list(
        &mut self,
        core: Arc<Core>,
        runtime: &tokio::runtime::Runtime,
        session: SessionId,
        path: String,
    ) {
        self.generation += 1;
        if self.session != session {
            self.initial_path.clear();
            self.initial_source = path.clone();
            self.entries.clear();
        }
        self.session = session;
        let generation = self.generation;
        let sender = self.sender.clone();
        runtime.spawn(async move {
            let mut resolved = path.clone();
            let result = async {
                let sftp = core.sessions.sftp(session).await?;
                resolved = if resolved.is_empty() {
                    sftp.home().await?
                } else {
                    sftp.resolve(&resolved).await?
                };
                sftp.list(&resolved).await
            }
            .await;
            let _ = sender.send(Event::Listing {
                generation,
                path: resolved,
                result: result.map_err(|e: nexssh_core::error::Error| e.to_string()),
            });
        });
    }
    pub fn download(&mut self, core: Arc<Core>, runtime: &tokio::runtime::Runtime, path: String) {
        if self.transferring {
            return;
        }
        self.transferring = true;
        self.cancel = Arc::new(AtomicBool::new(false));
        let (sender, session, cancel) =
            (self.sender.clone(), self.session, Arc::clone(&self.cancel));
        runtime.spawn(async move {
            let result = async {
                let selected = tokio::task::spawn_blocking(|| rfd::FileDialog::new().pick_folder())
                    .await
                    .map_err(|e| e.to_string())?;
                let Some(dir) = selected else {
                    return Ok(String::new());
                };
                let _ = sender.send(Event::TransferStarted(sftp::base_name(&path)));
                let sftp = core
                    .sessions
                    .sftp(session)
                    .await
                    .map_err(|e| e.to_string())?;
                sftp.download(
                    &path,
                    &dir,
                    &mut |p| {
                        let _ = sender.send(Event::Transfer {
                            done: p.done,
                            total: p.total,
                        });
                    },
                    &cancel,
                )
                .await
                .map(|p| p.display().to_string())
                .map_err(|e| e.to_string())
            }
            .await;
            let _ = sender.send(Event::Finished {
                session,
                result,
                cancelled: cancel.load(Ordering::Relaxed),
            });
        });
    }
    pub fn upload(&mut self, core: Arc<Core>, runtime: &tokio::runtime::Runtime, folder: bool) {
        if self.transferring {
            return;
        }
        self.transferring = true;
        self.cancel = Arc::new(AtomicBool::new(false));
        let (sender, session, cancel, dir) = (
            self.sender.clone(),
            self.session,
            Arc::clone(&self.cancel),
            self.path.clone(),
        );
        runtime.spawn(async move {
            let result = async {
                let selected = tokio::task::spawn_blocking(move || {
                    if folder {
                        rfd::FileDialog::new().pick_folder().map(|p| vec![p])
                    } else {
                        rfd::FileDialog::new().pick_files()
                    }
                })
                .await
                .map_err(|e| e.to_string())?;
                let Some(paths) = selected else {
                    return Ok(String::new());
                };
                let sftp = core
                    .sessions
                    .sftp(session)
                    .await
                    .map_err(|e| e.to_string())?;
                let mut replacing = vec![];
                for path in &paths {
                    let target = sftp::join(
                        &dir,
                        &path.file_name().unwrap_or_default().to_string_lossy(),
                    );
                    if sftp.stat(&target).await.is_ok() {
                        replacing.push(target);
                    }
                }
                if !replacing.is_empty() {
                    let confirmed = tokio::task::spawn_blocking(move || {
                        rfd::MessageDialog::new()
                            .set_title("NexSSH")
                            .set_description(format!(
                                "Replace existing files / Заменить существующие файлы?\n{}",
                                replacing.join("\n")
                            ))
                            .set_buttons(rfd::MessageButtons::YesNo)
                            .show()
                    })
                    .await
                    .map_err(|e| e.to_string())?;
                    if confirmed != rfd::MessageDialogResult::Yes {
                        return Ok(String::new());
                    }
                }
                for path in paths {
                    let _ = sender.send(Event::TransferStarted(
                        path.file_name()
                            .unwrap_or_default()
                            .to_string_lossy()
                            .into_owned(),
                    ));
                    sftp.upload_path(
                        &path,
                        &dir,
                        &mut |p| {
                            let _ = sender.send(Event::Transfer {
                                done: p.done,
                                total: p.total,
                            });
                        },
                        &cancel,
                    )
                    .await
                    .map_err(|e| e.to_string())?;
                }
                Ok("Transfer completed / Передача завершена".into())
            }
            .await;
            let _ = sender.send(Event::Finished {
                session,
                result,
                cancelled: cancel.load(Ordering::Relaxed),
            });
        });
    }
    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
    pub fn change(
        &self,
        core: Arc<Core>,
        runtime: &tokio::runtime::Runtime,
        session: SessionId,
        action: String,
        path: String,
        value: String,
    ) {
        let sender = self.sender.clone();
        runtime.spawn(async move {
            let result = async {
                let sftp = core
                    .sessions
                    .sftp(session)
                    .await
                    .map_err(|e| e.to_string())?;
                match action.as_str() {
                    "mkdir" => sftp
                        .mkdir(&sftp::join(&path, &value))
                        .await
                        .map_err(|e| e.to_string()),
                    "new-file" => {
                        let target = sftp::join(&path, &value);
                        sftp.new_file(&target).await.map_err(|e| e.to_string())
                    }
                    "rename" => {
                        let parent = path
                            .rsplit_once('/')
                            .map_or("/", |(p, _)| if p.is_empty() { "/" } else { p });
                        sftp.rename(&path, &sftp::join(parent, &value))
                            .await
                            .map_err(|e| e.to_string())
                    }
                    "delete" => sftp.remove(&path).await.map_err(|e| e.to_string()),
                    "chmod" => {
                        let mode = u32::from_str_radix(&value, 8).map_err(|e| e.to_string())?;
                        sftp.chmod(&path, mode, false)
                            .await
                            .map_err(|e| e.to_string())
                    }
                    _ => Err("Unknown file action".into()),
                }
            }
            .await;
            let _ = sender.send(Event::Changed { session, result });
        });
    }
    pub fn edit(
        &self,
        runtime: &tokio::runtime::Runtime,
        path: String,
        settings: serde_json::Value,
    ) {
        let (edits, sender, session) = (Arc::clone(&self.edits), self.sender.clone(), self.session);
        runtime.spawn(async move {
            match edits.open(session, &path).await {
                Ok(info) => {
                    let result = tokio::task::spawn_blocking(move || {
                        crate::editor::launch(&settings, &PathBuf::from(info.local_path))
                    })
                    .await;
                    if let Err(error) = result
                        .map_err(|e| e.to_string())
                        .and_then(|r| r.map_err(|e| e.to_string()))
                    {
                        let _ = sender.send(Event::Message(error));
                    }
                }
                Err(error) => {
                    let _ = sender.send(Event::Message(format!("{error:?}")));
                }
            }
        });
    }
    pub fn resolve(&self, runtime: &tokio::runtime::Runtime, id: u64, overwrite: bool) {
        let edits = Arc::clone(&self.edits);
        runtime.spawn(async move {
            edits
                .resolve(
                    id,
                    if overwrite {
                        Resolution::Overwrite
                    } else {
                        Resolution::Reload
                    },
                )
                .await;
        });
    }
}

/// The same units/precision as state/files.svelte.ts, with the UI language.
pub fn format_size(bytes: u64, lang: &str) -> String {
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < 4 {
        value /= 1024.0;
        unit += 1;
    }
    let text = if unit == 0 || value >= 100.0 {
        format!("{value:.0}")
    } else {
        format!("{value:.1}")
            .trim_end_matches('0')
            .trim_end_matches('.')
            .to_owned()
    };
    let text = if lang == "ru" {
        text.replace('.', ",")
    } else {
        text
    };
    format!(
        "{} {}",
        text,
        crate::i18n::tr(
            ["size.b", "size.kb", "size.mb", "size.gb", "size.tb"][unit],
            lang
        )
    )
}

pub fn format_date(seconds: Option<u64>, lang: &str) -> String {
    use chrono::{Datelike, Local, TimeZone};
    let Some(date) = seconds
        .filter(|n| *n > 0)
        .and_then(|n| i64::try_from(n).ok())
        .and_then(|n| Local.timestamp_opt(n, 0).single())
    else {
        return String::new();
    };
    let current = date.year() == Local::now().year();
    if lang == "ru" {
        let month = [
            "янв.",
            "февр.",
            "мар.",
            "апр.",
            "мая",
            "июн.",
            "июл.",
            "авг.",
            "сент.",
            "окт.",
            "нояб.",
            "дек.",
        ][date.month0() as usize];
        if current {
            format!("{} {month}, {}", date.day(), date.format("%H:%M"))
        } else {
            format!("{} {month} {} г.", date.day(), date.year())
        }
    } else if current {
        date.format("%b %-d, %I:%M %p").to_string()
    } else {
        date.format("%b %-d, %Y").to_string()
    }
}

pub fn valid_value(action: &str, value: &str) -> bool {
    match action {
        "chmod" => {
            (3..=4).contains(&value.len()) && value.bytes().all(|b| (b'0'..=b'7').contains(&b))
        }
        "mkdir" | "new-file" | "rename" => {
            !value.is_empty()
                && ![".", ".."].contains(&value)
                && !value
                    .chars()
                    .any(|c| c.is_control() || c == '/' || c == '\\')
        }
        "delete" => true,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn display_units_match_the_original_file_table_in_both_languages() {
        assert_eq!(format_size(0, "en"), "0 B");
        assert_eq!(format_size(1024, "en"), "1 KB");
        assert_eq!(format_size(2560, "ru"), "2,5 КБ");
        assert_eq!(format_size(1048576, "ru"), "1 МБ");
        assert_eq!(format_date(None, "en"), "");
        assert_eq!(format_date(Some(0), "ru"), "");
        assert_eq!(format_date(Some(u64::MAX), "en"), "");
        assert!(format_date(Some(946684800), "en").contains("2000"));
        assert!(format_date(Some(946684800), "ru").contains("2000"));
    }
    #[test]
    fn names_and_octal_permissions_cannot_escape_the_selected_directory() {
        for key in ["mkdir", "new-file", "rename"] {
            assert!(valid_value(key, "Папка с пробелами"));
            for name in ["", ".", "..", "../outside", "a/b", "a\\b", "a\nline", "a\0"] {
                assert!(!valid_value(key, name), "{key} {name:?}");
            }
        }
        for mode in ["000", "640", "755", "1777", "4755"] {
            assert!(valid_value("chmod", mode));
        }
        for mode in ["", "77", "888", "9999", "07777", "-01"] {
            assert!(!valid_value("chmod", mode));
        }
        assert!(valid_value("delete", ""));
    }
}
