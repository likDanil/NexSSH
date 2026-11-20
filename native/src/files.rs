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
};

pub enum Event {
    Listing {
        generation: u64,
        path: String,
        result: Result<Vec<Entry>, String>,
    },
    Message(String),
    Transfer(String),
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
        self.session = session;
        self.entries.clear();
        let generation = self.generation;
        let sender = self.sender.clone();
        runtime.spawn(async move {
            let mut resolved = path.clone();
            let result = async {
                let sftp = core.sessions.sftp(session).await?;
                if resolved.is_empty() {
                    resolved = sftp.home().await?;
                }
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
                let sftp = core
                    .sessions
                    .sftp(session)
                    .await
                    .map_err(|e| e.to_string())?;
                sftp.download(
                    &path,
                    &dir,
                    &mut |p| {
                        let _ = sender.send(Event::Transfer(format!("{} / {}", p.done, p.total)));
                    },
                    &cancel,
                )
                .await
                .map(|p| p.display().to_string())
                .map_err(|e| e.to_string())
            }
            .await;
            let _ = sender.send(Event::Message(result.unwrap_or_else(|e| e)));
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
                    sftp.upload_path(
                        &path,
                        &dir,
                        &mut |p| {
                            let _ =
                                sender.send(Event::Transfer(format!("{} / {}", p.done, p.total)));
                        },
                        &cancel,
                    )
                    .await
                    .map_err(|e| e.to_string())?;
                }
                Ok("Transfer completed / Передача завершена".into())
            }
            .await;
            let _ = sender.send(Event::Message(result.unwrap_or_else(|e| e)));
        });
    }
    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Relaxed);
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
