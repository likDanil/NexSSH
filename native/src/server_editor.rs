//! The 1.x server editor contract, including secrets kept outside servers.json.
use std::sync::{Arc, mpsc};

use nexssh_core::{
    Core, Server,
    model::{AgentAccess, AuthKind, ForwardKind, ForwardSpec},
    secrets::Secrets,
};
use slint::{Model, ModelRc, VecModel};

use crate::{AppWindow, Choice, ForwardRow, ServerForm, i18n};

#[derive(Debug)]
struct FormError {
    section: &'static str,
    message: String,
}

struct SaveRequest {
    server: Server,
    password: String,
    jump_password: String,
    clear_password: bool,
    clear_jump_password: bool,
    purge_unused_jump: bool,
}

struct Saved {
    server: Server,
    warning: Option<String>,
    access_changed: bool,
}

enum Message {
    Metadata(u64, Vec<nexssh_core::keys::KeyInfo>, bool, bool),
    Key(u64, Option<String>),
    Secret(u64, bool, Result<Option<String>, String>),
    Saved(u64, bool, Result<Saved, String>),
    Duplicated(Result<Saved, String>),
    Deleted(String, String, Result<Option<String>, String>),
}

pub enum Outcome {
    Saved {
        server: Box<Server>,
        connect: bool,
        access_changed: bool,
    },
    Deleted {
        id: String,
        name: String,
    },
}

pub struct Editor {
    initial: Server,
    generation: u64,
    keys: Vec<nexssh_core::keys::KeyInfo>,
    sender: mpsc::Sender<Message>,
    receiver: mpsc::Receiver<Message>,
}

impl Default for Editor {
    fn default() -> Self {
        let (sender, receiver) = mpsc::channel();
        Self {
            initial: Server::default(),
            generation: 0,
            keys: vec![],
            sender,
            receiver,
        }
    }
}

fn choice(value: impl Into<slint::SharedString>, label: impl Into<slint::SharedString>) -> Choice {
    Choice {
        value: value.into(),
        label: label.into(),
    }
}

fn form(server: &Server, core: &Core) -> ServerForm {
    ServerForm {
        id: server.id.clone().into(),
        name: server.name.clone().into(),
        host: server.host.clone().into(),
        port: server.port.to_string().into(),
        user: server.user.clone().into(),
        group: server.group.clone().into(),
        sftp_root: server.sftp_root.clone().unwrap_or_default().into(),
        auth: match server.auth {
            AuthKind::Auto => "auto",
            AuthKind::Password => "password",
            AuthKind::Key => "key",
            AuthKind::Agent => "agent",
        }
        .into(),
        identity_file: server.identity_file.clone().unwrap_or_default().into(),
        jump_host: server
            .jump_host
            .as_ref()
            .map(|v| {
                core.store
                    .find(v)
                    .map(|s| s.name)
                    .unwrap_or_else(|| v.clone())
            })
            .unwrap_or_default()
            .into(),
        jump_user: server.jump_user.clone().unwrap_or_default().into(),
        keepalive: server
            .keepalive_secs
            .map(|v| v.to_string())
            .unwrap_or_default()
            .into(),
        timeout: server
            .connect_timeout_secs
            .map(|v| v.to_string())
            .unwrap_or_default()
            .into(),
        commands: server.startup_commands.clone().unwrap_or_default().into(),
        agents: match server.agents {
            AgentAccess::Off => "off",
            AgentAccess::Ask => "ask",
            AgentAccess::Allow => "allow",
        }
        .into(),
        ..Default::default()
    }
}

fn forward_row(f: &ForwardSpec) -> ForwardRow {
    ForwardRow {
        kind: match f.kind {
            ForwardKind::Local => "local",
            ForwardKind::Remote => "remote",
            ForwardKind::Dynamic => "dynamic",
        }
        .into(),
        bind_host: f.bind_host.clone().into(),
        bind_port: f.bind_port.to_string().into(),
        target_host: f.target_host.clone().into(),
        target_port: f.target_port.to_string().into(),
    }
}

enum Jump {
    None,
    Saved { id: String, name: String },
    Chain,
    Typed,
}
fn jump(core: &Core, value: &str) -> Jump {
    let v = value.trim();
    if v.is_empty() || v.eq_ignore_ascii_case("none") {
        Jump::None
    } else if v.contains(',') {
        Jump::Chain
    } else if let Some(server) = core.store.find(v) {
        Jump::Saved {
            id: server.id,
            name: server.name,
        }
    } else {
        Jump::Typed
    }
}

fn error(section: &'static str, message: impl ToString) -> FormError {
    FormError {
        section,
        message: message.to_string(),
    }
}
fn optional(value: &str) -> Option<String> {
    let value = value.trim();
    (!value.is_empty()).then(|| value.to_owned())
}
fn number<T: std::str::FromStr>(
    value: &str,
    section: &'static str,
    label: &str,
) -> Result<T, FormError> {
    value
        .trim()
        .parse()
        .map_err(|_| error(section, format!("{label}: 0–65535")))
}
fn optional_seconds(value: &str, label: &str) -> Result<Option<u32>, FormError> {
    if value.trim().is_empty() {
        Ok(None)
    } else {
        value
            .trim()
            .parse()
            .map(Some)
            .map_err(|_| error("connection", format!("{label}: 0–4294967295")))
    }
}

fn request(
    core: &Core,
    initial: &Server,
    f: &ServerForm,
    forwards: impl Iterator<Item = ForwardRow>,
    lang: &str,
) -> Result<SaveRequest, FormError> {
    let mut server = initial.clone();
    server.name = f.name.to_string();
    server.host = f.host.to_string();
    server.user = f.user.to_string();
    server.group = f.group.to_string();
    server.port = number(&f.port, "general", &i18n::tr("editor.port", lang))?;
    server.auth = match f.auth.as_str() {
        "password" => AuthKind::Password,
        "key" => AuthKind::Key,
        "agent" => AuthKind::Agent,
        _ => AuthKind::Auto,
    };
    server.identity_file = if matches!(server.auth, AuthKind::Auto | AuthKind::Key) {
        optional(&f.identity_file)
    } else {
        None
    };
    // Validate general and login before forwarding and connection fields, like the original tab order.
    server.forwards.clear();
    server.jump_user = None;
    let auth = server.auth;
    server.auth = AuthKind::Auto;
    server.normalize().map_err(|e| error("general", e))?;
    server.auth = auth;
    if auth == AuthKind::Key && server.identity_file.is_none() {
        return Err(error("login", nexssh_core::i18n::key_file_required()));
    }
    let typed = matches!(jump(core, &f.jump_host), Jump::Typed);
    let mut jump_host = f.jump_host.trim().to_owned();
    let mut jump_user = f.jump_user.trim().to_owned();
    if typed && let Some((user, host)) = jump_host.rsplit_once('@') {
        if jump_user.is_empty() {
            jump_user = user.to_owned();
        }
        jump_host = host.to_owned();
    }
    server.jump_host = match jump(core, &jump_host) {
        Jump::None => None,
        Jump::Saved { id, .. } => Some(id),
        _ => optional(&jump_host),
    };
    server.jump_user = if typed { optional(&jump_user) } else { None };
    if !server.id.is_empty() && server.jump_host.as_ref() == Some(&server.id) {
        return Err(error(
            "connection",
            i18n::tr("editor.jumpHost", lang) + ": " + &server.name,
        ));
    }
    server.keepalive_secs = optional_seconds(&f.keepalive, &i18n::tr("editor.keepalive", lang))?;
    server.connect_timeout_secs = optional_seconds(&f.timeout, &i18n::tr("editor.timeout", lang))?;
    for (index, f) in forwards.enumerate() {
        let kind = match f.kind.as_str() {
            "remote" => ForwardKind::Remote,
            "dynamic" => ForwardKind::Dynamic,
            _ => ForwardKind::Local,
        };
        let mut forward = ForwardSpec {
            kind,
            bind_host: f.bind_host.to_string(),
            bind_port: number(
                &f.bind_port,
                "forwarding",
                &i18n::tr("editor.localPort", lang),
            )?,
            target_host: f.target_host.to_string(),
            target_port: if kind == ForwardKind::Dynamic {
                0
            } else {
                number(&f.target_port, "forwarding", &i18n::tr("editor.port", lang))?
            },
        };
        forward
            .normalize()
            .map_err(|e| error("forwarding", format!("{}: {e}", index + 1)))?;
        server.forwards.push(forward);
    }
    server.startup_commands = (!f.commands.trim().is_empty()).then(|| f.commands.to_string());
    server.sftp_root = optional(&f.sftp_root);
    server.agents = match f.agents.as_str() {
        "ask" => AgentAccess::Ask,
        "allow" => AgentAccess::Allow,
        _ => AgentAccess::Off,
    };
    server.normalize().map_err(|e| error("connection", e))?;
    Ok(SaveRequest {
        server,
        password: if f.auth == "password" && !f.forget_password {
            f.password.to_string()
        } else {
            String::new()
        },
        jump_password: if typed && !f.forget_jump_password {
            f.jump_password.to_string()
        } else {
            String::new()
        },
        clear_password: f.forget_password,
        clear_jump_password: f.forget_jump_password,
        purge_unused_jump: !typed && !initial.id.is_empty(),
    })
}

fn save(core: &Core, req: SaveRequest) -> Result<Saved, String> {
    let access_changed =
        core.store.get(&req.server.id).map(|s| s.agents) != Some(req.server.agents);
    let server = core
        .store
        .save_server(req.server)
        .map_err(|e| e.to_string())?;
    let mut warnings = vec![];
    // An unused jump password is removed even if its metadata arrived after Save.
    let clear_jump = req.clear_jump_password
        || (req.purge_unused_jump
            && core
                .secrets
                .has(&Secrets::jump_password_account(&server.id)));
    for (account, secret, clear) in [
        (
            Secrets::password_account(&server.id),
            req.password,
            req.clear_password,
        ),
        (
            Secrets::jump_password_account(&server.id),
            req.jump_password,
            clear_jump,
        ),
    ] {
        let result = if !secret.is_empty() {
            core.secrets.set(&account, &secret)
        } else if clear {
            core.secrets.delete(&account)
        } else {
            Ok(())
        };
        if let Err(e) = result {
            warnings.push(e.to_string());
        }
    }
    Ok(Saved {
        server,
        access_changed,
        warning: (!warnings.is_empty())
            .then(|| nexssh_core::i18n::password_not_stored(warnings.join("; "))),
    })
}

impl Editor {
    pub fn duplicate(&self, core: Arc<Core>, server: Server) {
        let tx = self.sender.clone();
        std::thread::spawn(move || {
            let request = SaveRequest {
                server,
                password: String::new(),
                jump_password: String::new(),
                clear_password: false,
                clear_jump_password: false,
                purge_unused_jump: false,
            };
            let _ = tx.send(Message::Duplicated(save(&core, request)));
        });
    }
    pub fn show(&mut self, ui: &AppWindow, core: Arc<Core>, existing: Option<Server>, lang: &str) {
        self.cancel(ui);
        self.initial = existing.unwrap_or_default();
        ui.set_server_form(form(&self.initial, &core));
        ui.set_server_forwards(ModelRc::new(VecModel::from(
            self.initial
                .forwards
                .iter()
                .map(forward_row)
                .collect::<Vec<_>>(),
        )));
        ui.set_server_section("general".into());
        ui.set_server_error("".into());
        ui.set_server_error_section("".into());
        ui.set_server_busy(false);
        let snapshot = core.store.snapshot();
        let mut groups = snapshot.groups;
        for key in ["group.production", "group.development", "group.personal"] {
            let value = i18n::tr(key, lang);
            if !groups.contains(&value) {
                groups.push(value);
            }
        }
        ui.set_server_groups(ModelRc::new(VecModel::from(
            groups
                .into_iter()
                .map(|s| choice(s.clone(), s))
                .collect::<Vec<_>>(),
        )));
        ui.set_server_jumps(ModelRc::new(VecModel::from(
            snapshot
                .servers
                .into_iter()
                .filter(|s| s.id != self.initial.id)
                .map(|s| choice(s.name.clone(), format!("{} · {}", s.name, s.host)))
                .collect::<Vec<_>>(),
        )));
        ui.set_overlay("server".into());
        self.derived(ui, &core, lang);
        let generation = self.generation;
        let id = self.initial.id.clone();
        let tx = self.sender.clone();
        std::thread::spawn(move || {
            let keys = nexssh_core::keys::list_local_keys();
            let password = !id.is_empty() && core.secrets.has(&Secrets::password_account(&id));
            let jump = !id.is_empty() && core.secrets.has(&Secrets::jump_password_account(&id));
            let _ = tx.send(Message::Metadata(generation, keys, password, jump));
        });
    }

    pub fn cancel(&mut self, ui: &AppWindow) {
        self.generation = self.generation.wrapping_add(1);
        ui.set_server_form(ServerForm::default());
        ui.set_server_forwards(ModelRc::default());
        self.keys.clear();
        ui.set_server_keys(ModelRc::default());
        ui.set_server_key_detail("".into());
    }

    pub fn derived(&self, ui: &AppWindow, core: &Core, lang: &str) {
        let f = ui.get_server_form();
        let kind = jump(core, &f.jump_host);
        ui.set_server_jump_typed(matches!(kind, Jump::Typed));
        ui.set_server_jump_hint(
            match kind {
                Jump::Saved { name, .. } => {
                    i18n::tr("editor.jumpSaved", lang).replace("{name}", &name)
                }
                Jump::Chain => i18n::tr("editor.jumpChain", lang),
                _ => String::new(),
            }
            .into(),
        );
        ui.set_server_key_detail(
            self.keys
                .iter()
                .find(|k| k.path == f.identity_file.as_str())
                .map(|k| {
                    format!(
                        "{}{}\n{}",
                        k.key_type,
                        if k.encrypted {
                            format!(" · {}", i18n::tr("editor.keyProtected", lang))
                        } else {
                            String::new()
                        },
                        k.fingerprint.as_deref().unwrap_or("")
                    )
                })
                .unwrap_or_default()
                .into(),
        );
    }

    pub fn action(&mut self, ui: &AppWindow, core: Arc<Core>, action: &str, lang: &str) {
        if ui.get_server_busy() {
            return;
        }
        match action {
            "browse-key" => {
                let tx = self.sender.clone();
                let generation = self.generation;
                let title = i18n::tr("editor.pickKey", lang);
                std::thread::spawn(move || {
                    let mut picker = rfd::FileDialog::new().set_title(title);
                    if let Some(dir) = nexssh_core::ssh_dir() {
                        picker = picker.set_directory(dir);
                    }
                    let value = picker.pick_file().map(|p| nexssh_core::contract_tilde(&p));
                    let _ = tx.send(Message::Key(generation, value));
                });
            }
            "reveal-password" | "reveal-jump-password" => {
                let id = self.initial.id.clone();
                if id.is_empty() {
                    return;
                }
                let generation = self.generation;
                let tx = self.sender.clone();
                let jump = action == "reveal-jump-password";
                std::thread::spawn(move || {
                    let account = if jump {
                        Secrets::jump_password_account(&id)
                    } else {
                        Secrets::password_account(&id)
                    };
                    let result = core
                        .secrets
                        .get(&account)
                        .map(|v| v.map(|s| s.to_string()))
                        .map_err(|e| e.to_string());
                    let _ = tx.send(Message::Secret(generation, jump, result));
                });
            }
            "add-forward" => {
                let rows = ui.get_server_forwards();
                let mut rows = rows.iter().collect::<Vec<_>>();
                rows.push(ForwardRow {
                    kind: "local".into(),
                    bind_host: "127.0.0.1".into(),
                    bind_port: "0".into(),
                    target_host: "localhost".into(),
                    target_port: "0".into(),
                });
                ui.set_server_forwards(ModelRc::new(VecModel::from(rows)));
            }
            _ => {
                if let Some(index) = action
                    .strip_prefix("remove-forward:")
                    .and_then(|s| s.parse::<usize>().ok())
                {
                    let rows = ui.get_server_forwards();
                    ui.set_server_forwards(ModelRc::new(VecModel::from(
                        rows.iter()
                            .enumerate()
                            .filter_map(|(i, f)| (i != index).then_some(f))
                            .collect::<Vec<_>>(),
                    )));
                }
            }
        }
    }

    pub fn save(&self, ui: &AppWindow, core: Arc<Core>, connect: bool, lang: &str) {
        if ui.get_server_busy() {
            return;
        }
        match request(
            &core,
            &self.initial,
            &ui.get_server_form(),
            ui.get_server_forwards().iter(),
            lang,
        ) {
            Ok(req) => {
                ui.set_server_busy(true);
                ui.set_server_error("".into());
                let tx = self.sender.clone();
                let generation = self.generation;
                std::thread::spawn(move || {
                    let _ = tx.send(Message::Saved(generation, connect, save(&core, req)));
                });
            }
            Err(e) => {
                ui.set_server_section(e.section.into());
                ui.set_server_error_section(e.section.into());
                ui.set_server_error(e.message.into());
            }
        }
    }

    pub fn delete(&self, core: Arc<Core>, id: String, name: String) {
        let tx = self.sender.clone();
        std::thread::spawn(move || {
            let result = core
                .store
                .delete_server(&id)
                .map_err(|e| e.to_string())
                .map(|_| {
                    let errors = [
                        Secrets::password_account(&id),
                        Secrets::jump_password_account(&id),
                    ]
                    .iter()
                    .filter_map(|a| core.secrets.delete(a).err().map(|e| e.to_string()))
                    .collect::<Vec<_>>();
                    (!errors.is_empty()).then(|| errors.join("; "))
                });
            let _ = tx.send(Message::Deleted(id, name, result));
        });
    }

    pub fn poll(&mut self, ui: &AppWindow) -> Vec<Outcome> {
        let mut outcomes = vec![];
        while let Ok(event) = self.receiver.try_recv() {
            match event {
                Message::Metadata(g, keys, password, jump)
                    if g == self.generation && ui.get_overlay() == "server" =>
                {
                    ui.set_server_keys(ModelRc::new(VecModel::from(
                        keys.iter()
                            .map(|k| {
                                choice(
                                    k.path.clone(),
                                    format!(
                                        "{} · {}{}",
                                        k.path,
                                        k.key_type,
                                        if k.encrypted { " 🔒" } else { "" }
                                    ),
                                )
                            })
                            .collect::<Vec<_>>(),
                    )));
                    self.keys = keys;
                    let mut f = ui.get_server_form();
                    f.has_password = password;
                    f.has_jump_password = jump;
                    ui.set_server_form(f);
                }
                Message::Key(g, Some(path))
                    if g == self.generation && ui.get_overlay() == "server" =>
                {
                    let mut f = ui.get_server_form();
                    f.identity_file = path.into();
                    ui.set_server_form(f);
                }
                Message::Secret(g, jump, result)
                    if g == self.generation && ui.get_overlay() == "server" =>
                {
                    match result {
                        Ok(Some(secret)) => {
                            let mut f = ui.get_server_form();
                            if jump {
                                if f.jump_password.is_empty() && !f.forget_jump_password {
                                    f.jump_password = secret.into();
                                }
                            } else if f.password.is_empty() && !f.forget_password {
                                f.password = secret.into();
                            }
                            ui.set_server_form(f);
                        }
                        Err(e) => ui.set_server_error(e.into()),
                        _ => {}
                    }
                }
                Message::Saved(g, connect, result) => {
                    let current = g == self.generation && ui.get_overlay() == "server";
                    match result {
                        Ok(saved) => {
                            if let Some(w) = saved.warning {
                                ui.set_notice(w.into());
                            }
                            if current {
                                ui.set_server_busy(false);
                                ui.invoke_close_overlay();
                            }
                            outcomes.push(Outcome::Saved {
                                server: Box::new(saved.server),
                                connect: current && connect,
                                access_changed: saved.access_changed,
                            });
                        }
                        Err(e) => {
                            if current {
                                ui.set_server_busy(false);
                                ui.set_server_error(e.into());
                            } else {
                                ui.set_notice(e.into());
                            }
                        }
                    }
                }
                Message::Deleted(id, name, result) => match result {
                    Ok(warning) => {
                        if let Some(w) = warning {
                            ui.set_notice(w.into());
                        }
                        outcomes.push(Outcome::Deleted { id, name });
                    }
                    Err(e) => ui.set_notice(e.into()),
                },
                Message::Duplicated(result) => match result {
                    Ok(saved) => outcomes.push(Outcome::Saved {
                        server: Box::new(saved.server),
                        connect: false,
                        access_changed: saved.access_changed,
                    }),
                    Err(error) => ui.set_notice(error.into()),
                },
                _ => {}
            }
        }
        outcomes
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn setup() -> (Core, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        (
            Core::with_secrets(dir.path(), Secrets::in_memory()).unwrap(),
            dir,
        )
    }

    fn valid(core: &Core) -> (Server, ServerForm) {
        let server = Server {
            host: "server.example".into(),
            ..Default::default()
        };
        let f = form(&server, core);
        (server, f)
    }

    #[test]
    fn saves_all_tabs_without_losing_import_identity_or_history() {
        let (core, dir) = setup();
        let initial = core
            .store
            .save_server(Server {
                host: "old.example".into(),
                alias: Some("ssh-import".into()),
                last_used: Some(1234),
                ..Default::default()
            })
            .unwrap();
        let mut f = form(&initial, &core);
        f.host = " new.example ".into();
        f.name = " My server ".into();
        f.port = "2222".into();
        f.user = "ops".into();
        f.group = "Production".into();
        f.auth = "key".into();
        f.identity_file = " ~/.ssh/id_ed25519 ".into();
        f.jump_host = "jump.example:2222".into();
        f.jump_user = "jump-ops".into();
        f.keepalive = "0".into();
        f.timeout = "900".into();
        f.commands = "  cd /var/www\n\n tmux new -A -s main  \n".into();
        f.sftp_root = " ~/www ".into();
        f.agents = "ask".into();
        let forwards = vec![
            ForwardRow {
                kind: "remote".into(),
                bind_host: "0.0.0.0".into(),
                bind_port: "0".into(),
                target_host: "localhost".into(),
                target_port: "8080".into(),
            },
            ForwardRow {
                kind: "dynamic".into(),
                bind_host: "127.0.0.1".into(),
                bind_port: "1080".into(),
                ..Default::default()
            },
        ];
        let req = request(&core, &initial, &f, forwards.into_iter(), "en").unwrap();
        let saved = save(&core, req).ok().unwrap().server;
        assert_eq!(saved.id, initial.id);
        assert_eq!(saved.alias, initial.alias);
        assert_eq!(saved.last_used, initial.last_used);
        assert_eq!(saved.host, "new.example");
        assert_eq!(saved.name, "My server");
        assert_eq!(saved.port, 2222);
        assert_eq!(saved.user, "ops");
        assert_eq!(saved.group, "Production");
        assert_eq!(saved.identity_file.as_deref(), Some("~/.ssh/id_ed25519"));
        assert_eq!(saved.jump_user.as_deref(), Some("jump-ops"));
        assert_eq!(saved.keepalive_secs, Some(0));
        assert_eq!(saved.connect_timeout_secs, Some(600));
        assert_eq!(saved.sftp_root.as_deref(), Some("~/www"));
        assert_eq!(
            saved.startup_commands.as_deref(),
            Some("  cd /var/www\n tmux new -A -s main")
        );
        assert_eq!(saved.agents, AgentAccess::Ask);
        assert_eq!(saved.forwards[0].bind_host, "0.0.0.0");
        assert_eq!(saved.forwards[0].bind_port, 0);
        assert_eq!(saved.forwards[1].kind, ForwardKind::Dynamic);
        assert_eq!(
            nexssh_core::ServerStore::open(dir.path().join("servers.json"))
                .unwrap()
                .get(&initial.id)
                .unwrap(),
            saved
        );
    }

    #[test]
    fn validation_selects_the_broken_tab_and_does_not_write() {
        let (core, _dir) = setup();
        let (server, mut f) = valid(&core);
        f.port = "70000".into();
        assert_eq!(
            request(&core, &server, &f, std::iter::empty(), "en")
                .err()
                .unwrap()
                .section,
            "general"
        );
        f.port = "22".into();
        f.auth = "key".into();
        assert_eq!(
            request(&core, &server, &f, std::iter::empty(), "en")
                .err()
                .unwrap()
                .section,
            "login"
        );
        f.auth = "auto".into();
        f.keepalive = "-1".into();
        assert_eq!(
            request(&core, &server, &f, std::iter::empty(), "en")
                .err()
                .unwrap()
                .section,
            "connection"
        );
        f.keepalive = "".into();
        let forward = ForwardRow {
            kind: "local".into(),
            bind_port: "0".into(),
            target_host: "localhost".into(),
            target_port: "80".into(),
            ..Default::default()
        };
        assert_eq!(
            request(&core, &server, &f, std::iter::once(forward), "en")
                .err()
                .unwrap()
                .section,
            "forwarding"
        );
        assert!(core.store.snapshot().servers.is_empty());
    }

    #[test]
    fn jump_reference_survives_rename_and_typed_login_is_split() {
        let (core, _dir) = setup();
        let (server, mut f) = valid(&core);
        let bastion = core
            .store
            .save_server(Server {
                host: "bastion.example".into(),
                name: "Gateway".into(),
                ..Default::default()
            })
            .unwrap();
        f.jump_host = "Gateway".into();
        f.jump_user = "ignored".into();
        let req = request(&core, &server, &f, std::iter::empty(), "en").unwrap();
        assert_eq!(req.server.jump_host, Some(bastion.id.clone()));
        assert_eq!(req.server.jump_user, None);
        assert!(req.jump_password.is_empty());
        f.jump_host = "ops@bastion:2222".into();
        f.jump_user = "".into();
        let req = request(&core, &server, &f, std::iter::empty(), "en").unwrap();
        assert_eq!(req.server.jump_host.as_deref(), Some("bastion:2222"));
        assert_eq!(req.server.jump_user.as_deref(), Some("ops"));
        f.jump_host = "hop1,ops@hop2".into();
        let req = request(&core, &server, &f, std::iter::empty(), "en").unwrap();
        assert_eq!(req.server.jump_host.as_deref(), Some("hop1,ops@hop2"));
        assert!(req.server.jump_user.is_none());
        f.jump_host = "none".into();
        assert!(
            request(&core, &server, &f, std::iter::empty(), "en")
                .unwrap()
                .server
                .jump_host
                .is_none()
        );
    }

    #[test]
    fn secrets_can_be_kept_replaced_and_forgotten_without_entering_json() {
        let (core, dir) = setup();
        let (server, mut f) = valid(&core);
        f.auth = "password".into();
        f.password = "unique-ssh-secret".into();
        f.jump_host = "jump.example".into();
        f.jump_password = "unique-jump-secret".into();
        let saved = save(
            &core,
            request(&core, &server, &f, std::iter::empty(), "en").unwrap(),
        )
        .ok()
        .unwrap()
        .server;
        let account = Secrets::password_account(&saved.id);
        let jump_account = Secrets::jump_password_account(&saved.id);
        assert_eq!(
            core.secrets
                .get(&account)
                .unwrap()
                .as_deref()
                .map(|s| s.as_str()),
            Some("unique-ssh-secret")
        );
        let mut f = form(&saved, &core);
        f.name = "Renamed".into();
        save(
            &core,
            request(&core, &saved, &f, std::iter::empty(), "en").unwrap(),
        )
        .ok()
        .unwrap();
        assert!(core.secrets.has(&account));
        assert!(core.secrets.has(&jump_account));
        f.password = "replacement-secret".into();
        f.jump_host = "".into();
        save(
            &core,
            request(&core, &saved, &f, std::iter::empty(), "en").unwrap(),
        )
        .ok()
        .unwrap();
        assert!(!core.secrets.has(&jump_account));
        assert_eq!(
            core.secrets
                .get(&account)
                .unwrap()
                .as_deref()
                .map(|s| s.as_str()),
            Some("replacement-secret")
        );
        f.password = "".into();
        f.forget_password = true;
        save(
            &core,
            request(&core, &saved, &f, std::iter::empty(), "en").unwrap(),
        )
        .ok()
        .unwrap();
        assert!(!core.secrets.has(&account));
        let json = std::fs::read_to_string(dir.path().join("servers.json")).unwrap();
        for secret in [
            "unique-ssh-secret",
            "unique-jump-secret",
            "replacement-secret",
        ] {
            assert!(!json.contains(secret));
        }
    }

    #[test]
    fn late_secret_lookup_cannot_populate_another_server_or_a_closed_form() {
        i_slint_backend_testing::init_no_event_loop();
        let ui = AppWindow::new().unwrap();
        let mut editor = Editor::default();
        ui.set_overlay("server".into());
        ui.set_server_form(ServerForm {
            id: "first".into(),
            ..Default::default()
        });
        let old_generation = editor.generation;
        editor.cancel(&ui);
        ui.set_server_form(ServerForm {
            id: "second".into(),
            ..Default::default()
        });
        editor
            .sender
            .send(Message::Secret(
                old_generation,
                false,
                Ok(Some("first-secret".into())),
            ))
            .unwrap();
        editor
            .sender
            .send(Message::Metadata(old_generation, vec![], true, true))
            .unwrap();
        editor.poll(&ui);
        assert!(ui.get_server_form().password.is_empty());
        assert!(!ui.get_server_form().has_password);
        ui.set_overlay("".into());
        editor
            .sender
            .send(Message::Secret(
                editor.generation,
                false,
                Ok(Some("second-secret".into())),
            ))
            .unwrap();
        editor.poll(&ui);
        assert!(ui.get_server_form().password.is_empty());
    }

    #[test]
    fn failed_save_keeps_entered_values_and_allows_retry() {
        i_slint_backend_testing::init_no_event_loop();
        let ui = AppWindow::new().unwrap();
        let mut editor = Editor::default();
        ui.set_overlay("server".into());
        ui.set_server_busy(true);
        ui.set_server_form(ServerForm {
            name: "Edited name".into(),
            host: "example.org".into(),
            ..Default::default()
        });
        editor
            .sender
            .send(Message::Saved(
                editor.generation,
                false,
                Err("Disk is full".into()),
            ))
            .unwrap();
        assert!(editor.poll(&ui).is_empty());
        assert!(!ui.get_server_busy());
        assert_eq!(ui.get_server_form().name, "Edited name");
        assert_eq!(ui.get_overlay(), "server");
        assert_eq!(ui.get_server_error(), "Disk is full");
    }
}
